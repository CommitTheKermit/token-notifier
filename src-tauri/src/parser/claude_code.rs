use super::{LocalLogParser, UsageEvent, UsageSource};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct ClaudeCodeParser {
    roots: Vec<PathBuf>,
    explicit_files: Vec<PathBuf>,
    offsets: HashMap<PathBuf, u64>,
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
        }
    }

    pub fn from_files(files: Vec<PathBuf>) -> Self {
        Self {
            roots: Vec::new(),
            explicit_files: files,
            offsets: HashMap::new(),
        }
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
        let mut line_no = 0usize;

        while reader.read_line(&mut line)? != 0 {
            line_no += 1;
            if let Some(event) = parse_usage_line(path, line_no, line.trim_end()) {
                events.push(event);
            }
            line.clear();
        }

        let new_offset = reader.stream_position()?;
        self.offsets.insert(path.to_path_buf(), new_offset);
        Ok(events)
    }
}

impl LocalLogParser for ClaudeCodeParser {
    fn read_delta(&mut self) -> anyhow::Result<Vec<UsageEvent>> {
        let mut events = Vec::new();
        for path in self.discover_files() {
            if path.is_file() {
                events.extend(self.read_file_delta(&path)?);
            }
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

fn parse_usage_line(path: &Path, line_no: usize, line: &str) -> Option<UsageEvent> {
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
        event_id: format!("{}:{line_no}", path.display()),
        occurred_at,
        tokens,
        metadata: None,
    })
}
