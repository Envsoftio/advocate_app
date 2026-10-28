# Phase B01A — Persistent bundle membership and BU05

28 October 2026. **B01A implementation and scoped browser/visual acceptance complete, with the deliberate differences below.** Scope is B01A; B01B and B01C have not been started.

## Design and implementation

BU05 is implemented as a full workspace picker over the caller, using the supplied JSON export and `implementation/previews/BU05.png`. Pencil access reported a plan limit; no Pen file was read or modified through the filesystem. The supplied JSON reader resolved BU05's reusable components and descendant overrides with no warnings. Shared semantic tokens and bundled Inter are reused.

The reference composition is a 32 px app bar, 40 px workspace tabs, 44 px utility bar, 184 px expanded sidebar, 28 px status bar, 24 px content gutters, 34 px controls, 36 px table header and 40 px rows. Counts, titles, groups, page counts and revisions come from the isolated runtime store. Compact mode collapses the sidebar, retains 13 px operational text and scrolls the table. Theme/scale checks cover 1440×900 and 1024×768, light/dark, 90/100/110/125%.

The picker supports included/reference-only filters, live counts, draft selection, Apply, discard/cancel, caller return and focus restoration. Apply failure retains selection. A stale input snapshot disables Apply until explicit discard/reload. Opening another workspace or a source uses the existing Save/Discard/Keep editing dialog. The caller's document identity and reader remain mounted. Index and Build have a visible inclusion summary; all matter workspaces have a status-bar membership entry.

## Persistence and engine contract

- SQLite schema **19** adds `documents.bundle_included` and `bundle_membership_state`. Migration includes all existing documents, preserving prior effective selection; archived records stay archived. No original or working PDF is rewritten.
- Both multipart and streamed imports default to **reference-only**. A caller can explicitly request `?included=true`; browser uploads stage by default and explain this after import. Existing export-test fixtures now explicitly request inclusion. Explicitly generated draft covers retain the existing included behavior.
- `GET|PUT /matters/{id}/membership` uses the existing processing lock and a build-input fingerprint as the optimistic write guard. Membership changes are one SQLite transaction with audit and Undo/Redo; they touch the matter and change the fingerprint. Previous output bytes stay frozen and become historical/stale.
- This bounded task has one primary whole-document bundle per matter. Membership follows existing document order and current working revisions. The recorded build membership snapshot identifies the bundle, membership revision, ordered document IDs, working revisions and source-page counts. Source-page identity remains **document ID + working revision + source page index**, not a claim of stable page lineage across edits. Partial-page supplements and multiple variants are outside B01A.
- `engine.py` remains the only page-map authority. Preview, technical checks, PDFs, CSV, indexes, outlines and Part extracts consume included documents. PDF marks on reference-only documents do not enter generated PDFs.
- An internal reviewed link from included material to excluded material blocks the build until retargeted, removed, or its destination included. A manual bookmark to excluded material is unresolved in the map and omitted with an explicit report warning. Nothing redirects to a different page.
- Portable package **15** retains membership and membership revision; versions 1–14 restore their prior all-included semantics. Metadata, originals, revisions, archives and prior builds remain retained. Existing broad freshness behavior is conservative: some reference-only changes can still stale a prior build.

## Changed paths

Backend: `app/storage.py`, `schemas.py`, `main.py`, `engine.py`, `portable.py`; new `tests/test_bundle_membership.py`; migration and existing export fixture tests.

Frontend: new `components/BundleMembership.vue`, `membership-design.css`; `App.vue`, `lib/types.ts`, `main.ts`; new `e2e/bundle-membership.spec.ts`; explicit-inclusion setup in existing browser fixtures and `e2e/navigation.ts`; one typed unit-test fixture.

Unrelated pre-existing changes, supplied design files, Documents/reader layouts and source-processing implementations were preserved. No commit or deployment.

## Deliberate differences from the illustrative screenshot

