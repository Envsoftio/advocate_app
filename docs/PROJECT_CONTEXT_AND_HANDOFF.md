# CourtBundle — Project Context, Discussion Handoff, POC Implementation and Development Roadmap

**Prepared:** 12 October 2026  
**Intended audience:** product owner, client, architects, legal-domain reviewer, Codex/development agents  
**Code state (13 October 2026):** Python API and document engine tested; Vue browser build and runtime exercised; unsigned Apple Silicon Tauri app and DMG compiled and exercised locally. Windows, macOS Intel, clean-device installation and minimum macOS 12 remain unverified. See [acceptance evidence](POC_ACCEPTANCE_TESTS.md).  
**Product positioning:** *Local-first Indian court filing preparation, collation and document automation workspace*  
**Important:** This is NOT court-certified software and does NOT file directly with any court.

---

## 1. Why this document exists

The client initially shared a broad desktop application brief proposing Electron + React + TypeScript for the user interface, Python FastAPI as a separate backend process, PyMuPDF for PDF processing, and Tesseract OCR. After discussion, we established that the product is for **Indian advocates preparing court filing documents**, not a generic PDF editor. The POC technology selection was subsequently changed to **Tauri 2 + Vue 3 + TypeScript + Python FastAPI**. On 13 October 2026, the product owner confirmed client approval of Tauri/Vue and Tauri packaging in place of Electron/React and Electron Builder; see DEC-01 in `docs/IMPLEMENTATION_TASKS.md` for the approved platform matrix. Native platform delivery remains unverified.

The agreed development strategy is **prove one deterministic end-to-end legal document bundling journey before attempting a full Acrobat-style editor or comprehensive national court rules engine**.

This document preserves the original scope, the Indian filing research considerations, decisions, implemented architecture, verifiable progress, unresolved requirements, and recommended next stages so that another developer or coding agent can continue from this repository without losing context.

## 2. Original client request: complete conceptual scope

The client described the following desired product:

1. **Desktop foundation:** File Open/Save/Save As/autosave, user storage panel, PDF/Word/Excel/PPT export, print, links/sharing, zoom, search, undo/redo, theme and scaling controls.
2. **PDF engine:** Merge, split, extract, reorder, replace; rotate, crop and delete pages; insert before/after; PDF conversion from/to Office and image types; compression; navigation; OCR-based editable text; annotations, highlights, images and links.
3. **Advanced PDF:** Password protection, permanent redaction, e-signatures, watermarks, timestamps, pagination, scans/OCR, manual and automatic bookmarks, drag/drop bookmarks, and index-derived bookmarks.
4. **Legal document creation:** Matter-based, multi-tab workflow; category/subcategory forms; conditional dropdowns; template-based document generation; structured ZIP output.
5. **Collation and structuring engine (the distinctive complexity):** Four-part workflows (Part 1–4), drag-and-drop document order, OCR, pagination, indexes, smart grouping e.g. 1a/1b/1c, splitting at an approximately 300-page constraint, preserving whole-document boundaries, per-volume indexes, final combined and bookmarked outputs.
6. **Automations:** Index page-number updates, bookmarks generated from index entries, category-driven rule/defect checking.
7. **Architecture:** Offline first, clean UI/backend boundary, secure IPC, progress/cancellation, modularity and maintainability.
8. **Testing/distribution:** pytest for Python, Vitest for TypeScript, Playwright E2E requested initially, native builds for Windows/macOS (Linux mentioned in build scope).

The client asked proposal respondents for prior PDF/Electron experience, actual desktop-app examples, an MVP timeline, monthly retainer and fixed-price estimate.

**Crucial gaps in the initial brief:** The precise Parts 1–4 meaning, legal case types, which courts, the source of the 300-page limit, index schema, which documents belong together, what “editable OCR text” means, valid digital signature workflows, what export conversions must round-trip, and court-specific completeness rules were NOT provided. Avoid filling these gaps with invented law or fictional business rules.

## 3. Domain understanding: Indian court filing preparation

### 3.1 One nationwide filing profile is NOT appropriate

India's Supreme Court, High Courts and District Courts have court-specific and sometimes case/stage-specific filing directions. A preparation application must determine at least:

`Court -> Bench/establishment -> jurisdiction/side -> case type -> procedural stage -> filing route -> rule/template versions`

E.g., a fresh Delhi High Court petition and a Supreme Court final-hearing compilation should not automatically get the same volumes, margins, index structure or annexure policy.

