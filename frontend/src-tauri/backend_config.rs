use std::{
    ffi::{OsStr, OsString},
    fs, io,
    path::Path,
};

pub const SELECTOR: &str = "MATTERDRAFT_BACKEND";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    Python,
    Rust,
    Synthetic,
}

pub fn select(
    configured: Option<&OsStr>,
    qualification: bool,
    allow_synthetic: bool,
) -> Result<Backend, &'static str> {
    match configured {
        None if qualification => Ok(Backend::Rust),
        None => Ok(Backend::Python),
        Some(value) => match value.to_str().map(str::trim) {
            Some("python") if qualification => {
                Err("This Rust qualification package has no Python sidecar. Use the standard MatterDraft package to select Python.")
            }
            Some("python") => Ok(Backend::Python),
            Some("rust" | "rust-vault") => Ok(Backend::Rust),
            Some("rust-synthetic") if allow_synthetic && !qualification => Ok(Backend::Synthetic),
            Some("rust-synthetic") => Err("The synthetic Rust store is available in development builds only."),
            _ => Err("Invalid MATTERDRAFT_BACKEND. Use python or rust."),
        },
    }
}

/// Read only the selector. An invalid explicit value is retained as an empty
/// value so native startup can report it through the connection error UI.
fn dotenv_backend(contents: &str) -> Option<&str> {
    contents
        .trim_start_matches('\u{feff}')
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let line = line
                .strip_prefix("export")
                .filter(|rest| rest.starts_with(char::is_whitespace))
                .unwrap_or(line);
            let (key, value) = line.split_once('=')?;
            (key.trim() == SELECTOR).then(|| {
                let value = value.trim();
                if let Some(quote) = value.chars().next().filter(|ch| matches!(ch, '\'' | '"')) {
                    let Some((value, tail)) = value[1..].split_once(quote) else {
                        return "";
                    };
                    let tail = tail.trim_start();
                    if tail.is_empty() || tail.starts_with('#') {
                        value
                    } else {
                        ""
                    }
                } else {
                    value.split('#').next().unwrap_or_default().trim_end()
                }
            })
        })
        .last()
}

#[allow(dead_code)] // The package launcher and build script read dotenv files.
pub fn configured_backend(
    frontend: &Path,
    mode: &str,
    process: Option<OsString>,
    mut watch: impl FnMut(&Path),
) -> Result<Option<OsString>, String> {
    let project = frontend.parent().ok_or("Missing project directory")?;
    let mut from_file = None;
    for name in [
        ".env".into(),
        ".env.local".into(),
        format!(".env.{mode}"),
        format!(".env.{mode}.local"),
    ] {
        for dir in [project, frontend] {
            let path = dir.join(&name);
            watch(&path);
            if process.is_some() {
                continue;
            }
            match fs::read_to_string(&path) {
                Ok(content) => {
                    if let Some(value) = dotenv_backend(&content) {
                        from_file = Some(value.into());
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("Cannot read {}: {error}", path.display())),
            }
        }
    }
    Ok(process.or(from_file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_python_and_routes_both_rust_selectors_to_the_protected_mode() {
        assert_eq!(select(None, false, true), Ok(Backend::Python));
        assert_eq!(select(None, true, false), Ok(Backend::Rust));
        for value in ["rust", "rust-vault"] {
            assert_eq!(
                select(Some(OsStr::new(value)), false, true),
                Ok(Backend::Rust)
            );
            assert_eq!(
                select(Some(OsStr::new(value)), false, false),
                Ok(Backend::Rust)
            );
        }
        assert_eq!(
            select(Some(OsStr::new("rust-synthetic")), false, true),
            Ok(Backend::Synthetic)
        );
        for value in ["", "invalid", "rust-synthetic"] {
            assert!(select(Some(OsStr::new(value)), false, false).is_err());
        }
        assert!(select(Some(OsStr::new("python")), true, false).is_err());
    }

    #[test]
    fn reads_only_the_literal_dotenv_selector_including_a_later_assignment() {
        let sample = "\u{feff}VITE_DEV_TOKEN=ignored\r\n# MATTERDRAFT_BACKEND=rust-synthetic\r\n\
            MATTERDRAFT_BACKEND=python\r\n\texport MATTERDRAFT_BACKEND = 'rust' # comment\r\n";
        assert_eq!(dotenv_backend(sample), Some("rust"));
        assert_eq!(dotenv_backend("MATTERDRAFT_BACKEND_NAME=rust"), None);
        assert_eq!(dotenv_backend("MATTERDRAFT_BACKEND=\"rust"), Some(""));
        assert_eq!(
            dotenv_backend("MATTERDRAFT_BACKEND=rust # use native IPC"),
            Some("rust")
        );
    }
}