- Runtime matter tabs, names, groups and counts replace synthetic design examples. Technical Parts are not assigned invented legal names.
- Review says **Not checked**, not the design fixture's **Ready**. B01C's named artifact validation has not been implemented, and membership alone cannot establish readiness.
- Title/date suggestions, variant review and native matter-folder opening remain visibly unavailable with explanatory tooltips; no unsupported workflow is represented as working. Existing metadata review, Storage and all current editor tools remain available through their existing routes.
- Browser window controls are platform-owned; no decorative minimize/maximize/close buttons are fabricated. A real caller-return action is retained in the status bar.

This is a scoped BU05 layout/behavior implementation, not a claim that every application screen or every pixel of a synthetic screenshot is identical.

## Verification

- Backend: **251 passed**, 6 dependency deprecation warnings, 39.31 s. Seven new B01A tests cover reference-only default/reopen/editability/original bytes; atomic stale/foreign/duplicate rejection; real multi-volume PDF/index/CSV/link/outline ranges and prior-byte retention; excluded destinations; Undo/Redo/portable restore/empty builds; queued-build rejection; and excluded-signature behavior.
- Production Vue/TypeScript/Vite build and **44 Vitest tests passed** on final source. The pre-existing chunk-size warning remains.
- Full Chrome suite: **88 passed, 1 opt-in vault skip**, 5.3 min. Final review then added keyboard/recovery handling, filtered signature acknowledgments to included documents, and made pending Cancel visible beside Apply. The subsequent scoped run passed **13/13**, including all membership tests, Documents/reader geometry and all existing document-state guards. Two final visual-only corrections fixed the Saved icon color and status alignment; **4/4 BU05 tests passed again** on final source. This is not a claim of a full-suite rerun after those final changes.
- Browser evidence uses isolated synthetic stores, backend 8768 and Vite 1436. It includes actual browser upload → reference-only staging → membership Apply → build → ZIP download with byte equality; reference-only source reading and original-byte comparison; loading, empty, pending, failed-save, stale and disconnected/recovered states; keyboard filters/checkboxes, caller focus and no implicit save replay.
- The current PyInstaller Apple Silicon sidecar and **69.70 MiB Tauri `.app` built successfully**. Native UI, native save/print dialogs, Windows, Intel Mac and clean-device checks were **not run** in this pass. Reopen the rebuilt app to load its new frontend.
- All membership semantic roles resolve. `git diff --check` passes. No new dependencies were needed.

Commands actually run (repeated after relevant corrections):

```sh
cd backend && .venv312/bin/python -m pytest -q
cd frontend && npm run build && npm run test
COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- --output=/private/tmp/courtbundle-b01a-full
COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- bundle-membership.spec.ts json-design-workspace.spec.ts reader-design.spec.ts documents-layout.spec.ts document-state.spec.ts --output=/private/tmp/courtbundle-b01a-final
COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- bundle-membership.spec.ts --output=/private/tmp/courtbundle-b01a-evidence
# From repository root:
backend/.venv312/bin/python backend/scripts/build_sidecar.py
# From frontend:
npm run tauri:build -- --bundles app && npm run test
```

The first browser launch was denied loopback binding by the sandbox and was rerun with local-server approval. Earlier implementation checks found a wrong API helper call shape and a typed fixture omission; initial backend fixtures assumed implicit inclusion and were changed to explicitly include their export sources. The first visual run exposed missing caller focus and the Apply completion-order defect; both were fixed. Two npm commands were accidentally run from the repository root and failed before doing work; the actual builds ran from `frontend`. No failing assertion was weakened.

Final captures were visually inspected against BU05.png at 1440×900 and checked at compact/dark sizes. The JSON container coordinates are preserved; the supplied preview renderer and Chromium have minor border/font rasterization differences, and runtime content differs as described above.

[BU05 / 1440 light](ui-evidence/b01a/BU05-1440-light.png) · [1440 dark](ui-evidence/b01a/BU05-1440-dark.png) · [1024 light](ui-evidence/b01a/BU05-1024-light.png) · [1024 dark](ui-evidence/b01a/BU05-1024-dark.png) · [Failed save](ui-evidence/b01a/BU05-save-error.png) · [Stale draft](ui-evidence/b01a/BU05-stale.png) · [Empty matter](ui-evidence/b01a/BU05-empty.png) · [Source/evidence hashes](ui-evidence/b01a/verification.json).

Next bounded task after B01A acceptance: **B01B**.
