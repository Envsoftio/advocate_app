# FLOW / 10 — generated review, export, storage and prior output

17 November 2026. Initial UI correction followed by the authorized PRD functionality implementation below. The initial correction left backend contracts unchanged; the follow-up adds revision/comparison/archive APIs and schema 29. Originals and immutable build bytes remain preserved.

## Design scope

The source is `CourtBundle_JSON_For_Codex/screens/{18A,18B,20A,19A,19B,18C}.json`, its resolved component definitions and tokens, and `implementation/previews/18A.png`. All six frames are 1440×900. The root `CourtBundle_Complete_Workflow (2).pen` was also inspected: it is an older revision, with “Save Generated Package” and “Stale Package” names and different stale/update compositions. The scoped JSON export supplied in the requested folder takes precedence, consistent with the repository design instructions. Illustration counts, local paths and preparation revision numbers are not runtime data.

| Screen | Implementation and audit result |
| --- | --- |
| 18A generated review | Real immutable PDF, saved build identity, outline, search, zoom, links and source navigation. The review temporarily uses the 64 px rail and 246 px outline composition; closing restores the user's sidebar preference. Details are available on demand. The unrelated working-document version picker no longer crowds standalone review. |
| 18B export | Saved file selector, measured size, real native/browser destination, failure/retry/cancellation and existing backend release gates. Human-review acknowledgement is distinct from current release eligibility: stale files can retain recorded human review while release is blocked. Retry checks is available after a validation-load failure. |
| 20A storage | Full-width category/table layout; keyboard-operable originals, current working revisions, saved builds and archived-document categories. Includes reference-only originals and remains usable without a completed build. Archived files are explicitly restored before opening. Generated rows show actual size, creation date and state. Review, export and change comparison retain the selected build. |
| 19A stale choices | Earlier/current summaries and existing rebuild, supplement and preserved-label policies. Insert policies only use saved plans for the selected baseline. Storage hands the chosen baseline into Build after the new workspace mounts, rather than reverting to the latest job. |
| 19B changes | Recorded revision/hash/membership/settings/map comparison with selected-document detail. Open working copy opens that row's current revision; storage comparison and return navigation work. |
| 18C prior output | Read-only earlier PDF, preserved identity/page numbers, stale notice, Save a copy and update action. Review-copy download and reviewed release remain separate. |

Storage's expanded shell uses the current shared 240 px rail rather than the older 184 px shell. Generated review follows the supplied compact rail. Native OS window decoration is not imitated. Additional reader controls, technical release safeguards and real synthetic PDF content are retained; no pixel-perfect claim is made.

## Functional defects corrected

- Source navigation previously ignored `source_start` in a frozen page-map run. Split documents and selected supplemental pages could open page 1 instead of the actual source page. Both source labels and navigation now use the recorded engine offset. Existing exact-revision checks still refuse changed/unavailable working copies. Unit cases cover run boundaries, noncontiguous selections, standalone volumes, front matter and legacy offsets; a real eight-page supplemental browser journey verifies the resulting source-page navigation.
- Storage forced its view to `storage`, making Review changes ineffective. Comparison now renders and returns correctly.
- Moving from storage comparison to update policy could lose the selected older build while the outgoing screen transitioned. The pending selection is matter-scoped and applied when the destination component mounts. A two-build browser test reproduces and verifies this case.
- Preserved-label planning could select a plan belonging to another baseline. Plans are now filtered against the selected artifact.
- Export used `can_release` as the human-review badge. A reviewed-but-stale build was wrongly labelled human-review pending. Persisted acknowledgement and release eligibility are now displayed independently.
- Standalone generated review automatically opened details and an unrelated document version selector. It now starts with the supplied reading composition; original review from Storage cannot offer another selected document's versions.
- A generic result-layout media rule reversed Storage's column proportions at 1024 px. Storage now retains its left category rail and usable document column, then stacks with two-column category controls in narrow containers.

## PRD audit boundaries

The audit covers the canonical handoff §2 desktop foundation and §6.2 rebuild/export review: actual generated bytes, explicit build identity, distinct source/printed/physical pages, included/reference-only documents, retained originals/prior outputs, stale states, separate technical/human review, and safe release/download behavior. No court approval, filing completion, signature verification or source-citation repair is inferred.

The historical-revision, generated-build archive and rendered comparison gaps identified in the initial audit are implemented in the follow-up below. Native reveal continues to open only explicitly saved external files; the encrypted vault has no internal plaintext output folder to reveal.

## Verification

