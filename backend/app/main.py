"""Local FastAPI service for the desktop POC. Always bind to loopback."""
from __future__ import annotations
import asyncio
import errno
from contextlib import asynccontextmanager
import json
import os
from pathlib import Path
import secrets
import shutil
import tempfile

from fastapi import FastAPI, Depends, HTTPException, UploadFile, File, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse, Response, JSONResponse
from starlette.background import BackgroundTask
import pymupdf

from .engine import load_profiles, preflight, build_bundle
from .jobs import JobManager
from .pdf_ops import (ProcessingError, normalize_to_pdf, reorder_pages,
                      rotate_page, delete_page, insert_pages, replace_pages, replace_document, crop_pages,
                      extract_pages, split_pages, add_text_overlay, editable_text_runs,
                      replace_text_run, compress_pdf,
                      ocr_pdf, make_cover_pdf)
from .schemas import MatterCreate, MatterPatch, DocumentPatch, MemoPartiesUpdate, ReorderDocuments, ReorderPages, RotatePage, DeletePage, InsertPages, ReplacePages, ReplaceDocument, ManualBookmarksUpdate, CropPages, TextOverlay, ReplaceTextRun
from .storage import Store, new_id
from .schemas import CompressionRequest
from .portable import create_package, restore_package, PortableMatterError, MAX_PACKAGE_BYTES

FILE_LIMIT_BYTES = 80 * 1024 * 1024
SUPPORTED = {'.pdf', '.png', '.jpg', '.jpeg'}


