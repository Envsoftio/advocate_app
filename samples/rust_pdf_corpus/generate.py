"""Generate synthetic PDF admission probes. Run with backend/.venv312/bin/python.

The checked-in private key is deliberately public test material. Never use it
for a real signature. This corpus tests refusal, not signer trust or validity.
"""

from __future__ import annotations

import base64
import hashlib
import json
import re
import subprocess
from pathlib import Path

import pymupdf
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.serialization import pkcs7


ROOT = Path(__file__).resolve().parent
KEY = serialization.load_pem_private_key((ROOT / "synthetic_key.pem").read_bytes(), None)
CERT = x509.load_pem_x509_certificate((ROOT / "synthetic_cert.pem").read_bytes())
PLACEHOLDER = b"0" * 16384


def base_pdf() -> bytes:
    doc = pymupdf.open()
    page = doc.new_page(width=595, height=842)
    page.insert_text((72, 100), "MatterDraft synthetic unsigned control")
    result = doc.tobytes(garbage=4, deflate=True, no_new_id=True)
    doc.close()
    return result


def append_objects(source: bytes, objects: dict[int, bytes], root: int) -> bytes:
    """Write a genuine incremental xref; preserve the preceding revision."""
    previous = int(re.findall(rb"startxref\s+(\d+)", source)[-1])
    out = bytearray(source)
    offsets = {}
    for number, body in sorted(objects.items()):
        offsets[number] = len(out)
        out.extend(f"\n{number} 0 obj\n".encode() + body + b"\nendobj\n")
    xref = len(out)
    for number, offset in sorted(offsets.items()):
        out.extend(f"xref\n{number} 1\n{offset:010d} 00000 n \n".encode())
    out.extend(
        f"trailer\n<< /Size {max(objects) + 1} /Root {root} 0 R /Prev {previous} >>\nstartxref\n{xref}\n%%EOF\n".encode()
    )
    return bytes(out)


def sign(source: bytes, *, escaped=False, perms=False) -> bytes:
    doc = pymupdf.open(stream=source, filetype="pdf")
    root = doc.pdf_catalog()
    page = doc[0].xref
    old_root = doc.xref_object(root).encode()
    old_page = doc.xref_object(page).encode()
    next_id = doc.xref_length()
    doc.close()
    sig, widget, form = next_id, next_id + 1, next_id + 2
    name = b"/Byte#52ange" if escaped else b"/ByteRange"
    signature = (
        b"<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached "
        + name
        + b" [0000000000 0000000000 0000000000 0000000000] /Contents <"
        + PLACEHOLDER
        + b"> /M (D:20261118000000Z) >>"
    )
    root_body = old_root[:-2].rstrip() + f" /AcroForm {form} 0 R".encode()
    if perms:
        root_body += f" /Perms << /DocMDP {sig} 0 R >>".encode()
    root_body += b" >>"
    page_body = old_page[:-2].rstrip() + f" /Annots [{widget} 0 R] >>".encode()
    widget_body = (
        f"<< /Type /Annot /Subtype /Widget /FT /Sig /T (SyntheticSignature) "
        f"/Rect [72 140 260 175] /P {page} 0 R /V {sig} 0 R >>"
    ).encode()
    data = append_objects(
        source,
        {
            root: root_body,
            page: page_body,
            sig: signature,
            widget: widget_body,
            form: f"<< /SigFlags 3 /Fields [{widget} 0 R] >>".encode(),
        },
        root,
    )
    match = re.search(rb"/Contents <(0{16384})>", data)
    assert match
    start, end = match.span(1)
    range_match = re.search(re.escape(name) + rb" \[(\d{10}) (\d{10}) (\d{10}) (\d{10})\]", data)
    assert range_match
    values = [0, start - 1, end + 1, len(data) - end - 1]
    mutable = bytearray(data)
    for group, value in enumerate(values, 1):
        a, b = range_match.span(group)
        mutable[a:b] = f"{value:010d}".encode()
    signed = bytes(mutable[: start - 1] + mutable[end + 1 :])
    cms = pkcs7.PKCS7SignatureBuilder().set_data(signed).add_signer(CERT, KEY, hashes.SHA256()).sign(
        serialization.Encoding.DER,
        [pkcs7.PKCS7Options.DetachedSignature, pkcs7.PKCS7Options.Binary, pkcs7.PKCS7Options.NoAttributes],
    )
    assert len(cms) * 2 <= len(PLACEHOLDER)
    mutable[start:end] = cms.hex().encode() + b"0" * (len(PLACEHOLDER) - len(cms) * 2)
    return bytes(mutable)


