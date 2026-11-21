//! Versioned bundle compositions and stable, source-bound page selection.
use super::{
    artifacts::refusal,
    bundle_inputs::{self, BundleInputWork, Inputs, Preview},
    document_verification::DocumentFileProof,
    *,
};
use std::collections::{HashMap, HashSet};

pub(super) fn migrate(con: &Connection) -> rusqlite::Result<()> {
    con.execute_batch(
        "CREATE TABLE rust_compositions(id TEXT PRIMARY KEY,matter_id TEXT NOT NULL,kind TEXT NOT NULL,body TEXT NOT NULL);
         CREATE INDEX rust_compositions_matter ON rust_compositions(matter_id,kind);
         CREATE TABLE rust_source_pages(id TEXT PRIMARY KEY,matter_id TEXT NOT NULL,document_id TEXT NOT NULL,relative_path TEXT NOT NULL,
          hash TEXT NOT NULL,page INTEGER NOT NULL,UNIQUE(document_id,relative_path,page));
         PRAGMA user_version=12;",
    )
}

fn db(_: rusqlite::Error) -> Reply {
    refusal(500, "Unable to access saved bundle compositions")
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map(Vec::as_slice).unwrap_or_default()
}

fn title(value: &Value, key: &str, max: usize) -> Result<String, Reply> {
    let title = text(value, key).trim();
    if title.is_empty() || title.chars().count() > max {
        return Err(refusal(
            422,
            "Enter a plan title within the supported length",
        ));
    }
    Ok(title.to_owned())
}

fn lists(con: &Connection, matter: &str, kind: &str) -> Result<Vec<Value>, Reply> {
    let mut query = con.prepare("SELECT body FROM rust_compositions WHERE matter_id=? AND kind=? ORDER BY json_extract(body,'$.updated_at') DESC").map_err(db)?;
    let rows = query
        .query_map(params![matter, kind], |row| {
            decode(&row.get::<_, String>(0)?)
        })
        .map_err(db)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db)
}

pub(super) fn get(con: &Connection, matter: &str, kind: &str, id: &str) -> Result<Value, Reply> {
    con.query_row(
        "SELECT body FROM rust_compositions WHERE matter_id=? AND kind=? AND id=?",
        params![matter, kind, id],
        |row| decode(&row.get::<_, String>(0)?),
    )
    .optional()
    .map_err(db)?
    .ok_or_else(|| refusal(404, "Saved plan not found"))
}

fn export(con: &Connection, matter: &str, id: &str) -> Result<Value, Reply> {
    con.query_row(
        "SELECT manifest FROM rust_exports WHERE matter_id=? AND id=?",
        params![matter, id],
        |row| decode(&row.get::<_, String>(0)?),
    )
    .optional()
    .map_err(db)?
    .ok_or_else(|| refusal(409, "Baseline build is unavailable"))
}

fn export_history(con: &Connection, matter: &str) -> Result<Vec<Value>, Reply> {
    let mut query = con
        .prepare("SELECT manifest FROM rust_exports WHERE matter_id=? ORDER BY rowid DESC")
        .map_err(db)?;
    let rows = query
        .query_map([matter], |row| decode(&row.get::<_, String>(0)?))
        .map_err(db)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db)
}

pub(super) fn contract(plan: &Value) -> String {
    let fields: &[&str] = if plan["kind"] == "variant" {
        &[
            "id",
            "matter_id",
            "version",
            "name",
            "baseline",
            "included_ids",
            "sources",
            "settings",
            "input_snapshot",
        ]
    } else {
        &[
            "id",
            "matter_id",
            "baseline_build_id",
            "title",
            "selected_page_ids",
            "settings",
            "input_snapshot",
            "insertion_policy",
        ]
    };
    sha256_hex(bundle_inputs::select(plan, fields).to_string().as_bytes())
}

fn source_pages(store: &Store, matter: &str, doc: &Value) -> Result<(String, Vec<Value>), Reply> {
    let document = text(doc, "id");
    let revision = doc["revision"]
        .as_u64()
        .ok_or_else(|| refusal(409, "Invalid working revision"))?;
    let count = doc["page_count"]
        .as_u64()
        .filter(|count| (1..=100_000).contains(count))
        .ok_or_else(|| refusal(409, "Invalid PDF page count"))?;
    let (relative, hash): (String, String) = store
        .con
        .query_row(
            "SELECT relpath,sha256 FROM document_revisions WHERE document_id=? AND revision=?",
            params![document, revision],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db)?;
    let mut query = store.con.prepare("SELECT id,hash,page FROM rust_source_pages WHERE document_id=? AND relative_path=? ORDER BY page").map_err(db)?;
    let stored = query
        .query_map(params![document, relative], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
            ))
        })
        .map_err(db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db)?;
    let mut pages = Vec::new();
    if stored.is_empty() {
        let tx = store.con.unchecked_transaction().map_err(db)?;
        for page in 0..count {
            let id = uuid::Uuid::new_v4().simple().to_string();
            tx.execute(
                "INSERT INTO rust_source_pages VALUES (?,?,?,?,?,?)",
                params![id, matter, document, relative, hash, page],
            )
            .map_err(db)?;
            pages.push(json!({"id":id,"page_index":page,"revision":revision}));
        }
        tx.commit().map_err(db)?;
    } else {
        if stored.len() != count as usize
            || stored
                .iter()
                .enumerate()
                .any(|(index, (_, saved, page))| saved != &hash || *page != index as u64)
        {
            return Err(refusal(
                409,
                "Stored working page identities differ from the verified working PDF",
            ));
        }
        for (id, _, page) in stored {
            pages.push(json!({"id":id,"page_index":page,"revision":revision}));
        }
    }
    Ok((hash, pages))
}

