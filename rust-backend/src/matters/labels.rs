//! Technical label proposals and revision-bound citation review.
use super::{
    artifacts::{refusal, PdfProof, PdfSource},
    document_metadata::{self, review_store_error, ReviewSnapshot},
    reading::folded,
    *,
};
use regex::Regex;
use serde::Deserialize;
use serde_json::Map;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::LazyLock,
};

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Proposal {
    ids: Vec<String>,
    mode: String,
    prefix: String,
    start: usize,
    group_separator: String,
    group_suffix_style: String,
    first_member_base: bool,
    existing_label_policy: String,
    reference_document_id: Option<String>,
}

impl Default for Proposal {
    fn default() -> Self {
        Self {
            ids: Vec::new(),
            mode: String::new(),
            prefix: "P-".into(),
            start: 1,
            group_separator: String::new(),
            group_suffix_style: "lower_alpha".into(),
            first_member_base: false,
            existing_label_policy: "compact".into(),
            reference_document_id: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mapping {
    document_id: String,
    old_label: String,
    new_label: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Apply {
    mappings: Vec<Mapping>,
    reference_document_id: Option<String>,
    expected_reference_revision: Option<u64>,
    #[serde(default)]
    acknowledge_existing_labels: bool,
    #[serde(default)]
    acknowledge_prior_exports: bool,
    #[serde(default)]
    acknowledge_references: bool,
}

fn s<'a>(doc: &'a Value, k: &str) -> &'a str {
    doc[k].as_str().unwrap_or_default()
}

fn proposal(body: &Value) -> Option<Proposal> {
    let config = Proposal::deserialize(body).ok()?;
    let visible =
        |field: &str, limit| field.chars().count() <= limit && field.chars().all(|c| c >= ' ');
    ((1..=1000).contains(&config.ids.len())
        && ["sequential", "grouped_suffix", "filename"].contains(&config.mode.as_str())
        && (1..=9999).contains(&config.start)
        && visible(&config.prefix, 20)
        && visible(&config.group_separator, 5)
        && [
            "lower_alpha",
            "upper_alpha",
            "numeric",
            "lower_alpha_parenthesized",
            "upper_alpha_parenthesized",
            "numeric_parenthesized",
        ]
        .contains(&config.group_suffix_style.as_str())
        && ["compact", "preserve"].contains(&config.existing_label_policy.as_str()))
    .then_some(config)
}

fn suffix(index: usize, style: &str) -> String {
    let mut n = index + 1;
    let mut letters = Vec::new();
    let value = if style.starts_with("numeric") {
        n.to_string()
    } else {
        let base = if style.starts_with("upper") {
            b'A'
        } else {
            b'a'
        };
        while n > 0 {
            n -= 1;
            letters.push(char::from(base + (n % 26) as u8));
            n /= 26;
        }
        letters.into_iter().rev().collect()
    };
    if style.ends_with("_parenthesized") {
        format!("({value})")
    } else {
        value
    }
}

fn filename_label(filename: &str) -> (String, String) {
    static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(?:annexure|annex|exhibit)[\s._-]+([a-z]{0,8}[- ]?\d+[a-z]?)\b").unwrap()
    });
    let stem = Path::new(filename)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let labels = TOKEN
        .captures_iter(stem)
        .map(|matched| matched[1].trim().replace(' ', "-"))
        .collect::<BTreeSet<_>>();
    match labels.len() {
        0 => (
            "".into(),
            "No conservative annexure/exhibit label was found in the filename.".into(),
        ),
        1 => (
            labels.into_iter().next().unwrap(),
            "One annexure/exhibit token was found in the filename.".into(),
        ),
        _ => (
            "".into(),
            "Multiple possible labels were found in the filename; choose manually.".into(),
        ),
    }
}

fn selected_groups(docs: &[Value]) -> Vec<(&str, Vec<&Value>)> {
    let mut order = Vec::<(&str, Vec<&Value>)>::new();
    let mut index = HashMap::new();
    for doc in docs {
        let group = s(doc, "group_key").trim();
        if group.is_empty() {
            continue;
        }
        let position = *index.entry(group).or_insert(order.len());
        if position == order.len() {
            order.push((group, vec![]));
        }
        order[position].1.push(doc);
    }
    order
}