**Domain data belongs in versioned profiles, conditional document checklists, lawyer-approved templates and transparent technical rules**, with source citations, scope, effective dates and last review dates.

### 3.2 The Delhi court limit: 300 MB is NOT 300 pages

The Delhi High Court amendment dated 24 January 2026 sets an e-file **maximum size of 300 MB for the High Court and 20 MB for the District Court**, subject to the detailed published terms and later changes. The notification says it takes effect on Gazette publication and records publication on 27 January 2026.

Sources:
- Official court e-filing hub: https://www.delhihighcourt.nic.in/web/e-filing
- Official amendment landing page: https://delhihighcourt.nic.in/web/no-6rulesdhc-dated-24012026-notification-re-amendment-rule-34-e-filing-rules-high-court-delhi-2021

The **300-page** limit mentioned in the client brief must remain a *configurable client/chamber workflow parameter*, not an asserted universal statutory court rule. Page caps and byte caps are independent. Index pages themselves count toward page limits when such a limit is applied.

### 3.3 Sources of filing complexity

Review these when extending the system beyond the demo:
- Delhi High Court e-filing rules, practice directions, FAQs and common registry objections (official site above). Among technical aspects are searchable documents, bookmarks matching the master index and clean output without unwanted encryption/watermarks/annotations (where applicable).
- Supreme Court publication of forms: https://www.sci.gov.in/forms/
- Supreme Court final-hearing compilation directions from April 2024, which describe differentiated volumes and linked indexes for the proceedings in their scope: https://cdnbbsr.s3waas.gov.in/s3ec0490f1f4972d133619a60c30f3559e/uploads/2024/04/2024040352.pdf
- Supreme Court advocate filing checklist (Jan 2025; different document types and conditional requirements): https://cdnbbsr.s3waas.gov.in/s3ec0490f1f4972d133619a60c30f3559e/uploads/2025/01/2025010980.pdf
- Bombay High Court e-filing rules, 2022: https://cdnbbsr.s3waas.gov.in/s3d0921d442ee91b896ad95059d13df618/uploads/2024/10/202410071417834105.pdf
- Bharatiya Sakshya Adhiniyam 2023, section 63 and its Schedule when relevant to electronic evidence certification: https://indiacode.nic.in/

Those sources should be independently confirmed for *current* version and precise case scope before incorporating them into any profile. A source page can remain live even when a specific rule has been amended.

### 3.4 Distinguish mechanical checks from legal judgments

Mechanically testable: document count, page order, duplicate labels, PDF openability, presence of text layer, generated outline and hyperlink targets, size limits, missing input and bundle/volume constraints.

Reviewer judgment required: whether filing is legally complete, jurisdiction, limitation, appropriate relief, due execution/attestation, source authenticity, adequacy of translations, validity of a signature/evidence certificate, and registry acceptance.

Do not display “court compliant” because some technical rules passed. Instead display a report of passed/flagged technical checks, with explicit “not assessed” legal considerations and human-review steps.

### 3.5 Preserve sources and legal evidence carefully

An imported scanned order or signed instrument is not a Word document. OCR may add an invisible searchable text layer but can misread details; flattening, rasterizing, cropping, reflowing, editing or combining signed material can alter appearance or invalidate a digital signature. Originals and processing copies must remain separate, and exports should be compared visually and procedurally with court-prescribed requirements. This POC preserves originals but is not a forensic evidence-management system.

### 3.6 Important future court-related knowledge

A production rule pack should eventually contain:
- exact court and case/stage applicability
- original pleading formatting (paper, margins, font, spacing)
- category-dependent document and filing checklists
- annexure/evidence numbering and group rules
- index format (columns, page-range conventions, signing)
- pagination convention (logical vs physical page number)
- PDF encoding, size, bookmark/OCR requirements
- signing, execution, attestation and translation workflows
- filing route, service/fee/acknowledgment metadata
- registry defect categories and refiling steps
- version, effective date, source URL, reviewed-by and confidence/approval status.

A rules update should NOT silently mutate already completed export artifacts.

### 3.7 Language and script support plan

Indian filing work can contain several languages and scripts in the same matter or on the same page. Treat these as separate capabilities: displaying imported pages, storing names and titles, rendering generated index/cover text, adding searchable OCR, searching text, editing visible text, and managing translations. Success in one does not establish success in the others.

