# B03B — New volume planning and size-exception workflows

29 October 2026. **[Certain] B03B implementation and required available-target acceptance checks are complete. The new 13A and BU06 workspaces are implemented from the supplied JSON.** B03C is the next bounded task. No court approval or legally verified OCR/signatures is implied.

## Design source and replacement

Parsed `CourtBundle_JSON_For_Codex/implementation/phases/B03_bundle_processing_size/processing_size.json` directly, including reusable components and descendant overrides for **screen-13A**, **screen-BU06** and existing **screen-BU04**. Used shared `design-tokens.json` / `resolved-tokens.json`. BU06 was compared with the supplied `implementation/previews/BU06.png`; BU04 remains the existing output-options integration. **No 13A PNG is supplied: its reference is the resolved JSON, not an invented screenshot.** The requested historical acceptance path is now `docs/ACCEPTANCE_TESTS.md`.

`VolumePlanning.vue` and `volume-planning.css` replace the previous build-tool volume form. The Index & bookmarks volume tab also routes to this new workspace; its old inline volume form is removed. Existing services are reused, not the old composition. All filenames, revisions, page counts, sizes and limits come from the active synthetic/runtime matter.

## Requirement-to-evidence checklist

| Requirement | Implementation and evidence |
|---|---|
| New 13A layout | Heading/actions, preparation tabs, 300px policy panel, results panel, per-volume progress/counts, estimated size, printed references and combined offsets. Geometry assertions and 16 theme/viewport/scale captures. |
| New BU06 layout | Full-width warning, 396px affected artifact/source panel, 796px three-path recovery panel. Actual source selection/revision and measured failed artifact evidence. Geometry assertions, direct PNG comparison and 16 captures. |
| Separate policies | Physical-page cap includes generated overhead. Optional exact-byte cap applies to individual volume PDFs only; existing profile file checks remain independent. Build summary displays the saved volume policy. ZIP/combined PDF are not constrained by this new cap. |
| Real recovery paths | BU04 optimization choices; explicit positive-byte allowance or no optional cap; revision-bound internal source-page boundaries with explicit authorization. Preview first, then save or apply/rebuild. No automatic split or compression. |
| Keep-together/front matter | Unsplit documents and optional groups remain intact. Each explicit boundary starts a new volume with its own index/optional cover. Group and front-matter settings are in a disclosure; numbering restart remains editable. |
| Accurate page map | `engine.py` maps `(document ID, original source index)` to destinations. PDF content, labels, generated index links, manual bookmarks, reviewed links, printable marks, Part extracts, CSV and comparisons use that map. Six-page/three-volume test reconciles every source and generated position. |
| Settings/history | Existing atomic `/bundle-settings` save and expected snapshot; defaults preserve prior digest compatibility. Split exceptions are matter-specific and excluded from reusable presets. Portable restore remaps document IDs and rejects invalid references. Store restart and native restart verified. |
| Draft/error/stale states | Successful current preview required for applying changes. Save failures retain drafts; stale previews are ignored and conflicts rejected. Reload and Save/Discard/Keep editing are available. Read-only preview can be left with Escape; focus returns to the rebuilt caller button. |
| Navigation/context | Page map, bookmarks, preflight, presets and BU04 connected. Index-origin volume planning returns to the index; build-origin planning returns to build. Matter, membership and working-page selection remain unchanged. Pending tool navigation waits for the destination component to mount. |
| Jobs and preservation | Rebuild uses snapshot-bound jobs and measured validation. Failure/cancellation leaves prior successful artifacts untouched. Browser failure/retry/conflict tests; backend cancellation; native measured failure/recovery. Original/prior hashes checked. |
| Review/export | Existing generated reader and native Save used on actual two-volume output. Second-volume bookmark, PDF index links, CSV ranges and bytes reconciled. Save cancel/retry/success and app/sidecar restart verified. |

## Persistence and deterministic generation

New output-marks fields are `volume_max_bytes` (nullable positive integer) and `volume_splits` (document ID → revision and ascending unique internal `after_pages`). No SQLite schema migration or cloud dependency. Boundaries are one-based “split after source page” inputs; persisted map/CSV `source_start` is zero-based inclusive and `source_end` is exclusive (also the displayed final one-based source page). A changed source revision requires review again.

`split_units` creates output segments, never new imported files. Explicit boundaries override keep-together only for the reviewed document. The ordinary page planner still accounts for cover/index/separators. Segments preserve original document identity and source offsets, avoiding duplicate-ID overwrites in bookmarks, links, annotations and comparison snapshots. Combined outputs retain cross-volume links; individual PDFs omit destinations outside that output under the existing policy.