fn duplicate_labels(labels: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    labels
        .filter(|label| !label.is_empty() && !seen.insert(folded(label)))
        .collect()
}

fn propose(docs: &[Value], config: &Proposal) -> Result<Value, Reply> {
    let ids: HashSet<_> = config.ids.iter().map(String::as_str).collect();
    let selected = docs
        .iter()
        .filter(|doc| ids.contains(s(doc, "id")))
        .collect::<Vec<_>>();
    if ids.len() != config.ids.len() || selected.len() != ids.len() {
        return Err(refusal(
            422,
            "Selection contains a missing, archived or duplicate document",
        ));
    }
    let mut warnings = Vec::new();
    let mut blocking = Vec::new();
    let mut proposals = HashMap::<&str, (String, String)>::new();
    let prefix = &config.prefix;
    match config.mode.as_str() {
        "sequential" => {
            for (index, doc) in selected.iter().enumerate() {
                proposals.insert(
                    s(doc, "id"),
                    (
                        format!("{prefix}{}", config.start + index),
                        "Sequential technical proposal in current matter order.".into(),
                    ),
                );
            }
        }
        "filename" => {
            for doc in &selected {
                proposals.insert(s(doc, "id"), filename_label(s(doc, "original_name")));
            }
            warnings.push("Filename parsing is conservative and remains unreviewed until every proposed label is confirmed.");
        }
        _ => {
            let groups = selected_groups(docs);
            for (group, members) in &groups {
                let chosen = members
                    .iter()
                    .filter(|doc| ids.contains(s(doc, "id")))
                    .count();
                if chosen > 0 && chosen != members.len() {
                    blocking.push(format!("Group “{group}” is only partly selected; select all {} members or use sequential mode.",members.len()));
                }
            }
            let mut group_index: HashMap<_, _> = groups.into_iter().collect();
            let mut number = config.start;
            for doc in &selected {
                let group = s(doc, "group_key").trim();
                if group.is_empty() {
                    proposals.insert(
                        s(doc, "id"),
                        (
                            format!("{prefix}{number}"),
                            "Ungrouped document receives one base number.".into(),
                        ),
                    );
                } else {
                    let Some(members) = group_index.remove(group) else {
                        continue;
                    };
                    let members = members
                        .into_iter()
                        .filter(|doc| ids.contains(s(doc, "id")))
                        .collect::<Vec<_>>();
                    let parents = members
                        .iter()
                        .filter(|doc| s(doc, "group_role") == "parent")
                        .collect::<Vec<_>>();
                    if parents.len() > 1 {
                        blocking.push(format!(
                            "Group “{group}” has more than one designated parent."
                        ));
                    }
                    let parent = if parents.len() == 1 {
                        Some(s(parents[0], "id"))
                    } else {
                        None
                    };
                    let style = config.group_suffix_style.replace('_', " ");
                    let mut next = 0;
                    for (position, member) in members.into_iter().enumerate() {
                        let id = s(member, "id");
                        let (ending, detail) = if Some(id) == parent {
                            (
                                "".into(),
                                "the explicitly designated parent keeps the base label".into(),
                            )
                        } else if parent.is_none() && config.first_member_base && position == 0 {
                            (
                                "".into(),
                                "the first ordered member keeps the base label".into(),
                            )
                        } else {
                            let ending = format!(
                                "{}{}",
                                config.group_separator,
                                suffix(next, &config.group_suffix_style)
                            );
                            next += 1;
                            let role = if parent.is_some() {
                                "child suffix style"
                            } else {
                                "suffix style"
                            };
                            (ending, format!("{role} {style}"))
                        };
                        proposals.insert(id,(format!("{prefix}{number}{ending}"),
                            format!("Configured grouped proposal for group “{group}” ({detail}); group order is unchanged.")));
                    }
                }
                number += 1;
            }
            warnings.push("Grouped formatting is an explicit technical choice. A designated parent records label hierarchy but does not establish a legal relationship or court approval.");
        }
    }
    if config.existing_label_policy == "preserve" {
        for doc in &selected {
            let label = s(doc, "label").trim();
            let reason = if label.is_empty() {
                "Preserve-existing policy requires the reviewer to enter a label for this new or unlabelled document."
            } else {
                "Existing label preserved; no circulated reference is renumbered automatically."
            };
            proposals.insert(s(doc, "id"), (label.into(), reason.into()));
        }
        warnings.push("Preserve-existing policy keeps current labels and requires manual labels for new documents. Choose compact to recalculate the selected sequence after insertion or removal.");
    }
    let mut rows = Vec::new();
    for doc in &selected {
        let title = s(doc, "title");
        let id = s(doc, "id");
        let (label, reason) = proposals
            .get_mut(id)
            .expect("proposal for selected document");
        if label.chars().count() > 60 {
            blocking.push(format!(
                "Proposed label for “{title}” exceeds 60 characters."
            ));
            *label = label.chars().take(60).collect();
        }
        rows.push(json!({"document_id":id,"title":title,"original_name":doc["original_name"],
            "description":title,"group_key":doc["group_key"],"group_role":doc["group_role"],
            "old_label":doc["label"],"old_status":doc["label_status"],"proposed_label":label,
            "proposal_status":if label.is_empty() {"needs_manual"} else {"suggested"}, "reason":reason}));
    }
    for label in duplicate_labels(docs.iter().map(|doc| {
        proposals
            .get(s(doc, "id"))
            .map_or(s(doc, "label"), |(label, _)| label)
            .trim()
            .to_owned()
    })) {
        blocking.push(format!("Label “{label}” would be duplicated."));
    }
    if rows
        .iter()
        .any(|row| s(row, "proposed_label").trim().is_empty())
    {
        blocking.push(
            "Every selected document needs a proposed label before this mapping can be applied."
                .into(),
        );
    }
    let mut seen = HashSet::new();
    blocking.retain(|warning| seen.insert(warning.clone()));
    Ok(json!({"rows":rows,"warnings":warnings,"blocking":blocking}))
}

