# Local security and retention state

**Status:** engineering draft, 14 October 2026. This POC must use synthetic matters only. The local data directory and portable backups are **not encrypted at rest**. Do not infer legal authenticity from hashes, OCR, audit events or a successful export.

## Data boundary and threat model

The Tauri app starts a Python sidecar on loopback with a random session bearer token. The browser development mode uses a fixed test token and must not be used for confidential matters. Core processing remains local. Other local processes running as the same OS user, filesystem backups, removable media, malware and anyone with access to the user profile can read the current SQLite database, originals, working revisions, exports and `.cbmatter` backup files. A local admin can also inspect process memory and environment. The POC does not claim to protect against these threats.

Imported originals stay byte-for-byte separate from working revisions. Page edits, OCR, bundle builds, backups and native Save can create additional readable copies. Build staging is removed on normal failure and startup recovery, but an operating-system crash may leave temporary data until the next launch or external cleanup. Portable restore validates paths, member sizes and SHA-256 values before publishing a new matter. These checks detect accidental or deliberate byte changes relative to the package manifest; they do not authenticate the package's author. Native Save streams into a same-directory temporary file and replaces the destination after a complete transfer.

On POSIX systems, the engine creates and tightens its data-root directory to owner-only mode (`0700`). This limits other ordinary OS accounts from traversing the matter store; it does not protect against the same account, administrators, backup software or copied exports. Windows ACL behavior remains to be verified on device. Portable transfer files use a dedicated hidden name and are removed after normal completion or on next engine startup after a crash. PDF edit scratch files use another dedicated name and are removed on normal completion or startup. An uncommitted revision candidate is quarantined rather than discarded so the last committed working copy remains available. An export or backup explicitly saved outside the data root inherits the destination's access controls and is not covered by the root mode.

## Retention and recovery policy in this POC

- Matter and document removal archives their SQLite records. Originals, working revisions, exports, Memo data and reusable matter values remain on disk. Users can restore archived records from the workspace.
- There is no in-app permanent-delete action. Removing app data outside CourtBundle requires the user's separate decision and a verified backup. Historical orphan folders from earlier record-deletion behavior are not automatically adopted.
- Portable `.cbmatter` packages include active and archived documents, originals, retained working revisions and metadata, including reusable matter values from v3. They do not include generated exports or job history. Restore creates new matter/document IDs and preserves archive state. Users should verify a restored synthetic matter before relying on any backup procedure.
- Edit history retains the newest 100 events per matter. Import, archive and restore now add reversible entries; a new edit clears the redo branch. Job records have a separate 90-day terminal-state policy. Audit events are currently retained without automatic deletion; a release retention policy remains to be decided.

## Audit and routine logs

SQLite `audit_events` records action codes, matter/document IDs and UTC times for core create, update, archive, restore and backup operations. It records no user identity, prior values, document names, text, passwords or bearer tokens. It is a troubleshooting trail, not an immutable or legally authenticated audit record. The API exposes the latest 500 events for an active matter to the local authenticated session. The packaged Tauri shell suppresses raw sidecar stderr; the sidecar disables HTTP access logging. Do not add request bodies, Authorization headers, OCR text or file contents to routine logs.

## Encryption and key recovery gate

**Product-owner direction, 14 October 2026:** use a user password with a separate recovery key for the first-release at-rest protection model. This is a requirement, not an implemented security control. The SQLite database, originals, working revisions, exports, portable backups and temporary files remain plaintext today.

The next engineering change must cover the whole local data boundary, including SQLite, current and historical working copies, originals, generated exports, `.cbmatter` backups and operation scratch space. Define how a password and recovery key unlock the same encrypted matter data, how either credential is rotated/revoked, how the recovery key is delivered without storing its plaintext in the app, and what happens after key loss. Preserve imported originals byte-for-byte within the protected store and avoid persisting a PDF-open password as matter metadata. Implement a versioned migration and portable format before exposing protected-PDF import. Test wrong passwords, recovery, interrupted migration/write, backup/restore and temporary cleanup on Windows and macOS. Until implementation and those tests pass, PDF-09 and DAT-07 remain open; this POC must use synthetic matters only.
