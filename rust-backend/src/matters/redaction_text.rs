//! Use the original composite font's character codes when removing PDF text.
use super::{
    document_output::{form_geometry, painted_box},
    page_tools::assembly::{
        self, Dictionary, EditedForm, Overlay, XObjectEdit, XObjectReplacement,
    },
    redaction_vectors::{draw_commands, marked_commands, rewrite_ranges, text_command_ranges},
    text_edit::{composite_codes, intersects, source_font_encoding},
};
use pdf_oxide::{
    content::{parse_content_stream, Matrix, Operator, TextElement},
    extractors::TextExtractor,
    fonts::FontInfo,
    object::{Object, ObjectRef},
    optional_content::decode_pdf_text_string,
    redaction::{
        redact_content_stream, text_engine::FontMetrics, FontInfoMetrics, RedactionOptions,
        RedactionRegion, RegionSet,
    },
    writer::ObjectSerializer,
    PdfDocument,
};
use std::{
    collections::{HashMap, HashSet},
    io::Write as _,
    sync::Arc,
};

const WORD_SPACE: u32 = 1 << 16;
const UNSUPPORTED: &str = "Cannot safely remove text encoded with the page's composite fonts";
const MAX_COMPOSITE_FONTS: usize = 32;
const FORM_EDIT: &str = "Cannot safely isolate the selected Form XObject placement";
const MAX_FORM_DEPTH: usize = 12;
const MAX_FORM_PLACEMENTS: usize = 4_096;
const MAX_FORM_BYTES: usize = 64 * 1024 * 1024;
const ACTUAL_TEXT_EDIT: &str = "Cannot preserve the remaining alternate PDF text";
const AUTHORED_EDIT: &str = "Selected glyphs could not be isolated safely";

pub(super) type EditRegions = HashMap<Option<ObjectRef>, Vec<[f32; 4]>>;

struct CompositeFont {
    info: Arc<FontInfo>,
    cids: HashMap<(u32, usize), u16>,
}

impl CompositeFont {
    fn decode(&self, mut bytes: &[u8]) -> Option<Vec<(u32, Vec<u8>)>> {
        let mut glyphs = Vec::new();
        while !bytes.is_empty() {
            let mut code = 0;
            let mut matched = None;
            for (index, &byte) in bytes.iter().take(4).enumerate() {
                code = (code << 8) | u32::from(byte);
                let length = index + 1;
                if let Some(&cid) = self.cids.get(&(code, length)) {
                    if matched.is_some() {
                        return None;
                    }
                    let word_space = length == 1 && code == 32;
                    matched = Some((
                        length,
                        u32::from(cid) | if word_space { WORD_SPACE } else { 0 },
                    ));
                }
            }
            let (length, metric_code) = matched?;
            glyphs.push((metric_code, bytes[..length].to_vec()));
            bytes = &bytes[length..];
        }
        Some(glyphs)
    }
}

struct PageFonts {
    standard: FontInfoMetrics,
    composite: HashMap<String, Option<CompositeFont>>,
    metrics: HashMap<String, Arc<FontInfo>>,
}

impl FontMetrics for PageFonts {
    fn width(&self, font: &str, code: u32) -> f32 {
        match self.composite.get(font) {
            Some(Some(composite)) => composite.info.get_glyph_width(code as u16),
            _ => self.standard.width(font, code),
        }
    }

    fn decode(&self, font: &str, bytes: &[u8]) -> Vec<(u32, Vec<u8>)> {
        match self.composite.get(font) {
            Some(Some(composite)) => composite.decode(bytes).unwrap_or_default(),
            Some(None) => Vec::new(),
            None => self.standard.decode(font, bytes),
        }
    }

    fn is_word_space(&self, font: &str, code: u32) -> bool {
        if self.composite.contains_key(font) {
            code & WORD_SPACE != 0
        } else {
            self.standard.is_word_space(font, code)
        }
    }

    fn is_simple(&self, font: &str) -> bool {
        self.standard.is_simple(font)
    }

