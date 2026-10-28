# Completed Phase 01/02 — JSON fidelity correction

28 October 2026. User-directed audit of the completed visual foundation and reader against `CourtBundle_JSON_For_Codex`. This supersedes the layout conclusions of 02D where noted below. It does not start B01 or the later editor/build phases.

## Audit and implementation

The supplied JSON and PNGs were read from the filesystem, without Pencil or a credit-dependent design service. References: Phase 01 `pilot.json` (05A), `adaptations.json` (22A/22F/23B); Phase 02 `read.json` (ED01/ED01I/ED22/ED23/ED24), `state_contracts.json` and `adaptations.json` (ED36/ED40). Component references/overrides were inspected with the supplied JSON reader. The existing local Inter font and 139 semantic roles remain the source for colors and typography.

| Completed area | Finding | Correction |
| --- | --- | --- |
| 01A visual tokens | Existing local tokens/font were already integrated. | Reuse them; new CSS references were checked for undefined roles. |
| 01B Documents | 02D had replaced 05A with an editor and overlay document list. A separate preparation workspace was missing. | Restore the 32 px application bar, 40 px workspace tabs, 44 px utility row, 64 px rail, 280 px source list, 824 px preview and 272 px Details panel at 1440×900. |
| 02A reader | Wide navigation stayed at compact 200 px; revision metadata was absent beside the version selector; reading modes were hidden in View. | 232 px wide navigation / 200 px compact; live revision and page count; direct Single page/Continuous buttons at wide sizes. Compact mode retains View. |
| 02A information/focus | Working-document information appeared on the left; focused reading used a different header and removed document identity. | ED01I information docks right at 292 px with thumbnails retained. ED24 keeps identity/modes, removes the rail and panels, and floats page/zoom controls over the canvas. |
| 02B state and guards | Existing API/revision/immutable-artifact guards are reusable. | Retain them. Leaving a pending editor draft for Documents uses the existing Save/Discard/Cancel guard. Search can open the document switcher without leaving the draft. |
| 02C adaptation | Documents needed actual compact panel behavior. | Collapse preparation side panels when space is constrained, allow explicit reopening and Escape/focus return, and avoid opening both compact side panels together. |

The Documents rail action and editor's Documents/Back to preparation actions open the preparation workspace. Selecting a source previews it with real metadata; Open editor enters the existing editor without remounting its PDF or losing the page. Search, technical Part filtering/collapse, keyboard/drag ordering within each Part (other Part positions retained), import, table/sort review and cover creation reuse existing services. Groups show technical Part numbers rather than inventing required legal Part names. The legacy text-search/archive drawer and supported editor tools remain reachable.

Metadata uses the existing persisted fields and autosave/explicit Save. Signature details are a disclosure and Save sits in a fixed panel footer. Original/generated versions retain their existing immutable readers. No source document text, fixture counts, fake revisions, legal approvals or future capability success states were added to the product.

The focused-reading verification exposed a genuine 4 px scroll drift: the viewport captured anchors assuming 20 px top padding, while the design uses 24 px. `PdfReaderViewport.vue` now reads the actual rendered padding. The existing exact-scroll restoration regression verifies the fix.

## Changed implementation paths

- `frontend/src/App.vue`: preparation/editor transitions, shared workspace tabs/utility row, metadata panel and retained state guards.
- `frontend/src/components/DocumentNavigator.vue`: real document navigation, Part filtering/collapse, import and existing review actions.
- `frontend/src/components/PdfViewer.vue`, `PdfReaderControls.vue`: identity, wide reading-mode controls, information/focus composition and transitions.
- `frontend/src/components/PdfReaderViewport.vue`: rendered-padding anchor capture.
- `frontend/src/documents-design.css`, `main.ts`: scoped design geometry and compact behavior over existing tokens.
- `frontend/e2e/json-design-workspace.spec.ts`: reference geometry, actual metadata persistence/source hash, page/PDF-instance continuity, information/focus and viewport/theme/scale matrix. Existing navigation helpers/journeys use the actual document switcher interaction.

