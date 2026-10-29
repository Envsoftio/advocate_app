# MatterDraft project context and handoff

The maintained document is [MatterDraft_Project_Context_and_Handoff.md](../MatterDraft_Project_Context_and_Handoff.md) at the repository root. Read that document for project context, architecture and requirements before changing architecture.

At the product owner's request on 15 October 2026, the full handoff was moved to that location and the retained improvements were merged into its related workflow, data-model, bundling, processing, QA and roadmap sections.

- [Single matter workspace](../MatterDraft_Project_Context_and_Handoff.md#41-single-matter-workspace)
- [Document table and chronology](../MatterDraft_Project_Context_and_Handoff.md#61-data-model)
- [Presets and rebuild review](../MatterDraft_Project_Context_and_Handoff.md#62-deterministic-page-map-strategy-critical)
- [Reviewed import cleanup](../MatterDraft_Project_Context_and_Handoff.md#631-reviewed-import-cleanup-queue-imp-01)
- [Approved priorities and roadmap](../MatterDraft_Project_Context_and_Handoff.md#10-recommended-backlog-after-developer-machine-brings-up-the-ui)
- [Language and script support](../MatterDraft_Project_Context_and_Handoff.md#37-language-and-script-support-plan)

Use the [implementation backlog](IMPLEMENTATION_TASKS.md) for task status and the [acceptance log](ACCEPTANCE_TESTS.md) for executed checks. This file remains a compatibility pointer, not a second copy of the requirements.

Current UI direction: [new design shell and reader correction](DESIGN_PHASE02D_UI_CORRECTION.md). Supplied screen designs define the layout for completed and future UI tasks; legacy layout is not a preservation requirement.

Completed Phase 01/02 JSON audit: [Documents and reader fidelity correction](DESIGN_PHASE02E_JSON_FIDELITY.md). Documents opens preparation; Open editor retains the same working PDF/page.

B01A bundle membership: [implementation and BU05 evidence](DESIGN_PHASEB01A_IMPLEMENTATION.md). Uploads now stage as reference-only; existing matters preserve their included documents. SQLite schema 19 / portable package 15. B01B follows in the record below.

B01B build/review/export: [new JSON design implementation and evidence](DESIGN_PHASEB01B_IMPLEMENTATION.md). Actual progress/recovery, persisted-output retrieval, generated reader, native/browser export and effective-input freshness are connected. No schema/page-map rule change. B01C follows in the record below.

B01C artifact validation/review: [new BU07, 14C and ED34 implementation and evidence](DESIGN_PHASEB01C_IMPLEMENTATION.md). Named measured output checks, immutable history, exact finding notes/navigation and separate build-specific human review are connected to backend export/release gates. Viewer labels derive from the existing deterministic map. No schema migration. B02A follows in the record below.

B02A preset/settings and cover/index: [new BU01/BU02 implementation and evidence](DESIGN_PHASEB02A_IMPLEMENTATION.md). Replaces the affected legacy workspace, preserves explicit overrides, commits settings atomically and renders actual generated front matter through the shared engine. Cover/density/date page-map rules, original/prior hashes, preview/export agreement and available Mac Save are verified. No schema migration. **B02B is now implemented; see the current record below.**


### B02B — implementation record, 28 October 2026

[Certain] New BU03/11C/12A layouts and numbering/navigation are implemented and verified. Arabic/Roman labels, volume/continuous/Part resets, separate front matter, five positions, PDF labels/index/backlinks and optional Bates use one engine map. Real current-revision placement/destination previews, explicit bookmark hierarchy/resolution, guarded atomic settings and stale-response rejection preserve prior outputs. See [B02B requirement checklist, visual matrix, native reconciliation and limits](DESIGN_PHASEB02B_IMPLEMENTATION.md). Final checks: backend **285**, frontend unit **44**, browser **101** passed with one opt-in vault skip; Apple Silicon sidecar/release built; isolated native packaged-sidecar preview, navigation and Save passed. Originals/prior files hash-checked unchanged. No schema migration. Windows/Intel Mac/clean install/packaged vault/native Office-print remain unverified. B02C was next at this checkpoint; its completed record follows.


### B02C — current implementation record, 28 October 2026

[Certain] New 06A/06D layouts and source-backed metadata review are implemented and verified. Actual filename/selected-page evidence, independent editable fields, explicit acceptance, conservative dates, opt-in group sorting, atomic history and stale-save protection are connected. Save/Discard/Keep editing and caller/selection context remain intact. See [B02C requirement checklist, visual matrix, native reconciliation and limits](DESIGN_PHASEB02C_IMPLEMENTATION.md). Final checks: backend **296**, frontend unit **44**, browser **107** passed with one opt-in vault skip; Apple Silicon sidecar/release built. Native field acceptance, restart persistence, generated PDF review/rebuild and CSV Save cancel/retry/success passed; originals/prior files remained hash-identical. No schema or page-map rule change. Windows/Intel Mac/clean install/packaged vault/native Office-print remain unverified. B03A follows in the record below.


### B03A — new output-options implementation, 29 October 2026

[Certain] The supplied BU04 design now defines Output options, with persistent original/lossless/balanced/smaller processing, scoped PDF metadata cleanup and selected DOCX/XLSX/PPTX drafts. Actual generated sizes, staged processing and post-processing verification preserve original/prior bytes and existing page maps. Unsupported duplex and fixed omission policies are explicit. Backend **306**, frontend unit **44**, production build and matching Apple Silicon sidecar/release pass. Sixteen viewport/theme/scale renders were inspected; native generation, review, Save cancellation/retry, exact bytes/hashes and restart persistence were verified. Final full browser suite **110 passed, 1 skipped** (opt-in vault). **B03A is complete for the available target.** See [B03A implementation, comparisons, measured evidence and platform limits](DESIGN_PHASEB03A_IMPLEMENTATION.md). No migration or new page-map rule. **B03B is next; not started.**


### Product name — 29 October 2026

The application is now **MatterDraft Studio**, shortened to **MatterDraft**. See [rename verification and compatibility decisions](ACCEPTANCE_TESTS.md#bw-matterdraft-studio-product-rename--29-october-2026). Existing persistence, encrypted-backup and wire-format identifiers remain stable; use `MATTERDRAFT_*` for new developer configuration.


### Product terminology — 29 October 2026

MatterDraft Studio is an application in active development. Current titles, About text, PDF draft footers and package/crate names use product terminology. The acceptance log has moved to `docs/ACCEPTANCE_TESTS.md`; current source/docs and local phase-kit links are updated. See [verification and compatibility scope](ACCEPTANCE_TESTS.md#bx-matterdraft-studio-product-terminology--29-october-2026) and DEC-05 in the backlog. Existing release gates remain independent of the name.


### B03B — new volume-planning and size-recovery implementation, 29 October 2026

[Certain] The supplied **13A/BU06** JSON defines the replacement workspaces. All entry points use the new layout; BU04 processing, actual revision/source context, page/byte policies, reviewed internal boundaries, atomic snapshot saves/history, dirty guards and caller return are connected. The deterministic engine map reconciles split source ranges, generated front matter, labels, bookmarks, links, marks, Part extracts and CSV/combined offsets. Measured over-limit failures preserve previous output; originals are unchanged. Portable restore remaps reviewed split IDs.

Backend **317**, frontend unit **44**, production build and matching Apple Silicon sidecar/release pass. Thirty-two viewport/theme/scale renders and BU06 reference comparison were inspected. Native measured failure → reviewed policy/split → two-volume build → review/Save cancel/retry and hash reconciliation passed; final app/sidecar restart retained settings. An earlier native automation paint delay did not recur in a fresh single-window follow-up: index and source raster/text rendered immediately. The observation and unknown cause remain documented. Final full browser: **113 passed, 1 skipped** (opt-in vault), **9.0m**. **B03B is complete for the available target.** See [B03B requirement checklist, design evidence, commands and platform limits](DESIGN_PHASEB03B_IMPLEMENTATION.md). **B03C is next; do not extend this task into it.**


### B03C — new desktop, compact and dark Build settings, 29 October 2026

[Certain] The supplied **15A / 22C / BU10** JSON now defines the build-settings composition. Desktop and compact use the same persisted state; compact has its own hierarchy and action placement. Saved preset/overrides, outputs, structure/navigation, numbering/processing, engine counts and separate page/byte/profile limits replace generic summaries. Snapshot-matched preview/settings reads, error/retry and explicit return/focus preserve existing workflows. Core output checkmarks are truthful read-only summaries of the engine contract; Generated pages replaces the nonexistent illustrative preparation revision. No backend rule or migration was added.

Backend **317**, frontend unit **44**, production build and matching Apple Silicon sidecar/release pass. Final complete browser **116 passed, 1 skipped** (opt-in vault), **7.2m**. Sixteen viewport/theme/scale renders and direct 15A/BU10 comparisons were inspected; 22C uses resolved JSON because no PNG was supplied. Native compact/dark navigation, lossless/XLSX persistence, real build, visible generated index/source rendering, Save cancellation/retry and app/sidecar restart passed. Six artifact sizes/hashes and PDF labels/index/bookmarks/CSV reconcile; eight original/prior hashes remain unchanged. B03B's earlier raster-paint delay did not recur. **B03C is complete for the available target.** See [B03C requirement checklist, adaptations, commands and evidence](DESIGN_PHASEB03C_IMPLEMENTATION.md). Windows/Intel/clean-device/vault-startup/Office-rendering/printing gates remain open. The rebuilt release app is in the repository build output; the installed app was not replaced. **B04A supplemental plan and prior-build comparison is next; no B04 implementation started.**


## B04A supplemental planning and revision comparison — 29 October 2026

B04A implements the supplied BU08/19A/19B layouts, real immutable build baselines, revision/hash/membership comparison and independently saved supplemental plans. SQLite schema **20** adds opaque revision-bound source-page identities and versioned plans. The shared engine maps exact ordered selection into contiguous source runs and derives actual generated overhead. Changed revisions invalidate selected IDs without rebinding; explicit reselection is required. Caller Apply/Cancel, dirty navigation, reopening, stale responses and failures retain the correct supplemental context.

Backend **323**, frontend unit **44**, production build and matching Apple Silicon sidecar/release pass. Forty-eight appearance screenshots, six contact sheets and direct BU08 comparison were inspected. Native comparison, exact eight-page selection, separate numbering, app/sidecar restart, ten-physical-page map, primary 33-page rebuild, visible generated review and system Save cancel/retry pass. Saved bytes match the artifact; nine original/prior file hashes remain unchanged. Native startup/resource and raster-paint delays were observed and are documented, not declared fixed. Final complete browser **120 passed, 1 skipped** (opt-in vault), **7.6m**, no retries. **B04A is complete for the available Mac target; B04B is next.**

See [B04A requirement checklist and evidence](DESIGN_PHASEB04A_IMPLEMENTATION.md). Portable backups do not carry supplemental plans/page-ID registry/prior builds; plan edits are not primary Undo history. B04B generation/insertion and B04C audience variants remain separate. Windows/Intel/clean-device/vault-startup/Office-rendering/printing gates remain open. The release app is in repository build output; installed app untouched.

## B04B insertion policy and supported supplemental generation — 29 October 2026

[Certain] B04B adds explicit reviewed `separate_supplement` and conservatively reconciled `preserve_printed_labels` policies. Pure insertions are proven against baseline page evidence; stale IDs, replacements, deletions, reorderings, unsupported labels and collisions block acceptance. The shared engine map owns labels, index/CSV ranges, bookmarks, links, viewer labels, volume positions and combined offsets. Jobs and manifests bind a new artifact to the exact plan version, baseline, ordered stable IDs, revisions, settings and accepted policy without overwriting earlier files.

Backend **325** and frontend unit **44** pass; the Apple Silicon sidecar and 70.14 MiB release app rebuild successfully. The native synthetic journey rendered BU09, built/reviewed/checked/saved a ten-page artifact, reconciled the native Save hash, and retained the plan/artifact through a clean quit/relaunch. The complete browser result and its exact timeout/retry history are recorded in [B04B implementation and evidence](DESIGN_PHASEB04B_IMPLEMENTATION.md) and `ACCEPTANCE_TESTS.md`. Windows/Intel/clean-device/vault-startup/low-disk/native Office-print remain open. The installed application was not replaced. **B04C audience variants are next; B04B does not implement them.**
