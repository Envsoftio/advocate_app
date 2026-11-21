//! Durable bundle membership and deterministic input identity.
use super::{artifacts, document_metadata, preparation, *};
use crate::planner;
use serde::Deserialize;
use serde_json::Map;
use std::collections::{BTreeSet, HashMap};

mod destinations;
mod settings;
pub(super) use settings::replay as replay_settings;

pub(super) fn history_kind(kind: &str) -> bool {
    destinations::history_kind(kind)
}

const DEFAULT_MARKS: &str = include_str!("bundle_inputs/output_defaults.json");
const LEGACY_OPTIONAL_MARKS: &[&str] = &[
    "volume_max_bytes",
    "volume_splits",
    "processing_quality",
    "cleanup_metadata",
    "print_layout",
    "office_docx",
    "office_xlsx",
    "office_pptx",
];

pub(super) fn migrate(con: &Connection) -> rusqlite::Result<()> {
    con.execute_batch(
        "CREATE TABLE bundle_membership_state (matter_id TEXT PRIMARY KEY, revision INTEGER NOT NULL);
         CREATE TABLE bundle_destinations (
             kind TEXT NOT NULL, id TEXT NOT NULL, matter_id TEXT NOT NULL,
             position INTEGER NOT NULL, body TEXT NOT NULL, PRIMARY KEY(kind,id));
         CREATE INDEX bundle_destinations_matter ON bundle_destinations(matter_id, kind, position);
         CREATE TABLE bundle_settings (matter_id TEXT PRIMARY KEY, marks TEXT NOT NULL, preset TEXT NOT NULL);
         CREATE TABLE output_presets (
             id TEXT PRIMARY KEY, name TEXT NOT NULL COLLATE NOCASE, version INTEGER NOT NULL,
             settings TEXT NOT NULL, created_at TEXT NOT NULL, UNIQUE(name,version));
         PRAGMA user_version=6;",
    )
}

#[derive(Deserialize)]
struct Membership {
    included_ids: Vec<String>,
    expected_snapshot: String,
}

#[derive(Clone, Copy, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Preview {
    pub(super) max_pages_per_volume: usize,
    pub(super) keep_groups_intact: bool,
}

pub(super) fn profile(matter: &Value) -> rusqlite::Result<Value> {
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../../../backend/app/rules/profiles.json"))
            .expect("embedded profiles");
    profiles
        .into_iter()
        .find(|profile| profile["id"] == matter["court_profile"])
        .ok_or(rusqlite::Error::InvalidQuery)
}

pub(super) fn backup_settings(con: &Connection, id: &str) -> rusqlite::Result<(Value, Value)> {
    settings::load_state(con, id)
}

pub(super) fn backup_destinations(
    con: &Connection,
    id: &str,
) -> rusqlite::Result<(Vec<Value>, Vec<Value>)> {
    Ok((
        destinations::load(con, id, destinations::Kind::Bookmark)?,
        destinations::load(con, id, destinations::Kind::Link)?,
    ))
}

pub(super) fn parse_marks(body: &Value) -> Result<Value, Reply> {
    settings::parse_marks(body)
}

pub(super) fn validate_portable_preset(preset: &Value) -> Result<(), String> {
    if preset.is_null() {
        return Ok(());
    }
    if !preset["id"].is_string()
        || preset["version"]
            .as_u64()
            .is_none_or(|version| version == 0)
        || preset["name"] != preset["settings"]["name"]
    {
        return Err("Invalid output preset snapshot".into());
    }
    settings::preset_settings(&preset["settings"]).map_err(|error| {
        error.body["detail"]
            .as_str()
            .unwrap_or("Invalid output preset snapshot")
            .to_owned()
    })?;
    Ok(())
}

pub(super) fn validate_portable_destinations(
    bookmarks: &[Value],
    links: &[Value],
) -> Result<(), String> {
    for (kind, entries) in [
        (destinations::Kind::Bookmark, bookmarks),
        (destinations::Kind::Link, links),
    ] {
        let entries = entries
            .iter()
            .cloned()
            .map(|mut entry| {
                if let Some(fields) = entry.as_object_mut() {
                    fields.remove("position");
                }
                entry
            })
            .collect::<Vec<_>>();
        destinations::parse(kind, &json!({"entries":entries})).map_err(|reply| {
            reply.body["detail"]
                .as_str()
                .unwrap_or("Invalid portable destination")
                .to_owned()
        })?;
    }
    Ok(())
}

