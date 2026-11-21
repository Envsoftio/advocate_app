//! Revision-qualified technical findings and human review records.
use super::{
    artifacts::{refusal, PdfProof, PdfSource},
    bundle_inputs::{self, BundleInputWork, Inputs, Preview},
    document_metadata,
    document_verification::DocumentFileProof,
    preparation,
    reading::folded,
    *,
};
use pdf_oxide::{
    rendering::{render_page, RenderOptions},
    PdfDocument,
};
use std::collections::{BTreeSet, HashSet};

const REVIEW_ITEMS: &[&str] = &["latest_order", "sequence", "titles", "pagination"];

pub(super) fn migrate(con: &Connection) -> rusqlite::Result<()> {
    con.execute_batch(
        "CREATE TABLE preflight_runs(id TEXT PRIMARY KEY, matter_id TEXT NOT NULL, body TEXT NOT NULL);
         CREATE INDEX preflight_runs_matter ON preflight_runs(matter_id);
         CREATE TABLE preflight_reviews(matter_id TEXT NOT NULL, input_digest TEXT NOT NULL,
             body TEXT NOT NULL, PRIMARY KEY(matter_id,input_digest));
         CREATE TABLE preflight_notes(run_id TEXT NOT NULL, finding_id TEXT NOT NULL,
             body TEXT NOT NULL, PRIMARY KEY(run_id,finding_id));
         PRAGMA user_version=7;"
    )
}

fn s<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap_or_default()
}

fn supported_script(text: &str) -> bool {
    text.chars().all(|c| matches!(c as u32, 32..=0x024f | 0x0900..=0x097f | 0x2000..=0x206f | 0x20a0..=0x20cf | 0xa8e0..=0xa8ff))
}

fn finding(severity: &str, code: &str, message: impl Into<String>) -> Value {
    json!({"severity":severity,"code":code,"message":message.into(),"document_id":null})
}

fn at_document(mut finding: Value, doc: &Value, artifact: &str, page: Option<usize>) -> Value {
    finding["document_id"] = doc["id"].clone();
    finding["target_revision"] = doc["revision"].clone();
    finding["artifact"] = json!(artifact);
    if let Some(page) = page {
        finding["page_index"] = json!(page);
    }
    finding
}

fn original_issues(result: Result<(), Reply>, doc: &Value) -> Option<Value> {
    let id = s(doc, "id");
    let title = s(doc, "title");
    match result {
        Ok(()) => None,
        Err(refusal) => Some(at_document(
            if refusal.status == 404 {
                finding(
                    "error",
                    "MISSING_ORIGINAL",
                    format!("Original missing for {title} (document {id})."),
                )
            } else if s(&refusal.body, "detail").contains("integrity") {
                finding("error","ORIGINAL_HASH_MISMATCH",format!("Original bytes changed for {title} (document {id}); import hash does not match."))
            } else {
                finding(
                    "error",
                    "ORIGINAL_UNREADABLE",
                    format!(
                        "Cannot read original for {title} (document {id}): {}.",
                        s(&refusal.body, "detail")
                    ),
                )
            },
            doc,
            "original",
            None,
        )),
    }
}

