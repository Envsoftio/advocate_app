use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rand::RngCore;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::BackendState;

#[derive(Default)]
pub(crate) struct SaveState {
    paths: Mutex<HashMap<String, PathBuf>>,
    last_directory: Mutex<Option<PathBuf>>,
}

struct PendingFile(PathBuf);

impl Drop for PendingFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn valid_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn valid_filename(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'))
        && matches!(
            value.rsplit('.').next(),
            Some("pdf" | "zip" | "csv" | "json" | "txt")
        )
}

fn working_pdf_path(document_id: &str, expected_revision: u64,
                    acknowledge_signature_risk: bool) -> Result<String, String> {
    if !valid_id(document_id) || expected_revision == 0 {
        return Err("Invalid working PDF selection".into());
    }
    Ok(format!(
        "/documents/{document_id}/pdf?revision={expected_revision}&editable=true&acknowledge_signature_risk={acknowledge_signature_risk}"
    ))
}

fn check_working_destination(destination: &Path, app_data: &Path) -> Result<(), String> {
    if destination.is_dir() || destination.file_name().is_none() {
        return Err("Choose a PDF file, not a folder".into());
    }
    // A debug shell can connect to an external backend before creating its own
    // app-data directory. Initialize that directory before resolving aliases.
    // This guard covers the app-owned production directory, not an unknown
    // external debug backend's data root.
    fs::create_dir_all(app_data)
        .map_err(|error| format!("Cannot initialize the internal data folder: {error}"))?;
    let internal = app_data
        .canonicalize()
        .map_err(|error| format!("Cannot verify the internal data folder: {error}"))?;
    let parent = destination
        .parent()
        .ok_or("Save location has no parent directory")?
        .canonicalize()
        .map_err(|error| format!("Cannot access the save folder: {error}"))?;
    // Resolve directory aliases before writing, so a symlink cannot lead a Save
    // into the application's preserved originals or revision catalog.
    if parent.starts_with(&internal)
        || destination
            .canonicalize()
            .is_ok_and(|path| path.starts_with(&internal))
    {
        return Err("Choose a file outside CourtBundle's internal data folder".into());
    }
    Ok(())
}

