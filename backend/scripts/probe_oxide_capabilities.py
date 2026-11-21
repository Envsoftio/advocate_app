"""Synthetic-only Oxide writer probe. PyMuPDF is the independent fixture/inspector.
Run with backend/.venv312/bin/python; no production routes are switched.
"""
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import pymupdf

ROOT = Path(__file__).resolve().parents[2]
worker = ROOT / 'backend/pdf-oxide-worker/target/debug/examples/capability_probe'
out = Path(tempfile.mkdtemp(prefix='matterdraft-oxide-probe-'))
source = out / 'source.pdf'
with pymupdf.open() as pdf:
    for index in range(3):
        page = pdf.new_page()
        page.insert_text((72, 100), f'SECRET{index} KEEP{index}', fontsize=20)
    pdf[0].insert_link({'kind': pymupdf.LINK_GOTO, 'from': pymupdf.Rect(70, 80, 250, 110), 'page': 2})
    pdf.set_toc([[1, 'First', 1], [1, 'Third', 3]])
    pdf.set_page_labels([{'startpage': 0, 'prefix': 'B-', 'style': 'D', 'firstpagenum': 1}])
    pdf.set_metadata({'title': 'Synthetic probe'})
    pdf.save(source)
original = hashlib.sha256(source.read_bytes()).hexdigest()
results = {}
for operation in ('rotate', 'reverse', 'merge', 'redact'):
    target = out / f'{operation}.pdf'
    run = subprocess.run([str(worker), operation, str(source), str(target)], capture_output=True, text=True, timeout=60)
    result = {'exit': run.returncode, 'diagnostic': run.stderr.strip()}
    if run.returncode == 0:
        with pymupdf.open(target) as pdf:
            texts = [p.get_text().strip() for p in pdf]
            result.update(pages=len(pdf), texts=texts, toc=pdf.get_toc(), labels=[p.get_label() for p in pdf],
                          goto=[[link.get('page') for link in p.get_links() if link['kind']==pymupdf.LINK_GOTO] for p in pdf],
                          rotations=[p.rotation for p in pdf])
            if operation == 'rotate':
                result['checks'] = {'rotation': pdf[0].rotation == 90, 'text': 'SECRET0' in texts[0],
                    'outline': pdf.get_toc()==[[1,'First',1],[1,'Third',3]],
                    'link': result['goto'][0]==[2], 'labels': result['labels']==['B-1','B-2','B-3']}
            elif operation == 'reverse':
                result['checks'] = {'order': all(f'SECRET{2-i}' in t for i,t in enumerate(texts)),
                    'outline_remap': pdf.get_toc()==[[1,'First',3],[1,'Third',1]],
                    'link_remap': result['goto'][2]==[0]}
            elif operation == 'merge':
                result['checks'] = {'count': len(pdf)==6, 'text': all(f'SECRET{i%3}' in t for i,t in enumerate(texts)),
                    'appended_link': result['goto'][3]==[5]}
            else:
                # Full-page simple-font redaction: no broad security claim.
                pix = pdf[0].get_pixmap(alpha=False)
                result['checks'] = {'removed_text': 'SECRET0' not in ''.join(texts),
                    'other_page_preserved': 'SECRET1' in texts[1],
                    'removed_stream': all(b'SECRET0' not in (pdf.xref_stream(x) or b'') for x in range(1,pdf.xref_length()) if pdf.xref_is_stream(x)),
                    'black_pixels': max(pix.samples)==0}
            pdf[0].get_pixmap(matrix=pymupdf.Matrix(.6,.6), alpha=False).save(out / f'{operation}.png')
    result['original_unchanged'] = hashlib.sha256(source.read_bytes()).hexdigest()==original
    results[operation] = result
(out / 'results.json').write_text(json.dumps(results, indent=2))
print(json.dumps({'artifacts':str(out), 'results':results},indent=2))
