//! OpenCode: sessions and token use from `~/.local/share/opencode/opencode.db`, read with the
//! `sqlite3` CLI in read-only mode (no SQLite dependency; skipped if unavailable).
//!
//! OpenCode 2.x keeps sessions in `session_v2` (with busy/idle times) and their messages in
//! `session_message`; 1.x has only `session`, which gives title, folder and times. The queries
//! re-run only when the database (or its write-ahead log) changes.

use super::usage::TokenLog;
use super::{Parsed, clip, project};
use crate::exec;
use crate::model::{Agent, AgentState, Tool};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// Sessions listed at most.
const MAX_SESSIONS: usize = 50;

#[derive(Default)]
pub(crate) struct OpenCode {
    /// Modification times of the database and its WAL when last read, and the cutoff used.
    stamp: Option<(SystemTime, SystemTime, i64)>,
    sessions: Vec<(Agent, bool)>,
    /// Session id → token events since the start of the token window.
    pub tokens: HashMap<String, TokenLog>,
}

fn db() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| crate::config::home().join(".local/share"));
    data.join("opencode/opencode.db")
}

fn mtime(p: &std::path::Path) -> SystemTime {
    std::fs::metadata(p).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH)
}

impl OpenCode {
    /// Sessions updated since `cutoff` and token events since `tokens_since` (unix seconds).
    pub async fn refresh(&mut self, cutoff: i64, tokens_since: i64) -> Vec<Parsed> {
        let db = db();
        if !db.exists() {
            self.sessions.clear();
            self.tokens.clear();
            return Vec::new();
        }
        let wal = db.with_file_name("opencode.db-wal");
        let since = cutoff.min(tokens_since);
        // Re-query when the files change, or hourly so the window keeps moving.
        let stamp = (mtime(&db), mtime(&wal), since - since.rem_euclid(3600));
        if self.stamp != Some(stamp) {
            match query(&db, &sessions_sql(cutoff)).await {
                Some(rows) => self.sessions = rows.iter().filter_map(parse_session).collect(),
                None => {
                    // OpenCode 1.x: no session_v2 / session_message.
                    let rows = query(&db, &legacy_sql(cutoff)).await.unwrap_or_default();
                    self.sessions = rows.iter().filter_map(parse_session).collect();
                }
            }
            self.tokens = tokens(query(&db, &tokens_sql(tokens_since)).await.unwrap_or_default());
            self.stamp = Some(stamp);
        }
        self.sessions
            .iter()
            .filter(|(a, _)| a.updated_at >= cutoff)
            .map(|(a, busy)| Parsed { agent: a.clone(), in_progress: *busy })
            .collect()
    }
}

async fn query(db: &std::path::Path, sql: &str) -> Option<Vec<Value>> {
    let argv = ["sqlite3", "-readonly", "-json", &db.display().to_string(), sql].map(String::from);
    let out = exec::program(&argv, Duration::from_secs(5)).await.ok()?;
    if out.trim().is_empty() {
        return Some(Vec::new()); // sqlite3 prints nothing for an empty result
    }
    serde_json::from_str(&out).ok()
}

fn sessions_sql(cutoff: i64) -> String {
    let ms = cutoff * 1000;
    format!(
        "select s.id, s.title, s.directory, s.time_created, s.time_updated, s.time_busy, s.time_idle, s.agent, \
           json_extract(s.model, '$.id') as model, \
           (select json_extract(m.data, '$.text') from session_message m \
             where m.session_id = s.id and m.type = 'user' order by m.seq desc limit 1) as prompt, \
           (select json_extract(c.value, '$.text') from session_message m, json_each(m.data, '$.content') c \
             where m.session_id = s.id and m.type = 'assistant' and json_extract(c.value, '$.type') = 'text' \
             order by m.seq desc, c.key desc limit 1) as reply, \
           (select json_extract(m.data, '$.tokens') from session_message m \
             where m.session_id = s.id and m.type = 'assistant' order by m.seq desc limit 1) as tokens \
         from session_v2 s \
         where s.parent_id is null and s.time_archived is null and s.time_updated >= {ms} \
         order by s.time_updated desc limit {MAX_SESSIONS}"
    )
}

fn legacy_sql(cutoff: i64) -> String {
    let ms = cutoff * 1000;
    format!(
        "select id, title, directory, time_created, time_updated, agent, json_extract(model, '$.id') as model \
         from session where parent_id is null and time_archived is null and time_updated >= {ms} \
         order by time_updated desc limit {MAX_SESSIONS}"
    )
}

/// Every assistant message's token use since `since`, for the token totals and chart.
fn tokens_sql(since: i64) -> String {
    let ms = since * 1000;
    format!(
        "select id, session_id, time_created, json_extract(data, '$.tokens') as tokens from session_message \
         where type = 'assistant' and time_created >= {ms}"
    )
}

fn int(v: &Value, k: &str) -> Option<i64> {
    v.get(k).and_then(|x| x.as_i64().or_else(|| x.as_str()?.parse().ok()))
}

fn text<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).filter(|s| !s.trim().is_empty())
}

