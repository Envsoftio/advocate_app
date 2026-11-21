//! Immutable saved build artifacts, draft downloads and review state.
use super::{
    artifacts::{refusal, PdfSnapshot},
    bundle_inputs::{self, Inputs},
    bundle_writer::Build,
    document_output::{binary, print_pdf},
    reading, *,
};
use pdf_oxide::{
    editor::{DocumentEditor, EncryptionAlgorithm, EncryptionConfig, SaveOptions},
    PdfDocument,
};
use std::io::Read;

const REVIEW_LABELS: &[(&str, &str)] = &[
    ("navigation", "Index links and bookmarks"),
    ("pagination", "Printed page references"),
    ("coverage", "Document order and coverage"),
    ("readability", "Scan and image readability"),
];
const ARTIFACT_UNAVAILABLE: &str = "Missing file or content digest differs from manifest";

pub(super) fn migrate(con: &Connection) -> rusqlite::Result<()> {
    con.execute_batch(
        "CREATE TABLE rust_exports(id TEXT PRIMARY KEY,matter_id TEXT NOT NULL,manifest TEXT NOT NULL);
         CREATE INDEX rust_exports_matter ON rust_exports(matter_id);
         CREATE TABLE rust_export_files(
            export_id TEXT NOT NULL, filename TEXT NOT NULL, content BLOB NOT NULL, PRIMARY KEY(export_id,filename));
         CREATE TABLE rust_export_checks(id TEXT PRIMARY KEY, export_id TEXT NOT NULL, body TEXT NOT NULL);
         CREATE INDEX rust_export_checks_build ON rust_export_checks(export_id);
         CREATE TABLE rust_export_reviews(export_id TEXT PRIMARY KEY, body TEXT NOT NULL);
         PRAGMA user_version=10;",
    )
}

pub(super) fn publish(
    con: &Connection,
    matter: &str,
    id: &str,
    build: Build,
) -> rusqlite::Result<()> {
    con.execute(
        "INSERT INTO rust_exports VALUES (?,?,?)",
        params![id, matter, build.manifest.to_string()],
    )?;
    let mut stmt = con.prepare("INSERT INTO rust_export_files VALUES (?,?,?)")?;
    for (filename, content) in build.files {
        stmt.execute(params![id, filename, content])?;
    }
    save_check(con, &build.initial_check)
}

pub(super) fn storage(con: &Connection, matter: &str) -> rusqlite::Result<u64> {
    con.query_row(
        "SELECT coalesce(sum(length(content)),0) FROM rust_export_files
         JOIN rust_exports ON rust_exports.id=rust_export_files.export_id WHERE matter_id=?",
        [matter],
        |row| row.get(0),
    )
}

pub(super) fn label_history(
    con: &Connection,
    matter: &str,
    ids: &std::collections::BTreeSet<&str>,
) -> rusqlite::Result<Value> {
    let mut query =
        con.prepare("SELECT manifest FROM rust_exports WHERE matter_id=? ORDER BY rowid DESC")?;
    let rows = query.query_map([matter], |row| decode(&row.get::<_, String>(0)?))?;
    let mut prior = Vec::new();
    let mut unknown = Vec::new();
    for manifest in rows {
        let manifest = manifest?;
        if let Some(labels) = manifest["label_inputs"].as_array() {
            let uses = labels
                .iter()
                .filter(|item| {
                    item["document_id"]
                        .as_str()
                        .is_some_and(|id| ids.contains(id))
                })
                .collect::<Vec<_>>();
            if !uses.is_empty() {
                prior.push(
                    json!({"id":manifest["id"],"created_at":manifest["created_at"],"labels":uses}),
                );
            }
        } else {
            unknown.push(json!({"id":manifest["id"],"created_at":manifest["created_at"]}));
        }
    }
    Ok(
        json!({"has_prior_exports":!prior.is_empty() || !unknown.is_empty(),"prior_uses":prior,"unknown_exports":unknown}),
    )
}

pub(super) fn measured_for(
    con: &Connection,
    matter: &str,
    digest: &str,
) -> rusqlite::Result<Option<Value>> {
    let raw: Option<String> = con
        .query_row(
            "SELECT manifest FROM rust_exports WHERE matter_id=? AND
        json_extract(manifest,'$.build_input_digest')=?
        ORDER BY json_extract(manifest,'$.created_at') DESC,rowid DESC LIMIT 1",
            params![matter, digest],
            |row| row.get(0),
        )
        .optional()?;
    Ok(raw
        .map(|raw| decode(&raw))
        .transpose()?
        .map(|build| json!({"build_id":build["id"],"files":build["files"]})))
}

pub(super) fn load(con: &Connection, matter: &str, id: &str) -> rusqlite::Result<Option<Value>> {
    con.query_row(
        "SELECT manifest FROM rust_exports WHERE matter_id=? AND id=?",
        params![matter, id],
        |row| decode(&row.get::<_, String>(0)?),
    )
    .optional()
}

