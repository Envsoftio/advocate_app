//! Resolve replacement text against the glyphs it covers in an in-memory page.
use super::{assembly, bbox, editable, reading_order, run_edit, working_spans, Rect};
use crate::matters::redaction_text::InlineActualTextEdit;
use crate::matters::redaction_vectors::text_command_ranges;
use pdf_oxide::{
    content::{parse_content_stream, Operator},
    layout::{TextChar, TextSpan},
    object::{Object, ObjectRef},
    optional_content::decode_pdf_text_string,
    structure::{McidScope, StructChild, StructElem},
    writer::ObjectSerializer,
    PdfDocument,
};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
};

const INVALID: &str = "Cannot isolate the glyphs behind PDF replacement text";
const MAX_FORMS: usize = 4_096;
const MAX_DEPTH: usize = 12;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ALTERNATES: usize = 32_768;
const MAX_SCALE_SHOWS: usize = 131_072;
const FIRST_MARK: u32 = 1_000_000;

pub(super) struct Alternate {
    pub text: String,
    pub structured: bool,
    pub retained_anchor: Option<assembly::RetainedAnchor>,
    source_order: Option<(Arc<[Rect]>, usize)>,
    inline: Option<(Arc<InlineMark>, Range<usize>)>,
    shared: Option<(SharedActualText, Range<usize>)>,
}

#[derive(Clone)]
struct SharedActualText {
    text: Arc<str>,
    anchors: usize,
    order: usize,
}

struct InlineMark {
    form: Option<ObjectRef>,
    property: usize,
    text: String,
}

pub(super) struct EditPage {
    pub spans: Vec<TextSpan>,
    pub chars: Vec<TextChar>,
    pub alternates: HashMap<usize, Alternate>,
    blocked: HashSet<usize>,
    fragmented: HashMap<usize, FragmentBounds>,
}

struct FragmentBounds {
    mark: u32,
    first: f64,
    last: f64,
}

impl EditPage {
    pub fn reading_order_split(
        &self,
        pdf: &PdfDocument,
        page: usize,
        index: usize,
    ) -> Option<reading_order::Split> {
        let target = self.alternates.get(&index)?;
        let (runs, position) = target.source_order.as_ref()?;
        if *position == 0 || *position + 1 == runs.len() {
            return None;
        }
        let span = &self.spans[index];
        let parts = match &target.inline {
            Some((mark, range)) => Some(reading_order::AlternateParts {
                original: &mark.text,
                before: mark.text.get(..range.start)?,
                after: mark.text.get(range.end..)?,
            }),
            None => None,
        };
        prove_split(
            pdf,
            page,
            span.mcid_scope.as_ref()?,
            span.mcid?,
            runs,
            *position,
            parts,
        )
    }

    pub fn text(&self, index: usize) -> &str {
        self.alternates
            .get(&index)
            .map_or(&self.spans[index].text, |alternate| &alternate.text)
    }

    pub fn text_for_span<'a>(&'a self, span: &'a TextSpan) -> &'a str {
        self.alternates
            .iter()
            .find(|(i, _)| std::ptr::eq(&self.spans[**i], span))
            .map_or(span.text.as_str(), |(_, value)| value.text.as_str())
    }

    pub fn editable(&self, index: usize, crop: Rect) -> bool {
        let span = &self.spans[index];
        !self.blocked.contains(&index)
            && editable(span, crop, run_edit::selection_bounds(span, &self.chars))
    }

    pub fn blocked_span(&self, span: &TextSpan) -> bool {
        self.blocked
            .iter()
            .any(|&i| std::ptr::eq(&self.spans[i], span))
    }

    pub fn fragment_removal_area(&self, index: usize) -> Option<Rect> {
        let bounds = self.fragmented.get(&index)?;
        let [_, bottom, _, top] = bbox(&self.spans[index]);
        // Remove by the interior of the outer glyphs so adjoining ink and
        // advance boxes cannot make the redactor take a neighboring glyph.
        Some([bounds.first - 0.001, bottom, bounds.last + 0.001, top])
    }

    pub fn hidden_in_selected_mark(&self, selected: usize, other: usize) -> bool {
        self.blocked.contains(&other)
            && self.fragmented.get(&other).is_some_and(|other| {
                self.fragmented
                    .get(&selected)
                    .is_some_and(|selected| selected.mark == other.mark)
            })
    }

    pub fn inline_edits(
        &self,
        selected: &[&TextSpan],
    ) -> Result<Vec<InlineActualTextEdit>, &'static str> {
        let mut groups = HashMap::<*const InlineMark, (&InlineMark, Vec<Range<usize>>)>::new();
        for (index, alternate) in &self.alternates {
            if !selected
                .iter()
                .any(|span| std::ptr::eq(*span, &self.spans[*index]))
            {
                continue;
            }
            if let Some((mark, cut)) = &alternate.inline {
                groups
                    .entry(Arc::as_ptr(mark))
                    .or_insert((mark, Vec::new()))
                    .1
                    .push(cut.clone());
            }
        }
        groups
            .into_values()
            .map(|(mark, mut cuts)| {
                cuts.sort_unstable_by_key(|cut| cut.start);
                let mut text = String::new();
                let mut cursor = 0;
                for cut in cuts {
                    text.push_str(mark.text.get(cursor..cut.start).ok_or(INVALID)?);
                    cursor = cut.end;
                }
                text.push_str(mark.text.get(cursor..).ok_or(INVALID)?);
                Ok(InlineActualTextEdit {
                    form: mark.form,
                    property: mark.property,
                    expected: mark.text.clone(),
                    remaining: text.trim_matches(['\r', '\n']).to_owned(),
                })
            })
            .collect()
    }

    pub fn shared_actual_text_edit(
        &self,
        index: usize,
        lines: &[&str],
    ) -> Option<assembly::SharedActualTextEdit> {
        let (shared, range) = self.alternates.get(&index)?.shared.as_ref()?;
        let mut replacement = shared.text.to_string();
        replacement.replace_range(range.clone(), &lines.join("\n"));
        Some(assembly::SharedActualTextEdit {
            original: shared.text.to_string(),
            replacement,
            anchors: shared.anchors,
        })
    }

    pub fn requires_individual_edit(&self, span: &TextSpan) -> bool {
        self.alternates.iter().any(|(index, alternate)| {
            (alternate.shared.is_some() || alternate.retained_anchor.is_some())
                && std::ptr::eq(&self.spans[*index], span)
        })
    }
}

