fn main() {
    // Re-embed the selected launcher artwork when it changes, including in
    // `tauri dev`, where macOS loads its Dock icon from the compiled context.
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
