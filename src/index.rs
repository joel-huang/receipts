//! SQLite index of all sessions. Transcripts are copied into the database so they
//! survive the agents' own log cleanup (Claude Code prunes after 30 days by default).

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

use crate::model::{Message, SessionMeta};
use crate::sources;

/// Cap on tool output copied into the full-text index (the full text is still stored).
const FTS_TOOL_RESULT_CHARS: usize = 4000;

/// Raise this when the parsers change what they store. The next scan then parses every log again.
const PARSE_VERSION: i64 = 2;

pub fn data_dir() -> PathBuf {
    std::env::var_os("RECEIPTS_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::data_dir().map(|d| d.join("receipts")))
        .unwrap_or_else(|| PathBuf::from(".receipts"))
}

pub fn open(path: &Path) -> anyhow::Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;
         CREATE TABLE IF NOT EXISTS sessions (
             id TEXT PRIMARY KEY,
             source TEXT NOT NULL,
             path TEXT NOT NULL,
             mtime INTEGER NOT NULL,
             size INTEGER NOT NULL,
             project TEXT,
             title TEXT,
             started_at TEXT,
             updated_at TEXT,
             message_count INTEGER NOT NULL DEFAULT 0,
             model TEXT,
             git_branch TEXT,
             input_tokens INTEGER NOT NULL DEFAULT 0,
             output_tokens INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS sessions_updated ON sessions(updated_at DESC);
         CREATE TABLE IF NOT EXISTS messages (
             session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
             idx INTEGER NOT NULL,
             role TEXT NOT NULL,
             kind TEXT NOT NULL,
             text TEXT NOT NULL,
             tool_name TEXT,
             tool_input TEXT,
             is_error INTEGER NOT NULL DEFAULT 0,
             timestamp TEXT,
             PRIMARY KEY (session_id, idx)
         );
         CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
             session_id UNINDEXED, idx UNINDEXED, role UNINDEXED, text,
             tokenize = 'porter unicode61'
         );",
    )?;
    // An mtime of -1 never matches a file, so the next scan parses every log again. Logs that the
    // agents already deleted keep their old parse.
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < PARSE_VERSION {
        conn.execute("UPDATE sessions SET mtime = -1", [])?;
        conn.pragma_update(None, "user_version", PARSE_VERSION)?;
    }
    Ok(conn)
}

#[derive(Debug, Default, Serialize)]
pub struct IndexReport {
    pub scanned: usize,
    pub updated: usize,
    pub errors: usize,
}

/// Incrementally index every source: only files whose mtime/size changed are re-parsed.
pub fn reindex(conn: &mut Connection) -> anyhow::Result<IndexReport> {
    let mut report = IndexReport::default();
    for source in sources::all() {
        for path in source.discover() {
            report.scanned += 1;
            let Ok(md) = std::fs::metadata(&path) else { continue };
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64);
            let size = md.len() as i64;
            let path_str = path.to_string_lossy().to_string();

            let known: Option<(i64, i64)> = conn
                .query_row(
                    "SELECT mtime, size FROM sessions WHERE path = ?1",
                    [&path_str],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if known == Some((mtime, size)) {
                continue;
            }

            match source.parse(&path) {
                Ok(parsed) if !parsed.messages.is_empty() => {
                    store(conn, &parsed.meta, &parsed.messages, mtime, size)?;
                    report.updated += 1;
                }
                Ok(_) => {}
                Err(e) => {
                    report.errors += 1;
                    eprintln!("receipts: failed to parse {} log {}: {e}", source.name(), path.display());
                }
            }
        }
    }
    Ok(report)
}

