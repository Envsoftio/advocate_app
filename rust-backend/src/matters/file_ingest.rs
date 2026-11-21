//! Scoped input transformations and generated cover sheets.
use super::{
    artifacts::refusal,
    document_output::{normalize_image, textbox::Textbox},
    document_verification::{DocumentFileProof, RevisionInput},
    page_tools::{assembly, signature_guard},
    revisions::ValidatedRevisionPdf,
    *,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::collections::{BTreeSet, HashMap};

const IPC_LIMIT: usize = 10 * 1024 * 1024;

fn bytes(item: &Value) -> Result<Vec<u8>, Reply> {
    let source = item["data"]
        .as_str()
        .ok_or_else(|| refusal(422, "A local source file is required"))?;
    if source.len() > 4 * IPC_LIMIT.div_ceil(3) {
        return Err(refusal(413, "File exceeds the Rust upload limit"));
    }
    let bytes = STANDARD
        .decode(source)
        .map_err(|_| refusal(422, "Invalid uploaded file"))?;
    if bytes.is_empty() {
        return Err(refusal(422, "Source file is empty"));
    }
    if bytes.len() > IPC_LIMIT {
        return Err(refusal(413, "File exceeds the Rust upload limit"));
    }
    Ok(bytes)
}

fn normalized(filename: &str, bytes: &[u8]) -> Result<(Vec<u8>, usize), Reply> {
    if bytes.is_empty() {
        return Err(refusal(422, "Replacement file is empty"));
    }
    if bytes.len() > DOCUMENT_IMPORT_MAX_BYTES {
        return Err(refusal(413, "Replacement file limit is 80 MB"));
    }
    let suffix = replacement_suffix(filename)?;
    match suffix {
        "pdf" => {
            let pages = admission::replacement_pdf_pages(bytes)
                .map_err(|error| refusal(error.status, error.detail))?;
            Ok((bytes.to_vec(), pages))
        }
        "png" | "jpg" | "jpeg" => normalize_image(bytes)
            .map(|bytes| (bytes, 1))
            .map_err(|error| refusal(422, error)),
        _ => Err(refusal(415, "Choose a PDF, PNG or JPEG file")),
    }
}

fn replacement_suffix(filename: &str) -> Result<&'static str, Reply> {
    let suffix = Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    ["pdf", "png", "jpg", "jpeg"]
        .into_iter()
        .find(|valid| suffix.eq_ignore_ascii_case(valid))
        .ok_or_else(|| refusal(415, "Choose a PDF, PNG or JPEG file"))
}

pub(super) fn cover_fields(matter: &Value) -> Value {
    super::bundle_inputs::select(
        matter,
        &[
            "name",
            "court_profile",
            "case_type",
            "petitioner",
            "respondent",
            "advocate",
            "case_number",
        ],
    )
}

fn cover(matter: &Value) -> Result<Vec<u8>, Reply> {
    let mut overlays = assembly::Overlay::default();
    let text = |key: &str| matter[key].as_str().unwrap_or_default();
    overlays
        .commands
        .push_str("q 0.12 0.2 0.33 RG 1.2 w 40 40 515 762 re S Q\n");
    let visible_text = super::bundle_writer::visible_text;
    let textbox = Textbox::registered("Helvetica");
    visible_text(
        &mut overlays,
        "CASE FILE - DRAFT COVER SHEET",
        "Helvetica-Bold",
        17.0,
        64.0,
        747.0,
        (0.12, 0.2, 0.33),
    )?;
    visible_text(
        &mut overlays,
        "For preparation only - not an approved court form",
        "Helvetica",
        10.0,
        64.0,
        710.0,
        (0.5, 0.15, 0.15),
    )?;
    for (index, (heading, field)) in [
        ("MATTER", "name"),
        ("COURT PROFILE", "court_profile"),
        ("CASE TYPE", "case_type"),
        ("PETITIONER", "petitioner"),
        ("RESPONDENT", "respondent"),
        ("ADVOCATE", "advocate"),
        ("CASE NUMBER", "case_number"),
    ]
    .into_iter()
    .enumerate()
    {
        let y = 642.0 - 64.0 * index as f64;
        let value = text(field).replace('\t', " ");
        visible_text(
            &mut overlays,
            heading,
            "Helvetica",
            9.0,
            65.0,
            y,
            (0.4, 0.46, 0.52),
        )?;
        let value = if value.is_empty() {
            "--"
        } else {
            value.as_str()
        };
        let lines = textbox.wrap(value, 465.0, 12.0);
        if textbox.fits(lines.len(), 12.0, 35.0) {
            for (index, (line, _)) in lines.into_iter().enumerate() {
                visible_text(
                    &mut overlays,
                    line,
                    "Helvetica",
                    12.0,
                    65.0,
                    textbox.baseline(y - 8.0, 12.0, index),
                    (0., 0., 0.),
                )?;
            }
        }
    }
    let base = assembly::blank((595.0, 842.0)).map_err(|error| refusal(422, error))?;
    assembly::overlay_document(base, &HashMap::from([(0, overlays)]))
        .map_err(|error| refusal(422, error))
}

