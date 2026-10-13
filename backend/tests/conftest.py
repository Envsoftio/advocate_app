import pytest
from fastapi.testclient import TestClient
from app.main import create_app


@pytest.fixture
def client(tmp_path):
    app = create_app(data_dir=tmp_path / 'userdata', token='unit-test-secret')
    with TestClient(app) as test:
        yield test


@pytest.fixture
def auth():
    return {'Authorization': 'Bearer unit-test-secret'}


@pytest.fixture
def matter(client, auth):
    response = client.post('/matters', headers=auth, json={
        'name': 'Synthetic Court Filing', 'court_profile': 'delhi-hc-illustrative',
        'case_type': 'Demonstration', 'petitioner': 'Example Applicant',
        'respondent': 'Example Respondent', 'max_pages_per_volume': 300})
    assert response.status_code == 201, response.text
    return response.json()


@pytest.fixture
def make_pdf():
    import pymupdf
    def create(pages=2, text='Hello, searchable document'):
        with pymupdf.open() as pdf:
            for i in range(pages):
                p = pdf.new_page(width=595, height=842)
                if text:
                    p.insert_text((70,90), f'{text} page {i+1}', fontsize=18)
            return pdf.tobytes()
    return create
