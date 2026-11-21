"""Emit the Python reference API and PDF dependencies as machine-readable JSON."""
import ast
import json
from pathlib import Path
import sys
import tempfile
import warnings

from fastapi.routing import APIRoute

ROOT = Path(__file__).resolve().parents[2]
BACKEND = ROOT / 'backend'
sys.path.insert(0, str(BACKEND))

from app.main import create_app

# Status is assigned against the Rust desktop IPC and core, not the optional Python worker.
RUST_COVERAGE = {
    'pilot': {
        'available_profiles', 'matters', 'archived_matters', 'new_matter', 'get_matter',
        'patch_matter', 'delete_matter', 'restore_archived_matter', 'matter_audit',
        'get_documents', 'archived_documents', 'restore_archived_document', 'remove_doc',
        'order_docs', 'add_matter_part', 'remove_matter_part', 'get_memo_parties',
        'put_memo_parties', 'get_matter_values', 'put_matter_values',
        'get_filing_review', 'put_filing_review', 'read_position', 'save_position',
        'patch_doc', 'batch_patch_docs', 'preview_doc_sort',
        'get_membership', 'apply_membership', 'manual_bookmarks', 'update_manual_bookmarks',
        'reviewed_links', 'update_reviewed_links', 'output_marks', 'update_output_marks',
        'list_output_presets', 'create_output_preset', 'applied_output_preset',
        'apply_output_preset', 'bundle_settings', 'save_bundle_settings',
        'preflight_history', 'preflight_review_state', 'save_preflight_review',
        'get_finding', 'review_finding',
    },
    'pilot_subset': {
        'health', 'history_state', 'traverse_history', 'matter_storage', 'exports',
        'upload', 'upload_raw', 'vault_status', 'original_info', 'read_original',
        'document_pdf', 'retained_revision_pdf', 'search_matter', 'search_document',
        'preview_labels', 'apply_labels', 'metadata_snapshot', 'save_metadata_review',
        'metadata_suggestions', 'accept_metadata', 'metadata_source', 'document_thumbnail',
        'preview_bundle', 'preview_volume_planning', 'page_extract', 'page_split', 'validate_matter',
    },
    'protected_pilot': {
        'vault_setup', 'vault_unlock', 'vault_rotate', 'page_rotate',
        'retained_revisions', 'restore_revision',
        'page_batch', 'page_crop', 'page_delete', 'page_insert', 'page_reorder',
        'page_replace', 'document_replace',
    },
    'rust_source_unverified': {
        'backup_matter', 'restore_matter', 'generate_cover',
        'page_annotations', 'create_page_annotation', 'create_image_overlay',
        'update_page_annotation', 'delete_page_annotation', 'page_annotation_asset',
        'printable_working_pdf', 'printable_original_pdf', 'page_replace_file',
        'page_text_overlay', 'page_image_overlay', 'page_text_runs', 'page_output_marks_preview',
        'page_replace_text_run', 'pdfium_paragraph_trial_status', 'page_paragraph_trial_preview',
        'page_paragraph_trial', 'page_move_text_run', 'page_redact', 'list_document_redactions',
        'remove_document_redaction', 'start_ocr', 'ocr_review', 'accept_ocr_review', 'rerun_ocr_page',
        'start_compression', 'screen_preflight_pages', 'preview_front_matter', 'preview_numbering',
        'start_build', 'start_supplement_build', 'matter_jobs', 'job_status', 'cancel_job', 'retry_job',
        'variant_catalog', 'create_variant', 'update_variant', 'review_variant', 'start_variant_build',
        'supplemental_catalog', 'create_supplement', 'save_supplement', 'preview_supplement',
        'preview_supplement_policy', 'archive_build', 'compare_pdf_pages', 'compare_export',
        'output_check_state', 'run_output_checks', 'save_output_review', 'inspect_output',
        'export_file', 'office_review_export', 'printable_export', 'protected_pdf_export',
    },
}


