use crate::parser::UsageSource;
use crate::window_estimator::UsageSnapshot;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::{App, LogicalPosition, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::native_status::NativeStatusClick;

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
    let mut headers = Vec::new();
    let mut percents = Vec::new();
    push_compact_source_label(&mut headers, &mut percents, &state.cc);
    push_compact_source_label(&mut headers, &mut percents, &state.cx);
    if headers.is_empty() {
        "Token Notifier".to_string()
    } else {
        format!("{}\n{}", headers.join("   "), percents.join("  "))
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
                    560.0,
                    380.0,
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
    let title = format_tray_label(state);
    let tooltip = format_tray_tooltip(state);
    crate::native_status::update_title(app, title, tooltip);
    Ok(())
}

fn push_compact_source_label(
    headers: &mut Vec<String>,
    percents: &mut Vec<String>,
    source: &SourceTrayState,
) {
    if !source.enabled {
        return;
    }

    let prefix = match source.source {
        UsageSource::ClaudeCode => "CC",
        UsageSource::Codex => "CX",
    };
    let percent = source
        .percent_used
        .map(|value| value.to_string())
        .unwrap_or_else(|| "--".to_string());
    let estimate = if source.estimated { "~" } else { "" };
    headers.push(prefix.to_string());
    percents.push(format!("{estimate}{percent}"));
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
    fn format_tray_label_handles_disabled_sources() {
        let now = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let mut state = TrayDisplayState::empty(now);
        state.cc.percent_used = Some(73);
        state.cc.reset_at = Some(now + Duration::minutes(75));
        state.cx.enabled = false;
        assert_eq!(format_tray_label(&state), "CC\n73");
        assert_eq!(format_tray_tooltip(&state), "CC  73% ↻1h15m");
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
        assert!(label.contains("CX"));
        assert!(label.contains("~91"));
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
                .always_on_top(true)
                .skip_taskbar(true)
                .shadow(true);
        }
        if let Ok(window) = builder.build() {
            if anchor_to_status {
                position_window_below_status(app, &window, width, height);
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
    }
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
