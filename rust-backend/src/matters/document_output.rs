//! Detached PDF output and working-copy overlay primitives.
use super::{
    annotations,
    artifacts::{refusal, Artifact, PdfSnapshot, PdfSource},
    page_tools::{assembly, signature_guard},
    reading, *,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use pdf_oxide::{editor::DocumentEditor, object::Object, PdfDocument};
use std::{collections::HashMap, fmt::Write as _, io::Cursor};

mod appearance_paint;
mod base14_bounds;
mod font_glyphs;
mod helvetica_bounds;
mod jpx_redaction;
mod native_image_redaction;
pub(super) mod source_codes;
mod source_paint;
pub(super) mod textbox;
mod type3_paint;
mod visible_text;
pub(super) use source_paint::{embedded_paragraph_bounds, EmbeddedParagraphFont};
use textbox::Textbox;
pub(super) use visible_text::non_opaque_text;

const IMAGE_LIMIT: u64 = 40_000_000;
const OUTPUT_ERROR: &str = "Unable to prepare PDF output";

pub(super) fn binary(bytes: Vec<u8>, mime: &str) -> Reply {
    reply(
        200,
        json!({"data":STANDARD.encode(bytes),"media_type":mime}),
    )
}

pub(super) fn decode_image(
    bytes: &[u8],
    format: ImageFormat,
) -> Result<DynamicImage, &'static str> {
    read_image(bytes, format).map_err(|error| match error {
        ImageDecodeError::Header(_) => "Cannot read image",
        ImageDecodeError::Pixels(_) => "Cannot decode image",
        ImageDecodeError::Empty | ImageDecodeError::TooLarge => {
            "Overlay image exceeds the 40-megapixel safety limit"
        }
    })
}

enum ImageDecodeError {
    Header(image::ImageError),
    Pixels(image::ImageError),
    Empty,
    TooLarge,
}

pub(super) fn decode_overlay_image(
    bytes: &[u8],
    format: ImageFormat,
) -> Result<DynamicImage, Reply> {
    read_image(bytes, format).map_err(|error| match error {
        ImageDecodeError::Header(cause) | ImageDecodeError::Pixels(cause) => {
            refusal(422, format!("Cannot read overlay image: {cause}"))
        }
        ImageDecodeError::Empty => refusal(422, "Overlay image has no visible pixels"),
        ImageDecodeError::TooLarge => {
            refusal(422, "Overlay image exceeds the 40-megapixel safety limit")
        }
    })
}

fn read_image(bytes: &[u8], format: ImageFormat) -> Result<DynamicImage, ImageDecodeError> {
    let reader = ImageReader::with_format(Cursor::new(bytes), format);
    let (width, height) = reader.into_dimensions().map_err(ImageDecodeError::Header)?;
    if width == 0 || height == 0 {
        return Err(ImageDecodeError::Empty);
    }
    if u64::from(width) * u64::from(height) > IMAGE_LIMIT {
        return Err(ImageDecodeError::TooLarge);
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(width);
    limits.max_image_height = Some(height);
    limits.max_alloc = Some(384 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().map_err(ImageDecodeError::Pixels)
}

fn supported_image_format(bytes: &[u8]) -> Result<ImageFormat, &'static str> {
    let format = image::guess_format(bytes).map_err(|_| "Cannot read image")?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
        return Err("Choose a PNG or JPEG image");
    }
    Ok(format)
}

pub(super) fn normalize_image(bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
    let format = supported_image_format(bytes)?;
    let decoded = decode_image(bytes, format)?;
    super::image_import::to_pdf(bytes, format, decoded)
}

#[derive(Clone, Copy)]
struct BoxOnPage {
    left: f64,
    bottom: f64,
    width: f64,
    height: f64,
}

impl BoxOnPage {
    fn relative(item: &Value, width: f64, height: f64) -> Result<Self, &'static str> {
        let val = |name: &str| item[name].as_f64().ok_or("Invalid annotation region");
        let x = val("rect_x")?;
        let y = val("rect_y")?;
        let w = val("rect_width")?;
        let h = val("rect_height")?;
        if x < 0.0 || y < 0.0 || w <= 0.0 || h <= 0.0 || x + w > 1.0 || y + h > 1.0 {
            return Err("Invalid annotation region");
        }
        Ok(Self {
            left: width * x,
            bottom: height * (1.0 - y - h),
            width: width * w,
            height: height * h,
        })
    }

    fn point(self, x: f64, y: f64) -> (f64, f64) {
        (
            self.left + self.width * x,
            self.bottom + self.height * (1.0 - y),
        )
    }

    fn stroke(self) -> f64 {
        (self.width.min(self.height) * 0.04).clamp(1.0, 3.0)
    }

    fn bounds(self) -> [f64; 4] {
        [
            self.left,
            self.bottom,
            self.left + self.width,
            self.bottom + self.height,
        ]
    }

    fn transformed(self, matrix: [f64; 6]) -> Self {
        let [left, bottom, right, top] = assembly::transform_box(matrix, self.bounds());
        Self {
            left,
            bottom,
            width: right - left,
            height: top - bottom,
        }
    }
}

fn color(mark: &Value) -> (f64, f64, f64) {
    match mark["color"].as_str() {
        Some("green") => (0.15, 0.48, 0.22),
        Some("blue") => (0.09, 0.41, 0.67),
        Some("pink") => (0.7, 0.29, 0.47),
        _ => (0.68, 0.48, 0.0),
    }
}

fn path(commands: &mut String, points: impl IntoIterator<Item = (f64, f64)>) {
    for (index, (x, y)) in points.into_iter().enumerate() {
        let name = if index == 0 { "m" } else { "l" };
        write!(commands, "{x:.5} {y:.5} {name} ").unwrap();
    }
    commands.push_str("S\n");
}

fn hex_pdf(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            write!(text, "{byte:02x}").unwrap();
            text
        })
}

fn font(style: &Value) -> &'static str {
    match (style["font_family"].as_str(), style["bold"] == true) {
        (Some("tiro"), false) => "Times-Roman",
        (Some("tiro"), true) => "Times-Bold",
        (Some("cour"), false) => "Courier",
        (Some("cour"), true) => "Courier-Bold",
        (_, false) => "Helvetica",
        (_, true) => "Helvetica-Bold",
    }
}

fn text_font(overlay: &mut assembly::Overlay, name: &'static str) -> usize {
    if let Some(index) = overlay.fonts.iter().position(|font| *font == name) {
        index
    } else {
        let index = overlay.fonts.len();
        overlay.fonts.push(name);
        index
    }
}

pub(super) fn append_text(
    overlay: &mut assembly::Overlay,
    name: &'static str,
    size: f64,
    x: f64,
    y: f64,
    color: (f64, f64, f64),
    bytes: &[u8],
) {
    let index = text_font(overlay, name);
    let (r, g, b) = color;
    let encoded = hex_pdf(bytes);
    writeln!(overlay.commands, "{r:.5} {g:.5} {b:.5} rg BT /@F{index} {size:.5} Tf 1 0 0 1 {x:.5} {y:.5} Tm <{encoded}> Tj ET").unwrap();
}

pub(super) fn text_lines<'a>(
    label: &'a str,
    width: f64,
    measure: impl Fn(&str) -> f64,
) -> Option<Vec<&'a str>> {
    let mut lines = Vec::new();
    for line in label.split('\n') {
        let mut rest = line;
        while measure(rest) > width {
            let index = rest
                .match_indices(' ')
                .map(|(index, _)| index)
                .rev()
                .find(|&index| index > 0 && measure(rest[..index].trim_end()) <= width)?;
            lines.push(rest[..index].trim_end());
            rest = rest[index..].trim_start();
        }
        lines.push(rest);
    }
    Some(lines)
}

fn text_mark(
    overlay: &mut assembly::Overlay,
    item: &Value,
    area: BoxOnPage,
    kind: &str,
    (r, g, b): (f64, f64, f64),
    textbox: Textbox,
    rotation: i32,
) -> Result<(), &'static str> {
    let label = match kind {
        "check" => "v",
        "cross" => "X",
        _ => item["body"].as_str().ok_or("Invalid mark text")?,
    };
    let style = &item["text_style"];
    let fixed = style["font_size"].as_f64();
    let longest = label.lines().map(str::len).max().unwrap_or(1).max(1);
    let (native_width, native_height) = if rotation % 180 == 0 {
        (area.width, area.height)
    } else {
        (area.height, area.width)
    };
    let mut size = fixed.unwrap_or_else(|| {
        (native_height / label.lines().count().max(1) as f64 * 0.45)
            .min(native_width / longest as f64 * 1.6)
            .clamp(6.0, 36.0)
    });
    let rgb = if kind == "text" {
        (0.0, 0.0, 0.0)
    } else {
        (r, g, b)
    };
    loop {
        let lines = textbox.wrap(label, area.width, size);
        if textbox.fits(lines.len(), size, area.height) {
            let align = style["alignment"].as_str();
            overlay.textbox_fonts = true;
            let font = text_font(overlay, textbox.name);
            let (r, g, b) = rgb;
            for (index, (line, justify)) in lines.iter().enumerate() {
                if line.is_empty() {
                    continue;
                }
                let slack = area.width - textbox.measure(line, size);
                let x = area.left
                    + match align {
                        Some("center") => slack / 2.0,
                        Some("right") => slack,
                        _ => 0.0,
                    };
                let y = textbox.baseline(area.bottom + area.height, size, index);
                let spaces = line.bytes().filter(|&b| b == b' ').count();
                let spacing = if align == Some("justify") && *justify && spaces > 0 {
                    slack / spaces as f64
                } else {
                    0.0
                };
                let encoded = hex_pdf(line.as_bytes());
                writeln!(overlay.commands, "{r:.5} {g:.5} {b:.5} rg BT /@F{font} {size:.5} Tf {spacing:.5} Tw 1 0 0 1 {x:.5} {y:.5} Tm <{encoded}> Tj ET").unwrap();
            }
            return Ok(());
        }
        if fixed.is_some() {
            return Err("Added text does not fit the selected size and box; enlarge it or reduce the font size");
        }
        size -= 1.0;
        if size < 6.0 {
            return Err(match kind {
                "check" => "Check mark does not fit its selected area",
                "cross" => "Cross mark does not fit its selected area",
                "date" => "Date mark does not fit its selected area",
                _ => "Text mark does not fit its selected area",
            });
        }
    }
}

