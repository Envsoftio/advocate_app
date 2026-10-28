# B03A — New output options and measured generated files

29 October 2026. **[Certain] B03A implementation and required available-target acceptance checks are complete. The supplied BU04 layout and supported processing controls are implemented.** B03B is next and has not started. Technical checks do not establish court approval, authenticity or legally verified signatures/OCR.

## Design and scope

The layout source is `CourtBundle_JSON_For_Codex/implementation/phases/B03_bundle_processing_size/processing_size.json`, `screens/BU04.json`, the supplied BU04 PNG and shared semantic/resolved tokens. Reusable references and descendant overrides were resolved directly from JSON. 13A and BU06 were inspected for shared integration contracts only. The Output options action now opens a newly built BU04 dialog; the legacy technical settings form is no longer its primary layout.

| Choice | Implemented behavior and scope |
|---|---|
| Original quality (default) | No image reduction or PDF optimization at the processing stage. Other explicitly selected saved output policies still apply during assembly. |
| Lossless optimization | Existing local PDF optimizer, without image downsampling. |
| Balanced / Smaller images | Existing optimizer at 240 dpi/JPEG 85 or 150 dpi/JPEG 70. Explicitly labelled lossy; no size promise. Text, navigation, page geometry and labels are compared before and after. |
| Remove author/editing metadata | Clears and verifies PDF Info and XMP on this build's combined, volume and optional Part PDFs. Does not remove visible text, filenames, matter metadata, reports or Office properties. Default off. |
| Private comments / source attachments/actions | Disabled checked controls labelled “Always omitted”: existing assembly already excludes these source objects. Explicit reviewed links and PDF marks continue to follow their saved policy. |
| Print layout | Screen output with zero intentional blank pages. Duplex document starts are disabled with a reason; no new page-map rule is invented. Existing Print remains available under More output tools. |
| DOCX / XLSX / PPTX | Default off. Selected review drafts are generated from this build's processed combined PDF / CSV and included in the ZIP and measured file manifest. Text/layout limitations are explicit. Existing on-demand Office tools remain available. |
| Output size | Actual generated file bytes, with a readable size and exact-byte tooltip in the result table. Estimates are not presented as achieved sizes. |

Password-protected copies remain in More output tools. A source-content or signature-verification guarantee is not substituted for evidence. The preview describes the checks performed after generation.

## Persistence, services and preserved behavior

Six fields extend the existing output-marks settings object. Defaults are original quality, no metadata cleanup, screen layout and no Office drafts. Existing atomic `/bundle-settings` saves, expected snapshots, history, presets, portable storage and effective-input freshness carry them without a schema migration. Default-valued new keys are excluded from the digest for compatibility with existing B01/B02 builds; nondefault values invalidate derived output. Reader defaults and preset comparisons include the new keys, avoiding false unsaved-state flags after archive/restore.

The dialog separates saved state from the draft, retains failed/conflicting drafts, supports explicit reload and Save/Discard/Keep editing, Escape, Tab/Space and Cmd/Ctrl-S, and restores focus. Pending volume settings must be saved first. Disabled/busy state prevents applying during a job. Late requests are invalidated on unmount. The existing build, validation, review/release, download, native Save, membership, numbering and bookmark services remain authoritative.

Processing runs only on unpublished staging files after deterministic assembly. Reopened outputs must retain extracted text, page dimensions/rotation, PDF labels, outline destinations and link records. Metadata removal is checked independently. Office package XML/ZIP structure is validated by the existing output gate. Failure, cancellation or changed inputs prevent publication; prior artifacts and imported originals remain unchanged. No cloud service, new dependency, rasterization, flattening or new volume/index rule was added.

## Changed paths

- `backend/app/output_processing.py`: generated-file processing and verification adapter.
- `backend/app/engine.py`: staged processing, selected Office generation, measured report/manifest and ZIP integration.
- `backend/app/schemas.py`, `storage.py`, `validation.py`: settings/defaults/freshness and actual PDF/Office validation.
- `frontend/src/components/OutputOptions.vue`, `frontend/src/output-options.css`: new scoped BU04 layout and states.
- `BuildWorkspace.vue`, `App.vue`, `PdfViewer.vue`, `main.ts`, `lib/types.ts`, `lib/bundleSettings.ts`: action/service/dirty-state/defaults integration; technical marks and Part extracts remain accessible separately.
- `backend/tests/test_output_processing.py`, `frontend/e2e/output-options.spec.ts`: processing/preservation/recovery and browser/visual coverage. Existing journey selectors follow the relocated technical tools; existing behavior assertions remain.
- README, root handoff, compatibility pointer and acceptance log. Preexisting root-handoff edits were preserved; no reset or commit.

## Reference and rendered evidence

Local synthetic evidence is retained in git-ignored [`ui-evidence/b03a/`](ui-evidence/b03a/).

