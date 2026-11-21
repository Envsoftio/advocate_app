//! Paint dispatch and cached geometry for PDF Type 3 glyph descriptions.
use super::assembly::Dictionary;
use super::font_glyphs::Fonts;
use super::source_paint::{union, Bounds, GlyphOutline, State, TextCursor};
use pdf_oxide::{
    content::{parse_content_stream, Matrix, Operator},
    extractors::paths::{FillRule, PathExtractor},
    fonts::FontInfo,
    object::{Object, ObjectRef},
    optional_content::{annotation_is_excluded, resolve_and_check_ocg_excluded},
    PdfDocument,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

type Rect = [f32; 4];
const MAX_GLYPH_DEPTH: usize = 12;
const MAX_GLYPH_STREAMS: usize = 4_096;
const MAX_GLYPH_BYTES: usize = 64 * 1024 * 1024;
const MAX_NESTED_TEXT_OUTLINES: usize = 262_144;

#[derive(Clone)]
pub(super) struct Glyph {
    pub bounds: GlyphBounds,
    pub direct: bool,
    pub colored: bool,
    pub events: Arc<[GlyphPaint]>,
}

#[derive(Clone, Copy)]
pub(super) enum GlyphBounds {
    Finite(Rect),
    Unbounded,
}

#[derive(Clone)]
pub(super) enum GlyphPaint {
    Other(usize),
    Image(Matrix),
    Text(NestedTextPaint),
}

#[derive(Clone)]
pub(super) struct NestedTextPaint {
    outlines: Arc<[GlyphOutline]>,
    stroke: Option<Graphics>,
}

impl NestedTextPaint {
    pub fn bounds(&self, placement: Option<Matrix>) -> Option<Bounds> {
        let mut bounds = None;
        for outline in self.outlines.iter() {
            union(&mut bounds, outline.project(placement)?);
        }
        let [x0, y0, x1, y1] = bounds?;
        if x0 >= x1 || y0 >= y1 {
            return bounds;
        }
        let [x0, y0, x1, y1] = match self.stroke {
            Some(mut state) => {
                if let Some(matrix) = placement {
                    state.matrix = state.matrix.multiply(&matrix);
                }
                state
                    .stroke_bounds([x0 as f32, y0 as f32, x1 as f32, y1 as f32], false)
                    .map(f64::from)
            }
            None => [x0, y0, x1, y1],
        };
        // The device rounds text paint by one unit in the frame in which it
        // draws. This is font space for cached glyphs and page space for
        // nested text replayed by a non-cacheable glyph.
        Some([x0 - 1.0, y0 - 1.0, x1 + 1.0, y1 + 1.0])
    }
}

pub(super) struct Font {
    info: FontInfo,
    resources: Object,
    char_procs: Dictionary,
    base_encoding: Option<String>,
    matrix: Matrix,
    bounds: Option<Rect>,
    glyphs: HashMap<u8, Option<Glyph>>,
}

#[derive(Default)]
pub(super) struct Budget {
    streams: usize,
    bytes: usize,
    nested_text_outlines: usize,
}

impl Font {
    pub fn metrics(&self) -> &FontInfo {
        &self.info
    }

    pub fn load(pdf: &PdfDocument, font: &Object, fallback_resources: &Object) -> Option<Self> {
        let fields = font.as_dict()?;
        let matrix = matrix(pdf, fields.get("FontMatrix")?)?;
        let bounds = pdf.resolve_object(fields.get("FontBBox")?).ok()?;
        let bounds = numbers::<4>(bounds.as_array()?).map(|rect| project(matrix, normalized(rect)));
        let encoding = pdf.resolve_object(fields.get("Encoding")?).ok()?;
        let base_encoding = encoding
            .as_name()
            .or_else(|| encoding.as_dict()?.get("BaseEncoding")?.as_name())
            .map(str::to_owned);
        let char_procs = pdf.resolve_object(fields.get("CharProcs")?).ok()?;
        let resources = match fields.get("Resources") {
            Some(value) => pdf.resolve_object(value).ok()?,
            None => fallback_resources.clone(),
        };
        Some(Self {
            info: FontInfo::from_dict(font, pdf).ok()?,
            resources,
            char_procs: char_procs.as_dict()?.clone(),
            base_encoding,
            matrix,
            bounds,
            glyphs: HashMap::new(),
        })
    }

    pub fn glyph(
        &mut self,
        pdf: &PdfDocument,
        code: u8,
        off_layers: &HashSet<String>,
        budget: &mut Budget,
    ) -> Option<Glyph> {
        if let Some(glyph) = self.glyphs.get(&code) {
            return glyph.clone();
        }
        let glyph = self.inspect_glyph(pdf, code, off_layers, budget);
        self.glyphs.insert(code, glyph.clone());
        glyph
    }

    pub fn character(&self, code: u8) -> Option<char> {
        let value = self.info.char_to_unicode(u32::from(code))?;
        let mut chars = value.chars();
        let character = chars.next()?;
        chars.next().is_none().then_some(character)
    }

    pub fn cached_glyph(&self, code: u8) -> Option<&Glyph> {
        self.glyphs.get(&code)?.as_ref()
    }

    fn inspect_glyph(
        &self,
        pdf: &PdfDocument,
        code: u8,
        off_layers: &HashSet<String>,
        budget: &mut Budget,
    ) -> Option<Glyph> {
        let name = if let Some(name) = self.info.diff_glyph_names.get(&code) {
            Some(name.as_str())
        } else if let Some(encoding) = self.base_encoding.as_deref() {
            // These base encodings share the printable ASCII glyph names with
            // two punctuation exceptions. A Differences entry always wins.
            match (encoding, code) {
                ("WinAnsiEncoding" | "MacRomanEncoding", 39) => Some("quotesingle"),
                ("WinAnsiEncoding" | "MacRomanEncoding", 96) => Some("grave"),
                ("WinAnsiEncoding" | "MacRomanEncoding" | "StandardEncoding", 32..=126) => {
                    FontInfo::gid_to_standard_glyph_name(u16::from(code))
                }
                _ => return None,
            }
        } else {
            None
        };
        let mut trace = Trace::default();
        if let Some(binding) = name.and_then(|name| self.char_procs.get(name)) {
            let mut scan = Scan {
                pdf,
                off_layers,
                budget,
                active: Vec::new(),
                fonts: Fonts::default(),
            };
            scan.stream(
                binding,
                &self.resources,
                Graphics::new(self.matrix),
                &mut trace,
            )?;
        }
        // The reference compares the declared glyph rectangle with the
        // transformed font box, and otherwise measures the glyph display list.
        let trusted = trace
            .declared
            .filter(|glyph| self.bounds.is_some_and(|font| contains(font, *glyph)));
        let bounds = match (trusted, trace.unbounded) {
            (Some(rect), _) => GlyphBounds::Finite(project(self.matrix, rect)),
            (None, true) => GlyphBounds::Unbounded,
            (None, false) => GlyphBounds::Finite(
                trace
                    .bounds
                    .or_else(|| trace.declared.map(|rect| project(self.matrix, rect)))
                    .unwrap_or([0.0; 4]),
            ),
        };
        Some(Glyph {
            bounds,
            direct: trace.direct,
            colored: trace.colored,
            events: Arc::from(trace.events),
        })
    }
}

fn number(value: &Object) -> Option<f32> {
    let number = value
        .as_real()
        .or_else(|| value.as_integer().map(|v| v as f64))? as f32;
    number.is_finite().then_some(number)
}

fn numbers<const N: usize>(values: &[Object]) -> Option<[f32; N]> {
    let values: &[Object; N] = values.try_into().ok()?;
    let mut output = [0.0; N];
    for (slot, value) in output.iter_mut().zip(values) {
        *slot = number(value)?;
    }
    Some(output)
}

fn matrix(pdf: &PdfDocument, value: &Object) -> Option<Matrix> {
    let object = pdf.resolve_object(value).ok()?;
    let [a, b, c, d, e, f] = numbers(object.as_array()?)?;
    Some(Matrix { a, b, c, d, e, f })
}

fn normalized([x0, y0, x1, y1]: Rect) -> Rect {
    [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]
}

fn project(matrix: Matrix, rect: Rect) -> Rect {
    let [x0, y0, x1, y1] = super::painted_box(matrix, rect).unwrap_or([0.0; 4]);
    [x0 as f32, y0 as f32, x1 as f32, y1 as f32]
}

fn contains(outer: Rect, inner: Rect) -> bool {
    outer[0] < outer[2]
        && outer[1] < outer[3]
        && outer[0] <= inner[0]
        && outer[1] <= inner[1]
        && outer[2] >= inner[2]
        && outer[3] >= inner[3]
}

fn clip(bounds: Rect, clipping: Option<Rect>) -> Rect {
    clipping.map_or(bounds, |[x0, y0, x1, y1]| {
        [
            bounds[0].max(x0),
            bounds[1].max(y0),
            bounds[2].min(x1),
            bounds[3].min(y1),
        ]
    })
}

#[derive(Default)]
struct Trace {
    colored: bool,
    declared: Option<Rect>,
    direct: bool,
    events: Vec<GlyphPaint>,
    bounds: Option<Rect>,
    unbounded: bool,
    defined: u8,
}

impl Trace {
    fn other(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        match self.events.last_mut() {
            Some(GlyphPaint::Other(previous)) => *previous += count,
            _ => self.events.push(GlyphPaint::Other(count)),
        }
    }

    fn add(&mut self, bounds: Rect, clipping: Option<Rect>) {
        let [x0, y0, x1, y1] = clip(bounds, clipping);
        if x0 < x1 && y0 < y1 {
            self.bounds = Some(self.bounds.map_or([x0, y0, x1, y1], |old| {
                [
                    old[0].min(x0),
                    old[1].min(y0),
                    old[2].max(x1),
                    old[3].max(y1),
                ]
            }));
        }
    }

    fn add_shade(&mut self, bounds: Option<Rect>, state: Graphics) {
        if let Some(bounds) = bounds {
            if bounds[0] < bounds[2] && bounds[1] < bounds[3] {
                self.add(project(state.matrix, bounds), state.clip);
            }
        } else if let Some(clipping) = state.clip {
            self.add(clipping, None);
        } else {
            self.unbounded = true;
        }
        self.other(1);
    }
}

struct PendingText {
    outlines: Vec<GlyphOutline>,
    render: u8,
    graphics: Graphics,
}

impl PendingText {
    fn flush(self, state: &mut Graphics, trace: &mut Trace) {
        if self.render == 3 {
            trace.other(1);
            return;
        }
        let fill = NestedTextPaint {
            outlines: Arc::from(self.outlines),
            stroke: None,
        };
        let render_fill = matches!(self.render, 0 | 2 | 4 | 6);
        let render_stroke = matches!(self.render, 1 | 2 | 5 | 6);
        let fill_bounds = fill.bounds(None).map(|bounds| bounds.map(|p| p as f32));
        if let Some(bounds) = fill_bounds.filter(|_| render_fill) {
            trace.add(bounds, self.graphics.clip);
        }
        if render_fill {
            trace.events.push(GlyphPaint::Text(fill.clone()));
        }
        if render_stroke {
            let stroke = NestedTextPaint {
                stroke: Some(self.graphics),
                ..fill
            };
            if let Some(bounds) = stroke.bounds(None) {
                trace.add(bounds.map(|p| p as f32), self.graphics.clip);
            }
            trace.events.push(GlyphPaint::Text(stroke));
        }
        if self.render >= 4 {
            state.clip = Some(clip(fill_bounds.unwrap_or([0.0; 4]), state.clip));
        }
    }
}

const FILL: u8 = 1;
const STROKE: u8 = 2;
const WIDTH: u8 = 4;
const JOIN: u8 = 8;
const MITER: u8 = 16;
const DASH: u8 = 32;
const CAP: u8 = 64;

#[derive(Clone, Copy)]
struct Graphics {
    matrix: Matrix,
    clip: Option<Rect>,
    width: f32,
    join: u8,
    miter: f32,
    dash: bool,
    text: State,
}

impl Graphics {
    fn new(matrix: Matrix) -> Self {
        Self {
            matrix,
            clip: None,
            width: 1.0,
            join: 0,
            miter: 10.0,
            dash: false,
            text: State::default(),
        }
    }

    fn stroke_bounds(self, [x0, y0, x1, y1]: Rect, rectangle: bool) -> Rect {
        let width = if self.width == 0.0 {
            1.0
        } else {
            self.width.abs()
        };
        let miter = if !rectangle && (x0 < x1 && y0 < y1) && self.join == 0 {
            self.miter.max(0.5) * 2.0
        } else {
            1.0
        };
        let Matrix { a, b, c, d, .. } = self.matrix;
        let margin = a.abs().max(b.abs()).max(c.abs()).max(d.abs()) * miter * width / 2.0;
        [x0 - margin, y0 - margin, x1 + margin, y1 + margin]
    }
}

struct Scan<'a> {
    pdf: &'a PdfDocument,
    off_layers: &'a HashSet<String>,
    budget: &'a mut Budget,
    active: Vec<ObjectRef>,
    fonts: Fonts,
}