fn compress(data: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut writer = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    writer.write_all(data).map_err(|_| OUTPUT_ERROR)?;
    writer.finish().map_err(|_| OUTPUT_ERROR)
}

pub(super) fn image_stream(
    width: u32,
    height: u32,
    color_space: &str,
    data: &[u8],
) -> Result<Object, &'static str> {
    let name = |name: &str| Object::Name(name.into());
    let fields = [
        ("Type", name("XObject")),
        ("Subtype", name("Image")),
        ("ColorSpace", name(color_space)),
        ("Filter", name("FlateDecode")),
        ("BitsPerComponent", Object::Integer(8)),
        ("Width", Object::Integer(width.into())),
        ("Height", Object::Integer(height.into())),
    ];
    Ok(Object::Stream {
        dict: fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
        data: compress(data)?.into(),
    })
}

fn append_image(
    overlay: &mut assembly::Overlay,
    bytes: &[u8],
    area: BoxOnPage,
) -> Result<(), &'static str> {
    let format = supported_image_format(bytes)?;
    let decoded = decode_image(bytes, format)?;
    let (resource, dimensions) = image_resource(bytes, format, decoded)?;
    paint_image(overlay, resource, dimensions, area, true);
    Ok(())
}

fn image_resource(
    bytes: &[u8],
    format: ImageFormat,
    decoded: DynamicImage,
) -> Result<(assembly::OverlayImage, [u32; 2]), &'static str> {
    if format == ImageFormat::Jpeg {
        drop(decoded);
        super::image_import::jpeg_overlay(bytes)
    } else {
        super::image_import::png_overlay(bytes, decoded)
    }
}

fn append_raster(
    overlay: &mut assembly::Overlay,
    image: image::RgbaImage,
    area: BoxOnPage,
    fit: bool,
) -> Result<(), &'static str> {
    let (width, height) = image.dimensions();
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    let mut alpha = Vec::with_capacity(width as usize * height as usize);
    for pixel in image.pixels() {
        rgb.extend_from_slice(&pixel.0[..3]);
        alpha.push(pixel.0[3]);
    }
    let object = image_stream(width, height, "DeviceRGB", &rgb)?;
    let transparency = if alpha.iter().any(|&sample| sample != 255) {
        Some(image_stream(width, height, "DeviceGray", &alpha)?)
    } else {
        None
    };
    let resource = assembly::OverlayImage {
        samples: object,
        mask: transparency,
        profile: None,
    };
    paint_image(overlay, resource, [width, height], area, fit);
    Ok(())
}

fn paint_image(
    overlay: &mut assembly::Overlay,
    resource: assembly::OverlayImage,
    [width, height]: [u32; 2],
    area: BoxOnPage,
    fit: bool,
) {
    let (w, h) = if fit {
        let (width, height) = (f64::from(width), f64::from(height));
        let scale = (area.width / width).min(area.height / height);
        (width * scale, height * scale)
    } else {
        (area.width, area.height)
    };
    let x = area.left + (area.width - w) / 2.0;
    let y = area.bottom + (area.height - h) / 2.0;
    let index = overlay.images.len();
    overlay.images.push(resource);
    writeln!(
        overlay.commands,
        "q {w:.5} 0 0 {h:.5} {x:.5} {y:.5} cm /@I{index} Do Q"
    )
    .unwrap();
}

fn draw(
    overlay: &mut assembly::Overlay,
    item: &Value,
    area: BoxOnPage,
    asset: Option<&[u8]>,
    frame: assembly::DisplayFrame,
    pdf: &PdfDocument,
    page: usize,
) -> Result<(), &'static str> {
    let kind = item["kind"].as_str().ok_or("Unsupported PDF mark kind")?;
    if kind == "image" {
        return append_image(
            overlay,
            asset.ok_or("Image overlay asset missing")?,
            area.transformed(frame.shape_matrix()),
        );
    }
    let (r, g, b) = color(item);
    let (matrix, stroke) = if matches!(kind, "check" | "cross" | "date" | "text") {
        (frame.text_matrix(true), 1.0)
    } else {
        // The drawing matrix scales the path on an unrotated UserUnit page.
        // The requested stroke width is written in native units.
        let scale = if frame.geometry.rotation == 0 {
            frame.unit
        } else {
            1.0
        };
        (frame.shape_matrix(), area.stroke() * scale)
    };
    let [a, b_axis, c, d, e, f] = matrix;
    writeln!(overlay.commands, "q {a:.9} {b_axis:.9} {c:.9} {d:.9} {e:.7} {f:.7} cm\n{r:.5} {g:.5} {b:.5} RG {stroke:.5} w").unwrap();
    let (x, y, w, h) = (area.left, area.bottom, area.width, area.height);
    match kind {
        "box" => writeln!(overlay.commands, "{x:.5} {y:.5} {w:.5} {h:.5} re S").unwrap(),
        "ellipse" => {
            let (rx, ry) = (w / 2.0, h / 2.0);
            let (cx, cy) = (x + rx, y + ry);
            let k = 0.552_284_749_831;
            let (east, west, north, south) = (x + w, x, y + h, y);
            let (upper_control, lower_control) = (cy + k * ry, cy - k * ry);
            let (right_control, left_control) = (cx + k * rx, cx - k * rx);
            writeln!(overlay.commands,
                "{east:.5} {cy:.5} m {east:.5} {upper_control:.5} {right_control:.5} {north:.5} {cx:.5} {north:.5} c
{left_control:.5} {north:.5} {west:.5} {upper_control:.5} {west:.5} {cy:.5} c
{west:.5} {lower_control:.5} {left_control:.5} {south:.5} {cx:.5} {south:.5} c
{right_control:.5} {south:.5} {east:.5} {lower_control:.5} {east:.5} {cy:.5} c S").unwrap();
        }
        "line" | "arrow" => {
            let tip = (x + w, y + h);
            path(&mut overlay.commands, [(x, y), tip]);
            if kind == "arrow" {
                let length = (w.min(h) * 0.35).clamp(5.0, 16.0);
                let angle = h.atan2(w) + std::f64::consts::PI;
                for offset in [-0.55, 0.55] {
                    let phi = angle + offset;
                    path(
                        &mut overlay.commands,
                        [
                            tip,
                            (tip.0 + length * phi.cos(), tip.1 + length * phi.sin()),
                        ],
                    );
                }
            }
        }
        "draw" => {
            let points = annotations::points(item["body"].as_str().unwrap_or_default())
                .ok_or("Invalid draw path")?;
            for pair in points.windows(2) {
                path(
                    &mut overlay.commands,
                    pair.iter().map(|[x, y]| area.point(*x, *y)),
                );
            }
        }
        "check" | "cross" | "date" | "text" => {
            let textbox = Textbox::new(pdf, page, font(&item["text_style"]));
            text_mark(
                overlay,
                item,
                area,
                kind,
                (r, g, b),
                textbox,
                frame.geometry.rotation,
            )?;
        }
        _ => return Err("Unsupported PDF mark kind"),
    }
    overlay.commands.push_str("Q\n");
    Ok(())
}

pub(super) fn flatten_with_assets(
    bytes: Vec<u8>,
    marks: &[Value],
    mut assets: impl FnMut(&str) -> Result<Option<Vec<u8>>, &'static str>,
) -> Result<Vec<u8>, &'static str> {
    if marks.is_empty() {
        return Ok(bytes);
    }
    let pdf = PdfDocument::from_bytes(bytes.clone()).map_err(|_| OUTPUT_ERROR)?;
    let mut overlays = HashMap::<usize, assembly::Overlay>::new();
    let mut redactions = Vec::new();
    // The saved image layer sits below editable drawing and text on each page.
    // Destructive redaction runs after both visible layers.
    let (images, editable): (Vec<_>, Vec<_>) =
        marks.iter().partition(|mark| mark["kind"] == "image");
    for item in images.into_iter().chain(editable) {
        let page = item["page_index"].as_u64().ok_or(OUTPUT_ERROR)? as usize;
        let frame = assembly::DisplayFrame::new(&pdf, page)?;
        let (width, height) = frame.size;
        let area = BoxOnPage::relative(item, width, height)?;
        if item["kind"] == "redaction" {
            let (width, height) = frame.visible.display_size();
            let region = BoxOnPage::relative(item, width, height)?
                .transformed(frame.visible.display_matrix());
            redactions.push((page, region.bounds().map(|value| value as f32)));
            continue;
        }
        let asset = if item["kind"] == "image" {
            assets(item["id"].as_str().ok_or(OUTPUT_ERROR)?)?
        } else {
            None
        };
        draw(
            overlays.entry(page).or_insert_with(|| assembly::Overlay {
                user_space_text: true,
                ..Default::default()
            }),
            item,
            area,
            asset.as_deref(),
            frame,
            &pdf,
            page,
        )?;
    }
    drop(pdf);
    let bytes = if overlays.is_empty() {
        bytes
    } else {
        assembly::append_document_overlays(bytes, &overlays, None)?
    };
    if redactions.is_empty() {
        return Ok(bytes);
    }
    destructive_redaction(bytes, &redactions)
}

pub(super) fn destructive_redaction(
    bytes: Vec<u8>,
    regions: &[(usize, [f32; 4])],
) -> Result<Vec<u8>, &'static str> {
    destructive_redaction_inner(bytes, regions, 0, true)
}

