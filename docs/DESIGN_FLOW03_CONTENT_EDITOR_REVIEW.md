# FLOW / 03 — PDF reader and content editor review

8 November 2026. Design source: `CourtBundle_JSON_For_Codex/SCREEN_INDEX.json`, `screens/ED01.json` through `ED06.json`, `screens/ED04B.json`, `screens/ED36.json`, and the available preview PNGs. The reference viewports checked were 1440 × 900 and 1024 × 768. All runtime captures use a synthetic PDF.

## Implemented in this pass

- The editor has the supplied 40 px application bar, 64 px icon rail, 64 px document header, 44 px mode row and 44 px Edit tool row. Open workspaces remain accessible from the application bar menu.
- The Edit row presents Text, Add text, Image, Link and Select object. It routes to the existing working-copy text replacement, added text, image annotation and reviewed link workflows. Redaction remains in its own mode.
- Page thumbnails stay visible during Edit. Context controls dock in a 292 px right inspector at the 1440 px reference width, instead of floating over the PDF. The reader's Undo and Redo controls remain available in its header.
- Add text shows the actual built-in PDF font, supported point-size range and printable-ASCII limit. This avoids advertising the reference's Georgia option when the PDF implementation uses Helvetica. The selected-text inspector shows the chosen text excerpt. The original imported PDF remains separate.

## Screen comparison

| Source state | Runtime evidence and result |
| --- | --- |
| ED01 / ED36 Read | The compact reader shell geometry is asserted by `reader-design.spec.ts`; search, bookmarks, position restoration and working-copy navigation are exercised. PDF contents and matter names differ because the test uses synthetic material. |
| ED02 choose an object | Edit mode displays the five supplied content tools, thumbnails and an editable PDF surface. |
| ED03 selected text | [1440 × 900 capture](ui-evidence/flow03/ED03-edit-text-1440.png). Selecting a real PDF text run opens the replacement card and selected-text inspector. The synthetic selection is one text run; the whole-paragraph option remains available. |
| ED04 Add text | [1440 × 900 capture](ui-evidence/flow03/ED04-add-text-1440.png) and [1024 × 768 capture](ui-evidence/flow03/ED04-add-text-1024.png). The contextual inspector is docked and usable at both widths. |
| ED04B placed object | [1440 × 900 capture](ui-evidence/flow03/ED04B-placed-text-1440.png). The inline card edits the draft before Apply text; the test applies it and checks that the text overlay appears on the working PDF. The reference's object typography, alignment and position fields are not implemented. |
| ED05 Image | [1440 × 900 capture](ui-evidence/flow03/ED05-image-1440.png). A synthetic JPEG is placed and selected; the existing overlay can be moved, resized or removed. The reference's image crop, rotation, replacement and layer-order controls are not implemented. |
| ED06 Link | [1440 × 900 capture](ui-evidence/flow03/ED06-link-1440.png). The test draws a link region, chooses the current matter document and verifies the save action is enabled. The source's single-document page choice is represented by the current document plus page field; no link is saved in this capture. |

These captures were inspected against the supplied PNGs and JSON. The editor shell, tool order, panel location and main viewport follow the reference. Literal pixel identity is not claimed: MatterDraft branding, synthetic PDF content, page count, PDF font support, inspector capabilities and browser rendering differ. ED04B and ED05 remain functional subsets of their source designs because the missing object properties need real PDF engine support.

## Verification

- Configured backend suite: `cd backend && .venv312/bin/python -m pytest -q` — 336 passed, six existing warnings. No backend or deterministic page-map code changed.
- Final frontend: `cd frontend && npm run build && npm run test` — build passed; 48 unit tests passed. The existing large-chunk advisory remains.
- Focused synthetic Chrome run: 8/8 passed across `matter-creation-flow.spec.ts`, `pdf-reader.spec.ts`, `read-workspace.spec.ts` and `reader-design.spec.ts`. A subsequent placed-object check passed 1/1 after moving the link region away from an existing overlay. These checks cover text placement, image placement/selection, link-area validation, inspector position, compact size, reader search and bookmarks, source revisions, and creation navigation.
- A broad Chrome run before the final selector corrections recorded 137 passed, four failed and one opt-in vault skip. The four failures were an empty-matters fixture contaminated by prior cases, an older FLOW / 01 heading-coordinate assertion, and two reader assertions using the previous toolbar/chrome. The empty-matters case and reader cases passed in the later isolated run; the FLOW / 01 heading mismatch remains a separate known issue. A fully green broad rerun is not claimed.

Native compile status is recorded in `ACCEPTANCE_TESTS.md`. Packaged native GUI interaction, OS Save/export dialogs, Windows and Intel Mac require separate checks.
