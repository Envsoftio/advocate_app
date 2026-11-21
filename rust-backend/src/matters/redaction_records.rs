//! Revision-bound destructive redactions and immediate restoration.
use super::{
    artifacts::{refusal, Artifact, PdfProof, PdfSource},
    document_output::destructive_redaction,
    document_verification::{DocumentFileProof, RevisionInput},
    page_tools::{
        assembly,
        request::{at_least, fraction, value_error, Fields},
        signature_guard,
    },
    revisions::{self, ValidatedRevisionPdf},
    *,
};
use pdf_oxide::PdfDocument;
use std::ops::Bound::{Excluded, Included};

pub(super) fn migrate(con: &Connection) -> rusqlite::Result<()> {
    con.execute_batch("CREATE TABLE rust_redactions(id TEXT PRIMARY KEY,document_id TEXT NOT NULL,body TEXT NOT NULL);
        CREATE INDEX rust_redactions_document ON rust_redactions(document_id);
        PRAGMA user_version=13;")
}

pub(super) fn records(con: &Connection, id: &str) -> rusqlite::Result<Vec<Value>> {
    let mut stmt = con.prepare(
        "SELECT body FROM rust_redactions WHERE document_id=? ORDER BY json_extract(body,'$.created_at'),id",
    )?;
    let rows = stmt.query_map([id], |row| decode(&row.get::<_, String>(0)?))?;
    rows.collect()
}

pub(super) fn restore_record(con: &Connection, id: &str, record: &Value) -> rusqlite::Result<()> {
    con.execute("INSERT INTO rust_redactions VALUES (?,?,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body",
        params![record["id"].as_str(),id,record.to_string()])?;
    Ok(())
}

pub(super) fn invalidate(con: &Connection, id: &str) -> rusqlite::Result<()> {
    con.execute(
        "UPDATE rust_redactions SET body=json_set(body,'$.structure_changed',1)
        WHERE document_id=? AND json_extract(body,'$.active')=1",
        [id],
    )?;
    Ok(())
}

pub(super) fn replay(con: &Connection, id: &str, revision: u64) -> rusqlite::Result<()> {
    con.execute(
        "UPDATE rust_redactions SET body=json_set(body,'$.active',
        CASE WHEN json_extract(body,'$.before_revision')=? THEN 0
        WHEN json_extract(body,'$.after_revision')=? THEN 1
        WHEN json_extract(body,'$.removed_revision')-1=? THEN 1
        WHEN json_extract(body,'$.removed_revision')=? THEN 0
        ELSE json_extract(body,'$.active') END) WHERE document_id=?",
        params![revision, revision, revision, revision, id],
    )?;
    Ok(())
}

const COORDINATES: [&str; 4] = ["rect_x", "rect_y", "rect_width", "rect_height"];
const INVALID_STRUCTURE: &str =
    "Page structure changed after this redaction; selective restoration cannot preserve later page edits safely";
const MISSING_REVISION_FILE: &str =
    "The private working revision needed to restore this area is missing";

struct Region {
    page: u64,
    fractions: [f64; 4],
}

impl Region {
    fn bounds(&self, visible: assembly::Geometry) -> [f32; 4] {
        let (width, height) = visible.display_size();
        let [x, y, w, h] = self.fractions;
        assembly::transform_box(
            visible.display_matrix(),
            [
                width * x,
                height * (1.0 - y - h),
                width * (x + w),
                height * (1.0 - y),
            ],
        )
        .map(|value| value as f32)
    }
}