fn references(bytes: Vec<u8>, mappings: &[Mapping], revision: u64) -> Result<Value, Reply> {
    static PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?i)\b(?:annexure|annex|exhibit)\s*(?:no\.?\s*)?[-:–—]?\s*([a-z0-9][a-z0-9./-]{0,30})").unwrap()
    });
    let source = pdf_oxide::PdfDocument::from_bytes(bytes).map_err(|_| citation_unreadable())?;
    let old: HashMap<_, _> = mappings
        .iter()
        .filter(|m| !m.old_label.trim().is_empty())
        .map(|m| (folded(m.old_label.trim()), m))
        .collect();
    let new: HashMap<_, _> = mappings
        .iter()
        .filter(|m| !m.new_label.trim().is_empty())
        .map(|m| (folded(m.new_label.trim()), m))
        .collect();
    let mut searchable = 0;
    let mut results = Vec::new();
    for page in 0..source.page_count().map_err(|_| citation_unreadable())? {
        let raw = source
            .extract_text(page)
            .map_err(|_| citation_unreadable())?;
        let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if !text.is_empty() {
            searchable += 1;
        }
        for matched in PATTERN.captures_iter(&text) {
            let token = matched[1]
                .trim_end_matches(['.', ',', ';', ':', ')'])
                .trim();
            let normalized = folded(token);
            let previous = old.get(&normalized);
            let proposed = new.get(&normalized);
            let (status, target) = if let Some(item) = previous {
                ("old_label", Some(*item))
            } else if let Some(item) = proposed {
                ("proposed_label", Some(*item))
            } else {
                ("unmatched", None)
            };
            let found = matched.get(0).unwrap();
            let before = &text[..found.start()];
            let after = &text[found.end()..];
            let start = before
                .char_indices()
                .rev()
                .nth(64)
                .map_or(0, |(start, _)| start);
            let end = after
                .char_indices()
                .nth(90)
                .map_or(text.len(), |(end, _)| found.end() + end);
            let snippet = format!(
                "{}{}{}",
                if start == 0 { "" } else { "…" },
                &text[start..end],
                if end == text.len() { "" } else { "…" }
            );
            results.push(
                json!({"page":page+1,"raw":found.as_str(),"label":token,"status":status,
                "document_id":target.map(|m| &m.document_id),"snippet":snippet}),
            );
            if results.len() == 200 {
                return Ok(
                    json!({"revision":revision,"searchable_pages":searchable,"references":results,"truncated":true}),
                );
            }
        }
    }
    Ok(
        json!({"revision":revision,"searchable_pages":searchable,"references":results,"truncated":false}),
    )
}

