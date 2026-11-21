# Incremental PDF Oxide migration

17 November 2026 technical history. New PDF work uses Rust `pdf_oxide`. The 18 November owner decision changes delivery to a [single shipped PDF cutover](RUST_PDF_SINGLE_CUTOVER_PLAN.md) after complete Rust parity. The operation order below remains useful for implementation and verification, but it no longer authorizes separate production routing switches. No zero-regression guarantee is inferred from a successful compilation.

## Delivered first slice

`backend/pdf-oxide-worker` pins PDF Oxide 0.3.78 and commits Cargo.lock. A read-only stdin/stdout worker extracts page text for existing DOCX/PPTX review drafts. It does not write PDF files, alter originals, or recalculate the deterministic page map. XLSX continues to use the authoritative CSV.

Opt in locally after `cargo build --locked --manifest-path backend/pdf-oxide-worker/Cargo.toml` by setting `MATTERDRAFT_PDF_OXIDE_WORKER` to the absolute `target/debug/matterdraft-pdf-oxide` executable path before starting the backend. Unset it to return to the existing extraction path. This is explicit migration configuration, not automatic fallback: an opted-in worker failure fails the draft. No UI change or native package integration is claimed.

Encrypted-store input uses the existing vault reader and stdin bytes, with no plaintext scratch PDF. Worker errors suppress parser details; a 60-second subprocess timeout, 256 MiB input limit, 2,000-page limit and 10-million-character limit bound this slice. These are not OS memory quotas; both the PDF input and JSON response are buffered. Large-input memory qualification remains necessary.

## Ordered remaining work

1. **PDF-08:** qualify extraction reading order, fonts/Unicode, scanned/mixed pages and draft content on a reviewed corpus; package the matching worker on supported targets. Only then change the default. Evaluate PDF-to-image rendering separately; Office layout reconstruction/import remains unimplemented.
2. **PDF-06:** qualify Oxide rasterization on rotated/mixed English/Hindi fixtures, retain Tesseract recognition and model provenance, then qualify searchable-layer insertion. Scanner acquisition and deskew are separate unfinished features. No network/model download during processing.
3. **PDF-03:** migrate editable marks/annotations using exact revision coordinates and existing export/privacy rules. Verify save/reopen, rotated geometry, Unicode, originals and prior-export hashes before routing production writes.
4. **PDF-07:** reviewed bounded text replacement fixtures first, including unsupported fonts, collisions, extraction order and repeated edits. Preserve refusal behavior and revision transactions. Scanned-pixel edits and OCR correction must remain distinct.
5. Migrate page operations, compression, inspection/validation, print and bundle writing individually. Keep `engine.py` authoritative for indexes, links, labels, volumes and offsets. Independently inspect redaction before changing that path.
6. Remove PyMuPDF only after every shipped dependency path is replaced and platform, integrity, licensing and native acceptance gates pass.

PDF-03/06/07/08 remain open. The first slice does not qualify rendering, writing, OCR or redaction. Existing production PyMuPDF processing remains in place. Upstream API reference: https://docs.rs/pdf_oxide/0.3.78/pdf_oxide/ ; upstream feature claims are not MatterDraft acceptance evidence.


## Protocol hardening and optional search — 17 November 2026

The worker now accepts `--page N` (zero-based) to extract a single page; JSON includes `page_count` and `start_page`, validated by the bridge. Out-of-range pages fail. Whole-document Office mode retains its 2,000-page limit; selected-page mode does not allocate a page-count-sized results array.

Python now drains stdout with a 64 MiB cap, discards worker stderr, terminates timed-out/oversized responses, joins pipe threads and rejects malformed/non-object JSON. Decrypted PDF input remains in memory; parser memory is not OS-limited. This is subprocess/protocol hardening, not a sandbox or full memory qualification.

Set `MATTERDRAFT_PDF_OXIDE_SEARCH=1` alongside the absolute worker executable variable to opt into Rust extraction for matter/document search. Query matching, snippets, authentication, revision checking and 100-result caps retain their existing Python endpoint contracts. No database/page-map ownership changed. Worker errors fail search without silent fallback. Default search remains unchanged.

At this checkpoint search still used whole-document extraction. The page-streaming implementation below supersedes that limitation; native worker packaging/discovery/quit remains pending.


## Page-streamed search — 17 November 2026

Optional search now invokes `--stream`: protocol 2 uses newline-delimited JSON with a versioned header, sequential zero-based page records and a required end record. The reader validates record types/order/count, bounded frame length, completion and exit status. Missing/truncated/out-of-order output fails the request rather than returning complete-looking partial results. Existing endpoint early return at 100 results intentionally closes and reaps the worker without consuming the remainder.

Rust flushes each page and does not retain the whole document's extracted text. Python feeds decrypted input chunks from the existing vault boundary, reads records on demand and cleans up worker/timer/feed thread on normal completion, early return, malformed output or timeout. No plaintext temporary files. The 2,000-page Office cap and aggregate 10-million-character draft cap no longer apply to search. A 2,001-page synthetic blank PDF verifies the page-count boundary, not production performance.

Remaining limits: 256 MiB PDF input, 60-second operation deadline, 10-million UTF-8-byte text limit per search page and 64 MiB maximum JSON frame. Rust still buffers the input PDF and PDF Oxide can retain internal caches; this is streaming extracted text, not bounded total parser memory. These explicit pilot limits mean default search is still unchanged. Office exports retain protocol 1 and their existing limits. Tesseract/PDF writers, page-map rules and database ownership are untouched.

Native bundle integration and supported-platform worker lifecycle remain the next qualification stage. No claim of native export or full Rust-backend completion follows from these tests.


## Packaging implementation — 17 November 2026

`backend/scripts/build_sidecar.py` now builds the pinned Rust worker with `--locked --release --target <host triple>` and embeds it under `pdf-tools` in the frozen backend. This keeps the existing Tauri sidecar launch/API contract intact during migration. The worker remains a separate Rust executable, not a Python PDF Oxide binding.

Packaged QA can set `MATTERDRAFT_PDF_OXIDE_BUNDLED=1` to resolve the worker from PyInstaller's extraction directory, plus `MATTERDRAFT_PDF_OXIDE_SEARCH=1` for Rust search. No absolute developer path is needed. The existing explicit worker path takes precedence. With neither worker selection option set, Office extraction and search defaults remain unchanged. Selecting bundled mode outside a frozen backend fails explicitly. Native UI launcher preferences and distribution signing remain separate work.

Reproducible service check: `backend/.venv312/bin/python backend/scripts/smoke_packaged_pdf.py frontend/src-tauri/binaries/filing-api-<host-triple> --protected --oxide` (append `.exe` on Windows). This uses a disposable encrypted synthetic store and a restricted PATH, clears developer-worker overrides and checks actual search/DOCX plus existing OCR/build/backup behavior. Run without `--oxide` to exercise the preserved default. See the acceptance log for executed results; a command here is not evidence that every platform passed.

Execution ownership and remaining gates: `RUST_BACKEND_PLAN.md`.
