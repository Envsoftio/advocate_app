//! Protected working-copy publication and revision history.
use super::artifacts::FileVersion;
use super::document_verification::{DocumentFileProof, DocumentFiles};
use super::page_tools::{
    request::{restore_request, RestoreRequest},
    signature_guard, PageEdit, PageRequest,
};
use super::*;
use pdf_oxide::PdfDocument;
use std::io;

const FIELDS: &str = "revision,relpath,sha256,created_at,page_count,ocr_status,ocr_provenance_json";
const MAX_WORKING_BYTES: u64 = 160 * 1024 * 1024;

pub(super) struct ValidatedRevisionPdf {
    bytes: Vec<u8>,
    pages: u64,
    digest: String,
}

impl ValidatedRevisionPdf {
    pub(super) fn prepare(bytes: Vec<u8>, pages: u64) -> Result<Self, Reply> {
        if bytes.len() as u64 > MAX_WORKING_BYTES {
            return Err(invalid(
                422,
                "Working PDF exceeds the revision editing limit",
            ));
        }
        let valid = PdfDocument::from_bytes(bytes.clone())
            .and_then(|pdf| {
                let count = pdf.page_count()?;
                if pdf.is_encrypted() && !pdf.is_authenticated() {
                    return Ok(false);
                }
                for index in 0..count {
                    let (x0, y0, x1, y1) = pdf.get_page_visible_box(index)?;
                    if ![x0, y0, x1, y1].iter().all(|value| value.is_finite())
                        || x1 <= x0
                        || y1 <= y0
                    {
                        return Ok(false);
                    }
                }
                Ok(count as u64 == pages)
            })
            .unwrap_or(false);
        if !valid {
            return Err(invalid(
                422,
                "Written PDF failed page-count or geometry validation",
            ));
        }
        let digest = sha256_hex(&bytes);
        Ok(Self {
            bytes,
            pages,
            digest,
        })
    }
}

pub(super) fn migrate(con: &Connection) -> rusqlite::Result<()> {
    con.execute_batch(
        "DROP INDEX IF EXISTS documents_matter_source;
         CREATE INDEX IF NOT EXISTS documents_source_lookup ON documents(matter_id,source_sha256);
         ALTER TABLE document_revisions RENAME TO revisions_v2;
         CREATE TABLE document_revisions (
             document_id TEXT NOT NULL, revision INTEGER NOT NULL, relpath TEXT NOT NULL,
             sha256 TEXT NOT NULL, created_at TEXT NOT NULL, page_count INTEGER NOT NULL,
             ocr_status TEXT NOT NULL, ocr_provenance_json TEXT NOT NULL,
             PRIMARY KEY(document_id,revision));
         INSERT INTO document_revisions
             SELECT r.*, json_extract(d.body,'$.page_count'), json_extract(d.body,'$.ocr_status'),
                 json_extract(d.body,'$.ocr_provenance_json')
             FROM revisions_v2 AS r JOIN documents AS d ON d.id=r.document_id;
         DROP TABLE revisions_v2;
         CREATE INDEX document_revision_paths ON document_revisions(document_id,relpath);
         PRAGMA user_version=3;",
    )
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Revision {
    number: u64,
    pub(super) relpath: String,
    pub(super) hash: String,
    created: String,
    pub(super) pages: u64,
    ocr: String,
    provenance: String,
}

impl Revision {
    fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            number: row.get(0)?,
            relpath: row.get(1)?,
            hash: row.get(2)?,
            created: row.get(3)?,
            pages: row.get(4)?,
            ocr: row.get(5)?,
            provenance: row.get(6)?,
        })
    }

    pub(super) fn fields(&self) -> Value {
        json!({"working_relpath":self.relpath,"page_count":self.pages,"ocr_status":self.ocr,
            "ocr_provenance_json":self.provenance})
    }

    pub(super) fn matches_working_metadata(&self, relpath: &str, doc: &Value) -> bool {
        self.relpath == relpath
            && doc["page_count"] == self.pages
            && doc["ocr_status"] == self.ocr
            && doc["ocr_provenance_json"] == self.provenance
    }

    fn insert(&self, con: &Connection, id: &str) -> rusqlite::Result<()> {
        con.execute(
            "INSERT INTO document_revisions VALUES (?,?,?,?,?,?,?,?)",
            params![
                id,
                self.number,
                self.relpath,
                self.hash,
                self.created,
                self.pages,
                self.ocr,
                self.provenance
            ],
        )?;
        Ok(())
    }
}

pub(super) fn record(
    con: &Connection,
    id: &str,
    revision: u64,
) -> rusqlite::Result<Option<Revision>> {
    con.query_row(
        &format!("SELECT {FIELDS} FROM document_revisions WHERE document_id=? AND revision=?"),
        params![id, revision],
        Revision::row,
    )
    .optional()
}

