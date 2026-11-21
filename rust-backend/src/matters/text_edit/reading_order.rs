//! Split a marked-content sequence between surviving text shows.
use crate::matters::redaction_vectors::text_and_scope_ranges;
use pdf_oxide::{
    content::{parse_content_stream, Operator},
    object::{Object, ObjectRef},
    optional_content::decode_pdf_text_string,
    writer::ObjectSerializer,
    PdfDocument,
};
use std::ops::Range;

pub(in crate::matters) const CONTINUATION: &str = "MDTextTagContinuation";

pub(in crate::matters) struct Split {
    pub scope: Option<ObjectRef>,
    pub(super) position: Position,
    pub next_mcid: usize,
}

pub(super) struct Position {
    offset: usize,
    open: Range<usize>,
    alternate: Option<(Vec<u8>, Vec<u8>)>,
    enclosure: Option<Enclosure>,
}

struct Enclosure {
    close: Range<usize>,
    inside_open: usize,
    inside_close: usize,
}

impl Split {
    pub(in crate::matters) fn rewrites_alternate(&self) -> bool {
        self.position.alternate.is_some()
    }

    pub(in crate::matters) fn mark(&self, data: &[u8]) -> Option<Vec<u8>> {
        let offset = self.position.offset;
        let original_open = self.position.open.clone();
        let (opening, properties): (&[u8], &[u8]) = match &self.position.alternate {
            Some((opening, follow)) => (opening, follow),
            None => (data.get(original_open.clone())?, b"<<>>"),
        };
        let marker = format!("\nEMC\n/{CONTINUATION} ");
        let mut output = Vec::with_capacity(data.len() + marker.len() + properties.len() + 9);
        let (from, to) = if let Some(enclosure) = &self.position.enclosure {
            output.extend_from_slice(data.get(..original_open.start)?);
            output.extend_from_slice(data.get(original_open.end..enclosure.inside_open)?);
            output.push(b'\n');
            output.extend_from_slice(opening);
            output.push(b'\n');
            (enclosure.inside_open, enclosure.inside_close)
        } else if self.rewrites_alternate() {
            output.extend_from_slice(data.get(..original_open.start)?);
            output.extend_from_slice(opening);
            (original_open.end, data.len())
        } else {
            (0, data.len())
        };
        output.extend_from_slice(data.get(from..offset)?);
        output.extend_from_slice(marker.as_bytes());
        output.extend_from_slice(properties);
        output.extend_from_slice(b" BDC\n");
        output.extend_from_slice(data.get(offset..to)?);
        if let Some(enclosure) = &self.position.enclosure {
            output.push(b'\n');
            output.extend_from_slice(data.get(enclosure.close.clone())?);
            output.push(b'\n');
            output.extend_from_slice(data.get(to..enclosure.close.start)?);
            output.extend_from_slice(data.get(enclosure.close.end..)?);
        }
        Some(output)
    }
}

pub(super) struct AlternateParts<'a> {
    pub original: &'a str,
    pub before: &'a str,
    pub after: &'a str,
}

#[derive(PartialEq)]
enum Scope {
    Mark(bool),
    Graphics,
    Text,
    Compatibility,
}

