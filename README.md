# CourtBundle POC — Indian Court Filing Preparation

**Status:** working PDF/bundling POC. The Vue browser journey, an unsigned Apple Silicon package, native OCR and native Save were exercised locally on 13 October 2026; an isolated native Hindi OCR run followed on 18 October. Desktop foundation features added on 24 October have browser and package checks recorded in `docs/POC_ACCEPTANCE_TESTS.md`; native print and new Office Save dialogs still need target-device verification. Clean-device installation, macOS 12 minimum, macOS Intel and Windows x64 remain unverified. Do not use this POC to submit real confidential case files or represent it as court-approved.

This is a local-first desktop application prototype using **Tauri 2 + Vue 3 + TypeScript + Python FastAPI + PyMuPDF + integrated Tesseract OCR**.

The [project context and requirements](CourtBundle_Project_Context_and_Handoff.md) define five approved improvements: batch document review, output presets, reviewed import cleanup, rebuild/export review and a source-linked chronology. The editable batch-review table is engineering-implemented with native acceptance still deferred; status remains authoritative in the [backlog](docs/IMPLEMENTATION_TASKS.md).

The proof-of-concept workflow is:

`Create matter -> Import PDF/images -> Inspect/edit working copies -> OCR -> Classify/group/drag to order -> Configure volumes -> Generate index/bookmarks -> Validate -> Export combined PDF + volumes + CSV + ZIP`

The desktop foundation includes **Open** through the local file picker and portable `.cbmatter` restore; **Save** for document and bundle settings, **Save As** for a working PDF or completed build files, and idle **Autosave** for those settings. Autosave reports failure and does not replace an explicit exported PDF. **Storage** shows active and archived documents, revisions, saved builds and backup/restore actions. The existing PDF reader supplies zoom, page/text search, and matter Undo/Redo; the sidebar supplies theme and interface size controls. Completed builds can be exported as PDFs, ZIP and CSV, plus editable **DOCX text draft**, **XLSX index**, and **PPTX text draft**. The Office drafts derive from the completed PDF/index, are labelled for review, and do not preserve page layout, images or verified signatures. Scanned PDFs need reviewed OCR text before a text draft can be made. **Print** prepares selected pages from a working PDF or completed build with source, A4 portrait or A4 landscape paper and fit/actual-size settings, then offers the system print dialog from the preview. Resolved PDF marks are included and private review notes omitted. The owner deferred online share-link generation; no hosted or revocable link service exists.

To find these controls, open a matter's **Overview → Desktop file tools**. **PDF tools & print** opens the working document, where **Save copy** and **Print** sit beside its title. **Local storage** opens the storage panel. **Office exports** jumps to a completed build's Word, Excel and PowerPoint actions; build a bundle first if the matter has no completed package. A rebuilt desktop `.app` must be quit and reopened before its UI changes appear in an already running instance.

**Bundle membership** explicitly selects which documents enter the primary bundle. New uploads start as **reference-only** and remain readable/editable; use the status-bar membership action (or the inclusion summary in Index/Build), select documents, and **Apply membership**. Cancel leaves the saved selection unchanged. Existing matters keep their previous effective selection through migration. Exclusion retains originals and working revisions, invalidates dependent output, and never edits a previous build. Internal links to excluded destinations must be resolved before building; excluded manual bookmarks are omitted with a warning. See [B01A implementation and visual evidence](docs/DESIGN_PHASEB01A_IMPLEMENTATION.md).

