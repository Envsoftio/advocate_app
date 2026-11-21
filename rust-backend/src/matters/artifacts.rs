//! Verified document byte snapshots. No transport or destination path is accepted here.
use super::*;
use serde::Deserialize;
use std::io::{self, Cursor, Seek, SeekFrom};

mod file_version;
pub(super) use file_version::FileVersion;

pub const READ_CHUNK_BYTES: usize = 256 * 1024;
const MAX_BUFFERED_ARTIFACT_BYTES: u64 = 160 * 1024 * 1024;
const MAX_STREAMED_ARTIFACT_BYTES: u64 = super::portable_matter::MAX_FILE_BYTES;
const PROCESSING_LIMIT: &str = "PDF exceeds the in-memory processing limit";
const STREAM_LIMIT: &str = "PDF exceeds the Rust artifact limit";

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum Artifact {
    Original,
    Working { revision: Option<u64> },
    Retained { revision: u64, sha256: String },
}

trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

pub struct PdfSnapshot {
    reader: Box<dyn ReadSeek>,
    pub bytes: u64,
}

/// A revision-qualified file location resolved under the store's database lane.
/// Opening and authenticating the payload requires no database access.
pub(super) struct PdfSource {
    path: PathBuf,
    hash: String,
    folder: PathBuf,
    receipt: Option<PathBuf>,
    key: Option<DataKey>,
    missing: Reply,
    mismatch: &'static str,
}

pub(super) struct PdfProof {
    source: PdfSource,
    version: Option<FileVersion>,
}

impl PdfProof {
    pub(super) fn into_version(
        self,
        path: &Path,
        hash: &str,
    ) -> Result<Option<FileVersion>, &'static str> {
        if self.source.path != path || self.source.hash != hash {
            return Err(self.source.mismatch);
        }
        Ok(self.version)
    }

    pub(super) fn verify_bytes(&self, plaintext: &[u8]) -> Result<(), Reply> {
        (sha256_hex(plaintext) == self.source.hash)
            .then_some(())
            .ok_or_else(|| refusal(409, self.source.mismatch))
    }

    pub(super) fn matches(&self, current: &PdfSource) -> bool {
        self.source.path == current.path && self.source.hash == current.hash
    }

    pub(super) fn check_current(&self) -> Result<(), Reply> {
        let file = self.source.processing_file()?;
        if let (Some(verified), Ok(current)) = (&self.version, FileVersion::read(&file)) {
            return if *verified == current {
                Ok(())
            } else {
                Err(refusal(409, self.source.mismatch))
            };
        }
        // Filesystems without reliable identity/change information must still
        // satisfy the authenticated hash check at the commit boundary.
        self.source.verify_chunks(&file, &|| false, |_| Ok(()))
    }
}

impl PdfSource {
    fn check_folder(folder: &Path, receipt: Option<&Path>) -> Result<(), Reply> {
        if folder.is_symlink() || folder.parent().is_some_and(Path::is_symlink) {
            return Err(refusal(409, "Invalid document folder"));
        }
        if let Some(receipt) = receipt {
            check_receipt(receipt).map_err(|error| refusal(409, error))?;
        }
        Ok(())
    }

    fn open_artifact(&self) -> Result<File, Reply> {
        let file = open_document_file(&self.path).map_err(|error| refusal(409, error))?;
        let size = file
            .metadata()
            .map_err(|error| refusal(409, error.to_string()))?
            .len();
        if size > MAX_STREAMED_ARTIFACT_BYTES + 1024 * 1024 {
            return Err(refusal(413, "PDF exceeds the Rust artifact limit"));
        }
        Ok(file)
    }

    fn pilot_bytes(&self, file: &File) -> Result<Vec<u8>, Reply> {
        let bytes = read_open_pilot_file(file).map_err(|_| refusal(409, self.mismatch))?;
        if self.hash != PILOT_PDF_SHA256 || bytes != PILOT_PDF_BYTES {
            return Err(refusal(409, self.mismatch));
        }
        Ok(bytes)
    }

