//! Exclusive ownership of an isolated Rust vault and its database connection.
use crate::{
    matters::{Artifact, OcrReviewMeasurement, OcrReviewWork, PdfSnapshot, Store},
    reply, secure_db, vault, Reply,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use tempfile::TempPath;
use zeroize::Zeroizing;

pub use crate::matters::ImportOptions;

const MARKER: &[u8] = b"MatterDraft Rust protected store v1\n";
const MARKER_NAME: &str = "RUST_PROTECTED_STORE";
const DATABASE: &str = "rust-matters.sqlite3";
const DATABASE_STAGE: &str = ".vault-db-stage";
const METADATA: &str = "vault.json";
const METADATA_STAGE: &str = ".vault-metadata-stage";
const METADATA_LIMIT: u64 = 16 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Migrating,
    Active,
}

#[derive(Deserialize, Serialize)]
struct Metadata {
    format: String,
    version: u32,
    state: State,
    envelope: Value,
}

pub struct VaultStore {
    root: PathBuf,
    store: Option<Store>,
    // Drop the database before releasing the exclusive file lock.
    _lock: File,
}

fn sync_root(root: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(root)?.sync_all()?;
    let _ = root;
    Ok(())
}

fn regular_file(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Unexpected vault file type",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

impl VaultStore {
    /// Convert a detached, closed Python vault snapshot into a new Rust vault.
    pub fn import_python_snapshot(
        snapshot: &Path,
        destination: &Path,
        credential: vault::Credential<'_>,
    ) -> Result<(), String> {
        crate::matters::legacy_snapshot::convert(snapshot, destination, credential)
    }

    pub(crate) fn initialize_snapshot(
        root: &Path,
        envelope: Value,
        key: vault::DataKey,
    ) -> Result<Self, String> {
        let mut vault = Self::open(root)?;
        let metadata = Metadata {
            format: "courtbundle-vault".into(),
            version: 1,
            state: State::Migrating,
            envelope,
        };
        vault.save_metadata(&metadata)?;
        vault.activate(metadata, key)?;
        Ok(vault)
    }

    pub(crate) fn snapshot_store(&mut self) -> Result<&mut Store, String> {
        self.store
            .as_mut()
            .ok_or_else(|| "Unlock the vault first".into())
    }

    pub fn open(root: &Path) -> Result<Self, String> {
        Self::ensure_isolated_root(root).map_err(|error| error.to_string())?;
        let lock_path = root.join(".rust-vault.lock");
        regular_file(&lock_path).map_err(|error| error.to_string())?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let lock = options.open(lock_path).map_err(|error| error.to_string())?;
        lock.try_lock()
            .map_err(|_| "Matter data is open in another MatterDraft engine".to_owned())?;
        let staged = root.join(METADATA_STAGE);
        if regular_file(&staged).map_err(|error| error.to_string())? {
            fs::remove_file(staged).map_err(|error| error.to_string())?;
            sync_root(root).map_err(|error| error.to_string())?;
        }
        Ok(Self {
            root: root.to_path_buf(),
            store: None,
            _lock: lock,
        })
    }

    fn ensure_isolated_root(root: &Path) -> io::Result<()> {
        let marker_path = root.join(MARKER_NAME);
        match fs::symlink_metadata(root) {
            Ok(metadata) if metadata.is_dir() => {
                let mut marker = Vec::new();
                if !regular_file(&marker_path)?
                    || File::open(&marker_path)?
                        .take(MARKER.len() as u64 + 1)
                        .read_to_end(&mut marker)?
                        != MARKER.len()
                    || marker != MARKER
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Unrecognized Rust protected store",
                    ));
                }
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Unsafe protected store location",
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder.create(root)?;
                let mut marker = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(marker_path)?;
                marker.write_all(MARKER)?;
                marker.sync_all()?;
                sync_root(root)?;
                if let Some(parent) = root
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                {
                    sync_root(parent)?;
                }
            }
            Err(error) => return Err(error),
        }
        Ok(())
    }

    fn load_metadata(&self) -> Result<Option<Metadata>, String> {
        const DAMAGED: &str = "Vault metadata is damaged; do not create a new vault over this data";
        let path = self.root.join(METADATA);
        if !regular_file(&path).map_err(|_| DAMAGED.to_owned())? {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        File::open(&path)
            .and_then(|file| file.take(METADATA_LIMIT + 1).read_to_end(&mut bytes))
            .map_err(|_| DAMAGED.to_owned())?;
        if bytes.len() as u64 > METADATA_LIMIT {
            return Err(DAMAGED.into());
        }
        let metadata: Metadata = serde_json::from_slice(&bytes).map_err(|_| DAMAGED.to_owned())?;
        if metadata.format != "courtbundle-vault" || metadata.version != 1 {
            return Err(DAMAGED.into());
        }
        Ok(Some(metadata))
    }

    fn save_metadata(&self, metadata: &Metadata) -> Result<(), String> {
        let path = self.root.join(METADATA);
        regular_file(&path).map_err(|error| error.to_string())?;
        let staged = self.root.join(METADATA_STAGE);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&staged).map_err(|error| error.to_string())?;
        let staged = TempPath::try_from_path(staged).map_err(|error| error.to_string())?;
        serde_json::to_writer(&mut file, metadata).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        staged
            .persist(&path)
            .map_err(|error| error.error.to_string())?;
        sync_root(&self.root).map_err(|error| error.to_string())
    }

    fn activate(&mut self, mut metadata: Metadata, key: vault::DataKey) -> Result<(), String> {
        let database = self.root.join(DATABASE);
        for database in [DATABASE, DATABASE_STAGE] {
            for suffix in ["-wal", "-shm", "-journal"] {
                regular_file(&self.root.join(format!("{database}{suffix}")))
                    .map_err(|error| error.to_string())?;
            }
        }
        if !regular_file(&database).map_err(|error| error.to_string())? {
            if metadata.state != State::Migrating {
                return Err("Protected database is missing".into());
            }
            let stage = self.root.join(DATABASE_STAGE);
            if regular_file(&stage).map_err(|error| error.to_string())? {
                // Recover only a complete database already written with this vault key.
                let staged =
                    secure_db::open_protected(&stage, &key).map_err(|error| error.to_string())?;
                secure_db::verify_integrity(&staged).map_err(|error| error.to_string())?;
            } else {
                secure_db::create_protected(&stage, &key).map_err(|error| error.to_string())?;
            }
            fs::rename(stage, &database).map_err(|error| error.to_string())?;
            sync_root(&self.root).map_err(|error| error.to_string())?;
        }
        let connection =
            secure_db::open_protected(&database, &key).map_err(|error| error.to_string())?;
        secure_db::verify_integrity(&connection).map_err(|error| error.to_string())?;
        // The vault lock excludes another live restore preparing into this root.
        crate::matters::recover_prepared_restores(&self.root, &key)?;
        let store = Store::open_keyed(&self.root, connection, key)?;
        if metadata.state == State::Migrating {
            metadata.state = State::Active;
            self.save_metadata(&metadata)?;
        }
        self.store = Some(store);
        Ok(())
    }

    fn setup(&mut self, password: &str) -> Result<Zeroizing<String>, String> {
        if self.load_metadata()?.is_some() {
            return Err("Vault already exists; unlock it instead".into());
        }
        for entry in fs::read_dir(&self.root).map_err(|error| error.to_string())? {
            let name = entry.map_err(|error| error.to_string())?.file_name();
            if name != MARKER_NAME && name != ".rust-vault.lock" {
                return Err("Unexpected content in the protected vault setup directory".into());
            }
        }
        let vault = vault::create_key_envelope(password).map_err(|error| error.to_string())?;
        let metadata = Metadata {
            format: "courtbundle-vault".into(),
            version: 1,
            state: State::Migrating,
            envelope: vault.envelope,
        };
        self.save_metadata(&metadata)?;
        self.activate(metadata, vault.data_key)?;
        Ok(vault.recovery_key)
    }

    fn unlock(&mut self, credential: vault::Credential<'_>) -> Result<(), String> {
        let metadata = self.load_metadata()?.ok_or("Set up a vault first")?;
        let key =
            vault::unlock_key(&metadata.envelope, credential).map_err(|error| error.to_string())?;
        if self.store.is_some() {
            return Err("Vault is already unlocked".into());
        }
        self.activate(metadata, key)
    }

    fn rotate(&mut self, password: &str) -> Result<Zeroizing<String>, String> {
        let key = self
            .store
            .as_ref()
            .and_then(Store::storage_key)
            .ok_or("Unlock the vault first")?;
        let mut metadata = self
            .load_metadata()?
            .filter(|metadata| metadata.state == State::Active)
            .ok_or("Vault is not ready")?;
        let (envelope, recovery) =
            vault::rotate_key_envelope(key, password).map_err(|error| error.to_string())?;
        metadata.envelope = envelope;
        self.save_metadata(&metadata)?;
        Ok(recovery)
    }

    /// Admit PDF bytes into the unlocked store's immutable original and first revision.
    pub fn import_pdf(
        &mut self,
        matter_id: &str,
        filename: &str,
        bytes: &[u8],
        options: ImportOptions<'_>,
    ) -> Reply {
        match self.store.as_mut() {
            Some(store) => store.import_protected_pdf(matter_id, filename, bytes, options),
            None => reply(423, json!({"detail":"Unlock the vault first"})),
        }
    }

    pub fn capture_document_import(
        &self,
        matter: &str,
    ) -> Result<crate::matters::DocumentImportTarget, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .capture_document_import(matter)
    }

    pub fn complete_document_import(
        &mut self,
        prepared: crate::matters::PreparedDocumentImport<'_>,
    ) -> Reply {
        match self.store.as_mut() {
            Some(store) => store.complete_document_import(prepared),
            None => reply(423, json!({"detail":"Unlock the vault first"})),
        }
    }

    pub fn capture_cover_import(
        &self,
        matter: &str,
    ) -> Result<crate::matters::CoverImportWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .capture_cover_import(matter)
    }

    pub fn complete_cover_import(
        &mut self,
        prepared: crate::matters::PreparedDocumentImport<'_>,
    ) -> Reply {
        match self.store.as_mut() {
            Some(store) => store.complete_cover_import(prepared),
            None => reply(423, json!({"detail":"Unlock the vault first"})),
        }
    }

    pub fn capture_document_upload(
        &self,
        target: crate::matters::DocumentUploadTarget,
        body: Value,
    ) -> Result<crate::matters::DocumentUploadBatch, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .capture_document_upload(target, body)
    }

    pub fn prepare_annotation_image(
        &self,
        document: &str,
        body: Value,
    ) -> Result<crate::matters::AnnotationImageWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_annotation_image(document, body)
    }

    pub fn complete_annotation_image(
        &mut self,
        measured: crate::matters::MeasuredAnnotationImage,
    ) -> Result<Reply, Reply> {
        self.store
            .as_mut()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .complete_annotation_image(measured)
    }

    pub fn prepare_annotation_asset(
        &self,
        id: &str,
    ) -> Result<crate::matters::AnnotationAssetWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_annotation_asset(id)
    }

    pub fn prepare_output_preview(
        &self,
        target: crate::matters::OutputPreviewTarget,
        body: &Value,
    ) -> Result<crate::matters::OutputPreviewWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_output_preview(target, body)
    }

    pub fn complete_output_preview(
        &self,
        measured: crate::matters::MeasuredOutputPreview,
    ) -> Result<Reply, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .complete_output_preview(measured)
    }

    pub fn prepare_metadata_review(
        &self,
        target: crate::matters::MetadataReviewTarget,
        body: &Value,
    ) -> Result<crate::matters::MetadataReviewWork, Reply> {
        self.prepare_metadata_review_with_order(target, body, &crate::RequestOrder::default())
    }

    pub fn prepare_metadata_review_with_order(
        &self,
        target: crate::matters::MetadataReviewTarget,
        body: &Value,
        order: &crate::RequestOrder,
    ) -> Result<crate::matters::MetadataReviewWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_metadata_review_with_order(target, body, order)
    }

    pub fn complete_metadata_review(
        &mut self,
        measured: crate::matters::MeasuredMetadataReview,
    ) -> Result<Reply, Reply> {
        self.store
            .as_mut()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .complete_metadata_review(measured)
    }

    pub fn prepare_label_review(
        &self,
        target: crate::matters::LabelReviewTarget,
        body: &Value,
    ) -> Result<crate::matters::LabelReviewWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_label_review(target, body)
    }

    pub fn complete_label_review(
        &mut self,
        measured: crate::matters::MeasuredLabelReview,
    ) -> Result<Reply, Reply> {
        self.store
            .as_mut()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .complete_label_review(measured)
    }

    pub fn prepare_search(
        &self,
        target: crate::matters::SearchTarget,
        query: &str,
    ) -> Result<crate::matters::SearchWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_search(target, query)
    }

    pub fn prepare_original_info(
        &self,
        document: &str,
    ) -> Result<crate::matters::OriginalInfoWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_original_info(document)
    }

    pub fn open_pdf(&self, id: &str, artifact: &Artifact) -> Result<PdfSnapshot, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .open_pdf(id, artifact)
    }

    pub fn editable_snapshot(
        &self,
        id: &str,
        revision: u64,
        acknowledged: bool,
    ) -> Result<PdfSnapshot, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .editable_snapshot(id, Some(revision), acknowledged)
    }

    pub fn prepare_pdf_snapshot(
        &self,
        id: &str,
        artifact: &Artifact,
    ) -> Result<crate::matters::PdfSnapshotWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_pdf_snapshot(id, artifact)
    }

    pub fn prepare_editable_snapshot(
        &self,
        id: &str,
        revision: u64,
        acknowledged: bool,
    ) -> Result<crate::matters::PdfSnapshotWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_editable_snapshot(id, Some(revision), acknowledged)
    }

    pub fn prepare_document_output(
        &self,
        target: crate::matters::DocumentOutputTarget,
        body: Value,
    ) -> Result<crate::matters::DocumentOutputWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_document_output(target, body)
    }

    pub fn portable_snapshot(&self, matter: &str) -> Result<PdfSnapshot, Reply> {
        let error = |status, message: &str| reply(status, json!({"detail":message}));
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| error(423, "Unlock the vault first"))?;
        let envelope = self
            .load_metadata()
            .map_err(|_| error(500, "Cannot read vault metadata"))?
            .ok_or_else(|| error(409, "Vault metadata is missing"))?
            .envelope;
        store.portable_snapshot(matter, Some(&envelope))
    }

    pub fn restore_portable(
        &mut self,
        input: impl std::io::Read + std::io::Seek,
        credential: Option<&str>,
        recovery: bool,
    ) -> Reply {
        let Some(store) = self.store.as_mut() else {
            return reply(423, json!({"detail":"Unlock the vault first"}));
        };
        match store.restore_package(input, credential, recovery) {
            Ok(matter) => reply(201, matter),
            Err(error) => reply(422, json!({"detail":error})),
        }
    }

    pub fn capture_portable_restore(&self) -> Result<crate::matters::PortableRestoreWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .capture_portable_restore()
    }

    pub fn complete_portable_restore(
        &mut self,
        prepared: crate::matters::PreparedPortableRestore,
    ) -> Reply {
        match self.store.as_mut() {
            Some(store) => store.complete_portable_restore(prepared),
            None => reply(423, json!({"detail":"Unlock the vault first"})),
        }
    }

    pub fn replace_working_file(
        &mut self,
        document: &str,
        filename: &str,
        bytes: &[u8],
        fields: &Value,
    ) -> Reply {
        self.store.as_mut().map_or_else(
            || reply(423, json!({"detail":"Unlock the vault first"})),
            |store| store.replace_working_file(document, filename, bytes, fields),
        )
    }

    pub fn prepare_uploaded_replacement(
        &self,
        document: &str,
        body: Value,
    ) -> Result<crate::matters::UploadedReplacement, Reply> {
        Ok(self
            .store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_uploaded_replacement(document, body))
    }

    pub fn prepare_working_replacement(
        &self,
        document: &str,
        fields: &Value,
    ) -> Result<crate::matters::WorkingReplacement, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_working_replacement(document, fields)
    }

    pub fn complete_working_replacement(
        &mut self,
        measured: crate::matters::MeasuredWorkingReplacement,
    ) -> Result<Reply, Reply> {
        self.store
            .as_mut()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .complete_working_replacement(measured)
    }

    pub fn write_portable_backup<W: Write>(
        &self,
        matter: &str,
        destination: W,
    ) -> Result<W, Reply> {
        let Some(store) = self.store.as_ref() else {
            return Err(reply(423, json!({"detail":"Unlock the vault first"})));
        };
        let envelope = self
            .load_metadata()
            .map_err(|_| reply(500, json!({"detail":"Cannot read vault metadata"})))?
            .ok_or_else(|| reply(409, json!({"detail":"Vault metadata is missing"})))?
            .envelope;
        store
            .write_portable(
                matter,
                Some(&envelope),
                destination,
                crate::matters::PORTABLE_PACKAGE_MAX_BYTES,
            )
            .map_err(|error| reply(409, json!({"detail":error})))
    }

    pub fn capture_portable_backup(
        &self,
        matter: &str,
    ) -> Result<crate::matters::PortableBackupWork, Reply> {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?;
        let envelope = self
            .load_metadata()
            .map_err(|_| reply(500, json!({"detail":"Cannot read vault metadata"})))?
            .ok_or_else(|| reply(409, json!({"detail":"Vault metadata is missing"})))?
            .envelope;
        store.capture_portable_backup(matter, Some(envelope))
    }

    pub fn complete_portable_backup<W>(
        &self,
        generated: crate::matters::GeneratedPortableBackup<W>,
    ) -> Result<W, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .complete_portable_backup(generated)
    }

    pub fn export_snapshot(
        &self,
        matter: &str,
        id: &str,
        name: &str,
        operation: &str,
        options: &Value,
    ) -> Result<PdfSnapshot, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .export_snapshot(matter, id, name, operation, options)
    }

    pub fn prepare_export_snapshot(
        &self,
        matter: &str,
        id: &str,
        name: &str,
        operation: &str,
        options: Value,
    ) -> Result<crate::matters::ExportSnapshotWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_export_snapshot(matter, id, name, operation, options)
    }

    pub fn prepare_export_output(
        &self,
        target: crate::matters::ExportOutputTarget,
        body: Value,
    ) -> Result<crate::matters::ExportOutputWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_export_output(target, body)
    }

    pub fn prepare_output_validation(
        &self,
        target: crate::matters::OutputValidationTarget,
    ) -> Result<crate::matters::OutputValidationWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_output_validation(target)
    }

    pub fn complete_output_validation(
        &self,
        measured: crate::matters::MeasuredOutputValidation,
    ) -> Result<Reply, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .complete_output_validation(measured)
    }

    pub fn begin_work(&mut self) -> Result<Option<crate::matters::PendingWork>, String> {
        self.store.as_mut().map_or(Ok(None), Store::begin_work)
    }

    pub fn begin_work_with_updates(
        &mut self,
        updated: impl FnMut(&str),
    ) -> Result<Option<crate::matters::PendingWork>, String> {
        self.store
            .as_mut()
            .map_or(Ok(None), |store| store.begin_work_with_updates(updated))
    }

    pub fn interrupt_processing_job(&mut self, id: &str) -> Result<(), String> {
        self.store
            .as_mut()
            .ok_or("Vault is locked")?
            .interrupt_processing_job(id)
    }

    pub fn checkpoint_processing_job(&self, id: &str) -> Result<(), Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .checkpoint_processing_job(id)
    }

    pub fn stop_processing(&mut self) -> Result<(), String> {
        self.store.as_mut().map_or(Ok(()), Store::stop_processing)
    }

    pub fn finish_work(&mut self, work: crate::matters::FinishedWork) -> Result<(), String> {
        self.store
            .as_mut()
            .ok_or("Vault is locked")?
            .finish_work(work)
    }

    pub fn prepare_screen_submission(
        &self,
        matter: &str,
        body: &Value,
    ) -> Result<crate::matters::ScreenSubmission, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_screen_submission(matter, body)
    }

    pub fn submit_screening(
        &mut self,
        verified: crate::matters::VerifiedScreenSubmission,
    ) -> Result<Reply, Reply> {
        self.store
            .as_mut()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .submit_screening(verified)
    }

    pub fn prepare_preflight(&self, matter: &str) -> Result<crate::matters::PreflightWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_preflight(matter)
    }

    pub fn prepare_bundle_preview(
        &self,
        matter: &str,
        body: &Value,
    ) -> Result<crate::matters::BundlePreviewWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_bundle_preview(matter, body)
    }

    pub fn prepare_volume_planning(
        &self,
        matter: &str,
        body: &Value,
    ) -> Result<crate::matters::BundlePreviewWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_volume_planning(matter, body)
    }

    pub fn complete_bundle_preview(
        &self,
        measured: crate::matters::MeasuredBundlePreview,
    ) -> Result<Reply, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .complete_bundle_preview(measured)
    }

    pub fn publish_preflight(
        &mut self,
        measured: crate::matters::MeasuredPreflight,
    ) -> Result<Reply, Reply> {
        self.store
            .as_mut()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .publish_preflight(measured)
    }

    pub fn prepare_variant_review(
        &self,
        matter: &str,
        id: &str,
    ) -> Result<crate::matters::VariantReviewWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_variant_review(matter, id)
    }

    pub fn prepare_composition_draft(
        &self,
        target: crate::matters::CompositionDraftTarget,
        body: &Value,
    ) -> Result<crate::matters::CompositionDraftWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_composition_draft(target, body)
    }

    pub fn prepare_composition_catalog(
        &self,
        matter: &str,
        kind: crate::matters::CompositionCatalogKind,
    ) -> Result<crate::matters::CompositionCatalogWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_composition_catalog(matter, kind)
    }

    pub fn prepare_export_comparison(
        &self,
        matter: &str,
        build: &str,
        operation: &str,
        body: &Value,
    ) -> Result<crate::matters::ExportComparisonWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_export_comparison(matter, build, operation, body)
    }

    pub fn complete_export_comparison(
        &self,
        measured: crate::matters::MeasuredExportComparison,
    ) -> Result<Reply, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .complete_export_comparison(measured)
    }

    pub fn complete_composition_catalog(
        &self,
        verified: crate::matters::VerifiedCompositionCatalog,
    ) -> Result<Reply, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .complete_composition_catalog(verified)
    }

    pub fn complete_composition_draft(
        &self,
        measured: crate::matters::MeasuredCompositionDraft,
    ) -> Result<Reply, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .complete_composition_draft(measured)
    }

    pub fn complete_variant_review(
        &self,
        measured: crate::matters::MeasuredVariantReview,
    ) -> Result<Reply, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .complete_variant_review(measured)
    }

    pub fn prepare_build_submission(
        &self,
        matter: &str,
        kind: &str,
        target: Option<&str>,
        body: &Value,
    ) -> Result<crate::matters::ProcessingSubmission, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_build_submission(matter, kind, target, body)
    }

    pub fn prepare_retry(
        &self,
        id: &str,
        body: &Value,
    ) -> Result<crate::matters::ProcessingSubmission, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_retry(id, body)
    }

    pub fn submit_processing(
        &mut self,
        prepared: crate::matters::PreparedProcessingSubmission,
    ) -> Result<Reply, Reply> {
        self.store
            .as_mut()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .submit_processing(prepared)
    }

    pub fn page_output(
        &mut self,
        id: &str,
        output: crate::matters::PageOutput,
        selection: &str,
        acknowledged: bool,
    ) -> Result<Vec<u8>, Reply> {
        self.store
            .as_mut()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .page_output(id, output, selection, acknowledged)
    }

    pub fn prepare_page_output(
        &self,
        id: &str,
        output: crate::matters::PageOutput,
        selection: &str,
        acknowledged: bool,
    ) -> Result<crate::matters::PageOutputWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_page_output(id, output, selection, acknowledged)
    }

    pub fn prepare_page_preview(
        &self,
        target: crate::matters::PagePreviewTarget,
        query: &str,
    ) -> Result<crate::matters::PagePreviewWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_page_preview(target, query)
    }

    pub fn prepare_redaction(
        &self,
        target: crate::matters::RedactionTarget,
        body: Value,
        order: &crate::RequestOrder,
    ) -> Result<crate::matters::RedactionWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_redaction(target, body, order)
    }

    pub fn complete_redaction(
        &mut self,
        measured: crate::matters::MeasuredRedaction,
    ) -> Result<Reply, Reply> {
        self.store
            .as_mut()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .complete_redaction(measured)
    }

    pub fn prepare_revision_edit(
        &self,
        target: crate::matters::RevisionTarget,
        body: Value,
        order: &crate::RequestOrder,
    ) -> Result<crate::matters::RevisionEditWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_revision_edit(target, body, order)
    }

    pub fn complete_revision_edit(
        &mut self,
        measured: crate::matters::MeasuredRevisionEdit,
    ) -> Result<Reply, Reply> {
        self.store
            .as_mut()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .complete_revision_edit(measured)
    }

    pub fn prepare_ocr_review(&self, id: &str, query: &str) -> Result<OcrReviewWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_ocr_review(id, query)
    }

    pub fn complete_ocr_review(&self, measured: OcrReviewMeasurement) -> Result<Reply, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .complete_ocr_review(measured)
    }

    pub fn prepare_text_page(
        &self,
        target: crate::matters::TextPageTarget,
        body: &Value,
    ) -> Result<crate::matters::TextPageWork, Reply> {
        self.store
            .as_ref()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .prepare_text_page(target, body)
    }

    pub fn complete_text_page(
        &mut self,
        measured: crate::matters::MeasuredTextPage,
    ) -> Result<Reply, Reply> {
        self.store
            .as_mut()
            .ok_or_else(|| reply(423, json!({"detail":"Unlock the vault first"})))?
            .complete_text_page(measured)
    }

    /// Core request contract for the isolated Rust vault. IPC scheduling belongs to the host.
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
        let error = |status, detail: String| reply(status, json!({"detail":detail}));
        match (method, path) {
            ("GET", "/health") => {
                let mut response = crate::request(method, path);
                if self.store.is_none() {
                    response.body["status"] = json!("locked");
                }
                response
            }
            ("GET", "/capabilities") => {
                let mut response = crate::request(method, path);
                response.body["storage"] = json!("isolated-protected");
                response.body["vault"] = json!(true);
                response
            }
            ("GET", "/vault/status") => {
                let state = if self.store.is_some() {
                    "unlocked"
                } else {
                    match self.load_metadata() {
                        Ok(Some(_)) => "locked",
                        Ok(None) => "setup",
                        Err(message) => return error(500, message),
                    }
                };
                reply(200, json!({"state":state,"enabled":true}))
            }
            ("GET", "/profiles") if self.store.is_some() => crate::request(method, path),
            ("POST", "/vault/setup") => match self
                .setup(body["password"].as_str().unwrap_or_default())
            {
                Ok(recovery) => reply(200, json!({"state":"unlocked","recovery_key":&*recovery})),
                Err(message) => error(422, message),
            },
            ("POST", "/vault/unlock") => {
                let value = body["credential"].as_str().unwrap_or_default();
                let credential = if body["recovery"] == true {
                    vault::Credential::Recovery(value)
                } else {
                    vault::Credential::Password(value)
                };
                match self.unlock(credential) {
                    Ok(()) => reply(200, json!({"state":"unlocked"})),
                    Err(message) => error(403, message),
                }
            }
            ("POST", "/vault/rotate") if self.store.is_some() => {
                match self.rotate(body["new_password"].as_str().unwrap_or_default()) {
                    Ok(recovery) => reply(200, json!({"recovery_key":&*recovery})),
                    Err(message) => error(422, message),
                }
            }
            ("GET", path) if path.starts_with("/matters/") && path.ends_with("/portable") => {
                let route = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
                if route.len() != 3 {
                    return crate::unsupported();
                }
                let envelope = match self.load_metadata() {
                    Ok(Some(metadata)) => metadata.envelope,
                    Ok(None) => return error(423, "Unlock the vault first".into()),
                    Err(message) => return error(500, message),
                };
                self.store
                    .as_mut()
                    .and_then(|store| {
                        store.portable_request(method, &route, &body, Some(&envelope))
                    })
                    .unwrap_or_else(|| error(423, "Unlock the vault first".into()))
            }
            _ => match self.store.as_mut() {
                Some(store) => store.request_with_order(method, path, body, order),
                None => error(423, "Unlock the vault first".into()),
            },
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Command;

    const PASSWORD: &str = "synthetic crash fixture password";
    const PDF: &[u8] = include_bytes!("../../samples/rust_pdf_corpus/unsigned.pdf");

    fn rotate(vault: &mut VaultStore, id: &str) -> Reply {
        vault.request(
            "POST",
            &format!("/documents/{id}/pages/rotate"),
            json!({"page_index":0,"degrees":90,"expected_revision":1}),
        )
    }

    #[test]
    #[ignore = "child process entry point"]
    fn protected_publication_child() {
        let root = std::env::var_os("MATTERDRAFT_PROTECTED_TEST_ROOT").unwrap();
        let matter = std::env::var("MATTERDRAFT_PROTECTED_TEST_MATTER").unwrap();
        let mut vault = VaultStore::open(Path::new(&root)).unwrap();
        vault.unlock(vault::Credential::Password(PASSWORD)).unwrap();
        if let Ok(document) = std::env::var("MATTERDRAFT_PROTECTED_TEST_DOCUMENT") {
            assert_eq!(rotate(&mut vault, &document).status, 200);
        } else {
            assert_eq!(
                vault
                    .import_pdf(&matter, "unsigned.pdf", PDF, ImportOptions::default())
                    .status,
                201
            );
        }
    }

    #[test]
    fn protected_pdf_transactions_recover_before_and_after_commit() {
        for phase in [
            "staging",
            "first_sync",
            "rename",
            "publication",
            "committed",
            "revision_synced",
            "revision_published",
            "revision_committed",
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().join("vault");
            let mut vault = VaultStore::open(&root).unwrap();
            vault.setup(PASSWORD).unwrap();
            let matter = vault.request(
                "POST",
                "/matters",
                json!({"name":"Synthetic protected interruption","court_profile":"general-demo"}),
            );
            assert_eq!(matter.status, 201);
            let mid = matter.body["id"].as_str().unwrap();
            let expected_before = if phase.starts_with("revision_") {
                let result = vault.import_pdf(mid, "unsigned.pdf", PDF, ImportOptions::default());
                assert_eq!(result.status, 201);
                Some(result.body[0]["id"].as_str().unwrap().to_owned())
            } else {
                None
            };
            drop(vault);
            let mut child = Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "vault_store::tests::protected_publication_child",
                    "--ignored",
                ])
                .env("MATTERDRAFT_TEST_CRASH_PHASE", phase)
                .env("MATTERDRAFT_PROTECTED_TEST_ROOT", &root)
                .env("MATTERDRAFT_PROTECTED_TEST_MATTER", mid)
                .env_remove("MATTERDRAFT_PROTECTED_TEST_DOCUMENT");
            if let Some(id) = &expected_before {
                child.env("MATTERDRAFT_PROTECTED_TEST_DOCUMENT", id);
            }
            assert_eq!(child.output().unwrap().status.code(), Some(97), "{phase}");
            let mut vault = VaultStore::open(&root).unwrap();
            vault.unlock(vault::Credential::Password(PASSWORD)).unwrap();
            let documents = vault.request("GET", &format!("/matters/{mid}/documents"), Value::Null);
            assert_eq!(documents.status, 200, "{phase}: {documents:?}");
            let has_document = expected_before.is_some() || phase == "committed";
            assert_eq!(
                documents.body.as_array().unwrap().len(),
                usize::from(has_document),
                "{phase}"
            );
            let folders = fs::read_dir(root.join("documents-v1")).unwrap().count();
            assert_eq!(folders, usize::from(has_document), "{phase}");
            if has_document {
                let id = documents.body[0]["id"].as_str().unwrap();
                let latest = if phase == "revision_committed" { 2 } else { 1 };
                let history =
                    vault.request("GET", &format!("/documents/{id}/revisions"), Value::Null);
                assert_eq!(history.status, 200, "{phase}");
                assert_eq!(documents.body[0]["revision"], latest, "{phase}");
                assert_eq!(
                    history.body.as_array().unwrap().len(),
                    latest as usize,
                    "{phase}"
                );
                assert_eq!(
                    fs::read_dir(root.join("documents-v1").join(id))
                        .unwrap()
                        .count(),
                    latest as usize + 2,
                    "{phase}"
                );
            }
        }
    }

    #[test]
    fn pending_setup_resumes_without_overwriting_vault_metadata() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("vault");
        let created = vault::create_key_envelope(PASSWORD).unwrap();
        let vault = VaultStore::open(&root).unwrap();
        vault
            .save_metadata(&Metadata {
                format: "courtbundle-vault".into(),
                version: 1,
                state: State::Migrating,
                envelope: created.envelope.clone(),
            })
            .unwrap();
        secure_db::create_protected(&root.join(DATABASE_STAGE), &created.data_key).unwrap();
        fs::write(root.join(METADATA_STAGE), b"Interrupted rotation stage").unwrap();
        let key = created.recovery_key.to_string();
        drop(vault);
        let mut vault = VaultStore::open(&root).unwrap();
        assert!(vault.setup("another test password").is_err());
        vault.unlock(vault::Credential::Recovery(&key)).unwrap();
        let metadata = vault.load_metadata().unwrap().unwrap();
        assert!(metadata.state == State::Active);
        assert_eq!(metadata.envelope, created.envelope);
        assert!(!root.join(DATABASE_STAGE).exists());
        assert!(!root.join(METADATA_STAGE).exists());
    }
}
