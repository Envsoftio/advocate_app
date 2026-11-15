# FLOW / 07 — index, bookmarks and volume planning

15 November 2026. UI correction against the supplied FLOW / 07 family: 11A current map, 11B unresolved destination, 11C pending map, 12A bookmark editor and 13A volume planning.

## Reference and scope

The scoped files in `CourtBundle_JSON_For_Codex/screens/` are the layout source, with component overrides and semantic tokens resolved by the supplied reader. The 1440 × 900 `implementation/previews/11C.png` supplies the raster reference. The root `CourtBundle_Complete_Workflow (2).pen` was also compared: it is an earlier revision with the same main panel composition, older fixture counts and fewer auxiliary actions. The supplied folder's JSON takes precedence for those differences. Product branding remains MatterDraft Studio.

Existing 12A/13A implementations already provide the designed split editor and volume-policy/result panels. This pass preserves those controls and processing behavior instead of replacing their data paths. No backend, storage, deterministic page-map rule, PDF calculation or API contract changed.

## Screen mapping

| Screen | Implementation and verification scope |
| --- | --- |
| 11A | Document-order panel, live source/printed/combined map, selected-row reference summary, Presets and Run preflight, explicit recalculation, membership/variants and existing layout tools. Order footer opens the existing Manage Parts dialog. |
| 11B | Actual unresolved bookmarks now appear in a warning panel and row/header status. Multiple unresolved entries have an explicitly selected bookmark. Select destination opens that exact saved entry. Remove bookmark opens its deletion controls; deletion remains an unsaved editor change until Save bookmark. Inspect document opens the existing working-document path. |
| 11C | Pending inputs retain source counts and hide generated positions. Pending/error states retain recalculation/retry and existing stale-response guards. |
| 12A | Existing tree, labels, hierarchy, source-page controls and actual PDF destination preview retained. Recovery selection focuses the exact entry; removal focuses its delete action. Compact editor uses a scrollable stacked form/preview. Empty selection has truthful status. |
| 13A | Existing workflow settings, physical-page totals, estimated/measured sizes, source/combined ranges, reviewed split exceptions and recovery flow retained and re-exercised. No new splitting or numbering rules. |

The map's bookmark-ID lookup uses the existing read endpoint; the map itself still comes solely from `bundle-preview`. UI changes cannot repair or remove a bookmark merely by displaying the warning. The existing snapshot-guarded bookmark save remains responsible for persistence. Previously generated artifacts are unchanged.

## Design adaptations

Live matter names, document counts, PDF text, page labels and estimates replace illustrative fixture values. Manage Parts is labelled for the existing operation rather than implying that the example's Add group action creates a new grouping model. Removal requires the existing explicit editor delete/save flow. A warning describes an unavailable destination conservatively; it does not assume every unresolved target was caused by page deletion. Compact screens keep all controls reachable by scrolling; they do not reproduce a fixed desktop canvas at reduced width.

The reference's main panels are checked at 1440 × 900: map left x=208, width=282; map right x=506, width=910; y=266. Existing bookmark and volume panel geometry is retained. This is a visual comparison and functional regression record, not a pixel-perfect or court-approval claim.

## Verification

Final results and commands are recorded in [the acceptance log](ACCEPTANCE_TESTS.md). Browser fixtures use isolated synthetic stores. Native package compilation is distinct from native GUI/OS Save verification; Windows, Intel Mac and clean-device installation remain outside this pass.