    pub(super) fn open(self) -> Result<PdfSnapshot, Reply> {
        Self::check_folder(&self.folder, self.receipt.as_deref())?;
        self.snapshot(self.snapshot_file()?)
    }

    pub(super) fn can_open_file(&self) -> bool {
        Self::check_folder(&self.folder, self.receipt.as_deref()).is_ok()
            && self.snapshot_file().is_ok()
    }

    fn snapshot_file(&self) -> Result<File, Reply> {
        if !self.path.is_file() {
            return Err(reply(self.missing.status, self.missing.body.clone()));
        }
        if self.key.is_none() {
            open_document_file(&self.path).map_err(|_| refusal(409, self.mismatch))
        } else {
            self.open_artifact()
        }
    }

    fn snapshot(&self, file: File) -> Result<PdfSnapshot, Reply> {
        let Some(key) = self.key.as_ref() else {
            return self.pilot_bytes(&file).map(PdfSnapshot::from_bytes);
        };
        let reader =
            vault::EncryptedReader::at(file, 0, key).map_err(|_| refusal(409, self.mismatch))?;
        if reader.is_empty() || reader.len() > MAX_STREAMED_ARTIFACT_BYTES {
            return Err(refusal(413, "PDF exceeds the Rust artifact limit"));
        }
        if vault::hex_bytes(reader.digest()).as_str() != self.hash {
            return Err(refusal(409, self.mismatch));
        }
        Ok(PdfSnapshot {
            bytes: reader.len(),
            reader: Box::new(reader),
        })
    }

    pub(super) fn open_with_proof(self) -> Result<(PdfSnapshot, PdfProof), Reply> {
        Self::check_folder(&self.folder, self.receipt.as_deref())?;
        let file = self.snapshot_file()?;
        let version = FileVersion::read(&file).ok();
        let snapshot = self.snapshot(file.try_clone().map_err(|_| refusal(409, self.mismatch))?)?;
        if version.is_some() && version != FileVersion::read(&file).ok() {
            return Err(refusal(409, self.mismatch));
        }
        Ok((
            snapshot,
            PdfProof {
                source: self,
                version,
            },
        ))
    }

    pub(super) fn authenticate(self, cancelled: impl Fn() -> bool) -> Result<PdfProof, Reply> {
        self.verified(cancelled, |_| Ok(()))
    }

    pub(super) fn read_with_proof(
        self,
        cancelled: impl Fn() -> bool,
    ) -> Result<(Vec<u8>, PdfProof), Reply> {
        let mut bytes = Vec::new();
        let proof = self.verified(cancelled, |contents| {
            let size = bytes.len() + contents.len();
            if size as u64 > MAX_BUFFERED_ARTIFACT_BYTES {
                return Err(vault::VaultError::Format(PROCESSING_LIMIT));
            }
            if size > bytes.capacity() {
                let capacity = (bytes.capacity() * 2)
                    .max(size)
                    .min(MAX_BUFFERED_ARTIFACT_BYTES as usize);
                bytes
                    .try_reserve_exact(capacity - bytes.len())
                    .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
            }
            bytes.extend_from_slice(contents);
            Ok(())
        })?;
        Ok((bytes, proof))
    }

    fn processing_file(&self) -> Result<File, Reply> {
        Self::check_folder(&self.folder, self.receipt.as_deref())?;
        if !self.path.is_file() {
            return Err(reply(self.missing.status, self.missing.body.clone()));
        }
        self.open_artifact()
    }