**Current POC boundary (14 October 2026):** Imported PDF/image pages retain their visible content in working copies and bundles, and PDF outline titles retain Unicode. CSV is written as UTF-8 with a BOM. The printed index uses built-in Latin fonts and replaces unsupported characters with `?`; a `NON_LATIN_INDEX` warning is emitted. The packaged OCR model and UI are English only. Search reads existing PDF text, so a scanned page in another script will not become reliably searchable through the current OCR path. Visible text placement/replacement is limited to printable ASCII. No translation, transliteration, language detection or legal review of translations is implemented. Do not label a multilingual bundle ready for filing on the basis of a successful build.

**Proposed implementation order, subject to target-workflow review:**

1. Select the first court/bench, case type and actual languages/scripts with an advocate; English plus Hindi is a *candidate pilot*, not an approved or comprehensive Indian-language list. Obtain licensed synthetic fixtures including mixed-script names, long titles, native-text PDFs, scans, stamps, tables and pages with more than one script. Record expected text and rendered appearance.
2. Make matter/document metadata Unicode end to end. Store optional document/page language and script hints, including `unknown` and multiple languages. Preserve the entered spelling; never silently translate or transliterate party names, annexure labels or quoted source text. Keep language hints and OCR review state with the working-copy revision.
3. Fix generated output before promising a script: select licensed fonts and a shaping/rendering path, support wrapping and variable-height index rows, and verify embedded glyphs, extractable text, links, bookmarks, CSV and page ranges from the single deterministic page map. The index-page count must be recalculated from actual layout; add page-map regression cases. Until a selected script passes these checks, a bundle requiring that script should fail an explicit readiness check rather than present `?` as usable text.
4. Add offline OCR models for each selected script and allow a user to choose the language(s) per document or page, with mixed-language runs where tested. Record provider/model/version and selected languages. OCR must create a new working-copy revision, keep visible source pixels and imported originals intact, expose uncertain or missing text for review, and never certify handwriting, seals, names or signatures. Compare character/word error and missed regions on reviewed fixtures, not just job success.
5. Test Unicode search on native and OCR text, including combining marks, punctuation, whitespace and script-specific input methods. A later optional transliterated or translated search should be visibly labelled as an aid, never substituted for the original text. Translation is a separate, human-reviewed document workflow; required translation, certification and filing language depend on the exact court and matter scope and cannot be inferred by this app.

**Release evidence:** For each advertised language/script, run synthetic end-to-end import → preview → OCR where applicable → search → index/bookmarks → volume/combined PDF/CSV/ZIP on Windows x64 and both macOS architectures. A reviewer should compare rendered output with the source, check copied/extracted text and every page destination, and record known limits for handwriting, poor scans and mixed scripts. Keep unsupported languages importable as visual source documents, but report unavailable OCR/generated-text capabilities clearly.

## 4. Agreed POC goal, technology and non-goals

**Success demonstration:** an advocate creates a matter, imports synthetic PDFs/scanned documents, edits page order and document metadata, adds OCR, arranges documents into parts/groups, configures a page limit, presses Build, and receives accurate per-volume indexes with hyperlinks and outlines plus combined PDF and ZIP/report. This is an engineering demonstration, not a valid Indian legal filing.

**Chosen stack** (after discussion):
- **Tauri 2** + Rust native shell (lightweight, supports secure commands and bundled sidecar), instead of Electron.
- **Vue 3 + TypeScript + Vite**, Pinia available for later expansion; currently component-local state.
- **PDF.js** to display existing PDF working copies within the UI.
- **FastAPI + Pydantic** sidecar exposed only on loopback and guarded by a per-run bearer token in packaged Tauri mode.
- **PyMuPDF** for normalization/page actions, collation, outlines/index links, OCR integration and PDF export.
- **Tesseract traineddata** for local English OCR. Tesseract is invoked via PyMuPDF integrated OCR rather than a cloud API.
- **SQLite** for matter/document metadata; local original/working/export filesystem storage.
- pytest/Vitest; Tauri-level E2E is future work. Playwright alone does not validate native Tauri sidecar behaviour.

**Why Tauri:** sensible Vue integration and generally lower native-shell overhead than bundling Chromium; Rust orchestration available. **Tradeoff:** platform WebViews differ (Windows WebView2/macOS WKWebView), so PDF.js rendering/downloads must be tested on both. Python/OCR dependencies can dominate installer size, irrespective of UI shell.

**Scope excluded from POC:** full visual OCR editing of PDF text, Office round-trip conversion, universal PDF editing, cropping/replacement of pages, physical scanner driver integration, cloud link sharing, qualified digital signatures, legal AI drafting, real court e-filing, actual template form libraries, nationwide rules, automated legal fee/limitation advice, encrypted-at-rest storage, PDF redaction and guaranteed acceptance.

