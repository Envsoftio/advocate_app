# DAT-07 target-device security verification

DAT-07 remains open until the checks below pass on the supported target matrix. Use only synthetic files in `samples/` and a disposable app data directory or test account. Record each result in `POC_ACCEPTANCE_TESTS.md`; a build, browser test, or frozen-sidecar API check does not replace the packaged native UI run. PDF-09 engineering was subsequently authorized before DAT-07 acceptance; neither ticket may be marked accepted from this engineering evidence alone.

## Target matrix and evidence

Use `NAT02_DEVICE_VERIFICATION.md` to build and install matching sidecars and packages. Run this checklist on Windows 10 x64, Windows 11 x64, macOS 12 Intel, and macOS 12 Apple Silicon, or record a reasoned compatibility substitution approved for the release matrix. Repeat any platform-specific failure on the affected platform after fixing it.

For each run record: date, device identifier, OS/build, CPU architecture, app and sidecar SHA-256, clean-install or existing-install status, whether the device had Python/Tesseract installed, each check's pass/fail, screenshots or command output, and failure text. Keep passwords, recovery keys, session tokens, matter text and raw document bytes out of the acceptance log.

## Native vault and protected workflow

1. Install and launch the packaged app offline. Before vault setup, confirm matter routes are inaccessible. Create a vault with a synthetic password; confirm the recovery key is displayed once and is absent from app data, logs and exported metadata.
2. Import `samples/01_petition.pdf` and `samples/04_scanned_annexure.jpg` through the native picker. Run OCR, rotate one working page, build and review an export. Quit and relaunch. Confirm the matter and changes persist after unlocking.
3. Enter a wrong password and a wrong recovery key; both must fail visibly without revealing matter data. Unlock using the correct password, then separately using the recovery key after another relaunch. Change the password in the native UI. Verify the old password and old recovery key fail, while the new password and new recovery key work. Use a disposable vault: rotating a real vault would revoke its prior recovery key.
4. Create a protected `.cbmatter` backup through the native Save picker. Restore it through the native Open picker in the same vault, then in a fresh vault using the source credential. Reject missing and wrong credentials visibly. Verify original SHA-256 values, restored page counts and no surviving transfer scratch after success or failure.
5. Save a PDF and ZIP to a chosen destination. Confirm they reopen and match the internal artifact hashes. Explicit native exports are plaintext at that destination; verify the UI and documentation do not imply otherwise. Cancel Save/Open and confirm no destination or matter is created.

## At-rest permissions, failure and recovery

1. With the app closed, inspect the app data root, SQLite database and WAL, originals, working revisions, internal exports and scratch candidates. They must be inaccessible to other ordinary users. On macOS check owner-only permissions and absence of plain `SQLite format 3`, `%PDF-`, ZIP headers and the chosen synthetic marker. On Windows inspect effective ACLs from a separate standard account, not only the file's owner view. The marker scan is a limited leakage check, not proof that arbitrary plaintext is absent.
2. Inspect routine logs and the audit API after the workflow. Action codes, IDs and times may appear; passwords, recovery keys, bearer tokens, OCR text and document contents must not. Verify the sidecar binds only to loopback and rejects an unauthenticated request.
3. On a disposable test volume, force insufficient space during native Save, protected backup and build. Verify a visible failure, preservation of an existing destination, no apparently complete new file and no persistent plaintext scratch. Restore free space before retrying.
4. Interrupt the packaged process during a large import, backup restore, build and native Save. Relaunch and verify SQLite opens, committed originals/revisions retain their hashes, incomplete outputs are absent, app-named scratch is cleaned or quarantined according to policy, and retry works. Also interrupt a legacy-store migration on a disposable copy and confirm it resumes without losing originals or leaving an accessible plaintext store. Never use a real matter for this check.
5. Measure peak app/sidecar memory, temporary disk use and elapsed time for representative large synthetic matters during backup creation, restore and export ZIP generation. Record corpus sizes and an explicit pass limit for each supported device. Chunked ZIP paths reduce buffering, but PDF parsing and some edits still materialize whole files; a passing small sample does not establish large-matter acceptance. Fix or cap the operation if the release limit is exceeded.

## PDF-09 native acceptance extension

1. In a disposable protected vault, import a synthetic password-protected PDF through the packaged UI. Confirm missing and wrong passwords are reported, cancellation creates no matter document, and the correct password imports. Compare the immutable encrypted original with the input byte-for-byte; confirm the decrypted working copy is encrypted at rest.
2. Import a protected PDF carrying synthetic signature indicators. Confirm the working-copy transformation requires explicit acknowledgment and makes no claim of certificate validity. Confirm a PDF whose permissions prohibit copying, modification or assembly is rejected.
3. Build under the general demonstration profile, choose **Protect PDF**, then cancel the native Save dialog. Retry with a password, save, and reopen the resulting PDF with that password; a wrong or missing password must fail. Confirm the user password is absent from app metadata, routine logs and manifests, and no plaintext app scratch remains. In the illustrative profile, confirm the protection option is disabled with its unapproved technical-setting explanation. ZIP protection is outside this ticket.

## Acceptance decision

Mark each row pass, fail or blocked per device, link the evidence, and resolve every failure before checking off DAT-07 in `IMPLEMENTATION_TASKS.md`. The Apple Silicon development-host checks in sections AO–AQ of `POC_ACCEPTANCE_TESTS.md` are partial evidence only. Keep PDF-09 acceptance open until DAT-07 and its own native password/save checks pass.
