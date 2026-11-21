//! Inspect reopened output against the navigation contract recorded at build time.
use super::page_tools::assembly::{geometry, page_user_unit};
use pdf_oxide::{
    extractors::page_labels::{PageLabelExtractor, PageLabelRange},
    object::{Object, ObjectRef},
    PdfDocument,
};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

type Dictionary = HashMap<String, Object>;

/// PDF operands for a generated GoTo request to the reference page's origin.
pub(super) fn generated_link_origin(
    pdf: &PdfDocument,
    index: usize,
) -> Result<[f64; 2], &'static str> {
    let page = geometry(pdf, index)?;
    let origin = if page.rotation != 0 {
        // The reference uses a zero-based, unscaled CropBox transform for
        // rotated pages. Use the raw CropBox even when it exceeds MediaBox.
        let [_, bottom, _, top] = page.crop.map(|value| value as f32);
        [0.0, top - bottom]
    } else {
        // The reference's unrotated transform uses the visible intersection
        // and a Letter page if it is empty. Both transforms use f32.
        let [left, top] = page
            .clipped()
            .and_then(|visible| {
                let [left, bottom, right, top] = visible.crop.map(|value| value as f32);
                (left < right && bottom < top).then_some([left, top])
            })
            .unwrap_or([0.0, 792.0]);
        let scale = page_user_unit(pdf, index)? as f32;
        let inverse = scale.recip();
        [left * scale * inverse, top * scale * inverse]
    };
    origin
        .iter()
        .all(|position| position.is_finite())
        .then_some(origin.map(f64::from))
        .ok_or("Generated link destination is outside the supported page geometry")
}

fn field(pdf: &PdfDocument, object: &Object, name: &str) -> Option<Object> {
    pdf.resolve_object(object.as_dict()?.get(name)?).ok()
}

fn decode_text(object: &Object) -> Option<String> {
    let Object::String(bytes) = object else {
        return None;
    };
    if let Some(bytes) = bytes.strip_prefix(&[0xfe, 0xff]) {
        if bytes.len() % 2 != 0 {
            return None;
        }
        return String::from_utf16(
            &bytes
                .chunks_exact(2)
                .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
                .collect::<Vec<_>>(),
        )
        .ok();
    }
    String::from_utf8(bytes.clone()).ok().or_else(|| {
        bytes
            .iter()
            .copied()
            .map(pdf_oxide::fonts::font_dict::pdfdoc_encoding_lookup)
            .collect()
    })
}

fn destination(value: &Object, pages: &HashMap<ObjectRef, usize>) -> Option<usize> {
    let values = value.as_array()?;
    if !matches!(
        values.get(1)?.as_name()?,
        "XYZ" | "Fit" | "FitH" | "FitV" | "FitR" | "FitB" | "FitBH" | "FitBV"
    ) {
        return None;
    }
    pages.get(&values.first()?.as_reference()?).copied()
}

fn local_destination(
    pdf: &PdfDocument,
    object: &Object,
    pages: &HashMap<ObjectRef, usize>,
) -> Option<usize> {
    destination(&local_action(pdf, object)?, pages)
}

fn local_action(pdf: &PdfDocument, object: &Object) -> Option<Object> {
    let dict = object.as_dict()?;
    if let Some(target) = dict.get("Dest") {
        pdf.resolve_object(target).ok()
    } else {
        let action = field(pdf, object, "A")?;
        if action.as_dict()?.get("S")?.as_name()? != "GoTo" {
            return None;
        }
        field(pdf, &action, "D")
    }
}

fn outlines(
    pdf: &PdfDocument,
    catalog: &Object,
    pages: &HashMap<ObjectRef, usize>,
    limit: usize,
) -> Option<Vec<Value>> {
    let Some(root) = catalog.as_dict()?.get("Outlines") else {
        return Some(Vec::new());
    };
    let outline = pdf.resolve_object(root).ok()?;
    let mut pending = Vec::new();
    if let Some(first) = outline.as_dict()?.get("First") {
        pending.push((1, root.as_reference()?, first.as_reference()?));
    }
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    while let Some((level, parent, id)) = pending.pop() {
        if result.len() >= limit || !seen.insert(id) {
            return None;
        }
        let node = pdf.load_object(id).ok()?;
        let dict = node.as_dict()?;
        if dict.get("Parent")?.as_reference()? != parent {
            return None;
        }
        let title = decode_text(dict.get("Title")?)?;
        let page = local_destination(pdf, &node, pages)?;
        result.push(json!([level, title, page + 1]));
        if let Some(next) = dict.get("Next") {
            pending.push((level, parent, next.as_reference()?));
        }
        if let Some(child) = dict.get("First") {
            pending.push((level + 1, id, child.as_reference()?));
        }
    }
    Some(result)
}

