//! Picking repository folders natively (owner feedback C1): a folder dialog and folders dropped on
//! the window. Both only hand a path to the page, which still registers it through the runtime
//! API, where every path is inspected. The dialog plugin is Rust-only: the capability grants the
//! page none of its commands, only `pick_folder` below. Desktop only, like the plugin dependency.

use std::path::PathBuf;
use tauri::{AppHandle, DragDropEvent, Manager, Runtime, Window, WindowEvent};
use tauri_plugin_dialog::DialogExt;

pub use tauri_plugin_dialog::init as dialog_plugin;

/// Opens the native folder dialog over the main window; `None` when the developer cancels.
#[tauri::command]
pub async fn pick_folder<R: Runtime>(app: AppHandle<R>) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = app.dialog().file().set_title("Choose a repository folder");
        if let Some(window) = app.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        dialog
            .blocking_pick_folder()
            .map(|picked| picked.into_path().map_err(|error| error.to_string()))
            .transpose()
            .map(|path| path.map(|path| path.to_string_lossy().into_owned()))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// The folders among dropped paths, in drop order; dropped files are ignored.
fn dropped_folders(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .filter(|path| path.is_dir())
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

/// Folders dropped on the main window reach the page as a `luuna:folders-dropped` event whose
/// detail is the list of folder paths.
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event else {
        return;
    };
    let folders = dropped_folders(paths);
    if folders.is_empty() {
        return;
    }
    // JSON is a JavaScript expression, so the paths reach the page as data, never as code.
    let Ok(detail) = serde_json::to_string(&folders) else {
        return;
    };
    if let Some(webview) = window.app_handle().get_webview_window("main") {
        let _ = webview.eval(format!(
            "window.dispatchEvent(new CustomEvent('luuna:folders-dropped', {{ detail: {detail} }}))"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_dropped_folders_are_passed_on() {
        let folder = std::env::temp_dir();
        let file = folder.join(format!("luuna-drop-{}.txt", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let missing = folder.join("luuna-drop-missing-folder");
        let picked = dropped_folders(&[file.clone(), folder.clone(), missing]);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(picked, [folder.to_string_lossy().into_owned()]);
    }

    #[test]
    fn a_dropped_path_is_json_data() {
        let detail = serde_json::to_string(&["/tmp/a'b\"c</script>"]).unwrap();
        assert_eq!(detail, r#"["/tmp/a'b\"c</script>"]"#);
    }
}
