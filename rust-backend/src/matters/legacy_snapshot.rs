//! One-time conversion of a detached Python vault into a separate Rust vault.
use super::*;
use crate::{secure_db, vault_store::VaultStore};
use rusqlite::{blob::ZeroBlob, types::ValueRef, DatabaseName, Params};
use serde_json::Map;
use std::{
    collections::HashMap,
    io::{Seek, SeekFrom},
    path::Component,
};

mod frozen_jobs;

const DATABASE: &str = "court_bundle.sqlite3";
const JSON_LIMIT: u64 = 32 * 1024 * 1024;
const ASSET_LIMIT: u64 = 10 * 1024 * 1024;

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str, String> {
    value[name]
        .as_str()
        .ok_or_else(|| format!("Legacy record is missing {name}"))
}

fn identifier(value: &Value, name: &str) -> Result<String, String> {
    let id = field(value, name)?;
    if !artifacts::valid_id(id) {
        return Err(format!("Invalid legacy {name}"));
    }
    Ok(id.to_owned())
}

fn rows(source: &Connection, sql: &str, parameters: impl Params) -> Result<Vec<Value>, String> {
    let mut statement = source.prepare(sql).map_err(message)?;
    let names = statement
        .column_names()
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    let items = statement
        .query_map(parameters, |row| {
            let mut item = Map::new();
            for (index, name) in names.iter().enumerate() {
                let value = match row.get_ref(index)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(value) => json!(value),
                    ValueRef::Real(value) => json!(value),
                    ValueRef::Text(value) => {
                        json!(std::str::from_utf8(value).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                index,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?)
                    }
                    ValueRef::Blob(_) => return Err(rusqlite::Error::InvalidQuery),
                };
                item.insert(name.clone(), value);
            }
            Ok(Value::Object(item))
        })
        .map_err(message)?;
    items.collect::<rusqlite::Result<_>>().map_err(message)
}

fn decode_field(value: &Value, name: &str) -> Result<Value, String> {
    serde_json::from_str(field(value, name)?).map_err(|_| format!("Invalid JSON in legacy {name}"))
}

fn checked_path(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    let mut path = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err("Invalid path in legacy snapshot".into());
        };
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() || metadata.is_file() => {}
            Ok(_) => return Err("Legacy snapshot contains an unsafe resource path".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(message(error)),
        }
    }
    Ok(path)
}

fn encrypted(
    root: &Path,
    relative: &Path,
    key: &DataKey,
) -> Result<vault::EncryptedReader<File>, String> {
    let path = checked_path(root, relative)?;
    let file = open_document_file(&path)?;
    vault::EncryptedReader::at(file, 0, key).map_err(message)
}

fn read_private(
    root: &Path,
    relative: &Path,
    key: &DataKey,
    limit: u64,
) -> Result<Vec<u8>, String> {
    let reader = encrypted(root, relative, key)?;
    if reader.len() > limit {
        return Err("Legacy vault resource exceeds the migration size limit".into());
    }
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(message)?;
    Ok(bytes)
}

fn private_json(root: &Path, relative: &Path, key: &DataKey) -> Result<Value, String> {
    serde_json::from_slice(&read_private(root, relative, key, JSON_LIMIT)?)
        .map_err(|_| "Invalid JSON in legacy vault".into())
}

fn verify_keyed_file(
    root: &Path,
    relative: &Path,
    target: &Path,
    key: &DataKey,
    expected: Option<&str>,
    read_only: bool,
) -> Result<String, String> {
    let path = checked_path(root, relative)?;
    let mut source = open_document_file(&path)?;
    let mut output = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(message)?;
    // Copy ciphertext and authenticate every record in the staged copy.
    let limit = portable_matter::MAX_FILE_BYTES + 1024 * 1024;
    let bytes = std::io::copy(&mut (&mut source).take(limit + 1), &mut output).map_err(message)?;
    if bytes > limit {
        return Err("Legacy document exceeds the migration size limit".into());
    }
    output.seek(SeekFrom::Start(0)).map_err(message)?;
    let mut hash = Sha256::new();
    let mut length = 0;
    vault::visit_encrypted(&mut output, key, |chunk| {
        length += chunk.len() as u64;
        hash.update(chunk);
    })
    .map_err(message)?;
    let digest = vault::hex_bytes(&hash.finalize()).to_string();
    if length == 0
        || length > portable_matter::MAX_FILE_BYTES
        || expected.is_some_and(|expected| expected != digest)
    {
        return Err("Legacy document failed its integrity check".into());
    }
    #[cfg(unix)]
    if read_only {
        let mut mode = output.metadata().map_err(message)?.permissions();
        mode.set_readonly(true);
        output.set_permissions(mode).map_err(message)?;
    }
    let _ = read_only;
    output.sync_all().map_err(message)?;
    Ok(digest)
}