fn citation_unreadable() -> Reply {
    refusal(
        409,
        "Citation review could not read the selected working copy",
    )
}

fn citation_stale() -> Reply {
    refusal(
        422,
        "Reference document changed after preview; preview again",
    )
}

struct CitationSource {
    document: String,
    revision: u64,
    title: Option<Value>,
    source: PdfSource,
}

struct CitationProof {
    document: String,
    revision: u64,
    proof: PdfProof,
}

impl CitationSource {
    fn compute(self, mappings: &[Mapping]) -> Result<(Value, CitationProof), Reply> {
        let Self {
            document,
            revision,
            title,
            source,
        } = self;
        let (bytes, proof) = source
            .read_with_proof(|| false)
            .map_err(|_| citation_unreadable())?;
        let mut body = references(bytes, mappings, revision)?;
        if let Some(title) = title {
            body["document_id"] = json!(document);
            body["title"] = title;
        }
        Ok((
            body,
            CitationProof {
                document,
                revision,
                proof,
            },
        ))
    }
}

fn capture_citation(
    store: &Store,
    docs: &[Value],
    id: Option<&str>,
    expected: Option<Option<u64>>,
) -> Result<Option<CitationSource>, Reply> {
    let Some(id) = id.filter(|id| !id.is_empty()) else {
        return Ok(None);
    };
    if expected == Some(None) {
        return Err(refusal(422, "Expected reference revision is required"));
    }
    let reference = docs.iter().find(|doc| doc["id"] == id).ok_or_else(|| {
        refusal(
            422,
            "Reference document is missing, archived or belongs to another matter",
        )
    })?;
    let revision = reference["revision"]
        .as_u64()
        .ok_or_else(|| refusal(409, "Reference revision is unavailable"))?;
    if expected
        .flatten()
        .is_some_and(|expected| expected != revision)
    {
        return Err(refusal(
            422,
            "Reference document changed after preview; preview again",
        ));
    }
    let source = store
        .pdf_source(
            id,
            &Artifact::Working {
                revision: Some(revision),
            },
        )
        .map_err(|_| citation_unreadable())?;
    let title = expected.is_none().then(|| reference["title"].clone());
    Ok(Some(CitationSource {
        document: id.to_owned(),
        revision,
        title,
        source,
    }))
}

enum LabelOperation {
    Preview,
    Apply,
}

pub struct LabelReviewTarget {
    matter: String,
    operation: LabelOperation,
}

impl LabelReviewTarget {
    pub fn from_path(method: &str, path: &str) -> Option<Self> {
        let route = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
        Self::select(method, &route)
    }

    fn select(method: &str, route: &[&str]) -> Option<Self> {
        let ("POST", ["matters", matter, "labels", operation]) = (method, route) else {
            return None;
        };
        let operation = match *operation {
            "preview" => LabelOperation::Preview,
            "apply" => LabelOperation::Apply,
            _ => return None,
        };
        Some(Self {
            matter: (*matter).to_owned(),
            operation,
        })
    }
}

enum LabelRequest {
    Preview(Proposal),
    Apply(Apply),
}

enum PreparedLabelAction {
    Preview {
        result: Value,
        mappings: Vec<Mapping>,
        ids: Vec<String>,
    },
    Apply(Apply),
}

pub struct LabelReviewWork {
    root: PathBuf,
    matter: String,
    documents: ReviewSnapshot,
    citation: Option<CitationSource>,
    action: PreparedLabelAction,
}

pub struct MeasuredLabelReview {
    root: PathBuf,
    matter: String,
    documents: ReviewSnapshot,
    reference: Value,
    proof: Option<CitationProof>,
    action: PreparedLabelAction,
}

impl LabelReviewWork {
    pub fn compute(self) -> Result<MeasuredLabelReview, Reply> {
        let Self {
            root,
            matter,
            documents,
            citation,
            action,
        } = self;
        let mappings = match &action {
            PreparedLabelAction::Preview { mappings, .. } => mappings,
            PreparedLabelAction::Apply(request) => &request.mappings,
        };
        let (reference, proof) = if let Some(citation) = citation {
            let (reference, proof) = citation.compute(mappings)?;
            (reference, Some(proof))
        } else {
            (Value::Null, None)
        };
        Ok(MeasuredLabelReview {
            root,
            matter,
            documents,
            reference,
            proof,
            action,
        })
    }
}

