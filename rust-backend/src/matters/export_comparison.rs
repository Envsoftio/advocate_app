//! Compare frozen export maps and source pages with the current working revision.
use super::{
    artifacts::{refusal, Artifact, PdfProof, PdfSource},
    bundle_inputs::{self, BundleInputWork, Inputs, Preview},
    document_verification::DocumentFileProof,
    *,
};
use pdf_oxide::PdfDocument;
use std::collections::{HashMap, HashSet};

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}
fn num(value: &Value, key: &str) -> usize {
    value[key].as_u64().unwrap_or_default() as usize
}
fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map(Vec::as_slice).unwrap_or_default()
}

fn source_end(row: &Value) -> usize {
    row["source_end"]
        .as_u64()
        .map(|page| page as usize)
        .unwrap_or_else(|| {
            num(row, "source_start")
                .saturating_add(num(row, "end_page").saturating_sub(num(row, "start_page")))
                .saturating_add(1)
        })
}

fn ranges(rows: &[&Value]) -> Vec<Value> {
    rows.iter()
        .map(
            |row| json!({"first":num(row,"source_start").saturating_add(1),"last":source_end(row)}),
        )
        .collect()
}

fn db(_: rusqlite::Error) -> Reply {
    refusal(500, "Unable to load saved comparison inputs")
}

struct MapWork {
    inputs: BundleInputWork,
    context: MapContext,
}

struct MapContext {
    snapshot: String,
    effective: String,
    settings: Preview,
    hashes: HashMap<String, String>,
    marks: Value,
    review: Value,
    bookmarks: Vec<Value>,
}

impl MapWork {
    fn capture(store: &Store, matter: &str, body: &Value) -> Result<Self, Reply> {
        let inputs =
            Inputs::capture(&store.con, &store.root, store.key.as_ref(), matter).map_err(db)??;
        let cap = body["max_pages_per_volume"]
            .as_u64()
            .filter(|cap| (2..=2000).contains(cap))
            .ok_or_else(|| refusal(422, "Invalid volume page cap"))?;
        let keep = body["keep_groups_intact"]
            .as_bool()
            .ok_or_else(|| refusal(422, "Choose whether groups stay intact"))?;
        let input = inputs.inputs();
        let (_, docs, _) = input.generation_inputs();
        let mut hashes = HashMap::new();
        for doc in docs {
            let id = text(doc, "id");
            let revision = doc["revision"]
                .as_u64()
                .ok_or_else(|| refusal(409, "Working revision is unavailable"))?;
            let digest: String = store
                .con
                .query_row(
                    "SELECT sha256 FROM document_revisions WHERE document_id=? AND revision=?",
                    params![id, revision],
                    |row| row.get(0),
                )
                .map_err(db)?;
            hashes.insert(id.to_owned(), digest);
        }
        let snapshot = input.digest(&store.con).map_err(db)?;
        let effective = input.effective_digest(&store.con).map_err(db)?;
        let marks = bundle_inputs::output_marks(&store.con, matter).map_err(db)?;
        let review = preparation::filing_review(&store.con, matter)
            .map_err(db)?
            .unwrap_or_default();
        let (bookmarks, _) = bundle_inputs::backup_destinations(&store.con, matter).map_err(db)?;
        Ok(Self {
            inputs,
            context: MapContext {
                snapshot,
                effective,
                hashes,
                marks,
                review,
                bookmarks,
                settings: Preview {
                    max_pages_per_volume: cap as usize,
                    keep_groups_intact: keep,
                },
            },
        })
    }

    fn measure(self, prior: &Value) -> Result<(Value, ComparisonProof), Reply> {
        let (input, files) = self.inputs.verify()?;
        let context = self.context;
        let map = input
            .preview_with_snapshot(context.settings, &context.snapshot)
            .map_err(db)?;
        if map.status != 200 {
            return Err(map);
        }
        let result = compare_maps(prior, &input, map.body, &context)?;
        Ok((
            result,
            ComparisonProof::Inputs {
                snapshot: context.snapshot,
                effective: context.effective,
                files,
            },
        ))
    }
}