pub(super) fn source_contract(
    store: &Store,
    matter: &str,
    docs: &[Value],
) -> Result<Vec<Value>, Reply> {
    docs.iter().map(|document|{
        let (hash,pages) = source_pages(store,matter,document)?;
        Ok(json!({"document_id":document["id"],"working_revision":document["revision"],"working_sha256":hash,
            "source_sha256":document["source_sha256"],"source_page_count":document["page_count"],
            "page_ids":pages.iter().map(|page|page["id"].clone()).collect::<Vec<_>>() }))
    }).collect()
}

fn default_settings(input: &Inputs) -> Value {
    let (matter, _, marks) = input.generation_inputs();
    json!({"max_pages_per_volume":matter["max_pages_per_volume"],"keep_groups_intact":matter["keep_groups_intact"],"output_marks":marks})
}

fn settings(body: &Value) -> Result<Value, Reply> {
    let cap = body["max_pages_per_volume"]
        .as_u64()
        .filter(|cap| (2..=2000).contains(cap))
        .ok_or_else(|| refusal(422, "Invalid volume page cap"))?;
    let keep = body["keep_groups_intact"]
        .as_bool()
        .ok_or_else(|| refusal(422, "Choose whether groups stay intact"))?;
    let marks = bundle_inputs::parse_marks(&body["output_marks"])?;
    Ok(json!({"max_pages_per_volume":cap,"keep_groups_intact":keep,"output_marks":marks}))
}

fn selection(value: &Value, key: &str, max: usize) -> Result<Vec<String>, Reply> {
    let selected = value[key]
        .as_array()
        .filter(|items| items.len() <= max)
        .and_then(|items| {
            items
                .iter()
                .map(|item| {
                    item.as_str()
                        .filter(|id| artifacts::valid_id(id))
                        .map(str::to_owned)
                })
                .collect::<Option<Vec<_>>>()
        })
        .ok_or_else(|| refusal(422, "Invalid source selection"))?;
    if selected.iter().collect::<HashSet<_>>().len() != selected.len() {
        return Err(refusal(422, "Select each source only once"));
    }
    Ok(selected)
}

fn ordered(input: &Inputs, ids: &[String]) -> Result<Vec<Value>, Reply> {
    let by_id = input
        .all_documents()
        .iter()
        .map(|doc| (text(doc, "id"), doc))
        .collect::<HashMap<_, _>>();
    ids.iter()
        .map(|id| {
            by_id
                .get(id.as_str())
                .map(|doc| (*doc).clone())
                .ok_or_else(|| {
                    refusal(
                        409,
                        "Variant membership contains a missing or archived document",
                    )
                })
        })
        .collect()
}

fn primary(store: &Store, matter: &str, input: &Inputs, digest: &str) -> Result<Value, Reply> {
    let effective = input.effective_digest(&store.con).map_err(db)?;
    let prior = export_history(&store.con, matter)?
        .into_iter()
        .find(|item| {
            item["artifact_kind"] == "primary" && item["effective_input_digest"] == effective
        });
    let (_, docs, _) = input.generation_inputs();
    let docs = docs.into_iter().cloned().collect::<Vec<_>>();
    Ok(
        json!({"kind":"primary","id":format!("primary:{digest}"),"version":null,"input_snapshot":digest,
        "build_id":prior.as_ref().map(|prior|&prior["id"]),
        "included_ids":docs.iter().map(|doc|&doc["id"]).collect::<Vec<_>>(),
        "sources":source_contract(store,matter,&docs)?,"settings":default_settings(input),
        "build_sha256":prior.as_ref().and_then(|prior|list(prior,"files").iter().find(|file|file["name"]=="combined.pdf")).map(|file|&file["sha256"]) }),
    )
}

fn catalog(store: &Store, matter: &str, input: &Inputs) -> Result<Vec<Value>, Reply> {
    let mut result = Vec::new();
    let mut count = 0usize;
    for doc in input.all_documents() {
        let (_, pages) = source_pages(store, matter, doc)?;
        count = count.saturating_add(pages.len());
        if count > 100_000 {
            return Err(refusal(
                422,
                "The source catalogue exceeds the Rust page limit",
            ));
        }
        let mut item = bundle_inputs::select(
            doc,
            &["id", "title", "revision", "page_count", "bundle_included"],
        );
        item["pages"] = json!(pages);
        result.push(item);
    }
    Ok(result)
}

pub(super) struct SourceSelection {
    pub docs: Vec<Value>,
    runs: Vec<Value>,
    stable: HashMap<(String, u64), String>,
}

impl SourceSelection {
    pub(super) fn includes_page(&self, document: &str, page: u64) -> bool {
        self.stable.contains_key(&(document.to_owned(), page))
    }

