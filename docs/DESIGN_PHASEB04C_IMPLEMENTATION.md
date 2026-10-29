# B04C implementation handoff — audience variants

Date: 29 October 2026

**Status: DONE for the available Apple Silicon Mac target.** [Certain] Functional and BU11 visual checks, native PDF rendering, Save cancellation/retry, byte reconciliation and app-plus-sidecar restart pass. Final verification: 327 backend tests, 48 frontend unit tests, production build, and 124 browser tests passed (one existing opt-in vault skip).

## Implemented scope

- SQLite schema 21 stores each named variant as an independently versioned membership/settings plan. A primary baseline records the real source snapshot and matching primary build identity; a variant baseline records the selected plan/version. Source IDs, working revisions, page IDs and hashes are shared by reference. Imported originals are never copied or edited.
- Create/select and Apply/Cancel carry the selected variant through membership, settings, page-map preview, differences, technical Preflight, build, output checks, PDF review, export/Save and the UI's active-plan state. Dirty edits are guarded. Runtime document/page counts replace BU11's fixture numbers. Exclusion, pending PDF redaction marks and applied content removal remain separate concepts.
- The build endpoint checks the plan version, exact contract and source snapshot before queueing and again in the worker. `backend/app/engine.py` builds the selected ordered membership and settings through the shared deterministic page map. Manifest, PDF, CSV, bookmarks, links, labels, volume ranges and validation bind to the issued snapshot. Each build has a distinct immutable directory and independent history; earlier primary/variant artifacts remain intact.
- The BU11 two-card screen replaces the previous variant workflow layout. It includes a saved-variants table, selection details, creation form, plan review actions and compact/light/dark/scale adaptations. The UI describes plans and technical checks without claiming filing or court approval.

## Changed paths

- Backend: `backend/app/storage.py`, `backend/app/variants.py`, `backend/app/schemas.py`, `backend/app/main.py`, `backend/app/engine.py`; tests in `backend/tests/test_audience_variants.py` and `backend/tests/test_storage_migration.py`.
- Frontend: `frontend/src/components/VariantWorkspace.vue`, `frontend/src/variant-design.css`, `frontend/src/components/BundleStructure.vue`, `frontend/src/App.vue`, `frontend/src/lib/types.ts`, `frontend/src/main.ts`, `frontend/src/lib/nativeAnimationFrames.ts` and its four unit tests, `frontend/src-tauri/tauri.conf.json`; browser coverage in `frontend/e2e/audience-variants.spec.ts`.
- Documentation: this handoff, `docs/IMPLEMENTATION_TASKS.md` and `docs/ACCEPTANCE_TESTS.md`.

## BU11 visual evidence

`CourtBundle_JSON_For_Codex/implementation/previews/BU11.png` and its JSON frame, descendant component references, overrides and interactions were inspected. The supplied resolved light/dark semantic tokens informed the implementation; current product wording is MatterDraft. At 1440×900, the running cards measure x=208/y=238/w=740/h=588 and x=968/y=238/w=448/h=588, matching the reference geometry. Sixteen screenshots are in `docs/ui-evidence/b04c/`: 1440×900 and 1024×768, light and dark, at 90/100/110/125%.

The intentional compact adaptation retains two cards, lets the table scroll horizontally, reduces the navigation rail, and scrolls lower form/detail content within the viewport. The 1440 screenshot has two saved rows because the browser test creates a real synthetic variant; the design's example counts and CourtBundle label are historical reference data. The running captures were inspected, including 1440 dark/100 and 1024 light/125.

## Verification record

