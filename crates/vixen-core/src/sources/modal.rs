//! Modal apps via the `modal` CLI (`modal app list --json`).

use super::Poll;
use crate::config::Target;
use crate::exec;
use crate::model::{Action, Job, JobState};
use crate::timefmt::RemoteClock;
use anyhow::{Context, Result};
use serde_json::Value;
use std::time::Duration;

/// CLI argv: the configured command, else `modal`, else `uvx modal`.
pub fn command(t: &Target) -> Vec<Vec<String>> {
    match &t.command {
        Some(c) => vec![c.split_whitespace().map(String::from).collect()],
        None => vec![vec!["modal".into()], vec!["uvx".into(), "modal".into()]],
    }
}

/// Run a modal subcommand with the first CLI that can be started.
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
    Err(last.context("modal CLI not found")?.context("install it with `uv tool install modal`"))
}

pub async fn poll(t: &Target) -> Result<Poll> {
    let out = run(t, &["app", "list", "--json"], Duration::from_secs(40)).await?;
    let clock = RemoteClock::local();
    Ok(Poll { clock: Some(clock), jobs: parse(&out, &clock)?, ..Default::default() })
}

fn field<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(Value::as_str))
}

pub fn parse(json: &str, clock: &RemoteClock) -> Result<Vec<Job>> {
    let apps: Vec<Value> = serde_json::from_str(json.trim()).context("modal app list --json")?;
    let mut jobs = Vec::new();
    for app in &apps {
        let raw = field(app, &["state", "State"]).unwrap_or("").to_string();
        if raw.starts_with("stopp") {
            continue;
        }
        let Some(id) = field(app, &["app_id", "App ID"]) else { continue };
        let name = field(app, &["description", "Description", "name"]).unwrap_or(id);
        let tasks: u32 = match app.get("tasks").or(app.get("Tasks")) {
            Some(Value::String(s)) => s.parse().unwrap_or(0),
            Some(Value::Number(n)) => n.as_u64().unwrap_or(0) as u32,
            _ => 0,
        };
        let st = if tasks > 0 || raw.starts_with("ephemeral") || raw == "running" {
            JobState::Running
        } else if raw.starts_with("deploy") || raw.starts_with("init") {
            JobState::Other
        } else {
            JobState::Pending
        };
        let mut job = Job::new(id, name, st);
        job.raw_state = if st == JobState::Other && tasks == 0 { format!("{raw} · idle") } else { raw };
        job.resources = Some(format!("{tasks} container{}", if tasks == 1 { "" } else { "s" }));
        job.started_at = field(app, &["created_at", "Created at"]).and_then(|s| clock.timestamp(s));
        if st == JobState::Running {
            job.elapsed_s = job.started_at.map(|s| (clock.now - s).max(0) as u64);
        }
        job.actions = vec![Action::Logs, Action::Cancel];
        jobs.push(job);
    }
    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_apps() {
        let json = r#"[
          {"app_id":"ap-1","description":"__harbor__","state":"deployed","tasks":"0","created_at":"2026-11-08 03:29:49+00:00","stopped_at":null},
          {"app_id":"ap-2","description":"train","state":"ephemeral","tasks":"2","created_at":"2026-11-14 10:00:00+00:00","stopped_at":null},
          {"app_id":"ap-3","description":"old","state":"stopped","tasks":"0","created_at":"2026-11-01 10:00:00+00:00","stopped_at":"x"}
        ]"#;
        let clock = RemoteClock::parse("1794657600 +0000"); // 2026-11-14 12:00 UTC
        let jobs = parse(json, &clock).unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].state, JobState::Other);
        assert_eq!(jobs[1].state, JobState::Running);
        assert_eq!(jobs[1].elapsed_s, Some(7200));
        assert_eq!(jobs[1].resources.as_deref(), Some("2 containers"));
    }
}