fn inspect_pdf(result: Result<PdfSnapshot, Reply>, doc: &Value, profile: &Value) -> Vec<Value> {
    let title = s(doc, "title");
    let error = |code, msg| at_document(finding("error", code, msg), doc, "working", None);
    let snapshot = match result {
        Ok(snapshot) => snapshot,
        Err(refusal) if refusal.status == 404 => {
            return vec![error(
                "MISSING_WORKING_COPY",
                format!("Working file missing: {title}"),
            )]
        }
        Err(refusal) => {
            return vec![error(
                "BAD_PDF",
                format!("Cannot process {title}: {}", s(&refusal.body, "detail")),
            )]
        }
    };
    let pdf = snapshot
        .into_bytes()
        .ok()
        .and_then(|bytes| PdfDocument::from_bytes(bytes).ok());
    let Some(pdf) = pdf else {
        return vec![error(
            "BAD_PDF",
            format!("Cannot process {title}: invalid PDF"),
        )];
    };
    let mut issues = Vec::new();
    if pdf.is_encrypted() && !pdf.is_authenticated() {
        issues.push(error(
            "ENCRYPTED_INPUT",
            format!("Encrypted input: {title}"),
        ));
        return issues;
    }
    let Ok(count) = pdf.page_count() else {
        return vec![error(
            "BAD_PDF",
            format!("Cannot process {title}: invalid page tree"),
        )];
    };
    if doc["page_count"] != count {
        issues.push(error("PAGE_COUNT_MISMATCH",format!("{title}: working PDF page count differs from the recorded source count. Restore or reimport the document.")));
    }
    let mut render = RenderOptions::with_dpi(18).as_raw();
    render.max_output_pixels = 1_000_000;
    let mut annotations = 0;
    let mut links = 0;
    for page in 0..count {
        if render_page(&pdf, page, &render).is_err() {
            issues.push(at_document(
                finding(
                    "error",
                    "PAGE_RENDER_FAILED",
                    format!(
                        "{title}, source page {} could not be rendered: invalid page content",
                        page + 1
                    ),
                ),
                doc,
                "working",
                Some(page),
            ));
        }
        if let Ok(items) = pdf.get_annotations(page) {
            for item in items {
                match item.subtype_enum {
                    pdf_oxide::AnnotationSubtype::Link => links += 1,
                    pdf_oxide::AnnotationSubtype::Widget | pdf_oxide::AnnotationSubtype::Popup => {
                        ()
                    }
                    _ => annotations += 1,
                }
            }
        }
        if profile["require_searchable_text"] == true
            && pdf
                .extract_text(page)
                .is_ok_and(|text| text.trim().is_empty())
        {
            issues.push(at_document(
                finding(
                    "warning",
                    "UNSEARCHABLE_PAGE",
                    format!(
                        "{title}, page {} has no extractable text. Consider OCR/review.",
                        page + 1
                    ),
                ),
                doc,
                "working",
                Some(page),
            ));
        }
    }
    if annotations > 0 {
        issues.push(at_document(finding("warning","SOURCE_ANNOTATIONS_OMITTED",format!("{title} has {annotations} working-copy annotation(s) that will be omitted from volume and combined PDFs; the imported original is retained.")),doc,"working",None));
    }
    if links > 0 {
        issues.push(at_document(finding("warning","SOURCE_LINKS_OMITTED",format!("{title} has {links} working-copy link(s) that will be omitted from volume and combined PDFs; generated index links remain.")),doc,"working",None));
    }
    issues
}

fn document_issues(
    doc: &Value,
    settings: &Value,
    hashes: &mut HashSet<String>,
    labels: &mut HashSet<String>,
) -> Vec<Value> {
    let (title, label, id) = (s(doc, "title"), s(doc, "label"), s(doc, "id"));
    let mut issues = Vec::new();
    let add = |severity, code, message| {
        at_document(finding(severity, code, message), doc, "working", None)
    };
    if !supported_script(&format!("{title}{label}")) {
        issues.push(add("error","UNSUPPORTED_INDEX_SCRIPT",format!("{title} contains a script outside the current Latin/Devanagari visual pilot. Generated index output is blocked until this script is verified.")));
    }
    if s(doc, "document_date").is_empty() {
        issues.push(add("warning","DOCUMENT_DATE_MISSING",format!("{title} has no document date. Review the source; add a known date or acknowledge that it is unavailable.")));
    }
    if doc["kind"] == "annexure" {
        let mode = settings["annexure_overrides"][id]
            .as_str()
            .unwrap_or_else(|| s(settings, "annexure_mode"));
        let printable = |value: &str| value.bytes().all(|byte| (32..=126).contains(&byte));
        if mode != "none" {
            if doc["label_status"] != "confirmed" || label.trim().is_empty() {
                issues.push(add(
                    "error",
                    "UNCONFIRMED_ANNEXURE_MARK",
                    format!(
                        "{title} needs a confirmed register label before {} output.",
                        mode.replace('_', " ")
                    ),
                ));
            } else if !printable(label) {
                issues.push(add(
                    "error",
                    "UNSUPPORTED_ANNEXURE_MARK_SCRIPT",
                    format!("{title} has a label that the technical stamp font cannot render."),
                ));
            }
        }
        if mode == "separator" && !printable(title) {
            issues.push(add(
                "error",
                "UNSUPPORTED_SEPARATOR_TITLE_SCRIPT",
                format!("{title} has a title that the technical separator font cannot render."),
            ));
        }
    }
    match s(doc,"signature_status") {
        "certificate_signature_detected" => issues.push(add("warning","CERTIFICATE_SIGNATURE_INDICATORS",format!("{title} contains PDF certificate-signature indicators. Validity and current byte coverage were not checked; edits, OCR and bundle generation may invalidate or omit signature protections. The imported original is retained."))),
        "inspection_inconclusive" => issues.push(add("warning","SIGNATURE_INSPECTION_INCONCLUSIVE",format!("Signature inspection for {title} was inconclusive. No cryptographic validation was attempted; acknowledge the risk before transforming it."))),
        "visible_signature_only" => issues.push(add("warning","VISIBLE_SIGNATURE_ONLY",format!("{title} was marked by a reviewer as containing a visible or scanned signature, but no PDF certificate-signature indicators were detected. The signature and signer are not verified."))),
        _ => (),
    }
    if !hashes.insert(s(doc, "source_sha256").to_owned()) {
        issues.push(add(
            "warning",
            "DUPLICATE_INPUT",
            format!("Possible duplicate original: {title}"),
        ));
    }
    if doc["kind"] == "annexure" && label.is_empty() {
        issues.push(add(
            "warning",
            "MISSING_ANNEXURE_LABEL",
            format!("Annexure {title} has no identifier."),
        ));
    }
    if !label.is_empty() && !labels.insert(folded(label.trim())) {
        issues.push(add(
            "warning",
            "DUPLICATE_LABEL",
            format!("Duplicate annexure/document label: {label}"),
        ));
    }
    issues
}

