//! Recent activity in a session transcript, as a few short lines a model can read:
//! prompts, replies, tool calls (with the file or command), edits and errors.

use super::{clip, read_tail};
use crate::model::{Agent, Tool};
use serde_json::Value;
use std::path::Path;

/// Characters kept per line.
const LINE: usize = 160;

/// The last `max` events of the session, oldest first. Empty when the tool keeps no transcript.
pub fn recent(agent: &Agent, max: usize) -> Vec<String> {
    let Some(file) = agent.file.as_deref() else { return Vec::new() };
    let Some(tail) = read_tail(Path::new(file)) else { return Vec::new() };
    let mut lines = match agent.tool {
        Tool::ClaudeCode => claude(&tail),
        Tool::Codex => codex(&tail),
        Tool::Antigravity | Tool::OpenCode => Vec::new(),
    };
    lines.dedup();
    let skip = lines.len().saturating_sub(max);
    lines.split_off(skip)
}

/// Text a person typed (not tool output, injected context or slash-command plumbing).
fn human_text(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty() && !t.starts_with('<') && !t.starts_with("Caveat:")).then(|| clip(t, LINE))
}

/// A short description of a tool call from its input: the file, command, pattern or URL.
fn tool_line(name: &str, input: &Value) -> String {
    let s = |k: &str| input.get(k).and_then(Value::as_str);
    let file = |p: &str| Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(p.into());
    // A tool's own one-line description (Claude's Bash) reads better than the raw command.
    let what = s("description")
        .filter(|_| input.get("command").is_some())
        .or(s("command"))
        .map(str::to_string)
        .or_else(|| s("file_path").or(s("notebook_path")).or(s("path")).map(file))
        .or_else(|| s("pattern").or(s("query")).or(s("url")).or(s("description")).or(s("prompt")).map(str::to_string))
        .or_else(|| input.as_object()?.values().find_map(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    if what.is_empty() { format!("tool {name}") } else { clip(&format!("tool {name}: {what}"), LINE) }
}

fn claude(tail: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in tail.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let t = v.get("type").and_then(Value::as_str);
        if !matches!(t, Some("user" | "assistant"))
            || v.get("isSidechain").and_then(Value::as_bool) == Some(true)
            || v.get("isMeta").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let content = v.pointer("/message/content");
        if let Some(text) = content.and_then(Value::as_str) {
            if t == Some("user")
                && let Some(h) = human_text(text)
            {
                out.push(format!("you: {h}"));
            }
            continue;
        }
        for part in content.and_then(Value::as_array).into_iter().flatten() {
            let text = part.get("text").and_then(Value::as_str);
            match (t, part.get("type").and_then(Value::as_str)) {
                (Some("user"), Some("text")) => {
                    if let Some(h) = text.and_then(human_text) {
                        out.push(format!("you: {h}"));
                    }
                }
                (Some("user"), Some("tool_result")) if part.get("is_error").and_then(Value::as_bool) == Some(true) => {
                    let msg = match part.get("content") {
                        Some(Value::String(s)) => s.clone(),
                        Some(Value::Array(a)) => {
                            a.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(" ")
                        }
                        _ => String::new(),
                    };
                    out.push(clip(&format!("error: {msg}"), LINE));
                }
                (Some("assistant"), Some("text")) => {
                    if let Some(text) = text.filter(|s| !s.trim().is_empty()) {
                        out.push(format!("agent: {}", clip(text, LINE)));
                    }
                }
                (Some("assistant"), Some("tool_use")) => {
                    let name = part.get("name").and_then(Value::as_str).unwrap_or("tool");
                    out.push(tool_line(name, part.get("input").unwrap_or(&Value::Null)));
                }
                _ => {}
            }
        }
    }
    out
}

fn codex(tail: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in tail.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let p = v.get("payload").unwrap_or(&Value::Null);
        let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        match (v.get("type").and_then(Value::as_str), p.get("type").and_then(Value::as_str)) {
            (Some("event_msg"), Some("user_message")) => {
                if let Some(h) = s(p, "message").as_deref().and_then(human_text) {
                    out.push(format!("you: {h}"));
                }
            }
            (Some("event_msg"), Some("agent_message")) => {
                if let Some(m) = s(p, "message") {
                    out.push(format!("agent: {}", clip(&m, LINE)));
                }
            }
            (Some("event_msg"), Some("turn_aborted")) => {
                out.push(format!("turn stopped ({})", s(p, "reason").unwrap_or_else(|| "aborted".into())));
            }
            (Some("event_msg"), Some("item_completed")) => {
                let item = p.get("item").unwrap_or(&Value::Null);
                if let Some(l) = codex_item(item) {
                    out.push(l);
                }
            }
            (Some("response_item"), Some("function_call")) => {
                let name = s(p, "name").unwrap_or_else(|| "tool".into());
                let args =
                    s(p, "arguments").and_then(|a| serde_json::from_str::<Value>(&a).ok()).unwrap_or(Value::Null);
                let mut args = args;
                // Older rollouts: `shell` with `command: ["bash", "-lc", "…"]`.
                if let Some(cmd) = args.get("command").and_then(Value::as_array).and_then(|c| c.last()).cloned() {
                    args = serde_json::json!({ "command": cmd });
                }
                out.push(tool_line(&name, &args));
            }
            _ => {}
        }
    }
    out
}

