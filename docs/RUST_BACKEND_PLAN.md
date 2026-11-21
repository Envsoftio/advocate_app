# Rust backend execution plan

Owner authorization: 17 November 2026, updated 18 November 2026. Objective: build the Rust backend in verified stages while preserving existing supported behavior, then switch the complete shipped PDF workflow in one cutover. The [single-cutover plan](RUST_PDF_SINGLE_CUTOVER_PLAN.md) governs that release gate. OXIDE-01 remains open. A completed slice is not full backend or platform acceptance.

## Current architecture decision

Direct Tauri commands over a separate Rust core library supersede the earlier Rust HTTP/delegation proposal. Python remains an independent reference; neither runtime writes the other's store. Read `../rust-backend/README.md` for native pilot entry points and limits.

## Execution ledger

| Step | Deliverable | Acceptance gate | Status |
| --- | --- | --- | --- |
| 1 | Pinned Rust extraction, page protocol, streamed search | Original/vault preservation, protocol failures, endpoint parity, 2,001-page fixture | Implemented; 383 backend tests recorded |
| 2 | Package worker with existing desktop sidecar | Locked host build, frozen discovery, actual packaged extraction and explicit disabled default | Implemented and frozen-service verified on local Apple Silicon; other platforms/native UI pending |
| 3 | Expand reusable API parity suite | Run same cases against Python and Rust-backed routes: auth, error codes, revision guards, Unicode/OCR, limits and downloads | Matter metadata/archive/restore/history: 402 live cases plus per-operation durable history/audit/time comparison; other API families pending |
| 4 | Rust core and direct Tauri commands | Existing UI contracts, explicit unsupported states, no Python delegation; native lifecycle | Core, IPC and isolated matter persistence implemented; full workflows pending |
| 5 | Vault/storage compatibility | Existing SQLCipher databases, encryption formats, backups, recovery and crash fixtures pass; single writer cutover | Pending |
| 6 | Job/revision/history ownership | Cancellation, retry snapshots, atomic publication, restart and Undo/Redo parity | Metadata history implemented/verified in isolated store; jobs/document revisions pending |
| 7 | Rust deterministic planner | Exact golden-map parity for volume/index/numbering/supplement/variant fixtures | Pending |
| 8 | Remaining PDF operations | Per-operation geometry, rendering, content/navigation, originals and failure checks; independent redaction review | Pending |
| 9 | Retire Python dependencies | All routes migrated; native target matrix and upgrade/recovery verified; no shipped dependency paths remain | Pending |

## Rules for every slice

The [requirements matrix](RUST_REQUIREMENTS_MATRIX.md) maps every original brief feature and later PRD additions. Each implementation ticket must name its matrix IDs and satisfy both existing-behavior parity and full feature acceptance. Migration completion and original-requirement completion are separate gates.

1. Record the current operation contract and synthetic fixtures before replacing it.
2. Keep explicit per-capability selection until parity is demonstrated. Never silently retry a mutation through a second engine.
3. Preserve one storage writer and one authoritative page map. No parallel database owners.
4. Run focused failure/parity tests, then the backend regression suite. Run frontend/browser/native checks when affected; record their actual scope.
5. Change the PDF default only after the entire current PDF workflow and dependent Rust storage/jobs/exports pass the single-cutover gate, including fidelity, limits and packaged-device checks. Maintain a documented rollback without data-format changes where possible.
6. Update this ledger and `ACCEPTANCE_TESTS.md`; leave failed/unverified gates open. Do not mark PDF-03/06/07/08 complete based on extraction evidence.

Current boundary details and platform limitations: `PDF_OXIDE_MIGRATION.md` and `RUST_BACKEND_MIGRATION_AUDIT.md`.


## Earlier packaging checkpoint

Packaging slice: embedded Rust worker, explicit frozen-resource discovery and preserved default path verified in local Apple Silicon frozen-service runs. Backend 384, frontend 51 tests pass; app bundle rebuilt. Platform/native UI qualification remains open. Next implementation step is a reusable dual-backend API contract suite, followed by Rust library/API ownership for a bounded read-only route set. Storage, jobs and planner remain Python-owned until their separate gates pass.


## Isolated matter persistence slice — 17 November 2026

Supersedes the earlier empty-matter/read-only pilot checkpoint. The UI-independent Rust core now owns a separate, marked plaintext synthetic SQLite store with schema version 1, atomic create/update/history/audit transactions, restart persistence and explicit storage refusal. Thin IPC carries structured JSON; there is no Rust HTTP server or Python fallback. Python remains the production/default reference and its stores are untouched.

