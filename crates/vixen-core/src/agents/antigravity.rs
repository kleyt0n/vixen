//! Antigravity: `~/.gemini/antigravity/conversation_summaries.db`, read with the
//! `sqlite3` CLI in read-only mode (no SQLite dependency; skipped if unavailable).

use super::{Parsed, clip, project};
use crate::exec;
use crate::model::{Agent, AgentState, Tool};
use std::time::Duration;

const SEP: char = '\u{1f}';

pub async fn sessions(cutoff: i64) -> Vec<Parsed> {
    let db = crate::config::home().join(".gemini/antigravity/conversation_summaries.db");
    if !db.exists() {
        return Vec::new();
    }
    let since = chrono::DateTime::from_timestamp(cutoff, 0).map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string());
    let Some(since) = since else { return Vec::new() };
    let sql = format!(
        "select conversation_id, title, preview, step_count, last_modified_time, workspace_uris, not_fully_idle \
         from conversation_summaries where killed = 0 and nesting_depth = 0 and last_modified_time >= '{since}' \
         order by last_modified_time desc limit 50"
    );
    let argv = ["sqlite3", "-readonly", "-noheader", "-separator", &SEP.to_string(), &db.display().to_string(), &sql]
        .map(String::from);
    let out = exec::program(&argv, Duration::from_secs(5)).await.unwrap_or_default();
    out.lines().filter_map(parse_row).collect()
}

pub(crate) fn parse_row(line: &str) -> Option<Parsed> {
    let f: Vec<&str> = line.split(SEP).collect();
    let [id, title, preview, steps, modified, workspaces, busy] = f.as_slice() else { return None };
    let cwd = workspace(workspaces);
    let title = if title.trim().is_empty() { clip(preview, 80) } else { clip(title, 80) };
    Some(Parsed {
        in_progress: *busy == "1",
        agent: Agent {
            tool: Tool::Antigravity,
            id: id.to_string(),
            title: if title.is_empty() { "Antigravity conversation".into() } else { title },
            project: cwd.as_deref().and_then(project),
            cwd,
            branch: None,
            model: None,
            state: AgentState::Idle,
            started_at: None,
            updated_at: timestamp(modified)?,
            context_tokens: None,
            context_window: None,
            last_prompt: None,
            last_reply: (!preview.trim().is_empty()).then(|| clip(preview, 240)),
            source: Some(format!("{steps} steps")),
            file: None,
            resume: None,
        },
    })
}

/// `2026-05-24 13:50:26.20394+00:00` → unix seconds.
fn timestamp(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M:%S%.f%:z").map(|d| d.timestamp()).ok().or_else(|| {
        chrono::NaiveDateTime::parse_from_str(s.get(..19)?, "%Y-%m-%d %H:%M:%S").ok().map(|d| d.and_utc().timestamp())
    })
}

/// First workspace folder from a JSON list or comma/newline-separated `file://` URIs.
fn workspace(raw: &str) -> Option<String> {
    let first = serde_json::from_str::<Vec<String>>(raw)
        .ok()
        .and_then(|v| v.into_iter().next())
        .or_else(|| raw.split([',', '\n']).map(str::trim).find(|s| !s.is_empty()).map(str::to_string))?;
    let path = first.strip_prefix("file://").unwrap_or(&first);
    Some(path.replace("%20", " ")).filter(|p| !p.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row() {
        let line = [
            "c1",
            "Refactor auth",
            "",
            "171",
            "2026-05-24 13:50:26.20394+00:00",
            r#"["file:///Users/a/My%20App"]"#,
            "1",
        ]
        .join(&SEP.to_string());
        let p = parse_row(&line).unwrap();
        assert!(p.in_progress);
        assert_eq!(p.agent.title, "Refactor auth");
        assert_eq!(p.agent.cwd.as_deref(), Some("/Users/a/My App"));
        assert_eq!(p.agent.project.as_deref(), Some("My App"));
        assert_eq!(p.agent.updated_at, 1779630626);
    }
}