| Reference | Running implementation |
|---|---|
| [Supplied BU04](ui-evidence/b03a/reference-BU04.png) | [1440×900 light/100%](ui-evidence/b03a/BU04-1440-light-100.png) |
| Desktop theme/scale coverage | [1440 matrix](ui-evidence/b03a/BU04-1440-matrix.png) |
| Compact theme/scale coverage | [1024 matrix](ui-evidence/b03a/BU04-1024-matrix.png) |
| Native persisted settings | [Restarted native dialog](ui-evidence/b03a/native-restarted-options.png), [measured files](ui-evidence/b03a/native-measured-files.png) |
| Actual output reconciliation | [Measured bytes, hashes and map](ui-evidence/b03a/native-reconciliation.json), [saved PDF](ui-evidence/b03a/native-saved-combined.pdf), [generated index](ui-evidence/b03a/native-generated-index.png) |

The 1440×900/100% dialog is **960×692 at x240/y104**. Its panels start at y218, widths **440/456**, with a 16px gap. These reference bounds are asserted in the browser. Inter, semantic colors, rounded panels, 36px selectors and footer action placement follow BU04. All 16 viewport/theme/scale combinations were captured and both contact sheets plus full reference-size images visually inspected. Primary actions remain reachable; keyboard traversal reaches every enabled field without horizontal overflow.

Necessary adaptations: actual data replaces fixture counts; controls expose available backend behavior; fixed omissions are disabled/checked; the preview replaces unverified retained-content statements with actual post-build checks. An additional disclosure explains scope and unavailable options. At an effective dialog width below 850px, panels stack in the scrolling body while the footer stays accessible. Native controls retain platform focus indicators and select arrows. The browser reference capture preserves the surrounding build workspace's current scroll position (41px above its unscrolled heading); the modal itself uses centered responsive layout, not design-canvas offsets. The native capture includes macOS title chrome and Retina scaling. Existing technical tools sit below the main build action rather than inside BU04.

## Verification executed

- `cd backend && python -m pytest -q`: **306 passed**, 6 existing warnings, 57.15s. Ten new executed processing cases cover real image reduction across four quality modes, PDF/CSV/label/outline/link reconciliation, two volumes and a Part extract, all Office files and ZIP, exact bytes/hashes, stale settings, failure/cancellation/retry, unsupported values and legacy digest compatibility.
- `cd frontend && npm run build && npm run test`: production/type build passed; **44 unit tests passed**. Existing Vite large-chunk advisory remains.
- `cd frontend && COURTBUNDLE_E2E_VITE_PORT=1443 npm run test:e2e -- --output=/private/tmp/courtbundle-b03a-accepted`: **110 passed, 1 skipped** (opt-in vault), **6.8m**. Three new B03A cases cover persistence, actual builds/downloads, stale artifacts, all-file hashes, settings errors/retry/conflict reload, keyboard save/focus, dirty close and the 16-render matrix. The archive/restore regression discovered during the full run was fixed by aligning PDF-reader defaults; its original assertion is retained.
- `backend/.venv312/bin/python backend/scripts/build_sidecar.py`: matching Apple Silicon sidecar built successfully.
- `cd frontend && npm run tauri:build -- --bundles app`: final unsigned Apple Silicon release app built, **69.80 MiB**.
- `npm run tauri:build -- --debug --bundles app --config /private/tmp/courtbundle-b03a-native-config.json`: final separate QA app built, **36.73 MiB**. It uses the existing debug external-backend connection to the packaged sidecar and isolated synthetic storage; it does not exercise production vault startup.

Earlier browser attempts are not acceptance results: one used a nonstandard backend port while older fixtures hard-code 8768; a sandboxed attempt could not bind localhost. The corrected full run initially found one genuine archive/restore false-dirty regression, fixed without weakening its assertion. Earlier Office-package validation failed due a shadowed import, corrected before the 306-pass backend run.

## Native measured result and preservation

Synthetic matter `24851374289a40889e88b2de23c8cb57`, generated build `065c13fbaf8548ec9c67a15a10d287bd`: balanced images, metadata cleanup and all three Office drafts selected in the native UI. Combined PDF **40,648 bytes**, volume **40,642**, DOCX **1,388**, XLSX **1,976**, PPTX **30,999**, CSV **190**, ZIP **101,192**. Native result table exposes these measured byte counts.

Native generated review rendered actual synthetic content and followed the petition bookmark to physical/printed page 2. The four-page PDF comprises one generated index and three source pages; CSV source range and combined offsets are 2–4. Metadata is cleared. Native Save was cancelled, then retried successfully; the saved PDF hash is `9ce3c9d561c2916c0b83722da1583e1d8479290dc2cd7ec330e907d29597a9ae`, exactly matching the artifact. Eight original/prior files remained hash-identical. Both app and packaged sidecar were restarted; saved choices and generated artifacts were then verified in the final rebuilt native app.

No remaining failed scoped acceptance check. Final browser log: [`ui-evidence/b03a/courtbundle-b03a-browser-accepted.log`](ui-evidence/b03a/matterdraft-b03a-browser-accepted.log).

## Limits and next task

Windows, Intel Mac, clean-device/minimum-macOS installation, production vault startup and native Office-app opening/printing remain unverified. Office packages were generated, structurally checked and downloaded; that is not a claim of Office rendering fidelity. Native PDF Save cancel/retry/success was exercised. Compression does not guarantee a size target or visually verify scan/signature quality. No court approval is implied; `delhi-hc-illustrative` remains unapproved.

**Stop after B03A. B03B volume planning and size-exception workflows are next; they have not been started.**