pub(super) fn file_name(key: &DataKey, id: &str, revision: u64) -> Result<String, String> {
    if revision == 1 {
        return Ok("revision-1.pdf".into());
    }
    let tag = vault::publication_tag(key, &format!("{id}:{revision}"))
        .map_err(|error| error.to_string())?;
    Ok(format!("revision-{revision}-{tag}.pdf"))
}

pub(super) fn valid_file_name(key: &DataKey, id: &str, name: &str) -> Result<bool, String> {
    let Some(number) = name
        .strip_prefix("revision-")
        .and_then(|name| name.strip_suffix(".pdf"))
        .and_then(|name| name.split('-').next())
        .and_then(|number| number.parse::<u64>().ok())
    else {
        return Ok(false);
    };
    Ok(number > 0 && file_name(key, id, number)? == name)
}

pub(super) fn revision_path(
    root: &Path,
    key: &DataKey,
    id: &str,
    retained: &Revision,
) -> Result<PathBuf, String> {
    let prefix = format!("documents-v1/{id}/");
    let name = retained
        .relpath
        .strip_prefix(&prefix)
        .ok_or("Invalid retained revision path")?;
    if !valid_file_name(key, id, name)? {
        return Err("Invalid retained revision path".into());
    }
    Ok(root.join(&retained.relpath))
}

pub(super) fn revision_file(
    root: &Path,
    key: &DataKey,
    id: &str,
    retained: &Revision,
) -> Result<PathBuf, String> {
    let path = revision_path(root, key, id, retained)?;
    if root.join("documents-v1").join(id).is_symlink() {
        return Err("Invalid retained revision path".into());
    }
    if !fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
        return Err("Retained revision file is unavailable".into());
    }
    Ok(path)
}

fn inspect_revision(
    root: &Path,
    key: &DataKey,
    id: &str,
    retained: &Revision,
    mut read: impl FnMut(&[u8]),
) -> Result<(u64, Option<FileVersion>), String> {
    let path = revision_file(root, key, id, retained)?;
    let file = open_document_file(&path)?;
    let version = FileVersion::read(&file).ok();
    let mut hash = Sha256::new();
    let mut length = 0;
    vault::visit_encrypted(&file, key, |chunk| {
        hash.update(chunk);
        length += chunk.len() as u64;
        if length <= MAX_WORKING_BYTES {
            read(chunk)
        }
    })
    .map_err(|error| error.to_string())?;
    if (version.is_some() && version != FileVersion::read(&file).ok())
        || vault::hex_bytes(&hash.finalize()).as_str() != retained.hash
    {
        return Err("Retained revision bytes changed; reload revision history".into());
    }
    Ok((length, version))
}

fn load_pdf(
    root: &Path,
    key: &DataKey,
    id: &str,
    retained: &Revision,
) -> Result<(Vec<u8>, Option<FileVersion>), String> {
    let mut buffer = Vec::new();
    let (length, version) = inspect_revision(root, key, id, retained, |chunk| {
        buffer.extend_from_slice(chunk)
    })?;
    if length > MAX_WORKING_BYTES {
        return Err("Working PDF exceeds the revision editing limit".into());
    }
    Ok((buffer, version))
}

