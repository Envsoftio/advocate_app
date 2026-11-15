# FLOW / 03 UI review — 15 November 2026

Layout sources: the supplied `.pen` and `CourtBundle_JSON_For_Codex/screens/ED01–ED06`, ED04B, ED31, ED30, ED29, ED26, ED27, ED01I and ED27B JSON. The root `.pen` and exported JSON differ in search-navigation metadata and icon node names; the four changed context frames (ED02, ED05, ED01I, ED27B) have identical visual properties when metadata/name fields are excluded, and their semantic variables match. Existing implementation and regression evidence are in DESIGN_FLOW03_CONTENT_EDITOR_REVIEW.md and DESIGN_FLOW03_CONTENT_BRANCHES.md. This pass changes frontend presentation and uses existing API contracts; backend storage, imported originals and the deterministic page map are unchanged.

## Changes

- ED02 now has the design's Document contents inspector, object-selection guidance and working-copy preservation note. Text, Image and Link choices invoke the existing tools.
- ED05 selected images stay in Edit, with an image preview and point-based position/size fields. Values are bounded to the page and saved through the existing revision-checked annotation PATCH. Drag, resize, remove, draft guards and output flattening retain their existing paths.
- ED01I keeps Read active and supplies View imported original and Open comments actions using existing navigation. Source hash and real metadata remain available.
- ED27B shows the real available redo action, a distinct undone history row and an undo-outcome notice. The existing matter history remains authoritative; no fictional per-document history or unsaved state is substituted.
- Existing ED01, ED03, ED04, ED04B, ED06 and recovery/source/unsaved states remain connected to their existing handlers.

## Design limits

This is not full design-feature parity or a literal pixel-match claim. ED05 image replacement, image-content crop, rotation and stacking controls have no matching existing atomic API and were not fabricated in a UI-only update. ED03 source-text font/alignment editing differs from the designed paragraph inspector; existing run replacement and movement remain available. Supported added-text font choices remain the real built-in fonts. Live matter names, PDF content, page counts, history labels and save state differ from the example fixtures. ED30 diagnostics remain inline, and ED26 describes the actual pending draft. These screen families should not be marked as having every illustrated capability implemented.

## Evidence

Synthetic browser captures at 1440 × 900 are saved under `ui-evidence/flow03/`, including ED02-object-selection-1440.png, ED05-image-1440.png, ED01I-information-1440.png and ED27B-undo-applied-1440.png. Comparison uses the resolved JSON inspector bounds (x1148, y192, width292, height680) and supplied ED03/ED04/ED31 rasters for common chrome. Functional evidence includes real image geometry persistence, original navigation, saved text Undo/Redo, existing overlays, recovery and working-copy saving. Final results are recorded in ACCEPTANCE_TESTS.md.

Native WebView interaction, OS save dialogs, Windows and Intel Mac are separate acceptance gates; a successful macOS compile does not establish those results.
