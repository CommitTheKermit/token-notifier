pub mod alerts;
pub mod autostart;
pub mod config;
pub mod native_status;
pub mod parser;
pub mod remote_sync;
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
fn get_remote_sync_states() -> Result<Vec<storage::RemoteSyncState>, String> {
    let path = config::database_path()
        .ok_or_else(|| "Could not resolve application data directory".to_string())?;
    let store = storage::UsageStore::open(path).map_err(|error| error.to_string())?;
    store
        .get_remote_sync_states()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_current_tray_state() -> tray::TrayDisplayState {
    tray::latest_display_state()
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

#[tauri::command]
fn open_settings_window<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    tray::open_settings_window(&app);
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            get_24h_series,
            get_rollups,
            get_remote_sync_states,
            get_current_tray_state,
            get_settings,
            save_settings,
            get_autostart_status,
            open_login_items_settings,
            open_settings_window
        ])
        // Autostart intentionally uses the SMAppService wrapper in autostart.rs.
        // tauri-plugin-autostart remains available but is not initialized in its
        // documented LaunchAgent mode, preserving the approved macOS 13+ decision.
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            tray::build_main_tray(app)?;
            runtime::start_background_runtime(app.handle().clone())
                .map_err(|error| tauri::Error::Anyhow(error.into()))?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Token Notifier");

    app.run(|_app_handle, event| {
        if let tauri::RunEvent::ExitRequested { code, api, .. } = event {
            if should_keep_menu_bar_app_alive(code) {
                api.prevent_exit();
            }
        }
    });
}

fn should_keep_menu_bar_app_alive(exit_code: Option<i32>) -> bool {
    // With the status item now owned by AppKit instead of Tauri's tray plugin,
    // macOS/Tauri may request a normal implicit exit when there are no WebView
    // windows. A menu-bar app must ignore that request, while explicit app
    // exits/restarts keep their non-None code and are allowed through.
    exit_code.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_bar_app_prevents_implicit_exit_requests() {
        assert!(should_keep_menu_bar_app_alive(None));
    }

    #[test]
    fn menu_bar_app_allows_explicit_exit_requests() {
        assert!(!should_keep_menu_bar_app_alive(Some(0)));
        assert!(!should_keep_menu_bar_app_alive(Some(130)));
    }
}