pub(super) fn reconcile(
    con: &Connection,
    folder: &Path,
    key: &DataKey,
    id: &str,
) -> Result<(), String> {
    let receipt = publication_receipt(key, id)?;
    check_receipt(&folder.join(&receipt))?;
    let mut orphans = Vec::new();
    for entry in fs::read_dir(folder).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            return Err("Foreign document folder content refused".into());
        }
        if name == receipt || name == "original.pdf" {
            continue;
        }
        if !valid_file_name(key, id, &name)? {
            return Err("Foreign document folder content refused".into());
        }
        let relpath = format!("documents-v1/{id}/{name}");
        let referenced: Option<i64> = con
            .query_row(
                "SELECT 1 FROM document_revisions WHERE document_id=? AND relpath=? LIMIT 1",
                params![id, relpath],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if referenced.is_none() {
            if name == "revision-1.pdf" {
                return Err("First revision record is missing".into());
            }
            let current: bool = con
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM documents WHERE id=? AND working_relpath=?)",
                    params![id, relpath],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            if current {
                return Err("Current working revision record is missing".into());
            }
            orphans.push(entry.path());
        }
    }
    for orphan in orphans {
        fs::remove_file(orphan).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn verify_document(
    con: &Connection,
    root: &Path,
    key: &DataKey,
    id: &str,
    doc: &Value,
) -> Result<(), String> {
    super::document_verification::DocumentFiles::capture(con, root, Some(key), doc, id)?
        .authenticate()
        .map(|_| ())
}

pub(super) fn storage_size(
    con: &Connection,
    folder: &Path,
    key: &DataKey,
    id: &str,
) -> rusqlite::Result<u64> {
    let mut query =
        con.prepare("SELECT DISTINCT relpath FROM document_revisions WHERE document_id=?")?;
    let mut size = 0;
    let prefix = format!("documents-v1/{id}/");
    for row in query.query_map([id], |row| row.get::<_, String>(0))? {
        let path = row?;
        let name = path
            .strip_prefix(&prefix)
            .ok_or(rusqlite::Error::InvalidQuery)?;
        if !valid_file_name(key, id, name).map_err(string_sql)? {
            return Err(rusqlite::Error::InvalidQuery);
        }
        match fs::symlink_metadata(folder.join(name)) {
            Ok(metadata) if metadata.is_file() => size += metadata.len(),
            Ok(_) => return Err(rusqlite::Error::InvalidQuery),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(io_sql(error)),
        }
    }
    Ok(size)
}

enum Operation {
    Edit(PageRequest, PageEdit),
    Restore(u64, RestoreRequest),
}

enum RevisionAction {
    Edit(String),
    Restore(String),
}

pub struct RevisionTarget {
    document: String,
    action: RevisionAction,
}

impl RevisionTarget {
    pub fn from_path(method: &str, path: &str) -> Option<Self> {
        let route = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
        Self::select(method, &route)
    }

    fn select(method: &str, route: &[&str]) -> Option<Self> {
        let (document, action) = match (method, route) {
            (
                "POST",
                ["documents", id, "pages", tool @ ("text-overlay" | "image-overlay" | "rotate" | "delete" | "reorder"
                | "insert" | "replace" | "replace-document" | "batch" | "crop")],
            ) => (id, RevisionAction::Edit((*tool).to_owned())),
            ("POST", ["documents", id, "replace"]) => {
                (id, RevisionAction::Edit("replace-document".into()))
            }
            ("POST", ["documents", id, "revisions", revision, "restore"]) => {
                (id, RevisionAction::Restore((*revision).to_owned()))
            }
            _ => return None,
        };
        Some(Self {
            document: (*document).to_owned(),
            action,
        })
    }
}

struct RevisionDocument {
    id: String,
    revision: Revision,
    files: DocumentFiles,
}

struct RevisionProof {
    id: String,
    revision: Revision,
    files: DocumentFileProof,
}

impl RevisionDocument {
    fn capture(store: &Store, id: String, doc: &Value) -> Result<Self, Reply> {
        let revision = doc["revision"]
            .as_u64()
            .ok_or_else(revision_metadata_error)?;
        let revision = record(&store.con, &id, revision)
            .map_err(|_| revision_store_error())?
            .ok_or_else(revision_metadata_error)?;
        let files = DocumentFiles::capture(&store.con, &store.root, store.key.as_ref(), doc, &id)
            .map_err(|error| invalid(409, error))?;
        Ok(Self {
            id,
            revision,
            files,
        })
    }

    fn authenticate(self, read_pdf: bool) -> Result<(RevisionProof, Vec<u8>), Reply> {
        let mut bytes = Vec::new();
        let (size, files) = self
            .files
            .authenticate_with(|chunk| {
                if read_pdf && (bytes.len() + chunk.len()) as u64 <= MAX_WORKING_BYTES {
                    bytes.extend_from_slice(chunk);
                }
            })
            .map_err(|error| invalid(409, error))?;
        if read_pdf && size > MAX_WORKING_BYTES {
            return Err(invalid(
                409,
                "Working PDF exceeds the revision editing limit",
            ));
        }
        Ok((
            RevisionProof {
                id: self.id,
                revision: self.revision,
                files,
            },
            bytes,
        ))
    }
}

impl RevisionProof {
    fn check_current(
        &self,
        con: &Connection,
        root: &Path,
        key: &DataKey,
        doc: &Value,
        stale: &str,
    ) -> Result<(), Reply> {
        if doc["revision"] != self.revision.number
            || record(con, &self.id, self.revision.number)
                .map_err(|_| revision_store_error())?
                .as_ref()
                != Some(&self.revision)
        {
            return Err(invalid(409, stale));
        }
        let current = DocumentFiles::capture(con, root, Some(key), doc, &self.id)
            .map_err(|error| invalid(409, error))?;
        if !self.files.matches(&current) {
            return Err(invalid(409, stale));
        }
        self.files
            .check_current()
            .map_err(|error| invalid(409, error))
    }
}

pub struct RevisionEditWork {
    captured: Result<RevisionEditSnapshot, Reply>,
    image_upload: Option<Value>,
}

struct RevisionEditSnapshot {
    root: PathBuf,
    key: DataKey,
    document: RevisionDocument,
    source: Option<RevisionDocument>,
    operation: Operation,
    retained: Option<Revision>,
    reserved: Option<(usize, Vec<Value>)>,
    acknowledged: bool,
}

pub struct MeasuredRevisionEdit {
    document: RevisionProof,
    source: Option<RevisionProof>,
    reserved: Option<(usize, Vec<Value>)>,
    acknowledged: bool,
    after: Revision,
    content: ValidatedRevisionPdf,
    retention: annotations::Retention,
    retained: Option<RetainedRevisionProof>,
}

struct RetainedRevisionProof {
    version: Option<FileVersion>,
}

impl RevisionEditWork {
    pub fn compute(self) -> Result<MeasuredRevisionEdit, Reply> {
        // Working-image upload errors take precedence over revision errors.
        // Validation and decoding stay on the bounded PDF worker.
        let image = self
            .image_upload
            .as_ref()
            .map(|body| {
                annotations::image_upload(body, annotations::ImageUploadTarget::WorkingCopy)
            })
            .transpose()?;
        self.captured?.compute(image)
    }
}

impl RevisionEditSnapshot {
    fn compute(
        self,
        image: Option<annotations::ImageUpload>,
    ) -> Result<MeasuredRevisionEdit, Reply> {
        let (document, working) = self
            .document
            .authenticate(matches!(self.operation, Operation::Edit(..)))?;
        let mut source = None;
        let mut retained_version = None;
        let mut after = document.revision.clone();
        let (bytes, retention) = match self.operation {
            Operation::Edit(_, edit) => {
                let related = if let Some(input) = self.source {
                    let (proof, bytes) = input.authenticate(true)?;
                    let pages = proof.revision.pages as usize;
                    source = Some(proof);
                    Some((bytes, pages))
                } else {
                    None
                };
                let retention = match &edit {
                    PageEdit::TextOverlay(..) => annotations::Retention::All,
                    PageEdit::ImageOverlay(..) => annotations::Retention::Pdf,
                    _ => annotations::Retention::None,
                };
                if edit.modifies_ocr_status() {
                    after.ocr = "modified".into();
                }
                let reserved = self.reserved.as_ref().map_or(&[][..], |(_, marks)| marks);
                let (bytes, pages) = edit.apply(
                    working,
                    after.pages as usize,
                    related,
                    &document.id,
                    reserved,
                    image,
                )?;
                after.pages = pages as u64;
                (bytes, retention)
            }
            Operation::Restore(revision, request) => {
                let retained = self
                    .retained
                    .ok_or_else(|| invalid(409, "Retained revision not found"))?;
                if retained.hash != request.expected_sha256 {
                    return Err(invalid(
                        409,
                        "Retained revision bytes changed; reload revision history",
                    ));
                }
                if revision == after.number {
                    return Err(invalid(409, "This revision is already current"));
                }
                let (bytes, version) = load_pdf(&self.root, &self.key, &document.id, &retained)
                    .map_err(|error| invalid(409, error))?;
                retained_version = Some(RetainedRevisionProof { version });
                after = retained;
                (bytes, annotations::Retention::None)
            }
        };
        let content = ValidatedRevisionPdf::prepare(bytes, after.pages)?;
        Ok(MeasuredRevisionEdit {
            document,
            source,
            reserved: self.reserved,
            acknowledged: self.acknowledged,
            after,
            content,
            retention,
            retained: retained_version,
        })
    }
}

fn revision_store_error() -> Reply {
    invalid(500, "Rust matter transaction failed; no changes committed")
}

fn revision_metadata_error() -> Reply {
    invalid(409, "Document or working revision metadata mismatch")
}

fn reserved_image_marks(
    con: &Connection,
    doc: &Value,
    id: &str,
    revision: u64,
    page: usize,
) -> Result<Vec<Value>, Reply> {
    let matter = doc["matter_id"]
        .as_str()
        .ok_or_else(revision_metadata_error)?;
    Ok(annotations::raw(con, matter)
        .map_err(|_| revision_store_error())?
        .into_iter()
        .filter(|mark| {
            mark["kind"] == "image"
                && mark["document_id"] == id
                && mark["target_revision"] == revision
                && mark["page_index"] == page
        })
        .collect())
}

pub(super) fn active_document(con: &Connection, id: &str) -> rusqlite::Result<Option<Value>> {
    let raw: Option<String> = con
        .query_row(
            "SELECT d.body FROM documents d JOIN matters m ON m.id=d.matter_id
         WHERE d.id=? AND d.archived_at IS NULL AND json_extract(m.body,'$.archived_at') IS NULL",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| decode(&raw)).transpose()
}

fn invalid(status: u16, detail: impl Into<String>) -> Reply {
    reply(status, json!({"detail":detail.into()}))
}

fn working_state(doc: &Value, retained: &Revision, annotation_ids: &[String]) -> Value {
    json!({"id":doc["id"],"fields":retained.fields(),"preserved_annotation_ids":annotation_ids})
}

fn update_working(
    con: &Connection,
    doc: &mut Value,
    retained: &Revision,
    stamp: &str,
) -> rusqlite::Result<()> {
    let fields = retained.fields();
    let data = doc.as_object_mut().ok_or(rusqlite::Error::InvalidQuery)?;
    for field in ["page_count", "ocr_status", "ocr_provenance_json"] {
        data.insert(field.into(), fields[field].clone());
    }
    data.insert("revision".into(), json!(retained.number));
    data.insert("updated_at".into(), json!(stamp));
    con.execute(
        "UPDATE documents SET body=?,working_relpath=? WHERE id=?",
        params![doc.to_string(), retained.relpath, doc["id"].as_str()],
    )?;
    Ok(())
}

impl Store {
    pub(super) fn revision_request(
        &mut self,
        method: &str,
        route: &[&str],
        body: &Value,
        order: &crate::RequestOrder,
    ) -> rusqlite::Result<Option<Reply>> {
        if let ("GET", ["documents", id, "revisions"]) = (method, route) {
            return self.retained_revisions(id).map(Some);
        }
        let Some(target) = RevisionTarget::select(method, route) else {
            return Ok(None);
        };
        Ok(Some(
            self.prepare_revision_edit(target, body.clone(), order)
                .and_then(RevisionEditWork::compute)
                .and_then(|measured| self.complete_revision_edit(measured))
                .unwrap_or_else(|refusal| refusal),
        ))
    }

    fn retained_revisions(&self, id: &str) -> rusqlite::Result<Reply> {
        let Some(doc) = active_document(&self.con, id)? else {
            return Ok(invalid(404, "Document not found"));
        };
        let key = self.key.as_ref().expect("protected revision route");
        let mut query = self.con.prepare(&format!(
            "SELECT {FIELDS} FROM document_revisions WHERE document_id=? ORDER BY revision DESC"
        ))?;
        let mut results = Vec::new();
        for row in query.query_map([id], Revision::row)? {
            let row = row?;
            let length = inspect_revision(&self.root, key, id, &row, |_| {})
                .ok()
                .map(|(length, _)| length);
            results.push(
                json!({"revision":row.number,"created_at":row.created,"page_count":row.pages,
                "available":length.is_some(),"sha256":length.map(|_|row.hash),"bytes":length,
                "current":doc["revision"] == row.number}),
            );
        }
        Ok(reply(200, json!(results)))
    }

    pub fn prepare_revision_edit(
        &self,
        target: RevisionTarget,
        body: Value,
        order: &crate::RequestOrder,
    ) -> Result<RevisionEditWork, Reply> {
        let image = matches!(&target.action, RevisionAction::Edit(tool) if tool == "image-overlay")
            && self.key.is_some();
        let captured = self.capture_revision_edit(target, &body, order);
        if image {
            Ok(RevisionEditWork {
                captured,
                image_upload: Some(body),
            })
        } else {
            Ok(RevisionEditWork {
                captured: Ok(captured?),
                image_upload: None,
            })
        }
    }

    fn capture_revision_edit(
        &self,
        target: RevisionTarget,
        body: &Value,
        order: &crate::RequestOrder,
    ) -> Result<RevisionEditSnapshot, Reply> {
        let key = self.key.as_ref().ok_or_else(crate::unsupported)?;
        let RevisionTarget {
            document: id,
            action,
        } = target;
        let operation = match action {
            RevisionAction::Edit(tool) => {
                let (request, edit) = PageEdit::parse(&tool, body, order)?;
                Operation::Edit(request, edit)
            }
            RevisionAction::Restore(revision) => {
                let (revision, request) = restore_request(&revision, body)?;
                Operation::Restore(revision, request)
            }
        };
        let doc = active_document(&self.con, &id)
            .map_err(|_| revision_store_error())?
            .ok_or_else(|| invalid(404, "Document not found"))?;
        let current = doc["revision"]
            .as_u64()
            .ok_or_else(revision_metadata_error)?;
        let (expected, acknowledged) = match &operation {
            Operation::Edit(request, _) => (
                request.expected_revision,
                request.acknowledge_signature_risk,
            ),
            Operation::Restore(_, request) => (
                Some(request.expected_revision),
                request.acknowledge_signature_risk,
            ),
        };
        let stale = expected.is_some_and(|revision| revision != current);
        let working_image = matches!(&operation, Operation::Edit(_, PageEdit::ImageOverlay(..)));
        if stale && !working_image {
            return Err(invalid(
                409,
                "Working PDF revision changed; reload the document",
            ));
        }
        let related = match &operation {
            Operation::Edit(request, edit) => edit
                .source()
                .map(|source| {
                    let doc_source = active_document(&self.con, source)
                        .map_err(|_| revision_store_error())?
                        .ok_or_else(|| invalid(404, "Document not found"))?;
                    if doc_source["matter_id"] != doc["matter_id"] {
                        return Err(invalid(422, "Source must be in the same matter"));
                    }
                    if request
                        .expected_source_revision
                        .is_some_and(|revision| doc_source["revision"] != revision)
                    {
                        return Err(invalid(
                            409,
                            "Source PDF revision changed; reopen the page preview",
                        ));
                    }
                    Ok((source.to_owned(), doc_source))
                })
                .transpose()?,
            Operation::Restore(..) => None,
        };
        signature_guard(
            std::iter::once(&doc).chain(related.iter().map(|(_, source)| source)),
            acknowledged,
            "transforming this working copy",
        )?;
        if stale {
            return Err(invalid(409, "Document changed; place the image again"));
        }
        let reserved = if let Operation::Edit(_, PageEdit::TextOverlay(page, ..)) = &operation {
            Some((
                *page,
                reserved_image_marks(&self.con, &doc, &id, current, *page)?,
            ))
        } else {
            None
        };
        let retained = if let Operation::Restore(number, _) = &operation {
            record(&self.con, &id, *number).map_err(|_| revision_store_error())?
        } else {
            None
        };
        let document = RevisionDocument::capture(self, id, &doc)?;
        let source = related
            .map(|(id, doc)| RevisionDocument::capture(self, id, &doc))
            .transpose()?;
        Ok(RevisionEditSnapshot {
            root: self.root.clone(),
            key: key.clone(),
            document,
            source,
            operation,
            retained,
            reserved,
            acknowledged,
        })
    }

    pub fn complete_revision_edit(
        &mut self,
        measured: MeasuredRevisionEdit,
    ) -> Result<Reply, Reply> {
        let MeasuredRevisionEdit {
            document,
            source,
            acknowledged,
            reserved,
            after,
            content,
            retention,
            retained,
        } = measured;
        let id = &document.id;
        let key = self.key.as_ref().ok_or_else(crate::unsupported)?;
        let tx = self
            .con
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| revision_store_error())?;
        let doc = active_document(&tx, id)
            .map_err(|_| revision_store_error())?
            .ok_or_else(|| invalid(404, "Document not found"))?;
        let source_doc = source
            .as_ref()
            .map(|source| {
                active_document(&tx, &source.id)
                    .map_err(|_| revision_store_error())?
                    .ok_or_else(|| invalid(404, "Document not found"))
            })
            .transpose()?;
        if source_doc
            .as_ref()
            .is_some_and(|source| source["matter_id"] != doc["matter_id"])
        {
            return Err(invalid(422, "Source must be in the same matter"));
        }
        signature_guard(
            std::iter::once(&doc).chain(source_doc.iter()),
            acknowledged,
            "transforming this working copy",
        )?;
        document.check_current(
            &tx,
            &self.root,
            key,
            &doc,
            "Working PDF revision changed; reload the document",
        )?;
        if let Some((source, source_doc)) = source.as_ref().zip(source_doc.as_ref()) {
            source.check_current(
                &tx,
                &self.root,
                key,
                source_doc,
                "Source PDF revision changed; reopen the page preview",
            )?;
        }
        if let Some((page, marks)) = reserved {
            if marks != reserved_image_marks(&tx, &doc, id, document.revision.number, page)? {
                return Err(invalid(
                    409,
                    "Image marks changed; reload the page before adding text",
                ));
            }
        }
        if let Some(RetainedRevisionProof { version }) = retained {
            let stale = || {
                invalid(
                    409,
                    "Retained revision bytes changed; reload revision history",
                )
            };
            if record(&tx, id, after.number)
                .map_err(|_| revision_store_error())?
                .as_ref()
                != Some(&after)
            {
                return Err(stale());
            }
            let path =
                revision_file(&self.root, key, id, &after).map_err(|error| invalid(409, error))?;
            let file = open_document_file(&path).map_err(|error| invalid(409, error))?;
            if let (Some(expected), Ok(current)) = (version, FileVersion::read(&file)) {
                if expected != current {
                    return Err(stale());
                }
            } else {
                inspect_revision(&self.root, key, id, &after, |_| {})
                    .map_err(|error| invalid(409, error))?;
            }
        }
        let after_number = document.revision.number.saturating_add(1);
        let mut created = false;
        let result = publish_revision(
            &self.root,
            key,
            tx,
            id,
            doc,
            document.revision,
            content,
            after,
            retention,
            &mut created,
            |con, _| {
                if matches!(retention, annotations::Retention::None) {
                    super::redaction_records::invalidate(con, id)
                } else {
                    Ok(())
                }
            },
        );
        if created {
            self.clean_failed_revision(id, after_number, &result);
        }
        result.map_err(|_| revision_store_error())
    }

    fn clean_failed_revision(&self, id: &str, number: u64, result: &rusqlite::Result<Reply>) {
        if result.is_err() && matches!(record(&self.con, id, number), Ok(None)) {
            if let Some(name) = self
                .key
                .as_ref()
                .and_then(|key| file_name(key, id, number).ok())
            {
                let _ = fs::remove_file(self.root.join("documents-v1").join(id).join(name));
            }
        }
    }

    pub(super) fn publish_measured_text_revision(
        &mut self,
        id: &str,
        expected: u64,
        content: ValidatedRevisionPdf,
        files: DocumentFileProof,
        provenance: String,
    ) -> rusqlite::Result<Reply> {
        self.publish_measured_revision(
            id,
            expected,
            Some(content),
            Some(("modified".into(), provenance)),
            annotations::Retention::Pdf,
            files,
            |_, _| Ok(()),
        )
    }

    pub(super) fn publish_measured_revision(
        &mut self,
        id: &str,
        expected: u64,
        content: Option<ValidatedRevisionPdf>,
        ocr: Option<(String, String)>,
        retention: annotations::Retention,
        files: DocumentFileProof,
        complete: impl FnOnce(&Connection, &Value) -> rusqlite::Result<()>,
    ) -> rusqlite::Result<Reply> {
        let Some(key) = self.key.as_ref() else {
            return Ok(invalid(423, "Unlock the vault first"));
        };
        let tx = self
            .con
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut doc) = active_document(&tx, id)? else {
            return Ok(invalid(404, "Document not found"));
        };
        if doc["revision"] != expected {
            return Ok(invalid(409, "Working revision changed during processing"));
        }
        let before = record(&tx, id, expected)?.ok_or(rusqlite::Error::InvalidQuery)?;
        let verified =
            DocumentFiles::capture(&tx, &self.root, Some(key), &doc, id).and_then(|current| {
                if !files.matches(&current) {
                    return Err("Document or working revision metadata mismatch".into());
                }
                files.check_current()
            });
        if let Err(error) = verified {
            return Ok(invalid(409, error));
        }
        if let Some(bytes) = content {
            let mut after = before.clone();
            if let Some((status, provenance)) = ocr {
                after.ocr = status;
                after.provenance = provenance;
            }
            let mut created = false;
            let result = publish_revision(
                &self.root,
                key,
                tx,
                id,
                doc,
                before,
                bytes,
                after,
                retention,
                &mut created,
                complete,
            );
            if created {
                self.clean_failed_revision(id, expected.saturating_add(1), &result);
            }
            return result;
        }
        if let Some((status, provenance)) = ocr {
            let stamp = now();
            doc["ocr_status"] = json!(&status);
            doc["ocr_provenance_json"] = json!(&provenance);
            doc["updated_at"] = json!(stamp);
            tx.execute(
                "UPDATE documents SET body=? WHERE id=?",
                params![doc.to_string(), id],
            )?;
            tx.execute(
                "UPDATE document_revisions SET ocr_status=?,ocr_provenance_json=? WHERE document_id=? AND revision=?",
                params![status, provenance, id, expected],
            )?;
            let matter = doc["matter_id"]
                .as_str()
                .ok_or(rusqlite::Error::InvalidQuery)?;
            document_metadata::touch(&tx, matter, &stamp)?;
        }
        complete(&tx, &doc)?;
        tx.commit()?;
        Ok(reply(200, doc))
    }
}