## 5. Current implemented capabilities

| Area | State | Notes |
| --- | --- | --- |
| Backend local API | Implemented/tested | Token auth; loopback in launcher, controlled CORS. |
| Matter CRUD | Implemented/tested | Name, profile, parties, case type, advocate, case number, volume policy. |
| PDFs and images | Implemented/tested | PDF/JPG/PNG; 80 MB per-file POC cap; preserve original. |
| Working copy page operations | Implemented/tested | Rotate current page, delete and reorder pages. |
| OCR | Implemented/tested on synthetic fixtures | English OCR on image-only/mixed pages; visible content retained. Existing GlyphLessFont OCR pages are skipped. |
| PDF viewer | Browser journey tested; native coverage incomplete | PDF.js canvas, navigation, zoom and page operations. |
| Document ordering | Vue + API code provided | HTML drag/drop and explicit up/down. |
| Document metadata | Implemented/tested | Part 1–4, doc kind, annexure label, keep-together group key. |
| Template sample | Implemented/tested | One synthetic draft cover page; NOT approved legal form. |
| Bundle policy | Implemented/tested | Max pages incl. indexes; optional group cohesion; no document breaks. |
| Index, hyperlinks and outline | Implemented/tested | Clickable index pages, per-volume/combined outlines. |
| Combined PDF + volumes | Implemented/tested | Pages correctly allocated and stamped. |
| Index CSV + ZIP + report | Implemented/tested | Stored and downloaded through authenticated endpoint. |
| Validation | Implemented/tested | Missing originals/working PDF, duplicate hashes/labels, blank text layer, sample source profile status. |
| Async progress and cancellation | Implemented/tested for basic cases | Single background worker; cancellation at document/page boundaries. |
| Vue/Tauri packaging | Vue and Apple Silicon Tauri release build passed locally | Unsigned arm64 app and DMG exist; clean-device and other target builds remain open under NAT-02. |

**Automated result (13 October 2026):** 15 backend tests and 6 Vitest tests passed before the current QA additions; Vue production build passed. Run the suites for current counts. The `samples/example_output/` artifact was generated from synthetic documents and contains a combined 10-page, 2-volume package, linked indexes and OCR text. The sample artifact is technical evidence, not an approved filing output.

## 6. Suggested architecture

```text
Vue / TypeScript in Tauri system WebView
   |
   +-- Tauri IPC: get_backend_connection() -> ephemeral base_url/token
   |     |
   |     +-- Tauri Rust: launch bundled PyInstaller Python sidecar
   |     +-- listen only on 127.0.0.1, chosen random port
   |     +-- kill child process on normal app exit
   |
   +-- authenticated REST requests (Authorization: Bearer ...)
         |
         v
       FastAPI
         |
         +-- SQLite matter + document metadata (storage.py)
         +-- Originals + working PDF copies
         +-- PDF page actions + English OCR (pdf_ops.py)
         +-- deterministic volume/index/outline engine (engine.py)
         +-- background job executor with polling/progress (jobs.py)
         +-- JSON illustrative court profiles (rules/profiles.json)
         +-- Export: combined.pdf / volume-NN.pdf / index.csv /
                     validation-report.json / filing-package.zip
```

The Python engine is deliberately independent of Tauri. The product owner approved Tauri/Vue for the first-phase targets under DEC-01.

### 6.1 Data model

SQLite tables: `matters` and `documents`.

Matter fields: id, name, court_profile, case_type, petitioner/respondent/advocate/case_number, max_pages_per_volume, keep_groups_intact, created/updated dates.

Document fields: id, matter_id, title, kind, part, label, group_key, position, page_count, ocr_status, original_name, source_sha256, source_relpath, working_relpath, timestamps.

Original source bytes are never edited by page actions or OCR. Working-copy edits now create numbered revisions, retaining prior files; a failed edit discards its candidate. Deleted document records do not erase on-disk originals. Stored source hashes are explicitly checked during validation and immediately before export publication. This checks byte consistency at those times; it does not prove authenticity. DAT-01 serializes app edits and build/OCR jobs per matter and rejects a second engine using the same data directory.

`~/.courtbundle-poc/matters/<uuid>/` for browser-development mode; native app uses Tauri's platform app-data directory.

### 6.2 Deterministic page-map strategy (critical)

For each selected volume, the engine:

1. Validates sources, grouping and sizes.
2. Computes required per-volume index page count (23 records per index page).
3. Places **whole document units** while ensuring document pages + index pages fit the configured cap.
4. Computes each entry's exact **volume-local** 1-based start/end pages.
5. Creates the index pages, copies source pages without working annotations, stamps consistent physical page numbers, and inserts internal links to the calculated start page.
6. Creates a PDF outline with part and document entries.
7. Appends completed volumes to combined PDF and recalculates global link destinations and nested outlines using each volume offset.
8. Exports CSV and report using the **same volume page map**.
9. Stages results and publishes them only when all steps complete.

**Important:** The engine does not split oversized documents. If a document or indivisible group cannot fit in a volume, it fails with an explanatory error. A repeated group key appearing non-contiguously produces an explicit error rather than silently merging unrelated document spans.

### 6.3 Background job model

Build and OCR operations are queued through one Python thread executor. Progress is polled via `/jobs/{id}` and persisted in SQLite. On restart, queued/running jobs are marked interrupted, unpublished build staging is removed, and the UI offers a new-job retry. Jobs do not resume midway. Terminal job records are retained for 90 days. Cancellation checks occur between volumes and OCR pages; responsiveness during large single-volume work remains unverified under DAT-03.

Document revisions and a process-wide PDF processing lock now serialize API PDF operations and builds/OCR/compression across matters. This prevents concurrent native-library use; it is not crash isolation. Moving native PDF work to supervised worker processes remains a release-hardening consideration. Do not add concurrent native calls without validating the library execution model.

### 6.4 Versioned court profiles

In `backend/app/rules/profiles.json`:
- `delhi-hc-illustrative`: technical subset; `max_file_mb: 300`, `require_searchable_text: true`, `require_bookmarks: true`, `disallow_pdf_encryption: true`. `status: NOT_LEGALLY_APPROVED`.
- `general-demo`: no court-based correctness assertion. `status: DEMO_ONLY`.

Every profile stores source links, checked date, and description. The prototype does not yet implement a granular court-rule DSL or lawyer-approval workflow.

## 7. API contracts

All endpoints require a bearer token. `launcher.py` refuses to start without `COURTBUNDLE_API_TOKEN`. In Tauri production-like mode Rust generates a new random token and sets it only for the launched child sidecar and its JS frontend via a narrow IPC command. In local browser development the static demo token is used; NEVER ship it as a production credential.

Main routes:

```text
GET    /health
GET    /profiles
GET    /matters
POST   /matters
GET    /matters/{matter_id}
PATCH  /matters/{matter_id}
DELETE /matters/{matter_id}
GET    /matters/{matter_id}/documents
POST   /matters/{matter_id}/documents         multipart files[]
POST   /matters/{matter_id}/documents/reorder
POST   /matters/{matter_id}/cover
PATCH  /documents/{doc_id}
DELETE /documents/{doc_id}
GET    /documents/{doc_id}/pdf
POST   /documents/{doc_id}/pages/reorder
POST   /documents/{doc_id}/pages/rotate
POST   /documents/{doc_id}/pages/delete
POST   /documents/{doc_id}/ocr
GET    /matters/{matter_id}/validate
POST   /matters/{matter_id}/build
GET    /jobs/{job_id}
POST   /jobs/{job_id}/cancel
GET    /matters/{matter_id}/exports
GET    /matters/{matter_id}/exports/{export_id}/{filename}
```

**Matter example:**

```json
{
  "name": "Fictional Filing - Demo",
  "court_profile": "delhi-hc-illustrative",
  "case_type": "Demonstration filing",
  "petitioner": "Example Applicant",
  "respondent": "Example Respondent",
  "advocate": "",
  "case_number": "",
  "max_pages_per_volume": 300,
  "keep_groups_intact": true
}
```

**Document metadata example:**

```json
{
  "title": "Synthetic Annexure - Bank Statement",
  "kind": "annexure",
  "part": 2,
  "label": "P-1",
  "group_key": "annexure-group-1"
}
```

**Page operation examples:**

```json
{"page_index": 0, "degrees": 90}
```

```json
{"order": [2, 0, 1]}
```

`POST /build` returns a job. Poll its status; on completion its `result` contains an export manifest and filenames. The combined and volume PDFs live in a matter-local export folder. The browser uses authenticated fetch and Blob download. Tauri uses a native Save dialog and streams to a same-directory temporary file before replacing the destination; Apple Silicon device evidence exists, while Windows and macOS Intel acceptance remain open under NAT-03.