fn destructive_redaction_inner(
    mut bytes: Vec<u8>,
    regions: &[(usize, [f32; 4])],
    depth: usize,
    paint_fill: bool,
) -> Result<Vec<u8>, &'static str> {
    if depth > 12 {
        return Err("The selected PDF forms are nested too deeply to redact safely");
    }
    let source = PdfDocument::from_bytes(bytes.clone()).map_err(|_| OUTPUT_ERROR)?;
    let count = source.page_count().map_err(|_| OUTPUT_ERROR)?;
    let mut targets = HashMap::<usize, Vec<[f32; 4]>>::new();
    for &(page, region) in regions {
        targets.entry(page).or_default().push(region);
    }
    let mut marked_content = HashMap::new();
    for &page in targets.keys() {
        if let Some(evidence) = super::redaction_metadata::capture(&source, page)? {
            marked_content.insert(page, evidence);
        }
    }
    let mut promoted = HashMap::new();
    for page in 0..count {
        super::text_edit::annotation_boxes(&source, page)
            .map_err(|_| "Resolve pending PDF redaction marks before applying a new redaction")?;
        if targets.contains_key(&page) {
            if let Some(overlay) = super::redaction_inline::promote(&source, page)? {
                promoted.insert(page, overlay);
            }
        }
    }
    let source = if promoted.is_empty() {
        source
    } else {
        drop(source);
        bytes = assembly::overlay_document(bytes, &promoted)?;
        PdfDocument::from_bytes(bytes.clone()).map_err(|_| OUTPUT_ERROR)?
    };
    let mut overlays = HashMap::new();
    for (&page, regions) in &targets {
        overlays.insert(
            page,
            redacted_xobjects(&source, &bytes, page, regions, depth)?,
        );
    }
    if overlays
        .values()
        .any(|overlay| !overlay.xobject_edits.is_empty())
    {
        for page in 0..count {
            if overlays.contains_key(&page) {
                continue;
            }
            let content = source
                .get_page_content_data(page)
                .map_err(|_| OUTPUT_ERROR)?;
            let operators =
                pdf_oxide::content::parse_content_stream(&content).map_err(|_| OUTPUT_ERROR)?;
            if operators
                .iter()
                .any(|op| matches!(op, pdf_oxide::content::Operator::InlineImage { .. }))
            {
                continue;
            }
            let used = super::redaction_vectors::retained_xobjects(&source, page, &content)?;
            overlays.insert(
                page,
                assembly::Overlay {
                    retain_xobjects: Some(used),
                    ..Default::default()
                },
            );
        }
    }
    drop(source);
    let mut bytes = assembly::overlay_document(bytes, &overlays)?;
    let source = PdfDocument::from_bytes(bytes.clone()).map_err(|_| OUTPUT_ERROR)?;
    let mut vectors = HashMap::new();
    for (&page, regions) in &targets {
        if let Some(overlay) =
            super::redaction_vectors::remove_covered(&source, &bytes, page, regions)?
        {
            vectors.insert(page, overlay);
        }
    }
    if vectors
        .values()
        .any(|overlay| overlay.retain_patterns.is_some())
    {
        for page in 0..count {
            if vectors
                .get(&page)
                .is_some_and(|overlay| overlay.retain_patterns.is_some())
            {
                continue;
            }
            let content = source
                .get_page_content_data(page)
                .map_err(|_| OUTPUT_ERROR)?;
            super::redaction_vectors::retain_paint_resources(
                &source,
                page,
                &content,
                vectors.entry(page).or_default(),
            )?;
        }
    }
    drop(source);
    if !vectors.is_empty() {
        bytes = assembly::overlay_document(bytes, &vectors)?;
    }
    let mut options = pdf_oxide::redaction::RedactionOptions::default();
    options.draw_overlay_when_no_ic = paint_fill;
    let fill = paint_fill.then_some([0.0; 3]);
    let source = PdfDocument::from_bytes(bytes.clone()).map_err(|_| OUTPUT_ERROR)?;
    let mut composite = HashMap::new();
    for (page, regions) in targets {
        if let Some((overlay, _)) =
            super::redaction_text::rewrite_composite(&source, page, &regions, fill, &options)?
        {
            composite.insert(page, overlay);
        }
    }
    drop(source);
    if !composite.is_empty() {
        bytes = assembly::overlay_document(bytes, &composite)?;
    }
    let mut editor =
        DocumentEditor::from_bytes(bytes).map_err(|_| "Unable to prepare permanent redaction")?;
    for &(page, coordinates) in regions {
        if !composite.contains_key(&page) {
            editor
                .add_redaction(page, coordinates, fill)
                .map_err(|_| OUTPUT_ERROR)?;
        }
    }
    editor
        .apply_redactions_destructive(options)
        .map_err(|_| "This PDF cannot be safely redacted")?;
    editor
        .sanitize_document(options)
        .map_err(|_| "Unable to remove PDF metadata during redaction")?;
    let mut bytes = editor.save_to_bytes().map_err(|_| OUTPUT_ERROR)?;
    let source = PdfDocument::from_bytes(bytes.clone()).map_err(|_| OUTPUT_ERROR)?;
    let mut unused_resources = HashMap::new();
    for page in 0..count {
        let mut overlay = super::redaction_vectors::prune_unpainted_patterns(&source, page)?;
        let mask_keys = super::redaction_metadata::inactive_soft_masks(&source, page);
        if !mask_keys.is_empty() {
            overlay
                .get_or_insert_with(Default::default)
                .clear_soft_masks = mask_keys;
        }
        if let Some(overlay) = overlay {
            unused_resources.insert(page, overlay);
        }
    }
    drop(source);
    if !unused_resources.is_empty() {
        bytes = assembly::overlay_document(bytes, &unused_resources)?;
    }
    if !marked_content.is_empty() {
        let source = PdfDocument::from_bytes(bytes.clone()).map_err(|_| OUTPUT_ERROR)?;
        let mut sanitized = HashMap::new();
        for page in 0..count {
            if let Some(overlay) =
                super::redaction_metadata::scrub(&source, page, marked_content.get(&page))?
            {
                sanitized.insert(page, overlay);
            }
        }
        drop(source);
        if !sanitized.is_empty() {
            bytes = assembly::overlay_document(bytes, &sanitized)?;
        }
    }
    let cleaned = assembly::clean_redacted_document(bytes)?;
    let check = PdfDocument::from_bytes(cleaned.clone()).map_err(|_| OUTPUT_ERROR)?;
    for &(page, [left, bottom, right, top]) in regions {
        let glyphs = check
            .extract_chars(page)
            .map_err(|_| "Cannot inspect redacted text")?;
        if glyphs.iter().any(|glyph| {
            let b = glyph.bbox;
            !glyph.char.is_whitespace()
                && b.width > 0.0
                && b.height > 0.0
                && b.x >= left
                && b.y >= bottom
                && b.x + b.width <= right
                && b.y + b.height <= top
        }) {
            return Err("Text remains inside the selected redaction region");
        }
    }
    Ok(cleaned)
}

