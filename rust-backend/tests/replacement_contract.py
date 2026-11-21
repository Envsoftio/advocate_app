"""Exercise replacement features with synthetic PDFs and independent PyMuPDF inspection."""
from base64 import b64decode, b64encode
from datetime import datetime
from hashlib import sha256
from io import BytesIO
import json
import os
import re
from pathlib import Path
from shutil import copyfile
import sqlite3
from struct import pack, pack_into
import subprocess
import tempfile
from time import monotonic, sleep
from unittest.mock import patch
from zipfile import ZIP_DEFLATED, ZipFile
from zoneinfo import ZoneInfo
from zlib import compress

import pymupdf
from fastapi.routing import APIRoute
from PIL import Image, ImageDraw

from vault_contract import PASSWORD, ROOT, TestClient, create_app, running_vault
from app.pdf_ops import permanently_redact_region
from python_route_inventory import rust_statuses

FONT = ROOT / "rust-backend/assets/fonts/NotoSansDevanagari-Regular.ttf"


def pdf_bytes(rust, document, revision, source="working"):
    body = {"kind": source}
    if source == "working":
        body["revision"] = revision
    return bytes(rust("SNAPSHOT", f"/documents/{document}", body)["bytes"])


def pdf_image_profile(pdf, xref):
    kind, value = pdf.xref_get_key(xref, "ColorSpace")
    if kind == "xref":
        value = pdf.xref_object(int(value.split()[0]))
    profile = re.search(r"/ICCBased\s+(\d+)\s+0\s+R", value)
    return pdf.xref_stream(int(profile[1])) if profile else None


def image_import(rust, matter, root):
    image = Image.new("RGB", (120, 72), (27, 63, 111))
    image.paste((209, 52, 74), (0, 0, 54, 33))
    image.paste((44, 173, 95), (65, 40, 120, 72))
    exif = Image.Exif()
    exif[274] = 6
    exif[282] = 300
    exif[283] = 144
    exif[296] = 2
    resource = pack(">IHHIHH", 144 << 16, 1, 1, 96 << 16, 1, 1)
    photoshop = b"Photoshop 3.0\x00" + b"8BIM\x03\xed\x00\x00" + pack(">I", len(resource)) + resource
    app13 = b"\xff\xed" + pack(">H", len(photoshop) + 2) + photoshop
    cases = [
        ("plain.png", {}, b""),
        ("density.png", {"dpi": (300, 144)}, b""),
        ("density.jpg", {"dpi": (300, 144)}, b""),
        ("rotated.jpg", {"exif": exif}, b""),
        ("photoshop-density.jpg", {"dpi": (300, 144)}, app13),
        ("photoshop-before-exif.jpg", {"exif": exif, "dpi": (300, 144)}, app13),
    ]
    for name, options, prefix in cases:
        path = root / name
        image.save(path, **options)
        if prefix:
            data = path.read_bytes()
            assert data[:2] == b"\xff\xd8"
            path.write_bytes(data[:2] + prefix + data[2:])
        doc = rust("IMPORT", matter, source=str(path), status=201)[0]["id"]
        with pymupdf.open(path) as baseline, pymupdf.open(stream=baseline.convert_to_pdf()) as reference:
            with pymupdf.open(stream=pdf_bytes(rust, doc, 1)) as output:
                assert len(output) == 1
                assert tuple(output[0].rect) == tuple(reference[0].rect), name
                assert output[0].get_pixmap().samples == reference[0].get_pixmap().samples, name
        assert pdf_bytes(rust, doc, 1, "original") == path.read_bytes()


def image_upload_refusals(rust, matter, root):
    fixture = root / "image-upload-refusals.pdf"
    with pymupdf.open() as pdf:
        pdf.new_page(width=240, height=320)
        pdf.save(fixture)
    original = fixture.read_bytes()
    native = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
    native_route = f"/documents/{native}"
    native_working = pdf_bytes(rust, native, 1)
    native_revisions = rust("GET", native_route + "/revisions")
    native_annotations = rust("GET", f"/matters/{matter}/annotations")
    image = BytesIO()
    Image.new("RGBA", (8, 6), (81, 123, 167, 192)).save(image, format="PNG")
    png = image.getvalue()
    fields = dict(page_index=0, expected_revision=1, rect_x=.2, rect_y=.2,
                  rect_width=.6, rect_height=.6)
    auth = {"Authorization": "Bearer synthetic-image-refusals"}
    with TestClient(create_app(root / "image-upload-reference", token="synthetic-image-refusals",
                               require_vault=False)) as python:
        def read(path):
            response = python.get(path, headers=auth)
            response.raise_for_status()
            return response.json()

        created = python.post("/matters", headers=auth, json={"name": "Synthetic image refusals"})
        created.raise_for_status()
        reference_matter = created.json()["id"]
        imported = python.post(f"/matters/{reference_matter}/documents/raw", content=original,
                               headers={**auth, "Content-Type": "application/octet-stream",
                                        "X-CourtBundle-Filename": fixture.name})
        imported.raise_for_status()
        reference_route = f"/documents/{imported.json()[0]['id']}"
        reference_annotations = read(f"/matters/{reference_matter}/annotations")
        reference_revisions = read(reference_route + "/revisions")

        for suffix, size_detail, stale_detail in (
            ("/pages/image-overlay", "Image exceeds 10 MiB", "Document changed; place the image again"),
            ("/annotations/image", "Image overlay exceeds 10 MiB",
             "Document changed; reload and place the image again"),
        ):
            cases = [
                ("hidden.png.txt", png, {}, 422, "Image overlays support PNG and JPEG only"),
                (".png", png, {}, 422, "Image overlays support PNG and JPEG only"),
                ("stamp.JPG", png, {}, 422, "Overlay image bytes do not match the PNG/JPEG filename"),
                ("broken.png", png[:10], {}, 422, None),
                ("oversized.png", bytes(10 * 1024 * 1024 + 1), {}, 413, size_detail),
                ("stamp.PNG", png, {"expected_revision": 2}, 409, stale_detail),
            ]
            if suffix == "/pages/image-overlay":
                cases.extend([
                    ("stamp.JPG", png, {"expected_revision": 2}, 422,
                     "Overlay image bytes do not match the PNG/JPEG filename"),
                    ("stamp.png", png, {"page_index": -1}, 422, "Invalid page number"),
                    ("stamp.png", png, {"page_index": 1}, 422, "Invalid page number"),
                    ("stamp.png", png, {"rect_width": .9}, 422, "Image placement must be inside the page"),
                ])
            else:
                cases.append(("stamp.JPG", png, {"expected_revision": 2}, 409, stale_detail))
            for filename, data, request, status, detail in cases:
                inputs = {**fields, **request}
                if suffix == "/pages/image-overlay":
                    inputs["page_index"] = str(inputs["page_index"])
                reference = python.post(reference_route + suffix, headers=auth, data=inputs,
                                        files={"file": (filename, data, "application/octet-stream")})
                actual = rust("POST", native_route + suffix,
                              dict(inputs, filename=filename, data=b64encode(data).decode()), status=None)
                assert reference.status_code == actual["status"] == status, (suffix, filename, actual)
                if detail is None:
                    assert reference.json()["detail"].startswith("Cannot read overlay image: ")
                    assert actual["body"]["detail"].startswith("Cannot read overlay image: ")
                else:
                    assert reference.json() == actual["body"] == {"detail": detail}, (suffix, filename)
        assert read(reference_route + "/revisions") == reference_revisions
        assert read(f"/matters/{reference_matter}/annotations") == reference_annotations

    assert rust("GET", native_route + "/revisions") == native_revisions
    assert rust("GET", f"/matters/{matter}/annotations") == native_annotations
    assert pdf_bytes(rust, native, 1) == native_working
    assert pdf_bytes(rust, native, 1, "original") == original


def annotation_layer_order(rust, matter, root):
    from app.engine import apply_page_annotations

    fixture = root / "annotation-layer-order.pdf"
    with pymupdf.open() as pdf:
        pdf.new_page(width=200, height=200)
        pdf.save(fixture)
    stamp = root / "annotation-layer-order.png"
    Image.new("RGB", (32, 32), (32, 211, 80)).save(stamp)
    original = fixture.read_bytes()
    doc = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
    route = f"/documents/{doc}"
    fields = dict(page_index=0, expected_revision=1, rect_x=.2, rect_y=.2,
                  rect_width=.6, rect_height=.6)
    line = dict(fields, kind="line", color="blue", output_scope="pdf")
    rust("POST", route + "/annotations", line, status=201)
    uploaded = rust("POST", route + "/annotations/image",
                    dict(fields, filename=stamp.name, data=b64encode(stamp.read_bytes()).decode()),
                    status=201)
    image, = (item for item in uploaded if item["document_id"] == doc and item["kind"] == "image")
    downloaded = rust("GET", f"/annotations/{image['id']}/asset")
    assert b64decode(downloaded["data"]) == stamp.read_bytes()

    with pymupdf.open(stream=original, filetype="pdf") as expected:
        apply_page_annotations(expected, [dict(line, document_id=doc, resolved=True),
            dict(fields, document_id=doc, kind="image", output_scope="pdf",
                 resolved=True, _asset_path=str(stamp))], {doc: 0})
        ref = expected[0].get_pixmap()
    with pymupdf.open(stream=b64decode(rust("GET", route + "/pdf?editable=true&revision=1")["data"]),
                       filetype="pdf") as output:
        actual = output[0].get_pixmap()
        for x, y in [(99, 100), (55, 55)]:
            assert all(abs(a - b) <= 2 for a, b in zip(ref.pixel(x, y), actual.pixel(x, y), strict=True))
        assert actual.pixel(99, 100)[2] > actual.pixel(99, 100)[1]
        assert actual.pixel(55, 55) == (32, 211, 80)
    assert pdf_bytes(rust, doc, 1, "original") == original


def rotated_image_overlays(rust, matter, root):
    import pypdfium2
    from PIL import ImageChops, ImageCms
    from app.pdf_ops import insert_working_image
    from app.print_export import print_copy

    placement = (.28, .32, .4, .25)
    fields = dict(zip(("rect_x", "rect_y", "rect_width", "rect_height"), placement, strict=True))
    for rotation in (0, 90, 180, 270):
        for kind in ("opaque", "alpha", "jpeg", "profile", "gray", "exif", "progressive"):
            alpha = kind == "alpha"
            jpeg = kind not in ("opaque", "alpha")
            case = (rotation, kind)
            fixture = root / f"image-overlay-{rotation}-{kind}.pdf"
            with pymupdf.open() as pdf:
                page = pdf.new_page(width=580, height=620)
                page.draw_rect(pymupdf.Rect(15, 20, 540, 580), color=None, fill=(.92, .90, .70))
                page.insert_text((95, 100), "Synthetic page heading", fontsize=15)
                page.set_cropbox(pymupdf.Rect(29, 31, 541, 572))
                page.set_rotation(rotation)
                pdf.new_page(width=440, height=560).insert_text((50, 80), "Unrelated page")
                pdf.save(fixture)
            image = Image.new("RGBA", (80, 32), (219, 29, 31, 255))
            ink = ImageDraw.Draw(image)
            ink.rectangle((20, 0, 44, 31), fill=(12, 141, 178, 127 if alpha else 255))
            ink.rectangle((53, 15, 78, 30), fill=(239, 245, 246, 0 if alpha else 255))
            payload = BytesIO()
            options = {}
            if kind == "profile":
                options["icc_profile"] = ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB")).tobytes()
            if kind == "exif":
                options["exif"] = Image.Exif()
                options["exif"][274] = 6
            if kind == "progressive":
                options["progressive"] = True
            mode = "L" if kind == "gray" else "RGB"
            (image if alpha else image.convert(mode)).save(payload, format="JPEG" if jpeg else "PNG", **options)
            original = fixture.read_bytes()
            reference = root / f"overlay-reference-{rotation}-{kind}.pdf"
            reference.write_bytes(original)
            insert_working_image(reference, 0, payload.getvalue(), "jpeg" if jpeg else "png", *placement)
            reference_data = reference.read_bytes()
            doc = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
            route = f"/documents/{doc}"
            upload = {**fields, "filename": "synthetic.jpg" if jpeg else "synthetic.png",
                      "data": b64encode(payload.getvalue()).decode(),
                      "expected_revision": 1, "page_index": 0}

            def identical(left, right):
                with pypdfium2.PdfDocument(left) as python, pypdfium2.PdfDocument(right) as native:
                    for expected, actual in zip(python, native, strict=True):
                        a, b = (page.render(scale=2).to_pil().convert("RGB") for page in (expected, actual))
                        assert ImageChops.difference(a, b).getbbox() is None, case

            def exact_jpeg(data):
                if not jpeg:
                    return
                with pymupdf.open(stream=data) as pdf:
                    item, = pdf[0].get_images(full=True)
                    assert pdf.xref_get_key(item[0], "Filter") == ("name", "/DCTDecode")
                    assert pdf.xref_stream_raw(item[0]) == payload.getvalue()

            mark, = (mark for mark in rust("POST", route + "/annotations/image", upload, status=201)
                     if mark["document_id"] == doc and mark["kind"] == "image")
            assert mark["output_scope"] == "pdf" and mark["resolved"]
            rust("POST", route + "/annotations", {**fields, "page_index": 0, "expected_revision": 1,
                 "kind": "comment", "body": "Synthetic private note"}, status=201)
            copy = rust("GET", route + "/pdf?editable=true&revision=1")
            identical(reference_data, b64decode(copy["data"]))
            exact_jpeg(b64decode(copy["data"]))
            assert pdf_bytes(rust, doc, 1) == original
            if alpha:
                printable = rust("POST", route + "/print", {"expected_revision": 1, "pages": "all",
                                 "orientation": "source", "scale": "actual"})
                identical(print_copy(reference_data, "all", "source", "actual"), b64decode(printable["data"]))
            result = rust("POST", route + "/pages/image-overlay", upload)
            assert result["revision"] == 2
            working = pdf_bytes(rust, doc, 2)
            identical(reference_data, working)
            exact_jpeg(working)
            rust("POST", route + "/pages/image-overlay", upload, status=409)
            assert pdf_bytes(rust, doc, 2) == working
            assert pdf_bytes(rust, doc, 1, "original") == original


def placement_on_physical_pages(rust, root):
    import pypdfium2
    from PIL import ImageChops
    from app.editable_marks import draw_pdf_mark
    from app.engine import apply_page_annotations, stamp_page_number
    from app.pdf_ops import ProcessingError, add_text_overlay, apply_output_marks, insert_working_image, preview_output_marks

    matter = rust("POST", "/matters", {"name": "Synthetic physical page placement"}, status=201)["id"]
    cases = [(rotation, unit) for rotation in (0, 90, 180, 270) for unit in (1, 2)]
    source = root / "placement-physical-pages.pdf"
    with pymupdf.open() as pdf:
        for rotation, unit in cases:
            for _ in range(2):
                page = pdf.new_page(width=200, height=180)
                pdf.xref_set_key(page.xref, "CropBox", "[-20 -10 160 120]")
                pdf.xref_set_key(page.xref, "UserUnit", str(unit))
                page.set_rotation(rotation)
        pdf.save(source)
    original = source.read_bytes()
    working_reference = root / "placement-physical-python-working.pdf"
    working_reference.write_bytes(original)
    stamp = root / "placement-physical-stamp.png"
    image = Image.new("RGB", (73, 31), "red")
    ImageDraw.Draw(image).rectangle((0, 0, 34, 11), fill="blue")
    image.save(stamp)
    stamp_bytes = stamp.read_bytes()
    placement = (.28, .23, .37, .31)
    fractions = dict(zip(("rect_x", "rect_y", "rect_width", "rect_height"), placement, strict=True))
    document = rust("IMPORT", matter, source=str(source), status=201)[0]["id"]
    route = f"/documents/{document}"
    image_marks = []
    revision = 1
    for index, (rotation, unit) in enumerate(cases):
        text_page, image_page = 2 * index, 2 * index + 1
        fields = {"page_index": text_page, "x": .22, "y": .22, "text": "TEST", "font_size": 12}
        if (rotation, unit) == (0, 1):
            edge = dict(fields, x=.8)
            try:
                add_text_overlay(working_reference, **edge)
            except ProcessingError as error:
                refused = rust("POST", route + "/pages/text-overlay",
                               dict(edge, expected_revision=revision), status=422)
                assert refused["detail"] == str(error)
            else:
                raise AssertionError("text placement used the off-sheet portion of the CropBox")
            assert pdf_bytes(rust, document, revision) == original
        add_text_overlay(working_reference, **fields)
        result = rust("POST", route + "/pages/text-overlay", dict(fields, expected_revision=revision))
        revision += 1
        assert result["revision"] == revision
        insert_working_image(working_reference, image_page, stamp_bytes, "png", *placement)
        upload = dict(fractions, filename=stamp.name, data=b64encode(stamp_bytes).decode(),
                      page_index=image_page, expected_revision=revision)
        result = rust("POST", route + "/pages/image-overlay", upload)
        revision += 1
        assert result["revision"] == revision
        image_marks.append(dict(fractions, document_id=document, page_index=image_page, kind="image",
                                resolved=True, _asset_path=stamp))

    def compare_images(expected, actual):
        with pymupdf.open(stream=expected) as baseline, pymupdf.open(stream=actual) as native:
            for index, case in enumerate(cases):
                page = 2 * index + 1
                a, = baseline[page].get_image_info()
                b, = native[page].get_image_info()
                assert a["width"] == b["width"] and a["height"] == b["height"], case
                assert max(abs(x - y) for x, y in zip(a["bbox"], b["bbox"], strict=True)) < .0003, case
        with pypdfium2.PdfDocument(expected) as baseline, pypdfium2.PdfDocument(actual) as native:
            for index, case in enumerate(cases):
                page = 2 * index + 1
                a, b = (doc[page].render(scale=2).to_pil().convert("RGB") for doc in (baseline, native))
                assert ImageChops.difference(a, b).getbbox() is None, case

    def compare_text(expected, actual, case, *, omit=()):
        def traces(page):
            return [trace for trace in page.get_texttrace()
                    if "".join(chr(char[0]) for char in trace["chars"]) not in omit]

        for a, b in zip(traces(expected), traces(actual), strict=True):
            assert abs(a["size"] - b["size"]) < .0003 and a["dir"] == b["dir"], case
            for (ca, _, origin_a, _), (cb, _, origin_b, _) in zip(a["chars"], b["chars"], strict=True):
                assert ca == cb, case
                assert max(abs(x - y) for x, y in zip(origin_a, origin_b, strict=True)) < .0003, case

    expected = working_reference.read_bytes()
    actual = pdf_bytes(rust, document, revision)
    compare_images(expected, actual)
    with pymupdf.open(stream=expected) as baseline, pymupdf.open(stream=actual) as native:
        for index, case in enumerate(cases):
            compare_text(baseline[2 * index], native[2 * index], case)
    assert pdf_bytes(rust, document, 1, "original") == original

    editable = rust("IMPORT", matter, source=str(source), included=True, status=201)[0]["id"]
    route = f"/documents/{editable}"
    style = {"font_family": "helv", "font_size": 12, "bold": False, "alignment": "left"}
    line = {"kind": "line", "rect_x": .23, "rect_y": .2, "rect_width": .33, "rect_height": .2, "color": "blue"}
    text = {**line, "kind": "text", "body": "TEST", "rect_width": .45, "rect_height": .37}
    for mark in image_marks:
        upload = dict(fractions, filename=stamp.name, data=b64encode(stamp_bytes).decode(),
                      page_index=mark["page_index"], expected_revision=1)
        rust("POST", route + "/annotations/image", upload, status=201)
        for editable_mark in (line, dict(text, text_style=style)):
            rust("POST", route + "/annotations", dict(editable_mark, page_index=mark["page_index"] - 1,
                                                        expected_revision=1, output_scope="pdf"), status=201)
    with pymupdf.open(stream=original) as reference:
        assert apply_page_annotations(reference, image_marks, {document: 0})["image_overlays_inserted"] == len(cases)
        for index in range(len(cases)):
            draw_pdf_mark(reference[2 * index], line)
            draw_pdf_mark(reference[2 * index], dict(text, text_style_json=json.dumps(style)))
        expected = reference.tobytes()
    actual = b64decode(rust("GET", route + "/pdf?editable=true&revision=1")["data"])
    compare_images(expected, actual)
    with pymupdf.open(stream=expected) as baseline, pymupdf.open(stream=actual) as native:
        for index, case in enumerate(cases):
            left, right = baseline[2 * index], native[2 * index]
            compare_text(left, right, case)
            a, = left.get_drawings()
            b, = right.get_drawings()
            assert abs(a["width"] - b["width"]) < .0003, case
            assert max(abs(x - y) for x, y in zip(a["rect"], b["rect"], strict=True)) < .0003, case
    assert pdf_bytes(rust, editable, 1) == original

    settings = rust("GET", f"/matters/{matter}/bundle-settings")
    marks = {**settings["output_marks"], "watermark_enabled": True, "watermark_text": "DRAFT",
             "watermark_font_size": 9, "watermark_position": "top_left", "numbering_back_link": False,
             "numbering_style": "arabic",
             "processing_quality": "lossless"}
    for index, case in enumerate(cases):
        page = 2 * index
        a, = preview_output_marks(source, page, marks)
        b, = rust("POST", f"{route}/pages/{page}/output-marks-preview", marks)["marks"]
        assert a["kind"] == b["kind"] == "watermark" and a["overlaps_content"] == b["overlaps_content"], case
        assert max(abs(x - y) for x, y in zip(a["relative_rect"], b["relative_rect"], strict=True)) < .0001, case

    link_source = root / "output-mark-link-obstacles.pdf"
    with pymupdf.open() as fixture:
        for kind in ("note", "active", "inert"):
            page = fixture.new_page(width=300, height=230)
            if kind == "note":
                page.add_text_annot((15, 18), "Synthetic note")
            else:
                page.insert_link({"kind": pymupdf.LINK_URI, "from": pymupdf.Rect(13, 15, 43, 27),
                                  "uri": "https://example.invalid/synthetic"})
                if kind == "inert":
                    page = fixture.reload_page(page)
                    fixture.xref_set_key(page.get_links()[0]["xref"], "A", "null")
        fixture.save(link_source)
    linked = rust("IMPORT", matter, source=str(link_source), status=201)[0]["id"]
    for page, overlaps in enumerate((True, True, False)):
        baseline, = preview_output_marks(link_source, page, marks)
        native, = rust("POST", f"/documents/{linked}/pages/{page}/output-marks-preview", marks)["marks"]
        assert baseline["overlaps_content"] == native["overlaps_content"] == overlaps

    uri = "https://example.invalid/physical-sheet"
    links = [{"id": f"{page + 1:032x}", "kind": "external", "title": "Synthetic reviewed link", "uri": uri,
              "source_document_id": editable, "source_revision": 1, "source_page_index": page,
              **fractions} for page in range(2 * len(cases))]
    assert all(link["resolved"] for link in rust("PUT", f"/matters/{matter}/links", {"entries": links}))

    # Open the actual generated volume and check numbering and output stamps
    # against a page-only reference clone at the deterministic destinations.
    rust("PUT", f"/matters/{matter}/bundle-settings", {
        "max_pages_per_volume": 200, "keep_groups_intact": True, "output_marks": marks,
        "expected_snapshot": rust("GET", f"/matters/{matter}/bundle-settings")["input_snapshot"],
    })
    build = rust("POST", f"/matters/{matter}/build", {}, status=202)
    rust("RUN_PENDING", "")
    completed = rust("GET", f"/jobs/{build['id']}")
    assert completed["status"] == "completed", completed
    result = completed["result"]
    exported = f"/matters/{matter}/exports/{result['id']}"
    checked = 0
    for filename, contract in result["validation_contract"].items():
        if not filename.startswith("volume-"):
            continue
        volume_pdf = b64decode(rust("GET", f"{exported}/inspection/{filename}")["data"])
        with pymupdf.open(stream=volume_pdf) as native, pymupdf.open(stream=expected) as baseline:
            for destination, source_page in contract["sources"].items():
                if source_page["document_id"] != editable:
                    continue
                destination = int(destination)
                sheet = source_page["page_index"]
                label = contract["printed_labels"][destination]
                stamp_page_number(baseline[sheet], {"printed_label": label}, marks)
                apply_output_marks(baseline[sheet], marks)
                case = cases[sheet // 2]
                compare_text(baseline[sheet], native[destination], case, omit=() if case == (0, 1) else (label,))
                output_page = native[destination]
                assert label and label in output_page.get_text().split()
                reviewed, = output_page.get_links()
                assert reviewed["uri"] == uri
                width, height = output_page.rect.width, output_page.rect.height
                x, y, w, h = placement
                region = (x * width, y * height, (x + w) * width, (y + h) * height)
                assert max(abs(a - b) for a, b in zip(region, reviewed["from"], strict=True)) < .001, case
                checked += 1
    assert checked == 2 * len(cases)


def textbox_placement(rust, matter, root):
    import pypdfium2
    from PIL import ImageChops
    from app.editable_marks import draw_pdf_mark
    from app.pdf_ops import add_text_overlay, ProcessingError
    from app.print_export import print_copy

    counts = {"exact": 0, "rounded": 0, "changed": 0}

    def compare(expected, actual, label, rounding=False):
        # Same visible text and baselines, including empty-line advances.
        def positions(data):
            with pymupdf.open(stream=data) as pdf:
                return [(span["text"], span["origin"]) for block in pdf[0].get_text("dict")["blocks"]
                        for line in block.get("lines", []) for span in line["spans"]]

        for (a, xy), (b, uv) in zip(positions(expected), positions(actual), strict=True):
            assert a == b and max(abs(x - y) for x, y in zip(xy, uv)) <= .0002, (label, a, b, xy, uv)
        with pypdfium2.PdfDocument(expected) as left, pypdfium2.PdfDocument(actual) as right:
            a, b = (doc[0].render(scale=2).to_pil().convert("RGB") for doc in (left, right))
            diff = ImageChops.difference(a, b)
            if diff.getbbox() is None:
                counts["exact"] += 1
                return
            red, green, blue = diff.split()
            changed = diff.width * diff.height - ImageChops.lighter(
                ImageChops.lighter(red, green), blue).histogram()[0]
            assert rounding and changed <= 256, (label, diff.getbbox(), changed)
            counts["rounded"] += 1
            counts["changed"] = max(counts["changed"], changed)

    text = "  FIRST  HERE  \nOne more longer line longer line to test\n\nNEXT"
    for rotation in (0, 90, 180, 270):
        fixture = root / f"textboxes-{rotation}.pdf"
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=580, height=620)
            page.draw_rect((30, 40, 480, 500), color=(.2, .5, .7))
            page.set_cropbox((31, 27, 529, 573))
            page.set_rotation(rotation)
            pdf.save(fixture)
        source = fixture.read_bytes()
        working_reference = root / f"reference-text-overlay-{rotation}.pdf"
        working_reference.write_bytes(source)
        document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
        route = f"/documents/{document}"
        working = {"page_index": 0, "x": .22, "y": .27,
                   "text": "  FIRST  HERE  \n\n  NEXT ", "font_size": 17.25}
        for revision, y in ((2, .27), (3, .54)):
            working["y"] = y
            add_text_overlay(working_reference, **working)
            assert rust("POST", route + "/pages/text-overlay", working)["revision"] == revision
            compare(working_reference.read_bytes(), pdf_bytes(rust, document, revision),
                    (rotation, "working", revision))
        assert pdf_bytes(rust, document, 1, "original") == source

        area = dict(rect_x=.22, rect_y=.27, rect_width=.28, rect_height=.43)
        styles = [("helv", False, "left"), ("helv", True, "right"),
                  ("tiro", False, "justify"), ("tiro", True, "center"),
                  ("cour", False, "right"), ("cour", True, "justify")]
        for family, bold, align in styles:
            style = dict(font_family=family, bold=bold, alignment=align, font_size=17.25)
            mark = dict(area, kind="text", body=text, page_index=0, color="blue",
                        expected_revision=1, output_scope="pdf", text_style=style)
            document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
            route = f"/documents/{document}"
            rust("POST", route + "/annotations", mark, status=201)
            with pymupdf.open(fixture) as expected:
                draw_pdf_mark(expected[0], dict(mark, text_style_json=json.dumps(style)))
                reference = expected.tobytes()
            editable = b64decode(rust("GET", route + "/pdf?editable=true&revision=1")["data"])
            label = (rotation, family, bold, align)
            compare(reference, editable, label,
                    rounding=family == "cour" and not bold and rotation in (0, 270))
            if family == "helv" and not bold:
                printable = rust("POST", route + "/print", {"expected_revision": 1, "pages": "all",
                                 "orientation": "source", "scale": "actual"})
                compare(print_copy(reference, "all", "source", "actual"),
                        b64decode(printable["data"]), (rotation, "print"))
            assert pdf_bytes(rust, document, 1) == source

    fixture = root / "textbox-obstacles.pdf"
    image = BytesIO()
    Image.new("RGB", (24, 16), (125, 25, 175)).save(image, format="PNG")
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=580, height=620)
        page.insert_text((100, 125), "Earlier words", fontsize=16)
        page.insert_text((100, 216), "Invisible text", fontsize=16, render_mode=3)
        page.insert_text((100, 250), "Outlined text", fontsize=16, render_mode=1)
        page.insert_image((102, 270, 165, 300), stream=image.getvalue())
        pdf.save(fixture)
    source = fixture.read_bytes()
    reserved = (.18, .55, .15, .06)
    cases = [(.19, .181, "O", "page content"), (.19, .33, "O", None),
             (.19, .39, "O", "page content"), (.19, .43, "O", "page content"),
             (.19, .51, "FIRST\n\nLAST", "page component"),
             (.95, .95, "Outside page", "does not fit on the page"),
             (.16, .183, "l", None)]
    for x, y, label, error in cases:
        document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
        route = f"/documents/{document}"
        region = dict(zip(("rect_x", "rect_y", "rect_width", "rect_height"), reserved, strict=True))
        upload = dict(region, filename="obstacle.png", data=b64encode(image.getvalue()).decode(),
                      expected_revision=1, page_index=0)
        rust("POST", route + "/annotations/image", upload, status=201)
        args = dict(page_index=0, x=x, y=y, font_size=13, text=label)
        working_reference.write_bytes(source)
        if error:
            try:
                add_text_overlay(working_reference, **args, reserved_rects=[reserved])
            except ProcessingError as exc:
                assert error in str(exc)
                rejected = rust("POST", route + "/pages/text-overlay", args, status=None)
                assert rejected == {"status": 422, "body": {"detail": str(exc)}}
            else:
                raise AssertionError((label, "Python accepted the rejected placement"))
            assert pdf_bytes(rust, document, 1) == source
            assert len(rust("GET", route + "/revisions")) == 1
        else:
            add_text_overlay(working_reference, **args, reserved_rects=[reserved])
            assert rust("POST", route + "/pages/text-overlay", args)["revision"] == 2
            compare(working_reference.read_bytes(), pdf_bytes(rust, document, 2), label)
        assert pdf_bytes(rust, document, 1, "original") == source
    print(f"Textboxes matched {counts['exact']} rasters exactly and {counts['rounded']} within "
          f"glyph-edge rounding ({counts['changed']} changed pixels at most)")


def working_text_collisions(rust, matter, root):
    import pypdfium2
    from PIL import ImageChops
    from app.pdf_ops import add_text_overlay, ProcessingError

    reference = root / "working-text-collision-reference.pdf"
    counts = {"placed": 0, "rejected": 0}
    image = BytesIO()
    Image.new("RGB", (24, 16), (125, 25, 175)).save(image, format="PNG")

    def form_streams(data):
        with pymupdf.open(stream=data) as pdf:
            return sorted((name, pdf.xref_stream(xref))
                          for xref, name, _, _ in pdf[0].get_xobjects())

    def source_text_resources(data):
        with pymupdf.open(stream=data) as pdf:
            contents = [pdf.xref_stream(xref) for xref in pdf[0].get_contents()]
            font_maps = []
            for xref, _, _, _, name, _, _ in pdf[0].get_fonts(full=True):
                for key in ("Encoding", "ToUnicode"):
                    kind, value = pdf.xref_get_key(xref, key)
                    if kind == "xref":
                        reference = int(value.split()[0])
                        if pdf.xref_is_stream(reference):
                            font_maps.append((name, key, pdf.xref_stream(reference)))
            return contents, sorted(font_maps)

    def check(source, args, rejection=None, reserved=None):
        original = source.read_bytes()
        reference.write_bytes(original)
        document = rust("IMPORT", matter, source=str(source), status=201)[0]["id"]
        route = f"/documents/{document}"
        if reserved:
            region = dict(zip(("rect_x", "rect_y", "rect_width", "rect_height"), reserved, strict=True))
            upload = dict(region, filename="collision.png", data=b64encode(image.getvalue()).decode(),
                          expected_revision=1, page_index=0)
            rust("POST", route + "/annotations/image", upload, status=201)

        try:
            add_text_overlay(reference, **args, reserved_rects=[reserved] if reserved else None)
        except ProcessingError as exc:
            assert rejection and rejection in str(exc), (source.name, args, str(exc))
            reply = rust("POST", route + "/pages/text-overlay", args, status=None)
            assert reply == {"status": 422, "body": {"detail": str(exc)}}, (source.name, args, reply)
            assert pdf_bytes(rust, document, 1) == original
            assert len(rust("GET", route + "/revisions")) == 1
            counts["rejected"] += 1
        else:
            assert not rejection, (source.name, args, rejection)
            assert rust("POST", route + "/pages/text-overlay", args)["revision"] == 2
            edited = pdf_bytes(rust, document, 2)
            with pypdfium2.PdfDocument(reference.read_bytes()) as left, pypdfium2.PdfDocument(edited) as right:
                a, b = (pdf[0].render(scale=2).to_pil().convert("RGB") for pdf in (left, right))
                assert ImageChops.difference(a, b).getbbox() is None, (source.name, args)
            assert form_streams(edited) == form_streams(original)
            if source.name.startswith("type3-"):
                assert type3_program_streams(edited) == type3_program_streams(original), source.name
            if "-cff-" in source.name:
                assert cff_program_streams(edited) == cff_program_streams(original), source.name
            if source.name.startswith("source-paint-"):
                before, before_maps = source_text_resources(original)
                after, after_maps = source_text_resources(edited)
                assert any(window == before for window in
                           (after[i:i + len(before)] for i in range(len(after) - len(before) + 1))), source.name
                assert before_maps == after_maps, source.name
            counts["placed"] += 1
        assert pdf_bytes(rust, document, 1, "original") == original

    for rotation in (0, 90, 180, 270):
        source = root / f"nested-text-visibility-{rotation}.pdf"
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=580, height=620)
            font = page.insert_font("helv")
            page.draw_rect((42, 42, 508, 552))

            def form(dictionary, commands):
                ref = pdf.get_new_xref()
                pdf.update_object(ref, f"<< /Type /XObject /Subtype /Form {dictionary} >>")
                pdf.update_stream(ref, commands)
                return ref

            leaf = form("/BBox [0 0 148 158] /Matrix [1 0 0 1 5 3]", b"""
              q BT /helv 14 Tf 1 0 0 1 5 20 Tm (Inherited) Tj ET Q
              q BT /helv 14 Tf 1 Tr 1 0 0 1 5 66 Tm (Outlined) Tj ET Q
              q BT /helv 14 Tf 3 Tr 1 0 0 1 5 108 Tm (Hidden) Tj 0 Tr ( Following) Tj ET Q
              q BT /helv 14 Tf 7 Tr 1 0 0 1 5 145 Tm (Clip) Tj ET Q
            """)
            resources = f"/Resources << /Font << /helv {font} 0 R >> /XObject << /Leaf {leaf} 0 R >> >>"
            parent = form("/BBox [0 0 181 185] " + resources, b"q 1 0 0 1 12 10 cm /Leaf Do Q")
            root_resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
            pdf.xref_set_key(root_resources, "XObject", f"<< /Parent {parent} 0 R >>")
            pdf.update_stream(page.get_contents()[0], b"""
              q 3 Tr 1 0 0 1 80 340 cm /Parent Do Q
              q 1 0 0 1 307 188 cm /Parent Do Q
            """)
            page.set_cropbox((31, 27, 529, 573))
            page.set_rotation(rotation)
            pdf.save(source)

        with pymupdf.open(source) as pdf:
            page = pdf[0]
            trace = page.get_texttrace()
            assert [(span["type"], "".join(chr(item[0]) for item in span["chars"]).strip())
                    for span in trace] == [
                (3, "Inherited"), (1, "Outlined"), (3, "Hidden"), (0, "Following"),
                (0, "Inherited"), (1, "Outlined"), (3, "Hidden"), (0, "Following"),
            ]
            def place_at(glyph, rejection=None):
                glyph = pymupdf.Rect(glyph) * page.rotation_matrix
                point = (glyph.tl + glyph.br) * .5
                args = dict(page_index=0, x=(point.x - 4) / page.rect.width,
                            y=(point.y - 4) / page.rect.height, text="X", font_size=10)
                check(source, args, rejection=rejection)

            for span in trace:
                place_at(span["chars"][2][3], "page content" if span["type"] != 3 else None)
            # Text rendering mode 7 supplies a clip and emits no painted glyphs.
            for span in (trace[2], trace[6]):
                glyph = pymupdf.Rect(span["chars"][2][3]) + (0, -37, 0, -37)
                place_at(glyph)

        source = root / f"text-ink-boundary-{rotation}.pdf"
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=580, height=620)
            page.set_cropbox((31, 27, 529, 573))
            page.set_rotation(rotation)
            pdf.save(source)
        for text in ("l", "jjjj", "...", "  A", "W\n j"):
            args = dict(page_index=0, x=.247, y=.319, font_size=17.5, text=text)
            reference.write_bytes(source.read_bytes())
            add_text_overlay(reference, **args)
            with pymupdf.open(reference) as expected:
                page = expected[0]
                logs = page.get_bboxlog()
                assert all(kind == "fill-text" for kind, _ in logs)
                lines = [pymupdf.Rect(box) * page.rotation_matrix for _, box in logs]
                assert len(lines) == len(text.splitlines())
                ink = lines[0]
                width, height = page.rect.width, page.rect.height
            for edge, reject in ((ink.x0 + .05, True), (ink.x0 - .05, False),
                                 (ink.x1 - .05, True), (ink.x1 + .05, False)):
                left = edge - 4 if edge < (ink.x0 + ink.x1) / 2 else edge
                region = (left / width, (ink.y0 + ink.y1 - 4) / (2 * height), 4 / width, 4 / height)
                check(source, args, rejection="page component" if reject else None, reserved=region)
            for edge, reject in ((ink.y0 + .05, True), (ink.y0 - .05, False),
                                 (ink.y1 - .05, True), (ink.y1 + .05, False)):
                top = edge - 4 if edge < (ink.y0 + ink.y1) / 2 else edge
                region = ((ink.x0 + ink.x1 - 4) / (2 * width), top / height, 4 / width, 4 / height)
                check(source, args, rejection="page component" if reject else None, reserved=region)
            if len(lines) == 2:
                first, second = lines
                clear_y = (first.y1 + second.y0) / 2
                clear_x = max(first.x0, second.x0) + 1
                check(source, args, reserved=(clear_x / width, (clear_y - 1) / height,
                                              2 / width, 2 / height))
    for indirect in (False, True):
        source = root / f"text-visibility-null-resources-{indirect}.pdf"
        with pymupdf.open() as pdf:
            page = pdf.new_page()
            font = page.insert_font("helv")
            page.insert_text((100, 150), "Invisible sample", fontsize=14, render_mode=3)
            resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
            empty = "null"
            if indirect:
                xref = pdf.get_new_xref()
                pdf.update_object(xref, "null")
                empty = f"{xref} 0 R"
            pdf.update_object(resources, f"<< /Font << /helv {font} 0 R >> /XObject {empty} >>")
            pdf.save(source)
        check(source, dict(page_index=0, text="X", x=.19, y=.165, font_size=12))
    print(f"Text collision cases matched {counts['placed']} placements and {counts['rejected']} refusals")

    counts = {"placed": 0, "rejected": 0}
    source_text_paint_boundaries(root, reference, check)
    print(f"Source text paint cases matched {counts['placed']} placements and {counts['rejected']} refusals")
    counts = {"placed": 0, "rejected": 0}
    type3_source_boundaries(root, reference, check)
    print(f"Type 3 source cases matched {counts['placed']} placements and {counts['rejected']} refusals")


def cff_program_streams(data):
    streams = []
    with pymupdf.open(stream=data) as pdf:
        for xref in range(1, pdf.xref_length()):
            kind, value = pdf.xref_get_key(xref, "FontFile3")
            if kind == "xref":
                streams.append(pdf.xref_stream(int(value.split()[0])))
    assert streams
    return sorted(streams)


def embedded_cff_source(pdf, composite):
    cff = pymupdf.Font("tiit")
    name = "SyntheticCffItalic"
    sample = "JOgj"
    program, descriptor, font = (pdf.get_new_xref() for _ in range(3))
    subtype = "CIDFontType0C" if composite else "Type1C"
    pdf.update_object(program, f"<< /Subtype /{subtype} >>")
    pdf.update_stream(program, cff.buffer)
    pdf.update_object(descriptor, f"<< /Type /FontDescriptor /FontName /{name} /Flags 98 "
                                  f"/FontBBox [-200 -300 1200 1000] /ItalicAngle -15 /Ascent 900 "
                                  f"/Descent -250 /CapHeight 650 /StemV 80 /FontFile3 {program} 0 R >>")
    if not composite:
        widths = " ".join(str(round(cff.glyph_advance(code) * 1000)) for code in range(32, 127))
        pdf.update_object(font, f"<< /Type /Font /Subtype /Type1 /BaseFont /{name} /Encoding /WinAnsiEncoding "
                                f"/FontDescriptor {descriptor} 0 R /FirstChar 32 /LastChar 126 /Widths [{widths}] >>")
        return font, f"({sample})"

    glyphs = {cff.has_glyph(ord(char)): char for char in sample}
    advances = {gid: round(cff.glyph_advance(ord(char)) * 1000) for gid, char in glyphs.items()}
    return composite_cff_source(pdf, font, descriptor, name, glyphs, advances)


def composite_cff_source(pdf, font, descriptor, name, glyphs, advances):
    descendant, to_unicode = (pdf.get_new_xref() for _ in range(2))
    widths = " ".join(f"{gid} [{advances[gid]}]" for gid in glyphs)
    pdf.update_object(descendant, f"<< /Type /Font /Subtype /CIDFontType0 /BaseFont /{name} "
                                  f"/CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> "
                                  f"/FontDescriptor {descriptor} 0 R /W [{widths}] >>")
    write_to_unicode(pdf, to_unicode, glyphs)
    pdf.update_object(font, f"<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H "
                            f"/DescendantFonts [{descendant} 0 R] /ToUnicode {to_unicode} 0 R >>")
    return font, "<" + "".join(f"{gid:04X}" for gid in glyphs) + ">"


def write_to_unicode(pdf, to_unicode, glyphs):
    pdf.update_object(to_unicode, "<<>>")
    codes = "\n".join(f"<{gid:04X}> <{text.encode('utf-16-be').hex().upper()}>" for gid, text in glyphs.items())
    pdf.update_stream(to_unicode, (
        "/CIDInit /ProcSet findresource begin 12 dict begin begincmap "
        "/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def "
        "/CMapName /SyntheticCollision def /CMapType 2 def 1 begincodespacerange "
        f"<0000> <FFFF> endcodespacerange {len(glyphs)} beginbfchar\n{codes}\n"
        "endbfchar endcmap CMapName currentdict /CMap defineresource pop end end"
    ).encode())


def cff_index(*entries, version=1):
    count = pack(">I" if version == 2 else ">H", len(entries))
    if not entries:
        return count
    offsets = [1]
    for entry in entries:
        offsets.append(offsets[-1] + len(entry))
    return count + b"\x02" + pack(f">{len(offsets)}H", *offsets) + b"".join(entries)


def cff_font_matrix(values):
    def real(value):
        digits = "0123456789.-"
        nibbles = [14 if char == "-" else digits.index(char) for char in value] + [15]
        if len(nibbles) % 2:
            nibbles.append(15)
        return b"\x1e" + bytes(a << 4 | b for a, b in zip(nibbles[::2], nibbles[1::2]))
    return b"".join(real(value) for value in values.split()) + b"\x0c\x07"


def wrap_cff_opentype(cff, name, upem, tag=b"CFF "):
    def checksum(data):
        data += b"\0" * (-len(data) % 4)
        return sum(int.from_bytes(data[i:i + 4], "big") for i in range(0, len(data), 4)) & 0xffffffff

    head = bytearray(54)
    pack_into(">IIIIHH", head, 0, 0x10000, 0x10000, 0, 0x5F0F3CF5, 0, upem)
    pack_into(">hhhhHHhhh", head, 36, -200, -200, 1400, 1000, 0, 8, 2, 0, 0)
    horizontal = bytearray(36)
    pack_into(">IhhH", horizontal, 0, 0x10000, 1000, -200, 0)  # ascent, descent, line gap
    pack_into(">h", horizontal, 18, 1)  # vertical caret slope numerator
    pack_into(">H", horizontal, 34, 3)
    encoded_name = name.encode("utf-16-be")
    names = pack(">9H", 0, 1, 18, 3, 1, 0x409, 6, len(encoded_name), 0) + encoded_name
    cmap = pack(">4H I", 0, 1, 3, 1, 12)
    cmap += pack(">16H", 4, 32, 0, 4, 4, 1, 0, 66, 65535, 0, 65, 65535, 65472, 1, 0, 0)
    tables = [(tag, cff), (b"cmap", cmap), (b"head", bytes(head)), (b"hhea", bytes(horizontal)),
              (b"hmtx", pack(">HhHhHh", 500, 0, 810, 90, 980, 120)),
              (b"maxp", pack(">IH", 0x5000, 3)), (b"name", names)]
    directory = bytearray(pack(">4s4H", b"OTTO", len(tables), 64, 2, len(tables) * 16 - 64))
    data = bytearray()
    offset = 12 + 16 * len(tables)
    for tag, table in tables:
        directory.extend(pack(">4sIII", tag, checksum(table), offset + len(data), len(table)))
        if tag == b"head":
            adjustment = offset + len(data) + 8
        data.extend(table + b"\0" * (-len(table) % 4))
    result = directory + data
    pack_into(">I", result, adjustment, (0xB1B0AFBA - checksum(bytes(result))) & 0xffffffff)
    return bytes(result)


def cff_collision_outlines():
    def coordinates(*values):
        return b"".join(bytes([n + 139]) if -107 <= n <= 107 else pack(">Bh", 28, n) for n in values)

    first = coordinates(90, 90) + b"\x15" + coordinates(170, 0, 0, 320, -170, 0) + b"\x05"
    second = coordinates(120, 80) + b"\x15" + coordinates(20, 355, 170, 85, 90, -440) + b"\x08"
    return b"", first, second


def cid_cff_source(pdf, variant):
    name = "SyntheticSparseCid"
    header = b"\x01\x00\x04\x04" + cff_index(name.encode())
    strings = cff_index(b"Adobe", b"Identity") + cff_index()
    glyphs = cff_index(*(outline + b"\x0e" for outline in cff_collision_outlines()))
    charset = b"\x02" + pack(">4H", 41, 0, 782, 0)
    top_matrix = b""
    if variant == "combined":
        top_matrix = cff_font_matrix(".001 .00018 -.00017 .00091 .013 -.018")
        transform = cff_font_matrix("1.3 .15 -.10 .95 47 -38")
    elif variant == "opentype":
        transform = cff_font_matrix(".00071 .000155 -.000135 .00055 .049 .025")
    elif variant in ("subfont", "missing-private"):
        transform = cff_font_matrix(".00161 .00025 -.00016 .00135 -.041 .027")
    else:
        transform = b""
    fd_select = b"\x03" + pack(">HHBHBH", 2, 0, 0, 2, 1, 3)

    def integer(value):
        return pack(">Bi", 29, value)

    def font_dictionaries(start):
        if variant.startswith("missing-private"):
            return cff_index(b"", transform)
        has_private = integer(0) + integer(0) + b"\x12"
        end = start + len(cff_index(has_private, transform + has_private)) + len(fd_select)
        private = integer(0) + integer(end) + b"\x12"
        return cff_index(private, transform + private)

    def dictionary(start):
        charset_start = start + len(glyphs)
        fd_start = charset_start + len(charset)
        return (integer(391) + integer(392) + integer(0) + b"\x0c\x1e"
                + integer(783) + b"\x0c\x22" + integer(start) + b"\x11"
                + integer(charset_start) + b"\x0f" + integer(fd_start) + b"\x0c\x24"
                + integer(fd_start + len(font_dictionaries(fd_start))) + b"\x0c\x25" + top_matrix)

    start = len(header) + len(strings) + len(cff_index(dictionary(0)))
    fd_array = font_dictionaries(start + len(glyphs) + len(charset))
    data = header + cff_index(dictionary(start)) + strings + glyphs + charset + fd_array + fd_select
    subtype = "CIDFontType0C"
    if variant == "opentype":
        data = wrap_cff_opentype(data, name, upem=2048)
        subtype = "OpenType"
    return synthetic_cff_source(pdf, name, data, subtype, (41, 782))


def cff2_source(pdf):
    name = "SyntheticCffTwo"
    glyphs = cff_index(*cff_collision_outlines(), version=2)

    def dictionary(start):
        return pack(">BiB", 29, start, 17) + pack(">BiBB", 29, start + len(glyphs), 12, 36)

    start = 5 + len(dictionary(0)) + len(cff_index(version=2))
    top = dictionary(start)
    data = b"\x02\x00\x05" + pack(">H", len(top)) + top + cff_index(version=2) + glyphs
    blank = cff_index(pack(">BiBiB", 29, 0, 29, 0, 18), version=2)
    fd = cff_index(pack(">BiBiB", 29, 0, 29, len(data) + len(blank), 18), version=2)
    return synthetic_cff_source(pdf, name, wrap_cff_opentype(data + fd, name, 1024, b"CFF2"), "OpenType", (1, 2))


def synthetic_cff_source(pdf, name, data, subtype, codes):
    program, descriptor, font = (pdf.get_new_xref() for _ in range(3))
    pdf.update_object(program, f"<< /Subtype /{subtype} >>")
    pdf.update_stream(program, data)
    pdf.update_object(descriptor, f"<< /Type /FontDescriptor /FontName /{name} /Flags 4 "
                                  f"/FontBBox [-200 -200 1400 1000] /ItalicAngle 0 /Ascent 1000 "
                                  f"/Descent -200 /CapHeight 700 /StemV 80 /FontFile3 {program} 0 R >>")
    return composite_cff_source(pdf, font, descriptor, name, dict(zip(codes, "AB")), dict(zip(codes, (405, 490))))


def remap_collision_cff(pdf, font, kind):
    def write(reference, body):
        pdf.update_stream(reference, (
            "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n"
            "/CMapName /SyntheticCIDPaint def\n"
            "/CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> def\n"
            f"{body}\nendcmap CMapName currentdict /CMap defineresource pop end end"
        ).encode())

    def stream(body, fields=""):
        reference = pdf.get_new_xref()
        pdf.update_object(reference, f"<< /Type /CMap {fields} >>")
        write(reference, body)
        return reference

    skipped = ""
    if kind in ("named", "named-vertical"):
        name = "90ms-RKSJ-V" if kind == "named-vertical" else "90ms-RKSJ-H"
        pdf.xref_set_key(font, "Encoding", f"/{name}")
        first = second = "8251"  # CID 782 in the predefined Adobe encoding.
        ranges = "1 begincodespacerange <0000> <FFFF> endcodespacerange"
    else:
        first, second = "9120", "9240"
        ranges = "1 begincodespacerange <0000> <FFFF> endcodespacerange"
        if kind in ("mixed", "mixed-unmapped"):
            first = "21" if kind == "mixed-unmapped" else "20"
            ranges = "2 begincodespacerange <00> <7F> <8000> <FFFF> endcodespacerange"
        if kind == "mixed-unmapped":
            skipped = "209166"  # An unencoded space retains its word-spacing adjustment.
        if kind == "four-byte":
            first, second = "EF019120", "EF019240"
            ranges = "1 begincodespacerange <EF000000> <EFFFFFFF> endcodespacerange"
        if kind == "usecmap-reference":
            parent = stream(f"{ranges} 1 begincidrange <{second}> <{second}> 782 endcidrange")
            encoding = stream(f"/WMode 0 def 1 begincidchar <{first}> 41 endcidchar", f"/WMode 1 /UseCMap {parent} 0 R")
        elif kind == "usecmap-name":
            second = "030E"
            encoding = stream(f"/Identity-H usecmap 1 begincidchar <{first}> 41 endcidchar")
        else:
            encoding = stream(f"{ranges} 2 begincidchar <{first}> 41 <{second}> 782 endcidchar")
        pdf.xref_set_key(font, "Encoding", f"{encoding} 0 R")
    unicode_map = int(pdf.xref_get_key(font, "ToUnicode")[1].split()[0])
    entries = {first: "0041", second: "0041"}
    logical = " ".join(f"<{code}> <{value}>" for code, value in entries.items())
    write(unicode_map, f"{ranges} {len(entries)} beginbfchar {logical} endbfchar")
    return f"<{first}{skipped}{second}>"


def source_text_paint_boundaries(root, reference, check):
    from app.pdf_ops import add_text_overlay

    embedded = pymupdf.Font(fontfile=str(FONT))
    glyphs = [embedded.has_glyph(ord(char)) for char in "घ क"]
    assert all(glyphs)
    encoded = "".join(f"{glyph:04x}" for glyph in glyphs)
    paints = {
        "fill": ("helv", "1 0 0 1 230 315 Tm ( HI.) Tj", "fill-text"),
        "miter": ("helv", "1 Tr 2 w 0 j 3 M 1 0 0 1 230 315 Tm (Juno) Tj", "stroke-text"),
        "bevel": ("tibi", "/Stroke gs 2 Tr 1 0 0 1 230 315 Tm (Wig.) Tj", "stroke-text"),
        "round": ("hebo", "5 Tr 4 w 1 j 1 0 0 1 230 315 Tm (Round) Tj", "stroke-text"),
        "hairline": ("coit", "1 Tr 0 w 0 j 1 0 0 1 230 315 Tm (HC) Tj", "stroke-text"),
        "shear": ("helv", "1 Tr 1.25 w 1 j 1.2 .18 -.31 .7 230 315 cm "
                          "78 Tz 1.2 Ts 0 1 -1 .2 0 0 Tm (Hi) Tj", "stroke-text"),
        "array": ("helv", "1 0 0 1 230 315 Tm [(W) 23 (i) -245 (j)] TJ", "fill-text"),
        "batch": ("tiro", "1 0 0 1 230 315 Tm (A) Tj 55 32 Td (Tiny) Tj", "fill-text"),
        "nested": ("tibo", None, "stroke-text"),
        "embedded-fill": ("embedded", f"1 0 0 1 230 315 Tm <{encoded}> Tj", "fill-text"),
        "embedded-outline": ("embedded", f"1 Tr 1.5 w 2 j 1 0 0 1 230 315 Tm "
                             f"<{encoded}> Tj", "stroke-text"),
        "cff-simple-fill": ("CffSimple", "1 0 0 1 230 315 Tm", "fill-text"),
        "cff-simple-outline": ("CffSimple", "1 Tr 1.5 w 2 j 1 0 0 1 230 315 Tm", "stroke-text"),
        "cff-identity-fill": ("CffIdentity", "1 0 0 1 230 315 Tm", "fill-text"),
        "cff-identity-outline": ("CffIdentity", "1 Tr 2 w 1 j .8 .23 -.17 1.1 230 315 Tm", "stroke-text"),
        "cff-cid-fill": ("CffCid-basic", "1 0 0 1 230 315 Tm", "fill-text"),
        "cff-cid-subfont": ("CffCid-subfont", "1 0 0 1 230 315 Tm", "fill-text"),
        "cff-cid-subfont-stroke": ("CffCid-subfont", "1 Tr 2 w 1 j .8 .23 -.17 1.1 230 315 Tm", "stroke-text"),
        "cff-cid-combined": ("CffCid-combined", "1 0 0 1 230 315 Tm", "fill-text"),
        "cff-cid-opentype": ("CffCid-opentype", "1 0 0 1 230 315 Tm", "fill-text"),
        "cff-cid-missing-private": ("CffCid-missing-private", "1 0 0 1 230 315 Tm", "fill-text"),
        "cff-cid-missing-private-basic": ("CffCid-missing-private-basic", "1 0 0 1 230 315 Tm", "fill-text"),
        "cff-two": ("CffTwo", "1 0 0 1 230 315 Tm", "fill-text"),
        "cff-shared-unicode": ("CffAlias-shared", "1 0 0 1 230 315 Tm", "fill-text"),
        "cff-expanded-unicode": ("CffAlias-expanded", "1 0 0 1 230 315 Tm", "fill-text"),
        "cff-unmapped-unicode": ("CffAlias-unmapped", "1 0 0 1 230 315 Tm", "fill-text"),
        "cff-overprinted-unicode": ("CffAlias-shared", "1 0 0 1 230 315 Tm [<0029> 405 <030E>] TJ", "fill-text"),
        "cff-adjusted-unicode": ("CffAlias-expanded", "1 Tr 1.55 w 1 j 1.2 .18 -.13 .73 230 315 cm "
                                 "78 Tz 1.8 Ts -.7 Tc .8 .23 -.17 1.1 0 0 Tm [<0029> 580 <030E>] TJ", "stroke-text"),
        "cff-quote-unicode": ("CffAlias-shared", "24 TL 1 0 0 1 230 355 Tm {text} '", "fill-text"),
        "cff-double-quote-unicode": ("CffAlias-expanded", "26 TL 1 0 0 1 230 355 Tm 2.5 -1.3 {text} \" {text} Tj", "fill-text"),
        "cff-inline-actualtext": ("CffAlias-expanded", "1 0 0 1 230 315 Tm /Span << /ActualText (A longer logical value) >> "
                                  "BDC {text} Tj 0 -4 Td {text} Tj EMC", "fill-text"),
        "cff-named-actualtext": ("CffAlias-shared", "1 0 0 1 230 315 Tm /Span /Replacement BDC {text} Tj EMC", "fill-text"),
        "cff-aliased-in-form": ("CffAlias-shared", None, "stroke-text"),
        "cff-vertical-default": ("CffAlias-vertical", "1 0 0 1 230 315 Tm", "fill-text"),
        "cff-vertical-adjusted": ("CffAlias-vertical-metrics", "1 Tr 1.55 w 1 j 1.2 .18 -.13 .73 230 315 cm "
                                  "78 Tz 1.8 Ts -.7 Tc .8 .23 -.17 1.1 0 0 Tm [<0029> 580 <030E>] TJ", "stroke-text"),
        "cff-cmap-fixed": ("CffRemap-fixed", "1 0 0 1 230 315 Tm", "fill-text"),
        "cff-cmap-mixed": ("CffRemap-mixed", "13.5 Tw 73 Tz 1 0 0 1 230 315 Tm", "fill-text"),
        "cff-cmap-unmapped": ("CffRemap-mixed-unmapped", "13.5 Tw 2.5 Tc 73 Tz 1 0 0 1 230 315 Tm <9166> Tj", "fill-text"),
        "cff-cmap-four-byte": ("CffRemap-four-byte", "1 Tr 1.5 w 2 j 1 0 0 1 230 315 Tm", "stroke-text"),
        "cff-cmap-inherited-reference": ("CffRemap-usecmap-reference", "2.5 Ts .9 .21 -.16 .8 230 315 Tm", "fill-text"),
        "cff-cmap-inherited-name": ("CffRemap-usecmap-name", "1 0 0 1 230 315 Tm", "fill-text"),
        "cff-cmap-named": ("CffRemap-named", "7 Tc 1 0 0 1 230 315 Tm", "fill-text"),
        "cff-cmap-named-vertical": ("CffRemap-named-vertical", "76 Tz 1 0 0 1 230 315 Tm", "fill-text"),
        "ttf-aliased-unicode": ("embeddedAlias", f"1 0 0 1 230 315 Tm <{encoded}> Tj", "fill-text"),
        "ttf-vertical-unicode": ("embeddedVertical", f"1 0 0 1 230 315 Tm <{encoded}> Tj", "fill-text"),
        "base14-actualtext": ("helv", "1 0 0 1 230 315 Tm /Span /Replacement BDC (Actual glyphs) Tj EMC", "fill-text"),
    }
    for face in ("helv", "hebo", "heit", "hebi", "tiro", "tibo", "tiit", "tibi",
                 "cour", "cobo", "coit", "cobi"):
        paints[f"font-{face}"] = (face, "1 0 0 1 230 315 Tm (AJ.wj) Tj", "fill-text")

    def create(path, rotation, face="helv", commands="", nested=False):
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=580, height=620)
            page.insert_font("helv")
            page.draw_rect((32, 32, 48, 48))
            resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
            sample = "(Nested.)"
            if face in ("CffSimple", "CffIdentity"):
                font, sample = embedded_cff_source(pdf, composite=face == "CffIdentity")
                pdf.xref_set_key(resources, f"Font/{face}", f"{font} 0 R")
                commands = f"{commands} {sample} Tj"
            elif face.startswith("CffCid-") or face == "CffTwo":
                font, sample = cff2_source(pdf) if face == "CffTwo" else cid_cff_source(pdf, face.removeprefix("CffCid-"))
                pdf.xref_set_key(resources, f"Font/{face}", f"{font} 0 R")
                commands = f"{commands} {sample} Tj"
            elif face.startswith("CffRemap-"):
                font, _ = cid_cff_source(pdf, "subfont")
                sample = remap_collision_cff(pdf, font, face.removeprefix("CffRemap-"))
                pdf.xref_set_key(resources, f"Font/{face}", f"{font} 0 R")
                commands = f"{commands} {sample} Tj"
            elif face.startswith("CffAlias-"):
                font, sample = cid_cff_source(pdf, "subfont")
                if face != "CffAlias-unmapped":
                    stream = int(pdf.xref_get_key(font, "ToUnicode")[1].split()[0])
                    mapping = {41: "ABC", 782: "XY"} if face == "CffAlias-expanded" else {41: "A", 782: "A"}
                    write_to_unicode(pdf, stream, mapping)
                else:
                    pdf.xref_set_key(font, "ToUnicode", "null")
                if face.startswith("CffAlias-vertical"):
                    pdf.xref_set_key(font, "Encoding", "/Identity-V")
                    descendant = int(pdf.xref_get_key(font, "DescendantFonts")[1].strip("[]").split()[0])
                    if face == "CffAlias-vertical-metrics":
                        pdf.xref_set_key(descendant, "W2", "[41 [-765.6 185.7 657.6] 782 782 -900.1 333.1 702.2]")
                    else:
                        pdf.xref_set_key(descendant, "W", "null")
                        pdf.xref_set_key(descendant, "DW", "null")
                        pdf.xref_set_key(descendant, "DW2", "[830 -1020]")
                pdf.xref_set_key(resources, f"Font/{face}", f"{font} 0 R")
                if commands is not None:
                    if "{text}" in commands:
                        commands = commands.replace("{text}", sample)
                    elif not commands.endswith(" TJ"):
                        commands = f"{commands} {sample} Tj"
            else:
                font = page.insert_font(face, fontfile=str(FONT)) if face.startswith("embedded") else page.insert_font(face)
                if face in ("embeddedAlias", "embeddedVertical"):
                    stream = int(pdf.xref_get_key(font, "ToUnicode")[1].split()[0])
                    write_to_unicode(pdf, stream, {glyphs[0]: "Z", glyphs[1]: "ZQ", glyphs[2]: "Z"})
                if face == "embeddedVertical":
                    pdf.xref_set_key(font, "Encoding", "/Identity-V")
            pdf.xref_set_key(resources, "ExtGState", "<< /Stroke << /LW 3.2 /LJ 2 /ML 4 >> >>")
            pdf.xref_set_key(resources, "Properties", "<< /Replacement << /ActualText (Different sequence) >> >>")
            if nested:
                leaf = pdf.get_new_xref()
                pdf.update_object(leaf, "<< /Type /XObject /Subtype /Form /BBox [0 0 148 110] "
                                        "/Matrix [1 -.12 .15 1 35 20] >>")
                pdf.update_stream(leaf, f"BT /{face} 14 Tf 1 0 0 1 41 34 Tm {sample} Tj ET".encode())
                outer = pdf.get_new_xref()
                pdf.update_object(outer, "<< /Type /XObject /Subtype /Form /BBox [0 0 220 160] "
                                         f"/Resources << /Font << /{face} {font} 0 R >> "
                                         f"/XObject << /Leaf {leaf} 0 R >> >> >>")
                pdf.update_stream(outer, b"q /Leaf Do Q")
                pdf.xref_set_key(resources, "XObject", f"<< /Outer {outer} 0 R >>")
                commands = "q /Stroke gs 1 Tr .8 0 .1 .9 160 260 cm /Outer Do Q"
            else:
                commands = f"q BT /{face} 14 Tf {commands} ET Q"
            pdf.update_stream(page.get_contents()[0], commands.encode())
            page.set_cropbox((31, 27, 529, 573))
            page.set_rotation(rotation)
            pdf.save(path)

    def painted_rect(path, kind):
        with pymupdf.open(path) as pdf:
            page = pdf[0]
            bounds, = [pymupdf.Rect(box) * page.rotation_matrix
                       for event, box in page.get_bboxlog() if event == kind]
            return bounds, page.rect.width, page.rect.height

    for rotation in (0, 90, 180, 270):
        create(reference, rotation)
        args = dict(page_index=0, x=.23, y=.24, font_size=10, text="I")
        add_text_overlay(reference, **args)
        new, width, height = painted_rect(reference, "fill-text")
        dx0, dy0, dx1, dy1 = new + (-.23 * width, -.24 * height, -.23 * width, -.24 * height)
        for label, (face, commands, kind) in paints.items():
            source = root / f"source-paint-{rotation}-{label}.pdf"
            create(source, rotation, face, commands, nested=label in ("nested", "cff-aliased-in-form"))
            obstacle, _, _ = painted_rect(source, kind)
            x_mid = (obstacle.x0 + obstacle.x1 - dx0 - dx1) / 2
            y_mid = (obstacle.y0 + obstacle.y1 - dy0 - dy1) / 2
            for reject in (True, False):
                gap = .07 if reject else -.07
                positions = [(obstacle.x0 - dx1 + gap, y_mid), (obstacle.x1 - dx0 - gap, y_mid),
                             (x_mid, obstacle.y0 - dy1 + gap), (x_mid, obstacle.y1 - dy0 - gap)]
                for x, y in positions:
                    check(source, dict(args, x=x / width, y=y / height),
                          rejection="page content" if reject else None)
        for mode, kind in ((0, "fill-text"), (1, "stroke-text")):
            source = root / f"source-paint-whitespace-{rotation}-{mode}.pdf"
            create(source, rotation, commands=f"{mode} Tr 1 0 0 1 230 315 Tm ( ) Tj")
            empty, _, _ = painted_rect(source, kind)
            assert empty.is_empty
            check(source, dict(args, x=(empty.x0 - (dx0 + dx1) / 2) / width,
                               y=(empty.y0 - (dy0 + dy1) / 2) / height))


def type3_program_streams(data):
    programs = []
    with pymupdf.open(stream=data) as pdf:
        for xref in range(1, pdf.xref_length()):
            if pdf.xref_get_key(xref, "Subtype") != ("name", "/Type3"):
                continue
            kind, value = pdf.xref_get_key(xref, "CharProcs")
            assert kind == "dict", (xref, kind, value)
            programs.extend((name, pdf.xref_stream(int(reference))) for name, reference in
                            re.findall(r"/([^/\s<>]+)\s+(\d+)\s+\d+\s+R", value))
    assert programs
    return sorted(programs)


def type3_source_font(pdf, programs, matrix=".001 0 0 .001 0 0", resources="<<>>"):
    font = pdf.get_new_xref()
    procedures = []
    for name, commands in programs.items():
        reference = pdf.get_new_xref()
        pdf.update_object(reference, "<<>>")
        pdf.update_stream(reference, commands.encode())
        procedures.append(f"/{name} {reference} 0 R")
    pdf.update_object(font, f"<< /Type /Font /Subtype /Type3 /Name /SyntheticType3 "
                            f"/FontBBox [-80 -100 620 820] /FontMatrix [{matrix}] "
                            f"/CharProcs << {' '.join(procedures)} >> /FirstChar 65 /LastChar 66 /Widths [620 620] "
                            f"/Encoding << /Type /Encoding /Differences [65 /A /B] >> /Resources {resources} >>")
    return font


def type3_source_boundaries(root, reference, check):
    from app.pdf_ops import add_text_overlay

    rectangle = "100 100 180 220 re f"
    designs = {
        "masked": (f"620 0 0 -100 620 820 d1 {rectangle}", ".001 0 0 .001 0 0", "", "(A) Tj"),
        "clipped": ("620 0 d0 .3 g 110 93 170 155 re W n 30 30 510 470 re f", ".001 0 0 .001 0 0", "", "(A) Tj"),
        "curve": ("620 0 d0 .2 g 85 85 m 80 430 480 455 490 80 c 390 15 125 10 85 85 c f",
                  ".0012 .0002 -.00012 .00095 .011 -.006", "", "(A) Tj"),
        "stroked": ("620 0 d0 .4 G 56 w 2 j [] 0 d 130 105 135 165 re S", ".001 0 0 .001 0 0", "1 Tr", "(A) Tj"),
        "adjusted": (f"620 0 d0 .6 g {rectangle}", ".001 0 0 .001 0 0", "", "[(A) 198 (A)] TJ"),
        "quote": (f"620 0 d0 .5 g {rectangle}", ".001 0 0 .001 0 0", "32 TL", "(A) '"),
        "double-quote": (f"620 0 d0 .5 g {rectangle}", ".001 0 0 .001 0 0", "27 TL", '3 2 (AA) "'),
        "nested": ("620 0 0 0 620 820 d1 /Glyph Do", ".001 0 0 .001 0 0", "", "(A) Tj"),
    }

    def create(path, rotation, program=None, matrix=".001 0 0 .001 0 0", prefix="", show="(A) Tj", resource=None):
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=580, height=620)
            page.insert_font("helv")
            page.draw_rect((32, 32, 48, 48))
            resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
            commands = ""
            if program is not None:
                own = "<<>>"
                if resource == "nested":
                    form = pdf.get_new_xref()
                    pdf.update_object(form, "<< /Type /XObject /Subtype /Form /Matrix [1 -.18 .22 1 42 6] "
                                            "/BBox [0 0 170 460] >>")
                    pdf.update_stream(form, b"90 130 160 240 re f")
                    own = f"<< /XObject << /Glyph {form} 0 R >> >>"
                elif resource == "image":
                    picture = pdf.get_new_xref()
                    pdf.update_object(picture, "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 "
                                               "/ColorSpace /DeviceRGB /BitsPerComponent 8 >>")
                    pdf.update_stream(picture, b"\x23\x61\xaa")
                    own = f"<< /XObject << /Glyph {picture} 0 R >> >>"
                font = type3_source_font(pdf, {"A": program, "B": f"620 0 d0 0 g {rectangle}"}, matrix, own)
                pdf.xref_set_key(resources, "Font/T3", f"{font} 0 R")
                commands = f"BT /T3 24 Tf {prefix} 1 0 0 1 230 315 Tm {show} ET"
            pdf.update_stream(page.get_contents()[0], commands.encode())
            page.set_cropbox((31, 27, 529, 573))
            page.set_rotation(rotation)
            pdf.save(path)

    def painted(path):
        with pymupdf.open(path) as pdf:
            page = pdf[0]
            return [(kind, pymupdf.Rect(box) * page.rotation_matrix)
                    for kind, box in page.get_bboxlog()], page.rect.width, page.rect.height

    for rotation in (0, 90, 180, 270):
        args = dict(page_index=0, x=.23, y=.24, font_size=10, text="I")
        create(reference, rotation)
        add_text_overlay(reference, **args)
        ((_, ink),), width, height = painted(reference)
        dx0, dy0, dx1, dy1 = ink + (-.23 * width, -.24 * height, -.23 * width, -.24 * height)
        for label, (program, matrix, prefix, show) in designs.items():
            source = root / f"type3-geometry-{rotation}-{label}.pdf"
            create(source, rotation, program, matrix, prefix, show, resource=label)
            ((kind, box),), _, _ = painted(source)
            assert kind == "fill-text", (label, kind)
            x_mid = (box.x0 + box.x1 - dx0 - dx1) / 2
            y_mid = (box.y0 + box.y1 - dy0 - dy1) / 2
            for refuse, margin in ((True, .07), (False, -.07)):
                for x, y in ((box.x0 - dx1 + margin, y_mid), (box.x1 - dx0 - margin, y_mid),
                             (x_mid, box.y0 - dy1 + margin), (x_mid, box.y1 - dy0 - margin)):
                    check(source, dict(args, x=x / width, y=y / height),
                          rejection="page content" if refuse else None)
        for mode in range(8):
            source = root / f"type3-inherited-{rotation}-{mode}.pdf"
            create(source, rotation, f"620 0 d0 {rectangle}", prefix=f"{mode} Tr", show="[(A) -200 (B)] TJ")
            events, _, _ = painted(source)
            if mode in (3, 7):
                assert all(kind == "ignore-text" for kind, _ in events), (mode, events)
            else:
                assert [kind for kind, _ in events] == ["fill-path", "ignore-text", "fill-text"], (mode, events)
            for kind, box in (events[0], events[-1]):
                x = (box.x0 + box.x1 - dx0 - dx1) / (2 * width)
                y = (box.y0 + box.y1 - dy0 - dy1) / (2 * height)
                check(source, dict(args, x=x, y=y), rejection="page content" if kind == "fill-text" else None)
        source = root / f"type3-direct-image-boundary-{rotation}.pdf"
        create(source, rotation, "620 0 d0 30 90 70 100 re f q 150 12 35 137 280 145 cm /Glyph Do Q",
               matrix=".0012 .0002 -.00012 .00095 .011 -.006", prefix="79 Tz 4 Ts", resource="image")
        events, _, _ = painted(source)
        assert [kind for kind, _ in events] == ["fill-path", "fill-image", "ignore-text"], events
        _, box = events[1]
        x_mid = (box.x0 + box.x1 - dx0 - dx1) / 2
        y_mid = (box.y0 + box.y1 - dy0 - dy1) / 2
        for refuse, margin in ((True, .07), (False, -.07)):
            for x, y in ((box.x0 - dx1 + margin, y_mid), (box.x1 - dx0 - margin, y_mid),
                         (x_mid, box.y0 - dy1 + margin), (x_mid, box.y1 - dy0 - margin)):
                check(source, dict(args, x=x / width, y=y / height), rejection="page content" if refuse else None)


def annotation_overlay_contract(rust, root):
    import pypdfium2
    from PIL import ImageChops
    from app.pdf_ops import add_text_overlay, ProcessingError

    matter = rust("POST", "/matters", {"name": "Synthetic annotation appearance collisions"},
                  status=201)["id"]
    overlap = "Text overlaps existing page content. Choose a clear area."
    empty = "Text could not be placed on this page"
    ref = root / "annotation-placement-reference.pdf"
    counts = {"placed": 0, "rejected": 0}

    def annotations(data):
        with pymupdf.open(stream=data) as pdf:
            result = []
            for xref, _, _ in pdf[0].annot_xrefs():
                attrs = [pdf.xref_get_key(xref, key) for key in ("Subtype", "Rect", "F", "AS", "V", "Contents")]
                kind, value = pdf.xref_get_key(xref, "AP/N")
                if kind == "dict":
                    streams = sorted((name, pdf.xref_stream(int(number))) for name, number in
                                     re.findall(r"/([^/\s<>]+)\s+(\d+)\s+\d+\s+R", value))
                elif kind == "xref":
                    streams = [pdf.xref_stream(int(value.split()[0]))]
                else:
                    streams = []
                result.append((attrs, kind, streams))
            return result

    def check(source, body, refusal=None):
        original = source.read_bytes()
        ref.write_bytes(original)
        doc = rust("IMPORT", matter, source=str(source), status=201)[0]["id"]
        route = f"/documents/{doc}"
        try:
            add_text_overlay(ref, **body)
        except ProcessingError as exc:
            assert str(exc) == refusal, (source.name, body, str(exc), refusal)
            rejected = rust("POST", route + "/pages/text-overlay", body, status=None)
            assert rejected == {"status": 422, "body": {"detail": refusal}}, (source.name, body, rejected)
            assert [revision["revision"] for revision in rust("GET", route + "/revisions")] == [1]
            assert pdf_bytes(rust, doc, 1) == original
            counts["rejected"] += 1
        else:
            assert refusal is None, (source.name, body, refusal)
            assert rust("POST", route + "/pages/text-overlay", body)["revision"] == 2
            actual = pdf_bytes(rust, doc, 2)
            assert annotations(actual) == annotations(original), source.name
            if "type3-" in source.name:
                assert type3_program_streams(actual) == type3_program_streams(original), source.name
            with pypdfium2.PdfDocument(ref.read_bytes()) as expected, pypdfium2.PdfDocument(actual) as edited:
                expected.init_forms()
                edited.init_forms()
                a, b = (pdf[0].render(scale=2).to_pil().convert("RGB") for pdf in (expected, edited))
                assert ImageChops.difference(a, b).getbbox() is None, (source.name, body)
            counts["placed"] += 1
        assert pdf_bytes(rust, doc, 1, "original") == original

    def create(path, rotation, kind, missing=False):
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=550, height=650)
            rect = pymupdf.Rect(293, 385, 494, 468)
            annot = None
            if kind in ("text", "hidden", "invisible", "noview", "off", "text-frame", "text-spaces"):
                message = "  " if kind == "text-spaces" else "EXISTING FREE TEXT"
                annot = page.add_freetext_annot(rect, message, fontsize=14,
                                               fill_color=(.8, .9, .7) if kind == "text-frame" else None,
                                               border_width=2 if kind == "text-frame" else 0)
                flags = {"hidden": 2, "invisible": 1, "noview": 32}
                if kind in flags:
                    annot.set_flags(flags[kind])
                    annot.update()
                if kind == "off":
                    annot.set_oc(pdf.add_ocg("Synthetic off", on=False))
            elif kind == "square":
                annot = page.add_rect_annot(rect)
            elif kind == "circle":
                annot = page.add_circle_annot(rect)
                annot.set_colors(fill=(.2, .6, .7))
                annot.update()
            elif kind == "note":
                annot = page.add_text_annot(rect.tl, "Synthetic note")
            elif kind == "note-help":
                annot = page.add_text_annot(rect.tl, "Synthetic help note", icon="Help")
            elif kind == "caret":
                annot = page.add_caret_annot(rect.tl)
            elif kind == "file-attachment":
                annot = page.add_file_annot(rect.tl, b"Synthetic fixture", "fixture.txt", icon="Paperclip")
            elif kind in ("line-caption", "line-no-stroke", "line-endings", "polyline-endings", "polygon-fill"):
                points = [rect.bl, rect.tl + (35, 4), rect.tr]
                annot = (page.add_polyline_annot(points) if kind == "polyline-endings"
                         else page.add_polygon_annot(points) if kind == "polygon-fill"
                         else page.add_line_annot(points[0], points[-1]))
                if kind == "line-caption":
                    pdf.xref_set_key(annot.xref, "Cap", "true")
                    pdf.xref_set_key(annot.xref, "Contents", "(Synthetic caption)")
                    pdf.xref_set_key(annot.xref, "LL", "9")
                if kind.endswith("endings"):
                    pdf.xref_set_key(annot.xref, "LE", "[/ClosedArrow /Circle]")
                    pdf.xref_set_key(annot.xref, "IC", "[0 .5 1]")
                if kind in ("line-no-stroke", "polygon-fill"):
                    pdf.xref_set_key(annot.xref, "C", "null")
                if kind == "polygon-fill":
                    pdf.xref_set_key(annot.xref, "IC", "[0 .5 1]")
            elif kind in ("callout", "callout-empty"):
                text = "Synthetic callout" if kind == "callout" else ""
                annot = page.add_freetext_annot(
                    rect, text, fontsize=14, border_width=1,
                    callout=[rect.tl + (-22, -6), rect.tl + (-10, -6), rect.tl + (5, 3)],
                )
            elif kind.startswith("rich-"):
                annot = page.add_freetext_annot(rect, "Stored appearance seed", fontsize=14)
                markup = {
                    "rich-shown": '<p><span style="color:red">Synthetic</span> <b>label</b></p>',
                    "rich-trailing-box": '<p>Synthetic label</p><p style="background-color:red; height:20pt"></p>',
                    "rich-space": "<span> </span>",
                }[kind]
                pdf.xref_set_key(annot.xref, "RC", pymupdf.get_pdf_str(markup))
            elif kind == "highlight":
                annot = page.add_highlight_annot([rect, rect + (0, 90, 0, 90)])
            elif kind == "link":
                page.insert_link({"kind": pymupdf.LINK_URI, "from": rect, "uri": "https://example.com"})
            elif kind == "stamp":
                annot = page.add_stamp_annot(rect, stamp=0)
            elif kind.startswith("widget") or kind in ("checkbox", "choice", "push"):
                widget = pymupdf.Widget()
                widget.rect = rect
                widget.field_name = "synthetic_field"
                widget.field_type = {
                    "checkbox": pymupdf.PDF_WIDGET_TYPE_CHECKBOX,
                    "choice": pymupdf.PDF_WIDGET_TYPE_LISTBOX,
                    "push": pymupdf.PDF_WIDGET_TYPE_BUTTON,
                    "widget-unsigned": pymupdf.PDF_WIDGET_TYPE_SIGNATURE,
                }.get(kind, pymupdf.PDF_WIDGET_TYPE_TEXT)
                widget.field_value = True if kind == "checkbox" else "EXISTING FIELD"
                if kind == "widget-unsigned":
                    widget.field_value = None
                if kind == "widget-formatted":
                    widget.script_format = 'event.value = "FORMATTED";'
                if kind == "choice":
                    widget.choice_values = ["ONE", "TWO"]
                    widget.field_value = "TWO"
                if kind == "push":
                    widget.button_caption = "Push"
                    widget.field_value = ""
                if kind in ("widget-empty", "widget-stale-blank"):
                    widget.field_value = ""
                if kind == "widget-frame":
                    widget.fill_color = (.8, .6, .7)
                    widget.border_color = (.3, .4, .2)
                    widget.border_width = 3
                annot = page.add_widget(widget)
                if kind in ("widget-rich-zero", "widget-rich-styled"):
                    markup = '<p><b>Synthetic field</b></p>'
                    if kind == "widget-rich-styled":
                        markup = '<p style="font-size:16pt"><b>Synthetic field</b></p>'
                    pdf.xref_set_key(annot.xref, "RV", pymupdf.get_pdf_str(markup))
                if kind in ("widget-stale-blank", "widget-stale-text"):
                    value = "CURRENT VALUE" if kind == "widget-stale-blank" else ""
                    pdf.xref_set_key(annot.xref, "V", pymupdf.get_pdf_str(value))
                    form_kind, form = pdf.xref_get_key(pdf.pdf_catalog(), "AcroForm")
                    if form_kind == "xref":
                        pdf.xref_set_key(int(form.split()[0]), "NeedAppearances", "true")
                    else:
                        pdf.xref_set_key(pdf.pdf_catalog(), "AcroForm/NeedAppearances", "true")
                if kind == "widget-order":
                    page.add_rect_annot((42, 50, 110, 116))
            else:
                annot = page.add_rect_annot(rect)
                ap = int(pdf.xref_get_key(annot.xref, "AP/N")[1].split()[0])
                helv = page.insert_font("helv")
                resources = f"/Font << /H {helv} 0 R >>"
                pdf.xref_set_key(ap, "BBox", "[0 0 140 60]")
                pdf.xref_set_key(ap, "Matrix", "[1 .12 -.13 1 4 -2]")
                if kind in ("image", "image-first", "image-last", "stencil"):
                    image = pdf.get_new_xref()
                    info = "/ImageMask true /BitsPerComponent 1" if kind == "stencil" else (
                        "/ColorSpace /DeviceRGB /BitsPerComponent 8")
                    pdf.update_object(image, "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 "
                                             f"{info} >>")
                    pdf.update_stream(image, b"\xff" if kind == "stencil" else b"\x70\x30\x92")
                    resources += f" /XObject << /I {image} 0 R >>"
                if kind == "inside-off":
                    ocg = pdf.add_ocg("Synthetic appearance off", on=False)
                    resources += f" /Properties << /H {ocg} 0 R >>"
                if kind.startswith("mask-"):
                    mask = pdf.get_new_xref()
                    pdf.update_object(mask, f"<< /Type /XObject /Subtype /Form /BBox [0 0 140 60] "
                                            f"/Group << /S /Transparency /CS /DeviceGray >> "
                                            f"/Resources << /Font << /H {helv} 0 R >> >> >>")
                    mask_commands = (b"1 g BT /H 12 Tf 1 0 0 1 6 14 Tm (Mask mark) Tj ET"
                                     if kind in ("mask-text", "mask-reset") else b"1 g 0 0 140 60 re f")
                    pdf.update_stream(mask, mask_commands)
                    resources += (f" /ExtGState << /Masked << /SMask << /S /Luminosity /G {mask} 0 R >> >> "
                                  f"/Clear << /SMask /None >> >>")
                if kind.startswith("type3-"):
                    programs = {"A": "620 0 d0 90 100 350 470 re f",
                                "B": "620 0 0 0 620 820 d1 90 100 350 470 re f"}
                    glyph_resources = "<<>>"
                    if kind == "type3-direct-image":
                        picture = pdf.get_new_xref()
                        pdf.update_object(picture, "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 "
                                                   "/ColorSpace /DeviceRGB /BitsPerComponent 8 >>")
                        pdf.update_stream(picture, b"\x36\x42\xb5")
                        glyph_resources = f"<< /XObject << /Image {picture} 0 R >> >>"
                        programs["A"] += " q 272 0 0 240 400 320 cm /Image Do Q"
                    t3 = type3_source_font(pdf, programs, resources=glyph_resources)
                    resources = f"/Font << /T {t3} 0 R >>"
                pdf.xref_set_key(ap, "Resources", f"<< {resources} >>")
                text = b"BT /H 12 Tf 1 0 0 1 10 15 Tm (Existing appearance) Tj ET"
                picture = b"q 104 0 0 36 4 9 cm /I Do Q"
                commands = {
                    "image": picture,
                    "stencil": picture,
                    "image-first": picture + b" " + text,
                    "image-last": text + b" " + picture,
                    "inline": b"q 100 0 0 30 4 10 cm BI /W 1 /H 1 /CS /RGB /BPC 8 ID \x14\x42\x99 EI Q",
                    "outside": b"BT /H 12 Tf 1 0 0 1 10 100 Tm (Outside clip) Tj ET",
                    "ignore-text": b"BT /H 12 Tf 3 Tr 1 0 0 1 10 15 Tm (Hidden) Tj ( words) Tj ET",
                    "clip-text": b"BT /H 12 Tf 7 Tr 1 0 0 1 10 15 Tm (Clip) Tj ET",
                    "space-stroke": b"BT /H 12 Tf 1 Tr 1 0 0 1 10 15 Tm ( ) Tj ET",
                    "inside-off": b"/OC /H BDC " + text + b" 2 2 20 20 re f EMC",
                    "state": text,
                    "mask-text": b"q /Masked gs " + text + b" 10 20 40 25 re f Q",
                    "mask-path": b"q /Masked gs " + text + b" 10 20 40 25 re f Q",
                    "mask-reset": b"q /Masked gs " + text + b" /Clear gs 10 20 40 25 re f Q",
                    "type3-direct": b"BT /T 16 Tf 1 0 0 1 10 15 Tm (AA) Tj ET",
                    "type3-ignore": b"BT /T 16 Tf 7 Tr 1 0 0 1 10 15 Tm (AA) Tj ET",
                    "type3-mixed-tail": b"BT /T 16 Tf 1 0 0 1 10 15 Tm (BA) Tj ET",
                    "type3-mixed-head": b"BT /T 16 Tf 1 0 0 1 10 15 Tm (AB) Tj ET",
                    "type3-direct-image": b"BT /T 28 Tf 1 0 0 1 10 15 Tm (A) Tj ET",
                }[kind]
                pdf.update_stream(ap, commands)
                if kind == "state":
                    other = pdf.get_new_xref()
                    pdf.update_object(other, pdf.xref_object(ap))
                    pdf.update_stream(other, b"q 2 2 30 30 re S Q")
                    pdf.xref_set_key(annot.xref, "AP", f"<< /N << /On {ap} 0 R /Off {other} 0 R >> >>")
                    pdf.xref_set_key(annot.xref, "AS", "/Off")
            if missing:
                pdf.xref_set_key(annot.xref, "AP", "null")
            page.set_cropbox((25, 30, 525, 625))
            page.set_rotation(rotation)
            pdf.save(path)

    cases = {
        "text": (overlap,) * 3,
        "hidden": (None,) * 3, "invisible": (None,) * 3, "noview": (None,) * 3, "off": (None,) * 3,
        "text-frame": (overlap,) * 3, "text-spaces": (None,) * 3,
        "square": (empty, None, None), "circle": (empty, empty, None), "note": (empty, empty, None),
        "highlight": (empty, empty, None), "link": (None,) * 3, "stamp": (overlap,) * 3,
        "line-caption": (empty, overlap, overlap), "line-no-stroke": (None,) * 3,
        "line-endings": (empty,) * 3, "polyline-endings": (empty,) * 3, "polygon-fill": (empty, None, None),
        "callout": (overlap,) * 3, "callout-empty": (empty, empty, None),
        "note-help": (empty, empty, None), "file-attachment": (empty, empty, None),
        "caret": (empty, None, None), "rich-shown": (overlap,) * 3,
        "rich-trailing-box": (empty, overlap, overlap), "rich-space": (None,) * 3,
        "widget-unsigned": (overlap,) * 3, "widget-formatted": (overlap,) * 3,
        "widget-rich-zero": (None,) * 3, "widget-rich-styled": (overlap,) * 3,
        "widget": (overlap,) * 3, "widget-order": (overlap,) * 3, "widget-empty": (None,) * 3,
        "widget-stale-blank": (overlap,) * 3, "widget-stale-text": (None,) * 3,
        "widget-frame": (overlap,) * 3, "checkbox": (overlap,) * 3, "choice": (overlap,) * 3,
        "push": (overlap,) * 3,
        "image": (empty, None, None), "stencil": (empty, None, None),
        "image-first": (overlap,) * 3, "image-last": (empty, overlap, overlap),
        "inline": (empty, None, None), "outside": (overlap,) * 3, "ignore-text": (empty, None, None),
        "clip-text": (None,) * 3, "space-stroke": (None,) * 3, "inside-off": (None,) * 3,
        "state": (empty, None, None),
        "mask-text": (empty, overlap, overlap), "mask-path": (empty, empty, overlap),
        "mask-reset": (empty, overlap, overlap),
        "type3-direct": (empty, empty, empty), "type3-ignore": (empty, empty, None),
        "type3-mixed-tail": (empty, empty, overlap), "type3-mixed-head": (overlap,) * 3,
        "type3-direct-image": (empty, empty, None),
    }
    regenerated = {"text", "text-frame", "text-spaces", "square", "circle", "note", "highlight",
                   "stamp", "widget", "widget-empty", "widget-frame", "checkbox", "choice", "push"}
    generated_only = {"line-caption", "line-no-stroke", "line-endings", "polyline-endings", "polygon-fill",
                      "callout", "callout-empty", "caret", "note-help", "file-attachment", "rich-shown",
                      "rich-trailing-box", "rich-space", "widget-formatted", "widget-rich-zero", "widget-rich-styled"}
    overlays = ("FIRST", "FIRST\nSECOND", "FIRST\nSECOND\nTHIRD\nFOURTH")
    for rotation in (0, 90, 180, 270):
        for kind, expected in cases.items():
            variants = (True,) if kind in generated_only else (False, True) if kind in regenerated | {"widget-unsigned"} else (False,)
            for missing in variants:
                source = root / f"annotation-appearance-{rotation}-{kind}-{missing}.pdf"
                create(source, rotation, kind, missing)
                for text, refusal in zip(overlays, expected, strict=True):
                    x = .64 if rotation == 180 else .20
                    check(source, dict(page_index=0, x=x, y=.22, font_size=12, text=text), refusal)
        for kind, refusal in (("image", overlap), ("stencil", None)):
            source = root / f"annotation-image-intersection-{rotation}-{kind}.pdf"
            create(source, rotation, kind)
            with pymupdf.open(source) as pdf:
                page = pdf[0]
                (_, box), = page.get_bboxlog()
                bounds = pymupdf.Rect(box) * page.rotation_matrix
                point = (bounds.tl + bounds.br) * .5
                x = (point.x - 8) / page.rect.width
                y = (point.y - 24) / page.rect.height
            check(source, dict(page_index=0, x=x, y=y, font_size=12, text="A\nI"), refusal)
        source = root / f"source-page-soft-mask-{rotation}.pdf"
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=550, height=650)
            font = page.insert_font("helv")
            mask = pdf.get_new_xref()
            pdf.update_object(mask, f"<< /Type /XObject /Subtype /Form /BBox [0 0 240 90] "
                                    f"/Group << /S /Transparency /CS /DeviceGray >> "
                                    f"/Resources << /Font << /H {font} 0 R >> >> >>")
            pdf.update_stream(mask, b"1 g BT /H 28 Tf 1 0 0 1 30 35 Tm (SYNTHETIC MASK) Tj ET")
            resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
            pdf.xref_set_key(resources, "ExtGState", f"<< /Masked << /SMask << /S /Alpha /G {mask} 0 R >> >> >>")
            content = pdf.get_new_xref()
            pdf.update_object(content, "<<>>")
            pdf.update_stream(content, b"q 1 0 0 1 120 220 cm /Masked gs 1 0 0 1 62 -5 cm 0 0 200 120 re f Q")
            pdf.xref_set_key(page.xref, "Contents", f"{content} 0 R")
            page.set_cropbox((25, 30, 525, 625))
            page.set_rotation(rotation)
            pdf.save(source)
        with pymupdf.open(source) as pdf:
            page = pdf[0]
            mask, = [pymupdf.Rect(bounds) * page.rotation_matrix
                     for kind, bounds in page.get_bboxlog() if kind == "fill-text"]
            center = (mask.tl + mask.br) * .5
            x = (center.x - 4) / page.rect.width
            y = (center.y - 13) / page.rect.height
        check(source, dict(page_index=0, x=x, y=y, font_size=12, text="X"), overlap)
        check(source, dict(page_index=0, x=.05, y=.05, font_size=12, text="Source mask clear"), None)
        source = root / f"annotation-type3-image-intersection-{rotation}.pdf"
        clear = root / "type3-annotation-clear.pdf"
        create(source, rotation, "type3-direct-image")
        with pymupdf.open(source) as pdf:
            page = pdf[0]
            image, = [pymupdf.Rect(bounds) * page.rotation_matrix
                      for kind, bounds in page.get_bboxlog() if kind == "fill-image"]
            width, height = page.rect.width, page.rect.height
            page.delete_annot(next(page.annots()))
            pdf.save(clear)
        position = dict(page_index=0, x=.20, y=.22, font_size=12, text="A\nA\nA\nI")
        add_text_overlay(clear, **position)
        with pymupdf.open(clear) as pdf:
            page = pdf[0]
            last = pymupdf.Rect(page.get_bboxlog()[-1][1]) * page.rotation_matrix
        delta = ((image.tl + image.br) - (last.tl + last.br)) * .5
        check(source, dict(position, x=position["x"] + delta.x / width,
                           y=position["y"] + delta.y / height), overlap)
        first = root / f"annotation-first-line-skipped-{rotation}.pdf"
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=550, height=650)
            page.set_cropbox((25, 30, 525, 625))
            page.set_rotation(rotation)
            pdf.save(first)
        position = dict(page_index=0, x=.20, y=.22, font_size=12)
        add_text_overlay(first, **position, text="FIRST")
        with pymupdf.open(first) as pdf:
            page = pdf[0]
            page.add_rect_annot((293, 385, 494, 468))
            pdf.saveIncr()
        check(first, {**position, "text": "FIRST\nSECOND"})
    print(f"Annotation appearance cases matched {counts['placed']} placements and {counts['rejected']} refusals")


def profiled_png_resources(rust, matter, root):
    from struct import pack, unpack_from
    from zlib import crc32
    from PIL import ImageChops, ImageCms
    import pypdfium2
    from app.pdf_ops import insert_working_image
    from app.print_export import print_copy

    def gamma_profile(gray):
        base = ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB")).tobytes()
        size, = unpack_from(">I", base, 128)
        tags = {}
        for index in range(size):
            name, offset, length = unpack_from(">4sII", base, 132 + index * 12)
            tags[name] = base[offset:offset + length]
        header = bytearray(base[:128])
        header[84:100] = bytes(16)
        curve = pack(">4sIIH", b"curv", 0, 1, int(1.5 * 256))
        if gray:
            header[16:20] = b"GRAY"
            tags = {name: tags[name] for name in (b"desc", b"cprt", b"wtpt")}
            tags[b"kTRC"] = curve
        else:
            for name in (b"rTRC", b"gTRC", b"bTRC"):
                tags[name] = curve
        index = bytearray(pack(">I", len(tags)))
        payload = bytearray()
        for name, raw in tags.items():
            offset = 128 + 4 + 12 * len(tags) + len(payload)
            index.extend(pack(">4sII", name, offset, len(raw)))
            payload.extend(raw + bytes(-len(raw) % 4))
        header[:4] = pack(">I", len(header) + len(index) + len(payload))
        return bytes(header + index + payload)

    def deep_png(kind):
        def chunk(name, data):
            return pack(">I", len(data)) + name + data + pack(">I", crc32(name + data))

        width, height = 40, 16
        components, color = {"rgb16": (3, 2), "rgba16": (4, 6), "gray16": (1, 0),
                             "gray-alpha16": (2, 4), "key16": (3, 2)}[kind]
        alpha = (0, 250, 511, 809, 16003, 29002, 32769, 60004, 65535)
        rows = []
        for y in range(height):
            values = []
            for x in range(width):
                for component in range(components):
                    if color in (4, 6) and component == components - 1:
                        values.append(alpha[(x + y) % len(alpha)])
                    else:
                        values.append((1229 * x + 1234 * y + 18517 * component) % 65536)
            rows.append(b"\0" + pack(f">{len(values)}H", *values))
        header = chunk(b"IHDR", pack(">IIBBBBB", width, height, 16, color, 0, 0, 0))
        transparency = chunk(b"tRNS", pack(">HHH", 1229, 19746, 38263)) if kind == "key16" else b""
        return b"\x89PNG\r\n\x1a\n" + header + transparency + chunk(b"IDAT", compress(b"".join(rows))) + chunk(b"IEND", b"")

    def matching(before, after, case):
        with pypdfium2.PdfDocument(before) as python, pypdfium2.PdfDocument(after) as native:
            for expected, actual in zip(python, native, strict=True):
                a, b = (page.render(scale=2).to_pil().convert("RGB") for page in (expected, actual))
                assert a.size == b.size and ImageChops.difference(a, b).getbbox() is None, case

    variants = ("rgb-profile", "rgba-profile", "gray-profile", "gray-alpha-profile", "palette-profile",
                "rgb16", "rgba16", "gray16", "gray-alpha16", "key16", "monochrome")
    profiles = {False: gamma_profile(False), True: gamma_profile(True)}
    for kind in variants:
        profile = None
        if kind.endswith("16"):
            png = deep_png(kind)
        else:
            gray = kind.startswith("gray")
            profile = profiles[gray] if kind.endswith("profile") else None
            image = Image.new("RGBA", (80, 32), (234, 11, 119, 255))
            draw = ImageDraw.Draw(image)
            for index, alpha in enumerate((0, 1, 2, 113, 127, 245, 255)):
                draw.rectangle((index * 8, 0, (index + 1) * 8 - 1, 31), fill=(15, 147, 77, alpha))
            mode = {"rgb-profile": "RGB", "gray-profile": "L", "gray-alpha-profile": "LA",
                    "palette-profile": "P", "monochrome": "1"}.get(kind, "RGBA")
            output = BytesIO()
            image.convert(mode).save(output, format="PNG", **({"icc_profile": profile} if profile else {}))
            png = output.getvalue()
        raster = root / f"profiled-png-{kind}.png"
        raster.write_bytes(png)
        original_image = rust("IMPORT", matter, source=str(raster), status=201)[0]["id"]
        with pymupdf.open(stream=png, filetype="png") as image:
            matching(image.convert_to_pdf(), pdf_bytes(rust, original_image, 1), (kind, "import"))
        assert pdf_bytes(rust, original_image, 1, "original") == png

        for degrees in (0, 90, 180, 270):
            path = root / f"profiled-png-{kind}-{degrees}.pdf"
            with pymupdf.open() as pdf:
                page = pdf.new_page(width=580, height=620)
                page.draw_rect(pymupdf.Rect(15, 20, 540, 580), fill=(.92, .90, .70), color=None)
                page.set_cropbox(pymupdf.Rect(29, 31, 541, 572))
                page.set_rotation(degrees)
                pdf.save(path)
            original = path.read_bytes()
            reference = root / f"profiled-png-{kind}-{degrees}-python.pdf"
            reference.write_bytes(original)
            insert_working_image(reference, 0, png, "png", .28, .32, .4, .25)
            doc = rust("IMPORT", matter, source=str(path), status=201)[0]["id"]
            route = f"/documents/{doc}"
            args = {"page_index": 0, "expected_revision": 1, "rect_x": .28, "rect_y": .32,
                    "rect_width": .4, "rect_height": .25, "filename": raster.name,
                    "data": b64encode(png).decode()}
            rust("POST", route + "/annotations/image", args, status=201)
            editable = rust("GET", route + "/pdf?editable=true&revision=1")
            matching(reference, b64decode(editable["data"]), (kind, degrees, "flattened"))
            if (kind, degrees) in (("rgba-profile", 90), ("key16", 270)):
                printable = rust("POST", route + "/print", {"expected_revision": 1, "pages": "all",
                                 "orientation": "source", "scale": "actual"})
                matching(print_copy(reference, "all", "source", "actual"),
                         b64decode(printable["data"]), (kind, degrees, "print"))
            assert pdf_bytes(rust, doc, 1) == original
            assert rust("POST", route + "/pages/image-overlay", args)["revision"] == 2
            result = pdf_bytes(rust, doc, 2)
            matching(reference, result, (kind, degrees, "working"))
            with pymupdf.open(reference) as python, pymupdf.open(stream=result) as native:
                left, = python[0].get_images()
                right, = native[0].get_images()
                assert bool(left[1]) == bool(right[1]), kind
                if kind != "monochrome":
                    assert python.xref_stream(left[0]) == native.xref_stream(right[0]), (kind, degrees)
                if right[1]:
                    assert pymupdf.Pixmap(python, left[1]).samples == pymupdf.Pixmap(native, right[1]).samples, kind
                assert pdf_image_profile(native, right[0]) == profile, kind
                if profile:
                    assert pdf_image_profile(python, left[0]) == profile, kind
            assert pdf_bytes(rust, doc, 1, "original") == original
            if degrees == 90:
                (root / f"profiled-png-{kind}-{degrees}-rust.pdf").write_bytes(result)


def reference_cmyk_profile(jpeg):
    with pymupdf.open(stream=jpeg, filetype="jpeg") as loaded:
        with pymupdf.open(stream=loaded.convert_to_pdf()) as reference:
            profile = pdf_image_profile(reference, reference[0].get_images()[0][0])
    assert profile and profile[16:20] == b"CMYK"
    assert sha256(profile).hexdigest() == "8472fa1493a024b800b67dee9424835ec0c41ab79490200ae8ec4a689fd1b9a9"
    return profile


def cmyk_jpeg_placement(rust, matter, root):
    import pypdfium2
    from PIL import ImageChops, ImageCms
    from app.pdf_ops import insert_working_image
    from app.print_export import print_copy

    image = Image.new("CMYK", (80, 32), (20, 143, 6, 21))
    draw = ImageDraw.Draw(image)
    draw.rectangle((25, 0, 52, 31), fill=(133, 3, 72, 53))
    draw.rectangle((56, 9, 76, 27), fill=(66, 38, 172, 24))
    def encode(**options):
        payload = BytesIO()
        image.save(payload, format="JPEG", **options)
        return payload.getvalue()

    unprofiled = encode()
    profile = reference_cmyk_profile(unprofiled)
    # Change an identification label while preserving all color transforms.
    # This makes the caller-supplied profile distinguishable from the fallback.
    custom = bytearray(profile)
    label = custom.index(b"Artifex CMYK SWOP Profile")
    custom[label + len(b"Artifex CMYK SWOP Profile") - 1] = ord("X")
    custom = bytes(custom)
    adobe = unprofiled.index(b"\xff\xee")
    length = int.from_bytes(unprofiled[adobe + 2:adobe + 4], "big")
    assert unprofiled[adobe + 4:adobe + 9] == b"Adobe"
    no_marker = unprofiled[:adobe] + unprofiled[adobe + 2 + length:]
    wrong_components = ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB")).tobytes()

    def compare(reference, actual, jpeg, expected_profile, case):
        with pypdfium2.PdfDocument(reference) as baseline, pypdfium2.PdfDocument(actual) as native:
            a, b = (pdf[0].render(scale=2).to_pil().convert("RGB") for pdf in (baseline, native))
            assert ImageChops.difference(a, b).getbbox() is None, case
        with pymupdf.open(stream=actual) as pdf:
            xref, = (item[0] for item in pdf[0].get_images())
            assert pdf.xref_stream_raw(xref) == jpeg, case
            assert pdf.xref_get_key(xref, "Decode") == ("array", "[1 0 1 0 1 0 1 0]"), case
            assert pdf_image_profile(pdf, xref) == expected_profile, case

    for kind, jpeg, expected_profile in (
        ("profiled", encode(icc_profile=custom), custom),
        ("unprofiled", unprofiled, profile),
        ("no-adobe-marker", no_marker, profile),
        ("wrong-components", encode(icc_profile=wrong_components), profile),
    ):
        fixture = root / f"{kind}-cmyk.jpg"
        fixture.write_bytes(jpeg)
        image_doc = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
        with pymupdf.open(stream=jpeg, filetype="jpeg") as loaded:
            compare(loaded.convert_to_pdf(), pdf_bytes(rust, image_doc, 1), jpeg, expected_profile, (kind, "import"))
        assert pdf_bytes(rust, image_doc, 1, "original") == jpeg
        for rotation in (0, 90, 180, 270):
            page_path = root / f"{kind}-cmyk-{rotation}.pdf"
            reference = root / f"{kind}-cmyk-{rotation}-python.pdf"
            with pymupdf.open() as pdf:
                page = pdf.new_page(width=460, height=510)
                page.set_cropbox(pymupdf.Rect(25, 22, 429, 493))
                page.set_rotation(rotation)
                pdf.save(page_path)
            original = page_path.read_bytes()
            reference.write_bytes(original)
            insert_working_image(reference, 0, jpeg, "jpeg", .24, .31, .36, .28)
            doc = rust("IMPORT", matter, source=str(page_path), status=201)[0]["id"]
            route = f"/documents/{doc}"
            args = {"page_index": 0, "expected_revision": 1, "rect_x": .24, "rect_y": .31,
                    "rect_width": .36, "rect_height": .28, "filename": fixture.name,
                    "data": b64encode(jpeg).decode()}
            rust("POST", route + "/annotations/image", args, status=201)
            copy = rust("GET", route + "/pdf?editable=true&revision=1")
            compare(reference, b64decode(copy["data"]), jpeg, expected_profile, (kind, rotation, "flattened"))
            if (kind, rotation) == ("no-adobe-marker", 270):
                printed = rust("POST", route + "/print", {"expected_revision": 1, "pages": "all",
                               "orientation": "source", "scale": "actual"})
                compare(print_copy(reference, "all", "source", "actual"), b64decode(printed["data"]),
                        jpeg, expected_profile, (kind, rotation, "print"))
            assert pdf_bytes(rust, doc, 1) == original
            assert rust("POST", route + "/pages/image-overlay", args)["revision"] == 2
            compare(reference, pdf_bytes(rust, doc, 2), jpeg, expected_profile, (kind, rotation, "working"))
            assert pdf_bytes(rust, doc, 1, "original") == original


def redaction_fixture(path, shear):
    with pymupdf.open() as inner, pymupdf.open() as outer:
        form = inner.new_page(width=200, height=90)
        form.insert_text((15, 25), "RESTRICTED")
        form.insert_text((15, 65), "Visible form line")
        page = outer.new_page(width=480, height=320)
        placement = pymupdf.Rect(30, 20, 230, 110)
        page.show_pdf_page(placement, inner, 0)
        # Draw the same resource a second time, below the selected region.
        contents = page.get_contents()[0]
        first = outer.xref_stream(contents)
        if shear:
            first = b"q 1 .12 .06 1 0 -10 cm " + first + b" Q"
        outer.update_stream(contents, first + b"\nq 1 0 0 1 0 -130 cm /fzFrm0 Do Q\n")
        second = outer.new_page(width=480, height=320)
        second.insert_text((30, 150), "Another page")
        outer.save(path)


def redaction(rust, matter, root, shear=False):
    file = root / f"partial-shared-form-{shear}.pdf"
    redaction_fixture(file, shear)
    document = rust("IMPORT", matter, source=str(file), status=201)[0]["id"]
    route = f"/documents/{document}/pages"
    original = pdf_bytes(rust, document, 1)
    with pymupdf.open(stream=original) as before:
        assert before[0].get_text().count("RESTRICTED") == 2
        word = before[0].search_for("RESTRICTED")[0]
    area = pymupdf.Rect(word.x0 - 2, word.y0 - 3, word.x1 + 3, word.y1 + 3)
    fields = {
        "expected_revision": 1, "page_index": 0,
        "rect_x": area.x0 / 480, "rect_y": area.y0 / 320,
        "rect_width": area.width / 480, "rect_height": area.height / 320,
    }
    edited = rust("POST", route + "/redact", fields)
    assert edited["revision"] == 2
    with pymupdf.open(stream=pdf_bytes(rust, document, 2)) as after:
        assert after[0].get_text().count("RESTRICTED") == 1
        assert after[0].get_text().count("Visible form line") == 2
        assert not after[0].get_textbox(area).strip()
        assert after[0].get_pixmap().pixel(int(area.x0 + 5), int(area.y0 + 5)) == (0, 0, 0)
        assert "Another page" in after[1].get_text()
        assert after.get_toc() == []
    assert pdf_bytes(rust, document, 1, "original") == file.read_bytes()


def restore_region_on_large_sheet(rust, matter, root):
    from app.pdf_ops import permanently_redact_region, redaction_restore_footprint, restore_redacted_region

    width, height = 5200, 4400
    fixture = root / "large-sheet-restoration.pdf"
    baseline = root / "large-sheet-restoration-reference.pdf"
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=width, height=height)
        page.draw_rect((200, 410, 296, 479), fill=(1, .6, .2), color=None)
        page.insert_text((213, 445), "SYNTHETIC FIRST", fontsize=10)
        page.insert_text((288, 472), "KEEP LATER", fontsize=10)
        page.insert_text((430, 552), "SYNTHETIC UNRELATED", fontsize=10)
        pdf.set_metadata({"author": "Synthetic private source metadata"})
        pdf.save(fixture)
    original = fixture.read_bytes()
    baseline.write_bytes(original)
    document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
    route = f"/documents/{document}/pages"
    first = (180 / width, 390 / height, 225 / width, 110 / height)
    second = (270 / width, 457 / height, 120 / width, 33 / height)
    coordinates = ("rect_x", "rect_y", "rect_width", "rect_height")
    shrink = tuple(redaction_restore_footprint(fixture, 0, *first))
    history = []
    for revision, selection in enumerate((first, second), start=1):
        region = dict(zip(coordinates, selection, strict=True))
        result = rust("POST", route + "/redact", dict(region, page_index=0, expected_revision=revision))
        assert result["revision"] == revision + 1
        permanently_redact_region(baseline, 0, *selection)
        history.append(region)
    records = rust("GET", route + "/redactions")
    earlier, = (item for item in records if item["before_revision"] == 1)
    with pymupdf.open(stream=pdf_bytes(rust, document, 3)) as redacted:
        assert redacted[0].get_text() == "SYNTHETIC UNRELATED\n"
    restored = rust("POST", f"{route}/redactions/{earlier['id']}/remove", {"expected_revision": 3})
    assert restored["revision"] == 4
    restore_redacted_region(baseline, fixture, 0, *shrink, history[1:])
    result = pdf_bytes(rust, document, 4)
    with pymupdf.open(stream=result) as actual, pymupdf.open(baseline) as expected:
        for pdf in (expected, actual):
            assert set(pdf[0].get_text().splitlines()) == {"SYNTHETIC FIRST", "SYNTHETIC UNRELATED"}
            assert not pdf.metadata["author"]
        for x, y in ((205, 421), (380, 475), (410, 520)):
            a, b = (p[0].get_pixmap(clip=(x, y, x + 1, y + 1)).pixel(0, 0) for p in (expected, actual))
            assert max(abs(s - t) for s, t in zip(a, b, strict=True)) <= 2, (x, y, a, b)
    assert [item["before_revision"] for item in rust("GET", route + "/redactions")] == [2]
    assert pdf_bytes(rust, document, 1, "original") == original


def inline_redaction(rust, matter, root, encoding):
    file = root / f"inline-{encoding}.pdf"
    samples = b" EI " + bytes([230, 40, 80]) * 127 + bytes([50, 180, 95]) * 128
    samples = samples[:384] + samples[-384:]
    if encoding == "stencil":
        dictionary = b"/W 16 /H 16 /IM true /BPC 1 /D [1 0]"
        samples = bytes([255, 255] * 16)
        payload = samples
    elif encoding == "fax":
        dictionary = b"/W 16 /H 16 /CS /G /BPC 1 /F /CCF /DP << /K -1 /Columns 16 /Rows 16 >>"
        samples = bytes([0xAA, 0x55] * 16)
        tiff = BytesIO()
        Image.frombytes("1", (16, 16), samples).save(tiff, format="TIFF", compression="group4")
        with Image.open(tiff) as encoded:
            offset, = encoded.tag_v2[273]
            length, = encoded.tag_v2[279]
        payload = tiff.getvalue()[offset:offset + length]
    else:
        dictionary = b"/W 16 /H 16 /CS /RGB /BPC 8"
        if encoding == "flate":
            dictionary += b" /F /Fl"
            payload = compress(samples)
        else:
            payload = samples
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=240, height=240)
        page.insert_text((15, 25), "Unselected inline page text")
        content = page.get_contents()[0]
        pdf.update_stream(content, pdf.xref_stream(content)
                          + b"\nq .1 .7 .2 rg 160 0 0 160 40 40 cm\nBI " + dictionary
                          + b"\nID " + payload + b"\nEI\nQ\n")
        pdf.save(file)
    document = rust("IMPORT", matter, source=str(file), status=201)[0]["id"]
    fields = {"expected_revision": 1, "page_index": 0, "rect_x": 1 / 6,
              "rect_y": 1 / 6, "rect_width": 1 / 3, "rect_height": 2 / 3}
    edited = rust("POST", f"/documents/{document}/pages/redact", fields)
    assert edited["revision"] == 2
    with pymupdf.open(file) as source, pymupdf.open(stream=pdf_bytes(rust, document, 2)) as result:
        page = result[0]
        assert page.get_text().strip() == "Unselected inline page text"
        assert page.get_pixmap().pixel(80, 110) == (0, 0, 0)
        outside = pymupdf.Rect(132, 45, 196, 197)
        unchanged = source[0].get_pixmap(clip=outside).samples
        assert page.get_pixmap(clip=outside).samples == unchanged, encoding
        assert min(unchanged) < 255, encoding
        assert all(samples not in (result.xref_stream(xref) or b"")
                   and payload not in (result.xref_stream_raw(xref) or b"")
                   for xref in range(1, result.xref_length()))
    assert pdf_bytes(rust, document, 1, "original") == file.read_bytes()


def vector_redaction(rust, matter, root, shear=False):
    file = root / f"overlapping-vectors-{shear}.pdf"
    matrix = b"2 .3 .4 2 18 18" if shear else b"2 0 0 2 18 18"
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=300, height=300)
        page.insert_text((25, 25), "Outside the selected region")
        content = page.get_contents()[0]
        pdf.update_stream(content, pdf.xref_stream(content)
                          + b"\nq " + matrix + b" cm 1 0 0 rg 6 6 110 110 re f Q\n"
                          + b"q 0 1 0 rg 109 140 m 113 196 183 191 194 143 c"
                          + b" 183 125 118 111 109 140 c h f Q\n"
                          + b"q 0 0 1 rg 255 70 20 20 re f Q\n")
        pdf.save(file)
    document = rust("IMPORT", matter, source=str(file), status=201)[0]["id"]
    fields = {"expected_revision": 1, "page_index": 0, "rect_x": 1 / 3,
              "rect_y": 1 / 3, "rect_width": 1 / 3, "rect_height": 1 / 3}
    edited = rust("POST", f"/documents/{document}/pages/redact", fields)
    assert edited["revision"] == 2
    with pymupdf.open(file) as source, pymupdf.open(stream=pdf_bytes(rust, document, 2)) as result:
        page = result[0]
        rendered = page.get_pixmap()
        before = source[0].get_pixmap()
        assert rendered.pixel(150, 150) == (0, 0, 0)
        for xy in ((65, 150), (222, 150), (145, 85), (145, 230), (265, 220)):
            assert rendered.pixel(*xy) == before.pixel(*xy)
        assert page.get_text() == source[0].get_text()
        assert all(item.get("fill") != (0.0, 1.0, 0.0) for item in page.get_drawings())
        assert b"109 140 m" not in page.read_contents()
    assert pdf_bytes(rust, document, 1, "original") == file.read_bytes()


def complex_vector_redaction(rust, matter, root, kind):
    file = root / f"partial-{kind}.pdf"
    shape = {
        "curve": b"1 0 0 rg 35 135 m 32 175 268 175 265 135 c 268 105 32 105 35 135 c h f",
        "stroke": b"0 0 1 RG 13 w 1 J 1 j 25 125 m 70 175 230 175 275 125 c S",
        "pattern": b"/Pattern cs /Grid scn 24 120 252 60 re f",
        "shading": b"24 120 252 60 re W n /Gradient sh",
        "curved-clip": (
            b"24 115 252 70 re 35 135 m 32 175 268 175 265 135 c"
            b" 268 105 32 105 35 135 c h W* n .2 .6 .8 rg 19 110 262 80 re f"
        ),
        "painted-clip": (
            b"0 0 1 rg 13 w 35 135 m 32 175 268 175 265 135 c"
            b" 268 105 32 105 35 135 c h W B .1 .8 .2 rg 19 110 262 80 re f"
        ),
        "stroked-clip": (
            b"0 0 1 RG 14 w 24 120 252 60 re W S 0 1 0 rg 10 110 280 85 re f"
        ),
        "pattern-text": (
            b"/Pattern cs /Grid scn 24 120 252 60 re f"
            b" BT /helv 11 Tf 1 0 0 1 32 252 Tm (Pattern outside selection) Tj ET"
        ),
    }[kind]
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=300, height=300)
        page.insert_text((25, 25), "Independent text above graphics")
        resource_type, reference = pdf.xref_get_key(page.xref, "Resources")
        assert resource_type == "xref"
        resources = int(reference.split()[0])
        if kind.startswith("pattern"):
            pattern = pdf.get_new_xref()
            pdf.update_object(pattern, "<< /Type /Pattern /PatternType 1 /PaintType 1 "
                              "/TilingType 1 /BBox [0 0 12 12] /XStep 12 /YStep 12 /Resources << >> >>")
            pdf.update_stream(pattern, b"0 0.6 0.2 rg 0 0 12 12 re f 1 .8 0 rg 4 4 4 4 re f")
            pdf.xref_set_key(resources, "Pattern", f"<< /Grid {pattern} 0 R >>")
        elif kind == "shading":
            shading = pdf.get_new_xref()
            pdf.update_object(shading, "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [24 120 276 120] "
                              "/Extend [true true] /Function << /FunctionType 2 /Domain [0 1] "
                              "/C0 [1 0 0] /C1 [0 0 1] /N 1 >> >>")
            pdf.xref_set_key(resources, "Shading", f"<< /Gradient {shading} 0 R >>")
        stream = page.get_contents()[0]
        pdf.update_stream(stream, pdf.xref_stream(stream) + b"\nq " + shape + b" Q\n")
        pdf.save(file)
    doc = rust("IMPORT", matter, source=str(file), status=201)[0]["id"]
    fields = {"expected_revision": 1, "page_index": 0, "rect_x": 120 / 300, "rect_y": 115 / 300,
              "rect_width": 60 / 300, "rect_height": 80 / 300}
    edited = rust("POST", f"/documents/{doc}/pages/redact", fields)
    with pymupdf.open(file) as source, pymupdf.open(stream=pdf_bytes(rust, doc, edited["revision"])) as result:
        before = source[0].get_pixmap()
        pixels = result[0].get_pixmap()
        assert pixels.pixel(150, 150) == (0, 0, 0)
        for left, right in [(55, 110), (190, 245)]:
            points = [(x, y) for y in range(130, 175) for x in range(left, right)
                      if before.pixel(x, y) != (255, 255, 255)]
            assert points, kind
            similar = sum(max(abs(a - b) for a, b in zip(before.pixel(x, y), pixels.pixel(x, y))) < 12
                          for x, y in points)
            assert similar > len(points) * .8, (kind, left, similar, len(points))
        assert result[0].get_text() == source[0].get_text()
        assert shape not in result[0].read_contents()
        if kind in ("curved-clip", "painted-clip"):
            assert b"32 175 268 175 265 135 c" not in result[0].read_contents()
        if kind == "stroked-clip":
            # The outer stroke is painted before W constrains the next fill.
            for point, color in [((19, 150), (0, 0, 255)), ((26, 150), (0, 255, 0))]:
                assert pixels.pixel(*point) == before.pixel(*point) == color
        if kind == "pattern-text":
            assert "Pattern outside selection" in result[0].get_text()
        if kind in ("pattern", "shading"):
            resource = b"/PatternType 1" if kind == "pattern" else b"/ShadingType 2"
            assert all(resource.decode() not in result.xref_object(xref, compressed=True)
                       for xref in range(1, result.xref_length()))
    assert pdf_bytes(rust, doc, 1, "original") == file.read_bytes()


def stroke_graphics_state_redaction(rust, matter, root, kind):
    file = root / f"stroke-graphics-state-{kind}.pdf"
    wide = kind != "exterior"
    if wide:
        # The centreline misses the selection, but the width set by /Wide
        # reaches it. /Ink changes opacity without resetting the width.
        region = (.45, .52, .1, .02)
        geometry = b"20 150 m 280 150 l"
        paint = b"q 1 w /Wide gs /Ink gs 0 0 1 RG " + geometry + b" S Q"
    else:
        # A saved graphics state temporarily used a wide stroke. After Q the
        # thin Multiply-blended stroke is well clear of the redaction.
        region = (.5, .5, .25, .25)
        geometry = b"20 260 m 55 286 67 276 95 245 c"
        paint = (b".9 .8 .3 rg 0 0 300 300 re f 5 w "
                 b"q /Wide gs Q q /Ink gs 0 0 1 RG " + geometry + b" S Q")
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=300, height=300)
        if not wide:
            page.insert_text((160, 170), "SELECTED")
        resource_type, resources = pdf.xref_get_key(page.xref, "Resources")
        assert resource_type == "xref"
        wide_state = pdf.get_new_xref()
        style = "/D [[40 40] 0] /LC 1 /LJ 1" if kind == "dashed" else ""
        pdf.update_object(wide_state, f"<< /Type /ExtGState /LW 34 /ML 2 {style} >>")
        pdf.xref_set_key(int(resources.split()[0]), "ExtGState",
                         f"<< /Wide {wide_state} 0 R /Ink << /CA .45 /BM /Multiply >> >>")
        stream = pdf.get_new_xref()
        pdf.update_object(stream, "<<>>")
        pdf.update_stream(stream, paint)
        prior = page.get_contents()
        contents = " ".join(f"{xref} 0 R" for xref in (stream, *prior))
        pdf.xref_set_key(page.xref, "Contents", f"[{contents}]")
        pdf.save(file)

    reference = root / f"reference-stroke-redaction-{kind}.pdf"
    copyfile(file, reference)
    permanently_redact_region(reference, 0, *region)
    document = rust("IMPORT", matter, source=str(file), status=201)[0]["id"]
    x, y, width, height = region
    edited = rust("POST", f"/documents/{document}/pages/redact", {
        "expected_revision": 1, "page_index": 0,
        "rect_x": x, "rect_y": y, "rect_width": width, "rect_height": height,
    })
    assert edited["revision"] == 2
    with pymupdf.open(reference) as expected, pymupdf.open(stream=pdf_bytes(rust, document, 2)) as actual:
        page = actual[0]
        content = b" ".join(page.read_contents().replace(b".000000", b"").split())
        if not wide:
            outside = pymupdf.Rect(0, 0, 120, 95)
            assert page.get_pixmap(clip=outside).samples == expected[0].get_pixmap(clip=outside).samples
            assert geometry in content
            assert not page.get_images()
            assert "SELECTED" not in page.get_text()
        else:
            assert geometry not in content
            assert page.get_pixmap().pixel(150, 159) == (0, 0, 0)
            xref, mask_xref, *_ = page.get_images()[0]
            bounds = page.get_image_rects(xref)[0]
            mask = pymupdf.Pixmap(actual, mask_xref)

            def sample(pixels, x, y):
                return pixels.pixel(int((x - bounds.x0) * pixels.width / bounds.width),
                                    int((y - bounds.y0) * pixels.height / bounds.height))

            assert sample(mask, 150, 159) == (0,)
            assert sample(pymupdf.Pixmap(actual, xref), 150, 159) == (0, 0, 0)
            assert sample(mask, 110, 159)[0] > 0
            before = expected[0].get_pixmap()
            after = page.get_pixmap()
            points = [(110, 159), (255, 159)]
            if kind == "dashed":
                # A gap between dashes and a point at the round starting cap.
                assert sample(mask, 80, 159) == (0,)
                assert sample(mask, 10, 159)[0] > 0
                points.extend([(80, 159), (10, 159)])
            for point in points:
                assert max(abs(a - b) for a, b in zip(before.pixel(*point), after.pixel(*point))) <= 2
    assert pdf_bytes(rust, document, 1, "original") == file.read_bytes()


def combined_vector_redaction(rust, matter, root, kind):
    file = root / f"combined-vector-{kind}.pdf"
    lens = b"25 100 m 24 215 276 215 275 100 c 210 118 100 118 25 100 c h"
    # This independent subpath and its stroke fit wholly inside the selection.
    hidden = b"146.125 150.625 m 154.375 151.625 l 150.125 160.875 l h "
    paint, state, following, transform = {
        "multiply": (lens + b" B", "/ca .6 /CA .6 /BM /Multiply", b"", b""),
        "alpha-is-shape": (lens + b" B", "/ca .35 /CA .7 /AIS true", b"", b""),
        "open": (b"25 100 m 24 215 276 215 275 100 c B", "/ca .35 /CA .7", b"", b""),
        "even-odd": (hidden + lens + b" B*", "/ca .4 /CA .7 /BM /Screen", b"", b""),
        "close": (b"150 177 m 240 177 l 275 120 l 25 120 l 55 177 l b", "/ca .4 /CA .7", b"", b""),
        "sheared-clip": (hidden + b"25 95 250 108 re W* b*", "/ca .4 /CA .7",
                         b" .3 .7 .1 rg 10 154 270 10 re f", b"1 .10 .12 1 -12 -21 cm "),
        "rotated-crop": (lens + b" B", "/ca .6 /CA .6 /BM /Multiply", b"", b""),
        "spot-color": (lens + b" B", "/ca .4 /CA .7", b"", b""),
        "dash": (lens + b" B", "/ca .35 /CA .7 /D [[18 12] 4]", b"", b""),
        "dash-odd": (lens + b" B", "/ca .35 /CA .7 /BM /Multiply", b"",
                     b"1 .08 .14 1 -5 -14 cm [18 12 8] 9 d "),
        "dash-covered-start": (b"146.125 177 m 240 177 l 275 140 l 25 140 l 55 177 l b",
                               "/ca .35 /CA .7 /D [[18 12] 4]", b"", b""),
        "dash-round-dots": (b"25 100 250 100 re B", "/ca .35 /CA .7 /D [[0 20] 6] /LC 1", b"", b""),
        "dash-dense": (lens + b" B", "/ca .35 /CA .7 /D [[.25 .25] 0]", b"", b""),
        "edge-ink": (b"121 126 25 34 re " + lens + b" B", "/ca .35 /CA .7", b"", b""),
    }[kind]
    region = (.4, .3, .2, .4)
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=300, height=300)
        page.insert_text((32, 36), "Independent searchable text")
        _, resources = pdf.xref_get_key(page.xref, "Resources")
        resources = int(resources.split()[0])
        pdf.xref_set_key(resources, "ExtGState", f"<< /Ink << {state} >> >>")
        ink = b".9 .15 .6 rg"
        if kind == "spot-color":
            pdf.xref_set_key(resources, "ColorSpace",
                             "<< /Spot [/Separation /FixtureSpot /DeviceRGB "
                             "<< /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [.9 .15 .6] /N 1 >>] >>")
            ink = b"/Spot cs .7 scn"
        stream, = page.get_contents()
        pdf.update_stream(stream, pdf.xref_stream(stream) + b"\n.5 .8 .3 rg 0 105 300 100 re f q "
                          + transform + b"/Ink gs " + ink + b" .7 .1 .4 RG 12 w " + paint + following + b" Q")
        if kind == "rotated-crop":
            page.set_cropbox(pymupdf.Rect(20, 20, 280, 280))
            page.set_rotation(90)
            region = (70 / 260, 100 / 260, 120 / 260, 60 / 260)
        pdf.save(file)

    reference = root / f"reference-combined-vector-{kind}.pdf"
    copyfile(file, reference)
    permanently_redact_region(reference, 0, *region)
    document = rust("IMPORT", matter, source=str(file), status=201)[0]["id"]
    left, top, width, height = region
    edited = rust("POST", f"/documents/{document}/pages/redact", {
        "expected_revision": 1, "page_index": 0, "rect_x": left, "rect_y": top,
        "rect_width": width, "rect_height": height,
    })
    assert edited["revision"] == 2
    with pymupdf.open(reference) as expected, pymupdf.open(stream=pdf_bytes(rust, document, 2)) as actual:
        page = actual[0]
        before, after = expected[0].get_pixmap(), page.get_pixmap()
        assert page.rect == expected[0].rect
        assert after.pixel(after.width // 2, after.height // 2) == (0, 0, 0)
        assert page.get_text() == expected[0].get_text() == "Independent searchable text\n"
        content = b" ".join(page.read_contents().replace(b".000000", b"").split())
        assert b"24 215 276 215 275 100 c" not in content
        assert b"146.125" not in content and b"154.375" not in content
        if kind in ("dash-dense", "edge-ink"):
            # Dense dashes exceed the vector analysis budget. Original ink
            # next to the selection can also exceed the geometry tolerance.
            assert page.get_images(), kind
        else:
            assert not page.get_images(), kind
            combined = b" B* " if kind in ("even-odd", "sheared-clip") else b" B "
            assert combined in content, kind
            if kind == "spot-color":
                colors = actual.xref_get_key(page.xref, "Resources/ColorSpace")
                assert "/Separation/FixtureSpot" in colors[1].replace(" ", "")
            bounds = pymupdf.Rect(left * after.width, top * after.height,
                                  (left + width) * after.width, (top + height) * after.height)
            if kind == "rotated-crop":
                # The existing Python operation discards the CropBox offset
                # when both a crop and a rotation are present. Compare the
                # preserved exterior directly with the original display.
                with pymupdf.open(file) as source:
                    before = source[0].get_pixmap()
                for x in (int(bounds.x0 + 2), int(bounds.x1 - 2)):
                    for y in (int(bounds.y0 + 2), int(bounds.y1 - 2)):
                        assert after.pixel(x, y) == (0, 0, 0)
            edge_band = bounds + (-1, -1, 1, 1)
            exterior = [(x, y) for y in range(50, after.height - 50) for x in range(15, after.width - 15)
                        if not edge_band.contains(pymupdf.Point(x, y))]
            divergent = sum(max(abs(a - b) for a, b in zip(before.pixel(*point), after.pixel(*point))) > 10
                            for point in exterior)
            assert divergent < 150, (kind, divergent, len(exterior))
    if kind == "multiply":
        route = f"/documents/{document}/pages"
        first, = rust("GET", route + "/redactions")
        for revision, x in enumerate((100 / 300, 164 / 300), start=2):
            edited = rust("POST", route + "/redact", {
                "expected_revision": revision, "page_index": 0, "rect_x": x, "rect_y": .4,
                "rect_width": 36 / 300, "rect_height": 76 / 300,
            })
            assert edited["revision"] == revision + 1
        restored = rust("POST", f"{route}/redactions/{first['id']}/remove", {"expected_revision": 4})
        assert restored["revision"] == 5
        active = rust("GET", route + "/redactions")
        assert {item["after_revision"] for item in active} == {3, 4}
        with pymupdf.open(file) as source, pymupdf.open(stream=pdf_bytes(rust, document, 5)) as output:
            original, restored = source[0].get_pixmap(), output[0].get_pixmap()
            assert max(abs(a - b) for a, b in zip(original.pixel(150, 150), restored.pixel(150, 150))) <= 2
            assert restored.pixel(128, 150) == restored.pixel(173, 150) == (0, 0, 0)
    if kind == "dash-odd":
        staged_dash_copy(rust, file, root)
    assert pdf_bytes(rust, document, 1, "original") == file.read_bytes()


def staged_dash_copy(rust, file, root):
    matter = rust("POST", "/matters", {"name": "Synthetic staged dash redactions"}, status=201)["id"]
    document = rust("IMPORT", matter, source=str(file), status=201, included=True)[0]["id"]
    route = f"/documents/{document}"
    reference = root / "reference-staged-dash.pdf"
    copyfile(file, reference)
    for x in (.36, .56):
        permanently_redact_region(reference, 0, x, .27, .10, .43)
        rust("POST", route + "/annotations", {
            "expected_revision": 1, "page_index": 0, "kind": "redaction", "output_scope": "pdf",
            "rect_x": x, "rect_y": .27, "rect_width": .10, "rect_height": .43,
        }, status=201)
    printed = rust("POST", route + "/print", {"expected_revision": 1, "pages": "1", "orientation": "source"})
    with pymupdf.open(stream=b64decode(printed["data"])) as output, pymupdf.open(reference) as expected:
        assert len(output) == 1 and output[0].rect == expected[0].rect
        assert not output[0].get_images()
        assert b" B " in b" " + b" ".join(output[0].read_contents().split()) + b" "
        assert output[0].get_text() == "Independent searchable text\n"
        reference_pixels, printed_pixels = expected[0].get_pixmap(), output[0].get_pixmap()
        assert printed_pixels.pixel(123, 150) == printed_pixels.pixel(183, 150) == (0, 0, 0)
        assert printed_pixels.pixel(150, 150) == reference_pixels.pixel(150, 150)
        outside = [(x, y) for y in range(100, 190) for x in (*range(40, 105), *range(201, 265))]
        divergent = sum(max(abs(a - b) for a, b in zip(reference_pixels.pixel(*p), printed_pixels.pixel(*p))) > 10
                        for p in outside)
        assert divergent < 150, (divergent, len(outside))
    matter_route = f"/matters/{matter}"
    job = rust("POST", matter_route + "/build", {}, status=202)
    rust("RUN_PENDING", "")
    finished = rust("GET", f"/jobs/{job['id']}")
    assert finished["status"] == "completed", finished
    export, = rust("GET", matter_route + "/exports")
    assert export["id"] == finished["result"]["export_id"]
    inspection = rust("GET", f"{matter_route}/exports/{export['id']}/inspection/combined.pdf")
    with pymupdf.open(stream=b64decode(inspection["data"])) as output:
        page, = [p for p in output if p.rect == pymupdf.Rect(0, 0, 300, 300)]
        assert "Independent searchable text" in page.get_text()
        assert not page.get_images()
        pixels = page.get_pixmap()
        assert pixels.pixel(123, 150) == pixels.pixel(183, 150) == (0, 0, 0)
        assert pixels.pixel(150, 150) == reference_pixels.pixel(150, 150)
    assert pdf_bytes(rust, document, 1) == file.read_bytes()
    assert pdf_bytes(rust, document, 1, "original") == file.read_bytes()


def inactive_soft_mask_redaction(rust, matter, root):
    file = root / "mask-pruning.pdf"
    mask_paint = b".25 g 0 0 95 300 re f .88 g 95 0 205 300 re f"
    live_paint = b".4 g 0 0 300 300 re f"
    curve = b"25 100 m 24 215 276 215 275 100 c 210 118 100 118 25 100 c h"
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=300, height=300)
        page.insert_text((24, 25), "Unrelated searchable text")
        _, reference = pdf.xref_get_key(page.xref, "Resources")
        resources = int(reference.split()[0])
        _, font = pdf.xref_get_key(resources, "Font")
        group = ("<< /Type /XObject /Subtype /Form /BBox [0 0 300 300] "
                 "/Group << /S /Transparency /I true /CS /DeviceGray >>")
        mask = pdf.get_new_xref()
        pdf.update_object(mask, group + f" /Resources << /Font {font} >> >>")
        pdf.update_stream(mask, mask_paint + b" 0 g BT /helv 8 Tf 1 0 0 1 140 152 Tm (MASKED) Tj ET")
        live_mask = pdf.get_new_xref()
        pdf.update_object(live_mask, group + " /Resources <<>> >>")
        pdf.update_stream(live_mask, live_paint)
        pdf.xref_set_key(resources, "ExtGState",
                         f"<< /Ink << /ca .8 /SMask << /S /Luminosity /G {mask} 0 R >> >> "
                         f"/Live << /SMask << /S /Luminosity /G {live_mask} 0 R >> >> >>")
        stream = page.get_contents()[0]
        paint = (b"\n.5 .8 .3 rg 0 105 300 100 re f q /Ink gs .9 .15 .6 rg "
                 + curve + b" W f Q q /Live gs .1 .2 .95 rg 5 250 50 23 re f Q")
        pdf.update_stream(stream, pdf.xref_stream(stream) + paint)
        pdf.save(file)

    reference = root / "reference-mask-pruning.pdf"
    copyfile(file, reference)
    permanently_redact_region(reference, 0, .4, .3, .2, .4)
    document = rust("IMPORT", matter, source=str(file), status=201)[0]["id"]
    edited = rust("POST", f"/documents/{document}/pages/redact", {
        "expected_revision": 1, "page_index": 0,
        "rect_x": .4, "rect_y": .3, "rect_width": .2, "rect_height": .4,
    })
    assert edited["revision"] == 2
    with pymupdf.open(reference) as expected, pymupdf.open(stream=pdf_bytes(rust, document, 2)) as actual:
        page = actual[0]
        before, after = expected[0].get_pixmap(), page.get_pixmap()
        assert after.pixel(150, 150) == (0, 0, 0)
        assert page.get_text() == "Unrelated searchable text\n"
        assert expected[0].get_text(clip=pymupdf.Rect(0, 0, 300, 40)) == page.get_text()
        assert before.pixel(20, 40) != (255, 255, 255)
        for point in ((20, 40), (65, 150), (215, 150)):
            error = max(abs(a - b) for a, b in zip(before.pixel(*point), after.pixel(*point)))
            assert error <= 2, (point, error)
        decoded = [actual.xref_stream(xref) for xref in range(1, actual.xref_length())
                   if actual.xref_is_stream(xref)]
        assert live_paint in decoded
        assert all(mask_paint not in stream and b"(MASKED)" not in stream for stream in decoded)
        image, alpha, *_ = page.get_images()[0]
        bounds, = page.get_image_rects(image)
        for xref, empty in ((image, (0, 0, 0)), (alpha, (0,))):
            pixels = pymupdf.Pixmap(actual, xref)
            x = int((150 - bounds.x0) * pixels.width / bounds.width)
            y = int((150 - bounds.y0) * pixels.height / bounds.height)
            assert pixels.pixel(x, y) == empty
    assert pdf_bytes(rust, document, 1, "original") == file.read_bytes()


def inherited_form_masks_after_redaction(rust, matter, root):
    def make_stream(pdf, dictionary, body):
        xref = pdf.get_new_xref()
        pdf.update_object(xref, dictionary)
        pdf.update_stream(xref, body)
        return xref

    file = root / "inherited-mask-resources.pdf"
    live_paint = b".4 g 0 0 300 300 re f"
    dead_paint = b".7 g 1 1 221 222 re f"
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=300, height=300)
        page.insert_text((24, 25), "Unrelated searchable text")
        resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
        group = ("<< /Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources <<>> "
                 "/Group << /S /Transparency /I true /CS /DeviceGray >> >>")
        live = make_stream(pdf, group, live_paint)
        dead = make_stream(pdf, group, dead_paint)
        form = "<< /Type /XObject /Subtype /Form /BBox [0 230 110 285] >>"
        child = make_stream(pdf, form, b"q /Live gs .1 .2 .95 rg 5 250 50 23 re f Q")
        wrapper = make_stream(pdf, form, b"/Child Do")
        pdf.xref_set_key(resources, "ExtGState",
                         f"<< /Ink << /SMask << /S /Luminosity /G {dead} 0 R >> >> "
                         f"/Live << /SMask << /S /Luminosity /G {live} 0 R >> >> >>")
        pdf.xref_set_key(resources, "XObject", f"<< /Wrapper {wrapper} 0 R /Child {child} 0 R >>")
        stream, = page.get_contents()
        pdf.update_stream(stream, pdf.xref_stream(stream)
                          + b"\n/Wrapper Do q /Ink gs .8 0 .4 rg 130 130 20 20 re f Q")
        pdf.save(file)

    document = rust("IMPORT", matter, source=str(file), status=201)[0]["id"]
    edited = rust("POST", f"/documents/{document}/pages/redact", {
        "expected_revision": 1, "page_index": 0,
        "rect_x": .4, "rect_y": .4, "rect_width": .2, "rect_height": .2,
    })
    assert edited["revision"] == 2
    with pymupdf.open(file) as original, pymupdf.open(stream=pdf_bytes(rust, document, 2)) as saved:
        before, after = original[0].get_pixmap(), saved[0].get_pixmap()
        assert before.pixel(20, 40) != (255, 255, 255)
        # MuPDF's reference redaction drops the inherited child from this
        # malformed-resource fixture. Check unchanged paint against the source.
        assert after.pixel(20, 40) == before.pixel(20, 40)
        assert after.pixel(140, 160) == (0, 0, 0)
        assert saved[0].get_text() == original[0].get_text()
        streams = [saved.xref_stream(xref) for xref in range(1, saved.xref_length())
                   if saved.xref_is_stream(xref)]
        assert any(live_paint in stream for stream in streams)
        assert all(dead_paint not in stream for stream in streams)
    assert pdf_bytes(rust, document, 1, "original") == file.read_bytes()


def marked_content_redaction(rust, matter, root, named):
    file = root / f"marked-content-{named}.pdf"
    evidence = b"SYNTHETIC MARKED CONTENT SECRET"
    visible = b"Covered text"
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=300, height=300)
        page.insert_text((20, 20), "Unrelated searchable text")
        stream = page.get_contents()[0]
        marker = b"/Span << /ActualText (" + evidence + b") /Alt (" + evidence + b") >> BDC"
        if named:
            props = pdf.get_new_xref()
            pdf.update_object(props, "<< /ActualText (" + evidence.decode() + ") >>")
            resource_type, reference = pdf.xref_get_key(page.xref, "Resources")
            assert resource_type == "xref"
            pdf.xref_set_key(int(reference.split()[0]), "Properties", f"<< /Private {props} 0 R >>")
            marker = b"/Span /Private BDC"
        pdf.update_stream(stream, pdf.xref_stream(stream)
                          + b"\n" + marker + b"\nBT /helv 12 Tf 1 0 0 1 95 150 Tm ("
                          + visible + b") Tj ET EMC\n")
        pdf.save(file)
    doc = rust("IMPORT", matter, source=str(file), status=201)[0]["id"]
    fields = {"expected_revision": 1, "page_index": 0, "rect_x": 90 / 300, "rect_y": 130 / 300,
              "rect_width": 120 / 300, "rect_height": 35 / 300}
    changed = rust("POST", f"/documents/{doc}/pages/redact", fields)
    raw = pdf_bytes(rust, doc, changed["revision"])
    with pymupdf.open(stream=raw) as pdf:
        assert pdf[0].get_text().strip() == "Unrelated searchable text"
        for xref in range(1, pdf.xref_length()):
            decoded = pdf.xref_stream(xref) if pdf.xref_is_stream(xref) else b""
            values = (decoded, pdf.xref_object(xref).encode())
            assert all(evidence not in value and evidence.hex().encode() not in value.lower()
                       for value in values)
    assert pdf_bytes(rust, doc, 1, "original") == file.read_bytes()


def native_color_image_redaction(rust, matter, root):
    from app.pdf_ops import permanently_redact_region

    region = (.35, 46 / 120, 37 / 180, 29 / 120)
    fields = dict(zip(("rect_x", "rect_y", "rect_width", "rect_height"), region, strict=True))
    width, height = 17, 12

    def pack(samples):
        row = samples + [0] * (len(samples) % 2)
        return bytes(a * 16 + b for a, b in zip(row[::2], row[1::2], strict=True))

    for kind in ("Separation", "DeviceN", "CalRGB", "CMYK-JPEG"):
        fixture = root / f"image-native-{kind}.pdf"
        placeholder = BytesIO()
        Image.new("RGB", (width, height), "white").save(placeholder, "PNG")
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=180, height=120)
            page.draw_rect(page.rect, color=None, fill=(.2, .4, .6))
            xref = page.insert_image((16, 12, 144, 108), stream=placeholder.getvalue(), keep_proportion=False)
            tint = pdf.get_new_xref()
            if kind == "Separation":
                pdf.update_object(tint, "<< /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [.1 .6 .8] /N 1 >>")
                space = f"[/Separation /SyntheticInk /DeviceRGB {tint} 0 R]"
                row = pack([10] * width)
                bits = 4
            elif kind == "DeviceN":
                pdf.update_object(tint, "<< /FunctionType 0 /Domain [0 1 0 1] /Range [0 1 0 1 0 1] "
                                  "/Size [2 2] /BitsPerSample 8 >>")
                pdf.update_stream(tint, bytes([255, 255, 255, 220, 10, 120, 32, 210, 88, 20, 10, 40]))
                space = f"[/DeviceN [/SyntheticInk /SecondInk] /DeviceRGB {tint} 0 R]"
                row = pack([3] * 4 + [11] * (width - 4))
                bits = 2
            elif kind == "CalRGB":
                space = "[/CalRGB << /WhitePoint [.9505 1 1.089] /Gamma [1 1 1] >>]"
                row = bytes([140, 90, 130]) * width
                bits = 8
            else:
                space = "/DeviceCMYK"
                bits = 8
                cmyk = Image.new("CMYK", (width, height), (96, 25, 159, 45))
                ImageDraw.Draw(cmyk).rectangle((0, 0, 7, height - 1), fill=(35, 110, 160, 20))
                encoded = BytesIO()
                cmyk.save(encoded, "JPEG", quality=100, subsampling=0)
                image_data = encoded.getvalue()
                with Image.open(BytesIO(image_data)) as decoded:
                    raw_jpeg_samples = bytes(255 - sample for sample in decoded.tobytes())
            pdf.update_stream(xref, image_data if kind == "CMYK-JPEG" else row * height,
                              compress=kind != "CMYK-JPEG")
            color = pdf.get_new_xref()
            pdf.update_object(color, space)
            pdf.xref_set_key(xref, "ColorSpace", f"{color} 0 R")
            pdf.xref_set_key(xref, "BitsPerComponent", str(bits))
            if kind == "DeviceN":
                pdf.xref_set_key(xref, "Decode", "[1 0 0 1]")
                pdf.xref_set_key(xref, "Mask", "[0 0 3 3]")
            if kind == "CalRGB":
                mask = pdf.get_new_xref()
                pdf.update_object(mask, f"<< /Type /XObject /Subtype /Image /Width {width} /Height {height} "
                                  "/ColorSpace /DeviceGray /BitsPerComponent 8 /Matte [.25 .25 .5] >>")
                pdf.update_stream(mask, bytes([128]) * (width * height))
                pdf.xref_set_key(xref, "SMask", f"{mask} 0 R")
            if kind == "CMYK-JPEG":
                names = " ".join(f"{value - 1} {value + 1}" for value in raw_jpeg_samples[:4])
                pdf.xref_set_key(xref, "Filter", "/DCTDecode")
                pdf.xref_set_key(xref, "Decode", "[1 0 1 0 1 0 1 0]")
                pdf.xref_set_key(xref, "Mask", f"[{names}]")
            pdf.save(fixture)
        original = fixture.read_bytes()
        reference = root / f"image-native-reference-{kind}.pdf"
        reference.write_bytes(original)
        permanently_redact_region(reference, 0, *region)
        document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
        route = f"/documents/{document}"
        request = dict(fields, page_index=0, expected_revision=1)

        def check(result):
            with (pymupdf.open(stream=original) as imported, pymupdf.open(reference) as baseline,
                  pymupdf.open(stream=result) as native):
                before, expected, after = (pdf[0].get_pixmap() for pdf in (imported, baseline, native))
                assert expected.pixel(80, 60) == after.pixel(80, 60) == (0, 0, 0), kind
                for x, y in ((27, 31), (58, 32), (137, 98)):
                    if kind == "CMYK-JPEG":
                        assert max(abs(a - b) for a, b in zip(after.pixel(x, y), before.pixel(x, y), strict=True)) <= 2
                    else:
                        assert after.pixel(x, y) == before.pixel(x, y), (kind, x, y)
                image, = native[0].get_images(full=True)
                assert image[1] and image[4] == bits, (kind, image)
                resource, opacity = native.xref_stream(image[0]), native.xref_stream(image[1])
                stride = 4 * width if kind == "CMYK-JPEG" else len(row)
                if kind == "CMYK-JPEG":
                    assert max(abs(a - b) for a, b in zip(resource[:stride], raw_jpeg_samples[:stride], strict=True)) <= 1
                    assert opacity[0] == 0 and opacity[width - 1] == 255
                else:
                    assert resource[:stride] == row
                offset = 32 if kind == "CMYK-JPEG" else 24 if bits == 8 else 4
                pixel = resource[6 * stride + offset]
                assert pixel == 0 and opacity[6 * width + 8] == 0, kind
                removed_samples = raw_jpeg_samples if kind == "CMYK-JPEG" else row * height
                assert all(native.xref_stream(xref) != removed_samples for xref in range(1, native.xref_length())
                           if native.xref_is_stream(xref)), kind

        if kind == "DeviceN":
            rust("POST", route + "/annotations", dict(request, kind="redaction", output_scope="pdf"), status=201)
            detached = rust("GET", route + "/pdf?editable=true&revision=1")
            check(b64decode(detached["data"]))
            assert pdf_bytes(rust, document, 1) == original

        assert rust("POST", route + "/pages/redact", request)["revision"] == 2
        check(pdf_bytes(rust, document, 2))
        assert pdf_bytes(rust, document, 1, "original") == original


def jpeg2000_redaction(rust, matter, root):
    from app.pdf_ops import permanently_redact_region

    region = (63 / 180, 46 / 120, 37 / 180, 29 / 120)
    fields = dict(zip(("rect_x", "rect_y", "rect_width", "rect_height"), region, strict=True))
    for kind, format_mode, value, mask_mode in (
        ("opaque", "RGB", (200, 40, 100), 0),
        ("straight", "RGBA", (200, 40, 100, 128), 0),
        ("gray", "LA", (160, 128), 1),
        ("associated", "RGBA", (100, 20, 50, 128), 2),
    ):
        encoded = BytesIO()
        Image.new(format_mode, (16, 12), value).save(encoded, "JPEG2000")
        jpx = bytearray(encoded.getvalue())
        if kind == "associated":
            marker = jpx.index(b"cdef")
            entries = int.from_bytes(jpx[marker + 4:marker + 6], "big")
            alpha_type = marker + 6 + 6 * (entries - 1) + 2
            assert jpx[alpha_type:alpha_type + 2] == b"\0\1"
            jpx[alpha_type:alpha_type + 2] = b"\0\2"
        base = BytesIO()
        Image.new("RGB", (16, 12), "white").save(base, "PNG")
        fixture = root / f"partial-jpeg2000-{kind}.pdf"
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=180, height=120)
            page.draw_rect(page.rect, color=None, fill=(.2, .4, .6))
            xref = page.insert_image((16, 12, 144, 108), stream=base.getvalue())
            pdf.update_stream(xref, bytes(jpx), compress=False)
            for key, value in (("Filter", "/JPXDecode"), ("ColorSpace", "null"),
                               ("BitsPerComponent", "null"), ("SMaskInData", str(mask_mode))):
                pdf.xref_set_key(xref, key, value)
            pdf.save(fixture)
        original = fixture.read_bytes()
        expected = root / f"partial-jpeg2000-reference-{kind}.pdf"
        expected.write_bytes(original)
        permanently_redact_region(expected, 0, *region)
        document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
        result = rust("POST", f"/documents/{document}/pages/redact", dict(fields, expected_revision=1, page_index=0))
        assert result["revision"] == 2
        with pymupdf.open(expected) as baseline, pymupdf.open(stream=pdf_bytes(rust, document, 2)) as native:
            source, output = baseline[0].get_pixmap(), native[0].get_pixmap()
            assert source.pixel(80, 60) == output.pixel(80, 60) == (0, 0, 0), kind
            image, = native[0].get_images(full=True)
            color = pymupdf.Pixmap(native, image[0])
            assert native.xref_get_key(image[0], "Filter") == ("name", "/FlateDecode")
            assert color.pixel(8, 6) == (0, 0, 0), kind
            if kind == "associated":
                # The independently decoded raw replacement must retain the
                # encoded associated color when recomposited at opacity 128.
                assert max(abs(a - b) for a, b in zip(color.pixel(0, 0), (199, 40, 100), strict=True)) <= 1
            else:
                assert max(abs(a - b) for a, b in zip(source.pixel(30, 30), output.pixel(30, 30), strict=True)) <= 2, kind
            if kind != "opaque":
                mask = pymupdf.Pixmap(native, image[1])
                assert mask.pixel(0, 0) == (128,) and mask.pixel(8, 6) == (255,), kind
        assert pdf_bytes(rust, document, 1, "original") == original


def different_mask_redaction(rust, matter, root, kind="soft"):
    file = root / f"image-mask-{kind}.pdf"
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=300, height=300)
        page.insert_text((20, 20), "Independent mask fixture line")
        resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
        photo = pdf.get_new_xref()
        if kind == "indexed":
            pdf.update_object(photo, "<< /Type /XObject /Subtype /Image "
                                      "/ColorSpace [/Indexed /DeviceRGB 3 <FFFFFF2060C020B050E04020>] "
                                      "/BitsPerComponent 2 /Width 8 /Height 8 /Mask [0 0] >>")
            pdf.update_stream(photo, bytes([0, 0x55]) * 4 + bytes([0xAA, 0xFF]) * 4)
        else:
            mask = pdf.get_new_xref()
            matte = " /Matte [1 1 1]" if kind == "matte" else ""
            dimension = 8 if matte else 2
            pdf.update_object(mask, "<< /Type /XObject /Subtype /Image /ColorSpace /DeviceGray "
                                   f"/BitsPerComponent 8 /Width {dimension} /Height {dimension}{matte} >>")
            pdf.update_stream(mask, bytes([150] * 64 if matte else [255, 150, 150, 255]))
            pdf.update_object(photo, f"<< /Type /XObject /Subtype /Image /ColorSpace /DeviceRGB "
                                    f"/BitsPerComponent 8 /Width 8 /Height 8 /SMask {mask} 0 R >>")
            color = [119, 169, 211] if matte else [28, 112, 180]
            pdf.update_stream(photo, bytes(color) * 64)
        pdf.xref_set_key(resources, "XObject", f"<< /Photo {photo} 0 R >>")
        content = page.get_contents()[0]
        pdf.update_stream(content, pdf.xref_stream(content) + b"\nq 240 0 0 160 30 70 cm /Photo Do Q\n")
        pdf.save(file)
    doc = rust("IMPORT", matter, source=str(file), status=201)[0]["id"]
    fields = {"expected_revision": 1, "page_index": 0, "rect_x": 120 / 300, "rect_y": 115 / 300,
              "rect_width": 60 / 300, "rect_height": 80 / 300}
    changed = rust("POST", f"/documents/{doc}/pages/redact", fields)
    with pymupdf.open(file) as source, pymupdf.open(stream=pdf_bytes(rust, doc, changed["revision"])) as pdf:
        before, after = source[0].get_pixmap(), pdf[0].get_pixmap()
        for x, y in [(70, 100), (70, 200), (230, 100), (230, 200)]:
            expected, actual = before.pixel(x, y), after.pixel(x, y)
            assert max(abs(a - b) for a, b in zip(expected, actual)) < 6, (kind, x, y, expected, actual)
        assert after.pixel(150, 150) == (0, 0, 0)
        assert pdf[0].get_text() == source[0].get_text()
    assert pdf_bytes(rust, doc, 1, "original") == file.read_bytes()


def lossless_compression(rust, matter, root):
    file = root / "lossless-document-graph.pdf"
    with pymupdf.open() as pdf:
        for _ in range(2):
            pdf.new_page().insert_text((30, 35), "Repeated page content")
        for page in pdf:
            stream = page.get_contents()[0]
            pdf.update_stream(stream, pdf.xref_stream(stream) + b"\n%" + b"x" * 16000 + b"\n", compress=False)
        dest = f"[{pdf[1].xref} 0 R /XYZ 61 710 1.75]"
        pdf.xref_set_key(pdf.pdf_catalog(), "Dests", f"<< /Target {dest} >>")
        pdf.xref_set_key(pdf.pdf_catalog(), "OpenAction", dest)
        link = pdf.get_new_xref()
        pdf.update_object(link, "<< /Type /Annot /Subtype /Link /Rect [25 780 100 812] /Dest /Target >>")
        pdf.xref_set_key(pdf[0].xref, "Annots", f"[{link} 0 R]")
        pdf.set_metadata({"title": "Lossless synthetic reference"})
        pdf.save(file, deflate=False)
    doc = rust("IMPORT", matter, source=str(file), status=201)[0]["id"]
    job = rust("POST", f"/documents/{doc}/compress", {"preset": "lossless", "expected_revision": 1}, status=202)
    rust("RUN_PENDING", "")
    done = rust("GET", f"/jobs/{job['id']}")
    assert done["status"] == "completed" and done["result"]["changed"], done
    output = pdf_bytes(rust, doc, 2)
    assert len(output) < file.stat().st_size
    with pymupdf.open(file) as source, pymupdf.open(stream=output) as result:
        assert len(result) == len(source)
        assert result.metadata["title"] == source.metadata["title"]
        assert result[0].get_contents() == result[1].get_contents()
        assert result[0].get_links()[0]["nameddest"] == "Target"
        assert "/XYZ 61 710 1.75" in result.xref_get_key(result.pdf_catalog(), "OpenAction")[1]
        for original, current in zip(source, result):
            assert original.read_contents() == current.read_contents()
            assert original.get_pixmap().samples == current.get_pixmap().samples
    assert pdf_bytes(rust, doc, 1, "original") == file.read_bytes()


def compression_image_placements(rust, matter, root):
    from random import Random
    import zlib
    from app.pdf_ops import compress_pdf

    fixture = root / "compression-image-placement.pdf"
    width, height = 480, 320
    samples = Random(17).randbytes(width * height * 3)
    with pymupdf.open() as pdf:
        for _ in range(2):
            pdf.new_page(width=580, height=520).insert_text((20, 32), "Text outside the images")

        def image(data, *, flate=False, bits=8):
            xref = pdf.get_new_xref()
            space = "/DeviceGray" if bits == 1 else "/DeviceRGB"
            pdf.update_object(xref, f"<< /Type /XObject /Subtype /Image /Width {width} "
                                   f"/Height {height} /ColorSpace {space} /BitsPerComponent {bits} >>")
            pdf.update_stream(xref, zlib.compress(data) if flate else data, compress=False)
            if flate:
                pdf.xref_set_key(xref, "Filter", "/FlateDecode")
            return xref

        large = image(samples)
        same_page = image(samples, flate=True)
        other_page = image(samples)
        unused = image(samples[::-1])
        hidden = image(bytes(255 - sample for sample in samples))
        bitonal = image(b"\x55" * (width // 8 * height), bits=1)
        layer = pdf.add_ocg("Hidden image", on=False)
        first = pdf[0]
        resources = int(pdf.xref_get_key(first.xref, "Resources")[1].split()[0])
        pdf.xref_set_key(resources, "XObject", f"<< /Large {large} 0 R /SamePage {same_page} 0 R "
                         f"/Unused {unused} 0 R /Hidden {hidden} 0 R /Bitonal {bitonal} 0 R >>")
        pdf.xref_set_key(resources, "Properties", f"<< /HiddenLayer {layer} 0 R >>")
        stream, = first.get_contents()
        pdf.update_stream(stream, pdf.xref_stream(stream) + b"""
            q 320 0 0 210 25 230 cm /Large Do Q
            q 48 0 0 32 380 370 cm /SamePage Do Q
            q 64 0 0 42 380 290 cm /Bitonal Do Q
            /OC /HiddenLayer BDC q 80 0 0 55 380 190 cm /Hidden Do Q EMC
        """)

        second = pdf[1]
        resources = int(pdf.xref_get_key(second.xref, "Resources")[1].split()[0])
        form = pdf.get_new_xref()
        pdf.update_object(form, "<< /Type /XObject /Subtype /Form /BBox [0 0 200 150] >>")
        pdf.update_stream(form, b"q 96 0 0 64 25 60 cm /Small Do Q")
        pdf.xref_set_key(resources, "XObject", f"<< /Small {other_page} 0 R /Picture {form} 0 R >>")
        parent = int(pdf.xref_get_key(second.xref, "Parent")[1].split()[0])
        pdf.xref_set_key(parent, "Resources", f"{resources} 0 R")
        pdf.xref_set_key(second.xref, "Resources", "null")
        stream, = second.get_contents()
        pdf.update_stream(stream, pdf.xref_stream(stream) + b"\nq 1 0 0 1 0 240 cm /Picture Do Q\n")
        pdf.save(fixture)

    original = fixture.read_bytes()
    reference = fixture.with_name("compression-image-placement-python.pdf")
    reference.write_bytes(original)
    expected = compress_pdf(reference, "small")
    document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
    job = rust("POST", f"/documents/{document}/compress",
               {"preset": "small", "expected_revision": 1}, status=202)
    rust("RUN_PENDING", "")
    state = rust("GET", f"/jobs/{job['id']}")
    assert state["status"] == "completed", state
    result = state["result"]
    for field, value in (("changed", True), ("images_rewritten", 3), ("images_skipped", 3)):
        assert result[field] == expected[field] == value
    assert result["after_bytes"] < result["before_bytes"] == len(original)
    for output in (pdf_bytes(rust, document, 2), reference.read_bytes()):
        with pymupdf.open(stream=output, filetype="pdf") as pdf:
            dimensions = [sorted((row["width"], row["height"]) for row in page.get_image_info())
                          for page in pdf]
            assert dimensions == [[(480, 320)] * 3, [(200, 133)]], dimensions
            assert [page.get_text().strip() for page in pdf] == ["Text outside the images"] * 2
    assert pdf_bytes(rust, document, 1, "original") == original


def compression_color_spaces(rust, matter, root):
    import base64
    import zlib
    from app.pdf_ops import compress_pdf

    width, height = 256, 128
    calibrated = "[/CalRGB << /WhitePoint [.9505 1 1.089] /Gamma [1.3 2 2.4] >>]"
    for variant in ("scoped-rgb", "managed-rgb"):
        fixture = root / f"compression-{variant}.pdf"
        managed = variant == "managed-rgb"
        if managed:
            samples = b"\0" * (width * height * 4)
        else:
            samples = bytes((x + y * 3 + channel * 60) % 256
                            for y in range(height) for x in range(width) for channel in range(3))
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=420, height=320)
            page.insert_text((30, 20), "Text above calibrated images")
            resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
            image = pdf.get_new_xref()
            space = "/DeviceCMYK" if managed else "/DeviceRGB"
            pdf.update_object(image, f"<< /Type /XObject /Subtype /Image /Width {width} /Height {height} "
                                     f"/ColorSpace {space} /BitsPerComponent 8 >>")
            pdf.update_stream(image, samples, compress=False)
            form = pdf.get_new_xref()
            pdf.update_object(form, f"<< /Type /XObject /Subtype /Form /BBox [0 0 240 150] "
                                    f"/Resources << /XObject << /Image {image} 0 R >> "
                                    f"/ColorSpace << /DefaultRGB {calibrated} >> >> >>")
            pdf.update_stream(form, b"q 60 0 0 30 20 20 cm /Image Do Q")
            forms = f"/Photo {form} 0 R"
            if not managed:
                alternate = pdf.get_new_xref()
                alternate_space = calibrated.replace("[1.3 2 2.4]", "[2.4 1.3 2]")
                pdf.update_object(alternate, f"<< /Type /XObject /Subtype /Form /BBox [0 0 240 150] "
                                             f"/Resources << /XObject << /Image {image} 0 R >> "
                                             f"/ColorSpace << /DefaultRGB {alternate_space} >> >> >>")
                pdf.update_stream(alternate, b"q 50 0 0 25 20 20 cm /Image Do Q")
                forms += f" /Alternate {alternate} 0 R"
            pdf.xref_set_key(resources, "XObject", f"<< {forms} >>")
            pdf.xref_set_key(resources, "ColorSpace", f"<< /DefaultRGB {calibrated} /InlineRGB /DeviceRGB >>")
            stream, = page.get_contents()
            commands = b"\nq 1 0 0 1 20 150 cm /Photo Do Q\n"
            if not managed:
                commands += b"q 1 0 0 1 20 85 cm /Alternate Do Q\n"
                payload = base64.a85encode(zlib.compress(samples)) + b"~>"
                commands += (f"q 180 0 0 90 120 70 cm BI /W {width} /H {height} /BPC 8 "
                             "/CS /InlineRGB /F [/A85 /Fl] ID\n").encode() + payload + b"\nEI Q\n"
            pdf.update_stream(stream, pdf.xref_stream(stream) + commands)
            pdf.save(fixture)

        original = fixture.read_bytes()
        document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
        job = rust("POST", f"/documents/{document}/compress",
                   {"preset": "small", "expected_revision": 1}, status=202)
        rust("RUN_PENDING", "")
        state = rust("GET", f"/jobs/{job['id']}")
        assert state["status"] == "completed", state
        assert state["result"]["changed"] and state["result"]["images_rewritten"] == 1, state
        output = pdf_bytes(rust, document, 2)
        if not managed:
            reference = fixture.with_name("compression-scoped-rgb-python.pdf")
            reference.write_bytes(original)
            expected = compress_pdf(reference, "small")
            assert expected["changed"] and expected["images_rewritten"] == 1
            for data in (output, reference.read_bytes()):
                with pymupdf.open(stream=data, filetype="pdf") as pdf:
                    # The large named inline paint contributes to the largest
                    # placement for the XObject with the same decoded samples.
                    assert [(row["width"], row["height"]) for row in pdf[0].get_image_info()] == [
                        (width, height)] * 3
        with pymupdf.open(stream=output, filetype="pdf") as pdf:
            xref = pdf[0].get_images(full=True)[0][0]
            kind, color = pdf.xref_get_key(xref, "ColorSpace")
            if managed:
                assert kind == "array" and color.startswith("[/ICCBased "), color
                profile = int(color.split()[1])
                assert pdf.xref_get_key(profile, "N") == ("int", "3")
                assert pdf.xref_stream(profile)[36:40] == b"acsp"
            else:
                assert (kind, color) == ("name", "/DeviceRGB")
                local_colors = [pdf.xref_get_key(xref, "Resources/ColorSpace/DefaultRGB")[1]
                                for xref, _, parent, _ in pdf[0].get_xobjects() if parent == 0]
                assert len(set(local_colors)) == 2
            assert pdf[0].get_text().strip() == "Text above calibrated images"
        assert pdf_bytes(rust, document, 1, "original") == original


def painted_image_admission(rust, matter, root):
    from app.pdf_ops import pdf_has_unsearchable_images

    for kind in ("inline", "hidden-layer", "inherited-form"):
        fixture = root / f"painted-image-admission-{kind}.pdf"
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=180, height=180)
            image = page.insert_image(pymupdf.Rect(25, 25, 90, 90), pixmap=pymupdf.Pixmap(
                pymupdf.csRGB, 2, 2, b"\x13\x25\x46" * 4, False))
            resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
            stream, = page.get_contents()
            commands = b"q 64 0 0 64 25 25 cm /Image Do Q"
            if kind == "inline":
                pdf.xref_set_key(resources, "XObject", "null")
                commands = b"q 64 0 0 64 25 25 cm BI /W 1 /H 1 /BPC 8 /CS /G ID\n\x26\nEI Q"
            elif kind == "hidden-layer":
                hidden = pdf.add_ocg("Hidden scan", on=False)
                pdf.xref_set_key(resources, "XObject", f"<< /Image {image} 0 R >>")
                pdf.xref_set_key(resources, "Properties", f"<< /Hidden {hidden} 0 R >>")
                commands = b"/OC /Hidden BDC " + commands + b" EMC"
            else:
                form = pdf.get_new_xref()
                pdf.update_object(form, "<< /Type /XObject /Subtype /Form /BBox [0 0 180 180] >>")
                pdf.update_stream(form, commands)
                pdf.xref_set_key(resources, "XObject", f"<< /Image {image} 0 R /Form {form} 0 R >>")
                parent = int(pdf.xref_get_key(page.xref, "Parent")[1].split()[0])
                pdf.xref_set_key(parent, "Resources", f"{resources} 0 R")
                pdf.xref_set_key(page.xref, "Resources", "null")
                commands = b"/Form Do"
            pdf.update_stream(stream, commands)
            pdf.save(fixture)
        expected = pdf_has_unsearchable_images(fixture)
        assert expected == (kind != "hidden-layer")
        document = rust("IMPORT", matter, source=str(fixture), status=201)[0]
        assert document["ocr_status"] == ("needs_ocr" if expected else "not_requested")
        assert pdf_bytes(rust, document["id"], 1, "original") == fixture.read_bytes()


def paragraph(rust, matter, root, variant):
    file = root / f"{variant}-font.pdf"
    original, replacement = {
        "hindi": ("कमल", "कलम"),
        "shaped-hindi": ("कमल", "शक्ति परीक्षा"),
        "shaped-javanese": ("ꦲꦤ", "ꦲꦤ꧀ꦕꦫ"),
        "shaped-arabic": ("سلام", "محكمة"),
        "predefined": ("日本語", "日本人"),
    }.get(variant, ("A trial example", "A real example"))
    reshaped = {"shaped-hindi": "परीक्षा शक्ति", "shaped-javanese": "ꦕꦫꦤ꧀ꦲ",
                "shaped-arabic": "السلام"}.get(variant)
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=470, height=500)
        name = "japan" if variant == "predefined" else "fixtureFont"
        if variant == "predefined":
            font_ref = page.insert_font(fontname=name)
        else:
            if variant in ("hindi", "shaped-hindi"):
                font = FONT.read_bytes()
            elif variant == "shaped-javanese":
                font = pymupdf.Font(script=pymupdf.mupdf.UCDN_SCRIPT_JAVANESE).buffer
            elif variant == "shaped-arabic":
                font = pymupdf.Font(script=pymupdf.mupdf.UCDN_SCRIPT_ARABIC).buffer
            else:
                font = pymupdf.Font("helv").buffer
            font_ref = page.insert_font(fontname=name, fontbuffer=font, set_simple=variant == "simple")
        page.insert_text((40, 65), original, fontname=name)
        page.insert_text((40, 200), "Independent lower line", fontname="cour")
        if variant in ("custom", "inherited", "named-base"):
            encoding = pdf.get_new_xref()
            pdf.update_object(encoding, "<< /Type /CMap /CMapName /Identity-H >>")
            pdf.update_stream(encoding, b"""begincmap
/CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> def
/CMapName /Identity-H def
/CMapType 1 def
1 begincodespacerange <0000> <FFFF> endcodespacerange
1 begincidrange <0000> <FFFF> 0 endcidrange
endcmap""")
            if variant == "custom":
                pdf.xref_set_key(font_ref, "Encoding", f"{encoding} 0 R")
            else:
                derived = pdf.get_new_xref()
                fields = "/UseCMap " + str(encoding) + " 0 R" if variant == "inherited" else ""
                pdf.update_object(derived, f"<< /Type /CMap /CMapName /Inherited-H {fields} >>")
                directive = b"/Identity-H usecmap\n" if variant == "named-base" else b""
                pdf.update_stream(derived, b"begincmap\n" + directive
                                  + b"/CMapName /Inherited-H def\n/WMode 0 def\nendcmap\n")
                pdf.xref_set_key(font_ref, "Encoding", f"{derived} 0 R")
        pdf.save(file)
    document = rust("IMPORT", matter, source=str(file), status=201)[0]["id"]
    route = f"/documents/{document}/pages"
    runs = rust("GET", route + "/0/text-runs")["runs"]
    run = next((item for item in runs if item["bbox"][1] < 100), None)
    assert run is not None, runs
    fields = {"expected_revision": 1, "page_index": 0,
              "span_index": run["index"], "expected_text": run["text"]}
    preview = rust("POST", route + "/paragraph-trial-preview", fields)
    edited = rust("POST", route + "/paragraph-trial",
                  {**fields, "expected_paragraph": preview["text"], "replacement": replacement})
    revision = edited["revision"]
    assert revision == 2
    assert edited["edit_layout"]["font_substituted"] is False, (variant, run, edited["edit_layout"])
    with pymupdf.open(stream=pdf_bytes(rust, document, 2)) as result:
        text = result[0].get_text().replace("\u200b", "")
        assert "".join(replacement.split()) in "".join(text.split()), text
        assert original not in text.splitlines(), text
        assert "Independent lower line" in text
        rect = result[0].search_for(replacement)[0] if variant in ("inherited", "predefined") else None
        if reshaped:
            assert b"/MatterDraftShaped" in b"".join(
                result.xref_stream(xref) or b"" for xref in range(1, result.xref_length())
                if result.xref_is_stream(xref) and "CMapName" in result.xref_object(xref)
            )
            rect = pymupdf.Rect(35, 46, 150, 78)
    if reshaped:
        runs = rust("GET", route + "/0/text-runs")["runs"]
        run = next(item for item in runs if item["text"] == replacement)
        fields = {**fields, "expected_revision": revision, "span_index": run["index"],
                  "expected_text": run["text"]}
        preview = rust("POST", route + "/paragraph-trial-preview", fields)
        edited = rust("POST", route + "/paragraph-trial", {
            **fields, "expected_paragraph": preview["text"], "replacement": reshaped,
        })
        revision = edited["revision"]
        assert revision == 3
        with pymupdf.open(stream=pdf_bytes(rust, document, revision)) as result:
            text = result[0].get_text().replace("\u200b", "")
            assert reshaped in text and replacement not in text
    if rect is not None:
        changed = rust("POST", route + "/redact", {
            "expected_revision": revision, "page_index": 0,
            "rect_x": (rect.x0 - 1) / 470, "rect_y": (rect.y0 - 2) / 500,
            "rect_width": (rect.width + 2) / 470, "rect_height": (rect.height + 4) / 500,
        })
        with pymupdf.open(stream=pdf_bytes(rust, document, changed["revision"])) as redacted:
            assert redacted[0].get_text().strip() == "Independent lower line"
            if reshaped:
                for xref in range(1, redacted.xref_length()):
                    kind, value = redacted.xref_get_key(xref, "CMapName")
                    if kind == "name" and value == "/MatterDraftShaped":
                        assert b"beginbfchar" not in redacted.xref_stream(xref)
    assert pdf_bytes(rust, document, 1, "original") == file.read_bytes()


def paragraph_trial_policy(root):
    current_flag = "MATTERDRAFT_ENABLE_PDFIUM_TRIAL"
    legacy_flag = "COURTBUNDLE_ENABLE_PDFIUM_TRIAL"
    settings = (
        ("default", {}, True),
        ("legacy-disabled", {legacy_flag: "0"}, False),
        ("current-disabled", {current_flag: "0", legacy_flag: "1"}, False),
        ("current-enabled", {current_flag: "1", legacy_flag: "0"}, True),
        ("current-empty", {current_flag: "", legacy_flag: "0"}, True),
    )
    sources = {}
    long_line = " ".join(["A paragraph line"] * 12)
    paint = {"stroked": {"render_mode": 1}, "filled-and-stroked": {"render_mode": 2},
             "translucent": {"fill_opacity": .5}, "rounded-opaque": {"fill_opacity": .999},
             "stroke-alpha": {"stroke_opacity": .5}}
    for kind in ("ordinary", "rotated", "oversized", "long-paragraph", "mixed-opacity", "covered-by-link", "bottom-flow", *paint):
        filename = root / f"paragraph-policy-{kind}.pdf"
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=1100)
            if kind == "long-paragraph":
                for line in range(12):
                    page.insert_text((36, 60 + line * 14), long_line, fontsize=10)
            else:
                page.insert_text((36, 795 if kind == "bottom-flow" else 60), "Original paragraph",
                                 fontsize=12, **paint.get(kind, {}))
            if kind == "covered-by-link":
                page.insert_link({"kind": pymupdf.LINK_URI, "uri": "https://example.invalid/paragraph",
                                  "from": pymupdf.Rect(30, 48, 140, 66)})
            if kind == "bottom-flow":
                page.insert_text((36, 825), "BOTTOM FOLLOWER", fontsize=12)
            if kind == "mixed-opacity":
                page.insert_text((36, 75), "Translucent line", fontsize=12, fill_opacity=.5)
                page.insert_text((36, 90), "Restored opaque fill", fontsize=12)
            if kind == "rotated":
                page.set_rotation(90)
            if kind == "oversized":
                stream = pdf.get_new_xref()
                pdf.update_object(stream, "<< /Type /Metadata /Subtype /XML >>")
                pdf.update_stream(stream, b"<fixture>" + b" " * (16 * 1024 * 1024) + b"</fixture>", compress=False)
                pdf.xref_set_key(pdf.pdf_catalog(), "Metadata", f"{stream} 0 R")
            pdf.save(filename)
        sources[kind] = filename
    styled_flows = ("styled-flow-relative", "styled-flow-matrix", "styled-flow-line-steps", "styled-flow-quoted")
    for kind in styled_flows:
        filename = root / f"paragraph-policy-{kind}.pdf"
        with pymupdf.open() as pdf:
            page = pdf.new_page()
            page.insert_text((72, 45), "UNTOUCHED HEADING", fontsize=12)
            page.insert_text((72, 100), "Short first paragraph", fontsize=12)
            if kind == "styled-flow-relative":
                page.insert_text((72, 127), "NEXT PARAGRAPH", fontsize=12, render_mode=1,
                                 color=(.6, .15, .05), stroke_opacity=.65)
                page.insert_text((72, 145), "STAYS IN ORDER", fontsize=12,
                                 color=(.1, .25, .8), fill_opacity=.4)
            else:
                resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
                pdf.xref_set_key(resources, "ExtGState",
                                 "<< /Stroke << /CA .65 >> /Fill << /ca .4 >> /Opaque << /ca 1 >> >>")
                contents = pdf.get_new_xref()
                pdf.update_object(contents, "<< >>")
                height = page.rect.height
                stroke = "1 Tr /Stroke gs .6 .15 .05 RG"
                fill = "0 Tr /Fill gs .1 .25 .8 rg"
                reset = "0 Tr /Opaque gs 0 g"
                if kind == "styled-flow-matrix":
                    y0 = (height - 127 - 40) / .95
                    y1 = (height - 145 - 40) / .95
                    lines = f"""{stroke} 1 0 0 1 72 {y0:.8f} Tm (NEXT PARAGRAPH) Tj
                    {fill} 1 0 0 1 72 {y1:.8f} Tm (STAYS IN ORDER) Tj"""
                else:
                    scale = .95 * .9
                    start = (height - 25 - 40) / .95
                    first = (127 - 25) / scale
                    second = (145 - 127) / scale
                    restore = (145 - 66) / scale
                    lines = f"1 0 0 .9 72 {start:.8f} Tm (FIXED START) Tj\n"
                    if kind == "styled-flow-line-steps":
                        lines += f"""0 {second - first:.8f} Td
                        {stroke} 0 {-second:.8f} TD (NEXT PARAGRAPH) Tj
                        0 -4 Td 0 4 Td
                        {fill} T* (STAYS IN ORDER) Tj
                        {reset} 0 {restore:.8f} Td (FIXED END) Tj"""
                    else:
                        lines += f"""{stroke} {first:.8f} TL .2 .09 (NEXT PARAGRAPH) "
                        {fill} {second:.8f} TL (STAYS IN ORDER) '
                        {reset} {-restore:.8f} TL (FIXED END) '"""
                pdf.update_stream(contents, f"q 1 0 0 .95 0 40 cm BT /helv 12 Tf\n{lines}\nET Q".encode("ascii"))
                references = [*page.get_contents(), contents]
                pdf.xref_set_key(page.xref, "Contents", "[" + " ".join(f"{xref} 0 R" for xref in references) + "]")
            pdf.save(filename)
        sources[kind] = filename
    paragraph_styles = ("paragraph-matrix-boundary", "paragraph-replacement-matrix", "paragraph-rounded-color",
                        "paragraph-graphics-state-font")
    for kind in paragraph_styles:
        filename = root / f"paragraph-policy-{kind}.pdf"
        with pymupdf.open() as pdf:
            page = pdf.new_page()
            page.insert_text((72, 100), "Original paragraph", fontsize=12)
            baseline = page.rect.height - 100
            if kind == "paragraph-matrix-boundary":
                data = f"""BT /helv 12 Tf
                1 0 0 1 72 {baseline} Tm (Original paragraph) Tj
                .75 0 0 1 72 {baseline - 18} Tm (Nearby separate line) Tj ET"""
            elif kind == "paragraph-replacement-matrix":
                data = f"""BT /helv 12 Tf 80 Tz
                .8 0 0 1.25 72 {baseline} Tm (Original paragraph) Tj ET"""
            elif kind == "paragraph-rounded-color":
                data = f"""BT /helv 12 Tf
                .6 .2 .1 rg 1 0 0 1 72 {baseline} Tm (Original paragraph) Tj
                .6004 .2 .1 rg 1 0 0 1 72 {baseline - 18} Tm (Nearby continuation) Tj ET"""
            else:
                font = page.insert_font("helv")
                resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
                pdf.xref_set_key(resources, "Font", "null")
                pdf.xref_set_key(resources, "ExtGState",
                                 f"<< /Source << /Font [{font} 0 R 12] >> /Accent << /Font [{font} 0 R 10] >> >>")
                data = f"""q /Source gs BT 1 0 0 1 72 {baseline} Tm (Original paragraph) Tj ET
                q /Accent gs BT 1 0 0 1 72 {baseline - 45} Tm (Smaller text) Tj ET Q
                BT 1 0 0 1 72 {baseline - 80} Tm (Restored size) Tj ET Q"""
            pdf.update_stream(page.get_contents()[0], data.encode("ascii"))
            pdf.save(filename)
        sources[kind] = filename
    for kind in ("paragraph-joined-objects", "paragraph-one-array", "paragraph-form-object",
                 "paragraph-overprinted-seed", "paragraph-overprinted-seed-cursor",
                 "paragraph-overprinted-seed-quote", "paragraph-overprinted-seed-graphics"):
        filename = root / f"paragraph-policy-{kind}.pdf"
        with pymupdf.open() as pdf:
            page = pdf.new_page()
            if kind == "paragraph-form-object":
                with pymupdf.open() as nested:
                    nested.new_page().insert_text((72, 100), "Synthetic joined text", fontsize=12)
                    page.show_pdf_page(page.rect, nested, 0)
            elif kind == "paragraph-overprinted-seed":
                for color in ((.2, .2, .2), (0, 0, 0)):
                    page.insert_text((72, 100), "Synthetic joined text", fontsize=12, color=color)
            elif kind.startswith("paragraph-overprinted-seed-"):
                page.insert_text((72, 100), "Synthetic joined text", fontsize=12)
                baseline = page.rect.height - 100
                first = (f'16 TL 1 0 0 1 72 {baseline + 16} Tm .4 .25 (Synthetic joined text) "'
                         if kind.endswith("-quote") else
                         f"1 0 0 1 72 {baseline} Tm (Synthetic joined text) Tj")
                state, selection_font = "", "/helv 12 Tf"
                if kind.endswith("-graphics"):
                    font = page.insert_font("helv")
                    resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
                    pdf.xref_set_key(resources, "Font", "null")
                    pdf.xref_set_key(resources, "ExtGState", f"<< /Source << /Font [{font} 0 R 12] >> >>")
                    state, selection_font = "/Source gs", ""
                pdf.update_stream(page.get_contents()[0], f"""
                    q {state} BT {selection_font} .25 Tc .4 Tw .2 g
                    {first}
                    [-16000 (FIXED CURSOR)] TJ ET Q
                    q {state} BT {selection_font} .25 Tc .4 Tw 0 g
                    1 0 0 1 72 {baseline} Tm (Synthetic joined text) Tj ET Q
                """.encode("ascii"))
            else:
                page.insert_text((72, 100), "Synthetic joined text", fontsize=12)
                baseline = page.rect.height - 100
                show = ("(Synthetic ) Tj (joined text) Tj" if kind == "paragraph-joined-objects"
                        else "/Span << /MCID 1500000000 >> BDC [(Synthetic ) 12 (joined text)] TJ EMC")
                pdf.update_stream(page.get_contents()[0], f"BT /helv 12 Tf 1 0 0 1 72 {baseline} Tm {show} ET".encode("ascii"))
            pdf.save(filename)
        sources[kind] = filename

    filename = root / "paragraph-policy-font-embedding.pdf"
    with pymupdf.open() as pdf:
        page = pdf.new_page()
        # The fixture needs real Latin glyphs in both resource variants.
        latin_font = pymupdf.Font("china-s").buffer
        source_font = page.insert_font(fontname="EmbeddedFixture", fontbuffer=latin_font)
        page.insert_text((72, 100), "Synthetic embedded line", fontsize=12, fontname="EmbeddedFixture")
        page.insert_text((72, 118), "Synthetic unembedded line", fontsize=12, fontname="EmbeddedFixture")
        cid = int(pdf.xref_get_key(source_font, "DescendantFonts")[1].lstrip("[").split()[0])
        descriptor = int(pdf.xref_get_key(cid, "FontDescriptor")[1].split()[0])
        plain_descriptor = pdf.get_new_xref()
        pdf.update_object(plain_descriptor, pdf.xref_object(descriptor))
        for program in ("FontFile", "FontFile2", "FontFile3"):
            pdf.xref_set_key(plain_descriptor, program, "null")
        plain_cid = pdf.get_new_xref()
        pdf.update_object(plain_cid, pdf.xref_object(cid))
        pdf.xref_set_key(plain_cid, "FontDescriptor", f"{plain_descriptor} 0 R")
        plain_font = pdf.get_new_xref()
        pdf.update_object(plain_font, pdf.xref_object(source_font))
        pdf.xref_set_key(plain_font, "DescendantFonts", f"[{plain_cid} 0 R]")
        resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
        pdf.xref_set_key(resources, "Font/UnembeddedFixture", f"{plain_font} 0 R")
        second = page.get_contents()[-1]
        pdf.update_stream(second, pdf.xref_stream(second).replace(b"/EmbeddedFixture ", b"/UnembeddedFixture "))
        pdf.save(filename)
    sources["paragraph-font-embedding"] = filename
    filename = root / "paragraph-policy-fragmented-line.pdf"
    with pymupdf.open() as pdf:
        page = pdf.new_page()
        page.insert_text((72, 100), "Synthetic first paragraph", fontsize=12)
        baseline = page.rect.height - 100
        pdf.update_stream(page.get_contents()[0], f"""BT /helv 12 Tf
            1 0 0 1 72 {baseline} Tm (Synthetic first paragraph) Tj
            1 0 0 1 72 {baseline - 18} Tm [(Split) -5000 (source line)] TJ
            1 0 0 1 72 {baseline - 90} Tm (Independent text) Tj
            ET""".encode("ascii"))
        pdf.save(filename)
    sources["paragraph-fragmented-line"] = filename
    extra_flows = ("overprinted-following", "near-overprinted-following", "array-overprinted-following",
                   "partial-overprinted-following", "tracked-overprint-following", "fragmented-following",
                   "graphics-state-following")
    for kind in extra_flows:
        filename = root / f"paragraph-policy-{kind}.pdf"
        with pymupdf.open() as pdf:
            page = pdf.new_page()
            page.insert_text((72, 100), "Short first paragraph", fontsize=12)
            baseline = page.rect.height - 127
            if kind in ("overprinted-following", "near-overprinted-following", "array-overprinted-following"):
                page.insert_text((72, 127), "NEXT PARAGRAPH", fontsize=12,
                                 color=(.6, .15, .05), render_mode=1)
                offset = (.7, .55) if kind == "near-overprinted-following" else (0, 0)
                page.insert_text((72 + offset[0], 127 + offset[1]), "NEXT PARAGRAPH", fontsize=12,
                                 color=(.1, .25, .8))
                if kind == "array-overprinted-following":
                    pdf.update_stream(page.get_contents()[-1], f"""BT /helv 12 Tf .1 .25 .8 rg
                        1 0 0 1 72 {baseline} Tm [(NEXT) 0 ( PARAGRAPH)] TJ ET""".encode("ascii"))
            elif kind == "tracked-overprint-following":
                font = page.insert_font("helv")
                resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
                alias = pdf.get_new_xref()
                pdf.update_object(alias, pdf.xref_object(font))
                pdf.xref_set_key(resources, "Font/FixtureAlias", f"{alias} 0 R")
                stream = page.get_contents()[0]
                pdf.update_stream(stream, pdf.xref_stream(stream) + f"""
                    q BT 1 Tr /helv 12 Tf .6 .15 .05 RG
                    1 0 0 1 72 {baseline} Tm (NEXT PARAGRAPH) Tj
                    0 Tr /FixtureAlias 12.02 Tf .1 .25 .8 rg
                    1 0 0 1 72 {baseline} Tm (NEXT PARAGRAPH) Tj ET Q
                """.encode("ascii"))
            elif kind == "partial-overprinted-following":
                page.insert_text((72, 127), "NEXT", fontsize=12, color=(.6, .15, .05))
                page.insert_text((72.15, 127), "NEXT PARAGRAPH", fontsize=12, color=(.1, .25, .8))
            elif kind == "fragmented-following":
                page.insert_text((72, 127), "NEXT PARAGRAPH", fontsize=12)
                pdf.update_stream(page.get_contents()[-1], f"""BT /helv 12 Tf
                    1 0 0 1 72 {baseline} Tm [(NEXT) -5000 (PARAGRAPH)] TJ
                    ET""".encode("ascii"))
            else:
                font = page.insert_font("helv")
                resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
                pdf.xref_set_key(resources, "ExtGState", f"<< /Follower << /Font [{font} 0 R 12] >> >>")
                page.insert_text((72, 127), "NEXT PARAGRAPH", fontsize=12)
                pdf.update_stream(page.get_contents()[-1], f"""q /Follower gs .1 .25 .8 rg
                    BT 1 0 0 1 72 {baseline} Tm (NEXT PARAGRAPH) Tj ET Q""".encode("ascii"))
            pdf.save(filename)
        sources[kind] = filename
    filename = root / "paragraph-policy-untracked-transform.pdf"
    with pymupdf.open() as pdf:
        page = pdf.new_page()
        page.insert_text((72, 100), "Short first paragraph", fontsize=12)
        baseline = page.rect.height - 100
        pdf.update_stream(page.get_contents()[0], f"""BT /helv 12 Tf
            0 1 -1 0 420 {baseline + 40} Tm (AX) Tj
            1 0 0 1 72 {baseline} Tm (Short first paragraph) Tj
            1 0 0 1 72 {baseline - 27} Tm (NEXT PARAGRAPH) Tj
            1 0 0 1 72 {baseline - 45} Tm (STAYS IN ORDER) Tj
            ET""".encode("ascii"))
        pdf.save(filename)
    sources["paragraph-untracked-transform"] = filename
    assert sources["oversized"].stat().st_size > 16 * 1024 * 1024

    status_path = "/pdfium-paragraph-trial/status"
    auth = {"Authorization": "Bearer synthetic-paragraph-policy"}
    for label, flags, available in settings:
        environment = {key: value for key, value in os.environ.items() if key not in (current_flag, legacy_flag)}
        environment.update(flags)
        with (patch.dict(os.environ, environment, clear=True),
              running_vault(root / f"paragraph-policy-{label}-rust") as rust,
              TestClient(create_app(root / f"paragraph-policy-{label}-python",
                                    token="synthetic-paragraph-policy", require_vault=False)) as python):
            rust("POST", "/vault/setup", {"password": PASSWORD})
            status = python.get(status_path, headers=auth)
            assert status.status_code == 200
            expected = status.json()
            actual = rust("GET", status_path)
            assert expected["available"] == available
            assert actual == {**expected, "engine": "pdf-oxide-paragraph"}
            rust_matter = rust("POST", "/matters", {"name": "Synthetic paragraph policy"}, status=201)["id"]
            py_matter_response = python.post("/matters", headers=auth, json={"name": "Synthetic paragraph policy"})
            assert py_matter_response.status_code == 201
            py_matter = py_matter_response.json()["id"]

            def documents(kind):
                source = sources[kind]
                rust_id = rust("IMPORT", rust_matter, source=str(source), status=201)[0]["id"]
                response = python.post(f"/matters/{py_matter}/documents/raw", content=source.read_bytes(),
                                       headers={**auth, "Content-Type": "application/octet-stream",
                                                "X-CourtBundle-Filename": source.name})
                assert response.status_code == 201, response.text
                return f"/documents/{rust_id}", f"/documents/{response.json()[0]['id']}"

            def refusal_pair(routes, endpoint, body, code, detail):
                rust_route, py_route = routes
                expected = python.post(py_route + "/pages/" + endpoint, headers=auth, json=body)
                assert (expected.status_code, expected.json()) == (code, {"detail": detail})
                assert rust("POST", rust_route + "/pages/" + endpoint, body, status=None) == {
                    "status": code, "body": expected.json()}
                for revisions in (rust("GET", rust_route + "/revisions"),
                                  python.get(py_route + "/revisions", headers=auth).json()):
                    assert [entry["revision"] for entry in revisions] == [1]

            selection = {"expected_revision": 1, "page_index": 0, "span_index": 0,
                         "expected_text": "Original paragraph"}
            replacement = {**selection, "expected_paragraph": "Original paragraph", "replacement": "Updated text"}
            routes = documents("ordinary")
            if not available:
                refusal_pair(routes, "paragraph-trial-preview", selection, 422, "PDFium paragraph trial is unavailable")
                refusal_pair(routes, "paragraph-trial", replacement, 422,
                             "PDFium paragraph trial is unavailable; no revision was published")
                refusal_pair(routes, "paragraph-trial", {**replacement, "expected_revision": 2}, 409,
                             "Document changed; reload and select text again")
            else:
                preview = python.post(routes[1] + "/pages/paragraph-trial-preview", headers=auth, json=selection)
                assert preview.status_code == 200, preview.text
                assert rust("POST", routes[0] + "/pages/paragraph-trial-preview", selection)["text"] == preview.json()["text"]
            if label != "default":
                continue
            for endpoint, body, fields in (
                ("paragraph-trial-preview", {"page_index": -1, "expected_text": "", "unknown": True},
                 ["page_index", "span_index", "expected_revision", "expected_text", "unknown"]),
                ("paragraph-trial", {**replacement, "replacement": "é" * 2001, "expected_paragraph": "a" * 4001},
                 ["replacement", "expected_paragraph"]),
                ("replace-text-run", {**replacement, "replacement": None}, ["replacement"]),
                ("move-text-run", {**selection, "horizontal_offset": "1.1", "vertical_offset": False},
                 ["horizontal_offset"]),
            ):
                expected = python.post(routes[1] + "/pages/" + endpoint, headers=auth, json=body)
                assert expected.status_code == 422
                assert [error["loc"][-1] for error in expected.json()["detail"]] == fields
                assert rust("POST", routes[0] + "/pages/" + endpoint, body, status=None) == {
                    "status": 422, "body": expected.json()}
            coercible = {**selection, "expected_revision": "1", "page_index": 0.0, "span_index": False}
            expected = python.post(routes[1] + "/pages/paragraph-trial-preview", headers=auth, json=coercible)
            actual = rust("POST", routes[0] + "/pages/paragraph-trial-preview", coercible)
            assert expected.status_code == 200 and actual["text"] == expected.json()["text"]
            refusal_pair(routes, "paragraph-trial", {**replacement, "replacement": "   "}, 422,
                         "Replacement paragraph is empty")
            refusal_pair(routes, "paragraph-trial-preview", {**selection, "page_index": 1}, 422, "Invalid page number")
            refusal_pair(routes, "paragraph-trial", {**replacement, "replacement": "Original paragraph"}, 422,
                         "Replacement matches the current paragraph; no revision was published")
            refusal_pair(routes, "paragraph-trial-preview", {**selection, "expected_text": "Changed beforehand"}, 422,
                         "Selected text changed; reload and select it again")
            for kind, message in (
                ("rotated", "Rotated pages are unsupported by the PDFium paragraph trial"),
                ("oversized", "PDF exceeds the 16 MiB PDFium trial limit"),
                ("long-paragraph", "Paragraph exceeds the 2,000-character trial limit"),
                *((kind, "PDFium trial supports visible opaque fill text only")
                  for kind in ("stroked", "filled-and-stroked", "translucent")),
            ):
                trial_routes = documents(kind)
                selected_text = long_line if kind == "long-paragraph" else selection["expected_text"]
                refusal_pair(trial_routes, "paragraph-trial-preview", {**selection, "expected_text": selected_text}, 422, message)
                refusal_pair(trial_routes, "paragraph-trial", {**replacement, "expected_text": selected_text}, 422, message)
                if kind in {"rotated", "oversized"}:
                    refusal_pair(trial_routes, "paragraph-trial-preview", {**selection, "expected_text": "Stale"}, 422,
                                 "Selected text changed; reload and select it again")
                    refusal_pair(trial_routes, "paragraph-trial", {**replacement, "expected_text": "Stale"}, 422,
                                 "Selected text changed; reload and select it again")

            valid_paint_routes = {kind: documents(kind) for kind in ("rounded-opaque", "stroke-alpha", "mixed-opacity")}
            for kind, text, index in (("rounded-opaque", "Original paragraph", 0),
                                      ("stroke-alpha", "Original paragraph", 0),
                                      ("mixed-opacity", "Original paragraph", 0),
                                      ("mixed-opacity", "Restored opaque fill", 2)):
                rust_route, py_route = valid_paint_routes[kind]
                body = {**selection, "span_index": index, "expected_text": text}
                expected = python.post(py_route + "/pages/paragraph-trial-preview", headers=auth, json=body)
                actual = rust("POST", rust_route + "/pages/paragraph-trial-preview", body)
                assert expected.status_code == 200 and actual["text"] == expected.json()["text"] == text
                assert actual["line_count"] == expected.json()["line_count"] == 1
            refusal_pair(documents("covered-by-link"), "paragraph-trial", replacement, 422,
                         "Paragraph overlaps an image, graphic, link or annotation")
            refusal_pair(documents("bottom-flow"), "paragraph-trial",
                         {**replacement, "replacement": "A longer bottom paragraph\nsecond line\nthird line"}, 422,
                         "Following text cannot fit on this page after reflow")

            for kind in ("paragraph-joined-objects", "paragraph-one-array", "paragraph-form-object",
                         "paragraph-font-embedding", "paragraph-overprinted-seed",
                         "paragraph-overprinted-seed-cursor", "paragraph-overprinted-seed-quote",
                         "paragraph-overprinted-seed-graphics"):
                rust_route, py_route = documents(kind)
                wanted = "Synthetic embedded line" if kind == "paragraph-font-embedding" else "Synthetic joined text"
                independent = "".join(wanted.split())
                choices = (rust("GET", rust_route + "/pages/0/text-runs")["runs"],
                           python.get(py_route + "/pages/0/text-runs", headers=auth).json()["runs"])
                arguments = []
                for backend, runs in zip(("Rust", "Python"), choices, strict=True):
                    run = next((run for run in runs if "".join(run["text"].split()) == independent), None)
                    assert run is not None, (kind, backend, [item["text"] for item in runs])
                    arguments.append({**selection, "span_index": run["index"], "expected_text": run["text"]})
                rust_pages, py_pages = rust_route + "/pages", py_route + "/pages"
                source = sources[kind].read_bytes()
                native = rust_route.rsplit("/", 1)[-1]
                if kind in ("paragraph-joined-objects", "paragraph-form-object"):
                    for endpoint, fields in (("paragraph-trial-preview", {}),
                                             ("paragraph-trial", {"expected_paragraph": wanted,
                                                                  "replacement": "Synthetic replacement"})):
                        expected = python.post(py_pages + "/" + endpoint, headers=auth, json={**arguments[1], **fields})
                        assert expected.status_code == 422, expected.text
                        assert expected.json() == {"detail": "PDFium needs one exact text object for this selection"}
                        assert rust("POST", rust_pages + "/" + endpoint, {**arguments[0], **fields}, status=None) == {
                            "status": 422, "body": expected.json()}
                    for revisions in (rust("GET", rust_route + "/revisions"),
                                      python.get(py_route + "/revisions", headers=auth).json()):
                        assert [row["revision"] for row in revisions] == [1]
                else:
                    left = rust("POST", rust_pages + "/paragraph-trial-preview", arguments[0])
                    right = python.post(py_pages + "/paragraph-trial-preview", headers=auth, json=arguments[1])
                    assert right.status_code == 200, right.text
                    assert left["line_count"] == right.json()["line_count"] == 1
                    assert "".join(left["text"].split()) == "".join(right.json()["text"].split()) == independent
                    if kind.startswith("paragraph-overprinted-seed"):
                        edits = [{**body, "expected_paragraph": preview["text"], "replacement": "Synthetic replacement"}
                                 for body, preview in zip(arguments, (left, right.json()), strict=True)]
                        expected = python.post(py_pages + "/paragraph-trial", headers=auth, json=edits[1])
                        assert expected.status_code == 422, expected.text
                        assert expected.json() == {"detail": "PDFium did not produce the requested searchable paragraph"}
                        assert rust("POST", rust_pages + "/paragraph-trial", edits[0], status=None) == {
                            "status": 422, "body": expected.json()}
                        for revisions in (rust("GET", rust_route + "/revisions"),
                                          python.get(py_route + "/revisions", headers=auth).json()):
                            assert [row["revision"] for row in revisions] == [1]
                        for edit in edits:
                            edit["replacement"] = wanted + "\nAdded second line"
                        expected = python.post(py_pages + "/paragraph-trial", headers=auth, json=edits[1])
                        cursor_dependent = kind.endswith(("-cursor", "-quote", "-graphics"))
                        if cursor_dependent:
                            # PDFium's regeneration loses spacing on the distant
                            # cursor text. Its publication guard must refuse it.
                            assert expected.status_code == 422, (label, kind, expected.text)
                            assert expected.json() == {
                                "detail": "PDFium changed extracted words outside the paragraph region",
                            }
                            revisions = python.get(py_route + "/revisions", headers=auth).json()
                            assert [row["revision"] for row in revisions] == [1]
                        else:
                            assert expected.status_code == 200, (label, kind, expected.text)
                        actual = rust("POST", rust_pages + "/paragraph-trial", edits[0])
                        assert actual["revision"] == 2

                        def target_glyphs(data, text, black):
                            with pymupdf.open(stream=data, filetype="pdf") as pdf:
                                for run in pdf[0].get_texttrace():
                                    if black and any(run["color"]):
                                        continue
                                    start = "".join(chr(glyph[0]) for glyph in run["chars"]).find(text)
                                    if start >= 0:
                                        return run["chars"][start:start + len(text)]
                            raise AssertionError(text)

                        outputs = [pdf_bytes(rust, native, 2)]
                        if not cursor_dependent:
                            assert expected.json()["revision"] == 2
                            outputs.append(python.get(py_route + "/pdf", headers=auth).content)
                        for content in outputs:
                            with pymupdf.open(stream=content, filetype="pdf") as pdf:
                                assert "Added second line" in pdf[0].get_text()
                            targets = [(wanted, True)]
                            if cursor_dependent:
                                targets.append(("FIXED CURSOR", False))
                            for text, black in targets:
                                before, after = target_glyphs(source, text, black), target_glyphs(content, text, black)
                                for a, b in zip(before, after, strict=True):
                                    assert a[:2] == b[:2]
                                    assert all(abs(x - y) < .08 for x, y in zip(a[2], b[2], strict=True))
                    elif kind == "paragraph-one-array":
                        left_edit = {**arguments[0], "expected_paragraph": left["text"], "replacement": "Synthetic replacement"}
                        right_edit = {**arguments[1], "expected_paragraph": right.json()["text"], "replacement": "Synthetic replacement"}
                        updated = rust("POST", rust_pages + "/paragraph-trial", left_edit)
                        reference = python.post(py_pages + "/paragraph-trial", headers=auth, json=right_edit)
                        assert reference.status_code == 200, reference.text
                        assert updated["revision"] == reference.json()["revision"] == 2
                        for content in (pdf_bytes(rust, native, 2), python.get(py_route + "/pdf", headers=auth).content):
                            with pymupdf.open(stream=content) as output:
                                assert output[0].get_text().strip() == "Synthetic replacement"
                assert pdf_bytes(rust, native, 1, "original") == source
                assert python.get(py_route + "/original", headers=auth).content == source

            rust_route, py_route = documents("paragraph-fragmented-line")
            native = rust_route.rsplit("/", 1)[-1]
            native_runs = rust("GET", rust_route + "/pages/0/text-runs")["runs"]
            py_runs = python.get(py_route + "/pages/0/text-runs", headers=auth).json()["runs"]
            assert {"Split", "source line"} <= {run["text"].strip() for run in native_runs}
            wanted = "Synthetic first paragraph"
            left_body, right_body = ({**selection, "expected_text": wanted,
                                       "span_index": next(run["index"] for run in runs if run["text"] == wanted)}
                                     for runs in (native_runs, py_runs))
            left = rust("POST", rust_route + "/pages/paragraph-trial-preview", left_body)
            right = python.post(py_route + "/pages/paragraph-trial-preview", headers=auth, json=right_body)
            assert right.status_code == 200, right.text
            assert left["line_count"] == right.json()["line_count"] == 2
            assert left["text"].split() == right.json()["text"].split() == (wanted + " Split source line").split()
            replacement_lines = "Revised first\nRevised second"
            left_body.update(expected_paragraph=left["text"], replacement=replacement_lines)
            right_body.update(expected_paragraph=right.json()["text"], replacement=replacement_lines)
            updated = rust("POST", rust_route + "/pages/paragraph-trial", left_body)
            reference = python.post(py_route + "/pages/paragraph-trial", headers=auth, json=right_body)
            assert reference.status_code == 200, reference.text
            assert updated["revision"] == reference.json()["revision"] == 2
            assert updated["edit_layout"]["line_count"] == reference.json()["edit_layout"]["line_count"] == 2
            for content in (pdf_bytes(rust, native, 2), python.get(py_route + "/pdf", headers=auth).content):
                with pymupdf.open(stream=content) as output:
                    page = output[0]
                    assert sorted(page.get_text().split()) == sorted((replacement_lines + " Independent text").split())
                    box, = page.search_for("Independent text")
                    with pymupdf.open(sources["paragraph-fragmented-line"]) as original_pdf:
                        assert list(box) == list(original_pdf[0].search_for("Independent text")[0])
            original = sources["paragraph-fragmented-line"].read_bytes()
            assert pdf_bytes(rust, native, 1, "original") == original
            assert python.get(py_route + "/original", headers=auth).content == original

            transform_routes = documents("paragraph-untracked-transform")
            wanted = "Short first paragraph"
            transform_seed = {**selection, "expected_text": wanted}
            assert rust("POST", transform_routes[0] + "/pages/paragraph-trial-preview", transform_seed)["text"] == wanted
            transform_preview = python.post(transform_routes[1] + "/pages/paragraph-trial-preview",
                                             headers=auth, json=transform_seed)
            assert transform_preview.status_code == 200 and transform_preview.json()["text"] == wanted
            refusal_pair(transform_routes, "paragraph-trial", {**transform_seed, "expected_paragraph": wanted,
                         "replacement": "A longer first paragraph\nwith a second line\nand a third line"}, 422,
                         "Page has unsupported text objects; following text cannot be reflowed safely")
            original = sources["paragraph-untracked-transform"].read_bytes()
            assert pdf_bytes(rust, transform_routes[0].rsplit("/", 1)[-1], 1, "original") == original
            assert python.get(transform_routes[1] + "/original", headers=auth).content == original

            def paint_trace(data):
                with pymupdf.open(stream=data, filetype="pdf") as pdf:
                    return {"".join(chr(glyph[0]) for glyph in run["chars"]): run
                            for run in pdf[0].get_texttrace()}

            for kind in paragraph_styles:
                rust_route, py_route = documents(kind)
                wanted = "Original paragraph"
                choices = (rust("GET", rust_route + "/pages/0/text-runs")["runs"],
                           python.get(py_route + "/pages/0/text-runs", headers=auth).json()["runs"])
                payloads = [{"page_index": 0, "expected_revision": 1, "expected_text": wanted,
                             "span_index": next(run["index"] for run in runs if run["text"] == wanted)}
                            for runs in choices]
                rust_pages, py_pages = rust_route + "/pages", py_route + "/pages"
                left = rust("POST", rust_pages + "/paragraph-trial-preview", payloads[0])
                right = python.post(py_pages + "/paragraph-trial-preview", headers=auth, json=payloads[1])
                assert right.status_code == 200, right.text
                expected_count = 2 if kind == "paragraph-rounded-color" else 1
                original_text = wanted + " Nearby continuation" if expected_count == 2 else wanted
                assert left["line_count"] == right.json()["line_count"] == expected_count
                assert left["text"] == right.json()["text"] == original_text
                replacement_text = ("REVISED FIRST\nREVISED SECOND" if kind == "paragraph-replacement-matrix"
                                    else "Edited first line")
                for payload in payloads:
                    payload.update(expected_paragraph=original_text, replacement=replacement_text)
                left = rust("POST", rust_pages + "/paragraph-trial", payloads[0])
                right = python.post(py_pages + "/paragraph-trial", headers=auth, json=payloads[1])
                assert right.status_code == 200, right.text
                assert left["revision"] == right.json()["revision"] == 2
                native = rust_route.rsplit("/", 1)[-1]
                source = sources[kind].read_bytes()
                assert pdf_bytes(rust, native, 1, "original") == source
                assert python.get(py_route + "/original", headers=auth).content == source
                actual = paint_trace(pdf_bytes(rust, native, 2))
                expected = paint_trace(python.get(py_route + "/pdf", headers=auth).content)
                assert wanted not in actual and wanted not in expected
                if kind == "paragraph-matrix-boundary":
                    key = "Nearby separate line"
                    assert actual[key]["chars"] == paint_trace(source)[key]["chars"]
                    assert expected[key]["chars"] == actual[key]["chars"]
                elif kind == "paragraph-replacement-matrix":
                    for key, y in (("REVISED FIRST", 100), ("REVISED SECOND", 114.4)):
                        assert abs(actual[key]["chars"][0][2][1] - y) < .08
                        for a, b in zip(actual[key]["chars"], expected[key]["chars"], strict=True):
                            assert a[0] == b[0]
                            assert all(abs(x - y) < .08 for x, y in zip(a[2], b[2], strict=True))
                            assert all(abs(x - y) < .1 for x, y in zip(a[3], b[3], strict=True))
                elif kind == "paragraph-rounded-color":
                    assert "Nearby continuation" not in actual and "Nearby continuation" not in expected
                    assert all(abs(a - b) < .004 for a, b in zip(actual[replacement_text]["color"],
                                                                 expected[replacement_text]["color"], strict=True))
                else:
                    original_paint = paint_trace(source)
                    for text, size in (("Smaller text", 10), ("Restored size", 12)):
                        for result in (actual, expected):
                            assert result[text]["size"] == size
                            assert result[text]["chars"] == original_paint[text]["chars"]

            def rgb(run):
                color = run["color"]
                return color * 3 if len(color) == 1 else color

            def follower_paint(data):
                with pymupdf.open(stream=data, filetype="pdf") as pdf:
                    return [run for run in pdf[0].get_texttrace()
                            if "".join(chr(glyph[0]) for glyph in run["chars"]).startswith(("NEXT", "PARAGRAPH"))]

            for kind in extra_flows:
                rust_route, py_route = documents(kind)
                original = sources[kind].read_bytes()
                native = rust_route.rsplit("/", 1)[-1]
                seed = "Short first paragraph"
                edit = {**selection, "expected_text": seed, "expected_paragraph": seed,
                        "replacement": "A longer first paragraph\nwith a second line\nand a third line"}
                py_runs = python.get(py_route + "/pages/0/text-runs", headers=auth).json()["runs"]
                rust_runs = rust("GET", rust_route + "/pages/0/text-runs")["runs"]
                left, right = ({**edit, "span_index": next(run["index"] for run in runs if run["text"] == seed)}
                               for runs in (rust_runs, py_runs))
                reference = python.post(py_route + "/pages/paragraph-trial", headers=auth, json=right)
                if kind in ("overprinted-following", "near-overprinted-following", "array-overprinted-following"):
                    detail = "Page has unsupported text objects; following text cannot be reflowed safely"
                    assert (reference.status_code, reference.json()) == (422, {"detail": detail})
                    assert rust("POST", rust_route + "/pages/paragraph-trial", left, status=None) == {
                        "status": 422, "body": reference.json()}
                    for revisions in (rust("GET", rust_route + "/revisions"),
                                      python.get(py_route + "/revisions", headers=auth).json()):
                        assert [row["revision"] for row in revisions] == [1]
                    assert pdf_bytes(rust, native, 1, "original") == original
                    assert python.get(py_route + "/original", headers=auth).content == original
                    continue
                assert reference.status_code == 200, reference.text
                result = rust("POST", rust_route + "/pages/paragraph-trial", left)
                copies = ((result, pdf_bytes(rust, native, 2)),
                          (reference.json(), python.get(py_route + "/pdf", headers=auth).content))
                before = follower_paint(original)
                expected_count = 2 if kind in ("tracked-overprint-following", "partial-overprinted-following") else 1
                for response, contents in copies:
                    assert response["revision"] == 2
                    layout = response["edit_layout"]
                    assert layout["following_text_objects_moved"] == expected_count
                    shift = layout["following_text_shift"]
                    assert shift > 0
                    for old, new in zip(before, follower_paint(contents), strict=True):
                        assert old["type"] == new["type"] and old["font"] == new["font"]
                        assert all(abs(a - b) < .005 for a, b in zip(rgb(old), rgb(new), strict=True))
                        for a, b in zip(old["chars"], new["chars"], strict=True):
                            assert a[:2] == b[:2] and abs(a[2][0] - b[2][0]) < .08
                            assert abs(a[2][1] + shift - b[2][1]) < .08
                assert pdf_bytes(rust, native, 1, "original") == original
                assert python.get(py_route + "/original", headers=auth).content == original

            for kind in styled_flows:
                rust_route, py_route = documents(kind)
                source = sources[kind].read_bytes()
                old_paint = paint_trace(source)
                native = rust_route.rsplit("/", 1)[-1]
                edit = {"page_index": 0, "expected_revision": 1,
                        "expected_text": "Short first paragraph", "expected_paragraph": "Short first paragraph",
                        "replacement": "A longer first paragraph\nwith a second line\nand a third line"}
                py_runs = python.get(py_route + "/pages/0/text-runs", headers=auth).json()["runs"]
                rust_runs = rust("GET", rust_route + "/pages/0/text-runs")["runs"]
                py_span = next(run["index"] for run in py_runs if run["text"] == edit["expected_text"])
                rust_span = next(run["index"] for run in rust_runs if run["text"] == edit["expected_text"])
                expected = python.post(py_route + "/pages/paragraph-trial", headers=auth,
                                       json={**edit, "span_index": py_span})
                if kind == "styled-flow-quoted":
                    assert expected.status_code == 422, expected.text
                    assert expected.json() == {
                        "detail": "PDFium changed extracted words outside the paragraph region",
                    }
                    revisions = python.get(py_route + "/revisions", headers=auth).json()
                    assert [row["revision"] for row in revisions] == [1]
                else:
                    assert expected.status_code == 200, (kind, expected.text)
                actual = rust("POST", rust_route + "/pages/paragraph-trial", {**edit, "span_index": rust_span})
                assert pdf_bytes(rust, native, 1, "original") == source
                assert python.get(py_route + "/original", headers=auth).content == source
                copies = [(actual, pdf_bytes(rust, native, 2))]
                if kind != "styled-flow-quoted":
                    copies.append((expected.json(), python.get(py_route + "/pdf", headers=auth).content))
                for result, contents in copies:
                    assert result["revision"] == 2
                    layout = result["edit_layout"]
                    assert layout["following_text_objects_moved"] == 2
                    shift = layout["following_text_shift"]
                    assert shift > 0
                    new_paint = paint_trace(contents)
                    assert "Short first paragraph" not in new_paint
                    placements = [("UNTOUCHED HEADING", 0), ("NEXT PARAGRAPH", shift), ("STAYS IN ORDER", shift)]
                    if kind in ("styled-flow-line-steps", "styled-flow-quoted"):
                        placements.extend([("FIXED START", 0), ("FIXED END", 0)])
                    for text, dy in placements:
                        before, after = old_paint[text], new_paint[text]
                        assert after["type"] == before["type"]
                        assert after["font"] == before["font"]
                        assert abs(after["opacity"] - before["opacity"]) < .005
                        assert all(abs(left - right) < .005 for left, right in zip(rgb(before), rgb(after), strict=True))
                        for left, right in zip(before["chars"], after["chars"], strict=True):
                            assert left[:2] == right[:2]
                            assert abs(left[2][0] - right[2][0]) < .08
                            assert abs(left[2][1] + dy - right[2][1]) < .08

            # The signed-working-copy guard still runs before a stale request
            # is refused. Only a literal JSON true acknowledges the risk.
            visible_signature = {"title": "Synthetic signed paragraph", "visible_signature_status": "present"}
            assert rust("PATCH", routes[0], visible_signature)["signature_status"] == "visible_signature_only"
            signed = python.patch(routes[1], headers=auth, json=visible_signature)
            assert signed.status_code == 200 and signed.json()["signature_status"] == "visible_signature_only"
            stale = {**replacement, "expected_revision": 2}
            endpoint = routes[1] + "/pages/paragraph-trial"
            challenge = python.post(endpoint, headers=auth, json=stale)
            detail = challenge.json()["detail"]
            assert challenge.status_code == 409 and detail.startswith("Signature-risk acknowledgment required")
            refusal_pair(routes, "paragraph-trial", {**stale, "acknowledge_signature_risk": "true"}, 409, detail)
            refusal_pair(routes, "paragraph-trial", {**stale, "acknowledge_signature_risk": True}, 409,
                         "Document changed; reload and select text again")
            original = sources["ordinary"].read_bytes()
            assert pdf_bytes(rust, routes[0].rsplit("/", 1)[-1], 1, "original") == original
            assert python.get(routes[1] + "/original", headers=auth).content == original


def paragraph_viewports(rust, matter, root):
    from app.paragraph_trial import preview_paragraph_trial, replace_paragraph_trial
    from app.pdf_ops import ProcessingError

    cases = (
        ("clipped", 320, 240, (85, 108), "[-20 -10 300 220]", 1, "New text", None, None),
        ("offset-preview", 320, 240, (85, 108), "[20 10 300 220]", 1, None, None, None),
        ("scaled-selection", 320, 240, (85, 108), "[-20 -10 300 220]", 2, "New text",
         "PDFium needs one exact text object for this selection", None),
        ("cropped-selection", 320, 240, (85, 108), "[70 40 270 200]", 1, "New text",
         "PDFium needs one exact text object for this selection", None),
        ("large-valid", 3200, 2100, (36, 60), None, 1, "New text", None, None),
        ("large-refused", 3600, 2700, (36, 60), None, 1, "New text", None,
         "Page is too large for the paragraph trial pixel check"),
        ("out-of-width", 240, 200, (224, 50), None, 1, "Q", None, "No room to reflow this paragraph"),
        ("long-word", 240, 200, (75, 50), None, 1, "W" * 45, None,
         "A replacement word does not fit the paragraph width"),
        ("line-limit", 3200, 2100, (36, 60), None, 1, "\n".join("x" for _ in range(101)), None,
         "Replacement has too many paragraph lines"),
        ("ink-boundary", 119, 200, (36, 50), None, 1, "I" * 20, None, None),
        ("embedded-simple", 470, 500, (40, 65), None, 1, "NEW", None, None),
        ("embedded-composite", 470, 500, (40, 65), None, 1, "NEW", None, None),
    )
    for case, width, height, baseline, crop, unit, replacement, preview_error, edit_error in cases:
        source = root / f"paragraph-viewport-{case}.pdf"
        selected = "X" if case == "out-of-width" else "AAA" if case.startswith("embedded-") else "Original text"
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=width, height=height)
            fontname = "helv"
            if case.startswith("embedded-"):
                fontname = "FixtureEmbedded"
                page.insert_font(fontname=fontname, fontbuffer=pymupdf.Font("helv").buffer,
                                 set_simple=case == "embedded-simple")
            page.insert_text(baseline, selected, fontname=fontname, fontsize=12)
            if crop is not None:
                pdf.xref_set_key(page.xref, "CropBox", crop)
            if unit != 1:
                pdf.xref_set_key(page.xref, "UserUnit", str(unit))
            pdf.set_metadata({"title": f"Synthetic paragraph {case}"})
            pdf.save(source)
        original = source.read_bytes()
        reference = root / f"paragraph-viewport-reference-{case}.pdf"
        reference.write_bytes(original)
        document = rust("IMPORT", matter, source=str(source), status=201)[0]["id"]
        route = f"/documents/{document}/pages"
        request = {"expected_revision": 1, "page_index": 0, "span_index": 0, "expected_text": selected}
        edit_request = dict(request, expected_paragraph=selected, replacement=replacement)
        if preview_error:
            for endpoint, body, operation in (
                ("paragraph-trial-preview", request, lambda: preview_paragraph_trial(reference, 0, 0, selected)),
                ("paragraph-trial", edit_request, lambda: replace_paragraph_trial(reference, 0, 0, selected,
                                                                                 selected, replacement)),
            ):
                try:
                    operation()
                except ProcessingError as error:
                    assert str(error) == preview_error, case
                else:
                    raise AssertionError(f"Unexpected reference selection result for {case}")
                assert rust("POST", f"{route}/{endpoint}", body, status=422) == {"detail": preview_error}, case
            assert pdf_bytes(rust, document, 1) == original
            continue

        expected = preview_paragraph_trial(reference, 0, 0, selected)
        actual = rust("POST", route + "/paragraph-trial-preview", request)
        assert actual["text"] == expected["text"] and actual["line_count"] == expected["line_count"] == 1, case
        # Viewport offsets must agree independently of the two glyph-bound extractors.
        for a, b in zip(actual["bbox"], expected["bbox"], strict=True):
            assert abs(a - b) < 4, (case, actual["bbox"], expected["bbox"])
        if replacement is None:
            continue
        if edit_error:
            try:
                replace_paragraph_trial(reference, 0, 0, selected, expected["text"], replacement)
            except ProcessingError as error:
                assert str(error) == edit_error, case
            else:
                raise AssertionError(f"Unexpected reference edit result for {case}")
            assert rust("POST", route + "/paragraph-trial", edit_request, status=422) == {"detail": edit_error}
            assert pdf_bytes(rust, document, 1) == original
            continue
        expected_layout = replace_paragraph_trial(reference, 0, 0, selected, expected["text"], replacement)
        edited = rust("POST", route + "/paragraph-trial", edit_request)
        assert edited["revision"] == 2 and edited["edit_layout"]["line_count"] == expected_layout["line_count"]
        for key in ("font_substituted", "font_name"):
            assert edited["edit_layout"][key] == expected_layout[key], (case, key, edited)
        with pymupdf.open(reference) as before, pymupdf.open(stream=pdf_bytes(rust, document, 2)) as after:
            assert before[0].get_text() == after[0].get_text() == replacement + "\n", case
            assert before[0].rect == after[0].rect, case
        assert pdf_bytes(rust, document, 1, "original") == original


def paragraph_object_bounds(rust, matter, root):
    from app.paragraph_trial import preview_paragraph_trial, replace_paragraph_trial
    from app.pdf_ops import ProcessingError, editable_text_runs

    cases = (
        # Courier's advance rectangles overlap by 34%, but its painted objects overlap by 19%.
        ("row-overlap", "Courier", "(II) Tj 1 0 0 1 49.5 135 Tm (II) Tj", None, "", None),
        ("close-baselines", "Helvetica", "(IIII) Tj 1 0 0 1 40 141 Tm (llll) Tj", "XX", "", None),
        ("spaced-array", "Helvetica", "75 Tz 1 Tc [(AB) 90 (CD)] TJ", None, "", None),
        ("space-origins", "Courier", "( II ) Tj", "AB", "", None),
        ("clear-graphic", "Helvetica", "(IIII) Tj", "XX", "40 159.6 8 1 re f", None),
        ("blocked-graphic", "Helvetica", "(IIII) Tj", "XX", "40 157.6 8 .5 re f",
         "Paragraph overlaps an image, graphic, link or annotation"),
        ("following", "Helvetica", "(BIG TITLE) Tj 1 0 0 1 40 122 Tm (low text) Tj "
         "1 0 0 1 40 100 Tm (end) Tj", "FIRST\nSECOND\nTHIRD", "", None),
    )
    for case, font, shows, replacement, graphic, edit_error in cases:
        source = root / f"paragraph-ink-{case}.pdf"
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=240, height=200)
            page.insert_font(fontname=font)
            font_xref = page.get_fonts()[0][0]
            resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
            pdf.xref_set_key(resources, "Font", f"<< /F {font_xref} 0 R >>")
            stream = pdf.get_new_xref()
            pdf.update_object(stream, "<< >>")
            prefix = "1.25 0 0 1.1 5 9 cm BT /F 12 Tf 1 0 0 1 40 100 Tm" if case == "spaced-array" else (
                "BT /F 12 Tf 1 0 0 1 40 150 Tm"
            )
            pdf.update_stream(stream, f"{prefix} {shows} ET {graphic}".encode("ascii"))
            page.set_contents(stream)
            pdf.save(source)
        original = source.read_bytes()
        reference = root / f"paragraph-ink-reference-{case}.pdf"
        reference.write_bytes(original)
        selected = editable_text_runs(reference, 0)[0]["text"]
        expected = preview_paragraph_trial(reference, 0, 0, selected)
        document = rust("IMPORT", matter, source=str(source), status=201)[0]["id"]
        route = f"/documents/{document}/pages"
        run = next(run for run in rust("GET", f"{route}/0/text-runs")["runs"]
                   if run["text"].strip() == selected.strip())
        request = {"expected_revision": 1, "page_index": 0, "span_index": run["index"],
                   "expected_text": run["text"]}
        actual = rust("POST", f"{route}/paragraph-trial-preview", request)
        assert actual["line_count"] == expected["line_count"] == 1, case
        assert (actual["text"], actual["font_name"]) == (expected["text"], expected["font_name"]), case
        assert all(abs(a - b) < .02 for a, b in zip(actual["bbox"], expected["bbox"], strict=True)), (
            case, actual, expected
        )
        if replacement is None:
            continue
        request.update(expected_paragraph=expected["text"], replacement=replacement)
        if edit_error:
            try:
                replace_paragraph_trial(reference, 0, 0, selected, expected["text"], replacement)
            except ProcessingError as error:
                assert str(error) == edit_error, case
            else:
                raise AssertionError(f"Unexpected reference success for {case}")
            assert rust("POST", f"{route}/paragraph-trial", request, status=422) == {"detail": edit_error}
            assert pdf_bytes(rust, document, 1) == original
            continue
        expected_layout = replace_paragraph_trial(reference, 0, 0, selected, expected["text"], replacement)
        edited = rust("POST", f"{route}/paragraph-trial", request)
        assert edited["revision"] == 2, case
        actual_layout = edited["edit_layout"]
        for field in ("following_text_shift", "following_text_objects_moved", "line_count"):
            assert actual_layout[field] == expected_layout[field], (case, field, actual_layout, expected_layout)
        for a, b in zip(actual_layout["edit_region"], expected_layout["edit_region"], strict=True):
            assert abs(a - b) < .02, (case, actual_layout, expected_layout)
        with pymupdf.open(reference) as expected_pdf, pymupdf.open(stream=pdf_bytes(rust, document, 2)) as actual_pdf:
            assert expected_pdf[0].get_text(sort=True) == actual_pdf[0].get_text(sort=True), case
            drawings = lambda page: [{key: value for key, value in path.items() if key != "seqno"}
                                     for path in page.get_drawings()]
            assert drawings(expected_pdf[0]) == drawings(actual_pdf[0]), case
        assert pdf_bytes(rust, document, 1, "original") == original


def embedded_paragraph_object_bounds(rust, matter, root):
    from app.paragraph_trial import preview_paragraph_trial, replace_paragraph_trial
    from app.pdf_ops import ProcessingError, editable_text_runs

    selected = "SYNTHETICCAPITALLETTERS"
    neighbor = "ANOTHERUNTOUCHEDLINE"
    replacement = "SYNTHETICLETTERS"
    for face, separation in (("helv", 10), ("helv", 11), ("china-s", 11)):
        case = f"{face}-{separation}"
        source = root / f"paragraph-embedded-ink-{case}.pdf"
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=400, height=700)
            page.insert_font(fontname="Embedded", fontbuffer=pymupdf.Font(face).buffer)
            page.insert_text((37, 90), selected, fontsize=14, fontname="Embedded")
            page.insert_text((37, 90 + separation), neighbor, fontsize=14, fontname="Embedded")
            pdf.save(source)
        original = source.read_bytes()
        reference = root / f"paragraph-embedded-ink-reference-{case}.pdf"
        reference.write_bytes(original)
        runs = editable_text_runs(reference, 0)
        assert pymupdf.Rect(runs[0]["bbox"]).intersects(pymupdf.Rect(runs[1]["bbox"])), case
        expected = preview_paragraph_trial(reference, 0, 0, selected)

        document = rust("IMPORT", matter, source=str(source), status=201)[0]["id"]
        route = f"/documents/{document}/pages"
        run = next(run for run in rust("GET", f"{route}/0/text-runs")["runs"]
                   if run["text"] == selected)
        request = {"expected_revision": 1, "page_index": 0, "span_index": run["index"],
                   "expected_text": run["text"]}
        preview = rust("POST", f"{route}/paragraph-trial-preview", request)
        assert preview["text"] == expected["text"] == selected, case
        assert preview["line_count"] == expected["line_count"] == 1, case
        assert all(abs(a - b) < .12 for a, b in zip(preview["bbox"], expected["bbox"], strict=True)), (
            case, preview, expected
        )
        request.update(expected_paragraph=selected, replacement=replacement)
        try:
            expected_layout = replace_paragraph_trial(reference, 0, 0, selected, selected, replacement)
        except ProcessingError as error:
            detail = "Paragraph overlaps another text object"
            assert separation == 10 and str(error) == detail, case
            assert rust("POST", f"{route}/paragraph-trial", request, status=422) == {"detail": detail}
            assert pdf_bytes(rust, document, 1) == original, case
        else:
            assert separation > 10, case
            edit = rust("POST", f"{route}/paragraph-trial", request)
            assert edit["revision"] == 2, case
            layout = edit["edit_layout"]
            for key in ("following_text_shift", "following_text_objects_moved", "font_substituted", "line_count"):
                assert layout[key] == expected_layout[key], (case, key, layout, expected_layout)
            assert all(abs(a - b) < .12 for a, b in zip(
                layout["edit_region"], expected_layout["edit_region"], strict=True
            )), (case, layout, expected_layout)
            with (pymupdf.open(reference) as expected_pdf,
                  pymupdf.open(stream=pdf_bytes(rust, document, 2)) as actual_pdf,
                  pymupdf.open(stream=original) as original_pdf):
                assert actual_pdf[0].get_text(sort=True) == expected_pdf[0].get_text(sort=True), case
                assert actual_pdf[0].search_for(neighbor) == original_pdf[0].search_for(neighbor), case
                clip = pymupdf.Rect(0, 93, 400, 117)
                assert (actual_pdf[0].get_pixmap(clip=clip).samples ==
                        original_pdf[0].get_pixmap(clip=clip).samples), case
        assert pdf_bytes(rust, document, 1, "original") == original, case


def paragraph_reflow_destination(rust, matter, root):
    from app.paragraph_trial import preview_paragraph_trial, replace_paragraph_trial
    from app.pdf_ops import ProcessingError

    selected = "MAIN SEED"
    following = "Synthetic long following line connects the two columns"
    replacement = "FIRST\nSECOND\nTHIRD"
    for blocked in (False, True):
        source = root / f"paragraph-reflow-destination-{blocked}.pdf"
        fixed = "pg" if blocked else "BIG GLYPHS pg"
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=430, height=420)
            page.insert_text((40, 100), selected, fontsize=12)
            page.insert_text((177, 129 if blocked else 106), fixed,
                             fontsize=90 if blocked else 24, fontname="cour")
            page.insert_text((40, 115), following, fontsize=10)
            pdf.save(source)
        original = source.read_bytes()
        reference = root / f"paragraph-reflow-destination-reference-{blocked}.pdf"
        reference.write_bytes(original)
        preview = preview_paragraph_trial(reference, 0, 0, selected)
        assert preview["text"] == selected and preview["line_count"] == 1

        document = rust("IMPORT", matter, source=str(source), status=201)[0]["id"]
        route = f"/documents/{document}/pages"
        run = next(run for run in rust("GET", f"{route}/0/text-runs")["runs"]
                   if run["text"] == selected)
        body = {"page_index": 0, "span_index": run["index"], "expected_revision": 1,
                "expected_text": selected, "expected_paragraph": preview["text"], "replacement": replacement}
        try:
            expected = replace_paragraph_trial(reference, 0, 0, selected, preview["text"], replacement)
        except ProcessingError as error:
            message = "Following text cannot be moved without covering other text"
            assert blocked and str(error) == message
            assert rust("POST", f"{route}/paragraph-trial", body, status=422) == {"detail": message}
            assert pdf_bytes(rust, document, 1) == original
        else:
            assert not blocked
            result = rust("POST", f"{route}/paragraph-trial", body)
            assert result["revision"] == 2
            layout = result["edit_layout"]
            assert layout["line_count"] == expected["line_count"] == 3
            assert layout["following_text_objects_moved"] == expected["following_text_objects_moved"] == 1
            assert layout["following_text_shift"] == expected["following_text_shift"] == 28.67
            with (pymupdf.open(stream=original) as original_pdf,
                  pymupdf.open(stream=pdf_bytes(rust, document, 2)) as edited,
                  pymupdf.open(reference) as python_pdf):
                assert edited[0].get_text(sort=True) == python_pdf[0].get_text(sort=True)
                assert edited[0].search_for(fixed) == original_pdf[0].search_for(fixed)
                old_box, = original_pdf[0].search_for(following)
                moved_box, = edited[0].search_for(following)
                assert abs(moved_box.y0 - old_box.y0 - layout["following_text_shift"]) < .02
                assert moved_box.x0 == old_box.x0
        assert pdf_bytes(rust, document, 1, "original") == original


def paragraph_annotation_obstacles(rust, matter, root):
    from app.paragraph_trial import preview_paragraph_trial, replace_paragraph_trial
    from app.pdf_ops import ProcessingError

    selected = "MAIN SEED"
    replacement = "REVISED"
    for case in ("inactive-link", "active-link", "redaction-outside", "redaction-inside"):
        source = root / f"paragraph-annotation-{case}.pdf"
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=400, height=400)
            page.insert_text((40, 100), selected, fontsize=12)
            box = pymupdf.Rect(40, 88, 85, 110)
            if case.endswith("-link"):
                page.insert_link({"kind": pymupdf.LINK_URI, "from": box,
                                  "uri": "https://example.test/matter"})
                if case == "inactive-link":
                    page = pdf.reload_page(page)
                    [(xref, _, _)] = page.annot_xrefs()
                    pdf.xref_set_key(xref, "A", "null")
            else:
                page.add_redact_annot(box if case == "redaction-inside" else (110, 270, 230, 310),
                                      fill=(.7, .8, .2), cross_out=True, text="PENDING")
            pdf.save(source)
        original = source.read_bytes()
        reference = root / f"paragraph-annotation-reference-{case}.pdf"
        reference.write_bytes(original)
        expected = preview_paragraph_trial(reference, 0, 0, selected)
        document = rust("IMPORT", matter, source=str(source), status=201)[0]["id"]
        route = f"/documents/{document}/pages"
        run = next(run for run in rust("GET", f"{route}/0/text-runs")["runs"]
                   if run["text"] == selected)
        body = {"page_index": 0, "span_index": run["index"], "expected_revision": 1,
                "expected_text": selected}
        preview = rust("POST", f"{route}/paragraph-trial-preview", body)
        assert expected["text"] == preview["text"] == selected, case
        body.update(expected_paragraph=selected, replacement=replacement)
        try:
            replace_paragraph_trial(reference, 0, 0, selected, selected, replacement)
        except ProcessingError as error:
            message = "Paragraph overlaps an image, graphic, link or annotation"
            assert case in ("active-link", "redaction-inside") and str(error) == message
            assert rust("POST", f"{route}/paragraph-trial", body, status=422) == {"detail": message}
            assert pdf_bytes(rust, document, 1) == original, case
        else:
            assert case in ("inactive-link", "redaction-outside")
            result = rust("POST", f"{route}/paragraph-trial", body)
            assert result["revision"] == 2, case
            with (pymupdf.open(stream=original) as original_pdf,
                  pymupdf.open(stream=pdf_bytes(rust, document, 2)) as edited):
                before, after = original_pdf[0], edited[0]
                assert after.get_text().strip() == replacement, case
                assert not before.get_links() and not after.get_links(), case
                assert [kind for _, kind, _ in after.annot_xrefs()] == [
                    kind for _, kind, _ in before.annot_xrefs()
                ], case
                if case == "redaction-outside":
                    [prior], [retained] = list(before.annots()), list(after.annots())
                    assert (retained.rect, retained.info, retained.colors) == (
                        prior.rect, prior.info, prior.colors
                    ), case
                    clip = pymupdf.Rect(110, 270, 230, 310)
                    assert before.get_pixmap(clip=clip).samples == after.get_pixmap(clip=clip).samples, case
        assert pdf_bytes(rust, document, 1, "original") == original, case


def rotated_text_edit(rust, matter, root):
    for angle in (90, 180, 270):
        file = root / f"rotated-text-{angle}.pdf"
        with pymupdf.open() as sample:
            page = sample.new_page(width=460, height=520)
            page.insert_text((56, 85), "Text for edit")
            page.insert_text((56, 195), "Independent line")
            page.set_cropbox(pymupdf.Rect(20, 21, 440, 490))
            page.set_rotation(angle)
            sample.save(file)
        document = rust("IMPORT", matter, source=str(file), status=201)[0]["id"]
        path = f"/documents/{document}/pages"
        run = next(item for item in rust("GET", path + "/0/text-runs")["runs"]
                   if item["text"] == "Text for edit")
        edited = rust("POST", path + "/replace-text-run", {
            "expected_revision": 1, "page_index": 0, "span_index": run["index"],
            "expected_text": run["text"], "replacement": "Edited text",
        })
        with pymupdf.open(stream=pdf_bytes(rust, document, edited["revision"])) as output:
            assert output[0].rotation == angle
            assert output[0].cropbox == pymupdf.Rect(20, 21, 440, 490)
            assert "Edited text" in output[0].get_text() and run["text"] not in output[0].get_text()
            assert "Independent line" in output[0].get_text()
        assert pdf_bytes(rust, document, 1, "original") == file.read_bytes()


def text_edit_preserves_document(rust, root):
    import pypdfium2
    from PIL import ImageChops

    fixture = root / "text-edit-document.pdf"
    with pymupdf.open() as pdf:
        first = pdf.new_page(width=520, height=420)
        first.draw_rect(pymupdf.Rect(28, 64, 172, 126), color=(.1, .3, .8), fill=(.85, .9, .97))
        raster = BytesIO()
        Image.new("RGB", (80, 40), (227, 210, 174)).save(raster, format="PNG")
        first.insert_image(pymupdf.Rect(28, 150, 172, 213), stream=raster.getvalue(), keep_proportion=False)
        first.insert_text((45, 100), "MOVE VECTOR", fontsize=14)
        first.insert_text((45, 185), "MOVE RASTER", fontsize=14)
        first.insert_text((45, 258), "EDIT ME", fontsize=14)
        first.insert_text((176, 258), "Independent label", fontsize=14)
        first.add_text_annot((430, 312), "Synthetic note")
        field = pymupdf.Widget()
        field.field_name = "Synthetic field"
        field.field_type = pymupdf.PDF_WIDGET_TYPE_TEXT
        field.field_value = "Unchanged value"
        field.rect = pymupdf.Rect(312, 345, 495, 369)
        first.add_widget(field)
        second = pdf.new_page(width=520, height=420)
        for key in ("Contents", "Resources"):
            pdf.xref_set_key(second.xref, key, pdf.xref_get_key(pdf[0].xref, key)[1])
        pdf[0].insert_link({"kind": pymupdf.LINK_URI, "from": pymupdf.Rect(310, 285, 380, 305),
                            "uri": "https://example.invalid/qa"})
        pdf.set_toc([[1, "Second page", 2]])
        pdf.set_page_labels([{"startpage": 0, "prefix": "S-", "style": "D", "firstpagenum": 1}])
        pdf.set_metadata({"title": "Synthetic edit graph", "author": "Fixture author"})
        pdf.set_xml_metadata("<qa>synthetic metadata</qa>")
        pdf.embfile_add("fixture.txt", b"synthetic attachment")
        destination = f"[{pdf[1].xref} 0 R /XYZ 61 350 1.4]"
        pdf.xref_set_key(pdf.pdf_catalog(), "Dests", f"<< /Section {destination} >>")
        pdf.xref_set_key(pdf.pdf_catalog(), "OpenAction", destination)
        pdf.save(fixture)
    original = fixture.read_bytes()

    def inspect(contents):
        with pymupdf.open(stream=contents) as pdf:
            assert len(pdf) == 2
            catalog = pdf.pdf_catalog()
            kind, location = pdf.xref_get_key(catalog, "Dests/Section")
            assert kind == "array" and re.fullmatch(rf"\[\s*{pdf[1].xref} 0 R\s*/XYZ 61 350 1\.4\s*\]", location)
            assert pdf.xref_get_key(catalog, "OpenAction") == (kind, location)
            assert pdf.get_toc() == [[1, "Second page", 2]]
            assert [page.get_label() for page in pdf] == ["S-1", "S-2"]
            assert pdf.embfile_get("fixture.txt") == b"synthetic attachment"
            assert (pdf.metadata["title"], pdf.metadata["author"]) == ("Synthetic edit graph", "Fixture author")
            assert pdf.get_xml_metadata() == "<qa>synthetic metadata</qa>"
            assert [link["uri"] for link in pdf[0].get_links() if link["kind"] == pymupdf.LINK_URI] == ["https://example.invalid/qa"]
            assert [note.info["content"] for note in pdf[0].annots()] == ["Synthetic note"]
            widget, = pdf[0].widgets()
            assert (widget.field_name, widget.field_value) == ("Synthetic field", "Unchanged value")
            field_type, fields = pdf.xref_get_key(catalog, "AcroForm/Fields")
            assert field_type == "array" and re.fullmatch(rf"\[\s*{widget.xref} 0 R\s*\]", fields)
            appearance_type, appearance = pdf.xref_get_key(widget.xref, "AP/N")
            assert appearance_type == "xref" and pdf.xref_stream(int(appearance.split()[0]))
            return pdf[0].get_text(), pdf[1].get_text()

    def render(contents, page):
        with pypdfium2.PdfDocument(contents) as pdf:
            rendered = pdf[page].render(scale=2)
            return rendered.to_pil().convert("RGB")

    original_text = inspect(original)
    original_render = render(original, 0)
    original_other = render(original, 1)
    auth = {"Authorization": "Bearer synthetic"}
    with TestClient(create_app(root / "text-edit-reference", token="synthetic", require_vault=False)) as python:
        def reference(method, path, body=None, *, status=200, **kwargs):
            headers = {**auth, **kwargs.pop("headers", {})}
            response = python.request(method, path, json=body, headers=headers, **kwargs)
            assert response.status_code == status, (method, path, response.text)
            return response

        def reference_json(*args, **kwargs):
            return reference(*args, **kwargs).json()

        matter = {"name": "Synthetic document graph text edits"}
        rust_matter = rust("POST", "/matters", matter, status=201)["id"]
        python_matter = reference_json("POST", "/matters", matter, status=201)["id"]
        for operation, label, top, bottom in (
            ("move", "MOVE VECTOR", 79, 106),
            ("move", "MOVE RASTER", 164, 191),
            ("replace", "EDIT ME", 237, 264),
            ("paragraph", "EDIT ME", 237, 264),
        ):
            native = rust("IMPORT", rust_matter, source=str(fixture), status=201)[0]["id"]
            baseline = reference_json("POST", f"/matters/{python_matter}/documents/raw", status=201, content=original,
                                      headers={"Content-Type": "application/octet-stream", "X-CourtBundle-Filename": fixture.name})[0]["id"]
            paths = (f"/documents/{native}", f"/documents/{baseline}")
            for backend, path in zip((rust, reference_json), paths, strict=True):
                run = next(item for item in backend("GET", path + "/pages/0/text-runs")["runs"] if item["text"] == label)
                args = {"expected_revision": 1, "page_index": 0, "span_index": run["index"], "expected_text": label}
                if operation == "move":
                    backend("POST", path + "/pages/move-text-run", {**args, "horizontal_offset": .05, "vertical_offset": 0}, status=422)
                    args.update(horizontal_offset=.42, vertical_offset=0)
                elif operation == "replace":
                    args["replacement"] = "@@@@@@@@@@"
                endpoint = operation + "-text-run"
                if operation == "paragraph":
                    preview = backend("POST", path + "/pages/paragraph-trial-preview", args)
                    assert preview["text"] == label
                    args.update(expected_paragraph=preview["text"], replacement="UPDATED")
                    endpoint = "paragraph-trial"
                result = backend("POST", path + "/pages/" + endpoint, args)
                assert result["revision"] == 2
                if operation == "replace":
                    assert result["edit_layout"]["font_size"] == 12.6
                    assert result["edit_layout"]["font_size_reduced"]
            actual = pdf_bytes(rust, native, 2)
            expected = reference("GET", paths[1] + "/pdf").content
            assert inspect(actual)[1] == inspect(expected)[1] == original_text[1]
            changed_label = {"move": label, "replace": "@@@@@@@@@@", "paragraph": "UPDATED"}[operation]
            for contents in (actual, expected):
                with pymupdf.open(stream=contents) as pdf:
                    assert pdf[0].get_text().count(changed_label) == 1
                    rect, = pdf[0].search_for(changed_label)
                    assert abs(rect.x0 - (45 + (.42 * pdf[0].rect.width if operation == "move" else 0))) < .1
                    if operation != "move":
                        neighbor, = pdf[0].search_for("Independent label")
                        assert rect.x1 < neighbor.x0 - 2
                        assert label not in pdf[0].get_text()
                assert ImageChops.difference(original_other, render(contents, 1)).getbbox() is None
                changed = ImageChops.difference(original_render, render(contents, 0)).getbbox()
                assert changed and top * 2 <= changed[1] < changed[3] <= bottom * 2, changed
            if operation != "paragraph":
                assert ImageChops.difference(render(actual, 0), render(expected, 0)).getbbox() is None
            assert pdf_bytes(rust, native, 1, "original") == original
            assert reference("GET", paths[1] + "/original").content == original
            (root / f"{label.lower().replace(' ', '-')}-rust.pdf").write_bytes(actual)
            (root / f"{label.lower().replace(' ', '-')}-reference.pdf").write_bytes(expected)


def shared_form_text_edit(rust, root):
    import pypdfium2
    from PIL import ImageChops
    from app.pdf_ops import editable_text_runs, move_text_run, replace_text_run

    matter = rust("POST", "/matters", {"name": "Synthetic Form text edits"}, status=201)["id"]
    variants = (("replace", 1, "explicit"), ("replace", .75, "explicit"),
                ("move", 1, "explicit"), ("replace", 1, "form-state"),
                ("replace", 1, "inherited-state"), ("move", 1, "form-state"))
    for operation, scale, font_state in variants:
        fixture = root / f"shared-form-{operation}-{scale}-{font_state}.pdf"
        with pymupdf.open() as leaf, pymupdf.open() as pdf:
            inside = leaf.new_page(width=360, height=130)
            inside.draw_rect(pymupdf.Rect(15, 42, 165, 87), color=(.2, .3, .6), fill=(.9, .94, .97))
            inside.insert_text((24, 66), "EDIT IN FORM", fontsize=16)
            inside.insert_text((24, 107), "Independent Form line", fontsize=12)
            page = pdf.new_page(width=520, height=570)
            form = page.show_pdf_page(pymupdf.Rect(55, 40, 55 + 360 * scale, 40 + 130 * scale), leaf, 0)
            # Paint the exact same outer Form binding again under a distinct CTM.
            content, = page.get_contents()
            first = pdf.xref_stream(content)
            second_paint = b"\nq 1 0 0 1 0 -240 cm\n" + first + b"\nQ\n"
            if font_state != "explicit":
                local = int(pdf.xref_get_key(form, "Resources")[1].split()[0])
                font = int(pdf.xref_get_key(local, "Font/helv")[1].split()[0])
                commands = pdf.xref_stream(form)
                if font_state == "form-state":
                    wrapper = next(xref for xref, _, parent, _ in page.get_xobjects() if parent == 0)
                    pdf.xref_set_key(form, "Resources", "null")
                    pdf.xref_set_key(wrapper, "Resources", f"""
                        << /XObject << /fullpage {form} 0 R >>
                           /ExtGState << /Large << /Font [{font} 0 R 16] >>
                                         /Small << /Font [{font} 0 R 12] >> >> >>
                    """)
                    commands = commands.replace(b"/helv 16 Tf", b"/Large gs")
                    commands = commands.replace(b"/helv 12 Tf", b"/Small gs")
                else:
                    resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
                    pdf.xref_set_key(resources, "ExtGState",
                                     f"<< /Large << /Font [{font} 0 R 16] >> /Small << /Font [{font} 0 R 14] >> >>")
                    commands = commands.replace(b"/helv 16 Tf", b"")
                    second_paint = b"\nq /Small gs 1 0 0 1 0 -240 cm\n" + first + b"\nQ\n"
                    first = b"q /Large gs\n" + first + b"\nQ\n"
                pdf.update_stream(form, commands)
            pdf.update_stream(content, first + second_paint)
            second = pdf.new_page(width=520, height=570)
            for key in ("Contents", "Resources"):
                pdf.xref_set_key(second.xref, key, pdf.xref_get_key(pdf[0].xref, key)[1])
            pdf.save(fixture)
        original = fixture.read_bytes()
        label = "EDIT IN FORM"
        replacement = "REVISED FORM" if operation == "replace" else label
        reference = fixture.with_name(f"reference-{fixture.name}")
        reference.write_bytes(original)
        source = min((run for run in editable_text_runs(reference, 0) if run["text"] == label),
                     key=lambda run: run["bbox"][1])
        if operation == "replace":
            replace_text_run(reference, 0, source["index"], label, replacement)
        else:
            move_text_run(reference, 0, source["index"], label, vertical_offset=0, horizontal_offset=.42)
        document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
        path = f"/documents/{document}/pages"
        run = min((run for run in rust("GET", path + "/0/text-runs")["runs"] if run["text"] == label),
                  key=lambda run: run["bbox"][1])
        args = {"expected_revision": 1, "page_index": 0, "span_index": run["index"], "expected_text": label}
        if operation == "replace":
            args["replacement"] = replacement
        else:
            args.update(horizontal_offset=.42, vertical_offset=0)
        edited = rust("POST", path + f"/{operation}-text-run", args)
        assert edited["revision"] == 2
        assert pdf_bytes(rust, document, 1, "original") == original

        def render(contents, page):
            with pypdfium2.PdfDocument(contents) as pdf:
                return pdf[page].render(scale=2).to_pil().convert("RGB")

        actual = pdf_bytes(rust, document, 2)
        expected = reference.read_bytes()
        baseline = render(original, 0)
        other = render(original, 1)
        for contents in (actual, expected):
            with pymupdf.open(stream=contents) as pdf:
                assert pdf[0].get_text().count("Independent Form line") == 2
                assert pdf[0].get_text().count(label) == (2 if operation == "move" else 1)
                assert pdf[1].get_text().count(label) == 2
                updated = min(pdf[0].search_for(replacement), key=lambda box: box.y0)
                assert updated.y0 < 140
                assert abs(updated.x0 - (55 + 24 * scale + (.42 * 520 if operation == "move" else 0))) < .1
                if font_state == "inherited-state":
                    spans = (span for block in pdf[0].get_text("dict")["blocks"]
                             for line in block.get("lines", []) for span in line["spans"])
                    assert next(span for span in spans if span["text"] == label)["size"] == 14
                assert max(box.y0 for box in pdf[0].search_for(label)) > 280
            assert ImageChops.difference(other, render(contents, 1)).getbbox() is None
            changed = ImageChops.difference(baseline, render(contents, 0)).getbbox()
            assert changed and (40 + 47 * scale) * 2 <= changed[1] < changed[3] <= (40 + 79 * scale) * 2, changed
        assert ImageChops.difference(render(actual, 0), render(expected, 0)).getbbox() is None, (operation, scale)
        fixture.with_name(f"rust-{fixture.name}").write_bytes(actual)


def tagged_form_text_edit(rust, root):
    import pypdfium2
    from PIL import ImageChops
    from app.pdf_ops import editable_text_runs, move_text_run, replace_text_run

    matter = rust("POST", "/matters", {"name": "Synthetic tagged Form text"}, status=201)["id"]
    old_text, new_text = "TAGGED EDIT HERE", "TAGGED REVISED"
    for case in ("marked", "foreign", "object", "shared-object", "graphics-state"):
        fixture = root / f"tagged-form-{case}.pdf"
        is_object = case in ("object", "shared-object")
        with pymupdf.open() as leaf, pymupdf.open() as pdf:
            inside = leaf.new_page(width=360, height=130)
            inside.insert_text((24, 51), old_text, fontsize=16)
            inside.insert_text((24, 103), "TAGGED SIBLING", fontsize=13)
            first = pdf.new_page(width=520, height=570)
            form = first.show_pdf_page(pymupdf.Rect(55, 40, 415, 170), leaf)
            if case == "graphics-state":
                local = int(pdf.xref_get_key(form, "Resources")[1].split()[0])
                font = int(pdf.xref_get_key(local, "Font/helv")[1].split()[0])
                pdf.xref_set_key(local, "Font", "null")
                pdf.xref_set_key(local, "ExtGState",
                                 f"<< /Large << /Font [{font} 0 R 16] >> /Small << /Font [{font} 0 R 13] >> >>")
                stream = pdf.xref_stream(form).replace(b"/helv 16 Tf", b"/Large gs")
                pdf.update_stream(form, stream.replace(b"/helv 13 Tf", b"/Small gs"))
            if case in ("foreign", "shared-object"):
                second = pdf.new_page(width=520, height=570)
                for key in ("Contents", "Resources"):
                    pdf.xref_set_key(second.xref, key, pdf.xref_get_key(pdf[0].xref, key)[1])
            struct, tree, branch = (pdf.get_new_xref() for _ in range(3))
            owner = pdf.get_new_xref()
            kids = [owner]
            pairs = []
            page = pdf[-1].xref
            if is_object:
                pdf.xref_set_key(form, "StructParent", "7")
                refs = " ".join(
                    f"<< /Type /OBJR /Pg {entry.xref} 0 R /Obj {form} 0 R >>"
                    for entry in pdf
                )
                pdf.update_object(owner, f"<< /Type /StructElem /S /Figure /P {struct} 0 R "
                                         f"/Pg {page} 0 R /Alt (Tagged object) /K [{refs}] >>")
                pairs.append(f"7 {owner} 0 R")
            else:
                pdf.xref_set_key(form, "StructParents", "7")
                sibling = pdf.get_new_xref()
                kids.append(sibling)
                data = pdf.xref_stream(form)
                blocks = list(re.finditer(rb"BT\b.*?\bET", data, flags=re.S))
                assert len(blocks) == 2
                for mcid, block in reversed(list(enumerate(blocks))):
                    data = (data[:block.start()] + f"/P <</MCID {mcid}>> BDC\n".encode()
                            + block[0] + b"\nEMC\n" + data[block.end():])
                pdf.update_stream(form, data)
                for mcid, item in enumerate((owner, sibling)):
                    pdf.update_object(item, f"<< /Type /StructElem /S /P /P {struct} 0 R /Pg {page} 0 R "
                                            f"/K << /Type /MCR /Stm {form} 0 R /MCID {mcid} >> >>")
                pairs.append(f"7 [{owner} 0 R {sibling} 0 R]")
                if case == "marked":
                    # A sparse existing page parent array and a named property list.
                    unrelated = pdf.get_new_xref()
                    kids.append(unrelated)
                    pdf[0].insert_text((79, 214), "TAGGED PAGE LINE", fontsize=14, fontname="cour")
                    stream = pdf[0].get_contents()[-1]
                    pdf.update_stream(stream, b"/P /ExistingTag BDC\n" + pdf.xref_stream(stream) + b"\nEMC\n")
                    resources = int(pdf.xref_get_key(page, "Resources")[1].split()[0])
                    pdf.xref_set_key(resources, "Properties", "<< /ExistingTag << /MCID 3 >> >>")
                    pdf.xref_set_key(page, "StructParents", "2")
                    pdf.update_object(unrelated, f"<< /Type /StructElem /S /P /Pg {page} 0 R "
                                                 f"/P {struct} 0 R /K 3 >>")
                    pairs.insert(0, f"2 [null null null {unrelated} 0 R]")
            child_refs = " ".join(f"{kid} 0 R" for kid in kids)
            pdf.update_object(struct, f"<< /Type /StructTreeRoot /K [{child_refs}] "
                                      f"/ParentTree {tree} 0 R /ParentTreeNextKey 8 >>")
            pdf.update_object(tree, f"<< /Kids [{branch} 0 R] >>")
            pdf.update_object(branch, f"<< /Nums [{' '.join(pairs)}] /Limits "
                                      f"[{2 if case == 'marked' else 7} 7] >>")
            pdf.xref_set_key(pdf.pdf_catalog(), "StructTreeRoot", f"{struct} 0 R")
            pdf.xref_set_key(pdf.pdf_catalog(), "MarkInfo", "<< /Marked true >>")
            pdf.save(fixture)

        source = fixture.read_bytes()
        expected_path = root / f"tagged-form-{case}-python.pdf"
        expected_path.write_bytes(source)
        selected = next(run for run in editable_text_runs(expected_path, 0) if run["text"] == old_text)
        if case == "shared-object":
            move_text_run(expected_path, 0, selected["index"], old_text, 0, .42)
        else:
            replace_text_run(expected_path, 0, selected["index"], old_text, new_text)
        document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
        route = f"/documents/{document}/pages"

        def change(old, revised, revision):
            run = next(run for run in rust("GET", route + "/0/text-runs")["runs"] if run["text"] == old)
            params = {"expected_revision": revision, "page_index": 0,
                      "span_index": run["index"], "expected_text": old}
            if revised is None:
                result = rust("POST", route + "/move-text-run",
                              {**params, "vertical_offset": 0, "horizontal_offset": .42})
            else:
                result = rust("POST", route + "/replace-text-run", {**params, "replacement": revised})
            assert result["revision"] == revision + 1

        change(old_text, None if case == "shared-object" else new_text, 1)
        revision = 2
        caption = old_text if case == "shared-object" else new_text
        if case == "marked":
            prior = pdf_bytes(rust, document, revision)
            chosen = next(run for run in editable_text_runs(expected_path, 0) if run["text"] == new_text)
            caption = "SECOND TAGGED EDIT"
            replace_text_run(expected_path, 0, chosen["index"], new_text, caption)
            change(new_text, caption, revision)
            revision += 1
        actual = pdf_bytes(rust, document, revision)
        (root / f"tagged-form-{case}-rust.pdf").write_bytes(actual)
        with pypdfium2.PdfDocument(actual) as output, pypdfium2.PdfDocument(expected_path) as reference:
            assert len(output) == len(reference)
            for a, b in zip(output, reference, strict=True):
                original_pixels = b.render(scale=2).to_pil().convert("RGB")
                edited_pixels = a.render(scale=2).to_pil().convert("RGB")
                assert ImageChops.difference(original_pixels, edited_pixels).getbbox() is None, case
        assert pdf_bytes(rust, document, 1, "original") == source
        if revision == 3:
            retained = rust("SNAPSHOT", f"/documents/{document}",
                            {"kind": "retained", "revision": 2, "sha256": sha256(prior).hexdigest()})
            assert bytes(retained["bytes"]) == prior
        with pymupdf.open(stream=actual) as pdf:
            def get_ref(ref, key):
                kind, value = pdf.xref_get_key(ref, key)
                assert kind == "xref", (case, ref, key, kind)
                return int(value.split()[0])

            def ref_list(ref, key):
                return [int(key) for key in re.findall(r"(\d+) 0 R", pdf.xref_get_key(ref, key)[1])]

            def content_link(item, marker, target_page, mcid=None):
                kids = pdf.xref_get_key(item, "K")[1]
                for element in re.findall(r"<<[^<>]*>>", kids):
                    declared_page = re.search(r"/Pg\s*(\d+) 0 R", element)
                    page_owner = int(declared_page[1]) if declared_page else get_ref(item, "Pg")
                    if page_owner != target_page:
                        continue
                    if mcid is not None and not re.search(rf"/MCID\s*{mcid}\b", element):
                        continue
                    link = re.search(rf"/{marker}\s*(\d+) 0 R", element)
                    if link:
                        return int(link[1])
                raise AssertionError((case, item, marker, target_page, mcid, kids))

            root_obj = get_ref(pdf.pdf_catalog(), "StructTreeRoot")
            edited_owner, *others = ref_list(root_obj, "K")
            tagged_page = pdf[-1].xref if case == "foreign" else pdf[0].xref
            form_obj = content_link(edited_owner, "Obj" if is_object else "Stm", tagged_page,
                                    None if is_object else 0)
            form_key = int(pdf.xref_get_key(form_obj, "StructParent" if is_object else "StructParents")[1])
            drawn = pdf.xref_stream(form_obj)
            assert b"TAGGED SIBLING" in drawn or b"TAGGED SIBLING".hex().encode() in drawn.lower(), drawn
            assert (old_text.encode() in drawn or old_text.encode().hex().encode() in drawn.lower()) == (case == "foreign")
            page_bytes = pdf[0].read_contents()
            assert pdf[0].get_text().count(caption) == 1
            assert pdf[0].get_text().count("TAGGED SIBLING") == 1
            if is_object:
                assert pdf.xref_get_key(edited_owner, "Alt") == ("string", "Tagged object")
            else:
                assert content_link(others[0], "Stm", tagged_page, 1) == form_obj
            if case == "foreign":
                assert pdf.xref_get_key(pdf[0].xref, "StructParents") == ("null", "null")
                assert pdf[1].get_text().count(old_text) == 1
                assert form_key == 7
                continue
            assert b"/MCID" in page_bytes
            parent_tree = get_ref(root_obj, "ParentTree")
            nums = pdf.xref_get_key(parent_tree, "Nums")[1]
            page_key = int(pdf.xref_get_key(pdf[0].xref, "StructParents")[1])
            next_key = int(pdf.xref_get_key(root_obj, "ParentTreeNextKey")[1])
            assert next_key > max(form_key, page_key)
            if case == "marked":
                assert page_key == 2 and form_key == 7
                unrelated = others[-1]
                assert pdf.xref_get_key(unrelated, "K") == ("int", "3")
                assert re.search(rf"\b2\s*\[null null null {unrelated} 0 R null {edited_owner} 0 R\]", nums)
                assert re.search(rb"/MCID\s+5", page_bytes)
                assert re.search(r"/MCID\s*5\b", pdf.xref_get_key(edited_owner, "K")[1])
                assert not re.search(r"/MCID\s*4\b", pdf.xref_get_key(edited_owner, "K")[1])
            else:
                assert re.search(rf"\b{page_key}\s*\[{edited_owner} 0 R\]", nums), nums
            if case == "shared-object":
                second = content_link(edited_owner, "Obj", pdf[1].xref)
                assert second != form_obj and pdf.xref_get_key(second, "StructParent") == ("int", "7")
                assert form_key == 8 and page_key == 9
                assert pdf[1].get_text().count(old_text) == 1


def page_tagged_form_edit(rust, root):
    import pypdfium2
    from PIL import ImageChops
    from app.pdf_ops import editable_text_runs, move_text_run, replace_text_run

    fixture = root / "page-tagged-form.pdf"
    label, revised, sibling = "PAGE TAGGED FORM", "REVISED TAGGED FORM", "FORM SIBLING"
    with pymupdf.open() as leaf, pymupdf.open() as pdf:
        content = leaf.new_page(width=360, height=130)
        content.insert_text((24, 59), label, fontsize=16)
        content.insert_text((24, 105), sibling, fontsize=13)
        page = pdf.new_page(width=520, height=570)
        page.show_pdf_page(pymupdf.Rect(55, 40, 415, 170), leaf)
        ref, = page.get_contents()
        paint = pdf.xref_stream(ref)
        pdf.update_stream(ref, b"/P /TagOne BDC /Span BMC " + paint + b" EMC EMC\n"
                               + b"/P <</MCID 1>> BDC q 1 0 0 1 0 -230 cm " + paint + b" Q EMC\n")
        resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
        pdf.xref_set_key(resources, "Properties", "<< /TagOne << /MCID 0 >> >>")
        tree, numbers, upper, lower = (pdf.get_new_xref() for _ in range(4))
        pdf.xref_set_key(page.xref, "StructParents", "3")
        pdf.update_object(tree, f"<< /Type /StructTreeRoot /ParentTree {numbers} 0 R "
                                f"/K [{upper} 0 R {lower} 0 R] /ParentTreeNextKey 4 >>")
        pdf.update_object(numbers, f"<< /Nums [3 [{upper} 0 R {lower} 0 R]] >>")
        for mcid, element in enumerate((upper, lower)):
            pdf.update_object(element, f"<< /Type /StructElem /S /P /P {tree} 0 R "
                                       f"/Pg {page.xref} 0 R /K {mcid} >>")
        pdf.xref_set_key(pdf.pdf_catalog(), "StructTreeRoot", f"{tree} 0 R")
        pdf.xref_set_key(pdf.pdf_catalog(), "MarkInfo", "<< /Marked true >>")
        pdf.save(fixture)

    original = fixture.read_bytes()
    reference = root / "page-tagged-form-python.pdf"
    reference.write_bytes(original)
    selected = min((run for run in editable_text_runs(reference, 0) if run["text"] == label),
                   key=lambda run: run["bbox"][1])
    replace_text_run(reference, 0, selected["index"], label, revised)
    matter = rust("POST", "/matters", {"name": "Synthetic page-tagged Form"}, status=201)["id"]
    document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
    route = f"/documents/{document}/pages"
    runs = rust("GET", route + "/0/text-runs")["runs"]
    upper = min((run for run in runs if run["text"] == label), key=lambda run: run["bbox"][1])
    assert rust("POST", route + "/replace-text-run", {
        "expected_revision": 1, "page_index": 0, "span_index": upper["index"],
        "expected_text": label, "replacement": revised,
    })["revision"] == 2
    revision_two = pdf_bytes(rust, document, 2)

    def verify(data, count):
        with pymupdf.open(stream=data) as pdf:
            page = pdf[0]
            cat = pdf.pdf_catalog()
            contents = page.read_contents()
            rows = pdf.xref_get_key(cat, "StructTreeRoot/K")[1]
            upper, lower = (int(value) for value in re.findall(r"(\d+) 0 R", rows))
            parents = pdf.xref_get_key(cat, "StructTreeRoot/ParentTree/Nums")[1]
            links = [upper, lower, upper] + ([lower] if count == 2 else [])
            link_refs = " ".join(f"{ref} 0 R" for ref in links)
            assert re.fullmatch(rf"\[\s*3\s*\[{link_refs}\]\s*\]", parents), parents
            assert pdf.xref_get_key(page.xref, "StructParents") == ("int", "3")
            assert pdf.xref_get_key(cat, "StructTreeRoot/ParentTreeNextKey") == ("int", "4")
            page_ref = rf"/Pg\s*{page.xref} 0 R"
            def tagged_body(mcid):
                marked = re.search(rb"/MCID\s+" + str(mcid).encode() + rb"\s*>>\s*BDC(.*?)EMC", contents, re.S)
                assert marked, contents
                return marked[1].lower()

            upper_kids = pdf.xref_get_key(upper, "K")[1]
            assert re.search(rf"\[0\s*<<(?=[^>]*{page_ref})[^>]*?/MCID\s*2\b", upper_kids)
            assert revised in page.get_text()
            assert revised.encode().hex().encode() in tagged_body(2)
            assert page.get_text().count(sibling) == 2
            old, = page.search_for(label)
            assert abs(old.x0 - (79 + (208 if count == 2 else 0))) < .1
            if count == 2:
                lower_kids = pdf.xref_get_key(lower, "K")[1]
                assert re.search(rf"\[1\s*<<(?=[^>]*{page_ref})[^>]*?/MCID\s*3\b", lower_kids)
                assert label.encode().hex().encode() in tagged_body(3)
            else:
                assert pdf.xref_get_key(lower, "K") == ("int", "1")
        with pypdfium2.PdfDocument(data) as edited, pypdfium2.PdfDocument(reference) as python:
            actual_pixels = edited[0].render(scale=2).to_pil().convert("RGB")
            reference_pixels = python[0].render(scale=2).to_pil().convert("RGB")
            assert ImageChops.difference(actual_pixels, reference_pixels).getbbox() is None

    verify(revision_two, 1)
    run, = (run for run in editable_text_runs(reference, 0) if run["text"] == label)
    move_text_run(reference, 0, run["index"], label, vertical_offset=0, horizontal_offset=.4)
    run, = (run for run in rust("GET", route + "/0/text-runs")["runs"] if run["text"] == label)
    assert rust("POST", route + "/move-text-run", {
        "expected_revision": 2, "page_index": 0, "span_index": run["index"],
        "expected_text": label, "horizontal_offset": .4, "vertical_offset": 0,
    })["revision"] == 3
    actual = pdf_bytes(rust, document, 3)
    verify(actual, 2)
    retained = rust("SNAPSHOT", f"/documents/{document}",
                    {"kind": "retained", "revision": 2, "sha256": sha256(revision_two).hexdigest()})
    assert bytes(retained["bytes"]) == revision_two
    assert pdf_bytes(rust, document, 1, "original") == original
    (root / "page-tagged-form-rust.pdf").write_bytes(actual)


def actual_text_run_edit(rust, root):
    import pypdfium2
    from PIL import ImageChops
    from app.pdf_ops import editable_text_runs, move_text_run, replace_text_run

    matter = rust("POST", "/matters", {"name": "Synthetic alternate text edits"}, status=201)["id"]
    painted, alternate, revised = "VISUAL WORDS", "EDITOR WORDS", "UPDATED WORDS"
    sibling = "Independent nearby line"
    variants = ("inline", "tj", "named", "kerning", "rotated-crop", "structure", "ancestor",
                "structure-form", "outer-form", "repeated-inner-form")
    for case in variants:
        fixture = root / f"actual-text-{case}.pdf"
        with pymupdf.open() as pdf, pymupdf.open() as leaf:
            page = pdf.new_page(width=520, height=570)
            inline = b"/Span <</ActualText (EDITOR WORDS)>> BDC\n"
            if case in ("structure-form", "outer-form", "repeated-inner-form"):
                inner = leaf.new_page(width=360, height=145)
                inner.insert_text((20, 40), painted, fontsize=16)
                if case == "repeated-inner-form":
                    inner_ref, = inner.get_contents()
                    leaf.update_stream(inner_ref, inline + leaf.xref_stream(inner_ref) + b"\nEMC")
                page.show_pdf_page(pymupdf.Rect(50, 50, 410, 195), leaf)
                ref, = page.get_contents()
                data = pdf.xref_stream(ref)
                if case == "repeated-inner-form":
                    pdf.update_stream(ref, data + b"\nq 1 0 0 1 0 -235 cm " + data + b" Q\n")
            else:
                page.insert_text((70, 90), painted, fontsize=16)
                ref, = page.get_contents()
                data = pdf.xref_stream(ref)
            marker = inline
            if case == "named":
                resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
                prop = pdf.get_new_xref()
                encoded = "feff" + alternate.encode("utf-16-be").hex()
                pdf.update_object(prop, f"<< /ActualText <{encoded}> >>")
                pdf.xref_set_key(resources, "Properties", f"<< /MA {prop} 0 R >>")
                marker = b"/Span /MA BDC\n"
            if case in ("structure", "ancestor", "structure-form"):
                tree, numbers, owner = (pdf.get_new_xref() for _ in range(3))
                parent = pdf.get_new_xref() if case == "ancestor" else tree
                role = "/K 0"
                if case == "structure-form":
                    form = next(obj for obj, _, container, _ in page.get_xobjects() if container != 0)
                    pdf.xref_set_key(form, "StructParents", "1")
                    wrapped = b"/P <</MCID 0>> BDC\n" + pdf.xref_stream(form) + b"\nEMC"
                    pdf.update_stream(form, wrapped)
                    role = f"/K << /Type /MCR /Stm {form} 0 R /Pg {page.xref} 0 R /MCID 0 >>"
                else:
                    pdf.xref_set_key(page.xref, "StructParents", "1")
                if case == "ancestor":
                    pdf.update_object(parent, f"<< /Type /StructElem /S /Sect /P {tree} 0 R "
                                              f"/K {owner} 0 R /ActualText ({alternate}) >>")
                owner_text = f"/ActualText ({alternate})" if case != "ancestor" else ""
                pdf.update_object(owner, f"<< /Type /StructElem /S /P /P {parent} 0 R "
                                         f"/Pg {page.xref} 0 R {role} {owner_text} >>")
                top = parent if case == "ancestor" else owner
                pdf.update_object(tree, f"<< /Type /StructTreeRoot /K {top} 0 R "
                                        f"/ParentTree {numbers} 0 R /ParentTreeNextKey 2 >>")
                pdf.update_object(numbers, f"<< /Nums [1 [{owner} 0 R]] >>")
                pdf.xref_set_key(pdf.pdf_catalog(), "StructTreeRoot", f"{tree} 0 R")
                pdf.xref_set_key(pdf.pdf_catalog(), "MarkInfo", "<< /Marked true >>")
                marker = b"/P <</MCID 0>> BDC\n"
            if case == "tj":
                data = re.sub(rb"\[(<[^>]+>)\]TJ", rb"\1 Tj", data)
            if case == "kerning":
                data = re.sub(rb"\[(<[^>]+>)\]TJ", b"[(VISUAL) -60 ( WORDS)] TJ", data)
            if case not in ("repeated-inner-form", "structure-form"):
                pdf.update_stream(ref, marker + data + b"\nEMC")
            # Keep the same on-page font even for a page whose target came from a Form.
            page.insert_text((70, 142), sibling, fontname="cour")
            if case == "rotated-crop":
                page.set_cropbox(pymupdf.Rect(15, 24, 505, 542))
                page.set_rotation(90)
            pdf.save(fixture)

        original = fixture.read_bytes()
        reference = root / f"actual-text-{case}-python.pdf"
        reference.write_bytes(original)
        reference_text = painted if case in ("ancestor", "structure-form") else alternate
        source = min((run for run in editable_text_runs(reference, 0) if run["text"] == reference_text),
                     key=lambda run: run["origin"][1])
        move = case == "kerning"
        if move:
            move_text_run(reference, 0, source["index"], alternate, vertical_offset=0, horizontal_offset=.4)
        else:
            replace_text_run(reference, 0, source["index"], reference_text, revised)
        document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
        route = f"/documents/{document}/pages"
        runs = rust("GET", route + "/0/text-runs")["runs"]
        run = min((run for run in runs if run["text"] == alternate), key=lambda run: run["origin"][1])
        assert all(abs(a - b) < .1 for a, b in zip(run["origin"], source["origin"], strict=True))
        assert abs(run["bbox"][2] - source["bbox"][2]) < .1
        expected = 2 if case == "repeated-inner-form" else 1
        assert sum(item["text"] == alternate for item in runs) == expected
        args = {"expected_revision": 1, "page_index": 0, "span_index": run["index"], "expected_text": alternate}
        if case == "inline":
            rust("POST", route + "/replace-text-run",
                 {**args, "expected_text": painted, "replacement": revised}, status=422)
            assert pdf_bytes(rust, document, 1) == original
        if move:
            result = rust("POST", route + "/move-text-run", {**args, "horizontal_offset": .4, "vertical_offset": 0})
        else:
            result = rust("POST", route + "/replace-text-run", {**args, "replacement": revised})
        assert result["revision"] == 2
        final = pdf_bytes(rust, document, 2)
        caption = alternate if move else revised
        with pymupdf.open(stream=final) as output:
            assert output[0].get_text().count(caption) == 1
            assert output[0].get_text().count(alternate) == int(move or case == "repeated-inner-form")
            assert painted not in output[0].get_text() and sibling in output[0].get_text()
            assert all("MDTextGeometry" not in output.xref_object(i) for i in range(1, output.xref_length()))
            if case in ("structure", "ancestor", "structure-form"):
                assert all("/ActualText" not in output.xref_object(i) for i in range(1, output.xref_length()))
                assert b"/MCID" in output[0].read_contents()
        with pypdfium2.PdfDocument(final) as output, pypdfium2.PdfDocument(reference) as python:
            pixels = lambda document: document[0].render(scale=2).to_pil().convert("RGB")
            assert ImageChops.difference(pixels(output), pixels(python)).getbbox() is None, case
        assert pdf_bytes(rust, document, 1, "original") == original
        (root / f"actual-text-{case}-rust.pdf").write_bytes(final)

    # A replacement spanning distinct baselines cannot be removed as one run.
    # Its glyphs must still block edits to text drawn over that region.
    fixture = root / "actual-text-overlapping-scope.pdf"
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=520, height=570)
        for y in (90, 140):
            page.insert_text((70, y), "Underlying label", fontsize=15)
        first, second = page.get_contents()
        glyphs = b"\n".join(pdf.xref_stream(ref) for ref in (first, second))
        pdf.update_stream(first, b"/Span <</ActualText (Long annotation)>> BDC\n" + glyphs + b"\nEMC")
        pdf.update_stream(second, b"\n")
        page.insert_text((92, 140), "Overlapping line", fontsize=15)
        pdf.save(fixture)
    document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
    route = f"/documents/{document}/pages"
    run, = (run for run in rust("GET", route + "/0/text-runs")["runs"]
            if run["text"] == "Overlapping line")
    rust("POST", route + "/replace-text-run", {
        "expected_revision": 1, "page_index": 0, "span_index": run["index"],
        "expected_text": run["text"], "replacement": "Blocked",
    }, status=422)
    assert pdf_bytes(rust, document, 1) == fixture.read_bytes()


def short_actual_text_edit(rust, root):
    import pypdfium2
    from PIL import ImageChops
    from app.pdf_ops import editable_text_runs, move_text_run, replace_text_run

    matter = rust("POST", "/matters", {"name": "Synthetic short alternate text"}, status=201)["id"]
    variants = (
        ("prefix", "SHORT", "DRAWN FIRST", "SHOR", "NEW"),
        ("suffix", "SHORT", "DRAWN FIRST", "T", "X"),
        ("prefix-only", "SMALL", "DRAWN FIRST", "SMALL", "NEW"),
        ("next-line", "DRAWN FIRSTSMALL", "DRAWN FIRST", "SMALL", "NEW"),
        ("literal-suffix", "N FIRST", "DRAWN FIRST", "N FIRST", "NEW"),
        ("unplaced-middle", "DRIFT", "DRAWN FIRST", "DR", "UP"),
        ("matched-edges", "DRAST", "DRAWN FIRST", "DRA", "NEW"),
        ("one-character", "T", "DRAWN FIRST", "T", "X"),
        ("split-across-lines", "SHORT", "XY", "OR", "UP"),
        ("named-form", "SHORT", "DRAWN FIRST", "SHOR", "NEW"),
        ("rotated-crop", "SHORT", "DRAWN FIRST", "SHOR", "NEW"),
        ("move", "SHORT", "DRAWN FIRST", "T", None),
    )
    for case, alternate, painted, target, replacement in variants:
        source = root / f"short-actual-text-{case}.pdf"
        reference = root / f"short-actual-text-{case}-reference.pdf"
        in_form = case == "named-form"
        with pymupdf.open() as pdf, pymupdf.open() as inner:
            page = pdf.new_page(width=580, height=600)
            content = inner.new_page(width=500, height=180) if in_form else page
            owner = content.parent
            content.insert_text((55, 90), painted, fontsize=15)
            content.insert_text((55, 140), "DRAWN NEXT", fontsize=15)
            a, b = content.get_contents()
            encoded = "feff" + alternate.encode("utf-16-be").hex()
            mark = f"/Span <</ActualText <{encoded}>>> BDC\n".encode()
            if in_form:
                resources = int(owner.xref_get_key(content.xref, "Resources")[1].split()[0])
                prop = owner.get_new_xref()
                owner.update_object(prop, f"<< /ActualText <{encoded}> >>")
                owner.xref_set_key(resources, "Properties", f"<< /Alternate {prop} 0 R >>")
                mark = b"/Span /Alternate BDC\n"
            owner.update_stream(a, mark + owner.xref_stream(a) + b"\n" + owner.xref_stream(b) + b"\nEMC")
            owner.update_stream(b, b"\n")
            if in_form:
                page.show_pdf_page(pymupdf.Rect(30, 30, 530, 210), inner)
            page.insert_text((52, 205), "Neighbor unaffected", fontname="cour")
            if case == "rotated-crop":
                page.set_cropbox(pymupdf.Rect(18, 24, 538, 562))
                page.set_rotation(90)
            pdf.save(source)
        original = source.read_bytes()
        reference.write_bytes(original)
        document = rust("IMPORT", matter, source=str(source), status=201)[0]["id"]
        route = f"/documents/{document}/pages"

        def check_runs():
            python = sorted(editable_text_runs(reference, 0), key=lambda item: (item["text"], item["origin"]))
            native = sorted(rust("GET", route + "/0/text-runs")["runs"],
                            key=lambda item: (item["text"], item["origin"]))
            assert [run["text"] for run in python] == [run["text"] for run in native], (case, python, native)
            for a, b in zip(python, native, strict=True):
                assert all(abs(left - right) < .1 for left, right in zip(a["origin"], b["origin"], strict=True)), case
                assert abs(a["bbox"][0] - b["bbox"][0]) < .1, (case, a, b)
                # MuPDF does not name its redraw font in the named Form fixture.
                if a["font"]:
                    assert abs(a["bbox"][2] - b["bbox"][2]) < .1, (case, a, b)
            return python, native

        def edit(revision, expected, value):
            python, native = check_runs()
            a, = (item for item in python if item["text"] == expected)
            b, = (item for item in native if item["text"] == expected)
            values = {"expected_revision": revision, "page_index": 0, "span_index": b["index"],
                      "expected_text": expected}
            if value is None:
                move_text_run(reference, 0, a["index"], expected, vertical_offset=0, horizontal_offset=.4)
                values.update(horizontal_offset=.4, vertical_offset=0)
                endpoint = "/move-text-run"
            else:
                replace_text_run(reference, 0, a["index"], expected, value)
                values["replacement"] = value
                endpoint = "/replace-text-run"
            assert rust("POST", route + endpoint, values)["revision"] == revision + 1, case
            output = pdf_bytes(rust, document, revision + 1)
            with pypdfium2.PdfDocument(output) as actual, pypdfium2.PdfDocument(reference) as baseline:
                pixels = lambda pdf: pdf[0].render(scale=2).to_pil().convert("RGB")
                assert ImageChops.difference(pixels(actual), pixels(baseline)).getbbox() is None, case
            check_runs()
            return output

        revision_two = edit(1, target, replacement)
        if case == "prefix":
            revision_three = edit(2, "T", "X")
            retained = rust("SNAPSHOT", f"/documents/{document}",
                            {"kind": "retained", "revision": 2, "sha256": sha256(revision_two).hexdigest()})
            assert bytes(retained["bytes"]) == revision_two
            (root / "short-actual-text-prefix-rust.pdf").write_bytes(revision_three)
        assert pdf_bytes(rust, document, 1, "original") == original


def single_anchor_actual_text_edit(rust, root):
    import pypdfium2
    from PIL import ImageChops
    from app.pdf_ops import editable_text_runs, move_text_run, replace_text_run

    matter = rust("POST", "/matters", {"name": "Synthetic multiline structural anchors"}, status=201)["id"]
    upper, lower = "UPPER WORDS", "LOWER WORD"
    variants = (
        ("structure", upper + lower, upper, "REVISED", True),
        ("structure-lower", upper + lower, lower, None, False),
        ("structure-break", upper + "\n" + lower, upper, "REVISED", True),
        ("structure-short", "SHORT", "SHOR", "NEW", True),
        ("structure-overflow", upper + lower + " LAST", upper + lower + " LA", "REVISED", True),
        ("inline-short", "SHORT", "SHOR", "NEW", True),
        ("inline-suffix", "SHORT", "T", "X", False),
        ("form", upper + lower, "DRAWN FIRST", "REVISED", True),
    )
    for case, alternate, target, replacement, first in variants:
        fixture = root / f"single-anchor-{case}.pdf"
        reference = root / f"single-anchor-{case}-reference.pdf"
        in_form, inline = case == "form", case.startswith("inline-")
        with pymupdf.open() as pdf, pymupdf.open() as inner:
            page = pdf.new_page(width=580, height=620)
            content = inner.new_page(width=500, height=180) if in_form else page
            painter = content.parent
            content.insert_text((55, 90), "DRAWN FIRST", fontsize=15)
            content.insert_text((55, 140), "DRAWN NEXT", fontsize=15)
            if case == "structure-overflow":
                content.insert_text((55, 190), "DRAWN FINAL", fontsize=15)
            first_stream, *rest = content.get_contents()
            encoded = "feff" + alternate.encode("utf-16-be").hex()
            prop = f" /ActualText <{encoded}>" if inline else ""
            mark = f"/P <</MCID 0{prop}>> BDC\n".encode()
            drawing = b"\n".join(painter.xref_stream(ref) for ref in (first_stream, *rest))
            painter.update_stream(first_stream, mark + drawing + b"\nEMC")
            for ref in rest:
                painter.update_stream(ref, b"\n")
            if in_form:
                form = page.show_pdf_page(pymupdf.Rect(30, 30, 530, 210), inner)
                pdf.xref_set_key(form, "StructParents", "1")
                source_kid = f"<< /Type /MCR /Pg {page.xref} 0 R /Stm {form} 0 R /MCID 0 >>"
            else:
                pdf.xref_set_key(page.xref, "StructParents", "1")
                source_kid = "0"
            tree, parent, owner = (pdf.get_new_xref() for _ in range(3))
            pdf.xref_set_key(pdf.pdf_catalog(), "StructTreeRoot", f"{tree} 0 R")
            pdf.xref_set_key(pdf.pdf_catalog(), "MarkInfo", "<< /Marked true >>")
            pdf.update_object(tree, f"<< /Type /StructTreeRoot /K {owner} 0 R "
                                    f"/ParentTree {parent} 0 R /ParentTreeNextKey 2 >>")
            pdf.update_object(parent, f"<< /Nums [1 [{owner} 0 R]] >>")
            actual = "" if inline else f" /ActualText <{encoded}>"
            pdf.update_object(owner, f"<< /Type /StructElem /S /P /P {tree} 0 R "
                                     f"/Pg {page.xref} 0 R /K {source_kid}{actual} >>")
            page.insert_text((52, 245), "Unrelated line", fontname="cour")
            pdf.save(fixture)
        source = fixture.read_bytes()
        reference.write_bytes(source)
        document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
        route = f"/documents/{document}/pages"

        def runs():
            expected = sorted(editable_text_runs(reference, 0), key=lambda item: item["text"])
            actual = sorted(rust("GET", route + "/0/text-runs")["runs"], key=lambda item: item["text"])
            assert [item["text"] for item in actual] == [item["text"] for item in expected], (case, expected, actual)
            for a, b in zip(expected, actual, strict=True):
                assert all(abs(x - y) < .1 for x, y in zip(a["origin"], b["origin"], strict=True)), (case, a, b)
            return expected, actual

        def edit(revision, text, change):
            expected, actual = runs()
            baseline, = (item for item in expected if item["text"] == text)
            selected, = (item for item in actual if item["text"] == text)
            args = {"expected_revision": revision, "page_index": 0, "span_index": selected["index"],
                    "expected_text": text}
            if change is None:
                move_text_run(reference, 0, baseline["index"], text, vertical_offset=0, horizontal_offset=.4)
                route_name = "/move-text-run"
                args.update(vertical_offset=0, horizontal_offset=.4)
            else:
                replace_text_run(reference, 0, baseline["index"], text, change)
                route_name = "/replace-text-run"
                args["replacement"] = change
            assert rust("POST", route + route_name, args)["revision"] == revision + 1
            output = pdf_bytes(rust, document, revision + 1)
            with pypdfium2.PdfDocument(output) as native, pypdfium2.PdfDocument(reference) as python:
                pixels = lambda pdf: pdf[0].render(scale=2).to_pil().convert("RGB")
                assert ImageChops.difference(pixels(native), pixels(python)).getbbox() is None, case
            runs()
            return output

        revision_two = edit(1, target, replacement)
        with pymupdf.open(stream=revision_two) as pdf:
            page = pdf[0]
            get_ref = lambda xref, key: int(pdf.xref_get_key(xref, key)[1].split()[0])
            tree = get_ref(pdf.pdf_catalog(), "StructTreeRoot")
            owner = get_ref(tree, "K")
            parent = get_ref(tree, "ParentTree")
            kids = pdf.xref_get_key(owner, "K")[1]
            nums = pdf.xref_get_key(parent, "Nums")[1]
            page_key = int(pdf.xref_get_key(page.xref, "StructParents")[1])
            new = 0 if in_form else 1
            assert pdf.xref_get_key(owner, "ActualText") == ("null", "null")
            edited_content = re.search(rf"<<(?=[^>]*?/MCID\s*{new}\b)(?![^>]*?/Stm)[^>]*?>>", kids)
            assert edited_content and re.search(rf"/Pg\s*{page.xref} 0 R", edited_content[0]), (case, kids)
            assert re.search(rb"/MCID\s+" + str(new).encode() + rb"\b", page.read_contents())
            if in_form:
                surviving = re.search(r"<<(?=[^>]*?/MCID\s*0\b)[^>]*?/Stm\s*(\d+) 0 R[^>]*?>>", kids)
                assert surviving and surviving.start() > edited_content.start(), kids
                form = int(surviving[1])
                streams = pdf.xref_stream(form).lower()
                assert b"drawn next" in streams or b"DRAWN NEXT".hex().encode() in streams
                assert b"drawn first" not in streams and b"DRAWN FIRST".hex().encode() not in streams
                assert pdf.xref_get_key(form, "StructParents") == ("int", "1")
                for key in (1, page_key):
                    assert re.search(rf"\b{key}\s*\[\s*{owner} 0 R\s*\]", nums), nums
            else:
                old = re.search(r"0\s*\]$" if first else r"^\[\s*0\b", kids)
                assert old and re.search(rf"\b{page_key}\s*\[\s*{owner} 0 R\s+{owner} 0 R\s*\]", nums), (kids, nums)
                assert pdf[0].read_contents().count(b"/MCID") == 2
                assert (b"/ActualText" in page.read_contents()) == inline
        (root / f"single-anchor-{case}-rust.pdf").write_bytes(revision_two)
        if case == "inline-short":
            edit(2, "T", "X")
            retained = rust("SNAPSHOT", f"/documents/{document}",
                            {"kind": "retained", "revision": 2, "sha256": sha256(revision_two).hexdigest()})
            assert bytes(retained["bytes"]) == revision_two
        assert pdf_bytes(rust, document, 1, "original") == source


def middle_tag_reading_order(rust, root):
    import pypdfium2
    from PIL import ImageChops
    from app.pdf_ops import editable_text_runs, move_text_run, replace_text_run

    matter = rust("POST", "/matters", {"name": "Synthetic middle line reading order"}, status=201)["id"]
    painted = ("DRAWN FIRST", "DRAWN NEXT", "DRAWN FINAL")
    alternate = ("UPPER WORDS", "MIDDL WORD", "FINAL WORDS")
    for case in ("page", "named", "text-object", "inside-text", "form", "inline", "inline-named", "inline-form"):
        fixture = root / f"middle-tag-{case}.pdf"
        baseline = root / f"middle-tag-{case}-reference.pdf"
        inline = case.startswith("inline")
        in_form = case.endswith("form")
        named = case.endswith("named")
        original_mcid = 3 if named else 0
        source_slots = [None] * original_mcid + ["owner"]
        if named:
            source_slots.extend([None, None])
        with pymupdf.open() as pdf, pymupdf.open() as inner:
            page = pdf.new_page(width=580, height=620)
            content = inner.new_page(width=500, height=260) if in_form else page
            source = content.parent
            for line, text in enumerate(painted):
                content.insert_text((55, 90 + 50 * line), text, fontsize=15)
            first, *rest = content.get_contents()
            encoded = "feff" + "".join(alternate).encode("utf-16-be").hex()
            props = f"<< /MCID {original_mcid}" + (f" /ActualText <{encoded}>" if inline else "") + " >>"
            if named:
                resources = int(source.xref_get_key(content.xref, "Resources")[1].split()[0])
                prop = source.get_new_xref()
                source.update_object(prop, props)
                source.xref_set_key(resources, "Properties", f"<< /StartMark {prop} 0 R >>")
                props = "/StartMark"
            mark = f"/Span {props} BDC\n".encode()
            drawing = b"\n".join(source.xref_stream(stream) for stream in (first, *rest))
            if case in ("text-object", "inside-text"):
                shows = b"\n".join(f"1 0 0 1 55 {620 - (90 + 50 * i)} Tm ({line}) Tj".encode()
                                    for i, line in enumerate(painted))
                if case == "text-object":
                    drawing = mark + b"q\nBT\n/helv 15 Tf\n" + shows + b"\nET\nQ\nEMC\n"
                else:
                    drawing = b"q\nBT\n/helv 15 Tf\n" + mark + shows + b"\nEMC\nET\nQ\n"
            else:
                drawing = mark + drawing + b"\nEMC\n"
            source.update_stream(first, drawing)
            for stream in rest:
                source.update_stream(stream, b"\n")
            if in_form:
                form = page.show_pdf_page(pymupdf.Rect(30, 30, 530, 290), inner)
                pdf.xref_set_key(form, "StructParents", "4")
                child = f"<< /Type /MCR /Pg {page.xref} 0 R /Stm {form} 0 R /MCID {original_mcid} >>"
            else:
                pdf.xref_set_key(page.xref, "StructParents", "4")
                child = str(original_mcid)
            tree, parent, owner = (pdf.get_new_xref() for _ in range(3))
            pdf.xref_set_key(pdf.pdf_catalog(), "StructTreeRoot", f"{tree} 0 R")
            pdf.xref_set_key(pdf.pdf_catalog(), "MarkInfo", "<< /Marked true >>")
            pdf.update_object(tree, f"<< /Type /StructTreeRoot /K {owner} 0 R "
                                    f"/ParentTree {parent} 0 R /ParentTreeNextKey 5 >>")
            slot_values = " ".join(f"{owner} 0 R" if slot else "null" for slot in source_slots)
            pdf.update_object(parent, f"<< /Nums [4 [{slot_values}]] >>")
            actual = "" if inline else f" /ActualText <{encoded}>"
            pdf.update_object(owner, f"<< /Type /StructElem /S /P /P {tree} 0 R /Pg {page.xref} 0 R /K {child}{actual} >>")
            page.insert_text((52, 325), "Unrelated line", fontname="cour")
            pdf.save(fixture)

        original = fixture.read_bytes()
        baseline.write_bytes(original)
        doc = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
        route = f"/documents/{doc}/pages"
        expected_mid = painted[1] if in_form and not inline else alternate[1]

        def edit(revision, old, new):
            a, = (item for item in editable_text_runs(baseline, 0) if item["text"] == old)
            b, = (item for item in rust("GET", route + "/0/text-runs")["runs"] if item["text"] == old)
            assert all(abs(x - y) < .1 for x, y in zip(a["origin"], b["origin"], strict=True))
            args = {"expected_revision": revision, "page_index": 0, "span_index": b["index"], "expected_text": old}
            if new is None:
                move_text_run(baseline, 0, a["index"], old, vertical_offset=0, horizontal_offset=.4)
                args.update(horizontal_offset=.4, vertical_offset=0)
                endpoint = "/move-text-run"
            else:
                replace_text_run(baseline, 0, a["index"], old, new)
                args["replacement"] = new
                endpoint = "/replace-text-run"
            assert rust("POST", route + endpoint, args)["revision"] == revision + 1
            data = pdf_bytes(rust, doc, revision + 1)
            with pypdfium2.PdfDocument(baseline) as expected, pypdfium2.PdfDocument(data) as native:
                def pixels(document):
                    return document[0].render(scale=2).to_pil().convert("RGB")
                assert ImageChops.difference(pixels(expected), pixels(native)).getbbox() is None, (case, revision)
            return data

        def structure(data, revision):
            with pymupdf.open(stream=data) as pdf:
                page = pdf[0]
                get_ref = lambda xref, key: int(pdf.xref_get_key(xref, key)[1].split()[0])
                struct = get_ref(pdf.pdf_catalog(), "StructTreeRoot")
                owner = get_ref(struct, "K")
                parent = get_ref(struct, "ParentTree")
                kids = pdf.xref_get_key(owner, "K")[1]
                nums = pdf.xref_get_key(parent, "Nums")[1]
                page_key = int(pdf.xref_get_key(page.xref, "StructParents")[1])
                redraw = (0 if in_form else len(source_slots))
                suffix = len(source_slots) if in_form else redraw + 1
                old_redraw = redraw
                if revision == 3:
                    redraw = 1 if in_form else suffix + 1
                refs = re.findall(r"<<[^<>]*>>|\d+", kids)
                assert len(refs) == 3, (case, revision, kids)
                form_stream = None
                for index, (ref, mcid) in enumerate(zip(refs, (original_mcid, redraw, suffix), strict=True)):
                    if index == 0 and not in_form:
                        assert ref == str(mcid), kids
                        continue
                    assert re.search(rf"/MCID\s*{mcid}\b", ref) and re.search(rf"/Pg\s*{page.xref} 0 R", ref), kids
                    if in_form and index != 1:
                        form = int(re.search(r"/Stm\s*(\d+) 0 R", ref)[1])
                        assert form_stream in (None, form), kids
                        form_stream = form
                    else:
                        assert "/Stm" not in ref, kids
                entries = {int(key): re.findall(r"null|\d+ 0 R", slots)
                           for key, slots in re.findall(r"(\d+)\s*\[([^\[\]]*)\]", nums)}
                assert entries[page_key][redraw] == f"{owner} 0 R", nums
                assert entries[4][original_mcid] == entries[4][suffix] == f"{owner} 0 R", nums
                if revision == 3:
                    assert entries[page_key][old_redraw] == "null", nums
                painted_stream = pdf.xref_stream(form_stream) if in_form else page.read_contents()
                assert b"MDTextTagContinuation" not in painted_stream
                def below(mcid):
                    match = re.search(rb"/MCID\s+" + str(mcid).encode() + rb"\b[^>]*>>\s*BDC(.*?)EMC", painted_stream, re.S)
                    assert match, (case, mcid, painted_stream)
                    return match[1]
                latter = below(suffix)
                assert painted[-1].encode().lower() in latter.lower() or painted[-1].encode().hex().encode() in latter.lower()
                if not named or inline:
                    earlier = below(original_mcid)
                    assert painted[0].encode().lower() in earlier.lower() or painted[0].encode().hex().encode() in earlier.lower()

        first = edit(1, expected_mid, "REVISED MIDDLE")
        structure(first, 2)
        current = sorted(item["text"] for item in rust("GET", route + "/0/text-runs")["runs"])
        assert current == sorted(["REVISED MIDDLE", "Unrelated line", *(alternate[::2] if inline else painted[::2])]), (case, current)
        latest = edit(2, "REVISED MIDDLE", None if case == "inside-text" else "SECOND EDIT")
        structure(latest, 3)
        retained = rust("SNAPSHOT", f"/documents/{doc}",
                        {"kind": "retained", "revision": 2, "sha256": sha256(first).hexdigest()})
        assert bytes(retained["bytes"]) == first
        assert pdf_bytes(rust, doc, 1, "original") == original
        (root / f"middle-tag-{case}-rust.pdf").write_bytes(latest)


def scaled_text_run_placement(rust, matter, root):
    from app.pdf_ops import editable_text_runs, move_text_run, replace_text_run

    label = "Edit me"
    for rotation in (0, 90, 180, 270):
        for unit in (1, 2):
            for action in ("move", "replace"):
                fixture = root / f"scaled-run-{rotation}-{unit}-{action}.pdf"
                with pymupdf.open() as pdf:
                    page = pdf.new_page(width=320, height=240)
                    page.insert_text((35, 50), label, fontsize=9, color=(.2, .4, .6))
                    page.draw_rect((250, 170, 265, 190), color=(.1, .4, .1))
                    pdf.xref_set_key(page.xref, "CropBox", "[-20 -10 300 220]")
                    pdf.xref_set_key(page.xref, "UserUnit", str(unit))
                    page.set_rotation(rotation)
                    pdf.save(fixture)
                case = (rotation, unit, action)
                original = fixture.read_bytes()
                baseline = root / f"scaled-run-reference-{rotation}-{unit}-{action}.pdf"
                baseline.write_bytes(original)
                document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
                route = f"/documents/{document}/pages"
                expected, = editable_text_runs(fixture, 0)
                native, = rust("GET", route + "/0/text-runs")["runs"]
                assert native["text"] == expected["text"] == label, case
                assert abs(native["size"] - expected["size"]) < .001, case
                assert max(abs(a - b) for a, b in zip(native["origin"], expected["origin"], strict=True)) < .001, case
                assert max(abs(a - b) for a, b in zip(native["relative_bbox"], expected["relative_bbox"], strict=True)) < .015, case
                request = {"expected_revision": 1, "page_index": 0, "span_index": native["index"], "expected_text": label}
                if action == "move":
                    offsets = {"vertical_offset": .02, "horizontal_offset": .03}
                    move_text_run(baseline, 0, 0, label, **offsets)
                    result = rust("POST", route + "/move-text-run", dict(request, **offsets))
                else:
                    expected_layout = replace_text_run(baseline, 0, 0, label, "New run")
                    result = rust("POST", route + "/replace-text-run", dict(request, replacement="New run"))
                    assert result["edit_layout"] == expected_layout, case
                assert result["revision"] == 2, case
                with pymupdf.open(baseline) as expected_pdf, pymupdf.open(stream=pdf_bytes(rust, document, 2)) as native_pdf:
                    a, = expected_pdf[0].get_texttrace()
                    b, = native_pdf[0].get_texttrace()
                    for first, second in zip(a["chars"], b["chars"], strict=True):
                        assert first[0] == second[0], case
                        assert max(abs(x - y) for x, y in zip(first[2], second[2], strict=True)) < .001, case
                    assert abs(a["size"] - b["size"]) < .001, case
                    assert native_pdf[0].rotation == rotation and native_pdf[0].cropbox == expected_pdf[0].cropbox
                    assert native_pdf[0].get_drawings() == expected_pdf[0].get_drawings()
                assert pdf_bytes(rust, document, 1, "original") == original

    fixture = root / "scaled-run-reference-off-sheet.pdf"
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=320, height=240)
        page.insert_text((75, 110), label, fontsize=12)
        pdf.xref_set_key(page.xref, "CropBox", "[-20 -10 300 220]")
        pdf.xref_set_key(page.xref, "UserUnit", "2")
        pdf.save(fixture)
    original = fixture.read_bytes()
    baseline = root / "scaled-run-off-sheet-reference.pdf"
    baseline.write_bytes(original)
    offsets = {"vertical_offset": .2, "horizontal_offset": .05}
    move_text_run(baseline, 0, 0, label, **offsets)
    with pymupdf.open(baseline) as pdf:
        assert set(pdf[0].get_pixmap().samples) == {255}
    document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
    result = rust("POST", f"/documents/{document}/pages/move-text-run", {
        "expected_revision": 1, "page_index": 0, "span_index": 0, "expected_text": label, **offsets,
    })
    assert result["revision"] == 2
    with pymupdf.open(stream=pdf_bytes(rust, document, 2)) as pdf:
        assert pdf[0].get_text().strip() == label
        paint, = pdf[0].get_texttrace()
        assert paint["chars"][0][2] == (180.0, 268.0)
        assert paint["size"] == 24
    assert pdf_bytes(rust, document, 1, "original") == original


def single_text_run_placement(rust, matter, root):
    from app.pdf_ops import ProcessingError, editable_text_runs, move_text_run, replace_text_run

    for case, mode, rotation, refusal in (
        ("move-outside", "move", 0, "The text would leave the page; drag it inside the page"),
        ("move-outside", "move", 90, "The text would leave the page; drag it inside the page"),
        ("move-outside", "move", 180, "The text would leave the page; drag it inside the page"),
        ("move-outside", "move", 270, "The text would leave the page; drag it inside the page"),
        ("move-graphics", "move", 0, "The new position overlaps text or page graphics; choose a clear gap"),
        ("move-into-link", "move", 0, "Text inside or over a link cannot be moved with this tool"),
        ("move-no-distance", "move", 0, "Drag the text to a new position before applying the move"),
        ("move-pending-redaction", "move", 0, "This page has pending redaction marks. Review them before moving text."),
        ("replace-source-link", "replace", 0, "Selected text contains a link; edit it outside this tool"),
        ("replace-source-link", "replace", 90, None),
        ("replace-over-link", "replace", 0, None),
        ("replace-inactive-link", "replace", 0, None),
        ("replace-whitespace", "replace", 0, None),
    ):
        label = f"single-text-{case}-{rotation}"
        source = root / f"{label}.pdf"
        selected_text = "IIIIIIIIIIIIIIIIIII"
        uri = "https://example.invalid/reference"
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=440, height=380)
            page.insert_text((48, 94), selected_text, fontsize=17,
                             fontname="cour", color=(.05, .5, .8))
            page.set_cropbox(pymupdf.Rect(10, 11, 420, 360))
            if case == "move-graphics":
                page.draw_rect(pymupdf.Rect(135, 171, 275, 185), fill=(.3, .4, .5))
            if case == "move-into-link":
                page.insert_link({"kind": pymupdf.LINK_URI, "uri": uri,
                                  "from": pymupdf.Rect(100, 167, 270, 189)})
            if case == "move-pending-redaction":
                page.add_redact_annot(pymupdf.Rect(24, 290, 70, 310), fill=False)
            if case in {"replace-source-link", "replace-over-link"}:
                rectangle = (pymupdf.Rect(43, 57, 68, 90) if case == "replace-source-link"
                             else pymupdf.Rect(49, 91, 270, 110))
                page.insert_link({"kind": pymupdf.LINK_URI, "uri": uri, "from": rectangle})
            if case == "replace-inactive-link":
                link = pdf.get_new_xref()
                pdf.update_object(link, "<< /Subtype /Link /Rect [60 280 100 304] >>")
                pdf.xref_set_key(page.xref, "Annots", f"[{link} 0 R]")
            page.set_rotation(rotation)
            pdf.save(source)
        original = source.read_bytes()
        document = rust("IMPORT", matter, source=str(source), status=201)[0]["id"]
        route = f"/documents/{document}/pages"
        before = pdf_bytes(rust, document, 1)
        words = rust("GET", route + "/0/text-runs")["runs"]
        rust_run, = (item for item in words if item["text"] == selected_text)
        py_run, = (item for item in editable_text_runs(source, 0) if item["text"] == selected_text)
        expected_text = "stale selection" if case == "move-no-distance" else selected_text
        request = dict(expected_revision=1, page_index=0, span_index=rust_run["index"],
                       expected_text=expected_text)
        candidate = root / f"{label}-reference.pdf"
        candidate.write_bytes(original)
        try:
            if mode == "move":
                with pymupdf.open(source) as opened:
                    page = opened[0]
                    offset = (pymupdf.Point(225, 0) if case == "move-outside"
                              else pymupdf.Point(0, 0 if case == "move-no-distance" else 101))
                    display = offset * page.rotation_matrix - pymupdf.Point() * page.rotation_matrix
                    request["horizontal_offset"] = display.x / page.rect.width
                    request["vertical_offset"] = display.y / page.rect.height
                move_text_run(candidate, 0, py_run["index"], expected_text,
                              request["vertical_offset"], request["horizontal_offset"])
            else:
                replacement = ("  Revised      wording  " + "\n" * 105 if case == "replace-whitespace"
                               else "Revised\nwording" if case == "replace-over-link"
                               else "Revised wording")
                request["replacement"] = replacement
                layout = replace_text_run(candidate, 0, py_run["index"], expected_text, replacement)
        except ProcessingError as error:
            assert str(error) == refusal, (label, error)
        else:
            assert refusal is None, label
        actual = rust("POST", route + f"/{mode}-text-run", request, status=None)
        if refusal:
            assert actual == {"status": 422, "body": {"detail": refusal}}, (label, actual)
            assert pdf_bytes(rust, document, 1) == before
        else:
            assert actual["status"] == 200 and actual["body"]["revision"] == 2, (label, actual)
            assert actual["body"]["edit_layout"] == layout
            with pymupdf.open(candidate) as expected, pymupdf.open(stream=pdf_bytes(rust, document, 2)) as output:
                assert expected[0].get_text() == output[0].get_text()
                assert output[0].get_text("dict")["blocks"][0]["lines"][0]["spans"][0]["color"] == py_run["color"]
                assert [item["uri"] for item in expected[0].get_links()] == [
                    item["uri"] for item in output[0].get_links()
                ]
        assert pdf_bytes(rust, document, 1, "original") == original


def cross_page_actual_text_edit(rust, root):
    import pypdfium2
    from PIL import ImageChops
    from app.pdf_ops import editable_text_runs, replace_text_run

    matter = rust("POST", "/matters", {"name": "Synthetic cross-page alternate text"}, status=201)["id"]
    labels = ("DRAWN FIRST", "DRAWN NEXT")
    for case in ("direct", "ancestor", "reverse", "line-break", "short", "default-page", "forms"):
        fixture = root / f"cross-page-actual-text-{case}.pdf"
        reference = root / f"cross-page-actual-text-{case}-reference.pdf"
        alternate = {"reverse": "LOWER WORDUPPER WORDS", "line-break": "UPPER WORDS\nLOWER WORD",
                     "short": "SHORT"}.get(case, "UPPER WORDSLOWER WORD")
        in_forms = case == "forms"
        with pymupdf.open() as pdf, pymupdf.open() as inner:
            def paint_marked(content, text):
                content.insert_text((55, 90), text, fontsize=15)
                stream, = content.get_contents()
                painter = content.parent
                painter.update_stream(stream, b"/P <</MCID 0>> BDC\n" + painter.xref_stream(stream) + b"\nEMC")

            for text in labels:
                pdf.new_page(width=520, height=560)
                if in_forms:
                    paint_marked(inner.new_page(width=450, height=190), text)
            tree, parent, owner = (pdf.get_new_xref() for _ in range(3))
            owners = [pdf.get_new_xref() for _ in labels] if case == "ancestor" else [owner] * 2
            pdf.xref_set_key(pdf.pdf_catalog(), "StructTreeRoot", f"{tree} 0 R")
            pdf.xref_set_key(pdf.pdf_catalog(), "MarkInfo", "<< /Marked true >>")
            pdf.update_object(tree, f"<< /Type /StructTreeRoot /K {owner} 0 R "
                                    f"/ParentTree {parent} 0 R /ParentTreeNextKey 2 >>")
            pdf.update_object(parent, f"<< /Nums [0 [{owners[0]} 0 R] 1 [{owners[1]} 0 R]] >>")
            kids = []
            for index, (page, text) in enumerate(zip(pdf, labels, strict=True)):
                marker = ""
                if in_forms:
                    form = page.show_pdf_page(pymupdf.Rect(30, 30, 480, 220), inner, index)
                    pdf.xref_set_key(form, "StructParents", str(index))
                    marker = f" /Stm {form} 0 R"
                else:
                    paint_marked(page, text)
                    pdf.xref_set_key(page.xref, "StructParents", str(index))
                if case == "ancestor":
                    child = owners[index]
                    pdf.update_object(child, f"<< /Type /StructElem /S /P /P {owner} 0 R "
                                             f"/Pg {page.xref} 0 R /K 0 >>")
                    kids.append(f"{child} 0 R")
                else:
                    kids.append(f"<< /Type /MCR /Pg {page.xref} 0 R /MCID 0{marker} >>")
                page.insert_text((50, 240), "Unrelated page text", fontname="cour")
            if case == "reverse":
                kids.reverse()
            encoded = "feff" + alternate.encode("utf-16-be").hex()
            default_page = f" /Pg {pdf[1].xref} 0 R" if case == "default-page" else ""
            pdf.update_object(owner, f"<< /Type /StructElem /S /P /P {tree} 0 R "
                                     f"/K [{' '.join(kids)}] /ActualText <{encoded}>{default_page} >>")
            pdf.save(fixture)
        source = fixture.read_bytes()
        reference.write_bytes(source)
        doc = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
        route = f"/documents/{doc}/pages"

        def same_labels():
            for number in range(2):
                python = sorted((row["text"], row["origin"]) for row in editable_text_runs(reference, number))
                native = sorted((row["text"], row["origin"]) for row in rust("GET", f"{route}/{number}/text-runs")["runs"])
                assert [text for text, _ in native] == [text for text, _ in python], (case, number, python, native)
                for (_, actual), (_, expected) in zip(native, python, strict=True):
                    assert all(abs(a - b) < .1 for a, b in zip(actual, expected, strict=True)), case

        same_labels()
        for revision, number in enumerate((1, 0) if case == "reverse" else (0, 1), start=1):
            chosen = labels[number]
            expected, = (r for r in editable_text_runs(reference, number) if r["text"] == chosen)
            native, = (r for r in rust("GET", f"{route}/{number}/text-runs")["runs"] if r["text"] == chosen)
            change = "FIRST REVISED" if number == 0 else "SECOND REVISED"
            replace_text_run(reference, number, expected["index"], chosen, change)
            assert rust("POST", route + "/replace-text-run", {"expected_revision": revision, "page_index": number,
                        "span_index": native["index"], "expected_text": chosen, "replacement": change})["revision"] == revision + 1
            pdf_data = pdf_bytes(rust, doc, revision + 1)
            with pypdfium2.PdfDocument(pdf_data) as native, pypdfium2.PdfDocument(reference) as python:
                for a, b in zip(native, python, strict=True):
                    assert ImageChops.difference(a.render(scale=2).to_pil().convert("RGB"),
                                                 b.render(scale=2).to_pil().convert("RGB")).getbbox() is None, case
            same_labels()
            if revision == 1:
                revision_two = pdf_data
        with pymupdf.open(stream=pdf_data) as pdf:
            get_ref = lambda xref, key: int(pdf.xref_get_key(xref, key)[1].split()[0])
            tree = get_ref(pdf.pdf_catalog(), "StructTreeRoot")
            owner = get_ref(tree, "K")
            parent = get_ref(tree, "ParentTree")
            nums = pdf.xref_get_key(parent, "Nums")[1]
            assert pdf.xref_get_key(owner, "ActualText") == ("string", alternate)
            kids = pdf.xref_get_key(owner, "K")[1]
            owners = [owner] * 2
            if case == "ancestor":
                owners = [int(id) for id in re.findall(r"(\d+) 0 R", kids)]
                assert len(owners) == 2
                for i, child in enumerate(owners):
                    assert get_ref(child, "P") == owner
                    assert re.search(rf"^\[\s*<<(?=[^>]*?/Pg\s*{pdf[i].xref} 0 R)(?=[^>]*?/MCID\s*1\b)[^>]*>>\s*0\s*\]$",
                                     pdf.xref_get_key(child, "K")[1])
            else:
                refs = re.findall(r"<<[^<>]*>>", kids)
                assert len(refs) == 4, (case, refs)
                order = (1, 0) if case == "reverse" else (0, 1)
                for pair, i in enumerate(order):
                    new, old = refs[2 * pair:2 * pair + 2]
                    assert all(re.search(rf"/Pg\s*{pdf[i].xref} 0 R", ref) for ref in (new, old)), refs
                    assert ("/Stm" in old) == in_forms and "/Stm" not in new
                    for expected, ref in ((int(not in_forms), new), (0, old)):
                        assert re.search(rf"/MCID\s*{expected}\b", ref), refs
                    if in_forms:
                        form = int(re.search(r"/Stm\s*(\d+) 0 R", old)[1])
                        form_key = int(pdf.xref_get_key(form, "StructParents")[1])
                        assert re.search(rf"\b{form_key}\s*\[\s*{owner} 0 R\s*\]", nums), nums
            for page, tag_owner in zip(pdf, owners, strict=True):
                page_key = int(pdf.xref_get_key(page.xref, "StructParents")[1])
                slots = (rf"\s*{tag_owner} 0 R" * (1 if in_forms else 2))
                assert re.search(rf"\b{page_key}\s*\[{slots}\s*\]", nums), nums
        retained = rust("SNAPSHOT", f"/documents/{doc}",
                        {"kind": "retained", "revision": 2, "sha256": sha256(revision_two).hexdigest()})
        assert bytes(retained["bytes"]) == revision_two
        assert pdf_bytes(rust, doc, 1, "original") == source


def grouped_actual_text_edit(rust, root):
    import pypdfium2
    from PIL import ImageChops
    from app.pdf_ops import editable_text_runs, move_text_run, replace_text_run

    matter = rust("POST", "/matters", {"name": "Synthetic grouped alternate text"}, status=201)["id"]
    upper, lower, edited_upper = "UPPER WORDS", "LOWER WORD", "UPPER REPLACED"
    neighbor = "Neighbor unaffected"
    variants = ("inline", "line-break", "long-tail", "columns", "named-form",
                "structure-array", "structure-children", "structure-reverse", "structure-form")
    for case in variants:
        fixture = root / f"grouped-actual-text-{case}.pdf"
        structured = case.startswith("structure-")
        in_form = case in ("named-form", "structure-form")
        last = lower + " AND MORE" if case == "long-tail" else lower
        alternate = upper + ("\n" if case == "line-break" else "") + last
        if case == "structure-reverse":
            alternate = lower + upper
        encoded = (b"\xfe\xff" + alternate.encode("utf-16-be")).hex()
        latitudes = (90, 90 if case == "columns" else 140)
        positions = ((55, latitudes[0]), (275 if case == "columns" else 55, latitudes[1]))
        with pymupdf.open() as pdf, pymupdf.open() as leaf:
            page = pdf.new_page(width=520, height=570)
            content = leaf.new_page(width=450, height=220) if in_form else page
            owner_doc = content.parent
            for point, label in zip(positions, ("DRAWN FIRST", "DRAWN NEXT"), strict=True):
                content.insert_text(point, label, fontsize=15)
            first, second = content.get_contents()
            raw = [owner_doc.xref_stream(ref) for ref in (first, second)]
            if not structured:
                marker = f"/Span <</ActualText <{encoded}>>> BDC\n".encode()
                if case == "named-form":
                    resources = int(leaf.xref_get_key(content.xref, "Resources")[1].split()[0])
                    prop = leaf.get_new_xref()
                    leaf.update_object(prop, f"<< /ActualText <{encoded}> >>")
                    leaf.xref_set_key(resources, "Properties", f"<< /SourceAlt {prop} 0 R >>")
                    marker = b"/Span /SourceAlt BDC\n"
                owner_doc.update_stream(first, marker + b"\n".join(raw) + b"\nEMC")
                owner_doc.update_stream(second, b"\n")
            if in_form:
                form = page.show_pdf_page(pymupdf.Rect(30, 30, 480, 250), leaf)
            if structured:
                struct, tree, owner = (pdf.get_new_xref() for _ in range(3))
                pdf.xref_set_key(pdf.pdf_catalog(), "StructTreeRoot", f"{struct} 0 R")
                pdf.xref_set_key(pdf.pdf_catalog(), "MarkInfo", "<< /Marked true >>")
                pdf.update_object(struct, f"<< /Type /StructTreeRoot /K {owner} 0 R "
                                          f"/ParentTree {tree} 0 R /ParentTreeNextKey 2 >>")
                marked = [f"/P <</MCID {i}>> BDC\n".encode() + row + b"\nEMC"
                          for i, row in enumerate(raw)]
                if in_form:
                    pdf.xref_set_key(form, "StructParents", "1")
                    pdf.update_stream(form, b"\n".join(marked))
                    refs = " ".join(f"<< /Type /MCR /Pg {page.xref} 0 R /Stm {form} 0 R /MCID {i} >>"
                                    for i in range(2))
                    kids = f"[{refs}]"
                else:
                    pdf.xref_set_key(page.xref, "StructParents", "1")
                    for ref, data in zip((first, second), marked, strict=True):
                        pdf.update_stream(ref, data)
                    kids = "[1 0]" if case == "structure-reverse" else "[0 1]"
                children = [owner, owner]
                if case == "structure-children":
                    children = [pdf.get_new_xref(), pdf.get_new_xref()]
                    for i, ref in enumerate(children):
                        pdf.update_object(ref, f"<< /Type /StructElem /S /P /P {owner} 0 R "
                                               f"/Pg {page.xref} 0 R /K {i} >>")
                    kids = "[" + " ".join(f"{ref} 0 R" for ref in children) + "]"
                pdf.update_object(owner, f"<< /Type /StructElem /S /P /P {struct} 0 R "
                                         f"/Pg {page.xref} 0 R /K {kids} /ActualText <{encoded}> >>")
                values = " ".join(f"{ref} 0 R" for ref in children)
                pdf.update_object(tree, f"<< /Nums [1 [{values}]] >>")
            page.insert_text((52, 218), neighbor, fontname="cour")
            pdf.save(fixture)

        original = fixture.read_bytes()
        reference = root / f"grouped-actual-text-{case}-python.pdf"
        reference.write_bytes(original)
        py_upper = editable_text_runs(reference, 0)[0]
        moving = case == "columns"
        if moving:
            move_text_run(reference, 0, py_upper["index"], upper, vertical_offset=.16)
        else:
            replace_text_run(reference, 0, py_upper["index"], py_upper["text"], edited_upper)
        document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
        route = f"/documents/{document}/pages"
        runs = rust("GET", route + "/0/text-runs")["runs"]
        expected_lower = "\n" + last if case == "line-break" else last
        assert [item["text"] for item in runs] == [upper, expected_lower, neighbor], case
        old_lower = next(item for item in runs if item["text"] == expected_lower)
        args = {"expected_revision": 1, "page_index": 0, "span_index": runs[0]["index"],
                "expected_text": upper}
        result = (rust("POST", route + "/move-text-run", {**args, "vertical_offset": .16, "horizontal_offset": 0})
                  if moving else rust("POST", route + "/replace-text-run", {**args, "replacement": edited_upper}))
        assert result["revision"] == 2
        actual = pdf_bytes(rust, document, 2)
        caption = upper if moving else edited_upper
        before_edit = rust("GET", route + "/0/text-runs")["runs"]
        assert sorted(item["text"] for item in before_edit) == sorted([caption, last, neighbor]), case
        remaining = next(item for item in before_edit if item["text"] == last)
        assert remaining["bbox"] == old_lower["bbox"]
        with pypdfium2.PdfDocument(actual) as output, pypdfium2.PdfDocument(reference) as python:
            pixels = lambda doc: doc[0].render(scale=2).to_pil().convert("RGB")
            assert ImageChops.difference(pixels(output), pixels(python)).getbbox() is None, case
        (root / f"grouped-actual-text-{case}-rust.pdf").write_bytes(actual)

        # Re-edit the untouched run after its original property or logical
        # anchor has moved from the starting PDF objects.
        response = rust("POST", route + "/replace-text-run", {
            "expected_revision": 2, "page_index": 0, "span_index": remaining["index"],
            "expected_text": last, "replacement": "LOWER REPLACED",
        })
        assert response["revision"] == 3
        final = rust("GET", route + "/0/text-runs")["runs"]
        assert sorted(item["text"] for item in final) == sorted([caption, "LOWER REPLACED", neighbor])
        retained = rust("SNAPSHOT", f"/documents/{document}",
                        {"kind": "retained", "revision": 2, "sha256": sha256(actual).hexdigest()})
        assert bytes(retained["bytes"]) == actual
        assert pdf_bytes(rust, document, 1, "original") == original


def inherited_form_font_edit(rust, root):
    import pypdfium2
    from PIL import ImageChops
    from app.pdf_ops import editable_text_runs, replace_text_run

    matter = rust("POST", "/matters", {"name": "Synthetic inherited Form fonts"}, status=201)["id"]
    old, new, sibling = "INHERITED FORM TEXT", "REVISED INHERITED", "INHERITED SIBLING"
    for face, font_name in (("hebo", "Helvetica-Bold"), ("heit", "Helvetica-Oblique")):
        fixture = root / f"inherited-form-{face}.pdf"
        with pymupdf.open() as leaf, pymupdf.open() as pdf:
            inside = leaf.new_page(width=360, height=130)
            inside.insert_font(fontname="LocalFace", fontbuffer=pymupdf.Font(face).buffer)
            inside.insert_text((30, 58), old, fontname="LocalFace", fontsize=15)
            inside.insert_text((30, 104), sibling, fontname="LocalFace", fontsize=12)
            page = pdf.new_page(width=520, height=570)
            form = page.show_pdf_page(pymupdf.Rect(55, 40, 415, 170), leaf)
            wrapper = next(xref for xref, _, parent, _ in page.get_xobjects() if parent == 0)
            form_resources = int(pdf.xref_get_key(form, "Resources")[1].split()[0])
            fonts = pdf.xref_get_key(form_resources, "Font")[1]
            pdf.xref_set_key(form, "Resources", "null")
            pdf.xref_set_key(wrapper, "Resources", f"<< /Font {fonts} /XObject << /fullpage {form} 0 R >> >>")
            second = pdf.new_page(width=520, height=570)
            for key in ("Contents", "Resources"):
                pdf.xref_set_key(second.xref, key, pdf.xref_get_key(pdf[0].xref, key)[1])
            pdf.save(fixture)

        source = fixture.read_bytes()
        reference = root / f"inherited-form-{face}-python.pdf"
        reference.write_bytes(source)
        original_run = next(run for run in editable_text_runs(reference, 0) if run["text"] == old)
        replace_text_run(reference, 0, original_run["index"], old, new)
        document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
        path = f"/documents/{document}/pages"
        run = next(run for run in rust("GET", path + "/0/text-runs")["runs"] if run["text"] == old)
        edited = rust("POST", path + "/replace-text-run", {
            "expected_revision": 1, "page_index": 0, "span_index": run["index"],
            "expected_text": old, "replacement": new,
        })
        assert edited["revision"] == 2
        result = pdf_bytes(rust, document, 2)
        (root / f"inherited-form-{face}-rust.pdf").write_bytes(result)
        assert pdf_bytes(rust, document, 1, "original") == source
        with pymupdf.open(stream=result) as pdf, pymupdf.open(reference) as python:
            assert sorted(pdf[0].get_text().splitlines()) == sorted([new, sibling])
            assert pdf[1].get_text().splitlines() == [old, sibling]
            replacement = next(span for block in pdf[0].get_text("dict")["blocks"]
                               for line in block["lines"] for span in line["spans"] if span["text"] == new)
            assert replacement["font"] == font_name
            assert pdf[0].search_for(new) == python[0].search_for(new)
        with (pypdfium2.PdfDocument(result) as actual, pypdfium2.PdfDocument(source) as before,
              pypdfium2.PdfDocument(reference) as expected):
            def render(pdf, page):
                return pdf[page].render(scale=2).to_pil().convert("RGB")

            edited_image = render(actual, 0)
            prior_image = render(before, 0)
            comparison = render(expected, 0)
            # Compare the replacement itself with Python. Check preservation
            # outside the original line against the unedited source.
            region = (164, 157, 480, 210)
            assert ImageChops.difference(edited_image.crop(region), comparison.crop(region)).getbbox() is None
            changes = ImageChops.difference(edited_image, prior_image).getbbox()
            assert changes and 157 <= changes[1] < changes[3] <= 210 and 163 <= changes[0] < changes[2] < 522
            assert ImageChops.difference(render(actual, 1), render(before, 1)).getbbox() is None


def tiling_pattern_text_is_artwork(rust, root):
    import pypdfium2
    from PIL import ImageChops
    from app.pdf_ops import editable_text_runs, replace_text_run

    fixture = root / "tiling-pattern-text.pdf"
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=520, height=570)
        page.insert_text((70, 430), "DIRECT TEXT")
        font = page.get_fonts()[0][0]
        pattern = pdf.get_new_xref()
        pdf.update_object(pattern, "<< /Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 "
                                    "/BBox [0 0 210 70] /XStep 210 /YStep 70 "
                                    f"/Resources << /Font << /F {font} 0 R >> >> >>")
        pdf.update_stream(pattern, b"0.1 0.2 0.7 rg BT /F 14 Tf 1 0 0 1 18 27 Tm (TILE PATTERN TEXT) Tj ET")
        resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
        pdf.xref_set_key(resources, "Pattern", f"<< /TiledText {pattern} 0 R >>")
        stream, = page.get_contents()
        pdf.update_stream(stream, pdf.xref_stream(stream) + b"\nq /Pattern cs /TiledText scn 0 330 490 140 re f Q\n")
        pdf.save(fixture)
    original = fixture.read_bytes()
    reference = root / "tiling-pattern-text-python.pdf"
    reference.write_bytes(original)
    py_run, = editable_text_runs(reference, 0)
    assert py_run["text"] == "DIRECT TEXT"
    replace_text_run(reference, 0, py_run["index"], "DIRECT TEXT", "REVISED DIRECT")
    matter = rust("POST", "/matters", {"name": "Synthetic tiling-pattern text"}, status=201)["id"]
    document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
    route = f"/documents/{document}/pages"
    run, = rust("GET", route + "/0/text-runs")["runs"]
    assert run["text"] == "DIRECT TEXT"
    edited = rust("POST", route + "/replace-text-run", {
        "expected_revision": 1, "page_index": 0, "span_index": run["index"],
        "expected_text": run["text"], "replacement": "REVISED DIRECT",
    })
    assert edited["revision"] == 2 and pdf_bytes(rust, document, 1, "original") == original

    def pixels(data):
        with pypdfium2.PdfDocument(data) as pdf:
            return pdf[0].render(scale=2).to_pil().convert("RGB")

    before = pixels(original)
    after = pixels(pdf_bytes(rust, document, 2))
    area = before.crop((5, 205, 960, 460))
    assert ImageChops.difference(area, Image.new("RGB", area.size, "white")).getbbox() is not None
    assert ImageChops.difference(after, pixels(reference)).getbbox() is None
    change = ImageChops.difference(before, after).getbbox()
    assert change and change[1] > 750


def ocr(rust, matter):
    for name, language in [
        ("04_scanned_annexure.jpg", "eng"),
        ("05_hindi_scan.pdf", "hin"),
        ("06_hindi_mixed.pdf", "eng+hin"),
    ]:
        source = ROOT / "samples" / name
        document = rust("IMPORT", matter, source=str(source), status=201)[0]["id"]
        before = pdf_bytes(rust, document, 1)
        submitted = rust("POST", f"/documents/{document}/ocr",
                         {"language": language, "expected_revision": 1}, status=202)
        rust("RUN_PENDING", "")
        completed = rust("GET", f"/jobs/{submitted['id']}")
        assert completed["status"] == "completed", completed
        assert completed["result"]["pages_ocr_applied"] > 0, completed
        revision = completed["result"]["provenance"]["applied_revision"]
        after = pdf_bytes(rust, document, revision)
        with pymupdf.open(stream=before) as source_pdf, pymupdf.open(stream=after) as result_pdf:
            assert len(result_pdf) == len(source_pdf)
            for source_page, result_page in zip(source_pdf, result_pdf):
                assert result_page.rotation == source_page.rotation
                assert result_page.rect == source_page.rect
                assert result_page.get_pixmap().samples == source_page.get_pixmap().samples
            recognized = "".join(result_pdf[0].get_text().split())
            if language == "eng":
                assert "OCRTESTDOCUMENT" in recognized.upper(), recognized
            else:
                assert "हिंदीपरीक्षणदस्तावेज" in recognized, recognized
                if language == "eng+hin":
                    assert "CASE2026" in recognized and recognized.count("NativeHeader2026") == 1, recognized
        assert pdf_bytes(rust, document, 1, "original") == source.read_bytes()


def ocr_mixed_native_overlap(rust, root):
    auth = {"Authorization": "Bearer synthetic"}
    rotations = (0, 90, 180, 270)
    source = root / "mixed-overlap.pdf"
    with pymupdf.open() as image:
        page = image.new_page(width=520, height=230)
        page.insert_text((26, 90), "TRANSCRIPT VERIFIED FOLLOWUP", fontsize=25)
        scan = page.get_pixmap(dpi=180).tobytes("png")
    with pymupdf.open() as pdf:
        for rotation in rotations:
            page = pdf.new_page(width=620, height=420)
            page.insert_image(pymupdf.Rect(35, 50, 555, 280), stream=scan)
            page.insert_text((61, 140), "TRANS", fontsize=25, render_mode=3)
            page.insert_text((60, 320), "Native header", fontsize=22)
            page.set_cropbox(pymupdf.Rect(17, 23, 605, 397))
            page.set_rotation(rotation)
        pdf.new_page().insert_text((65, 90), "Unaffected native page")
        pdf.save(source)
    original = source.read_bytes()

    with TestClient(create_app(root / "ocr-overlap-reference", token="synthetic", require_vault=False)) as python:
        def reference(method, path, body=None, *, status=200, **kwargs):
            headers = {**auth, **kwargs.pop("headers", {})}
            response = python.request(method, path, json=body, headers=headers, **kwargs)
            assert response.status_code == status, (method, path, response.text)
            return response

        def dispatch(method, path, body=None, *, status=200):
            return reference(method, path, body, status=status).json()

        matter = {"name": "Synthetic OCR native overlap"}
        rust_matter = rust("POST", "/matters", matter, status=201)["id"]
        python_matter = dispatch("POST", "/matters", matter, status=201)["id"]
        native = rust("IMPORT", rust_matter, source=str(source), included=True, status=201)[0]["id"]
        baseline = reference("POST", f"/matters/{python_matter}/documents/raw?included=true",
                             status=201, content=original, headers={"Content-Type": "application/octet-stream",
                             "X-CourtBundle-Filename": source.name}).json()[0]["id"]
        paths = (f"/documents/{native}", f"/documents/{baseline}")

        def completed_jobs(first, second):
            rust("RUN_PENDING", "")
            actual = rust("GET", f"/jobs/{first}")
            deadline = monotonic() + 45
            while True:
                expected = dispatch("GET", f"/jobs/{second}")
                if expected["status"] not in {"queued", "running"} or monotonic() >= deadline:
                    break
                sleep(.05)
            assert actual["status"] == expected["status"] == "completed", (actual, expected)
            if actual["kind"] in {"ocr", "ocr-rerun"}:
                def page_events(job):
                    return [(event["progress"], event["message"]) for event in job["events"]
                            if event["message"].startswith("OCR page ")]
                assert page_events(actual) == page_events(expected) and page_events(actual)
            return actual["result"], expected["result"]

        def complete_ocr(route, body):
            first = rust("POST", paths[0] + route, body, status=202)["id"]
            second = dispatch("POST", paths[1] + route, body, status=202)["id"]
            return completed_jobs(first, second)

        first_run = complete_ocr("/ocr", {"language": "eng", "expected_revision": 1})
        for result in first_run:
            assert result["pages_checked"] == 5 and result["pages_ocr_applied"] == 4
            assert result["requested_page_index"] is None
            assert result["provenance"]["base_revision"] == 1
            assert result["provenance"]["applied_revision"] == 2

        def compare_outputs(revision):
            actual = pdf_bytes(rust, native, revision)
            expected = reference("GET", paths[1] + "/pdf", params={"revision": revision}).content
            with (pymupdf.open(stream=actual) as native_pdf, pymupdf.open(stream=expected) as baseline_pdf,
                  pymupdf.open(stream=original) as input_pdf):
                for index, (left, right, before) in enumerate(zip(native_pdf, baseline_pdf, input_pdf, strict=True)):
                    assert left.rotation == right.rotation == before.rotation
                    assert left.cropbox == right.cropbox == before.cropbox
                    assert left.get_pixmap().samples == right.get_pixmap().samples == before.get_pixmap().samples
                    assert left.get_text() == right.get_text()
                    if index < len(rotations):
                        assert left.get_text().split() == ["TRANS", "Native", "header", "RIPT", "VERIFIED", "FOLLOWUP"]
                        left_rect, = left.search_for("RIPT")
                        right_rect, = right.search_for("RIPT")
                        assert all(abs(a - b) < 3.0 for a, b in zip(left_rect, right_rect, strict=True))
                    else:
                        assert left.get_text() == before.get_text()

        compare_outputs(2)
        for backend, path, matter_id in zip((rust, dispatch), paths, (rust_matter, python_matter), strict=True):
            matches = backend("GET", f"/matters/{matter_id}/search?q=RIPT")["matches"]
            assert sorted(match["page"] for match in matches) == [1, 2, 3, 4]
            accepted = backend("POST", path + "/ocr-review/accept", {"expected_revision": 2, "page_index": 1})
            assert accepted["accepted_at"]
            assert backend("GET", path + "/ocr-review?page_index=1&revision=2")["can_rerun"]

        rerun = complete_ocr("/ocr-rerun", {"language": "eng", "expected_revision": 2, "page_index": 1})
        for result in rerun:
            assert result["page_index"] == 1 and result["provenance"]["rerun"]
            assert result["provenance"]["base_revision"] == 1
            assert result["provenance"]["applied_revision"] == 3
        compare_outputs(3)
        for backend, path in zip((rust, dispatch), paths, strict=True):
            backend("GET", path + "/ocr-review?page_index=1&revision=2", status=409)
            review = backend("GET", path + "/ocr-review?page_index=1&revision=3")
            assert review["accepted_at"] is None and review["can_rerun"]

        working = (pdf_bytes(rust, native, 3), reference("GET", paths[1] + "/pdf").content)
        screenings, digests = [], []
        for backend, matter_id in zip((rust, dispatch), (rust_matter, python_matter), strict=True):
            path = f"/matters/{matter_id}"
            checks = backend("GET", path + "/validate")
            digest = next(check["effective_input_digest"] for check in checks if check.get("effective_input_digest"))
            digests.append(digest)
            screenings.append(backend("POST", path + "/preflight/screen", {"expected_snapshot": digest}, status=202)["id"])
        for result in completed_jobs(*screenings):
            assert result["pages_screened"] == 5 and result["blank_candidates"] == 0
        for backend, matter_id, document_path, digest in zip((rust, dispatch), (rust_matter, python_matter), paths, digests, strict=True):
            checks = backend("GET", f"/matters/{matter_id}/validate")
            completed = next(check for check in checks if check["code"] == "PAGE_SCREENING_COMPLETED")
            assert re.search(r"Screened 5 source pages; OCR measurements available for [1-5]\.", completed["message"])
            for check in checks:
                if check["code"] == "LOW_OCR_CONFIDENCE":
                    assert check["target_revision"] == 3
                    assert re.search(r"\d+\.\d% mean word confidence with \d+ flagged regions", check["message"])
            assert backend("PATCH", document_path, {"title": "Updated synthetic screening input"})["revision"] == 3
            invalidated = backend("GET", f"/matters/{matter_id}/validate")
            assert any(check["code"] == "PAGE_SCREENING_NOT_RUN" for check in invalidated)
            assert not any(check["code"] == "LOW_OCR_CONFIDENCE" for check in invalidated)
            backend("POST", f"/matters/{matter_id}/preflight/screen", {"expected_snapshot": digest}, status=409)
        assert (pdf_bytes(rust, native, 3), reference("GET", paths[1] + "/pdf").content) == working
        assert pdf_bytes(rust, native, 1, "original") == original
        assert reference("GET", paths[1] + "/original").content == original


def ocr_physical_page_size(rust, matter, root):
    from app.pdf_ops import ocr_pdf, ProcessingError

    with pymupdf.open() as text:
        page = text.new_page(width=350, height=150)
        page.insert_text((22, 80), "RECOGNIZE THIS PAGE", fontsize=29)
        scan = page.get_pixmap(dpi=180).tobytes("png")
    fixture = root / "ocr-physical-page-size.pdf"
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=420, height=300)
        page.insert_image(pymupdf.Rect(40, 70, 390, 220), stream=scan)
        pdf.xref_set_key(page.xref, "CropBox", "[-20 -10 410 270]")
        pdf.xref_set_key(page.xref, "UserUnit", "2")
        page.set_rotation(90)
        large = pdf.new_page(width=1_000, height=1_000)
        large.insert_image(pymupdf.Rect(40, 70, 390, 220), stream=scan)
        pdf.xref_set_key(large.xref, "UserUnit", "2")
        pdf.save(fixture)
    original = fixture.read_bytes()
    document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
    route = f"/documents/{document}"

    def selected(index, revision):
        queued = rust("POST", route + "/ocr", {
            "language": "eng", "expected_revision": revision, "page_index": index,
        }, status=202)
        rust("RUN_PENDING", "")
        return rust("GET", f"/jobs/{queued['id']}")

    reference = root / "ocr-physical-reference.pdf"
    reference.write_bytes(original)
    assert ocr_pdf(reference, language="eng", page_index=0) == (2, 1)
    processed = selected(0, 1)
    assert processed["status"] == "completed" and processed["result"]["pages_ocr_applied"] == 1
    output = pdf_bytes(rust, document, 2)
    with pymupdf.open(stream=original) as imported, pymupdf.open(reference) as baseline, \
            pymupdf.open(stream=output) as actual:
        phrase = "RECOGNIZE THIS PAGE"
        assert phrase in " ".join(actual[0].get_text().split())
        assert phrase in " ".join(baseline[0].get_text().split())
        a, b = actual[0].search_for(phrase)[0], baseline[0].search_for(phrase)[0]
        assert a.intersects(b)
        for before, after in zip(imported, actual, strict=True):
            assert before.get_pixmap().samples == after.get_pixmap().samples
    limit = "OCR page exceeds the 40-megapixel processing limit; crop or resize a working copy first"
    try:
        ocr_pdf(reference, language="eng", page_index=1)
    except ProcessingError as exc:
        assert str(exc) == limit
    else:
        raise AssertionError("reference accepted a page above the physical raster limit")
    failed = selected(1, 2)
    assert failed["status"] == "failed" and failed["error"] == limit, failed
    assert pdf_bytes(rust, document, 2) == output
    assert pdf_bytes(rust, document, 1, "original") == original


def ocr_screening_sources(rust, root):
    from app import page_analysis

    name = "Synthetic large screening matter"
    native_matter = rust("POST", "/matters", {"name": name}, status=201)["id"]
    auth = {"Authorization": "Bearer synthetic-screening"}
    with TestClient(create_app(root / "screening-reference", token="synthetic-screening", require_vault=False)) as reference:
        py_matter = reference.post("/matters", headers=auth, json={"name": name}).json()["id"]
        files = []
        for kind in ("inline", "unused-image", "nested-image"):
            fixture = root / f"screening-{kind}.pdf"
            with pymupdf.open() as pdf:
                page = pdf.new_page(width=320, height=220)
                page.insert_text((30, 75), "SYNTHETIC SCREENING REVIEW", fontsize=14)
                if kind == "inline":
                    contents = page.get_contents()[0]
                    pdf.update_stream(contents, pdf.xref_stream(contents)
                                      + b"\nq 12 0 0 12 20 20 cm BI /W 1 /H 1 /BPC 8 /CS /G ID\n\x00\nEI Q\n")
                else:
                    raster = BytesIO()
                    Image.new("RGB", (4, 4), "black").save(raster, format="PNG")
                    image = page.insert_image(pymupdf.Rect(20, 170, 60, 210), stream=raster.getvalue())
                    pdf.update_stream(page.get_contents()[-1], b"\n")
                    if kind == "nested-image":
                        form = pdf.get_new_xref()
                        pdf.update_object(form, f"<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] "
                                          f"/Resources << /XObject << /Scan {image} 0 R >> >> >>")
                        pdf.update_stream(form, b"\n")
                        resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])
                        pdf.xref_set_key(resources, "XObject", f"<< /UnusedForm {form} 0 R >>")
                metadata = pdf.get_new_xref()
                pdf.update_object(metadata, "<< /Type /Metadata /Subtype /XML >>")
                pdf.update_stream(metadata, b"<synthetic>" + b" " * (55 * 1024 * 1024) + b"</synthetic>", compress=False)
                pdf.xref_set_key(pdf.pdf_catalog(), "Metadata", f"{metadata} 0 R")
                pdf.save(fixture)
            files.append(fixture)

        assert sum(path.stat().st_size for path in files) > 160 * 1024 * 1024
        documents = []
        for fixture in files:
            data = fixture.read_bytes()
            native = rust("IMPORT", native_matter, source=str(fixture), included=True, status=201)[0]
            imported = reference.post(f"/matters/{py_matter}/documents/raw?included=true", content=data,
                                      headers={**auth, "Content-Type": "application/octet-stream",
                                               "X-CourtBundle-Filename": fixture.name})
            assert imported.status_code == 201, imported.text
            python_doc = imported.json()[0]
            assert native["page_count"] == python_doc["page_count"] == 1
            documents.append((native["id"], python_doc["id"], sha256(data).hexdigest()))

        native_path, py_path = f"/matters/{native_matter}", f"/matters/{py_matter}"
        expected_checks = reference.get(py_path + "/validate", headers=auth).json()
        actual_checks = rust("GET", native_path + "/validate")
        digest = lambda rows: next(row["effective_input_digest"] for row in rows if row.get("effective_input_digest"))
        expected = reference.post(py_path + "/preflight/screen", headers=auth,
                                  json={"expected_snapshot": digest(expected_checks)})
        assert expected.status_code == 202, expected.text
        native_job = rust("POST", native_path + "/preflight/screen", {"expected_snapshot": digest(actual_checks)}, status=202)
        rust("RUN_PENDING", "")
        result = rust("GET", f"/jobs/{native_job['id']}")
        assert result["status"] == "completed", result
        assert result["result"]["pages_screened"] == 3 and result["result"]["blank_candidates"] == 0
        deadline = monotonic() + 120
        while monotonic() < deadline:
            py_job = reference.get(f"/jobs/{expected.json()['id']}", headers=auth).json()
            if py_job["status"] not in ("queued", "running"):
                break
            sleep(.03)
        assert py_job["status"] == "completed", py_job
        report = page_analysis.current_report(reference.app.state.store, py_matter)
        rows_by_document = {row["document_id"]: row for row in report["pages"]}
        assert [rows_by_document[identifier]["has_images"] for _, identifier, _ in documents] == [False, True, True]
        assert rows_by_document[documents[0][1]]["ocr_outcome"] == "not_applicable_native_text"
        for checks in (rust("GET", native_path + "/validate"), reference.get(py_path + "/validate", headers=auth).json()):
            completed, = (row for row in checks if row["code"] == "PAGE_SCREENING_COMPLETED")
            assert "Screened 3 source pages; OCR measurements available for 2." in completed["message"]
        for native, python_id, expected_hash in documents:
            assert rust("GET", f"/documents/{native}/original-info")["sha256"] == expected_hash
            assert reference.get(f"/documents/{python_id}/original-info", headers=auth).json()["sha256"] == expected_hash
            assert [row["revision"] for row in rust("GET", f"/documents/{native}/revisions")] == [1]


def ocr_preserves_catalog_and_rerun(rust, root, scenario):
    from app.pdf_ops import ProcessingError, restore_page_for_ocr

    protected_navigation = "This PDF has bookmarks, attachments or forms; page OCR rerun needs manual review"
    protected_link = "This PDF has links; page OCR rerun needs manual review"
    protected_annotation = "This page has annotations; page OCR rerun needs manual review"
    expected_error = {
        "navigation": protected_navigation,
        "form": protected_navigation,
        "attachment": protected_navigation,
        "annotation": protected_annotation,
        "link-before-annotation": protected_link,
    }.get(scenario)
    matter = rust("POST", "/matters", {"name": "Synthetic OCR graph"}, status=201)["id"]
    source = ROOT / "samples/04_scanned_annexure.jpg"
    file = root / f"ocr-catalog-{scenario}.pdf"
    with pymupdf.open() as pdf:
        page = pdf.new_page()
        page.insert_image(pymupdf.Rect(45, 50, 550, 750), filename=source)
        target = pdf.new_page()
        target.insert_text((65, 150), "Catalog destination")
        catalog = pdf.pdf_catalog()
        dest = f"[{target.xref} 0 R /XYZ 65 692 1.5]"
        pdf.xref_set_key(catalog, "Dests", f"<< /Section {dest} >>")
        pdf.xref_set_key(catalog, "OpenAction", dest)
        pdf.set_metadata({"title": "Synthetic navigation test"})
        if scenario in {"navigation", "attachment"}:
            pdf.embfile_add("note.txt", b"synthetic attachment")
        if scenario == "navigation":
            pdf.set_toc([[1, "Destination", 2]])
            link = pdf.get_new_xref()
            pdf.update_object(link, "<< /Type /Annot /Subtype /Link /Rect [30 25 70 45] /Dest /Section >>")
            pdf.xref_set_key(pdf[0].xref, "Annots", f"[{link} 0 R]")
        if scenario == "form":
            field = pymupdf.Widget()
            field.field_name = "Synthetic field"
            field.field_type = pymupdf.PDF_WIDGET_TYPE_TEXT
            field.rect = pymupdf.Rect(25, 25, 100, 45)
            pdf[1].add_widget(field)
        if scenario in {"annotation", "link-before-annotation"}:
            pdf[0].add_text_annot((24, 28), "Retain this synthetic note")
        if scenario == "link-before-annotation":
            link = pdf.get_new_xref()
            pdf.update_object(
                link, "<< /Subtype /Link /Rect [30 25 70 45] /AA << /U << /S /Named /N /FirstPage >> >> >>",
            )
            pdf.xref_set_key(pdf[1].xref, "Annots", f"[{link} 0 R]")
        if scenario == "empty-containers":
            pdf.xref_set_key(catalog, "Outlines", "<< /Type /Outlines /Count 0 >>")
            pdf.xref_set_key(catalog, "Names", "<< /EmbeddedFiles << /Names [] >> >>")
            pdf.xref_set_key(catalog, "AcroForm", "<< /Fields [] >>")
            pdf.xref_set_key(catalog, "AF", "[<< /Type /Filespec /F (source.txt) >>]")
            entries = []
            for content in (
                "<< /Subtype /Link /Rect [30 25 70 45] >>",
                "<< /Subtype /Popup /Rect [42 25 70 45] >>",
            ):
                entry = pdf.get_new_xref()
                pdf.update_object(entry, content)
                entries.append(f"{entry} 0 R")
            pdf.xref_set_key(pdf[0].xref, "Annots", f"[{' '.join(entries)}]")
        pdf.save(file)

    probe = root / f"reference-ocr-rerun-{scenario}.pdf"
    copyfile(file, probe)
    try:
        restore_page_for_ocr(probe, file, 0)
    except ProcessingError as error:
        assert str(error) == expected_error, (scenario, error)
    else:
        assert expected_error is None, scenario

    document = rust("IMPORT", matter, source=str(file), status=201)[0]["id"]
    route = f"/documents/{document}"
    original = pdf_bytes(rust, document, 1)

    def complete_ocr(operation, revision):
        job = rust("POST", route + operation, {"expected_revision": revision, "page_index": 0}, status=202)
        rust("RUN_PENDING", "")
        return rust("GET", f"/jobs/{job['id']}")

    def inspect_catalog(revision):
        with pymupdf.open(stream=original) as before, pymupdf.open(stream=pdf_bytes(rust, document, revision)) as after:
            assert len(after) == len(before) == 2
            assert after.metadata["title"] == before.metadata["title"]
            assert after.get_toc() == before.get_toc()
            assert after.embfile_names() == before.embfile_names()
            if scenario in {"navigation", "attachment"}:
                assert after.embfile_get("note.txt") == b"synthetic attachment"
            if scenario == "navigation":
                assert re.search(r"/Dest\s*/Section", after.xref_object(after[0].annot_xrefs()[0][0]))
            if scenario == "empty-containers":
                for name in ("Outlines", "Names", "AcroForm", "AF"):
                    assert after.xref_get_key(after.pdf_catalog(), name)[0] != "null"
                assert [row[1] for row in after[0].annot_xrefs()] == [
                    row[1] for row in before[0].annot_xrefs()
                ]
            opened = after.xref_get_key(after.pdf_catalog(), "OpenAction")
            assert opened[0] == "array" and re.match(rf"\[\s*{after[1].xref} 0 R\s*/XYZ", opened[1])
            dest = after.xref_get_key(after.pdf_catalog(), "Dests/Section")
            assert dest == opened
            assert after[0].get_pixmap().samples == before[0].get_pixmap().samples
            assert "OCRTESTDOCUMENT" in "".join(after[0].get_text().upper().split())
            assert after[1].get_text() == before[1].get_text()

    first = complete_ocr("/ocr", 1)
    assert first["status"] == "completed", first
    inspect_catalog(2)
    reviewed = rust("POST", route + "/ocr-review/accept", {"expected_revision": 2, "page_index": 0})
    assert set(reviewed) == {"document_id", "revision", "page_index", "accepted_at"}
    review = rust("GET", route + "/ocr-review?page_index=0&revision=2")
    assert review["available"] and review["accepted_at"] == reviewed["accepted_at"]
    first_bytes = pdf_bytes(rust, document, 2)
    rerun = complete_ocr("/ocr-rerun", 2)
    if expected_error:
        assert rerun["status"] == "failed" and rerun["error"] == expected_error, rerun
        assert len(rust("GET", route + "/revisions")) == 2
        assert pdf_bytes(rust, document, 2) == first_bytes
        assert rust("GET", route + "/ocr-review?page_index=0&revision=2")["accepted_at"] == reviewed["accepted_at"]
    else:
        assert rerun["status"] == "completed", rerun
        inspect_catalog(3)
        with pymupdf.open(stream=pdf_bytes(rust, document, 3)) as after:
            assert "".join(after[0].get_text().upper().split()).count("OCRTESTDOCUMENT") == 1
        assert rust("GET", route + "/ocr-review?page_index=0&revision=3")["accepted_at"] is None
        if scenario == "plain":
            rerun_bytes = pdf_bytes(rust, document, 3)
            unchanged = complete_ocr("/ocr", 3)
            assert unchanged["status"] == "completed", unchanged
            assert unchanged["result"]["pages_checked"] == 1
            assert unchanged["result"]["pages_ocr_applied"] == 0
            assert unchanged["result"]["provenance"] is None
            assert len(rust("GET", route + "/revisions")) == 3
            assert pdf_bytes(rust, document, 3) == rerun_bytes
    assert pdf_bytes(rust, document, 1, "original") == file.read_bytes()


def restore_rust_ocr_in_reference(rust, documents, package, recovery, root):
    from app.file_io import read_bytes

    auth = {"Authorization": "Bearer synthetic"}
    with TestClient(create_app(root / "ocr-restore-reference", token="synthetic", require_vault=True)) as python:
        python.post("/vault/setup", headers=auth, json={"password": PASSWORD}).raise_for_status()
        response = python.post("/matters/restore", content=package.read_bytes(), headers={
            **auth, "Content-Type": "application/octet-stream",
            "X-CourtBundle-Backup-Credential": recovery, "X-CourtBundle-Backup-Recovery": "true",
        })
        assert response.status_code == 201, response.text
        matter = response.json()["id"]
        response = python.get(f"/matters/{matter}/documents", headers=auth)
        response.raise_for_status()
        restored = {row["source_sha256"]: row for row in response.json()}
        store = python.app.state.store
        for document in documents:
            runs = json.loads(document["ocr_provenance_json"])
            if not runs:
                continue
            row = restored[document["source_sha256"]]
            assert runs == json.loads(row["ocr_provenance_json"])
            assert runs[-1]["provider"] == "Tesseract"
            assert runs[-1]["text_verified"] is False
            revision = row["revision"]
            assert pdf_bytes(rust, document["id"], revision) == read_bytes(store.working_path(row))
            assert pdf_bytes(rust, document["id"], revision, "original") == read_bytes(store.original_path(row))
            runs_response = python.get(f"/documents/{row['id']}/pages/0/text-runs", headers=auth)
            runs_response.raise_for_status()
            assert all("matterdraftocr" not in item["font"].lower() for item in runs_response.json()["runs"])


def all_route_dispatch(rust, root):
    reference = create_app(root / "route-reference", token="synthetic", require_vault=False)
    routes = [route for route in reference.routes if isinstance(route, APIRoute)]
    rust_statuses(routes)
    values = {"part": "99", "page_index": "0", "revision": "1", "filename": "combined.pdf",
              "kind": "docx", "direction": "undo"}
    failures = []
    total = 0
    for route in routes:
        path = re.sub(r"\{([^}]+)\}", lambda match: values.get(match[1], "0" * 32), route.path)
        for method in sorted(route.methods):
            response = rust(method, path, {}, status=None)
            if response["status"] >= 500:
                failures.append((method, route.path, response))
            total += 1
    assert not failures, failures
    print(f"{total} Python method/path pairs reached Rust handlers with synthetic inputs")


def draft_cover(rust):
    matter = rust("POST", "/matters", {
        "name": "Synthetic cover",
        "case_type": "Synthetic application",
        "case_number": "Synthetic-42",
    }, status=201)["id"]
    document = rust("POST", f"/matters/{matter}/cover", status=201)
    assert document["original_name"] == "generated-cover.pdf"
    assert document["bundle_included"] == 1
    assert document["ocr_status"] == "native_text"
    assert document["signature_status"] == "unsigned"
    with pymupdf.open(stream=pdf_bytes(rust, document["id"], 1)) as pdf:
        text = pdf[0].get_text()
        assert "Synthetic application" in text
        assert "Synthetic-42" in text
        assert "not an approved court form" in text


def named_destinations(rust, root):
    matter = rust("POST", "/matters", {"name": "Synthetic navigation"}, status=201)["id"]
    file = root / "positioned-named-destination.pdf"
    with pymupdf.open() as pdf:
        pdf.new_page().insert_text((55, 55), "Link source")
        target = pdf.new_page()
        target.insert_text((65, 150), "Link destination")
        destination = f"[{target.xref} 0 R /XYZ 65 692 1.5]"
        children = pdf.get_new_xref()
        pdf.update_object(children, f"<< /Names [(section) << /D {destination} >>] >>")
        pdf.xref_set_key(pdf.pdf_catalog(), "Names", f"<< /Dests << /Kids [{children} 0 R] >> >>")
        link = pdf.get_new_xref()
        pdf.update_object(link, "<< /Type /Annot /Subtype /Link /Rect [50 760 160 800] /Dest (section) >>")
        pdf.xref_set_key(pdf[0].xref, "Annots", f"[{link} 0 R]")
        pdf.save(file)
    document = rust("IMPORT", matter, source=str(file), status=201)[0]["id"]
    reordered = rust("POST", f"/documents/{document}/pages/reorder",
                     {"expected_revision": 1, "order": [1, 0]})
    with pymupdf.open(stream=pdf_bytes(rust, document, reordered["revision"])) as pdf:
        link = pdf[1].get_links()[0]
        assert link["page"] == 0
        assert link["to"] == pymupdf.Point(65, 150)
        kind, target = pdf.xref_get_key(link["xref"], "Dest")
        assert kind == "array" and re.search(r"/XYZ\s+65\s+692\s+1\.5\s*\]$", target), target
    assert pdf_bytes(rust, document, 1, "original") == file.read_bytes()


def generated_bundle(rust):
    matter = rust("POST", "/matters", {"name": "Synthetic bundle qualification"}, status=201)["id"]
    path = f"/matters/{matter}"
    source = ROOT / "samples/01_petition.pdf"
    document = rust("IMPORT", matter, source=str(source), included=True, status=201)[0]["id"]
    catalog = rust("GET", path + "/variants")
    variant = rust("POST", path + "/variants", {
        **catalog["primary_settings"],
        "name": "Synthetic audience",
        "baseline_kind": "primary",
        "baseline_id": catalog["primary_baseline"]["id"],
        "included_ids": [document],
        "expected_snapshot": catalog["source_snapshot"],
        "max_pages_per_volume": 25,
    })
    review = rust("GET", f"{path}/variants/{variant['id']}/review")
    assert review["current"] and review["checks"]
    assert "max_pages_per_volume" in review["settings_differences"]
    submitted = rust("POST", path + "/build", {}, status=202)
    rust("RUN_PENDING", "")
    done = rust("GET", f"/jobs/{submitted['id']}")
    assert done["status"] == "completed", done
    exports = rust("GET", path + "/exports")
    assert len(exports) == 1, exports
    export = f"{path}/exports/{exports[0]['id']}"
    review = rust("GET", export + "/inspection/combined.pdf")
    with pymupdf.open(stream=b64decode(review["data"])) as pdf:
        assert len(pdf) == 4
        assert pdf[0].get_links()[0]["page"] == 1
        assert pdf.get_toc()
        source_text = pdf[1].get_text()
    state = rust("POST", export + "/validation/run", {})
    assert state["can_download"], state
    assert not state["can_release"]
    report_file = rust("GET", export + "/validation-report.json")
    report = json.loads(b64decode(report_file["data"]))
    assert report["export_id"] == exports[0]["id"]
    assert report["signature_inputs"][0]["document_id"] == exports[0]["page_map_snapshot"][0]["id"]
    assert [{key: row[key] for key in snapshot} for row, snapshot in
            zip(report["page_map"], exports[0]["page_map_snapshot"], strict=True)] == exports[0]["page_map_snapshot"]
    for name, kind, member in [
        ("combined.pdf", "docx", "word/document.xml"),
        ("index.csv", "xlsx", "xl/worksheets/sheet1.xml"),
        ("combined.pdf", "pptx", "ppt/slides/slide1.xml"),
    ]:
        artifact = rust("GET", f"{export}/{name}/office/{kind}")
        with ZipFile(BytesIO(b64decode(artifact["data"]))) as package:
            assert package.testzip() is None
            assert package.read(member)
    printed = rust("POST", f"{export}/combined.pdf/print",
                   {"pages": "2", "orientation": "landscape", "scale": "fit"})
    with pymupdf.open(stream=b64decode(printed["data"])) as pdf:
        assert len(pdf) == 1 and pdf[0].rect == pymupdf.Rect(0, 0, 842, 595)
        assert pdf[0].get_text() == source_text
    password = "Synthetic export password"
    protected = rust("POST", f"{export}/combined.pdf/protected", {"password": password})
    with pymupdf.open(stream=b64decode(protected["data"])) as pdf:
        assert pdf.needs_pass and not pdf.authenticate("incorrect")
        assert pdf.authenticate(password)
        assert len(pdf) == 4 and pdf[1].get_text() == source_text


def paired_page_comparisons(rust, root):
    source = root / "comparison-source.pdf"
    with pymupdf.open() as pdf:
        for line in ("First source page", "Second source page", "Third source page"):
            pdf.new_page().insert_text((58, 72), line)
        pdf.save(source)
    original = source.read_bytes()
    auth = {"Authorization": "Bearer synthetic-page-comparison"}
    name = {"name": "Synthetic frozen page comparison"}
    with TestClient(create_app(root / "page-comparison-reference", token="synthetic-page-comparison",
                               require_vault=False)) as python:
        def reference(method, path, *, status=200, **kwargs):
            headers = {**auth, **kwargs.pop("headers", {})}
            response = python.request(method, path, headers=headers, **kwargs)
            assert response.status_code == status, (method, path, response.text)
            return response

        matter = rust("POST", "/matters", name, status=201)["id"]
        py_matter = reference("POST", "/matters", json=name, status=201).json()["id"]
        doc = rust("IMPORT", matter, source=str(source), included=True, status=201)[0]["id"]
        py_doc = reference(
            "POST", f"/matters/{py_matter}/documents/raw?included=true", content=original, status=201,
            headers={"Content-Type": "application/octet-stream", "X-CourtBundle-Filename": source.name},
        ).json()[0]["id"]
        job = rust("POST", f"/matters/{matter}/build", {}, status=202)["id"]
        py_job = reference("POST", f"/matters/{py_matter}/build", json={}, status=202).json()["id"]
        rust("RUN_PENDING", "")
        assert rust("GET", f"/jobs/{job}")["status"] == "completed"
        deadline = monotonic() + 35
        while True:
            completed = reference("GET", f"/jobs/{py_job}").json()
            if completed["status"] not in {"queued", "running"} or monotonic() > deadline:
                break
            sleep(.05)
        assert completed["status"] == "completed", completed
        build = f"/matters/{matter}/exports/{job}"
        py_build = f"/matters/{py_matter}/exports/{py_job}"
        before = rust("GET", build + "/combined.pdf")["data"]
        py_before = reference("GET", py_build + "/combined.pdf").content
        rust("PUT", build + "/archive", {"archived": True})
        reference("PUT", py_build + "/archive", json={"archived": True})
        order = {"expected_revision": 1, "order": [2, 0, 1]}
        rust("POST", f"/documents/{doc}/pages/reorder", order)
        reference("POST", f"/documents/{py_doc}/pages/reorder", json=order)
        selection = dict(expected_revision="2", prior_source_page="1", current_source_page=2)

        def compare(fields, status=200, *, missing=False):
            route = f"/matters/{matter}/exports/missing" if missing else build
            py_route = f"/matters/{py_matter}/exports/missing" if missing else py_build
            actual = rust("POST", route + "/compare-pages", {"document_id": doc, **fields}, status=status)
            expected = reference("POST", py_route + "/compare-pages", status=status,
                                 json={"document_id": py_doc, **fields}).json()
            if status == 200:
                del actual["build_id"], actual["document_id"], expected["build_id"], expected["document_id"]
            assert actual == expected, (fields, actual, expected)
            return actual

        raw = compare(selection)
        assert raw["differences"] and not raw["numbering_removed"]
        filtered = compare({**selection, "ignore_generated_numbering": "yes"})
        assert filtered["numbering_removed"] and not filtered["differences"]
        assert not filtered["rendered_equal"]
        assert compare({**selection, "current_source_page": 1, "ignore_generated_numbering": True})["differences"]
        compare({**selection, "expected_revision": 1}, status=409)
        compare({**selection, "prior_source_page": 9, "current_source_page": 8}, status=409)
        compare({**selection, "current_source_page": 9}, status=409)
        compare({**selection, "prior_source_page": 0, "ignore_generated_numbering": "maybe"}, status=422)
        compare(selection, status=409, missing=True)
        assert rust("GET", build + "/combined.pdf")["data"] == before
        assert reference("GET", py_build + "/combined.pdf").content == py_before
        assert pdf_bytes(rust, doc, 1, "original") == original


def paired_editable_review_drafts(rust, root):
    from app.office_export import excel_index, powerpoint_draft, word_draft
    from xml.etree import ElementTree as ET

    source = root / "office-source.pdf"
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=1050, height=700)
        page.insert_text((72, 130), "Second row", fontname="cour", fontsize=12)
        page.insert_text((129.6, 90), "Bravo", fontname="cour", fontsize=12)
        page.insert_text((72, 90), "Alpha", fontname="cour", fontsize=12)
        for line in range(12):
            page.insert_text((72, 160 + 20 * line), "x" * 99 + " repeat-able",
                             fontname="cour", fontsize=12)
        pdf.new_page(width=1050, height=700)
        pdf.save(source)

    def paragraphs(package, member, namespace):
        xml = ET.fromstring(package.read(member))
        return [node.text or "" for node in xml.iter(f"{{{namespace}}}t")]

    def sheet(package):
        xml = ET.fromstring(package.read("xl/worksheets/sheet1.xml"))
        ns = {"s": "http://schemas.openxmlformats.org/spreadsheetml/2006/main"}
        cells = [(cell.attrib, cell.findtext("s:is/s:t", namespaces=ns))
                 for cell in xml.findall(".//s:sheetData/s:row/s:c", ns)]
        return cells, xml.find("s:autoFilter", ns).attrib, xml.find(".//s:pane", ns).attrib

    def slides(package):
        ns = "http://schemas.openxmlformats.org/drawingml/2006/main"
        return [paragraphs(package, f"ppt/slides/slide{number}.xml", ns)
                for number in range(1, 1 + sum(bool(re.fullmatch(r"ppt/slides/slide\d+\.xml", name))
                                              for name in package.namelist()))]

    matter = rust("POST", "/matters", {"name": "Synthetic editable Office review"}, status=201)["id"]
    document = rust("IMPORT", matter, source=str(source), included=True, status=201)[0]
    matter_path = f"/matters/{matter}"
    settings = rust("GET", matter_path + "/bundle-settings")
    rust("PUT", matter_path + "/bundle-settings", {
        "expected_snapshot": settings["input_snapshot"],
        "output_marks": {**settings["output_marks"], "office_docx": True,
                         "office_xlsx": True, "office_pptx": True},
    })
    started = rust("POST", matter_path + "/build", {}, status=202)["id"]
    rust("RUN_PENDING", "")
    job = rust("GET", f"/jobs/{started}")
    assert job["status"] == "completed", job
    export = f"{matter_path}/exports/{job['result']['id']}"
    frozen_pdf = root / "office-frozen.pdf"
    frozen_pdf.write_bytes(b64decode(rust("GET", export + "/combined.pdf")["data"]))
    csv = b64decode(rust("GET", export + "/index.csv")["data"])
    file = b64decode(rust("GET", export + "/filing-package.zip")["data"])
    word_text = lambda package: paragraphs(package, "word/document.xml",
                                            "http://schemas.openxmlformats.org/wordprocessingml/2006/main")
    for filename, kind, expected, semantic, output_name in [
        ("combined.pdf", "docx", word_draft(frozen_pdf), word_text, "combined-review.docx"),
        ("index.csv", "xlsx", excel_index(csv), sheet, "index-review.xlsx"),
        ("combined.pdf", "pptx", powerpoint_draft(frozen_pdf), slides, "combined-review.pptx"),
    ]:
        artifact = rust("GET", f"{export}/{filename}/office/{kind}")
        assert artifact["content_disposition"] == f'attachment; filename="{output_name}"'
        assert artifact["conversion_note"].startswith("Editable")
        generated = b64decode(rust("GET", f"{export}/{output_name}")["data"])
        with ZipFile(BytesIO(file)) as filing, ZipFile(BytesIO(b64decode(artifact["data"]))) as native, \
                ZipFile(BytesIO(expected)) as reference, ZipFile(BytesIO(generated)) as bundled:
            assert filing.read(output_name) == generated
            assert semantic(bundled) == semantic(native) == semantic(reference), output_name
            if kind == "docx":
                result = word_text(native)
                assert result.index("Alpha   Bravo") < result.index("Second row")
            if kind == "pptx":
                titles = [slide[0] for slide in slides(native)]
                assert "PDF page 2 · part 1" in titles and "PDF page 2 · part 2" in titles
    for filename, kind, status, detail in [
        ("combined.pdf", "xlsx", 422, "Excel export is derived from the completed index CSV"),
        ("index.csv", "docx", 422, "Word and PowerPoint drafts require a completed PDF"),
        ("combined.pdf", "ods", 422, "Choose Word, Excel or PowerPoint"),
        ("missing.pdf", "docx", 404, "Completed export file not found"),
    ]:
        assert rust("GET", f"{export}/{filename}/office/{kind}", status=status)["detail"] == detail
    assert pdf_bytes(rust, document["id"], 1, "original") == source.read_bytes()


def paired_build_and_part_extracts(rust, root):
    import pypdfium2

    auth = {"Authorization": "Bearer synthetic"}
    options = {"max_pages_per_volume": 4, "keep_groups_intact": True}
    marks = {
        "front_matter": "cover_index", "index_style": "review",
        "numbering_style": "roman_lower", "numbering_prefix": "V{volume} · ",
        "numbering_back_link": True, "part_extracts_enabled": True,
        "processing_quality": "lossless", "cleanup_metadata": True,
        "watermark_enabled": True, "watermark_text": "Review watermark", "watermark_position": "top_left",
        "watermark_font_size": 18, "timestamp_enabled": True, "timestamp_format": "date",
        "timestamp_prefix": "As of", "timestamp_timezone": "Asia/Kolkata",
        "bates_enabled": True, "bates_include_index": True, "bates_prefix": "SYN-",
    }
    filenames = ["combined.pdf", "volume-01.pdf", "volume-02.pdf", "part-01.pdf"]
    matter_data = {"name": "Synthetic Part extracts"}
    source_uri = "https://example.invalid/imported"
    reviewed_uri = "https://example.invalid/reviewed"
    with TestClient(create_app(root / "part-reference", token="synthetic", require_vault=False)) as python:
        def reference(method, path, body=None, *, status=200, **kwargs):
            response = python.request(method, path, json=body, headers={**auth, **kwargs.pop("headers", {})}, **kwargs)
            assert response.status_code == status, (method, path, response.text)
            return response

        def reference_json(*args, **kwargs):
            return reference(*args, **kwargs).json()

        def completed_pair(native_route, python_route):
            native_id = rust("POST", native_route, {}, status=202)["id"]
            python_id = reference_json("POST", python_route, {}, status=202)["id"]
            rust("RUN_PENDING", "")
            native = rust("GET", f"/jobs/{native_id}")
            assert native["status"] == "completed", native
            deadline = monotonic() + 20
            while True:
                baseline = reference_json("GET", f"/jobs/{python_id}")
                if baseline["status"] not in ("queued", "running") or monotonic() > deadline:
                    break
                sleep(.05)
            assert baseline["status"] == "completed", baseline
            result, python_result = native["result"], baseline["result"]
            assert result["id"] == result["export_id"] == native_id
            assert python_result["id"] == python_id
            assert set(python_result) <= set(result), set(python_result) - set(result)
            for key in ("artifact_kind", "pages", "volumes", "interactive_content_policy"):
                assert result[key] == python_result[key], key
            return result, python_result

        mids = [rust("POST", "/matters", matter_data, status=201)["id"],
                reference("POST", "/matters", matter_data, status=201).json()["id"]]
        paths = [f"/matters/{matter}" for matter in mids]
        ids = [[], []]
        original_files = []
        for i in range(2):
            filename = root / f"part-source-{i}.pdf"
            with pymupdf.open() as sample:
                for p in range(2):
                    sample.new_page().insert_text((60, 125), f"Source {i + 1} page {p + 1}")
                if i == 0:
                    first = sample[0]
                    first.draw_rect(pymupdf.Rect(8, 8, 150, 60), color=(0, 0, 1))
                    first.insert_text((240, 180), "Synthetic staged removal")
                    first.add_text_annot((100, 150), "Synthetic source review note")
                    first.insert_link({"kind": pymupdf.LINK_URI, "from": pymupdf.Rect(70, 180, 210, 195),
                                       "uri": source_uri})
                    sample.xref_set_key(first.xref, "AA", "<< /O << /S /Named /N /FirstPage >> >>")
                    sample[1].insert_link({"kind": pymupdf.LINK_URI, "from": pymupdf.Rect(14, 18, 85, 35),
                                           "uri": source_uri})
                else:
                    first = sample[0]
                    width, height = first.rect.width, first.rect.height
                    obstacle = BytesIO()
                    Image.new("RGB", (142, 52), (100, 130, 190)).save(obstacle, format="PNG")
                    first.insert_image(pymupdf.Rect(width - 150, height - 60, width - 8, height - 8),
                                       stream=obstacle.getvalue())
                    first.set_cropbox(pymupdf.Rect(4, 6, width - 7, height - 8))
                    sample[1].add_text_annot((18, 22), "Synthetic source comment icon")
                sample.save(filename)
            original_files.append(filename.read_bytes())
            ids[0].append(rust("IMPORT", mids[0], source=str(filename), included=True, status=201)[0]["id"])
            ids[1].append(reference(
                "POST", paths[1] + "/documents/raw", status=201, content=filename.read_bytes(),
                headers={"Content-Type": "application/octet-stream", "X-CourtBundle-Filename": filename.name},
            ).json()[0]["id"])
        selection = reference("GET", paths[1] + "/membership").json()
        reference("PUT", paths[1] + "/membership",
                  {"included_ids": ids[1], "expected_snapshot": selection["input_snapshot"]})

        def set_up(backend, path, documents, upload_image):
            settings = backend("GET", path + "/bundle-settings")
            output_marks = {**settings["output_marks"], **marks}
            backend("PUT", path + "/bundle-settings", {
                **options, "output_marks": output_marks,
                "expected_snapshot": settings["input_snapshot"],
            })
            annotations = f"/documents/{documents[0]}/annotations"
            backend("POST", annotations, {
                "expected_revision": 1, "page_index": 0, "kind": "text", "body": "Reviewed text mark",
                "rect_x": .15, "rect_y": .45, "rect_width": .5, "rect_height": .16, "output_scope": "pdf",
                "text_style": {"font_family": "cour", "font_size": 14, "bold": True, "alignment": "center"},
            }, status=201)
            backend("POST", annotations, {
                "expected_revision": 1, "page_index": 0, "kind": "comment", "body": "Synthetic private note",
                "rect_x": .32, "rect_y": .61, "rect_width": .15, "rect_height": .05,
            }, status=201)
            backend("POST", annotations, {
                "expected_revision": 1, "page_index": 0, "kind": "redaction", "output_scope": "pdf",
                "rect_x": .37, "rect_y": .18, "rect_width": .32, "rect_height": .055,
            }, status=201)
            image = BytesIO()
            Image.new("RGB", (80, 72), (212, 41, 80)).save(image, format="PNG")
            upload_image(annotations + "/image", {
                "expected_revision": 1, "page_index": 0,
                "rect_x": .69, "rect_y": .5, "rect_width": .12, "rect_height": .08,
            }, image.getvalue())
            stale_link = {
                "id": "4" * 32, "title": "Earlier reviewed destination", "kind": "internal", "uri": "",
                "source_document_id": documents[0], "source_page_index": 1, "source_revision": 1,
                "target_document_id": documents[1], "target_page_index": 1, "target_revision": 1,
                "rect_x": .12, "rect_y": .26, "rect_width": .25, "rect_height": .05,
            }
            assert backend("PUT", path + "/links", {"entries": [stale_link]})[0]["resolved"]
            stale_bookmark = {"id": "6" * 32, "title": "Earlier title", "document_id": documents[1],
                              "page_index": 0, "target_revision": 1}
            assert backend("PUT", path + "/bookmarks", {"entries": [stale_bookmark]})[0]["resolved"]
            revised = backend("POST", f"/documents/{documents[1]}/pages/rotate", {
                "expected_revision": 1, "page_index": 0, "degrees": 180,
            })
            assert revised["revision"] == 2
            backend("PUT", path + "/bookmarks", {"entries": [
                {"id": "1" * 32, "title": "Root source", "document_id": documents[0],
                 "page_index": 0, "target_revision": 1},
                {"id": "2" * 32, "title": "Other volume", "document_id": documents[1],
                 "page_index": 1, "target_revision": 2, "parent_id": "1" * 32},
                stale_bookmark,
            ]})
            saved_links = backend("PUT", path + "/links", {"entries": [
                {"id": "3" * 32, "title": "Reviewed reference", "source_document_id": documents[0],
                 "source_page_index": 0, "source_revision": 1, "kind": "external", "uri": reviewed_uri,
                 "rect_x": .12, "rect_y": .26, "rect_width": .25, "rect_height": .05},
                {**stale_link, "id": "5" * 32, "title": "Reviewed cross-volume destination",
                 "source_page_index": 0, "target_revision": 2, "rect_y": .35},
                stale_link,
            ]})
            assert [entry["resolved"] for entry in saved_links] == [True, True, False], saved_links
            return [backend("POST", f"/documents/{document}/pages/{page}/output-marks-preview", output_marks)["marks"]
                    for document in documents for page in range(2)]

        def native_image(path, fields, image):
            rust("POST", path, {**fields, "filename": "overlay.png", "data": b64encode(image).decode("ascii")}, status=201)

        def reference_image(path, fields, image):
            reference("POST", path, status=201, data=fields, files={"file": ("overlay.png", image, "image/png")})

        rust_previews = set_up(rust, paths[0], ids[0], native_image)
        python_previews = set_up(reference_json, paths[1], ids[1], reference_image)
        for left, right in zip(rust_previews, python_previews, strict=True):
            assert [(item["kind"], item["text"], item["overlaps_content"]) for item in left] == [
                (item["kind"], item["text"], item["overlaps_content"]) for item in right], (left, right)
            assert [item["overlaps_content"] for item in left] == [True, False, False], left
            for native, baseline in zip(left, right, strict=True):
                assert all(abs(a - b) < .0001 for a, b in zip(native["relative_rect"], baseline["relative_rect"], strict=True)), (native, baseline)
        result, python_result = completed_pair(*(path + "/build" for path in paths))
        native_export = f"{paths[0]}/exports/{result['id']}"
        python_export = f"{paths[1]}/exports/{python_result['id']}"
        actual = next(item for item in rust("GET", paths[0] + "/exports") if item["id"] == result["id"])
        expected = next(item for item in reference_json("GET", paths[1] + "/exports") if item["id"] == python_result["id"])
        for key in ("page_map_snapshot", "input_revisions", "files", "validation_contract", "input_snapshot_id"):
            assert result[key] == actual[key], key
        assert result["input_snapshot_id"]
        assert result["insertion_policy_contract"] is python_result["insertion_policy_contract"] is None
        for manifest in (result, python_result):
            assert {"SOURCE_ANNOTATIONS_OMITTED", "SOURCE_LINKS_OMITTED", "UNRESOLVED_REVIEWED_LINKS",
                    "CROSS_VOLUME_LINKS_COMBINED_ONLY", "UNRESOLVED_MANUAL_BOOKMARKS", "OUTPUT_MARK_OVERLAP"} <= {
                finding["code"] for finding in manifest["checks"]
            }
            assert manifest["reviewed_link_inputs"][2]["title"] is None
            assert manifest["reviewed_link_inputs"][2]["uri"] is None
            assert manifest["manual_bookmark_inputs"][2]["title"] is None
        report = json.loads(b64decode(rust("GET", native_export + "/validation-report.json")["data"]))
        expected_report = reference_json("GET", python_export + "/validation-report.json")
        assert report["interactive_content_policy"] == result["interactive_content_policy"]
        assert report["reviewed_link_summary"] == expected_report["reviewed_link_summary"] == {
            "combined_retained": 2, "standalone_cross_volume_omissions": 1, "unresolved": 1,
        }
        assert report["output_mark_summary"] == expected_report["output_mark_summary"] == {
            "inserted": 14, "overlaps_content": 2,
        }, (report["output_mark_summary"], expected_report["output_mark_summary"])
        assert report["bates_policy"] == expected_report["bates_policy"] == {
            "enabled": True, "include_index": True, "include_separator": False, "restart_per_volume": False,
        }
        assert report["page_annotation_summary"] == expected_report["page_annotation_summary"] == {
            "image_overlays_inserted": 1, "editable_marks_inserted": 1, "redactions_applied": 1,
            "private_review_annotations_omitted": 1, "unresolved_image_overlays": 0,
        }
        assert next(job for job in rust("GET", paths[0] + "/jobs") if job["id"] == result["id"])["result"] == result
        for manifest, documents in ((actual, ids[0]), (expected, ids[1])):
            for row in manifest["page_map_snapshot"]:
                row["id"] = documents.index(row["id"])
        assert actual["page_map_snapshot"] == expected["page_map_snapshot"]
        actual_csv = b64decode(rust("GET", native_export + "/index.csv")["data"])
        expected_csv = reference("GET", python_export + "/index.csv").content
        assert actual_csv == expected_csv
        for filename in filenames:
            lhs = b64decode(rust("GET", native_export + "/inspection/" + filename)["data"])
            rhs = reference("GET", python_export + "/inspection/" + filename).content
            with pymupdf.open(stream=lhs) as a, pymupdf.open(stream=rhs) as b:
                assert a.get_toc() == b.get_toc(), filename
                assert not a.metadata["creator"] and not b.metadata["creator"]
                assert len(a) == len(b)
                for left, right in zip(a, b):
                    assert left.rect == right.rect and left.rotation == right.rotation
                    assert not list(left.annots() or ()) and not list(right.annots() or ())
                    assert a.xref_get_key(left.xref, "AA")[0] == "null"
                    assert [link.get("page") for link in left.get_links()] == [link.get("page") for link in right.get_links()]
                    external = [link["uri"] for link in left.get_links() if link["kind"] == pymupdf.LINK_URI]
                    assert external == [link["uri"] for link in right.get_links() if link["kind"] == pymupdf.LINK_URI]
                    expects_reviewed_link = not filename.startswith("part-") and "Source 1 page 1" in left.get_text()
                    assert external == ([reviewed_uri] if expects_reviewed_link else []), (
                        filename, left.number, external, left.get_text(), right.get_text()
                    )
                    expected_text = right.get_text()
                    for phrase in ("Reviewed text mark", "Source 1 page 1", "Source 2 page 2"):
                        assert (phrase in left.get_text()) == (phrase in expected_text)
                    for output, manifest, previews in ((left, result, rust_previews), (right, python_result, python_previews)):
                        page_text = output.get_text()
                        assert "Synthetic staged removal" not in page_text and "Synthetic private note" not in page_text
                        source_page = re.search(r"Source ([12]) page ([12])", page_text)
                        if source_page:
                            source, page = (int(value) - 1 for value in source_page.groups())
                            output_date = datetime.fromisoformat(manifest["output_mark_timestamp"]).astimezone(ZoneInfo("Asia/Kolkata"))
                            for mark in previews[2 * source + page]:
                                x, y, width, height = mark["relative_rect"]
                                rect = pymupdf.Rect(x * output.rect.width, y * output.rect.height,
                                                   (x + width) * output.rect.width, (y + height) * output.rect.height)
                                value = f"As of {output_date:%Y-%m-%d}" if mark["kind"] == "timestamp" else mark["text"]
                                assert any((glyphs * output.rotation_matrix).intersects(rect)
                                           for glyphs in output.search_for(value)), (filename, output.number, value)
                        if "Source 1 page 1" in page_text:
                            pixels = output.get_pixmap()
                            assert pixels.pixel(int(output.rect.width * .75), int(output.rect.height * .54)) == (212, 41, 80)
            with pypdfium2.PdfDocument(lhs) as a, pypdfium2.PdfDocument(rhs) as b:
                assert [a.get_page_label(i) for i in range(len(a))] == [b.get_page_label(i) for i in range(len(b))]

        variant_plans, supplement_plans = [], []
        for backend, path, initial, documents in zip((rust, reference_json), paths, (result, python_result), ids, strict=True):
            catalog = backend("GET", path + "/variants")
            variant_plans.append(backend("POST", path + "/variants", {
                **catalog["primary_settings"], "name": "Synthetic audience", "baseline_kind": "primary",
                "baseline_id": catalog["primary_baseline"]["id"], "expected_snapshot": catalog["source_snapshot"],
                "included_ids": documents[1:],
            }))
            catalog = backend("GET", path + "/supplements")
            source = next(doc for doc in catalog["documents"] if doc["id"] == documents[0])
            supplement_plans.append(backend("POST", path + "/supplements", {
                **catalog["defaults"], "title": "Synthetic supplement", "baseline_build_id": initial["id"],
                "selected_page_ids": [source["pages"][1]["id"]], "expected_snapshot": catalog["input_snapshot"],
                "insertion_policy": "separate_supplement", "policy_reviewed": True,
            }))
        variants = completed_pair(*(f"{path}/variants/{plan['id']}/build"
                                    for path, plan in zip(paths, variant_plans, strict=True)))
        for variant, plan in zip(variants, variant_plans, strict=True):
            assert variant["artifact_kind"] == "variant"
            assert variant["variant_plan_id"] == plan["id"]
            assert variant["variant_plan_version"] == plan["version"]
            assert variant["variant_baseline"] == plan["baseline"]
            assert variant["reviewed_link_inputs"] == []
            assert [bookmark["id"] for bookmark in variant["manual_bookmark_inputs"]] == ["2" * 32, "6" * 32]
        supplements = completed_pair(*(f"{path}/supplements/{plan['id']}/build"
                                       for path, plan in zip(paths, supplement_plans, strict=True)))
        for supplement, plan in zip(supplements, supplement_plans, strict=True):
            assert supplement["artifact_kind"] == "supplement"
            assert supplement["supplement_plan_id"] == plan["id"]
            assert supplement["supplement_plan_version"] == plan["version"]
            assert supplement["baseline_build_id"] == plan["baseline_build_id"]
            policy = supplement["insertion_policy_contract"]
            assert policy["mode"] == "separate_supplement"
            assert policy["selected_page_ids"] == plan["selected_page_ids"]
            assert not policy["blockers"]
            assert [link["id"] for link in supplement["reviewed_link_inputs"]] == ["4" * 32]
            assert supplement["manual_bookmark_inputs"] == supplement["page_annotation_inputs"] == []
        supplement_report = rust("GET", f"{paths[0]}/exports/{supplements[0]['id']}/validation-report.json")
        report_policy = json.loads(b64decode(supplement_report["data"]))["policy_preview"]
        assert report_policy == {key: supplements[0]["insertion_policy_contract"][key] for key in report_policy}
        for document, original in zip(ids[0], original_files, strict=True):
            assert pdf_bytes(rust, document, 1, "original") == original
        archived = rust("PUT", native_export + "/archive", {"archived": True})
        assert archived["archived_at"]
        assert rust("GET", f"/jobs/{result['id']}")["result"] == result


def print_request_contract(rust, matter, root):
    fixture = root / "print-page-selection.pdf"
    with pymupdf.open() as pdf:
        for page in range(1, 4):
            pdf.new_page().insert_text((72, 90), f"Synthetic print page {page}")
        pdf.save(fixture)
    original = fixture.read_bytes()
    native = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
    auth = {"Authorization": "Bearer synthetic-print"}

    def page_text(data):
        with pymupdf.open(stream=data, filetype="pdf") as pdf:
            return [page.get_text().strip() for page in pdf]

    with TestClient(create_app(root / "print-reference", token="synthetic-print", require_vault=False)) as python:
        matter = python.post("/matters", headers=auth, json={"name": "Synthetic print requests"}).json()["id"]
        imported = python.post(f"/matters/{matter}/documents/raw", content=original,
                               headers={**auth, "Content-Type": "application/octet-stream",
                                        "X-CourtBundle-Filename": fixture.name})
        assert imported.status_code == 201, imported.text
        reference = imported.json()[0]["id"]

        def assert_refusal(native_path, reference_path, body, status, detail):
            expected = python.post(reference_path, headers=auth, json=body)
            assert (expected.status_code, expected.json()) == (status, {"detail": detail})
            assert rust("POST", native_path, body, status=None) == {"status": status, "body": expected.json()}

        for suffix in ("/print", "/original/print"):
            native_path = f"/documents/{native}{suffix}"
            reference_path = f"/documents/{reference}{suffix}"
            for spec, pages in (("३,١-٢", [3, 1, 2]), ("𝟛\t-𝟛,\u2003１", [3, 1]),
                                ("\x1cALL\u00a0", [1, 2, 3]), ("\u2003" * 2047 + "२", [2])):
                body = {"expected_revision": 1, "pages": spec}
                expected = python.post(reference_path, headers=auth, json=body)
                assert expected.status_code == 200, expected.text
                actual = b64decode(rust("POST", native_path, body)["data"])
                assert page_text(actual) == page_text(expected.content) == [f"Synthetic print page {page}" for page in pages]

            invalid = "Enter all or page numbers such as 1,3-5"
            for options, detail in (
                ({"pages": "4,x"}, invalid),
                ({"pages": "1,1,-"}, invalid),
                ({"pages": "²"}, invalid),
                ({"pages": 1}, invalid),
                ({"pages": "\u2003" * 2048 + "२"}, invalid),
                ({"pages": "1,1,4"}, "Page range must stay between 1 and 3"),
                ({"pages": "3,3"}, "Select 1–500 distinct pages for one print job"),
                ({"orientation": "invalid", "scale": "invalid"}, "Choose source, portrait or landscape paper"),
                ({"orientation": []}, "unhashable type: 'list'"),
                ({"scale": {}}, "unhashable type: 'dict'"),
            ):
                assert_refusal(native_path, reference_path, {"expected_revision": 1, **options}, 422, detail)

        assert_refusal(f"/documents/{native}/print", f"/documents/{reference}/print", {}, 409,
                       "Working PDF revision changed; reload the document")
        absent = "/documents/" + "0" * 32 + "/print"
        assert_refusal(absent, absent, {}, 404, "Document not found")
        assert pdf_bytes(rust, native, 1, "original") == original
        assert python.get(f"/documents/{reference}/original", headers=auth).content == original


def fixed_paper_print_frames(rust, matter, root):
    from app.print_export import print_copy

    fixture = root / "print-rotation-and-unit.pdf"
    with pymupdf.open() as pdf:
        for unit in (0.5, 1, 2):
            for rotation in (0, 90, 180, 270):
                page = pdf.new_page(width=200, height=180)
                page.draw_rect(pymupdf.Rect(20, 80, 43, 101), fill=(0.1, 0.4, 0.7), width=0)
                page.insert_text((35, 100), "Visible markers")
                pdf.xref_set_key(page.xref, "UserUnit", str(unit))
                pdf.xref_set_key(page.xref, "CropBox", "[-20 -10 160 120]")
                page.set_rotation(rotation)
        pdf.save(fixture)
    original = fixture.read_bytes()
    document = rust("IMPORT", matter, source=str(fixture), status=201)[0]["id"]
    with pymupdf.open(stream=original) as source:
        thumbnail = b64decode(rust("GET", f"/documents/{document}/thumbnail?revision=1")["png"])
        scale = min(180 / source[0].rect.width, 220 / source[0].rect.height, 1)
        expected = source[0].get_pixmap(matrix=pymupdf.Matrix(scale, scale))
        with Image.open(BytesIO(thumbnail)) as image:
            assert image.size == (expected.width, expected.height)
        for index, page in enumerate(source, start=1):
            preview = rust("GET", f"/documents/{document}/metadata-source?revision=1&page={index}")
            image_bytes = b64decode(preview["image"].partition(",")[2])
            expected = page.get_pixmap(matrix=pymupdf.Matrix(1.4, 1.4))
            with Image.open(BytesIO(image_bytes)) as image:
                assert image.size == (expected.width, expected.height), index
    for orientation, scale in (
        ("source", "fit"), ("portrait", "actual"), ("landscape", "fit"),
    ):
        requested = dict(expected_revision=1, orientation=orientation, scale=scale, pages="all")
        baseline = print_copy(original, "all", orientation, scale)
        for scope in ("", "/original"):
            printed = rust("POST", f"/documents/{document}{scope}/print", requested)
            with pymupdf.open(stream=baseline) as expected, pymupdf.open(
                    stream=b64decode(printed["data"])) as actual:
                assert len(expected) == len(actual) == 12
                for index, (before, after) in enumerate(zip(expected, actual, strict=True)):
                    case = (orientation, scale, scope, index)
                    assert tuple(before.rect) == tuple(after.rect), case
                    assert before.rotation == after.rotation, case
                    assert before.get_text() == after.get_text(), case
                    left = [drawing["rect"] for drawing in before.get_drawings()]
                    right = [drawing["rect"] for drawing in after.get_drawings()]
                    assert len(left) == len(right), case
                    for expected_box, actual_box in zip(left, right, strict=True):
                        assert max(abs(a - b) for a, b in zip(expected_box, actual_box, strict=True)) < .005, case
                    left_raster = before.get_pixmap()
                    right_raster = after.get_pixmap()
                    assert (left_raster.width, left_raster.height) == (right_raster.width, right_raster.height), case
    assert pdf_bytes(rust, document, 1, "original") == original
    assert pdf_bytes(rust, document, 1) == original


def portable_limits(rust, root):
    auth = {"Authorization": "Bearer synthetic"}
    with TestClient(create_app(root / "python-store", token="synthetic", require_vault=False)) as python:
        matter = python.post("/matters", headers=auth, json={"name": "Large backup fixture"}).json()["id"]
        original = (ROOT / "samples/01_petition.pdf").read_bytes()
        response = python.post(f"/matters/{matter}/documents/raw", content=original,
                               headers={**auth, "Content-Type": "application/octet-stream",
                                        "X-CourtBundle-Filename": "large-fixture.pdf"})
        assert response.status_code == 201, response.text
        backup = python.get(f"/matters/{matter}/portable", headers=auth)
        assert backup.status_code == 200, backup.text
    # Enlarge one retained working member with legal PDF trailing whitespace. The
    # outer ZIP stays small, so this exercises the per-member limit directly.
    large = root / "large-member.cbmatter"
    padding = b" " * (1024 * 1024)
    with ZipFile(BytesIO(backup.content)) as reference, ZipFile(large, "w", ZIP_DEFLATED) as output:
        manifest = json.loads(reference.read("manifest.json"))
        working = manifest["documents"][0]["working_relpath"]
        for entry in manifest["files"]:
            name = "files/" + entry["path"]
            data = reference.read(name)
            if entry["path"] == working:
                digest = sha256(data)
                with output.open(name, "w") as member:
                    member.write(data)
                    for _ in range(161):
                        member.write(padding)
                        digest.update(padding)
                entry["bytes"] += 161 * len(padding)
                entry["sha256"] = digest.hexdigest()
            else:
                output.writestr(name, data)
        output.writestr("manifest.json", json.dumps(manifest))
    restored = rust("RESTORE_BACKUP", "", source=str(large), status=201)
    rust("SAVE_BACKUP", restored["id"], destination=str(root / "large-roundtrip.cbmatter"))
    # Outer packages beyond the native restore limit are refused before ZIP parsing.
    oversized = root / "oversized.cbmatter"
    with oversized.open("wb") as file:
        file.truncate(4 * 1024**3 + 1)
    result = rust("RESTORE_BACKUP", "", source=str(oversized), status=422)
    assert "4 GiB" in result["detail"], result


def legacy_revision_backup(rust, root):
    store = root / "legacy-python-store"
    auth = {"Authorization": "Bearer synthetic"}
    original = (ROOT / "samples/01_petition.pdf").read_bytes()
    with TestClient(create_app(store, token="synthetic", require_vault=False)) as python:
        matter = python.post("/matters", headers=auth, json={"name": "Legacy revision fixture"}).json()["id"]
        response = python.post(f"/matters/{matter}/documents/raw", content=original,
                               headers={**auth, "Content-Type": "application/octet-stream",
                                        "X-CourtBundle-Filename": "legacy.pdf"})
        assert response.status_code == 201, response.text
        document = response.json()[0]["id"]
        # Reproduce the historical revision-table backfill from a document that
        # had already been edited when retention was introduced.
        with sqlite3.connect(store / "court_bundle.sqlite3") as database:
            database.execute("UPDATE documents SET revision=3 WHERE id=?", [document])
            database.execute("UPDATE document_revisions SET revision=3 WHERE document_id=?", [document])
        backup = python.get(f"/matters/{matter}/portable", headers=auth)
        assert backup.status_code == 200, backup.text
    package = root / "legacy-revision.cbmatter"
    package.write_bytes(backup.content)
    restored = rust("RESTORE_BACKUP", "", source=str(package), status=201)
    doc, = rust("GET", f"/matters/{restored['id']}/documents")
    route = f"/documents/{doc['id']}"
    assert doc["revision"] == 3
    assert rust("GET", f"/matters/{restored['id']}/storage")["by_category"]["working"] > 0
    assert [item["revision"] for item in rust("GET", route + "/revisions")] == [3]
    assert pdf_bytes(rust, doc["id"], 3) == original
    assert pdf_bytes(rust, doc["id"], 3, "original") == original
    edited = rust("POST", route + "/pages/rotate",
                  {"expected_revision": 3, "page_index": 0, "degrees": 90})
    assert edited["revision"] == 4
    assert [item["revision"] for item in rust("GET", route + "/revisions")] == [4, 3]
    exported = root / "restored-legacy-revision.cbmatter"
    rust("SAVE_BACKUP", restored["id"], destination=str(exported))
    again = rust("RESTORE_BACKUP", "", source=str(exported), status=201)
    copied, = rust("GET", f"/matters/{again['id']}/documents")
    assert [item["revision"] for item in rust("GET", f"/documents/{copied['id']}/revisions")] == [4, 3]


def run(root):
    paragraph_trial_policy(root)
    pixels = BytesIO()
    Image.new("CMYK", (2, 2)).save(pixels, format="JPEG")
    local_profile = root / "reference-cmyk.icc"
    local_profile.write_bytes(reference_cmyk_profile(pixels.getvalue()))
    with patch.dict(os.environ, {"MATTERDRAFT_CMYK_PROFILE": str(local_profile)}), running_vault(root / "vault") as rust:
        setup = rust("POST", "/vault/setup", {"password": PASSWORD})
        all_route_dispatch(rust, root)
        draft_cover(rust)
        named_destinations(rust, root)
        matter = rust("POST", "/matters", {"name": "Synthetic replacement contract"},
                      status=201)["id"]
        image_import(rust, matter, root)
        image_upload_refusals(rust, matter, root)
        annotation_layer_order(rust, matter, root)
        rotated_image_overlays(rust, matter, root)
        placement_on_physical_pages(rust, root)
        textbox_placement(rust, matter, root)
        working_text_collisions(rust, matter, root)
        annotation_overlay_contract(rust, root)
        profiled_png_resources(rust, matter, root)
        cmyk_jpeg_placement(rust, matter, root)
        redaction(rust, matter, root)
        redaction(rust, matter, root, shear=True)
        restore_region_on_large_sheet(rust, matter, root)
        for encoding in ("raw", "flate", "stencil", "fax"):
            inline_redaction(rust, matter, root, encoding)
        vector_redaction(rust, matter, root)
        vector_redaction(rust, matter, root, shear=True)
        for kind in ("curve", "stroke", "pattern", "shading", "curved-clip", "painted-clip",
                     "stroked-clip", "pattern-text"):
            complex_vector_redaction(rust, matter, root, kind)
        for kind in ("exterior", "wide", "dashed"):
            stroke_graphics_state_redaction(rust, matter, root, kind)
        for kind in ("multiply", "alpha-is-shape", "open", "even-odd", "close", "sheared-clip",
                     "rotated-crop", "spot-color", "dash", "dash-odd", "dash-covered-start",
                     "dash-round-dots", "dash-dense", "edge-ink"):
            combined_vector_redaction(rust, matter, root, kind)
        inactive_soft_mask_redaction(rust, matter, root)
        inherited_form_masks_after_redaction(rust, matter, root)
        for named in (False, True):
            marked_content_redaction(rust, matter, root, named)
        for kind in ("soft", "matte", "indexed"):
            different_mask_redaction(rust, matter, root, kind)
        native_color_image_redaction(rust, matter, root)
        jpeg2000_redaction(rust, matter, root)
        lossless_compression(rust, matter, root)
        compression_image_placements(rust, matter, root)
        compression_color_spaces(rust, matter, root)
        painted_image_admission(rust, matter, root)
        for variant in ("simple", "identity", "custom", "inherited", "named-base",
                        "hindi", "predefined", "shaped-hindi", "shaped-javanese", "shaped-arabic"):
            paragraph(rust, matter, root, variant)
        rotated_text_edit(rust, matter, root)
        paragraph_viewports(rust, matter, root)
        paragraph_object_bounds(rust, matter, root)
        embedded_paragraph_object_bounds(rust, matter, root)
        paragraph_reflow_destination(rust, matter, root)
        paragraph_annotation_obstacles(rust, matter, root)
        single_text_run_placement(rust, matter, root)
        scaled_text_run_placement(rust, matter, root)
        text_edit_preserves_document(rust, root)
        shared_form_text_edit(rust, root)
        tagged_form_text_edit(rust, root)
        page_tagged_form_edit(rust, root)
        actual_text_run_edit(rust, root)
        short_actual_text_edit(rust, root)
        single_anchor_actual_text_edit(rust, root)
        middle_tag_reading_order(rust, root)
        cross_page_actual_text_edit(rust, root)
        grouped_actual_text_edit(rust, root)
        inherited_form_font_edit(rust, root)
        tiling_pattern_text_is_artwork(rust, root)
        ocr(rust, matter)
        ocr_mixed_native_overlap(rust, root)
        ocr_physical_page_size(rust, matter, root)
        ocr_screening_sources(rust, root)
        for scenario in ("plain", "navigation", "empty-containers", "form", "attachment",
                         "annotation", "link-before-annotation"):
            ocr_preserves_catalog_and_rerun(rust, root, scenario)
        package = root / "roundtrip.cbmatter"
        rust("SAVE_BACKUP", matter, destination=str(package))
        restored = rust("RESTORE_BACKUP", "", source=str(package), status=201)
        assert restored["id"] != matter
        expected = rust("GET", f"/matters/{matter}/documents")
        actual = rust("GET", f"/matters/{restored['id']}/documents")
        fields = ("title", "source_sha256", "revision", "page_count", "ocr_status")
        assert [[row[k] for k in fields] for row in actual] == [
            [row[k] for k in fields] for row in expected
        ]
        restore_rust_ocr_in_reference(rust, expected, package, setup["recovery_key"], root)
        generated_bundle(rust)
        paired_page_comparisons(rust, root)
        paired_editable_review_drafts(rust, root)
        paired_build_and_part_extracts(rust, root)
        print_request_contract(rust, matter, root)
        fixed_paper_print_frames(rust, matter, root)
        portable_limits(rust, root)
        legacy_revision_backup(rust, root)


if __name__ == "__main__":
    subprocess.run(["cargo", "build", "--offline", "--locked", "--manifest-path",
                    str(ROOT / "rust-backend/Cargo.toml"), "--example", "vault_contract"],
                   check=True)
    with tempfile.TemporaryDirectory(prefix="matterdraft-replacement-contract-") as directory:
        run(Path(directory))
    print("Partial/shared form redaction, embedded-font reflow, offline OCR, streamed backup and generated bundle passed")
