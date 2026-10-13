# Development-agent instructions

- Read `README.md`, `docs/PROJECT_CONTEXT_AND_HANDOFF.md` and `docs/POC_ACCEPTANCE_TESTS.md` before changing architecture.
- Stack: Vue 3 + TypeScript + Tauri 2; local Python FastAPI/PyMuPDF/Tesseract sidecar; SQLite.
- **Never describe court rules as nationwide or court-approved unless exact scoped official sources and an advocate's approval exist.** The `delhi-hc-illustrative` pack is explicitly NOT legally approved.
- Preserve imported originals; all page edits operate on working copies. Never infer that OCR text or a scanned signature is legally verified.
- PDF index, bookmarks, volume page numbers, CSV ranges and combined offsets must all derive from one deterministic page map (`backend/app/engine.py`). Add pytest cases for every new volume/index rule.
- Keep backend processing UI-independent. Avoid adding cloud dependencies for core features.
- Python tests: `cd backend && python -m pytest -q` (9 passing at handoff).
- Frontend: `cd frontend && npm install && npm run build && npm run test`. NOT compiled in authoring container (npm DNS unavailable).
- Rust/Tauri: native build requires matching PyInstaller sidecar in `frontend/src-tauri/binaries/`; create via `python backend/scripts/build_sidecar.py`. NOT compiled in authoring container (Rust toolchain absent).
- First priority: real Mac/Windows compile, browser UI runtime verification, native export/save testing. Do not infer those tests passed.
- Work with synthetic files in `samples/`, not actual confidential matters. Source hashes and signed documents require extra caution.