def create_app(data_dir: str | Path | None = None, token: str | None = None) -> FastAPI:
    token = token if token is not None else os.getenv('COURTBUNDLE_API_TOKEN')
    if not token:
        raise RuntimeError('COURTBUNDLE_API_TOKEN is required. Set a session secret or use backend/dev.sh.')
    profiles = load_profiles()
    root = Path(data_dir or os.getenv('COURTBUNDLE_DATA_DIR') or Path.home() / '.courtbundle-poc')
    store = Store(root)
    try:
        jobs = JobManager(store)
    except BaseException:
        store.close()
        raise
    @asynccontextmanager
    async def lifespan(_app: FastAPI):
        try:
            yield
        finally:
            jobs.shutdown()
            store.close()

    app = FastAPI(title='CourtBundle Local API', version='0.1.0', docs_url=None,
                  redoc_url=None, openapi_url=None, lifespan=lifespan)
    app.state.store = store
    app.state.jobs = jobs

    @app.exception_handler(OSError)
    async def storage_error(_request, exc):
        if exc.errno in (errno.ENOSPC, errno.EDQUOT):
            return JSONResponse(status_code=507, content={'detail': 'Not enough disk space. Free space and retry; the previous working revision is retained.'})
        if exc.errno in (errno.EACCES, errno.EPERM):
            return JSONResponse(status_code=403, content={'detail': 'The local data folder is not writable or readable. Check its permissions and retry.'})
        return JSONResponse(status_code=409, content={'detail': 'Local file access failed. Check available storage and file access, then reload before retrying.'})
    allowed_origins = ['http://localhost:1420', 'http://127.0.0.1:1420',
                       'http://localhost:5173', 'http://127.0.0.1:5173',
                       'tauri://localhost', 'http://tauri.localhost', 'https://tauri.localhost']
    if os.environ.get('COURTBUNDLE_E2E_VITE_PORT'):
        port = int(os.environ['COURTBUNDLE_E2E_VITE_PORT'])
        allowed_origins.append(f'http://127.0.0.1:{port}')
    app.add_middleware(CORSMiddleware,
                       allow_origins=allowed_origins,
                       allow_methods=['GET','POST','PUT','PATCH','DELETE'],
                       allow_headers=['Authorization','Content-Type'])

    def auth(request: Request):
        supplied = request.headers.get('Authorization', '')
        if not secrets.compare_digest(supplied, 'Bearer ' + token):
            raise HTTPException(status_code=401, detail='Missing or invalid local session token')

    def matter_or_404(matter_id: str):
        item = store.get_matter(matter_id)
        if not item:
            raise HTTPException(status_code=404, detail='Matter not found')
        return item

    def doc_or_404(doc_id: str):
        item = store.get_document(doc_id)
        if not item or not store.get_matter(item['matter_id']):
            raise HTTPException(status_code=404, detail='Document not found')
        return item

    @app.get('/health')
    def health(_: None = Depends(auth)):
        return {'status': 'ok', 'version': '0.1.0', 'ocr_engine': 'PyMuPDF+Tesseract',
                'app': 'CourtBundle POC'}

    @app.get('/profiles')
    def available_profiles(_: None = Depends(auth)):
        return list(profiles.values())

    @app.get('/matters')
    def matters(_: None = Depends(auth)):
        return store.list_matters()

    @app.get('/matters/archived')
    def archived_matters(_: None = Depends(auth)):
        return store.list_archived_matters()

    @app.post('/matters', status_code=201)
    def new_matter(payload: MatterCreate, _: None = Depends(auth)):
        if payload.court_profile not in profiles:
            raise HTTPException(status_code=422, detail='Unknown court profile')
        return store.create_matter(payload.model_dump())

    @app.get('/matters/{matter_id}')
    def get_matter(matter_id: str, _: None = Depends(auth)):
        return matter_or_404(matter_id)

    @app.patch('/matters/{matter_id}')
    def patch_matter(matter_id: str, payload: MatterPatch, _: None = Depends(auth)):
        matter_or_404(matter_id)
        changes = payload.model_dump(exclude_unset=True)
        if 'court_profile' in changes and changes['court_profile'] not in profiles:
            raise HTTPException(status_code=422, detail='Unknown court profile')
        if any(v is None for v in changes.values()):
            raise HTTPException(status_code=422, detail='Do not set matter fields to null')
        with store.matter_lock(matter_id):
            return store.update_matter(matter_id, changes)

    @app.delete('/matters/{matter_id}')
    def delete_matter(matter_id: str, _: None = Depends(auth)):
        matter_or_404(matter_id)
        with store.matter_lock(matter_id):
            store.delete_matter(matter_id)
        return {'status': 'archived', 'note': 'Matter, records and original files retained locally'}

    @app.post('/matters/{matter_id}/restore')
    def restore_archived_matter(matter_id: str, _: None = Depends(auth)):
        with store.matter_lock(matter_id):
            restored = store.restore_matter(matter_id)
        if not restored:
            raise HTTPException(status_code=404, detail='Archived matter not found')
        return restored

    @app.get('/matters/{matter_id}/storage')
    def matter_storage(matter_id: str, _: None = Depends(auth)):
        matter_or_404(matter_id)
        return store.storage_usage(matter_id)

    @app.get('/matters/{matter_id}/audit')
    def matter_audit(matter_id: str, _: None = Depends(auth)):
        matter_or_404(matter_id)
        return store.list_audit(matter_id)

    @app.get('/matters/{matter_id}/archived-documents')
    def archived_documents(matter_id: str, _: None = Depends(auth)):
        matter_or_404(matter_id)
        return store.list_archived_documents(matter_id)

    @app.post('/matters/{matter_id}/archived-documents/{doc_id}/restore')
    def restore_archived_document(matter_id: str, doc_id: str, _: None = Depends(auth)):
        matter_or_404(matter_id)
        with store.matter_lock(matter_id):
            restored = store.restore_document(matter_id, doc_id)
        if not restored:
            raise HTTPException(status_code=404, detail='Archived document not found')
        return restored

    @app.get('/matters/{matter_id}/portable')
    def backup_matter(matter_id: str, _: None = Depends(auth)):
        matter_or_404(matter_id)
        with tempfile.NamedTemporaryFile(dir=store.root, prefix='.portable-',
                                         suffix='.cbmatter', delete=False) as temp:
            path = Path(temp.name)
        try:
            with store.matter_lock(matter_id):
                create_package(store, matter_id, path)
        except BaseException as exc:
            path.unlink(missing_ok=True)
            if isinstance(exc, PortableMatterError):
                raise HTTPException(status_code=409, detail=str(exc)) from exc
            raise
        return FileResponse(path, media_type='application/zip',
                            filename='court-matter.cbmatter',
                            background=BackgroundTask(path.unlink, missing_ok=True))

    @app.post('/matters/restore', status_code=201)
    async def restore_matter(file: UploadFile = File(...), _: None = Depends(auth)):
        with tempfile.NamedTemporaryFile(dir=store.root, prefix='.portable-',
                                         suffix='.cbmatter', delete=False) as temp:
            path = Path(temp.name)
        try:
            size = 0
            with path.open('wb') as output:
                while chunk := await file.read(1024 * 1024):
                    size += len(chunk)
                    if size > MAX_PACKAGE_BYTES:
                        raise HTTPException(status_code=413, detail='Portable package exceeds 4 GiB')
                    output.write(chunk)
            try:
                return restore_package(store, path)
            except PortableMatterError as exc:
                raise HTTPException(status_code=422, detail=str(exc)) from exc
        finally:
            path.unlink(missing_ok=True)
            await file.close()

    @app.get('/matters/{matter_id}/documents')
    def get_documents(matter_id: str, _: None = Depends(auth)):
        matter_or_404(matter_id)
        return store.get_documents(matter_id)

    @app.get('/matters/{matter_id}/search')
    def search_matter(matter_id: str, q: str, _: None = Depends(auth)):
        matter_or_404(matter_id)
        query = q.strip()
        if not 2 <= len(query) <= 120:
            raise HTTPException(status_code=422, detail='Search for 2 to 120 characters')
        matches = []
        with store.matter_lock(matter_id):
            for document in store.get_documents(matter_id):
                try:
                    with pymupdf.open(store.working_path(document)) as pdf:
                        for page_index, page in enumerate(pdf):
                            text = ' '.join(page.get_text('text').split())
                            position = text.casefold().find(query.casefold())
                            if position < 0:
                                continue
                            start = max(0, position - 70)
                            end = min(len(text), position + len(query) + 100)
                            matches.append({
                                'document_id': document['id'], 'title': document['title'],
                                'revision': document['revision'], 'page': page_index + 1,
                                'snippet': ('…' if start else '') + text[start:end] + ('…' if end < len(text) else ''),
                            })
                            if len(matches) >= 100:
                                return {'matches': matches, 'truncated': True}
                except Exception as exc:
                    raise HTTPException(status_code=409, detail=f'Cannot search working copy of {document["title"]}: {exc}') from exc
        return {'matches': matches, 'truncated': False}

    @app.get('/matters/{matter_id}/memo-parties')
    def get_memo_parties(matter_id: str, _: None = Depends(auth)):
        matter_or_404(matter_id)
        return {'parties': store.get_memo_parties(matter_id)}

    @app.put('/matters/{matter_id}/memo-parties')
    def put_memo_parties(matter_id: str, payload: MemoPartiesUpdate, _: None = Depends(auth)):
        matter_or_404(matter_id)
        with store.matter_lock(matter_id):
            return {'parties': store.set_memo_parties(
                matter_id, [party.model_dump() for party in payload.parties])}

    async def import_document(matter_id: str, file: UploadFile):
        suffix = Path(file.filename or '').suffix.lower()
        if suffix not in SUPPORTED:
            raise HTTPException(status_code=415, detail='Only PDF, PNG and JPEG documents are supported in this POC')
        doc_id = new_id()
        folder = store.matter_dir(matter_id)
        originals = folder / 'originals'
        working = folder / 'working'
        originals.mkdir(parents=True, exist_ok=True)
        working.mkdir(parents=True, exist_ok=True)
        source_rel = f'originals/{doc_id}{suffix}'
        work_rel = f'working/{doc_id}.pdf'
        source = folder / source_rel
        target = folder / work_rel
        try:
            size = 0
            with source.open('wb') as stream:
                while chunk := await file.read(1024 * 1024):
                    size += len(chunk)
                    if size > FILE_LIMIT_BYTES:
                        raise HTTPException(status_code=413, detail='POC import limit is 80 MB per file')
                    stream.write(chunk)
            conversion = asyncio.create_task(asyncio.to_thread(normalize_to_pdf, source, target))
            try:
                pages = await asyncio.shield(conversion)
            except asyncio.CancelledError:
                # Finish native work before releasing the process lock or
                # removing files which the conversion thread still has open.
                try:
                    await conversion
                finally:
                    raise
            name = Path(file.filename or '').name or 'Untitled'
            item = dict(id=doc_id, matter_id=matter_id, title=Path(name).stem[:250],
                        kind='other', part=1, label='', group_key='', page_count=pages,
                        ocr_status='not_requested', original_name=name[:250],
                        source_sha256=store.sha256(source), source_relpath=source_rel,
                        working_relpath=work_rel)
            return store.add_document(item)
        except HTTPException:
            raise
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc
        finally:
            if not store.get_document(doc_id):
                source.unlink(missing_ok=True)
                target.unlink(missing_ok=True)
            await file.close()

    @app.post('/matters/{matter_id}/documents', status_code=201)
    async def upload(matter_id: str, files: list[UploadFile] = File(...), _: None = Depends(auth)):
        matter_or_404(matter_id)
        if not 1 <= len(files) <= 20:
            raise HTTPException(status_code=422, detail='Import 1-20 files per request')
        lock = store.matter_lock(matter_id)
        # A cancelled to_thread(lock.acquire) can acquire later with nobody left
        # to release it. Poll nonblocking so disconnects cannot strand the lock.
        while not lock.acquire(blocking=False):
            await asyncio.sleep(.05)
        try:
            records = []
            for f in files:
                records.append(await import_document(matter_id, f))
            return records
        finally:
            lock.release()

    @app.post('/matters/{matter_id}/cover', status_code=201)
    def generate_cover(matter_id: str, _: None = Depends(auth)):
        with store.matter_lock(matter_id):
            matter = matter_or_404(matter_id)
            doc_id = new_id()
            folder = store.matter_dir(matter_id)
            originals, working = folder / 'originals', folder / 'working'
            originals.mkdir(parents=True, exist_ok=True)
            working.mkdir(parents=True, exist_ok=True)
            original_rel = f'originals/{doc_id}.pdf'
            work_rel = f'working/{doc_id}.pdf'
            path = folder / original_rel
            make_cover_pdf(path, matter)
            shutil.copy2(path, folder / work_rel)
            return store.add_document(dict(id=doc_id, matter_id=matter_id,
                                           title='Draft matter cover sheet', kind='other', part=1,
                                           label='', group_key='', page_count=1, ocr_status='native_text',
                                           original_name='generated-cover.pdf', source_sha256=store.sha256(path),
                                           source_relpath=original_rel, working_relpath=work_rel))

    @app.post('/matters/{matter_id}/documents/reorder')
    def order_docs(matter_id: str, payload: ReorderDocuments, _: None = Depends(auth)):
        matter_or_404(matter_id)
        try:
            with store.matter_lock(matter_id):
                return store.reorder(matter_id, payload.ids)
        except ValueError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.get('/matters/{matter_id}/history')
    def history_state(matter_id: str, _: None = Depends(auth)):
        matter_or_404(matter_id)
        return store.history_state(matter_id)

    @app.get('/matters/{matter_id}/bookmarks')
    def manual_bookmarks(matter_id: str, _: None = Depends(auth)):
        matter_or_404(matter_id)
        return store.list_manual_bookmarks(matter_id)

    @app.put('/matters/{matter_id}/bookmarks')
    def update_manual_bookmarks(matter_id: str, payload: ManualBookmarksUpdate,
                                _: None = Depends(auth)):
        matter_or_404(matter_id)
        try:
            with store.matter_lock(matter_id):
                return store.set_manual_bookmarks(matter_id,
                    [entry.model_dump() for entry in payload.entries])
        except ValueError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.post('/matters/{matter_id}/history/{direction}')
    def traverse_history(matter_id: str, direction: str, _: None = Depends(auth)):
        matter_or_404(matter_id)
        if direction not in ('undo', 'redo'):
            raise HTTPException(status_code=404, detail='Unknown history operation')
        try:
            with store.matter_lock(matter_id):
                return store.traverse_history(matter_id, redo=direction == 'redo')
        except ValueError as exc:
            raise HTTPException(status_code=409, detail=str(exc)) from exc

    @app.patch('/documents/{doc_id}')
    def patch_doc(doc_id: str, payload: DocumentPatch, _: None = Depends(auth)):
        doc = doc_or_404(doc_id)
        values = payload.model_dump(exclude_unset=True)
        if any(v is None for v in values.values()):
            raise HTTPException(status_code=422, detail='Do not clear document fields with null; use empty string instead')
        with store.matter_lock(doc['matter_id']):
            return store.update_document(doc_id, values)

    @app.delete('/documents/{doc_id}')
    def remove_doc(doc_id: str, _: None = Depends(auth)):
        doc = doc_or_404(doc_id)
        with store.matter_lock(doc['matter_id']):
            store.delete_document(doc_id)
        # Intentionally preserve originals and working copies for safe recovery.
        return {'status': 'archived', 'files_retained': True}

    @app.get('/documents/{doc_id}/pdf')
    def document_pdf(doc_id: str, _: None = Depends(auth)):
        doc = doc_or_404(doc_id)
        path = store.working_path(doc)
        if not path.is_file():
            raise HTTPException(status_code=404, detail='PDF working copy missing')
        return FileResponse(path, media_type='application/pdf', filename=doc['id'] + '.pdf')

    def edit_working_copy(doc_id: str, operation):
        doc = doc_or_404(doc_id)
        with store.matter_lock(doc['matter_id']):
            doc = doc_or_404(doc_id)
            rel, candidate = store.next_working_path(doc)
            try:
                shutil.copy2(store.working_path(doc), candidate)
                changes = operation(candidate, doc) or {}
                return store.update_document(doc_id, dict(changes, working_relpath=rel))
            except BaseException:
                store.discard_uncommitted_working(doc_id, rel, candidate)
                raise

    @app.post('/documents/{doc_id}/pages/reorder')
    def page_reorder(doc_id: str, payload: ReorderPages, _: None = Depends(auth)):
        try:
            return edit_working_copy(doc_id, lambda path, doc: {
                'page_count': reorder_pages(path, payload.order), 'ocr_status': 'modified'})
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.post('/documents/{doc_id}/pages/rotate')
    def page_rotate(doc_id: str, payload: RotatePage, _: None = Depends(auth)):
        try:
            def rotate(path, doc):
                rotate_page(path, payload.page_index, payload.degrees)
                return {'page_count': doc['page_count']}
            return edit_working_copy(doc_id, rotate)
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.post('/documents/{doc_id}/pages/delete')
    def page_delete(doc_id: str, payload: DeletePage, _: None = Depends(auth)):
        try:
            return edit_working_copy(doc_id, lambda path, doc: {
                'page_count': delete_page(path, payload.page_index)})
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    def source_for_edit(doc: dict, source_id: str):
        source = doc_or_404(source_id)
        if source['matter_id'] != doc['matter_id']:
            raise HTTPException(status_code=422, detail='Source must be in the same matter')
        return store.working_path(source)

    @app.post('/documents/{doc_id}/pages/insert')
    def page_insert(doc_id: str, payload: InsertPages, _: None = Depends(auth)):
        try:
            return edit_working_copy(doc_id, lambda path, doc: {
                'page_count': insert_pages(path, source_for_edit(doc, payload.source_document_id),
                                           payload.source_pages, payload.before_index),
                'ocr_status': 'modified'})
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.post('/documents/{doc_id}/pages/replace')
    def page_replace(doc_id: str, payload: ReplacePages, _: None = Depends(auth)):
        try:
            return edit_working_copy(doc_id, lambda path, doc: {
                'page_count': replace_pages(path, source_for_edit(doc, payload.source_document_id),
                                            payload.target_pages, payload.source_pages),
                'ocr_status': 'modified'})
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.post('/documents/{doc_id}/replace')
    def document_replace(doc_id: str, payload: ReplaceDocument, _: None = Depends(auth)):
        try:
            def replace(path, doc):
                if payload.source_document_id == doc_id:
                    raise ProcessingError('Choose a different document as the replacement source')
                return {'page_count': replace_document(
                    path, source_for_edit(doc, payload.source_document_id)),
                    'ocr_status': 'modified'}
            return edit_working_copy(doc_id, replace)
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.post('/documents/{doc_id}/pages/crop')
    def page_crop(doc_id: str, payload: CropPages, _: None = Depends(auth)):
        try:
            return edit_working_copy(doc_id, lambda path, doc: {
                'page_count': crop_pages(path, payload.pages, payload.left, payload.top,
                                         payload.right, payload.bottom),
                'ocr_status': 'modified'})
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.post('/documents/{doc_id}/pages/text-overlay')
    def page_text_overlay(doc_id: str, payload: TextOverlay, _: None = Depends(auth)):
        try:
            def place(path, doc):
                add_text_overlay(path, payload.page_index, payload.x, payload.y,
                                 payload.text, payload.font_size)
                return {'page_count': doc['page_count'], 'ocr_status': 'modified'}
            return edit_working_copy(doc_id, place)
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.get('/documents/{doc_id}/pages/{page_index}/text-runs')
    def page_text_runs(doc_id: str, page_index: int, _: None = Depends(auth)):
        doc = doc_or_404(doc_id)
        try:
            with store.matter_lock(doc['matter_id']):
                current = doc_or_404(doc_id)
                return {'revision': current['revision'],
                        'runs': editable_text_runs(store.working_path(current), page_index)}
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.post('/documents/{doc_id}/pages/replace-text-run')
    def page_replace_text_run(doc_id: str, payload: ReplaceTextRun, _: None = Depends(auth)):
        try:
            edit_layout = {}
            def replace(path, doc):
                if doc['revision'] != payload.expected_revision:
                    raise HTTPException(status_code=409, detail='Document changed; reload and select text again')
                edit_layout.update(replace_text_run(path, payload.page_index, payload.span_index,
                                                    payload.expected_text, payload.replacement))
                return {'page_count': doc['page_count'], 'ocr_status': 'modified'}
            return dict(edit_working_copy(doc_id, replace), edit_layout=edit_layout)
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    def page_selection(value: str) -> list[int]:
        if not value or len(value) > 10000 or any(piece == '' or not piece.isdecimal()
                                                   for piece in value.split(',')):
            raise HTTPException(status_code=422, detail='Use comma-separated zero-based page indexes')
        return [int(piece) for piece in value.split(',')]

    @app.get('/documents/{doc_id}/pages/extract')
    def page_extract(doc_id: str, selection: str, _: None = Depends(auth)):
        doc = doc_or_404(doc_id)
        try:
            with store.matter_lock(doc['matter_id']):
                current = doc_or_404(doc_id)
                data = extract_pages(store.working_path(current), page_selection(selection))
            return Response(data, media_type='application/pdf', headers={
                'Content-Disposition': 'attachment; filename="extracted-pages.pdf"'})
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.get('/documents/{doc_id}/pages/split')
    def page_split(doc_id: str, after: str, _: None = Depends(auth)):
        doc = doc_or_404(doc_id)
        try:
            with store.matter_lock(doc['matter_id']):
                current = doc_or_404(doc_id)
                data = split_pages(store.working_path(current), page_selection(after))
            return Response(data, media_type='application/zip', headers={
                'Content-Disposition': 'attachment; filename="split-pages.zip"'})
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    def submit_ocr(doc: dict, job_id: str | None = None):
        doc_id = doc['id']
        def task(job_id, progress, cancelled):
            with store.matter_lock(doc['matter_id']):
                current = store.get_document(doc_id)
                if current is None:
                    raise ProcessingError('Document was removed before OCR started')
                rel, candidate = store.next_working_path(current)
                committed = False
                try:
                    shutil.copy2(store.working_path(current), candidate)
                    total, changed = ocr_pdf(candidate, progress=progress, cancelled=cancelled)
                    if cancelled():
                        raise ProcessingError('Operation cancelled')
                    if changed:
                        store.update_document(doc_id, {'working_relpath': rel,
                                                       'ocr_status': 'processed', 'page_count': total})
                        committed = True
                    else:
                        store.update_document(doc_id, {'ocr_status': 'no_new_text', 'page_count': total})
                finally:
                    if not committed:
                        store.discard_uncommitted_working(doc_id, rel, candidate)
                return {'document_id': doc_id, 'pages_checked': total, 'pages_ocr_applied': changed}
        return jobs.submit(doc['matter_id'], 'ocr', task, target_id=doc_id, job_id=job_id)

    @app.post('/documents/{doc_id}/ocr', status_code=202)
    def start_ocr(doc_id: str, _: None = Depends(auth)):
        return submit_ocr(doc_or_404(doc_id))

    @app.post('/documents/{doc_id}/compress', status_code=202)
    def start_compression(doc_id: str, payload: CompressionRequest, _: None = Depends(auth)):
        doc = doc_or_404(doc_id)
        def task(job_id, progress, cancelled):
            with store.matter_lock(doc['matter_id']):
                current = doc_or_404(doc_id)
                if current['revision'] != payload.expected_revision:
                    raise ProcessingError('Document changed before compression; reload and try again')
                rel, candidate = store.next_working_path(current)
                committed = False
                try:
                    shutil.copy2(store.working_path(current), candidate)
                    result = compress_pdf(candidate, payload.preset, progress, cancelled)
                    if cancelled():
                        raise ProcessingError('Operation cancelled')
                    if result['changed']:
                        store.update_document(doc_id, {'working_relpath': rel})
                        committed = True
                    return dict(result, document_id=doc_id)
                finally:
                    if not committed:
                        store.discard_uncommitted_working(doc_id, rel, candidate)
        return jobs.submit(doc['matter_id'], 'compress', task, target_id=doc_id)

    @app.get('/matters/{matter_id}/validate')
    def validate_matter(matter_id: str, _: None = Depends(auth)):
        with store.matter_lock(matter_id):
            matter = matter_or_404(matter_id)
            return preflight(store, matter_id, profiles[matter['court_profile']])

    def submit_build(matter_id: str, job_id: str | None = None):
        def task(job_id, progress, cancelled):
            with store.matter_lock(matter_id):
                return build_bundle(store, matter_id, job_id, progress, cancelled)
        return jobs.submit(matter_id, 'build', task, job_id=job_id)

    @app.post('/matters/{matter_id}/build', status_code=202)
    def start_build(matter_id: str, _: None = Depends(auth)):
        matter_or_404(matter_id)
        return submit_build(matter_id)

    @app.get('/matters/{matter_id}/jobs')
    def matter_jobs(matter_id: str, _: None = Depends(auth)):
        matter_or_404(matter_id)
        return store.list_jobs(matter_id)

    @app.get('/jobs/{job_id}')
    def job_status(job_id: str, _: None = Depends(auth)):
        job = jobs.get(job_id)
        if not job:
            raise HTTPException(status_code=404, detail='Job not found or lost after backend restart')
        return job

    @app.post('/jobs/{job_id}/cancel')
    def cancel_job(job_id: str, _: None = Depends(auth)):
        job = jobs.cancel(job_id)
        if not job:
            raise HTTPException(status_code=404, detail='Job not found')
        return {'cancel_requested': True}

    @app.post('/jobs/{job_id}/retry', status_code=202)
    def retry_job(job_id: str, _: None = Depends(auth)):
        prior = jobs.get(job_id)
        if not prior:
            raise HTTPException(status_code=404, detail='Job not found')
        matter_or_404(prior['matter_id'])
        if prior['kind'] == 'ocr':
            doc = doc_or_404(prior['target_id'])
            if doc['matter_id'] != prior['matter_id']:
                raise HTTPException(status_code=409, detail='OCR document no longer belongs to this matter')
        elif prior['kind'] != 'build':
            raise HTTPException(status_code=409, detail='This job type cannot be retried')
        new_job_id = new_id()
        if not jobs.claim_retry(job_id, new_job_id):
            raise HTTPException(status_code=409, detail='Only an interrupted job may be retried once')
        return submit_ocr(doc, new_job_id) if prior['kind'] == 'ocr' else submit_build(prior['matter_id'], new_job_id)

    @app.get('/matters/{matter_id}/exports')
    def exports(matter_id: str, _: None = Depends(auth)):
        matter_or_404(matter_id)
        return store.list_exports(matter_id)

    @app.get('/matters/{matter_id}/exports/{export_id}/{filename}')
    def export_file(matter_id: str, export_id: str, filename: str, _: None = Depends(auth)):
        matter_or_404(matter_id)
        path = store.get_export_file(matter_id, export_id, filename)
        if not path:
            raise HTTPException(status_code=404, detail='Export file not found')
        media = 'application/pdf' if filename.endswith('.pdf') else ('application/zip' if filename.endswith('.zip') else 'application/octet-stream')
        return FileResponse(path, filename=filename, media_type=media)

    return app
