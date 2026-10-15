import pytest

from app.file_io import read_bytes
from app.storage import Store
from app.vault import MAGIC, VaultError
from app.vault_store import VaultStore


def test_setup_migrates_legacy_files_and_database_then_password_recovery(tmp_path):
    legacy = Store(tmp_path)
    try:
        matter = legacy.create_matter({'name': 'Synthetic legacy secret',
            'court_profile': 'delhi-hc-illustrative', 'case_type': 'Synthetic',
            'petitioner': '', 'respondent': '', 'advocate': '', 'case_number': '',
            'max_pages_per_volume': 10, 'keep_groups_intact': True})
        file = legacy.matter_dir(matter['id']) / 'originals' / 'synthetic.pdf'
        file.parent.mkdir(parents=True)
        file.write_bytes(b'CONFIDENTIAL SYNTHETIC FILE')
    finally:
        legacy.close()
    vault = VaultStore(tmp_path)
    assert vault.status() == 'setup'
    key, recovery = vault.setup('synthetic password 123')
    assert vault.status() == 'locked'
    assert file.read_bytes().startswith(MAGIC)
    assert b'CONFIDENTIAL SYNTHETIC' not in file.read_bytes()
    assert not (tmp_path / 'court_bundle.sqlite3').read_bytes().startswith(b'SQLite format 3')
    with pytest.raises(VaultError):
        vault.unlock('wrong password')
    assert vault.unlock('synthetic password 123') == key
    assert vault.unlock(recovery, recovery=True) == key
    store = Store(tmp_path, data_key=key)
    try:
        assert store.get_matter(matter['id'])['name'] == 'Synthetic legacy secret'
        assert read_bytes(file) == b'CONFIDENTIAL SYNTHETIC FILE'
    finally:
        store.close()
    new_recovery = vault.rotate(key, 'new synthetic password 123')
    with pytest.raises(VaultError):
        vault.unlock('synthetic password 123')
    with pytest.raises(VaultError):
        vault.unlock(recovery, recovery=True)
    assert vault.unlock(new_recovery, recovery=True) == key


def test_interrupted_file_migration_resumes_with_password(tmp_path, monkeypatch):
    first = tmp_path / 'first.pdf'
    second = tmp_path / 'second.pdf'
    first.write_bytes(b'SYNTHETIC FIRST')
    second.write_bytes(b'SYNTHETIC SECOND')
    vault = VaultStore(tmp_path)
    import app.vault_store as module
    real_encrypt = module.encrypt_file
    count = 0
    def interrupted(source, destination, key):
        nonlocal count
        count += 1
        if count == 2:
            raise OSError('synthetic interruption')
        return real_encrypt(source, destination, key)
    monkeypatch.setattr(module, 'encrypt_file', interrupted)
    with pytest.raises(OSError):
        vault.setup('synthetic password 123')
    assert vault.status() == 'locked'
    monkeypatch.setattr(module, 'encrypt_file', real_encrypt)
    key = vault.unlock('synthetic password 123')
    assert len(key) == 32
    assert first.read_bytes().startswith(MAGIC)
    assert second.read_bytes().startswith(MAGIC)
    assert vault._load()['state'] == 'active'


def test_migration_refuses_open_legacy_engine(tmp_path):
    legacy = Store(tmp_path)
    try:
        vault = VaultStore(tmp_path)
        with pytest.raises(VaultError, match='open in another'):
            vault.setup('synthetic password 123')
        assert vault.status() == 'setup'
    finally:
        legacy.close()
