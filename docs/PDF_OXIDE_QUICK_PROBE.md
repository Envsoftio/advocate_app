# PDF Oxide capability probe — 17 November 2026

Pinned Rust `pdf_oxide` 0.3.78; local Apple Silicon host. Production routes unchanged. This is a four-operation synthetic probe, not complete capability/platform acceptance.

Reproduce:

```sh
cargo build --locked --offline --manifest-path backend/pdf-oxide-worker/Cargo.toml --example capability_probe
backend/.venv312/bin/python backend/scripts/probe_oxide_capabilities.py
```

The runner prints an artifact directory and JSON results. It creates a three-page synthetic PDF with simple Helvetica text, two outlines, B-prefixed page labels and an internal link from page 1 to page 3. Rust alone performs the four candidate edits. Existing PyMuPDF is used only to create the fixture and independently inspect/render outputs, not as a new production workload. Poppler/reportlab were unavailable; rendered PNGs were inspected using the existing independent renderer.

| Operation | Observed result | Conclusion |
| --- | --- | --- |
| Rotate page 1 by 90 degrees | Text, outline destinations, internal link target and page labels retained; rotation metadata and rendered orientation correct | Pass for this fixture; link rectangle hit-testing not qualified |
| Reverse three pages | Text order reversed; outline destinations remapped; original page-1 link now on page 3 points to page 1 | Pass for this fixture; page labels remain positional B-1/B-2/B-3, not source identity |
| Append same document | Six pages and text correct; appended page-4 link points to page 1 (zero-based 0), expected page 6 (zero-based 5) | **FAIL: direct `merge_from` does not meet our navigation contract on this fixture.** Only original two outlines retained; appended outline policy also needs explicit handling |
| Destructive full-page text redaction | 13 glyphs reported removed; target text absent from independent extraction and decompressed streams; first page renders black; other-page text retained | Narrow simple-font text pass only. No image/vector/Type0/Unicode/attachment/metadata/orphan-object security qualification |

All input hashes remained unchanged. Rotation, reversed first page and black redaction output PNGs were visually inspected. Raw results retained in `oxide-probe/results.json`.

The merge result blocks directly routing MatterDraft page assembly to this API. It does not establish that a safe Rust adapter cannot be built: reconstructing links/outlines from our authoritative mapping may address it, but that needs regression evidence. Do not silently accept the wrong destination or remove navigation.

Not tested here: Oxide rasterizer, Hindi shaping, OCR-layer insertion, partial-region/mixed-content redaction, editable marks, password/permission behavior, compression, corruption recovery, repeated edits, performance, Windows or Intel Mac. Source PDF metadata and every original object were not comprehensively audited after redaction. No claim of confidential-filing safety follows.