**Build, review and export** now use the supplied B01B JSON design: settings, actual job progress/failure/recovery, persisted generated files, read-only PDF review, export, stale-output choices and Storage retrieval. Presets/numbering/volume tools remain available from the new settings cards; Office/print/protected-copy tools sit under **More output tools**. New-build freshness follows effective included inputs; reference-only edits do not stale those artifacts. See [B01B implementation, visual comparisons and native Save evidence](docs/DESIGN_PHASEB01B_IMPLEMENTATION.md). B01C now adds the new BU07 output-checks dialog, 14C Preflight workspace and ED34 exact-finding panel. Actual artifact checks and separate persisted human review control reviewed release; technically valid review copies remain explicitly unreleased. See [B01C design, release policy and evidence](docs/DESIGN_PHASEB01C_IMPLEMENTATION.md). B02A now replaces the preset and index/cover workspace with the supplied BU01/BU02 design, persistent field overrides and actual generated PDF previews. See [B02A implementation, visual checks and native Save evidence](docs/DESIGN_PHASEB02A_IMPLEMENTATION.md). B02B now implements the supplied BU03 numbering/navigation, 11C page map and 12A bookmark editor, using actual PDF previews and the shared engine map. See [B02B implementation, visual checks and native export reconciliation](docs/DESIGN_PHASEB02B_IMPLEMENTATION.md). B02C now implements the supplied 06A batch-review grid and 06D source-backed title/date review, with explicit field acceptance, atomic history, stale-result protection and optional group-only sorting. See [B02C design, acceptance checks and native evidence](docs/DESIGN_PHASEB02C_IMPLEMENTATION.md). B03A now implements the supplied BU04 output-options layout with saved processing choices, selected Office drafts and actual measured output sizes. See [B03A implementation, design comparisons and verification](docs/DESIGN_PHASEB03A_IMPLEMENTATION.md). B03B is next; it has not started.

The redesigned **Documents** rail opens the preparation workspace: a Part navigator, PDF preview and document details. **Open editor** keeps the same document and page while opening the full PDF tools; **Documents** returns to preparation. At compact widths the source list and details collapse into explicit panels. The reader uses the supplied JSON layout, including a right-side information panel and focused reading with floating page controls. See the [JSON design audit and verification](docs/DESIGN_PHASE02E_JSON_FIDELITY.md).

Use **Matters** in the sidebar to search active matters by name, case number or party and sort by name or recent update. Opening a card shows **Overview**, with case details, the latest package and shortcuts to each preparation step. Sidebar matter shortcuts continue to open Documents directly. The PDF reader identifies the working document and groups tools by task. **Full screen** expands the editor across the app window, hiding matter navigation and initially hiding the source list. The page thumbnail rail renders only the visible rows and supports keyboard navigation/reordering. Reopen the list inside the editor as needed; **Exit full screen** or Escape restores the workspace. **Focus on PDF** separately toggles the source list.

The reader offers **Single page** and **Continuous** modes. New documents open at **Fit page**, and entering **Full screen** fits the page even when an older reading position stored another zoom; **Fit width** and numeric zoom remain available afterward. Opening an editing tool from Fit page changes to Fit width so the page and its handles remain usable; numeric zoom choices are retained. At 100%, 96 CSS pixels represent 72 PDF points. Reading positions are saved in the local store per document revision and restored after reopening, and **Back** returns after a page, outline, link or search jump. Position saves follow a short idle delay; an immediate forced quit can lose the latest navigation change. Search lists individual occurrences and highlights the active one, including text split across PDF runs. **PDF outline** shows navigation embedded in the source PDF; **Comments** opens a searchable, scrollable side panel of private review notes without reducing PDF height. A compact page-bookmark control opens the manual bookmarks on the current page and links to **Index & bookmarks** for destination review. Editing uses a focused single page and returns to the selected reading mode when you choose Select. **Save copy** downloads the selected working revision with current editable PDF marks flattened into it; private review notes are omitted. When there are no editable marks, it saves the unchanged working bytes.

The matter workspace provides one preparation path through Documents, Index & bookmarks, Preflight checks and Build bundle. **Open sample matter** creates a separate synthetic draft matter; it is not an approved court example.

Index & bookmarks summarizes the current technical Part 1–4 assignments. Build bundle can compare a selected completed export with the current draft page plan, including document and label changes, shifted volume/combined ranges, changed settings and unresolved bookmarks. Older builds without a recorded page-map snapshot say comparison is unavailable. This does not define required Part contents or repair citations in source documents.

For a court-neutral matter, choose the **Court-neutral preparation** profile. In **Index & bookmarks**, a reviewer can import a local CSV/TSV with `title,document,page` columns, resolve each source document/page and explicitly check every row before it becomes a manual bookmark. **Preflight checks** has a user-authored checklist scoped by court, case type and stage; it does not impose or certify required documents. **Build bundle → Cover & sections** offers compact or readable technical indexes and optional source-page-only Part PDFs. The readable index changes index page counts in the shared page map; Part PDFs omit volume indexes and separators. Combined PDF and indexed volumes remain the main outputs. These choices do not claim to meet any court's filing directions.

