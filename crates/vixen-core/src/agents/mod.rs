//! Code-agent sessions (Claude Code, Codex, Antigravity), read from each tool's own
//! session files on this machine. Nothing is sent anywhere; files are only read.
//!
//! Each scan stats candidate files and re-parses only those whose size or mtime
//! changed, so polling every few seconds stays cheap.

mod antigravity;
mod claude;
mod codex;
mod usage;

use crate::config::Agents;
use crate::model::{Agent, AgentState, Tool, ToolUsage};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use usage::{LimitCache, TokenLog};

/// Bytes read from the end of a transcript; enough for the last few turns.
const TAIL_BYTES: u64 = 256 * 1024;
/// Mid-turn with no writes for this long → treat as stalled (idle).
const STALL_S: i64 = 10 * 60;
/// A finished turn counts as "waiting for you" for this long.
const WAITING_S: i64 = 60 * 60;
/// Live Claude limits are re-fetched this often.
const LIVE_EVERY_S: i64 = 300;

/// What a parser extracts; `state` is derived at scan time from age.
pub(crate) struct Parsed {
    pub agent: Agent,
    /// The last turn hasn't finished (tool calls / generation in flight).
    pub in_progress: bool,
}

#[derive(Default)]
pub struct Scanner {
    cache: HashMap<PathBuf, (SystemTime, u64, Option<Parsed>)>,
    tokens: HashMap<PathBuf, (Tool, TokenLog)>,
    claude_limits: LimitCache,
    codex_limits: LimitCache,
    /// Live Claude limits (when enabled) and when to try fetching next.
    claude_live: Option<(Vec<crate::model::UsageLimit>, i64)>,
    next_live: i64,
}

/// One scan: sessions plus per-tool limits and token totals.
#[derive(Default)]
pub struct Scan {
    pub agents: Vec<Agent>,
    pub usage: Vec<ToolUsage>,
}

impl Scanner {
    /// All sessions active within the window, working first, then waiting, then idle.
    pub async fn scan(&mut self, cfg: &Agents) -> Scan {
        if !cfg.enabled {
            return Scan::default();
        }
        let now = crate::monitor::now();
        let cutoff = now - (cfg.window_hours as i64) * 3600;
        let mut files: Vec<(Tool, PathBuf)> = Vec::new();
        if cfg.watches(Tool::ClaudeCode) {
            files.extend(claude::files().into_iter().map(|f| (Tool::ClaudeCode, f)));
        }
        if cfg.watches(Tool::Codex) {
            files.extend(codex::files(cfg.window_hours).into_iter().map(|f| (Tool::Codex, f)));
        }

        let mut seen = Vec::with_capacity(files.len());
        let mut out = Vec::new();
        for (tool, path) in files {
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            if unix(mtime) < cutoff {
                continue;
            }
            seen.push(path.clone());
            self.tokens.entry(path.clone()).or_insert_with(|| (tool, TokenLog::default())).1.update(
                tool,
                &path,
                meta.len(),
            );
            let entry = self.cache.entry(path.clone()).or_insert((SystemTime::UNIX_EPOCH, u64::MAX, None));
            if entry.0 != mtime || entry.1 != meta.len() {
                let parsed = match tool {
                    Tool::ClaudeCode => claude::parse(&path),
                    Tool::Codex => codex::parse(&path),
                    Tool::Antigravity => None,
                };
                *entry = (mtime, meta.len(), parsed);
            }
            if let Some(p) = &entry.2 {
                let mut a = p.agent.clone();
                a.updated_at = a.updated_at.max(unix(mtime));
                a.state = state(p.in_progress, now - a.updated_at);
                out.push(a);
            }
        }
        self.cache.retain(|k, _| seen.contains(k));
        self.tokens.retain(|k, _| seen.contains(k));

        if cfg.watches(Tool::Antigravity) {
            for p in antigravity::sessions(cutoff).await {
                let mut a = p.agent;
                a.state = state(p.in_progress, now - a.updated_at);
                out.push(a);
            }
        }

        let rank = |s: AgentState| match s {
            AgentState::Working => 0,
            AgentState::Waiting => 1,
            AgentState::Idle => 2,
        };
        out.sort_by(|a, b| rank(a.state).cmp(&rank(b.state)).then(b.updated_at.cmp(&a.updated_at)));
        let usage = self.usage(cfg, now).await;
        Scan { agents: out, usage }
    }

