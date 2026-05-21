use tauri::tray::TrayIconBuilder;

const INITIAL_TRAY_TITLE: &str = "CC --%  CX --%";

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        // Step 2 intentionally does not initialize tauri-plugin-autostart.
        // The plan preserves macOS 13+ SMAppService for Step 9; the current
        // Tauri plugin default examples use LaunchAgent, so wiring autostart
        // here would change the approved mechanism prematurely.
        .setup(|app| {
            TrayIconBuilder::with_id("main")
                .title(INITIAL_TRAY_TITLE)
                .tooltip("Token Notifier")
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Token Notifier");
}
