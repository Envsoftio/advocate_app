//! Generate bundle pages from the saved planner map and place reviewed navigation.
use super::{
    artifacts::{refusal, Artifact, PdfProof, PdfSource},
    bundle_inputs::{self, BundleInputWork, Inputs, Preview},
    compositions::{PolicyWork, SourceSelection},
    document_output::{append_text, flatten_with_assets},
    document_verification::DocumentFileProof,
    page_tools::{
        assembly::{self, LinkTarget, Navigation, Overlay, Source},
        signature_guard,
    },
    *,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write as _,
    io::Cursor,
    sync::LazyLock,
};

const PAGE_WIDTH: f64 = 595.0;
const PAGE_HEIGHT: f64 = 842.0;
const INK: (f64, f64, f64) = (0.08, 0.16, 0.29);
const BLUE: (f64, f64, f64) = (0.16, 0.40, 0.75);
const DRAFT: (f64, f64, f64) = (0.50, 0.32, 0.24);
pub(super) const BUILD_LIMIT: usize = 160 * 1024 * 1024;
const IMAGE_CHANGED: &str = "An image overlay changed during build preparation";
pub(super) const BUILD_INPUT_CHANGED: &str =
    "Saved inputs changed during build preparation; submit current inputs again";
pub(super) const OLDER_DRAFT_README: &[u8] = b"MatterDraft Studio - DRAFT EXPORT\n\nReview this package before filing. It does not certify the matter, signatures or filing completeness.\n";

pub(super) fn draft_readme(kind: &Value) -> String {
    let descriptor = match kind.as_str() {
        Some("variant") => "AUDIENCE VARIANT ",
        Some("supplement") => "SUPPLEMENTAL ",
        _ => "",
    };
    format!("MatterDraft Studio - DRAFT {descriptor}EXPORT\n\n\
        This package is NOT court-certified and is NOT proof of filing.\n\
        The selected rules are illustrative technical checks only.\n\
        Original evidence and signed files must be reviewed independently.\n\
        Working-copy PDF annotation objects and link actions are omitted from volume and combined PDFs.\n\
        Flattened marks already in page content remain visible.\n\
        Resolved PDF-scope marks and image overlays are flattened into new source-document pages; staged redactions remove content in the exported PDFs. Private review notes are omitted.\n\
        Generated index links, bookmarks and explicitly reviewed MatterDraft links are retained when their revision-anchored targets resolve.\n\
        Cross-volume reviewed links are available only in combined.pdf; configured output marks apply to source document pages, not index pages.\n\
        See validation-report.json for omissions, mark settings and detectable overlap warnings.\n\
        Every PDF page number in an index is the 1-based page position WITHIN that volume.\n\
        combined.pdf concatenates all generated volumes, including their separate indexes.\n")
}

fn interactive_content_policy() -> Value {
    json!({
        "source_annotations": "omitted",
        "source_links": "omitted",
        "courtbundle_image_overlays": "flattened into source-document pages when revision anchors resolve",
        "courtbundle_highlights_comments": "private review annotations omitted from filing-preparation PDFs",
        "courtbundle_editable_pdf_marks": "PDF-scope text, shapes and stamps flattened; staged redactions applied to generated PDFs when revision anchors resolve",
        "reviewed_user_links": "retained when both revision-anchored endpoints resolve",
        "generated_index_links": "retained",
    })
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn page(value: &Value, key: &str) -> Result<usize, Reply> {
    value[key]
        .as_u64()
        .and_then(|number| usize::try_from(number).ok())
        .ok_or_else(|| refusal(422, "Generated page map is incomplete"))
}

fn list<'a>(value: &'a Value, key: &str) -> Result<&'a [Value], Reply> {
    value[key]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| refusal(422, "Generated page map is incomplete"))
}

fn build_error(error: impl Into<String>) -> Reply {
    refusal(422, error)
}

fn revision_evidence(records: Vec<Value>, composition: Option<&Value>) -> Vec<Value> {
    let selected = composition
        .filter(|plan| plan["kind"] == "supplement")
        .and_then(|plan| plan["selected_page_ids"].as_array())
        .map(|pages| {
            pages
                .iter()
                .enumerate()
                .filter_map(|(order, id)| id.as_str().map(|id| (id, order)))
                .collect::<HashMap<_, _>>()
        });
    records
        .into_iter()
        .map(|record| {
            let page_ids = if let Some(selected) = &selected {
                let mut ordered = record["page_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|value| {
                        let id = value.as_str()?;
                        Some((*selected.get(id)?, id))
                    })
                    .collect::<Vec<_>>();
                ordered.sort_unstable_by_key(|(order, _)| *order);
                json!(ordered.into_iter().map(|(_, id)| id).collect::<Vec<_>>())
            } else {
                record["page_ids"].clone()
            };
            json!({"document_id":record["document_id"],"revision":record["working_revision"],
                "working_sha256":record["working_sha256"],"source_page_count":record["source_page_count"],
                "page_ids":page_ids})
        })
        .collect()
}

pub(super) struct Build {
    pub manifest: Value,
    pub files: BTreeMap<String, Vec<u8>>,
    pub initial_check: Value,
}

pub(super) struct BuildFreeze {
    frozen: FrozenBuild,
    sources: Vec<BuildSource>,
    asset_store: Option<StoreReader>,
}

struct BuildSource {
    file: PdfSource,
    marks: Vec<Value>,
    asset_digests: HashMap<String, String>,
}

impl BuildFreeze {
    pub(super) fn compute(self) -> Result<(FrozenBuild, Vec<Vec<u8>>, Vec<PdfProof>), Reply> {
        let mut frozen = self.frozen;
        let mut sources = Vec::with_capacity(self.sources.len());
        let mut proofs = Vec::with_capacity(self.sources.len());
        let assets = self
            .asset_store
            .as_ref()
            .map(StoreReader::open)
            .transpose()
            .map_err(|_| refusal(409, "Unable to open saved image overlays"))?;
        let mut source_bytes = 0usize;
        let mut prepared_bytes = 0usize;
        for source in self.sources {
            let (bytes, proof) = source.file.read_with_proof(|| false).map_err(|error| {
                if error.status == 422 {
                    refusal(409, "Unable to load working PDF")
                } else {
                    error
                }
            })?;
            source_bytes = source_bytes.saturating_add(bytes.len());
            if source_bytes > BUILD_LIMIT {
                return Err(build_error("Bundle sources exceed the Rust build limit"));
            }
            let bytes = flatten_with_assets(bytes, &source.marks, |id| {
                let unavailable = "Unable to read an image overlay";
                let con = assets.as_ref().ok_or(unavailable)?;
                let bytes = super::annotations::asset(con, id).map_err(|_| unavailable)?;
                if bytes.as_deref().map(sha256_hex).as_ref() != source.asset_digests.get(id) {
                    return Err(IMAGE_CHANGED);
                }
                Ok(bytes)
            })
            .map_err(|error| {
                if error == IMAGE_CHANGED {
                    refusal(409, BUILD_INPUT_CHANGED)
                } else {
                    build_error(error)
                }
            })?;
            prepared_bytes = prepared_bytes.saturating_add(bytes.len());
            if prepared_bytes > BUILD_LIMIT {
                return Err(build_error(
                    "Prepared bundle sources exceed the Rust build limit",
                ));
            }
            frozen.source_digests.push(sha256_hex(&bytes));
            sources.push(bytes);
            proofs.push(proof);
        }
        Ok((frozen, sources, proofs))
    }
}

#[derive(Deserialize, Serialize)]
pub(super) struct BuildInputs {
    matter: Value,
    marks: Value,
    profile: Value,
    #[serde(default)]
    preset: Value,
    #[serde(default)]
    membership_revision: u64,
    plan: Value,
    checks: Vec<Value>,
    docs: Vec<Value>,
    links: Vec<Value>,
    bookmarks: Vec<Value>,
    filing_review: Value,
    digest: String,
    full_digest: String,
    acknowledged: bool,
    composition: Option<Value>,
    #[serde(default)]
    insertion_policy_contract: Value,
}

pub(super) struct CompositionBuildPlan {
    inputs: BundleInputWork,
    pub selection: SourceSelection,
    composition: Value,
    review: CompositionBuildReview,
    signature: Result<(), Reply>,
}

enum CompositionBuildReview {
    Variant(Result<super::preflight::VariantPreflightWork, Reply>),
    Supplement(PolicyWork),
}

pub(super) struct MeasuredCompositionBuild {
    plan: Value,
    checks: Vec<Value>,
    policy: Value,
    pub proofs: Vec<PdfProof>,
    pub input_proofs: Vec<DocumentFileProof>,
}

impl CompositionBuildPlan {
    pub(super) fn measure(self) -> Result<MeasuredCompositionBuild, Reply> {
        let (inputs, input_proofs) = self.inputs.verify()?;
        let mut plan = self.selection.plan(&inputs, &self.composition)?;
        let (checks, policy, proofs) = match self.review {
            CompositionBuildReview::Variant(work) => {
                if self.selection.docs.is_empty() {
                    return Err(refusal(422, "Include source pages before building"));
                }
                let measurement = work?.measure();
                if measurement
                    .checks
                    .iter()
                    .any(|finding| finding["severity"] == "error")
                {
                    return Err(refusal(409, "Variant Preflight has blocking findings. Review this exact plan before building."));
                }
                (measurement.checks, Value::Null, measurement.proofs)
            }
            CompositionBuildReview::Supplement(work) => {
                let (review, proofs) = work.measure(&self.composition, &plan);
                if review["blockers"]
                    .as_array()
                    .is_some_and(|items| !items.is_empty())
                    || self.composition["reviewed_policy"]["preview_digest"]
                        != sha256_hex(review.to_string().as_bytes())
                {
                    return Err(refusal(
                        409,
                        "Saved insertion policy changed; review the exact baseline and insert pages",
                    ));
                }
                let labels = serde_json::from_value(review["labels_by_page_id"].clone())
                    .map_err(|_| build_error("Invalid reviewed labels"))?;
                super::preserved_labels::apply(&mut plan, &labels)?;
                if self.selection.docs.is_empty() {
                    return Err(refusal(422, "Include source pages before building"));
                }
                (Vec::new(), review, proofs)
            }
        };
        self.signature?;
        Ok(MeasuredCompositionBuild {
            plan,
            checks,
            policy,
            proofs,
            input_proofs,
        })
    }
}

pub(super) fn check_primary_signatures(input: &Inputs, acknowledged: bool) -> Result<(), Reply> {
    let (_, docs, _) = input.generation_inputs();
    signature_guard(docs, acknowledged, "building transformed bundle PDFs")
}

impl BuildInputs {
    pub(super) fn capture(store: &Store, id: &str, acknowledged: bool) -> Result<Self, Reply> {
        let db_error = |_| refusal(500, "Unable to load saved bundle inputs");
        let input =
            Inputs::read(&store.con, &store.root, store.key.as_ref(), id).map_err(db_error)??;
        check_primary_signatures(&input, acknowledged)?;
        let (mut captured, settings) =
            Self::capture_primary_metadata(store, id, acknowledged, &input)?;
        captured.plan = input
            .preview_with_snapshot(settings, &captured.full_digest)
            .map_err(db_error)?
            .body;
        Ok(captured)
    }

    pub(super) fn capture_primary_metadata(
        store: &Store,
        id: &str,
        acknowledged: bool,
        input: &Inputs,
    ) -> Result<(Self, Preview), Reply> {
        let db_error = |_| refusal(500, "Unable to load saved bundle inputs");
        let (matter, docs, marks) = input.generation_inputs();
        let profile = bundle_inputs::profile(matter).map_err(db_error)?;
        let settings = Preview {
            max_pages_per_volume: page(matter, "max_pages_per_volume")?,
            keep_groups_intact: matter["keep_groups_intact"].as_bool().unwrap_or(true),
        };
        let (bookmarks, links) =
            bundle_inputs::resolved_destinations(&store.con, id, &input.all_documents())
                .map_err(db_error)?;
        let checks = Vec::new();
        let (_, preset) = bundle_inputs::backup_settings(&store.con, id).map_err(db_error)?;
        let membership_revision = Inputs::membership_revision(&store.con, id).map_err(db_error)?;
        Ok((
            Self {
                matter: matter.clone(),
                marks: marks.clone(),
                profile,
                preset,
                membership_revision,
                plan: Value::Null,
                checks,
                filing_review: preparation::filing_review(&store.con, id)
                    .map_err(db_error)?
                    .unwrap_or_default(),
                docs: docs.into_iter().cloned().collect(),
                links,
                bookmarks,
                digest: input.effective_digest(&store.con).map_err(db_error)?,
                full_digest: input.digest(&store.con).map_err(db_error)?,
                acknowledged,
                composition: None,
                insertion_policy_contract: Value::Null,
            },
            settings,
        ))
    }

    pub(super) fn capture_composition(
        store: &Store,
        matter: &str,
        kind: &str,
        id: &str,
        acknowledged: bool,
    ) -> Result<(Self, CompositionBuildPlan), Reply> {
        let db = |_| refusal(500, "Unable to load saved composition");
        let composition = super::compositions::get(&store.con, matter, kind, id)?;
        let inputs =
            Inputs::capture(&store.con, &store.root, store.key.as_ref(), matter).map_err(db)??;
        let input = inputs.inputs();
        let digest = input.digest(&store.con).map_err(db)?;
        if composition["input_snapshot"] != digest {
            return Err(refusal(
                409,
                "Matter inputs changed; review and save the exact composition again",
            ));
        }
        if kind == "supplement"
            && composition["reviewed_policy"]["contract_digest"]
                != super::compositions::contract(&composition)
        {
            return Err(refusal(
                409,
                "Review and accept the supported insertion policy before building",
            ));
        }
        let (owner, _, _) = input.generation_inputs();
        let selection = super::compositions::select_sources(store, matter, input, &composition)?;
        let docs = &selection.docs;
        let signature =
            signature_guard(docs, acknowledged, "building transformed composition PDFs");
        let review = if kind == "variant" {
            let prepared = (|| {
                if composition["sources"]
                    != json!(super::compositions::source_contract(store, matter, docs)?)
                {
                    return Err(refusal(
                        409,
                        "Working source revisions changed. Review membership before building.",
                    ));
                }
                super::preflight::VariantPreflightWork::capture(
                    store,
                    owner,
                    &composition["settings"],
                    docs,
                    input.all_documents(),
                )
            })();
            CompositionBuildReview::Variant(prepared)
        } else {
            CompositionBuildReview::Supplement(PolicyWork::capture(
                store,
                matter,
                &composition,
                docs,
            ))
        };
        let profile = bundle_inputs::profile(owner).map_err(db)?;
        let (bookmarks, links) =
            bundle_inputs::resolved_destinations(&store.con, matter, input.all_documents())
                .map_err(db)?;
        let marks = composition["settings"]["output_marks"].clone();
        Ok((
            Self {
                matter: owner.clone(),
                profile,
                preset: Value::Null,
                membership_revision: 0,
                plan: Value::Null,
                marks,
                docs: docs.clone(),
                bookmarks,
                links,
                checks: Vec::new(),
                filing_review: preparation::filing_review(&store.con, matter)
                    .map_err(db)?
                    .unwrap_or_default(),
                full_digest: digest.clone(),
                digest: super::compositions::contract(&composition),
                acknowledged,
                composition: Some(composition.clone()),
                insertion_policy_contract: Value::Null,
            },
            CompositionBuildPlan {
                inputs,
                selection,
                composition,
                review,
                signature,
            },
        ))
    }