    pub(super) fn plan(&self, input: &Inputs, plan: &Value) -> Result<Value, Reply> {
        let mode = text(plan, "kind");
        let options = &plan["settings"];
        let settings = Preview {
            max_pages_per_volume: options["max_pages_per_volume"].as_u64().unwrap_or(300) as usize,
            keep_groups_intact: options["keep_groups_intact"].as_bool().unwrap_or_default(),
        };
        let response = input
            .composition_plan(settings, &options["output_marks"], &self.runs)
            .map_err(db)?;
        if response.status != 200 {
            return Err(response);
        }
        let mut map = response.body;
        if mode == "supplement" {
            map["included_documents"] = json!(self.docs.len());
            map["selected_page_ids"] = plan["selected_page_ids"].clone();
            if let Some(volumes) = map["volumes"].as_array_mut() {
                for volume in volumes {
                    if let Some(entries) = volume["pages"].as_array_mut() {
                        for page in entries {
                            let id = (
                                text(page, "document_id").to_owned(),
                                page["page_index"].as_u64().unwrap_or(u64::MAX),
                            );
                            page["stable_page_id"] = json!(self.stable.get(&id));
                        }
                    }
                }
            }
        }
        Ok(map)
    }
}

pub(super) fn select_sources(
    store: &Store,
    matter: &str,
    input: &Inputs,
    plan: &Value,
) -> Result<SourceSelection, Reply> {
    let mode = text(plan, "kind");
    let (docs, runs, stable) = if mode == "variant" {
        let docs = ordered(input, &selection(plan, "included_ids", 1000)?)?;
        (docs.clone(), docs, HashMap::new())
    } else {
        let chosen = selection(plan, "selected_page_ids", 100_000)?;
        let identifiers = catalog(store, matter, input)?;
        let coords = identifiers
            .iter()
            .flat_map(|doc| {
                list(doc, "pages").iter().map(move |page| {
                    (
                        text(page, "id"),
                        (text(doc, "id"), page["page_index"].as_u64().unwrap_or(0)),
                    )
                })
            })
            .collect::<HashMap<_, _>>();
        let by_id = input
            .all_documents()
            .iter()
            .map(|doc| (text(doc, "id"), doc))
            .collect::<HashMap<_, _>>();
        let mut stable = HashMap::new();
        let mut runs: Vec<Value> = Vec::new();
        let mut docs = Vec::new();
        let mut seen = HashSet::new();
        for identity in &chosen {
            let (id, page) = coords.get(identity.as_str()).ok_or_else(|| {
                refusal(
                    409,
                    "Missing page or stale working revision. Explicitly remove or reselect.",
                )
            })?;
            let doc = by_id[id];
            if seen.insert(*id) {
                docs.push(doc.clone());
            }
            if let Some(run) = runs.last_mut().filter(|run| {
                run["id"] == *id
                    && run["_source_start"].as_u64().unwrap_or(0)
                        + run["page_count"].as_u64().unwrap_or(0)
                        == *page
            }) {
                run["page_count"] = json!(run["page_count"].as_u64().unwrap_or(0) + 1);
            } else {
                let mut run = doc.clone();
                run["_source_start"] = json!(page);
                run["_source_total"] = doc["page_count"].clone();
                run["page_count"] = json!(1);
                run["group_key"] = json!("");
                runs.push(run);
            }
            stable.insert(((*id).to_owned(), *page), identity.clone());
        }
        (docs, runs, stable)
    };
    Ok(SourceSelection { docs, runs, stable })
}

pub(super) struct PolicyWork {
    baseline: Option<Result<(Value, super::preserved_labels::ReconciliationWork), Reply>>,
}

impl PolicyWork {
    pub(super) fn capture(store: &Store, matter: &str, plan: &Value, selected: &[Value]) -> Self {
        let baseline = (plan["insertion_policy"] == "preserve_printed_labels"
            && !selected.is_empty())
        .then(|| {
            export(&store.con, matter, text(plan, "baseline_build_id")).map(|prior| {
                let work =
                    super::preserved_labels::ReconciliationWork::capture(store, &prior, selected);
                (prior, work)
            })
        });
        Self { baseline }
    }

    pub(super) fn measure(
        self,
        plan: &Value,
        map: &Value,
    ) -> (Value, Vec<super::artifacts::PdfProof>) {
        let mut proofs = Vec::new();
        let value = policy(self.baseline, plan, map, &mut proofs);
        (value, proofs)
    }
}

