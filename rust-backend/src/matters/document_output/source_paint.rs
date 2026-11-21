//! Text paint metadata from the source operators, independent of extraction order.
use super::assembly::Geometry;
use super::font_glyphs::{Font, Fonts, GlyphAdvance, Glyphs};
use pdf_oxide::{
    content::{parse_content_stream, Matrix, Operator, TextElement},
    layout::{TextChar, TextSpan},
    object::Object,
    PdfDocument,
};
use std::collections::HashMap;

pub(super) type Bounds = [f64; 4];
pub(super) const FIRST_SHOW: u32 = 1_000_000_000;

#[derive(Clone, Copy)]
pub(super) struct State {
    pub ctm: Matrix,
    pub render: u8,
    pub fill_pattern: bool,
    pub soft_mask: Option<usize>,
    fill_alpha: f32,
    width: f32,
    join: u8,
    miter: f32,
    font: Option<Font>,
    size: f32,
    scale: f32,
    rise: f32,
    char_space: f32,
    word_space: f32,
    leading: f32,
}

impl Default for State {
    fn default() -> Self {
        Self {
            ctm: Matrix::identity(),
            render: 0,
            fill_pattern: false,
            soft_mask: None,
            fill_alpha: 1.0,
            width: 1.0,
            join: 0,
            miter: 10.0,
            font: None,
            size: 0.0,
            scale: 1.0,
            rise: 0.0,
            char_space: 0.0,
            word_space: 0.0,
            leading: 0.0,
        }
    }
}

fn resource(pdf: &PdfDocument, resources: &Object, category: &str, name: &str) -> Option<Object> {
    let dict = pdf
        .resolve_object(resources.as_dict()?.get(category)?)
        .ok()?;
    dict.as_dict()?.get(name).cloned()
}

fn number(value: Option<&Object>) -> Option<f32> {
    match value? {
        Object::Integer(value) => Some(*value as f32),
        Object::Real(value) => Some(*value as f32),
        _ => None,
    }
}

impl State {
    pub fn at_soft_mask(mut self, placement: Matrix) -> Self {
        self.ctm = placement;
        self.soft_mask = None;
        self.fill_alpha = 1.0;
        self
    }

    // True when the paint device can have emitted a pending text batch.
    pub fn apply(
        &mut self,
        op: &Operator,
        pdf: &PdfDocument,
        resources: &Object,
        fonts: &mut Fonts,
    ) -> bool {
        match op {
            Operator::Cm { a, b, c, d, e, f } => {
                self.ctm = Matrix {
                    a: *a,
                    b: *b,
                    c: *c,
                    d: *d,
                    e: *e,
                    f: *f,
                }
                .multiply(&self.ctm);
            }
            Operator::Tf { font, size } => {
                self.size = *size;
                self.font = resource(pdf, resources, "Font", font)
                    .and_then(|object| fonts.select(pdf, &object, resources));
                return false;
            }
            Operator::Tr { render } => {
                self.render = *render;
                return false;
            }
            Operator::Tz { scale } => {
                self.scale = *scale / 100.0;
                return false;
            }
            Operator::Ts { rise } => {
                self.rise = *rise;
                return false;
            }
            Operator::Tc { char_space } => {
                self.char_space = *char_space;
                return false;
            }
            Operator::Tw { word_space } => {
                self.word_space = *word_space;
                return false;
            }
            Operator::TL { leading } => {
                self.leading = *leading;
                return false;
            }
            Operator::TD { ty, .. } => {
                self.leading = -*ty;
                return false;
            }
            Operator::DoubleQuote {
                char_space,
                word_space,
                ..
            } => {
                self.char_space = *char_space;
                self.word_space = *word_space;
                return false;
            }
            Operator::SetLineWidth { width } => self.width = *width,
            Operator::SetLineJoin { join_style } => self.join = *join_style,
            Operator::SetMiterLimit { limit } => self.miter = *limit,
            Operator::SetExtGState { dict_name } => {
                if let Some(object) = resource(pdf, resources, "ExtGState", dict_name)
                    .and_then(|binding| pdf.resolve_object(&binding).ok())
                {
                    if let Some(fields) = object.as_dict() {
                        if let Some(alpha) = fields
                            .get("ca")
                            .and_then(|value| pdf.resolve_object(value).ok())
                        {
                            self.fill_alpha = number(Some(&alpha)).unwrap_or(self.fill_alpha);
                        }
                        self.width = number(fields.get("LW")).unwrap_or(self.width);
                        self.join = fields
                            .get("LJ")
                            .and_then(Object::as_integer)
                            .map_or(self.join, |join| join as u8);
                        self.miter = number(fields.get("ML")).unwrap_or(self.miter);
                        if let Some(font) = fields.get("Font").and_then(Object::as_array) {
                            if let [font, size] = font.as_slice() {
                                self.font = fonts.select(pdf, font, resources);
                                self.size = number(Some(size)).unwrap_or(0.0);
                            }
                        }
                    }
                }
            }
            Operator::SetFillGray { .. }
            | Operator::SetFillRgb { .. }
            | Operator::SetFillCmyk { .. } => {
                self.fill_pattern = false;
            }
            Operator::SetFillColorSpace { name } => {
                self.fill_pattern = name == "Pattern";
            }
            Operator::BeginText
            | Operator::EndText
            | Operator::SetStrokeRgb { .. }
            | Operator::SetStrokeGray { .. }
            | Operator::SetStrokeCmyk { .. }
            | Operator::SetStrokeColorSpace { .. }
            | Operator::SetFillColor { .. }
            | Operator::SetStrokeColor { .. }
            | Operator::SetFillColorN { .. }
            | Operator::SetStrokeColorN { .. }
            | Operator::InlineImage { .. }
            | Operator::PaintShading { .. }
            | Operator::SetLineCap { .. }
            | Operator::SetDash { .. }
            | Operator::BeginMarkedContent { .. }
            | Operator::BeginMarkedContentDict { .. }
            | Operator::EndMarkedContent => {}
            _ => return false,
        }
        true
    }

