//! Codex: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` (or `$CODEX_HOME`).

use super::{Parsed, clip, first_line, project, read_tail, ts};
use crate::model::{Agent, AgentState, Tool};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| crate::config::home().join(".codex"))
}

/// Rollouts from the day directories that can overlap the window.
pub fn files(window_hours: u64) -> Vec<PathBuf> {
    let base = root().join("sessions");
    let today = chrono::Local::now().date_naive();
    let days = window_hours.div_ceil(24) as i64 + 1;
    (0..days)
        .filter_map(|d| today.checked_sub_signed(chrono::Duration::days(d)))
        .filter_map(|day| std::fs::read_dir(base.join(day.format("%Y/%m/%d").to_string())).ok())
        .flat_map(|it| it.flatten().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|e| e == "jsonl")
                && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("rollout-"))
        })
        .collect()
}

pub fn parse(path: &Path) -> Option<Parsed> {
    let head: Value = serde_json::from_str(&first_line(path)?).ok()?;
    let mut p = parse_parts(&head, &read_tail(path)?)?;
    p.agent.file = Some(path.display().to_string());
    Some(p)
}

pub(crate) fn parse_parts(head: &Value, tail: &str) -> Option<Parsed> {
    let meta = head.get("payload").filter(|_| head.get("type").and_then(Value::as_str) == Some("session_meta"))?;
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let id = s(meta, "id")?;
    let mut cwd = s(meta, "cwd");
    let source = s(meta, "originator");
    let started_at = head.get("timestamp").and_then(Value::as_str).and_then(ts);

    let (mut model, mut context, mut window, mut prompt, mut reply, mut last_ts) = (None, None, None, None, None, None);
    let mut in_progress = None;
    for line in tail.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        last_ts = last_ts.or_else(|| v.get("timestamp").and_then(Value::as_str).and_then(ts));
        let p = v.get("payload").unwrap_or(&Value::Null);
        match (v.get("type").and_then(Value::as_str), p.get("type").and_then(Value::as_str)) {
            (Some("turn_context"), _) => {
                model = model.or_else(|| s(p, "model"));
                cwd = cwd.or_else(|| s(p, "cwd"));
            }
            (Some("event_msg"), Some("token_count")) if context.is_none() => {
                let info = p.get("info").unwrap_or(&Value::Null);
                context = info.pointer("/last_token_usage/input_tokens").and_then(Value::as_u64);
                window = info.get("model_context_window").and_then(Value::as_u64);
            }
            (Some("event_msg"), Some("user_message")) if prompt.is_none() => prompt = s(p, "message"),
            (Some("event_msg"), Some("agent_message")) if reply.is_none() => reply = s(p, "message"),
            (Some("event_msg"), Some("task_complete")) => {
                in_progress = in_progress.or(Some(false));
                reply = reply.or_else(|| s(p, "last_agent_message"));
            }
            (Some("event_msg"), Some("task_started")) => in_progress = in_progress.or(Some(true)),
            _ => {}
        }
    }

    let title = prompt.as_deref().map(|t| clip(t, 80)).unwrap_or_else(|| "Codex session".into());
    Some(Parsed {
        in_progress: in_progress.unwrap_or(false),
        agent: Agent {
            tool: Tool::Codex,
            resume: Some(format!("codex resume {id}")),
            id,
            title,
            project: cwd.as_deref().and_then(project),
            cwd,
            branch: meta.pointer("/git/branch").and_then(Value::as_str).map(str::to_string),
            model,
            state: AgentState::Idle,
            started_at,
            updated_at: last_ts.or(started_at).unwrap_or(0),
            context_tokens: context,
            context_window: window,
            last_prompt: prompt.map(|t| clip(&t, 240)),
            last_reply: reply.map(|t| clip(&t, 240)),
            source,
            file: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = r#"{"timestamp":"2026-11-13T13:09:14Z","type":"session_meta","payload":{"id":"abc-123","cwd":"/home/alice/app","originator":"codex_cli_rs","cli_version":"0.169.0","git":{"branch":"feat/x"}}}"#;

    #[test]
    fn running_then_complete() {
        let head: Value = serde_json::from_str(HEAD).unwrap();
        let running = r#"{"timestamp":"2026-11-13T13:09:15Z","type":"event_msg","payload":{"type":"task_started"}}
{"timestamp":"2026-11-13T13:09:16Z","type":"turn_context","payload":{"model":"gpt-5.5-codex","cwd":"/home/alice/app"}}
{"timestamp":"2026-11-13T13:09:16Z","type":"event_msg","payload":{"type":"user_message","message":"add tests"}}
{"timestamp":"2026-11-13T13:09:20Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":5000},"model_context_window":272000}}}"#;
        let p = parse_parts(&head, running).unwrap();
        assert!(p.in_progress);
        assert_eq!(p.agent.title, "add tests");
        assert_eq!(p.agent.model.as_deref(), Some("gpt-5.5-codex"));
        assert_eq!((p.agent.context_tokens, p.agent.context_window), (Some(5000), Some(272000)));
        assert_eq!(p.agent.branch.as_deref(), Some("feat/x"));
        assert_eq!(p.agent.resume.as_deref(), Some("codex resume abc-123"));

        let done = format!(
            "{running}\n{}",
            r#"{"timestamp":"2026-11-13T13:17:44Z","type":"event_msg","payload":{"type":"task_complete","last_agent_message":"Added 4 tests."}}"#
        );
        let p = parse_parts(&head, &done).unwrap();
        assert!(!p.in_progress);
        assert_eq!(p.agent.last_reply.as_deref(), Some("Added 4 tests."));
    }
}
