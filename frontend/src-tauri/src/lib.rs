use std::sync::Mutex;

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

mod native_save;

#[derive(Clone, Serialize)]
struct BackendConnection {
    base_url: String,
    token: String,
}

struct BackendState {
    connection: BackendConnection,
    child: Mutex<Option<CommandChild>>,
}

#[tauri::command]
fn get_backend_connection(state: State<'_, BackendState>) -> BackendConnection {
    state.connection.clone()
}

#[cfg(not(debug_assertions))]
fn random_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                // Keep only the standard macOS application menu. CourtBundle actions
                // live in the app UI rather than the default desktop menus.
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
                app.set_menu(Menu::with_items(app, &[&app_menu])?)?;
            }
            app.manage(native_save::SaveState::default());
            #[cfg(debug_assertions)]
            {
                let base_url = std::env::var("COURTBUNDLE_DEV_BACKEND_URL")
                    .unwrap_or_else(|_| "http://127.0.0.1:8765".to_owned());
                let token = std::env::var("COURTBUNDLE_API_TOKEN")
                    .unwrap_or_else(|_| "development-token-change-me".to_owned());

                app.manage(BackendState {
                    connection: BackendConnection { base_url, token },
                    child: Mutex::new(None),
                });
            }

            #[cfg(not(debug_assertions))]
            {
                let listener = TcpListener::bind("127.0.0.1:0")?;
                let port = listener.local_addr()?.port();
                drop(listener);

                let token = random_secret();
                let data_dir = app.path().app_data_dir()?;
                std::fs::create_dir_all(&data_dir)?;
                let port_str = port.to_string();
                let data_dir_str = data_dir.to_string_lossy().into_owned();
                let parent_pid = std::process::id().to_string();

                // Bundled PyInstaller sidecar: no Python interpreter is needed at runtime.
                // The secret stays in the child environment and in guarded Tauri state.
                let command = app
                    .shell()
                    .sidecar("filing-api")?
                    .env("COURTBUNDLE_API_TOKEN", &token)
                    .env("COURTBUNDLE_REQUIRE_VAULT", "1")
                    .args([
                        "--port",
                        port_str.as_str(),
                        "--data-dir",
                        data_dir_str.as_str(),
                        "--parent-pid",
                        parent_pid.as_str(),
                    ]);
                let (mut receiver, child) = command.spawn()?;

                tauri::async_runtime::spawn(async move {
                    while let Some(event) = receiver.recv().await {
                        match event {
                            tauri_plugin_shell::process::CommandEvent::Stderr(_data) => {
                                eprintln!(
                                    "Filing engine reported a diagnostic; contents suppressed"
                                );
                            }
                            tauri_plugin_shell::process::CommandEvent::Terminated(payload) => {
                                eprintln!("Filing engine exited: {:?}", payload.code);
                            }
                            _ => {}
                        }
                    }
                });

                app.manage(BackendState {
                    connection: BackendConnection {
                        base_url: format!("http://127.0.0.1:{port}"),
                        token,
                    },
                    child: Mutex::new(Some(child)),
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_backend_connection,
            native_save::save_export,
            native_save::save_protected_export,
            native_save::save_matter,
            native_save::save_working_pdf,
            native_save::save_page_output
        ])
        .build(tauri::generate_context!())
        .expect("Failed to initialize CourtBundle")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
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
