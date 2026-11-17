//! Insights: a model served by Ollama reads what's running and writes a short summary.
//!
//! Each round turns the latest [`Snapshot`] into a compact digest: jobs with progress, ETA
//! and the tail of their logs, and agent sessions with what they did recently, read from
//! their transcripts. The model answers with a headline and one note per item, in a fixed
//! JSON shape. Rounds are skipped while nothing changes, so an idle machine costs nothing.
//!
//! Nothing leaves this machine unless `[insights] url` points elsewhere.

use crate::agents::{activity, clip};
use crate::config::{Config, InsightsConfig, Kind};
use crate::model::{Agent, AgentState, Insights, Job, JobState, Note, NoteKind, Snapshot, TargetStatus, Tone, Tool};
use crate::monitor::now;
use crate::timefmt::human;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Duration;

/// Items described to the model, at most.
const MAX_AGENTS: usize = 8;
const MAX_JOBS: usize = 10;
/// Transcript events per agent and log lines per job.
const AGENT_EVENTS: usize = 12;
const LOG_LINES: u32 = 12;
/// Agents idle longer than this are left out.
const AGENT_RECENT_S: i64 = 2 * 3600;
/// First answers can include loading the model from disk.
const GENERATE_TIMEOUT: Duration = Duration::from_secs(240);

const SYSTEM: &str = "You are Vixen, a monitoring assistant for an engineer who runs GPU training jobs \
and AI coding agents (Claude Code, Codex, Antigravity, OpenCode). You read a status digest and write short, clean, concrete insights.

Write:
- headline: one sentence (max 20 words) with the most important thing right now.
- notes: one note per [j…] or [a…] item in the digest, using its reference as `ref`. Use ref \"general\" \
only for a real machine problem (unreachable, GPUs idle while jobs queue), at most two.

