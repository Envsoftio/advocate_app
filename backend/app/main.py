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
from datetime import datetime
from urllib.parse import unquote

from fastapi import FastAPI, Depends, HTTPException, UploadFile, File, Form, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse, Response, JSONResponse, StreamingResponse
from starlette.background import BackgroundTask
from starlette.datastructures import UploadFile as ParsedUploadFile
import pymupdf

from .engine import load_profiles, preflight, build_bundle, bundle_preview
from .jobs import JobManager
from .pdf_ops import (ProcessingError, normalize_to_pdf, reorder_pages,
                      rotate_page, delete_page, insert_pages, replace_pages, replace_document, crop_pages,
                      extract_pages, split_pages, add_text_overlay, editable_text_runs,
                      replace_text_run, move_text_run, compress_pdf,
                      ocr_pdf, make_cover_pdf)
from .schemas import MatterCreate, MatterPatch, BundlePreviewRequest, DocumentPatch, BatchDocumentPatch, DocumentSortPreview, LabelProposalRequest, ApplyLabelProposal, MemoPartiesUpdate, MatterValuesUpdate, ReorderDocuments, ReorderPages, RotatePage, DeletePage, InsertPages, ReplacePages, ReplaceDocument, ManualBookmarksUpdate, CropPages, TextOverlay, ReplaceTextRun, MoveTextRun, SignatureRiskAcknowledgement
from .storage import Store, new_id
from .file_io import copy_file, open_pdf, protected, read_bytes, write_bytes
from .schemas import CompressionRequest
from .portable import (create_package, restore_package, PortableMatterError, MAX_PACKAGE_BYTES,
                       PROTECTED_PACKAGE_MAGIC, protected_package_header, parse_protected_package)
from .vault import VaultError, decrypt_blob, encrypted_sink, unlock_key
from .vault_store import VaultStore
from .labels import propose_labels, extract_label_references
from .signatures import (combined_signature_status, inspect_source_signature,
                         risk_summary)

FILE_LIMIT_BYTES = 80 * 1024 * 1024
SUPPORTED = {'.pdf', '.png', '.jpg', '.jpeg'}