#[derive(Clone, Copy, Default)]
struct Mark {
    mcid: Option<u32>,
    alternate: Option<u32>,
}

struct Runs {
    text: String,
    structured: bool,
    inline: Option<Arc<InlineMark>>,
    shared: Option<SharedActualText>,
    mcid: Option<u32>,
    scope: Option<McidScope>,
    mixed: bool,
    across_pages: bool,
}

struct Stream {
    data: Vec<u8>,
    forms: Vec<(String, assembly::EditedForm)>,
}

struct Analysis<'a> {
    pdf: &'a PdfDocument,
    tree: StructureAlternates,
    marked: HashMap<u32, Runs>,
    source_mcids: HashSet<u32>,
    forms: usize,
    decoded_bytes: usize,
    proof: Option<ShowProof>,
    scales: Option<Vec<f64>>,
}

struct ShowProof {
    scope: McidScope,
    mcid: u32,
    shows: Vec<(usize, usize)>,
    resources: Vec<Object>,
}

const FIRST_PROOF: u32 = FIRST_MARK + MAX_ALTERNATES as u32;
const FIRST_SCALE: u32 = FIRST_PROOF + MAX_ALTERNATES as u32;

fn prove_split(
    pdf: &PdfDocument,
    page: usize,
    scope: &McidScope,
    mcid: u32,
    runs: &[Rect],
    selected: usize,
    alternate: Option<reading_order::AlternateParts<'_>>,
) -> Option<reading_order::Split> {
    let form = match scope {
        McidScope::Page(number) if *number as usize == page => None,
        McidScope::Form(reference) => Some(*reference),
        _ => return None,
    };
    let resources = pdf.get_page_resources(page).ok()?;
    let data = pdf.get_page_content_data(page).ok()?;
    let mut analysis = Analysis::new(pdf, false);
    analysis.proof = Some(ShowProof {
        scope: scope.clone(),
        mcid,
        shows: Vec::new(),
        resources: Vec::new(),
    });
    let stream = analysis
        .stream(
            &data,
            &resources,
            &McidScope::Page(page.try_into().ok()?),
            Mark::default(),
            1.0,
            0,
        )
        .ok()??;
    let proof = analysis.proof?;
    let last = FIRST_PROOF + proof.shows.len() as u32;
    if analysis
        .source_mcids
        .iter()
        .any(|mark| (FIRST_PROOF..last).contains(mark))
    {
        return None;
    }
    let (view, _) = assembly::text_geometry_document(pdf, page, stream.data, &stream.forms).ok()?;
    let chars = view.extract_chars(0).ok()?;
    let baseline = |glyph: &TextChar| f64::from(glyph.origin_y) + 0.001;
    let shown = |glyph: &TextChar| {
        let index = glyph.mcid?.checked_sub(FIRST_PROOF)? as usize;
        proof.shows.get(index).copied()
    };
    let [left, bottom, right, top] = runs[selected];
    let mut selected_occurrences = chars.iter().filter_map(|glyph| {
        let (visit, _) = shown(glyph)?;
        ((left..=right).contains(&glyph_center(glyph)) && (bottom..top).contains(&baseline(glyph)))
            .then_some(visit)
    });
    let visit = selected_occurrences.next()?;
    if selected_occurrences.any(|other| other != visit) {
        return None;
    }
    let mut prefix = None;
    let mut suffix = None;
    let mut glyphs = chars
        .iter()
        .filter_map(|glyph| {
            let (occurrence, show) = shown(glyph)?;
            (occurrence == visit && !glyph.char.is_whitespace())
                .then(|| (baseline(glyph), glyph_center(glyph), show))
        })
        .collect::<Vec<_>>();
    glyphs.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    let mut matched = vec![false; glyphs.len()];
    for (position, &[left, bottom, right, top]) in runs.iter().enumerate() {
        let start = glyphs.partition_point(|&(y, _, _)| y < bottom);
        let end = glyphs.partition_point(|&(y, _, _)| y < top);
        let mut seen = false;
        for (&(_, x, show), matched) in glyphs[start..end].iter().zip(&mut matched[start..end]) {
            if !(left..=right).contains(&x) {
                continue;
            }
            if std::mem::replace(matched, true) {
                return None;
            }
            seen = true;
            if position > selected {
                suffix = Some(suffix.map_or(show, |other: usize| other.min(show)));
            } else {
                prefix = Some(prefix.map_or(show, |other: usize| other.max(show)));
            }
        }
        if !seen {
            return None;
        }
    }
    let suffix = suffix?;
    if prefix? >= suffix || matched.contains(&false) {
        return None;
    }
    let data = match form {
        Some(reference) => pdf.load_object(reference).ok()?.decode_stream_data().ok()?,
        None => data,
    };
    let (position, next_mcid) = reading_order::split_position(
        pdf,
        &data,
        proof.resources.get(visit)?,
        mcid,
        suffix,
        alternate,
    )?;
    Some(reading_order::Split {
        scope: form,
        position,
        next_mcid,
    })
}

