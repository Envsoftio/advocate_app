# DEC-02 synthetic engineering reference

**Technical review:** approved by Codex for repeatable engineering tests on 13 October 2026, following the product owner's request for provisional review. **Legal review:** pending an advocate. This reference is not a filing rule, court-approved template or deliverable for a real case.

## Scope and unresolved legal choices

The profile is `delhi-hc-illustrative`. The example is a fictional, offline filing-preparation exercise using only `samples/` files. It has no selected Delhi High Court bench/side, jurisdiction, real case type, filing stage or filing route. No required-document checklist is asserted. Parts 1 and 2 below are engineering tags; Parts 3 and 4 are deliberately empty because their legal meaning has not been specified. The 6-page volume limit, including index pages, is a stress fixture for the allocation engine and is not a court page limit. The existing profile's byte cap is a separate technical setting and has not been accepted for any case-specific filing.

## Reproducible inputs and expected output

Run `backend/.venv312/bin/python backend/scripts/build_demo_output.py` from the repository root. The script creates a temporary local matter, imports four synthetic files, applies English OCR to the scan, assigns the tags below and writes sample output to `samples/example_output/`.

| Input | Engineering part | Type | Label | Volume pages | Combined pages |
| --- | ---: | --- | --- | --- | --- |
| `01_petition.pdf` | 1 | petition | blank | 2–4 | 2–4 |
| `02_impugned_order.pdf` | 1 | order | blank | 5–6 | 5–6 |
| `03_affidavit.pdf` | 2 | affidavit | blank | 2–3 | 8–9 |
| `04_scanned_annexure.jpg` | 2 | annexure | P-1 | 4 | 10 |

Each volume starts with a one-page generated index. Volume 1 has six pages; Volume 2 has four; `combined.pdf` has ten. The ZIP inventory is `combined.pdf`, `volume-01.pdf`, `volume-02.pdf`, `index.csv`, `validation-report.json` and `READ_ME_FIRST.txt`. Current CSV columns are `volume,part,kind,label,title,start_page,end_page,total_pages`. The index links and bookmarks should point to the same deterministic page-map destinations. These are technical expected results for this fixture only.

## Advocate review required before DEC-02 closes

The advocate must choose the exact court/bench/side, case type, stage and route; define Parts 1–4, required and conditional documents, approved index columns/templates, group numbering, separators/covers, pagination, byte/page policies and oversized-document handling; then review and approve a separate synthetic reference matter and expected outputs for that scope. Until then, downstream FIL-01/02/03/07 work may use this fixture to test mechanics but may not present its tags or outputs as legally approved.