    fn can_prune(&self, font: &str) -> bool {
        self.composite
            .get(font)
            .map_or_else(|| self.standard.can_prune(font), Option::is_some)
    }

    fn redaction_safe_show(&self, font: &str, bytes: &[u8]) -> bool {
        match self.composite.get(font) {
            Some(Some(composite)) => composite.decode(bytes).is_some(),
            Some(None) => false,
            None => self.standard.redaction_safe_show(font, bytes),
        }
    }
}

/// Return a replacement content stream when this page needs more than the
/// PDF Oxide editor's simple-font and Identity-H support.
pub(super) fn rewrite_composite(
    pdf: &PdfDocument,
    page: usize,
    bounds: &[[f32; 4]],
    color: Option<[f32; 3]>,
    options: &RedactionOptions,
) -> Result<Option<(Overlay, usize)>, &'static str> {
    let content = pdf.get_page_content_data(page).map_err(|_| UNSUPPORTED)?;
    let resources = pdf.get_page_resources(page).map_err(|_| UNSUPPORTED)?;
    let regions = regions(page, bounds, color);
    Ok(
        rewrite_stream(pdf, &resources, &content, &regions, options, true, None)?.map(
            |(data, glyphs)| {
                (
                    Overlay {
                        replace_page_content: Some(data),
                        ..Default::default()
                    },
                    glyphs,
                )
            },
        ),
    )
}

/// Rewrite the selected text operators for an ordinary text edit.
pub(super) struct TextEdit {
    pub overlay: Overlay,
    pub glyphs: usize,
    pub enclosing_mcid: Option<usize>,
}

pub(super) struct InlineActualTextEdit {
    pub form: Option<ObjectRef>,
    pub property: usize,
    pub expected: String,
    pub remaining: String,
}

pub(super) fn edit_stream(
    pdf: &PdfDocument,
    page: usize,
    data: Vec<u8>,
    selected: &EditRegions,
    locate_page_tag: bool,
    painted_text: Option<&str>,
    inline: &[InlineActualTextEdit],
    order_split: Option<&super::text_edit::reading_order::Split>,
    authored_shows: Option<&HashSet<usize>>,
) -> Result<TextEdit, &'static str> {
    let resources = pdf.get_page_resources(page).map_err(|_| UNSUPPORTED)?;
    let mut options = RedactionOptions::default();
    options.draw_overlay_when_no_ic = false;
    options.edge_padding = 0.0;
    let mut editor = FormEditor {
        pdf,
        page,
        selected,
        options,
        form_regions: selected
            .iter()
            .filter(|(scope, _)| scope.is_some())
            .flat_map(|(_, bounds)| bounds.iter().map(|area| area.map(f64::from)))
            .collect(),
        placements: 0,
        decoded: data.len(),
        locate_page_tag,
        painted_text,
        inline,
        updated_inline: HashSet::new(),
        order_split,
        split_applied: false,
        authored_shows,
    };
    let stream = editor.edit(data, &resources, None, Matrix::identity(), 0)?;
    if editor.updated_inline.len() != inline.len() || order_split.is_some() != editor.split_applied
    {
        return Err(ACTUAL_TEXT_EDIT);
    }
    Ok(TextEdit {
        overlay: Overlay {
            replace_page_content: Some(stream.data),
            replace_properties: stream.properties,
            xobject_edits: stream
                .children
                .into_iter()
                .map(|(target, form)| XObjectEdit {
                    source: None,
                    target,
                    replacement: XObjectReplacement::EditedForm(form),
                })
                .collect(),
            ..Default::default()
        },
        glyphs: stream.glyphs,
        enclosing_mcid: stream.enclosing_mcid,
    })
}

