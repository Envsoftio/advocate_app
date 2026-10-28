# B02B — New numbering, page map and bookmark design

28 October 2026. Implementation and verification record for **BU03**, **11C** and **12A**. This scope replaces their legacy layouts with the supplied JSON composition. BU01/BU02 remain integrated. B02C is not started. **[Certain] Scoped implementation, visual/browser acceptance and available Apple Silicon native preview/Save checks are complete.** Technical QA is not court approval.

## Design and behavior checklist

| Requirement | Implementation and evidence |
|---|---|
| Authoritative design | Read B02 phase/shared instructions, tokens, reference index and `preset_cover_numbering.json`; expanded reusable refs and descendant overrides directly with `read_design.py`. BU03 and 11C PNG references inspected. No supplied 12A PNG exists; its resolved JSON hierarchy/dimensions are the reference. |
| BU03 new composition | Dedicated `NumberingSettings` and `NumberPlacementPreview` replace the primary legacy numbering layout. At 1440×900: left x208/y266/w440/h586; right x668/y266/w748/h586. Bundled Inter, semantic tokens, designed tabs, top-right Apply, paired style/restart and start/prefix controls, position/front matter/navigation/Bates controls. Browser geometry assertions pass. |
| 11C new composition | Part-grouped document order at x208/w282; six-column map at x506/w910, both y266/h586. Header Presets/Recalculate actions. Source counts and bookmark source identity remain visible while printed/combined destinations become Pending, Recalculating or Error. Current requires a successful current request. |
| 12A new composition | Tree x208/w286, editor x510/w906 with 350px form and 555px actual destination preview. Generated destination is in the form; Save bookmark is top right. Real source selector/page/revision and explicit resolution. Hierarchy/deletion and CSV import remain secondary disclosures. |
| Number styles | Arabic, lower Roman and upper Roman. Positive start 1–3999; Roman values above 3999 use repeated M. Prefix is bounded Latin-1 text with explicit `{volume}` / `{section}` substitutions. No frontend number generator. |
| Restart | Each volume; continuous; each Part section. Part transitions reset section numbering, while the same Part spanning volumes continues. Separators consume content numbers. |
| Front matter | Existing counted/stamped content policy; separate lowercase Roman viewer labels with no printed stamp; or unlabelled front matter with no printed stamp. Separate front matter does not consume content numbers. Physical cover/index pages always count toward volume limits and destinations. |
| Placement | Left/center/right footer and left/right header. Center footer preserves existing 24pt policy; other positions use 12mm inset. Labels that cannot fit are rejected, never silently clipped. Actual source preview uses the export renderer and current source annotations/marks. |
| Navigation | Matching PDF page labels, linked index and back-to-index are independent saved choices. Bates is independently configured and previewed. PDF label strings are escaped/encoded correctly, including accented text, parentheses and backslashes. |
| Single map | `engine.numbering_maps` extends the deterministic volume map. Index references, stamp records, PDF labels, outlines, links, CSV physical/printed ranges, combined offsets, previews and frozen review labels use its output. |
| Atomic save/history | Existing settings transaction, expected-input conflict detection, field overrides, explicit Apply/Cancel, reload persistence and Undo/Redo preserved. Optional new output-mark defaults preserve older data without SQLite migration. |
| Bookmark persistence | Explicit draft/save/discard, parent nesting, keyboard/pointer sibling ordering, generate-from-index, page selection and reviewed CSV import. Loading gates editing and queues map-prefill, preventing replacement of existing bookmarks by an early Add. Save uses expected snapshot and retains drafts on failure. |
| Invalid targets | Current included document, exact revision and bounds required for preview. Missing/stale targets display Needs resolution, have no generated destination and are omitted from PDF outlines until explicitly resolved. No silent retarget/clamp. |
| Invalidation | Edits, order, membership, revisions and settings invalidate current map/placement. Request generations reject late responses. Unsaved drafts do not mutate prior outputs. Bookmarks survive internal tab switching while dirty; cross-workspace Save/Discard/Keep editing remains active. |
| Preservation | Original hashes and frozen prior files remain unchanged. B01 build/progress/retry/review, output checks, release gates, native/browser export, Office/print/protected copies remain connected. No cloud dependency or future B02C/B03/B04 feature added. |

