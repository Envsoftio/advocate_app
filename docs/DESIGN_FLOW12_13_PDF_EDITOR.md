# FLOW / 12–13 — compact and dark PDF editor

17 November 2026. Scope: ED36, ED37, ED38, ED38B at 1024×768 and ED40–ED43 at 1440×900. The scoped JSON in `CourtBundle_JSON_For_Codex/screens/` is the layout authority, checked against the repository `.pen`, shared components, tokens and supplied ED36/ED38B PNGs. The older `.pen` differs in metadata/naming; the named JSON folder takes precedence. Real synthetic PDF pages replace illustrative document content.

## Implementation

- Compact Edit opens with its useful canvas and page navigation, not an automatic object-chooser column. Context properties open deliberately; other editing tools still expose their required controls.
- Properties overlay the compact canvas in a 300 px drawer. Collapse and Escape preserve the active tool, selected object and pending draft; the toggle restores them and receives focus after collapse. Desktop properties retain the 292 px dock.
- Text replacement now lives in the actual properties panel rather than behind the drawer on the PDF. Existing replacement, paragraph-trial, signature, cancellation and save handlers are retained.
- Compact Edit/Annotate use floating page/zoom controls. View retains additional reader commands. The compact editing hint no longer covers selectable PDF text; actual tool errors remain visible.
- Pages preserves the application/document/mode bars, starts the toolbar at y148 and the content at y192. Actual 144×186 page cards, selection counts, source ranges and a desktop Page selection panel connect to existing guarded insertion/extraction operations. Selection options, keyboard/drag ordering, confirmations, signature warnings and Undo remain intact.
- Page-organizer viewport sizing accounts for interface scale; close/footer actions remain reachable at 90/100/110/125%. Light/dark paper keeps authored PDF colors.

No backend processing, API contract, database migration, page-map or original-file changes are part of this UI correction.

## Requirements audit

The canonical `MatterDraft_Project_Context_and_Handoff.md`, README and implementation backlog define the behavior contract. Existing operations retain explicit selections, working revisions, signature guards, revision conflicts, save-failure recovery, private-review omission and original preservation. New checks cover collapsed draft retention, real annotation persistence, original-byte equality and reference-size/scale layout. Existing suites cover actual page operations, text editing, document switching, independent reader positions, exports and dirty-state navigation.

This correction does not implement the entire PDF roadmap. Underline/strikeout remain visibly disabled under Phase 03B. General paragraph reflow/font substitution, arbitrary font-weight editing, verified signing and legal/court qualification remain outside existing engine support. The constrained text-run/experimental paragraph tools retain their existing restrictions; no cosmetic control promises unsupported PDF transformations. Resizable/persisted panel separators from the design metadata remain a separate unimplemented interaction. Native Save/print, Windows, Intel Mac and clean-device qualification cannot be inferred from browser tests or a successful Apple Silicon build.

## Verification

Final executed results are recorded in `ACCEPTANCE_TESTS.md`. Local screenshot evidence is retained under `docs/ui-evidence/flow12-13/` (ignored generated evidence). Comparison uses the supplied previews and JSON geometry at 100%, with additional light/dark scale checks. This is not a pixel-perfect claim: the application retains real filenames/revisions, additional supported actions, View access and explicit capability limits.
