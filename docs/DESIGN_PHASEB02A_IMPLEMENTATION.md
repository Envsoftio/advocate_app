# B02A — New preset and cover/index design

28 October 2026. **[Certain] B02A implementation, browser/visual checks and the available Apple Silicon native preview/Save journey are complete.** The supplied JSON defines the new composition. This replaces the affected legacy index/settings presentation; it is not a cosmetic patch to the old layout. B02B had not begun at this checkpoint; see the subsequent [B02B implementation record](DESIGN_PHASEB02B_IMPLEMENTATION.md). Platform and capability limits are recorded below.

## Design contract and control mapping

Read `CourtBundle_JSON_For_Codex/implementation/shared/{WORKING_RULES.md,SOURCES_AND_DECISIONS.md,design-tokens.json,resolved-tokens.json}`, the B02 task/reference index and `preset_cover_numbering.json`. Resolved reusable components and descendant overrides for BU01, BU02, 11C, 12A and adjacent BU03 without unresolved references. Compared the supplied BU01/BU02 PNGs directly with running screenshots. The kit's packaging status is not application progress.

| Design/control | Implementation and real service | Persistence/verification |
| --- | --- | --- |
| BU01 preset list, selected detail, version/provenance | New `BundlePresetDialog.vue`; existing local `/output-presets` and matter preset application | Immutable local versions, actual saved values, timestamp and local provenance. No invented built-in or court-approved presets. |
| BU01 Save preset / Use selected preset | Existing preset store; `PresetApplication` adds explicit override preservation and expected input snapshot | Save creates a version without silently applying it. Apply is explicit; reusable settings exclude matter cover text and document exceptions. |
| Visible field differences / keep overrides | New `lib/bundleSettings.ts`, dialog and workspace disclosure | Compares effective saved values to the applied baseline, including defaults for older records. Explicit keep-overrides checkbox; unchecked application resets reusable fields. Cover identity and document exceptions always stay with the matter. |
| BU02 front-matter fields / columns | New `BundleStructure.vue`; GET/PUT `/matters/{id}/bundle-settings` | One transaction saves volume capacity, grouping and output marks; expected snapshot rejects stale saves. Existing matter history supplies Undo/Redo. Failed saves retain the draft and offer retry. |
| BU02 generated preview / volume and index pagination | POST `/matters/{id}/front-matter-preview`; shared engine PDF renderer | Actual included documents, order, revisions, draft settings and deterministic page map. Bounded PNGs of the selected volume's cover and index page; no static mock content. |
| BU02 Apply / cancel / navigation | Component draft API integrated with existing App navigation guard | Apply commits atomically; cancel restores saved state; navigation offers save/discard. Request sequences and unmount guards reject obsolete preview results. |
| 11C surrounding index/order workspace | New shell composition in App and `BundleStructure.vue`; existing `BundlePreview.vue` and reorder service | Included-only order controls preserve reference-only positions. Existing manual bookmarks and index import remain under an explicit Bookmarks tab. |
| 12A relevant volume context | Volume planning tab plus existing Build volume tool | Actual maximum physical pages, group preservation and engine-derived totals. Cover/index pages consume capacity. |
| BU03 adjacent numbering / output policy | Existing tools remain reachable from the new tabs and Build cards | Existing printed physical numbering, Bates/annexure options, watermark/timestamp and Part exports retained. No new B02B numbering policy. |

`structure-design.css` uses the existing semantic tokens and bundled Inter. At 1440×900, BU01 is **1060×720 at (190,90)** with 304px/692px columns and 16px gap. BU02's settings card is **340×586 at (208,266)** and generated-preview panel **852×586 at (564,266)**. The expanded matter sidebar is 184px. Tests assert these dimensions and the 36px front-matter fields/16px radio controls, preventing inherited legacy control sizing from changing the design.

## Supported settings and deterministic generation

Supported front matter is **generated index only** or **cover plus generated index in every volume**. Cover heading/reference/title are matter-specific, with sensible matter-data fallbacks. Applicant/respondent and actual volume/page totals are rendered from saved matter data. Document titles and printed references remain mandatory; the date column is optional. Dates are actual recorded values or explicitly absent, never invented. Compact and readable index layouts remain available; the new review layout provides a spacious linked index. Index density and optional technical Part PDFs remain in a disclosure.

All new allocation rules live in `backend/app/engine.py`: cover pages count toward volume capacity; index density/date spacing controls index page counts; source ranges, CSV, generated links, bookmarks, printed labels, PDF labels, combined offsets and Bates all use that shared map. Cover pages are excluded from Bates; the existing include-index flag applies only to index pages. Review layout uses eight rows per index page. Date-enabled compact layout uses readable spacing. No allocator was added to the UI.

`render_front_matter` is shared by preview and build. Draft preview is read-only. Actual PNG/text equality is tested by independently reopening and rasterizing published PDFs. Applying settings changes the next build and its freshness state, never prior artifact bytes. Originals and all prior native build files were hash-checked unchanged. The new settings are backward-compatible defaults in existing JSON storage; there is **no SQLite schema migration** (schema 19 / portable package 15 remain).

Source revision identity, included membership, build cancellation/retry, generated review, artifact validation/release gates, native/browser Save, Office/print/protected-copy tools and Storage remain connected. The bookmark prefill integration now waits for the new tab's editor to mount, retaining existing page-level bookmarking behavior.

## Visual comparisons and deliberate adaptations