fn store(conn: &mut Connection, meta: &SessionMeta, messages: &[Message], mtime: i64, size: i64) -> anyhow::Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM messages WHERE session_id = ?1", [&meta.id])?;
    tx.execute("DELETE FROM messages_fts WHERE session_id = ?1", [&meta.id])?;
    tx.execute(
        "INSERT OR REPLACE INTO sessions
           (id, source, path, mtime, size, project, title, started_at, updated_at,
            message_count, model, git_branch, input_tokens, output_tokens)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            meta.id, meta.source, meta.path, mtime, size, meta.project, meta.title,
            meta.started_at, meta.updated_at, meta.message_count, meta.model,
            meta.git_branch, meta.input_tokens, meta.output_tokens
        ],
    )?;
    {
        let mut ins = tx.prepare(
            "INSERT INTO messages (session_id, idx, role, kind, text, tool_name, tool_input, is_error, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        let mut fts = tx.prepare("INSERT INTO messages_fts (session_id, idx, role, text) VALUES (?1, ?2, ?3, ?4)")?;
        for (i, m) in messages.iter().enumerate() {
            let input = m.tool_input.as_ref().map(Value::to_string);
            ins.execute(params![
                meta.id, i as i64, m.role, m.kind, m.text, m.tool_name, input, m.is_error, m.timestamp
            ])?;
            let searchable = match m.kind.as_str() {
                "tool_use" => format!("{} {}", m.tool_name.as_deref().unwrap_or(""), input.as_deref().unwrap_or("")),
                "tool_result" => m.text.chars().take(FTS_TOOL_RESULT_CHARS).collect(),
                _ => m.text.clone(),
            };
            if !searchable.trim().is_empty() {
                fts.execute(params![meta.id, i as i64, m.role, searchable])?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

// ---- queries -------------------------------------------------------------

#[derive(Serialize)]
pub struct SessionRow {
    #[serde(flatten)]
    pub meta: SessionMeta,
    pub available: bool,
}

fn row_to_session(r: &rusqlite::Row) -> rusqlite::Result<SessionRow> {
    let path: String = r.get("path")?;
    Ok(SessionRow {
        available: Path::new(&path).exists(),
        meta: SessionMeta {
            id: r.get("id")?,
            source: r.get("source")?,
            path,
            project: r.get("project")?,
            title: r.get("title")?,
            started_at: r.get("started_at")?,
            updated_at: r.get("updated_at")?,
            message_count: r.get("message_count")?,
            model: r.get("model")?,
            git_branch: r.get("git_branch")?,
            input_tokens: r.get("input_tokens")?,
            output_tokens: r.get("output_tokens")?,
        },
    })
}

pub struct ListFilter<'a> {
    pub source: Option<&'a str>,
    pub project: Option<&'a str>,
    pub limit: i64,
    pub offset: i64,
}

pub fn list_sessions(conn: &Connection, f: &ListFilter) -> anyhow::Result<Vec<SessionRow>> {
    let mut stmt = conn.prepare(
        "SELECT * FROM sessions
         WHERE (?1 IS NULL OR source = ?1) AND (?2 IS NULL OR project = ?2)
         ORDER BY updated_at DESC LIMIT ?3 OFFSET ?4",
    )?;
    let rows = stmt
        .query_map(params![f.source, f.project, f.limit, f.offset], row_to_session)?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// Most characters of each kind that the chat view gets. Tool output makes up most of a large
/// chat, and its blocks start collapsed, so the view loads the rest when a block opens.
const PREVIEW_TOOL_RESULT: i64 = 2_000;
const PREVIEW_THINKING: i64 = 4_000;
const PREVIEW_TEXT: i64 = 50_000;
/// Longest string value inside a tool input in the chat view.
const PREVIEW_INPUT_STRING: usize = 500;

/// Part of a chat for the chat view: the messages from `offset` on, with long fields shortened.
#[derive(Serialize)]
pub struct SessionPage {
    pub session: SessionRow,
    /// Number of messages in the whole chat.
    pub total: i64,
    /// Position of the first message in `messages`.
    pub offset: i64,
    pub messages: Vec<Message>,
}

/// Messages from position `from` on, at most `limit` of them.
pub fn get_session(conn: &Connection, id: &str, from: i64, limit: i64) -> anyhow::Result<Option<SessionPage>> {
    let Some(session) = conn
        .query_row("SELECT * FROM sessions WHERE id = ?1", [id], row_to_session)
        .optional()?
    else {
        return Ok(None);
    };
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM messages WHERE session_id = ?1", [id], |r| r.get(0))?;
    let offset = from.clamp(0, total);
    let mut stmt = conn.prepare(
        "SELECT role, kind,
                substr(text, 1, CASE kind WHEN 'tool_result' THEN ?3 WHEN 'thinking' THEN ?4 ELSE ?5 END),
                length(text), tool_name, tool_input, is_error, timestamp
         FROM messages WHERE session_id = ?1 AND idx >= ?2 ORDER BY idx LIMIT ?6",
    )?;
    let messages = stmt
        .query_map(params![id, offset, PREVIEW_TOOL_RESULT, PREVIEW_THINKING, PREVIEW_TEXT, limit], |r| {
            let text: String = r.get(2)?;
            let full_len: i64 = r.get(3)?;
            let input: Option<String> = r.get(5)?;
            let mut input: Option<Value> = input.and_then(|s| serde_json::from_str(&s).ok());
            let input_cut = input.as_mut().is_some_and(|v| shorten_strings(v, PREVIEW_INPUT_STRING));
            Ok(Message {
                truncated: (text.chars().count() as i64) < full_len || input_cut,
                role: r.get(0)?,
                kind: r.get(1)?,
                text,
                tool_name: r.get(4)?,
                tool_input: input,
                is_error: r.get(6)?,
                timestamp: r.get(7)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(Some(SessionPage { session, total, offset, messages }))
}

/// Most characters of a user message in the outline. The navigation column and the timeline
/// tooltips show only the start of a prompt.
const OUTLINE_TEXT: i64 = 160;

/// Every message of a chat in brief, for the navigation column and the timeline, which need the
/// whole chat but not its content. Each item is [timestamp, role, kind, text], where text is the
/// start of a user message or an assistant reply, or the first character of a thinking block so
/// that the web app can tell empty ones apart. Other kinds get no text.
#[derive(Serialize)]
pub struct Outline {
    pub session: SessionRow,
    pub total: i64,
    pub offset: i64,
    pub items: Vec<(Option<String>, String, String, Option<String>)>,
}

pub fn get_outline(conn: &Connection, id: &str, from: i64) -> anyhow::Result<Option<Outline>> {
    let Some(session) = conn
        .query_row("SELECT * FROM sessions WHERE id = ?1", [id], row_to_session)
        .optional()?
    else {
        return Ok(None);
    };
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM messages WHERE session_id = ?1", [id], |r| r.get(0))?;
    let offset = from.clamp(0, total);
    let mut stmt = conn.prepare(
        "SELECT timestamp, role, kind,
                CASE WHEN kind = 'text' AND role IN ('user', 'assistant') THEN substr(text, 1, ?3)
                     WHEN kind = 'thinking' THEN substr(trim(text), 1, 1) END
         FROM messages WHERE session_id = ?1 AND idx >= ?2 ORDER BY idx",
    )?;
    let items = stmt
        .query_map(params![id, offset, OUTLINE_TEXT], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    Ok(Some(Outline { session, total, offset, items }))
}

/// Shortens every string inside a JSON value to `max` characters. Returns true if it cut any.
fn shorten_strings(v: &mut Value, max: usize) -> bool {
    match v {
        Value::String(s) if s.chars().count() > max => {
            *s = s.chars().take(max).collect();
            true
        }
        Value::Array(items) => items.iter_mut().fold(false, |cut, x| shorten_strings(x, max) | cut),
        Value::Object(map) => map.values_mut().fold(false, |cut, x| shorten_strings(x, max) | cut),
        _ => false,
    }
}

/// One message in full, for a block that the chat view got shortened.
pub fn get_message(conn: &Connection, id: &str, idx: i64) -> anyhow::Result<Option<Message>> {
    let msg = conn
        .query_row(
            "SELECT role, kind, text, tool_name, tool_input, is_error, timestamp
             FROM messages WHERE session_id = ?1 AND idx = ?2",
            params![id, idx],
            |r| {
                let input: Option<String> = r.get(4)?;
                Ok(Message {
                    role: r.get(0)?,
                    kind: r.get(1)?,
                    text: r.get(2)?,
                    tool_name: r.get(3)?,
                    tool_input: input.and_then(|s| serde_json::from_str(&s).ok()),
                    is_error: r.get(5)?,
                    timestamp: r.get(6)?,
                    truncated: false,
                })
            },
        )
        .optional()?;
    Ok(msg)
}

#[derive(Serialize)]
pub struct Facet {
    pub name: String,
    pub count: i64,
}

pub fn facets(conn: &Connection, column: &str) -> anyhow::Result<Vec<Facet>> {
    // `column` is only ever a hard-coded identifier from the server.
    let sql = format!(
        "SELECT {column}, COUNT(*) FROM sessions WHERE {column} IS NOT NULL
         GROUP BY {column} ORDER BY MAX(updated_at) DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], |r| Ok(Facet { name: r.get(0)?, count: r.get(1)? }))?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

#[derive(Serialize)]
pub struct SearchHit {
    pub session_id: String,
    pub idx: i64,
    pub role: String,
    /// Matches are wrapped in \u{2} … \u{3} so the client can highlight without trusting HTML.
    pub snippet: String,
    pub title: Option<String>,
    pub source: String,
    pub project: Option<String>,
    pub updated_at: Option<String>,
}

/// Turn free text into a safe FTS5 query: every term is quoted and ANDed; a
/// trailing `*` on a term is kept as a prefix match.
fn fts_query(q: &str) -> String {
    q.split_whitespace()
        .map(|t| {
            let (term, prefix) = match t.strip_suffix('*') {
                Some(stripped) if !stripped.is_empty() => (stripped, "*"),
                _ => (t, ""),
            };
            format!("\"{}\"{prefix}", term.replace('"', "\"\""))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn search(conn: &Connection, q: &str, source: Option<&str>, limit: i64) -> anyhow::Result<Vec<SearchHit>> {
    let query = fts_query(q);
    if query.is_empty() {
        return Ok(vec![]);
    }
    let mut stmt = conn.prepare(
        "SELECT f.session_id, f.idx, f.role,
                snippet(messages_fts, 3, char(2), char(3), '…', 24),
                s.title, s.source, s.project, s.updated_at
         FROM messages_fts f JOIN sessions s ON s.id = f.session_id
         WHERE messages_fts MATCH ?1 AND (?2 IS NULL OR s.source = ?2)
         ORDER BY rank LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![query, source, limit], |r| {
            Ok(SearchHit {
                session_id: r.get(0)?,
                idx: r.get(1)?,
                role: r.get(2)?,
                snippet: r.get(3)?,
                title: r.get(4)?,
                source: r.get(5)?,
                project: r.get(6)?,
                updated_at: r.get(7)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_query_quotes_terms() {
        assert_eq!(fts_query("foo bar*"), "\"foo\" \"bar\"*");
        assert_eq!(fts_query("a\"b OR"), "\"a\"\"b\" \"OR\"");
        assert_eq!(fts_query("  "), "");
    }
}