fn destination_issues(documents: &[Value], bookmarks: Vec<Value>, links: Vec<Value>) -> Vec<Value> {
    let ids: HashSet<_> = documents.iter().map(|doc| s(doc, "id")).collect();
    let mut issues = Vec::new();
    for item in &bookmarks {
        if !ids.contains(s(item, "document_id")) {
            let mut value = finding("warning","EXCLUDED_BOOKMARK_OMITTED",format!("Bookmark “{}” targets a reference-only or archived document and will be omitted.",s(item,"title")));
            value["document_id"] = item["document_id"].clone();
            issues.push(value);
        }
    }
    for item in &links {
        if ids.contains(s(item, "source_document_id"))
            && item["kind"] == "internal"
            && !ids.contains(s(item, "target_document_id"))
        {
            let mut value = finding("error","EXCLUDED_LINK_DESTINATION",format!("Link “{}” targets excluded material. Retarget or remove this link, or include its destination.",s(item,"title")));
            value["document_id"] = item["source_document_id"].clone();
            value["target_revision"] = item["source_revision"].clone();
            value["page_index"] = item["source_page_index"].clone();
            value["artifact"] = json!("working");
            issues.push(value);
        }
    }
    for item in &bookmarks {
        if ids.contains(s(item, "document_id")) && item["resolved"] == false {
            let mut value = finding("warning","UNRESOLVED_BOOKMARK","A manual bookmark has an unavailable destination and will be omitted. Resolve or remove it in Index & bookmarks.");
            value["document_id"] = item["document_id"].clone();
            value["record_id"] = item["id"].clone();
            value["action"] = json!("bookmarks");
            issues.push(value);
        }
    }
    for item in &links {
        if ids.contains(s(item, "source_document_id")) && item["resolved"] == false {
            let mut value = finding("warning","UNRESOLVED_LINK","A reviewed link has an unavailable endpoint and will be omitted. Review its source document.");
            value["document_id"] = item["source_document_id"].clone();
            value["record_id"] = item["id"].clone();
            issues.push(value);
        }
    }
    issues
}

fn documents(con: &Connection, id: &str) -> rusqlite::Result<Vec<Value>> {
    let mut query = con.prepare("SELECT body FROM documents WHERE matter_id=? AND archived_at IS NULL ORDER BY json_extract(body,'$.position'),created_at")?;
    let rows = query.query_map([id], |row| row.get::<_, String>(0))?;
    rows.map(|row| decode(&row?)).collect()
}

struct TechnicalInputs<'a> {
    matter: &'a Value,
    marks: &'a Value,
    review: &'a Value,
    profile: &'a Value,
    docs: &'a [Value],
}

struct TechnicalWork {
    findings: Vec<Value>,
    sources: Vec<TechnicalSource>,
    profile: Value,
    marks: Value,
}

struct TechnicalSource {
    doc: Value,
    original: Result<PdfSource, Reply>,
    working: Result<PdfSource, Reply>,
}

impl TechnicalWork {
    fn measure(self) -> (Vec<Value>, Vec<PdfProof>) {
        let mut findings = self.findings;
        let mut proofs = Vec::new();
        let mut workings = Vec::with_capacity(self.sources.len());
        for source in self.sources {
            let outcome = source
                .original
                .and_then(PdfSource::open_with_proof)
                .map(|(_, proof)| {
                    proofs.push(proof);
                });
            findings.extend(original_issues(outcome, &source.doc));
            workings.push((source.doc, source.working));
        }
        let mut labels = HashSet::new();
        let mut hashes = HashSet::new();
        for (doc, working) in workings {
            let snapshot = working
                .and_then(PdfSource::open_with_proof)
                .map(|(pdf, proof)| {
                    proofs.push(proof);
                    pdf
                });
            findings.extend(document_issues(&doc, &self.marks, &mut hashes, &mut labels));
            findings.extend(inspect_pdf(snapshot, &doc, &self.profile));
        }
        (findings, proofs)
    }
}