fn temp_file(destination: &Path) -> Result<(PendingFile, std::fs::File), String> {
    let directory = destination
        .parent()
        .ok_or("Save location has no parent directory")?;
    for _ in 0..10 {
        let mut random = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut random);
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let path = directory.join(format!(".courtbundle-{suffix}.partial"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((PendingFile(path), file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Cannot create a temporary file at the save location: {error}"
                ))
            }
        }
    }
    Err("Cannot create a unique temporary file at the save location".into())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

    if !destination.exists() {
        return fs::rename(source, destination);
    }
    let source_wide: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination_wide: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // ReplaceFileW keeps the existing destination if replacement fails. Both paths
    // are on the same volume because the temporary file lives beside the target.
    let ok = unsafe {
        ReplaceFileW(
            destination_wide.as_ptr(),
            source_wide.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

async fn transfer(
    connection: &crate::BackendConnection,
    path: &str,
    destination: &Path,
    password: Option<&str>,
) -> Result<(), String> {
    let url = format!("{}{}", connection.base_url.trim_end_matches('/'), path);
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|e| format!("Cannot initialize local export transfer: {e}"))?;
    let request = match password {
        Some(value) => client
            .post(url)
            .json(&serde_json::json!({"password": value})),
        None => client.get(url),
    };
    let mut response = request
        .bearer_auth(&connection.token)
        .send()
        .await
        .map_err(|e| format!("Cannot reach the local filing engine: {e}"))?;
    if !response.status().is_success() {
        if response.status() == reqwest::StatusCode::CONFLICT
            && path.starts_with("/documents/")
            && path.contains("/pdf?revision=")
        {
            return Err(
                "Working PDF revision changed. Refresh the document, review it, and save again."
                    .into(),
            );
        }
        return Err(format!(
            "Export unavailable (HTTP {}). Rebuild or retry.",
            response.status()
        ));
    }
    let expected = response.content_length();
    let (pending, mut file) = temp_file(destination)?;
    let mut received = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("Export transfer interrupted: {e}"))?
    {
        file.write_all(&chunk)
            .map_err(|e| format!("Cannot write export at this location: {e}"))?;
        received += chunk.len() as u64;
    }
    if expected.is_some_and(|length| length != received) {
        return Err("Export transfer ended before all bytes arrived".into());
    }
    file.sync_all()
        .map_err(|e| format!("Cannot finish writing export: {e}"))?;
    drop(file);
    replace_file(&pending.0, destination)
        .map_err(|e| format!("Cannot replace destination file: {e}"))?;
    // Best effort directory sync on platforms that support it.
    #[cfg(unix)]
    if let Some(parent) = destination.parent() {
        if let Ok(directory) = std::fs::File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn save_working_pdf(
    app: AppHandle,
    backend: State<'_, BackendState>,
    saves: State<'_, SaveState>,
    document_id: String,
    expected_revision: u64,
    filename: String,
    acknowledge_signature_risk: bool,
) -> Result<bool, String> {
    let path = working_pdf_path(&document_id, expected_revision, acknowledge_signature_risk)?;
    if !valid_filename(&filename) || !filename.ends_with(".pdf") || filename.len() > 160 {
        return Err("Invalid working PDF filename".into());
    }
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Save working PDF copy")
        .set_file_name(&filename)
        .add_filter("PDF", &["pdf"]);
    if let Some(directory) = saves
        .last_directory
        .lock()
        .map_err(|error| error.to_string())?
        .clone()
    {
        dialog = dialog.set_directory(directory);
    }
    let Some(chosen) = dialog.blocking_save_file() else {
        return Ok(false);
    };
    let destination = chosen
        .into_path()
        .map_err(|error| format!("Unsupported save location: {error}"))?;
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    check_working_destination(&destination, &app_data)?;
    // The revision-checked endpoint includes editable PDF marks in the saved
    // copy while leaving the stored working revision and private notes intact.
    transfer(&backend.connection, &path, &destination, None).await?;
    *saves
        .last_directory
        .lock()
        .map_err(|error| error.to_string())? = destination.parent().map(Path::to_path_buf);
    Ok(true)
}

#[tauri::command]
pub(crate) async fn save_export(
    app: AppHandle,
    backend: State<'_, BackendState>,
    saves: State<'_, SaveState>,
    matter_id: String,
    export_id: String,
    filename: String,
    save_as: bool,
) -> Result<bool, String> {
    if !valid_id(&matter_id) || !valid_id(&export_id) || !valid_filename(&filename) {
        return Err("Invalid export selection".into());
    }
    let key = format!("{matter_id}/{export_id}/{filename}");
    let existing = saves
        .paths
        .lock()
        .map_err(|e| e.to_string())?
        .get(&key)
        .cloned();
    let destination = if !save_as { existing } else { None };
    let destination = match destination {
        Some(path) => path,
        None => {
            let mut dialog = app
                .dialog()
                .file()
                .set_title("Save CourtBundle export")
                .set_file_name(&filename);
            if let Some(directory) = saves
                .last_directory
                .lock()
                .map_err(|e| e.to_string())?
                .clone()
            {
                dialog = dialog.set_directory(directory);
            }
            let extension = filename.rsplit('.').next().unwrap_or("pdf");
            dialog = dialog.add_filter("Export file", &[extension]);
            let Some(path) = dialog.blocking_save_file() else {
                return Ok(false);
            };
            path.into_path()
                .map_err(|e| format!("Unsupported save location: {e}"))?
        }
    };
    if destination.is_dir() {
        return Err("Choose a file, not a folder".into());
    }
    let app_data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    if destination.starts_with(&app_data) {
        return Err("Choose a location outside CourtBundle's internal data folder".into());
    }
    transfer(
        &backend.connection,
        &format!("/matters/{matter_id}/exports/{export_id}/{filename}"),
        &destination,
        None,
    )
    .await?;
    saves
        .paths
        .lock()
        .map_err(|e| e.to_string())?
        .insert(key, destination.clone());
    *saves.last_directory.lock().map_err(|e| e.to_string())? =
        destination.parent().map(Path::to_path_buf);
    Ok(true)
}

#[tauri::command]
pub(crate) async fn save_protected_export(
    app: AppHandle,
    backend: State<'_, BackendState>,
    saves: State<'_, SaveState>,
    matter_id: String,
    export_id: String,
    filename: String,
    password: String,
) -> Result<bool, String> {
    if !valid_id(&matter_id)
        || !valid_id(&export_id)
        || !valid_filename(&filename)
        || !filename.ends_with(".pdf")
    {
        return Err("Invalid PDF export selection".into());
    }
    if password.is_empty() || password.len() > 40 {
        return Err("Enter a PDF password of 1 to 40 UTF-8 bytes".into());
    }
    let output_name = format!("{}-protected.pdf", &filename[..filename.len() - 4]);
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Save password-protected CourtBundle PDF")
        .set_file_name(&output_name)
        .add_filter("PDF", &["pdf"]);
    if let Some(directory) = saves
        .last_directory
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
    {
        dialog = dialog.set_directory(directory);
    }
    let Some(path) = dialog.blocking_save_file() else {
        return Ok(false);
    };
    let destination = path
        .into_path()
        .map_err(|e| format!("Unsupported save location: {e}"))?;
    let app_data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    if destination.is_dir() || destination.starts_with(&app_data) {
        return Err("Choose a file outside CourtBundle's internal data folder".into());
    }
    transfer(
        &backend.connection,
        &format!("/matters/{matter_id}/exports/{export_id}/{filename}/protected"),
        &destination,
        Some(&password),
    )
    .await?;
    *saves.last_directory.lock().map_err(|e| e.to_string())? =
        destination.parent().map(Path::to_path_buf);
    Ok(true)
}

#[tauri::command]
pub(crate) async fn save_matter(
    app: AppHandle,
    backend: State<'_, BackendState>,
    saves: State<'_, SaveState>,
    matter_id: String,
) -> Result<bool, String> {
    if !valid_id(&matter_id) {
        return Err("Invalid matter selection".into());
    }
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Back up CourtBundle matter")
        .set_file_name("court-matter.cbmatter")
        .add_filter("CourtBundle matter", &["cbmatter"]);
    if let Some(directory) = saves
        .last_directory
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
    {
        dialog = dialog.set_directory(directory);
    }
    let Some(path) = dialog.blocking_save_file() else {
        return Ok(false);
    };
    let destination = path
        .into_path()
        .map_err(|e| format!("Unsupported save location: {e}"))?;
    let app_data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    if destination.is_dir() || destination.starts_with(&app_data) {
        return Err("Choose a file outside CourtBundle's internal data folder".into());
    }
    transfer(
        &backend.connection,
        &format!("/matters/{matter_id}/portable"),
        &destination,
        None,
    )
    .await?;
    *saves.last_directory.lock().map_err(|e| e.to_string())? =
        destination.parent().map(Path::to_path_buf);
    Ok(true)
}

#[tauri::command]
pub(crate) async fn save_page_output(
    app: AppHandle,
    backend: State<'_, BackendState>,
    saves: State<'_, SaveState>,
    document_id: String,
    operation: String,
    selection: String,
    acknowledge_signature_risk: bool,
) -> Result<bool, String> {
    if !valid_id(&document_id)
        || !matches!(operation.as_str(), "extract" | "split")
        || selection.is_empty()
        || selection.len() > 10000
        || selection
            .split(',')
            .any(|piece| piece.is_empty() || !piece.bytes().all(|c| c.is_ascii_digit()))
    {
        return Err("Invalid page selection".into());
    }
    let filename = if operation == "extract" {
        "extracted-pages.pdf"
    } else {
        "split-pages.zip"
    };
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Save selected pages")
        .set_file_name(filename)
        .add_filter(
            "Page output",
            &[if operation == "extract" { "pdf" } else { "zip" }],
        );
    if let Some(directory) = saves
        .last_directory
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
    {
        dialog = dialog.set_directory(directory);
    }
    let Some(path) = dialog.blocking_save_file() else {
        return Ok(false);
    };
    let destination = path
        .into_path()
        .map_err(|e| format!("Unsupported save location: {e}"))?;
    let app_data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    if destination.is_dir() || destination.starts_with(&app_data) {
        return Err("Choose a file outside CourtBundle's internal data folder".into());
    }
    let query = if operation == "extract" {
        "selection"
    } else {
        "after"
    };
    transfer(
        &backend.connection,
        &format!("/documents/{document_id}/pages/{operation}?{query}={selection}&acknowledge_signature_risk={acknowledge_signature_risk}"),
        &destination,
        None,
    )
    .await?;
    *saves.last_directory.lock().map_err(|e| e.to_string())? =
        destination.parent().map(Path::to_path_buf);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn rejects_path_like_names() {
        assert!(!valid_filename("../combined.pdf"));
        assert!(!valid_filename(".hidden.pdf"));
        assert!(!valid_filename("file.pdf/other"));
        assert!(valid_filename("volume-1.pdf"));
    }

    #[test]
    fn working_save_uses_exact_revision_and_rejects_invalid_identity() {
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            working_pdf_path(id, 7, false).unwrap(),
            format!("/documents/{id}/pdf?revision=7&editable=true&acknowledge_signature_risk=false")
        );
        assert!(working_pdf_path(id, 0, false).is_err());
        assert!(working_pdf_path("../originals", 1, false).is_err());
        assert!(working_pdf_path("0123456789ABCDEF0123456789ABCDEF", 1, false).is_err());
    }

    #[test]
    fn working_save_rejects_internal_destination_and_directory_alias() {
        let directory =
            std::env::temp_dir().join(format!("courtbundle-save-paths-{}", rand::random::<u64>()));
        let internal = directory.join("app-data");
        let outside = directory.join("chosen");
        fs::create_dir_all(internal.join("originals")).unwrap();
        fs::create_dir(&outside).unwrap();
        assert!(check_working_destination(&outside.join("copy.pdf"), &internal).is_ok());
        assert!(
            check_working_destination(&internal.join("originals/source.pdf"), &internal).is_err()
        );
        assert!(check_working_destination(&outside, &internal).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(internal.join("originals"), outside.join("alias")).unwrap();
            assert!(
                check_working_destination(&outside.join("alias/source.pdf"), &internal).is_err()
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn working_save_accepts_external_destination_before_app_data_exists() {
        let directory = std::env::temp_dir().join(format!(
            "courtbundle-save-new-data-{}",
            rand::random::<u64>()
        ));
        let internal = directory.join("missing/app-data");
        fs::create_dir_all(directory.join("chosen")).unwrap();
        assert!(!internal.exists());
        assert!(check_working_destination(&directory.join("chosen/copy.pdf"), &internal).is_ok());
        assert!(internal.is_dir());
        assert!(check_working_destination(&internal.join("copy.pdf"), &internal).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn working_save_preserves_response_bytes_and_stale_failure_keeps_existing_file() {
        let id = "0123456789abcdef0123456789abcdef";
        let path = working_pdf_path(id, 7, false).unwrap();
        let directory = std::env::temp_dir().join(format!(
            "courtbundle-working-save-{}",
            rand::random::<u64>()
        ));
        fs::create_dir(&directory).unwrap();
        let destination = directory.join("working-r7.pdf");
        const ORIGINAL_BYTES: &[u8] = b"%PDF-1.7\n\x00\xffsynthetic exact revision\n%%EOF";
        for (status, body) in [("200 OK", ORIGINAL_BYTES), ("409 Conflict", b"".as_slice())] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let expected_path = path.clone();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 4096];
                let count = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..count]);
                assert!(request.starts_with(&format!("GET {expected_path} HTTP/1.1")));
                assert!(request
                    .to_lowercase()
                    .contains("authorization: bearer test-token"));
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(body).unwrap();
            });
            let connection = crate::BackendConnection {
                base_url: format!("http://{address}"),
                token: "test-token".into(),
            };
            let result =
                tauri::async_runtime::block_on(transfer(&connection, &path, &destination, None));
            server.join().unwrap();
            if status.starts_with("200") {
                result.unwrap();
            } else {
                assert!(result.unwrap_err().contains("revision changed"));
            }
            assert_eq!(fs::read(&destination).unwrap(), ORIGINAL_BYTES);
            assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        }
        fs::remove_dir_all(directory).unwrap();
    }

    fn test_response(body: &'static [u8], declared_length: usize) -> crate::BackendConnection {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let size = stream.read(&mut request).unwrap();
            assert!(
                String::from_utf8_lossy(&request[..size])
                    .contains("authorization: Bearer test-token")
                    || String::from_utf8_lossy(&request[..size])
                        .contains("Authorization: Bearer test-token")
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {declared_length}\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });
        crate::BackendConnection {
            base_url: format!("http://{address}"),
            token: "test-token".into(),
        }
    }

    #[test]
    fn transfer_replaces_only_after_complete_download() {
        let directory =
            std::env::temp_dir().join(format!("courtbundle-save-test-{}", rand::random::<u64>()));
        fs::create_dir(&directory).unwrap();
        let destination = directory.join("combined.pdf");
        fs::write(&destination, b"previous export").unwrap();
        let path = "/synthetic-export";

        let complete = test_response(b"new export", 10);
        tauri::async_runtime::block_on(transfer(&complete, path, &destination, None)).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"new export");

        let incomplete = test_response(b"partial", 99);
        assert!(
            tauri::async_runtime::block_on(transfer(&incomplete, path, &destination, None))
                .is_err()
        );
        assert_eq!(fs::read(&destination).unwrap(), b"new export");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn protected_transfer_posts_password_and_saves_response_only() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..count]);
                let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]).to_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0);
                if request.len() >= header_end + 4 + length {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&request);
            assert!(text.starts_with("POST /synthetic-protected HTTP/1.1"));
            assert!(text.contains("synthetic password"));
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 14\r\nConnection: close\r\n\r\n%PDF-encrypted").unwrap();
        });
        let directory = std::env::temp_dir().join(format!(
            "courtbundle-protected-test-{}",
            rand::random::<u64>()
        ));
        fs::create_dir(&directory).unwrap();
        let destination = directory.join("protected.pdf");
        let connection = crate::BackendConnection {
            base_url: format!("http://{address}"),
            token: "test-token".into(),
        };
        tauri::async_runtime::block_on(transfer(
            &connection,
            "/synthetic-protected",
            &destination,
            Some("synthetic password"),
        ))
        .unwrap();
        server.join().unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"%PDF-encrypted");
        fs::remove_dir_all(directory).unwrap();
    }
}