In the PDF viewer, **Select** permits native PDF text selection. Tools are grouped under **Edit**, **Annotate**, **Stamps**, **Pages** and **Output**. **Add text** creates an editable text object at the clicked position. **Annotate** has independent Highlight, Comment, Draw, Box, Ellipse, Line and Arrow controls; **Stamps** has image, Check, Cross and Date controls. Each object can be selected, dragged, resized and deleted with Delete/Backspace. Draw, shapes, added text, image and visual stamps are flattened into Save copy and new bundle exports. Highlight and Comment remain private review content and are omitted. **Redact** creates an editable black mark; drag or resize it after adding it. Save copy and new bundles permanently remove covered PDF content in those generated files. The underlying working revision and imported original still contain that content, so never share the raw working PDF as a redacted copy. Existing destructive redaction records remain restorable through the legacy workflow. **Links** are separate revision-anchored objects. In **Edit text**, moving or replacing an existing PDF text run remains a working-copy revision edit; it is not an independently movable object afterward. See [trial scope and limits](docs/PDFIUM_PARAGRAPH_TRIAL.md).

On the matter's **Documents** page, use **Bookmark page** in the PDF viewer to mark the page you are reading. Drag document rows or use their arrow buttons to set index order; open thumbnails to drag pages within a working copy. The adjacent **Index & bookmarks** tab repeats document ordering beside the page map, offers a bookmark shortcut on every index row, and lets you rename, nest, reorder or resolve manual bookmarks. Drag bookmarks among siblings or use their arrow buttons. The preview distinguishes printed volume pages from physical combined PDF pages and shows each bookmark's source page. Build opens its read-only page plan through **Preview** on the settings card. After building, **Review bundle** or a PDF file row opens a generated PDF inside the app using authenticated range loading and the same reader, with continuous/single-page modes, occurrence search, selectable text, bookmarks and index-link navigation. The review identifies its build and flags older output when matter inputs have changed. **Shortcuts** lists keyboard controls in both readers. The two index densities and per-volume printed numbering are technical POC choices, not reviewed filing rules. Devanagari index titles use shaped fallback fonts but require visual review in saved PDFs; Hindi text extraction is not qualified. Other scripts block generated output until qualified.

In the PDF viewer, **Pages → Add pages** opens the insertion task directly; choose pages from another document in the matter or duplicate pages from the current one. **Delete page** removes the current page after confirmation, while **Organize pages** selects, moves or deletes several pages. Drag page thumbnails to reorder, or use the organizer's arrow buttons and **Apply page order**. These actions change the editable working copy; the imported original remains unchanged. **Page tools** presents the remaining tasks one at a time: combine PDFs, replace selected pages or the whole working PDF, crop margins, save selected pages, split into PDFs, or reduce file size. Each task explains its purpose and shows a live outcome summary before the primary action. Add/combine/replace show actual source pages and destination neighbors or replacement mappings; source and target revision checks prevent applying an outdated preview. Page ranges refer to source-document pages, not printed bundle numbers. Extract/split save separate files; other tasks update the working copy. Crop margins previews the keep area on the actual working page, including rotation; drag a rectangle or adjust the unrotated-edge percentages. Crop is not secure redaction. **Pages → Organize pages** opens a thumbnail grid with explicit selection, Shift ranges, keyboard/drag ordering and confirmed batch rotation/deletion; each applied operation is one undoable working-copy revision.

**Redact** in the PDF viewer adds an editable black mark. It can be moved, resized and deleted before output. Save copy and new bundle exports apply PDF redaction to those marks, remove covered text and image pixels, and rebuild the saved document without source metadata, attachments, outlines, links, annotations or widgets. The raw working revision remains unchanged and can still expose the content through search or extraction; do not share it as a redacted PDF. Previously committed destructive redactions remain in working revisions and can be selectively restored through their recorded areas when the page structure still matches. Imported originals, Undo history, prior exports and portable backups can also retain material. Review document and matter fields separately before sharing; native security acceptance remains open.

Use **Open table** on Documents to review filename, display title, label, document date/provenance, type, Part, group, page count and processing status together. Selection is explicit and remains fixed while filtering. Type/Part/group batch changes are one undoable action. Upload, filename and document-date sorts show a cancelable preview before changing order; unknown and ambiguous dates remain explicit and sort last with stable ties. File modification time is shown only as source metadata and never becomes a document date. Duplicate/gapped labels are warnings, not silently corrected.

