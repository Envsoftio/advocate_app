"""Compare working-copy placement against synthetic Type 3 paint traces."""

from pathlib import Path
from tempfile import TemporaryDirectory

from PIL import ImageChops
import pymupdf
import pypdfium2

from app.pdf_ops import ProcessingError, add_text_overlay
from replacement_contract import pdf_bytes, type3_program_streams, type3_source_font
from vault_contract import PASSWORD, running_vault


def source_pdf(path, rotation, program, in_appearance=False):
    with pymupdf.open() as pdf:
        page = pdf.new_page(width=580, height=620)
        page.insert_font("helv")
        page.draw_rect((32, 32, 48, 48))
        resources = int(pdf.xref_get_key(page.xref, "Resources")[1].split()[0])

        def object_ref(fields, stream=None):
            reference = pdf.get_new_xref()
            pdf.update_object(reference, f"<< {fields} >>")
            if stream is not None:
                pdf.update_stream(reference, stream)
            return f"{reference} 0 R"

        function = "/Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>"
        axial = f"/ShadingType 2 /ColorSpace /DeviceGray /Coords [160 100 330 400] {function}"
        shade = object_ref(axial)
        bounded = object_ref(f"{axial} /BBox [120 100 425 400]")
        radial = object_ref(f"/ShadingType 3 /ColorSpace /DeviceGray "
                            f"/Coords [160 110 15 270 300 190] /BBox [123 89 482 490] {function}")
        calculator = object_ref("/FunctionType 4 /Domain [0 1 0 1] /Range [0 1]", b"{ pop }")
        function_based = object_ref(
            "/ShadingType 1 /ColorSpace /DeviceGray /Domain [0 1 0 1] "
            f"/Matrix [200 30 -15 220 140 150] /BBox [125 145 310 330] /Function {calculator}",
        )
        mesh = object_ref(
            "/ShadingType 4 /ColorSpace /DeviceGray /BitsPerCoordinate 8 /BitsPerComponent 8 "
            "/BitsPerFlag 8 /Decode [140 430 110 370 0 1]",
            bytes((0, 0, 0, 0, 0, 255, 0, 128, 0, 128, 255, 255)),
        )
        image = object_ref("/Subtype /Image /Width 1 /Height 1 "
                           "/ColorSpace /DeviceRGB /BitsPerComponent 8", bytes((35, 97, 170)))
        transparency = object_ref(
            "/Subtype /Form /BBox [100 100 450 420] /Group << /S /Transparency /I true >> "
            f"/Resources << /XObject << /I {image} >> >>",
            b".25 g 150 150 120 155 re f q 100 0 0 100 325 300 cm /I Do Q",
        )
        form = object_ref(
            f"/Subtype /Form /BBox [100 100 450 420] /Resources << /Shading << /Axis {shade} >> >>",
            b"/Axis sh",
        )
        font_resources = (f"<< /XObject << /Group {transparency} /Form {form} >> "
                          f"/Shading << /Axis {shade} /Bounded {bounded} /Radial {radial} "
                          f"/Function {function_based} /Mesh {mesh} >> "
                          "/ExtGState << /Setup << /LW 56 /LJ 2 /D [[] 0] >> "
                          "/Alpha << /ca .5 >> /Zero << /ca 0 >> >> >>")
        stream = b""
        if program is not None:
            font = type3_source_font(pdf, {"A": program}, resources=font_resources)
            stream = b"BT /T3 24 Tf 1 0 0 1 230 315 Tm (A) Tj ET"
            if in_appearance:
                appearance = object_ref(
                    "/Subtype /Form /BBox [0 0 580 620] "
                    f"/Resources << /Font << /T3 {font} 0 R >> >>",
                    stream,
                )
                annotation = object_ref(
                    "/Type /Annot /Subtype /Square /F 4 /Rect [0 0 580 620] "
                    f"/AP << /N {appearance} >>",
                )
                pdf.xref_set_key(page.xref, "Annots", f"[{annotation}]")
                stream = b""
            else:
                pdf.xref_set_key(resources, "Font/T3", f"{font} 0 R")
        pdf.update_stream(page.get_contents()[0], stream)
        page.set_cropbox((31, 27, 529, 573))
        page.set_rotation(rotation)
        pdf.save(path)


def trace(path):
    with pymupdf.open(path) as pdf:
        page = pdf[0]
        return [(kind, pymupdf.Rect(box) * page.rotation_matrix) for kind, box in page.get_bboxlog()], page.rect


