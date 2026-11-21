//! Portable matter v19 encoding and staged restore into an isolated protected store.
use super::{
    artifacts::{refusal, FileVersion, PdfSnapshot},
    bundle_inputs, preparation, revisions, *,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{Cursor, Seek, SeekFrom},
};
use zip::{
    write::{SimpleFileOptions, StreamWriter},
    CompressionMethod, ZipArchive, ZipWriter,
};

const IPC_LIMIT: usize = 160 * 1024 * 1024;
pub(super) const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_PACKAGE_BYTES: u64 = 4 * MAX_FILE_BYTES;
const ASSET_LIMIT: u64 = 10 * 1024 * 1024;
const MANIFEST_LIMIT: usize = 10 * 1024 * 1024;
const VERSION: u64 = 19;
const INVALID: &str = "Invalid portable matter package";

fn id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn list<'a>(value: &'a Value, key: &str) -> Result<&'a [Value], String> {
    value[key]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("Missing package {key}"))
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("Invalid package field {key}"))
}

fn number(value: &Value, key: &str) -> Result<u64, String> {
    value[key]
        .as_u64()
        .ok_or_else(|| format!("Invalid package field {key}"))
}

fn sql(error: rusqlite::Error) -> String {
    error.to_string()
}
fn io(error: std::io::Error) -> String {
    error.to_string()
}

fn safe_name(path: &str) -> bool {
    let Some((root, name)) = path.split_once('/') else {
        return false;
    };
    matches!(root, "originals" | "working" | "assets")
        && !name.is_empty()
        && name.len() <= 250
        && !name.starts_with('.')
        && !name.contains(['/', '\\', '\0'])
        && name.chars().all(|char| !char.is_control())
}

fn copy_hashed(
    input: impl Read,
    output: &mut impl Write,
    limit: u64,
) -> Result<(u64, String), String> {
    let mut input = input.take(limit + 1);
    let mut hasher = Sha256::new();
    let mut count = 0;
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let size = input.read(&mut buffer).map_err(io)?;
        if size == 0 {
            return Ok((count, vault::hex_bytes(&hasher.finalize()).to_string()));
        }
        count += size as u64;
        if count > limit {
            return Err("Matter artifact exceeds its size limit".into());
        }
        let chunk = &buffer[..size];
        hasher.update(chunk);
        output.write_all(chunk).map_err(io)?;
    }
}

struct LimitedWriter<W> {
    target: W,
    available: u64,
}

impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.available {
            return Err(std::io::Error::other(
                "Matter backup exceeds the package limit",
            ));
        }
        let count = self.target.write(bytes)?;
        self.available -= count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.target.flush()
    }
}

struct PackageWriter<W: Write> {
    archive: ZipWriter<StreamWriter<W>>,
    files: BTreeMap<String, Value>,
    total: u64,
    limit: u64,
}

impl<W: Write> PackageWriter<W> {
    fn entry(&mut self, name: &str, data: &[u8]) -> Result<(), String> {
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        self.archive
            .start_file(name, options)
            .map_err(|error| error.to_string())?;
        self.archive.write_all(data).map_err(io)?;
        Ok(())
    }

    fn file(&mut self, name: &str, input: impl Read, expected: &str) -> Result<(), String> {
        if !safe_name(name) {
            return Err("Invalid portable artifact path".into());
        }
        if let Some(previous) = self.files.get(name) {
            return if previous["sha256"] == expected {
                Ok(())
            } else {
                Err("Conflicting matter artifact identity".into())
            };
        }
        let limit = MAX_FILE_BYTES.min(self.limit - self.total);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .large_file(true);
        self.archive
            .start_file(format!("files/{name}"), options)
            .map_err(|error| error.to_string())?;
        let (count, hash) = copy_hashed(input, &mut self.archive, limit)?;
        if hash != expected {
            return Err("Matter artifact failed its integrity check".into());
        }
        self.total += count;
        self.files.insert(
            name.into(),
            json!({"path":name,"bytes":count,"sha256":hash}),
        );
        Ok(())
    }
}

enum BackupArtifact {
    Document {
        member: String,
        path: String,
        hash: String,
    },
    Asset {
        member: String,
        id: String,
        rowid: i64,
        length: u64,
    },
}

struct PortablePlan {
    manifest: Value,
    artifacts: Vec<BackupArtifact>,
}

pub struct PortableBackupWork {
    matter: String,
    root: PathBuf,
    reader: StoreReader,
    envelope: Option<Value>,
}

pub struct GeneratedPortableBackup<W> {
    matter: String,
    root: PathBuf,
    output: W,
}

impl PortableBackupWork {
    pub fn compute<W: Write>(self, destination: W) -> Result<GeneratedPortableBackup<W>, Reply> {
        self.compute_limit(destination, MAX_PACKAGE_BYTES)
    }

    pub fn compute_reply(self) -> Result<GeneratedPortableBackup<Reply>, Reply> {
        let mime = if self.reader.key.is_some() {
            "application/octet-stream"
        } else {
            "application/zip"
        };
        let GeneratedPortableBackup {
            matter,
            root,
            output,
        } = self.compute_limit(Vec::new(), IPC_LIMIT as u64)?;
        Ok(GeneratedPortableBackup {
            matter,
            root,
            output: super::document_output::binary(output, mime),
        })
    }

    fn compute_limit<W: Write>(
        self,
        destination: W,
        limit: u64,
    ) -> Result<GeneratedPortableBackup<W>, Reply> {
        let result = (|| -> Result<W, String> {
            let mut con = self.reader.open().map_err(|error| error.to_string())?;
            let tx = con.transaction().map_err(sql)?;
            let plan = capture_zip(&tx, self.reader.key.as_ref(), &self.matter)?;
            tx.commit().map_err(sql)?;
            write_package(
                plan,
                &con,
                &self.root,
                self.reader.key.as_ref(),
                self.envelope.as_ref(),
                destination,
                limit,
            )
        })()
        .map_err(|error| refusal(409, error))?;
        Ok(GeneratedPortableBackup {
            matter: self.matter,
            root: self.root,
            output: result,
        })
    }
}

fn artifact_name(doc: &Value) -> &str {
    match doc["original_name"]
        .as_str()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "png",
        Some("jpg" | "jpeg") => "jpg",
        _ => "pdf",
    }
}

impl Store {
    pub fn portable_snapshot(
        &self,
        matter: &str,
        envelope: Option<&Value>,
    ) -> Result<PdfSnapshot, Reply> {
        self.portable(matter, envelope)
            .map(PdfSnapshot::from_bytes)
            .map_err(|error| refusal(409, error))
    }

    pub fn write_portable_backup<W: Write>(
        &self,
        matter: &str,
        destination: W,
    ) -> Result<W, Reply> {
        self.write_portable(matter, None, destination, MAX_PACKAGE_BYTES)
            .map_err(|error| refusal(409, error))
    }

    fn portable(&self, matter_id: &str, envelope: Option<&Value>) -> Result<Vec<u8>, String> {
        self.write_portable(matter_id, envelope, Vec::new(), IPC_LIMIT as u64)
    }

    pub(crate) fn write_portable<W: Write>(
        &self,
        matter: &str,
        envelope: Option<&Value>,
        destination: W,
        limit: u64,
    ) -> Result<W, String> {
        let plan = capture_zip(&self.con, self.key.as_ref(), matter)?;
        let output = write_package(
            plan,
            &self.con,
            &self.root,
            self.key.as_ref(),
            envelope,
            destination,
            limit,
        )?;
        self.record_portable_backup(matter)?;
        Ok(output)
    }

