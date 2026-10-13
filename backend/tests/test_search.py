import io

import pymupdf


def test_search_current_working_copy_and_revisions(client, auth, matter):
    with pymupdf.open() as pdf:
        first = pdf.new_page()
        first.insert_text((60, 80), 'PETITION ALPHA searchable source')
        second = pdf.new_page()
        second.insert_text((60, 80), 'ANNEXURE BRAVO searchable source')
        data = pdf.tobytes()
    response = client.post(
        f'/matters/{matter["id"]}/documents', headers=auth,
        files=[('files', ('search.pdf', io.BytesIO(data), 'application/pdf'))],
    )
    assert response.status_code == 201, response.text
    document = response.json()[0]
    url = f'/matters/{matter["id"]}/search'
    assert client.get(url, params={'q': 'bravo'}).status_code == 401
    found = client.get(url, headers=auth, params={'q': 'bravo'}).json()
    assert found['truncated'] is False
    assert [(item['document_id'], item['page'], item['revision']) for item in found['matches']] == [
        (document['id'], 2, document['revision'])]
    assert 'BRAVO' in found['matches'][0]['snippet']
    assert client.get(url, headers=auth, params={'q': 'a'}).status_code == 422
    changed = client.post(f'/documents/{document["id"]}/pages/delete', headers=auth,
                          json={'page_index': 1})
    assert changed.status_code == 200, changed.text
    assert client.get(url, headers=auth, params={'q': 'bravo'}).json()['matches'] == []
    found = client.get(url, headers=auth, params={'q': 'alpha'}).json()['matches']
    assert len(found) == 1 and found[0]['revision'] > document['revision']
