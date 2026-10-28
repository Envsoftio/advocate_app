# Phase B01C — Artifact validation and review

28 October 2026. **B01C scoped implementation, visual/browser verification and available Apple Silicon native Save acceptance complete**, with the capability and platform limits below. The supplied new design defines the B01C composition: BU07 output checks, 14C current-input Preflight, ED34 finding review and adjacent build/review/export controls. This record describes technical QA, not legal completeness, court approval or verified signatures.

## Design contract and implementation map

Read the bounded `membership_validation.json`, BU07/14C previews, reusable components, descendant overrides and shared light/dark tokens. Read adjacent 17A/18A/18B from `build_journey.json` and ED34 for finding navigation. No unresolved reference was accepted as a layout instruction. The supplied counts, filenames and successful outcomes are illustrative; runtime values come from actual saved files.

| Screen/control | Component and service | Persistence and acceptance |
| --- | --- | --- |
| BU07 file selector, seven named rows, measured evidence, run history | New `OutputChecksDialog.vue`; `validation/state` and `validation/run` | Immutable build ID, selected files, SHA-256, input digest/revisions/settings; backend tests independently reopen files; browser asserts 1000×704 dialog at (220,98). |
| BU07 run checks, copy details, review bundle, Done/Escape | Actual local rerun, clipboard JSON, existing generated reader, native dialog focus management | History is append-only; per-build file/history selection survives navigation; focus returns to caller. |
| 14C summary/filter, errors/warnings/technical notes, Run checks | New `PreflightWorkspace.vue`, existing engine preflight plus persisted run | Current saved input snapshot; explicit pending/error states, request sequence and matter guards; summary begins x208, width1208 at 1440×900. |
| 14C human review | Existing user-authored `FilingReview`, opened from new composition | Court/case/stage checklist preserved; no fabricated legal checklist. Separate from four per-build review items. |
| ED34 exact finding, note, Keep page & return, Page tools | New `FindingReviewPanel.vue`, structured document/revision/page and run/finding endpoint | Notes saved separately; stale/reordered/deleted targets refused; existing working-page tools and draft guards retained. Filter and finding focus restored. Returning from an output finding reruns current-input checks and saved-output checks while retaining the originating historical run. |
| 17A review checklist and output-checks entry | `BuildWorkspace.vue`, new `useOutputValidation` | Four independent, explicitly recorded human-review booleans per build; optimistic version and artifact digest prevent stale saves. |
| 18A output checks and inspection | `ExportPdfReview.vue`, authenticated PDF range loading from inspection endpoint | Actual saved PDF stays read-only and identifiable; failed/historical files remain inspectable when manifest hashes match. |
| 18B review-copy/release export | `BundleExportDialog.vue`, API and Tauri native save | Backend gate also protects direct download, Office drafts, print and protected-copy paths. System Save/browser download behavior retained. |

The old Preflight composition is replaced. `validation-design.css` uses existing semantic tokens and bundled Inter. BU07 uses the supplied two-column table/details hierarchy; 14C uses the expanded 184px navigation, 54px summary and grouped findings card; ED34 uses the right-hand finding panel beside authentic PDF content. Compact layouts scroll or stack. Informational notes use an information icon, not a success checkmark.

## Validation, identity and release policy

`backend/app/validation.py` is UI-independent. Atomic records live under each local matter's `validation/<build-id>/` directory; protected stores use existing encrypted file I/O. Human review and finding notes never rewrite generated artifacts. There is no SQLite schema migration. Portable matter packages retain their existing source/revision scope; this phase does not add generated builds or QA history to portable transfer.

Each completed build runs actual output checks after publication. The engine records expected counts, destinations, viewer labels and source mappings from its deterministic page map. Validation reopens files and compares measured output with that contract:

- PDF content/catalog parsing, exact saved SHA-256 and bytes; ZIP CRC, member identity and required members; CSV row count; report build identity.
- PDF counts, outlines and internal GOTO destinations against the saved engine contract.
- Printed footer tokens and viewer page labels against the same expected labels.
- Measured bytes against the build's saved configured limit. The existing engine applies its configured file limit to downloadable artifacts; this is not a nationwide filing rule.
- Optional general export cleanup is **not run**, explicitly unimplemented/not selected. It cannot appear passed.
- Recorded staged or destructive redaction operations produce **not run**, blocking release because independent removal verification is unavailable. With no recorded operations the result is **not applicable**, with an explicit statement that imported content is not certified. Visible black rectangles are not verification.

Outcomes are passed, failed, warning, not-run or justified not-applicable. Recorded bookmark/link omissions remain warnings. Selection-only runs are retained but cannot replace the latest all-files run used by the gate. File hashes are checked again when gate state is requested. Historical evidence remains unchanged after effective-input edits; reference-only edits preserve B01B freshness behavior. Legacy builds without a saved expected contract remain inspectable but cannot pass the new export gate: rebuild to obtain a contract; a rerun never fabricates missing expectations.

| Operation | Policy |
| --- | --- |
| Build | Existing preflight errors block. Warnings require review but do not prohibit technical generation. |
| Inspect saved PDF | Authenticated `/inspection/<filename>` checks immutable file identity and supplies bounded PDF ranges; carries an inspection-only response header. Failed/historical contracts do not prevent inspection. |
| Download review copy, Office draft, print or protected copy | Requires a complete applicable all-files technical assessment, with no failed/core missing checks and matching artifact hashes. Human review and current-input freshness are not required for explicitly unreleased review copies. |
| Release reviewed output (`?release=true`, including native save) | All review-copy requirements plus current effective inputs, all four human items tied to this artifact digest, and no unverified recorded redactions. Backend returns 409 when blocked. |

