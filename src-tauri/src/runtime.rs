use crate::alerts::{send_notification, ThresholdEvaluator};
use crate::config::{database_path, HiddenConfig};
use crate::parser::claude_code::{ClaudeCodeParser, ClaudeRateLimitStatus};
use crate::parser::codex::{CodexParser, CodexRateLimitStatus};
use crate::parser::UsageSource;
use crate::remote_sync;
use crate::scheduler::{UsageScheduler, MIN_POLL_INTERVAL_SECS};
use crate::settings::{load_settings, SourceSettings};
use crate::storage::UsageStore;
use crate::tray::{update_main_tray, TrayDisplayState};
use crate::window_estimator::WindowEstimator;
use chrono::{Duration as ChronoDuration, Utc};
use tauri::{AppHandle, Emitter};
use tokio::time::{self, Duration};

const CODEX_RATE_LIMIT_FRESHNESS_SECS: i64 = 5 * 60;

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

    let app_for_remote_sync = app.clone();
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
                            let _ = publish_tray_state(&app_for_reset, &[]);
                        });
                    }
                }
                Ok(None) => {
                    let _ = publish_tray_state(&app, &scheduler.current_snapshots());
                }
                Err(error) => eprintln!("token-notifier poll failed: {error:#}"),
            }
        }
    });

    start_remote_sync_runtime(app_for_remote_sync);

    Ok(())
}

fn start_remote_sync_runtime<R: tauri::Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        loop {
            let settings = load_settings().remote_sync;
            if settings.enabled {
                if let Some(db_path) = database_path() {
                    let now = Utc::now();
                    match remote_sync::sync_once(db_path, &settings, now).await {
                        Ok(()) => {
                            let _ = update_main_tray(&app, &crate::tray::latest_display_state());
                        }
                        Err(error) => eprintln!("token-notifier remote sync failed: {error:#}"),
                    }
                }
                time::sleep(Duration::from_secs(settings.interval_minutes * 60)).await;
            } else {
                time::sleep(Duration::from_secs(60)).await;
            }
        }
    });
}

