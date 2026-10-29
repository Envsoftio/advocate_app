# B04A — Supplemental plans and prior-build revision comparison

29 October 2026. **B04A complete for the available Mac target.** B04B generation/insertion policies and B04C audience variants are outside this slice.

## Requirement-to-evidence checklist

| Requirement | Implementation and evidence | Status |
| --- | --- | --- |
| Supplied BU08 composition | Parsed scoped JSON, reusable button/input/table components and descendant overrides; semantic light/dark tokens. 858/332 columns, 18px gap, 538px panels at 1440×900. Browser asserts x208/y236. | Verified |
| 19A stale choices / 19B revision comparison | Real selected baseline; three update choices; five-column revision table and selected-document provenance panel. 19A/19B checked against scoped JSON, no PNG supplied. | Verified |
| Real immutable baseline | Saved build ID, timestamp, physical count and file digest provenance; saved plan baseline cannot change. Earlier files are never rewritten. | Verified |
| Added / changed / omitted / unchanged | Document IDs, input revisions, recorded working digests and actual primary membership. Reorder/metadata/map impacts remain separate reasons. Incomplete old revision/map snapshots report unavailable. | Verified |
| Persistent plan identity and selection | SQLite schema 20; opaque page identities bound to immutable working-file identity and digest; independent plan ID/version/title/baseline/settings/ordered selected IDs. | Verified |
| Exact eight selected IDs | Backend map has eight sources + actual cover/index = ten physical; index-only is nine. Noncontiguous runs and three-volume overhead also tested. Browser checks exact saved IDs. | Verified |
| Caller Apply / Cancel | Page picker and numbering edit copies; Apply updates only the supplement draft, Cancel restores caller. Global membership shortcut routes to active supplement. Focus restores to caller control. | Verified |
| Active context / reopen | Saved plan pointer persists per matter in local UI preferences; real plan lives in SQLite. Reenter Build resumes saved supplement. Dedicated Resume actions remain available. | Verified |
| Pending, empty, stale, deleted, failure | Empty plans can be saved but cannot prepare. Missing IDs remain visible and block maps; no rebinding. Full input digest + plan version reject stale saves. Delayed previews cannot overwrite newer selection. Errors retain draft and offer explicit reload. | Verified |
| Shared map | `supplemental_preview` in engine.py resolves ordered IDs into contiguous runs, then reuses existing volume/index/numbering map. Every source map entry includes opaque ID, original source index, printed label and physical position. | Verified |
| Original / prior hashes | Backend restart and native reconciliation compare originals and prior artifacts byte-for-byte. | Verified |
| Scope boundary | Generation, artifact QA/review/export and preserve-reference insertion visibly unavailable for supplements. Primary generation/review/Save stays reachable by leaving the planner. No supplemental PDF is claimed. | Verified |
| Required complete suites | Backend 323, frontend unit 44 and production build pass. Complete browser 120 passed / 1 opt-in vault skip. | Verified |
| Native package / planning / restart / regression Save | Matching sidecar + release .app built; isolated QA bundle built. Native planning, restart, comparison, primary rebuild/review and Save verified. | Verified |

## Persistence and identity decisions

`source_page_identities` stores a UUID, matter/document ID, immutable working relative path, original revision, zero-based source position and working-file SHA-256. IDs are allocated once for a stored working file, including existing stores on first access. They are **revision-bound page identities**, not content lineage across edits. Any changed working file invalidates selection, even if a human considers an individual page unchanged. The plan keeps the old IDs and requires explicit removal/reselection; it never guesses from filenames, page counts, visual similarity or new positions. Undo to the exact stored file can recover its identities if its bytes still match.

`supplemental_plans` stores versioned JSON atomically in SQLite, separate from primary membership/output settings. Save requires the current full input digest (including reference-only material) and the previous plan version. Baseline ID is immutable after the first save. No map or check result is persisted as permanently current. Input changes clear displayed maps; recalculation resolves current evidence again. Selected IDs retain explicit picker order. Contiguous selected source positions form index runs; noncontiguous runs remain separate entries. Group cohesion and reviewed primary internal-split policies are not implicitly transferred to selected-page plans.

The current portable `.cbmatter` format does not carry saved generated builds, supplemental plans or their page-identity registry. B04A save/reopen is local-store persistence, not portable supplemental-plan transfer. This limitation does not change existing portable primary-matter behavior. Plan edits are explicitly saved, not added to the primary matter Undo history. The UI identifies Save plan and never claims plan Undo support.

## Design and capability adaptations

- [BU08 reference comparison](ui-evidence/b04a/BU08-reference-comparison.png) retains the reference hierarchy, placement, two-column proportions and semantic palette. MatterDraft branding, actual build IDs and actual counts replace fixtures.
- Multiple selected documents use a bounded keyboard-scrollable selection area rather than hardcoding the one reference Annexure B line. Individual page choices use explicit revision-labelled page checkboxes; applying a document-only membership form would lose the required exact selected IDs.
- **Save plan** is added beside **Prepare supplement** so an incomplete/empty selection can be persisted without claiming build readiness. Baseline hashes are in a disclosure beneath the main composition.
- The summary says **Generated pages** because the shared engine may add separators or additional volume indexes; two pages are not assumed. The displayed numerical values come from `supplemental_preview`.
- Supplement numbering reuses `NumberingSettings`; its right column explains supplemental settings and scope. The 11C page-map component accepts an externally resolved supplemental map and never fetches the primary map in that mode. Source bookmark mutations are not exposed in the read-only supplemental plan.
- 19B shows all document statuses, including unchanged entries, and a selectable revision-detail panel. More than one row naturally extends the reference table. Detailed old/new ranges, hashes and settings remain available under a disclosure.
- At compact widths, 19A choices stack; BU08 retains a narrower summary until the single-column breakpoint. Page scrolling and keyboard focus keep lower actions reachable at 125%. Primary actions are not hidden behind the fixed status bar.
- Supplemental generation, artifact checks and output review/export are unavailable in B04A, consistent with the separately scoped B04B supported-build work. Planning checks do not certify an artifact or filing.