impl Scan<'_> {
    fn stream(
        &mut self,
        binding: &Object,
        resources: &Object,
        mut state: Graphics,
        trace: &mut Trace,
    ) -> Option<()> {
        if self.active.len() >= MAX_GLYPH_DEPTH || self.budget.streams >= MAX_GLYPH_STREAMS {
            return None;
        }
        self.budget.streams += 1;
        let object = self.pdf.resolve_object(binding).ok()?;
        let data = object.decode_stream_data().ok()?;
        self.budget.bytes += data.len();
        if self.budget.bytes > MAX_GLYPH_BYTES {
            return None;
        }
        let mut path = PathExtractor::new();
        let mut rectangle = true;
        let mut pending_clip = false;
        let mut stack = Vec::new();
        let mut marked = Vec::new();
        let mut hidden = 0;
        let mut cursor = TextCursor::default();
        let mut text: Option<PendingText> = None;
        for op in parse_content_stream(&data).ok()? {
            path.set_ctm(state.matrix);
            state.text.ctm = state.matrix;
            let show = matches!(
                op,
                Operator::Tj { .. }
                    | Operator::TJ { .. }
                    | Operator::Quote { .. }
                    | Operator::DoubleQuote { .. }
            );
            let flush = state.text.apply(&op, self.pdf, resources, &mut self.fonts)
                || matches!(
                    op,
                    Operator::SaveState | Operator::RestoreState | Operator::Do { .. }
                )
                || (show
                    && text
                        .as_ref()
                        .is_some_and(|text| text.render != state.text.render));
            if flush {
                if let Some(pending) = text.take() {
                    pending.flush(&mut state, trace);
                }
            }
            cursor.position(&op, state.text);
            match op {
                Operator::Other { ref name, .. } if name == "d0" => trace.colored = true,
                Operator::Other {
                    ref name,
                    ref operands,
                } if name == "d1" => {
                    let [_, _, x0, y0, x1, y1] = numbers(operands)?;
                    trace.declared = Some(normalized([x0, y0, x1, y1]));
                    trace.defined |= FILL | STROKE;
                }
                Operator::SaveState => stack.push(state),
                Operator::RestoreState => state = stack.pop().unwrap_or(state),
                Operator::Cm { a, b, c, d, e, f } => {
                    state.matrix = Matrix { a, b, c, d, e, f }.multiply(&state.matrix)
                }
                Operator::SetLineWidth { width } => {
                    state.width = width;
                    trace.defined |= WIDTH;
                }
                Operator::SetLineJoin { join_style } => {
                    state.join = join_style;
                    trace.defined |= JOIN;
                }
                Operator::SetMiterLimit { limit } => {
                    state.miter = limit;
                    trace.defined |= MITER;
                }
                Operator::SetDash { ref array, .. } => {
                    state.dash = !array.is_empty();
                    trace.defined |= DASH;
                }
                Operator::SetLineCap { .. } => trace.defined |= CAP,
                Operator::SetExtGState { ref dict_name } => {
                    self.graphics(dict_name, resources, &mut state, trace)?
                }
                _ if show => {
                    if hidden > 0 {
                        cursor.show(&op, state.text, &self.fonts, false);
                        continue;
                    }
                    let pending = text.get_or_insert_with(|| PendingText {
                        outlines: Vec::new(),
                        render: state.text.render,
                        graphics: state,
                    });
                    if pending.render == 3 {
                        cursor.show(&op, state.text, &self.fonts, false);
                        continue;
                    }
                    let limit = MAX_NESTED_TEXT_OUTLINES - self.budget.nested_text_outlines;
                    let outlines = cursor.nested_outlines(&op, state.text, &self.fonts, limit)?;
                    self.budget.nested_text_outlines += outlines.len();
                    pending.outlines.extend(outlines);
                }
                Operator::BeginText
                | Operator::EndText
                | Operator::Tf { .. }
                | Operator::Tr { .. }
                | Operator::Tz { .. }
                | Operator::Ts { .. }
                | Operator::Tc { .. }
                | Operator::Tw { .. }
                | Operator::TL { .. }
                | Operator::Tm { .. }
                | Operator::Td { .. }
                | Operator::TD { .. }
                | Operator::TStar => {}
                Operator::SetFillGray { .. }
                | Operator::SetFillRgb { .. }
                | Operator::SetFillCmyk { .. }
                | Operator::SetFillColor { .. } => trace.defined |= FILL,
                Operator::SetStrokeGray { .. }
                | Operator::SetStrokeRgb { .. }
                | Operator::SetStrokeCmyk { .. }
                | Operator::SetStrokeColor { .. } => trace.defined |= STROKE,
                Operator::SetFillColorSpace { ref name } if name != "Pattern" => {
                    trace.defined |= FILL
                }
                Operator::SetStrokeColorSpace { ref name } if name != "Pattern" => {
                    trace.defined |= STROKE
                }
                Operator::BeginMarkedContentDict {
                    ref tag,
                    ref properties,
                } => {
                    let excluded = tag == "OC"
                        && resolve_and_check_ocg_excluded(
                            properties,
                            Some(resources),
                            Some(self.pdf),
                            self.off_layers,
                        );
                    hidden += usize::from(excluded);
                    marked.push(excluded);
                }
                Operator::BeginMarkedContent { .. } => marked.push(false),
                Operator::EndMarkedContent => hidden -= usize::from(marked.pop().unwrap_or(false)),
                Operator::MoveTo { x, y } => {
                    rectangle = false;
                    path.move_to(x, y);
                }
                Operator::LineTo { x, y } => path.line_to(x, y),
                Operator::CurveTo {
                    x1,
                    y1,
                    x2,
                    y2,
                    x3,
                    y3,
                } => path.curve_to(x1, y1, x2, y2, x3, y3),
                Operator::CurveToV { x2, y2, x3, y3 } => path.curve_to_v(x2, y2, x3, y3),
                Operator::CurveToY { x1, y1, x3, y3 } => path.curve_to_y(x1, y1, x3, y3),
                Operator::ClosePath => path.close_path(),
                Operator::Rectangle {
                    x,
                    y,
                    width,
                    height,
                } => {
                    path.move_to(x, y);
                    path.line_to(x + width, y);
                    path.line_to(x + width, y + height);
                    path.line_to(x, y + height);
                    path.close_path();
                }
                Operator::ClipNonZero | Operator::ClipEvenOdd => pending_clip = true,
                Operator::Fill
                | Operator::FillEvenOdd
                | Operator::Stroke
                | Operator::CloseAndStroke
                | Operator::FillStroke
                | Operator::FillStrokeEvenOdd
                | Operator::CloseFillStroke
                | Operator::CloseFillStrokeEvenOdd
                | Operator::EndPath => {
                    let fill = !matches!(
                        op,
                        Operator::EndPath | Operator::Stroke | Operator::CloseAndStroke
                    );
                    let stroke = !matches!(
                        op,
                        Operator::EndPath | Operator::Fill | Operator::FillEvenOdd
                    );
                    let required_stroke = STROKE
                        | WIDTH
                        | JOIN
                        | DASH
                        | if state.dash { CAP } else { 0 }
                        | if state.join == 0 { MITER } else { 0 };
                    trace.direct |= (fill && trace.defined & FILL == 0)
                        || (stroke && trace.defined & required_stroke != required_stroke);
                    path.fill(FillRule::NonZero);
                    let mut finished = std::mem::replace(&mut path, PathExtractor::new()).finish();
                    if let Some(finished) = finished.pop() {
                        let bounds = finished.bbox;
                        let rect = [
                            bounds.x,
                            bounds.y,
                            bounds.x + bounds.width,
                            bounds.y + bounds.height,
                        ];
                        if hidden == 0 {
                            if fill {
                                trace.add(rect, state.clip);
                            }
                            if stroke {
                                trace.add(state.stroke_bounds(rect, rectangle), state.clip);
                            }
                            trace.other(usize::from(fill) + usize::from(stroke));
                        }
                        if pending_clip {
                            state.clip = Some(clip(rect, state.clip));
                        }
                    }
                    rectangle = true;
                    pending_clip = false;
                }
                Operator::Do { ref name } if hidden == 0 => {
                    self.xobject(name, resources, state, trace)?
                }
                Operator::InlineImage { ref dict, .. } if hidden == 0 => {
                    Self::image(dict, state, trace)
                }
                Operator::PaintShading { ref name } if hidden == 0 => {
                    self.shading(name, resources, state, trace)?
                }
                Operator::Do { .. }
                | Operator::InlineImage { .. }
                | Operator::PaintShading { .. } => {}
                Operator::Other { ref name, .. }
                    if matches!(name.as_str(), "BX" | "EX" | "MP" | "DP") => {}
                _ => return None,
            }
        }
        if let Some(pending) = text {
            pending.flush(&mut state, trace);
        }
        Some(())
    }

    fn resource(&self, resources: &Object, category: &str, name: &str) -> Option<Object> {
        let entries = self
            .pdf
            .resolve_object(resources.as_dict()?.get(category)?)
            .ok()?;
        self.pdf.resolve_object(entries.as_dict()?.get(name)?).ok()
    }

    fn graphics(
        &self,
        name: &str,
        resources: &Object,
        state: &mut Graphics,
        trace: &mut Trace,
    ) -> Option<()> {
        let Some(object) = self.resource(resources, "ExtGState", name) else {
            return Some(());
        };
        let fields = object.as_dict()?;
        let get = |key| {
            fields
                .get(key)
                .and_then(|value| self.pdf.resolve_object(value).ok())
        };
        if let Some(width) = get("LW").as_ref().and_then(number) {
            state.width = width;
            trace.defined |= WIDTH;
        }
        if let Some(join) = get("LJ").as_ref().and_then(Object::as_integer) {
            state.join = join as u8;
            trace.defined |= JOIN;
        }
        if let Some(limit) = get("ML").as_ref().and_then(number) {
            state.miter = limit;
            trace.defined |= MITER;
        }
        if get("LC").as_ref().and_then(Object::as_integer).is_some() {
            trace.defined |= CAP;
        }
        if let Some(dash) = get("D")
            .as_ref()
            .and_then(Object::as_array)
            .and_then(|array| array.first())
            .and_then(|array| self.pdf.resolve_object(array).ok())
        {
            state.dash = !dash.as_array()?.is_empty();
            trace.defined |= DASH;
        }
        // This is a geometry trace. Blend modes and constant opacity do not
        // change the paint bounds. An active soft mask has its own geometry.
        if get("SMask").is_some_and(|mask| mask.as_dict().is_some()) {
            return None;
        }
        Some(())
    }

    fn shading(
        &self,
        name: &str,
        resources: &Object,
        state: Graphics,
        trace: &mut Trace,
    ) -> Option<()> {
        let Some(object) = self.resource(resources, "Shading", name) else {
            return Some(());
        };
        let fields = object.as_dict()?;
        let get = |key| {
            fields
                .get(key)
                .and_then(|value| self.pdf.resolve_object(value).ok())
        };
        let kind = get("ShadingType")?.as_integer()?;
        get("ColorSpace")?;
        if kind <= 3 {
            get("Function")?;
        }
        let bbox = get("BBox")
            .as_ref()
            .and_then(Object::as_array)
            .and_then(|values| numbers(values))
            .map(normalized);
        let shading_bounds = match kind {
            1 => {
                let [x0, x1, y0, y1] = get("Domain")
                    .as_ref()
                    .and_then(Object::as_array)
                    .and_then(|values| numbers(values))
                    .unwrap_or([0.0, 1.0, 0.0, 1.0]);
                let local = fields
                    .get("Matrix")
                    .map_or(Some(Matrix::identity()), |value| matrix(self.pdf, value))?;
                Some(clip(project(local, normalized([x0, y0, x1, y1])), bbox))
            }
            2 | 3 => bbox,
            4..=7 => {
                let decode = get("Decode")?;
                let [x0, x1, y0, y1] = numbers(decode.as_array()?.get(..4)?)?;
                Some(clip(normalized([x0, y0, x1, y1]), bbox))
            }
            _ => return None,
        };
        trace.add_shade(shading_bounds, state);
        Some(())
    }

    fn image(fields: &Dictionary, state: Graphics, trace: &mut Trace) {
        let mask = fields
            .get("ImageMask")
            .or_else(|| fields.get("IM"))
            .and_then(Object::as_bool)
            == Some(true);
        if mask {
            trace.other(1);
        } else {
            trace.events.push(GlyphPaint::Image(state.matrix));
        }
        trace.add(project(state.matrix, [0.0, 0.0, 1.0, 1.0]), state.clip);
    }

    fn xobject(
        &mut self,
        name: &str,
        resources: &Object,
        mut state: Graphics,
        trace: &mut Trace,
    ) -> Option<()> {
        let objects = self
            .pdf
            .resolve_object(resources.as_dict()?.get("XObject")?)
            .ok()?;
        let binding = objects.as_dict()?.get(name)?;
        let object = self.pdf.resolve_object(binding).ok()?;
        let fields = object.as_dict()?;
        if fields
            .get("OC")
            .is_some_and(|oc| annotation_is_excluded(oc, self.pdf, self.off_layers))
        {
            return Some(());
        }
        match fields.get("Subtype")?.as_name()? {
            "Image" => Self::image(fields, state, trace),
            "Form" => {
                let reference = binding.as_reference()?;
                if self.active.contains(&reference) {
                    return None;
                }
                if let Some(value) = fields.get("Matrix") {
                    state.matrix = matrix(self.pdf, value)?.multiply(&state.matrix);
                }
                if let Some(value) = fields.get("BBox") {
                    let bbox = self.pdf.resolve_object(value).ok()?;
                    let bbox = project(state.matrix, normalized(numbers(bbox.as_array()?)?));
                    state.clip = Some(clip(bbox, state.clip));
                }
                let own = fields
                    .get("Resources")
                    .map(|value| self.pdf.resolve_object(value))
                    .transpose()
                    .ok()?;
                self.active.push(reference);
                let result = self.stream(
                    binding,
                    own.as_ref()
                        .filter(|value| !matches!(value, Object::Null))
                        .unwrap_or(resources),
                    state,
                    trace,
                );
                self.active.pop();
                result?;
            }
            _ => return None,
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{assembly, overlay_working_text, visible_text};
    use super::*;
    use std::fmt::Write as _;

    const OVERLAP: &str = "Text overlaps existing page content. Choose a clear area.";

    fn pdf_with_glyph(glyph: &str) -> Vec<u8> {
        let stream = |content: &str| {
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            )
        };
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 580 620] /Contents 4 0 R \
                /Resources << /Font << /T3 5 0 R >> >> >>"
                .to_owned(),
            stream("BT /T3 24 Tf 1 0 0 1 230 315 Tm (A) Tj ET"),
            "<< /Type /Font /Subtype /Type3 /Name /SyntheticType3 /FontBBox [-80 -100 620 820] \
                /FontMatrix [.001 0 0 .001 0 0] /FirstChar 65 /LastChar 65 /Widths [620] \
                /Encoding << /Type /Encoding /Differences [65 /A] >> /CharProcs << /A 6 0 R >> \
                /Resources << /Font << /Nested 7 0 R >> >> >>"
                .to_owned(),
            stream(glyph),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .to_owned(),
        ];
        let mut bytes = "%PDF-1.7\n".to_owned();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            writeln!(bytes, "{} 0 obj\n{object}\nendobj", index + 1).unwrap();
        }
        let xref = bytes.len();
        bytes.push_str("xref\n0 8\n0000000000 65535 f \n");
        for offset in offsets {
            writeln!(bytes, "{offset:010} 00000 n ").unwrap();
        }
        writeln!(
            bytes,
            "trailer\n<< /Size 8 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF"
        )
        .unwrap();
        bytes.into_bytes()
    }

    #[test]
    fn nested_type3_text_uses_cached_and_direct_paint_bounds() {
        let text = "620 0 d0 BT /Nested 240 Tf 1 0 0 1 100 130 Tm (A) Tj ET";
        let direct = format!("{text} 130 105 135 165 re f");
        // Reference paint-trace text bounds expressed in PDF user space.
        for (program, expected, distant_refusal) in [
            (text, [207.498, 293.12, 261.161, 347.319], true),
            (direct.as_str(), [231.498, 317.12, 237.161, 323.319], false),
        ] {
            let bytes = pdf_with_glyph(program);
            let pdf = PdfDocument::from_bytes(bytes.clone()).unwrap();
            let geometry = assembly::geometry(&pdf, 0).unwrap();
            let paint = visible_text::page_paints(&pdf, 0, geometry).unwrap();
            assert_eq!(paint.text.len(), 1, "{:?}", paint.text);
            for (actual, expected) in paint.text[0].into_iter().zip(expected) {
                assert!((actual - expected).abs() < 0.02, "{actual} vs {expected}");
            }
            let place =
                |x, y| overlay_working_text(bytes.clone(), 0, x / 580.0, y / 620.0, "I", 10.0, &[]);
            assert_eq!(place(231.0, 292.0).unwrap_err(), OVERLAP);
            assert_eq!(place(220.0, 280.0).is_err(), distant_refusal);
            let revised = PdfDocument::from_bytes(place(220.0, 331.0).unwrap()).unwrap();
            let resources = revised.get_page_resources(0).unwrap();
            let fonts = revised
                .resolve_object(&resources.as_dict().unwrap()["Font"])
                .unwrap();
            let font = revised
                .resolve_object(&fonts.as_dict().unwrap()["T3"])
                .unwrap();
            let procedures = revised
                .resolve_object(&font.as_dict().unwrap()["CharProcs"])
                .unwrap();
            let retained = revised
                .resolve_object(&procedures.as_dict().unwrap()["A"])
                .unwrap();
            assert_eq!(retained.decode_stream_data().unwrap(), program.as_bytes());
        }
    }
}
