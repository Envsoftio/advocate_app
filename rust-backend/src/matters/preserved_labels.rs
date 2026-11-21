//! Reconcile reviewed inserts with retained baseline pages before assigning labels.
use super::{
    artifacts::{refusal, Artifact, PdfProof, PdfSource},
    page_tools::assembly,
    revisions, *,
};
use pdf_oxide::{
    rendering::{render_page, RenderOptions},
    PdfDocument,
};
use std::collections::{BTreeMap, HashMap, HashSet};

pub(super) struct Reconciliation {
    pub labels: BTreeMap<String, String>,
    pub preview: Vec<Value>,
    pub boundaries: Vec<Value>,
    pub existing_labels: usize,
}

struct InsertGroup<'a> {
    document: &'a str,
    combined_page: usize,
    label: &'a str,
    anchor: usize,
    pages: Vec<(usize, &'a str)>,
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map(Vec::as_slice).unwrap_or_default()
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn number(value: &Value, key: &str) -> usize {
    value[key].as_u64().unwrap_or_default() as usize
}

fn fingerprints(pdf: &PdfDocument) -> Result<Vec<String>, String> {
    let count = pdf.page_count().map_err(|_| "Cannot inspect page count")?;
    if count > 2000 {
        return Err("Baseline page count exceeds the Rust reconciliation limit".into());
    }
    let mut options = RenderOptions::with_dpi(72).as_raw();
    options.max_output_pixels = 4_000_000;
    let mut results = Vec::new();
    for page in 0..count {
        let geometry = assembly::geometry(pdf, page)?;
        let rendered = render_page(pdf, page, &options)
            .map_err(|_| "Cannot render baseline-comparison page")?;
        let mut digest = Sha256::new();
        let (w, h) = geometry.display_size();
        digest.update(format!("{w:.3}:{h:.3}:{}:", geometry.rotation));
        digest.update(rendered.data);
        digest.update(
            pdf.extract_text(page)
                .map_err(|_| "Cannot extract baseline-comparison page")?,
        );
        results.push(vault::hex_bytes(&digest.finalize()).to_string());
    }
    Ok(results)
}

fn block(code: &str, message: &str) -> Value {
    json!({"code":code, "message":message})
}

fn missing_map() -> Value {
    block(
        "MISSING_BASELINE_MAP",
        "The baseline lacks exact source-label evidence.",
    )
}

type SourcePair = (PdfSource, PdfSource);

pub(super) struct ReconciliationWork {
    sources: HashMap<String, Result<SourcePair, Value>>,
}

impl ReconciliationWork {
    pub(super) fn capture(store: &Store, prior: &Value, selected: &[Value]) -> Self {
        let ids = selected
            .iter()
            .map(|doc| text(doc, "id"))
            .collect::<HashSet<_>>();
        let sources = list(prior, "input_revisions")
            .iter()
            .filter_map(|doc| {
                let document = text(doc, "document_id");
                ids.contains(document)
                    .then(|| (document.to_owned(), capture_pair(store, doc, document)))
            })
            .collect();
        Self { sources }
    }
}

fn capture_pair(store: &Store, prior_doc: &Value, document: &str) -> Result<SourcePair, Value> {
    let current = store.artifact_document(document).map_err(|_| {
        block(
            "DELETED_BASELINE_REFERENCE",
            "A baseline document is unavailable.",
        )
    })?;
    let revision = prior_doc["revision"].as_u64().ok_or_else(|| {
        block(
            "MISSING_BASELINE_MAP",
            "Baseline working revision is unavailable.",
        )
    })?;
    let retained = revisions::record(&store.con, document, revision)
        .ok()
        .flatten()
        .ok_or_else(|| {
            block(
                "BASELINE_BYTES_UNAVAILABLE",
                "Baseline working revision is unavailable.",
            )
        })?;
    if prior_doc["working_sha256"] != retained.hash
        || list(prior_doc, "page_ids").len() as u64 != retained.pages
    {
        return Err(block(
            "MISSING_BASELINE_MAP",
            "Baseline working hash or exact page identity evidence is incomplete.",
        ));
    }
    let mut evidence = store
        .con
        .prepare(
            "SELECT id,hash,page FROM rust_source_pages
         WHERE document_id=? AND relative_path=? ORDER BY page",
        )
        .map_err(|_| missing_map())?;
    let entries = evidence
        .query_map(params![document, retained.relpath], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, usize>(2)?,
            ))
        })
        .map_err(|_| missing_map())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| missing_map())?;
    if entries.len() != retained.pages as usize
        || entries
            .iter()
            .zip(list(prior_doc, "page_ids"))
            .enumerate()
            .any(|(index, ((id, hash, page), expected))| {
                expected.as_str() != Some(id.as_str()) || *page != index || hash != &retained.hash
            })
    {
        return Err(block(
            "DELETED_BASELINE_REFERENCE",
            "A baseline revision page identity is missing or changed.",
        ));
    }
    let unavailable = |_| {
        block(
            "BASELINE_BYTES_UNAVAILABLE",
            "Baseline or current working bytes are unavailable.",
        )
    };
    let baseline = store
        .pdf_source(
            document,
            &Artifact::Retained {
                revision,
                sha256: retained.hash,
            },
        )
        .map_err(unavailable)?;
    let working = store
        .pdf_source(
            document,
            &Artifact::Working {
                revision: current.body["revision"].as_u64(),
            },
        )
        .map_err(unavailable)?;
    Ok((baseline, working))
}