def create_app(data_dir: str | Path | None = None, token: str | None = None,
               data_key: bytes | None = None, require_vault: bool | None = None) -> FastAPI:
    token = token if token is not None else os.getenv('COURTBUNDLE_API_TOKEN')
    if not token:
        raise RuntimeError('COURTBUNDLE_API_TOKEN is required. Set a session secret or use backend/dev.sh.')
    profiles = load_profiles()
    root = Path(data_dir or os.getenv('COURTBUNDLE_DATA_DIR') or Path.home() / '.courtbundle-poc')
    protected_mode = (os.getenv('COURTBUNDLE_REQUIRE_VAULT') == '1') if require_vault is None else require_vault
    vault = VaultStore(root) if protected_mode else None
    store = None if protected_mode and data_key is None else Store(root, data_key=data_key)
    try:
        jobs = JobManager(store) if store is not None else None
    except BaseException:
        if store is not None:
            store.close()
        raise
    @asynccontextmanager
    async def lifespan(_app: FastAPI):
        try:
            yield
        finally:
            if jobs is not None:
                jobs.shutdown()
            if store is not None:
                store.close()

    app = FastAPI(title='CourtBundle Local API', version='0.1.0', docs_url=None,
                  redoc_url=None, openapi_url=None, lifespan=lifespan)
    app.state.store = store
    app.state.jobs = jobs
    app.state.vault = vault

    @app.middleware('http')
    async def reject_unprotected_backup_upload(request: Request, call_next):
        if (protected_mode and request.url.path == '/matters/restore' and
                request.headers.get('content-type', '').split(';', 1)[0] != 'application/octet-stream'):
            return JSONResponse(status_code=415, content={
                'detail': 'Protected backup restore requires a streamed binary upload'})
        if (protected_mode and request.method == 'POST' and
                request.url.path.startswith('/matters/') and request.url.path.endswith('/documents')):
            return JSONResponse(status_code=415, content={
                'detail': 'Protected document import requires a streamed binary upload'})
        return await call_next(request)

    def activate(data_key: bytes) -> None:
        nonlocal store, jobs
        if store is not None:
            raise VaultError('Vault is already unlocked')
        opened = Store(root, data_key=data_key)
        try:
            worker = JobManager(opened)
        except BaseException:
            opened.close()
            raise
        store, jobs = opened, worker
        app.state.store, app.state.jobs = opened, worker

    def file_response(path: Path, media_type: str, filename: str,
                      background: BackgroundTask | None = None):
        if protected(path):
            response = Response(content=read_bytes(path), media_type=media_type,
                                headers={'Content-Disposition': f'attachment; filename="{filename}"'})
            response.background = background
            return response
        return FileResponse(path, media_type=media_type, filename=filename,
                            background=background)

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
                       allow_headers=['Authorization','Content-Type', 'X-CourtBundle-Filename',
                                      'X-CourtBundle-File-Mtime',
                                      'X-CourtBundle-Backup-Credential', 'X-CourtBundle-Backup-Recovery'])

    def auth(request: Request):
        supplied = request.headers.get('Authorization', '')
        if not secrets.compare_digest(supplied, 'Bearer ' + token):
            raise HTTPException(status_code=401, detail='Missing or invalid local session token')
        if store is None and request.url.path != '/health' and not request.url.path.startswith('/vault/'):
            raise HTTPException(status_code=423, detail='Matter store is locked. Unlock it first.')

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

    def require_signature_acknowledgement(documents: list[dict], acknowledged: bool,
                                          action: str) -> None:
        risks = risk_summary(documents)
        if not risks or acknowledged:
            return
        names = ', '.join(item['title'] for item in risks[:5])
        if len(risks) > 5:
            names += f' and {len(risks) - 5} more'
        raise HTTPException(status_code=409, detail=(
            f'Signature-risk acknowledgment required before {action}: {names}. '
            'Certificate indicators are not proof of validity, and transforming a working '
            'copy or generated bundle may invalidate or omit signature protections. '
            'Imported originals will remain unchanged.'))

    @app.get('/health')
    def health(_: None = Depends(auth)):
        return {'status': 'locked' if store is None else 'ok', 'version': '0.1.0', 'ocr_engine': 'PyMuPDF+Tesseract',
                'app': 'CourtBundle POC'}

    @app.get('/vault/status')
    def vault_status(_: None = Depends(auth)):
        return {'state': 'unlocked' if store is not None else vault.status(),
                'enabled': vault is not None}

    @app.post('/vault/setup')
    def vault_setup(payload: dict, _: None = Depends(auth)):
        if vault is None:
            raise HTTPException(status_code=409, detail='Vault setup is unavailable in development mode')
        try:
            data_key, recovery_key = vault.setup(payload.get('password'))
            activate(data_key)
            return {'state': 'unlocked', 'recovery_key': recovery_key}
        except VaultError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.post('/vault/unlock')
    def vault_unlock(payload: dict, _: None = Depends(auth)):
        if vault is None:
            raise HTTPException(status_code=409, detail='Vault unlock is unavailable in development mode')
        try:
            data_key = vault.unlock(payload.get('credential'), recovery=payload.get('recovery') is True)
            activate(data_key)
            return {'state': 'unlocked'}
        except VaultError as exc:
            raise HTTPException(status_code=403, detail=str(exc)) from exc

    @app.post('/vault/rotate')
    def vault_rotate(payload: dict, _: None = Depends(auth)):
        if vault is None or store is None:
            raise HTTPException(status_code=423, detail='Unlock the vault first')
        try:
            return {'recovery_key': vault.rotate(store._data_key, payload.get('new_password'))}
        except VaultError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

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
        try:
            with store.matter_lock(matter_id):
                restored = store.restore_document(matter_id, doc_id)
        except ValueError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc
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
        if vault is not None:
            try:
                header = protected_package_header(vault.envelope())
                def chunks():
                    yield header
                    with path.open('rb') as stream:
                        while block := stream.read(1024 * 1024):
                            yield block
                return StreamingResponse(chunks(), media_type='application/octet-stream',
                                headers={'Content-Disposition': 'attachment; filename="court-matter.cbmatter"'},
                                background=BackgroundTask(path.unlink, missing_ok=True))
            except BaseException:
                path.unlink(missing_ok=True)
                raise
        return file_response(path, 'application/zip', 'court-matter.cbmatter',
                             BackgroundTask(path.unlink, missing_ok=True))

    @app.post('/matters/restore', status_code=201)
    async def restore_matter(request: Request, _: None = Depends(auth)):
        with tempfile.NamedTemporaryFile(dir=store.root, prefix='.portable-',
                                         suffix='.cbmatter', delete=False) as temp:
            path = Path(temp.name)
        file = None
        try:
            size = 0
            if request.headers.get('content-type', '').split(';', 1)[0] == 'application/octet-stream':
                if vault is not None:
                    with encrypted_sink(path, store._data_key) as output:
                        async for chunk in request.stream():
                            size += len(chunk)
                            if size > MAX_PACKAGE_BYTES:
                                raise HTTPException(status_code=413, detail='Portable package exceeds 4 GiB')
                            output.write(chunk)
                else:
                    with path.open('wb') as output:
                        async for chunk in request.stream():
                            size += len(chunk)
                            if size > MAX_PACKAGE_BYTES:
                                raise HTTPException(status_code=413, detail='Portable package exceeds 4 GiB')
                            output.write(chunk)
                credential = request.headers.get('x-courtbundle-backup-credential')
                recovery = request.headers.get('x-courtbundle-backup-recovery') == 'true'
            else:
                if vault is not None:
                    raise HTTPException(status_code=415, detail='Protected backup restore requires a streamed binary upload')
                form = await request.form()
                file = form.get('file')
                if not isinstance(file, ParsedUploadFile):
                    raise HTTPException(status_code=422, detail='Select a portable matter file')
                chunks = []
                while chunk := await file.read(1024 * 1024):
                    size += len(chunk)
                    if size > MAX_PACKAGE_BYTES:
                        raise HTTPException(status_code=413, detail='Portable package exceeds 4 GiB')
                    chunks.append(chunk)
                write_bytes(path, b''.join(chunks))
                credential = form.get('credential')
                recovery = form.get('recovery') == 'true'
            try:
                raw = read_bytes(path)
                if raw.startswith(PROTECTED_PACKAGE_MAGIC):
                    if vault is None:
                        raise PortableMatterError('Unlock a protected store before restoring this backup')
                    envelope, ciphertext = parse_protected_package(raw)
                    try:
                        package_bytes = decrypt_blob(ciphertext, store._data_key)
                    except VaultError:
                        if not credential:
                            raise PortableMatterError('This backup needs its password or recovery key')
                        package_key = unlock_key(envelope, credential, recovery=recovery)
                        package_bytes = decrypt_blob(ciphertext, package_key)
                    return restore_package(store, package_bytes)
                return restore_package(store, path)
            except (PortableMatterError, VaultError) as exc:
                raise HTTPException(status_code=422, detail=str(exc)) from exc
        finally:
            path.unlink(missing_ok=True)
            if file is not None:
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
                    with open_pdf(store.working_path(document)) as pdf:
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

    @app.get('/matters/{matter_id}/values')
    def get_matter_values(matter_id: str, _: None = Depends(auth)):
        matter_or_404(matter_id)
        return {'values': store.get_matter_values(matter_id)}

    @app.put('/matters/{matter_id}/values')
    def put_matter_values(matter_id: str, payload: MatterValuesUpdate, _: None = Depends(auth)):
        matter_or_404(matter_id)
        with store.matter_lock(matter_id):
            return {'values': store.set_matter_values(matter_id, payload.values)}

    async def import_document(matter_id: str, filename: str,
                              file: UploadFile | None = None, request: Request | None = None,
                              source_file_mtime: str = ''):
        suffix = Path(filename).suffix.lower()
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
            if request is not None:
                if protected(source):
                    with encrypted_sink(source, store._data_key) as output:
                        async for chunk in request.stream():
                            size += len(chunk)
                            if size > FILE_LIMIT_BYTES:
                                raise HTTPException(status_code=413, detail='POC import limit is 80 MB per file')
                            output.write(chunk)
                else:
                    with source.open('wb') as output:
                        async for chunk in request.stream():
                            size += len(chunk)
                            if size > FILE_LIMIT_BYTES:
                                raise HTTPException(status_code=413, detail='POC import limit is 80 MB per file')
                            output.write(chunk)
            else:
                chunks = []
                while chunk := await file.read(1024 * 1024):
                    size += len(chunk)
                    if size > FILE_LIMIT_BYTES:
                        raise HTTPException(status_code=413, detail='POC import limit is 80 MB per file')
                    chunks.append(chunk)
                write_bytes(source, b''.join(chunks))
            signature = await asyncio.to_thread(inspect_source_signature, source, suffix)
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
            name = Path(filename).name or 'Untitled'
            item = dict(id=doc_id, matter_id=matter_id, title=Path(name).stem[:250],
                        kind='other', part=1, label='', label_status='unreviewed',
                        label_source='none', group_key='', group_role='member', page_count=pages,
                        document_date='', date_status='unknown', date_source='none', date_note='',
                        source_file_mtime=source_file_mtime,
                        ocr_status='not_requested', original_name=name[:250],
                        source_sha256=store.sha256(source), source_relpath=source_rel,
                        working_relpath=work_rel, **signature)
            return store.add_document(item)
        except HTTPException:
            raise
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc
        finally:
            if not store.get_document(doc_id):
                source.unlink(missing_ok=True)
                target.unlink(missing_ok=True)
            if file is not None:
                await file.close()

    @app.post('/matters/{matter_id}/documents/raw', status_code=201)
    async def upload_raw(matter_id: str, request: Request, _: None = Depends(auth)):
        matter_or_404(matter_id)
        if request.headers.get('content-type', '').split(';', 1)[0] != 'application/octet-stream':
            raise HTTPException(status_code=415, detail='Stream one PDF, PNG or JPEG file')
        filename = unquote(request.headers.get('x-courtbundle-filename', ''))
        if not filename or len(filename) > 500 or '/' in filename or '\\' in filename:
            raise HTTPException(status_code=422, detail='Invalid document filename')
        source_file_mtime = request.headers.get('x-courtbundle-file-mtime', '')
        if source_file_mtime:
            try:
                if len(source_file_mtime) > 40:
                    raise ValueError
                datetime.fromisoformat(source_file_mtime.replace('Z', '+00:00'))
            except ValueError as exc:
                raise HTTPException(status_code=422, detail='Invalid source file timestamp') from exc
        lock = store.matter_lock(matter_id)
        while not lock.acquire(blocking=False):
            await asyncio.sleep(.05)
        try:
            return [await import_document(matter_id, filename, request=request,
                                          source_file_mtime=source_file_mtime)]
        finally:
            lock.release()

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
                records.append(await import_document(matter_id, f.filename or '', file=f))
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
            copy_file(path, folder / work_rel)
            return store.add_document(dict(id=doc_id, matter_id=matter_id,
                                           title='Draft matter cover sheet', kind='other', part=1,
                                           label='', label_status='unreviewed', label_source='none',
                                           group_key='', group_role='member', page_count=1, ocr_status='native_text',
                                           document_date='', date_status='unknown', date_source='none', date_note='',
                                           source_file_mtime='',
                                           original_name='generated-cover.pdf', source_sha256=store.sha256(path),
                                           source_relpath=original_rel, working_relpath=work_rel,
                                           certificate_signature_status='not_detected',
                                           certificate_signature_count=0,
                                           signature_inspection_note='Generated locally without a PDF certificate signature.',
                                           visible_signature_status='absent',
                                           signature_status='unsigned'))

    @app.post('/matters/{matter_id}/documents/reorder')
    def order_docs(matter_id: str, payload: ReorderDocuments, _: None = Depends(auth)):
        matter_or_404(matter_id)
        try:
            with store.matter_lock(matter_id):
                return store.reorder(matter_id, payload.ids)
        except ValueError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.patch('/matters/{matter_id}/documents/batch')
    def batch_patch_docs(matter_id: str, payload: BatchDocumentPatch,
                         _: None = Depends(auth)):
        matter_or_404(matter_id)
        values = payload.model_dump(exclude={'ids'}, exclude_none=True)
        try:
            with store.matter_lock(matter_id):
                return store.batch_update_documents(matter_id, payload.ids, values)
        except ValueError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.post('/matters/{matter_id}/documents/sort-preview')
    def preview_doc_sort(matter_id: str, payload: DocumentSortPreview,
                         _: None = Depends(auth)):
        matter_or_404(matter_id)
        try:
            with store.matter_lock(matter_id):
                return store.preview_document_sort(matter_id, payload.mode)
        except ValueError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.post('/matters/{matter_id}/labels/preview')
    def preview_labels(matter_id: str, payload: LabelProposalRequest,
                       _: None = Depends(auth)):
        matter_or_404(matter_id)
        try:
            with store.matter_lock(matter_id):
                documents = store.get_documents(matter_id)
                result = propose_labels(documents, payload.ids, payload.mode,
                                        payload.prefix, payload.start,
                                        payload.group_separator,
                                        payload.group_suffix_style,
                                        payload.first_member_base,
                                        payload.existing_label_policy)
                reference_review = None
                if payload.reference_document_id:
                    reference = store.get_document(payload.reference_document_id)
                    if not reference or reference['matter_id'] != matter_id:
                        raise ValueError('Reference document is missing, archived or belongs to another matter')
                    mappings = [{'document_id': row['document_id'],
                                 'old_label': row['old_label'],
                                 'new_label': row['proposed_label']}
                                for row in result['rows']]
                    reference_review = extract_label_references(
                        store.working_path(reference), mappings, reference['revision'])
                    reference_review.update(document_id=reference['id'], title=reference['title'])
                    if reference_review['searchable_pages'] == 0:
                        result['warnings'].append(
                            'The selected reference document has no searchable text. OCR may help, but OCR results remain unverified.')
                result['reference_review'] = reference_review
                result['export_history'] = store.label_export_history(matter_id, payload.ids)
                result['policy'] = (
                    'Technical proposal only. Confirm the mapping and check court-specific requirements; '
                    'no numbering convention is represented as court-approved.')
                return result
        except ValueError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc
        except Exception as exc:
            raise HTTPException(status_code=409,
                detail=f'Citation review could not read the selected working copy: {exc}') from exc

    @app.post('/matters/{matter_id}/labels/apply')
    def apply_labels(matter_id: str, payload: ApplyLabelProposal,
                     _: None = Depends(auth)):
        matter_or_404(matter_id)
        mappings = [item.model_dump() for item in payload.mappings]
        try:
            with store.matter_lock(matter_id):
                reference_review = None
                if payload.reference_document_id:
                    if payload.expected_reference_revision is None:
                        raise ValueError('Expected reference revision is required')
                    reference = store.get_document(payload.reference_document_id)
                    if not reference or reference['matter_id'] != matter_id:
                        raise ValueError('Reference document is missing, archived or belongs to another matter')
                    if reference['revision'] != payload.expected_reference_revision:
                        raise ValueError('Reference document changed after preview; preview again')
                    reference_review = extract_label_references(
                        store.working_path(reference), mappings, reference['revision'])

                changed_existing = any(item['old_label'].strip() and
                                       item['old_label'].strip() != item['new_label'].strip()
                                       for item in mappings)
                history = store.label_export_history(
                    matter_id, [item['document_id'] for item in mappings])
                has_references = bool(reference_review and reference_review['references'])
                missing = []
                if changed_existing and not payload.acknowledge_existing_labels:
                    missing.append('existing labels and the old-to-new mapping')
                if history['has_prior_exports'] and not payload.acknowledge_prior_exports:
                    missing.append('prior generated builds, which remain unchanged and may have circulated')
                if has_references and not payload.acknowledge_references:
                    missing.append('citation references requiring human review')
                if missing:
                    raise RuntimeError('Acknowledge ' + '; '.join(missing) + ' before applying labels')
                documents = store.apply_label_mapping(matter_id, mappings)
                return {'documents': documents, 'applied': len(mappings),
                        'reference_review': reference_review, 'export_history': history}
        except RuntimeError as exc:
            raise HTTPException(status_code=409, detail=str(exc)) from exc
        except ValueError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc
        except Exception as exc:
            raise HTTPException(status_code=409,
                detail=f'Label mapping could not be applied: {exc}') from exc

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
        if 'label' in values and 'label_status' not in values:
            values['label'] = values['label'].strip()
            values['label_status'] = 'confirmed' if values['label'] else 'unreviewed'
            values['label_source'] = 'user' if values['label'] else 'none'
        if 'visible_signature_status' in values:
            values['signature_status'] = combined_signature_status(
                doc['certificate_signature_status'], values['visible_signature_status'])
        try:
            with store.matter_lock(doc['matter_id']):
                return store.update_document(doc_id, values)
        except ValueError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

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
        response = file_response(path, 'application/pdf', doc['id'] + '.pdf')
        response.headers['Cache-Control'] = 'no-store'
        return response

    def edit_working_copy(doc_id: str, operation, acknowledged: bool = False,
                          related_document_ids: list[str] | None = None):
        doc = doc_or_404(doc_id)
        with store.matter_lock(doc['matter_id']):
            doc = doc_or_404(doc_id)
            related = [doc_or_404(item) for item in (related_document_ids or [])]
            if any(item['matter_id'] != doc['matter_id'] for item in related):
                raise HTTPException(status_code=422, detail='Source must be in the same matter')
            require_signature_acknowledgement([doc, *related], acknowledged,
                                              'transforming this working copy')
            rel, candidate = store.next_working_path(doc)
            try:
                copy_file(store.working_path(doc), candidate)
                changes = operation(candidate, doc) or {}
                return store.update_document(doc_id, dict(changes, working_relpath=rel))
            except BaseException:
                store.discard_uncommitted_working(doc_id, rel, candidate)
                raise

    @app.post('/documents/{doc_id}/pages/reorder')
    def page_reorder(doc_id: str, payload: ReorderPages, _: None = Depends(auth)):
        try:
            return edit_working_copy(doc_id, lambda path, doc: {
                'page_count': reorder_pages(path, payload.order), 'ocr_status': 'modified'},
                payload.acknowledge_signature_risk)
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.post('/documents/{doc_id}/pages/rotate')
    def page_rotate(doc_id: str, payload: RotatePage, _: None = Depends(auth)):
        try:
            def rotate(path, doc):
                rotate_page(path, payload.page_index, payload.degrees)
                return {'page_count': doc['page_count']}
            return edit_working_copy(doc_id, rotate, payload.acknowledge_signature_risk)
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.post('/documents/{doc_id}/pages/delete')
    def page_delete(doc_id: str, payload: DeletePage, _: None = Depends(auth)):
        try:
            return edit_working_copy(doc_id, lambda path, doc: {
                'page_count': delete_page(path, payload.page_index)},
                payload.acknowledge_signature_risk)
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
                'ocr_status': 'modified'}, payload.acknowledge_signature_risk,
                [payload.source_document_id])
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.post('/documents/{doc_id}/pages/replace')
    def page_replace(doc_id: str, payload: ReplacePages, _: None = Depends(auth)):
        try:
            return edit_working_copy(doc_id, lambda path, doc: {
                'page_count': replace_pages(path, source_for_edit(doc, payload.source_document_id),
                                            payload.target_pages, payload.source_pages),
                'ocr_status': 'modified'}, payload.acknowledge_signature_risk,
                [payload.source_document_id])
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
            return edit_working_copy(doc_id, replace, payload.acknowledge_signature_risk,
                                     [payload.source_document_id])
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.post('/documents/{doc_id}/pages/crop')
    def page_crop(doc_id: str, payload: CropPages, _: None = Depends(auth)):
        try:
            return edit_working_copy(doc_id, lambda path, doc: {
                'page_count': crop_pages(path, payload.pages, payload.left, payload.top,
                                         payload.right, payload.bottom),
                'ocr_status': 'modified'}, payload.acknowledge_signature_risk)
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.post('/documents/{doc_id}/pages/text-overlay')
    def page_text_overlay(doc_id: str, payload: TextOverlay, _: None = Depends(auth)):
        try:
            def place(path, doc):
                add_text_overlay(path, payload.page_index, payload.x, payload.y,
                                 payload.text, payload.font_size)
                return {'page_count': doc['page_count'], 'ocr_status': 'modified'}
            return edit_working_copy(doc_id, place, payload.acknowledge_signature_risk)
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
            return dict(edit_working_copy(doc_id, replace, payload.acknowledge_signature_risk),
                        edit_layout=edit_layout)
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.post('/documents/{doc_id}/pages/move-text-run')
    def page_move_text_run(doc_id: str, payload: MoveTextRun, _: None = Depends(auth)):
        try:
            def move(path, doc):
                if doc['revision'] != payload.expected_revision:
                    raise HTTPException(status_code=409, detail='Document changed; reload and select text again')
                move_text_run(path, payload.page_index, payload.span_index,
                              payload.expected_text, payload.vertical_offset)
                return {'page_count': doc['page_count'], 'ocr_status': 'modified'}
            return edit_working_copy(doc_id, move, payload.acknowledge_signature_risk)
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    def page_selection(value: str) -> list[int]:
        if not value or len(value) > 10000 or any(piece == '' or not piece.isdecimal()
                                                   for piece in value.split(',')):
            raise HTTPException(status_code=422, detail='Use comma-separated zero-based page indexes')
        return [int(piece) for piece in value.split(',')]

    @app.get('/documents/{doc_id}/pages/extract')
    def page_extract(doc_id: str, selection: str, acknowledge_signature_risk: bool = False,
                     _: None = Depends(auth)):
        doc = doc_or_404(doc_id)
        try:
            with store.matter_lock(doc['matter_id']):
                current = doc_or_404(doc_id)
                require_signature_acknowledgement([current], acknowledge_signature_risk,
                                                  'creating an extracted PDF')
                data = extract_pages(store.working_path(current), page_selection(selection))
            return Response(data, media_type='application/pdf', headers={
                'Content-Disposition': 'attachment; filename="extracted-pages.pdf"'})
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.get('/documents/{doc_id}/pages/split')
    def page_split(doc_id: str, after: str, acknowledge_signature_risk: bool = False,
                   _: None = Depends(auth)):
        doc = doc_or_404(doc_id)
        try:
            with store.matter_lock(doc['matter_id']):
                current = doc_or_404(doc_id)
                require_signature_acknowledgement([current], acknowledge_signature_risk,
                                                  'creating split PDFs')
                data = split_pages(store.working_path(current), page_selection(after))
            return Response(data, media_type='application/zip', headers={
                'Content-Disposition': 'attachment; filename="split-pages.zip"'})
        except ProcessingError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    def submit_ocr(doc: dict, job_id: str | None = None,
                   acknowledge_signature_risk: bool = False):
        doc_id = doc['id']
        require_signature_acknowledgement([doc], acknowledge_signature_risk, 'running OCR')
        def task(job_id, progress, cancelled):
            with store.matter_lock(doc['matter_id']):
                current = store.get_document(doc_id)
                if current is None:
                    raise ProcessingError('Document was removed before OCR started')
                require_signature_acknowledgement([current], acknowledge_signature_risk,
                                                  'running OCR')
                rel, candidate = store.next_working_path(current)
                committed = False
                try:
                    copy_file(store.working_path(current), candidate)
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
    def start_ocr(doc_id: str, payload: SignatureRiskAcknowledgement | None = None,
                  _: None = Depends(auth)):
        return submit_ocr(doc_or_404(doc_id), acknowledge_signature_risk=(
            payload.acknowledge_signature_risk if payload else False))

    @app.post('/documents/{doc_id}/compress', status_code=202)
    def start_compression(doc_id: str, payload: CompressionRequest, _: None = Depends(auth)):
        doc = doc_or_404(doc_id)
        require_signature_acknowledgement([doc], payload.acknowledge_signature_risk,
                                          'compressing this working copy')
        def task(job_id, progress, cancelled):
            with store.matter_lock(doc['matter_id']):
                current = doc_or_404(doc_id)
                require_signature_acknowledgement([current], payload.acknowledge_signature_risk,
                                                  'compressing this working copy')
                if current['revision'] != payload.expected_revision:
                    raise ProcessingError('Document changed before compression; reload and try again')
                rel, candidate = store.next_working_path(current)
                committed = False
                try:
                    copy_file(store.working_path(current), candidate)
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

    @app.post('/matters/{matter_id}/bundle-preview')
    def preview_bundle(matter_id: str, payload: BundlePreviewRequest,
                       _: None = Depends(auth)):
        with store.matter_lock(matter_id):
            matter_or_404(matter_id)
            try:
                return bundle_preview(store, matter_id, payload.max_pages_per_volume,
                                      payload.keep_groups_intact)
            except ProcessingError as exc:
                raise HTTPException(status_code=422, detail=str(exc)) from exc

    def submit_build(matter_id: str, job_id: str | None = None,
                     acknowledge_signature_risk: bool = False):
        require_signature_acknowledgement(store.get_documents(matter_id),
                                          acknowledge_signature_risk,
                                          'building transformed bundle PDFs')
        def task(job_id, progress, cancelled):
            with store.matter_lock(matter_id):
                return build_bundle(store, matter_id, job_id, progress, cancelled,
                                    signature_acknowledged=acknowledge_signature_risk)
        return jobs.submit(matter_id, 'build', task, job_id=job_id)

    @app.post('/matters/{matter_id}/build', status_code=202)
    def start_build(matter_id: str, payload: SignatureRiskAcknowledgement | None = None,
                    _: None = Depends(auth)):
        matter_or_404(matter_id)
        return submit_build(matter_id, acknowledge_signature_risk=(
            payload.acknowledge_signature_risk if payload else False))

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
    def retry_job(job_id: str, payload: SignatureRiskAcknowledgement | None = None,
                  _: None = Depends(auth)):
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
        acknowledged = payload.acknowledge_signature_risk if payload else False
        if prior['kind'] == 'ocr':
            require_signature_acknowledgement([doc], acknowledged, 'retrying OCR')
        else:
            require_signature_acknowledgement(store.get_documents(prior['matter_id']),
                                              acknowledged, 'retrying bundle generation')
        new_job_id = new_id()
        if not jobs.claim_retry(job_id, new_job_id):
            raise HTTPException(status_code=409, detail='Only an interrupted job may be retried once')
        return (submit_ocr(doc, new_job_id, acknowledged) if prior['kind'] == 'ocr'
                else submit_build(prior['matter_id'], new_job_id, acknowledged))

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
        return file_response(path, media, filename)

    return app