fn apply(
    tx: &Connection,
    id: &str,
    request: Apply,
    mut docs: Vec<Value>,
    reference: Value,
    export_history: Value,
) -> Result<Value, Reply> {
    let mut missing = Vec::new();
    if !request.acknowledge_existing_labels
        && request
            .mappings
            .iter()
            .any(|m| !m.old_label.trim().is_empty() && m.old_label.trim() != m.new_label.trim())
    {
        missing.push("existing labels and the old-to-new mapping");
    }
    if export_history["has_prior_exports"] == true && !request.acknowledge_prior_exports {
        missing.push("prior generated builds, which remain unchanged and may have circulated");
    }
    if !request.acknowledge_references
        && reference["references"]
            .as_array()
            .is_some_and(|results| !results.is_empty())
    {
        missing.push("citation references requiring human review");
    }
    if !missing.is_empty() {
        return Err(refusal(
            409,
            format!("Acknowledge {} before applying labels", missing.join("; ")),
        ));
    }
    let by_id: HashMap<_, _> = docs
        .iter()
        .enumerate()
        .map(|(index, doc)| (s(doc, "id"), index))
        .collect();
    let mut changes = Vec::new();
    let mut final_labels: HashMap<_, _> = docs
        .iter()
        .map(|doc| (s(doc, "id"), s(doc, "label").trim().to_owned()))
        .collect();
    for mapping in &request.mappings {
        let Some(&index) = by_id.get(mapping.document_id.as_str()) else {
            return Err(refusal(
                422,
                "A mapped document is missing or archived; preview again",
            ));
        };
        let doc = &docs[index];
        if doc["label"] != mapping.old_label {
            return Err(refusal(
                422,
                format!(
                    "Label for “{}” changed after preview; preview again",
                    s(doc, "title")
                ),
            ));
        }
        let label = mapping.new_label.trim();
        if label.is_empty() {
            return Err(refusal(
                422,
                "Every proposed label must contain 1 to 60 visible characters",
            ));
        }
        final_labels.insert(&mapping.document_id, label.to_owned());
        changes.push((
            index,
            Map::from_iter([
                ("label".into(), json!(label)),
                ("label_status".into(), json!("confirmed")),
                ("label_source".into(), json!("proposal")),
            ]),
        ));
    }
    if let Some(label) = duplicate_labels(docs.iter().map(|doc| final_labels[s(doc, "id")].clone()))
        .into_iter()
        .next()
    {
        return Err(refusal(422, format!("Label “{label}” would be duplicated")));
    }
    document_metadata::write_batch(tx, id, &mut docs, changes, "document_labels_applied")
        .map_err(|_| review_store_error())?;
    Ok(
        json!({"documents":docs,"applied":request.mappings.len(),"reference_review":reference,"export_history":export_history}),
    )
}

