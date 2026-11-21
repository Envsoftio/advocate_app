//! Drop accessibility text and unused transparency masks after redaction.
use super::{page_tools::assembly::Overlay, redaction_vectors};
use pdf_oxide::{
    content::{parse_content_stream, Operator},
    object::Object,
    writer::ObjectSerializer,
    PdfDocument,
};
use sha2::{Digest as _, Sha256};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

const INVALID: &str = "Cannot safely reconcile marked content in a redacted page";
const SENSITIVE: [&str; 4] = ["ActualText", "Alt", "E", "T"];

#[derive(Default)]
pub(super) struct Evidence {
    unchanged: HashSet<[u8; 32]>,
}

struct Mark {
    operator: Range<usize>,
    tag: String,
    property: Option<String>,
    original: Object,
    retained: Object,
    fingerprint: Option<[u8; 32]>,
}

struct Scope {
    mark: Option<Mark>,
    digest: Sha256,
}

fn marks(pdf: &PdfDocument, page: usize, data: &[u8]) -> Result<Vec<Mark>, &'static str> {
    let ranges = redaction_vectors::marked_commands(data)?;
    if ranges.is_empty() {
        return Ok(Vec::new());
    }
    let mut ranges = ranges.into_iter();
    let resources = pdf.get_page_resources(page).map_err(|_| INVALID)?;
    let properties = resources
        .as_dict()
        .and_then(|resources| resources.get("Properties"))
        .map(|value| pdf.resolve_object(value))
        .transpose()
        .map_err(|_| INVALID)?;
    let serializer = ObjectSerializer::compact();
    let create_mark =
        |tag: String, value: Object, operator: Range<usize>| -> Result<Mark, &'static str> {
            let property = value.as_name().map(str::to_owned);
            let original = if let Some(name) = &property {
                let value = properties
                    .as_ref()
                    .and_then(Object::as_dict)
                    .and_then(|properties| properties.get(name))
                    .ok_or(INVALID)?;
                pdf.resolve_object(value).map_err(|_| INVALID)?
            } else {
                value
            };
            let mut retained = original.as_dict().cloned().ok_or(INVALID)?;
            for key in SENSITIVE {
                retained.remove(key);
            }
            Ok(Mark {
                operator,
                tag,
                property,
                original,
                retained: Object::Dictionary(retained),
                fingerprint: None,
            })
        };
    let mut scopes = Vec::<Scope>::new();
    let mut records = Vec::new();
    for operator in parse_content_stream(data).map_err(|_| INVALID)? {
        // Each parent scope includes nested modifications. Hash one operator
        // at a time to bound memory even for long text or inline-image streams.
        if scopes.iter().any(|scope| scope.mark.is_some()) {
            let operation = match &operator {
                Operator::InlineImage { dict, data } => {
                    let mut digest = Sha256::new();
                    digest.update(serializer.serialize(&Object::Dictionary((**dict).clone())));
                    digest.update(data);
                    digest.finalize()
                }
                Operator::Tj { text } => Sha256::digest(text),
                Operator::TJ { array }
                    if matches!(
                        array.as_slice(),
                        [pdf_oxide::content::TextElement::String(_)]
                    ) =>
                {
                    let [pdf_oxide::content::TextElement::String(text)] = array.as_slice() else {
                        unreachable!()
                    };
                    Sha256::digest(text)
                }
                Operator::BeginMarkedContentDict { tag, properties } => {
                    let mut digest = Sha256::new();
                    digest.update(tag.as_bytes());
                    digest.update(serializer.serialize(properties));
                    digest.finalize()
                }
                _ => Sha256::digest(format!("{operator:?}")),
            };
            for scope in scopes.iter_mut().filter(|scope| scope.mark.is_some()) {
                scope.digest.update(operation);
            }
        }
        match operator {
            Operator::BeginMarkedContent { .. } => scopes.push(Scope {
                mark: None,
                digest: Sha256::new(),
            }),
            Operator::BeginMarkedContentDict { tag, properties } => {
                let mark = create_mark(tag, *properties, ranges.next().ok_or(INVALID)?)?;
                let mut digest = Sha256::new();
                digest.update(mark.tag.as_bytes());
                digest.update(serializer.serialize(&mark.original));
                scopes.push(Scope {
                    mark: Some(mark),
                    digest,
                });
            }
            Operator::EndMarkedContent => {
                let Scope { mark, digest } = scopes.pop().ok_or(INVALID)?;
                if let Some(mut mark) = mark {
                    mark.fingerprint = Some(digest.finalize().into());
                    records.push(mark);
                }
            }
            Operator::Other { name, operands } if name == "DP" => {
                let [tag, value] = operands.as_slice() else {
                    return Err(INVALID);
                };
                let tag = tag.as_name().ok_or(INVALID)?.to_owned();
                records.push(create_mark(
                    tag,
                    value.clone(),
                    ranges.next().ok_or(INVALID)?,
                )?);
            }
            _ => {}
        }
        if scopes.len() > 64 {
            return Err(INVALID);
        }
    }
    if !scopes.is_empty() || ranges.next().is_some() {
        return Err(INVALID);
    }
    Ok(records)
}

