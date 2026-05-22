pub mod alerts;
pub mod config;
pub mod parser;
pub mod scheduler;
pub mod storage;
pub mod tray;
pub mod window_estimator;


pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        // Step 2 intentionally does not initialize tauri-plugin-autostart.
        // The plan preserves macOS 13+ SMAppService for Step 9; the current
        // Tauri plugin default examples use LaunchAgent, so wiring autostart
        // here would change the approved mechanism prematurely.
        .setup(|app| {
            tray::build_main_tray(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Token Notifier");
}
