use crate::config::{database_path, HiddenConfig};
use crate::parser::UsageSource;
use crate::storage::{HourlyPoint, UsageStore};
use crate::window_estimator::UsageSnapshot;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use tauri::{App, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::native_status::{NativePopoverSourceState, NativePopoverState, NativeStatusClick};

static LAST_DISPLAY_STATE: OnceLock<Mutex<TrayDisplayState>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PercentColor {
    Default,
    Yellow,
    Red,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceTrayState {
    pub source: UsageSource,
    pub enabled: bool,
    pub percent_used: Option<u8>,
    pub reset_at: Option<DateTime<Utc>>,
    pub estimated: bool,
    pub status_source: Option<String>,
    pub observed_at: Option<DateTime<Utc>>,
    pub status_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrayDisplayState {
    pub cc: SourceTrayState,
    pub cx: SourceTrayState,
    pub now: DateTime<Utc>,
}

impl TrayDisplayState {
    pub fn empty(now: DateTime<Utc>) -> Self {
        Self {
            cc: SourceTrayState {
                source: UsageSource::ClaudeCode,
                enabled: true,
                percent_used: None,
                reset_at: None,
                estimated: false,
                status_source: None,
                observed_at: None,
                status_message: None,
            },
            cx: SourceTrayState {
                source: UsageSource::Codex,
                enabled: true,
                percent_used: None,
                reset_at: None,
                estimated: false,
                status_source: Some("unavailable".to_string()),
                observed_at: None,
                status_message: Some("공식 실시간 데이터 없음".to_string()),
            },
            now,
        }
    }

    pub fn from_snapshots(snapshots: &[UsageSnapshot], now: DateTime<Utc>) -> Self {
        let mut state = Self::empty(now);
        for snapshot in snapshots {
            let target = match snapshot.source {
                UsageSource::ClaudeCode => &mut state.cc,
                UsageSource::Codex => &mut state.cx,
            };
            target.percent_used = Some(100u8.saturating_sub(snapshot.percent_used));
            target.reset_at = Some(snapshot.reset_at);
            target.estimated = snapshot.estimated;
            target.status_source = Some(
                if snapshot.estimated {
                    "local_estimate"
                } else {
                    "official_provider"
                }
                .to_string(),
            );
            target.observed_at = None;
            target.status_message = None;
        }
        state
    }
}

pub fn color_for_percent(percent: u8) -> PercentColor {
    match percent {
        90..=u8::MAX => PercentColor::Red,
        70..=89 => PercentColor::Yellow,
        _ => PercentColor::Default,
    }
}

pub fn format_tray_label(state: &TrayDisplayState) -> String {
    let state = display_ready_state(state);
    let mut percents = Vec::new();
    let mut reset_hours = Vec::new();
    push_grid_source_label(&mut percents, &mut reset_hours, &state.cc, state.now);
    push_grid_source_label(&mut percents, &mut reset_hours, &state.cx, state.now);
    if percents.is_empty() {
        "Token Notifier".to_string()
    } else {
        format!("{}\n{}", percents.join(" "), reset_hours.join(" "))
    }
}

pub fn format_tray_tooltip(state: &TrayDisplayState) -> String {
    let state = display_ready_state(state);
    let mut parts = Vec::new();
    push_source_label(&mut parts, &state.cc, state.now);
    push_source_label(&mut parts, &state.cx, state.now);
    if parts.is_empty() {
        "Token Notifier".to_string()
    } else {
        parts.join("  ")
    }
}

pub fn build_main_tray(app: &App) -> tauri::Result<()> {
    let mut initial_state = TrayDisplayState::empty(Utc::now());
    apply_claude_local_history_fallback(&mut initial_state, &HiddenConfig::load());
    store_latest_display_state(&initial_state);
    let initial_title = format_tray_label(&initial_state);
    let initial_tooltip = format_tray_tooltip(&initial_state);
    let (click_sender, click_receiver) = std::sync::mpsc::channel();
    crate::native_status::install_initial(
        &app.handle().clone(),
        initial_title,
        initial_tooltip,
        click_sender,
    );
    let app_handle = app.handle().clone();
    std::thread::spawn(move || {
        while let Ok(click) = click_receiver.recv() {
            let app = app_handle.clone();
            let window_app = app.clone();
            let _ = app.run_on_main_thread(move || match click {
                NativeStatusClick::OpenPopover => {
                    crate::native_status::toggle_popover(native_popover_state())
                }
                NativeStatusClick::OpenSettings => open_or_focus_window(
                    &window_app,
                    "settings",
                    "settings.html",
                    "Token Notifier Settings",
                    460.0,
                    520.0,
                ),
            });
        }
    });
    Ok(())
}

pub fn update_main_tray<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &TrayDisplayState,
) -> tauri::Result<()> {
    let state = display_ready_state(state);
    store_latest_display_state(&state);
    let title = format_tray_label(&state);
    let tooltip = format_tray_tooltip(&state);
    crate::native_status::update_title(app, title, tooltip);
    crate::native_status::update_popover(app, native_popover_state_from_tray_state(&state));
    Ok(())
}

pub fn latest_display_state() -> TrayDisplayState {
    let mut state = LAST_DISPLAY_STATE
        .get()
        .and_then(|state| state.lock().ok().map(|state| state.clone()))
        .unwrap_or_else(|| TrayDisplayState::empty(Utc::now()));
    apply_claude_local_history_fallback(&mut state, &HiddenConfig::load());
    state
}

pub fn open_settings_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    open_or_focus_window(
        app,
        "settings",
        "settings.html",
        "Token Notifier Settings",
        460.0,
        520.0,
    );
}

fn store_latest_display_state(state: &TrayDisplayState) {
    let latest = LAST_DISPLAY_STATE.get_or_init(|| Mutex::new(TrayDisplayState::empty(Utc::now())));
    if let Ok(mut latest) = latest.lock() {
        *latest = state.clone();
    }
}

fn display_ready_state(state: &TrayDisplayState) -> TrayDisplayState {
    let mut state = state.clone();
    apply_claude_local_history_fallback(&mut state, &HiddenConfig::load());
    state
}

pub fn apply_claude_local_history_fallback(
    tray_state: &mut TrayDisplayState,
    config: &HiddenConfig,
) {
    if tray_state.cc.percent_used.is_some() {
        return;
    }
    let Some(db_path) = database_path() else {
        return;
    };
    let Ok(store) = UsageStore::open(db_path) else {
        return;
    };
    let Ok(points) = store.get_24h_series(tray_state.now) else {
        return;
    };
    apply_claude_local_history_points(tray_state, config, &points);
}

fn apply_claude_local_history_points(
    tray_state: &mut TrayDisplayState,
    config: &HiddenConfig,
    points: &[HourlyPoint],
) {
    if tray_state.cc.percent_used.is_some() {
        return;
    }
    let window_secs = config.default_window_secs.max(60);
    let Ok(window_secs_i64) = i64::try_from(window_secs) else {
        return;
    };
    let start = tray_state.now - ChronoDuration::seconds(window_secs_i64);
    let mut tokens_used = 0u64;
    let mut earliest_hour: Option<DateTime<Utc>> = None;
    for point in points
        .iter()
        .filter(|point| point.source == UsageSource::ClaudeCode && point.hour_start >= start)
    {
        tokens_used = tokens_used.saturating_add(point.tokens_used);
        earliest_hour = Some(
            earliest_hour
                .map(|current| current.min(point.hour_start))
                .unwrap_or(point.hour_start),
        );
    }
    if tokens_used == 0 {
        return;
    }

    let quota = config.quota_for(UsageSource::ClaudeCode).max(1);
    let used_percent = ((tokens_used.saturating_mul(100)) / quota).min(100) as u8;
    tray_state.cc.percent_used = Some(100u8.saturating_sub(used_percent));
    tray_state.cc.reset_at =
        earliest_hour.map(|hour| hour + ChronoDuration::seconds(window_secs_i64));
    tray_state.cc.estimated = true;
    tray_state.cc.status_source = Some("local_history".to_string());
    tray_state.cc.status_message = Some("로컬 기록 기반 추정".to_string());
}

fn native_popover_state() -> NativePopoverState {
    native_popover_state_from_tray_state(&latest_display_state())
}

fn native_popover_state_from_tray_state(state: &TrayDisplayState) -> NativePopoverState {
    let state = display_ready_state(state);
    let (rollup_day, rollup_week, rollup_month) = rollup_totals()
        .map(|(day, week, month)| {
            (
                format_tokens(day),
                format_tokens(week),
                format_tokens(month),
            )
        })
        .unwrap_or_else(|| ("--".to_string(), "--".to_string(), "--".to_string()));

    NativePopoverState {
        sources: [&state.cc, &state.cx]
            .into_iter()
            .filter(|source| source.enabled)
            .map(|source| native_popover_source(source, state.now))
            .collect(),
        rollup_day,
        rollup_week,
        rollup_month,
        updated_text: format!(
            "업데이트 {}",
            state.now.with_timezone(&chrono::Local).format("%H:%M")
        ),
    }
}

fn native_popover_source(source: &SourceTrayState, now: DateTime<Utc>) -> NativePopoverSourceState {
    let label = match source.source {
        UsageSource::ClaudeCode => "Claude Code",
        UsageSource::Codex => "Codex",
    };
    let percent = source.percent_used.unwrap_or(0);
    let percent_text = source_percent_text(source);
    let reset_text = source
        .reset_at
        .map(|reset_at| format!("다음 갱신까지 {}", format_countdown(now, reset_at)))
        .unwrap_or_else(|| {
            source.status_message.clone().unwrap_or_else(|| {
                if source.source == UsageSource::ClaudeCode {
                    "로컬 기록 없음".to_string()
                } else {
                    "다음 갱신까지 --".to_string()
                }
            })
        });

    NativePopoverSourceState {
        label: label.to_string(),
        percent_text,
        reset_text,
        fraction: f64::from(percent) / 100.0,
    }
}

fn rollup_totals() -> Option<(u64, u64, u64)> {
    let path = crate::config::database_path()?;
    let store = crate::storage::UsageStore::open(path).ok()?;
    let rollups = store.get_rollups(Utc::now()).ok()?;
    Some(
        rollups
            .into_iter()
            .filter(|item| item.source == UsageSource::ClaudeCode)
            .fold((0, 0, 0), |acc, item| {
                (
                    acc.0 + item.day_tokens,
                    acc.1 + item.week_tokens,
                    acc.2 + item.month_tokens,
                )
            }),
    )
}

fn format_tokens(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect::<String>()
}

fn push_grid_source_label(
    percents: &mut Vec<String>,
    reset_hours: &mut Vec<String>,
    source: &SourceTrayState,
    now: DateTime<Utc>,
) {
    if !source.enabled {
        return;
    }
    let percent = source
        .percent_used
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "--%".to_string());
    let estimate = if source.estimated { "~" } else { "" };
    percents.push(fixed_grid_cell(&format!("{estimate}{percent}")));
    reset_hours.push(fixed_grid_cell(
        &source
            .reset_at
            .map(|reset_at| format_reset_hours(now, reset_at))
            .unwrap_or_else(|| "--h".to_string()),
    ));
}