#[derive(Default)]
struct StructureAlternates {
    anchors: HashMap<(McidScope, u32), Option<StructureAnchor>>,
    groups: Vec<StructureGroup>,
}

#[derive(Clone, Copy)]
struct StructureAnchor {
    group: usize,
    order: usize,
}

struct StructureGroup {
    text: Arc<str>,
    anchors: usize,
    first_page: Option<u32>,
    across_pages: bool,
}

pub(super) fn inspect(
    pdf: &PdfDocument,
    page: usize,
    geometry: assembly::Geometry,
) -> Result<EditPage, &'static str> {
    let resources = pdf.get_page_resources(page).map_err(|_| INVALID)?;
    let data = pdf.get_page_content_data(page).map_err(|_| INVALID)?;
    let mut analysis = Analysis::new(pdf, false);
    let base_scope = McidScope::Page(u32::try_from(page).map_err(|_| INVALID)?);
    let plan = analysis.stream(&data, &resources, &base_scope, Mark::default(), 1.0, 0);
    let plan = match plan {
        Err(_) if analysis.marked.is_empty() => None,
        other => other?,
    };
    if let Some(stream) = plan {
        if analysis
            .marked
            .keys()
            .any(|mcid| analysis.source_mcids.contains(mcid))
        {
            return Err(INVALID);
        }
        let (view, spans) = geometry_spans(pdf, page, geometry, &base_scope, stream)?;
        let chars = view.extract_chars(0).map_err(|_| INVALID)?;
        let spans = spans
            .into_iter()
            .flat_map(|span| {
                split_mixed_marks(&span, &chars, &analysis.marked).unwrap_or_else(|| vec![span])
            })
            .collect();
        return Ok(combine(spans, chars, analysis.marked));
    }
    Ok(EditPage {
        spans: working_spans(pdf, page, geometry).map_err(|_| INVALID)?,
        chars: pdf.extract_chars(page).map_err(|_| INVALID)?,
        alternates: HashMap::new(),
        blocked: HashSet::new(),
        fragmented: HashMap::new(),
    })
}

fn geometry_spans(
    pdf: &PdfDocument,
    page: usize,
    geometry: assembly::Geometry,
    base_scope: &McidScope,
    stream: Stream,
) -> Result<(PdfDocument, Vec<TextSpan>), &'static str> {
    let (view, origins) = assembly::text_geometry_document(pdf, page, stream.data, &stream.forms)?;
    let mut spans = working_spans(&view, 0, geometry).map_err(|_| INVALID)?;
    for span in &mut spans {
        span.mcid_scope = match span.mcid_scope {
            Some(McidScope::Form(id)) => {
                Some(McidScope::Form(*origins.get(&id.id).ok_or(INVALID)?))
            }
            Some(McidScope::Page(_)) => Some(base_scope.clone()),
            _ => span.mcid_scope.clone(),
        };
    }
    Ok((view, spans))
}

pub(super) fn run_scales(
    pdf: &PdfDocument,
    page: usize,
    geometry: assembly::Geometry,
) -> Option<Vec<(TextSpan, f64)>> {
    let resources = pdf.get_page_resources(page).ok()?;
    let data = pdf.get_page_content_data(page).ok()?;
    let base_scope = McidScope::Page(page.try_into().ok()?);
    let mut analysis = Analysis::new(pdf, true);
    let stream = analysis
        .stream(&data, &resources, &base_scope, Mark::default(), 1.0, 0)
        .ok()??;
    let scales = analysis.scales?;
    if !scales.iter().any(|scale| *scale != 1.0)
        || analysis
            .source_mcids
            .iter()
            .any(|mark| (FIRST_SCALE..FIRST_SCALE + scales.len() as u32).contains(mark))
    {
        return None;
    }
    let (_, spans) = geometry_spans(pdf, page, geometry, &base_scope, stream).ok()?;
    Some(
        spans
            .into_iter()
            .filter_map(|span| {
                let index = span.mcid?.checked_sub(FIRST_SCALE)? as usize;
                Some((span, *scales.get(index)?))
            })
            .collect(),
    )
}

impl StructureAlternates {
    fn walk(&mut self, element: &StructElem, inherited: Option<usize>) {
        let group = element
            .actual_text
            .as_deref()
            .filter(|text| !text.is_empty())
            .map(|text| {
                let group = self.groups.len();
                self.groups.push(StructureGroup {
                    text: Arc::from(text),
                    anchors: 0,
                    first_page: None,
                    across_pages: false,
                });
                group
            })
            .or(inherited);
        for kid in &element.children {
            match kid {
                StructChild::StructElem(element) => self.walk(element, group),
                StructChild::MarkedContentRef { scope, mcid, page } => {
                    let Some(group) = group else { continue };
                    let data = &mut self.groups[group];
                    data.across_pages |= *data.first_page.get_or_insert(*page) != *page;
                    let order = data.anchors;
                    data.anchors += 1;
                    self.anchors
                        .entry((scope.clone(), *mcid))
                        .and_modify(|mark| *mark = None)
                        .or_insert(Some(StructureAnchor { group, order }));
                }
                _ => {}
            }
        }
    }
}

