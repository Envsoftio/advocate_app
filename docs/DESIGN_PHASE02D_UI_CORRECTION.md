# Phase 01/02 UI correction — new design shell

> Follow-up: the [JSON fidelity audit and correction](DESIGN_PHASE02E_JSON_FIDELITY.md) restores the missing 05A preparation workspace and corrects ED01/ED01I/ED24. This record preserves the earlier evidence.

28 October 2026. User-directed correction of the previously delivered visual foundation and reader. **Implementation and browser acceptance complete for this correction.** This is not B01 or a later capability phase.

The previous 01B/02A–C implementation records established functional behavior, but did not establish fidelity to the supplied editor layout. The user rejected retaining the old matter layout. That visual completion claim is reopened here.

## Layout contract

The supplied ED01 / ED36 / ED40 reader structure is the implementation target: 40 px application bar, 64 px matter rail, 64 px document header, 44 px mode row, 44 px navigation row, 200 px page/navigation panel, and 28 px status bar. These are design pixels before the existing interface scale is applied. The reader fills the available application area; the old centered matter heading, statistics, workflow tabs and permanent source list do not surround it.

The new document browser is an explicit drawer. Workspace navigation/appearance and open tabs are explicit menus. Existing metadata, archive, Save copy, print, stamps and output tools remain accessible through document actions. The main Save button commits the existing pending draft or metadata state; Save copy remains the existing flattened PDF output operation.

Read, Edit, Annotate, Pages, OCR and Redact are backed by existing operations. Imported originals and generated PDFs use the same full-height document structure with disabled editing controls, explicit artifact identity and retained source/bookmark navigation. No fixture content is substituted for runtime matter, document, revision or page data.

No backend changes are introduced by this correction. Existing original preservation, deterministic page map, draft guards, independent artifact reading positions and disconnected-engine recovery remain required behavior.

## Implementation

- `frontend/src/App.vue`: new application bar, matter rail, document drawer, workspace menu and status bar; existing workspace state and navigation guards retained. Details docks beside the canvas. Search focus survives section transitions; notifications do not intercept editing controls.
- `frontend/src/reader-design.css` and `main.ts`: the new layout and responsive sizing, using the existing semantic tokens and bundled Inter. Native window decoration remains platform-owned.
- `PdfViewer.vue` and `PdfReaderControls.vue`: document/version header, real draft Save with pending-state protection, six primary tool modes, View and additional-action menus, and separate page-panel navigation. Existing handlers remain responsible for edits and exports.
- `PdfThumbnailRail.vue` / `PdfReaderViewport.vue`: design-sized page cards and canvas gutters, retaining bounded thumbnail rendering, real page rasters and reading-position behavior.
- `ExportPdfReview.vue`: matching full-height read-only structure, explicit preserved-source/generated identity, and docked artifact information.
- `DocumentTable.vue`: history actions remain available in the table. Empty-reader history remains reachable after undoing an import.
- Existing browser journeys now open the new drawers/menus explicitly. New `reader-design.spec.ts` checks reference geometry, real PDF interactions and the theme/scale matrix. New `reader-header-save.spec.ts` holds an actual annotation request and checks one save, disabled pending Save, no download and unchanged original hash.

This is a replacement of the delivered shell and Phase 02 reader layouts. Preparation screens remain accessible through the rail; redesign of later capability phases is not claimed. Document text, titles, page counts, hashes and revisions come from actual imported data. Screenshot PDFs are synthetic test fixtures only. Browser previews do not imitate operating-system window buttons.

## Verification

Final production build and **44 unit tests pass**. The final full Chrome run returned **81 passed, 3 failed, 1 skipped** (5.7 minutes). The three failures concerned link-drag readiness, reopening the Save copy menu, and workspace-menu focus after a cancelled tab close. Added an actionability wait before drawing, explicitly reopened the real action menu, and made the workspace menu dismiss when focus returns outside it. The final focused run returned **11 passed**, including all five document-state guards, those three workflows, appearance/keyboard behavior, reference geometry and header Save. **All 84 enabled browser cases passed across the full and focused runs; this is not a claim of a single all-green full-suite run.** The vault case requires opt-in and remains skipped.

Visual review covers working and original readers, including the reference 1024×768 light layout and 1440×900 dark layout. The automated matrices cover both sizes, both themes and 90/100/110/125% interface scales. Working and immutable readers retain the existing visible-paper thresholds, focus checks, draft guards and actual navigation. Reference rows measure 40/64/44/44 px with the canvas column beginning at x=264 at 100% scale. Page thumbnails render real source pages. PDF typography remains authored document content; the synthetic reference fixture is not an application asset.

Final commands (from `frontend`):

```sh
npm run build && npm run test
PLAYWRIGHT_JSON_OUTPUT_NAME=/private/tmp/courtbundle-design-final.json COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- --reporter=list,json --output=/private/tmp/courtbundle-design-final
PLAYWRIGHT_JSON_OUTPUT_NAME=/private/tmp/courtbundle-design-final-checks.json COURTBUNDLE_E2E_VITE_PORT=1436 npm run test:e2e -- --grep 'reviewed link and output|Save copy downloads|matter and document tabs|02B|appearance persists|new reader follows|document header Save' --reporter=list,json --output=/private/tmp/courtbundle-design-final-checks
```

Tests used isolated synthetic stores and backend port 8768. Application source stayed unchanged during each browser run. Earlier correction runs exposed legacy selectors and the actual history, compact-details, notification-hit-testing and keyboard-focus regressions addressed above. The existing Vite chunk-size warning remains. `git diff --check` passes. No backend pytest rerun, backend processing changes, commit or deployment occurred in this UI correction; pre-existing backend and `.gitignore` changes were preserved.

[Reference-size light reader](ui-evidence/02d/reader-1024-light.png) · [Dark reader](ui-evidence/02d/reader-1440-dark.png) · [Read-only original](ui-evidence/02d/original-1440-light.png) · [Verification summary](ui-evidence/02d/verification.json).

Native Tauri/WKWebView, OS print/save dialogs, sidecar packaging, encrypted-vault opt-in, Windows and clean-device acceptance remain unverified by this pass.

## Rule for subsequent tasks

Use each task's supplied scoped screen JSON, resolved component overrides, semantic tokens and preview as the layout source. Preserve working capabilities and data, not legacy layout. Before marking a UI task complete, compare rendered screenshots at its reference viewport(s), inspect light/dark and supported interface scales, and verify the relevant real interactions. Do not postpone an implemented screen's design fidelity to a later capability phase.