def verify(rust, matter, source, reference, args, refused):
    original = source.read_bytes()
    original_programs = type3_program_streams(original)
    reference.write_bytes(original)
    document = rust("IMPORT", matter, source=str(source), status=201)[0]["id"]
    baseline = pdf_bytes(rust, document, 1)
    route = f"/documents/{document}/pages/text-overlay"
    try:
        add_text_overlay(reference, **args)
    except ProcessingError as exc:
        assert refused and str(exc) == "Text overlaps existing page content. Choose a clear area.", (source.name, args, exc)
        assert rust("POST", route, args, status=None) == {"status": 422, "body": {"detail": str(exc)}}, (source.name, args)
        assert pdf_bytes(rust, document, 1) == baseline
    else:
        assert not refused, (source.name, args)
        rust("POST", route, args, status=200)
        actual = pdf_bytes(rust, document, 2)
        with pypdfium2.PdfDocument(reference.read_bytes()) as left, pypdfium2.PdfDocument(actual) as right:
            a, b = (pdf[0].render(scale=2).to_pil().convert("RGB") for pdf in (left, right))
            assert ImageChops.difference(a, b).getbbox() is None, (source.name, args)
        assert type3_program_streams(actual) == original_programs
    assert pdf_bytes(rust, document, 1, "original") == original


def main():
    cases = {
        "clipped-shade": "620 0 d0 170 145 110 190 re W n /Axis sh",
        "bounded-axial": "620 0 0 -100 620 820 d1 /Bounded sh",
        "bounded-radial": "620 0 d0 /Radial sh",
        "function-shade": "620 0 d0 /Function sh",
        "mesh-shade": "620 0 d0 /Mesh sh",
        "form-shade": "620 0 d0 /Form Do",
        "transparent-group": "620 0 d0 /Group Do",
        "direct-group": "620 0 d0 30 90 70 100 re f /Group Do",
        "stroke-gstate": "620 0 d0 /Setup gs .3 G 130 105 135 165 re S",
        "alpha-gstate": "620 0 d0 /Alpha gs .3 g 130 105 135 165 re f",
        "zero-alpha-shade": "620 0 d0 /Zero gs /Axis sh",
        "unbounded-shade": "620 0 d0 /Axis sh",
    }
    count = 0
    with TemporaryDirectory(prefix="matterdraft-type3-shading-") as directory:
        root = Path(directory)
        with running_vault(root / "vault") as rust:
            rust("POST", "/vault/setup", {"password": PASSWORD})
            matter = rust("POST", "/matters", {"name": "Synthetic Type 3 shader placement"}, status=201)["id"]
            reference = root / "reference.pdf"
            for rotation in (0, 90, 180, 270):
                args = dict(page_index=0, x=.23, y=.24, font_size=10, text="I")
                source_pdf(reference, rotation, None)
                add_text_overlay(reference, **args)
                ((_, ink),), page = trace(reference)
                left, top, right, bottom = ink + (-.23 * page.width, -.24 * page.height,
                                                  -.23 * page.width, -.24 * page.height)
                reference.unlink()
                for label, program in cases.items():
                    source = root / f"{label}-{rotation}.pdf"
                    source_pdf(source, rotation, program)
                    events, _ = trace(source)
                    obstacles = [box for kind, box in events if kind in ("fill-text", "fill-image")]
                    assert len(obstacles) == 1, (label, events)
                    box = obstacles[0]
                    if box.width > page.width:
                        placements = [(.23, .24, False), (.72, .11, False)]
                    else:
                        x = (box.x0 + box.x1 - left - right) / 2
                        y = (box.y0 + box.y1 - top - bottom) / 2
                        placements = [(px / page.width, py / page.height, refused)
                                      for refused, margin in ((True, .08), (False, -.08))
                                      for px, py in ((box.x0 - right + margin, y),
                                                     (x, box.y1 - top - margin))]
                    for x, y, refused in placements:
                        verify(rust, matter, source, reference, dict(args, x=x, y=y), refused)
                        count += 1
                for name, refused in (("bounded-axial", True), ("unbounded-shade", False)):
                    source = root / f"appearance-{name}-{rotation}.pdf"
                    source_pdf(source, rotation, cases[name], in_appearance=True)
                    verify(rust, matter, source, reference, args, refused)
                    count += 1
    print(f"{count} synthetic Type 3 working-copy placement comparisons passed")


if __name__ == "__main__":
    main()