fn manifest_files(manifest: &Value) -> &[Value] {
    manifest["files"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
}

pub(super) fn file(con: &Connection, manifest: &Value, filename: &str) -> Result<Vec<u8>, Reply> {
    let expected = manifest_files(manifest)
        .iter()
        .find(|file| file["name"] == filename)
        .ok_or_else(|| refusal(404, "Export file not found"))?;
    let data: Option<Vec<u8>> = con
        .query_row(
            "SELECT content FROM rust_export_files WHERE export_id=? AND filename=?",
            params![manifest["id"].as_str(), filename],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| refusal(500, "Unable to read saved output"))?;
    let data = data.ok_or_else(|| refusal(404, "Export file not found"))?;
    if expected["sha256"] != sha256_hex(&data) || expected["bytes"] != data.len() {
        return Err(refusal(
            409,
            "Saved artifact bytes no longer match the manifest",
        ));
    }
    Ok(data)
}

fn checks(con: &Connection, id: &str) -> rusqlite::Result<Vec<Value>> {
    let mut query =
        con.prepare("SELECT body FROM rust_export_checks WHERE export_id=? ORDER BY rowid DESC")?;
    let rows = query.query_map([id], |row| decode(&row.get::<_, String>(0)?))?;
    rows.collect()
}

pub(super) fn output_run(
    con: &Connection,
    matter: &str,
    run: &str,
) -> rusqlite::Result<Option<Value>> {
    con.query_row(
        "SELECT runs.body FROM rust_export_checks runs JOIN rust_exports e ON e.id=runs.export_id
         WHERE runs.id=? AND e.matter_id=?",
        params![run, matter],
        |row| decode(&row.get::<_, String>(0)?),
    )
    .optional()
}

fn identity(manifest: &Value) -> String {
    sha256_hex(manifest["files"].to_string().as_bytes())
}

fn matches_identity(manifest: &Value, saved: &Value) -> bool {
    saved == &identity(manifest)
        || saved == &sha256_hex(preparation::python_json(&manifest["files"]).as_bytes())
}

fn review(con: &Connection, id: &str, manifest: &Value) -> rusqlite::Result<Value> {
    let saved = con
        .query_row(
            "SELECT body FROM rust_export_reviews WHERE export_id=?",
            [id],
            |row| decode(&row.get::<_, String>(0)?),
        )
        .optional()?;
    Ok(saved.unwrap_or_else(|| {
        let items = REVIEW_LABELS.iter().map(|&(key,_)| (key.to_owned(),json!(false)))
            .collect::<serde_json::Map<_,_>>();
        json!({"version":0,"build_id":id,"artifact_digest":identity(manifest),"items":items,"updated_at":null})
    }))
}

pub(super) fn measure_build(
    matter: &str,
    manifest: &Value,
    selected: Option<&Value>,
    mut read: impl FnMut(&str) -> Result<Vec<u8>, Reply>,
) -> Result<Value, Reply> {
    let all = manifest_files(manifest)
        .iter()
        .filter_map(|file| file["name"].as_str())
        .collect::<Vec<_>>();
    let mut names = match selected.filter(|value| !value.is_null()) {
        None => all.clone(),
        Some(value) => value
            .as_array()
            .and_then(|items| items.iter().map(Value::as_str).collect::<Option<Vec<_>>>())
            .filter(|names| {
                !names.is_empty()
                    && names.iter().all(|name| all.contains(name))
                    && names
                        .iter()
                        .copied()
                        .collect::<std::collections::HashSet<_>>()
                        .len()
                        == names.len()
            })
            .ok_or_else(|| refusal(422, "Choose existing, distinct build files"))?,
    };
    names.sort_unstable();
    names.dedup();
    let mut measured = serde_json::Map::new();
    let mut failed_files = 0;
    let mut navigation_failures = Vec::new();
    let mut label_failures = Vec::new();
    let mut cleanup_failures = Vec::new();
    let mut size_failures = Vec::new();
    let mut contracts_missing = Vec::new();
    let mut count_failures = Vec::new();
    let mut labels_found = Vec::new();
    let pdf_names = names
        .iter()
        .copied()
        .filter(|name| name.ends_with(".pdf"))
        .collect::<Vec<_>>();
    let cleanup = manifest["output_marks"]["cleanup_metadata"] == true;
    let legacy = manifest["backend_origin"] == "python";
    let max_bytes = manifest["validation_profile"]["max_file_mb"]
        .as_f64()
        .filter(|mb| *mb > 0.0)
        .map(|mb| (mb * 1024.0 * 1024.0) as u64);
    let volume_bytes = manifest["output_marks"]["volume_max_bytes"]
        .as_u64()
        .filter(|bytes| *bytes > 0);
    let id = uuid::Uuid::new_v4().simple().to_string();
    for &name in &names {
        let mut details = json!({});
        let measure = (|| -> Result<(), Reply> {
            let bytes = read(name).map_err(|error| match error.status {
                404 | 409 => refusal(422, ARTIFACT_UNAVAILABLE),
                _ => error,
            })?;
            details = json!({"bytes":bytes.len(),"sha256":sha256_hex(&bytes)});
            if max_bytes.is_some_and(|limit| bytes.len() as u64 > limit)
                || (name.starts_with("volume-")
                    && name.ends_with(".pdf")
                    && volume_bytes.is_some_and(|limit| bytes.len() as u64 > limit))
            {
                size_failures.push(name);
            }
            if name.ends_with(".pdf") {
                let pdf = PdfDocument::from_bytes(bytes)
                    .map_err(|_| refusal(422, "PDF cannot be opened"))?;
                let count = pdf
                    .page_count()
                    .map_err(|_| refusal(422, "PDF page count is unavailable"))?;
                if count == 0 || pdf.is_encrypted() {
                    return Err(refusal(422, "Encrypted or empty PDF"));
                }
                for page in 0..count {
                    pdf.extract_spans(page)
                        .map_err(|_| refusal(422, "PDF page content cannot be parsed"))?;
                }
                if cleanup && super::output_navigation::metadata_cleared(&pdf, legacy) != Some(true)
                {
                    cleanup_failures.push(name);
                }
                details["pages"] = json!(count);
                let contract = &manifest["validation_contract"][name];
                if contract.is_null() || contract.as_object().is_some_and(|spec| spec.is_empty()) {
                    contracts_missing.push(name);
                } else {
                    let count_matches = contract["pages"] == count;
                    details["counts"] = json!(if count_matches { "passed" } else { "failed" });
                    if !count_matches {
                        count_failures.push(format!(
                            "{name}: {count} pages; expected {}",
                            contract["pages"]
                        ));
                    }

                    let inspection =
                        super::output_navigation::inspect(&pdf, count, contract, legacy);
                    let navigation = inspection.as_ref().is_some_and(|spec| spec.navigation);
                    let labels = inspection.as_ref().is_some_and(|spec| spec.labels);
                    details["navigation"] = json!(if navigation { "passed" } else { "failed" });
                    details["labels"] = json!(if labels { "passed" } else { "failed" });
                    if !navigation {
                        navigation_failures.push(format!(
                            "{name}: destinations differ from saved deterministic map"
                        ));
                    }
                    if !labels {
                        let mismatches = inspection
                            .map(|spec| spec.label_mismatches)
                            .unwrap_or_default();
                        if mismatches.is_empty() {
                            label_failures.push(format!(
                                "{name}: viewer labels differ from saved deterministic map"
                            ));
                        }
                        for index in mismatches {
                            let label = contract["labels"][index]
                                .as_str()
                                .or_else(|| contract["printed_labels"][index].as_str())
                                .unwrap_or_default();
                            let message = format!(
                                "{name}: physical page {}, expected printed/viewer label {label}",
                                index + 1
                            );
                            if let Some(source) = contract["sources"][index.to_string()].as_object()
                            {
                                let finding_key =
                                    sha256_hex(format!("{id}/{name}/{index}").as_bytes());
                                labels_found.push(json!({"severity":"error","code":"OUTPUT_LABEL_MISMATCH",
                                    "message":message,"output_file":name,"output_page_index":index,
                                    "artifact":"working","document_id":source["document_id"],
                                    "page_index":source["page_index"],"target_revision":source["target_revision"],
                                    "finding_id":&finding_key[..24],"validation_run_id":id}));
                            }
                            label_failures.push(message);
                        }
                    }
                }
            } else if name.ends_with(".zip")
                || name.ends_with(".docx")
                || name.ends_with(".xlsx")
                || name.ends_with(".pptx")
            {
                let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
                    .map_err(|_| refusal(422, "ZIP cannot be opened"))?;
                let mut members = std::collections::HashSet::new();
                let mut member_names = Vec::new();
                for index in 0..archive.len() {
                    let mut member = archive
                        .by_index(index)
                        .map_err(|_| refusal(422, "ZIP entry cannot be opened"))?;
                    let member_name = member
                        .name()
                        .map_err(|_| refusal(422, "ZIP entry name is invalid"))?
                        .into_owned();
                    if !members.insert(member_name.clone()) {
                        return Err(refusal(422, "ZIP contains duplicate entries"));
                    }
                    member_names.push(member_name.clone());
                    if name.ends_with(".zip") && member_name == "READ_ME_FIRST.txt" {
                        if legacy {
                            std::io::copy(&mut member, &mut std::io::sink()).map_err(|_| {
                                refusal(422, "The ZIP review notice cannot be read")
                            })?;
                            continue;
                        }
                        let expected =
                            super::bundle_writer::draft_readme(&manifest["artifact_kind"]);
                        let older = super::bundle_writer::OLDER_DRAFT_README;
                        let mut notice = Vec::new();
                        member
                            .take(expected.len().max(older.len()) as u64 + 1)
                            .read_to_end(&mut notice)
                            .map_err(|_| refusal(422, "The ZIP review notice cannot be read"))?;
                        if notice != expected.as_bytes() && notice != older {
                            return Err(refusal(
                                422,
                                "The ZIP review notice differs from the saved build",
                            ));
                        }
                    } else if name.ends_with(".zip") {
                        let expected = manifest_files(manifest)
                            .iter()
                            .find(|file| file["name"] == member_name);
                        let length = expected
                            .and_then(|file| file["bytes"].as_u64())
                            .filter(|length| *length == member.size())
                            .ok_or_else(|| {
                                refusal(422, "ZIP entry differs from the saved build")
                            })?;
                        let mut raw = Vec::new();
                        member
                            .take(length + 1)
                            .read_to_end(&mut raw)
                            .map_err(|_| refusal(422, "ZIP entry failed its integrity check"))?;
                        if raw.len() as u64 != length
                            || expected.is_none_or(|file| file["sha256"] != sha256_hex(&raw))
                        {
                            return Err(refusal(422, "ZIP entry differs from the saved build"));
                        }
                    } else if member_name.ends_with(".xml") || member_name.ends_with(".rels") {
                        const XML_LIMIT: u64 = 80 * 1024 * 1024;
                        let mut raw = Vec::new();
                        member
                            .take(XML_LIMIT + 1)
                            .read_to_end(&mut raw)
                            .map_err(|_| refusal(422, "Office XML cannot be read"))?;
                        if raw.len() as u64 > XML_LIMIT {
                            return Err(refusal(422, "Office XML exceeds the review draft limit"));
                        }
                        let xml = std::str::from_utf8(&raw)
                            .map_err(|_| refusal(422, "Office XML is not UTF-8"))?;
                        roxmltree::Document::parse(xml)
                            .map_err(|_| refusal(422, "Office component is not valid XML"))?;
                    } else {
                        std::io::copy(&mut member, &mut std::io::sink())
                            .map_err(|_| refusal(422, "Office entry failed its integrity check"))?;
                    }
                }
                if name.ends_with(".zip") {
                    if !members.contains("READ_ME_FIRST.txt")
                        || !all
                            .iter()
                            .filter(|other| **other != name)
                            .all(|name| members.contains(*name))
                    {
                        return Err(refusal(422, "ZIP is missing a saved artifact"));
                    }
                    details["members"] = json!(member_names);
                } else {
                    let required = if name.ends_with(".xlsx") {
                        "xl/workbook.xml"
                    } else if name.ends_with(".docx") {
                        "word/document.xml"
                    } else {
                        "ppt/presentation.xml"
                    };
                    if !["[Content_Types].xml", "_rels/.rels", required]
                        .iter()
                        .all(|name| members.contains(*name))
                    {
                        return Err(refusal(
                            422,
                            "Office package is missing a required component",
                        ));
                    }
                    details["review_draft"] = json!(true);
                }
            } else if name.ends_with(".csv") {
                let rows =
                    super::office_drafts::parse_csv(&bytes).map_err(|error| refusal(422, error))?;
                let fields = super::bundle_writer::CSV_FIELDS;
                let expected = manifest["page_map_snapshot"]
                    .as_array()
                    .ok_or_else(|| refusal(422, "The build has no recorded CSV page map"))?;
                let (header, rows) = rows
                    .split_first()
                    .ok_or_else(|| refusal(422, "Index CSV header is missing"))?;
                if !header.iter().map(String::as_str).eq(fields.iter().copied()) {
                    return Err(refusal(
                        422,
                        "CSV columns differ from the saved index format",
                    ));
                }
                if rows.len() != expected.len() {
                    return Err(refusal(422, "CSV row count differs from the page map"));
                }
                for (index, (actual, planned)) in rows.iter().zip(expected).enumerate() {
                    if actual.len() != fields.len()
                        || actual.iter().zip(fields).any(|(cell, field)| {
                            planned
                                .get(*field)
                                .is_some_and(|value| *cell != super::bundle_writer::csv_cell(value))
                        })
                    {
                        return Err(refusal(
                            422,
                            format!("CSV row {} differs from the saved page map", index + 1),
                        ));
                    }
                }
                details["rows"] = json!(rows.len());
            } else if name.ends_with(".json") {
                let report = serde_json::from_slice::<Value>(&bytes)
                    .map_err(|_| refusal(422, "Report cannot be read"))?;
                if report["export_id"] != manifest["id"] {
                    return Err(refusal(422, "Report identifies a different build"));
                }
            } else {
                return Err(refusal(422, "No parser implemented for this file type"));
            }
            Ok(())
        })();
        if let Err(error) = measure {
            details["error"] = error.body["detail"].clone();
            failed_files += 1;
            if name.ends_with(".pdf") {
                let reason = format!("{name}: could not parse PDF");
                count_failures.push(reason.clone());
                navigation_failures.push(reason.clone());
                label_failures.push(reason);
                if cleanup {
                    cleanup_failures.push(name);
                }
            }
        }
        measured.insert(name.to_owned(), details);
    }
    let outcome = |failures: &[String]| {
        if !failures.is_empty() {
            "failed"
        } else if !contracts_missing.is_empty() {
            "not-run"
        } else if pdf_names.is_empty() {
            "not-applicable"
        } else {
            "passed"
        }
    };
    let redacted = manifest["redaction_inputs"]
        .as_array()
        .is_some_and(|items| !items.is_empty())
        || manifest["page_annotation_inputs"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|annotation| annotation["kind"] == "redaction");
    let omissions = manifest["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|check| {
            matches!(
                check["code"].as_str(),
                Some(
                    "UNRESOLVED_MANUAL_BOOKMARKS"
                        | "UNRESOLVED_BOOKMARK"
                        | "EXCLUDED_BOOKMARK_OMITTED"
                        | "UNRESOLVED_REVIEWED_LINKS"
                        | "UNRESOLVED_LINK"
                )
            )
        })
        .filter_map(|check| check["message"].as_str())
        .collect::<Vec<_>>();
    let navigation_outcome = outcome(&navigation_failures);
    let navigation_warning = navigation_outcome == "passed" && !omissions.is_empty();
    let cleanup_outcome = if !cleanup {
        "not-run"
    } else if !cleanup_failures.is_empty() {
        "failed"
    } else if pdf_names.is_empty() {
        "not-applicable"
    } else {
        "passed"
    };
    let measured_sizes = names
        .iter()
        .map(|&name| (name.to_owned(), measured[name]["bytes"].clone()))
        .collect::<serde_json::Map<_, _>>();
    let expected_files = manifest_files(manifest)
        .iter()
        .filter_map(|item| {
            let name = item["name"].as_str()?;
            names
                .contains(&name)
                .then(|| (name.to_owned(), item.clone()))
        })
        .collect::<serde_json::Map<_, _>>();
    let summary = |failed: &[String]| {
        if !failed.is_empty() {
            failed.join("; ")
        } else if !contracts_missing.is_empty() {
            format!("No recorded contract for {}", contracts_missing.join(", "))
        } else if pdf_names.is_empty() {
            "Selection contains no PDFs".to_owned()
        } else {
            format!("{} PDFs checked", pdf_names.len())
        }
    };
    let navigation_evidence = if navigation_warning {
        json!({"summary":"Recorded destinations match; omitted targets need review",
            "files":pdf_names,"omissions":omissions})
    } else {
        json!({"summary":summary(&navigation_failures),"files":pdf_names})
    };
    let cleanup_evidence = if cleanup {
        json!({"summary":"PDF Info and XMP cleared in selected PDFs; visible text and other artifacts are outside this cleanup scope",
            "files":pdf_names,"failures":cleanup_failures})
    } else {
        json!({"summary":"Optional PDF Info / XMP cleanup was not selected for this build"})
    };
    let input_revisions = manifest
        .get("input_revisions")
        .cloned()
        .unwrap_or_else(|| json!([]));
    Ok(json!({
        "id":id,"matter_id":matter,"build_id":manifest["id"],"scope":"output","created_at":now(),
        "input_digest":manifest["effective_input_digest"],"input_revisions":input_revisions,
        "settings":{"build":manifest["build_settings"],"output_marks":manifest["output_marks"],
            "profile":manifest["validation_profile"],"preset":manifest["output_preset"]},
        "selected_files":names,"artifact_digest":identity(manifest),"measured_files":measured,"expected_files":expected_files,
        "checks":[{"id":"files","name":"Files reopen successfully","outcome":if failed_files == 0 {"passed"} else {"failed"},
            "blocking":failed_files != 0,"findings":[],
            "evidence":{"summary":format!("{} / {} files parsed with matching SHA-256", names.len()-failed_files, names.len()),"files":measured}},
            {"id":"counts","name":"PDF counts match the page map","outcome":outcome(&count_failures),"blocking":matches!(outcome(&count_failures),"failed"|"not-run"),
                "findings":[],"evidence":{"summary":summary(&count_failures),"files":pdf_names}},
            {"id":"navigation","name":"Index links and bookmarks","outcome":if navigation_warning {"warning"} else {navigation_outcome},
                "blocking":matches!(navigation_outcome,"failed"|"not-run"),
                "findings":[],"evidence":navigation_evidence},
            {"id":"labels","name":"Printed and viewer page labels","outcome":outcome(&label_failures),"blocking":matches!(outcome(&label_failures),"failed"|"not-run"),
                "findings":labels_found,"evidence":{"summary":summary(&label_failures),"files":pdf_names}},
            {"id":"sizes","name":"Volume file-size limits","outcome":if !size_failures.is_empty() {"failed"} else if max_bytes.is_some() || volume_bytes.is_some() {"passed"} else {"not-applicable"},
                "blocking":!size_failures.is_empty(),"findings":[],"evidence":{"summary":format!("{} exceed configured limits; volume policy applies only to volume PDFs", size_failures.len()),
                    "limit_mb":manifest["validation_profile"]["max_file_mb"],"volume_limit_bytes":manifest["output_marks"]["volume_max_bytes"],"bytes":measured_sizes}},
            {"id":"cleanup","name":"Export cleanup","outcome":cleanup_outcome,"blocking":cleanup_outcome=="failed",
                "findings":[],"evidence":cleanup_evidence},
            {"id":"redaction","name":"Redaction verification","outcome":if redacted {"not-run"} else {"not-applicable"},"blocking":redacted,"findings":[],
                "evidence":{"summary":if redacted {"Content removal needs independent verification; a visible black mark is not proof"} else {"No recorded redaction operations in this build; this does not certify imported content."}}}],
    }))
}

