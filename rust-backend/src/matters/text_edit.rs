//! Revision-qualified native PDF text run inspection and bounded replacement.
use super::{
    artifacts::{refusal, Artifact},
    document_output::{append_text, non_opaque_text, text_lines, EmbeddedParagraphFont},
    page_tools::{assembly, pdf_annotations, signature_guard},
    shaped_text::{Shaper, CONTINUATION},
    *,
};
use pdf_oxide::{
    extractors::TextExtractor,
    fonts::{encoding::unicode_to_winansi, FontInfo},
    layout::{TextChar, TextSpan},
    object::Object,
    structure::McidScope,
    PdfDocument,
};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::{fmt::Write as _, sync::Arc};

mod actual_text;
mod font_state;
pub(super) mod reading_order;
mod reflow;
mod request;
mod run_edit;
mod standard_ink;

const INVALID: &str = "Unable to edit the selected PDF text";
const TRIAL_ENGINE: &str = "pdf-oxide-paragraph";
const TRIAL_BYTES: usize = 16 * 1024 * 1024;
const TRIAL_PARAGRAPH_CHARS: usize = 2_000;
const TRIAL_PIXEL_AREA: f64 = 8_000_000.0;
type Rect = [f64; 4];

fn paragraph_trial_disabled() -> bool {
    std::env::var_os("MATTERDRAFT_ENABLE_PDFIUM_TRIAL")
        .or_else(|| std::env::var_os("COURTBUNDLE_ENABLE_PDFIUM_TRIAL"))
        .is_some_and(|value| value == "0")
}

pub(super) fn bbox(span: &TextSpan) -> Rect {
    let box_ = span.bbox;
    [
        box_.x.into(),
        box_.y.into(),
        (box_.x + box_.width).into(),
        (box_.y + box_.height).into(),
    ]
}

// PDF Oxide maps some spans through /Rotate about the MediaBox, while other
// spans remain in PDF user space. Resolve that distinction before applying
// display-relative CropBox geometry or comparing against page operators.
fn span_to_user(
    geometry: assembly::Geometry,
    rect: pdf_oxide::geometry::Rect,
    rotation: i32,
) -> Rect {
    let x0 = f64::from(rect.x);
    let y0 = f64::from(rect.y);
    let x1 = x0 + f64::from(rect.width);
    let y1 = y0 + f64::from(rect.height);
    if rotation == 0 {
        return [x0, y0, x1, y1];
    }
    let media = assembly::Geometry {
        crop: geometry.media,
        rotation,
        ..geometry
    };
    let [left, low, _, _] = media.crop;
    let (a, b) = media.user_point(x0 - left, y0 - low);
    let (c, d) = media.user_point(x1 - left, y1 - low);
    [a.min(c), b.min(d), a.max(c), b.max(d)]
}

pub(super) fn span_user_bbox(geometry: assembly::Geometry, span: &TextSpan) -> Rect {
    span_to_user(geometry, span.page_bbox(), span.page_rotation_applied)
}

fn working_spans(
    pdf: &PdfDocument,
    page: usize,
    geometry: assembly::Geometry,
) -> Result<Vec<TextSpan>, pdf_oxide::Error> {
    let mut spans = pdf.extract_spans(page)?;
    for span in &mut spans {
        if span.page_rotation_applied == 0 {
            continue;
        }
        let [x0, y0, x1, y1] = span_to_user(geometry, span.bbox, span.page_rotation_applied);
        span.bbox = pdf_oxide::geometry::Rect::new(
            x0 as f32,
            y0 as f32,
            (x1 - x0) as f32,
            (y1 - y0) as f32,
        );
        span.page_rotation_applied = 0;
    }
    Ok(spans)
}

fn contains(outer: Rect, inner: Rect) -> bool {
    outer[0] <= inner[0] && outer[1] <= inner[1] && outer[2] >= inner[2] && outer[3] >= inner[3]
}

pub(super) fn intersects(a: Rect, b: Rect) -> bool {
    a[0] < b[2] && a[2] > b[0] && a[1] < b[3] && a[3] > b[1]
}

fn first_glyph<'a>(
    span: &TextSpan,
    chars: impl IntoIterator<Item = &'a TextChar>,
) -> Option<&'a TextChar> {
    let area = bbox(span);
    let first = span.text.chars().next()?;
    // PDF Oxide names Form runs by resource key and individual glyphs by base font.
    let inside_form = matches!(span.mcid_scope, Some(McidScope::Form(_)));
    let mut candidates = chars.into_iter().filter(|c| {
        c.char == first
            && (c.font_name == span.font_name || inside_form)
            && (c.font_size - span.font_size).abs() < 0.01
            && (f64::from(c.bbox.x) - area[0]).abs() < 1.0
            && (f64::from(c.bbox.y) - area[1]).abs() < 1.0
    });
    let first = candidates.next()?;
    candidates.next().is_none().then_some(first)
}

fn paragraph_glyph<'a>(span: &TextSpan, chars: &'a [TextChar]) -> Option<&'a TextChar> {
    let area = bbox(span);
    chars
        .iter()
        .filter(|character| {
            character.font_name == span.font_name
                && (character.font_size - span.font_size).abs() < 0.01
                && character.rotation_degrees.abs() < 0.001
                && character.advance_width > 0.0
                && intersects(
                    area,
                    [
                        f64::from(character.bbox.x),
                        f64::from(character.bbox.y),
                        f64::from(character.bbox.x + character.bbox.width),
                        f64::from(character.bbox.y + character.bbox.height),
                    ],
                )
        })
        .min_by(|a, b| a.origin_x.total_cmp(&b.origin_x))
}

fn editable(span: &TextSpan, crop: Rect, bounds: Rect) -> bool {
    let font = span.font_name.to_ascii_lowercase();
    !span.text.trim().is_empty()
        && span.rotation_degrees.abs() < 0.001
        && span.wmode == 0
        && !font.contains("glyphless")
        && !font.contains("matterdraftocr")
        && span.bbox.width > 0.0
        && span.bbox.height > 0.0
        && contains(crop, bounds)
}

pub(super) fn displayed(geometry: assembly::Geometry, rect: Rect) -> Rect {
    let [a, b, c, d, e, f] = geometry.display_matrix();
    let to_display = |x: f64, y: f64| (a * (x - e) + b * (y - f), c * (x - e) + d * (y - f));
    let (x0, y0) = to_display(rect[0], rect[1]);
    let (x1, y1) = to_display(rect[2], rect[3]);
    let (w, h) = geometry.display_size();
    [
        x0.min(x1) / w,
        1.0 - y0.max(y1) / h,
        x0.max(x1) / w,
        1.0 - y0.min(y1) / h,
    ]
}

fn color(span: &TextSpan) -> u32 {
    [span.color.r, span.color.g, span.color.b]
        .into_iter()
        .fold(0, |value, channel| {
            (value << 8) | (channel.clamp(0.0, 1.0) * 255.0).round() as u32
        })
}

fn view(pdf: &PdfDocument, page: usize, revision: u64) -> Result<Value, Reply> {
    let frame =
        assembly::DisplayFrame::new(pdf, page).map_err(|_| refusal(422, "Invalid page number"))?;
    let selection =
        actual_text::inspect(pdf, page, frame.geometry).map_err(|detail| refusal(422, detail))?;
    let visible_text_bounds = run_edit::page_bounds(frame);
    let scales = reflow::RunScales::read(pdf, page, frame.geometry);
    let rendered = visible_runs(&selection, visible_text_bounds)
        .enumerate()
        .map(|(index, (source_index, span))| {
            let bounds = run_edit::selection_bounds(span, &selection.chars);
            let (left, top) = run_edit::visual_point(frame, bounds[0], bounds[3]);
            let (right, bottom) = run_edit::visual_point(frame, bounds[2], bounds[1]);
            let glyph = first_glyph(span, &selection.chars);
            let (x, y) = glyph.map_or((bounds[0], bounds[1]), |glyph| {
                run_edit::origin(span, glyph)
            });
            let origin = run_edit::visual_point(frame, x, y);
            let size = run_edit::font_size(span, glyph, scales.factor(span)) * frame.unit;
            json!({"index":index,"text":selection.text(source_index),"bbox":[left,top,right,bottom],
                "relative_bbox":run_edit::relative_bounds(frame,bounds),"origin":origin,
                "size":size,"font":span.font_name,"color":color(span)})
        })
        .collect::<Vec<_>>();
    Ok(json!({"revision":revision,"runs":rendered}))
}

fn helvetica(bold: bool, italic: bool) -> &'static str {
    match (bold, italic) {
        (true, true) => "Helvetica-BoldOblique",
        (true, false) => "Helvetica-Bold",
        (false, true) => "Helvetica-Oblique",
        _ => "Helvetica",
    }
}

fn choose_font(span: &TextSpan) -> &'static str {
    let font = span.font_name.to_ascii_lowercase();
    let bold = font.contains("bold") || font.contains("black");
    let italic = span.is_italic || font.contains("oblique") || font.contains("italic");
    if font.contains("courier") || span.is_monospace {
        return match (bold, italic) {
            (true, true) => "Courier-BoldOblique",
            (true, false) => "Courier-Bold",
            (false, true) => "Courier-Oblique",
            _ => "Courier",
        };
    }
    if font.contains("times") || font.contains("serif") || font.contains("roman") {
        return match (bold, italic) {
            (true, true) => "Times-BoldItalic",
            (true, false) => "Times-Bold",
            (false, true) => "Times-Italic",
            _ => "Times-Roman",
        };
    }
    helvetica(bold, italic)
}

fn strip_subset(name: &str) -> &str {
    name.split_once('+')
        .filter(|(prefix, _)| prefix.len() == 6 && prefix.bytes().all(|b| b.is_ascii_uppercase()))
        .map_or(name, |(_, font)| font)
}

fn escape_pdf_name(name: &str) -> String {
    let mut escaped = String::new();
    for byte in name.bytes() {
        if (33..=126).contains(&byte) && !b"#()<>[]{}/%".contains(&byte) {
            escaped.push(char::from(byte));
        } else {
            write!(escaped, "#{byte:02X}").unwrap();
        }
    }
    escaped
}

fn winansi(value: &str) -> Option<Vec<u8>> {
    value
        .chars()
        .map(|character| {
            (!character.is_control())
                .then(|| unicode_to_winansi(character as u32))
                .flatten()
        })
        .collect()
}

pub(super) fn standard_font(pdf: &PdfDocument, name: &str) -> pdf_oxide::Result<FontInfo> {
    let dict = Object::Dictionary(
        [
            ("BaseFont", name),
            ("Subtype", "Type1"),
            ("Encoding", "WinAnsiEncoding"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), Object::Name(value.to_owned())))
        .collect(),
    );
    FontInfo::from_dict(&dict, pdf)
}

pub(super) fn standard_text_width(font: &FontInfo, encoded: &[u8], size: f64) -> f64 {
    encoded
        .iter()
        .map(|&code| f64::from(font.get_glyph_width(u16::from(code))))
        .sum::<f64>()
        * size
        / 1000.0
}

#[derive(Clone, Copy)]
pub(super) struct GlyphCode {
    pub code: u32,
    pub hex_width: usize,
    pub cid: u16,
}

type GlyphCodes = HashMap<(u32, usize), GlyphCode>;

pub(super) fn source_font_encoding(
    pdf: &PdfDocument,
    resources: &Object,
    resource: &str,
) -> Option<Object> {
    let fonts = pdf.resolve_object(resources.as_dict()?.get("Font")?).ok()?;
    let font = pdf.resolve_object(fonts.as_dict()?.get(resource)?).ok()?;
    pdf.resolve_object(font.as_dict()?.get("Encoding")?).ok()
}

pub(super) fn composite_codes(
    pdf: &PdfDocument,
    encoding: &Object,
    needed: &str,
) -> Option<Vec<GlyphCode>> {
    let mut codes = GlyphCodes::new();
    collect_codes(pdf, encoding, needed, &mut codes, 0)?;
    (!codes.is_empty()).then(|| codes.into_values().collect())
}

