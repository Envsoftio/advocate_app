//! Convert retryable jobs using their original saved inputs.
use super::*;

fn python_ascii(value: &Value) -> String {
    use std::fmt::Write as _;
    let mut output = String::new();
    for character in preparation::python_json(value).chars() {
        if character <= '~' {
            output.push(character);
        } else {
            for unit in character.encode_utf16(&mut [0; 2]) {
                write!(output, "\\u{unit:04x}").expect("formatting a String");
            }
        }
    }
    output
}

fn verify_frozen_tables(source: &Connection, expected: &Value) -> Result<(), String> {
    let mut content = Map::new();
    for table in [
        "matters",
        "documents",
        "memo_parties",
        "matter_values",
        "manual_bookmarks",
        "reviewed_links",
        "output_marks",
        "matter_output_presets",
        "page_annotations",
        "source_page_identities",
        "supplemental_plans",
        "bundle_membership_state",
        "audience_variants",
        "document_revisions",
        "redaction_records",
        "ocr_page_reviews",
    ] {
        let mut data = rows(source, &format!("SELECT * FROM {table}"), [])?;
        data.sort_by_cached_key(python_ascii);
        content.insert(table.to_owned(), json!(data));
    }
    if expected != &sha256_hex(python_ascii(&Value::Object(content)).as_bytes()) {
        return Err("Saved legacy build tables failed their integrity check".into());
    }
    Ok(())
}

fn job_error(error: Reply) -> String {
    error.body["detail"]
        .as_str()
        .unwrap_or("Legacy build inputs are unavailable")
        .to_owned()
}

fn prepare_frozen(
    target: &Connection,
    snapshot: &Path,
    staging: &Path,
    key: &DataKey,
    matter: &str,
    job: &Value,
) -> Result<(String, Vec<Vec<u8>>), String> {
    let context = &job["input_context"];
    let metadata = private_json(snapshot, Path::new("snapshot.json"), key)?;
    if metadata["summary"] != *context {
        return Err("Saved legacy build input contract changed".into());
    }
    let database = checked_path(snapshot, Path::new(DATABASE))?;
    let con = secure_db::open_protected_snapshot(&database, key).map_err(message)?;
    verify_frozen_tables(&con, &metadata["tables_digest"])?;
    let files = metadata["file_hashes"]
        .as_object()
        .ok_or("Saved build has no file identities")?;
    let source_root = checked_path(snapshot, &Path::new("matters").join(matter))?;
    for (relative, expected) in files {
        let reader = encrypted(&source_root, Path::new(relative), key)?;
        if expected != &vault::hex_bytes(reader.digest()).as_str() {
            return Err("Saved legacy build file failed its integrity check".into());
        }
    }
    if metadata["unavailable"]
        .as_array()
        .is_none_or(|items| !items.is_empty())
    {
        return Err("Saved legacy build is missing source files".into());
    }
    let scratch = tempfile::Builder::new()
        .prefix(".frozen-")
        .tempdir_in(staging)
        .map_err(message)?;
    let database = scratch.path().join("rust-matters.sqlite3");
    secure_db::create_protected(&database, key).map_err(message)?;
    let database = secure_db::open_protected(&database, key).map_err(message)?;
    let mut store = Store::open_keyed(scratch.path(), database, key.clone())?;
    let kind = field(job, "kind")?;
    let mut owner = rows(&con, "SELECT * FROM matters WHERE id=?", [matter])?
        .pop()
        .ok_or("Saved legacy build matter is missing")?;
    insert_matter(&store.con, &mut owner)?;
    if bundle_inputs::profile(&owner).map_err(message)? != metadata["profile"] {
        return Err("The technical profile changed since the legacy build was submitted".into());
    }
    let current_only = kind != "supplement-build";
    let paths = documents(
        &con,
        &store.con,
        &source_root,
        scratch.path(),
        key,
        matter,
        current_only,
    )?;
    matter_records(
        &con,
        &store.con,
        &source_root,
        key,
        matter,
        &paths,
        current_only,
    )?;
    let input = bundle_inputs::Inputs::read(&store.con, &store.root, store.key.as_ref(), matter)
        .map_err(message)?
        .map_err(job_error)?;
    if context["input_digest"] != input.digest(&store.con).map_err(message)? {
        return Err("Saved legacy build inputs differ after conversion".into());
    }
    let freeze = if kind == "build" {
        let mut capture =
            bundle_writer::BuildInputs::capture(&store, matter, true).map_err(job_error)?;
        let checks = store.run_preflight(matter).map_err(job_error)?;
        capture.set_checks(
            checks
                .body
                .as_array()
                .cloned()
                .ok_or_else(|| job_error(checks))?,
        );
        capture
            .prepare_freeze(&store, matter, None)
            .map_err(job_error)?
    } else {
        let plan = &metadata["plan"];
        let id = identifier(plan, "id")?;
        let mode = field(plan, "kind")?;
        if context["plan_id"] != id || kind != format!("{mode}-build") {
            return Err("Saved legacy composition belongs to a different job".into());
        }
        if mode == "supplement" {
            let baseline = identifier(plan, "baseline_build_id")?;
            let manifest: String = target
                .query_row(
                    "SELECT manifest FROM rust_exports WHERE id=? AND matter_id=?",
                    params![baseline, matter],
                    |row| row.get(0),
                )
                .map_err(|_| "The frozen supplement's historical build is unavailable")?;
            store
                .con
                .execute(
                    "INSERT INTO rust_exports VALUES (?,?,?)",
                    params![baseline, matter, manifest],
                )
                .map_err(message)?;
        }
        store
            .con
            .execute(
                "INSERT INTO rust_compositions VALUES (?,?,?,?)
             ON CONFLICT(id) DO UPDATE SET body=excluded.body",
                params![id, matter, mode, plan.to_string()],
            )
            .map_err(message)?;
        let (build, plan) =
            bundle_writer::BuildInputs::capture_composition(&store, matter, mode, &id, true)
                .map_err(job_error)?;
        let mut freeze = build
            .prepare_freeze(&store, matter, Some(&plan.selection))
            .map_err(job_error)?;
        freeze.set_composition(plan.measure().map_err(job_error)?);
        freeze
    };
    let (frozen, sources, _) = freeze.compute().map_err(job_error)?;
    let snapshot_id = identifier(context, "snapshot_id")?;
    let summary = frozen.summary(kind, &snapshot_id, "", context["plan_id"].as_str());
    for field in [
        "included_documents",
        "source_pages",
        "generated_pages",
        "physical_pages",
        "volumes",
    ] {
        if summary[field] != context[field] {
            return Err(format!("Frozen legacy build differs in {field}"));
        }
    }
    Ok((serde_json::to_string(&frozen).map_err(message)?, sources))
}