impl<'a> Analysis<'a> {
    fn new(pdf: &'a PdfDocument, measuring_scales: bool) -> Self {
        let mut tree = StructureAlternates::default();
        if !measuring_scales {
            if let Some(structure) = pdf.structure_tree().ok().flatten() {
                for root in &structure.root_elements {
                    tree.walk(root, None);
                }
            }
        }
        Self {
            pdf,
            tree,
            marked: HashMap::new(),
            source_mcids: HashSet::new(),
            forms: 0,
            decoded_bytes: 0,
            proof: None,
            scales: measuring_scales.then(Vec::new),
        }
    }
    fn alternate(
        &mut self,
        text: String,
        structured: bool,
        mixed: bool,
        inline: Option<Arc<InlineMark>>,
    ) -> Result<u32, &'static str> {
        if self.marked.len() >= MAX_ALTERNATES {
            return Err(INVALID);
        }
        let index = FIRST_MARK
            .checked_add(self.marked.len() as u32)
            .ok_or(INVALID)?;
        self.marked.insert(
            index,
            Runs {
                text,
                structured,
                inline,
                shared: None,
                mcid: None,
                scope: None,
                mixed,
                across_pages: false,
            },
        );
        Ok(index)
    }

    fn property_mark(
        &mut self,
        tag: String,
        ordinal: usize,
        property: Object,
        lists: Option<&Object>,
        scope: &McidScope,
        inherited: Mark,
    ) -> Result<(Mark, Option<Vec<u8>>), &'static str> {
        let property = if let Some(name) = property.as_name() {
            lists
                .and_then(Object::as_dict)
                .and_then(|items| items.get(name))
                .map(|value| self.pdf.resolve_object(value))
                .transpose()
                .map_err(|_| INVALID)?
                .unwrap_or(Object::Null)
        } else {
            property
        };
        let mut fields = property.as_dict().cloned().unwrap_or_default();
        let mcid = fields
            .get("MCID")
            .and_then(Object::as_integer)
            .and_then(|n| u32::try_from(n).ok());
        self.source_mcids.extend(mcid);
        let mut mark = Mark {
            mcid: mcid.or(inherited.mcid),
            alternate: inherited.alternate,
        };
        if let Some(literal) = fields.remove("ActualText") {
            if self.scales.is_none() {
                let literal = self.pdf.resolve_object(&literal).map_err(|_| INVALID)?;
                let text = decode_pdf_text_string(literal.as_string().ok_or(INVALID)?);
                let source = Arc::new(InlineMark {
                    form: match scope {
                        McidScope::Form(id) => Some(*id),
                        _ => None,
                    },
                    property: ordinal,
                    text: text.clone(),
                });
                mark.alternate = Some(self.alternate(text, false, false, Some(source))?);
            }
            let serializer = ObjectSerializer::compact();
            let mut data = serializer.serialize(&Object::Name(tag));
            data.push(b' ');
            data.extend(serializer.serialize(&Object::Dictionary(fields)));
            data.extend_from_slice(b" BDC");
            return Ok((mark, Some(data)));
        }
        if let (Some(mcid), None) = (mcid, mark.alternate) {
            let key = (scope.clone(), mcid);
            if let Some(anchor) = self.tree.anchors.get(&key) {
                if let Some(anchor) = anchor {
                    let group = &self.tree.groups[anchor.group];
                    let across_pages = group.across_pages;
                    let shared = (group.anchors > 1 && !across_pages).then(|| SharedActualText {
                        text: Arc::clone(&group.text),
                        anchors: group.anchors,
                        order: anchor.order,
                    });
                    let id = self.alternate(group.text.to_string(), true, false, None)?;
                    let run = self.marked.get_mut(&id).ok_or(INVALID)?;
                    run.shared = shared;
                    run.across_pages = across_pages;
                    mark.alternate = Some(id);
                } else {
                    mark.alternate = Some(self.alternate(String::new(), true, true, None)?);
                }
            }
        }
        Ok((mark, None))
    }

    fn form(
        &mut self,
        object: &Object,
        inherited: &Object,
        mark: Mark,
        horizontal_scale: f64,
        depth: usize,
    ) -> Result<Option<assembly::EditedForm>, &'static str> {
        let resolved = self.pdf.resolve_object(object).map_err(|_| INVALID)?;
        let Some(fields) = resolved.as_dict() else {
            return Ok(None);
        };
        if fields.get("Subtype").and_then(Object::as_name) != Some("Form") {
            return Ok(None);
        }
        self.forms += 1;
        if self.forms > MAX_FORMS || depth >= MAX_DEPTH {
            return Err(INVALID);
        }
        let original = object.as_reference().ok_or(INVALID)?;
        let local = fields
            .get("Resources")
            .map(|value| self.pdf.resolve_object(value))
            .transpose()
            .map_err(|_| INVALID)?;
        let local = local
            .as_ref()
            .filter(|object| !matches!(object, Object::Null))
            .unwrap_or(inherited);
        let content = resolved.decode_stream_data().map_err(|_| INVALID)?;
        let Some(form) = self.stream(
            &content,
            local,
            &McidScope::Form(original),
            mark,
            horizontal_scale,
            depth + 1,
        )?
        else {
            return Ok(None);
        };
        let mut fields = fields.clone();
        for key in [
            "Length",
            "Filter",
            "DecodeParms",
            "StructParents",
            "StructParent",
        ] {
            fields.remove(key);
        }
        if !form.forms.is_empty() {
            fields.insert("Resources".into(), local.clone());
        }
        Ok(Some(assembly::EditedForm {
            original,
            fields,
            data: form.data,
            children: form.forms,
        }))
    }

    fn stream(
        &mut self,
        data: &[u8],
        resources: &Object,
        scope: &McidScope,
        enclosing: Mark,
        horizontal_scale: f64,
        depth: usize,
    ) -> Result<Option<Stream>, &'static str> {
        // Use source offsets only for streams that can affect the selected
        // inspection or invoke a Form that does.
        let can_draw_forms = resources
            .as_dict()
            .is_some_and(|items| items.contains_key("XObject"));
        let measuring_scales = self.scales.is_some();
        let might_measure_scales =
            measuring_scales && (horizontal_scale != 1.0 || data.windows(2).any(|w| w == b"Tz"));
        if !might_measure_scales
            && enclosing.alternate.is_none()
            && !data.windows(3).any(|w| w == b"BDC")
            && !(can_draw_forms && data.windows(2).any(|w| w == b"Do"))
        {
            return Ok(None);
        }
        self.decoded_bytes = self.decoded_bytes.checked_add(data.len()).ok_or(INVALID)?;
        if depth > MAX_DEPTH || self.decoded_bytes > MAX_BYTES {
            return Err(INVALID);
        }
        let operators = parse_content_stream(data).map_err(|_| INVALID)?;
        let measure_stream = measuring_scales
            && (horizontal_scale != 1.0
                || operators
                    .iter()
                    .any(|op| matches!(op, Operator::Tz { scale } if *scale != 100.0)));
        let mut horizontal_scale = horizontal_scale;
        let mut saved_scales = Vec::new();
        let commands = text_command_ranges(data)?;
        let mut properties = commands
            .properties
            .into_iter()
            .filter(|range| data[range.clone()].ends_with(b"BDC"))
            .enumerate();
        let mut shows = commands.shows.into_iter().enumerate();
        let mut draws = commands.draws.into_iter();
        let resource = |key| {
            resources
                .as_dict()
                .and_then(|fields| fields.get(key))
                .map(|source| self.pdf.resolve_object(source))
                .transpose()
                .map_err(|_| INVALID)
        };
        let property_lists = resource("Properties")?;
        let xobjects = resource("XObject")?;
        let xobjects = xobjects.as_ref().and_then(Object::as_dict);
        let prefix = xobjects.and_then(|entries| {
            (0..=entries.len())
                .map(|n| format!("MDTextGeometry{n}_"))
                .find(|prefix| entries.keys().all(|key| !key.starts_with(prefix)))
        });
        let mut marks = vec![enclosing];
        let mut patches: Vec<(Range<usize>, Vec<u8>)> = Vec::new();
        let mut forms = Vec::new();
        let proof_visit = self
            .proof
            .as_mut()
            .filter(|proof| proof.scope == *scope)
            .map(|proof| {
                let visit = proof.resources.len();
                proof.resources.push(resources.clone());
                visit
            });
        for operator in operators {
            let current = *marks.last().ok_or(INVALID)?;
            match operator {
                Operator::SaveState if measuring_scales => saved_scales.push(horizontal_scale),
                Operator::RestoreState if measuring_scales => {
                    horizontal_scale = saved_scales.pop().ok_or(INVALID)?;
                }
                Operator::Tz { scale } if measuring_scales => {
                    horizontal_scale = f64::from(scale) / 100.0;
                }
                Operator::BeginMarkedContent { .. } => marks.push(current),
                Operator::BeginMarkedContentDict {
                    tag,
                    properties: property,
                } => {
                    let (ordinal, range) = properties.next().ok_or(INVALID)?;
                    let (mark, replacement) = self.property_mark(
                        tag,
                        ordinal,
                        *property,
                        property_lists.as_ref(),
                        scope,
                        current,
                    )?;
                    if let Some(replacement) = replacement {
                        patches.push((range, replacement));
                    }
                    marks.push(mark);
                }
                Operator::EndMarkedContent => {
                    marks.pop();
                }
                Operator::Tj { .. }
                | Operator::TJ { .. }
                | Operator::Quote { .. }
                | Operator::DoubleQuote { .. } => {
                    let (show, range) = shows.next().ok_or(INVALID)?;
                    if let Some(scales) = &mut self.scales {
                        if measure_stream {
                            if scales.len() >= MAX_SCALE_SHOWS {
                                return Err(INVALID);
                            }
                            let id = FIRST_SCALE + scales.len() as u32;
                            scales.push(horizontal_scale);
                            patches.push(marked_show(data, range, id));
                        }
                        continue;
                    }
                    let proof_mark = match (self.proof.as_mut(), proof_visit) {
                        (Some(proof), Some(visit)) if current.mcid == Some(proof.mcid) => {
                            if proof.shows.len() >= MAX_ALTERNATES {
                                return Err(INVALID);
                            }
                            let id = FIRST_PROOF + proof.shows.len() as u32;
                            proof.shows.push((visit, show));
                            Some(id)
                        }
                        _ => None,
                    };
                    if let Some(index) = current.alternate.filter(|_| proof_mark.is_none()) {
                        let marked = self.marked.get_mut(&index).ok_or(INVALID)?;
                        if let Some(prior) = &marked.scope {
                            marked.mixed |= prior != scope || marked.mcid != current.mcid;
                        } else {
                            marked.scope = Some(scope.clone());
                            marked.mcid = current.mcid;
                        }
                    }
                    if let Some(id) = proof_mark.or(current.alternate) {
                        patches.push(marked_show(data, range, id));
                    }
                }
                Operator::Do { name } => {
                    let range = draws.next().ok_or(INVALID)?;
                    let Some(object) = xobjects.and_then(|objects| objects.get(&name)) else {
                        continue;
                    };
                    if let Some(form) =
                        self.form(object, resources, current, horizontal_scale, depth)?
                    {
                        let target =
                            format!("{}{}", prefix.as_deref().ok_or(INVALID)?, forms.len());
                        patches.push((range, format!("/{target} Do").into_bytes()));
                        forms.push((target, form));
                    }
                }
                _ => {}
            }
        }
        if properties.next().is_some() || draws.next().is_some() || shows.next().is_some() {
            return Err(INVALID);
        }
        if patches.is_empty() {
            return Ok(None);
        }
        let mut output = Vec::with_capacity(data.len());
        let mut cursor = 0;
        for (range, patch) in patches {
            if range.start < cursor {
                return Err(INVALID);
            }
            output.extend_from_slice(&data[cursor..range.start]);
            output.extend(patch);
            cursor = range.end;
        }
        output.extend_from_slice(&data[cursor..]);
        Ok(Some(Stream {
            data: output,
            forms,
        }))
    }
}