/// Newer rollouts record each finished step as an `item_completed` event.
fn codex_item(item: &Value) -> Option<String> {
    let text_of = |v: &Value| {
        v.get("content")
            .and_then(Value::as_array)?
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .filter_map(human_text)
            .next_back()
    };
    match item.get("type").and_then(Value::as_str)? {
        "UserMessage" => text_of(item).map(|t| format!("you: {t}")),
        "AgentMessage" => text_of(item).map(|t| format!("agent: {t}")),
        "CommandExecution" => {
            let cmd = match item.get("command") {
                Some(Value::Array(a)) => a.last().and_then(Value::as_str).unwrap_or_default().to_string(),
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            let code = item.get("exit_code").and_then(|c| c.as_i64().or_else(|| c.as_str()?.parse().ok()));
            let mut l = format!("ran: {cmd}");
            if let Some(c) = code.filter(|c| *c != 0) {
                let out = item.get("aggregated_output").and_then(Value::as_str).unwrap_or_default();
                let last = out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default();
                l = format!("{} → exit {c}: {last}", clip(&l, 100));
            }
            Some(clip(&l, LINE))
        }
        "FileChange" => {
            let files: Vec<String> = item
                .get("changes")
                .and_then(Value::as_object)?
                .keys()
                .filter_map(|p| Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect();
            Some(clip(&format!("edited {}", files.join(", ")), LINE))
        }
        "Extension" => item.get("query").and_then(Value::as_str).map(|q| clip(&format!("web search: {q}"), LINE)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_timeline() {
        let tail = r#"{"type":"user","message":{"role":"user","content":"run the tests"}}
{"type":"user","isMeta":true,"message":{"role":"user","content":"<command-name>/clear</command-name>"}}
{"type":"assistant","message":{"content":[{"type":"text","text":"Running them."},{"type":"tool_use","name":"Bash","input":{"command":"cargo test","description":"Run tests"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","is_error":true,"content":"error[E0425]: cannot find value `x`"}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"/repo/src/lib.rs","old_string":"a"}}]}}
{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"sub-agent chatter"}]}}"#;
        assert_eq!(
            claude(tail),
            [
                "you: run the tests",
                "agent: Running them.",
                "tool Bash: Run tests",
                "error: error[E0425]: cannot find value `x`",
                "tool Edit: lib.rs",
            ]
        );
    }

    #[test]
    fn codex_timeline() {
        let tail = r#"{"type":"event_msg","payload":{"type":"user_message","message":"train the model"}}
{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","command":["/bin/zsh","-lc","python train.py"],"exit_code":"1","aggregated_output":"Traceback\nRuntimeError: CUDA out of memory\n"}}}
{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"FileChange","changes":{"/r/train.py":{"type":"update"}}}}}
{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"bash\",\"-lc\",\"nvidia-smi\"]}"}}
{"type":"event_msg","payload":{"type":"agent_message","message":"Reduced the batch size."}}"#;
        assert_eq!(
            codex(tail),
            [
                "you: train the model",
                "ran: python train.py → exit 1: RuntimeError: CUDA out of memory",
                "edited train.py",
                "tool shell: nvidia-smi",
                "agent: Reduced the batch size.",
            ]
        );
    }
}