fn links(
    pdf: &PdfDocument,
    page: &Object,
    pages: &HashMap<ObjectRef, usize>,
    index: usize,
    expected: &Value,
    legacy: bool,
) -> Option<Vec<Value>> {
    let Some(entries) = page.as_dict()?.get("Annots") else {
        return Some(Vec::new());
    };
    let entries = pdf.resolve_object(entries).ok()?;
    let mut targets = Vec::new();
    for entry in entries.as_array()? {
        let annotation = pdf.resolve_object(entry).ok()?;
        let spec = annotation.as_dict()?;
        if spec.get("Subtype")?.as_name()? != "Link" {
            if legacy {
                continue;
            }
            return None;
        }
        let local = local_action(pdf, &annotation);
        let position = targets.len();
        let mut target =
            if let Some(page) = local.as_ref().and_then(|action| destination(action, pages)) {
                if legacy {
                    targets.push(json!(page));
                    continue;
                }
                let mut target = json!({"page":page});
                if let Some(landing) = expected[position].get("landing").filter(|landing| {
                    local
                        .as_ref()
                        .is_some_and(|action| landing_matches(action, landing))
                }) {
                    target["landing"] = landing.clone();
                }
                target
            } else {
                let action = field(pdf, &annotation, "A")?;
                let action = action.as_dict()?;
                if action.get("S")?.as_name()? != "URI" {
                    return None;
                }
                if legacy {
                    continue;
                }
                json!({"uri":decode_text(action.get("URI")?)?})
            };
        if let Some(rect) = expected[position]["rect"].as_array() {
            let bounds = field(pdf, &annotation, "Rect")?;
            let bounds = rectangle(&bounds)?;
            let geometry = super::page_tools::assembly::geometry(pdf, index).ok()?;
            let (w, h) = geometry.display_size();
            let [x0, y0, x1, y1] = super::text_edit::displayed(geometry, bounds);
            let actual = [x0 * w, (1.0 - y1) * h, x1 * w, (1.0 - y0) * h];
            target["rect"] = if rect.len() == 4
                && actual
                    .iter()
                    .zip(rect)
                    .all(|(a, b)| b.as_f64().is_some_and(|b| (a - b).abs() <= 0.01))
            {
                expected[position]["rect"].clone()
            } else {
                json!(actual)
            };
        }
        targets.push(target);
    }
    Some(targets)
}

fn rectangle(value: &Object) -> Option<[f64; 4]> {
    let bounds = value.as_array()?;
    let [a, b, c, d] = bounds.as_slice() else {
        return None;
    };
    Some([number(a)?, number(b)?, number(c)?, number(d)?])
}

fn number(value: &Object) -> Option<f64> {
    value
        .as_real()
        .or_else(|| value.as_integer().map(|n| n as f64))
        .filter(|value| value.is_finite())
}

#[derive(Debug, PartialEq)]
pub(super) struct NavigationEvidence {
    outlines: Vec<Value>,
    labels: Vec<PageLabelRange>,
    page_links: Vec<Vec<RecordedLink>>,
}

impl NavigationEvidence {
    pub(super) fn matches_serialized(&self, rewritten: &Self) -> bool {
        self.outlines == rewritten.outlines
            && self.labels == rewritten.labels
            && self.page_links.len() == rewritten.page_links.len()
            && self
                .page_links
                .iter()
                .zip(&rewritten.page_links)
                .all(|(before, after)| {
                    before.len() == after.len()
                        && before.iter().zip(after).all(|(before, after)| {
                            before
                                .rect
                                .into_iter()
                                .zip(after.rect)
                                .all(|(left, right)| serialized_coordinate_matches(left, right))
                                && before.action.matches_serialized(&after.action)
                        })
                })
    }
}