| Check | Actual result |
|---|---|
| `cd backend && .venv312/bin/python -m pytest -q` | **327 passed**, six dependency deprecation warnings, 67.42s. Includes persistence, stale/version conflict, exact membership, map/CSV/links/labels/PDF reconciliation, restart, independent artifacts and original/prior-output hashes. |
| `cd frontend && npm run build && npm run test` | **Pass; 48 unit tests passed.** Existing large-chunk advisory remains. |
| Final complete browser suite | **124 passed, 1 opt-in vault skip**, 7.8m, after the final native/frontend fixes. `MATTERDRAFT_E2E_VITE_PORT=1438 npm run test:e2e`, default backend port 8768. [Log](ui-evidence/b04c/browser-final.log). |
| Final focused B04C browser suite | **3 passed**, 15.9s; all three also pass in the final complete suite. Covers create/select/differences, Apply/Cancel and focus, build/check/review/export with SHA match, reload, keyboard and responsive visual matrix. |
| `.venv312/bin/python scripts/build_sidecar.py` | **Pass**, PyInstaller 6.22.3 / Python 3.12.13, arm64 sidecar SHA-256 `03db7f7f4e319361ef32fcab1693568de8be897870a709e8a4ffb60bd91dc359`. |
| `npm run tauri:build -- --bundles app` | **Pass**, unsigned 70.17 MiB Apple Silicon app; executable SHA-256 `af5ab09a898d36a34ae87f5f96ca53e4af9a97cb2fc101244db9de2245a4515d`. A separate 37.05 MiB debug QA app was built for native checks. |
| Native synthetic plan/build/checks | **Pass** in the isolated QA session: version 3, two included sources/six source pages, cover and index, eight physical pages, distinct build `de57d0ed…`. Output checks reported reopened files, matching counts and consistent links/bookmarks/labels. |
| Native Save cancellation/retry and hashes | **Pass**: cancelled Save reported unchanged build; retry saved `/private/tmp/matterdraft-b04c-native-variant.zip`, SHA-256 `1f1e8de747a432425c47006767c29bf485ddd842a60d665dbe03f12b9c0c4d99`, exactly matching the stored ZIP. Primary PDF and both imported originals retained their prebuild hashes. |
| Native PDF visual review | **Pass**: exact saved build `de57d0ed…` renders raster and selectable text. After restart, the variant-only source bookmark opens physical/printed/combined page 6. [Screenshot](ui-evidence/b04c/native-restarted-review.png). |
| Native quit/relaunch UI | **Pass**: app and frozen sidecar restarted against the isolated synthetic store; selected plan v3, primary baseline `141b31c2…`, independent output `de57d0ed…`, membership/settings and eight-page history restored. |
| Final native PDF Save | **Pass**: Cancel reported unchanged build; retry saved 78,985 bytes, SHA-256 `e62abfbd783daaa3ace3f39fa2217e8318dd0b30093caaa10be4385cd7cac95e`. All five manifest artifact sizes/hashes, two originals and prior primary PDF reconcile after restart. [Evidence](ui-evidence/b04c/native-reconciliation.json). |

The earlier blank native surface was caused by stalled WebKit animation-frame delivery. macOS Tauri now disables background throttling, and a native-only 100 ms timer fallback delivers a pending frame exactly once when WebKit stalls. Normal frames and cancellation retain their behavior; four unit tests cover the race, cancellation, cleanup and reentrant PDF continuation. The outer native welcome transition also disables CSS animation. These changes preserve PDF.js display rendering and do not alter output bytes. A final embedded debug QA bundle exercised the same frontend as the rebuilt release app and the matching frozen sidecar.

The earlier wrong-workspace relaunch was a QA launch-context error: the automation launched another instance without the explicit synthetic backend environment. Final verification launches one registered GUI process with the isolated endpoint/token, waits for the sidecar to be ready, and then connects native automation. An earlier launch before sidecar readiness temporarily lacked output history; it is not counted as the successful restart. General cold-start/reconnect timing remains a separate platform reliability limit.

The nonstandard-port browser attempt ended with **83 passed, 34 failed, 1 skipped, 7 not run** (interrupted); legacy fixtures contacted default port 8768 while that run served 8797. These failures are not counted as passes. The final standard-port run is recorded separately.

## Remaining platform limits

Windows x64, Intel Mac, clean-device/minimum-macOS, production encrypted-vault startup, native Office/print and broader accessibility checks remain unverified. Background-throttling configuration is supported on macOS 14+; the fallback was exercised on this available Mac only. Portable backup inclusion remains unchanged from B04A. The installed app and real matters were untouched. No technical check implies legal completeness, verified OCR/signature, court approval or filing acceptance. No next phase was started.

`git diff --check` passed after final documentation updates. B04C is complete within the available-target scope; no next phase began.