    pub fn capture_portable_backup(
        &self,
        matter: &str,
        envelope: Option<Value>,
    ) -> Result<PortableBackupWork, Reply> {
        if !self
            .active_matter(matter)
            .map_err(|error| refusal(500, error.to_string()))?
        {
            return Err(refusal(404, "Matter not found"));
        }
        let reader = StoreReader::capture(self).map_err(|error| refusal(500, error.to_string()))?;
        Ok(PortableBackupWork {
            matter: matter.to_owned(),
            root: self.root.clone(),
            reader,
            envelope,
        })
    }

    pub fn complete_portable_backup<W>(
        &self,
        backup: GeneratedPortableBackup<W>,
    ) -> Result<W, Reply> {
        if backup.root != self.root {
            return Err(refusal(409, "The destination vault changed during backup"));
        }
        self.record_portable_backup(&backup.matter)
            .map_err(|error| refusal(409, error))?;
        Ok(backup.output)
    }

    fn record_portable_backup(&self, matter: &str) -> Result<(), String> {
        self.con.execute("INSERT INTO audit_events (matter_id,action,created_at) VALUES (?,'matter_backed_up',?)",
            params![matter, now()]).map_err(sql)?;
        Ok(())
    }

    pub(crate) fn portable_request(
        &mut self,
        method: &str,
        route: &[&str],
        body: &Value,
        envelope: Option<&Value>,
    ) -> Option<Reply> {
        match (method, route) {
            ("GET", ["matters", matter, "portable"]) => {
                let bytes = self
                    .portable(matter, envelope)
                    .map_err(|error| refusal(409, error));
                let mime = if self.key.is_some() {
                    "application/octet-stream"
                } else {
                    "application/zip"
                };
                Some(
                    bytes
                        .map(|bytes| super::document_output::binary(bytes, mime))
                        .unwrap_or_else(|error| error),
                )
            }
            ("POST", ["matters", "restore"]) => {
                Some(self.restore_body(body).unwrap_or_else(|error| error))
            }
            _ => None,
        }
    }

    fn restore_body(&mut self, body: &Value) -> Result<Reply, Reply> {
        let bytes = restore_upload_bytes(body)?;
        let work = self.capture_portable_restore()?;
        let prepared = work.compute(
            Cursor::new(bytes),
            body["credential"].as_str(),
            body["recovery"] == true,
        )?;
        Ok(self.complete_portable_restore(prepared))
    }

    pub fn capture_portable_restore(&self) -> Result<PortableRestoreWork, Reply> {
        let key = self.key.clone().ok_or_else(|| {
            refusal(
                422,
                "Unlock a protected Rust store before restoring a backup",
            )
        })?;
        Ok(PortableRestoreWork {
            root: self.root.clone(),
            key,
        })
    }

    pub fn complete_portable_restore(&mut self, prepared: PreparedPortableRestore) -> Reply {
        self.publish_portable_restore(prepared)
            .map_or_else(|error| refusal(422, error), |matter| reply(201, matter))
    }

    pub(crate) fn restore_package(
        &mut self,
        source: impl Read + Seek,
        credential: Option<&str>,
        recovery: bool,
    ) -> Result<Value, String> {
        let work = self
            .capture_portable_restore()
            .map_err(|error| error.body["detail"].as_str().unwrap_or(INVALID).to_owned())?;
        self.publish_portable_restore(work.stage(source, credential, recovery)?)
    }

    fn publish_portable_restore(
        &mut self,
        prepared: PreparedPortableRestore,
    ) -> Result<Value, String> {
        let PreparedPortableRestore {
            staging,
            matter_id,
            mut matter,
            manifest,
            documents,
            remap,
            assets,
        } = prepared;
        let key = self
            .key
            .as_ref()
            .ok_or("Unlock a protected Rust store before restoring a backup")?;
        if staging.path.parent() != Some(self.root.as_path())
            || key.expose() != staging.key.expose()
        {
            return Err("Destination vault changed during portable restore".into());
        }
        let stamp = now();
        matter["updated_at"] = json!(stamp);
        let mut published = Vec::new();
        let root = self.root.join("documents-v1");
        let tx = self
            .con
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let result = (|| {
            tx.execute(
                "INSERT INTO matters VALUES (?,?,?)",
                params![matter_id, matter.to_string(), stamp],
            )
            .map_err(sql)?;
            for document in &documents {
                let id = &document.id;
                tx.execute(
                    "INSERT INTO documents VALUES (?,?,?,?,?,?,?,?)",
                    params![
                        id,
                        matter_id,
                        document.body.to_string(),
                        document.hash,
                        format!("documents-v1/{id}/original.pdf"),
                        document.working,
                        field(&document.body, "created_at")?,
                        document.body["archived_at"].as_str()
                    ],
                )
                .map_err(sql)?;
                for version in &document.retained {
                    tx.execute(
                        "INSERT INTO document_revisions VALUES (?,?,?,?,?,?,?,?)",
                        params![
                            id,
                            version.number,
                            version.relpath,
                            version.hash,
                            version.created,
                            version.pages,
                            version.ocr,
                            version.provenance
                        ],
                    )
                    .map_err(sql)?;
                }
            }
            restore_metadata(&tx, &matter_id, &manifest, &assets, key, &remap)?;
            for document in &documents {
                let target = root.join(&document.id);
                if target.exists() {
                    return Err("Restored document path collision".into());
                }
                publish_directory(&staging.path.join(&document.id), &target).map_err(io)?;
                published.push(target);
            }
            sync_directory(&root)?;
            crash_at("restore_published");
            tx.execute("INSERT INTO audit_events (matter_id,action,created_at) VALUES (?,'matter_restored_from_package',?)",
                params![matter_id,stamp]).map_err(sql)?;
            tx.commit().map_err(sql)?;
            crash_at("restore_committed");
            Ok(matter)
        })();
        if result.is_err() {
            let found = self
                .con
                .query_row("SELECT 1 FROM matters WHERE id=?", [&matter_id], |row| {
                    row.get::<_, i64>(0)
                })
                .optional();
            if matches!(found, Ok(None)) {
                for directory in published {
                    let _ = remove_document_directory(&directory);
                }
            }
        }
        result
    }
}

