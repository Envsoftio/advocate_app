import copy

import pytest

from app.vault import (VaultError, create_key_envelope, decrypt_bytes, decrypt_file,
                       encrypt_file, encrypted_sink, rotate_key_envelope, unlock_key)


def test_password_recovery_rotation_and_tamper():
    envelope, recovery, key = create_key_envelope('synthetic password 123')
    assert unlock_key(envelope, 'synthetic password 123') == key
    assert unlock_key(envelope, recovery, recovery=True) == key
    with pytest.raises(VaultError):
        unlock_key(envelope, 'wrong password')
    with pytest.raises(VaultError):
        unlock_key(envelope, '00' * 32, recovery=True)
    changed = copy.deepcopy(envelope)
    changed['password']['wrapped'] = envelope['recovery']['wrapped']
    with pytest.raises(VaultError):
        unlock_key(changed, 'synthetic password 123')
    replacement, new_recovery = rotate_key_envelope(key, 'new synthetic password 123')
    assert unlock_key(replacement, 'new synthetic password 123') == key
    assert unlock_key(replacement, new_recovery, recovery=True) == key
    with pytest.raises(VaultError):
        unlock_key(replacement, recovery, recovery=True)


def test_file_encryption_detects_truncation_and_keeps_prior_output(tmp_path):
    _, _, key = create_key_envelope('synthetic password 123')
    plain = tmp_path / 'original.pdf'
    content = b'%PDF synthetic marker\n' + bytes(range(256)) * 8200
    plain.write_bytes(content)
    encrypted = tmp_path / 'working.cbenc'
    restored = tmp_path / 'restored.pdf'
    encrypt_file(plain, encrypted, key)
    assert content[:20] not in encrypted.read_bytes()
    decrypt_file(encrypted, restored, key)
    assert restored.read_bytes() == content
    restored.write_bytes(b'previous valid output')
    damaged = tmp_path / 'damaged.cbenc'
    damaged.write_bytes(encrypted.read_bytes()[:-10])
    with pytest.raises(VaultError):
        decrypt_file(damaged, restored, key)
    assert restored.read_bytes() == b'previous valid output'
    assert list(tmp_path.glob('.restored.pdf.decrypted-*')) == []
    with pytest.raises(VaultError):
        decrypt_file(encrypted, restored, b'X' * 32)
    assert restored.read_bytes() == b'previous valid output'


def test_vault_rejects_weak_password_and_extra_bytes(tmp_path):
    with pytest.raises(VaultError):
        create_key_envelope('12345')
    envelope, recovery, key = create_key_envelope('123456')
    assert unlock_key(envelope, '123456') == key
    rotated, _ = rotate_key_envelope(key, 'abcdef')
    assert unlock_key(rotated, 'abcdef') == key
    _, _, key = create_key_envelope('synthetic password 123')
    source = tmp_path / 'source'
    source.write_bytes(b'')
    encrypted = tmp_path / 'source.cbenc'
    encrypt_file(source, encrypted, key)
    encrypted.write_bytes(encrypted.read_bytes() + b'junk')
    with pytest.raises(VaultError):
        decrypt_file(encrypted, tmp_path / 'output', key)
    assert not (tmp_path / 'output').exists()


def test_streamed_encryption_publishes_only_complete_output(tmp_path):
    _, _, key = create_key_envelope('synthetic password 123')
    destination = tmp_path / 'backup.cbmatter'
    with encrypted_sink(destination, key) as output:
        output.write(b'synthetic first chunk')
        output.write(b'X' * (1024 * 1024 + 7))
    previous = destination.read_bytes()
    assert decrypt_bytes(destination, key) == b'synthetic first chunk' + b'X' * (1024 * 1024 + 7)
    with pytest.raises(RuntimeError):
        with encrypted_sink(destination, key) as output:
            output.write(b'incomplete replacement')
            raise RuntimeError('synthetic interruption')
    assert destination.read_bytes() == previous
    assert not list(tmp_path.glob('.backup.cbmatter.encrypted-*'))
