# FLOW / 09 — Build package and recovery

17 November 2026. Scope: `FLOW / 09 / Build the package` and `FLOW / 09 / Build recovery` in `CourtBundle_Complete_Workflow (2).pen`: **15A settings, 15B progress, 17A complete, 16A failure**. These are states of the existing Build workspace. The matching files in `CourtBundle_JSON_For_Codex/screens/` supply the component overrides, geometry and semantic tokens; the 15A PNG is the supplied raster reference. The other three frames have JSON design references but no supplied PNG.

## Initial UI-only requirements and implementation audit

The canonical bundle requirements are in `MatterDraft_Project_Context_and_Handoff.md` §§6.2 and 6.3 and the FIL backlog, rather than the newer customer/admin/licensing PRDs. The build engine is already substantially implemented; a missing-looking screen is not evidence of missing PDF assembly.

| Requirement | Current implementation and this correction |
| --- | --- |
| Settings, membership, presets, numbering and output choices | Existing real settings/preview APIs and focused workspaces retained. 15A already follows the supplied hierarchy. Supplemental shortcuts now follow the primary content so they do not displace the settings/completion composition. |
| Deterministic PDFs, indexes, bookmarks, volumes and CSV | Existing `backend/app/engine.py` remains unchanged. No page-map, schema, processing, API or export rule changed. |
| Progress and cancellation | 15B now has a five-row progress list and submitted-build panel. Stages follow the engine's emitted boundaries (5/10/70/85/97/100), not elapsed time. Per-volume index/numbering/navigation work is grouped truthfully. Cancellation uses an unstyled Reka alert dialog, initially focusing Keep building, with Escape/return focus and no request until confirmed. Terminal backend status still decides whether cancellation succeeded. |
| Failure and recovery | 16A has the separate main error panel and recovery sidebar, full actual error, technical details, working-document/preflight/settings actions, retry and previous-artifact access. Failed, cancelled and interrupted states stay distinct. Failure survives reloading the matter. Retry guards now also respect dirty settings and empty membership. |
| Completion, review and export | 17A retains actual saved files, measured sizes, selected-build review and output-check gates. Adds PDF page counts from the selected build's measured validation data; combined count can fall back to its manifest. Non-PDF/unknown counts remain a dash. Removes the redundant repeated “Generated” column to preserve readable file sizes. |
| Preservation | Existing original/prior-artifact hashes, durable jobs, late-response guards, output review persistence and export/release policies remain intact. No source PDF is modified by these UI changes. |

## Initial gaps (superseded by the implementation below)

- **Exact old-snapshot retry is not implemented.** The 16A design's `retrySameSnapshot` metadata exceeds the backend contract. Failed/cancelled retry submits current saved inputs; interrupted retry creates one new job, also using current inputs. The UI now states this explicitly. Resuming midway is also unsupported, as already documented in handoff §6.3.
- **Frozen-input counts are not exposed on active Job responses.** The progress sidebar shows the real job identity and submission date instead of displaying live draft counts as frozen inputs or inventing a preparation revision. Detailed per-stage timestamps are also unavailable; the list uses the engine's actual coarse milestones.
- **Exact affected-page routing is not available for every build error.** Failures expose an error string, with structured recovery for measured size exceptions. The recovery panel therefore opens working documents/preflight instead of guessing a document/page from text. Existing size recovery remains available.
- **Reveal generated folder is not a supported export operation.** Local storage and the existing native Save/browser download workflow remain available. No fictional output path is shown.
- **Legal qualification remains open.** FIL-01/03/07/08/09/10 contain scoped template, court-rule, multilingual and target-device acceptance requirements. Existing technical output, reviewed splitting, numbering, supplements and audience variants do not supply advocate approval. Older narrative checkpoints saying B04C is next are historical: the current backlog records it implemented.

These are explicit capability/acceptance limits, not features silently added to a UI-only correction. The core end-to-end build flow exists; the entire broader PRD is not declared complete.

## Visual verification

Compared the actual settings render with the supplied 1440×900 15A PNG; inspected progress, failure and completion against their resolved JSON geometry/hierarchy. The newer application shell intentionally retains its 240 px sidebar and 120 px top chrome: content starts at x264 rather than the older design's x208. Tests now assert that current geometry (settings x264/y276/826×518 and summary x1106/y276/310×518).

The screen matrix captures 1440×900 and 1024×768, light/dark, 90/100/110/125% interface size. Recovery stacks at compact widths and the workspace scrolls to remaining content/actions. The final pass fixes truncated completion sizes, removes transient toasts before evidence capture, and retains all supplemental actions below the primary composition. This is a scoped design match with real data and documented capability adaptations, not a pixel-perfect claim.

Verification results and platform limits are recorded in `ACCEPTANCE_TESTS.md` under “FLOW / 09 build and recovery”.

Selected captures: [progress](ui-evidence/flow09/15B-progress-1440-light-100.png), [recovery](ui-evidence/flow09/16A-failure-1440-light-100.png), [completion](ui-evidence/flow09/17A-complete-1440-light-100.png), [compact dark completion](ui-evidence/flow09/17A-complete-1024-dark-125.png). Final results: **350 backend / 49 frontend unit tests; 13 targeted browser cases passed across the main and follow-up runs; matching Apple Silicon sidecar and app compiled**. Native runtime/Save dialogs and Windows remain unverified by this pass.

## Bundle functionality follow-up — 17 November 2026

The four technical gaps above are now implemented:

- Primary, supplemental and audience-variant submissions capture durable, integrity-checked source revisions, settings, membership and plans before queuing. **Retry saved inputs** creates one new attempt from that snapshot after failure/cancellation/interruption, including after restart. **Build current inputs** explicitly submits current preparation. Existing legacy jobs retain their previous retry behavior; no halfway resume is claimed.
- Job responses persist frozen document/page/volume counts and actual milestone timestamps. Invalid plans show unavailable counts rather than fabricated totals. Schema 28 adds nullable job context/events without changing existing API routes.
- Structured failures carry known source/revision/page targets. Navigation verifies the current revision and refuses a substitute page. Errors without a known target retain general recovery actions. Imported-original integrity is checked again before publication, including live original bytes; prior artifacts remain immutable.
- Native **Show saved file** reveals only a successfully exported file recorded by the native save service in this application session. It does not expose the encrypted internal output directory. Browser downloads retain browser behavior.

Snapshots use the existing encryption key when the vault is enabled; plaintext-to-vault migration also encrypts snapshot databases. Restart cleanup removes abandoned captures and snapshots no longer referenced by retained jobs. Snapshots consume additional local storage and are not portable-backup job history. Missing/corrupt inputs or changed technical profiles fail closed. Retry preserves submitted inputs, not byte-identical generation timestamps.

Validation: 356 backend tests, 49 frontend unit tests, seven native Rust tests, production frontend build, matching Apple Silicon sidecar and macOS app compilation passed. Browser and visual details are in the latest acceptance entry. Legal qualification, clean-device/Windows/Intel acceptance, and native OS Save/Finder runtime checks remain open; these are not satisfied by compilation. Universal exact-page attribution is not claimed for failures that provide no source-page evidence.