fn capture_zip(
    con: &Connection,
    key: Option<&DataKey>,
    matter_id: &str,
) -> Result<PortablePlan, String> {
    let matter = document_metadata::matter(con, matter_id)
        .map_err(sql)?
        .ok_or("Matter not found")?;
    let mut artifacts = Vec::new();
    let mut documents = Vec::new();
    let mut revisions = Vec::new();
    let mut ocr = Vec::new();
    let mut redactions = Vec::new();
    let mut query = con.prepare("SELECT id,body,source_sha256,original_relpath FROM documents WHERE matter_id=? ORDER BY archived_at, json_extract(body,'$.position')").map_err(sql)?;
    let rows = query
        .query_map([matter_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                decode(&row.get::<_, String>(1)?)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(sql)?;
    for row in rows {
        let (doc_id, mut document, hash, source) = row.map_err(sql)?;
        if !artifacts::valid_id(&doc_id)
            || source != format!("documents-v1/{doc_id}/original.pdf")
            || document["source_sha256"] != hash
        {
            return Err("Invalid source resource".into());
        }
        let original = format!("originals/{doc_id}.{}", artifact_name(&document));
        artifacts.push(BackupArtifact::Document {
            member: original.clone(),
            path: source,
            hash,
        });
        document["source_relpath"] = json!(original);
        let current = number(&document, "revision")?;
        let mut rows = con.prepare("SELECT revision,relpath,sha256,created_at FROM document_revisions WHERE document_id=? ORDER BY revision").map_err(sql)?;
        let entries = rows
            .query_map([&doc_id], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(sql)?;
        for entry in entries {
            let (number, relative, hash, stamp) = entry.map_err(sql)?;
            let filename = if let Some(key) = key {
                revisions::file_name(key, &doc_id, number)?
            } else {
                "revision-1.pdf".into()
            };
            let actual = format!("documents-v1/{doc_id}/{filename}");
            if relative != actual {
                let allowed = if let Some(key) = key {
                    if let Some(file) = relative.strip_prefix(&format!("documents-v1/{doc_id}/")) {
                        let version = file
                            .strip_prefix("revision-")
                            .and_then(|name| name.split(['-', '.']).next())
                            .and_then(|number| number.parse::<u64>().ok());
                        version.is_some_and(|version| version <= number)
                            && revisions::valid_file_name(key, &doc_id, file)?
                    } else {
                        false
                    }
                } else {
                    false
                };
                if !allowed {
                    return Err("Invalid working revision resource".into());
                }
            }
            let path = format!("working/{doc_id}-r{number}.pdf");
            let fields = if key.is_some() {
                revisions::record(con, &doc_id, number)
                    .map_err(sql)?
                    .ok_or("Missing working revision")?
                    .fields()
            } else {
                json!({"page_count":document["page_count"],"ocr_status":document["ocr_status"],"ocr_provenance_json":"[]"})
            };
            artifacts.push(BackupArtifact::Document {
                member: path.clone(),
                path: relative,
                hash,
            });
            if number == current {
                document["working_relpath"] = json!(path);
            }
            revisions.push(json!({"document_id":doc_id,"revision":number,"working_relpath":path,"created_at":stamp,
               "page_count":fields["page_count"],"ocr_status":fields["ocr_status"],"ocr_provenance_json":fields["ocr_provenance_json"]}));
        }
        if document["working_relpath"].is_null() {
            return Err("Missing current working revision".into());
        }
        if document["ocr_provenance_json"] == "{}" {
            document["ocr_provenance_json"] = json!("[]");
        }
        let mut records = con.prepare("SELECT revision,page,accepted_at FROM rust_ocr_reviews WHERE document_id=? ORDER BY revision,page").map_err(sql)?;
        let items = records
            .query_map([&doc_id], |row| {
                Ok(json!({"document_id":doc_id,"revision":row.get::<_,u64>(0)?,
            "page_index":row.get::<_,u64>(1)?,"accepted_at":row.get::<_,String>(2)?}))
            })
            .map_err(sql)?;
        for item in items {
            ocr.push(item.map_err(sql)?);
        }
        redactions.extend(super::redaction_records::records(con, &doc_id).map_err(sql)?);
        documents.push(document);
    }
    let annotations = super::annotations::raw(con, matter_id).map_err(sql)?;
    for annotation in &annotations {
        if annotation["kind"] != "image" {
            continue;
        }
        let id = field(annotation, "id")?;
        let path = field(annotation, "asset_relpath")?;
        let (rowid, length) = con
            .query_row(
                "SELECT rowid,length(bytes) FROM rust_annotation_assets WHERE id=?",
                [id],
                |row| Ok((row.get(0)?, row.get::<_, u64>(1)?)),
            )
            .optional()
            .map_err(sql)?
            .ok_or("Image overlay asset missing")?;
        if length > ASSET_LIMIT || !safe_name(path) {
            return Err("Invalid image overlay asset".into());
        }
        artifacts.push(BackupArtifact::Asset {
            member: path.to_owned(),
            id: id.to_owned(),
            rowid,
            length,
        });
    }
    let (marks, preset) = bundle_inputs::backup_settings(con, matter_id).map_err(sql)?;
    let (bookmarks, links) = bundle_inputs::backup_destinations(con, matter_id).map_err(sql)?;
    let (parties, values) = preparation::backup(con, matter_id).map_err(sql)?;
    let membership = con
        .query_row(
            "SELECT revision FROM bundle_membership_state WHERE matter_id=?",
            [matter_id],
            |row| row.get::<_, u64>(0),
        )
        .optional()
        .map_err(sql)?
        .unwrap_or(1);
    let manifest = json!({"format":"courtbundle-matter","version":VERSION,"created_at":now(),"matter":matter,
        "documents":documents,"revisions":revisions,"ocr_page_reviews":ocr,"redactions":redactions,"membership_revision":membership,
        "memo_parties":parties,"matter_values":values,"manual_bookmarks":bookmarks,"reviewed_links":links,
        "output_marks":marks,"output_preset":preset,"page_annotations":annotations});
    Ok(PortablePlan {
        manifest,
        artifacts,
    })
}

fn write_package<W: Write>(
    plan: PortablePlan,
    con: &Connection,
    root: &Path,
    key: Option<&DataKey>,
    envelope: Option<&Value>,
    destination: W,
    limit: u64,
) -> Result<W, String> {
    let output = LimitedWriter {
        target: destination,
        available: limit,
    };
    let output = if let Some(key) = key {
        let encrypted =
            vault::protected_package_writer(output, envelope.ok_or("Vault metadata missing")?, key)
                .map_err(|error| error.to_string())?;
        write_zip(plan, con, root, Some(key), encrypted, limit)?
            .finish()
            .map_err(|error| error.to_string())?
    } else {
        write_zip(plan, con, root, None, output, limit)?
    };
    Ok(output.target)
}

fn write_zip<W: Write>(
    plan: PortablePlan,
    con: &Connection,
    root: &Path,
    key: Option<&DataKey>,
    destination: W,
    limit: u64,
) -> Result<W, String> {
    let PortablePlan {
        mut manifest,
        mut artifacts,
    } = plan;
    let mut output = PackageWriter {
        archive: ZipWriter::new_stream(destination),
        files: BTreeMap::new(),
        total: 0,
        limit,
    };
    artifacts.sort_by(|a, b| a.name().cmp(b.name()));
    for artifact in artifacts {
        match artifact {
            BackupArtifact::Document { member, path, hash } => {
                let path = root.join(path);
                if path.parent().is_some_and(Path::is_symlink)
                    || root.join("documents-v1").is_symlink()
                {
                    return Err("Unsafe matter artifact path".into());
                }
                let file = open_document_file(&path)?;
                if let Some(key) = key {
                    let reader = vault::EncryptedReader::at(file, 0, key)
                        .map_err(|error| error.to_string())?;
                    output.file(&member, reader, &hash)?;
                } else {
                    output.file(&member, file, &hash)?;
                }
            }
            BackupArtifact::Asset {
                member,
                id,
                rowid,
                length,
            } => {
                let data = con.query_row(
                    "SELECT bytes FROM rust_annotation_assets WHERE id=? AND rowid=? AND length(bytes)=?",
                    params![id,rowid,length], |row| row.get::<_, Vec<u8>>(0),
                ).optional().map_err(sql)?.ok_or("Image overlay asset changed during backup")?;
                output.file(&member, &data[..], &sha256_hex(&data))?;
            }
        }
    }
    manifest["files"] = json!(output.files.values().collect::<Vec<_>>());
    let encoded = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
    if encoded.len() > MANIFEST_LIMIT {
        return Err("Portable manifest is too large".into());
    }
    output.entry("manifest.json", &encoded)?;
    output
        .archive
        .finish()
        .map(StreamWriter::into_inner)
        .map_err(|error| error.to_string())
}

impl BackupArtifact {
    fn name(&self) -> &str {
        match self {
            Self::Document { member, .. } | Self::Asset { member, .. } => member,
        }
    }
}

fn restore_upload_bytes(body: &Value) -> Result<Vec<u8>, Reply> {
    let raw = body["data"]
        .as_str()
        .ok_or_else(|| refusal(422, "Select a portable matter file"))?;
    if raw.len() > 4 * IPC_LIMIT.div_ceil(3) {
        return Err(refusal(
            413,
            "Portable package exceeds the Rust restore limit",
        ));
    }
    let bytes = STANDARD.decode(raw).map_err(|_| refusal(422, INVALID))?;
    Ok(bytes)
}

const RESTORE_PREFIX: &str = ".portable-restore-";

struct RestoreStaging {
    path: PathBuf,
    key: DataKey,
}

impl Drop for RestoreStaging {
    fn drop(&mut self) {
        let _ = remove_restore_stage(&self.path, &self.key);
    }
}

fn remove_restore_stage(path: &Path, key: &DataKey) -> Result<(), String> {
    let entries = fs::read_dir(path)
        .map_err(io)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io)?;
    for entry in &entries {
        let name = entry.file_name().into_string().map_err(|_| INVALID)?;
        let kind = entry.file_type().map_err(io)?;
        if let Some(asset) = name.strip_prefix("asset-") {
            if !kind.is_file() || !artifacts::valid_id(asset) {
                return Err("Unexpected portable restore staging asset".into());
            }
        } else if !kind.is_dir()
            || !artifacts::valid_id(&name)
            || !safe_protected_orphan(
                &entry.path(),
                &publication_receipt(key, &name)?,
                true,
                key,
                &name,
            )?
        {
            return Err("Unexpected portable restore staging content".into());
        }
    }
    for entry in entries {
        if entry.file_type().map_err(io)?.is_dir() {
            remove_document_directory(&entry.path()).map_err(io)?;
        } else {
            fs::remove_file(entry.path()).map_err(io)?;
        }
    }
    fs::remove_dir(path).map_err(io)
}

pub(crate) fn recover_prepared_restores(root: &Path, key: &DataKey) -> Result<(), String> {
    for entry in fs::read_dir(root).map_err(io)? {
        let entry = entry.map_err(io)?;
        let name = entry.file_name();
        let Some(stamp) = name
            .to_str()
            .and_then(|name| name.strip_prefix(RESTORE_PREFIX))
        else {
            continue;
        };
        let Some((identity, _)) = stamp.split_once('.') else {
            continue;
        };
        if !artifacts::valid_id(identity)
            || stamp != format!("{identity}{}.staging", publication_receipt(key, identity)?)
        {
            continue;
        }
        if !entry.file_type().map_err(io)?.is_dir() {
            return Err("Invalid portable restore staging path".into());
        }
        remove_restore_stage(&entry.path(), key)?;
    }
    Ok(())
}

pub struct PortableRestoreWork {
    root: PathBuf,
    key: DataKey,
}

struct PortableRevision {
    number: u64,
    relpath: String,
    hash: String,
    created: String,
    pages: u64,
    ocr: String,
    provenance: String,
}

struct PortableDocument {
    id: String,
    body: Value,
    hash: String,
    working: String,
    retained: Vec<PortableRevision>,
}

struct PortableAsset {
    path: PathBuf,
    hash: String,
    mime: String,
}

pub struct PreparedPortableRestore {
    staging: RestoreStaging,
    matter_id: String,
    matter: Value,
    manifest: Value,
    documents: Vec<PortableDocument>,
    remap: HashMap<String, String>,
    assets: HashMap<String, PortableAsset>,
}

impl PortableRestoreWork {
    pub fn compute_selected(
        self,
        source: &mut File,
        size: u64,
        hash: &str,
        credential: Option<&str>,
        recovery: bool,
    ) -> Result<PreparedPortableRestore, Reply> {
        if size > MAX_PACKAGE_BYTES {
            return Err(refusal(422, "Portable package exceeds 4 GiB"));
        }
        let verify = |source: &mut File| -> Result<(), Reply> {
            source
                .seek(SeekFrom::Start(0))
                .map_err(|_| refusal(422, "Selected backup cannot be read"))?;
            let (actual_size, actual_hash) = copy_hashed(source, &mut std::io::sink(), size)
                .map_err(|_| refusal(422, "Selected backup changed after selection"))?;
            if actual_size != size || actual_hash != hash {
                return Err(refusal(422, "Selected backup changed after selection"));
            }
            Ok(())
        };
        let version = FileVersion::read(source).ok();
        verify(source)?;
        let prepared = self.compute(&mut *source, credential, recovery)?;
        if let Some(version) = version {
            if FileVersion::read(source).ok().as_ref() != Some(&version) {
                return Err(refusal(422, "Selected backup changed after selection"));
            }
        } else {
            verify(source)?;
        }
        Ok(prepared)
    }

    pub fn compute(
        self,
        source: impl Read + Seek,
        credential: Option<&str>,
        recovery: bool,
    ) -> Result<PreparedPortableRestore, Reply> {
        self.stage(source, credential, recovery)
            .map_err(|error| refusal(422, error))
    }

    pub fn compute_upload(self, body: Value) -> Result<PreparedPortableRestore, Reply> {
        let bytes = restore_upload_bytes(&body)?;
        self.compute(
            Cursor::new(bytes),
            body["credential"].as_str(),
            body["recovery"] == true,
        )
    }

    fn stage(
        self,
        mut source: impl Read + Seek,
        credential: Option<&str>,
        recovery: bool,
    ) -> Result<PreparedPortableRestore, String> {
        if source.seek(SeekFrom::End(0)).map_err(io)? > MAX_PACKAGE_BYTES {
            return Err("Portable package exceeds 4 GiB".into());
        }
        source.seek(SeekFrom::Start(0)).map_err(io)?;
        let key = &self.key;
        let mut signature = [0; 7];
        source.read_exact(&mut signature).map_err(|_| INVALID)?;
        source.seek(SeekFrom::Start(0)).map_err(io)?;
        let zip: Box<dyn ReadSeek + '_> = if &signature == b"CBMAT4\n" {
            let (envelope, offset) =
                vault::protected_package_header(&mut source).map_err(|error| error.to_string())?;
            let package_key = if vault::check_encrypted_start(&mut source, offset, key).is_ok() {
                key.clone()
            } else {
                let credential =
                    credential.ok_or("This backup needs its password or recovery key")?;
                let access = if recovery {
                    vault::Credential::Recovery(credential)
                } else {
                    vault::Credential::Password(credential)
                };
                vault::unlock_key(&envelope, access).map_err(|error| error.to_string())?
            };
            Box::new(
                vault::EncryptedReader::at(&mut source, offset, &package_key)
                    .map_err(|error| error.to_string())?,
            )
        } else {
            Box::new(&mut source)
        };
        let (manifest, mut files) = parse(zip)?;
        validate_portable_references(&manifest)?;
        let incoming = &manifest["matter"];
        let mut matter =
            validate(incoming.clone(), true).map_err(|_| "Invalid matter fields in package")?;
        preparation::validate_portable_review(&manifest["matter_values"], &matter)?;
        let matter_id = id();
        matter["id"] = json!(matter_id);
        matter["created_at"] = json!(field(incoming, "created_at")?);
        matter["archived_at"] = Value::Null;
        let path = self.root.join(format!(
            "{RESTORE_PREFIX}{matter_id}{}.staging",
            publication_receipt(key, &matter_id)?
        ));
        fs::create_dir(&path).map_err(io)?;
        let staging = RestoreStaging {
            path,
            key: key.clone(),
        };
        let mut remap = HashMap::new();
        let mut documents = Vec::new();
        let mut referenced = HashSet::new();
        let mut group_parents = HashSet::new();
        for old in list(&manifest, "documents")? {
            let previous = field(old, "id")?;
            let new = id();
            if !artifacts::valid_id(previous)
                || remap.insert(previous.to_owned(), new.clone()).is_some()
            {
                return Err("Invalid document identity in package".into());
            }
            let metadata = bundle_inputs::select(
                old,
                &[
                    "title",
                    "notes",
                    "kind",
                    "part",
                    "label",
                    "label_status",
                    "label_source",
                    "group_key",
                    "group_role",
                    "document_date",
                    "date_status",
                    "date_source",
                    "date_note",
                    "visible_signature_status",
                ],
            );
            document_metadata::parse_patch(&metadata)
                .map_err(|_| "Invalid document fields in package")?;
            if !matches!(
                old["certificate_signature_status"].as_str(),
                Some("detected" | "not_detected" | "unknown")
            ) || old["certificate_signature_count"].as_u64().is_none()
                || field(old, "signature_inspection_note")?.chars().count() > 1000
                || old["signature_status"]
                    != document_metadata::signature_status(old, &old["visible_signature_status"])
            {
                return Err("Invalid signature inspection metadata in package".into());
            }
            if old["group_role"] == "parent" {
                let key = field(old, "group_key")?.trim();
                if key.is_empty() || (old["archived_at"].is_null() && !group_parents.insert(key)) {
                    return Err("Invalid group parent in package".into());
                }
            }
            validate_provenance(field(old, "ocr_provenance_json")?)?;
            let archive = old.get("archived_at").unwrap_or(&Value::Null);
            if !archive.is_null() && archive.as_str().is_none_or(|date| date.len() > 64) {
                return Err("Invalid document archive state".into());
            }
            if number(old, "position")? > 100_000 || field(old, "source_file_mtime")?.len() > 40 {
                return Err("Invalid document order or source timestamp".into());
            }
            let part = number(old, "part")?;
            if !matter["parts"]
                .as_array()
                .is_some_and(|parts| parts.contains(&json!(part)))
            {
                return Err("Document Part is missing from the restored matter".into());
            }
            if old["bundle_included"] != 0 && old["bundle_included"] != 1 {
                return Err("Invalid bundle membership in package".into());
            }
            let source = field(old, "source_relpath")?;
            let hash = files.digest(source)?.to_owned();
            referenced.insert(source.to_owned());
            if old["source_sha256"] != hash {
                return Err("Imported original failed its hash check".into());
            }
            let receipt = publication_receipt(key, &new)?;
            let stage = staging.path.join(&new);
            fs::create_dir(&stage).map_err(io)?;
            File::create(stage.join(&receipt))
                .and_then(|file| file.sync_all())
                .map_err(io)?;
            write_file(&stage.join("original.pdf"), key, &mut files, source, true)?;
            let current = number(old, "revision")?;
            let mut working = None;
            let mut versions = HashSet::new();
            let mut retained = Vec::new();
            for version in list(&manifest, "revisions")?
                .iter()
                .filter(|item| item["document_id"] == previous)
            {
                validate_provenance(field(version, "ocr_provenance_json")?)?;
                let revision = number(version, "revision")?;
                if revision == 0 || revision > current || !versions.insert(revision) {
                    return Err("Invalid retained revision identity".into());
                }
                let path = field(version, "working_relpath")?;
                let pages = number(version, "page_count")?;
                if pages == 0 {
                    return Err("Invalid retained working revision page count".into());
                }
                referenced.insert(path.to_owned());
                let name = revisions::file_name(key, &new, revision)?;
                let hash = write_file(&stage.join(&name), key, &mut files, path, false)?;
                let rel = format!("documents-v1/{new}/{name}");
                retained.push(PortableRevision {
                    number: revision,
                    relpath: rel.clone(),
                    hash,
                    created: field(version, "created_at")?.to_owned(),
                    pages,
                    ocr: field(
                        if revision == current { old } else { version },
                        "ocr_status",
                    )?
                    .to_owned(),
                    provenance: field(
                        if revision == current { old } else { version },
                        "ocr_provenance_json",
                    )?
                    .to_owned(),
                });
                if revision == current {
                    if old["working_relpath"] != path || old["page_count"] != pages {
                        return Err("Current working revision differs from the manifest".into());
                    }
                    working = Some(rel);
                }
            }
            // Older stores backfilled only the working revision that existed
            // when revision retention was introduced.
            let working = working.ok_or("Current working revision is unavailable")?;
            let mut document = old.clone();
            document["id"] = json!(new);
            document["matter_id"] = json!(matter_id);
            documents.push(PortableDocument {
                id: new,
                body: document,
                hash,
                working,
                retained,
            });
            sync_directory(&stage)?;
        }
        let mut assets = HashMap::new();
        for item in list(&manifest, "page_annotations")? {
            if item["kind"] != "image" {
                if !item["asset_relpath"].is_null() || !item["asset_mime"].is_null() {
                    return Err("Review annotations cannot contain image assets".into());
                }
                continue;
            }
            let path = field(item, "asset_relpath")?;
            if assets.contains_key(path) {
                // Every reference still has to agree on its declared media type.
                let stored: &PortableAsset = &assets[path];
                if stored.mime != field(item, "asset_mime")? {
                    return Err("Image overlay asset type differs between annotations".into());
                }
                continue;
            }
            let data = files.read_asset(path)?;
            let mime = field(item, "asset_mime")?;
            let (format, _) = asset_format(mime)?;
            super::document_output::decode_image(&data, format).map_err(str::to_owned)?;
            let staged = staging.path.join(format!("asset-{}", id()));
            let file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&staged)
                .map_err(io)?;
            let mut encrypted =
                vault::EncryptedWriter::new(file, key).map_err(|error| error.to_string())?;
            encrypted.write_all(&data).map_err(io)?;
            encrypted
                .finish()
                .map_err(|error| error.to_string())?
                .sync_all()
                .map_err(io)?;
            referenced.insert(path.to_owned());
            assets.insert(
                path.to_owned(),
                PortableAsset {
                    path: staged,
                    hash: sha256_hex(&data),
                    mime: mime.to_owned(),
                },
            );
        }
        if referenced.len() != files.entries.len() {
            return Err("Package contains unreferenced artifacts".into());
        }
        sync_directory(&staging.path)?;
        sync_directory(&self.root)?;
        crash_at("restore_staged");
        Ok(PreparedPortableRestore {
            staging,
            matter_id,
            matter,
            manifest,
            documents,
            remap,
            assets,
        })
    }
}