fn policy(
    baseline_work: Option<Result<(Value, super::preserved_labels::ReconciliationWork), Reply>>,
    plan: &Value,
    map: &Value,
    proofs: &mut Vec<super::artifacts::PdfProof>,
) -> Value {
    let mut blockers = Vec::new();
    let mut labels = std::collections::BTreeMap::new();
    let mut preview = Vec::new();
    let mut boundaries = Vec::new();
    let preserved = plan["insertion_policy"] == "preserve_printed_labels";
    let mut effects = json!({
        "index_rows":list(map,"volumes").iter().map(|volume|list(volume,"rows").len()).sum::<usize>(),
        "bookmarks":"Rebuilt inside the supplemental artifact from its deterministic map.",
        "links":"Only revision-anchored destinations present in the supplemental artifact can be retained.",
        "viewer_labels":"Generated from the supplemental numbering settings.",
        "physical_positions":format!("{} physical pages in the distinct artifact.", map["combined_pages"]),
    });
    if list(plan, "selected_page_ids").is_empty() {
        blockers.push(json!({"code":"EMPTY_SELECTION","message":"Select at least one exact source page before reviewing an insertion policy."}));
    }
    if preserved && blockers.is_empty() {
        let baseline =
            baseline_work.unwrap_or_else(|| Err(refusal(409, "Baseline build is unavailable")));
        match baseline {
            Ok((prior, work)) => match super::preserved_labels::evaluate(work, &prior, map, proofs)
            {
                Ok(review) => {
                    let baseline = &prior["validation_contract"]["combined.pdf"];
                    let link_count = baseline["links"]
                        .as_object()
                        .into_iter()
                        .flatten()
                        .map(|(_, page)| page.as_array().map_or(0, Vec::len))
                        .sum::<usize>();
                    effects = json!({
                        "index_rows":list(&prior,"page_map_snapshot").len(),
                        "bookmarks":format!("{} stored outline entries will be rebuilt against reconciled physical positions.", list(baseline,"toc").len()),
                        "links":format!("{link_count} stored internal index/link destinations will be recalculated."),
                        "viewer_labels":format!("{} existing printed labels retained; {} reviewed insert labels added.", review.existing_labels, review.labels.len()),
                        "physical_positions":format!("{} inserted source pages shift later combined positions and affected volume boundaries.", review.labels.len()),
                    });
                    labels = review.labels;
                    preview = review.preview;
                    boundaries = review.boundaries;
                }
                Err(blocker) => blockers.push(blocker),
            },
            Err(error) => {
                blockers.push(json!({"code":"MISSING_BASELINE_MAP","message":error.body["detail"]}))
            }
        }
    } else if !preserved && plan["insertion_policy"] != "separate_supplement" {
        blockers.push(
            json!({"code":"MISSING_POLICY","message":"Choose a supplemental numbering policy."}),
        );
    }
    let summary = if !blockers.is_empty() {
        effects = json!({
            "index_rows":"Not recalculated","bookmarks":"Not recalculated","links":"Not recalculated",
            "viewer_labels":"Not accepted","physical_positions":"Not accepted",
        });
        "Preserved labels are blocked until every baseline reference reconciles."
    } else if preserved {
        "Earlier printed/viewer labels stay fixed; only supported decimal-suffix insert labels are added. Physical positions and navigation are recalculated."
    } else {
        "A distinct supplemental artifact will use its own reviewed numbering. The baseline stays unchanged."
    };
    json!({"mode":plan["insertion_policy"],"baseline_build_id":plan["baseline_build_id"],"baseline_created_at":plan["baseline_created_at"],
        "plan_id":plan["id"],"plan_version":plan["version"],"selected_page_ids":plan["selected_page_ids"],
        "selected_source_pages":list(plan,"selected_page_ids").len(),"artifact_map":map,"blockers":blockers,
        "summary":summary,"labels_by_page_id":labels,"preview_rows":preview,"volume_boundaries":boundaries,
        "effects":effects})
}