## 8. Local development and QA

For exact instructions see root `README.md`. Condensed:

```bash
# Terminal 1 (macOS/Linux)
cd backend
python3 -m venv .venv && source .venv/bin/activate
python -m pip install -r requirements-dev.txt
./dev.sh

# Terminal 2
cd frontend
npm install
npm run dev
# visit http://127.0.0.1:1420
```

For a native desktop build, first build the platform-specific Python sidecar (with PyInstaller and `eng.traineddata`) by running `python backend/scripts/build_sidecar.py` from the repository root. Then `cd frontend && npm run tauri:dev` or `npm run tauri:build`.

**Do not try to run `tauri:dev` before creating the target-triple sidecar binary.** Tauri's externalBin configuration requires it to exist. On macOS both Intel and Apple Silicon need matching builds. Use Rust, Node and OS build tooling appropriate for Tauri.

Run: `cd backend && python -m pytest -q`. The suite covers OCR, PDF index links, combined PDF, volume splitting, oversized document errors, group errors, page editing, authentication and source-integrity failure paths. Use the current run result rather than the handoff's historical test count.

Run: `cd backend && python scripts/build_demo_output.py`; outputs are in `samples/example_output/`. The demonstration uses invented parties, invented order and invented affidavits, not client material.

### Tested and untested reality (updated 13 October 2026)

- **Automated:** backend pytest, Vue production build and Vitest passed locally. Apple Silicon Rust release compilation and focused native Save transfer tests passed. See the acceptance log for exact runs and counts.
- **Browser runtime:** synthetic create/import/preview/edit/OCR/build/download journey exercised; browser file-picker automation was initially blocked and is covered by QA-01 once its Playwright run passes.
- **Apple Silicon native runtime:** packaged app startup, import, OCR, persistence, shutdown and native Save/Save As with PDF/ZIP byte checks were exercised on macOS 26.6.2. The unsigned arm64 DMG was built and verified structurally.
- **Still unverified:** clean devices, macOS 12 minimum, macOS Intel, Windows x64, signing/notarization and full failure-path/device coverage. Automated or local development results do not close these gates.

## 9. POC limitations and required engineering decisions

1. **Licensing:** PyMuPDF offers AGPL/commercial licensing routes; obtain an appropriate license/assessment before proprietary distribution.
2. **Local data security:** The SQLite DB and original/working files are plain on disk; production legal work needs threat modeling, optional storage encryption and safe temporary-file/wipe policies.
3. **No product-grade PDF signature handling:** Reordering, rasterizing, stamping and combining may invalidate or disturb signed material; signed PDFs should be detected/blocked or handled through an approved workflow.
4. **OCR fidelity:** OCR may misrecognise text; image-only and mixed pages are rendered at 240 DPI for recognition, but only the invisible recognition layer is added to the existing page; existing GlyphLessFont layers are skipped. It is NOT safe to treat OCR output as the authoritative document text.
5. **English-only text generation:** The PDF index uses built-in Latin fonts. Hindi and other scripts require bundled, properly licensed fonts and comprehensive text shaping tests. UI strings may be Unicode but PDF index rendering is not yet complete.
6. **Single fixed index layout:** 23 entries per A4 index page, simple columns. Real cases require court/category-specific index templates, sometimes hyperlink conventions and paragraph-level references.
7. **No complete document QA:** Signature validity, legal sufficiency, translation certification, chain-of-custody and proper execution are unassessed.
8. **No court portal submit:** App exports files, it does not upload or mark matters filed.
9. **No generic Office conversion:** Imported formats are PDF/JPEG/PNG only. Word/Excel/PPT conversion is substantially more complex than image-to-PDF; especially reverse conversion to editable formats.
10. **Native Save target coverage incomplete:** The app streams exports through a native Save dialog. Apple Silicon Save/Save As, cancellation, replacement and permission failure passed locally; Windows/macOS Intel and low-disk/interrupted device cases remain NAT-03 work.
11. **Export profile size check:** The selected technical cap blocks publication if the ZIP, combined PDF, any volume PDF, CSV or validation report exceeds it. The illustrative Delhi profile is not legally approved; other routes may require distinct constraints.
12. **Editing history is partial:** Matter-scoped undo/redo covers page/OCR revisions, metadata, settings and order, with 100 retained events. Import, archive and replacement are not reversible; document-set changes clear the replay history. There is no collaborative locking or complete filesystem recovery system.
13. **Archive recovery is available:** Matter/document removal archives records and keeps originals and working revisions. The workspace can restore them, and a portable package can restore a separate copy. There is no in-app permanent deletion; local data and backups remain unencrypted.
14. **Target builds incomplete:** Vue and local Apple Silicon Tauri builds passed. Windows, macOS Intel and clean-device/minimum-OS checks remain open.
15. **No app updates or signing pipeline:** Production distribution requires signed/notarized builds, updates, release rollback, reproducible builds and architecture-specific testing.