fn technical_work(
    con: &Connection,
    root: &Path,
    key: Option<&DataKey>,
    inputs: TechnicalInputs<'_>,
) -> TechnicalWork {
    let TechnicalInputs {
        matter,
        marks,
        review,
        profile,
        docs,
    } = inputs;
    let mut findings = Vec::new();
    if docs.is_empty() {
        findings.push(finding(
            "error",
            "NO_DOCUMENTS",
            "Include at least one document in Bundle membership before building.",
        ));
    }
    if profile["status"] != "APPROVED" {
        findings.push(finding("info","PROFILE_NOT_APPROVED",format!("{} has no recorded advocate approval. Technical checks do not establish case-specific legal completeness. Review the official references for the applicable proceeding.",s(profile,"name"))));
    }
    if !supported_script(s(matter, "name")) {
        findings.push(finding("error","UNSUPPORTED_INDEX_SCRIPT","The matter name contains a script outside the current Latin/Devanagari visual pilot. Generated index output is blocked until this script is verified."));
    }
    if marks["front_matter"] == "cover_index" {
        let supported = ["cover_heading", "cover_reference", "cover_title"]
            .iter()
            .all(|field| supported_script(s(&marks, field)))
            && ["petitioner", "respondent", "case_number"]
                .iter()
                .all(|field| supported_script(s(matter, field)));
        if !supported {
            findings.push(finding("error","UNSUPPORTED_COVER_SCRIPT","Cover text contains a script outside the current Latin/Devanagari visual pilot. Review supported text before building."));
        }
    }
    let pending = review["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["status"] == "unreviewed")
        .count();
    if pending > 0 {
        findings.push(finding("info","USER_CHECKLIST_PENDING",format!("{pending} user-authored checklist item(s) remain unreviewed. This checklist is not a court-rule determination.")));
    }
    if !s(review, "case_type").is_empty()
        && folded(s(review, "case_type").trim()) != folded(s(matter, "case_type").trim())
    {
        findings.push(finding("warning","USER_CHECKLIST_SCOPE_MISMATCH","The user-authored checklist case type differs from the matter case type. Review whether these items still apply."));
    }
    let sources = docs
        .iter()
        .map(|doc| {
            let id = s(doc, "id");
            TechnicalSource {
                doc: doc.clone(),
                original: Store::pdf_source_at(con, root, key, id, &Artifact::Original),
                working: Store::pdf_source_at(
                    con,
                    root,
                    key,
                    id,
                    &Artifact::Working {
                        revision: doc["revision"].as_u64(),
                    },
                ),
            }
        })
        .collect();
    TechnicalWork {
        findings,
        sources,
        profile: profile.clone(),
        marks: marks.clone(),
    }
}

pub(super) struct VariantPreflightWork {
    technical: TechnicalWork,
    destinations: Vec<Value>,
}

pub(super) struct VariantPreflightMeasurement {
    pub checks: Vec<Value>,
    pub proofs: Vec<PdfProof>,
}

impl VariantPreflightWork {
    pub(super) fn capture(
        store: &Store,
        matter: &Value,
        options: &Value,
        docs: &[Value],
        all: &[Value],
    ) -> Result<Self, Reply> {
        let id = s(matter, "id");
        let load = |_| refusal(500, "Unable to load variant Preflight inputs");
        let profile = bundle_inputs::profile(matter).map_err(load)?;
        let review = preparation::filing_review(&store.con, id)
            .map_err(load)?
            .ok_or_else(|| refusal(409, "Stored filing review is invalid"))?;
        let technical = technical_work(
            &store.con,
            &store.root,
            store.key.as_ref(),
            TechnicalInputs {
                matter,
                marks: &options["output_marks"],
                review: &review,
                profile: &profile,
                docs,
            },
        );
        let (bookmarks, links) =
            bundle_inputs::resolved_destinations(&store.con, id, all).map_err(load)?;
        let destinations = destination_issues(docs, bookmarks, links);
        Ok(Self {
            technical,
            destinations,
        })
    }

    pub(super) fn measure(self) -> VariantPreflightMeasurement {
        let (mut checks, proofs) = self.technical.measure();
        checks.extend(self.destinations);
        VariantPreflightMeasurement { checks, proofs }
    }
}

fn plan_unavailable(detail: &str, plan_loaded: bool) -> Value {
    let mut issue = finding(
        "error",
        "PAGE_MAP_UNAVAILABLE",
        format!("The current volume/page plan cannot be generated: {detail}"),
    );
    if plan_loaded {
        issue["action"] = json!("volumes");
    }
    issue
}

fn add_completion(findings: &mut Vec<Value>) {
    if !findings
        .iter()
        .any(|finding| finding["severity"] == "error")
    {
        findings.push(finding("info","TECHNICAL_PREFLIGHT","Basic technical preflight completed. Legal completeness, authenticity, signatures, limitation and registry acceptance are not verified."));
    }
}

struct RunRecord {
    id: String,
    matter: String,
    body: String,
    findings: Vec<Value>,
}

impl RunRecord {
    fn save(&self, con: &Connection) -> rusqlite::Result<()> {
        con.execute(
            "INSERT INTO preflight_runs(id,matter_id,body) VALUES (?,?,?)",
            params![self.id, self.matter, self.body],
        )?;
        Ok(())
    }
}

fn prepare_record(
    id: &str,
    mut findings: Vec<Value>,
    docs: &[Value],
    marks: Value,
    snapshot: Option<&str>,
    effective: Option<&str>,
) -> RunRecord {
    let run_id = uuid::Uuid::new_v4().simple().to_string();
    for issue in &mut findings {
        let mut identity = bundle_inputs::select(
            issue,
            &[
                "code",
                "document_id",
                "target_revision",
                "page_index",
                "message",
            ],
        );
        if let Some(record) = issue.get("record_id") {
            identity["record_id"] = record.clone();
        }
        issue["input_snapshot"] = json!(snapshot);
        issue["finding_id"] =
            json!(&sha256_hex(preparation::python_json(&identity).as_bytes())[..24]);
        issue["validation_run_id"] = json!(run_id);
        issue["effective_input_digest"] = json!(effective);
        issue["blocks_build"] = json!(issue["severity"] == "error");
    }
    let revisions = docs.iter().map(|doc| json!({"document_id":doc["id"],"revision":doc["revision"],"pages":doc["page_count"]})).collect::<Vec<_>>();
    let body = json!({"id":run_id,"matter_id":id,"build_id":null,"scope":"preflight","created_at":now(),
        "contract":{"input_digest":effective,"revisions":revisions,"settings":marks},"findings":findings});
    RunRecord {
        id: run_id,
        matter: id.into(),
        body: body.to_string(),
        findings,
    }
}

pub struct PreflightWork {
    root: PathBuf,
    matter: String,
    marks: Value,
    docs: Vec<Value>,
    plan: PreflightPlan,
    technical: TechnicalWork,
    destinations: Vec<Value>,
}

pub struct MeasuredPreflight {
    root: PathBuf,
    record: RunRecord,
    binding: RunBinding,
    proofs: Vec<PdfProof>,
    source_proofs: Vec<DocumentFileProof>,
}

enum PreflightPlan {
    Build(PlanEvidence),
    Manual(Box<ManualPlan>),
}

struct PlanEvidence {
    binding: RunBinding,
    issue: rusqlite::Result<Option<Value>>,
    screenings: Vec<Value>,
    source_proofs: Vec<DocumentFileProof>,
}

struct ManualPlan {
    subjects: Box<RunSubjects>,
    loaded: Option<ManualInputs>,
    unavailable_screenings: rusqlite::Result<Vec<Value>>,
}

struct ManualInputs {
    work: BundleInputWork,
    report: rusqlite::Result<(InputDigests, Vec<Value>)>,
}

impl ManualPlan {
    fn measure(self, has_documents: bool) -> Result<PlanEvidence, Reply> {
        if let Some(loaded) = self.loaded {
            if let Ok((inputs, source_proofs)) = loaded.work.verify() {
                let (digests, screenings) = loaded.report.map_err(preflight_failed)?;
                // Preserve the finding order and only surface a planning issue when
                // the technical findings would otherwise allow a build.
                let issue = (|| {
                    if !has_documents {
                        return Ok(None);
                    }
                    let matter = &self.subjects.matter;
                    let reply = inputs.preview_with_snapshot(
                        Preview {
                            max_pages_per_volume: matter["max_pages_per_volume"]
                                .as_u64()
                                .ok_or(rusqlite::Error::InvalidQuery)?
                                as usize,
                            keep_groups_intact: matter["keep_groups_intact"] == true,
                        },
                        &digests.snapshot,
                    )?;
                    Ok((reply.status != 200)
                        .then(|| plan_unavailable(s(&reply.body, "detail"), true)))
                })();
                return Ok(PlanEvidence {
                    binding: RunBinding::Verified(digests),
                    issue,
                    screenings,
                    source_proofs,
                });
            }
        }
        Ok(PlanEvidence {
            binding: RunBinding::Unavailable(self.subjects),
            issue: Ok(Some(plan_unavailable(
                "saved inputs are unavailable",
                false,
            ))),
            screenings: self.unavailable_screenings.map_err(preflight_failed)?,
            source_proofs: Vec::new(),
        })
    }
}

struct InputDigests {
    snapshot: String,
    effective: String,
}

impl InputDigests {
    fn from_inputs(inputs: &Inputs, con: &Connection) -> rusqlite::Result<Self> {
        Ok(Self {
            snapshot: inputs.digest(con)?,
            effective: inputs.effective_digest(con)?,
        })
    }
}

enum RunBinding {
    Verified(InputDigests),
    Unavailable(Box<RunSubjects>),
}

#[derive(PartialEq, Eq)]
struct RunSubjects {
    matter: Value,
    marks: Value,
    all: Vec<Value>,
    review: Value,
    profile: Value,
    bookmarks: Vec<Value>,
    links: Vec<Value>,
}

impl RunSubjects {
    fn capture(con: &Connection, id: &str) -> Result<Self, Reply> {
        let matter = document_metadata::matter(con, id)
            .map_err(preflight_failed)?
            .ok_or_else(|| refusal(404, "Matter not found"))?;
        let marks = bundle_inputs::output_marks(con, id).map_err(preflight_failed)?;
        let profile = bundle_inputs::profile(&matter).map_err(preflight_failed)?;
        let Some(review) = preparation::filing_review(con, id).map_err(preflight_failed)? else {
            return Err(refusal(
                409,
                "Stored filing review is invalid; restore or correct it before building",
            ));
        };
        let all = documents(con, id).map_err(preflight_failed)?;
        let (bookmarks, links) =
            bundle_inputs::resolved_destinations(con, id, &all).map_err(preflight_failed)?;
        Ok(Self {
            matter,
            marks,
            all,
            review,
            profile,
            bookmarks,
            links,
        })
    }
}

pub(super) struct BuildPlanEvidence<'a> {
    pub snapshot: &'a str,
    pub effective: &'a str,
}

