"""Check checked-in corpus with PyMuPDF's object parser and OpenSSL CMS.

Run after generation, or on an untouched checkout. This never writes PDF files.
"""

import hashlib
import json
import subprocess
import tempfile
from pathlib import Path

import pymupdf

ROOT = Path(__file__).resolve().parent
SIGNED = {
    "certificate.pdf",
    "incremental_certificate.pdf",
    "escaped_name_certificate.pdf",
    "perms_certificate.pdf",
    "object_stream_certificate.pdf",
}
ENCRYPTED = {"encrypted.pdf"}
MALFORMED = {"malformed.pdf"}


def der_length(value: bytes) -> int:
    assert value[0] == 0x30, "CMS must be a DER SEQUENCE"
    first = value[1]
    if first < 128:
        return 2 + first
    count = first & 0x7F
    assert 1 <= count <= 4
    return 2 + count + int.from_bytes(value[2 : 2 + count], "big")


def check_one(name: str, recorded: dict) -> None:
    path = ROOT / name
    data = path.read_bytes()
    assert hashlib.sha256(data).hexdigest() == recorded["sha256"], name
    assert len(data) == recorded["bytes"], name
    if name in MALFORMED:
        try:
            pymupdf.open(path)
        except pymupdf.FileDataError:
            return
        raise AssertionError(f"{name}: malformed input unexpectedly opened")
    doc = pymupdf.open(path)
    assert len(doc) == recorded["pymupdf_pages"], name
    assert bool(doc.is_encrypted) == (name in ENCRYPTED), name
    if name in ENCRYPTED:
        return
    widgets = [w for page in doc for w in (page.widgets() or []) if w.field_type_string == "Signature"]
    assert bool(widgets) == (name in SIGNED), name
    if name not in SIGNED:
        assert doc.get_sigflags() == -1, name
        return
    assert len(widgets) == 1 and doc.get_sigflags() == 3, name
    signature_objects = []
    for xref in range(1, doc.xref_length()):
        kind, value = doc.xref_get_key(xref, "ByteRange")
        if kind == "array":
            signature_objects.append((xref, [int(part) for part in value.strip("[]").split()]))
    assert len(signature_objects) == 1, name
    xref, offsets = signature_objects[0]
    first, count, second, tail = offsets
    assert first == 0 and count > 0 and second > count and second + tail == len(data), name
    gap = data[count:second]
    assert gap.startswith(b"<") and gap.endswith(b">") and len(gap) == 16386, name
    cms_padded = bytes.fromhex(gap[1:-1].decode("ascii"))
    cms = cms_padded[: der_length(cms_padded)]
    assert len(cms) < len(cms_padded) and set(cms_padded[len(cms) :]) <= {0}, name
    signed_bytes = data[:count] + data[second:]
    with tempfile.TemporaryDirectory(prefix="md-pdf-verify-") as tmp:
        cms_file = Path(tmp) / "signature.der"
        content_file = Path(tmp) / "covered.bin"
        cms_file.write_bytes(cms)
        content_file.write_bytes(signed_bytes)
        result = subprocess.run(
            ["openssl", "cms", "-verify", "-binary", "-inform", "DER", "-in", str(cms_file), "-content", str(content_file), "-noverify", "-out", str(Path(tmp) / "out")],
            capture_output=True,
            text=True,
        )
        assert result.returncode == 0, f"{name}: {result.stderr}"
    assert doc.xref_get_key(xref, "Type") == ("name", "/Sig"), name


def main() -> None:
    manifest = json.loads((ROOT / "manifest.json").read_text())
    assert len(manifest) == 12
    assert SIGNED | ENCRYPTED | MALFORMED <= set(manifest)
    for name, recorded in manifest.items():
        check_one(name, recorded)
        print(f"PASS {name} {recorded['sha256']}")


if __name__ == "__main__":
    main()