struct FormEditor<'a> {
    pdf: &'a PdfDocument,
    page: usize,
    selected: &'a EditRegions,
    options: RedactionOptions,
    form_regions: Vec<[f64; 4]>,
    placements: usize,
    decoded: usize,
    locate_page_tag: bool,
    painted_text: Option<&'a str>,
    inline: &'a [InlineActualTextEdit],
    updated_inline: HashSet<usize>,
    order_split: Option<&'a super::text_edit::reading_order::Split>,
    split_applied: bool,
    authored_shows: Option<&'a HashSet<usize>>,
}

struct StreamEdit {
    data: Vec<u8>,
    glyphs: usize,
    children: Vec<(String, EditedForm)>,
    enclosing_mcid: Option<usize>,
    properties: Option<Dictionary>,
}

impl FormEditor<'_> {
    fn edit(
        &mut self,
        data: Vec<u8>,
        resources: &Object,
        scope: Option<ObjectRef>,
        placement: Matrix,
        depth: usize,
    ) -> Result<StreamEdit, &'static str> {
        let mut stream = self.edit_glyphs(data, resources, scope, placement, depth)?;
        if stream.glyphs > 0 {
            let edits = self
                .inline
                .iter()
                .enumerate()
                .filter(|(_, edit)| edit.form == scope)
                .map(|(index, edit)| (edit.property, (index, edit)))
                .collect::<HashMap<_, _>>();
            if !edits.is_empty() {
                let (data, properties) = update_actual_text(
                    self.pdf,
                    resources,
                    &stream.data,
                    &edits,
                    &mut self.updated_inline,
                )?;
                stream.data = data;
                stream.properties = Some(properties);
            }
        }
        Ok(stream)
    }

    fn edit_glyphs(
        &mut self,
        data: Vec<u8>,
        resources: &Object,
        scope: Option<ObjectRef>,
        placement: Matrix,
        depth: usize,
    ) -> Result<StreamEdit, &'static str> {
        let mut stream = StreamEdit {
            data,
            glyphs: 0,
            children: Vec::new(),
            enclosing_mcid: None,
            properties: None,
        };
        if let Some(bounds) = self.selected.get(&scope) {
            let split = self.order_split.filter(|split| split.scope == scope);
            let marked = split
                .map(|split| split.mark(&stream.data).ok_or(ACTUAL_TEXT_EDIT))
                .transpose()?;
            let content = marked.as_deref().unwrap_or(&stream.data);
            let (rewritten, glyphs) =
                if let Some(shows) = self.authored_shows.filter(|_| scope.is_none()) {
                    remove_authored_shows(
                        self.pdf,
                        self.page,
                        resources,
                        content,
                        shows,
                        self.painted_text,
                    )?
                } else {
                    // Analyze the selected boxes in page space, then restore the
                    // Form's local coordinates when writing its replacement stream.
                    let regions = regions(self.page, bounds, None);
                    let projected = transform_stream(placement, content);
                    let (rewritten, glyphs) = rewrite_stream(
                        self.pdf,
                        resources,
                        &projected,
                        &regions,
                        &self.options,
                        false,
                        self.painted_text,
                    )?
                    .ok_or(UNSUPPORTED)?;
                    (transform_stream(inverse(placement)?, &rewritten), glyphs)
                };
            if glyphs > 0 {
                if split.is_some() && std::mem::replace(&mut self.split_applied, true) {
                    return Err(ACTUAL_TEXT_EDIT);
                }
                stream.data = rewritten;
                stream.glyphs = glyphs;
            }
        }
        if self.form_regions.is_empty() {
            return Ok(stream);
        }
        let xobjects = resources
            .as_dict()
            .and_then(|resources| resources.get("XObject"));
        let Some(xobjects) = xobjects else {
            return Ok(stream);
        };
        let xobjects = self.pdf.resolve_object(xobjects).map_err(|_| FORM_EDIT)?;
        let xobjects = xobjects.as_dict().ok_or(FORM_EDIT)?;
        let prefix = (0..=xobjects.len())
            .map(|n| format!("MatterDraftTextForm{n}_"))
            .find(|prefix| xobjects.keys().all(|key| !key.starts_with(prefix)))
            .ok_or(FORM_EDIT)?;
        let operators = parse_content_stream(&stream.data).map_err(|_| FORM_EDIT)?;
        let ranges = draw_commands(&stream.data).map_err(|_| FORM_EDIT)?;
        let mut draws = ranges.into_iter();
        let mut state = placement;
        let mut saved = Vec::new();
        let mut revised = Vec::new();
        let mut offset = 0;
        let mut edited_paint = None;
        for (index, operator) in operators.iter().enumerate() {
            match operator {
                Operator::SaveState => saved.push(state),
                Operator::RestoreState => state = saved.pop().ok_or(FORM_EDIT)?,
                Operator::Cm { a, b, c, d, e, f } => {
                    state = Matrix {
                        a: *a,
                        b: *b,
                        c: *c,
                        d: *d,
                        e: *e,
                        f: *f,
                    }
                    .multiply(&state);
                }
                Operator::Do { name } => {
                    let range = draws.next().ok_or(FORM_EDIT)?;
                    let original = xobjects.get(name).ok_or(FORM_EDIT)?;
                    if let Some((form, glyphs)) =
                        self.edit_form(original, resources, state, depth)?
                    {
                        // Other invocations of the same binding stay intact.
                        let target = format!("{prefix}{}", stream.children.len());
                        revised.extend_from_slice(&stream.data[offset..range.start]);
                        write!(revised, "/{target} Do").map_err(|_| FORM_EDIT)?;
                        offset = range.end;
                        stream.children.push((target, form));
                        stream.glyphs += glyphs;
                        edited_paint = Some(index);
                    }
                }
                _ => {}
            }
        }
        if draws.next().is_some() {
            return Err(FORM_EDIT);
        }
        if self.locate_page_tag && depth == 0 && stream.children.len() == 1 {
            let index = edited_paint.ok_or(FORM_EDIT)?;
            stream.enclosing_mcid = enclosing_page_mcid(self.pdf, resources, &operators[..index])?;
        }
        if offset > 0 {
            revised.extend_from_slice(&stream.data[offset..]);
            stream.data = revised;
        }
        Ok(stream)
    }

    fn edit_form(
        &mut self,
        original: &Object,
        resources: &Object,
        placement: Matrix,
        depth: usize,
    ) -> Result<Option<(EditedForm, usize)>, &'static str> {
        let object = self.pdf.resolve_object(original).map_err(|_| FORM_EDIT)?;
        let fields = object.as_dict().ok_or(FORM_EDIT)?;
        if fields.get("Subtype").and_then(Object::as_name) != Some("Form") {
            return Ok(None);
        }
        let (local, placement) = form_geometry(self.pdf, fields, placement)?;
        let visible = painted_box(placement, local)?;
        if !self
            .form_regions
            .iter()
            .any(|&area| intersects(visible, area))
        {
            return Ok(None);
        }
        self.placements += 1;
        if depth >= MAX_FORM_DEPTH || self.placements > MAX_FORM_PLACEMENTS {
            return Err(FORM_EDIT);
        }
        let effective = fields
            .get("Resources")
            .map(|value| self.pdf.resolve_object(value))
            .transpose()
            .map_err(|_| FORM_EDIT)?;
        let effective = effective
            .as_ref()
            .filter(|value| !matches!(value, Object::Null))
            .unwrap_or(resources);
        let data = object.decode_stream_data().map_err(|_| FORM_EDIT)?;
        self.decoded += data.len();
        if self.decoded > MAX_FORM_BYTES {
            return Err(FORM_EDIT);
        }
        let scope = original.as_reference().ok_or(FORM_EDIT)?;
        let edited = self.edit(data, effective, Some(scope), placement, depth + 1)?;
        if edited.glyphs == 0 {
            return Ok(None);
        }
        let mut fields = fields.clone();
        for key in ["Length", "Filter", "DecodeParms"] {
            fields.remove(key);
        }
        if !edited.children.is_empty() || edited.properties.is_some() {
            let mut local = effective.as_dict().cloned().ok_or(FORM_EDIT)?;
            if let Some(properties) = edited.properties {
                local.insert("Properties".into(), Object::Dictionary(properties));
            }
            fields.insert("Resources".into(), Object::Dictionary(local));
        }
        Ok(Some((
            EditedForm {
                original: scope,
                fields,
                data: edited.data,
                children: edited.children,
            },
            edited.glyphs,
        )))
    }
}