Selected OR-24 scope: create/list/read/update matter metadata, native existing-form editing and reopen. Reusable fixtures compare live Python/Rust responses and durable audit/history records in disposable stores. OR-09 is **not complete**: history records are preserved but listing/Undo/Redo replay remains deferred. Archive/delete/restore, Part mutation, vault/portable migration, documents/revisions, jobs, page mapping, builds and PDFs remain explicitly unsupported. OR-25 dynamic legal forms and the original full matter workflow are not closed by metadata CRUD. `workflow_ready=false` remains mandatory.

Contract, store location and reproduction: [Rust core README](../rust-backend/README.md). Executed results, browser geometry/repaint limits and remaining target gates: [acceptance log](ACCEPTANCE_TESTS.md). Next qualify archive/restore/history and dependent matter-resource contracts before document import; do not infer that this plaintext development schema can replace protected Python storage. PDF Oxide merge-link failure and PDF-03/06/07/08 gates remain open.


## Metadata archive/restore and history slice — 17 November 2026

Supersedes the deferred archive/history statements in the earlier isolated-persistence checkpoint. Rust now implements DELETE-as-archive, archived listing, idempotent restore, history labels/counts/recent actions and metadata-only Undo/Redo in `rust-backend/src/matters.rs`. No schema bump: existing schema-1 JSON records/history are retained, with future-schema refusal unchanged. Immediate transactions publish matter state, history applied flags and audit together. Unsupported history kinds/fields are refused without mutation; no permanent deletion or skipped entries. No-op edits preserve the redo branch; real edits clear it and retain the newest 100 entries. Python defaults, originals, vault boundaries and deterministic page mapping are unchanged; no HTTP server or Python fallback.

OR-24/UX-01 now includes bounded archive/restore and reopen; OR-09/DAT-02 includes bounded metadata replay with restart and rollback checks. Neither full requirement is complete: document/revision history, missing-file/stale-build behavior, full tabs/drafts, vault, jobs and complete application/platform parity remain open. `workflow_ready=false`.