pub(super) fn restore_backup(
    con: &Connection,
    id: &str,
    marks: &Value,
    preset: &Value,
    bookmarks: &[Value],
    links: &[Value],
) -> Result<(), String> {
    let marks = settings::parse_marks(marks).map_err(|_| "Invalid output marks in backup")?;
    if !preset.is_null() && !preset.is_object() {
        return Err("Invalid output preset in backup".into());
    }
    // Refuse invalid destinations rather than silently removing them.
    for item in bookmarks {
        if item["title"]
            .as_str()
            .is_none_or(|title| title.is_empty() || title.chars().count() > 200)
            || item["page_index"].as_u64().is_none()
            || item["target_revision"]
                .as_u64()
                .is_none_or(|revision| revision == 0)
            || item["document_id"]
                .as_str()
                .is_none_or(|id| !artifacts::valid_id(id))
        {
            return Err("Invalid manual bookmark in backup".into());
        }
    }
    for item in links {
        let url = item["uri"].as_str().unwrap_or_default();
        if !(item["kind"] == "internal" && url.is_empty() && item["target_document_id"].is_string()
            || item["kind"] == "external"
                && ["https://", "http://", "mailto:"]
                    .iter()
                    .any(|prefix| url.starts_with(prefix))
                && !url.chars().any(char::is_whitespace))
        {
            return Err("Invalid reviewed link in backup".into());
        }
    }
    settings::save(con, id, &marks, preset).map_err(|error| error.to_string())?;
    destinations::write(con, id, destinations::Kind::Bookmark, bookmarks)
        .map_err(|error| error.to_string())?;
    destinations::write(con, id, destinations::Kind::Link, links)
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn output_marks(con: &Connection, id: &str) -> rusqlite::Result<Value> {
    settings::load_state(con, id).map(|(marks, _)| marks)
}

pub(super) fn resolved_destinations(
    con: &Connection,
    id: &str,
    docs: &[Value],
) -> rusqlite::Result<(Vec<Value>, Vec<Value>)> {
    let bookmarks = destinations::Kind::Bookmark;
    let links = destinations::Kind::Link;
    Ok((
        destinations::resolve(&destinations::load(con, id, bookmarks)?, docs, bookmarks),
        destinations::resolve(&destinations::load(con, id, links)?, docs, links),
    ))
}

pub(super) fn select(value: &Value, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .map(|&key| (key.to_owned(), value[key].clone()))
            .collect(),
    )
}

pub(super) fn marks() -> Value {
    serde_json::from_str(DEFAULT_MARKS).expect("embedded output marks")
}

pub(super) struct Inputs {
    owner: Value,
    documents: Vec<Value>,
    bookmarks: Vec<Value>,
    links: Vec<Value>,
    sizes: HashMap<String, u64>,
    filing_review: Value,
    annotations: Vec<Value>,
    output_marks: Value,
    preset: Value,
}

pub(super) struct BundleInputWork {
    inputs: Inputs,
    sources: Vec<(
        String,
        Result<super::document_verification::DocumentFiles, String>,
    )>,
}

impl BundleInputWork {
    pub(super) fn inputs(&self) -> &Inputs {
        &self.inputs
    }

    pub(super) fn verify(
        mut self,
    ) -> Result<(Inputs, Vec<super::document_verification::DocumentFileProof>), Reply> {
        let mut proofs = Vec::with_capacity(self.sources.len());
        let invalid = |detail: String| artifacts::refusal(409, detail);
        for (id, source) in self.sources {
            let (size, proof) = source
                .and_then(super::document_verification::DocumentFiles::authenticate)
                .map_err(invalid)?;
            self.inputs.sizes.insert(id, size);
            proofs.push(proof);
        }
        Ok((self.inputs, proofs))
    }
}

impl Inputs {
    pub(super) fn read(
        con: &Connection,
        root: &Path,
        key: Option<&DataKey>,
        matter_id: &str,
    ) -> rusqlite::Result<Result<Self, Reply>> {
        Ok(Self::capture(con, root, key, matter_id)?
            .and_then(BundleInputWork::verify)
            .map(|(input, _)| input))
    }

