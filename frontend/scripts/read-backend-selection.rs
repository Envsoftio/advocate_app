// This host tool shares the desktop build's selector and .env precedence.
#[path = "../src-tauri/backend_config.rs"]
mod backend_config;

use backend_config::{Backend, SELECTOR};
use std::{env, path::PathBuf};

fn main() -> Result<(), String> {
    let mut args = env::args_os().skip(1);
    let frontend = args
        .next()
        .map(PathBuf::from)
        .ok_or("Missing frontend directory")?;
    let mode = args
        .next()
        .and_then(|mode| mode.into_string().ok())
        .ok_or("Missing desktop build mode")?;
    let configured =
        backend_config::configured_backend(&frontend, &mode, env::var_os(SELECTOR), |_| {})?;
    if let Some(value) = configured.as_deref() {
        match backend_config::select(Some(value), false, mode == "development") {
            Ok(Backend::Python) => println!("python"),
            Ok(Backend::Rust) => println!("rust"),
            Ok(Backend::Synthetic) => println!("rust-synthetic"),
            Err(error) => return Err(error.to_owned()),
        }
    }
    Ok(())
}