Fresh verification: **402** live Python/Rust contract cases with per-operation durable audit/history and timestamp-change comparison; **9** Rust core tests; **7** Tauri tests; **57** frontend tests; **384** Python tests; final **2** light/dark browser lifecycle journeys. Apple Silicon debug bundle rebuilt; native edit → Undo → Redo → archive → quit/relaunch → restore → close/reopen verified with retained name/notes and audit order. See [exact scope, intermediate failures and remaining issues](ACCEPTANCE_TESTS.md#rust-matter-archiverestore-and-metadata-history--17-november-2026) and the [contract](../rust-backend/README.md#archiverestore-and-metadata-history-contract--17-november-2026).

The existing home geometry test still fails at y=152 versus 144; its assertion is unchanged. Native blank-content repaint reproduced after restore and recovered on window resize, with cause unresolved. These do not close visual/platform acceptance. Next qualify dependent matter-resource contracts and document/revision ownership before extending replay or import; keep vault migration and PDF-03/06/07/08 gates separate.


## Direct IPC scheduling follow-up — 17 November 2026

Rust mode uses the linked library, with no Rust server/backend subprocess. The thin Tauri adapter now dispatches store opening and SQLite operations off the UI/async executor threads, caps database admission at 32 with one async execution lane, and caps PDF extraction at two blocking workers. Started workers retain permits after caller abandonment; cancellation is still unimplemented. Python defaults are unchanged. Fresh tests: Tauri 12, core 9, frontend 57; debug app rebuilt. Native latency/repaint acceptance was not rerun and remains open. See the latest [acceptance entry](ACCEPTANCE_TESTS.md#rust-asynchronous-direct-ipc--17-november-2026).

Next task: [bounded in-process jobs, cancellation and native responsiveness](PROMPT_RUST_IN_PROCESS_JOBS.md). Complete this foundation using the existing read-only extraction workload before expanding to document workflows. No HTTP server, worker executable or Python delegation in Rust mode; maintain `workflow_ready=false`.

## Ephemeral extraction job foundation — 17 November 2026

Step 6 now has a **bounded read-only synthetic extraction slice**: the UI-independent Rust library owns queued/running/completed/failed/cancelled state, two worker threads, four waiting jobs, input/result budgets, progress, result retrieval, cancellation at page boundaries and ten-minute process-local retention. Tauri only transports direct IPC replies. No database writer or transaction was added; Python remains the durable reference/default and `workflow_ready=false`. The [core contract](../rust-backend/README.md#ephemeral-in-process-pdf-extraction-jobs--17-november-2026) records Python route semantics and differences. The shared fixture aligns terminal success and queued shutdown; Python shutdown persists `cancelled` for a never-started future. This does not close step 6: durable document/build jobs, saved-input retry, revision ownership and Python restart parity are still pending.

Fresh core/frontend/Python/402-case metadata results and Apple Silicon native job, cancellation, restart, latency and repaint observations are in [the acceptance log](ACCEPTANCE_TESTS.md#bounded-in-process-rust-extraction-jobs--17-november-2026). A real 3-page extraction and deterministic 3.6-second slow run exercised the linked library. The coarse responsiveness gate passed for command/input latency and heartbeat. The separate blank-content repaint defect reproduced before the scoped transition change; the later final-source native check below supersedes that pending state. The existing home geometry assertion remains unchanged. OR-02, OR-09, OR-24, NAT-01 and the jobs/recovery PRD row receive foundation evidence only; full parity, Windows/Intel, clean-device and PDF writer gates remain open. The prior PDF Oxide merge-link defect is unchanged.

### Native rendering follow-up — 17 November 2026

No-job Rust native captures showed current Overview/Documents accessibility against blank content pixels; modal paint worked, and native full-screen exit restored the content. A scoped macOS WebView workspace-transition override was built and browser-checked. The user relaunched the final-source Rust debug bundle: repeated settled Overview/Documents captures, navigation during delayed extraction, modal/result interaction and full-screen entry all painted. The observed Apple Silicon debug path is qualified; exact WebKit mechanism, release/device breadth and native Save remain open. This does not change Rust job admission, transport, store ownership or `workflow_ready=false`.

### Full flow and next ownership slice — 18 November 2026

The previously failing `matter-flow.spec.ts` now passes 2/2: the empty Overview metrics element and source-backed y=226 geometry are verified, and the Import drop/table align with the supplied 03A y=226/y=398 coordinates. The focused home and browser pixel guards pass 2/2. Final-source frontend 57, Rust core 13, Tauri 12 and Python 387 tests pass; an Apple Silicon debug app was rebuilt and the user relaunched it in Rust mode. Matters, Overview, Documents, Details and full-screen Overview painted in settled native captures; a transient previous-view frame still appeared after navigation. See the [acceptance record](ACCEPTANCE_TESTS.md#full-flow-geometry-and-final-source-native-follow-up--18-november-2026).

Next task: [qualify dependent matter resources and establish synthetic document import ownership](PROMPT_RUST_DOCUMENT_RESOURCE_FOUNDATION.md). Step 3 and step 6 remain partial; a safe import requires immutable-original and working-revision boundaries, atomic publication, restart and failure checks. No vault or planner cutover, Python default change or `workflow_ready` change follows from the visual fix.

### Matter resources and first document ownership slice — 18 November 2026

The bounded prompt above now has a schema-2, Rust-owned synthetic PDF import and matter-resource implementation. A direct IPC command uses the existing bounded database lane and a blocking worker. The core keeps the original and working revision in distinct fixed app-local files, validates and rereads hashes, and commits document/revision/history/audit/matter metadata in one SQLite writer transaction. Reopen cleans unpublished folders and refuses changed or missing committed files. The [route and failure contract](RUST_DOCUMENT_RESOURCE_CONTRACT.md) lists supported responses and explicit 501s. Python remains the default and owns the complete workflow; `workflow_ready=false`.

The 412-case live resource fixture, import subset, focused failure/reopen checks, browser light/dark import and local Apple Silicon native picker import are recorded in [acceptance](ACCEPTANCE_TESTS.md#rust-matter-resources-and-synthetic-document-ownership--18-november-2026). Step 3 and step 6 remain partial: document history cannot replay, certificate-signature screening is incomplete, parser memory is not hard-capped, and crash/power-loss/Windows durability is unqualified. Next qualify signed/encrypted input refusal with a dedicated corpus, document archive/revision replay and durable job/cancellation semantics before widening import. Do not move the Python vault or deterministic planner, or close PDF-03/06/07/08, merge-link or platform gates on this evidence.

The [next bounded task](PROMPT_RUST_DOCUMENT_IMPORT_QUALIFICATION.md) first fixes or gates the exposed import's signature, interruption, memory and native-reopen gaps. Document archive/replay and durable jobs retain separate acceptance gates.

### Import qualification decision — 18 November 2026

[Certain] General PDF admission is disabled in the opt-in Rust pilot. Unix import accepts only the exact `samples/01_petition.pdf` SHA-256; Windows import returns 501 because directory/rename durability is unqualified. The UI excludes mismatched bytes before JSON IPC serialization, and the core remains authoritative. Six child-process interruption points, a concurrent opener, collision/SQLite failures, stale revision metadata, native refusal and native reopen were checked. The [acceptance log](ACCEPTANCE_TESTS.md#rust-pdf-import-qualification-and-restricted-admission--18-november-2026) gives commands, counts and failed attempts; the [resource contract](RUST_DOCUMENT_RESOURCE_CONTRACT.md) gives status and detection limits.

[Likely] The pinned fixture's Mac process-termination recovery is repeatable. The next admission gate is independently validated signed/encrypted/visible-mark PDFs and an ambiguity-refusing inspector. Keep power-loss/disk-full, Windows durability, parser memory/scoped transport, document replay and target devices open. Python remains default, `workflow_ready=false`, and neither PDF writer nor planner ownership changes.

### Restricted import ownership follow-up — 18 November 2026

[Certain] The exact-fixture pilot now checks byte length and equality before parsing, and normal UI/transport checks length before reading. Import revalidates active matter under the SQLite writer lock; startup preserves foreign UUID-shaped folders and removes only recognizable interrupted fixture copies. Resource reads reject stale body identity, page count, signature status, archive state or source hash. Five injected file failures augment the six process-exit fixtures. Core **23 passed/4 ignored child entry points**, Tauri **13**, frontend **62**, live parity **412**, synthetic import subset and browser **2/2** passed. This closes the material race and cleanup gaps in the restricted import; it does not widen PDF admission. The next task is [general PDF admission qualification](PROMPT_RUST_GENERAL_PDF_ADMISSION.md). Keep the exact-byte gate, Python default and `workflow_ready=false` until its gates pass.

### General PDF admission corpus checkpoint — 18 November 2026

[Certain] A deterministic 12-file synthetic corpus now covers five real CMS signatures plus encrypted, unsigned form/Perms/object-stream, visible-mark-only, malformed and plain unsigned controls. OpenSSL and PyMuPDF results are in [the manifest](../samples/rust_pdf_corpus/manifest.json). Rust refuses every corpus file before publication. `pdf_oxide/signatures` cannot compile offline because `cms` is uncached. The next gate is a packaged ambiguity-refusing parser, hard pre-deserialization transport/parser memory bound, disk-full and power-loss evidence, and real Windows flush/rename and native checks. No new class is admitted; Python stays default and `workflow_ready=false`.

### Tauri byte-argument bound and next feature — 18 November 2026

Direct PDF byte commands now use capped Serde sequence deserialization (4 MiB import, 16 MiB extraction/job start); raw Tauri IPC buffering and PDF Oxide parser memory remain unbounded at the process level. No admission class or default backend changes. The next independent Python-to-Rust feature mapping is [document archive/restore and `document_presence` history](PROMPT_RUST_DOCUMENT_LIFECYCLE_PARITY.md). General PDF admission remains blocked on inspector, total memory, durability and device gates.

### Native token import and document lifecycle slice — 18 November 2026

[Certain] This implemented slice supersedes the preceding next-feature and byte-array-command statements. The Rust pilot now uses a native picker → bounded open file handle → opaque token → core import path; no exposed Tauri command accepts PDF byte-array JSON. The file is rechecked at import, and the core retains its exact 6,004-byte pinned gate. The normal browser File path is explicitly 501 in Rust mode. Native macOS picker refusal and import were verified. This closes the specific raw-JSON-buffering-before-Serde gap for the **shipped pilot PDF input path**, while general IPC/parser memory and broader PDF admission remain unqualified.

Rust document archive, restore and `document_presence` replay now run in the UI-independent core with one SQLite writer transaction and preserved original/revision files. Paired Python/Rust synthetic contract requests, injected failure and child-process exits, browser light/dark flow, and native archive → restart → restore → Undo/Redo are the acceptance path. The [resource contract](RUST_DOCUMENT_RESOURCE_CONTRACT.md#native-token-import-and-document-lifecycle--18-november-2026) records deliberate stronger Rust hash refusal and exact route precedence. OR-03/04/09/11/24 and DAT-01/02/03/06/07 remain partial. The owner-requested [single shipped PDF cutover](RUST_PDF_SINGLE_CUTOVER_PLAN.md) remains the production plan; no cutover happened. Python remains default, `workflow_ready=false`, Windows import 501, and PDF-03/06/07/08, native Save, power-loss/disk-full and target-device gates remain open.
