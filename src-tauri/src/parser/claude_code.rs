use super::{LocalLogParser, UsageEvent, UsageSource};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration as StdDuration, SystemTime};

const PARSER_NAME: &str = "claude_code";
const INITIALIZED_KEY: &str = "__initialized";
const CLAUDE_PRIMARY_WINDOW_MINUTES: u64 = 300;
const RATE_LIMIT_FETCH_TTL: StdDuration = StdDuration::from_secs(120);
const RATE_LIMIT_STALE_TTL: chrono::Duration = chrono::Duration::minutes(15);
const RATE_LIMIT_CACHE_VERSION: u8 = 2;
const CLAUDE_USAGE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";

static RATE_LIMIT_CACHE: OnceLock<Mutex<RateLimitMemoryCache>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeRateLimitStatus {
    pub used_percent: u8,
    pub remaining_percent: u8,
    pub reset_at: DateTime<Utc>,
    pub window_minutes: u64,
}

#[derive(Debug, Default)]
struct RateLimitMemoryCache {
    checked_at: Option<SystemTime>,
    status: Option<ClaudeRateLimitStatus>,
}

#[derive(Debug, Default)]
pub struct ClaudeCodeParser {
    roots: Vec<PathBuf>,
    explicit_files: Vec<PathBuf>,
    offsets: HashMap<PathBuf, u64>,
    state_db_path: Option<PathBuf>,
    state_loaded: bool,
    initialized: bool,
}

impl ClaudeCodeParser {
    pub fn new() -> Self {
        let roots = dirs::home_dir()
            .map(|home| vec![home.join(".claude").join("projects")])
            .unwrap_or_default();
        Self {
            roots,
            explicit_files: Vec::new(),
            offsets: HashMap::new(),
            state_db_path: crate::config::database_path(),
            state_loaded: false,
            initialized: false,
        }
    }

    pub fn from_files(files: Vec<PathBuf>) -> Self {
        Self {
            roots: Vec::new(),
            explicit_files: files,
            offsets: HashMap::new(),
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

    pub fn latest_rate_limit_status() -> Option<ClaudeRateLimitStatus> {
        let cache = RATE_LIMIT_CACHE.get_or_init(|| Mutex::new(RateLimitMemoryCache::default()));
        if let Ok(cache) = cache.lock() {
            if cache
                .checked_at
                .and_then(|checked_at| checked_at.elapsed().ok())
                .is_some_and(|elapsed| elapsed < RATE_LIMIT_FETCH_TTL)
            {
                return cache.status.clone();
            }
        }

        let stale_status = latest_cached_rate_limit_status();
        let fetched_status = fetch_rate_limit_status().ok().flatten();
        if let Some(status) = &fetched_status {
            let _ = write_cached_rate_limit_status(status);
        }
        let status = fetched_status.or(stale_status);
        if let Ok(mut cache) = cache.lock() {
            cache.checked_at = Some(SystemTime::now());
            cache.status = status.clone();
        }
        status
    }

    fn discover_files(&self) -> Vec<PathBuf> {
        if !self.explicit_files.is_empty() {
            return self.explicit_files.clone();
        }

        let mut files = Vec::new();
        for root in &self.roots {
            collect_jsonl(root, &mut files);
        }
        files.sort();
        files
    }

    fn read_file_delta(&mut self, path: &Path) -> anyhow::Result<Vec<UsageEvent>> {
        let metadata = fs::metadata(path)?;
        let size = metadata.len();
        let previous_offset = self.offsets.get(path).copied().unwrap_or(0);
        let offset = if size < previous_offset {
            0
        } else {
            previous_offset
        };

        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut reader = BufReader::new(file);
        let mut events = Vec::new();
        let mut line = String::new();

        loop {
            let line_offset = reader.stream_position()?;
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            if let Some(event) = parse_usage_line(path, line_offset, line.trim_end()) {
                events.push(event);
            }
            line.clear();
        }

        let new_offset = reader.stream_position()?;
        self.offsets.insert(path.to_path_buf(), new_offset);
        self.save_offset(path, new_offset)?;
        Ok(events)
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
            self.offsets.insert(PathBuf::from(key), value as u64);
        }
        Ok(())
    }

    fn save_offset(&self, path: &Path, offset: u64) -> anyhow::Result<()> {
        let Some(db_path) = &self.state_db_path else {
            return Ok(());
        };
        crate::storage::UsageStore::open(db_path)?.set_parser_state_value(
            PARSER_NAME,
            path.to_string_lossy().as_ref(),
            offset.min(i64::MAX as u64) as i64,
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

impl LocalLogParser for ClaudeCodeParser {
    fn read_delta(&mut self) -> anyhow::Result<Vec<UsageEvent>> {
        self.load_state_if_needed()?;
        let mut events = Vec::new();
        let should_baseline = self.state_db_path.is_some() && !self.initialized;
        for path in self.discover_files() {
            if path.is_file() {
                if should_baseline {
                    let size = fs::metadata(&path)?.len();
                    self.offsets.insert(path.clone(), size);
                    self.save_offset(&path, size)?;
                } else {
                    events.extend(self.read_file_delta(&path)?);
                }
            }
        }
        if should_baseline {
            self.mark_initialized()?;
            return Ok(Vec::new());
        }
        events.sort_by_key(|event| event.occurred_at);
        Ok(events)
    }
}

fn collect_jsonl(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
            files.push(path);
        }
    }
}

fn parse_usage_line(path: &Path, line_offset: u64, line: &str) -> Option<UsageEvent> {
    if line.trim().is_empty() {
        return None;
    }

    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }

    let usage = value.get("message")?.get("usage")?;
    let tokens = [
        "input_tokens",
        "output_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
    ]
    .iter()
    .filter_map(|key| usage.get(*key).and_then(Value::as_u64))
    .sum::<u64>();

    if tokens == 0 {
        return None;
    }

    let occurred_at = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|ts| ts.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    Some(UsageEvent {
        source: UsageSource::ClaudeCode,
        event_id: format!("{}:{line_offset}", path.display()),
        occurred_at,
        tokens,
        metadata: None,
    })
}

#[derive(Debug, Deserialize)]
struct OAuthCredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OAuthCredentials>,
    #[serde(flatten)]
    flat: OAuthCredentials,
}