#[derive(Debug, PartialEq)]
struct RecordedLink {
    rect: [f64; 4],
    action: LinkAction,
}

#[derive(Debug, PartialEq)]
enum LinkAction {
    GoTo {
        page: usize,
        mode: String,
        parameters: Vec<Option<f64>>,
    },
    Uri(String),
}

impl LinkAction {
    fn matches_serialized(&self, rewritten: &Self) -> bool {
        match (self, rewritten) {
            (
                Self::GoTo {
                    page,
                    mode,
                    parameters,
                },
                Self::GoTo {
                    page: new_page,
                    mode: new_mode,
                    parameters: new_parameters,
                },
            ) => {
                page == new_page
                    && mode == new_mode
                    && parameters.len() == new_parameters.len()
                    && parameters.iter().zip(new_parameters).all(|(left, right)| {
                        match (left, right) {
                            (Some(left), Some(right)) => {
                                serialized_coordinate_matches(*left, *right)
                            }
                            _ => left == right,
                        }
                    })
            }
            _ => self == rewritten,
        }
    }
}

fn serialized_coordinate_matches(left: f64, right: f64) -> bool {
    // The PDF writer serializes real numbers to five decimal places.
    let rounding = 0.000_005 + f64::EPSILON * left.abs().max(right.abs());
    (left - right).abs() <= rounding
}

fn landing_matches(action: &Object, expected: &Value) -> bool {
    let Some(values) = action.as_array() else {
        return false;
    };
    let [_, mode, left, top, zoom] = values.as_slice() else {
        return false;
    };
    let Some([expected_mode, x, y, z]) = expected.as_array().map(Vec::as_slice) else {
        return false;
    };
    mode.as_name() == expected_mode.as_str()
        && [left, top, zoom]
            .into_iter()
            .zip([x, y, z])
            .all(|(actual, expected)| {
                number(actual)
                    .zip(expected.as_f64())
                    .is_some_and(|(actual, expected)| {
                        serialized_coordinate_matches(actual, expected)
                    })
            })
}

fn recorded_links(
    pdf: &PdfDocument,
    page: &Object,
    pages: &HashMap<ObjectRef, usize>,
) -> Option<Vec<RecordedLink>> {
    let Some(entries) = page.as_dict()?.get("Annots") else {
        return Some(Vec::new());
    };
    let annotations = pdf.resolve_object(entries).ok()?;
    let mut links = Vec::new();
    for entry in annotations.as_array()? {
        let annotation = pdf.resolve_object(entry).ok()?;
        if annotation.as_dict()?.get("Subtype")?.as_name()? != "Link" {
            continue;
        }
        let rect = rectangle(&field(pdf, &annotation, "Rect")?)?;
        let action = if let Some(local) = local_action(pdf, &annotation) {
            let page = destination(&local, pages)?;
            let values = local.as_array()?;
            let mode = values.get(1)?.as_name()?.to_owned();
            let parameters = values[2..]
                .iter()
                .map(|value| match value {
                    Object::Null => Some(None),
                    _ => number(value).map(Some),
                })
                .collect::<Option<Vec<_>>>()?;
            LinkAction::GoTo {
                page,
                mode,
                parameters,
            }
        } else {
            let action = field(pdf, &annotation, "A")?;
            let fields = action.as_dict()?;
            if fields.get("S")?.as_name()? != "URI" {
                return None;
            }
            LinkAction::Uri(decode_text(&field(pdf, &action, "URI")?)?)
        };
        links.push(RecordedLink { rect, action });
    }
    Some(links)
}

/// Compare the semantic navigation after rewriting a completed PDF. Object IDs
/// may change, but the link rectangle, action and destination operands must
/// stay intact.
pub(super) fn navigation_evidence(pdf: &PdfDocument, count: usize) -> Option<NavigationEvidence> {
    let catalog = pdf.catalog().ok()?;
    let (leaves, pages) = page_tree(pdf, &catalog, count)?;
    let page_links = leaves
        .iter()
        .map(|reference| recorded_links(pdf, &pdf.resolve_object(reference).ok()?, &pages))
        .collect::<Option<Vec<_>>>()?;
    Some(NavigationEvidence {
        outlines: outlines(pdf, &catalog, &pages, usize::MAX)?,
        labels: PageLabelExtractor::extract(pdf).ok()?,
        page_links,
    })
}

