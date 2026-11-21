use std::{ffi::OsStr, sync::Mutex};

#[cfg(not(debug_assertions))]
use rand::RngCore;
use serde::Serialize;
#[cfg(not(debug_assertions))]
use std::net::TcpListener;
#[cfg(target_os = "macos")]
use tauri::menu::{Menu, PredefinedMenuItem, Submenu};
use tauri::{Manager, State};
use tauri_plugin_shell::process::CommandChild;
#[cfg(not(debug_assertions))]
use tauri_plugin_shell::ShellExt;

#[path = "../backend_config.rs"]
mod backend_config;
mod native_print;
mod native_save;
mod rust_backend;

use backend_config::Backend;
include!(concat!(env!("OUT_DIR"), "/backend-selection.rs"));

#[derive(Clone, Serialize)]
struct BackendConnection {
    base_url: String,
    token: String,
    rust_protected: bool,
}

impl BackendConnection {
    fn is_rust(&self) -> bool {
        self.base_url == "rust-ipc:"
    }
}

struct BackendState {
    connection: Result<BackendConnection, String>,
    child: Mutex<Option<CommandChild>>,
}

impl BackendState {
    fn connection(&self) -> Result<&BackendConnection, String> {
        self.connection.as_ref().map_err(Clone::clone)
    }
}

#[tauri::command]
async fn get_backend_connection(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
) -> Result<BackendConnection, String> {
    let connection = state.connection()?.clone();
    if connection.is_rust() {
        let runtime = app
            .try_state::<rust_backend::RustRuntime>()
            .ok_or("Rust engine could not be initialized")?;
        runtime.initialize().await?;
    }
    Ok(connection)
}

#[cfg(not(debug_assertions))]
fn random_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn connect_backend(app: &tauri::App) -> Result<(BackendConnection, Option<CommandChild>), String> {
    let process = std::env::var_os(backend_config::SELECTOR);
    let configured = process
        .as_deref()
        .or_else(|| CONFIGURED_BACKEND.map(OsStr::new));
    let qualification = cfg!(feature = "rust-qualification");
    let mode = backend_config::select(configured, qualification, cfg!(debug_assertions))?;
    if mode == Backend::Python {
        return connect_python(app);
    }

    let data = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?;
    match app.path().resource_dir() {
        Ok(resources) => {
            matterdraft_backend::matters::set_ocr_resource_root(resources.clone(), qualification);
            matterdraft_backend::matters::set_pdf_resource_root(resources);
        }
        Err(error) if qualification => {
            return Err(format!("Cannot locate Rust engine resources: {error}"))
        }
        Err(_) => {}
    }
    let runtime = match mode {
        Backend::Rust => rust_backend::RustRuntime::protected(data.join(if qualification {
            "rust-qualification-v1"
        } else {
            "rust-protected-v1"
        })),
        Backend::Synthetic => rust_backend::RustRuntime::new(data.join("rust-synthetic-v1")),
        Backend::Python => unreachable!(),
    };
    // RustRuntime opens its store in the bounded database lane when the
    // frontend requests a connection.
    app.manage(runtime);
    Ok((
        BackendConnection {
            base_url: "rust-ipc:".to_owned(),
            token: String::new(),
            rust_protected: mode == Backend::Rust,
        },
        None,
    ))
}

#[cfg(debug_assertions)]
fn connect_python(_app: &tauri::App) -> Result<(BackendConnection, Option<CommandChild>), String> {
    let base_url = std::env::var("MATTERDRAFT_DEV_BACKEND_URL")
        .or_else(|_| std::env::var("COURTBUNDLE_DEV_BACKEND_URL"))
        .unwrap_or_else(|_| "http://127.0.0.1:8765".to_owned());
    let token = std::env::var("MATTERDRAFT_API_TOKEN")
        .or_else(|_| std::env::var("COURTBUNDLE_API_TOKEN"))
        .unwrap_or_else(|_| "development-token-change-me".to_owned());
    Ok((
        BackendConnection {
            base_url,
            token,
            rust_protected: false,
        },
        None,
    ))
}