fn preflight_failed(_: rusqlite::Error) -> Reply {
    refusal(500, "Rust matter transaction failed; no changes committed")
}

impl PreflightWork {
    pub(super) fn capture(
        store: &Store,
        id: &str,
        plan: Option<BuildPlanEvidence<'_>>,
    ) -> Result<Self, Reply> {
        let con = &store.con;
        let subjects = RunSubjects::capture(con, id)?;
        let docs = subjects
            .all
            .iter()
            .filter(|doc| doc["bundle_included"] == 1)
            .cloned()
            .collect::<Vec<_>>();
        let technical = technical_work(
            con,
            &store.root,
            store.key.as_ref(),
            TechnicalInputs {
                matter: &subjects.matter,
                marks: &subjects.marks,
                review: &subjects.review,
                profile: &subjects.profile,
                docs: &docs,
            },
        );
        let marks = subjects.marks.clone();
        let destinations =
            destination_issues(&docs, subjects.bookmarks.clone(), subjects.links.clone());
        let plan = if let Some(plan) = plan {
            PreflightPlan::Build(PlanEvidence {
                binding: RunBinding::Verified(InputDigests {
                    snapshot: plan.snapshot.into(),
                    effective: plan.effective.into(),
                }),
                issue: Ok(None),
                screenings: super::ocr::findings(con, id, Some(plan.effective))
                    .map_err(preflight_failed)?,
                source_proofs: Vec::new(),
            })
        } else {
            let loaded = Inputs::capture(con, &store.root, store.key.as_ref(), id)
                .map_err(preflight_failed)?
                .ok()
                .map(|work| {
                    let report = (|| {
                        let digests = InputDigests::from_inputs(work.inputs(), con)?;
                        let screenings = super::ocr::findings(con, id, Some(&digests.effective))?;
                        Ok((digests, screenings))
                    })();
                    ManualInputs { work, report }
                });
            PreflightPlan::Manual(Box::new(ManualPlan {
                subjects: Box::new(subjects),
                loaded,
                unavailable_screenings: super::ocr::findings(con, id, None),
            }))
        };
        Ok(Self {
            root: store.root.clone(),
            matter: id.into(),
            marks,
            docs,
            plan,
            technical,
            destinations,
        })
    }

