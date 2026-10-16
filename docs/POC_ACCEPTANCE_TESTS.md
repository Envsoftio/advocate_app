# CourtBundle POC — Acceptance and Test Matrix

**Intended status:** technical prototype; synthetic documents only; not a compliance or legal-accuracy certification.

## A. Automated tests already executed

Run: `cd backend && python -m pytest -q`

| Test | Expected outcome | Result in authoring environment |
| --- | --- | --- |
| Local bearer auth | Unauthenticated requests get HTTP 401 | PASS |
| Matter creation and metadata | Matter returned with unique ID | PASS |
| PDF import and preview endpoint | Working PDF created; original retained | PASS |
| Page operations | Reorder, rotate, delete correctly update PDF | PASS |
| Indexed bundle | Index links resolve to correct page; outline exists | PASS |
| Combined bundle | Links preserved/re-created with global page offsets | PASS |
| Volume splitting | Never divides a single document | PASS |
| Oversized document | Fails with explicit diagnostic rather than splitting | PASS |
| Noncontiguous groups | Fails with explicit diagnostic when keep-together enabled | PASS |
| Draft cover | Generates synthetic cover, marks it as draft | PASS |
| English OCR | Extractable text found after OCR-ing scanned sample | PASS |
| ZIP package | Combined/volume PDFs, CSV and JSON are present | PASS |
| Original preservation | Working-copy page edits do not alter imported source bytes | PASS |
| Multi-page index | 25 entries span two linked index pages correctly | PASS |

## B. Manual end-to-end demo (browser or Tauri)

1. Open app; see local engine connected. Create a matter called `Synthetic Demonstration` using **Delhi High Court (illustrative)**.
2. Import four synthetic source files in `samples/` (PDFs, one JPG). Confirm each item appears with accurate page count; originals remain unchanged.
3. Open JPG preview. Click **Run English OCR**. Wait until the job completes. Verify recognised text is present in the working PDF; visually inspect for mistakes.
4. Mark scanned file as `annexure`, label `P-1`, put in Part 2. Set petition/order to Part 1. Save.
5. Drag to reorder. Confirm document list and persisted ordering survive refresh/restart.
6. In the PDF viewer, rotate one working page and verify original PDF under the matter's `originals` folder is not changed. Optionally test page deletion on a spare sample document.
7. Set the maximum pages per volume to `6`, leaving group cohesion enabled. Build bundle.
8. Expect **2 volumes, 10 combined pages** on the included dataset if starting from unmodified source files and adding no cover.
9. Download `filing-package.zip`. Open `combined.pdf`, check volume index links, bookmarks, and number stamps. For volume 1, index entries should link to pages 2 and 5 (1-based). For volume 2, index entries should link to pages 2 and 4 (within volume).
10. Open `validation-report.json`. Verify that the court profile is flagged as *not legally approved* and that validation does not claim a filing was accepted.
11. Set max pages to 2. Build should fail with an explanatory oversized-document error; prior successful exports must remain.
12. Turn on group cohesion and assign the same group key to documents separated by a different group. Expect a clear noncontiguous-group error.
13. Close and reopen the app. Confirm local matter metadata, working documents, and completed exports persist.

## C. Native distribution checks (not executed in the authoring environment)

- [ ] Windows clean device: installer starts without user-installed Python; WebView2 present or installed; sidecar starts; image OCR works.
- [ ] macOS Intel build: matching PyInstaller and Tauri architecture; prompts and download behaviour checked.
- [ ] macOS Apple Silicon build: same, Apple Silicon architecture.
- [ ] Tauri CSP: no console policy errors for PDF.js worker/canvas and local FastAPI requests.
- [ ] Session token: frontend gets only its own current session. Backend is loopback-only; unauthorized requests rejected.
- [ ] Multiple/large scan files: memory, timeouts, disk usage and cancellation measured.
- [ ] Malformed, encrypted and damaged documents handled without process corruption.
- [ ] Multi-byte/multilingual filenames and actual Hindi-language PDFs evaluated; current index font limitation is expected.
- [ ] Interrupted build does not publish a half-complete export and later rebuild succeeds.
- [ ] Original files hashes stay unchanged after page edits and bundle builds.
- [ ] Signing/notarization, installer updates and PyMuPDF commercial licensing addressed for distribution.

## D. Explicit acceptance exclusions

No actual Indian court submission; no automatic legal opinion; no court-certified correctness; no valid electronic evidence certificate or digitally signed filing; no Microsoft Office import/export; no live scanner acquisition; no Word-like PDF text editing; no legal research AI; no cloud sync or remote links; no encryption at rest.

## E. How to verify PDF sample without running the frontend

The checked-in `samples/example_output/combined.pdf` is a verified deterministic output created from the synthetic sources. Check links/TOC in a PDF reader and compare it to `samples/example_output/validation-report.json`. To regenerate it, run `python backend/scripts/build_demo_output.py` from the repo root, or the same command from `backend/`.

## F. NAT-01 execution evidence — 13 October 2026

Environment: macOS Apple Silicon arm64, isolated FastAPI on `127.0.0.1:8766`, Vite on `127.0.0.1:5173`, Chrome browser UI, synthetic files from `samples/`, and a disposable data directory under `/private/tmp`. Existing development servers on ports 8765/1420 were left untouched. These are technical checks of the illustrative profile, not legal approval.

| Check | Evidence and result |
| --- | --- |
| Browser create and persistence | Created `NAT-01 Synthetic Demonstration` in the UI. Four documents, eight source pages, metadata, 6-page cap and one completed export survived browser reload. **PASS** |
| Import | Four synthetic files imported through the isolated local API and appeared in the browser. Chrome automation could not attach to the file picker because its extension lacks “Allow access to file URLs”; the browser file-picker path is **UNVERIFIED**. |
| Preview and PDF.js worker | Petition and scanned-annexure previews rendered in Chrome; the scan page counter now changes to `1 / 1` after switching from a three-page document. **PASS in browser** |
| Editing and OCR | UI metadata save, document reorder, working-copy page move/delete on a disposable import, rotation with immediate refreshed preview, and English OCR completed. OCR text included `ANNEXURE P-1 (SYNTHETIC SCAN)` and `OCR TEST DOCUMENT - 2026`. The four retained source SHA-256 hashes matched the files in `samples/` after these operations. **PASS in browser** |
| Build and downloads | UI built two volumes and a 10-page combined PDF with a 6-page cap. Browser ZIP download matched the stored export SHA-256 `c0859233fd053172d964215b98d29d82a62b200a33e76da63e1c3423f8393ba8`. ZIP contained combined and volume PDFs, CSV index, JSON report and warning note. PDF links/bookmarks resolved to volume pages 2/5 and 2/4; combined links resolved to pages 2/5/8/10. Report said `NOT_LEGALLY_APPROVED`. **PASS in browser and artifact inspection** |
| Error and retained export | Setting the cap to 2 produced a visible oversized-document failure without splitting a document. The prior successful export remained available; cap was restored to 6. **PASS** |
| Automated checks | `backend/.venv312/bin/python -m pytest -q`: **9 passed**. `npm run build`: **PASS**. `npm run test`: **4 passed**. |
| Apple Silicon native development | `tauri dev` compiled the Rust debug binary and started `target/debug/courtbundle-poc` against the isolated servers. Tauri reported version mismatches (`tauri` 2.9.3 versus JS API 2.12.3; opener 2.5.3 versus JS plugin 2.7.1). The native window was unavailable to the UI automation surface, so native create/import/preview/export/CSP checks are **UNVERIFIED**. |
| Other target devices and packaged lifecycle | Windows 10/11 x64, macOS 12+ Intel, clean installs, sidecar startup/shutdown and native downloads have **NO DEVICE EVIDENCE**. These remain NAT-01/NAT-02 blockers. Progress/cancellation and large-file behaviour also remain unverified. |

