use super::{parse_epoch_like_timestamp, LocalLogParser, UsageEvent, UsageSource};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const PARSER_NAME: &str = "codex";
const INITIALIZED_KEY: &str = "__initialized";
const MAX_RATE_LIMIT_SESSION_FILES: usize = 12;
const CODEX_LOCAL_ACCOUNTING_ENABLED: bool = true;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexRateLimitStatus {
    #[serde(default = "default_observed_at")]
    pub observed_at: DateTime<Utc>,
    pub used_percent: u8,
    pub remaining_percent: u8,
    pub reset_at: DateTime<Utc>,
    pub window_minutes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct CodexRateLimitCacheFile {
    timestamp: DateTime<Utc>,
    status: CodexRateLimitStatus,
}

#[derive(Debug, Default)]
pub struct CodexParser {
    db_path: Option<PathBuf>,
    last_tokens_by_thread: HashMap<String, u64>,
    state_db_path: Option<PathBuf>,
    state_loaded: bool,
    initialized: bool,
}

impl CodexParser {
    pub fn new() -> Self {
        Self {
            db_path: dirs::home_dir().map(|home| home.join(".codex").join("state_5.sqlite")),
            last_tokens_by_thread: HashMap::new(),
            state_db_path: crate::config::database_path(),
            state_loaded: false,
            initialized: false,
        }
    }

    pub fn from_db(path: PathBuf) -> Self {
        Self {
            db_path: Some(path),
            last_tokens_by_thread: HashMap::new(),
            state_db_path: None,
            state_loaded: false,
            initialized: false,
        }
    }

    #[cfg(test)]
    pub fn with_state_db_path(mut self, path: PathBuf) -> Self {
        self.state_db_path = Some(path);
        self
    }

    pub fn latest_rate_limit_status() -> Option<CodexRateLimitStatus> {
        let root = dirs::home_dir()?.join(".codex").join("sessions");
        let session_status = latest_rate_limit_observation_from_root(&root)
            .ok()
            .flatten();
        let cached_status = latest_cached_rate_limit_status();
        newer_rate_limit_observation(cached_status, session_status)
    }

    fn load_state_if_needed(&mut self) -> anyhow::Result<()> {
        if self.state_loaded {
            return Ok(());
        }
        self.state_loaded = true;
        let Some(path) = &self.state_db_path else {
            self.initialized = true;
            return Ok(());
        };
        let store = crate::storage::UsageStore::open(path)?;
        self.initialized = store
            .get_parser_state_value(PARSER_NAME, INITIALIZED_KEY)?
            .is_some_and(|value| value == 1);
        for (key, value) in store.get_parser_state_values(PARSER_NAME)? {
            if key == INITIALIZED_KEY || value < 0 {
                continue;
            }
            self.last_tokens_by_thread.insert(key, value as u64);
        }
        Ok(())
    }

    fn save_thread_tokens(&self, thread_id: &str, tokens_used: u64) -> anyhow::Result<()> {
        let Some(path) = &self.state_db_path else {
            return Ok(());
        };
        crate::storage::UsageStore::open(path)?.set_parser_state_value(
            PARSER_NAME,
            thread_id,
            tokens_used.min(i64::MAX as u64) as i64,
        )
    }

    fn mark_initialized(&mut self) -> anyhow::Result<()> {
        self.initialized = true;
        let Some(path) = &self.state_db_path else {
            return Ok(());
        };
        crate::storage::UsageStore::open(path)?.set_parser_state_value(
            PARSER_NAME,
            INITIALIZED_KEY,
            1,
        )
    }
}

impl LocalLogParser for CodexParser {
    fn read_delta(&mut self) -> anyhow::Result<Vec<UsageEvent>> {
        if !CODEX_LOCAL_ACCOUNTING_ENABLED {
            return Ok(Vec::new());
        }

        self.load_state_if_needed()?;
        let Some(path) = &self.db_path else {
            return Ok(Vec::new());
        };
        if !path.exists() {
            return Ok(Vec::new());
        }

        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut stmt = conn.prepare(
            "SELECT id, updated_at, tokens_used FROM threads WHERE tokens_used > 0 ORDER BY updated_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let updated_at: i64 = row.get(1)?;
            let tokens_used: u64 = row.get(2)?;
            Ok((id, updated_at, tokens_used))
        })?;
        let rows = rows.collect::<Result<Vec<_>, _>>()?;

        if self.state_db_path.is_some() && !self.initialized {
            for (thread_id, _updated_at, tokens_used) in rows {
                self.last_tokens_by_thread
                    .insert(thread_id.clone(), tokens_used);
                self.save_thread_tokens(&thread_id, tokens_used)?;
            }
            self.mark_initialized()?;
            return Ok(Vec::new());
        }

        let mut events = Vec::new();
        for row in rows {
            let (thread_id, updated_at, tokens_used) = row;
            let previous = self
                .last_tokens_by_thread
                .insert(thread_id.clone(), tokens_used)
                .unwrap_or(0);
            self.save_thread_tokens(&thread_id, tokens_used)?;
            let delta = tokens_used.saturating_sub(previous);
            if delta == 0 {
                continue;
            }
            let occurred_at =
                parse_epoch_like_timestamp(updated_at).unwrap_or_else(chrono::Utc::now);
            events.push(UsageEvent {
                source: UsageSource::Codex,
                event_id: format!("{thread_id}:{tokens_used}"),
                occurred_at,
                tokens: delta,
                metadata: None,
            });
        }
        Ok(events)
    }
}

