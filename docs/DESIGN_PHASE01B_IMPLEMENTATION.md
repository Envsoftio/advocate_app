# Design Phase 01B — Documents hierarchy and compact layout

> Visual layout superseded by the user-directed [new UI correction](DESIGN_PHASE02D_UI_CORRECTION.md). This record retains the earlier functional evidence; it is not the current visual completion claim.

27 October 2026. **01B complete; Phase 01 complete under the authorized PNG/token fallback.** Scope is 01B only; no Phase 02 work. Completion records the browser acceptance below; inaccessible-frame fidelity and native runtime remain explicit limitations.

## Implementation

- Reused the 01A semantic tokens, locally bundled Inter, shared controls and persisted theme/interface scale. Documents now has a compact source list with real counts, selected rows, existing thumbnails/reorder handles, the working reader and an independently scrolling metadata panel. Existing search, table, import and cover actions remain connected.
- The Documents grid has one bounded row. Below 1000 CSS pixels of available workspace width, opening Details replaces the list column with the metadata panel. It does not create a second row that takes the reader's height. Closing restores the list (unless the existing reader-focus mode is active). The nonmodal panel leaves the reader operable; opening focuses Close, Escape inside the panel or Close returns focus to the invoking control. No focus trap is appropriate for this adjacent nonmodal panel. The existing search shortcut reveals the list when Details temporarily replaces it, preserving metadata drafts.
- The existing navigation drawer is used at Documents window widths up to 1120 pixels. Other views retain their breakpoint. This avoids squeezing the workspace behind a full navigation sidebar. The root's legacy 860px minimum is removed only while Documents is active, fixing physical overflow with CSS interface zoom. Shell/sidebar heights and bottom padding also account for the existing scale, eliminating blank vertical page scroll and keeping the navigation drawer bounded to the window.
- Reader title/actions reflow within the available panel width; long names ellipsize within their allocated title area and retain the full native title tooltip. Existing tool tabs/review navigation share a scrollable row; navigation/document actions retain deliberate local scrolling. The visible Details label is compact; its accessible name remains “Document details.” Existing page thumbnails sit beside the paper, freeing reading height. No PDF lifecycle, coordinate calculation, source bytes, reader persistence, editing handler or render engine was replaced.
- Paper, source ink and authored PDF typography remain document content. All presentation changes target shell/control elements; no theme recoloring or UI font is applied to PDF text or raster output.

## Design evidence and limits

Pencil `get_app_state` returned the account plan usage limit before any frame inspection. No `.pen` file was opened, parsed or modified directly. Inspected supplied **05A.png and 23B.png**, the phase reference index and semantic/resolved token JSON. **22A/22F could not be inspected**: no corresponding PNG previews were supplied. Native frame details and exact fidelity to inaccessible frames remain unverified.

The PNG hierarchy is adapted to real existing capabilities. Retained thumbnails, PDF editing tabs, OCR and history rather than adding unsupported artifact selectors, Part trees or freeform Notes. The existing matter tabs and workflow navigation remain. No decorative OS controls, routes, library, schema, backend change or new PDF feature.

## Changed paths in this task

| Path | Purpose |
| --- | --- |
| `frontend/src/App.vue` | Documents-specific shell flag, compact-panel state, focus handoff, responsive navigation reuse, live list counts |
| `frontend/src/styles.css` | Scoped Documents layout/typography, responsive panels, reader toolbar reflow and existing thumbnail placement |
| `frontend/src/components/PdfViewer.vue` | Presentation wrapper around existing review navigation/tool tabs; Details `aria-controls` association |
| `frontend/e2e/journey.spec.ts` | Replace obsolete always-three-column expectation with compact visibility/non-overlap/reader-size checks; retain the search-style check before the list is hidden |
| `frontend/e2e/documents-layout.spec.ts` | Real import, visible/uncovered PDF geometry, overflow, header action reachability, keyboard Details open/close and page navigation across 16 viewport/theme/scale combinations |
| `docs/evidence/phase01b-20261027/` | Reproducible synthetic capture script, before/after PNGs, metrics and actual logs |
| This handoff; `docs/ACCEPTANCE_TESTS.md`; kit Phase 01 `TASK.md` | Scope, verification and status |

Existing `.gitignore`, Phase 00/01A application changes, tests, tokens, fonts and evidence are preserved. Workspace CSS formatting changed during the session outside this task's editing commands and was retained; a final production rebuild produced byte-identical CSS to the previously verified build (`091453ff9db5c24a3806123357af3b10b1124c234a7077a767055769aa792515`). The local kit remains ignored under the existing ignore policy. No commit or deployment.

## Acceptance