pub const DOCUMENT_IMPORT_MAX_BYTES: usize = admission::FILE_LIMIT_BYTES;

pub struct RawDocumentUpload {
    filename: String,
    source_file_mtime: String,
    password: Option<String>,
    acknowledge_signature_risk: bool,
    included: bool,
}

impl RawDocumentUpload {
    pub fn parse(metadata: &str) -> Result<Self, Reply> {
        let field = |key| reading::lookup(metadata, key);
        let media = field("content_type").unwrap_or_default();
        if media.split(';').next() != Some("application/octet-stream") {
            return Err(refusal(415, "Stream one PDF, PNG or JPEG file"));
        }
        let filename = field("filename").unwrap_or_default().into_owned();
        if filename.is_empty()
            || filename.chars().count() > 500
            || filename.contains(['/', '\\', '\0'])
        {
            return Err(refusal(422, "Invalid document filename"));
        }
        let source_file_mtime = field("source_file_mtime").unwrap_or_default().into_owned();
        validate_source_timestamp(&source_file_mtime)?;
        Ok(Self {
            filename,
            source_file_mtime,
            password: field("password").map(Cow::into_owned),
            acknowledge_signature_risk: field("acknowledge_signature_risk").as_deref()
                == Some("true"),
            included: field("included").as_deref() == Some("true"),
        })
    }

    pub fn compute(
        self,
        target: DocumentImportTarget,
        bytes: Vec<u8>,
    ) -> Result<PreparedDocumentImport<'static>, Reply> {
        target.measure(
            self.filename,
            bytes,
            ImportOptions {
                password: self.password.as_deref(),
                acknowledge_signature_risk: self.acknowledge_signature_risk,
                included: self.included,
                source_file_mtime: &self.source_file_mtime,
            },
        )
    }
}

fn validate_source_timestamp(input: &str) -> Result<(), Reply> {
    if input.is_empty() {
        return Ok(());
    }
    if input.chars().count() > 40 || !iso_timestamp(input) {
        return Err(refusal(422, "Invalid source file timestamp"));
    }
    Ok(())
}

fn iso_timestamp(input: &str) -> bool {
    use chrono::{Datelike, NaiveDate};

    // Week dates may omit the weekday, including when a single-character
    // separator precedes the time. Try the valid date shapes before the clock.
    for (size, format, suffix) in [
        (10, "%Y-%m-%d", ""),
        (8, "%Y%m%d", ""),
        (10, "%G-W%V-%u", ""),
        (8, "%GW%V%u", ""),
        (8, "%G-W%V-%u", "-1"),
        (7, "%GW%V%u", "1"),
    ] {
        let Some(date) = input.get(..size) else {
            continue;
        };
        if !date.is_ascii()
            || NaiveDate::parse_from_str(&format!("{date}{suffix}"), format)
                .ok()
                .is_none_or(|date| !(1..=9999).contains(&date.year()))
        {
            continue;
        }
        let rest = &input[size..];
        if rest.is_empty() {
            return true;
        }
        let clock = &rest[rest.chars().next().unwrap().len_utf8()..];
        let clock = clock.replace('Z', "+00:00");
        let (local, offset) = clock
            .split_once(['+', '-'])
            .map_or((clock.as_str(), None), |(local, offset)| {
                (local, Some(offset))
            });
        if clock_fields(local).is_some_and(|[h, m, s]| h < 24 && m < 60 && s < 60)
            && offset.is_none_or(|offset| {
                clock_fields(offset).is_some_and(|[h, m, s]| h * 3600 + m * 60 + s < 86400)
            })
        {
            return true;
        }
    }
    false
}