    pub fn type3(self) -> Option<usize> {
        match self.font {
            Some(Font::Type3(index)) => Some(index),
            _ => None,
        }
    }

    pub fn paragraph_fill(self) -> bool {
        // The reference trial reads alpha through PDFium's 8-bit color API.
        self.render == 0 && (self.fill_alpha * 255.0 + 0.5).trunc() >= 255.0
    }

    pub fn paint(self) -> Paint {
        let stroke = matches!(self.render, 1 | 2 | 5 | 6);
        let width = if self.width == 0.0 {
            1.0
        } else {
            self.width.abs()
        };
        Paint {
            render: self.render,
            paragraph_fill: self.paragraph_fill(),
            measured: None,
            stroke: if stroke { width / 2.0 } else { 0.0 },
            miter: if stroke && self.join == 0 {
                self.miter.max(0.5) * 2.0
            } else {
                1.0
            },
            scale: [self.ctm.a, self.ctm.b, self.ctm.c, self.ctm.d]
                .into_iter()
                .map(f32::abs)
                .fold(0.0, f32::max),
        }
    }

    pub fn show(
        self,
        bytes: &[u8],
        paint: Option<usize>,
        text_axes: Matrix,
        fonts: &Fonts,
    ) -> Show {
        Show {
            paint,
            glyphs: paint
                .and(self.font)
                .and_then(|font| fonts.measure(font, bytes)),
            size: self.size,
            scale: self.scale,
            rise: self.rise,
            transform: text_axes.multiply(&self.ctm),
        }
    }
}

/// The text matrix advances in source-code order. Extracted Unicode can merge,
/// expand, reorder or omit glyphs, so it cannot supply these advances.
#[derive(Default)]
pub(super) struct TextCursor {
    current: Option<Matrix>,
    line: Matrix,
}

#[derive(Clone, Copy)]
pub(super) struct GlyphOutline {
    bounds: [f32; 4],
    state: Matrix,
    size: f32,
    scale: f32,
    rise: f32,
}

impl GlyphOutline {
    pub fn project(self, placement: Option<Matrix>) -> Option<Bounds> {
        let matrix = placement.map_or(self.state, |matrix| self.state.multiply(&matrix));
        outline_bounds(
            self.bounds,
            self.size,
            self.scale,
            self.rise,
            matrix,
            [matrix.e, matrix.f],
        )
    }
}

impl TextCursor {
    pub fn position(&mut self, op: &Operator, state: State) {
        let line = match *op {
            Operator::BeginText => Matrix::identity(),
            Operator::Tm { a, b, c, d, e, f } => Matrix { a, b, c, d, e, f },
            Operator::Td { tx, ty } | Operator::TD { tx, ty } => {
                Matrix::translation(tx, ty).multiply(&self.line)
            }
            Operator::TStar | Operator::Quote { .. } | Operator::DoubleQuote { .. } => {
                Matrix::translation(0.0, -state.leading).multiply(&self.line)
            }
            _ => return,
        };
        self.line = line;
        self.current = Some(line);
    }

