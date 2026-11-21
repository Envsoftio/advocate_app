//! Isolated Rust matter stores. Legacy database ownership stays with Python.
use crate::{
    admission,
    vault::{self, DataKey},
};
use crate::{reply, unsupported, Reply};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

mod annotations;
mod artifacts;
mod bundle_inputs;
mod bundle_writer;
mod compositions;
mod compression;
mod document_metadata;
mod document_output;
mod document_verification;
mod export_comparison;
mod file_ingest;
mod generated_exports;
mod image_import;
pub(crate) mod image_uses;
mod labels;
mod legacy_schema;
pub(crate) mod legacy_snapshot;
mod metadata_review;
mod ocr;
mod office_drafts;
mod ordering;
mod output_navigation;
mod page_tools;
mod portable_matter;
pub(crate) use portable_matter::recover_prepared_restores;
pub use portable_matter::{
    GeneratedPortableBackup, PortableBackupWork, PortableRestoreWork, PreparedPortableRestore,
    MAX_PACKAGE_BYTES as PORTABLE_PACKAGE_MAX_BYTES,
};
mod preflight;
mod preparation;
mod preserved_labels;
mod preview;
mod processing_jobs;
mod reading;
mod redaction_clip;
mod redaction_inline;
mod redaction_metadata;
mod redaction_paint;
mod redaction_records;
mod redaction_text;
mod redaction_vectors;
mod revisions;
mod shaped_text;
mod tesseract;
mod text_edit;

pub fn set_ocr_resource_root(root: PathBuf, required: bool) {
    tesseract::set_resource_root(root, required);
}

pub fn set_pdf_resource_root(root: PathBuf) {
    image_import::set_resource_root(root);
}
pub use annotations::{
    annotation_asset_id, annotation_image_document, AnnotationAssetWork, AnnotationImageWork,
    MeasuredAnnotationImage,
};
pub use artifacts::{
    original_info_document, Artifact, OriginalInfoWork, PdfSnapshot, READ_CHUNK_BYTES,
};
pub use bundle_inputs::{BundlePreviewWork, MeasuredBundlePreview};
pub use bundle_writer::{MeasuredOutputPreview, OutputPreviewTarget, OutputPreviewWork};
pub use compositions::{
    CompositionCatalogKind, CompositionCatalogWork, CompositionDraftTarget, CompositionDraftWork,
    MeasuredCompositionDraft, MeasuredVariantReview, VariantReviewWork, VerifiedCompositionCatalog,
};
pub use document_output::{DocumentOutputTarget, DocumentOutputWork, PdfSnapshotWork};
pub use export_comparison::{ExportComparisonWork, MeasuredExportComparison};
pub use file_ingest::{
    cover_import_matter, uploaded_replacement_document, CoverImportWork, DocumentUploadBatch,
    DocumentUploadTarget, DocumentUploadWork, MeasuredWorkingReplacement, RawDocumentUpload,
    RawReplacementUpload, UploadedReplacement, WorkingReplacement, DOCUMENT_IMPORT_MAX_BYTES,
};
pub use generated_exports::{
    ExportOutputTarget, ExportOutputWork, ExportSnapshotWork, MeasuredOutputValidation,
    OutputValidationTarget, OutputValidationWork,
};
pub use labels::{LabelReviewTarget, LabelReviewWork, MeasuredLabelReview};
pub use metadata_review::{MeasuredMetadataReview, MetadataReviewTarget, MetadataReviewWork};
pub use ocr::{OcrReviewMeasurement, OcrReviewWork};
pub use page_tools::{PageOutput, PageOutputTarget, PageOutputWork};
pub use preflight::{MeasuredPreflight, PreflightWork};
pub use preview::{PagePreviewTarget, PagePreviewWork};
pub use processing_jobs::{
    FinishedWork, PendingWork, PreparedProcessingSubmission, ProcessingSubmission,
    ScreenSubmission, VerifiedScreenSubmission,
};
pub use reading::{SearchTarget, SearchWork};
pub use redaction_records::{MeasuredRedaction, RedactionTarget, RedactionWork};
pub use revisions::{MeasuredRevisionEdit, RevisionEditWork, RevisionTarget};
pub use text_edit::{MeasuredTextPage, TextPageTarget, TextPageWork};

pub struct Store {
    con: Connection,
    root: PathBuf,
    key: Option<DataKey>,
    active_jobs: processing_jobs::ActiveJobs,
    processing_session: String,
    processing_stopped: bool,
}

/// A worker can read committed rows without taking ownership of store writes.
struct StoreReader {
    path: PathBuf,
    key: Option<DataKey>,
}

impl StoreReader {
    fn capture(store: &Store) -> rusqlite::Result<Self> {
        let path = store
            .con
            .path()
            .filter(|path| !path.is_empty())
            .ok_or(rusqlite::Error::InvalidPath(store.root.clone()))?;
        Ok(Self {
            path: PathBuf::from(path),
            key: store.key.clone(),
        })
    }

    fn open(&self) -> Result<Connection, crate::secure_db::DatabaseError> {
        if let Some(key) = &self.key {
            crate::secure_db::open_protected_read_only(&self.path, key)
        } else {
            Ok(Connection::open_with_flags(
                &self.path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?)
        }
    }
}

#[derive(Default)]
pub struct ImportOptions<'a> {
    pub password: Option<&'a str>,
    pub acknowledge_signature_risk: bool,
    pub included: bool,
    pub source_file_mtime: &'a str,
}
#[derive(Clone)]
pub struct DocumentImportTarget {
    root: PathBuf,
    matter: String,
    protected: bool,
}

pub struct PreparedDocumentImport<'a> {
    target: DocumentImportTarget,
    filename: String,
    original: Cow<'a, [u8]>,
    working: Option<Vec<u8>>,
    hash: String,
    working_hash: String,
    pages: usize,
    signature: Option<admission::SignatureInspection>,
    ocr_status: &'static str,
    ocr_provenance: &'static str,
    included: bool,
    source_file_mtime: String,
    cover_inputs: Option<Value>,
}

impl DocumentImportTarget {
    pub fn measure(
        self,
        filename: String,
        bytes: Vec<u8>,
        options: ImportOptions<'_>,
    ) -> Result<PreparedDocumentImport<'static>, Reply> {
        self.measure_bytes(filename, Cow::Owned(bytes), options, None)
    }

    fn measure_bytes<'a>(
        self,
        filename: String,
        bytes: Cow<'a, [u8]>,
        options: ImportOptions<'_>,
        cover_inputs: Option<Value>,
    ) -> Result<PreparedDocumentImport<'a>, Reply> {
        let generated_cover = cover_inputs.is_some();
        let filename_length_valid = if self.protected {
            filename.chars().count() <= 500
        } else {
            filename.len() <= 255
        };
        if !filename_length_valid
            || filename.is_empty()
            || filename.contains(['/', '\\', '\0'])
            || filename == "."
            || filename == ".."
        {
            return Err(reply(
                422,
                json!({"detail":if self.protected { "Invalid document filename" } else { "A plain PDF filename is required" }}),
            ));
        }
        let (stem, suffix) = filename.rsplit_once('.').unwrap_or(("", ""));
        let suffix = suffix.to_ascii_lowercase();
        if (!self.protected && suffix != "pdf")
            || !["pdf", "png", "jpg", "jpeg"].contains(&suffix.as_str())
        {
            return Err(reply(
                415,
                json!({"detail":"Only PDF, PNG and JPEG files are supported"}),
            ));
        }
        if stem.trim_end_matches([' ', '.']).is_empty() {
            return Err(reply(422, json!({"detail":"PDF filename needs a title"})));
        }
        if !self.protected
            && (bytes.is_empty() || bytes.len() > 4 * 1024 * 1024 || !bytes.starts_with(b"%PDF-"))
        {
            return Err(reply(
                422,
                json!({"detail":"Invalid or oversized PDF; 4 MiB maximum"}),
            ));
        }
        let mut signature = None;
        let (pages, working, ocr_status, ocr_provenance) = if generated_cover {
            signature = Some(admission::SignatureInspection {
                certificate_signature_status: "not_detected",
                certificate_signature_count: 0,
                signature_inspection_note: "Generated locally without a PDF certificate signature."
                    .into(),
                visible_signature_status: "absent",
                signature_status: "unsigned",
            });
            (1, None, "native_text", "[]")
        } else if self.protected && suffix != "pdf" {
            if bytes.len() > admission::FILE_LIMIT_BYTES {
                return Err(reply(
                    413,
                    json!({"detail":"Image exceeds the import limit"}),
                ));
            }
            let working = match document_output::normalize_image(&bytes) {
                Ok(pdf) => pdf,
                Err(error) => return Err(reply(422, json!({"detail":error}))),
            };
            signature = Some(admission::SignatureInspection {
                certificate_signature_status: "not_detected",
                certificate_signature_count: 0,
                signature_inspection_note:
                    "Source image has no PDF certificate container. Visible marks require review."
                        .into(),
                visible_signature_status: "unreviewed",
                signature_status: "no_certificate_signature_detected",
            });
            (1, Some(working), "needs_ocr", "[]")
        } else if self.protected {
            match admission::prepare_pdf(
                &bytes,
                options.password,
                options.acknowledge_signature_risk,
            ) {
                Ok(prepared) => {
                    signature = Some(prepared.signature);
                    (
                        prepared.page_count,
                        match prepared.working_pdf {
                            Cow::Borrowed(_) => None,
                            Cow::Owned(pdf) => Some(pdf),
                        },
                        if prepared.needs_ocr {
                            "needs_ocr"
                        } else {
                            "not_requested"
                        },
                        "[]",
                    )
                }
                Err(error) => return Err(reply(error.status, json!({"detail":error.detail}))),
            }
        } else {
            // The established plaintext fixture gate stays ahead of parsing or writing.
            if bytes.len() != PILOT_PDF_BYTES.len() || bytes.as_ref() != PILOT_PDF_BYTES {
                return Err(reply(
                    422,
                    json!({"detail":"Rust PDF import is limited to the exact samples/01_petition.pdf synthetic fixture; other PDFs are refused until signature and recovery qualification"}),
                ));
            }
            let pdf = pdf_oxide::PdfDocument::from_bytes(bytes.to_vec())
                .map_err(|_| reply(500, json!({"detail":"Invalid repository PDF fixture"})))?;
            let pages = pdf
                .page_count()
                .map_err(|_| reply(500, json!({"detail":"Invalid repository PDF page count"})))?;
            (pages, None, "not_started", "{}")
        };
        let hash = sha256_hex(&bytes);
        let working_hash = working.as_deref().map_or_else(|| hash.clone(), sha256_hex);
        Ok(PreparedDocumentImport {
            target: self,
            filename,
            original: bytes,
            working,
            hash,
            working_hash,
            pages,
            signature,
            ocr_status,
            ocr_provenance,
            included: options.included,
            source_file_mtime: options.source_file_mtime.to_owned(),
            cover_inputs,
        })
    }
}

const SCHEMA_VERSION: i64 = 14;
const MARKER: &str = "MatterDraft Rust synthetic store v1\n";
// Admission is deliberately limited to this reviewed, repository-owned synthetic fixture.
// A hash match checks exact bytes; it does not verify a signer or make a general PDF safe.
const PILOT_PDF_SHA256: &str = "76da5e34c1799a5aff3d3e9bec8cc689cb6cf8430dfc163f93bfc3ec286bf17a";
const PILOT_PDF_BYTES: &[u8] = include_bytes!("../../samples/01_petition.pdf");

#[cfg(test)]
fn crash_at(phase: &str) {
    if std::env::var("MATTERDRAFT_TEST_HOLD_PHASE").as_deref() == Ok(phase) {
        if let Ok(path) = std::env::var("MATTERDRAFT_TEST_READY_FILE") {
            std::fs::write(path, phase).unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(600));
    }
    if std::env::var("MATTERDRAFT_TEST_CRASH_PHASE").as_deref() == Ok(phase) {
        std::process::exit(97);
    }
}
#[cfg(not(test))]
fn crash_at(_: &str) {}
#[cfg(test)]
fn fail_at(phase: &str) -> Result<(), String> {
    if std::env::var("MATTERDRAFT_TEST_FAIL_PHASE").as_deref() == Ok(phase) {
        return Err(format!("Injected import file failure at {phase}"));
    }
    Ok(())
}
#[cfg(not(test))]
fn fail_at(_: &str) -> Result<(), String> {
    Ok(())
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false)
}
fn sha256_hex(bytes: &[u8]) -> String {
    vault::hex_bytes(&Sha256::digest(bytes)).to_string()
}

fn open_document_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options
        .open(path)
        .map_err(|e| format!("Document file cannot be opened: {e}"))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Document resource is not a regular file".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err("Document resource is a reparse point".into());
        }
    }
    Ok(file)
}

fn read_pilot_file(path: &Path) -> Result<Vec<u8>, String> {
    read_open_pilot_file(&open_document_file(path)?)
}

