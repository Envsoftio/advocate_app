# B02C — New batch review and source-backed metadata suggestions

28 October 2026. **[Certain] B02C implementation and scoped acceptance checks are complete.** The primary layouts were rebuilt from supplied **screen-06A** and **screen-06D**, with local source evidence and explicit field acceptance. B03 has not started. Technical QA does not establish court approval, document authenticity or legally verified OCR/signatures.

## Requirement-to-evidence checklist

| Requirement | Implementation and evidence |
|---|---|
| New supplied design | Read the B02 metadata JSON, shared rules/tokens and reference index; resolved reusable components, descendant overrides and theme variables with a JSON parser. Inspected `implementation/previews/06D.png`. No supplied 06A PNG exists; its resolved JSON hierarchy and dimensions define the batch layout. |
| 06A composition | New heading, compact action/filter toolbar, editable column grid, row selection, column visibility/density and persistent bottom Save/Discard bar replace the legacy primary layout. Existing labels, Parts, groups, group hierarchy, ordering and document opening remain available. Advanced label/sort/group tools are secondary disclosures. |
| 06D composition | New filter/list/source workspace. At 1440×900/100%: list **x208/y266/w724/h586**, source **x948/y266/w468/h586**, verified by browser assertions. Bundled Inter and supplied semantic tokens. Actual source PDF replaces the reference illustration. |
| Real suggestions/evidence | Title proposal is the actual filename stem with underscores/whitespace normalized. Date extraction uses the actual filename and explicitly selected working page (page 1 initially). Evidence records document/source identity, source and working SHA-256, working revision, selected page, raw matches and an evidence digest. No AI provider, confidence score or cloud service. |
| Conservative dates | Only valid ISO or English named-month dates are proposed. Numeric day/month, invalid or conflicting dates remain blank for manual review. Missing text/date stays unavailable; unreadable working sources cannot support date acceptance. A valid manually entered date still needs explicit selection and acceptance. |
| Current/proposed/evidence separation | List and source panel show saved current values separately from editable proposals and evidence. Working revision/page are visible; another source page can be explicitly read. Filters use actual pending-field counts. Real page preview, enlarge/fit and scroll are connected. |
| Independent acceptance | All fields start unchecked. Acceptance count includes only selected, valid, undismissed fields. Title-only acceptance changes no date, membership, order or other document. Date acceptance explicitly records user confirmation. Both UI and backend validate; confirmed values are never overwritten without the user selecting that field. |
| Dismissal/pending drafts | Dismiss only hides a field for the current review; it does not persist a metadata change. Pending proposals never update authoritative metadata. Unselected edited proposals survive another field's successful acceptance. Refresh/discard starts a new local review; dismissals are not durable rejections. |
| States | Loading, empty filter, uncertain, unavailable source, error/retry and stale conflict states. Old preview requests cannot replace a newly selected document. Current context versions distinguish a delayed refresh of our own save from a genuinely conflicting change. |
| Optional sorting | Separate unchecked checkbox. Available only for the selected document's named group. Confirmed dates ascending; unknown/unconfirmed last; equal dates retain previous order. Only the group's existing position slots are reused, leaving other groups/documents in place. Sorting and metadata commit together. Tested with ties, unknowns and Undo. |
| Atomic persistence/concurrency | Authenticated local endpoints, matter lock, expected whole-matter/document snapshot, recalculated source-evidence digest and one SQLite transaction. Metadata, optional positions, audit and existing `document_batch` history commit together or roll back. Duplicate/out-of-scope fields and stale saves reject. Accepted evidence is stored in history's `after_json.metadata_evidence`; normal existing history retention applies. No schema migration. |
| Dirty/navigation behavior | Save/Discard/Keep editing integrated with batch close, review return, workspace/tab navigation, before-unload and Cmd/Ctrl-S. Failed/conflicting saves retain drafts. Table selection/filter and caller document/bundle context remain in the existing workspace. Tab moves editable fields; Escape resets a batch row. Compact source drawer restores focus on Escape. |
| Derived state/rebuild | Existing modified-event refresh and effective-input digest invalidate affected build/index/preflight context. Current page references are recalculated by the existing engine. Backend tests and native checks rebuild and verify accepted title/date in the actual PDF index and title in the outline. Existing output validation/download gates remain active. |
| Preservation | Original bytes and every file of a prior frozen build remain hash-identical. Membership is not rewritten. B01/B02A/B02B regression coverage passes, including numbering, bookmarks, previews, release gates, generated review, export and history. No new volume/index rule or change to `engine.py` in B02C. |

