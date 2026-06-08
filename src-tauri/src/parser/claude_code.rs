use super::{LocalLogParser, UsageEvent, UsageSource};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration as StdDuration, SystemTime};

const PARSER_NAME: &str = "claude_code";
const INITIALIZED_KEY: &str = "__initialized";
const CLAUDE_PRIMARY_WINDOW_MINUTES: u64 = 300;
const CACHE_READ_TOKEN_WEIGHT_DIVISOR: u64 = 10;
const RATE_LIMIT_FETCH_TTL: StdDuration = StdDuration::from_secs(120);
const RATE_LIMIT_STALE_TTL: chrono::Duration = chrono::Duration::minutes(15);
const RATE_LIMIT_CACHE_VERSION: u8 = 2;
const CLAUDE_USAGE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_OAUTH_TOKEN_ENDPOINT: &str = "https://console.anthropic.com/v1/oauth/token";
const CLAUDE_OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_CODE_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const CLAUDE_OAUTH_DIAGNOSTIC_LOG: &str = "claude-oauth.log";
/// Claude Code CLI 와 동일한 키체인 자격증명(`CLAUDE_CODE_KEYCHAIN_SERVICE`)을
/// 공유하기 때문에, token-notifier 가 OAuth refresh 를 수행하면 rotating refresh
/// token 이 회전되어 CLI 가 들고 있던 토큰이 무효화된다. 그 결과 사용자가 매일
/// 아침 Claude Code 에서 `/login` 을 다시 요구받는 회귀가 발생했다. 따라서 refresh
/// 를 비활성화하고 자격증명을 읽기 전용으로만 사용한다. 만료된 access token 은 CLI
/// 가 스스로 갱신할 때 키체인에 반영되며, 다음 폴링에서 자연히 따라간다. Anthropic
/// 이 non-rotating refresh token 으로 바꾸기 전까지 false 를 유지할 것.
const CLAUDE_OAUTH_REFRESH_ENABLED: bool = false;

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

    fn read_file_bootstrap_window(
        &mut self,
        path: &Path,
        window_start: DateTime<Utc>,
    ) -> anyhow::Result<Vec<UsageEvent>> {
        let mut reader = BufReader::new(File::open(path)?);
        let mut events = Vec::new();
        let mut line = String::new();

        loop {
            let line_offset = reader.stream_position()?;
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            if let Some(event) = parse_usage_line(path, line_offset, line.trim_end()) {
                if event.occurred_at >= window_start {
                    events.push(event);
                }
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
    fn source(&self) -> UsageSource {
        UsageSource::ClaudeCode
    }

    fn read_delta(&mut self) -> anyhow::Result<Vec<UsageEvent>> {
        self.load_state_if_needed()?;
        let mut events = Vec::new();
        let should_baseline = self.state_db_path.is_some() && !self.initialized;
        let bootstrap_window_start = if should_baseline {
            let window_secs = crate::config::HiddenConfig::load()
                .default_window_secs
                .max(60);
            i64::try_from(window_secs)
                .ok()
                .map(|secs| Utc::now() - ChronoDuration::seconds(secs))
        } else {
            None
        };
        for path in self.discover_files() {
            if path.is_file() {
                if let Some(window_start) = bootstrap_window_start {
                    events.extend(self.read_file_bootstrap_window(&path, window_start)?);
                } else {
                    events.extend(self.read_file_delta(&path)?);
                }
            }
        }
        if should_baseline {
            self.mark_initialized()?;
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

    let message = value.get("message")?;
    let usage = message.get("usage")?;
    let tokens = effective_usage_tokens(usage);

    if tokens == 0 {
        return None;
    }
    let event_id = message
        .get("id")
        .and_then(Value::as_str)
        .map(|message_id| format!("{}:{message_id}", path.display()))
        .unwrap_or_else(|| format!("{}:{line_offset}", path.display()));

    let occurred_at = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|ts| ts.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    Some(UsageEvent {
        source: UsageSource::ClaudeCode,
        event_id,
        occurred_at,
        tokens,
        metadata: None,
    })
}

fn effective_usage_tokens(usage: &Value) -> u64 {
    let direct_tokens = [
        "input_tokens",
        "output_tokens",
        "cache_creation_input_tokens",
    ]
    .iter()
    .filter_map(|key| usage.get(*key).and_then(Value::as_u64))
    .sum::<u64>();
    let cache_read = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    direct_tokens.saturating_add(cache_read / CACHE_READ_TOKEN_WEIGHT_DIVISOR)
}

#[derive(Debug)]
struct StoredOAuthCredentials {
    access_token: String,
    refresh_token: Option<String>,
    value: Value,
    encoding: OAuthCredentialEncoding,
    storage: OAuthCredentialStorage,
}

#[derive(Debug)]
enum OAuthCredentialStorage {
    Keychain { account: Option<String> },
    File { path: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OAuthCredentialEncoding {
    Json,
    HexJson,
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

#[derive(Debug, Serialize)]
struct OAuthRefreshRequest<'a> {
    grant_type: &'static str,
    refresh_token: &'a str,
    client_id: &'static str,
}

#[derive(Debug, Deserialize)]
struct OAuthRefreshResponse {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    expires_in: Option<i64>,
}

enum UsageFetchResult {
    Status(Option<ClaudeRateLimitStatus>),
    NeedsRefresh(reqwest::StatusCode),
}

fn fetch_rate_limit_status() -> anyhow::Result<Option<ClaudeRateLimitStatus>> {
    let Some(mut credentials) = read_oauth_credentials() else {
        log_claude_oauth_event("credentials unavailable; usage request skipped");
        return Ok(None);
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(StdDuration::from_secs(10))
        .build()?;

    log_claude_oauth_event("usage request started");
    match fetch_usage_status(&client, &credentials.access_token) {
        Ok(UsageFetchResult::Status(status)) => {
            log_usage_fetch_status("usage request", status.as_ref());
            Ok(status)
        }
        Ok(UsageFetchResult::NeedsRefresh(status)) => {
            if CLAUDE_OAUTH_REFRESH_ENABLED {
                log_claude_oauth_event(format!(
                    "usage request returned {status}; attempting OAuth refresh"
                ));
                refresh_and_retry_usage_status(&client, &mut credentials)
            } else {
                // 읽기 전용 정책: refresh 시 CLI 와 공유하는 rotating refresh token 이
                // 회전돼 CLI 세션이 무효화되므로(매일 /login 요구 원인) refresh 하지 않고
                // 공식 데이터 없음으로 둔다. CLAUDE_OAUTH_REFRESH_ENABLED 주석 참고.
                log_claude_oauth_event(format!(
                    "usage request returned {status}; OAuth refresh disabled by read-only policy"
                ));
                Ok(None)
            }
        }
        Err(error) => {
            log_claude_oauth_event(format!("usage request failed: {error:#}"));
            Ok(None)
        }
    }
}

fn refresh_and_retry_usage_status(
    client: &reqwest::blocking::Client,
    credentials: &mut StoredOAuthCredentials,
) -> anyhow::Result<Option<ClaudeRateLimitStatus>> {
    let Some(refresh_token) = credentials.refresh_token.clone() else {
        log_claude_oauth_event("OAuth refresh skipped; refresh token is unavailable");
        return Ok(None);
    };
    let refreshed = match refresh_oauth_tokens(client, &refresh_token) {
        Ok(refreshed) => {
            log_claude_oauth_event("OAuth refresh succeeded; persisting replacement credentials");
            refreshed
        }
        Err(error) => {
            log_claude_oauth_event(format!("OAuth refresh failed: {error:#}"));
            return Ok(None);
        }
    };
    if let Err(error) = update_and_save_oauth_credentials(credentials, refreshed) {
        log_claude_oauth_event(format!(
            "credential persistence failed after refresh: {error:#}"
        ));
        return Ok(None);
    }
    log_claude_oauth_event("replacement credentials persisted; retrying usage request");
    match fetch_usage_status(client, &credentials.access_token) {
        Ok(UsageFetchResult::Status(status)) => {
            log_usage_fetch_status("usage retry", status.as_ref());
            Ok(status)
        }
        Ok(UsageFetchResult::NeedsRefresh(status)) => {
            log_claude_oauth_event(format!("usage retry returned {status} after OAuth refresh"));
            Ok(None)
        }
        Err(error) => {
            log_claude_oauth_event(format!("usage retry failed after OAuth refresh: {error:#}"));
            Ok(None)
        }
    }
}

fn log_usage_fetch_status(prefix: &str, status: Option<&ClaudeRateLimitStatus>) {
    match status {
        Some(status) => log_claude_oauth_event(format!(
            "{prefix} succeeded; remaining={}%, used={}%, reset_at={}",
            status.remaining_percent, status.used_percent, status.reset_at
        )),
        None => log_claude_oauth_event(format!("{prefix} returned no usable official status")),
    }
}

fn log_claude_oauth_event(message: impl AsRef<str>) {
    let timestamp = Utc::now().to_rfc3339();
    let line = format!("{timestamp} {message}\n", message = message.as_ref());
    eprint!("{line}");
    let Some(path) = claude_oauth_diagnostic_log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

fn claude_oauth_diagnostic_log_path() -> Option<PathBuf> {
    crate::config::app_support_dir().map(|dir| dir.join(CLAUDE_OAUTH_DIAGNOSTIC_LOG))
}

fn fetch_usage_status(
    client: &reqwest::blocking::Client,
    access_token: &str,
) -> anyhow::Result<UsageFetchResult> {
    let response = client
        .get(CLAUDE_USAGE_ENDPOINT)
        .bearer_auth(access_token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("Content-Type", "application/json")
        .send()?;
    let status = response.status();
    if matches!(
        status,
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::TOO_MANY_REQUESTS
    ) {
        log_claude_oauth_event(format!(
            "usage request returned refreshable HTTP {status}: {}",
            summarize_http_body(response.text().unwrap_or_default())
        ));
        return Ok(UsageFetchResult::NeedsRefresh(status));
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        log_claude_oauth_event(format!(
            "usage request returned non-refreshable HTTP {status}; Claude Code /login may be required: {}",
            summarize_http_body(response.text().unwrap_or_default())
        ));
        return Ok(UsageFetchResult::Status(None));
    }
    if !status.is_success() {
        log_claude_oauth_event(format!(
            "usage request returned HTTP {status}: {}",
            summarize_http_body(response.text().unwrap_or_default())
        ));
        return Ok(UsageFetchResult::Status(None));
    }
    let usage = response.json::<ClaudeUsageApiResponse>()?;
    Ok(UsageFetchResult::Status(parse_usage_api_response(
        &usage,
        Utc::now(),
    )))
}

fn refresh_oauth_tokens(
    client: &reqwest::blocking::Client,
    refresh_token: &str,
) -> anyhow::Result<OAuthRefreshResponse> {
    let response = client
        .post(CLAUDE_OAUTH_TOKEN_ENDPOINT)
        .header("Content-Type", "application/json")
        .json(&OAuthRefreshRequest {
            grant_type: "refresh_token",
            refresh_token,
            client_id: CLAUDE_OAUTH_CLIENT_ID,
        })
        .send()?;
    if !response.status().is_success() {
        anyhow::bail!(
            "Claude OAuth token refresh failed with {}",
            response.status()
        );
    }
    Ok(response.json::<OAuthRefreshResponse>()?)
}

fn update_and_save_oauth_credentials(
    credentials: &mut StoredOAuthCredentials,
    refreshed: OAuthRefreshResponse,
) -> anyhow::Result<()> {
    update_oauth_credentials_value(&mut credentials.value, &refreshed, Utc::now())?;
    let raw = encode_oauth_credentials_for_storage(&credentials.value, credentials.encoding)?;
    match &credentials.storage {
        OAuthCredentialStorage::Keychain { account } => {
            write_oauth_credentials_to_keychain(&raw, account.as_deref())?;
        }
        OAuthCredentialStorage::File { path } => {
            fs::write(path, raw)?;
        }
    }
    credentials.access_token = refreshed.access_token;
    credentials.refresh_token = Some(refreshed.refresh_token);
    Ok(())
}

fn update_oauth_credentials_value(
    value: &mut Value,
    refreshed: &OAuthRefreshResponse,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    let Some(object) = oauth_credentials_object_mut(value) else {
        anyhow::bail!("Claude OAuth credentials are not a JSON object");
    };
    let existing_expires_at = object.get("expiresAt").cloned();
    object.insert(
        "accessToken".to_string(),
        Value::String(refreshed.access_token.clone()),
    );
    object.insert(
        "refreshToken".to_string(),
        Value::String(refreshed.refresh_token.clone()),
    );
    if let Some(expires_at) =
        refreshed_expires_at_value(existing_expires_at.as_ref(), refreshed.expires_in, now)
    {
        object.insert("expiresAt".to_string(), expires_at);
    }
    Ok(())
}

fn refreshed_expires_at_value(
    existing: Option<&Value>,
    expires_in: Option<i64>,
    now: DateTime<Utc>,
) -> Option<Value> {
    let expires_in = expires_in?;
    if expires_in < 0 {
        return None;
    }
    let is_seconds = existing
        .and_then(Value::as_i64)
        .is_some_and(|value| value < 10_000_000_000);
    let timestamp = if is_seconds {
        now.timestamp().saturating_add(expires_in)
    } else {
        now.timestamp_millis()
            .saturating_add(expires_in.saturating_mul(1000))
    };
    Some(Value::Number(Number::from(timestamp)))
}

fn read_oauth_credentials() -> Option<StoredOAuthCredentials> {
    if let Some(credentials) = read_oauth_credentials_from_keychain() {
        if !credentials.access_token.trim().is_empty() {
            log_claude_oauth_event("credentials loaded from macOS Keychain");
            return Some(credentials);
        }
        log_claude_oauth_event("macOS Keychain credentials had an empty access token");
    }
    if let Some(credentials) = read_oauth_credentials_from_file() {
        if !credentials.access_token.trim().is_empty() {
            log_claude_oauth_event("credentials loaded from ~/.claude/.credentials.json");
            return Some(credentials);
        }
        log_claude_oauth_event("file credentials had an empty access token");
    }
    None
}

fn read_oauth_credentials_from_keychain() -> Option<StoredOAuthCredentials> {
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
            let account_label = account
                .as_deref()
                .map(str::to_string)
                .unwrap_or_else(|| "<default>".to_string());
            let mut command = Command::new("/usr/bin/security");
            command
                .arg("find-generic-password")
                .arg("-s")
                .arg(CLAUDE_CODE_KEYCHAIN_SERVICE);
            if let Some(account) = account.as_deref() {
                command.arg("-a").arg(account);
            }
            command.arg("-w");
            let Ok(output) = command.output() else {
                log_claude_oauth_event(format!(
                    "Keychain lookup command failed for account={account_label}"
                ));
                continue;
            };
            if !output.status.success() {
                log_claude_oauth_event(format!(
                    "Keychain lookup returned status={} for account={account_label}: {}",
                    output.status,
                    sanitized_command_stderr(&output.stderr)
                ));
                continue;
            }
            let Ok(raw) = String::from_utf8(output.stdout) else {
                log_claude_oauth_event(format!(
                    "Keychain lookup returned non-UTF8 credentials for account={account_label}"
                ));
                continue;
            };
            if let Some(credentials) =
                parse_stored_oauth_credentials(&raw, OAuthCredentialStorage::Keychain { account })
            {
                return Some(credentials);
            }
            log_claude_oauth_event(format!(
                "Keychain credentials were present but did not contain Claude OAuth tokens for account={account_label}"
            ));
        }
        None
    }
}

fn read_oauth_credentials_from_file() -> Option<StoredOAuthCredentials> {
    let path = dirs::home_dir()?.join(".claude").join(".credentials.json");
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) => {
            log_claude_oauth_event(format!(
                "file credential lookup failed at {}: {error}",
                path.display()
            ));
            return None;
        }
    };
    let credentials = parse_stored_oauth_credentials(&raw, OAuthCredentialStorage::File { path });
    if credentials.is_none() {
        log_claude_oauth_event(
            "file credentials were present but did not contain Claude OAuth tokens",
        );
    }
    credentials
}

fn sanitized_command_stderr(stderr: &[u8]) -> String {
    let message = String::from_utf8_lossy(stderr);
    let trimmed = message.trim();
    if trimmed.is_empty() {
        "<empty stderr>".to_string()
    } else {
        trimmed.to_string()
    }
}

fn summarize_http_body(body: String) -> String {
    let single_line = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.is_empty() {
        "<empty body>".to_string()
    } else {
        single_line.chars().take(500).collect()
    }
}

fn parse_stored_oauth_credentials(
    raw: &str,
    storage: OAuthCredentialStorage,
) -> Option<StoredOAuthCredentials> {
    let (value, encoding) = parse_oauth_credentials_value(raw)?;
    let access_token = oauth_credentials_object(&value)?
        .get("accessToken")
        .and_then(Value::as_str)?
        .trim()
        .to_string();
    let refresh_token = oauth_credentials_object(&value)?
        .get("refreshToken")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string);
    Some(StoredOAuthCredentials {
        access_token,
        refresh_token,
        value,
        encoding,
        storage,
    })
}

fn parse_oauth_credentials_value(raw: &str) -> Option<(Value, OAuthCredentialEncoding)> {
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        return Some((value, OAuthCredentialEncoding::Json));
    }
    let decoded = decode_hex_json(raw)?;
    serde_json::from_str::<Value>(&decoded)
        .ok()
        .map(|value| (value, OAuthCredentialEncoding::HexJson))
}

fn encode_oauth_credentials_for_storage(
    value: &Value,
    encoding: OAuthCredentialEncoding,
) -> anyhow::Result<String> {
    let raw = serde_json::to_string_pretty(value)?;
    Ok(match encoding {
        OAuthCredentialEncoding::Json => raw,
        OAuthCredentialEncoding::HexJson => encode_hex(raw.as_bytes()),
    })
}

fn decode_hex_json(raw: &str) -> Option<String> {
    let hex = raw.trim();
    if hex.len() % 2 != 0 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let bytes = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|chunk| {
            let text = std::str::from_utf8(chunk).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn oauth_credentials_object(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    value
        .get("claudeAiOauth")
        .and_then(Value::as_object)
        .or_else(|| value.as_object())
}

fn oauth_credentials_object_mut(value: &mut Value) -> Option<&mut serde_json::Map<String, Value>> {
    if value
        .get("claudeAiOauth")
        .is_some_and(|oauth| oauth.is_object())
    {
        return value.get_mut("claudeAiOauth")?.as_object_mut();
    }
    value.as_object_mut()
}

#[cfg(target_os = "macos")]
fn write_oauth_credentials_to_keychain(raw: &str, account: Option<&str>) -> anyhow::Result<()> {
    let mut command = Command::new("/usr/bin/security");
    command
        .arg("add-generic-password")
        .arg("-U")
        .arg("-s")
        .arg(CLAUDE_CODE_KEYCHAIN_SERVICE);
    if let Some(account) = account {
        command.arg("-a").arg(account);
    }
    command.arg("-w").arg(raw);
    let output = command.output()?;
    if !output.status.success() {
        anyhow::bail!("failed to update Claude Code credentials in Keychain");
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn write_oauth_credentials_to_keychain(_raw: &str, _account: Option<&str>) -> anyhow::Result<()> {
    anyhow::bail!("Keychain credentials are only supported on macOS")
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
    fn persisted_state_bootstraps_current_window_logs_once() {
        let dir = tempfile::tempdir().expect("temp dir");
        let log = dir.path().join("session.jsonl");
        let db = dir.path().join("usage.sqlite");
        append_usage_line_at(&log, 10, Utc::now());

        let mut parser =
            ClaudeCodeParser::from_files(vec![log.clone()]).with_state_db_path(db.clone());
        let events = parser.read_delta().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].tokens, 10);
        assert!(parser.read_delta().unwrap().is_empty());

        append_usage_line_at(&log, 7, Utc::now());
        let events = parser.read_delta().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].tokens, 7);

        let mut restarted = ClaudeCodeParser::from_files(vec![log]).with_state_db_path(db);
        assert!(restarted.read_delta().unwrap().is_empty());
    }

    #[test]
    fn uses_message_id_for_stable_deduplication() {
        let path = Path::new("/tmp/session.jsonl");
        let line = r#"{"type":"assistant","timestamp":"2026-05-21T01:01:24.096Z","message":{"id":"msg_123","usage":{"input_tokens":6,"output_tokens":3,"cache_creation_input_tokens":10,"cache_read_input_tokens":20}}}"#;

        let event = parse_usage_line(path, 42, line).expect("usage event");

        assert_eq!(event.event_id, "/tmp/session.jsonl:msg_123");
        assert_eq!(event.tokens, 21);
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
    fn parses_nested_claude_oauth_tokens() {
        let raw = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": " access-token ",
                "refreshToken": " refresh-token ",
                "scopes": ["user:inference"]
            },
            "mcpOAuth": {
                "accessToken": "mcp-token"
            }
        })
        .to_string();

        let credentials = parse_stored_oauth_credentials(
            &raw,
            OAuthCredentialStorage::File {
                path: PathBuf::from("/tmp/credentials.json"),
            },
        )
        .expect("credentials");

        assert_eq!(credentials.access_token, "access-token");
        assert_eq!(credentials.refresh_token, Some("refresh-token".into()));
        assert_eq!(credentials.encoding, OAuthCredentialEncoding::Json);
    }

    #[test]
    fn parses_hex_encoded_claude_oauth_tokens() {
        let raw_json = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "access-token",
                "refreshToken": "refresh-token"
            }
        })
        .to_string();
        let raw_hex = encode_hex(raw_json.as_bytes());

        let credentials = parse_stored_oauth_credentials(
            &raw_hex,
            OAuthCredentialStorage::Keychain {
                account: Some("user".into()),
            },
        )
        .expect("credentials");

        assert_eq!(credentials.access_token, "access-token");
        assert_eq!(credentials.refresh_token, Some("refresh-token".into()));
        assert_eq!(credentials.encoding, OAuthCredentialEncoding::HexJson);
    }

    #[test]
    fn preserves_hex_encoding_when_saving_refreshed_tokens() {
        let value = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "new-access",
                "refreshToken": "new-refresh"
            }
        });

        let encoded =
            encode_oauth_credentials_for_storage(&value, OAuthCredentialEncoding::HexJson)
                .expect("encoded");
        let decoded = decode_hex_json(&encoded).expect("decoded");

        assert_eq!(
            serde_json::from_str::<Value>(&decoded).expect("json"),
            value
        );
    }

    #[test]
    fn updates_nested_claude_oauth_tokens_without_dropping_other_fields() {
        let mut value = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "old-access",
                "refreshToken": "old-refresh",
                "expiresAt": 1_700_000_000_000i64,
                "subscriptionType": "max"
            },
            "mcpOAuth": {
                "accessToken": "mcp-token"
            }
        });
        let refreshed = OAuthRefreshResponse {
            access_token: "new-access".into(),
            refresh_token: "new-refresh".into(),
            expires_in: Some(3600),
        };
        let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("timestamp");

        update_oauth_credentials_value(&mut value, &refreshed, now).expect("updated");

        let oauth = value.get("claudeAiOauth").expect("oauth");
        assert_eq!(
            oauth.get("accessToken").and_then(Value::as_str),
            Some("new-access")
        );
        assert_eq!(
            oauth.get("refreshToken").and_then(Value::as_str),
            Some("new-refresh")
        );
        assert_eq!(
            oauth.get("expiresAt").and_then(Value::as_i64),
            Some(1_700_003_600_000)
        );
        assert_eq!(
            oauth.get("subscriptionType").and_then(Value::as_str),
            Some("max")
        );
        assert_eq!(
            value
                .get("mcpOAuth")
                .and_then(|mcp| mcp.get("accessToken"))
                .and_then(Value::as_str),
            Some("mcp-token")
        );
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
        append_usage_line_at(
            path,
            output_tokens,
            DateTime::parse_from_rfc3339("2026-05-21T01:01:24.096Z")
                .unwrap()
                .with_timezone(&Utc),
        );
    }

    fn append_usage_line_at(path: &Path, output_tokens: u64, timestamp: DateTime<Utc>) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open log");
        writeln!(
            file,
            r#"{{"type":"assistant","timestamp":"{}","message":{{"usage":{{"output_tokens":{output_tokens}}}}}}}"#,
            timestamp.to_rfc3339()
        )
        .unwrap();
    }
}