fn asset_format(mime: &str) -> Result<(image::ImageFormat, &'static str), String> {
    match mime {
        "image/png" => Ok((image::ImageFormat::Png, "png")),
        "image/jpeg" => Ok((image::ImageFormat::Jpeg, "jpg")),
        _ => Err("Unsupported overlay image type".into()),
    }
}

fn validate_portable_references(manifest: &Value) -> Result<(), String> {
    let docs = list(manifest, "documents")?;
    let identities = docs
        .iter()
        .map(|doc| field(doc, "id"))
        .collect::<Result<HashSet<_>, _>>()?;
    if identities.len() != docs.len() {
        return Err("Invalid document revision references".into());
    }
    let mut revisions = HashMap::new();
    for row in list(manifest, "revisions")? {
        let doc = field(row, "document_id")?;
        let revision = number(row, "revision")?;
        let pages = number(row, "page_count")?;
        if !identities.contains(doc)
            || revision == 0
            || pages == 0
            || revisions.insert((doc, revision), pages).is_some()
        {
            return Err("Invalid document revision references".into());
        }
    }
    let page_count = |doc, revision| revisions.get(&(doc, revision)).copied().unwrap_or(0);

    let mut redaction_ids = HashSet::new();
    for record in list(manifest, "redactions")? {
        let doc = field(record, "document_id")?;
        if !redaction_ids.insert(field(record, "id")?) || !identities.contains(doc) {
            return Err("Invalid redaction record identity".into());
        }
        let before = number(record, "before_revision")?;
        let after = number(record, "after_revision")?;
        if after <= before
            || page_count(doc, before) == 0
            || number(record, "page_index")? >= page_count(doc, after)
            || !record["active"].as_u64().is_some_and(|flag| flag <= 1)
            || !record["structure_changed"]
                .as_u64()
                .is_some_and(|flag| flag <= 1)
            || (!record["removed_revision"].is_null()
                && !record["removed_revision"]
                    .as_u64()
                    .is_some_and(|version| page_count(doc, version) > 0))
        {
            return Err("Invalid redaction revision record".into());
        }
        field(record, "created_at")?;
        for (fields, limit) in [
            (["rect_x", "rect_y", "rect_width", "rect_height"], 1.0),
            (
                ["restore_x", "restore_y", "restore_width", "restore_height"],
                1.000001,
            ),
        ] {
            let [x, y, w, h] = fields.map(|key| record[key].as_f64().unwrap_or(f64::NAN));
            if ![x, y, w, h].iter().all(|value| value.is_finite())
                || x < 0.0
                || y < 0.0
                || w <= 0.0
                || h <= 0.0
                || x + w > limit
                || y + h > limit
            {
                return Err("Invalid redaction area".into());
            }
        }
    }
    let reviews = list(manifest, "ocr_page_reviews")?;
    if reviews.len() > 100_000 {
        return Err("Invalid OCR page reviews".into());
    }
    let mut reviewed = HashSet::new();
    for item in reviews {
        let doc = field(item, "document_id")?;
        let revision = number(item, "revision")?;
        let page = number(item, "page_index")?;
        if page >= page_count(doc, revision)
            || !reviewed.insert((doc, revision, page))
            || field(item, "accepted_at")?.chars().count() > 40
        {
            return Err("Invalid OCR page review reference".into());
        }
    }
    let bookmarks = list(manifest, "manual_bookmarks")?;
    let links = list(manifest, "reviewed_links")?;
    bundle_inputs::validate_portable_destinations(bookmarks, links)?;
    let mut parents = HashSet::new();
    for (position, item) in bookmarks.iter().enumerate() {
        if number(item, "position")? != position as u64
            || number(item, "page_index")?
                >= page_count(
                    field(item, "document_id")?,
                    number(item, "target_revision")?,
                )
            || (!item["parent_id"].is_null() && !parents.contains(field(item, "parent_id")?))
            || !parents.insert(field(item, "id")?)
        {
            return Err("Invalid manual bookmark target or hierarchy".into());
        }
    }
    let mut link_ids = HashSet::new();
    for (position, item) in links.iter().enumerate() {
        if number(item, "position")? != position as u64
            || !link_ids.insert(field(item, "id")?)
            || number(item, "source_page_index")?
                >= page_count(
                    field(item, "source_document_id")?,
                    number(item, "source_revision")?,
                )
            || (item["kind"] == "internal"
                && number(item, "target_page_index")?
                    >= page_count(
                        field(item, "target_document_id")?,
                        number(item, "target_revision")?,
                    ))
        {
            return Err("Invalid reviewed link source or target".into());
        }
    }
    bundle_inputs::validate_portable_preset(&manifest["output_preset"])?;
    let marks = bundle_inputs::parse_marks(&manifest["output_marks"])
        .map_err(|_| "Invalid output marks in backup")?;
    if let Some(splits) = marks["volume_splits"].as_object() {
        for (document, split) in splits {
            let pages = page_count(document.as_str(), number(split, "revision")?);
            if pages == 0
                || list(split, "after_pages")?
                    .iter()
                    .any(|page| page.as_u64().is_none_or(|page| page >= pages))
            {
                return Err("Invalid volume split document or boundary".into());
            }
        }
    }
    let annotations = list(manifest, "page_annotations")?;
    if annotations.len() > 2000 {
        return Err("Invalid page annotations".into());
    }
    let mut annotation_ids = HashSet::new();
    for (position, item) in annotations.iter().enumerate() {
        let id = field(item, "id")?;
        if id.chars().count() != 32
            || !annotation_ids.insert(id)
            || number(item, "position")? != position as u64
            || number(item, "page_index")?
                >= page_count(
                    field(item, "document_id")?,
                    number(item, "target_revision")?,
                )
            || ["created_at", "updated_at"].iter().any(|key| {
                !item[*key].is_null()
                    && item[*key]
                        .as_str()
                        .is_none_or(|stamp| stamp.chars().count() > 64)
            })
        {
            return Err("Invalid page annotation target or content".into());
        }
    }
    Ok(())
}

