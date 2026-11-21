//! Durable work queue. Native hosts compute on a bounded worker outside the store lane.
use super::{
    artifacts::{refusal, PdfProof},
    bundle_inputs::{BundleInputWork, Inputs, Preview},
    bundle_writer::{
        check_primary_signatures, retained_source_rows, Build, BuildFreeze, BuildInputs,
        CompositionBuildPlan, FrozenBuild, RetainedSource, BUILD_INPUT_CHANGED,
    },
    document_verification::DocumentFileProof,
    ocr, *,
};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

pub(super) fn migrate(con: &Connection) -> rusqlite::Result<()> {
    con.execute_batch(
        "CREATE TABLE rust_processing_jobs(
            id TEXT PRIMARY KEY, matter_id TEXT NOT NULL, body TEXT NOT NULL);
         CREATE INDEX rust_processing_jobs_matter ON rust_processing_jobs(matter_id);
         PRAGMA user_version=9;",
    )
}

pub(super) fn migrate_frozen_builds(con: &Connection) -> rusqlite::Result<()> {
    con.execute_batch("CREATE TABLE rust_frozen_builds(id TEXT PRIMARY KEY,matter_id TEXT NOT NULL,body TEXT NOT NULL);
        CREATE INDEX rust_frozen_builds_matter ON rust_frozen_builds(matter_id);
        CREATE TABLE rust_frozen_sources(snapshot_id TEXT NOT NULL,position INTEGER NOT NULL,bytes BLOB NOT NULL,
            PRIMARY KEY(snapshot_id,position));
        PRAGMA user_version=14;")
}

pub(super) fn recover(con: &Connection) -> rusqlite::Result<()> {
    con.execute(
        "UPDATE rust_processing_jobs SET body=json_set(body,'$.status','interrupted',
            '$.message','Interrupted by engine restart; retry starts a new job',
            '$.error','The prior operation did not complete','$.updated_at',?1)
         WHERE json_extract(body,'$.status') IN ('queued','running')",
        [now()],
    )?;
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(90)).to_rfc3339();
    con.execute(
        "DELETE FROM rust_processing_jobs
         WHERE json_extract(body,'$.status') IN ('completed','failed','cancelled')
           AND julianday(coalesce(json_extract(body,'$.updated_at'),json_extract(body,'$.created_at'))) < julianday(?1)",
        [cutoff],
    )?;
    con.execute_batch(
        "DELETE FROM rust_frozen_builds WHERE NOT EXISTS (
            SELECT 1 FROM rust_processing_jobs
            WHERE rust_processing_jobs.matter_id=rust_frozen_builds.matter_id
              AND json_extract(rust_processing_jobs.body,'$.input_context.snapshot_id')=rust_frozen_builds.id
         );
         DELETE FROM rust_frozen_sources WHERE NOT EXISTS (
            SELECT 1 FROM rust_frozen_builds WHERE id=rust_frozen_sources.snapshot_id
         );"
    )?;
    Ok(())
}

pub(super) struct Live {
    cancelled: AtomicBool,
    progress: Mutex<LiveProgress>,
}

#[derive(Default)]
struct LiveProgress {
    percent: u8,
    message: String,
    updated_at: String,
    events: Vec<Value>,
}

pub(super) type ActiveJobs = HashMap<String, Arc<Live>>;

pub struct PendingWork {
    id: String,
    matter: String,
    digest: String,
    job: Work,
    live: Arc<Live>,
}

pub struct FinishedWork {
    id: String,
    matter: String,
    digest: String,
    product: Result<Product, Reply>,
    live: Arc<Live>,
}

enum Work {
    Build(StoreReader, Value),
    Ocr(ocr::Work),
    Screen(ocr::Screening),
    Compress(super::compression::Work),
}

enum Product {
    Build(Build),
    Revision(ocr::Product),
    Screen(ocr::ScreeningProduct),
}

pub struct ScreenSubmission {
    root: PathBuf,
    matter: String,
    digest: String,
    sources: Vec<ocr::ScreeningDocument>,
}

pub struct VerifiedScreenSubmission {
    root: PathBuf,
    matter: String,
    digest: String,
    files: Vec<super::artifacts::PdfProof>,
}

pub struct ProcessingSubmission {
    root: PathBuf,
    matter: String,
    kind: String,
    target: Option<String>,
    acknowledged: bool,
    previous: Option<Value>,
    inputs: SubmissionInputs,
}

enum SubmissionInputs {
    Build(Box<BuildWork>),
    SavedBuild(StoreReader),
    Ocr { body: Value, context: Value },
}

struct BuildWork {
    freeze: Result<BuildFreeze, Reply>,
    preflight: BuildInspection,
}

enum BuildInspection {
    Primary(Box<PrimaryBuildPlan>),
    Composition(Box<CompositionBuildPlan>),
}

struct PrimaryBuildPlan {
    inputs: BundleInputWork,
    settings: Preview,
    snapshot: String,
    preflight: Option<PreflightWork>,
}

impl PrimaryBuildPlan {
    fn measure(self, acknowledged: bool) -> Result<(Value, Option<MeasuredPreflight>), Reply> {
        let (inputs, proofs) = self.inputs.verify()?;
        check_primary_signatures(&inputs, acknowledged)?;
        let plan = inputs
            .preview_with_snapshot(self.settings, &self.snapshot)
            .map_err(|_| refusal(500, "Unable to load saved bundle inputs"))?;
        let preflight = self
            .preflight
            .map(|mut work| {
                work.set_build_plan(&plan, proofs);
                work.measure()
            })
            .transpose()?;
        Ok((plan.body, preflight))
    }
}

pub struct PreparedProcessingSubmission {
    root: PathBuf,
    matter: String,
    previous: Option<Value>,
    preflight: Option<MeasuredPreflight>,
    queue: Result<(Value, PreparedInputs), Reply>,
}

enum PreparedInputs {
    Build(Box<PreparedBuildInputs>),
    SavedBuild {
        raw: String,
        rows: Vec<RetainedSource>,
    },
    Ocr(Value),
}

struct PreparedBuildInputs {
    snapshot: String,
    raw: String,
    frozen: FrozenBuild,
    sources: Vec<Vec<u8>>,
    proofs: Vec<PdfProof>,
    input_proofs: Vec<DocumentFileProof>,
}

fn transaction_failed() -> Reply {
    refusal(500, "Rust matter transaction failed; no changes committed")
}

fn retry_already_claimed() -> Reply {
    refusal(
        409,
        "This stopped attempt has already been retried or is not retryable",
    )
}

impl ProcessingSubmission {
    pub fn compute(self) -> Result<PreparedProcessingSubmission, Reply> {
        let target = self.target.as_deref();
        let (preflight, preparation) = match self.inputs {
            SubmissionInputs::Build(work) => {
                let BuildWork { freeze, preflight } = *work;
                let (plan, preflight, composition) = match preflight {
                    BuildInspection::Primary(work) => {
                        let (plan, preflight) = work.measure(self.acknowledged)?;
                        (Some(plan), preflight, None)
                    }
                    BuildInspection::Composition(work) => (None, None, Some(work.measure()?)),
                };
                let prepared = freeze.and_then(|mut freeze| {
                    if let Some(plan) = plan {
                        freeze.set_plan(plan);
                    }
                    if let Some(run) = &preflight {
                        freeze.set_checks(run.findings());
                    }
                    let (mut inspected, input_proofs) = composition
                        .map(|measured| freeze.set_composition(measured))
                        .unwrap_or_default();
                    let (frozen, sources, mut proofs) = freeze.compute()?;
                    proofs.append(&mut inspected);
                    let snapshot = uuid::Uuid::new_v4().simple().to_string();
                    let raw = serde_json::to_string(&frozen).map_err(|_| transaction_failed())?;
                    let context =
                        frozen.summary(&self.kind, &snapshot, &sha256_hex(raw.as_bytes()), target);
                    Ok((
                        PreparedInputs::Build(Box::new(PreparedBuildInputs {
                            snapshot,
                            raw,
                            frozen,
                            sources,
                            proofs,
                            input_proofs,
                        })),
                        context,
                    ))
                });
                (preflight, prepared)
            }
            SubmissionInputs::SavedBuild(database) => {
                let previous = self.previous.as_ref().ok_or_else(transaction_failed)?;
                let con = database
                    .open()
                    .map_err(|_| saved_build_unavailable(previous))?;
                let (raw, mut saved) = saved_build(&con, previous)?;
                saved.require_signature_review(self.acknowledged)?;
                let rows = saved.verify_retained_sources(&con, snapshot_id(previous))?;
                (
                    None,
                    Ok((
                        PreparedInputs::SavedBuild { raw, rows },
                        previous["input_context"].clone(),
                    )),
                )
            }
            SubmissionInputs::Ocr { body, context } => {
                (None, Ok((PreparedInputs::Ocr(body), context)))
            }
        };
        let queue = preparation.map(|(inputs, context)| {
            let mut job = queued(&self.matter, &self.kind, target, context);
            job["acknowledge_signature_risk"] = json!(self.acknowledged);
            (job, inputs)
        });
        Ok(PreparedProcessingSubmission {
            root: self.root,
            matter: self.matter,
            previous: self.previous,
            preflight,
            queue,
        })
    }
}

impl ScreenSubmission {
    pub fn verify(self) -> Result<VerifiedScreenSubmission, Reply> {
        let mut files = Vec::new();
        for source in self.sources {
            files.push(source.original.authenticate(|| false)?);
            files.push(source.working.authenticate(|| false)?);
        }
        Ok(VerifiedScreenSubmission {
            root: self.root,
            matter: self.matter,
            digest: self.digest,
            files,
        })
    }
}

impl PendingWork {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn compute(self) -> FinishedWork {
        self.compute_with_checkpoint(|_| Ok(()))
    }

    pub fn compute_with_checkpoint(
        self,
        checkpoint: impl Fn(&str) -> Result<(), Reply>,
    ) -> FinishedWork {
        let progress = |progress: u8, message: &str| {
            if self.live.cancelled.load(Ordering::Acquire) {
                return Err(refusal(409, "Operation cancelled"));
            }
            let mut state = self
                .live
                .progress
                .lock()
                .map_err(|_| refusal(500, "Job progress is unavailable"))?;
            let stamp = now();
            let progress = progress.min(100);
            state.percent = progress;
            state.message = message.into();
            if state
                .events
                .last()
                .is_none_or(|event| event["message"] != message)
            {
                state
                    .events
                    .push(json!({"progress":progress,"message":message,"at":stamp}));
            }
            state.updated_at = stamp;
            drop(state);
            checkpoint(&self.id)
        };
        let product = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match self.job {
            Work::Build(reader, job) => load_build_work(&reader, &job)
                .and_then(|build| build.run(&self.id, progress))
                .map(Product::Build),
            Work::Ocr(job) => job
                .run(&self.live.cancelled, progress)
                .map(Product::Revision),
            Work::Screen(job) => job.run(&self.live.cancelled, progress).map(Product::Screen),
            Work::Compress(job) => job
                .run(&self.live.cancelled, progress)
                .map(Product::Revision),
        }))
        .unwrap_or_else(|_| Err(refusal(500, "Rust job worker stopped before publication")));
        FinishedWork {
            id: self.id,
            matter: self.matter,
            digest: self.digest,
            product,
            live: self.live,
        }
    }
}

