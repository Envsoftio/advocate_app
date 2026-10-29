# MatterDraft Studio — Project Context, Discussion Handoff, Product Implementation and Development Roadmap

**Prepared:** 12 October 2026
**Intended audience:** product owner, client, architects, legal-domain reviewer, Codex/development agents
**Code state (13 October 2026):** Python API and document engine tested; Vue browser build and runtime exercised; unsigned Apple Silicon Tauri app and DMG compiled and exercised locally. Windows, macOS Intel, clean-device installation and minimum macOS 12 remain unverified. See [acceptance evidence](docs/ACCEPTANCE_TESTS.md).
**Product positioning:** *Local-first Indian court filing preparation, collation and document automation workspace*
**Important:** This is NOT court-certified software and does NOT file directly with any court.

**Product identity (29 October 2026):** MatterDraft Studio, shortened to MatterDraft, is the application name. Current UI, packages and documentation use product terminology. Existing release-verification and legal-review requirements remain in force; dated baseline limitations below are historical, not a substitute for the current acceptance record.

**Maintained handoff:** This root file is the canonical project context and requirements document, as requested by the product owner on 15 October 2026. The former `docs/PROJECT_CONTEXT_AND_HANDOFF.md` location is a compatibility pointer. Implementation status and executed checks remain in the [backlog](docs/IMPLEMENTATION_TASKS.md) and [acceptance log](docs/ACCEPTANCE_TESTS.md). Historical implementation notes below are dated; use the latest backlog/security evidence for current status.

**Approved scope — 15 October 2026:** The six accepted workflow improvements are merged into the relevant sections below and prioritized in §10. They remain planned work, not completed features or legal approval. Existing client commitments and native/security, multilingual-output and advocate-review gates remain in force.

**Product direction — 15 October 2026 follow-up:** Keep the focus on India; US expansion is off the current plan. Pricing, tiers, billing periods and discounts are deferred for a later decision. Reuse useful competitor presentation and interaction patterns in the existing landing page and desktop workflow. “High-converting” is a hypothesis to measure, not an established property of any reviewed site. The Bundledocs findings below refine the five approved improvements; additional capability proposals are explicitly distinguished from committed implementation scope.

---

## 1. Why this document exists

The client initially shared a broad desktop application brief proposing Electron + React + TypeScript for the user interface, Python FastAPI as a separate backend process, PyMuPDF for PDF processing, and Tesseract OCR. After discussion, we established that the product is for **Indian advocates preparing court filing documents**, not a generic PDF editor. The technology selection was subsequently changed to **Tauri 2 + Vue 3 + TypeScript + Python FastAPI**. On 13 October 2026, the product owner confirmed client approval of Tauri/Vue and Tauri packaging in place of Electron/React and Electron Builder; see DEC-01 in `docs/IMPLEMENTATION_TASKS.md` for the approved platform matrix. Native platform delivery remains unverified.

The agreed development strategy is **prove one deterministic end-to-end legal document bundling journey before attempting a full Acrobat-style editor or comprehensive national court rules engine**.

This document preserves the original scope, the Indian filing research considerations, decisions, implemented architecture, verifiable progress, unresolved requirements, and recommended next stages so that another developer or coding agent can continue from this repository without losing context.

## 2. Original client request: complete conceptual scope

The client described the following desired product:

1. **Desktop foundation:** File Open/Save/Save As/autosave, user storage panel, PDF/Word/Excel/PPT export, print, links/sharing, zoom, search, undo/redo, theme and scaling controls.
2. **PDF engine:** Merge, split, extract, reorder, replace; rotate, crop and delete pages; insert before/after; PDF conversion from/to Office and image types; compression; navigation; OCR-based editable text; annotations, highlights, images and links.
3. **Advanced PDF:** Password protection, permanent redaction, e-signatures, watermarks, timestamps, pagination, scans/OCR, manual and automatic bookmarks, drag/drop bookmarks, and index-derived bookmarks.
4. **Legal document creation:** Matter-based, multi-tab workflow; category/subcategory forms; conditional dropdowns; template-based document generation; structured ZIP output.
5. **Collation and structuring engine (the distinctive complexity):** Four-part workflows (Part 1–4), drag-and-drop document order, OCR, pagination, indexes, smart grouping e.g. 1a/1b/1c, splitting at an approximately 300-page constraint, preserving whole-document boundaries, per-volume indexes, final combined and bookmarked outputs.
6. **Automations:** Index page-number updates, bookmarks generated from index entries, category-driven rule/defect checking.
7. **Architecture:** Offline first, clean UI/backend boundary, secure IPC, progress/cancellation, modularity and maintainability.

**24 October 2026 desktop-foundation scope clarification:** The product owner selected modern DOCX/XLSX/PPTX exports for this increment, rather than legacy `.doc/.xls/.ppt` or every pairwise Office conversion. Share-link generation is deferred; no online destination or revocation service is authorized. The original broader PDF/Office conversion discussion below remains historical context and requires a separate scope decision before implementation. Editable Office review drafts must identify their PDF/CSV source build and describe extraction/layout losses; they are not filing PDFs.
8. **Testing/distribution:** pytest for Python, Vitest for TypeScript, Playwright E2E requested initially, native builds for Windows/macOS (Linux mentioned in build scope).

The client asked proposal respondents for prior PDF/Electron experience, actual desktop-app examples, an MVP timeline, monthly retainer and fixed-price estimate.

**Crucial gaps in the initial brief:** The precise Parts 1–4 meaning, legal case types, which courts, the source of the 300-page limit, index schema, which documents belong together, what “editable OCR text” means, valid digital signature workflows, what export conversions must round-trip, and court-specific completeness rules were NOT provided. Avoid filling these gaps with invented law or fictional business rules.

## 3. Domain understanding: Indian court filing preparation

### 3.1 One nationwide filing profile is NOT appropriate

India's Supreme Court, High Courts and District Courts have court-specific and sometimes case/stage-specific filing directions. A preparation application must determine at least:

`Court -> Bench/establishment -> jurisdiction/side -> case type -> procedural stage -> filing route -> rule/template versions`

E.g., a fresh Delhi High Court petition and a Supreme Court final-hearing compilation should not automatically get the same volumes, margins, index structure or annexure policy.

**Domain data belongs in versioned profiles, conditional document checklists, lawyer-approved templates and transparent technical rules**, with source citations, scope, effective dates and last review dates.

### 3.2 The Delhi court limit: 300 MB is NOT 300 pages

The Delhi High Court amendment dated 24 January 2026 sets an e-file **maximum size of 300 MB for the High Court and 20 MB for the District Court**, subject to the detailed published terms and later changes. The notification says it takes effect on Gazette publication and records publication on 27 January 2026.

Sources:
- Official court e-filing hub: https://www.delhihighcourt.nic.in/web/e-filing
- Official amendment landing page: https://delhihighcourt.nic.in/web/no-6rulesdhc-dated-24012026-notification-re-amendment-rule-34-e-filing-rules-high-court-delhi-2021

The **300-page** limit mentioned in the client brief must remain a *configurable client/chamber workflow parameter*, not an asserted universal statutory court rule. Page caps and byte caps are independent. Index pages themselves count toward page limits when such a limit is applied.

### 3.3 Sources of filing complexity

Review these when extending the system beyond the demo:
- Delhi High Court e-filing rules, practice directions, FAQs and common registry objections (official site above). Among technical aspects are searchable documents, bookmarks matching the master index and clean output without unwanted encryption/watermarks/annotations (where applicable).
- Supreme Court publication of forms: https://www.sci.gov.in/forms/
- Supreme Court final-hearing compilation directions from April 2024, which describe differentiated volumes and linked indexes for the proceedings in their scope: https://cdnbbsr.s3waas.gov.in/s3ec0490f1f4972d133619a60c30f3559e/uploads/2024/04/2024040352.pdf
- Supreme Court advocate filing checklist (Jan 2025; different document types and conditional requirements): https://cdnbbsr.s3waas.gov.in/s3ec0490f1f4972d133619a60c30f3559e/uploads/2025/01/2025010980.pdf
- Bombay High Court e-filing rules, 2022: https://cdnbbsr.s3waas.gov.in/s3d0921d442ee91b896ad95059d13df618/uploads/2024/10/202410071417834105.pdf
- Bharatiya Sakshya Adhiniyam 2023, section 63 and its Schedule when relevant to electronic evidence certification: https://indiacode.nic.in/

Those sources should be independently confirmed for *current* version and precise case scope before incorporating them into any profile. A source page can remain live even when a specific rule has been amended.

### 3.4 Distinguish mechanical checks from legal judgments

Mechanically testable: document count, page order, duplicate labels, PDF openability, presence of text layer, generated outline and hyperlink targets, size limits, missing input and bundle/volume constraints.

Reviewer judgment required: whether filing is legally complete, jurisdiction, limitation, appropriate relief, due execution/attestation, source authenticity, adequacy of translations, validity of a signature/evidence certificate, and registry acceptance.

Do not display “court compliant” because some technical rules passed. Instead display a report of passed/flagged technical checks, with explicit “not assessed” legal considerations and human-review steps.

### 3.5 Preserve sources and legal evidence carefully

An imported scanned order or signed instrument is not a Word document. OCR may add an invisible searchable text layer but can misread details; flattening, rasterizing, cropping, reflowing, editing or combining signed material can alter appearance or invalidate a digital signature. Originals and processing copies must remain separate, and exports should be compared visually and procedurally with court-prescribed requirements. MatterDraft preserves originals but is not a forensic evidence-management system.

### 3.6 Important future court-related knowledge

A production rule pack should eventually contain:
- exact court and case/stage applicability
- original pleading formatting (paper, margins, font, spacing)
- category-dependent document and filing checklists
- annexure/evidence numbering and group rules
- index format (columns, page-range conventions, signing)
- pagination convention (logical vs physical page number)
- PDF encoding, size, bookmark/OCR requirements
- signing, execution, attestation and translation workflows
- filing route, service/fee/acknowledgment metadata
- registry defect categories and refiling steps
- version, effective date, source URL, reviewed-by and confidence/approval status.

A rules update should NOT silently mutate already completed export artifacts.

### 3.7 Language and script support plan

Indian filing work can contain several languages and scripts in the same matter or on the same page. Treat these as separate capabilities: displaying imported pages, storing names and titles, rendering generated index/cover text, adding searchable OCR, searching text, editing visible text, and managing translations. Success in one does not establish success in the others.

**Implementation boundary recorded on 14 October 2026:** Imported PDF/image pages retain their visible content in working copies and bundles, and PDF outline titles retain Unicode. CSV is written as UTF-8 with a BOM. The printed index uses built-in Latin fonts and replaces unsupported characters with `?`; a `NON_LATIN_INDEX` warning is emitted. The packaged OCR model and UI are English only. Search reads existing PDF text, so a scanned page in another script will not become reliably searchable through the current OCR path. Visible text placement/replacement is limited to printable ASCII. No translation, transliteration, language detection or legal review of translations is implemented. Do not label a multilingual bundle ready for filing on the basis of a successful build.

**Proposed implementation order, subject to target-workflow review:**

1. Select the first court/bench, case type and actual languages/scripts with an advocate; English plus Hindi is a *candidate pilot*, not an approved or comprehensive Indian-language list. Obtain licensed synthetic fixtures including mixed-script names, long titles, native-text PDFs, scans, stamps, tables and pages with more than one script. Record expected text and rendered appearance.
2. Make matter/document metadata Unicode end to end. Store optional document/page language and script hints, including `unknown` and multiple languages. Preserve the entered spelling; never silently translate or transliterate party names, annexure labels or quoted source text. Keep language hints and OCR review state with the working-copy revision.
3. Fix generated output before promising a script: select licensed fonts and a shaping/rendering path, support wrapping and variable-height index rows, and verify embedded glyphs, extractable text, links, bookmarks, CSV and page ranges from the single deterministic page map. The index-page count must be recalculated from actual layout; add page-map regression cases. Until a selected script passes these checks, a bundle requiring that script should fail an explicit readiness check rather than present `?` as usable text.
4. Add offline OCR models for each selected script and allow a user to choose the language(s) per document or page, with mixed-language runs where tested. Record provider/model/version and selected languages. OCR must create a new working-copy revision, keep visible source pixels and imported originals intact, expose uncertain or missing text for review, and never certify handwriting, seals, names or signatures. Compare character/word error and missed regions on reviewed fixtures, not just job success.
5. Test Unicode search on native and OCR text, including combining marks, punctuation, whitespace and script-specific input methods. A later optional transliterated or translated search should be visibly labelled as an aid, never substituted for the original text. Translation is a separate, human-reviewed document workflow; required translation, certification and filing language depend on the exact court and matter scope and cannot be inferred by this app.

