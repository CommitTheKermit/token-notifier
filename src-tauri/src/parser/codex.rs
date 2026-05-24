use super::{parse_epoch_like_timestamp, LocalLogParser, UsageEvent, UsageSource};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::PathBuf;

const PARSER_NAME: &str = "codex";
const INITIALIZED_KEY: &str = "__initialized";

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
mod tests {
    use super::*;
    use crate::parser::LocalLogParser;
    use rusqlite::{params, Connection};

    #[test]
    fn persisted_state_baselines_existing_codex_threads_once() {
        let dir = tempfile::tempdir().expect("temp dir");
        let codex_db = dir.path().join("state_5.sqlite");
        let app_db = dir.path().join("usage.sqlite");
        seed_thread(&codex_db, "thread-a", 100);

        let mut parser = CodexParser::from_db(codex_db.clone()).with_state_db_path(app_db);
        assert!(parser.read_delta().unwrap().is_empty());

        seed_thread(&codex_db, "thread-a", 130);
        let events = parser.read_delta().unwrap();
        assert_eq!(events.len(), 1);
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
}
