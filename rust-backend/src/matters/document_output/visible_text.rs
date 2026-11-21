use super::super::redaction_vectors::{rewrite_ranges, text_command_ranges};
use super::appearance_paint;
use super::assembly::{self, EditedForm, Geometry};
use super::font_glyphs::Fonts;
use super::source_paint::{self, Bounds, Paint, Show, State, TextCursor, FIRST_SHOW};
use super::type3_paint::{GlyphBounds, GlyphPaint, NestedTextPaint};
use pdf_oxide::{
    content::{parse_content_stream, Matrix, Operator, TextElement},
    extractors::TextExtractor,
    object::{Object, ObjectRef},
    optional_content::{
        annotation_is_excluded, compute_default_off_ocgs, resolve_and_check_ocg_excluded,
    },
    writer::ObjectSerializer,
    PdfDocument,
};
use std::{collections::HashSet, fmt::Write as _};

const INSPECTION: &str = "Cannot inspect existing page text";
const MAX_FORM_DEPTH: usize = 12;
const MAX_FORM_PLACEMENTS: usize = 4_096;
const MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;
const MAX_TEXT_SHOWS: usize = 131_072;
const MAX_GLYPH_EVENTS: usize = 262_144;
const UNBOUNDED_PAINT: Bounds = [
    f64::NEG_INFINITY,
    f64::NEG_INFINITY,
    f64::INFINITY,
    f64::INFINITY,
];

#[derive(Default)]
pub(super) struct PagePaints {
    pub text: Vec<Bounds>,
    pub glyph_images: Vec<Bounds>,
    pub appearances: Vec<AppearancePaint>,
    non_opaque: Vec<Bounds>,
}

pub(super) enum AppearancePaint {
    Text(Option<Bounds>),
    GeneratedText(bool),
    Image(Bounds),
    Other(usize),
}

impl AppearancePaint {
    pub fn count(&self) -> usize {
        match self {
            Self::Other(count) => *count,
            _ => 1,
        }
    }
}

#[derive(Clone, Copy)]
enum Event {
    Text(usize),
    GeneratedText(bool),
    Image(Bounds),
    GlyphImage(usize, Matrix),
    GlyphText(usize, usize),
    Other(usize),
}

pub(super) fn page_paints(
    pdf: &PdfDocument,
    page: usize,
    geometry: Geometry,
) -> Result<PagePaints, &'static str> {
    if let Some(bounds) = inspect(pdf, page, geometry, false) {
        return Ok(bounds);
    }
    let mut spans = pdf.extract_spans(page).map_err(|_| INSPECTION)?;
    // Keep the previous OCR fallback when a stream cannot be inspected.
    spans.retain(|span| {
        let name = span.font_name.to_ascii_lowercase();
        !name.contains("glyphless") && !name.contains("matterdraftocr")
    });
    let text = spans
        .iter()
        .map(|span| super::super::text_edit::span_user_bbox(geometry, span))
        .collect();
    Ok(PagePaints {
        text,
        ..Default::default()
    })
}

pub(in crate::matters) fn non_opaque_text(
    pdf: &PdfDocument,
    page: usize,
    geometry: Geometry,
) -> Result<Vec<Bounds>, &'static str> {
    inspect(pdf, page, geometry, true)
        .map(|paints| paints.non_opaque)
        .ok_or(INSPECTION)
}