/// `tokens` as stored: a JSON object, which sqlite3 hands back as a string.
fn tokens_of(v: &Value) -> Option<Value> {
    match v.get("tokens")? {
        Value::String(s) => serde_json::from_str(s).ok(),
        o @ Value::Object(_) => Some(o.clone()),
        _ => None,
    }
}

fn n(t: &Value, path: &str) -> u64 {
    t.pointer(path).and_then(|x| x.as_u64().or_else(|| x.as_str()?.parse().ok())).unwrap_or(0)
}

pub(crate) fn parse_session(row: &Value) -> Option<(Agent, bool)> {
    let id = text(row, "id")?;
    let updated = int(row, "time_updated")? / 1000;
    // Busy since it last went idle (OpenCode 2.x records both).
    let busy = int(row, "time_busy").is_some_and(|b| int(row, "time_idle").is_none_or(|i| b > i));
    let prompt = text(row, "prompt").map(|p| clip(p, 240));
    let title = text(row, "title")
        .map(|t| clip(t, 80))
        .or_else(|| prompt.as_deref().map(|p| clip(p, 80)))
        .unwrap_or_else(|| "OpenCode session".into());
    let cwd = text(row, "directory").map(str::to_string);
    let context = tokens_of(row).map(|t| n(&t, "/input") + n(&t, "/cache/read") + n(&t, "/cache/write"));
    Some((
        Agent {
            tool: Tool::OpenCode,
            id: id.to_string(),
            title,
            project: cwd.as_deref().and_then(project),
            branch: None,
            model: text(row, "model").map(str::to_string),
            state: AgentState::Idle,
            started_at: int(row, "time_created").map(|t| t / 1000),
            updated_at: updated,
            context_tokens: context.filter(|&c| c > 0),
            context_window: None,
            last_prompt: prompt,
            last_reply: text(row, "reply").map(|r| clip(r, 240)),
            source: text(row, "agent").map(|a| format!("{a} agent")),
            file: None,
            resume: Some(format!("opencode --session {id}")),
            cwd,
        },
        busy,
    ))
}

fn tokens(rows: Vec<Value>) -> HashMap<String, TokenLog> {
    let mut out: HashMap<String, TokenLog> = HashMap::new();
    for r in &rows {
        let (Some(id), Some(session), Some(at), Some(t)) =
            (text(r, "id"), text(r, "session_id"), int(r, "time_created"), tokens_of(r))
        else {
            continue;
        };
        let total = n(&t, "/input") + n(&t, "/output") + n(&t, "/cache/read") + n(&t, "/cache/write");
        out.entry(session.to_string()).or_default().record(id, at / 1000, total);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_row() {
        let row: Value = serde_json::from_str(
            r#"{"id":"ses_1","title":"Training monitoring","directory":"/Users/a/repos/pytorch-world",
                "time_created":1794784000000,"time_updated":1794786366473,"time_busy":1794786400000,
                "time_idle":1794786366473,"agent":"build","model":"glm-5.3","prompt":"check the code\nand submit a pr",
                "reply":"PR created.","tokens":"{\"input\":1138,\"output\":429,\"cache\":{\"read\":125999,\"write\":0}}"}"#,
        )
        .unwrap();
        let (a, busy) = parse_session(&row).unwrap();
        assert!(busy);
        assert_eq!(a.tool, Tool::OpenCode);
        assert_eq!(a.title, "Training monitoring");
        assert_eq!(a.project.as_deref(), Some("pytorch-world"));
        assert_eq!(a.updated_at, 1794786366);
        assert_eq!(a.started_at, Some(1794784000));
        assert_eq!(a.context_tokens, Some(127137));
        assert_eq!(a.last_prompt.as_deref(), Some("check the code and submit a pr"));
        assert_eq!(a.resume.as_deref(), Some("opencode --session ses_1"));
        assert_eq!(a.source.as_deref(), Some("build agent"));
    }

    #[test]
    fn idle_and_untitled() {
        let row: Value = serde_json::from_str(
            r#"{"id":"ses_2","title":"","directory":"/x","time_created":1,"time_updated":5000,
                "time_busy":1000,"time_idle":4000,"prompt":"fix the loader"}"#,
        )
        .unwrap();
        let (a, busy) = parse_session(&row).unwrap();
        assert!(!busy);
        assert_eq!(a.title, "fix the loader");
        // 1.x rows have neither busy nor idle times.
        let legacy: Value =
            serde_json::from_str(r#"{"id":"s","title":"t","directory":"/x","time_updated":5000}"#).unwrap();
        assert!(!parse_session(&legacy).unwrap().1);
    }

    #[test]
    fn token_rows() {
        let rows: Vec<Value> = serde_json::from_str(
            r#"[{"id":"m1","session_id":"s1","time_created":3600000,"tokens":"{\"input\":10,\"output\":5,\"cache\":{\"read\":100,\"write\":1}}"},
                {"id":"m2","session_id":"s1","time_created":7200000,"tokens":"{\"input\":1,\"output\":1,\"cache\":{\"read\":0,\"write\":0}}"},
                {"id":"m3","session_id":"s2","time_created":7200000,"tokens":null}]"#,
        )
        .unwrap();
        let t = tokens(rows);
        assert_eq!(t.len(), 1);
        assert_eq!(t["s1"].since(0), 118);
        assert_eq!(t["s1"].since(7200), 2);
    }
}
