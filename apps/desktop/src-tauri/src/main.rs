#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// ADR 0038: on Windows the in-app top bar is the title bar and there is no native menu bar; every
// menu command is also a page shortcut or the palette. macOS and Linux keep the menu (ADR 0030).
#[cfg(not(target_os = "windows"))]
mod menu;
mod runtime;
mod window;

fn main() {
    let builder = tauri::Builder::default();
    // ADR 0039: the single-instance plugin is registered first, so a second launch hands over to
    // the running window and exits before it creates a window or asks for the local runtime.
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            window::reveal_main(app)
        }))
        .plugin(window::state_plugin());
    #[cfg(not(target_os = "windows"))]
    let builder = builder
        .menu(menu::build)
        .on_menu_event(|app, event| menu::forward(app, event.id().as_ref()));
    builder
        .setup(|_app| {
            // Opt in when diagnosing rendering; ordinary local GUI launches stay focused on the app.
            #[cfg(debug_assertions)]
            {
                use tauri::Manager;
                if std::env::var("LUUNA_DESKTOP_DEVTOOLS").as_deref() == Ok("1") {
                    if let Some(window) = _app.get_webview_window("main") {
                        window.open_devtools();
                    }
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![runtime::ensure_local_runtime])
        .run(tauri::generate_context!())
        .expect("failed to start Luuna desktop");
}