fn publish_revision(
    root: &Path,
    key: &DataKey,
    con: rusqlite::Transaction<'_>,
    id: &str,
    mut doc: Value,
    before: Revision,
    content: ValidatedRevisionPdf,
    mut after: Revision,
    retention: annotations::Retention,
    created: &mut bool,
    complete: impl FnOnce(&Connection, &Value) -> rusqlite::Result<()>,
) -> rusqlite::Result<Reply> {
    let current = before.number;
    if content.pages != after.pages {
        return Ok(invalid(409, "Working revision changed during processing"));
    }
    let ValidatedRevisionPdf { bytes, digest, .. } = content;
    let stamp = now();
    after.number = current
        .checked_add(1)
        .ok_or(rusqlite::Error::InvalidQuery)?;
    after.created = stamp.clone();
    let name = file_name(key, id, after.number).map_err(string_sql)?;
    after.relpath = format!("documents-v1/{id}/{name}");
    after.hash = digest;
    let path = root.join(&after.relpath);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(io_sql)?;
    *created = true;
    let result = (|| -> rusqlite::Result<Reply> {
        let mut stream = vault::EncryptedWriter::new(file, key).map_err(vault_sql)?;
        stream.write_all(&bytes).map_err(io_sql)?;
        stream
            .finish()
            .map_err(vault_sql)?
            .sync_all()
            .map_err(io_sql)?;
        sync_directory(path.parent().unwrap()).map_err(string_sql)?;
        crash_at("revision_synced");
        inspect_revision(root, key, id, &after, |_| {}).map_err(string_sql)?;
        after.insert(&con, id)?;
        let annotation_ids = annotations::retained_ids(&con, id, before.number, retention)?;
        let previous = working_state(&doc, &before, &annotation_ids);
        update_working(&con, &mut doc, &after, &stamp)?;
        annotations::retarget(&con, id, after.number, &annotation_ids)?;
        let matter = doc["matter_id"]
            .as_str()
            .ok_or(rusqlite::Error::InvalidQuery)?;
        con.execute(
            "DELETE FROM edit_history WHERE matter_id=? AND applied=0",
            [matter],
        )?;
        con.execute("INSERT INTO edit_history (matter_id,kind,before_json,after_json,created_at) VALUES (?,'document',?,?,?)",
                params![matter,previous.to_string(),working_state(&doc,&after,&annotation_ids).to_string(),stamp])?;
        con.execute("DELETE FROM edit_history WHERE matter_id=? AND id NOT IN (SELECT id FROM edit_history WHERE matter_id=? ORDER BY id DESC LIMIT 100)", params![matter,matter])?;
        con.execute("INSERT INTO audit_events (matter_id,document_id,action,created_at) VALUES (?,?,'document_updated',?)", params![matter,id,stamp])?;
        con.execute(
            "UPDATE matters SET body=json_set(body,'$.updated_at',?),updated_at=? WHERE id=?",
            params![stamp, stamp, matter],
        )?;
        complete(&con, &doc)?;
        crash_at("revision_published");
        con.commit()?;
        crash_at("revision_committed");
        Ok(reply(200, doc))
    })();
    result
}