    async fn usage(&mut self, cfg: &Agents, now: i64) -> Vec<ToolUsage> {
        let midnight = chrono::Local::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .and_then(|d| d.and_local_timezone(chrono::Local).earliest())
            .map_or(now - 86400, |d| d.timestamp());
        if cfg.watches(Tool::ClaudeCode) {
            self.claude_limits.refresh(&claude::state_file(), usage::claude_limits);
            if cfg.live_claude_limits && now >= self.next_live {
                let wait = match usage::claude_live().await {
                    Ok(limits) => {
                        self.claude_live = Some((limits, now));
                        LIVE_EVERY_S
                    }
                    Err(usage::LiveError::NoLogin) => 3600,
                    Err(usage::LiveError::Expired) => 120,
                    Err(usage::LiveError::Fetch) => 300,
                };
                self.next_live = now + wait;
            }
        }
        if cfg.watches(Tool::Codex)
            && let Some(newest) =
                codex::files(24 * 7).into_iter().max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        {
            self.codex_limits.refresh(&newest, usage::codex_limits);
        }

        let mut out = Vec::new();
        for (tool, limits) in [(Tool::ClaudeCode, &self.claude_limits), (Tool::Codex, &self.codex_limits)] {
            if !cfg.watches(tool) {
                continue;
            }
            // Prefer a live fetch when it's newer than what the tool saved.
            let (current, as_of) = match &self.claude_live {
                Some((l, at)) if tool == Tool::ClaudeCode && limits.as_of.is_none_or(|t| *at >= t) => {
                    (l.clone(), Some(*at))
                }
                _ => (limits.limits.clone(), limits.as_of),
            };
            let logs = self.tokens.values().filter(|(t, _)| *t == tool).map(|(_, l)| l);
            let (mut five, mut today, mut sessions) = (0, 0, 0);
            for log in logs {
                five += log.since(now - 5 * 3600);
                today += log.since(midnight);
                sessions += usize::from(log.active_since(midnight));
            }
            if current.is_empty() && today == 0 && five == 0 {
                continue;
            }
            out.push(ToolUsage {
                tool,
                plan: limits.plan.clone(),
                limits: current,
                limits_as_of: as_of,
                tokens_5h: five,
                tokens_today: today,
                sessions_today: sessions,
            });
        }
        out
    }
}

pub(crate) fn state(in_progress: bool, age_s: i64) -> AgentState {
    match (in_progress, age_s) {
        (true, a) if a < STALL_S => AgentState::Working,
        (false, a) if a < WAITING_S => AgentState::Waiting,
        _ => AgentState::Idle,
    }
}

fn unix(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Last `TAIL_BYTES` of a file, starting at a line boundary.
pub(crate) fn read_tail(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    Some(if start > 0 { text.split_once('\n').map(|(_, rest)| rest.to_string()).unwrap_or_default() } else { text })
}

/// First line of a file (bounded), e.g. a session header.
pub(crate) fn first_line(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 64 * 1024];
    let n = f.read(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf[..n]);
    text.lines().next().map(str::to_string)
}

/// RFC 3339 timestamp → unix seconds.
pub(crate) fn ts(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp())
}

/// Single-line, whitespace-collapsed, at most `n` characters.
pub(crate) fn clip(s: &str, n: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= n {
        return flat;
    }
    let mut out: String = flat.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub(crate) fn project(cwd: &str) -> Option<String> {
    Path::new(cwd).file_name().map(|n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_rules() {
        assert_eq!(state(true, 30), AgentState::Working);
        assert_eq!(state(true, STALL_S + 1), AgentState::Idle);
        assert_eq!(state(false, 120), AgentState::Waiting);
        assert_eq!(state(false, WAITING_S + 1), AgentState::Idle);
    }

    #[test]
    fn clips() {
        assert_eq!(clip("  a\n b  ", 10), "a b");
        assert_eq!(clip("abcdef", 4), "abc…");
    }
}