## Changed paths and integration

- New `backend/app/metadata_review.py` contains UI-independent extraction, evidence, validation, transaction and source-preview logic. `backend/app/main.py` exposes GET/PUT `/matters/{id}/metadata-review`, POST `/matters/{id}/metadata-suggestions`, POST `/matters/{id}/metadata-suggestions/accept`, and GET `/documents/{id}/metadata-source`.
- New `frontend/src/components/MetadataSuggestions.vue` and `frontend/src/metadata-design.css`; rebuilt primary `DocumentTable.vue` template. `App.vue` integrates shared shell/context and dirty guards; `main.ts` loads scoped design CSS.
- New `backend/tests/test_metadata_review.py` (11 executed cases, including parameterized dates) and `frontend/e2e/metadata-suggestions.spec.ts` (5 cases). Existing `journey.spec.ts` and `json-design-workspace.spec.ts` selectors follow the new editable layout while retaining behavior assertions.
- Updated README, handoff pointer/root and acceptance log. No dependency, storage schema or page-map rule change. Existing staged/unstaged work was preserved; no reset or commit.

## Visual comparisons and necessary adaptations

Local, git-ignored evidence: [`ui-evidence/b02c/`](ui-evidence/b02c/). All matter content is synthetic.

| Design source | Rendered implementation |
|---|---|
| [Supplied 06D PNG](ui-evidence/b02c/reference-06D.png) | [06D 1440×900 light/100%](ui-evidence/b02c/06D-1440-light-100.png) |
| Resolved `screens/06A.json` (no supplied PNG) | [06A 1440×900 light/100%](ui-evidence/b02c/06A-1440-light-100.png) |
| 06A compact/theme/scale adaptation | [06A matrix](ui-evidence/b02c/06A-matrix.png) |
| 06D compact/theme/scale adaptation | [06D matrix](ui-evidence/b02c/06D-matrix.png), [compact keyboard source drawer](ui-evidence/b02c/06D-compact-source-keyboard.png) |

Captured **32 matrix screenshots**: both screens at 1440×900 and 1024×768, light/dark, 90/100/110/125%, plus the compact keyboard drawer. Reference-size, compact dark/125% images and both contact sheets were visually inspected. Source panels match the reference bounds. The runtime batch toolbar is 50px rather than the drawn 48px to accommodate its real 36px form controls and border/padding; the table consequently starts at y286 rather than y284. The title/header, compact table hierarchy and 66px bottom save bar retain the new composition.

Real filenames/counts, current values, extraction evidence and uncertainty replace reference fixtures. Title and date editors sit side by side so both independently editable fields fit the source panel. The actual PDF keeps its aspect ratio inside a scrollable 238px viewport and can enlarge to 200% width; it is not stretched or replaced with a decorative page. Native date inputs follow OS locale while persisted/API dates remain ISO. Source/current evidence and status notices need additional runtime text; notices shift content rather than cover it.

At compact widths the source becomes a keyboard-accessible drawer, with Escape/focus return. Batch columns scroll horizontally, toolbars wrap/scroll, and the review workspace scrolls vertically at larger scales. Required acceptance/save actions remain in the viewport; source fields can receive keyboard focus and scroll into view. These adaptations do not preserve the legacy primary layout.

## Commands and results