fn update_actual_text(
    pdf: &PdfDocument,
    resources: &Object,
    data: &[u8],
    edits: &HashMap<usize, (usize, &InlineActualTextEdit)>,
    updated: &mut HashSet<usize>,
) -> Result<(Vec<u8>, Dictionary), &'static str> {
    let original = resources
        .as_dict()
        .and_then(|fields| fields.get("Properties"))
        .map(|value| pdf.resolve_object(value))
        .transpose()
        .map_err(|_| ACTUAL_TEXT_EDIT)?;
    let mut properties = original
        .as_ref()
        .and_then(Object::as_dict)
        .cloned()
        .unwrap_or_default();
    let prefix = (0..=properties.len())
        .map(|n| format!("MDActualText{n}_"))
        .find(|prefix| properties.keys().all(|name| !name.starts_with(prefix)))
        .ok_or(ACTUAL_TEXT_EDIT)?;
    let ranges = marked_commands(data)?
        .into_iter()
        .filter(|range| data[range.clone()].ends_with(b"BDC"));
    let values = parse_content_stream(data)
        .map_err(|_| ACTUAL_TEXT_EDIT)?
        .into_iter()
        .filter_map(|op| match op {
            Operator::BeginMarkedContentDict { tag, properties } => Some((tag, *properties)),
            _ => None,
        });
    let serializer = ObjectSerializer::compact();
    let mut patches = Vec::new();
    for (ordinal, (range, (tag, property))) in ranges.zip(values).enumerate() {
        let Some(&(edit_index, edit)) = edits.get(&ordinal) else {
            continue;
        };
        let source = if let Some(key) = property.as_name() {
            let value = properties.get(key).ok_or(ACTUAL_TEXT_EDIT)?;
            pdf.resolve_object(value).map_err(|_| ACTUAL_TEXT_EDIT)?
        } else {
            property
        };
        let mut fields = source.as_dict().cloned().ok_or(ACTUAL_TEXT_EDIT)?;
        let encoded = pdf
            .resolve_object(&fields.remove("ActualText").ok_or(ACTUAL_TEXT_EDIT)?)
            .map_err(|_| ACTUAL_TEXT_EDIT)?;
        let text = encoded.as_string().ok_or(ACTUAL_TEXT_EDIT)?;
        if decode_pdf_text_string(text) != edit.expected || !updated.insert(edit_index) {
            return Err(ACTUAL_TEXT_EDIT);
        }
        if !edit.remaining.is_empty() {
            fields.insert("ActualText".into(), assembly::pdf_text(&edit.remaining));
        }
        let key = format!("{prefix}{ordinal}");
        properties.insert(key.clone(), Object::Dictionary(fields));
        let tag = serializer.serialize_to_string(&Object::Name(tag));
        patches.push((range, format!("{tag} /{key} BDC")));
    }
    if patches.len() != edits.len() {
        return Err(ACTUAL_TEXT_EDIT);
    }
    Ok((rewrite_ranges(data, patches)?, properties))
}

