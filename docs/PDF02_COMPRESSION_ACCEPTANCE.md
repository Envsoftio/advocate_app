# PDF-02 compression acceptance

**Accepted engineering scope:** the existing lossless, Balanced (240 DPI/JPEG 85)
and Smaller (150 DPI/JPEG 70) working-copy presets for PDF inputs. This is a
synthetic technical baseline, not a guarantee that an arbitrary filing will meet
a portal limit or remain suitable for filing.

## Reproducible fixture

`backend/scripts/pdf02_compression_acceptance.py` creates a three-page, 300 DPI
raster PDF without real matter data:

1. a lightly noisy office scan containing dates, amounts, rules and body text;
2. a red circular mark labelled **SYNTHETIC SEAL / NOT VERIFIED** over grey text;
3. a 40-row table containing approximately 5–5.8 pt raster text, ambiguous
   character groups and fine rules.

The command produces the source PDF, one PDF per preset, side-by-side PNG review
sheets and `metrics.json`:

```bash
cd backend
.venv312/bin/python scripts/pdf02_compression_acceptance.py \
  --output ../artifacts/pdf02-compression
```

The pytest regression uses the same generator without retaining its temporary
artifacts:

```bash
cd backend
.venv312/bin/python -m pytest -q tests/test_compression_quality.py
```

## Budgets

All comparisons render source and result at 300 DPI. PSNR measures the selected
page region. Ink recall and contrast retention use the source dark-pixel mask;
red precision/recall measures retention of the seal's colour/shape. Whole-page
PSNR is intentionally not the sole gate because removing harmless scanner grain
can lower it while leaving the document more readable.

| Preset | Maximum result/source bytes | Scan floor: PSNR / ink / contrast | Seal floor: PSNR / red recall / precision | Small-text floor: PSNR / ink / contrast |
| --- | ---: | --- | --- | --- |
| Lossless | 100% | 99 dB | 99 dB | 99 dB |
| Balanced | 20% | 28 dB / 97% / 75% | 27 dB / 94% / 88% | 25.8 dB / 97% / 70% |
| Smaller | 8% | 24 dB / 88% / 65% | 23 dB / 86% / 78% | 21.5 dB / 78% / 58% |

These thresholds are regression floors for this fixed corpus. They are not
universal visual-quality percentages and must not be shown as legal or signature
verification.

## Authoring-machine result — 16 October 2026

Environment: macOS arm64, Python 3.12.13, PyMuPDF 1.28.2, Pillow from the checked
development environment. The generated source was 20,596,771 bytes.

| Preset | Result bytes / ratio | Scan: PSNR / ink / contrast | Seal: PSNR / red recall / precision | Small text: PSNR / ink / contrast |
| --- | ---: | --- | --- | --- |
| Lossless | 20,596,771 / 100% | 99 / 100% / 100% | 99 / 100% / 100% | 99 / 100% / 100% |
| Balanced | 1,913,963 / 9.29% | 29.52 / 99.99% / 81.78% | 28.36 / 98.66% / 93.70% | 26.93 / 100% / 77.47% |
| Smaller | 442,236 / 2.15% | 25.45 / 95.59% / 73.33% | 24.22 / 94.15% / 88.58% | 22.72 / 87.08% / 66.37% |

The six lossy side-by-side sheets were visually inspected. Dates and amounts on
the scan, the seal rings/letters, every table row and the deliberately ambiguous
small-text groups remained distinguishable. The Smaller preset was visibly
softer, as expected, and still requires case-specific review before use.

## What this closes—and what it does not

This corpus closes PDF-02's missing measured scan/seal/small-text acceptance.
Existing regression coverage separately verifies native searchable text, internal
links, outlines, immutable originals, revision history, Undo, cancellation and
the honest “no smaller result” path. Lossy compression still has no target-size
or universal fidelity guarantee. A visible seal is not evidence of authenticity,
and compression may affect signed-document protections; PDF-11 acknowledgment
continues to apply.

Supported native Windows/macOS UI and device qualification remains tracked under
NAT-01/NAT-02/NAT-03 rather than being inferred from this backend corpus.

Verification after adding the corpus: backend **123 passed** with six dependency
deprecation warnings; Vue production build passed with its existing chunk-size
warning; Vitest **9 passed**. The focused compression/PDF reliability run passed
**31 tests**.