fn validate_provenance(value: &str) -> Result<(), String> {
    const INVALID_OCR: &str = "Invalid OCR provenance in package";
    let items: Vec<Value> = serde_json::from_str(value).map_err(|_| INVALID_OCR)?;
    if items.len() > 1000 {
        return Err(INVALID_OCR.into());
    }
    for item in items {
        let language = item["language"]
            .as_str()
            .filter(|language| matches!(*language, "eng" | "hin" | "eng+hin"))
            .ok_or(INVALID_OCR)?;
        let provider_hashes = match item["provider"].as_str() {
            Some("PyMuPDF+Tesseract")
                if item["pymupdf_version"].is_string()
                    && (item["tesseract_cli_version"].is_string()
                        || item["tesseract_cli_version"].is_null()) =>
            {
                &item["model_sha256"]
            }
            Some("Tesseract") if item["version"].is_string() && item["text_verified"] == false => {
                &item["traineddata_sha256"]
            }
            _ => return Err(INVALID_OCR.into()),
        };
        let hashes = provider_hashes.as_object().ok_or(INVALID_OCR)?;
        if hashes.len() != language.split('+').count()
            || !language.split('+').all(|model| {
                hashes
                    .get(model)
                    .and_then(Value::as_str)
                    .is_some_and(artifacts::valid_hash)
            })
            || item["applied_revision"]
                .as_u64()
                .is_none_or(|revision| revision == 0)
            || item["pages_ocr_applied"]
                .as_u64()
                .is_none_or(|count| count == 0)
            || !item["applied_at"].is_string()
            || (!item["base_revision"].is_null()
                && item["base_revision"]
                    .as_u64()
                    .is_none_or(|revision| revision == 0))
            || (!item["rerun"].is_null() && !item["rerun"].is_boolean())
        {
            return Err(INVALID_OCR.into());
        }
    }
    Ok(())
}

