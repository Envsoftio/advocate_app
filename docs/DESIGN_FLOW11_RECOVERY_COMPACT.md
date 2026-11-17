# FLOW / 11 — recovery, compact workspaces and scale

Scope: the supplied **Existing recovery and compact layouts** and **Existing scale and preflight coverage** groups: 21A, 21B and 22A–22F. Sources are the matching `.pen` group and the self-contained JSON files in `CourtBundle_JSON_For_Codex/screens/`. These eight entries have no supplied PNG previews. Geometry, component definitions, descendant overrides and semantic tokens are the reference; screenshots of the actual application are verification evidence, not supplied design images.

## Implementation contract

| Frame | Implementation and retained behavior |
| --- | --- |
| 21A · Engine unavailable | Persistent recovery notice, explicit Retry, mounted cached reading context and paused writes. Internet availability does not determine local-engine availability. |
| 21B · Diagnostics | 520 px right drawer, retained reader-specific page/revision/error context, real last successful API response, available/paused capability descriptions, Copy diagnostics, Retry and Continue browsing. Diagnostics exclude credentials, paths, document text and matter identifiers. Clipboard failure offers selectable text. |
| 22A · Compact Documents | Existing supplied preparation layout, collapsed navigator/details controls, real working/original PDF and retained source page. |
| 22B · Compact Index | 238 px order panel and adjacent page map at the 1024 reference viewport; compact table proportions keep all six columns visible at 100%. Bounded scrolling at enlarged sizes retains actual engine ranges and bookmark repair. |
| 22C · Compact Build | Existing B03C composition retained; actual saved settings, snapshot counts and guarded build routes. |
| 22D · Compact Batch review | Selection, title, type, Part, page count, OCR and review status are initially visible. Additional metadata remains editable via More fields/Columns. Resizing and hidden-field drafts remain intact; persistence uses the existing atomic API. |
| 22E · Compact Preflight | Four human-review controls across at the 1024 reference size, reflow below it. Actual findings, screening, warning acknowledgements and separate human review remain connected. |
| 22F · Documents at 125% | Existing scaled preparation layout retains explicit navigator/details access and readable PDF space. |

The current application shell is 4 px taller than the older reference at 100%; the user-selected sidebar expansion and details remain authoritative. Browser chrome does not imitate native window buttons. Counts, timestamps and document content come from actual session data, not the illustrative records in the design. Additional real capabilities such as page screening remain accessible even where the older reference omits them.

## PRD audit and correction

Recovery uses the existing bounded authenticated local connection and explicit saved-input comparison. A successful health response alone never unlocks writes. Retry does not replay edits, OCR or builds. Changed revisions require explicit review/discard; an unchanged revision retains mounted drafts for an explicit save. The corrected review path fetches fresh resources before discarding retained drafts, so a failed recovery read preserves the draft. Backend API contracts, schema, deterministic page map and original files are unchanged.

DAT-03 already persists job status and supports interruption retry as a new attempt. This screen does not claim halfway job resume or start jobs merely by reconnecting. DAT-05's autosave hold remains in force after interruption. Preflight acknowledgements remain tied to saved inputs and do not imply legal completeness or generated-output release. Compact layouts expose the same capabilities rather than substitute illustrative controls.

Physical power-loss durability, full PDF caching, native process supervision, Windows/Intel qualification and legal approval are separate existing acceptance boundaries. They are not inferred from browser recovery tests or from the supplied diagnostics copy.

## Verification

See the dated FLOW11 entry in `ACCEPTANCE_TESTS.md` for final executed results and remaining platform limits.

Current checks: 363 backend tests, 51 frontend unit tests and seven Rust tests passed. Twenty-five distinct selected browser scenarios passed across the recorded main/follow-up runs; final all-screen replay passed after the map-column correction. The Apple Silicon app and packaged-service smoke passed. See the acceptance record for the exact split of runs and native-device limitations. Local visual evidence is under `docs/ui-evidence/flow11/`.