    pub(super) fn set_build_plan(&mut self, reply: &Reply, source_proofs: Vec<DocumentFileProof>) {
        let PreflightPlan::Build(plan) = &mut self.plan else {
            unreachable!("Expected build Preflight")
        };
        plan.issue =
            Ok((reply.status != 200).then(|| plan_unavailable(s(&reply.body, "detail"), true)));
        plan.source_proofs = source_proofs;
    }

    pub fn measure(self) -> Result<MeasuredPreflight, Reply> {
        let plan = match self.plan {
            PreflightPlan::Build(plan) => plan,
            PreflightPlan::Manual(plan) => plan.measure(!self.docs.is_empty())?,
        };
        let (mut findings, proofs) = self.technical.measure();
        findings.extend(self.destinations);
        if !self.docs.is_empty() && !findings.iter().any(|issue| issue["severity"] == "error") {
            findings.extend(plan.issue.map_err(preflight_failed)?);
        }
        add_completion(&mut findings);
        findings.extend(plan.screenings);
        let record = prepare_record(
            &self.matter,
            findings,
            &self.docs,
            self.marks,
            plan.binding.snapshot(),
            plan.binding.effective(),
        );
        Ok(MeasuredPreflight {
            root: self.root,
            record,
            binding: plan.binding,
            proofs,
            source_proofs: plan.source_proofs,
        })
    }
}

impl RunBinding {
    fn snapshot(&self) -> Option<&str> {
        match self {
            Self::Verified(digests) => Some(&digests.snapshot),
            Self::Unavailable(_) => None,
        }
    }