The same table can preview policy-neutral label mappings for an explicit selection: user-chosen sequential values, configurable grouped suffixes over existing group keys, or conservative filename candidates. A group may designate one technical parent, including a parent that is not first in document order. Group formatting supports lower/upper-case letters, numeric or parenthesized suffixes, a custom separator, and an option for the designated parent to retain the base label. Reviewers choose whether a new proposal compacts the selected sequence or preserves existing labels for manual completion. The preview remains editable and cancelable. Applying it requires review of existing-label changes, searchable-text citation candidates and prior generated builds; it is one undoable transaction. Prior artifacts and originals remain unchanged. Citation matches show source page and revision but are incomplete text candidates—not verified legal citations—and neither hierarchy nor a selected pattern is represented as a court-approved convention. Reusable named court/chamber presets remain scoped under FIL-09.

The Build workspace can save versioned technical output presets from current volume settings and persisted output marks, then explicitly apply a version to a matter. Applied versions are recorded in new export manifests and portable matter backups. A matter can set a separate Bates sequence with prefix, start, padding, placement, index/separator inclusion and per-volume restart, plus annexure label stamps or technical separator sheets with per-document exceptions. The shared page map drives preview, generated PDF, outlines, index links, CSV and combined offsets. Covers remain ordinary source documents in the current technical layout; reviewed filing-specific cover and index layouts remain open under FIL-09/DEC-02. The PDF viewer can also author explicitly reviewed internal or `http`/`https`/`mailto` links and preview matter-wide output watermarks, build timestamps, Bates identifiers and annexure labels on the actual page. Internal destinations are tied to a document revision and resolved through the deterministic page map: later page edits leave the link visibly unresolved instead of silently retargeting it. Combined exports retain resolved cross-document links; a standalone volume retains only destinations available inside that volume and reports omitted cross-volume links. Source-page marks warn when their configured area overlaps existing content, and the build records mark inputs and collision results.

The Delhi High Court profile also includes a source-linked preparation guide,
official form/template links, explicit handling of the 2026 Rule 3.4 size
amendment, and a Memo of Parties CSV review helper. The helper does not replace
the court's official XLS template or an advocate's case-specific review.

## Repository layout

```text
CourtBundle_POC/
  backend/
    app/
      main.py         # FastAPI endpoints, local bearer token
      storage.py      # SQLite metadata, filesystem originals and working copies
      pdf_ops.py      # PDF import/page operations, OCR, draft cover
      engine.py       # deterministic bundling, index, links, volumes, checks
      jobs.py         # queued jobs, progress and cancellation
      rules/profiles.json # source-linked ILLUSTRATIVE court rules
    launcher.py       # local API / packaged sidecar entry point
    scripts/
      generate_demo_docs.py
      build_demo_output.py
      build_sidecar.py
    tests/             # pytest integration coverage
  frontend/
    src/               # Vue 3 interface with PDF.js preview
    src-tauri/         # Rust Tauri desktop shell, native icons, capabilities
    package.json
  samples/             # synthetic PDF/JPEG sources and verified output package
  docs/
    PROJECT_CONTEXT_AND_HANDOFF.md # pointer to the root handoff
    POC_ACCEPTANCE_TESTS.md
```

## Requirements

- Approved first-phase targets: Windows 10/11 x64 and macOS 12+ on Intel and Apple Silicon. Each combination still requires native build and device verification.
- Python 3.11–3.13 with an environment supporting PyMuPDF; Node.js >=20; npm; Rust toolchain (for Tauri); system toolchains required by Tauri.
- **Development OCR:** Tesseract English and Hindi traineddata (`eng.traineddata`, `hin.traineddata`); install Tesseract and both models as appropriate for your OS.
- Internet **only for installing development dependencies**; all application processing is local after dependencies are installed.

## Quick start: browser-based frontend development

### Terminal 1 — backend

**macOS / Linux**

```bash
cd backend
python3 -m venv .venv
source .venv/bin/activate
python -m pip install -r requirements-dev.txt
./dev.sh
```

**Windows PowerShell**