| Command actually run | Final result |
|---|---|
| `cd backend && .venv312/bin/python -m pytest -q` | **296 passed**, six existing dependency/deprecation warnings, 56.21s. |
| `cd frontend && npm run build && npm run test` | Production build passed; **44 unit tests passed**. Production compilation repeated successfully in the final native builds. Existing Vite chunk-size warning remains. |
| Scoped Playwright: `npm run test:e2e -- metadata-suggestions.spec.ts journey.spec.ts --grep 'B02C|document table preserves|label proposal preview' --output=/private/tmp/courtbundle-b02c-sixth` (isolated backend 8779) | **5 passed** at that checkpoint (three B02C and two table regression cases). Two additional B02C regression cases were then added and passed in the final full run below. |
| `COURTBUNDLE_E2E_VITE_PORT=1441 npm run test:e2e -- --output=/private/tmp/courtbundle-b02c-final` | **107 passed, 1 skipped**, 6.5 minutes. All five final B02C cases passed. Skip is the opt-in protected-vault scenario. |
| `backend/.venv312/bin/python backend/scripts/build_sidecar.py` | Final matching **aarch64-apple-darwin** sidecar built successfully. |
| `npm run tauri:build -- --bundles app` | Final Apple Silicon `CourtBundle POC.app` built, **69.79 MiB**. |
| `npm run tauri:build -- --debug --bundles app --config /private/tmp/courtbundle-b02c-native-config.json` | Final isolated `CourtBundle B02C QA.app` built, **36.73 MiB**. |
| `git diff --check` | Passed. |

Tests cover exact-field writes, unselected/dismissed proposals, ambiguous/invalid/missing dates, unavailable evidence, page selection, reopen, history/Undo, stale snapshots and source changes, transaction rollback/retry, group-only stable sorting, delayed source responses, retained unselected drafts, dirty guards, keyboard/focus, engine invalidation, actual regenerated output and original/prior hashes.

Earlier failures were corrected before final acceptance: inherited input heights and shell scale handling, a Vue transition needing one component root, same-matter navigation bypassing a dirty guard, row-menu clipping, legacy text assertions now needing input-value assertions, and delayed parent refresh incorrectly flagging our own accepted save as stale. The first full browser run's 105 pass/1 fail/1 skip is superseded by the final full passing run. No application-source changes followed that final run.

## Native reconciliation and boundaries

Native testing used the unique QA identity `io.courtbundle.desktop.b02cqa`, packaged sidecar on **127.0.0.1:8781**, and `/private/tmp/courtbundle-b02c-native-store`. The user's existing engine on 8765 and real matters were not used. Both isolated QA processes were stopped after verification.

Synthetic matter **05191f0d2bec4d44b4b9768fa6d157cc** contains two one-page documents. Native title-only acceptance saved **NATIVE ACCEPTED ORDER** while leaving its date blank and the entire second document unchanged. Separate date acceptance saved **2026-01-12** and made the old build stale. Native rebuild **98c87db147234ac2a65ef8d5ba916305** produced three physical pages. Its PDF index contains the accepted title/date; outline and index navigation reach source content at physical page 2. The generated reader rendered the actual PDF. Independent parsing reconciled the title, date, outline, links and CSV.

The macOS system Save dialog passed **Cancel → retry → successful `index.csv` Save**. The saved CSV's SHA-256 equals the frozen manifest:

`2902150b48a5c5286df882555a69e4f0a55bef87f897879ed414b1ab1d148982`

This phase verified native **CSV Save** and generated PDF review; it does not claim a new native PDF Save test. Native PDF Save has earlier B01/B02 evidence. All nine tracked original/prior files for build **1a8fee29a8004a2fb807b26bf653514f** remain unchanged.

After rebuilding the final matching sidecar and app, restart showed the persisted title and confirmed date. A further title-only acceptance to **NATIVE ACCEPTED ORDER FINAL** passed without the false stale banner; the date and second document remained unchanged. This final deliberate metadata change leaves the previously regenerated artifact stale, as expected; that artifact retains its original **NATIVE ACCEPTED ORDER** contents. The final UI was visually inspected. Evidence: [native reconciliation](ui-evidence/b02c/native-reconciliation.json), [generated index](ui-evidence/b02c/native-index.png), [native saved CSV](ui-evidence/b02c/native-saved-index.csv), [verification inventory](ui-evidence/b02c/verification.json) and retained logs.

Windows, Intel Mac, minimum-version/clean-device installation, packaged-vault startup, native print/Office Save and native transport-failure injection were not verified in B02C. Browser save failure/retry and native cancellation/retry were verified. Local extraction intentionally covers a limited set of unambiguous date formats; other wording/languages require manual review. Accepted evidence records are subject to existing history retention, not a separate permanent legal audit service. No court-compliance, authenticity, OCR or signature approval is claimed.

No remaining failed scoped acceptance check. **B03A is the next bounded task; it was not started.**
