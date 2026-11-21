//! Compare the document context that a paragraph edit must preserve.
use super::{page_tree, Dictionary, Result, COMPLEX, INVALID, MAX_DEPTH, MAX_OBJECTS};
use pdf_oxide::{
    object::{Object, ObjectRef},
    PdfDocument,
};
use std::collections::{HashMap, HashSet};

pub(in crate::matters) fn verify_edit_context(
    source: &PdfDocument,
    edited: &PdfDocument,
    edited_page: usize,
) -> Result<()> {
    let source_catalog = source.catalog().map_err(|_| INVALID)?;
    let edited_catalog = edited.catalog().map_err(|_| INVALID)?;
    let (source_pages, _) = page_tree(source, &source_catalog)?;
    let (edited_pages, _) = page_tree(edited, &edited_catalog)?;
    if source_pages.len() != edited_pages.len() || edited_page >= source_pages.len() {
        return Err(INVALID);
    }

    let mut graph = Graph {
        source,
        edited,
        // Destinations and widget page references follow the physical page order.
        references: source_pages.iter().copied().zip(edited_pages).collect(),
        edited_refs: HashSet::new(),
    };
    graph.edited_refs.extend(graph.references.values().copied());

    let info = |pdf: &PdfDocument| -> Result<Option<Object>> {
        pdf.trailer()
            .as_dict()
            .ok_or(INVALID)?
            .get("Info")
            .map(|value| pdf.resolve_object(value).map_err(|_| INVALID))
            .transpose()
    };
    graph.optional(info(source)?.as_ref(), info(edited)?.as_ref(), 0)?;

    let source_catalog = source_catalog.as_dict().ok_or(INVALID)?;
    let edited_catalog = edited_catalog.as_dict().ok_or(INVALID)?;
    for key in [
        "Outlines",
        "Dests",
        "Names",
        "AcroForm",
        "Metadata",
        "AF",
        "PageLabels",
        "OpenAction",
        "AA",
    ] {
        graph.optional(source_catalog.get(key), edited_catalog.get(key), 0)?;
    }

    for page in 0..source_pages.len() {
        let before = source.get_page_for_debug(page).map_err(|_| INVALID)?;
        let after = edited.get_page_for_debug(page).map_err(|_| INVALID)?;
        let before = before.as_dict().ok_or(INVALID)?;
        let after = after.as_dict().ok_or(INVALID)?;
        for key in ["Annots", "AA", "Metadata", "AF"] {
            graph.optional(before.get(key), after.get(key), 0)?;
        }
        if page != edited_page {
            let before = source.get_page_resources(page).map_err(|_| INVALID)?;
            let after = edited.get_page_resources(page).map_err(|_| INVALID)?;
            graph.compare(&before, &after, 0)?;
        }
    }
    Ok(())
}

struct Graph<'a> {
    source: &'a PdfDocument,
    edited: &'a PdfDocument,
    references: HashMap<ObjectRef, ObjectRef>,
    edited_refs: HashSet<ObjectRef>,
}

impl Graph<'_> {
    fn optional(
        &mut self,
        before: Option<&Object>,
        after: Option<&Object>,
        depth: usize,
    ) -> Result<()> {
        match (before, after) {
            (None, None) => Ok(()),
            (Some(before), Some(after)) => self.compare(before, after, depth),
            _ => Err(INVALID),
        }
    }

    fn compare(&mut self, before: &Object, after: &Object, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(COMPLEX);
        }
        match (before, after) {
            (Object::Reference(before), Object::Reference(after)) => {
                if let Some(previous) = self.references.get(before) {
                    return (previous == after).then_some(()).ok_or(INVALID);
                }
                if self.references.len() >= MAX_OBJECTS {
                    return Err(COMPLEX);
                }
                if !self.edited_refs.insert(*after) {
                    return Err(INVALID);
                }
                self.references.insert(*before, *after);
                let before = self.source.load_object(*before).map_err(|_| INVALID)?;
                let after = self.edited.load_object(*after).map_err(|_| INVALID)?;
                self.compare(&before, &after, depth + 1)
            }
            (Object::Array(before), Object::Array(after)) if before.len() == after.len() => {
                for (before, after) in before.iter().zip(after) {
                    self.compare(before, after, depth + 1)?;
                }
                Ok(())
            }
            (Object::Dictionary(before), Object::Dictionary(after)) => {
                self.dictionary(before, after, false, depth + 1)
            }
            (
                Object::Stream {
                    dict: before,
                    data: original,
                },
                Object::Stream {
                    dict: after,
                    data: candidate,
                },
            ) if original == candidate => {
                // The writer computes stream lengths when serializing them.
                self.dictionary(before, after, true, depth + 1)
            }
            // PDF Oxide writes real numbers to five places and can emit an integer
            // for a real-valued PDF object. Allow only that serialization precision.
            (Object::Real(before), Object::Real(after)) => equal_real(*before, *after),
            (Object::Integer(integer), Object::Real(real))
            | (Object::Real(real), Object::Integer(integer)) => equal_real(*integer as f64, *real),
            _ if before == after => Ok(()),
            _ => Err(INVALID),
        }
    }

    fn dictionary(
        &mut self,
        before: &Dictionary,
        after: &Dictionary,
        stream: bool,
        depth: usize,
    ) -> Result<()> {
        let retained = |key: &&String| !stream || key.as_str() != "Length";
        if before.keys().filter(retained).count() != after.keys().filter(retained).count() {
            return Err(INVALID);
        }
        for key in before.keys().filter(retained) {
            self.compare(&before[key], after.get(key).ok_or(INVALID)?, depth)?;
        }
        Ok(())
    }
}

fn equal_real(before: f64, after: f64) -> Result<()> {
    ((before - after).abs() <= 0.000_005_01)
        .then_some(())
        .ok_or(INVALID)
}