fn draft(
    store: &Store,
    matter: &str,
    input: &Inputs,
    kind: &str,
    plan_id: Option<&str>,
    body: &Value,
) -> Result<(Value, SourceSelection, Option<Value>), Reply> {
    let snapshot = input.digest(&store.con).map_err(db)?;
    if body["expected_snapshot"] != snapshot {
        return Err(refusal(
            409,
            "Matter inputs changed. Reload and review the saved sources.",
        ));
    }
    let old = plan_id
        .map(|id| get(&store.con, matter, kind, id))
        .transpose()?;
    if old
        .as_ref()
        .is_some_and(|old| old["version"] != body["expected_version"])
    {
        return Err(refusal(
            409,
            "Plan changed in another view. Reload before saving.",
        ));
    }
    let version = old
        .as_ref()
        .and_then(|old| old["version"].as_u64())
        .unwrap_or(0)
        + 1;
    let stamp = now();
    let id = plan_id
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
    let mut plan = json!({"id":id,"kind":kind,"matter_id":matter,"version":version,"input_snapshot":snapshot,
        "settings":settings(body)?,"created_at":old.as_ref().and_then(|old|old["created_at"].as_str()).unwrap_or(&stamp),"updated_at":stamp});
    if kind == "variant" {
        let selected = selection(body, "included_ids", 1000)?;
        let baseline_id = text(body, "baseline_id");
        let baseline_kind = text(body, "baseline_kind");
        let baseline = if let Some(old) = &old {
            if old["baseline"]["id"] != baseline_id || old["baseline"]["kind"] != baseline_kind {
                return Err(refusal(
                    409,
                    "The baseline is immutable. Create another variant for a new baseline.",
                ));
            }
            old["baseline"].clone()
        } else if baseline_kind == "primary" {
            let current = primary(store, matter, input, &snapshot)?;
            if current["id"] != baseline_id {
                return Err(refusal(
                    409,
                    "Primary baseline changed; reload before saving",
                ));
            }
            current
        } else if baseline_kind == "variant" {
            let (id, version) = baseline_id
                .split_once(':')
                .ok_or_else(|| refusal(409, "Variant baseline is unavailable"))?;
            let current = get(&store.con, matter, "variant", id)?;
            if current["version"].as_u64() != version.parse().ok() {
                return Err(refusal(409, "Variant baseline changed"));
            }
            json!({"kind":"variant","id":baseline_id,"version":current["version"],"input_snapshot":current["input_snapshot"],
                "contract_digest":contract(&current),"included_ids":current["included_ids"],"sources":current["sources"],"settings":current["settings"]})
        } else {
            return Err(refusal(422, "Choose a baseline kind"));
        };
        for key in ["volume_splits", "annexure_overrides"] {
            if plan["settings"]["output_marks"][key]
                .as_object()
                .is_some_and(|items| items.keys().any(|id| !selected.contains(id)))
            {
                return Err(refusal(
                    409,
                    "A saved output exception references a document outside this variant",
                ));
            }
        }
        plan["name"] = json!(title(body, "name", 80)?);
        plan["baseline"] = baseline;
        plan["sources"] = json!(source_contract(store, matter, &ordered(input, &selected)?)?);
        plan["included_ids"] = json!(selected);
    } else {
        let baseline = export(&store.con, matter, text(body, "baseline_build_id"))?;
        if old
            .as_ref()
            .is_some_and(|old| old["baseline_build_id"] != baseline["id"])
        {
            return Err(refusal(409, "A saved supplement baseline is immutable"));
        }
        if plan["settings"]["output_marks"]["volume_splits"]
            .as_object()
            .is_some_and(|items| !items.is_empty())
        {
            return Err(refusal(
                409,
                "Primary-bundle split policies cannot be applied to a supplemental plan",
            ));
        }
        plan["title"] = json!(title(body, "title", 200)?);
        plan["baseline_build_id"] = baseline["id"].clone();
        plan["baseline_created_at"] = baseline["created_at"].clone();
        plan["baseline_pages"] = baseline["pages"].clone();
        plan["baseline_files"] = baseline["files"].clone();
        plan["selected_page_ids"] = json!(selection(body, "selected_page_ids", 100_000)?);
        plan["insertion_policy"] = body["insertion_policy"].clone();
        plan["reviewed_policy"] = Value::Null;
    }
    let selection = select_sources(store, matter, input, &plan)?;
    Ok((plan, selection, old))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DraftAction {
    Save,
    Preview,
    PreviewPolicy,
}

pub struct CompositionDraftTarget {
    matter: String,
    kind: &'static str,
    id: Option<String>,
    action: DraftAction,
}

impl CompositionDraftTarget {
    fn for_route(method: &str, matter: &str, head: &str, tail: &[&str]) -> Option<Self> {
        let (kind, id, action) = match (method, head, tail) {
            ("POST", "variants", []) => ("variant", None, DraftAction::Save),
            ("PUT", "variants", [id]) => ("variant", Some(*id), DraftAction::Save),
            ("POST", "supplements", []) => ("supplement", None, DraftAction::Save),
            ("PUT", "supplements", [id]) => ("supplement", Some(*id), DraftAction::Save),
            ("POST", "supplement-preview", []) => ("supplement", None, DraftAction::Preview),
            ("POST", "supplement-policy-preview", []) => {
                ("supplement", None, DraftAction::PreviewPolicy)
            }
            _ => return None,
        };
        Some(Self {
            matter: matter.into(),
            kind,
            id: id.map(str::to_owned),
            action,
        })
    }

    pub fn from_path(method: &str, path: &str) -> Option<Self> {
        let route = path
            .split('?')
            .next()?
            .trim_start_matches('/')
            .split('/')
            .collect::<Vec<_>>();
        let ["matters", matter, head, tail @ ..] = route.as_slice() else {
            return None;
        };
        Self::for_route(method, matter, head, tail)
    }
}

pub struct CompositionDraftWork {
    root: PathBuf,
    target: CompositionDraftTarget,
    inputs: BundleInputWork,
    selection: SourceSelection,
    plan: Value,
    previous: Option<Value>,
    request: Value,
    policy: Option<PolicyWork>,
}

pub struct MeasuredCompositionDraft {
    root: PathBuf,
    target: CompositionDraftTarget,
    plan: Value,
    previous: Option<Value>,
    response: Value,
    proofs: Vec<super::artifacts::PdfProof>,
    input_proofs: Vec<DocumentFileProof>,
}

impl CompositionDraftWork {
    pub fn measure(self) -> Result<MeasuredCompositionDraft, Reply> {
        let (inputs, input_proofs) = self.inputs.verify()?;
        let mut plan = self.plan;
        let mut map = self.selection.plan(&inputs, &plan)?;
        let mut proofs = Vec::new();
        let mut review = self.policy.map(|policy| {
            let (review, file_proofs) = policy.measure(&plan, &map);
            proofs = file_proofs;
            review
        });
        if self.target.kind == "supplement" && self.request["policy_reviewed"] == true {
            let measured = review
                .as_ref()
                .ok_or_else(|| refusal(409, "Insertion policy is unavailable"))?;
            if let Some(blocker) = list(measured, "blockers").first() {
                return Err(refusal(409, text(blocker, "message")));
            }
            plan["reviewed_policy"] = json!({
                "mode":plan["insertion_policy"], "reviewed_at":plan["updated_at"],
                "contract_digest":contract(&plan),
                "preview_digest":sha256_hex(measured.to_string().as_bytes()),
            });
        }
        let response = match self.target.action {
            DraftAction::Save => plan.clone(),
            DraftAction::Preview => {
                map["input_snapshot"] = plan["input_snapshot"].clone();
                map["baseline_build_id"] = plan["baseline_build_id"].clone();
                map["settings_snapshot"] = self.request;
                map["generation_available"] = json!(true);
                map
            }
            DraftAction::PreviewPolicy => {
                let mut review = review
                    .take()
                    .ok_or_else(|| refusal(409, "Insertion policy is unavailable"))?;
                review["plan_id"] = Value::Null;
                review["plan_version"] = self.request["expected_version"].clone();
                review
            }
        };
        Ok(MeasuredCompositionDraft {
            root: self.root,
            target: self.target,
            plan,
            previous: self.previous,
            response,
            proofs,
            input_proofs,
        })
    }
}

impl Store {
    pub fn prepare_composition_draft(
        &self,
        target: CompositionDraftTarget,
        request: &Value,
    ) -> Result<CompositionDraftWork, Reply> {
        let matter = &target.matter;
        let inputs =
            Inputs::capture(&self.con, &self.root, self.key.as_ref(), matter).map_err(db)??;
        let (plan, selection, previous) = draft(
            self,
            matter,
            inputs.inputs(),
            target.kind,
            target.id.as_deref(),
            request,
        )?;
        let policy = (target.kind == "supplement"
            && (request["policy_reviewed"] == true || target.action == DraftAction::PreviewPolicy))
            .then(|| PolicyWork::capture(self, matter, &plan, &selection.docs));
        Ok(CompositionDraftWork {
            root: self.root.clone(),
            target,
            inputs,
            selection,
            plan,
            previous,
            request: request.clone(),
            policy,
        })
    }

    pub fn complete_composition_draft(
        &self,
        measured: MeasuredCompositionDraft,
    ) -> Result<Reply, Reply> {
        let MeasuredCompositionDraft {
            root,
            target,
            plan,
            previous,
            response,
            proofs,
            input_proofs,
        } = measured;
        let stale = || {
            refusal(
                409,
                "Matter inputs changed. Reload and review the saved sources.",
            )
        };
        if self.root != root {
            return Err(stale());
        }
        let tx = self.con.unchecked_transaction().map_err(db)?;
        let current = Inputs::metadata(&tx, &target.matter).map_err(db)??;
        if current.digest(&tx).map_err(db)? != plan["input_snapshot"]
            || proofs.iter().any(|proof| proof.check_current().is_err())
            || input_proofs
                .iter()
                .any(|proof| proof.check_current().is_err())
        {
            return Err(stale());
        }
        if target.kind == "supplement" {
            export(&tx, &target.matter, text(&plan, "baseline_build_id"))?;
        } else if target.id.is_none() && plan["baseline"]["kind"] == "variant" {
            let (id, version) = text(&plan["baseline"], "id")
                .split_once(':')
                .ok_or_else(|| refusal(409, "Variant baseline is unavailable"))?;
            let baseline = get(&tx, &target.matter, "variant", id)?;
            if baseline["version"].as_u64() != version.parse().ok()
                || contract(&baseline) != plan["baseline"]["contract_digest"]
            {
                return Err(refusal(409, "Variant baseline changed"));
            }
        }
        if target.action == DraftAction::Save {
            if let Some(id) = &target.id {
                if Some(get(&tx, &target.matter, target.kind, id)?) != previous {
                    return Err(refusal(
                        409,
                        "Plan changed in another view. Reload before saving.",
                    ));
                }
            }
            tx.execute(
                "INSERT INTO rust_compositions VALUES (?,?,?,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body",
                params![plan["id"].as_str(), target.matter, target.kind, plan.to_string()],
            ).map_err(db)?;
            tx.execute(
                "INSERT INTO audit_events (matter_id,action,created_at) VALUES (?,?,?)",
                params![
                    target.matter,
                    if target.kind == "variant" {
                        "audience_variant_saved"
                    } else {
                        "supplement_plan_saved"
                    },
                    now()
                ],
            )
            .map_err(db)?;
        }
        tx.commit().map_err(db)?;
        Ok(reply(200, response))
    }
}

pub struct VariantReviewWork {
    root: PathBuf,
    matter: String,
    plan_id: String,
    digest: String,
    body: Value,
    inputs: BundleInputWork,
    selection: SourceSelection,
    preflight: super::preflight::VariantPreflightWork,
}

pub struct MeasuredVariantReview {
    root: PathBuf,
    matter: String,
    plan_id: String,
    digest: String,
    body: Value,
    proofs: Vec<super::artifacts::PdfProof>,
    input_proofs: Vec<DocumentFileProof>,
}

impl VariantReviewWork {
    pub fn measure(self) -> Result<MeasuredVariantReview, Reply> {
        let (inputs, input_proofs) = self.inputs.verify()?;
        let mut body = self.body;
        body["preview"] = self.selection.plan(&inputs, &body["plan"])?;
        let measured = self.preflight.measure();
        body["checks"] = json!(measured.checks);
        Ok(MeasuredVariantReview {
            root: self.root,
            matter: self.matter,
            plan_id: self.plan_id,
            digest: self.digest,
            body,
            proofs: measured.proofs,
            input_proofs,
        })
    }
}

impl Store {
    pub fn prepare_variant_review(
        &self,
        matter: &str,
        id: &str,
    ) -> Result<VariantReviewWork, Reply> {
        let inputs =
            Inputs::capture(&self.con, &self.root, self.key.as_ref(), matter).map_err(db)??;
        let input = inputs.inputs();
        let digest = input.digest(&self.con).map_err(db)?;
        let plan = get(&self.con, matter, "variant", id)?;
        let selection = select_sources(self, matter, input, &plan)?;
        let documents = &selection.docs;
        let (owner, _, _) = input.generation_inputs();
        let options = &plan["settings"];
        let preflight = super::preflight::VariantPreflightWork::capture(
            self,
            owner,
            options,
            documents,
            input.all_documents(),
        )?;
        let sources = source_contract(self, matter, documents)?;
        let included = list(&plan, "included_ids");
        let baseline = list(&plan["baseline"], "included_ids");
        let baseline_sources = list(&plan["baseline"], "sources")
            .iter()
            .map(|source| (text(source, "document_id"), source))
            .collect::<HashMap<_, _>>();
        let current_sources = sources
            .iter()
            .map(|source| (text(source, "document_id"), source))
            .collect::<HashMap<_, _>>();
        let differences = input
            .all_documents()
            .iter()
            .map(|doc| {
                let current = included.contains(&doc["id"]);
                let previous = baseline.contains(&doc["id"]);
                let key = text(doc, "id");
                let changed = baseline_sources
                    .get(key)
                    .zip(current_sources.get(key))
                    .is_some_and(|(before, after)| {
                        before["working_revision"] != after["working_revision"]
                            || before["working_sha256"] != after["working_sha256"]
                    });
                let state = match (previous, current) {
                    (true, false) => "excluded",
                    (false, true) => "added",
                    (true, true) if changed => "changed",
                    _ => "unchanged",
                };
                json!({"document_id":doc["id"],"title":doc["title"],"state":state,
                "pages":doc["page_count"],"revision":doc["revision"]})
            })
            .collect::<Vec<_>>();
        let before = &plan["baseline"]["settings"];
        let mut settings_differences = ["max_pages_per_volume", "keep_groups_intact"]
            .into_iter()
            .filter(|key| before[*key] != options[*key])
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if let Some(marks) = options["output_marks"].as_object() {
            settings_differences.extend(
                marks
                    .iter()
                    .filter(|(key, value)| before["output_marks"][key.as_str()] != **value)
                    .map(|(key, _)| format!("output_marks.{key}")),
            );
        }
        let stale_reason = if plan["input_snapshot"] != digest {
            "Matter inputs changed. Review this variant and save a new version before building."
        } else if plan["sources"] != json!(sources) {
            "Working source revisions changed. Review membership before building."
        } else {
            ""
        };
        let body = json!({
            "plan":plan, "contract_digest":contract(&plan), "current":stale_reason.is_empty(),
            "stale_reason":stale_reason, "differences":differences,
            "settings_differences":settings_differences, "included_documents":documents, "baseline":plan["baseline"],
        });
        Ok(VariantReviewWork {
            root: self.root.clone(),
            matter: matter.into(),
            plan_id: id.into(),
            digest,
            body,
            inputs,
            selection,
            preflight,
        })
    }

    pub fn complete_variant_review(&self, measured: MeasuredVariantReview) -> Result<Reply, Reply> {
        let MeasuredVariantReview {
            root,
            matter,
            plan_id,
            digest,
            mut body,
            proofs,
            input_proofs,
        } = measured;
        let stale = || {
            refusal(
                409,
                "Saved inputs changed during variant review; reopen the variant",
            )
        };
        if root != self.root {
            return Err(stale());
        }
        let current = Inputs::metadata(&self.con, &matter).map_err(db)??;
        let plan = get(&self.con, &matter, "variant", &plan_id)?;
        if current.digest(&self.con).map_err(db)? != digest
            || plan != body["plan"]
            || proofs.iter().any(|proof| proof.check_current().is_err())
            || input_proofs
                .iter()
                .any(|proof| proof.check_current().is_err())
        {
            return Err(stale());
        }
        let contract_digest = contract(&plan);
        let mut output_history = export_history(&self.con, &matter)?
            .into_iter()
            .filter(|item| item["variant_plan_id"] == plan_id)
            .collect::<Vec<_>>();
        for item in &mut output_history {
            if item["artifact_kind"] == "variant" {
                item["stale"] = json!(
                    plan["input_snapshot"] != digest
                        || item["effective_input_digest"] != contract_digest
                );
            } else {
                self.mark_stale(&matter, item).map_err(db)?;
            }
        }
        body["output_history"] = json!(output_history);
        Ok(reply(200, body))
    }

    pub(super) fn composition_request(
        &self,
        method: &str,
        route: &[&str],
        body: &Value,
    ) -> Option<Reply> {
        let ["matters", matter, head, tail @ ..] = route else {
            return None;
        };
        if let ("GET", "variants", [id, "review"]) = (method, *head, tail) {
            let response = self
                .prepare_variant_review(matter, id)
                .and_then(VariantReviewWork::measure)
                .and_then(|measured| self.complete_variant_review(measured));
            return Some(response.unwrap_or_else(|error| error));
        }
        if let Some(target) = CompositionDraftTarget::for_route(method, matter, head, tail) {
            let response = self
                .prepare_composition_draft(target, body)
                .and_then(CompositionDraftWork::measure)
                .and_then(|measured| self.complete_composition_draft(measured));
            return Some(response.unwrap_or_else(|error| error));
        }
        if let ("GET", [], Some(kind)) = (method, tail, CompositionCatalogKind::from_route(head)) {
            let result = self
                .prepare_composition_catalog(matter, kind)
                .and_then(CompositionCatalogWork::verify)
                .and_then(|verified| self.complete_composition_catalog(verified));
            return Some(result.unwrap_or_else(|error| error));
        }
        match *head {
            "variants" | "supplements" | "supplement-preview" | "supplement-policy-preview" => {}
            _ => return None,
        }
        if route.last() == Some(&"build") {
            return None;
        }
        let result = (|| {
            let input =
                Inputs::read(&self.con, &self.root, self.key.as_ref(), matter).map_err(db)??;
            input.digest(&self.con).map_err(db)?;
            Err(unsupported())
        })();
        Some(
            result
                .map(|value| reply(200, value))
                .unwrap_or_else(|error| error),
        )
    }
}

#[derive(Clone, Copy)]
pub enum CompositionCatalogKind {
    Variants,
    Supplements,
}

impl CompositionCatalogKind {
    fn from_route(resource: &str) -> Option<Self> {
        match resource {
            "variants" => Some(Self::Variants),
            "supplements" => Some(Self::Supplements),
            _ => None,
        }
    }
}

pub struct CompositionCatalogWork {
    root: PathBuf,
    matter: String,
    kind: CompositionCatalogKind,
    snapshot: String,
    inputs: BundleInputWork,
}

pub struct VerifiedCompositionCatalog {
    root: PathBuf,
    matter: String,
    kind: CompositionCatalogKind,
    snapshot: String,
    inputs: Inputs,
    proofs: Vec<DocumentFileProof>,
}

impl CompositionCatalogWork {
    pub fn verify(self) -> Result<VerifiedCompositionCatalog, Reply> {
        let (inputs, proofs) = self.inputs.verify()?;
        Ok(VerifiedCompositionCatalog {
            root: self.root,
            matter: self.matter,
            kind: self.kind,
            snapshot: self.snapshot,
            inputs,
            proofs,
        })
    }
}

impl Store {
    pub fn prepare_composition_catalog(
        &self,
        matter: &str,
        kind: CompositionCatalogKind,
    ) -> Result<CompositionCatalogWork, Reply> {
        let inputs =
            Inputs::capture(&self.con, &self.root, self.key.as_ref(), matter).map_err(db)??;
        let snapshot = inputs.inputs().digest(&self.con).map_err(db)?;
        Ok(CompositionCatalogWork {
            root: self.root.clone(),
            matter: matter.into(),
            kind,
            snapshot,
            inputs,
        })
    }

    pub fn complete_composition_catalog(
        &self,
        verified: VerifiedCompositionCatalog,
    ) -> Result<Reply, Reply> {
        let VerifiedCompositionCatalog {
            root,
            matter,
            kind,
            snapshot,
            inputs,
            proofs,
        } = verified;
        let stale = || {
            refusal(
                409,
                "Saved inputs changed during catalog preparation; reload the sources",
            )
        };
        if root != self.root {
            return Err(stale());
        }
        let current = Inputs::metadata(&self.con, &matter).map_err(db)??;
        if current.digest(&self.con).map_err(db)? != snapshot
            || proofs.iter().any(|proof| proof.check_current().is_err())
        {
            return Err(stale());
        }
        let result = match kind {
            CompositionCatalogKind::Variants => {
                let settings = default_settings(&inputs);
                let mut blank = settings.clone();
                blank["output_marks"] = bundle_inputs::marks();
                json!({
                    "plans":lists(&self.con, &matter, "variant")?, "documents":inputs.all_documents(),
                    "source_snapshot":snapshot, "primary_baseline":primary(self, &matter, &inputs, &snapshot)?,
                    "primary_settings":settings, "blank_settings":blank,
                })
            }
            CompositionCatalogKind::Supplements => {
                let mut defaults = default_settings(&inputs);
                defaults["keep_groups_intact"] = json!(false);
                defaults["output_marks"]["front_matter"] = json!("cover_index");
                defaults["output_marks"]["numbering_prefix"] = json!("S1 · ");
                defaults["output_marks"]["numbering_start"] = json!(1);
                defaults["output_marks"]["volume_splits"] = json!({});
                defaults["output_marks"]["annexure_overrides"] = json!({});
                json!({
                    "plans":lists(&self.con, &matter, "supplement")?,
                    "documents":catalog(self, &matter, &inputs)?, "input_snapshot":snapshot, "defaults":defaults,
                })
            }
        };
        Ok(reply(200, result))
    }
}
