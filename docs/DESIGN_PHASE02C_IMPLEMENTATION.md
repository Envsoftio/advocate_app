# Design Phase 02C — Compact/dark readers and engine recovery

28 October 2026. **Functional 02C implementation delivered; visual completion reopened by the user.** See [the new UI correction](DESIGN_PHASE02D_UI_CORRECTION.md) for the replacement layout and current verification status. This record supersedes earlier handoffs' future-work status for 02C; combined evidence is assessed below. No B01 or later phase work.

## Scope and design evidence

Read the repository guidance, README/handoffs/acceptance records and the supplied JSON kit's README, screen index, Phase 02 task/reference index, shared rules/decisions and semantic/resolved tokens. Inspected ED36 and ED40 in `adaptations.json` and ED30 in `state_contracts.json`. Resolved six adaptation components with 84 descendant overrides and 48 semantic tokens, and seven ED30 components with 49 overrides and 52 tokens. Viewed the supplied ED36 preview; ED30 and ED40 have no supplied preview. No master dump, Pencil MCP or `.pen` access.

The existing working reader, original/generated artifact dialog, responsive Documents shell, Details behavior, reader positions, appearance persistence, bundled Inter and semantic tokens remain the implementation base. Fixture titles/counts/revisions were not used as runtime data. No backend change, schema, persistent PDF cache, cloud service, new layout system or later editor/bundle mode.

## Implemented behavior

- **Compact/dark artifacts:** original/generated review has collapsible page/bookmark navigation, focused reading, bounded original information and search results, ellipsized titles with full title tooltips, theme-aware controls and scale-correct dialog dimensions. Artifact identity, read-only labels, version selection, search, page/mode/zoom controls and stale-build notices remain reachable. Focused reading retains the mounted viewport, query, page and zoom, restores the navigation choice, and exits with Escape. Tab/Shift+Tab stay inside the artifact dialog; closing returns focus to the invoker. PDF paper, raster colors, source text and authored typography remain independent of app appearance.
- **Engine versus internet:** the existing authenticated health probe and transport failures drive local-engine state. An internet-offline event alone does not disable a reachable local engine. A compact, keyboard-accessible status disclosure explains the distinction and the limits of in-memory reading/search. At compact widths, the redundant matter heading yields space during disconnection; matter identity remains in the workspace tab and document identity remains in the reader. Working-reader navigation and already available PDF text remain usable.
- **Honest unavailable data:** keep loaded PDF.js documents and results mounted during loss of the engine. Reading/search continue only as far as data available to that document instance permits. Missing pages report unavailable; failed/stalled text reads retain prior matches and explicitly report incomplete results. Per-page text reads are bounded (10 seconds normally, 2 seconds when the engine is unavailable). No whole-PDF/search-cache claim or persistent cache. Original access never falls back to working bytes.
- **Recovery and drafts:** the shared API rejects engine-dependent writes while disconnected/checking/review-blocked; native file-export entry points use the same gate. Metadata/settings and Memo helper autosave timers stop. Retry checks actual health, current matter/document state and the available build-input fingerprint before enabling writes. It does not resubmit edits, OCR, redactions or builds. Unchanged recovery preserves the mounted reader, artifact, query, position and drafts; retained document/settings drafts need explicit Save. Metadata/settings and Memo autosave resume after explicit successful saves when the tracked drafts are clean; a remaining draft keeps the pause in place. Changed inputs or unverified retained drafts in other tabs require an explicit, focus-contained Discard/Keep editing decision before loading the latest state. The dialog names all open-tab drafts; Save cannot silently apply an old draft to changed state.
- **Derived/exact context:** disconnect invalidates outstanding resource/check responses and hides current maps/checks. Retained generated files/maps are unchanged. Existing revision-bound Preflight targets and their return context remain intact; a changed revision does not silently remap a finding. Existing 02B revision/job and original-permission contracts remain in force.
- **Explicit PDF retry:** unavailable original/generated review retries its exact artifact and retained position/zoom/query. Working retry uses the existing Save/Discard/Cancel guard before a reload that could clear a PDF-tool draft. A failed range/text stream can stall PDF.js transport destruction; the existing document owner now owns the public PDFWorker explicitly and terminates that document's worker after a two-second cleanup limit. This bounds retry without sharing old/new document workers, changing renderer libraries or discarding the active document merely because health failed. Existing bundled main-thread worker fallback remains tested.