fn parse_request(
    body: &Value,
    restoring: bool,
    order: &crate::RequestOrder,
) -> Result<(u64, bool, Option<Region>), Reply> {
    let mut fields = Fields::new(body)?;
    let page = if restoring {
        None
    } else {
        fields.field("page_index", None, |input, loc| {
            at_least(input, loc, false, 0)
        })
    };
    let expected = fields.revision("expected_revision", true, false);
    let allowed = ["acknowledge_signature_risk", "expected_revision"];
    let region = if restoring {
        fields.extras(&allowed, order);
        None
    } else {
        let [x, y, width, height] = COORDINATES.map(|key| {
            fields.field(key, None, |input, loc| {
                let limits = if matches!(key, "rect_x" | "rect_y") {
                    (Included(0.0), Excluded(1.0))
                } else {
                    (Excluded(0.0), Included(1.0))
                };
                fraction(input, loc, limits)
            })
        });
        fields.extras(
            &[
                "acknowledge_signature_risk",
                "expected_revision",
                "page_index",
                "rect_x",
                "rect_y",
                "rect_width",
                "rect_height",
            ],
            order,
        );
        page.zip(x)
            .zip(y)
            .zip(width)
            .zip(height)
            .map(|((((page, x), y), width), height)| Region {
                page,
                fractions: [x, y, width, height],
            })
    };
    let (expected, region) = fields.finish(expected.map(|expected| (expected, region)))?;
    if let Some(Region {
        fractions: [x, y, width, height],
        ..
    }) = &region
    {
        if x + width > 1.0 || y + height > 1.0 {
            return Err(reply(
                422,
                json!({"detail": [value_error(
                    json!(["body"]), body, "Redaction region must stay within the displayed page",
                )]}),
            ));
        }
    }
    Ok((expected, body["acknowledge_signature_risk"] == true, region))
}

fn apply(
    id: &str,
    expected: u64,
    region: Region,
    bytes: Vec<u8>,
) -> Result<(Vec<u8>, Value), Reply> {
    let pdf = PdfDocument::from_bytes(bytes.clone())
        .map_err(|_| refusal(422, "Unable to open working PDF"))?;
    let page = region.page as usize;
    let frame = assembly::DisplayFrame::new(&pdf, page).map_err(|error| refusal(422, error))?;
    let bounds = region.bounds(frame.visible);
    let mut expanded = bounds.map(f64::from);
    for span in pdf
        .extract_spans(page)
        .map_err(|_| refusal(422, "Cannot inspect the redaction region"))?
    {
        let bounds = super::text_edit::span_user_bbox(frame.geometry, &span);
        if super::text_edit::intersects(bounds, expanded) {
            expanded = [
                expanded[0].min(bounds[0]),
                expanded[1].min(bounds[1]),
                expanded[2].max(bounds[2]),
                expanded[3].max(bounds[3]),
            ];
        }
    }
    let shape =
        super::text_edit::displayed(frame.visible, expanded).map(|point| point.clamp(0.0, 1.0));
    let candidate =
        destructive_redaction(bytes, &[(page, bounds)]).map_err(|error| refusal(422, error))?;
    let mut record = json!({"page_index": region.page});
    for (name, value) in COORDINATES.into_iter().zip(region.fractions) {
        record[name] = json!(value);
    }
    record["id"] = json!(uuid::Uuid::new_v4().simple().to_string());
    record["document_id"] = json!(id);
    record["before_revision"] = json!(expected);
    record["after_revision"] = json!(expected + 1);
    record["active"] = json!(1);
    record["removed_revision"] = Value::Null;
    record["structure_changed"] = json!(0);
    record["created_at"] = json!(now());
    record["restore_x"] = json!(shape[0]);
    record["restore_y"] = json!(shape[1]);
    record["restore_width"] = json!(shape[2] - shape[0]);
    record["restore_height"] = json!(shape[3] - shape[1]);
    Ok((candidate, record))
}

struct RedactionRestore {
    artifact: Artifact,
    source: PdfSource,
    history: Vec<Value>,
    record: Value,
    page: usize,
    footprint: Value,
    later: Vec<Value>,
}