pub(super) fn overlay_working_text(
    bytes: Vec<u8>,
    page: usize,
    x: f64,
    y: f64,
    text: &str,
    size: f64,
    reserved: &[Value],
) -> Result<Vec<u8>, &'static str> {
    let pdf = PdfDocument::from_bytes(bytes.clone()).map_err(|_| OUTPUT_ERROR)?;
    let frame = assembly::DisplayFrame::new(&pdf, page)?;
    let (width, height) = frame.size;
    let textbox = Textbox::new(&pdf, page, "Helvetica");
    let left = width * x;
    let top = height * (1.0 - y);
    let lines = text.split('\n').collect::<Vec<_>>();
    let line_width = lines
        .iter()
        .map(|line| textbox.measure(line, size))
        .fold(0.0, f64::max);
    let bottom = top - size * (2.8 + 1.2 * lines.len().saturating_sub(1) as f64) - 2.0;
    let right = left + line_width + 3.0;
    if right > width || bottom < 0.0 {
        return Err("Text does not fit on the page; move it or reduce the font size");
    }
    let lines = textbox.wrap(text, line_width + 3.0, size);
    if !textbox.fits(lines.len(), size, top - bottom) {
        return Err("Text does not fit from this point; move it or reduce the font size");
    }
    let text_matrix = frame.text_matrix(true);
    let [a, b, c, d, e, f] = text_matrix;
    let reserved_matrix = frame.paint_matrix();
    let rect = |rect: pdf_oxide::geometry::Rect| {
        [
            f64::from(rect.x),
            f64::from(rect.y),
            f64::from(rect.x + rect.width),
            f64::from(rect.y + rect.height),
        ]
    };
    let visible_text::PagePaints {
        text: mut obstacles,
        glyph_images,
        appearances,
        ..
    } = visible_text::page_paints(&pdf, page, frame.geometry)?;
    let (visible_width, visible_height) = frame.visible.display_size();
    let backdrop_area = 0.8 * visible_width * visible_height;
    obstacles.extend(
        glyph_images
            .into_iter()
            .chain(
                appearances
                    .iter()
                    .filter_map(|appearance| match appearance {
                        visible_text::AppearancePaint::Image(bounds) => Some(*bounds),
                        _ => None,
                    }),
            )
            .filter(|bounds| (bounds[2] - bounds[0]) * (bounds[3] - bounds[1]) <= backdrop_area),
    );
    obstacles.extend(
        pdf.page_image_handles(page)
            .map_err(|_| "Cannot inspect existing page images")?
            .iter()
            // A page-sized scan is the background of the working page.
            .filter(|image| {
                f64::from(image.bbox.width) * f64::from(image.bbox.height) <= backdrop_area
            })
            .map(|image| rect(image.bbox)),
    );
    let reserved = reserved
        .iter()
        .map(|mark| {
            BoxOnPage::relative(mark, width, height)
                .map(|area| assembly::transform_box(reserved_matrix, area.bounds()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut overlay = assembly::Overlay {
        textbox_fonts: true,
        user_space_text: true,
        ..Default::default()
    };
    let font = text_font(&mut overlay, "Helvetica");
    writeln!(
        overlay.commands,
        "q {a} {b} {c} {d} {e:.7} {f:.7} cm\nBT /@F{font} {size:.5} Tf"
    )
    .unwrap();
    let mut ink = Vec::new();
    for (index, (line, _)) in lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let baseline = textbox.baseline(top, size, index);
        let bounds = textbox.helvetica_ink(line, size, left, baseline);
        ink.push(assembly::transform_box(text_matrix, bounds));
        let encoded = hex_pdf(line.as_bytes());
        writeln!(
            overlay.commands,
            "1 0 0 1 {left:.5} {baseline:.5} Tm 0 0 0 rg <{encoded}> Tj"
        )
        .unwrap();
    }
    overlay.commands.push_str("ET\nQ\n");
    // Python takes the newly added suffix of a trace that records page
    // content first and annotation/widget appearances last.
    let mut remaining = ink.len();
    let mut first = appearances.len();
    for paint in appearances.iter().rev() {
        if remaining == 0 {
            break;
        }
        remaining = remaining.saturating_sub(paint.count());
        first -= 1;
    }
    let new_appearances = &appearances[first..];
    let added_lines = &ink[ink.len() - remaining..];
    if added_lines.is_empty()
        && !new_appearances.iter().any(|paint| {
            matches!(
                paint,
                visible_text::AppearancePaint::Text(_)
                    | visible_text::AppearancePaint::GeneratedText(_)
            )
        })
    {
        return Err("Text could not be placed on this page");
    }
    let added_ink = added_lines
        .iter()
        .copied()
        .chain(new_appearances.iter().filter_map(|paint| match paint {
            visible_text::AppearancePaint::Text(Some(bounds))
                if bounds[0] < bounds[2] && bounds[1] < bounds[3] =>
            {
                Some(*bounds)
            }
            _ => None,
        }))
        .collect::<Vec<_>>();
    let overlaps = |rect: &[f64; 4]| {
        added_ink
            .iter()
            .any(|&line| super::text_edit::intersects(line, *rect))
    };
    if new_appearances
        .iter()
        .any(|paint| matches!(paint, visible_text::AppearancePaint::GeneratedText(true)))
        || obstacles.iter().any(overlaps)
    {
        return Err("Text overlaps existing page content. Choose a clear area.");
    }
    if reserved.iter().any(overlaps) {
        return Err("Text overlaps an existing page component. Choose a clear area.");
    }
    assembly::append_document_overlays(bytes, &HashMap::from([(page, overlay)]), None)
}

pub(super) fn overlay_working_image(
    bytes: Vec<u8>,
    page: i64,
    count: usize,
    body: &Value,
    image: annotations::ImageUpload,
) -> Result<Vec<u8>, Reply> {
    let invalid = |detail| refusal(422, detail);
    BoxOnPage::relative(body, 1.0, 1.0)
        .map_err(|_| invalid("Image placement must be inside the page"))?;
    let page = usize::try_from(page)
        .ok()
        .filter(|&page| page < count)
        .ok_or_else(|| invalid("Invalid page number"))?;
    let pdf = PdfDocument::from_bytes(bytes.clone()).map_err(|_| invalid(OUTPUT_ERROR))?;
    let frame = assembly::DisplayFrame::new(&pdf, page).map_err(invalid)?;
    let area = BoxOnPage::relative(body, frame.size.0, frame.size.1)
        .map_err(invalid)?
        .transformed(frame.shape_matrix());
    let (resource, dimensions) =
        image_resource(&image.bytes, image.format, image.decoded).map_err(invalid)?;
    let mut overlay = assembly::Overlay {
        user_space_text: true,
        ..Default::default()
    };
    paint_image(&mut overlay, resource, dimensions, area, true);
    assembly::append_document_overlays(bytes, &HashMap::from([(page, overlay)]), None)
        .map_err(invalid)
}

fn print_whitespace(character: char) -> bool {
    character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
}

fn decimal(character: char) -> Option<u8> {
    // Decimal zero code points in Unicode 15.0, the Python 3.12 reference's
    // Unicode database. Each zero begins ten consecutive decimal digits.
    const ZEROES: [u32; 68] = [
        0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66,
        0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90,
        0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0,
        0xff10, 0x104a0, 0x10d30, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450, 0x114d0,
        0x11650, 0x116c0, 0x11730, 0x118e0, 0x11950, 0x11c50, 0x11d50, 0x11da0, 0x11f50, 0x16a60,
        0x16ac0, 0x16b50, 0x1d7ce, 0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0,
        0x1e950, 0x1fbf0,
    ];
    let code = u32::from(character);
    let zero = ZEROES
        .partition_point(|zero| *zero <= code)
        .checked_sub(1)?;
    let value = code - ZEROES[zero];
    (value < 10).then_some(value as u8)
}

fn parse_pages(spec: &Value, count: usize) -> Result<Vec<usize>, String> {
    let invalid = || "Enter all or page numbers such as 1,3-5".to_owned();
    let spec = spec.as_str().ok_or_else(invalid)?;
    if spec
        .trim_matches(print_whitespace)
        .eq_ignore_ascii_case("all")
    {
        return Ok((0..count).collect());
    }
    if spec.chars().take(2049).count() > 2048 {
        return Err(invalid());
    }
    let mut normalized = String::new();
    for character in spec.chars() {
        normalized.push(match character {
            ',' | '-' => character,
            character if print_whitespace(character) => ' ',
            character => char::from(b'0' + decimal(character).ok_or_else(invalid)?),
        });
    }
    let mut pages = Vec::new();
    let mut exceeds_limit = false;
    for segment in normalized.split(',') {
        let (start, end) = segment.split_once('-').unwrap_or((segment, segment));
        let number = |part: &str| {
            let part = part.trim();
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid());
            }
            Ok(part.parse::<usize>().unwrap_or(usize::MAX))
        };
        let (start, end) = (number(start)?, number(end)?);
        if start == 0 || end < start || end > count {
            return Err(format!("Page range must stay between 1 and {count}"));
        }
        exceeds_limit |= end - start + 1 > 500 - pages.len();
        if !exceeds_limit {
            pages.extend(start - 1..end);
        }
    }
    if exceeds_limit || pages.iter().collect::<std::collections::HashSet<_>>().len() != pages.len()
    {
        return Err("Select 1–500 distinct pages for one print job".into());
    }
    Ok(pages)
}

pub(super) fn print_pdf(bytes: Vec<u8>, body: &Value) -> Result<Vec<u8>, Reply> {
    let option = |key, default| match body.get(key) {
        None => Ok(default),
        Some(Value::String(value)) => Ok(value.as_str()),
        Some(Value::Array(_)) => Err(refusal(422, "unhashable type: 'list'")),
        Some(Value::Object(_)) => Err(refusal(422, "unhashable type: 'dict'")),
        Some(_) => Ok(""),
    };
    let dimensions = match option("orientation", "source")? {
        "source" => None,
        "portrait" => Some((595.0, 842.0)),
        "landscape" => Some((842.0, 595.0)),
        _ => return Err(refusal(422, "Choose source, portrait or landscape paper")),
    };
    let fit = match option("scale", "fit")? {
        "fit" => true,
        "actual" => false,
        _ => return Err(refusal(422, "Choose fit or actual size")),
    };
    let paper = dimensions.map(|(width, height)| assembly::Paper { width, height, fit });
    let failure = || refusal(422, OUTPUT_ERROR);
    let mut editor = DocumentEditor::from_bytes(bytes).map_err(|_| failure())?;
    if editor.source().is_encrypted() && !editor.source().authenticate(b"").unwrap_or(false) {
        return Err(refusal(422, "PDF requires a password"));
    }
    let selection = parse_pages(
        body.get("pages").unwrap_or(&json!("all")),
        editor.current_page_count(),
    )
    .map_err(|error| refusal(422, error))?;
    for &page in &selection {
        editor
            .flatten_page_annotations(page)
            .map_err(|_| failure())?;
        editor.flatten_forms_on_page(page).map_err(|_| failure())?;
    }
    let bytes = editor.save_to_bytes().map_err(|_| failure())?;
    assembly::print_document(bytes, &selection, paper).map_err(|err| refusal(422, err))
}

struct PrintableMarks {
    marks: Vec<Value>,
    assets: HashMap<String, Result<Option<Vec<u8>>, &'static str>>,
}

pub struct PdfSnapshotWork {
    source: PdfSource,
    marks: Result<Option<PrintableMarks>, Reply>,
}

impl PdfSnapshotWork {
    fn raw(source: PdfSource) -> Self {
        Self {
            source,
            marks: Ok(None),
        }
    }

    pub fn compute(self) -> Result<PdfSnapshot, Reply> {
        let snapshot = self.source.open()?;
        let Some(PrintableMarks { marks, mut assets }) = self.marks? else {
            return Ok(snapshot);
        };
        let bytes = snapshot
            .into_bytes()
            .map_err(|_| refusal(409, OUTPUT_ERROR))?;
        flatten_with_assets(bytes, &marks, |id| assets.remove(id).unwrap_or(Ok(None)))
            .map(PdfSnapshot::from_bytes)
            .map_err(|error| refusal(422, error))
    }
}

enum DocumentOutputAction {
    Original,
    Working {
        revision: Option<String>,
        editable: bool,
        acknowledged: bool,
    },
    Retained {
        revision: String,
        sha256: Option<String>,
    },
    PrintWorking,
    PrintOriginal,
}

pub struct DocumentOutputTarget {
    document: String,
    action: DocumentOutputAction,
    head: bool,
}

impl DocumentOutputTarget {
    pub fn from_path(method: &str, path: &str, query: &str) -> Option<Self> {
        let route = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
        Self::select(method, &route, query)
    }

    fn select(method: &str, route: &[&str], query: &str) -> Option<Self> {
        let lookup = |key| reading::lookup(query, key);
        let (document, action) = match (method, route) {
            ("GET" | "HEAD", ["documents", id, "original"]) => (id, DocumentOutputAction::Original),
            ("GET" | "HEAD", ["documents", id, "pdf"]) => (
                id,
                DocumentOutputAction::Working {
                    revision: lookup("revision").map(|value| value.into_owned()),
                    editable: lookup("editable").as_deref() == Some("true"),
                    acknowledged: lookup("acknowledge_signature_risk").as_deref() == Some("true"),
                },
            ),
            ("GET" | "HEAD", ["documents", id, "revisions", revision, "pdf"]) => (
                id,
                DocumentOutputAction::Retained {
                    revision: (*revision).to_owned(),
                    sha256: lookup("sha256").map(|value| value.into_owned()),
                },
            ),
            ("POST", ["documents", id, "print"]) => (id, DocumentOutputAction::PrintWorking),
            ("POST", ["documents", id, "original", "print"]) => {
                (id, DocumentOutputAction::PrintOriginal)
            }
            _ => return None,
        };
        Some(Self {
            document: (*document).to_owned(),
            action,
            head: method == "HEAD",
        })
    }
}

enum DocumentOutput {
    File(PdfSnapshotWork),
    PrintWorking(PdfSnapshotWork, Value),
    PrintOriginal {
        source: PdfSource,
        convert_image: bool,
        options: Value,
    },
}

pub struct DocumentOutputWork {
    output: DocumentOutput,
    head: bool,
    media: &'static str,
}

impl DocumentOutputWork {
    pub fn compute(self) -> Result<Reply, Reply> {
        let head = self.head;
        let media = self.media;
        let snapshot = self.snapshot()?;
        if head {
            return Ok(reply(
                200,
                json!({"length":snapshot.bytes,"media_type":media}),
            ));
        }
        let bytes = snapshot
            .into_bytes()
            .map_err(|_| refusal(413, "Use the native file reader for this artifact"))?;
        Ok(binary(bytes, media))
    }