fn save_check(con: &Connection, run: &Value) -> rusqlite::Result<()> {
    con.execute(
        "INSERT INTO rust_export_checks VALUES (?,?,?)",
        params![
            run["id"].as_str(),
            run["build_id"].as_str(),
            run.to_string()
        ],
    )?;
    Ok(())
}

fn files_match_manifest(con: &Connection, manifest: &Value) -> bool {
    let verify = || -> rusqlite::Result<bool> {
        let mut lookup =
            con.prepare("SELECT rowid FROM rust_export_files WHERE export_id=? AND filename=?")?;
        let mut buffer = [0u8; 64 * 1024];
        for entry in manifest_files(manifest) {
            let (Some(name), Some(bytes), Some(expected)) = (
                entry["name"].as_str(),
                entry["bytes"].as_u64(),
                entry["sha256"].as_str(),
            ) else {
                return Ok(false);
            };
            let row: i64 =
                lookup.query_row(params![manifest["id"].as_str(), name], |row| row.get(0))?;
            let mut file = con.blob_open(
                rusqlite::DatabaseName::Main,
                "rust_export_files",
                "content",
                row,
                true,
            )?;
            if file.len() as u64 != bytes {
                return Ok(false);
            }
            let mut digest = Sha256::new();
            loop {
                let Ok(read) = file.read(&mut buffer) else {
                    return Ok(false);
                };
                if read == 0 {
                    break;
                }
                digest.update(&buffer[..read]);
            }
            if vault::hex_bytes(&digest.finalize()).as_str() != expected {
                return Ok(false);
            }
        }
        Ok(true)
    };
    verify().unwrap_or(false)
}

