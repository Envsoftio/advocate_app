# FLOW / 08 — Preflight implementation and requirements audit

16 November 2026. Scope: supplied `CourtBundle_Complete_Workflow (2).pen`, `CourtBundle_JSON_For_Codex/screens/14A.json`, `14B.json`, `14C.json`, and the 1440×900 `implementation/previews/14C.png`. The implementation kit identifies 14C as the current-revision layout and 14A/14B as earlier contextual examples. One real workspace implements findings, acknowledged review and changed-input states; no separate mock screens or fixture counters were introduced.

## Requirements and behavior

| Requirement / design element | Implementation and boundary |
| --- | --- |
| Separate technical errors, warnings and human review | Shared summary, severity sections, filter, technical notes, explicit pending/unavailable states. No success message or Build settings action when checks are unavailable or inputs are pending. |
| Missing originals, signatures, encryption, duplicate inputs/labels and searchable text | Existing engine checks and transformation acknowledgements retained. Original hashes and immutable output checks remain independent of UI review. |
| Working-page rendering and page counts | Preflight renders working pages at reduced resolution and compares PDF counts with stored source counts. Exact render failures carry document/revision/page context. This is not visual readability certification. |
| Invalid volume plan | The existing `bundle_preview`/deterministic engine map now runs during Preflight. Actual planning failures become blockers; Review volumes opens the real planner and returns to Preflight. No new volume/index rule. |
| Unresolved destinations | Existing omission policy remains a warning for unresolved bookmarks/links, now visible before building. Fix target opens the existing bookmark editor. Stable record IDs distinguish multiple unresolved entries. Stale titles/URLs are omitted from these messages to preserve redaction-report protections. |
| Document date | Missing saved dates produce reviewable warnings. No date is inferred or filled automatically. |
| OCR review | Current-revision OCR pages without persisted page acceptance produce exact-page review findings. Opening the page uses the existing reader's measured confidence and acceptance tools. The explicit Screen pages action added below separately measures candidate pages; it does not invent confidence percentages. |
| Four human acknowledgements | Latest order, annexure sequence, titles and planned pagination are explicitly acknowledged by the reviewer. They are preparation aids, not required-document rules or output approval. |
| Warning acknowledgement | Explicit Acknowledge/Reviewed control; warnings stay visible. Errors cannot be added to the acknowledged-warning set. Page-review notes remain tied to their historical check run. |
| Persistence and stale data | Local encrypted-file-aware atomic persistence under the existing validation directory, keyed by effective input digest. Rechecking or reopening identical included inputs retains acknowledgement; changed inputs show a fresh review. Exact prior-input evidence remains stored. Version conflicts and stale runs reject writes. Reference-only imports do not invalidate included-input review. |
| Optional filing checklist | Existing court/case/stage user-authored checklist remains available under Open review checklist. It is separate from the four preparation acknowledgements. |
| Build/output/release | Existing generated-file checks, exact-finding return, build-specific human review and reviewed-release gates remain unchanged. Preparation acknowledgements do not approve an artifact or unblock technical errors. |

The broader PRD is **not fully implemented as a legal-compliance system**. `MatterDraft_Project_Context_and_Handoff.md` and FIL-07/DEC-02 still require scoped official rules, effective dates, conditional requirements and advocate review. This change cannot supply those approvals. The illustrative Delhi profile remains unapproved. At the 16 November checkpoint automatic visual blank-page classification and bulk OCR-confidence classification were not implemented. The 17 November increment below supplies conservative screening; its results still require human review. Post-build pagination/readability must still be checked on the generated PDF. No cloud dependency, schema migration, original modification or change to the deterministic numbering rules was added.

## Design and UX decisions

The supplied hierarchy is retained: title and Run checks, four-column summary, error/warning rows with actions, expandable technical notes, four human-review controls and Build settings footer. The current 240 px expanded application sidebar replaces the older reference's 184 px sidebar, consistent with the user's later shell changes. At 1440 px the content starts at x=264 and has 1152 px available. Compact layouts wrap the toolbar, summary and review items; lists remain scrollable. Existing Inter, semantic colors, Reka select wrapper and native checkbox behavior are retained. No mock “7 passed,” invented preparation revision, court approval or automatic verification of OCR/signatures is shown.

