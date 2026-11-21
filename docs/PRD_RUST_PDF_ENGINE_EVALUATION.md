# MatterDraft Studio — Cross-platform Rust PDF engine evaluation PRD

**Status:** Incremental Rust PDF Oxide migration authorized, 17 November 2026; production replacement not yet qualified  
**Recorded:** 16 November 2026  
**Purpose:** Evaluate a commercially distributable PDF processing backend that can be packaged with Tauri on macOS, Windows and Linux.

## 1. Authorized direction and qualification

Determine whether MatterDraft can replace its PyMuPDF dependency while preserving the current PDF features, document safeguards and deterministic outputs. “Native Rust” here means a Rust backend usable in a Tauri desktop package on all three operating systems. A Rust binding to a bundled native library is eligible; a pure Rust implementation is not required. Each operating system and architecture still needs its own build and runtime verification. Linux is an evaluation target, not a currently accepted MatterDraft release target.

The owner authorized systematic migration to Rust PDF Oxide on 17 November 2026, prioritizing PDF-03/06/07/08 and avoiding new PyMuPDF workloads. This supersedes the earlier research-only decision. It does not establish production readiness or resolve distribution licensing. The current PyMuPDF dependency remains subject to its AGPL or commercial terms until it is removed from every shipped path or appropriately licensed. See `PDF_OXIDE_MIGRATION.md` for delivered scope and remaining gates.

## 2. Current PyMuPDF responsibilities

The local Python sidecar uses PyMuPDF for PDF/password inspection and image-to-PDF conversion; page insertion, deletion, reorder, rotation, crop, extract and split; text and image placement; bounded existing-text edits; permanent redaction; image compression; page rendering for OCR and preview; searchable OCR-layer insertion; and saved-file validation. Bundle generation uses it to create covers, indexes, volume and combined PDFs, page numbers, PDF page labels, links, bookmarks and metadata. The authoritative page map is computed in `backend/app/engine.py`; a replacement must consume that map rather than independently derive offsets. See `backend/app/pdf_ops.py`, `backend/app/engine.py`, `backend/app/editable_marks.py`, `backend/app/validation.py` and `backend/app/print_export.py`.

Tesseract performs OCR recognition; PyMuPDF currently renders pages and incorporates the recognized text into PDF working copies. PDF.js renders the interactive Vue reader. The existing `pypdfium2` paragraph-edit trial already introduces PDFium inside the Python sidecar. A new engine must preserve imported originals and the current working-copy revision transaction.

## 3. Candidate assessment

