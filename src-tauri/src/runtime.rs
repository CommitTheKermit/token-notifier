use crate::alerts::{send_notification, ThresholdEvaluator};
use crate::config::{database_path, HiddenConfig};
use crate::parser::claude_code::ClaudeCodeParser;
use crate::parser::codex::CodexParser;
use crate::parser::UsageSource;
use crate::scheduler::{UsageScheduler, MIN_POLL_INTERVAL_SECS};
use crate::settings::{load_settings, SourceSettings};
use crate::storage::UsageStore;
use crate::tray::{update_main_tray, TrayDisplayState};
use crate::window_estimator::WindowEstimator;
use chrono::Utc;
use tauri::{AppHandle, Emitter};
use tokio::time::{self, Duration};

pub fn start_background_runtime<R: tauri::Runtime>(app: AppHandle<R>) -> anyhow::Result<()> {
    let db_path =
        database_path().ok_or_else(|| anyhow::anyhow!("Could not resolve database path"))?;
    let store = UsageStore::open(db_path)?;
    let mut scheduler = UsageScheduler::new(
        vec![
            Box::new(ClaudeCodeParser::new()),
            Box::new(CodexParser::new()),
        ],
        WindowEstimator::new(HiddenConfig::load()),
        store,
    );

    tauri::async_runtime::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(MIN_POLL_INTERVAL_SECS));
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            match scheduler.poll_once() {
                Ok(Some(outcome)) => {
                    if let Err(error) = handle_outcome(&app, &outcome.snapshots) {
                        eprintln!("token-notifier runtime update failed: {error:#}");
                    }
                    if let Some(reset_at) = outcome
                        .snapshots
                        .iter()
                        .filter(|snapshot| snapshot.reset_at > Utc::now())
                        .map(|snapshot| snapshot.reset_at)
                        .min()
                    {
                        let app_for_reset = app.clone();
                        scheduler.schedule_reset(reset_at, move |generation| {
                            let _ = app_for_reset.emit("usage-reset", generation);
                            let _ = update_main_tray(
                                &app_for_reset,
                                &TrayDisplayState::empty(Utc::now()),
                            );
                        });
                    }
                }
                Ok(None) => {}
                Err(error) => eprintln!("token-notifier poll failed: {error:#}"),
            }
        }
    });

    Ok(())
}

fn handle_outcome<R: tauri::Runtime>(
    app: &AppHandle<R>,
    snapshots: &[crate::window_estimator::UsageSnapshot],
) -> anyhow::Result<()> {
    let settings = load_settings();
    let mut tray_state = TrayDisplayState::from_snapshots(snapshots, Utc::now());
    tray_state.cc.enabled = settings.claude_code.enabled;
    tray_state.cx.enabled = settings.codex.enabled;
    update_main_tray(app, &tray_state)?;
    app.emit("usage-update", &tray_state)?;

    let db_path =
        database_path().ok_or_else(|| anyhow::anyhow!("Could not resolve database path"))?;
    let alert_store = UsageStore::open(db_path)?;
    for snapshot in snapshots {
        let source_settings = match snapshot.source {
            UsageSource::ClaudeCode => &settings.claude_code,
            UsageSource::Codex => &settings.codex,
        };
        maybe_send_alert(app, &alert_store, source_settings, snapshot)?;
    }
    Ok(())
}

fn maybe_send_alert<R: tauri::Runtime>(
    app: &AppHandle<R>,
    store: &UsageStore,
    settings: &SourceSettings,
    snapshot: &crate::window_estimator::UsageSnapshot,
) -> anyhow::Result<()> {
    if !settings.enabled {
        return Ok(());
    }
    if let Some(notification) = ThresholdEvaluator::evaluate(store, snapshot, &settings.thresholds)?
    {
        send_notification(app, &notification)?;
    }
    Ok(())
}
