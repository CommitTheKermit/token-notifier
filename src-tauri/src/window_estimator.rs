use crate::config::{HiddenConfig, DEFAULT_WINDOW_SECS};
use crate::parser::{UsageEvent, UsageSource};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageSnapshot {
    pub source: UsageSource,
    pub window_id: String,
    pub window_start: DateTime<Utc>,
    pub reset_at: DateTime<Utc>,
    pub tokens_used: u64,
    pub quota_tokens: u64,
    pub percent_used: u8,
    pub estimated: bool,
}

#[derive(Debug, Clone)]
struct WindowState {
    window_start: DateTime<Utc>,
    reset_at: DateTime<Utc>,
    quota_tokens: u64,
    tokens_used: u64,
    estimated: bool,
}

#[derive(Debug, Clone)]
pub struct WindowEstimator {
    config: HiddenConfig,
    states: HashMap<UsageSource, WindowState>,
}

impl Default for WindowEstimator {
    fn default() -> Self {
        Self::new(HiddenConfig::default())
    }
}

impl WindowEstimator {
    pub fn new(config: HiddenConfig) -> Self {
        Self {
            config,
            states: HashMap::new(),
        }
    }

    pub fn ingest(&mut self, events: &[UsageEvent]) -> Vec<UsageSnapshot> {
        let mut snapshots = Vec::new();
        for event in events {
            let state = if let Some(metadata) = &event.metadata {
                let state = self.states.entry(event.source).or_insert(WindowState {
                    window_start: metadata.window_start,
                    reset_at: metadata.reset_at,
                    quota_tokens: metadata.quota_tokens.max(1),
                    tokens_used: 0,
                    estimated: false,
                });
                if state.window_start != metadata.window_start || state.reset_at != metadata.reset_at {
                    *state = WindowState {
                        window_start: metadata.window_start,
                        reset_at: metadata.reset_at,
                        quota_tokens: metadata.quota_tokens.max(1),
                        tokens_used: 0,
                        estimated: false,
                    };
                } else {
                    state.quota_tokens = metadata.quota_tokens.max(1);
                    state.estimated = false;
                }
                state
            } else {
                let default_window_secs = self.config.default_window_secs.max(60);
                let quota = self.config.quota_for(event.source).max(1);
                let state = self.states.entry(event.source).or_insert_with(|| WindowState {
                    window_start: event.occurred_at,
                    reset_at: event.occurred_at
                        + Duration::seconds(default_window_secs.try_into().unwrap_or(DEFAULT_WINDOW_SECS as i64)),
                    quota_tokens: quota,
                    tokens_used: 0,
                    estimated: true,
                });
                if event.occurred_at >= state.reset_at {
                    let windows_elapsed = ((event.occurred_at - state.window_start).num_seconds()
                        / default_window_secs as i64)
                        .max(1);
                    let new_start = state.window_start
                        + Duration::seconds(windows_elapsed * default_window_secs as i64);
                    *state = WindowState {
                        window_start: new_start,
                        reset_at: new_start + Duration::seconds(default_window_secs as i64),
                        quota_tokens: quota,
                        tokens_used: 0,
                        estimated: true,
                    };
                }
                state
            };

            state.tokens_used = state.tokens_used.saturating_add(event.tokens);
            snapshots.push(snapshot_for(event.source, state));
        }
        snapshots
    }

    pub fn current_snapshots(&self) -> Vec<UsageSnapshot> {
        self.states
            .iter()
            .map(|(source, state)| snapshot_for(*source, state))
            .collect()
    }
}

fn snapshot_for(source: UsageSource, state: &WindowState) -> UsageSnapshot {
    let percent = ((state.tokens_used.saturating_mul(100)) / state.quota_tokens).min(100) as u8;
    UsageSnapshot {
        source,
        window_id: format!("{}:{}", source.as_str(), state.window_start.timestamp()),
        window_start: state.window_start,
        reset_at: state.reset_at,
        tokens_used: state.tokens_used,
        quota_tokens: state.quota_tokens,
        percent_used: percent,
        estimated: state.estimated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::tests_support::usage_event;
    use crate::parser::{UsageEvent, WindowMetadata};
    use chrono::TimeZone;

    #[test]
    fn prefers_metadata_over_fallback() {
        let mut estimator = WindowEstimator::default();
        let at = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let event = UsageEvent {
            metadata: Some(WindowMetadata {
                window_start: at - Duration::minutes(10),
                reset_at: at + Duration::minutes(50),
                quota_tokens: 1_000,
            }),
            ..usage_event(UsageSource::ClaudeCode, at, 250)
        };

        let snapshots = estimator.ingest(&[event]);
        assert_eq!(snapshots[0].percent_used, 25);
        assert!(!snapshots[0].estimated);
        assert_eq!(snapshots[0].reset_at, at + Duration::minutes(50));
    }
}