The browser run found two preview defects and fixed them: rotation now updates the document timestamp used by the viewer, and the page counter tracks the loaded PDF reactively. The rotation API test asserts the timestamp changes and the first page rotates to 90°. NAT-01 remains open until native runtime and target-platform evidence are recorded.

## G. NAT-02 Apple Silicon packaging evidence — 13 October 2026

Environment: macOS Apple Silicon arm64. This is a local build and synthetic sidecar check, **not** a clean-machine install or native UI journey.

| Check | Evidence and result |
| --- | --- |
| Release compile and package | `npm run tauri:build -- --bundles app --no-sign` completed. The resulting unsigned `CourtBundle POC.app` is 57.64 MiB. The app executable and embedded `filing-api` are arm64 Mach-O binaries. **PASS for local Apple Silicon build only**. |
| Tauri integration blockers fixed | The release build initially failed because `set_process_group` is absent from the locked `tauri-plugin-shell` API. That unsupported call was removed; child shutdown still needs app lifecycle verification. JavaScript API/opener versions were pinned to the locked Rust minor versions (`2.9.0` and `2.5.3`), so the ordinary build no longer needs a mismatch override. |
| Sidecar identity | The app's `Contents/MacOS/filing-api` SHA-256 matched `frontend/src-tauri/binaries/filing-api-aarch64-apple-darwin`: `74b64f492db4dc61cd601e38c9e06227b974268f00cd4f94207aedcd6caa0c97`. **PASS**. |
| Frozen sidecar and OCR | The frozen arm64 sidecar started on an isolated loopback port with a temporary data directory and test token. Authenticated `/health` returned `ok`; unauthenticated health returned 401. Importing `samples/04_scanned_annexure.jpg` and starting OCR completed with one page checked and one page OCR applied. The test removed `tesseract` from `PATH` and cleared `TESSDATA_PREFIX` before launch. **PASS in this development environment**; an actual clean device may expose missing runtime libraries. |
| Automated checks | Backend: **9 passed**. Frontend Vitest: **4 passed**. The production Vue build ran during the successful Tauri build. |
| Still required | Test app-launched sidecar startup and shutdown, native OCR/UI and save/export, clean install and reinstall without Python/Tesseract, and corresponding Windows x64 and macOS Intel builds/devices. The unsigned app is not a distributable release. |

## H. NAT-02 packaged macOS runtime and shutdown — 13 October 2026

Environment: macOS 26.6.2 arm64, unsigned local package, synthetic `samples/04_scanned_annexure.jpg`. These checks do not establish the approved macOS 12 minimum or clean-device compatibility.

| Check | Evidence and result |
| --- | --- |
| Native startup and OCR | Opened the packaged `.app` through the desktop UI. It connected to its bundled sidecar, created `NAT-02 Synthetic Native Test`, imported the sample through the native file picker, and reported English OCR `processed`. **PASS on this machine**. |
| Shutdown defect and fix | Before the fix, quitting the app killed PyInstaller's bootstrap but left its worker serving HTTP 401 on the old loopback port. The exact orphan was stopped. The desktop now passes its PID to the launcher; the worker requests Uvicorn shutdown when that PID exits. After rebuild, quitting removed both PyInstaller processes and closed the port. **PASS on macOS arm64**; Windows behavior remains untested. |
| Reopen and relocated copy | Relaunch restored the synthetic matter, imported document and `processed` OCR status. A byte-identical app copy under `/private/tmp` also launched, connected, reopened that matter and quit with no remaining sidecar or listener. **PASS on this machine**. This does not simulate a clean OS or a different macOS version. |
| Frozen sidecar rebuild | PyInstaller now keeps its cache under `.build-sidecar/config`, avoiding a sandbox failure while cleaning a user-global cache. The rebuilt sidecar completed one-page synthetic OCR with `tesseract` absent from `PATH`, returned 401 without the bearer token, and left no process after its desktop parent exited. **PASS on this machine**. |
| DMG | The unsigned `CourtBundle POC_0.1.0_aarch64.dmg` built with macOS disk-image tooling outside the workspace sandbox. `hdiutil verify` reported a valid checksum. Read-only mount contained the app and Applications link; embedded sidecar SHA-256 matched the final build input (`93e7f2bb90fb1c897e87c1d1529ec1b0dab24e1d88a8d2e3bb5b4bd6e0354120`) and was arm64 Mach-O. **PASS for local package structure**. |
| Automated checks | Backend `pytest`: **11 passed**. Frontend Vitest: **4 passed**. Vue production build and Rust release compile passed during packaging. |
| Remaining release gate | On clean macOS 12+ Intel, macOS 12+ Apple Silicon, and Windows 10/11 x64 devices: install the native package without Python/Tesseract, test offline OCR, quit/restart, reinstall with data retained, and capture process/port evidence. Windows parent liveness uses a Win32 process handle and has not been device-tested. The unsigned DMG is not signed or notarized. See `docs/NAT02_DEVICE_VERIFICATION.md`. |

## I. NAT-03 native Save implementation — 13 October 2026

The export UI now offers Save and Save As in Tauri. A Rust command owns the native picker and streams an authenticated export from the local sidecar directly to a temporary file beside the chosen destination. It checks the response length, syncs the file, then replaces the destination. Cancel returns without writing; errors remove the temporary file and are shown in the UI. Browser downloads retain their existing behaviour.

| Check | Evidence and result |
| --- | --- |
| Source and dependency check | `cargo check` passed with Tauri 2.9.3 and the pinned dialog plugin. **PASS on macOS arm64 compile**. |
| Transfer failure behavior | `cargo test --lib`: 2 passed. The loopback test checks authenticated streaming, replacing a prior destination after a complete transfer, and retaining that destination with no `.partial` file after an incomplete response. **PASS on macOS arm64**. |
| Frontend and backend | Vue production build and 4 Vitest tests passed; backend `.venv312/bin/python -m pytest -q` reported 11 passed. **PASS**. |
| Package | Unsigned Apple Silicon `.app` compiled and bundled. This proves packaging only. |
| Native UI | The computer-use native pipe closed before returning the app window. Save picker, Save As, cancel, PDF/ZIP reopen, low-disk error, and actual destination bytes are **UNVERIFIED**. Windows x64 and macOS Intel compile/device checks are also pending. |

On each target device, use a synthetic export to test Save to a new path, Save again to the same path, Save As to another path, cancelling the picker, replacing an existing file, and a forced write failure. Compare SHA-256 of saved PDF and ZIP files with their stored export files, reopen both, and confirm no partial destination is presented as complete.

## J. NAT-03 Apple Silicon native Save verification — 13 October 2026

Environment: packaged unsigned macOS arm64 `.app` on macOS 26.6.2; the existing synthetic `NAT-02 Synthetic Native Test` matter and its one-page scanned sample. A new one-volume, two-page export was built in the native UI. Saved files were placed under `/private/tmp/courtbundle-nat03-save-check`; no confidential matter was used.