    fn effective(&self) -> Option<&str> {
        match self {
            Self::Verified(digests) => Some(&digests.effective),
            Self::Unavailable(_) => None,
        }
    }

    fn is_current(&self, con: &Connection, matter: &str) -> Result<bool, Reply> {
        match self {
            Self::Verified(expected) => {
                let current = Inputs::metadata(con, matter).map_err(preflight_failed)??;
                let current = InputDigests::from_inputs(&current, con).map_err(preflight_failed)?;
                Ok(
                    current.snapshot == expected.snapshot
                        && current.effective == expected.effective,
                )
            }
            Self::Unavailable(expected) => {
                Ok(*expected.as_ref() == RunSubjects::capture(con, matter)?)
            }
        }
    }
}

impl MeasuredPreflight {
    pub(super) fn findings(&self) -> Vec<Value> {
        self.record.findings.clone()
    }

    fn publish(self, store: &mut Store, stale_message: &str) -> Result<Reply, Reply> {
        if self.root != store.root {
            return Err(refusal(409, stale_message));
        }
        let tx = store
            .con
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(preflight_failed)?;
        if !self.binding.is_current(&tx, &self.record.matter)?
            || self
                .proofs
                .iter()
                .any(|proof| proof.check_current().is_err())
            || self
                .source_proofs
                .iter()
                .any(|proof| proof.check_current().is_err())
        {
            return Err(refusal(409, stale_message));
        }
        self.record.save(&tx).map_err(preflight_failed)?;
        tx.commit().map_err(preflight_failed)?;
        Ok(reply(200, Value::Array(self.record.findings)))
    }
}

impl Store {
    pub fn prepare_preflight(&self, matter: &str) -> Result<PreflightWork, Reply> {
        PreflightWork::capture(self, matter, None)
    }

    pub fn publish_preflight(&mut self, computed: MeasuredPreflight) -> Result<Reply, Reply> {
        computed.publish(
            self,
            "Saved inputs changed during Preflight; run validation again",
        )
    }

    pub(super) fn publish_build_preflight(
        &mut self,
        computed: MeasuredPreflight,
    ) -> Result<(), Reply> {
        computed
            .publish(self, super::bundle_writer::BUILD_INPUT_CHANGED)
            .map(|_| ())
    }

    pub(super) fn run_preflight(&mut self, matter: &str) -> Result<Reply, Reply> {
        let measured = self.prepare_preflight(matter)?.measure()?;
        self.publish_preflight(measured)
    }
}