fn clock_fields(value: &str) -> Option<[u32; 3]> {
    let base = if let Some((base, fraction)) = value.split_once(['.', ',']) {
        if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        base
    } else {
        value
    };
    let number = |start, end| -> Option<u32> {
        let digits = base.get(start..end)?;
        digits
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| digits.parse().ok())
            .flatten()
    };
    match (base.len(), base.as_bytes().get(2)) {
        (2, _) => Some([number(0, 2)?, 0, 0]),
        (4, _) => Some([number(0, 2)?, number(2, 4)?, 0]),
        (5, Some(b':')) => Some([number(0, 2)?, number(3, 5)?, 0]),
        (6, _) => Some([number(0, 2)?, number(2, 4)?, number(4, 6)?]),
        (8, Some(b':')) if base.as_bytes().get(5) == Some(&b':') => {
            Some([number(0, 2)?, number(3, 5)?, number(6, 8)?])
        }
        _ => None,
    }
}

pub struct CoverImportWork {
    target: DocumentImportTarget,
    fields: Value,
}

impl CoverImportWork {
    pub fn compute(self) -> Result<PreparedDocumentImport<'static>, Reply> {
        let content = cover(&self.fields)?;
        self.target.measure_bytes(
            "generated-cover.pdf".into(),
            Cow::Owned(content),
            ImportOptions {
                included: true,
                ..Default::default()
            },
            Some(self.fields),
        )
    }
}

pub fn cover_import_matter<'a>(method: &str, path: &'a str) -> Option<&'a str> {
    if method != "POST" {
        return None;
    }
    path.trim_start_matches('/')
        .strip_prefix("matters/")?
        .strip_suffix("/cover")
        .filter(|part| !part.contains('/'))
}

pub struct DocumentUploadTarget {
    matter: String,
    raw: bool,
    included: bool,
}

impl DocumentUploadTarget {
    pub fn from_path(method: &str, path: &str, query: &str) -> Option<Self> {
        let route = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
        Self::select(method, &route, query)
    }

    fn select(method: &str, route: &[&str], query: &str) -> Option<Self> {
        let ("POST", ["matters", matter, "documents"] | ["matters", matter, "documents", "raw"]) =
            (method, route)
        else {
            return None;
        };
        Some(Self {
            matter: (*matter).to_owned(),
            raw: route.len() == 4,
            included: super::reading::lookup(query, "included").as_deref() == Some("true"),
        })
    }
}

pub struct DocumentUploadBatch {
    target: DocumentImportTarget,
    included: bool,
    raw: bool,
    files: Vec<Value>,
}

impl DocumentUploadBatch {
    pub fn into_files(self) -> impl Iterator<Item = DocumentUploadWork> {
        let Self {
            target,
            included,
            raw,
            files,
        } = self;
        files.into_iter().map(move |file| DocumentUploadWork {
            target: target.clone(),
            included,
            raw,
            file,
        })
    }
}

pub struct DocumentUploadWork {
    target: DocumentImportTarget,
    included: bool,
    raw: bool,
    file: Value,
}

impl DocumentUploadWork {
    pub fn compute(self) -> Result<PreparedDocumentImport<'static>, Reply> {
        let source_file_mtime = self.file["source_file_mtime"].as_str().unwrap_or_default();
        if self.raw {
            validate_source_timestamp(source_file_mtime)?;
        }
        let bytes = bytes(&self.file)?;
        let filename = self.file["filename"].as_str().unwrap_or_default();
        self.target.measure(
            filename.to_owned(),
            bytes,
            ImportOptions {
                password: self.file["password"].as_str(),
                acknowledge_signature_risk: self.file["acknowledge_signature_risk"] == true,
                included: self.included,
                source_file_mtime,
            },
        )
    }
}

pub struct WorkingReplacement {
    document: String,
    revision: u64,
    count: usize,
    pages: Vec<usize>,
    acknowledged: bool,
    guard: Result<(), Reply>,
    working: Result<RevisionInput, Reply>,
}

pub struct MeasuredWorkingReplacement {
    document: String,
    revision: u64,
    acknowledged: bool,
    files: DocumentFileProof,
    output: ValidatedRevisionPdf,
}

pub struct UploadedReplacement {
    work: Result<WorkingReplacement, Reply>,
    body: Value,
}

pub struct RawReplacementUpload {
    pub filename: String,
    pub fields: Value,
}