#[derive(Default)]
struct WorkingPaths(HashMap<(String, String), (String, String)>);

impl WorkingPaths {
    fn get(&self, document: &str, path: &str) -> Result<&(String, String), String> {
        self.0
            .get(&(document.to_owned(), path.to_owned()))
            .ok_or_else(|| "Legacy working path has no retained revision".into())
    }
}

fn documents(
    source: &Connection,
    target: &Connection,
    source_root: &Path,
    target_root: &Path,
    key: &DataKey,
    matter: &str,
    current_only: bool,
) -> Result<WorkingPaths, String> {
    let mut paths = WorkingPaths::default();
    for mut document in rows(
        source,
        "SELECT * FROM documents WHERE matter_id=? ORDER BY position,created_at",
        [matter],
    )? {
        let id = identifier(&document, "id")?;
        let folder = target_root.join("documents-v1").join(&id);
        fs::create_dir(&folder).map_err(message)?;
        let receipt = publication_receipt(key, &id)?;
        File::create(folder.join(receipt))
            .and_then(|file| file.sync_all())
            .map_err(message)?;
        let prefix = format!("documents-v1/{id}");
        let original = format!("{prefix}/original.pdf");
        let source_hash = verify_keyed_file(
            source_root,
            Path::new(field(&document, "source_relpath")?),
            &target_root.join(&original),
            key,
            Some(field(&document, "source_sha256")?),
            true,
        )?;
        let current = document["revision"]
            .as_u64()
            .filter(|revision| *revision > 0)
            .ok_or("Invalid legacy working revision")?;
        let current_path = field(&document, "working_relpath")?.to_owned();
        let mut working = None;
        for mut version in rows(
            source,
            "SELECT * FROM document_revisions WHERE document_id=? ORDER BY revision",
            [&id],
        )? {
            let number = version["revision"]
                .as_u64()
                .filter(|number| (1..=current).contains(number))
                .ok_or("Invalid legacy retained revision")?;
            let old_path = field(&version, "working_relpath")?.to_owned();
            if current_only && number != current && old_path != current_path {
                // Ordinary Python build snapshots store only current PDF bytes.
                // Keep revision metadata for stale annotation validation.
                target
                    .execute(
                        "INSERT INTO document_revisions VALUES (?,?,?,?,?,?,?,?)",
                        params![
                            id,
                            number,
                            old_path,
                            "",
                            field(&version, "created_at")?,
                            version["page_count"].as_u64(),
                            field(&version, "ocr_status")?,
                            field(&version, "ocr_provenance_json")?
                        ],
                    )
                    .map_err(message)?;
                continue;
            }
            let lookup = (id.clone(), old_path.clone());
            let (relative, hash) = if let Some(existing) = paths.0.get(&lookup) {
                existing.clone()
            } else {
                let name = revisions::file_name(key, &id, number)?;
                let hash = verify_keyed_file(
                    source_root,
                    Path::new(&old_path),
                    &folder.join(&name),
                    key,
                    None,
                    false,
                )?;
                let file = (format!("{prefix}/{name}"), hash);
                paths.0.insert(lookup, file.clone());
                file
            };
            if number == current {
                if old_path != current_path || document["page_count"] != version["page_count"] {
                    return Err("Legacy working revision metadata differs from its document".into());
                }
                version["ocr_status"] = document["ocr_status"].clone();
                version["ocr_provenance_json"] = document["ocr_provenance_json"].clone();
                working = Some(relative.clone());
            }
            target
                .execute(
                    "INSERT INTO document_revisions VALUES (?,?,?,?,?,?,?,?)",
                    params![
                        id,
                        number,
                        relative,
                        hash,
                        field(&version, "created_at")?,
                        version["page_count"].as_u64(),
                        field(&version, "ocr_status")?,
                        field(&version, "ocr_provenance_json")?
                    ],
                )
                .map_err(message)?;
        }
        let working = working.ok_or("Legacy current working revision is missing")?;
        let fields = document.as_object_mut().expect("SQLite row object");
        fields.remove("working_relpath");
        fields.remove("source_relpath");
        target
            .execute(
                "INSERT INTO documents VALUES (?,?,?,?,?,?,?,?)",
                params![
                    id,
                    matter,
                    document.to_string(),
                    source_hash,
                    original,
                    working,
                    field(&document, "created_at")?,
                    document["archived_at"].as_str()
                ],
            )
            .map_err(message)?;
        sync_directory(&folder)?;
    }
    Ok(paths)
}