fn write_file(
    path: &Path,
    key: &DataKey,
    package: &mut PackageReader<'_>,
    member: &str,
    read_only: bool,
) -> Result<String, String> {
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(io)?;
    let mut encrypted =
        vault::EncryptedWriter::new(file, key).map_err(|error| error.to_string())?;
    let hash = package.copy(member, &mut encrypted, MAX_FILE_BYTES)?;
    let file = encrypted.finish().map_err(|error| error.to_string())?;
    if read_only {
        let mut permissions = file.metadata().map_err(io)?.permissions();
        permissions.set_readonly(true);
        file.set_permissions(permissions).map_err(io)?;
    }
    file.sync_all().map_err(io)?;
    Ok(hash)
}

trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

struct PackageReader<'a> {
    archive: ZipArchive<Box<dyn ReadSeek + 'a>>,
    entries: BTreeMap<String, (u64, String)>,
}

impl PackageReader<'_> {
    fn digest(&self, path: &str) -> Result<&str, String> {
        self.entries
            .get(path)
            .map(|(_, hash)| hash.as_str())
            .ok_or_else(|| "Portable artifact is missing".into())
    }

    fn copy(&mut self, path: &str, output: &mut impl Write, limit: u64) -> Result<String, String> {
        let (count, digest) = self
            .entries
            .get(path)
            .ok_or("Portable artifact is missing")?;
        if *count > limit {
            return Err("Portable artifact exceeds its size limit".into());
        }
        let member = self
            .archive
            .by_name(&format!("files/{path}"))
            .map_err(|_| INVALID)?;
        let (written, actual) = copy_hashed(member, output, *count)?;
        if written != *count || actual != *digest {
            return Err("Portable artifact failed its hash check".into());
        }
        Ok(actual)
    }

    fn read_asset(&mut self, path: &str) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        self.copy(path, &mut bytes, ASSET_LIMIT)?;
        Ok(bytes)
    }
}

