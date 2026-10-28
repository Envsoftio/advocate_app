# CourtBundle design Phase 00 — implementation map

Date: 27 October 2026. Status: **DONE — audit only**. Source baseline: `004cb593d7b433be51e2d1d902810ac352ccbd9b`.

[Certain] The existing app is the implementation base. Reader, editing, OCR, storage and bundle services already exist; replacing them would discard tested behavior. The new design also requires behavior that is absent today, especially artifact selection, page identity, consistent dirty navigation and exact Preflight return context.

Evidence: [capability matrix](DESIGN_PHASE00_CAPABILITY_MATRIX.md), [baseline verification](POC_ACCEPTANCE_TESTS.md#bg-design-phase-00-baseline--27-october-2026), [runtime captures](evidence/phase00-20261027/README.md). “Working” below means the scoped behavior passed the recorded checks, not release acceptance.

## Scope and design access

- Read repository `AGENTS.md`, kit README, shared working rules, Phase 00 task, root README, handoff pointer and relevant maintained handoff/acceptance/backlog sections. No nested `AGENTS.md` was found in the repository inventory.
- The connected `mcp__pencil__get_app_state` call failed with the account's plan usage limit. **Master-file MCP readability is unverified.** The requested server command/config was not installed or changed, and no claim is made that the connected server uses that exact launch configuration. No `.pen` file was parsed or modified through a fallback.
- Phase 00 has no scoped canvas (`reference-index.json` is empty). Used the kit overview, frame-index titles, 139 semantic token definitions/resolved themes, and supplied `05A.png` preview as design intent. Detailed master/subset content and fidelity remain unverified. No later phase was executed.
- Before the audit, the kit directory and `CourtBundle_Complete_Workflow (2).pen` were already untracked. Preserve them. No application source, dependency manifest/lockfile, schema, imported sample or existing user store was changed. Builds regenerated ignored artifacts; runtime checks used temporary synthetic stores.

## Actual implementation boundaries

| Area | Current implementation and reuse boundary |
| --- | --- |
| UI/bootstrap | `frontend/src/main.ts`, `App.vue`, `styles.css`; Vue 3/TypeScript/Vite, Lucide icons, VueDraggable, locally bundled Manrope. Existing CSS contains hard-coded colors and later theme/responsive overrides; supplied Inter/semantic tokens are not integrated. |
| Navigation/state | `App.vue` owns `View`, `activeView`, matter/document tabs, selection, drafts, autosave, jobs and dialogs through Vue refs/reactive/computed values. Views are matters/overview/documents/index/build/validation/storage/rules/about. No Vue Router or `defineStore` implementation found; Pinia is registered but not the actual state owner. Keep design frame IDs out of routing. |
| API boundary | `src/lib/api.ts`: authenticated local requests, timeouts, Tauri session discovery and browser/native download paths; `types.ts` contains existing contracts. Reuse these, including error propagation. |
| Reader | `PdfViewer.vue` and `ExportPdfReview.vue` share `lib/pdfReader.ts`, `pdfNavigation.ts`, `pdfSearch.ts`, `pdfSurface.ts`, `pdfWorker.ts`, `PdfReaderViewport.vue`, `PdfPageSurface.vue`, `PdfReaderControls.vue`, `PdfThumbnailRail.vue`, `PdfShortcutHelp.vue`. PDF.js renders bounded surfaces/ranges; backend owns document bytes. |
| Document preparation | `DocumentTable.vue`, `DocumentThumbnail.vue`, `MatterHome.vue`, `MatterForm.vue`; metadata/batch sorting/labels and import orchestration remain in existing components plus `App.vue`. |
| Page/edit tools | `PageTools.vue`, `PageOrganizer.vue`, `PageCropPreview.vue`, `PageTransferPreview.vue`, and `PdfViewer.vue`; backend `main.py`, `pdf_ops.py`, `editable_marks.py`, `paragraph_trial.py`, `schemas.py`. Keep working revision commits in the backend. |
| Index/build | `BundlePreview.vue`, `ManualBookmarks.vue`, `IndexImport.vue`, `BuildComparison.vue`, `OutputPresets.vue`, `FilingReview.vue`; `backend/app/engine.py` owns deterministic volume plans, index rows, destination offsets, PDF/CSV generation and publication. No parallel frontend page-map calculation. |
| Jobs | `jobs.py`: one ThreadPoolExecutor worker, persisted queued/running/completed/failed/cancelled/interrupted states. `main.py` submits OCR/compression/build and exposes polling/cancel/retry. `App.vue` polls jobs. Global PDF lock serializes API/native processing, including across matters. Interrupted OCR/build can retry as a new job; no mid-operation resume. |
| Storage/security | `storage.py`: SQLite schema version 18, document/revision records, matter history, annotations, bookmarks, links, positions, jobs and audit; files retain originals/working revisions/completed builds. `secure_db.py`, `vault.py`, `vault_store.py`, `file_io.py`, `portable.py` implement protected storage and backup/restore. Default browser-development storage and native app-data storage differ; neither was opened for this audit. |
| Desktop host | `src-tauri/src/lib.rs` launches the release PyInstaller sidecar on loopback with a random token and vault requirement, terminates it on exit; debug uses an external backend. `native_save.rs` validates destinations and streams into temporary files before replacement. `tauri.conf.json`, capabilities, `backend/launcher.py` and `scripts/build_sidecar.py` own packaging/lifecycle. |

## Safeguards before new mutation features

| Contract | Verified current boundary | Required follow-up |
| --- | --- | --- |
| Original/artifact identity | Original hash/path, working document ID + integer revision, build ID + files exist. `/documents/{id}/pdf` serves the current working PDF; generated files have separate endpoints. | Original-vs-working reader selector/read-only artifact route is missing. Model artifact type explicitly; never label the working endpoint “original.” |
| Page identity | Bookmarks, links, marks and reader positions use revision + positional page index; stale references become unresolved. No cross-revision stable page ID/lineage table found. | Design the narrow page-identity/mapping adapter and migration separately. Reorder/replace/delete cannot be solved by renaming page numbers. Include history and portable restore semantics. |
| Dirty navigation | Metadata/settings autosave, tab-retained drafts, in-app discard dialog, local PDF drafts and some `window.confirm` paths exist. | No unified Save/Discard/Cancel transition carrying origin and destination. Verify failed save, tab/matter/view switch, native close and reload paths before enabling new edits. |
| Working commits | `edit_working_copy` creates a candidate; `Store.update_document` publishes a revision in a DB transaction; failures discard uncommitted files. Original integrity tests pass. | Preserve these helpers. Some older mutation schemas permit omitted revision guards; new actions should require exact input identities. Cross-file/DB crash behavior is not a single filesystem+DB transaction; existing recovery must remain. |
| Job input identity | Compression requires `expected_revision`; OCR takes only language/signature acknowledgement and reloads the document when execution starts. Builds read state under the execution lock and record input revisions/digest. | OCR and queued builds are not bound to a submit-time revision/digest. Locking prevents concurrent processing during execution, but is not a frozen submission snapshot. Persist/validate job inputs before new job UI promises that contract. |
| Derived-state invalidation | `loadMatterResources` reloads checks/builds and increments preview refresh; `BundlePreview` recalculates with response sequencing. Export freshness compares inputs/digest; prior builds remain stored. | No shared typed dirty/stale/recalculating state contract or stable finding identity. Preserve current sequencing and introduce only narrow coordination. |
| Preflight entry/return | `Check` contains severity/code/message/document_id; source-page details can exist only inside message text. Rendered checks have no editor action. | Add structured finding/page/revision identity, tool entry and return context, then actual revalidation. Do not treat a review note as a passing technical check. Blank-page diagnosis shown by ED34 is also absent. |
| Redaction boundary | Current movable marks are persisted; Save copy/new bundles apply them to detached output. Raw working bytes, originals and prior artifacts retain content. A legacy destructive region endpoint also exists. | The new deliberate batch Apply/review/result contract must choose and verify its artifact semantics. A black overlay or existing output-only flow is not evidence that a working revision was redacted. |

[Likely] The first visual slice needs no data migration. Stable page lineage, persistent OCR review, richer comment status/threading and durable finding identity may require schema changes, but exact migrations are undecided and were not implemented. Reuse revision/history/portable infrastructure; do not create a second store. Existing release tickets remain open under `IMPLEMENTATION_TASKS.md`.

## First bounded implementation task: 01A

**Status: TODO; not started.** Apply the supplied visual tokens and Documents shell presentation to the existing workspace. Use `phases/01_visual_foundation/pilot.pen` when MCP becomes readable, plus `previews/00A.png`, `05A.png` and shared token JSON. Read Phase 01's task before executing; this audit does not authorize proceeding automatically.

Concrete scope:

1. Map all 139 semantic roles for both themes into CSS variables and use them for the Documents shell, navigation, controls, selected rows and details panel. Bundle licensed Inter locally; keep document paper/ink independent of the UI theme.
2. Restyle the existing Documents list/header/details structure with live names, counts, statuses and existing handlers. Keep import, metadata save, ordering, OCR, reader tools and export paths reachable. Do not add nonfunctional Notes, artifact selector or other design-only controls.
3. At 1024×768, collapse/draw out Details and navigation instead of consuming the PDF viewport. The captured baseline with Details open hides the PDF; this must be an explicit acceptance check. Keep PDF reader lifecycle/geometry code intact.

Affected files (planned, not changed):

| File | Bounded change |
| --- | --- |
| `frontend/src/styles.css` | Use semantic tokens and adjust Documents layout, focus/selected/disabled states and compact Details behavior. |
| `frontend/src/design-tokens.css` (new) | CSS representation of the supplied roles and light/dark values, using the existing effective-theme attribute. |
| `frontend/src/App.vue` | Documents shell markup/classes and accessible Details drawer toggle; retain existing state and event paths. |
| `frontend/src/main.ts` | Import token/font styles; replace the current Manrope UI font import once the local Inter asset is available. |
| `frontend/src/assets/fonts/InterVariable.woff2`, `OFL.txt` (new) | Verified licensed local font and license; no remote runtime font dependency. |
| `frontend/e2e/workspace-design.spec.ts` | Meaningful regression for opening/closing Details while the PDF remains usable at both target sizes, with keyboard focus return. |

Acceptance: actual app captures at 1440×900 and 1024×768 in light/dark; keyboard focus and existing selection/disabled states; import/select/metadata save/reorder/reader/OCR/index/build/export remain available. Run production build, Vitest and existing browser checks using backend port 8768 (several specs hard-code it). Keep backend/page-map/schema behavior unchanged; no new mutation is introduced by 01A. Record native appearance as unverified unless a fresh isolated Tauri runtime check is actually performed.

After 01A, verify/complete Phase 02 state safeguards before any newly introduced mutation. Retain the kit's incremental phases; the existence of tested page/OCR services does not waive those prerequisites. No calendar estimate is asserted from design-frame count.
