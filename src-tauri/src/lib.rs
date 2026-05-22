pub mod alerts;
pub mod autostart;
pub mod config;
pub mod parser;
pub mod runtime;
pub mod scheduler;
pub mod settings;
pub mod storage;
pub mod tray;
pub mod window_estimator;

use chrono::Utc;
use tauri::Emitter;

#[tauri::command]
fn get_24h_series() -> Result<Vec<storage::HourlyPoint>, String> {
    let path = config::database_path()
        .ok_or_else(|| "Could not resolve application data directory".to_string())?;
    let store = storage::UsageStore::open(path).map_err(|error| error.to_string())?;
    store
        .get_24h_series(Utc::now())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_rollups() -> Result<Vec<storage::Rollups>, String> {
    let path = config::database_path()
        .ok_or_else(|| "Could not resolve application data directory".to_string())?;
    let store = storage::UsageStore::open(path).map_err(|error| error.to_string())?;
    store
        .get_rollups(Utc::now())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_settings() -> settings::AppSettings {
    settings::load_settings()
}

#[tauri::command]
fn save_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    settings: settings::AppSettings,
) -> Result<settings::AppSettings, String> {
    let previous = settings::load_settings();
    if previous.autostart_enabled != settings.autostart_enabled {
        autostart::set_login_item_enabled(settings.autostart_enabled)
            .map_err(|error| error.to_string())?;
    }
    let saved = settings::save_settings(&settings).map_err(|error| error.to_string())?;
    app.emit("settings-reloaded", &saved)
        .map_err(|error| error.to_string())?;
    Ok(saved)
}

#[tauri::command]
fn get_autostart_status() -> autostart::AutostartStatus {
    autostart::login_item_status()
}

#[tauri::command]
fn open_login_items_settings() {
    autostart::open_login_items_settings();
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            get_24h_series,
            get_rollups,
            get_settings,
            save_settings,
            get_autostart_status,
            open_login_items_settings
        ])
        // Autostart intentionally uses the SMAppService wrapper in autostart.rs.
        // tauri-plugin-autostart remains available but is not initialized in its
        // documented LaunchAgent mode, preserving the approved macOS 13+ decision.
        .setup(|app| {
            tray::build_main_tray(app)?;
            runtime::start_background_runtime(app.handle().clone())
                .map_err(|error| tauri::Error::Anyhow(error.into()))?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Token Notifier");
}