impl Store {
    pub fn prepare_label_review(
        &self,
        target: LabelReviewTarget,
        body: &Value,
    ) -> Result<LabelReviewWork, Reply> {
        let LabelReviewTarget { matter, operation } = target;
        let parsed = match operation {
            LabelOperation::Preview => LabelRequest::Preview(
                proposal(body).ok_or_else(|| refusal(422, "Invalid label proposal request"))?,
            ),
            LabelOperation::Apply => LabelRequest::Apply(
                Apply::deserialize(body)
                    .ok()
                    .filter(|request| {
                        (1..=1000).contains(&request.mappings.len())
                            && request.expected_reference_revision != Some(0)
                            && request.mappings.iter().all(|mapping| {
                                mapping.old_label.chars().count() <= 60
                                    && (1..=60).contains(&mapping.new_label.chars().count())
                            })
                    })
                    .ok_or_else(|| refusal(422, "Invalid label mapping request"))?,
            ),
        };
        let documents = ReviewSnapshot::capture(&self.con, &matter)?;
        let (action, citation) = match parsed {
            LabelRequest::Preview(config) => {
                let result = propose(&documents.documents, &config)?;
                let mappings = result["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| Mapping {
                        document_id: s(row, "document_id").into(),
                        old_label: s(row, "old_label").into(),
                        new_label: s(row, "proposed_label").into(),
                    })
                    .collect();
                let citation = capture_citation(
                    self,
                    &documents.documents,
                    config.reference_document_id.as_deref(),
                    None,
                )?;
                (
                    PreparedLabelAction::Preview {
                        result,
                        mappings,
                        ids: config.ids,
                    },
                    citation,
                )
            }
            LabelRequest::Apply(request) => {
                let ids = request
                    .mappings
                    .iter()
                    .map(|row| row.document_id.as_str())
                    .collect::<BTreeSet<_>>();
                if ids.len() != request.mappings.len() {
                    return Err(refusal(422, "Map each document once"));
                }
                let citation = capture_citation(
                    self,
                    &documents.documents,
                    request.reference_document_id.as_deref(),
                    Some(request.expected_reference_revision),
                )?;
                (PreparedLabelAction::Apply(request), citation)
            }
        };
        Ok(LabelReviewWork {
            root: self.root.clone(),
            matter,
            documents,
            citation,
            action,
        })
    }

    pub fn complete_label_review(&mut self, measured: MeasuredLabelReview) -> Result<Reply, Reply> {
        let MeasuredLabelReview {
            root,
            matter: id,
            documents,
            reference,
            proof,
            action,
        } = measured;
        if root != self.root {
            return Err(refusal(
                409,
                "Documents changed during label review; preview again",
            ));
        }
        let tx = self
            .con
            .transaction_with_behavior(if matches!(action, PreparedLabelAction::Apply(_)) {
                TransactionBehavior::Immediate
            } else {
                TransactionBehavior::Deferred
            })
            .map_err(|_| review_store_error())?;
        let current = ReviewSnapshot::capture(&tx, &id)?;
        if let Some(CitationProof {
            document,
            revision,
            proof,
        }) = proof
        {
            let source = Self::pdf_source_at(
                &tx,
                &self.root,
                self.key.as_ref(),
                &document,
                &Artifact::Working {
                    revision: Some(revision),
                },
            )
            .map_err(|_| citation_stale())?;
            if !proof.matches(&source) {
                return Err(citation_stale());
            }
            proof.check_current().map_err(|_| citation_stale())?;
        }
        if !documents.matches(&current) {
            return Err(refusal(
                409,
                "Documents changed during label review; preview again",
            ));
        }
        let result = match action {
            PreparedLabelAction::Preview {
                mut result, ids, ..
            } => {
                if reference["searchable_pages"] == 0 {
                    result["warnings"].as_array_mut().unwrap().push(json!(
                        "The selected reference document has no searchable text. OCR may help, but OCR results remain unverified."));
                }
                result["reference_review"] = reference;
                let ids = ids.iter().map(String::as_str).collect::<BTreeSet<_>>();
                result["export_history"] = super::generated_exports::label_history(&tx, &id, &ids)
                    .map_err(|_| review_store_error())?;
                result["policy"] = json!("Technical proposal only. Confirm the mapping and check court-specific requirements; no numbering convention is represented as court-approved.");
                result
            }
            PreparedLabelAction::Apply(request) => {
                let ids = request
                    .mappings
                    .iter()
                    .map(|row| row.document_id.as_str())
                    .collect::<BTreeSet<_>>();
                let history = super::generated_exports::label_history(&tx, &id, &ids)
                    .map_err(|_| review_store_error())?;
                apply(&tx, &id, request, current.documents, reference, history)?
            }
        };
        tx.commit().map_err(|_| review_store_error())?;
        Ok(reply(200, result))
    }

    pub(super) fn label_request(
        &mut self,
        method: &str,
        route: &[&str],
        body: &Value,
    ) -> rusqlite::Result<Option<Reply>> {
        let Some(target) = LabelReviewTarget::select(method, route) else {
            return Ok(None);
        };
        let result = self
            .prepare_label_review(target, body)
            .and_then(LabelReviewWork::compute)
            .and_then(|measured| self.complete_label_review(measured));
        Ok(Some(result.unwrap_or_else(|error| error)))
    }
}