def validate(path: Path) -> dict:
    data = path.read_bytes()
    result = {"sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)}
    try:
        doc = pymupdf.open(path)
        result.update(
            pymupdf_pages=len(doc),
            pymupdf_encrypted=bool(doc.is_encrypted),
            pymupdf_sigflags=doc.get_sigflags(),
        )
        doc.close()
    except Exception as exc:
        result["pymupdf_error"] = type(exc).__name__
    match = re.search(rb"/(?:ByteRange|Byte#52ange) \[(\d+) (\d+) (\d+) (\d+)\] /Contents <([0-9a-f]+)>", data)
    if not match:
        return result
    offsets = [int(match.group(i)) for i in range(1, 5)]
    cms_hex = match.group(5)
    cms = bytes.fromhex(cms_hex.decode()).rstrip(b"\0")
    signed = data[offsets[0] : offsets[0] + offsets[1]] + data[offsets[2] : offsets[2] + offsets[3]]
    cms_path = ROOT / ".verify.cms"
    data_path = ROOT / ".verify.data"
    cms_path.write_bytes(cms)
    data_path.write_bytes(signed)
    proc = subprocess.run(
        ["openssl", "cms", "-verify", "-binary", "-inform", "DER", "-in", str(cms_path), "-content", str(data_path), "-noverify", "-out", "/dev/null"],
        capture_output=True,
        text=True,
    )
    result["openssl_cms_verify"] = proc.returncode == 0
    result["openssl_message"] = proc.stderr.strip().splitlines()[-1] if proc.stderr else ""
    cms_path.unlink()
    data_path.unlink()
    return result


def main() -> None:
    unsigned = base_pdf()
    files = {"unsigned.pdf": unsigned}
    files["certificate.pdf"] = sign(unsigned)
    files["incremental_certificate.pdf"] = sign(append_objects(unsigned, {99: b"<< /SyntheticRevision true >>"}, 1))
    files["escaped_name_certificate.pdf"] = sign(unsigned, escaped=True)
    files["perms_certificate.pdf"] = sign(unsigned, perms=True)
    files["unsigned_acroform.pdf"] = append_objects(
        unsigned, {1: b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [] >> >>"}, 1
    )
    files["unsigned_perms.pdf"] = append_objects(
        unsigned, {1: b"<< /Type /Catalog /Pages 2 0 R /Perms << >> >>"}, 1
    )
    # PyMuPDF encryption uses fresh randomness. Keep one synthetic AES-256 output
    # as a fixed seed so regeneration cannot silently change corpus hashes.
    files["encrypted.pdf"] = base64.b64decode((ROOT / "encrypted_seed.b64").read_bytes())
    doc = pymupdf.open(stream=unsigned, filetype="pdf")
    doc[0].insert_text((72, 160), "SIGNED BY HAND - visible mark only")
    files["visible_mark_only.pdf"] = doc.tobytes(no_new_id=True)
    doc.close()
    files["malformed.pdf"] = unsigned[: len(unsigned) // 2]
    doc = pymupdf.open(stream=unsigned, filetype="pdf")
    files["object_stream_unsigned.pdf"] = doc.tobytes(use_objstms=1, no_new_id=True)
    doc.close()
    files["object_stream_certificate.pdf"] = sign(files["object_stream_unsigned.pdf"])
    manifest = {}
    for name, data in files.items():
        path = ROOT / name
        path.write_bytes(data)
        manifest[name] = validate(path)
    (ROOT / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