    pub fn snapshot(self) -> Result<PdfSnapshot, Reply> {
        let unreadable = || refusal(409, OUTPUT_ERROR);
        let bytes = match self.output {
            DocumentOutput::File(work) => return work.compute(),
            DocumentOutput::PrintWorking(work, options) => {
                let bytes = work.compute()?.into_bytes().map_err(|_| unreadable())?;
                print_pdf(bytes, &options)?
            }
            DocumentOutput::PrintOriginal {
                source,
                convert_image,
                options,
            } => {
                let bytes = source
                    .open()
                    .map_err(|_| original_print_integrity_error())?
                    .into_bytes()
                    .map_err(|_| unreadable())?;
                let bytes = if convert_image {
                    normalize_image(&bytes).map_err(|error| refusal(422, error))?
                } else {
                    bytes
                };
                print_pdf(bytes, &options)?
            }
        };
        Ok(PdfSnapshot::from_bytes(bytes))
    }
}

fn original_print_integrity_error() -> Reply {
    refusal(409, "Imported original is unavailable or its hash changed")
}

impl Store {
    pub fn prepare_pdf_snapshot(
        &self,
        id: &str,
        artifact: &Artifact,
    ) -> Result<PdfSnapshotWork, Reply> {
        self.pdf_source(id, artifact).map(PdfSnapshotWork::raw)
    }

    pub fn editable_snapshot(
        &self,
        id: &str,
        revision: Option<u64>,
        acknowledged: bool,
    ) -> Result<PdfSnapshot, Reply> {
        self.prepare_editable_snapshot(id, revision, acknowledged)?
            .compute()
    }

    pub fn prepare_editable_snapshot(
        &self,
        id: &str,
        revision: Option<u64>,
        acknowledged: bool,
    ) -> Result<PdfSnapshotWork, Reply> {
        let doc = self.artifact_document(id)?;
        let source = self.document_pdf_source(id, &doc, &Artifact::Working { revision })?;
        // Read and authenticate before returning a marks refusal, as with an ordinary snapshot.
        let marks = self.printable_marks(id, &doc.body, acknowledged);
        Ok(PdfSnapshotWork { source, marks })
    }

    fn printable_marks(
        &self,
        id: &str,
        doc: &Value,
        acknowledged: bool,
    ) -> Result<Option<PrintableMarks>, Reply> {
        let matter = doc["matter_id"]
            .as_str()
            .ok_or_else(|| refusal(409, "Invalid document matter"))?;
        let marks = annotations::list(&self.con, matter)
            .map_err(|_| refusal(500, "Unable to load review marks"))?
            .into_iter()
            .filter(|mark| mark["document_id"] == id && mark["output_scope"] == "pdf")
            .collect::<Vec<_>>();
        if marks
            .iter()
            .any(|mark| mark["resolved"] != true && mark["kind"] != "image")
        {
            return Err(refusal(409, "Editable PDF marks need review after the working PDF changed; resolve or remove them before saving"));
        }
        let marks = marks
            .into_iter()
            .filter(|mark| mark["resolved"] == true)
            .collect::<Vec<_>>();
        if marks.is_empty() {
            return Ok(None);
        }
        signature_guard([doc], acknowledged, "saving or printing an edited PDF copy")?;
        let assets = marks
            .iter()
            .filter(|mark| mark["kind"] == "image")
            .filter_map(|mark| mark["id"].as_str())
            .map(|id| {
                (
                    id.to_owned(),
                    annotations::asset(&self.con, id).map_err(|_| OUTPUT_ERROR),
                )
            })
            .collect();
        Ok(Some(PrintableMarks { marks, assets }))
    }

    pub fn prepare_document_output(
        &self,
        target: DocumentOutputTarget,
        body: Value,
    ) -> Result<DocumentOutputWork, Reply> {
        let DocumentOutputTarget {
            document: id,
            action,
            head,
        } = target;
        let mut media = "application/pdf";
        let output = match action {
            DocumentOutputAction::Original => {
                let doc = self.artifact_document(&id)?;
                media = artifacts::original_media(&doc.body["original_name"]);
                let source = self.document_pdf_source(&id, &doc, &Artifact::Original)?;
                DocumentOutput::File(PdfSnapshotWork::raw(source))
            }
            DocumentOutputAction::Working {
                revision,
                editable,
                acknowledged,
            } => {
                let revision = revision
                    .as_deref()
                    .map(str::parse::<u64>)
                    .transpose()
                    .map_err(|_| refusal(422, "Invalid working revision"))?;
                let work = if editable {
                    self.prepare_editable_snapshot(&id, revision, acknowledged)?
                } else {
                    self.prepare_pdf_snapshot(&id, &Artifact::Working { revision })?
                };
                DocumentOutput::File(work)
            }
            DocumentOutputAction::Retained { revision, sha256 } => {
                let revision = revision
                    .parse()
                    .map_err(|_| refusal(422, "Invalid retained revision"))?;
                let sha256 = sha256
                    .filter(|value| artifacts::valid_hash(value))
                    .ok_or_else(|| refusal(422, "A lowercase SHA-256 is required"))?;
                DocumentOutput::File(
                    self.prepare_pdf_snapshot(&id, &Artifact::Retained { revision, sha256 })?,
                )
            }
            DocumentOutputAction::PrintWorking => {
                let doc = self.artifact_document(&id)?;
                let stale = || refusal(409, "Working PDF revision changed; reload the document");
                let revision = doc.body["revision"].as_u64().ok_or_else(stale)?;
                let expected = &body["expected_revision"];
                let matches = match expected.as_u64() {
                    Some(number) => number == revision,
                    None => {
                        expected
                            .as_f64()
                            .is_some_and(|value| value == revision as f64)
                            || (expected == true && revision == 1)
                    }
                };
                if !matches {
                    return Err(stale());
                }
                let work = self.prepare_editable_snapshot(
                    &id,
                    Some(revision),
                    body["acknowledge_signature_risk"] == true,
                )?;
                DocumentOutput::PrintWorking(work, body)
            }
            DocumentOutputAction::PrintOriginal => {
                let doc = self.artifact_document(&id)?;
                let source = self
                    .document_pdf_source(&id, &doc, &Artifact::Original)
                    .map_err(|_| original_print_integrity_error())?;
                let convert_image = !doc.body["original_name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .ends_with(".pdf");
                DocumentOutput::PrintOriginal {
                    source,
                    convert_image,
                    options: body,
                }
            }
        };
        Ok(DocumentOutputWork {
            output,
            head,
            media,
        })
    }

    pub(super) fn document_output_request(
        &self,
        method: &str,
        route: &[&str],
        query: &str,
        body: &Value,
    ) -> Option<Reply> {
        let target = DocumentOutputTarget::select(method, route, query)?;
        Some(
            self.prepare_document_output(target, body.clone())
                .and_then(DocumentOutputWork::compute)
                .unwrap_or_else(|error| error),
        )
    }
}

/// Restore only the reviewed region. Retained source page objects and resources
/// are never imported into the current PDF.
pub(super) fn restore_redacted_area(
    current: Vec<u8>,
    source: Vec<u8>,
    index: usize,
    footprint: &Value,
    later: &[Value],
) -> Result<Vec<u8>, &'static str> {
    use pdf_oxide::rendering::{render_page, RenderOptions};
    let current_pdf = PdfDocument::from_bytes(current.clone()).map_err(|_| OUTPUT_ERROR)?;
    let retained = assembly::Source::open(source)?;
    let prior = retained.document();
    let frame = assembly::DisplayFrame::new(&current_pdf, index)?;
    let previous = assembly::DisplayFrame::new(prior, index)?;
    let (width, height) = frame.size;
    if frame.geometry.rotation != previous.geometry.rotation
        || (width - previous.size.0).abs() > 0.01
        || (height - previous.size.1).abs() > 0.01
        || prior.page_count().map_err(|_| OUTPUT_ERROR)?
            != current_pdf.page_count().map_err(|_| OUTPUT_ERROR)?
    {
        return Err("Page structure changed; the selected area cannot be restored safely");
    }
    let (width, height) = frame.visible.display_size();
    let transform = frame.visible.display_matrix();
    let region = BoxOnPage::relative(footprint, width, height)?
        .transformed(transform)
        .bounds();
    let later_regions = later
        .iter()
        .map(|item| {
            BoxOnPage::relative(item, width, height)
                .map(|area| area.transformed(transform).bounds())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let [source_left, source_bottom, source_right, source_top] = previous.visible.crop;
    let [left, bottom, right, top] = [
        region[0].max(source_left),
        region[1].max(source_bottom),
        region[2].min(source_right),
        region[3].min(source_top),
    ];
    if right <= left || top <= bottom {
        return Err("Restore region is empty");
    }

    // Write a one-page rendering copy with the source origin preserved and
    // the selected rectangle as its viewport. Only its pixels are reused.
    let selection = assembly::assemble_bundle(
        std::slice::from_ref(&retained),
        &[(0, index)],
        &HashMap::new(),
        &assembly::Navigation::default(),
    )?;
    let mut viewport = DocumentEditor::from_bytes(selection).map_err(|_| OUTPUT_ERROR)?;
    viewport.set_page_rotation(0, 0).map_err(|_| OUTPUT_ERROR)?;
    viewport
        .set_page_crop_box(0, [left as f32, bottom as f32, right as f32, top as f32])
        .map_err(|_| OUTPUT_ERROR)?;
    let viewport = PdfDocument::from_bytes(viewport.save_to_bytes().map_err(|_| OUTPUT_ERROR)?)
        .map_err(|_| OUTPUT_ERROR)?;
    let mut options =
        RenderOptions::with_dpi((144.0 * previous.unit).ceil().max(1.0) as u32).as_raw();
    options.max_output_pixels = 16_000_000;
    options.render_annotations = false;
    let raster = render_page(&viewport, 0, &options)
        .map_err(|_| "Unable to render the retained source region")?;
    let dx = (right - left) / f64::from(raster.width);
    let dy = (top - bottom) / f64::from(raster.height);
    let mut crop =
        image::RgbaImage::from_raw(raster.width, raster.height, raster.data).ok_or(OUTPUT_ERROR)?;
    for (x, y, pixel) in crop.enumerate_pixels_mut() {
        let pixel_left = left + f64::from(x) * dx;
        let pixel_top = top - f64::from(y) * dy;
        let rect = [pixel_left, pixel_top - dy, pixel_left + dx, pixel_top];
        if later_regions
            .iter()
            .any(|other| super::text_edit::intersects(rect, *other))
        {
            pixel.0 = [0, 0, 0, 255];
        } else {
            let alpha = u16::from(pixel.0[3]);
            for channel in &mut pixel.0[..3] {
                *channel = ((u16::from(*channel) * alpha + 255 * (255 - alpha)) / 255) as u8;
            }
            pixel.0[3] = 255;
        }
    }
    let [base_x, base_y, max_x, max_y] = frame.visible.crop;
    let (raw_width, raw_height) = (max_x - base_x, max_y - base_y);
    let source_spans = prior.extract_spans(index).map_err(|_| OUTPUT_ERROR)?;
    let words = source_spans
        .into_iter()
        .filter(|span| {
            !span.text.is_empty() && span.text.bytes().all(|byte| (32..=126).contains(&byte))
        })
        .filter_map(|span| {
            let bounds = super::text_edit::span_user_bbox(previous.geometry, &span);
            let [x0, y0, x1, y1] = bounds;
            if x0 < left
                || y0 < bottom
                || x1 > right
                || y1 > top
                || later_regions
                    .iter()
                    .any(|other| super::text_edit::intersects(bounds, *other))
            {
                return None;
            }
            Some(super::tesseract::Word {
                text: span.text,
                confidence: 0.0,
                rect: [
                    (x0 - base_x) / raw_width,
                    (max_y - y1) / raw_height,
                    (x1 - x0) / raw_width,
                    (y1 - y0) / raw_height,
                ],
            })
        })
        .collect::<Vec<_>>();
    let mut overlay = if words.is_empty() {
        assembly::Overlay::default()
    } else {
        super::ocr::layer(&words, (raw_width, raw_height))?
    };
    overlay.ocr = false;
    overlay.unrotated_page_coordinates = true;
    append_raster(
        &mut overlay,
        crop,
        BoxOnPage {
            left: left - base_x,
            bottom: bottom - base_y,
            width: right - left,
            height: top - bottom,
        },
        false,
    )?;
    drop(current_pdf);
    let current = destructive_redaction_inner(
        current,
        &[(index, region.map(|point| point as f32))],
        0,
        false,
    )?;
    assembly::overlay_document(current, &HashMap::from([(index, overlay)]))
}

pub(super) fn painted_box(
    matrix: pdf_oxide::content::Matrix,
    [x0, y0, x1, y1]: [f32; 4],
) -> Result<[f64; 4], &'static str> {
    let mut bounds = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for (x, y) in [(x0, y0), (x0, y1), (x1, y0), (x1, y1)] {
        let point = matrix.transform_point(x, y);
        if !point.x.is_finite() || !point.y.is_finite() {
            return Err(OUTPUT_ERROR);
        }
        bounds[0] = bounds[0].min(f64::from(point.x));
        bounds[1] = bounds[1].min(f64::from(point.y));
        bounds[2] = bounds[2].max(f64::from(point.x));
        bounds[3] = bounds[3].max(f64::from(point.y));
    }
    if bounds[2] <= bounds[0] || bounds[3] <= bounds[1] {
        return Err(OUTPUT_ERROR);
    }
    Ok(bounds)
}

pub(super) fn form_geometry(
    pdf: &PdfDocument,
    metadata: &HashMap<String, Object>,
    placement: pdf_oxide::content::Matrix,
) -> Result<([f32; 4], pdf_oxide::content::Matrix), &'static str> {
    use pdf_oxide::content::Matrix;
    fn tuple<const N: usize>(value: &Object) -> Result<[f32; N], &'static str> {
        let source = value
            .as_array()
            .filter(|fields| fields.len() == N)
            .ok_or(OUTPUT_ERROR)?;
        let mut result = [0.; N];
        for (target, value) in result.iter_mut().zip(source) {
            *target = match value {
                Object::Real(number) => *number as f32,
                Object::Integer(number) => *number as f32,
                _ => return Err(OUTPUT_ERROR),
            };
        }
        Ok(result)
    }
    let bounds = pdf
        .resolve_object(metadata.get("BBox").ok_or(OUTPUT_ERROR)?)
        .map_err(|_| OUTPUT_ERROR)?;
    let bounds = tuple::<4>(&bounds)?;
    let Some(form) = metadata.get("Matrix") else {
        return Ok((bounds, placement));
    };
    let matrix = pdf.resolve_object(form).map_err(|_| OUTPUT_ERROR)?;
    let [a, b, c, d, e, f] = tuple::<6>(&matrix)?;
    let form = Matrix { a, b, c, d, e, f };
    Ok((bounds, form.multiply(&placement)))
}