fn string_sql(error: String) -> rusqlite::Error {
    io_sql(io::Error::other(error))
}
fn vault_sql(error: vault::VaultError) -> rusqlite::Error {
    string_sql(error.to_string())
}

pub(super) fn replay(
    con: &Connection,
    root: &Path,
    key: &DataKey,
    matter: &str,
    target: &Value,
    expected: &Value,
) -> rusqlite::Result<Option<Reply>> {
    let Some(id) = target["id"].as_str().filter(|id| expected["id"] == *id) else {
        return Ok(Some(invalid(
            409,
            "Unsupported working revision history entry",
        )));
    };
    let Some(mut doc) = active_document(con, id)?.filter(|doc| doc["matter_id"] == matter) else {
        return Ok(Some(invalid(
            409,
            "Document was removed; this edit cannot be restored",
        )));
    };
    if let Err(error) = verify_document(con, root, key, id, &doc) {
        return Ok(Some(invalid(409, error)));
    }
    let number = doc["revision"]
        .as_u64()
        .ok_or(rusqlite::Error::InvalidQuery)?;
    let current = record(con, id, number)?.ok_or(rusqlite::Error::InvalidQuery)?;
    if current.fields() != expected["fields"] {
        return Ok(Some(invalid(
            409,
            "Working revision changed; this edit cannot be restored",
        )));
    }
    let retained: Option<Revision> = con.query_row(&format!(
        "SELECT {FIELDS} FROM document_revisions WHERE document_id=? AND relpath=? ORDER BY revision LIMIT 1"),
        params![id,target["fields"]["working_relpath"].as_str()], Revision::row).optional()?;
    let Some(mut retained) = retained.filter(|record| record.fields() == target["fields"]) else {
        return Ok(Some(invalid(
            409,
            "Retained revision record is unavailable",
        )));
    };
    if let Err(error) = inspect_revision(root, key, id, &retained, |_| {}) {
        return Ok(Some(invalid(409, error)));
    }
    let stamp = now();
    super::redaction_records::replay(con, id, retained.number)?;
    retained.number = number.checked_add(1).ok_or(rusqlite::Error::InvalidQuery)?;
    retained.created = stamp.clone();
    retained.insert(con, id)?;
    update_working(con, &mut doc, &retained, &stamp)?;
    if let Some(ids) = target["preserved_annotation_ids"].as_array() {
        annotations::retarget(
            con,
            id,
            retained.number,
            ids.iter().filter_map(Value::as_str),
        )?;
    }
    Ok(None)
}
