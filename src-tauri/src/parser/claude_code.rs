use super::{LocalLogParser, UsageEvent, UsageSource};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const PARSER_NAME: &str = "claude_code";
const INITIALIZED_KEY: &str = "__initialized";

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::LocalLogParser;
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
