"""Reusable live Python/Rust contract comparison; only disposable synthetic stores."""
import json
import pathlib
import subprocess
import sys
import sqlite3
import re
from datetime import datetime
import tempfile
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / 'backend'))
from app.main import create_app
from fastapi.testclient import TestClient

repo = pathlib.Path(__file__).resolve().parents[2]
fixtures = json.loads((pathlib.Path(__file__).with_name('matter_contract.json')).read_text())
with tempfile.TemporaryDirectory(prefix='matterdraft-parity-') as tmp:
    app = create_app(pathlib.Path(tmp) / 'python', token='synthetic', require_vault=False)
    rust = subprocess.Popen([str(repo / 'rust-backend/target/debug/examples/contract'), str(pathlib.Path(tmp) / 'rust')], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    ids = {'python': {}, 'rust': {}}
    def normalize(v):
        if isinstance(v, dict):
            if 'created_at' in v and 'id' in v:
                assert re.fullmatch('[0-9a-f]{32}', v['id'])
                for field in ('created_at', 'updated_at'):
                    assert re.fullmatch(r'.*\.\d{6}\+00:00', v[field])
                    assert datetime.fromisoformat(v[field]).utcoffset().total_seconds() == 0
            return {k: ('<timestamp>' if x is not None else None) if k == 'archived_at' else normalize(x) for k, x in v.items() if k not in ('id', 'created_at', 'updated_at')}
        if isinstance(v, list): return [normalize(x) for x in v]
        return v
    previous_times = {'python': {}, 'rust': {}}
    def snapshot(engine):
        dbpath = pathlib.Path(tmp) / ('python/court_bundle.sqlite3' if engine == 'python' else 'rust/rust-matters.sqlite3')
        with sqlite3.connect(dbpath) as db:
            result = {}
            for alias, matter_id in ids[engine].items():
                if engine == 'rust':
                    item = json.loads(db.execute('SELECT body FROM matters WHERE id=?', [matter_id]).fetchone()[0])
                else:
                    db.row_factory = sqlite3.Row
                    item = dict(db.execute('SELECT * FROM matters WHERE id=?', [matter_id]).fetchone())
                times = {k: item[k] for k in ('created_at','updated_at','archived_at')}
                for key, value in times.items():
                    if value is not None:
                        assert re.fullmatch(r'.*\.\d{6}\+00:00', value)
                        datetime.fromisoformat(value)
                old = previous_times[engine].get(alias)
                changed = {k: old is None or v != old[k] for k,v in times.items()}
                if old:
                    assert times['created_at'] == old['created_at']
                    assert times['updated_at'] >= old['updated_at']
                assert times['updated_at'] >= times['created_at']
                if times['archived_at']: assert times['updated_at'] >= times['archived_at']
                previous_times[engine][alias] = times
                audit = [tuple(row) for row in db.execute('SELECT action FROM audit_events WHERE matter_id=? ORDER BY id',[matter_id])]
                history = [[row[0],json.loads(row[1]),json.loads(row[2]),row[3]] for row in db.execute('SELECT kind,before_json,after_json,applied FROM edit_history WHERE matter_id=? ORDER BY id',[matter_id])]
                result[alias] = (changed,audit,history)
            return result
    try:
        with TestClient(app) as client:
            for case in fixtures:
                paths = {engine: case['path'].format(**ids[engine]) for engine in ids}
                response = client.request(case['method'], paths['python'], json=case.get('body'), headers={'Authorization': 'Bearer synthetic'})
                rust.stdin.write(json.dumps({**case, 'path': paths['rust']}) + '\n'); rust.stdin.flush()
                actual = json.loads(rust.stdout.readline())
                assert actual['status'] == response.status_code, (case, actual, response.text)
                assert normalize(actual['body']) == normalize(response.json()), (case, actual, response.text)
                if 'save' in case:
                    for engine, value in [('python', response.json()), ('rust', actual['body'])]:
                        assert re.fullmatch('[0-9a-f]{32}', value['id'])
                        ids[engine][case['save']] = value['id']
                        previous_times[engine].pop(case['save'], None)
                assert snapshot('python') == snapshot('rust'), ('durable state/timestamp divergence', case)
                print('PASS', case['method'], case['path'], actual['status'])
            # Compare durable side effects, preserving field values and history retention.
            pydb = sqlite3.connect(pathlib.Path(tmp) / 'python/court_bundle.sqlite3')
            rsdb = sqlite3.connect(pathlib.Path(tmp) / 'rust/rust-matters.sqlite3')
            for alias in ids['python']:
                for query in [
                    'SELECT action FROM audit_events WHERE matter_id=? ORDER BY id',
                    'SELECT kind,before_json,after_json,applied FROM edit_history WHERE matter_id=? ORDER BY id',
                ]:
                    expected = pydb.execute(query, [ids['python'][alias]]).fetchall()
                    actual = rsdb.execute(query, [ids['rust'][alias]]).fetchall()
                    def canonical(rows):
                        return [[json.loads(x) if isinstance(x, str) and x.startswith('{') else x for x in row] for row in rows]
                    assert canonical(expected) == canonical(actual), (query, expected, actual)
            pydb.close(); rsdb.close()
    finally:
        rust.stdin.close(); rust.wait(timeout=10)
print(f'{len(fixtures)} live contract cases passed')
