# Rust backend migration audit

17 November 2026. Owner requests Rust backend and PDF Oxide while preserving existing PDF capabilities. This audit does not authorize claiming zero regressions or removing unqualified legacy paths.

## Findings from current code

The existing Rust Tauri host launches the Python `filing-api` sidecar and owns native Save. Python still owns HTTP authentication/routes, SQLCipher/SQLite, working revisions, vault files, jobs, backups and the bundle planner. The opt-in Rust worker replaces only Office-draft text extraction. A Rust PDF worker alone is not a Rust backend migration.

### Ranked PDF replacements

| Order | Actual call sites | Risk and qualification before routing |
| --- | --- | --- |
| 1 | `office_export.py:_pdf_pages` | Existing opt-in slice. Read-only, but sorted text/reading order and blank-page behavior need reviewed fixtures. Keep 2,000-page draft limits scoped to this operation. |
| 2 | `main.py:search_matter`, `search_document` | Best next user-visible slice: only text and page numbers, no geometry. Preserve query validation, revision checks, locks, snippets, case folding, 100-result truncation and errors. Current Rust worker's 2,000-page/256 MiB limits cannot silently become search limits. Use page-streamed extraction and test large documents before switching. |
| 3 | `labels.py:extract_label_references` | Read-only candidate discovery. Preserve regex, page/revision evidence, searchable-page count and 200-result cap. Reading-order changes can change candidate matching; compare actual expected references. |
| 4 | `metadata_review.py:proposals` | Selected-page date evidence, not just text: extraction feeds evidence hashes and later acceptance checks. Add single-page extraction, preserve the 40,000-character bound, and explicitly invalidate old proposals on extractor changes. Do not rebind old evidence silently. |
| 5 | `main.py` thumbnail rendering, `metadata_review.py` source preview | Isolated image outputs, but require rendering features and pixel/geometry comparisons: rotations, CropBox/MediaBox, fonts, image masks, annotations, malformed PDFs and memory limits. Keep PDF.js interactive rendering unchanged. |
| 6 | `pdf_ops.py` extraction/split, then page edits | Outputs may lose links, outlines, labels, widgets, metadata or annotations. Even rotate/crop is a PDF rewrite. Must use existing revision transaction, signed-input guard and original/prior hash checks. Not an easy first switch. |
| 7 | `editable_marks.py`, OCR, text editing, print and compression | High risk: coordinates, font metrics, raster fidelity, mixed native/OCR text, navigation retention, private-note omission and undo history. Qualify individual operations independently. |
| 8 | `engine.py`, `validation.py`, redaction and signature inspection | Last: authoritative map, generated indexes/labels/bookmarks/links, independent validation and security-sensitive refusal behavior. Do not replace writer and independent verifier together. |

`output_processing.py:content_contract` is not a safe first text-extraction swap: it compares text, geometry, labels, outlines and links before/after processing. A different extractor can change the validation contract or hide a writer defect.

## Measured local comparison

Ran the compiled pinned 0.3.78 Rust worker against the six root `samples/*.pdf` files, compared page counts and whitespace-normalized text with the current PyMuPDF reader, and checked source hashes after extraction.

| Fixture | Pages | Matching normalized pages |
| --- | ---: | ---: |
| 01_petition.pdf | 3 | 3 |
| 02_impugned_order.pdf | 2 | 2 |
| 03_affidavit.pdf | 2 | 2 |
| 05_hindi_scan.pdf | 1 | 1 |
| 06_hindi_mixed.pdf | 2 | 2 |
| 07_hindi_poor_scan.pdf | 1 | 1 |

All original hashes were unchanged. Raw strings differ in whitespace; exact string parity is not established. Both readers return empty text for the two image-only Hindi PDFs, so this is not OCR qualification. No conclusions about complex tables, multi-column reading order, actual Hindi OCR layers, encryption permissions, render fidelity or malformed recovery follow from this small corpus. Existing 16 Oxide-enabled tests and 366 backend checks are recorded separately in the acceptance log; they were not rerun for this documentation audit.

## Rust backend destination and migration sequence