    /// `Some(None)` records a fully decoded show with no painted glyphs.
    pub fn show(
        &mut self,
        op: &Operator,
        state: State,
        fonts: &Fonts,
        visible: bool,
    ) -> Option<Option<Bounds>> {
        let mut bounds = None;
        self.visit_show(op, state, fonts, visible, false, &mut |glyph| {
            union(&mut bounds, glyph.project(None)?);
            Some(())
        })?;
        Some(bounds)
    }

    pub fn nested_outlines(
        &mut self,
        op: &Operator,
        state: State,
        fonts: &Fonts,
        limit: usize,
    ) -> Option<Vec<GlyphOutline>> {
        let mut outlines = Vec::new();
        self.visit_show(op, state, fonts, true, true, &mut |glyph| {
            if outlines.len() >= limit {
                return None;
            }
            outlines.push(glyph);
            Some(())
        })?;
        Some(outlines)
    }

    fn visit_show(
        &mut self,
        op: &Operator,
        state: State,
        fonts: &Fonts,
        visible: bool,
        nested: bool,
        visit: &mut impl FnMut(GlyphOutline) -> Option<()>,
    ) -> Option<()> {
        let mut exact = visible;
        match op {
            Operator::Tj { text }
            | Operator::Quote { text }
            | Operator::DoubleQuote { text, .. } => {
                exact &= self.string(text, state, fonts, visible, nested, visit);
            }
            Operator::TJ { array } => {
                for element in array {
                    match element {
                        TextElement::String(text) => {
                            exact &= self.string(text, state, fonts, exact, nested, visit)
                        }
                        TextElement::Offset(offset) => {
                            self.current = self.current.zip(state.font).map(|(matrix, font)| {
                                let delta = -offset * state.size * 0.001;
                                let shift = if fonts.vertical(font) {
                                    Matrix::translation(0.0, delta)
                                } else {
                                    Matrix::translation(delta * state.scale, 0.0)
                                };
                                shift.multiply(&matrix)
                            });
                        }
                    }
                }
            }
            _ => return None,
        }
        exact.then_some(())
    }

    fn string(
        &mut self,
        bytes: &[u8],
        state: State,
        fonts: &Fonts,
        measure: bool,
        nested: bool,
        visit: &mut impl FnMut(GlyphOutline) -> Option<()>,
    ) -> bool {
        if bytes.is_empty() {
            return true;
        }
        let Some(start) = self.current else {
            return false;
        };
        let Some(font) = state.font else {
            self.current = None;
            return false;
        };
        let Some(mut advances) = fonts.advances(font, bytes) else {
            self.current = None;
            return false;
        };
        if measure {
            let mut matrix = start;
            let mut steps = advances.clone();
            let project = |outline| {
                let advance = steps.next()??;
                if let Some(outline) = outline {
                    let origin = advance.vertical_origin.map_or(matrix, |[x, y]| {
                        Matrix::translation(-x * state.size * state.scale, -y * state.size)
                            .multiply(&matrix)
                    });
                    visit(GlyphOutline {
                        bounds: outline,
                        state: origin.multiply(&state.ctm),
                        size: state.size,
                        scale: state.scale,
                        rise: state.rise,
                    })?;
                }
                matrix = Self::advance(matrix, state, advance);
                Some(())
            };
            let result = if nested {
                fonts.visit_nested_outlines(font, bytes, project)
            } else {
                fonts.visit_outlines(font, bytes, project)
            };
            if result.is_some() {
                self.current = Some(matrix);
                return true;
            }
        }
        self.current = advances.try_fold(start, |matrix, advance| {
            Some(Self::advance(matrix, state, advance?))
        });
        false
    }

    fn advance(matrix: Matrix, state: State, advance: GlyphAdvance) -> Matrix {
        let spacing = if advance.word { state.word_space } else { 0.0 };
        let space = if advance.paints_glyph {
            state.char_space
        } else {
            0.0
        };
        let distance = advance.em * state.size + space + spacing;
        let shift = if advance.vertical_origin.is_some() {
            Matrix::translation(0.0, distance)
        } else {
            Matrix::translation(distance * state.scale, 0.0)
        };
        shift.multiply(&matrix)
    }
}