fn status(
    con: &Connection,
    id: &str,
    manifest: &Value,
    files_match: bool,
) -> rusqlite::Result<Value> {
    let history = checks(con, id)?;
    let files = manifest_files(manifest);
    let run = history.iter().find(|run| {
        run["selected_files"].as_array().is_some_and(|selected| {
            selected.len() == files.len()
                && files.iter().all(|file| selected.contains(&file["name"]))
        })
    });
    let mut reasons = Vec::<String>::new();
    if let Some(run) = run.filter(|run| matches_identity(manifest, &run["artifact_digest"])) {
        for check in run["checks"].as_array().into_iter().flatten() {
            if check["outcome"] == "failed"
                || (check["outcome"] == "not-run"
                    && check["blocking"] == true
                    && check["id"] != "redaction")
            {
                reasons.push(format!(
                    "{}: {}",
                    check["name"].as_str().unwrap_or("Output checks"),
                    check["outcome"].as_str().unwrap_or_default()
                ));
            }
        }
    } else {
        reasons.push("Run checks for all saved output files".into());
    }
    if !files_match {
        reasons.push("Saved artifact bytes no longer match the manifest".into());
    }
    let labels = REVIEW_LABELS
        .iter()
        .map(|&(key, label)| (key.to_owned(), json!(label)))
        .collect::<serde_json::Map<_, _>>();
    let review = review(con, id, manifest)?;
    let mut release_reasons = reasons.clone();
    if !crate::RUST_WORKFLOW_READY {
        release_reasons
            .push("The Rust pilot still requires independent output qualification".into());
    }
    if run.is_some_and(|run| {
        run["checks"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|check| check["id"] == "redaction" && check["blocking"] == true)
    }) {
        release_reasons.push("Redaction verification: not-run".into());
    }
    if !manifest["archived_at"].is_null() {
        release_reasons.push("This build is archived; restore it before reviewed release".into());
    }
    if manifest["stale"] == true {
        release_reasons.push("Current effective inputs differ; this is a historical build".into());
    }
    if !matches_identity(manifest, &review["artifact_digest"])
        || !REVIEW_LABELS
            .iter()
            .all(|(key, _)| review["items"][*key] == true)
    {
        release_reasons.push("Human review is incomplete for this build".into());
    }
    Ok(json!({
        "build_id":id,"run":run,"history":history,"review":review,"review_labels":labels,"stale":manifest["stale"],
        "can_download":reasons.is_empty(),"can_release":release_reasons.is_empty(),"download_reasons":reasons,"release_reasons":release_reasons,
    }))
}