pub(super) fn evaluate(
    mut work: ReconciliationWork,
    prior: &Value,
    map: &Value,
    proofs: &mut Vec<PdfProof>,
) -> Result<Reconciliation, Value> {
    let baseline = list(prior, "input_revisions");
    let contract = &prior["validation_contract"]["combined.pdf"];
    let printed = contract["printed_labels"].as_array().ok_or_else(|| {
        block(
            "MISSING_BASELINE_MAP",
            "The baseline lacks recorded printed labels.",
        )
    })?;
    let viewer = contract["labels"].as_array().ok_or_else(|| {
        block(
            "MISSING_BASELINE_MAP",
            "The baseline lacks recorded viewer labels.",
        )
    })?;
    // Earlier Rust builds used an empty array when the PDF had no label tree.
    // Its viewer labels follow the physical page order.
    let historical_physical_labels = viewer.is_empty()
        && prior["backend_origin"] != "python"
        && prior["output_marks"]["numbering_pdf_labels"] == false;
    let mut labels = printed
        .iter()
        .filter_map(Value::as_str)
        .filter(|label| !label.is_empty())
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    let existing_labels = labels.len();
    let mut anchors = HashMap::new();
    for (position, source) in contract["sources"].as_object().ok_or_else(missing_map)? {
        let index = position.parse::<usize>().map_err(|_| missing_map())?;
        let label = printed
            .get(index)
            .and_then(Value::as_str)
            .ok_or_else(missing_map)?;
        if !historical_physical_labels && viewer.get(index).and_then(Value::as_str).is_none() {
            return Err(missing_map());
        }
        let doc = source["document_id"].as_str().ok_or_else(missing_map)?;
        let page = source["page_index"].as_u64().ok_or_else(missing_map)? as usize;
        if anchors.insert((doc, page), (index + 1, label)).is_some() {
            return Err(block(
                "MISSING_BASELINE_MAP",
                "The baseline maps one source page to multiple physical pages.",
            ));
        }
    }
    let document_order = baseline
        .iter()
        .enumerate()
        .map(|(index, doc)| (text(doc, "document_id"), index))
        .collect::<HashMap<_, _>>();
    let mut selected = HashMap::<&str, Vec<(usize, &str)>>::new();
    let mut ordered = Vec::new();
    for entry in list(map, "volumes")
        .iter()
        .flat_map(|volume| list(volume, "pages"))
    {
        let Some(identity) = entry["stable_page_id"].as_str() else {
            continue;
        };
        let document = text(entry, "document_id");
        let order = document_order.get(document).ok_or_else(|| {
            block(
                "NEW_DOCUMENT_INSERTION_UNSUPPORTED",
                "Preserved labels require selected pages from existing baseline documents.",
            )
        })?;
        let page = number(entry, "page_index");
        let coordinate = (*order, page);
        if ordered
            .last()
            .is_some_and(|previous| previous >= &coordinate)
        {
            return Err(block(
                "INSERT_SELECTION_ORDER_UNSUPPORTED",
                "Choose insert pages in current source order.",
            ));
        }
        ordered.push(coordinate);
        selected.entry(document).or_default().push((page, identity));
    }
    if selected.is_empty() {
        return Err(block(
            "EMPTY_SELECTION",
            "Select source pages before reviewing preserved labels.",
        ));
    }
    let mut groups = Vec::<InsertGroup<'_>>::new();
    for prior_doc in baseline {
        let document = text(prior_doc, "document_id");
        let Some(pages) = selected.get(document) else {
            continue;
        };
        let (baseline, working) = work.sources.remove(document).ok_or_else(missing_map)??;
        let mut load = |source: PdfSource| {
            let unavailable = || {
                block(
                    "BASELINE_BYTES_UNAVAILABLE",
                    "Baseline or current working bytes are unavailable.",
                )
            };
            let (snapshot, proof) = source.open_with_proof().map_err(|_| unavailable())?;
            proofs.push(proof);
            snapshot
                .into_bytes()
                .ok()
                .and_then(|bytes| PdfDocument::from_bytes(bytes).ok())
                .ok_or_else(unavailable)
        };
        let prior_pdf = load(baseline)?;
        let current_pdf = load(working)?;
        let initial = fingerprints(&prior_pdf).map_err(|_| {
            block(
                "BASELINE_BYTES_UNAVAILABLE",
                "Cannot measure baseline page evidence.",
            )
        })?;
        let working = fingerprints(&current_pdf).map_err(|_| {
            block(
                "BASELINE_BYTES_UNAVAILABLE",
                "Cannot measure working page evidence.",
            )
        })?;
        let inserts = pages.iter().map(|(page, _)| *page).collect::<HashSet<_>>();
        if working.len().saturating_sub(inserts.len()) != initial.len() {
            return Err(block(
                "DELETED_BASELINE_REFERENCE",
                "Removing the selected inserts does not restore the baseline page count.",
            ));
        }
        if working
            .iter()
            .enumerate()
            .filter(|(page, _)| !inserts.contains(page))
            .map(|(_, hash)| hash)
            .ne(initial.iter())
        {
            let duplicates = initial.iter().collect::<HashSet<_>>().len() != initial.len();
            return Err(block(
                if duplicates {
                    "AMBIGUOUS_REPLACEMENT"
                } else {
                    "CHANGED_BASELINE_REFERENCE"
                },
                "Baseline pages do not reconcile in the same source order.",
            ));
        }
        for (page, identity) in pages {
            let previous = (0..*page).filter(|index| !inserts.contains(index)).count();
            let Some(anchor) = previous.checked_sub(1) else {
                return Err(block(
                    "INSERT_BEFORE_FIRST_UNSUPPORTED",
                    "Preserved suffix labels require an earlier baseline page.",
                ));
            };
            let (position, label) = anchors.get(&(document, anchor)).copied().ok_or_else(|| {
                block(
                    "MISSING_BASELINE_MAP",
                    "The insertion anchor has no recorded source-label evidence.",
                )
            })?;
            if let Some(group) = groups
                .last_mut()
                .filter(|group| group.combined_page == position)
            {
                group.pages.push((*page, *identity));
            } else {
                groups.push(InsertGroup {
                    document,
                    combined_page: position,
                    label,
                    anchor,
                    pages: vec![(*page, *identity)],
                });
            }
        }
    }
    groups.sort_unstable_by_key(|group| group.combined_page);
    let mut result = Reconciliation {
        labels: BTreeMap::new(),
        preview: Vec::new(),
        boundaries: Vec::new(),
        existing_labels,
    };
    let mut offset = 0;
    for group in &groups {
        let InsertGroup {
            document,
            combined_page: position,
            label,
            anchor,
            pages,
        } = group;
        if !label
            .chars()
            .last()
            .is_some_and(|character| character.is_ascii_digit())
        {
            return Err(block(
                "UNSUPPORTED_BASELINE_LABEL",
                "The anchor label cannot use decimal-suffix notation.",
            ));
        }
        result.preview.push(json!({"material":"Earlier material","source_position":format!("Baseline source {} · current source {}",anchor+1,pages[0].0),
            "printed_label":label,"combined_position":position+offset,"kind":"earlier"}));
        for (index, (source, id)) in pages.iter().enumerate() {
            let inserted = format!("{label}.{}", index + 1);
            if !labels.insert(inserted.clone()) {
                return Err(block(
                    "INSERT_LABEL_COLLISION",
                    "An inserted label collides with an existing printed label.",
                ));
            }
            result.labels.insert((*id).to_owned(), inserted.clone());
            if index == 0 || index + 1 == pages.len() {
                result.preview.push(json!({"material":"Inserted material","stable_page_id":id,
                    "source_position":format!("Current source {}",source+1),"printed_label":inserted,
                    "combined_position":position+offset+index+1,"kind":"inserted"}));
            }
        }
        offset += pages.len();
        if let Some((position, label)) = anchors.get(&(*document, anchor + 1)) {
            result.preview.push(
                json!({"material":format!("Earlier source page {}",anchor+2),
                "source_position":format!("Current source {}",pages[pages.len()-1].0+2),
                "printed_label":label,"combined_position":position+offset,"kind":"earlier"}),
            );
        }
    }
    let mut end = 0;
    for volume in 1..=number(prior, "volumes") {
        let pages = number(
            &prior["validation_contract"][format!("volume-{volume:02}.pdf")],
            "pages",
        );
        if pages == 0 {
            continue;
        }
        end += pages;
        let shift = groups
            .iter()
            .filter(|group| group.combined_page <= end)
            .map(|group| group.pages.len())
            .sum::<usize>();
        result.boundaries.push(
            json!({"volume":volume,"baseline_end":end,"reconciled_end":end+shift,"shift":shift}),
        );
    }
    Ok(result)
}