fn marked_show(data: &[u8], range: Range<usize>, id: u32) -> (Range<usize>, Vec<u8>) {
    let mut paint = format!("/Span <</MCID {id}>> BDC\n").into_bytes();
    paint.extend_from_slice(&data[range.clone()]);
    paint.extend_from_slice(b"\nEMC");
    (range, paint)
}

fn same_line(left: &TextSpan, right: &TextSpan) -> bool {
    let a = bbox(left);
    let b = bbox(right);
    (a[1] - b[1]).abs() <= 0.5
        && (left.font_size - right.font_size).abs() <= 0.01
        && left.font_name == right.font_name
        && left.color.r == right.color.r
        && left.color.g == right.color.g
        && left.color.b == right.color.b
        && left.rotation_degrees == right.rotation_degrees
        && left.wmode == right.wmode
        && left.mcid_scope == right.mcid_scope
        && a[0].max(b[0]) - a[2].min(b[2]) <= f64::from(left.font_size) * 0.3
}

fn slices(text: &str, glyphs: &[usize]) -> Option<Vec<Range<usize>>> {
    if glyphs.len() == 1 {
        return Some(vec![0..text.len()]);
    }
    let mut lines = vec![0];
    lines.extend(text.match_indices('\n').map(|(index, _)| index));
    lines.push(text.len());
    if lines.len() == glyphs.len() + 1 {
        return Some(lines.windows(2).map(|pair| pair[0]..pair[1]).collect());
    }

    // Each visual line consumes the characters backed by its glyphs. Only
    // the last line can receive additional text without inventing a boundary.
    let mut offsets = text
        .char_indices()
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if offsets.len() < glyphs.iter().sum() {
        return None;
    }
    offsets.push(text.len());
    let mut cut = Vec::new();
    let mut count = 0;
    for glyphs in &glyphs[..glyphs.len() - 1] {
        cut.push(offsets[count]..offsets[count + glyphs]);
        count += glyphs;
    }
    cut.push(offsets[count]..text.len());
    Some(cut)
}