// Keep the outline cursor in sync even when a show has no measurable ink.
// Its ordinal includes empty shows, just like the paragraph object inventory.
pub(in crate::matters) fn embedded_paragraph_bounds(
    pdf: &PdfDocument,
    resources: &Object,
    data: &[u8],
) -> Option<HashMap<usize, Bounds>> {
    let mut fonts = Fonts::default();
    let mut state = State::default();
    let mut cursor = TextCursor::default();
    let mut saved = Vec::new();
    let mut bounds = HashMap::new();
    let mut ordinal = 0;
    for op in parse_content_stream(data).ok()? {
        cursor.position(&op, state);
        match op {
            Operator::SaveState => saved.push(state),
            Operator::RestoreState => state = saved.pop()?,
            Operator::Tj { .. }
            | Operator::TJ { .. }
            | Operator::Quote { .. }
            | Operator::DoubleQuote { .. } => {
                state.apply(&op, pdf, resources, &mut fonts);
                let embedded = state
                    .font
                    .is_some_and(|font| matches!(font, Font::Embedded(_)) && !fonts.vertical(font));
                if let Some(Some(area)) = cursor.show(&op, state, &fonts, embedded) {
                    bounds.insert(ordinal, area);
                }
                ordinal += 1;
            }
            _ => {
                state.apply(&op, pdf, resources, &mut fonts);
            }
        }
    }
    Some(bounds)
}

pub(in crate::matters) struct EmbeddedParagraphFont {
    fonts: Fonts,
    font: Font,
}

impl EmbeddedParagraphFont {
    pub fn new(pdf: &PdfDocument, resources: &Object, name: &str) -> Option<Self> {
        let binding = resource(pdf, resources, "Font", name)?;
        let mut fonts = Fonts::default();
        let font = fonts.select(pdf, &binding, resources)?;
        (matches!(font, Font::Embedded(_)) && !fonts.vertical(font)).then_some(Self { fonts, font })
    }

    pub fn bounds(&self, text: Vec<u8>, size: f64) -> Option<Bounds> {
        let mut cursor = TextCursor::default();
        let state = State {
            font: Some(self.font),
            size: size as f32,
            ..State::default()
        };
        cursor.position(&Operator::BeginText, state);
        cursor
            .show(&Operator::Tj { text }, state, &self.fonts, true)
            .flatten()
    }
}

pub(super) struct Paint {
    pub render: u8,
    pub paragraph_fill: bool,
    measured: Option<Bounds>,
    stroke: f32,
    miter: f32,
    scale: f32,
}

impl Paint {
    pub fn include(&mut self, bounds: Bounds) {
        union(&mut self.measured, bounds);
    }

    fn bounds(&self, bounds: Bounds, measured: bool) -> Bounds {
        let miter = if bounds[0] < bounds[2] && bounds[1] < bounds[3] {
            self.miter
        } else {
            1.0
        };
        let margin = f64::from(self.stroke * miter * self.scale)
            + if measured || self.stroke > 0.0 {
                1.0
            } else {
                0.0
            };
        [
            bounds[0] - margin,
            bounds[1] - margin,
            bounds[2] + margin,
            bounds[3] + margin,
        ]
    }
}

pub(super) struct Show {
    paint: Option<usize>,
    glyphs: Option<(Glyphs, usize)>,
    size: f32,
    scale: f32,
    rise: f32,
    transform: Matrix,
}

impl Show {
    pub fn can_measure(&self) -> bool {
        self.paint.is_none() || self.glyphs.is_some()
    }

    fn position(&self, glyph: &TextChar) -> Matrix {
        let Matrix { a, b, c, d, .. } = self.transform;
        Matrix {
            a: a * self.size * self.scale,
            b: b * self.size * self.scale,
            c: c * self.size,
            d: d * self.size,
            e: glyph.origin_x + c * self.rise,
            f: glyph.origin_y + d * self.rise,
        }
    }

    fn bounds(&self, glyph: &TextChar) -> Option<Bounds> {
        let (glyphs, _) = self.glyphs.as_ref()?;
        outline_bounds(
            glyphs.bounds(glyph.char)?,
            self.size,
            self.scale,
            self.rise,
            self.transform,
            [glyph.origin_x, glyph.origin_y],
        )
    }
}