impl RawReplacementUpload {
    pub fn parse(metadata: &str) -> Result<Self, Reply> {
        let field = |key| reading::lookup(metadata, key).unwrap_or_default();
        if field("content_type").split(';').next() != Some("application/octet-stream") {
            return Err(refusal(415, "Replacement requires a streamed binary file"));
        }
        let filename = field("filename").into_owned();
        replacement_suffix(&filename)?;
        let pages: Value = serde_json::from_str(&field("target_pages"))
            .map_err(|_| refusal(422, "Choose distinct working pages to replace"))?;
        let pages = replacement_pages(&pages)?;
        let revision = field("expected_revision");
        let revision = (!revision.is_empty() && revision.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| revision.parse::<u64>().ok())
            .flatten()
            .filter(|revision| *revision > 0)
            .ok_or_else(|| refusal(422, "Invalid working revision"))?;
        Ok(Self {
            filename,
            fields: json!({
                "target_pages": pages,
                "expected_revision": revision,
                "acknowledge_signature_risk": field("acknowledge_signature_risk") == "true",
            }),
        })
    }
}

fn replacement_pages(value: &Value) -> Result<Vec<usize>, Reply> {
    value
        .as_array()
        .filter(|pages| (1..=2000).contains(&pages.len()))
        .and_then(|pages| {
            pages
                .iter()
                .map(|page| page.as_u64().and_then(|index| usize::try_from(index).ok()))
                .collect::<Option<Vec<_>>>()
        })
        .filter(|pages| pages.iter().collect::<BTreeSet<_>>().len() == pages.len())
        .ok_or_else(|| refusal(422, "Choose distinct working pages to replace"))
}

impl UploadedReplacement {
    pub fn compute(self) -> Result<MeasuredWorkingReplacement, Reply> {
        let work = self.work?;
        let input = bytes(&self.body)?;
        let filename = self.body["filename"].as_str().unwrap_or_default();
        work.compute(filename, &input)
    }
}

impl WorkingReplacement {
    pub fn compute(
        self,
        filename: &str,
        input: &[u8],
    ) -> Result<MeasuredWorkingReplacement, Reply> {
        let (source, source_count) = normalized(filename, input)?;
        if source_count != self.pages.len() {
            return Err(refusal(422,format!("Replacement has {source_count} pages; select exactly {source_count} working pages")));
        }
        self.guard?;
        if self.pages.iter().any(|&index| index >= self.count) {
            return Err(refusal(
                422,
                "Select existing pages using zero-based page indexes",
            ));
        }
        let (working, files) = self.working?.read(|| false)?;
        let replacements = self
            .pages
            .into_iter()
            .enumerate()
            .map(|(src, target)| (target, src))
            .collect::<HashMap<_, _>>();
        let selections = (0..self.count)
            .map(|page| replacements.get(&page).map_or((0, page), |src| (1, *src)))
            .collect::<Vec<_>>();
        let bytes = assembly::assemble(working, Some(source), &selections)
            .map_err(|error| refusal(422, error))?;
        let output = ValidatedRevisionPdf::prepare(bytes, self.count as u64)?;
        Ok(MeasuredWorkingReplacement {
            document: self.document,
            revision: self.revision,
            acknowledged: self.acknowledged,
            files,
            output,
        })
    }
}

pub fn uploaded_replacement_document<'a>(method: &str, path: &'a str) -> Option<&'a str> {
    if method != "POST" {
        return None;
    }
    path.strip_prefix("/documents/")?
        .strip_suffix("/pages/replace-file")
        .filter(|document| !document.contains('/'))
}

impl Store {
    pub fn inspect_replacement(filename: &str, input: &[u8]) -> Reply {
        normalized(filename, input).map_or_else(
            |error| error,
            |(_, pages)| reply(200, json!({"pages":pages})),
        )
    }

    pub fn replace_working_file(
        &mut self,
        id: &str,
        filename: &str,
        input: &[u8],
        fields: &Value,
    ) -> Reply {
        self.prepare_working_replacement(id, fields)
            .and_then(|work| work.compute(filename, input))
            .and_then(|measured| self.complete_working_replacement(measured))
            .unwrap_or_else(|error| error)
    }

    pub fn prepare_uploaded_replacement(&self, id: &str, body: Value) -> UploadedReplacement {
        let work = self.prepare_working_replacement(id, &body);
        UploadedReplacement { work, body }
    }

