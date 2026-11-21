//! Revision-bound local metadata suggestions and atomic reviewed edits.
use super::{
    artifacts::{refusal, PdfProof, PdfSource},
    document_metadata::{self as metadata, request::Save, review_store_error, ReviewSnapshot},
    revisions, *,
};
use regex::Regex;
use serde::Deserialize;
use serde_json::Map;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::LazyLock,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    ids: Vec<String>,
    #[serde(default)]
    pages: BTreeMap<String, u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptedField {
    document_id: String,
    field: String,
    value: String,
    evidence_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Accept {
    ids: Vec<String>,
    #[serde(default)]
    pages: BTreeMap<String, u64>,
    expected_snapshot: String,
    fields: Vec<AcceptedField>,
    sort_group: Option<String>,
}

type DocumentChanges = BTreeMap<String, Map<String, Value>>;

enum Action {
    Snapshot,
    Suggest(Scope),
    Accept(Accept),
    Save(Save),
}

fn s<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap_or_default()
}

fn digest(value: Value) -> String {
    sha256_hex(super::preparation::python_json(&value).as_bytes())
}

fn date_candidates(text: &str) -> (String, Vec<String>, bool) {
    const MONTHS: &[&[&str]] = &[
        &["january", "jan"],
        &["february", "feb"],
        &["march", "mar"],
        &["april", "apr"],
        &["may"],
        &["june", "jun"],
        &["july", "jul"],
        &["august", "aug"],
        &["september", "sep"],
        &["october", "oct"],
        &["november", "nov"],
        &["december", "dec"],
    ];
    static DATE: LazyLock<Regex> = LazyLock::new(|| {
        let names = MONTHS
            .iter()
            .flat_map(|names| names.iter().copied())
            .collect::<Vec<_>>()
            .join("|");
        Regex::new(&format!(r"(?i)\b(?:\d{{4}}-\d{{2}}-\d{{2}}|\d{{1,2}}[ /.-]\d{{1,2}}[ /.-]\d{{4}}|\d{{1,2}}[ -]+(?:{names})[ ,-]+\d{{4}})\b")).unwrap()
    });
    let mut values = BTreeSet::new();
    let mut evidence = Vec::new();
    let mut uncertain = false;
    for found in DATE.find_iter(&text.replace('_', " ")) {
        let raw = found.as_str();
        evidence.push(raw.to_owned());
        let parts = raw
            .split([' ', '-', ',', '/', '.'])
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let parsed = if raw.len() == 10 && raw.as_bytes()[4] == b'-' && raw.as_bytes()[7] == b'-' {
            chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()
        } else if let [day, month, year] = parts.as_slice() {
            let month = MONTHS
                .iter()
                .position(|names| names.iter().any(|name| name.eq_ignore_ascii_case(month)));
            month
                .zip(day.parse::<u32>().ok())
                .zip(year.parse::<i32>().ok())
                .and_then(|((month, day), year)| {
                    chrono::NaiveDate::from_ymd_opt(year, month as u32 + 1, day)
                })
        } else {
            None
        };
        if let Some(date) = parsed.filter(|date| chrono::Datelike::year(date) > 0) {
            values.insert(date.to_string());
        } else {
            uncertain = true;
        }
    }
    let value = values
        .iter()
        .next()
        .filter(|_| values.len() == 1 && !uncertain)
        .cloned()
        .unwrap_or_default();
    (value, evidence, uncertain || values.len() > 1)
}

struct EvidenceSource {
    document: String,
    page: u64,
    working_hash: String,
    pdf: Option<PdfSource>,
}

struct EvidenceProof {
    document: String,
    revision: Option<u64>,
    pdf: Option<PdfProof>,
}

impl EvidenceProof {
    fn check_current(
        &self,
        con: &Connection,
        root: &Path,
        key: Option<&DataKey>,
    ) -> Result<(), Reply> {
        let current = Store::pdf_source_at(
            con,
            root,
            key,
            &self.document,
            &Artifact::Working {
                revision: self.revision,
            },
        );
        match (&self.pdf, current) {
            (Some(proof), Ok(source)) if proof.matches(&source) => {
                proof.check_current().map_err(|_| source_changed())
            }
            (None, Err(_)) => Ok(()),
            (None, Ok(source)) if !source.can_open_file() => Ok(()),
            _ => Err(source_changed()),
        }
    }
}