#[derive(Clone, Copy)]
enum PageMark {
    Unmarked,
    Artifact,
    Content(usize),
}

fn enclosing_page_mcid(
    pdf: &PdfDocument,
    resources: &Object,
    operators: &[Operator],
) -> Result<Option<usize>, &'static str> {
    let properties = resources
        .as_dict()
        .and_then(|dict| dict.get("Properties"))
        .map(|value| pdf.resolve_object(value))
        .transpose()
        .map_err(|_| FORM_EDIT)?;
    let mut stack = Vec::new();
    for operator in operators {
        let inherited = stack.last().copied().unwrap_or(PageMark::Unmarked);
        match operator {
            Operator::BeginMarkedContent { tag } => {
                stack.push(if tag == "Artifact" {
                    PageMark::Artifact
                } else {
                    inherited
                });
            }
            Operator::BeginMarkedContentDict {
                tag,
                properties: value,
            } => {
                if tag == "Artifact" || matches!(inherited, PageMark::Artifact) {
                    stack.push(PageMark::Artifact);
                    continue;
                }
                let named;
                let value = if let Some(name) = value.as_name() {
                    let object = properties
                        .as_ref()
                        .and_then(Object::as_dict)
                        .and_then(|props| props.get(name))
                        .ok_or(FORM_EDIT)?;
                    named = pdf.resolve_object(object).map_err(|_| FORM_EDIT)?;
                    &named
                } else {
                    value.as_ref()
                };
                let mcid = value.as_dict().and_then(|dict| dict.get("MCID"));
                let mark = if let Some(mcid) = mcid {
                    let id = pdf
                        .resolve_object(mcid)
                        .map_err(|_| FORM_EDIT)?
                        .as_integer()
                        .and_then(|id| usize::try_from(id).ok())
                        .ok_or(FORM_EDIT)?;
                    PageMark::Content(id)
                } else {
                    inherited
                };
                stack.push(mark);
            }
            Operator::EndMarkedContent => {
                stack.pop().ok_or(FORM_EDIT)?;
            }
            _ => {}
        }
    }
    Ok(match stack.last() {
        Some(PageMark::Content(id)) => Some(*id),
        _ => None,
    })
}