fn printed_label_present(
    pdf: &PdfDocument,
    index: usize,
    label: &str,
    region: &Value,
) -> Option<bool> {
    let Some(region) = region.as_array() else {
        // Historical Rust builds did not record the numbered text region.
        return Some(
            pdf.extract_text(index)
                .ok()?
                .lines()
                .any(|line| line.trim() == label.trim()),
        );
    };
    let [x0, y0, x1, y1] = region.as_slice() else {
        return None;
    };
    let geometry = super::page_tools::assembly::geometry(pdf, index).ok()?;
    let (width, height) = geometry.display_size();
    let area = [
        (x0.as_f64()? - 2.0) / width,
        (y0.as_f64()? - 2.0) / height,
        (x1.as_f64()? + 2.0) / width,
        (y1.as_f64()? + 2.0) / height,
    ];
    Some(pdf.extract_spans(index).ok()?.iter().any(|span| {
        span.text.trim() == label.trim()
            && super::text_edit::intersects(
                area,
                super::text_edit::displayed(
                    geometry,
                    super::text_edit::span_user_bbox(geometry, span),
                ),
            )
    }))
}

fn viewer_labels(pdf: &PdfDocument, catalog: &Object, count: usize) -> Option<Vec<String>> {
    let Some(labels) = catalog.as_dict()?.get("PageLabels") else {
        return Some(Vec::new());
    };
    let labels = pdf.resolve_object(labels).ok()?;
    let values = field(pdf, &labels, "Nums")?;
    let values = values.as_array()?;
    if values.len() != count * 2 {
        return None;
    }
    values
        .chunks_exact(2)
        .enumerate()
        .map(|(index, pair)| {
            let spec = pair[1].as_dict()?;
            if pair[0].as_integer()? != index as i64 || spec.contains_key("S") {
                return None;
            }
            decode_text(spec.get("P")?)
        })
        .collect()
}

fn label_streams(pdf: &PdfDocument, page: &Dictionary) -> Option<usize> {
    let Some(contents) = page.get("Contents") else {
        return Some(0);
    };
    let contents = pdf.resolve_object(contents).ok()?;
    let streams = contents
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_else(|| std::slice::from_ref(&contents));
    let mut tagged = 0;
    for stream in streams {
        let stream = pdf.resolve_object(stream).ok()?;
        if stream.as_dict()?.get("MatterDraftPageLabel") == Some(&Object::Boolean(true)) {
            tagged += 1;
        }
    }
    Some(tagged)
}

fn inspect_navigation(
    pdf: &PdfDocument,
    catalog: &Object,
    kids: &[Object],
    pages: &HashMap<ObjectRef, usize>,
    expected: &Value,
    legacy: bool,
) -> Option<bool> {
    let mut destinations = Map::new();
    for (index, page) in kids.iter().enumerate() {
        let page = pdf.resolve_object(page).ok()?;
        let targets = links(
            pdf,
            &page,
            pages,
            index,
            &expected["links"][index.to_string()],
            legacy,
        )?;
        if !targets.is_empty() {
            destinations.insert(index.to_string(), json!(targets));
        }
    }
    let toc = expected["toc"].as_array()?;
    Some(
        outlines(pdf, catalog, pages, toc.len()).as_ref() == Some(toc)
            && Value::Object(destinations) == expected["links"],
    )
}

