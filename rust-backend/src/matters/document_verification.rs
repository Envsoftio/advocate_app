//! Authentication of the two files bound to a saved document revision.
use super::{
    artifacts::{refusal, FileVersion, PdfProof, PdfSource},
    *,
};

pub(super) struct DocumentFiles {
    folder: PathBuf,
    original: DocumentFile,
    working: DocumentFile,
    protection: Option<Protection>,
}

struct DocumentFile {
    path: PathBuf,
    hash: String,
    mismatch: &'static str,
}

struct Protection {
    key: DataKey,
    receipt: PathBuf,
}

pub(super) struct DocumentFileProof {
    files: DocumentFiles,
    versions: [Option<FileVersion>; 2],
}

impl DocumentFiles {
    pub(super) fn capture(
        con: &Connection,
        root: &Path,
        key: Option<&DataKey>,
        doc: &Value,
        id: &str,
    ) -> Result<Self, String> {
        let folder = root.join("documents-v1").join(id);
        let revision = doc["revision"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or("Invalid working revision")?;
        if key.is_none()
            && (doc["page_count"] != 3
                || doc["certificate_signature_status"] != "unknown"
                || doc["certificate_signature_count"] != 0
                || doc["signature_status"] != "inspection_inconclusive")
        {
            return Err("Document identity or page metadata mismatch".into());
        }
        let stored = con.query_row(
            "SELECT source_sha256,original_relpath,working_relpath FROM documents WHERE id=?",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        );
        let (hash, original, working) = if key.is_some() {
            stored.map_err(|error| error.to_string())?
        } else {
            stored
                .optional()
                .map_err(|error| error.to_string())?
                .ok_or("Document record missing")?
        };
        let (working_path, working_hash, protection) = if let Some(key) = key {
            let current = revisions::record(con, id, revision)
                .map_err(|error| error.to_string())?
                .ok_or("Working revision record missing")?;
            if doc["source_sha256"] != hash
                || original != format!("documents-v1/{id}/original.pdf")
                || !current.matches_working_metadata(&working, doc)
            {
                return Err("Document or working revision metadata mismatch".into());
            }
            let path = revisions::revision_path(root, key, id, &current)?;
            let protected = Protection {
                receipt: folder.join(publication_receipt(key, id)?),
                key: key.clone(),
            };
            (path, current.hash, Some(protected))
        } else {
            let (relative, working_hash): (String, String) = con.query_row(
                "SELECT relpath,sha256 FROM document_revisions WHERE document_id=? AND revision=1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional().map_err(|error| error.to_string())?.ok_or("Working revision record missing")?;
            if doc["source_sha256"] != hash
                || hash != PILOT_PDF_SHA256
                || working_hash != hash
                || revision != 1
                || original != format!("documents-v1/{id}/original.pdf")
                || working != format!("documents-v1/{id}/revision-1.pdf")
                || relative != working
            {
                return Err("Document resource metadata mismatch".into());
            }
            (root.join(working), working_hash, None)
        };
        let original = DocumentFile {
            path: root.join(original),
            hash,
            mismatch: if protection.is_some() {
                "Imported original failed hash verification"
            } else {
                "Document hash mismatch: original.pdf"
            },
        };
        let working = DocumentFile {
            path: working_path,
            hash: working_hash,
            mismatch: if protection.is_some() {
                "Retained revision bytes changed; reload revision history"
            } else {
                "Document hash mismatch: revision-1.pdf"
            },
        };
        Ok(Self {
            folder,
            original,
            working,
            protection,
        })
    }

    fn check_folder(&self) -> Result<(), String> {
        if let Some(protected) = &self.protection {
            if self.folder.is_symlink() || !self.folder.is_dir() {
                return Err("Invalid document folder".into());
            }
            check_receipt(&protected.receipt)?;
        } else {
            if self.folder.is_symlink() {
                return Err("Document folder symlink refused".into());
            }
            let mut names = fs::read_dir(&self.folder)
                .map_err(|_| "Document folder missing".to_owned())?
                .map(|entry| {
                    entry
                        .map(|item| item.file_name().to_string_lossy().into_owned())
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            names.sort();
            if names != ["original.pdf", "revision-1.pdf"] {
                return Err("Foreign document folder content refused".into());
            }
        }
        Ok(())
    }

    fn open(&self, source: &DocumentFile, working: bool) -> Result<File, String> {
        if self.protection.is_some()
            && working
            && !fs::symlink_metadata(&source.path).is_ok_and(|metadata| metadata.is_file())
        {
            return Err("Retained revision file is unavailable".into());
        }
        if self.protection.is_none() && source.path.is_symlink() {
            return Err("Document symlink refused".into());
        }
        open_document_file(&source.path)
    }

    fn verify_file(
        &self,
        source: &DocumentFile,
        file: &File,
        cancelled: &impl Fn() -> bool,
        mut collect: impl FnMut(&[u8]),
    ) -> Result<u64, String> {
        if cancelled() {
            return Err("Operation cancelled".into());
        }
        if let Some(protected) = &self.protection {
            let mut hash = Sha256::new();
            let mut size = 0u64;
            let read = vault::try_visit_encrypted(file, &protected.key, |chunk| {
                if cancelled() {
                    return Err(std::io::Error::from(std::io::ErrorKind::Interrupted).into());
                }
                hash.update(chunk);
                size += chunk.len() as u64;
                collect(chunk);
                Ok(())
            });
            if cancelled() {
                return Err("Operation cancelled".into());
            }
            read.map_err(|error| error.to_string())?;
            if vault::hex_bytes(&hash.finalize()).as_str() != source.hash {
                return Err(source.mismatch.into());
            }
            Ok(size)
        } else {
            let bytes = read_open_pilot_file(file)?;
            if sha256_hex(&bytes) != source.hash {
                return Err(source.mismatch.into());
            }
            collect(&bytes);
            Ok(bytes.len() as u64)
        }
    }

    fn verify_with_version(
        &self,
        source: &DocumentFile,
        working: bool,
        cancelled: &impl Fn() -> bool,
        collect: impl FnMut(&[u8]),
    ) -> Result<(u64, Option<FileVersion>), String> {
        let file = self.open(source, working)?;
        let before = FileVersion::read(&file).ok();
        let size = self.verify_file(source, &file, cancelled, collect)?;
        if before.is_some() && before != FileVersion::read(&file).ok() {
            return Err(source.mismatch.into());
        }
        Ok((size, before))
    }

    pub(super) fn authenticate(self) -> Result<(u64, DocumentFileProof), String> {
        self.authenticate_with(|_| {})
    }

    pub(super) fn authenticate_with(
        self,
        collect: impl FnMut(&[u8]),
    ) -> Result<(u64, DocumentFileProof), String> {
        self.check_folder()?;
        let (_, original) = self.verify_with_version(&self.original, false, &|| false, |_| {})?;
        let (size, working) = self.verify_with_version(&self.working, true, &|| false, collect)?;
        Ok((
            size,
            DocumentFileProof {
                files: self,
                versions: [original, working],
            },
        ))
    }

    pub(super) fn authenticate_original(
        self,
        working: PdfProof,
        cancelled: impl Fn() -> bool,
    ) -> Result<DocumentFileProof, String> {
        self.check_folder()?;
        let working = working.into_version(&self.working.path, &self.working.hash)?;
        let (_, original) = self.verify_with_version(&self.original, false, &cancelled, |_| {})?;
        Ok(DocumentFileProof {
            files: self,
            versions: [original, working],
        })
    }
}

/// Captures database-qualified inputs before a job reads them on a worker.
pub(super) struct RevisionInput {
    source: PdfSource,
    files: DocumentFiles,
    pub pages: u64,
}

impl RevisionInput {
    pub(super) fn capture(
        store: &Store,
        doc: &Value,
        id: &str,
        revision: u64,
    ) -> Result<Self, Reply> {
        let source = store.pdf_source(
            id,
            &Artifact::Working {
                revision: Some(revision),
            },
        )?;
        let files = DocumentFiles::capture(&store.con, &store.root, store.key.as_ref(), doc, id)
            .map_err(|error| refusal(409, error))?;
        let pages = doc["page_count"]
            .as_u64()
            .ok_or_else(|| refusal(409, "Working PDF revision metadata mismatch"))?;
        Ok(Self {
            source,
            files,
            pages,
        })
    }

    pub(super) fn read(
        self,
        cancelled: impl Fn() -> bool,
    ) -> Result<(Vec<u8>, DocumentFileProof), Reply> {
        let (bytes, working) = self.source.read_with_proof(&cancelled)?;
        let files = self
            .files
            .authenticate_original(working, cancelled)
            .map_err(|error| refusal(409, error))?;
        Ok((bytes, files))
    }
}

impl DocumentFileProof {
    pub(super) fn matches(&self, current: &DocumentFiles) -> bool {
        let same = |before: &DocumentFile, after: &DocumentFile| {
            before.path == after.path && before.hash == after.hash
        };
        self.files.folder == current.folder
            && same(&self.files.original, &current.original)
            && same(&self.files.working, &current.working)
    }

    pub(super) fn check_current(&self) -> Result<(), String> {
        self.files.check_folder()?;
        let sources = [&self.files.original, &self.files.working];
        for (index, (source, expected)) in sources.into_iter().zip(&self.versions).enumerate() {
            let file = self.files.open(source, index == 1)?;
            if let (Some(expected), Ok(current)) = (expected, FileVersion::read(&file)) {
                if expected != &current {
                    return Err(source.mismatch.into());
                }
            } else {
                self.files.verify_file(source, &file, &|| false, |_| {})?;
            }
        }
        Ok(())
    }
}