Planning uses source-file bytes proportional to segment pages plus 2,048 bytes per generated page. **This is a rough estimate, not a measured output or target-size promise**: shared images, embedded fonts and processing can make output substantially different. Actual generated bytes are shown separately for matching saved inputs. After assembly and explicitly selected processing, every volume is measured. An over-limit build records the artifact name, actual bytes, selected cap, affected source IDs and input snapshot in the failed job; staging is not published. A subsequent accepted policy/split/processing choice requires a new build and validation. No content removal, flattening or rasterization was introduced for size recovery.

## Visual evidence and adaptations

Local synthetic evidence is retained in git-ignored [`ui-evidence/b03b/`](ui-evidence/b03b/).

| Reference / check | Evidence |
|---|---|
| Resolved source, including reusable overrides and source SHA-256 | [resolved-design.json](ui-evidence/b03b/resolved-design.json) |
| BU06 supplied PNG versus runtime | [side-by-side comparison](ui-evidence/b03b/BU06-reference-runtime.png) |
| 13A at reference viewport | [1440×900 light/100%](ui-evidence/b03b/13A-1440-light-100.png) |
| Desktop light/dark, 90/100/110/125% | [13A matrix](ui-evidence/b03b/13A-1440-matrix.png), [BU06 matrix](ui-evidence/b03b/BU06-1440-matrix.png) |
| Compact 1024×768 matrix | [13A matrix](ui-evidence/b03b/13A-1024-matrix.png), [BU06 matrix](ui-evidence/b03b/BU06-1024-matrix.png) |
| Compact keyboard primary-action access | [13A](ui-evidence/b03b/13A-compact-keyboard-actions.png), [BU06](ui-evidence/b03b/BU06-compact-keyboard-actions.png) |
| Native recovery, generated review and Save | [failed size](ui-evidence/b03b/native-size-exception.png), [plan](ui-evidence/b03b/native-volume-plan.png), [split destination](ui-evidence/b03b/native-split-review.png), [Save success](ui-evidence/b03b/native-save-success.png), [restart](ui-evidence/b03b/native-restart.png) |

At 1440×900/100%, 13A panel origins are **208/266** and **524/266**, widths **300/892**. BU06 warning starts **208/214**, affected/recovery panels **208/322** and **620/322**, widths **396/796**, with a 16px gap. Browser tests assert these bounds. Inter, 28px headings, semantic surfaces, 36px fields, rounded cards, 102px recovery rows and 112px volume-row rhythm follow the source. Both full-size views and all four contact sheets were visually inspected. The dark warning's pale surface retains dark semantic ink. No horizontal document overflow; keyboard traversal scrolls primary actions into view.

Necessary adaptations: real data replaces illustrative names/25MB/40MB values; the size allowance is entered explicitly in bytes; actual source sizes and failed output measurements are labelled distinctly. A source picker and revision details support real recovery. Disabled checked keep-together/front-matter indicators express fixed defaults; the extra disclosure exposes supported group/cover choices and can extend the policy panel below the illustrative height. A review/save/rebuild footer and job state extend the scrolling workspace. Native select arrows/focus indicators remain accessible. Narrow layouts wrap actions and stack panels below 760px content width. Active rail selection follows the containing Build workspace, with explicit return to an index caller. Mac screenshots include title chrome/Retina scaling. These adaptations retain the new screen hierarchy and panel placement.

## Changed paths

- `backend/app/engine.py`: source-offset planner/map/rendering, estimates, comparison snapshots, measured volume gate and CSV ranges.
- `schemas.py`, `storage.py`, `main.py`, `jobs.py`, `validation.py`, `portable.py`: validated policy, atomic settings/defaults, planning endpoint, failed-job evidence, measured validation and restore remapping.
- New `frontend/src/components/VolumePlanning.vue`, `frontend/src/volume-planning.css`; `App.vue`, `BuildWorkspace.vue`, `BundleStructure.vue`, `BundlePreview.vue`, `PdfViewer.vue`, `main.ts`, `lib/types.ts`, `lib/bundleSettings.ts`: new workspace, routes, focus, caller context, defaults and shared page-map integration.
- New `backend/tests/test_volume_recovery.py` and `frontend/e2e/volume-planning.spec.ts`; existing browser journeys now interact with explicit previews/saves and the new workspace. Impossible-cap failure fixtures use the existing API because the reviewed UI correctly rejects an impossible plan. Preservation/failure assertions remain intact.
- README, root handoff, compatibility pointer, implementation tasks and acceptance log. Existing branding/tagline/icon edits were preserved. No reset or commit.

## Verification executed

