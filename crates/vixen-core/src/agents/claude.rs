//! Claude Code: `~/.claude/projects/<encoded cwd>/<session>.jsonl` (or `$CLAUDE_CONFIG_DIR`).

use super::{Parsed, clip, first_line, project, read_tail, ts};
use crate::model::{Agent, AgentState, Tool};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(crate) fn root() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| crate::config::home().join(".claude"))
}

/// Claude Code's state file, which caches the last `/usage` fetch.
pub fn state_file() -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir).join(".claude.json"),
        None => crate::config::home().join(".claude.json"),
    }
}

/// Top-level session transcripts (sub-agent transcripts live in subdirectories).
pub fn files() -> Vec<PathBuf> {
    let Ok(dirs) = std::fs::read_dir(root().join("projects")) else { return Vec::new() };
    dirs.flatten()
        .filter_map(|d| std::fs::read_dir(d.path()).ok())
        .flat_map(|it| it.flatten().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect()
}

pub fn parse(path: &Path) -> Option<Parsed> {
    let id = path.file_stem()?.to_string_lossy().into_owned();
    let started_at = first_line(path)
        .and_then(|l| serde_json::from_str::<Value>(&l).ok())
        .and_then(|v| v.get("timestamp").and_then(Value::as_str).and_then(ts));
    parse_tail(&id, &read_tail(path)?, started_at).map(|mut p| {
        p.agent.file = Some(path.display().to_string());
        p
    })
}

pub(crate) fn parse_tail(id: &str, tail: &str, started_at: Option<i64>) -> Option<Parsed> {
    let (mut title, mut prompt, mut reply) = (None, None, None);
    let (mut cwd, mut branch, mut source, mut model, mut context) = (None, None, None, None, None);
    let mut last_ts = None;
    let mut in_progress = None;

    for line in tail.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let str_of = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        match v.get("type").and_then(Value::as_str) {
            Some("ai-title") if title.is_none() => title = str_of("aiTitle"),
            Some("last-prompt") if prompt.is_none() => prompt = str_of("lastPrompt"),
            Some(t @ ("user" | "assistant")) => {
                if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                cwd = cwd.or_else(|| str_of("cwd"));
                branch = branch.or_else(|| str_of("gitBranch"));
                source = source.or_else(|| str_of("entrypoint"));
                last_ts = last_ts.or_else(|| v.get("timestamp").and_then(Value::as_str).and_then(ts));
                let msg = v.get("message");
                if t == "assistant" {
                    let m = msg.unwrap_or(&Value::Null);
                    model = model.or_else(|| m.get("model").and_then(Value::as_str).map(str::to_string));
                    if context.is_none()
                        && let Some(u) = m.get("usage")
                    {
                        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                        context =
                            Some(n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens"));
                    }
                    if reply.is_none() {
                        reply = m.get("content").and_then(Value::as_array).and_then(|parts| {
                            parts
                                .iter()
                                .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
                                .filter_map(|p| p.get("text").and_then(Value::as_str))
                                .next_back()
                                .map(|t| clip(t, 240))
                        });
                    }
                }
                // The most recent message decides whether a turn is still running.
                if in_progress.is_none() {
                    let stop = msg.and_then(|m| m.get("stop_reason")).and_then(Value::as_str);
                    in_progress = Some(!(t == "assistant" && matches!(stop, Some("end_turn" | "stop_sequence"))));
                }
            }
            _ => {}
        }
    }

    let in_progress = in_progress?; // metadata-only tail: not a usable session
    let cwd_s: Option<String> = cwd;
    let title =
        title.or_else(|| prompt.as_deref().map(|p| clip(p, 80))).unwrap_or_else(|| "Claude Code session".into());
    Some(Parsed {
        in_progress,
        agent: Agent {
            tool: Tool::ClaudeCode,
            id: id.to_string(),
            title,
            project: cwd_s.as_deref().and_then(project),
            resume: Some(format!("claude --resume {id}")),
            cwd: cwd_s,
            branch: branch.filter(|b| !b.is_empty() && b != "HEAD"),
            model,
            state: AgentState::Idle,
            started_at,
            updated_at: last_ts.unwrap_or(0),
            context_tokens: context,
            context_window: None,
            last_prompt: prompt.map(|p| clip(&p, 240)),
            last_reply: reply,
            source,
            file: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKING: &str = r#"{"type":"user","cwd":"/home/alice/proj","gitBranch":"main","entrypoint":"cli","timestamp":"2026-11-14T10:00:00Z","message":{"role":"user","content":"fix the bug"}}
{"type":"assistant","cwd":"/home/alice/proj","timestamp":"2026-11-14T10:00:05Z","message":{"role":"assistant","model":"claude-opus-5-5","stop_reason":"tool_use","usage":{"input_tokens":10,"cache_read_input_tokens":1000,"cache_creation_input_tokens":90,"output_tokens":20},"content":[{"type":"text","text":"Looking at it."},{"type":"tool_use","name":"Read"}]}}
{"type":"assistant","isSidechain":true,"timestamp":"2026-11-14T10:00:06Z","message":{"role":"assistant","stop_reason":"end_turn","content":[]}}
{"type":"ai-title","aiTitle":"Fix the parser bug","sessionId":"s1"}
{"type":"last-prompt","lastPrompt":"fix the bug","sessionId":"s1"}"#;

    #[test]
    fn working_session() {
        let p = parse_tail("s1", WORKING, Some(1)).unwrap();
        assert!(p.in_progress, "tool_use pending, sidechain ignored");
        let a = p.agent;
        assert_eq!(a.title, "Fix the parser bug");
        assert_eq!(a.project.as_deref(), Some("proj"));
        assert_eq!(a.branch.as_deref(), Some("main"));
        assert_eq!(a.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(a.context_tokens, Some(1100));
        assert_eq!(a.last_reply.as_deref(), Some("Looking at it."));
        assert_eq!(a.resume.as_deref(), Some("claude --resume s1"));
    }

    #[test]
    fn finished_turn_is_not_in_progress() {
        let done = r#"{"type":"assistant","cwd":"/p","timestamp":"2026-11-14T10:00:05Z","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"Done."}]}}"#;
        assert!(!parse_tail("s2", done, None).unwrap().in_progress);
    }

    #[test]
    fn metadata_only_is_skipped() {
        assert!(parse_tail("s3", r#"{"type":"ai-title","aiTitle":"x"}"#, None).is_none());
    }
}