fn transform_stream(transform: Matrix, data: &[u8]) -> Vec<u8> {
    if transform == Matrix::identity() {
        return data.to_vec();
    }
    let Matrix { a, b, c, d, e, f } = transform;
    let mut projected = assembly::form_transform([a, b, c, d, e, f]);
    projected.extend_from_slice(data);
    projected.extend_from_slice(b"\nQ\n");
    projected
}

fn inverse(transform: Matrix) -> Result<Matrix, &'static str> {
    let Matrix { a, b, c, d, e, f } = transform;
    let det = transform.determinant();
    if !det.is_finite() || det.abs() < 1e-12 {
        return Err(FORM_EDIT);
    }
    Ok(Matrix {
        a: d / det,
        b: -b / det,
        c: -c / det,
        d: a / det,
        e: (c * f - d * e) / det,
        f: (b * e - a * f) / det,
    })
}

fn regions(page: usize, bounds: &[[f32; 4]], color: Option<[f32; 3]>) -> RegionSet {
    let mut regions = RegionSet::new(page);
    for &[left, bottom, right, top] in bounds {
        regions.push(RedactionRegion::from_rect(left, bottom, right, top, color));
    }
    regions
}

#[derive(Clone, Copy, Default)]
struct TextSpacing<'a> {
    font: Option<&'a str>,
    size: f32,
    character: f32,
    word: f32,
}

impl TextSpacing<'_> {
    fn string_adjustment(self, bytes: &[u8], fonts: &PageFonts) -> Option<f64> {
        if bytes.is_empty() {
            return Some(0.0);
        }
        let name = self.font?;
        let font = fonts.metrics.get(name)?;
        if font.wmode != 0
            || !self.size.is_finite()
            || self.size <= 0.0
            || !fonts.can_prune(name)
            || !fonts.redaction_safe_show(name, bytes)
        {
            return None;
        }
        let decoded = fonts.decode(name, bytes);
        if decoded
            .iter()
            .map(|(_, source)| source.len())
            .sum::<usize>()
            != bytes.len()
        {
            return None;
        }
        let unit = 1000.0 / f64::from(self.size);
        decoded.into_iter().try_fold(0.0, |sum, (code, _)| {
            let word = if fonts.is_word_space(name, code) {
                self.word
            } else {
                0.0
            };
            let width = f64::from(fonts.width(name, code)) * f64::from(font.font_matrix_a) * 1000.0;
            let adjustment = sum - width - f64::from(self.character + word) * unit;
            adjustment.is_finite().then_some(adjustment)
        })
    }
}

