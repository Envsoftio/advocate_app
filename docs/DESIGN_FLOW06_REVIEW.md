# FLOW / 06 — Focused review and return to preparation

15 November 2026. UI scope: ED24, ED33, ED34 and ED32. Backend endpoints, persistence, imported originals, PDF operations and deterministic page mapping are unchanged.

## Design source

Used `CourtBundle_JSON_For_Codex/screens/{ED24,ED33,ED34,ED32}.json`, their component definitions and semantic tokens, and compared the corresponding frames in `CourtBundle_Complete_Workflow (2).pen`. The geometry agrees; the older PEN uses “package” in several places where the supplied JSON uses “bundle”, and has older global-search metadata. The scoped JSON is authoritative for those differences. These four states have no supplied PNG previews; the shared ED01 PNG provides the shell reference. Browser screenshots are compared against the scoped frame geometry and content hierarchy, not claimed as a pixel-exact match to a nonexistent raster.

## Implemented states

- **ED24:** focused-reading context strip, retained floating navigation and document identity. Tool and Preflight inspectors hide during focus, and return with Escape; the current document/page stays in the same viewer.
- **ED33:** docked Bundle impact inspector when the latest generated output is stale. Shows the actual build ID, saved working revision, source count and recorded input revision. Rebuild, current page-map review and previous-output review use existing handlers. The notice collapses at compact widths to avoid covering the source list; the inspector retains its explanation. No fabricated page delta, crop history or illustrative revision number is displayed.
- **ED34:** source-context strip, full-width document header and a footer Return to Preflight action wired to the existing exact-origin return/recheck handler. Focus restoration waits for refreshed checks so transition timing cannot select the fallback prematurely. The existing finding panel, historical review-note persistence, current-revision checks and source tools remain intact. Rechecking produces a new run; a note on an earlier run is not silently copied to the new finding.
- **ED32:** generated PDFs opened from the editor use the editor header/mode/navigation layout, with disabled mutation modes, bookmarks and a docked Build artifact inspector. Actual physical/printed/source context comes from the existing manifest. Frozen output and staleness are explicit; source navigation, export, checks and rebuild retain their existing handlers. Build-workspace entry retains its separate 18A shell. Compact docking overrides were verified separately from the older overlay layout.

The legacy preparation-focused view remains available. PDF typography and zoom depend on actual documents and reader settings. Current MatterDraft branding replaces the historical design branding. OS chrome is not simulated.

## Verification

See the FLOW / 06 entry in `ACCEPTANCE_TESTS.md` for final executed results and platform limits. Synthetic browser coverage includes original/prior-output byte preservation, stale-state persistence, focus and page restoration, preparation return, source revision rejection, run-scoped review notes, real appearance settings, failed Save/Cancel/Discard, and header Save without export.

During verification, the first scale fixture bypassed the application's appearance state; it was replaced with actual Theme/Interface size controls. Visual inspection also found and corrected a compact generated-reader grid conflict and warning-text contrast. Existing document-state assertions were updated for the already-established PDF draft dialog labels and scoped to the panel now that the footer also offers Return to Preflight.