    pub(super) fn set_checks(&mut self, checks: Vec<Value>) {
        self.checks = checks;
    }

    pub(super) fn snapshot(&self) -> &str {
        &self.full_digest
    }

    pub(super) fn prepare_preflight(
        &self,
        store: &Store,
        id: &str,
    ) -> Result<super::preflight::PreflightWork, Reply> {
        super::preflight::PreflightWork::capture(
            store,
            id,
            Some(super::preflight::BuildPlanEvidence {
                snapshot: &self.full_digest,
                effective: &self.digest,
            }),
        )
    }

    pub(super) fn prepare_freeze(
        self,
        store: &Store,
        id: &str,
        selection: Option<&SourceSelection>,
    ) -> Result<BuildFreeze, Reply> {
        let marks = super::annotations::list(&store.con, id)
            .map_err(|_| refusal(500, "Unable to load bundle annotations"))?;
        let mut sources = Vec::new();
        let input_revisions = revision_evidence(
            super::compositions::source_contract(store, id, &self.docs)?,
            self.composition.as_ref(),
        );
        let mut redactions = Vec::new();
        let mut annotation_inputs = Vec::new();
        let supplement = self
            .composition
            .as_ref()
            .is_some_and(|plan| plan["kind"] == "supplement");
        for doc in &self.docs {
            let doc_id = text(doc, "id");
            let expected_revision = doc["revision"].as_u64();
            let file = store.pdf_source(
                doc_id,
                &Artifact::Working {
                    revision: expected_revision,
                },
            )?;
            let page_selected = |entry: &Value| {
                !supplement
                    || selection.is_some_and(|selection| {
                        entry["page_index"]
                            .as_u64()
                            .is_some_and(|page| selection.includes_page(doc_id, page))
                    })
            };
            let document_marks = marks
                .iter()
                .filter(|mark| mark["document_id"] == doc_id && page_selected(mark));
            let current_marks = document_marks
                .clone()
                .filter(|mark| mark["output_scope"] == "pdf")
                .collect::<Vec<_>>();
            let mut asset_digests = HashMap::new();
            for mark in document_marks {
                let mut input = bundle_inputs::select(
                    mark,
                    &[
                        "id",
                        "document_id",
                        "page_index",
                        "target_revision",
                        "kind",
                        "rect_x",
                        "rect_y",
                        "rect_width",
                        "rect_height",
                        "color",
                        "position",
                        "output_scope",
                        "resolved",
                    ],
                );
                input["body_present"] = json!(!text(mark, "body").is_empty());
                input["asset_sha256"] = if mark["kind"] == "image" {
                    let asset_id = text(mark, "id");
                    let digest = super::annotations::asset(&store.con, asset_id)
                        .map_err(|_| refusal(500, "Unable to read an image overlay"))?
                        .as_deref()
                        .map(sha256_hex);
                    if mark["resolved"] == true
                        && current_marks.iter().any(|item| item["id"] == asset_id)
                    {
                        if let Some(digest) = &digest {
                            asset_digests.insert(asset_id.to_owned(), digest.clone());
                        }
                    }
                    json!(digest)
                } else {
                    Value::Null
                };
                annotation_inputs.push(input);
            }
            if current_marks
                .iter()
                .any(|mark| mark["resolved"] != true && mark["kind"] != "image")
            {
                return Err(refusal(
                    409,
                    "Editable PDF marks need review after the working PDF changed",
                ));
            }
            let selected = current_marks
                .into_iter()
                .filter(|mark| mark["resolved"] == true)
                .cloned()
                .collect::<Vec<_>>();
            redactions.extend(
                super::redaction_records::records(&store.con, doc_id)
                    .map_err(|_| refusal(500, "Unable to load prior redaction records"))?
                    .into_iter()
                    .filter(|record| record["active"] == 1 && page_selected(record)),
            );
            sources.push(BuildSource {
                file,
                marks: selected,
                asset_digests,
            });
        }
        let asset_store = if sources
            .iter()
            .any(|source| !source.asset_digests.is_empty())
        {
            Some(
                StoreReader::capture(store)
                    .map_err(|_| refusal(500, "Unable to locate saved image overlays"))?,
            )
        } else {
            None
        };
        Ok(BuildFreeze {
            frozen: FrozenBuild {
                inputs: self,
                input_revisions,
                redactions,
                annotation_inputs,
                source_digests: Vec::new(),
            },
            sources,
            asset_store,
        })
    }
}

impl BuildFreeze {
    pub(super) fn set_composition(
        &mut self,
        measured: MeasuredCompositionBuild,
    ) -> (Vec<PdfProof>, Vec<DocumentFileProof>) {
        self.frozen.inputs.plan = measured.plan;
        self.frozen.inputs.checks = measured.checks;
        self.frozen.inputs.insertion_policy_contract = measured.policy;
        (measured.proofs, measured.input_proofs)
    }

    pub(super) fn set_plan(&mut self, plan: Value) {
        self.frozen.inputs.plan = plan;
    }

    pub(super) fn set_checks(&mut self, checks: Vec<Value>) {
        self.frozen.inputs.set_checks(checks);
    }
}

#[derive(Deserialize, Serialize)]
pub(super) struct FrozenBuild {
    inputs: BuildInputs,
    input_revisions: Vec<Value>,
    redactions: Vec<Value>,
    #[serde(default)]
    annotation_inputs: Vec<Value>,
    source_digests: Vec<String>,
}

#[derive(PartialEq, Eq)]
pub(super) struct RetainedSource {
    rowid: i64,
    position: usize,
    bytes: u64,
}

pub(super) fn retained_source_rows(
    con: &Connection,
    snapshot: &str,
) -> Result<Vec<RetainedSource>, Reply> {
    let missing = |_| refusal(409, "Saved build sources are unavailable");
    let mut query = con
        .prepare(
            "SELECT rowid, position, length(bytes) FROM rust_frozen_sources
             WHERE snapshot_id=? ORDER BY position",
        )
        .map_err(missing)?;
    let rows = query
        .query_map([snapshot], |row| {
            Ok(RetainedSource {
                rowid: row.get(0)?,
                position: row.get(1)?,
                bytes: row.get(2)?,
            })
        })
        .map_err(missing)?;
    rows.collect::<rusqlite::Result<_>>().map_err(missing)
}

impl FrozenBuild {
    pub(super) fn check_current_inputs(&self, con: &Connection, matter: &str) -> Result<(), Reply> {
        let failed = |_| refusal(500, "Unable to recheck saved build inputs");
        let stale = || refusal(409, BUILD_INPUT_CHANGED);
        let current = Inputs::metadata(con, matter).map_err(failed)??;
        if current.digest(con).map_err(failed)? != self.inputs.full_digest {
            return Err(stale());
        }
        if let Some(composition) = &self.inputs.composition {
            let saved = super::compositions::get(
                con,
                matter,
                text(composition, "kind"),
                text(composition, "id"),
            )?;
            if &saved != composition {
                return Err(stale());
            }
        } else if Inputs::membership_revision(con, matter).map_err(failed)?
            != self.inputs.membership_revision
            || current.effective_digest(con).map_err(failed)? != self.inputs.digest
        {
            return Err(stale());
        }
        Ok(())
    }

    pub(super) fn verify_retained_sources(
        &self,
        con: &Connection,
        snapshot: &str,
    ) -> Result<Vec<RetainedSource>, Reply> {
        use std::io::Read as _;

        let missing = |_| refusal(409, "Saved build sources are unavailable");
        let rows = retained_source_rows(con, snapshot)?;
        if rows.len() != self.source_digests.len() || rows.len() != self.inputs.docs.len() {
            return Err(refusal(409, "Saved bundle sources are incomplete"));
        }
        let mut size = 0u64;
        let mut buffer = [0; 64 * 1024];
        for (position, (row, expected)) in rows.iter().zip(&self.source_digests).enumerate() {
            if row.position != position {
                return Err(refusal(409, "Saved bundle sources are incomplete"));
            }
            size = size.saturating_add(row.bytes);
            if size > BUILD_LIMIT as u64 {
                return Err(refusal(409, "Saved bundle sources exceed the build limit"));
            }
            let mut source = con
                .blob_open(
                    rusqlite::DatabaseName::Main,
                    "rust_frozen_sources",
                    "bytes",
                    row.rowid,
                    true,
                )
                .map_err(missing)?;
            if source.len() as u64 != row.bytes {
                return Err(refusal(409, "Saved build sources are unavailable"));
            }
            let mut digest = Sha256::new();
            loop {
                let read = source
                    .read(&mut buffer)
                    .map_err(|_| refusal(409, "Saved build source cannot be read"))?;
                if read == 0 {
                    break;
                }
                digest.update(&buffer[..read]);
            }
            if vault::hex_bytes(&digest.finalize()).as_str() != expected {
                return Err(refusal(
                    409,
                    "Saved bundle source failed its integrity check",
                ));
            }
        }
        Ok(rows)
    }

    pub(super) fn summary(
        &self,
        kind: &str,
        id: &str,
        digest: &str,
        target: Option<&str>,
    ) -> Value {
        let physical = self.inputs.plan["combined_pages"].as_u64();
        let placed = self.inputs.plan["volumes"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|volume| volume["pages"].as_array().into_iter().flatten())
            .filter(|page| !page["document_id"].is_null())
            .count();
        json!({"snapshot_id":id,"snapshot_digest":digest,"input_digest":self.inputs.digest,
            "captured_at":now(),"kind":kind,"plan_id":target,
            "included_documents":self.inputs.docs.len(),"source_pages":placed,
            "generated_pages":physical.map(|pages| pages.saturating_sub(placed as u64)),"physical_pages":physical,
            "volumes":self.inputs.plan["volumes"].as_array().map(Vec::len),
            "documents":self.inputs.docs.iter().map(|doc|bundle_inputs::select(doc,
                &["id","title","revision","page_count","signature_status","certificate_signature_status","visible_signature_status"])).collect::<Vec<_>>()})
    }

    pub(super) fn require_signature_review(&mut self, acknowledged: bool) -> Result<(), Reply> {
        if bundle_inputs::profile(&self.inputs.matter).ok().as_ref() != Some(&self.inputs.profile) {
            return Err(refusal(
                409,
                "The technical profile changed since submission. Review and build current inputs.",
            ));
        }
        signature_guard(
            self.inputs.docs.iter(),
            acknowledged,
            "processing saved bundle inputs",
        )?;
        self.inputs.acknowledged = acknowledged;
        Ok(())
    }

    pub(super) fn prepare(mut self, raw: Vec<Vec<u8>>) -> Result<PreparedBuild, Reply> {
        if raw.len() != self.source_digests.len() || raw.len() != self.inputs.docs.len() {
            return Err(refusal(409, "Saved bundle sources are incomplete"));
        }
        let blocked = self
            .inputs
            .checks
            .iter()
            .filter(|finding| finding["severity"] == "error")
            .collect::<Vec<_>>();
        if !blocked.is_empty() {
            return Err(reply(
                422,
                json!({"detail":"Preflight failed","failures":blocked}),
            ));
        }
        if self.inputs.docs.is_empty() {
            return Err(build_error("Include documents before building"));
        }
        if self.inputs.plan["volumes"]
            .as_array()
            .is_none_or(Vec::is_empty)
        {
            return Err(build_error(
                self.inputs.plan["detail"]
                    .as_str()
                    .unwrap_or("Saved build has no volume plan"),
            ));
        }
        let mut sources = vec![
            Source::open(assembly::blank((595.0, 842.0)).map_err(build_error)?)
                .map_err(build_error)?,
        ];
        let mut source_ids = HashMap::new();
        for ((document, bytes), digest) in
            self.inputs.docs.iter().zip(raw).zip(&self.source_digests)
        {
            if sha256_hex(&bytes) != *digest {
                return Err(refusal(
                    409,
                    "Saved bundle source failed its integrity check",
                ));
            }
            source_ids.insert(text(document, "id").to_owned(), sources.len());
            sources.push(Source::open(bytes).map_err(build_error)?);
        }
        let mut locations = HashMap::new();
        for volume in list(&self.inputs.plan, "volumes")? {
            for entry in list(volume, "pages")? {
                if let Some(id) = entry["document_id"].as_str() {
                    locations.insert(
                        (id.to_owned(), page(entry, "page_index")?),
                        (
                            page(volume, "number")?,
                            page(entry, "physical_page")? - 1,
                            page(entry, "combined_page")? - 1,
                        ),
                    );
                }
            }
        }
        let supplement = self
            .inputs
            .composition
            .as_ref()
            .is_some_and(|plan| plan["kind"] == "supplement");
        let selected = |entry: &Value, id: &str, index: &str| {
            if supplement {
                location(&locations, entry, id, index).is_some()
            } else {
                source_ids.contains_key(text(entry, id))
            }
        };
        self.inputs
            .bookmarks
            .retain(|entry| selected(entry, "document_id", "page_index"));
        self.inputs
            .links
            .retain(|entry| selected(entry, "source_document_id", "source_page_index"));
        self.annotation_inputs
            .retain(|entry| selected(entry, "document_id", "page_index"));
        for link in self
            .inputs
            .links
            .iter()
            .filter(|link| link["resolved"] == true)
        {
            if let Some(source) =
                location(&locations, link, "source_document_id", "source_page_index")
            {
                if !supplement
                    && link["kind"] == "internal"
                    && location(&locations, link, "target_document_id", "target_page_index")
                        .is_none()
                {
                    return Err(refusal(
                        409,
                        format!("Reviewed link on bundle page {} needs review", source.2 + 1),
                    ));
                }
            }
        }
        Ok(PreparedBuild {
            inputs: self.inputs,
            sources,
            source_ids,
            locations,
            input_revisions: self.input_revisions,
            redactions: self.redactions,
            annotation_inputs: self.annotation_inputs,
            stamp: now(),
        })
    }
}