fn collect_codes(
    pdf: &PdfDocument,
    encoding: &Object,
    needed: &str,
    codes: &mut GlyphCodes,
    depth: usize,
) -> Option<()> {
    const MAX_CODES: usize = 131_072;
    if depth >= 12 {
        return None;
    }
    let encoding = pdf.resolve_object(encoding).ok()?;
    if let Some(name) = encoding.as_name() {
        if name == "Identity-H" {
            codes.extend((0..=u16::MAX).map(|cid| {
                (
                    (cid.into(), 4),
                    GlyphCode {
                        code: cid.into(),
                        hex_width: 4,
                        cid,
                    },
                )
            }));
            return Some(());
        }
        let data = hayro_cmap::load_embedded(hayro_cmap::CMapName::from_bytes(name.as_bytes()))?;
        let cmap = hayro_cmap::CMap::parse(data, hayro_cmap::load_embedded)?;
        if cmap.metadata().writing_mode == Some(hayro_cmap::WritingMode::Vertical) {
            return None;
        }
        let mut collect = |code: u32, length: u8| {
            let Some(cid) = cmap
                .lookup_cid_code(code, length)
                .and_then(|value| u16::try_from(value).ok())
            else {
                return;
            };
            if cid != 0 {
                let hex_width = usize::from(length) * 2;
                codes.insert(
                    (code, hex_width),
                    GlyphCode {
                        code,
                        hex_width,
                        cid,
                    },
                );
            }
        };
        for code in 0..=255 {
            collect(code, 1);
        }
        for code in 0..=65535 {
            collect(code, 2);
        }
        if name.contains("UTF16") {
            for character in needed
                .chars()
                .filter(|character| u32::from(*character) > 0xFFFF)
            {
                let mut units = [0; 2];
                character.encode_utf16(&mut units);
                collect((u32::from(units[0]) << 16) | u32::from(units[1]), 4);
            }
        }
        return Some(());
    }
    let data = encoding.decode_stream_data().ok()?;
    if data.len() > 1024 * 1024 {
        return None;
    }
    let source = std::str::from_utf8(&data).ok()?;
    let source = source
        .lines()
        .map(|line| line.split('%').next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    static SECTIONS: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static TOKEN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static BASE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static VERTICAL: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let sections = SECTIONS.get_or_init(|| {
        Regex::new(r"(?s)([0-9]+)\s+begin(cidchar|cidrange)\b(.*?)\bend(cidchar|cidrange)\b")
            .unwrap()
    });
    let token = TOKEN.get_or_init(|| Regex::new(r"<[0-9a-fA-F]{2,8}>|[0-9]+|\S+").unwrap());
    let base = BASE.get_or_init(|| Regex::new(r"/([^\s/]+)\s+usecmap\b").unwrap());
    let vertical = VERTICAL.get_or_init(|| Regex::new(r"/WMode\s+1\s+def\b").unwrap());
    let mut named_bases = base.captures_iter(&source);
    let named_base = named_bases.next().map(|matched| matched[1].to_owned());
    if named_bases.next().is_some()
        || (named_base.is_none() && source.split_whitespace().any(|word| word == "usecmap"))
        || vertical.is_match(&source)
    {
        return None;
    }
    if let Some(base) = encoding.as_dict()?.get("UseCMap") {
        collect_codes(pdf, base, needed, codes, depth + 1)?;
    } else if let Some(name) = named_base {
        collect_codes(pdf, &Object::Name(name), needed, codes, depth + 1)?;
    }
    let mut expanded = 0u32;
    for section in sections.captures_iter(&source) {
        if section[2] != section[4] {
            return None;
        }
        let ranged = &section[2] == "cidrange";
        let mut values = token.find_iter(&section[3]).map(|value| value.as_str());
        let parse_code = |value: &str| {
            let value = value.strip_prefix('<')?.strip_suffix('>')?;
            (value.len() % 2 == 0).then_some((u32::from_str_radix(value, 16).ok()?, value.len()))
        };
        let count: usize = section[1].parse().ok()?;
        if count > u16::MAX as usize {
            return None;
        }
        for _ in 0..count {
            let (start, hex_width) = parse_code(values.next()?)?;
            let end = if ranged {
                let (end, width) = parse_code(values.next()?)?;
                if width != hex_width {
                    return None;
                }
                end
            } else {
                start
            };
            let first_cid: u16 = values.next()?.parse().ok()?;
            let length = end.checked_sub(start)?;
            expanded = expanded.checked_add(length.checked_add(1)?)?;
            if expanded > 65536 {
                return None;
            }
            let last = u16::try_from(length).ok()?.checked_add(first_cid)?;
            for (code, cid) in (start..=end).zip(first_cid..=last) {
                codes.insert(
                    (code, hex_width),
                    GlyphCode {
                        code,
                        hex_width,
                        cid,
                    },
                );
                if codes.len() > MAX_CODES {
                    return None;
                }
            }
        }
        if values.next().is_some() {
            return None;
        }
    }
    Some(())
}

struct EmbeddedText {
    resource: String,
    font: Arc<FontInfo>,
    codes: HashMap<char, Vec<(String, GlyphCode)>>,
}

fn span_font(
    pdf: &PdfDocument,
    resources: &Object,
    span: &TextSpan,
    source: Option<&str>,
) -> Option<(String, Arc<FontInfo>)> {
    let mut extractor = TextExtractor::new();
    pdf.load_fonts_public(resources, &mut extractor).ok()?;
    let fonts = extractor.get_font_set();
    if let Some(name) = source {
        let (_, font) = fonts.iter().find(|(resource, _)| resource == name)?;
        return (font.wmode == 0).then(|| (name.to_owned(), Arc::clone(font)));
    }
    let mut exact = fonts
        .iter()
        .filter(|(resource, font)| resource == &span.font_name || font.base_font == span.font_name)
        .collect::<Vec<_>>();
    if exact.len() > 1 {
        let source_fonts = pdf.resolve_object(resources.as_dict()?.get("Font")?).ok()?;
        exact.retain(|(name, _)| {
            let generated = source_fonts
                .as_dict()
                .and_then(|fonts| fonts.get(name.as_str()))
                .and_then(|font| pdf.resolve_object(font).ok())
                .and_then(|font| pdf.resolve_object(font.as_dict()?.get("ToUnicode")?).ok())
                .and_then(|cmap| {
                    Some(cmap.as_dict()?.get("CMapName")?.as_name()? == "MatterDraftShaped")
                });
            generated != Some(true)
        });
    }
    let (resource, font) = match exact.as_slice() {
        [font] => *font,
        [first, rest @ ..]
            if first.1.embedded_font_data.is_some()
                && rest
                    .iter()
                    .all(|(_, font)| font.embedded_font_data == first.1.embedded_font_data) =>
        {
            *first
        }
        [] => {
            let mut matches = fonts
                .iter()
                .filter(|(_, font)| strip_subset(&font.base_font) == strip_subset(&span.font_name));
            let font = matches.next()?;
            if matches.next().is_some() {
                return None;
            }
            font
        }
        _ => return None,
    };
    (font.wmode == 0).then(|| (resource.clone(), Arc::clone(font)))
}

impl EmbeddedText {
    fn for_span(
        pdf: &PdfDocument,
        page: usize,
        span: &TextSpan,
        needed: &str,
        source: Option<&str>,
    ) -> Option<Self> {
        let resources = pdf.get_page_resources(page).ok()?;
        let (resource, font) = span_font(pdf, &resources, span, source)?;
        let wide = font.subtype == "Type0";
        let map = font.to_unicode.as_ref().and_then(|map| map.get());
        let encoding = wide
            .then(|| source_font_encoding(pdf, &resources, &resource))
            .flatten();
        let uses_unicode_codes = encoding
            .as_ref()
            .and_then(Object::as_name)
            .is_some_and(|name| name.contains("UCS2") || name.contains("UTF16"));
        let source_codes = if wide {
            composite_codes(pdf, encoding.as_ref()?, needed)?
        } else {
            (0..=u8::MAX)
                .map(|byte| GlyphCode {
                    code: byte.into(),
                    hex_width: 2,
                    cid: byte.into(),
                })
                .collect()
        };
        let glyph_map = wide.then(|| font.truetype_cmap()).flatten();
        let mut codes = HashMap::<char, Vec<(String, GlyphCode)>>::new();
        for code in source_codes {
            let text = if let Some(text) = map.as_ref().and_then(|map| map.get(&code.code)) {
                text.into_owned()
            } else if !wide {
                let Some(text) = font.char_to_unicode(code.code) else {
                    continue;
                };
                text
            } else if uses_unicode_codes {
                let units = [(code.code >> 16) as u16, code.code as u16];
                let units = if code.hex_width == 8 {
                    &units[..]
                } else {
                    &units[1..]
                };
                let Ok(text) = String::from_utf16(units) else {
                    continue;
                };
                text
            } else if let Some(glyph_map) = glyph_map {
                let glyph = font
                    .cid_to_gid_map
                    .as_ref()
                    .map_or(code.cid, |map| map.get_gid(code.cid));
                let Some(character) = glyph_map.get_unicode(glyph).filter(|_| glyph != 0) else {
                    continue;
                };
                character.to_string()
            } else {
                use pdf_oxide::fonts::cid_mappings::{
                    lookup_adobe_cns1, lookup_adobe_gb1, lookup_adobe_japan1, lookup_adobe_korea1,
                };
                let Some(system) = font
                    .cid_system_info
                    .as_ref()
                    .filter(|info| info.registry == "Adobe")
                else {
                    continue;
                };
                let unicode = match system.ordering.as_str() {
                    "CNS1" => lookup_adobe_cns1(code.cid),
                    "GB1" => lookup_adobe_gb1(code.cid),
                    "Japan1" => lookup_adobe_japan1(code.cid),
                    "Korea1" => lookup_adobe_korea1(code.cid),
                    _ => None,
                };
                let Some(character) = unicode.and_then(char::from_u32) else {
                    continue;
                };
                character.to_string()
            };
            if let Some(first) = text
                .chars()
                .next()
                .filter(|_| !text.chars().any(|ch| ch.is_control() || ch == '\u{fffd}'))
            {
                codes.entry(first).or_default().push((text, code));
            }
        }
        for terms in codes.values_mut() {
            terms.sort_unstable_by(|a, b| {
                b.0.len()
                    .cmp(&a.0.len())
                    .then_with(|| (a.1.hex_width, a.1.code).cmp(&(b.1.hex_width, b.1.code)))
            });
        }
        (!codes.is_empty()).then_some(Self {
            resource,
            font,
            codes,
        })
    }

    fn encode(&self, mut value: &str) -> Option<(Vec<u8>, f64)> {
        let mut bytes = Vec::new();
        let mut width = 0.0;
        while !value.is_empty() {
            let (text, code) = self
                .codes
                .get(&value.chars().next()?)?
                .iter()
                .find(|(text, _)| value.starts_with(text.as_str()))?;
            value = &value[text.len()..];
            let packed = code.code.to_be_bytes();
            bytes.extend_from_slice(&packed[packed.len() - code.hex_width / 2..]);
            width +=
                f64::from(self.font.get_glyph_width(code.cid)) * f64::from(self.font.font_matrix_a);
        }
        Some((bytes, width))
    }

    fn write(
        &self,
        overlay: &mut assembly::Overlay,
        value: &str,
        size: f64,
        x: f64,
        y: f64,
        ink: (f64, f64, f64),
    ) -> Result<(), Reply> {
        let (encoding, _) = self.encode(value).ok_or_else(|| {
            refusal(
                422,
                "The embedded font cannot represent this replacement text",
            )
        })?;
        let encoded = crate::vault::hex_bytes(&encoding);
        let encoded = encoded.as_str();
        let resource = escape_pdf_name(&self.resource);
        let (r, g, b) = ink;
        writeln!(overlay.commands,"{r:.5} {g:.5} {b:.5} rg BT /{resource} {size:.5} Tf 1 0 0 1 {x:.5} {y:.5} Tm <{encoded}> Tj ET")
            .map_err(|_| refusal(422,INVALID))
    }
}

pub(super) fn annotation_boxes(pdf: &PdfDocument, page: usize) -> Result<Vec<Rect>, Reply> {
    selected_annotation_boxes(pdf, page, |_| true)
}

pub(super) fn selected_annotation_boxes(
    pdf: &PdfDocument,
    page: usize,
    include: impl Fn(Option<&str>) -> bool,
) -> Result<Vec<Rect>, Reply> {
    annotation_collision_boxes(pdf, page, |_, subtype| {
        if subtype == Some("Redact") {
            return Err(refusal(
                422,
                "Review pending PDF redaction marks before editing text",
            ));
        }
        Ok(include(subtype))
    })
}

fn annotation_collision_boxes(
    pdf: &PdfDocument,
    page: usize,
    include: impl Fn(&Object, Option<&str>) -> Result<bool, Reply>,
) -> Result<Vec<Rect>, Reply> {
    let invalid = |_| refusal(422, INVALID);
    let page = pdf.get_page_for_debug(page).map_err(invalid)?;
    let Some(annotations) = page.as_dict().and_then(|page| page.get("Annots")) else {
        return Ok(Vec::new());
    };
    let annotations = pdf.resolve_object(annotations).map_err(invalid)?;
    let mut obstacles = Vec::new();
    for annotation in annotations
        .as_array()
        .ok_or_else(|| refusal(422, INVALID))?
    {
        let annotation = pdf.resolve_object(annotation).map_err(invalid)?;
        let dict = annotation.as_dict().ok_or_else(|| refusal(422, INVALID))?;
        let subtype = dict.get("Subtype").and_then(Object::as_name);
        if !include(&annotation, subtype)? {
            continue;
        }
        let Some(rect) = dict
            .get("Rect")
            .and_then(Object::as_array)
            .filter(|values| values.len() == 4)
        else {
            continue;
        };
        let mut bounds = [0.0; 4];
        for (slot, value) in bounds.iter_mut().zip(rect) {
            *slot = match value {
                Object::Integer(value) => *value as f64,
                Object::Real(value) => *value,
                _ => return Err(refusal(422, INVALID)),
            };
        }
        obstacles.push(bounds);
    }
    Ok(obstacles)
}

#[derive(Clone, Copy)]
struct ParagraphPlacement {
    baseline: [f64; 2],
    axes: [f64; 4],
    size: f64,
}

impl ParagraphPlacement {
    fn from_glyph(style: reflow::ShowStyle, glyph: &TextChar) -> Option<Self> {
        let reflow::ShowStyle {
            axes, size, rise, ..
        } = style;
        let [_, _, c, d] = axes;
        let baseline = [
            f64::from(glyph.origin_x) + c * rise,
            f64::from(glyph.origin_y) + d * rise,
        ];
        (style.supports_paragraph() && baseline.into_iter().all(f64::is_finite)).then_some(Self {
            baseline,
            axes,
            size,
        })
    }

    fn place_bounds(self, rect: Rect, x: f64, y: f64) -> Rect {
        let [a, b, c, d] = self.axes;
        let [x0, y0, x1, y1] = rect;
        union(
            [(x0, y0), (x0, y1), (x1, y0), (x1, y1)]
                .into_iter()
                .map(|(i, j)| {
                    let px = x + a * i + c * j;
                    let py = y + b * i + d * j;
                    [px, py, px, py]
                }),
        )
    }
}

#[derive(Clone)]
struct ParagraphLine<'a, 's> {
    spans: Vec<&'a TextSpan>,
    bounds: Rect,
    ink_bounds: Rect,
    placement: ParagraphPlacement,
    font: Option<&'s reflow::SourceFont>,
    source_orders: Vec<usize>,
}

fn paragraph<'a, 's>(
    selection: &'a actual_text::EditPage,
    seed: &'a TextSpan,
    source: &'s reflow::PageText,
    frame: assembly::DisplayFrame,
    non_opaque: &[Rect],
) -> Result<Vec<ParagraphLine<'a, 's>>, Reply> {
    let chars = &selection.chars;
    let mut rows: HashMap<i64, Vec<&TextChar>> = HashMap::new();
    for glyph in chars {
        rows.entry(glyph.bbox.y.floor() as i64)
            .or_default()
            .push(glyph);
    }
    let mut by_text: HashMap<&str, Vec<(usize, &reflow::TextObject)>> = HashMap::new();
    let mut objects_by_row: HashMap<i64, Vec<usize>> = HashMap::new();
    for (index, object) in source.objects.iter().enumerate() {
        if object.reference_suppressed {
            continue;
        }
        by_text
            .entry(object.reference_text())
            .or_default()
            .push((index, object));
        objects_by_row
            .entry(object.bounds[1].floor() as i64)
            .or_default()
            .push(index);
    }
    let top_level = |span: &TextSpan| {
        !matches!(
            span.mcid_scope,
            Some(McidScope::Form(_) | McidScope::Pattern(_))
        )
    };
    let line_text = |span| reflow::letters(&selection.text_for_span(span)).collect::<String>();
    let locate = |bounds: Rect, text: &str, seed| {
        let mut matches = by_text
            .get(text)
            .into_iter()
            .flatten()
            .copied()
            .filter(|(_, object)| {
                intersects(
                    bounds,
                    if seed {
                        object.paragraph_bounds()
                    } else {
                        object.bounds
                    },
                )
            });
        if let Some(first) = matches.next() {
            if seed && matches.next().is_some() {
                return Err(());
            }
            Ok(Some(vec![first.0]))
        } else {
            // Shaped lines written by MatterDraft place each glyph explicitly.
            // Their generated fonts identify the objects belonging to that line.
            Ok(source.shaped_line(bounds, text))
        }
    };
    let make_line = |spans: Vec<&'a TextSpan>, source_orders: Vec<usize>| {
        let object = &source.objects[*source_orders.first()?];
        let span = *spans.first()?;
        let row = bbox(span)[1].floor() as i64;
        let glyphs = (row.saturating_sub(1)..=row.saturating_add(1))
            .flat_map(|row| rows.get(&row).into_iter().flatten().copied());
        let glyph = first_glyph(span, glyphs).or_else(|| paragraph_glyph(span, chars))?;
        let placement = ParagraphPlacement::from_glyph(object.style, glyph)?;
        let bounds = union(spans.iter().map(|span| bbox(span)));
        let ink_bounds = union(
            source_orders
                .iter()
                .map(|&index| source.objects[index].paragraph_bounds()),
        );
        Some(ParagraphLine {
            spans,
            bounds,
            ink_bounds,
            placement,
            font: source.font(object.style).filter(|font| !font.shaped),
            source_orders,
        })
    };
    let invalid_seed = || refusal(422, "PDFium needs one exact text object for this selection");
    if !top_level(seed) {
        return Err(invalid_seed());
    }
    let seed_bounds = bbox(seed);
    let selection_bounds = run_edit::selection_bounds(seed, chars);
    let (left, top) = run_edit::visual_point(frame, selection_bounds[0], selection_bounds[3]);
    let (right, bottom) = run_edit::visual_point(frame, selection_bounds[2], selection_bounds[1]);
    let page_height = frame.visible.display_size().1;
    // The reference receives the reader's physical top-left box but
    // measures paragraph objects in unscaled PDF coordinates.
    let target = [left, page_height - bottom, right, page_height - top];
    let seed_line = locate(target, &line_text(seed), true)
        .ok()
        .flatten()
        .filter(|orders| {
            orders
                .iter()
                .any(|&index| intersects(seed_bounds, source.objects[index].bounds))
        })
        .and_then(|orders| make_line(vec![seed], orders))
        .ok_or_else(invalid_seed)?;
    let seed_placement = seed_line.placement;
    if non_opaque.iter().any(|&area| intersects(seed_bounds, area)) {
        return Err(refusal(
            422,
            "PDFium trial supports visible opaque fill text only",
        ));
    }
    let size = seed_placement.size;
    let fill = paragraph_fill(seed);
    let same_style = |line: &ParagraphLine<'_, '_>| {
        let candidate = line.placement;
        let same_span_font = || line.spans[0].font_name == seed.font_name;
        let same_font = match (line.font, seed_line.font) {
            (Some(candidate), Some(seed_font)) => {
                candidate.embedded == seed_font.embedded
                    && if candidate.name.is_empty() || seed_font.name.is_empty() {
                        same_span_font()
                    } else {
                        candidate.name == seed_font.name
                    }
            }
            (None, None) => same_span_font(),
            _ => false,
        };
        let same_matrix = candidate
            .axes
            .into_iter()
            .zip(seed_placement.axes)
            .all(|(a, b)| (a - b).abs() <= 0.001);
        same_font
            && same_matrix
            && (candidate.size - size).abs() <= size * 0.15
            && (candidate.baseline[0] - seed_placement.baseline[0]).abs() <= size * 2.5
            && !non_opaque.iter().any(|&area| intersects(line.bounds, area))
    };
    let mut candidates = Vec::new();
    let mut whole_objects = seed_line
        .source_orders
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let mut fragments = HashMap::<usize, Vec<&TextSpan>>::new();
    for span in selection.spans.iter().filter(|span| {
        !std::ptr::eq(*span, seed)
            && top_level(span)
            && editable(span, frame.visible.crop, bbox(span))
            && paragraph_fill(span) == fill
    }) {
        let text = line_text(span);
        if text.is_empty() {
            continue;
        }
        let bounds = bbox(span);
        match locate(bounds, &text, false) {
            Ok(Some(orders)) => {
                if orders.iter().all(|index| !whole_objects.contains(index)) {
                    whole_objects.extend(orders.iter().copied());
                    candidates.extend(make_line(vec![span], orders).filter(&same_style));
                }
            }
            Ok(None) => {
                let row = bounds[1].floor() as i64;
                let mut owners = (row.saturating_sub(1)..=row.saturating_add(1))
                    .flat_map(|row| objects_by_row.get(&row).into_iter().flatten().copied())
                    .filter(|&index| source.objects[index].covers_fragment(bounds, &text));
                if let Some(index) = owners.next().filter(|_| owners.next().is_none()) {
                    fragments.entry(index).or_default().push(span);
                }
            }
            Err(()) => {}
        }
    }
    for (index, mut spans) in fragments {
        if spans.len() < 2 || whole_objects.contains(&index) {
            continue;
        }
        spans.sort_by_key(|span| span.sequence);
        let object = &source.objects[index];
        let mut bounds = spans.iter().map(|span| bbox(span)).collect::<Vec<_>>();
        bounds.sort_by(|left, right| left[0].total_cmp(&right[0]));
        let disjoint = bounds
            .windows(2)
            .all(|pair| pair[0][2] <= pair[1][0] + reflow::POSITION_TOLERANCE);
        let text = spans.iter().map(|span| line_text(span)).collect::<String>();
        if disjoint && text == object.reference_text() && reflow::near(union(bounds), object.bounds)
        {
            candidates.extend(make_line(spans, vec![index]).filter(&same_style));
        }
    }
    candidates.push(seed_line);
    let mut group = vec![candidates.len() - 1];
    for direction in [-1.0, 1.0] {
        let mut current = &candidates[group[0]];
        let mut spacing: Option<f64> = None;
        while group.len() < 100 {
            let area = current.ink_bounds;
            let next = candidates
                .iter()
                .enumerate()
                .filter(|(index, _)| !group.contains(index))
                .filter_map(|(index, line)| {
                    let bounds = line.ink_bounds;
                    let distance =
                        (line.placement.baseline[1] - current.placement.baseline[1]) * direction;
                    let horizontal = area[2].min(bounds[2]) - area[0].max(bounds[0]);
                    let minimum = ((area[2] - area[0]).min(bounds[2] - bounds[0])).max(1.0);
                    ((0.9 * size..=2.2 * size).contains(&distance)
                        && spacing.is_none_or(|spacing| (distance - spacing).abs() <= 0.2 * size)
                        && horizontal / minimum >= 0.3)
                        .then_some((distance, index))
                })
                .min_by(|(a, first), (b, second)| {
                    a.total_cmp(b).then_with(|| {
                        candidates[*first]
                            .source_orders
                            .cmp(&candidates[*second].source_orders)
                    })
                });
            let Some((distance, index)) = next else {
                break;
            };
            spacing = spacing.or(Some(distance));
            group.push(index);
            current = &candidates[index];
        }
    }
    group.sort_by(|&left, &right| {
        candidates[right].placement.baseline[1].total_cmp(&candidates[left].placement.baseline[1])
    });
    Ok(group
        .into_iter()
        .map(|index| candidates[index].clone())
        .collect())
}