Pre-existing working-tree edits were preserved. No backend/schema/page-map changes, commit or deployment were made by this correction.

## Verification

- Final Vue/TypeScript/Vite build and **44 Vitest tests passed**. The existing large-chunk warning remains.
- `cd backend && .venv312/bin/python -m pytest -q`: **244 passed**, six dependency deprecation warnings, 37.56 s. No backend source was changed in this task.
- `backend/.venv312/bin/python backend/scripts/build_sidecar.py`: built the current Apple Silicon sidecar, including PDFium and English/Hindi OCR models.
- `cd frontend && npm run tauri:build -- --bundles app`: release Rust compile and **69.69 MiB Apple Silicon `.app` passed**. The historical application bundle was built under `frontend/src-tauri/target/release/bundle/macos/`.
- The bundled sidecar was launched against a disposable empty vault on a random local port; authenticated `/health` returned HTTP 200 and `status: locked`. It was stopped and the temporary store removed. This is a sidecar startup smoke check, not a native UI or save-dialog test.
- Full Chrome run: **85 passed, 0 failed, 1 opt-in vault skip**, 4.8 min, on the completed functional source. Two final CSS specificity fixes then restored 36 px metadata inputs and visible selected reading modes. Final focused checks on the exact delivered source: **4 passed**, 47.1 s, including both viewport/theme/scale matrices, reference geometry, real metadata/source-hash checks, Part ordering, Sort/table entry, PDF continuity and information/focus layouts. The final app/build/44 unit tests were rerun after those CSS fixes. This is not a claim that the full suite was rerun after the two CSS-only fixes.
- All new semantic-token references resolve; the bundled Inter license exists in `dist/licenses/`; `git diff --check` passes.

Commands from `frontend`:

```sh
COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- --output=/private/tmp/courtbundle-json-ui-final
COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- json-design-workspace.spec.ts reader-design.spec.ts documents-layout.spec.ts --output=/private/tmp/courtbundle-json-ui-evidence
npm run tauri:build -- --bundles app && npm run test
```

Final screenshots were visually inspected against the reference hierarchy and viewport. [Documents / 05A](ui-evidence/02e/05A-documents-1440-light.png) · [Dark Documents](ui-evidence/02e/documents-1440-dark.png) · [Compact Documents](ui-evidence/02e/documents-1024-light.png) · [Reader](ui-evidence/02e/ED01-reader-1440-light.png) · [Information / ED01I](ui-evidence/02e/ED01I-document-information.png) · [Focus / ED24](ui-evidence/02e/ED24-focused-reading.png) · [Immutable original](ui-evidence/02e/original-1440-light.png) · [Verification and source hashes](ui-evidence/02e/verification.json).

Earlier corrective runs found the inherited inspector positioning, a compact-panel initialization race, the 4 px reading-anchor drift, and test selectors for the superseded rail. These were fixed without lowering geometry, readable-paper or state assertions. The first full browser pass returned **83 passed / 2 obsolete Matters-navigation selectors failed / 1 opt-in vault skip**. The updated menu interactions and new Part-order/table checks subsequently passed in a four-case focused run. Sandbox-denied localhost starts and two commands issued from the wrong directory were rerun from the correct directory with local-server permission.

## Deliberate limits

The JSON is broader than the completed phases. Compare/read pair, new bundle flows and later editing capabilities are not enabled by this correction. Matter-wide search and document-level freeform Notes do not have completed services here; the toolbar accurately exposes the existing document/page-text search, and private page comments remain available in the editor. PDF paper and text come from the imported synthetic documents, not the design's illustration. Browser titlebars do not imitate OS window buttons.

Native build success does not establish native runtime, OS save/print-dialog, Windows, Intel Mac, clean-device or legal acceptance. A running older `.app` must be quit and the rebuilt app reopened to display its new bundled frontend.