fn read_open_pilot_file(file: &File) -> Result<Vec<u8>, String> {
    if file.metadata().map_err(|e| e.to_string())?.len() != PILOT_PDF_BYTES.len() as u64 {
        return Err("Document file is not the pinned fixture size".into());
    }
    let mut bytes = Vec::with_capacity(PILOT_PDF_BYTES.len() + 1);
    file.take(PILOT_PDF_BYTES.len() as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() != PILOT_PDF_BYTES.len() {
        return Err("Document file changed during verification".into());
    }
    Ok(bytes)
}

fn document_file_hash(path: &Path, key: Option<&DataKey>) -> Result<String, String> {
    let Some(key) = key else {
        return Ok(sha256_hex(&read_pilot_file(path)?));
    };
    let mut hash = Sha256::new();
    vault::visit_encrypted(open_document_file(path)?, key, |contents| {
        hash.update(contents)
    })
    .map_err(|error| error.to_string())?;
    Ok(vault::hex_bytes(&hash.finalize()).to_string())
}

fn publication_receipt(key: &DataKey, id: &str) -> Result<String, String> {
    vault::publication_tag(key, id)
        .map(|tag| format!(".receipt-{tag}"))
        .map_err(|error| error.to_string())
}
impl Store {
    pub fn open(root: &Path) -> Result<Self, String> {
        if root.is_symlink() {
            return Err("Symlink store refused".into());
        }
        if root.exists() {
            let marker = std::fs::read_to_string(root.join("RUST_SYNTHETIC_STORE"));
            if marker.as_deref().ok() != Some(MARKER) {
                return Err(
                    "Unrecognized store; Python/protected stores must never be opened".into(),
                );
            }
        } else {
            std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
                    .map_err(|e| e.to_string())?;
            }
            std::fs::write(root.join("RUST_SYNTHETIC_STORE"), MARKER).map_err(|e| e.to_string())?;
        }
        for entry in std::fs::read_dir(root).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let name = path.file_name().unwrap().to_string_lossy();
            if path.is_symlink()
                || ![
                    "RUST_SYNTHETIC_STORE",
                    "rust-matters.sqlite3",
                    "rust-matters.sqlite3-journal",
                    "rust-matters.sqlite3-wal",
                    "rust-matters.sqlite3-shm",
                    "documents-v1",
                ]
                .contains(&name.as_ref())
            {
                return Err("Foreign or protected store content refused".into());
            }
        }
        let con = Connection::open(root.join("rust-matters.sqlite3")).map_err(|e| e.to_string())?;
        Self::initialize(root, con, None)
    }

    pub(crate) fn open_keyed(root: &Path, con: Connection, key: DataKey) -> Result<Self, String> {
        Self::initialize(root, con, Some(key))
    }

    pub(crate) fn storage_key(&self) -> Option<&DataKey> {
        self.key.as_ref()
    }

    fn initialize(root: &Path, mut con: Connection, key: Option<DataKey>) -> Result<Self, String> {
        con.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        let tx = con
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let version: i64 = tx
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if version > SCHEMA_VERSION || (version == 3 && key.is_none()) {
            return Err("Unsupported Rust schema version".into());
        }
        if version == 0 {
            tx.execute_batch("CREATE TABLE matters (id TEXT PRIMARY KEY, body TEXT NOT NULL, updated_at TEXT NOT NULL); CREATE TABLE audit_events (id INTEGER PRIMARY KEY, matter_id TEXT NOT NULL, action TEXT NOT NULL, created_at TEXT NOT NULL); CREATE TABLE edit_history (id INTEGER PRIMARY KEY, matter_id TEXT NOT NULL, kind TEXT NOT NULL, before_json TEXT NOT NULL, after_json TEXT NOT NULL, created_at TEXT NOT NULL, applied INTEGER NOT NULL DEFAULT 1); PRAGMA user_version=1;").map_err(|e| e.to_string())?;
        }
        if version < 2 {
            tx.execute_batch("ALTER TABLE audit_events ADD COLUMN document_id TEXT; CREATE TABLE documents (id TEXT PRIMARY KEY, matter_id TEXT NOT NULL, body TEXT NOT NULL, source_sha256 TEXT NOT NULL, original_relpath TEXT NOT NULL UNIQUE, working_relpath TEXT NOT NULL UNIQUE, created_at TEXT NOT NULL, archived_at TEXT); CREATE INDEX documents_matter_order ON documents(matter_id, archived_at, created_at, id); CREATE UNIQUE INDEX documents_matter_source ON documents(matter_id, source_sha256); CREATE TABLE document_revisions (document_id TEXT NOT NULL, revision INTEGER NOT NULL, relpath TEXT NOT NULL UNIQUE, sha256 TEXT NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY(document_id,revision)); PRAGMA user_version=2;").map_err(|e| e.to_string())?;
        }
        if key.is_some() && version < 3 {
            revisions::migrate(&tx).map_err(|error| error.to_string())?;
        }
        if version < 4 {
            preparation::migrate(&tx).map_err(|error| error.to_string())?;
        }
        if version < 5 {
            reading::migrate(&tx).map_err(|error| error.to_string())?;
        }
        if version < 6 {
            bundle_inputs::migrate(&tx).map_err(|error| error.to_string())?;
        }
        if version < 7 {
            preflight::migrate(&tx).map_err(|error| error.to_string())?;
        }
        if version < 8 {
            annotations::migrate(&tx).map_err(|error| error.to_string())?;
        }
        if version < 9 {
            processing_jobs::migrate(&tx).map_err(|error| error.to_string())?;
        }
        if version < 10 {
            generated_exports::migrate(&tx).map_err(|error| error.to_string())?;
        }
        if version < 11 {
            ocr::migrate(&tx).map_err(|error| error.to_string())?;
        }
        if version < 12 {
            compositions::migrate(&tx).map_err(|error| error.to_string())?;
        }
        if version < 13 {
            redaction_records::migrate(&tx).map_err(|error| error.to_string())?;
        }
        if version < 14 {
            processing_jobs::migrate_frozen_builds(&tx).map_err(|error| error.to_string())?;
        }
        processing_jobs::recover(&tx).map_err(|error| error.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        let mut store = Self {
            con,
            root: root.to_path_buf(),
            key,
            active_jobs: Default::default(),
            processing_session: uuid::Uuid::new_v4().simple().to_string(),
            processing_stopped: false,
        };
        store.reconcile_documents()?;
        Ok(store)
    }
    pub fn request(&mut self, method: &str, path: &str, body: Value) -> Reply {
        self.request_with_order(method, path, body, &crate::RequestOrder::default())
    }

    pub fn request_with_order(
        &mut self,
        method: &str,
        path: &str,
        body: Value,
        order: &crate::RequestOrder,
    ) -> Reply {
        match self.handle(method, path, body, order) {
            Ok(r) => r,
            Err(_) => reply(
                500,
                json!({"detail":"Rust matter transaction failed; no changes committed"}),
            ),
        }
    }
    fn handle(
        &mut self,
        method: &str,
        path: &str,
        body: Value,
        order: &crate::RequestOrder,
    ) -> rusqlite::Result<Reply> {
        let (head, query) = path.split_once('?').unwrap_or((path, ""));
        let route: Vec<_> = head.trim_start_matches('/').split('/').collect();
        if let Some(response) = self.portable_request(method, &route, &body, None) {
            return Ok(response);
        }
        if let Some(response) = self.ingest_request(method, &route, query, &body)? {
            return Ok(response);
        }
        if let Some(response) = self.generated_export_request(method, &route, query, &body)? {
            return Ok(response);
        }
        if let Some(response) = self.output_preview_request(method, &route, &body) {
            return Ok(response);
        }
        if let Some(response) = self.composition_request(method, &route, &body) {
            return Ok(response);
        }
        if let Some(response) = self.processing_request(method, &route, &body)? {
            return Ok(response);
        }
        if let Some(response) = self.resource_request(method, path)? {
            return Ok(response);
        }
        if let Some(response) = reading::route(self, method, &route, query, &body) {
            return Ok(response);
        }
        if let Some(response) = preview::route(self, method, &route, query) {
            return Ok(response);
        }
        if let Some(response) = self.redaction_request(method, &route, &body, order)? {
            return Ok(response);
        }
        if let Some(response) = self.text_request(method, &route, &body)? {
            return Ok(response);
        }
        if let Some(response) = self.ocr_review_request(method, &route, query, &body) {
            return Ok(response);
        }
        if let Some(target) = PageOutputTarget::select(method, &route, query) {
            return Ok(target
                .and_then(|target| {
                    self.page_output(
                        &target.document,
                        target.output,
                        &target.selection,
                        target.acknowledged,
                    )
                    .map(|bytes| PageOutputTarget::into_reply(target.output, bytes))
                })
                .unwrap_or_else(|error| error));
        }
        if let Some(response) = self.document_output_request(method, &route, query, &body) {
            return Ok(response);
        }
        if let Some(response) = self.annotation_request(method, &route, &body, order)? {
            return Ok(response);
        }
        let route: Vec<_> = path.trim_start_matches('/').split('/').collect();
        if let Some(id) = original_info_document(method, path) {
            return Ok(self
                .prepare_original_info(id)
                .and_then(OriginalInfoWork::compute)
                .unwrap_or_else(|refusal| refusal));
        }
        if let Some(response) = self.preflight_request(method, &route, &body)? {
            return Ok(response);
        }
        if let Some(response) = self.bundle_input_request(method, &route, &body)? {
            return Ok(response);
        }
        if let Some(response) = self.label_request(method, &route, &body)? {
            return Ok(response);
        }
        if let Some(response) = self.metadata_review_request(method, &route, &body, order)? {
            return Ok(response);
        }
        if let Some(response) = self.metadata_request(method, &route, &body, order)? {
            return Ok(response);
        }
        if let Some(response) = preparation::request(&mut self.con, method, &route, &body)? {
            return Ok(response);
        }
        if let ("POST", ["matters", id, "documents", "reorder"]) = (method, route.as_slice()) {
            return self.reorder_documents(id, &body);
        }
        if self.key.is_some() {
            if let Some(response) = self.revision_request(method, &route, &body, order)? {
                return Ok(response);
            }
        }
        if let ("DELETE", ["documents", doc_id]) = (method, route.as_slice()) {
            return self.document_transition(None, doc_id, false);
        }
        if let ("POST", ["matters", matter_id, "archived-documents", doc_id, "restore"]) =
            (method, route.as_slice())
        {
            return self.document_transition(Some(matter_id), doc_id, true);
        }
        if ["/matters", "/matters/archived"].contains(&path) && method == "GET" {
            let mut stmt = self
                .con
                .prepare(if path == "/matters/archived" {
                    "SELECT body FROM matters WHERE json_extract(body,'$.archived_at') IS NOT NULL ORDER BY json_extract(body,'$.archived_at') DESC"
                } else {
                    "SELECT body FROM matters WHERE json_extract(body,'$.archived_at') IS NULL ORDER BY updated_at DESC"
                })?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            return Ok(reply(
                200,
                Value::Array(
                    rows.iter()
                        .map(|s| decode(s))
                        .collect::<rusqlite::Result<Vec<_>>>()?,
                ),
            ));
        }
        let segments = route;
        if segments.first() == Some(&"matters") && segments.get(1).is_some_and(|s| !s.is_empty()) {
            let id = segments[1];
            let operation = match (method, &segments[2..]) {
                ("DELETE", []) => Some("archive"),
                ("POST", ["restore"]) => Some("restore"),
                ("GET", ["history"]) => Some("history"),
                ("POST", ["history", direction]) => Some(if ["undo", "redo"].contains(direction) {
                    direction
                } else {
                    "unknown"
                }),
                _ => None,
            };
            if let Some(operation) = operation {
                return self.transition(id, operation);
            }
        }
        let id = path
            .strip_prefix("/matters/")
            .filter(|s| !s.contains('/') && !s.is_empty());
        let create = path == "/matters" && method == "POST";
        if !create && !(id.is_some() && ["GET", "PATCH"].contains(&method)) {
            return Ok(unsupported());
        }
        let fields = if method != "GET" {
            match validate_with_order(body, create, &order.root) {
                Ok(v) => v,
                Err(e) => return Ok(e),
            }
        } else {
            json!({})
        };
        let tx = self
            .con
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut item = if create {
            fields.clone()
        } else {
            let raw: Option<String> = tx
                .query_row("SELECT body FROM matters WHERE id=?", [id.unwrap()], |r| {
                    r.get(0)
                })
                .optional()?;
            match raw {
                Some(s) => {
                    let item = decode(&s)?;
                    if !item["archived_at"].is_null() {
                        return Ok(reply(404, json!({"detail":"Matter not found"})));
                    }
                    item
                }
                None => return Ok(reply(404, json!({"detail":"Matter not found"}))),
            }
        };
        if method == "GET" {
            return Ok(reply(200, item));
        }
        // Route-level checks run after missing-ID detection, as in FastAPI.
        if let Some(profile) = fields.get("court_profile") {
            let profiles: Value =
                serde_json::from_str(include_str!("../../backend/app/rules/profiles.json"))
                    .unwrap();
            if !profiles
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["id"] == *profile)
            {
                return Ok(reply(422, json!({"detail":"Unknown court profile"})));
            }
        }
        if fields.as_object().unwrap().values().any(Value::is_null) {
            return Ok(reply(
                422,
                json!({"detail":"Do not set matter fields to null"}),
            ));
        }
        let stamp = now();
        if create {
            item["id"] = json!(uuid::Uuid::new_v4().simple().to_string());
            item["created_at"] = json!(stamp);
            item["updated_at"] = json!(stamp);
            item["archived_at"] = Value::Null;
            tx.execute(
                "INSERT INTO matters VALUES (?,?,?)",
                params![item["id"].as_str(), item.to_string(), stamp],
            )?;
        } else {
            let mut before = json!({});
            for (key, value) in fields.as_object().unwrap() {
                before[key] = item[key].clone();
                item[key] = value.clone();
            }
            if before == fields {
                return Ok(reply(200, item));
            }
            item["updated_at"] = json!(stamp);
            tx.execute(
                "UPDATE matters SET body=?,updated_at=? WHERE id=?",
                params![item.to_string(), stamp, id],
            )?;
            let mut tracked = fields.clone();
            if tracked.get("keep_groups_intact").is_some() {
                before["keep_groups_intact"] =
                    json!(i64::from(before["keep_groups_intact"].as_bool().unwrap()));
                tracked["keep_groups_intact"] =
                    json!(i64::from(tracked["keep_groups_intact"].as_bool().unwrap()));
            }
            record_edit(&tx, id.unwrap(), "matter", &before, &tracked, &stamp)?;
        }
        tx.execute(
            "INSERT INTO audit_events (matter_id,action,created_at) VALUES (?,?,?)",
            params![
                item["id"].as_str(),
                if create {
                    "matter_created"
                } else {
                    "matter_updated"
                },
                stamp
            ],
        )?;
        tx.commit()?;
        Ok(reply(if create { 201 } else { 200 }, item))
    }
    fn transition(&mut self, id: &str, operation: &str) -> rusqlite::Result<Reply> {
        let tx = self
            .con
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let raw: Option<String> = tx
            .query_row("SELECT body FROM matters WHERE id=?", [id], |r| r.get(0))
            .optional()?;
        let missing = || {
            reply(
                404,
                json!({"detail": if operation == "restore" { "Archived matter not found" } else { "Matter not found" }}),
            )
        };
        let Some(raw) = raw else {
            return Ok(missing());
        };
        let mut item = decode(&raw)?;
        if operation != "restore" && !item["archived_at"].is_null() {
            return Ok(missing());
        }
        if operation == "history" {
            return Ok(reply(200, history_state(&tx, id, self.key.is_some())?));
        }
        if !["archive", "restore", "undo", "redo"].contains(&operation) {
            return Ok(reply(404, json!({"detail":"Unknown history operation"})));
        }
        // Python restore is idempotent for an already-active matter.
        if operation == "restore" && item["archived_at"].is_null() {
            return Ok(reply(200, item));
        }
        let action;
        let mut presence_replay = false;
        if operation == "archive" {
            item["archived_at"] = json!(now());
            action = "matter_archived";
        } else if operation == "restore" {
            item["archived_at"] = Value::Null;
            action = "matter_restored";
        } else {
            let redo = operation == "redo";
            let sql = format!("SELECT id,kind,before_json,after_json FROM edit_history WHERE matter_id=? AND applied=? ORDER BY id {} LIMIT 1", if redo { "ASC" } else { "DESC" });
            let entry: Option<(i64, String, String, String)> = tx
                .query_row(&sql, params![id, i64::from(!redo)], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .optional()?;
            let Some((entry_id, kind, before, after)) = entry else {
                return Ok(reply(
                    409,
                    json!({"detail":format!("No edit to {operation}")}),
                ));
            };
            if kind == "document_presence" {
                presence_replay = true;
                let target = decode(if redo { &after } else { &before })?;
                let expected = decode(if redo { &before } else { &after })?;
                if let Some(refusal) = Self::replay_document_presence(
                    &tx,
                    &self.root,
                    self.key.as_ref(),
                    id,
                    &target,
                    &expected,
                )? {
                    return Ok(refusal);
                }
            } else if kind == "document_batch"
                || (kind == "document"
                    && decode(&after)?["fields"].get("working_relpath").is_none())
            {
                let target = decode(if redo { &after } else { &before })?;
                let expected = decode(if redo { &before } else { &after })?;
                if let Some(refusal) =
                    document_metadata::replay(&tx, id, &kind, &target, &expected)?
                {
                    return Ok(refusal);
                }
            } else if kind == "document" && self.key.is_some() {
                let target = decode(if redo { &after } else { &before })?;
                let expected = decode(if redo { &before } else { &after })?;
                if let Some(refusal) = revisions::replay(
                    &tx,
                    &self.root,
                    self.key.as_ref().unwrap(),
                    id,
                    &target,
                    &expected,
                )? {
                    return Ok(refusal);
                }
            } else if ["output_marks", "output_preset"].contains(&kind.as_str()) {
                let target = decode(if redo { &after } else { &before })?;
                let expected = decode(if redo { &before } else { &after })?;
                if let Some(refusal) =
                    bundle_inputs::replay_settings(&tx, id, &mut item, &kind, &target, &expected)?
                {
                    return Ok(refusal);
                }
            } else if kind == "page_annotations" {
                let target = decode(if redo { &after } else { &before })?;
                let expected = decode(if redo { &before } else { &after })?;
                if let Some(refusal) = annotations::replay(&tx, id, &target, &expected)? {
                    return Ok(refusal);
                }
            } else if kind == "bundle_membership" || bundle_inputs::history_kind(&kind) {
                let target = decode(if redo { &after } else { &before })?;
                let expected = decode(if redo { &before } else { &after })?;
                if let Some(refusal) = bundle_inputs::replay(
                    &tx,
                    &self.root,
                    self.key.as_ref(),
                    id,
                    &kind,
                    &target,
                    &expected,
                )? {
                    return Ok(refusal);
                }
            } else if kind == "order" {
                let target = decode(if redo { &after } else { &before })?;
                let expected = decode(if redo { &before } else { &after })?;
                if let Some(refusal) =
                    ordering::replay(&tx, &self.root, self.key.as_ref(), id, &target, &expected)?
                {
                    return Ok(refusal);
                }
            } else if preparation::history_supported(&kind) {
                let target = decode(if redo { &after } else { &before })?;
                let expected = decode(if redo { &before } else { &after })?;
                if !preparation::replay(&tx, id, &kind, &mut item, &target, &expected)? {
                    return Ok(reply(
                        409,
                        json!({"detail":"Unsupported or stale matter preparation history entry"}),
                    ));
                }
            } else if kind == "matter" {
                let state = decode(if redo { &after } else { &before })?;
                let fields = match validate(state, false) {
                    Ok(v)
                        if v.as_object()
                            .is_some_and(|o| o.values().all(|v| !v.is_null())) =>
                    {
                        v
                    }
                    _ => {
                        return Ok(reply(
                            409,
                            json!({"detail":"Unsupported matter metadata history entry"}),
                        ))
                    }
                };
                for (key, value) in fields.as_object().unwrap() {
                    item[key] = value.clone();
                }
            } else {
                return Ok(reply(
                    409,
                    json!({"detail":"Unsupported edit history entry"}),
                ));
            }
            tx.execute(
                "UPDATE edit_history SET applied=? WHERE id=?",
                params![i64::from(redo), entry_id],
            )?;
            action = if redo { "edit_redone" } else { "edit_undone" };
        }
        let stamp = now();
        item["updated_at"] = json!(stamp);
        tx.execute(
            "UPDATE matters SET body=?,updated_at=? WHERE id=?",
            params![item.to_string(), stamp, id],
        )?;
        tx.execute(
            "INSERT INTO audit_events (matter_id,action,created_at) VALUES (?,?,?)",
            params![id, action, now()],
        )?;
        let body = match operation {
            "archive" => {
                json!({"status":"archived","note":"Matter, records and original files retained locally"})
            }
            "restore" => item,
            _ => history_state(&tx, id, self.key.is_some())?,
        };
        if presence_replay {
            crash_at("presence_replay_publication");
        }
        tx.commit()?;
        if presence_replay {
            crash_at("presence_replay_committed");
        }
        Ok(reply(200, body))
    }
}

