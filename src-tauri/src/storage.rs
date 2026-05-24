use crate::parser::{UsageEvent, UsageSource};
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

const LOCAL_ACCOUNTING_GENERATION_KEY: &str = "local_accounting_generation";
const CURRENT_LOCAL_ACCOUNTING_GENERATION: &str = "3";

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteHourlyPoint {
    pub provider: String,
    pub source: UsageSource,
    pub hour_start: DateTime<Utc>,
    pub tokens_used: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteSyncState {
    pub provider: String,
    pub last_synced_at: DateTime<Utc>,
    pub status: String,
    pub message: Option<String>,
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
            CREATE TABLE IF NOT EXISTS remote_hourly_bucket (
                provider TEXT NOT NULL,
                source TEXT NOT NULL,
                hour_start INTEGER NOT NULL,
                tokens_used INTEGER NOT NULL DEFAULT 0,
                synced_at INTEGER NOT NULL,
                PRIMARY KEY (provider, source, hour_start)
            );
            CREATE TABLE IF NOT EXISTS remote_sync_state (
                provider TEXT PRIMARY KEY,
                last_synced_at INTEGER NOT NULL,
                status TEXT NOT NULL,
                message TEXT
            );
            CREATE TABLE IF NOT EXISTS processed_usage_event (
                source TEXT NOT NULL,
                event_id TEXT NOT NULL,
                recorded_at INTEGER NOT NULL,
                PRIMARY KEY (source, event_id)
            );
            CREATE TABLE IF NOT EXISTS parser_state (
                parser TEXT NOT NULL,
                key TEXT NOT NULL,
                value INTEGER NOT NULL,
                PRIMARY KEY (parser, key)
            );
            CREATE TABLE IF NOT EXISTS schema_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )?;
        self.rebase_local_aggregates_if_needed()?;
        Ok(())
    }

    pub fn record_usage_event(&self, event: &UsageEvent) -> anyhow::Result<bool> {
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO processed_usage_event (source, event_id, recorded_at)
             VALUES (?1, ?2, ?3)",
            params![
                event.source.as_str(),
                event.event_id,
                Utc::now().timestamp()
            ],
        )?;
        if inserted == 0 {
            return Ok(false);
        }

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
        Ok(true)
    }

    pub fn get_parser_state_value(&self, parser: &str, key: &str) -> anyhow::Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT value FROM parser_state WHERE parser = ?1 AND key = ?2",
                params![parser, key],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn get_parser_state_values(&self, parser: &str) -> anyhow::Result<HashMap<String, i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM parser_state WHERE parser = ?1")?;
        let rows = stmt.query_map(params![parser], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect::<Result<HashMap<_, _>, _>>()
            .map_err(Into::into)
    }

    pub fn set_parser_state_value(
        &self,
        parser: &str,
        key: &str,
        value: i64,
    ) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO parser_state (parser, key, value)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(parser, key) DO UPDATE SET value = excluded.value",
            params![parser, key, value],
        )?;
        Ok(())
    }

    pub fn get_24h_series(&self, now: DateTime<Utc>) -> anyhow::Result<Vec<HourlyPoint>> {
        let start = truncate_to_hour(now - Duration::hours(23));
        let mut stmt = self.conn.prepare(
            "WITH remote AS (
                SELECT source, hour_start, SUM(tokens_used) AS tokens_used
                FROM remote_hourly_bucket
                WHERE hour_start >= ?1
                GROUP BY source, hour_start
             ),
             merged_keys AS (
                SELECT source, hour_start FROM hourly_bucket WHERE hour_start >= ?1
                UNION
                SELECT source, hour_start FROM remote
             )
             SELECT k.source, k.hour_start, COALESCE(r.tokens_used, l.tokens_used, 0)
             FROM merged_keys k
             LEFT JOIN hourly_bucket l ON l.source = k.source AND l.hour_start = k.hour_start
             LEFT JOIN remote r ON r.source = k.source AND r.hour_start = k.hour_start
             ORDER BY k.hour_start ASC, k.source ASC",
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
            day_tokens: self.sum_merged_hourly(source, day)?,
            week_tokens: self.sum_merged_hourly(source, week)?,
            month_tokens: self.sum_merged_hourly(source, month)?,
        })
    }

    pub fn upsert_remote_hourly_points(
        &mut self,
        provider: &str,
        points: &[RemoteHourlyPoint],
        synced_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        for point in points {
            tx.execute(
                "INSERT INTO remote_hourly_bucket (provider, source, hour_start, tokens_used, synced_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(provider, source, hour_start) DO UPDATE SET
                    tokens_used = excluded.tokens_used,
                    synced_at = excluded.synced_at",
                params![
                    provider,
                    point.source.as_str(),
                    truncate_to_hour(point.hour_start).timestamp(),
                    point.tokens_used as i64,
                    synced_at.timestamp()
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn mark_remote_sync_state(
        &self,
        provider: &str,
        status: &str,
        message: Option<&str>,
        synced_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO remote_sync_state (provider, last_synced_at, status, message)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(provider) DO UPDATE SET
                last_synced_at = excluded.last_synced_at,
                status = excluded.status,
                message = excluded.message",
            params![provider, synced_at.timestamp(), status, message],
        )?;
        Ok(())
    }

    pub fn get_remote_sync_states(&self) -> anyhow::Result<Vec<RemoteSyncState>> {
        let mut stmt = self.conn.prepare(
            "SELECT provider, last_synced_at, status, message
             FROM remote_sync_state
             ORDER BY provider ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let provider: String = row.get(0)?;
            let ts: i64 = row.get(1)?;
            Ok(RemoteSyncState {
                provider,
                last_synced_at: Utc.timestamp_opt(ts, 0).single().unwrap_or_else(Utc::now),
                status: row.get(2)?,
                message: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
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

    fn sum_merged_hourly(&self, source: UsageSource, start_hour: i64) -> anyhow::Result<u64> {
        let value: Option<u64> = self
            .conn
            .query_row(
                "WITH remote AS (
                    SELECT source, hour_start, SUM(tokens_used) AS tokens_used
                    FROM remote_hourly_bucket
                    WHERE source = ?1 AND hour_start >= ?2
                    GROUP BY source, hour_start
                 ),
                 merged_keys AS (
                    SELECT source, hour_start FROM hourly_bucket WHERE source = ?1 AND hour_start >= ?2
                    UNION
                    SELECT source, hour_start FROM remote
                 )
                 SELECT COALESCE(SUM(COALESCE(r.tokens_used, l.tokens_used, 0)), 0)
                 FROM merged_keys k
                 LEFT JOIN hourly_bucket l ON l.source = k.source AND l.hour_start = k.hour_start
                 LEFT JOIN remote r ON r.source = k.source AND r.hour_start = k.hour_start",
                params![source.as_str(), start_hour],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value.unwrap_or(0))
    }

    fn rebase_local_aggregates_if_needed(&self) -> anyhow::Result<()> {
        let current: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = ?1",
                params![LOCAL_ACCOUNTING_GENERATION_KEY],
                |row| row.get(0),
            )
            .optional()?;
        if current.as_deref() == Some(CURRENT_LOCAL_ACCOUNTING_GENERATION) {
            return Ok(());
        }

        if current.as_deref() == Some("2") {
            self.conn.execute_batch(
                "
                DELETE FROM hourly_bucket WHERE source = 'cx';
                DELETE FROM daily_rollup WHERE source = 'cx';
                DELETE FROM threshold_state WHERE source = 'cx';
                DELETE FROM processed_usage_event WHERE source = 'cx';
                DELETE FROM parser_state WHERE parser = 'codex';
                ",
            )?;
        } else {
            self.conn.execute_batch(
                "
                DELETE FROM hourly_bucket;
                DELETE FROM daily_rollup;
                DELETE FROM threshold_state;
                DELETE FROM processed_usage_event;
                DELETE FROM parser_state;
                ",
            )?;
        }
        self.conn.execute(
            "INSERT INTO schema_meta (key, value)
             VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![
                LOCAL_ACCOUNTING_GENERATION_KEY,
                CURRENT_LOCAL_ACCOUNTING_GENERATION
            ],
        )?;
        Ok(())
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

    #[test]
    fn remote_hourly_points_override_local_hourly_when_present() {
        let mut store = UsageStore::in_memory().expect("store");
        let at = Utc.with_ymd_and_hms(2026, 5, 21, 10, 15, 0).unwrap();
        store
            .record_usage_event(&usage_event(UsageSource::Codex, at, 100))
            .unwrap();
        store
            .upsert_remote_hourly_points(
                "openai",
                &[RemoteHourlyPoint {
                    provider: "openai".to_string(),
                    source: UsageSource::Codex,
                    hour_start: at,
                    tokens_used: 175,
                }],
                at,
            )
            .unwrap();

        let series = store.get_24h_series(at).unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].tokens_used, 175);

        let rollup = store.rollups_for(UsageSource::Codex, at).unwrap();
        assert_eq!(rollup.day_tokens, 175);
    }

    #[test]
    fn duplicate_usage_events_are_not_counted_twice() {
        let store = UsageStore::in_memory().expect("store");
        let at = Utc.with_ymd_and_hms(2026, 5, 21, 10, 15, 0).unwrap();
        let event = usage_event(UsageSource::ClaudeCode, at, 100);

        assert!(store.record_usage_event(&event).unwrap());
        assert!(!store.record_usage_event(&event).unwrap());

        let rollup = store.rollups_for(UsageSource::ClaudeCode, at).unwrap();
        assert_eq!(rollup.day_tokens, 100);
    }

    #[test]
    fn opening_legacy_store_rebases_polluted_local_aggregates() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("usage.sqlite");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE hourly_bucket (
                    source TEXT NOT NULL,
                    hour_start INTEGER NOT NULL,
                    tokens_used INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (source, hour_start)
                );
                CREATE TABLE daily_rollup (
                    source TEXT NOT NULL,
                    day_start INTEGER NOT NULL,
                    tokens_used INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (source, day_start)
                );
                CREATE TABLE threshold_state (
                    source TEXT NOT NULL,
                    window_id TEXT NOT NULL,
                    threshold INTEGER NOT NULL,
                    notified_at INTEGER NOT NULL,
                    PRIMARY KEY (source, window_id, threshold)
                );
                INSERT INTO hourly_bucket VALUES ('cc', 1779519600, 999);
                INSERT INTO daily_rollup VALUES ('cc', 1779494400, 999);
                ",
            )
            .unwrap();
        }

        let store = UsageStore::open(&db_path).unwrap();
        let at = Utc.with_ymd_and_hms(2026, 5, 21, 10, 15, 0).unwrap();
        let rollup = store.rollups_for(UsageSource::ClaudeCode, at).unwrap();
        assert_eq!(rollup.day_tokens, 0);
    }

    #[test]
    fn generation_two_migration_removes_only_codex_local_accounting() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("usage.sqlite");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE hourly_bucket (
                    source TEXT NOT NULL,
                    hour_start INTEGER NOT NULL,
                    tokens_used INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (source, hour_start)
                );
                CREATE TABLE daily_rollup (
                    source TEXT NOT NULL,
                    day_start INTEGER NOT NULL,
                    tokens_used INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (source, day_start)
                );
                CREATE TABLE threshold_state (
                    source TEXT NOT NULL,
                    window_id TEXT NOT NULL,
                    threshold INTEGER NOT NULL,
                    notified_at INTEGER NOT NULL,
                    PRIMARY KEY (source, window_id, threshold)
                );
                CREATE TABLE processed_usage_event (
                    source TEXT NOT NULL,
                    event_id TEXT NOT NULL,
                    recorded_at INTEGER NOT NULL,
                    PRIMARY KEY (source, event_id)
                );
                CREATE TABLE parser_state (
                    parser TEXT NOT NULL,
                    key TEXT NOT NULL,
                    value INTEGER NOT NULL,
                    PRIMARY KEY (parser, key)
                );
                CREATE TABLE schema_meta (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
                INSERT INTO schema_meta VALUES ('local_accounting_generation', '2');
                INSERT INTO hourly_bucket VALUES ('cc', 1779519600, 100);
                INSERT INTO hourly_bucket VALUES ('cx', 1779519600, 200);
                ",
            )
            .unwrap();
        }

        let store = UsageStore::open(&db_path).unwrap();
        let at = Utc.with_ymd_and_hms(2026, 5, 21, 10, 15, 0).unwrap();
        assert_eq!(
            store
                .rollups_for(UsageSource::ClaudeCode, at)
                .unwrap()
                .day_tokens,
            100
        );
        assert_eq!(
            store
                .rollups_for(UsageSource::Codex, at)
                .unwrap()
                .day_tokens,
            0
        );
    }
}
