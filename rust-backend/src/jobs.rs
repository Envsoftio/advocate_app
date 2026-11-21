//! Ephemeral, bounded PDF extraction jobs. No UI, store, or process dependency.
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;

const MAX_INPUT: usize = 16 * 1024 * 1024;
const MAX_QUEUED_BYTES: usize = 32 * 1024 * 1024;
const MAX_RESULT_BYTES: usize = 20 * 1024 * 1024;
const MAX_RETAINED: usize = 16;
const RETENTION: Duration = Duration::from_secs(600);
const ERROR: &str = "PDF extraction failed or exceeds supported limits";

#[derive(Clone, Debug, Serialize)]
pub struct JobView {
    pub id: String,
    pub kind: &'static str,
    pub status: &'static str,
    pub progress: u8,
    pub message: &'static str,
    pub error: Option<String>,
    pub page_count: Option<usize>,
    pub completed_pages: usize,
    pub cancel_requested: bool,
}
struct Entry {
    view: JobView,
    input: Option<Vec<u8>>,
    result: Option<Vec<String>>,
    delay_ms: u64,
    finished: Option<Instant>,
}
#[derive(Default)]
struct State {
    entries: HashMap<String, Entry>,
    queue: VecDeque<String>,
    running: usize,
    input_bytes: usize,
    result_bytes: usize,
    stopped: bool,
}
#[derive(Clone, Default)]
pub struct Jobs {
    state: Arc<Mutex<State>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum JobError {
    Busy,
    Invalid,
    Missing,
    NotReady,
    Stopped,
}

impl Jobs {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn start(&self, bytes: Vec<u8>, delay_ms: u64) -> Result<JobView, JobError> {
        if bytes.is_empty()
            || bytes.len() > MAX_INPUT
            || delay_ms > 2_000
            || (!cfg!(debug_assertions) && delay_ms != 0)
        {
            return Err(JobError::Invalid);
        }
        let mut state = self.state.lock().unwrap();
        Self::expire(&mut state);
        if state.stopped {
            return Err(JobError::Stopped);
        }
        if state.queue.len() >= 4
            || state.input_bytes.saturating_add(bytes.len()) > MAX_QUEUED_BYTES
            || state.entries.len() >= MAX_RETAINED
        {
            return Err(JobError::Busy);
        }
        let id = Uuid::new_v4().simple().to_string();
        let view = JobView {
            id: id.clone(),
            kind: "pdf-text-extract",
            status: "queued",
            progress: 0,
            message: "Queued",
            error: None,
            page_count: None,
            completed_pages: 0,
            cancel_requested: false,
        };
        state.input_bytes += bytes.len();
        state.entries.insert(
            id.clone(),
            Entry {
                view: view.clone(),
                input: Some(bytes),
                result: None,
                delay_ms,
                finished: None,
            },
        );
        state.queue.push_back(id);
        Self::dispatch(&self.state, &mut state);
        Ok(view)
    }
    pub fn status(&self, id: &str) -> Result<JobView, JobError> {
        let mut state = self.state.lock().unwrap();
        Self::expire(&mut state);
        state
            .entries
            .get(id)
            .map(|e| e.view.clone())
            .ok_or(JobError::Missing)
    }
    pub fn cancel(&self, id: &str) -> Result<JobView, JobError> {
        let mut state = self.state.lock().unwrap();
        Self::expire(&mut state);
        let entry = state.entries.get_mut(id).ok_or(JobError::Missing)?;
        if entry.view.status == "queued" {
            state.queue.retain(|queued| queued != id);
            let entry = state.entries.get_mut(id).unwrap();
            let len = entry.input.take().unwrap().len();
            entry.view.status = "cancelled";
            entry.view.message = "Cancelled";
            entry.view.cancel_requested = true;
            entry.finished = Some(Instant::now());
            state.input_bytes -= len;
        } else if entry.view.status == "running" {
            entry.view.cancel_requested = true;
            entry.view.message = "Cancellation requested";
        }
        Ok(state.entries[id].view.clone())
    }
    pub fn result(&self, id: &str) -> Result<Vec<String>, JobError> {
        let mut state = self.state.lock().unwrap();
        Self::expire(&mut state);
        let entry = state.entries.get(id).ok_or(JobError::Missing)?;
        if entry.view.status != "completed" {
            return Err(JobError::NotReady);
        }
        Ok(entry.result.as_ref().unwrap().clone())
    }
    pub fn shutdown(&self) {
        let mut state = self.state.lock().unwrap();
        state.stopped = true;
        for entry in state.entries.values_mut() {
            if entry.view.status == "running" {
                entry.view.cancel_requested = true;
            }
        }
        while let Some(id) = state.queue.pop_front() {
            let entry = state.entries.get_mut(&id).unwrap();
            let len = entry.input.take().unwrap().len();
            entry.view.status = "cancelled";
            entry.view.message = "Cancelled";
            entry.view.cancel_requested = true;
            entry.finished = Some(Instant::now());
            state.input_bytes -= len;
        }
    }
    fn expire(state: &mut State) {
        let expired: Vec<_> = state
            .entries
            .iter()
            .filter(|(_, e)| e.finished.is_some_and(|at| at.elapsed() >= RETENTION))
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            if let Some(e) = state.entries.remove(&id) {
                state.result_bytes -= e
                    .result
                    .as_ref()
                    .map_or(0, |pages| pages.iter().map(String::len).sum::<usize>());
            }
        }
    }
    fn dispatch(shared: &Arc<Mutex<State>>, state: &mut State) {
        while state.running < 2 && !state.stopped {
            let Some(id) = state.queue.pop_front() else {
                break;
            };
            let entry = state.entries.get_mut(&id).unwrap();
            let bytes = entry.input.take().unwrap();
            entry.view.status = "running";
            entry.view.message = "Starting";
            state.running += 1;
            let delay_ms = entry.delay_ms;
            let owned = Arc::clone(shared);
            let input_len = bytes.len();
            let worker_id = id.clone();
            let launched = std::thread::Builder::new()
                .name("matterdraft-pdf-job".into())
                .spawn(move || {
                    let outcome = catch_unwind(AssertUnwindSafe(|| {
                        Self::extract(&owned, &worker_id, bytes, delay_ms)
                    }));
                    let mut state = owned.lock().unwrap();
                    state.running -= 1;
                    state.input_bytes -= input_len;
                    let stopped = state.stopped;
                    let result_budget = state.result_bytes;
                    let mut added_bytes = 0;
                    if let Some(entry) = state.entries.get_mut(&worker_id) {
                        if entry.view.cancel_requested || stopped {
                            entry.view.status = "cancelled";
                            entry.view.message = "Cancelled";
                        } else if let Ok(Ok(pages)) = outcome {
                            let size: usize = pages.iter().map(String::len).sum();
                            if result_budget.saturating_add(size) <= MAX_RESULT_BYTES {
                                entry.view.status = "completed";
                                entry.view.message = "Completed";
                                entry.view.progress = 100;
                                entry.result = Some(pages);
                                added_bytes = size;
                            } else {
                                entry.view.status = "failed";
                                entry.view.message = "Failed";
                                entry.view.error = Some("Result capacity exhausted".into());
                            }
                        } else {
                            entry.view.status = "failed";
                            entry.view.message = "Failed";
                            entry.view.error = Some(ERROR.into());
                        }
                        entry.finished = Some(Instant::now());
                    }
                    state.result_bytes += added_bytes;
                    Self::dispatch(&owned, &mut state);
                });
            if launched.is_err() {
                state.running -= 1;
                state.input_bytes -= input_len;
                let entry = state.entries.get_mut(&id).unwrap();
                entry.view.status = "failed";
                entry.view.message = "Failed";
                entry.view.error = Some("Could not start in-process worker".into());
                entry.finished = Some(Instant::now());
            }
        }
    }
    fn extract(
        shared: &Arc<Mutex<State>>,
        id: &str,
        bytes: Vec<u8>,
        delay_ms: u64,
    ) -> Result<Vec<String>, ()> {
        #[cfg(test)]
        if delay_ms == 1_999 {
            panic!("synthetic worker panic");
        }
        let pdf = pdf_oxide::PdfDocument::from_bytes(bytes).map_err(|_| ())?;
        if pdf.is_encrypted() {
            return Err(());
        }
        let count = pdf.page_count().map_err(|_| ())?;
        if count > 2_000 {
            return Err(());
        }
        {
            let mut state = shared.lock().unwrap();
            state.entries.get_mut(id).unwrap().view.page_count = Some(count);
        }
        let mut pages = Vec::with_capacity(count);
        let mut total = 0;
        for i in 0..count {
            if shared.lock().unwrap().entries[id].view.cancel_requested {
                return Err(());
            }
            if delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
            let text = pdf.extract_text(i).map_err(|_| ())?;
            total += text.len();
            if total > 10_000_000 {
                return Err(());
            }
            pages.push(text);
            let mut state = shared.lock().unwrap();
            let view = &mut state.entries.get_mut(id).unwrap().view;
            view.completed_pages = i + 1;
            view.progress = (((i + 1) * 99) / count.max(1)) as u8;
            view.message = "Extracting text";
        }
        Ok(pages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pdf() -> Vec<u8> {
        include_bytes!("../../samples/01_petition.pdf").to_vec()
    }
    fn terminal(jobs: &Jobs, id: &str) -> JobView {
        for _ in 0..400 {
            let view = jobs.status(id).unwrap();
            if !["queued", "running"].contains(&view.status) {
                return view;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("job did not finish");
    }
    #[test]
    fn progress_result_and_restart_expiry() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/job_contract.json")).unwrap();
        assert_eq!(fixture["rust_terminal_success"], "completed");
        let jobs = Jobs::new();
        let first = jobs.start(pdf(), 40).unwrap();
        let done = terminal(&jobs, &first.id);
        assert_eq!(done.status, "completed");
        assert_eq!(done.progress, 100);
        assert_eq!(done.completed_pages, 3);
        assert!(jobs.result(&first.id).unwrap()[0]
            .to_lowercase()
            .contains("petition"));
        assert_eq!(
            Jobs::new().status(&first.id).unwrap_err(),
            JobError::Missing
        );
        jobs.state
            .lock()
            .unwrap()
            .entries
            .get_mut(&first.id)
            .unwrap()
            .finished = Some(Instant::now() - RETENTION);
        assert_eq!(jobs.status(&first.id).unwrap_err(), JobError::Missing);
    }
    #[test]
    fn queued_running_cancel_and_repeated_cancel() {
        let jobs = Jobs::new();
        let a = jobs.start(pdf(), 150).unwrap();
        let b = jobs.start(pdf(), 150).unwrap();
        let c = jobs.start(pdf(), 0).unwrap();
        assert_eq!(c.status, "queued");
        assert_eq!(jobs.cancel(&c.id).unwrap().status, "cancelled");
        assert_eq!(jobs.cancel(&c.id).unwrap().status, "cancelled");
        assert_eq!(terminal(&jobs, &c.id).status, "cancelled");
        assert_eq!(jobs.result(&c.id).unwrap_err(), JobError::NotReady);
        assert!(jobs.cancel(&a.id).unwrap().cancel_requested);
        assert_eq!(terminal(&jobs, &a.id).status, "cancelled");
        assert_eq!(terminal(&jobs, &b.id).status, "completed");
        assert_eq!(jobs.state.lock().unwrap().input_bytes, 0);
    }
    #[test]
    fn detached_caller_does_not_cancel_worker() {
        let jobs = Jobs::new();
        let observer = jobs.clone();
        let id = jobs.start(pdf(), 80).unwrap().id;
        drop(jobs);
        assert_eq!(terminal(&observer, &id).status, "completed");
        assert_eq!(observer.state.lock().unwrap().input_bytes, 0);
    }
    #[test]
    fn saturation_errors_panic_and_shutdown_release_capacity() {
        let jobs = Jobs::new();
        assert_eq!(jobs.start(vec![], 0).unwrap_err(), JobError::Invalid);
        let a = jobs.start(pdf(), 150).unwrap();
        let b = jobs.start(pdf(), 150).unwrap();
        let mut queued = vec![];
        for _ in 0..4 {
            queued.push(jobs.start(pdf(), 0).unwrap());
        }
        assert_eq!(jobs.start(pdf(), 0).unwrap_err(), JobError::Busy);
        jobs.shutdown();
        for job in queued {
            let fixture: serde_json::Value =
                serde_json::from_str(include_str!("../tests/job_contract.json")).unwrap();
            assert_eq!(
                jobs.status(&job.id).unwrap().status,
                fixture["rust_queued_shutdown_in_memory"].as_str().unwrap()
            );
        }
        assert_eq!(terminal(&jobs, &a.id).status, "cancelled");
        assert_eq!(terminal(&jobs, &b.id).status, "cancelled");
        assert_eq!(jobs.state.lock().unwrap().input_bytes, 0);
        assert_eq!(jobs.start(pdf(), 0).unwrap_err(), JobError::Stopped);
        let failed = Jobs::new();
        let bad = failed.start(b"bad PDF".to_vec(), 0).unwrap();
        assert_eq!(terminal(&failed, &bad.id).status, "failed");
        let panic_job = failed.start(pdf(), 1_999).unwrap();
        assert_eq!(terminal(&failed, &panic_job.id).status, "failed");
        assert_eq!(failed.state.lock().unwrap().running, 0);
    }
}