fn load(con: &Connection, id: &str) -> rusqlite::Result<Option<Value>> {
    con.query_row(
        "SELECT body FROM rust_processing_jobs WHERE id=?",
        [id],
        |row| decode(&row.get::<_, String>(0)?),
    )
    .optional()
}

fn write(con: &Connection, job: &Value) -> rusqlite::Result<()> {
    con.execute("INSERT INTO rust_processing_jobs VALUES (?,?,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body",
        params![job["id"].as_str(),job["matter_id"].as_str(),job.to_string()])?;
    Ok(())
}

pub(in crate::matters) fn measured_exception(
    con: &Connection,
    matter: &str,
    snapshot: &str,
) -> rusqlite::Result<Option<Value>> {
    let raw:Option<String>=con.query_row("SELECT body FROM rust_processing_jobs WHERE matter_id=? AND
        json_extract(body,'$.result.input_snapshot')=? AND json_array_length(body,'$.result.size_exceptions')>0
        ORDER BY json_extract(body,'$.created_at') DESC,rowid DESC LIMIT 1",
        params![matter,snapshot],|row|row.get(0)).optional()?;
    Ok(raw.map(|raw| decode(&raw)).transpose()?.map(|job| {
        let mut result = job["result"].clone();
        result["job_id"] = job["id"].clone();
        result
    }))
}

fn queued(matter: &str, kind: &str, target: Option<&str>, context: Value) -> Value {
    let stamp = now();
    json!({
        "id":uuid::Uuid::new_v4().simple().to_string(),"matter_id":matter,"kind":kind,
        "status":"queued","target_id":target,"progress":0,"message":"Queued","input_context":context,
        "events":[],"error":null,"result":null,"retry_job_id":null,
        "created_at":stamp,"updated_at":stamp,"started_at":null,
    })
}

fn queue_space(con: &Connection) -> rusqlite::Result<bool> {
    con.query_row(
        "SELECT count(*) < 16 FROM rust_processing_jobs WHERE json_extract(body,'$.status')='queued'",
        [], |row| row.get(0),
    )
}

fn queued_reply(mut job: Value) -> Reply {
    job.as_object_mut()
        .expect("job object")
        .remove("session_owner");
    reply(202, job)
}

fn enqueue(con: &Connection, mut job: Value, owner: &str) -> rusqlite::Result<Reply> {
    if !queue_space(con)? {
        return Ok(refusal(503, "Rust job queue is full"));
    }
    job["session_owner"] = json!(owner);
    write(con, &job)?;
    Ok(queued_reply(job))
}

fn snapshot_id(job: &Value) -> &str {
    job["input_context"]["snapshot_id"]
        .as_str()
        .unwrap_or_default()
}

fn saved_build_unavailable(job: &Value) -> Reply {
    refusal(
        409,
        job["legacy_retry_blocker"]
            .as_str()
            .unwrap_or("Saved build inputs are unavailable. Build current inputs instead."),
    )
}