pub(super) fn migrate(
    target: &Connection,
    source_root: &Path,
    staging: &Path,
    key: &DataKey,
    matter: &str,
) -> Result<(), String> {
    let jobs = rows(target,
        "SELECT body FROM rust_processing_jobs WHERE matter_id=? AND
            json_extract(body,'$.kind') IN ('build','variant-build','supplement-build') AND
            json_extract(body,'$.status') IN ('queued','running','interrupted','failed','cancelled') AND
            json_extract(body,'$.retry_job_id') IS NULL",
        [matter],
    )?;
    for row in jobs {
        let mut job = decode_field(&row, "body")?;
        let id = identifier(&job, "id")?;
        let Ok(snapshot_id) = identifier(&job["input_context"], "snapshot_id") else {
            continue;
        };
        let exists: bool = target
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM rust_frozen_builds WHERE matter_id=? AND id=?)",
                params![matter, snapshot_id],
                |row| row.get(0),
            )
            .map_err(message)?;
        let digest = if exists {
            let raw: String = target
                .query_row(
                    "SELECT body FROM rust_frozen_builds WHERE id=?",
                    [&snapshot_id],
                    |row| row.get(0),
                )
                .map_err(message)?;
            sha256_hex(raw.as_bytes())
        } else {
            let prepared = checked_path(source_root, &Path::new("build-inputs").join(&snapshot_id))
                .and_then(|snapshot| prepare_frozen(target, &snapshot, staging, key, matter, &job));
            let (raw, sources) = match prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    job["legacy_retry_blocker"] = json!(error);
                    target
                        .execute(
                            "UPDATE rust_processing_jobs SET body=? WHERE id=?",
                            params![job.to_string(), id],
                        )
                        .map_err(message)?;
                    continue;
                }
            };
            target
                .execute(
                    "INSERT INTO rust_frozen_builds VALUES (?,?,?)",
                    params![snapshot_id, matter, raw],
                )
                .map_err(message)?;
            let mut insert = target
                .prepare("INSERT INTO rust_frozen_sources VALUES (?,?,?)")
                .map_err(message)?;
            for (index, bytes) in sources.into_iter().enumerate() {
                insert
                    .execute(params![snapshot_id, index, bytes])
                    .map_err(message)?;
            }
            sha256_hex(raw.as_bytes())
        };
        job["input_context"]["snapshot_digest"] = json!(digest);
        target
            .execute(
                "UPDATE rust_processing_jobs SET body=? WHERE id=?",
                params![job.to_string(), id],
            )
            .map_err(message)?;
    }
    Ok(())
}