fn remove_authored_shows(
    pdf: &PdfDocument,
    page: usize,
    resources: &Object,
    data: &[u8],
    selected: &HashSet<usize>,
    painted_text: Option<&str>,
) -> Result<(Vec<u8>, usize), &'static str> {
    let operators = parse_content_stream(data).map_err(|_| AUTHORED_EDIT)?;
    let ranges = text_command_ranges(data).map_err(|_| AUTHORED_EDIT)?.shows;
    let shows = operators
        .iter()
        .enumerate()
        .filter_map(|(index, op)| {
            matches!(
                op,
                Operator::Tj { .. }
                    | Operator::TJ { .. }
                    | Operator::Quote { .. }
                    | Operator::DoubleQuote { .. }
            )
            .then_some(index)
        })
        .collect::<Vec<_>>();
    if ranges.len() != shows.len() || selected.iter().any(|&index| index >= shows.len()) {
        return Err(AUTHORED_EDIT);
    }
    let targets = selected
        .iter()
        .map(|&ordinal| (shows[ordinal], &ranges[ordinal]))
        .collect::<HashMap<_, _>>();
    let mut keep_advance = HashSet::new();
    let mut cursor_needed = false;
    for (index, op) in operators.iter().enumerate().rev() {
        let selected = targets.contains_key(&index);
        match op {
            Operator::Tj { .. } | Operator::TJ { .. } => {
                if selected && cursor_needed {
                    keep_advance.insert(index);
                }
                cursor_needed |= !selected;
            }
            Operator::Quote { .. } | Operator::DoubleQuote { .. } => {
                if selected && cursor_needed {
                    keep_advance.insert(index);
                }
                cursor_needed = false;
            }
            Operator::Tm { .. }
            | Operator::Td { .. }
            | Operator::TD { .. }
            | Operator::TStar
            | Operator::BeginText
            | Operator::EndText => cursor_needed = false,
            _ => {}
        }
    }
    let fonts = (!keep_advance.is_empty())
        .then(|| load_page_fonts(pdf, page, resources, false, painted_text))
        .transpose()?
        .flatten();
    let graphics = resources
        .as_dict()
        .and_then(|dict| dict.get("ExtGState"))
        .map(|object| pdf.resolve_object(object))
        .transpose()
        .map_err(|_| AUTHORED_EDIT)?;
    let mut state = TextSpacing::default();
    let mut saved = Vec::new();
    let mut edits = Vec::with_capacity(targets.len());
    for (index, op) in operators.iter().enumerate() {
        match op {
            Operator::SaveState => saved.push(state),
            Operator::RestoreState => state = saved.pop().ok_or(AUTHORED_EDIT)?,
            Operator::Tf { font, size } => {
                state.font = Some(font);
                state.size = *size;
            }
            Operator::Tc { char_space } => state.character = *char_space,
            Operator::Tw { word_space } => state.word = *word_space,
            Operator::DoubleQuote {
                word_space,
                char_space,
                ..
            } => {
                state.word = *word_space;
                state.character = *char_space;
            }
            Operator::SetExtGState { dict_name } if fonts.is_some() => {
                if let Some(binding) = graphics
                    .as_ref()
                    .and_then(Object::as_dict)
                    .and_then(|dict| dict.get(dict_name))
                {
                    let fields = pdf.resolve_object(binding).map_err(|_| AUTHORED_EDIT)?;
                    if fields
                        .as_dict()
                        .is_some_and(|dict| dict.contains_key("Font"))
                    {
                        state.font = None;
                    }
                }
            }
            _ => {}
        }
        let Some(range) = targets.get(&index) else {
            continue;
        };
        let mut replacement = match op {
            Operator::Quote { .. } => "T*".to_owned(),
            Operator::DoubleQuote {
                word_space,
                char_space,
                ..
            } => {
                format!("{word_space:.12} Tw {char_space:.12} Tc T*")
            }
            _ => String::new(),
        };
        if keep_advance.contains(&index) {
            let fonts = fonts.as_ref().ok_or(AUTHORED_EDIT)?;
            let adjustment = match op {
                Operator::Tj { text }
                | Operator::Quote { text }
                | Operator::DoubleQuote { text, .. } => state.string_adjustment(text, fonts),
                Operator::TJ { array } => array.iter().try_fold(0.0, |distance, element| {
                    Some(
                        distance
                            + match element {
                                TextElement::String(text) => {
                                    state.string_adjustment(text, fonts)?
                                }
                                TextElement::Offset(offset) => f64::from(*offset),
                            },
                    )
                }),
                _ => None,
            }
            .filter(|adjustment| adjustment.is_finite())
            .ok_or(AUTHORED_EDIT)?;
            use std::fmt::Write as _;
            write!(replacement, " [{adjustment:.12}] TJ").map_err(|_| AUTHORED_EDIT)?;
        }
        edits.push(((*range).clone(), replacement));
    }
    Ok((
        rewrite_ranges(data, edits).map_err(|_| AUTHORED_EDIT)?,
        selected.len(),
    ))
}