    fn verified(
        self,
        cancelled: impl Fn() -> bool,
        collect: impl FnMut(&[u8]) -> Result<(), vault::VaultError>,
    ) -> Result<PdfProof, Reply> {
        if cancelled() {
            return Err(refusal(409, "Operation cancelled"));
        }
        let file = self.processing_file()?;
        let before = FileVersion::read(&file).ok();
        self.verify_chunks(&file, &cancelled, collect)?;
        let after = FileVersion::read(&file).ok();
        if before.is_some() && before != after {
            return Err(refusal(409, self.mismatch));
        }
        Ok(PdfProof {
            source: self,
            version: before,
        })
    }

    fn verify_chunks(
        &self,
        file: &File,
        cancelled: &impl Fn() -> bool,
        mut collect: impl FnMut(&[u8]) -> Result<(), vault::VaultError>,
    ) -> Result<(), Reply> {
        let Some(key) = self.key.as_ref() else {
            collect(&self.pilot_bytes(file)?).map_err(|_| refusal(409, self.mismatch))?;
            return Ok(());
        };
        let mut digest = Sha256::new();
        let mut length = 0;
        let read = vault::try_visit_encrypted(file, key, |contents| {
            if cancelled() {
                return Err(io::Error::from(io::ErrorKind::Interrupted).into());
            }
            length += contents.len() as u64;
            if length > MAX_STREAMED_ARTIFACT_BYTES {
                return Err(vault::VaultError::Format(STREAM_LIMIT));
            }
            digest.update(contents);
            collect(contents)
        });
        if cancelled() {
            return Err(refusal(409, "Operation cancelled"));
        }
        read.map_err(|error| match error {
            vault::VaultError::Format(STREAM_LIMIT) => refusal(413, STREAM_LIMIT),
            vault::VaultError::Format(PROCESSING_LIMIT) => refusal(422, PROCESSING_LIMIT),
            vault::VaultError::Io(error) if error.kind() == io::ErrorKind::OutOfMemory => {
                refusal(422, "Unable to allocate PDF processing memory")
            }
            _ => refusal(409, self.mismatch),
        })?;
        if length == 0 || vault::hex_bytes(&digest.finalize()).as_str() != self.hash {
            return Err(refusal(409, self.mismatch));
        }
        Ok(())
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn worker_read_checks_the_frozen_hash_and_can_stop_between_chunks() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("documents-v1").join("synthetic");
        std::fs::create_dir_all(&folder).unwrap();
        let receipt = folder.join("receipt");
        std::fs::write(&receipt, []).unwrap();
        let path = folder.join("revision.pdf");
        let bytes = vec![b'A'; 2 * vault::CHUNK_BYTES];
        let key = DataKey::from_bytes([23; 32]);
        let plan = || PdfSource {
            path: path.clone(),
            hash: sha256_hex(&bytes),
            folder: folder.clone(),
            receipt: Some(receipt.clone()),
            key: Some(key.clone()),
            missing: refusal(404, "PDF working copy missing"),
            mismatch: "Working PDF failed its integrity check",
        };
        vault::publish_encrypted(&path, &key, |file| {
            file.write_all(&bytes)?;
            Ok(())
        })
        .unwrap();
        let checks = Cell::new(0);
        let cancelled = plan()
            .read_with_proof(|| {
                checks.set(checks.get() + 1);
                checks.get() >= 3
            })
            .err()
            .unwrap();
        assert_eq!(cancelled.body["detail"], "Operation cancelled");
        assert_eq!(plan().read_with_proof(|| false).unwrap().0, bytes);

        let verified = plan().authenticate(|| false).ok().unwrap();
        let ciphertext = std::fs::read(&path).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let mut altered = ciphertext.clone();
        altered[64] ^= 1;
        std::fs::write(&path, altered).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert!(verified.check_current().is_err());
        std::fs::write(&path, ciphertext).unwrap();

        let frozen = plan();
        vault::publish_encrypted(&path, &key, |file| {
            file.write_all(b"Different content with valid encryption")?;
            Ok(())
        })
        .unwrap();
        let replaced = frozen.read_with_proof(|| false).err().unwrap();
        assert_eq!(replaced.status, 409);
        assert_eq!(
            replaced.body["detail"],
            "Working PDF failed its integrity check"
        );
    }
}