type Locations = HashMap<(String, usize), (usize, usize, usize)>;

pub(super) struct PreparedBuild {
    inputs: BuildInputs,
    sources: Vec<Source>,
    source_ids: HashMap<String, usize>,
    locations: Locations,
    input_revisions: Vec<Value>,
    redactions: Vec<Value>,
    annotation_inputs: Vec<Value>,
    stamp: String,
}

fn location(
    locations: &Locations,
    entry: &Value,
    id: &str,
    index: &str,
) -> Option<(usize, usize, usize)> {
    let index = usize::try_from(entry[index].as_u64()?).ok()?;
    locations
        .get(&(entry[id].as_str()?.to_owned(), index))
        .copied()
}

fn layout(marks: &Value) -> (usize, f64, f64) {
    if marks["index_style"] == "review" {
        (8, 220.0, 65.0)
    } else if marks["index_style"] == "readable" || marks["index_date"] == true {
        (16, 187.0, 35.0)
    } else {
        (23, 187.0, 25.0)
    }
}

fn winansi(value: &str) -> Result<Vec<u8>, Reply> {
    value
        .chars()
        .map(|c| match c {
            ' '..='~' | '\u{00a0}'..='\u{00ff}' => Ok(c as u8),
            '–' => Ok(0x96),
            '—' => Ok(0x97),
            '’' => Ok(0x92),
            '‘' => Ok(0x91),
            '“' => Ok(0x93),
            '”' => Ok(0x94),
            '…' => Ok(0x85),
            '€' => Ok(0x80),
            _ => Err(build_error(
                "Generated index text needs a qualified Unicode font",
            )),
        })
        .collect()
}

pub(super) fn visible_text(
    target: &mut Overlay,
    value: &str,
    font: &'static str,
    size: f64,
    x: f64,
    y: f64,
    ink: (f64, f64, f64),
) -> Result<(), Reply> {
    match winansi(value) {
        Ok(bytes) => append_text(target, font, size, x, y, ink, &bytes),
        Err(_) if value.chars().all(|character| !character.is_control()) => {
            target.unicode_lines.push(assembly::UnicodeLine {
                text: value.to_owned(),
                x: x as f32,
                y: y as f32,
                size: size as f32,
                bold: font.contains("Bold"),
                ink: (ink.0 as f32, ink.1 as f32, ink.2 as f32),
            });
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

fn label(
    target: &mut Overlay,
    value: &str,
    x: f64,
    top: f64,
    font_size: f64,
    bold: bool,
    color: (f64, f64, f64),
) -> Result<(), Reply> {
    let font = if bold { "Helvetica-Bold" } else { "Helvetica" };
    visible_text(target, value, font, font_size, x, PAGE_HEIGHT - top, color)
}

fn center(target: &mut Overlay, value: &str, top: f64, size: f64, bold: bool) -> Result<(), Reply> {
    let font = if bold { "Helvetica-Bold" } else { "Helvetica" };
    let width =
        f64::from(pdf_oxide::writer::FontManager::new().text_width(value, font, size as f32));
    let fitted = if width > 470.0 {
        size * 470.0 / width
    } else {
        size
    };
    label(
        target,
        value,
        (PAGE_WIDTH - width.min(470.0)) / 2.0,
        top,
        fitted,
        bold,
        INK,
    )
}

fn nonempty<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.is_empty() {
        fallback
    } else {
        value
    }
}

fn cover(matter: &Value, marks: &Value, volume: &Value, total: usize) -> Result<Overlay, Reply> {
    let mut overlay = Overlay::default();
    center(
        &mut overlay,
        nonempty(text(marks, "cover_heading"), "MATTER BUNDLE"),
        90.0,
        22.0,
        true,
    )?;
    center(
        &mut overlay,
        nonempty(
            text(marks, "cover_reference"),
            nonempty(text(matter, "case_number"), "Reference not recorded"),
        ),
        183.0,
        20.0,
        false,
    )?;
    overlay
        .commands
        .push_str("0.82 0.86 0.90 RG 65 592 m 530 592 l S\n");
    center(
        &mut overlay,
        nonempty(text(matter, "petitioner"), text(matter, "name")),
        330.0,
        28.0,
        true,
    )?;
    if !text(matter, "respondent").is_empty() {
        center(&mut overlay, "v.", 408.0, 20.0, false)?;
        center(&mut overlay, text(matter, "respondent"), 480.0, 28.0, true)?;
    }
    center(
        &mut overlay,
        nonempty(text(marks, "cover_title"), "DOCUMENT BUNDLE"),
        620.0,
        32.0,
        true,
    )?;
    center(
        &mut overlay,
        &format!("Volume {} of {total}", page(volume, "number")?),
        711.0,
        20.0,
        true,
    )?;
    center(
        &mut overlay,
        &format!("Physical pages 1–{}", page(volume, "page_count")?),
        751.0,
        16.0,
        false,
    )?;
    center(
        &mut overlay,
        "DRAFT — NOT COURT CERTIFIED",
        795.0,
        8.0,
        false,
    )?;
    Ok(overlay)
}

fn index(matter: &Value, marks: &Value, volume: &Value, index: usize) -> Result<Overlay, Reply> {
    let mut overlay = Overlay::default();
    let (capacity, top, step) = layout(marks);
    let rows = list(volume, "rows")?;
    let number = page(volume, "number")?;
    let review = marks["index_style"] == "review";
    let title = text(matter, "name").chars().take(95).collect::<String>();
    if review {
        center(
            &mut overlay,
            &format!("INDEX - VOLUME {number}"),
            92.0,
            25.0,
            true,
        )?;
    } else {
        overlay
            .commands
            .push_str("0.10 0.25 0.46 rg 0 832 595 10 re f\n");
        label(
            &mut overlay,
            "MATTERDRAFT / INDEX",
            50.0,
            50.0,
            10.0,
            true,
            BLUE,
        )?;
        label(
            &mut overlay,
            &format!("Volume {number} - Document Index"),
            50.0,
            82.0,
            19.0,
            true,
            INK,
        )?;
        label(&mut overlay, &title, 50.0, 104.0, 9.0, false, INK)?;
        label(&mut overlay, "PART", 50.0, 156.0, 9.0, true, INK)?;
    }
    let left = if review { 36.0 } else { 100.0 };
    let legend = if review {
        "Document"
    } else {
        "DOCUMENT / PARTICULARS"
    };
    label(
        &mut overlay,
        legend,
        left,
        156.0,
        if review { 18.0 } else { 9.0 },
        true,
        INK,
    )?;
    label(
        &mut overlay,
        "PAGES",
        485.0,
        156.0,
        if review { 18.0 } else { 9.0 },
        true,
        INK,
    )?;
    overlay
        .commands
        .push_str("0.7 0.75 0.8 RG 0.8 w 50 678 m 545 678 l S\n");
    for (offset, row) in rows
        .iter()
        .skip(index * capacity)
        .take(capacity)
        .enumerate()
    {
        let y = top + step * offset as f64;
        let prefix = text(row, "label");
        let title = if prefix.is_empty() {
            text(row, "title").to_owned()
        } else {
            format!("{prefix}  {}", text(row, "title"))
        };
        let printed = text(row, "printed_start");
        let end = text(row, "printed_end");
        let range = if printed == end {
            printed.to_owned()
        } else {
            format!("{printed}–{end}")
        };
        if !review {
            label(
                &mut overlay,
                &row["part"].to_string(),
                50.0,
                y,
                9.0,
                false,
                INK,
            )?;
        }
        let size = if review { 16.0 } else { 9.0 };
        let max = if review { 52 } else { 66 };
        label(
            &mut overlay,
            &title.chars().take(max).collect::<String>(),
            left,
            y,
            size,
            review,
            if review { BLUE } else { INK },
        )?;
        label(
            &mut overlay,
            &range,
            485.0,
            y,
            if review { 14.0 } else { 9.0 },
            false,
            INK,
        )?;
        if marks["index_date"] == true {
            label(
                &mut overlay,
                row["document_date"].as_str().unwrap_or("Date not recorded"),
                left,
                y + size + 2.0,
                size - 1.0,
                false,
                INK,
            )?;
        }
        if review || offset % 2 == 0 {
            let baseline = PAGE_HEIGHT
                - y
                - if review {
                    40.0
                } else if step == 35.0 {
                    16.0
                } else {
                    8.0
                };
            writeln!(
                overlay.commands,
                "0.92 0.93 0.95 RG 0.5 w 50 {baseline:.4} m 545 {baseline:.4} l S"
            )
            .unwrap();
        }
    }
    label(
        &mut overlay,
        "DRAFT - NOT COURT CERTIFIED",
        50.0,
        796.0,
        8.0,
        false,
        DRAFT,
    )?;
    Ok(overlay)
}

fn separator(row: &Value) -> Result<Overlay, Reply> {
    let mut overlay = Overlay::default();
    label(
        &mut overlay,
        "MATTERDRAFT / TECHNICAL SEPARATOR",
        50.0,
        52.0,
        10.0,
        true,
        BLUE,
    )?;
    center(&mut overlay, text(row, "label"), 204.0, 28.0, true)?;
    center(&mut overlay, text(row, "title"), 330.0, 16.0, false)?;
    label(
        &mut overlay,
        "DRAFT technical separator - review before filing",
        60.0,
        770.0,
        8.0,
        false,
        DRAFT,
    )?;
    Ok(overlay)
}

fn mark_box(
    dimensions: (f64, f64),
    value: &str,
    position: &str,
    size: f64,
) -> Result<([f64; 4], f64), Reply> {
    let (width, height) = dimensions;
    let measured = helvetica_mark_width(value, size)?;
    let margin = 12.0f64.max(width.min(height) * 0.02);
    let box_width = (width - margin * 2.0).min((size * 3.0).max(measured + 10.0));
    let box_height = size * 2.1;
    if box_width <= 0.0 || box_height > height {
        return Err(build_error("Output marks do not fit the selected page"));
    }
    let x = if position.ends_with("left") {
        margin
    } else if position.ends_with("right") {
        width - margin - box_width
    } else {
        (width - box_width) / 2.0
    };
    let top = if position.starts_with("top") {
        margin
    } else if position.starts_with("bottom") {
        height - margin - box_height
    } else {
        (height - box_height) / 2.0
    };
    Ok(([x, top, box_width, box_height], measured))
}

fn helvetica_mark_width(value: &str, size: f64) -> Result<f64, Reply> {
    use pdf_oxide::{fonts::FontInfo, PdfDocument};

    // The writer's layout estimates use approximate widths. The extraction
    // font model uses the standard glyph advances for the actual font we emit.
    static FONT: LazyLock<Result<FontInfo, &'static str>> = LazyLock::new(|| {
        let pdf =
            PdfDocument::from_bytes(assembly::blank((PAGE_WIDTH as f32, PAGE_HEIGHT as f32))?)
                .map_err(|_| "Cannot measure output marks")?;
        super::text_edit::standard_font(&pdf, "Helvetica")
            .map_err(|_| "Cannot measure output marks")
    });
    let Ok(encoded) = winansi(value) else {
        return Ok(f64::from(pdf_oxide::writer::FontManager::new().text_width(
            value,
            "Helvetica",
            size as f32,
        )));
    };
    let font = FONT.as_ref().map_err(|&error| build_error(error))?;
    Ok(super::text_edit::standard_text_width(font, &encoded, size))
}

struct OutputMark<'a> {
    kind: &'static str,
    text: &'a str,
    position: &'a str,
    size: f64,
    opacity: f64,
    rect: [f64; 4],
    text_width: f64,
}

impl OutputMark<'_> {
    fn write(&self, target: &mut Overlay, height: f64) -> Result<(), Reply> {
        let [left, top, width, _] = self.rect;
        if self.text_width > width {
            return Err(build_error("Output label does not fit the selected page"));
        }
        let x = left
            + if self.position.ends_with("left") {
                0.0
            } else if self.position.ends_with("right") {
                width - self.text_width
            } else {
                (width - self.text_width) / 2.0
            };
        if self.opacity < 1.0 {
            writeln!(&mut target.commands, "q /@G{} gs", target.opacity.len())
                .map_err(|_| build_error("Cannot write mark opacity"))?;
            target.opacity.push(self.opacity);
        }
        let ink = if self.kind == "watermark" {
            (0.42, 0.42, 0.42)
        } else {
            (0.22, 0.25, 0.3)
        };
        visible_text(
            target,
            self.text,
            "Helvetica",
            self.size,
            x,
            height - top - 1.075 * self.size,
            ink,
        )?;
        if self.opacity < 1.0 {
            target.commands.push_str("Q\n");
        }
        Ok(())
    }
}

fn output_mark_plan<'a>(
    size: (f64, f64),
    marks: &'a Value,
    source: bool,
    time: &'a str,
    bates: Option<&'a str>,
    label: Option<&'a str>,
) -> Result<Vec<OutputMark<'a>>, Reply> {
    let candidates = [
        (
            "watermark",
            "watermark",
            (source && marks["watermark_enabled"] == true)
                .then(|| text(marks, "watermark_text").trim()),
            32.0,
            marks["watermark_opacity"].as_f64().unwrap_or(0.16),
        ),
        (
            "timestamp",
            "timestamp",
            (source && marks["timestamp_enabled"] == true).then_some(time),
            8.0,
            1.0,
        ),
        ("bates", "bates", bates, 8.0, 1.0),
        ("annexure_label", "annexure_stamp", label, 10.0, 1.0),
    ];
    let mut plan = Vec::new();
    for (kind, setting, value, default_size, opacity) in candidates {
        let Some(value) = value.filter(|value| !value.is_empty()) else {
            continue;
        };
        let font_size = marks[format!("{setting}_font_size")]
            .as_f64()
            .unwrap_or(default_size);
        let position = text(marks, &format!("{setting}_position"));
        let (rect, text_width) = mark_box(size, value, position, font_size)?;
        plan.push(OutputMark {
            kind,
            text: value,
            position,
            size: font_size,
            opacity,
            rect,
            text_width,
        });
    }
    Ok(plan)
}

