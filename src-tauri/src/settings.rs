use crate::alerts::normalized_thresholds;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceSettings {
    pub enabled: bool,
    pub thresholds: Vec<u8>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSettings {
    pub claude_code: SourceSettings,
    pub codex: SourceSettings,
    #[serde(default)]
    pub remote_sync: RemoteSyncSettings,
    pub autostart_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteSyncSettings {
    pub enabled: bool,
    pub openai_enabled: bool,
    pub anthropic_enabled: bool,
    pub interval_minutes: u64,
    pub lookback_hours: u64,
}

impl Default for SourceSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            thresholds: vec![75, 90],
        }
    }
}

impl Default for RemoteSyncSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            openai_enabled: true,
            anthropic_enabled: true,
            interval_minutes: 30,
            lookback_hours: 48,
        }
    }
}

impl AppSettings {
    pub fn normalized(mut self) -> Self {
        self.claude_code.thresholds =
            ensure_at_least_one(normalized_thresholds(&self.claude_code.thresholds));
        self.codex.thresholds = ensure_at_least_one(normalized_thresholds(&self.codex.thresholds));
        self.remote_sync = self.remote_sync.normalized();
        self
    }
}

impl RemoteSyncSettings {
    pub fn normalized(mut self) -> Self {
        self.interval_minutes = self.interval_minutes.clamp(5, 24 * 60);
        self.lookback_hours = self.lookback_hours.clamp(1, 24 * 31);
        self
    }
}

pub fn settings_path() -> Option<PathBuf> {
    crate::config::app_support_dir().map(|dir| dir.join("settings.json"))
}

pub fn load_settings() -> AppSettings {
    let Some(path) = settings_path() else {
        return AppSettings::default();
    };
    let Ok(raw) = fs::read_to_string(path) else {
        return AppSettings::default();
    };
    serde_json::from_str::<AppSettings>(&raw)
        .map(AppSettings::normalized)
        .unwrap_or_default()
}

pub fn save_settings(settings: &AppSettings) -> anyhow::Result<AppSettings> {
    let normalized = settings.clone().normalized();
    let path = settings_path().ok_or_else(|| anyhow::anyhow!("Could not resolve settings path"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string_pretty(&normalized)?)?;
    Ok(normalized)
}

fn ensure_at_least_one(mut values: Vec<u8>) -> Vec<u8> {
    if values.is_empty() {
        values.push(75);
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_normalized_to_one_to_three_thresholds() {
        let settings = AppSettings {
            claude_code: SourceSettings {
                enabled: true,
                thresholds: vec![0, 90, 75, 75, 99, 100],
            },
            codex: SourceSettings {
                enabled: false,
                thresholds: vec![],
            },
            remote_sync: RemoteSyncSettings {
                enabled: true,
                openai_enabled: true,
                anthropic_enabled: false,
                interval_minutes: 1,
                lookback_hours: 24 * 40,
            },
            autostart_enabled: true,
        }
        .normalized();

        assert_eq!(settings.claude_code.thresholds, vec![75, 90, 99]);
        assert_eq!(settings.codex.thresholds, vec![75]);
        assert_eq!(settings.remote_sync.interval_minutes, 5);
        assert_eq!(settings.remote_sync.lookback_hours, 24 * 31);
        assert!(settings.autostart_enabled);
    }
}