```powershell
cd backend
py -m venv .venv
.\.venv\Scripts\Activate.ps1
python -m pip install -r requirements-dev.txt
$env:COURTBUNDLE_API_TOKEN = 'development-token-change-me'
python launcher.py --port 8765
```

The backend listens on `127.0.0.1:8765`, requires a bearer token for every API route, and saves data to `~/.courtbundle-poc` by default. This fixed token is for local *development only*. The Tauri shell creates a random token at launch.

### Terminal 2 — frontend

```bash
cd frontend
npm install
npm run dev
```

Open **http://127.0.0.1:1420**. Browser development uses `frontend/.env.development` (local test token). The native Tauri app does **not** use this fixed token.

Create a matter, import the files in `samples/`, run OCR on `04_scanned_annexure.jpg`, set part/type/labels, configure a 6-page volume cap under **Build bundle**, and download the result.

## Automated Python tests

```bash
cd backend
python -m pytest -q
```

The included integration tests cover authentication, matter creation, PDF import, page reordering/rotation/deletion, cover generation, preflight, OCR, volume splitting, export-size failure, original-byte integrity, annotation/link export policy, group contiguity, PDF outlines/index links and ZIP export. Use the current pytest result; the 15-test count was the baseline before QA-02 additions on 13 October 2026.

Browser E2E coverage uses an isolated temporary data directory and synthetic samples:

```bash
cd frontend
npm run test:e2e
```

Set `COURTBUNDLE_TEST_PYTHON` to a Python with backend requirements if neither `backend/.venv312` nor `backend/.venv` exists. The test uses installed Google Chrome and starts local servers on ports 8768 and 1420. It does not verify Tauri native behavior.

To rebuild the demo export from purely synthetic files:

```bash
cd backend
python scripts/build_demo_output.py
```

Results appear in `samples/example_output/`. `combined.pdf` and `filing-package.zip` were generated and inspected; `filing-package.zip` includes all volume PDFs, CSV index, JSON validation report and a warning note.

## Tauri desktop: development and packaging

For native development with Vue, Rust and Python hot reload, run this single
command from the repository root:

```bash
./dev.sh
```

The script uses `backend/.venv312` when available, installs missing development
dependencies, starts FastAPI from source with Uvicorn reload, and starts Tauri
with Vite hot module replacement. Development mode deliberately does not build
or launch the frozen PyInstaller sidecar. Stop both processes with `Ctrl+C`.

The packaged application still requires the platform-specific sidecar below.

Tauri CLI expects `frontend/src-tauri/`. First make a native Python executable for your target host:

```bash
# backend virtual environment must have requirements-dev.txt installed,
# including PyInstaller; Rust/rustc must be on PATH.
python backend/scripts/build_sidecar.py
```

**Run from the repository root**. The script detects the local Rust host target and copies a correctly suffixed executable into `frontend/src-tauri/binaries/`. It bundles English and Hindi Tesseract traineddata alongside the Python API. Set `TESSDATA_SOURCE` to a tessdata directory containing `eng.traineddata` and `hin.traineddata` if it cannot find both models automatically. See [Hindi OCR fixtures](docs/PDF06_HINDI_OCR_FIXTURES.md) for the synthetic test scope.

Next:

```bash
cd frontend
npm install
npm run tauri:dev
# To build a distributable for this host:
npm run tauri:build
```

The Tauri desktop process starts the bundled sidecar on a random loopback port, passes a random per-session token via environment, stores matter files in the platform-specific app data directory, and stops the child when the app exits. A platform-native sidecar **must be built on that target platform** (or via an appropriate CI runner). Neither a signed/notarized macOS build nor a Windows installer is included in this source ZIP.

### Supported builds need device QA

- **Windows:** test WebView2, PDF.js, native sidecar spawn, OCR traineddata, download/export and installer.
- **macOS Intel / Apple Silicon:** build the matching Python sidecar for each architecture, test WKWebView canvas/PDF.js, permissions and app termination. Signing/notarization needed for production distribution.
- **Linux:** Deferred from the approved first-phase targets; no Linux build is verified here.

## API summary

All routes require `Authorization: Bearer <local-token>`.