fn paragraph_fill(span: &TextSpan) -> [u8; 3] {
    [span.color.r, span.color.g, span.color.b].map(|value| (value * 255.0).round() as u8)
}

fn union(bounds: impl IntoIterator<Item = Rect>) -> Rect {
    bounds
        .into_iter()
        .reduce(|a, b| {
            [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]
        })
        .unwrap_or([0.0; 4])
}

fn paragraph_text(group: &[&TextSpan], page: &actual_text::EditPage) -> String {
    group
        .iter()
        .map(|span| page.text_for_span(span).trim())
        .collect::<Vec<_>>()
        .join(" ")
}

fn line_spacing(group: &[ParagraphLine<'_, '_>]) -> f64 {
    let mut gaps = group
        .windows(2)
        .map(|runs| runs[0].placement.baseline[1] - runs[1].placement.baseline[1])
        .collect::<Vec<_>>();
    if gaps.is_empty() {
        return group[0].placement.size * 1.2;
    }
    gaps.sort_by(f64::total_cmp);
    (gaps[(gaps.len() - 1) / 2] + gaps[gaps.len() / 2]) / 2.0
}

fn measured_spans<'a>(
    source: &reflow::PageText,
    selected: &HashSet<usize>,
    spans: impl Iterator<Item = &'a TextSpan>,
) -> Vec<(&'a TextSpan, Option<Rect>)> {
    let mut rows = HashMap::<i64, Vec<&reflow::TextObject>>::new();
    for (index, object) in source.objects.iter().enumerate() {
        if !selected.contains(&index) && !object.reference_suppressed {
            rows.entry(object.bounds[1].floor() as i64)
                .or_default()
                .push(object);
        }
    }
    spans
        .map(|span| {
            let bounds = bbox(span);
            let ink = if matches!(
                span.mcid_scope,
                Some(McidScope::Form(_) | McidScope::Pattern(_))
            ) {
                None
            } else {
                let row = bounds[1].floor() as i64;
                let mut matches = (row.saturating_sub(1)..=row.saturating_add(1))
                    .flat_map(|key| rows.get(&key).into_iter().flatten().copied())
                    .filter(|object| {
                        reflow::near(object.bounds, bounds)
                            && reflow::letters(&span.text).eq(object.reference_text().chars())
                    });
                matches
                    .next()
                    .filter(|_| matches.next().is_none())
                    .and_then(|object| object.ink_bounds)
            };
            (span, ink)
        })
        .collect()
}

