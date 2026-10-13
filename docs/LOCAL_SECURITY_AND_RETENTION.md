# Local security and retention state

**Status:** engineering draft, 13 October 2026. This POC must use synthetic matters only. The local data directory and portable backups are **not encrypted at rest**. Do not infer legal authenticity from hashes, OCR, audit events or a successful export.

## Data boundary and threat model

The Tauri app starts a Python sidecar on loopback with a random session bearer token. The browser development mode uses a fixed test token and must not be used for confidential matters. Core processing remains local. Other local processes running as the same OS user, filesystem backups, removable media, malware and anyone with access to the user profile can read the current SQLite database, originals, working revisions, exports and `.cbmatter` backup files. A local admin can also inspect process memory and environment. The POC does not claim to protect against these threats.

Imported originals stay byte-for-byte separate from working revisions. Page edits, OCR, bundle builds, backups and native Save can create additional readable copies. Build staging is removed on normal failure and startup recovery, but an operating-system crash may leave temporary data until the next launch or external cleanup. Portable restore validates paths, member sizes and SHA-256 values before publishing a new matter. These checks detect accidental or deliberate byte changes relative to the package manifest; they do not authenticate the package's author. Native Save streams into a same-directory temporary file and replaces the destination after a complete transfer.

## Retention and recovery policy in this POC

- Matter and document removal archives their SQLite records. Originals, working revisions, exports and Memo data remain on disk. Users can restore archived records from the workspace.
- There is no in-app permanent-delete action. Removing app data outside CourtBundle requires the user's separate decision and a verified backup. Historical orphan folders from earlier record-deletion behavior are not automatically adopted.
- Portable `.cbmatter` packages include active and archived documents, originals, retained working revisions and metadata. They do not include generated exports or job history. Restore creates new matter/document IDs and preserves archive state. Users should verify a restored synthetic matter before relying on any backup procedure.
- Edit history retains the newest 100 events per matter; importing or archiving a document clears replay history because the document set changed. Job records have a separate 90-day terminal-state policy. Audit events are currently retained without automatic deletion; a release retention policy remains to be decided.

## Audit and routine logs

SQLite `audit_events` records action codes, matter/document IDs and UTC times for core create, update, archive, restore and backup operations. It records no user identity, prior values, document names, text, passwords or bearer tokens. It is a troubleshooting trail, not an immutable or legally authenticated audit record. The API exposes the latest 500 events for an active matter to the local authenticated session. The packaged Tauri shell suppresses raw sidecar stderr; the sidecar disables HTTP access logging. Do not add request bodies, Authorization headers, OCR text or file contents to routine logs.

## Encryption and key recovery gate

At-rest encryption and key recovery are **not implemented**. Before authorized password-protected PDF import or any confidential-data release, decide whether the whole matter store and backups are encrypted, choose a platform key-storage and recovery model for Windows and macOS, and test key loss, backup restore, interrupted writes, migrations and temporary-file cleanup on every target. A password used to open a PDF must never become a stored matter field or audit value. Until that design is implemented and tested, PDF-09 and DAT-07 remain open.
