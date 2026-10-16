"""Policy-neutral label proposals and review-only citation discovery.

These helpers do not encode a court rule. They only generate a preview from
explicit user parameters and report searchable text that still needs review.
"""
from __future__ import annotations

import re
from pathlib import Path

from .file_io import open_pdf


def _letters(index: int) -> str:
    value = index + 1
    result = ''
    while value:
        value, remainder = divmod(value - 1, 26)
        result = chr(97 + remainder) + result
    return result


def _group_suffix(index: int, style: str) -> str:
    if style.startswith('upper_alpha'):
        value = _letters(index).upper()
    elif style.startswith('numeric'):
        value = str(index + 1)
    else:
        value = _letters(index)
    return f'({value})' if style.endswith('_parenthesized') else value


def _filename_label(name: str) -> tuple[str, str]:
    stem = Path(name).stem
    matches = {match.group(1).strip().replace(' ', '-') for match in re.finditer(
        r'(?i)\b(?:annexure|annex|exhibit)[\s._-]+([a-z]{0,8}[- ]?\d+[a-z]?)\b', stem)}
    if len(matches) == 1:
        return next(iter(matches)), 'One annexure/exhibit token was found in the filename.'
    if len(matches) > 1:
        return '', 'Multiple possible labels were found in the filename; choose manually.'
    return '', 'No conservative annexure/exhibit label was found in the filename.'


def propose_labels(documents: list[dict], selected_ids: list[str], mode: str,
                   prefix: str, start: int, group_separator: str = '',
                   group_suffix_style: str = 'lower_alpha',
                   first_member_base: bool = False,
                   existing_label_policy: str = 'compact') -> dict:
    by_id = {doc['id']: doc for doc in documents}
    if len(selected_ids) != len(set(selected_ids)) or any(item not in by_id for item in selected_ids):
        raise ValueError('Selection contains a missing, archived or duplicate document')
    selected = [doc for doc in documents if doc['id'] in set(selected_ids)]
    selected_set = set(selected_ids)
    warnings: list[str] = []
    blocking: list[str] = []
    proposals: dict[str, tuple[str, str]] = {}

    if mode == 'sequential':
        for offset, doc in enumerate(selected):
            proposals[doc['id']] = (f'{prefix}{start + offset}',
                                    'Sequential technical proposal in current matter order.')
    elif mode == 'grouped_suffix':
        grouped: dict[str, list[dict]] = {}
        for doc in documents:
            key = doc['group_key'].strip()
            if key:
                grouped.setdefault(key, []).append(doc)
        for key, members in grouped.items():
            chosen = [doc for doc in members if doc['id'] in selected_set]
            if chosen and len(chosen) != len(members):
                blocking.append(f'Group “{key}” is only partly selected; select all {len(members)} members or use sequential mode.')
        unit_number = start
        handled_groups: set[str] = set()
        for doc in selected:
            key = doc['group_key'].strip()
            if not key:
                proposals[doc['id']] = (f'{prefix}{unit_number}',
                                        'Ungrouped document receives one base number.')
                unit_number += 1
                continue
            if key in handled_groups:
                continue
            handled_groups.add(key)
            members = [item for item in selected if item['group_key'].strip() == key]
            parents = [item for item in members if item.get('group_role', 'member') == 'parent']
            if len(parents) > 1:
                blocking.append(f'Group “{key}” has more than one designated parent.')
            explicit_parent = parents[0] if len(parents) == 1 else None
            children = [item for item in members if item is not explicit_parent]
            for index, member in enumerate(members):
                if explicit_parent and member['id'] == explicit_parent['id']:
                    proposed = f'{prefix}{unit_number}'
                    detail = 'the explicitly designated parent keeps the base label'
                elif explicit_parent:
                    suffix_index = children.index(member)
                    proposed = (f'{prefix}{unit_number}{group_separator}'
                                f'{_group_suffix(suffix_index, group_suffix_style)}')
                    detail = f'child suffix style {group_suffix_style.replace("_", " ")}'
                elif first_member_base and index == 0:
                    proposed = f'{prefix}{unit_number}'
                    detail = 'the first ordered member keeps the base label'
                else:
                    suffix_index = index - 1 if first_member_base else index
                    proposed = (f'{prefix}{unit_number}{group_separator}'
                                f'{_group_suffix(suffix_index, group_suffix_style)}')
                    detail = f'suffix style {group_suffix_style.replace("_", " ")}'
                proposals[member['id']] = (proposed,
                    f'Configured grouped proposal for group “{key}” ({detail}); group order is unchanged.')
            unit_number += 1
        warnings.append('Grouped formatting is an explicit technical choice. A designated parent records label hierarchy but does not establish a legal relationship or court approval.')
    elif mode == 'filename':
        for doc in selected:
            proposals[doc['id']] = _filename_label(doc['original_name'])
        warnings.append('Filename parsing is conservative and remains unreviewed until every proposed label is confirmed.')
    else:
        raise ValueError('Unknown label proposal mode')

    if existing_label_policy == 'preserve':
        for doc in selected:
            if doc['label'].strip():
                proposals[doc['id']] = (doc['label'].strip(),
                    'Existing label preserved; no circulated reference is renumbered automatically.')
            else:
                proposals[doc['id']] = ('',
                    'Preserve-existing policy requires the reviewer to enter a label for this new or unlabelled document.')
        warnings.append('Preserve-existing policy keeps current labels and requires manual labels for new documents. Choose compact to recalculate the selected sequence after insertion or removal.')

    rows = []
    for doc in selected:
        proposed, reason = proposals.get(doc['id'], ('', 'No proposal was generated.'))
        if len(proposed) > 60:
            blocking.append(f'Proposed label for “{doc["title"]}” exceeds 60 characters.')
            proposed = proposed[:60]
        rows.append({
            'document_id': doc['id'], 'title': doc['title'],
            'original_name': doc['original_name'], 'description': doc['title'],
            'group_key': doc['group_key'],
            'group_role': doc.get('group_role', 'member'), 'old_label': doc['label'],
            'old_status': doc.get('label_status', 'unreviewed'),
            'proposed_label': proposed,
            'proposal_status': 'suggested' if proposed else 'needs_manual',
            'reason': reason,
        })

    final_labels = {doc['id']: doc['label'].strip() for doc in documents}
    for row in rows:
        final_labels[row['document_id']] = row['proposed_label'].strip()
    seen: dict[str, str] = {}
    for doc in documents:
        label = final_labels[doc['id']]
        if not label:
            continue
        folded = label.casefold()
        if folded in seen:
            blocking.append(f'Label “{label}” would be duplicated.')
        else:
            seen[folded] = doc['id']
    if any(not row['proposed_label'].strip() for row in rows):
        blocking.append('Every selected document needs a proposed label before this mapping can be applied.')
    return {'rows': rows, 'warnings': list(dict.fromkeys(warnings)),
            'blocking': list(dict.fromkeys(blocking))}