def rust_statuses(routes):
    handlers = {route.endpoint.__name__ for route in routes}
    statuses = {}
    for status, names in RUST_COVERAGE.items():
        assert names <= handlers, f'Inventory refers to unknown Python handlers: {names - handlers}'
        assert not statuses.keys() & names, f'Duplicate Rust coverage: {statuses.keys() & names}'
        statuses.update(dict.fromkeys(names, status))
    assert handlers == statuses.keys(), f"Unmapped Python handlers: {handlers - statuses.keys()}"
    return statuses


def call_sites(node):
    for child in ast.walk(node):
        if isinstance(child, ast.Call):
            yield child, ast.unparse(child.func)


def route_sources():
    filename = BACKEND / 'app/main.py'
    source = filename.read_text(encoding='utf-8')
    handlers = [node for node in ast.walk(ast.parse(source))
                if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))]
    by_name = {handler.name: handler for handler in handlers}
    references = {}
    for handler in handlers:
        nodes = [handler]
        seen = set()
        calls = set()
        status_codes = set()
        while nodes:
            node = nodes.pop()
            if node.name in seen:
                continue
            seen.add(node.name)
            for call, name in call_sites(node):
                if name in by_name and name not in seen:
                    nodes.append(by_name[name])
                if name:
                    calls.add(name)
                if name not in {'HTTPException', 'Response', 'JSONResponse'}:
                    continue
                for keyword in call.keywords:
                    if (keyword.arg == 'status_code' and isinstance(keyword.value, ast.Constant)
                            and type(keyword.value.value) is int):
                        status_codes.add(keyword.value.value)
        references[handler.name] = {
            'file': filename.relative_to(ROOT).as_posix(),
            'line': handler.lineno,
            'calls': sorted(calls),
            'explicit_status_codes': sorted(status_codes),
        }
    return references


def pdf_dependencies():
    dependencies = []
    for filename in sorted((BACKEND / 'app').glob('*.py')):
        source = filename.read_text(encoding='utf-8')
        dependencies.extend(
            {'file': filename.relative_to(ROOT).as_posix(), 'line': node.lineno, 'call': name}
            for node, name in call_sites(ast.parse(source))
            if name and (name.startswith('pymupdf.') or name in {'open_pdf', 'save_pdf'})
        )
    return sorted(dependencies, key=lambda record: (record['file'], record['line'], record['call']))


def main():
    # Include HEAD routes even where the Python OpenAPI operation IDs are shared.
    with tempfile.TemporaryDirectory(prefix='matterdraft-route-inventory-') as directory:
        app = create_app(Path(directory), 'synthetic-route-inventory')
        with warnings.catch_warnings():
            warnings.filterwarnings('ignore', message='Duplicate Operation ID', module='fastapi.openapi.utils')
            operations = app.openapi()
        for path in operations['paths'].values():
            for operation in path.values():
                operation.pop('operationId', None)
    sources = route_sources()
    routes = sorted((route for route in app.routes if isinstance(route, APIRoute)),
                    key=lambda route: (route.path, sorted(route.methods)))
    statuses = rust_statuses(routes)
    inventory = {
        'routes': [
            {'path': route.path, 'method': method, 'handler': route.endpoint.__name__,
             'success_status': route.status_code or 200,
             'rust_status': statuses.get(route.endpoint.__name__, 'pending'),
             'dependencies': [dependency.call.__name__ for dependency in route.dependant.dependencies],
             'operation': operations['paths'].get(route.path, {}).get(method.lower()),
             'source': sources[route.endpoint.__name__]}
            for route in routes for method in sorted(route.methods)
        ],
        'schemas': operations.get('components', {}).get('schemas', {}),
        'pdf_call_sites': pdf_dependencies(),
    }
    json.dump(inventory, sys.stdout, indent=2, sort_keys=True)
    print()


if __name__ == '__main__':
    main()