| Check | Evidence and result |
| --- | --- |
| Native Save and Save As | The native picker saved `combined.pdf` to a new path, Save As saved it to a second path, and repeat Save replaced a disposable prior destination without reopening the picker. Both saved PDFs matched the stored export SHA-256 `d4082469e259bf87197df911f064eafbe1995a4b0102b0d2246fbede0f876095` (151,292 bytes). **PASS on this device**. |
| ZIP and cancel | Cancelling the ZIP picker created no file. A subsequent Save wrote `filing-package.zip`, matching the stored export SHA-256 `e4eaae5fc94b427d9ab7d7bcd460717c50feb18d2e099f077cb5d73de9b7e877` (238,353 bytes). The ZIP reopened, listed the expected five entries, and passed CRC inspection. **PASS on this device**. |
| PDF reopen | PyMuPDF reopened the saved PDF as two pages with three outline entries. **PASS**. |
| Forced write failure | Save As targeted an existing disposable PDF in a directory made read-only (`0555`). The UI reported `Cannot create a temporary file at the save location: Permission denied (os error 13)`. The destination SHA-256 remained `504aacdb6b2b38881956c02b7b572615a573e1c5eb00cf45732af43db5763ca0`, and no `.partial` file remained. The directory permission was restored afterward. **PASS for permission-denied failure handling**. |
| Remaining NAT-03 acceptance | Low-disk and interrupted native transfer behavior have no device evidence. macOS 12+ Intel and Windows 10/11 x64 native Save journeys remain untested. These Apple Silicon checks do not close NAT-03 for the approved target matrix. |

## K. FIX-02 export-size failure — 13 October 2026

The selected profile's file-size cap now blocks publication when any downloadable artifact exceeds it: the ZIP, combined PDF, each volume PDF, CSV or validation report. The check runs in the staging directory before the export manifest is published. A failed job names the oversized files; no new report claims a completed export. The build UI marks any retained package as a previous export after a failed build. This is a technical cap, not a legal approval of the resulting files.

The synthetic API regression first built a valid package, then lowered the cap to force (1) an oversized combined PDF and (2) an oversized ZIP while the combined PDF remained under the cap. Both jobs failed, only the earlier export remained listed, and its PDF remained downloadable. `backend/.venv312/bin/python -m pytest -q` reported **12 passed**. `npm run build` and `npm run test` passed with **4 frontend tests**. Native UI verification of this failure path remains pending under NAT-01.

## L. FIX-03 imported-original integrity — 13 October 2026

Explicit validation now checks each imported original against its recorded SHA-256. Missing, unreadable and changed originals are errors with document IDs. Build preflight blocks these errors, and the engine repeats the check immediately before publishing a staged export. Neither check repairs the source or changes the stored baseline hash. Source hashes indicate byte consistency with the import record; they do not establish authenticity or legal validity.

Synthetic API tests removed one original, changed another, and separately changed an original after rendering during a build. Validation reported both identified errors, the builds failed, the prior export remained available, and no new export was published after the during-build change. `backend/.venv312/bin/python -m pytest -q`: **14 passed**. This checks integrity at the check times; DAT-01 still needs revision locking for a consistent build snapshot.

## M. FIX-04 annotation and link export policy — 13 October 2026

The selected POC policy omits working-copy PDF annotation objects and source link actions from volume and combined PDFs. It preserves imported originals and recreates generated index links and bookmarks from the page map. Flattened marks already in page content remain visible. This is an output policy, not signature or legal verification.

A synthetic two-page PDF contained one text annotation, one internal page link and one URI link. Explicit validation and the stored export report warned about one annotation and two links for that document. Inspection of both output PDFs found no annotations or source-page links; each index page had its generated link to output page 2 (1-based). The original bytes matched the import fixture. The ZIP README described the policy. `backend/.venv312/bin/python -m pytest -q`: **15 passed**; Vue production build and **4 Vitest tests** passed. Native UI inspection remains under NAT-01.

## N. FIX-01, FIX-05, QA-01, QA-02 and DOC-01 work — 13 October 2026

The browser E2E suite runs `cd frontend && npm run test:e2e` with Google Chrome, an isolated backend on loopback port 8768, Vite on 1420 and a temporary data directory. Its two tests passed. One blocked health requests, observed the UI leave Connecting within about 13 seconds, then retried and reconnected. The other imported the four synthetic `samples/` files through the browser file input, rendered and rotated the petition preview, moved and removed a working-copy page, ran English OCR on the scan, saved a renamed document and verified it after reload. A successful package ZIP downloaded through the UI matched the authenticated stored ZIP by SHA-256. A subsequent oversized build failed while the earlier export remained listed. This is browser evidence, not native WebView or clean-device evidence.

The request client now bounds health and other fetch waits, retries session lookup after failure, and displays native-specific restart guidance if the packaged engine is unavailable. Backend regression checks cover unchanged source bytes after rotation, malformed-PDF cleanup, and cancellation of a queued job before its task begins. `backend/.venv312/bin/python -m pytest -q`: **17 passed**. `npm run build`: **PASS**. `npm run test`: **6 passed**. These checks do not prove concurrent editing or crash recovery; QA-02 remains open with DAT-01/DAT-03.

The canonical handoff is `docs/PROJECT_CONTEXT_AND_HANDOFF.md`; the old root handoff redirects there. README and handoff status were updated to reflect existing Apple Silicon package/native Save evidence without implying Windows, macOS Intel, minimum OS or legal approval. FIX-01, FIX-05, QA-01 and DOC-01 are closed in the backlog; QA-02 remains in progress.

## O. DAT-01 revisions and DAT-03 interruption recovery — 13 October 2026

The backend now keeps numbered working-copy revisions and a SQLite revision catalog. Page edits and OCR work on a candidate file; successful edits publish a new path, while failed edits delete the candidate. An OS lock excludes a second engine on the same data directory. A per-matter lock serializes app mutations, OCR and bundle assembly. Export manifests and validation reports record input revision numbers and working-file SHA-256 values. This is technical provenance, not evidence authenticity or signature verification.

Job progress/state is also stored in SQLite. Startup marks queued/running jobs interrupted, removes unpublished `.build-*` directories, and offers a new-job retry through the API and build UI. Completed, failed and cancelled job records expire after 90 days; interrupted records remain until a later retention decision. Jobs are restarted from the beginning, never resumed midway. A synthetic recovery test confirmed an interrupted build, abandoned-staging cleanup, one retry, no published export from the failed retry, and retained status after reopening. Bundle rendering checks cancellation between chunks of at most ten pages; a 60-page synthetic cancellation left no staging or published export. A single PyMuPDF save call remains non-interruptible.

| Check | Result |
| --- | --- |
| Backend regression | `backend/.venv312/bin/python -m pytest -q`: **25 passed**. Includes legacy migration, future-version refusal, revision preservation/failed-edit cleanup, paused-build/concurrent-edit ordering, second-engine rejection, job restart/retry, large single-volume cancellation and matter-scoped Memo data persistence. |
| Frontend | `npm run build`: **PASS**; `npm run test`: **6 passed**. |
| Browser journey | `npm run test:e2e`: **3 passed** against isolated loopback services and synthetic samples, including Memo autosave, reload, failed save and Retry. |
| Open at this checkpoint | Forced native process crash/restart and device evidence under NAT-01/NAT-02; DAT-02 undo/redo and DAT-04 portable backup/restore. DAT-04 was completed later in Section Q; DAT-02 remains partial. Native window-close prompting remains under NAT-01. |

## P. DAT-05 form draft handling — 13 October 2026

Document details and bundle settings now indicate unsaved values. Switching document, matter or view and reloading the browser prompts before discarding them. Refresh is guarded, and a failed bundle-settings save stops the build before job submission. Existing Memo of Parties autosave, failure state and retry remain covered by the browser journey.

