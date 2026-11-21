"""Convert a disposable Python vault and exercise the migrated Rust workflows."""
from hashlib import sha256
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

from vault_contract import PASSWORD, ROOT, TestClient, create_app, running_vault
from app import build_inputs
from app.storage import new_id

CARGO = ROOT / 'rust-backend/Cargo.toml'
CONVERTER = ROOT / 'rust-backend/target/debug/import_python_snapshot'


def inventory(root):
    return {str(p.relative_to(root)): sha256(p.read_bytes()).hexdigest()
            for p in root.rglob('*') if p.is_file()}


def create_legacy_vault(root):
    token = 'synthetic-snapshot-fixture'
    auth = {'Authorization': f'Bearer {token}'}
    original = (ROOT / 'samples/01_petition.pdf').read_bytes()
    with TestClient(create_app(root, token=token, require_vault=True)) as python:
        def request(method, path, **kwargs):
            response = python.request(method, path, headers={**auth, **kwargs.pop('headers', {})}, **kwargs)
            response.raise_for_status()
            return response

        setup = request('POST', '/vault/setup', json={'password': PASSWORD}).json()
        matter = request('POST', '/matters', json={'name': 'Synthetic Migration Fixture'}).json()['id']
        archived = request('POST', '/matters', json={'name': 'Synthetic Archive Fixture'}).json()['id']
        request('DELETE', f'/matters/{archived}')
        imported = request('POST', f'/matters/{matter}/documents/raw?included=true',
                           headers={'X-CourtBundle-Filename': 'migration.pdf', 'Content-Type': 'application/octet-stream'},
                           content=original).json()[0]
        document = imported['id']
        membership = request('GET', f'/matters/{matter}/membership').json()
        if imported['bundle_included'] != 1:
            request('PUT', f'/matters/{matter}/membership', json={
                'included_ids': [document], 'expected_snapshot': membership['input_snapshot'],
            })
        revision = request('POST', f'/documents/{document}/pages/rotate',
                           json={'expected_revision': 1, 'page_index': 0, 'degrees': 90}).json()['revision']
        working = request('GET', f'/documents/{document}/pdf?revision={revision}').content
        reader_key = f'{document}:{revision}'
        reader = {'page': 2, 'x': 0.25, 'y': 0.5, 'zoom': '100', 'mode': 'continuous'}
        request('PUT', '/reader-position', json={'key': reader_key, **reader})
        store = python.app.state.store
        frozen = build_inputs.capture(store, matter)
        retry, missing, contextless = new_id(), new_id(), new_id()
        for job_id, context in (
            (retry, frozen),
            (missing, {**frozen, 'snapshot_id': new_id()}),
            (contextless, None),
        ):
            store.save_job(dict(id=job_id, matter_id=matter, kind='build', status='interrupted',
                                progress=0, message='Interrupted', input_context=context))
        job = request('POST', f'/matters/{matter}/build', json={}).json()['id']
        deadline = time.monotonic() + 90
        while True:
            state = request('GET', f'/jobs/{job}').json()
            if state['status'] == 'completed':
                break
            assert state['status'] in {'queued', 'running'}, state
            assert time.monotonic() < deadline, state
            time.sleep(0.1)
        build = state['result']['id']
        inspection = request('GET', f'/matters/{matter}/exports/{build}/inspection/combined.pdf').content
        parameters = {
            'job': job, 'job_result': state['result'], 'build': build, 'matter': matter, 'archived': archived,
            'document': document, 'revision': revision, 'reader_key': reader_key, 'reader': reader,
            'original': original, 'working': working, 'inspection': inspection,
            'recovery': setup['recovery_key'], 'retry': retry, 'missing': missing,
            'contextless': contextless,
        }
        request('PUT', f'/matters/{matter}/exports/{build}/archive', json={'archived': True})
    return parameters