## Changed paths for 02C

- `frontend/src/App.vue`: recovery reconciliation, draft/autosave pause/resume, derived-state invalidation, operation gates, guarded reader retry and compact availability state.
- `frontend/src/components/EngineNotice.vue` and `frontend/src/lib/engineConnection.ts`: shared local-engine state/reason and compact recovery disclosure.
- `frontend/src/components/ExportPdfReview.vue`: compact/dark artifact navigation/focus, exact retry, information disclosure and keyboard containment.
- `frontend/src/components/CourtProfileGuide.vue`: retain Memo drafts, pause delayed saves during disconnection/recovery and expose explicit retry.
- `frontend/src/components/PdfViewer.vue`: reading while disconnected, clear unavailable-page feedback, guarded retry and non-obscuring status behavior.
- `frontend/src/lib/api.ts`: transport detection and service/native-save gates.
- `frontend/src/lib/pdfReader.ts`: bounded cleanup with an owned PDF.js worker.
- `frontend/src/lib/pdfSearch.ts`: bounded unavailable text extraction with incomplete-result feedback.
- `frontend/src/lib/appearance.ts`, `frontend/src/styles.css`: reuse persisted interface scale for dialogs and scoped semantic reader/recovery styling.
- `frontend/src/lib/api.test.ts`, `frontend/e2e/engine-reader.spec.ts`: transport/write-gate, draft/revision/recovery, uncached data, Preflight context, focus, read-only and layout regressions.
- `frontend/e2e/journey.spec.ts`: update unavailable-engine status copy, explicitly open generated bookmarks and reconnect before retrying a failed Memo save. Preserve existing assertions and add retention of further Memo edits during disconnection.
- This handoff, `docs/ACCEPTANCE_TESTS.md`, and the ignored kit's Phase 02 task record.

Pre-existing `.gitignore`, backend/02B changes, fonts/tokens, design files and unrelated local changes are preserved. No commit or deployment.

## Verification and corrected failures

All browser tests use disposable synthetic stores, backend **8768**, Vite **1436**, and installed dependencies. No application edits occurred during an active browser run. Existing usable-paper/overflow thresholds were not reduced. Runtime images were inspected only to resolve the artifact composition and compact disconnection failures; no new capture gallery.

- Initial frontend build passed; unit tests found shared connection state leaking between API tests. Added test isolation and a meaningful write-gate/no-replay regression; **44 unit tests passed** afterward. Two path mistakes (a root npm invocation and a script using root-relative paths from `frontend`) failed before their intended work; rerun in the correct directories.
- Initial browser startup was blocked by sandbox binding restrictions; the same local-server command ran with approved escalation.
- Initial new browser run: **2 passed, 3 failed**. Exposed a stuck unavailable-text search and artifact-dialog Shift+Tab escape; both fixed.
- Next reader run: **3 passed**, then interrupted a matrix test waiting for an incorrect new canvas selector. Corrected the fixture (`canvas.pdf-paper`), retaining pixel/geometry checks.
- Required scoped run: **13 passed, 2 failed**. All five 02B cases, both existing working layout matrices, both Read workspace cases and the four recovery cases passed. The two artifact matrices focused a still-disabled loading selector before opening; the fixture now waits for enablement and verifies invoker focus before selection.
- Artifact follow-up: **5 passed, 2 failed**. Both artifact matrices passed; a repeated uncached-page recovery exposed stalled PDF.js cleanup, and compact disconnection exposed excessive status/header height. Corrected cleanup and scoped layout without lowering the paper threshold.
- Recovery/lifecycle run: **11 passed**, 57.3 seconds. Includes all seven new 02C cases, both delayed document-switch cases, the >25 MiB generated range-loading journey, and production worker fallback. A concurrent typecheck identified a type-only import path; corrected before the final build/full run.

Actual commands, from `frontend`:

