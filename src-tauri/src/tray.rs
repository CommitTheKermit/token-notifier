use crate::parser::UsageSource;
use crate::window_estimator::UsageSnapshot;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{image::Image, App, Manager, WebviewUrl, WebviewWindowBuilder};

pub const MAIN_TRAY_ID: &str = "main";

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
    let mut parts = Vec::new();
    push_source_label(&mut parts, &state.cc, state.now);
    push_source_label(&mut parts, &state.cx, state.now);
    if parts.is_empty() {
        "Token Notifier".to_string()
    } else {
        parts.join("  ")
    }
}

pub fn build_main_tray(app: &App) -> tauri::Result<TrayIcon> {
    let icon = tray_status_icon();

    TrayIconBuilder::with_id(MAIN_TRAY_ID)
        .icon(icon)
        .icon_as_template(true)
        .title(format_tray_label(&TrayDisplayState::empty(Utc::now())))
        .tooltip("Token Notifier")
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                let app = tray.app_handle();
                open_or_focus_window(
                    app,
                    "popover",
                    "popover.html",
                    "Token Notifier",
                    560.0,
                    380.0,
                );
            } else if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Right,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                let app = tray.app_handle();
                open_or_focus_window(
                    app,
                    "settings",
                    "settings.html",
                    "Token Notifier Settings",
                    460.0,
                    520.0,
                );
            }
        })
        .build(app)
}

pub fn update_main_tray<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &TrayDisplayState,
) -> tauri::Result<()> {
    if let Some(tray) = app.tray_by_id(MAIN_TRAY_ID) {
        tray.set_title(Some(format_tray_label(state)))?;
    }
    Ok(())
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
        assert_eq!(format_tray_label(&state), "CC  73% ↻1h15m");
    }

    #[test]
    fn format_tray_label_marks_estimated_values() {
        let now = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let mut state = TrayDisplayState::empty(now);
        state.cx.percent_used = Some(91);
        state.cx.reset_at = Some(now + Duration::minutes(5));
        state.cx.estimated = true;
        let label = format_tray_label(&state);
        assert!(label.contains("CX ~ 91% ↻5m"));
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
    } else {
        let _ = WebviewWindowBuilder::new(app, label, WebviewUrl::App(url.into()))
            .title(title)
            .inner_size(width, height)
            .resizable(false)
            .visible(true)
            .build();
    }
}

fn tray_status_icon() -> Image<'static> {
    let size = 18u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let center = (size as f32 - 1.0) / 2.0;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let distance = (dx * dx + dy * dy).sqrt();
            let alpha = if distance <= 7.0 { 255 } else { 0 };
            rgba.extend_from_slice(&[0, 0, 0, alpha]);
        }
    }
    Image::new_owned(rgba, size, size)
}