#[derive(Debug, Default, Deserialize)]
struct OAuthCredentials {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaudeUsageApiResponse {
    five_hour: Option<UsageWindow>,
    seven_day: Option<UsageWindow>,
}

#[derive(Debug, Deserialize)]
struct UsageWindow {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ClaudeRateLimitCacheFile {
    #[serde(default)]
    format_version: u8,
    timestamp: DateTime<Utc>,
    status: ClaudeRateLimitStatus,
}

fn fetch_rate_limit_status() -> anyhow::Result<Option<ClaudeRateLimitStatus>> {
    let Some(access_token) = read_oauth_access_token() else {
        return Ok(None);
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(StdDuration::from_secs(10))
        .build()?;
    let response = client
        .get(CLAUDE_USAGE_ENDPOINT)
        .bearer_auth(access_token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("Content-Type", "application/json")
        .send()?;
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Ok(None);
    }
    let usage = response.json::<ClaudeUsageApiResponse>()?;
    Ok(parse_usage_api_response(&usage, Utc::now()))
}

fn read_oauth_access_token() -> Option<String> {
    read_oauth_access_token_from_keychain()
        .or_else(read_oauth_access_token_from_file)
        .filter(|token| !token.trim().is_empty())
}

fn read_oauth_access_token_from_keychain() -> Option<String> {
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
    #[cfg(target_os = "macos")]
    {
        let mut candidates = Vec::new();
        if let Ok(user) = std::env::var("USER") {
            if !user.trim().is_empty() {
                candidates.push(Some(user));
            }
        }
        candidates.push(None);

        for account in candidates {
            let mut command = Command::new("/usr/bin/security");
            command
                .arg("find-generic-password")
                .arg("-s")
                .arg("Claude Code-credentials");
            if let Some(account) = account {
                command.arg("-a").arg(account);
            }
            command.arg("-w");
            let Ok(output) = command.output() else {
                continue;
            };
            if !output.status.success() {
                continue;
            }
            let Ok(raw) = String::from_utf8(output.stdout) else {
                continue;
            };
            if let Some(token) = parse_oauth_access_token(&raw) {
                return Some(token);
            }
        }
        None
    }
}

fn read_oauth_access_token_from_file() -> Option<String> {
    let path = dirs::home_dir()?.join(".claude").join(".credentials.json");
    let raw = fs::read_to_string(path).ok()?;
    parse_oauth_access_token(&raw)
}

fn parse_oauth_access_token(raw: &str) -> Option<String> {
    let parsed = serde_json::from_str::<OAuthCredentialsFile>(raw).ok()?;
    parsed
        .claude_ai_oauth
        .and_then(|credentials| credentials.access_token)
        .or(parsed.flat.access_token)
}

fn parse_usage_api_response(
    response: &ClaudeUsageApiResponse,
    now: DateTime<Utc>,
) -> Option<ClaudeRateLimitStatus> {
    parse_usage_window(
        response.five_hour.as_ref(),
        CLAUDE_PRIMARY_WINDOW_MINUTES,
        now,
    )
    .or_else(|| parse_usage_window(response.seven_day.as_ref(), 7 * 24 * 60, now))
}

fn parse_usage_window(
    window: Option<&UsageWindow>,
    window_minutes: u64,
    now: DateTime<Utc>,
) -> Option<ClaudeRateLimitStatus> {
    let window = window?;
    let reset_at = window
        .resets_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())?
        .with_timezone(&Utc);
    if reset_at <= now {
        return None;
    }
    let used_percent = rounded_percent(window.utilization?);
    Some(ClaudeRateLimitStatus {
        used_percent,
        remaining_percent: 100u8.saturating_sub(used_percent),
        reset_at,
        window_minutes,
    })
}

fn rounded_percent(value: f64) -> u8 {
    let normalized = if (0.0..=1.0).contains(&value) {
        value * 100.0
    } else {
        value
    };
    normalized.round().clamp(0.0, 100.0) as u8
}

fn latest_cached_rate_limit_status() -> Option<ClaudeRateLimitStatus> {
    cached_status_path()
        .and_then(|path| read_cached_rate_limit_status(&path))
        .or_else(latest_omc_cached_rate_limit_status)
}

fn read_cached_rate_limit_status(path: &Path) -> Option<ClaudeRateLimitStatus> {
    let raw = fs::read_to_string(path).ok()?;
    let cache = serde_json::from_str::<ClaudeRateLimitCacheFile>(&raw).ok()?;
    if cache.format_version != RATE_LIMIT_CACHE_VERSION {
        return None;
    }
    let now = Utc::now();
    if cache.status.reset_at <= now || now - cache.timestamp > RATE_LIMIT_STALE_TTL {
        return None;
    }
    Some(cache.status)
}

fn write_cached_rate_limit_status(status: &ClaudeRateLimitStatus) -> anyhow::Result<()> {
    let Some(path) = cached_status_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let cache = ClaudeRateLimitCacheFile {
        format_version: RATE_LIMIT_CACHE_VERSION,
        timestamp: Utc::now(),
        status: status.clone(),
    };
    fs::write(path, serde_json::to_string_pretty(&cache)?)?;
    Ok(())
}

fn cached_status_path() -> Option<PathBuf> {
    crate::config::app_support_dir().map(|dir| dir.join("claude-rate-limit.json"))
}

fn latest_omc_cached_rate_limit_status() -> Option<ClaudeRateLimitStatus> {
    let path = dirs::home_dir()?
        .join(".claude")
        .join("plugins")
        .join("oh-my-claudecode")
        .join(".usage-cache-anthropic.json");
    let raw = fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<Value>(&raw).ok()?;
    let data = value.get("data")?;
    let timestamp = value
        .get("timestamp")
        .and_then(Value::as_i64)
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or_else(Utc::now);
    if Utc::now() - timestamp > RATE_LIMIT_STALE_TTL {
        return None;
    }
    parse_omc_rate_limit_data(data, Utc::now())
}

fn parse_omc_rate_limit_data(data: &Value, now: DateTime<Utc>) -> Option<ClaudeRateLimitStatus> {
    let five_hour_percent = data.get("fiveHourPercent").and_then(Value::as_f64);
    let five_hour_resets_at = data.get("fiveHourResetsAt").and_then(Value::as_str);
    let weekly_percent = data.get("weeklyPercent").and_then(Value::as_f64);
    let weekly_resets_at = data.get("weeklyResetsAt").and_then(Value::as_str);

    parse_omc_rate_limit_window(five_hour_percent, five_hour_resets_at, 300, now)
        .or_else(|| parse_omc_rate_limit_window(weekly_percent, weekly_resets_at, 7 * 24 * 60, now))
}

fn parse_omc_rate_limit_window(
    percent: Option<f64>,
    resets_at: Option<&str>,
    window_minutes: u64,
    now: DateTime<Utc>,
) -> Option<ClaudeRateLimitStatus> {
    let reset_at = DateTime::parse_from_rfc3339(resets_at?)
        .ok()?
        .with_timezone(&Utc);
    if reset_at <= now {
        return None;
    }
    let used_percent = rounded_percent(percent?);
    Some(ClaudeRateLimitStatus {
        used_percent,
        remaining_percent: 100u8.saturating_sub(used_percent),
        reset_at,
        window_minutes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::LocalLogParser;
    use chrono::Duration;
    use std::fs::OpenOptions;
    use std::io::Write;

    #[test]
    fn persisted_state_baselines_existing_claude_logs_once() {
        let dir = tempfile::tempdir().expect("temp dir");
        let log = dir.path().join("session.jsonl");
        let db = dir.path().join("usage.sqlite");
        append_usage_line(&log, 10);

        let mut parser = ClaudeCodeParser::from_files(vec![log.clone()]).with_state_db_path(db);
        assert!(parser.read_delta().unwrap().is_empty());

        append_usage_line(&log, 7);
        let events = parser.read_delta().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].tokens, 7);
    }

    #[test]
    fn parses_claude_usage_endpoint_as_remaining_percent() {
        let reset_at = Utc::now() + Duration::hours(4);
        let response = ClaudeUsageApiResponse {
            five_hour: Some(UsageWindow {
                utilization: Some(21.3),
                resets_at: Some(reset_at.to_rfc3339()),
            }),
            seven_day: Some(UsageWindow {
                utilization: Some(6.0),
                resets_at: Some((Utc::now() + Duration::days(6)).to_rfc3339()),
            }),
        };

        let status = parse_usage_api_response(&response, Utc::now()).expect("status");
        assert_eq!(status.used_percent, 21);
        assert_eq!(status.remaining_percent, 79);
        assert_eq!(status.window_minutes, 300);
        assert_eq!(status.reset_at.timestamp(), reset_at.timestamp());
    }

    #[test]
    fn ignores_legacy_retry_after_rate_limit_cache() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("claude-rate-limit.json");
        let now = Utc::now();
        fs::write(
            &path,
            serde_json::json!({
                "timestamp": now,
                "status": {
                    "used_percent": 100,
                    "remaining_percent": 0,
                    "reset_at": now + Duration::minutes(30),
                    "window_minutes": 300
                }
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(read_cached_rate_limit_status(&path), None);
    }

    #[test]
    fn reads_versioned_claude_rate_limit_cache() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("claude-rate-limit.json");
        let now = Utc::now();
        fs::write(
            &path,
            serde_json::json!({
                "format_version": RATE_LIMIT_CACHE_VERSION,
                "timestamp": now,
                "status": {
                    "used_percent": 21,
                    "remaining_percent": 79,
                    "reset_at": now + Duration::hours(4),
                    "window_minutes": 300
                }
            })
            .to_string(),
        )
        .unwrap();

        let status = read_cached_rate_limit_status(&path).expect("status");
        assert_eq!(status.remaining_percent, 79);
    }

    #[test]
    fn parses_omc_cache_as_remaining_percent() {
        let reset_at = Utc::now() + Duration::hours(2);
        let data = serde_json::json!({
            "fiveHourPercent": 33.6,
            "fiveHourResetsAt": reset_at.to_rfc3339(),
            "weeklyPercent": 10.0,
            "weeklyResetsAt": (Utc::now() + Duration::days(3)).to_rfc3339()
        });

        let status = parse_omc_rate_limit_data(&data, Utc::now()).expect("status");
        assert_eq!(status.used_percent, 34);
        assert_eq!(status.remaining_percent, 66);
        assert_eq!(status.window_minutes, 300);
    }

    fn append_usage_line(path: &Path, output_tokens: u64) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open log");
        writeln!(
            file,
            r#"{{"type":"assistant","timestamp":"2026-05-21T01:01:24.096Z","message":{{"usage":{{"output_tokens":{output_tokens}}}}}}}"#
        )
        .unwrap();
    }
}
