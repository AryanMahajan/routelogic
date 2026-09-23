//! Request history — the private tier.
//!
//! History rows quote request and response bodies, so a resolved bearer token would sit in
//! plain text in the database if nothing stopped it. Nothing does stop it by convention, so
//! the *type system* does: [`History::record`] accepts only a [`RedactedEntry`], and the only
//! way to obtain one is [`NewEntry::redacted`].
//!
//! This crate deliberately does not depend on `rl-http`. Entries carry opaque JSON, so the
//! storage layer never learns the shape of an HTTP exchange and the dependency graph stays
//! one-directional.

use crate::error::{Result, WorkspaceError};
use rl_model::VariableContext;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Bumped when the schema changes, and checked on open.
const SCHEMA_VERSION: i32 = 2;

/// A history row about to be written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewEntry {
    pub method: String,
    pub url: String,
    /// Absent when the request never got a response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Set instead of `status` when the request failed outright.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What was actually sent, as opaque JSON.
    pub request: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<serde_json::Value>,
    /// Who sent it, when it was not the person at the keyboard: `agent` for a request an
    /// agent made over MCP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl NewEntry {
    pub fn new(method: impl Into<String>, url: impl Into<String>) -> Self {
        NewEntry {
            method: method.into(),
            url: url.into(),
            status: None,
            duration_ms: None,
            error: None,
            request: serde_json::Value::Null,
            response: None,
            source: None,
        }
    }

    /// Strip every known secret value from this entry.
    ///
    /// Uses [`VariableContext::redact_all`] rather than only the secrets the resolver
    /// reported using, because a response can echo a token back — a `/login` handler
    /// returning the credential it was given, an error message quoting the header. Those
    /// never passed through resolution and would otherwise land in the database untouched.
    pub fn redacted(mut self, ctx: &VariableContext) -> RedactedEntry {
        self.url = ctx.redact_all(&self.url);
        self.error = self.error.map(|e| ctx.redact_all(&e));
        self.request = redact_json(self.request, ctx);
        self.response = self.response.map(|r| redact_json(r, ctx));
        RedactedEntry(self)
    }
}

/// A [`NewEntry`] that has been through redaction.
///
/// The only way to construct one is [`NewEntry::redacted`], which is what makes the
/// guarantee hold: a caller cannot forget.
#[derive(Debug, Clone, PartialEq)]
pub struct RedactedEntry(NewEntry);

impl RedactedEntry {
    pub fn inner(&self) -> &NewEntry {
        &self.0
    }
}

fn redact_json(value: serde_json::Value, ctx: &VariableContext) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => serde_json::Value::String(ctx.redact_all(&s)),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(|v| redact_json(v, ctx)).collect())
        }
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(k, v)| (ctx.redact_all(&k), redact_json(v, ctx)))
                .collect(),
        ),
        other => other,
    }
}

/// A row read back out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: i64,
    /// Unix seconds.
    pub at: i64,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub duration_ms: Option<u64>,
    pub error: Option<String>,
    pub request: serde_json::Value,
    pub response: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl HistoryEntry {
    pub fn succeeded(&self) -> bool {
        matches!(self.status, Some(s) if (200..400).contains(&s))
    }
}

/// The history database.
#[derive(Debug)]
pub struct History {
    conn: Connection,
}