Review-copy export and inspection are not release certification or DRM. A human can copy inspected bytes. The product records workflow readiness rather than claiming to prevent every external use. Office/print/protected outputs remain review derivatives, not separately qualified released artifacts. Human acknowledgement cannot turn a technical failure into a pass. An immutable failed output needs a corrected new build; editing its source does not repair historical bytes.

The only numbering addition is PDF viewer labels: standalone volumes start at 1, combined output restarts at the existing volume boundaries, and Part extracts retain their existing printed source ranges. Expectations and labels derive from `engine.py`; no second page allocator or index range algorithm was introduced. Multivolume/Part pytest coverage verifies these rules.

## Changed implementation paths

- Backend: new `app/validation.py`; `engine.py` contracts/viewer labels/post-build validation; `main.py` run/review/finding endpoints, inspection and gate wiring.
- Frontend: new `OutputChecksDialog.vue`, `PreflightWorkspace.vue`, `FindingReviewPanel.vue`, `lib/outputValidation.ts`, `validation-design.css`; integration in App, BuildWorkspace, BundleExportDialog, ExportPdfReview, API/types/main and Tauri native_save.
- Tests: six backend validation cases, new output-validation browser cases and updated existing selectors/reader URL assertions for the new design and inspection endpoint. Existing draft, identity, range, cancellation and usable-paper assertions remain.

Preserved capabilities include membership, settings/presets, build/retry/cancel, generated PDF search/bookmarks/ranges, saved-output retrieval, original/working/generated switching, Office/print/protected review tools, storage and existing draft guards. Original and prior-artifact hash preservation is tested. Processing stays local.

## Verification and visual evidence

Evidence is retained under `docs/ui-evidence/b01c/` following the repository's local ignored-evidence convention. `verification.json` records screenshot/build hashes; logs and independently saved synthetic PDFs are retained there. No confidential matter was used.

- `cd backend && .venv312/bin/python -m pytest -q`: **260 passed**, six existing warnings, 45.28 s. Six new B01C cases cover persistence/reopen, measured contracts, all export gates, subset isolation, effective/reference freshness, exact/obsolete finding notes, legacy missing checks and redaction unavailability.
- `cd frontend && npm run build && npm run test`: **PASS; 44 unit tests pass**, existing Vite chunk-size warning.
- Scoped B01C browser verification: **2 passed**, followed by **1 passed** for exact output-finding return/recheck, including actual download independently reopened, review persistence, unchanged original, exact finding/filter/focus and delayed older-build fetch completion. The latter delays a real fetch result rather than fabricating a response.
- `COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- --output=/private/tmp/courtbundle-b01c-complete`: **95 passed, 1 opt-in vault skip**, 5.8 minutes. No implementation changes followed this run. The preceding full run passed 94 enabled cases; the final run includes the additional output-finding regression. Early full runs exposed old-layout test selectors/reader URLs; a new route-based late-response harness also double-handled routes. Selectors now target the new controls and the late-response test delays actual browser fetch completion. No application assertion was removed to hide a failure.
- Matrices capture BU07, 14C and ED34 at 1440×900 and 1024×768, light/dark, 90/100/110/125%. BU07 exact outer geometry and 14C summary coordinates are asserted. Scrolling, dialog Escape/caller focus, exact PDF page and usable paper width are exercised. Manually inspected representative reference-size light and compact dark/125% images. Corrected oversized table rows, summary height, finding panel offset, compact summary-label wrapping and information-state icon semantics during this phase.
- `.venv312/bin/python backend/scripts/build_sidecar.py`: current Apple Silicon sidecar built; frontend release `.app` built with the matching sidecar, **69.74 MiB**. Final UI rebuild completed after the last context/recheck/visual correction (`cd frontend && npm run tauri:build -- --bundles app`). The packaged sidecar hash matches the source binary.
- `git diff --check`: **passed**. Pre-existing changes preserved; no commit, reset or deployment.

Layout adaptations are limited to runtime content and supported capabilities: real validation filenames/counts, measured evidence disclosure, history selection, honest unsupported cleanup/redaction states, user-authored checklist, compact scrolling and platform-owned Save dialogs. The output-finding regression injects a bad expected label into a disposable synthetic manifest, verifies a real measured failure, navigates to the exact page, saves a note, returns to the same run/file/focus and proves that the automatic rerun remains failed. This is not a claim that every future design frame is implemented or pixel-identical.

## Native workflow evidence and limits

Used a separate `CourtBundle B01C QA.app` identifier with synthetic matter `8edd4e96708c43fabb6a4dfeed55b230`, build `2e10b52c8a5041f4940fe69929ccc92a` and local engine at 8773. The user's engine at 8765 was not used or stopped. This uses the real Tauri save command against an isolated development backend, not a packaged-vault/clean-device qualification.

Observed BU07 measured checks, generated PDF index navigation, incomplete-human-review release blocking, explicit four-item review and successful reviewed Save. Cancel returned “Save cancelled. The generated build is unchanged.” Stopping only the isolated engine while Save was open produced an error and Retry export, with no output file. After explicit Retry connection, another Save succeeded. Both saved PDFs independently reopen with **4 pages, 3 bookmark entries and viewer labels 1–4**, SHA-256 `5df238b782a065d640f5a1682bbaba1eec6b4918539e74b90bb98023aba35623`, exactly matching the manifest. Review persisted across the isolated engine restart. The QA app and its backend were stopped after verification.

Windows, Intel Mac, minimum-version/clean-device installation, packaged-vault native runtime and native print/Office Save dialogs remain untested in this phase. The normal browser suite skips the opt-in vault scenario. Backend protected-store tests run, but they do not substitute for native vault qualification. Optional general cleanup and independent redaction verification remain explicitly unavailable, not passed.

Stop at B01C. **B02A** is next: preset/settings persistence, visible overrides, and cover/index preview.