- **PASS:** real API-backed synthetic Documents workflow; full existing enabled browser journeys passed.
- **PASS:** available 05A/23B hierarchy, 01A semantic colors/Inter and selected states adapted to the existing working controls. This is not a fidelity claim for uninspected 22A/22F.
- **PASS:** target viewport/theme/scale matrix, reader/title/action layout, local scrolling, focus entry/return, disabled and failed-save states. Long titles deliberately ellipsize with full text retained in their title tooltip and metadata field; long reader navigation strips remain scrollable.
- **PASS:** meaningful regression checks, production build/unit tests, before/after evidence and handoff. Final shell-height follow-ups passed after the complete browser run.

## Executed verification

| Command/check actually executed | Result |
| --- | --- |
| In `frontend`: `npm run build && npm run test` | **PASS** on the final source: Vue/TypeScript/Vite production build and **43 Vitest cases in 11 files**. Existing >500 kB chunk warning remains. [Final combined log](evidence/phase01b-20261027/build-test-final.log). Existing installed dependencies were used; no installation. |
| `COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- --output=/private/tmp/courtbundle-phase01b-full` | Initial complete run: **66 passed, 2 failed, 1 skipped**. The appearance test expected three columns at every scale; the tab-draft test showed the compact cutoff hid the list at a width where all three panels still fit. |
| Same browser command with `--grep 'Documents Details'`, output `...phase01b-layout` then `...phase01b-layout-final` | First run: one pass, one failure at 132px visible paper height. The assertion was retained and the layout corrected; second run: **2 passed**, covering all 16 combinations. |
| Same command with `--grep 'Documents Details\|appearance persists\|matter and document tabs'`, output `...phase01b-focused` | Interrupted after identifying a new test race: it clicked Close while the search shortcut was already closing the panel. Switched the test to inspect expanded state and wait for hidden state; no behavior assertion was removed. |
| Focused follow-ups, output `...phase01b-focused-final` and `...phase01b-focused-verified` | **3 passed, 1 failed** each. First failure exposed excess reader-navigation/footer padding at the existing 1280×720/125% case; corrected the layout without lowering the new height assertion. Next failure was the old search-style check querying the intentionally hidden list; moved the unchanged assertion before Details opens. |
| Same command with `--grep 'appearance persists'`, output `...phase01b-appearance` | **1 passed**. |
| `COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- --output=/private/tmp/courtbundle-phase01b-full-final` | **68 passed, 1 skipped**, exit 0, 3.4 min. All 66 previously enabled journeys plus the two new viewport regressions passed in this single full run. A subsequent shell-height-only adjustment was checked by the final focused run below, rather than another full-suite run. Opt-in encrypted-vault setup remains skipped. [Log](evidence/phase01b-20261027/browser-full-final.log). |
| Same browser command with `--grep 'Documents Details\|appearance persists\|matter home leads\|workspace and export controls\|matter and document tabs'`, output `...phase01b-shell` then `...phase01b-shell-final` | Visual review exposed viewport units being enlarged again by CSS zoom. First follow-up: **5 passed, 1 failed** on the added vertical-overflow check; matched bottom padding to the workspace's physical reserve without relaxing the check. Final follow-up: **6 passed**, including the entire 16-combination matrix, appearance, full screen, narrow pages and tab drafts. [Final shell log](evidence/phase01b-20261027/browser-shell-final.log). |
| From repo root: `node docs/evidence/phase01b-20261027/capture.cjs before` | **24 before captures**, including Details open at 100% and 125%, captured before application changes. |
| `node docs/evidence/phase01b-20261027/capture.cjs after` | **PASS**: 48 final captures (32 matrix/Details, two long-title Fit width, 14 state captures), plus 24 before images: **72 PNGs**. All 16 combinations visually reviewed. Zero horizontal/blank vertical page overflow; eight matched light/dark PDF raster hashes identical; local Inter loaded; focus, disabled/selected/dirty/error/retry states passed. [Evidence index](evidence/phase01b-20261027/README.md). |
| `git diff --check` | PASS. |

The final browser suite covers imports, metadata autosave/failure and dirty guards, tab drafts, ordering, page operations, OCR requests, Undo/Redo, index/page map, PDF/ZIP and Office downloads, print preparation, Save copy, large-file range rendering, worker fallback and reader positions. The new test uses a deliberately long persisted document title, checks both Details close paths and search from the open panel, verifies the existing reader remains mounted and exercises page navigation after closing. Its usable-PDF threshold is **over 200×180 physical pixels at Fit width**, intersected with every clipping ancestor and checked for occlusion. Header controls remain reachable. No test merely restates token values.

**Unresolved limitations:** Pencil frame access (22A/22F and native frame-level fidelity); no Tauri/WKWebView runtime, native compilation, packaged sidecar, native Save/print, Windows/macOS Intel, encrypted-vault runtime, clean-device or full accessibility qualification. Backend code is unchanged; historical backend test results are not a new run. Browser evidence is synthetic Chrome evidence, not court approval or verification of OCR/signatures.

The current `.gitignore` excludes both the design kit and `docs/evidence`; that user policy is preserved. Evidence exists locally at the linked paths and is not staged or committed automatically. No Phase 02 work was started.