fn mark_obstacles(
    pdf: &pdf_oxide::PdfDocument,
    page: usize,
    geometry: assembly::Geometry,
) -> Result<Vec<[f64; 4]>, Reply> {
    let bounds = |rect: pdf_oxide::geometry::Rect| {
        [
            rect.x.into(),
            rect.y.into(),
            (rect.x + rect.width).into(),
            (rect.y + rect.height).into(),
        ]
    };
    let mut obstacles = pdf
        .extract_spans(page)
        .map_err(|_| build_error("Cannot inspect source text"))?
        .iter()
        .map(|span| super::text_edit::span_user_bbox(geometry, span))
        .collect::<Vec<_>>();
    obstacles.extend(
        pdf.page_image_handles(page)
            .map_err(|_| build_error("Cannot inspect source images"))?
            .iter()
            .map(|image| bounds(image.bbox)),
    );
    obstacles.extend(
        pdf.extract_paths(page)
            .map_err(|_| build_error("Cannot inspect source graphics"))?
            .iter()
            .map(|path| bounds(path.bbox)),
    );
    Ok(obstacles)
}

fn display_mark_box(frame: assembly::DisplayFrame, [x, top, width, height]: [f64; 4]) -> [f64; 4] {
    let bottom = frame.size.1 - top - height;
    assembly::transform_box(
        frame.paint_matrix(),
        [x, bottom, x + width, bottom + height],
    )
}

fn mark_overlaps(
    frame: assembly::DisplayFrame,
    rects: impl IntoIterator<Item = [f64; 4]>,
    mut obstacles: Vec<[f64; 4]>,
) -> Vec<bool> {
    rects
        .into_iter()
        .map(|rect| {
            let area = display_mark_box(frame, rect);
            let overlap = obstacles
                .iter()
                .any(|obstacle| super::text_edit::intersects(*obstacle, area));
            obstacles.push(area);
            overlap
        })
        .collect()
}

fn timestamp(marks: &Value, time: &str) -> Result<String, Reply> {
    let time = chrono::DateTime::parse_from_rfc3339(time)
        .map_err(|_| build_error("Invalid output timestamp"))?;
    let zone = jiff::tz::TimeZone::get(text(marks, "timestamp_timezone"))
        .map_err(|_| build_error("Invalid output time zone"))?;
    let formatted = jiff::Timestamp::from_second(time.timestamp())
        .map_err(|_| build_error("Invalid output timestamp"))?
        .to_zoned(zone);
    let format = if marks["timestamp_format"] == "date" {
        "%Y-%m-%d"
    } else {
        "%Y-%m-%d %H:%M %Z"
    };
    Ok(format!(
        "{} {}",
        text(marks, "timestamp_prefix").trim(),
        formatted.strftime(format)
    )
    .trim()
    .to_owned())
}

fn numbering_region(size: (f64, f64), marks: &Value, printed: &str) -> Result<[f64; 4], Reply> {
    let position = text(marks, "numbering_position");
    let inset = if position == "bottom_center" {
        24.0
    } else {
        12.0 * 72.0 / 25.4
    };
    let top = if position.starts_with("top") {
        inset
    } else {
        size.1 - inset - 11.0
    };
    let measured =
        f64::from(pdf_oxide::writer::FontManager::new().text_width(printed, "Helvetica", 9.0));
    if measured > size.0 - inset * 2.0 || top < 0.0 {
        return Err(build_error("Number label does not fit this page"));
    }
    let x = if position.ends_with("left") {
        inset
    } else if position.ends_with("right") {
        size.0 - inset - measured
    } else {
        (size.0 - measured) / 2.0
    };
    Ok([x, top, x + measured, top + 12.0])
}

fn number_matrix(frame: assembly::DisplayFrame, rect: [f64; 4]) -> [f64; 6] {
    let [left, top, right, bottom] = rect;
    let reference = frame.text_matrix(false);
    let [x0, y0, x1, y1] = assembly::transform_box(
        reference,
        [left, frame.size.1 - bottom, right, frame.size.1 - top],
    );
    let [left, bottom, right, top] = frame.visible.crop;
    if x0 >= left && y0 >= bottom && x1 <= right && y1 <= top {
        reference
    } else {
        // A large UserUnit or a rotated narrow sheet can put the reference
        // stamp off the visible page. Keep the planned label on the sheet.
        frame.visible_matrix()
    }
}

fn marks_overlay(
    target: &mut Overlay,
    frame: Option<assembly::DisplayFrame>,
    marks: &Value,
    entry: &Value,
    row: Option<&Value>,
    bates: &Value,
    time: &str,
) -> Result<Vec<[f64; 4]>, Reply> {
    let size = frame.map_or((PAGE_WIDTH, PAGE_HEIGHT), |frame| frame.size);
    let printed = text(entry, "printed_label");
    if !printed.is_empty() {
        let mut label = Box::new(Overlay::default());
        let region = numbering_region(size, marks, printed)?;
        if let Some(frame) = frame {
            position_output_text(&mut label, number_matrix(frame, region));
        }
        let [x, top, _, _] = region;
        visible_text(
            &mut label,
            printed,
            "Helvetica",
            9.0,
            x,
            size.1 - top - 1.075 * 9.0,
            (0.24, 0.28, 0.33),
        )?;
        target.page_number = Some(label);
    }
    let rendered = if row.is_some() && marks["timestamp_enabled"] == true {
        timestamp(marks, time)?
    } else {
        String::new()
    };
    let label = row
        .filter(|row| {
            row["annexure_mode"] == "label_stamp"
                && (marks["annexure_stamp_scope"] == "all" || entry["page_index"] == 0)
        })
        .and_then(|row| row["label"].as_str());
    let plan = output_mark_plan(size, marks, row.is_some(), &rendered, bates.as_str(), label)?;
    if plan.is_empty() {
        return Ok(Vec::new());
    }
    let mut output = Box::new(Overlay::default());
    if let Some(frame) = frame {
        position_output_text(&mut output, frame.text_matrix(true));
    }
    for mark in &plan {
        mark.write(&mut output, size.1)?;
    }
    target.output_marks = Some(output);
    Ok(plan.into_iter().map(|mark| mark.rect).collect())
}

fn position_output_text(target: &mut Overlay, [a, b, c, d, e, f]: [f64; 6]) {
    target.user_space_text = true;
    writeln!(target.commands, "{a} {b} {c} {d} {e:.7} {f:.7} cm").unwrap();
}

const BACK_INDEX_REGION: [f64; 4] = [34.0, 43.0, 130.0, 58.0];

fn back_index_stamp(overlay: &mut Overlay, frame: assembly::DisplayFrame) {
    position_output_text(overlay, frame.visible_matrix());
    append_text(
        overlay,
        "Helvetica",
        8.0,
        34.0,
        47.0,
        BLUE,
        b"Back to index",
    );
}

fn navigation_region(geometry: assembly::Geometry, bounds: [f64; 4]) -> [f64; 4] {
    let (width, height) = geometry.display_size();
    let [x0, y0, x1, y1] = super::text_edit::displayed(geometry, bounds);
    [
        x0 * width,
        (1.0 - y1) * height,
        x1 * width,
        (1.0 - y0) * height,
    ]
}

fn generated_mark_obstacles(
    source: &Source,
    overlay: &mut Overlay,
) -> Result<Vec<[f64; 4]>, Reply> {
    // Inspect generated index or separator content, including its page number,
    // in a one-page copy with the output marks layer temporarily omitted.
    let marks = overlay.output_marks.take();
    let mut overlays = HashMap::from([(0, std::mem::take(overlay))]);
    let bytes = assembly::assemble_bundle(
        std::slice::from_ref(source),
        &[(0, 0)],
        &overlays,
        &Navigation::default(),
    );
    *overlay = overlays.remove(&0).expect("generated page overlay");
    overlay.output_marks = marks;
    let pdf = pdf_oxide::PdfDocument::from_bytes(bytes.map_err(build_error)?)
        .map_err(|_| build_error("Cannot inspect generated page"))?;
    mark_obstacles(&pdf, 0, assembly::geometry(&pdf, 0).map_err(build_error)?)
}

#[derive(Default)]
struct OutputMap {
    pages: Vec<(usize, usize)>,
    overlays: HashMap<usize, Overlay>,
    navigation: Navigation,
    printed_labels: Vec<String>,
    label_regions: Vec<Value>,
    sources: BTreeMap<String, Value>,
    inserted_marks: usize,
    mark_overlaps: usize,
}

impl OutputMap {
    fn append_volume(
        &mut self,
        prepared: &PreparedBuild,
        volume: &Value,
        total: usize,
        combined: bool,
    ) -> Result<(), Reply> {
        let offset = self.pages.len();
        let marks = &prepared.inputs.marks;
        self.navigation.cleanup_metadata = marks["cleanup_metadata"] == true;
        let number = page(volume, "number")?;
        let cover_count = page(volume, "cover_pages")?;
        let index_count = page(volume, "index_pages")?;
        let rows = list(volume, "rows")?;
        let bates = list(volume, "bates_labels")?;
        let (rows_per_page, top, step) = layout(marks);
        let part_level = if combined { 2 } else { 1 };
        if combined {
            self.navigation
                .outlines
                .push((1, format!("Volume {number}"), offset));
        } else {
            if cover_count > 0 {
                self.navigation
                    .outlines
                    .push((1, format!("Volume {number}: Cover"), offset));
            }
            self.navigation.outlines.push((
                1,
                format!("Volume {number}: Index"),
                offset + cover_count,
            ));
        }
        let mut current_part = Value::Null;
        for (index, row) in rows.iter().enumerate() {
            let target = offset + page(row, "start_page")? - 1;
            if row["part"] != current_part {
                current_part = row["part"].clone();
                self.navigation
                    .outlines
                    .push((part_level, format!("Part {current_part}"), target));
            }
            if let Some(separator) = row["separator_page"].as_u64() {
                self.navigation.outlines.push((
                    part_level + 1,
                    format!("{} separator", text(row, "label")),
                    offset + separator as usize - 1,
                ));
            }
            self.navigation
                .outlines
                .push((part_level + 1, row_title(row), target));
            if marks["numbering_linked_index"] == true {
                let index_page = offset + cover_count + index / rows_per_page;
                let y = PAGE_HEIGHT - (top + step * (index % rows_per_page) as f64);
                let rect = if step == 65.0 {
                    [30.0, y - 38.0, 565.0, y + 22.0]
                } else if step == 35.0 {
                    [90.0, y - 15.0, 545.0, y + 20.0]
                } else {
                    [90.0, y - 8.0, 545.0, y + 14.0]
                };
                self.navigation
                    .links
                    .push((index_page, rect, LinkTarget::Page(target)));
            }
        }
        for (local, entry) in list(volume, "pages")?.iter().enumerate() {
            let physical = local + 1;
            let position = offset + local;
            let mut row = None;
            let (source, source_page, mut overlay) = if physical <= cover_count {
                (0, 0, cover(&prepared.inputs.matter, marks, volume, total)?)
            } else if physical <= cover_count + index_count {
                (
                    0,
                    0,
                    index(&prepared.inputs.matter, marks, volume, local - cover_count)?,
                )
            } else if let Some(document) = entry["document_id"].as_str() {
                let source = prepared
                    .source_ids
                    .get(document)
                    .ok_or_else(|| build_error("Page source differs from plan"))?;
                row = rows.iter().find(|row| {
                    row["id"] == document
                        && row["start_page"].as_u64().unwrap_or(u64::MAX) <= physical as u64
                        && row["end_page"].as_u64().unwrap_or(0) >= physical as u64
                });
                (*source, page(entry, "page_index")?, Overlay::default())
            } else {
                let separator_row = rows
                    .iter()
                    .find(|row| row["separator_page"] == physical)
                    .ok_or_else(|| build_error("Separator page differs from plan"))?;
                (0, 0, separator(separator_row)?)
            };
            let source_pdf = &prepared.sources[source];
            let frame = assembly::DisplayFrame::new(source_pdf.document(), source_page)
                .map_err(build_error)?;
            let size = frame.size;
            let mark_rects = marks_overlay(
                &mut overlay,
                Some(frame),
                marks,
                entry,
                row,
                if local < cover_count {
                    &Value::Null
                } else {
                    &bates[local]
                },
                &prepared.stamp,
            )?;
            let printed = text(entry, "printed_label");
            let number_region = (!printed.is_empty())
                .then(|| numbering_region(size, marks, printed))
                .transpose()?
                .map(|rect| {
                    let [x, top, right, bottom] = rect;
                    assembly::transform_box(
                        number_matrix(frame, rect),
                        [x, size.1 - bottom, right, size.1 - top],
                    )
                });
            if row.is_some() && marks["numbering_back_link"] == true {
                back_index_stamp(&mut overlay, frame);
                self.navigation.links.push((
                    position,
                    navigation_region(
                        frame.geometry,
                        assembly::transform_box(frame.visible_matrix(), BACK_INDEX_REGION),
                    ),
                    LinkTarget::Page(offset + cover_count),
                ));
            }
            if !combined && !mark_rects.is_empty() {
                let obstacles = if source == 0 {
                    generated_mark_obstacles(source_pdf, &mut overlay)?
                } else {
                    let mut obstacles =
                        mark_obstacles(source_pdf.document(), source_page, frame.geometry)?;
                    if let Some(region) = number_region {
                        obstacles.push(region);
                    }
                    if marks["numbering_back_link"] == true {
                        obstacles.push(assembly::transform_box(
                            frame.visible_matrix(),
                            BACK_INDEX_REGION,
                        ));
                    }
                    obstacles
                };
                self.inserted_marks += mark_rects.len();
                self.mark_overlaps += mark_overlaps(frame, mark_rects, obstacles)
                    .into_iter()
                    .filter(|overlap| *overlap)
                    .count();
            }
            self.pages.push((source, source_page));
            if !overlay.commands.is_empty()
                || !overlay.unicode_lines.is_empty()
                || overlay.page_number.is_some()
                || overlay.output_marks.is_some()
            {
                self.overlays.insert(position, overlay);
            }
            self.printed_labels.push(printed.into());
            self.label_regions.push(json!(number_region.map(|rect| {
                let (w, h) = frame.geometry.display_size();
                let [x0, y0, x1, y1] = super::text_edit::displayed(frame.geometry, rect);
                [x0 * w, y0 * h, x1 * w, y1 * h]
            })));
            if let Some(doc_id) = entry["document_id"].as_str() {
                let target_revision = prepared
                    .inputs
                    .docs
                    .iter()
                    .find(|doc| doc["id"] == doc_id)
                    .map_or(Value::Null, |doc| doc["revision"].clone());
                self.sources.insert(
                    position.to_string(),
                    json!({"document_id":doc_id,
                    "page_index":source_page,"target_revision":target_revision}),
                );
            }
            if marks["numbering_pdf_labels"] == true {
                self.navigation
                    .labels
                    .push(text(entry, "pdf_label").to_owned());
            }
        }
        Ok(())
    }