fn compare_maps(
    prior: &Value,
    input: &Inputs,
    current: Value,
    context: &MapContext,
) -> Result<Value, Reply> {
    let MapContext {
        snapshot: full_digest,
        effective,
        settings: options,
        hashes,
        marks,
        review,
        bookmarks,
    } = context;
    let before = list(prior, "page_map_snapshot");
    let after = list(&current, "volumes")
        .iter()
        .flat_map(|volume| list(volume, "rows"))
        .collect::<Vec<_>>();
    const MAP_FIELDS: &[&str] = &[
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
    ];
    if before.iter().any(|row| {
        MAP_FIELDS[..9]
            .iter()
            .any(|field| row.get(*field).is_none())
    }) {
        return Ok(
            json!({"available":false,"reason":"The stored page-map snapshot is incomplete."}),
        );
    }
    let revisions = list(prior, "input_revisions");
    if revisions
        .iter()
        .any(|item| text(item, "document_id").is_empty() || item["revision"].as_u64().is_none())
    {
        return Ok(json!({"available":false,"reason":"Stored revision evidence is incomplete."}));
    }
    let baseline = revisions
        .iter()
        .map(|doc| (text(doc, "document_id"), doc))
        .collect::<HashMap<_, _>>();
    let key = |row: &Value| (text(row, "id").to_owned(), num(row, "source_start"));
    let old_rows = before
        .iter()
        .map(|row| (key(row), row))
        .collect::<HashMap<_, _>>();
    let new_rows = after
        .iter()
        .map(|row| (key(row), *row))
        .collect::<HashMap<_, _>>();
    if before
        .iter()
        .any(|row| !baseline.contains_key(text(row, "id")))
    {
        return Ok(
            json!({"available":false,"reason":"Stored document revision evidence is missing."}),
        );
    }
    let old_order = before
        .iter()
        .map(key)
        .filter(|key| new_rows.contains_key(key))
        .enumerate()
        .map(|(index, key)| (key, index))
        .collect::<HashMap<_, _>>();
    let new_order = after
        .iter()
        .map(|row| key(row))
        .filter(|key| old_rows.contains_key(key))
        .enumerate()
        .map(|(index, key)| (key, index))
        .collect::<HashMap<_, _>>();
    let working = input
        .all_documents()
        .iter()
        .map(|doc| (text(doc, "id"), doc))
        .collect::<HashMap<_, _>>();
    let mut changes = Vec::new();
    let mut reasons_by_document = HashMap::<String, Vec<&str>>::new();
    let mut seen = HashSet::new();
    for row in before.iter().chain(after.iter().copied()) {
        let row_key = key(row);
        if !seen.insert(row_key.clone()) {
            continue;
        }
        let id = row_key.0.as_str();
        let old = old_rows.get(&row_key).copied();
        let new = new_rows.get(&row_key).copied();
        let mut reasons = Vec::new();
        match (old, new) {
            (None, _) => reasons.push("added"),
            (_, None) => reasons.push("removed from current bundle"),
            (Some(old), Some(new)) => {
                let prior_revision = baseline[id];
                let current_doc = working[id];
                if prior_revision["revision"] != current_doc["revision"]
                    || prior_revision["working_sha256"]
                        .as_str()
                        .is_some_and(|digest| {
                            hashes.get(id).is_none_or(|working| digest != working)
                        })
                {
                    reasons.push("working PDF changed");
                }
                if old_order[&row_key] != new_order[&row_key] {
                    reasons.push("order changed");
                }
                for (field, reason) in [
                    ("title", "title changed"),
                    ("part", "Part changed"),
                    ("label", "label changed"),
                ] {
                    if old[field] != new[field] {
                        reasons.push(reason);
                    }
                }
                if MAP_FIELDS[4..9]
                    .iter()
                    .any(|field| old[*field] != new[*field])
                {
                    reasons.push("page range shifted");
                }
            }
        }
        if !reasons.is_empty() {
            let accumulated = reasons_by_document.entry(id.to_owned()).or_default();
            for reason in &reasons {
                if !accumulated.contains(reason) {
                    accumulated.push(reason);
                }
            }
            let title = new.or(old).map(|row| &row["title"]);
            let new = new.map(|row| {
                let mut snapshot = bundle_inputs::select(row, MAP_FIELDS);
                let volume = &list(&current, "volumes")[num(row, "volume") - 1];
                snapshot["printed_labels"] = json!(list(volume, "pages")
                    .iter()
                    .skip(num(row, "start_page") - 1)
                    .take(num(row, "end_page") - num(row, "start_page") + 1)
                    .map(|page| &page["printed_label"])
                    .collect::<Vec<_>>());
                snapshot
            });
            changes.push(json!({"document_id":id,"title":title,"reasons":reasons,
                "before":old,"after":new}));
        }
    }
    let mut documents = Vec::new();
    let mut seen = HashSet::new();
    for row in before.iter().chain(after.iter().copied()) {
        let id = text(row, "id");
        if !seen.insert(id) {
            continue;
        }
        let old = before
            .iter()
            .filter(|row| row["id"] == id)
            .collect::<Vec<_>>();
        let new = after
            .iter()
            .copied()
            .filter(|row| row["id"] == id)
            .collect::<Vec<_>>();
        let left = baseline.get(id).copied().unwrap_or(&Value::Null);
        let right = working
            .get(id)
            .copied()
            .filter(|_| !new.is_empty())
            .unwrap_or(&Value::Null);
        let reasons = reasons_by_document
            .get(id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let status = if old.is_empty() {
            "added"
        } else if new.is_empty() {
            "omitted"
        } else if reasons.is_empty() {
            "unchanged"
        } else {
            "changed"
        };
        let pages = |rows: &[&Value]| {
            rows.iter()
                .map(|row| num(row, "end_page").saturating_sub(num(row, "start_page")) + 1)
                .sum::<usize>()
        };
        documents.push(json!({"document_id":id,"title":new.first().or(old.first()).map(|row|&row["title"]),
            "status":status,"reasons":reasons,"before_revision":left["revision"],"after_revision":right["revision"],
            "before_sha256":left["working_sha256"],"after_sha256":hashes.get(id),
            "before_source_pages":pages(&old),"after_source_pages":pages(&new),"before_source_ranges":ranges(&old)}));
    }
    let mut settings = Vec::new();
    if prior["build_settings"]["max_pages_per_volume"] != options.max_pages_per_volume {
        settings.push("Volume page cap");
    }
    if prior["build_settings"]["keep_groups_intact"]
        .as_bool()
        .unwrap_or(false)
        != options.keep_groups_intact
    {
        settings.push("Keep groups together");
    }
    if &prior["output_marks"] != marks {
        settings.push("Output formatting and marks");
    }
    if &prior["filing_review"] != review {
        settings.push("User-authored review checklist");
    }
    let old = list(prior, "manual_bookmark_inputs");
    let mut changed = bookmarks
        .iter()
        .filter(|item| !old.iter().any(|entry| entry == *item))
        .map(|item| text(item, "title"))
        .collect::<Vec<_>>();
    changed.sort_unstable();
    changed.extend(
        old.iter()
            .filter(|item| !bookmarks.iter().any(|entry| entry["id"] == item["id"]))
            .map(|item| {
                item["title"]
                    .as_str()
                    .filter(|title| !title.is_empty())
                    .unwrap_or("Untitled bookmark")
            }),
    );
    let matches = if prior["effective_input_digest"]
        .as_str()
        .is_some_and(|digest| !digest.is_empty())
    {
        prior["effective_input_digest"] == effective.as_str()
    } else {
        prior["build_input_digest"] == full_digest.as_str()
    };
    Ok(
        json!({"available":true,"prior_build_id":prior["id"],"prior_created_at":prior["created_at"],"prior_pages":prior["pages"],
        "current_pages":current["combined_pages"],"documents":documents,"input_snapshot":full_digest,
        "changes":changes,"setting_changes":settings,"bookmark_changes":changed,
        "unresolved_bookmarks":list(&current,"manual_bookmarks").iter().filter(|entry|entry["resolved"]!=true).map(|entry|&entry["title"]).collect::<Vec<_>>(),
        "other_inputs_changed":!matches && changes.is_empty() && settings.is_empty() && changed.is_empty(),
        "current_matches_prior":matches && settings.is_empty()}),
    )
}

struct PageWork {
    reader: StoreReader,
    working: PdfSource,
    selection: PageSelection,
}

struct PageComparisonRequest {
    document: String,
    revision: u64,
    prior_page: usize,
    current_page: usize,
    ignore_numbering: bool,
}

impl PageComparisonRequest {
    fn parse(body: &Value) -> Result<Self, Reply> {
        use super::preparation::validation::issue;
        let invalid = |errors: Vec<Value>| reply(422, json!({"detail":errors}));
        let Some(fields) = body.as_object() else {
            let (kind, message) = if body.is_null() {
                ("missing", "Field required")
            } else {
                (
                    "model_attributes_type",
                    "Input should be a valid dictionary or object to extract fields from",
                )
            };
            return Err(invalid(vec![issue(
                kind,
                json!(["body"]),
                message,
                body.clone(),
            )]));
        };
        let mut errors = Vec::new();
        let ignore_numbering = match fields.get("ignore_generated_numbering") {
            Some(input) => match boolean(input) {
                Some(value) => value,
                None => {
                    let (kind, message) = if input.is_string() || input.is_i64() || input.is_u64() {
                        (
                            "bool_parsing",
                            "Input should be a valid boolean, unable to interpret input",
                        )
                    } else {
                        ("bool_type", "Input should be a valid boolean")
                    };
                    errors.push(issue(
                        kind,
                        json!(["body", "ignore_generated_numbering"]),
                        message,
                        input.clone(),
                    ));
                    false
                }
            },
            None => false,
        };
        let loc = json!(["body", "document_id"]);
        let document = match fields.get("document_id") {
            Some(Value::String(value)) if (1..=100).contains(&value.chars().count()) => {
                Some(value.clone())
            }
            Some(Value::String(value)) => {
                let mut error = if value.is_empty() {
                    issue(
                        "string_too_short",
                        loc,
                        "String should have at least 1 character",
                        json!(value),
                    )
                } else {
                    issue(
                        "string_too_long",
                        loc,
                        "String should have at most 100 characters",
                        json!(value),
                    )
                };
                error["ctx"] = if value.is_empty() {
                    json!({"min_length":1})
                } else {
                    json!({"max_length":100})
                };
                errors.push(error);
                None
            }
            Some(input) => {
                errors.push(issue(
                    "string_type",
                    loc,
                    "Input should be a valid string",
                    input.clone(),
                ));
                None
            }
            None => {
                errors.push(issue("missing", loc, "Field required", body.clone()));
                None
            }
        };
        let mut positive_integer = |key| {
            let loc = json!(["body", key]);
            let Some(input) = fields.get(key) else {
                errors.push(issue("missing", loc, "Field required", body.clone()));
                return None;
            };
            match input
                .as_u64()
                .map(Ok)
                .or_else(|| integer(input).map(u64::try_from))
            {
                Some(Ok(value)) if value > 0 => Some(value),
                Some(_) => {
                    let mut error = issue(
                        "greater_than_equal",
                        loc,
                        "Input should be greater than or equal to 1",
                        input.clone(),
                    );
                    error["ctx"] = json!({"ge":1});
                    errors.push(error);
                    None
                }
                None => {
                    let (kind, message) = integer_error(input);
                    errors.push(issue(kind, loc, message, input.clone()));
                    None
                }
            }
        };
        let revision = positive_integer("expected_revision");
        let prior_page = positive_integer("prior_source_page").map(|page| (page - 1) as usize);
        let current_page = positive_integer("current_source_page").map(|page| (page - 1) as usize);
        match (document, revision, prior_page, current_page) {
            (Some(document), Some(revision), Some(prior_page), Some(current_page))
                if errors.is_empty() =>
            {
                Ok(Self {
                    document,
                    revision,
                    prior_page,
                    current_page,
                    ignore_numbering,
                })
            }
            _ => Err(invalid(errors)),
        }
    }
}

struct PageSelection {
    document: String,
    revision: u64,
    page_count: Value,
    prior_page: usize,
    current_page: usize,
    physical: usize,
    prior_ranges: Vec<Value>,
    ignore_numbering: bool,
}

fn unavailable_pdf() -> Reply {
    refusal(
        409,
        "Saved PDF is unavailable or failed its integrity check",
    )
}

impl PageWork {
    fn capture(
        store: &Store,
        matter: &str,
        prior: Option<&Value>,
        request: PageComparisonRequest,
    ) -> Result<Self, Reply> {
        let PageComparisonRequest {
            document,
            revision,
            prior_page,
            current_page,
            ignore_numbering,
        } = request;
        let doc = store.artifact_document(&document)?;
        if doc.body["matter_id"] != matter || doc.body["revision"] != revision {
            return Err(refusal(
                409,
                "Working revision changed or belongs to another matter; reopen comparison",
            ));
        }
        let prior = prior.ok_or_else(|| refusal(409, "Saved build is unavailable"))?;
        let rows = list(prior, "page_map_snapshot")
            .iter()
            .filter(|row| row["id"] == document)
            .collect::<Vec<_>>();
        let row = rows
            .iter()
            .find(|row| num(row, "source_start") <= prior_page && prior_page < source_end(row))
            .ok_or_else(|| refusal(409, "This source page is not mapped in the selected build; choose a retained source page"))?;
        if current_page >= num(&doc.body, "page_count") {
            return Err(refusal(409, "Current source page is outside this revision"));
        }
        let physical = num(row, "combined_start_page")
            .checked_add(prior_page - num(row, "source_start"))
            .filter(|page| *page > 0)
            .ok_or_else(unavailable_pdf)?;
        let working = store.pdf_source(
            &document,
            &Artifact::Working {
                revision: Some(revision),
            },
        )?;
        Ok(Self {
            reader: StoreReader::capture(store).map_err(db)?,
            working,
            selection: PageSelection {
                document,
                revision,
                page_count: doc.body["page_count"].clone(),
                prior_page,
                current_page,
                physical,
                prior_ranges: ranges(&rows),
                ignore_numbering,
            },
        })
    }

    fn measure(self, prior: &Value) -> Result<(Value, ComparisonProof), Reply> {
        let con = self.reader.open().map_err(|_| unavailable_pdf())?;
        let saved = super::generated_exports::file(&con, prior, "combined.pdf")
            .map_err(|_| unavailable_pdf())?;
        drop(con);
        let (after, proof) = self.working.open_with_proof()?;
        let after = after.into_bytes().map_err(|_| unavailable_pdf())?;
        let report = compare_pages(prior, saved, after, &self.selection)?;
        Ok((
            report,
            ComparisonProof::Page {
                document: self.selection.document,
                revision: self.selection.revision,
                proof,
            },
        ))
    }
}

fn compare_pages(
    prior: &Value,
    saved: Vec<u8>,
    after: Vec<u8>,
    selection: &PageSelection,
) -> Result<Value, Reply> {
    let failure = unavailable_pdf;
    let PageSelection {
        document,
        revision,
        page_count,
        prior_page,
        current_page,
        physical,
        prior_ranges,
        ignore_numbering,
    } = selection;
    let before = PdfDocument::from_bytes(saved.clone()).map_err(|_| failure())?;
    let after = PdfDocument::from_bytes(after).map_err(|_| failure())?;
    let old_index = physical.checked_sub(1).ok_or_else(failure)?;
    let (raw, numbering_removed) = if *ignore_numbering {
        let contract = &prior["validation_contract"]["combined.pdf"];
        if prior["backend_origin"] == "python" {
            legacy_comparison_text(&before, old_index, contract).map_err(|_| failure())?
        } else if contract["printed_labels"][old_index]
            .as_str()
            .is_some_and(|label| !label.is_empty())
        {
            match super::page_tools::assembly::without_generated_page_number(saved, old_index) {
                Ok(pdf) => (
                    comparison_text(&pdf, 0)
                        .map_err(|_| failure())?
                        .trim_end_matches('\n')
                        .to_owned(),
                    true,
                ),
                Err(_) => {
                    legacy_comparison_text(&before, old_index, contract).map_err(|_| failure())?
                }
            }
        } else {
            (
                comparison_text(&before, old_index).map_err(|_| failure())?,
                false,
            )
        }
    } else {
        (
            comparison_text(&before, old_index).map_err(|_| failure())?,
            false,
        )
    };
    let edited = comparison_text(&after, *current_page).map_err(|_| failure())?;
    let (left, right) = (
        raw.chars().take(50_000).collect::<String>(),
        edited.chars().take(50_000).collect::<String>(),
    );
    let image = fingerprint(before, old_index).map_err(|_| failure())?;
    let next = fingerprint(after, *current_page).map_err(|_| failure())?;
    let (diff, truncated) = differences(&left, &right);
    Ok(
        json!({"build_id":prior["id"],"document_id":document,"revision":revision,
        "prior_physical_page":physical,"prior_source_page":prior_page+1,"current_source_page":current_page+1,
        "current_page_count":page_count,"prior_source_ranges":prior_ranges,
        "differences":diff,"truncated":truncated || raw.chars().count()>=50_000 || edited.chars().count()>=50_000,
        "numbering_removed":numbering_removed,"rendered_equal":image==next,"has_text":!left.trim().is_empty() || !right.trim().is_empty(),
        "note":if numbering_removed {
            "Recorded generated numbering was excluded from text differences only.  Pages are selected independently; text differences may include OCR or layout differences. This is not automatic insertion alignment or legal verification."
        } else {
            "Generated numbering and output marks are included.  Pages are selected independently; text differences may include OCR or layout differences. This is not automatic insertion alignment or legal verification."
        }}),
    )
}

enum ComparisonWork {
    Unavailable,
    Map(Box<MapWork>),
    Pages(Box<PageWork>),
}

enum ComparisonProof {
    SavedOnly,
    Inputs {
        snapshot: String,
        effective: String,
        files: Vec<DocumentFileProof>,
    },
    Page {
        document: String,
        revision: u64,
        proof: PdfProof,
    },
}

pub struct ExportComparisonWork {
    root: PathBuf,
    matter: String,
    build: String,
    prior: Value,
    work: ComparisonWork,
}

pub struct MeasuredExportComparison {
    root: PathBuf,
    matter: String,
    build: String,
    prior: Value,
    body: Value,
    proof: ComparisonProof,
}

impl ExportComparisonWork {
    pub fn measure(self) -> Result<MeasuredExportComparison, Reply> {
        let (body, proof) = match self.work {
            ComparisonWork::Unavailable => (
                json!({"available":false,
                    "reason":"This older build did not record enough page-map detail for a reliable comparison."}),
                ComparisonProof::SavedOnly,
            ),
            ComparisonWork::Map(work) => work.measure(&self.prior)?,
            ComparisonWork::Pages(work) => work.measure(&self.prior)?,
        };
        Ok(MeasuredExportComparison {
            root: self.root,
            matter: self.matter,
            build: self.build,
            prior: self.prior,
            body,
            proof,
        })
    }
}

impl Store {
    pub fn prepare_export_comparison(
        &self,
        matter: &str,
        build: &str,
        operation: &str,
        body: &Value,
    ) -> Result<ExportComparisonWork, Reply> {
        let page_request = (operation == "compare-pages")
            .then(|| PageComparisonRequest::parse(body))
            .transpose()?;
        if !self.active_matter(matter).map_err(db)? {
            return Err(refusal(404, "Matter not found"));
        }
        let prior = super::generated_exports::load(&self.con, matter, build).map_err(db)?;
        let work = if let Some(request) = page_request {
            ComparisonWork::Pages(Box::new(PageWork::capture(
                self,
                matter,
                prior.as_ref(),
                request,
            )?))
        } else if operation == "compare" {
            let manifest = prior
                .as_ref()
                .ok_or_else(|| refusal(404, "Build not found"))?;
            if !manifest["page_map_snapshot"].is_array() || !manifest["input_revisions"].is_array()
            {
                ComparisonWork::Unavailable
            } else {
                ComparisonWork::Map(Box::new(MapWork::capture(self, matter, body)?))
            }
        } else {
            return Err(refusal(422, "Choose a build comparison"));
        };
        Ok(ExportComparisonWork {
            root: self.root.clone(),
            matter: matter.into(),
            build: build.into(),
            prior: prior.ok_or_else(|| refusal(404, "Build not found"))?,
            work,
        })
    }

    pub fn complete_export_comparison(
        &self,
        measured: MeasuredExportComparison,
    ) -> Result<Reply, Reply> {
        let MeasuredExportComparison {
            root,
            matter,
            build,
            mut prior,
            body,
            proof,
        } = measured;
        let stale = || {
            refusal(
                409,
                "Saved inputs changed during comparison; reopen the comparison",
            )
        };
        if root != self.root {
            return Err(stale());
        }
        if !self.active_matter(&matter).map_err(db)? {
            return Err(refusal(404, "Matter not found"));
        }
        let mut current = super::generated_exports::load(&self.con, &matter, &build)
            .map_err(db)?
            .ok_or_else(|| refusal(404, "Build not found"))?;
        for manifest in [&mut prior, &mut current] {
            if let Some(fields) = manifest.as_object_mut() {
                fields.remove("archived_at");
            }
        }
        if current != prior {
            return Err(stale());
        }
        match proof {
            ComparisonProof::SavedOnly => {}
            ComparisonProof::Inputs {
                snapshot,
                effective,
                files,
            } => {
                let current = Inputs::metadata(&self.con, &matter).map_err(db)??;
                if current.digest(&self.con).map_err(db)? != snapshot
                    || current.effective_digest(&self.con).map_err(db)? != effective
                    || files.iter().any(|file| file.check_current().is_err())
                {
                    return Err(stale());
                }
            }
            ComparisonProof::Page {
                document,
                revision,
                proof,
            } => {
                let source = self.artifact_document(&document)?;
                if source.body["matter_id"] != matter
                    || source.body["revision"] != revision
                    || proof.check_current().is_err()
                {
                    return Err(stale());
                }
            }
        }
        Ok(reply(200, body))
    }
}

fn comparison_text(pdf: &PdfDocument, page: usize) -> pdf_oxide::Result<String> {
    let mut text = String::new();
    for line in pdf.extract_text_lines(page)? {
        text.push_str(&line.text);
        text.push('\n');
    }
    Ok(text)
}

fn legacy_comparison_text(
    pdf: &PdfDocument,
    page: usize,
    contract: &Value,
) -> Result<(String, bool), &'static str> {
    use super::{page_tools::assembly, text_edit};
    let label = contract["printed_labels"][page]
        .as_str()
        .unwrap_or_default()
        .trim();
    let region = contract["label_regions"][page]
        .as_array()
        .and_then(|bounds| {
            let [x0, y0, x1, y1] = bounds.as_slice() else {
                return None;
            };
            Some([x0.as_f64()?, y0.as_f64()?, x1.as_f64()?, y1.as_f64()?])
        });
    let error = |_| "Unable to read saved comparison text";
    let Some([x0, y0, x1, y1]) = region.filter(|_| !label.is_empty()) else {
        return comparison_text(pdf, page)
            .map(|text| (text, false))
            .map_err(error);
    };
    let geometry = assembly::geometry(pdf, page)?;
    let (width, height) = geometry.display_size();
    let region = [x0 / width, y0 / height, x1 / width, y1 / height];
    let mut lines = Vec::new();
    let mut removed = false;
    for line in pdf.extract_text_lines(page).map_err(error)? {
        let rect = line.bbox;
        let bbox = [rect.x, rect.y, rect.x + rect.width, rect.y + rect.height].map(f64::from);
        if line.text.trim() == label
            && text_edit::intersects(region, text_edit::displayed(geometry, bbox))
        {
            removed = true;
        } else {
            lines.push(line.text);
        }
    }
    Ok((lines.join("\n"), removed))
}

