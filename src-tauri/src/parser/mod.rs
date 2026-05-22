use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

pub mod claude_code;
pub mod codex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageSource {
    ClaudeCode,
    Codex,
}

impl UsageSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "cc",
            Self::Codex => "cx",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
        }
    }
}

impl fmt::Display for UsageSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowMetadata {
    pub window_start: DateTime<Utc>,
    pub reset_at: DateTime<Utc>,
    pub quota_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageEvent {
    pub source: UsageSource,
    pub event_id: String,
    pub occurred_at: DateTime<Utc>,
    pub tokens: u64,
    pub metadata: Option<WindowMetadata>,
}

pub trait LocalLogParser {
    fn read_delta(&mut self) -> anyhow::Result<Vec<UsageEvent>>;
}

fn parse_epoch_like_timestamp(value: i64) -> Option<DateTime<Utc>> {
    let millis = if value > 10_000_000_000_000 {
        value / 1_000_000
    } else if value > 10_000_000_000 {
        value
    } else {
        value * 1_000
    };
    DateTime::<Utc>::from_timestamp_millis(millis)
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    pub fn usage_event(source: UsageSource, at: DateTime<Utc>, tokens: u64) -> UsageEvent {
        UsageEvent {
            source,
            event_id: format!("{}-{tokens}", source.as_str()),
            occurred_at: at,
            tokens,
            metadata: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::claude_code::ClaudeCodeParser;
    use super::{LocalLogParser, UsageSource};
    use std::fs::OpenOptions;
    use std::io::Write;

    #[test]
    fn reads_incremental_with_offset() {
        let dir = tempfile::tempdir().expect("temp dir");
        let log = dir.path().join("session.jsonl");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .expect("open log");
        writeln!(
            file,
            r#"{{"type":"assistant","timestamp":"2026-05-21T01:01:24.096Z","message":{{"usage":{{"input_tokens":6,"output_tokens":3,"cache_creation_input_tokens":10,"cache_read_input_tokens":20}}}}}}"#
        )
        .unwrap();
        drop(file);

        let mut parser = ClaudeCodeParser::from_files(vec![log.clone()]);
        let first = parser.read_delta().expect("first read");
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].source, UsageSource::ClaudeCode);
        assert_eq!(first[0].tokens, 39);
        assert!(parser.read_delta().expect("second read").is_empty());

        let mut file = OpenOptions::new().append(true).open(&log).unwrap();
        writeln!(
            file,
            r#"{{"type":"assistant","timestamp":"2026-05-21T01:02:24.096Z","message":{{"usage":{{"input_tokens":1,"output_tokens":2}}}}}}"#
        )
        .unwrap();
        drop(file);

        let second = parser.read_delta().expect("third read");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].tokens, 3);
    }
}