pub(super) fn apply(map: &mut Value, labels: &BTreeMap<String, String>) -> Result<(), Reply> {
    let invalid = || {
        refusal(
            409,
            "Generated label destinations differ from the saved page map",
        )
    };
    let mut printed = HashMap::new();
    for volume in map["volumes"].as_array_mut().ok_or_else(invalid)? {
        let entries = volume["pages"].as_array_mut().ok_or_else(invalid)?;
        for entry in entries.iter_mut() {
            if let Some(label) = entry["stable_page_id"]
                .as_str()
                .and_then(|id| labels.get(id))
            {
                entry["printed_label"] = json!(label);
                entry["pdf_label"] = json!(label);
                printed.insert(number(entry, "combined_page"), label.clone());
            }
        }
        let values = entries
            .iter()
            .map(|entry| entry["printed_label"].clone())
            .collect::<Vec<_>>();
        for row in volume["rows"].as_array_mut().ok_or_else(invalid)? {
            let start = number(row, "start_page")
                .checked_sub(1)
                .ok_or_else(invalid)?;
            let end = number(row, "end_page").checked_sub(1).ok_or_else(invalid)?;
            row["printed_start"] = values.get(start).ok_or_else(invalid)?.clone();
            row["printed_end"] = values.get(end).ok_or_else(invalid)?.clone();
        }
    }
    if let Some(bookmarks) = map["manual_bookmarks"].as_array_mut() {
        for bookmark in bookmarks {
            if let Some(label) = printed.get(&number(bookmark, "combined_page")) {
                bookmark["printed_label"] = json!(label);
            }
        }
    }
    Ok(())
}
