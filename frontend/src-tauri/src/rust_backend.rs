//! Thin direct-IPC adapter. Workers are threads in this process, never servers/children.
use crate::BackendState;
use matterdraft_backend::{
    jobs::{JobError, Jobs},
    matters::{
        annotation_asset_id, annotation_image_document, cover_import_matter,
        original_info_document, uploaded_replacement_document, Artifact, CompositionCatalogKind,
        CompositionDraftTarget, DocumentImportTarget, DocumentOutputTarget, DocumentUploadTarget,
        ExportOutputTarget, LabelReviewTarget, MeasuredWorkingReplacement, MetadataReviewTarget,
        OutputPreviewTarget, OutputValidationTarget, PageOutput, PageOutputTarget,
        PagePreviewTarget, PdfSnapshot, PreparedDocumentImport, RawDocumentUpload,
        RawReplacementUpload, RedactionTarget, RevisionTarget, SearchTarget, Store, TextPageTarget,
        DOCUMENT_IMPORT_MAX_BYTES, READ_CHUNK_BYTES,
    },
    vault_store::{ImportOptions, VaultStore},
    Reply,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const PINNED_PDF_SIZE: u64 = 6004;
const PINNED_PDF_SHA256: &str = "76da5e34c1799a5aff3d3e9bec8cc689cb6cf8430dfc163f93bfc3ec286bf17a";
const MAX_PICKED_FILES: usize = 32;
const MAX_IMPORT_BATCH: usize = 20;
const MAX_OPEN_PDFS: usize = 16;
const PDF_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const JOB_UPDATE_EVENT: &str = "matterdraft-job-updated";

fn emit_job_update(app: &AppHandle, id: &str) {
    // The event is a hint. It carries neither a result nor any artifact
    // data. The frontend reads the saved state through the job route.
    let _ = app.emit_to("main", JOB_UPDATE_EVENT, serde_json::json!({ "id": id }));
}

struct OpenPdf {
    snapshot: Arc<Mutex<PdfSnapshot>>,
    last_used: Instant,
    print_fit: Option<bool>,
}

struct MeasuredPdf<M> {
    output: M,
    admission: OwnedSemaphorePermit,
    lane: OwnedSemaphorePermit,
}

struct NativeDrop {
    id: String,
    paths: Vec<PathBuf>,
    created: Instant,
}

enum Session {
    Synthetic(Store),
    Protected(VaultStore),
}

impl Session {
    fn request(
        &mut self,
        method: &str,
        path: &str,
        body: Value,
        order: &matterdraft_backend::RequestOrder,
    ) -> Reply {
        match self {
            Self::Synthetic(store) => store.request_with_order(method, path, body, order),
            Self::Protected(vault) => vault.request_with_order(method, path, body, order),
        }
    }

    fn publish_working_replacement(
        &mut self,
        measured: MeasuredWorkingReplacement,
    ) -> Result<Reply, Reply> {
        match self {
            Self::Synthetic(_) => Err(matterdraft_backend::unsupported()),
            Self::Protected(vault) => vault.complete_working_replacement(measured),
        }
    }

    fn capture_portable_backup(
        &self,
        matter: &str,
    ) -> Result<matterdraft_backend::matters::PortableBackupWork, Reply> {
        match self {
            Self::Synthetic(store) => store.capture_portable_backup(matter, None),
            Self::Protected(vault) => vault.capture_portable_backup(matter),
        }
    }

    fn complete_portable_backup<W>(
        &self,
        generated: matterdraft_backend::matters::GeneratedPortableBackup<W>,
    ) -> Result<W, Reply> {
        match self {
            Self::Synthetic(store) => store.complete_portable_backup(generated),
            Self::Protected(vault) => vault.complete_portable_backup(generated),
        }
    }

    fn capture_document_import(&self, matter: &str) -> Result<DocumentImportTarget, Reply> {
        match self {
            Self::Synthetic(store) => store.capture_document_import(matter),
            Self::Protected(vault) => vault.capture_document_import(matter),
        }
    }

    fn complete_document_import(
        &mut self,
        prepared: PreparedDocumentImport<'_>,
    ) -> Result<Reply, Reply> {
        Ok(match self {
            Self::Synthetic(store) => store.complete_document_import(prepared),
            Self::Protected(vault) => vault.complete_document_import(prepared),
        })
    }
}

struct PickedFile {
    file: File,
    filename: String,
    hash: String,
    size: u64,
    source_mtime: Option<i64>,
    purpose: PickPurpose,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PickPurpose {
    SyntheticDocument,
    Document,
    PortableBackup,
    Replacement,
}

fn reply(status: u16, detail: &str) -> Reply {
    Reply {
        status,
        body: serde_json::json!({"detail": detail}),
    }
}

// The path comes only from the native dialog. Open it once and retain the descriptor;
// JavaScript receives an opaque token, never a filesystem path or PDF bytes.
#[cfg(test)]
fn open_picked_file(path: &Path) -> Result<PickedFile, Reply> {
    open_import_file(path, false)
}

fn open_import_file(path: &Path, general: bool) -> Result<PickedFile, Reply> {
    open_scoped_file(
        path,
        if general {
            PickPurpose::Document
        } else {
            PickPurpose::SyntheticDocument
        },
    )
}

fn open_scoped_file(path: &Path, purpose: PickPurpose) -> Result<PickedFile, Reply> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| reply(422, "Invalid selected filename"))?
        .to_owned();
    let extension = filename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    let (extensions, limit, invalid, oversized): (&[&str], _, _, _) = match purpose {
        PickPurpose::SyntheticDocument => (
            &["pdf"],
            4 * 1024 * 1024,
            "Rust pilot accepts PDF only",
            "Rust pilot PDF limit is 4 MiB",
        ),
        PickPurpose::Document | PickPurpose::Replacement => (
            &["pdf", "png", "jpg", "jpeg"],
            80 * 1024 * 1024,
            "Choose a PDF, PNG or JPEG",
            "Import limit is 80 MB per file",
        ),
        PickPurpose::PortableBackup => (
            &["cbmatter"],
            matterdraft_backend::matters::PORTABLE_PACKAGE_MAX_BYTES,
            "Choose a .cbmatter backup",
            "Portable package exceeds 4 GiB",
        ),
    };
    if !extensions.contains(&extension.as_str()) {
        return Err(reply(415, invalid));
    }
    let link =
        std::fs::symlink_metadata(path).map_err(|_| reply(422, "Selected PDF is unavailable"))?;
    if !link.file_type().is_file() {
        return Err(reply(422, "Selected PDF must be a regular file"));
    }
    let size = link.len();
    if size > limit {
        return Err(reply(413, oversized));
    }
    if size == 0 {
        return Err(reply(422, "Selected file is empty"));
    }
    if purpose == PickPurpose::SyntheticDocument && size != PINNED_PDF_SIZE {
        return Err(reply(
            422,
            "Only the exact samples/01_petition.pdf synthetic fixture is allowed",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options
        .open(path)
        .map_err(|_| reply(422, "Selected PDF cannot be opened safely"))?;
    let metadata = file
        .metadata()
        .map_err(|_| reply(422, "Selected PDF is unavailable"))?;
    if !metadata.is_file() || metadata.len() != size {
        return Err(reply(422, "Selected PDF changed while opening"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if link.dev() != metadata.dev() || link.ino() != metadata.ino() {
            return Err(reply(422, "Selected PDF changed while opening"));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(reply(422, "Selected PDF must be a regular file"));
        }
    }
    let hash = hash_picked_file(&mut file, size)?;
    if purpose == PickPurpose::SyntheticDocument && hash != PINNED_PDF_SHA256 {
        return Err(reply(
            422,
            "Only the exact samples/01_petition.pdf synthetic fixture is allowed",
        ));
    }
    let source_mtime = metadata
        .modified()
        .ok()
        .and_then(|stamp| stamp.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok());
    Ok(PickedFile {
        file,
        filename,
        hash,
        size,
        source_mtime,
        purpose,
    })
}

fn hash_picked_file(file: &mut File, size: u64) -> Result<String, Reply> {
    let invalid = |_| reply(422, "Selected file cannot be read");
    file.seek(SeekFrom::Start(0)).map_err(invalid)?;
    let mut bounded = file.take(size + 1);
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0;
    loop {
        let count = bounded.read(&mut buffer).map_err(invalid)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        total += count as u64;
    }
    if total != size {
        return Err(reply(422, "Selected file changed after selection"));
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn read_picked_bytes(file: &mut File, size: u64) -> Result<Vec<u8>, Reply> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| reply(422, "Selected PDF cannot be reread"))?;
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(size + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| reply(422, "Selected PDF cannot be reread"))?;
    if bytes.len() as u64 != size {
        return Err(reply(422, "Selected PDF changed after selection"));
    }
    Ok(bytes)
}

pub(crate) struct RustRuntime {
    root: PathBuf,
    store: Arc<Mutex<Option<Session>>>,
    protected: bool,
    unlocked: Arc<AtomicBool>,
    // Bound outstanding requests before waiting for the single database lane.
    admission: Arc<Semaphore>,
    database_lane: Arc<Semaphore>,
    pdf_lane: Arc<Semaphore>,
    jobs: Jobs,
    picked: Arc<Mutex<HashMap<String, PickedFile>>>,
    native_drop: Mutex<Option<NativeDrop>>,
    open_pdfs: Mutex<HashMap<String, OpenPdf>>,
    artifact_admission: Arc<Semaphore>,
    ocr_review_admission: Arc<Semaphore>,
    ocr_review_lane: Arc<Semaphore>,
    processing_lane: Arc<Semaphore>,
    processing_wake: AtomicBool,
    processing_shutdown: AtomicU8,
}

enum ProcessingTarget {
    Build {
        matter: String,
        kind: &'static str,
        plan: Option<String>,
    },
    Retry {
        job: String,
    },
}

fn processing_target(path: &str) -> Option<ProcessingTarget> {
    let route = path
        .split('?')
        .next()?
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    let (matter, kind, plan) = match route.as_slice() {
        ["matters", matter, "build"] => (matter, "build", None),
        ["matters", matter, "variants", plan, "build"] => (matter, "variant-build", Some(*plan)),
        ["matters", matter, "supplements", plan, "build"] => {
            (matter, "supplement-build", Some(*plan))
        }
        ["jobs", job, "retry"] => return Some(ProcessingTarget::Retry { job: (*job).into() }),
        _ => return None,
    };
    Some(ProcessingTarget::Build {
        matter: (*matter).into(),
        kind,
        plan: plan.map(str::to_owned),
    })
}

fn processing_submission(route: &str) -> bool {
    processing_target(route).is_some()
        || matches!(
            route
                .trim_start_matches('/')
                .split('/')
                .collect::<Vec<_>>()
                .as_slice(),
            ["matters", _, "preflight", "screen"]
                | ["documents", _, "ocr" | "ocr-rerun" | "compress"]
        )
}

fn matter_resource<'a>(path: &'a str, resource: &str) -> Option<&'a str> {
    path.split('?')
        .next()?
        .trim_start_matches('/')
        .strip_prefix("matters/")?
        .split_once('/')
        .and_then(|(matter, tail)| (tail == resource).then_some(matter))
}

fn variant_review_target(path: &str) -> Option<(&str, &str)> {
    let (matter, tail) = path
        .split('?')
        .next()?
        .trim_start_matches('/')
        .strip_prefix("matters/")?
        .split_once('/')?;
    let plan = tail.strip_prefix("variants/")?.strip_suffix("/review")?;
    (!plan.contains('/')).then_some((matter, plan))
}

fn catalog_target(path: &str) -> Option<(&str, CompositionCatalogKind)> {
    matter_resource(path, "variants")
        .map(|matter| (matter, CompositionCatalogKind::Variants))
        .or_else(|| {
            matter_resource(path, "supplements")
                .map(|matter| (matter, CompositionCatalogKind::Supplements))
        })
}

fn comparison_target(path: &str) -> Option<(&str, &str, &'static str)> {
    let (matter, tail) = path
        .split('?')
        .next()?
        .trim_start_matches('/')
        .strip_prefix("matters/")?
        .split_once('/')?;
    let (build, operation) = tail.strip_prefix("exports/")?.split_once('/')?;
    match operation {
        "compare" => Some((matter, build, "compare")),
        "compare-pages" => Some((matter, build, "compare-pages")),
        _ => None,
    }
}

impl RustRuntime {
    pub(crate) fn shutdown(&self) {
        self.jobs.shutdown();
    }

    pub(crate) fn request_shutdown(
        &self,
        app: &AppHandle,
        code: Option<i32>,
        api: &tauri::ExitRequestApi,
    ) {
        if code == Some(tauri::RESTART_EXIT_CODE)
            || self.processing_shutdown.load(Ordering::Acquire) == 2
        {
            return;
        }
        api.prevent_exit();
        if self
            .processing_shutdown
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(runtime) = app.try_state::<RustRuntime>() {
                if let Err(error) = runtime.stop_processing().await {
                    eprintln!("Rust processing shutdown: {error}");
                }
                runtime.processing_shutdown.store(2, Ordering::Release);
            }
            app.exit(code.unwrap_or(0));
        });
    }

    async fn stop_processing(&self) -> Result<(), String> {
        let result = self
            .on_store_lane_optional(None, false, |session| {
                match session {
                    Some(Session::Synthetic(store)) => store.stop_processing(),
                    Some(Session::Protected(vault)) => vault.stop_processing(),
                    None => Ok(()),
                }
                .map_err(|error| reply(500, &error))
            })
            .await;
        let _drained = self
            .processing_lane
            .acquire()
            .await
            .map_err(|_| "Rust processing queue unavailable")?;
        result?.map_err(|error| {
            error.body["detail"]
                .as_str()
                .unwrap_or("Unable to close Rust processing queue")
                .to_owned()
        })
    }
    pub(crate) fn new(root: PathBuf) -> Self {
        Self::with_mode(root, false)
    }

    pub(crate) fn protected(root: PathBuf) -> Self {
        Self::with_mode(root, true)
    }

    pub(crate) async fn initialize(&self) -> Result<(), String> {
        self.with_store(|_| Ok(()))
            .await
            .map_err(|error| format!("Cannot open the Rust workspace. {error}"))?
            .map_err(|response| {
                response.body["detail"]
                    .as_str()
                    .unwrap_or("Cannot open the Rust workspace")
                    .to_owned()
            })
    }

    fn with_mode(root: PathBuf, protected: bool) -> Self {
        Self {
            root,
            store: Arc::new(Mutex::new(None)),
            protected,
            unlocked: Arc::new(AtomicBool::new(!protected)),
            admission: Arc::new(Semaphore::new(32)),
            database_lane: Arc::new(Semaphore::new(1)),
            pdf_lane: Arc::new(Semaphore::new(2)),
            jobs: Jobs::new(),
            picked: Arc::new(Mutex::new(HashMap::new())),
            native_drop: Mutex::new(None),
            open_pdfs: Mutex::new(HashMap::new()),
            artifact_admission: Arc::new(Semaphore::new(32)),
            ocr_review_admission: Arc::new(Semaphore::new(8)),
            ocr_review_lane: Arc::new(Semaphore::new(1)),
            processing_lane: Arc::new(Semaphore::new(1)),
            processing_wake: AtomicBool::new(false),
            processing_shutdown: AtomicU8::new(0),
        }
    }
    #[cfg(test)]
    async fn request(&self, method: String, path: String, body: Value) -> Result<Reply, String> {
        self.request_with_keys(method, path, body, None).await
    }

    async fn request_with_keys(
        &self,
        method: String,
        path: String,
        body: Value,
        key_source: Option<String>,
    ) -> Result<Reply, String> {
        if self.processing_shutdown.load(Ordering::Acquire) != 0 && method == "POST" {
            return Ok(reply(503, "Rust processing queue is shutting down"));
        }
        if method == "GET" && path == "/health" {
            let mut health = matterdraft_backend::request(&method, &path);
            if !self.unlocked.load(Ordering::Acquire) {
                health.body["status"] = serde_json::json!("locked");
            }
            return Ok(health);
        }
        let (route, query) = path.split_once('?').unwrap_or((&path, ""));
        if let ("GET", Some(matter)) = (method.as_str(), matter_resource(route, "portable")) {
            let matter = matter.to_owned();
            return self
                .staged_pdf_request(
                    move |session| session.capture_portable_backup(&matter),
                    |work| work.compute_reply(),
                    |session, backup| session.complete_portable_backup(backup),
                )
                .await;
        }
        if self.protected && method == "POST" && route == "/matters/restore" {
            return self
                .staged_pdf_request(
                    move |session| {
                        let Session::Protected(vault) = session else {
                            return Err(matterdraft_backend::unsupported());
                        };
                        vault.capture_portable_restore().map(|work| (work, body))
                    },
                    |(work, body)| work.compute_upload(body),
                    |session, prepared| {
                        let Session::Protected(vault) = session else {
                            return Err(matterdraft_backend::unsupported());
                        };
                        Ok(vault.complete_portable_restore(prepared))
                    },
                )
                .await;
        }
        if self.protected {
            if let Some(target) = DocumentUploadTarget::from_path(&method, route, query) {
                return self.import_uploaded_documents(target, body).await;
            }
            if let Some(matter) = cover_import_matter(&method, route) {
                let matter = matter.to_owned();
                return self
                    .staged_pdf_request(
                        move |session| {
                            let Session::Protected(vault) = session else {
                                return Err(matterdraft_backend::unsupported());
                            };
                            vault.capture_cover_import(&matter)
                        },
                        |work| work.compute(),
                        |session, prepared| {
                            let Session::Protected(vault) = session else {
                                return Err(matterdraft_backend::unsupported());
                            };
                            Ok(vault.complete_cover_import(prepared))
                        },
                    )
                    .await;
            }
        }
        if let Some(document) = annotation_image_document(&method, route) {
            let document = document.to_owned();
            return self
                .staged_pdf_request(
                    move |session| match session {
                        Session::Synthetic(store) => {
                            store.prepare_annotation_image(&document, body)
                        }
                        Session::Protected(vault) => {
                            vault.prepare_annotation_image(&document, body)
                        }
                    },
                    |work| work.compute(),
                    |session, measured| match session {
                        Session::Synthetic(store) => store.complete_annotation_image(measured),
                        Session::Protected(vault) => vault.complete_annotation_image(measured),
                    },
                )
                .await;
        }
        if let Some(id) = annotation_asset_id(&method, route) {
            let id = id.to_owned();
            return Ok(self
                .compute_pdf_request(
                    move |session| match session {
                        Session::Synthetic(store) => store.prepare_annotation_asset(&id),
                        Session::Protected(vault) => vault.prepare_annotation_asset(&id),
                    },
                    |work| work.compute(),
                )
                .await?
                .unwrap_or_else(|refusal| refusal));
        }
        if let Some(target) = OutputPreviewTarget::from_path(&method, route) {
            return self
                .staged_pdf_request(
                    move |session| match session {
                        Session::Synthetic(store) => store.prepare_output_preview(target, &body),
                        Session::Protected(vault) => vault.prepare_output_preview(target, &body),
                    },
                    |work| work.compute(),
                    |session, measured| match session {
                        Session::Synthetic(store) => store.complete_output_preview(measured),
                        Session::Protected(vault) => vault.complete_output_preview(measured),
                    },
                )
                .await;
        }
        if let Some(target) = MetadataReviewTarget::from_path(&method, &path) {
            return self
                .staged_pdf_request(
                    move |session| {
                        let order =
                            matterdraft_backend::RequestOrder::from_json(key_source.as_deref());
                        match session {
                            Session::Synthetic(store) => {
                                store.prepare_metadata_review_with_order(target, &body, &order)
                            }
                            Session::Protected(vault) => {
                                vault.prepare_metadata_review_with_order(target, &body, &order)
                            }
                        }
                    },
                    |work| work.compute(),
                    |session, measured| match session {
                        Session::Synthetic(store) => store.complete_metadata_review(measured),
                        Session::Protected(vault) => vault.complete_metadata_review(measured),
                    },
                )
                .await;
        }
        if let Some(target) = LabelReviewTarget::from_path(&method, &path) {
            return self
                .staged_pdf_request(
                    move |session| match session {
                        Session::Synthetic(store) => store.prepare_label_review(target, &body),
                        Session::Protected(vault) => vault.prepare_label_review(target, &body),
                    },
                    |work| work.compute(),
                    |session, measured| match session {
                        Session::Synthetic(store) => store.complete_label_review(measured),
                        Session::Protected(vault) => vault.complete_label_review(measured),
                    },
                )
                .await;
        }
        if let Some(target) = SearchTarget::from_path(&method, route) {
            let query = query.to_owned();
            return Ok(self
                .compute_pdf_request(
                    move |session| match session {
                        Session::Synthetic(store) => store.prepare_search(target, &query),
                        Session::Protected(vault) => vault.prepare_search(target, &query),
                    },
                    |work| work.compute(),
                )
                .await?
                .unwrap_or_else(|error| error));
        }
        if let Some(target) = TextPageTarget::from_path(&method, route) {
            return self
                .staged_pdf_request(
                    move |session| match session {
                        Session::Synthetic(store) => store.prepare_text_page(target, &body),
                        Session::Protected(vault) => vault.prepare_text_page(target, &body),
                    },
                    |work| work.compute(),
                    |session, measured| match session {
                        Session::Synthetic(store) => store.complete_text_page(measured),
                        Session::Protected(vault) => vault.complete_text_page(measured),
                    },
                )
                .await;
        }
        if let Some(target) = RevisionTarget::from_path(&method, &path) {
            return self
                .staged_pdf_request(
                    move |session| {
                        let order =
                            matterdraft_backend::RequestOrder::from_json(key_source.as_deref());
                        match session {
                            Session::Synthetic(store) => {
                                store.prepare_revision_edit(target, body, &order)
                            }
                            Session::Protected(vault) => {
                                vault.prepare_revision_edit(target, body, &order)
                            }
                        }
                    },
                    |work| work.compute(),
                    |session, measured| match session {
                        Session::Synthetic(store) => store.complete_revision_edit(measured),
                        Session::Protected(vault) => vault.complete_revision_edit(measured),
                    },
                )
                .await;
        }
        if let Some(target) = PagePreviewTarget::from_path(&method, route) {
            let query = query.to_owned();
            return Ok(self
                .compute_pdf_request(
                    move |session| match session {
                        Session::Synthetic(store) => store.prepare_page_preview(target, &query),
                        Session::Protected(vault) => vault.prepare_page_preview(target, &query),
                    },
                    |work| work.compute(),
                )
                .await?
                .unwrap_or_else(|refusal| refusal));
        }
        if let Some(target) = RedactionTarget::from_path(&method, route) {
            return self
                .staged_pdf_request(
                    move |session| {
                        let order =
                            matterdraft_backend::RequestOrder::from_json(key_source.as_deref());
                        match session {
                            Session::Synthetic(store) => {
                                store.prepare_redaction(target, body, &order)
                            }
                            Session::Protected(vault) => {
                                vault.prepare_redaction(target, body, &order)
                            }
                        }
                    },
                    |work| work.compute(),
                    |session, measured| match session {
                        Session::Synthetic(store) => store.complete_redaction(measured),
                        Session::Protected(vault) => vault.complete_redaction(measured),
                    },
                )
                .await;
        }
        if let (true, Some(document)) = (
            self.protected,
            uploaded_replacement_document(&method, route),
        ) {
            let document = document.to_owned();
            return self
                .staged_pdf_request(
                    move |session| {
                        let Session::Protected(vault) = session else {
                            return Err(matterdraft_backend::unsupported());
                        };
                        vault.prepare_uploaded_replacement(&document, body)
                    },
                    |work| work.compute(),
                    Session::publish_working_replacement,
                )
                .await;
        }
        if let Some(target) = PageOutputTarget::from_path(&method, route, query) {
            let target = match target {
                Ok(target) => target,
                Err(error) => return Ok(error),
            };
            return Ok(self
                .measure_page_output(
                    target.document,
                    target.output,
                    target.selection,
                    target.acknowledged,
                    move |bytes| PageOutputTarget::into_reply(target.output, bytes),
                )
                .await?
                .unwrap_or_else(|refusal| refusal));
        }
        if let Some(document) = original_info_document(&method, &path) {
            let document = document.to_owned();
            return Ok(self
                .compute_pdf_request(
                    move |session| match session {
                        Session::Synthetic(store) => store.prepare_original_info(&document),
                        Session::Protected(vault) => vault.prepare_original_info(&document),
                    },
                    |work| work.compute(),
                )
                .await?
                .unwrap_or_else(|refusal| refusal));
        }
        if let Some(target) = DocumentOutputTarget::from_path(&method, route, query) {
            return Ok(self
                .compute_pdf_request(
                    move |session| match session {
                        Session::Synthetic(store) => store.prepare_document_output(target, body),
                        Session::Protected(vault) => vault.prepare_document_output(target, body),
                    },
                    |work| work.compute(),
                )
                .await?
                .unwrap_or_else(|refusal| refusal));
        }
        if let Some(target) = OutputValidationTarget::from_path(&method, route, &body) {
            return self
                .staged_pdf_request(
                    move |session| match session {
                        Session::Synthetic(store) => store.prepare_output_validation(target),
                        Session::Protected(vault) => vault.prepare_output_validation(target),
                    },
                    |work| work.compute(),
                    |session, measured| match session {
                        Session::Synthetic(store) => store.complete_output_validation(measured),
                        Session::Protected(vault) => vault.complete_output_validation(measured),
                    },
                )
                .await;
        }
        if let Some(target) = ExportOutputTarget::from_path(&method, route, query) {
            return Ok(self
                .compute_pdf_request(
                    move |session| match session {
                        Session::Synthetic(store) => store.prepare_export_output(target, body),
                        Session::Protected(vault) => vault.prepare_export_output(target, body),
                    },
                    |work| work.compute(),
                )
                .await?
                .unwrap_or_else(|refusal| refusal));
        }
        if let ("GET", Some(id)) = (
            method.as_str(),
            route
                .strip_prefix("/documents/")
                .and_then(|route| route.strip_suffix("/ocr-review"))
                .filter(|id| !id.contains('/')),
        ) {
            return self.inspect_ocr(id.to_owned(), query.to_owned()).await;
        }
        if let ("POST", Some(id)) = (
            method.as_str(),
            route
                .strip_prefix("/matters/")
                .and_then(|route| route.strip_suffix("/preflight/screen"))
                .filter(|id| !id.contains('/')),
        ) {
            return self.submit_screening(id.to_owned(), body).await;
        }
        if let ("GET", Some(matter)) = (method.as_str(), matter_resource(route, "validate")) {
            return self.run_preflight(matter.to_owned()).await;
        }
        if let ("GET", Some((matter, plan))) = (method.as_str(), variant_review_target(route)) {
            return self
                .review_variant(matter.to_owned(), plan.to_owned())
                .await;
        }
        if let ("GET", Some((matter, kind))) = (method.as_str(), catalog_target(route)) {
            return self.composition_catalog(matter.to_owned(), kind).await;
        }
        if let ("POST", Some(target)) = (method.as_str(), processing_target(route)) {
            return self.submit_processing(target, body).await;
        }
        if let ("POST", Some((matter, build, operation))) =
            (method.as_str(), comparison_target(route))
        {
            return self
                .export_comparison(matter.to_owned(), build.to_owned(), operation, body)
                .await;
        }
        if let Some(target) = CompositionDraftTarget::from_path(&method, route) {
            return self.composition_draft(target, body).await;
        }
        if let ("POST", Some(matter)) = (method.as_str(), matter_resource(route, "bundle-preview"))
        {
            return self.bundle_preview(matter.to_owned(), body, false).await;
        }
        if let ("POST", Some(matter)) = (method.as_str(), matter_resource(route, "volume-planning"))
        {
            return self.bundle_preview(matter.to_owned(), body, true).await;
        }
        if !self.protected
            && !path.starts_with("/matters")
            && !path.starts_with("/documents/")
            && !path.starts_with("/annotations/")
            && !path.starts_with("/jobs/")
            && path != "/pdfium-paragraph-trial/status"
            && path != "/output-presets"
            && !matches!(path.split('?').next(), Some("/reader-position"))
        {
            return Ok(matterdraft_backend::request(&method, &path));
        }
        let unlocks = method == "POST" && matches!(path.as_str(), "/vault/setup" | "/vault/unlock");
        let state = self.unlocked.clone();
        Ok(self
            .with_store(move |store| {
                let order = matterdraft_backend::RequestOrder::from_json(key_source.as_deref());
                let result = store.request(&method, &path, body, &order);
                if unlocks && result.status == 200 {
                    state.store(true, Ordering::Release);
                }
                Ok(result)
            })
            .await?
            .unwrap_or_else(|refusal| refusal))
    }

    async fn import_uploaded_documents(
        &self,
        target: DocumentUploadTarget,
        body: Value,
    ) -> Result<Reply, String> {
        let Ok(admission) = self.admission.clone().try_acquire_owned() else {
            return Ok(reply(503, "Rust store busy; request not started"));
        };
        let lane = self.acquire_pdf_worker().await?;
        let captured = self
            .on_store_lane(None, move |session| {
                let Session::Protected(vault) = session else {
                    return Err(matterdraft_backend::unsupported());
                };
                vault
                    .capture_document_upload(target, body)
                    .map(|batch| (batch, admission, lane))
            })
            .await?;
        let (batch, mut admission, mut lane) = match captured {
            Ok(captured) => captured,
            Err(error) => return Ok(error),
        };
        let mut imported = Vec::new();
        for file in batch.into_files() {
            let measured = tauri::async_runtime::spawn_blocking(move || {
                file.compute().map(|prepared| (prepared, admission, lane))
            })
            .await
            .map_err(|_| "Rust import worker failed; verify state before retrying")?;
            let (prepared, held_admission, held_lane) = match measured {
                Ok(prepared) => prepared,
                Err(error) => return Ok(error),
            };
            let published = self
                .on_store_lane(None, move |session| {
                    session
                        .complete_document_import(prepared)
                        .map(|response| (response, held_admission, held_lane))
                })
                .await?;
            let (response, held_admission, held_lane) = match published {
                Ok(published) => published,
                Err(error) => return Ok(error),
            };
            admission = held_admission;
            lane = held_lane;
            if response.status != 201 {
                return Ok(response);
            }
            if let Value::Array(items) = response.body {
                imported.extend(items);
            }
        }
        Ok(Reply {
            status: 201,
            body: Value::Array(imported),
        })
    }

    async fn inspect_ocr(&self, id: String, query: String) -> Result<Reply, String> {
        let Ok(admission) = self.ocr_review_admission.clone().try_acquire_owned() else {
            return Ok(reply(503, "OCR review is busy; request not started"));
        };
        let lane = self
            .ocr_review_lane
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "OCR review worker is unavailable".to_owned())?;
        let work = match self
            .with_store(move |session| match session {
                Session::Synthetic(store) => store.prepare_ocr_review(&id, &query),
                Session::Protected(vault) => vault.prepare_ocr_review(&id, &query),
            })
            .await?
        {
            Ok(work) => work,
            Err(refusal) => return Ok(refusal),
        };
        let (measured, admission) = tauri::async_runtime::spawn_blocking(move || {
            let _lane = lane;
            (work.compute(), admission)
        })
        .await
        .map_err(|_| "OCR review worker failed".to_owned())?;
        let measured = match measured {
            Ok(measured) => measured,
            Err(refusal) => return Ok(refusal),
        };
        Ok(self
            .with_store(move |session| {
                let _admission = admission;
                match session {
                    Session::Synthetic(store) => store.complete_ocr_review(measured),
                    Session::Protected(vault) => vault.complete_ocr_review(measured),
                }
            })
            .await?
            .unwrap_or_else(|refusal| refusal))
    }

    async fn submit_screening(&self, id: String, body: Value) -> Result<Reply, String> {
        let Ok(admission) = self.admission.clone().try_acquire_owned() else {
            return Ok(reply(503, "Rust store busy; request not started"));
        };
        let prepared = self
            .on_store_lane(None, move |session| match session {
                Session::Synthetic(store) => store.prepare_screen_submission(&id, &body),
                Session::Protected(vault) => vault.prepare_screen_submission(&id, &body),
            })
            .await?;
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(refusal) => return Ok(refusal),
        };
        let lane = self.acquire_pdf_worker().await?;
        let (verified, admission) = tauri::async_runtime::spawn_blocking(move || {
            let _lane = lane;
            (prepared.verify(), admission)
        })
        .await
        .map_err(|_| "Screening verification worker failed")?;
        let verified = match verified {
            Ok(verified) => verified,
            Err(refusal) => return Ok(refusal),
        };
        Ok(self
            .on_store_lane(Some(admission), move |session| match session {
                Session::Synthetic(store) => store.submit_screening(verified),
                Session::Protected(vault) => vault.submit_screening(verified),
            })
            .await?
            .unwrap_or_else(|refusal| refusal))
    }

    async fn submit_processing(
        &self,
        target: ProcessingTarget,
        body: Value,
    ) -> Result<Reply, String> {
        self.staged_pdf_request(
            move |session| match (session, target) {
                (Session::Synthetic(store), ProcessingTarget::Build { matter, kind, plan }) => {
                    store.prepare_build_submission(&matter, kind, plan.as_deref(), &body)
                }
                (Session::Protected(vault), ProcessingTarget::Build { matter, kind, plan }) => {
                    vault.prepare_build_submission(&matter, kind, plan.as_deref(), &body)
                }
                (Session::Synthetic(store), ProcessingTarget::Retry { job }) => {
                    store.prepare_retry(&job, &body)
                }
                (Session::Protected(vault), ProcessingTarget::Retry { job }) => {
                    vault.prepare_retry(&job, &body)
                }
            },
            |work| work.compute(),
            |session, measured| match session {
                Session::Synthetic(store) => store.submit_processing(measured),
                Session::Protected(vault) => vault.submit_processing(measured),
            },
        )
        .await
    }

    async fn run_preflight(&self, matter: String) -> Result<Reply, String> {
        self.staged_pdf_request(
            move |session| match session {
                Session::Synthetic(store) => store.prepare_preflight(&matter),
                Session::Protected(vault) => vault.prepare_preflight(&matter),
            },
            |work| work.measure(),
            |session, measured| match session {
                Session::Synthetic(store) => store.publish_preflight(measured),
                Session::Protected(vault) => vault.publish_preflight(measured),
            },
        )
        .await
    }

    async fn bundle_preview(
        &self,
        matter: String,
        body: Value,
        volume: bool,
    ) -> Result<Reply, String> {
        self.staged_pdf_request(
            move |session| match (session, volume) {
                (Session::Synthetic(store), false) => store.prepare_bundle_preview(&matter, &body),
                (Session::Protected(vault), false) => vault.prepare_bundle_preview(&matter, &body),
                (Session::Synthetic(store), true) => store.prepare_volume_planning(&matter, &body),
                (Session::Protected(vault), true) => vault.prepare_volume_planning(&matter, &body),
            },
            |work| work.measure(),
            |session, measured| match session {
                Session::Synthetic(store) => store.complete_bundle_preview(measured),
                Session::Protected(vault) => vault.complete_bundle_preview(measured),
            },
        )
        .await
    }

    async fn export_comparison(
        &self,
        matter: String,
        build: String,
        operation: &'static str,
        body: Value,
    ) -> Result<Reply, String> {
        self.staged_pdf_request(
            move |session| match session {
                Session::Synthetic(store) => {
                    store.prepare_export_comparison(&matter, &build, operation, &body)
                }
                Session::Protected(vault) => {
                    vault.prepare_export_comparison(&matter, &build, operation, &body)
                }
            },
            |work| work.measure(),
            |session, measured| match session {
                Session::Synthetic(store) => store.complete_export_comparison(measured),
                Session::Protected(vault) => vault.complete_export_comparison(measured),
            },
        )
        .await
    }

    async fn composition_catalog(
        &self,
        matter: String,
        kind: CompositionCatalogKind,
    ) -> Result<Reply, String> {
        self.staged_pdf_request(
            move |session| match session {
                Session::Synthetic(store) => store.prepare_composition_catalog(&matter, kind),
                Session::Protected(vault) => vault.prepare_composition_catalog(&matter, kind),
            },
            |work| work.verify(),
            |session, verified| match session {
                Session::Synthetic(store) => store.complete_composition_catalog(verified),
                Session::Protected(vault) => vault.complete_composition_catalog(verified),
            },
        )
        .await
    }

    async fn composition_draft(
        &self,
        target: CompositionDraftTarget,
        body: Value,
    ) -> Result<Reply, String> {
        self.staged_pdf_request(
            move |session| match session {
                Session::Synthetic(store) => store.prepare_composition_draft(target, &body),
                Session::Protected(vault) => vault.prepare_composition_draft(target, &body),
            },
            |work| work.measure(),
            |session, measured| match session {
                Session::Synthetic(store) => store.complete_composition_draft(measured),
                Session::Protected(vault) => vault.complete_composition_draft(measured),
            },
        )
        .await
    }

    async fn review_variant(&self, matter: String, plan: String) -> Result<Reply, String> {
        self.staged_pdf_request(
            move |session| match session {
                Session::Synthetic(store) => store.prepare_variant_review(&matter, &plan),
                Session::Protected(vault) => vault.prepare_variant_review(&matter, &plan),
            },
            |work| work.measure(),
            |session, measured| match session {
                Session::Synthetic(store) => store.complete_variant_review(measured),
                Session::Protected(vault) => vault.complete_variant_review(measured),
            },
        )
        .await
    }

    async fn staged_pdf_request<C: Send + 'static, M: Send + 'static>(
        &self,
        capture: impl FnOnce(&mut Session) -> Result<C, Reply> + Send + 'static,
        measure: impl FnOnce(C) -> Result<M, Reply> + Send + 'static,
        publish: impl FnOnce(&mut Session, M) -> Result<Reply, Reply> + Send + 'static,
    ) -> Result<Reply, String> {
        Ok(self
            .staged_pdf_work(capture, measure, publish)
            .await?
            .unwrap_or_else(|refusal| refusal))
    }

    async fn staged_pdf_work<C: Send + 'static, M: Send + 'static, T: Send + 'static>(
        &self,
        capture: impl FnOnce(&mut Session) -> Result<C, Reply> + Send + 'static,
        measure: impl FnOnce(C) -> Result<M, Reply> + Send + 'static,
        publish: impl FnOnce(&mut Session, M) -> Result<T, Reply> + Send + 'static,
    ) -> Result<Result<T, Reply>, String> {
        let measured = match self.measure_pdf_request(capture, measure).await? {
            Ok(measured) => measured,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let MeasuredPdf {
            output,
            admission,
            lane,
        } = measured;
        self.on_store_lane(Some(admission), move |session| {
            let _lane = lane;
            publish(session, output)
        })
        .await
    }

    async fn measure_pdf_request<C: Send + 'static, M: Send + 'static>(
        &self,
        capture: impl FnOnce(&mut Session) -> Result<C, Reply> + Send + 'static,
        measure: impl FnOnce(C) -> Result<M, Reply> + Send + 'static,
    ) -> Result<Result<MeasuredPdf<M>, Reply>, String> {
        let Ok(admission) = self.admission.clone().try_acquire_owned() else {
            return Ok(Err(reply(503, "Rust store busy; request not started")));
        };
        let lane = self.acquire_pdf_worker().await?;
        let captured = self
            .on_store_lane(None, move |session| {
                capture(session).map(|work| (work, admission, lane))
            })
            .await?;
        let (work, admission, lane) = match captured {
            Ok(captured) => captured,
            Err(refusal) => return Ok(Err(refusal)),
        };
        tauri::async_runtime::spawn_blocking(move || {
            let output = measure(work)?;
            Ok(MeasuredPdf {
                output,
                admission,
                lane,
            })
        })
        .await
        .map_err(|_| "Rust PDF worker failed; verify state before retrying".into())
    }

    async fn compute_pdf_request<C: Send + 'static, M: Send + 'static>(
        &self,
        capture: impl FnOnce(&mut Session) -> Result<C, Reply> + Send + 'static,
        compute: impl FnOnce(C) -> Result<M, Reply> + Send + 'static,
    ) -> Result<Result<M, Reply>, String> {
        Ok(self
            .measure_pdf_request(capture, compute)
            .await?
            .map(|measured| measured.output))
    }

    async fn with_store<T: Send + 'static>(
        &self,
        task: impl FnOnce(&mut Session) -> Result<T, Reply> + Send + 'static,
    ) -> Result<Result<T, Reply>, String> {
        let admission = match self.admission.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => return Ok(Err(reply(503, "Rust store busy; request not started"))),
        };
        self.on_store_lane(Some(admission), task).await
    }

    async fn on_store_lane<T: Send + 'static>(
        &self,
        admission: Option<tokio::sync::OwnedSemaphorePermit>,
        task: impl FnOnce(&mut Session) -> Result<T, Reply> + Send + 'static,
    ) -> Result<Result<T, Reply>, String> {
        self.on_store_lane_optional(admission, true, |session| {
            task(session.expect("initialized store"))
        })
        .await
    }

    async fn on_store_lane_optional<T: Send + 'static>(
        &self,
        admission: Option<tokio::sync::OwnedSemaphorePermit>,
        open: bool,
        task: impl FnOnce(Option<&mut Session>) -> Result<T, Reply> + Send + 'static,
    ) -> Result<Result<T, Reply>, String> {
        let lane = self
            .database_lane
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "Rust store unavailable".to_owned())?;
        let store = self.store.clone();
        let root = self.root.clone();
        let protected = self.protected;
        tauri::async_runtime::spawn_blocking(move || {
            // Keep permits until actual completion even if the caller stops waiting.
            let (_admission, _lane) = (admission, lane);
            let mut store = store
                .lock()
                .map_err(|_| "Rust store unavailable".to_owned())?;
            if store.is_none() && open {
                *store = Some(if protected {
                    let parent = root.parent().ok_or("Invalid Rust vault location")?;
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                    Session::Protected(VaultStore::open(&root)?)
                } else {
                    Session::Synthetic(Store::open(&root)?)
                });
            }
            Ok(task(store.as_mut()))
        })
        .await
        .map_err(|_| "Rust store task failed; verify state before retrying".to_owned())?
    }

    pub(crate) async fn open_snapshot(
        &self,
        document_id: String,
        source: Artifact,
    ) -> Result<Result<PdfSnapshot, Reply>, String> {
        self.compute_pdf_request(
            move |session| match session {
                Session::Synthetic(store) => store.prepare_pdf_snapshot(&document_id, &source),
                Session::Protected(vault) => vault.prepare_pdf_snapshot(&document_id, &source),
            },
            |work| work.compute(),
        )
        .await
    }

    pub(crate) async fn with_editable_snapshot<T: Send + 'static>(
        &self,
        document_id: String,
        revision: u64,
        acknowledged: bool,
        transfer: impl FnOnce(PdfSnapshot) -> T + Send + 'static,
    ) -> Result<Result<T, Reply>, String> {
        self.compute_pdf_request(
            move |session| match session {
                Session::Synthetic(store) => {
                    store.prepare_editable_snapshot(&document_id, Some(revision), acknowledged)
                }
                Session::Protected(vault) => {
                    vault.prepare_editable_snapshot(&document_id, revision, acknowledged)
                }
            },
            move |work| work.compute().map(transfer),
        )
        .await
    }

    pub(crate) async fn write_portable_backup(
        &self,
        matter: String,
        destination: File,
    ) -> Result<Result<File, Reply>, String> {
        self.staged_pdf_work(
            move |session| session.capture_portable_backup(&matter),
            move |work| work.compute(destination),
            |session, backup| session.complete_portable_backup(backup),
        )
        .await
    }

    pub(crate) async fn export_snapshot(
        &self,
        matter: String,
        id: String,
        name: String,
        operation: String,
        options: Value,
    ) -> Result<Result<PdfSnapshot, Reply>, String> {
        self.with_export_snapshot(matter, id, name, operation, options, |snapshot| snapshot)
            .await
    }

    pub(crate) async fn with_export_snapshot<T: Send + 'static>(
        &self,
        matter: String,
        id: String,
        name: String,
        operation: String,
        options: Value,
        transfer: impl FnOnce(PdfSnapshot) -> T + Send + 'static,
    ) -> Result<Result<T, Reply>, String> {
        self.compute_pdf_request(
            move |session| match session {
                Session::Synthetic(store) => {
                    store.prepare_export_snapshot(&matter, &id, &name, &operation, options)
                }
                Session::Protected(vault) => {
                    vault.prepare_export_snapshot(&matter, &id, &name, &operation, options)
                }
            },
            move |work| work.compute().map(transfer),
        )
        .await
    }

    async fn process_jobs(&self, updated: impl Fn(&str)) -> Result<(), String> {
        while self.processing_shutdown.load(Ordering::Acquire) == 0 {
            let Ok(permit) = self.processing_lane.try_acquire() else {
                return Ok(());
            };
            self.processing_wake.store(false, Ordering::Release);
            self.drain_jobs(&updated).await?;
            drop(permit);
            if !self.processing_wake.swap(false, Ordering::AcqRel) {
                return Ok(());
            }
        }
        Ok(())
    }

    async fn drain_jobs(&self, updated: &impl Fn(&str)) -> Result<(), String> {
        while self.processing_shutdown.load(Ordering::Acquire) == 0 {
            let pending = self
                .on_store_lane(None, |session| {
                    let mut updates = Vec::new();
                    let collect = |id: &str| updates.push(id.to_owned());
                    let work = match session {
                        Session::Synthetic(store) => store.begin_work_with_updates(collect),
                        Session::Protected(vault) => vault.begin_work_with_updates(collect),
                    }
                    .map_err(|error| reply(500, &error))?;
                    Ok((work, updates))
                })
                .await?;
            let (pending, changes) = pending.map_err(|error| {
                error.body["detail"]
                    .as_str()
                    .unwrap_or("Unable to start Rust job")
                    .to_owned()
            })?;
            for id in changes {
                updated(&id);
            }
            let Some(pending) = pending else {
                return Ok(());
            };
            let job_id = pending.id().to_owned();
            let (progress, mut checkpoints) = tokio::sync::mpsc::channel(1);
            let worker = tauri::async_runtime::spawn_blocking(move || {
                pending.compute_with_checkpoint(|id| {
                    let (committed, outcome) = tokio::sync::oneshot::channel();
                    progress
                        .blocking_send((id.to_owned(), committed))
                        .map_err(|_| reply(500, "Unable to save job progress"))?;
                    outcome
                        .blocking_recv()
                        .map_err(|_| reply(500, "Unable to save job progress"))?
                })
            });
            while let Some((id, committed)) = checkpoints.recv().await {
                let checkpoint_id = id.clone();
                let result = self
                    .on_store_lane(None, move |session| match session {
                        Session::Synthetic(store) => store.checkpoint_processing_job(&id),
                        Session::Protected(vault) => vault.checkpoint_processing_job(&id),
                    })
                    .await
                    .unwrap_or_else(|_| Err(reply(500, "Unable to save job progress")));
                if result.is_ok() {
                    updated(&checkpoint_id);
                }
                let _ = committed.send(result);
            }
            let finished = worker.await;
            let finished_id = job_id.clone();
            let published = self
                .on_store_lane(None, move |session| {
                    match (session, finished) {
                        (Session::Synthetic(store), Ok(work)) => store.finish_work(work),
                        (Session::Protected(vault), Ok(work)) => vault.finish_work(work),
                        (Session::Synthetic(store), Err(_)) => {
                            store.interrupt_processing_job(&finished_id)
                        }
                        (Session::Protected(vault), Err(_)) => {
                            vault.interrupt_processing_job(&finished_id)
                        }
                    }
                    .map_err(|error| reply(500, &error))
                })
                .await?;
            updated(&job_id);
            if published.is_err() {
                eprintln!("Rust processing queue could not publish a job");
            }
        }
        Ok(())
    }

    pub(crate) async fn measure_page_output<T: Send + 'static>(
        &self,
        document_id: String,
        output: PageOutput,
        selection: String,
        acknowledged: bool,
        format: impl FnOnce(Vec<u8>) -> T + Send + 'static,
    ) -> Result<Result<T, Reply>, String> {
        self.compute_pdf_request(
            move |session| match session {
                Session::Synthetic(store) => {
                    store.prepare_page_output(&document_id, output, &selection, acknowledged)
                }
                Session::Protected(vault) => {
                    vault.prepare_page_output(&document_id, output, &selection, acknowledged)
                }
            },
            move |work| work.compute().map(format),
        )
        .await
    }

    async fn insert_pdf(&self, document_id: String, source: Artifact) -> Result<Reply, String> {
        let snapshot = match self.open_snapshot(document_id, source).await? {
            Ok(snapshot) => snapshot,
            Err(refusal) => return Ok(refusal),
        };
        self.register_snapshot(snapshot, None)
    }

    fn register_snapshot(
        &self,
        snapshot: PdfSnapshot,
        print_fit: Option<bool>,
    ) -> Result<Reply, String> {
        let mut files = self
            .open_pdfs
            .lock()
            .map_err(|_| "PDF reader unavailable")?;
        files.retain(|_, file| {
            file.last_used.elapsed() < PDF_IDLE_TIMEOUT
                && (print_fit.is_none() || file.print_fit.is_none())
        });
        if files.len() >= MAX_OPEN_PDFS {
            return Ok(reply(503, "Too many open Rust PDF readers"));
        }
        let token = format!("{:032x}", rand::random::<u128>());
        let length = snapshot.bytes;
        files.insert(
            token.clone(),
            OpenPdf {
                snapshot: Arc::new(Mutex::new(snapshot)),
                last_used: Instant::now(),
                print_fit,
            },
        );
        Ok(Reply {
            status: 200,
            body: serde_json::json!({"token":token,"length":length}),
        })
    }

    #[cfg(target_os = "macos")]
    pub(crate) async fn register_print(
        &self,
        route: String,
        options: Value,
        fit: bool,
    ) -> Result<Reply, String> {
        let snapshot = if let Some(target) = DocumentOutputTarget::from_path("POST", &route, "") {
            self.compute_pdf_request(
                move |session| match session {
                    Session::Synthetic(store) => store.prepare_document_output(target, options),
                    Session::Protected(vault) => vault.prepare_document_output(target, options),
                },
                |work| work.snapshot(),
            )
            .await?
        } else {
            let segments = route.trim_start_matches('/').split('/').collect::<Vec<_>>();
            let ["matters", matter, "exports", export, filename, "print"] = segments.as_slice()
            else {
                return Err("Choose an original, working copy or saved PDF to print".into());
            };
            self.export_snapshot(
                (*matter).to_owned(),
                (*export).to_owned(),
                (*filename).to_owned(),
                "print".into(),
                options,
            )
            .await?
        };
        match snapshot {
            Ok(snapshot) if snapshot.bytes <= 160 * 1024 * 1024 => {
                self.register_snapshot(snapshot, Some(fit))
            }
            Ok(_) => Ok(reply(413, "The prepared print PDF exceeds 160 MiB")),
            Err(error) => Ok(error),
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) async fn with_print_bytes<T: Send + 'static>(
        &self,
        token: &str,
        load: impl FnOnce(Vec<u8>, bool) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let (snapshot, fit) = {
            let mut files = self
                .open_pdfs
                .lock()
                .map_err(|_| "PDF reader unavailable")?;
            files.retain(|_, file| file.last_used.elapsed() < PDF_IDLE_TIMEOUT);
            let file = files
                .get_mut(token)
                .ok_or("Print preview expired. Prepare it again.")?;
            let fit = file.print_fit.ok_or("Prepare the PDF for print first.")?;
            file.last_used = Instant::now();
            (file.snapshot.clone(), fit)
        };
        let lane = self.acquire_pdf_worker().await?;
        tauri::async_runtime::spawn_blocking(move || {
            let _lane = lane;
            let bytes = {
                let mut snapshot = snapshot.lock().map_err(|_| "PDF reader unavailable")?;
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(snapshot.bytes as usize)
                    .map_err(|_| "Could not allocate the prepared print PDF")?;
                snapshot
                    .copy_to(&mut bytes)
                    .map_err(|_| "Unable to read prepared print PDF")?;
                bytes
            };
            load(bytes, fit)
        })
        .await
        .map_err(|_| "The PDF print worker failed")?
    }

    pub(crate) async fn acquire_pdf_worker(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
        self.pdf_lane
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "Rust PDF workers are unavailable".to_owned())
    }

    async fn read_pdf(&self, token: &str, begin: u64, end: u64) -> Result<Vec<u8>, Reply> {
        if !end
            .checked_sub(begin)
            .is_some_and(|len| len > 0 && len <= READ_CHUNK_BYTES as u64)
        {
            return Err(reply(416, "Invalid PDF byte range"));
        }
        let admission = self
            .artifact_admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| reply(503, "Rust PDF readers are busy"))?;
        let permit = self
            .acquire_pdf_worker()
            .await
            .map_err(|error| reply(503, &error))?;
        let snapshot = {
            let mut files = self
                .open_pdfs
                .lock()
                .map_err(|_| reply(500, "PDF reader unavailable"))?;
            files.retain(|_, file| file.last_used.elapsed() < PDF_IDLE_TIMEOUT);
            let file = files
                .get_mut(token)
                .ok_or_else(|| reply(409, "PDF reader expired; reload the document"))?;
            file.last_used = Instant::now();
            file.snapshot.clone()
        };
        tauri::async_runtime::spawn_blocking(move || {
            let (_admission, _permit) = (admission, permit);
            let mut snapshot = snapshot
                .lock()
                .map_err(|_| reply(500, "PDF reader unavailable"))?;
            if end > snapshot.bytes {
                return Err(reply(416, "Invalid PDF byte range"));
            }
            snapshot
                .read_range(begin, end)
                .map_err(|_| reply(409, "PDF bytes changed during reading"))
        })
        .await
        .map_err(|_| reply(500, "Rust PDF worker failed"))?
    }

    fn close_pdf(&self, token: &str) -> Result<(), String> {
        self.open_pdfs
            .lock()
            .map_err(|_| "PDF reader unavailable")?
            .remove(token);
        Ok(())
    }
    #[cfg(test)]
    async fn extract(&self, bytes: Vec<u8>) -> Result<Value, String> {
        if bytes.len() > 16 * 1024 * 1024 {
            return Err("PDF extraction failed or exceeds supported limits".into());
        }
        let permit = self
            .pdf_lane
            .clone()
            .try_acquire_owned()
            .map_err(|_| "Rust PDF workers busy; request not started".to_owned())?;
        tauri::async_runtime::spawn_blocking(move || {
            let _permit = permit;
            matterdraft_backend::extract_text(bytes).map_err(str::to_owned)
        })
        .await
        .map_err(|_| "Rust PDF task failed".to_owned())?
    }
    #[cfg(test)]
    async fn import_pdf(
        &self,
        matter_id: String,
        filename: String,
        bytes: Vec<u8>,
    ) -> Result<Reply, String> {
        if bytes.len() > 4 * 1024 * 1024 {
            return Ok(Reply {
                status: 413,
                body: serde_json::json!({"detail":"Rust pilot PDF limit is 4 MiB"}),
            });
        }
        self.staged_pdf_request(
            move |session| session.capture_document_import(&matter_id),
            move |target| target.measure(filename, bytes, ImportOptions::default()),
            Session::complete_document_import,
        )
        .await
    }

    async fn stage_picked(&self, path: PathBuf) -> Result<Reply, String> {
        let purpose = if self.protected {
            PickPurpose::Document
        } else {
            PickPurpose::SyntheticDocument
        };
        self.stage_picked_for(path, purpose).await
    }

    async fn stage_documents(&self, paths: Vec<Option<PathBuf>>) -> Vec<Reply> {
        if paths.len() > MAX_IMPORT_BATCH {
            return vec![reply(422, "Choose up to 20 files in this import batch")];
        }
        let mut results = Vec::with_capacity(paths.len());
        for path in paths {
            let result = match path {
                Some(path) => self
                    .stage_picked_for(path, PickPurpose::Document)
                    .await
                    .unwrap_or_else(|error| reply(503, &error)),
                None => reply(422, "Choose a regular local file"),
            };
            results.push(result);
        }
        results
    }

    async fn stage_picked_for(&self, path: PathBuf, purpose: PickPurpose) -> Result<Reply, String> {
        let permit = self
            .pdf_lane
            .clone()
            .try_acquire_owned()
            .map_err(|_| "Rust PDF workers busy; pick not started".to_owned())?;
        let picked = self.picked.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let _permit = permit;
            let opened = match purpose {
                PickPurpose::PortableBackup | PickPurpose::Replacement => {
                    open_scoped_file(&path, purpose)
                }
                _ => open_import_file(&path, purpose == PickPurpose::Document),
            };
            let mut selected = match opened {
                Ok(selected) => selected,
                Err(mut refusal) => {
                    refusal.body["name"] = serde_json::json!(path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Selected PDF"));
                    return Ok(refusal);
                }
            };
            let pages = if purpose == PickPurpose::Replacement {
                let source = match read_picked_bytes(&mut selected.file, selected.size) {
                    Ok(bytes) => bytes,
                    Err(error) => return Ok(error),
                };
                let inspection = Store::inspect_replacement(&selected.filename, &source);
                if inspection.status != 200 {
                    return Ok(inspection);
                }
                inspection.body["pages"].as_u64()
            } else {
                None
            };
            let mut staged = picked
                .lock()
                .map_err(|_| "Rust picker unavailable".to_owned())?;
            if staged.len() >= MAX_PICKED_FILES {
                return Ok(reply(503, "Rust import queue is full"));
            }
            let token = format!("{:032x}", rand::random::<u128>());
            let result = Reply {
                status: 200,
                body: serde_json::json!({"token": token, "name": selected.filename,
                    "size": selected.size, "hash": selected.hash,"pages":pages}),
            };
            staged.insert(token, selected);
            Ok(result)
        })
        .await
        .map_err(|_| "Rust picker task failed".to_owned())?
    }

    #[cfg(test)]
    async fn import_picked(&self, matter_id: String, token: String) -> Result<Reply, String> {
        self.import_picked_with_options(matter_id, token, None, false)
            .await
    }

    async fn import_picked_with_options(
        &self,
        matter_id: String,
        token: String,
        password: Option<String>,
        acknowledged: bool,
    ) -> Result<Reply, String> {
        let picked = self.picked.clone();
        let measure_picks = picked.clone();
        let protected = self.protected;
        self.staged_pdf_request(
            move |session| {
                let selected = picked
                    .lock()
                    .map_err(|_| reply(500, "Rust picker unavailable"))?
                    .remove(&token)
                    .ok_or_else(|| reply(409, "Selected PDF expired; choose it again"))?;
                if !matches!(
                    selected.purpose,
                    PickPurpose::Document | PickPurpose::SyntheticDocument
                ) {
                    return Err(reply(422, "Choose a PDF, PNG or JPEG for document import"));
                }
                let target = session.capture_document_import(&matter_id);
                Ok((selected, token, target))
            },
            move |(mut selected, token, target)| {
                let bytes = read_picked_bytes(&mut selected.file, selected.size)?;
                if format!("{:x}", Sha256::digest(&bytes)) != selected.hash {
                    return Err(reply(422, "Selected PDF changed after selection"));
                }
                let modified = selected
                    .source_mtime
                    .and_then(matterdraft_backend::import_timestamp)
                    .unwrap_or_default();
                let options = if protected {
                    ImportOptions {
                        password: password.as_deref(),
                        acknowledge_signature_risk: acknowledged,
                        included: false,
                        source_file_mtime: &modified,
                    }
                } else {
                    ImportOptions::default()
                };
                let prepared = target?.measure(selected.filename.clone(), bytes, options);
                if let Err(error) = &prepared {
                    let detail = error.body["detail"].as_str().unwrap_or_default();
                    if (error.status == 422
                        && matches!(detail, "PDF password required" | "Incorrect PDF password"))
                        || (error.status == 409
                            && detail.starts_with("Protected PDF signature review required"))
                    {
                        measure_picks
                            .lock()
                            .map_err(|_| reply(500, "Rust picker unavailable"))?
                            .insert(token, selected);
                    }
                }
                prepared
            },
            Session::complete_document_import,
        )
        .await
    }

    async fn restore_picked_package(
        &self,
        token: String,
        credential: Option<String>,
        recovery: bool,
    ) -> Result<Reply, String> {
        let picked = self.picked.clone();
        let retry = picked.clone();
        let commit_retry = picked.clone();
        self.staged_pdf_request(
            move |session| {
                let Session::Protected(vault) = session else {
                    return Err(reply(
                        423,
                        "Open the protected Rust vault before restoring a backup",
                    ));
                };
                let selected = picked
                    .lock()
                    .map_err(|_| reply(500, "Rust picker unavailable"))?
                    .remove(&token)
                    .ok_or_else(|| reply(409, "Selected backup expired; choose it again"))?;
                if selected.purpose != PickPurpose::PortableBackup {
                    return Err(reply(422, "Choose a .cbmatter backup"));
                }
                Ok((selected, token, vault.capture_portable_restore()))
            },
            move |(mut selected, token, work)| match work?.compute_selected(
                &mut selected.file,
                selected.size,
                &selected.hash,
                credential.as_deref(),
                recovery,
            ) {
                Ok(prepared) => Ok((prepared, selected, token)),
                Err(refusal) => {
                    if !refusal.body["detail"]
                        .as_str()
                        .is_some_and(|detail| detail.starts_with("Selected backup "))
                    {
                        retry
                            .lock()
                            .map_err(|_| reply(500, "Rust picker unavailable"))?
                            .insert(token, selected);
                    }
                    Err(refusal)
                }
            },
            move |session, (prepared, selected, token)| {
                let Session::Protected(vault) = session else {
                    return Err(matterdraft_backend::unsupported());
                };
                let result = vault.complete_portable_restore(prepared);
                if result.status == 422 {
                    commit_retry
                        .lock()
                        .map_err(|_| reply(500, "Rust picker unavailable"))?
                        .insert(token, selected);
                }
                Ok(result)
            },
        )
        .await
    }

    async fn apply_replacement(
        &self,
        document: String,
        token: String,
        fields: Value,
    ) -> Result<Reply, String> {
        let picked = self.picked.clone();
        self.staged_pdf_request(
            move |session| {
                let Session::Protected(vault) = session else {
                    return Err(reply(
                        423,
                        "Open the protected Rust vault before replacing pages",
                    ));
                };
                let selected = picked
                    .lock()
                    .map_err(|_| reply(500, "Rust picker unavailable"))?
                    .remove(&token)
                    .filter(|selected| selected.purpose == PickPurpose::Replacement)
                    .ok_or_else(|| reply(409, "Selected replacement expired. Choose it again."))?;
                Ok((
                    selected,
                    vault.prepare_working_replacement(&document, &fields),
                ))
            },
            |(mut selected, work)| {
                let bytes = read_picked_bytes(&mut selected.file, selected.size)?;
                if format!("{:x}", Sha256::digest(&bytes)) != selected.hash {
                    return Err(reply(422, "Selected replacement changed after selection"));
                }
                work?.compute(&selected.filename, &bytes)
            },
            Session::publish_working_replacement,
        )
        .await
    }

    fn discard_picked(&self, token: &str) -> Result<Reply, String> {
        let removed = self
            .picked
            .lock()
            .map_err(|_| "Rust picker unavailable".to_owned())?
            .remove(token)
            .is_some();
        Ok(Reply {
            status: 200,
            body: serde_json::json!({"discarded": removed}),
        })
    }
}
fn job_reply<T: serde::Serialize>(value: Result<T, JobError>) -> Reply {
    match value {
        Ok(body) => Reply {
            status: 200,
            body: serde_json::to_value(body).unwrap(),
        },
        Err(err) => {
            let (status, detail) = match err {
                JobError::Busy => (503, "Rust job capacity exhausted"),
                JobError::Invalid => (422, "Invalid or oversized synthetic PDF input"),
                JobError::Missing => (404, "Job not found or expired after restart"),
                JobError::NotReady => (409, "Job result is not available"),
                JobError::Stopped => (503, "Rust jobs are shutting down"),
            };
            Reply {
                status,
                body: serde_json::json!({"detail":detail}),
            }
        }
    }
}
fn ensure_selected(backend: &BackendState) -> Result<(), String> {
    if !backend.connection()?.is_rust() {
        return Err("Rust development mode is not selected".into());
    }
    Ok(())
}
#[tauri::command]
pub(crate) async fn rust_request(
    app: AppHandle,
    backend: State<'_, BackendState>,
    method: String,
    path: String,
    body: Option<Value>,
    key_source: Option<String>,
    runtime: State<'_, RustRuntime>,
) -> Result<Reply, String> {
    ensure_selected(&backend)?;
    let body = body.unwrap_or(Value::Null);
    let route = path
        .split_once('?')
        .map_or(path.as_str(), |(route, _)| route);
    if method == "POST"
        && (processing_submission(route)
            || comparison_target(&path).is_some()
            || matter_resource(&path, "bundle-preview").is_some()
            || matter_resource(&path, "volume-planning").is_some()
            || TextPageTarget::from_path(&method, route).is_some()
            || RevisionTarget::from_path(&method, &path).is_some()
            || RedactionTarget::from_path(&method, route).is_some()
            || uploaded_replacement_document(&method, route).is_some()
            || annotation_image_document(&method, route).is_some()
            || route == "/matters/restore"
            || cover_import_matter(&method, route).is_some()
            || DocumentUploadTarget::from_path(&method, route, "").is_some())
        || method == "GET"
            && (matter_resource(&path, "portable").is_some()
                || matter_resource(&path, "validate").is_some()
                || variant_review_target(&path).is_some()
                || catalog_target(&path).is_some())
        || CompositionDraftTarget::from_path(&method, &path).is_some()
        || LabelReviewTarget::from_path(&method, &path).is_some()
        || (method != "GET" && MetadataReviewTarget::from_path(&method, &path).is_some())
        || (method != "GET" && OutputValidationTarget::from_path(&method, route, &body).is_some())
    {
        // Staged requests own their eventual publication even if the caller stops waiting.
        return tauri::async_runtime::spawn(async move {
            let runtime = app
                .try_state::<RustRuntime>()
                .ok_or("Rust store unavailable")?;
            dispatch_rust_request(&app, &runtime, method, path, body, key_source).await
        })
        .await
        .map_err(|_| "Rust staged request failed; verify state before retrying".to_owned())?;
    }
    dispatch_rust_request(&app, &runtime, method, path, body, key_source).await
}

