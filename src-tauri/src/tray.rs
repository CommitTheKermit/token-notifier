use crate::parser::UsageSource;
use crate::window_estimator::UsageSnapshot;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration as StdDuration, Instant};
use tauri::{
    window::{Effect, EffectState, EffectsBuilder},
    App, LogicalPosition, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use crate::native_status::NativeStatusClick;

static LAST_POPOVER_AUTO_HIDE: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
static LAST_DISPLAY_STATE: OnceLock<Mutex<TrayDisplayState>> = OnceLock::new();
const POPOVER_AUTO_HIDE_DEBOUNCE: StdDuration = StdDuration::from_millis(300);

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
            },
            cx: SourceTrayState {
                source: UsageSource::Codex,
                enabled: true,
                percent_used: None,
                reset_at: None,
                estimated: false,
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
            target.percent_used = Some(snapshot.percent_used);
            target.reset_at = Some(snapshot.reset_at);
            target.estimated = snapshot.estimated;
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
    let initial_state = TrayDisplayState::empty(Utc::now());
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
                NativeStatusClick::OpenPopover => open_or_focus_window(
                    &window_app,
                    "popover",
                    "popover.html",
                    "Token Notifier",
                    486.0,
                    582.0,
                    true,
                ),
                NativeStatusClick::OpenSettings => open_or_focus_window(
                    &window_app,
                    "settings",
                    "settings.html",
                    "Token Notifier Settings",
                    460.0,
                    520.0,
                    false,
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
    store_latest_display_state(state);
    let title = format_tray_label(state);
    let tooltip = format_tray_tooltip(state);
    crate::native_status::update_title(app, title, tooltip);
    Ok(())
}

pub fn latest_display_state() -> TrayDisplayState {
    LAST_DISPLAY_STATE
        .get()
        .and_then(|state| state.lock().ok().map(|state| state.clone()))
        .unwrap_or_else(|| TrayDisplayState::empty(Utc::now()))
}

pub fn open_settings_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    open_or_focus_window(
        app,
        "settings",
        "settings.html",
        "Token Notifier Settings",
        460.0,
        520.0,
        false,
    );
}

fn store_latest_display_state(state: &TrayDisplayState) {
    let latest = LAST_DISPLAY_STATE.get_or_init(|| Mutex::new(TrayDisplayState::empty(Utc::now())));
    if let Ok(mut latest) = latest.lock() {
        *latest = state.clone();
    }
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

fn push_source_label(parts: &mut Vec<String>, source: &SourceTrayState, now: DateTime<Utc>) {
    if !source.enabled {
        return;
    }

    let prefix = match source.source {
        UsageSource::ClaudeCode => "CC",
        UsageSource::Codex => "CX",
    };
    let percent = source
        .percent_used
        .map(|value| format!("{value:>3}%"))
        .unwrap_or_else(|| " --%".to_string());
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
}

fn open_or_focus_window<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    label: &str,
    url: &str,
    title: &str,
    width: f64,
    height: f64,
    anchor_to_status: bool,
) {
    if let Some(window) = app.get_webview_window(label) {
        if anchor_to_status {
            if window.is_visible().unwrap_or(false) {
                let _ = window.hide();
                return;
            }
            if popover_was_just_auto_hidden() {
                return;
            }
        }
        if anchor_to_status {
            let _ = window.set_effects(popover_window_effects());
            position_window_below_status(app, &window, width, height);
        }
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        let mut builder = WebviewWindowBuilder::new(app, label, WebviewUrl::App(url.into()))
            .title(title)
            .inner_size(width, height)
            .resizable(false)
            .visible(!anchor_to_status);
        if anchor_to_status {
            builder = builder
                .decorations(false)
                .transparent(true)
                .effects(popover_window_effects())
                .always_on_top(true)
                .skip_taskbar(true)
                .shadow(true);
        }
        if let Ok(window) = builder.build() {
            if anchor_to_status {
                attach_popover_autohide(&window);
                position_window_below_status(app, &window, width, height);
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
    }
}

fn popover_window_effects() -> tauri::utils::config::WindowEffectsConfig {
    EffectsBuilder::new()
        .effect(Effect::Popover)
        .state(EffectState::Active)
        .radius(22.0)
        .build()
}

fn attach_popover_autohide<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    let popover = window.clone();
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Focused(false)) && popover.is_visible().unwrap_or(false) {
            let _ = popover.hide();
            record_popover_auto_hide();
        }
    });
}

fn record_popover_auto_hide() {
    let state = LAST_POPOVER_AUTO_HIDE.get_or_init(|| Mutex::new(None));
    if let Ok(mut last_hide) = state.lock() {
        *last_hide = Some(Instant::now());
    }
}

fn popover_was_just_auto_hidden() -> bool {
    let state = LAST_POPOVER_AUTO_HIDE.get_or_init(|| Mutex::new(None));
    state
        .lock()
        .ok()
        .and_then(|last_hide| *last_hide)
        .is_some_and(|last_hide| last_hide.elapsed() < POPOVER_AUTO_HIDE_DEBOUNCE)
}

fn position_window_below_status<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    window: &WebviewWindow<R>,
    width: f64,
    height: f64,
) {
    if let Some(position) = popover_position_below_status(app, width, height) {
        let _ = window.set_position(position);
    }
}

fn popover_position_below_status<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    width: f64,
    height: f64,
) -> Option<LogicalPosition<f64>> {
    let anchor = crate::native_status::anchor_rect()?;
    let monitor = app.primary_monitor().ok().flatten();
    let (screen_width, screen_height) = if let Some(monitor) = monitor {
        let scale = monitor.scale_factor();
        (
            monitor.size().width as f64 / scale,
            monitor.size().height as f64 / scale,
        )
    } else {
        (1440.0, 900.0)
    };

    let margin = 8.0;
    let x = (anchor.x + anchor.width / 2.0 - width / 2.0)
        .max(margin)
        .min((screen_width - width - margin).max(margin));
    let y = (screen_height - anchor.y + 4.0)
        .max(margin)
        .min((screen_height - height - margin).max(margin));
    Some(LogicalPosition::new(x, y))
}