impl PdfSnapshot {
    pub(super) fn from_bytes(bytes: Vec<u8>) -> Self {
        Self {
            bytes: bytes.len() as u64,
            reader: Box::new(Cursor::new(bytes)),
        }
    }

    pub fn read_range(&mut self, begin: u64, end: u64) -> io::Result<Vec<u8>> {
        let length = end
            .checked_sub(begin)
            .filter(|size| *size > 0 && *size <= READ_CHUNK_BYTES as u64 && end <= self.bytes)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid PDF byte range"))?;
        self.reader.seek(SeekFrom::Start(begin))?;
        let mut data = vec![0; length as usize];
        self.reader.read_exact(&mut data)?;
        Ok(data)
    }

    pub fn copy_to(&mut self, output: &mut impl Write) -> io::Result<()> {
        self.reader.seek(SeekFrom::Start(0))?;
        let copied = io::copy(&mut self.reader.by_ref().take(self.bytes), output)?;
        if copied != self.bytes {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "PDF transfer ended early",
            ));
        }
        Ok(())
    }

    pub(super) fn into_bytes(mut self) -> io::Result<Vec<u8>> {
        if self.bytes > MAX_BUFFERED_ARTIFACT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PDF exceeds the in-memory processing limit",
            ));
        }
        let mut bytes = Vec::with_capacity(self.bytes as usize);
        self.copy_to(&mut bytes)?;
        Ok(bytes)
    }
}

pub(super) fn refusal(status: u16, detail: impl Into<String>) -> Reply {
    reply(status, json!({"detail":detail.into()}))
}

pub(super) fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(super) fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(super) struct Document {
    pub(super) body: Value,
    original: String,
    working: String,
    hash: String,
}

impl Store {
    pub(super) fn artifact_document(&self, id: &str) -> Result<Document, Reply> {
        Self::artifact_at(&self.con, id)
    }