fn inspect_labels(
    pdf: &PdfDocument,
    catalog: &Object,
    kids: &[Object],
    expected: &Value,
    legacy: bool,
) -> Option<(bool, Vec<usize>)> {
    let viewer = expected["labels"].as_array()?;
    let printed = expected["printed_labels"].as_array().unwrap_or(viewer);
    let has_tree = catalog.as_dict()?.contains_key("PageLabels");
    let actual_viewer = if !has_tree && (legacy || !viewer.is_empty()) {
        // Viewers fall back to physical page numbers in the absence of a tree.
        Some((1..=kids.len()).map(|index| index.to_string()).collect())
    } else if legacy {
        PageLabelExtractor::extract(pdf)
            .ok()
            .map(|ranges| PageLabelExtractor::get_all_labels(&ranges, kids.len()))
    } else {
        viewer_labels(pdf, catalog, kids.len())
    };

    let mut mismatches = Vec::new();
    for index in 0..viewer.len().max(printed.len()) {
        let viewer_matches = viewer.get(index).is_none_or(|expected| {
            actual_viewer
                .as_ref()
                .and_then(|actual| actual.get(index))
                .is_some_and(|actual| expected == actual)
        });
        let printed_matches = printed.get(index).is_none_or(|label| {
            let Some(label) = label.as_str() else {
                return false;
            };
            let Some(page) = kids.get(index) else {
                return false;
            };
            label.is_empty()
                || ((legacy
                    || pdf
                        .resolve_object(page)
                        .ok()
                        .and_then(|page| label_streams(pdf, page.as_dict()?))
                        == Some(2))
                    && printed_label_present(pdf, index, label, &expected["label_regions"][index])
                        == Some(true))
        });
        if !viewer_matches || !printed_matches {
            mismatches.push(index);
        }
    }
    // Old Rust manifests used an empty array to record an absent label tree.
    let matches = mismatches.is_empty()
        && actual_viewer
            .as_ref()
            .is_some_and(|actual| actual.len() == viewer.len())
        && (legacy || printed.is_empty() || printed.len() == kids.len());
    Some((matches, mismatches))
}

pub(super) struct Inspection {
    pub navigation: bool,
    pub labels: bool,
    pub label_mismatches: Vec<usize>,
}

pub(super) fn metadata_cleared(pdf: &PdfDocument, legacy: bool) -> Option<bool> {
    let catalog = pdf.catalog().ok()?;
    Some(
        !catalog.as_dict()?.contains_key("Metadata")
            && pdf.trailer().as_dict()?.get("Info").is_none_or(|info| {
                legacy
                    && pdf.resolve_object(info).ok().and_then(|info| {
                        Some(
                            info.as_dict()?
                                .values()
                                .all(|value| value.as_string().is_some_and(|text| text.is_empty())),
                        )
                    }) == Some(true)
            }),
    )
}

type PageTree = (Vec<Object>, HashMap<ObjectRef, usize>);

fn page_tree(pdf: &PdfDocument, catalog: &Object, count: usize) -> Option<PageTree> {
    let tree = field(pdf, catalog, "Pages")?;
    let mut pending = field(pdf, &tree, "Kids")?.as_array()?.to_vec();
    pending.reverse();
    let mut kids = Vec::with_capacity(count);
    let mut seen = HashSet::new();
    while let Some(child) = pending.pop() {
        if !seen.insert(child.as_reference()?)
            || seen.len() > count.saturating_mul(4).saturating_add(64)
        {
            return None;
        }
        let node = pdf.resolve_object(&child).ok()?;
        if node.as_dict()?.get("Type")?.as_name()? == "Pages" {
            pending.extend(field(pdf, &node, "Kids")?.as_array()?.iter().rev().cloned());
        } else if node.as_dict()?.get("Type")?.as_name()? == "Page" {
            kids.push(child);
        } else {
            return None;
        }
    }
    let pages = kids
        .iter()
        .enumerate()
        .map(|(index, value)| Some((value.as_reference()?, index)))
        .collect::<Option<HashMap<_, _>>>()?;
    if kids.len() != count || pages.len() != count {
        return None;
    }
    Some((kids, pages))
}

pub(super) fn inspect(
    pdf: &PdfDocument,
    count: usize,
    expected: &Value,
    legacy: bool,
) -> Option<Inspection> {
    let catalog = pdf.catalog().ok()?;
    let (kids, pages) = page_tree(pdf, &catalog, count)?;
    let navigation =
        inspect_navigation(pdf, &catalog, &kids, &pages, expected, legacy) == Some(true);
    let (labels, label_mismatches) =
        inspect_labels(pdf, &catalog, &kids, expected, legacy).unwrap_or_default();
    Some(Inspection {
        navigation,
        labels,
        label_mismatches,
    })
}