pub(super) fn capture(pdf: &PdfDocument, page: usize) -> Result<Option<Evidence>, &'static str> {
    let data = pdf.get_page_content_data(page).map_err(|_| INVALID)?;
    let resources = pdf.get_page_resources(page).map_err(|_| INVALID)?;
    let has_properties = resources
        .as_dict()
        .is_some_and(|resources| resources.contains_key("Properties"));
    let mut sensitive = marks(pdf, page, &data)?
        .into_iter()
        .filter(|mark| mark.original != mark.retained)
        .peekable();
    if sensitive.peek().is_none() && !has_properties {
        return Ok(None);
    }
    let unchanged = sensitive.filter_map(|mark| mark.fingerprint).collect();
    Ok(Some(Evidence { unchanged }))
}

pub(super) fn scrub(
    pdf: &PdfDocument,
    page: usize,
    evidence: Option<&Evidence>,
) -> Result<Option<Overlay>, &'static str> {
    let resources = pdf.get_page_resources(page).map_err(|_| INVALID)?;
    let has_properties = resources
        .as_dict()
        .is_some_and(|resources| resources.contains_key("Properties"));
    if evidence.is_none() && !has_properties {
        return Ok(None);
    }
    let data = pdf.get_page_content_data(page).map_err(|_| INVALID)?;
    let serializer = ObjectSerializer::compact();
    let mut edits = Vec::new();
    let mut live_properties = HashSet::new();
    for mark in marks(pdf, page, &data)? {
        let changes = evidence.is_some_and(|source| {
            mark.original != mark.retained
                && mark
                    .fingerprint
                    .is_none_or(|digest| !source.unchanged.contains(&digest))
        });
        if !changes {
            live_properties.extend(mark.property);
            continue;
        }
        let tag = serializer.serialize_to_string(&Object::Name(mark.tag));
        let retained = serializer.serialize_to_string(&mark.retained);
        let replacement = if mark.fingerprint.is_none() {
            format!("{tag} {retained} DP")
        } else {
            format!("{tag} {retained} BDC")
        };
        edits.push((mark.operator, replacement));
    }
    if has_properties {
        inherited_properties(pdf, page, &resources, &data, &mut live_properties)?;
    }
    if edits.is_empty() && !has_properties {
        return Ok(None);
    }
    Ok(Some(Overlay {
        replace_page_content: if edits.is_empty() {
            None
        } else {
            Some(redaction_vectors::rewrite_ranges(&data, edits)?)
        },
        retain_properties: has_properties.then_some(live_properties),
        redact_content: true,
        ..Default::default()
    }))
}