pub(super) fn split_position(
    pdf: &PdfDocument,
    data: &[u8],
    resources: &Object,
    target: u32,
    suffix_show: usize,
    alternate: Option<AlternateParts<'_>>,
) -> Option<(Position, usize)> {
    if data
        .windows(CONTINUATION.len())
        .any(|word| word == CONTINUATION.as_bytes())
    {
        return None;
    }
    let (shows, scopes) = text_and_scope_ranges(data).ok()?;
    let end = shows.get(suffix_show)?.start;
    let start = shows.get(suffix_show.checked_sub(1)?)?.end;
    let properties = resources
        .as_dict()?
        .get("Properties")
        .map(|value| pdf.resolve_object(value))
        .transpose()
        .ok()?;
    let mut stack = Vec::<(Scope, usize)>::new();
    let mut closures = vec![None; scopes.len()];
    let mut previous = 0;
    let mut split = None;
    let mut target_open = None;
    let mut properties_split = None;
    let mut next_mcid = 0;
    for (index, range) in scopes.iter().enumerate() {
        let entry = &data[range.clone()];
        let boundary = start.max(previous);
        if boundary <= end
            && boundary <= range.start
            && stack
                .last()
                .is_some_and(|(scope, _)| *scope == Scope::Mark(true))
        {
            split.get_or_insert(boundary);
        }
        previous = range.end;
        let opener = match entry {
            b"q" => Scope::Graphics,
            b"BT" => Scope::Text,
            b"BX" => Scope::Compatibility,
            b"Q" | b"ET" | b"EX" | b"EMC" => {
                let (scope, open) = stack.pop()?;
                if !matches!(
                    (entry, scope),
                    (b"Q", Scope::Graphics)
                        | (b"ET", Scope::Text)
                        | (b"EX", Scope::Compatibility)
                        | (b"EMC", Scope::Mark(_))
                ) {
                    return None;
                }
                closures[open] = Some(index);
                continue;
            }
            _ if entry.ends_with(b"BDC") => {
                let operators = parse_content_stream(entry).ok()?;
                let [Operator::BeginMarkedContentDict {
                    properties: value,
                    tag,
                }] = operators.as_slice()
                else {
                    return None;
                };
                let fields = match value.as_name() {
                    Some(name) => pdf
                        .resolve_object(properties.as_ref()?.as_dict()?.get(name)?)
                        .ok()?,
                    None => *value.clone(),
                };
                let fields = fields.as_dict()?;
                let mcid = fields.get("MCID").map(|value| {
                    usize::try_from(pdf.resolve_object(value).ok()?.as_integer()?).ok()
                });
                let mcid = match mcid {
                    Some(mcid) => Some(mcid?),
                    None => None,
                };
                if let Some(mcid) = mcid {
                    next_mcid = next_mcid.max(mcid.checked_add(1)?);
                }
                let is_target = mcid == Some(target as usize);
                if is_target {
                    if target_open.replace(index).is_some()
                        || fields.len() != if alternate.is_some() { 2 } else { 1 }
                    {
                        return None;
                    }
                    if let Some(parts) = &alternate {
                        let text = pdf.resolve_object(fields.get("ActualText")?).ok()?;
                        if decode_pdf_text_string(text.as_string()?) != parts.original {
                            return None;
                        }
                        let props = |text: &str, mcid: Option<u32>| {
                            let mut fields = std::collections::HashMap::new();
                            if let Some(mcid) = mcid {
                                fields.insert("MCID".into(), Object::Integer(i64::from(mcid)));
                            }
                            fields.insert(
                                "ActualText".into(),
                                super::assembly::pdf_text(text.trim_matches(['\r', '\n'])),
                            );
                            ObjectSerializer::compact().serialize(&Object::Dictionary(fields))
                        };
                        let mut opening =
                            ObjectSerializer::compact().serialize(&Object::Name(tag.clone()));
                        opening.push(b' ');
                        opening.extend(props(parts.before, Some(target)));
                        opening.extend_from_slice(b" BDC");
                        properties_split = Some((opening, props(parts.after, None)));
                    }
                }
                Scope::Mark(is_target)
            }
            _ if entry.ends_with(b"BMC") => Scope::Mark(false),
            _ => return None,
        };
        stack.push((opener, index));
    }
    if !stack.is_empty() {
        return None;
    }
    let open = target_open?;
    let close = closures[open]?;
    if let Some(offset) = split {
        return Some((
            Position {
                offset,
                open: scopes[open].clone(),
                alternate: properties_split,
                enclosure: None,
            },
            next_mcid,
        ));
    }
    // A single BT or q can enclose all the text. Move the two original tag
    // delimiters inside complete wrappers so the new boundary stays balanced.
    let mut inner = open;
    let mut outer_close = close;
    while inner + 1 < outer_close {
        let opener = inner + 1;
        let closer = outer_close - 1;
        if closures[opener] != Some(closer)
            || !matches!(&data[scopes[opener].clone()], b"BT" | b"q" | b"BX")
            || !only_trivia(&data[scopes[inner].end..scopes[opener].start])
            || !only_trivia(&data[scopes[closer].end..scopes[outer_close].start])
        {
            break;
        }
        inner = opener;
        outer_close = closer;
    }
    if inner == open {
        return None;
    }
    let inside_open = scopes[inner].end;
    let inside_close = scopes[outer_close].start;
    let mut previous = inside_open;
    let mut depth = 0usize;
    for index in inner + 1..=outer_close {
        let boundary = start.max(previous);
        if depth == 0 && boundary <= end && boundary <= scopes[index].start {
            return Some((
                Position {
                    offset: boundary,
                    open: scopes[open].clone(),
                    alternate: properties_split,
                    enclosure: Some(Enclosure {
                        close: scopes[close].clone(),
                        inside_open,
                        inside_close,
                    }),
                },
                next_mcid,
            ));
        }
        if index < outer_close {
            depth = if closures[index].is_some() {
                depth + 1
            } else {
                depth.checked_sub(1)?
            };
        }
        previous = scopes[index].end;
    }
    None
}

fn only_trivia(data: &[u8]) -> bool {
    let mut in_comment = false;
    for byte in data {
        match byte {
            b'%' => in_comment = true,
            b'\r' | b'\n' => in_comment = false,
            b if !in_comment && *b != 0 && !b.is_ascii_whitespace() => return false,
            _ => {}
        }
    }
    !in_comment
}