fn handle_outcome<R: tauri::Runtime>(
    app: &AppHandle<R>,
    snapshots: &[crate::window_estimator::UsageSnapshot],
) -> anyhow::Result<()> {
    let settings = publish_tray_state(app, snapshots)?;

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

fn publish_tray_state<R: tauri::Runtime>(
    app: &AppHandle<R>,
    snapshots: &[crate::window_estimator::UsageSnapshot],
) -> anyhow::Result<crate::settings::AppSettings> {
    let settings = load_settings();
    let mut tray_state = TrayDisplayState::from_snapshots(snapshots, Utc::now());
    apply_live_claude_rate_limit(
        &mut tray_state,
        ClaudeCodeParser::latest_rate_limit_status(),
    );
    apply_live_codex_rate_limit(&mut tray_state, CodexParser::latest_rate_limit_status());
    tray_state.cc.enabled = settings.claude_code.enabled;
    tray_state.cx.enabled = settings.codex.enabled;
    update_main_tray(app, &tray_state)?;
    app.emit("usage-update", &tray_state)?;
    maybe_send_codex_observation_alert(app, &settings.codex, &tray_state)?;
    Ok(settings)
}

fn apply_live_claude_rate_limit(
    tray_state: &mut TrayDisplayState,
    status: Option<ClaudeRateLimitStatus>,
) {
    if let Some(status) = status {
        tray_state.cc.percent_used = Some(status.remaining_percent);
        tray_state.cc.reset_at = Some(status.reset_at);
        tray_state.cc.estimated = false;
        tray_state.cc.status_source = Some("official_observation".to_string());
        tray_state.cc.status_message = Some(if status.remaining_percent == 0 {
            "남은 토큰 없음".to_string()
        } else {
            "공식 확인".to_string()
        });
    }
}

fn apply_live_codex_rate_limit(
    tray_state: &mut TrayDisplayState,
    status: Option<CodexRateLimitStatus>,
) {
    match status {
        Some(status) if is_fresh_codex_observation(&status, tray_state.now) => {
            tray_state.cx.percent_used = Some(status.remaining_percent);
            tray_state.cx.reset_at = Some(status.reset_at);
            tray_state.cx.estimated = false;
            tray_state.cx.status_source = Some("official_observation".to_string());
            tray_state.cx.observed_at = Some(status.observed_at);
            tray_state.cx.status_message = Some("공식 확인".to_string());
        }
        Some(status) => {
            tray_state.cx.percent_used = None;
            tray_state.cx.reset_at = None;
            tray_state.cx.estimated = false;
            tray_state.cx.status_source = Some("stale_observation".to_string());
            tray_state.cx.observed_at = Some(status.observed_at);
            tray_state.cx.status_message = Some(format!(
                "마지막 공식 확인 {} 전",
                format_elapsed_since(tray_state.now, status.observed_at)
            ));
        }
        None => {
            tray_state.cx.percent_used = None;
            tray_state.cx.reset_at = None;
            tray_state.cx.estimated = false;
            tray_state.cx.status_source = Some("unavailable".to_string());
            tray_state.cx.observed_at = None;
            tray_state.cx.status_message = Some("공식 실시간 데이터 없음".to_string());
        }
    }
}

fn is_fresh_codex_observation(status: &CodexRateLimitStatus, now: chrono::DateTime<Utc>) -> bool {
    status.observed_at + ChronoDuration::seconds(CODEX_RATE_LIMIT_FRESHNESS_SECS) >= now
        && status.reset_at > now
}

fn format_elapsed_since(now: chrono::DateTime<Utc>, observed_at: chrono::DateTime<Utc>) -> String {
    let elapsed = (now - observed_at).num_minutes().max(0);
    if elapsed < 60 {
        format!("{elapsed}분")
    } else {
        let hours = elapsed / 60;
        let minutes = elapsed % 60;
        if minutes == 0 {
            format!("{hours}시간")
        } else {
            format!("{hours}시간 {minutes}분")
        }
    }
}

fn maybe_send_codex_observation_alert<R: tauri::Runtime>(
    app: &AppHandle<R>,
    settings: &SourceSettings,
    tray_state: &TrayDisplayState,
) -> anyhow::Result<()> {
    if !settings.enabled || tray_state.cx.status_source.as_deref() != Some("official_observation") {
        return Ok(());
    }
    let Some(observed_at) = tray_state.cx.observed_at else {
        return Ok(());
    };
    let Some(percent_remaining) = tray_state.cx.percent_used else {
        return Ok(());
    };
    let Some(reset_at) = tray_state.cx.reset_at else {
        return Ok(());
    };
    let percent_used = 100u8.saturating_sub(percent_remaining);
    let db_path =
        database_path().ok_or_else(|| anyhow::anyhow!("Could not resolve database path"))?;
    let store = UsageStore::open(db_path)?;
    let snapshot = crate::window_estimator::UsageSnapshot {
        source: UsageSource::Codex,
        window_id: format!("cx-official:{}", reset_at.timestamp()),
        window_start: observed_at,
        reset_at,
        tokens_used: percent_used as u64,
        quota_tokens: 100,
        percent_used,
        estimated: false,
    };
    maybe_send_alert(app, &store, settings, &snapshot)
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
    if snapshot.source == UsageSource::Codex && snapshot.estimated {
        return Ok(());
    }
    if let Some(notification) = ThresholdEvaluator::evaluate(store, snapshot, &settings.thresholds)?
    {
        send_notification(app, &notification)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn codex_status(
        observed_at: chrono::DateTime<Utc>,
        reset_at: chrono::DateTime<Utc>,
    ) -> CodexRateLimitStatus {
        CodexRateLimitStatus {
            observed_at,
            used_percent: 42,
            remaining_percent: 58,
            reset_at,
            window_minutes: 300,
        }
    }

    #[test]
    fn fresh_codex_observation_populates_percent_and_reset() {
        let now = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let mut state = TrayDisplayState::empty(now);
        apply_live_codex_rate_limit(
            &mut state,
            Some(codex_status(
                now - Duration::minutes(4),
                now + Duration::hours(2),
            )),
        );

        assert_eq!(state.cx.percent_used, Some(58));
        assert_eq!(state.cx.reset_at, Some(now + Duration::hours(2)));
        assert_eq!(
            state.cx.status_source.as_deref(),
            Some("official_observation")
        );
        assert_eq!(state.cx.status_message.as_deref(), Some("공식 확인"));
    }

    #[test]
    fn exhausted_claude_observation_displays_zero_remaining() {
        let now = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let mut state = TrayDisplayState::empty(now);
        apply_live_claude_rate_limit(
            &mut state,
            Some(ClaudeRateLimitStatus {
                used_percent: 100,
                remaining_percent: 0,
                reset_at: now + Duration::hours(2),
                window_minutes: 300,
            }),
        );

        assert_eq!(state.cc.percent_used, Some(0));
        assert_eq!(state.cc.reset_at, Some(now + Duration::hours(2)));
        assert!(!state.cc.estimated);
        assert_eq!(
            state.cc.status_source.as_deref(),
            Some("official_observation")
        );
        assert_eq!(state.cc.status_message.as_deref(), Some("남은 토큰 없음"));
    }

    #[test]
    fn stale_codex_observation_hides_percent_and_reset() {
        let now = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let mut state = TrayDisplayState::empty(now);
        apply_live_codex_rate_limit(
            &mut state,
            Some(codex_status(
                now - Duration::minutes(6),
                now + Duration::hours(2),
            )),
        );

        assert_eq!(state.cx.percent_used, None);
        assert_eq!(state.cx.reset_at, None);
        assert_eq!(state.cx.status_source.as_deref(), Some("stale_observation"));
        assert_eq!(
            state.cx.status_message.as_deref(),
            Some("마지막 공식 확인 6분 전")
        );
    }

    #[test]
    fn missing_codex_observation_reports_official_data_unavailable() {
        let now = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let mut state = TrayDisplayState::empty(now);
        apply_live_codex_rate_limit(&mut state, None);

        assert_eq!(state.cx.percent_used, None);
        assert_eq!(state.cx.reset_at, None);
        assert_eq!(state.cx.status_source.as_deref(), Some("unavailable"));
        assert_eq!(
            state.cx.status_message.as_deref(),
            Some("공식 실시간 데이터 없음")
        );
    }
}