Local evidence follows the repository's ignored `docs/ui-evidence/b02a/` convention. All **32 BU01/BU02 matrix screenshots** cover 1440×900 and 1024×768, light/dark and 90/100/110/125% scales, plus `11C-index-1440-light.png`. Representative final images were visually inspected against the reference-size previews, including compact dark 125% views. Exact geometry, radio alignment, field heights, generated index heading, dialog focus restoration and horizontal overflow are checked. Compact dialog panels scroll while actions stay visible; compact BU02 puts settings behind a disclosure and lets the actual pages scroll vertically.

- Reference-size comparisons: `BU01-1440-light-100.png`, `BU02-1440-light-100.png`.
- Compact examples: `BU01-1024-dark-125.png`, `BU02-1024-dark-125.png`.
- Generated native output: `native-cover.png`, `native-index.png`, `native-saved.pdf`.
- Manifest, preview, settings, restart settings, CSV, validation report and hashes: `native-*.json`, `index.csv`, `validation-report.json`, `verification.json`.

Runtime adaptations are explicit: local preset names/counts and real matter totals replace the illustrative values; provenance and override controls add necessary detail; actual A4 PDF aspect ratios and printable type replace drawn preview text. Mandatory index title/reference checkboxes are disabled/checked. Manage variants is disabled and labelled unavailable because variants are outside B02A. Existing physical numbering is described honestly rather than displaying the reference's unimplemented B02B label syntax. Platform-owned window controls/Save dialogs remain native. These are scoped adaptations, not a claim that every design frame or future capability is implemented.

## Commands and observed results

| Check | Observed final result |
| --- | --- |
| `cd backend && .venv312/bin/python -m pytest -q` | **265 passed**, six existing warnings, 44.30s. |
| `cd frontend && npm run build && npm run test` | Production build passed; **44 unit tests passed**. Existing Vite chunk-size warning remains. Production compilation also ran in the final native build. |
| Scoped B02A browser verification | Both new B02A cases passed; final focused run with two affected existing regressions: **4 passed**. |
| `COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- --output=/private/tmp/courtbundle-b02a-final` | **97 passed, 1 opt-in vault skipped**, 6.1m. No application source changes after this run. |
| `backend/.venv312/bin/python backend/scripts/build_sidecar.py` | Matching Apple Silicon PyInstaller sidecar built. |
| `cd frontend && npm run tauri:build -- --bundles app` | **69.74 MiB** release app built at `frontend/src-tauri/target/release/bundle/macos/CourtBundle POC.app`. |
| Isolated debug Tauri QA app | Built using a separate application ID and local synthetic backend, then exercised through native UI. |
| `git diff --check` | Passed; pre-existing staged work preserved. No commit/reset/deployment. |

Five new backend tests cover multivolume cover/date/review-index allocation, Bates, CSV/labels/links, preview/export pixel and text equality, originals/prior output preservation, atomic conflict/failure/Undo/Redo and explicit override preservation. Browser cases cover reopen persistence, cancel, caller focus, injected failed save and successful retry, a delayed real preview result, navigation's atomic save and the complete visual matrix. The full suite retains membership, PDF editing/review, generated output, validation and export regressions.

Earlier runs exposed a nested non-reentrant matter lock in settings save, inherited field/radio styling, a clipped review-index heading and bookmark prefill before editor mount. These were fixed and covered before final verification. Existing journey selectors were updated for the new tabs; the annexure disclosure is reopened after remount. The final green suite does not drop the underlying behavior assertions.

## Native reconciliation and boundaries

Used `CourtBundle B02A QA.app` (`io.courtbundle.desktop.b02aqa`) against isolated port **8774**, synthetic matter `0a5b497b09ed4dcda73eefb8bde392b5`. The user's engine at 8765 was untouched. In the native UI, edited the cover title to **NATIVE ACCEPTED BUNDLE**, applied settings, observed the old build become stale, rebuilt and opened generated PDF review. The source bookmark reached physical page **3**. Cancel in macOS Save returned “Save cancelled. The generated build is unchanged”; retry saved the selected combined PDF successfully.

Final build `df5c3a06c4f44b489e993ac591cc4c75`: **3 source + 2 generated = 5 physical pages**. Independently reopened the native saved file with PyMuPDF: cover/index text and raster pixels match the current preview exactly, PDF labels are 1–5, outline matches the saved engine contract, the index link targets physical page 3 and CSV gives source range 3–5. Its SHA-256 matches the immutable manifest:

`9347aefdc1db0405c45094e5f737fc6c87e5d5ce1d1e28747db9a7414dee5490`

The packaged sidecar matches its build input:

`654e28f2f57df8921ee9186147d2bdab3a34447b58b085c1dabda70789a83463`

After closing the QA app and stopping only its engine, reopening the isolated backend retained the accepted cover title and Native review layout v1/provenance. All original/prior artifact hashes remained unchanged. Native inspection used the real Tauri/PDF.js reader and Save command with a development backend; the separately built release app was not launched against the user's vault.

Windows, Intel Mac, minimum-macOS/clean-device installation, packaged-vault runtime and native print/Office Save remain untested in this phase. Native Save cancellation/retry/success was exercised; native transport-failure recovery was not repeated here (B01C records that earlier test). B02A settings failure/retry was exercised in browser tests. Optional vault browser test remains skipped. Existing script/font constraints still apply to generated cover/index content; unsupported scripts do not become court-approved typography. No filing approval, legally verified OCR or verified signature is asserted.

**Next bounded task: B02B — numbering rules, current page map, bookmark/TOC destinations and preview reconciliation.** Do not interpret the retained old numbering controls as completion of B02B.