Visual evidence: [BU08 desktop](ui-evidence/b04a/BU08-matrix-1440.png), [BU08 compact](ui-evidence/b04a/BU08-matrix-1024.png), [19A desktop](ui-evidence/b04a/19A-matrix-1440.png), [19A compact](ui-evidence/b04a/19A-matrix-1024.png), [19B desktop](ui-evidence/b04a/19B-matrix-1440.png), [19B compact](ui-evidence/b04a/19B-matrix-1024.png). All six contact sheets and the direct BU08 comparison were inspected. Each covers both themes and 90/100/110/125% interface scales. The scoped tests assert no horizontal page overflow, keyboard Enter/Space/Escape, Tab/Shift-Tab, caller focus and scrolling to primary actions.

## Changed paths

- Backend: `app/supplements.py`, `storage.py` (schema 20), `schemas.py`, `main.py`, `engine.py`; `tests/test_supplemental_plans.py`, `test_storage_migration.py`.
- Frontend: `SupplementWorkspace.vue`, `BuildWorkspace.vue`, `BuildComparison.vue`, `BundlePreview.vue`, `App.vue`, `supplement-design.css`, `main.ts`; `e2e/supplement-plans.spec.ts`.
- Handoff/status: this file, acceptance log, implementation backlog, context pointer and README.

## Actual verification

- `cd backend && .venv312/bin/python -m pytest -q`: **323 passed**, six dependency warnings, 65.86s. Six new supplemental cases cover restart, independent settings, exact IDs/map/overhead, empty/duplicate/unknown identities, missing/stale pages, baseline immutability, concurrent plan version refusal, revision comparison and preservation hashes.
- `cd frontend && npm run build && npm run test`: production/type build and **44 unit tests passed**. Existing chunk-size advisory remains.
- Full command: `cd frontend && MATTERDRAFT_E2E_VITE_PORT=1447 npm run test:e2e -- --output=/private/tmp/matterdraft-b04a-final`.
- Final focused browser run (four B04A cases plus existing comparison regression): **5 passed**, 48.4s. The first complete run had **118 passed, 1 failed, 1 skipped**: an existing comparison test expected the replaced layout. Updated its selectors/disclosure interaction while retaining assertions; final complete rerun **120 passed, 1 skipped** (opt-in vault), **7.6m**, no retries. [Final log](ui-evidence/b04a/browser-final.log). Loopback services required a sandbox-permitted retry.
- `backend/.venv312/bin/python backend/scripts/build_sidecar.py`: matching Apple Silicon sidecar built.
- `cd frontend && npm run tauri:build -- --bundles app`: **70.13 MiB** release `.app` built.
- Separate debug QA bundle: **37.04 MiB**, identifier `io.courtbundle.desktop.b04aqa`; connects to the frozen sidecar on port 8790 with isolated `/private/tmp/matterdraft-b04a-native-store` storage. This does not test production vault startup. Installed application not replaced.

## Native evidence and limitations

The isolated frozen-sidecar fixture starts with 24 included pages and a 25-physical-page baseline, then inserts eight source pages into the working copy (revision 2, 32 source pages). Native 19B reports revision 1 → 2 and 24 → 32; BU08 explicitly selects source 17–24. Numbering Cancel retained the prior prefix; Apply saved `N1 · `. Save and Prepare resolved eight source IDs + cover/index = ten physical pages, printed N1 · 3–10 / physical 3–10. App and sidecar restart retained the exact IDs, baseline and settings; recalculation returned the same map. [Restart map](ui-evidence/b04a/native-restarted-map.png), [revision comparison](ui-evidence/b04a/native-19B.png).

Leaving the planner exposed unchanged primary settings: index-only, Arabic labels and 32 sources. Native rebuild produced 33 physical pages; generated bookmark navigation reached source 1 / physical 2 with actual raster and selectable text. System Save Cancel retained the dialog and build; retry saved `/private/tmp/b04a-native-saved.pdf`. SHA-256 `ae4284bc102954fb931c4c034d629d6d2599f4e418f699f95164dbcc244ad78b` equals the generated artifact. Nine recorded original/baseline files remain unchanged, and imported-original digests match. [Rendered review](ui-evidence/b04a/native-primary-review.png), [Save](ui-evidence/b04a/native-save.png), [machine reconciliation](ui-evidence/b04a/native-reconciliation.json).

Observed native limitations: one launch before the external QA sidecar was ready showed incomplete matter resources until reopening; this isolated development connection does not certify production startup recovery. During generated review the outline/page destinations appeared before raster/text; those became visible after further interaction. The final screenshot verifies actual rendering, not bounded render latency. A similar delay is recorded in B03B; cause remains unestablished.

## Target limits and next bounded task

All scoped functional and visual gates passed on the available Mac target. `git diff --check` passed after handoff updates. Windows, Intel Mac, clean-device/minimum-macOS, production encrypted-vault startup and native Office rendering/printing remain unverified target checks.

Stop after B04A. Next separately authorized task: **B04B**, explicit insertion/label policy, mapping reconciliation and supported supplemental generation. B04C audience variants are separate. No whole-B04 completion claim.