fn source_changed() -> Reply {
    refusal(
        409,
        "Source evidence changed; reload suggestions before accepting",
    )
}

fn review_stale(action: &Action) -> Reply {
    if matches!(action, Action::Save(_)) {
        refusal(
            409,
            "Review is stale: matter changed. Keep your draft or discard and reload.",
        )
    } else {
        refusal(409, "Review is stale: matter or source changed. Drafts are retained; reload and review again.")
    }
}

fn collect_sources(
    store: &Store,
    docs: &[Value],
    scope: &Scope,
) -> Result<Vec<EvidenceSource>, Reply> {
    let by_id: HashMap<_, _> = docs.iter().map(|doc| (s(doc, "id"), doc)).collect();
    let ids: HashSet<_> = scope.ids.iter().map(String::as_str).collect();
    if ids.len() != scope.ids.len() || ids.iter().any(|id| !by_id.contains_key(id)) {
        return Err(refusal(
            422,
            "Choose existing documents from this matter once each",
        ));
    }
    if scope
        .pages
        .iter()
        .any(|(id, page)| !ids.contains(id.as_str()) || *page == 0)
    {
        return Err(refusal(
            422,
            "Source pages must belong to the review selection",
        ));
    }
    let mut sources = Vec::with_capacity(scope.ids.len());
    for id in &scope.ids {
        let doc = by_id[id.as_str()];
        let page = scope.pages.get(id).copied().unwrap_or(1);
        if doc["page_count"].as_u64().is_none_or(|count| page > count) {
            return Err(refusal(422, "Source page is outside the current document"));
        }
        let revision = doc["revision"].as_u64();
        let working_hash = if store.key.is_some() {
            revisions::record(&store.con, id, revision.unwrap_or_default())
                .map_err(|_| review_store_error())?
                .map(|record| record.hash)
                .unwrap_or_default()
        } else {
            s(doc, "source_sha256").to_owned()
        };
        let pdf = store.pdf_source(id, &Artifact::Working { revision }).ok();
        sources.push(EvidenceSource {
            document: id.clone(),
            page,
            working_hash,
            pdf,
        });
    }
    Ok(sources)
}