#[cfg(test)]
fn latest_rate_limit_status_from_root(root: &Path) -> anyhow::Result<Option<CodexRateLimitStatus>> {
    latest_rate_limit_observation_from_root(root)
}

fn latest_rate_limit_observation_from_root(
    root: &Path,
) -> anyhow::Result<Option<CodexRateLimitStatus>> {
    let mut files = Vec::new();
    collect_jsonl_files(root, &mut files);
    files.sort_by(|left, right| {
        file_modified_at(right)
            .cmp(&file_modified_at(left))
            .then_with(|| right.cmp(left))
    });
    files.truncate(MAX_RATE_LIMIT_SESSION_FILES);

    let now = Utc::now();
    let mut latest = None::<CodexRateLimitStatus>;
    for file in files {
        let reader = BufReader::new(File::open(file)?);
        for line in reader.lines().map_while(Result::ok) {
            let Some(status) = parse_rate_limit_line(&line) else {
                continue;
            };
            if status.reset_at <= now {
                continue;
            }
            let is_newer = latest
                .as_ref()
                .map(|latest| status.observed_at > latest.observed_at)
                .unwrap_or(true);
            if is_newer {
                latest = Some(status);
            }
        }
    }
    Ok(latest)
}

fn collect_jsonl_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, files);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            files.push(path);
        }
    }
}

fn file_modified_at(path: &Path) -> SystemTime {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn parse_rate_limit_line(line: &str) -> Option<CodexRateLimitStatus> {
    let value: Value = serde_json::from_str(line).ok()?;
    let timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())?
        .with_timezone(&Utc);
    let primary = value.get("payload")?.get("rate_limits")?.get("primary")?;
    let used_percent = primary.get("used_percent").and_then(Value::as_f64)?;
    let window_minutes = primary
        .get("window_minutes")
        .and_then(Value::as_u64)
        .unwrap_or(300);
    let resets_at = primary.get("resets_at").and_then(Value::as_i64)?;
    let reset_at = Utc.timestamp_opt(resets_at, 0).single()?;
    let used_percent = rounded_percent(used_percent);
    let remaining_percent = 100u8.saturating_sub(used_percent);
    Some(CodexRateLimitStatus {
        observed_at: timestamp,
        used_percent,
        remaining_percent,
        reset_at,
        window_minutes,
    })
}

fn rounded_percent(value: f64) -> u8 {
    value.round().clamp(0.0, 100.0) as u8
}

