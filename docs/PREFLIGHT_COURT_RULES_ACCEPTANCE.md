# Court-rule validation: inputs and acceptance needed

17 November 2026. This is the completion checklist for **DEC-02 / FIL-07**, not an approved rule pack. Automated page screening and input checks are independent of this legal-workflow decision.

## Research delivered and remaining acceptance

The user authorized research for the Supreme Court of India, Delhi High Court, and Punjab and Haryana High Court and explicitly has no advocate reviewer. Draft reference profiles and selectable official-source guidance are implemented for these courts. This is not waiting for the user to repeat that information. A specific proceeding's conditional requirements and professional approval remain unavailable; no nationwide default or inferred approval is permitted.

Official sources checked on 17 November 2026:

- [Supreme Court e-filing services](https://www.sci.gov.in/e-filing/) and [SC-eFM 3.0 FAQ](https://efiling3.sci.gov.in/resources/FAQ): PDF/per-file upload guidance and links to procedural rules. The 50 MB portal statement is not applied as an all-artifact/ZIP cap. The January 2025 scrutiny-checklist PDF timed out, so its detailed requirements are not encoded.
- [Delhi notification No. 6/Rules/DHC](https://delhihighcourt.nic.in/files/2026-02/notifications-and-practice-directions/12_0.pdf) and [IT Cell implementation notice](https://www.delhihighcourt.nic.in/files/2026-02/announcements/16it.pdf): the existing 300 MB High Court reference was corroborated. This does not approve the rest of the inherited illustrative profile; older source-review dates remain visible.
- [Punjab and Haryana notifications index](https://www.highcourtchd.gov.in/?mod=noti_giudelines) lists the September 2026 software change and earlier guidelines/checklists. The index was accessible through a search-indexed route, but direct detailed notice/checklist downloads were unavailable. No upload cap or conditional document rules were guessed.

All three profiles carry a research version and explicit `not_reviewed` approval metadata. Profile content already participates in input freshness and new build manifests. Reference browsing does not silently change a matter's saved profile.

## Required inputs for legal-completeness validation

| Field | Current evidence | Required input |
| --- | --- | --- |
| Court | Research authorized for Supreme Court, Delhi High Court, Punjab and Haryana High Court | Select exact court for each proceeding |
| Bench / jurisdiction / side | Not specified | Exact applicable jurisdiction and side |
| Case type | No approved type | Exact type and subcategory |
| Filing stage | Not specified | Initial filing, refiling, application, appeal, etc. |
| Filing route | Not specified | Applicable online/designated-centre/other route |
| Advocate reviewer | User explicitly has no reviewer | Professional review remains unavailable; never infer approval |
| Applicable rules | Source-linked illustrative technical profile exists | Exact official documents/clauses and amendments applicable to the chosen scope |
| Effective and review dates | Existing profile metadata is not approval | Effective dates, review date, next review/expiry policy |
| Required documents | No approved checklist | Required and conditional items, accepted alternatives and exceptions |
| Conditional facts | No approved predicates | Facts that trigger each item; unknown facts must remain unresolved |
| Evidence of approval | None | Written reviewer acceptance of this version and synthetic expected outputs |

An advocate's approval is a review record; it is not an endorsement by a court. Screens, reports and profile names must preserve that distinction. The existing `delhi-hc-illustrative` profile remains unapproved.

## Rule-by-rule review record

For each proposed requirement, provide all of:

1. Stable rule identifier, title and plain-language explanation.
2. Exact applicability: court/side/case type/stage/route and relevant exceptions.
3. Official source URL, document title, clause/page, publication and effective dates, amendment/supersession relationships, and captured source identity/hash.
4. Required evidence: document type or explicit reviewer finding; whether alternatives or multiple documents satisfy the requirement. Filenames alone do not establish compliance.
5. Condition and facts: values allowed, meaning of unknown/not-applicable, and the source of each matter-specific fact.
6. Result and remedy: missing, satisfied by recorded evidence, not applicable, unknown, or pending review. Technical blockers and legal-review warnings must remain distinguishable.
7. Reviewer, review date, decision, rationale, version, scope and review expiry. Draft/rejected/expired versions must not be presented as approved.

## Implementation after the scope and rule evidence are supplied

- Version the reviewed profile without rewriting the illustrative pack or previously generated manifests.
- Validate schema and exact scope at import/application; reject mismatched or incomplete metadata.
- Add explicit matter facts and document-evidence assignments, with unknown states and no automatic truth from OCR or filenames.
- Evaluate conditional requirements locally and produce explainable source-linked findings, including why a rule applies or cannot yet be evaluated.
- Bind findings and human review to the precise profile version, facts, document revisions and input digest. Changes invalidate current applicability; prior evidence remains historical.
- Carry the selected version and provenance through build/review/export and portable matter handling, with fail-closed behavior when the referenced profile/evidence is unavailable.
- Retain the existing deterministic page map; any newly approved index/numbering/volume rule requires its own engine regression fixtures.

## Completion evidence

The reviewer supplies and accepts a synthetic reference matter with expected PDF/CSV/ZIP outputs. Tests must cover:

- Required document present and missing; alternative evidence and duplicates.
- Each conditional branch, explicit not-applicable and unknown facts.
- Wrong court/side/type/stage/route, future/expired/superseded profiles and conflicting sources.
- Profile edits, changed source revisions, changed facts, membership and reviewer rejection invalidating prior status.
- Restart/restore persistence, immutable originals/prior builds and source-linked explanations in UI/reports.
- Actual supported-device import → review → build → inspect → Save; native dialogs and printed/page-map reconciliation are tested rather than inferred from compilation.

Until these inputs and checks exist, legal-completeness validation remains open. Technical screening can identify candidates and prevent processing errors; it cannot close this acceptance gate.