    pub(super) fn artifact_at(con: &Connection, id: &str) -> Result<Document, Reply> {
        if !valid_id(id) {
            return Err(refusal(404, "Document not found"));
        }
        let row = con.query_row(
            "SELECT d.body,d.matter_id,d.original_relpath,d.working_relpath,d.source_sha256
             FROM documents d JOIN matters m ON m.id=d.matter_id
             WHERE d.id=? AND d.archived_at IS NULL AND json_extract(m.body,'$.archived_at') IS NULL",
            [id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?,
                row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?)),
        ).optional().map_err(|_| refusal(500, "Cannot load document metadata"))?;
        let (raw, matter, original, working, hash) =
            row.ok_or_else(|| refusal(404, "Document not found"))?;
        let body = decode(&raw).map_err(|_| refusal(409, "Document metadata is unavailable"))?;
        if body["id"] != id
            || body["matter_id"] != matter
            || !body["archived_at"].is_null()
            || body["source_sha256"] != hash
            || !valid_hash(&hash)
            || original != format!("documents-v1/{id}/original.pdf")
        {
            return Err(refusal(409, "Document artifact metadata mismatch"));
        }
        Ok(Document {
            body,
            original,
            working,
            hash,
        })
    }

    pub fn open_pdf(&self, id: &str, artifact: &Artifact) -> Result<PdfSnapshot, Reply> {
        let doc = self.artifact_document(id)?;
        self.open_document_pdf(id, &doc, artifact)
    }

    pub(super) fn pdf_source_at(
        con: &Connection,
        root: &Path,
        key: Option<&DataKey>,
        id: &str,
        artifact: &Artifact,
    ) -> Result<PdfSource, Reply> {
        let doc = Self::artifact_at(con, id)?;
        Self::resolve_pdf(con, root, key, id, &doc, artifact)
    }

    pub(super) fn open_document_pdf(
        &self,
        id: &str,
        doc: &Document,
        artifact: &Artifact,
    ) -> Result<PdfSnapshot, Reply> {
        self.document_pdf_source(id, doc, artifact)?.open()
    }

    pub(super) fn document_pdf_source(
        &self,
        id: &str,
        doc: &Document,
        artifact: &Artifact,
    ) -> Result<PdfSource, Reply> {
        Self::resolve_pdf(&self.con, &self.root, self.key.as_ref(), id, doc, artifact)
    }

    pub(super) fn pdf_source(&self, id: &str, artifact: &Artifact) -> Result<PdfSource, Reply> {
        let doc = self.artifact_document(id)?;
        self.document_pdf_source(id, &doc, artifact)
    }

    pub(super) fn screening_sources(
        &self,
        id: &str,
        revision: u64,
    ) -> Result<(PdfSource, PdfSource), Reply> {
        let doc = self.artifact_document(id)?;
        if self.key.is_some() {
            let retained = revisions::record(&self.con, id, revision)
                .map_err(|_| refusal(500, "Cannot load PDF revision metadata"))?
                .ok_or_else(|| refusal(409, "Retained revision not found"))?;
            if !retained.matches_working_metadata(&doc.working, &doc.body) {
                return Err(refusal(
                    409,
                    "Document or working revision metadata mismatch",
                ));
            }
        }
        let resolve = |artifact: &Artifact| {
            Self::resolve_pdf(&self.con, &self.root, self.key.as_ref(), id, &doc, artifact)
        };
        let original = resolve(&Artifact::Original)?;
        let working = resolve(&Artifact::Working {
            revision: Some(revision),
        })?;
        Ok((original, working))
    }

    fn resolve_pdf(
        con: &Connection,
        root: &Path,
        key: Option<&DataKey>,
        id: &str,
        doc: &Document,
        artifact: &Artifact,
    ) -> Result<PdfSource, Reply> {
        let folder = root.join("documents-v1").join(id);
        let receipt = key
            .map(|key| publication_receipt(key, id).map(|receipt| folder.join(receipt)))
            .transpose()
            .map_err(|error| refusal(409, error))?;
        PdfSource::check_folder(&folder, receipt.as_deref())?;
        let current = doc.body["revision"]
            .as_u64()
            .filter(|r| *r > 0)
            .ok_or_else(|| refusal(409, "Invalid working revision"))?;
        let (path, hash, missing, mismatch) = match artifact {
            Artifact::Original => (
                root.join(&doc.original),
                doc.hash.clone(),
                refusal(404, "Imported original is unavailable"),
                "Imported original failed its integrity check",
            ),
            Artifact::Working { revision } => {
                if revision.is_some_and(|expected| expected != current) {
                    return Err(refusal(
                        409,
                        "Working PDF revision changed; reload the document",
                    ));
                }
                let (path, hash) = Self::artifact_revision(con, root, key, id, current, Some(doc))?;
                (
                    path,
                    hash,
                    refusal(404, "PDF working copy missing"),
                    "Working PDF failed its integrity check",
                )
            }
            Artifact::Retained { revision, sha256 } => {
                if !valid_hash(sha256) {
                    return Err(refusal(422, "A lowercase SHA-256 is required"));
                }
                let (path, hash) = Self::artifact_revision(con, root, key, id, *revision, None)?;
                if hash != *sha256 {
                    return Err(refusal(
                        409,
                        "Retained revision bytes changed; reload revision history",
                    ));
                }
                (
                    path,
                    hash,
                    refusal(409, "Retained revision file is unavailable"),
                    "Retained revision bytes changed; reload revision history",
                )
            }
        };
        Ok(PdfSource {
            path,
            hash,
            folder,
            receipt,
            key: key.cloned(),
            missing,
            mismatch,
        })
    }

    fn artifact_revision(
        con: &Connection,
        root: &Path,
        key: Option<&DataKey>,
        id: &str,
        number: u64,
        current: Option<&Document>,
    ) -> Result<(PathBuf, String), Reply> {
        let unable = || refusal(500, "Cannot load PDF revision metadata");
        let missing = || refusal(409, "Retained revision not found");
        if number > i64::MAX as u64 {
            return Err(missing());
        }
        if let Some(key) = key {
            let revision = revisions::record(con, id, number)
                .map_err(|_| unable())?
                .ok_or_else(missing)?;
            if let Some(doc) = current {
                if revision.relpath != doc.working || doc.body["page_count"] != revision.pages {
                    return Err(refusal(409, "Working PDF revision metadata mismatch"));
                }
                if !root.join(&doc.working).is_file() {
                    return Err(refusal(404, "PDF working copy missing"));
                }
            }
            let path = revisions::revision_file(root, key, id, &revision)
                .map_err(|error| refusal(409, error))?;
            return Ok((path, revision.hash));
        }
        let (relpath, hash): (String, String) = con
            .query_row(
                "SELECT relpath,sha256 FROM document_revisions WHERE document_id=? AND revision=?",
                params![id, number],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|_| unable())?
            .ok_or_else(missing)?;
        if number != 1
            || relpath != format!("documents-v1/{id}/revision-1.pdf")
            || current.is_some_and(|doc| doc.working != relpath)
        {
            return Err(refusal(409, "Working PDF revision metadata mismatch"));
        }
        Ok((root.join(relpath), hash))
    }

    pub fn prepare_original_info(&self, id: &str) -> Result<OriginalInfoWork, Reply> {
        let doc = self.artifact_document(id)?;
        let source = self.document_pdf_source(id, &doc, &Artifact::Original)?;
        let filename = doc.body["original_name"].clone();
        let media = original_media(&filename);
        Ok(OriginalInfoWork {
            document: id.to_owned(),
            source,
            filename,
            media,
            hash: doc.hash,
            imported: doc.body["created_at"].clone(),
        })
    }
}