impl Store {
    fn replay_document_presence(
        con: &Connection,
        root: &Path,
        key: Option<&DataKey>,
        matter_id: &str,
        target: &Value,
        expected: &Value,
    ) -> rusqlite::Result<Option<Reply>> {
        let valid = |state: &Value| {
            state.as_object().is_some_and(|map| {
                map.len() == 3
                    && map.get("id").and_then(Value::as_str).is_some()
                    && map
                        .get("position")
                        .and_then(Value::as_i64)
                        .is_some_and(|p| p >= 0)
                    && map
                        .get("archived_at")
                        .is_some_and(|v| v.is_null() || v.as_str().is_some())
            })
        };
        if !valid(target) || !valid(expected) || target["id"] != expected["id"] {
            return Ok(Some(reply(
                409,
                json!({"detail":"Unsupported document presence history entry"}),
            )));
        }
        let doc_id = target["id"].as_str().unwrap();
        let row: Option<(String, String, String, Option<String>)> = con
            .query_row(
                "SELECT id,matter_id,body,archived_at FROM documents WHERE id=? AND matter_id=?",
                params![doc_id, matter_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((row_id, row_matter, raw, archived_at)) = row else {
            return Ok(Some(reply(
                409,
                json!({"detail":"Document is missing; this edit cannot be restored"}),
            )));
        };
        let mut doc = decode(&raw)?;
        if let Err(message) = Self::verify_document_at(
            con,
            root,
            key,
            &doc,
            &row_id,
            &row_matter,
            archived_at.as_deref(),
        ) {
            return Ok(Some(reply(409, json!({"detail":message}))));
        }
        if doc["position"] != expected["position"] || doc["archived_at"] != expected["archived_at"]
        {
            return Ok(Some(reply(
                409,
                json!({"detail":"Document state changed; this edit cannot be restored"}),
            )));
        }
        if target["archived_at"].is_null() {
            if let Some(mut refusal) = Self::group_parent_refusal(con, matter_id, doc_id, &doc)? {
                refusal.status = 409;
                return Ok(Some(refusal));
            }
        }
        let stamp = now();
        doc["archived_at"] = target["archived_at"].clone();
        doc["position"] = target["position"].clone();
        doc["updated_at"] = json!(stamp);
        let changed = con.execute("UPDATE documents SET body=?,archived_at=? WHERE id=? AND matter_id=? AND archived_at IS ?",
            params![doc.to_string(),target["archived_at"].as_str(),doc_id,matter_id,archived_at])?;
        if changed != 1 {
            return Ok(Some(reply(409, json!({"detail":"Document state changed"}))));
        }
        Ok(None)
    }

    fn document_transition(
        &mut self,
        requested_matter: Option<&str>,
        doc_id: &str,
        restore: bool,
    ) -> rusqlite::Result<Reply> {
        let tx = self
            .con
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(matter_id) = requested_matter {
            let active: Option<i64> = tx.query_row(
                "SELECT 1 FROM matters WHERE id=? AND json_extract(body,'$.archived_at') IS NULL",
                [matter_id], |row| row.get(0),
            ).optional()?;
            if active.is_none() {
                return Ok(reply(404, json!({"detail":"Matter not found"})));
            }
        }
        let row: Option<(String, String, String, Option<String>)> = tx
            .query_row(
                "SELECT id,matter_id,body,archived_at FROM documents WHERE id=?",
                [doc_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let missing = || {
            reply(
                404,
                json!({"detail":if restore {"Archived document not found"} else {"Document not found"}}),
            )
        };
        let Some((row_id, matter_id, raw, archived_at)) = row else {
            return Ok(missing());
        };
        if restore {
            if Some(matter_id.as_str()) != requested_matter || archived_at.is_none() {
                return Ok(missing());
            }
        } else {
            let active: Option<i64> = tx.query_row(
                "SELECT 1 FROM matters WHERE id=? AND json_extract(body,'$.archived_at') IS NULL",
                [&matter_id], |row| row.get(0),
            ).optional()?;
            if active.is_none() || archived_at.is_some() {
                return Ok(missing());
            }
        }
        let mut doc = decode(&raw)?;
        if let Err(message) = Self::verify_document_at(
            &tx,
            &self.root,
            self.key.as_ref(),
            &doc,
            &row_id,
            &matter_id,
            archived_at.as_deref(),
        ) {
            return Ok(reply(409, json!({"detail":message})));
        }
        let previous_position = doc["position"]
            .as_i64()
            .ok_or(rusqlite::Error::InvalidQuery)?;
        if restore {
            if let Some(refusal) = Self::group_parent_refusal(&tx, &matter_id, doc_id, &doc)? {
                return Ok(refusal);
            }
        }
        let position = if restore {
            tx.query_row("SELECT COALESCE(MAX(json_extract(body,'$.position')),-1)+1 FROM documents WHERE matter_id=? AND archived_at IS NULL", [&matter_id], |r| r.get::<_,i64>(0))?
        } else {
            previous_position
        };
        let stamp = now();
        let next_archive = if restore { Value::Null } else { json!(stamp) };
        let before = json!({"id":doc_id,"archived_at":archived_at,"position":previous_position});
        let after = json!({"id":doc_id,"archived_at":next_archive,"position":position});
        doc["archived_at"] = next_archive.clone();
        doc["position"] = json!(position);
        doc["updated_at"] = json!(stamp);
        let changed = tx.execute("UPDATE documents SET body=?,archived_at=? WHERE id=? AND matter_id=? AND archived_at IS ?",
            params![doc.to_string(),next_archive.as_str(),doc_id,matter_id,archived_at])?;
        if changed != 1 {
            return Ok(reply(409, json!({"detail":"Document state changed"})));
        }
        tx.execute(
            "DELETE FROM edit_history WHERE matter_id=? AND applied=0",
            [&matter_id],
        )?;
        tx.execute("INSERT INTO edit_history (matter_id,kind,before_json,after_json,created_at) VALUES (?,'document_presence',?,?,?)",
            params![matter_id,before.to_string(),after.to_string(),stamp])?;
        tx.execute("DELETE FROM edit_history WHERE matter_id=? AND id NOT IN (SELECT id FROM edit_history WHERE matter_id=? ORDER BY id DESC LIMIT 100)",params![matter_id,matter_id])?;
        tx.execute(
            "INSERT INTO audit_events (matter_id,document_id,action,created_at) VALUES (?,?,?,?)",
            params![
                matter_id,
                doc_id,
                if restore {
                    "document_restored"
                } else {
                    "document_archived"
                },
                stamp
            ],
        )?;
        tx.execute(
            "UPDATE matters SET body=json_set(body,'$.updated_at',?),updated_at=? WHERE id=?",
            params![stamp, stamp, matter_id],
        )?;
        crash_at(if restore {
            "document_restore_publication"
        } else {
            "document_archive_publication"
        });
        tx.commit()?;
        crash_at(if restore {
            "document_restore_committed"
        } else {
            "document_archive_committed"
        });
        Ok(reply(
            200,
            if restore {
                doc
            } else {
                json!({"status":"archived","files_retained":true})
            },
        ))
    }

    fn group_parent_refusal(
        con: &Connection,
        matter_id: &str,
        doc_id: &str,
        doc: &Value,
    ) -> rusqlite::Result<Option<Reply>> {
        if doc["group_role"] != "parent" {
            return Ok(None);
        }
        let group = doc["group_key"].as_str().unwrap_or("").trim();
        if group.is_empty() {
            return Ok(Some(reply(
                422,
                json!({"detail":"A group parent must have a group name"}),
            )));
        }
        let collision: Option<i64> = con.query_row("SELECT 1 FROM documents WHERE matter_id=? AND archived_at IS NULL AND id<>? AND json_extract(body,'$.group_key')=? AND json_extract(body,'$.group_role')='parent' LIMIT 1",
            params![matter_id,doc_id,group], |r| r.get(0)).optional()?;
        if collision.is_some() {
            return Ok(Some(reply(
                422,
                json!({"detail":format!("Group “{group}” already has a designated parent")}),
            )));
        }
        Ok(None)
    }

    fn active_matter(&self, id: &str) -> rusqlite::Result<bool> {
        let active: Option<i64> = self
            .con
            .query_row(
                "SELECT 1 FROM matters WHERE id=? AND json_extract(body,'$.archived_at') IS NULL",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(active.is_some())
    }

    fn verified_documents(
        con: &Connection,
        root: &Path,
        key: Option<&DataKey>,
        id: &str,
        archived: bool,
    ) -> rusqlite::Result<Result<Vec<Value>, Reply>> {
        let rows = Self::document_rows(con, id, archived)?;
        let mut documents = Vec::new();
        for (row_id, matter_id, row, archived_at) in rows {
            let doc = decode(&row)?;
            if let Err(message) = Self::verify_document_at(
                con,
                root,
                key,
                &doc,
                &row_id,
                &matter_id,
                archived_at.as_deref(),
            ) {
                return Ok(Err(reply(409, json!({"detail":message}))));
            }
            documents.push(doc);
        }
        Ok(Ok(documents))
    }

    fn document_rows(
        con: &Connection,
        id: &str,
        archived: bool,
    ) -> rusqlite::Result<Vec<(String, String, String, Option<String>)>> {
        let mut stmt = con.prepare(if archived {
                    "SELECT id,matter_id,body,archived_at FROM documents WHERE matter_id=? AND archived_at IS NOT NULL ORDER BY archived_at DESC,id DESC"
                } else {
                    "SELECT id,matter_id,body,archived_at FROM documents WHERE matter_id=? AND archived_at IS NULL ORDER BY json_extract(body,'$.position'),created_at,id"
                })?;
        let rows = stmt
            .query_map([id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect();
        rows
    }

    fn resource_request(&mut self, method: &str, path: &str) -> rusqlite::Result<Option<Reply>> {
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let ["matters", id, tail @ ..] = parts.as_slice() else {
            return Ok(None);
        };
        if method != "GET"
            || tail.len() != 1
            || !["documents", "archived-documents", "storage", "audit"].contains(&tail[0])
        {
            return Ok(None);
        }
        if !self.active_matter(id)? {
            return Ok(Some(reply(404, json!({"detail":"Matter not found"}))));
        }
        let response = match tail[0] {
            "documents" | "archived-documents" => {
                match Self::verified_documents(
                    &self.con,
                    &self.root,
                    self.key.as_ref(),
                    id,
                    tail[0] == "archived-documents",
                )? {
                    Ok(documents) => reply(200, Value::Array(documents)),
                    Err(refusal) => refusal,
                }
            }
            "storage" => {
                let mut originals = 0u64;
                let mut working = 0u64;
                let mut stmt = self.con.prepare(
                    "SELECT id,matter_id,body,archived_at FROM documents WHERE matter_id=?",
                )?;
                let rows = stmt
                    .query_map([id], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for (row_id, matter_id, row, archived_at) in rows {
                    let doc = decode(&row)?;
                    if let Err(message) =
                        self.verify_document(&doc, &row_id, &matter_id, archived_at.as_deref())
                    {
                        return Ok(Some(reply(409, json!({"detail":message}))));
                    }
                    let folder = self.document_folder(&row_id);
                    originals += fs::metadata(folder.join("original.pdf"))
                        .map_err(io_sql)?
                        .len();
                    if let Some(key) = &self.key {
                        working += revisions::storage_size(&self.con, &folder, key, &row_id)?;
                    } else {
                        working += fs::metadata(folder.join("revision-1.pdf"))
                            .map_err(io_sql)?
                            .len();
                    }
                }
                let exports = generated_exports::storage(&self.con, id)?;
                let other: u64 = self.con.query_row(
                    "SELECT (SELECT coalesce(sum(length(bytes)),0) FROM rust_annotation_assets
                     JOIN rust_annotations ON rust_annotations.id=rust_annotation_assets.id WHERE matter_id=?1)
                     + (SELECT coalesce(sum(length(bytes)),0) FROM rust_frozen_sources
                     JOIN rust_frozen_builds ON rust_frozen_builds.id=rust_frozen_sources.snapshot_id WHERE matter_id=?1)",
                    [id],|row|row.get(0),
                )?;
                reply(
                    200,
                    json!({"bytes":originals+working+exports+other,"by_category":{"originals":originals,"working":working,"exports":exports,"other":other}}),
                )
            }
            "audit" => {
                let mut stmt = self.con.prepare("SELECT action,document_id,created_at FROM audit_events WHERE matter_id=? ORDER BY id DESC LIMIT 500")?;
                let rows = stmt.query_map([id], |row| Ok(json!({"action":row.get::<_,String>(0)?,"document_id":row.get::<_,Option<String>>(1)?,"created_at":row.get::<_,String>(2)?})))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                reply(200, json!(rows))
            }
            _ => unreachable!(),
        };
        Ok(Some(response))
    }

    fn document_folder(&self, id: &str) -> PathBuf {
        self.root.join("documents-v1").join(id)
    }

    fn verify_document(
        &self,
        doc: &Value,
        id: &str,
        matter_id: &str,
        archived_at: Option<&str>,
    ) -> Result<(), String> {
        Self::verify_document_at(
            &self.con,
            &self.root,
            self.key.as_ref(),
            doc,
            id,
            matter_id,
            archived_at,
        )
    }

    fn check_document_identity(
        doc: &Value,
        id: &str,
        matter_id: &str,
        archived_at: Option<&str>,
    ) -> Result<(), String> {
        if uuid::Uuid::parse_str(id).is_err() || id.contains('-') {
            return Err("Invalid document identity".into());
        }
        if doc["id"] != id
            || doc["matter_id"] != matter_id
            || doc["archived_at"].as_str() != archived_at
            || doc["page_count"].as_u64().is_none_or(|pages| pages == 0)
        {
            return Err("Document identity or page metadata mismatch".into());
        }
        Ok(())
    }

    fn verify_document_at(
        con: &Connection,
        root: &Path,
        key: Option<&DataKey>,
        doc: &Value,
        id: &str,
        matter_id: &str,
        archived_at: Option<&str>,
    ) -> Result<(), String> {
        Self::check_document_identity(doc, id, matter_id, archived_at)?;
        document_verification::DocumentFiles::capture(con, root, key, doc, id)?
            .authenticate()
            .map(|_| ())
    }

    fn reconcile_documents(&mut self) -> Result<(), String> {
        // Import holds the same SQLite writer lock from staging through publication.
        // Reopen cleanup therefore cannot remove another process's live staging files.
        let tx = self
            .con
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let base = self.root.join("documents-v1");
        if base.is_symlink() {
            return Err("Document root symlink refused".into());
        }
        if !base.exists() {
            fs::create_dir(&base).map_err(|e| e.to_string())?;
        }
        for entry in fs::read_dir(&base).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_symlink() || !path.is_dir() {
                return Err("Foreign document content refused".into());
            }
            let id = entry.file_name().to_string_lossy().to_string();
            if id.ends_with(".staging") {
                let stage = id.trim_end_matches(".staging");
                let stem = stage.split('.').next().unwrap_or_default();
                if uuid::Uuid::parse_str(stem).is_err() || stem.contains('-') {
                    return Err("Foreign staging folder refused".into());
                }
                let safe = match &self.key {
                    Some(key) => {
                        let receipt = publication_receipt(key, stem)?;
                        stage == format!("{stem}{receipt}")
                            && safe_protected_orphan(&path, &receipt, true, key, stem)?
                    }
                    None => stem == stage && safe_pilot_orphan(&path, true)?,
                };
                if !safe {
                    return Err("Foreign staging content refused; nothing removed".into());
                }
                remove_document_directory(&path).map_err(|e| e.to_string())?;
                continue;
            }
            if uuid::Uuid::parse_str(&id).is_err() || id.contains('-') {
                return Err("Foreign document folder refused".into());
            }
            let exists: Option<i64> = tx
                .query_row("SELECT 1 FROM documents WHERE id=?", [&id], |r| r.get(0))
                .optional()
                .map_err(|e| e.to_string())?;
            if exists.is_none() {
                let safe = match &self.key {
                    Some(key) => safe_protected_orphan(
                        &path,
                        &publication_receipt(key, &id)?,
                        false,
                        key,
                        &id,
                    )?,
                    None => safe_pilot_orphan(&path, false)?,
                };
                if !safe {
                    return Err("Foreign document content refused; nothing removed".into());
                }
                remove_document_directory(&path).map_err(|e| e.to_string())?;
            } else if let Some(key) = &self.key {
                revisions::reconcile(&tx, &path, key, &id)?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn import_pdf(&mut self, matter_id: &str, filename: &str, bytes: Vec<u8>) -> Reply {
        match self.import_pdf_inner(matter_id, filename, bytes, false) {
            Ok(reply) => reply,
            Err(message) => reply(500, json!({"detail":message})),
        }
    }

    fn import_pdf_inner(
        &mut self,
        matter_id: &str,
        filename: &str,
        bytes: Vec<u8>,
        inject_rename_collision: bool,
    ) -> Result<Reply, String> {
        self.import_pdf_with_options(
            matter_id,
            filename,
            &bytes,
            ImportOptions::default(),
            inject_rename_collision,
        )
    }

    pub(crate) fn import_protected_pdf(
        &mut self,
        matter_id: &str,
        filename: &str,
        bytes: &[u8],
        options: ImportOptions<'_>,
    ) -> Reply {
        if self.key.is_none() {
            return reply(423, json!({"detail":"Unlock the vault first"}));
        }
        self.import_pdf_with_options(matter_id, filename, bytes, options, false)
            .unwrap_or_else(|error| reply(500, json!({"detail":error})))
    }

    fn import_pdf_with_options(
        &mut self,
        matter_id: &str,
        filename: &str,
        bytes: &[u8],
        options: ImportOptions<'_>,
        inject_rename_collision: bool,
    ) -> Result<Reply, String> {
        let target = match self.capture_document_import(matter_id) {
            Ok(target) => target,
            Err(error) => return Ok(error),
        };
        let prepared =
            match target.measure_bytes(filename.to_owned(), Cow::Borrowed(bytes), options, None) {
                Ok(prepared) => prepared,
                Err(error) => return Ok(error),
            };
        self.publish_document_import(prepared, inject_rename_collision)
    }

    pub fn capture_document_import(&self, matter: &str) -> Result<DocumentImportTarget, Reply> {
        if !self
            .active_matter(matter)
            .map_err(|error| reply(500, json!({"detail":error.to_string()})))?
        {
            return Err(reply(404, json!({"detail":"Matter not found"})));
        }
        Ok(DocumentImportTarget {
            root: self.root.clone(),
            matter: matter.to_owned(),
            protected: self.key.is_some(),
        })
    }

    pub fn complete_document_import(&mut self, prepared: PreparedDocumentImport<'_>) -> Reply {
        self.publish_document_import(prepared, false)
            .unwrap_or_else(|error| reply(500, json!({"detail":error})))
    }

    fn publish_document_import(
        &mut self,
        prepared: PreparedDocumentImport<'_>,
        inject_rename_collision: bool,
    ) -> Result<Reply, String> {
        let PreparedDocumentImport {
            target,
            filename,
            original,
            working,
            hash,
            working_hash,
            pages,
            signature,
            ocr_status,
            ocr_provenance,
            included,
            source_file_mtime,
            cover_inputs,
        } = prepared;
        let key = self.key.as_ref();
        if target.root != self.root || target.protected != key.is_some() {
            return Ok(reply(
                409,
                json!({"detail":"Document store changed; choose the source again"}),
            ));
        }
        let matter_id = target.matter.as_str();
        let (stem, _) = filename.rsplit_once('.').unwrap_or(("", ""));
        let working = working.as_deref().unwrap_or(&original);
        let generated_cover = cover_inputs.is_some();
        crash_at("before_writer_lock");
        let tx = self
            .con
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let still_active: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM matters WHERE id=? AND json_extract(body,'$.archived_at') IS NULL",
                [matter_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if still_active.is_none() {
            return Ok(reply(404, json!({"detail":"Matter not found"})));
        }
        if let Some(inputs) = cover_inputs {
            let current = document_metadata::matter(&tx, matter_id)
                .map_err(|error| error.to_string())?
                .ok_or("Matter not found")?;
            if file_ingest::cover_fields(&current) != inputs {
                return Ok(reply(
                    409,
                    json!({"detail":"Matter details changed; generate the cover again"}),
                ));
            }
        }
        let duplicate: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM documents WHERE matter_id=? AND source_sha256=?",
                params![matter_id, hash],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if key.is_none() && duplicate.is_some() {
            return Ok(reply(
                409,
                json!({"detail":"This PDF was already imported into the matter"}),
            ));
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let receipt = key.map(|key| publication_receipt(key, &id)).transpose()?;
        let folder = self.root.join("documents-v1").join(&id);
        let staging = self.root.join("documents-v1").join(format!(
            "{id}{}.staging",
            receipt.as_deref().unwrap_or_default()
        ));
        fs::create_dir(&staging).map_err(|e| e.to_string())?;
        let mut published = false;
        let result = (|| -> Result<Reply, String> {
            crash_at("staging");
            if let Some(receipt) = &receipt {
                let marker = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(staging.join(receipt))
                    .map_err(|error| error.to_string())?;
                marker.sync_all().map_err(|error| error.to_string())?;
            }
            for (name, content) in [
                ("original.pdf", original.as_ref()),
                ("revision-1.pdf", working),
            ] {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(staging.join(name))
                    .map_err(|e| e.to_string())?;
                if let Some(key) = key {
                    let mut encrypted = vault::EncryptedWriter::new(&mut file, key)
                        .map_err(|error| error.to_string())?;
                    encrypted
                        .write_all(content)
                        .map_err(|error| error.to_string())?;
                    encrypted.finish().map_err(|error| error.to_string())?;
                } else {
                    file.write_all(content).map_err(|e| e.to_string())?;
                }
                fail_at(if name == "original.pdf" {
                    "before_first_sync"
                } else {
                    "before_second_sync"
                })?;
                file.sync_all().map_err(|e| e.to_string())?;
                crash_at(if name == "original.pdf" {
                    "first_sync"
                } else {
                    "second_sync"
                });
            }
            let original = staging.join("original.pdf");
            let mut permissions = fs::metadata(&original)
                .map_err(|e| e.to_string())?
                .permissions();
            permissions.set_readonly(true);
            fs::set_permissions(&original, permissions).map_err(|e| e.to_string())?;
            fail_at("before_stage_directory_sync")?;
            sync_directory(&staging)?;
            if inject_rename_collision {
                fs::create_dir(&folder).map_err(|e| e.to_string())?;
                fs::write(folder.join("collision-sentinel"), b"untouched")
                    .map_err(|e| e.to_string())?;
            }
            publish_directory(&staging, &folder).map_err(|e| e.to_string())?;
            published = true;
            crash_at("rename");
            fail_at("before_parent_directory_sync")?;
            sync_directory(&self.root.join("documents-v1"))?;
            let stamp = now();
            let mut title = stem;
            if key.is_none() {
                title = title.trim_end_matches([' ', '.']);
            }
            let position: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(json_extract(body,'$.position')),-1)+1 FROM documents WHERE matter_id=?",
                    [matter_id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            let matter_raw: String = tx
                .query_row("SELECT body FROM matters WHERE id=?", [matter_id], |r| {
                    r.get(0)
                })
                .map_err(|e| e.to_string())?;
            let matter = decode(&matter_raw).map_err(|e| e.to_string())?;
            let part = matter["parts"][0]
                .as_i64()
                .ok_or("Matter has no valid Part")?;
            let title: String = if generated_cover {
                "Draft matter cover sheet"
            } else {
                title
            }
            .chars()
            .take(250)
            .collect();
            let original_name: String = filename.chars().take(250).collect();
            let mut body = json!({"id":id,"matter_id":matter_id,"title":title,"notes":"","kind":"other","part":part,"group_key":"","group_role":"member","label":"","label_status":"unreviewed","label_source":"none","document_date":"","date_status":"unknown","date_source":"none","date_note":"","source_file_mtime":source_file_mtime,"position":position,"page_count":pages,"ocr_status":ocr_status,"ocr_provenance_json":ocr_provenance,"original_name":original_name,"source_sha256":hash,"certificate_signature_status":"unknown","certificate_signature_count":0,"signature_inspection_note":"Pilot screening only; signature inspection and legal validity are not verified","visible_signature_status":"unreviewed","signature_status":"inspection_inconclusive","revision":1,"bundle_included":i64::from(included),"created_at":stamp,"updated_at":stamp,"archived_at":null});
            if let Some(signature) = signature {
                let fields = serde_json::to_value(signature).map_err(|error| error.to_string())?;
                body.as_object_mut()
                    .expect("document JSON is an object")
                    .extend(
                        fields
                            .as_object()
                            .expect("signature JSON is an object")
                            .clone(),
                    );
            }
            fail_at("before_readback")?;
            for (name, expected) in [("original.pdf", &hash), ("revision-1.pdf", &working_hash)] {
                if document_file_hash(&folder.join(name), key)? != *expected {
                    return Err(format!("Imported {name} failed hash verification"));
                }
            }
            tx.execute(
                "INSERT INTO documents VALUES (?,?,?,?,?,?,?,NULL)",
                params![
                    id,
                    matter_id,
                    body.to_string(),
                    hash,
                    format!("documents-v1/{id}/original.pdf"),
                    format!("documents-v1/{id}/revision-1.pdf"),
                    stamp
                ],
            )
            .map_err(|e| e.to_string())?;
            if key.is_some() {
                tx.execute(
                    "INSERT INTO document_revisions VALUES (?,?,?,?,?,?,?,?)",
                    params![
                        id,
                        1,
                        format!("documents-v1/{id}/revision-1.pdf"),
                        working_hash,
                        stamp,
                        pages,
                        ocr_status,
                        ocr_provenance
                    ],
                )
            } else {
                tx.execute(
                    "INSERT INTO document_revisions VALUES (?,?,?,?,?)",
                    params![
                        id,
                        1,
                        format!("documents-v1/{id}/revision-1.pdf"),
                        working_hash,
                        stamp
                    ],
                )
            }
            .map_err(|error| error.to_string())?;
            tx.execute(
                "DELETE FROM edit_history WHERE matter_id=? AND applied=0",
                [matter_id],
            )
            .map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO edit_history (matter_id,kind,before_json,after_json,created_at) VALUES (?,'document_presence',?,?,?)",params![matter_id,json!({"id":id,"archived_at":stamp,"position":position}).to_string(),json!({"id":id,"archived_at":null,"position":position}).to_string(),stamp]).map_err(|e| e.to_string())?;
            tx.execute("DELETE FROM edit_history WHERE matter_id=? AND id NOT IN (SELECT id FROM edit_history WHERE matter_id=? ORDER BY id DESC LIMIT 100)",params![matter_id,matter_id]).map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO audit_events (matter_id,document_id,action,created_at) VALUES (?,?,'document_imported',?)",params![matter_id,id,stamp]).map_err(|e| e.to_string())?;
            tx.execute(
                "UPDATE matters SET body=json_set(body,'$.updated_at',?),updated_at=? WHERE id=?",
                params![stamp, stamp, matter_id],
            )
            .map_err(|e| e.to_string())?;
            crash_at("publication");
            tx.commit().map_err(|e| e.to_string())?;
            crash_at("committed");
            Ok(reply(201, json!([body])))
        })();
        if result.is_err() {
            let _ = remove_document_directory(&staging);
            if published {
                let committed: Option<i64> = self
                    .con
                    .query_row("SELECT 1 FROM documents WHERE id=?", [&id], |r| r.get(0))
                    .optional()
                    .unwrap_or(Some(1));
                if committed.is_none() {
                    let _ = remove_document_directory(&folder);
                }
            }
        }
        result
    }
}

fn safe_pilot_orphan(folder: &Path, staging: bool) -> Result<bool, String> {
    // A generated UUID alone does not prove ownership. Only an empty stage or
    // partial copies of the pinned fixture may be removed after interruption.
    // Published folders must contain both complete files: rename follows both syncs.
    let mut original = false;
    let mut revision = false;
    for entry in fs::read_dir(folder).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if name != "original.pdf" && name != "revision-1.pdf" {
            return Ok(false);
        }
        if name == "original.pdf" {
            original = true;
        } else {
            revision = true;
        }
        let path = entry.path();
        if path.is_symlink() || !path.is_file() {
            return Ok(false);
        }
        if fs::metadata(&path).map_err(|e| e.to_string())?.len() > PILOT_PDF_BYTES.len() as u64 {
            return Ok(false);
        }
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        if !PILOT_PDF_BYTES.starts_with(&bytes) {
            return Ok(false);
        }
        if !staging && bytes != PILOT_PDF_BYTES {
            return Ok(false);
        }
    }
    Ok(staging || (original && revision))
}

fn check_receipt(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() != 0 {
        return Err("Invalid document publication receipt".into());
    }
    Ok(())
}

fn safe_protected_orphan(
    folder: &Path,
    receipt: &str,
    staging: bool,
    key: &DataKey,
    id: &str,
) -> Result<bool, String> {
    let mut names = Vec::new();
    for entry in fs::read_dir(folder).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if (!matches!(name.as_str(), "original.pdf")
            && name != receipt
            && !revisions::valid_file_name(key, id, &name)?)
            || !entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_file()
        {
            return Ok(false);
        }
        if name == receipt {
            check_receipt(&entry.path())?;
        }
        names.push(name);
    }
    // Staging ownership is encoded in the directory name. After the rename
    // the persisted receipt establishes ownership of the complete folder.
    Ok(staging
        || (names.iter().any(|name| name == receipt)
            && names.iter().any(|name| name == "original.pdf")
            && names.iter().any(|name| name.starts_with("revision-"))))
}

fn io_sql(error: std::io::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

fn publish_directory(source: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
        let wide = |path: &Path| {
            path.as_os_str()
                .encode_wide()
                .chain([0])
                .collect::<Vec<_>>()
        };
        let source = wide(source);
        let target = wide(target);
        if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    fs::rename(source, target)
}

fn remove_document_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        let original = path.join("original.pdf");
        if let Ok(metadata) = fs::symlink_metadata(&original) {
            if metadata.is_file() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
            {
                let mut permissions = metadata.permissions();
                if permissions.readonly() {
                    permissions.set_readonly(false);
                    fs::set_permissions(original, permissions)?;
                }
            }
        }
    }
    fs::remove_dir_all(path)
}

fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn record_edit(
    con: &Connection,
    matter_id: &str,
    kind: &str,
    before: &Value,
    after: &Value,
    stamp: &str,
) -> rusqlite::Result<()> {
    con.execute(
        "DELETE FROM edit_history WHERE matter_id=? AND applied=0",
        [matter_id],
    )?;
    con.execute(
        "INSERT INTO edit_history (matter_id,kind,before_json,after_json,created_at) VALUES (?,?,?,?,?)",
        params![matter_id, kind, before.to_string(), after.to_string(), stamp],
    )?;
    con.execute("DELETE FROM edit_history WHERE matter_id=? AND id NOT IN (SELECT id FROM edit_history WHERE matter_id=? ORDER BY id DESC LIMIT 100)",params![matter_id,matter_id])?;
    Ok(())
}

fn history_state(con: &Connection, id: &str, protected: bool) -> rusqlite::Result<Value> {
    let mut stmt = con.prepare(
        "SELECT kind,after_json,applied FROM edit_history WHERE matter_id=? ORDER BY id DESC",
    )?;
    let rows = stmt
        .query_map([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let label = |row: &(String, String, bool)| -> rusqlite::Result<Value> {
        Ok(json!(match row.0.as_str() {
            "matter" => "Matter settings",
            "matter_parts" => "Matter Parts",
            "document" =>
                if decode(&row.1)?["fields"].get("working_relpath").is_some() {
                    "Working PDF edit"
                } else {
                    "Document details"
                },
            "document_presence" =>
                if decode(&row.1)?["archived_at"].is_null() {
                    "Add or restore document"
                } else {
                    "Archive document"
                },
            "document_batch" => "Batch document details",
            "bundle_membership" => "Bundle membership",
            "order" => "Document order",
            "bookmarks" => "Bookmarks",
            "reviewed_links" => "Reviewed links",
            "output_marks" => "Output marks",
            "output_preset" => "Output preset",
            "page_annotations" => "Private review notes or overlays",
            "memo_parties" => "Memo of Parties",
            "matter_values" => "Matter values",
            _ => "Matter edit",
        }))
    };
    let undo = rows.iter().filter(|r| r.2).count();
    let redo = rows.len() - undo;
    let undo_label = rows
        .iter()
        .find(|r| r.2)
        .map(&label)
        .transpose()?
        .unwrap_or(Value::Null);
    let redo_label = rows
        .iter()
        .rev()
        .find(|r| !r.2)
        .map(&label)
        .transpose()?
        .unwrap_or(Value::Null);
    let recent = rows
        .iter()
        .take(8)
        .map(|r| Ok(json!({"label":label(r)?,"applied":r.2})))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let supported = |r: &(String, String, bool)| {
        ["matter", "document_presence", "order"].contains(&r.0.as_str())
            || preparation::history_supported(&r.0)
            || ["document_batch", "page_annotations"].contains(&r.0.as_str())
            || ["bundle_membership", "output_marks", "output_preset"].contains(&r.0.as_str())
            || bundle_inputs::history_kind(&r.0)
            || (r.0 == "document"
                && (protected
                    || decode(&r.1)
                        .is_ok_and(|state| state["fields"].get("working_relpath").is_none())))
    };
    let next_undo_supported = rows.iter().find(|r| r.2).is_some_and(supported);
    let next_redo_supported = rows.iter().rev().find(|r| !r.2).is_some_and(supported);
    Ok(
        json!({"can_undo":undo>0 && next_undo_supported,"can_redo":redo>0 && next_redo_supported,"undo_count":undo,"redo_count":redo,"undo_label":undo_label,"redo_label":redo_label,"recent_actions":recent}),
    )
}

fn validate(body: Value, create: bool) -> Result<Value, Reply> {
    validate_with_order(body, create, &[])
}

fn validate_with_order(body: Value, create: bool, order: &[String]) -> Result<Value, Reply> {
    let mut errors = Vec::new();
    let mut out = if create {
        json!({"court_forum":"","client_reference":"","internal_reference":"","notes":"","court_profile":"general-demo","case_type":"Demonstration filing","petitioner":"","respondent":"","advocate":"","case_number":"","max_pages_per_volume":300,"keep_groups_intact":true,"parts":[1,2,3,4],"next_part":5})
    } else {
        json!({})
    };
    if body.is_null() {
        return Err(reply(
            422,
            json!({"detail":[{"type":"missing","loc":["body"],"msg":"Field required","input":null}]}),
        ));
    }
    let Some(obj) = body.as_object() else {
        return Err(reply(
            422,
            json!({"detail":[{"type":"model_attributes_type","loc":["body"],"msg":"Input should be a valid dictionary or object to extract fields from","input":body}]}),
        ));
    };
    let strings = [
        ("name", 2, 200),
        ("court_forum", 0, 300),
        ("client_reference", 0, 300),
        ("internal_reference", 0, 200),
        ("notes", 0, 2000),
        ("court_profile", 0, usize::MAX),
        ("case_type", 0, 150),
        ("petitioner", 0, 300),
        ("respondent", 0, 300),
        ("advocate", 0, 200),
        ("case_number", 0, 100),
    ];
    if create && !obj.contains_key("name") {
        errors.push(
            json!({"type":"missing","loc":["body","name"],"msg":"Field required","input":body}),
        );
    }
    let mut keys: Vec<&str> = strings.iter().map(|(key, _, _)| *key).collect();
    keys.extend([
        "max_pages_per_volume",
        "keep_groups_intact",
        "parts",
        "next_part",
    ]);
    let extra: Vec<&str> = crate::request_order::ordered_fields(obj, order)
        .into_iter()
        .map(|(key, _)| key)
        .filter(|k| !keys.contains(k))
        .collect();
    keys.extend(extra);
    for key in keys {
        let Some(input) = obj.get(key) else { continue };
        let mut value = input.clone();
        let mut err: Option<(&str, String, Value)> = None;
        if let Some((_, min, max)) = strings.iter().find(|(k, _, _)| *k == key) {
            if input.is_null() && !create {
            } else if let Some(s) = input.as_str() {
                let n = s.chars().count();
                if n < *min {
                    err = Some((
                        "string_too_short",
                        format!("String should have at least {min} characters"),
                        json!({"min_length":min}),
                    ));
                }
                if n > *max {
                    err = Some((
                        "string_too_long",
                        format!("String should have at most {max} characters"),
                        json!({"max_length":max}),
                    ));
                }
            } else {
                err = Some((
                    "string_type",
                    "Input should be a valid string".into(),
                    Value::Null,
                ));
            }
        } else if key == "keep_groups_intact" {
            if !(input.is_null() && !create) {
                let b = boolean(input);
                if let Some(b) = b {
                    value = json!(b)
                } else {
                    err = Some((
                        if input.is_string()
                            || input
                                .as_f64()
                                .is_some_and(|n| n.fract() == 0.0 && n.abs() < i64::MAX as f64)
                        {
                            "bool_parsing"
                        } else {
                            "bool_type"
                        },
                        if input.is_string()
                            || input
                                .as_f64()
                                .is_some_and(|n| n.fract() == 0.0 && n.abs() < i64::MAX as f64)
                        {
                            "Input should be a valid boolean, unable to interpret input"
                        } else {
                            "Input should be a valid boolean"
                        }
                        .into(),
                        Value::Null,
                    ));
                }
            }
        } else if key == "max_pages_per_volume" || (create && key == "next_part") {
            if !(input.is_null() && !create) {
                let n = integer(input);
                let max = if key == "next_part" { 100 } else { 2000 };
                if let Some(n) = n {
                    value = json!(n);
                    if n < 2 {
                        err = Some((
                            "greater_than_equal",
                            "Input should be greater than or equal to 2".into(),
                            json!({"ge":2}),
                        ));
                    } else if n > max {
                        err = Some((
                            "less_than_equal",
                            format!("Input should be less than or equal to {max}"),
                            json!({"le":max}),
                        ));
                    }
                } else {
                    let (kind, msg) = integer_error(input);
                    err = Some((kind, msg.into(), Value::Null));
                }
            }
        } else if create && key == "parts" {
            if let Some(parts) = input.as_array() {
                if parts.is_empty() || parts.len() > 50 {
                    let (kind, bound, word) = if parts.is_empty() {
                        ("too_short", 1, "at least")
                    } else {
                        ("too_long", 50, "at most")
                    };
                    let mut ctx = json!({"field_type":"List","actual_length":parts.len()});
                    ctx[if parts.is_empty() {
                        "min_length"
                    } else {
                        "max_length"
                    }] = json!(bound);
                    err = Some((
                        kind,
                        format!(
                            "List should have {word} {bound} item{} after validation, not {}",
                            if bound == 1 { "" } else { "s" },
                            parts.len()
                        ),
                        ctx,
                    ));
                } else {
                    let mut parsed = Vec::with_capacity(parts.len());
                    for (i, part) in parts.iter().enumerate() {
                        if let Some(n) = integer(part) {
                            parsed.push(n);
                        } else {
                            let (kind, msg) = integer_error(part);
                            errors.push(
                                json!({"type":kind,"loc":["body",key,i],"msg":msg,"input":part}),
                            );
                        }
                    }
                    value = json!(parsed);
                }
            } else {
                err = Some((
                    "list_type",
                    "Input should be a valid list".into(),
                    Value::Null,
                ));
            }
        } else {
            if !create {
                errors.push(json!({"type":"extra_forbidden","loc":["body",key],"msg":"Extra inputs are not permitted","input":input}));
            }
            continue;
        }
        if let Some((kind, msg, ctx)) = err {
            let mut e = json!({"type":kind,"loc":["body",key],"msg":msg,"input":input});
            if !ctx.is_null() {
                e["ctx"] = ctx;
            }
            errors.push(e);
        } else {
            out[key] = value;
        }
    }
    if !errors.is_empty() {
        return Err(reply(422, json!({"detail":errors})));
    }
    if create {
        let p = out["parts"].as_array().unwrap();
        let nums: Vec<i64> = p.iter().map(|v| v.as_i64().unwrap()).collect();
        if nums.iter().any(|n| *n < 1 || *n > 99)
            || nums.windows(2).any(|w| w[0] >= w[1])
            || out["next_part"].as_i64().unwrap() <= *nums.last().unwrap()
        {
            return Err(reply(
                422,
                json!({"detail":[{"type":"value_error","loc":["body"],"msg":"Value error, Parts must be unique ordered numbers below the next Part number","input":body,"ctx":{"error":{}}}]}),
            ));
        }
    }
    Ok(out)
}
fn boolean(input: &Value) -> Option<bool> {
    if let Some(value) = input.as_bool() {
        return Some(value);
    }
    match input.as_f64() {
        Some(0.0) => Some(false),
        Some(1.0) => Some(true),
        _ => match input.as_str()?.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "y" | "on" | "t" => Some(true),
            "false" | "0" | "no" | "n" | "off" | "f" => Some(false),
            _ => None,
        },
    }
}

fn integer(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_u64().map(|n| n.min(i64::MAX as u64) as i64))
        .or_else(|| v.as_bool().map(i64::from))
        .or_else(|| {
            v.as_f64()
                .filter(|n| n.fract() == 0.0 && n.abs() < i64::MAX as f64)
                .map(|n| n as i64)
        })
        .or_else(|| {
            v.as_str().and_then(|s| {
                let text = s.trim();
                let (whole, fraction) = text
                    .split_once('.')
                    .map_or((text, None), |(a, b)| (a, Some(b)));
                if fraction.is_some_and(|f| f.is_empty() || !f.bytes().all(|b| b == b'0')) {
                    return None;
                }
                let digits = whole.strip_prefix(['+', '-']).unwrap_or(whole);
                if digits.is_empty()
                    || digits.starts_with('_')
                    || digits.ends_with('_')
                    || digits.contains("__")
                    || !digits.bytes().all(|b| b.is_ascii_digit() || b == b'_')
                {
                    return None;
                }
                whole
                    .replace('_', "")
                    .parse()
                    .ok()
                    .or(Some(if whole.starts_with('-') {
                        i64::MIN
                    } else {
                        i64::MAX
                    }))
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    const PDF: &[u8] = include_bytes!("../../samples/01_petition.pdf");
    fn root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("matterdraft-rust-{}", uuid::Uuid::new_v4()))
    }
    #[test]
    fn preparation_audit_failure_rolls_back_the_edit_and_redo_branch() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(&root.path().join("matter")).unwrap();
        let matter = store
            .request(
                "POST",
                "/matters",
                json!({"name":"Synthetic preparation rollback"}),
            )
            .body;
        let path = format!("/matters/{}", matter["id"].as_str().unwrap());
        let values = format!("{path}/values");
        assert_eq!(
            store
                .request("PUT", &values, json!({"values":{"note":"Synthetic"}}))
                .status,
            200
        );
        assert_eq!(
            store
                .request("POST", &format!("{path}/history/undo"), Value::Null)
                .status,
            200
        );
        store
            .con
            .execute_batch(
                "CREATE TRIGGER refuse_preparation_audit BEFORE INSERT ON audit_events
             WHEN NEW.action IN ('matter_values_updated', 'matter_part_added') BEGIN
                 SELECT RAISE(ABORT, 'synthetic audit failure');
             END;",
            )
            .unwrap();
        let before = store.request("GET", &path, Value::Null).body;
        let history = format!("{path}/history");
        let history_before = store.request("GET", &history, Value::Null).body;
        assert_eq!(
            store
                .request("PUT", &values, json!({"values":{"note":"Aborted"}}))
                .status,
            500
        );
        assert_eq!(
            store
                .request("POST", &format!("{path}/parts"), Value::Null)
                .status,
            500
        );
        assert_eq!(store.request("GET", &path, Value::Null).body, before);
        assert_eq!(
            store.request("GET", &values, Value::Null).body,
            json!({"values":{}})
        );
        assert_eq!(
            store.request("GET", &history, Value::Null).body,
            history_before
        );
    }

    #[test]
    fn upgrades_existing_schema_one_without_replacing_matter_history() {
        let root = root();
        fs::create_dir(&root).unwrap();
        fs::write(root.join("RUST_SYNTHETIC_STORE"), MARKER).unwrap();
        let con = Connection::open(root.join("rust-matters.sqlite3")).unwrap();
        con.execute_batch("CREATE TABLE matters (id TEXT PRIMARY KEY, body TEXT NOT NULL, updated_at TEXT NOT NULL); CREATE TABLE audit_events (id INTEGER PRIMARY KEY, matter_id TEXT NOT NULL, action TEXT NOT NULL, created_at TEXT NOT NULL); CREATE TABLE edit_history (id INTEGER PRIMARY KEY, matter_id TEXT NOT NULL, kind TEXT NOT NULL, before_json TEXT NOT NULL, after_json TEXT NOT NULL, created_at TEXT NOT NULL, applied INTEGER NOT NULL DEFAULT 1); PRAGMA user_version=1;").unwrap();
        let legacy = json!({"id":"legacy","name":"Synthetic v1","archived_at":null,"updated_at":"2026-11-17T00:00:00.000000+00:00"});
        con.execute(
            "INSERT INTO matters VALUES (?,?,?)",
            params!["legacy", legacy.to_string(), legacy["updated_at"].as_str()],
        )
        .unwrap();
        con.execute("INSERT INTO audit_events (matter_id,action,created_at) VALUES ('legacy','matter_created','2026')",[]).unwrap();
        drop(con);
        let mut store = Store::open(&root).unwrap();
        assert_eq!(
            store
                .con
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        assert_eq!(
            store.request("GET", "/matters/legacy", Value::Null).body,
            legacy
        );
        assert_eq!(
            store
                .request("GET", "/matters/legacy/documents", Value::Null)
                .body,
            json!([])
        );
        assert_eq!(
            store
                .request("GET", "/matters/legacy/audit", Value::Null)
                .body[0]["document_id"],
            Value::Null
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn pdf_import_reopen_duplicate_and_missing_file_refusal() {
        let root = root();
        let source_hash = sha256_hex(PDF);
        let mut store = Store::open(&root).unwrap();
        let matter = store
            .request("POST", "/matters", json!({"name":"Synthetic PDF"}))
            .body;
        let matter_id = matter["id"].as_str().unwrap();
        let route = format!("/matters/{matter_id}/documents");
        let imported = store.import_pdf(matter_id, "petition.pdf", PDF.to_vec());
        assert_eq!(imported.status, 201, "{:?}", imported.body);
        let doc = &imported.body[0];
        assert_eq!(doc["page_count"], 3);
        assert_eq!(doc["source_sha256"], source_hash);
        assert_eq!(doc["bundle_included"], 0);
        let folder = store.document_folder(doc["id"].as_str().unwrap());
        assert_eq!(fs::read(folder.join("original.pdf")).unwrap(), PDF);
        assert_eq!(fs::read(folder.join("revision-1.pdf")).unwrap(), PDF);
        assert!(fs::metadata(folder.join("original.pdf"))
            .unwrap()
            .permissions()
            .readonly());
        assert_eq!(
            store.import_pdf(matter_id, "same.pdf", PDF.to_vec()).status,
            409
        );
        assert_eq!(
            store
                .request("GET", &route, Value::Null)
                .body
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let usage = store.request("GET", &format!("/matters/{matter_id}/storage"), Value::Null);
        assert_eq!(usage.body["by_category"]["originals"], PDF.len());
        assert_eq!(usage.body["by_category"]["working"], PDF.len());
        drop(store);
        let mut store = Store::open(&root).unwrap();
        assert_eq!(
            store.request("GET", &route, Value::Null).body[0]["source_sha256"],
            source_hash
        );
        let audit = store.request("GET", &format!("/matters/{matter_id}/audit"), Value::Null);
        assert_eq!(audit.body[0]["action"], "document_imported");
        assert_eq!(audit.body[0]["document_id"], doc["id"]);
        let history = store.request("GET", &format!("/matters/{matter_id}/history"), Value::Null);
        assert_eq!(
            history.body["recent_actions"][0]["label"],
            "Add or restore document"
        );
        assert_eq!(history.body["can_undo"], true);
        assert_eq!(
            store
                .request(
                    "POST",
                    &format!("/matters/{matter_id}/history/undo"),
                    Value::Null
                )
                .status,
            200
        );
        assert_eq!(store.request("GET", &route, Value::Null).body, json!([]));
        assert_eq!(
            store
                .request(
                    "POST",
                    &format!("/matters/{matter_id}/history/redo"),
                    Value::Null
                )
                .status,
            200
        );
        store
            .con
            .execute(
                "UPDATE document_revisions SET sha256='stale' WHERE document_id=?",
                [doc["id"].as_str().unwrap()],
            )
            .unwrap();
        assert_eq!(store.request("GET", &route, Value::Null).status, 409);
        store
            .con
            .execute(
                "UPDATE document_revisions SET sha256=? WHERE document_id=?",
                params![source_hash, doc["id"].as_str().unwrap()],
            )
            .unwrap();
        let foreign = folder.join("foreign-sentinel");
        fs::write(&foreign, b"preserve foreign content").unwrap();
        assert_eq!(store.request("GET", &route, Value::Null).status, 409);
        assert_eq!(fs::read(&foreign).unwrap(), b"preserve foreign content");
        fs::remove_file(foreign).unwrap();
        let original_path = folder.join("original.pdf");
        let mut permissions = fs::metadata(&original_path).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(&original_path, permissions.clone()).unwrap();
        fs::write(&original_path, vec![0; PDF.len() + 1]).unwrap();
        assert!(read_pilot_file(&original_path).is_err());
        assert_eq!(store.request("GET", &route, Value::Null).status, 409);
        fs::write(&original_path, b"changed synthetic bytes").unwrap();
        assert_eq!(store.request("GET", &route, Value::Null).status, 409);
        fs::write(&original_path, PDF).unwrap();
        permissions.set_readonly(true);
        fs::set_permissions(&original_path, permissions).unwrap();
        fs::remove_file(folder.join("revision-1.pdf")).unwrap();
        assert_eq!(store.request("GET", &route, Value::Null).status, 409);
        assert_eq!(
            store
                .request("GET", &format!("/matters/{matter_id}/storage"), Value::Null)
                .status,
            409
        );
        assert_eq!(fs::read(folder.join("original.pdf")).unwrap(), PDF);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pdf_import_failure_rollback_and_orphan_recovery() {
        let root = root();
        let mut store = Store::open(&root).unwrap();
        let matter = store
            .request("POST", "/matters", json!({"name":"Rollback PDF"}))
            .body;
        let id = matter["id"].as_str().unwrap();
        for (name, bytes, status) in [
            ("bad.png", PDF.to_vec(), 415),
            ("../bad.pdf", PDF.to_vec(), 422),
            ("bad.pdf", b"%PDF-invalid".to_vec(), 422),
        ] {
            assert_eq!(store.import_pdf(id, name, bytes).status, status);
        }
        let source_hash = Sha256::digest(PDF);
        assert!(store
            .import_pdf_inner(id, "petition.pdf", PDF.to_vec(), true)
            .is_err());
        assert_eq!(Sha256::digest(PDF), source_hash);
        let collision = fs::read_dir(root.join("documents-v1"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            fs::read(collision.join("collision-sentinel")).unwrap(),
            b"untouched"
        );
        fs::remove_dir_all(collision).unwrap();
        store.con.execute_batch("CREATE TRIGGER fail_import BEFORE INSERT ON audit_events BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert_eq!(
            store.import_pdf(id, "petition.pdf", PDF.to_vec()).status,
            500
        );
        assert_eq!(
            store
                .request("GET", &format!("/matters/{id}/documents"), Value::Null)
                .body,
            json!([])
        );
        assert_eq!(fs::read_dir(root.join("documents-v1")).unwrap().count(), 0);
        store.con.execute_batch("DROP TRIGGER fail_import").unwrap();
        let orphan = root
            .join("documents-v1")
            .join(uuid::Uuid::new_v4().simple().to_string());
        fs::create_dir(&orphan).unwrap();
        fs::write(orphan.join("original.pdf"), PDF).unwrap();
        fs::write(orphan.join("revision-1.pdf"), PDF).unwrap();
        let staging = root
            .join("documents-v1")
            .join(format!("{}.staging", uuid::Uuid::new_v4().simple()));
        fs::create_dir(&staging).unwrap();
        drop(store);
        let mut store = Store::open(&root).unwrap();
        assert!(!orphan.exists());
        assert!(!staging.exists());
        assert_eq!(
            store.import_pdf(id, "petition.pdf", PDF.to_vec()).status,
            201
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn startup_preserves_foreign_collision_content() {
        let root = root();
        let store = Store::open(&root).unwrap();
        drop(store);
        let source_before = Sha256::digest(PDF);
        let base = root.join("documents-v1");
        for suffix in ["", ".staging"] {
            let folder = base.join(format!("{}{}", uuid::Uuid::new_v4().simple(), suffix));
            fs::create_dir(&folder).unwrap();
            fs::write(folder.join("collision-sentinel"), b"unrelated").unwrap();
            assert!(Store::open(&root).is_err());
            assert_eq!(
                fs::read(folder.join("collision-sentinel")).unwrap(),
                b"unrelated"
            );
            fs::remove_dir_all(folder).unwrap();
        }
        let empty_foreign = base.join(uuid::Uuid::new_v4().simple().to_string());
        fs::create_dir(&empty_foreign).unwrap();
        assert!(Store::open(&root).is_err());
        assert!(empty_foreign.is_dir());
        fs::remove_dir(empty_foreign).unwrap();
        let partial = base.join(format!("{}.staging", uuid::Uuid::new_v4().simple()));
        fs::create_dir(&partial).unwrap();
        fs::write(partial.join("original.pdf"), &PDF[..100]).unwrap();
        let store = Store::open(&root).unwrap();
        assert!(!partial.exists());
        assert_eq!(Sha256::digest(PDF), source_before);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resource_reads_refuse_stale_document_identity_and_page_metadata() {
        let root = root();
        let mut store = Store::open(&root).unwrap();
        let id = store
            .request("POST", "/matters", json!({"name":"Identity checks"}))
            .body["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let imported = store.import_pdf(&id, "petition.pdf", PDF.to_vec());
        assert_eq!(imported.status, 201);
        let doc_id = imported.body[0]["id"].as_str().unwrap().to_owned();
        let raw: String = store
            .con
            .query_row("SELECT body FROM documents WHERE id=?", [&doc_id], |row| {
                row.get(0)
            })
            .unwrap();
        for (key, stale) in [
            ("id", json!(uuid::Uuid::new_v4().simple().to_string())),
            ("matter_id", json!("other")),
            ("page_count", json!(4)),
            ("archived_at", json!("2026-11-18T00:00:00Z")),
            ("certificate_signature_status", json!("verified")),
            ("certificate_signature_count", json!(1)),
            ("signature_status", json!("verified")),
        ] {
            let mut changed: Value = serde_json::from_str(&raw).unwrap();
            changed[key] = stale;
            store
                .con
                .execute(
                    "UPDATE documents SET body=? WHERE id=?",
                    params![changed.to_string(), doc_id],
                )
                .unwrap();
            assert_eq!(
                store
                    .request("GET", &format!("/matters/{id}/documents"), Value::Null)
                    .status,
                409,
                "{key}"
            );
            assert_eq!(
                store
                    .request("GET", &format!("/matters/{id}/storage"), Value::Null)
                    .status,
                409,
                "{key}"
            );
        }
        let mut changed: Value = serde_json::from_str(&raw).unwrap();
        changed["source_sha256"] = json!("changed");
        store
            .con
            .execute(
                "UPDATE documents SET body=?,source_sha256='changed' WHERE id=?",
                params![changed.to_string(), doc_id],
            )
            .unwrap();
        assert_eq!(
            store
                .request("GET", &format!("/matters/{id}/documents"), Value::Null)
                .status,
            409
        );
        store
            .con
            .execute(
                "UPDATE documents SET body=?,source_sha256=?,archived_at='2026-11-18T00:00:00Z' WHERE id=?",
                params![raw, PILOT_PDF_SHA256, doc_id],
            )
            .unwrap();
        assert_eq!(
            store
                .request(
                    "GET",
                    &format!("/matters/{id}/archived-documents"),
                    Value::Null
                )
                .status,
            409
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pinned_fixture_refuses_all_other_pdf_bytes_before_parser_or_write() {
        let root = root();
        let mut store = Store::open(&root).unwrap();
        let id = store
            .request("POST", "/matters", json!({"name":"Refusal"}))
            .body["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let source_hash = Sha256::digest(PDF);
        let variants: &[(&str, &[u8])] = &[
            (
                "ordinary-signature",
                b"/AcroForm << /Fields [ /FT /Sig /ByteRange [0 1 2 3] ] >>",
            ),
            (
                "incremental-signature",
                b"\n1 0 obj << /Perms << /DocMDP 2 0 R >> >> endobj\n%%EOF",
            ),
            ("escaped-name", b"/Byte#52ange [0 1 2 3]"),
            ("object-stream", b"/ObjStm /Sig"),
            ("encrypted", b"/Encrypt 4 0 R"),
            ("visible-mark-only", b"/Subtype /Stamp"),
            ("malformed", b"truncated"),
        ];
        for (name, suffix) in variants {
            let mut bytes = PDF.to_vec();
            bytes.extend_from_slice(suffix);
            let original = bytes.clone();
            let result = store.import_pdf(&id, &format!("{name}.pdf"), bytes);
            assert_eq!(result.status, 422, "{name}: {:?}", result.body);
            assert!(result.body["detail"]
                .as_str()
                .unwrap()
                .contains("exact samples/01_petition.pdf"));
            assert_eq!(original, [PDF, *suffix].concat());
        }
        let mut near_limit = PDF.to_vec();
        near_limit.resize(4 * 1024 * 1024, b' ');
        assert_eq!(store.import_pdf(&id, "near.pdf", near_limit).status, 422);
        let mut over_limit = PDF.to_vec();
        over_limit.resize(4 * 1024 * 1024 + 1, b' ');
        assert_eq!(store.import_pdf(&id, "over.pdf", over_limit).status, 422);
        assert_eq!(Sha256::digest(PDF), source_hash);
        assert_eq!(
            store
                .request("GET", &format!("/matters/{id}/documents"), Value::Null)
                .body,
            json!([])
        );
        assert_eq!(fs::read_dir(root.join("documents-v1")).unwrap().count(), 0);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn independently_checked_pdf_corpus_stays_outside_pinned_admission() {
        let root = root();
        let mut store = Store::open(&root).unwrap();
        let id = store
            .request("POST", "/matters", json!({"name":"Real corpus refusal"}))
            .body["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let corpus: &[(&str, &[u8])] = &[
            (
                "certificate",
                include_bytes!("../../samples/rust_pdf_corpus/certificate.pdf"),
            ),
            (
                "incremental",
                include_bytes!("../../samples/rust_pdf_corpus/incremental_certificate.pdf"),
            ),
            (
                "escaped",
                include_bytes!("../../samples/rust_pdf_corpus/escaped_name_certificate.pdf"),
            ),
            (
                "perms",
                include_bytes!("../../samples/rust_pdf_corpus/perms_certificate.pdf"),
            ),
            (
                "object-stream-signed",
                include_bytes!("../../samples/rust_pdf_corpus/object_stream_certificate.pdf"),
            ),
            (
                "encrypted",
                include_bytes!("../../samples/rust_pdf_corpus/encrypted.pdf"),
            ),
            (
                "unsigned",
                include_bytes!("../../samples/rust_pdf_corpus/unsigned.pdf"),
            ),
            (
                "unsigned-form",
                include_bytes!("../../samples/rust_pdf_corpus/unsigned_acroform.pdf"),
            ),
            (
                "unsigned-perms",
                include_bytes!("../../samples/rust_pdf_corpus/unsigned_perms.pdf"),
            ),
            (
                "object-stream-unsigned",
                include_bytes!("../../samples/rust_pdf_corpus/object_stream_unsigned.pdf"),
            ),
            (
                "visible-mark",
                include_bytes!("../../samples/rust_pdf_corpus/visible_mark_only.pdf"),
            ),
            (
                "malformed",
                include_bytes!("../../samples/rust_pdf_corpus/malformed.pdf"),
            ),
        ];
        for (name, bytes) in corpus {
            let response = store.import_pdf(&id, &format!("{name}.pdf"), bytes.to_vec());
            assert_eq!(response.status, 422, "{name}: {:?}", response.body);
        }
        assert_eq!(
            store
                .request("GET", &format!("/matters/{id}/documents"), Value::Null)
                .body,
            json!([])
        );
        assert_eq!(fs::read_dir(root.join("documents-v1")).unwrap().count(), 0);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore]
    fn pdf_import_crash_child() {
        let root = PathBuf::from(std::env::var("MATTERDRAFT_TEST_CRASH_ROOT").unwrap());
        let id = std::env::var("MATTERDRAFT_TEST_CRASH_MATTER").unwrap();
        let mut store = Store::open(&root).unwrap();
        let _ = store.import_pdf(&id, "petition.pdf", PDF.to_vec());
        panic!("crash phase was not reached");
    }

    #[test]
    #[ignore]
    fn pdf_import_hold_child() {
        let root = PathBuf::from(std::env::var("MATTERDRAFT_TEST_CRASH_ROOT").unwrap());
        let id = std::env::var("MATTERDRAFT_TEST_CRASH_MATTER").unwrap();
        let mut store = Store::open(&root).unwrap();
        assert_eq!(
            store.import_pdf(&id, "petition.pdf", PDF.to_vec()).status,
            201
        );
    }

    #[test]
    #[ignore]
    fn pdf_import_archived_matter_child() {
        let root = PathBuf::from(std::env::var("MATTERDRAFT_TEST_CRASH_ROOT").unwrap());
        let id = std::env::var("MATTERDRAFT_TEST_CRASH_MATTER").unwrap();
        let mut store = Store::open(&root).unwrap();
        assert_eq!(
            store.import_pdf(&id, "petition.pdf", PDF.to_vec()).status,
            404
        );
    }

    #[test]
    #[ignore]
    fn pdf_import_file_failure_child() {
        let root = PathBuf::from(std::env::var("MATTERDRAFT_TEST_CRASH_ROOT").unwrap());
        let id = std::env::var("MATTERDRAFT_TEST_CRASH_MATTER").unwrap();
        let mut store = Store::open(&root).unwrap();
        assert_eq!(
            store.import_pdf(&id, "petition.pdf", PDF.to_vec()).status,
            500
        );
    }

    #[test]
    fn import_file_failures_leave_no_record_or_orphan() {
        for phase in [
            "before_first_sync",
            "before_second_sync",
            "before_stage_directory_sync",
            "before_parent_directory_sync",
            "before_readback",
        ] {
            let root = root();
            let mut store = Store::open(&root).unwrap();
            let id = store
                .request("POST", "/matters", json!({"name":"File fault"}))
                .body["id"]
                .as_str()
                .unwrap()
                .to_owned();
            drop(store);
            let source_before = Sha256::digest(PDF);
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "matters::tests::pdf_import_file_failure_child",
                    "--nocapture",
                ])
                .env("MATTERDRAFT_TEST_CRASH_ROOT", &root)
                .env("MATTERDRAFT_TEST_CRASH_MATTER", &id)
                .env("MATTERDRAFT_TEST_FAIL_PHASE", phase)
                .status()
                .unwrap();
            assert!(status.success(), "{phase}");
            let mut store = Store::open(&root).unwrap();
            assert_eq!(
                store
                    .request("GET", &format!("/matters/{id}/documents"), Value::Null)
                    .body,
                json!([]),
                "{phase}"
            );
            assert_eq!(
                fs::read_dir(root.join("documents-v1")).unwrap().count(),
                0,
                "{phase}"
            );
            assert_eq!(Sha256::digest(PDF), source_before, "{phase}");
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn import_rechecks_active_matter_under_writer_lock() {
        let root = root();
        let mut store = Store::open(&root).unwrap();
        let id = store
            .request("POST", "/matters", json!({"name":"Archive race"}))
            .body["id"]
            .as_str()
            .unwrap()
            .to_owned();
        drop(store);
        let ready = root.with_extension("ready");
        let source_before = Sha256::digest(PDF);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "matters::tests::pdf_import_archived_matter_child",
                "--nocapture",
            ])
            .env("MATTERDRAFT_TEST_CRASH_ROOT", &root)
            .env("MATTERDRAFT_TEST_CRASH_MATTER", &id)
            .env("MATTERDRAFT_TEST_HOLD_PHASE", "before_writer_lock")
            .env("MATTERDRAFT_TEST_READY_FILE", &ready)
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ready.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "import did not reach race point"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let mut other = Store::open(&root).unwrap();
        assert_eq!(
            other
                .request("DELETE", &format!("/matters/{id}"), Value::Null)
                .status,
            200
        );
        assert!(child.wait().unwrap().success());
        assert_eq!(
            other
                .con
                .query_row("SELECT count(*) FROM documents", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(fs::read_dir(root.join("documents-v1")).unwrap().count(), 0);
        assert_eq!(Sha256::digest(PDF), source_before);
        drop(other);
        fs::remove_file(ready).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn second_process_waits_for_active_writer_before_reconciliation() {
        let root = root();
        let mut store = Store::open(&root).unwrap();
        let id = store
            .request("POST", "/matters", json!({"name":"Active writer"}))
            .body["id"]
            .as_str()
            .unwrap()
            .to_owned();
        drop(store);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "matters::tests::pdf_import_hold_child",
                "--nocapture",
            ])
            .env("MATTERDRAFT_TEST_CRASH_ROOT", &root)
            .env("MATTERDRAFT_TEST_CRASH_MATTER", &id)
            .env("MATTERDRAFT_TEST_HOLD_PHASE", "first_sync")
            .spawn()
            .unwrap();
        let base = root.join("documents-v1");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while fs::read_dir(&base).unwrap().count() == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "writer did not stage a folder"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let reopened = Store::open(&root).unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(
            reopened
                .con
                .query_row("SELECT count(*) FROM documents", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(fs::read_dir(&base).unwrap().count(), 1);
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn import_process_termination_reconciles_every_publication_phase() {
        for phase in [
            "staging",
            "first_sync",
            "second_sync",
            "rename",
            "publication",
            "committed",
        ] {
            let source_path =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../samples/01_petition.pdf");
            let source_before = fs::read(&source_path).unwrap();
            let root = root();
            let mut store = Store::open(&root).unwrap();
            let id = store
                .request("POST", "/matters", json!({"name":"Crash fixture"}))
                .body["id"]
                .as_str()
                .unwrap()
                .to_owned();
            drop(store);
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "matters::tests::pdf_import_crash_child",
                    "--nocapture",
                ])
                .env("MATTERDRAFT_TEST_CRASH_ROOT", &root)
                .env("MATTERDRAFT_TEST_CRASH_MATTER", &id)
                .env("MATTERDRAFT_TEST_CRASH_PHASE", phase)
                .output()
                .unwrap();
            assert_eq!(status.status.code(), Some(97), "{phase}: {:?}", status);
            let mut store = Store::open(&root).unwrap();
            let docs = store.request("GET", &format!("/matters/{id}/documents"), Value::Null);
            assert_eq!(docs.status, 200, "{phase}: {:?}", docs.body);
            let count = docs.body.as_array().unwrap().len();
            assert_eq!(count, usize::from(phase == "committed"), "{phase}");
            assert_eq!(
                fs::read_dir(root.join("documents-v1")).unwrap().count(),
                count,
                "{phase}"
            );
            let history = store.request("GET", &format!("/matters/{id}/history"), Value::Null);
            let audit = store.request("GET", &format!("/matters/{id}/audit"), Value::Null);
            assert_eq!(
                history.body["undo_count"].as_u64().unwrap(),
                count as u64,
                "{phase}"
            );
            assert_eq!(audit.body.as_array().unwrap().len(), count + 1, "{phase}");
            if count == 1 {
                let doc = &docs.body[0];
                assert_eq!(doc["source_sha256"], PILOT_PDF_SHA256);
                let folder = store.document_folder(doc["id"].as_str().unwrap());
                assert_eq!(fs::read(folder.join("original.pdf")).unwrap(), PDF);
                assert_eq!(fs::read(folder.join("revision-1.pdf")).unwrap(), PDF);
            }
            assert_eq!(
                fs::read(&source_path).unwrap(),
                source_before,
                "{phase}: source changed"
            );
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn rollback_restart_and_history() {
        let root = root();
        let mut s = Store::open(&root).unwrap();
        let created = s.request("POST", "/matters", json!({"name":"Synthetic"}));
        assert_eq!(created.status, 201);
        let path = format!("/matters/{}", created.body["id"].as_str().unwrap());
        s.con.execute_batch("CREATE TRIGGER fail_audit BEFORE INSERT ON audit_events BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert_eq!(
            s.request("PATCH", &path, json!({"name":"Rollback"})).status,
            500
        );
        assert_eq!(s.request("GET", &path, Value::Null).body, created.body);
        assert_eq!(
            s.con
                .query_row("SELECT count(*) FROM edit_history", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        s.con.execute_batch("DROP TRIGGER fail_audit").unwrap();
        let changed = s.request("PATCH", &path, json!({"notes":"Persisted"}));
        assert_eq!(changed.status, 200);
        drop(s);
        let mut s = Store::open(&root).unwrap();
        assert_eq!(s.request("GET", &path, Value::Null).body, changed.body);
        assert_eq!(
            s.request("PATCH", &path, json!({"notes":"Persisted"})).body,
            changed.body
        );
        assert_eq!(
            s.con
                .query_row("SELECT count(*) FROM edit_history", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(s.request("DELETE", &path, Value::Null).status, 200);
        assert_eq!(
            s.request("GET", "/matters/missing", Value::Null).status,
            404
        );
        drop(s);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn create_failure_and_corrupt_read_are_safe() {
        let root = root();
        let mut s = Store::open(&root).unwrap();
        s.con.execute_batch("CREATE TRIGGER fail_create BEFORE INSERT ON audit_events BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert_eq!(
            s.request("POST", "/matters", json!({"name":"Rollback create"}))
                .status,
            500
        );
        assert_eq!(s.request("GET", "/matters", Value::Null).body, json!([]));
        s.con.execute_batch("DROP TRIGGER fail_create; INSERT INTO matters VALUES ('broken','invalid JSON','2026');").unwrap();
        assert_eq!(s.request("GET", "/matters", Value::Null).status, 500);
        assert_eq!(s.request("GET", "/matters/broken", Value::Null).status, 500);
        drop(s);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn history_retention_and_branch_clearing() {
        let root = root();
        let mut s = Store::open(&root).unwrap();
        let item = s
            .request("POST", "/matters", json!({"name":"History retention"}))
            .body;
        let path = format!("/matters/{}", item["id"].as_str().unwrap());
        for n in 0..105 {
            assert_eq!(
                s.request("PATCH", &path, json!({"notes":n.to_string()}))
                    .status,
                200
            );
        }
        assert_eq!(
            s.con
                .query_row("SELECT count(*) FROM edit_history", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            100
        );
        s.con
            .execute_batch(
                "UPDATE edit_history SET applied=0 WHERE id=(SELECT max(id) FROM edit_history)",
            )
            .unwrap();
        assert_eq!(
            s.request("PATCH", &path, json!({"notes":"New branch"}))
                .status,
            200
        );
        assert_eq!(
            s.con
                .query_row(
                    "SELECT count(*) FROM edit_history WHERE applied=0",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        drop(s);
        std::fs::remove_dir_all(root).unwrap();
    }
    fn snapshot(s: &Store) -> Vec<Vec<String>> {
        ["SELECT body FROM matters ORDER BY id", "SELECT printf('%s|%s|%s|%s|%s|%s|%s',id,matter_id,kind,before_json,after_json,created_at,applied) FROM edit_history ORDER BY id", "SELECT printf('%s|%s|%s|%s',id,matter_id,action,created_at) FROM audit_events ORDER BY id"]
            .iter().map(|sql| s.con.prepare(sql).unwrap().query_map([], |r| r.get(0)).unwrap().collect::<rusqlite::Result<Vec<String>>>().unwrap()).collect()
    }
    #[test]
    fn lifecycle_failures_are_atomic() {
        let root = root();
        let mut s = Store::open(&root).unwrap();
        let item = s
            .request("POST", "/matters", json!({"name":"Atomic lifecycle"}))
            .body;
        let path = format!("/matters/{}", item["id"].as_str().unwrap());
        s.request("PATCH", &path, json!({"notes":"edit"}));
        // Fail at history publication, matter publication and final audit publication.
        for (method, suffix) in [
            ("POST", "/history/undo"),
            ("POST", "/history/redo"),
            ("DELETE", ""),
            ("POST", "/restore"),
        ] {
            for table in ["edit_history", "matters", "audit_events"] {
                if table == "edit_history" && (suffix == "/restore" || method == "DELETE") {
                    continue;
                }
                let before = snapshot(&s);
                let verb = if table == "audit_events" {
                    "INSERT"
                } else {
                    "UPDATE"
                };
                s.con.execute_batch(&format!("CREATE TRIGGER fail_transition BEFORE {verb} ON {table} BEGIN SELECT RAISE(ABORT,'injected'); END;")).unwrap();
                assert_eq!(
                    s.request(method, &format!("{path}{suffix}"), Value::Null)
                        .status,
                    500
                );
                assert_eq!(snapshot(&s), before);
                s.con.execute_batch("DROP TRIGGER fail_transition").unwrap();
            }
            assert_eq!(
                s.request(method, &format!("{path}{suffix}"), Value::Null)
                    .status,
                200
            );
        }
        // A failed branch replacement must retain the original redo branch.
        s.request("POST", &format!("{path}/history/undo"), Value::Null);
        let before = snapshot(&s);
        s.con.execute_batch("CREATE TRIGGER fail_branch BEFORE INSERT ON edit_history BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert_eq!(
            s.request("PATCH", &path, json!({"notes":"branch"})).status,
            500
        );
        assert_eq!(snapshot(&s), before);
        drop(s);
        let s = Store::open(&root).unwrap();
        assert_eq!(snapshot(&s), before);
        drop(s);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unsupported_replay_is_atomic_and_does_not_skip_entries() {
        let root = root();
        let mut s = Store::open(&root).unwrap();
        let item = s
            .request("POST", "/matters", json!({"name":"Unsupported replay"}))
            .body;
        let id = item["id"].as_str().unwrap();
        let path = format!("/matters/{id}");
        s.request("PATCH", &path, json!({"notes":"Supported edit"}));
        for (kind, state) in [
            ("document", json!({"fields":{}})),
            ("matter_parts", json!({"parts":[1]})),
            ("unknown", json!({})),
            ("matter", json!({"archived_at":"invalid"})),
        ] {
            for applied in [0, 1] {
                s.con.execute("INSERT INTO edit_history (matter_id,kind,before_json,after_json,created_at,applied) VALUES (?,?,?,?,?,?)",params![id,kind,state.to_string(),state.to_string(),now(),applied]).unwrap();
                let before = snapshot(&s);
                let direction = if applied == 1 { "undo" } else { "redo" };
                assert_eq!(
                    s.request("POST", &format!("{path}/history/{direction}"), Value::Null)
                        .status,
                    409
                );
                assert_eq!(snapshot(&s), before);
                s.con
                    .execute(
                        "DELETE FROM edit_history WHERE id=(SELECT max(id) FROM edit_history)",
                        [],
                    )
                    .unwrap();
            }
        }
        drop(s);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn schema_two_retained_records_and_tied_archive_order() {
        let root = root();
        let mut s = Store::open(&root).unwrap();
        let first = s
            .request("POST", "/matters", json!({"name":"First existing"}))
            .body;
        let second = s
            .request("POST", "/matters", json!({"name":"Second existing"}))
            .body;
        let path = format!("/matters/{}", first["id"].as_str().unwrap());
        s.request("PATCH", &path, json!({"notes":"Retained edit"}));
        s.con
            .execute_batch(
                "DROP TABLE reader_positions; DROP TABLE matter_preparation; DROP TABLE bundle_membership_state; DROP TABLE bundle_destinations; DROP TABLE bundle_settings; DROP TABLE output_presets; DROP TABLE preflight_runs; DROP TABLE preflight_reviews; DROP TABLE preflight_notes; DROP TABLE rust_annotations; DROP TABLE rust_annotation_assets; DROP TABLE rust_processing_jobs; DROP TABLE rust_exports; DROP TABLE rust_export_files; DROP TABLE rust_export_checks; DROP TABLE rust_export_reviews; DROP TABLE rust_ocr_reviews; DROP TABLE rust_page_screenings; DROP TABLE rust_compositions; DROP TABLE rust_source_pages; DROP TABLE rust_redactions; DROP TABLE rust_frozen_builds; DROP TABLE rust_frozen_sources; PRAGMA user_version=2",
            )
            .unwrap();
        drop(s);
        let mut s = Store::open(&root).unwrap();
        assert_eq!(
            s.con
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        assert_eq!(
            s.request("POST", &format!("{path}/history/undo"), Value::Null)
                .status,
            200
        );
        assert_eq!(s.request("GET", &path, Value::Null).body["notes"], "");
        s.con.execute("UPDATE matters SET body=json_set(body,'$.archived_at','2026-11-17T00:00:00.000000+00:00')",[]).unwrap();
        let rows = s.request("GET", "/matters/archived", Value::Null).body;
        assert_eq!(rows[0]["id"], first["id"]);
        assert_eq!(rows[1]["id"], second["id"]);
        drop(s);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn document_archive_restore_presence_replay_and_restart() {
        let root = root();
        let mut s = Store::open(&root).unwrap();
        let matter = s
            .request("POST", "/matters", json!({"name":"Presence lifecycle"}))
            .body;
        let mid = matter["id"].as_str().unwrap();
        let doc = s.import_pdf(mid, "petition.pdf", PDF.to_vec()).body[0].clone();
        let did = doc["id"].as_str().unwrap();
        let folder = s.document_folder(did);
        let history = format!("/matters/{mid}/history");
        let active = format!("/matters/{mid}/documents");
        let archived = format!("/matters/{mid}/archived-documents");
        assert_eq!(
            s.request("DELETE", &format!("/documents/{did}"), Value::Null)
                .status,
            200
        );
        assert_eq!(
            s.request("DELETE", &format!("/documents/{did}"), Value::Null)
                .status,
            404
        );
        assert_eq!(s.request("GET", &active, Value::Null).body, json!([]));
        assert_eq!(s.request("GET", &archived, Value::Null).body[0]["id"], did);
        assert_eq!(
            s.request("GET", &history, Value::Null).body["undo_label"],
            "Archive document"
        );
        drop(s);
        let mut s = Store::open(&root).unwrap();
        let working = folder.join("revision-1.pdf");
        let missing = folder.join("revision-1.missing");
        fs::rename(&working, &missing).unwrap();
        let restore = format!("/matters/{mid}/archived-documents/{did}/restore");
        assert_eq!(s.request("POST", &restore, Value::Null).status, 409);
        assert_eq!(
            s.request("GET", &history, Value::Null).body["undo_count"],
            2
        );
        fs::rename(&missing, &working).unwrap();
        assert_eq!(s.request("POST", &restore, Value::Null).status, 200);
        assert_eq!(s.request("POST", &restore, Value::Null).status, 404);
        for (direction, is_archived) in [
            ("undo", true),
            ("undo", false),
            ("undo", true),
            ("redo", false),
            ("redo", true),
            ("redo", false),
        ] {
            assert_eq!(
                s.request("POST", &format!("{history}/{direction}"), Value::Null)
                    .status,
                200
            );
            assert_eq!(
                s.request("GET", &archived, Value::Null)
                    .body
                    .as_array()
                    .unwrap()
                    .len(),
                usize::from(is_archived)
            );
        }
        assert_eq!(
            s.request("POST", &format!("{history}/redo"), Value::Null)
                .status,
            409
        );
        assert_eq!(fs::read(folder.join("original.pdf")).unwrap(), PDF);
        assert_eq!(fs::read(&working).unwrap(), PDF);
        drop(s);
        let mut s = Store::open(&root).unwrap();
        assert_eq!(s.request("GET", &active, Value::Null).body[0]["id"], did);
        assert_eq!(
            s.request("GET", &history, Value::Null).body["undo_count"],
            3
        );
        drop(s);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn document_replay_refuses_corrupt_history_and_stale_revision_atomically() {
        let root = root();
        let mut store = Store::open(&root).unwrap();
        let matter = store
            .request("POST", "/matters", json!({"name":"Corrupt presence guard"}))
            .body;
        let mid = matter["id"].as_str().unwrap();
        let doc = store.import_pdf(mid, "petition.pdf", PDF.to_vec()).body[0].clone();
        let did = doc["id"].as_str().unwrap();
        assert_eq!(
            store
                .request("DELETE", &format!("/documents/{did}"), Value::Null)
                .status,
            200
        );
        let latest: (i64, String) = store
            .con
            .query_row(
                "SELECT id,after_json FROM edit_history WHERE matter_id=? ORDER BY id DESC LIMIT 1",
                [mid],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let audit_count: i64 = store
            .con
            .query_row(
                "SELECT count(*) FROM audit_events WHERE matter_id=?",
                [mid],
                |row| row.get(0),
            )
            .unwrap();
        store
            .con
            .execute(
                "UPDATE edit_history SET after_json='not-json' WHERE id=?",
                [latest.0],
            )
            .unwrap();
        let undo = format!("/matters/{mid}/history/undo");
        assert_eq!(store.request("POST", &undo, Value::Null).status, 500);
        store
            .con
            .execute(
                "UPDATE edit_history SET after_json=? WHERE id=?",
                params![latest.1, latest.0],
            )
            .unwrap();
        store
            .con
            .execute(
                "UPDATE document_revisions SET sha256='stale' WHERE document_id=?",
                [did],
            )
            .unwrap();
        assert_eq!(store.request("POST", &undo, Value::Null).status, 409);
        let state: (Option<String>, i64) = store
            .con
            .query_row(
                "SELECT archived_at,json_extract(body,'$.position') FROM documents WHERE id=?",
                [did],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(state.0.is_some());
        assert_eq!(state.1, 0);
        assert_eq!(
            store
                .con
                .query_row(
                    "SELECT applied FROM edit_history WHERE id=?",
                    [latest.0],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .con
                .query_row(
                    "SELECT count(*) FROM audit_events WHERE matter_id=?",
                    [mid],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            audit_count
        );
        store
            .con
            .execute(
                "UPDATE document_revisions SET sha256=? WHERE document_id=?",
                params![PILOT_PDF_SHA256, did],
            )
            .unwrap();
        assert_eq!(store.request("POST", &undo, Value::Null).status, 200);
        assert_eq!(
            fs::read(store.document_folder(&did).join("original.pdf")).unwrap(),
            PDF
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn document_restore_checks_group_parent_and_appends_position() {
        let root = root();
        let mut store = Store::open(&root).unwrap();
        let matter = store
            .request("POST", "/matters", json!({"name":"Group restore guard"}))
            .body;
        let mid = matter["id"].as_str().unwrap();
        let doc = store.import_pdf(mid, "petition.pdf", PDF.to_vec()).body[0].clone();
        let did = doc["id"].as_str().unwrap().to_owned();
        let mut parent = doc;
        parent["group_key"] = json!("Bundle A");
        parent["group_role"] = json!("parent");
        store
            .con
            .execute(
                "UPDATE documents SET body=? WHERE id=?",
                params![parent.to_string(), did],
            )
            .unwrap();
        assert_eq!(
            store
                .request("DELETE", &format!("/documents/{did}"), Value::Null)
                .status,
            200
        );
        // Exact-byte admission permits only one real source per matter. This SQL-only
        // row isolates the group/position rule without admitting another PDF class.
        let other = uuid::Uuid::new_v4().simple().to_string();
        let sibling = json!({"position":5,"group_key":"Bundle A","group_role":"parent"});
        store.con.execute("INSERT INTO documents (id,matter_id,body,source_sha256,original_relpath,working_relpath,created_at,archived_at) VALUES (?,?,?,?,?,?,?,NULL)",
            params![other,mid,sibling.to_string(),"synthetic-other",format!("documents-v1/{other}/original.pdf"),format!("documents-v1/{other}/revision-1.pdf"),now()]).unwrap();
        let restore = format!("/matters/{mid}/archived-documents/{did}/restore");
        assert_eq!(store.request("POST", &restore, Value::Null).status, 422);
        let archived: Option<String> = store
            .con
            .query_row(
                "SELECT archived_at FROM documents WHERE id=?",
                [&did],
                |r| r.get(0),
            )
            .unwrap();
        assert!(archived.is_some());
        store
            .con
            .execute(
                "UPDATE documents SET body=json_set(body,'$.group_role','member') WHERE id=?",
                [&other],
            )
            .unwrap();
        assert_eq!(store.request("POST", &restore, Value::Null).status, 200);
        let position: i64 = store
            .con
            .query_row(
                "SELECT json_extract(body,'$.position') FROM documents WHERE id=?",
                [&did],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(position, 6);
        assert_eq!(
            fs::read(store.document_folder(&did).join("original.pdf")).unwrap(),
            PDF
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn document_transition_failures_roll_back_state_history_and_audit() {
        let root = root();
        let mut s = Store::open(&root).unwrap();
        let mid = s
            .request("POST", "/matters", json!({"name":"Atomic presence"}))
            .body["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let did = s.import_pdf(&mid, "petition.pdf", PDF.to_vec()).body[0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let path = format!("/documents/{did}");
        let counts = |s: &Store| -> (i64, i64) {
            (
                s.con
                    .query_row(
                        "SELECT count(*) FROM edit_history WHERE matter_id=?",
                        [&mid],
                        |r| r.get(0),
                    )
                    .unwrap(),
                s.con
                    .query_row(
                        "SELECT count(*) FROM audit_events WHERE matter_id=?",
                        [&mid],
                        |r| r.get(0),
                    )
                    .unwrap(),
            )
        };
        let before = counts(&s);
        s.con.execute_batch("CREATE TRIGGER fail_document_archive BEFORE INSERT ON audit_events WHEN NEW.action='document_archived' BEGIN SELECT RAISE(ABORT,'injected archive failure'); END;").unwrap();
        assert_eq!(s.request("DELETE", &path, Value::Null).status, 500);
        assert_eq!(counts(&s), before);
        assert_eq!(
            s.request("GET", &format!("/matters/{mid}/documents"), Value::Null)
                .body
                .as_array()
                .unwrap()
                .len(),
            1
        );
        s.con
            .execute_batch("DROP TRIGGER fail_document_archive")
            .unwrap();
        assert_eq!(s.request("DELETE", &path, Value::Null).status, 200);
        let archived_counts = counts(&s);
        let restore = format!("/matters/{mid}/archived-documents/{did}/restore");
        s.con.execute_batch("CREATE TRIGGER fail_document_restore BEFORE INSERT ON audit_events WHEN NEW.action='document_restored' BEGIN SELECT RAISE(ABORT,'injected restore failure'); END;").unwrap();
        assert_eq!(s.request("POST", &restore, Value::Null).status, 500);
        assert_eq!(counts(&s), archived_counts);
        assert_eq!(
            s.request(
                "GET",
                &format!("/matters/{mid}/archived-documents"),
                Value::Null
            )
            .body
            .as_array()
            .unwrap()
            .len(),
            1
        );
        s.con
            .execute_batch("DROP TRIGGER fail_document_restore")
            .unwrap();
        let folder = s.document_folder(&did);
        let mut changed = PDF.to_vec();
        changed[200] ^= 1;
        fs::write(folder.join("revision-1.pdf"), &changed).unwrap();
        assert_eq!(s.request("POST", &restore, Value::Null).status, 409);
        assert_eq!(counts(&s), archived_counts);
        fs::write(folder.join("revision-1.pdf"), PDF).unwrap();
        assert_eq!(s.request("POST", &restore, Value::Null).status, 200);
        assert_eq!(fs::read(folder.join("original.pdf")).unwrap(), PDF);
        drop(s);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore]
    fn document_presence_crash_child() {
        let root = std::env::var("MATTERDRAFT_TEST_CRASH_ROOT").unwrap();
        let mid = std::env::var("MATTERDRAFT_TEST_CRASH_MATTER").unwrap();
        let did = std::env::var("MATTERDRAFT_TEST_CRASH_DOCUMENT").unwrap();
        let operation = std::env::var("MATTERDRAFT_TEST_PRESENCE_OPERATION").unwrap();
        let mut s = Store::open(Path::new(&root)).unwrap();
        let path = match operation.as_str() {
            "archive" => format!("/documents/{did}"),
            "restore" => format!("/matters/{mid}/archived-documents/{did}/restore"),
            "replay" => format!("/matters/{mid}/history/undo"),
            _ => panic!("unknown fixture operation"),
        };
        s.request(
            if operation == "archive" {
                "DELETE"
            } else {
                "POST"
            },
            &path,
            Value::Null,
        );
    }
    #[test]
    fn document_presence_process_exit_is_zero_or_one_complete_transition() {
        for (operation, before_commit, after_commit) in [
            (
                "archive",
                "document_archive_publication",
                "document_archive_committed",
            ),
            (
                "restore",
                "document_restore_publication",
                "document_restore_committed",
            ),
            (
                "replay",
                "presence_replay_publication",
                "presence_replay_committed",
            ),
        ] {
            for phase in [before_commit, after_commit] {
                let root = root();
                let mut s = Store::open(&root).unwrap();
                let mid = s
                    .request("POST", "/matters", json!({"name":"Crash presence"}))
                    .body["id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                let did = s.import_pdf(&mid, "petition.pdf", PDF.to_vec()).body[0]["id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                if operation == "restore" {
                    assert_eq!(
                        s.request("DELETE", &format!("/documents/{did}"), Value::Null)
                            .status,
                        200
                    );
                }
                let audit_before: i64 = s
                    .con
                    .query_row(
                        "SELECT count(*) FROM audit_events WHERE matter_id=?",
                        [&mid],
                        |r| r.get(0),
                    )
                    .unwrap();
                let history_before: i64 = s
                    .con
                    .query_row(
                        "SELECT count(*) FROM edit_history WHERE matter_id=?",
                        [&mid],
                        |r| r.get(0),
                    )
                    .unwrap();
                drop(s);
                let status = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--ignored",
                        "--exact",
                        "matters::tests::document_presence_crash_child",
                        "--nocapture",
                    ])
                    .env("MATTERDRAFT_TEST_CRASH_ROOT", &root)
                    .env("MATTERDRAFT_TEST_CRASH_MATTER", &mid)
                    .env("MATTERDRAFT_TEST_CRASH_DOCUMENT", &did)
                    .env("MATTERDRAFT_TEST_PRESENCE_OPERATION", operation)
                    .env("MATTERDRAFT_TEST_CRASH_PHASE", phase)
                    .status()
                    .unwrap();
                assert_eq!(status.code(), Some(97), "{operation} {phase}");
                let mut s = Store::open(&root).unwrap();
                let committed = phase == after_commit;
                let archived = if operation == "restore" {
                    !committed
                } else {
                    committed
                };
                let archived_rows = s
                    .request(
                        "GET",
                        &format!("/matters/{mid}/archived-documents"),
                        Value::Null,
                    )
                    .body;
                assert_eq!(
                    archived_rows.as_array().unwrap().len(),
                    usize::from(archived),
                    "{operation} {phase}"
                );
                assert_eq!(
                    s.con
                        .query_row(
                            "SELECT count(*) FROM audit_events WHERE matter_id=?",
                            [&mid],
                            |r| r.get::<_, i64>(0)
                        )
                        .unwrap(),
                    audit_before + i64::from(committed)
                );
                assert_eq!(
                    s.con
                        .query_row(
                            "SELECT count(*) FROM edit_history WHERE matter_id=?",
                            [&mid],
                            |r| r.get::<_, i64>(0)
                        )
                        .unwrap(),
                    history_before
                        + if committed && operation != "replay" {
                            1
                        } else {
                            0
                        }
                );
                assert_eq!(
                    fs::read(s.document_folder(&did).join("original.pdf")).unwrap(),
                    PDF
                );
                assert_eq!(
                    fs::read(s.document_folder(&did).join("revision-1.pdf")).unwrap(),
                    PDF
                );
                drop(s);
                fs::remove_dir_all(root).unwrap();
            }
        }
    }
    #[test]
    fn isolation_and_future_schema() {
        let root = root();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("court_bundle.sqlite3"), b"sentinel").unwrap();
        assert!(Store::open(&root).is_err());
        assert_eq!(
            std::fs::read(root.join("court_bundle.sqlite3")).unwrap(),
            b"sentinel"
        );
        std::fs::remove_dir_all(root).unwrap();
        let root = self::root();
        let s = Store::open(&root).unwrap();
        s.con
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        drop(s);
        assert!(Store::open(&root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

fn integer_error(v: &Value) -> (&'static str, &'static str) {
    if v.is_string() {
        (
            "int_parsing",
            "Input should be a valid integer, unable to parse string as an integer",
        )
    } else if v.as_f64().is_some_and(|n| n.abs() >= i64::MAX as f64) {
        (
            "int_parsing_size",
            "Unable to parse input string as an integer, exceeded maximum size",
        )
    } else if v.is_number() {
        (
            "int_from_float",
            "Input should be a valid integer, got a number with a fractional part",
        )
    } else {
        ("int_type", "Input should be a valid integer")
    }
}

fn decode(s: &str) -> rusqlite::Result<Value> {
    serde_json::from_str(s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}