fn parse(raw: Box<dyn ReadSeek + '_>) -> Result<(Value, PackageReader<'_>), String> {
    let mut archive = ZipArchive::new(raw).map_err(|_| INVALID)?;
    if archive.len() > 20_000 {
        return Err("Portable package contains too many entries".into());
    }
    let mut names = HashSet::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|_| INVALID)?;
        if entry.is_dir()
            || entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
            || !names.insert(entry.name().map_err(|_| INVALID)?.into_owned())
        {
            return Err(INVALID.into());
        }
    }
    let mut manifest = Vec::new();
    archive
        .by_name("manifest.json")
        .map_err(|_| INVALID)?
        .take(MANIFEST_LIMIT as u64 + 1)
        .read_to_end(&mut manifest)
        .map_err(io)?;
    if manifest.len() > MANIFEST_LIMIT {
        return Err("Portable manifest is too large".into());
    }
    let mut manifest: Value = serde_json::from_slice(&manifest).map_err(|_| INVALID)?;
    if manifest["format"] != "courtbundle-matter" {
        return Err("Unsupported portable matter package".into());
    }
    normalize_manifest(&mut manifest)?;
    let mut files = BTreeMap::new();
    let mut total = 0u64;
    for item in list(&manifest, "files")? {
        let path = field(item, "path")?;
        if !safe_name(path) || files.contains_key(path) {
            return Err("Invalid portable artifact path".into());
        }
        let count = number(item, "bytes")?;
        total = total.saturating_add(count);
        if total > MAX_PACKAGE_BYTES || count > MAX_FILE_BYTES {
            return Err("Invalid package member size or type".into());
        }
        let member = archive
            .by_name(&format!("files/{path}"))
            .map_err(|_| INVALID)?;
        if member.size() != count {
            return Err("Portable artifact size differs from the manifest".into());
        }
        files.insert(path.to_owned(), (count, field(item, "sha256")?.to_owned()));
    }
    if names.len() != files.len() + 1 {
        return Err("Unexpected package entries".into());
    }
    Ok((
        manifest,
        PackageReader {
            archive,
            entries: files,
        },
    ))
}

fn normalize_manifest(manifest: &mut Value) -> Result<(), String> {
    let version = number(manifest, "version")?;
    if !(1..=VERSION).contains(&version) {
        return Err("Unsupported portable matter package".into());
    }
    let fields = manifest.as_object_mut().ok_or(INVALID)?;
    for (key, introduced, default) in [
        ("manual_bookmarks", 2, json!([])),
        ("matter_values", 3, json!({})),
        ("reviewed_links", 9, json!([])),
        ("output_marks", 9, bundle_inputs::marks()),
        ("page_annotations", 10, json!([])),
        ("output_preset", 12, Value::Null),
        ("redactions", 13, json!([])),
        ("membership_revision", 15, json!(1)),
        ("ocr_page_reviews", 18, json!([])),
    ] {
        if !fields.contains_key(key) {
            if version >= introduced && key != "membership_revision" && key != "redactions" {
                return Err(format!("Portable matter manifest is missing {key}"));
            }
            fields.insert(key.to_owned(), default);
        }
    }
    if version >= 19
        && (manifest["matter"].get("parts").is_none()
            || manifest["matter"].get("next_part").is_none())
    {
        return Err("Missing matter Parts in package".into());
    }
    let documents = manifest["documents"].as_array_mut().ok_or(INVALID)?;
    for doc in documents {
        let label = doc["label"].as_str().is_some_and(|label| !label.is_empty());
        let doc = doc.as_object_mut().ok_or(INVALID)?;
        for (key, introduced, default) in [
            ("document_date", 4, json!("")),
            ("date_status", 4, json!("unknown")),
            ("date_source", 4, json!("none")),
            ("source_file_mtime", 4, json!("")),
            ("date_note", 5, json!("")),
            ("label_status", 6, json!("unreviewed")),
            (
                "label_source",
                6,
                json!(if label { "legacy" } else { "none" }),
            ),
            ("group_role", 7, json!("member")),
            ("certificate_signature_status", 8, json!("unknown")),
            ("certificate_signature_count", 8, json!(0)),
            (
                "signature_inspection_note",
                8,
                json!("Legacy package was not inspected for PDF certificate-signature indicators."),
            ),
            ("visible_signature_status", 8, json!("unreviewed")),
            ("signature_status", 8, json!("inspection_inconclusive")),
            ("ocr_provenance_json", 11, json!("[]")),
            ("bundle_included", 15, json!(1)),
            ("notes", 17, json!("")),
        ] {
            if version < introduced {
                doc.entry(key.to_owned()).or_insert(default);
            }
        }
        if version < 15 {
            doc.insert("bundle_included".into(), json!(1));
        }
        doc.entry("archived_at".to_owned()).or_insert(Value::Null);
    }
    let revisions = manifest["revisions"].as_array_mut().ok_or(INVALID)?;
    if version < 11 {
        for revision in revisions {
            revision
                .as_object_mut()
                .ok_or(INVALID)?
                .entry("ocr_provenance_json".to_owned())
                .or_insert(json!("[]"));
        }
    }
    let annotations = manifest["page_annotations"].as_array_mut().ok_or(INVALID)?;
    for item in annotations {
        let kind = item["kind"].as_str().unwrap_or_default();
        let default_scope = if kind == "image" { "pdf" } else { "private" };
        let fields = item.as_object_mut().ok_or(INVALID)?;
        if version < 14 {
            fields.insert("output_scope".to_owned(), json!(default_scope));
        }
        if version < 16 {
            for key in ["created_at", "updated_at"] {
                fields.insert(key.to_owned(), Value::Null);
            }
        }
        fields
            .entry("text_style_json".to_owned())
            .or_insert(json!("{}"));
    }
    Ok(())
}

fn remap_field(
    entry: &mut Value,
    field_: &str,
    map: &HashMap<String, String>,
) -> Result<(), String> {
    if entry[field_].is_null() {
        return Ok(());
    }
    let old = field(entry, field_)?;
    entry[field_] = json!(map
        .get(old)
        .ok_or("Package references an unknown source identity")?);
    Ok(())
}

