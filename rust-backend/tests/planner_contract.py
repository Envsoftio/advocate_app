"""Compare full JSON page maps produced by the Python and Rust planners."""
from itertools import product
import json
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'backend'))

from app.engine import bundle_preview
from app.pdf_ops import ProcessingError
from app.variants import ordered_documents


def document(index, pages=1, **overrides):
    return dict(id=f'{index:032x}', title=f'Synthetic source {index}', page_count=pages, part=1,
                group_key='', kind='other', revision=1, label=str(index), document_date=None,
                archived_at=None, byte_size=301 + index) | overrides


def bookmark(index, page, *, resolved=True):
    return dict(id=f'manual-{index}-{page}', title=f'Manual target {index}:{page}',
                document_id=f'{index:032x}', page_index=page, resolved=resolved)


def case(name, documents, *, settings=None, max_pages=30, keep_groups=True,
         bookmarks=None, all_documents=None):
    return dict(name=name, documents=documents, all_documents=all_documents or documents,
                max_pages=max_pages, keep_groups=keep_groups, settings=settings or {},
                bookmarks=bookmarks or [])


def fixture_cases():
    yield case('empty matter with unresolved document', [], bookmarks=[bookmark(990, 0)])
    yield case('one source at page cap', [document(1, 2)], max_pages=3)
    for style, date in product(['compact', 'readable', 'review'], [False, True]):
        docs = [document(index) for index in range(1, 49)]
        capacity = 8 if style == 'review' else 16 if style == 'readable' or date else 23
        for cap in [capacity + 2, capacity + 3, 60]:
            yield case(f'index growth {style=} {date=} {cap=}', docs,
                       max_pages=cap, settings={'index_style': style, 'index_date': date})

    docs = [document(1, 2, group_key=' A '), document(2, 2, group_key='A', kind='annexure'),
            document(3, 3, part=2, kind='annexure'), document(4, 4, part=2),
            document(5, 2, part=3, kind='annexure', document_date='2025-01-01')]
    manual = [bookmark(1, 0), bookmark(3, 1), bookmark(4, 3), bookmark(4, 0, resolved=False),
              bookmark(5, 20), bookmark(900, 0)]
    for style, restart, front, labels in product(
            ['roman_lower', 'arabic', 'roman_upper'], ['volume', 'continuous', 'section'],
            ['physical', 'roman', 'none'], [True, False]):
        yield case(f'numbering {style=} {restart=} {front=} {labels=}', docs,
                   max_pages=9, bookmarks=manual, settings={
                       'front_matter': 'cover_index', 'numbering_style': style,
                       'numbering_restart': restart, 'numbering_start': 3998,
                       'numbering_front_matter': front, 'numbering_pdf_labels': labels,
                       'numbering_prefix': 'V{volume} P{section}-', 'part_extracts_enabled': True,
                       'annexure_mode': 'separator', 'annexure_overrides': {docs[2]['id']: 'label_stamp'},
                       'bates_enabled': True, 'bates_prefix': 'B-', 'bates_start': 98,
                       'bates_padding': 2, 'bates_include_index': True,
                       'bates_include_separator': labels, 'bates_restart_per_volume': labels,
                   })

    split = document(6, 6, revision=2, byte_size=303, kind='annexure')
    for revision, cuts in [(2, [1, 4]), (2, [0]), (2, []), (2, [4, 4]), (2, [5, 3]),
                           (2, [6]), (1, [3])]:
        yield case(f'reviewed split {revision=} {cuts=}', [docs[0], split, docs[1]],
                   max_pages=7, bookmarks=[bookmark(6, index) for index in range(6)], settings={
                       'annexure_mode': 'separator', 'part_extracts_enabled': True,
                       'volume_splits': {split['id']: {'revision': revision, 'after_pages': cuts}},
                   })
    for keep_groups in [True, False]:
        yield case(f'noncontiguous group {keep_groups=}', [docs[0], docs[2], docs[1]],
                   keep_groups=keep_groups)
        yield case(f'oversized group {keep_groups=}', docs[:2], max_pages=4, keep_groups=keep_groups)
    yield case('oversized document', [document(10, 500)], max_pages=200)
    selected_runs = [split | {'_source_start': 4, '_source_total': 6, 'page_count': 2},
                     docs[2], split | {'_source_start': 1, '_source_total': 6, 'page_count': 1}]
    yield case('noncontiguous supplemental source runs', selected_runs, max_pages=5,
               all_documents=[*docs, split], bookmarks=[bookmark(6, index) for index in range(6)])
    variant = case('variant ordering and exclusions', [], all_documents=docs, bookmarks=manual)
    variant['variant_ids'] = [docs[index]['id'] for index in (4, 2, 0)]
    yield variant


class FixtureStore:
    def __init__(self, root, fixture):
        self.root = root
        self.fixture = fixture

    def get_documents(self, _):
        return self.fixture['all_documents']

    def list_manual_bookmarks(self, _):
        return self.fixture['bookmarks']

    def working_path(self, doc):
        return self.root / str(doc['byte_size'])


def python_expected(root, fixtures):
    for fixture in fixtures:
        store = FixtureStore(root, fixture)
        if 'variant_ids' in fixture:
            fixture['documents'] = ordered_documents(store, 'synthetic', fixture['variant_ids'])
        fixture['document_titles'] = {doc['id']: doc['title'] for doc in fixture['all_documents']}
        for doc in fixture['documents']:
            store.working_path(doc).write_bytes(bytes(doc['byte_size']))
        try:
            expected = bundle_preview(store, 'synthetic', fixture['max_pages'], fixture['keep_groups'],
                                      fixture['settings'], documents_override=fixture['documents'])
            yield {'plan': expected}
        except ProcessingError as error:
            yield {'error': str(error)}


def main():
    fixtures = list(fixture_cases())
    with tempfile.TemporaryDirectory(prefix='matterdraft-planner-') as directory:
        expected = list(python_expected(Path(directory), fixtures))
    run = subprocess.run(
        ['cargo', 'run', '--quiet', '--offline', '--locked', '--manifest-path',
         str(ROOT / 'rust-backend/Cargo.toml'), '--example', 'planner_contract'],
        input=json.dumps(fixtures), text=True, check=True, capture_output=True,
    )
    for case_data, python, rust in zip(fixtures, expected, json.loads(run.stdout), strict=True):
        compare(python, rust, case_data['name'])
    print(f'{len(fixtures)} exact Python/Rust page-map cases passed')


def compare(python, rust, path):
    if isinstance(python, dict) and isinstance(rust, dict):
        assert python.keys() == rust.keys(), (path, python.keys(), rust.keys())
        for key in python:
            compare(python[key], rust[key], f'{path}.{key}')
    elif isinstance(python, list) and isinstance(rust, list):
        assert len(python) == len(rust), (path, len(python), len(rust))
        for index, (left, right) in enumerate(zip(python, rust, strict=True)):
            compare(left, right, f'{path}[{index}]')
    else:
        assert type(python) is type(rust) and python == rust, (path, python, rust)


if __name__ == '__main__':
    main()