fn latest_cached_rate_limit_status() -> Option<CodexRateLimitStatus> {
    let path = cached_status_path()?;
    let raw = fs::read_to_string(path).ok()?;
    let cache = serde_json::from_str::<CodexRateLimitCacheFile>(&raw).ok()?;
    let mut status = cache.status;
    if status.observed_at == default_observed_at() {
        status.observed_at = cache.timestamp;
    }
    if status.reset_at <= Utc::now() {
        return None;
    }
    Some(status)
}

fn cached_status_path() -> Option<PathBuf> {
    crate::config::app_support_dir().map(|dir| dir.join("codex-rate-limit.json"))
}

fn newer_rate_limit_observation(
    left: Option<CodexRateLimitStatus>,
    right: Option<CodexRateLimitStatus>,
) -> Option<CodexRateLimitStatus> {
    match (left, right) {
        (Some(left), Some(right)) => {
            if left.observed_at >= right.observed_at {
                Some(left)
            } else {
                Some(right)
            }
        }
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

fn default_observed_at() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(0, 0).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::LocalLogParser;
    use chrono::Duration;
    use rusqlite::{params, Connection};
    use std::fs::OpenOptions;
    use std::io::Write;

    #[test]
    fn local_thread_token_deltas_are_used_for_codex_fallback_accounting() {
        let dir = tempfile::tempdir().expect("temp dir");
        let codex_db = dir.path().join("state_5.sqlite");
        let app_db = dir.path().join("usage.sqlite");
        seed_thread(&codex_db, "thread-a", 100);

        let mut parser = CodexParser::from_db(codex_db.clone()).with_state_db_path(app_db);
        assert!(parser.read_delta().unwrap().is_empty());

        seed_thread(&codex_db, "thread-a", 130);
        let events = parser.read_delta().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].source, UsageSource::Codex);
        assert_eq!(events[0].tokens, 30);
    }

    fn seed_thread(path: &PathBuf, id: &str, tokens: u64) {
        let conn = Connection::open(path).expect("open codex db");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS threads (
                id TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                cwd TEXT NOT NULL,
                tokens_used INTEGER NOT NULL DEFAULT 0,
                model TEXT,
                reasoning_effort TEXT
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, created_at, updated_at, cwd, tokens_used, model, reasoning_effort)
             VALUES (?1, 1779614000, 1779614100, '/', ?2, NULL, NULL)
             ON CONFLICT(id) DO UPDATE SET tokens_used = excluded.tokens_used,
                                           updated_at = excluded.updated_at",
            params![id, tokens as i64],
        )
        .unwrap();
    }

    #[test]
    fn reads_latest_primary_rate_limit_as_remaining_percent() {
        let dir = tempfile::tempdir().expect("temp dir");
        let session = dir.path().join("rollout.jsonl");
        let reset_at = Utc::now() + Duration::hours(3);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&session)
            .unwrap();
        writeln!(
            file,
            r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","rate_limits":{{"primary":{{"used_percent":1.0,"window_minutes":300,"resets_at":{}}}}}}}}}"#,
            Utc::now().to_rfc3339(),
            reset_at.timestamp()
        )
        .unwrap();

        let status = latest_rate_limit_status_from_root(dir.path())
            .unwrap()
            .expect("rate limit");
        assert!(status.observed_at <= Utc::now());
        assert_eq!(status.used_percent, 1);
        assert_eq!(status.remaining_percent, 99);
        assert_eq!(status.window_minutes, 300);
        assert_eq!(status.reset_at.timestamp(), reset_at.timestamp());
    }

    #[test]
    fn prefers_newer_cached_codex_status_over_stale_session_status() {
        let reset_at = Utc::now() + Duration::hours(3);
        let old = CodexRateLimitStatus {
            observed_at: Utc::now() - Duration::minutes(10),
            used_percent: 23,
            remaining_percent: 77,
            reset_at,
            window_minutes: 300,
        };
        let new = CodexRateLimitStatus {
            observed_at: Utc::now(),
            used_percent: 63,
            remaining_percent: 37,
            reset_at,
            window_minutes: 300,
        };

        let status = newer_rate_limit_observation(Some(new), Some(old)).expect("status");
        assert_eq!(status.used_percent, 63);
        assert_eq!(status.remaining_percent, 37);
    }
}
