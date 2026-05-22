use super::{parse_epoch_like_timestamp, LocalLogParser, UsageEvent, UsageSource};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Default)]
pub struct CodexParser {
    db_path: Option<PathBuf>,
    last_tokens_by_thread: HashMap<String, u64>,
}

impl CodexParser {
    pub fn new() -> Self {
        Self {
            db_path: dirs::home_dir().map(|home| home.join(".codex").join("state_5.sqlite")),
            last_tokens_by_thread: HashMap::new(),
        }
    }

    pub fn from_db(path: PathBuf) -> Self {
        Self {
            db_path: Some(path),
            last_tokens_by_thread: HashMap::new(),
        }
    }
}

impl LocalLogParser for CodexParser {
    fn read_delta(&mut self) -> anyhow::Result<Vec<UsageEvent>> {
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

        let mut events = Vec::new();
        for row in rows {
            let (thread_id, updated_at, tokens_used) = row?;
            let previous = self
                .last_tokens_by_thread
                .insert(thread_id.clone(), tokens_used)
                .unwrap_or(0);
            let delta = tokens_used.saturating_sub(previous);
            if delta == 0 {
                continue;
            }
            let occurred_at = parse_epoch_like_timestamp(updated_at).unwrap_or_else(chrono::Utc::now);
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