fn redact_form(
    bytes: &[u8],
    object: &Object,
    parent_resources: &Object,
    bounds: [f64; 4],
    transform: pdf_oxide::content::Matrix,
    selected: &[&[f32; 4]],
    depth: usize,
) -> Result<assembly::XObjectReplacement, &'static str> {
    use pdf_oxide::content::Matrix;
    let det = transform.determinant();
    let Matrix { a, b, c, d, e, f } = transform;
    if !det.is_finite() || det.abs() < 1e-12 {
        return Err("A partially covered form has a singular transform");
    }
    let inverse = [
        d / det,
        -b / det,
        -c / det,
        a / det,
        (c * f - d * e) / det,
        (b * e - a * f) / det,
    ];
    if inverse.iter().any(|value| !value.is_finite()) {
        return Err("A partially covered form has an invalid transform");
    }
    let regions = selected.iter().map(|&&area| (0, area)).collect::<Vec<_>>();
    // Analyze the isolated content in page coordinates. Reversing the
    // projection when importing it restores the Form's original coordinate
    // system and preserves its clip, including rotation and shear.
    let proxy =
        assembly::form_document(bytes, object, parent_resources, bounds, [a, b, c, d, e, f])?;
    let body = destructive_redaction_inner(proxy, &regions, depth + 1, false)?;
    let fields = object
        .as_dict()
        .ok_or(OUTPUT_ERROR)?
        .iter()
        .filter(|(key, _)| {
            matches!(
                key.as_str(),
                "Type" | "Subtype" | "FormType" | "BBox" | "Matrix" | "Group" | "OC"
            )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Ok(assembly::XObjectReplacement::Form {
        body,
        fields,
        inverse,
    })
}

fn redacted_xobjects(
    pdf: &PdfDocument,
    bytes: &[u8],
    page: usize,
    regions: &[[f32; 4]],
    depth: usize,
) -> Result<assembly::Overlay, &'static str> {
    use assembly::{XObjectEdit, XObjectReplacement};
    use pdf_oxide::content::{Matrix, Operator};
    let data = pdf.get_page_content_data(page).map_err(|_| OUTPUT_ERROR)?;
    let operators = pdf_oxide::content::parse_content_stream(&data).map_err(|_| OUTPUT_ERROR)?;
    if operators
        .iter()
        .any(|op| matches!(op, Operator::InlineImage { .. }))
    {
        return Err("Unable to isolate the inline image before redaction");
    }
    let mut paints = Vec::new();
    let mut state = Matrix::identity();
    let mut saved = Vec::new();
    for operator in &operators {
        match operator {
            Operator::SaveState => saved.push(state),
            Operator::RestoreState => state = saved.pop().ok_or(OUTPUT_ERROR)?,
            Operator::Cm { a, b, c, d, e, f } => {
                state = Matrix {
                    a: *a,
                    b: *b,
                    c: *c,
                    d: *d,
                    e: *e,
                    f: *f,
                }
                .multiply(&state)
            }
            Operator::Do { name } => paints.push((name.as_str(), state)),
            _ => {}
        }
    }
    if paints.is_empty() {
        return Ok(assembly::Overlay {
            replace_page_content: Some(data),
            retain_xobjects: Some(Default::default()),
            redact_content: true,
            ..Default::default()
        });
    }
    let resources = pdf.get_page_resources(page).map_err(|_| OUTPUT_ERROR)?;
    let xobjects = resources
        .as_dict()
        .and_then(|resources| resources.get("XObject"))
        .ok_or(OUTPUT_ERROR)?;
    let xobjects = pdf.resolve_object(xobjects).map_err(|_| OUTPUT_ERROR)?;
    let xobjects = xobjects.as_dict().ok_or(OUTPUT_ERROR)?;
    let draws = super::redaction_vectors::draw_commands(&data)?;
    if paints.len() != draws.len() {
        return Err("Could not identify every PDF resource placement");
    }
    let mut replacements = Vec::new();
    let mut keep = std::collections::HashSet::new();
    let mut original_paints = std::collections::HashSet::new();
    let mut updated = Vec::new();
    let mut offset = 0;
    let prefix = (0..)
        .map(|n| format!("MatterDraftRedaction{n}_"))
        .find(|prefix| xobjects.keys().all(|key| !key.starts_with(prefix)))
        .ok_or(OUTPUT_ERROR)?;
    for (index, (&(key, placement), range)) in paints.iter().zip(draws).enumerate() {
        updated.extend_from_slice(&data[offset..range.start]);
        offset = range.end;
        let original = xobjects.get(key).ok_or(OUTPUT_ERROR)?;
        let object = pdf.resolve_object(original).map_err(|_| OUTPUT_ERROR)?;
        let metadata = object.as_dict().ok_or(OUTPUT_ERROR)?;
        let form = match metadata.get("Subtype").and_then(Object::as_name) {
            Some("Image") => None,
            Some("Form") => Some(form_geometry(pdf, metadata, placement)?),
            // PostScript XObjects have no PDF viewer appearance. Drop their
            // alternate program from the sanitized target page.
            Some("PS") => continue,
            _ => return Err("Unsupported PDF visual resource in a redaction area"),
        };
        let bounds = if let Some((local, transform)) = form {
            painted_box(transform, local)?
        } else {
            painted_box(placement, [0., 0., 1., 1.])?
        };
        let selected = regions
            .iter()
            .filter(|region| super::text_edit::intersects(bounds, region.map(f64::from)))
            .collect::<Vec<_>>();
        if selected.is_empty() {
            original_paints.insert(key.to_owned());
            updated.extend_from_slice(&data[range]);
            continue;
        }
        let target = format!("{prefix}{index}");
        write!(updated, "/{target} Do ").map_err(|_| OUTPUT_ERROR)?;
        keep.insert(target.clone());
        let fully_covered = selected.iter().any(|area| {
            let area = area.map(f64::from);
            area[0] <= bounds[0]
                && area[1] <= bounds[1]
                && area[2] >= bounds[2]
                && area[3] >= bounds[3]
        });
        let replacement = if fully_covered {
            XObjectReplacement::EmptyForm
        } else if let Some((_, transform)) = form {
            redact_form(
                bytes, &object, &resources, bounds, transform, &selected, depth,
            )?
        } else {
            redact_image(
                pdf, &resources, original, &object, metadata, placement, &selected,
            )?
        };
        replacements.push(XObjectEdit {
            source: Some(key.to_owned()),
            target,
            replacement,
        });
    }
    updated.extend_from_slice(&data[offset..]);
    super::redaction_vectors::retain_inherited_xobjects(pdf, xobjects, &mut original_paints)?;
    keep.extend(original_paints);
    Ok(assembly::Overlay {
        replace_page_content: Some(updated),
        retain_xobjects: Some(keep),
        redact_content: true,
        xobject_edits: replacements,
        ..Default::default()
    })
}