**Release evidence:** For each advertised language/script, run synthetic end-to-end import → preview → OCR where applicable → search → index/bookmarks → volume/combined PDF/CSV/ZIP on Windows x64 and both macOS architectures. A reviewer should compare rendered output with the source, check copied/extracted text and every page destination, and record known limits for handwriting, poor scans and mixed scripts. Keep unsupported languages importable as visual source documents, but report unavailable OCR/generated-text capabilities clearly.

## 4. Agreed product goal, technology and non-goals

**Success demonstration:** an advocate creates a matter, imports synthetic PDFs/scanned documents, edits page order and document metadata, adds OCR, arranges documents into parts/groups, configures a page limit, presses Build, and receives accurate per-volume indexes with hyperlinks and outlines plus combined PDF and ZIP/report. This is an engineering demonstration, not a valid Indian legal filing.

**Chosen stack** (after discussion):
- **Tauri 2** + Rust native shell (lightweight, supports secure commands and bundled sidecar), instead of Electron.
- **Vue 3 + TypeScript + Vite**, Pinia available for later expansion; currently component-local state.
- **PDF.js** to display existing PDF working copies within the UI.
- **FastAPI + Pydantic** sidecar exposed only on loopback and guarded by a per-run bearer token in packaged Tauri mode.
- **PyMuPDF** for normalization/page actions, collation, outlines/index links, OCR integration and PDF export.
- **Tesseract traineddata** for local English OCR. Tesseract is invoked via PyMuPDF integrated OCR rather than a cloud API.
- **SQLite** for matter/document metadata; local original/working/export filesystem storage.
- pytest/Vitest; Tauri-level E2E is future work. Playwright alone does not validate native Tauri sidecar behaviour.

**Why Tauri:** sensible Vue integration and generally lower native-shell overhead than bundling Chromium; Rust orchestration available. **Tradeoff:** platform WebViews differ (Windows WebView2/macOS WKWebView), so PDF.js rendering/downloads must be tested on both. Python/OCR dependencies can dominate installer size, irrespective of UI shell.

**Initial scope exclusions (13 October 2026; see subsequent implementation updates):** full visual OCR editing of PDF text, Office round-trip conversion, universal PDF editing, cropping/replacement of pages, physical scanner driver integration, cloud link sharing, qualified digital signatures, legal AI drafting, real court e-filing, actual template form libraries, nationwide rules, automated legal fee/limitation advice, encrypted-at-rest storage, PDF redaction and guaranteed acceptance.

### 4.1 Single matter workspace

**Status:** The alternate guided mode was removed on 16 October 2026. The application now exposes one matter workspace and one set of navigation controls.

The workspace uses the existing Documents, Index & bookmarks, Preflight checks and Build bundle views. They share the same matter metadata, working copies, settings, validation routes, jobs and deterministic page map. There is no separate workflow state or duplicate pagination calculation.

- **Documents:** import files, inspect working copies, run OCR, edit metadata and arrange document order.
- **Index & bookmarks:** review the proposed page map, index ranges, volume allocation and manual bookmark destinations.
- **Preflight checks:** distinguish blocking technical errors, warnings and legal matters the software has not assessed.
- **Build bundle:** save settings, generate an identified build, inspect generated PDFs and save the selected artifacts. A failed settings save prevents the build request.

**Open sample matter** creates a separate synthetic matter with a generated draft cover and never overwrites the current matter. Existing save/discard protection, progress/cancellation, immutable exports and native Save controls remain part of the single workspace.

**Matter navigation and editor refinement — 16 October 2026:** The sidebar's Matters destination now opens a searchable, sortable active-matter home. A matter card opens Overview with case metadata, the latest package and preparation shortcuts. Sidebar matter shortcuts retain direct Documents access. The PDF header names the working copy; grouped page/edit controls sit above the canvas and document actions stay in a footer. Focus on PDF temporarily hides the source list. Full screen expands the same editor to fill the app window, hiding matter navigation; Exit full screen or Escape restores the workspace without remounting the viewer. These are views over the existing matter state, not a second preparation mode, dual reader or new workflow engine. Comparative task-time/usability testing and packaged native verification remain outstanding.

**Folio-informed reader implementation — 22 October 2026:** Working and generated PDFs now share document lifetime, bounded page surfaces, selectable text, safe source links and occurrence search. The reader adds continuous/single-page modes, explicit fit/numeric zoom, revision-keyed reading positions persisted through the existing local store, Back navigation, source outlines and a private-comment list. Existing text editing cards remain; editing focuses one page. A paginated organizer supports explicit/range selection, staged keyboard/drag reorder and atomic revision-checked rotate/delete. Crop previews the actual rotated page with drag/numeric controls. Originals, backend revision history and the deterministic output page map remain authoritative. The completion follow-up adds a virtual full-screen thumbnail rail, actual insertion/replacement previews with source/target revision guards, consistent keyboard help, exact-revision Save copy, named matter-wide Undo/Redo descriptions and authenticated range loading for working/generated PDFs, including protected files. Generated review no longer has the earlier 25 MiB UI cap. Native Mac checks verify page/zoom restoration after Quit and byte-identical Save copy/reopen; synthetic 10/100/1,000-page and large-output measurements are recorded in acceptance section AX. This completes the authorized reader/page-management direction of the [Folio review](docs/FOLIO_UI_FUNCTIONALITY_REVIEW.md); it does not implement dual panes, interactive forms or signing. See the [acceptance log](docs/ACCEPTANCE_TESTS.md) for exact evidence and remaining supported-device qualification.

**Casedo-informed reading workspace — proposed refinement:** Make **Read & link** available from Documents and Index & bookmarks. Keep the matter navigator beside one reader, with an optional second reader for a petition/evidence or judgment/reference comparison. Each pane retains its own document, page, scroll and zoom; users explicitly choose which pane receives a search/bookmark/link destination. Show the active pane with text as well as colour, provide **Open beside** and keyboard alternatives, and retain a readable single-pane mode on smaller windows. Reopening a matter should restore saved reading positions against the recorded revisions or explain why a position cannot be restored. Two panes share the same matter and revision store; they do not create independent copies. Precise links and research notes are scoped in §6.2.3; this remains a proposal, not implemented dual-reader functionality.

**Acceptance:** A synthetic matter can be prepared and exported without choosing between competing modes. Page-map preview, generated index ranges, bookmarks and CSV destinations agree. Unsaved drafts receive the established save/discard handling; a failed settings save does not start a build; job progress, retry, generated-PDF review and Save remain available.

### 4.2 Existing landing page: presentation improvements, pricing deferred

**Reference implementation:** `/Users/vishnu/Downloads/caseready` is the owner's existing marketing project, separate from this desktop repository and separate from the competitor `caseready.io`. Its working brand is CaseReady; this review does not decide the final brand. Read its own instructions before implementation.

**Reviewed 15 October 2026:** Nuxt/Vue source in `web/app/components/LandingPage.vue`, feature/FAQ data in `web/app/data/site.js`, the project README and the rendered `preview/index.html`. The local static preview was inspected; the production Nuxt build, signup delivery and conversion performance were not tested. The preview is a generated artifact and may lag source changes; future edits belong in the source components, then regenerate the preview using that project's documented command.

**Finding [Certain]:** The current page already has a consistent cream/green visual identity, responsive layout code, an interactive concept preview, feature categories, a founding-circle application, FAQs and development-status disclosures. It presents a broader legal workspace: research, case checklists, Document Studio and client guidance. Its default hero preview is a case checklist; the document preview is a secondary tab with illustrative completion checks, not a working import-to-export demonstration.

**Recommendation [Likely]:** Preserve the existing visual identity and useful page components. Give Document Studio a focused entry page or clearly dominant pilot section, with a document-preparation hero, default document preview and one consistent pilot CTA. Keep the wider product vision available, but do not require a bundle buyer to understand research and consultation plans before seeing the bundle workflow.

Proposed presentation work, to implement in that marketing project later:

1. Lead with a concrete outcome, for example **“Prepare indexed, bookmarked case bundles on your computer.”** Describe supported inputs and the review/export result; avoid promising universal court compliance. Show current pilot availability and verified platform status beside the CTA.
2. Replace or supplement the concept checklist with an actual readable desktop screenshot and a short synthetic-matter video. Use **Watch the bundle workflow** as the secondary CTA; use **Apply for the Document Studio pilot** consistently while release gates remain open. Retain the existing waitlist/confirmation system rather than introducing another signup backend.
3. Present **Import → Arrange → Review → Export** using the same language as the application. Show the final linked index, a bookmark jump, volumes and a late-document rebuild. Offer a synthetic sample output without requiring a visitor to submit case papers.
4. Explain benefits with visible evidence: source files retained; working-copy edits; automatic page-map-derived navigation; reviewable changes. Identify which examples are actual application output and which are planned interaction concepts. A mockup must not masquerade as a working feature.
5. Add a brief, accurate local-processing explanation: local case processing, exported plaintext files at the chosen destination, and any separate online services. State vault/platform limitations from current acceptance evidence. Use customer logos, testimonials, security certifications and speed claims only with evidence and permission where applicable.
6. Audit “First release” labels against the approved backlog. Legal research, approved forms, permanent redaction and broad conversion must not appear as already delivered merely because they are on the marketing roadmap. Keep the broader original brief intact, but publish honest current/planned/later distinctions.
7. Retain a short pricing FAQ stating that commercial terms will be shared later. Do not add prices, tier entitlements, lifetime offers, trial durations or billing commitments in this revision. Treat country expansion as uncommitted research, not a promised US launch.
8. Test whether the more focused message improves qualified pilot applications. Define the denominator, variant and observation period; distinguish CTA clicks, video engagement, form completion and confirmed applications. Use minimal aggregate events consistent with consent choices; never collect case names, documents or form free text as analytics. No conversion increase is claimed before measurement.

**Casedo demo presentation lesson:** Use a short, captioned problem→interaction→finished-output story: scattered synthetic papers, a precise passage-to-evidence jump with both passages visible, then the generated index and saved PDF. Keep an optional longer walkthrough for details. Demonstrate linking only once it works; until then label any illustration as a planned concept. Reuse the existing pilot CTA, brand and truthful release status. Casedo's “no training” and speed claims are not evidence for our onboarding or conversion performance.

**Presentation acceptance:** On desktop and mobile, the page clearly identifies the pilot user, input/output, product status and next action. Video has captions/transcript and a fallback image; sample links work without sending real matter data; keyboard/reduced-motion behavior remains usable. Headline/CTA experiments are compared with the existing page, and demonstrated capabilities are checked against the release evidence. This is documented work, not a landing-page deployment or redesign completed in this turn.

**Folio implementation follow-up — 15 October 2026:** At the owner's direction, the existing marketing project at `/Users/vishnu/Downloads/caseready` was updated directly, with **Folio** as its working name. It now leads with Document Studio, retains the existing application/confirmation backend, distinguishes current/planned/later capabilities, and includes a synthetic-workspace screenshot, captioned video/transcript and downloadable actual bundle output. The Nuxt production build, Go race-enabled tests/vet and 13 production-browser check groups passed locally. A local mobile Lighthouse run measured performance 98, accessibility 100 and best practices 100; SEO scored 66 because preview indexing is deliberately disabled. Conversion improvement, live email delivery and public deployment are not verified. Dependency audit findings and operator/domain/privacy setup remain release work. Exact implementation and test evidence are maintained in that project's `docs/FOLIO_IMPLEMENTATION.md` and `docs/TESTING.md`. The broader brief is retained in `docs/ORIGINAL_MARKETING_BRIEF.md`. This landing-page work does not change the desktop architecture or establish native/court approval.

### 4.3 Bundledocs review: evidence and workflow alignment

**Reviewed 15 October 2026.** Sources are vendor descriptions and a historical vendor demo, not an independent reliability audit or evidence of conversion rates. No trial account was created, no case files were uploaded and no Bundledocs output was generated by this review.