## Files

Backend: `app/engine.py`, `schemas.py`, `main.py`, `storage.py`, `validation.py`, and `tests/test_bundle_numbering.py`. New endpoint POST `/matters/{id}/numbering-preview` accepts a source document/page/revision plus draft settings. Existing bookmarks PUT gains an optional expected snapshot. PDF viewer labels are independently read with the already-bundled PDFium engine; PyMuPDF's raw escaped-string label representation is not used as the Unicode truth.

Frontend: new `NumberingSettings.vue`, `NumberPlacementPreview.vue`; rebuilt `BundlePreview.vue` and `ManualBookmarks.vue`; integrated `BundleStructure.vue`, `BuildWorkspace.vue`, `App.vue`, `ExportPdfReview.vue`, `lib/types.ts`, `lib/bundleSettings.ts` and `structure-design.css`. `PdfViewer.vue` output-mark dirty tracking uses a reactive saved baseline and complete defaults, avoiding false unsaved prompts after archive/restore. New `e2e/bundle-numbering.spec.ts`; affected journey/document-state/structure assertions updated to the new UI while retaining functional requirements.

No dependency or database schema change. Existing staged and unstaged work was preserved; no commit was made.

## Visual evidence

Local ignored evidence directory: [`ui-evidence/b02b/`](ui-evidence/b02b/). Screenshots use synthetic matter data, not legal case files.

| Reference | Running application |
|---|---|
| [BU03 supplied PNG](ui-evidence/b02b/reference-BU03.png) | [BU03 1440 light](ui-evidence/b02b/BU03-1440-light-100.png) |
| [11C supplied PNG](ui-evidence/b02b/reference-11C.png) | [11C 1440 light](ui-evidence/b02b/11C-1440-light-100.png) |
| `screens/12A.json` resolved nodes; no supplied PNG | [12A 1440 light](ui-evidence/b02b/12A-1440-light-100.png) |

All three screens captured at **1440×900 and 1024×768**, **light/dark**, **90/100/110/125%** (48 captures); contact sheets `BU03-matrix.png`, `11C-matrix.png`, `12A-matrix.png` inspected. Additional screenshots cover pending/error maps and the compact keyboard settings panel. Desktop panel coordinates are checked against the reference. Compact layouts scroll instead of compressing PDF content or hiding controls; map columns scroll horizontally, and numbering settings use a keyboard-accessible disclosure. Actual PDF aspect ratio, source text/counts and generated destinations replace reference fixture content. Advanced capabilities absent from the drawing remain in secondary disclosures.

## Verification

Final command results and native reconciliation follow. Earlier failures were fixed, not counted as passes: bookmark load/save timing, stale legacy UI selectors, reactive dirty-state reset after archive/restore, and generated-reader printed-label context.

Coverage includes all nine style/restart combinations, blank/Roman front matter, separators, multiple volumes, independent viewer-label parsing, all five positions and escaped prefixes, exact preview/export pixels, CSV, outlines/index/backlinks, reordered sources and unresolved bookmarks, conflict/Undo/Redo, and original/prior hashes. Browser tests cover save/cancel/retry, late responses, state transitions, nested bookmarks, load-prefill races, keyboard/drag ordering, compact scrolling/focus and generated reader content/labels.

## Native/platform boundaries

Isolated `CourtBundle B02B QA.app` (`io.courtbundle.desktop.b02bqa`) uses port 8776 and a synthetic store. The user's engine on 8765 is untouched. The debug QA shell and release app are compiled separately. Native checks exercise accepted settings, stale old output, rebuild, actual PDF review and macOS Save cancellation/retry/success. The final QA shell is also exercised against the rebuilt packaged sidecar on the isolated port; this is not a packaged-vault/clean-install qualification.

