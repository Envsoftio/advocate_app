# MatterDraft Rust application core

A library linked directly into Tauri. There is no Rust HTTP server, port, CORS layer or Python delegation in this crate. The earlier Axum experiment was removed. Python remains the separate default/reference backend and its stores are never opened by this pilot.

## Start the current app in direct Rust mode

From the repository root run `./dev-rust.sh`. Stop an existing frontend on port 1420 first; the script refuses to reuse a potentially Python-configured development server. The only development web server is Vite for frontend hot reload, not a processing backend. Browser-only mode is intentionally unsupported because Tauri IPC requires the native host.

The launcher sets `MATTERDRAFT_BACKEND=rust` and opens the isolated protected store at `<app_local_data_dir>/rust-protected-v1`. The earlier `rust-vault` value and `--vault` launcher option select the same store. Set the selector in the existing frontend environment files to use the standard Tauri development command. The native host opens the store on the database worker before advertising direct IPC to the frontend. Python remains the standard package default. [Configuration and restart behavior](../README.md#backend-selection)

The original synthetic fixtures are available through `./dev-rust.sh --synthetic`. This debug-only mode uses `<app_local_data_dir>/rust-synthetic-v1/rust-matters.sqlite3`. The native host owns these paths, and Rust opens neither the Python database nor its vault. `workflow_ready` remains false pending the migration acceptance checks.

The store is deliberately **plaintext and synthetic-only**. A new root is marked `RUST_SYNTHETIC_STORE`; existing unmarked directories, foreign contents, store symlinks and future schema versions are refused. Unix roots are created with mode 0700. This is not SQLCipher/vault support or a defence against a malicious same-user process replacing filesystem entries. Vault setup/unlock/migration, protected backups and key rotation remain unsupported; no protected store is downgraded.

### Original synthetic compatibility contract

Source evidence: `backend/app/schemas.py` (`MatterCreate`, `MatterPatch`), `main.py` matter routes and `storage.py` matter/history/audit methods. The original brief's matter workflow (OR-24) is only partially addressed here; OR-09 has bounded metadata replay; document/revision history and later PRD vault/backup/job requirements remain open.

- `POST /matters`: 201; Python field defaults, Unicode character limits, known embedded profile IDs, UUID-v4 lowercase 32-hex identifiers, ordered unique initial Parts and next-Part rules. Extra create fields are ignored as in the reference.
- `GET /matters`: 200, descending `updated_at`; ties retain insertion order. `GET /matters/{id}`: record or 404 `Matter not found`.
- `PATCH /matters/{id}`: supplied mutable fields only; forbidden extra fields, null/profile checks, 422 validation detail arrays, missing-ID precedence. Empty or value-identical patches preserve timestamps and do not add audit/history. The 402 live fixtures exercise Python integer/boolean coercions as well as normal typed UI input.
- Response keys include `parts`, `next_part`, `archived_at: null`, Boolean group policy and microsecond UTC timestamps. No original/document files exist in this bounded store.
- Create and update use immediate SQLite transactions; edit, retained history and audit publication commit together. Schema initialization uses the same transactional boundary and `PRAGMA user_version=1`. A bounded native async lane serializes requests; the store mutex is acquired only on a blocking worker, and SQLite locks protect separate connections. Failed commits return a structured 500; no automatic retry/fallback.
- History records preserve the Python `matter` kind, integer group-policy values, before/after fields, 100-entry retention and removal of unapplied branches. History listing and metadata-only Undo/Redo now use these retained records. Unsupported kinds are refused atomically; they are never skipped.
- Python `DELETE` archives instead of destroying data and restore retains records/originals. Rust now supports archive/restore (see below). Part mutation remains 501. Portable restore, audit listing, storage totals, document/count reads, validation, jobs, reader positions, bundles and PDF transport remain 501. The home table therefore shows unavailable counts; this is not a zero-document parity claim for imported matters.

Native pilot interaction: create a synthetic matter, open Overview → Matter details → Edit matter, then Save changes. The edit action is enabled only for the Rust pilot and reuses the existing matter form. Python production defaults and UI entry points remain unchanged. Native details edits and restart persistence were exercised on Apple Silicon; platform and repaint limitations are in `docs/ACCEPTANCE_TESTS.md`.

`extract_text` is genuine PDF Oxide work and is exposed through the earlier one-shot `rust_extract_text` on a blocking worker thread, with a 16 MiB input and 2,000-page/10-million-byte output-text cap. That one-shot command is not cancellable. The job API below adds page-boundary cancellation for the same read-only extraction workload. These synthetic-development commands use byte-array or embedded-sample arguments; they are not the final large-file IPC design. No arbitrary filesystem path commands are exposed. Large-file scoped handles/range transport remains pending. Read-command waiting is bounded in the frontend; abandoning a promise is not worker cancellation.

## Checks

- `cargo test --locked --manifest-path rust-backend/Cargo.toml`
- `cargo test --locked --manifest-path frontend/src-tauri/Cargo.toml --lib`
- `cd frontend && npm run build && npm run test && npm run format:check`

Full workflow parity, production vault/storage and durable document/build job ownership, document history replay, deterministic planner and PDF writer migration remain open in `docs/RUST_BACKEND_PLAN.md` and `docs/RUST_REQUIREMENTS_MATRIX.md`.

### Reusable parity checks

```sh
cargo build --locked --offline --manifest-path rust-backend/Cargo.toml --example contract
backend/.venv312/bin/python rust-backend/tests/parity.py
cd frontend
MATTERDRAFT_E2E_VITE_PORT=1437 MATTERDRAFT_E2E_BACKEND_PORT=8779 npm run test:e2e -- rust-matters.spec.ts
```

`tests/matter_contract.json` feeds the live Python FastAPI reference and Rust example using separate disposable roots. Only generated IDs/times are normalized; status, response fields, ordering and error bodies are compared. The runner additionally compares durable audit/history fields. The Rust process reopens the store for every operation. The example is a JSON-lines test driver, not a shipped service or HTTP transport. Browser tests substitute only the IPC host, invoking the real Rust driver, and exercise cancellation, focus, save and reload in light/dark layouts. Native Tauri checks are recorded separately.

Remaining gates: broad adversarial validation fuzzing, filesystem tampering/race hardening, disk-full/power-loss crash recovery, production data migration, encrypted storage and Windows/Intel Mac/clean-device qualification. These tests do not close those gates.

## Archive/restore and metadata history contract — 17 November 2026

Established from `backend/app/main.py` matter/history routes, `schemas.py` MatterPatch and `storage.py` list/get/delete/restore/history methods before implementation. Status codes are carried in structured IPC replies, not a Rust HTTP server. Existing thin IPC required no changes. `workflow_ready=false`; `history_replay_scope=matter-metadata-only`.

| Request | Reference contract implemented in Rust |
| --- | --- |
| `GET /matters` | 200 active records only; descending updated time, insertion order for ties. |
| `GET /matters/archived` | 200 archived records only; descending archive time, insertion order for ties. |
| `DELETE /matters/{id}` | 200 `{"status":"archived","note":"Matter, records and original files retained locally"}`. Sets archive/update times, publishes `matter_archived`. Missing/already archived: 404 `Matter not found`. No permanent deletion. |
| `POST /matters/{id}/restore` | 200 complete matter, clears archive time, advances update time, publishes `matter_restored`. Already-active ID returns unchanged record with no audit/timestamp/history change. Missing ID: 404 `Archived matter not found`. |
| `GET /matters/{id}/history` | 200 `can_undo`, `can_redo`, `undo_count`, `redo_count`, nullable `undo_label`/`redo_label`, `recent_actions` (newest eight, label and Boolean applied). Metadata label: `Matter settings`. |
| `POST /matters/{id}/history/undo` or `/redo` | 200 resulting history state. Undo newest applied; Redo oldest unapplied. Restores only recorded mutable metadata, advances matter update time, flips applied and publishes `edit_undone`/`edit_redone` atomically. Does not add a history entry. Empty direction: 409 `No edit to undo` / `No edit to redo`. |
| Unknown history direction | 404 `Unknown history operation`, after active-ID lookup. Missing/archived IDs take precedence with 404 `Matter not found`. |

Archived IDs also return 404 for GET/PATCH/history; schema validation still precedes route lookup for malformed PATCH bodies. All timestamps remain microsecond UTC, creation time remains unchanged. Archive and restore retain all history and the redo branch and are not themselves undoable. Empty/value-identical patches preserve times/audit/history, including an existing redo branch. A real new edit removes unapplied entries and retains the newest 100 entries. Returned group policy stays Boolean while retained Python-compatible history stores integer values.

Replay accepts only `matter` entries containing implemented MatterPatch fields. Other kinds return 409 `Unsupported edit history entry`; unsupported metadata fields return 409 `Unsupported matter metadata history entry`. Listing retains reference labels/counts even for unsupported kinds, but replay refuses the selected entry rather than skipping it. Corrupt stored JSON/SQLite errors return structured 500 and roll back. No document, Part, build, validation, vault or PDF results are fabricated.

**No schema upgrade is needed:** schema 1 already holds archive timestamps in the JSON body and complete before/after metadata history. Existing marked stores and history remain usable; future schemas are still refused. Mutations and history-state response calculation run within one immediate transaction. Core trigger-injection tests fail history, record and audit publication, compare complete before/after snapshots and verify restart consistency; this is not simulated power-loss/disk-full qualification.

The 402 shared fixtures run live against disposable Python/Rust stores. The runner compares responses, generated timestamp formats, timestamp change/no-op patterns and durable audit/history after every request. Rust reopens the store each operation. Fixtures cover archive visibility/order, restore idempotence, Undo/Redo boundaries, branching, no-ops while redo exists, archived/missing IDs, invalid direction, retention and restart. Core tests additionally cover tied archive times, prior schema-1 records, future schema refusal and unsupported replay.

The existing Archive matter, Archived matters filter, restore and toolbar Undo/Redo controls are connected. Editing refreshes history, archive closes Details and returns to Matters, and restore checks the selected transport before opening Overview in Rust mode (including a fresh restart). Python retains its existing Documents restore destination. Native and browser results, including unresolved repaint/geometry issues, are recorded in the latest acceptance-log section.


## Direct asynchronous IPC scheduling — 17 November 2026

The Rust pilot does not launch a backend server or worker executable. Tauri calls the linked library directly. The adapter's `RustRuntime` is lightweight at setup: store opening/initialization is deferred to the first matter request on an in-process blocking-pool thread. Both `rust_request` and `rust_extract_text` are asynchronous commands; marking a function async alone is insufficient, so blocking operations explicitly use `spawn_blocking`.

Database admission is capped at 32 outstanding requests. An async semaphore serializes them before dispatching to the blocking pool; database waiting does not occupy 32 worker threads. Capacity exhaustion returns structured 503 before any work starts. PDF extraction admits two concurrent workers and rejects oversized input before dispatch. Its permits and database permits remain owned by started worker closures through completion, even if a caller stops waiting. There is no automatic retry or Python fallback. Small static health/capability reads bypass the database lane.

This fixes synchronous database dispatch and startup I/O in the Rust pilot. It does not provide cooperative cancellation, hard parser memory/CPU limits, bounded large-file IPC serialization, production jobs or proof that native repaint cannot stall. The default Python/release path and its earlier optional Oxide executable are separate and unchanged. The next bounded task is specified in [the in-process jobs prompt](../docs/PROMPT_RUST_IN_PROCESS_JOBS.md).

## Ephemeral in-process PDF extraction jobs — 17 November 2026

The reference contract was inspected in `backend/app/jobs.py`, `main.py` job routes, `schemas.py` and `storage.py`. Python creates UUID-style job IDs, starts `queued`, moves to `running`, then `completed`/`failed`/`cancelled`. `GET /jobs/{id}` returns status, progress, messages, events, error and embedded result; missing ID is 404. `POST /jobs/{id}/cancel` returns `{"cancel_requested":true}` and cancellation is cooperative. Durable startup marks queued/running jobs `interrupted`; saved build inputs can be retried with a new ID, and terminal records expire after 90 days. Shutdown now persists `cancelled` for queued futures that never execute, and restart retains that terminal state.

The **Rust subset is deliberately different**: `rust_job_start`, `rust_job_status`, `rust_job_cancel`, `rust_job_result` are direct IPC commands for read-only `pdf-text-extract` only. `rust_debug_job_start` loads the embedded three-page synthetic sample and optionally delays each page in debug builds. Production builds refuse that debug command. No route claims document OCR, build, import, retry or revision ownership. An opaque random 32-hex job ID scopes status, cancellation and result access to the current process. Rust uses `completed` for terminal success and keeps results out of status/progress responses. Status has `page_count`, `completed_pages`, percentage and an error without full page text; result retrieval is explicit and returns only completed pages. Missing/expired/restarted ID is 404; unavailable result is 409; saturation is 503; invalid input is 422. `workflow_ready=false` remains.

The core library owns the job state and PDF Oxide call. At most two in-process worker **threads** run, four jobs wait, total admitted PDF input is 32 MiB, each input is at most 16 MiB, retained text is at most 20 MiB, and at most 16 jobs remain. Terminal entries expire after ten minutes on the next job operation. The 2,000-page and 10-million-byte extracted-text limits still apply. Admission fails before a new job is enqueued. Cancellation before start drops the queued input and publishes `cancelled`; running cancellation is only requested until the worker checks before the next page, then it publishes one terminal state. A synchronous PDF Oxide page call cannot be interrupted midway. A caller dropping its promise leaves the worker running. Panic/error paths release worker/input ownership. Shutdown requests cancellation and drops queued inputs; the process owns no durable job records, so restart cannot replay work or show a perpetual running state. This ephemeral policy is acceptable only for synthetic read-only verification; Python's durable job/retry semantics remain the requirement for future document/build jobs.

No extraction holds a SQLite transaction. Health/capability reads and metadata navigation retain their separate admission lane. PDF Oxide parser caches and Tauri JSON byte-array serialization are **not** hard memory bounds; large-file scoped transport and OS-level parser limits remain open. Progress is polled through structured IPC replies, not an event stream. The shared `tests/job_contract.json` is exercised by Rust core tests and `backend/tests/test_job_contract_fixture.py` for comparable states. Python embeds results in its durable status record and supports interrupted saved-input retry; Rust requires a separate result call and returns 404 after process restart or ten-minute expiry. The acceptance log records the reproduced native blank workspace and its local Apple Silicon debug after-fix pixel check; wider native qualification remains open.

## Final visual and contract handoff — 18 November 2026

The browser full flow now passes 2/2 after exact empty Overview y=226 and Import y=226/y=398 corrections; home and pixel guards pass 2/2. Final-source frontend 57, Rust core 13, Tauri 12 and Python 387 tests pass. The rebuilt Apple Silicon debug app was relaunched in Rust mode and its settled Matters, Overview, Documents, Details and full-screen Overview pixels were inspected alongside AX. Brief previous-view pixels can still precede the settled view. This does not expand the Rust backend's document/import capabilities or qualify release/Windows/Intel/clean-device/native Save. The [next bounded task](../docs/PROMPT_RUST_DOCUMENT_RESOURCE_FOUNDATION.md) covers dependent matter-resource contracts and synthetic document ownership; `workflow_ready=false`.

## Synthetic matter resources and PDF import — 18 November 2026

The isolated `rust-synthetic-v1` store migrates schema 1 to 2 and adds document, revision and document-linked audit records. `Store::import_pdf` is the UI-independent owner; Tauri's `rust_import_pdf` transports bytes through the existing bounded database admission lane and runs blocking work in `spawn_blocking`. The UI accepts only PDF files at most 4 MiB in Rust mode. No Rust HTTP server, Python delegation or Python vault access is added. Python remains the default; `workflow_ready=false`.

For an active matter, the core parses one plain PDF, counts 1–2000 pages, rejects duplicate source SHA-256 and writes a read-only `original.pdf` plus distinct `revision-1.pdf` under an opaque fixed `documents-v1/<document-id>` directory. It stages/syncs files while holding the SQLite immediate writer transaction, renames and verifies hashes, then publishes document, revision, `document_presence` history, audit and matter timestamp together. Normal failure removes uncommitted files; reopen removes orphan staging/published folders and refuses missing/changed committed files with 409. Import history is retained but Undo refuses it, with `can_undo=false` when it is next. These tests do not simulate power loss. Windows directory fsync and parser memory bounds remain open.

`GET` documents, archived-documents, storage, audit and exports use Rust-owned data and honor missing/archived matter 404. Parts, preview, PDF page tools, build/export and other workflows remain explicit 501 or are guarded in the pilot UI. The exact [resource and failure contract](../docs/RUST_DOCUMENT_RESOURCE_CONTRACT.md) includes route shapes and deviations. The pilot refuses encrypted PDFs, AcroForm/permission dictionaries and literal `/ByteRange` markers, but certificate-signature inspection is incomplete and saved status remains inconclusive. The optional PDF Oxide signature feature was unavailable offline (`cms` uncached, crates.io DNS failed). A signed corpus is required before widening import.

Focused checks: 412-case live Python/Rust matter fixture, synthetic cross-backend import subset, 16 core tests, 6 Tauri command tests, 59 frontend tests, 387 Python tests, final 2 light/dark browser import journeys and an Apple Silicon debug native picker import. See the [acceptance entry](../docs/ACCEPTANCE_TESTS.md#rust-matter-resources-and-synthetic-document-ownership--18-november-2026). PDF-03/06/07/08, merge-link, vault, planner, release/Windows/Intel/clean-device and native Save remain open.

## Restricted import qualification — 18 November 2026

[Certain] The earlier general unsigned-PDF claim is superseded. AcroForm/Perms and literal ByteRange screening is insufficient for arbitrary signed PDFs. `Store::import_pdf` now rejects every byte stream except exact `samples/01_petition.pdf` SHA-256 `76da5e34c1799a5aff3d3e9bec8cc689cb6cf8430dfc163f93bfc3ec286bf17a` before parser construction or filesystem writes on Unix. Windows returns 501 until directory/rename durability is qualified. A matching filename is insufficient; any changed byte returns 422. The frontend uses the same hash to exclude files before IPC byte-array conversion; the core check is authoritative. This is a synthetic byte allowlist, not signature or legal verification, and recorded certificate status stays `unknown`.

[Certain] Six child-process exit fixtures cover staging, each file sync, rename, SQLite publication before commit and after commit. Reopen removes uncommitted folders, exposes no uncommitted record, or finds both exact files and one complete audit/history publication. A second process waits for an active writer's staging transaction. Existing collision and injected SQLite failures preserve unrelated content; document reads now cross-check body/SQL paths and original/revision hashes, refusing stale metadata with 409. Core **19 passed, 2 ignored child entry points**, Tauri **13**, frontend **59**, live matter parity **412**, synthetic import subset and browser light/dark **2/2** passed. The final Apple Silicon debug bundle reopened the imported synthetic matter and excluded a changed PDF through the native picker. Full commands and limitations are in [acceptance](../docs/ACCEPTANCE_TESTS.md#rust-pdf-import-qualification-and-restricted-admission--18-november-2026) and the [resource contract](../docs/RUST_DOCUMENT_RESOURCE_CONTRACT.md).

[Likely] Mac process-interruption recovery for this pinned fixture is repeatable. General admission still needs independently validated signed/encrypted/visible-mark fixtures, an ambiguity-refusing inspector, hard parser memory or scoped transport, power-loss/disk-full, Windows durability and target-device evidence. `document_presence` remains non-replayable; Undo/Redo returns 409. Python remains default, `workflow_ready=false`, and PDF-03/06/07/08, merge-link, native Save, vault and planner parity remain open.

### Restricted import gap closure — 18 November 2026

[Certain] The core now checks exact fixture length, SHA-256 and byte equality before parsing. Normal UI/transport refuses wrong-sized files before reading; direct Tauri IPC deserialization has no hard memory cap. Import checks active-matter status again inside the writer transaction. Reopen removes only empty/partial pinned staging content or complete unpublished pinned files, preserving foreign UUID-shaped folders and failing closed. Resource reads cross-check body/SQL identity, archive state, page count, pinned source hash, signature status and revision hash. Focused archive race, foreign-folder, stale-metadata and five injected file-failure fixtures pass; see [acceptance](../docs/ACCEPTANCE_TESTS.md). The exact sample remains the only accepted PDF on macOS/Linux, and Windows remains 501. No document replay or general signature conclusion follows.

### General PDF corpus checkpoint — 18 November 2026

The [12-file synthetic corpus](../samples/rust_pdf_corpus/README.md) contains five OpenSSL-verified detached CMS signature variants, encrypted and unsigned controls, a visible mark without a certificate signature, and malformed input. Its [manifest](../samples/rust_pdf_corpus/manifest.json) records SHA-256 and independent parser output. The core refusal test runs each file through `Store::import_pdf` and verifies no document folder is created. No new class is enabled: Unix still accepts exact `samples/01_petition.pdf` bytes only, Windows returns 501, and imported certificate status stays `unknown`. Offline `pdf_oxide/signatures` compilation fails because `cms` is missing. Hard direct-IPC/parser memory limits, disk-full/power-loss and native target evidence remain open.

The thin Tauri adapter now bounds deserialized PDF byte arrays to 4 MiB for import and 16 MiB for extraction/job start. This is an adapter safeguard, not a core ownership change or hard total-process memory bound; raw IPC buffering and parser allocations remain open. The next [document lifecycle parity prompt](../docs/PROMPT_RUST_DOCUMENT_LIFECYCLE_PARITY.md) maps archive/restore and `document_presence` replay without widening PDF admission.

## Native token import and document presence — 18 November 2026

[Certain] This section supersedes the preceding byte-array transport and non-replayable-presence statements. The exposed Tauri PDF byte-array commands were removed. In Rust mode, `rust_pick_import_pdf` opens a native picker, checks a regular 6,004-byte file and the pinned SHA, and stages a capped open handle. It returns only metadata and a one-use opaque token; `rust_import_picked_pdf` rechecks and sends bytes inside the native process to `Store::import_pdf`. `rust_discard_picked_pdf` releases a staged handle. The HTML File upload path returns 501 before reading bytes. Windows picker/import returns 501. No new PDF class is admitted and certificate status stays `unknown`.

The core now supports `DELETE /documents/{id}`, `POST /matters/{mid}/archived-documents/{id}/restore`, and `document_presence` Undo/Redo for import/archive/restore. Archived listing and history expose the corresponding live state. Original/revision files and hashes remain unchanged; restore and replay refuse stale or missing files before publication. All metadata, history and audit changes use the single SQLite writer transaction. Read [route shapes, precedence and safety deviations](../docs/RUST_DOCUMENT_RESOURCE_CONTRACT.md#native-token-import-and-document-lifecycle--18-november-2026) and [verification](../docs/ACCEPTANCE_TESTS.md#native-token-import-and-document-presence--18-november-2026). The Python backend and protected vault are untouched; `workflow_ready=false`. This is not a hard global Tauri JSON or PDF parser memory bound, and native Save, power-loss/disk-full, general PDF admission and full cutover remain open.

## Admission and page-map parity checks

```sh
backend/.venv312/bin/python rust-backend/tests/admission_contract.py
backend/.venv312/bin/python rust-backend/tests/planner_contract.py
backend/.venv312/bin/python rust-backend/tests/python_route_inventory.py
```

The first harness compares the in-memory Rust PDF admission candidate against live Python uploads in a disposable store. It uses synthetic PDFs and passwords. The planner harness compares complete maps and errors from the same frozen document/settings fixtures. The inventory writes JSON to stdout. [Single-cutover plan](../docs/RUST_PDF_SINGLE_CUTOVER_PLAN.md), [acceptance log](../docs/ACCEPTANCE_TESTS.md#rust-pdf-admission-and-page-map-parity--18-november-2026).

## Protected storage and revision checks

```sh
backend/.venv312/bin/python rust-backend/tests/vault_contract.py
cargo test --locked --offline --manifest-path rust-backend/Cargo.toml --all-targets
```

`VaultStore` owns a keyed SQLite connection in a separately marked Rust root and holds an exclusive lock for its lifetime. Core-only protected imports and edits publish encrypted original and revision files beside the SQLCipher database. The examples use disposable synthetic roots. The desktop uses the existing cutover plan for activation. [Protected storage results](../docs/ACCEPTANCE_TESTS.md#rust-protected-storage-and-retained-working-revisions--18-november-2026).

## Matter preparation and document order

Schema 4 adds matter preparation records to the isolated synthetic and protected Rust stores. The Rust request path implements Parts, Memo of Parties, matter values, filing review and document reordering. Each edit publishes state, audit and Undo/Redo history in one transaction. Document ordering updates metadata and verifies the retained document artifacts. The [coverage register](../docs/RUST_REQUIREMENTS_MATRIX.md#current-python-to-rust-coverage--19-november-2026) lists available and pending Python features.

## Vault, reader and working-copy Save

Run `./dev-rust.sh` to exercise vault setup, unlock and rotation in the isolated `rust-protected-v1` store. The protected picker accepts PDF, PNG and JPEG. Rust provides original, working and SHA-bound retained PDF reads in bounded binary IPC chunks. The Documents preview can page through a working PDF and export an exact revision through Save copy.

The [verification record](../docs/ACCEPTANCE_TESTS.md#rust-vault-reader-and-working-copy-save--19-november-2026) lists exercised behavior and remaining native checks.

## Offline Rust OCR qualification bundles

Use `backend/scripts/build_rust_ocr.py` with local source trees, CMake and the target toolchain. The local qualification candidates used [Tesseract 5.5.3](https://github.com/tesseract-ocr/tesseract/tree/5.5.3) and [Leptonica 1.87.0](https://github.com/DanBloomberg/leptonica/tree/1.87.0). The script defaults to macOS 12.0 and supports Apple Silicon, Intel Mac and Windows x64 builds. Use a Windows developer shell or pass a CMake toolchain for the Windows cross-build. LLVM-MinGW builds of these sources need `-D_USE_MATH_DEFINES` in the C++ compiler flags.

Run `backend/scripts/stage_rust_ocr.py` with `--library`, `--library-source` and `--licenses`. The build machine fetches the pinned [English and Hindi tessdata_fast 4.1.0 models and license](https://github.com/tesseract-ocr/tessdata_fast/tree/4.1.0) and verifies their SHA-256 hashes. For a build with a local model cache, pass `--tessdata` and `--models-source` together. On macOS the script rejects libraries that require a version above 12.0 by default, rewrites bundled library references relative to the loader and signs the staged libraries. On Windows put redistributable DLLs alongside Tesseract and use `dumpbin` from a Visual Studio developer shell or supply `--windows-objdump` for a cross-build. Staging records native architectures, model and library hashes, and macOS deployment targets.

Build an isolated qualification app with the following command from `frontend`. The same selector can come from a production `.env` layer. The package launcher adds the existing Rust qualification feature and offline OCR configuration. Select the matching target and installer format on the build host. The packaged app opens the separate `rust-qualification-v1` protected store.

```sh
MATTERDRAFT_BACKEND=rust npm run tauri:build -- --bundles app
```

For a different staging path, supply the `rust-qualification` feature and its matching `--config` explicitly. Target qualification requires running the bundle with English, Hindi and mixed synthetic scans on each supported OS and architecture. The [acceptance log](../docs/ACCEPTANCE_TESTS.md) records executed results.

The Rust PDF engine is linked into the app. Packaged OCR loads Tesseract, Leptonica, their required redistributable libraries and the language models from `matterdraft-ocr/` in the app resources. The qualification build rejects a missing or mismatched target manifest and checks every listed file's checksum. Users can run core PDF functions and OCR offline without installing a package manager or language models.

The frontend build embeds the PDF.js worker, standard fonts, predefined character maps and their notices. On Windows, build the distribution with `--bundles nsis` or `--bundles msi` on a Windows build host. The Tauri configuration embeds Microsoft's WebView2 offline installer. Windows setup supplies WebView2 if needed, and MatterDraft launches without a dependency installation step. A raw Windows executable or developer ZIP requires a machine with WebView2 already installed.

## Convert a detached Python vault

The `import_python_snapshot` binary reads a copy of a closed, active Python vault. It writes a new protected Rust vault to an unused directory. The converter accepts the vault password on standard input. Use `--recovery` to supply its recovery key instead.

```sh
cargo run --locked --manifest-path rust-backend/Cargo.toml --bin import_python_snapshot -- /path/to/detached-vault /path/to/new-rust-vault
```

Use a disposable synthetic vault for qualification. Copy the complete vault directory after its Python engine closes and checkpoints the database. The converter preserves document and build IDs and retains the original key envelope, so the resulting Rust vault opens with the same credentials. A failed frozen-job conversion keeps the historical job and records why it cannot be retried. The conversion fixture is `backend/.venv312/bin/python rust-backend/tests/legacy_snapshot_contract.py`. See the [acceptance log](../docs/ACCEPTANCE_TESTS.md#rust-conversion-and-pdf-parity-implementation--19-november-2026).