fn outline_bounds(
    [x0, y0, x1, y1]: [f32; 4],
    size: f32,
    scale: f32,
    rise: f32,
    transform: Matrix,
    [ox, oy]: [f32; 2],
) -> Option<Bounds> {
    let Matrix { a, b, c, d, .. } = transform;
    let mut bounds = None;
    for (x, y) in [(x0, y0), (x0, y1), (x1, y0), (x1, y1)] {
        let x = x * size * scale;
        let y = y * size + rise;
        let px = f64::from(ox + x * a + y * c);
        let py = f64::from(oy + x * b + y * d);
        if !px.is_finite() || !py.is_finite() {
            return None;
        }
        union(&mut bounds, [px, py, px, py]);
    }
    bounds
}

fn show_index(mark: Option<u32>) -> Option<usize> {
    Some(mark?.checked_sub(FIRST_SHOW)? as usize)
}

#[derive(Default)]
struct MeasuredShow {
    bounds: Option<Bounds>,
    seen: usize,
    unsupported: bool,
}

#[derive(Default)]
pub(super) struct SourcePaints {
    pub text: Vec<Bounds>,
    pub by_paint: Vec<Option<Bounds>>,
    pub glyph_placements: Vec<Option<Matrix>>,
    pub non_opaque: Vec<Bounds>,
}

pub(super) fn reconstruct(
    geometry: Geometry,
    paints: &[Paint],
    shows: &[Show],
    spans: Vec<TextSpan>,
    chars: Vec<TextChar>,
    check_paragraph: bool,
) -> SourcePaints {
    let mut measured = (0..shows.len())
        .map(|_| MeasuredShow::default())
        .collect::<Vec<_>>();
    let mut glyph_placements = vec![None; shows.len()];
    for glyph in &chars {
        let Some(index) = show_index(glyph.mcid).filter(|&index| index < shows.len()) else {
            continue;
        };
        let sample = &mut measured[index];
        sample.seen += 1;
        if shows[index].paint.is_none() {
            glyph_placements[index].get_or_insert_with(|| shows[index].position(glyph));
            continue;
        }
        match shows[index].bounds(glyph) {
            Some(bounds) => union(&mut sample.bounds, bounds),
            None => sample.unsupported = true,
        }
    }
    let mut fallback = vec![None; shows.len()];
    let mut unmarked = Vec::new();
    for span in spans {
        let bounds = super::super::text_edit::span_user_bbox(geometry, &span);
        if let Some(index) = show_index(span.mcid).filter(|&index| index < shows.len()) {
            union(&mut fallback[index], bounds);
        } else {
            unmarked.push(bounds);
        }
    }
    let mut groups: Vec<_> = paints.iter().map(|paint| (paint.measured, true)).collect();
    for (index, (show, measured)) in shows.iter().zip(measured).enumerate() {
        let Some(paint) = show.paint else {
            continue;
        };
        let (bounds, exact) = &mut groups[paint];
        if let Some(geometry) = measured.bounds.filter(|_| {
            !measured.unsupported
                && show
                    .glyphs
                    .as_ref()
                    .is_some_and(|(_, count)| measured.seen == *count)
        }) {
            union(bounds, geometry);
        } else if let Some(span) = fallback[index] {
            union(bounds, span);
            *exact = false;
        }
    }
    let mut non_opaque = Vec::new();
    let by_paint = paints
        .iter()
        .zip(groups)
        .map(|(paint, (bounds, exact))| {
            bounds.map(|bounds| {
                if check_paragraph
                    && !paint.paragraph_fill
                    && bounds[0] < bounds[2]
                    && bounds[1] < bounds[3]
                {
                    non_opaque.push(bounds);
                }
                // A whitespace-only paint can leave only a point or line in the
                // trace at normal page coordinates. It has no intersection area.
                if exact && (bounds[0] >= bounds[2] || bounds[1] >= bounds[3]) {
                    return bounds;
                }
                let bounds = paint.bounds(bounds, exact);
                unmarked.push(bounds);
                bounds
            })
        })
        .collect();
    SourcePaints {
        text: unmarked,
        by_paint,
        glyph_placements,
        non_opaque,
    }
}

pub(super) fn union(bounds: &mut Option<Bounds>, [x0, y0, x1, y1]: Bounds) {
    *bounds = Some(bounds.map_or([x0, y0, x1, y1], |previous| {
        [
            x0.min(previous[0]),
            y0.min(previous[1]),
            x1.max(previous[2]),
            y1.max(previous[3]),
        ]
    }));
}
