# Design Phase 01A — tokens and shared Documents controls

27 October 2026. **01A complete; Phase 01 in progress. 01B not started.**

[Certain] This is the scoped tokens/controls slice requested by the owner. It supersedes the broader proposed first-task scope in the Phase 00 implementation map: the complete 05A layout and compact adaptations belong to **01B**. No Phase 02 work was begun.

## Implementation

- All **139 semantic roles × two themes** are represented in `frontend/src/design-tokens.css`. Names are prefixed `--cb-`; dots become hyphens and case is retained (for example `status.warning.text` → `--cb-status-warning-text`). Values were compared with both supplied JSON files. Sizes, spaces and radii use px; weights and line height are unitless. The `type.scale125.*` values remain available as references, without multiplying the existing CSS zoom a second time.
- Existing `--panel`, `--surface`, `--surface-soft`, `--page-bg`, `--border`, `--muted`, `--primary` and `--text` aliases resolve to the supplied roles. Theme selection still uses `data-effective-theme`; appearance storage and the supported 90/100/110/125% scales are unchanged.
- Inter 4.1 variable upright is bundled locally and actually loaded in Chrome. Its unmodified SIL OFL license is in `frontend/public/licenses/Inter-OFL.txt`, copied by Vite into the production distribution. Font provenance and hashes are in `frontend/src/assets/fonts/README.md`. No runtime font service was added. The old unused Manrope package remains in the dependency manifest to avoid unrelated dependency churn.
- Existing buttons, fields (including OCR/reader controls), tabs, document/table selections, notices, status labels, borders, surfaces, hover and focus states use the semantic roles. Pale success/warning/danger surfaces use their dedicated status foregrounds in both themes. Metadata's existing saved/dirty/saving/failed state now supplies presentation classes only; no save behavior was changed.
- UI font rules do not target PDF-authored content or mark fonts. Paper remains white in both themes. Eight matched light/dark PDF canvas hashes (two sizes × four scales) were identical.
- No component library, navigation, handler, API, schema, backend, PDF feature or layout breakpoint was added. The existing PDF engine, dirty guards, autosave, page tools, OCR, history, index and exports remain in place.

## Changed paths

| Path | Purpose |
| --- | --- |
| `frontend/src/design-tokens.css` | Complete supplied theme mapping, 278 declarations |
| `frontend/src/styles.css` | Existing theme aliases, local font face, shared control presentation; no Documents grid or compact-layout rewrite |
| `frontend/src/main.ts` | Import semantic tokens; remove Manrope font import |
| `frontend/src/App.vue` | Class binding on the existing metadata save status only |
| `frontend/src/assets/fonts/InterVariable.woff2`, `README.md` | Local variable UI font and provenance |
| `frontend/public/licenses/Inter-OFL.txt` | Full font license shipped in production assets |
| `frontend/e2e/journey.spec.ts` | Update obsolete Manrope expectation; replace affected old-palette assertions with contrast/hover checks, polling through the existing theme transition |
| This handoff; `docs/POC_ACCEPTANCE_TESTS.md`; `docs/evidence/phase01a-20261027/` | Scope, execution results, reproducible capture script, metrics, logs and PNGs |
| `CourtBundle_Codex_Phases/phases/01_visual_foundation/TASK.md` | Local kit status: only 01A complete; overall Phase 01 in progress |

Unrelated pre-existing `.gitignore` changes, Phase 00 documents/evidence and untracked master `.pen` were preserved. The kit is ignored by the existing local `.gitignore`; this tracked-location handoff records its status without altering ignore policy. No commit, deployment or native package rebuild was performed.

## Executed verification