- [Homepage](https://www.bundledocs.com/): outcome-focused explanation, visual steps, product features, integrations and customer evidence. Borrow the sequence of explanation and demonstration; our own proof must replace their claims and logos.
- [Five-step written workflow](https://www.bundledocs.com/blog/2019/7/9/how-to-collate-a-document-bundle): create, upload, arrange, generate, download. The single matter workspace covers the same mechanical preparation task with explicit index, bookmark and preflight review.
- [Templates guide](https://www.bundledocs.com/blog/2019/1/8/how-to-prepare-a-court-bundle-template-with-bundledocs): reusable structure and formatting. This motivates the structure-template proposal in §6.1.3, distinct from legal form generation.
- [Features](https://www.bundledocs.com/features): describes section inclusion, page ranges, dates, configurable output and alternative late-insert numbering. These motivate the candidates and safeguards below; they are not Indian filing requirements.
- [Annotation design explanation](https://www.bundledocs.com/blog/2018/5/10/how-do-i-add-annotations-to-my-digital-binder-using-bundledocs-video): document-attached notes and selectable export inclusion. See §6.2.3 for our proposed review-copy boundary.
- [Product overview page](https://www.bundledocs.com/electronic-document-bundling-software) embeds [Ultimate Electronic Document Bundling Software](https://www.youtube.com/watch?v=yVgr65hLIX4), a 4:51 video. Sampled visible workflow frames were inspected; no transcript was available through the transcript tool. This is a visual review, not a complete narration transcription. The older Features-page video `fC_h2k2s0Pc` was unavailable. Historical demo styling must not be represented as a verified current logged-in UI.

| Observed point in the overview video | Visible interaction | MatterDraft implication |
| --- | --- | --- |
| About 0:58 | Section/document table, dates and descriptions, inclusion controls | Strengthen UX-05; make hierarchy and intended contents visible |
| About 1:27 | Generate dialog with collapsible cover, custom index, watermark, annotation, style and numbering settings | FIL-09 should expose useful defaults and grouped advanced options |
| About 1:56 | Download dialog selecting a dated bundle receipt, with download and OCR actions | FIL-10 should clearly identify the build being reviewed/saved; do OCR before our final validation |
| About 2:04 | Finished PDF index with section headings, document titles, page ranges and a draft watermark | Make the combined indexed PDF a primary product outcome; improve FIL-03/09 presentation |
| About 2:09–2:14 | Index hyperlink demonstration followed by a document page and an automatic-bookmark callout | Verify usable navigation in the saved artifact and the in-app review viewer |
| About 2:24–2:34 | Thumbnail-based page reordering and split controls | Reuse existing page tools; improve preview, undo and navigation rather than build a duplicate editor |
| About 2:54–3:14 | Public/private comment tools, area-redaction selection and search across all uploaded documents for redaction | Extend PDF-03/10 with explicit scope, match review and finalisation; the video does not prove sanitisation |
| About 3:24–3:44 | Highlighted regions and an on-page public comment | Develop PDF-03 with explicit source/revision targets and visible note status |
| About 3:54–4:04 | Private comment and a reference dialog choosing a destination document and page | Extend PDF-04 with source-linked cross-document references and target previews |
| About 4:22–4:34 | Annotation inclusion options, then a generated page displaying notes, highlights and masked regions | Inspect the selected output policy in the actual generated PDF; never include private notes by default |

**Follow-up after the owner's request to inspect beyond 2:00:** The segment was re-examined at closer visual intervals on 15 October 2026. The earlier table missed the finished combined-PDF/index demonstration. This is a central output requirement, not merely a document-table design reference. The first part shows the generated PDF; subsequent page tools edit individual source documents, and the closing part returns to generated output. MatterDraft must distinguish these contexts visibly rather than imply that editing an immutable export updates its sources. See the concrete implementation comparison and acceptance additions in §6.2.2.

**Alignment [Certain at the described-workflow level]:** import, ordering, metadata, page tools, OCR, generated indexes/bookmarks, PDF output and regeneration overlap with MatterDraft. Our implemented building blocks are listed in §5; the six UX enhancements remain planned. Cloud collaboration, hosted sharing and DMS integrations described by Bundledocs are a separate operating model, not prerequisites for our local core. No cloud migration, US rule pack or feature-parity programme is authorised by this review.

### 4.4 Exhibit Flow review: batch preparation adapted to India

**Reviewed 15 October 2026. [Certain] Evidence:** Read the complete auto-generated English transcript of the [6:20 vendor demo](https://www.youtube.com/watch?v=vhY7r5becvw), and visually sampled the stamp-position preview (1:26), appendix planner (2:32) and numbered file slots (3:48). The description identifies the video as **Exhibit Flow Classic**, the original Windows application; it is not a review of the newer Pro interface. The [vendor website](https://www.exhibitflowapp.com/) corroborates the tool categories. No software was installed, documents uploaded or competitor outputs independently tested. Timing, reliability and local-processing statements remain vendor claims.

| Demo segment | Demonstrated/described capability | MatterDraft opportunity |
| --- | --- | --- |
| 0:17–1:04 | Batch cover sheets using filename labels and formatting options | Reviewed annexure-label register and optional separators |
| 1:09–1:40 | Batch stickers with per-file placement preview | Label stamps with visible exceptions and safe placement |
| 1:42–1:55 | Word exhibit list | Optional editable index draft, derived from confirmed metadata |
| 1:58–2:50 | Volume assignment, pagination, bookmarks and contents ranges | Explicit volume-boundary review over our existing planner |
| 2:54–3:24 | Bates settings and continuation from a previous production | Later separate identifier ledger; never confuse it with court pagination |
| 3:26–3:59 | Evidence dropped into numbered/lettered slots | Arrange by manually reviewed petition-reference order |
| 4:01–4:38 | Merge with bookmarks; batch password tools | Reuse our merger; keep storage protection separate from filing-output policy |
| 4:42–5:24 | Renumber filenames after insert/remove/swap | Preview label changes and flag references requiring review |

**[Likely] Recommended Indian preparation flow:** Keep **Import → Arrange → Review → Export**. Select the exact court/filing context first. Import originals and review OCR/cleanup; arrange documents alongside a petition preview; confirm labels and linked groups; preview optional separators/stamps, index and volume allocation; then inspect the actual generated PDF and measured output sizes. The advocate chooses the applicable terminology and layout. A label assigned by software must never imply judicial admission, authenticity or legal verification.

**Priority within this review:** First refine UX-05/FIL-02 label review; next FIL-09 batch formatting and placement; then FIL-10 label/page-change explanations. These fit the existing five approved priorities. Explicit volume breaks, individual labelled-PDF export, editable Word indexes and a persistent Bates ledger are **candidates for scoping**, not newly approved implementation tickets. Do not reproduce the competitor's separate-tool menu when the same actions can share one matter, metadata register and build.

**Avoid copying these defaults:** automatic filename-driven confirmation, silent relabelling after documents have been cited, automatic shrinking of evidence, US-specific exhibit wording, or password-protected filing outputs irrespective of profile. Preserve originals and prior builds. Reordering a bundle does not update references inside an external petition; expose the review work instead of claiming it was done. Price/plan selection remains deferred.

### 4.5 Casedo review: reading, paragraph references and bundling

**Evidence checked 15 October 2026. [Certain]:** The [homepage](https://www.casedo.com/) embeds the 1:18 Vimeo video [Casedo — Get on top of your papers](https://player.vimeo.com/video/826160435). Reviewed its complete transcript, with visual inspection of the opening, workspace, bookmark/link sequence and closing sections. This is a short promotional demonstration, not an exhaustive current-version walkthrough or an independently tested export. The displayed TOC includes a 2021 generation date; do not assume the illustrated interface is the latest release.

| Video segment | Observed content | Improvement for MatterDraft |
| --- | --- | --- |
| 0:00–0:29 | Document overload followed by a single organised workspace | Preserve one matter and reusable reading context throughout preparation |
| 0:30–0:35 | Side-by-side reading and navigator positions for each view | Optional second reader with explicit destination pane |
| About 0:40 | Bookmark placed at a paragraph and given a meaningful title | Extend our page bookmarks to precise passage/region anchors |
| About 0:45–0:50 | Link from a judgment passage to related text in the other reader | Two-way passage references, visible destination and return navigation |
| About 0:53–1:03 | Pagination/TOC generation | Reuse our engine; show the real output and its freshness clearly |
| 1:04–1:18 | Closing benefit statements and trial invitation | Concise demo ending with our existing pilot CTA; no copied pricing/trial promise |

**How the linking actually works:** The [annotation guide](https://www.casedo.com/casedo-document-annotation-101/) describes manually placing one link endpoint, placing its second endpoint elsewhere, then navigating in either direction. It also describes paragraph-position bookmarks and using a second viewer. The evidence supports positional cross-references; it does **not** establish automatic legal citation recognition, semantic paragraph matching or reliable reattachment after arbitrary source edits. A text-selection-based MatterDraft interaction below is our proposed improvement.

**Other useful patterns, confirmed in documentation rather than this entire short video:** [Navigation guidance](https://www.casedo.com/navigate-documents-in-casedo-using-search-bookmarks-and-links/) combines bundle search, bookmarks and links. [PDF export guidance](https://www.casedo.com/support/how-to-export-case-to-pdf/) separates the export Index from Desk Space holding material. Borrow explicit inclusion, while avoiding a hidden export rule: MatterDraft should show **Included in this bundle** and **Reference only** counts and an output preview.

**Export/search caveat:** The [annotation-export article](https://www.casedo.com/support/how-to-export-annotations-in-casedo/), updated February 2025, describes direct bookmark export and a TOC workaround for comments/highlights. The [annotation-search article](https://www.casedo.com/support/how-to-search-annotations-in-casedo/), updated March 2024, describes a similar workaround. These dated articles do not prove the latest app's complete capabilities. They do show why we must independently specify workspace links, note search and saved-PDF behavior. Do not adopt the vendor's broad statements about what all courts allow.

**[Likely] India-focused value:** Let an advocate connect petition paragraph 12 to the relevant annexure passage, a proposition to a judgment paragraph, or a translation passage to its source. These are manually reviewed relationships, not legal conclusions or translation certification. Keep reference-only research separate from intended filing contents. The five approved enhancement priorities remain unchanged; the reading/linking extensions below refine existing candidates and require implementation scoping.

## 5. Current implemented capabilities

| Area | State | Notes |
| --- | --- | --- |
| Backend local API | Implemented/tested | Token auth; loopback in launcher, controlled CORS. |
| Matter CRUD | Implemented/tested | Name, profile, parties, case type, advocate, case number, volume policy. |
| PDFs and images | Implemented/tested | PDF/JPG/PNG; 80 MB per-file import cap; preserve original. |
| Working copy page operations | Implemented/tested | Rotate current page, delete and reorder pages. |
| OCR | Implemented/tested on synthetic fixtures | English OCR on image-only/mixed pages; visible content retained. Existing GlyphLessFont OCR pages are skipped. |
| PDF viewer | Browser journey tested; native coverage incomplete | PDF.js canvas, navigation, zoom and page operations. |
| Document ordering | Vue + API code provided | HTML drag/drop and explicit up/down. |
| Document metadata | Implemented/tested | Part 1–4, doc kind, annexure label, keep-together group key. |
| Template sample | Implemented/tested | One synthetic draft cover page; NOT approved legal form. |
| Bundle policy | Implemented/tested | Max pages incl. indexes; optional group cohesion; no document breaks. |
| Index, hyperlinks and outline | Implemented/tested | Clickable index pages, per-volume/combined outlines. |
| Combined PDF + volumes | Implemented/tested | Pages correctly allocated and stamped. |
| Index CSV + ZIP + report | Implemented/tested | Stored and downloaded through authenticated endpoint. |
| Validation | Implemented/tested | Missing originals/working PDF, duplicate hashes/labels, blank text layer, sample source profile status. |
| Async progress and cancellation | Implemented/tested for basic cases | Single background worker; cancellation at document/page boundaries. |
| Vue/Tauri packaging | Vue and Apple Silicon Tauri release build passed locally | Unsigned arm64 app and DMG exist; clean-device and other target builds remain open under NAT-02. |

**Automated result (13 October 2026):** 15 backend tests and 6 Vitest tests passed before the current QA additions; Vue production build passed. Run the suites for current counts. The `samples/example_output/` artifact was generated from synthetic documents and contains a combined 10-page, 2-volume package, linked indexes and OCR text. The sample artifact is technical evidence, not an approved filing output.

## 6. Suggested architecture

```text
Vue / TypeScript in Tauri system WebView
   |
   +-- Tauri IPC: get_backend_connection() -> ephemeral base_url/token
   |     |
   |     +-- Tauri Rust: launch bundled PyInstaller Python sidecar
   |     +-- listen only on 127.0.0.1, chosen random port
   |     +-- kill child process on normal app exit
   |
   +-- authenticated REST requests (Authorization: Bearer ...)
         |
         v
       FastAPI
         |
         +-- SQLite matter + document metadata (storage.py)
         +-- Originals + working PDF copies
         +-- PDF page actions + English OCR (pdf_ops.py)
         +-- deterministic volume/index/outline engine (engine.py)
         +-- background job executor with polling/progress (jobs.py)
         +-- JSON illustrative court profiles (rules/profiles.json)
         +-- Export: combined.pdf / volume-NN.pdf / index.csv /
                     validation-report.json / filing-package.zip
```

The Python engine is deliberately independent of Tauri. The product owner approved Tauri/Vue for the first-phase targets under DEC-01.

### 6.1 Data model

SQLite tables: `matters` and `documents`.

Matter fields: id, name, court_profile, case_type, petitioner/respondent/advocate/case_number, max_pages_per_volume, keep_groups_intact, created/updated dates.

Document fields: id, matter_id, title, kind, part, label, label_status, label_source, group_key, group_role, document_date, date_status, date_source, date_note, source_file_mtime, position, page_count, ocr_status, original_name, source_sha256, source_relpath, working_relpath, certificate-signature inspection status/count/note, reviewer visible-signature status, combined signature-review status and timestamps. A group role is a technical `member`/`parent` designation with at most one parent per group; it does not establish a legal relationship. Labels distinguish unreviewed legacy/filename values from user-confirmed or reviewed-proposal values. Document dates remain exact/unknown/ambiguous/suggested/confirmed metadata; a source file timestamp is never treated as the document/event date. Signature metadata distinguishes structural PDF certificate indicators from a human review of visible/scanned marks; neither verifies a signature or signer.

Original source bytes are never edited by page actions or OCR. Working-copy edits now create numbered revisions, retaining prior files; a failed edit discards its candidate. Deleted document records do not erase on-disk originals. Stored source hashes are explicitly checked during validation and immediately before export publication. This checks byte consistency at those times; it does not prove authenticity. DAT-01 serializes app edits and build/OCR jobs per matter and rejects a second engine using the same data directory.

`~/.courtbundle-poc/matters/<uuid>/` for browser-development mode; native app uses Tauri's platform app-data directory.

#### 6.1.1 Editable document table and batch actions (UX-05)

**Status:** Engineering implemented 16 October 2026; native acceptance deferred. Enhancement priority 1 of 5. The policy-neutral proposal/review layer is complete under FIL-02, including explicit technical parent designation, per-preview group formatting and reviewer-selected compact-or-preserve insertion/removal behaviour. The model does not infer a legal relationship or approved court convention; named reusable court/chamber presets remain FIL-09.

**Need:** Review and classify many documents without opening a separate detail form for every item.

- Show editable title, annexure label, document date, type, part and group alongside filename and read-only page count. Keep access to page preview and the existing detail editor.
- Support explicit multi-selection and batch assignment of type, part and group. Make the affected selection/count visible; sorting and filtering must not silently change which documents receive an action.
- Offer upload/manual order, filename order and document-date order with a preview before committing changes. Keep groups and existing ordering constraints visible; reject invalid arrangements with an explanation.
- Distinguish user-entered/confirmed document dates, unreviewed extracted suggestions and filesystem timestamps. Preserve unknown or ambiguous dates; do not silently equate a file timestamp with the date of an event or document. Define and display unknown-date placement and stable tie handling before applying a sort.
- Persist edits through the existing revision/history and export-freshness mechanisms. Automatic annexure renumbering remains governed by FIL-02 and its reviewed policy.
- Present collapsible part/section rows with document counts and explicit move targets. Start by exposing existing Part/group concepts accurately; arbitrary nesting is a separate model/engine change, not merely a drag-and-drop styling change. Offer both drag and **Move to…**/keyboard controls. Keep editable display titles separate from original filenames, and show OCR/import/review status without treating it as legal verification.

**Exhibit Flow refinement — label review (UX-05/FIL-02):** The table provides a batch preview of document identity, original filename, proposed label, description and confirmation status. A chosen prefix/start, existing group keys or conservative filename parsing may suggest values; ambiguous names, duplicate labels and missing values stay visible for manual resolution. A group can designate one technical parent, even when it is not first in document order. For grouped proposals, the reviewer can choose lower/upper-case alphabetic, numeric or parenthesized suffixes, a separator, base-label treatment, and whether to compact the selected sequence or preserve existing labels for manual completion. These are matter-level technical choices, not court approval or proof of a legal parent/child relationship. Searchable petition text can be scanned for annexure/exhibit tokens with page/revision evidence, but matches are incomplete, unverified citation candidates and are never rewritten automatically. Labels stay independent of page numbers and original filenames. Existing-label changes require an explicit old-to-new acknowledgement; prior generated artifacts remain immutable and trigger a separate warning because they may have circulated. Applying a reviewed map is one atomic undoable edit and new manifests record the exact label snapshot. User-defined document/translation groups retain their order and cohesion; grouping does not certify a translation. Manual citation-order placement beside the petition and reusable named court/chamber presets remain future work under their separately scoped tickets.

**Acceptance:** Batch changes affect only the displayed selection, survive reload and can be undone/redone. Cancelled sort previews leave order unchanged; committed sorts are deterministic and respect the approved group policy. Fixtures cover equal, missing and ambiguous dates, filtered selections, duplicate labels and failed saves. A guessed date never becomes confirmed merely because a sort or build succeeds.

#### 6.1.2 Source-linked chronology, later increment (CHR-01)

**Status:** Approved scope, 15 October 2026; TODO. Enhancement priority 5 of 5.

**Need:** Arrange material events and inspect the document pages supporting each event.

- Add a matter chronology with event date, description, one or more source document/page references and explicit review status. Support unknown/approximate dates without inventing exact dates.
- Begin with manually entered, human-reviewed events. Keep chronology order separate from bundle order; editing a chronology must not silently reorder evidence or create a new filing document.
- Open each source reference in the viewer. Store source document identity, revision and page target; changes or archive operations that invalidate a target leave a visible unresolved reference requiring review, rather than silently pointing elsewhere.
- Persist chronology edits with Undo/Redo and portable matter backup/restore, remapping source identities on restore.
- AI date/event extraction is a possible future, separately specified extension. If introduced, outputs must remain suggestions with source references until reviewed. Predictive insights and automatic legal conclusions are not included.

**Acceptance:** Events survive restart and portable round trip, keep their review status, and navigate to the intended source pages. Fixtures cover multiple sources, unknown/approximate/equal dates, page deletion/reorder/replacement, archived sources and Undo/Redo. Invalid references stay visible and cannot be shown as reviewed current links without resolution. Chronology changes leave bundle ordering unchanged. The initial release of this feature requires no AI or network access.

#### 6.1.3 Structure templates and selective bundle contents — proposals

**Status:** Bundledocs-informed candidates for product scoping, not additional completed tickets or automatic first-release commitments. Extend UX-05, FIL-01/02/03/09 and DAT-01/02/04 if selected. Preserve the existing five-item priority order.

- **Reusable structure templates:** Create a matter from named sections and a versioned output preset. Store structure/settings only by default; never copy another client's documents, parties, notes or completed review statuses. Keep a structure template distinct from a court profile (rules/provenance), an output preset (appearance) and a generated legal form (GEN-03). Show the proposed structure before applying it. A reusable template update must not rewrite existing matters.
- **Include/exclude without archive:** Let users prepare a selected bundle from retained matter documents, with visible included/excluded counts and an explicit output selection. Search filters do not change inclusion. Excluding a section must clearly resolve child inclusion; excluded sources stay in the matter. Persist selection in build inputs/history/backups and make dependent outputs stale. This is separate from record archive/removal.
- **Reference-only reading material:** Casedo's Desk Space suggests a useful holding area for research and unsorted material. Prefer an explicit document inclusion state over making panel location the only signal. Show where a link targets reference-only material before export; never silently include that document to repair a link. Research tags and search filters must not change the filing order or inclusion set.
- **Selected page ranges, later:** Preview source-page selections without deleting pages from the working copy. Validate ranges and page identities against the chosen revision; reject unresolved selections after source edits. Use a defined policy for repeat selections, empty sections, bookmarks to excluded pages and document/group cohesion. The present engine treats whole documents as allocation units, so this requires a planned engine extension.
- **Missing-document checklist, later:** Show expected items as unresolved tasks alongside sections. Do not quietly turn a missing item into a blank page or claim the bundle is complete. Any intentional placeholder page is a separate, visible output choice participating in the page map.

**Acceptance before enabling any candidate:** A second synthetic matter created from a template contains no prior matter data. Inclusion/range changes survive restart and portable restore, have truthful undo support and appear in build comparisons. Preview, PDF index, outlines, volume totals and CSV all derive from the same selected content. Boundary tests cover exclusion of whole sections, mixed groups, invalidated source-page targets and empty selections; old artifacts remain unchanged.

### 6.2 Deterministic page-map strategy (critical)

For each selected volume, the engine:

1. Validates sources, grouping and sizes.
2. Computes required per-volume index page count (23 records per index page).
3. Places **whole document units** while ensuring document pages + index pages fit the configured cap.
4. Computes each entry's exact **volume-local** 1-based start/end pages.
5. Creates the index pages, copies source pages without working annotations, stamps consistent physical page numbers, and inserts internal links to the calculated start page.
6. Creates a PDF outline with part and document entries.
7. Appends completed volumes to combined PDF and recalculates global link destinations and nested outlines using each volume offset.
8. Exports CSV and report using the **same volume page map**.
9. Stages results and publishes them only when all steps complete.

**Important:** The engine does not split oversized documents. If a document or indivisible group cannot fit in a volume, it fails with an explanatory error. A repeated group key appearing non-contiguously produces an explicit error rather than silently merging unrelated document spans.

#### 6.2.1 Reusable output presets and stamp previews (FIL-09)

**Status:** Engineering implemented for the current technical layout, 18 October 2026; FIL-09 acceptance remains open. Immutable named technical versions capture volume settings and output marks; explicit matter application, Undo/Redo, export snapshots and portable restore are implemented. Separate Bates identifiers, annexure label stamps, technical separator sheets, per-document exceptions and actual-page preview are implemented on the shared page map. The existing linked index is fixed; covers are ordinary source documents. Reviewed court-specific cover/index variants need DEC-02/FIL-03 specifications, and supported native-device visual/export review remains pending. Enhancement priority 2 of 5.

**Need:** Reuse chamber preferences and inspect formatting before generating the complete bundle.

- Save and reuse versioned presets for numbering, cover/index layout and optional separators. Start from the current technical layout; additional filing layouts require reviewed specifications under DEC-02/FIL-03.
- Treat physical/logical page numbers, annexure labels and Bates identifiers as separate concepts. Support configurable Bates prefix, start number, padding and placement, with an explicit policy for covers, indexes, separators and volume boundaries. Do not substitute Bates identifiers for court pagination implicitly.
- Preview stamps on representative actual working-copy pages, including rotated and differing-size pages. Warn about detectable overlap with existing content and allow placement adjustment; visual review remains required.
- A matter records the selected preset version and overrides. Updating a reusable preset does not silently alter an existing matter or completed artifact; applying an update is explicit and marks affected outputs stale.
- All inserted pages and numbering policies must participate in the single deterministic page map. Preview, generated PDFs, indexes, bookmarks and CSV must agree.
- Group controls into clear categories (cover/sections, index, numbering and optional marks), with a live summary of overrides. Avoid a long mandatory setup form. Start with technical/chamber preferences; preview any new date column, title wrapping or separator because each may alter index length and volume boundaries.

**Exhibit Flow refinement — batch annexure formatting:** Offer distinct **no added mark**, **label stamp**, and **separator/cover sheet** choices, with a reviewed specification for whether combinations are supported. Show which pages receive marks; do not assume every page needs the annexure label. Start with a preset position, allow per-document exceptions and show the affected batch before applying. Use confirmed register values rather than parsing filenames again during export. Do not silently shrink/crop source content to make room: any fit-to-page transform needs a before/after preview, legibility review and PDF-11 signed-input handling. Stamps must not obscure signatures, seals or existing text. Added sheets affect pagination and must enter the page map before ranges are calculated.

**Acceptance:** A preset survives restart and can be applied to another synthetic matter with reproducible settings. Preview and exported placement agree on mixed-size/rotated fixtures. Index growth, cover/separator inclusion, group/volume boundaries and each new numbering rule have pytest cases. Original bytes remain unchanged. Existing exports remain immutable after preset edits. Any layout presented as court-reviewed carries its exact scope, sources, version and advocate review; unreviewed presets are visibly technical/chamber preferences.

#### 6.2.2 Rebuild explanation and export review (FIL-10)

**Status:** Approved scope, 15 October 2026; TODO. Enhancement priority 4 of 5.

**Need:** Understand what a late change affects and verify the regenerated package before saving it.

- Extend FIL-06's existing stale-export detection with an explanation of changed inputs/settings, added/removed/replaced/reordered documents, affected page ranges and unresolved bookmark targets.
- Compare a selected prior build with the current plan using stored build inputs and the deterministic page map. When an older manifest lacks sufficient detail, display that the comparison is unavailable rather than inventing a change history.
- Place **Review PDF** beside the technical checklist and export actions. Distinguish technical checks, unresolved review items and unassessed legal questions. Keep the current bounded in-app preview behavior explicit and provide Save/external review for larger files.
- Rebuild into a new immutable artifact set. Retain prior completed outputs and identify the version being reviewed/saved; a failed or cancelled rebuild must not replace the last successful package.
- Show build date/identity, current versus outdated status, included documents/pages, file sizes and technical-check result together. Use readable labels for **Review PDF**, **Save PDF**, **Save volumes/package** and **Save report**, displaying only artifacts actually generated. Never use a green “ready” badge to imply legal approval or successful native Save before it happens.
- Extend the change explanation with old/new annexure labels separately from old/new page ranges. Keep previously issued artifacts unchanged and list references requiring manual review. A successful rebuild must not imply that citations in a separately edited petition were repaired.

**Combined-PDF experience — clarified from the video after 2:00:** Treat the finished navigable bundle as the main deliverable, with volumes/CSV/report/ZIP available beside it. Reuse the current combined-PDF generator; this is not a proposal to build a second merger.

| Capability | Code-inspected baseline, 15 October 2026 | Required improvement / task |
| --- | --- | --- |
| Combined PDF, per-volume index rows/ranges, generated index hyperlinks and hierarchical outlines | Already produced by `backend/app/engine.py`; navigation validation checks serialized destinations against the page map | Preserve as core acceptance; demonstrate it clearly in the workspace and landing-page output samples |
| Finished-PDF review inside the app | `frontend/src/components/ExportPdfReview.vue` renders pages and resolves outline destinations; it explicitly tells users to test index hyperlinks in a saved viewer | Extend FIL-10 with clickable internal PDF links; retain external review fallback |
| Sectioned/styled index | Current fixed A4/23-row layout and existing part/volume outlines; not the demo's styled section-header table | Scope section-header rows, columns, wrapping, optional cover/separators and typography through FIL-03/09; include their page cost in the planner |
| Search/select text in generated output | The export review component currently renders a canvas without a text layer or find UI; source-document search is a separate existing capability | Add generated-artifact find and selectable text as a FIL-10/UX-02 refinement; distinguish no searchable text from no matches |
| Review navigation controls | Physical page input, Previous/Next and outline buttons exist; no navigation-history or explicit zoom controls in this component | Add collapsible outline, zoom/fit and Back/Forward after link/bookmark jumps; add a clearly labelled index return action |

- Inspect the **actual generated PDF**, not an HTML simulation of its index. Internal links should resolve PDF destinations and respect rotation/scaling; do not infer destinations from the displayed page-range text. Arbitrary PDF scripts/actions or external links must not execute automatically.
- Display source-document page, printed volume page and physical combined-PDF page distinctly when a mapping is available. Never assume they are equal. The competitor's sample visibly restarts index page ranges by section; do not adopt that scheme as our default or infer an Indian court requirement.
- Keep a **Generated output / read-only** context. A proposed **Open source page** action maps back to the exact document revision used by that build; if unavailable, explain the limitation. Working-copy edits make the output stale and require a new build. Never edit the completed artifact in place.
- Keep file-size and memory bounds explicit. The current in-app review cap remains 25 MiB until a separately tested larger-file strategy is implemented; Save/external PDF review stays available. A review UI improvement is not proof of native performance at arbitrary bundle size.

**Additional acceptance:** For synthetic multi-volume fixtures with mixed native/scanned pages, a reviewer opens `combined.pdf`, reads the real index, clicks an index entry, navigates its outline, returns to the prior view, finds known searchable text and saves the same build. Index links, outline targets and page-range labels agree with the shared map in the app and a saved desktop PDF viewer. Test multi-page indexes, long titles, mixed-size/rotated pages, missing text layers, stale builds and boundary destinations. If section headers/covers change index length, add planner/export pytest cases before claiming completion. Do not present this proposed viewer functionality as already implemented.

**Acceptance:** A late inserted document produces an accurate explanation of changed volume/combined ranges and bookmark status. Rebuilt PDF destinations, index rows and CSV agree. Tests also cover replacement, reorder, metadata/preset changes, Undo, insufficient legacy manifests and failed/cancelled rebuilds. The reviewed and saved artifact belongs to the same identified build. Native Save checks remain required under NAT-03; review does not mark the matter filed or court-approved.

#### 6.2.3 Late additions and review-copy policy — separate proposals

**Ordinary late additions:** FIL-10 already covers inserting a document and explaining the resulting renumbering. Make that reliable and understandable first. Do not describe it as preserving previously circulated page references.

**Stable-reference inserts, later candidate:** Bundledocs describes alternative late-insert numbering. A MatterDraft equivalent needs a separately approved numbering specification, scoped court/advocate review where applicable, and an immutable baseline build. Distinguish physical PDF page positions, displayed page labels and document identifiers. Define insertion before/after boundaries, repeated insertions, removals/replacements, index growth, volumes and whether a supplement is preferable. Never patch an old PDF silently. If implemented, previews, PDF page labels/stamps, indexes, links, bookmarks and CSV must share one deterministic map, with pytest boundary fixtures. This is not a new default or a nationwide filing rule.

**Review copy versus submission-preparation copy, later candidate:** Extend PDF-03/04 and FIL-10 with document/revision/page-anchored review notes and links, rather than attaching work only to an ephemeral combined PDF. Keep targets visibly unresolved when they cannot safely be mapped after an edit. A clearly labelled review copy may include explicitly selected notes/highlights; the submission-preparation copy excludes private review annotations by default. Store that choice in its build manifest and preview the exact resulting artifact. A note marked “reviewed” is not legal approval.

**Redaction boundary:** Redaction is not a note-visibility toggle. Pending redactions require explicit finalisation and verification under PDF-10; changing annotation inclusion must never restore removed sensitive text or expose an unredacted original. Current omission of PDF annotation objects and existing pending-redaction safeguards remain in force until a tested policy replaces them. Acceptance must inspect underlying text/images, attachments and metadata as applicable, not only visible black boxes.

**Search-assisted redaction — candidate within PDF-10:** The video around 3:14 shows searching across documents for a redaction operation. For MatterDraft, show the searched scope, each match in context, page/revision and explicit selection before marking anything. Keep area-based marking available for scans or missed OCR. Case/word matching and unknown OCR coverage must be clear; zero search hits never proves that sensitive information is absent. Test the final exported bytes and visual result after finalisation, including repeated names, false positives, image-only pages and changed sources. Build the verified single-region redaction path before adding batch search; this remains unimplemented scope, not a reliability claim about the competitor.

**Cross-document reference links — refinement of PDF-04:** Let the user select a source region and choose a target document plus page, preview the destination and give the link a readable label. Store both ends against document identities/revisions and resolve them through the page map on export. Reordering must not silently retarget the link; excluded, archived, replaced or deleted targets require visible resolution. Define export behaviour for targets in another volume (including standalone volumes); never retain a broken link by assuming the combined-PDF offset applies everywhere. The present engine omits working-copy link actions, so implementing the authoring UI alone does not complete this feature. Reuse the target-resolution concepts from FIL-04 and CHR-01, and verify saved-PDF navigation.

**Casedo refinement — precise two-way passage links (PDF-04/FIL-04; proposed):**

- **Authoring:** Select readable text or draw a region on a scan, choose **Link to passage**, open a target beside it, select its passage/region and confirm both ends. Give the relation a human-written title. Permit same-document references and several links from one passage. Keep unfinished links visibly pending and cancellable. A scan-region link works without OCR; text selection depends on a usable text layer and does not certify OCR accuracy.
- **Reading:** Follow either endpoint, briefly emphasise the exact destination and offer **Back to source** without losing scroll/zoom. Show incoming and outgoing references with document title, source page, user-entered paragraph label and review status. A printed paragraph number is descriptive metadata, not a unique technical identity. A two-way navigation relation does not mean each passage legally supports the other.
- **Anchor integrity:** Record document identity, revision, page identity and page-space coordinates for both endpoints, with selected text/context only when available. Do not anchor solely to final bundle page numbers, a text-string search or an extraction-run index. Supported geometric edits may remap anchors through a tested transform; replacement, deletion, reflow, OCR changes or ambiguous text must expose unresolved/review-needed links rather than silently relocate them. Keep mappings consistent with Undo/Redo and portable restore.
- **Paragraph bookmarks:** Extend FIL-04's existing page-target model with an optional position/region and descriptive title, sharing the anchor rules. Existing page bookmarks remain valid. A named point or passage bookmark is separate from a link with two endpoints.
- **Review notes and search:** Extend PDF-03 with a matter-wide list of notes, highlights, passage bookmarks and references; filter by document/type/review state and search note text directly. Reuse the current working-PDF search, keeping content hits distinct from authored notes and incomplete OCR coverage visible. Named issue tags may help users organise analysis; colour alone must not carry meaning, and tags never reorder the filing bundle. Start with search/list controls, not a new legal-research or AI service.
- **Export contract:** Workspace links remain useful even when their targets are reference-only. Export only explicitly selected, resolved internal references whose endpoints exist in that output, mapping coordinates/pages through the deterministic page map. Define standalone-volume behavior separately from combined-PDF behavior; report omitted/unresolved references rather than emitting broken links or leaking local paths. App backlinks are not automatically PDF backlinks. Preserve the current omission policy until this new path is verified. Review notes/highlights/private bookmarks stay out of submission-preparation output by default; any review-copy inclusion is explicit and recorded.
- **Data handling:** Quoted snippets, link titles and review notes are matter data under the existing vault/backup boundary. Redaction must also consider linked snippets, search indexes and exported metadata so the new feature cannot reintroduce removed content. A redacted or unavailable destination needs visible resolution, never a leaked preview from an old cache.

**Code-inspected baseline, 15 October 2026:** `App.vue` renders one working-document `PdfViewer` and already offers matter-wide working-PDF text search. `PdfViewer.vue` has page bookmarks and bounded native-text editing, not a general passage-link authoring layer. PDF-03 remains partial; PDF-04 remains open. Build on these foundations and the existing target-resolution work; do not mark this feature complete because a link icon or second canvas exists.

**Acceptance if selected:** In a synthetic matter, link a petition paragraph to an annexure region and to a judgment passage; navigate both ways and open the target in the second pane. Test duplicate quoted text, native/scanned/mixed-script pages, rotation/crop/reorder, source replacement/deletion, redaction, Undo/Redo, restart and portable restore. Check exact destination placement after supported edits; all other cases show truthful unresolved states. Verify saved combined and standalone PDFs independently, with reference-only sources/private notes excluded and no local paths or cached redacted snippets exposed. Measure reader memory/rendering on bounded large fixtures; two panes do not remove existing resource limits.

Cloud collaboration, author permissions, secure download links and DMS connectors remain separate future architecture decisions. A local review-copy option must not imply any of those services exist.

#### 6.2.4 Exhibit Flow candidates: volume control and output variants

**Status:** Research recommendations, not implemented or added to the approved five-item scope. Scope against FIL-03/06/08/09/10 and GEN-03 before coding.

- **Explicit volume breaks:** Allow a proposed break before a whole document/group, with backend-calculated index growth and ranges. Reject breaks that violate cohesion or page limits; explain oversized indivisible inputs. Keep page capacity and measured byte size separate. Rendered file sizes require final verification, and any compression/split/rebuild must regenerate and revalidate navigation. Do not promise arbitrary files will fit a portal limit or split evidence silently. §6.4 supplies a Delhi-specific example, not a national split policy.
- **Optional individual labelled PDFs:** If selected, generate these alongside the combined PDF/volumes from the same confirmed metadata and build manifest. Define local versus bundle pagination explicitly, naming collisions and included cover/stamp policy. Every offered output must have been generated and checked; exporting individual files must not overwrite imported sources.
- **Editable Word index, later:** Generate an explicitly labelled editable draft from the same metadata/page map and identify its build. Manual edits in Word cannot become an authoritative page map or silently update the PDF. Regeneration makes old drafts visibly outdated; final PDF ranges still require reconciliation. Hindi/regional-language output depends on §3.7 font/shaping work.
- **Persistent Bates continuation, later:** A separate identifier ledger would reserve ranges per matter/production and record completed builds, cancellation/retry behavior and backup/restore conflict handling. Define an explicit rule against accidental reuse across issued productions. Never use “continue last number” as an implicit court-pagination policy or infer that such numbering is required by Indian courts.

**Acceptance if selected:** Add pytest cases for manual breaks near index growth, group boundaries, oversized documents, cover insertion and mixed-size outputs. Check actual bytes of each intended filing artifact after generation. Combined/volume/individual outputs and editable drafts must resolve to the same recorded input revisions and label assignments. Test cancelled jobs, failed exports, duplicate output names and stale drafts; original hashes and earlier build bytes remain unchanged.

### 6.3 Background job model

Build and OCR operations are queued through one Python thread executor. Progress is polled via `/jobs/{id}` and persisted in SQLite. On restart, queued/running jobs are marked interrupted, unpublished build staging is removed, and the UI offers a new-job retry. Jobs do not resume midway. Terminal job records are retained for 90 days. Cancellation checks occur between volumes and OCR pages; responsiveness during large single-volume work remains unverified under DAT-03.

Document revisions and a process-wide PDF processing lock now serialize API PDF operations and builds/OCR/compression across matters. This prevents concurrent native-library use; it is not crash isolation. Moving native PDF work to supervised worker processes remains a release-hardening consideration. Do not add concurrent native calls without validating the library execution model.

#### 6.3.1 Reviewed import-cleanup queue (IMP-01)

**Status:** Approved scope, 15 October 2026; TODO. Enhancement priority 3 of 5.

**Need:** Make phone images and scans easier to prepare without silently changing evidence.

- Add **HEIC and multipage TIFF (`.tif`/`.tiff`) import** alongside existing formats, with local conversion to working PDFs. This is the approved amendment to DEC-03's earlier exclusion of these formats; other excluded formats remain outside that amendment.
- Queue optional orientation suggestions and page-size normalization choices. Flag very small/low-resolution images or likely unreadable screenshot scaling for review; report uncertainty rather than claiming recovered detail.
- Show before/after previews and allow accept/reject per proposed change, with explicit selection for batch acceptance. Preserve aspect ratio and full visible content when fitting a page. Do not automatically crop evidence or overwrite source files.
- Record accepted transformations as working revisions with Undo/Redo, progress, cancellation and per-file failure reporting. Identify unsupported codec/variant and resource-limit failures clearly.
- Observe signed-input safeguards under PDF-11 before applying transformations to PDFs; do not imply a visible signature or OCR result is verified.

**Acceptance:** Licensed synthetic HEIC and multipage TIFF fixtures preserve expected page order, orientation, visible content and original hashes. Supported codec variants and limits are documented and tested on Windows and both macOS architectures. Rejecting a suggestion leaves the working copy unchanged; accepting it is undoable and changes export freshness. Fixtures include rotated photos, small screenshots, mixed page sizes, corrupt input and cancellation; partial failures cannot be presented as complete success. Core conversion works offline without an external upload service.

### 6.4 Versioned court profiles

In `backend/app/rules/profiles.json`:
- `delhi-hc-illustrative`: technical subset; `max_file_mb: 300`, `require_searchable_text: true`, `require_bookmarks: true`, `disallow_pdf_encryption: true`. `status: NOT_LEGALLY_APPROVED`.
- `general-demo`: no court-based correctness assertion. `status: DEMO_ONLY`.

Every profile stores source links, checked date, and description. The application does not yet implement a granular court-rule DSL or lawyer-approval workflow.

**India adaptation evidence checked 15 October 2026:** These sources support scoped design decisions, not legal approval of our profile:

- [Delhi notification No. 6/Rules/DHC, 24 January 2026](https://delhihighcourt.nic.in/files/2026-02/notifications-and-practice-directions/12_0.pdf), effective on Gazette publication recorded as 27 January 2026: Rule 3.4 specifies 300 MB for the High Court and 20 MB for Delhi District Courts, subject to notified enhancement. It directs oversized High Court files to designated-centre intranet filing; for District Courts it describes parts no larger than 20 MB or E-Sewa assistance. This supports route-specific limits and measured-byte review, not a universal 300-page rule or an automatic High Court splitting instruction.
- [Delhi High Court e-filing FAQ](https://www.delhihighcourt.nic.in/web/sites/default/files/efiling/efilingfaq.pdf), Q12 and Q37–38: describes bookmarks and scanning/OCR expectations. The document includes lockdown-era instructions and a 50 MB limit; do not copy that limit over the later amendment. Retain source dates/conflicts and recheck operative directions during profile approval. OCR/searchable text and navigable output remain useful technical checks without proving accuracy or acceptance.
- [Delhi practice directions No. 74/Rules/DHC, 16 March 2021](https://www.delhihighcourt.nic.in/web/sites/default/files/efiling/PRACTICE_DIRECTIONS.pdf): specifies A4 and formatting for the stated Delhi High Court/District Court document scope. Do not extrapolate it into nationwide formatting or automatically shrink every evidentiary page. The treatment of source annexures and new separators needs a scoped advocate-reviewed specification.

Keep any filing-output encryption restriction tied to its reviewed profile and source. Protected local storage and portable backups are separate security features; adding a PDF password is not a general filing-preparation improvement.

## 7. API contracts

All endpoints require a bearer token. `launcher.py` refuses to start without `MATTERDRAFT_API_TOKEN`. In Tauri production-like mode Rust generates a new random token and sets it only for the launched child sidecar and its JS frontend via a narrow IPC command. In local browser development the static demo token is used; NEVER ship it as a production credential.

Main routes:

```text
GET    /health
GET    /profiles
GET    /matters
POST   /matters
GET    /matters/{matter_id}
PATCH  /matters/{matter_id}
DELETE /matters/{matter_id}
GET    /matters/{matter_id}/documents
POST   /matters/{matter_id}/documents         multipart files[]
POST   /matters/{matter_id}/documents/reorder
POST   /matters/{matter_id}/cover
PATCH  /documents/{doc_id}
DELETE /documents/{doc_id}
GET    /documents/{doc_id}/pdf
POST   /documents/{doc_id}/pages/reorder
POST   /documents/{doc_id}/pages/rotate
POST   /documents/{doc_id}/pages/delete
POST   /documents/{doc_id}/ocr
GET    /matters/{matter_id}/validate
POST   /matters/{matter_id}/build
GET    /jobs/{job_id}
POST   /jobs/{job_id}/cancel
GET    /matters/{matter_id}/exports
GET    /matters/{matter_id}/exports/{export_id}/{filename}
```

**Matter example:**

```json
{
  "name": "Fictional Filing - Demo",
  "court_profile": "delhi-hc-illustrative",
  "case_type": "Demonstration filing",
  "petitioner": "Example Applicant",
  "respondent": "Example Respondent",
  "advocate": "",
  "case_number": "",
  "max_pages_per_volume": 300,
  "keep_groups_intact": true
}
```

**Document metadata example:**

```json
{
  "title": "Synthetic Annexure - Bank Statement",
  "kind": "annexure",
  "part": 2,
  "label": "P-1",
  "group_key": "annexure-group-1"
}
```

**Page operation examples:**

```json
{"page_index": 0, "degrees": 90}
```

```json
{"order": [2, 0, 1]}
```

`POST /build` returns a job. Poll its status; on completion its `result` contains an export manifest and filenames. The combined and volume PDFs live in a matter-local export folder. The browser uses authenticated fetch and Blob download. Tauri uses a native Save dialog and streams to a same-directory temporary file before replacing the destination; Apple Silicon device evidence exists, while Windows and macOS Intel acceptance remain open under NAT-03.

## 8. Local development and QA

For exact instructions see root `README.md`. Condensed:

```bash
# Terminal 1 (macOS/Linux)
cd backend
python3 -m venv .venv && source .venv/bin/activate
python -m pip install -r requirements-dev.txt
./dev.sh

# Terminal 2
cd frontend
npm install
npm run dev
# visit http://127.0.0.1:1420
```

For a native desktop build, first build the platform-specific Python sidecar (with PyInstaller and `eng.traineddata`) by running `python backend/scripts/build_sidecar.py` from the repository root. Then `cd frontend && npm run tauri:dev` or `npm run tauri:build`.

**Do not try to run `tauri:dev` before creating the target-triple sidecar binary.** Tauri's externalBin configuration requires it to exist. On macOS both Intel and Apple Silicon need matching builds. Use Rust, Node and OS build tooling appropriate for Tauri.

Run: `cd backend && python -m pytest -q`. The suite covers OCR, PDF index links, combined PDF, volume splitting, oversized document errors, group errors, page editing, authentication and source-integrity failure paths. Use the current run result rather than the handoff's historical test count.

Run: `cd backend && python scripts/build_demo_output.py`; outputs are in `samples/example_output/`. The demonstration uses invented parties, invented order and invented affidavits, not client material.

### Tested and untested reality (updated 13 October 2026)

- **Automated:** backend pytest, Vue production build and Vitest passed locally. Apple Silicon Rust release compilation and focused native Save transfer tests passed. See the acceptance log for exact runs and counts.
- **Browser runtime:** synthetic create/import/preview/edit/OCR/build/download journey exercised; browser file-picker automation was initially blocked and is covered by QA-01 once its Playwright run passes.
- **Apple Silicon native runtime:** packaged app startup, import, OCR, persistence, shutdown and native Save/Save As with PDF/ZIP byte checks were exercised on macOS 26.6.2. The unsigned arm64 DMG was built and verified structurally.
- **Still unverified:** clean devices, macOS 12 minimum, macOS Intel, Windows x64, signing/notarization and full failure-path/device coverage. Automated or local development results do not close these gates.

### Acceptance for the approved workflow improvements

Each planned task above has feature-specific acceptance criteria. Use synthetic fixtures and record browser checks separately from native Windows/macOS and clean-device checks. Workspace navigation and batch tables must retain keyboard access, readable scaling and existing save/discard behavior. New metadata must follow the packaged vault boundary and portable-backup migration conventions in [local security and retention](docs/LOCAL_SECURITY_AND_RETENTION.md).

For workspace and batch-table refinements, compare a fixed synthetic preparation task before and after the change: record elapsed time, interaction count, corrections and completion without assistance. Establish a baseline and product-reviewed targets during implementation; do not adopt competitor time-saving or court-acceptance statistics.

The repeatable demonstration must complete a synthetic matter, insert a late document, explain the impact and regenerate matching indexes, bookmarks, volumes and CSV. Record artifact checks and visible output review. Every new volume/index/numbering rule requires pytest boundary cases derived from the shared page map; UI previews must not implement an independent pagination calculation. This demonstration supplements failure-path and native-device acceptance rather than replacing them.

**Bundledocs-review follow-up checks:** Add sample-matter isolation, section navigation and keyboard move coverage when refining UX-05. If the §6.1.3/§6.2.3 candidates are selected, first translate their acceptance criteria into scoped implementation tasks; do not mark them accepted based on the competitor demo. Check private-note exclusion and template cross-matter isolation explicitly. Record web-marketing QA separately from desktop/native QA; loading the landing-page static preview is not a successful Nuxt production build, signup-email test or evidence of increased conversion.

**Exhibit Flow-review follow-up checks:** Use a synthetic petition with explicit annexure references, ambiguous/duplicate filenames and a linked translation group. Review proposed labels, apply formatting with one placement exception, build, then insert/remove/reorder a document. Verify a truthful label/page impact report, unresolved external-reference checklist and immutable prior output. Include rotated pages, crowded signature/seal areas and covers that push an index/volume boundary. Cancellation of a label preview changes nothing; confirmation never changes original filenames or bytes. Candidate output/volume features require the additional §6.2.4 acceptance checks. Documentation of these proposals is not implementation completion.

**Casedo-review follow-up checks:** Scope the §4.1 dual-reader and §6.2.3 passage-link acceptance before implementation. Compare finding a cited passage and returning to the petition against the current page-only workflow; record wrong-target errors as well as elapsed time. Verify keyboard access, active-pane clarity, private-note exclusion and no implicit document inclusion. Apply the existing page-map pytest requirement to any new exported link/bookmark rule. The homepage demo is evidence of an interaction pattern, not successful MatterDraft implementation or court acceptance.

## 9. Product limitations and required engineering decisions

1. **Licensing:** PyMuPDF offers AGPL/commercial licensing routes; obtain an appropriate license/assessment before proprietary distribution.
2. **Local data security:** The SQLite DB and original/working files are plain on disk; production legal work needs threat modeling, optional storage encryption and safe temporary-file/wipe policies.
3. **No product-grade PDF signature validation:** PDF-11 now detects conservative certificate-signature dictionary indicators, retains inconclusive status, supports reviewer classification of visible/scanned marks and requires explicit acknowledgment before transformations. It does not validate certificates, identities, trust chains, revocation, signing time or byte coverage. Reordering, rasterizing, stamping and combining may still invalidate or disturb signed material; use an approved legal/signing workflow outside this application.
4. **OCR fidelity:** OCR may misrecognise text; image-only and mixed pages are rendered at 240 DPI for recognition, but only the invisible recognition layer is added to the existing page; existing GlyphLessFont layers are skipped. It is NOT safe to treat OCR output as the authoritative document text.
5. **English-only text generation:** The PDF index uses built-in Latin fonts. Hindi and other scripts require bundled, properly licensed fonts and comprehensive text shaping tests. UI strings may be Unicode but PDF index rendering is not yet complete.
6. **Single fixed index layout:** 23 entries per A4 index page, simple columns. Real cases require court/category-specific index templates, sometimes hyperlink conventions and paragraph-level references.
7. **No complete document QA:** Signature validity, legal sufficiency, translation certification, chain-of-custody and proper execution are unassessed.
8. **No court portal submit:** App exports files, it does not upload or mark matters filed.
9. **No generic Office conversion:** Imported formats are PDF/JPEG/PNG only. Word/Excel/PPT conversion is substantially more complex than image-to-PDF; especially reverse conversion to editable formats.
10. **Native Save target coverage incomplete:** The app streams exports through a native Save dialog. Apple Silicon Save/Save As, cancellation, replacement and permission failure passed locally; Windows/macOS Intel and low-disk/interrupted device cases remain NAT-03 work.
11. **Export profile size check:** The selected technical cap blocks publication if the ZIP, combined PDF, any volume PDF, CSV or validation report exceeds it. The illustrative Delhi profile is not legally approved; other routes may require distinct constraints.
12. **Editing history is partial:** Matter-scoped undo/redo covers page/OCR revisions, metadata, settings and order, with 100 retained events. Import, archive and replacement are not reversible; document-set changes clear the replay history. There is no collaborative locking or complete filesystem recovery system.
13. **Archive recovery is available:** Matter/document removal archives records and keeps originals and working revisions. The workspace can restore them, and a portable package can restore a separate copy. There is no in-app permanent deletion; local data and backups remain unencrypted.
14. **Target builds incomplete:** Vue and local Apple Silicon Tauri builds passed. Windows, macOS Intel and clean-device/minimum-OS checks remain open.
15. **No app updates or signing pipeline:** Production distribution requires signed/notarized builds, updates, release rollback, reproducible builds and architecture-specific testing.

## 10. Recommended backlog after developer machine brings up the UI

### Approved enhancement priorities — 15 October 2026

The product owner retained five recommendations from the CaseReady review in this order. The ranking applies within this enhancement set; existing P0 integrity/native work and security, multilingual-output and legal-review gates retain precedence. No delivery date or first-release inclusion of every item is implied. Chronology remains a later increment. Product approval does not close DEC-02 or approve any court-specific label/index rule.

| Rank | Improvement | Task / requirement section | Existing work to extend |
| --- | --- | --- | --- |
| 1 | Editable document table and batch actions | UX-05 / §6.1.1 | DAT-01/02/05, FIL-02 |
| 2 | Reusable output presets and stamp previews | FIL-09 / §6.2.1 | FIL-02/03/08, PDF-04 |
| 3 | Reviewed import cleanup, HEIC and multipage TIFF | IMP-01 / §6.3.1 | PDF-01/02/06/08/11, DAT-01/03 |
| 4 | Rebuild explanation and export review | FIL-10 / §6.2.2 | FIL-04/06, NAT-03 |
| 5 | Source-linked chronology, later increment | CHR-01 / §6.1.2 | DAT-01/02/04, FIL-04 |

These requirements extend the existing engine and workspace. The HEIC/TIFF selection amends DEC-03's earlier format exclusion for those inputs only. Permanent redaction remains PDF-10. This scope addition does not introduce a cloud client portal, team cloud storage, AI drafting, predictive outcomes, automated e-filing or universal compliance claims.

Design references reviewed: [CaseReady demo](https://caseready.io/case1.mp4), [feature roadmap](https://caseready.io/features), and [workflow](https://caseready.io/how-it-works). These are inspiration, not legal requirements or independently verified competitor acceptance results.

### Presentation and workflow follow-up — 15 October 2026

- **Pricing:** Deferred by the product owner. No price, plan or billing model is selected here.
- **Market:** India remains the delivery focus; US expansion is off the current plan. Competitor research is for product/design learning.
- **Landing page:** Improve the existing `/Users/vishnu/Downloads/caseready` project according to §4.2, preserving its visual identity and signup infrastructure. Implementation and any deployment are future work; this review changes the handoff only.
- **App interaction refinements:** Fold the §4.1 section/table/preview principles into UX-05, the grouped settings into FIL-09, and named-build review into FIL-10. The after-2:00 follow-up makes the combined indexed PDF and in-app link navigation explicit under §6.2.2; search-assisted redaction and cross-document references extend existing PDF-10/PDF-04 proposals under §6.2.3. Reuse current tools rather than duplicate the bundling engine or PDF editor.
- **Candidate capability order after scoping:** (1) reusable structure templates and explicit include/exclude, (2) source-linked review notes and output inclusion policy, (3) selected page ranges/missing-item checklist, (4) stable-reference late inserts. These proposals do not reorder the five approved priorities or bypass native/integrity/legal gates. Track any selected additions in `docs/IMPLEMENTATION_TASKS.md` before coding.
- **Evidence:** §4.3 records the current sources, sampled video timestamps and limits of verification. Earlier BatesFast sample-editor observations support a document/preview/context-panel layout; [Casedo](https://www.casedo.com/) and [Bundledocs](https://www.bundledocs.com/) support a clear workflow/demo/help presentation. None establishes a measured conversion advantage.
- **Exhibit Flow follow-up:** §4.4 records the new video review; merge label-register/renumbering safeguards into UX-05/FIL-02, batch mark/separator previews into FIL-09 and label-change explanations into FIL-10. Scope §6.2.4 candidates separately before adding implementation tickets. §6.4 records the Indian court-source checks and their limits. Preserve the approved five-item order, India focus and deferred pricing decision.
- **Casedo follow-up:** §4.5 records the full homepage-video transcript review, visual observations and supporting documentation. Within the existing review-note/reference candidate, recommend (1) optional dual reader and precise bookmarks, (2) two-way passage links with revision-safe targets, (3) searchable review notes, then (4) explicitly scoped PDF link export. Tie these to UX-05, PDF-03/04 and FIL-04/10 before coding. Reuse §6.1.3 inclusion controls and the §4.2 demo presentation plan; do not reorder the five approved priorities or introduce cloud/AI/US expansion.

### Milestone 0 — Native integration verification (first)

- `npm install`, `npm run build`, `npm run test` and `npm run tauri:dev` on Windows and macOS.
- Fix any Tauri API/plugin/binary packaging issues found in runtime testing; check CSP, PDF.js WebWorker and downloaded PDFs in each OS WebView.
- Verify PyInstaller sidecar embeds tessdata and handles fresh installation without external Python/Tesseract dependency.
- Finish native Save validation on Windows and macOS Intel, including low-disk and interrupted transfer cases; test files larger than 80 MB.

### Milestone 1 — Filing bundle workflow completion with an advocate

- Refine UX-05 batch review in the existing workspace; extend FIL-02/03 with FIL-09 presets and stamp previews once the relevant output policies are reviewed.
- Extend FIL-06 with FIL-10 change explanations and integrated export review.

- Obtain 2-3 **anonymized** full real filing examples and the approved outputs, including indexes, bookmarks, annexures and chamber workflow.
- Agree exactly what Parts 1–4 mean and why ~300 pages is mentioned.
- Implement at least one **lawyer-approved** case-type intake schema + document checklist/template, not only the current synthetic cover.
- Add index layout selection, page-label schemes, and stricter source-to-output validation tests.
- Add an explicit “needs legal review” workflow and a case-type/stage selector.

### Milestone 2 — Harden the core

- Add IMP-01 reviewed cleanup and local HEIC/multipage TIFF import, with before/after approval, original preservation and platform/codec acceptance.

- Complete undo/redo across document-set changes and test portable backup/recovery on native target devices.
- Durable job queue, interruption recovery, failed-file quarantine, preview thumbnails and memory/performance tests on 1000+ pages.
- Encrypted local storage design; handling of signed documents, redaction and native printing.
- Additional OCR languages with approved traineddata and fonts, mixed scanned/native PDFs, batch automation.
- Complete transaction-safe event history and recovery from failed operations.

### Milestone 3 — Expansion toward full client brief

- Add CHR-01 manual source-linked chronology with review status and revision-aware page references; keep it independent of bundle ordering and defer AI extraction to a separate specification.

- Court profile data model with exact rule provenance, source references, effective dates, lawyer approval and versioned updates.
- Conditional legal forms/templates, well-defined Word and Office conversion capabilities.
- Manual and auto generated bookmarks/indices with template variations; index recheck after replacement.
- Legally reviewed objection/refiling workflow and acknowledgment tracking, optionally portal integrations where explicitly authorised.
- Advance PDF operations (crop/replace/compress/true redaction, link insertion), optional signature provider integrations and secure sharing architecture.

## 11. Client questions that require answers before a paid fixed-scope implementation

1. Which court(s), bench/side(s), case types and filing stages are the FIRST commercial targets?
2. What exactly are Parts 1, 2, 3 and 4? Supply an annotated screenshot or accepted sample matter.
3. What official direction or local chamber practice gives the approximate 300-page volume cap? Does the index consume pages? What about one document exceeding the cap?
4. Are groups like 1a/1b/1c indivisible? Are annexures separately indexed, and how are separators/covers handled?
5. What does “OCR-based editable text” mean: selectable/searchable text, correcting OCR, or visual full-PDF text reflow?
6. Which Word/Excel/PPT conversions are necessary, in which directions, and with what quality guarantees?
7. Which case types and conditional document templates should be included, and who approves them legally?
8. How should locally edited copies and signed/certified originals be separated in each real workflow?
9. If online sharing is requested after the approved offline first release, what hosting, access, expiry, revocation and retention model is required?
10. Who maintains official court rules, resolves conflicting amendments and signs off updated case-type templates?
11. What installer signing, encryption, audit-log and production support expectations apply to the approved target platforms?

## 12. Architecture decisions to preserve

**Keep:** local-first; immutable imports; Python PDF processing separated from desktop shell; deterministic index/outline/page-map; source-linked technical court profiles; configurable volume splitting; explicit warnings, progress and cancellation; simple versionable service modules.

**Avoid:** a hard-coded nationwide court checklist; treating 300 MB as 300 pages; treating OCR output as verified evidence; “AI compliant” claims; cloud dependency for core document processing; embedding arbitrary filesystem write capability in the Vue frontend; making every PDF editing action change the source PDF; pretending visual PDF editing is trivial.

## 13. Useful technical documentation

- Tauri 2 sidecars: https://v2.tauri.app/develop/sidecar/
- Tauri 2 configuration: https://v2.tauri.app/reference/config/
- Tauri shell command: https://docs.rs/tauri-plugin-shell/latest/tauri_plugin_shell/process/struct.Command.html
- PyMuPDF OCR guide: https://pymupdf.readthedocs.io/en/latest/recipes-ocr.html
- PyMuPDF documentation: https://pymupdf.readthedocs.io/en/latest/
- PyMuPDF licensing: https://pymupdf.io/licensing
- FastAPI documentation: https://fastapi.tiangolo.com/
- Vue 3: https://vuejs.org/
- PDF.js: https://mozilla.github.io/pdf.js/

## 14. Handoff request for the next coding agent

**14 October 2026 implementation note:** SQLite schema v5 adds revision-aware manual bookmarks. `GET|PUT /matters/{id}/bookmarks` manages an ordered, nested list; older revision targets remain visible but are omitted from generated outlines with an explicit report warning. PDF volume and combined outline destinations derive from the same document page map as indexes and CSV. Whole-document replacement uses `POST /documents/{id}/replace` with another active matter document as the source, writes a new working revision, and supports Undo/Redo. Portable matter packages are now v2 to carry bookmarks; v1 restore remains supported. Build manifests have a digest of relevant matter settings, ordered document metadata, original/working bytes, bookmarks and profile content; old manifests are treated as stale. The 14 October evidence and remaining human/legal gates are in `docs/ACCEPTANCE_TESTS.md` and `docs/IMPLEMENTATION_TASKS.md`. These additions do not approve the illustrative court profile.

**14 October 2026 index review follow-up:** In the matter's Documents page, **Bookmark page** marks the currently displayed PDF page. The former collapsible index preview above the document workspace was removed to restore panel height. The adjacent **Index & bookmarks** tab contains the full preview and manual bookmark editor for rename/nesting/reorder/review before building; Build retains a read-only preview alongside settings. Preview calls read-only `POST /matters/{id}/bundle-preview`; its rows and PDF rendering call the same `volume_rows` page-map function. The UI shows index-page reservation, volume page ranges, combined offsets and manual bookmark targets; index rows open their source working document. After export, a read-only PDF.js viewer can show generated PDF pages and outlines for files up to 25 MiB. Filing-specific index layout, numbering conventions and multilingual rendering remain under FIL-03/FIL-08 and require reviewed examples. Native viewer behavior is unverified.

**Later 14 October update:** SQLite schema v6 adds bounded, matter-scoped reusable draft values at `GET|PUT /matters/{id}/values`. Memo of Parties autosaves and these values enter persisted Undo/Redo. Portable packages are now v3 and restore v1/v2 packages as well. The values are not yet connected to forms or generated filing documents; that mapping requires the reviewed DEC-02 workflow. They remain plaintext under the current local security boundary.

> Continue this MatterDraft Studio **without replacing its core architecture**. Start by installing the frontend dependencies and running the Vue/Tauri application on a real machine, resolve compile/runtime issues, execute the Python test suite, and validate one complete synthetic matter. Preserve the deterministic page-map model and keep original files immutable. The existing Delhi court profile is illustrative and must never be described as an approved court rules pack. Do not begin nationwide legal logic or complete Office document conversion until actual case workflows and supported courts are confirmed. Maintain backward compatibility of API endpoints or document migration steps, and expand tests with each change.

---

**Decision summary:** Tauri 2 + Vue 3 + TypeScript frontend; Python FastAPI local backend; PyMuPDF/Tesseract PDF engine; SQLite/local files; deterministic matter, document, volume and index pipeline. Build the court-rule knowledge layer only from verifiable scoped sources and advocate-reviewed templates. MatterDraft Studio is an actively developed filing-preparation application. Release qualification and scoped legal-workflow approval remain separate requirements.

## PDF core hardening update — 14 October 2026

See acceptance section AB and the PDF reliability regression suite. This pass adds navigation-preserving page assembly, exact serialized bundle destination checks, Unicode outlines, canonical bookmark hierarchy, additive mixed-page OCR, rotated text placement, measured compression, validated/synced PDF replacement, partial-copy cleanup, honest cancellation outcomes and hash-verified committed-build recovery. Existing imported originals and all retained revisions remain untouched.

Production qualification remains incomplete: native-device coverage, complex scan/font/form/signature fixtures, process crash isolation, full durability under actual power loss, multilingual printed indexes, legal workflow approval, security and distribution gates are not established by these automated tests. The 1,000-page benchmark uses native-text synthetic pages; it does not establish scanned-matter throughput or memory ceilings.

## PDF-02 compression acceptance update — 16 October 2026

The existing lossless, Balanced (240 DPI/JPEG 85) and Smaller (150 DPI/JPEG 70) presets now have a deterministic synthetic visual corpus and fixed regression budgets. The three 300 DPI pages cover a noisy scan, an explicitly unverified red synthetic seal and approximately 5–5.8 pt raster text. The generator emits before/after PDFs, side-by-side PNG sheets and JSON measurements; pytest enforces byte, PSNR, ink/contrast and red-shape floors. See [PDF-02 compression acceptance](docs/PDF02_COMPRESSION_ACCEPTANCE.md) and acceptance section AH. This closes the PDF-02 backend engineering ticket for current PDF inputs, not native device qualification, a universal size/fidelity promise, filing readiness or signature verification.

## PDF-04 links and output-marks update — 16 October 2026

PDF-04 is engineering-implemented with native acceptance still open. Reviewed internal links store source geometry and exact source/target document revisions; external links are limited to `http`, `https` and `mailto`. Export resolves them through the deterministic page map, retains cross-document targets in the combined PDF, retains same-volume targets in standalone volumes, and warns instead of emitting broken cross-volume or stale-revision links. Source PDF link actions remain omitted unless the user recreates them as reviewed links.

Matter-wide watermark and build-timestamp settings now have actual-page preview, timezone/format/placement controls and conservative overlap warnings. Builds apply them only to source-document pages at one fixed timestamp and record settings/results in reports and manifests. Originals, working copies and completed exports are never stamped. SQLite schema v12, portable package v9 and Undo/Redo preserve the settings and link review state. This supplies the PDF-04 primitives required by FIL-09; it does not implement reusable versioned presets, Bates numbering or court-specific layouts. See acceptance section AI and the PDF-04 backlog status for exact automated evidence and remaining native-device gates.

## PDF-03 image overlays and private review annotations update — 16 October 2026

Revision-anchored PNG/JPEG overlays, highlights and comments can now be drawn on a working-document page, reopened through persisted matter state, repositioned, edited where applicable and deleted. Annotation mutations participate in matter Undo/Redo, stale-output calculation and portable backup/restore. Later page edits leave annotations unresolved instead of silently remapping them. Image assets live inside the matter boundary; imported originals and working-copy PDFs are not modified.

New builds flatten only resolved image overlays into source-document pages through the deterministic page-map render path. Highlights/comments stay private and are excluded from filing-preparation PDFs by default. Reports/manifests record policy, counts, geometry and image hashes without private note bodies or internal asset paths. SQLite schema v13 and portable package v10 carry this state. See acceptance section AJ and PDF-03 backlog status for the automated evidence. Unicode visible-text/font support, an explicit review-copy inclusion policy and packaged Windows/macOS placement checks remain open; this implementation does not treat a note, highlight, image or OCR text as legally verified.


### B01B design implementation — 28 October 2026

The supplied B01B JSON now defines Build settings, progress/failure/completion, generated PDF review, export, stale updates/comparison and generated-file Storage retrieval. Existing presets, output marks, volume tools, Office/print/protected exports and storage maintenance remain connected. See [the B01B implementation and evidence record](docs/DESIGN_PHASEB01B_IMPLEMENTATION.md) and acceptance log for actual test/platform boundaries. New manifests track effective included inputs while preserving conservative legacy freshness and unchanged prior artifacts. No schema or deterministic page-map rule changed. B01C named artifact validation/release gating remains the next bounded task.


### B01C design implementation — 28 October 2026

The supplied BU07, 14C and ED34 JSON now defines saved-artifact checks, current-revision Preflight and exact finding review. Actual local output parsing/counts/navigation/labels/size measurements are stored separately from immutable files, with build/input/file identities and independent human review. Source changes preserve historical evidence; effective inputs determine current applicability. Authenticated inspection remains available for historical/failed contracts. Technical failures block review-copy export; reviewed release additionally requires current inputs, four human items and no unverified recorded redactions. Optional cleanup and independent redaction verification are explicitly unavailable, never passed. See [B01C implementation and acceptance evidence](docs/DESIGN_PHASEB01C_IMPLEMENTATION.md) for policy, new viewer-label rules, commands and platform limits. No SQLite migration; originals and prior artifacts are preserved. **B02A** is the next bounded task: preset/settings persistence, visible overrides, and cover/index preview.


### B02A design implementation — 28 October 2026

The supplied BU01/BU02 JSON now defines the preset dialog and cover/index workspace, with the relevant 11C/12A surrounding context. Local preset versions/provenance, field-level overrides and explicit preservation, atomic settings with history/guards, and bounded actual cover/index previews are connected. Cover pages, optional dates and review-index density extend the single deterministic engine map; generated totals, CSV, links, bookmarks, labels and combined offsets reconcile with actual PDFs. No schema migration; originals and frozen prior builds remain unchanged. Backend 265, frontend unit 44 and full browser 97 tests passed (one opt-in vault skip); reference/compact theme-scale matrices and Apple Silicon native preview/Save were checked. See [B02A implementation, native reconciliation and platform limits](docs/DESIGN_PHASEB02A_IMPLEMENTATION.md). B02B was next at this checkpoint; its completed record follows.


### B02B design implementation — 28 October 2026

The supplied new BU03, 11C and 12A JSON now defines numbering/navigation, the current/pending/error page map and bookmark destinations. Deterministic content/PDF/printed/Bates references, source identity and physical offsets stay distinct. Real working-page previews, saved settings/history/guards, hierarchy and explicit missing-target resolution are connected. Backend **285**, frontend unit **44**, full browser **101** passed (one opt-in vault skip). Apple Silicon sidecar/release compilation and isolated native packaged-sidecar review/navigation/Save passed; original/prior bytes and exported labels/links/CSV/pixels reconcile. See [B02B implementation and platform boundaries](docs/DESIGN_PHASEB02B_IMPLEMENTATION.md). No schema migration. B02C was next at this checkpoint; its completed record follows.


### B02C design implementation — 28 October 2026

[Certain] The supplied new 06A batch-review and 06D suggestion-review layouts are implemented. Actual filename/selected working-page evidence, editable independently selected fields, conservative date handling and optional group-only stable sorting use atomic local persistence and existing history. Stale context/source results reject without losing drafts; dirty navigation and caller/selection context are preserved. Accepted changes flow through existing engine invalidation/rebuild, with no new schema or page-map rule. Backend **296**, frontend unit **44**, full browser **107** passed (one opt-in vault skip). Thirty-two viewport/theme/scale screenshots plus a compact keyboard drawer were inspected. Apple Silicon sidecar/release and isolated native acceptance/reopen/generated review/CSV Save passed; original/prior bytes remain unchanged. See [B02C implementation, visual/native evidence and platform limits](docs/DESIGN_PHASEB02C_IMPLEMENTATION.md). B03A follows in the record below.


### B03A — new output-options implementation, 29 October 2026

[Certain] The supplied BU04 design now defines Output options, with persistent original/lossless/balanced/smaller processing, scoped PDF metadata cleanup and selected DOCX/XLSX/PPTX drafts. Actual generated sizes, staged processing and post-processing verification preserve original/prior bytes and existing page maps. Unsupported duplex and fixed omission policies are explicit. Backend **306**, frontend unit **44**, production build and matching Apple Silicon sidecar/release pass. Sixteen viewport/theme/scale renders were inspected; native generation, review, Save cancellation/retry, exact bytes/hashes and restart persistence were verified. Final full browser suite **110 passed, 1 skipped** (opt-in vault). **B03A is complete for the available target.** See [B03A implementation, comparisons, measured evidence and platform limits](docs/DESIGN_PHASEB03A_IMPLEMENTATION.md). No migration or new page-map rule. **B03B is next; not started.**


### B03B — new volume-planning and size-recovery implementation, 29 October 2026

[Certain] The supplied **13A/BU06** JSON defines the replacement workspaces. All entry points use the new layout; BU04 processing, actual revision/source context, page/byte policies, reviewed internal boundaries, atomic snapshot saves/history, dirty guards and caller return are connected. The deterministic engine map reconciles split source ranges, generated front matter, labels, bookmarks, links, marks, Part extracts and CSV/combined offsets. Measured over-limit failures preserve previous output; originals are unchanged. Portable restore remaps reviewed split IDs.

Backend **317**, frontend unit **44**, production build and matching Apple Silicon sidecar/release pass. Thirty-two viewport/theme/scale renders and BU06 reference comparison were inspected. Native measured failure → reviewed policy/split → two-volume build → review/Save cancel/retry and hash reconciliation passed; final app/sidecar restart retained settings. An earlier native automation paint delay did not recur in a fresh single-window follow-up: index and source raster/text rendered immediately. The observation and unknown cause remain documented. Final full browser: **113 passed, 1 skipped** (opt-in vault), **9.0m**. **B03B is complete for the available target.** See [B03B requirement checklist, design evidence, commands and platform limits](docs/DESIGN_PHASEB03B_IMPLEMENTATION.md). **B03C is next; do not extend this task into it.**


### B03C — new desktop, compact and dark Build settings, 29 October 2026

[Certain] The supplied **15A / 22C / BU10** JSON now defines the build-settings composition. Desktop and compact use the same persisted state; compact has its own hierarchy and action placement. Saved preset/overrides, outputs, structure/navigation, numbering/processing, engine counts and separate page/byte/profile limits replace generic summaries. Snapshot-matched preview/settings reads, error/retry and explicit return/focus preserve existing workflows. Core output checkmarks are truthful read-only summaries of the engine contract; Generated pages replaces the nonexistent illustrative preparation revision. No backend rule or migration was added.

Backend **317**, frontend unit **44**, production build and matching Apple Silicon sidecar/release pass. Final complete browser **116 passed, 1 skipped** (opt-in vault), **7.2m**. Sixteen viewport/theme/scale renders and direct 15A/BU10 comparisons were inspected; 22C uses resolved JSON because no PNG was supplied. Native compact/dark navigation, lossless/XLSX persistence, real build, visible generated index/source rendering, Save cancellation/retry and app/sidecar restart passed. Six artifact sizes/hashes and PDF labels/index/bookmarks/CSV reconcile; eight original/prior hashes remain unchanged. B03B's earlier raster-paint delay did not recur. **B03C is complete for the available target.** See [B03C requirement checklist, adaptations, commands and evidence](docs/DESIGN_PHASEB03C_IMPLEMENTATION.md). Windows/Intel/clean-device/vault-startup/Office-rendering/printing gates remain open. The rebuilt release app is in the repository build output; the installed app was not replaced. **B04A supplemental plan and prior-build comparison is next; no B04 implementation started.**
