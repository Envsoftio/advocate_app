import threading
import time

from app.jobs import JobManager


def test_cancelling_queued_job_never_runs_its_task():
    jobs = JobManager()
    first_started = threading.Event()
    release_first = threading.Event()
    second_ran = threading.Event()

    def first_task(_job_id, _progress, _cancelled):
        first_started.set()
        assert release_first.wait(5)
        return {'first': True}

    def second_task(_job_id, _progress, _cancelled):
        second_ran.set()
        return {'second': True}

    try:
        jobs.submit('synthetic', 'build', first_task)
        assert first_started.wait(5)
        queued = jobs.submit('synthetic', 'build', second_task)
        assert jobs.cancel(queued['id'])['status'] == 'queued'
        release_first.set()
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and jobs.get(queued['id'])['status'] == 'queued':
            time.sleep(.01)
        assert jobs.get(queued['id'])['status'] == 'cancelled'
        assert not second_ran.is_set()
    finally:
        release_first.set()
        jobs.shutdown()