#[cfg(not(debug_assertions))]
fn connect_python(app: &tauri::App) -> Result<(BackendConnection, Option<CommandChild>), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|error| error.to_string())?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    drop(listener);

    let token = random_secret();
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    let port_str = port.to_string();
    let data_dir_str = data_dir.to_string_lossy().into_owned();
    let parent_pid = std::process::id().to_string();

    // Bundled PyInstaller sidecar: no Python interpreter is needed at runtime.
    // The secret stays in the child environment and in guarded Tauri state.
    let command = app
        .shell()
        .sidecar("filing-api")
        .map_err(|error| error.to_string())?
        .env("MATTERDRAFT_API_TOKEN", &token)
        .env("MATTERDRAFT_REQUIRE_VAULT", "1")
        .args([
            "--port",
            port_str.as_str(),
            "--data-dir",
            data_dir_str.as_str(),
            "--parent-pid",
            parent_pid.as_str(),
        ]);
    let (mut receiver, child) = command.spawn().map_err(|error| error.to_string())?;

    tauri::async_runtime::spawn(async move {
        while let Some(event) = receiver.recv().await {
            match event {
                tauri_plugin_shell::process::CommandEvent::Stderr(_data) => {
                    eprintln!("Filing engine reported a diagnostic; contents suppressed");
                }
                tauri_plugin_shell::process::CommandEvent::Terminated(payload) => {
                    eprintln!("Filing engine exited: {:?}", payload.code);
                }
                _ => {}
            }
        }
    });

    Ok((
        BackendConnection {
            base_url: format!("http://127.0.0.1:{port}"),
            token,
            rust_protected: false,
        },
        Some(child),
    ))
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                // WebKit routes standard clipboard shortcuts through the native
                // responder chain. Keep these Edit items even though matter/PDF
                // actions live in the app UI.
                let app_menu = Submenu::with_items(
                    app,
                    app.package_info().name.clone(),
                    true,
                    &[
                        &PredefinedMenuItem::about(app, None, None)?,
                        &PredefinedMenuItem::separator(app)?,
                        &PredefinedMenuItem::quit(app, None)?,
                    ],
                )?;
                let edit_menu = Submenu::with_items(
                    app,
                    "Edit",
                    true,
                    &[
                        &PredefinedMenuItem::cut(app, None)?,
                        &PredefinedMenuItem::copy(app, None)?,
                        &PredefinedMenuItem::paste(app, None)?,
                        &PredefinedMenuItem::separator(app)?,
                        &PredefinedMenuItem::select_all(app, None)?,
                    ],
                )?;
                app.set_menu(Menu::with_items(app, &[&app_menu, &edit_menu])?)?;
            }
            app.manage(native_save::SaveState::default());
            let (connection, child) = match connect_backend(app) {
                Ok((connection, child)) => (Ok(connection), child),
                Err(error) => (Err(error), None),
            };
            app.manage(BackendState {
                connection,
                child: Mutex::new(child),
            });
            Ok(())
        })
        .on_window_event(rust_backend::handle_native_drop)
        .invoke_handler(tauri::generate_handler![
            get_backend_connection,
            rust_backend::rust_request,
            rust_backend::rust_open_pdf,
            rust_backend::rust_open_export_pdf,
            rust_backend::rust_read_pdf,
            rust_backend::rust_close_pdf,
            rust_backend::rust_pick_import_pdf,
            rust_backend::rust_pick_import_files,
            rust_backend::rust_claim_import_drop,
            rust_backend::rust_import_picked_pdf,
            rust_backend::rust_import_raw_document,
            rust_backend::rust_replace_raw_document,
            rust_backend::rust_discard_picked_pdf,
            rust_backend::rust_pick_portable_backup,
            rust_backend::rust_restore_picked_backup,
            rust_backend::rust_pick_replacement,
            rust_backend::rust_apply_replacement,
            rust_backend::rust_job_status,
            rust_backend::rust_job_cancel,
            rust_backend::rust_job_result,
            rust_backend::rust_debug_job_start,
            native_save::save_export,
            native_save::reveal_saved_export,
            native_save::save_protected_export,
            native_save::save_matter,
            native_save::save_office_export,
            native_save::save_working_pdf,
            native_save::save_page_output,
            native_print::rust_prepare_print_pdf,
            native_print::print_rust_pdf
        ])
        .build(tauri::generate_context!())
        .expect("Failed to initialize MatterDraft")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { code, api, .. } = &event {
                if let Some(runtime) = app.try_state::<rust_backend::RustRuntime>() {
                    runtime.request_shutdown(app, *code, api);
                }
            }
            if let tauri::RunEvent::Exit = event {
                if let Some(runtime) = app.try_state::<rust_backend::RustRuntime>() {
                    runtime.shutdown();
                }
                if let Some(state) = app.try_state::<BackendState>() {
                    if let Ok(mut guard) = state.child.lock() {
                        if let Some(child) = guard.take() {
                            let _ = child.kill();
                        }
                    }
                }
            }
        });
}
