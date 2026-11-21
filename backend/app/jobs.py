from __future__ import annotations
from concurrent.futures import ThreadPoolExecutor
import threading
from .storage import Store, new_id, utcnow


class JobManager:
    """Sequential worker avoids concurrent PyMuPDF processing in MatterDraft.
    Durable states identify interrupted work after restart; retries begin new jobs.
    """
    def __init__(self, store: Store | None = None):
        self._executor = ThreadPoolExecutor(max_workers=1, thread_name_prefix='filing-worker')
        self._jobs = {}
        self._lock = threading.Lock()
        self._store = store
        if store:
            store.recover_interrupted_work()

    def submit(self, matter_id: str, kind: str, task, target_id: str | None = None,
               job_id: str | None = None, input_context: dict | None = None):
        job_id = job_id or new_id()
        job = {'id': job_id, 'matter_id': matter_id, 'kind': kind, 'status': 'queued',
               'target_id': target_id, 'progress': 0, 'message': 'Queued',
               'input_context': input_context, 'events': [], 'error': None, 'result': None, 'retry_job_id': None, 'created_at': utcnow()}
        cancelled = threading.Event()
        with self._lock:
            self._jobs[job_id] = (job, cancelled)
            if self._store:
                self._store.save_job(job)

        def progress(value: int, message: str):
            with self._lock:
                job['progress'] = max(0, min(100, value))
                job['message'] = message
                if not job['events'] or job['events'][-1]['message'] != message:
                    job['events'].append(dict(progress=job['progress'], message=message, at=utcnow()))
                if self._store:
                    self._store.save_job(job)

        def run():
            with self._lock:
                if cancelled.is_set():
                    job.update(status='cancelled', message='Cancelled')
                    if self._store:
                        self._store.save_job(job)
                    return
                job.update(status='running', message='Starting', started_at=utcnow())
                if self._store:
                    self._store.save_job(job)
            try:
                result = task(job_id, progress, cancelled.is_set)
                with self._lock:
                    # Tasks must raise when cancellation prevents their commit.
                    # A late request cannot undo a successfully published result.
                    job.update(status='completed', result=result,
                               progress=100, message='Completed')
                    if self._store:
                        self._store.save_job(job)
            except Exception as exc:
                with self._lock:
                    job.update(status='cancelled' if cancelled.is_set() else 'failed',
                               error=str(exc), result=getattr(exc, 'recovery', None) or (dict(failures=[dict(
                                   code='BUILD_FAILED', severity='error', message=str(exc))])
                                   if kind in ('build', 'supplement-build', 'variant-build') else None), message='Cancelled' if cancelled.is_set() else 'Failed')
                    if self._store:
                        self._store.save_job(job)

        self._executor.submit(run)
        return dict(job)

    def get(self, job_id):
        with self._lock:
            item = self._jobs.get(job_id)
            return dict(item[0]) if item else (self._store.get_job(job_id) if self._store else None)

    def cancel(self, job_id):
        with self._lock:
            item = self._jobs.get(job_id)
            if not item:
                return None
            if item[0]['status'] in ('queued', 'running'):
                item[1].set()
            return dict(item[0])

    def claim_retry(self, job_id: str, retry_job_id: str) -> dict | None:
        """Record one retry for a durable interrupted job."""
        if not self._store:
            return None
        with self._lock:
            job = self._store.get_job(job_id)
            if not job or (job['status'] != 'interrupted' and not (job.get('input_context') and job['status'] in ('failed', 'cancelled'))) or job['retry_job_id']:
                return None
            job['retry_job_id'] = retry_job_id
            self._store.save_job(job)
            if job_id in self._jobs:
                self._jobs[job_id][0]['retry_job_id'] = retry_job_id
            return job

    def release_retry(self, job_id, retry_job_id):
        with self._lock:
            job = self._store.get_job(job_id)
            if job and job.get('retry_job_id') == retry_job_id:
                job['retry_job_id'] = None
                self._store.save_job(job)
                if job_id in self._jobs:
                    self._jobs[job_id][0]['retry_job_id'] = None

    def shutdown(self):
        with self._lock:
            for job, cancelled in self._jobs.values():
                if job['status'] in ('queued', 'running'):
                    cancelled.set()
        self._executor.shutdown(wait=True, cancel_futures=True)
        # cancel_futures prevents queued run() closures from publishing a terminal
        # state. Close those records explicitly, including the durable copy.
        with self._lock:
            for job, _ in self._jobs.values():
                if job['status'] == 'queued':
                    job.update(status='cancelled', message='Cancelled')
                    if self._store:
                        self._store.save_job(job)
