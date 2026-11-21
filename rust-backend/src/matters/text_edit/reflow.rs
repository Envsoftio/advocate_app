//! Inspect source text objects and translate complete lines with their original paint.
use super::{
    actual_text, assembly, bbox, intersects, shifted, standard_ink, union, ParagraphPlacement,
    Rect, CONTINUATION,
};
use crate::matters::document_output::{
    embedded_paragraph_bounds,
    source_codes::{CidEncoding, Code, Codes},
};
use crate::matters::redaction_vectors::{rewrite_ranges, text_position_ranges};
use crate::vault::hex_bytes;
use pdf_oxide::{
    content::{parse_content_stream, Matrix, Operator, TextElement},
    extractors::TextExtractor,
    fonts::{encoding::unicode_to_winansi, FontInfo},
    layout::{TextChar, TextSpan},
    object::{Object, ObjectRef},
    optional_content::compute_default_off_ocgs,
    structure::McidScope,
    PdfDocument,
};
use std::{
    collections::{HashMap, VecDeque},
    ops::Range,
};

const FIRST_MARK: u32 = 1_500_000_000;
const MAX_SHOWS: usize = 131_072;
const MAX_BYTES: usize = 64 * 1024 * 1024;
pub(super) const POSITION_TOLERANCE: f64 = 0.08;

#[derive(Clone, Copy)]
struct Graphics {
    ctm: Matrix,
    render: u8,
    leading: f32,
    font: Option<usize>,
    size: f32,
    scale: f32,
    rise: f32,
}

