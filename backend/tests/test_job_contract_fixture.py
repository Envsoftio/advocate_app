import json
import threading
import time
from pathlib import Path

from app.jobs import JobManager
from app.storage import Store


def test_shared_job_contract_fixture():
    fixture = json.loads((Path(__file__).parents[2] / 'rust-backend/tests/job_contract.json').read_text())
    jobs = JobManager()
    started = threading.Event()
    release = threading.Event()

    def task(_id, progress, cancelled):
        started.set()
        release.wait(5)
        if cancelled():
            raise RuntimeError('cancelled')
        progress(50, 'Halfway')
        return {'ok': True}

    try:
        first = jobs.submit('synthetic', 'fixture', task)
        assert started.wait(5)
        assert jobs.get(first['id'])['status'] == fixture['running']
        second = jobs.submit('synthetic', 'fixture', task)
        assert second['status'] == fixture['queued']
        assert jobs.cancel(second['id'])['status'] == fixture['queued']
        assert jobs.cancel(first['id'])['status'] == fixture['running']
        assert fixture['cancel_request_is_terminal'] is False
        release.set()
        jobs.shutdown()
        assert jobs.get(first['id'])['status'] == fixture['cancelled']
        assert jobs.get(second['id'])['status'] == fixture['python_queued_shutdown_in_memory']
        assert jobs.get('missing') is None
    finally:
        release.set()
        jobs.shutdown()


def test_shared_job_success_and_missing_result_contract():
    fixture = json.loads((Path(__file__).parents[2] / 'rust-backend/tests/job_contract.json').read_text())
    jobs = JobManager()
    try:
        finished = threading.Event()

        def task(_id, progress, _cancelled):
            progress(50, 'Halfway')
            finished.set()
            return {'pages': ['Synthetic page']}

        started = jobs.submit('synthetic', 'fixture', task)
        assert len(started['id']) == 32
        assert finished.wait(5)
        deadline = time.monotonic() + 5
        while jobs.get(started['id'])['status'] != fixture['reference_terminal_success']:
            assert time.monotonic() < deadline
            time.sleep(.01)
        done = jobs.get(started['id'])
        assert done['status'] == fixture['rust_terminal_success']
        assert done['progress'] == 100
        assert done['result'] == {'pages': ['Synthetic page']}
        assert jobs.cancel(started['id'])['status'] == fixture['reference_terminal_success']
        assert jobs.get('missing') is None
    finally:
        jobs.shutdown()


def test_shutdown_closes_durable_queued_job_before_restart(tmp_path):
    root = tmp_path / 'synthetic-jobs'
    store = Store(root)
    jobs = JobManager(store)
    started = threading.Event()
    release = threading.Event()
    queued_ran = threading.Event()

    def blocking_task(_id, _progress, cancelled):
        started.set()
        deadline = time.monotonic() + 5
        while not cancelled() and not release.is_set() and time.monotonic() < deadline:
            release.wait(.01)
        assert cancelled()
        raise RuntimeError('cancelled')

    def queued_task(_id, _progress, _cancelled):
        queued_ran.set()

    try:
        jobs.submit('synthetic', 'fixture', blocking_task)
        assert started.wait(5)
        queued = jobs.submit('synthetic', 'fixture', queued_task)
        jobs.shutdown()
        assert not queued_ran.is_set()
        assert jobs.get(queued['id'])['status'] == 'cancelled'
        assert store.get_job(queued['id'])['status'] == 'cancelled'
    finally:
        release.set()
        jobs.shutdown()
        store.close()

    reopened = Store(root)
    try:
        JobManager(reopened).shutdown()
        assert reopened.get_job(queued['id'])['status'] == 'cancelled'
    finally:
        reopened.close()
