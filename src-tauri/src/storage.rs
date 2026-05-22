use crate::parser::{UsageEvent, UsageSource};
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HourlyPoint {
    pub source: UsageSource,
    pub hour_start: DateTime<Utc>,
    pub tokens_used: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Rollups {
    pub source: UsageSource,
    pub day_tokens: u64,
    pub week_tokens: u64,
    pub month_tokens: u64,
}

pub struct UsageStore {
    conn: Connection,
}

impl UsageStore {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    pub fn in_memory() -> anyhow::Result<Self> {
        let store = Self {
            conn: Connection::open_in_memory()?,
        };
        store.init()?;
        Ok(store)
    }

    pub fn init(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS hourly_bucket (
                source TEXT NOT NULL,
                hour_start INTEGER NOT NULL,
                tokens_used INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (source, hour_start)
            );
            CREATE TABLE IF NOT EXISTS daily_rollup (
                source TEXT NOT NULL,
                day_start INTEGER NOT NULL,
                tokens_used INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (source, day_start)
            );
            CREATE TABLE IF NOT EXISTS threshold_state (
                source TEXT NOT NULL,
                window_id TEXT NOT NULL,
                threshold INTEGER NOT NULL,
                notified_at INTEGER NOT NULL,
                PRIMARY KEY (source, window_id, threshold)
            );
            ",
        )?;
        Ok(())
    }

    pub fn record_usage_event(&self, event: &UsageEvent) -> anyhow::Result<()> {
        let hour = truncate_to_hour(event.occurred_at);
        let day = truncate_to_day(event.occurred_at);
        let source = event.source.as_str();
        let tokens = event.tokens as i64;
        self.conn.execute(
            "INSERT INTO hourly_bucket (source, hour_start, tokens_used)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(source, hour_start) DO UPDATE SET tokens_used = tokens_used + excluded.tokens_used",
            params![source, hour.timestamp(), tokens],
        )?;
        self.conn.execute(
            "INSERT INTO daily_rollup (source, day_start, tokens_used)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(source, day_start) DO UPDATE SET tokens_used = tokens_used + excluded.tokens_used",
            params![source, day.timestamp(), tokens],
        )?;
        Ok(())
    }

    pub fn get_24h_series(&self, now: DateTime<Utc>) -> anyhow::Result<Vec<HourlyPoint>> {
        let start = truncate_to_hour(now - Duration::hours(23));
        let mut stmt = self.conn.prepare(
            "SELECT source, hour_start, tokens_used FROM hourly_bucket
             WHERE hour_start >= ?1 ORDER BY hour_start ASC, source ASC",
        )?;
        let rows = stmt.query_map(params![start.timestamp()], |row| {
            let source: String = row.get(0)?;
            let ts: i64 = row.get(1)?;
            let tokens: u64 = row.get(2)?;
            Ok(HourlyPoint {
                source: parse_source(&source),
                hour_start: Utc.timestamp_opt(ts, 0).single().unwrap_or(start),
                tokens_used: tokens,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_rollups(&self, now: DateTime<Utc>) -> anyhow::Result<Vec<Rollups>> {
        [UsageSource::ClaudeCode, UsageSource::Codex]
            .into_iter()
            .map(|source| self.rollups_for(source, now))
            .collect()
    }

    pub fn rollups_for(&self, source: UsageSource, now: DateTime<Utc>) -> anyhow::Result<Rollups> {
        let day = truncate_to_day(now).timestamp();
        let week =
            truncate_to_day(now - Duration::days(now.weekday().num_days_from_monday() as i64))
                .timestamp();
        let month = Utc
            .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp();
        Ok(Rollups {
            source,
            day_tokens: self.sum_rollup(source, day)?,
            week_tokens: self.sum_rollup(source, week)?,
            month_tokens: self.sum_rollup(source, month)?,
        })
    }

    pub fn mark_threshold_notified(
        &self,
        source: UsageSource,
        window_id: &str,
        threshold: u8,
        notified_at: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let changed = self.conn.execute(
            "INSERT OR IGNORE INTO threshold_state (source, window_id, threshold, notified_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                source.as_str(),
                window_id,
                threshold as i64,
                notified_at.timestamp()
            ],
        )?;
        Ok(changed == 1)
    }

    pub fn clear_threshold_state_for_window(&self, window_id: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "DELETE FROM threshold_state WHERE window_id = ?1",
            params![window_id],
        )?;
        Ok(())
    }

    pub fn threshold_exists(
        &self,
        source: UsageSource,
        window_id: &str,
        threshold: u8,
    ) -> anyhow::Result<bool> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM threshold_state WHERE source = ?1 AND window_id = ?2 AND threshold = ?3",
                params![source.as_str(), window_id, threshold as i64],
                |row| row.get(0),
            )
            .optional()?;
        Ok(existing.is_some())
    }

    fn sum_rollup(&self, source: UsageSource, start_day: i64) -> anyhow::Result<u64> {
        let value: Option<u64> = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(tokens_used), 0) FROM daily_rollup WHERE source = ?1 AND day_start >= ?2",
                params![source.as_str(), start_day],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value.unwrap_or(0))
    }
}

fn truncate_to_hour(dt: DateTime<Utc>) -> DateTime<Utc> {
    Utc.timestamp_opt(dt.timestamp() - dt.timestamp().rem_euclid(3600), 0)
        .single()
        .unwrap()
}

fn truncate_to_day(dt: DateTime<Utc>) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(dt.year(), dt.month(), dt.day(), 0, 0, 0)
        .single()
        .unwrap()
}

fn parse_source(source: &str) -> UsageSource {
    match source {
        "cx" => UsageSource::Codex,
        _ => UsageSource::ClaudeCode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::tests_support::usage_event;
    use chrono::TimeZone;

    #[test]
    fn aggregates_hourly_to_daily() {
        let store = UsageStore::in_memory().expect("store");
        let at = Utc.with_ymd_and_hms(2026, 5, 21, 10, 15, 0).unwrap();
        store
            .record_usage_event(&usage_event(UsageSource::ClaudeCode, at, 100))
            .unwrap();
        store
            .record_usage_event(&usage_event(
                UsageSource::ClaudeCode,
                at + Duration::minutes(10),
                50,
            ))
            .unwrap();

        let series = store.get_24h_series(at).unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].tokens_used, 150);

        let rollup = store.rollups_for(UsageSource::ClaudeCode, at).unwrap();
        assert_eq!(rollup.day_tokens, 150);
        assert_eq!(rollup.week_tokens, 150);
        assert_eq!(rollup.month_tokens, 150);
    }
}