fn image_dimension(
    pdf: &PdfDocument,
    metadata: &HashMap<String, Object>,
    field: &str,
) -> Result<u32, &'static str> {
    metadata
        .get(field)
        .and_then(|value| pdf.resolve_object(value).ok())
        .and_then(|value| value.as_integer())
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or(OUTPUT_ERROR)
}

fn image_components(pdf: &PdfDocument, space: &Object) -> Option<usize> {
    let array = space
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(std::slice::from_ref(space));
    match array.first()?.as_name()? {
        "DeviceGray" | "CalGray" | "Indexed" | "Separation" => Some(1),
        "DeviceRGB" | "CalRGB" | "Lab" => Some(3),
        "DeviceCMYK" => Some(4),
        "DeviceN" => {
            let inks = pdf.resolve_object(array.get(1)?).ok()?;
            let inks = inks
                .as_array()
                .filter(|inks| (1..=32).contains(&inks.len()))?;
            inks.iter()
                .all(|ink| {
                    pdf.resolve_object(ink)
                        .is_ok_and(|ink| ink.as_name().is_some())
                })
                .then_some(inks.len())
        }
        "ICCBased" => {
            let profile = pdf.resolve_object(array.get(1)?).ok()?;
            let n = pdf
                .resolve_object(profile.as_dict()?.get("N")?)
                .ok()?
                .as_integer()?;
            matches!(n, 1 | 3 | 4).then_some(n as usize)
        }
        _ => None,
    }
}

fn redact_image(
    pdf: &PdfDocument,
    resources: &Object,
    original: &Object,
    object: &Object,
    metadata: &HashMap<String, Object>,
    placement: pdf_oxide::content::Matrix,
    selected: &[&[f32; 4]],
) -> Result<assembly::XObjectReplacement, &'static str> {
    let width = image_dimension(pdf, metadata, "Width")?;
    let height = image_dimension(pdf, metadata, "Height")?;
    if u64::from(width) * u64::from(height) > IMAGE_LIMIT {
        return Err("The image placement or masking cannot be redacted safely by this writer");
    }
    if metadata.get("ImageMask") == Some(&Object::Boolean(true)) {
        return redact_stencil(object, metadata, width, height, placement, selected);
    }
    let color_spaces = resources
        .as_dict()
        .and_then(|fields| fields.get("ColorSpace"))
        .map(|value| pdf.resolve_object(value))
        .transpose()
        .map_err(|_| OUTPUT_ERROR)?;
    let jpeg2000 = jpx_redaction::mask_mode(pdf, metadata)?;
    let color = metadata
        .get("ColorSpace")
        .map(|space| {
            let space = pdf.resolve_object(space).map_err(|_| OUTPUT_ERROR)?;
            if let Some(named) = space
                .as_name()
                .filter(|name| !matches!(*name, "DeviceGray" | "DeviceRGB" | "DeviceCMYK"))
            {
                let named = color_spaces
                    .as_ref()
                    .and_then(Object::as_dict)
                    .and_then(|spaces| spaces.get(named))
                    .ok_or("The source image color space is missing")?;
                pdf.resolve_object(named).map_err(|_| OUTPUT_ERROR)
            } else {
                Ok(space)
            }
        })
        .transpose()?
        .filter(|space| !matches!(space, Object::Null));
    let color_kind = color.as_ref().and_then(|color| {
        color.as_name().or_else(|| {
            color
                .as_array()
                .and_then(|fields| fields.first())
                .and_then(Object::as_name)
        })
    });
    if color.is_none() && jpeg2000.is_none() {
        return Err("The source image has no declared color space");
    }
    if !matches!(
        color_kind,
        None | Some(
            "DeviceGray"
                | "DeviceRGB"
                | "DeviceCMYK"
                | "Indexed"
                | "ICCBased"
                | "CalGray"
                | "CalRGB"
                | "Lab"
                | "Separation"
                | "DeviceN"
        )
    ) {
        return Err("The source image color space cannot be redacted safely");
    }
    let (decoded_object, embedded_alpha) = if let Some(mode) = jpeg2000 {
        let (mut image, opacity) = jpx_redaction::samples(
            object,
            (width, height),
            color.as_ref(),
            color_kind == Some("Indexed"),
            mode,
        )?;
        if let Object::Stream { dict, .. } = &mut image {
            for name in ["Intent", "Interpolate", "Mask"] {
                if let Some(value) = metadata.get(name) {
                    dict.insert(name.into(), value.clone());
                }
            }
        }
        (Some(image), opacity)
    } else {
        (None, None)
    };
    let soft_mask = match embedded_alpha {
        Some(alpha) => Some((alpha, None)),
        None => soft_image_mask(pdf, metadata, width, height)?,
    };
    let color = color
        .as_ref()
        .or_else(|| decoded_object.as_ref()?.as_dict()?.get("ColorSpace"));
    let components = color.and_then(|color| image_components(pdf, color));
    let source = decoded_object.as_ref().unwrap_or(object);
    let source_fields = source.as_dict().ok_or(OUTPUT_ERROR)?;
    let source_ref = if decoded_object.is_some() {
        None
    } else {
        original.as_reference()
    };
    let jpeg_color_key = if let Some(mask) = metadata.get("Mask") {
        pdf.resolve_object(mask)
            .map_err(|_| OUTPUT_ERROR)?
            .as_array()
            .is_some()
            && matches!(
                native_image_redaction::last_filter(pdf, source_fields)?.as_deref(),
                Some("DCTDecode" | "DCT")
            )
    } else {
        false
    };
    let use_native = matches!(color_kind, Some("Separation" | "DeviceN"))
        || jpeg_color_key
        || (soft_mask.as_ref().is_some_and(|(_, matte)| matte.is_some())
            && matches!(
                color_kind,
                Some("ICCBased" | "CalRGB" | "CalGray" | "Lab" | "DeviceCMYK")
            ));
    if use_native {
        return native_image_redaction::redact(
            pdf,
            source_ref,
            source,
            source_fields,
            color.cloned().ok_or(OUTPUT_ERROR)?,
            components.ok_or("Cannot resolve the original image color channels")?,
            (width, height),
            soft_mask,
            placement,
            selected,
        );
    }
    let decoded = pdf_oxide::extractors::extract_image_from_xobject(
        Some(pdf),
        source,
        source_ref,
        color_spaces.as_ref().and_then(Object::as_dict),
    )
    .map_err(|_| "Cannot decode the source image for redaction")?;
    let png = decoded
        .to_png_bytes()
        .map_err(|_| "Cannot read the source image samples")?;
    let mut samples = decode_image(&png, ImageFormat::Png)?.into_rgb8();
    let (w, h) = samples.dimensions();
    if (w, h) != (width, height) {
        return Err("The decoded image size differs from its PDF placement");
    }
    let mut alpha = match soft_mask {
        Some((opacity, matte)) => {
            if let Some(matte) = matte {
                unblend_matte(&mut samples, &opacity, &matte, color_kind)?;
            }
            Some(opacity)
        }
        None => match metadata
            .get("Mask")
            .map(|mask| pdf.resolve_object(mask))
            .transpose()
            .map_err(|_| OUTPUT_ERROR)?
        {
            None | Some(Object::Null) => None,
            Some(Object::Array(range)) => {
                let channels = components.ok_or("Cannot resolve the image color key channels")?;
                let bits = native_image_redaction::bits_per_component(pdf, source_fields)?;
                let original_samples = native_image_redaction::packed_image(
                    pdf,
                    source,
                    source_ref,
                    source_fields,
                    (w, h),
                    channels,
                    bits,
                )?;
                Some(original_samples.color_key(pdf, &range)?)
            }
            Some(stencil) => Some(explicit_stencil_mask(&stencil, w, h)?),
        },
    };
    redact_samples(w, h, placement, selected, |x, y| {
        samples.put_pixel(x, y, image::Rgb([0, 0, 0]));
        if let Some(mask) = &mut alpha {
            mask.put_pixel(x, y, image::Luma([255]));
        }
    })?;
    let mut replacement = image_stream(w, h, "DeviceRGB", samples.as_raw())?;
    if let (Object::Stream { dict, .. }, Some(value @ Object::Boolean(_))) =
        (&mut replacement, metadata.get("Interpolate"))
    {
        dict.insert("Interpolate".into(), value.clone());
    }
    let mask = alpha
        .map(|alpha| image_stream(w, h, "DeviceGray", alpha.as_raw()))
        .transpose()?;
    Ok(assembly::XObjectReplacement::Image(replacement, mask))
}

type ImageSoftMask = (image::GrayImage, Option<Object>);

