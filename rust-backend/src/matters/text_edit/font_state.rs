//! Resolve inherited text state for PDF Oxide inspection and edits.
use super::assembly::{Dictionary, ReboundTextForm, ReboundTextStream};
use crate::matters::redaction_vectors::{rewrite_ranges, text_command_ranges};
use pdf_oxide::{
    content::{parse_content_stream, Operator},
    object::{Object, ObjectRef},
    writer::ObjectSerializer,
    PdfDocument,
};
use std::collections::{HashMap, HashSet};

const INVALID: &str = "Unable to resolve the font selected by the PDF graphics state";
const MAX_FORM_DEPTH: usize = 12;
const MAX_FORM_CONTEXTS: usize = 4_096;
const MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, PartialEq)]
struct FontBinding {
    font: Object,
    size: f64,
}

#[derive(Clone, Copy, PartialEq)]
enum FontSelection<'a> {
    Explicit(&'a str, f64),
    Bound(&'a FontBinding),
}

impl FontSelection<'_> {
    fn binding(self, fonts: &Dictionary) -> Option<FontBinding> {
        match self {
            Self::Explicit(name, size) => Some(FontBinding {
                font: fonts.get(name)?.clone(),
                size,
            }),
            Self::Bound(binding) => Some(binding.clone()),
        }
    }

    fn command(self, fonts: &mut Dictionary, font_serial: &mut usize) -> String {
        let (name, size) = match self {
            Self::Explicit(name, size) => (name.to_owned(), size),
            Self::Bound(binding) => {
                let name = fonts
                    .iter()
                    .filter(|(_, font)| *font == &binding.font)
                    .map(|(name, _)| name.as_str())
                    .min()
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        let name = next_name(fonts, "MatterDraftGraphicsFont", font_serial);
                        fonts.insert(name.clone(), binding.font.clone());
                        name
                    });
                (name, binding.size)
            }
        };
        let name = ObjectSerializer::compact().serialize_to_string(&Object::Name(name));
        format!("{name} {size:.12} Tf")
    }
}

fn next_name(dictionary: &Dictionary, prefix: &str, serial: &mut usize) -> String {
    loop {
        let name = format!("{prefix}{serial}");
        *serial += 1;
        if !dictionary.contains_key(&name) {
            return name;
        }
    }
}

fn dictionary(pdf: &PdfDocument, parent: &Dictionary, name: &str) -> Option<Dictionary> {
    let value = pdf.resolve_object(parent.get(name)?).ok()?;
    value.as_dict().cloned()
}

fn graphics_fonts(pdf: &PdfDocument, resources: &Dictionary) -> HashMap<String, FontBinding> {
    let Some(graphics) = dictionary(pdf, resources, "ExtGState") else {
        return HashMap::new();
    };
    graphics
        .into_iter()
        .filter_map(|(name, binding)| {
            let state = pdf.resolve_object(&binding).ok()?;
            let font = pdf.resolve_object(state.as_dict()?.get("Font")?).ok()?;
            let [font, size] = font.as_array()?.as_slice() else {
                return None;
            };
            let size = match pdf.resolve_object(size).ok()? {
                Object::Integer(size) => size as f64,
                Object::Real(size) => size,
                _ => return None,
            };
            size.is_finite().then(|| {
                (
                    name,
                    FontBinding {
                        font: font.clone(),
                        size,
                    },
                )
            })
        })
        .collect()
}

type ResourceOwner = Option<ObjectRef>;

struct CachedForm {
    inherited: InheritedText,
    rewritten: Option<usize>,
}

#[derive(PartialEq)]
struct InheritedText {
    font: Option<FontBinding>,
    horizontal_scale: f32,
}

struct FontStreams<'a> {
    pdf: &'a PdfDocument,
    forms: Vec<ReboundTextForm>,
    cache: HashMap<(ObjectRef, ResourceOwner), Vec<CachedForm>>,
    active: HashSet<ObjectRef>,
    contexts: usize,
    decoded: usize,
}