impl History {
    /// Open, creating the file and schema if needed.
    pub fn open(path: impl AsRef<Path>) -> Result<History> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| WorkspaceError::io(format!("creating {}", parent.display()), e))?;
        }

        let conn = Connection::open(path).map_err(|source| WorkspaceError::Database { source })?;
        // The app and an agent's MCP server may both be writing. Write-ahead logging lets a
        // reader and a writer overlap, and the timeout makes a brief lock a wait rather than
        // a failed insert.
        let _ = conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()));
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|source| WorkspaceError::Database { source })?;
        let history = History { conn };
        history.migrate()?;
        Ok(history)
    }

    /// An in-memory database, for tests.
    pub fn in_memory() -> Result<History> {
        let conn =
            Connection::open_in_memory().map_err(|source| WorkspaceError::Database { source })?;
        let history = History { conn };
        history.migrate()?;
        Ok(history)
    }

    fn migrate(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS history (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    at          INTEGER NOT NULL,
                    method      TEXT    NOT NULL,
                    url         TEXT    NOT NULL,
                    status      INTEGER,
                    duration_ms INTEGER,
                    error       TEXT,
                    request     TEXT    NOT NULL,
                    response    TEXT,
                    source      TEXT
                 );
                 CREATE INDEX IF NOT EXISTS history_at ON history (at DESC);",
            )
            .map_err(|source| WorkspaceError::Database { source })?;

        // Version 1 files predate `source`.
        if self
            .conn
            .prepare("SELECT source FROM history LIMIT 0")
            .is_err()
        {
            self.conn
                .execute("ALTER TABLE history ADD COLUMN source TEXT", [])
                .map_err(|source| WorkspaceError::Database { source })?;
        }

        self.conn
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(|source| WorkspaceError::Database { source })?;

        Ok(())
    }

    /// Write an entry, returning its id.
    pub fn record(&self, entry: &RedactedEntry) -> Result<i64> {
        let e = entry.inner();
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let request = serde_json::to_string(&e.request)
            .map_err(|source| WorkspaceError::Secrets { source })?;
        let response = e
            .response
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|source| WorkspaceError::Secrets { source })?;

        self.conn
            .execute(
                "INSERT INTO history (at, method, url, status, duration_ms, error, request, response, source)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    at,
                    e.method,
                    e.url,
                    e.status,
                    // SQLite has no unsigned integer type; durations are far inside i64.
                    e.duration_ms.map(|d| d as i64),
                    e.error,
                    request,
                    response,
                    e.source
                ],
            )
            .map_err(|source| WorkspaceError::Database { source })?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Most recent first.
    pub fn recent(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, at, method, url, status, duration_ms, error, request, response, source
                 FROM history ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|source| WorkspaceError::Database { source })?;

        let rows = stmt
            .query_map([limit as i64], row_to_entry)
            .map_err(|source| WorkspaceError::Database { source })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|source| WorkspaceError::Database { source })
    }

    pub fn get(&self, id: i64) -> Result<Option<HistoryEntry>> {
        self.conn
            .query_row(
                "SELECT id, at, method, url, status, duration_ms, error, request, response, source
                 FROM history WHERE id = ?1",
                [id],
                row_to_entry,
            )
            .optional()
            .map_err(|source| WorkspaceError::Database { source })
    }

    pub fn count(&self) -> Result<usize> {
        self.conn
            .query_row("SELECT COUNT(*) FROM history", [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|n| n as usize)
            .map_err(|source| WorkspaceError::Database { source })
    }

    pub fn clear(&self) -> Result<()> {
        self.conn
            .execute("DELETE FROM history", [])
            .map(|_| ())
            .map_err(|source| WorkspaceError::Database { source })
    }

    /// Keep the newest `keep` entries, discarding the rest. Returns how many went.
    pub fn prune(&self, keep: usize) -> Result<usize> {
        let removed = self
            .conn
            .execute(
                "DELETE FROM history WHERE id NOT IN
                 (SELECT id FROM history ORDER BY id DESC LIMIT ?1)",
                [keep as i64],
            )
            .map_err(|source| WorkspaceError::Database { source })?;
        Ok(removed)
    }
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    let request: String = row.get(7)?;
    let response: Option<String> = row.get(8)?;

    Ok(HistoryEntry {
        id: row.get(0)?,
        at: row.get(1)?,
        method: row.get(2)?,
        url: row.get(3)?,
        status: row.get(4)?,
        duration_ms: row.get::<_, Option<i64>>(5)?.map(|d| d as u64),
        error: row.get(6)?,
        request: serde_json::from_str(&request).unwrap_or(serde_json::Value::Null),
        response: response.and_then(|r| serde_json::from_str(&r).ok()),
        source: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn ctx() -> VariableContext {
        let mut secrets = BTreeMap::new();
        secrets.insert("api_token".to_string(), "s3cr3t-value".to_string());
        VariableContext::new().with_secrets(secrets)
    }

    /// A database written before `source` existed opens, gains the column, and keeps
    /// its rows.
    #[test]
    fn a_version_1_database_is_migrated_in_place() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("history.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE history (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL,
                    method TEXT NOT NULL, url TEXT NOT NULL, status INTEGER,
                    duration_ms INTEGER, error TEXT, request TEXT NOT NULL, response TEXT);
                 INSERT INTO history (at, method, url, request) VALUES (1, 'GET', '/old', '{}');",
            )
            .unwrap();
        }
        let history = History::open(&path).unwrap();
        let mut agent = NewEntry::new("POST", "/new");
        agent.source = Some("agent".into());
        history.record(&agent.redacted(&ctx())).unwrap();

        let rows = history.recent(10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].source.as_deref(), Some("agent"));
        assert_eq!(
            (rows[1].url.as_str(), rows[1].source.as_deref()),
            ("/old", None)
        );
    }

    fn entry() -> NewEntry {
        let mut e = NewEntry::new("GET", "https://api.example.com/users");
        e.status = Some(200);
        e.duration_ms = Some(42);
        e.request = serde_json::json!({
            "headers": [["Authorization", "Bearer s3cr3t-value"]],
        });
        e.response = Some(serde_json::json!({"body": "{\"ok\":true}"}));
        e
    }

    #[test]
    fn entries_round_trip() {
        let history = History::in_memory().unwrap();
        let id = history.record(&entry().redacted(&ctx())).unwrap();

        let stored = history.get(id).unwrap().unwrap();
        assert_eq!(stored.method, "GET");
        assert_eq!(stored.status, Some(200));
        assert_eq!(stored.duration_ms, Some(42));
        assert!(stored.succeeded());
    }

    #[test]
    fn recent_returns_newest_first() {
        let history = History::in_memory().unwrap();
        for i in 0..5 {
            let mut e = NewEntry::new("GET", format!("https://x.test/{i}"));
            e.status = Some(200);
            history.record(&e.redacted(&ctx())).unwrap();
        }

        let recent = history.recent(3).unwrap();
        assert_eq!(recent.len(), 3);
        assert!(recent[0].url.ends_with("/4"));
        assert!(recent[2].url.ends_with("/2"));
    }

    /// The guarantee this module exists to make.
    #[test]
    fn a_resolved_secret_never_reaches_the_database() {
        let history = History::in_memory().unwrap();
        let id = history.record(&entry().redacted(&ctx())).unwrap();

        let stored = history.get(id).unwrap().unwrap();
        let text = serde_json::to_string(&stored).unwrap();

        assert!(!text.contains("s3cr3t-value"), "secret leaked into history");
        assert!(text.contains(rl_model::REDACTION));
    }

    #[test]
    fn a_secret_echoed_back_by_the_server_is_also_redacted() {
        let history = History::in_memory().unwrap();

        // A /login handler that returns the token it was given. This value never passed
        // through variable resolution, so only a full sweep catches it.
        let mut e = NewEntry::new("POST", "https://api.example.com/login");
        e.status = Some(200);
        e.request = serde_json::Value::Null;
        e.response = Some(serde_json::json!({"access_token": "s3cr3t-value"}));

        let id = history.record(&e.redacted(&ctx())).unwrap();
        let stored = history.get(id).unwrap().unwrap();

        assert!(!serde_json::to_string(&stored)
            .unwrap()
            .contains("s3cr3t-value"));
    }

    #[test]
    fn a_secret_in_the_url_or_an_error_message_is_redacted() {
        let history = History::in_memory().unwrap();

        let mut e = NewEntry::new("GET", "https://api.example.com/x?token=s3cr3t-value");
        e.error = Some("connection failed sending Bearer s3cr3t-value".into());
        e.request = serde_json::Value::Null;

        let id = history.record(&e.redacted(&ctx())).unwrap();
        let stored = history.get(id).unwrap().unwrap();

        assert!(!stored.url.contains("s3cr3t-value"));
        assert!(!stored.error.unwrap().contains("s3cr3t-value"));
    }

    #[test]
    fn a_failed_request_is_recorded_without_a_status() {
        let history = History::in_memory().unwrap();
        let mut e = NewEntry::new("GET", "http://127.0.0.1:1/");
        e.error = Some("connection refused".into());
        e.request = serde_json::Value::Null;

        let id = history.record(&e.redacted(&ctx())).unwrap();
        let stored = history.get(id).unwrap().unwrap();

        assert_eq!(stored.status, None);
        assert!(!stored.succeeded());
        assert_eq!(stored.error.as_deref(), Some("connection refused"));
    }

    #[test]
    fn pruning_keeps_the_newest() {
        let history = History::in_memory().unwrap();
        for i in 0..10 {
            let mut e = NewEntry::new("GET", format!("https://x.test/{i}"));
            e.status = Some(200);
            history.record(&e.redacted(&ctx())).unwrap();
        }

        assert_eq!(history.prune(4).unwrap(), 6);
        assert_eq!(history.count().unwrap(), 4);
        assert!(history.recent(1).unwrap()[0].url.ends_with("/9"));
    }

    #[test]
    fn clearing_empties_the_database() {
        let history = History::in_memory().unwrap();
        history.record(&entry().redacted(&ctx())).unwrap();
        history.clear().unwrap();
        assert_eq!(history.count().unwrap(), 0);
    }

    #[test]
    fn a_missing_id_reads_as_none() {
        let history = History::in_memory().unwrap();
        assert!(history.get(999).unwrap().is_none());
    }

    #[test]
    fn opening_a_file_creates_its_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("local").join("history.sqlite");

        let history = History::open(&path).unwrap();
        history.record(&entry().redacted(&ctx())).unwrap();

        assert!(path.is_file());

        // Reopening keeps the data.
        drop(history);
        let reopened = History::open(&path).unwrap();
        assert_eq!(reopened.count().unwrap(), 1);
    }
}