    pub(super) fn capture(
        con: &Connection,
        root: &Path,
        key: Option<&DataKey>,
        matter_id: &str,
    ) -> rusqlite::Result<Result<BundleInputWork, Reply>> {
        Ok(Self::metadata(con, matter_id)?.map(|inputs| {
            let sources = inputs
                .documents
                .iter()
                .map(|doc| {
                    let id = doc["id"].as_str().expect("validated document identity");
                    (
                        id.to_owned(),
                        super::document_verification::DocumentFiles::capture(
                            con, root, key, doc, id,
                        ),
                    )
                })
                .collect();
            BundleInputWork { inputs, sources }
        }))
    }

    pub(super) fn metadata(
        con: &Connection,
        matter_id: &str,
    ) -> rusqlite::Result<Result<Self, Reply>> {
        let Some(owner) = document_metadata::matter(con, matter_id)? else {
            return Ok(Err(artifacts::refusal(404, "Matter not found")));
        };
        let mut documents = Vec::new();
        for (id, matter, raw, archived_at) in Store::document_rows(con, matter_id, false)? {
            let doc = decode(&raw)?;
            if let Err(detail) =
                Store::check_document_identity(&doc, &id, &matter, archived_at.as_deref())
            {
                return Ok(Err(artifacts::refusal(409, detail)));
            }
            documents.push(doc);
        }
        let Some(filing_review) = preparation::filing_review(con, matter_id)? else {
            return Ok(Err(artifacts::refusal(
                409,
                "Stored filing review is invalid; restore or correct it before building",
            )));
        };
        let bookmarks = destinations::load(con, matter_id, destinations::Kind::Bookmark)?;
        let links = destinations::load(con, matter_id, destinations::Kind::Link)?;
        let (output_marks, preset) = settings::load_state(con, matter_id)?;
        Ok(Ok(Self {
            owner,
            documents,
            bookmarks,
            links,
            sizes: HashMap::new(),
            filing_review,
            annotations: super::annotations::raw(con, matter_id)?,
            output_marks,
            preset,
        }))
    }

    pub(super) fn generation_inputs(&self) -> (&Value, Vec<&Value>, &Value) {
        (
            &self.owner,
            self.documents
                .iter()
                .filter(|document| document["bundle_included"] == 1)
                .collect(),
            &self.output_marks,
        )
    }

    pub(super) fn all_documents(&self) -> &[Value] {
        &self.documents
    }

    pub(super) fn digest(&self, con: &Connection) -> rusqlite::Result<String> {
        self.input_digest(con, false)
    }

    pub(super) fn effective_digest(&self, con: &Connection) -> rusqlite::Result<String> {
        self.input_digest(con, true)
    }

