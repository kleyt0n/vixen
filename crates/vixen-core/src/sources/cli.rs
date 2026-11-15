//! Shared runner for CLI-backed targets (Modal, Vast.ai, SkyPilot).

use crate::config::{Kind, Target};
use crate::exec;
use anyhow::{Context, Result};
use serde_json::Value;
use std::time::Duration;

/// CLI argv candidates: the configured command, else the kind's defaults in order.
pub fn command(t: &Target) -> Vec<Vec<String>> {
    if let Some(c) = &t.command {
        return vec![c.split_whitespace().map(String::from).collect()];
    }
    let defaults: &[&[&str]] = match t.kind {
        Kind::Modal => &[&["modal"], &["uvx", "modal"]],
        Kind::Vast => &[&["vastai"], &["uvx", "vastai"]],
        Kind::Skypilot => &[&["sky"]],
        Kind::Host => &[],
    };
    defaults.iter().map(|argv| argv.iter().map(|s| s.to_string()).collect()).collect()
}

fn install_hint(kind: Kind) -> &'static str {
    match kind {
        Kind::Modal => "install it with `uv tool install modal`",
        Kind::Vast => "install it with `uv tool install vastai` and run `vastai set api-key …`",
        Kind::Skypilot => "install it with `uv tool install skypilot` (plus your cloud extras)",
        Kind::Host => "",
    }
}

/// argv for `args`, using the first candidate CLI (no probing; for streaming commands).
pub fn argv(t: &Target, args: &[&str]) -> Vec<String> {
    let mut argv = command(t).into_iter().next().unwrap_or_default();
    argv.extend(args.iter().map(|s| s.to_string()));
    argv
}

/// Run a subcommand with the first CLI that can be started.
pub async fn run(t: &Target, args: &[&str], timeout: Duration) -> Result<String> {
    let mut last = None;
    for base in command(t) {
        let argv: Vec<String> = base.into_iter().chain(args.iter().map(|s| s.to_string())).collect();
        match exec::program(&argv, timeout).await {
            Ok(out) => return Ok(out),
            Err(e) if e.to_string().starts_with("cannot start") => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    let name = command(t).first().and_then(|a| a.last().cloned()).unwrap_or_default();
    Err(last.context(format!("{name} CLI not found"))?.context(install_hint(t.kind)))
}

/// Parse JSON that may be preceded by CLI banners or update notices.
pub fn json(out: &str, what: &str) -> Result<Value> {
    find_json(out).with_context(|| format!("{what}: no JSON in output: {}", first_line(out)))
}

/// The first line-initial `[` or `{` that parses as JSON through the end of `out`.
/// Banners such as `[notice] …` are skipped because they don't parse.
pub fn find_json(out: &str) -> Option<Value> {
    let mut at = 0;
    for line in out.split_inclusive('\n') {
        let lead = line.len() - line.trim_start().len();
        if line.trim_start().starts_with(['[', '{'])
            && let Ok(v) = serde_json::from_str(out[at + lead..].trim_end())
        {
            return Some(v);
        }
        at += line.len();
    }
    None
}

fn first_line(s: &str) -> &str {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("(empty)")
}

/// Last `n` lines of `s`.
pub fn tail(s: &str, n: u32) -> String {
    let all: Vec<&str> = s.lines().collect();
    all[all.len().saturating_sub(n as usize)..].join("\n")
}

pub fn str_field<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(Value::as_str)).filter(|s| !s.is_empty())
}

/// A number given as a JSON number or a numeric string.
pub fn num_field(v: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| match v.get(*k)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}