## 10. Recommended backlog after developer machine brings up the UI

### Milestone 0 — Native integration verification (first)

- `npm install`, `npm run build`, `npm run test` and `npm run tauri:dev` on Windows and macOS.
- Fix any Tauri API/plugin/binary packaging issues found in runtime testing; check CSP, PDF.js WebWorker and downloaded PDFs in each OS WebView.
- Verify PyInstaller sidecar embeds tessdata and handles fresh installation without external Python/Tesseract dependency.
- Finish native Save validation on Windows and macOS Intel, including low-disk and interrupted transfer cases; test files larger than 80 MB.

### Milestone 1 — Filing bundle POC completion with an advocate

- Obtain 2-3 **anonymized** full real filing examples and the approved outputs, including indexes, bookmarks, annexures and chamber workflow.
- Agree exactly what Parts 1–4 mean and why ~300 pages is mentioned.
- Implement at least one **lawyer-approved** case-type intake schema + document checklist/template, not only the current synthetic cover.
- Add index layout selection, page-label schemes, and stricter source-to-output validation tests.
- Add an explicit “needs legal review” workflow and a case-type/stage selector.

### Milestone 2 — Harden the core

- Complete undo/redo across document-set changes and test portable backup/recovery on native target devices.
- Durable job queue, interruption recovery, failed-file quarantine, preview thumbnails and memory/performance tests on 1000+ pages.
- Encrypted local storage design; handling of signed documents, redaction and native printing.
- Additional OCR languages with approved traineddata and fonts, mixed scanned/native PDFs, batch automation.
- Complete transaction-safe event history and recovery from failed operations.

### Milestone 3 — Expansion toward full client brief

- Court profile data model with exact rule provenance, source references, effective dates, lawyer approval and versioned updates.
- Conditional legal forms/templates, well-defined Word and Office conversion capabilities.
- Manual and auto generated bookmarks/indices with template variations; index recheck after replacement.
- Legally reviewed objection/refiling workflow and acknowledgment tracking, optionally portal integrations where explicitly authorised.
- Advance PDF operations (crop/replace/compress/true redaction, link insertion), optional signature provider integrations and secure sharing architecture.

## 11. Client questions that require answers before a paid fixed-scope implementation

1. Which court(s), bench/side(s), case types and filing stages are the FIRST commercial targets?
2. What exactly are Parts 1, 2, 3 and 4? Supply an annotated screenshot or accepted sample matter.
3. What official direction or local chamber practice gives the approximate 300-page volume cap? Does the index consume pages? What about one document exceeding the cap?
4. Are groups like 1a/1b/1c indivisible? Are annexures separately indexed, and how are separators/covers handled?
5. What does “OCR-based editable text” mean: selectable/searchable text, correcting OCR, or visual full-PDF text reflow?
6. Which Word/Excel/PPT conversions are necessary, in which directions, and with what quality guarantees?
7. Which case types and conditional document templates should be included, and who approves them legally?
8. How should locally edited copies and signed/certified originals be separated in each real workflow?
9. If online sharing is requested after the approved offline first release, what hosting, access, expiry, revocation and retention model is required?
10. Who maintains official court rules, resolves conflicting amendments and signs off updated case-type templates?
11. What installer signing, encryption, audit-log and production support expectations apply to the approved target platforms?

## 12. Architecture decisions to preserve

**Keep:** local-first; immutable imports; Python PDF processing separated from desktop shell; deterministic index/outline/page-map; source-linked technical court profiles; configurable volume splitting; explicit warnings, progress and cancellation; simple versionable service modules.

**Avoid:** a hard-coded nationwide court checklist; treating 300 MB as 300 pages; treating OCR output as verified evidence; “AI compliant” claims; cloud dependency for core document processing; embedding arbitrary filesystem write capability in the Vue frontend; making every PDF editing action change the source PDF; pretending visual PDF editing is trivial.

## 13. Useful technical documentation