Compared the supplied 1440×900 14C PNG with actual browser captures. The current layout deliberately has variable content height and additional acknowledgement/help text. This is an evidenced design adaptation, not a pixel-perfect claim. The final browser evidence includes real findings, reviewed state, changed inputs, technical blockers, light/dark layouts and compact/125% views.

## Verification

- Full configured backend pytest: **343 passed**, six dependency warnings, 71.58 seconds. After the final stable unresolved-record identity adjustment, **24 focused tests passed**, including all new Preflight tests, redaction and output-validation regressions.
- Frontend production build and **49 unit tests passed**; final formatting and browser/native results are recorded in the acceptance log.
- New backend tests cover snapshot persistence and conflict detection, reference-only freshness, authentication, rejection of error acknowledgements, exact render failures, volume-plan failure, unresolved bookmark policy, OCR page acceptance and working-count mismatches. Existing tests caught and prevented stale bookmark/link text leaking into redaction reports.
- Browser tests exercise current checks, acknowledgement and reload persistence, input-change reset, validation failure/retry, original-byte preservation, membership/volume repair, keyboard acknowledgement, bookmark repair, exact-source finding review, immutable output checks and reviewed download with matching bytes.
- macOS Apple Silicon matching sidecar and app compilation are exercised. Browser download is verified; OS Save dialogs, packaged WebView interactions, Windows, Intel Mac and clean-device installation are not claimed as tested in this pass.

New preparation-review files use the existing validation directory lifecycle. Portable matter backups do not export validation-run history or these preparation acknowledgements; a restored matter requires a new review. This matches existing validation evidence boundaries rather than suggesting portable approval.


## 17 November increment — automated page screening

Preflight now offers **Screen pages** for included working PDFs. It renders bounded grayscale pages, identifies nearly blank candidates conservatively (no extractable text and at most 0.01% pixels darker than 245/255), and vetoes blank classification where resolved PDF output marks are present. The rendered crop and thresholds are screening aids: faint, cropped or unsupported content can escape detection. No page is removed.

Image-bearing and OCR-status pages receive an actual separate Tesseract pass. Mean confidence and low-confidence regions below 75 are review signals, not text accuracy or signature verification. Native-text pages and possible blank pages skip OCR. No recognized words are saved in screening evidence; unavailable analysis and no recognized words remain explicit warnings. The evidence records method, thresholds, language, engine/model provenance and per-page revision identity.

The scan is an explicit background job with progress, cancellation and rerun, using the existing sequential worker and matter lock. Tesseract cancellation terminates its child process. Results publish atomically only after all pages and the unchanged effective input digest are checked. Cancelling preserves prior completed evidence; edits hide stale findings. Original and working bytes are unchanged. Existing output/release gates remain independent. The supplied layout is extended with a compact screening strip above findings; review rows retain their design and exact-source actions.

Court-specific conditional validation is still awaiting scope and actual reviewer evidence. See [the concrete scope, source and approval acceptance checklist](PREFLIGHT_COURT_RULES_ACCEPTANCE.md). The earlier FIL-02 cross-reference was incorrect; FIL-07 is the scoped validation ticket.


## 17 November court-reference increment

Official-source draft guidance now covers Supreme Court of India and Punjab and Haryana High Court alongside the existing Delhi illustrative reference. The reference selector is independent of saved matter settings, Delhi-only Memo tooling stays Delhi-only, and Preflight links unapproved-profile findings to the selected guidance. All profiles retain explicit lack of advocate approval. Research sources, retrieval limits and remaining conditional-validation acceptance are recorded in [court-rule acceptance](PREFLIGHT_COURT_RULES_ACCEPTANCE.md).

Final verification: **350 backend tests**, **49 frontend unit tests**, **8 focused browser tests**, frontend build/format checks and a matching Apple Silicon sidecar/app build passed. See acceptance entry ER for precise evidence and native-runtime limits.