fn soft_image_mask(
    pdf: &PdfDocument,
    fields: &HashMap<String, Object>,
    width: u32,
    height: u32,
) -> Result<Option<ImageSoftMask>, &'static str> {
    const INVALID: &str = "This source image mask cannot be redacted safely";
    let Some(reference) = fields.get("SMask") else {
        return Ok(None);
    };
    let stream = pdf
        .resolve_object(reference)
        .map_err(|_| "Unable to open the source image mask")?;
    if matches!(stream, Object::Null) || stream.as_name() == Some("None") {
        return Ok(None);
    }
    let mask = stream.as_dict().ok_or("Invalid image mask resource")?;
    if mask.get("Subtype").and_then(Object::as_name) != Some("Image")
        || mask.get("ColorSpace").and_then(Object::as_name) != Some("DeviceGray")
        || ["SMask", "ImageMask", "Mask", "SMaskInData"]
            .iter()
            .any(|key| {
                mask.get(*key)
                    .is_some_and(|value| !matches!(value, Object::Null))
            })
    {
        return Err(INVALID);
    }
    let (mask_w, mask_h) = (
        image_dimension(pdf, mask, "Width")?,
        image_dimension(pdf, mask, "Height")?,
    );
    if u64::from(mask_w) * u64::from(mask_h) > IMAGE_LIMIT {
        return Err(INVALID);
    }
    let matte = mask
        .get("Matte")
        .map(|value| pdf.resolve_object(value))
        .transpose()
        .map_err(|_| INVALID)?
        .filter(|value| !matches!(value, Object::Null));
    if matte.is_some() && (mask_w, mask_h) != (width, height) {
        return Err("A preblended soft mask must match the source image dimensions");
    }
    let decoded = pdf_oxide::extractors::extract_image_from_xobject(
        Some(pdf),
        &stream,
        reference.as_reference(),
        None,
    )
    .and_then(|image| image.to_png_bytes())
    .map_err(|_| "Cannot decode the source image mask")?;
    let alpha = decode_image(&decoded, ImageFormat::Png)?.into_luma8();
    if alpha.dimensions() != (mask_w, mask_h) {
        return Err(INVALID);
    }
    Ok(Some((resize_mask(alpha, width, height), matte)))
}

fn resize_mask(alpha: image::GrayImage, width: u32, height: u32) -> image::GrayImage {
    let (source_w, source_h) = alpha.dimensions();
    if (source_w, source_h) == (width, height) {
        return alpha;
    }
    image::GrayImage::from_fn(width, height, |x, y| {
        let source_x = (u64::from(x) * u64::from(source_w) / u64::from(width)) as u32;
        let source_y = (u64::from(y) * u64::from(source_h) / u64::from(height)) as u32;
        *alpha.get_pixel(source_x, source_y)
    })
}

fn unblend_matte(
    samples: &mut image::RgbImage,
    alpha: &image::GrayImage,
    matte: &Object,
    color: Option<&str>,
) -> Result<(), &'static str> {
    const INVALID: &str = "Cannot remove this color-space matte from an image mask safely";
    let array = matte.as_array().ok_or(INVALID)?;
    let component = |value: &Object| {
        let value = match value {
            Object::Integer(value) => *value as f64,
            Object::Real(value) => *value,
            _ => return Err(INVALID),
        };
        (0.0..=1.0)
            .contains(&value)
            .then_some(value * 255.0)
            .ok_or(INVALID)
    };
    let rgb = match (color, array.as_slice()) {
        (Some("DeviceGray"), [gray]) => [component(gray)?; 3],
        (Some("DeviceRGB"), [red, green, blue]) => {
            [component(red)?, component(green)?, component(blue)?]
        }
        _ => return Err(INVALID),
    };
    for (pixel, mask) in samples.pixels_mut().zip(alpha.pixels()) {
        if mask[0] == 0 {
            *pixel = image::Rgb([0; 3]);
            continue;
        }
        let opacity = f64::from(mask[0]) / 255.0;
        for (sample, matte) in pixel.0.iter_mut().zip(rgb) {
            *sample = ((f64::from(*sample) - (1.0 - opacity) * matte) / opacity)
                .clamp(0.0, 255.0)
                .round() as u8;
        }
    }
    Ok(())
}

fn explicit_stencil_mask(
    mask: &Object,
    width: u32,
    height: u32,
) -> Result<image::GrayImage, &'static str> {
    const INVALID: &str = "This source image mask cannot be redacted safely";
    let fields = mask.as_dict().ok_or(INVALID)?;
    if fields.get("Subtype").and_then(Object::as_name) != Some("Image")
        || fields.get("ImageMask") != Some(&Object::Boolean(true))
    {
        return Err(INVALID);
    }
    let size = |field| {
        fields
            .get(field)
            .and_then(Object::as_integer)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or(INVALID)
    };
    let (mask_w, mask_h) = (size("Width")?, size("Height")?);
    if u64::from(mask_w) * u64::from(mask_h) > IMAGE_LIMIT {
        return Err(INVALID);
    }
    let (samples, transparent_bit) = stencil_samples(mask, fields, mask_w, mask_h)?;
    let stride = mask_w.div_ceil(8) as usize;
    let alpha = image::GrayImage::from_fn(mask_w, mask_h, |x, y| {
        let set = samples[y as usize * stride + (x / 8) as usize] & (0x80 >> (x % 8)) != 0;
        image::Luma([if set == transparent_bit { 0 } else { 255 }])
    });
    Ok(resize_mask(alpha, width, height))
}

fn redact_stencil(
    object: &Object,
    metadata: &HashMap<String, Object>,
    width: u32,
    height: u32,
    placement: pdf_oxide::content::Matrix,
    selected: &[&[f32; 4]],
) -> Result<assembly::XObjectReplacement, &'static str> {
    let row_bytes = width.div_ceil(8) as usize;
    let (mut samples, clear_bit) = stencil_samples(object, metadata, width, height)?;
    redact_samples(width, height, placement, selected, |x, y| {
        let byte = &mut samples[y as usize * row_bytes + (x / 8) as usize];
        let bit = 0x80 >> (x % 8);
        if clear_bit {
            *byte |= bit;
        } else {
            *byte &= !bit;
        }
    })?;
    let mut dict = HashMap::from([
        ("Type".into(), Object::Name("XObject".into())),
        ("Subtype".into(), Object::Name("Image".into())),
        ("Width".into(), Object::Integer(i64::from(width))),
        ("Height".into(), Object::Integer(i64::from(height))),
        ("ImageMask".into(), Object::Boolean(true)),
        ("BitsPerComponent".into(), Object::Integer(1)),
        ("Filter".into(), Object::Name("FlateDecode".into())),
    ]);
    if let Some(decode) = metadata.get("Decode") {
        dict.insert("Decode".into(), decode.clone());
    }
    if let Some(interpolate @ Object::Boolean(_)) = metadata.get("Interpolate") {
        dict.insert("Interpolate".into(), interpolate.clone());
    }
    let replacement = Object::Stream {
        dict,
        data: compress(&samples)?.into(),
    };
    Ok(assembly::XObjectReplacement::Image(replacement, None))
}

fn stencil_samples(
    object: &Object,
    metadata: &HashMap<String, Object>,
    width: u32,
    height: u32,
) -> Result<(Vec<u8>, bool), &'static str> {
    const INVALID: &str = "Cannot redact the source image stencil";
    if ["Mask", "SMask", "SMaskInData"]
        .iter()
        .any(|key| metadata.contains_key(*key))
        || metadata
            .get("BitsPerComponent")
            .is_some_and(|value| value.as_integer() != Some(1))
    {
        return Err(INVALID);
    }
    let clear_bit = match metadata.get("Decode") {
        None => true,
        Some(Object::Array(range)) => {
            let number = |value: &Object| match value {
                Object::Integer(n) => Some(*n as f64),
                Object::Real(n) => Some(*n),
                _ => None,
            };
            match range.as_slice() {
                [first, last] if number(first) == Some(0.0) && number(last) == Some(1.0) => true,
                [first, last] if number(first) == Some(1.0) && number(last) == Some(0.0) => false,
                _ => return Err(INVALID),
            }
        }
        _ => return Err(INVALID),
    };
    let row_bytes = width.div_ceil(8) as usize;
    let samples = object.decode_stream_data().map_err(|_| INVALID)?;
    if samples.len() != row_bytes * height as usize {
        return Err(INVALID);
    }
    Ok((samples, clear_bit))
}

fn redact_samples(
    w: u32,
    h: u32,
    placement: pdf_oxide::content::Matrix,
    selected: &[&[f32; 4]],
    mut clear: impl FnMut(u32, u32),
) -> Result<(), &'static str> {
    let [a, b, c, d, e, f] = [
        placement.a,
        placement.b,
        placement.c,
        placement.d,
        placement.e,
        placement.f,
    ]
    .map(f64::from);
    let determinant = a * d - b * c;
    if ![a, b, c, d, e, f, determinant]
        .iter()
        .all(|value| value.is_finite())
        || determinant.abs() <= 1e-12
    {
        return Err("Cannot resolve the transformed image sample positions");
    }
    let (width, height) = (f64::from(w), f64::from(h));
    let axis = |x: f64, y: f64| {
        let (x, y) = (x - e, y - f);
        (
            (d * x - c * y) / determinant * width,
            (1.0 - (a * y - b * x) / determinant) * height,
        )
    };
    let (col_x, col_y, row_x, row_y) = (a / width, b / width, -c / height, -d / height);
    let x_min = col_x.min(0.0) + row_x.min(0.0);
    let x_max = col_x.max(0.0) + row_x.max(0.0);
    let y_min = col_y.min(0.0) + row_y.min(0.0);
    let y_max = col_y.max(0.0) + row_y.max(0.0);
    for region in selected {
        let [left, bottom, right, top] = region.map(f64::from);
        let (u_min, v_min, u_max, v_max) =
            [(left, bottom), (right, bottom), (right, top), (left, top)]
                .into_iter()
                .map(|(x, y)| axis(x, y))
                .fold(
                    (
                        f64::INFINITY,
                        f64::INFINITY,
                        f64::NEG_INFINITY,
                        f64::NEG_INFINITY,
                    ),
                    |(u0, v0, u1, v1), (u, v)| (u0.min(u), v0.min(v), u1.max(u), v1.max(v)),
                );
        let cols =
            (u_min.floor().clamp(0.0, width) as u32)..(u_max.ceil().clamp(0.0, width) as u32);
        let rows =
            (v_min.floor().clamp(0.0, height) as u32)..(v_max.ceil().clamp(0.0, height) as u32);
        for y in rows {
            let row_left = e + c + row_x * f64::from(y);
            let row_bottom = f + d + row_y * f64::from(y);
            for x in cols.clone() {
                let ox = row_left + col_x * f64::from(x);
                let oy = row_bottom + col_y * f64::from(x);
                if ox + x_min < right
                    && ox + x_max > left
                    && oy + y_min < top
                    && oy + y_max > bottom
                {
                    clear(x, y);
                }
            }
        }
    }
    Ok(())
}