Each note is one sentence, max 18 words, starting with a verb or the fact (not \"The agent\"), \
about substance, not status:
- agent: what it did or is doing, from its recent activity and task (files, commands, PRs, test results, \
errors), then what it needs from the user if it is waiting. Never write only \"waiting for user input\".
- job: progress, speed and ETA, plus anything odd in the log (errors, NaN or rising loss, OOM, silent log, \
will exceed its time limit).

tone: \"bad\" = failed, erroring, crashed, will miss its time limit. \"warn\" = stalled, slow, silent log, \
waiting on the user for over 15 minutes, close to a limit. \"ok\" = otherwise.

Use only facts in the digest; never invent numbers. No greetings, markdown or emoji.

Example notes:
{\"ref\":\"a1\",\"text\":\"Pushed three review fixes to PR #1012; CI green, ready for your review.\",\"tone\":\"ok\"}
{\"ref\":\"a2\",\"text\":\"Tests fail in quiet-org-rebuilds.test.ts after the merge; it is investigating a time-of-day bug.\",\"tone\":\"warn\"}
{\"ref\":\"j1\",\"text\":\"Epoch 136/310 at 2.1 it/s, loss steady at 0.31; done around 22:50.\",\"tone\":\"ok\"}";

/// What the model reads, and how its item references map back to rows in the UI.
pub struct Digest {
    pub text: String,
    subjects: HashMap<String, Subject>,
}

#[derive(Clone)]
struct Subject {
    kind: NoteKind,
    key: String,
    label: String,
    /// The least severe tone the facts allow (models sometimes call a failure "ok").
    floor: Tone,
}

/// Waiting on the user longer than this is at least worth a look.
const LONG_WAIT_S: i64 = 15 * 60;
/// A running job whose log hasn't been written for this long may be stuck.
const SILENT_LOG_S: u64 = 10 * 60;

fn severity(t: Tone) -> u8 {
    match t {
        Tone::Ok => 0,
        Tone::Warn => 1,
        Tone::Bad => 2,
    }
}

fn tool_id(t: Tool) -> &'static str {
    match t {
        Tool::ClaudeCode => "claude_code",
        Tool::Codex => "codex",
        Tool::Antigravity => "antigravity",
        Tool::OpenCode => "opencode",
    }
}

/// Agents worth describing: working or waiting, recently active, working first.
fn active_agents(snap: &Snapshot, now: i64) -> impl Iterator<Item = &Agent> {
    snap.agents
        .iter()
        .filter(move |a| a.state != AgentState::Idle && now - a.updated_at < AGENT_RECENT_S)
        .take(MAX_AGENTS)
}

/// Jobs worth describing: running, finishing, suspended or failed.
fn notable_jobs(snap: &Snapshot) -> impl Iterator<Item = (&TargetStatus, &Job)> {
    snap.targets
        .iter()
        .flat_map(|t| t.jobs.iter().map(move |j| (t, j)))
        .filter(|(_, j)| {
            matches!(j.state, JobState::Running | JobState::Completing | JobState::Suspended | JobState::Failed)
        })
        .take(MAX_JOBS)
}

/// Nothing to say: no jobs (running or queued), no active agents, every machine reachable.
pub fn is_quiet(snap: &Snapshot) -> bool {
    let now = now();
    active_agents(snap, now).next().is_none()
        && notable_jobs(snap).next().is_none()
        && snap.targets.iter().all(|t| t.jobs.is_empty())
        && snap.targets.iter().all(|t| t.ok || t.updated_at == 0 && t.error.is_none())
}

/// Changes when something a summary would mention changes (not when only clocks advance).
pub fn fingerprint(snap: &Snapshot) -> u64 {
    let mut h = DefaultHasher::new();
    let now = now();
    // Facts that change with time alone are bucketed, so a stall or a long wait still
    // triggers a fresh summary once it crosses the line.
    for a in active_agents(snap, now) {
        let long_wait = a.state == AgentState::Waiting && now - a.updated_at > LONG_WAIT_S;
        (a.tool, &a.id, a.state.label(), a.updated_at, long_wait).hash(&mut h);
    }
    for (t, j) in notable_jobs(snap) {
        let silent = j.log_age_s.is_some_and(|s| s > SILENT_LOG_S);
        let late = j.eta.as_ref().is_some_and(|e| e.exceeds_limit);
        (&t.name, &j.id, j.state, j.progress.as_ref().map(|p| &p.label), &j.last_line, silent, late).hash(&mut h);
    }
    for t in &snap.targets {
        let queued = t.jobs.iter().filter(|j| matches!(j.state, JobState::Pending | JobState::Held)).count();
        (&t.name, t.ok, queued).hash(&mut h);
    }
    h.finish()
}

/// Build the digest for `snap`. Reads agent transcripts and, when enabled, the last lines
/// of running jobs' logs (one SSH round trip per job, run concurrently).
pub async fn digest(snap: &Snapshot, cfg: &Config) -> Digest {
    let now = now();
    let local = |ts: i64| {
        chrono::DateTime::from_timestamp(ts, 0)
            .map(|d| d.with_timezone(&chrono::Local).format("%a %H:%M").to_string())
            .unwrap_or_default()
    };
    let mut subjects = HashMap::new();
    let mut out = String::new();
    let _ = writeln!(out, "Now: {}\n", local(now));

    // Machines.
    if !snap.targets.is_empty() {
        out.push_str("MACHINES\n");
        for t in &snap.targets {
            let mut parts = vec![t.kind.clone()];
            if !t.ok && t.error.is_some() {
                parts.push(format!("UNREACHABLE: {}", clip(t.error.as_deref().unwrap_or(""), 120)));
            }
            if !t.gpus.is_empty() {
                let utils: Vec<f32> = t.gpus.iter().filter_map(|g| g.util).collect();
                let avg = if utils.is_empty() { 0.0 } else { utils.iter().sum::<f32>() / utils.len() as f32 };
                let idle = utils.iter().filter(|u| **u < 10.0).count();
                parts.push(format!("{} GPUs, avg util {avg:.0}%, {idle} idle", t.gpus.len()));
                if let Some(hot) = t.gpus.iter().filter_map(|g| g.temp_c).reduce(f32::max).filter(|c| *c >= 85.0) {
                    parts.push(format!("hottest GPU {hot:.0}°C"));
                }
            }
            let pending = t.jobs.iter().filter(|j| j.state == JobState::Pending).count();
            let held = t.jobs.iter().filter(|j| j.state == JobState::Held).count();
            if pending + held > 0 {
                parts.push(format!("{pending} pending, {held} held"));
            }
            let _ = writeln!(out, "- {}: {}", t.name, parts.join("; "));
        }
        out.push('\n');
    }

    // Jobs, with log tails fetched concurrently.
    let jobs: Vec<(&TargetStatus, &Job)> = notable_jobs(snap).collect();
    let mut logs: HashMap<(String, String), String> = HashMap::new();
    if cfg.insights.job_logs {
        let mut set = tokio::task::JoinSet::new();
        for (t, j) in &jobs {
            let Some(target) = cfg.targets.iter().find(|c| c.name == t.name && c.kind == Kind::Host).cloned() else {
                continue;
            };
            let (Some(path), true) = (j.log_path.clone(), j.state.is_active() || j.state == JobState::Failed) else {
                continue;
            };
            let key = (t.name.clone(), j.id.clone());
            let id = j.id.clone();
            set.spawn(async move { (key, crate::control::logs(&target, &id, Some(path), LOG_LINES).await) });
        }
        while let Some(done) = set.join_next().await {
            if let Ok((key, Ok(text))) = done {
                logs.insert(key, text);
            }
        }
    }
    if !jobs.is_empty() {
        out.push_str("JOBS\n");
        for (n, (t, j)) in jobs.iter().enumerate() {
            let r = format!("j{}", n + 1);
            subjects.insert(
                r.clone(),
                Subject {
                    kind: NoteKind::Job,
                    key: format!("{}::{}", t.name, j.id),
                    label: j.name.clone(),
                    floor: if j.state == JobState::Failed || j.eta.as_ref().is_some_and(|e| e.exceeds_limit) {
                        Tone::Bad
                    } else {
                        Tone::Ok
                    },
                },
            );
            let mut parts = vec![format!("{} on {} (job {})", j.state.label(), t.name, j.id)];
            match (j.elapsed_s, j.time_limit_s) {
                (Some(e), Some(l)) => parts.push(format!("ran {} of {} limit", human(e), human(l))),
                (Some(e), None) => parts.push(format!("ran {}", human(e))),
                _ => {}
            }
            if let Some(p) = &j.progress {
                let mut s = format!("{} ({:.0}%)", p.label, p.fraction * 100.0);
                if let Some(r) = p.it_per_s {
                    let _ = write!(s, ", {r:.2} it/s");
                }
                parts.push(s);
            }
            if let Some(e) = &j.eta {
                let late = if e.exceeds_limit { ", WILL EXCEED TIME LIMIT" } else { "" };
                parts.push(format!("ETA {} ({}){late}", human(e.remaining_s), local(e.finish_at)));
            }
            if let Some(age) = j.log_age_s {
                parts.push(format!("log last written {} ago", human(age)));
            }
            if let Some(r) = &j.resources {
                parts.push(r.clone());
            }
            let _ = writeln!(out, "[{r}] {}: {}", j.name, parts.join("; "));
            match logs.get(&(t.name.clone(), j.id.clone())) {
                Some(log) if !log.trim().is_empty() => {
                    out.push_str("    log tail:\n");
                    let lines: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
                    for l in &lines[lines.len().saturating_sub(LOG_LINES as usize)..] {
                        let _ = writeln!(out, "    | {}", clip(l, 200));
                    }
                }
                _ => {
                    if let Some(l) = &j.last_line {
                        let _ = writeln!(out, "    last log line: {}", clip(l, 200));
                    }
                }
            }
        }
        out.push('\n');
    }

    // Agents, with recent transcript activity.
    let agents: Vec<&Agent> = active_agents(snap, now).collect();
    // Transcript tails are read from disk; keep that off the async workers.
    let owned: Vec<Agent> = agents.iter().map(|a| (*a).clone()).collect();
    let recents: Vec<Vec<String>> =
        tokio::task::spawn_blocking(move || owned.iter().map(|a| activity::recent(a, AGENT_EVENTS)).collect())
            .await
            .unwrap_or_default();
    if !agents.is_empty() {
        out.push_str("AGENTS\n");
        for (n, a) in agents.iter().enumerate() {
            let r = format!("a{}", n + 1);
            subjects.insert(
                r.clone(),
                Subject {
                    kind: NoteKind::Agent,
                    key: format!("{}::{}", tool_id(a.tool), a.id),
                    label: a.project.clone().unwrap_or_else(|| a.title.clone()),
                    floor: if a.state == AgentState::Waiting && now - a.updated_at > LONG_WAIT_S {
                        Tone::Warn
                    } else {
                        Tone::Ok
                    },
                },
            );
            let state = match a.state {
                AgentState::Working => "working now".to_string(),
                AgentState::Waiting => {
                    format!("finished its turn {} ago, waiting for the user", human((now - a.updated_at).max(0) as u64))
                }
                AgentState::Idle => "idle".into(),
            };
            let mut parts = vec![a.tool.label().to_string()];
            parts.extend(a.project.iter().map(|p| format!("project {p}")));
            parts.extend(a.branch.iter().map(|b| format!("branch {b}")));
            parts.extend(a.model.clone());
            parts.push(state);
            if let Some(c) = a.context_tokens {
                parts.push(match a.context_window {
                    Some(w) => format!("context {}k of {}k", c / 1000, w / 1000),
                    None => format!("context {}k tokens", c / 1000),
                });
            }
            let _ = writeln!(out, "[{r}] {}", parts.join("; "));
            let _ = writeln!(out, "    task: {}", clip(&a.title, 140));
            let recent = recents.get(n).cloned().unwrap_or_default();
            if recent.is_empty() {
                if let Some(p) = &a.last_prompt {
                    let _ = writeln!(out, "    you: {}", clip(p, 160));
                }
                if let Some(r) = &a.last_reply {
                    let _ = writeln!(out, "    agent: {}", clip(r, 160));
                }
            } else {
                out.push_str("    recent, oldest first:\n");
                for l in recent {
                    let _ = writeln!(out, "    - {l}");
                }
            }
        }
        out.push('\n');
    }

    // Plan limits.
    let limits: Vec<String> = snap
        .usage
        .iter()
        .flat_map(|u| {
            u.limits.iter().filter(|l| l.resets_at.is_none_or(|r| r > now)).map(move |l| {
                let reset = l.resets_at.map(|r| format!(", resets in {}", human((r - now) as u64))).unwrap_or_default();
                format!("- {} {}: {:.0}% used{reset}", u.tool.label(), l.label, l.percent)
            })
        })
        .collect();
    if !limits.is_empty() {
        out.push_str("PLAN LIMITS\n");
        out.push_str(&limits.join("\n"));
        out.push('\n');
    }

    Digest { text: out, subjects }
}

/// JSON schema the model's answer must follow (Ollama structured outputs); `ref` is limited
/// to the digest's own references.
fn schema(d: &Digest) -> Value {
    let mut refs: Vec<&str> = d.subjects.keys().map(String::as_str).collect();
    refs.sort_unstable();
    refs.push("general");
    json!({
        "type": "object",
        "properties": {
            "headline": { "type": "string" },
            "notes": {
                "type": "array",
                "minItems": d.subjects.len(),
                "items": {
                    "type": "object",
                    "properties": {
                        "ref": { "type": "string", "enum": refs },
                        "text": { "type": "string" },
                        "tone": { "type": "string", "enum": ["ok", "warn", "bad"] }
                    },
                    "required": ["ref", "text", "tone"]
                }
            }
        },
        "required": ["headline", "notes"]
    })
}

/// Turn the model's JSON answer into notes, keeping only references that exist.
fn parse_answer(content: &str, d: &Digest) -> Result<(String, Vec<Note>)> {
    let body = content.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```");
    let v: Value = serde_json::from_str(body.trim()).context("the model didn't answer in JSON")?;
    let headline = clip(v.get("headline").and_then(Value::as_str).unwrap_or_default(), 200);
    let mut notes: Vec<Note> = Vec::new();
    for n in v.get("notes").and_then(Value::as_array).into_iter().flatten() {
        let r = n.get("ref").and_then(Value::as_str).unwrap_or_default();
        let r = r.trim().trim_matches(['[', ']']).to_ascii_lowercase();
        let text = clip(n.get("text").and_then(Value::as_str).unwrap_or_default(), 200);
        if text.is_empty() {
            continue;
        }
        let tone = match n.get("tone").and_then(Value::as_str) {
            Some("bad") => Tone::Bad,
            Some("warn") => Tone::Warn,
            _ => Tone::Ok,
        };
        let note = match d.subjects.get(&r) {
            Some(s) => Note {
                kind: s.kind,
                key: Some(s.key.clone()),
                label: s.label.clone(),
                text,
                tone: if severity(s.floor) > severity(tone) { s.floor } else { tone },
            },
            None => Note { kind: NoteKind::General, key: None, label: String::new(), text, tone },
        };
        // One note per row: keep the first.
        if note.key.is_some() && notes.iter().any(|x| x.key == note.key) {
            continue;
        }
        notes.push(note);
    }
    if notes.is_empty() && headline.is_empty() {
        bail!("the model returned an empty summary");
    }
    // Most urgent first.
    notes.sort_by_key(|n| std::cmp::Reverse(severity(n.tone)));
    // Notes without a headline: lead with the most urgent one.
    let headline = if headline.is_empty() {
        let n = &notes[0];
        if n.label.is_empty() { n.text.clone() } else { format!("{}: {}", n.label, n.text) }
    } else {
        headline
    };
    Ok((headline, notes))
}

/// POST/GET to Ollama through `curl` (as the live-limits fetch does): JSON body on stdin.
async fn http(url: &str, body: Option<&Value>, timeout: Duration) -> Result<Value> {
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "-m", &timeout.as_secs().to_string(), "-w", "\n%{http_code}"]);
    if body.is_some() {
        cmd.args(["-X", "POST", "-H", "Content-Type: application/json", "--data-binary", "@-"]);
    }
    cmd.arg(url)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().context("couldn't run curl")?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Some(b) = body {
            stdin.write_all(&serde_json::to_vec(b)?).await?;
        }
        drop(stdin);
    }
    let out = child.wait_with_output().await?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(match out.status.code() {
            Some(7) => anyhow!("Ollama isn't running at {} (start it with `ollama serve`)", base(url)),
            Some(28) => anyhow!("Ollama took too long to answer"),
            _ => anyhow!("couldn't reach Ollama: {}", err.trim().trim_start_matches("curl: ")),
        });
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, code) = text.rsplit_once('\n').unwrap_or((&text, ""));
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    if code.trim() != "200" {
        let msg = v.get("error").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| clip(body, 160));
        bail!("Ollama: {msg} (HTTP {})", code.trim());
    }
    Ok(v)
}

fn base(url: &str) -> &str {
    url.split("/api/").next().unwrap_or(url)
}

/// Installed models on the Ollama server, as `ollama list` names them (embedding models left out).
pub async fn models(url: &str) -> Result<Vec<String>> {
    let v = http(&format!("{}/api/tags", url.trim_end_matches('/')), None, Duration::from_secs(5)).await?;
    Ok(v.get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|m| {
            let caps = m.get("capabilities").and_then(Value::as_array);
            caps.is_none_or(|c| c.iter().any(|x| x == "completion"))
        })
        .filter_map(|m| m.get("name").and_then(Value::as_str).map(str::to_string))
        .filter(|n| !n.contains("embed"))
        .collect())
}

/// Ask the model for a summary of `d`. Returns the headline, notes and the model used.
pub async fn ask(cfg: &InsightsConfig, d: &Digest) -> Result<(String, Vec<Note>, String)> {
    let model = if cfg.model.trim().is_empty() {
        let all = models(&cfg.url).await?;
        // Code-completion models follow summary instructions poorly; prefer a general one.
        all.iter()
            .find(|m| !m.contains("coder"))
            .or(all.first())
            .cloned()
            .ok_or_else(|| anyhow!("no models in Ollama — run `ollama pull gemma3` (or any chat model)"))?
    } else {
        cfg.model.trim().to_string()
    };
    let body = json!({
        "model": model,
        "stream": false,
        "think": false,
        "format": schema(d),
        "keep_alive": "10m",
        "options": { "temperature": 0.2, "num_ctx": 8192 },
        "messages": [
            { "role": "system", "content": SYSTEM },
            { "role": "user", "content": d.text },
        ],
    });
    let v = http(&format!("{}/api/chat", cfg.url.trim_end_matches('/')), Some(&body), GENERATE_TIMEOUT).await?;
    let content = v.pointer("/message/content").and_then(Value::as_str).unwrap_or_default();
    let (headline, notes) = parse_answer(content, d)?;
    Ok((headline, notes, model))
}

/// Keeps the latest summary and decides when a new one is worth writing.
#[derive(Default)]
pub struct Engine {
    current: Insights,
    last_print: Option<u64>,
}

impl Engine {
    pub fn current(&self) -> Insights {
        self.current.clone()
    }

    /// Something changed since the last summary (or there's none yet).
    pub fn due(&self, snap: &Snapshot) -> bool {
        self.last_print != Some(fingerprint(snap))
    }

    /// Mark a summary as in progress.
    pub fn start(&mut self) -> Insights {
        self.current.busy = true;
        self.current.clone()
    }

    /// Write a new summary of `snap`. Failures keep the previous summary and record the error.
    pub async fn run(&mut self, snap: &Snapshot, cfg: &Config) -> Insights {
        let print = fingerprint(snap);
        if is_quiet(snap) {
            self.current = Insights {
                generated_at: now(),
                model: None,
                headline: "All quiet: no jobs running and no agents active.".into(),
                ..Default::default()
            };
            self.last_print = Some(print);
            return self.current();
        }
        let d = digest(snap, cfg).await;
        match ask(&cfg.insights, &d).await {
            Ok((headline, notes, model)) => {
                self.current =
                    Insights { generated_at: now(), model: Some(model), headline, notes, busy: false, error: None };
                self.last_print = Some(print);
            }
            Err(e) => {
                self.current.busy = false;
                self.current.error = Some(format!("{e:#}"));
            }
        }
        self.current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Progress, TargetStatus};

    fn snap() -> Snapshot {
        let mut j = Job::new("42", "train-vit", JobState::Running);
        j.progress = Some(Progress {
            fraction: 0.5,
            label: "epoch 5/10".into(),
            it_per_s: Some(2.0),
            tool_remaining_s: None,
            throughput_remaining_s: None,
        });
        j.last_line = Some("loss 0.31".into());
        Snapshot {
            taken_at: now(),
            targets: vec![TargetStatus {
                name: "dgx".into(),
                kind: "slurm · ssh dgx".into(),
                ok: true,
                updated_at: now(),
                jobs: vec![j],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn digest_lists_jobs_with_refs() {
        let cfg = Config::parse("").unwrap();
        let d = digest(&snap(), &cfg).await;
        assert!(d.text.contains("[j1] train-vit: running on dgx (job 42)"), "{}", d.text);
        assert!(d.text.contains("epoch 5/10 (50%), 2.00 it/s"));
        assert!(d.text.contains("last log line: loss 0.31"));
        assert_eq!(d.subjects["j1"].key, "dgx::42");
    }

    #[tokio::test]
    async fn answers_map_back_to_rows() {
        let cfg = Config::parse("").unwrap();
        let d = digest(&snap(), &cfg).await;
        let answer = r#"{"headline":"One job halfway.","notes":[
            {"ref":"j1","text":"Halfway, loss falling.","tone":"ok"},
            {"ref":"[J1]","text":"duplicate","tone":"bad"},
            {"ref":"x9","text":"GPUs mostly idle.","tone":"warn"}]}"#;
        let (h, notes) = parse_answer(answer, &d).unwrap();
        assert_eq!(h, "One job halfway.");
        assert_eq!(notes.len(), 2);
        assert_eq!((notes[0].kind, notes[0].tone), (NoteKind::General, Tone::Warn), "warn sorts first");
        assert_eq!(notes[1].key.as_deref(), Some("dgx::42"));
        assert_eq!(notes[1].label, "train-vit");
        assert!(parse_answer("not json", &d).is_err());

        let mut failed = snap();
        failed.targets[0].jobs[0].state = JobState::Failed;
        let d = digest(&failed, &cfg).await;
        let (_, notes) =
            parse_answer(r#"{"headline":"x","notes":[{"ref":"j1","text":"Done.","tone":"ok"}]}"#, &d).unwrap();
        assert_eq!(notes[0].tone, Tone::Bad, "a failed job is never ok");
        assert!(parse_answer(r#"{"headline":"","notes":[]}"#, &d).is_err());
    }

    #[test]
    fn quiet_and_fingerprint() {
        assert!(is_quiet(&Snapshot::default()));
        let s = snap();
        assert!(!is_quiet(&s));
        let mut t = s.clone();
        t.taken_at += 100;
        assert_eq!(fingerprint(&s), fingerprint(&t), "clock alone doesn't change it");
        t.targets[0].jobs[0].progress.as_mut().unwrap().label = "epoch 6/10".into();
        assert_ne!(fingerprint(&s), fingerprint(&t));

        let mut stalled = s.clone();
        stalled.targets[0].jobs[0].log_age_s = Some(SILENT_LOG_S + 1);
        assert_ne!(fingerprint(&s), fingerprint(&stalled), "a log going silent is news");

        let mut queued = Snapshot { targets: s.targets.clone(), ..Default::default() };
        queued.targets[0].jobs[0].state = JobState::Pending;
        assert!(!is_quiet(&queued), "a queue alone is worth a look");
    }

    #[tokio::test]
    async fn notes_without_headline_lead_with_the_worst() {
        let cfg = Config::parse("").unwrap();
        let d = digest(&snap(), &cfg).await;
        let (h, _) =
            parse_answer(r#"{"headline":"","notes":[{"ref":"j1","text":"Loss is NaN.","tone":"bad"}]}"#, &d).unwrap();
        assert_eq!(h, "train-vit: Loss is NaN.");
    }
}