fn optional_payload(
    source: &Connection,
    sql: &str,
    matter: &str,
    default: Value,
) -> Result<Value, String> {
    let payload: Option<String> = source
        .query_row(sql, [matter], |row| row.get(0))
        .optional()
        .map_err(message)?;
    payload
        .map(|payload| serde_json::from_str(&payload).map_err(message))
        .transpose()
        .map(|payload| payload.unwrap_or(default))
}

fn annotation_asset(
    target: &Connection,
    root: &Path,
    key: &DataKey,
    item: &Value,
) -> Result<(), String> {
    if item["kind"] != "image" {
        return Ok(());
    }
    let id = identifier(item, "id")?;
    let bytes = read_private(
        root,
        Path::new(field(item, "asset_relpath")?),
        key,
        ASSET_LIMIT,
    )?;
    let existing: Option<Vec<u8>> = target
        .query_row(
            "SELECT bytes FROM rust_annotation_assets WHERE id=?",
            [&id],
            |row| row.get(0),
        )
        .optional()
        .map_err(message)?;
    match existing {
        Some(existing) if existing != bytes => {
            Err("Legacy overlay asset identity was reused".into())
        }
        Some(_) => Ok(()),
        None => {
            target
                .execute(
                    "INSERT INTO rust_annotation_assets VALUES (?,?)",
                    params![id, bytes],
                )
                .map_err(message)?;
            Ok(())
        }
    }
}

