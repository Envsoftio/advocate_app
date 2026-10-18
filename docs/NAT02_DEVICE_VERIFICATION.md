# NAT-02 clean-device verification

NAT-02 stays open until the approved target matrix has device evidence. Use only `samples/` synthetic files. Do not install Python or Tesseract on the test device. Record an artifact SHA-256 before installing and keep the same artifact for the reinstall check.

| Target | Package to build on matching host | Minimum device to test |
| --- | --- | --- |
| Windows x64 | PyInstaller sidecar, then Tauri Windows installer | Windows 10 x64 and Windows 11 x64 |
| macOS Intel x64 | PyInstaller sidecar, then Tauri x64 DMG | macOS 12 Intel |
| macOS Apple Silicon arm64 | PyInstaller sidecar, then Tauri arm64 DMG | macOS 12 Apple Silicon |

On each **build host**, use the matching architecture, install the backend requirements in a local Python environment, and run that environment's Python with `backend/scripts/build_sidecar.py`. Then run `npm ci` and `npm run tauri:build` from `frontend/`. Confirm the sidecar in the package has the expected architecture and that the installer contains it. Build on a host at or below the intended minimum OS version, or separately prove the generated binaries load on that minimum version. Record PyInstaller, Python, Rust, Node, PyMuPDF and Tauri versions, plus the package hash.

On each **clean test device**:

1. Record OS version and architecture. Confirm Python and Tesseract are not installed and disconnect the network. Install the package through the normal OS flow; record any security or permission prompt. A rejected unsigned/unnotarized package is a distribution blocker, not a passing run.
2. Launch CourtBundle. Confirm `Local engine connected` within a bounded time, without an indefinite spinner. Create a synthetic matter using the illustrative profile.
3. Import `samples/04_scanned_annexure.jpg` through the native picker. Run English OCR. Confirm the job completes, the document status is `processed`, and the working PDF has extractable `ANNEXURE` text. Verify the imported original's SHA-256 is unchanged.
4. Import `samples/05_hindi_scan.pdf` through the native picker. Select **English + Hindi** and run OCR. Confirm the job completes, the document status is `processed`, and a search for `हिंदी` finds the one-page working PDF. Compare the extracted text with the fixture record in `PDF06_HINDI_OCR_FIXTURES.md`, confirm 595 × 842 pt geometry and verify the imported original's SHA-256 is unchanged.
5. Quit the app through its Quit menu. Confirm the app, PyInstaller bootstrap and worker processes exit and the loopback port no longer accepts connections. Relaunch and confirm the matter, documents and OCR statuses persist.
6. Reinstall the **same package** using the normal installer flow. Confirm the app still starts, the prior synthetic matter is intact, offline OCR works on a fresh synthetic scan, and final quit leaves no process or listener.

For each step, capture a pass/fail result, timestamp, screenshot or command output, and any failure text in `docs/POC_ACCEPTANCE_TESTS.md`. Do not mark NAT-02 done for a platform based on a successful build or a development machine alone. Keep native save/export testing under NAT-03 and broader native journey checks under NAT-01.