struct Fragment {
    span: TextSpan,
    text: Option<String>,
    first_center: f64,
    last_center: f64,
}

fn retained_anchor(run: &Runs, first: bool) -> Option<assembly::RetainedAnchor> {
    run.mcid.map(|_| {
        if first {
            assembly::RetainedAnchor::InsertBefore
        } else {
            assembly::RetainedAnchor::InsertAfter
        }
    })
}

fn fragment_span(span: &TextSpan, glyphs: &[&TextChar], text: Option<&[char]>) -> Option<Fragment> {
    let first = glyphs.first()?;
    let last = glyphs.last()?;
    let mut fragment = span.clone();
    fragment.text = glyphs.iter().map(|glyph| glyph.char).collect();
    fragment.bbox.x = first.bbox.x;
    fragment.bbox.width = (last.bbox.x + last.bbox.width) - first.bbox.x;
    fragment.char_widths = glyphs.iter().map(|glyph| glyph.advance_width).collect();
    fragment.char_x_offsets = glyphs.iter().map(|glyph| glyph.origin_x).collect();
    Some(Fragment {
        span: fragment,
        text: text
            .map(|chars| chars.iter().collect::<String>())
            .filter(|text| !text.trim().is_empty()),
        first_center: glyph_center(first),
        last_center: glyph_center(last),
    })
}

