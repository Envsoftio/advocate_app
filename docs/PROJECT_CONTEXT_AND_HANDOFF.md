# CourtBundle project context and handoff

The maintained document is [CourtBundle_Project_Context_and_Handoff.md](../CourtBundle_Project_Context_and_Handoff.md) at the repository root. Read that document for project context, architecture and requirements before changing architecture.

At the product owner's request on 15 October 2026, the full handoff was moved to that location and the retained improvements were merged into its related workflow, data-model, bundling, processing, QA and roadmap sections.

- [Single matter workspace](../CourtBundle_Project_Context_and_Handoff.md#41-single-matter-workspace)
- [Document table and chronology](../CourtBundle_Project_Context_and_Handoff.md#61-data-model)
- [Presets and rebuild review](../CourtBundle_Project_Context_and_Handoff.md#62-deterministic-page-map-strategy-critical)
- [Reviewed import cleanup](../CourtBundle_Project_Context_and_Handoff.md#631-reviewed-import-cleanup-queue-imp-01)
- [Approved priorities and roadmap](../CourtBundle_Project_Context_and_Handoff.md#10-recommended-backlog-after-developer-machine-brings-up-the-ui)
- [Language and script support](../CourtBundle_Project_Context_and_Handoff.md#37-language-and-script-support-plan)

Use the [implementation backlog](IMPLEMENTATION_TASKS.md) for task status and the [acceptance log](POC_ACCEPTANCE_TESTS.md) for executed checks. This file remains a compatibility pointer, not a second copy of the requirements.

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