fn inspect(
    pdf: &PdfDocument,
    page: usize,
    geometry: Geometry,
    check_paragraph: bool,
) -> Option<PagePaints> {
    let content = pdf.get_page_content_data(page).ok()?;
    if check_paragraph && content.len() > MAX_DECODED_BYTES {
        return None;
    }
    let resources = pdf.get_page_resources(page).ok()?;
    let mut inspection = Inspection {
        pdf,
        layer: format!("MatterDraftInvisibleText{}", uuid::Uuid::new_v4().simple()),
        active_forms: HashSet::new(),
        placements: 0,
        decoded_bytes: content.len(),
        paints: Vec::new(),
        shows: Vec::new(),
        fonts: Fonts::default(),
        off_layers: compute_default_off_ocgs(pdf),
        events: None,
        source_glyph_images: Vec::new(),
        source_glyph_text: Vec::new(),
        glyph_text_paints: Vec::new(),
        mask_bindings: Vec::new(),
        shadow_masks: Vec::new(),
        mask_images: Vec::new(),
        check_paragraph,
        paragraph_shows: 0,
    };
    let mut stream = inspection.stream(&content, &resources, State::default())?;
    if check_paragraph {
        if inspection.paints.is_empty() {
            return Some(PagePaints::default());
        }
    } else {
        inspection.append_appearances(page, &resources, &mut stream);
        inspection.append_mask_geometry(&mut stream);
    }
    let Stream { data, forms } = stream;
    let events = inspection.events.take().unwrap_or_default();
    let source_paint::SourcePaints {
        mut text,
        by_paint,
        glyph_placements,
        non_opaque,
    } = if let Some(data) = data {
        // A shadow page qualifies the resource names and Form transforms for each
        // inspected invocation. Only the original document is used to save the edit.
        let shadow = (!forms.is_empty())
            .then(|| assembly::text_geometry_document(pdf, page, data.clone(), &forms))
            .transpose()
            .ok()?;
        let (source, index) = shadow.as_ref().map_or((pdf, page), |(view, _)| (view, 0));

        let extractor = || {
            let resources = source.get_page_resources(index).ok()?;
            let mut extractor = TextExtractor::new();
            extractor.set_document(source);
            extractor.set_page_index(u32::try_from(index).ok()?);
            let mut excluded = inspection.off_layers.clone();
            excluded.insert(inspection.layer.clone());
            extractor.set_excluded_layers(excluded);
            source.load_fonts_public(&resources, &mut extractor).ok()?;
            extractor.set_resources(resources);
            Some(extractor)
        };
        let spans = extractor()?.extract_text_spans(&data).ok()?;
        let chars = if inspection.shows.iter().any(Show::can_measure) {
            extractor()?.extract_owned(&data).ok()?
        } else {
            Vec::new()
        };
        source_paint::reconstruct(
            geometry,
            &inspection.paints,
            &inspection.shows,
            spans,
            chars,
            check_paragraph,
        )
    } else {
        source_paint::SourcePaints::default()
    };
    // An infinite text batch still occupies a trace slot. The reference
    // rectangle intersection excludes it even when another glyph is finite.
    let finite = |bounds: &Bounds| bounds.iter().all(|coordinate| coordinate.is_finite());
    text.retain(finite);
    let image_bounds = |show: usize, matrix: Matrix| {
        let placement = glyph_placements.get(show).copied().flatten()?;
        super::painted_box(matrix.multiply(&placement), [0.0, 0.0, 1.0, 1.0]).ok()
    };
    let nested_bounds = |show: usize, paint: usize| {
        let placement = glyph_placements.get(show).copied().flatten()?;
        inspection
            .glyph_text_paints
            .get(paint)?
            .bounds(Some(placement))
    };
    text.extend(
        inspection
            .source_glyph_text
            .iter()
            .filter_map(|&(show, paint)| nested_bounds(show, paint)),
    );
    let appearances = events
        .into_iter()
        .map(|event| match event {
            Event::Text(index) => {
                AppearancePaint::Text(by_paint.get(index).copied().flatten().filter(finite))
            }
            Event::GeneratedText(ink) => AppearancePaint::GeneratedText(ink),
            Event::Image(bounds) => AppearancePaint::Image(bounds),
            Event::GlyphImage(show, matrix) => {
                image_bounds(show, matrix).map_or(AppearancePaint::Other(1), AppearancePaint::Image)
            }
            Event::GlyphText(show, paint) => {
                let bounds = nested_bounds(show, paint);
                text.extend(bounds);
                AppearancePaint::Text(bounds)
            }
            Event::Other(count) => AppearancePaint::Other(count),
        })
        .collect();
    let glyph_images = inspection
        .source_glyph_images
        .into_iter()
        .filter_map(|(show, matrix)| image_bounds(show, matrix))
        .chain(inspection.mask_images)
        .collect();
    Some(PagePaints {
        text,
        glyph_images,
        appearances,
        non_opaque,
    })
}

