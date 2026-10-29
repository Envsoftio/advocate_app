# Matter navigation and import flow correction — 30 October 2026

The supplied `CourtBundle_Complete_Workflow (2).pen`, full `CourtBundle_Complete_Workflow.json`, scoped 01A/04A/03A/03B JSON and the owner's screenshot define this correction. The old cards, Overview dashboard and second navigation sidebar have been replaced. This record supersedes earlier descriptions of those surfaces.

## Design and navigation contract

The full JSON contains 133 screen frames. The navigation audit finds 37 expanded 184 px sidebars, 26 compact 64 px sidebars and one 80 px variant. All four requested FLOW / 01 screens use a 184 px sidebar at y=116, with a 32 px app bar, 40 px tabs, 44 px utility bar and 28 px status bar at 1440×900. 05A uses the compact rail. To address the owner's reported unexpected changes, expand/collapse is now one persisted workspace preference across matter sections. Narrow windows under 760 CSS px use a compact rail; focused reading can still hide navigation deliberately.

The implemented path is **Matters → Overview → Import queue → Import processing/results → Documents preparation → editor / Index / Preflight / Build / Storage**. Creating a matter opens Import. A permanent Matters tab and the matter sidebar's All matters action provide an explicit return. Workspace tabs continue to preserve document and matter contexts and existing draft guards.

The brand menu opens a small workspace/settings panel, not a second sidebar. Appearance, restore, court profiles and About remain reachable there; archived matters are on the Matters filter. Overview's Matter details contains case information, PDF/print, Office exports, backup, archive and refresh. The status bar retains bundle membership and shows the active preparation step. The legacy disabled Open matter folder control is replaced by a functioning All matters return.

## Implementation

- `App.vue` owns the shared sidebar preference and stage navigation. Import queues are scoped to matter IDs. An in-flight upload pins its target ID even if the user browses elsewhere.
- `MatterHome.vue` renders the 01A table, search, filters, density and recent matter updates. Counts come from real documents and outputs; unavailable counts remain unavailable. Pending summary fetches stop advancing after the screen unmounts.
- `MatterOverview.vue` follows 04A's metrics, review action, preparation rows and activity column. Review counts derive from unconfirmed labels/dates and OCR-needed status. These are preparation hints, not legal verification.
- `ImportWorkspace.vue` implements 03A/03B with a visible queue, actual per-file stages, imported source-page counts and results. Supported files over 80 MiB and unsupported extensions are excluded; SHA-256 detects exact duplicates against current originals and the queue. A reviewer can explicitly include a duplicate. Password/signature prompts use the existing upload service. Failed files do not prevent later files from running. Cancellation stops after the current request and preserves successful imports. Retry remaining files excludes completed imports.
- `matter-flow.css` defines the reference shell, table and Overview/import geometry. Existing capability components and the backend page map are retained. Preparation sizing is scoped to preparation; the shared shell no longer changes full-screen reader geometry. Small-window Details and document-information layouts retain useful paper space. The image inspector stays mounted during image placement so the canvas does not resize mid-drag.
- Browser feature tests now explicitly enter their synthetic matter from the new Home screen. They do not depend on the removed drawer or automatic sidebar expansion.

## Evidence and limits

Reference extraction and full navigation audit are in `docs/ui-evidence/matter-flow/`. Browser renders include 01A, 04A, 03A and 03B at 1440×900; Overview also covers light/dark, 100/125% and 1024 px widths. Captures wait for transitions to finish. Synthetic file contents, actual counts, product branding and supported operations replace illustrative source data. The source's Windows titlebar buttons are platform-owned in the desktop app and are not simulated in the browser.

This is a layout and workflow correction, not a claim that every frame in the master design is implemented. OCR inspection before import and email-thread review are not available in the current local import service; the queue does not fabricate those results or offer an inert action. New matter retains existing supported metadata fields. A literal pixel-identical claim would be inaccurate where live data, branding, platform chrome and these supported capabilities differ.

Verification results are recorded in `ACCEPTANCE_TESTS.md` §DG. No backend processing/schema/page-map change is included.

### Native check

An isolated Apple Silicon debug bundle named `MatterDraft Flow QA` (`io.matterdraft.flow.qa`) used the freshly frozen sidecar with a disposable `/private/tmp/matterdraft-flow-native` store. Native Open imported `samples/01_petition.pdf`, the queue completed, Documents rendered selectable synthetic PDF text, and More document actions → Save working copy opened the macOS Save dialog. Saving to `saved-working.pdf` produced 6,004 bytes, three readable pages and SHA-256 `76da5e34c1799a5aff3d3e9bec8cc689cb6cf8430dfc163f93bfc3ec286bf17a`. Screenshot: `docs/ui-evidence/matter-flow/native-save.png`. After the shared-shell fixes, both release and isolated QA bundles were rebuilt. The QA app reopened the same synthetic store, rendered the final Overview and reader, kept collapse state across sections, and saved `01_petition-working-r1.pdf` with the identical byte count and SHA-256. App restart returned to Matters with the sidebar still collapsed. Native screenshots before the final queue geometry/page-count polish: `native-final-overview.png`, `native-final-save.png`, `native-restart-collapsed.png`; byte evidence: `native-save-verification.json`. No installed application or confidential matter store was replaced.