async fn dispatch_rust_request(
    app: &AppHandle,
    runtime: &RustRuntime,
    method: String,
    path: String,
    body: Value,
    key_source: Option<String>,
) -> Result<Reply, String> {
    let cancelled_job = (method == "POST")
        .then(|| {
            path.strip_prefix("/jobs/")?
                .strip_suffix("/cancel")
                .filter(|id| !id.contains('/'))
                .map(str::to_owned)
        })
        .flatten();
    let response = runtime
        .request_with_keys(method, path, body, key_source)
        .await?;
    if response.status == 202 {
        if let Some(id) = response.body["id"].as_str() {
            emit_job_update(app, id);
        }
        runtime.processing_wake.store(true, Ordering::Release);
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            use tauri::Manager;
            if let Some(runtime) = app.try_state::<RustRuntime>() {
                if let Err(error) = runtime.process_jobs(|id| emit_job_update(&app, id)).await {
                    eprintln!("Rust processing queue: {error}");
                }
            }
        });
    }
    if response.status == 200 && response.body["cancel_requested"] == true {
        if let Some(id) = cancelled_job {
            emit_job_update(app, &id);
        }
    }
    Ok(response)
}
#[tauri::command]
pub(crate) async fn rust_open_pdf(
    backend: State<'_, BackendState>,
    runtime: State<'_, RustRuntime>,
    document_id: String,
    source: Artifact,
) -> Result<Reply, String> {
    ensure_selected(&backend)?;
    runtime.insert_pdf(document_id, source).await
}