1. Extract the worker implementation into a UI-independent Rust library with a versioned operation protocol, typed errors, capability/version reporting, page-scoped extraction and bounded output. Keep the executable as a process-isolation adapter. Fix worker hardening before wider exposure: input/output are currently buffered, stderr is captured, and the Python response decoder assumes an object. Add malformed-response, timeout and limit tests. No automatic engine retry for mutations.
2. Qualify read-only search and reference extraction behind separate operation flags. Compare synthetic outputs through the existing endpoint contracts. Keep Rust free of UI state and preserve current revision ownership in Python during this stage.
3. Introduce a Rust local HTTP service, with Axum as a proposed implementation choice, retaining existing route/JSON/error/range-download contracts. Validate loopback binding, random session token, readiness, CORS, streaming, process exit and cancellation. Route qualified operations to Rust; proxy unmigrated operations to Python over an authenticated private connection. Do not expose two competing public services or rewrite Vue and backend simultaneously.
4. Port storage/vault compatibility as a separate project: exact SQLCipher settings, schema migrations, encrypted file `CBENC1` format, scrypt/AES-GCM key envelopes, recovery/rotation, backup compatibility, atomic writes and file locks. Test old data round trips and crash recovery on synthetic stores. Until qualified, Python remains the only writer. After cutover, Rust owns writes; remaining Python PDF workers receive bytes and return candidates, not database access.
5. Port persistent jobs/history and revision publication under that single owner. Verify idempotency, restart/retry snapshots, cancellation, stale responses, undo/redo and rollback before removing Python ownership.
6. Port the deterministic planner into pure Rust only after golden fixtures compare every field: volume/index growth, split ranges, numbering, source offsets, bookmarks, supplemental and audience variants. Rust then becomes the single authority. Do not independently derive offsets in both runtimes.
7. Migrate PDF writers operation by operation. Validate candidate outputs before atomic publication. Retain explicit unsupported states. Tesseract can remain a local native executable; Python removal does not require changing OCR recognition simultaneously. Package matching Rust/Tesseract resources per supported target.
8. Remove Python/PyInstaller/PyMuPDF only when no production path needs them and full native/platform, vault, recovery and output acceptance passes. Keep prior-version rollback compatible with unchanged schemas or provide tested restore recovery.

## Recommended next bounded ticket

Worker hardening + page-scoped text protocol + optional search integration. Acceptance: existing endpoint behavior and synthetic phrase/case/Unicode/multi-page/scan/revision fixtures; protected-store and timeout/worker failure tests; large-document limits no stricter than current supported behavior; actual native worker discovery/startup/quit. Do not expand PDF-03/06/07 into production on the strength of extraction tests.

Source inspected: local pinned `pdf_oxide-0.3.78/src/document.rs` explains that `extract_text` uses row-aware assembly and artifact filtering, while the full plain-text pipeline handles complex reading order differently. Upstream reference: https://docs.rs/pdf_oxide/0.3.78/pdf_oxide/ . Published capabilities are candidates, not app acceptance evidence.


### Next-ticket progress

Protocol hardening, single-page extraction and separately opt-in search are now implemented; see `PDF_OXIDE_MIGRATION.md`. Search retains whole-document pilot limits, so large-document streaming and native discovery/startup/quit acceptance are still open. This does not complete the full recommended ticket or move database/job ownership to Rust.


Page-streamed search now supersedes the whole-document output pilot: protocol 2 has sequential bounded records, completion verification and early-close process cleanup. The 2,001-page fixture passes. Input PDF buffering, 256 MiB/60-second/per-page limits and native packaging qualification remain open; see the migration document for current details.

### 18 November 2026 general-PDF admission checkpoint

[Certain] The [synthetic corpus](../samples/rust_pdf_corpus/README.md) provides real CMS-signed, encrypted, malformed, visible-mark and unsigned controls with SHA-256 and independent OpenSSL/PyMuPDF output. Every file remains refused by the Rust exact-byte gate. This closes only the missing fixture-evidence subtask. `pdf_oxide/signatures` offline build fails on missing `cms`; no replacement inspector was introduced. Tauri JSON byte-array and parser memory bounds, disk-full/power-loss, Windows directory flush/rename and target-device checks remain blockers. Keep Python as default/reference, the deterministic Python planner and vault boundaries, and `workflow_ready=false`.

Tauri PDF byte arguments now use bounded Serde sequence deserialization (4 MiB import; 16 MiB extraction and job start). This narrows one exposed allocation path, but does not cap raw Tauri IPC buffering or PDF Oxide parser allocations. Broader PDF admission remains disabled. The next scoped Python-to-Rust mapping is [document archive/restore and presence-history parity](PROMPT_RUST_DOCUMENT_LIFECYCLE_PARITY.md).