```sh
npm run build && npm run test
COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- engine-reader.spec.ts --output=/private/tmp/courtbundle-02c-initial
COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- engine-reader.spec.ts --output=/private/tmp/courtbundle-02c-reader
COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- engine-reader.spec.ts document-state.spec.ts read-workspace.spec.ts documents-layout.spec.ts --output=/private/tmp/courtbundle-02c-scoped
COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- engine-reader.spec.ts --output=/private/tmp/courtbundle-02c-adaptations
COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- engine-reader.spec.ts pdf-worker-fallback.spec.ts pdf-document-switch.spec.ts pdf-large-export.spec.ts --output=/private/tmp/courtbundle-02c-recovery
COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- --output=/private/tmp/courtbundle-02c-full
COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- --grep 'index preview updates|Memo of Parties|02C' --output=/private/tmp/courtbundle-02c-regression-fixes
COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- --output=/private/tmp/courtbundle-02c-final
COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- pdf-overlay-selection.spec.ts --repeat-each=3 --output=/private/tmp/courtbundle-02c-overlay-recheck
```

- First full browser run: **80 passed, 2 failed, 1 skipped**, 4.9 minutes. The existing generated-bookmark journey expected an always-open panel; it now opens the real control before retaining its navigation checks. The Memo save-failure test aborted a request, so the new engine gate correctly required reconnection before Save retry. Updated that interaction and guarded the Memo helper's delayed autosave as well.
- Focused correction run: **10 passed**, 1.0 minute, covering all seven 02C cases and the existing generated-index/Memo journeys. The final Memo assertion additionally verifies edits made while disconnected remain available for explicit Save.

Final production build and **44 Vitest cases in 11 files passed**. The final full browser run on final application source finished **81 passed, one failed, one opt-in vault case skipped**, 4.7 minutes. The lone existing overlay-drag test observed an unchanged coordinate after a simulated pointer drag; it had passed the preceding full run. Repeated the unchanged overlay file three times on the same final source: **six passed**, 14.8 seconds, including three successful drag/resize/Delete journeys. Its cause is unproven; no assertion was relaxed and no overlay implementation or test was edited. Thus **all 82 enabled browser cases passed across the final full run and unchanged targeted repeats**, not in a single all-green final full-suite run. `git diff --check` passed.

### Combined Phase 02 acceptance assessment

| Requirement | Evidence assessed |
| --- | --- |
| 02A real reader, search, bookmarks, information and reading context | Final `read-workspace`, reader/lifecycle, layout and keyboard cases pass; prior 02A final 70-case run is recorded in its handoff. |
| 02B immutable artifacts, save/discard/cancel, exact revisions/Preflight and invalid derived state | All five `document-state` cases pass on final source. The unchanged backend contracts have the 02B record's **244 passing pytest cases**; backend tests were not rerun for this frontend-only task. |
| 02C compact/dark and scale adaptation | Both working and original/generated matrices pass with the existing paper/overflow thresholds, plus the compact dark disconnected case. |
| 02C engine loss/recovery, retained drafts, unavailable data, no operation replay | All seven new browser cases and the API gate regression pass; existing initial-engine failure, failed-save/Memo recovery and worker/range retry cases also pass. |
| Preserved editing, OCR, page operations, history, index, exports, autosave and tab drafts | All enabled existing cases passed across final-source runs; the isolated pointer-drag failure and three unchanged successful repeats remain explicitly recorded above. |

This closes the scoped 02A/02B/02C implementation and browser acceptance, not broader native/release qualification. Stop after 02C.

The existing working-reader matrix and new original/generated matrix cover **1440×900 and 1024×768, light/dark, 90/100/110/125%**, including over **200×180 physical pixels of clipped/intersected, unobscured PDF paper** at Fit width. Artifact checks also cover search-result visibility, navigation panels, page/zoom/query continuity, focused-reading restoration, modal keyboard containment, invoker focus and white PDF raster pixels. An additional test checks usable PDF space during disconnection at **1024×768, dark, 125%**. Recovery tests inspect actual API state and outgoing writes, including resumed autosave only after an explicit recovery save.

## Limits

No native/Tauri runtime or build, native Save/print dialog, packaged sidecar, encrypted-vault opt-in, Windows/macOS Intel/minimum-version/clean-device or full assistive-technology qualification is claimed. Existing large-chunk/dependency warnings remain. The isolated simulated overlay-drag failure did not reproduce in three unchanged repeats; its root cause remains unqualified. Browser network-failure injection represents an unavailable local engine; it does not qualify OS process crashes or every backend restart/credential scenario. There is no durable offline PDF cache: a forced reload/quit can lose unsaved drafts and uncached content remains unavailable. Stale retained drafts require explicit discard/re-entry rather than automatic merging. Original password-entry preview and arbitrary historical-revision browsing remain unavailable. No court approval, legally verified OCR or verified-signature claim.