    pub fn complete_working_replacement(
        &mut self,
        measured: MeasuredWorkingReplacement,
    ) -> Result<Reply, Reply> {
        let MeasuredWorkingReplacement {
            document,
            revision,
            acknowledged,
            files,
            output,
        } = measured;
        let doc = self.artifact_document(&document)?.body;
        if doc["revision"] != revision {
            return Err(refusal(
                409,
                "Working PDF revision changed; reload the document",
            ));
        }
        signature_guard([&doc], acknowledged, "transforming this working copy")?;
        let provenance = doc["ocr_provenance_json"]
            .as_str()
            .unwrap_or("[]")
            .to_owned();
        self.publish_measured_revision(
            &document,
            revision,
            Some(output),
            Some(("modified".into(), provenance)),
            super::annotations::Retention::None,
            files,
            |con, _| super::redaction_records::invalidate(con, &document),
        )
        .map_err(|_| refusal(500, "Working-copy replacement could not be committed"))
    }

    pub fn capture_cover_import(&self, id: &str) -> Result<CoverImportWork, Reply> {
        let target = self.capture_document_import(id)?;
        if !target.protected {
            return Err(unsupported());
        }
        let matter = document_metadata::matter(&self.con, id)
            .map_err(|error| refusal(500, error.to_string()))?
            .ok_or_else(|| refusal(404, "Matter not found"))?;
        Ok(CoverImportWork {
            target,
            fields: cover_fields(&matter),
        })
    }

    pub fn complete_cover_import(&mut self, prepared: PreparedDocumentImport<'_>) -> Reply {
        let mut result = self.complete_document_import(prepared);
        if result.status == 201 {
            result.body = result.body[0].take();
        }
        result
    }

    pub fn capture_document_upload(
        &self,
        target: DocumentUploadTarget,
        mut body: Value,
    ) -> Result<DocumentUploadBatch, Reply> {
        let DocumentUploadTarget {
            matter,
            raw,
            included,
        } = target;
        let target = self.capture_document_import(&matter)?;
        if !target.protected {
            return Err(unsupported());
        }
        let files = if raw {
            vec![body]
        } else {
            match body["files"].take() {
                Value::Array(files) if (1..=20).contains(&files.len()) => files,
                _ => return Err(refusal(422, "Import 1–20 files per request")),
            }
        };
        Ok(DocumentUploadBatch {
            target,
            included,
            raw,
            files,
        })
    }