    fn reviewed_navigation(
        &mut self,
        prepared: &PreparedBuild,
        volume: Option<usize>,
    ) -> Result<(usize, usize), Reply> {
        let target = |location: (usize, usize, usize)| match volume {
            Some(number) if number == location.0 => Some(location.1),
            None => Some(location.2),
            _ => None,
        };
        let mut levels = HashMap::new();
        let mut manual = Vec::new();
        for item in &prepared.inputs.bookmarks {
            if item["resolved"] != true {
                continue;
            }
            let Some(page) =
                location(&prepared.locations, item, "document_id", "page_index").and_then(target)
            else {
                continue;
            };
            let id = text(item, "id");
            let level = levels.get(text(item, "parent_id")).copied().unwrap_or(1) + 1;
            levels.insert(id, level);
            manual.push((level, text(item, "title").to_owned(), page));
        }
        if let Some((_, _, first)) = manual.first() {
            self.navigation
                .outlines
                .push((1, "Manual bookmarks".into(), *first));
            self.navigation.outlines.extend(manual);
        }
        let mut retained = 0;
        let mut omitted = 0;
        for link in &prepared.inputs.links {
            if link["resolved"] != true {
                continue;
            }
            let Some(source) = location(
                &prepared.locations,
                link,
                "source_document_id",
                "source_page_index",
            )
            .and_then(target) else {
                continue;
            };
            let destination = if link["kind"] == "external" {
                LinkTarget::Uri(text(link, "uri").to_owned())
            } else {
                let Some(target) = location(
                    &prepared.locations,
                    link,
                    "target_document_id",
                    "target_page_index",
                )
                .and_then(target) else {
                    omitted += 1;
                    continue;
                };
                LinkTarget::Page(target)
            };
            let (document, page) = self.pages[source];
            let frame = assembly::DisplayFrame::new(prepared.sources[document].document(), page)
                .map_err(build_error)?;
            let (width, height) = frame.size;
            let val = |key: &str| link[key].as_f64().unwrap_or(0.0);
            let left = val("rect_x");
            let top = val("rect_y");
            let selection = [
                width * left,
                height * (1.0 - top - val("rect_height")),
                width * (left + val("rect_width")),
                height * (1.0 - top),
            ];
            let bounds = assembly::transform_box(frame.visible_matrix(), selection);
            self.navigation.links.push((
                source,
                navigation_region(frame.geometry, bounds),
                destination,
            ));
            retained += 1;
        }
        Ok((retained, omitted))
    }

    fn append_part_row(&mut self, combined: &Self, row: &Value) -> Result<(), Reply> {
        let start = page(row, "combined_start_page")?.checked_sub(1);
        let end = page(row, "combined_end_page")?;
        let count = page(row, "total_pages")?;
        let start = start
            .filter(|start| end.checked_sub(*start) == Some(count))
            .filter(|_| end <= combined.pages.len())
            .ok_or_else(|| build_error("Part extract differs from the saved page map"))?;
        self.navigation
            .outlines
            .push((1, row_title(row), self.pages.len()));
        for index in start..end {
            let position = self.pages.len();
            self.pages.push((0, index));
            self.printed_labels
                .push(combined.printed_labels[index].clone());
            self.label_regions
                .push(combined.label_regions[index].clone());
            let viewer_label = combined
                .navigation
                .labels
                .get(index)
                .cloned()
                .unwrap_or_else(|| (position + 1).to_string());
            self.navigation.labels.push(viewer_label);
            if let Some(source) = combined.sources.get(&index.to_string()) {
                self.sources.insert(position.to_string(), source.clone());
            }
        }
        Ok(())
    }

    fn contract(&self, sources: &[Source]) -> Result<Value, Reply> {
        let viewer_labels = if self.navigation.labels.is_empty() {
            (1..=self.pages.len())
                .map(|index| index.to_string())
                .collect()
        } else {
            self.navigation.labels.clone()
        };
        let mut links = BTreeMap::<String, Vec<Value>>::new();
        for (page, rect, target) in &self.navigation.links {
            let target = match target {
                LinkTarget::Page(index) => {
                    let (source_index, target_page) = self.pages.get(*index).ok_or_else(|| {
                        build_error("Link destination differs from the saved page map")
                    })?;
                    let source = sources
                        .get(*source_index)
                        .ok_or_else(|| build_error("Link destination source is unavailable"))?;
                    let [x, y] = super::output_navigation::generated_link_origin(
                        source.document(),
                        *target_page,
                    )
                    .map_err(build_error)?;
                    json!({"page":index,"rect":rect,"landing":["XYZ", x, y, 0]})
                }
                LinkTarget::Uri(uri) => json!({"uri":uri,"rect":rect}),
            };
            links.entry(page.to_string()).or_default().push(target);
        }
        Ok(
            json!({"pages": self.pages.len(), "toc": self.navigation.outlines.iter()
            .map(|(level,title,page)| json!([level,title,page+1])).collect::<Vec<_>>(),
            "links":links, "labels":viewer_labels, "printed_labels":self.printed_labels,
            "label_regions":self.label_regions,"sources":self.sources}),
        )
    }