impl FontStreams<'_> {
    fn form(
        &mut self,
        binding: &Object,
        parent_resources: &Dictionary,
        parent_owner: ResourceOwner,
        inherited: InheritedText,
        depth: usize,
    ) -> Result<Option<usize>, &'static str> {
        let Some(reference) = binding.as_reference() else {
            return Ok(None);
        };
        if self.active.contains(&reference) || depth >= MAX_FORM_DEPTH {
            return Ok(None);
        }
        let Some(form) = self.pdf.resolve_object(binding).ok() else {
            return Ok(None);
        };
        let Some(fields) = form.as_dict() else {
            return Ok(None);
        };
        if fields.get("Subtype").and_then(Object::as_name) != Some("Form") {
            return Ok(None);
        }
        let own_resources = dictionary(self.pdf, fields, "Resources");
        let owner = if own_resources.is_some() {
            Some(reference)
        } else {
            parent_owner
        };
        let cache_key = (reference, owner);
        if let Some(context) = self
            .cache
            .get(&cache_key)
            .into_iter()
            .flatten()
            .find(|context| context.inherited == inherited)
        {
            return Ok(context.rewritten);
        }
        if self.contexts >= MAX_FORM_CONTEXTS || self.decoded >= MAX_DECODED_BYTES {
            return Ok(None);
        }
        let data = form.decode_stream_data().map_err(|_| INVALID)?;
        self.decoded = self.decoded.checked_add(data.len()).ok_or(INVALID)?;
        if self.decoded > MAX_DECODED_BYTES {
            return Ok(None);
        }
        self.contexts += 1;
        self.active.insert(reference);
        let effective = own_resources.as_ref().unwrap_or(parent_resources);
        let stream = self.stream(data, effective, owner, &inherited, depth + 1);
        self.active.remove(&reference);
        let rewritten = stream?.map(|stream| {
            // Local text operators can make distinct inherited contexts
            // produce the same stream and the same child bindings.
            if let Some(index) = self
                .cache
                .get(&cache_key)
                .into_iter()
                .flatten()
                .filter_map(|context| context.rewritten)
                .find(|&index| {
                    let previous = &self.forms[index].stream;
                    previous.data == stream.data
                        && previous.resources == stream.resources
                        && previous.forms == stream.forms
                })
            {
                return index;
            }
            let mut fields = fields.clone();
            for key in ["Length", "Filter", "DecodeParms", "Resources"] {
                fields.remove(key);
            }
            let index = self.forms.len();
            self.forms.push(ReboundTextForm {
                original: reference,
                fields,
                stream,
            });
            index
        });
        self.cache.entry(cache_key).or_default().push(CachedForm {
            inherited,
            rewritten,
        });
        Ok(rewritten)
    }

    fn stream(
        &mut self,
        data: Vec<u8>,
        resources: &Dictionary,
        owner: ResourceOwner,
        inherited: &InheritedText,
        depth: usize,
    ) -> Result<Option<ReboundTextStream>, &'static str> {
        let graphics = graphics_fonts(self.pdf, resources);
        let xobjects =
            resources.contains_key("XObject") && data.windows(2).any(|token| token == b"Do");
        if graphics.is_empty()
            && inherited.font.is_none()
            && inherited.horizontal_scale == 100.0
            && !xobjects
        {
            return Ok(None);
        }
        let xobjects = if xobjects {
            dictionary(self.pdf, resources, "XObject").unwrap_or_default()
        } else {
            Dictionary::new()
        };
        let mut fonts = dictionary(self.pdf, resources, "Font").unwrap_or_default();
        let mut font_serial = 0;
        let mut form_serial = 0;
        let mut form_names = HashMap::new();
        let mut forms = Vec::new();
        let operators = parse_content_stream(&data).map_err(|_| INVALID)?;
        let ranges = text_command_ranges(&data)?;
        let mut shows = ranges.shows.into_iter();
        let mut draws = ranges.draws.into_iter();
        let mut active = inherited.font.as_ref().map(FontSelection::Bound);
        let mut pending = active.is_some();
        let mut horizontal_scale = inherited.horizontal_scale;
        let mut scale_pending = horizontal_scale != 100.0;
        let mut saved = Vec::new();
        let mut edits = Vec::new();
        let mut font_edits = 0;
        for operator in &operators {
            match operator {
                Operator::SaveState => {
                    saved.push((
                        (active, pending, font_edits),
                        (horizontal_scale, scale_pending),
                    ));
                }
                Operator::RestoreState => {
                    let ((restored, was_pending, previous_edits), scale) =
                        saved.pop().ok_or(INVALID)?;
                    (horizontal_scale, scale_pending) = scale;
                    // The removal engine tracks the CTM at Q. Restored font
                    // selections need to be explicit when a frame changed them.
                    let changed = active != restored
                        && (font_edits != previous_edits
                            || matches!(active, Some(FontSelection::Bound(_)))
                            || matches!(restored, Some(FontSelection::Bound(_))));
                    (active, pending) = (restored, was_pending || changed);
                }
                Operator::Tf { font, size } => {
                    active = Some(FontSelection::Explicit(font, (*size).into()));
                    pending = false;
                }
                Operator::Tz { scale } => {
                    horizontal_scale = *scale;
                    scale_pending = false;
                }
                Operator::SetExtGState { dict_name } => {
                    if let Some(font) = graphics.get(dict_name) {
                        active = Some(FontSelection::Bound(font));
                        pending = true;
                    }
                }
                Operator::Tj { .. }
                | Operator::TJ { .. }
                | Operator::Quote { .. }
                | Operator::DoubleQuote { .. } => {
                    let range = shows.next().ok_or(INVALID)?;
                    if pending {
                        if let Some(state) = active {
                            edits.push((
                                range.start..range.start,
                                state.command(&mut fonts, &mut font_serial),
                            ));
                            font_edits += 1;
                        }
                        pending = false;
                    }
                    if scale_pending {
                        if !horizontal_scale.is_finite() {
                            return Err(INVALID);
                        }
                        // A Form inspected in isolation starts at 100%.
                        // Materialize the scale this invocation inherited.
                        edits.push((
                            range.start..range.start,
                            format!("{horizontal_scale:.12} Tz"),
                        ));
                        scale_pending = false;
                    }
                }
                Operator::Do { name } => {
                    let range = draws.next().ok_or(INVALID)?;
                    let Some(object) = xobjects.get(name) else {
                        continue;
                    };
                    let inherited = InheritedText {
                        font: active.and_then(|state| state.binding(&fonts)),
                        horizontal_scale,
                    };
                    if let Some(index) = self.form(object, resources, owner, inherited, depth)? {
                        let alias = form_names.entry(index).or_insert_with(|| {
                            let alias =
                                next_name(&xobjects, "MatterDraftTextFontForm", &mut form_serial);
                            forms.push((alias.clone(), index));
                            alias
                        });
                        edits.push((range, format!("/{alias} Do")));
                    }
                }
                _ => {}
            }
        }
        if shows.next().is_some() || draws.next().is_some() {
            return Err(INVALID);
        }
        if edits.is_empty() {
            return Ok(None);
        }
        let mut resources = resources.clone();
        if font_serial != 0 {
            resources.insert("Font".into(), Object::Dictionary(fonts));
        }
        Ok(Some(ReboundTextStream {
            data: rewrite_ranges(&data, edits)?,
            resources: Object::Dictionary(resources),
            forms,
        }))
    }
}

pub(super) fn page(
    pdf: &PdfDocument,
    page: usize,
) -> Result<Option<(ReboundTextStream, Vec<ReboundTextForm>)>, &'static str> {
    let Ok(Object::Dictionary(resources)) = pdf.get_page_resources(page) else {
        return Ok(None);
    };
    if !resources.contains_key("ExtGState") && !resources.contains_key("XObject") {
        return Ok(None);
    }
    let data = pdf.get_page_content_data(page).map_err(|_| INVALID)?;
    if data.len() > MAX_DECODED_BYTES {
        return Ok(None);
    }
    let mut plan = FontStreams {
        pdf,
        forms: Vec::new(),
        cache: HashMap::new(),
        active: HashSet::new(),
        contexts: 0,
        decoded: data.len(),
    };
    let root = InheritedText {
        font: None,
        horizontal_scale: 100.0,
    };
    let stream = plan.stream(data, &resources, None, &root, 0)?;
    Ok(stream.map(|stream| (stream, plan.forms)))
}