- Configured backend `.venv312/bin/python -m pytest -q`: **356 passed**, six dependency warnings, **76.30s**. Log `/private/tmp/flow10-pytest.log`.
- Frontend production build, **51 unit tests**, formatting and whitespace checks pass. Two source-map navigation cases were added. Existing Vite chunk-size advisory remains.
- Main synthetic browser suite (`build-workflow`, `output-validation`, `supplement-plans`): **13 passed**, **1.6m**, `/private/tmp/flow10-browser-final.log`. The initial run had 12 passes and a missing Storage review-button failure; the button was corrected before this passing run.
- Added storage coverage for no-build categories, keyboard activation, reference-only originals, archive restore with original-byte equality, older-build selection and update-policy routing. Its initial run exposed the transition defect described above.
- Final affected-workflow run: **4 passed**, **52.6s**, `/private/tmp/flow10-verified.log`, including the new storage test, stale reviewed-output badge/release refusal, exact supplemental source page, read-only review and matching download hash. These overlap the main run: **14 distinct cases passed across runs**, not a full browser-suite claim.
- Viewport matrix for all six screens: 1440×900 and 1024×768, light/dark, 90/100/110/125% interface scales; additional Storage checks at 640×900. Inspected actual screenshots against the 18A PNG and the other frames' JSON hierarchy and geometry. The final compact Storage correction receives a separate rerun and screenshot inspection recorded in the acceptance log. Compact content remains scrollable and generated bookmarks overlay the PDF when necessary.
- Unsigned Apple Silicon Tauri app compilation passes with the existing matching sidecar. Backend source was not changed. Packaged-sidecar smoke with a disposable **encrypted** store passes English/Hindi OCR, compression, bookmarked export, original integrity and protected backup/restore/rotation. Logs `/private/tmp/flow10-native-final.log` and `/private/tmp/flow10-packaged.log`.

Native WebView and OS Save/Finder interactions, Windows, Intel Mac and clean-device installation were not exercised. Packaged service checks and browser download hashes do not replace those runtime checks. The installed app was not replaced; no commit or deployment occurred. Pre-existing document-state/acceptance edits and untracked PRD files were preserved.

Final local evidence: [generated review](ui-evidence/flow10/18A-review.png), [export dialog](ui-evidence/flow10/18B-export.png), [Storage](ui-evidence/flow10/20A-storage.png), [stale choices](ui-evidence/flow10/19A-stale.png), [change comparison](ui-evidence/flow10/19B-changes.png), [prior-output review](ui-evidence/flow10/18C-stale-review.png), [compact dark Storage](ui-evidence/flow10/20A-storage-1024-dark-125.png), and [narrow originals](ui-evidence/flow10/originals-640-light.png). Final capture replay passed 1/1 after the last CSS correction; all-screen light/dark evidence was recaptured.


## PRD functionality follow-up — 17 November 2026

The canonical PRD review/export requirements and supplied 20A/19B/ED45 designs exposed three missing workflows. These are now connected to persisted data:

- **Retained working revisions:** Storage → Working revisions → Revision history lists available retained PDFs, previews any page and restores the selected PDF as a new revision. Explicit acknowledgement and confirmation warn about reintroducing removed content. Revision/hash preconditions, the existing signature-risk gate, atomic working-copy commit and Undo are retained. Original and saved build bytes are untouched. Metadata and editable overlays are not historical snapshots; stale links/marks/redaction geometry require review rather than being silently rebound.
- **Generated-build archive:** Archive/Restore controls and an Archived builds category use schema 29 metadata. Both operations preserve artifacts, manifests, baseline identity and timestamps used for staleness. Archived output stays reviewable/downloadable as a copy but cannot be released until restored and otherwise eligible. Supplement baselines remain resolvable.
- **Rendered comparison:** Review changes → Compare PDF pages opens the real frozen combined-output page beside the exact current working revision, with separate source-page selectors, zoom, optional synchronized scrolling and bounded text differences. The prior page comes from the frozen source-range/combined-offset map. Optional numbering filtering removes only text matching the recorded label inside its recorded stamp region; PDFs and rendered-difference status remain unchanged. Invalid coordinates, unavailable/tampered output and changed revisions fail closed.
- **Review navigation and label changes:** Forward reading-position history and Return to index complement existing Back. Index destinations derive from the engine's frozen map and cover rule. Change summaries explicitly show old/new annexure labels.

The ED45 PNG supplies the two-PDF/side-differences composition. The implementation uses the current modal review shell and actual PDF text, with 1440×900 light and 1024×900 dark screenshots inspected. It does not claim pixel-identical ED45 editor chrome, automatic insertion alignment, semantic/legal interpretation, image-difference heatmaps or spatial text highlighting. Independent page selection makes changed pagination explicit. The comparison is a bounded page review rather than a whole-document diff engine. Portable `.cbmatter` backups still do not export the entire retained revision/build history; this change does not alter that backup contract.

Verification details and native runtime limits are recorded in the follow-up acceptance entry. Evidence: `docs/ui-evidence/flow10/comparison-1440-light.png`, `comparison-1024-dark.png`, `revision-history-dark.png`.