fn saved_build(con: &Connection, job: &Value) -> Result<(String, FrozenBuild), Reply> {
    let missing = || saved_build_unavailable(job);
    let raw: Option<String> = con
        .query_row(
            "SELECT body FROM rust_frozen_builds WHERE id=? AND matter_id=?",
            params![snapshot_id(job), job["matter_id"].as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| missing())?;
    let raw = raw.ok_or_else(missing)?;
    let expected = sha256_hex(raw.as_bytes());
    if job["input_context"]["snapshot_digest"] != expected {
        return Err(refusal(
            409,
            "Saved build input contract failed its integrity check",
        ));
    }
    let frozen = serde_json::from_str(&raw).map_err(|_| missing())?;
    Ok((raw, frozen))
}

fn load_build_work(
    reader: &StoreReader,
    job: &Value,
) -> Result<super::bundle_writer::PreparedBuild, Reply> {
    let unavailable = |_| refusal(409, "Saved build sources are unavailable");
    let con = reader.open().map_err(|_| saved_build_unavailable(job))?;
    let tx = con.unchecked_transaction().map_err(unavailable)?;
    let (_, mut saved) = saved_build(&tx, job)?;
    saved.require_signature_review(job["acknowledge_signature_risk"] == true)?;
    let snapshot = snapshot_id(job);
    let size: u64 = tx
        .query_row(
            "SELECT coalesce(sum(length(bytes)),0) FROM rust_frozen_sources WHERE snapshot_id=?",
            [snapshot],
            |row| row.get(0),
        )
        .map_err(unavailable)?;
    if size > super::bundle_writer::BUILD_LIMIT as u64 {
        return Err(refusal(409, "Saved bundle sources exceed the build limit"));
    }
    let sources = {
        let mut query = tx
            .prepare("SELECT bytes FROM rust_frozen_sources WHERE snapshot_id=? ORDER BY position")
            .map_err(unavailable)?;
        let rows = query
            .query_map([snapshot], |row| row.get(0))
            .map_err(unavailable)?
            .collect::<rusqlite::Result<Vec<Vec<u8>>>>()
            .map_err(unavailable)?;
        rows
    };
    tx.commit().map_err(unavailable)?;
    drop(con);
    saved.prepare(sources)
}

fn with_status(mut job: Value, live: Option<&Arc<Live>>) -> Value {
    if job.get("started_at").is_none() {
        job["started_at"] = Value::Null;
    }
    if job.get("updated_at").is_none() {
        job["updated_at"] = job["created_at"].clone();
    }
    if let Some(live) = live {
        if let Ok(progress) = live.progress.lock() {
            if !progress.events.is_empty() {
                job["progress"] = json!(progress.percent);
                job["message"] = json!(progress.message);
                job["updated_at"] = json!(progress.updated_at);
                job["events"] = json!(progress.events);
            }
        }
    }
    job
}

fn public_status(
    con: &Connection,
    job: Value,
    live: Option<&Arc<Live>>,
) -> rusqlite::Result<Value> {
    let mut job = with_status(job, live);
    job.as_object_mut()
        .expect("job object")
        .remove("session_owner");
    // Local builds retain a small publication receipt in the job row. The
    // manifest is committed with that receipt and remains readable after
    // restart. Imported Python jobs can already contain their full result.
    if job["status"] != "completed"
        || !matches!(
            job["kind"].as_str(),
            Some("build" | "variant-build" | "supplement-build")
        )
        || job["result"].get("id").is_some()
    {
        return Ok(job);
    }
    let (Some(matter), Some(id)) = (job["matter_id"].as_str(), job["id"].as_str()) else {
        return Ok(job);
    };
    if job["result"]["export_id"] != id {
        return Ok(job);
    }
    let Some(Value::Object(mut manifest)) = super::generated_exports::load(con, matter, id)? else {
        return Ok(job);
    };
    if manifest.get("id").and_then(Value::as_str) != Some(id)
        || manifest.get("matter_id").and_then(Value::as_str) != Some(matter)
    {
        return Ok(job);
    }
    // Archive status and live freshness belong to the export view.
    manifest.remove("archived_at");
    manifest.remove("stale");
    manifest.insert("export_id".into(), json!(id));
    job["result"] = Value::Object(manifest);
    Ok(job)
}

fn finish_job(job: &mut Value, result: Result<Value, Reply>, cancelled: bool) {
    job["updated_at"] = json!(now());
    match result {
        Ok(result) => {
            job["status"] = json!("completed");
            job["progress"] = json!(100);
            job["message"] = json!("Completed");
            job["result"] = result;
        }
        Err(error) => {
            let detail = error.body["detail"]
                .as_str()
                .unwrap_or("Unable to complete bundle job");
            job["status"] = json!(if cancelled { "cancelled" } else { "failed" });
            job["message"] = json!(if cancelled { "Cancelled" } else { "Failed" });
            job["error"] = json!(detail);
            if matches!(
                job["kind"].as_str(),
                Some("build" | "variant-build" | "supplement-build")
            ) {
                job["result"] = error.body.get("recovery").cloned()
                    .or_else(|| error.body.get("failures").map(|failures| json!({"failures":failures})))
                    .unwrap_or_else(|| json!({"failures":[{"code":"BUILD_FAILED","severity":"error","message":detail}]}));
            }
        }
    }
}

const SCREEN_STALE: &str = "Saved inputs changed; run checks again before screening";

fn screen_submission_inputs(
    con: &Connection,
    matter: &str,
    expected: Option<&str>,
) -> Result<(super::bundle_inputs::Inputs, String), Reply> {
    let database = |_| refusal(500, "Unable to load saved screening inputs");
    let input = super::bundle_inputs::Inputs::metadata(con, matter).map_err(database)??;
    let digest = input.effective_digest(con).map_err(database)?;
    if Some(digest.as_str()) != expected {
        return Err(refusal(409, SCREEN_STALE));
    }
    if input.generation_inputs().1.is_empty() {
        return Err(refusal(422, "Include documents before screening pages"));
    }
    let active: bool = con.query_row(
        "SELECT EXISTS(SELECT 1 FROM rust_processing_jobs WHERE matter_id=? AND json_extract(body,'$.status') IN ('queued','running'))",
        [matter], |row| row.get(0),
    ).map_err(database)?;
    if active {
        return Err(refusal(
            409,
            "Wait for the active operation before screening pages",
        ));
    }
    Ok((input, digest))
}

impl Store {
    fn require_open_processing_queue(&self) -> Result<(), Reply> {
        if self.processing_stopped {
            return Err(refusal(503, "Rust processing queue is shutting down"));
        }
        Ok(())
    }

    pub fn checkpoint_processing_job(&self, id: &str) -> Result<(), Reply> {
        let unavailable = || refusal(500, "Unable to save job progress");
        let live = self.active_jobs.get(id).ok_or_else(unavailable)?;
        let job = load(&self.con, id)
            .map_err(|_| unavailable())?
            .filter(|job| job["status"] == "running")
            .ok_or_else(unavailable)?;
        write(&self.con, &with_status(job, Some(live))).map_err(|_| unavailable())
    }

    pub fn stop_processing(&mut self) -> Result<(), String> {
        self.processing_stopped = true;
        for job in self.active_jobs.values() {
            job.cancelled.store(true, Ordering::Release);
        }
        self.con
            .execute(
                "UPDATE rust_processing_jobs SET body=json_set(body,'$.status','cancelled',
                '$.message','Cancelled','$.updated_at',?1)
             WHERE json_extract(body,'$.status')='queued'",
                [now()],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn prepare_screen_submission(
        &self,
        matter: &str,
        body: &Value,
    ) -> Result<ScreenSubmission, Reply> {
        let (input, digest) =
            screen_submission_inputs(&self.con, matter, body["expected_snapshot"].as_str())?;
        let sources = ocr::screening_documents(self, &input)?;
        Ok(ScreenSubmission {
            root: self.root.clone(),
            matter: matter.into(),
            digest,
            sources,
        })
    }

    pub fn submit_screening(&mut self, verified: VerifiedScreenSubmission) -> Result<Reply, Reply> {
        self.require_open_processing_queue()?;
        let VerifiedScreenSubmission {
            root,
            matter,
            digest,
            files,
        } = verified;
        if self.root != root {
            return Err(refusal(409, SCREEN_STALE));
        }
        let database = |_| refusal(500, "Unable to queue page screening");
        let tx = self
            .con
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database)?;
        screen_submission_inputs(&tx, &matter, Some(&digest))?;
        if files.iter().any(|file| file.check_current().is_err()) {
            return Err(refusal(409, SCREEN_STALE));
        }
        let response = enqueue(
            &tx,
            queued(
                &matter,
                "preflight-screen",
                None,
                json!({"input_digest":digest}),
            ),
            &self.processing_session,
        )
        .map_err(database)?;
        tx.commit().map_err(database)?;
        Ok(response)
    }
    pub fn prepare_build_submission(
        &self,
        matter: &str,
        kind: &str,
        target: Option<&str>,
        body: &Value,
    ) -> Result<ProcessingSubmission, Reply> {
        self.require_open_processing_queue()?;
        let acknowledged = body["acknowledge_signature_risk"] == true;
        let queue_has_space = queue_space(&self.con).map_err(|_| transaction_failed())?;
        let queue_full = || refusal(503, "Rust job queue is full");
        let (inputs, preflight) = match (kind, target) {
            ("build", None) => {
                let read = Inputs::capture(&self.con, &self.root, self.key.as_ref(), matter)
                    .map_err(|_| refusal(500, "Unable to load saved bundle inputs"))??;
                let (inputs, settings) = BuildInputs::capture_primary_metadata(
                    self,
                    matter,
                    acknowledged,
                    read.inputs(),
                )?;
                let preflight = queue_has_space
                    .then(|| inputs.prepare_preflight(self, matter))
                    .transpose()?;
                let snapshot = inputs.snapshot().to_owned();
                (
                    inputs,
                    BuildInspection::Primary(Box::new(PrimaryBuildPlan {
                        inputs: read,
                        settings,
                        snapshot,
                        preflight,
                    })),
                )
            }
            ("variant-build" | "supplement-build", Some(target)) => {
                let (inputs, composition) = BuildInputs::capture_composition(
                    self,
                    matter,
                    kind.trim_end_matches("-build"),
                    target,
                    acknowledged,
                )?;
                (inputs, BuildInspection::Composition(Box::new(composition)))
            }
            _ => return Err(refusal(422, "Invalid build target")),
        };
        let freeze = if queue_has_space {
            let selection = match &preflight {
                BuildInspection::Primary(_) => None,
                BuildInspection::Composition(work) => Some(&work.selection),
            };
            inputs.prepare_freeze(self, matter, selection)
        } else {
            Err(queue_full())
        };
        Ok(ProcessingSubmission {
            root: self.root.clone(),
            matter: matter.into(),
            kind: kind.into(),
            target: target.map(str::to_owned),
            acknowledged,
            previous: None,
            inputs: SubmissionInputs::Build(Box::new(BuildWork { freeze, preflight })),
        })
    }

    pub fn prepare_retry(&self, id: &str, body: &Value) -> Result<ProcessingSubmission, Reply> {
        self.require_open_processing_queue()?;
        let failed = |_| transaction_failed();
        let previous = load(&self.con, id)
            .map_err(failed)?
            .ok_or_else(|| refusal(404, "Job not found"))?;
        let matter = previous["matter_id"]
            .as_str()
            .ok_or_else(transaction_failed)?;
        if !self.active_matter(matter).map_err(failed)? {
            return Err(refusal(404, "Matter not found"));
        }
        let kind = previous["kind"].as_str().unwrap_or_default();
        let is_build = matches!(kind, "build" | "variant-build" | "supplement-build");
        if kind != "ocr" && !is_build {
            return Err(refusal(409, "This job type cannot be retried"));
        }
        let has_saved_build = is_build
            && (!previous["input_context"].is_null()
                || !previous["legacy_retry_blocker"].is_null());
        let retryable = previous["status"] == "interrupted"
            || has_saved_build
                && matches!(previous["status"].as_str(), Some("failed" | "cancelled"));
        if !retryable || !previous["retry_job_id"].is_null() {
            return Err(if has_saved_build {
                retry_already_claimed()
            } else {
                refusal(409, "Only an interrupted job may be retried once")
            });
        }
        if !queue_space(&self.con).map_err(failed)? {
            return Err(refusal(503, "Rust job queue is full"));
        }
        let target = previous["target_id"].as_str();
        let acknowledged = body["acknowledge_signature_risk"] == true;
        let inputs = match kind {
            "build" | "variant-build" | "supplement-build" if !has_saved_build => {
                if kind != "build" && target.is_none() {
                    return Err(refusal(409, "The prior job has no saved plan"));
                }
                let mut submission = self.prepare_build_submission(
                    matter,
                    kind,
                    if kind == "build" { None } else { target },
                    body,
                )?;
                submission.target = target.map(str::to_owned);
                submission.previous = Some(previous);
                return Ok(submission);
            }
            "build" | "variant-build" | "supplement-build" => SubmissionInputs::SavedBuild(
                StoreReader::capture(self).map_err(|_| saved_build_unavailable(&previous))?,
            ),
            "ocr" => {
                let document = target.ok_or_else(|| refusal(409, "OCR document was removed"))?;
                let payload = json!({
                    "language":body.get("language").cloned().unwrap_or(json!("eng")),
                    "acknowledge_signature_risk":acknowledged,
                });
                let (owner, context) = ocr::job_inputs(self, document, &payload, false)?;
                if owner != matter {
                    return Err(refusal(
                        409,
                        "OCR document no longer belongs to this matter",
                    ));
                }
                SubmissionInputs::Ocr {
                    body: payload,
                    context,
                }
            }
            _ => return Err(refusal(409, "This job type cannot be retried")),
        };
        Ok(ProcessingSubmission {
            root: self.root.clone(),
            matter: matter.into(),
            kind: kind.into(),
            target: target.map(str::to_owned),
            acknowledged,
            previous: Some(previous),
            inputs,
        })
    }

    pub fn submit_processing(
        &mut self,
        prepared: PreparedProcessingSubmission,
    ) -> Result<Reply, Reply> {
        self.require_open_processing_queue()?;
        let PreparedProcessingSubmission {
            root,
            matter,
            previous,
            preflight,
            queue,
        } = prepared;
        let failed = |_| transaction_failed();
        let stale = || refusal(409, BUILD_INPUT_CHANGED);
        if self.root != root {
            return Err(stale());
        }
        if let Some(run) = preflight {
            self.publish_build_preflight(run)?;
        }
        let (mut job, inputs) = queue?;
        if let PreparedInputs::Ocr(body) = &inputs {
            let target = job["target_id"].as_str().unwrap_or_default();
            let (owner, context) = ocr::job_inputs(self, target, body, false)?;
            if owner != matter || context != job["input_context"] {
                return Err(refusal(
                    409,
                    "Working revision changed before OCR submission",
                ));
            }
        }
        let tx = self
            .con
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        if !queue_space(&tx).map_err(failed)? {
            return Err(refusal(503, "Rust job queue is full"));
        }
        if let Some(previous) = &previous {
            let prior = previous["id"].as_str().unwrap_or_default();
            if load(&tx, prior).map_err(failed)?.as_ref() != Some(previous) {
                return Err(retry_already_claimed());
            }
            if document_metadata::matter(&tx, &matter)
                .map_err(failed)?
                .is_none()
            {
                return Err(refusal(404, "Matter not found"));
            }
        }
        match inputs {
            PreparedInputs::Build(inputs) => {
                let PreparedBuildInputs {
                    snapshot,
                    raw,
                    frozen,
                    sources,
                    proofs,
                    input_proofs,
                } = *inputs;
                frozen.check_current_inputs(&tx, &matter)?;
                if proofs.iter().any(|proof| proof.check_current().is_err())
                    || input_proofs
                        .iter()
                        .any(|proof| proof.check_current().is_err())
                {
                    return Err(stale());
                }
                tx.execute(
                    "INSERT INTO rust_frozen_builds VALUES (?,?,?)",
                    params![snapshot, matter, raw],
                )
                .map_err(failed)?;
                let mut insert = tx
                    .prepare("INSERT INTO rust_frozen_sources VALUES (?,?,?)")
                    .map_err(failed)?;
                for (index, bytes) in sources.into_iter().enumerate() {
                    insert
                        .execute(params![snapshot, index, bytes])
                        .map_err(failed)?;
                }
            }
            PreparedInputs::SavedBuild { raw, rows } => {
                let snapshot = snapshot_id(&job);
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM rust_frozen_builds WHERE id=? AND matter_id=? AND body=?)",
                    params![snapshot, matter, raw],
                    |row| row.get(0),
                ).map_err(failed)?;
                // Frozen source rows are write-once. Their identities and the snapshot
                // body bind the worker verification to the rows referenced by this job.
                if !exists || retained_source_rows(&tx, snapshot)? != rows {
                    return Err(refusal(409, "Saved build sources are unavailable"));
                }
            }
            PreparedInputs::Ocr(_) => {}
        }
        if let Some(mut previous) = previous {
            previous["retry_job_id"] = job["id"].clone();
            write(&tx, &previous).map_err(failed)?;
        }
        job["session_owner"] = json!(self.processing_session);
        write(&tx, &job).map_err(failed)?;
        tx.commit().map_err(failed)?;
        Ok(queued_reply(job))
    }

    pub(super) fn processing_request(
        &mut self,
        method: &str,
        route: &[&str],
        body: &Value,
    ) -> rusqlite::Result<Option<Reply>> {
        let result = match (method, route) {
            ("POST", ["matters", matter, "build"]) => self
                .prepare_build_submission(matter, "build", None, body)
                .and_then(ProcessingSubmission::compute)
                .and_then(|prepared| self.submit_processing(prepared))
                .unwrap_or_else(|error| error),
            ("POST", ["matters", matter, "variants" | "supplements", plan, "build"]) => {
                let kind = if route[2] == "variants" {
                    "variant-build"
                } else {
                    "supplement-build"
                };
                self.prepare_build_submission(matter, kind, Some(plan), body)
                    .and_then(ProcessingSubmission::compute)
                    .and_then(|prepared| self.submit_processing(prepared))
                    .unwrap_or_else(|error| error)
            }
            ("POST", ["documents", id, "ocr" | "ocr-rerun" | "compress"]) => {
                if let Err(error) = self.require_open_processing_queue() {
                    return Ok(Some(error));
                }
                let kind = if route[2] == "compress" {
                    "compress"
                } else {
                    "ocr"
                };
                let inputs = if kind == "compress" {
                    super::compression::job_inputs(self, id, body)
                } else {
                    ocr::job_inputs(self, id, body, route[2] == "ocr-rerun")
                };
                let (matter, context) = match inputs {
                    Ok(inputs) => inputs,
                    Err(error) => return Ok(Some(error)),
                };
                enqueue(
                    &self.con,
                    queued(&matter, kind, Some(id), context),
                    &self.processing_session,
                )?
            }
            ("POST", ["matters", matter, "preflight", "screen"]) => self
                .prepare_screen_submission(matter, body)
                .and_then(ScreenSubmission::verify)
                .and_then(|verified| self.submit_screening(verified))
                .unwrap_or_else(|error| error),
            ("GET", ["matters", matter, "jobs"]) => {
                if !self.active_matter(matter)? {
                    return Ok(Some(refusal(404, "Matter not found")));
                }
                let mut query = self.con.prepare(
                    "SELECT body FROM rust_processing_jobs WHERE matter_id=?
                     ORDER BY julianday(json_extract(body,'$.created_at')) DESC, rowid DESC",
                )?;
                let rows = query.query_map([matter], |row| decode(&row.get::<_, String>(0)?))?;
                let mut jobs = Vec::new();
                for job in rows {
                    let job = job?;
                    let live = job["id"].as_str().and_then(|id| self.active_jobs.get(id));
                    jobs.push(public_status(&self.con, job, live)?);
                }
                reply(200, json!(jobs))
            }
            ("GET", ["jobs", id]) => match load(&self.con, id)? {
                Some(job) => reply(
                    200,
                    public_status(&self.con, job, self.active_jobs.get(*id))?,
                ),
                None => refusal(404, "Job not found or lost after backend restart"),
            },
            ("POST", ["jobs", id, "cancel"]) => {
                let Some(mut job) = load(&self.con, id)?
                    .filter(|job| job["session_owner"] == self.processing_session)
                else {
                    return Ok(Some(refusal(404, "Job not found")));
                };
                if job["status"] == "queued" {
                    job["status"] = json!("cancelled");
                    job["message"] = json!("Cancelled");
                    job["updated_at"] = json!(now());
                    write(&self.con, &job)?;
                } else if let Some(live) = self.active_jobs.get(*id) {
                    live.cancelled.store(true, Ordering::Release);
                }
                reply(200, json!({"cancel_requested":true}))
            }
            ("POST", ["jobs", id, "retry"]) => self
                .prepare_retry(id, body)
                .and_then(ProcessingSubmission::compute)
                .and_then(|prepared| self.submit_processing(prepared))
                .unwrap_or_else(|error| error),
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    pub fn begin_work(&mut self) -> Result<Option<PendingWork>, String> {
        self.begin_work_with_updates(|_| {})
    }

    pub fn begin_work_with_updates(
        &mut self,
        mut updated: impl FnMut(&str),
    ) -> Result<Option<PendingWork>, String> {
        if self.processing_stopped || !self.active_jobs.is_empty() {
            return Ok(None);
        }
        loop {
            let job = self.con.query_row(
                "SELECT body FROM rust_processing_jobs WHERE json_extract(body,'$.status')='queued' ORDER BY rowid LIMIT 1",
                [], |row| decode(&row.get::<_,String>(0)?),
            ).optional().map_err(|error| error.to_string())?;
            let Some(mut job) = job else {
                return Ok(None);
            };
            let matter = job["matter_id"]
                .as_str()
                .ok_or("Invalid saved job matter")?
                .to_owned();
            let id = job["id"].as_str().ok_or("Invalid saved job ID")?.to_owned();
            let digest = job["input_context"]["input_digest"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            job["status"] = json!("running");
            job["started_at"] = json!(now());
            job["updated_at"] = job["started_at"].clone();
            job["message"] = json!("Starting");
            write(&self.con, &job).map_err(|error| error.to_string())?;
            let prepared = match job["kind"].as_str() {
                Some("ocr") => ocr::Work::prepare(
                    self,
                    job["target_id"].as_str().unwrap_or_default(),
                    &job["input_context"],
                )
                .map(Work::Ocr),
                Some("compress") => super::compression::Work::prepare(
                    self,
                    job["target_id"].as_str().unwrap_or_default(),
                    &job["input_context"],
                )
                .map(Work::Compress),
                Some("preflight-screen") => {
                    ocr::Screening::prepare(self, &matter, &digest).map(Work::Screen)
                }
                Some("build" | "variant-build" | "supplement-build") => StoreReader::capture(self)
                    .map(|reader| Work::Build(reader, job.clone()))
                    .map_err(|_| saved_build_unavailable(&job)),
                _ => Err(refusal(422, "Unrecognized processing job")),
            };
            match prepared {
                Ok(prepared) => {
                    let live = Arc::new(Live {
                        cancelled: AtomicBool::new(false),
                        progress: Mutex::new(LiveProgress::default()),
                    });
                    self.active_jobs.insert(id.clone(), Arc::clone(&live));
                    updated(&id);
                    return Ok(Some(PendingWork {
                        id,
                        matter,
                        digest,
                        job: prepared,
                        live,
                    }));
                }
                Err(error) => {
                    let failures = error.body.get("failures").cloned();
                    finish_job(&mut job, Err(error), false);
                    if let Some(failures) = failures {
                        job["result"]["failures"] = failures;
                    }
                    write(&self.con, &job).map_err(|error| error.to_string())?;
                    updated(&id);
                }
            }
        }
    }

    pub fn interrupt_processing_job(&mut self, id: &str) -> Result<(), String> {
        let result = (|| -> rusqlite::Result<()> {
            let Some(live) = self.active_jobs.get(id) else {
                return Ok(());
            };
            let Some(job) = load(&self.con, id)?.filter(|job| {
                job["status"] == "running" && job["session_owner"] == self.processing_session
            }) else {
                return Ok(());
            };
            let mut job = with_status(job, Some(live));
            if live.cancelled.load(Ordering::Acquire) {
                finish_job(&mut job, Err(refusal(409, "Operation cancelled")), true);
            } else {
                job["status"] = json!("interrupted");
                job["message"] = json!("Interrupted by worker failure");
                job["error"] = json!("The worker stopped before publishing a result");
                job["updated_at"] = json!(now());
            }
            write(&self.con, &job)
        })();
        self.active_jobs.remove(id);
        result.map_err(|error| error.to_string())
    }

    pub fn finish_work(&mut self, work: FinishedWork) -> Result<(), String> {
        let FinishedWork {
            id,
            matter,
            digest,
            product,
            live,
        } = work;
        let result = (|| -> rusqlite::Result<()> {
            let mut job = with_status(
                load(&self.con, &id)?.ok_or(rusqlite::Error::InvalidQuery)?,
                Some(&live),
            );
            let cancelled = live.cancelled.load(Ordering::Acquire);
            let product = match product {
                Ok(Product::Revision(product)) if !cancelled => {
                    let permitted = (|| {
                        let doc = self.artifact_document(&product.document)?.body;
                        if doc["revision"] != product.revision {
                            return Err(refusal(409, "Working revision changed during processing"));
                        }
                        let context = &job["input_context"];
                        let action = if job["kind"] == "compress" {
                            "compressing this working copy"
                        } else if context["rerun"] == true {
                            "rerunning OCR"
                        } else {
                            "running OCR"
                        };
                        super::page_tools::signature_guard(
                            [&doc],
                            context["acknowledge_signature_risk"] == true,
                            action,
                        )?;
                        if let Some(source) = &product.retained {
                            source.check_current()?;
                        }
                        Ok(())
                    })();
                    let completed = match permitted {
                        Ok(()) => self.publish_measured_revision(
                            &product.document,
                            product.revision,
                            product.candidate,
                            product.ocr,
                            super::annotations::Retention::None,
                            product.files,
                            |con, _| {
                                finish_job(&mut job, Ok(product.result), false);
                                write(con, &job)
                            },
                        )?,
                        Err(refusal) => refusal,
                    };
                    if completed.status != 200 {
                        finish_job(&mut job, Err(completed), false);
                        write(&self.con, &job)?;
                    }
                    return Ok(());
                }
                product => product,
            };
            let result = match product {
                Ok(_) if cancelled => Err(refusal(409, "Operation cancelled")),
                Ok(Product::Build(mut build)) => {
                    build.manifest["input_snapshot_id"] =
                        job["input_context"]["snapshot_id"].clone();
                    self.mark_stale(&matter, &mut build.manifest)?;
                    Ok(Product::Build(build))
                }
                product => product,
            };
            let tx = self
                .con
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let publishing_build = matches!(result, Ok(Product::Build(_)));
            let completed = match result {
                Ok(Product::Build(build)) => {
                    let summary = json!({
                        "export_id":id,"pages":build.manifest["pages"],"volumes":build.manifest["volumes"],
                        "files":build.manifest["files"],"checks":build.manifest["checks"],
                    });
                    super::generated_exports::publish(&tx, &matter, &id, build)?;
                    crash_at("build_files_staged");
                    Ok(summary)
                }
                Ok(Product::Screen(ocr::ScreeningProduct {
                    report,
                    result,
                    files,
                })) => {
                    let current = super::bundle_inputs::Inputs::metadata(&tx, &matter)?;
                    let unchanged = match current {
                        Ok(inputs) => inputs.effective_digest(&tx)? == digest,
                        Err(_) => false,
                    };
                    if unchanged && files.iter().all(|file| file.check_current().is_ok()) {
                        tx.execute("INSERT INTO rust_page_screenings VALUES (?,?,?) ON CONFLICT(matter_id,digest) DO UPDATE SET report=excluded.report",
                            params![matter,digest,report.to_string()])?;
                        Ok(result)
                    } else {
                        Err(refusal(
                            409,
                            "Saved inputs changed during screening; results were not published",
                        ))
                    }
                }
                Ok(Product::Revision(_)) => return Err(rusqlite::Error::InvalidQuery),
                Err(error) => Err(error),
            };
            finish_job(&mut job, completed, cancelled);
            write(&tx, &job)?;
            tx.commit()?;
            if publishing_build {
                crash_at("build_committed");
            }
            Ok(())
        })();
        self.active_jobs.remove(&id);
        if let Err(error) = result {
            if let Ok(Some(mut job)) = load(&self.con, &id) {
                if job["status"] == "running" {
                    finish_job(
                        &mut job,
                        Err(refusal(500, "Unable to publish saved output")),
                        false,
                    );
                    let _ = write(&self.con, &job);
                }
            }
            return Err(error.to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secure_db;

    const SOURCE: &[u8] = include_bytes!("../../../samples/rust_pdf_corpus/unsigned.pdf");

    fn open_test_store(root: &Path) -> Store {
        let key = DataKey::from_bytes([14; 32]);
        let db = root.join("test.sqlite3");
        if !db.exists() {
            secure_db::create_protected(&db, &key).unwrap();
        }
        let con = secure_db::open_protected(&db, &key).unwrap();
        Store::open_keyed(root, con, key).unwrap()
    }

    fn fixture(store: &mut Store) -> (String, String) {
        let created = store.request(
            "POST",
            "/matters",
            json!({"name":"Synthetic build recovery"}),
        );
        assert_eq!(created.status, 201, "{created:?}");
        let matter = created.body["id"].as_str().unwrap().to_owned();
        let doc = store.import_protected_pdf(
            &matter,
            "synthetic.pdf",
            SOURCE,
            ImportOptions {
                included: true,
                ..Default::default()
            },
        );
        assert_eq!(doc.status, 201, "{doc:?}");
        (matter, doc.body[0]["id"].as_str().unwrap().to_owned())
    }

    #[test]
    fn lost_worker_preserves_its_job_identity_and_committed_progress() {
        let root = tempfile::tempdir().unwrap();
        let mut store = open_test_store(root.path());
        let (matter, document) = fixture(&mut store);
        let response = store.request(
            "POST",
            &format!("/documents/{document}/compress"),
            json!({"expected_revision":1,"preset":"lossless"}),
        );
        assert_eq!(response.status, 202, "{response:?}");
        let id = response.body["id"].as_str().unwrap().to_owned();
        let mut updates = Vec::new();
        let pending = store
            .begin_work_with_updates(|id| updates.push(id.to_owned()))
            .unwrap()
            .unwrap();
        assert_eq!(updates, [id.clone()]);
        assert_eq!(pending.id(), id);
        {
            let mut progress = pending.live.progress.lock().unwrap();
            progress.percent = 24;
            progress.message = "Reading source".into();
            progress.updated_at = now();
            progress.events = vec![json!({"progress":24,"message":"Reading source","at":now()})];
        }
        store.checkpoint_processing_job(&id).unwrap();
        drop(pending);
        store.interrupt_processing_job(&id).unwrap();
        let job = store
            .request("GET", &format!("/jobs/{id}"), Value::Null)
            .body;
        assert_eq!(job["id"], id);
        assert_eq!(job["matter_id"], matter);
        assert_eq!(job["status"], "interrupted");
        assert_eq!(job["progress"], 24);
        assert_eq!(job["events"][0]["message"], "Reading source");
        assert!(job["retry_job_id"].is_null());
        assert!(store.begin_work().unwrap().is_none());
    }

    fn screen_request(store: &Store, matter: &str) -> Value {
        let inputs = super::super::bundle_inputs::Inputs::metadata(&store.con, matter)
            .unwrap()
            .ok()
            .unwrap();
        json!({"expected_snapshot":inputs.effective_digest(&store.con).unwrap()})
    }

    #[test]
    fn build_submission_checks_saved_inputs_and_commits_snapshot_with_job() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, document) = fixture(&mut store);
        let capture = |store: &Store| {
            store
                .prepare_build_submission(&matter, "build", None, &Value::Null)
                .unwrap()
        };
        let stale = capture(&store).compute().unwrap();
        let edit = store.request(
            "PATCH",
            &format!("/documents/{document}"),
            json!({"title":"Updated synthetic build input"}),
        );
        assert_eq!(edit.status, 200, "{edit:?}");
        let refused = store.submit_processing(stale).unwrap_err();
        assert_eq!(refused.status, 409);
        assert_eq!(refused.body["detail"], BUILD_INPUT_CHANGED);
        let counts = |store: &Store| {
            store
                .con
                .query_row(
                    "SELECT (SELECT count(*) FROM rust_frozen_builds),
                            (SELECT count(*) FROM rust_frozen_sources),
                            (SELECT count(*) FROM rust_processing_jobs)",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, u32>(0)?,
                            row.get::<_, u32>(1)?,
                            row.get::<_, u32>(2)?,
                        ))
                    },
                )
                .unwrap()
        };
        assert_eq!(counts(&store), (0, 0, 0));
        let current = capture(&store).compute().unwrap();
        assert_eq!(store.submit_processing(current).unwrap().status, 202);
        assert_eq!(counts(&store), (1, 1, 1));
    }

    #[test]
    fn primary_build_saves_worker_findings_with_the_same_contract_as_manual_preflight() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, _) = fixture(&mut store);
        let manual = store.request("GET", &format!("/matters/{matter}/validate"), Value::Null);
        assert_eq!(manual.status, 200);
        let history_path = format!("/matters/{matter}/preflight/history");
        let prepared = store
            .prepare_build_submission(&matter, "build", None, &Value::Null)
            .unwrap()
            .compute()
            .unwrap();
        let before = store.request("GET", &history_path, Value::Null);
        assert_eq!(before.body.as_array().unwrap().len(), 1);

        let expected = prepared.preflight.as_ref().unwrap().findings();
        let run_id = expected[0]["validation_run_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let queued = store.submit_processing(prepared).unwrap();
        let history = store.request("GET", &history_path, Value::Null);
        let records = history.body.as_array().unwrap();
        assert_eq!(records.len(), 2);
        let saved = records.iter().find(|run| run["id"] == run_id).unwrap();
        assert_eq!(saved["findings"], json!(&expected));
        let (_, frozen) = saved_build(&store.con, &queued.body).unwrap();
        assert_eq!(
            serde_json::to_value(frozen).unwrap()["inputs"]["checks"],
            saved["findings"]
        );

        let normalize = |mut findings: Vec<Value>| {
            for finding in &mut findings {
                finding.as_object_mut().unwrap().remove("validation_run_id");
            }
            findings
        };
        assert_eq!(
            normalize(expected),
            normalize(manual.body.as_array().unwrap().clone()),
        );
    }

    #[test]
    fn bundle_preview_and_primary_build_use_the_same_authenticated_page_plan() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, _) = fixture(&mut store);
        let preview = store.request(
            "POST",
            &format!("/matters/{matter}/bundle-preview"),
            json!({"max_pages_per_volume":300, "keep_groups_intact":true}),
        );
        assert_eq!(preview.status, 200, "{preview:?}");
        let build = store.request("POST", &format!("/matters/{matter}/build"), Value::Null);
        assert_eq!(build.status, 202, "{build:?}");
        let (_, inputs) = saved_build(&store.con, &build.body).unwrap();
        assert_eq!(
            serde_json::to_value(inputs).unwrap()["inputs"]["plan"],
            preview.body
        );
    }

    #[test]
    fn volume_planning_returns_latest_matching_measurements_after_the_worker() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, document) = fixture(&mut store);
        let settings = store.request(
            "GET",
            &format!("/matters/{matter}/bundle-settings"),
            Value::Null,
        );
        let snapshot = settings.body["input_snapshot"].as_str().unwrap();
        let request = json!({
            "max_pages_per_volume":settings.body["max_pages_per_volume"],
            "keep_groups_intact":settings.body["keep_groups_intact"],
            "output_marks":settings.body["output_marks"],
            "expected_snapshot":snapshot,
        });
        let measured = store
            .prepare_volume_planning(&matter, &request)
            .unwrap()
            .measure()
            .unwrap();
        let newest = enqueue(&mut store, &matter);
        let work = prepare(&mut store);
        store.finish_work(work).unwrap();
        let mut exception = queued(&matter, "build", None, Value::Null);
        exception["result"] = json!({"input_snapshot":snapshot,"size_exceptions":[{"bytes":504}]});
        exception["status"] = json!("failed");
        write(&store.con, &exception).unwrap();
        let mut empty = queued(&matter, "build", None, Value::Null);
        empty["result"] = json!({"input_snapshot":snapshot,"size_exceptions":[]});
        write(&store.con, &empty).unwrap();

        let response = store.complete_bundle_preview(measured).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body["documents"][0]["id"], document);
        assert_eq!(response.body["documents"][0]["bytes"], SOURCE.len());
        assert_eq!(response.body["generated"]["build_id"], newest);
        assert_eq!(
            response.body["measured_exception"]["job_id"],
            exception["id"]
        );
        assert!(response.body["plan"]["volumes"].is_array());
        assert!(response.body["plan"].get("input_snapshot").is_none());
    }

    #[test]
    fn composition_catalog_reuses_page_ids_after_worker_authentication() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, document) = fixture(&mut store);
        let path = format!("/matters/{matter}/supplements");
        let first = store.request("GET", &path, Value::Null);
        assert_eq!(first.status, 200);
        let verified = store
            .prepare_composition_catalog(&matter, CompositionCatalogKind::Supplements)
            .unwrap()
            .verify()
            .unwrap();
        let edit = store.request(
            "PATCH",
            &format!("/documents/{document}"),
            json!({"title":"Changed during catalog verification"}),
        );
        assert_eq!(edit.status, 200);
        assert_eq!(
            store
                .complete_composition_catalog(verified)
                .unwrap_err()
                .status,
            409
        );
        let current = store.request("GET", &path, Value::Null);
        assert_eq!(current.status, 200);
        assert_eq!(
            current.body["documents"][0]["pages"],
            first.body["documents"][0]["pages"]
        );
    }

    #[test]
    fn export_comparison_keeps_the_baseline_through_archive_and_rechecks_the_working_revision() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, document) = fixture(&mut store);
        let id = enqueue(&mut store, &matter);
        let work = prepare(&mut store);
        store.finish_work(work).unwrap();
        let map = store
            .prepare_export_comparison(
                &matter,
                &id,
                "compare",
                &json!({"max_pages_per_volume":300,"keep_groups_intact":true}),
            )
            .unwrap()
            .measure()
            .unwrap();
        let archive = store.request(
            "PUT",
            &format!("/matters/{matter}/exports/{id}/archive"),
            json!({"archived":true}),
        );
        assert_eq!(archive.status, 200);
        let comparison = store.complete_export_comparison(map).unwrap();
        assert_eq!(comparison.body["available"], true);
        assert_eq!(comparison.body["current_matches_prior"], true);
        let pages = store.prepare_export_comparison(&matter, &id, "compare-pages",
            &json!({"document_id":document,"prior_source_page":1,"current_source_page":1,"expected_revision":1}))
            .unwrap().measure().unwrap();
        let rotate = store.request(
            "POST",
            &format!("/documents/{document}/pages/rotate"),
            json!({"expected_revision":1,"page_index":0,"degrees":90}),
        );
        assert_eq!(rotate.status, 200, "{rotate:?}");
        assert_eq!(
            store.complete_export_comparison(pages).unwrap_err().status,
            409
        );
    }

    #[test]
    fn queued_ocr_and_compression_recheck_signature_review_before_processing() {
        let root = tempfile::tempdir().unwrap();
        let mut store = open_test_store(root.path());
        let (_, document) = fixture(&mut store);
        let mut jobs = Vec::new();
        for operation in ["ocr", "compress"] {
            let queued = store.request(
                "POST",
                &format!("/documents/{document}/{operation}"),
                json!({"expected_revision":1}),
            );
            assert_eq!(queued.status, 202, "{queued:?}");
            jobs.push(queued.body["id"].as_str().unwrap().to_owned());
        }
        let changed = store.request(
            "PATCH",
            &format!("/documents/{document}"),
            json!({"visible_signature_status":"present"}),
        );
        assert_eq!(changed.body["signature_status"], "visible_signature_only");
        assert!(store.begin_work().unwrap().is_none());
        for id in jobs {
            let stopped = load(&store.con, &id).unwrap().unwrap();
            assert_eq!(stopped["status"], "failed");
            assert!(stopped["error"]
                .as_str()
                .unwrap()
                .starts_with("Signature-risk acknowledgment required"));
        }
        assert_eq!(
            store.artifact_document(&document).unwrap().body["revision"],
            1
        );
    }

    #[test]
    fn stale_compression_is_queued_and_fails_before_reading_the_pdf() {
        let root = tempfile::tempdir().unwrap();
        let mut store = open_test_store(root.path());
        let (_, document) = fixture(&mut store);
        let queued = store.request(
            "POST",
            &format!("/documents/{document}/compress"),
            json!({"expected_revision":2}),
        );
        assert_eq!(queued.status, 202, "{queued:?}");
        assert!(store.begin_work().unwrap().is_none());
        let stopped = load(&store.con, queued.body["id"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(stopped["status"], "failed");
        assert_eq!(
            stopped["error"],
            "Document changed before compression; reload and try again"
        );
        assert_eq!(
            store.artifact_document(&document).unwrap().body["revision"],
            1
        );
    }

    #[test]
    fn compression_rechecks_original_bytes_and_signature_risk_at_publication() {
        let root = tempfile::tempdir().unwrap();
        let mut store = open_test_store(root.path());
        let (_, document) = fixture(&mut store);
        let queue = |store: &mut Store| {
            let response = store.request(
                "POST",
                &format!("/documents/{document}/compress"),
                json!({"expected_revision":1}),
            );
            assert_eq!(response.status, 202, "{response:?}");
            let measured = store.begin_work().unwrap().unwrap().compute();
            assert!(matches!(&measured.product, Ok(Product::Revision(_))));
            measured
        };
        let path = root
            .path()
            .join("documents-v1")
            .join(&document)
            .join("original.pdf");
        // Permit this fixture to inject a same-file content change after measurement.
        let permissions = fs::metadata(&path).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                &path,
                fs::Permissions::from_mode(permissions.mode() | 0o200),
            )
            .unwrap();
        }
        #[cfg(not(unix))]
        {
            let mut writable = permissions.clone();
            writable.set_readonly(false);
            fs::set_permissions(&path, writable).unwrap();
        }
        let measured = queue(&mut store);
        let id = measured.id.clone();
        let original = fs::read(&path).unwrap();
        let mut changed = original.clone();
        changed[64] ^= 1;
        fs::write(&path, changed).unwrap();
        store.finish_work(measured).unwrap();
        assert_eq!(load(&store.con, &id).unwrap().unwrap()["status"], "failed");
        assert_eq!(
            store.artifact_document(&document).unwrap().body["revision"],
            1
        );
        fs::write(&path, original).unwrap();
        fs::set_permissions(&path, permissions).unwrap();

        let measured = queue(&mut store);
        let id = measured.id.clone();
        let signed = store.request(
            "PATCH",
            &format!("/documents/{document}"),
            json!({"visible_signature_status":"present"}),
        );
        assert_eq!(signed.body["signature_status"], "visible_signature_only");
        store.finish_work(measured).unwrap();
        let stopped = load(&store.con, &id).unwrap().unwrap();
        assert_eq!(stopped["status"], "failed");
        assert!(stopped["error"]
            .as_str()
            .unwrap()
            .starts_with("Signature-risk acknowledgment required"));
        assert_eq!(
            store.artifact_document(&document).unwrap().body["revision"],
            1
        );
        assert_eq!(
            store
                .open_pdf(&document, &Artifact::Original)
                .unwrap()
                .into_bytes()
                .unwrap(),
            SOURCE
        );
    }

    #[test]
    fn text_inspection_rejects_a_changed_revision_or_working_file_after_processing() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (_, document) = fixture(&mut store);
        let path = format!("/documents/{document}/pages/0/text-runs");
        let capture = |store: &Store| {
            store
                .prepare_text_page(
                    super::super::TextPageTarget::from_path("GET", &path).unwrap(),
                    &Value::Null,
                )
                .unwrap()
                .compute()
                .unwrap()
        };
        let measured = capture(&store);
        let rotate = store.request(
            "POST",
            &format!("/documents/{document}/pages/rotate"),
            json!({"expected_revision":1,"page_index":0,"degrees":90}),
        );
        assert_eq!(rotate.status, 200, "{rotate:?}");
        assert_eq!(store.complete_text_page(measured).unwrap_err().status, 409);

        let measured = capture(&store);
        let record = super::super::revisions::record(&store.con, &document, 2)
            .unwrap()
            .unwrap();
        let working = directory.path().join(record.relpath);
        let modified = fs::metadata(&working).unwrap().modified().unwrap();
        let mut bytes = fs::read(&working).unwrap();
        bytes[64] ^= 1;
        fs::write(&working, bytes).unwrap();
        File::options()
            .write(true)
            .open(&working)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_eq!(store.complete_text_page(measured).unwrap_err().status, 409);
    }

    #[test]
    fn measured_text_edits_recheck_signature_risk_and_publish_one_revision() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (_, document) = fixture(&mut store);
        let path = format!("/documents/{document}/pages/replace-text-run");
        let capture = |store: &Store, acknowledge| {
            store.prepare_text_page(
                super::super::TextPageTarget::from_path("POST", &path).unwrap(),
                &json!({
                    "expected_revision":1,"page_index":0,"span_index":0,
                    "expected_text":"MatterDraft synthetic unsigned control",
                    "replacement":"Updated control text","acknowledge_signature_risk":acknowledge
                }),
            ).unwrap().compute().unwrap()
        };
        let awaiting_review = capture(&store, false);
        let signed = store.request(
            "PATCH",
            &format!("/documents/{document}"),
            json!({"visible_signature_status":"present"}),
        );
        assert_eq!(signed.body["signature_status"], "visible_signature_only");
        let refused = store.complete_text_page(awaiting_review).unwrap_err();
        assert_eq!(refused.status, 409);
        assert!(refused.body["detail"]
            .as_str()
            .unwrap()
            .starts_with("Signature-risk acknowledgment required"));

        let first = capture(&store, true);
        let second = capture(&store, true);
        let edited = store.complete_text_page(first).unwrap();
        assert_eq!(edited.status, 200, "{edited:?}");
        assert_eq!(edited.body["revision"], 2);
        assert_eq!(store.complete_text_page(second).unwrap_err().status, 409);
        assert_eq!(
            store
                .open_pdf(&document, &Artifact::Original)
                .unwrap()
                .into_bytes()
                .unwrap(),
            SOURCE
        );
    }

    #[test]
    fn build_authenticates_reference_only_originals_on_the_worker() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, _) = fixture(&mut store);
        let reference_pdf =
            include_bytes!("../../../samples/rust_pdf_corpus/object_stream_unsigned.pdf");
        let reference = store.import_protected_pdf(
            &matter,
            "reference.pdf",
            reference_pdf,
            ImportOptions::default(),
        );
        assert_eq!(reference.status, 201, "{reference:?}");
        assert_eq!(reference.body[0]["bundle_included"], 0);
        let document = reference.body[0]["id"].as_str().unwrap();
        let path = directory
            .path()
            .join("documents-v1")
            .join(document)
            .join("original.pdf");
        let capture = || {
            store
                .prepare_build_submission(&matter, "build", None, &Value::Null)
                .unwrap()
        };
        let pending = capture();
        let replace = |bytes: &[u8]| {
            vault::publish_encrypted(&path, store.key.as_ref().unwrap(), |file| {
                file.write_all(bytes)?;
                Ok(())
            })
            .unwrap();
        };
        let mut changed = reference_pdf.to_vec();
        changed[10] ^= 1;
        replace(&changed);
        let error = pending.compute().err().unwrap();
        assert_eq!(error.status, 409);
        assert_eq!(
            error.body["detail"],
            "Imported original failed hash verification"
        );
        // Restore the protected fixture, then change the reference after measurement.
        replace(reference_pdf);
        let measured = capture().compute().unwrap();
        replace(&changed);
        assert_eq!(store.submit_processing(measured).unwrap_err().status, 409);
        let history = store.request(
            "GET",
            &format!("/matters/{matter}/preflight/history"),
            Value::Null,
        );
        assert_eq!(history.body, json!([]));
    }

    #[test]
    fn bundle_preview_refuses_the_map_after_a_saved_input_edit() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, document) = fixture(&mut store);
        let measured = store
            .prepare_bundle_preview(
                &matter,
                &json!({"max_pages_per_volume":300, "keep_groups_intact":true}),
            )
            .unwrap()
            .measure()
            .unwrap();
        let edit = store.request(
            "PATCH",
            &format!("/documents/{document}"),
            json!({"title":"Edited while planning"}),
        );
        assert_eq!(edit.status, 200);
        assert_eq!(
            store.complete_bundle_preview(measured).unwrap_err().status,
            409
        );
    }

    #[test]
    fn manual_preflight_refuses_to_record_findings_after_a_saved_edit() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, document) = fixture(&mut store);
        let measured = store.prepare_preflight(&matter).unwrap().measure().unwrap();
        let edit = store.request(
            "PATCH",
            &format!("/documents/{document}"),
            json!({"title":"Changed after Preflight started"}),
        );
        assert_eq!(edit.status, 200);
        let refused = store.publish_preflight(measured).unwrap_err();
        assert_eq!(refused.status, 409);
        let history = store.request(
            "GET",
            &format!("/matters/{matter}/preflight/history"),
            Value::Null,
        );
        assert_eq!(history.body, json!([]));
    }

    #[test]
    fn manual_preflight_records_a_missing_original_with_unavailable_digests() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, document) = fixture(&mut store);
        let original = directory
            .path()
            .join("documents-v1")
            .join(&document)
            .join("original.pdf");
        std::fs::rename(original, directory.path().join("unavailable-synthetic.pdf")).unwrap();
        let measured = store.prepare_preflight(&matter).unwrap().measure().unwrap();
        let run = store.publish_preflight(measured).unwrap();
        assert_eq!(run.status, 200);
        let findings = run.body.as_array().unwrap();
        assert!(findings
            .iter()
            .any(|item| item["code"] == "MISSING_ORIGINAL"
                && item["document_id"] == document
                && item["blocks_build"] == true));
        assert!(findings.iter().all(
            |item| item["input_snapshot"].is_null() && item["effective_input_digest"].is_null()
        ));
        let history = store.request(
            "GET",
            &format!("/matters/{matter}/preflight/history"),
            Value::Null,
        );
        assert_eq!(history.body[0]["findings"], run.body);
    }

    #[test]
    fn variant_review_and_build_measure_the_same_plan_and_recheck_edits() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, document) = fixture(&mut store);
        let variants = format!("/matters/{matter}/variants");
        let catalog = store.request("GET", &variants, Value::Null);
        assert_eq!(catalog.status, 200);
        let mut body = catalog.body["blank_settings"].clone();
        body["baseline_kind"] = json!("primary");
        body["baseline_id"] = catalog.body["primary_baseline"]["id"].clone();
        body["expected_snapshot"] = catalog.body["source_snapshot"].clone();
        body["included_ids"] = json!([document]);
        body["name"] = json!("Synthetic variant");
        let saved = store.request("POST", &variants, body.clone());
        assert_eq!(saved.status, 200, "{saved:?}");
        let plan = saved.body["id"].as_str().unwrap();
        let edit_path = format!("{variants}/{plan}");
        body["expected_version"] = saved.body["version"].clone();
        let prepare_edit = |name: &str| {
            let target = CompositionDraftTarget::from_path("PUT", &edit_path).unwrap();
            let mut request = body.clone();
            request["name"] = json!(name);
            store
                .prepare_composition_draft(target, &request)
                .unwrap()
                .measure()
                .unwrap()
        };
        let first = prepare_edit("First staged edit");
        let second = prepare_edit("Competing staged edit");
        let updated = store.complete_composition_draft(first).unwrap();
        assert_eq!(updated.body["version"], 2);
        let refused = store.complete_composition_draft(second).unwrap_err();
        assert_eq!(refused.status, 409);
        assert_eq!(
            refused.body["detail"],
            "Plan changed in another view. Reload before saving."
        );
        let review = store.request("GET", &format!("{variants}/{plan}/review"), Value::Null);
        assert_eq!(review.status, 200, "{review:?}");
        let work = store
            .prepare_build_submission(&matter, "variant-build", Some(plan), &Value::Null)
            .unwrap()
            .compute()
            .unwrap();
        let queued = store.submit_processing(work).unwrap();
        let (_, frozen) = saved_build(&store.con, &queued.body).unwrap();
        assert_eq!(
            serde_json::to_value(frozen).unwrap()["inputs"]["checks"],
            review.body["checks"]
        );
        let stale = store
            .prepare_variant_review(&matter, plan)
            .unwrap()
            .measure()
            .unwrap();
        let edit = store.request(
            "PATCH",
            &format!("/documents/{document}"),
            json!({"title":"Changed after variant inspection"}),
        );
        assert_eq!(edit.status, 200);
        assert_eq!(
            store.complete_variant_review(stale).unwrap_err().status,
            409
        );
    }

    #[test]
    fn supplement_policy_preview_save_and_build_reuse_the_same_selected_page_map() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, document) = fixture(&mut store);
        let baseline = enqueue(&mut store, &matter);
        let rendered = prepare(&mut store);
        store.finish_work(rendered).unwrap();
        let plans = format!("/matters/{matter}/supplements");
        let catalog = store.request("GET", &plans, Value::Null);
        assert_eq!(catalog.status, 200, "{catalog:?}");
        let source = catalog.body["documents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["id"] == document)
            .unwrap();
        let mut request = catalog.body["defaults"].clone();
        request["title"] = json!("Synthetic supplement");
        request["baseline_build_id"] = json!(baseline);
        request["selected_page_ids"] = json!([source["pages"][0]["id"]]);
        request["expected_snapshot"] = catalog.body["input_snapshot"].clone();
        request["insertion_policy"] = json!("separate_supplement");
        let preview = store.request(
            "POST",
            &format!("/matters/{matter}/supplement-policy-preview"),
            request.clone(),
        );
        assert_eq!(preview.status, 200, "{preview:?}");
        assert_eq!(preview.body["blockers"], json!([]));
        assert!(preview.body["plan_id"].is_null());
        request["policy_reviewed"] = json!(true);
        let saved = store.request("POST", &plans, request);
        assert_eq!(saved.status, 200, "{saved:?}");
        let plan = saved.body["id"].as_str().unwrap();
        let build = store.request("POST", &format!("{plans}/{plan}/build"), Value::Null);
        assert_eq!(build.status, 202, "{build:?}");
        let (_, frozen) = saved_build(&store.con, &build.body).unwrap();
        let encoded = serde_json::to_value(frozen).unwrap();
        let policy = &encoded["inputs"]["insertion_policy_contract"];
        assert_eq!(policy["artifact_map"], preview.body["artifact_map"]);
        assert_eq!(policy["selected_page_ids"], saved.body["selected_page_ids"]);
        assert_eq!(
            saved.body["reviewed_policy"]["preview_digest"],
            sha256_hex(policy.to_string().as_bytes())
        );
    }

    #[test]
    fn replacing_an_original_after_build_inspection_refuses_the_preflight_record() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, document) = fixture(&mut store);
        let prepared = store
            .prepare_build_submission(&matter, "build", None, &Value::Null)
            .unwrap()
            .compute()
            .unwrap();
        let original = directory
            .path()
            .join("documents-v1")
            .join(document)
            .join("original.pdf");
        let mut changed = SOURCE.to_vec();
        changed[10] ^= 1;
        vault::publish_encrypted(&original, store.key.as_ref().unwrap(), |output| {
            output.write_all(&changed)?;
            Ok(())
        })
        .unwrap();
        let refused = store.submit_processing(prepared).unwrap_err();
        assert_eq!(refused.status, 409);
        assert_eq!(refused.body["detail"], BUILD_INPUT_CHANGED);
        let history = store.request(
            "GET",
            &format!("/matters/{matter}/preflight/history"),
            Value::Null,
        );
        assert_eq!(history.body, json!([]));
        let jobs = store.request("GET", &format!("/matters/{matter}/jobs"), Value::Null);
        assert_eq!(jobs.body, json!([]));
    }

    #[test]
    fn build_source_refusal_preserves_preflight_history_without_queueing() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, document) = fixture(&mut store);
        let marked = store.request(
            "POST",
            &format!("/documents/{document}/annotations"),
            json!({
                "kind":"redaction", "expected_revision":1, "page_index":0, "output_scope":"pdf",
                "rect_x":0.15, "rect_y":0.45, "rect_width":0.12, "rect_height":0.08,
            }),
        );
        assert_eq!(marked.status, 201, "{marked:?}");
        let rotated = store.request(
            "POST",
            &format!("/documents/{document}/pages/rotate"),
            json!({"expected_revision":1, "page_index":0, "degrees":90}),
        );
        assert_eq!(rotated.status, 200, "{rotated:?}");
        let refused = store.request("POST", &format!("/matters/{matter}/build"), Value::Null);
        assert_eq!(refused.status, 409, "{refused:?}");
        assert_eq!(
            refused.body["detail"],
            "Editable PDF marks need review after the working PDF changed"
        );
        let history = store.request(
            "GET",
            &format!("/matters/{matter}/preflight/history"),
            Value::Null,
        );
        assert_eq!(history.body.as_array().unwrap().len(), 1);
        let jobs = store.request("GET", &format!("/matters/{matter}/jobs"), Value::Null);
        assert_eq!(jobs.body, json!([]));
    }

    #[test]
    fn build_worker_reads_protected_overlay_and_rejects_changed_bytes() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
        use std::io::Cursor;

        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, document) = fixture(&mut store);
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, Rgb([24, 72, 144])))
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let marked = store.request(
            "POST",
            &format!("/documents/{document}/annotations/image"),
            json!({
                "filename":"synthetic.png", "data":STANDARD.encode(png.into_inner()),
                "expected_revision":1, "page_index":0,
                "rect_x":0.15, "rect_y":0.45, "rect_width":0.12, "rect_height":0.08,
            }),
        );
        assert_eq!(marked.status, 201, "{marked:?}");
        let capture = |store: &Store| {
            store
                .prepare_build_submission(&matter, "build", None, &Value::Null)
                .unwrap()
        };
        let prepared = capture(&store).compute().unwrap();
        let (_, PreparedInputs::Build(inputs)) = prepared.queue.as_ref().ok().unwrap() else {
            panic!("Expected a frozen build")
        };
        assert_ne!(inputs.sources[0], SOURCE);
        let stale = capture(&store);
        assert_eq!(
            store
                .con
                .execute("UPDATE rust_annotation_assets SET bytes=x'00'", [])
                .unwrap(),
            1
        );
        let refused = store
            .submit_processing(stale.compute().unwrap())
            .unwrap_err();
        assert_eq!(refused.status, 409);
        assert_eq!(refused.body["detail"], BUILD_INPUT_CHANGED);
        assert_eq!(store.submit_processing(prepared).unwrap_err().status, 409);
    }

    #[test]
    fn build_submission_rejects_replaced_working_file_before_queueing() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, document) = fixture(&mut store);
        let work = store
            .prepare_build_submission(&matter, "build", None, &Value::Null)
            .unwrap()
            .compute()
            .unwrap();
        let relative: String = store
            .con
            .query_row(
                "SELECT working_relpath FROM documents WHERE id=?",
                [&document],
                |row| row.get(0),
            )
            .unwrap();
        let working = directory.path().join(relative);
        let modified = std::fs::metadata(&working).unwrap().modified().unwrap();
        let mut changed = SOURCE.to_vec();
        changed[10] ^= 1;
        vault::publish_encrypted(&working, store.key.as_ref().unwrap(), |output| {
            output.write_all(&changed)?;
            Ok(())
        })
        .unwrap();
        File::options()
            .write(true)
            .open(&working)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let refused = store.submit_processing(work).unwrap_err();
        assert_eq!(refused.status, 409);
        assert_eq!(refused.body["detail"], BUILD_INPUT_CHANGED);
        let jobs = store.request("GET", &format!("/matters/{matter}/jobs"), Value::Null);
        assert_eq!(jobs.body, json!([]));
    }

    #[test]
    fn concurrent_retry_preparations_can_claim_the_same_attempt_once() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, document) = fixture(&mut store);
        let prior = enqueue(&mut store, &matter);
        let stopped = store.request("POST", &format!("/jobs/{prior}/cancel"), Value::Null);
        assert_eq!(stopped.status, 200);
        let original = load(&store.con, &prior).unwrap().unwrap();
        let edit = store.request(
            "PATCH",
            &format!("/documents/{document}"),
            json!({"title":"Changed live build inputs"}),
        );
        assert_eq!(edit.status, 200, "{edit:?}");
        let prepare = |store: &Store| {
            store
                .prepare_retry(&prior, &Value::Null)
                .unwrap()
                .compute()
                .unwrap()
        };
        let first = prepare(&store);
        let concurrent = prepare(&store);
        let queued = store.submit_processing(first).unwrap();
        assert_eq!(queued.status, 202);
        assert_eq!(queued.body["input_context"], original["input_context"]);
        let refused = store.submit_processing(concurrent).unwrap_err();
        assert_eq!(refused.status, 409);
        assert_eq!(
            refused.body["detail"],
            retry_already_claimed().body["detail"]
        );
        let saved = load(&store.con, &prior).unwrap().unwrap();
        assert_eq!(saved["retry_job_id"], queued.body["id"]);
        let jobs = store.request("GET", &format!("/matters/{matter}/jobs"), Value::Null);
        assert_eq!(jobs.body.as_array().unwrap().len(), 2);
    }

    #[test]
    fn retained_source_corruption_or_removal_cannot_claim_a_retry() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, _) = fixture(&mut store);
        let prior = enqueue(&mut store, &matter);
        let stopped = store.request("POST", &format!("/jobs/{prior}/cancel"), Value::Null);
        assert_eq!(stopped.status, 200);
        let verified = store
            .prepare_retry(&prior, &Value::Null)
            .unwrap()
            .compute()
            .unwrap();
        assert_eq!(
            store
                .con
                .execute(
                    "UPDATE rust_frozen_sources SET bytes=zeroblob(length(bytes))",
                    []
                )
                .unwrap(),
            1,
        );
        let error = store
            .prepare_retry(&prior, &Value::Null)
            .unwrap()
            .compute()
            .err()
            .unwrap();
        assert_eq!(error.status, 409);
        assert_eq!(
            error.body["detail"],
            "Saved bundle source failed its integrity check"
        );
        assert_eq!(
            store
                .con
                .execute("DELETE FROM rust_frozen_sources", [])
                .unwrap(),
            1
        );
        let error = store.submit_processing(verified).unwrap_err();
        assert_eq!(error.status, 409);
        let previous = load(&store.con, &prior).unwrap().unwrap();
        assert_eq!(previous["status"], "cancelled");
        assert!(previous["retry_job_id"].is_null());
        let jobs = store.request("GET", &format!("/matters/{matter}/jobs"), Value::Null);
        assert_eq!(jobs.body.as_array().unwrap().len(), 1);
    }

    #[test]
    fn queued_build_worker_authenticates_retained_bytes_before_output() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, _) = fixture(&mut store);
        let id = enqueue(&mut store, &matter);
        let pending = store.begin_work().unwrap().unwrap();
        assert_eq!(load(&store.con, &id).unwrap().unwrap()["status"], "running");
        assert_eq!(
            store
                .con
                .execute(
                    "UPDATE rust_frozen_sources SET bytes=zeroblob(length(bytes))",
                    []
                )
                .unwrap(),
            1,
        );
        let finished = pending.compute();
        let error = finished.product.as_ref().err().unwrap();
        assert_eq!(
            error.body["detail"],
            "Saved bundle source failed its integrity check"
        );
        store.finish_work(finished).unwrap();
        assert_eq!(load(&store.con, &id).unwrap().unwrap()["status"], "failed");
        assert_eq!(counts(&store, &id), (0, 0, 0));
    }

    #[test]
    fn screen_submission_rechecks_changes_and_queues_once() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, document) = fixture(&mut store);
        let prepare = |store: &Store| {
            store
                .prepare_screen_submission(&matter, &screen_request(store, &matter))
                .ok()
                .unwrap()
                .verify()
                .ok()
                .unwrap()
        };
        let stale = prepare(&store);
        let edit = store.request(
            "PATCH",
            &format!("/documents/{document}"),
            json!({"title":"Updated synthetic metadata"}),
        );
        assert_eq!(edit.status, 200);
        assert_eq!(
            store.submit_screening(stale).unwrap_err().body["detail"],
            SCREEN_STALE
        );

        let first = prepare(&store);
        let duplicate = prepare(&store);
        assert_eq!(store.submit_screening(first).unwrap().status, 202);
        let refusal = store.submit_screening(duplicate).unwrap_err();
        assert_eq!(refusal.status, 409);
        assert_eq!(
            refusal.body["detail"],
            "Wait for the active operation before screening pages"
        );
        assert_eq!(
            store
                .request("GET", &format!("/matters/{matter}/jobs"), Value::Null)
                .body
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn screen_publication_checks_files_and_preserves_previous_report() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = open_test_store(directory.path());
        let (matter, document) = fixture(&mut store);
        let post = format!("/matters/{matter}/preflight/screen");
        let body = screen_request(&store, &matter);
        let before = store.request("POST", &post, body.clone());
        assert_eq!(before.status, 202);
        let computed = store.begin_work().unwrap().unwrap().compute();
        store.finish_work(computed).unwrap();
        let report = |store: &Store| -> String {
            store
                .con
                .query_row(
                    "SELECT report FROM rust_page_screenings WHERE matter_id=?",
                    [&matter],
                    |row| row.get(0),
                )
                .unwrap()
        };
        let previous = report(&store);

        let next = store.request("POST", &post, body);
        assert_eq!(next.status, 202);
        let computed = store.begin_work().unwrap().unwrap().compute();
        assert!(matches!(&computed.product, Ok(Product::Screen(_))));
        let original = store
            .root
            .join("documents-v1")
            .join(document)
            .join("original.pdf");
        let modified = std::fs::metadata(&original).unwrap().modified().unwrap();
        let mut changed = SOURCE.to_vec();
        changed[10] ^= 1;
        vault::publish_encrypted(&original, store.key.as_ref().unwrap(), |output| {
            output.write_all(&changed)?;
            Ok(())
        })
        .unwrap();
        File::options()
            .write(true)
            .open(&original)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        store.finish_work(computed).unwrap();
        let job = load(&store.con, next.body["id"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(job["status"], "failed");
        assert_eq!(
            job["error"],
            "Saved inputs changed during screening; results were not published"
        );
        assert_eq!(report(&store), previous);
    }

    fn enqueue(store: &mut Store, matter: &str) -> String {
        let response = store.request("POST", &format!("/matters/{matter}/build"), json!({}));
        assert_eq!(response.status, 202, "{response:?}");
        response.body["id"].as_str().unwrap().to_owned()
    }

    fn prepare(store: &mut Store) -> FinishedWork {
        let work = store.begin_work().unwrap().unwrap().compute();
        assert!(
            matches!(work.product, Ok(Product::Build(_))),
            "Expected a build"
        );
        work
    }

    fn counts(store: &Store, id: &str) -> (i64, i64, i64) {
        store
            .con
            .query_row(
                "SELECT (SELECT count(*) FROM rust_exports WHERE id=?1),
                        (SELECT count(*) FROM rust_export_files WHERE export_id=?1),
                        (SELECT count(*) FROM rust_export_checks WHERE export_id=?1)",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
    }

    fn verify_output(store: &Store, matter: &str, id: &str) -> Vec<u8> {
        let persisted = load(&store.con, id).unwrap().unwrap();
        let job = public_status(&store.con, persisted, None).unwrap();
        assert_eq!(job["status"], "completed");
        assert_eq!(job["result"]["id"], id);
        assert_eq!(job["result"]["export_id"], id);
        assert!(job["result"]["page_map_snapshot"].is_array());
        let files = job["result"]["files"].as_array().unwrap();
        assert!(files.len() >= 4, "{job}");
        assert_eq!(counts(store, id), (1, files.len() as i64 + 1, 1));
        let read_file = |name: &str| -> Vec<u8> {
            store
                .con
                .query_row(
                    "SELECT content FROM rust_export_files WHERE export_id=? AND filename=?",
                    params![id, name],
                    |row| row.get(0),
                )
                .unwrap()
        };
        for file in files {
            let bytes = read_file(file["name"].as_str().unwrap());
            assert_eq!(file["bytes"], bytes.len());
            assert_eq!(file["sha256"], sha256_hex(&bytes));
        }
        assert_eq!(
            read_file("READ_ME_FIRST.txt"),
            super::super::bundle_writer::draft_readme(&job["result"]["artifact_kind"]).into_bytes()
        );
        store
            .export_snapshot(matter, id, "combined.pdf", "inspection", &Value::Null)
            .unwrap()
            .into_bytes()
            .unwrap()
    }

    #[test]
    fn a_job_recovers_through_restart_with_its_committed_progress() {
        let root = tempfile::tempdir().unwrap();
        let mut store = open_test_store(root.path());
        let (matter, _) = fixture(&mut store);
        let id = enqueue(&mut store, &matter);
        let work = store.begin_work().unwrap().unwrap();
        let finished = work.compute_with_checkpoint(|job| {
            store.checkpoint_processing_job(job)?;
            let persisted = load(&store.con, job).unwrap().unwrap();
            assert_eq!(persisted["status"], "running");
            assert!(!persisted["events"].as_array().unwrap().is_empty());
            Ok(())
        });
        assert!(matches!(finished.product, Ok(Product::Build(_))));
        drop(finished);
        let before = load(&store.con, &id).unwrap().unwrap();
        assert!(before["progress"].as_u64().unwrap() > 0);
        drop(store);

        let mut store = open_test_store(root.path());
        let job = store.request("GET", &format!("/jobs/{id}"), Value::Null);
        assert_eq!(job.body["status"], "interrupted");
        assert_eq!(job.body["events"], before["events"]);
        assert_eq!(job.body["progress"], before["progress"]);
        assert!(job.body.get("session_owner").is_none());
        let cancelled = store.request("POST", &format!("/jobs/{id}/cancel"), Value::Null);
        assert_eq!(cancelled.status, 404);
        let retried = store.request("POST", &format!("/jobs/{id}/retry"), Value::Null);
        assert_eq!(retried.status, 202);
    }

    #[test]
    fn graceful_shutdown_cancels_queued_and_running_jobs_before_publication() {
        let root = tempfile::tempdir().unwrap();
        let mut store = open_test_store(root.path());
        let (matter, _) = fixture(&mut store);
        let running = enqueue(&mut store, &matter);
        let queued = enqueue(&mut store, &matter);
        let measured = prepare(&mut store);
        store.stop_processing().unwrap();
        assert!(store.begin_work().unwrap().is_none());
        let refused = store.request("POST", &format!("/matters/{matter}/build"), Value::Null);
        assert_eq!(refused.status, 503);
        store.finish_work(measured).unwrap();
        drop(store);

        let store = open_test_store(root.path());
        let stopped = load(&store.con, &running).unwrap().unwrap();
        assert_eq!(stopped["status"], "cancelled");
        assert_eq!(stopped["error"], "Operation cancelled");
        assert_eq!(counts(&store, &running), (0, 0, 0));
        let stopped = load(&store.con, &queued).unwrap().unwrap();
        assert_eq!(stopped["status"], "cancelled");
        assert!(stopped["result"].is_null());
        assert!(stopped["error"].is_null());
        assert!(stopped["started_at"].is_null());
    }

    #[test]
    fn protected_build_rolls_back_when_sqlite_is_full() {
        let root = tempfile::tempdir().unwrap();
        let mut store = open_test_store(root.path());
        let (matter, document) = fixture(&mut store);
        let prior = enqueue(&mut store, &matter);
        let work = prepare(&mut store);
        store.finish_work(work).unwrap();
        let prior_bytes = verify_output(&store, &matter, &prior);
        let stopped = enqueue(&mut store, &matter);
        let work = prepare(&mut store);

        store.con.execute_batch("VACUUM").unwrap();
        let pages: i64 = store
            .con
            .pragma_query_value(None, "page_count", |r| r.get(0))
            .unwrap();
        let limit: i64 = store
            .con
            .pragma_query_value(None, "max_page_count", |r| r.get(0))
            .unwrap();
        store
            .con
            .pragma_update(None, "max_page_count", pages)
            .unwrap();
        let error = store.finish_work(work).unwrap_err();
        assert!(error.contains("full"), "{error}");
        store
            .con
            .pragma_update(None, "max_page_count", limit)
            .unwrap();
        assert_eq!(counts(&store, &stopped), (0, 0, 0));
        drop(store);

        let mut store = open_test_store(root.path());
        secure_db::verify_integrity(&store.con).unwrap();
        let stopped_job = load(&store.con, &stopped).unwrap().unwrap();
        assert!(matches!(
            stopped_job["status"].as_str(),
            Some("failed" | "interrupted")
        ));
        assert_eq!(counts(&store, &stopped), (0, 0, 0));
        assert_eq!(verify_output(&store, &matter, &prior), prior_bytes);
        assert_eq!(
            store
                .open_pdf(&document, &Artifact::Original)
                .unwrap()
                .into_bytes()
                .unwrap(),
            SOURCE
        );
        assert!(store.begin_work().unwrap().is_none());

        let retry_path = format!("/jobs/{stopped}/retry");
        let retry = store.request("POST", &retry_path, json!({}));
        assert_eq!(retry.status, 202, "{retry:?}");
        let work = prepare(&mut store);
        store.finish_work(work).unwrap();
        verify_output(&store, &matter, retry.body["id"].as_str().unwrap());
        assert_eq!(store.request("POST", &retry_path, json!({})).status, 409);
    }

    #[test]
    #[ignore = "child process entry point"]
    fn protected_build_publication_child() {
        let root = std::env::var_os("MATTERDRAFT_BUILD_CRASH_ROOT").unwrap();
        let mut store = open_test_store(Path::new(&root));
        let matter = std::env::var("MATTERDRAFT_BUILD_CRASH_MATTER").unwrap();
        // Initialization marks old queued work interrupted. Submit in this process.
        let id = enqueue(&mut store, &matter);
        std::fs::write(store.root.join("fixture-job-id"), id).unwrap();
        let work = prepare(&mut store);
        store.finish_work(work).unwrap();
    }

    #[test]
    fn protected_build_publication_reopens_at_both_commit_boundaries() {
        use std::process::Command;

        let root = tempfile::tempdir().unwrap();
        let mut store = open_test_store(root.path());
        let (matter, _) = fixture(&mut store);
        drop(store);
        for phase in ["build_files_staged", "build_committed"] {
            let child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "matters::processing_jobs::tests::protected_build_publication_child",
                    "--ignored",
                ])
                .env("MATTERDRAFT_BUILD_CRASH_ROOT", root.path())
                .env("MATTERDRAFT_BUILD_CRASH_MATTER", &matter)
                .env("MATTERDRAFT_TEST_CRASH_PHASE", phase)
                .output()
                .unwrap();
            assert_eq!(child.status.code(), Some(97), "{phase}: {child:?}");
            let id = std::fs::read_to_string(root.path().join("fixture-job-id")).unwrap();
            let mut store = open_test_store(root.path());
            secure_db::verify_integrity(&store.con).unwrap();
            if phase == "build_committed" {
                verify_output(&store, &matter, &id);
            } else {
                assert_eq!(counts(&store, &id), (0, 0, 0));
                assert_eq!(
                    load(&store.con, &id).unwrap().unwrap()["status"],
                    "interrupted"
                );
            }
            assert!(store.begin_work().unwrap().is_none());
        }
    }
}