    fn write(&self, sources: &[Source]) -> Result<Vec<u8>, Reply> {
        let bytes =
            assembly::assemble_bundle(sources, &self.pages, &self.overlays, &self.navigation)
                .map_err(build_error)?;
        let pdf = pdf_oxide::PdfDocument::from_bytes(bytes.clone())
            .map_err(|_| build_error("Generated PDF cannot be opened"))?;
        if pdf.page_count().ok() != Some(self.pages.len()) {
            return Err(build_error(
                "Generated PDF differs from the planned page count",
            ));
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod output_map_tests {
    use super::*;

    #[test]
    fn saved_links_keep_reference_landings_on_cropped_output_targets() {
        use crate::matters::output_navigation::inspect;
        use pdf_oxide::{editor::DocumentEditor, PdfDocument};

        let index = assembly::blank((595.0, 842.0)).unwrap();
        let original = assembly::blank((600.0, 800.0)).unwrap();
        let fixtures = [
            ([40.0, 80.0, 530.0, 750.0], [40.0, 750.0], 670.0),
            ([-35.0, -44.0, 678.0, 880.0], [23.0, 817.0], 924.0),
        ];
        for (crop, unrotated, rotated_height) in fixtures {
            for rotation in [0, 90, 180, 270] {
                let mut editor = DocumentEditor::from_bytes(original.clone()).unwrap();
                editor
                    .set_page_media_box(0, [23.0, 17.0, 623.0, 817.0])
                    .unwrap();
                editor.set_page_crop_box(0, crop).unwrap();
                editor.rotate_page_by(0, rotation).unwrap();
                let target = Source::open(editor.save_to_bytes().unwrap()).unwrap();
                let sources = [Source::open(index.clone()).unwrap(), target];
                let map = OutputMap {
                    pages: vec![(0, 0), (0, 0), (1, 0)],
                    printed_labels: vec![String::new(); 3],
                    navigation: Navigation {
                        links: vec![
                            (0, [20.0, 30.0, 55.0, 65.0], LinkTarget::Page(2)),
                            (1, [25.0, 40.0, 60.0, 75.0], LinkTarget::Page(2)),
                        ],
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut spec = map.contract(&sources).unwrap();
                let [x, y] = if rotation == 0 {
                    unrotated
                } else {
                    [0.0, rotated_height]
                };
                for page in ["0", "1"] {
                    assert_eq!(spec["links"][page][0]["page"], 2);
                    assert_eq!(spec["links"][page][0]["landing"], json!(["XYZ", x, y, 0]));
                }
                let written = PdfDocument::from_bytes(map.write(&sources).unwrap()).unwrap();
                assert!(inspect(&written, 3, &spec, false).unwrap().navigation);
                spec["links"]["1"][0]["landing"][2] = json!(y + 1.0);
                assert!(!inspect(&written, 3, &spec, false).unwrap().navigation);
            }
        }
    }

    #[test]
    fn physical_viewer_labels_follow_each_artifact_when_pdf_labels_are_disabled() {
        let map = |pages| OutputMap {
            pages: vec![(0, 0); pages],
            printed_labels: vec!["II".to_owned(); pages],
            ..Default::default()
        };
        assert_eq!(map(2).contract(&[]).unwrap()["labels"], json!(["1", "2"]));
        let combined = map(4);
        let spec = combined.contract(&[]).unwrap();
        assert_eq!(spec["labels"], json!(["1", "2", "3", "4"]));
        assert_eq!(spec["printed_labels"], json!(["II", "II", "II", "II"]));
        assert!(combined.navigation.labels.is_empty());
    }

    #[test]
    fn a_supplement_records_only_its_selected_page_identities_in_request_order() {
        let records = vec![json!({
            "document_id":"document","working_revision":3,"working_sha256":"working",
            "source_page_count":4,"page_ids":["first","second","third","fourth"]
        })];
        let supplement = json!({"kind":"supplement","selected_page_ids":["fourth","second"]});
        let prepared = revision_evidence(records, Some(&supplement));
        assert_eq!(
            prepared[0],
            json!({
                "document_id":"document","revision":3,"working_sha256":"working",
                "source_page_count":4,"page_ids":["fourth","second"]
            })
        );
    }

    #[test]
    fn package_order_and_size_failure_follow_the_export_artifact_list() {
        let names = [
            "combined.pdf",
            "volume-01.pdf",
            "combined-review.docx",
            "READ_ME_FIRST.txt",
        ];
        let files = names
            .iter()
            .map(|name| (name.to_string(), name.as_bytes().to_vec()))
            .collect::<BTreeMap<_, _>>();
        let package = zip_files(names, &files, || Ok(())).unwrap();
        let mut archive = zip::ZipArchive::new(Cursor::new(package)).unwrap();
        for (index, name) in names.into_iter().enumerate() {
            assert_eq!(archive.by_index(index).unwrap().name().unwrap(), name);
        }
        let cancelled = zip_files(names, &files, || Err(refusal(409, "Operation cancelled")));
        assert_eq!(cancelled.unwrap_err().status, 409);
        let size = file_size_error(1.5, &[("filing-package.zip", 1_572_865)]);
        assert_eq!(size.body["detail"],
            "Export exceeds the selected profile 1.5 MB per-file technical limit: filing-package.zip (1,572,865 bytes). No new export was published. Reduce the output or choose a reviewed workflow.");
    }
}

fn row_title(row: &Value) -> String {
    match text(row, "label") {
        "" => text(row, "title").to_owned(),
        prefix => format!("{prefix} - {}", text(row, "title")),
    }
}

pub(super) const CSV_FIELDS: &[&str] = &[
    "volume",
    "part",
    "kind",
    "label",
    "title",
    "source_start",
    "source_end",
    "separator_page",
    "start_page",
    "end_page",
    "total_pages",
    "printed_start",
    "printed_end",
    "combined_start_page",
    "combined_end_page",
];

pub(super) fn csv_cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

fn csv(rows: &[Value]) -> Vec<u8> {
    let mut output = format!("\u{feff}{}\r\n", CSV_FIELDS.join(","));
    for row in rows {
        let fields = CSV_FIELDS
            .iter()
            .map(|field| {
                let value = csv_cell(&row[*field]);
                if value.contains([',', '"', '\n', '\r']) {
                    format!("\"{}\"", value.replace('"', "\"\""))
                } else {
                    value
                }
            })
            .collect::<Vec<_>>();
        output.push_str(&fields.join(","));
        output.push_str("\r\n");
    }
    output.into_bytes()
}

fn zip_files<'a>(
    names: impl IntoIterator<Item = &'a str>,
    files: &BTreeMap<String, Vec<u8>>,
    checkpoint: impl Fn() -> Result<(), Reply>,
) -> Result<Vec<u8>, Reply> {
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for name in names {
        zip.start_file(name, options)
            .map_err(|_| build_error("Unable to create bundle ZIP"))?;
        for chunk in files[name].chunks(256 * 1024) {
            checkpoint()?;
            zip.write_all(chunk)
                .map_err(|_| build_error("Unable to write bundle ZIP"))?;
        }
    }
    zip.finish()
        .map(Cursor::into_inner)
        .map_err(|_| build_error("Unable to publish bundle ZIP"))
}

fn file_size_error(limit_mb: f64, oversized: &[(&str, usize)]) -> Reply {
    let details = oversized
        .iter()
        .map(|(name, bytes)| {
            let digits = bytes.to_string();
            let mut grouped = String::new();
            for (index, digit) in digits.chars().enumerate() {
                if index != 0 && (digits.len() - index) % 3 == 0 {
                    grouped.push(',');
                }
                grouped.push(digit);
            }
            format!("{name} ({grouped} bytes)")
        })
        .collect::<Vec<_>>()
        .join(", ");
    build_error(format!(
        "Export exceeds the selected profile {limit_mb} MB per-file technical limit: {details}. No new export was published. Reduce the output or choose a reviewed workflow."
    ))
}

impl PreparedBuild {
    pub(super) fn run(
        self,
        id: &str,
        progress: impl Fn(u8, &str) -> Result<(), Reply>,
    ) -> Result<Build, Reply> {
        progress(5, "Checking source documents")?;
        let volumes = list(&self.inputs.plan, "volumes")?;
        let mut files = BTreeMap::new();
        let mut pdf_names = vec!["combined.pdf".to_owned()];
        let mut combined = OutputMap::default();
        let mut rows = Vec::new();
        let mut reader_indexes = serde_json::Map::new();
        let mut validation_contract = serde_json::Map::new();
        let mut cross_volume_omissions = 0;
        let mut inserted_marks = 0;
        let mut mark_overlaps = 0;
        let mut total_bytes = 0usize;
        let max_volume = self.inputs.marks["volume_max_bytes"]
            .as_u64()
            .filter(|limit| *limit > 0);
        let max_file_mb = self.inputs.profile["max_file_mb"].as_f64();
        for volume in volumes {
            let number = page(volume, "number")?;
            progress(
                (10 + number * 60 / volumes.len()) as u8,
                &format!("Writing volume {number} of {}", volumes.len()),
            )?;
            let mut standalone = OutputMap::default();
            standalone.append_volume(&self, volume, volumes.len(), false)?;
            inserted_marks += standalone.inserted_marks;
            mark_overlaps += standalone.mark_overlaps;
            standalone.navigation.title = Some(format!(
                "{} - Volume {number}",
                text(&self.inputs.matter, "name")
            ));
            let (_, omitted) = standalone.reviewed_navigation(&self, Some(number))?;
            cross_volume_omissions += omitted;
            let filename = format!("volume-{number:02}.pdf");
            validation_contract.insert(filename.clone(), standalone.contract(&self.sources)?);
            let bytes = standalone.write(&self.sources)?;
            total_bytes = total_bytes.saturating_add(bytes.len());
            if total_bytes > BUILD_LIMIT {
                return Err(build_error("Build exceeds the Rust output limit"));
            }
            files.insert(filename.clone(), bytes);
            pdf_names.push(filename.clone());
            let index_page = page(volume, "cover_pages")? + 1;
            reader_indexes.insert(filename, json!(index_page));
            if number == 1 {
                reader_indexes.insert("combined.pdf".into(), json!(index_page));
            }
            combined.append_volume(&self, volume, volumes.len(), true)?;
            for row in list(volume, "rows")? {
                let mut row = row.clone();
                row["printed_labels"] = json!(list(volume, "pages")?
                    [page(&row, "start_page")? - 1..page(&row, "end_page")?]
                    .iter()
                    .map(|page| &page["printed_label"])
                    .collect::<Vec<_>>());
                rows.push(row);
            }
        }
        progress(75, "Writing combined PDF")?;
        let (combined_retained, _) = combined.reviewed_navigation(&self, None)?;
        combined.navigation.title = Some(format!(
            "{} - Combined Filing Bundle",
            text(&self.inputs.matter, "name")
        ));
        validation_contract.insert("combined.pdf".into(), combined.contract(&self.sources)?);
        let combined_bytes = combined.write(&self.sources)?;
        let mut extracts = Vec::new();
        let parts = list(&self.inputs.plan, "part_extracts")?;
        if !parts.is_empty() {
            let generated = [Source::open(combined_bytes.clone()).map_err(build_error)?];
            for item in parts {
                progress(80, "Writing technical Part extracts")?;
                let number = page(item, "part")?;
                let mut output = OutputMap::default();
                output.navigation.cleanup_metadata = combined.navigation.cleanup_metadata;
                output.navigation.title = Some(format!("Technical Part {number} extract"));
                for row in rows.iter().filter(|row| row["part"] == number) {
                    output.append_part_row(&combined, row)?;
                }
                let filename = format!("part-{number:02}.pdf");
                let contract = output.contract(&generated)?;
                validation_contract.insert(filename.clone(), contract.clone());
                files.insert(filename.clone(), output.write(&generated)?);
                pdf_names.push(filename.clone());
                let mut details = item.clone();
                details["filename"] = json!(filename);
                details["validation"] = contract;
                extracts.push(details);
            }
        }
        files.insert("combined.pdf".into(), combined_bytes);
        let mut size_exceptions = Vec::new();
        let mut processing = Vec::new();
        for name in &pdf_names {
            let bytes = files
                .get_mut(name)
                .ok_or_else(|| build_error("Generated artifact is missing"))?;
            let quality = text(&self.inputs.marks, "processing_quality");
            let before = bytes.len();
            let cleanup = self.inputs.marks["cleanup_metadata"] == true;
            let mut item = json!({"filename":name,"quality":quality,"before_bytes":before,"after_bytes":before,
                "compression":null,"metadata_cleared":false});
            let navigation_before = if !quality.is_empty() && quality != "original" {
                let before = pdf_oxide::PdfDocument::from_bytes(bytes.clone())
                    .map_err(|_| build_error("Unable to reopen generated PDF before processing"))?;
                let page_count = before
                    .page_count()
                    .map_err(|_| build_error("Unable to read generated PDF pages"))?;
                let navigation = super::output_navigation::navigation_evidence(&before, page_count)
                    .ok_or_else(|| build_error("Unable to read generated PDF navigation"))?;
                drop(before);
                let (output, report) =
                    super::compression::compress(std::mem::take(bytes), quality, |_, _| {
                        progress(85, "Processing generated PDFs")
                    })?;
                *bytes = output;
                item["after_bytes"] = json!(bytes.len());
                item["compression"] = report;
                Some(navigation)
            } else {
                None
            };
            let reopened = pdf_oxide::PdfDocument::from_bytes(bytes.clone())
                .map_err(|_| build_error("Unable to reopen processed PDF"))?;
            let count = reopened
                .page_count()
                .map_err(|_| build_error("Unable to read processed PDF pages"))?;
            if let Some(before) =
                navigation_before.filter(|_| item["compression"]["changed"] == true)
            {
                let after = super::output_navigation::navigation_evidence(&reopened, count);
                if after.as_ref() != Some(&before) {
                    return Err(build_error(
                        "Output processing changed text, page geometry, labels or navigation; no build published",
                    ));
                }
            }
            let contract = &validation_contract[name];
            let inspection =
                super::output_navigation::inspect(&reopened, count, contract, false)
                    .ok_or_else(|| build_error("Unable to inspect processed PDF navigation"))?;
            if contract["pages"] != count {
                return Err(build_error(format!(
                    "{name}: written PDF differs from the saved page count"
                )));
            }
            if !inspection.navigation {
                return Err(build_error(format!(
                    "{name}: written PDF destinations differ from the saved page map"
                )));
            }
            if !inspection.labels {
                let pages = inspection
                    .label_mismatches
                    .iter()
                    .map(|index| (index + 1).to_string())
                    .collect::<Vec<_>>();
                let detail = if pages.is_empty() {
                    "written viewer labels differ from the saved page map".to_owned()
                } else {
                    format!(
                        "written page numbers differ from the saved page map on pages {}",
                        pages.join(", ")
                    )
                };
                return Err(build_error(format!("{name}: {detail}")));
            }
            let metadata_cleared =
                super::output_navigation::metadata_cleared(&reopened, false).unwrap_or(false);
            if cleanup && !metadata_cleared {
                return Err(build_error("Output metadata cleanup verification failed"));
            }
            // Compression already compares reopened text and geometry against the input
            // before a smaller candidate can replace the generated bytes.
            if item["compression"].is_null() {
                for index in 0..count {
                    reopened
                        .extract_spans(index)
                        .map_err(|_| build_error("Unable to read generated PDF text"))?;
                    assembly::geometry(&reopened, index).map_err(build_error)?;
                }
            }
            item["metadata_cleared"] = json!(metadata_cleared);
            item["verified"] = json!([
                "extracted_text",
                "page_geometry",
                "page_labels",
                "bookmarks",
                "links"
            ]);
            if let Some(max_volume) = max_volume
                .filter(|limit| name.starts_with("volume-") && bytes.len() as u64 > *limit)
            {
                let number = name
                    .strip_prefix("volume-")
                    .and_then(|name| name.strip_suffix(".pdf"))
                    .and_then(|number| number.parse::<u64>().ok());
                let documents = rows
                    .iter()
                    .filter(|row| row["volume"].as_u64() == number)
                    .map(|row| row["id"].clone())
                    .collect::<Vec<_>>();
                size_exceptions.push(
                    json!({"filename":name,"bytes":bytes.len(),"limit_bytes":max_volume,
                    "scope":"individual_volume","document_ids":documents}),
                );
            }
            processing.push(item);
        }
        if !size_exceptions.is_empty() {
            return Err(reply(
                422,
                json!({
                    "detail":"Generated volume PDF exceeds the selected workflow byte limit. Review size recovery; no export was published.",
                    "recovery":{"input_snapshot":self.inputs.full_digest,"size_exceptions":size_exceptions}
                }),
            ));
        }
        files.insert("index.csv".into(), csv(&rows));
        let mut office = Vec::new();
        for (setting, kind, source) in [
            ("office_docx", "docx", "combined.pdf"),
            ("office_xlsx", "xlsx", "index.csv"),
            ("office_pptx", "pptx", "combined.pdf"),
        ] {
            if self.inputs.marks[setting] == true {
                let bytes = super::office_drafts::convert(source, kind, files[source].clone())
                    .map_err(|error| {
                        build_error(format!(
                            "{} review draft failed: {error}",
                            kind.to_ascii_uppercase()
                        ))
                    })?;
                let name = format!(
                    "{}-review.{kind}",
                    source.split('.').next().unwrap_or("bundle")
                );
                files.insert(name.clone(), bytes);
                office.push(name);
            }
        }
        let artifact = self
            .inputs
            .composition
            .as_ref()
            .map_or("primary", |plan| text(plan, "kind"));
        let signature_inputs = self
            .inputs
            .docs
            .iter()
            .map(|doc| {
                json!({
                    "document_id":doc["id"],"signature_status":doc["signature_status"],
                    "certificate_signature_status":doc["certificate_signature_status"],
                    "certificate_signature_count":doc["certificate_signature_count"],
                    "visible_signature_status":doc["visible_signature_status"],
                })
            })
            .collect::<Vec<_>>();
        let acknowledgement_needed =
            signature_guard(&self.inputs.docs, false, "building a bundle").is_err();
        let reviewed_link_inputs = self
            .inputs
            .links
            .iter()
            .map(|link| {
                let mut entry = bundle_inputs::select(
                    link,
                    &[
                        "id",
                        "title",
                        "source_document_id",
                        "source_page_index",
                        "source_revision",
                        "rect_x",
                        "rect_y",
                        "rect_width",
                        "rect_height",
                        "kind",
                        "target_document_id",
                        "target_page_index",
                        "target_revision",
                        "uri",
                        "position",
                        "resolved",
                    ],
                );
                if link["resolved"] != true {
                    entry["title"] = Value::Null;
                    entry["uri"] = Value::Null;
                }
                entry
            })
            .collect::<Vec<_>>();
        let unresolved_links = reviewed_link_inputs
            .iter()
            .filter(|link| link["resolved"] != true)
            .count();
        let unresolved_bookmarks = self
            .inputs
            .bookmarks
            .iter()
            .filter(|bookmark| bookmark["resolved"] != true)
            .count();
        let mut checks = self.inputs.checks;
        if rows.iter().any(|row| {
            text(row, "title")
                .chars()
                .chain(text(row, "label").chars())
                .any(|character| character > '~')
        }) {
            checks.push(json!({"severity":"warning","code":"NON_LATIN_INDEX","document_id":null,
                "message":"Printed index uses shaped fallback fonts for non-Latin titles. Visually inspect every generated row and saved PDF; text extraction and script coverage are not verified. PDF outlines retain Unicode titles."}));
        }
        if unresolved_bookmarks != 0 {
            checks.push(json!({"severity":"warning", "code":"UNRESOLVED_MANUAL_BOOKMARKS", "document_id":null,
                "message":format!("{unresolved_bookmarks} manual bookmark(s) target an older revision, removed document, or missing page and were omitted from this export. Resolve them before use.")}));
        }
        if unresolved_links != 0 {
            checks.push(json!({"severity":"warning", "code":"UNRESOLVED_REVIEWED_LINKS", "document_id":null,
                "message":format!("{unresolved_links} reviewed link(s) target an older revision, removed document, or missing page and were omitted from this export. Resolve them before use.")}));
        }
        if cross_volume_omissions != 0 {
            checks.push(json!({"severity":"warning", "code":"CROSS_VOLUME_LINKS_COMBINED_ONLY", "document_id":null,
                "message":format!("{cross_volume_omissions} reviewed internal link occurrence(s) crossed a volume boundary. They are active in combined.pdf and omitted from standalone volumes to avoid broken destinations.")}));
        }
        if mark_overlaps != 0 {
            checks.push(json!({"severity":"warning", "code":"OUTPUT_MARK_OVERLAP", "document_id":null,
                "message":format!("{mark_overlaps} output mark placement(s) intersect detectable text, image, drawing, link or generated page-number content. Review every marked page; signatures and seals are not reliably detectable.")}));
        }
        checks.push(json!({"severity":"info","code":"NOT_A_FILING_APPROVAL","document_id":null,
            "message":"Technical checks passed or reported. Filing acceptance, legal validity and registry objections need human review."}));
        let manual_bookmark_inputs = self
            .inputs
            .bookmarks
            .iter()
            .map(|entry| {
                let mut input = bundle_inputs::select(
                    entry,
                    &[
                        "id",
                        "title",
                        "document_id",
                        "page_index",
                        "target_revision",
                        "parent_id",
                        "position",
                    ],
                );
                if entry["resolved"] != true {
                    input["title"] = Value::Null;
                }
                input
            })
            .collect::<Vec<_>>();
        let membership = match &self.inputs.composition {
            Some(plan) if plan["kind"] == "supplement" => json!({
                "bundle_id":plan["id"],"revision":plan["version"],"selection_mode":"exact_revision_pages",
                "selected_page_ids":plan["selected_page_ids"],
                "documents":self.input_revisions.iter().map(|record| json!({
                    "document_id":record["document_id"],"working_revision":record["revision"],
                    "source_page_count":record["source_page_count"],"selected_page_ids":record["page_ids"],
                })).collect::<Vec<_>>(),
            }),
            Some(plan) => json!({
                "bundle_id":plan["id"],"revision":plan["version"],"selection_mode":"whole_documents",
                "documents":plan["sources"],
            }),
            None => json!({
                "bundle_id":format!("{}:primary", self.inputs.matter["id"].as_str().unwrap_or_default()),
                "revision":self.inputs.membership_revision.max(1),"selection_mode":"whole_documents",
                "documents":self.inputs.docs.iter().map(|doc| json!({"document_id":doc["id"],
                    "working_revision":doc["revision"],"source_page_count":doc["page_count"],
                    "position":doc["position"]})).collect::<Vec<_>>(),
            }),
        };
        let page_map_snapshot = rows
            .iter()
            .map(|row| {
                bundle_inputs::select(
                    row,
                    &[
                        "id",
                        "part",
                        "label",
                        "title",
                        "volume",
                        "start_page",
                        "end_page",
                        "combined_start_page",
                        "combined_end_page",
                        "printed_start",
                        "printed_end",
                        "source_start",
                        "source_end",
                        "printed_labels",
                    ],
                )
            })
            .collect::<Vec<_>>();
        let mut manifest = json!({
            "id":id,"created_at":self.stamp,"matter_id":self.inputs.matter["id"],"artifact_kind":artifact,
            "pages":self.inputs.plan["combined_pages"],"volumes":volumes.len(),
            "matter_updated_at":self.inputs.matter["updated_at"],"input_revisions":self.input_revisions,
            "label_inputs":self.inputs.docs.iter().map(|doc|json!({
                "document_id":doc["id"],"position":doc["position"],"label":doc["label"],"label_status":doc["label_status"],
                "label_source":doc["label_source"],"group_key":doc["group_key"],"group_role":doc["group_role"]
            })).collect::<Vec<_>>(),
            "output_marks":self.inputs.marks,"build_input_digest":if self.inputs.composition.is_some() {
                &self.inputs.digest
            } else { &self.inputs.full_digest },
            "effective_input_digest":self.inputs.digest,"reader_indexes":reader_indexes,"filing_review":self.inputs.filing_review,
            "manual_bookmark_inputs":manual_bookmark_inputs,"checks":checks,"page_map_snapshot":page_map_snapshot,
            "part_extracts":extracts,"processing":processing,"office_review_files":office,
            "signature_inputs":signature_inputs,"signature_risk_acknowledged":self.inputs.acknowledged && acknowledgement_needed,
            "reviewed_link_inputs":reviewed_link_inputs,"page_annotation_inputs":self.annotation_inputs,
            "output_preset":self.inputs.preset,
            "bundle_membership":membership,
            "validation_contract":validation_contract,"validation_profile":self.inputs.profile,
            "redaction_inputs":self.redactions,
            "output_mark_timestamp":self.stamp,"archived_at":null,"stale":false,
            "build_settings":{"max_pages_per_volume":self.inputs.matter["max_pages_per_volume"],
                "keep_groups_intact":self.inputs.matter["keep_groups_intact"],"court_profile":self.inputs.matter["court_profile"]},
        });
        for key in [
            "variant_plan_id",
            "variant_plan_version",
            "variant_name",
            "variant_baseline",
            "supplement_plan_id",
            "supplement_plan_version",
            "baseline_build_id",
            "insertion_policy",
        ] {
            manifest[key] = Value::Null;
        }
        manifest["insertion_policy_contract"] = self.inputs.insertion_policy_contract.clone();
        manifest["interactive_content_policy"] = interactive_content_policy();
        if let Some(composition) = &self.inputs.composition {
            let kind = text(composition, "kind");
            manifest[format!("{kind}_plan_id")] = composition["id"].clone();
            manifest[format!("{kind}_plan_version")] = composition["version"].clone();
            if kind == "variant" {
                manifest["variant_name"] = composition["name"].clone();
                manifest["variant_baseline"] = composition["baseline"].clone();
            } else {
                manifest["baseline_build_id"] = composition["baseline_build_id"].clone();
                manifest["insertion_policy"] = composition["insertion_policy"].clone();
            }
            manifest["build_settings"] = composition["settings"].clone();
            let settings = &mut manifest["build_settings"];
            settings["court_profile"] = self.inputs.matter["court_profile"].clone();
            settings[format!("{kind}_plan_id")] = composition["id"].clone();
            settings[format!("{kind}_plan_version")] = composition["version"].clone();
            if kind == "variant" {
                settings["baseline"] = composition["baseline"].clone();
            } else {
                settings["baseline_build_id"] = composition["baseline_build_id"].clone();
                settings["insertion_policy"] = composition["insertion_policy"].clone();
            }
        }
        let mut report = bundle_inputs::select(
            &manifest,
            &[
                "matter_id",
                "artifact_kind",
                "created_at",
                "matter_updated_at",
                "input_revisions",
                "label_inputs",
                "bundle_membership",
                "signature_inputs",
                "signature_risk_acknowledged",
                "build_input_digest",
                "build_settings",
                "manual_bookmark_inputs",
                "reviewed_link_inputs",
                "page_annotation_inputs",
                "output_marks",
                "output_preset",
                "processing",
                "office_review_files",
                "output_mark_timestamp",
                "part_extracts",
                "variant_plan_id",
                "variant_plan_version",
                "variant_name",
                "variant_baseline",
                "supplement_plan_id",
                "supplement_plan_version",
                "baseline_build_id",
                "insertion_policy",
                "interactive_content_policy",
                "checks",
            ],
        );
        report["policy_preview"] = if self.inputs.insertion_policy_contract.is_null() {
            Value::Null
        } else {
            bundle_inputs::select(
                &self.inputs.insertion_policy_contract,
                &[
                    "mode",
                    "summary",
                    "preview_rows",
                    "volume_boundaries",
                    "effects",
                ],
            )
        };
        report["reviewed_link_summary"] = json!({
            "combined_retained":combined_retained,
            "standalone_cross_volume_omissions":cross_volume_omissions,
            "unresolved":unresolved_links,
        });
        report["output_mark_summary"] =
            json!({"inserted":inserted_marks, "overlaps_content":mark_overlaps});
        report["bates_policy"] = json!({
            "enabled":self.inputs.marks["bates_enabled"],
            "include_index":self.inputs.marks["bates_include_index"],
            "include_separator":self.inputs.marks["bates_include_separator"],
            "restart_per_volume":self.inputs.marks["bates_restart_per_volume"],
        });
        let marks = &self.annotation_inputs;
        let printed = marks
            .iter()
            .filter(|item| item["output_scope"] == "pdf" && item["resolved"] == true);
        report["page_annotation_summary"] = json!({
            "image_overlays_inserted":printed.clone().filter(|item| item["kind"] == "image").count(),
            "editable_marks_inserted":printed.clone().filter(|item| item["kind"] != "image" && item["kind"] != "redaction").count(),
            "redactions_applied":printed.filter(|item| item["kind"] == "redaction").count(),
            "private_review_annotations_omitted":marks.iter().filter(|item| item["output_scope"] == "private").count(),
            "unresolved_image_overlays":marks.iter().filter(|item| item["kind"] == "image" && item["resolved"] != true).count(),
        });
        report.as_object_mut().expect("report object").extend(
            json!({
                "export_id":id,"draft":true,"profile":self.inputs.profile["id"],"profile_status":self.inputs.profile["status"],
                "volume_count":volumes.len(),"page_map":rows,"combined_pages":self.inputs.plan["combined_pages"],
                "combined_bytes":files["combined.pdf"].len(),"total_input_documents":self.inputs.docs.len(),
                "selected_source_pages":self.inputs.composition.as_ref()
                    .and_then(|plan| plan["selected_page_ids"].as_array()).map(Vec::len),
                "volumes":volumes.iter().map(|volume| json!({"name": format!("volume-{:02}.pdf", volume["number"].as_u64().unwrap_or_default()),
                    "items":volume["rows"].as_array().map_or(0,Vec::len)})).collect::<Vec<_>>()
            }).as_object().expect("report fields").clone(),
        );
        files.insert(
            "validation-report.json".into(),
            serde_json::to_vec_pretty(&report)
                .map_err(|_| build_error("Unable to write validation report"))?,
        );
        files.insert(
            "READ_ME_FIRST.txt".into(),
            draft_readme(&manifest["artifact_kind"]).into_bytes(),
        );
        if files.values().map(Vec::len).sum::<usize>() > BUILD_LIMIT {
            return Err(build_error("Build exceeds the Rust output limit"));
        }
        progress(90, "Packaging export ZIP")?;
        let names = pdf_names
            .iter()
            .chain(&office)
            .map(String::as_str)
            .chain(["index.csv", "validation-report.json"]);
        let package = zip_files(names.clone().chain(["READ_ME_FIRST.txt"]), &files, || {
            progress(90, "Packaging export ZIP")
        })?;
        files.insert("filing-package.zip".into(), package);
        if files.values().map(Vec::len).sum::<usize>() > BUILD_LIMIT {
            return Err(build_error("Build package exceeds the Rust output limit"));
        }
        let names = std::iter::once("filing-package.zip").chain(names);
        if let Some(limit) = max_file_mb {
            let oversized = names
                .clone()
                .map(|name| (name, files[name].len()))
                .filter(|(_, bytes)| *bytes as f64 > limit * 1024.0 * 1024.0)
                .collect::<Vec<_>>();
            if !oversized.is_empty() {
                return Err(file_size_error(limit, &oversized));
            }
        }
        manifest["files"] = json!(names
            .map(|name| {
                let bytes = &files[name];
                json!({"name":name,"bytes":bytes.len(),"sha256":sha256_hex(bytes)})
            })
            .collect::<Vec<_>>());
        progress(95, "Checking generated output")?;
        let matter = self.inputs.matter["id"]
            .as_str()
            .ok_or_else(|| build_error("Build matter is missing"))?;
        let initial_check =
            super::generated_exports::measure_build(matter, &manifest, None, |name| {
                progress(95, "Checking generated output")?;
                files
                    .get(name)
                    .cloned()
                    .ok_or_else(|| build_error("Generated artifact is missing"))
            })?;
        progress(98, "Publishing reviewed-copy artifacts")?;
        Ok(Build {
            manifest,
            files,
            initial_check,
        })
    }
}

fn preview_image(bytes: Vec<u8>) -> Result<(String, String), Reply> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let failed = || build_error("Unable to render generated PDF preview");
    let pdf = pdf_oxide::PdfDocument::from_bytes(bytes).map_err(|_| failed())?;
    let image = super::preview::render_at_scale(&pdf, 0, Some(1.2)).map_err(|_| failed())?;
    let text = pdf.extract_text(0).map_err(|_| failed())?;
    Ok((
        format!("data:image/png;base64,{}", STANDARD.encode(image)),
        text,
    ))
}

enum PreviewOperation {
    FrontMatter(String),
    Numbering(String),
    PageMarks { document: String, page: String },
}

pub struct OutputPreviewTarget(PreviewOperation);

impl OutputPreviewTarget {
    pub fn from_path(method: &str, path: &str) -> Option<Self> {
        let route = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
        Self::select(method, &route)
    }

    fn select(method: &str, route: &[&str]) -> Option<Self> {
        let operation = match (method, route) {
            ("POST", ["matters", matter, "front-matter-preview"]) => {
                PreviewOperation::FrontMatter((*matter).to_owned())
            }
            ("POST", ["matters", matter, "numbering-preview"]) => {
                PreviewOperation::Numbering((*matter).to_owned())
            }
            ("POST", ["documents", id, "pages", page, "output-marks-preview"]) => {
                PreviewOperation::PageMarks {
                    document: (*id).to_owned(),
                    page: (*page).to_owned(),
                }
            }
            _ => return None,
        };
        Some(Self(operation))
    }
}

struct PreviewSource {
    document: String,
    index: usize,
    revision: u64,
    pdf: PdfSource,
}

struct PreviewSourceProof {
    document: String,
    revision: u64,
    proof: PdfProof,
}

struct NumberingSource {
    page: PreviewSource,
    document: Value,
    annotations: Vec<Value>,
    assets: HashMap<String, Vec<u8>>,
}

enum OutputPreviewLayout {
    FrontMatter { volume: u64, index_page: u64 },
    Numbering(Result<NumberingSource, Reply>),
    PageMarks(Result<PreviewSource, Reply>),
}

pub struct OutputPreviewWork {
    root: PathBuf,
    matter: String,
    inputs: BundleInputWork,
    snapshot: String,
    settings: Preview,
    marks: Value,
    layout: OutputPreviewLayout,
}

pub struct MeasuredOutputPreview {
    root: PathBuf,
    matter: String,
    snapshot: String,
    proofs: Vec<DocumentFileProof>,
    source: Option<PreviewSourceProof>,
    output: Value,
}

fn numbering_unavailable() -> Reply {
    build_error("Select a page in a current included working document. The target is missing, excluded or changed.")
}

fn preview_disk_error(_: rusqlite::Error) -> Reply {
    refusal(500, "Unable to load output mark settings")
}

fn front_matter_pages(
    mut plan: Value,
    owner: &Value,
    marks: Value,
    timestamp: &str,
    requested_volume: u64,
    requested_index: u64,
) -> Result<Value, Reply> {
    let volumes = list(&plan, "volumes")?;
    let mut pages = Vec::new();
    let mut chosen = None;
    if !volumes.is_empty() {
        let volume = requested_volume.clamp(1, volumes.len() as u64) as usize;
        let selected = &volumes[volume - 1];
        let index_page = requested_index.clamp(1, page(selected, "index_pages")? as u64) as usize;
        let cover_count = page(selected, "cover_pages")?;
        let mut overlays = Vec::new();
        if cover_count > 0 {
            overlays.push(("cover", 1, cover(owner, &marks, selected, volumes.len())?));
        }
        overlays.push((
            "index",
            cover_count + index_page,
            index(owner, &marks, selected, index_page - 1)?,
        ));
        for (kind, physical, mut overlay) in overlays {
            let record = &list(selected, "pages")?[physical - 1];
            let bates = &list(selected, "bates_labels")?[physical - 1];
            marks_overlay(&mut overlay, None, &marks, record, None, bates, timestamp)?;
            let bytes = assembly::overlay_document(
                assembly::blank((595.0, 842.0)).map_err(build_error)?,
                &HashMap::from([(0, overlay)]),
            )
            .map_err(build_error)?;
            let (image, text) = preview_image(bytes)?;
            pages.push(json!({"kind":kind,"physical_page":physical,"text":text,"image":image}));
        }
        chosen = Some((volume, index_page));
    }
    plan["output_marks"] = marks;
    plan["pages"] = json!(pages);
    if let Some((volume, index)) = chosen {
        plan["selected_volume"] = json!(volume);
        plan["selected_index_page"] = json!(index);
    }
    Ok(plan)
}

fn numbering_page(
    plan: &Value,
    marks: &Value,
    timestamp: &str,
    source: NumberingSource,
) -> Result<(Value, PreviewSourceProof), Reply> {
    let NumberingSource {
        page: source,
        document: doc,
        annotations,
        mut assets,
    } = source;
    let document = source.document.clone();
    let index = source.index;
    let volumes = list(plan, "volumes")?;
    let unavailable = numbering_unavailable;
    let (volume, record) = volumes
        .iter()
        .find_map(|volume| {
            let record = volume["pages"].as_array()?.iter().find(|record| {
                record["document_id"] == document.as_str() && record["page_index"] == index
            })?;
            Some((volume, record))
        })
        .ok_or_else(unavailable)?;
    let physical = page(record, "physical_page")?;
    let bates = &list(volume, "bates_labels")?[physical - 1];
    let row = list(volume, "rows")?.iter().find(|row| {
        row["id"] == document.as_str()
            && row["start_page"].as_u64().unwrap_or(u64::MAX) <= physical as u64
            && row["end_page"].as_u64().unwrap_or(0) >= physical as u64
    });
    let (content, proof) = source.pdf.read_with_proof(|| false)?;
    let source_proof = PreviewSourceProof {
        document: source.document,
        revision: source.revision,
        proof,
    };
    let source = Source::open(content).map_err(build_error)?;
    let frame = assembly::DisplayFrame::new(source.document(), index).map_err(build_error)?;
    let selection = assembly::assemble_bundle(
        &[source],
        &[(0, index)],
        &HashMap::new(),
        &Navigation::default(),
    )
    .map_err(build_error)?;
    let bytes = flatten_with_assets(selection, &annotations, |id| Ok(assets.remove(id)))
        .map_err(build_error)?;
    let mut overlay = Overlay::default();
    marks_overlay(
        &mut overlay,
        Some(frame),
        &marks,
        record,
        row,
        bates,
        &timestamp,
    )?;
    if marks["numbering_back_link"] == true {
        back_index_stamp(&mut overlay, frame);
    }
    let bytes =
        assembly::overlay_document(bytes, &HashMap::from([(0, overlay)])).map_err(build_error)?;
    let (image, extracted) = preview_image(bytes)?;
    let mut answer = record.clone();
    answer["volume"] = volume["number"].clone();
    answer["document_title"] = doc["title"].clone();
    answer["source_pages"] = doc["page_count"].clone();
    answer["combined_pages"] = plan["combined_pages"].clone();
    answer["bates"] = bates.clone();
    answer["image"] = json!(image);
    answer["text"] = json!(extracted);
    answer["input_snapshot"] = plan["input_snapshot"].clone();
    Ok((answer, source_proof))
}

fn output_mark_boxes(
    plan: &Value,
    marks: &Value,
    stamp: &str,
    source: PreviewSource,
) -> Result<(Value, PreviewSourceProof), Reply> {
    let document = source.document.clone();
    let index = source.index;
    let revision = source.revision;
    let (mut bates, mut label) = (None, None);
    for volume in list(&plan, "volumes")? {
        if let Some(record) = list(volume, "pages")?.iter().find(|record| {
            record["document_id"] == document.as_str() && record["page_index"] == index
        }) {
            let physical = page(record, "physical_page")?;
            bates = list(volume, "bates_labels")?
                .get(physical - 1)
                .and_then(Value::as_str);
            if let Some(row) = list(volume, "rows")?.iter().find(|row| {
                row["id"] == document.as_str()
                    && row["start_page"].as_u64().unwrap_or(u64::MAX) <= physical as u64
                    && row["end_page"].as_u64().unwrap_or(0) >= physical as u64
            }) {
                if row["annexure_mode"] == "label_stamp"
                    && (index == 0 || marks["annexure_stamp_scope"] == "all")
                {
                    label = row["label"].as_str();
                }
            }
            break;
        }
    }
    let (bytes, proof) = source.pdf.read_with_proof(|| false)?;
    let proof = PreviewSourceProof {
        document: source.document,
        revision,
        proof,
    };
    let pdf = pdf_oxide::PdfDocument::from_bytes(bytes)
        .map_err(|_| build_error("Cannot open working PDF"))?;
    let frame = assembly::DisplayFrame::new(&pdf, index).map_err(build_error)?;
    let (width, height) = frame.size;
    let rendered = if marks["timestamp_enabled"] == true {
        timestamp(&marks, stamp)?
    } else {
        String::new()
    };
    let plan = output_mark_plan((width, height), &marks, true, &rendered, bates, label)?;
    let mut obstacles = mark_obstacles(&pdf, index, frame.geometry)?;
    obstacles.extend(
        super::text_edit::selected_annotation_boxes(&pdf, index, |kind| kind != Some("Link"))
            .unwrap_or_default(),
    );
    let (raw_width, raw_height) = frame.visible.display_size();
    let [left, _, _, top] = frame.visible.crop;
    let links = super::page_tools::pdf_annotations::link_rectangles(&pdf, index)
        .map_err(|_| build_error("Cannot inspect source links"))?;
    // Reference link rectangles are already rotated into display coordinates
    // when compared to the planned, derotated mark and source content.
    obstacles.extend(links.into_iter().map(|bounds| {
        let [x0, y0, x1, y1] = super::text_edit::displayed(frame.visible, bounds);
        [
            left + raw_width * x0,
            top - raw_height * y1,
            left + raw_width * x1,
            top - raw_height * y0,
        ]
    }));
    let overlaps = mark_overlaps(frame, plan.iter().map(|mark| mark.rect), obstacles);
    let preview = plan.iter().zip(overlaps).map(|(mark, overlap)| {
        let [x, y, box_width, box_height] = mark.rect;
        json!({"kind":mark.kind,"text":mark.text,"font_size":mark.size,"position":mark.position,"opacity":mark.opacity,
            "overlaps_content":overlap,"relative_rect":[x/width,y/height,box_width/width,box_height/height]})
    }).collect::<Vec<_>>();
    Ok((
        json!({"revision":revision,"page_index":index,"marks":preview}),
        proof,
    ))
}

impl OutputPreviewWork {
    pub fn compute(self) -> Result<MeasuredOutputPreview, Reply> {
        let Self {
            root,
            matter,
            inputs,
            snapshot,
            settings,
            marks,
            layout,
        } = self;
        let (inputs, proofs) = inputs.verify()?;
        let (owner, docs, _) = inputs.generation_inputs();
        let selected = docs.into_iter().cloned().collect::<Vec<_>>();
        let response = inputs
            .composition_plan(settings, &marks, &selected)
            .map_err(preview_disk_error)?;
        if response.status != 200 {
            return Err(response);
        }
        let mut plan = response.body;
        plan["input_snapshot"] = json!(snapshot);
        let stamp = now();
        let (output, source) = match layout {
            OutputPreviewLayout::FrontMatter { volume, index_page } => (
                front_matter_pages(plan, owner, marks, &stamp, volume, index_page)?,
                None,
            ),
            OutputPreviewLayout::Numbering(source) => {
                let (output, proof) = numbering_page(&plan, &marks, &stamp, source?)?;
                (output, Some(proof))
            }
            OutputPreviewLayout::PageMarks(source) => {
                let (output, proof) = output_mark_boxes(&plan, &marks, &stamp, source?)?;
                (output, Some(proof))
            }
        };
        Ok(MeasuredOutputPreview {
            root,
            matter,
            snapshot,
            proofs,
            source,
            output,
        })
    }
}

impl Store {
    fn preview_source(
        &self,
        document: &str,
        index: usize,
        revision: u64,
    ) -> Result<PreviewSource, Reply> {
        let pdf = self.pdf_source(
            document,
            &Artifact::Working {
                revision: Some(revision),
            },
        )?;
        Ok(PreviewSource {
            document: document.to_owned(),
            index,
            revision,
            pdf,
        })
    }

    fn numbering_source(
        &self,
        matter: &str,
        docs: &[&Value],
        body: &Value,
    ) -> Result<NumberingSource, Reply> {
        let document = text(body, "document_id");
        let index = page(body, "page")?
            .checked_sub(1)
            .ok_or_else(|| build_error("Invalid source page"))?;
        let revision = body["expected_revision"]
            .as_u64()
            .ok_or_else(numbering_unavailable)?;
        let doc = docs
            .iter()
            .copied()
            .find(|doc| doc["id"] == document && doc["revision"] == revision)
            .filter(|doc| {
                doc["page_count"]
                    .as_u64()
                    .is_some_and(|pages| (index as u64) < pages)
            })
            .ok_or_else(numbering_unavailable)?;
        let source = self.preview_source(document, index, revision)?;
        let annotations = super::annotations::list(&self.con, matter)
            .map_err(preview_disk_error)?
            .into_iter()
            .filter(|item| {
                item["document_id"] == document
                    && item["page_index"] == index
                    && item["output_scope"] == "pdf"
                    && item["resolved"] == true
            })
            .map(|mut item| {
                item["page_index"] = json!(0);
                item
            })
            .collect::<Vec<_>>();
        let mut assets = HashMap::new();
        for item in &annotations {
            if item["kind"] == "image" {
                let id = text(item, "id");
                let bytes = super::annotations::asset(&self.con, id)
                    .map_err(preview_disk_error)?
                    .ok_or_else(|| build_error("Image overlay asset missing"))?;
                assets.insert(id.to_owned(), bytes);
            }
        }
        Ok(NumberingSource {
            page: source,
            document: doc.clone(),
            annotations,
            assets,
        })
    }

    pub fn prepare_output_preview(
        &self,
        target: OutputPreviewTarget,
        body: &Value,
    ) -> Result<OutputPreviewWork, Reply> {
        let OutputPreviewTarget(operation) = target;
        let (matter, mark_page) = match &operation {
            PreviewOperation::FrontMatter(matter) | PreviewOperation::Numbering(matter) => {
                (matter.clone(), None)
            }
            PreviewOperation::PageMarks { document, page } => {
                let index = page
                    .parse::<usize>()
                    .map_err(|_| build_error("Invalid page number"))?;
                let doc = self.artifact_document(document)?;
                (text(&doc.body, "matter_id").to_owned(), Some((index, doc)))
            }
        };
        let inputs = Inputs::capture(&self.con, &self.root, self.key.as_ref(), &matter)
            .map_err(preview_disk_error)??;
        let snapshot = inputs
            .inputs()
            .digest(&self.con)
            .map_err(preview_disk_error)?;
        let (owner, docs, _) = inputs.inputs().generation_inputs();
        let (settings, marks) = if mark_page.is_some() {
            (
                Preview {
                    max_pages_per_volume: page(owner, "max_pages_per_volume")?,
                    keep_groups_intact: owner["keep_groups_intact"].as_bool().unwrap_or_default(),
                },
                bundle_inputs::parse_marks(body)?,
            )
        } else {
            (
                Preview {
                    max_pages_per_volume: page(body, "max_pages_per_volume")?,
                    keep_groups_intact: body["keep_groups_intact"]
                        .as_bool()
                        .ok_or_else(|| build_error("Choose whether groups stay intact"))?,
                },
                bundle_inputs::parse_marks(&body["output_marks"])?,
            )
        };
        let layout = match operation {
            PreviewOperation::FrontMatter(_) => OutputPreviewLayout::FrontMatter {
                volume: body["volume"].as_u64().unwrap_or(1),
                index_page: body["index_page"].as_u64().unwrap_or(1),
            },
            PreviewOperation::Numbering(_) => {
                OutputPreviewLayout::Numbering(self.numbering_source(&matter, &docs, body))
            }
            PreviewOperation::PageMarks { document, .. } => {
                let (index, doc) = mark_page.expect("mark preview selects a document");
                let source = if let Some(revision) = doc.body["revision"]
                    .as_u64()
                    .filter(|_| Some(index as u64) < doc.body["page_count"].as_u64())
                {
                    self.preview_source(&document, index, revision)
                } else {
                    Err(build_error("Invalid page number"))
                };
                OutputPreviewLayout::PageMarks(source)
            }
        };
        Ok(OutputPreviewWork {
            root: self.root.clone(),
            matter,
            inputs,
            snapshot,
            settings,
            marks,
            layout,
        })
    }

    pub fn complete_output_preview(&self, measured: MeasuredOutputPreview) -> Result<Reply, Reply> {
        let stale = || {
            refusal(
                409,
                "Bundle inputs changed during preview; run the preview again",
            )
        };
        if self.root != measured.root {
            return Err(stale());
        }
        let current =
            Inputs::metadata(&self.con, &measured.matter).map_err(preview_disk_error)??;
        if current.digest(&self.con).map_err(preview_disk_error)? != measured.snapshot
            || measured
                .proofs
                .iter()
                .any(|proof| proof.check_current().is_err())
        {
            return Err(stale());
        }
        if let Some(PreviewSourceProof {
            document,
            revision,
            proof,
        }) = measured.source
        {
            let current = self
                .pdf_source(
                    &document,
                    &Artifact::Working {
                        revision: Some(revision),
                    },
                )
                .map_err(|_| stale())?;
            if !proof.matches(&current) || proof.check_current().is_err() {
                return Err(stale());
            }
        }
        Ok(reply(200, measured.output))
    }

    pub(super) fn output_preview_request(
        &self,
        method: &str,
        route: &[&str],
        body: &Value,
    ) -> Option<Reply> {
        let target = OutputPreviewTarget::select(method, route)?;
        Some(
            self.prepare_output_preview(target, body)
                .and_then(OutputPreviewWork::compute)
                .and_then(|measured| self.complete_output_preview(measured))
                .unwrap_or_else(|error| error),
        )
    }
}