Windows, Intel Mac, minimum-version/clean-device installation, packaged-vault startup, native print/Office Save and native transport-failure injection are not verified in this phase. Browser failure/retry is tested. The opt-in vault browser test remains outside the normal synthetic run. Existing script/font limitations remain; numbering prefixes explicitly support Latin-1. No legal approval, OCR verification or signature verification is claimed.

## Final acceptance results

| Command actually run | Result |
|---|---|
| `cd backend && .venv312/bin/python -m pytest -q` | **285 passed**, six existing dependency/deprecation warnings, 54.32s. |
| `cd frontend && npm run build && npm run test` | Production build and **44 unit tests passed**. Existing Vite chunk-size warning remains. Production compilation also passed in final native builds. |
| `COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- --output=/private/tmp/courtbundle-b02b-complete` | **101 passed, 1 skipped** (opt-in vault), 6.3 minutes. Includes B01/B02A, editor/navigation/archive/history/export regressions and four new B02B browser cases. |
| `backend/.venv312/bin/python backend/scripts/build_sidecar.py` | Matching `filing-api-aarch64-apple-darwin` built successfully. |
| `npm run tauri:build -- --debug --bundles app --config /private/tmp/courtbundle-b02b-native-config.json` | Final isolated QA app built successfully, 36.68 MiB. |
| `npm run tauri:build -- --bundles app` | Apple Silicon release application bundle built successfully, 69.77 MiB. |
| `backend/.venv312/bin/python /private/tmp/courtbundle-b02b-reconcile.py` | Native saved bytes, labels, outlines, links, printed regions, preview raster/text, CSV and preservation assertions passed. Script retained in evidence directory. |
| `git diff --check` | Passed. |

No remaining failed scoped acceptance check. All results above are from actual runs; untested platform/runtime combinations remain explicitly listed above. Earlier full browser run was 93 pass/6 fail and the first extended backend run had a malformed new test request; those are superseded by the final full passing runs, not hidden as passes.

### Native reconciliation

Synthetic matter `43304c7aa2aa4c1ca0ad9338c9d1ad7a`; final packaged-sidecar build **`51bb7a15102b481f8d95e04edff15057`**. Native settings were changed to lower Roman, start 17, `V{volume} · ` prefix, each-volume restart, 12mm right footer, separate Roman viewer front matter and backlinks. Restart confirmed the accepted values. Old output became stale without byte changes. Final output has **6 source + 4 generated = 10 physical pages across two volumes**.

The native outline opens Affidavit at combined page **8**, whose printed/PDF label is **V2 · xvii**. Its backlink reaches the second volume's index at combined page **7**; that index returns to page 8. The final review banner uses frozen engine labels. Actual PDF canvas/text rendered successfully. Initial QA inspection briefly showed a blank canvas; final rebuilt-app review rendered the cover and target content promptly and was captured. Native Save cancellation/retry/success passed; final Save was repeated using the packaged sidecar.

Independently parsed both volume PDFs and combined PDF. Every viewer label (PDFium), outline, index/backlink target, printed label region, CSV range and frozen map matched. Actual target-page PNG/text equals the current placement preview exactly. All original hashes and every file of initial build `a3da193c464b40f096a4c40613813a44` remain unchanged. Final native saved PDF SHA-256 matches the manifest:

`0ba9c5c1c76434476cec1bf3287bbf04c9bb90f58ee4dfe3dc516bff75398b3d`

Evidence: [verification JSON](ui-evidence/b02b/verification.json), [native map](ui-evidence/b02b/native-map.png), [reopened numbering](ui-evidence/b02b/native-numbering-reopened.png), [final review](ui-evidence/b02b/native-review-final.png), [final index](ui-evidence/b02b/native-index-final.png), [final Save](ui-evidence/b02b/native-save-final.png), [saved PDF](ui-evidence/b02b/native-saved.pdf). Frozen manifest, current preview/settings, CSV, validation state and test/build logs are retained alongside them. Evidence is local and git-ignored, as in previous phases.

The next bounded task is **B02C**. It was not started automatically.
