# Phase 03A implementation — highlight and comment

Date: 29–30 October 2026

**Status: DONE for the scoped 03A highlight/comment workflow on the available Apple Silicon Mac target.** [Certain] Backend, frontend, complete browser and native synthetic Save/reopen checks pass within the limits below. The final full browser suite passed 126 tests with one existing opt-in vault skip.

## Scope and design mapping

The supplied `annotations.json`, `reference-index.json`, ED07/ED09 previews and shared resolved tokens define this slice. ED08, ED23 and ED43 were inspected for boundaries. MatterDraft uses real document text, source pages and review records in place of the design fixture. Underline and Strikeout remain disabled; resolution states and the wider tool set belong to 03B. This page records technical behavior, not legal verification or filing approval.

| 03A requirement | Implementation | Evidence |
|---|---|---|
| ED07 review toolbar and selected Highlight state | `frontend/src/components/PdfViewer.vue`, `frontend/src/reader-design.css`: Highlight, disabled Underline/Strikeout, Comment, Draw, Box and More; real per-page highlight count. | Runtime ED07 screenshot and browser journey. |
| ED08 contextual highlight/comment controls | Right inspector opens after drawing or selecting a saved mark; current body, color, scope and source page come from the record. Source region is drawn on the PDF overlay. | Browser create/edit/reopen journeys; native selected-comment capture. |
| ED09 comments rail and source navigation | Left rail shows real private notes, source page, revision status and recorded time; selecting a note navigates to its document page and region. A pending draft blocks navigation until discarded. | Browser comment filter/navigation and cancel tests; native restart. |
| ED23/ED43 session and compact/dark behavior | Header Save commits the draft through the authenticated annotation API. Cancel discards an unsaved draft. Dirty status is based on actual changes to the selected record. At compact widths the rail and inspector narrow and scroll independently. | Browser failure/retry, Undo/Redo, compact/dark screenshot; native Save/restart. |
| Real record metadata | SQLite schema 22 records `created_at` and `updated_at` UTC times on new annotations; the UI renders only those values. Older records show no invented author/time. Portable package v16 preserves the fields. | Migration, annotation and portable-restore pytest assertions. |
| Stable target and PDF preservation | Existing revision-bound document ID, zero-based source page and normalized rectangle remain the single annotation target. Drawing uses the displayed paper dimensions, including zoom/rotation/scroll. A later page revision marks the record unresolved. | Rotated 125% browser case, backend revision tests, original hashes. |

## Persistence and output policy

The existing backend stores an annotation when Add/Update mark succeeds; the header Save now commits the current draft through that same operation. The source PDF is not mutated by a private highlight or comment. Records survive API reload, application relaunch and portable package restore. Matter Undo/Redo changes the annotation set; selecting an unchanged saved record leaves the header clean. A failed request retains the draft and offers retry. Navigation from a pending draft asks before discarding it. The existing revision check prevents a stale annotation from silently moving to a different working page.

Private highlights and comments are preparation notes. Save working copy and newly built filing PDFs omit them; existing export output remains immutable. Other PDF-scope marks retain their current export behavior. Imported original bytes, searchable PDF text and existing marks are preserved. An author identity is not stored, so no author is displayed. Existing records before schema 22 have no historical timestamps, so the UI does not fabricate them.

## Verification and reference comparison

Backend: `cd backend && .venv312/bin/python -m pytest -q` passed **327 tests**, with six dependency warnings. Frontend production build and **48 unit tests** passed. The final complete browser suite passed **126 tests**, with one existing opt-in vault skip, in **7.9m**. The [acceptance record](ACCEPTANCE_TESTS.md) includes the first-run failure, its isolated rerun, exact native Save/reopen and output hashes.

ED07/ED09 were compared at the supplied 1440×900 viewport. The running toolbar, split page/inspector composition, comments rail, selected states, spacing and semantic light/dark surfaces use the supplied hierarchy and tokens. The PDF in runtime evidence is a synthetic source file with its actual typography and text, so its contents differ from the fixture. Compact and dark states are checked separately; the inspector and rail have independent scrolling. A highlight remains a rectangular review region, including over text; semantic multi-line text selection is outside the existing annotation model and remains an explicit 03B decision.

Runtime captures: [ED07 1440×900](ui-evidence/03a/ED07-runtime.png), [ED09 1440×900](ui-evidence/03a/ED09-runtime.png), [ED09 compact dark 1024×768](ui-evidence/03a/ED09-compact-dark.png), [compact contextual inspector](ui-evidence/03a/ED08-compact-dark-inspector.png), and [native selected comment with clear page raster](ui-evidence/03a/native-final-comment-inspector.png). ED09's All/Open/Resolved filters are 03B states, so the running 03A rail retains the existing search filter and displays current/revision status. The selected ED07 fixture shows a semantic text-range highlight; the running app draws a normalized rectangle over selectable PDF text. These are the remaining visible differences in this bounded slice.

## Platform and next task

The native QA session uses an isolated synthetic matter store, a local Python backend and the rebuilt Apple Silicon debug app. The matching frozen sidecar and release app compile separately; the encrypted release vault was not used for this synthetic workflow. On the final exact-bundle launch, the app first required Retry connection and explicit review/discard of an old synthetic session draft before reopening the saved comment; general cold-start recovery remains unqualified. Its final capture has a muted PDF raster even though the AX tree exposes selectable text and the saved comment; a prior QA capture shows the page clearly. Final-build native paint timing is not qualified by that capture. Windows x64, Intel Mac, clean-device/minimum-macOS, production encrypted-vault startup, native Office/printing and broader accessibility qualification remain unverified. Native screenshots and exact Save/restart evidence are linked from the acceptance log. The next bounded task is **03B**: comment-list resolution/reopen behavior, document bookmark editing and the supported remaining annotation tools. No 03B behavior is claimed complete here.