def extract_label_references(pdf_path: Path, mappings: list[dict], revision: int) -> dict:
    old = {item['old_label'].strip().casefold(): item for item in mappings if item['old_label'].strip()}
    new = {item['new_label'].strip().casefold(): item for item in mappings if item['new_label'].strip()}
    references = []
    searchable_pages = 0
    pattern = re.compile(
        r'(?i)\b(?:annexure|annex|exhibit)\s*(?:no\.?\s*)?[-:–—]?\s*([a-z0-9][a-z0-9./-]{0,30})')
    with open_pdf(pdf_path) as pdf:
        for page_index, page in enumerate(pdf):
            text = ' '.join(page.get_text('text').split())
            if text:
                searchable_pages += 1
            for match in pattern.finditer(text):
                token = match.group(1).rstrip('.,;:)').strip()
                normalized = token.casefold()
                mapped = old.get(normalized)
                status = 'old_label' if mapped else 'proposed_label' if normalized in new else 'unmatched'
                target = mapped or new.get(normalized)
                start = max(0, match.start() - 65)
                end = min(len(text), match.end() + 90)
                references.append({
                    'page': page_index + 1, 'raw': match.group(0), 'label': token,
                    'status': status,
                    'document_id': target['document_id'] if target else None,
                    'snippet': ('…' if start else '') + text[start:end] + ('…' if end < len(text) else ''),
                })
                if len(references) >= 200:
                    return {'revision': revision, 'searchable_pages': searchable_pages,
                            'references': references, 'truncated': True}
    return {'revision': revision, 'searchable_pages': searchable_pages,
            'references': references, 'truncated': False}