- `GET /health`, `GET /profiles`
- `GET|POST /matters`; `GET|PATCH|DELETE /matters/{id}`
- `GET /matters/{id}/portable`; `POST /matters/restore` (versioned local matter backup/restore; protected mode streams `application/octet-stream`)
- `GET /matters/{id}/history`; `POST /matters/{id}/history/undo|redo`
- `GET|PUT /matters/{id}/bookmarks` (revision-aware manual outline entries)
- `GET|PUT /matters/{id}/links` (reviewed revision-aware internal/external links)
- `GET /matters/{id}/annotations`; `POST /documents/{id}/annotations`; `POST /documents/{id}/annotations/image`; `PATCH|DELETE /annotations/{id}`; `GET /annotations/{id}/asset`
- `GET|PUT /matters/{id}/output-marks` (watermark and build-timestamp settings)
- `GET /matters/archived`; `POST /matters/{id}/restore`; `GET /matters/{id}/archived-documents|storage|audit`
- `GET /matters/{id}/documents`; `POST /matters/{id}/documents/raw` (streamed file with `X-CourtBundle-Filename`); legacy multipart `POST /matters/{id}/documents` is for plaintext development mode; `POST /matters/{id}/cover`
- `GET|PUT /matters/{id}/memo-parties` (matter-scoped review data)
- `GET|PUT /matters/{id}/values` (matter-scoped reusable draft values; no filing template consumes them yet)
- `POST /matters/{id}/documents/reorder`
- `PATCH /matters/{id}/documents/batch`; `POST /matters/{id}/documents/sort-preview`
- `PATCH|DELETE /documents/{id}`; `GET /documents/{id}/pdf`
- `POST /documents/{id}/pages/reorder|rotate|delete`; `POST /documents/{id}/ocr`
- `POST /documents/{id}/pages/{pageIndex}/output-marks-preview`
- `POST /documents/{id}/pages/redact` (reviewed rectangle on a working-copy revision)
- `POST /documents/{id}/replace` (whole working-copy replacement from another matter document)
- `GET /matters/{id}/validate`; `POST /matters/{id}/bundle-preview`; `POST /matters/{id}/build`
- `GET /jobs/{id}`; `POST /jobs/{id}/cancel`
- `GET /matters/{id}/exports`; `GET /matters/{id}/exports/{exportId}/{filename}`

Full request/response examples and architecture notes are in `CourtBundle_Project_Context_and_Handoff.md`.

## POC boundaries — important

