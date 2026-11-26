fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&["ensure_local_runtime", "pick_folder"]),
    ))
    .expect("failed to configure desktop command permissions");
}
