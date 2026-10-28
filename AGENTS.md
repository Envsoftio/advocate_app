# Development-agent instructions

- Product name: **MatterDraft Studio**, shortened to **MatterDraft**. Use application/product terminology in current UI, packages and documentation. Keep historical evidence and compatibility identifiers exact; do not infer production qualification or court approval from branding.
- Read `README.md`, `docs/PROJECT_CONTEXT_AND_HANDOFF.md` and `docs/ACCEPTANCE_TESTS.md` before changing architecture.
- Stack: Vue 3 + TypeScript + Tauri 2; local Python FastAPI/PyMuPDF/Tesseract sidecar; SQLite.
- **Never describe court rules as nationwide or court-approved unless exact scoped official sources and an advocate's approval exist.** The `delhi-hc-illustrative` pack is explicitly NOT legally approved.
- Preserve imported originals; all page edits operate on working copies. Never infer that OCR text or a scanned signature is legally verified.
- PDF index, bookmarks, volume page numbers, CSV ranges and combined offsets must all derive from one deterministic page map (`backend/app/engine.py`). Add pytest cases for every new volume/index rule.
- Keep backend processing UI-independent. Avoid adding cloud dependencies for core features.
- Python tests: `cd backend && python -m pytest -q`. Use the configured backend virtual environment; record current results in `docs/ACCEPTANCE_TESTS.md`.
- Frontend: `cd frontend && npm run build && npm run test`; install dependencies with `npm install` if missing. Browser checks use `npm run test:e2e` with synthetic data.
- Rust/Tauri: native build requires matching PyInstaller sidecar in `frontend/src-tauri/binaries/`; create via `python backend/scripts/build_sidecar.py`, then `cd frontend && npm run tauri:build -- --bundles app` on macOS. Local Apple Silicon builds have been verified; consult the acceptance log for remaining target-device checks.
- First priority: real Mac/Windows compile, browser UI runtime verification, native export/save testing. Do not infer those tests passed.
- Work with synthetic files in `samples/`, not actual confidential matters. Source hashes and signed documents require extra caution.
- For new implementation tickets, carry authorized work through implementation, verification and handoff in one pass; do not stop after scaffolding. Mark a ticket complete only when its stated acceptance checks pass.

- UI work must use the supplied scoped design as the layout source. Preserve capabilities and data, not the legacy layout. Verify rendered screenshots against the reference viewport before marking a UI task complete; do not defer visual fidelity to later feature tasks. See `docs/DESIGN_PHASE02D_UI_CORRECTION.md`.