- Delhi High Court is represented by an **illustrative technical subset**, not a comprehensive court profile. The configurable page cap is **not** a national court rule. The 2026 Delhi High Court rule concerns **300 MB file size**, not a 300-page maximum.
- This tool prepares PDFs; it does not e-file documents with courts or establish legal validity, admissibility, certification, signing, translation accuracy, limitation or filing acceptance.
- Imported originals are preserved. PDF imports are inspected conservatively for certificate-signature dictionary indicators before the initial working copy is created byte-for-byte. The UI distinguishes detected indicators, reviewed unsigned inputs, reviewer-marked visible/scanned signatures and inconclusive inspection, and requires acknowledgment before risky transformations. This is not cryptographic validation: working-copy changes, OCR, extraction, compression and bundle generation can invalidate or omit signature protections or alter context. Do not use this POC as an evidence-preservation tool without review.
- OCR uses English and Hindi traineddata and adds an invisible layer to image-only and mixed native/scanned pages while retaining visible source content, geometry and navigation. Pages with an existing GlyphLessFont OCR layer are skipped; OCR rendering is bounded to 40 megapixels per page. OCR is fallible, particularly for handwriting and seals. Non-Latin index titles are not yet properly rendered.
- PDF and JPG/PNG import only; Word, Excel and PowerPoint conversion remain future development. On working-copy pages, including pages rotated by 90°, 180° or 270°, the preview can add visible ASCII text. It can replace one selectable horizontal text run even when the PDF page has a 90°, 180° or 270° rotation setting. Replacement can use clear space to the right and wrap into clear space below, reducing font size by at most 25% if needed; it does not move surrounding page content, preserve arbitrary fonts or correct scanned pixels. Edit text mode can also move one selectable ASCII text run horizontally and vertically into clear space by dragging its outline and applying the preview. Whole-paragraph reflow is separately available only for PDFs supported by the PDFium trial. Neither mode moves scanned pixels or silently shifts other text. Unfittable or genuinely overlapping edits are rejected. Review the result and use matter Undo to revert it. This is not a confidential redaction tool.
- Browser export uses authenticated fetch and browser-style download. Tauri export uses a native Save dialog and streams to a temporary file before replacing the destination. Apple Silicon native Save has local device evidence; validate save and failure behaviour on every target system.
- Job state and progress persist in SQLite. After a restart, unfinished jobs are shown as interrupted and can be retried from a new job; abandoned build staging is removed. Working-copy revisions and a process-wide processing lock keep PDF requests and jobs from running concurrently. Late cancellation cannot relabel a committed output as cancelled. Restart recovery recognizes a published build only after verifying its artifact hashes. Large-job cancellation and native crash recovery still need device testing.
- Manual bookmarks persist with document/page/revision targets. A later edit or archive leaves an affected bookmark visible as unresolved until reviewed; unresolved entries are omitted from generated outlines with a report warning. The v10 portable matter package includes bookmarks, reusable draft values, reviewed links/output marks and page annotations with their image assets, while accepting legacy v1-v9 packages. Build manifests fingerprint matter inputs, current working-copy hashes, manual bookmarks, annotations and the selected profile; older manifests are conservatively marked stale.
- Revision-anchored PNG/JPEG image overlays are flattened into newly generated source-document pages when their exact revision resolves. Highlights and comments are private workspace review data and are excluded from filing-preparation PDFs by default. Validation reports record the policy and counts but not private note bodies or internal asset paths. Originals, working copies and completed exports are not modified by these annotations.
- Reviewed links and output marks are matter-scoped, undoable and portable. Link targets use exact document revisions; unresolved or unavailable destinations are omitted with explicit warnings. Output marks affect newly generated source-document pages only, not imported originals, working copies, indexes or completed exports. Overlap detection is conservative and must still be visually reviewed.
- Memo of Parties helper rows autosave to the selected local matter and show save failures. Document-detail and bundle-setting forms use explicit Save, show unsaved indicators, and prompt before discarding drafts during navigation or reload. A failed bundle-settings save stops the build.
- The packaged desktop app requires a password-protected local vault. Its SQLite database, matter files, internal exports and new `.cbmatter` backups are encrypted at rest. Browser development mode remains plaintext, and explicitly saved PDF/ZIP downloads are plaintext at their chosen destination. Matter/document removal archives records and keeps files; there is no in-app permanent delete. See [local security and retention](docs/LOCAL_SECURITY_AND_RETENTION.md). Use synthetic matters until native macOS and Windows security checks are complete.
- Vue build, Vitest, Apple Silicon Rust compile and local package tests passed on 13 October 2026; see the acceptance log for exact runs. Other target builds and clean-device tests are still required.
- **PyMuPDF licensing:** Determine AGPL/commercial licensing requirements before distributing any proprietary build. Review other dependencies' licenses too.

See [Project Context and Handoff](CourtBundle_Project_Context_and_Handoff.md) and [Acceptance Tests](docs/POC_ACCEPTANCE_TESTS.md).

### PDF reliability hardening — 14 October 2026

Page assembly remaps retained internal links and outlines, including rotated link rectangles/destinations; links to excluded pages are omitted. Generated PDF outlines retain full Unicode titles, while printed index multilingual fonts remain unfinished. Export checks verify exact outline/link destinations against the shared page map, ZIP CRCs and per-artifact SHA-256 values. Downloads reject hash-mismatched new exports.

Page tools offer lossless cleanup, Balanced (240 DPI/JPEG 85), and Smaller (150 DPI/JPEG 70) compression. They report measured sizes, preserve native text and navigation, and create an undoable working revision only if the file gets smaller. Masked, bitonal and images above 40 megapixels are retained. A reproducible [synthetic scan/seal/small-text corpus](docs/PDF02_COMPRESSION_ACCEPTANCE.md) now supplies preset-specific regression budgets and visual-review sheets. Review every lossy result; the corpus is not a size, fidelity, signature-validity or filing-readiness guarantee. Compression jobs interrupted by restart must be restarted explicitly from page tools with a selected preset.

New regression coverage includes write/validation/sync/rename failures, partial-copy cleanup, pending redaction protection, mixed-page OCR, rotated overlays and links, late cancellation, verified build recovery, Unicode/nested bookmarks and index-growth boundaries. See the acceptance log for results and release limitations.
