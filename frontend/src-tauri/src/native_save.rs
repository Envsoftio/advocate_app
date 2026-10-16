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
) -> Result<(), String> {
    let url = format!("{}{}", connection.base_url.trim_end_matches('/'), path);
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|e| format!("Cannot initialize local export transfer: {e}"))?;
    let mut response = client
        .get(url)
        .bearer_auth(&connection.token)
        .send()
        .await
        .map_err(|e| format!("Cannot reach the local filing engine: {e}"))?;
    if !response.status().is_success() {
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
        tauri::async_runtime::block_on(transfer(&complete, path, &destination)).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"new export");

        let incomplete = test_response(b"partial", 99);
        assert!(tauri::async_runtime::block_on(transfer(&incomplete, path, &destination)).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"new export");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