fn prepare_restore(
    store: &Store,
    id: &str,
    history: Vec<Value>,
    record: Value,
) -> Result<RedactionRestore, Reply> {
    let redaction = &record["id"];
    let requested = record["before_revision"]
        .as_u64()
        .ok_or_else(|| refusal(422, MISSING_REVISION_FILE))?;
    let retained = revisions::record(&store.con, id, requested)
        .map_err(|_| refusal(500, "Unable to load retained revision"))?
        .ok_or_else(|| {
            refusal(
                422,
                "The working revision needed to restore this redaction is missing",
            )
        })?;
    let page = record["page_index"]
        .as_u64()
        .ok_or_else(|| refusal(422, "Invalid redaction page"))?;
    let mut footprint = json!({});
    for (name, key) in [
        ("rect_x", "restore_x"),
        ("rect_y", "restore_y"),
        ("rect_width", "restore_width"),
        ("rect_height", "restore_height"),
    ] {
        footprint[name] = record[key].clone();
    }
    let rectangle = |item: &Value| {
        let x = item["rect_x"].as_f64().unwrap_or_default();
        let y = item["rect_y"].as_f64().unwrap_or_default();
        [
            x,
            y,
            x + item["rect_width"].as_f64().unwrap_or_default(),
            y + item["rect_height"].as_f64().unwrap_or_default(),
        ]
    };
    let conflicting = history
        .iter()
        .filter(|item| &item["id"] != redaction && item["page_index"] == page)
        .filter(|item| super::text_edit::intersects(rectangle(item), rectangle(&footprint)))
        .collect::<Vec<_>>();
    if conflicting.iter().any(|item| {
        item["active"] == 0 && item["after_revision"].as_u64() < record["after_revision"].as_u64()
    }) {
        return Err(refusal(
            422,
            "An earlier overlapping redaction was already removed; this area cannot be restored selectively without changing that content",
        ));
    }
    let later = conflicting
        .into_iter()
        .filter(|item| {
            item["active"] == 1
                && item["after_revision"].as_u64() > record["after_revision"].as_u64()
        })
        .cloned()
        .collect::<Vec<_>>();
    let artifact = Artifact::Retained {
        revision: requested,
        sha256: retained.hash,
    };
    let source = store.pdf_source(id, &artifact)?;
    Ok(RedactionRestore {
        artifact,
        source,
        history,
        record,
        page: page as usize,
        footprint,
        later,
    })
}

pub struct RedactionTarget {
    document: String,
    restore: Option<String>,
}

impl RedactionTarget {
    pub fn from_path(method: &str, path: &str) -> Option<Self> {
        let route = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
        Self::select(method, &route)
    }

    fn select(method: &str, route: &[&str]) -> Option<Self> {
        let (document, restore) = match (method, route) {
            ("POST", ["documents", id, "pages", "redact"]) => (id, None),
            ("POST", ["documents", id, "pages", "redactions", redaction, "remove"]) => {
                (id, Some((*redaction).to_owned()))
            }
            _ => return None,
        };
        Some(Self {
            document: (*document).to_owned(),
            restore,
        })
    }
}

enum RedactionAction {
    Apply(Region),
    Restore(RedactionRestore),
}

pub struct RedactionWork {
    document: String,
    expected: u64,
    acknowledged: bool,
    working: RevisionInput,
    action: RedactionAction,
}

struct RestoredRedactionInput {
    artifact: Artifact,
    source: PdfProof,
    history: Vec<Value>,
}

pub struct MeasuredRedaction {
    document: String,
    expected: u64,
    acknowledged: bool,
    files: DocumentFileProof,
    output: ValidatedRevisionPdf,
    record: Value,
    retained: Option<RestoredRedactionInput>,
}

impl RedactionWork {
    pub fn compute(self) -> Result<MeasuredRedaction, Reply> {
        let Self {
            document,
            expected,
            acknowledged,
            working,
            action,
        } = self;
        let pages = working.pages;
        let (files, bytes, record, retained) = match action {
            RedactionAction::Apply(region) => {
                let (current, files) = working.read(|| false)?;
                let (bytes, record) = apply(&document, expected, region, current)?;
                (files, bytes, record, None)
            }
            RedactionAction::Restore(RedactionRestore {
                artifact,
                source,
                history,
                mut record,
                page,
                footprint,
                later,
            }) => {
                let (bytes, source) = source.read_with_proof(|| false).map_err(|error| {
                    if error.body["detail"] == "Retained revision file is unavailable" {
                        refusal(422, MISSING_REVISION_FILE)
                    } else {
                        error
                    }
                })?;
                let (current, files) = working.read(|| false)?;
                let bytes = super::document_output::restore_redacted_area(
                    current, bytes, page, &footprint, &later,
                )
                .map_err(|error| refusal(422, error))?;
                record["active"] = json!(0);
                record["removed_revision"] = json!(expected + 1);
                let retained = RestoredRedactionInput {
                    artifact,
                    source,
                    history,
                };
                (files, bytes, record, Some(retained))
            }
        };
        let output = ValidatedRevisionPdf::prepare(bytes, pages)?;
        Ok(MeasuredRedaction {
            document,
            expected,
            acknowledged,
            files,
            output,
            record,
            retained,
        })
    }
}

