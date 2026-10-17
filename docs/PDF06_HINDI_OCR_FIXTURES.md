# PDF-06 Hindi OCR fixture record

**Scope:** Synthetic Hindi (Devanagari) and English (Latin) text only. These samples are not legal documents. Generate them with `backend/.venv312/bin/python backend/scripts/generate_hindi_ocr_fixtures.py` on a machine with a Devanagari font, or use the checked-in PDFs. The image quality in `07` is deliberately degraded in software; it is not output from a tested scanner model.

| Fixture | Expected visible text and order | Page geometry/orientation | Acceptable OCR result |
| --- | --- | --- | --- |
| `samples/05_hindi_scan.pdf` | One image-only page: `हिंदी परीक्षण दस्तावेज`, then `CASE 2026` | 595 × 842 pt, 0° | Both phrases searchable; whitespace between Hindi words may be absent; no substituted or missing letters/digits in these two short lines. |
| `samples/06_hindi_mixed.pdf` | Page 1 native `Native Header 2026`, followed by the same scanned Hindi and English lines; page 2 native `Native page two` | Both 595 × 842 pt; page 1 at 90°, page 2 at 0° | Each phrase searchable on its original page; native header occurs once; same whitespace allowance. |
| `samples/07_hindi_poor_scan.pdf` | One low-contrast, blurred JPEG page with the same scanned Hindi and English lines | 595 × 842 pt, 0° | Same short-line threshold as `05` for this controlled fixture; no general poor-scan accuracy claim. |

For all three, imported original bytes must remain identical, and OCR must publish a separate working revision. The tests also verify a Hindi search hit. OCR text is unverified and must be reviewed against the visible page.

On the development Mac, Tesseract 5.5.3 with `eng.traineddata` SHA-256 `7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2` and `hin.traineddata` SHA-256 `4c73ffc59d497c186b19d1e90f5d721d678ea6b2e277b719bee4e2af12271825` recognized both lines in each sample with Hindi word spaces collapsed. These hashes identify the local test models; the build script currently bundles whatever local models it finds and does not pin or record their versions with each revision.

**Open acceptance work:** Choose and test actual scanner models and import routes on target macOS and Windows devices; supply real scanner output and varied poor scans; measure character/word error and missed regions on reviewed reference text; decide per-page language selection and OCR metadata persistence; evaluate pages with partial pre-existing OCR. Do not mark PDF-06 complete from these synthetic checks.