fn restore_metadata(
    con: &Connection,
    matter: &str,
    manifest: &Value,
    assets: &HashMap<String, PortableAsset>,
    key: &DataKey,
    documents: &HashMap<String, String>,
) -> Result<(), String> {
    preparation::restore_backup(
        con,
        matter,
        &manifest["memo_parties"],
        &manifest["matter_values"],
    )?;
    let membership = number(manifest, "membership_revision")?;
    if membership == 0 {
        return Err(INVALID.into());
    }
    con.execute(
        "INSERT INTO bundle_membership_state VALUES (?,?)",
        params![matter, membership],
    )
    .map_err(sql)?;
    let mut bookmarks = list(manifest, "manual_bookmarks")?.to_vec();
    let bookmark_ids = bookmarks
        .iter()
        .map(|bookmark| Ok((field(bookmark, "id")?.to_owned(), id())))
        .collect::<Result<HashMap<_, _>, String>>()?;
    if bookmark_ids.len() != bookmarks.len() {
        return Err("Duplicate manual bookmark identities".into());
    }
    for bookmark in &mut bookmarks {
        remap_field(bookmark, "id", &bookmark_ids)?;
        remap_field(bookmark, "parent_id", &bookmark_ids)?;
        remap_field(bookmark, "document_id", documents)?;
    }
    let mut links = list(manifest, "reviewed_links")?.to_vec();
    for link in &mut links {
        link["id"] = json!(id());
        remap_field(link, "source_document_id", documents)?;
        remap_field(link, "target_document_id", documents)?;
    }
    let mut marks = manifest["output_marks"].clone();
    if let Some(fields) = marks.as_object_mut() {
        for key in ["volume_splits", "annexure_overrides"] {
            if let Some(items) = fields.get_mut(key).and_then(Value::as_object_mut) {
                let mut new = serde_json::Map::new();
                for (id, item) in std::mem::take(items) {
                    new.insert(
                        documents
                            .get(&id)
                            .ok_or("Output settings reference a missing document")?
                            .clone(),
                        item,
                    );
                }
                *items = new;
            }
        }
    }
    bundle_inputs::restore_backup(
        con,
        matter,
        &marks,
        &manifest["output_preset"],
        &bookmarks,
        &links,
    )?;
    let mut annotations = list(manifest, "page_annotations")?.to_vec();
    for item in &mut annotations {
        let identifier = id();
        item["id"] = json!(identifier);
        remap_field(item, "document_id", documents)?;
        if item["kind"] == "image" {
            let path = field(item, "asset_relpath")?;
            let asset = assets
                .get(path)
                .ok_or("Image overlay asset is unavailable")?;
            let (_, extension) = asset_format(&asset.mime)?;
            let file = open_document_file(&asset.path)?;
            let mut source =
                vault::EncryptedReader::at(file, 0, key).map_err(|error| error.to_string())?;
            let mut data = Vec::new();
            let (_, hash) = copy_hashed(&mut source, &mut data, ASSET_LIMIT)?;
            if hash != asset.hash {
                return Err("Image overlay asset failed its integrity check".into());
            }
            item["asset_relpath"] = json!(format!("assets/{identifier}.{extension}"));
            con.execute(
                "INSERT INTO rust_annotation_assets VALUES (?,?)",
                params![identifier, data],
            )
            .map_err(sql)?;
        }
    }
    super::annotations::restore_backup(con, matter, &annotations)?;
    for stored in list(manifest, "redactions")? {
        let mut record = stored.clone();
        record["id"] = json!(id());
        remap_field(&mut record, "document_id", documents)?;
        let document = field(&record, "document_id")?;
        let previous = number(&record, "before_revision")?;
        let resulting = number(&record, "after_revision")?;
        if previous >= resulting {
            return Err("Invalid redaction revision boundaries".into());
        }
        let revision = revisions::record(con, document, resulting)
            .map_err(sql)?
            .ok_or("Redaction revision is unavailable")?;
        if number(&record, "page_index")? >= revision.pages
            || revisions::record(con, document, previous)
                .map_err(sql)?
                .is_none()
        {
            return Err("Redaction source revision or page is unavailable".into());
        }
        super::redaction_records::restore_record(con, document, &record).map_err(sql)?;
    }
    for review in list(manifest, "ocr_page_reviews")? {
        let document = documents
            .get(field(review, "document_id")?)
            .ok_or("OCR review references an unknown document")?;
        let revision = number(review, "revision")?;
        let page = number(review, "page_index")?;
        let row = revisions::record(con, document, revision)
            .map_err(sql)?
            .ok_or("OCR review revision is unavailable")?;
        if page >= row.pages {
            return Err("OCR review page is outside its revision".into());
        }
        con.execute(
            "INSERT INTO rust_ocr_reviews VALUES (?,?,?,?)",
            params![document, revision, page, field(review, "accepted_at")?],
        )
        .map_err(sql)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_store::VaultStore;

    const PASSWORD: &str = "Synthetic portable recovery fixture";
    const ORIGINAL: &[u8] = include_bytes!("../../../samples/rust_pdf_corpus/unsigned.pdf");

    fn legacy_manifest(store: &mut VaultStore) -> Value {
        let matter = store.request(
            "POST",
            "/matters",
            json!({"name":"Historical revision fixture"}),
        );
        assert_eq!(matter.status, 201, "{matter:?}");
        let imported = store.import_pdf(
            matter.body["id"].as_str().unwrap(),
            "historical.pdf",
            ORIGINAL,
            ImportOptions::default(),
        );
        assert_eq!(imported.status, 201, "{imported:?}");
        let mut doc = imported.body[0].clone();
        doc["source_relpath"] = json!("originals/historical.pdf");
        doc["working_relpath"] = json!("working/historical.pdf");
        doc["revision"] = json!(3);
        json!({
            "format":"courtbundle-matter","version":19,"matter":matter.body,
            "documents":[doc],
            "revisions":[{"document_id":doc["id"],"revision":3,
                "working_relpath":doc["working_relpath"],"page_count":doc["page_count"],
                "ocr_status":doc["ocr_status"],"ocr_provenance_json":"[]",
                "created_at":doc["created_at"]}],
            "files":[
                {"path":"originals/historical.pdf","sha256":sha256_hex(ORIGINAL),"bytes":ORIGINAL.len()},
                {"path":"working/historical.pdf","sha256":sha256_hex(ORIGINAL),"bytes":ORIGINAL.len()}
            ],
            "memo_parties":[],"matter_values":{},"manual_bookmarks":[],"reviewed_links":[],
            "output_marks":bundle_inputs::marks(),"output_preset":null,"page_annotations":[],
            "ocr_page_reviews":[],"membership_revision":1,"redactions":[]
        })
    }

    fn package(manifest: &Value) -> Cursor<Vec<u8>> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        zip.start_file("manifest.json", options).unwrap();
        zip.write_all(&serde_json::to_vec(manifest).unwrap())
            .unwrap();
        for member in ["originals/historical.pdf", "working/historical.pdf"] {
            zip.start_file(format!("files/{member}"), options).unwrap();
            zip.write_all(ORIGINAL).unwrap();
        }
        zip.finish().unwrap()
    }

    #[test]
    fn recover_uncommitted_portable_restore_with_no_first_revision() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("vault");
        let mut store = VaultStore::open(&root).unwrap();
        let setup = store.request("POST", "/vault/setup", json!({"password":PASSWORD}));
        assert_eq!(setup.status, 200);
        let manifest = legacy_manifest(&mut store);
        let prepared = store
            .capture_portable_restore()
            .unwrap()
            .compute(package(&manifest), None, false)
            .ok()
            .unwrap();

        let stage = prepared.staging.path.clone();
        let doc = &prepared.documents[0].id;
        let published = root.join("documents-v1").join(doc);
        let orphan_matter = prepared.matter_id.clone();
        assert!(!stage.join(doc).join("revision-1.pdf").exists());
        // Model the interval after the document rename, before the SQLite commit.
        publish_directory(&stage.join(doc), &published).unwrap();
        drop(prepared);
        assert!(published.is_dir());
        fs::create_dir(&stage).unwrap();
        drop(store);

        let mut reopened = VaultStore::open(&root).unwrap();
        let unlocked = reopened.request("POST", "/vault/unlock", json!({"credential":PASSWORD}));
        assert_eq!(unlocked.status, 200, "{unlocked:?}");
        assert!(!stage.exists());
        assert!(!published.exists());
        assert_eq!(
            reopened
                .request("GET", &format!("/matters/{orphan_matter}"), Value::Null)
                .status,
            404
        );
        let restored = reopened.restore_portable(package(&manifest), None, false);
        assert_eq!(restored.status, 201, "{restored:?}");
        let docs = reopened.request(
            "GET",
            &format!(
                "/matters/{}/documents",
                restored.body["id"].as_str().unwrap()
            ),
            Value::Null,
        );
        assert_eq!(docs.body[0]["revision"], 3);
        let mut source = reopened
            .open_pdf(docs.body[0]["id"].as_str().unwrap(), &Artifact::Original)
            .ok()
            .unwrap();
        assert_eq!(source.read_range(0, source.bytes).unwrap(), ORIGINAL);
    }
}