fn update_review(
    con: &Connection,
    id: &str,
    manifest: &Value,
    payload: &Value,
) -> Result<(), Reply> {
    let mut review =
        review(con, id, manifest).map_err(|_| refusal(500, "Unable to load output review"))?;
    if payload["artifact_digest"] != review["artifact_digest"]
        || !matches_identity(manifest, &payload["artifact_digest"])
        || payload["version"] != review["version"]
    {
        return Err(refusal(
            409,
            "Review changed or artifact identity differs; reload before saving",
        ));
    }
    if !payload["items"].as_object().is_some_and(|items| {
        items.len() == REVIEW_LABELS.len()
            && REVIEW_LABELS
                .iter()
                .all(|(key, _)| items.get(*key).is_some_and(Value::is_boolean))
    }) {
        return Err(refusal(
            409,
            "Supply all four human-review items as booleans",
        ));
    }
    review["items"] = payload["items"].clone();
    review["artifact_digest"] = json!(identity(manifest));
    review["version"] = json!(review["version"].as_u64().unwrap_or(0) + 1);
    review["updated_at"] = json!(now());
    con.execute("INSERT INTO rust_export_reviews VALUES (?,?) ON CONFLICT(export_id) DO UPDATE SET body=excluded.body",
        params![id,review.to_string()]).map_err(|_| refusal(500,"Unable to save output review"))?;
    Ok(())
}

fn gate(
    con: &Connection,
    id: &str,
    manifest: &Value,
    files_match: bool,
    release: bool,
) -> Result<(), Reply> {
    let state = status(con, id, manifest, files_match)
        .map_err(|_| refusal(500, "Unable to load output checks"))?;
    let (ready, reason) = if release {
        ("can_release", "release_reasons")
    } else {
        ("can_download", "download_reasons")
    };
    if state[ready] != true {
        let reasons = state[reason]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(refusal(409, format!("Output gate: {reasons}")));
    }
    Ok(())
}

fn protected_content(
    pdf: &PdfDocument,
    count: usize,
) -> Option<([u8; 32], super::output_navigation::NavigationEvidence)> {
    use super::page_tools::assembly::DisplayFrame;
    let mut digest = Sha256::new();
    for page in 0..count {
        let frame = DisplayFrame::new(pdf, page).ok()?;
        let text = pdf.extract_text(page).ok()?;
        for value in frame
            .geometry
            .crop
            .into_iter()
            .chain(frame.geometry.media)
            .chain([frame.unit])
        {
            // Oxide writes real numbers to five decimal places. Compare using
            // the same representation and normalize negative zero.
            let formatted = format!("{value:.5}");
            let serialized = if formatted == "-0.00000" {
                "0.00000"
            } else {
                &formatted
            };
            digest.update(serialized.as_bytes());
            digest.update([0]);
        }
        digest.update(frame.geometry.rotation.to_le_bytes());
        digest.update((text.len() as u64).to_le_bytes());
        digest.update(text.as_bytes());
    }
    let navigation = super::output_navigation::navigation_evidence(pdf, count)?;
    Some((digest.finalize().into(), navigation))
}

fn protected(bytes: Vec<u8>, password: &str) -> Result<Vec<u8>, Reply> {
    let failed = || refusal(422, "Protected PDF could not be reopened");
    let original = PdfDocument::from_bytes(bytes.clone()).map_err(|_| failed())?;
    let count = original.page_count().map_err(|_| failed())?;
    let source_content = protected_content(&original, count).ok_or_else(failed)?;
    drop(original);
    let mut editor = DocumentEditor::from_bytes(bytes).map_err(|_| failed())?;
    let owner = uuid::Uuid::new_v4().simple().to_string();
    let config = EncryptionConfig::new(password, owner).with_algorithm(EncryptionAlgorithm::Aes256);
    let result = editor
        .save_to_bytes_with_options(SaveOptions::with_encryption(config))
        .map_err(|_| failed())?;
    drop(editor);
    let opened = PdfDocument::from_bytes(result.clone()).map_err(|_| failed())?;
    if !opened.is_encrypted()
        || opened.authenticate(b"").unwrap_or(false)
        || !opened.authenticate(password.as_bytes()).unwrap_or(false)
        || opened.page_count().ok() != Some(count)
    {
        return Err(failed());
    }
    if !protected_content(&opened, count).is_some_and(|(pages, navigation)| {
        pages == source_content.0 && source_content.1.matches_serialized(&navigation)
    }) {
        return Err(refusal(
            422,
            "Protected PDF changed text, page geometry, labels or navigation",
        ));
    }
    Ok(result)
}

enum ValidationAction {
    Read,
    Run(Option<Value>),
    Review(Value),
}

pub struct OutputValidationTarget {
    matter: String,
    build: String,
    action: ValidationAction,
}