fn fingerprint(pdf: PdfDocument, page: usize) -> Result<(u32, u32, String), &'static str> {
    use pdf_oxide::rendering::{render_page, render_page_fit, RenderOptions};
    const MAX_EDGE: u32 = 1200;
    let frame = super::page_tools::assembly::DisplayFrame::new(&pdf, page)?;
    let visible = frame.visible.crop;
    let edge = frame.size.0.max(frame.size.1);
    let error = |_| "Unable to render comparison page";
    let mut options = RenderOptions::with_dpi(72).as_raw();
    options.max_output_pixels = 1_500_000;
    let raster = if edge <= f64::from(MAX_EDGE) && frame.unit == 1.0 {
        render_page(&pdf, page, &options).map_err(error)?
    } else {
        // Fit uses the MediaBox to calculate its fractional scale. Use the
        // visible box for that calculation on a detached comparison copy.
        let pdf = if visible == frame.geometry.media {
            pdf
        } else {
            let mut editor =
                pdf_oxide::editor::DocumentEditor::from_document(pdf).map_err(error)?;
            editor
                .set_page_media_box(page, visible.map(|coordinate| coordinate as f32))
                .map_err(error)?;
            PdfDocument::from_bytes(editor.save_to_bytes().map_err(error)?).map_err(error)?
        };
        let target = edge.ceil().clamp(1.0, f64::from(MAX_EDGE)) as u32;
        render_page_fit(&pdf, page, target, target, &options).map_err(error)?
    };
    Ok((raster.width, raster.height, sha256_hex(&raster.data)))
}

