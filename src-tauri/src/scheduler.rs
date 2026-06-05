use crate::parser::{LocalLogParser, UsageEvent, UsageSource};
use crate::storage::UsageStore;
use crate::window_estimator::{UsageSnapshot, WindowEstimator};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;
use tokio::time::{self, Duration as TokioDuration, Instant};

pub const MIN_POLL_INTERVAL_SECS: u64 = 90;

#[derive(Debug, Clone, Serialize)]
pub struct PollOutcome {
    pub generation: u64,
    pub events_read: usize,
    pub snapshots: Vec<UsageSnapshot>,
}

pub struct UsageScheduler {
    window_generation_id: Arc<AtomicU64>,
    parsers: Vec<Box<dyn LocalLogParser + Send>>,
    estimator: WindowEstimator,
    store: Arc<Mutex<UsageStore>>,
    reset_task: Option<JoinHandle<()>>,
}

impl UsageScheduler {
    pub fn new(
        parsers: Vec<Box<dyn LocalLogParser + Send>>,
        estimator: WindowEstimator,
        store: UsageStore,
    ) -> Self {
        Self {
            window_generation_id: Arc::new(AtomicU64::new(0)),
            parsers,
            estimator,
            store: Arc::new(Mutex::new(store)),
            reset_task: None,
        }
    }

    pub fn generation(&self) -> u64 {
        self.window_generation_id.load(Ordering::Acquire)
    }

    pub fn generation_handle(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.window_generation_id)
    }

    pub fn current_snapshots(&self) -> Vec<UsageSnapshot> {
        self.estimator.current_snapshots()
    }

    pub fn poll_once(
        &mut self,
        is_enabled: impl Fn(UsageSource) -> bool,
    ) -> anyhow::Result<Option<PollOutcome>> {
        let generation_at_dispatch = self.generation();
        let mut events = Vec::new();
        for parser in &mut self.parsers {
            // 표시를 끈(비활성) 소스는 폴링하지 않는다.
            if !is_enabled(parser.source()) {
                continue;
            }
            events.extend(parser.read_delta()?);
        }
        self.commit_events_if_fresh(generation_at_dispatch, events)
    }

    pub fn commit_events_if_fresh(
        &mut self,
        generation_at_dispatch: u64,
        events: Vec<UsageEvent>,
    ) -> anyhow::Result<Option<PollOutcome>> {
        if self.generation() != generation_at_dispatch {
            return Ok(None);
        }

        let mut fresh_events = Vec::new();
        {
            let store = self.store.lock().expect("usage store lock");
            for event in &events {
                if store.record_usage_event(event)? {
                    fresh_events.push(event.clone());
                }
            }
        }

        if fresh_events.is_empty() {
            return Ok(None);
        }

        let snapshots = self.estimator.ingest(&fresh_events);
        let outcome = PollOutcome {
            generation: generation_at_dispatch,
            events_read: events.len(),
            snapshots,
        };
        Ok(Some(outcome))
    }

    pub fn reset_window(&mut self) -> u64 {
        self.window_generation_id.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub fn schedule_reset<F>(&mut self, reset_at: DateTime<Utc>, on_reset: F)
    where
        F: FnOnce(u64) + Send + 'static,
    {
        let delay = reset_delay(reset_at);
        self.schedule_reset_after(delay, on_reset);
    }

    pub fn schedule_reset_after<F>(&mut self, delay: TokioDuration, on_reset: F)
    where
        F: FnOnce(u64) + Send + 'static,
    {
        if let Some(task) = self.reset_task.take() {
            task.abort();
        }
        let generation = self.generation();
        let generation_handle = self.generation_handle();
        self.reset_task = Some(tokio::spawn(async move {
            time::sleep(delay).await;
            if generation_handle.load(Ordering::Acquire) == generation {
                let next = generation_handle.fetch_add(1, Ordering::AcqRel) + 1;
                on_reset(next);
            }
        }));
    }

    pub async fn run_poll_loop<F>(&mut self, mut on_outcome: F) -> anyhow::Result<()>
    where
        F: FnMut(PollOutcome) + Send,
    {
        let mut interval = time::interval(TokioDuration::from_secs(MIN_POLL_INTERVAL_SECS));
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if let Some(outcome) = self.poll_once(|_| true)? {
                on_outcome(outcome);
            }
        }
    }
}