fn history_state(
    target: &Connection,
    root: &Path,
    key: &DataKey,
    paths: &WorkingPaths,
    kind: &str,
    state: &mut Value,
) -> Result<(), String> {
    if kind == "document" && state["fields"]["working_relpath"].is_string() {
        let doc = field(state, "id")?;
        let (relative, _) = paths.get(doc, field(&state["fields"], "working_relpath")?)?;
        // Legacy entries record changed columns. Rust replay requires the
        // complete retained revision identity before restoring any fields.
        let (pages, ocr, provenance) = target
            .query_row(
                "SELECT page_count,ocr_status,ocr_provenance_json FROM document_revisions
             WHERE document_id=? AND relpath=? ORDER BY revision LIMIT 1",
                params![doc, relative],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(message)?;
        state["fields"] = json!({
            "working_relpath":relative,"page_count":pages,"ocr_status":ocr,
            "ocr_provenance_json":provenance,
        });
    }
    if kind == "page_annotations" {
        for annotation in state["entries"]
            .as_array_mut()
            .ok_or("Invalid legacy annotation history")?
        {
            if annotation.get("text_style_json").is_none() {
                annotation["text_style_json"] = json!("{}");
            }
            if annotation.get("output_scope").is_none() {
                annotation["output_scope"] = json!(if annotation["kind"] == "image" {
                    "pdf"
                } else {
                    "private"
                });
            }
            for name in ["created_at", "updated_at"] {
                if annotation.get(name).is_none() {
                    annotation[name] = Value::Null;
                }
            }
            annotation_asset(target, root, key, annotation)?;
        }
    }
    if kind == "matter_parts" {
        state["parts_json"] = json!(preparation::python_json(&decode_field(
            state,
            "parts_json"
        )?));
    }
    let settings = match kind {
        "output_marks" => Some("settings"),
        "output_preset" => Some("output_marks"),
        _ => None,
    };
    if let Some(settings) = settings {
        let mut normalized = bundle_inputs::marks();
        let saved = state[settings]
            .as_object()
            .ok_or("Invalid legacy output history")?;
        normalized
            .as_object_mut()
            .expect("Output marks object")
            .extend(saved.clone());
        state[settings] = normalized;
    }
    Ok(())
}

fn matter_records(
    source: &Connection,
    target: &Connection,
    root: &Path,
    key: &DataKey,
    matter: &str,
    paths: &WorkingPaths,
    current_only: bool,
) -> Result<(), String> {
    let parties = optional_payload(
        source,
        "SELECT payload_json FROM memo_parties WHERE matter_id=?",
        matter,
        json!([]),
    )?;
    let values = optional_payload(
        source,
        "SELECT payload_json FROM matter_values WHERE matter_id=?",
        matter,
        json!({}),
    )?;
    preparation::restore_backup(target, matter, &parties, &values)?;
    let (defaults, _) = bundle_inputs::backup_settings(target, matter).map_err(message)?;
    let marks = optional_payload(
        source,
        "SELECT payload_json FROM output_marks WHERE matter_id=?",
        matter,
        defaults,
    )?;
    let preset = optional_payload(
        source,
        "SELECT snapshot_json FROM matter_output_presets WHERE matter_id=?",
        matter,
        Value::Null,
    )?;
    let bookmarks = rows(source, "SELECT id,title,document_id,page_index,target_revision,parent_id,position FROM manual_bookmarks WHERE matter_id=? ORDER BY position", [matter])?;
    let links = rows(source, "SELECT id,title,source_document_id,source_page_index,source_revision,rect_x,rect_y,rect_width,rect_height,kind,target_document_id,target_page_index,target_revision,uri,position FROM reviewed_links WHERE matter_id=? ORDER BY position", [matter])?;
    bundle_inputs::restore_backup(target, matter, &marks, &preset, &bookmarks, &links)?;
    if let Some(revision) = source
        .query_row(
            "SELECT revision FROM bundle_membership_state WHERE matter_id=?",
            [matter],
            |row| row.get::<_, u64>(0),
        )
        .optional()
        .map_err(message)?
    {
        target
            .execute(
                "INSERT INTO bundle_membership_state VALUES (?,?)",
                params![matter, revision],
            )
            .map_err(message)?;
    }
    let annotations = rows(source, "SELECT id,document_id,page_index,target_revision,kind,rect_x,rect_y,rect_width,rect_height,body,color,asset_relpath,asset_mime,position,output_scope,created_at,updated_at,text_style_json FROM page_annotations WHERE matter_id=? ORDER BY position", [matter])?;
    for annotation in &annotations {
        annotation_asset(target, root, key, annotation)?;
    }
    annotations::restore_backup(target, matter, &annotations)?;
    for row in rows(source, "SELECT id,kind,before_json,after_json,created_at,applied FROM edit_history WHERE matter_id=? ORDER BY id", [matter])? {
        let kind = field(&row, "kind")?;
        let mut before = decode_field(&row, "before_json")?;
        let mut after = decode_field(&row, "after_json")?;
        history_state(target, root, key, paths, kind, &mut before)?;
        history_state(target, root, key, paths, kind, &mut after)?;
        target.execute(
            "INSERT INTO edit_history (id,matter_id,kind,before_json,after_json,created_at,applied) VALUES (?,?,?,?,?,?,?)",
            params![row["id"].as_i64(), matter, kind, before.to_string(), after.to_string(),
                field(&row, "created_at")?, row["applied"].as_i64()],
        ).map_err(message)?;
    }
    for row in rows(
        source,
        "SELECT * FROM reader_positions WHERE matter_id=?",
        [matter],
    )? {
        target
            .execute(
                "INSERT INTO reader_positions VALUES (?,?,?,?)",
                params![
                    field(&row, "resource_key")?,
                    matter,
                    field(&row, "payload_json")?,
                    field(&row, "updated_at")?
                ],
            )
            .map_err(message)?;
    }
    for row in rows(source, "SELECT r.* FROM redaction_records r JOIN documents d ON d.id=r.document_id WHERE d.matter_id=?", [matter])? {
        redaction_records::restore_record(target, field(&row, "document_id")?, &row).map_err(message)?;
    }
    for row in rows(source, "SELECT r.* FROM ocr_page_reviews r JOIN documents d ON d.id=r.document_id WHERE d.matter_id=?", [matter])? {
        target.execute("INSERT INTO rust_ocr_reviews VALUES (?,?,?,?)", params![
            field(&row, "document_id")?, row["revision"].as_u64(), row["page_index"].as_u64(), field(&row, "accepted_at")?,
        ]).map_err(message)?;
    }
    for row in rows(
        source,
        "SELECT * FROM source_page_identities WHERE matter_id=?",
        [matter],
    )? {
        let document = field(&row, "document_id")?;
        let relative = field(&row, "working_relpath")?;
        let (relative, hash) = match paths.get(document, relative) {
            Ok(artifact) => artifact,
            Err(_) if current_only => continue,
            Err(error) => return Err(error),
        };
        if row["working_sha256"] != *hash {
            return Err("Legacy source page failed its working revision integrity check".into());
        }
        target
            .execute(
                "INSERT INTO rust_source_pages VALUES (?,?,?,?,?,?)",
                params![
                    identifier(&row, "id")?,
                    matter,
                    document,
                    relative,
                    hash,
                    row["page_index"].as_u64(),
                ],
            )
            .map_err(message)?;
    }
    for (kind, query) in [
        (
            "variant",
            "SELECT id,payload_json FROM audience_variants WHERE matter_id=?",
        ),
        (
            "supplement",
            "SELECT id,payload_json FROM supplemental_plans WHERE matter_id=?",
        ),
    ] {
        for row in rows(source, query, [matter])? {
            let id = identifier(&row, "id")?;
            let plan = decode_field(&row, "payload_json")?;
            if plan["id"] != id || plan["matter_id"] != matter || plan["kind"] != kind {
                return Err("Invalid legacy composition identity".into());
            }
            target
                .execute(
                    "INSERT INTO rust_compositions VALUES (?,?,?,?)",
                    params![id, matter, kind, plan.to_string()],
                )
                .map_err(message)?;
        }
    }
    Ok(())
}

fn json_filenames(root: &Path, relative: &Path) -> Result<Vec<String>, String> {
    let directory = checked_path(root, relative)?;
    let items = match fs::read_dir(directory) {
        Ok(items) => items,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(message(error)),
    };
    let mut names = Vec::new();
    for item in items {
        let item = item.map_err(message)?;
        let name = item
            .file_name()
            .into_string()
            .map_err(|_| "Invalid legacy resource name")?;
        if name.ends_with(".json") && !name.starts_with('.') {
            names.push(name);
        }
    }
    names.sort_unstable();
    Ok(names)
}

fn validation(
    target: &Connection,
    root: &Path,
    key: &DataKey,
    matter: &str,
    build: Option<&str>,
) -> Result<(), String> {
    let scope = build.unwrap_or("preflight");
    let folder = Path::new("validation").join(scope);
    for name in json_filenames(root, &folder)? {
        let path = folder.join(&name);
        let record = private_json(root, &path, key)?;
        if name.starts_with("run-") {
            let run = identifier(&record, "id")?;
            if name != format!("run-{run}.json") || record["matter_id"] != matter {
                return Err("Legacy validation run identity differs from its location".into());
            }
            if let Some(id) = build {
                if record["build_id"] != id {
                    return Err("Legacy validation run belongs to another build".into());
                }
                target
                    .execute(
                        "INSERT INTO rust_export_checks VALUES (?,?,?)",
                        params![run, id, record.to_string()],
                    )
                    .map_err(message)?;
            } else {
                target
                    .execute(
                        "INSERT INTO preflight_runs VALUES (?,?,?)",
                        params![run, matter, record.to_string()],
                    )
                    .map_err(message)?;
            }
        } else if name == "review.json" {
            let id = build.ok_or("Unexpected legacy validation review")?;
            if record["build_id"] != id {
                return Err("Legacy validation review belongs to another build".into());
            }
            target
                .execute(
                    "INSERT INTO rust_export_reviews VALUES (?,?)",
                    params![id, record.to_string()],
                )
                .map_err(message)?;
        } else if name.starts_with("review-") {
            let digest = field(&record, "input_digest")?;
            if build.is_some()
                || !artifacts::valid_hash(digest)
                || name != format!("review-{digest}.json")
            {
                return Err("Invalid legacy preflight review identity".into());
            }
            target
                .execute(
                    "INSERT INTO preflight_reviews VALUES (?,?,?)",
                    params![matter, digest, record.to_string()],
                )
                .map_err(message)?;
        } else if name.starts_with("analysis-") && build.is_none() {
            let digest = field(&record, "input_digest")?;
            if record["method"] != "page-screening-v1"
                || !artifacts::valid_hash(digest)
                || name != format!("analysis-{digest}.json")
                || !record["pages"].is_array()
            {
                return Err("Invalid legacy page-screening report identity".into());
            }
            target
                .execute(
                    "INSERT INTO rust_page_screenings VALUES (?,?,?)",
                    params![matter, digest, record.to_string()],
                )
                .map_err(message)?;
        } else if let Some(note) = name
            .strip_prefix("note-")
            .and_then(|name| name.strip_suffix(".json"))
        {
            let (run, finding) = note
                .split_once('-')
                .ok_or("Invalid legacy finding note identity")?;
            if !artifacts::valid_id(run)
                || finding.is_empty()
                || finding.len() > 64
                || !finding
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            {
                return Err("Invalid legacy finding note identity".into());
            }
            target
                .execute(
                    "INSERT INTO preflight_notes VALUES (?,?,?)",
                    params![run, finding, record.to_string()],
                )
                .map_err(message)?;
        }
    }
    Ok(())
}

fn reader_indexes(manifest: &Value) -> Value {
    let cover = u64::from(manifest["output_marks"]["front_matter"] == "cover_index");
    let mut indexes = Map::new();
    for row in manifest["page_map_snapshot"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let Some(volume) = row["volume"].as_u64() else {
            continue;
        };
        indexes.insert(format!("volume-{volume:02}.pdf"), json!(cover + 1));
        if volume == 1 {
            let offset = row["combined_start_page"]
                .as_u64()
                .zip(row["start_page"].as_u64())
                .and_then(|(combined, local)| combined.checked_sub(local));
            if let Some(offset) = offset.and_then(|offset| offset.checked_add(cover + 1)) {
                indexes.insert("combined.pdf".into(), json!(offset));
            }
        }
    }
    Value::Object(indexes)
}

fn export_data(
    target: &Connection,
    root: &Path,
    key: &DataKey,
    folder: &Path,
    build: &str,
    member: &Value,
) -> Result<(), String> {
    let name = field(member, "name")?;
    if name.starts_with('.')
        || !Path::new(name)
            .file_name()
            .is_some_and(|component| component == name)
        || name.contains(['\\', '\0'])
    {
        return Err("Invalid legacy export filename".into());
    }
    let mut reader = encrypted(root, &folder.join(name), key)?;
    if member["bytes"] != reader.len()
        || field(member, "sha256")? != vault::hex_bytes(reader.digest()).as_str()
    {
        return Err("Legacy generated output failed its integrity check".into());
    }
    let count =
        i32::try_from(reader.len()).map_err(|_| "Legacy export exceeds the storage limit")?;
    target
        .execute(
            "INSERT INTO rust_export_files VALUES (?,?,?)",
            params![build, name, ZeroBlob(count)],
        )
        .map_err(message)?;
    let mut blob = target
        .blob_open(
            DatabaseName::Main,
            "rust_export_files",
            "content",
            target.last_insert_rowid(),
            false,
        )
        .map_err(message)?;
    if std::io::copy(&mut reader, &mut blob).map_err(message)? != count as u64 {
        return Err("Legacy export ended before its declared length".into());
    }
    Ok(())
}

fn exports(
    source: &Connection,
    target: &Connection,
    root: &Path,
    key: &DataKey,
    matter: &str,
) -> Result<(), String> {
    let export_root = checked_path(root, Path::new("exports"))?;
    let entries = match fs::read_dir(export_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(message(error)),
    };
    let mut builds = Vec::new();
    for entry in entries {
        let entry = entry.map_err(message)?;
        let name = entry.file_name();
        let Some(id) = name.to_str().filter(|name| artifacts::valid_id(name)) else {
            continue;
        };
        let folder = Path::new("exports").join(id);
        let mut manifest = private_json(root, &folder.join("export_manifest.json"), key)?;
        if manifest["id"] != id
            || manifest
                .get("matter_id")
                .is_some_and(|owner| owner != matter)
        {
            return Err("Legacy export identity differs from its location".into());
        }
        let archived: Option<String> = source
            .query_row(
                "SELECT archived_at FROM export_archive WHERE matter_id=? AND build_id=?",
                params![matter, id],
                |row| row.get(0),
            )
            .optional()
            .map_err(message)?;
        manifest["archived_at"] = json!(archived);
        manifest["reader_indexes"] = reader_indexes(&manifest);
        manifest["backend_origin"] = json!("python");
        builds.push((id.to_owned(), folder, manifest));
    }
    builds.sort_by(|a, b| a.2["created_at"].as_str().cmp(&b.2["created_at"].as_str()));
    for (id, folder, manifest) in builds {
        target
            .execute(
                "INSERT INTO rust_exports VALUES (?,?,?)",
                params![id, matter, manifest.to_string()],
            )
            .map_err(message)?;
        for member in manifest["files"]
            .as_array()
            .ok_or("Legacy build has no artifact inventory")?
        {
            export_data(target, root, key, &folder, &id, member)?;
        }
        validation(target, root, key, matter, Some(&id))?;
    }
    Ok(())
}

fn copy_global(source: &Connection, target: &Connection) -> Result<(), String> {
    for row in rows(source, "SELECT * FROM output_presets", [])? {
        target
            .execute(
                "INSERT INTO output_presets VALUES (?,?,?,?,?)",
                params![
                    identifier(&row, "id")?,
                    field(&row, "name")?,
                    row["version"].as_u64(),
                    field(&row, "payload_json")?,
                    field(&row, "created_at")?,
                ],
            )
            .map_err(message)?;
    }
    for row in rows(
        source,
        "SELECT * FROM audit_events WHERE matter_id IS NOT NULL ORDER BY id",
        [],
    )? {
        target.execute("INSERT INTO audit_events (id,matter_id,document_id,action,created_at) VALUES (?,?,?,?,?)", params![
            row["id"].as_i64(), field(&row, "matter_id")?, row["document_id"].as_str(),
            field(&row, "action")?, field(&row, "created_at")?,
        ]).map_err(message)?;
    }
    for mut row in rows(source, "SELECT * FROM jobs ORDER BY created_at", [])? {
        let id = identifier(&row, "id")?;
        let matter = identifier(&row, "matter_id")?;
        for (column, key, default) in [
            ("context_json", "input_context", Value::Null),
            ("events_json", "events", json!([])),
            ("result_json", "result", Value::Null),
        ] {
            let parsed = if row[column].is_null() {
                default
            } else {
                decode_field(&row, column)?
            };
            row.as_object_mut()
                .expect("SQLite row object")
                .remove(column);
            row[key] = parsed;
        }
        target
            .execute(
                "INSERT INTO rust_processing_jobs VALUES (?,?,?)",
                params![id, matter, row.to_string()],
            )
            .map_err(message)?;
    }
    Ok(())
}

fn populate(source: &Connection, store: &mut Store, snapshot: &Path) -> Result<(), String> {
    let key = store.key.as_ref().ok_or("Protected store is locked")?;
    let root = &store.root;
    let tx = store
        .con
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(message)?;
    copy_global(source, &tx)?;
    for mut matter in rows(source, "SELECT * FROM matters ORDER BY created_at,id", [])? {
        let id = insert_matter(&tx, &mut matter)?;
        let source_root = checked_path(snapshot, &Path::new("matters").join(&id))?;
        let paths = documents(source, &tx, &source_root, root, key, &id, false)?;
        matter_records(source, &tx, &source_root, key, &id, &paths, false)?;
        exports(source, &tx, &source_root, key, &id)?;
        validation(&tx, &source_root, key, &id, None)?;
        frozen_jobs::migrate(&tx, &source_root, root, key, &id)?;
    }
    sync_directory(&root.join("documents-v1"))?;
    tx.commit().map_err(message)?;
    secure_db::verify_integrity(&store.con).map_err(message)?;
    Ok(())
}

fn insert_matter(target: &Connection, matter: &mut Value) -> Result<String, String> {
    let id = identifier(matter, "id")?;
    let parts = decode_field(matter, "parts_json")?;
    matter
        .as_object_mut()
        .expect("SQLite row object")
        .remove("parts_json");
    matter["parts"] = parts;
    matter["keep_groups_intact"] = json!(matter["keep_groups_intact"] == 1);
    validate(matter.clone(), true).map_err(|_| "Legacy matter metadata is invalid")?;
    target
        .execute(
            "INSERT INTO matters VALUES (?,?,?)",
            params![id, matter.to_string(), field(matter, "updated_at")?],
        )
        .map_err(message)?;
    Ok(id)
}

pub(crate) fn convert(
    snapshot: &Path,
    destination: &Path,
    credential: vault::Credential<'_>,
) -> Result<(), String> {
    let source = snapshot.canonicalize().map_err(message)?;
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()
        .map_err(message)?;
    let target = parent.join(
        destination
            .file_name()
            .ok_or("Choose a new Rust vault directory")?,
    );
    if target.starts_with(&source)
        || !matches!(fs::symlink_metadata(&target), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
    {
        return Err("Choose an unused destination outside the detached Python snapshot".into());
    }
    let path = checked_path(&source, Path::new("vault.json"))?;
    let mut raw = Vec::new();
    open_document_file(&path)?
        .take(16 * 1024 + 1)
        .read_to_end(&mut raw)
        .map_err(message)?;
    if raw.len() > 16 * 1024 {
        return Err("Legacy vault metadata exceeds its size limit".into());
    }
    let metadata: Value = serde_json::from_slice(&raw).map_err(message)?;
    if metadata["format"] != "courtbundle-vault"
        || metadata["version"] != 1
        || metadata["state"] != "active"
    {
        return Err("Expected a closed snapshot of an active Python vault".into());
    }
    let source_lock = checked_path(&source, Path::new(".sidecar.lock"))?;
    let _lock = if source_lock.is_file() {
        let file = open_document_file(&source_lock)?;
        file.try_lock_shared()
            .map_err(|_| "Close the source engine before copying the vault")?;
        Some(file)
    } else {
        None
    };
    for suffix in ["-wal", "-journal"] {
        let path = checked_path(&source, Path::new(&format!("{DATABASE}{suffix}")))?;
        if fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0) {
            return Err("Python snapshot contains uncheckpointed database changes".into());
        }
    }
    let key = vault::unlock_key(&metadata["envelope"], credential).map_err(message)?;
    let database = checked_path(&source, Path::new(DATABASE))?;
    let connection = secure_db::open_protected_snapshot(&database, &key).map_err(message)?;
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(message)?;
    if version > legacy_schema::VERSION {
        return Err(format!("Unsupported Python vault schema {version}"));
    }
    secure_db::verify_integrity(&connection).map_err(message)?;
    let staging = tempfile::Builder::new()
        .prefix(".matterdraft-migration-")
        .tempdir_in(&parent)
        .map_err(message)?;
    let connection = if version < legacy_schema::VERSION {
        let path = staging.path().join(DATABASE);
        fs::copy(&database, &path).map_err(message)?;
        let mut connection = secure_db::open_protected(&path, &key).map_err(message)?;
        connection
            .pragma_update(None, "trusted_schema", "OFF")
            .map_err(message)?;
        legacy_schema::upgrade(&mut connection, version).map_err(message)?;
        secure_db::verify_integrity(&connection).map_err(message)?;
        connection
    } else {
        connection
    };
    let staged = staging.path().join("vault");
    let mut vault =
        VaultStore::initialize_snapshot(&staged, metadata["envelope"].clone(), key.clone())?;
    populate(&connection, vault.snapshot_store()?, &source)?;
    drop(vault);
    sync_directory(&staged)?;
    // Keep the staged root private until the complete database and artifacts are durable.
    publish_unused(&staged, &target).map_err(message)?;
    sync_directory(&parent)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn publish_unused(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let source = CString::new(source.as_os_str().as_bytes())?;
    let target = CString::new(target.as_os_str().as_bytes())?;
    #[cfg(target_os = "macos")]
    let status = unsafe { libc::renamex_np(source.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(target_os = "linux")]
    let status = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn publish_unused(source: &Path, target: &Path) -> std::io::Result<()> {
    publish_directory(source, target)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn publish_unused(_source: &Path, _target: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Atomic vault publication is unavailable",
    ))
}