fn source_percent_text(source: &SourceTrayState) -> String {
    source
        .percent_used
        .map(|value| format!("{value}%"))
        .or_else(|| source.status_message.clone())
        .unwrap_or_else(|| {
            if source.source == UsageSource::ClaudeCode {
                "로컬 기록 없음".to_string()
            } else {
                "--".to_string()
            }
        })
}

fn push_source_label(parts: &mut Vec<String>, source: &SourceTrayState, now: DateTime<Utc>) {
    if !source.enabled {
        return;
    }

    let prefix = match source.source {
        UsageSource::ClaudeCode => "CC",
        UsageSource::Codex => "CX",
    };
    let Some(percent_value) = source.percent_used else {
        let message = source
            .status_message
            .clone()
            .unwrap_or_else(|| "데이터 없음".to_string());
        parts.push(format!("{prefix} {message}"));
        return;
    };
    let percent = format!("{percent_value:>3}%");
    let estimate = if source.estimated { "~" } else { "" };
    let reset = source
        .reset_at
        .map(|reset_at| format!("↻{}", format_countdown(now, reset_at)))
        .unwrap_or_else(|| "↻--".to_string());
    parts.push(format!("{prefix} {estimate}{percent} {reset}"));
}

fn fixed_grid_cell(value: &str) -> String {
    format!("{value:>6}")
}

