# Phase B01B — Build, review and export

28 October 2026. **B01B scoped implementation, browser/visual verification and available Apple Silicon native Save acceptance complete**, with the capability adaptations and platform limits below. B01C is next.

## Design contract and capability mapping (before implementation)

Source: supplied `CourtBundle_JSON_For_Codex/implementation/phases/B01_bundle_build_foundation/{build_journey,prior_outputs}.json`, component definitions and descendant overrides; shared light/dark semantic tokens; 15A and 18A PNG previews. Existing B01A completion takes precedence over kit status labels.

| Screen | Component / operation | Control disposition |
| --- | --- | --- |
| 15A Settings | New BuildWorkspace; persisted matter settings, OutputPresets and engine preview | Presets, Preview, numbering, output options and volume planning open focused tools; Build saves settings then submits actual job. Mandatory engine outputs/navigation are read-only indicators, not pretend toggles. Membership opens BU05. |
| 15B Progress | New BuildWorkspace with durable JobManager status | Actual progress/message, continue working, cancellation pending until terminal backend state; prior build retained. No invented per-stage completion or time estimate. |
| 16A Failure | BuildWorkspace terminal state | Real error/details, preparation/checks and new attempt; interrupted jobs use supported one-time retry. No invented affected-page identity. |
| 17A Complete | BuildWorkspace artifact table | Review exact PDF, export dialog, rebuild, persisted build picker. Named output QA unavailable until B01C; no readiness claim. |
| 18A / 18C Review | ExportPdfReview with generated-specific composition | Read-only real PDF, bookmarks, page/zoom/search, build identity, export, stale banner. Frozen page-map source navigation only when revision still matches. |
| 18B Export | New BundleExportDialog using existing downloadExport | Select saved file, native Save As/browser download, pending/cancel/error/completion; no fictitious folder path. ZIP uses engine-created package. |
| 19A Stale | BuildWorkspace stale state | Rebuild, review changes, earlier PDF. Supplements and insertion policies unavailable (B04). |
| 19B Changes | Existing comparison service in new workflow | Actual document/range/settings changes, working/earlier PDF and preflight. Side-by-side PDF comparison unavailable. |
| 20A Retrieval | New persisted output browser within Storage | Build selection, generated files, review/export; retain storage/backup/history services. Archive generated build and reveal encrypted folder unavailable. |

Shell: 32/40/44 px top rows, 184 px expanded navigation, 28 px status; content starts x208/y140 at 1440×900. Settings panels 882/310 px with 16 px gap and 518 px height. Review uses 64 px rail and 246 px bookmark panel. Compact layouts collapse navigation and scroll content; all sizes use existing interface zoom and Inter.

Existing original, working, generated and exported identities remain distinct. No new page-map rule, legal approval, cloud service or B01C release gate is introduced.

## Delivered behavior and preserved capabilities

New `BuildWorkspace.vue`, `BundleExportDialog.vue` and `build-design.css` replace the old Build composition. App wiring retains persisted volume/mark settings, presets, deterministic preview, Office drafts, print copies, protected copies, backup/history and storage maintenance behind named tools. Original and working readers retain their independent positions, search and dirty guards. Generated review retains artifact switching, focused reading and exact-build navigation within the new header/sidebar composition.

Backend processing remains UI-independent. New manifests record an effective input digest: included document order/revisions/content, applicable settings and relevant links/bookmarks/marks. Reference-only edits no longer stale a newly generated bundle. Included document modification timestamps deliberately retain the established conservative Undo/review invalidation behavior. Legacy manifests retain conservative freshness checks. Broad submission/concurrency checks remain unchanged. No volume/index rule, schema migration or page-map algorithm was introduced.

The UI saves settings before submission, blocks duplicate submission, ignores late job responses after matter/job switches and waits for backend terminal status before claiming cancellation. Durable failed/cancelled/interrupted jobs remain identifiable; retry creates a separate attempt. A previous successful artifact stays available during a later attempt. Backend tests independently inspect generated PDFs and verify original/prior-output hashes, real cancellation cleanup, failure/retry, interrupted-job recovery and reopened storage.

Native testing found a synchronous Vue out-in transition failure (`parentNode` on a removed node). Tauri now changes workspace sections without out-in/CSS animation; browser transitions remain. Actual native navigation, persisted-result reopening and generated PDF rendering were verified after that correction.

## Visual evidence and justified adaptations