impl Store {
    pub fn prepare_redaction(
        &self,
        target: RedactionTarget,
        body: Value,
        order: &crate::RequestOrder,
    ) -> Result<RedactionWork, Reply> {
        if self.key.is_none() {
            return Err(refusal(423, "Unlock the protected Rust store first"));
        }
        let RedactionTarget { document, restore } = target;
        let (expected, acknowledged, region) = parse_request(&body, restore.is_some(), order)?;
        let doc = self.artifact_document(&document)?.body;
        let to_restore = if let Some(id) = restore {
            let history = records(&self.con, &document)
                .map_err(|_| refusal(500, "Unable to load redaction records"))?;
            let record = history
                .iter()
                .find(|record| record["id"] == id && record["active"] == 1)
                .cloned()
                .ok_or_else(|| refusal(404, "Applied redaction not found"))?;
            if record["structure_changed"] == 1 {
                return Err(refusal(422, INVALID_STRUCTURE));
            }
            Some((history, record))
        } else {
            None
        };
        if doc["revision"] != expected {
            return Err(refusal(
                409,
                "Working PDF revision changed; reload the document",
            ));
        }
        let action = if let Some((history, record)) = to_restore {
            let work = prepare_restore(self, &document, history, record)?;
            signature_guard([&doc], acknowledged, "transforming this working copy")?;
            RedactionAction::Restore(work)
        } else {
            signature_guard([&doc], acknowledged, "transforming this working copy")?;
            let region = region.expect("validated redaction region");
            if Some(region.page) >= doc["page_count"].as_u64() {
                return Err(refusal(422, "Invalid page number"));
            }
            RedactionAction::Apply(region)
        };
        let working = RevisionInput::capture(self, &doc, &document, expected)?;
        Ok(RedactionWork {
            document,
            expected,
            acknowledged,
            working,
            action,
        })
    }

    pub fn complete_redaction(&mut self, measured: MeasuredRedaction) -> Result<Reply, Reply> {
        let MeasuredRedaction {
            document,
            expected,
            acknowledged,
            files,
            output,
            record,
            retained,
        } = measured;
        let doc = self.artifact_document(&document)?.body;
        if doc["revision"] != expected {
            return Err(refusal(
                409,
                "Working PDF revision changed; reload the document",
            ));
        }
        signature_guard([&doc], acknowledged, "transforming this working copy")?;
        if let Some(retained) = retained {
            let current = self.pdf_source(&document, &retained.artifact)?;
            let history = records(&self.con, &document)
                .map_err(|_| refusal(500, "Unable to load redaction records"))?;
            if !retained.source.matches(&current) || history != retained.history {
                return Err(refusal(
                    409,
                    "Redaction history changed; reload the document",
                ));
            }
            retained.source.check_current()?;
        }
        let provenance = doc["ocr_provenance_json"]
            .as_str()
            .unwrap_or("[]")
            .to_owned();
        self.publish_measured_revision(
            &document,
            expected,
            Some(output),
            Some(("modified".into(), provenance)),
            super::annotations::Retention::Pdf,
            files,
            |con, _| restore_record(con, &document, &record),
        )
        .map_err(|_| refusal(500, "Rust matter transaction failed; no changes committed"))
    }

    pub(super) fn redaction_request(
        &mut self,
        method: &str,
        route: &[&str],
        body: &Value,
        order: &crate::RequestOrder,
    ) -> rusqlite::Result<Option<Reply>> {
        if let ("GET", ["documents", id, "pages", "redactions"]) = (method, route) {
            let doc = match self.artifact_document(id) {
                Ok(doc) => doc,
                Err(error) => return Ok(Some(error)),
            };
            let revision = doc.body["revision"].as_u64().unwrap_or(0);
            let records = records(&self.con, id)?
                .into_iter()
                .filter(|record| {
                    record["active"] == 1
                        && record["after_revision"]
                            .as_u64()
                            .is_some_and(|version| version <= revision)
                })
                .collect::<Vec<_>>();
            return Ok(Some(reply(200, json!(records))));
        }
        let Some(target) = RedactionTarget::select(method, route) else {
            return Ok(None);
        };
        let result = self
            .prepare_redaction(target, body.clone(), order)
            .and_then(RedactionWork::compute)
            .and_then(|measured| self.complete_redaction(measured));
        Ok(Some(result.unwrap_or_else(|error| error)))
    }
}