fn proposals(
    docs: &[Value],
    sources: Vec<EvidenceSource>,
    snapshot: &str,
) -> (Value, Vec<EvidenceProof>) {
    let by_id: HashMap<_, _> = docs.iter().map(|doc| (s(doc, "id"), doc)).collect();
    let mut rows = Vec::new();
    let mut proofs = Vec::new();
    for EvidenceSource {
        document: id,
        page,
        mut working_hash,
        pdf,
    } in sources
    {
        let doc = by_id[id.as_str()];
        let revision = doc["revision"].as_u64();
        let (text, proof) = match pdf.and_then(|pdf| pdf.open_with_proof().ok()) {
            Some((snapshot, proof)) => {
                let text = snapshot
                    .into_bytes()
                    .ok()
                    .and_then(|bytes| pdf_oxide::PdfDocument::from_bytes(bytes).ok())
                    .and_then(|pdf| pdf.extract_text(page as usize - 1).ok());
                (text, Some(proof))
            }
            None => {
                working_hash.clear();
                (None, None)
            }
        };
        let available = text.is_some();
        let text = text
            .unwrap_or_default()
            .chars()
            .take(40_000)
            .collect::<String>();
        let mut title = json!({"document_id":id,"original_name":doc["original_name"],"revision":revision,
            "source_sha256":doc["source_sha256"],"working_sha256":working_hash,"page":page,"group_key":doc["group_key"]});
        let mut date = title.clone();
        let stem = Path::new(s(doc, "original_name"))
            .file_stem()
            .and_then(|v| v.to_str())
            .unwrap_or_default();
        let proposed = stem
            .replace('_', " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(250)
            .collect::<String>();
        title.as_object_mut().unwrap().extend(
            json!({
                "field":"title","current":doc["title"],"proposed":proposed,
                "status":if proposed.is_empty() {"unavailable"} else {"proposed"},
                "source":"filename","evidence":doc["original_name"],
                "note":"Filename wording only; review the title before accepting.",
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let (value, mut matches, ambiguous) =
            date_candidates(&format!("{}\n{text}", s(doc, "original_name")));
        let (_, page_matches, _) = date_candidates(&text);
        let status = if !value.is_empty() {
            "proposed"
        } else if ambiguous {
            "uncertain"
        } else {
            "unavailable"
        };
        let note = match status {
            "proposed" => "Source text is not legally verified.",
            "uncertain" => "Review required: numeric, conflicting or invalid dates.",
            _ if available => {
                "No unambiguous date found. Enter a date only after checking the source."
            }
            _ => "Working source is unavailable. Restore it before accepting a date.",
        };
        let mut seen = HashSet::new();
        matches.retain(|value| seen.insert(value.clone()));
        let evidence = if matches.is_empty() {
            if available {
                "No date in selected page or filename"
            } else {
                "Source unavailable"
            }
            .into()
        } else {
            matches.join(" · ")
        };
        date.as_object_mut().unwrap().extend(json!({
            "field":"document_date","current":doc["document_date"],"proposed":value,"status":status,
            "source":if page_matches.is_empty() {"filename"} else {"page"},"available":available,
            "evidence":evidence,"note":note,
        }).as_object().unwrap().clone());
        for mut row in [title, date] {
            row["evidence_id"] = json!(digest(json!([row, text, available])));
            rows.push(row);
        }
        proofs.push(EvidenceProof {
            document: id,
            revision,
            pdf: proof,
        });
    }
    let document_versions = docs
        .iter()
        .map(|doc| (s(doc, "id").to_owned(), doc["updated_at"].clone()))
        .collect::<Map<_, _>>();
    (
        json!({"input_snapshot":snapshot,"rows":rows,"document_versions":document_versions}),
        proofs,
    )
}

fn accepted_changes(
    payload: &Accept,
    suggestions: &Value,
) -> Result<(DocumentChanges, Vec<Value>), Reply> {
    let rows: HashMap<_, _> = suggestions["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| ((s(row, "document_id"), s(row, "field")), row))
        .collect();
    let mut seen = HashSet::new();
    let mut changes = BTreeMap::<String, Map<String, Value>>::new();
    let mut evidence = Vec::new();
    for field in &payload.fields {
        let key = (field.document_id.as_str(), field.field.as_str());
        let Some(&row) = rows.get(&key).filter(|_| seen.insert(key)) else {
            return Err(refusal(
                422,
                "Accept each selected field once within the review selection",
            ));
        };
        if row["evidence_id"] != field.evidence_id {
            return Err(refusal(
                409,
                "Source evidence changed; reload suggestions before accepting",
            ));
        }
        let value = field.value.trim();
        if value.is_empty() || value.chars().count() > 250 || value.chars().any(|c| c < ' ') {
            return Err(refusal(422, "Enter 1–250 visible characters"));
        }
        let patch = changes.entry(field.document_id.clone()).or_default();
        if field.field == "document_date" {
            if row["available"] != true {
                return Err(refusal(422, "Source unavailable; date cannot be accepted"));
            }
            if !metadata::iso_date(value) {
                return Err(refusal(422, "Enter an unambiguous ISO date (YYYY-MM-DD)"));
            }
            patch.extend(json!({"document_date":value,"date_status":"confirmed","date_source":"user","date_note":""}).as_object().unwrap().clone());
        } else {
            patch.insert("title".into(), json!(value));
        }
        let mut reviewed = row.clone();
        reviewed["accepted_value"] = json!(value);
        evidence.push(reviewed);
    }
    Ok((changes, evidence))
}

fn validated_changes(
    docs: &[Value],
    changes: &mut DocumentChanges,
    sort_group: Option<&str>,
) -> Result<(), Reply> {
    let ids: HashSet<_> = docs.iter().map(|doc| s(doc, "id")).collect();
    if changes.keys().any(|id| !ids.contains(id.as_str())) {
        return Err(refusal(422, "A reviewed document is missing or archived"));
    }
    let mut parents = HashSet::new();
    let mut final_docs = docs.to_vec();
    for doc in &mut final_docs {
        if let Some(fields) = changes.get(s(doc, "id")) {
            if fields
                .get("title")
                .and_then(Value::as_str)
                .is_some_and(|title| title.trim().is_empty())
            {
                return Err(refusal(422, "Title must contain visible characters"));
            }
            doc.as_object_mut().unwrap().extend(fields.clone());
        }
        let group = s(doc, "group_key");
        if doc["group_role"] == "parent" && (group.is_empty() || !parents.insert(group.to_owned()))
        {
            return Err(refusal(
                422,
                "Each named group can have only one designated parent",
            ));
        }
    }
    if let Some(group) = sort_group {
        if group.trim().is_empty() {
            return Err(refusal(422, "Select a named group before sorting"));
        }
        let members = final_docs
            .iter()
            .filter(|doc| doc["group_key"] == group)
            .collect::<Vec<_>>();
        if !members.iter().any(|doc| changes.contains_key(s(doc, "id"))) {
            return Err(refusal(422, "Sort only a group containing accepted fields"));
        }
        let mut ordered = members.clone();
        ordered.sort_by_key(|doc| {
            let date = if doc["date_status"] == "confirmed" {
                s(doc, "document_date")
            } else {
                ""
            };
            (date.is_empty(), date)
        });
        for (slot, doc) in members.iter().zip(ordered) {
            changes
                .entry(s(doc, "id").to_owned())
                .or_default()
                .insert("position".into(), slot["position"].clone());
        }
    }
    Ok(())
}

enum MetadataReviewOperation {
    Snapshot,
    Suggest,
    Accept,
    Save,
}

pub struct MetadataReviewTarget {
    matter: String,
    operation: MetadataReviewOperation,
}

impl MetadataReviewTarget {
    pub fn from_path(method: &str, path: &str) -> Option<Self> {
        let route = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
        Self::select(method, &route)
    }

    fn select(method: &str, route: &[&str]) -> Option<Self> {
        let ["matters", id, rest @ ..] = route else {
            return None;
        };
        let operation = match (method, rest) {
            ("GET", ["metadata-review"]) => MetadataReviewOperation::Snapshot,
            ("POST", ["metadata-suggestions"]) => MetadataReviewOperation::Suggest,
            ("POST", ["metadata-suggestions", "accept"]) => MetadataReviewOperation::Accept,
            ("PUT", ["metadata-review"]) => MetadataReviewOperation::Save,
            _ => return None,
        };
        Some(Self {
            matter: (*id).to_owned(),
            operation,
        })
    }
}

pub struct MetadataReviewWork {
    root: PathBuf,
    matter: String,
    documents: ReviewSnapshot,
    snapshot: String,
    sources: Vec<EvidenceSource>,
    action: Action,
}

enum ReviewResult {
    Read(Value),
    Change {
        changes: DocumentChanges,
        evidence: Vec<Value>,
        proofs: Vec<EvidenceProof>,
    },
}

pub struct MeasuredMetadataReview {
    root: PathBuf,
    matter: String,
    original: ReviewSnapshot,
    action: Action,
    result: ReviewResult,
}

impl MetadataReviewWork {
    pub fn compute(self) -> Result<MeasuredMetadataReview, Reply> {
        let MetadataReviewWork {
            root,
            matter,
            documents,
            snapshot,
            sources,
            action,
        } = self;
        let result = match &action {
            Action::Snapshot => ReviewResult::Read(
                json!({"input_snapshot":snapshot,"documents":documents.documents}),
            ),
            Action::Suggest(_) => {
                let (suggested, _) = proposals(&documents.documents, sources, &snapshot);
                ReviewResult::Read(suggested)
            }
            Action::Accept(request) => {
                let (suggested, mut proofs) = proposals(&documents.documents, sources, &snapshot);
                let (mut changes, evidence) = accepted_changes(request, &suggested)?;
                proofs.retain(|proof| changes.contains_key(&proof.document));
                validated_changes(
                    &documents.documents,
                    &mut changes,
                    request.sort_group.as_deref(),
                )?;
                ReviewResult::Change {
                    changes,
                    evidence,
                    proofs,
                }
            }
            Action::Save(request) => {
                let mut changes = BTreeMap::new();
                for row in &request.rows {
                    if changes.contains_key(&row.document_id) {
                        return Err(refusal(422, "Save each document once"));
                    }
                    changes.insert(row.document_id.clone(), row.changes.clone());
                }
                validated_changes(&documents.documents, &mut changes, None)?;
                ReviewResult::Change {
                    changes,
                    evidence: Vec::new(),
                    proofs: Vec::new(),
                }
            }
        };
        Ok(MeasuredMetadataReview {
            root,
            matter,
            original: documents,
            action,
            result,
        })
    }
}

impl Store {
    pub fn prepare_metadata_review(
        &self,
        target: MetadataReviewTarget,
        body: &Value,
    ) -> Result<MetadataReviewWork, Reply> {
        self.prepare_metadata_review_with_order(target, body, &crate::RequestOrder::default())
    }

    pub fn prepare_metadata_review_with_order(
        &self,
        target: MetadataReviewTarget,
        body: &Value,
        order: &crate::RequestOrder,
    ) -> Result<MetadataReviewWork, Reply> {
        let MetadataReviewTarget { matter, operation } = target;
        let valid_scope = |ids: &[String]| (1..=500).contains(&ids.len());
        let action = match operation {
            MetadataReviewOperation::Snapshot => Some(Action::Snapshot),
            MetadataReviewOperation::Suggest => Scope::deserialize(body)
                .ok()
                .filter(|scope| valid_scope(&scope.ids))
                .map(Action::Suggest),
            MetadataReviewOperation::Accept => Accept::deserialize(body)
                .ok()
                .filter(|request| {
                    valid_scope(&request.ids) && (1..=1000).contains(&request.fields.len())
                })
                .map(Action::Accept),
            MetadataReviewOperation::Save => {
                Some(Action::Save(metadata::request::save(body, order)?))
            }
        }
        .ok_or_else(|| refusal(422, "Invalid metadata review request"))?;
        let documents = ReviewSnapshot::capture(&self.con, &matter)?;
        let snapshot = digest(json!([documents.matter, documents.documents]));
        let sources = match &action {
            Action::Suggest(scope) => collect_sources(self, &documents.documents, scope)?,
            Action::Accept(request) => {
                if request.expected_snapshot != snapshot {
                    return Err(review_stale(&action));
                }
                let scope = Scope {
                    ids: request.ids.clone(),
                    pages: request.pages.clone(),
                };
                collect_sources(self, &documents.documents, &scope)?
            }
            Action::Save(request) if request.expected_snapshot != snapshot => {
                return Err(review_stale(&action));
            }
            _ => Vec::new(),
        };
        Ok(MetadataReviewWork {
            root: self.root.clone(),
            matter,
            documents,
            snapshot,
            sources,
            action,
        })
    }

    pub fn complete_metadata_review(
        &mut self,
        measured: MeasuredMetadataReview,
    ) -> Result<Reply, Reply> {
        let MeasuredMetadataReview {
            root,
            matter: id,
            original,
            action,
            result,
        } = measured;
        if root != self.root {
            return Err(review_stale(&action));
        }
        match result {
            ReviewResult::Read(result) => Ok(reply(200, result)),
            ReviewResult::Change {
                mut changes,
                evidence,
                proofs,
            } => {
                let tx = self
                    .con
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|_| review_store_error())?;
                let current = ReviewSnapshot::capture(&tx, &id)?;
                if !original.matches(&current) {
                    return Err(review_stale(&action));
                }
                for proof in proofs {
                    proof.check_current(&tx, &self.root, self.key.as_ref())?;
                }
                let mut docs = current.documents;
                let changes = docs
                    .iter()
                    .enumerate()
                    .filter_map(|(index, doc)| {
                        let mut patch = changes.remove(s(doc, "id"))?;
                        patch.retain(|key, value| doc[key] != *value);
                        (!patch.is_empty()).then_some((index, patch))
                    })
                    .collect::<Vec<_>>();
                metadata::write_batch_with_evidence(
                    &tx,
                    &id,
                    &mut docs,
                    changes,
                    "metadata_review_accepted",
                    Some(&evidence),
                )
                .map_err(|_| review_store_error())?;
                docs.sort_by(|left, right| {
                    left["position"]
                        .as_i64()
                        .cmp(&right["position"].as_i64())
                        .then_with(|| s(left, "created_at").cmp(s(right, "created_at")))
                });
                tx.commit().map_err(|_| review_store_error())?;
                Ok(reply(200, json!(docs)))
            }
        }
    }

    pub(super) fn metadata_review_request(
        &mut self,
        method: &str,
        route: &[&str],
        body: &Value,
        order: &crate::RequestOrder,
    ) -> rusqlite::Result<Option<Reply>> {
        let Some(target) = MetadataReviewTarget::select(method, route) else {
            return Ok(None);
        };
        let result = self
            .prepare_metadata_review_with_order(target, body, order)
            .and_then(MetadataReviewWork::compute)
            .and_then(|measured| self.complete_metadata_review(measured));
        Ok(Some(result.unwrap_or_else(|refusal| refusal)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_store::VaultStore;

    const PDF: &[u8] = include_bytes!("../../../samples/rust_pdf_corpus/unsigned.pdf");

    fn document(vault: &mut VaultStore) -> (String, String) {
        let ready = vault.request(
            "POST",
            "/vault/setup",
            json!({"password":"Synthetic metadata review fixture"}),
        );
        assert_eq!(ready.status, 200, "{ready:?}");
        let created = vault.request(
            "POST",
            "/matters",
            json!({"name":"Metadata evidence fixture"}),
        );
        let matter = created.body["id"].as_str().unwrap().to_owned();
        let imported = vault.import_pdf(
            &matter,
            "Petition_21_November_2026.pdf",
            PDF,
            ImportOptions::default(),
        );
        assert_eq!(imported.status, 201, "{imported:?}");
        (matter, imported.body[0]["id"].as_str().unwrap().to_owned())
    }

    fn accept_payload(vault: &mut VaultStore, matter: &str, document: &str, field: &str) -> Value {
        let request = vault.request(
            "POST",
            &format!("/matters/{matter}/metadata-suggestions"),
            json!({"ids":[document]}),
        );
        assert_eq!(request.status, 200, "{request:?}");
        let row = request.body["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["field"] == field)
            .unwrap();
        json!({
            "ids":[document],"expected_snapshot":request.body["input_snapshot"],
            "fields":[{"document_id":document,"field":field,"value":row["proposed"],"evidence_id":row["evidence_id"]}]
        })
    }

    #[test]
    fn measured_metadata_and_citations_refuse_a_changed_working_file() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        let mut vault = VaultStore::open(&root).unwrap();
        let (matter, document) = document(&mut vault);
        let payload = accept_payload(&mut vault, &matter, &document, "title");
        let metadata = vault.prepare_metadata_review(
            MetadataReviewTarget::from_path(
                "POST",
                &format!("/matters/{matter}/metadata-suggestions/accept"),
            )
            .unwrap(),
            &payload,
        );
        let metadata = metadata.unwrap().compute().unwrap();
        let label = vault
            .prepare_label_review(
                super::super::LabelReviewTarget::from_path(
                    "POST",
                    &format!("/matters/{matter}/labels/apply"),
                )
                .unwrap(),
                &json!({"mappings":[{"document_id":document,"old_label":"","new_label":"P-1"}],
                "reference_document_id":document,"expected_reference_revision":1,
                "acknowledge_references":true}),
            )
            .unwrap()
            .compute()
            .unwrap();

        let working = root
            .join("documents-v1")
            .join(&document)
            .join("revision-1.pdf");
        fs::write(&working, b"changed ciphertext").unwrap();
        let stale_metadata = vault.complete_metadata_review(metadata).unwrap_err();
        assert_eq!(stale_metadata.status, 409);
        assert_eq!(
            stale_metadata.body["detail"],
            "Source evidence changed; reload suggestions before accepting"
        );
        assert_eq!(vault.complete_label_review(label).unwrap_err().status, 422);
        let snapshot = vault.request(
            "GET",
            &format!("/matters/{matter}/metadata-review"),
            Value::Null,
        );
        assert_eq!(
            snapshot.body["documents"][0]["title"],
            "Petition_21_November_2026"
        );
        assert_eq!(snapshot.body["documents"][0]["label"], "");
    }

    #[test]
    fn a_missing_working_file_allows_filename_review_and_refuses_a_date() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        let mut vault = VaultStore::open(&root).unwrap();
        let (matter, document) = document(&mut vault);
        let folder = root.join("documents-v1").join(&document);
        fs::remove_file(folder.join("revision-1.pdf")).unwrap();
        let accept = format!("/matters/{matter}/metadata-suggestions/accept");

        let date = accept_payload(&mut vault, &matter, &document, "document_date");
        let refused = vault.request("POST", &accept, date);
        assert_eq!(refused.status, 422);
        assert_eq!(
            refused.body["detail"],
            "Source unavailable; date cannot be accepted"
        );

        let title = accept_payload(&mut vault, &matter, &document, "title");
        let applied = vault.request("POST", &accept, title);
        assert_eq!(applied.status, 200, "{applied:?}");
        assert_eq!(applied.body[0]["title"], "Petition 21 November 2026");
        assert_eq!(applied.body[0]["document_date"], "");
        assert_eq!(applied.body[0]["source_sha256"], sha256_hex(PDF));
    }
}