#[tauri::command]
pub(crate) async fn rust_open_export_pdf(
    backend: State<'_, BackendState>,
    runtime: State<'_, RustRuntime>,
    matter_id: String,
    export_id: String,
    filename: String,
) -> Result<Reply, String> {
    ensure_selected(&backend)?;
    match runtime
        .export_snapshot(
            matter_id,
            export_id,
            filename,
            "inspection".into(),
            Value::Null,
        )
        .await?
    {
        Ok(snapshot) => runtime.register_snapshot(snapshot, None),
        Err(error) => Ok(error),
    }
}

#[tauri::command]
pub(crate) async fn rust_read_pdf(
    backend: State<'_, BackendState>,
    runtime: State<'_, RustRuntime>,
    token: String,
    begin: u64,
    end: u64,
) -> Result<tauri::ipc::Response, Reply> {
    ensure_selected(&backend).map_err(|error| reply(501, &error))?;
    runtime
        .read_pdf(&token, begin, end)
        .await
        .map(tauri::ipc::Response::new)
}

#[tauri::command]
pub(crate) fn rust_close_pdf(
    backend: State<'_, BackendState>,
    runtime: State<'_, RustRuntime>,
    token: String,
) -> Result<(), String> {
    ensure_selected(&backend)?;
    runtime.close_pdf(&token)
}

