use crate::parser::UsageSource;
use crate::storage::UsageStore;
use crate::window_estimator::UsageSnapshot;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationSpec {
    pub source: UsageSource,
    pub threshold: u8,
    pub percent_used: u8,
    pub window_id: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThresholdConfig {
    pub source: UsageSource,
    pub thresholds: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct ThresholdEvaluator;

impl ThresholdEvaluator {
    pub fn evaluate(
        store: &UsageStore,
        snapshot: &UsageSnapshot,
        thresholds: &[u8],
    ) -> anyhow::Result<Option<NotificationSpec>> {
        let mut thresholds = normalized_thresholds(thresholds);
        thresholds.sort_unstable();
        for threshold in thresholds {
            if snapshot.percent_used < threshold {
                continue;
            }
            let inserted = store.mark_threshold_notified(
                snapshot.source,
                &snapshot.window_id,
                threshold,
                Utc::now(),
            )?;
            if inserted {
                return Ok(Some(NotificationSpec::new(snapshot, threshold)));
            }
        }
        Ok(None)
    }
}

impl NotificationSpec {
    fn new(snapshot: &UsageSnapshot, threshold: u8) -> Self {
        let source_name = snapshot.source.display_name();
        Self {
            source: snapshot.source,
            threshold,
            percent_used: snapshot.percent_used,
            window_id: snapshot.window_id.clone(),
            title: format!("{source_name} usage reached {threshold}%"),
            body: format!(
                "{source_name} is at {}% of the current{} window.",
                snapshot.percent_used,
                if snapshot.estimated { " estimated" } else { "" }
            ),
        }
    }
}

pub fn send_notification<R: tauri::Runtime>(
    app: &AppHandle<R>,
    notification: &NotificationSpec,
) -> anyhow::Result<()> {
    app.notification()
        .builder()
        .title(&notification.title)
        .body(&notification.body)
        .show()?;
    Ok(())
}

pub fn normalized_thresholds(thresholds: &[u8]) -> Vec<u8> {
    let mut values = thresholds
        .iter()
        .copied()
        .filter(|value| (1..=99).contains(value))
        .collect::<Vec<_>>();
    values.sort_unstable();
    values.dedup();
    values.truncate(3);
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::UsageSource;
    use crate::storage::UsageStore;
    use crate::window_estimator::UsageSnapshot;
    use chrono::{Duration, TimeZone};

    fn snapshot(percent: u8, window: &str) -> UsageSnapshot {
        let start = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        UsageSnapshot {
            source: UsageSource::ClaudeCode,
            window_id: window.to_string(),
            window_start: start,
            reset_at: start + Duration::hours(5),
            tokens_used: percent as u64,
            quota_tokens: 100,
            percent_used: percent,
            estimated: true,
        }
    }

    #[test]
    fn duplicate_threshold_is_emitted_once_per_window() {
        let store = UsageStore::in_memory().unwrap();
        let first = ThresholdEvaluator::evaluate(&store, &snapshot(76, "cc:1"), &[75])
            .unwrap()
            .expect("first notification");
        assert_eq!(first.threshold, 75);
        let second = ThresholdEvaluator::evaluate(&store, &snapshot(80, "cc:1"), &[75]).unwrap();
        assert!(second.is_none());
    }

    #[test]
    fn oscillation_inside_window_is_absorbed() {
        let store = UsageStore::in_memory().unwrap();
        assert!(
            ThresholdEvaluator::evaluate(&store, &snapshot(76, "cc:1"), &[75])
                .unwrap()
                .is_some()
        );
        assert!(
            ThresholdEvaluator::evaluate(&store, &snapshot(65, "cc:1"), &[75])
                .unwrap()
                .is_none()
        );
        assert!(
            ThresholdEvaluator::evaluate(&store, &snapshot(76, "cc:1"), &[75])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn next_window_can_emit_again() {
        let store = UsageStore::in_memory().unwrap();
        assert!(
            ThresholdEvaluator::evaluate(&store, &snapshot(76, "cc:1"), &[75])
                .unwrap()
                .is_some()
        );
        assert!(
            ThresholdEvaluator::evaluate(&store, &snapshot(76, "cc:2"), &[75])
                .unwrap()
                .is_some()
        );
    }
}