fn format_reset_hours(now: DateTime<Utc>, reset_at: DateTime<Utc>) -> String {
    if reset_at <= now {
        return "0.0h".to_string();
    }
    let total_minutes = (reset_at - now).num_minutes().max(0);
    let hours = total_minutes as f64 / 60.0;
    format!("{hours:.1}h")
}

fn format_countdown(now: DateTime<Utc>, reset_at: DateTime<Utc>) -> String {
    if reset_at <= now {
        return "0m".to_string();
    }
    let total_minutes = (reset_at - now).num_minutes().max(0);
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;
    if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else {
        format!("{minutes}m")
    }
}

fn open_or_focus_window<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    label: &str,
    url: &str,
    title: &str,
    width: f64,
    height: f64,
) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.show();
        let _ = window.set_focus();
    } else if let Ok(window) = WebviewWindowBuilder::new(app, label, WebviewUrl::App(url.into()))
        .title(title)
        .inner_size(width, height)
        .resizable(false)
        .visible(true)
        .build()
    {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    #[test]
    fn color_for_percent_uses_plan_boundaries() {
        assert_eq!(color_for_percent(69), PercentColor::Default);
        assert_eq!(color_for_percent(70), PercentColor::Yellow);
        assert_eq!(color_for_percent(89), PercentColor::Yellow);
        assert_eq!(color_for_percent(90), PercentColor::Red);
    }

    #[test]
    fn format_tray_label_uses_percent_over_reset_hour_grid() {
        let now = Utc.with_ymd_and_hms(2026, 5, 23, 5, 0, 0).unwrap();
        let mut state = TrayDisplayState::empty(now);
        state.cc.percent_used = Some(47);
        state.cc.reset_at = Some(now + Duration::hours(3));
        state.cx.percent_used = Some(82);
        state.cx.reset_at = Some(now + Duration::hours(1));

        assert_eq!(format_tray_label(&state), "   47%    82%\n  3.0h   1.0h");
    }

    #[test]
    fn format_tray_label_handles_disabled_sources() {
        let now = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let mut state = TrayDisplayState::empty(now);
        state.cc.percent_used = Some(73);
        state.cc.reset_at = Some(now + Duration::minutes(75));
        state.cx.enabled = false;
        assert_eq!(format_tray_label(&state), "   73%\n  1.2h");
        assert_eq!(format_tray_tooltip(&state), "CC  73% ↻1h15m");
    }

    #[test]
    fn format_tray_label_keeps_enabled_sources_visible_without_percent() {
        let now = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let state = TrayDisplayState::empty(now);

        assert_eq!(format_tray_label(&state), "   --%    --%\n   --h    --h");
        assert_eq!(
            format_tray_tooltip(&state),
            "CC 데이터 없음  CX 공식 실시간 데이터 없음"
        );
    }

    #[test]
    fn format_tray_label_keeps_constant_grid_width_across_digit_counts() {
        let now = Utc.with_ymd_and_hms(2026, 5, 23, 5, 0, 0).unwrap();
        let mut state = TrayDisplayState::empty(now);
        state.cc.percent_used = Some(1);
        state.cc.reset_at = Some(now + Duration::hours(1));
        state.cx.percent_used = Some(100);
        state.cx.reset_at = Some(now + Duration::hours(100));

        let label = format_tray_label(&state);
        let lines = label.lines().collect::<Vec<_>>();
        assert_eq!(lines, ["    1%   100%", "  1.0h 100.0h"]);
        assert_eq!(lines[0].chars().count(), lines[1].chars().count());
        assert_eq!(lines[0].chars().count(), 13);
    }

    #[test]
    fn format_tray_label_marks_estimated_values() {
        let now = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let mut state = TrayDisplayState::empty(now);
        state.cx.percent_used = Some(91);
        state.cx.reset_at = Some(now + Duration::minutes(5));
        state.cx.estimated = true;
        let label = format_tray_label(&state);
        let tooltip = format_tray_tooltip(&state);
        assert!(label.contains("~91%"));
        assert!(label.contains("0.1h"));
        assert!(tooltip.contains("CX ~ 91% ↻5m"));
    }

    #[test]
    fn codex_local_estimator_snapshots_populate_tray_percent() {
        let now = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let snapshot = UsageSnapshot {
            source: UsageSource::Codex,
            window_id: "cx-local".to_string(),
            window_start: now,
            reset_at: now + Duration::hours(5),
            tokens_used: 75,
            quota_tokens: 100,
            percent_used: 75,
            estimated: true,
        };

        let state = TrayDisplayState::from_snapshots(&[snapshot], now);
        assert_eq!(state.cx.percent_used, Some(25));
        assert_eq!(state.cx.reset_at, Some(now + Duration::hours(5)));
        assert_eq!(state.cx.status_source.as_deref(), Some("local_estimate"));
        assert!(state.cx.estimated);
    }

    #[test]
    fn claude_local_estimator_displays_remaining_percent() {
        let now = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let snapshot = UsageSnapshot {
            source: UsageSource::ClaudeCode,
            window_id: "cc-local".to_string(),
            window_start: now,
            reset_at: now + Duration::hours(1),
            tokens_used: 1_000_000,
            quota_tokens: 1_000_000,
            percent_used: 100,
            estimated: true,
        };

        let state = TrayDisplayState::from_snapshots(&[snapshot], now);

        assert_eq!(state.cc.percent_used, Some(0));
        assert!(state.cc.estimated);
    }

    #[test]
    fn local_history_fallback_displays_zero_remaining_when_exhausted() {
        let now = Utc.with_ymd_and_hms(2026, 5, 21, 10, 30, 0).unwrap();
        let mut state = TrayDisplayState::empty(now);
        let config = HiddenConfig {
            default_window_secs: 5 * 60 * 60,
            cc_quota_tokens: 1_000,
            cx_quota_tokens: 1_000,
        };
        let points = vec![HourlyPoint {
            source: UsageSource::ClaudeCode,
            hour_start: Utc.with_ymd_and_hms(2026, 5, 21, 9, 0, 0).unwrap(),
            tokens_used: 1_500,
        }];

        apply_claude_local_history_points(&mut state, &config, &points);

        assert_eq!(state.cc.percent_used, Some(0));
        assert_eq!(
            state.cc.reset_at,
            Some(Utc.with_ymd_and_hms(2026, 5, 21, 14, 0, 0).unwrap())
        );
        assert_eq!(format_tray_label(&state), "   ~0%    --%\n  3.5h    --h");
        assert!(state.cc.estimated);
        assert_eq!(state.cc.status_source.as_deref(), Some("local_history"));
        assert_eq!(
            state.cc.status_message.as_deref(),
            Some("로컬 기록 기반 추정")
        );
    }

    #[test]
    fn local_history_fallback_does_not_override_live_claude_status() {
        let now = Utc.with_ymd_and_hms(2026, 5, 21, 10, 30, 0).unwrap();
        let mut state = TrayDisplayState::empty(now);
        state.cc.percent_used = Some(42);
        let config = HiddenConfig::default();
        let points = vec![HourlyPoint {
            source: UsageSource::ClaudeCode,
            hour_start: Utc.with_ymd_and_hms(2026, 5, 21, 9, 0, 0).unwrap(),
            tokens_used: 1_500_000,
        }];

        apply_claude_local_history_points(&mut state, &config, &points);

        assert_eq!(state.cc.percent_used, Some(42));
    }
}
