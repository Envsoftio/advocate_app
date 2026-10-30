# FLOW / 01 — Matter creation states

30 October 2026. Scope: `.pen` frames `screen-01B`, `screen-02A`, `screen-02B` at 1440 × 900. The `.pen` JSON hierarchy, visible text, field coordinates and frame dimensions were inspected directly. The browser renders were captured at 1440 × 900 with a fresh synthetic store in `docs/ui-evidence/matter-creation-states/`.

## Result

- **01B Empty Matters:** the empty state now uses the supplied title, icon, two actions, local-processing note and lower preservation note. `Open existing matter` activates the existing `.cbmatter` restore picker. The heading's measured box starts at x=212. A later clipping correction moved its top from y≈145 to y≈159 so the title and New matter button stay inside the page scrollport; this is a 15 px vertical difference from the illustrative frame.
- **02A New Matter:** the dialog now presents matter name, court/forum, case number, client/party reference, internal reference and notes in the supplied order. Its measured frame is x=370, y≈138, 700 × 638 px. The four new metadata fields persist in SQLite schema 23 and portable matter backups; older databases get empty defaults. Required-name validation, keyboard Escape and focus containment are active. Court profile and case type remain in a disclosure so the existing technical settings remain usable.
- **02B No Documents:** creation opens the empty Overview, showing zero documents/pages, the preparation pathway, Import documents and local Storage actions. The import action enters the existing working queue. The matter details dialog exposes the new metadata.

## Bounded differences from the illustrative design

The source design's `Browse…` suggests selecting a per-matter storage folder. MatterDraft's existing storage engine fixes the local workspace root and derives each matter directory from its server-generated ID. A folder picker would imply that arbitrary paths are honored when they are not. The dialog therefore displays **App-managed local workspace** with a Details action. The 02B action opens MatterDraft's local Storage view and is labelled **Open local storage**. Exact pixel identity and a selectable matter folder are not claimed. Product branding follows MatterDraft while the historical `.pen` labels retain CourtBundle.

## Verification

- Backend: `.venv312/bin/python -m pytest -q` — **328 passed**, six dependency warnings. New migration test covers persisted references and empty defaults for older creation callers.
- Frontend: final `npm run build && npm run test` — production/type build passed and **48 unit tests passed**, with the existing chunk-size advisory.
- Browser: the fresh-store `matter-creation-flow.spec.ts` passed. It checks restore picker activation, required-name validation, creation, persisted metadata, details and import navigation, saves the three settled 1440 × 900 screenshots, and checks no page-level horizontal overflow and a visible import action at 1024 × 768. The affected import cancellation, main browser export journey and protected-PDF import tests passed in isolated follow-up runs.
- Not checked: native folder selection (unsupported by the current storage contract), packaged Tauri runtime, Windows/macOS Intel, and a pixel-diff raster of `.pen` (no direct renderer available). No court approval or verified OCR/signature is implied.

## Short-window follow-up — 30 October 2026

The initial dialog height cap omitted its backdrop top padding, leaving the footer below a short window. The backdrop and dialog now share one viewport-derived top offset. Browser captures at **1400 × 430** and **560 × 430** show the initial header and scrolled footer; both stay within the viewport, and wheel scrolling reaches Create matter. The original 1440 × 900 frame geometry still passes. The final frontend build, 48 unit tests and both focused browser cases pass. An isolated native macOS debug app at **600 × 430** also showed the internal scrollbar, scrolled to visible Cancel/Create matter actions, and kept those actions reachable with Preparation settings expanded.

The later owner screenshot came from an installed app dated 29 October, before this source fix. The footer's inherited negative top margin also caused **Preparation settings** to touch its divider. The current layout uses 12 px clearance, checked in the short-window browser tests and visible in the 600 × 430 native dialog. A matching arm64 sidecar and unsigned Apple Silicon app were rebuilt on 30 October; the `/Applications` app was not replaced, and no real matter store was opened for this verification.

The next owner screenshot exposed a separate Matters header clipping issue. Browser geometry showed the page scrollport at y≈157 while the heading and New matter button began at y≈145, so the first 12 px of both controls were hidden. Removing the heading's negative top margin places both inside the scrollport. The focused 01B/02A/02B and short-window browser tests pass, the final frontend build and 48 unit tests pass, and a rebuilt native 600 × 430 QA app visibly shows the complete title and button. The updated release app has been rebuilt; the installed `/Applications` copy remains older.