    fn input_digest(&self, con: &Connection, effective: bool) -> rusqlite::Result<String> {
        let profile = profile(&self.owner)?;
        let included: BTreeSet<_> = self
            .documents
            .iter()
            .filter(|doc| doc["bundle_included"] == 1)
            .filter_map(|doc| doc["id"].as_str())
            .collect();
        let matter = select(
            &self.owner,
            &[
                "name",
                "court_profile",
                "case_type",
                "petitioner",
                "respondent",
                "advocate",
                "case_number",
                "max_pages_per_volume",
                "keep_groups_intact",
            ],
        );
        let mut docs = Vec::new();
        for doc in &self.documents {
            if effective && !doc["id"].as_str().is_some_and(|id| included.contains(id)) {
                continue;
            }

            let mut fields = select(
                doc,
                &[
                    "id",
                    "position",
                    "title",
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
                    "page_count",
                    "revision",
                    "source_sha256",
                    "bundle_included",
                    "certificate_signature_status",
                    "certificate_signature_count",
                    "visible_signature_status",
                    "signature_status",
                ],
            );
            let working: String = con.query_row(
                "SELECT sha256 FROM document_revisions WHERE document_id=? AND revision=?",
                params![doc["id"].as_str(), doc["revision"].as_u64()],
                |row| row.get(0),
            )?;
            fields["working_sha256"] = json!(working);
            fields["original_sha256"] = doc["source_sha256"].clone();
            if effective {
                fields["position"] = json!(docs.len());
                fields["input_updated_at"] = doc["updated_at"].clone();
            }
            docs.push(fields);
        }
        let mut output_marks = self.output_marks.clone();
        let defaults = marks();
        for &key in LEGACY_OPTIONAL_MARKS {
            if output_marks[key] == defaults[key] {
                output_marks.as_object_mut().unwrap().remove(key);
            }
        }
        let included_entries = |entries: &[Value], field: &str| {
            entries
                .iter()
                .filter(|entry| {
                    !effective
                        || entry[field]
                            .as_str()
                            .is_some_and(|id| included.contains(id))
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut annotations = included_entries(&self.annotations, "document_id");
        for annotation in &mut annotations {
            if annotation["kind"] == "image" {
                let id = annotation["id"]
                    .as_str()
                    .ok_or(rusqlite::Error::InvalidQuery)?;
                let data =
                    super::annotations::asset(con, id)?.ok_or(rusqlite::Error::InvalidQuery)?;
                annotation["asset_sha256"] = json!(sha256_hex(&data));
            }
        }
        let payload = json!({
            "schema":2,"matter":matter,"documents":docs,"profile":profile,
            "manual_bookmarks":included_entries(&self.bookmarks,"document_id"),"reviewed_links":included_entries(&self.links,"source_document_id"),"output_marks":output_marks,
            "filing_review":self.filing_review,"output_preset":self.preset,"page_annotations":annotations
        });
        Ok(sha256_hex(payload.to_string().as_bytes()))
    }

    pub(super) fn membership_revision(con: &Connection, matter_id: &str) -> rusqlite::Result<u64> {
        Ok(con
            .query_row(
                "SELECT revision FROM bundle_membership_state WHERE matter_id=?",
                [matter_id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(1))
    }

    pub(super) fn membership(&self, con: &Connection, matter_id: &str) -> rusqlite::Result<Value> {
        let revision = Self::membership_revision(con, matter_id)?;
        let mut included = 0;
        let mut included_pages = 0;
        let mut reference_pages = 0;
        let documents = self
            .documents
            .iter()
            .map(|doc| {
                let pages = doc["page_count"].as_u64().unwrap_or_default();
                if doc["bundle_included"] == 1 {
                    included += 1;
                    included_pages += pages;
                } else {
                    reference_pages += pages;
                }
                select(
                    doc,
                    &[
                        "id",
                        "title",
                        "part",
                        "group_key",
                        "page_count",
                        "position",
                        "revision",
                        "bundle_included",
                    ],
                )
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "bundle_id":format!("{matter_id}:primary"),"title":"Hearing bundle","revision":revision,
            "input_snapshot":self.digest(con)?,"documents":documents,"included_documents":included,
            "included_source_pages":included_pages,"reference_documents":documents.len()-included,
            "reference_source_pages":reference_pages
        }))
    }

    fn planner_documents(
        &self,
    ) -> rusqlite::Result<(Vec<planner::Document>, HashMap<String, String>)> {
        let mut documents = Vec::new();
        let mut titles = HashMap::new();
        for doc in &self.documents {
            let id = doc["id"].as_str().ok_or(rusqlite::Error::InvalidQuery)?;
            let title = doc["title"].as_str().ok_or(rusqlite::Error::InvalidQuery)?;
            titles.insert(id.to_owned(), title.to_owned());
            if doc["bundle_included"] == 1 {
                let mut doc = doc.clone();
                doc["byte_size"] =
                    json!(self.sizes.get(id).ok_or(rusqlite::Error::InvalidQuery)?);
                documents.push(serde_json::from_value::<planner::Document>(doc).map_err(json_sql)?);
            }
        }
        Ok((documents, titles))
    }

    pub(super) fn preview_with_snapshot(
        &self,
        settings: Preview,
        snapshot: &str,
    ) -> rusqlite::Result<Reply> {
        let (documents, titles) = self.planner_documents()?;
        self.preview_map(settings, &self.output_marks, documents, titles, || {
            Ok(Some(snapshot.to_owned()))
        })
    }

    pub(super) fn composition_plan(
        &self,
        settings: Preview,
        marks: &Value,
        selected: &[Value],
    ) -> rusqlite::Result<Reply> {
        self.selected_map(settings, marks, selected, || Ok(None))
    }

    fn selected_map(
        &self,
        settings: Preview,
        marks: &Value,
        selected: &[Value],
        snapshot: impl FnOnce() -> rusqlite::Result<Option<String>>,
    ) -> rusqlite::Result<Reply> {
        let titles = self
            .documents
            .iter()
            .filter_map(|doc| {
                Some((
                    doc["id"].as_str()?.to_owned(),
                    doc["title"].as_str()?.to_owned(),
                ))
            })
            .collect::<HashMap<_, _>>();
        let documents = selected
            .iter()
            .map(|document| {
                let mut doc = document.clone();
                let id = doc["id"].as_str().ok_or(rusqlite::Error::InvalidQuery)?;
                doc["byte_size"] =
                    json!(self.sizes.get(id).ok_or(rusqlite::Error::InvalidQuery)?);
                serde_json::from_value::<planner::Document>(doc).map_err(json_sql)
            })
            .collect::<rusqlite::Result<Vec<_>>>()?;
        self.preview_map(settings, marks, documents, titles, snapshot)
    }

    fn preview_map(
        &self,
        settings: Preview,
        marks: &Value,
        documents: Vec<planner::Document>,
        titles: HashMap<String, String>,
        snapshot: impl FnOnce() -> rusqlite::Result<Option<String>>,
    ) -> rusqlite::Result<Reply> {
        let bookmarks = destinations::resolve(
            &self.bookmarks,
            &self.documents,
            destinations::Kind::Bookmark,
        )
        .into_iter()
        .map(serde_json::from_value::<planner::Bookmark>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(json_sql)?;
        let marks = serde_json::from_value(marks.clone()).map_err(json_sql)?;
        let plan = match planner::plan_bundle(
            &documents,
            &titles,
            &bookmarks,
            settings.max_pages_per_volume,
            settings.keep_groups_intact,
            &marks,
        ) {
            Ok(plan) => plan,
            Err(message) => return Ok(artifacts::refusal(422, message)),
        };
        let mut result = serde_json::to_value(plan).map_err(json_sql)?;
        if let Some(snapshot) = snapshot()? {
            result["input_snapshot"] = json!(snapshot);
            result["settings_snapshot"] = json!(settings);
        }
        Ok(reply(200, result))
    }
}

fn json_sql(error: serde_json::Error) -> rusqlite::Error {
    io_sql(std::io::Error::other(error))
}

fn bump_membership(con: &Connection, id: &str) -> rusqlite::Result<()> {
    con.execute(
        "INSERT INTO bundle_membership_state(matter_id,revision) VALUES (?,2)
         ON CONFLICT(matter_id) DO UPDATE SET revision=revision+1",
        [id],
    )?;
    Ok(())
}

fn apply(con: &Connection, docs: &mut [Value], state: &Map<String, Value>) -> rusqlite::Result<()> {
    for doc in docs {
        let id = doc["id"]
            .as_str()
            .ok_or(rusqlite::Error::InvalidQuery)?
            .to_owned();
        let included = state.get(&id).ok_or(rusqlite::Error::InvalidQuery)?;
        doc["bundle_included"] = included.clone();
        con.execute(
            "UPDATE documents SET body=? WHERE id=?",
            params![doc.to_string(), id],
        )?;
    }
    Ok(())
}

fn membership_state(docs: &[Value]) -> Map<String, Value> {
    docs.iter()
        .map(|doc| {
            (
                doc["id"]
                    .as_str()
                    .expect("verified document identifier")
                    .to_owned(),
                doc["bundle_included"].clone(),
            )
        })
        .collect()
}

pub(super) fn replay(
    con: &Connection,
    root: &Path,
    key: Option<&DataKey>,
    id: &str,
    kind: &str,
    target: &Value,
    expected: &Value,
) -> rusqlite::Result<Option<Reply>> {
    if kind != "bundle_membership" {
        return destinations::replay(con, id, kind, target, expected);
    }
    let mut docs = match Store::verified_documents(con, root, key, id, false)? {
        Ok(docs) => docs,
        Err(refusal) => return Ok(Some(refusal)),
    };
    let state = membership_state(&docs);
    let invalid = || {
        Some(artifacts::refusal(
            409,
            "Bundle membership changed; this edit cannot be restored",
        ))
    };
    let Some(target) = target.as_object() else {
        return Ok(invalid());
    };
    if expected.as_object() != Some(&state)
        || !state.keys().eq(target.keys())
        || target
            .values()
            .any(|value| !matches!(value.as_i64(), Some(0 | 1)))
    {
        return Ok(invalid());
    }
    apply(con, &mut docs, target)?;
    bump_membership(con, id)?;
    Ok(None)
}

enum PreviewOutput {
    Bundle,
    Volume { marks: Value, expected: String },
}

const VOLUME_INPUT_CHANGED: &str = "Matter changed; reload settings and review your changes";

pub struct BundlePreviewWork {
    root: PathBuf,
    matter: String,
    inputs: BundleInputWork,
    snapshot: String,
    settings: Preview,
    output: PreviewOutput,
}

pub struct MeasuredBundlePreview {
    root: PathBuf,
    matter: String,
    snapshot: String,
    response: Reply,
    volume: bool,
    proofs: Vec<super::document_verification::DocumentFileProof>,
}

fn preview_unavailable(_: rusqlite::Error) -> Reply {
    artifacts::refusal(500, "Rust matter transaction failed; no changes committed")
}

impl BundlePreviewWork {
    pub fn measure(self) -> Result<MeasuredBundlePreview, Reply> {
        let (mut inputs, proofs) = self.inputs.verify()?;
        let (response, volume) = match self.output {
            PreviewOutput::Bundle => (
                inputs
                    .preview_with_snapshot(self.settings, &self.snapshot)
                    .map_err(preview_unavailable)?,
                false,
            ),
            PreviewOutput::Volume { marks, expected } => {
                if self.snapshot != expected {
                    return Err(artifacts::refusal(409, VOLUME_INPUT_CHANGED));
                }
                inputs.output_marks = marks;
                let documents = inputs
                    .documents
                    .iter()
                    .filter(|doc| doc["bundle_included"] == 1)
                    .map(|doc| {
                        let mut item = select(doc, &["id", "title", "revision", "page_count"]);
                        let id = doc["id"].as_str().ok_or(rusqlite::Error::InvalidQuery)?;
                        item["bytes"] =
                            json!(inputs.sizes.get(id).ok_or(rusqlite::Error::InvalidQuery)?);
                        Ok(item)
                    })
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(preview_unavailable)?;
                let mut plan = inputs
                    .preview_with_snapshot(self.settings, &self.snapshot)
                    .map_err(preview_unavailable)?;
                let mut body = json!({"input_snapshot":self.snapshot,"documents":documents});
                if plan.status == 200 {
                    let fields = plan
                        .body
                        .as_object_mut()
                        .ok_or(rusqlite::Error::InvalidQuery)
                        .map_err(preview_unavailable)?;
                    fields.remove("input_snapshot");
                    fields.remove("settings_snapshot");
                    body["plan"] = plan.body;
                } else {
                    body["plan_error"] = plan.body["detail"].clone();
                }
                (reply(200, body), true)
            }
        };
        Ok(MeasuredBundlePreview {
            root: self.root,
            matter: self.matter,
            snapshot: self.snapshot,
            response,
            volume,
            proofs,
        })
    }
}

impl Store {
    pub fn prepare_bundle_preview(
        &self,
        matter: &str,
        body: &Value,
    ) -> Result<BundlePreviewWork, Reply> {
        let mut body = body.clone();
        if let Some(value) = integer(&body["max_pages_per_volume"]) {
            body["max_pages_per_volume"] = json!(value);
        }
        if let Some(value) = boolean(&body["keep_groups_intact"]) {
            body["keep_groups_intact"] = json!(value);
        }
        let settings = Preview::deserialize(&body)
            .ok()
            .filter(|request| (2..=2000).contains(&request.max_pages_per_volume))
            .ok_or_else(|| artifacts::refusal(422, "Invalid bundle preview request"))?;
        self.prepare_planning_inputs(matter, settings, PreviewOutput::Bundle)
    }

    pub fn prepare_volume_planning(
        &self,
        matter: &str,
        body: &Value,
    ) -> Result<BundlePreviewWork, Reply> {
        let (marks, settings) = settings::layout(body)?;
        let expected = settings::layout_snapshot(body)?.to_owned();
        self.prepare_planning_inputs(matter, settings, PreviewOutput::Volume { marks, expected })
    }

    fn prepare_planning_inputs(
        &self,
        matter: &str,
        settings: Preview,
        output: PreviewOutput,
    ) -> Result<BundlePreviewWork, Reply> {
        let inputs = Inputs::capture(&self.con, &self.root, self.key.as_ref(), matter)
            .map_err(preview_unavailable)??;
        let snapshot = inputs
            .inputs()
            .digest(&self.con)
            .map_err(preview_unavailable)?;
        Ok(BundlePreviewWork {
            root: self.root.clone(),
            matter: matter.into(),
            inputs,
            snapshot,
            settings,
            output,
        })
    }

    pub fn complete_bundle_preview(
        &self,
        mut measured: MeasuredBundlePreview,
    ) -> Result<Reply, Reply> {
        let stale = || {
            artifacts::refusal(
                409,
                if measured.volume {
                    VOLUME_INPUT_CHANGED
                } else {
                    "Bundle inputs changed during preview; run the preview again"
                },
            )
        };
        if self.root != measured.root {
            return Err(stale());
        }
        let current =
            Inputs::metadata(&self.con, &measured.matter).map_err(preview_unavailable)??;
        if current.digest(&self.con).map_err(preview_unavailable)? != measured.snapshot
            || measured
                .proofs
                .iter()
                .any(|proof| proof.check_current().is_err())
        {
            return Err(stale());
        }
        if measured.volume {
            measured.response.body["generated"] = json!(super::generated_exports::measured_for(
                &self.con,
                &measured.matter,
                &measured.snapshot,
            )
            .map_err(preview_unavailable)?);
            measured.response.body["measured_exception"] =
                json!(super::processing_jobs::measured_exception(
                    &self.con,
                    &measured.matter,
                    &measured.snapshot,
                )
                .map_err(preview_unavailable)?);
        }
        Ok(measured.response)
    }
}

impl Store {
    pub(super) fn bundle_input_request(
        &mut self,
        method: &str,
        route: &[&str],
        body: &Value,
    ) -> rusqlite::Result<Option<Reply>> {
        if let Some(response) = self.output_settings_request(method, route, body)? {
            return Ok(Some(response));
        }
        let ["matters", id, resource] = route else {
            return Ok(None);
        };
        if let Some(kind) = destinations::Kind::from_route(resource) {
            return self.destination_request(method, id, kind, body);
        }
        if (method, *resource) == ("POST", "bundle-preview") {
            let result = self
                .prepare_bundle_preview(id, body)
                .and_then(BundlePreviewWork::measure)
                .and_then(|measured| self.complete_bundle_preview(measured));
            return Ok(Some(result.unwrap_or_else(|error| error)));
        }
        let membership = match (method, *resource) {
            ("GET", "membership") => None,
            ("PUT", "membership") => {
                match Membership::deserialize(body).ok().filter(|r| {
                    r.expected_snapshot.chars().count() == 64 && r.included_ids.len() <= 10_000
                }) {
                    Some(request) => Some(request),
                    None => {
                        return Ok(Some(artifacts::refusal(
                            422,
                            "Invalid bundle membership request",
                        )))
                    }
                }
            }
            _ => return Ok(None),
        };
        let tx = self
            .con
            .transaction_with_behavior(if membership.is_some() {
                TransactionBehavior::Immediate
            } else {
                TransactionBehavior::Deferred
            })?;
        let mut inputs = match Inputs::read(&tx, &self.root, self.key.as_ref(), id)? {
            Ok(inputs) => inputs,
            Err(refusal) => return Ok(Some(refusal)),
        };
        if let Some(request) = membership {
            if inputs.digest(&tx)? != request.expected_snapshot {
                return Ok(Some(artifacts::refusal(
                    409,
                    "Bundle inputs changed. Reload membership before applying your selection.",
                )));
            }
            let before = membership_state(&inputs.documents);
            let selected: BTreeSet<_> = request.included_ids.iter().collect();
            if selected.len() != request.included_ids.len()
                || selected.iter().any(|id| !before.contains_key(id.as_str()))
            {
                return Ok(Some(artifacts::refusal(
                    409,
                    "Membership must contain unique active documents from this matter.",
                )));
            }
            let after: Map<_, _> = before
                .keys()
                .map(|id| (id.clone(), json!(i64::from(selected.contains(id)))))
                .collect();
            if before != after {
                let stamp = now();
                apply(&tx, &mut inputs.documents, &after)?;
                bump_membership(&tx, id)?;
                record_edit(
                    &tx,
                    id,
                    "bundle_membership",
                    &json!(before),
                    &json!(after),
                    &stamp,
                )?;
                tx.execute("INSERT INTO audit_events (matter_id,action,created_at) VALUES (?,'bundle_membership_applied',?)", params![id, stamp])?;
                document_metadata::touch(&tx, id, &stamp)?;
            }
        }
        let result = inputs.membership(&tx, id)?;
        tx.commit()?;
        Ok(Some(reply(200, result)))
    }
}