fn run_record(con: &Connection, id: &str, run: &str) -> rusqlite::Result<Option<Value>> {
    con.query_row(
        "SELECT body FROM preflight_runs WHERE id=? AND matter_id=?",
        params![run, id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|raw| decode(&raw))
    .transpose()
}

fn review(
    con: &Connection,
    root: &Path,
    key: Option<&DataKey>,
    id: &str,
    body: Option<&Value>,
) -> rusqlite::Result<Reply> {
    let inputs = match Inputs::read(con, root, key, id)? {
        Ok(inputs) => inputs,
        Err(refusal) => return Ok(refusal),
    };
    let snapshot = inputs.effective_digest(con)?;
    let stored: Option<String> = con
        .query_row(
            "SELECT body FROM preflight_reviews WHERE matter_id=? AND input_digest=?",
            params![id, snapshot],
            |row| row.get(0),
        )
        .optional()?;
    let current = match stored {
        Some(record) => decode(&record)?,
        None => json!({"input_digest":snapshot,"version":0,
            "items":{"latest_order":false,"sequence":false,"titles":false,"pagination":false},"warnings":[],"updated_at":null}),
    };
    let Some(body) = body else {
        return Ok(reply(200, current));
    };
    if body["input_digest"] != snapshot || body["version"] != current["version"] {
        return Ok(refusal(
            409,
            "Saved inputs or review changed; run checks again before reviewing",
        ));
    }
    if !body["items"].as_object().is_some_and(|items| {
        items.len() == REVIEW_ITEMS.len()
            && REVIEW_ITEMS
                .iter()
                .all(|&key| items.get(key).is_some_and(Value::is_boolean))
    }) {
        return Ok(refusal(
            409,
            "Supply all four human-review items as booleans",
        ));
    }
    let run = run_record(con, id, s(body, "run_id"))?;
    let Some(run) = run.filter(|record| record["contract"]["input_digest"] == snapshot) else {
        return Ok(refusal(
            409,
            "Run checks on the current saved inputs before reviewing",
        ));
    };
    let allowed: HashSet<_> = run["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["severity"] == "warning")
        .map(|finding| s(finding, "finding_id"))
        .collect();
    let warnings = body["warnings"].as_array().and_then(|warnings| {
        warnings
            .iter()
            .map(|warning| warning.as_str().filter(|warning| allowed.contains(warning)))
            .collect::<Option<BTreeSet<_>>>()
    });
    let Some(warnings) = warnings else {
        return Ok(refusal(
            409,
            "Only warnings from this check run can be acknowledged; errors must be fixed",
        ));
    };
    let version = current["version"]
        .as_u64()
        .ok_or(rusqlite::Error::InvalidQuery)?;
    let changed = json!({"input_digest":snapshot,"version":version+1,"warnings":warnings,"items":body["items"],"updated_at":now()});
    con.execute("INSERT INTO preflight_reviews VALUES (?,?,?) ON CONFLICT(matter_id,input_digest) DO UPDATE SET body=excluded.body",
        params![id,snapshot,changed.to_string()])?;
    Ok(reply(200, changed))
}

fn finding_review(
    con: &Connection,
    id: &str,
    run: &str,
    finding: &str,
    body: Option<&Value>,
) -> rusqlite::Result<Reply> {
    let lost = |message| refusal(if body.is_some() { 409 } else { 404 }, message);
    if [run, finding].into_iter().any(|value| {
        value.is_empty()
            || value.len() > 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Ok(lost("Invalid finding identity"));
    }
    let saved_run = match run_record(con, id, run)? {
        Some(record) => Some(record),
        None => super::generated_exports::output_run(con, id, run)?,
    };
    let Some(run_record) = saved_run else {
        return Ok(lost("Finding run is unavailable"));
    };
    let Some(record) = run_record["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(
            run_record["checks"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|check| check["findings"].as_array().into_iter().flatten()),
        )
        .find(|item| item["finding_id"] == finding)
    else {
        return Ok(lost("Finding is not part of this run"));
    };
    let current = revisions::active_document(con, s(record, "document_id"))?;
    let applicable = current.is_some_and(|doc| {
        doc["matter_id"] == id
            && doc["revision"] == record["target_revision"]
            && (record["page_index"].is_null()
                || record["page_index"]
                    .as_u64()
                    .zip(doc["page_count"].as_u64())
                    .is_some_and(|(page, count)| page < count))
    });
    let note = if let Some(body) = body {
        if !applicable {
            return Ok(lost("Finding target changed; run checks again"));
        }
        if !body["note"]
            .as_str()
            .is_some_and(|note| note.chars().count() <= 2000)
            || !body["acknowledged"].is_boolean()
        {
            return Ok(lost(
                "Provide a review note up to 2000 characters and an acknowledgement",
            ));
        }
        let mut note = body.clone();
        note["updated_at"] = json!(now());
        con.execute("INSERT INTO preflight_notes VALUES (?,?,?) ON CONFLICT(run_id,finding_id) DO UPDATE SET body=excluded.body",
            params![run,finding,note.to_string()])?;
        note
    } else {
        con.query_row(
            "SELECT body FROM preflight_notes WHERE run_id=? AND finding_id=?",
            params![run, finding],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|raw| decode(&raw))
        .transpose()?
        .unwrap_or_else(|| json!({"note":"","acknowledged":false}))
    };
    Ok(reply(
        200,
        json!({"finding":record,"applicable":applicable,"review":note}),
    ))
}

impl Store {
    pub(super) fn preflight_request(
        &mut self,
        method: &str,
        route: &[&str],
        body: &Value,
    ) -> rusqlite::Result<Option<Reply>> {
        let ["matters", id, tail @ ..] = route else {
            return Ok(None);
        };
        if (method, tail) == ("GET", &["validate"][..]) {
            return Ok(Some(self.run_preflight(id).unwrap_or_else(|error| error)));
        }
        let mutate = match (method, tail) {
            ("PUT", ["preflight", "review"] | ["validation", "findings", _, _]) => true,
            ("GET", ["preflight", "review" | "history"] | ["validation", "findings", _, _]) => {
                false
            }
            _ => return Ok(None),
        };
        let tx = self.con.transaction_with_behavior(if mutate {
            TransactionBehavior::Immediate
        } else {
            TransactionBehavior::Deferred
        })?;
        if document_metadata::matter(&tx, id)?.is_none() {
            return Ok(Some(refusal(404, "Matter not found")));
        }
        let payload = (method == "PUT").then_some(body);
        let response = match tail {
            ["preflight", "review"] => review(&tx, &self.root, self.key.as_ref(), id, payload)?,
            ["validation", "findings", run, finding] => {
                finding_review(&tx, id, run, finding, payload)?
            }
            _ => {
                let mut stmt = tx.prepare("SELECT body FROM preflight_runs WHERE matter_id=? ORDER BY json_extract(body,'$.created_at') DESC")?;
                let rows = stmt.query_map([id], |row| row.get::<_, String>(0))?;
                let body = rows
                    .map(|row| decode(&row?))
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                reply(200, json!(body))
            }
        };
        tx.commit()?;
        Ok(Some(response))
    }
}
