"""Build a representative sample filing package from synthetic sample files.
Not a court-approved filing or legal template.
"""
from pathlib import Path
import shutil
import sys
import tempfile
import time
from fastapi.testclient import TestClient
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from app.main import create_app

ROOT = Path(__file__).resolve().parents[2]
SAMPLES = ROOT / 'samples'
OUT = SAMPLES / 'example_output'
TOKEN = 'temporary-demo-output-token'


def wait(client, job_id, timeout=120):
    start = time.monotonic()
    while time.monotonic() - start < timeout:
        job = client.get('/jobs/' + job_id, headers=HEADERS).json()
        if job['status'] == 'completed':
            return job['result']
        if job['status'] in ('failed','cancelled'):
            raise RuntimeError(f'Job {job_id} failed: {job["error"]}')
        time.sleep(.15)
    raise TimeoutError('Waiting for sample export')

HEADERS = {'Authorization': 'Bearer ' + TOKEN}


def main():
    from generate_demo_docs import main as generate
    generate()
    with tempfile.TemporaryDirectory(prefix='courtbundle-sample-') as temp:
        with TestClient(create_app(data_dir=temp, token=TOKEN)) as client:
            matter = client.post('/matters', headers=HEADERS, json={
                'name':'Fictional Filing - Demonstration Only',
                'court_profile':'delhi-hc-illustrative',
                'case_type':'Synthetic court file - NOT A REAL PETITION',
                'petitioner':'Example Applicant', 'respondent':'Example Respondent',
                'max_pages_per_volume':6, 'keep_groups_intact':True,
            }).json()
            files = ['01_petition.pdf','02_impugned_order.pdf','03_affidavit.pdf','04_scanned_annexure.jpg']
            uploaded = []
            for name in files:
                with (SAMPLES / name).open('rb') as stream:
                    r = client.post(f'/matters/{matter["id"]}/documents', headers=HEADERS,
                        files=[('files',(name,stream,'application/octet-stream'))])
                if r.status_code != 201:
                    raise RuntimeError(r.text)
                uploaded.append(r.json()[0])
            tags=[('petition','',1,''),('order','',1,''),('affidavit','',2,''),('annexure','P-1',2,'')]
            for doc,(kind,label,part,group) in zip(uploaded,tags):
                assert client.patch(f'/documents/{doc["id"]}',headers=HEADERS,json={'kind':kind,'label':label,'part':part,'group_key':group}).status_code==200
            job=client.post(f'/documents/{uploaded[-1]["id"]}/ocr', headers=HEADERS).json()
            print('OCR:', wait(client,job['id']))
            job=client.post(f'/matters/{matter["id"]}/build',headers=HEADERS).json()
            result=wait(client,job['id'])
            print('Export:',result['volumes'],'volumes',result['pages'],'pages')
            src=Path(temp)/'matters'/matter['id']/'exports'/result['id']
            OUT.mkdir(parents=True,exist_ok=True)
            for name in ['filing-package.zip','combined.pdf','validation-report.json']:
                shutil.copy2(src/name,OUT/name)
            print('Sample export at',OUT)

if __name__ == '__main__':
    main()