fn split_mixed_marks(
    span: &TextSpan,
    chars: &[TextChar],
    marked: &HashMap<u32, Runs>,
) -> Option<Vec<TextSpan>> {
    if span.wmode != 0 || span.rotation_degrees.abs() >= 0.001 {
        return None;
    }
    let [left, bottom, right, _] = bbox(span);
    let start = chars.partition_point(|glyph| f64::from(glyph.bbox.y) > bottom + 0.01);
    let glyphs = chars[start..]
        .iter()
        .take_while(|glyph| f64::from(glyph.bbox.y) >= bottom - 0.01)
        .filter(|glyph| {
            let x = glyph_center(glyph);
            x >= left && x <= right
        })
        .collect::<Vec<_>>();
    if !glyphs.windows(2).any(|pair| pair[0].mcid != pair[1].mcid)
        || !glyphs
            .iter()
            .any(|glyph| glyph.mcid.is_some_and(|id| marked.contains_key(&id)))
        || !glyphs.iter().map(|glyph| glyph.char).eq(span.text.chars())
    {
        return None;
    }
    // The span extractor can join adjacent glyphs from different fonts across
    // a marked-content boundary. Recover that boundary from the characters.
    glyphs
        .chunk_by(|a, b| a.mcid == b.mcid)
        .map(|glyphs| {
            let glyph = glyphs.first()?;
            let id = glyph.mcid;
            let mut part = fragment_span(span, glyphs, None)?.span;
            part.mcid = id;
            part.mcid_scope = if let Some(run) = id.and_then(|id| marked.get(&id)) {
                run.scope.clone()
            } else if id == span.mcid || matches!(span.mcid_scope, Some(McidScope::Page(_)) | None)
            {
                span.mcid_scope.clone()
            } else {
                return None;
            };
            part.font_name = glyph.font_name.clone();
            part.font_size = glyph.font_size;
            part.font_weight = glyph.font_weight;
            part.is_italic = glyph.is_italic;
            part.is_monospace = glyph.is_monospace;
            part.color = glyph.color;
            Some(part)
        })
        .collect()
}

fn glyph_center(glyph: &TextChar) -> f64 {
    f64::from(glyph.bbox.x + glyph.bbox.width * 0.5)
}

fn fragment_short_alternate(
    spans: &[TextSpan],
    indexes: &[usize],
    glyphs: &[&TextChar],
    alternate: &str,
) -> Option<Vec<Vec<Fragment>>> {
    let alternate = alternate.chars().collect::<Vec<_>>();
    let mut remaining = alternate.as_slice();
    let mut fragments = Vec::with_capacity(indexes.len());
    for &index in indexes {
        let span = &spans[index];
        if span.wmode != 0 || span.rotation_degrees.abs() >= 0.001 {
            return None;
        }
        let [left, bottom, right, top] = bbox(span);
        let line = glyphs
            .iter()
            .copied()
            .filter(|glyph| {
                let x = glyph_center(glyph);
                x >= left && x <= right && (bottom..top).contains(&(f64::from(glyph.bbox.y) + 0.01))
            })
            .collect::<Vec<_>>();
        if !line.iter().map(|glyph| glyph.char).eq(span.text.chars())
            || !line.windows(2).all(|pair| {
                pair[0].bbox.x < pair[1].bbox.x
                    && pair[0].bbox.x + pair[0].bbox.width <= pair[1].bbox.x + 0.001
            })
        {
            return None;
        }
        // Anchor any equal edges to their original glyphs. The intervening
        // painted glyphs may have no position in the replacement string.
        let prefix = line
            .iter()
            .zip(remaining)
            .take_while(|(glyph, text)| glyph.char == **text)
            .count();
        if prefix == line.len() {
            fragments.push(vec![fragment_span(
                span,
                &line,
                Some(&remaining[..prefix]),
            )?]);
            remaining = &remaining[prefix..];
            continue;
        }
        let suffix = line[prefix..]
            .iter()
            .rev()
            .zip(remaining[prefix..].iter().rev())
            .take_while(|(glyph, text)| glyph.char == **text)
            .count();
        let tail = line.len() - suffix;
        let available = remaining.len() - prefix - suffix;
        let head = prefix.max(tail.min(available));
        // Excess alternate characters can share the preceding glyph's origin
        // when the head and matching suffix exhaust the painted run.
        let head_text = if suffix != 0 && head == tail {
            &remaining[..remaining.len() - suffix]
        } else {
            &remaining[..head]
        };
        let mut line_fragments = Vec::new();
        if let Some(item) = fragment_span(span, &line[..head], Some(head_text)) {
            line_fragments.push(item);
        }
        if let Some(item) = fragment_span(span, &line[head..tail], None) {
            line_fragments.push(item);
        }
        if suffix != 0 {
            line_fragments.push(fragment_span(
                span,
                &line[tail..],
                Some(&remaining[remaining.len() - suffix..]),
            )?);
            remaining = &[];
        } else {
            remaining = &remaining[head..];
        }
        fragments.push(line_fragments);
    }
    remaining.is_empty().then_some(fragments)
}

