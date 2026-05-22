use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const DEFAULT_WINDOW_SECS: u64 = 5 * 60 * 60;
pub const DEFAULT_CC_QUOTA_TOKENS: u64 = 1_000_000;
pub const DEFAULT_CX_QUOTA_TOKENS: u64 = 1_000_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HiddenConfig {
    #[serde(default = "default_window_secs")]
    pub default_window_secs: u64,
    #[serde(default = "default_cc_quota_tokens")]
    pub cc_quota_tokens: u64,
    #[serde(default = "default_cx_quota_tokens")]
    pub cx_quota_tokens: u64,
}

impl Default for HiddenConfig {
    fn default() -> Self {
        Self {
            default_window_secs: DEFAULT_WINDOW_SECS,
            cc_quota_tokens: DEFAULT_CC_QUOTA_TOKENS,
            cx_quota_tokens: DEFAULT_CX_QUOTA_TOKENS,
        }
    }
}

impl HiddenConfig {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        let Ok(raw) = fs::read_to_string(path) else {
            return Self::default();
        };
        toml::from_str(&raw).unwrap_or_default()
    }

    pub fn quota_for(&self, source: crate::parser::UsageSource) -> u64 {
        match source {
            crate::parser::UsageSource::ClaudeCode => self.cc_quota_tokens,
            crate::parser::UsageSource::Codex => self.cx_quota_tokens,
        }
    }
}

pub fn app_support_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("token-notifier"))
}

pub fn config_path() -> Option<PathBuf> {
    app_support_dir().map(|dir| dir.join("config.toml"))
}

fn default_window_secs() -> u64 {
    DEFAULT_WINDOW_SECS
}

fn default_cc_quota_tokens() -> u64 {
    DEFAULT_CC_QUOTA_TOKENS
}

fn default_cx_quota_tokens() -> u64 {
    DEFAULT_CX_QUOTA_TOKENS
}


pub fn database_path() -> Option<PathBuf> {
    app_support_dir().map(|dir| dir.join("usage.sqlite"))
}