- Tauri 2 sidecars: https://v2.tauri.app/develop/sidecar/
- Tauri 2 configuration: https://v2.tauri.app/reference/config/
- Tauri shell command: https://docs.rs/tauri-plugin-shell/latest/tauri_plugin_shell/process/struct.Command.html
- PyMuPDF OCR guide: https://pymupdf.readthedocs.io/en/latest/recipes-ocr.html
- PyMuPDF documentation: https://pymupdf.readthedocs.io/en/latest/
- PyMuPDF licensing: https://pymupdf.io/licensing
- FastAPI documentation: https://fastapi.tiangolo.com/
- Vue 3: https://vuejs.org/
- PDF.js: https://mozilla.github.io/pdf.js/

## 14. Handoff request for the next coding agent

**14 October 2026 implementation note:** SQLite schema v5 adds revision-aware manual bookmarks. `GET|PUT /matters/{id}/bookmarks` manages an ordered, nested list; older revision targets remain visible but are omitted from generated outlines with an explicit report warning. PDF volume and combined outline destinations derive from the same document page map as indexes and CSV. Whole-document replacement uses `POST /documents/{id}/replace` with another active matter document as the source, writes a new working revision, and supports Undo/Redo. Portable matter packages are now v2 to carry bookmarks; v1 restore remains supported. Build manifests have a digest of relevant matter settings, ordered document metadata, original/working bytes, bookmarks and profile content; old manifests are treated as stale. The 14 October evidence and remaining human/legal gates are in `docs/POC_ACCEPTANCE_TESTS.md` and `docs/IMPLEMENTATION_TASKS.md`. These additions do not approve the illustrative court profile.

**14 October 2026 index review follow-up:** In the matter's Documents page, **Bookmark page** marks the currently displayed PDF page. The former collapsible index preview above the document workspace was removed to restore panel height. The adjacent **Index & bookmarks** tab contains the full preview and manual bookmark editor for rename/nesting/reorder/review before building; Build retains a read-only preview alongside settings. Preview calls read-only `POST /matters/{id}/bundle-preview`; its rows and PDF rendering call the same `volume_rows` page-map function. The UI shows index-page reservation, volume page ranges, combined offsets and manual bookmark targets; index rows open their source working document. After export, a read-only PDF.js viewer can show generated PDF pages and outlines for files up to 25 MiB. Filing-specific index layout, numbering conventions and multilingual rendering remain under FIL-03/FIL-08 and require reviewed examples. Native viewer behavior is unverified.

**Later 14 October update:** SQLite schema v6 adds bounded, matter-scoped reusable draft values at `GET|PUT /matters/{id}/values`. Memo of Parties autosaves and these values enter persisted Undo/Redo. Portable packages are now v3 and restore v1/v2 packages as well. The values are not yet connected to forms or generated filing documents; that mapping requires the reviewed DEC-02 workflow. They remain plaintext under the current local security boundary.

> Continue this CourtBundle POC **without replacing its core architecture**. Start by installing the frontend dependencies and running the Vue/Tauri application on a real machine, resolve compile/runtime issues, execute the Python test suite, and validate one complete synthetic matter. Preserve the deterministic page-map model and keep original files immutable. The existing Delhi court profile is illustrative and must never be described as an approved court rules pack. Do not begin nationwide legal logic or complete Office document conversion until actual case workflows and supported courts are confirmed. Maintain backward compatibility of API endpoints or document migration steps, and expand tests with each change.

---

**Decision summary:** Tauri 2 + Vue 3 + TypeScript frontend; Python FastAPI local backend; PyMuPDF/Tesseract PDF engine; SQLite/local files; deterministic matter, document, volume and index pipeline. Build the court-rule knowledge layer only from verifiable scoped sources and advocate-reviewed templates. The present output is an engineering POC and development starting point, not a production legal compliance platform.

## PDF core hardening update — 14 October 2026

See acceptance section AB and the PDF reliability regression suite. This pass adds navigation-preserving page assembly, exact serialized bundle destination checks, Unicode outlines, canonical bookmark hierarchy, additive mixed-page OCR, rotated text placement, measured compression, validated/synced PDF replacement, partial-copy cleanup, honest cancellation outcomes and hash-verified committed-build recovery. Existing imported originals and all retained revisions remain untouched.

Production qualification remains incomplete: native-device coverage, complex scan/font/form/signature fixtures, process crash isolation, full durability under actual power loss, multilingual printed indexes, legal workflow approval, security and distribution gates are not established by these automated tests. The 1,000-page benchmark uses native-text synthetic pages; it does not establish scanned-matter throughput or memory ceilings.