`backend/.venv312/bin/python -m pytest -q`: **25 passed**. `npm run build`: **PASS**. `npm run test`: **6 passed**. `npm run test:e2e`: **4 passed**, including a synthetic browser test that declined and accepted the discard prompt, then forced a settings PATCH failure and confirmed that no build job was created. Native window-close prompting and target-device behavior remain part of NAT-01.

## Q. Ten-task backlog pass — 13 October 2026

The product owner requested the first ten unchecked backlog items in list order. DEC-02 received a [provisional synthetic engineering reference](DEC02_ENGINEERING_REFERENCE.md), approved for repeatable technical tests only; advocate review and a scoped filing workflow remain pending. DEC-03's broad Office conversion/visible-edit scope is recorded, but actual Office fixtures, engines and fidelity acceptance are still absent. NAT-01/NAT-02/NAT-03 remain open for target-device evidence.

| Task | Outcome in this pass |
| --- | --- |
| DEC-02 | Engineering fixture documented; **open for advocate-approved scope and expected outputs**. |
| DEC-03 | Scope reviewed; **open for fixtures, conversion implementation and fidelity checks**. |
| NAT-01 | Seven synthetic browser journeys pass; **native end-to-end target matrix remains open**. |
| NAT-02 | New arm64 sidecar/app/DMG built and sidecar started locally; **clean-device, macOS 12, Intel and Windows checks remain open**. |
| NAT-03 | Native save command compiles and two transfer tests pass; **target-device Save/Save As, low-disk and interrupted transfer remain open**. |
| DAT-02 | Persisted partial undo/redo and stale-export indication; **import/archive/replacement and full transaction safety remain open**. |
| DAT-04 | **Done**: versioned portable backup/restore, hash/path/version rejection, browser round trip. |
| DAT-05 | **Done**: draft indicators/navigation prompts and failed-save build guard. |
| DAT-06 | **Done**: matter/document archive and restore, retained originals/revisions, storage usage. |
| DAT-07 | Metadata-only audit and log suppression; **at-rest encryption/key recovery and temporary-file controls remain open**. |

Final automated run: `backend/.venv312/bin/python -m pytest -q` **33 passed**; `npm run build` **passed**; `npm run test` **6 passed**; `npm run test:e2e` **7 passed** against isolated loopback services and synthetic files. `cargo check` and `cargo test --lib` passed on macOS arm64 (2 Rust tests). The focused browser cases covered backup/restore, archive/restore and Undo/Redo across reload. The archive API regression verified unchanged original hashes and preserved IDs; portable-package regressions rejected corrupt, unsafe and future-version packages. The audit regression found no document title text or test bearer token in returned events.

PyInstaller 6.22.3 with Python 3.12.13 rebuilt `filing-api-aarch64-apple-darwin`. `npm run tauri:build` produced an unsigned arm64 `.app` and DMG; both packaged executables inspected as arm64. Final DMG SHA-256: `2d7a13d18dca40136eb3cfbb20b53cfca52dbd6f02ec97751159fb0eb673391e`; the app's sidecar SHA-256 matches the rebuilt binary at `1fcfc8b339473745e390b17149cb74bb458250e0d50a57ebce3041de9b62e9d0`. One first-run packaged-sidecar smoke attempt did not reach `/health` within 15 seconds; a second attempt succeeded, and three further isolated starts reached health in 7.2–7.6 seconds. The native UI readiness window was extended to 30 seconds, then the package was rebuilt. The exact sidecar inside the final `.app` reached authenticated `/health` in 13.7 seconds against a fresh temporary store. The built DMG was not installed on a clean device in this pass, and the updated native UI was not exercised. These results do not close NAT-01, NAT-02 or NAT-03.

## R. UX-02 local text search and thumbnails — 13 October 2026

Matter search now reads text from current working-copy PDFs under the matter lock and returns matching document/page/revision/snippet records. Browser results highlight the query in snippets and jump to the matching page. A window of seven PDF.js thumbnails supports page navigation. The existing metadata filter remains separate. OCR text remains unverified, and search does not judge document authenticity or legal sufficiency.

`backend/.venv312/bin/python -m pytest -q`: **34 passed**, including native-text search after a page revision and OCR text search. `npm run build`: **PASS**. `npm run test`: **6 passed**. `npm run test:e2e`: **8 passed** after fixing an older journey's empty-store assumption; a subsequent focused browser run confirmed that deleting a matched working-copy page removes its search result (**1 passed**). Browser tests used isolated loopback services and synthetic samples. Native target-device UI and large-matter search performance remain unverified under NAT-01 and REL-01.

## S. UX-03 browser appearance and keyboard checks — 13 October 2026

The UI now saves system/light/dark appearance and 90/100/110/125% interface size in local browser storage. It offers a skip link, a visible focus outline, `/` for document search, and Ctrl/Command+Z and Shift+Ctrl/Command+Z for matter undo/redo outside editable fields. The sidebar and workspace have narrower-window layouts, and motion effects respect the OS reduced-motion setting. These preferences affect presentation only, not PDF output or matter data.

An isolated Chrome browser test confirmed the dark/125% choice persisted across reload, keyboard skip-link focus reached main content, `/` focused document search both in place and from Build, a text-field shortcut did not move focus to app history, and the 820 px window at 125% had no horizontal document overflow with primary controls visible. The full browser E2E suite passed **9 tests** on the alternate isolated Vite port 1421; backend pytest passed **34**, Vue production build passed, and Vitest passed **6**. Target-device native visual, keyboard and screen-reader review at each supported scale remains open under UX-03/NAT-01.

## T. Dark-mode review and PDF-01 page tools — 13 October 2026

Dark-mode browser assertions now cover document panels, the viewer, upload zone, build settings, validation, filing-guide cards, party form, About cards, and the matter modal. A 125% Chrome screenshot revealed clipped matter actions and a white page-edit toolbar; both were corrected. System appearance was tested by switching the emulated OS color scheme from dark to light. This is browser visual evidence only; native contrast, keyboard and screen-reader review remains UX-03/NAT-01.

PDF-01 adds working-copy insert before/after, append/merge, one-for-one replace, and proportional crop. Extract saves selected pages in user order as a PDF; general split saves sequential parts as a ZIP independently of filing volume allocation. Browser files were checked for PDF/ZIP signatures; the backend tests opened the files and checked page order, split counts, crop geometry, JPEG-derived page dimensions, unchanged imported originals, edit revisions, undo/redo, and prior-export staleness. Read-only extract/split did not advance revision history. The native Save command compiled on macOS arm64 and uses the existing authenticated, temporary-file replacement transfer; it has not been exercised in the packaged UI or on Windows/macOS Intel.

Final checks for this pass: backend pytest **38 passed**, Vue production build passed, Vitest **6 passed**, `cargo check` passed, `cargo test --lib` **2 passed** when loopback binding was permitted, and the full isolated Chrome E2E suite **10 passed**. The initial sandboxed Rust transfer test failed solely because loopback binding was denied; the same tests passed with loopback permission. PDF-01 device Save behaviour remains under NAT-03.

## U. PDF-03 first editing slice — visible text overlay — 13 October 2026

The preview now places a printable ASCII text overlay on a selected unrotated working-copy page. The user types text, clicks a position, and applies it. The backend refuses invalid pages, insufficient space, non-ASCII characters and rotated-page placement without advancing the revision. This is added visible PDF content, not editing or legal verification of source text. The overlay is searchable and carried into generated PDFs. Existing annotation omission policy is unchanged.