pub fn reset_delay(reset_at: DateTime<Utc>) -> TokioDuration {
    let now = Utc::now();
    if reset_at <= now {
        return TokioDuration::ZERO;
    }
    (reset_at - now).to_std().unwrap_or(TokioDuration::ZERO)
}

pub async fn sleep_until_utc(reset_at: DateTime<Utc>) {
    time::sleep_until(Instant::now() + reset_delay(reset_at)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::tests_support::usage_event;
    use crate::parser::{LocalLogParser, UsageSource};
    use crate::storage::UsageStore;
    use chrono::TimeZone;
    use std::sync::atomic::AtomicBool;

    struct FakeParser {
        source: UsageSource,
        events: Vec<UsageEvent>,
    }

    impl Default for FakeParser {
        fn default() -> Self {
            Self {
                source: UsageSource::ClaudeCode,
                events: Vec::new(),
            }
        }
    }

    impl LocalLogParser for FakeParser {
        fn source(&self) -> UsageSource {
            self.source
        }

        fn read_delta(&mut self) -> anyhow::Result<Vec<UsageEvent>> {
            Ok(std::mem::take(&mut self.events))
        }
    }

    #[test]
    fn stale_tick_is_dropped_after_reset_generation_changes() {
        let at = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let parser = FakeParser::default();
        let mut scheduler = UsageScheduler::new(
            vec![Box::new(parser)],
            WindowEstimator::default(),
            UsageStore::in_memory().unwrap(),
        );
        let generation = scheduler.generation();
        scheduler.reset_window();
        let outcome = scheduler
            .commit_events_if_fresh(
                generation,
                vec![usage_event(UsageSource::ClaudeCode, at, 10)],
            )
            .unwrap();
        assert!(outcome.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn reset_oneshot_advances_generation_independently() {
        let parser = FakeParser::default();
        let mut scheduler = UsageScheduler::new(
            vec![Box::new(parser)],
            WindowEstimator::default(),
            UsageStore::in_memory().unwrap(),
        );
        let fired = Arc::new(AtomicBool::new(false));
        let fired_for_task = Arc::clone(&fired);
        scheduler.schedule_reset_after(TokioDuration::from_secs(5), move |_| {
            fired_for_task.store(true, Ordering::Release);
        });
        tokio::task::yield_now().await;

        time::advance(TokioDuration::from_secs(4)).await;
        assert_eq!(scheduler.generation(), 0);
        assert!(!fired.load(Ordering::Acquire));

        time::advance(TokioDuration::from_secs(2)).await;
        tokio::task::yield_now().await;
        assert_eq!(scheduler.generation(), 1);
        assert!(fired.load(Ordering::Acquire));
    }

    #[test]
    fn poll_once_reads_parser_estimates_and_records() {
        let at = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let parser = FakeParser {
            source: UsageSource::Codex,
            events: vec![usage_event(UsageSource::Codex, at, 20)],
        };
        let mut scheduler = UsageScheduler::new(
            vec![Box::new(parser)],
            WindowEstimator::default(),
            UsageStore::in_memory().unwrap(),
        );
        let outcome = scheduler.poll_once(|_| true).unwrap().unwrap();
        assert_eq!(outcome.events_read, 1);
        assert_eq!(outcome.snapshots[0].source, UsageSource::Codex);
        assert!(outcome.snapshots[0].estimated);
    }

    #[test]
    fn poll_once_skips_disabled_sources() {
        let at = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let parser = FakeParser {
            source: UsageSource::Codex,
            events: vec![usage_event(UsageSource::Codex, at, 20)],
        };
        let mut scheduler = UsageScheduler::new(
            vec![Box::new(parser)],
            WindowEstimator::default(),
            UsageStore::in_memory().unwrap(),
        );
        // 코덱스를 끄면 폴링되지 않아 결과가 없다.
        let outcome = scheduler
            .poll_once(|source| source != UsageSource::Codex)
            .unwrap();
        assert!(outcome.is_none());
    }

    #[test]
    fn duplicate_events_do_not_refresh_current_snapshots() {
        let at = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let event = usage_event(UsageSource::Codex, at, 20);
        let mut scheduler = UsageScheduler::new(
            vec![Box::new(FakeParser::default())],
            WindowEstimator::default(),
            UsageStore::in_memory().unwrap(),
        );

        assert!(scheduler
            .commit_events_if_fresh(0, vec![event.clone()])
            .unwrap()
            .is_some());
        assert!(scheduler
            .commit_events_if_fresh(0, vec![event])
            .unwrap()
            .is_none());
    }
}