fn following_text<'a>(
    others: &[(&'a TextSpan, Rect)],
    old: Rect,
    new: Rect,
    crop: Rect,
    obstacles: &[Rect],
    has_untracked_objects: bool,
) -> Result<(Vec<&'a TextSpan>, f64, Rect), Reply> {
    let (following, shift) = if others.iter().any(|&(_, bounds)| intersects(bounds, new)) {
        let (following, fixed): (Vec<_>, Vec<_>) = others
            .iter()
            .copied()
            .partition(|(_, bounds)| bounds[3] <= old[1] + 0.5);
        if fixed.iter().any(|&(_, bounds)| intersects(bounds, new)) {
            return Err(refusal(
                422,
                "Replacement would cover text outside this paragraph flow",
            ));
        }
        if following.is_empty() {
            return Err(refusal(422, "No following text can be moved to make room"));
        }
        let shift = old[1] - new[1];
        if shift <= 0.0 {
            return Err(refusal(
                422,
                "Replacement would cover text outside this paragraph flow",
            ));
        }
        if following
            .iter()
            .any(|(_, bounds)| bounds[1] - shift < crop[1] + 2.0)
        {
            return Err(refusal(
                422,
                "Following text cannot fit on this page after reflow",
            ));
        }
        let overlaps = following.iter().any(|&(_, area)| {
            let moved = shifted(area, shift);
            intersects(moved, new) || fixed.iter().any(|&(_, bounds)| intersects(bounds, moved))
        });
        if overlaps {
            return Err(refusal(
                422,
                "Following text cannot be moved without covering other text",
            ));
        }
        if has_untracked_objects
            || following
                .iter()
                .any(|(span, _)| !editable(span, crop, bbox(span)))
        {
            return Err(refusal(
                422,
                "Page has unsupported text objects; following text cannot be reflowed safely",
            ));
        }
        (following, shift)
    } else {
        (Vec::new(), 0.0)
    };
    let region = union(
        [old, new].into_iter().chain(
            following
                .iter()
                .flat_map(|&(_, area)| [area, shifted(area, shift)]),
        ),
    );
    if obstacles.iter().any(|area| intersects(*area, region)) {
        return Err(refusal(
            422,
            "Paragraph overlaps an image, graphic, link or annotation",
        ));
    }
    if (region[2] - region[0]) * (region[3] - region[1])
        > (crop[2] - crop[0]) * (crop[3] - crop[1]) * 0.5
    {
        return Err(refusal(422, "Paragraph trial region is too large"));
    }
    Ok((
        following.into_iter().map(|(span, _)| span).collect(),
        shift,
        region,
    ))
}

fn shifted(area: Rect, delta: f64) -> Rect {
    [area[0], area[1] - delta, area[2], area[3] - delta]
}

fn inspect_paragraph(
    source: &PdfDocument,
    data: &[u8],
    original_bytes: usize,
    page: usize,
    region: Rect,
    old_text: &str,
    lines: &[&str],
    kept_overprint: bool,
) -> Result<(), Reply> {
    let invalid = || refusal(422, "Paragraph edit did not preserve the surrounding PDF");
    if data.len() > (original_bytes * 2 + 8_000_000).min(TRIAL_BYTES * 2) {
        return Err(invalid());
    }
    let edited = PdfDocument::from_bytes(data.to_vec()).map_err(|_| invalid())?;
    let count = source.page_count().map_err(|_| invalid())?;
    if edited.is_encrypted() || edited.page_count().ok() != Some(count) {
        return Err(invalid());
    }
    assembly::verify_edit_context(source, &edited, page).map_err(|_| invalid())?;
    for index in 0..count {
        let before = assembly::DisplayFrame::new(source, index).map_err(|_| invalid())?;
        let after = assembly::DisplayFrame::new(&edited, index).map_err(|_| invalid())?;
        if before.geometry.media != after.geometry.media
            || before.geometry.crop != after.geometry.crop
            || before.geometry.rotation != after.geometry.rotation
            || before.unit != after.unit
        {
            return Err(invalid());
        }
        if index != page
            && source.get_page_content_data(index).map_err(|_| invalid())?
                != edited.get_page_content_data(index).map_err(|_| invalid())?
        {
            return Err(invalid());
        }
    }
    let region = [
        region[0] - 3.0,
        region[1] - 3.0,
        region[2] + 3.0,
        region[3] + 3.0,
    ];
    let before = source.extract_spans(page).map_err(|_| invalid())?;
    let after = edited.extract_spans(page).map_err(|_| invalid())?;
    let unchanged = |spans: &[TextSpan]| {
        spans
            .iter()
            .filter(|span| !intersects(bbox(span), region))
            .map(|span| {
                (
                    span.text.clone(),
                    bbox(span).map(|value| (value * 100.0).round() as i64),
                )
            })
            .collect::<Vec<_>>()
    };
    if unchanged(&before) != unchanged(&after) {
        return Err(refusal(
            422,
            "Paragraph edit changed extracted text outside the edit region",
        ));
    }
    let inside = after
        .iter()
        .filter(|span| intersects(bbox(span), region))
        .flat_map(|span| span.text.chars())
        .filter(|c| !c.is_whitespace() && *c != CONTINUATION)
        .collect::<String>();
    let expected = lines
        .iter()
        .flat_map(|line| line.chars())
        .filter(|c| !c.is_whitespace() && *c != CONTINUATION)
        .collect::<String>();
    let removed = old_text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != CONTINUATION)
        .collect::<String>();
    if !inside.contains(&expected) || (!expected.contains(&removed) && inside.contains(&removed)) {
        return Err(refusal(
            422,
            if kept_overprint {
                "PDFium did not produce the requested searchable paragraph"
            } else {
                "Paragraph edit did not preserve the requested searchable text"
            },
        ));
    }
    let frame = assembly::DisplayFrame::new(source, page).map_err(|_| invalid())?;
    let (page_width, page_height) = frame.size;
    let raster_pixels = page_width.ceil() * page_height.ceil();
    if page_width * page_height > TRIAL_PIXEL_AREA {
        return Err(refusal(
            422,
            "Page is too large for the paragraph trial pixel check",
        ));
    }
    let area = displayed(frame.visible, region);
    let draw = |pdf| {
        super::preview::raster_at_scale(pdf, page, Some(1.0), raster_pixels as u64)
            .map_err(|_| invalid())
    };
    let old = draw(source)?;
    let new = draw(&edited)?;
    if old.dimensions() != new.dimensions() {
        return Err(invalid());
    }
    let [x0, y0, x1, y1] = [
        (area[0] * page_width).floor() - 2.0,
        (area[1] * page_height).floor() - 2.0,
        (area[2] * page_width).ceil() + 2.0,
        (area[3] * page_height).ceil() + 2.0,
    ];
    if old.enumerate_pixels().any(|(x, y, pixel)| {
        let outside =
            f64::from(x) < x0 || f64::from(x) >= x1 || f64::from(y) < y0 || f64::from(y) >= y1;
        outside && pixel != new.get_pixel(x, y)
    }) {
        return Err(refusal(
            422,
            "Paragraph edit changed pixels outside the edit region",
        ));
    }
    Ok(())
}

fn visible_runs(
    selection: &actual_text::EditPage,
    crop: Rect,
) -> impl Iterator<Item = (usize, &TextSpan)> {
    selection
        .spans
        .iter()
        .enumerate()
        .filter(move |&(index, _)| selection.editable(index, crop))
}

fn select_run<'a>(
    selection: &'a actual_text::EditPage,
    body: &Value,
    crop: Rect,
    paragraph_edit: bool,
) -> Result<(usize, &'a TextSpan), Reply> {
    let offset = body["span_index"]
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
        .ok_or_else(|| refusal(422, INVALID))?;
    visible_runs(selection, crop)
        .nth(offset)
        .filter(|(index, _)| body["expected_text"] == selection.text(*index))
        .ok_or_else(|| {
            refusal(
                422,
                if paragraph_edit {
                    "Selected text changed; reload and select it again"
                } else {
                    "Selected text changed; reload the page and select it again"
                },
            )
        })
}

fn enforce_paragraph_size(bytes: usize) -> Result<(), Reply> {
    if bytes > TRIAL_BYTES {
        return Err(refusal(422, "PDF exceeds the 16 MiB PDFium trial limit"));
    }
    Ok(())
}

fn paragraph_preview(
    pdf: &PdfDocument,
    page: usize,
    body: &Value,
    original_bytes: usize,
) -> Result<Value, Reply> {
    let failure = || refusal(422, "Unable to inspect the selected visible text");
    if page >= pdf.page_count().map_err(|_| failure())? {
        return Err(refusal(422, "Invalid page number"));
    }
    let frame = assembly::DisplayFrame::new(pdf, page).map_err(|_| failure())?;
    let geometry = frame.geometry;
    let selection = actual_text::inspect(pdf, page, geometry).map_err(|_| failure())?;
    let (_, seed) = select_run(&selection, body, run_edit::page_bounds(frame), true)?;
    enforce_paragraph_size(original_bytes)?;
    if geometry.rotation != 0 {
        return Err(refusal(
            422,
            "Rotated pages are unsupported by the PDFium paragraph trial",
        ));
    }
    let non_opaque = non_opaque_text(pdf, page, geometry).map_err(|detail| refusal(422, detail))?;
    let source = reflow::PageText::inspect(pdf, page).ok_or_else(failure)?;
    let lines = paragraph(&selection, seed, &source, frame, &non_opaque)?;
    let font_name = lines[0]
        .font
        .filter(|font| !font.name.is_empty())
        .map_or(lines[0].spans[0].font_name.as_str(), |font| {
            font.name.as_str()
        });
    let group = lines
        .iter()
        .flat_map(|line| line.spans.iter().copied())
        .collect::<Vec<_>>();
    if group.iter().any(|span| selection.blocked_span(span)) {
        return Err(failure());
    }
    let bounds = union(lines.iter().map(|line| line.ink_bounds));
    let top = frame.visible.display_size().1;
    let text = paragraph_text(&group, &selection);
    if text.chars().count() > TRIAL_PARAGRAPH_CHARS {
        return Err(refusal(
            422,
            "Paragraph exceeds the 2,000-character trial limit",
        ));
    }
    Ok(
        json!({"text":text,"line_count":lines.len(),"font_name":font_name,
        "bbox":[bounds[0],top-bounds[3],bounds[2],top-bounds[1]],"engine":TRIAL_ENGINE}),
    )
}