fn load_page_fonts(
    pdf: &PdfDocument,
    page: usize,
    resources: &Object,
    composite_only: bool,
    painted_text: Option<&str>,
) -> Result<Option<PageFonts>, &'static str> {
    let mut extractor = TextExtractor::new();
    pdf.load_fonts_public(resources, &mut extractor)
        .map_err(|_| UNSUPPORTED)?;
    let available = extractor.get_font_set();
    let composite = available
        .iter()
        .filter(|(name, font)| {
            font.subtype == "Type0"
                && !source_font_encoding(pdf, resources, name)
                    .is_some_and(|encoding| encoding.as_name() == Some("Identity-H"))
        })
        .collect::<Vec<_>>();
    if composite_only && composite.is_empty() {
        return Ok(None);
    }
    if composite.len() > MAX_COMPOSITE_FONTS {
        return Err(UNSUPPORTED);
    }
    let displayed = if composite.is_empty() {
        String::new()
    } else {
        let mut text = pdf.extract_text(page).map_err(|_| UNSUPPORTED)?;
        if let Some(painted_text) = painted_text {
            text.push_str(painted_text);
        }
        text
    };
    let custom_fonts = composite
        .into_iter()
        .map(|(name, font)| {
            let resolved = (|| {
                if font.wmode != 0 {
                    return None;
                }
                let encoding = source_font_encoding(pdf, resources, name)?;
                let codes = composite_codes(pdf, &encoding, &displayed)?;
                let cids = codes
                    .into_iter()
                    .map(|code| ((code.code, code.hex_width / 2), code.cid))
                    .collect();
                Some(CompositeFont {
                    info: Arc::clone(font),
                    cids,
                })
            })();
            (name.clone(), resolved)
        })
        .collect();
    let metrics = available.into_iter().collect::<HashMap<_, _>>();
    Ok(Some(PageFonts {
        standard: FontInfoMetrics::new(metrics.clone()),
        composite: custom_fonts,
        metrics,
    }))
}

fn rewrite_stream(
    pdf: &PdfDocument,
    resources: &Object,
    content: &[u8],
    regions: &RegionSet,
    options: &RedactionOptions,
    composite_only: bool,
    painted_text: Option<&str>,
) -> Result<Option<(Vec<u8>, usize)>, &'static str> {
    let Some(fonts) = load_page_fonts(
        pdf,
        regions.page_index,
        resources,
        composite_only,
        painted_text,
    )?
    else {
        return Ok(None);
    };
    let (data, report) =
        redact_content_stream(content, regions, options, &fonts).map_err(|_| UNSUPPORTED)?;
    Ok(Some((data, report.glyphs_removed)))
}