| Candidate | Relevant published capabilities | Unresolved MatterDraft fit |
| --- | --- | --- |
| [PDF Oxide](https://github.com/yfedoseev/pdf_oxide) (Rust core; MIT or Apache-2.0) | Its [editing documentation](https://pdf.oxide.fyi/rust/docs/editing/overview) describes page operations, text edits, rendering, compression and destructive redaction. Its [releases](https://github.com/yfedoseev/pdf_oxide/releases) provide macOS, Windows and Linux binaries. | These are upstream claims, not MatterDraft test results. Verify PDF fidelity, API behavior, dependency notices, OCR model/runtime packaging, fonts, navigation and labels. Its [redaction documentation](https://pdf.oxide.fyi/rust/docs/editing/redaction) says destructive redaction refuses unsupported composite/Type0 fonts and distinguishes an older cosmetic flatten path. |
| [`pdfium-render`](https://docs.rs/pdfium-render/latest/pdfium_render/) plus PDFium (Rust binding MIT or Apache-2.0; PDFium permissive) | Rendering, text/image extraction, page copying, page-object edits and saving. PDFium is already used by a bounded MatterDraft feature. | The crate [does not ship PDFium](https://docs.rs/pdfium-render/latest/pdfium_render/#binding-to-pdfium). Package matching native libraries and notices for each target. Establish safe redaction, OCR-layer writing, page-label/bookmark creation and reliable existing-text edits; documented object removal alone is insufficient proof. |
| [`lopdf`](https://docs.rs/lopdf/latest/lopdf/) and [`pdf-writer`](https://docs.rs/pdf-writer/latest/pdf_writer/) | Permissively licensed Rust tools for PDF structure, extraction and/or writing. | Useful supporting crates for navigation, labels or generation, but not an independently demonstrated replacement for rendering, OCR integration and visual edits. |
| [`hayro`](https://docs.rs/hayro/latest/hayro/) | Pure Rust page rasterization. | Its documentation says rendering completeness and performance are still in progress. It is a rendering candidate, not an editor or OCR engine. |

No candidate is accepted as a drop-in replacement. The evaluation should first test PDF Oxide as the broadest Rust-first candidate, then compare a PDFium-based path where it fails or lacks an operation. Candidate versions and complete transitive licences must be pinned and reviewed at implementation time. Avoid substituting AGPL MuPDF or Ghostscript bindings as a licensing workaround.

## 4. Product and architecture requirements

1. Keep all core PDF processing local. No cloud dependency or matter-content upload is introduced by the replacement.
2. Preserve imported originals byte-for-byte. Edit a candidate working copy, validate it, then publish a new revision atomically; failed operations publish nothing. Preserve existing undo/redo and stale-export behavior.
3. Keep PDF processing independent of the Vue UI. A Rust sidecar retaining the current local API is the lowest-disruption migration path to assess; direct Tauri commands are an alternative only with a separately costed frontend/API change.
4. Keep `backend/app/engine.py`'s deterministic page map authoritative until an explicitly reviewed migration of that algorithm. PDF index rows, bookmarks, page labels, volume numbering, CSV ranges and combined offsets must reconcile to one map.
5. Preserve explicit limits and refusal states for unsupported text edits, scanned text, signatures, links, mixed fonts and redaction. A black overlay is not a substitute for content removal.
6. Package and run the chosen engine on macOS Intel and Apple Silicon, Windows x64, and Linux x64; define additional architectures only after evidence. Do not infer Linux release support from a crate's published binaries.

## 5. Acceptance gate for a future implementation ticket

- Build a bounded prototype using synthetic PDFs in `samples/`. Compare the same inputs and operations with the current backend and an independent PDF reader. Record crate/binary versions, transitive licences, package sizes and per-platform startup behavior.
- Exercise import/password handling; every current page operation; text/image placement and bounded text editing; OCR with English/Hindi models and unverified-text provenance; lossless and lossy compression; metadata cleanup; print copies; and volume/combined exports.
- For each generated export, verify visible pixels and extracted text, page order/count, page labels, indexes, bookmarks, internal/external links, CSV ranges, volume/combined offsets, and identical hashes for imported originals and prior exports. Add pytest coverage for every new volume/index rule; preserve a cross-engine regression corpus for the Rust implementation.
- For redaction, test text, images, vector paths, OCR layers, annotations, attachments, metadata and orphaned PDF objects. Inspect saved bytes independently; reject any case that cannot be shown to remove covered content. Do not mark redaction safe for confidential filing from API documentation alone.
- Verify repeated edits/saves, malformed and encrypted inputs, cancellation, low-disk/write failure, restart recovery, and bounded memory/time on large synthetic bundles. Build and run the native Tauri package on each proposed target. Record target-device Save/export behavior separately from browser tests.
- Remove PyMuPDF from shipped runtime paths only after the replacement meets these gates and distribution notices are reviewed. If gaps remain, keep PyMuPDF with an appropriate licence or defer the affected release capability.

## 6. Follow-up

The incremental migration is authorized and in progress. Follow `RUST_BACKEND_PLAN.md` and the feature-by-feature `RUST_REQUIREMENTS_MATRIX.md`; existing worker/protocol/packaging evidence is recorded in `PDF_OXIDE_MIGRATION.md` and `ACCEPTANCE_TESTS.md`. The destination is a Rust desktop backend, not a permanent Python wrapper. Production defaults change only after the applicable operation and platform gates pass. The related commercial distribution gate remains in `PRD_DEVICE_LICENSING_AND_PRICING.md`.