Backend tests confirmed unchanged imported-original hash, new revision, searchable visible text, combined-PDF inclusion, undo and rejected-edit cleanup. `backend/.venv312/bin/python -m pytest -q`: **40 passed**. Vue production build and **6 Vitest tests** passed. A focused isolated Chrome test placed text through the preview and found it through matter search (**1 passed**); the full isolated Chrome regression suite then passed **11 tests** on port 1434. The first browser attempt could not bind loopback in the sandbox; the permitted rerun passed. Native UI placement, Unicode fonts, rotated pages, individual overlay editing/deletion, image overlays, highlights and comments remain unverified or unimplemented under PDF-03.

## V. PDF-07 first native-text replacement slice — 13 October 2026

The preview highlights selectable horizontal text runs. Clicking one opens its original text and an editable replacement field. A replacement must be one line of printable ASCII and fit within the original run's width. The backend checks the current revision and exact selected text, rejects runs overlapping other runs or links, excludes the invisible `GlyphLessFont` OCR layer, and refuses rotated pages. It removes the selected text from a new working-copy revision and draws the replacement at the old baseline. This does not reflow nearby content and does not constitute secure redaction.

Synthetic backend tests verified that text extraction on the edited working copy and combined export contains the replacement and not the selected old run; the imported original hash is unchanged; Undo restores the old run; stale and overwide replacements fail without publishing a new edit. The OCR test verifies that searchable scanned text is not offered as visually editable text. The edited sample page was rendered and visually inspected: the replacement appeared at the intended position with surrounding lines intact. Backend suite: **42 passed**; Vue production build and **6 Vitest tests** passed. A focused isolated Chrome test selected and replaced a run through the preview, then verified page-specific search results (**1 passed**). The full isolated Chrome suite passed **12 tests** on port 1437. Native UI, complex fonts/layouts, multilingual text and scanned visual editing remain unverified or unimplemented.

## W. Text replacement fit and 422 feedback — 14 October 2026

The earlier replacement endpoint rejected any text wider than the selected original run, returning HTTP 422. The user reported that this made editing nonfunctional. The backend now measures clear space to the right and below the selected run, wraps at word boundaries and tries font sizes down to 75% of the source size. It will not move neighboring content; an edit that cannot fit still fails without publishing a revision. The response reports applied font size, line count and whether size was reduced. The preview now uses a multiline replacement field, displays the applied layout, and shows an explicit fitting error when an edit is rejected. The exact cause of the user's reported 422 was not available from the status line alone; this addresses the reproducible width rejection.

Synthetic backend tests cover a formerly rejected longer replacement, a wrapped replacement, an unfittable word, stale revision rejection, export and undo. A wrapped synthetic page was rendered and visually inspected; the new lines stayed above the next section. The full backend suite passed **43 tests**; Vue production build and **6 Vitest tests** passed. A focused Chrome test replaced text with a longer string, then confirmed that a deliberately unfittable replacement shows its reason in the editor (**1 passed**). The full isolated Chrome suite passed **12 tests** on port 1439. Native target-device UI and full Word-like document reflow remain unverified or unimplemented.

## X. DAT-02 document-set history — 14 October 2026

Import, archive and restore now add persisted matter history entries. Undoing an import hides that document without removing its original or working copy; redo restores it. Archive and restore replay their prior position and visibility. Every history replay applies its data change and cursor update in one SQLite transaction, and a missing file blocks restore without advancing the cursor. New edits still clear the redo branch. The backend suite passed **49 tests** at this checkpoint, including source-file integrity, order, branching, manual archive/restore, and missing-file failure. A focused isolated Chrome test passed for import Undo, reload and Redo (**1 passed**); this is browser evidence, not native evidence. The preceding full Chrome suite passed **13 tests** before this history change.

An additional startup regression simulates a crash after a new revision PDF is written but before its SQLite record is committed. Restart moves that orphan file into the matter's `quarantine/working` directory, retains the committed revision, records a metadata-only audit event, and still permits Undo. The full backend suite now passes **50 tests**. Full-document replacement scope and product-owner native history checks remain open.

## Y. Existing PDF rotation metadata and text editing — 14 October 2026