def run():
    subprocess.run(['cargo', 'build', '--locked', '--offline', '--manifest-path', CARGO,
                    '--bin', 'import_python_snapshot', '--example', 'vault_contract'], check=True)
    with tempfile.TemporaryDirectory(prefix='matterdraft-vault-migration-') as temporary:
        root = Path(temporary)
        source, snapshot, target = root / 'python', root / 'closed copy #1', root / 'rust'
        fixture = create_legacy_vault(source)
        before = inventory(source)
        shutil.copytree(source, snapshot)
        snapshot_before = inventory(snapshot)
        rejected = subprocess.run([CONVERTER, snapshot, target], input='wrong password\n',
                                  text=True, capture_output=True)
        assert rejected.returncode and not target.exists()
        subprocess.run([CONVERTER, snapshot, target, '--recovery'],
                       input=fixture['recovery'] + '\n', text=True, check=True)
        assert before == inventory(source)
        assert snapshot_before == inventory(snapshot)
        target_before = inventory(target)
        occupied = subprocess.run([CONVERTER, snapshot, target], input=PASSWORD + '\n',
                                  text=True, capture_output=True)
        assert occupied.returncode and target_before == inventory(target)

        with running_vault(target) as rust:
            rust('POST', '/vault/unlock', {'credential': PASSWORD})
            matter, document, build = fixture['matter'], fixture['document'], fixture['build']
            assert [row['id'] for row in rust('GET', '/matters')] == [matter]
            assert [row['id'] for row in rust('GET', '/matters/archived')] == [fixture['archived']]
            assert rust('GET', f"/reader-position?key={fixture['reader_key']}") == fixture['reader']
            path = f'/documents/{document}'
            assert bytes(rust('SNAPSHOT', path, {'kind': 'original'})['bytes']) == fixture['original']
            assert bytes(rust('SNAPSHOT', path, {'kind': 'working', 'revision': fixture['revision']})['bytes']) == fixture['working']
            completed = rust('GET', f"/jobs/{fixture['job']}")
            assert completed['status'] == 'completed'
            assert completed['result'] == fixture['job_result']
            manifest = rust('GET', f'/matters/{matter}/exports')[0]
            assert manifest['id'] == build and manifest['archived_at']
            export = f'/matters/{matter}/exports/{build}'
            from base64 import b64decode
            output = rust('GET', f'{export}/inspection/combined.pdf')
            assert b64decode(output['data']) == fixture['inspection']
            reviewed = rust('GET', f'{export}/validation/state')
            assert reviewed['run']['selected_files']
            rerun = rust('POST', f'{export}/validation/run', {'files': ['combined.pdf']})
            checks = rerun['history'][0]['checks']
            assert {item['id']: item['outcome'] for item in checks}['navigation'] == 'passed'
            undone = rust('POST', f'/matters/{matter}/history/undo')
            assert undone['can_redo']
            item = rust('GET', f'/matters/{matter}/documents')[0]
            assert bytes(rust('SNAPSHOT', path, {'kind': 'working', 'revision': item['revision']})['bytes']) == fixture['original']
            missing_job = f"/jobs/{fixture['missing']}"
            assert rust('GET', missing_job)['status'] == 'interrupted'
            rust('POST', f'{missing_job}/retry', {}, status=409)
            migrated_job = f"/jobs/{fixture['retry']}"
            state = rust('GET', migrated_job)
            assert not state.get('legacy_retry_blocker'), state
            retry = rust('POST', f'{migrated_job}/retry', {}, status=202)
            rust('RUN_PENDING', '')
            completed = rust('GET', f"/jobs/{retry['id']}")
            assert completed['status'] == 'completed', completed
            rebuilt = next(export for export in rust('GET', f'/matters/{matter}/exports')
                           if export['id'] == completed['id'])
            assert completed['result']['id'] == rebuilt['id']
            assert completed['result']['input_snapshot_id'] == rebuilt['input_snapshot_id']
            assert completed['result']['page_map_snapshot'] == rebuilt['page_map_snapshot']
            assert rebuilt['bundle_membership']['documents'][0]['working_revision'] == fixture['revision']
            assert rebuilt['stale']
            contextless = f"/jobs/{fixture['contextless']}"
            retried = rust('POST', f'{contextless}/retry', {}, status=202)
            assert rust('GET', contextless)['retry_job_id'] == retried['id']
            rust('POST', f'{contextless}/retry', {}, status=409)
            rust('RUN_PENDING', '')
            completed = rust('GET', f"/jobs/{retried['id']}")
            assert completed['status'] == 'completed', completed
            rebuilt = next(export for export in rust('GET', f'/matters/{matter}/exports')
                           if export['id'] == completed['id'])
            assert rebuilt['bundle_membership']['documents'][0]['working_revision'] == item['revision']
            assert not rebuilt['stale']


if __name__ == '__main__':
    run()
