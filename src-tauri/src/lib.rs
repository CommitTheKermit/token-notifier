pub mod alerts;
pub mod config;
pub mod parser;
pub mod scheduler;
pub mod storage;
pub mod tray;
pub mod window_estimator;

use chrono::Utc;

#[tauri::command]
fn get_24h_series() -> Result<Vec<storage::HourlyPoint>, String> {
    let path = config::database_path().ok_or_else(|| "Could not resolve application data directory".to_string())?;
    let store = storage::UsageStore::open(path).map_err(|error| error.to_string())?;
    store.get_24h_series(Utc::now()).map_err(|error| error.to_string())
}

#[tauri::command]
fn get_rollups() -> Result<Vec<storage::Rollups>, String> {
    let path = config::database_path().ok_or_else(|| "Could not resolve application data directory".to_string())?;
    let store = storage::UsageStore::open(path).map_err(|error| error.to_string())?;
    store.get_rollups(Utc::now()).map_err(|error| error.to_string())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![get_24h_series, get_rollups])
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