fn combine(spans: Vec<TextSpan>, chars: Vec<TextChar>, marked: HashMap<u32, Runs>) -> EditPage {
    // Keep the real glyph text, font and origin for removal and replacement.
    // The alternate string only governs the selection's displayed text.
    let mut result: Vec<TextSpan> = Vec::new();
    let mut spans_by_mark = HashMap::<u32, Vec<usize>>::new();
    let mut blocked = HashSet::new();
    for mut span in spans {
        let Some(mark) = span.mcid.filter(|id| marked.contains_key(id)) else {
            result.push(span);
            continue;
        };
        let run = &marked[&mark];
        span.mcid = run.mcid;
        if span.text.trim().is_empty() {
            continue;
        }
        if run.mixed || run.text.trim().is_empty() {
            blocked.insert(result.len());
            result.push(span);
            continue;
        }
        let runs = spans_by_mark.entry(mark).or_default();
        if let Some(&index) = runs.last() {
            let prior = &mut result[index];
            if same_line(prior, &span) {
                prior.text.push_str(&span.text);
                prior.bbox = prior.bbox.union(&span.bbox);
                prior.sequence = prior.sequence.min(span.sequence);
                continue;
            }
        }
        runs.push(result.len());
        result.push(span);
    }

    let mut alternates = HashMap::new();
    let mut fragments = HashMap::new();
    let mut marked_glyphs = HashMap::<u32, Vec<&TextChar>>::new();
    for glyph in &chars {
        if let Some(mark) = glyph.mcid.filter(|id| spans_by_mark.contains_key(id)) {
            marked_glyphs.entry(mark).or_default().push(glyph);
        }
    }
    let mut shared_spans = HashMap::<*const str, Vec<(SharedActualText, usize)>>::new();
    for (mark, mut spans) in spans_by_mark {
        let run = &marked[&mark];
        if let Some(shared) = &run.shared {
            if let [index] = spans.as_slice() {
                shared_spans
                    .entry(Arc::as_ptr(&shared.text))
                    .or_default()
                    .push((shared.clone(), *index));
            } else {
                blocked.extend(spans);
            }
            continue;
        }
        spans.sort_unstable_by_key(|&index| result[index].sequence);
        let multiple = spans.len() > 1;
        let order: Option<Arc<[Rect]>> = (spans.len() > 2 && run.mcid.is_some())
            .then(|| spans.iter().map(|&index| bbox(&result[index])).collect());
        if run.across_pages
            || (multiple && run.structured && matches!(run.scope, Some(McidScope::Form(_))))
        {
            // The reference exposes the painted runs for a structure alternate
            // across pages or over several lines in a Form.
            for (position, index) in spans.into_iter().enumerate() {
                alternates.insert(
                    index,
                    Alternate {
                        text: result[index].text.clone(),
                        structured: !run.across_pages,
                        retained_anchor: retained_anchor(run, position == 0),
                        source_order: order.as_ref().map(|order| (Arc::clone(order), position)),
                        inline: None,
                        shared: None,
                    },
                );
            }
            continue;
        }
        let glyphs = spans
            .iter()
            .map(|&index| result[index].text.chars().count())
            .collect::<Vec<_>>();
        let cut = slices(&run.text, &glyphs);
        let Some(cut) = cut else {
            let parts = marked_glyphs
                .get(&mark)
                .and_then(|glyphs| fragment_short_alternate(&result, &spans, glyphs, &run.text));
            if let Some(parts) = parts {
                fragments.extend(
                    spans.into_iter().zip(
                        parts
                            .into_iter()
                            .enumerate()
                            .map(|(position, part)| (mark, position, part)),
                    ),
                );
            } else {
                blocked.extend(spans);
            }
            continue;
        };
        for (position, (index, range)) in spans.into_iter().zip(cut).enumerate() {
            let text = run.text[range.clone()].to_owned();
            if text.trim().is_empty() {
                blocked.insert(index);
                continue;
            }
            alternates.insert(
                index,
                Alternate {
                    text,
                    structured: run.structured,
                    source_order: order.as_ref().map(|order| (Arc::clone(order), position)),
                    retained_anchor: multiple
                        .then(|| retained_anchor(run, position == 0))
                        .flatten(),
                    inline: run
                        .inline
                        .as_ref()
                        .map(|source| (Arc::clone(source), range)),
                    shared: None,
                },
            );
        }
    }
    for (_, mut members) in shared_spans {
        members.sort_unstable_by_key(|(shared, _)| shared.order);
        let first = &members[0].0;
        let glyphs = members
            .iter()
            .map(|(_, index)| result[*index].text.chars().count())
            .collect::<Vec<_>>();
        let unique = members
            .windows(2)
            .all(|pair| pair[0].0.order != pair[1].0.order);
        let cut = (members.len() == first.anchors && unique)
            .then(|| slices(&first.text, &glyphs))
            .flatten();
        let Some(cut) = cut else {
            blocked.extend(members.into_iter().map(|(_, index)| index));
            continue;
        };
        for ((shared, index), range) in members.into_iter().zip(cut) {
            let text = shared.text[range.clone()].to_owned();
            if text.trim().is_empty() {
                blocked.insert(index);
                continue;
            }
            alternates.insert(
                index,
                Alternate {
                    text,
                    structured: true,
                    source_order: None,
                    retained_anchor: None,
                    inline: None,
                    shared: Some((shared, range)),
                },
            );
        }
    }
    let mut page = EditPage {
        spans: Vec::with_capacity(result.len()),
        chars,
        alternates: HashMap::new(),
        blocked: HashSet::new(),
        fragmented: HashMap::new(),
    };
    for (index, span) in result.into_iter().enumerate() {
        if let Some((mark, line, parts)) = fragments.remove(&index) {
            let run = &marked[&mark];
            for (position, part) in parts.into_iter().enumerate() {
                let index = page.spans.len();
                page.fragmented.insert(
                    index,
                    FragmentBounds {
                        mark,
                        first: part.first_center,
                        last: part.last_center,
                    },
                );
                if let Some(text) = part.text {
                    page.alternates.insert(
                        index,
                        Alternate {
                            text,
                            structured: run.structured,
                            source_order: None,
                            retained_anchor: retained_anchor(run, line == 0 && position == 0),
                            inline: None,
                            shared: None,
                        },
                    );
                } else {
                    page.blocked.insert(index);
                }
                page.spans.push(part.span);
            }
        } else {
            let new = page.spans.len();
            if let Some(alternate) = alternates.remove(&index) {
                page.alternates.insert(new, alternate);
            }
            if blocked.contains(&index) {
                page.blocked.insert(new);
            }
            page.spans.push(span);
        }
    }
    page
}
