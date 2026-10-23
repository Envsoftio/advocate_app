from test_api import upload, wait_for_job


def test_user_authored_scoped_checklist_is_review_only_and_recorded(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}'
    upload(client, auth, matter, make_pdf, pages=1)
    review = {'court': 'Synthetic Forum', 'case_type': 'Demonstration',
              'stage': 'Initial filing', 'source_note': 'User-entered test note',
              'items': [{'title': 'Synthetic petition', 'part': 1, 'status': 'present'},
                        {'title': 'Synthetic translation review', 'part': 2,
                         'status': 'unreviewed'}]}
    saved = client.put(base + '/filing-review', headers=auth, json=review)
    assert saved.status_code == 200, saved.text
    assert client.get(base + '/filing-review', headers=auth).json() == review
    checks = client.get(base + '/validate', headers=auth).json()
    assert any(item['code'] == 'USER_CHECKLIST_PENDING' and item['severity'] == 'info'
               for item in checks)
    started = client.post(base + '/build', headers=auth)
    assert wait_for_job(client, auth, started.json()['id'])['status'] == 'completed'
    prior = client.get(base + '/exports', headers=auth).json()[0]
    assert prior['filing_review'] == review
    assert prior['stale'] is False
    review['items'][1]['status'] = 'not_applicable'
    assert client.put(base + '/filing-review', headers=auth, json=review).status_code == 200
    assert client.get(base + '/exports', headers=auth).json()[0]['stale'] is True
    comparison = client.post(base + f'/exports/{prior["id"]}/compare', headers=auth,
                             json={'max_pages_per_volume': 300,
                                   'keep_groups_intact': True}).json()
    assert 'User-authored review checklist' in comparison['setting_changes']
    assert client.post(base + '/history/undo', headers=auth).status_code == 200
    assert client.get(base + '/filing-review', headers=auth).json()['items'][1]['status'] == 'unreviewed'
    backup = client.get(base + '/portable', headers=auth)
    restored = client.post('/matters/restore', headers=auth,
                           files={'file': ('review.cbmatter', backup.content, 'application/zip')})
    assert restored.status_code == 201, restored.text
    restored_review = client.get(f'/matters/{restored.json()["id"]}/filing-review', headers=auth).json()
    assert restored_review['court'] == 'Synthetic Forum'
    assert restored_review['items'][1]['status'] == 'unreviewed'


def test_checklist_requires_explicit_user_items(client, auth, matter, make_pdf):
    base = f'/matters/{matter["id"]}/filing-review'
    assert client.get(base, headers=auth).json()['items'] == []
    bad = client.put(base, headers=auth, json={
        'court': '', 'case_type': '', 'stage': '', 'source_note': '',
        'items': [{'title': ' ', 'part': 1, 'status': 'present'}]})
    assert bad.status_code == 422
    assert client.get(base, headers=auth).json()['items'] == []
    upload(client, auth, matter, make_pdf, pages=1)
    saved = client.put(base, headers=auth, json={'court': '', 'case_type': 'Different case',
        'stage': '', 'source_note': '', 'items': []})
    assert saved.status_code == 200
    checks = client.get(f'/matters/{matter["id"]}/validate', headers=auth).json()
    assert any(item['code'] == 'USER_CHECKLIST_SCOPE_MISMATCH' for item in checks)