- `cd backend && .venv312/bin/python -m pytest -q`: **317 passed**, 6 dependency deprecation warnings, **58.21s**. New B03B module: **9 passed**, including parametrized invalid boundaries, multi-volume links/labels/marks, no silent splitting, measured failure/retry/cancellation, stale saves, restore and original/prior preservation.
- `cd frontend && npm run build && npm run test`: production/type build passed; **44 unit tests** passed. Existing large-chunk advisory remains.
- `cd frontend && MATTERDRAFT_E2E_VITE_PORT=1445 npm run test:e2e -- --output=/private/tmp/matterdraft-b03b-final`: **113 passed, 1 skipped** (opt-in vault), **9.0m**. All three B03B cases, existing processing/build/review/export flows and overlay regressions passed. [Final log](ui-evidence/b03b/matterdraft-b03b-browser-final.log).
- `backend/.venv312/bin/python backend/scripts/build_sidecar.py`: final matching Apple Silicon sidecar built.
- `cd frontend && npm run tauri:build -- --bundles app`: final unsigned Apple Silicon release **70.10 MiB**, successful.
- `npm run tauri:build -- --debug --bundles app --config /private/tmp/matterdraft-b03b-native-config.json`: isolated native QA bundle **37.04 MiB**, successful. Debug external-backend mode connects to the packaged sidecar on 8786 with `/private/tmp/matterdraft-b03b-native-store`; production encrypted-vault startup is not exercised.
- `git diff --check`: passed after implementation and documentation updates.

Earlier browser runs are diagnostic, not acceptance: the initial full run reported 104 passed / 9 failed / 1 skipped. Legacy-dialog selectors were updated for explicit preview/save; subsequent checks caught and corrected Escape/focus return, wrong page-map routing and a tool launch before its destination mounted. A synthetic matrix route check needed its manually applied zoom reset before navigation (the app owns the live scale setting). The production-worker asset assertion passed on rerun and still requires the emitted asset. A later complete run passed 112 tests with one annotation-drag failure and one opt-in vault skip. That coordinate test now waits for stable geometry and a finished PDF raster after tool layout changes; all persistence assertions remain. Five repetitions of each overlay case passed (10 total, 39.5s). The final full run passed with stable source files; no test retries or removed behavioral assertions.

## Native output reconciliation

Synthetic matter `2bfffaf1ff744d6589ebe5120d997956`. A native **1-byte** cap produced a real failed **40,827-byte** volume. In BU06, BU04 lossless optimization was explicitly selected, the volume allowance changed to **100,000 bytes**, and a split after source page **1** was reviewed/authorized, previewed and applied. Build `6f9836009c4b471b8e4c79e91d670e36` contains **2 volumes / 5 physical pages** (3 source + 2 indexes).

Actual bytes: combined **42,263**, volume 1 **39,701**, volume 2 **40,265**, CSV **268**, ZIP **109,894**, validation report **8,789**. The ZIP exceeds the optional volume cap and remains valid because it is a different policy scope. The second-volume bookmark opens source page 2 at combined page 4. Labels are `1,2,1,2,3`; index links go to physical combined pages 2 and 4; CSV source starts and combined offsets agree. Eight original/prior files remain hash-identical.

Native Save was cancelled, then retried successfully. The chosen filename acquired a native-dialog `private:` prefix; the actual QA save is `/private/tmp/private:matterdraft-b03b-native-saved.pdf`, retained as [native-saved-combined.pdf](ui-evidence/b03b/native-saved-combined.pdf). SHA-256 **`40f796cd7e10ce94bc94ec0dc18b6d69550900694c2ce7d1339e1e63aada743f`** matches the artifact exactly. [Reconciliation JSON](ui-evidence/b03b/native-reconciliation.json) records files, hashes, CSV and PDF navigation; [second-volume generated index](ui-evidence/b03b/native-second-index.png) is retained. Restarting both app and sidecar retained the reviewed settings and completed build. The final rebuilt app also verified index → new volume planning → index return ([final plan](ui-evidence/b03b/native-final-plan.png)). Its generated index/source text and pixels were captured in [final review](ui-evidence/b03b/native-final-review.png). During that final automation the raster initially stayed blank after outline/page references loaded, then painted after further pointer interaction. Cause is not established. A subsequent fresh single-window native session, with the browser suite stopped, rendered the index and source page immediately through the same accessibility actions. The second-volume bookmark again reached source page 2 / combined page 4; [isolated final review](ui-evidence/b03b/native-isolated-final-review.png) retains that successful check. The earlier observation is retained and is not a rendering-latency qualification.

## Limits and next task

Windows, Intel Mac, clean-device/minimum-macOS installation, production vault startup and native Office-app rendering/printing remain unverified. Estimates and optimization do not guarantee a byte target. A reviewed numeric boundary is a user choice, not automated semantic/legal document division. `delhi-hc-illustrative` remains unapproved. Stop after B03B; **B03C** is next.