## Python backend module map

The [route inventory](../rust-backend/tests/python_route_inventory.py) accounts for every Python API handler, including routes with both GET and HEAD. Coverage labels describe implementation or scoped evidence. The module map follows the supporting behavior behind those routes.

| Python module | Rust implementation |
| --- | --- |
| `main.py`, `schemas.py`, `environment.py` | Tauri `rust_backend.rs`, core `matters.rs`, route-local validation and runtime configuration |
| `launcher.py`, `dev.sh`, `scripts/build_sidecar.py`, `scripts/smoke_packaged_pdf.py` | Tauri selects and owns the linked Rust runtime. `dev-rust.sh` and the qualification bundle select the Rust store. `scripts/build_rust_ocr.py`, `scripts/stage_rust_ocr.py` and Tauri `build.rs` supply target-qualified OCR resources. The launcher and sidecar scripts continue to serve the Python reference. |
| `storage.py`, `review_storage.py` | `matters.rs`, `preparation`, `preflight`, `bundle_inputs`, `document_metadata`, `revisions`, `generated_exports` |
| `file_io.py`, `secure_db.py`, `vault.py`, `vault_store.py` | `artifacts`, `secure_db.rs`, `vault.rs`, `vault_store.rs`, Tauri `native_save.rs` |
| `portable.py` | `portable_matter`, native file picker and streamed Save/restore |
| `jobs.py` | `jobs.rs`, `processing_jobs`, writer queue in Tauri `rust_backend.rs` |
| `signatures.py`, `oxide_pdf.py` | `admission.rs`, direct PDF Oxide reads and signature guards. The optional Python worker and its subprocess protocol have no Rust runtime dependency. |
| `pdf_search.py`, `pdf_stream.py` | `reading`, `preview`, `artifacts`, bounded binary IPC reads |
| `pdf_ops.py`, `editable_marks.py`, `print_export.py` | `page_tools`, `annotations`, `text_edit`, `document_output`, `redaction_records`, `redaction_vectors`, `ocr`, `tesseract`, `compression` |
| `paragraph_trial.py` | `text_edit`. Native PDF Oxide inspection, font encoding, bounded reflow and post-edit checks. Composite CMaps, arbitrary substitute glyph coverage and full engine fidelity still require qualification. |
| `page_analysis.py` | `ocr`, `preflight`, `preparation/validation` |
| `build_inputs.py`, `engine.py`, `validation.py`, `rules/profiles.json` | `bundle_inputs`, `planner.rs`, `bundle_writer`, `output_navigation`, `preflight`, `preparation/validation`, shared profile data |
| `labels.py`, `metadata_review.py` | `labels`, `metadata_review`, `document_metadata` |
| `variants.py`, `supplements.py` | `compositions`, `preserved_labels`, `bundle_writer` |
| `output_processing.py` | `generated_exports`, `compression`, `document_output`, `export_comparison` |
| `office_export.py` | `office_drafts` |

The normal desktop call paths also include `frontend/src/lib/api.ts`, `rustTransport.ts`, `rustPdf.ts`, and Tauri `native_save.rs`. The adapters map Python route and response contracts onto direct requests, one-use file-picker tokens, bounded binary ranges, native export and detached restore. Browser parity uses separate synthetic stores and compares the application-level effects. The package, native input and filesystem checks use the actual desktop entry point.

## Shaped editing, graphics redaction and OCR restore — 19 November 2026

Rust now positions shaped glyphs from supported embedded fonts and preserves searchable text through a second paragraph edit. The redaction path removes selected paint from curved fills, strokes, tiling patterns and axial shadings on the exercised corpus. Python backup restore accepts Rust OCR provenance and preserves the provider and model hashes. Detached vault conversion exercises an interrupted job with its frozen source and keeps a missing-snapshot job visible with its retry blocker. Native acceptance and broader PDF fidelity remain open. [Verification and limits](ACCEPTANCE_TESTS.md#rust-shaped-text-graphics-redaction-and-backup-interoperability--19-november-2026).

The later redaction checks cover clipping paths shared with subsequent paint, active pattern use by searchable text, soft and indexed image masks, and inline and named accessibility text. Lossless compression now retains the source PDF's navigation and catalog through a reachable-object copy. [Shared-resource and compression checks](ACCEPTANCE_TESTS.md#rust-shared-pdf-resources-and-lossless-compression--19-november-2026).