impl Default for Graphics {
    fn default() -> Self {
        Self {
            ctm: Matrix::default(),
            render: 0,
            leading: 0.0,
            font: None,
            size: 0.0,
            scale: 1.0,
            rise: 0.0,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct ShowStyle {
    pub axes: [f64; 4],
    pub size: f64,
    pub rise: f64,
    font: Option<usize>,
}

#[derive(Default)]
pub(super) struct RunScales(HashMap<i64, Vec<SourceRunScale>>);

struct SourceRunScale {
    text: String,
    font: String,
    x: f64,
    y: f64,
    first_advance: Option<f32>,
    scope: Option<McidScope>,
    scale: f64,
}

impl RunScales {
    pub fn read(pdf: &PdfDocument, page: usize, geometry: assembly::Geometry) -> Self {
        let mut rows = HashMap::<i64, Vec<_>>::new();
        for (span, scale) in actual_text::run_scales(pdf, page, geometry).unwrap_or_default() {
            let row = span.bbox.y.floor() as i64;
            rows.entry(row).or_default().push(SourceRunScale {
                text: span.text,
                font: span.font_name,
                x: f64::from(span.bbox.x),
                y: f64::from(span.bbox.y),
                first_advance: span.char_widths.first().copied(),
                scope: span.mcid_scope,
                scale,
            });
        }
        Self(rows)
    }

    pub fn factor(&self, span: &TextSpan) -> f64 {
        let reported = f64::from(span.horizontal_scaling) / 100.0;
        if reported != 1.0 {
            return reported;
        }
        let x = f64::from(span.bbox.x);
        let y = f64::from(span.bbox.y);
        let row = y.floor() as i64;
        let mut matches = (row.saturating_sub(1)..=row.saturating_add(1))
            .flat_map(|row| self.0.get(&row).into_iter().flatten())
            .filter(|authored| {
                (x - authored.x).abs() <= POSITION_TOLERANCE
                    && (y - authored.y).abs() <= POSITION_TOLERANCE
                    && authored.font == span.font_name
                    && authored.scope == span.mcid_scope
                    && authored
                        .first_advance
                        .zip(span.char_widths.first().copied())
                        .is_none_or(|(a, b)| f64::from((a - b).abs()) <= POSITION_TOLERANCE)
                    && (authored.text.starts_with(&span.text)
                        || span.text.starts_with(&authored.text))
            })
            .map(|authored| authored.scale);
        // Marked extraction recovers the source Tz for standalone Tj shows.
        // Keep the reader's value if several different source shows match.
        matches
            .next()
            .filter(|first| matches.all(|scale| scale == *first))
            .unwrap_or(reported)
    }
}

impl ShowStyle {
    pub fn supports_paragraph(self) -> bool {
        let [a, b, c, d] = self.axes;
        self.axes
            .into_iter()
            .chain([self.size, self.rise])
            .all(f64::is_finite)
            && a > 0.0
            && d > 0.0
            && b.abs() <= 0.001
            && c.abs() <= 0.001
            && self.size > 0.0
    }
}

struct Show {
    ordinal: usize,
    range: Range<usize>,
    style: ShowStyle,
    ctm: Matrix,
    multiple_strings: bool,
    encoded_text: Vec<Vec<u8>>,
    render_mode: u8,
}

#[derive(Clone, Copy)]
enum StandardEncoding {
    BuiltIn,
    WinAnsi,
}

impl StandardEncoding {
    fn winansi_code(self, code: u8) -> Option<u8> {
        match (self, code) {
            (Self::WinAnsi, 32..=126 | 128..=255) => Some(code),
            (Self::BuiltIn, b'\'') => Some(0x92),
            (Self::BuiltIn, b'`') => Some(0x91),
            (Self::BuiltIn, 32..=126) => Some(code),
            _ => None,
        }
    }
}

pub(super) struct SourceFont {
    pub name: String,
    pub resource: Option<String>,
    pub embedded: bool,
    pub shaped: bool,
    binding: Object,
    composite: bool,
    encoding: Option<CidEncoding>,
    ink_face: Option<(usize, StandardEncoding)>,
}

impl SourceFont {
    fn read(pdf: &PdfDocument, binding: &Object, resource: Option<String>) -> Option<Self> {
        let object = pdf.resolve_object(binding).ok()?;
        let fields = object.as_dict()?;
        let subtype = fields.get("Subtype")?.as_name()?;
        let name = fields
            .get("BaseFont")
            .or_else(|| fields.get("Name"))
            .and_then(Object::as_name)
            .unwrap_or_default()
            .to_owned();
        let shaped = fields
            .get("ToUnicode")
            .and_then(|binding| pdf.resolve_object(binding).ok())
            .and_then(|cmap| {
                cmap.as_dict()?
                    .get("CMapName")?
                    .as_name()
                    .map(str::to_owned)
            })
            .as_deref()
            == Some("MatterDraftShaped");
        let descendant = (subtype == "Type0")
            .then(|| {
                let fonts = pdf.resolve_object(fields.get("DescendantFonts")?).ok()?;
                pdf.resolve_object(fonts.as_array()?.first()?).ok()
            })
            .flatten();
        let descriptor = descendant
            .as_ref()
            .and_then(Object::as_dict)
            .unwrap_or(fields)
            .get("FontDescriptor")
            .and_then(|descriptor| pdf.resolve_object(descriptor).ok());
        let embedded = subtype == "Type3"
            || descriptor
                .as_ref()
                .and_then(Object::as_dict)
                .is_some_and(|dict| {
                    ["FontFile", "FontFile2", "FontFile3"]
                        .into_iter()
                        .any(|key| {
                            dict.get(key)
                                .and_then(|binding| pdf.resolve_object(binding).ok())
                                .is_some_and(|object| matches!(object, Object::Stream { .. }))
                        })
                });
        let composite = subtype == "Type0";
        let encoding = composite
            .then(|| CidEncoding::load(pdf, fields.get("Encoding")?, 0))
            .flatten();
        let ink_face = (subtype == "Type1" && !embedded)
            .then(|| {
                let face = standard_ink::face(&name)?;
                let encoding = match fields.get("Encoding") {
                    None => StandardEncoding::BuiltIn,
                    Some(value) => match pdf.resolve_object(value).ok()? {
                        Object::Null => StandardEncoding::BuiltIn,
                        Object::Name(name) if name == "StandardEncoding" => {
                            StandardEncoding::BuiltIn
                        }
                        Object::Name(name) if name == "WinAnsiEncoding" => {
                            StandardEncoding::WinAnsi
                        }
                        _ => return None,
                    },
                };
                Some((face, encoding))
            })
            .flatten();
        Some(Self {
            name,
            resource,
            embedded,
            shaped,
            binding: binding.clone(),
            composite,
            encoding,
            ink_face,
        })
    }

    fn codes<'a>(&'a self, show: &'a Show) -> Option<impl Iterator<Item = Option<Code>> + 'a> {
        if self.composite && self.encoding.is_none() {
            return None;
        }
        Some(
            show.encoded_text
                .iter()
                .flat_map(|text| Codes::new(text, 1, self.encoding.as_ref())),
        )
    }
}

#[derive(Default)]
struct SourceFonts {
    fonts: Vec<SourceFont>,
    text: HashMap<String, usize>,
    graphics: HashMap<String, (Option<usize>, f32)>,
}

impl SourceFonts {
    fn read(pdf: &PdfDocument, resources: &Object) -> Self {
        let mut result = Self::default();
        let dictionary = |key| {
            resources
                .as_dict()?
                .get(key)
                .and_then(|value| pdf.resolve_object(value).ok())
        };
        let fonts = dictionary("Font");
        if let Some(fonts) = fonts.as_ref().and_then(Object::as_dict) {
            for (name, binding) in fonts {
                if let Some(font) = SourceFont::read(pdf, binding, Some(name.clone())) {
                    result.text.insert(name.clone(), result.fonts.len());
                    result.fonts.push(font);
                }
            }
        }
        let graphics = dictionary("ExtGState");
        if let Some(graphics) = graphics.as_ref().and_then(Object::as_dict) {
            for (name, binding) in graphics {
                let font = pdf
                    .resolve_object(binding)
                    .ok()
                    .and_then(|fields| pdf.resolve_object(fields.as_dict()?.get("Font")?).ok());
                let Some([font, size]) =
                    font.as_ref().and_then(Object::as_array).map(Vec::as_slice)
                else {
                    continue;
                };
                let size = match pdf.resolve_object(size) {
                    Ok(Object::Integer(size)) => size as f32,
                    Ok(Object::Real(size)) => size as f32,
                    _ => continue,
                };
                let index = SourceFont::read(pdf, font, None).map(|font| {
                    let index = result.fonts.len();
                    result.fonts.push(font);
                    index
                });
                result.graphics.insert(name.clone(), (index, size));
            }
        }
        result
    }
}

pub(super) struct TextObject {
    pub show: usize,
    pub text: String,
    pub bounds: Rect,
    pub ink_bounds: Option<Rect>,
    pub style: ShowStyle,
    pub recovered: bool,
    pub reference_suppressed: bool,
    reference_text: Option<String>,
}

impl TextObject {
    pub fn paragraph_bounds(&self) -> Rect {
        self.ink_bounds.unwrap_or(self.bounds)
    }

    pub fn reference_text(&self) -> &str {
        self.reference_text.as_deref().unwrap_or(&self.text)
    }

    pub fn covers_fragment(&self, bounds: Rect, text: &str) -> bool {
        let [left, bottom, right, top] = self.bounds;
        bounds[0] >= left - POSITION_TOLERANCE
            && bounds[2] <= right + POSITION_TOLERANCE
            && (bounds[1] - bottom).abs() <= POSITION_TOLERANCE
            && (bounds[3] - top).abs() <= POSITION_TOLERANCE
            && self.reference_text().contains(text)
    }
}

pub(super) struct PageText {
    pub objects: Vec<TextObject>,
    pub has_untracked_objects: bool,
    fonts: Vec<SourceFont>,
}

impl PageText {
    pub fn font(&self, style: ShowStyle) -> Option<&SourceFont> {
        self.fonts.get(style.font?)
    }

    pub fn shaped_line(&self, bounds: Rect, text: &str) -> Option<Vec<usize>> {
        if !self.fonts.iter().any(|font| font.shaped) {
            return None;
        }
        let mut objects = Vec::new();
        let mut letters = Vec::new();
        for (index, object) in
            self.objects.iter().enumerate().filter(|(_, object)| {
                !object.reference_suppressed && intersects(object.bounds, bounds)
            })
        {
            if !self.font(object.style).is_some_and(|font| font.shaped)
                || object.bounds[0] + POSITION_TOLERANCE < bounds[0]
                || object.bounds[2] - POSITION_TOLERANCE > bounds[2]
            {
                return None;
            }
            objects.push(index);
            letters.extend(object.text.chars());
        }
        let mut expected = text.chars().collect::<Vec<_>>();
        letters.sort_unstable();
        expected.sort_unstable();
        (!objects.is_empty() && letters == expected).then_some(objects)
    }

    pub fn inspect(pdf: &PdfDocument, page: usize) -> Option<Self> {
        let data = pdf.get_page_content_data(page).ok()?;
        let resources = pdf.get_page_resources(page).ok()?;
        let fonts = SourceFonts::read(pdf, &resources);
        let (segments, properties) = segments(&data, Some(&fonts))?;
        let (fragments, owners) =
            authored_spans(pdf, page, &data, &segments, &properties, &fonts, true)?;
        let embedded_bounds = if fonts.fonts.iter().any(|font| font.embedded && !font.shaped) {
            embedded_paragraph_bounds(pdf, &resources, &data).unwrap_or_default()
        } else {
            HashMap::new()
        };
        let has_untracked_objects = owners.iter().enumerate().any(|(index, (_, show))| {
            !show.style.supports_paragraph()
                || fragments[index].reference_suppressed
                || fragments[index].spans.is_empty()
        });
        let objects = fragments
            .into_iter()
            .zip(owners)
            .filter_map(|(fragments, (_, show))| {
                let spans = &fragments.spans;
                let text = spans
                    .iter()
                    .flat_map(|span| letters(&span.text))
                    .collect::<String>();
                (!text.is_empty()).then(|| TextObject {
                    show: show.ordinal,
                    text,
                    bounds: union(
                        spans
                            .iter()
                            .filter(|span| !span.text.trim().is_empty())
                            .map(bbox),
                    ),
                    style: show.style,
                    ink_bounds: fragments.ink_bounds.or_else(|| {
                        if fonts.fonts.get(show.style.font?)?.shaped {
                            return None;
                        }
                        embedded_bounds.get(&show.ordinal).copied()
                    }),
                    recovered: fragments.recovered,
                    reference_suppressed: fragments.reference_suppressed,
                    reference_text: fragments.reference_text,
                })
            })
            .collect();
        Some(Self {
            objects,
            has_untracked_objects,
            fonts: fonts.fonts,
        })
    }
}

enum Position {
    Start(usize),
    Matrix(Range<usize>),
    Line {
        range: Range<usize>,
        x: f32,
        y: f32,
        command: LineCommand,
    },
}

enum LineCommand {
    Offset,
    SetLeading,
    Show {
        text: Vec<u8>,
        spacing: Option<(f32, f32)>,
    },
}

struct Segment {
    position: Position,
    to_page: Matrix,
    shows: Vec<Show>,
    movable: bool,
}

impl Segment {
    fn new(position: Position, to_page: Matrix) -> Self {
        Self {
            position,
            to_page,
            shows: Vec::new(),
            movable: true,
        }
    }
}

fn segments(data: &[u8], fonts: Option<&SourceFonts>) -> Option<(Vec<Segment>, Vec<Object>)> {
    if data.len() > MAX_BYTES {
        return None;
    }
    let ranges = text_position_ranges(data).ok()?;
    if ranges.shows.len() > MAX_SHOWS {
        return None;
    }
    let mut boundaries = ranges.boundaries.into_iter();
    let mut shows = ranges.shows.into_iter();
    let mut matrices = ranges.matrices.into_iter();
    let mut offsets = ranges.offsets.into_iter();
    let mut segments = Vec::new();
    let mut properties = Vec::new();
    let mut state = Graphics::default();
    let mut line_basis = Matrix::default();
    let mut saved = Vec::new();
    let mut block = None;
    let mut movable = true;
    let mut show_ordinal = 0;
    for op in parse_content_stream(data).ok()? {
        match op {
            Operator::SaveState => {
                movable &= block.is_none();
                saved.push((state, line_basis));
            }
            Operator::RestoreState => {
                movable &= block.is_none();
                (state, line_basis) = saved.pop()?;
            }
            Operator::Cm { a, b, c, d, e, f } => {
                movable &= block.is_none();
                state.ctm = Matrix { a, b, c, d, e, f }.multiply(&state.ctm);
            }
            Operator::Tr { render } => state.render = render,
            Operator::TL { leading } => state.leading = leading,
            Operator::Tf { font, size } => {
                state.font = fonts.and_then(|fonts| fonts.text.get(&font)).copied();
                state.size = size;
            }
            Operator::SetExtGState { dict_name } => {
                if let Some(&(font, size)) = fonts.and_then(|fonts| fonts.graphics.get(&dict_name))
                {
                    state.font = font;
                    state.size = size;
                }
            }
            Operator::Tz { scale } => state.scale = scale / 100.0,
            Operator::Ts { rise } => state.rise = rise,
            Operator::BeginText => {
                if block.replace(segments.len()).is_some() {
                    return None;
                }
                movable = true;
                line_basis = Matrix::default();
                let range = boundaries.next()?;
                if &data[range.clone()] != b"BT" {
                    return None;
                }
                segments.push(Segment::new(Position::Start(range.end), state.ctm));
            }
            Operator::EndText => {
                let first = block.take()?;
                if &data[boundaries.next()?] != b"ET" {
                    return None;
                }
                if !movable {
                    for segment in &mut segments[first..] {
                        segment.movable = false;
                    }
                }
            }
            Operator::Tm { a, b, c, d, .. } => {
                let range = matrices.next()?;
                block?;
                line_basis = Matrix {
                    a,
                    b,
                    c,
                    d,
                    ..Matrix::default()
                };
                segments.push(Segment::new(Position::Matrix(range), state.ctm));
            }
            Operator::Td { .. } | Operator::TD { .. } | Operator::TStar => {
                block?;
                let (x, y, command) = match op {
                    Operator::Td { tx, ty } => (tx, ty, LineCommand::Offset),
                    Operator::TD { tx, ty } => {
                        state.leading = -ty;
                        (tx, ty, LineCommand::SetLeading)
                    }
                    _ => (0.0, -state.leading, LineCommand::Offset),
                };
                segments.push(Segment::new(
                    Position::Line {
                        range: offsets.next()?,
                        x,
                        y,
                        command,
                    },
                    line_basis.multiply(&state.ctm),
                ));
            }
            Operator::BeginMarkedContentDict {
                properties: value, ..
            } => properties.push(*value),
            Operator::Tj { .. }
            | Operator::TJ { .. }
            | Operator::Quote { .. }
            | Operator::DoubleQuote { .. } => {
                let range = shows.next()?;
                let ordinal = show_ordinal;
                show_ordinal += 1;
                block?;
                // A text clip also affects graphics painted after ET.
                movable &= state.render < 4;
                let strings = match &op {
                    Operator::Tj { text }
                    | Operator::Quote { text }
                    | Operator::DoubleQuote { text, .. } => usize::from(!text.is_empty()),
                    Operator::TJ { array } => array
                        .iter()
                        .filter(
                            |part| matches!(part, TextElement::String(text) if !text.is_empty()),
                        )
                        .take(2)
                        .count(),
                    _ => unreachable!(),
                };
                let encoded_text = match &op {
                    Operator::Tj { text }
                    | Operator::Quote { text }
                    | Operator::DoubleQuote { text, .. } => vec![text.clone()],
                    Operator::TJ { array } => array
                        .iter()
                        .filter_map(|part| match part {
                            TextElement::String(text) => Some(text.clone()),
                            TextElement::Offset(_) => None,
                        })
                        .collect(),
                    _ => unreachable!(),
                };
                let quoted = match op {
                    Operator::Quote { text } => Some(LineCommand::Show {
                        text,
                        spacing: None,
                    }),
                    Operator::DoubleQuote {
                        text,
                        word_space,
                        char_space,
                    } => Some(LineCommand::Show {
                        text,
                        spacing: Some((word_space, char_space)),
                    }),
                    _ => None,
                };
                if let Some(command) = quoted {
                    segments.push(Segment::new(
                        Position::Line {
                            range: range.clone(),
                            x: 0.0,
                            y: -state.leading,
                            command,
                        },
                        line_basis.multiply(&state.ctm),
                    ));
                }
                if strings > 0 {
                    let matrix = line_basis.multiply(&state.ctm);
                    segments.last_mut()?.shows.push(Show {
                        ordinal,
                        range,
                        ctm: state.ctm,
                        multiple_strings: strings > 1,
                        encoded_text,
                        render_mode: state.render,
                        style: ShowStyle {
                            axes: [
                                matrix.a * state.scale,
                                matrix.b * state.scale,
                                matrix.c,
                                matrix.d,
                            ]
                            .map(f64::from),
                            size: f64::from(state.size),
                            rise: f64::from(state.rise),
                            font: state.font,
                        },
                    });
                }
            }
            Operator::Do { .. } | Operator::InlineImage { .. } | Operator::Other { .. } => {
                movable &= block.is_none()
            }
            _ => (),
        }
        if segments.len() > MAX_SHOWS {
            return None;
        }
    }
    if block.is_some()
        || boundaries.next().is_some()
        || shows.next().is_some()
        || matrices.next().is_some()
        || offsets.next().is_some()
    {
        return None;
    }
    Some((segments, properties))
}

fn position_edit(data: &[u8], segment: &Segment, shift: f64) -> Option<(Range<usize>, String)> {
    let Matrix { a, b, c, d, .. } = segment.to_page;
    let [a, b, c, d] = [a, b, c, d].map(f64::from);
    let determinant = a * d - b * c;
    let x = c * shift / determinant;
    let y = -a * shift / determinant;
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    match &segment.position {
        Position::Start(end) => Some((*end..*end, format!("1 0 0 1 {x:.9} {y:.9} Tm"))),
        Position::Matrix(range) => {
            // Retain the spelling and precision of the four linear operands.
            let mut tokens = data[range.clone()]
                .split(|byte| matches!(byte, b'\r' | b'\n'))
                .flat_map(|line| {
                    line.split(|&byte| byte == b'%')
                        .next()
                        .unwrap_or_default()
                        .split(|byte| byte.is_ascii_whitespace() || *byte == 0)
                        .filter(|token| !token.is_empty())
                });
            let mut next = || std::str::from_utf8(tokens.next()?).ok();
            let (a, b, c, d) = (next()?, next()?, next()?, next()?);
            let e = next()?.parse::<f64>().ok()? + x;
            let f = next()?.parse::<f64>().ok()? + y;
            if next()? != "Tm" || tokens.next().is_some() || !e.is_finite() || !f.is_finite() {
                return None;
            }
            Some((range.clone(), format!("{a} {b} {c} {d} {e:.9} {f:.9} Tm")))
        }
        Position::Line {
            range,
            x: tx,
            y: ty,
            command,
        } => {
            let leading = -f64::from(*ty);
            let x = x + f64::from(*tx);
            let y = y + f64::from(*ty);
            if !x.is_finite() || !y.is_finite() {
                return None;
            }
            let offset = format!("{x:.9} {y:.9} Td");
            let replacement = match command {
                LineCommand::Offset => offset,
                LineCommand::SetLeading => format!("{leading:.9} TL {offset}"),
                LineCommand::Show { text, spacing } => {
                    let text = hex_bytes(text);
                    let show = format!("{offset} <{}> Tj", text.as_str());
                    if let Some((word, character)) = spacing {
                        format!("{word} Tw {character} Tc {show}")
                    } else {
                        show
                    }
                }
            };
            Some((range.clone(), replacement))
        }
    }
}

#[derive(Default)]
struct ShowFragments {
    spans: Vec<TextSpan>,
    recovered: bool,
    entire_show_recovered: bool,
    reference_suppressed: bool,
    reference_text: Option<String>,
    ink_bounds: Option<Rect>,
}

fn standard_object_bounds(show: &Show, font: &SourceFont, glyphs: &[TextChar]) -> Option<Rect> {
    let (face, encoding) = font.ink_face?;
    let [_, _, c, d] = show.style.axes;
    if glyphs.len() != show.encoded_text.iter().map(Vec::len).sum::<usize>() {
        return None;
    }
    let origin = glyphs.first()?;
    let placement = ParagraphPlacement::from_glyph(show.style, origin)?;
    let mut bounds = None;
    for (glyph, &code) in glyphs.iter().zip(show.encoded_text.iter().flatten()) {
        let code = encoding.winansi_code(code)?;
        // Match the encoded show to every emitted glyph. If the reader skips
        // a glyph or adds a space for a TJ gap, keep the extraction bounds.
        if glyph.font_name != font.name || unicode_to_winansi(glyph.char as u32) != Some(code) {
            return None;
        }
        let x = f64::from(glyph.origin_x) + c * show.style.rise;
        let y = f64::from(glyph.origin_y) + d * show.style.rise;
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let ink = standard_ink::glyph_bounds(face, code, show.style.size)?;
        let placed = placement.place_bounds(ink, x, y);
        bounds = Some(bounds.map_or(placed, |old| union([old, placed])));
    }
    bounds
}

struct ShowGeometry {
    bounds: Rect,
    origin: (f64, f64),
    last_advance: f64,
    first_advance: f64,
}

impl ShowGeometry {
    fn from_spans(spans: &[TextSpan]) -> Option<Self> {
        let first = spans.first()?;
        let last = spans.last()?;
        let width = last
            .char_widths
            .last()
            .copied()
            .unwrap_or_else(|| last.bbox.width / last.text.chars().count().max(1) as f32);
        Some(Self {
            bounds: union(spans.iter().map(bbox)),
            origin: (
                first
                    .char_x_offsets
                    .first()
                    .copied()
                    .unwrap_or(first.bbox.x)
                    .into(),
                first.bbox.y.into(),
            ),
            last_advance: f64::from(width),
            first_advance: f64::from(
                first
                    .char_widths
                    .first()
                    .copied()
                    .unwrap_or_else(|| first.bbox.width / first.text.chars().count().max(1) as f32),
            ),
        })
    }

    // The reference considers encoded glyphs, intersection with the preceding
    // show, and tolerances based on the current font and the overlapping area.
    fn duplicates(&self, prior: &Self, font_size: f64) -> bool {
        let [left, bottom, right, top] = self.bounds;
        let [prior_left, prior_bottom, prior_right, prior_top] = prior.bounds;
        let width = right.min(prior_right) - left.max(prior_left);
        let height = top.min(prior_top) - bottom.max(prior_bottom);
        width > 0.0
            && height > 0.0
            && width >= (prior_right - prior_left) / 2.0
            && (self.origin.0 - prior.origin.0).abs() <= self.last_advance * 0.9
            && (self.origin.1 - prior.origin.1).abs() <= width.max(height).max(font_size) / 8.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GlyphFont {
    Indirect(ObjectRef),
    Direct(usize),
}

struct ReferenceGlyph {
    font: GlyphFont,
    code: (u32, u8),
    origin: (f64, f64),
}

enum GlyphText {
    Scalar(char),
    Expansion(String),
}

fn same_encoded_text(current: &Show, prior: &Show, fonts: &SourceFonts) -> bool {
    let codes = |show: &Show| {
        fonts
            .fonts
            .get(show.style.font?)?
            .codes(show)?
            .map(|code| code.map(|code| (code.raw, code.bytes)))
            .collect::<Option<Vec<_>>>()
    };
    match (codes(current), codes(prior)) {
        (Some(current), Some(prior)) => current == prior,
        _ => current.encoded_text == prior.encoded_text && current.style.font == prior.style.font,
    }
}

fn reference_glyphs(
    pdf: &PdfDocument,
    show: &Show,
    spans: &[TextSpan],
    fonts: &SourceFonts,
) -> Option<Vec<(ReferenceGlyph, GlyphText)>> {
    let index = show.style.font?;
    let font = fonts.fonts.get(index)?;
    if !show.style.supports_paragraph()
        || spans
            .iter()
            .any(|span| span.wmode != 0 || span.rtl_draw_logical)
        || font
            .encoding
            .as_ref()
            .is_some_and(|encoding| encoding.vertical)
    {
        return None;
    }
    let glyphs = spans
        .iter()
        .flat_map(TextSpan::to_chars)
        .collect::<Vec<_>>();
    let codes = font.codes(show)?.collect::<Option<Vec<_>>>()?;
    if glyphs
        .windows(2)
        .any(|pair| pair[0].origin_x > pair[1].origin_x)
    {
        return None;
    }
    let identity = font
        .binding
        .as_reference()
        .map_or(GlyphFont::Direct(index), GlyphFont::Indirect);
    let reference = |glyph: &pdf_oxide::layout::TextChar, code: Code| ReferenceGlyph {
        font: identity,
        code: (code.raw, code.bytes),
        origin: (glyph.origin_x.into(), glyph.origin_y.into()),
    };
    if glyphs.len() == codes.len() {
        return Some(
            glyphs
                .iter()
                .zip(codes)
                .map(|(glyph, code)| (reference(glyph, code), GlyphText::Scalar(glyph.char)))
                .collect(),
        );
    }
    // Ligatures can decode several characters from one encoded glyph. Only
    // apply suppression if the full mapping agrees with the extracted show.
    let metrics = FontInfo::from_dict(&pdf.resolve_object(&font.binding).ok()?, pdf).ok()?;
    let mut cursor = 0;
    let mut mapped = Vec::with_capacity(codes.len());
    for code in codes {
        let text = metrics
            .char_to_unicode(code.raw)
            .filter(|text| !text.is_empty())?;
        let end = cursor + text.chars().count();
        let matching = glyphs.get(cursor..end)?;
        if !matching.iter().map(|glyph| glyph.char).eq(text.chars()) {
            return None;
        }
        mapped.push((reference(&matching[0], code), GlyphText::Expansion(text)));
        cursor = end;
    }
    (cursor == glyphs.len()).then_some(mapped)
}

fn suppress_reference_glyphs(
    pdf: &PdfDocument,
    owners: &[(usize, &Show)],
    geometry: &[Option<ShowGeometry>],
    fragments: &mut [ShowFragments],
    fonts: &SourceFonts,
) {
    let mut row = Vec::new();
    let mut previous: Option<&ShowGeometry> = None;
    let apply = |row: &mut Vec<usize>, fragments: &mut [ShowFragments]| {
        row.sort_by(|a, b| {
            let origin = |index: usize| geometry[index].as_ref().map_or(0.0, |show| show.origin.0);
            origin(*a).total_cmp(&origin(*b))
        });
        let mut recent = VecDeque::<ReferenceGlyph>::with_capacity(7);
        for index in row.drain(..) {
            let show = owners[index].1;
            let fragment = &mut fragments[index];
            let Some(glyphs) = reference_glyphs(pdf, show, &fragment.spans, fonts) else {
                recent.clear();
                continue;
            };
            let distance = 0.07 * show.style.size * show.style.axes[0];
            let mut changed = false;
            let mut visible = String::new();
            for (glyph, text) in glyphs {
                if recent.iter().any(|candidate| {
                    candidate.font == glyph.font
                        && candidate.code == glyph.code
                        && (candidate.origin.0 - glyph.origin.0).abs() < distance
                        && (candidate.origin.1 - glyph.origin.1).abs() < distance
                }) {
                    changed = true;
                    continue;
                }
                if recent.len() == 7 {
                    recent.pop_front();
                }
                recent.push_back(glyph);
                match text {
                    GlyphText::Scalar(character) => visible.push(character),
                    GlyphText::Expansion(text) => visible.push_str(&text),
                }
            }
            if changed {
                let text = letters(&visible).collect::<String>();
                fragment.reference_suppressed = text.is_empty();
                fragment.reference_text = Some(text);
            }
        }
    };
    for (index, current) in geometry.iter().enumerate() {
        if fragments[index].reference_suppressed {
            continue;
        }
        let Some(current) = current else {
            apply(&mut row, fragments);
            previous = None;
            continue;
        };
        if previous.is_some_and(|prior| {
            (prior.origin.1 - current.origin.1).abs()
                > prior.last_advance.max(current.first_advance) / 2.0
        }) {
            apply(&mut row, fragments);
        }
        row.push(index);
        previous = Some(current);
    }
    apply(&mut row, fragments);
}

impl ShowFragments {
    fn gains_text(&self, probe: &Self) -> bool {
        let mut original = self
            .spans
            .iter()
            .flat_map(|span| letters(&span.text))
            .peekable();
        let mut added = false;
        for character in probe.spans.iter().flat_map(|span| letters(&span.text)) {
            if original.peek() == Some(&character) {
                original.next();
            } else {
                added = true;
            }
        }
        added && original.peek().is_none()
    }
}

struct ProbeOffset {
    x: f32,
    y: f32,
    local_x: f32,
    local_y: f32,
}

impl ProbeOffset {
    fn new(ctm: Matrix, slot: usize, direction: f32) -> Option<Self> {
        let x = direction * (slot % 64 + 1) as f32 * 32.0;
        let y = direction * (slot / 64 + 1) as f32 * 32.0;
        let Matrix { a, b, c, d, .. } = ctm;
        let determinant = a * d - b * c;
        let local_x = (d * x - c * y) / determinant;
        let local_y = (a * y - b * x) / determinant;
        if !local_x.is_finite() || !local_y.is_finite() {
            return None;
        }
        let projected = Matrix {
            e: local_x,
            f: local_y,
            ..Matrix::default()
        }
        .multiply(&ctm);
        let x = projected.e - ctm.e;
        let y = projected.f - ctm.f;
        (x.is_finite() && y.is_finite() && x.abs() > 16.0 && y.abs() > 16.0).then_some(Self {
            x,
            y,
            local_x,
            local_y,
        })
    }
}

fn authored_spans<'a>(
    pdf: &PdfDocument,
    page: usize,
    data: &[u8],
    segments: &'a [Segment],
    properties: &[Object],
    fonts: &SourceFonts,
    inspect_ink: bool,
) -> Option<(Vec<ShowFragments>, Vec<(usize, &'a Show)>)> {
    let resources = pdf.get_page_resources(page).ok()?;
    let named = resources
        .as_dict()?
        .get("Properties")
        .map(|value| pdf.resolve_object(value))
        .transpose()
        .ok()?;
    let mut used_marks = Vec::new();
    for value in properties {
        let fields = match value.as_name() {
            Some(name) => pdf
                .resolve_object(named.as_ref()?.as_dict()?.get(name)?)
                .ok()?,
            None => pdf.resolve_object(value).ok()?,
        };
        if let Some(mark) = fields
            .as_dict()
            .and_then(|fields| fields.get("MCID"))
            .and_then(|value| pdf.resolve_object(value).ok())
            .and_then(|value| value.as_integer())
            .and_then(|value| u32::try_from(value).ok())
        {
            used_marks.push(mark);
        }
    }
    let mark_count = u32::try_from(
        segments
            .iter()
            .map(|segment| segment.shows.len())
            .sum::<usize>(),
    )
    .ok()?;
    used_marks.sort_unstable();
    let first_mark = [FIRST_MARK, 0].into_iter().find_map(|start| {
        let mut next = start;
        for mark in used_marks.iter().copied().filter(|&mark| mark >= start) {
            if mark >= next.checked_add(mark_count)? {
                break;
            }
            next = next.max(mark.checked_add(1)?);
        }
        next.checked_add(mark_count).map(|_| next)
    })?;
    let owners = segments
        .iter()
        .enumerate()
        .flat_map(|(index, segment)| segment.shows.iter().map(move |show| (index, show)))
        .collect::<Vec<_>>();
    let page_index = u32::try_from(page).ok()?;
    let scope = McidScope::Page(page_index);
    let off_layers = compute_default_off_ocgs(pdf);
    let ink_fonts = if inspect_ink {
        owners
            .iter()
            .map(|(_, show)| {
                let font = fonts.fonts.get(show.style.font?)?;
                (show.render_mode == 0
                    && show.style.supports_paragraph()
                    && font.ink_face.is_some())
                .then_some(font)
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let measure_ink = ink_fonts.iter().any(Option::is_some);
    let make_extractor = || {
        let mut extractor = TextExtractor::new();
        extractor.set_document(pdf);
        extractor.set_page_index(page_index);
        extractor.set_excluded_layers(off_layers.clone());
        pdf.load_fonts_public(&resources, &mut extractor).ok()?;
        extractor.set_resources(resources.clone());
        Some(extractor)
    };
    let extract = |offsets: &[Option<ProbeOffset>]| -> Option<Vec<ShowFragments>> {
        let mut edits = Vec::with_capacity(owners.len() * 2);
        for (index, (_, show)) in owners.iter().enumerate() {
            let range = &show.range;
            let mark = first_mark + index as u32;
            let mut begin = format!("/Span << /MCID {mark} >> BDC");
            let mut end = "EMC".to_owned();
            if let Some(offset) = offsets.get(index).and_then(Option::as_ref) {
                let (x, y) = (offset.local_x, offset.local_y);
                begin = format!("1 0 0 1 {x:.12} {y:.12} cm\n{begin}");
                end.push_str(&format!("\n1 0 0 1 {:.12} {:.12} cm", -x, -y));
            }
            edits.push((range.start..range.start, begin));
            edits.push((range.end..range.end, end));
        }
        let shadow = rewrite_ranges(data, edits).ok()?;
        let mut extractor = make_extractor()?;
        let mut fragments = (0..owners.len())
            .map(|_| ShowFragments::default())
            .collect::<Vec<_>>();
        for mut span in extractor.extract_text_spans(&shadow).ok()? {
            if span.mcid_scope.as_ref() != Some(&scope) || letters(&span.text).next().is_none() {
                continue;
            }
            let Some(index) = span.mcid.and_then(|mark| mark.checked_sub(first_mark)) else {
                continue;
            };
            let index = index as usize;
            let Some(fragment) = fragments.get_mut(index) else {
                continue;
            };
            if let Some(offset) = offsets.get(index).and_then(Option::as_ref) {
                span.bbox.x -= offset.x;
                span.bbox.y -= offset.y;
                for x in &mut span.char_x_offsets {
                    *x -= offset.x;
                }
                fragment.recovered = true;
            }
            fragment.spans.push(span);
        }
        for fragment in &mut fragments {
            fragment.spans.sort_by_key(|span| span.sequence);
        }
        if measure_ink {
            let mut by_show = HashMap::<usize, Vec<TextChar>>::new();
            for glyph in make_extractor()?.extract_owned(&shadow).unwrap_or_default() {
                let Some(index) = glyph.mcid.and_then(|mark| mark.checked_sub(first_mark)) else {
                    continue;
                };
                let index = index as usize;
                if ink_fonts.get(index).is_some_and(Option::is_some) {
                    by_show.entry(index).or_default().push(glyph);
                }
            }
            for (index, mut glyphs) in by_show {
                glyphs.sort_by(|a, b| a.origin_x.total_cmp(&b.origin_x));
                let Some(font) = ink_fonts[index] else {
                    continue;
                };
                fragments[index].ink_bounds =
                    standard_object_bounds(owners[index].1, font, &glyphs).map(
                        |[x0, y0, x1, y1]| {
                            let (x, y) = offsets
                                .get(index)
                                .and_then(Option::as_ref)
                                .map_or((0.0, 0.0), |offset| {
                                    (f64::from(offset.x), f64::from(offset.y))
                                });
                            [x0 - x, y0 - y, x1 - x, y1 - y]
                        },
                    );
            }
        }
        Some(fragments)
    };
    let mut fragments = extract(&[])?;
    // The reader drops overlapping render passes even when their MCIDs differ.
    // Probe absent shows and arrays that may have lost some of their fragments
    // at distinct positions in temporary extraction streams.
    // Moving the CTM leaves the text cursor and line matrices in place.
    for direction in [1.0, -1.0] {
        let mut slot = 0;
        let offsets = fragments
            .iter()
            .zip(&owners)
            .map(|(fragment, (_, show))| {
                if !fragment.spans.is_empty() && !(direction > 0.0 && show.multiple_strings) {
                    return None;
                }
                let offset = ProbeOffset::new(show.ctm, slot, direction);
                slot += 1;
                offset
            })
            .collect::<Vec<_>>();
        if offsets.iter().all(Option::is_none) {
            break;
        }
        for (fragment, mut probe) in fragments.iter_mut().zip(extract(&offsets)?) {
            if probe.recovered && fragment.gains_text(&probe) {
                probe.entire_show_recovered = fragment.spans.is_empty();
                *fragment = probe;
            }
        }
    }
    // PDFium's object text comes from its page text list. It can omit a
    // duplicate even though the object is still present and paints the page.
    let geometry = fragments
        .iter()
        .map(|object| ShowGeometry::from_spans(&object.spans))
        .collect::<Vec<_>>();
    for (index, (_, show)) in owners.iter().enumerate() {
        let Some(current) = &geometry[index] else {
            continue;
        };
        fragments[index].reference_suppressed = (index.saturating_sub(5)..index).any(|earlier| {
            let earlier_show = owners[earlier].1;
            earlier_show.style.size == show.style.size
                && geometry[earlier]
                    .as_ref()
                    .is_some_and(|prior| current.duplicates(prior, show.style.size))
                && same_encoded_text(show, earlier_show, fonts)
        });
    }
    suppress_reference_glyphs(pdf, &owners, &geometry, &mut fragments, fonts);
    Some((fragments, owners))
}

pub(super) fn near(left: Rect, right: Rect) -> bool {
    left.into_iter()
        .zip(right)
        .all(|(x, y)| (x - y).abs() <= POSITION_TOLERANCE)
}

pub(super) fn letters(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars()
        .filter(|c| !c.is_whitespace() && *c != CONTINUATION)
}

pub(super) fn translate(
    pdf: &PdfDocument,
    page: usize,
    data: &[u8],
    following: &[&TextSpan],
    shift: f64,
) -> Option<(Vec<u8>, usize)> {
    let resources = pdf.get_page_resources(page).ok()?;
    let fonts = SourceFonts::read(pdf, &resources);
    let (segments, properties) = segments(data, Some(&fonts))?;
    let (authored, owners) =
        authored_spans(pdf, page, data, &segments, &properties, &fonts, false)?;
    let mut by_row: HashMap<i64, Vec<usize>> = HashMap::new();
    for (index, span) in following.iter().enumerate() {
        by_row
            .entry(bbox(span)[1].floor() as i64)
            .or_default()
            .push(index);
    }
    let mut fragments = vec![Vec::new(); following.len()];
    let mut covered = vec![false; following.len()];
    let mut matched = vec![false; owners.len()];
    for (index, object) in authored.iter().enumerate() {
        if let Some(text) = object
            .reference_text
            .as_deref()
            .filter(|text| !text.is_empty())
        {
            let bounds = union(object.spans.iter().map(bbox));
            let row = bounds[1].floor() as i64;
            let mut parts = (row.saturating_sub(1)..=row.saturating_add(1))
                .flat_map(|row| by_row.get(&row).into_iter().flatten().copied())
                .filter(|&target| {
                    let area = bbox(following[target]);
                    area[0] >= bounds[0] - POSITION_TOLERANCE
                        && area[2] <= bounds[2] + POSITION_TOLERANCE
                        && (area[1] - bounds[1]).abs() <= POSITION_TOLERANCE
                        && (area[3] - bounds[3]).abs() <= POSITION_TOLERANCE
                })
                .collect::<Vec<_>>();
            parts.sort_by_key(|&target| following[target].sequence);
            if text.chars().eq(parts
                .iter()
                .flat_map(|&target| letters(&following[target].text)))
            {
                // Keep the full authored paint when extraction retained just a suffix.
                matched[index] = true;
                for target in parts {
                    covered[target] = true;
                }
                continue;
            }
        }
        let mut fixed = false;
        for span in &object.spans {
            let [left, low, right, high] = bbox(span);
            let row = low.floor() as i64;
            let same_row = |tolerance: f64| {
                (row.saturating_sub(1)..=row.saturating_add(1))
                    .flat_map(|row| by_row.get(&row).into_iter().flatten().copied())
                    .filter(move |&target| {
                        let [_, y0, _, y1] = bbox(following[target]);
                        (y0 - low).abs() <= tolerance && (y1 - high).abs() <= tolerance
                    })
            };
            if object.entire_show_recovered
                && same_row(0.5).any(|target| {
                    let reader = following[target];
                    let [x0, _, x1, _] = bbox(reader);
                    (left - x0).abs() <= 0.5
                        && (right - x1).abs() <= 0.5
                        && letters(&span.text).eq(letters(&reader.text))
                })
            {
                // This paint pass must move with the text the reader kept.
                matched[index] = true;
                continue;
            }
            let mut enclosing = same_row(POSITION_TOLERANCE).filter(|&target| {
                let [x0, _, x1, _] = bbox(following[target]);
                left >= x0 - POSITION_TOLERANCE && right <= x1 + POSITION_TOLERANCE
            });
            if let Some(target) = enclosing.next() {
                if enclosing.next().is_some() {
                    return None;
                }
                fragments[target].push((index, span));
                matched[index] = true;
                continue;
            }
            // A single authored show may be split into several reader runs.
            let mut split = same_row(POSITION_TOLERANCE)
                .filter(|&target| {
                    let [x0, _, x1, _] = bbox(following[target]);
                    x0 >= left - POSITION_TOLERANCE && x1 <= right + POSITION_TOLERANCE
                })
                .collect::<Vec<_>>();
            split.sort_by_key(|&target| following[target].sequence);
            if split.len() > 1
                && near(bbox(span), union(split.iter().map(|&i| bbox(following[i]))))
                && letters(&span.text).eq(split.iter().flat_map(|&i| letters(&following[i].text)))
            {
                matched[index] = true;
                for target in split {
                    covered[target] = true;
                }
            } else {
                fixed = true;
            }
        }
        if fixed && matched[index] {
            return None;
        }
    }
    for ((run, parts), covered) in following.iter().zip(&mut fragments).zip(covered) {
        if covered {
            continue;
        }
        parts.sort_by_key(|(show, span)| (*show, span.sequence));
        if parts.is_empty()
            || !near(bbox(run), union(parts.iter().map(|(_, span)| bbox(span))))
            || !letters(&run.text).eq(parts.iter().flat_map(|(_, span)| letters(&span.text)))
        {
            return None;
        }
    }
    let mut edits = Vec::new();
    let mut offset = 0;
    let mut previous_shift = 0.0;
    for segment in &segments {
        let shows = &matched[offset..][..segment.shows.len()];
        offset += segment.shows.len();
        let relative = matches!(segment.position, Position::Line { .. });
        let next_shift = if shows.contains(&true) {
            if shows.contains(&false) {
                return None;
            }
            shift
        } else if relative && shows.is_empty() {
            // An empty relative line still passes its origin to the next line.
            previous_shift
        } else {
            0.0
        };
        // Relative positions inherit the previous line's translation.
        let delta = if relative {
            next_shift - previous_shift
        } else {
            next_shift
        };
        if delta != 0.0 {
            if !segment.movable {
                return None;
            }
            edits.push(position_edit(data, segment, delta)?);
        }
        previous_shift = next_shift;
    }
    Some((
        rewrite_ranges(data, edits).ok()?,
        matched.into_iter().filter(|moved| *moved).count(),
    ))
}

pub(super) fn moved(following: &[&TextSpan], found: &[TextSpan], shift: f64) -> bool {
    let mut sources: HashMap<&str, Vec<Rect>> = HashMap::new();
    for span in following {
        sources.entry(&span.text).or_default().push(bbox(span));
    }
    let mut placements: HashMap<&str, Vec<Rect>> = HashMap::new();
    for span in found {
        if sources.contains_key(span.text.as_str()) {
            placements.entry(&span.text).or_default().push(bbox(span));
        }
    }
    following.iter().all(|run| {
        let original = bbox(run);
        let expected = shifted(original, shift);
        let output = placements
            .get(run.text.as_str())
            .into_iter()
            .flatten()
            .copied();
        let mut matches = output.clone().filter(|&area| near(area, expected));
        matches.next().is_some()
            && matches.next().is_none()
            && !output.into_iter().any(|area| {
                near(area, original)
                    && !sources[run.text.as_str()]
                        .iter()
                        .any(|&other| near(shifted(other, shift), area))
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_oxide::geometry::Rect as Bounds;

    #[test]
    fn duplicate_show_uses_overlap_and_glyph_advance() {
        let position = |x, y| ShowGeometry {
            bounds: [x, y, x + 30.0, y + 12.0],
            origin: (x, y),
            first_advance: 6.0,
            last_advance: 6.0,
        };
        let prior = position(72.0, 500.0);
        assert!(position(72.7, 499.4).duplicates(&prior, 12.0));
        assert!(!position(77.5, 500.0).duplicates(&prior, 12.0));
        assert!(!position(72.0, 496.0).duplicates(&prior, 12.0));
    }

    #[test]
    fn partial_reference_suppression_preserves_the_authored_show_and_visible_suffix() {
        let show = |ordinal, text: &str| Show {
            ordinal,
            range: 0..0,
            style: ShowStyle {
                axes: [1.0, 0.0, 0.0, 1.0],
                size: 12.0,
                rise: 0.0,
                font: Some(0),
            },
            ctm: Matrix::default(),
            multiple_strings: false,
            encoded_text: vec![text.as_bytes().to_vec()],
            render_mode: 0,
        };
        let span = |text: &str, x| TextSpan {
            text: text.into(),
            bbox: Bounds::new(x, 500.0, text.len() as f32 * 6.0, 12.0),
            ..Default::default()
        };
        let (first, second) = (show(0, "LEFTABC"), show(1, "ABCRIGHT"));
        let mut fragments = [
            ShowFragments {
                spans: vec![span("LEFTABC", 72.0)],
                ..Default::default()
            },
            ShowFragments {
                spans: vec![span("ABCRIGHT", 96.15)],
                ..Default::default()
            },
        ];
        let geometry = fragments
            .each_ref()
            .map(|fragment| ShowGeometry::from_spans(&fragment.spans));
        let fonts = SourceFonts {
            fonts: vec![SourceFont {
                name: "Synthetic".into(),
                resource: None,
                embedded: false,
                shaped: false,
                composite: false,
                encoding: None,
                ink_face: None,
                binding: Object::Null,
            }],
            ..Default::default()
        };
        let pdf = PdfDocument::from_bytes(
            include_bytes!("../../../../samples/rust_pdf_corpus/unsigned.pdf").to_vec(),
        )
        .unwrap();
        suppress_reference_glyphs(
            &pdf,
            &[(0, &first), (1, &second)],
            &geometry,
            &mut fragments,
            &fonts,
        );
        assert!(fragments[0].reference_text.is_none());
        assert_eq!(fragments[1].reference_text.as_deref(), Some("RIGHT"));
        assert!(!fragments[1].reference_suppressed);
        assert_eq!(fragments[1].spans[0].text, "ABCRIGHT");
    }
}