struct Inspection<'a> {
    pdf: &'a PdfDocument,
    layer: String,
    active_forms: HashSet<ObjectRef>,
    placements: usize,
    decoded_bytes: usize,
    paints: Vec<Paint>,
    shows: Vec<Show>,
    fonts: Fonts,
    off_layers: HashSet<String>,
    events: Option<Vec<Event>>,
    source_glyph_images: Vec<(usize, Matrix)>,
    source_glyph_text: Vec<(usize, usize)>,
    glyph_text_paints: Vec<NestedTextPaint>,
    mask_bindings: Vec<MaskBinding>,
    shadow_masks: Vec<(Matrix, EditedForm)>,
    mask_images: Vec<Bounds>,
    check_paragraph: bool,
    paragraph_shows: usize,
}

#[derive(Clone)]
struct MaskBinding {
    reference: ObjectRef,
    placement: Matrix,
    resources: Object,
}

#[derive(Default)]
struct Stream {
    data: Option<Vec<u8>>,
    forms: Vec<(String, EditedForm)>,
}

impl Inspection<'_> {
    fn append_appearances(&mut self, page: usize, resources: &Object, stream: &mut Stream) {
        let appearances = appearance_paint::visible(self.pdf, page, &self.off_layers);
        if appearances.is_empty() {
            return;
        }
        self.events = Some(Vec::new());
        let mut calls = Vec::new();
        for (index, appearance) in appearances.into_iter().enumerate() {
            let (reference, matrix) = match appearance {
                appearance_paint::Appearance::Stored {
                    reference,
                    placement,
                } => (reference, placement),
                appearance_paint::Appearance::Generated(paints) => {
                    for paint in paints {
                        match paint {
                            appearance_paint::GeneratedPaint::Paths(paths) => {
                                self.record(Event::Other(paths));
                            }
                            appearance_paint::GeneratedPaint::Text(ink) => {
                                self.record(Event::GeneratedText(ink));
                            }
                        }
                    }
                    continue;
                }
            };
            let before = self.events.as_ref().map_or(0, Vec::len);
            let paints = self.paints.len();
            let shows = self.shows.len();
            let masks = self.shadow_masks.len();
            let glyph_text = self.glyph_text_paints.len();
            let mut state = State::default();
            state.ctm = matrix;
            let mut form = match self.form(reference, resources, state) {
                Some(Some(form)) => form,
                Some(None) => continue,
                None => {
                    self.events.as_mut().unwrap().truncate(before);
                    self.paints.truncate(paints);
                    self.shows.truncate(shows);
                    self.shadow_masks.truncate(masks);
                    self.glyph_text_paints.truncate(glyph_text);
                    continue;
                }
            };
            // An appearance is invoked as a Form even when its stream omits
            // the explicit subtype. Qualify only the inspection copy.
            form.fields
                .insert("Subtype".into(), Object::Name("Form".into()));
            let alias = format!("{}Appearance{index}", self.layer);
            calls.extend(assembly::form_transform([
                matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f,
            ]));
            calls.extend_from_slice(format!("/{alias} Do Q\n").as_bytes());
            stream.forms.push((alias, form));
        }
        if !calls.is_empty() {
            let mut inspection = b"q\n".to_vec();
            inspection.extend(stream.data.take().unwrap_or_default());
            inspection.extend_from_slice(b"\nQ\n");
            inspection.extend(calls);
            stream.data = Some(inspection);
        }
    }

    fn select_mask(&mut self, state: &mut State, resources: &Object, name: &str) {
        if self.check_paragraph {
            return;
        }
        let binding = (|| {
            let values = self
                .pdf
                .resolve_object(resources.as_dict()?.get("ExtGState")?)
                .ok()?;
            let graphics = self.pdf.resolve_object(values.as_dict()?.get(name)?).ok()?;
            self.pdf
                .resolve_object(graphics.as_dict()?.get("SMask")?)
                .ok()
        })();
        let Some(binding) = binding else { return };
        state.soft_mask = binding.as_dict().and_then(|fields| {
            let reference = fields.get("G")?.as_reference()?;
            let index = self.mask_bindings.len();
            self.mask_bindings.push(MaskBinding {
                reference,
                placement: state.ctm,
                resources: resources.clone(),
            });
            Some(index)
        });
    }

    fn paint_mask(&mut self, state: State) -> Option<()> {
        let Some(index) = state.soft_mask else {
            return Some(());
        };
        let binding = self.mask_bindings.get(index)?.clone();
        if self.active_forms.contains(&binding.reference) {
            return Some(());
        }
        // A mask runs at the graphics state that selected it, before the
        // masked drawing. Use an absolute invocation only on the inspection copy.
        let source_page = self.events.is_none();
        if source_page {
            self.events = Some(Vec::new());
        }
        let form = self.form(
            binding.reference,
            &binding.resources,
            state.at_soft_mask(binding.placement),
        );
        if source_page {
            for event in self.events.take().unwrap_or_default() {
                match event {
                    Event::Image(bounds) => self.mask_images.push(bounds),
                    Event::GlyphImage(show, matrix) => {
                        self.source_glyph_images.push((show, matrix))
                    }
                    Event::GlyphText(show, paint) => self.source_glyph_text.push((show, paint)),
                    _ => {}
                }
            }
        }
        if let Some(mut form) = form? {
            form.fields
                .entry("Resources".into())
                .or_insert(binding.resources);
            form.fields
                .insert("Subtype".into(), Object::Name("Form".into()));
            self.shadow_masks.push((binding.placement, form));
        }
        Some(())
    }

    fn append_mask_geometry(&mut self, stream: &mut Stream) {
        if self.shadow_masks.is_empty() {
            return;
        }
        let mut data = b"q\n".to_vec();
        data.extend(stream.data.take().unwrap_or_default());
        data.extend_from_slice(b"\nQ\n");
        for (index, (matrix, form)) in self.shadow_masks.drain(..).enumerate() {
            let alias = format!("{}SoftMask{index}", self.layer);
            data.extend(assembly::form_transform([
                matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f,
            ]));
            data.extend_from_slice(format!("/{alias} Do Q\n").as_bytes());
            stream.forms.push((alias, form));
        }
        stream.data = Some(data);
    }

    fn record(&mut self, event: Event) {
        if let Some(events) = &mut self.events {
            if !matches!(event, Event::Other(0)) {
                events.push(event);
            }
        }
    }

    fn paint_image(&mut self, dict: &assembly::Dictionary, state: State) -> Option<()> {
        if dict
            .get("OC")
            .is_some_and(|oc| annotation_is_excluded(oc, self.pdf, &self.off_layers))
        {
            return Some(());
        }
        let explicit_mask = ["SMask", "Mask"].into_iter().any(|name| {
            dict.get(name)
                .and_then(|value| self.pdf.resolve_object(value).ok())
                .is_some_and(|value| matches!(value, Object::Stream { .. }))
        });
        if !explicit_mask {
            self.paint_mask(state)?;
        }
        let mask = dict
            .get("ImageMask")
            .or_else(|| dict.get("IM"))
            .and_then(Object::as_bool);
        let bounds = super::painted_box(state.ctm, [0.0, 0.0, 1.0, 1.0]);
        let event = match (mask, bounds) {
            (Some(true), _) | (_, Err(_)) => Event::Other(1),
            (None | Some(false), Ok(bounds)) => Event::Image(bounds),
        };
        self.record(event);
        Some(())
    }

    fn stream(&mut self, content: &[u8], resources: &Object, mut state: State) -> Option<Stream> {
        let ops = parse_content_stream(content).ok()?;
        if self.check_paragraph
            && self.active_forms.is_empty()
            && !ops.iter().any(|op| {
                matches!(op,
                    Operator::Tr { render } if *render != 0
                ) || matches!(op, Operator::SetExtGState { .. } | Operator::Do { .. })
            })
        {
            return Some(Stream::default());
        }
        if self.events.is_none()
            && !ops.iter().any(|op| {
                matches!(
                    op,
                    Operator::Tj { .. }
                        | Operator::TJ { .. }
                        | Operator::Quote { .. }
                        | Operator::DoubleQuote { .. }
                        | Operator::Do { .. }
                )
            })
        {
            return Some(Stream::default());
        }
        let xobjects = resources
            .as_dict()?
            .get("XObject")
            .map(|objects| self.pdf.resolve_object(objects))
            .transpose()
            .ok()?;
        let empty = assembly::Dictionary::new();
        let xobjects = match xobjects.as_ref() {
            None | Some(Object::Null) => &empty,
            Some(objects) => objects.as_dict()?,
        };
        let ranges = text_command_ranges(content).ok()?;
        if self.check_paragraph {
            self.paragraph_shows += ranges.shows.len();
            if self.paragraph_shows > MAX_TEXT_SHOWS {
                return None;
            }
        }
        let mut shows = ranges.shows.into_iter();
        let mut draws = ranges.draws.into_iter();
        let mut property_ranges = ranges.properties.into_iter();
        let property_lists = resources
            .as_dict()?
            .get("Properties")
            .and_then(|binding| self.pdf.resolve_object(binding).ok());
        let mut edits = Vec::new();
        let mut forms = Vec::new();
        let mut stack = Vec::new();
        let mut alias_index = 0;
        let mut paint: Option<usize> = None;
        let mut text_axes = Matrix::identity();
        let mut cursor = TextCursor::default();
        let mut layers = Vec::new();
        let mut excluded_depth = 0;
        for op in ops {
            cursor.position(&op, state);
            match op {
                Operator::SaveState => stack.push(state),
                Operator::RestoreState => {
                    state = stack.pop().unwrap_or(state);
                    paint = None;
                }
                Operator::BeginText => {
                    text_axes = Matrix::identity();
                    paint = None;
                }
                Operator::Tm { a, b, c, d, .. } => {
                    text_axes = Matrix {
                        a,
                        b,
                        c,
                        d,
                        e: 0.0,
                        f: 0.0,
                    };
                }
                Operator::BeginMarkedContentDict {
                    ref tag,
                    ref properties,
                } => {
                    let range = property_ranges.next()?;
                    let property = match properties.as_name() {
                        Some(name) => property_lists
                            .as_ref()
                            .and_then(Object::as_dict)
                            .and_then(|list| list.get(name))
                            .and_then(|obj| self.pdf.resolve_object(obj).ok()),
                        None => self.pdf.resolve_object(properties).ok(),
                    };
                    if let Some(mut fields) = property.and_then(|obj| obj.as_dict().cloned()) {
                        if fields.remove("ActualText").is_some() {
                            // Replacement text is a reading aid. Collision
                            // inspection needs the original glyph advances.
                            let serializer = ObjectSerializer::compact();
                            let mut replacement = serializer.serialize(&Object::Name(tag.clone()));
                            replacement.push(b' ');
                            replacement.extend(serializer.serialize(&Object::Dictionary(fields)));
                            replacement.extend_from_slice(b" BDC");
                            edits.push((range, String::from_utf8(replacement).ok()?));
                        }
                    }
                    let excluded = tag == "OC"
                        && resolve_and_check_ocg_excluded(
                            properties,
                            Some(resources),
                            Some(self.pdf),
                            &self.off_layers,
                        );
                    layers.push(excluded);
                    excluded_depth += usize::from(excluded);
                    paint = None;
                }
                Operator::BeginMarkedContent { .. } => {
                    layers.push(false);
                    paint = None;
                }
                Operator::EndMarkedContent => {
                    excluded_depth -= usize::from(layers.pop().unwrap_or(false));
                    paint = None;
                }
                Operator::Other { ref name, .. } if name == "DP" => {
                    property_ranges.next()?;
                }
                Operator::Tj { .. }
                | Operator::TJ { .. }
                | Operator::Quote { .. }
                | Operator::DoubleQuote { .. } => {
                    if self.check_paragraph {
                        paint = None;
                    }
                    let range = shows.next()?;
                    let type3 = state.type3();
                    if matches!(op, Operator::DoubleQuote { .. }) {
                        state.apply(&op, self.pdf, resources, &mut self.fonts);
                    }
                    let restricted = self.check_paragraph && !state.paragraph_fill();
                    let direct = cursor.show(
                        &op,
                        state,
                        &self.fonts,
                        excluded_depth == 0
                            && (restricted
                                || (!self.check_paragraph && !matches!(state.render, 3 | 7)))
                            && type3.is_none(),
                    );
                    if excluded_depth > 0
                        || (self.check_paragraph && !restricted)
                        || (!restricted && state.render == 7 && type3.is_none())
                    {
                        paint = None;
                        edits.push((range.start..range.start, self.hidden()));
                        edits.push((range.end..range.end, "EMC".to_owned()));
                        continue;
                    }
                    if let Some(font) = type3.filter(|_| !self.check_paragraph) {
                        let mut instrumented = String::new();
                        let hidden = self.hidden();
                        let mut show = |bytes, instrumented: &mut String| {
                            self.type3_show(font, bytes, state, text_axes, &mut paint, instrumented)
                        };
                        match &op {
                            Operator::Tj { text } => show(text.as_slice(), &mut instrumented)?,
                            Operator::Quote { text } => {
                                instrumented.push_str("T*\n");
                                show(text.as_slice(), &mut instrumented)?;
                            }
                            Operator::DoubleQuote {
                                word_space,
                                char_space,
                                text,
                            } => {
                                writeln!(instrumented, "{word_space:.9} Tw {char_space:.9} Tc T*")
                                    .ok()?;
                                show(text.as_slice(), &mut instrumented)?;
                            }
                            Operator::TJ { array } => {
                                // Reuse the same paint buffer across strings and
                                // keep adjustment-only operators in the shadow.
                                for element in array {
                                    match element {
                                        TextElement::String(bytes) => {
                                            show(bytes.as_slice(), &mut instrumented)?
                                        }
                                        TextElement::Offset(offset) => {
                                            writeln!(instrumented, "{hidden} [{offset:.9}] TJ EMC")
                                                .ok()?;
                                        }
                                    }
                                }
                            }
                            _ => unreachable!(),
                        }
                        edits.push((range, instrumented));
                        continue;
                    }
                    let has_glyphs = match &op {
                        Operator::Tj { text }
                        | Operator::Quote { text }
                        | Operator::DoubleQuote { text, .. } => !text.is_empty(),
                        Operator::TJ { array } => array.iter().any(
                            |item| matches!(item, TextElement::String(text) if !text.is_empty()),
                        ),
                        _ => unreachable!(),
                    };
                    if !has_glyphs {
                        continue;
                    }
                    if (!restricted && state.render == 3) || direct.is_some() {
                        if let Some(Some(bounds)) = direct {
                            let index = self.text_paint(&mut paint, state)?;
                            self.paints[index].include(bounds);
                        } else if state.render == 3 {
                            self.text_paint(&mut paint, state)?;
                        }
                        edits.push((range.start..range.start, self.hidden()));
                        edits.push((range.end..range.end, "EMC".to_owned()));
                        continue;
                    }
                    let index = self.text_paint(&mut paint, state)?;
                    match op {
                        Operator::TJ { array } => {
                            let mut instrumented = String::new();
                            // Preserve the advance while suppressing any
                            // synthetic spaces the extractor inserts for it.
                            for item in array {
                                match item {
                                    TextElement::String(bytes) => {
                                        let tag = self.tag(state.show(
                                            &bytes,
                                            Some(index),
                                            text_axes,
                                            &self.fonts,
                                        ))?;
                                        writeln!(
                                            instrumented,
                                            "{tag} <{}> Tj EMC",
                                            super::hex_pdf(&bytes)
                                        )
                                        .ok()?;
                                    }
                                    TextElement::Offset(offset) => {
                                        writeln!(
                                            instrumented,
                                            "{} [{offset:.9}] TJ EMC",
                                            self.hidden()
                                        )
                                        .ok()?;
                                    }
                                }
                            }
                            edits.push((range, instrumented));
                        }
                        Operator::Tj { text }
                        | Operator::Quote { text }
                        | Operator::DoubleQuote { text, .. } => {
                            edits.push((
                                range.start..range.start,
                                self.tag(state.show(&text, Some(index), text_axes, &self.fonts))?,
                            ));
                            edits.push((range.end..range.end, "EMC".to_owned()));
                        }
                        _ => unreachable!(),
                    }
                }
                Operator::Do { name } => {
                    paint = None;
                    let range = draws.next()?;
                    if excluded_depth > 0 {
                        continue;
                    }
                    let Some(reference) = xobjects.get(&name).and_then(Object::as_reference) else {
                        continue;
                    };
                    if !self.pdf.is_form_xobject(reference) {
                        if self.events.is_some() || state.soft_mask.is_some() {
                            if let Ok(image) = self.pdf.load_object(reference) {
                                if let Some(dict) = image.as_dict() {
                                    if dict.get("Subtype").and_then(Object::as_name)
                                        == Some("Image")
                                    {
                                        self.paint_image(dict, state)?;
                                    }
                                }
                            }
                        }
                        continue;
                    }
                    if self.active_forms.contains(&reference) {
                        continue;
                    }
                    let mut child_state = state;
                    if state.soft_mask.is_some() {
                        let form = self.pdf.load_object(reference).ok()?;
                        let fields = form.as_dict()?;
                        if fields.get("OC").is_some_and(|oc| {
                            annotation_is_excluded(oc, self.pdf, &self.off_layers)
                        }) {
                            continue;
                        }
                        let isolated_group = fields
                            .get("Group")
                            .and_then(|binding| self.pdf.resolve_object(binding).ok())
                            .as_ref()
                            .and_then(Object::as_dict)
                            .and_then(|fields| fields.get("S"))
                            .and_then(Object::as_name)
                            == Some("Transparency");
                        if isolated_group {
                            self.paint_mask(state)?;
                            child_state.soft_mask = None;
                        }
                    }
                    if let Some(form) = self.form(reference, resources, child_state)? {
                        // The same resource can be invoked with different
                        // inherited text states. Rebind only this invocation.
                        let alias = loop {
                            let name = format!("MatterDraftVisibilityForm{alias_index}");
                            alias_index += 1;
                            if !xobjects.contains_key(&name) {
                                break name;
                            }
                        };
                        edits.push((range, format!("/{alias} Do")));
                        forms.push((alias, form));
                    }
                }
                Operator::InlineImage { ref dict, .. } => {
                    paint = None;
                    if excluded_depth == 0 {
                        self.paint_image(dict, state)?;
                    }
                }
                other => {
                    if excluded_depth == 0 && (self.events.is_some() || state.soft_mask.is_some()) {
                        let count = appearance_paint::other_paints(&other);
                        if count > 0 {
                            paint = None;
                            self.paint_mask(state)?;
                            self.record(Event::Other(count));
                        }
                    }
                    if let Operator::SetExtGState { dict_name } = &other {
                        self.select_mask(&mut state, resources, dict_name);
                    }
                    if state.apply(&other, self.pdf, resources, &mut self.fonts) {
                        paint = None;
                    }
                }
            }
        }
        if shows.next().is_some() || draws.next().is_some() {
            return None;
        }
        if edits.is_empty() {
            return Some(Stream::default());
        }
        // The hidden show still advances the text matrix. Only the extractor
        // skips emitting its glyphs, so later visible glyphs keep their positions.
        Some(Stream {
            data: Some(rewrite_ranges(content, edits).ok()?),
            forms,
        })
    }

    fn hidden(&self) -> String {
        format!("/OC << /Type /OCG /Name ({}) >> BDC", self.layer)
    }

    fn text_paint(&mut self, paint: &mut Option<usize>, state: State) -> Option<usize> {
        if let Some(index) = *paint {
            if self.paints[index].render == state.render
                && (!self.check_paragraph
                    || self.paints[index].paragraph_fill == state.paragraph_fill())
            {
                return Some(index);
            }
        }
        if matches!(state.render, 0 | 1 | 2 | 4 | 5 | 6) {
            self.paint_mask(state)?;
        }
        let index = self.paints.len();
        self.paints.push(state.paint());
        *paint = Some(index);
        match state.render {
            3 => self.record(Event::Other(1)),
            2 | 6 => {
                self.record(Event::Text(index));
                self.record(Event::Text(index));
            }
            _ => self.record(Event::Text(index)),
        }
        Some(index)
    }

    fn type3_show(
        &mut self,
        font: usize,
        bytes: &[u8],
        state: State,
        text_axes: Matrix,
        paint: &mut Option<usize>,
        output: &mut String,
    ) -> Option<()> {
        if bytes.len() > MAX_TEXT_SHOWS.saturating_sub(self.shows.len()) {
            return None;
        }
        for &code in bytes {
            let glyph = self
                .fonts
                .type3_glyph(self.pdf, font, code, &self.off_layers);
            let direct = glyph
                .as_ref()
                .filter(|glyph| glyph.direct || (glyph.colored && state.fill_pattern));
            let invisible = matches!(state.render, 3 | 7);
            let mut effective = state;
            effective.render = if invisible || direct.is_some() { 3 } else { 0 };
            if state.render != effective.render || direct.is_some() {
                *paint = None;
            }
            let nested_tag = if let Some(glyph) = direct.filter(|_| !invisible) {
                let show = self.shows.len();
                let recorded = self.events.as_ref().map_or(
                    self.source_glyph_images.len() + self.source_glyph_text.len(),
                    Vec::len,
                );
                if glyph.events.len() > MAX_GLYPH_EVENTS.saturating_sub(recorded) {
                    return None;
                }
                let mut needs_origin = false;
                for event in glyph.events.iter() {
                    match event {
                        GlyphPaint::Other(count) => self.record(Event::Other(*count)),
                        GlyphPaint::Image(matrix) => {
                            needs_origin = true;
                            if self.events.is_some() {
                                self.record(Event::GlyphImage(show, *matrix));
                            } else {
                                self.source_glyph_images.push((show, *matrix));
                            }
                        }
                        GlyphPaint::Text(paint) => {
                            needs_origin = true;
                            let index = self.glyph_text_paints.len();
                            self.glyph_text_paints.push(paint.clone());
                            if self.events.is_some() {
                                self.record(Event::GlyphText(show, index));
                            } else {
                                self.source_glyph_text.push((show, index));
                            }
                        }
                    }
                }
                if needs_origin {
                    Some(self.tag(effective.show(&[code], None, text_axes, &self.fonts))?)
                } else {
                    None
                }
            } else {
                None
            };
            let index = self.text_paint(paint, effective)?;
            let unbounded = effective.render == 0
                && glyph.is_some_and(|glyph| matches!(glyph.bounds, GlyphBounds::Unbounded));
            if unbounded {
                self.paints[index].include(UNBOUNDED_PAINT);
            }
            let scope = if let Some(tag) = nested_tag {
                tag
            } else if effective.render == 3 || unbounded {
                self.hidden()
            } else {
                self.tag(effective.show(&[code], Some(index), text_axes, &self.fonts))?
            };
            writeln!(output, "{scope} <{code:02X}> Tj EMC").ok()?;
        }
        Some(())
    }

    fn tag(&mut self, show: Show) -> Option<String> {
        if self.shows.len() >= MAX_TEXT_SHOWS {
            return None;
        }
        let mark = FIRST_SHOW + self.shows.len() as u32;
        self.shows.push(show);
        Some(format!("/Span << /MCID {mark} >> BDC"))
    }

    fn form(
        &mut self,
        reference: ObjectRef,
        parent_resources: &Object,
        mut state: State,
    ) -> Option<Option<EditedForm>> {
        self.placements += 1;
        if self.active_forms.len() >= MAX_FORM_DEPTH || self.placements > MAX_FORM_PLACEMENTS {
            return None;
        }
        let object = self.pdf.load_object(reference).ok()?;
        let fields = object.as_dict()?;
        if fields
            .get("OC")
            .is_some_and(|oc| annotation_is_excluded(oc, self.pdf, &self.off_layers))
        {
            return Some(None);
        }
        state.ctm = super::form_geometry(self.pdf, fields, state.ctm).ok()?.1;
        let resources = fields
            .get("Resources")
            .map(|value| self.pdf.resolve_object(value).ok())
            .unwrap_or(Some(Object::Null))?;
        let resources = if matches!(resources, Object::Null) {
            parent_resources
        } else {
            &resources
        };
        let content = object.decode_stream_data().ok()?;
        self.decoded_bytes += content.len();
        if self.decoded_bytes > MAX_DECODED_BYTES {
            return None;
        }
        self.active_forms.insert(reference);
        let stream = self.stream(&content, resources, state);
        self.active_forms.remove(&reference);
        let Stream { data, forms } = stream?;
        let Some(data) = data else {
            return Some(None);
        };
        let mut fields = fields.clone();
        for key in ["Length", "Filter", "DecodeParms"] {
            fields.remove(key);
        }
        if self.events.is_some() {
            // The reference log reports paint bounds even beyond the
            // appearance clip. Retain the matrix but bypass text extraction's
            // Form clipping on this inspection copy.
            fields.remove("BBox");
        }
        if !forms.is_empty() {
            fields.insert("Resources".into(), resources.clone());
        } else if fields.get("Resources") == Some(&Object::Null) {
            fields.remove("Resources");
        }
        Some(Some(EditedForm {
            original: reference,
            fields,
            data,
            children: forms,
        }))
    }
}
