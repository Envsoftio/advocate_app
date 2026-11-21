use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Component, Path},
};
mod backend_config;
use backend_config::{Backend, SELECTOR};

fn embed_backend_selection() -> Result<(), String> {
    let manifest = env::var_os("CARGO_MANIFEST_DIR").ok_or("Missing Cargo manifest directory")?;
    let frontend = Path::new(&manifest)
        .parent()
        .ok_or("Missing frontend directory")?;
    let mode = if env::var("PROFILE").as_deref() == Ok("debug") {
        "development"
    } else {
        "production"
    };
    println!("cargo:rerun-if-env-changed={SELECTOR}");
    let configured =
        backend_config::configured_backend(frontend, mode, env::var_os(SELECTOR), |path| {
            // Cargo watches absent paths too, so adding a local file invalidates this build.
            println!("cargo:rerun-if-changed={}", path.display());
        })?;
    let compiled =
        configured.as_deref().map(
            |value| match backend_config::select(Some(value), false, true) {
                Ok(Backend::Python) => "python",
                Ok(Backend::Rust) => "rust",
                Ok(Backend::Synthetic) => "rust-synthetic",
                Err(_) => "",
            },
        );
    // Embed just the normalized selector. Do not package the environment file.
    let source = format!("const CONFIGURED_BACKEND: Option<&str> = {compiled:?};\n");
    let out = env::var_os("OUT_DIR").ok_or("Missing Cargo output directory")?;
    fs::write(Path::new(&out).join("backend-selection.rs"), source)
        .map_err(|error| error.to_string())
}

fn validate_ocr_resources() -> Result<(), String> {
    let target_os = env::var("CARGO_CFG_TARGET_OS").map_err(|error| error.to_string())?;
    let arch = env::var("CARGO_CFG_TARGET_ARCH").map_err(|error| error.to_string())?;
    let arch = if arch == "aarch64" { "arm64" } else { &arch };
    let (target, suffix) = match target_os.as_str() {
        "macos" => (format!("{arch}-darwin"), ".dylib"),
        "windows" => (format!("{arch}-windows"), ".dll"),
        _ => return Err("Rust desktop packages support macOS and Windows".into()),
    };
    let config = env::var("TAURI_CONFIG")
        .map_err(|_| "Supply the staged OCR bundle configuration to Tauri")?;
    let config: Value = serde_json::from_str(&config).map_err(|error| error.to_string())?;
    let bundle = &config["bundle"];
    if !bundle["externalBin"].as_array().is_some_and(Vec::is_empty) {
        return Err("The Rust package needs an empty bundle.externalBin array".into());
    }
    let resources = bundle["resources"]
        .as_object()
        .ok_or("Map the staged OCR runtime to matterdraft-ocr/ in bundle.resources")?;
    let mut runtime_paths = resources.iter().filter(|(_, destination)| {
        destination
            .as_str()
            .is_some_and(|path| path.trim_end_matches('/') == "matterdraft-ocr")
    });
    let (source, _) = runtime_paths
        .next()
        .ok_or("Missing matterdraft-ocr resource mapping")?;
    if runtime_paths.next().is_some() {
        return Err("Use one OCR resource directory for the target".into());
    }

    let root = Path::new(source);
    let manifest_path = root.join("manifest.json");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let manifest = fs::read(&manifest_path)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let manifest: Value = serde_json::from_slice(&manifest).map_err(|error| error.to_string())?;
    if manifest["target"] != target {
        return Err(format!("OCR runtime manifest must match {target}"));
    }
    let entry = manifest["entry"]
        .as_str()
        .filter(|name| {
            name.ends_with(suffix)
                && (name.starts_with("tesseract") || name.starts_with("libtesseract"))
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        })
        .ok_or("The OCR runtime manifest needs a target Tesseract library")?;
    let files = manifest["sha256"]
        .as_object()
        .ok_or("The OCR runtime manifest needs file checksums")?;
    for required in [
        format!("lib/{entry}").as_str(),
        "tessdata/eng.traineddata",
        "tessdata/hin.traineddata",
    ] {
        if !files.contains_key(required) {
            return Err(format!("Missing {required} in the OCR runtime manifest"));
        }
    }
    if !files.keys().any(|path| {
        path.strip_prefix("lib/").is_some_and(|name| {
            (name.starts_with("libleptonica") || name.starts_with("leptonica"))
                && name.ends_with(suffix)
        })
    }) {
        return Err("Missing the bundled Leptonica library".into());
    }
    for (relative, expected) in files {
        let relative = Path::new(relative);
        if !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(format!("Invalid OCR resource path {}", relative.display()));
        }
        let path = root.join(relative);
        println!("cargo:rerun-if-changed={}", path.display());
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(format!("{} is not a staged OCR file", path.display()));
        }
        let mut file =
            fs::File::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if expected.as_str() != Some(&format!("{:x}", hasher.finalize())) {
            return Err(format!("Checksum mismatch for {}", path.display()));
        }
    }
    if target_os == "macos" {
        let versions = manifest["macos_deployment_targets"]
            .as_object()
            .ok_or("Missing macOS deployment targets in the OCR manifest")?;
        for file in files.keys().filter_map(|path| path.strip_prefix("lib/")) {
            let version = versions
                .get(file)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("Missing macOS deployment target for {file}"))?;
            let mut parts = version.split('.');
            let major = parts.next().and_then(|part| part.parse::<u32>().ok());
            let minor = parts.next().and_then(|part| part.parse::<u32>().ok());
            let patch = parts.next().unwrap_or("0").parse::<u32>().ok();
            if !matches!((major, minor, patch), (Some(major), Some(minor), Some(patch))
                if (major, minor, patch) <= (12, 0, 0))
                || parts.next().is_some()
            {
                return Err(format!(
                    "{file} targets macOS {version}, outside the macOS 12.0 package requirement"
                ));
            }
        }
    }
    Ok(())
}

fn main() {
    // Re-embed the selected launcher artwork when it changes, including in
    // `tauri dev`, where macOS loads its Dock icon from the compiled context.
    println!("cargo:rerun-if-changed=icons");
    println!("cargo:rerun-if-env-changed=TAURI_CONFIG");
    embed_backend_selection()
        .unwrap_or_else(|error| panic!("Cannot configure MatterDraft backend: {error}"));
    if env::var_os("CARGO_FEATURE_RUST_QUALIFICATION").is_some() {
        if let Err(error) = validate_ocr_resources() {
            panic!("Incomplete Rust OCR bundle: {error}");
        }
    }
    tauri_build::build()
}