#[tauri::command]
pub(crate) async fn rust_pick_import_pdf(
    backend: State<'_, BackendState>,
    app: AppHandle,
    runtime: State<'_, RustRuntime>,
) -> Result<Reply, String> {
    ensure_selected(&backend)?;
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let extensions: &[&str] = if runtime.protected {
            &["pdf", "png", "jpg", "jpeg"]
        } else {
            &["pdf"]
        };
        app.dialog()
            .file()
            .add_filter("Matter documents", extensions)
            .pick_file(move |path| {
                let _ = sender.send(path);
            });
        let path = receiver
            .await
            .map_err(|_| "Native PDF picker failed".to_owned())?;
        match path {
            Some(path) => {
                runtime
                    .stage_picked(path.into_path().map_err(|e| e.to_string())?)
                    .await
            }
            None => Ok(Reply {
                status: 200,
                body: serde_json::json!({"cancelled": true}),
            }),
        }
    }
}

#[tauri::command]
pub(crate) async fn rust_pick_import_files(
    backend: State<'_, BackendState>,
    app: AppHandle,
    runtime: State<'_, RustRuntime>,
) -> Result<Vec<Reply>, String> {
    ensure_selected(&backend)?;
    if !runtime.protected {
        return Err("Open the protected Rust vault to import documents".into());
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Matter documents", &["pdf", "png", "jpg", "jpeg"])
        .pick_files(move |paths| {
            let _ = sender.send(paths);
        });
    let paths = receiver
        .await
        .map_err(|_| "Native file picker failed".to_owned())?
        .unwrap_or_default();
    Ok(runtime
        .stage_documents(
            paths
                .into_iter()
                .map(|path| path.into_path().ok())
                .collect(),
        )
        .await)
}

pub(crate) fn handle_native_drop(window: &tauri::Window, event: &tauri::WindowEvent) {
    let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop {
        paths, position, ..
    }) = event
    else {
        return;
    };
    let Some(runtime) = window.try_state::<RustRuntime>() else {
        return;
    };
    if !runtime.protected || !runtime.unlocked.load(Ordering::Acquire) {
        return;
    }
    let id = format!("{:032x}", rand::random::<u128>());
    // Capture only actual native drops. The WebView never supplies a path.
    if let Ok(mut drop) = runtime.native_drop.lock() {
        *drop = Some(NativeDrop {
            id: id.clone(),
            paths: paths.iter().take(MAX_IMPORT_BATCH + 1).cloned().collect(),
            created: Instant::now(),
        });
    } else {
        return;
    }
    let _ = window.emit_to(
        tauri::EventTarget::webview(window.label()),
        "matterdraft-document-drop",
        serde_json::json!({"id":id,"x":position.x,"y":position.y}),
    );
}