| Command/check | Result |
| --- | --- |
| `cd frontend && npm run build && npm run test` | Final Vue/TypeScript/Vite build PASS; **43 Vitest cases in 11 files PASS**. Inter is emitted as a local 352.24 kB WOFF2; the license exists in `dist/licenses/`. Existing >500 kB chunk warning remains. |
| One-off comparison of CSS declarations against `shared/design-tokens.json` and `shared/resolved-tokens.json` | **139/139 roles in each theme**, identical values and reversible names. No token-value unit test was added. |
| `cd frontend && COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- --output=/private/tmp/courtbundle-phase01a-e2e-20261027` | Initial full run: **63 passed, 3 failed, 1 skipped**. Two failures were obsolete Manrope/old-palette expectations; the third was the production-worker asset-observation assertion, although the PDF rendered. |
| Same command with `--grep 'dragging document, bookmark\|appearance persists\|production PDF reader renders'` and output `...phase01a-focused-20261027` | Drag ordering and production worker fallback **passed**. Appearance's new immediate contrast assertion caught the existing 100 ms background transition; changed it to poll for settled contrast. Worker implementation/test was not changed. |
| Same command with `--grep 'appearance persists'` and output `...phase01a-appearance-20261027` | **1 passed**, including persistence, 125% zoom, keyboard search, dialogs and system-theme transitions. Across the full run and focused reruns, all **66 enabled browser cases passed**; this is not a claim of a fresh all-green full-suite run. The opt-in encrypted-vault case remains skipped. |
| `node docs/evidence/phase01a-20261027/capture.cjs before` | **20 before captures**: two viewport sizes × light/dark × four scales, plus Details-open at 100%. |
| `node docs/evidence/phase01a-20261027/capture.cjs after` | Same matrix plus real clean/dirty/failed/recovered metadata states; **PASS**, including local Inter load, keyboard search focus, successful retry persisted to backend, and eight theme-independent PDF raster comparisons. |
| `node docs/evidence/phase01a-20261027/capture.cjs after --states-only` | Final state captures, focus on Save after dismissing the notification, selected table rows; saved/failed status contrast above 4.5:1 in both themes. **PASS**. |
| `git diff --check` | PASS. |

All browser checks used Chrome, temporary synthetic stores and samples only. The backend stayed on **8768**; the capture used Vite **1438**, existing specs **1436**. The capture script shuts down its servers and removes its temporary store. The initial sandbox capture could not use local networking; permitted runs outside the sandbox completed. Two accidental npm invocations from the repository root failed with missing `package.json`; the reported build/test results are from `frontend`. No installation was needed.

The browser coverage includes real import, metadata autosave/dirty guards, sorting and drag order, working page edits, OCR request integration, Undo/Redo, index/map, generated PDF/ZIP bytes, reader behavior and Save copy. No backend code changed, so the historical **239-pass** Phase 00 pytest result was not presented as a new run.

## Visual evidence and limits

[Capture index, screenshots and logs](evidence/phase01a-20261027/README.md). Both themes and all four scales were inspected, with focus/selection/disabled/error labels checked. The 1440×900 screenshots show the existing layout with the supplied control colors and Inter, not completion of the 05A design hierarchy.

**Known 01B defects remain:** at **1024×768, opening Details hides the PDF behind the constrained/overflow-clipped reader area**. At 1024×768/125%, the page also overflows horizontally in both before and after captures; narrow reader title/actions clip, and high-scale tool rows leave very little paper area. These are existing layout constraints, not accepted compact behavior. No “no clipped controls” or full Phase 01 compact acceptance is claimed. Current labels are readable where exposed, but long labels still use the existing truncation/scrolling behavior. Full accessibility qualification is not claimed.

Pencil `get_app_state` was rechecked against the scoped `pilot.pen` path and returned the account plan usage limit. **00A/05A frame inspection through MCP remains blocked.** Used only the supplied `00A.png`, `05A.png`, semantic and resolved tokens. No `.pen` file was read, parsed or modified outside Pencil; no restriction was bypassed. Detailed frame fidelity remains unverified.

No Tauri/WKWebView runtime, native Save/print, native compilation, sidecar packaging, Windows/macOS Intel, clean-device or encrypted-vault runtime checks were performed in this pass. No claim of court approval, OCR accuracy or verified signatures is made.

**Next bounded task: 01B**, only when separately requested. Complete the Documents hierarchy and compact/dark adaptations, explicitly fixing the Details-open reader defect and rechecking every supported scale. Stop here; Phase 02 is untouched.