Retained evidence: [contact sheet](ui-evidence/b01b/contact-sheet.png), [verification and screenshot hashes](ui-evidence/b01b/verification.json), and individual PNGs in `docs/ui-evidence/b01b/` (local evidence directory follows the repository's existing ignore policy). Browser run output: `/private/tmp/courtbundle-b01b-final`.

Settings assertions require the supplied 15A card rectangles exactly at 1440×900: `(208,272,882,518)` and `(1106,272,310,518)`. Screenshots were compared with 15A/18A previews and the bounded screen JSON. Inspected settings, actual queued/progress, actual engine failure, completion, generated review, export, stale review, update policies, changes and storage retrieval. Matrices cover 1440×900 and 1024×768, light/dark, 90/100/110/125%. Corrected compact stacking/scrolling, dark warning foregrounds, generated-reader clipping, compact search-result height and version-selector sizing. PDF content is generated test content, never a mock illustration.

Intentional capability/data adaptations:

- Required engine outputs/index/bookmarks are indicators; there are no unsupported cosmetic toggles. Output controls show the actual configured technical marks and limits.
- Progress shows the actual backend percentage/message. No invented stage checklist, remaining time, cancellation completion or unsupported frozen-input count.
- Actual errors replace the design's sample failure text; unknown affected-page identities are not invented.
- Human review remains pending and named output checks explicitly unavailable. No green readiness, filing/serving status or legal approval is manufactured.
- Generated reader uses the exact PDF; its default 75% zoom approximates the reference's physical paper width. The selector preserves original/working/generated switching in place of a purely decorative generated-output pill. Saved zoom/position still wins.
- Native export uses the system Save dialog; browser export uses browser downloads. The application does not invent a saved-folder path. ZIP exports the actual engine package. Browser success means download requested; native success is only reported after the save operation returns successfully.
- Stale rebuild is functioning; supplement/insertion preservation, named B01C checks, side-by-side PDF comparison, generated-build archive and encrypted-folder reveal remain honestly unavailable.
- Storage's in-scope generated-file retrieval is redesigned. Existing backup, storage usage, revisions and archive operations remain accessible below it. No filesystem browser or unsupported generic filters are imitated.
- On compact screens cards stack and scroll; PDF navigation can scroll horizontally, and search occurrences have bounded height. Focus, keyboard escape/return and readable PDF space remain tested.

## Native Save evidence

Used a separate `CourtBundle B01B QA.app` identifier and a disposable synthetic store/engine at port 8772; the existing user engine at 8765 was not used or stopped. The QA runtime uses the real Tauri native download command against a development backend. It is not clean-device or packaged-vault runtime qualification.

Observed macOS system Save dialog cancellation → “Save cancelled. The generated build is unchanged.” Observed successful Save As to `/private/tmp/courtbundle-b01b-native-export.pdf` → “combined.pdf saved.” Independently reopened that saved PDF: **4 pages, 3 bookmarks, 1 index link**. SHA-256 **`0ae4295ef094ca115ee6464ddf6c1b11144d164a0663a109665ef8c8c0e57e4a`** matches the selected manifest exactly. A copy is retained as `docs/ui-evidence/b01b/native-saved-combined.pdf`.

For a real failure, stopped only the disposable engine after opening Save As and selected a separate output name. Native export reported the connection error and offered Retry export, without success. Reconnected explicitly from the reader's Retry connection control after restarting that engine; no export was automatically replayed. When an outage also blocks the workspace, close export and reconnect before retrying. Native startup restored the same persisted build and its readable generated index after application restart.

## Verification log

- `cd backend && .venv312/bin/python -m pytest -q`: **254 passed**, six existing deprecation warnings, 39.91 s. Includes the three new B01B workflow tests. An earlier run exposed the historical Undo freshness expectation; the final effective digest preserves it.
- `cd frontend && npm run build && npm run test`: **PASS; 44 unit cases passed** on delivered source. Existing Vite bundle-size warning remains.
- Initial full browser regression: **87 passed, 4 failed, 1 opt-in vault skip**. Restored generated artifact switching/focus controls, corrected `display:contents` overflow, and updated stale-result navigation to the actual new design.
- Focused reader follow-up: **12 passed, 1 failed**; the compact search panel left insufficient visible paper at 125%. Reduced compact occurrence-list height; did not weaken the usable-paper assertion.
- `COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- --output=/private/tmp/courtbundle-b01b-final`: **91 passed, 1 opt-in vault skip**, 5.6 minutes, no failures. No implementation code changed after this run.
- `.venv312/bin/python backend/scripts/build_sidecar.py`: current sidecar built successfully, including bundled OCR languages/PDFium.
- `cd frontend && npm run tauri:build -- --bundles app`: **PASS; 69.72 MiB Apple Silicon release app**, rebuilt from delivered source. Bundled sidecar SHA-256 matches the current source-side binary; recorded in verification JSON.
- Additional late-result test: `COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- e2e/build-workflow.spec.ts -g "late build response" --output=/private/tmp/courtbundle-b01b-late-result`: **1 passed**. Delayed an actual backend job response across a matter switch; verified it cannot publish completion into the new matter. This test was added after the full run, with no implementation change.
- Separate native QA bundle built and launched; system Save success/cancel/failure and actual generated reader verified as above.
- `git diff --check`: passed. Existing unrelated working-tree edits preserved; no commit or deployment.

Windows, Intel Mac, macOS minimum-version/clean-device installation, packaged-vault native runtime, native print and Office Save dialogs remain unverified in this pass. The opt-in vault browser scenario is outside the normal unencrypted synthetic run. B01C named artifact validation and release gating is the next bounded task; it is not implemented here.