pub(super) fn original_media(filename: &Value) -> &'static str {
    let name = filename.as_str().unwrap_or_default().to_ascii_lowercase();
    if name.ends_with(".png") {
        "image/png"
    } else if name.ends_with(".jpeg") || name.ends_with(".jpg") {
        "image/jpeg"
    } else {
        "application/pdf"
    }
}

pub fn original_info_document<'a>(method: &str, path: &'a str) -> Option<&'a str> {
    if method != "GET" {
        return None;
    }
    path.trim_start_matches('/')
        .strip_prefix("documents/")?
        .strip_suffix("/original-info")
        .filter(|id| !id.contains('/'))
}

pub struct OriginalInfoWork {
    document: String,
    source: PdfSource,
    filename: Value,
    media: &'static str,
    hash: String,
    imported: Value,
}

impl OriginalInfoWork {
    pub fn compute(self) -> Result<Reply, Reply> {
        let snapshot = self.source.open()?;
        let (pages, password_protected) = if self.media == "application/pdf" {
            let parse_error = || refusal(409, "Cannot inspect imported original PDF");
            let pdf = pdf_oxide::PdfDocument::from_bytes(
                snapshot.into_bytes().map_err(|_| parse_error())?,
            )
            .map_err(|_| parse_error())?;
            let protected = pdf.is_encrypted() && !pdf.authenticate(b"").unwrap_or(false);
            let pages = if protected {
                None
            } else {
                Some(pdf.page_count().map_err(|_| parse_error())?)
            };
            (pages, protected)
        } else {
            (None, false)
        };
        Ok(reply(
            200,
            json!({
                "document_id":self.document, "artifact":"original", "read_only":true,
                "sha256":self.hash, "filename":self.filename, "media_type":self.media,
                "page_count":pages, "password_protected":password_protected, "imported_at":self.imported
            }),
        ))
    }
}