pub(super) fn inactive_soft_masks(pdf: &PdfDocument, page: usize) -> HashSet<String> {
    let inspect = || {
        let resources = pdf.get_page_resources(page).ok()?;
        let resources = resources.as_dict()?;
        let states = pdf.resolve_object(resources.get("ExtGState")?).ok()?;
        let mut setters = HashMap::new();
        for (name, state) in states.as_dict()? {
            let state = pdf.resolve_object(state).ok()?;
            if let Some(mask) = state.as_dict()?.get("SMask") {
                let mask = pdf.resolve_object(mask).ok()?;
                let form = if mask.as_name() == Some("None") {
                    None
                } else {
                    Some(mask.as_dict()?.get("G")?.clone())
                };
                setters.insert(name.as_str(), form);
            }
        }
        if !setters.values().any(Option::is_some) {
            return Some(HashSet::new());
        }
        // Pattern and Type 3 programs can paint with the caller's state.
        // An independent Form owns its resource names. A Form that relies
        // on the page's names must be checked before any page mask is cleared.
        if resources.contains_key("Pattern") {
            return None;
        }
        if let Some(fonts) = resources.get("Font") {
            let fonts = pdf.resolve_object(fonts).ok()?;
            for font in fonts.as_dict()?.values() {
                let font = pdf.resolve_object(font).ok()?;
                let subtype = font.as_dict()?.get("Subtype")?.as_name()?;
                if !matches!(subtype, "Type0" | "Type1" | "MMType1" | "TrueType") {
                    return None;
                }
            }
        }
        let objects = resources
            .get("XObject")
            .map(|objects| pdf.resolve_object(objects))
            .transpose()
            .ok()?;
        let content = pdf.get_page_content_data(page).ok()?;
        let mut decoded = content.len();
        let mut pending = vec![content];
        let mut live = HashSet::new();
        let mut forms = HashSet::new();
        while let Some(content) = pending.pop() {
            let mut active = None;
            let mut stack = Vec::new();
            let mut scan_form = |entry: &Object| {
                let form = pdf.resolve_object(entry).ok()?;
                if let Some(content) = redaction_vectors::inherited_form_content(pdf, &form)? {
                    decoded = decoded.checked_add(content.len())?;
                    if decoded > 64 * 1024 * 1024 {
                        return None;
                    }
                    pending.push(content);
                }
                Some(())
            };
            for operator in parse_content_stream(&content).ok()? {
                match &operator {
                    Operator::SaveState => stack.push(active),
                    Operator::RestoreState => active = stack.pop()?,
                    Operator::SetExtGState { dict_name } => {
                        if let Some((&name, mask)) = setters.get_key_value(dict_name.as_str()) {
                            active = mask.as_ref().map(|entry| (name, entry));
                        }
                    }
                    Operator::Fill
                    | Operator::FillEvenOdd
                    | Operator::Stroke
                    | Operator::CloseAndStroke
                    | Operator::FillStroke
                    | Operator::FillStrokeEvenOdd
                    | Operator::CloseFillStroke
                    | Operator::CloseFillStrokeEvenOdd
                    | Operator::PaintShading { .. }
                    | Operator::Do { .. }
                    | Operator::InlineImage { .. }
                    | Operator::Tj { .. }
                    | Operator::TJ { .. }
                    | Operator::Quote { .. }
                    | Operator::DoubleQuote { .. }
                    | Operator::Other { .. } => {
                        if let Some((name, group)) = active {
                            if live.insert(name) {
                                scan_form(group)?;
                            }
                        }
                        if let Operator::Do { name } = operator {
                            let entry = objects.as_ref()?.as_dict()?.get(&name)?;
                            if forms.insert(name) {
                                if forms.len() > 4096 {
                                    return None;
                                }
                                scan_form(entry)?;
                            }
                        }
                    }
                    _ => {}
                }
            }
            if !stack.is_empty() {
                return None;
            }
        }
        Some(
            setters
                .into_iter()
                .filter_map(|(name, form)| {
                    (form.is_some() && !live.contains(name)).then(|| name.to_owned())
                })
                .collect(),
        )
    };
    inspect().unwrap_or_default()
}

fn inherited_properties(
    pdf: &PdfDocument,
    page: usize,
    resources: &Object,
    content: &[u8],
    live: &mut HashSet<String>,
) -> Result<(), &'static str> {
    let Some(objects) = resources
        .as_dict()
        .and_then(|resources| resources.get("XObject"))
    else {
        return Ok(());
    };
    let objects = pdf.resolve_object(objects).map_err(|_| INVALID)?;
    let objects = objects.as_dict().ok_or(INVALID)?;
    let mut pending = vec![content.to_vec()];
    let mut visited = HashSet::new();
    let mut decoded = content.len();
    while let Some(data) = pending.pop() {
        for operator in parse_content_stream(&data).map_err(|_| INVALID)? {
            let Operator::Do { name } = operator else {
                continue;
            };
            if !visited.insert(name.clone()) {
                continue;
            }
            if visited.len() > 4_096 {
                return Err(INVALID);
            }
            let object = pdf
                .resolve_object(objects.get(&name).ok_or(INVALID)?)
                .map_err(|_| INVALID)?;
            let fields = object.as_dict().ok_or(INVALID)?;
            if fields.get("Subtype").and_then(Object::as_name) == Some("Form")
                && !fields.contains_key("Resources")
            {
                let form = object.decode_stream_data().map_err(|_| INVALID)?;
                decoded = decoded.checked_add(form.len()).ok_or(INVALID)?;
                if decoded > 64 * 1024 * 1024 {
                    return Err(INVALID);
                }
                live.extend(
                    marks(pdf, page, &form)?
                        .into_iter()
                        .filter_map(|mark| mark.property),
                );
                pending.push(form);
            }
        }
    }
    Ok(())
}