The text editor previously rejected every page with a nonzero PDF rotation setting. A user reported this error despite never rotating the page in the application. Imported PDFs may already carry `/Rotate` metadata. According to the [PyMuPDF Page coordinate documentation](https://pymupdf.readthedocs.io/en/latest/page.html), extracted text and insertion coordinates are unrotated, while `page.rect` reflects displayed rotation. The old guard and bounds check were therefore too broad.

Text-run listing now checks unrotated bounds and maps each selection rectangle through `page.rotation_matrix` for the preview. Replacement keeps redaction, obstacle detection, fitting and insertion in unrotated coordinates. Parametrized API tests imported synthetic 90°, 180° and 270° PDFs, verified preview-relative selection coordinates, replaced text, preserved page rotation and original hashes. A focused Chrome test selected and replaced text on a page already marked 90° and checked that its selection box stayed within the rendered canvas. A rotated synthetic output was rendered and visually inspected. Backend: **46 passed**; Vue build and **6 Vitest tests** passed; full isolated Chrome suite: **13 passed** on port 1442. Text drawn with a non-horizontal direction in unrotated coordinates remains outside this editor's selection scope, and Add text overlay still requires an unrotated page.

## Z. DAT-07 local temporary-file controls — 14 October 2026

The backend data root is created or tightened to POSIX mode `0700` at startup. Portable backup/restore transfer files and PDF-edit scratch files now have dedicated hidden prefixes. Normal completion removes them; the next startup removes leftovers from an interrupted process. It leaves unrelated `.cbmatter` files alone. The previous DAT-02 orphan-revision quarantine still preserves an uncommitted working candidate for inspection.

A synthetic restart test began with a `0755` data root, an abandoned portable package, an abandoned partial package and an unrelated package. Startup tightened the root, removed only the app-named temporary files and retained the unrelated file. The portable round-trip test found no transfer leftovers; a simulated interrupted PDF-edit scratch file was removed on restart. Backend suite: **51 passed**. Windows ACL behavior, encrypted storage/key recovery and clean-device security review remain open; this is not evidence that local data is encrypted.

## AA. DAT-02, FIL-04 and FIL-06 implementation review — 14 October 2026

Whole-document replacement copies another active document's current working PDF into a new revision of the target document, retaining the target's imported original and prior revisions. Undo/Redo restore the expected page content. The UI exposes this in page tools; it refuses using the target document as its own source. Synthetic tests confirmed page counts/content, source-byte retention and history replay. Packaged native replacement and history still need target-device inspection.

Manual bookmarks use SQLite schema v5. Each entry records a document ID, zero-based page index and revision, title, parent and order. The Build UI supports adding, renaming, deleting, nesting and drag/drop or button reordering. Export resolution uses the same document page map as the index, CSV and generated outlines. An edited or archived document leaves affected bookmarks stored and visibly unresolved; the export omits those manual entries and records `UNRESOLVED_MANUAL_BOOKMARKS`. The v2 portable matter package carries bookmarks and remaps their IDs on restore; v1 packages remain accepted. Nested multi-volume synthetic tests inspected both volume and combined PDF outlines and the ZIP report. Browser E2E verified a bookmark across reload, stale revision display and explicit resolution. Packaged Windows/macOS PDF viewer, keyboard and drag/drop checks remain open.

Export manifests and reports carry `build_input_digest`, a SHA-256 fingerprint of relevant matter settings, ordered document metadata, current original/working-copy bytes and revisions, manual bookmarks and the selected profile contents. A timestamp check keeps exports stale after Undo even if values return to an earlier state. Old manifests without the digest are conservatively stale. Before atomic publication, the build recomputes the digest and rejects changed inputs. This is a technical freshness indicator, not proof of legal validity or authenticity.

Review found a stale-export regression after Undo when fingerprint comparison alone was used; combining the edit timestamp and fingerprint corrected it. Browser E2E initially found ambiguous generic number-input locators after the bookmark page field was added; those tests now select the named volume cap. An original-byte check initially ran before preflight and masked detailed integrity errors; moving it after preflight preserved those diagnostics. Final verification: `backend/.venv312/bin/python -m pytest -q` **58 passed**; `npm run build` passed; `npm run test` **6 passed**; full isolated Chrome `npm run test:e2e` **15 passed** after local loopback permission. The export journey passed again after the original-byte fingerprint was added (**1 focused E2E passed**). Native build/device checks were not run in this pass. DAT-07 and DEC-02 dependent filing tasks remain open as recorded in `docs/IMPLEMENTATION_TASKS.md`.

## AB. PDF core reliability hardening — 14 October 2026

This is implemented hardening with synthetic evidence, **not a claim of production readiness or error-free PDF processing**. Existing uncommitted project work was retained.

### Implemented changes

- **Page assembly:** reorder/insert/replace/extract/split preserve copied annotations and explicitly reconstruct retained internal links and outlines. Rotated click rectangles and destination points are transformed correctly. Links to excluded pages are omitted. Duplicate destination pages resolve to their first selected occurrence, except self-links which stay with the copied occurrence. Source metadata follows the first source. These tools do not promise preservation of every PDF catalog feature, attachment, script, form or signature.
- **Bundle navigation:** serialized outline entries and index destinations must exactly match the shared deterministic page map before publication. Unicode/full-length outline titles are retained. Manual bookmark lists are normalized to depth-first order so children cannot silently acquire another parent. Index-growth boundary regressions cover 23/24 and 46/47 document transitions. Printed index fonts/templates remain limited.
- **OCR:** mixed native/scanned pages receive an invisible layer; duplicated native text is removed from the new layer. The original visible page, rotation, annotations, links and outline remain. Synthetic 0/90/180/270-degree fixtures retained identical rendered pixel buffers and acquired searchable scanned text without duplicating the native header. Existing GlyphLessFont layers are skipped; partial/poor old OCR therefore still requires review. A 40-megapixel rendering limit prevents pathological OCR page allocations. Cancellation is checked before publication. A no-change result is labelled `no_new_text`, not proof that a document is fully searchable.
- **Editing:** text overlays now accept rotated pages using display-relative coordinates. Text replacement refuses pending redaction annotations and overlapping non-editable text instead of potentially applying unrelated marks or removing another layer. These checks do not constitute a secure redaction implementation.
- **Compression:** Page Tools exposes lossless cleanup, Balanced (240 DPI/JPEG 85) and Smaller (150 DPI/JPEG 70) as background jobs. Results show measured sizes. Only smaller files become new undoable revisions. Largest placement controls shared-image resolution; masks, bitonal and images above 40 megapixels are skipped. Native text, navigation and original bytes are covered. At this 14 October checkpoint, lossy scan/seal fidelity still needed representative visual fixtures; that engineering gap was completed later in section AH. Interrupted compression is restarted explicitly from page tools.
- **Failure handling:** PDF candidates are reopened, page/content/navigation checked and synced before replacement. Failed writes/validation/sync/rename preserve previous bytes. Partial candidate copies are cleaned. Cleanup consults the revision catalog and never deletes a committed revision after an exception; an unavailable database leaves uncertain files for startup reconciliation. Document revision and matter timestamp changes share a transaction. Disk-space/permission failures have actionable API errors.
- **Concurrency/cancellation:** PDF work is serialized process-wide across request and worker threads. Cancelling an upload while it waits for a lock cannot strand the lock; ongoing native conversion finishes before cancellation releases it. A successful task return means committed completion even if a cancellation request arrived too late. This prevents reporting a published artifact as cancelled. It does not interrupt an individual native-library call or isolate native process crashes.
- **Export recovery:** artifacts have SHA-256 values, ZIP CRC validation and synced files before atomic publication. Downloads verify sizes/hashes for new manifests. On startup, a build interrupted between publish and status update becomes completed only if its manifest and every artifact verify. Same-length corruption is rejected. Unpublished staging stays hidden and is cleaned. Actual power-loss/filesystem durability still needs device testing.

### Verification

| Check | Result |
| --- | --- |
| Backend `.venv312/bin/python -m pytest -q` | **88 passed**, 6 dependency deprecation warnings |
| Vue production build | **Passed**; existing >500 kB bundle warning remains |
| Vitest | **6 passed** |
| Full isolated Chrome/Playwright | **16 passed (46.0 seconds)** on Vite 1448/API 8768, synthetic data |
| PyInstaller Apple Silicon sidecar | **Built successfully** with bundled English OCR data |
| Unsigned Apple Silicon Tauri `.app` | **Built successfully**, 58.23 MiB; not signed/notarized |
| Frozen sidecar synthetic smoke | **Passed**: compression, bundled mixed-page OCR, manual outline/index destinations and original SHA-256; system Tesseract removed from PATH, TESSDATA_PREFIX unset |

The first browser run found a strict selector that assumed Page Tools had only one dropdown; it now selects the named source-document control. A subsequent run timed out in the pre-existing unavailable-engine test, though its failure snapshot showed the offline UI. The final unchanged connection test passed in 13.2 seconds. This intermittent timing observation remains recorded; tests were not relaxed to hide it. Initial loopback binding was denied by the sandbox; permitted runs used isolated local services.

### Repeatable scale baseline

Command: `cd backend && .venv312/bin/python scripts/benchmark_pdf_core.py`. Environment: macOS 26.6.2 arm64, Python 3.12.13, PyMuPDF 1.28.2. Fixture: 100 generated native-text PDFs of 10 A4 pages each. Data is temporary and removed afterward.

| Measurement | Result |
| --- | --- |
| Generate and import 1,000 pages | 1.226 seconds |
| Build, verify navigation and package ZIP | 1.516 seconds |
| Full-matter search for an absent term | 0.533 seconds |
| Peak process RSS | 99.70 MiB |
| Output | 4 volumes, 1,007 combined pages including indexes |
| Combined PDF / ZIP bytes | 477,827 / 155,269 |

This is a local baseline for native text, not a scan-heavy benchmark, throughput guarantee or agreed release budget. The script reports no RSS value on Windows, where Python's `resource` module is unavailable.

### Remaining release gates

Windows x64, Intel Mac, minimum macOS 12, clean-machine install, native WebView interaction, native Save failure scenarios, power loss and large scan stress remain unverified for these changes. Native crash isolation/supervision is not implemented. Multilingual printed indexes/fonts, Office conversion, full layout/OCR correction, scanner/deskew, annotation/image authoring, password/security workflows, permanent redaction and legal workflow specifications remain incomplete as listed in IMPLEMENTATION_TASKS.md. Signed-input engineering was completed later in section AG, while its native target-device coverage remains open. No task is closed merely because this hardening suite passes.

Packaged smoke command: `backend/.venv312/bin/python backend/scripts/smoke_packaged_pdf.py frontend/src-tauri/binaries/filing-api-aarch64-apple-darwin`. Local frozen-engine startup took **23.52 seconds**; compression reduced the synthetic mixed PDF from **18,454 to 15,409 bytes**. This is within the current 30-second native readiness window on this run, but shows cold-start performance still needs target-device measurements. The smoke exercises an isolated sidecar API with temporary data; it does not establish packaged WebView/Save dialog behavior or a clean-device install.

## Next ten unchecked tasks from DAT-02 — 14 October 2026

The active backlog selection was DAT-02. The next ten unchecked entries in list order are DAT-02, DAT-07, FIL-01, FIL-02, FIL-03, FIL-04, FIL-05, FIL-07, FIL-08 and GEN-01. The product owner confirmed that no advocate-approved filing specification or expected outputs exist yet.

| Task | Work and remaining gate |
| --- | --- |
| DAT-02 | Memo of Parties autosaves and reusable matter values now enter persisted Undo/Redo. Court rules has Undo/Redo controls that flush pending Memo changes before replay. Packaged Windows/macOS replay and the 100-event retention decision remain open. |
| DAT-07 | Existing plaintext storage and backup boundary remains. Password plus separate recovery key is selected, but encryption, migration, key recovery and device tests remain open. |
| FIL-01/02/03 | Part outputs, group numbering and index schemes remain blocked on DEC-02's reviewed examples. No filing rule was inferred. |
| FIL-04 | Bookmark reorder/delete controls gained accessible labels; synthetic Chrome keyboard reorder persists after reload. Packaged PDF viewer and keyboard/drag review remain open. |
| FIL-05/07/08 | Index import, scoped validation and multilingual generated output remain blocked on reviewed source/schema, checklist and language/font fixtures. |
| GEN-01 | SQLite v6 adds bounded reusable draft values with API, history and v3 portable round trip; Memo rows remain persisted and feed the review CSV. Form/template mapping and agreed roles await DEC-02. |

Final verification: backend `.venv312/bin/python -m pytest -q` **91 passed**; frontend `npm run build` and `npm run test` **passed** (6 unit tests); synthetic Chrome `COURTBUNDLE_E2E_VITE_PORT=1421 npm run test:e2e` **18 passed** when loopback binding was allowed. Initial sandboxed E2E start was denied on port 8768; rerun with loopback access passed. These checks do not establish native Windows/macOS behavior or legal approval.

## AC. Visible index, numbering and bookmark review — 14 October 2026

The matter's Documents page now offers **Bookmark page** on the displayed PDF page. The adjacent **Index & bookmarks** tab shows the index plan and manual bookmark editor during preparation. The collapsible plan above Documents was removed after it reduced document-panel height. Build retains a read-only plan beside its settings. Rows, index-page counts, volume page ranges and combined offsets come from the same `volume_rows` function used for rendering generated PDFs and CSV. Draft page-cap/group settings update the Build plan without being saved. Manual bookmark destinations are shown; unresolved entries are marked for review. An authenticated in-app PDF.js viewer lets users inspect generated PDFs up to 25 MiB, turn pages and jump through the PDF outline. Larger PDFs must be saved and reviewed externally. This is a technical review flow, not an approved filing index template or numbering policy.

Synthetic backend regression compares the preview with a built two-volume PDF, its bookmark destinations and CSV ranges, and checks that an impossible page cap returns HTTP 422. The final full backend suite passed **96 tests**. Vue production build and Vitest **6 tests** passed. The final isolated Chrome Playwright run passed **22 tests**, including draft cap changes, opening an index row's working copy, rendering the generated combined PDF and navigating to a bookmark. An existing journey exposed an initial HTTP 500 when the cap was impossible; the endpoint now returns a clear 422 response, and the corrected full suite passed. Native Tauri viewer behavior still requires device verification.

**Matter preparation visibility follow-up:** The initial preview was only in Build, which made the claim that it was visible while preparing a matter misleading. The **Index & bookmarks** matter tab hosts the full preview and manual bookmark editor; Build keeps a read-only plan. A subsequent usability correction removed the collapsible Documents preview and added page-level bookmarking in the PDF viewer. Responsive workspace sizing now uses the panel's actual position for both wide and two-column layouts. Vue build and 6 Vitest tests passed; the final full synthetic Chrome suite passed **23 tests**, including page-level creation, bookmark reorder, generated PDF review and settled 1024×800 PDF / 1440×800 details-panel bottom-gap checks. The first full run exposed bookmark-editor remounting while a second entry was being entered, plus a test measurement during the entrance transition; both were corrected and the next full suite passed. A matching Apple Silicon PyInstaller sidecar and unsigned `.app` built successfully; the embedded sidecar SHA-256 matched the build input (`a4c7614329b76aee144ae3aa5dcc866c905d35efbbf1c3c48f88f60fb45d0528`). The final page-level layout was rebuilt into the unsigned `.app`; it has not yet been launched for native UI verification. An older running or installed app will not show this new UI until replaced and relaunched.

## AD. UX-05 editable document table and batch actions — 16 October 2026

The Documents workspace now opens a wide editable register without replacing the existing PDF/detail workflow. It shows original filename separately from editable title and label, exact/unknown/ambiguous document-date state, provenance, file timestamp, type, Part, keep-together group, pages and OCR/import status. Part rows are collapsible; each row has Preview, Save and numeric **Move to** controls, while the established drag and arrow controls remain available in the normal workspace. Duplicate labels and numeric label gaps are shown for review without correction or renumbering.

Selection is an explicit ID set and does not change when rows are filtered. Batch type, Part and group assignment is one SQLite transaction and one undo/redo history event; missing/archived IDs reject the whole change. The server produces deterministic manual/upload/filename/document-date previews. Unknown and ambiguous dates sort last, equal keys retain current manual order, suggestions remain unconfirmed, and keep-together groups remain units when the saved policy is enabled. Canceling the preview does not write order; applying it uses the existing reorder/history route. Document metadata continues to participate in export freshness. Portable matter format v5 retains date provenance and accepts legacy v1-v4 packages with conservative unknown-date defaults.

Synthetic backend fixtures cover selected-only atomic updates, one-step undo/redo, failed selection saves, equal/missing/ambiguous/suggested dates, group-aware sorting, duplicate labels, label gaps, source timestamps, schema migration and portable round trip. Final backend verification passed **112 tests** with 6 dependency deprecation warnings. Vue production build and Vitest **6 tests** passed with the existing Vite chunk-size warning. The final isolated Chrome suite passed **26 tests**, with **1 protected-vault test skipped** by its normal environment gate; it covers filtered selection, atomic batch undo, canceled/applied sort previews and confirmed date persistence. A full-suite run first exposed a missing CORS allowlist entry for the source timestamp header; the allowlist was corrected and the affected import/export journey plus final suite passed. Supported native Windows/macOS visual, keyboard, scale and window-size review remains deferred, so UX-05 stays unchecked rather than being presented as fully accepted.

## AE. FIL-02 safe label proposal and citation-review layer — 16 October 2026

The editable document table proposes labels only for an explicit selection. Supported technical modes are user-parameterized sequential labels, configurable grouped suffixes over saved group keys, and conservative filename candidates. A saved group can designate at most one technical parent; the parent receives the base label even if it is not the first document, and member suffixes remain in deterministic document order. Grouped mode supports lower/upper-case alphabetic, numeric and parenthesized suffixes, a custom separator and base-label treatment. The reviewer also selects an existing-label policy: compact recalculates the selected sequence, while preserve leaves existing labels unchanged and exposes new/unlabelled entries for manual completion. Every proposal displays stable document identity, original filename, description, group role, old label/status, editable proposed label and the reason for the suggestion. Partial groups, missing candidates and case-insensitive duplicates are visible blockers. Cancel writes nothing; apply confirms the full mapping as one atomic undoable event. No mode is described as a court-approved naming rule or legally verified hierarchy.

An optional reference document scan extracts searchable-text annexure/exhibit token candidates with the exact working-copy revision, page and context snippet. Old-label, proposed-label and unmatched candidates are distinguished. The scan can miss image-only or imperfect OCR text, is not legal verification, and never edits the petition or another source. Existing-label changes, citation candidates and prior generated bundles each require an explicit acknowledgement. Old build directories remain immutable; new manifests and validation reports record `label_inputs`, including group key and role, while legacy manifests are reported as lacking a label snapshot. SQLite schema v10 and portable matter v7 preserve group hierarchy and label status/source; older packages default legacy documents to the member role and legacy non-empty labels remain unreviewed values.

Synthetic backend tests cover sequential/grouped/filename proposals, configurable `ANN-4.A` and parent-base `P-1/P-1(a)` patterns, a designated parent that is not first in order, duplicate-parent rejection across edits, archived-document restoration and portable-package input, compact versus preserve behaviour after removal, partial groups, missing and ambiguous filename candidates, no-write preview, page/revision citation evidence, acknowledgement gates, stale mappings, duplicate rejection, one-step undo, original-hash retention, prior-build immutability, and agreement of a new label across manifest, CSV and PDF outline. Oversized-group fixtures verify both chosen policies: keep-groups-intact rejects an impossible group, while the split policy produces two volumes. The browser journey designates the second group document as parent, chooses an uppercase dotted group pattern, cancels its preview, verifies no writes, applies the reviewed mapping and undoes all labels in one step. FIL-02's policy-neutral engineering acceptance is complete. Court-specific legal meaning/defaults remain subject to DEC-02/profile review; reusable named court/chamber presets remain FIL-09; native release qualification remains tracked separately.

Final verification: backend `.venv312/bin/python -m pytest -q` **119 passed** with 6 dependency deprecation warnings; Vue production build passed with the existing Vite chunk-size warning; Vitest **6 passed**; the isolated Chrome/Playwright suite **27 passed, 1 protected-vault test skipped by its normal environment gate**. The hierarchy, invariant and volume-policy follow-up added three backend cases, and the focused pair of document-table browser journeys passed before the final full suite. These checks do not replace supported native Windows/macOS review or approve court-specific legal defaults.

**Configurable-pattern and hierarchy follow-up:** grouped proposals expose suffix style, separator and base-label treatment rather than hard-coding `P-1a/P-1b`. Explicit technical parent designation and compact/preserve renumbering complete the hierarchy and insertion/removal mechanics without selecting one universal format in code. This does not turn a reviewer-selected pattern, parent role or renumber policy into a court rule or a legally verified relationship.

## AF. Single-workspace simplification — 16 October 2026

The alternate guided mode, its persisted per-matter mode state, duplicate step navigation, walkthrough panel and mode-specific styling were removed. Matter creation and synthetic sample creation now open the normal Documents workspace. Documents, Index & bookmarks, Preflight checks and Build bundle remain the only preparation navigation; they continue to use the shared backend page map, validation, job and export operations.

Verification after removal: Vue/TypeScript production build passed with the existing chunk-size warning; Vitest passed **6 tests**. A focused isolated Chrome journey passed and confirmed that the remaining workspace displays the shared index/page preview, prevents a build request when bundle settings cannot be saved, retries successfully, generates a package and opens `combined.pdf` in the existing reviewer. Native Windows/macOS behavior remains governed by the existing NAT acceptance work.

## AG. PDF-11 signed-input detection and transformation warnings — 16 October 2026

PDF imports are inspected from immutable original bytes for structural signature dictionaries containing both `ByteRange` and `Contents` indicators. Incomplete or unreadable signature structures are `inspection_inconclusive`; absence of those indicators is not represented as proof that no visible autograph exists. A reviewer separately marks visible/scanned signatures as present or absent. The resulting states distinguish certificate indicators, reviewed unsigned, visible/scanned-only and inconclusive fixtures without asserting validity, identity, trust, revocation, signing time or current byte coverage.

The initial working PDF is a byte-for-byte copy of the imported PDF. Page reorder/rotate/delete/insert/replace/crop, visible-text edits, compression, extraction/split, OCR and bundle generation require explicit acknowledgment for detected, visible/scanned-only or inconclusive inputs. Dismissing the browser warning performs no transformation request; backend enforcement independently rejects unacknowledged requests. Preflight identifies affected documents, and build manifests/reports record the inspection snapshot and whether a signature risk was acknowledged. SQLite schema v11 and portable matter v8 retain the inspection/reviewer state; older backups restore conservatively as inconclusive. Originals remain unchanged.

Synthetic backend fixtures covered all four statuses, exact initial PDF hash retention, denied and acknowledged page edits, denied OCR/build, completed acknowledged build, preflight codes and report metadata. Final verification: backend `.venv312/bin/python -m pytest -q` **122 passed** with 6 dependency deprecation warnings; Vue production build passed with the existing Vite chunk-size warning; Vitest **9 passed**; isolated Chrome/Playwright **28 passed, 1 protected-vault test skipped by its normal environment gate**. `cargo check` and `cargo test --lib` (**2 passed**) succeeded on Apple Silicon after the native extract/split Save path forwarded the acknowledgment; the first sandboxed Rust test attempt was denied permission to bind its temporary loopback listener, and the permitted rerun passed unchanged. No packaged native UI journey, external cryptographic verifier or representative real signed filing was used; native target coverage remains under NAT-01/NAT-02/NAT-03.

## AH. PDF-02 measured compression corpus — 16 October 2026

PDF-02 now has a deterministic 300 DPI, three-page visual corpus: a lightly noisy office scan, a red circular mark explicitly labelled **SYNTHETIC SEAL / NOT VERIFIED**, and a 40-row table containing approximately 5–5.8 pt raster text and fine rules. `backend/scripts/pdf02_compression_acceptance.py` generates source/preset PDFs, side-by-side review PNGs and a machine-readable report. `backend/tests/test_compression_quality.py` applies the same preset-specific byte, PSNR, ink/contrast and red-shape budgets without retaining test artifacts. These measurements are corpus-specific regression floors, not universal visual-quality or portal-size guarantees.

The authoring run used a 20,596,771-byte source. Lossless retained identical rendered pixels and size. Balanced produced 1,913,963 bytes (9.29%); its scan/seal/small-text PSNR values were 29.52/28.36/26.93 dB. Smaller produced 442,236 bytes (2.15%); the corresponding values were 25.45/24.22/22.72 dB. Ink recall was 99.99%/100% for Balanced scan/small text and 95.59%/87.08% for Smaller; seal red recall was 98.66%/94.15%. All recorded budgets passed. Side-by-side inspection found the scan dates/amounts, seal rings/letters, every table row and ambiguous small-text groups distinguishable; Smaller was visibly softer and remains a review-required lossy choice.

The focused compression/PDF reliability run passed **31 tests**. The full backend suite passed **123 tests** with six dependency deprecation warnings; Vue production build passed with the existing chunk-size warning; Vitest passed **9 tests**. Existing PDF reliability tests separately cover searchable native text, links, outlines, immutable originals, revision history, Undo and cancellation. The corpus also exercises and asserts the honest lossless result, `No smaller result; working copy retained.` A visible seal is not a verified signature, and PDF-11 transformation acknowledgment remains required where applicable. Supported native Windows/macOS UI/device qualification remains under NAT-01/NAT-02/NAT-03; it is not inferred from this backend acceptance corpus.