#[tauri::command]
pub(crate) async fn rust_claim_import_drop(
    backend: State<'_, BackendState>,
    runtime: State<'_, RustRuntime>,
    id: String,
) -> Result<Vec<Reply>, String> {
    ensure_selected(&backend)?;
    let paths = {
        let mut drop = runtime
            .native_drop
            .lock()
            .map_err(|_| "Native drop is unavailable")?;
        if !drop
            .as_ref()
            .is_some_and(|drop| drop.id == id && drop.created.elapsed() < Duration::from_secs(30))
        {
            return Ok(vec![reply(
                409,
                "This file drop expired. Drop the files again.",
            )]);
        }
        drop.take().expect("matching native drop").paths
    };
    Ok(runtime
        .stage_documents(paths.into_iter().map(Some).collect())
        .await)
}

#[tauri::command]
pub(crate) async fn rust_import_raw_document(
    app: AppHandle,
    backend: State<'_, BackendState>,
    runtime: State<'_, RustRuntime>,
    request: tauri::ipc::Request<'_>,
) -> Result<Reply, String> {
    ensure_selected(&backend)?;
    if !runtime.protected {
        return Ok(matterdraft_backend::unsupported());
    }
    let field = |name| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    let Some(matter) = field("x-matterdraft-matter-id")
        .filter(|id| {
            id.len() == 32
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .map(str::to_owned)
    else {
        return Ok(reply(404, "Matter not found"));
    };
    let Ok(admission) = runtime.admission.clone().try_acquire_owned() else {
        return Ok(reply(503, "Rust store busy; request not started"));
    };
    let lane = runtime.acquire_pdf_worker().await?;
    let captured = runtime
        .on_store_lane(None, move |session| {
            session
                .capture_document_import(&matter)
                .map(|target| (target, admission, lane))
        })
        .await?;
    let (target, admission, lane) = match captured {
        Ok(captured) => captured,
        Err(refusal) => return Ok(refusal),
    };
    let metadata = match RawDocumentUpload::parse(field("x-matterdraft-import").unwrap_or_default())
    {
        Ok(metadata) => metadata,
        Err(refusal) => return Ok(refusal),
    };
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Ok(reply(415, "Stream one PDF, PNG or JPEG file"));
    };
    let source = match copy_raw_document(bytes, "Import limit is 80 MB per file").await {
        Ok(source) => source,
        Err(refusal) => return Ok(refusal),
    };
    tauri::async_runtime::spawn(async move {
        let runtime = app
            .try_state::<RustRuntime>()
            .ok_or("Rust store unavailable")?;
        let measured = tauri::async_runtime::spawn_blocking(move || {
            metadata
                .compute(target, source)
                .map(|prepared| (prepared, admission, lane))
        })
        .await
        .map_err(|_| "Rust import worker failed; verify state before retrying")?;
        let (prepared, admission, lane) = match measured {
            Ok(measured) => measured,
            Err(refusal) => return Ok(refusal),
        };
        Ok(runtime
            .on_store_lane(Some(admission), move |session| {
                let _lane = lane;
                session.complete_document_import(prepared)
            })
            .await?
            .unwrap_or_else(|refusal| refusal))
    })
    .await
    .map_err(|_| "Rust import failed; verify state before retrying".to_owned())?
}

async fn copy_raw_document(bytes: &[u8], limit_detail: &str) -> Result<Vec<u8>, Reply> {
    if bytes.len() > DOCUMENT_IMPORT_MAX_BYTES {
        return Err(reply(413, limit_detail));
    }
    // The request body is borrowed. Copy it under admission before moving it
    // to the worker, and yield between chunks while the copy is in flight.
    let mut source = Vec::new();
    source
        .try_reserve_exact(bytes.len())
        .map_err(|_| reply(413, "Unable to allocate document upload memory"))?;
    for chunk in bytes.chunks(1024 * 1024) {
        source.extend_from_slice(chunk);
        tokio::task::yield_now().await;
    }
    Ok(source)
}

#[tauri::command]
pub(crate) async fn rust_replace_raw_document(
    app: AppHandle,
    backend: State<'_, BackendState>,
    runtime: State<'_, RustRuntime>,
    request: tauri::ipc::Request<'_>,
) -> Result<Reply, String> {
    ensure_selected(&backend)?;
    if !runtime.protected {
        return Ok(reply(
            423,
            "Open the protected Rust vault before replacing pages",
        ));
    }
    let field = |name| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    let metadata =
        match RawReplacementUpload::parse(field("x-matterdraft-replacement").unwrap_or_default()) {
            Ok(metadata) => metadata,
            Err(refusal) => return Ok(refusal),
        };
    let Some(document) = field("x-matterdraft-document-id")
        .filter(|id| {
            id.len() == 32
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .map(str::to_owned)
    else {
        return Ok(reply(404, "Document not found"));
    };
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Ok(reply(415, "Replacement requires a streamed binary file"));
    };
    let Ok(admission) = runtime.admission.clone().try_acquire_owned() else {
        return Ok(reply(503, "Rust store busy; request not started"));
    };
    let lane = runtime.acquire_pdf_worker().await?;
    let captured = runtime
        .on_store_lane(None, move |session| {
            let Session::Protected(vault) = session else {
                return Err(matterdraft_backend::unsupported());
            };
            vault
                .prepare_working_replacement(&document, &metadata.fields)
                .map(|work| (work, metadata.filename, admission, lane))
        })
        .await?;
    let (work, filename, admission, lane) = match captured {
        Ok(captured) => captured,
        Err(refusal) => return Ok(refusal),
    };
    let source = match copy_raw_document(bytes, "Replacement file limit is 80 MB").await {
        Ok(source) => source,
        Err(refusal) => return Ok(refusal),
    };
    tauri::async_runtime::spawn(async move {
        let runtime = app
            .try_state::<RustRuntime>()
            .ok_or("Rust store unavailable")?;
        let measured = tauri::async_runtime::spawn_blocking(move || {
            work.compute(&filename, &source)
                .map(|prepared| (prepared, admission, lane))
        })
        .await
        .map_err(|_| "Rust replacement worker failed; verify state before retrying")?;
        let (prepared, admission, lane) = match measured {
            Ok(measured) => measured,
            Err(refusal) => return Ok(refusal),
        };
        Ok(runtime
            .on_store_lane(Some(admission), move |session| {
                let _lane = lane;
                session.publish_working_replacement(prepared)
            })
            .await?
            .unwrap_or_else(|refusal| refusal))
    })
    .await
    .map_err(|_| "Rust replacement failed; verify state before retrying".to_owned())?
}

#[tauri::command]
pub(crate) async fn rust_import_picked_pdf(
    app: AppHandle,
    backend: State<'_, BackendState>,
    matter_id: String,
    token: String,
    password: Option<String>,
    acknowledge_signature_risk: Option<bool>,
) -> Result<Reply, String> {
    ensure_selected(&backend)?;
    tauri::async_runtime::spawn(async move {
        let runtime = app
            .try_state::<RustRuntime>()
            .ok_or("Rust store unavailable")?;
        runtime
            .import_picked_with_options(
                matter_id,
                token,
                password,
                acknowledge_signature_risk.unwrap_or(false),
            )
            .await
    })
    .await
    .map_err(|_| "Rust import failed; verify state before retrying".to_owned())?
}

#[tauri::command]
pub(crate) fn rust_discard_picked_pdf(
    backend: State<'_, BackendState>,
    token: String,
    runtime: State<'_, RustRuntime>,
) -> Result<Reply, String> {
    ensure_selected(&backend)?;
    runtime.discard_picked(&token)
}

#[tauri::command]
pub(crate) async fn rust_pick_portable_backup(
    backend: State<'_, BackendState>,
    app: AppHandle,
    runtime: State<'_, RustRuntime>,
) -> Result<Reply, String> {
    ensure_selected(&backend)?;
    if !runtime.protected {
        return Ok(reply(
            423,
            "Open the protected Rust vault before restoring a backup",
        ));
    }
    pick_scoped_source(
        &app,
        &runtime,
        PickPurpose::PortableBackup,
        "MatterDraft backup",
        &["cbmatter"],
    )
    .await
}

async fn pick_scoped_source(
    app: &AppHandle,
    runtime: &RustRuntime,
    purpose: PickPurpose,
    filter: &str,
    extensions: &[&str],
) -> Result<Reply, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter(filter, extensions)
        .pick_file(move |path| {
            let _ = sender.send(path);
        });
    let path = receiver
        .await
        .map_err(|_| "Native file picker failed".to_owned())?;
    match path {
        Some(path) => {
            runtime
                .stage_picked_for(
                    path.into_path().map_err(|error| error.to_string())?,
                    purpose,
                )
                .await
        }
        None => Ok(Reply {
            status: 200,
            body: serde_json::json!({"cancelled":true}),
        }),
    }
}

#[tauri::command]
pub(crate) async fn rust_pick_replacement(
    backend: State<'_, BackendState>,
    app: AppHandle,
    runtime: State<'_, RustRuntime>,
) -> Result<Reply, String> {
    ensure_selected(&backend)?;
    if !runtime.protected {
        return Ok(reply(
            423,
            "Open the protected Rust vault before replacing pages",
        ));
    }
    pick_scoped_source(
        &app,
        &runtime,
        PickPurpose::Replacement,
        "Replacement PDF or image",
        &["pdf", "png", "jpg", "jpeg"],
    )
    .await
}

#[tauri::command]
pub(crate) async fn rust_apply_replacement(
    app: AppHandle,
    backend: State<'_, BackendState>,
    document_id: String,
    token: String,
    fields: Value,
) -> Result<Reply, String> {
    ensure_selected(&backend)?;
    tauri::async_runtime::spawn(async move {
        let runtime = app
            .try_state::<RustRuntime>()
            .ok_or("Rust store unavailable")?;
        runtime.apply_replacement(document_id, token, fields).await
    })
    .await
    .map_err(|_| "Rust replacement failed; verify state before retrying".to_owned())?
}

#[tauri::command]
pub(crate) async fn rust_restore_picked_backup(
    app: AppHandle,
    backend: State<'_, BackendState>,
    token: String,
    credential: Option<String>,
    recovery: bool,
) -> Result<Reply, String> {
    ensure_selected(&backend)?;
    tauri::async_runtime::spawn(async move {
        let runtime = app
            .try_state::<RustRuntime>()
            .ok_or("Rust store unavailable")?;
        runtime
            .restore_picked_package(token, credential, recovery)
            .await
    })
    .await
    .map_err(|_| "Rust restore failed; verify state before retrying".to_owned())?
}

#[tauri::command]
pub(crate) async fn rust_job_status(
    backend: State<'_, BackendState>,
    id: String,
    runtime: State<'_, RustRuntime>,
) -> Result<Reply, String> {
    ensure_selected(&backend)?;
    Ok(job_reply(runtime.jobs.status(&id)))
}
#[tauri::command]
pub(crate) async fn rust_job_cancel(
    backend: State<'_, BackendState>,
    id: String,
    runtime: State<'_, RustRuntime>,
) -> Result<Reply, String> {
    ensure_selected(&backend)?;
    Ok(job_reply(runtime.jobs.cancel(&id)))
}
#[tauri::command]
pub(crate) async fn rust_job_result(
    backend: State<'_, BackendState>,
    id: String,
    runtime: State<'_, RustRuntime>,
) -> Result<Reply, String> {
    ensure_selected(&backend)?;
    Ok(job_reply(runtime.jobs.result(&id).map(
        |pages| serde_json::json!({"engine":"pdf_oxide","version":"0.3.78","pages":pages}),
    )))
}
#[tauri::command]
pub(crate) async fn rust_debug_job_start(
    backend: State<'_, BackendState>,
    delay_ms: u64,
    runtime: State<'_, RustRuntime>,
) -> Result<Reply, String> {
    ensure_selected(&backend)?;
    #[cfg(not(debug_assertions))]
    {
        let _ = (delay_ms, runtime);
        return Ok(matterdraft_backend::unsupported());
    }
    #[cfg(debug_assertions)]
    {
        let mut reply = job_reply(runtime.jobs.start(
            include_bytes!("../../../samples/01_petition.pdf").to_vec(),
            delay_ms,
        ));
        if reply.status == 200 {
            reply.status = 202;
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn runtime() -> RustRuntime {
        RustRuntime::new(
            std::env::temp_dir().join(format!("matterdraft-ipc-{}", rand::random::<u64>())),
        )
    }
    fn executor() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
    }

    #[test]
    fn initialization_reports_invalid_workspace_and_can_retry_before_opening() {
        let sandbox = runtime();
        let root = sandbox.root.join("protected");
        std::fs::create_dir_all(&root).unwrap();
        let rust = RustRuntime::protected(root.clone());
        executor().block_on(async {
            let failure = rust.initialize().await.unwrap_err();
            assert!(
                failure.contains("Unrecognized Rust protected store"),
                "{failure}"
            );
            assert!(!root.join("RUST_PROTECTED_STORE").exists());
            std::fs::remove_dir(&root).unwrap();
            rust.initialize().await.unwrap();
            let status = rust
                .request("GET".into(), "/vault/status".into(), Value::Null)
                .await
                .unwrap();
            assert_eq!(status.status, 200);
            assert_eq!(status.body["state"], "setup");
            assert_eq!(status.body["enabled"], true);
        });
        drop(rust);
        std::fs::remove_dir_all(sandbox.root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn protected_runtime_unlocks_after_restart_and_serves_scoped_pdf_ranges() {
        let sandbox = runtime();
        let root = sandbox.root.join("isolated-vault");
        let runtime = RustRuntime::protected(root.clone());
        let password = "Synthetic desktop vault password";
        let fixture = include_bytes!("../../../samples/01_petition.pdf");
        let (matter_id, document_id, recovery) = executor().block_on(async {
            assert_eq!(
                runtime
                    .request("GET".into(), "/health".into(), Value::Null)
                    .await
                    .unwrap()
                    .body["status"],
                "locked"
            );
            assert!(!root.exists());
            assert_eq!(
                runtime
                    .request("GET".into(), "/vault/status".into(), Value::Null)
                    .await
                    .unwrap()
                    .body["state"],
                "setup"
            );
            assert_eq!(
                runtime
                    .request("GET".into(), "/matters".into(), Value::Null)
                    .await
                    .unwrap()
                    .status,
                423
            );
            let created = runtime
                .request(
                    "POST".into(),
                    "/vault/setup".into(),
                    serde_json::json!({"password":password}),
                )
                .await
                .unwrap();
            assert_eq!(created.status, 200);
            assert!(runtime
                .request("GET".into(), "/profiles".into(), Value::Null)
                .await
                .unwrap()
                .body
                .is_array());
            let recovery = created.body["recovery_key"].as_str().unwrap().to_owned();
            let matter = runtime
                .request(
                    "POST".into(),
                    "/matters".into(),
                    serde_json::json!({"name":"Protected desktop fixture"}),
                )
                .await
                .unwrap();
            let imported = runtime
                .import_pdf(
                    matter.body["id"].as_str().unwrap().to_owned(),
                    "fixture.pdf".into(),
                    fixture.to_vec(),
                )
                .await
                .unwrap();
            assert_eq!(imported.status, 201, "{imported:?}");
            (
                matter.body["id"].as_str().unwrap().to_owned(),
                imported.body[0]["id"].as_str().unwrap().to_owned(),
                recovery,
            )
        });
        drop(runtime);
        let runtime = RustRuntime::protected(root);
        executor().block_on(async {
            assert_eq!(
                runtime
                    .insert_pdf(document_id.clone(), Artifact::Original)
                    .await
                    .unwrap()
                    .status,
                423
            );
            let unlocked = runtime
                .request(
                    "POST".into(),
                    "/vault/unlock".into(),
                    serde_json::json!({"credential":recovery,"recovery":true}),
                )
                .await
                .unwrap();
            assert_eq!(unlocked.status, 200);
            let expected = format!("{:x}", Sha256::digest(fixture));
            for source in [
                Artifact::Original,
                Artifact::Working { revision: Some(1) },
                Artifact::Retained {
                    revision: 1,
                    sha256: expected,
                },
            ] {
                let opened = runtime
                    .insert_pdf(document_id.clone(), source)
                    .await
                    .unwrap();
                assert_eq!(opened.status, 200);
                assert_eq!(opened.body["length"], fixture.len());
                let token = opened.body["token"].as_str().unwrap();
                assert_eq!(
                    runtime.read_pdf(token, 17, 311).await.unwrap(),
                    fixture[17..311]
                );
                assert_eq!(
                    runtime
                        .read_pdf(token, 0, 256 * 1024 + 1)
                        .await
                        .unwrap_err()
                        .status,
                    416
                );
                runtime.close_pdf(token).unwrap();
                assert_eq!(
                    runtime.read_pdf(token, 0, 10).await.unwrap_err().status,
                    409
                );
            }
            assert_eq!(
                runtime
                    .insert_pdf(
                        document_id.clone(),
                        Artifact::Working { revision: Some(99) }
                    )
                    .await
                    .unwrap()
                    .status,
                409
            );
            let preview = runtime
                .request(
                    "GET".into(),
                    format!("/documents/{document_id}/thumbnail?revision=1"),
                    Value::Null,
                )
                .await
                .unwrap();
            assert_eq!(preview.status, 200);
            assert!(preview.body["png"]
                .as_str()
                .unwrap()
                .starts_with("iVBORw0KGgo"));
            let split = runtime
                .measure_page_output(document_id.clone(), PageOutput::Split, "0".into(), true, |bytes| bytes)
                .await
                .unwrap()
                .unwrap();
            assert!(split.starts_with(b"PK\x03\x04"));
            let membership = format!("/matters/{matter_id}/membership");
            let current = runtime
                .request("GET".into(), membership.clone(), Value::Null)
                .await
                .unwrap();
            let included = runtime
                .request(
                    "PUT".into(),
                    membership,
                    serde_json::json!({
                        "included_ids":[document_id],"expected_snapshot":current.body["input_snapshot"]
                    }),
                )
                .await
                .unwrap();
            assert_eq!(included.status, 200, "{included:?}");
            let build = runtime
                .request("POST".into(), format!("/matters/{matter_id}/build"), Value::Null)
                .await
                .unwrap();
            assert_eq!(build.status, 202, "{build:?}");
            let updates = std::cell::RefCell::new(Vec::new());
            runtime.process_jobs(|id| updates.borrow_mut().push(id.to_owned())).await.unwrap();
            let build_id = build.body["id"].as_str().unwrap();
            let updates = updates.into_inner();
            assert!(updates.len() >= 3, "{updates:?}");
            assert!(updates.iter().all(|id| id == build_id), "{updates:?}");
            let completed = runtime
                .request("GET".into(), format!("/jobs/{build_id}"), Value::Null)
                .await
                .unwrap();
            assert_eq!(completed.body["status"], "completed", "{completed:?}");
            assert!(!completed.body["events"].as_array().unwrap().is_empty());
            assert!(completed.body.get("session_owner").is_none());
            let pending = runtime
                .request("POST".into(), format!("/matters/{matter_id}/build"), Value::Null)
                .await
                .unwrap();
            assert_eq!(pending.status, 202, "{pending:?}");
            runtime.stop_processing().await.unwrap();
            let stopped_id = pending.body["id"].as_str().unwrap();
            let stopped = runtime
                .request("GET".into(), format!("/jobs/{stopped_id}"), Value::Null)
                .await
                .unwrap();
            assert_eq!(stopped.body["status"], "cancelled");
            assert!(stopped.body["error"].is_null());
            let export = |operation: &str, options| {
                runtime.export_snapshot(
                    matter_id.clone(),
                    build_id.to_owned(),
                    "combined.pdf".into(),
                    operation.to_owned(),
                    options,
                )
            };
            let mut saved = export("file", Value::Null).await.unwrap().unwrap();
            let inspected = runtime
                .register_snapshot(export("inspection", Value::Null).await.unwrap().unwrap(), None)
                .unwrap();
            let token = inspected.body["token"].as_str().unwrap();
            let mut bytes = Vec::new();
            saved.copy_to(&mut bytes).unwrap();
            assert!(bytes.starts_with(b"%PDF-"));
            assert_eq!(runtime.read_pdf(token, 0, bytes.len() as u64).await.unwrap(), bytes);
            runtime.close_pdf(token).unwrap();
            let mut print = export("print", serde_json::json!({"pages":"2","orientation":"landscape","scale":"fit"}))
                .await
                .unwrap()
                .unwrap();
            let mut print_bytes = Vec::new();
            print.copy_to(&mut print_bytes).unwrap();
            assert!(print_bytes.starts_with(b"%PDF-"));
            assert_ne!(print_bytes, bytes);
            let rotated = runtime
                .request(
                    "POST".into(),
                    "/vault/rotate".into(),
                    serde_json::json!({"new_password":"Synthetic changed vault password"}),
                )
                .await
                .unwrap();
            assert_eq!(rotated.status, 200);
        });
        drop(runtime);
        std::fs::remove_dir_all(sandbox.root).unwrap();
    }
    #[test]
    fn lazy_store_and_direct_mutations() {
        let r = runtime();
        assert!(!r.root.exists());
        executor().block_on(async {
            assert_eq!(
                r.request("GET".into(), "/health".into(), Value::Null)
                    .await
                    .unwrap()
                    .status,
                200
            );
            assert!(!r.root.exists());
            let created = r
                .request(
                    "POST".into(),
                    "/matters".into(),
                    serde_json::json!({"name":"Async synthetic"}),
                )
                .await
                .unwrap();
            assert_eq!(created.status, 201);
            let listed = r
                .request("GET".into(), "/matters".into(), Value::Null)
                .await
                .unwrap();
            assert_eq!(listed.body[0], created.body);
            let matter_path = format!("/matters/{}", created.body["id"].as_str().unwrap());
            let added = r
                .request("POST".into(), format!("{matter_path}/parts"), Value::Null)
                .await
                .unwrap();
            assert_eq!(added.status, 201);
            assert_eq!(added.body["parts"], serde_json::json!([1, 2, 3, 4, 5]));
            let saved = r
                .request(
                    "PUT".into(),
                    format!("{matter_path}/filing-review"),
                    serde_json::json!({"items":[{"title":"Review synthetic cover","part":5}]}),
                )
                .await
                .unwrap();
            assert_eq!(saved.status, 200);
            let blocked = r
                .request(
                    "DELETE".into(),
                    format!("{matter_path}/parts/5"),
                    Value::Null,
                )
                .await
                .unwrap();
            assert_eq!(blocked.status, 409);
            let undone = r
                .request(
                    "POST".into(),
                    format!("{matter_path}/history/undo"),
                    Value::Null,
                )
                .await
                .unwrap();
            assert_eq!(undone.body["redo_label"], "Matter values");
        });
        let root = r.root.clone();
        drop(r);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn import_command_uses_single_database_lane_and_persists() {
        let r = runtime();
        executor().block_on(async {
            let matter = r
                .request(
                    "POST".into(),
                    "/matters".into(),
                    serde_json::json!({"name":"IPC import"}),
                )
                .await
                .unwrap();
            let id = matter.body["id"].as_str().unwrap().to_owned();
            let mut altered = include_bytes!("../../../samples/01_petition.pdf").to_vec();
            altered.extend_from_slice(b"\n/ByteRange [0 1 2 3]");
            let refused = r
                .import_pdf(id.clone(), "changed.pdf".into(), altered)
                .await
                .unwrap();
            assert_eq!(refused.status, 422);
            let imported = r
                .import_pdf(
                    id.clone(),
                    "petition.pdf".into(),
                    include_bytes!("../../../samples/01_petition.pdf").to_vec(),
                )
                .await
                .unwrap();
            assert_eq!(imported.status, 201, "{:?}", imported.body);
            let listed = r
                .request(
                    "GET".into(),
                    format!("/matters/{id}/documents"),
                    Value::Null,
                )
                .await
                .unwrap();
            assert_eq!(listed.body[0]["id"], imported.body[0]["id"]);
            assert_eq!(r.admission.available_permits(), 32);
            assert_eq!(r.database_lane.available_permits(), 1);
        });
        let root = r.root.clone();
        drop(r);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn protected_import_preserves_password_retry_original_and_cover_fields() {
        let sandbox = runtime();
        let runtime = RustRuntime::protected(sandbox.root.join("vault"));
        executor().block_on(async {
            let setup = runtime
                .request(
                    "POST".into(),
                    "/vault/setup".into(),
                    serde_json::json!({"password":"Synthetic import worker vault password"}),
                )
                .await
                .unwrap();
            assert_eq!(setup.status, 200, "{setup:?}");
            let matter = runtime
                .request(
                    "POST".into(),
                    "/matters".into(),
                    serde_json::json!({"name":"Import worker fixture"}),
                )
                .await
                .unwrap();
            let matter_id = matter.body["id"].as_str().unwrap();
            let fixture = include_bytes!("../../../samples/rust_pdf_corpus/encrypted.pdf");
            let source = sandbox.root.join("encrypted.pdf");
            std::fs::write(&source, fixture).unwrap();
            let selected = runtime.stage_picked(source).await.unwrap();
            assert_eq!(selected.status, 200, "{selected:?}");
            let token = selected.body["token"].as_str().unwrap();
            let occupied = runtime.pdf_lane.clone().try_acquire_many_owned(2).unwrap();
            let mut waiting =
                std::pin::pin!(runtime.import_picked(matter_id.to_owned(), token.to_owned()));
            assert!(
                tokio::time::timeout(Duration::from_millis(100), &mut waiting)
                    .await
                    .is_err()
            );
            let list = tokio::time::timeout(
                Duration::from_secs(2),
                runtime.request(
                    "GET".into(),
                    format!("/matters/{matter_id}/documents"),
                    Value::Null,
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(list.body.as_array().unwrap().len(), 0);
            drop(occupied);
            let missing = waiting.await.unwrap();
            assert_eq!(missing.status, 422, "{missing:?}");
            assert_eq!(missing.body["detail"], "PDF password required");
            let wrong = runtime
                .import_picked_with_options(
                    matter_id.to_owned(),
                    token.to_owned(),
                    Some("incorrect".into()),
                    false,
                )
                .await
                .unwrap();
            assert_eq!(wrong.status, 422, "{wrong:?}");
            assert_eq!(wrong.body["detail"], "Incorrect PDF password");
            let imported = runtime
                .import_picked_with_options(
                    matter_id.to_owned(),
                    token.to_owned(),
                    Some("synthetic-user".into()),
                    true,
                )
                .await
                .unwrap();
            assert_eq!(imported.status, 201, "{imported:?}");
            let document = imported.body[0]["id"].as_str().unwrap();
            assert_eq!(imported.body[0]["bundle_included"], 0);
            let mut original = runtime
                .open_snapshot(document.to_owned(), Artifact::Original)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(original.read_range(0, original.bytes).unwrap(), fixture);
            let info = runtime
                .request(
                    "GET".into(),
                    format!("/documents/{document}/original-info"),
                    Value::Null,
                )
                .await
                .unwrap();
            assert_eq!(info.status, 200, "{info:?}");
            assert_eq!(info.body["password_protected"], true);
            assert!(info.body["page_count"].is_null());
            let mut working = runtime
                .open_snapshot(document.to_owned(), Artifact::Working { revision: Some(1) })
                .await
                .unwrap()
                .unwrap();
            assert_ne!(working.read_range(0, working.bytes).unwrap(), fixture);

            let id = matter_id.to_owned();
            let work = runtime
                .with_store(move |session| {
                    let Session::Protected(vault) = session else {
                        unreachable!()
                    };
                    vault.capture_cover_import(&id)
                })
                .await
                .unwrap()
                .unwrap();
            let changed = runtime
                .request(
                    "PATCH".into(),
                    format!("/matters/{matter_id}"),
                    serde_json::json!({"name":"Changed cover name"}),
                )
                .await
                .unwrap();
            assert_eq!(changed.status, 200, "{changed:?}");
            let obsolete = work.compute().unwrap();
            let refused = runtime
                .with_store(move |session| {
                    let Session::Protected(vault) = session else {
                        unreachable!()
                    };
                    Ok(vault.complete_cover_import(obsolete))
                })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(refused.status, 409, "{refused:?}");
            let cover = runtime
                .request(
                    "POST".into(),
                    format!("/matters/{matter_id}/cover"),
                    Value::Null,
                )
                .await
                .unwrap();
            assert_eq!(cover.status, 201, "{cover:?}");
            assert_eq!(cover.body["title"], "Draft matter cover sheet");
            assert_eq!(cover.body["ocr_status"], "native_text");
            assert_eq!(cover.body["bundle_included"], 1);
        });
        drop(runtime);
        std::fs::remove_dir_all(&sandbox.root).unwrap();
    }

    #[test]
    fn native_picker_keeps_bytes_out_of_ipc_and_rechecks_source() {
        let r = runtime();
        let source = r.root.with_extension("picked.pdf");
        let fixture = include_bytes!("../../../samples/01_petition.pdf");
        std::fs::write(&source, fixture).unwrap();
        let source_hash = format!("{:x}", Sha256::digest(fixture));
        executor().block_on(async {
            let matter = r
                .request(
                    "POST".into(),
                    "/matters".into(),
                    serde_json::json!({"name":"Native picker fixture"}),
                )
                .await
                .unwrap();
            let id = matter.body["id"].as_str().unwrap().to_owned();
            let selected = r.stage_picked(source.clone()).await.unwrap();
            assert_eq!(selected.status, 200);
            assert_eq!(selected.body["hash"], source_hash);
            let token = selected.body["token"].as_str().unwrap().to_owned();
            let mut altered = fixture.to_vec();
            altered[100] ^= 1;
            std::fs::write(&source, &altered).unwrap();
            assert_eq!(
                r.import_picked(id.clone(), token).await.unwrap().status,
                422
            );
            assert_eq!(
                r.request(
                    "GET".into(),
                    format!("/matters/{id}/documents"),
                    Value::Null
                )
                .await
                .unwrap()
                .body
                .as_array()
                .unwrap()
                .len(),
                0
            );
            assert_eq!(r.stage_picked(source.clone()).await.unwrap().status, 422);
            std::fs::write(&source, fixture).unwrap();
            let selected = r.stage_picked(source.clone()).await.unwrap();
            let token = selected.body["token"].as_str().unwrap().to_owned();
            assert_eq!(
                r.import_picked(id.clone(), token.clone())
                    .await
                    .unwrap()
                    .status,
                201
            );
            assert_eq!(r.import_picked(id, token).await.unwrap().status, 409);
        });
        assert_eq!(std::fs::read(&source).unwrap(), fixture);
        std::fs::write(&source, vec![0_u8; 4 * 1024 * 1024 + 1]).unwrap();
        assert_eq!(open_picked_file(&source).err().unwrap().status, 413);
        #[cfg(unix)]
        {
            let alias = r.root.with_extension("alias.pdf");
            std::os::unix::fs::symlink(&source, &alias).unwrap();
            assert_eq!(open_picked_file(&alias).err().unwrap().status, 422);
            std::fs::remove_file(alias).unwrap();
        }
        std::fs::remove_file(source).unwrap();
        std::fs::remove_dir_all(r.root).unwrap();
    }
    #[test]
    fn waiting_for_database_yields_and_health_stays_available() {
        let r = runtime();
        let held = r.database_lane.clone().try_acquire_owned().unwrap();
        executor().block_on(async {
            // Single-thread executor: the timer cannot run if the request blocks its thread.
            let pending = r.request("GET".into(), "/matters".into(), Value::Null);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(30), pending)
                    .await
                    .is_err()
            );
            assert_eq!(
                r.request("GET".into(), "/health".into(), Value::Null)
                    .await
                    .unwrap()
                    .status,
                200
            );
        });
        assert_eq!(r.admission.available_permits(), 32);
        assert!(!r.root.exists());
        drop(held);
    }
    #[test]
    fn waiting_for_ocr_review_keeps_store_available() {
        let r = runtime();
        let held = r.ocr_review_lane.clone().try_acquire_owned().unwrap();
        executor().block_on(async {
            let requested = r.request(
                "GET".into(),
                "/documents/missing/ocr-review?page_index=0&revision=1".into(),
                Value::Null,
            );
            let mut requested = std::pin::pin!(requested);
            assert!(
                tokio::time::timeout(Duration::from_millis(30), &mut requested)
                    .await
                    .is_err()
            );
            assert_eq!(r.database_lane.available_permits(), 1);
            assert_eq!(r.ocr_review_admission.available_permits(), 7);
            assert_eq!(
                r.request("GET".into(), "/matters".into(), Value::Null)
                    .await
                    .unwrap()
                    .status,
                200
            );
            drop(held);
            assert_eq!(requested.await.unwrap().status, 404);
        });
        let root = r.root.clone();
        drop(r);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn waiting_for_screening_verification_keeps_store_available() {
        let r = runtime();
        executor().block_on(async {
            let matter = r.request("POST".into(), "/matters".into(), serde_json::json!({"name":"Synthetic screening queue"}))
                .await.unwrap();
            let matter = matter.body["id"].as_str().unwrap();
            let imported = r.import_pdf(matter.into(), "screening.pdf".into(), include_bytes!("../../../samples/01_petition.pdf").to_vec()).await.unwrap();
            assert_eq!(imported.status, 201);
            let membership = r.request("GET".into(), format!("/matters/{matter}/membership"), Value::Null)
                .await.unwrap();
            let included = r.request("PUT".into(), format!("/matters/{matter}/membership"), serde_json::json!({
                "expected_snapshot":membership.body["input_snapshot"], "included_ids":[imported.body[0]["id"]]
            })).await.unwrap();
            assert_eq!(included.status, 200);
            let preflight = r.request("GET".into(), format!("/matters/{matter}/validate"), Value::Null)
                .await.unwrap();
            let digest = preflight.body.as_array().unwrap().iter()
                .find_map(|row| row["effective_input_digest"].as_str()).unwrap();

            let held = r.pdf_lane.clone().try_acquire_many_owned(2).unwrap();
            let mut screen = std::pin::pin!(r.request("POST".into(), format!("/matters/{matter}/preflight/screen"),
                serde_json::json!({"expected_snapshot":digest})));
            assert!(tokio::time::timeout(Duration::from_millis(300), &mut screen).await.is_err());
            let unrelated = tokio::time::timeout(Duration::from_secs(2),
                r.request("GET".into(), "/matters".into(), Value::Null)).await.unwrap().unwrap();
            assert_eq!(unrelated.status, 200);
            assert_eq!(r.database_lane.available_permits(), 1);
            drop(held);
            assert_eq!(screen.await.unwrap().status, 202);
        });
        let root = r.root.clone();
        drop(r);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn page_revision_worker_waits_off_store_and_refuses_stale_commit() {
        let sandbox = runtime();
        let runtime = RustRuntime::protected(sandbox.root.join("vault"));
        executor().block_on(async {
            let setup = runtime
                .request(
                    "POST".into(),
                    "/vault/setup".into(),
                    serde_json::json!({"password":"Synthetic staged revision password"}),
                )
                .await
                .unwrap();
            assert_eq!(setup.status, 200, "{setup:?}");
            let matter = runtime
                .request(
                    "POST".into(),
                    "/matters".into(),
                    serde_json::json!({"name":"Staged revision"}),
                )
                .await
                .unwrap();
            let imported = runtime
                .import_pdf(
                    matter.body["id"].as_str().unwrap().to_owned(),
                    "unsigned.pdf".into(),
                    include_bytes!("../../../samples/rust_pdf_corpus/unsigned.pdf").to_vec(),
                )
                .await
                .unwrap();
            assert_eq!(imported.status, 201, "{imported:?}");
            let id = imported.body[0]["id"].as_str().unwrap();
            let route = format!("/documents/{id}/pages/rotate");
            let body = serde_json::json!({"expected_revision":1,"page_index":0,"degrees":90});
            let captured_route = route.clone();
            let captured_body = body.clone();
            let first = runtime
                .measure_pdf_request(
                    move |session| {
                        let Session::Protected(vault) = session else {
                            unreachable!()
                        };
                        vault.prepare_revision_edit(
                            RevisionTarget::from_path("POST", &captured_route).unwrap(),
                            captured_body,
                            &matterdraft_backend::RequestOrder::default(),
                        )
                    },
                    |work| work.compute(),
                )
                .await
                .unwrap()
                .unwrap();
            let MeasuredPdf {
                output,
                admission,
                lane,
            } = first;
            let second_slot = runtime.pdf_lane.clone().try_acquire_owned().unwrap();
            let mut queued = std::pin::pin!(runtime.request("POST".into(), route, body));
            assert!(
                tokio::time::timeout(Duration::from_millis(100), &mut queued)
                    .await
                    .is_err()
            );
            let unrelated = tokio::time::timeout(
                Duration::from_secs(2),
                runtime.request("GET".into(), "/matters".into(), Value::Null),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(unrelated.status, 200);
            drop(second_slot);
            let published = queued.await.unwrap();
            assert_eq!(published.status, 200, "{published:?}");
            assert_eq!(published.body["revision"], 2);
            let stale = runtime
                .on_store_lane(Some(admission), move |session| {
                    let _lane = lane;
                    let Session::Protected(vault) = session else {
                        unreachable!()
                    };
                    vault.complete_revision_edit(output)
                })
                .await
                .unwrap()
                .unwrap_err();
            assert_eq!(stale.status, 409);
            assert_eq!(
                stale.body["detail"],
                "Working PDF revision changed; reload the document"
            );

            let history_route = format!("/documents/{id}/revisions");
            let history = runtime
                .request("GET".into(), history_route.clone(), Value::Null)
                .await
                .unwrap();
            let revisions = history.body.as_array().unwrap();
            assert_eq!(revisions.len(), 2);
            let restore = runtime.request(
                "POST".into(), format!("{history_route}/1/restore"),
                serde_json::json!({"expected_revision":2,"expected_sha256":revisions[1]["sha256"]}),
            ).await.unwrap();
            assert_eq!(restore.status, 200, "{restore:?}");
            assert_eq!(restore.body["revision"], 3);

            let redact_path = format!("/documents/{id}/pages/redact");
            let invalid = serde_json::json!({
                "z":true,"page_index":-1,"expected_revision":0,"rect_x":1,
                "rect_y":0,"rect_width":0,"rect_height":1,"a":null
            });
            let rejected = runtime
                .request_with_keys(
                    "POST".into(),
                    redact_path.clone(),
                    invalid,
                    Some(r#"{"z":true,"page_index":-1,"expected_revision":0,"rect_x":1,"rect_y":0,"rect_width":0,"rect_height":1,"a":null}"#.into()),
                )
                .await
                .unwrap();
            assert_eq!(rejected.status, 422, "{rejected:?}");
            let issues = rejected.body["detail"].as_array().unwrap();
            assert_eq!(
                issues.iter().map(|issue| issue["loc"][1].as_str().unwrap()).collect::<Vec<_>>(),
                ["page_index", "expected_revision", "rect_x", "rect_width", "z", "a"],
            );
            assert_eq!(issues[3]["type"], "greater_than");

            let redact = runtime
                .request(
                    "POST".into(),
                    redact_path,
                    serde_json::json!({
                        "expected_revision":"3.0","page_index":"0",
                        "rect_x":"0.02","rect_y":0.02,"rect_width":0.05,"rect_height":0.05
                    }),
                )
                .await
                .unwrap();
            assert_eq!(redact.status, 200, "{redact:?}");
            assert_eq!(redact.body["revision"], 4);
            let records = runtime
                .request(
                    "GET".into(),
                    format!("/documents/{id}/pages/redactions"),
                    Value::Null,
                )
                .await
                .unwrap();
            let record = records.body[0]["id"].as_str().unwrap();
            assert_eq!(records.body[0]["rect_x"], 0.02);
            let removal_path = format!("/documents/{id}/pages/redactions/{record}/remove");
            let removed = runtime
                .request(
                    "POST".into(),
                    removal_path.clone(),
                    serde_json::json!({"expected_revision":"4"}),
                )
                .await
                .unwrap();
            assert_eq!(removed.status, 200, "{removed:?}");
            assert_eq!(removed.body["revision"], 5);
            let inactive = runtime
                .request("POST".into(), removal_path, serde_json::json!({"expected_revision":4}))
                .await
                .unwrap();
            assert_eq!(inactive.status, 404, "{inactive:?}");
            assert_eq!(inactive.body["detail"], "Applied redaction not found");

            for (path, body) in [
                (
                    format!("/documents/{id}/print"),
                    serde_json::json!({"expected_revision":5}),
                ),
                (format!("/documents/{id}/original/print"), Value::Null),
            ] {
                let printed = runtime.request("POST".into(), path, body).await.unwrap();
                assert_eq!(printed.status, 200, "{printed:?}");
                assert_eq!(printed.body["media_type"], "application/pdf");
                assert!(printed.body["data"].as_str().unwrap().starts_with("JVBERi"));
            }

            #[cfg(target_os = "macos")]
            {
                let preview = runtime
                    .register_print(
                        format!("/documents/{id}/print"),
                        serde_json::json!({"expected_revision":5,"scale":"actual","pages":"1"}),
                        false,
                    )
                    .await
                    .unwrap();
                assert_eq!(preview.status, 200, "{preview:?}");
                let token = preview.body["token"].as_str().unwrap();
                let length = preview.body["length"].as_u64().unwrap();
                let bytes = runtime.read_pdf(token, 0, length).await.unwrap();
                runtime
                    .with_print_bytes(token, move |printed, fit| {
                        assert_eq!(printed, bytes);
                        assert!(!fit);
                        Ok(())
                    })
                    .await
                    .unwrap();
                runtime.close_pdf(token).unwrap();
                assert!(runtime.with_print_bytes(token, |_, _| Ok(())).await.is_err());
            }

            let mut original = runtime
                .open_snapshot(id.to_owned(), Artifact::Original)
                .await
                .unwrap()
                .unwrap();
            let source = include_bytes!("../../../samples/rust_pdf_corpus/unsigned.pdf");
            assert_eq!(original.read_range(0, original.bytes).unwrap(), source);
        });
        drop(runtime);
        std::fs::remove_dir_all(sandbox.root.clone()).unwrap();
    }

    #[test]
    fn waiting_for_staged_pdf_requests_keeps_store_available() {
        let r = runtime();
        executor().block_on(async {
            for (method, path) in [
                ("POST", "/matters/missing/build"),
                ("POST", "/jobs/missing/retry"),
                ("POST", "/matters/missing/bundle-preview"),
                ("POST", "/matters/missing/volume-planning"),
                ("GET", "/matters/missing/validate"),
                ("GET", "/matters/missing/variants"),
                ("GET", "/matters/missing/supplements"),
                ("GET", "/matters/missing/variants/unknown/review"),
                ("POST", "/matters/missing/supplement-policy-preview"),
                ("PUT", "/matters/missing/variants/unknown"),
                ("POST", "/matters/missing/exports/unknown/compare"),
                ("POST", "/matters/missing/exports/unknown/compare-pages"),
                ("GET", "/documents/missing/pages/0/text-runs"),
                ("GET", "/documents/missing/thumbnail?revision=1"),
                ("GET", "/annotations/missing/asset"),
                ("POST", "/documents/missing/annotations/image"),
                ("POST", "/documents/missing/pages/0/output-marks-preview"),
                ("POST", "/matters/missing/front-matter-preview"),
                ("POST", "/matters/missing/numbering-preview"),
                ("GET", "/documents/missing/metadata-source?revision=1"),
                ("GET", "/documents/missing/search?q=fixture"),
                ("GET", "/matters/missing/search?q=fixture"),
                ("POST", "/matters/missing/metadata-suggestions"),
                ("POST", "/matters/missing/labels/preview"),
                ("GET", "/documents/missing/pages/extract?selection=0"),
                ("GET", "/documents/missing/pdf?editable=true"),
                ("HEAD", "/documents/missing/original"),
                ("POST", "/documents/missing/original/print"),
                ("POST", "/documents/missing/print"),
                (
                    "POST",
                    "/matters/missing/exports/unknown/volume-01.pdf/print",
                ),
                ("GET", "/documents/missing/pages/split?after=0"),
                ("POST", "/documents/missing/pages/paragraph-trial-preview"),
                ("POST", "/documents/missing/pages/paragraph-trial"),
                ("POST", "/documents/missing/pages/move-text-run"),
                ("POST", "/documents/missing/pages/replace-text-run"),
            ] {
                let held = r.pdf_lane.clone().try_acquire_many_owned(2).unwrap();
                let body = if path.ends_with("/bundle-preview") {
                    serde_json::json!({"max_pages_per_volume":300,"keep_groups_intact":true})
                } else if path.ends_with("/volume-planning") {
                    serde_json::json!({
                        "max_pages_per_volume":300,"keep_groups_intact":true,
                        "expected_snapshot":"0".repeat(64),"output_marks":{}
                    })
                } else if path.ends_with("/annotations/image") {
                    serde_json::json!({"expected_revision":1,"page_index":0,
                        "rect_x":0.1,"rect_y":0.2,"rect_width":0.3,"rect_height":0.4})
                } else if path.ends_with("/metadata-suggestions") {
                    serde_json::json!({"ids":["fixture"]})
                } else if path.ends_with("/labels/preview") {
                    serde_json::json!({"ids":["fixture"],"mode":"sequential"})
                } else if path.ends_with("/compare-pages") {
                    serde_json::json!({
                        "document_id":"fixture","expected_revision":1,
                        "prior_source_page":1,"current_source_page":1
                    })
                } else if path.ends_with("/move-text-run") {
                    serde_json::json!({
                        "expected_revision":1,"page_index":0,"span_index":0,
                        "expected_text":"Synthetic text","vertical_offset":0.01
                    })
                } else if path.starts_with("/documents/") && method == "POST" {
                    let mut body = serde_json::json!({
                        "expected_revision":1,"page_index":0,"span_index":0,
                        "expected_text":"Synthetic text"
                    });
                    if !path.ends_with("-preview") {
                        body["replacement"] = serde_json::json!("Revised synthetic text");
                    }
                    body
                } else {
                    Value::Null
                };
                let mut submission = std::pin::pin!(r.request(method.into(), path.into(), body));
                assert!(
                    tokio::time::timeout(Duration::from_millis(300), &mut submission)
                        .await
                        .is_err()
                );
                let unrelated = tokio::time::timeout(
                    Duration::from_secs(2),
                    r.request("GET".into(), "/matters".into(), Value::Null),
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(unrelated.status, 200);
                assert_eq!(r.database_lane.available_permits(), 1);
                drop(held);
                assert_eq!(submission.await.unwrap().status, 404);
            }
        });
        let root = r.root.clone();
        drop(r);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn blocking_work_yields_and_keeps_capacity_until_it_finishes() {
        let r = runtime();
        let store = r.store.clone();
        let (started, ready) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _guard = store.lock().unwrap();
            started.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(200));
        });
        ready.recv().unwrap();
        executor().block_on(async {
            let pending = r.request(
                "POST".into(),
                "/matters".into(),
                serde_json::json!({"name":"Late completion"}),
            );
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(30), pending)
                    .await
                    .is_err()
            );
            assert_eq!(r.database_lane.available_permits(), 0);
            assert_eq!(r.admission.available_permits(), 31);
            // Abandoning a started command is not cancellation: it still commits once.
            let lane = r.database_lane.clone().acquire_owned().await.unwrap();
            drop(lane);
            let list = r
                .request("GET".into(), "/matters".into(), Value::Null)
                .await
                .unwrap();
            assert_eq!(list.body.as_array().unwrap().len(), 1);
        });
        holder.join().unwrap();
        let root = r.root.clone();
        drop(r);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn saturation_refuses_before_work_and_pdf_failures_release_capacity() {
        let r = runtime();
        let held = r.admission.clone().try_acquire_many_owned(32).unwrap();
        let pdf_held = r.pdf_lane.clone().try_acquire_many_owned(2).unwrap();
        executor().block_on(async {
            assert_eq!(
                r.request(
                    "POST".into(),
                    "/matters".into(),
                    serde_json::json!({"name":"Refused"})
                )
                .await
                .unwrap()
                .status,
                503
            );
            assert!(r.extract(vec![]).await.unwrap_err().contains("busy"));
            drop(pdf_held);
            assert!(r.extract(b"not PDF".to_vec()).await.is_err());
            assert_eq!(r.pdf_lane.available_permits(), 2);
        });
        assert!(!r.root.exists());
        drop(held);
    }
    #[test]
    fn store_open_failure_is_explicit_and_releases_lane() {
        let r = runtime();
        std::fs::create_dir(&r.root).unwrap(); // Unmarked foreign root must be refused.
        executor().block_on(async {
            assert!(r
                .request("GET".into(), "/matters".into(), Value::Null)
                .await
                .unwrap_err()
                .contains("Unrecognized store"));
        });
        assert_eq!(r.admission.available_permits(), 32);
        assert_eq!(r.database_lane.available_permits(), 1);
        assert!(r.store.lock().unwrap().is_none());
        std::fs::remove_dir(r.root).unwrap();
    }
}