    pub(super) fn ingest_request(
        &mut self,
        method: &str,
        route: &[&str],
        query: &str,
        body: &Value,
    ) -> rusqlite::Result<Option<Reply>> {
        if self.key.is_some() {
            if let Some(target) = DocumentUploadTarget::select(method, route, query) {
                let batch = match self.capture_document_upload(target, body.clone()) {
                    Ok(batch) => batch,
                    Err(error) => return Ok(Some(error)),
                };
                let mut imported = Vec::new();
                for file in batch.into_files() {
                    let result = match file.compute() {
                        Ok(prepared) => self.complete_document_import(prepared),
                        Err(error) => return Ok(Some(error)),
                    };
                    if result.status != 201 {
                        return Ok(Some(result));
                    }
                    if let Value::Array(items) = result.body {
                        imported.extend(items);
                    }
                }
                return Ok(Some(reply(201, json!(imported))));
            }
        }
        let result = match (method, route) {
            ("POST", ["matters", id, "cover"]) if self.key.is_some() => self
                .capture_cover_import(id)
                .and_then(CoverImportWork::compute)
                .map_or_else(|error| error, |work| self.complete_cover_import(work)),
            ("POST", ["documents", id, "pages", "replace-file"]) if self.key.is_some() => self
                .prepare_uploaded_replacement(id, body.clone())
                .compute()
                .and_then(|measured| self.complete_working_replacement(measured))
                .unwrap_or_else(|error| error),
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    pub fn prepare_working_replacement(
        &self,
        id: &str,
        body: &Value,
    ) -> Result<WorkingReplacement, Reply> {
        if self.key.is_none() {
            return Err(refusal(423, "Unlock the protected Rust store first"));
        }
        let pages = replacement_pages(&body["target_pages"])?;
        let revision = body["expected_revision"]
            .as_u64()
            .filter(|revision| *revision > 0)
            .ok_or_else(|| refusal(422, "Invalid working revision"))?;
        let doc = self.artifact_document(id)?;
        if doc.body["revision"] != revision {
            return Err(refusal(
                409,
                "Working PDF revision changed; reload the document",
            ));
        }
        let count = doc.body["page_count"]
            .as_u64()
            .ok_or_else(|| refusal(409, "Invalid working page count"))?
            as usize;
        let acknowledged = body["acknowledge_signature_risk"] == true;
        let guard = signature_guard([&doc.body], acknowledged, "transforming this working copy");
        let working = RevisionInput::capture(self, &doc.body, id, revision);
        Ok(WorkingReplacement {
            document: id.to_owned(),
            revision,
            count,
            pages,
            acknowledged,
            guard,
            working,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_replacement_validates_reference_headers_and_preserves_page_order() {
        let metadata = "content_type=application%2Foctet-stream&filename=Synthetic+caf%C3%A9.PDF&+                        target_pages=%5B2%2C0%5D&expected_revision=002&acknowledge_signature_risk=TRUE";
        let parsed = RawReplacementUpload::parse(metadata).unwrap();
        assert_eq!(parsed.filename, "Synthetic café.PDF");
        assert_eq!(
            parsed.fields,
            json!({"expected_revision":2,"target_pages":[2,0],"acknowledge_signature_risk":false})
        );
        for (from, to, status, detail) in [
            (
                "application%2Foctet-stream",
                "application%2Fpdf",
                415,
                "Replacement requires a streamed binary file",
            ),
            (
                "Synthetic+caf%C3%A9.PDF",
                ".PDF",
                415,
                "Choose a PDF, PNG or JPEG file",
            ),
            (
                "%5B2%2C0%5D",
                "%5B0%2C0%5D",
                422,
                "Choose distinct working pages to replace",
            ),
            (
                "%5B2%2C0%5D",
                "%5Btrue%5D",
                422,
                "Choose distinct working pages to replace",
            ),
            ("002", "2.0", 422, "Invalid working revision"),
        ] {
            let response = RawReplacementUpload::parse(&metadata.replace(from, to))
                .err()
                .unwrap();
            assert_eq!(response.status, status);
            assert_eq!(response.body["detail"], detail);
        }
    }

    #[test]
    fn raw_import_source_timestamp_accepts_reference_iso_forms() {
        for valid in [
            "",
            "2026-11-21",
            "2026-11-21T10:05:06.123Z",
            "20261121☀101501.3",
            "2026-W01-3T13:42,5",
            "2026-W01-12:00",
            "2026-11-21T12+05:15:04,3",
        ] {
            assert!(validate_source_timestamp(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "yesterday",
            "2026-02-30",
            "2026-11-21T24:00",
            "2026-11-21T01:01+24:00",
            "2026-11-21T12:60:00",
            "2026-11-21T12:12:14.012345678901234567890Z",
        ] {
            assert_eq!(
                validate_source_timestamp(invalid).unwrap_err().body["detail"],
                "Invalid source file timestamp",
                "{invalid}"
            );
        }
    }

    #[test]
    fn upload_batch_keeps_prior_import_when_the_next_pdf_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let mut vault =
            crate::vault_store::VaultStore::open(&directory.path().join("vault")).unwrap();
        let setup = vault.request(
            "POST",
            "/vault/setup",
            json!({"password":"Synthetic upload batch password"}),
        );
        assert_eq!(setup.status, 200, "{setup:?}");
        let matter = vault.request("POST", "/matters", json!({"name":"Upload batch fixture"}));
        let id = matter.body["id"].as_str().unwrap();
        let path = format!("/matters/{id}/documents");
        let first = include_bytes!("../../../samples/rust_pdf_corpus/unsigned.pdf");
        let result = vault.request(
            "POST",
            &format!("{path}?included=true"),
            json!({
                "files":[
                    {"filename":"first.pdf","data":STANDARD.encode(first)},
                    {"filename":"broken.pdf","data":STANDARD.encode(b"not a PDF")},
                    {"filename":"never-imported.pdf","data":STANDARD.encode(first)},
                ]
            }),
        );
        assert_eq!(result.status, 422, "{result:?}");
        let listed = vault.request("GET", &path, Value::Null);
        let docs = listed.body.as_array().unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0]["original_name"], "first.pdf");
        assert_eq!(docs[0]["bundle_included"], 1);
        assert_eq!(docs[0]["source_sha256"], sha256_hex(first));
    }
}