fn split_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut characters = text.char_indices().peekable();
    while let Some((index, ch)) = characters.next() {
        if matches!(
            ch,
            '\n' | '\r' | '\x0b' | '\x0c' | '\x1c'..='\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}'
        ) {
            lines.push(&text[start..index]);
            start = index + ch.len_utf8();
            if ch == '\r' && characters.next_if(|&(_, next)| next == '\n').is_some() {
                start += 1;
            }
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

// Use the reference's longest contiguous matching blocks and autojunk threshold.
fn differences(left: &str, right: &str) -> (Vec<Value>, bool) {
    let a = split_lines(left);
    let b = split_lines(right);
    let mut locations = HashMap::<&str, Vec<usize>>::new();
    for (j, line) in b.iter().enumerate() {
        locations.entry(line).or_default().push(j);
    }
    if b.len() >= 200 {
        locations.retain(|_, indexes| indexes.len() <= b.len() / 100 + 1);
    }
    let mut queue = vec![(0, a.len(), 0, b.len())];
    let mut blocks = Vec::new();
    while let Some((a0, a1, b0, b1)) = queue.pop() {
        let (mut ai, mut bi, mut length) = (a0, b0, 0);
        let mut previous = HashMap::<usize, usize>::new();
        let mut current = HashMap::<usize, usize>::new();
        for (i, line) in a.iter().enumerate().take(a1).skip(a0) {
            if let Some(indexes) = locations.get(line) {
                for &j in indexes
                    .iter()
                    .skip(indexes.partition_point(|j| *j < b0))
                    .take_while(|j| **j < b1)
                {
                    let size = 1 + previous.get(&j.wrapping_sub(1)).copied().unwrap_or(0);
                    current.insert(j, size);
                    if size > length {
                        (ai, bi, length) = (i + 1 - size, j + 1 - size, size);
                    }
                }
            }
            std::mem::swap(&mut previous, &mut current);
            current.clear();
        }
        while ai > a0 && bi > b0 && a[ai - 1] == b[bi - 1] {
            ai -= 1;
            bi -= 1;
            length += 1;
        }
        while ai + length < a1 && bi + length < b1 && a[ai + length] == b[bi + length] {
            length += 1;
        }
        if length > 0 {
            blocks.push((ai, bi, length));
            if a0 < ai && b0 < bi {
                queue.push((a0, ai, b0, bi));
            }
            if ai + length < a1 && bi + length < b1 {
                queue.push((ai + length, a1, bi + length, b1));
            }
        }
    }
    blocks.sort_unstable();
    blocks.push((a.len(), b.len(), 0));
    let (mut i, mut j) = (0, 0);
    let mut result = Vec::new();
    for (ai, bi, length) in blocks {
        if i < ai || j < bi {
            if result.len() == 100 {
                return (result, true);
            }
            let kind = if i == ai {
                "insert"
            } else if j == bi {
                "delete"
            } else {
                "replace"
            };
            let old = a[i..ai].join("\n");
            let new = b[j..bi].join("\n");
            result.push(json!({"kind":kind,"before":old.chars().take(4000).collect::<String>(),"after":new.chars().take(4000).collect::<String>()}));
        }
        (i, j) = (ai + length, bi + length);
    }
    (result, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matters::page_tools::assembly;
    use pdf_oxide::editor::DocumentEditor;

    #[test]
    fn comparison_fingerprint_uses_physical_page_size() {
        use std::fmt::Write;

        for (unit, rotation, expected) in [
            (0.5, 0, (250, 200)),
            (2.0, 90, (800, 1000)),
            (3.0, 0, (1200, 960)),
        ] {
            let mut bytes = String::from("%PDF-1.6\n");
            let objects = [
                "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
                "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
                format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 500 400] /Resources <<>> /UserUnit {unit} /Rotate {rotation} >>"),
            ];
            let mut offsets = Vec::new();
            for (i, object) in objects.iter().enumerate() {
                offsets.push(bytes.len());
                writeln!(bytes, "{} 0 obj\n{object}\nendobj", i + 1).unwrap();
            }
            let xref = bytes.len();
            bytes.push_str("xref\n0 4\n0000000000 65535 f \n");
            for offset in offsets {
                writeln!(bytes, "{offset:010} 00000 n ").unwrap();
            }
            writeln!(
                bytes,
                "trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF"
            )
            .unwrap();
            let pdf = PdfDocument::from_bytes(bytes.into_bytes()).unwrap();
            let (width, height, _) = fingerprint(pdf, 0).unwrap();
            assert_eq!((width, height), expected);
        }
    }

    #[test]
    fn comparison_fingerprint_fits_the_rotated_crop_at_fractional_scale() {
        let full = assembly::blank((1700.0, 850.0)).unwrap();
        let full = PdfDocument::from_bytes(full).unwrap();
        let expected = fingerprint(full, 0).unwrap();
        assert_eq!((expected.0, expected.1), (1200, 600));

        let larger = assembly::blank((3100.0, 2000.0)).unwrap();
        let mut editor = DocumentEditor::from_bytes(larger).unwrap();
        editor
            .set_page_crop_box(0, [113.0, 171.0, 1813.0, 1021.0])
            .unwrap();
        let cropped = PdfDocument::from_bytes(editor.save_to_bytes().unwrap()).unwrap();
        assert_eq!(fingerprint(cropped, 0).unwrap(), expected);

        editor.rotate_page_by(0, 90).unwrap();
        let rotated = PdfDocument::from_bytes(editor.save_to_bytes().unwrap()).unwrap();
        let measured = fingerprint(rotated, 0).unwrap();
        assert_eq!((measured.0, measured.1), (600, 1200));
    }
}