impl OutputValidationTarget {
    pub fn from_path(method: &str, path: &str, body: &Value) -> Option<Self> {
        let route = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
        Self::select(method, &route, body)
    }

    fn select(method: &str, route: &[&str], body: &Value) -> Option<Self> {
        let ["matters", matter, "exports", build, "validation", resource] = route else {
            return None;
        };
        let action = match (method, *resource) {
            ("GET", "state") => ValidationAction::Read,
            ("POST", "run") => ValidationAction::Run(body.get("files").cloned()),
            ("PUT", "review") => ValidationAction::Review(body.clone()),
            _ => return None,
        };
        Some(Self {
            matter: (*matter).to_owned(),
            build: (*build).to_owned(),
            action,
        })
    }
}

fn saved_file_rows(con: &Connection, build: &str) -> rusqlite::Result<Vec<(String, i64, u64)>> {
    let mut query = con.prepare(
        "SELECT filename,rowid,length(content) FROM rust_export_files WHERE export_id=? ORDER BY filename",
    )?;
    let rows = query.query_map([build], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    rows.collect()
}

fn validation_store_error() -> Reply {
    refusal(500, "Unable to load output checks")
}

fn validation_stale() -> Reply {
    refusal(409, "Saved output changed during checks; reopen the export")
}

fn missing_export(operation: &str) -> Reply {
    refusal(
        404,
        match operation {
            "inspection" => "Saved PDF not found",
            "print" => "Completed PDF not found",
            "protected" => "Export PDF not found",
            "docx" | "xlsx" | "pptx" => "Completed export file not found",
            _ => "Export file not found",
        },
    )
}

pub struct OutputValidationWork {
    target: OutputValidationTarget,
    root: PathBuf,
    reader: StoreReader,
    manifest: Value,
}

pub struct MeasuredOutputValidation {
    target: OutputValidationTarget,
    root: PathBuf,
    manifest: Value,
    stale: Value,
    files_match: bool,
    files: Vec<(String, i64, u64)>,
    run: Option<Value>,
}

impl OutputValidationWork {
    pub fn compute(self) -> Result<MeasuredOutputValidation, Reply> {
        let Self {
            target,
            root,
            reader,
            manifest,
        } = self;
        let con = reader.open().map_err(|_| validation_store_error())?;
        let tx = con
            .unchecked_transaction()
            .map_err(|_| validation_store_error())?;
        let OutputValidationTarget {
            matter,
            build,
            action,
        } = &target;
        let current = load(&tx, matter, build).map_err(|_| validation_store_error())?;
        if current.as_ref() != Some(&manifest) {
            return Err(validation_stale());
        }
        let files = saved_file_rows(&tx, build).map_err(|_| validation_store_error())?;
        // Artifact rows are immutable. Let writers commit during PDF parsing.
        tx.commit().map_err(|_| validation_store_error())?;
        let mut working = manifest.clone();
        mark_stale_at(&con, &root, reader.key.as_ref(), matter, &mut working)
            .map_err(|_| validation_store_error())?;
        let run = if let ValidationAction::Run(selected) = action {
            Some(measure_build(
                matter,
                &working,
                selected.as_ref(),
                |name| file(&con, &working, name),
            )?)
        } else {
            None
        };
        let files_match = files_match_manifest(&con, &working);
        Ok(MeasuredOutputValidation {
            target,
            root,
            manifest,
            stale: working["stale"].clone(),
            files_match,
            files,
            run,
        })
    }
}

pub struct ExportOutputTarget {
    matter: String,
    id: String,
    name: String,
    operation: &'static str,
    head: bool,
    release: bool,
}

impl ExportOutputTarget {
    pub fn from_path(method: &str, path: &str, query: &str) -> Option<Self> {
        let route = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
        Self::select(method, &route, query)
    }

    fn select(method: &str, route: &[&str], query: &str) -> Option<Self> {
        let ["matters", matter, "exports", id, tail @ ..] = route else {
            return None;
        };
        let (name, operation) = match (method, tail) {
            ("GET" | "HEAD", [name]) => (name, "file"),
            ("GET" | "HEAD", ["inspection", name]) => (name, "inspection"),
            ("POST", [name, "protected"]) => (name, "protected"),
            ("POST", [name, "print"]) => (name, "print"),
            ("GET", [name, "office", "docx"]) => (name, "docx"),
            ("GET", [name, "office", "xlsx"]) => (name, "xlsx"),
            ("GET", [name, "office", "pptx"]) => (name, "pptx"),
            ("GET", [name, "office", _]) => (name, "office"),
            _ => return None,
        };
        Some(Self {
            matter: (*matter).to_owned(),
            id: (*id).to_owned(),
            name: (*name).to_owned(),
            operation,
            head: method == "HEAD",
            release: reading::lookup(query, "release").as_deref() == Some("true"),
        })
    }
}

pub struct ExportSnapshotWork {
    reader: StoreReader,
    root: PathBuf,
    matter: String,
    build: String,
    manifest: Value,
    name: String,
    operation: String,
    options: Value,
}

impl ExportSnapshotWork {
    fn read_bytes(&self) -> Result<Vec<u8>, Reply> {
        let unavailable = || refusal(500, "Unable to read saved output");
        let con = self.reader.open().map_err(|_| unavailable())?;
        let tx = con.unchecked_transaction().map_err(|_| unavailable())?;
        if !super::document_metadata::matter(&tx, &self.matter)
            .map_err(|_| unavailable())?
            .is_some_and(|matter| matter["archived_at"].is_null())
        {
            return Err(refusal(404, "Matter not found"));
        }
        let mut manifest = load(&tx, &self.matter, &self.build)
            .map_err(|_| unavailable())?
            .ok_or_else(|| missing_export(&self.operation))?;
        if manifest != self.manifest {
            return Err(refusal(409, "Saved build changed; reopen the export"));
        }
        tx.commit().map_err(|_| unavailable())?;
        let bytes = file(&con, &manifest, &self.name).map_err(|error| {
            if matches!(error.status, 404 | 409) {
                missing_export(&self.operation)
            } else {
                error
            }
        })?;
        if self.operation != "inspection" {
            let release = self.operation == "file" && self.options["release"] == true;
            let files_match = files_match_manifest(&con, &manifest);
            if release {
                mark_stale_at(
                    &con,
                    &self.root,
                    self.reader.key.as_ref(),
                    &self.matter,
                    &mut manifest,
                )
                .map_err(|_| refusal(500, "Unable to check saved build freshness"))?;
            }
            // Read the latest full run and review together after the longer
            // file checks. Artifact rows are immutable during normal writes.
            let tx = con.unchecked_transaction().map_err(|_| unavailable())?;
            let current = load(&tx, &self.matter, &self.build).map_err(|_| unavailable())?;
            if current.as_ref() != Some(&self.manifest) {
                return Err(refusal(409, "Saved build changed; reopen the export"));
            }
            gate(&tx, &self.build, &manifest, files_match, release)?;
            tx.commit().map_err(|_| unavailable())?;
        }
        Ok(bytes)
    }

    pub fn compute(self) -> Result<PdfSnapshot, Reply> {
        let bytes = self.read_bytes()?;
        let output = match self.operation.as_str() {
            "file" | "inspection" => bytes,
            "print" => print_pdf(bytes, &self.options)?,
            "protected" => protected(bytes, self.options["password"].as_str().unwrap_or_default())?,
            "docx" | "xlsx" | "pptx" => {
                super::office_drafts::convert(&self.name, &self.operation, bytes)
                    .map_err(|error| refusal(422, error))?
            }
            _ => return Err(refusal(422, "Choose a completed PDF output")),
        };
        Ok(PdfSnapshot::from_bytes(output))
    }
}

pub struct ExportOutputWork {
    snapshot: ExportSnapshotWork,
    head: bool,
    media: &'static str,
}

impl ExportOutputWork {
    pub fn compute(self) -> Result<Reply, Reply> {
        let operation = self.snapshot.operation.as_str();
        let name = self.snapshot.name.as_str();
        let office = super::office_drafts::review_note(operation);
        let disposition = name.rsplit_once('.').and_then(|(stem, _)| match operation {
            "print" => Some(format!("inline; filename=\"{stem}-print.pdf\"")),
            "protected" => Some(format!("attachment; filename=\"{stem}-protected.pdf\"")),
            _ => office.map(|_| format!("attachment; filename=\"{stem}-review.{operation}\"")),
        });
        let snapshot = self.snapshot.compute()?;
        if self.head {
            return Ok(reply(
                200,
                json!({"bytes":snapshot.bytes,"media_type":self.media}),
            ));
        }
        snapshot
            .into_bytes()
            .map(|bytes| {
                let mut output = binary(bytes, self.media);
                if let Some(disposition) = disposition {
                    output.body["content_disposition"] = json!(disposition);
                }
                if let Some(note) = office {
                    output.body["conversion_note"] = json!(note);
                }
                output
            })
            .map_err(|_| refusal(409, "Export file is unavailable"))
    }
}

fn mark_stale_at(
    con: &Connection,
    root: &Path,
    key: Option<&DataKey>,
    matter: &str,
    manifest: &mut Value,
) -> rusqlite::Result<()> {
    let input = Inputs::read(con, root, key, matter)?.ok();
    let stale = if let (Some(input), Some(kind @ ("variant" | "supplement"))) =
        (&input, manifest["artifact_kind"].as_str())
    {
        let id = manifest[format!("{kind}_plan_id")]
            .as_str()
            .unwrap_or_default();
        let snapshot = input.digest(con)?;
        super::compositions::get(con, matter, kind, id).map_or(true, |plan| {
            plan["input_snapshot"] != snapshot
                || super::compositions::contract(&plan) != manifest["effective_input_digest"]
        })
    } else {
        let digest = input
            .as_ref()
            .map(|inputs| inputs.effective_digest(con))
            .transpose()?;
        digest.as_deref() != manifest["effective_input_digest"].as_str()
    };
    manifest["stale"] = json!(stale);
    Ok(())
}

impl Store {
    pub(super) fn mark_stale(&self, matter: &str, manifest: &mut Value) -> rusqlite::Result<()> {
        mark_stale_at(&self.con, &self.root, self.key.as_ref(), matter, manifest)
    }

    pub fn export_snapshot(
        &self,
        matter: &str,
        id: &str,
        name: &str,
        operation: &str,
        options: &Value,
    ) -> Result<PdfSnapshot, Reply> {
        self.prepare_export_snapshot(matter, id, name, operation, options.clone())?
            .compute()
    }

    pub fn prepare_export_snapshot(
        &self,
        matter: &str,
        id: &str,
        name: &str,
        operation: &str,
        options: Value,
    ) -> Result<ExportSnapshotWork, Reply> {
        if !self.active_matter(matter).unwrap_or(false) {
            return Err(refusal(404, "Matter not found"));
        }
        self.validate_output_options(matter, name, operation, &options)?;
        let manifest = self.output_manifest(matter, id).map_err(|error| {
            if error.status == 404 {
                missing_export(operation)
            } else {
                error
            }
        })?;
        self.capture_export_snapshot(matter, id, manifest, name, operation, options)
    }

    fn output_manifest(&self, matter: &str, id: &str) -> Result<Value, Reply> {
        load(&self.con, matter, id)
            .map_err(|_| refusal(500, "Unable to load saved build"))?
            .ok_or_else(|| refusal(404, "Build not found"))
    }

    fn validate_output_options(
        &self,
        matter: &str,
        name: &str,
        operation: &str,
        options: &Value,
    ) -> Result<(), Reply> {
        let is_pdf = name.ends_with(".pdf");
        match operation {
            "office" => return Err(refusal(422, "Choose Word, Excel or PowerPoint")),
            "inspection" if !is_pdf => return Err(refusal(404, "Saved PDF not found")),
            "print" if !is_pdf => return Err(refusal(422, "Only completed PDFs can be printed")),
            "xlsx" if name != "index.csv" => {
                return Err(refusal(
                    422,
                    "Excel export is derived from the completed index CSV",
                ));
            }
            "docx" | "pptx" if !is_pdf => {
                return Err(refusal(
                    422,
                    "Word and PowerPoint drafts require a completed PDF",
                ));
            }
            "protected" => {
                let owner = document_metadata::matter(&self.con, matter)
                    .map_err(|_| refusal(500, "Unable to load profile"))?
                    .ok_or_else(|| refusal(404, "Matter not found"))?;
                let profile = bundle_inputs::profile(&owner)
                    .map_err(|_| refusal(500, "Unable to load profile"))?;
                if profile["disallow_pdf_encryption"] == true {
                    return Err(refusal(409, "This illustrative profile disables encrypted PDF output as an unapproved technical setting. Choose a reviewed workflow before saving a protected PDF."));
                }
                if !is_pdf {
                    return Err(refusal(422, "Only PDF output can be password protected"));
                }
                options["password"]
                    .as_str()
                    .filter(|password| (1..=40).contains(&password.len()))
                    .ok_or_else(|| refusal(422, "Enter a PDF password of 1 to 40 UTF-8 bytes"))?;
            }
            _ => {}
        }
        Ok(())
    }

    fn capture_export_snapshot(
        &self,
        matter: &str,
        build: &str,
        manifest: Value,
        name: &str,
        operation: &str,
        options: Value,
    ) -> Result<ExportSnapshotWork, Reply> {
        let reader =
            StoreReader::capture(self).map_err(|_| refusal(500, "Unable to read saved output"))?;
        Ok(ExportSnapshotWork {
            reader,
            root: self.root.clone(),
            matter: matter.to_owned(),
            build: build.to_owned(),
            manifest,
            name: name.to_owned(),
            operation: operation.to_owned(),
            options,
        })
    }

    pub fn prepare_export_output(
        &self,
        target: ExportOutputTarget,
        body: Value,
    ) -> Result<ExportOutputWork, Reply> {
        let ExportOutputTarget {
            matter,
            id,
            name,
            operation,
            head,
            release,
        } = target;
        if !self
            .active_matter(&matter)
            .map_err(|_| refusal(500, "Rust matter transaction failed; no changes committed"))?
        {
            return Err(refusal(404, "Matter not found"));
        }
        let options = if operation == "file" {
            json!({"release":release})
        } else {
            body
        };
        self.validate_output_options(&matter, &name, operation, &options)?;
        let manifest = self.output_manifest(&matter, &id).map_err(|error| {
            if error.status == 404 {
                missing_export(operation)
            } else {
                error
            }
        })?;
        let snapshot =
            self.capture_export_snapshot(&matter, &id, manifest, &name, operation, options)?;
        Ok(ExportOutputWork {
            snapshot,
            head,
            media: media(&name, operation),
        })
    }

    pub fn prepare_output_validation(
        &self,
        target: OutputValidationTarget,
    ) -> Result<OutputValidationWork, Reply> {
        if !self
            .active_matter(&target.matter)
            .map_err(|_| validation_store_error())?
        {
            return Err(refusal(404, "Matter not found"));
        }
        let manifest = self
            .output_manifest(&target.matter, &target.build)
            .map_err(|error| {
                if error.status != 404 {
                    return error;
                }
                let status = match target.action {
                    ValidationAction::Read => 404,
                    ValidationAction::Run(_) => 422,
                    ValidationAction::Review(_) => 409,
                };
                refusal(status, "Saved build not found")
            })?;
        let reader = StoreReader::capture(self).map_err(|_| validation_store_error())?;
        Ok(OutputValidationWork {
            target,
            root: self.root.clone(),
            reader,
            manifest,
        })
    }

    pub fn complete_output_validation(
        &self,
        measured: MeasuredOutputValidation,
    ) -> Result<Reply, Reply> {
        let MeasuredOutputValidation {
            target,
            root,
            mut manifest,
            stale,
            files_match,
            files,
            run,
        } = measured;
        if root != self.root {
            return Err(validation_stale());
        }
        let OutputValidationTarget {
            matter,
            build,
            action,
        } = target;
        let tx = self
            .con
            .unchecked_transaction()
            .map_err(|_| validation_store_error())?;
        if !document_metadata::matter(&tx, &matter)
            .map_err(|_| validation_store_error())?
            .is_some_and(|matter| matter["archived_at"].is_null())
        {
            return Err(refusal(404, "Matter not found"));
        }
        let current = load(&tx, &matter, &build).map_err(|_| validation_store_error())?;
        if current.as_ref() != Some(&manifest)
            || saved_file_rows(&tx, &build).map_err(|_| validation_store_error())? != files
        {
            return Err(validation_stale());
        }
        manifest["stale"] = stale;
        if let Some(run) = run {
            save_check(&tx, &run).map_err(|_| refusal(500, "Unable to save output checks"))?;
        }
        if let ValidationAction::Review(fields) = action {
            update_review(&tx, &build, &manifest, &fields)?;
        }
        let output =
            status(&tx, &build, &manifest, files_match).map_err(|_| validation_store_error())?;
        tx.commit().map_err(|_| validation_store_error())?;
        Ok(reply(200, output))
    }

    pub(super) fn generated_export_request(
        &self,
        method: &str,
        route: &[&str],
        query: &str,
        body: &Value,
    ) -> rusqlite::Result<Option<Reply>> {
        let ["matters", matter, "exports", tail @ ..] = route else {
            return Ok(None);
        };
        if let ("POST", [id, operation @ ("compare" | "compare-pages")]) = (method, tail) {
            let result = self
                .prepare_export_comparison(matter, id, operation, body)
                .and_then(super::export_comparison::ExportComparisonWork::measure)
                .and_then(|measured| self.complete_export_comparison(measured));
            return Ok(Some(result.unwrap_or_else(|error| error)));
        }
        if let Some(target) = OutputValidationTarget::select(method, route, body) {
            let result = self
                .prepare_output_validation(target)
                .and_then(OutputValidationWork::compute)
                .and_then(|measured| self.complete_output_validation(measured));
            return Ok(Some(result.unwrap_or_else(|error| error)));
        }
        if let Some(target) = ExportOutputTarget::select(method, route, query) {
            let result = self
                .prepare_export_output(target, body.clone())
                .and_then(ExportOutputWork::compute);
            return Ok(Some(result.unwrap_or_else(|error| error)));
        }
        if !self.active_matter(matter)? {
            return Ok(Some(refusal(404, "Matter not found")));
        }
        if method == "GET" && tail.is_empty() {
            let mut query = self.con.prepare(
                "SELECT manifest FROM rust_exports WHERE matter_id=? ORDER BY rowid DESC",
            )?;
            let rows = query.query_map([matter], |row| decode(&row.get::<_, String>(0)?))?;
            let mut result = Vec::new();
            for manifest in rows {
                let mut manifest = manifest?;
                self.mark_stale(matter, &mut manifest)?;
                result.push(manifest);
            }
            return Ok(Some(reply(200, json!(result))));
        }
        let [id, tail @ ..] = tail else {
            return Ok(None);
        };
        let Some(mut manifest) = load(&self.con, matter, id)? else {
            return Ok(Some(refusal(404, "Build not found")));
        };
        let response = match (method, tail) {
            ("PUT", ["archive"]) => {
                let Some(archived) = body["archived"].as_bool() else {
                    return Ok(Some(refusal(422, "Choose an archive status")));
                };
                if archived && manifest["archived_at"].is_null() {
                    manifest["archived_at"] = json!(now());
                }
                if !archived {
                    manifest["archived_at"] = Value::Null;
                }
                let tx = self.con.unchecked_transaction()?;
                tx.execute(
                    "UPDATE rust_exports SET manifest=? WHERE id=?",
                    params![manifest.to_string(), id],
                )?;
                tx.execute(
                    "INSERT INTO audit_events (matter_id,action,created_at) VALUES (?,?,?)",
                    params![
                        matter,
                        if archived {
                            "build_archived"
                        } else {
                            "build_restored"
                        },
                        now()
                    ],
                )?;
                tx.commit()?;
                self.mark_stale(matter, &mut manifest)?;
                reply(200, manifest)
            }
            _ => return Ok(None),
        };
        Ok(Some(response))
    }
}

fn media(filename: &str, operation: &str) -> &'static str {
    match operation {
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ if filename.ends_with(".pdf") => "application/pdf",
        _ if filename.ends_with(".zip") => "application/zip",
        _ if filename.ends_with(".csv") => "text/csv",
        _ if filename.ends_with(".json") => "application/json",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests;