fn edit(
    bytes: Vec<u8>,
    page: usize,
    body: &Value,
    move_text: bool,
    paragraph_edit: bool,
) -> Result<(Vec<u8>, Value), Reply> {
    let original_bytes = bytes.len();
    let failed = || refusal(422, INVALID);
    let move_offsets = move_text
        .then(|| run_edit::move_offsets(body))
        .transpose()?;
    if !paragraph_edit
        && !move_text
        && body["replacement"].as_str().is_none_or(|replacement| {
            replacement.trim().is_empty()
                || replacement
                    .bytes()
                    .any(|byte| byte != b'\n' && !(32..=126).contains(&byte))
        })
    {
        return Err(refusal(
            422,
            "Replacement supports printable ASCII and line breaks only",
        ));
    }
    let pdf = PdfDocument::from_bytes(bytes.clone()).map_err(|_| failed())?;
    if page >= pdf.page_count().map_err(|_| failed())? {
        return Err(refusal(422, "Invalid page number"));
    }
    let (pdf, bytes) = match font_state::page(&pdf, page).map_err(|detail| refusal(422, detail))? {
        Some((content, forms)) => {
            let bytes = assembly::rebind_text_page(pdf, page, content, forms)
                .map_err(|detail| refusal(422, detail))?;
            (
                PdfDocument::from_bytes(bytes.clone()).map_err(|_| failed())?,
                bytes,
            )
        }
        None => (pdf, bytes),
    };
    let frame = assembly::DisplayFrame::new(&pdf, page).map_err(|_| failed())?;
    let geometry = frame.geometry;
    let selection_crop = run_edit::page_bounds(frame);
    let selection =
        actual_text::inspect(&pdf, page, geometry).map_err(|detail| refusal(422, detail))?;
    let spans = &selection.spans;
    let chars = &selection.chars;
    let (span_index, span) = select_run(&selection, body, selection_crop, paragraph_edit)?;
    if paragraph_edit {
        enforce_paragraph_size(original_bytes)?;
    }
    if move_text
        && selection
            .text(span_index)
            .bytes()
            .any(|byte| !(32..=126).contains(&byte))
    {
        return Err(refusal(
            422,
            "Moving text currently supports single-line printable ASCII only",
        ));
    }
    if paragraph_edit && geometry.rotation != 0 {
        return Err(refusal(
            422,
            "Rotated pages are unsupported by the PDFium paragraph trial",
        ));
    }
    let non_opaque = if paragraph_edit {
        non_opaque_text(&pdf, page, geometry).map_err(|detail| refusal(422, detail))?
    } else {
        Vec::new()
    };
    let paragraph_source = paragraph_edit
        .then(|| reflow::PageText::inspect(&pdf, page).ok_or_else(failed))
        .transpose()?;
    let mut paragraph_objects = HashSet::new();
    let (
        group,
        paragraph_placement,
        paragraph_spacing,
        font_resource,
        paragraph_line_count,
        first_font_embedded,
    ) = if let Some(source) = &paragraph_source {
        let lines = paragraph(&selection, span, source, frame, &non_opaque)?;
        paragraph_objects.extend(
            lines
                .iter()
                .flat_map(|line| line.source_orders.iter().copied()),
        );
        let spacing = line_spacing(&lines);
        let placement = lines[0].placement;
        let font = lines[0].font.and_then(|font| font.resource.clone());
        let embedded = lines[0].font.is_some_and(|font| font.embedded);
        let line_count = lines.len();
        (
            lines.into_iter().flat_map(|line| line.spans).collect(),
            Some(placement),
            spacing,
            font,
            line_count,
            embedded,
        )
    } else {
        (
            vec![span],
            None,
            f64::from(span.font_size) * 1.2,
            None,
            0,
            false,
        )
    };
    let has_untracked_objects = paragraph_source
        .as_ref()
        .is_some_and(|source| source.has_untracked_objects);
    if group.iter().any(|span| selection.blocked_span(span)) {
        return Err(refusal(
            422,
            "Some replacement text spans cannot be isolated",
        ));
    }
    let original_paragraph = if paragraph_edit {
        let current = paragraph_text(&group, &selection);
        if current.chars().count() > TRIAL_PARAGRAPH_CHARS {
            return Err(refusal(
                422,
                "Paragraph exceeds the 2,000-character trial limit",
            ));
        }
        if body["expected_paragraph"] != current {
            return Err(refusal(
                422,
                "Paragraph changed; reload and select it again",
            ));
        }
        if body["replacement"].as_str().is_some_and(|text| {
            text.chars()
                .filter(|c| !c.is_whitespace() && *c != CONTINUATION)
                .eq(current
                    .chars()
                    .filter(|c| !c.is_whitespace() && *c != CONTINUATION))
        }) {
            return Err(refusal(
                422,
                "Replacement matches the current paragraph; no revision was published",
            ));
        }
        Some(current)
    } else {
        None
    };
    let span = group[0];
    let selected = union(group.iter().map(|span| bbox(span)));
    let display_bounds = |span| {
        if paragraph_edit {
            bbox(span)
        } else {
            run_edit::selection_bounds(span, chars)
        }
    };
    // Glyph removal uses source token boxes. Placement and obstacles also
    // account for the visible displacement caused by text rise.
    let selected_display = display_bounds(span);
    let paragraph_bounds = paragraph_source.as_ref().map_or(selected, |source| {
        union(
            paragraph_objects
                .iter()
                .map(|&index| source.objects[index].paragraph_bounds()),
        )
    });
    let fragment_area = (group.len() == 1)
        .then(|| selection.fragment_removal_area(span_index))
        .flatten();
    let fragment = fragment_area.is_some();
    // Individual glyph extents can touch at the same cursor position. Allow
    // for rounding in those horizontal edges when checking a partial run.
    let overlap_area = if fragment {
        [
            selected[0] + 0.001,
            selected[1],
            selected[2] - 0.001,
            selected[3],
        ]
    } else {
        [
            selected[0] - 0.25,
            selected[1] - 0.25,
            selected[2] + 0.25,
            selected[3] + 0.25,
        ]
    };
    let other_text = || {
        spans
            .iter()
            .filter(|other| !group.iter().any(|span| std::ptr::eq(*other, *span)))
    };
    let mut other_spans = paragraph_source.as_ref().map_or_else(Vec::new, |source| {
        measured_spans(source, &paragraph_objects, other_text())
    });
    let others = spans
        .iter()
        .enumerate()
        .filter(|(index, other)| {
            !group.iter().any(|span| std::ptr::eq(*other, *span))
                && (paragraph_edit || !other.text.trim().is_empty())
                && (paragraph_edit || !selection.hidden_in_selected_mark(span_index, *index))
        })
        .map(|(_, span)| display_bounds(span))
        .collect::<Vec<_>>();
    if !move_text
        && !paragraph_edit
        && spans.iter().enumerate().any(|(index, other)| {
            index != span_index
                && selection.editable(index, selection_crop)
                && intersects(selected_display, display_bounds(other))
        })
    {
        return Err(refusal(
            422,
            "Selected text overlaps another run; cannot replace it safely",
        ));
    }
    let run_annotations = if paragraph_edit {
        run_edit::RunAnnotations::default()
    } else {
        run_edit::annotations(&pdf, page, frame, selected_display, move_text)?
    };
    let links = &run_annotations.links;
    let mut annotations = if paragraph_edit {
        annotation_collision_boxes(&pdf, page, |entry, subtype| match subtype {
            Some("Link") => {
                pdf_annotations::has_link(&pdf, entry).map_err(|_| refusal(422, INVALID))
            }
            Some("Popup") => Ok(false),
            _ => Ok(true),
        })?
    } else {
        Vec::new()
    };
    let raw_overlaps = || {
        other_spans
            .iter()
            .filter(|(run, _)| intersects(overlap_area, bbox(run)))
    };
    let overlaps_text = raw_overlaps().next().is_some();
    let overlaps_object = |reference_suppressed| {
        paragraph_source.as_ref().is_some_and(|source| {
            source.objects.iter().enumerate().any(|(index, object)| {
                object.reference_suppressed == reference_suppressed
                    && !paragraph_objects.contains(&index)
                    && if reference_suppressed {
                        intersects(selected, object.bounds)
                    } else {
                        intersects(paragraph_bounds, object.paragraph_bounds())
                    }
            })
        })
    };
    let overlaps_suppressed = overlaps_object(true);
    let exact_isolation = overlaps_text
        && paragraph_source.as_ref().is_some_and(|source| {
            paragraph_objects
                .iter()
                .all(|&index| source.objects[index].ink_bounds.is_some())
                && raw_overlaps()
                    .all(|(_, ink)| ink.is_some_and(|ink| !intersects(ink, paragraph_bounds)))
        });
    let use_authored_shows = overlaps_suppressed || exact_isolation;
    if paragraph_edit && ((overlaps_text && !use_authored_shows) || overlaps_object(false)) {
        return Err(refusal(422, "Paragraph overlaps another text object"));
    }
    if !paragraph_edit
        && others
            .iter()
            .any(|&area| intersects(selected_display, area))
    {
        return Err(refusal(
            422,
            if move_text {
                "Selected text overlaps other text; cannot move it safely"
            } else {
                "Selected text overlaps other text, including a non-editable layer; cannot replace it safely"
            },
        ));
    }
    if !move_text && !paragraph_edit && links.iter().any(|&area| intersects(selected_display, area))
    {
        return Err(refusal(
            422,
            "Selected text contains a link; edit it outside this tool",
        ));
    }
    let retained_overprints = paragraph_source
        .as_ref()
        .filter(|_| overlaps_suppressed)
        .map(|source| {
            source
                .objects
                .iter()
                .filter(|object| object.reference_suppressed && intersects(selected, object.bounds))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !retained_overprints.is_empty() {
        other_spans.retain(|(span, _)| {
            !retained_overprints.iter().any(|object| {
                reflow::near(object.bounds, bbox(span))
                    && reflow::letters(&span.text).eq(object.text.chars())
            })
        });
    }
    let bounds = |rect: pdf_oxide::geometry::Rect| {
        [
            f64::from(rect.x),
            f64::from(rect.y),
            f64::from(rect.x + rect.width),
            f64::from(rect.y + rect.height),
        ]
    };
    let images = pdf.page_image_handles(page).map_err(|_| failed())?;
    annotations.extend(images.iter().map(|image| bounds(image.bbox)));
    if move_text || paragraph_edit {
        let paths = pdf.extract_paths(page).map_err(|_| failed())?;
        annotations.extend(paths.iter().map(|path| bounds(path.bbox)));
    }
    let glyph = first_glyph(span, chars);
    let font_name = glyph
        .filter(|_| matches!(span.mcid_scope, Some(McidScope::Form(_))))
        .map_or(span.font_name.as_str(), |glyph| glyph.font_name.as_str())
        .to_ascii_lowercase();
    let font = if paragraph_edit {
        choose_font(span)
    } else {
        helvetica(
            font_name.contains("bold"),
            font_name.contains("italic") || font_name.contains("oblique"),
        )
    };
    let metrics = standard_font(&pdf, font).map_err(|_| failed())?;
    let normalized_replacement = (!move_text).then(|| {
        body["replacement"]
            .as_str()
            .unwrap_or_default()
            .split('\n')
            .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("\n")
    });
    let normalized = normalized_replacement.as_deref().filter(|_| paragraph_edit);
    let standard_substitute = first_font_embedded
        && normalized
            .zip(original_paragraph.as_deref())
            .is_some_and(|(replacement, current)| {
                replacement.split('\n').all(|line| winansi(line).is_some())
                    && replacement
                        .chars()
                        .any(|character| !character.is_whitespace() && !current.contains(character))
            });
    // The reference substitutes a standard face for newly introduced Latin
    // glyphs. Non-Latin text can still use the source encoding and shaping.
    let source_replacement = normalized.filter(|_| !standard_substitute);
    let embedded = source_replacement.and_then(|replacement| {
        EmbeddedText::for_span(&pdf, page, span, replacement, font_resource.as_deref()).filter(
            |font| {
                replacement
                    .split('\n')
                    .all(|line| font.encode(line).is_some())
            },
        )
    });
    let shaped = source_replacement.and_then(|text| {
        let resources = pdf.get_page_resources(page).ok()?;
        let (_, font) = span_font(&pdf, &resources, span, font_resource.as_deref())?;
        let shaper = Shaper::new(&font)?;
        let mut lines = text.split('\n');
        let valid = lines.clone().all(|line| shaper.width(line).is_some());
        (valid && (embedded.is_none() || lines.any(|line| shaper.needs_shaping(line))))
            .then_some((font, shaper))
    });
    if embedded.is_none()
        && shaped.is_none()
        && normalized.is_some_and(|text| text.split('\n').any(|line| winansi(line).is_none()))
    {
        return Err(refusal(
            422,
            "The available font cannot encode this replacement text",
        ));
    }
    let embedded_outline = embedded.as_ref().and_then(|font| {
        let resources = pdf.get_page_resources(page).ok()?;
        EmbeddedParagraphFont::new(&pdf, &resources, &font.resource)
    });
    let embedded_bounds = |code, size| embedded_outline.as_ref()?.bounds(code, size);
    let glyph_width = |text: &str, size: f64| {
        if let Some((_, shaped)) = &shaped {
            shaped
                .width(text)
                .map_or(f64::INFINITY, |width| width * size)
        } else if let Some(font) = &embedded {
            font.encode(text).map_or(f64::INFINITY, |(code, width)| {
                embedded_bounds(code, size).map_or(width * size, |[left, _, right, _]| right - left)
            })
        } else {
            winansi(text).map_or(f64::INFINITY, |text| {
                if paragraph_edit {
                    if let Some([left, _, right, _]) = standard_ink::bounds(font, &text, size) {
                        return right - left;
                    }
                }
                standard_text_width(&metrics, &text, size)
            })
        }
    };
    let horizontal_scale = paragraph_placement.map_or(1.0, |placement| placement.axes[0]);
    let measure = |text: &str, size: f64| glyph_width(text, size) * horizontal_scale;
    let source_size = paragraph_placement.map_or_else(
        || {
            let scale = reflow::RunScales::read(&pdf, page, geometry).factor(span);
            run_edit::font_size(span, glyph, scale)
        },
        |placement| placement.size,
    );
    let ink = if paragraph_edit {
        paragraph_fill(span)
    } else {
        let srgb = color(span);
        [(srgb >> 16) as u8, (srgb >> 8) as u8, srgb as u8]
    };
    let [r, g, b] = ink.map(|component| f64::from(component) / 255.0);
    let style = (r, g, b);
    let [baseline_x, baseline_y] = paragraph_placement
        .map(|placement| placement.baseline)
        .or_else(|| {
            glyph.map(|glyph| {
                let (x, y) = run_edit::origin(span, glyph);
                [x, y]
            })
        })
        .ok_or_else(|| refusal(422, "Cannot isolate the selected text baseline"))?;
    let source_x = baseline_x - geometry.crop[0];
    let source_y = baseline_y - geometry.crop[1];
    let spacing = paragraph_spacing;
    let (x, y, size, lines) = if let Some((horizontal, vertical)) = move_offsets {
        let [a, b, c, d, _, _] = frame.paint_matrix();
        let (x, y) = (horizontal * frame.size.0, -vertical * frame.size.1);
        let (dx, dy) = (a * x + c * y, b * x + d * y);
        let [left, bottom, right, top] = selected_display;
        let target = [left + dx, bottom + dy, right + dx, top + dy];
        if !contains(selection_crop, target) {
            return Err(refusal(
                422,
                "The text would leave the page; drag it inside the page",
            ));
        }
        if links
            .iter()
            .any(|&area| intersects(selected_display, area) || intersects(target, area))
        {
            return Err(refusal(
                422,
                "Text inside or over a link cannot be moved with this tool",
            ));
        }
        if others
            .iter()
            .chain(&annotations)
            .any(|&area| intersects(target, area))
        {
            return Err(refusal(
                422,
                "The new position overlaps text or page graphics; choose a clear gap",
            ));
        }
        (
            source_x + dx,
            source_y + dy,
            source_size,
            vec![selection.text(span_index)],
        )
    } else if paragraph_edit {
        let width = frame.visible.display_size().0 - baseline_x - 18.0;
        let available = if paragraph_line_count > 1 {
            width.min(paragraph_bounds[2] - baseline_x + source_size / 2.0)
        } else {
            width
        };
        if available <= 0.0 {
            return Err(refusal(422, "No room to reflow this paragraph"));
        }
        let replacement = normalized_replacement.as_deref().ok_or_else(failed)?;
        let lines = text_lines(replacement, available, |line| measure(line, source_size))
            .ok_or_else(|| refusal(422, "A replacement word does not fit the paragraph width"))?;
        if lines.len() > 100 {
            return Err(refusal(422, "Replacement has too many paragraph lines"));
        }
        (source_x, source_y, source_size, lines)
    } else {
        let replacement = body["replacement"]
            .as_str()
            .filter(|text| text.chars().count() <= 2000)
            .ok_or_else(failed)?;
        let replacement = normalized_replacement.as_deref().unwrap_or(replacement);
        let mut layout = None;
        let [_, bottom, right, top] = selected_display;
        let point = 1.0 / frame.unit;
        let limit = others
            .iter()
            .chain(&annotations)
            .filter(|area| {
                area[1] < top + point && area[3] > bottom - point && area[0] >= right - point
            })
            .map(|area| area[0] - 2.0 * point)
            .fold(selection_crop[2] - 12.0 * point, f64::min);
        let available = limit - baseline_x;
        if available <= 0.0 {
            return Err(refusal(
                422,
                "No clear space to the right of the selected text",
            ));
        }
        let minimum = (source_size * 0.75).max(6.0 / frame.unit).min(source_size);
        for step in 0..=10 {
            let size = source_size - (source_size - minimum) * f64::from(step) / 10.0;
            let Some(lines) = text_lines(replacement, available, |line| measure(line, size)) else {
                continue;
            };
            let fits_layout = fits(
                &lines,
                baseline_x,
                baseline_y,
                size,
                &measure,
                selection_crop,
                &others,
                &annotations,
            );
            if fits_layout {
                layout = Some((source_x, source_y, size, lines));
                break;
            }
        }
        layout.ok_or_else(|| refusal(422,"Text cannot fit in clear space on this page. Shorten it or use Add text elsewhere."))?
    };
    if lines.iter().all(|line| line.trim().is_empty()) {
        return Err(refusal(422, "Replacement text is empty"));
    }
    if embedded.is_none()
        && shaped.is_none()
        && lines.iter().any(|line| {
            if paragraph_edit {
                winansi(line).is_none()
            } else {
                line.bytes().any(|byte| !(32..=126).contains(&byte))
            }
        })
    {
        return Err(refusal(
            422,
            if move_text {
                "Moving text currently supports single-line printable ASCII only"
            } else {
                "The available font cannot encode this replacement text"
            },
        ));
    }
    let (target_x, target_y) = (x + geometry.crop[0], y + geometry.crop[1]);
    let (write_x, write_y, write_size) = if paragraph_edit {
        (target_x, target_y, size)
    } else {
        let (left, baseline) = run_edit::redraw_baseline(frame, target_x, target_y);
        let drawing_size = size * frame.unit;
        let unchanged_frame = frame.unit == 1.0 && frame.visible.crop == geometry.crop;
        if unchanged_frame
            || fits(
                &lines,
                left,
                baseline,
                drawing_size,
                &measure,
                frame.visible.crop,
                &[],
                &[],
            )
        {
            (left, baseline, drawing_size)
        } else if fits(
            &lines,
            target_x,
            target_y,
            size,
            &measure,
            frame.visible.crop,
            &[],
            &[],
        ) {
            // Some documents' physical text coordinates put the reference
            // redraw outside the sheet. Keep the validated run on the page.
            (target_x, target_y, size)
        } else {
            return Err(refusal(
                422,
                if move_text {
                    "The text would leave the page; drag it inside the page"
                } else {
                    "Text cannot fit in clear space on this page. Shorten it or use Add text elsewhere."
                },
            ));
        }
    };
    let (following, shift, region) = if let Some(placement) = paragraph_placement {
        let [left, bottom, right, top] = frame.visible.crop;
        let source_font = shaped
            .as_ref()
            .map(|(font, _)| font)
            .or_else(|| embedded.as_ref().map(|font| &font.font));
        let (ascent, descent) = source_font.map_or((0.9, -0.3), |font| {
            (f64::from(font.ascent), f64::from(font.descent))
        });
        let replacement = union(
            lines
                .iter()
                .enumerate()
                .filter(|(_, line)| !line.is_empty())
                .map(|(index, line)| {
                    let baseline = target_y - spacing * index as f64;
                    let bounds = shaped
                        .as_ref()
                        .and_then(|(_, shaper)| shaper.bounds(line))
                        .map(|bounds| bounds.map(|value| value * size))
                        .or_else(|| match &embedded {
                            Some(font) => embedded_bounds(font.encode(line)?.0, size),
                            None => standard_ink::bounds(font, &winansi(line)?, size),
                        })
                        .unwrap_or_else(|| {
                            [0.0, size * descent, glyph_width(line, size), size * ascent]
                        });
                    placement.place_bounds(bounds, target_x, baseline)
                }),
        );
        if !contains(
            [left + 2.0, bottom + 2.0, right - 2.0, top - 2.0],
            replacement,
        ) {
            return Err(refusal(422, "Replacement paragraph would leave the page"));
        }
        let layout = other_spans
            .iter()
            .map(|&(span, ink)| (span, ink.unwrap_or_else(|| bbox(span))))
            .collect::<Vec<_>>();
        following_text(
            &layout,
            paragraph_bounds,
            replacement,
            frame.visible.crop,
            &annotations,
            has_untracked_objects,
        )?
    } else {
        (Vec::new(), 0.0, selected)
    };
    let page_content = pdf.get_page_content_data(page).map_err(|_| failed())?;
    let translation = if following.is_empty() {
        None
    } else {
        reflow::translate(&pdf, page, &page_content, &following, shift)
    };
    let (translated, following_objects_moved) = translation
        .map(|(content, objects)| (Some(content), objects))
        .unwrap_or((None, following.len()));
    let native_reflow = translated.is_some();
    let redrawn = if native_reflow {
        &[][..]
    } else {
        following.as_slice()
    };
    if use_authored_shows && !redrawn.is_empty() {
        return Err(refusal(
            422,
            "Following text cannot be moved with its original paint",
        ));
    }
    if paragraph_source.as_ref().is_some_and(|source| {
        source.objects.iter().any(|object| {
            object.recovered
                && redrawn
                    .iter()
                    .any(|span| intersects(bbox(span), object.bounds))
        })
    }) {
        return Err(refusal(
            422,
            "Following text cannot be moved with its original paint",
        ));
    }
    let resources = pdf.get_page_resources(page).map_err(|_| failed())?;
    if redrawn.iter().any(|span| {
        selection.blocked_span(span)
            || selection.requires_individual_edit(span)
            || selection.text_for_span(span) != span.text
    }) {
        return Err(refusal(
            422,
            "Following replacement text cannot be moved as part of this paragraph",
        ));
    }
    if redrawn
        .iter()
        .any(|run| non_opaque.iter().any(|&area| intersects(bbox(run), area)))
    {
        return Err(refusal(
            422,
            "Following text cannot be moved with its original paint",
        ));
    }
    let following_fonts = redrawn
        .iter()
        .map(|run| {
            let embedded = EmbeddedText::for_span(&pdf, page, run, &run.text, None)
                .filter(|font| font.encode(&run.text).is_some());
            let shaped = span_font(&pdf, &resources, run, None)
                .and_then(|(_, font)| Shaper::new(&font))
                .filter(|font| {
                    font.width(&run.text).is_some()
                        && (embedded.is_none() || font.needs_shaping(&run.text))
                });
            if shaped.is_some()
                || embedded.is_some()
                || (run.font_name == choose_font(run) && winansi(&run.text).is_some())
            {
                Ok((embedded, shaped))
            } else {
                Err(refusal(
                    422,
                    "Following text cannot be moved with its original font",
                ))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let removed_regions = std::iter::once(if fragment { overlap_area } else { selected })
        .chain(redrawn.iter().map(|span| bbox(span)))
        .collect::<Vec<_>>();
    let form_scope = |run: &TextSpan| match run.mcid_scope {
        Some(McidScope::Form(id)) => Ok(Some(id)),
        Some(McidScope::Pattern(_)) => Err(refusal(
            422,
            "Cannot safely edit text painted by a tiling pattern",
        )),
        _ => Ok(None),
    };
    let mut scoped = super::redaction_text::EditRegions::new();
    let mut selected_scopes = HashMap::<_, Vec<_>>::new();
    for run in &group {
        selected_scopes
            .entry(form_scope(run)?)
            .or_default()
            .push(fragment_area.unwrap_or_else(|| bbox(run)));
    }
    for (scope, bounds) in selected_scopes {
        scoped.insert(
            scope,
            vec![union(bounds.into_iter()).map(|value| value as f32)],
        );
    }
    for run in redrawn {
        scoped
            .entry(form_scope(run)?)
            .or_default()
            .push(bbox(run).map(|value| value as f32));
    }
    let remove_actual_text = selection
        .alternates
        .get(&span_index)
        .is_some_and(|item| item.structured);
    let shared_actual_text = selection.shared_actual_text_edit(span_index, &lines);
    let retained_anchor = selection
        .alternates
        .get(&span_index)
        .and_then(|item| item.retained_anchor);
    if group
        .iter()
        .any(|span| selection.requires_individual_edit(span))
        && (group.len() != 1 || !following.is_empty())
    {
        return Err(refusal(
            422,
            "Edit one marked line at a time to preserve its alternate text",
        ));
    }
    let track_selected_tag = !paragraph_edit
        || (group.len() == 1
            && following.is_empty()
            && (remove_actual_text || retained_anchor.is_some()));
    let mut selected_tag = track_selected_tag
        .then(|| replacement_tag(&pdf, page, span))
        .flatten()
        .map(|mut tag| {
            tag.remove_actual_text = remove_actual_text;
            tag.retained_anchor = retained_anchor;
            tag
        });
    let order_split = selected_tag.as_mut().and_then(|tag| {
        let split = selection.reading_order_split(&pdf, page, span_index)?;
        tag.retained_anchor = Some(assembly::RetainedAnchor::InsertBetween {
            next_source_mcid: split.next_mcid,
        });
        Some(split)
    });
    let enclosing_page_key = (track_selected_tag
        && selected_tag.is_none()
        && matches!(span.mcid_scope, Some(McidScope::Form(_))))
    .then(|| page_parent_key(&pdf, page))
    .flatten();
    let painted_text = (!selection.alternates.is_empty()).then(|| {
        selection
            .spans
            .iter()
            .map(|span| span.text.as_str())
            .collect::<String>()
    });
    let inline = if order_split
        .as_ref()
        .is_some_and(|split| split.rewrites_alternate())
    {
        Vec::new()
    } else {
        selection
            .inline_edits(&group)
            .map_err(|detail| refusal(422, detail))?
    };
    let authored_shows = paragraph_source
        .as_ref()
        .filter(|_| use_authored_shows)
        .map(|source| {
            paragraph_objects
                .iter()
                .map(|&index| source.objects[index].show)
                .collect::<HashSet<_>>()
        });
    let edit = super::redaction_text::edit_stream(
        &pdf,
        page,
        translated.unwrap_or(page_content),
        &scoped,
        enclosing_page_key.is_some(),
        painted_text.as_deref(),
        &inline,
        order_split.as_ref(),
        authored_shows.as_ref(),
    )
    .map_err(|detail| refusal(422, detail))?;
    if edit.glyphs == 0
        || (authored_shows.is_none() && fragment && edit.glyphs != span.text.chars().count())
    {
        return Err(refusal(422, "Selected glyphs could not be isolated safely"));
    }
    let selected_tag = selected_tag.or_else(|| {
        let (parent_key, mcid) = enclosing_page_key.zip(edit.enclosing_mcid)?;
        Some(assembly::ReplacementTag {
            parent_key,
            mcid: Some(mcid),
            from_page: true,
            remove_actual_text,
            retained_anchor,
        })
    });
    let requires_tag = shared_actual_text.is_some()
        || retained_anchor.is_some() && (remove_actual_text || selected_tag.is_some());
    let (removed, replacement_tag) =
        assembly::remove_text_for_replacement(bytes, page, edit.overlay, selected_tag)
            .map_err(|_| failed())?;
    if requires_tag && replacement_tag.is_none() {
        return Err(refusal(
            422,
            "Cannot keep the edited text in its structure element",
        ));
    }
    let check = PdfDocument::from_bytes(removed.clone()).map_err(|_| failed())?;
    let extracted = actual_text::inspect(&check, page, geometry)
        .map_err(|_| failed())?
        .spans;
    let remains = |bounds| removed_regions.iter().any(|&area| intersects(bounds, area));
    let isolates_text = if let Some(source) = paragraph_source
        .as_ref()
        .filter(|_| authored_shows.is_some())
    {
        let after = reflow::PageText::inspect(&check, page).ok_or_else(failed)?;
        source.objects.len() == after.objects.len() + paragraph_objects.len()
            && source
                .objects
                .iter()
                .enumerate()
                .filter(|(index, _)| !paragraph_objects.contains(index))
                .map(|(_, object)| object)
                .zip(&after.objects)
                .all(|(before, after)| {
                    before.text == after.text
                        && (reflow::near(before.bounds, after.bounds)
                            || (native_reflow
                                && following
                                    .iter()
                                    .any(|span| intersects(before.bounds, bbox(span)))
                                && reflow::near(shifted(before.bounds, shift), after.bounds)))
                })
    } else {
        !extracted
            .iter()
            .any(|item| !item.text.trim().is_empty() && remains(bbox(item)))
    };
    if !isolates_text {
        return Err(refusal(422, "Selected glyphs could not be isolated safely"));
    }
    if native_reflow && !reflow::moved(&following, &extracted, shift) {
        return Err(refusal(
            422,
            "Following text could not be moved without changing its placement",
        ));
    }
    let mut overlay = assembly::Overlay {
        user_space_text: true,
        omit_annotations: run_annotations.removed,
        // The original fill removed here preceded the retained paint pass.
        paint_before_source: overlaps_suppressed,
        replacement_tag,
        shared_actual_text,
        ..Default::default()
    };
    let spacing = if paragraph_edit {
        spacing
    } else {
        write_size * 1.2
    };
    let axes = paragraph_placement
        .map(|placement| placement.axes)
        .filter(|axes| *axes != [1.0, 0.0, 0.0, 1.0]);
    for (line, text) in lines.iter().enumerate() {
        if !text.is_empty() {
            let left = write_x;
            let baseline = write_y - line as f64 * spacing;
            let (left, baseline) = if let Some([a, b, c, d]) = axes {
                writeln!(
                    overlay.commands,
                    "q {a:.9} {b:.9} {c:.9} {d:.9} {left:.9} {baseline:.9} cm"
                )
                .map_err(|_| failed())?;
                (0.0, 0.0)
            } else {
                (left, baseline)
            };
            if let Some((_, shaped)) = &shaped {
                shaped
                    .write(&mut overlay, text, write_size, left, baseline, style)
                    .map_err(|detail| refusal(422, detail))?;
            } else if let Some(embedded) = &embedded {
                embedded.write(&mut overlay, text, write_size, left, baseline, style)?;
            } else {
                append_text(
                    &mut overlay,
                    font,
                    write_size,
                    left,
                    baseline,
                    style,
                    &winansi(text).ok_or_else(failed)?,
                );
            }
            if axes.is_some() {
                overlay.commands.push_str("Q\n");
            }
        }
    }
    for (run, (font, shaped)) in redrawn.iter().zip(following_fonts) {
        let (x, y) = first_glyph(run, chars)
            .or_else(|| paragraph_glyph(run, chars))
            .map(|glyph| run_edit::origin(run, glyph))
            .ok_or_else(|| refusal(422, "Cannot isolate the following text baseline"))?;
        let size = f64::from(run.font_size);
        let ink = (run.color.r.into(), run.color.g.into(), run.color.b.into());
        if let Some(shaped) = shaped {
            shaped
                .write(&mut overlay, &run.text, size, x, y - shift, ink)
                .map_err(|detail| refusal(422, detail))?;
        } else if let Some(font) = font {
            font.write(&mut overlay, &run.text, size, x, y - shift, ink)?;
        } else {
            append_text(
                &mut overlay,
                choose_font(run),
                size,
                x,
                y - shift,
                ink,
                &winansi(&run.text).ok_or_else(failed)?,
            );
        }
    }
    let data = assembly::append_document_overlays(removed, &HashMap::from([(page, overlay)]), None)
        .map_err(|_| failed())?;
    if let Some(original) = original_paragraph {
        inspect_paragraph(
            &pdf,
            &data,
            original_bytes,
            page,
            region,
            &original,
            &lines,
            overlaps_suppressed,
        )?;
    }
    let details = if move_text {
        json!({"edit_method":"remove-and-redraw"})
    } else if paragraph_edit {
        let top = frame.visible.display_size().1;
        json!({"edit_layout":{"edit_method":"remove-and-redraw","engine":TRIAL_ENGINE,
            "font_substituted":standard_substitute || (embedded.is_none() && shaped.is_none() && span.font_name!=font),
            "font_name":shaped.as_ref().map(|(font, _)| font.base_font.as_str())
                .or_else(|| embedded.as_ref().map(|embedded| embedded.font.base_font.as_str())).unwrap_or(font),"line_count":lines.len(),
            "edit_region":[region[0]-3.0,top-region[3]-3.0,region[2]+3.0,top-region[1]+3.0],
            "following_text_shift":(shift*100.0).round()/100.0,"following_text_objects_moved":following_objects_moved}})
    } else {
        let physical_size = size * frame.unit;
        json!({"edit_layout":{"font_size":(physical_size*100.0).round()/100.0,"line_count":lines.len(),
            "font_size_reduced":physical_size<source_size*frame.unit-0.01,"edit_method":"remove-and-redraw"}})
    };
    Ok((data, details))
}

fn replacement_tag(
    pdf: &PdfDocument,
    page: usize,
    span: &TextSpan,
) -> Option<assembly::ReplacementTag> {
    pdf.catalog().ok()?.as_dict()?.get("StructTreeRoot")?;
    let (object, from_page) = match span.mcid_scope {
        Some(McidScope::Form(id)) => (pdf.load_object(id).ok()?, false),
        Some(McidScope::Page(index)) if index as usize == page => {
            (pdf.get_page_for_debug(page).ok()?, true)
        }
        _ => return None,
    };
    let fields = object.as_dict()?;
    let (value, mcid) = if let Some(value) = fields.get("StructParents") {
        (value, Some(span.mcid? as usize))
    } else if from_page {
        return None;
    } else {
        (fields.get("StructParent")?, None)
    };
    Some(assembly::ReplacementTag {
        parent_key: pdf.resolve_object(value).ok()?.as_integer()?,
        mcid,
        from_page,
        remove_actual_text: false,
        retained_anchor: None,
    })
}

fn page_parent_key(pdf: &PdfDocument, page: usize) -> Option<i64> {
    pdf.catalog().ok()?.as_dict()?.get("StructTreeRoot")?;
    let page = pdf.get_page_for_debug(page).ok()?;
    pdf.resolve_object(page.as_dict()?.get("StructParents")?)
        .ok()?
        .as_integer()
}

fn fits(
    lines: &[&str],
    x: f64,
    y: f64,
    size: f64,
    measure: &impl Fn(&str, f64) -> f64,
    bounds: Rect,
    others: &[Rect],
    annotations: &[Rect],
) -> bool {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !line.is_empty())
        .all(|(index, line)| {
            let baseline = y - size * 1.2 * index as f64;
            let area = [
                x,
                baseline - size * 0.3,
                x + measure(line, size),
                baseline + size * 0.9,
            ];
            contains(bounds, area)
                && !others
                    .iter()
                    .chain(annotations)
                    .any(|other| intersects(area, *other))
        })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextPageAction {
    Runs,
    Preview,
    Replace,
    Move,
    Paragraph,
}

impl TextPageAction {
    fn mutates(self) -> bool {
        matches!(self, Self::Replace | Self::Move | Self::Paragraph)
    }

    fn stale(self) -> Reply {
        refusal(
            409,
            if self.mutates() {
                "Working revision changed during processing"
            } else {
                "Document changed; reload and select text again"
            },
        )
    }
}

pub struct TextPageTarget {
    document: String,
    action: TextPageAction,
    query_page: Option<String>,
}

impl TextPageTarget {
    pub fn from_path(method: &str, path: &str) -> Option<Self> {
        let route = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
        Self::select(method, &route)
    }

    fn select(method: &str, route: &[&str]) -> Option<Self> {
        let (document, action, query_page) = match (method, route) {
            ("GET", ["documents", id, "pages", page, "text-runs"]) => {
                (id, TextPageAction::Runs, Some((*page).to_owned()))
            }
            ("POST", ["documents", id, "pages", "paragraph-trial-preview"]) => {
                (id, TextPageAction::Preview, None)
            }
            ("POST", ["documents", id, "pages", "paragraph-trial"]) => {
                (id, TextPageAction::Paragraph, None)
            }
            ("POST", ["documents", id, "pages", "replace-text-run"]) => {
                (id, TextPageAction::Replace, None)
            }
            ("POST", ["documents", id, "pages", "move-text-run"]) => {
                (id, TextPageAction::Move, None)
            }
            _ => return None,
        };
        Some(Self {
            document: (*document).to_owned(),
            action,
            query_page,
        })
    }
}

pub struct TextPageWork {
    root: PathBuf,
    document: String,
    action: TextPageAction,
    revision: u64,
    page: usize,
    page_count: u64,
    body: Value,
    working: super::artifacts::PdfSource,
    publication: Option<super::document_verification::DocumentFiles>,
}

enum TextPageResult {
    Read(Value),
    Edit {
        pdf: super::revisions::ValidatedRevisionPdf,
        files: super::document_verification::DocumentFileProof,
        details: serde_json::Map<String, Value>,
    },
}

pub struct MeasuredTextPage {
    root: PathBuf,
    document: String,
    action: TextPageAction,
    revision: u64,
    acknowledged: bool,
    working: super::artifacts::PdfProof,
    result: TextPageResult,
}

impl TextPageWork {
    pub fn compute(self) -> Result<MeasuredTextPage, Reply> {
        let (snapshot, working) = self.working.open_with_proof()?;
        let bytes = snapshot.into_bytes().map_err(|_| refusal(409, INVALID))?;
        let original_bytes = bytes.len();
        working.verify_bytes(&bytes)?;
        let result = if self.action.mutates() {
            let (candidate, details) = edit(
                bytes,
                self.page,
                &self.body,
                self.action == TextPageAction::Move,
                self.action == TextPageAction::Paragraph,
            )?;
            let details = details
                .as_object()
                .cloned()
                .ok_or_else(|| refusal(500, "Working revision details are unavailable"))?;
            let (_, files) = self
                .publication
                .ok_or_else(|| refusal(409, "Document files are unavailable"))?
                .authenticate()
                .map_err(|error| refusal(409, error))?;
            let pdf = super::revisions::ValidatedRevisionPdf::prepare(candidate, self.page_count)?;
            TextPageResult::Edit {
                pdf,
                files,
                details,
            }
        } else {
            let pdf = PdfDocument::from_bytes(bytes).map_err(|_| refusal(422, INVALID))?;
            let pdf =
                match font_state::page(&pdf, self.page).map_err(|detail| refusal(422, detail))? {
                    Some((content, forms)) => {
                        let bytes = assembly::rebind_text_page(pdf, self.page, content, forms)
                            .map_err(|detail| refusal(422, detail))?;
                        PdfDocument::from_bytes(bytes).map_err(|_| refusal(422, INVALID))?
                    }
                    None => pdf,
                };
            TextPageResult::Read(match self.action {
                TextPageAction::Runs => view(&pdf, self.page, self.revision)?,
                _ => paragraph_preview(&pdf, self.page, &self.body, original_bytes)?,
            })
        };
        Ok(MeasuredTextPage {
            root: self.root,
            document: self.document,
            action: self.action,
            revision: self.revision,
            acknowledged: self.body["acknowledge_signature_risk"] == true,
            working,
            result,
        })
    }
}

impl Store {
    pub fn prepare_text_page(
        &self,
        target: TextPageTarget,
        body: &Value,
    ) -> Result<TextPageWork, Reply> {
        let TextPageTarget {
            document,
            action,
            query_page,
        } = target;
        let body = match action {
            TextPageAction::Runs => Value::Null,
            TextPageAction::Preview => request::parse(body, request::Model::Selection)?,
            TextPageAction::Move => request::parse(body, request::Model::Move)?,
            TextPageAction::Replace | TextPageAction::Paragraph => {
                request::parse(body, request::Model::Replacement)?
            }
        };
        let doc = self.artifact_document(&document)?;
        if action.mutates() {
            signature_guard(
                [&doc.body],
                body["acknowledge_signature_risk"] == true,
                "transforming this working copy",
            )?;
        }
        let revision = if action == TextPageAction::Runs {
            doc.body["revision"]
                .as_u64()
                .ok_or_else(|| refusal(409, "Invalid working revision"))?
        } else {
            body["expected_revision"]
                .as_u64()
                .ok_or_else(|| refusal(422, "Invalid working revision"))?
        };
        if doc.body["revision"] != revision {
            return Err(refusal(
                409,
                "Document changed; reload and select text again",
            ));
        }
        if matches!(action, TextPageAction::Preview | TextPageAction::Paragraph)
            && paragraph_trial_disabled()
        {
            let message = if action == TextPageAction::Paragraph {
                "PDFium paragraph trial is unavailable; no revision was published"
            } else {
                "PDFium paragraph trial is unavailable"
            };
            return Err(refusal(422, message));
        }
        if action == TextPageAction::Paragraph
            && body["replacement"]
                .as_str()
                .is_some_and(|text| text.trim().is_empty())
        {
            return Err(refusal(422, "Replacement paragraph is empty"));
        }
        let page = if let Some(query_page) = query_page {
            query_page.parse::<usize>().ok()
        } else {
            body["page_index"]
                .as_u64()
                .and_then(|page| usize::try_from(page).ok())
        }
        .ok_or_else(|| refusal(422, "Invalid page number"))?;
        let working = self.pdf_source(
            &document,
            &Artifact::Working {
                revision: Some(revision),
            },
        )?;
        let publication = action
            .mutates()
            .then(|| {
                super::document_verification::DocumentFiles::capture(
                    &self.con,
                    &self.root,
                    self.key.as_ref(),
                    &doc.body,
                    &document,
                )
                .map_err(|error| refusal(409, error))
            })
            .transpose()?;
        Ok(TextPageWork {
            root: self.root.clone(),
            document,
            action,
            revision,
            page,
            page_count: doc.body["page_count"].as_u64().unwrap_or_default(),
            body,
            working,
            publication,
        })
    }

    pub fn complete_text_page(&mut self, measured: MeasuredTextPage) -> Result<Reply, Reply> {
        let MeasuredTextPage {
            root,
            document,
            action,
            revision,
            acknowledged,
            working,
            result,
        } = measured;
        if self.root != root {
            return Err(action.stale());
        }
        let doc = self.artifact_document(&document)?.body;
        if doc["revision"] != revision {
            return Err(action.stale());
        }
        if action.mutates() {
            signature_guard([&doc], acknowledged, "transforming this working copy")?;
        }
        let current = self.pdf_source(
            &document,
            &Artifact::Working {
                revision: Some(revision),
            },
        )?;
        if !working.matches(&current) {
            return Err(action.stale());
        }
        working.check_current()?;
        match result {
            TextPageResult::Read(value) => Ok(reply(200, value)),
            TextPageResult::Edit {
                pdf,
                files,
                details,
            } => {
                let provenance = doc["ocr_provenance_json"]
                    .as_str()
                    .unwrap_or("[]")
                    .to_owned();
                let mut result = self
                    .publish_measured_text_revision(&document, revision, pdf, files, provenance)
                    .map_err(|_| {
                        refusal(500, "Rust matter transaction failed; no changes committed")
                    })?;
                if result.status == 200 {
                    if let Some(body) = result.body.as_object_mut() {
                        body.extend(details);
                    }
                }
                Ok(result)
            }
        }
    }

    pub(super) fn text_request(
        &mut self,
        method: &str,
        route: &[&str],
        body: &Value,
    ) -> rusqlite::Result<Option<Reply>> {
        if ("GET", ["pdfium-paragraph-trial", "status"].as_slice()) == (method, route) {
            let reason = if paragraph_trial_disabled() {
                Some("Trial disabled locally")
            } else if self.key.is_none() {
                Some("Use the protected Rust store for working revisions")
            } else {
                None
            };
            return Ok(Some(reply(
                200,
                json!({
                    "available": reason.is_none(), "engine": TRIAL_ENGINE, "reason": reason,
                }),
            )));
        }
        Ok(TextPageTarget::select(method, route).map(|target| {
            self.prepare_text_page(target, body)
                .and_then(TextPageWork::compute)
                .and_then(|measured| self.complete_text_page(measured))
                .unwrap_or_else(|error| error)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secure_db;

    fn transformed_fixture(content: &str) -> Vec<u8> {
        let blank = assembly::blank((420.0, 300.0)).unwrap();
        let overlay = assembly::Overlay {
            commands: format!("q {content} Q\nBT /@F0 12 Tf 1 0 0 1 45 65 Tm (FIXED LABEL) Tj ET"),
            fonts: vec!["Helvetica"],
            user_space_text: true,
            ..Default::default()
        };
        assembly::append_document_overlays(blank, &HashMap::from([(0, overlay)]), None).unwrap()
    }

    fn assert_run(run: &Value, expected_size: f64, expected_origin: (f64, f64)) {
        let origin = &run["origin"];
        for (actual, expected) in [
            (run["size"].as_f64(), expected_size),
            (origin[0].as_f64(), expected_origin.0),
            (origin[1].as_f64(), expected_origin.1),
        ] {
            assert!((actual.unwrap() - expected).abs() < 0.01, "{run}");
        }
    }

    fn protected_store(root: &Path) -> Store {
        let key = DataKey::from_bytes([19; 32]);
        let db = root.join("text-runs.sqlite3");
        secure_db::create_protected(&db, &key).unwrap();
        let con = secure_db::open_protected(&db, &key).unwrap();
        Store::open_keyed(root, con, key).unwrap()
    }

    #[test]
    fn move_and_replace_use_the_visible_size_and_baseline_of_transformed_runs() {
        let root = tempfile::tempdir().unwrap();
        let mut store = protected_store(root.path());
        for (content, size, origin) in [
            (
                "BT /@F0 14 Tf 169 Tz 1 0 0 1 45 215 Tm (SELECTED LABEL) Tj ET",
                18.2,
                (45.0, 85.0),
            ),
            (
                "BT /@F0 14 Tf 64 Tz 1 0 0 1 45 215 Tm [(SELECTED ) 0 (LABEL)] TJ ET",
                11.2,
                (45.0, 85.0),
            ),
            (
                "BT /@F0 14 Tf 1.8 0 .35 .8 45 215 Tm (SELECTED LABEL) Tj ET",
                16.8,
                (45.0, 85.0),
            ),
            (
                "q 1.8 0 .35 .8 0 0 cm BT /@F0 14 Tf 1 0 0 1 30 255 Tm (SELECTED LABEL) Tj ET Q",
                16.8,
                (143.25, 96.0),
            ),
            (
                "BT /@F0 14 Tf 169 Tz 10 Ts 1 0 0 1 45 215 Tm (SELECTED LABEL) Tj ET",
                18.2,
                (45.0, 75.0),
            ),
        ] {
            let original = transformed_fixture(content);
            for moving in [false, true] {
                let matter = store.request(
                    "POST",
                    "/matters",
                    json!({"name":"Synthetic native labels"}),
                );
                assert_eq!(matter.status, 201, "{matter:?}");
                let matter = matter.body["id"].as_str().unwrap();
                let imported = store.import_protected_pdf(
                    matter,
                    "synthetic.pdf",
                    &original,
                    ImportOptions::default(),
                );
                assert_eq!(imported.status, 201, "{imported:?}");
                let document = imported.body[0]["id"].as_str().unwrap();
                let path = format!("/documents/{document}/pages");
                let runs_path = format!("{path}/0/text-runs");
                let before = store.request("GET", &runs_path, Value::Null);
                assert_eq!(before.status, 200, "{before:?}");
                let runs = before.body["runs"].as_array().unwrap();
                let selected = runs
                    .iter()
                    .find(|run| run["text"] == "SELECTED LABEL")
                    .unwrap();
                assert_run(selected, size, origin);

                let mut body = json!({"page_index":0,"span_index":selected["index"],
                    "expected_revision":1,"expected_text":selected["text"]});
                let (action, changed, expected_origin) = if moving {
                    body["horizontal_offset"] = json!(0.1);
                    body["vertical_offset"] = json!(0.2);
                    (
                        "move-text-run",
                        "SELECTED LABEL",
                        (origin.0 + 42.0, origin.1 + 60.0),
                    )
                } else {
                    body["replacement"] = json!("RECAST LINE");
                    ("replace-text-run", "RECAST LINE", origin)
                };
                let result = store.request("POST", &format!("{path}/{action}"), body);
                assert_eq!(result.status, 200, "{content} {result:?}");
                assert_eq!(result.body["revision"], 2, "{result:?}");
                if !moving {
                    assert_eq!(result.body["edit_layout"]["font_size"], size, "{result:?}");
                    assert_eq!(
                        result.body["edit_layout"]["font_size_reduced"], false,
                        "{result:?}"
                    );
                }
                let after = store.request("GET", &runs_path, Value::Null);
                assert_eq!(after.status, 200, "{after:?}");
                assert_eq!(after.body["revision"], 2, "{after:?}");
                let runs = after.body["runs"].as_array().unwrap();
                assert_eq!(runs.len(), 2, "{after:?}");
                let edited = runs.iter().find(|run| run["text"] == changed).unwrap();
                assert_run(edited, size, expected_origin);
                let fixed = runs
                    .iter()
                    .find(|run| run["text"] == "FIXED LABEL")
                    .unwrap();
                assert_run(fixed, 12.0, (45.0, 235.0));
                let (bytes, _) = store
                    .pdf_source(document, &Artifact::Original)
                    .unwrap()
                    .read_with_proof(|| false)
                    .unwrap();
                assert_eq!(bytes, original, "{content}");
            }
        }
    }

    fn nested_form_fixture(local_scale: &str) -> Vec<u8> {
        let stream = |dictionary: &str, data: &str| {
            format!(
                "<< {dictionary} /Length {} >>\nstream\n{data}\nendstream",
                data.len()
            )
        };
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 420 300] /Contents 4 0 R /Resources << /Font << /F0 5 0 R >> /XObject << /Outer 7 0 R >> >> >>".to_owned(),
            stream(
                "",
                concat!(
                    "q BT 64 Tz ET 1 0 0 1 40 180 cm /Outer Do Q\n",
                    "q BT 144 Tz ET 1 0 0 1 45 100 cm /Outer Do Q\n",
                    "BT /F0 12 Tf 1 0 0 1 40 30 Tm (OUTER FIXED) Tj ET",
                ),
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_owned(),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 265 70] /Resources << /Font << /F0 5 0 R >> >>",
                   &format!("BT /F0 14 Tf {local_scale} 1 0 0 1 5 15 Tm (NESTED LABEL) Tj ET")),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 265 95] /Resources << /XObject << /Inner 6 0 R >> >>",
                   "q 1 0 0 1 0 10 cm /Inner Do Q"),
        ];
        let mut pdf = String::from("%PDF-1.7\n");
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            writeln!(pdf, "{} 0 obj\n{object}\nendobj", index + 1).unwrap();
        }
        let xref = pdf.len();
        let size = objects.len() + 1;
        writeln!(pdf, "xref\n0 {size}\n0000000000 65535 f ").unwrap();
        for offset in offsets {
            writeln!(pdf, "{offset:010} 00000 n ").unwrap();
        }
        writeln!(
            pdf,
            "trailer\n<< /Root 1 0 R /Size {size} >>\nstartxref\n{xref}\n%%EOF"
        )
        .unwrap();
        pdf.into_bytes()
    }

    #[test]
    fn nested_forms_keep_local_and_inherited_run_scales_after_editing() {
        let root = tempfile::tempdir().unwrap();
        let mut store = protected_store(root.path());
        for (local_scale, first_size, second_size) in [("169 Tz", 18.2, 18.2), ("", 11.2, 16.8)] {
            let original = nested_form_fixture(local_scale);
            for moving in [false, true] {
                let matter =
                    store.request("POST", "/matters", json!({"name":"Synthetic nested text"}));
                assert_eq!(matter.status, 201, "{matter:?}");
                let imported = store.import_protected_pdf(
                    matter.body["id"].as_str().unwrap(),
                    "nested.pdf",
                    &original,
                    ImportOptions::default(),
                );
                assert_eq!(imported.status, 201, "{imported:?}");
                let document = imported.body[0]["id"].as_str().unwrap();
                let base = format!("/documents/{document}/pages");
                let route = format!("{base}/0/text-runs");
                let before = store.request("GET", &route, Value::Null);
                assert_eq!(before.status, 200, "{before:?}");
                let runs = before.body["runs"].as_array().unwrap();
                assert_eq!(runs.len(), 3, "{before:?}");
                let selected = runs.iter().find(|run| run["origin"][1] == 95.0).unwrap();
                let neighbor = runs.iter().find(|run| run["origin"][1] == 175.0).unwrap();
                assert_eq!(selected["text"], "NESTED LABEL");
                assert_run(selected, first_size, (45.0, 95.0));
                assert_run(neighbor, second_size, (50.0, 175.0));
                let mut body = json!({"page_index":0,"span_index":selected["index"],
                    "expected_text":"NESTED LABEL","expected_revision":1});
                let (action, edited_text, x, y) = if moving {
                    body["vertical_offset"] = json!(0.06);
                    body["horizontal_offset"] = json!(0.1);
                    ("move-text-run", "NESTED LABEL", 87.0, 113.0)
                } else {
                    body["replacement"] = json!("RECAST LINE");
                    ("replace-text-run", "RECAST LINE", 45.0, 95.0)
                };
                let result = store.request("POST", &format!("{base}/{action}"), body);
                assert_eq!(result.status, 200, "{local_scale} {result:?}");
                assert_eq!(result.body["revision"], 2, "{result:?}");
                let after = store.request("GET", &route, Value::Null);
                assert_eq!(after.status, 200, "{after:?}");
                assert_eq!(after.body["revision"], 2, "{after:?}");
                let runs = after.body["runs"].as_array().unwrap();
                assert_eq!(runs.len(), 3, "{after:?}");
                let find_run = |baseline: f64| {
                    runs.iter()
                        .find(|run| (run["origin"][1].as_f64().unwrap() - baseline).abs() < 0.01)
                        .unwrap()
                };
                assert_eq!(find_run(y)["text"], edited_text);
                assert_run(find_run(y), first_size, (x, y));
                assert_eq!(find_run(175.0)["text"], "NESTED LABEL");
                assert_run(find_run(175.0), second_size, (50.0, 175.0));
                assert_run(find_run(270.0), 12.0, (40.0, 270.0));
                let (bytes, _) = store
                    .pdf_source(document, &Artifact::Original)
                    .unwrap()
                    .read_with_proof(|| false)
                    .unwrap();
                assert_eq!(bytes, original);
            }
        }
    }
}
