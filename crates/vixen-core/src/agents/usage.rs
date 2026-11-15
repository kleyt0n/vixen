//! Plan limits and token totals per agent tool.
//!
//! * Tokens: transcripts are read incrementally (each file once in full, then only
//!   appended bytes). Claude Code writes one line per streamed content block, all
//!   carrying the same usage, so messages are de-duplicated by id. Codex records a
//!   running total, so a window's use is the difference across it.
//! * Limits: Claude Code caches what `/usage` last fetched in `~/.claude.json`;
//!   Codex logs `rate_limits` with every token count. Both are only as fresh as the
//!   tool's last update, so `limits_as_of` is reported alongside.

use crate::model::{Tool, UsageLimit};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Token events from one transcript.
#[derive(Default)]
pub(crate) struct TokenLog {
    offset: u64,
    /// Claude: message id → (timestamp, tokens), last write wins.
    messages: HashMap<String, (i64, u64)>,
    /// Codex: (timestamp, running total).
    totals: Vec<(i64, u64)>,
}

impl TokenLog {
    /// Read whatever was appended since the last call.
    pub fn update(&mut self, tool: Tool, path: &Path, len: u64) {
        if len < self.offset {
            *self = Self::default(); // truncated or replaced
        }
        if len == self.offset {
            return;
        }
        let Ok(mut f) = std::fs::File::open(path) else { return };
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        if f.take(len - self.offset).read_to_end(&mut buf).is_err() {
            return;
        }
        // Only consume complete lines; a partial last line is re-read next time.
        let Some(end) = buf.iter().rposition(|&b| b == b'\n') else { return };
        self.offset += end as u64 + 1;
        for line in String::from_utf8_lossy(&buf[..end]).lines() {
            match tool {
                Tool::ClaudeCode => self.claude_line(line),
                Tool::Codex => self.codex_line(line),
                Tool::Antigravity => {}
            }
        }
    }

    fn claude_line(&mut self, line: &str) {
        if !line.contains("\"usage\"") {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        let Some(m) = v.get("message") else { return };
        let (Some(id), Some(u)) = (m.get("id").and_then(Value::as_str), m.get("usage")) else { return };
        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        let tokens =
            n("input_tokens") + n("output_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
        let at = v.get("timestamp").and_then(Value::as_str).and_then(super::ts).unwrap_or(0);
        self.messages.insert(id.to_string(), (at, tokens));
    }

    fn codex_line(&mut self, line: &str) {
        if !line.contains("\"token_count\"") {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        let total = v.pointer("/payload/info/total_token_usage/total_tokens").and_then(Value::as_u64);
        let at = v.get("timestamp").and_then(Value::as_str).and_then(super::ts);
        if let (Some(total), Some(at)) = (total, at) {
            self.totals.push((at, total));
        }
    }

    /// Tokens used at or after `since`.
    pub fn since(&self, since: i64) -> u64 {
        if !self.messages.is_empty() {
            return self.messages.values().filter(|(t, _)| *t >= since).map(|(_, n)| n).sum();
        }
        let Some(&(_, last)) = self.totals.last() else { return 0 };
        let before = self.totals.iter().rev().find(|(t, _)| *t < since).map_or(0, |(_, n)| *n);
        last.saturating_sub(before)
    }

    /// Tokens per hour for the 24 hours ending with the hour containing `now`, oldest first.
    pub fn by_hour(&self, now: i64) -> [u64; 24] {
        let mut out = [0u64; 24];
        let current = now - now.rem_euclid(3600);
        let mut put = |at: i64, n: u64| {
            let back = (current - (at - at.rem_euclid(3600))) / 3600;
            if (0..24).contains(&back) {
                out[23 - back as usize] += n;
            }
        };
        for &(at, n) in self.messages.values() {
            put(at, n);
        }
        let mut prev = 0;
        for &(at, total) in &self.totals {
            put(at, total.saturating_sub(prev));
            prev = total;
        }
        out
    }

    pub fn active_since(&self, since: i64) -> bool {
        self.messages.values().any(|(t, _)| *t >= since) || self.totals.iter().any(|(t, _)| *t >= since)
    }
}

/// Parsed limits, cached by the source file's mtime.
#[derive(Default)]
pub(crate) struct LimitCache {
    stamp: Option<(PathBuf, SystemTime)>,
    pub plan: Option<String>,
    pub limits: Vec<UsageLimit>,
    pub as_of: Option<i64>,
}

impl LimitCache {
    /// Re-parse only when `path` (or its mtime) changed.
    pub fn refresh(
        &mut self,
        path: &Path,
        parse: impl Fn(&Path) -> Option<(Option<String>, Vec<UsageLimit>, Option<i64>)>,
    ) {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let stamp = mtime.map(|m| (path.to_path_buf(), m));
        if stamp.is_none() || stamp == self.stamp {
            return;
        }
        self.stamp = stamp;
        (self.plan, self.limits, self.as_of) = parse(path).unwrap_or_default();
    }
}

/// `~/.claude.json` → `cachedUsageUtilization`, as last fetched by Claude Code.
pub(crate) fn claude_limits(path: &Path) -> Option<(Option<String>, Vec<UsageLimit>, Option<i64>)> {
    let v: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    parse_claude(v.get("cachedUsageUtilization")?)
}

pub(crate) fn parse_claude(cache: &Value) -> Option<(Option<String>, Vec<UsageLimit>, Option<i64>)> {
    let as_of = cache.get("fetchedAtMs").and_then(Value::as_i64).map(|ms| ms / 1000);
    Some((None, parse_utilization(cache.get("utilization")?), as_of))
}

/// The `utilization` object, as cached by Claude Code or returned by the usage endpoint.
pub(crate) fn parse_utilization(u: &Value) -> Vec<UsageLimit> {
    let resets = |v: &Value| v.get("resets_at").and_then(Value::as_str).and_then(super::ts);
    let mut limits = Vec::new();
    if let Some(list) = u.get("limits").and_then(Value::as_array) {
        for l in list {
            let Some(percent) = l.get("percent").and_then(Value::as_f64) else { continue };
            let scoped = l.pointer("/scope/model/display_name").and_then(Value::as_str);
            let label = match (l.get("kind").and_then(Value::as_str), scoped) {
                (Some("session"), _) => "Session (5h)".to_string(),
                (_, Some(model)) => format!("Weekly · {model}"),
                (Some(k), _) if k.starts_with("weekly") => "Weekly".to_string(),
                (Some(k), _) => k.replace('_', " "),
                _ => continue,
            };
            let severity = l.get("severity").and_then(Value::as_str).map(str::to_string);
            limits.push(UsageLimit { label, percent, resets_at: resets(l), severity });
        }
    } else {
        for (key, label) in [("five_hour", "Session (5h)"), ("seven_day", "Weekly")] {
            if let Some(l) = u.get(key).filter(|l| !l.is_null())
                && let Some(percent) = l.get("utilization").and_then(Value::as_f64)
            {
                limits.push(UsageLimit { label: label.into(), percent, resets_at: resets(l), severity: None });
            }
        }
    }
    limits
}

/// Why a live fetch didn't happen; decides how long to wait before trying again.
#[derive(Debug, PartialEq)]
pub(crate) enum LiveError {
    /// No login, or Keychain access refused: try again much later (avoid repeated prompts).
    NoLogin,
    /// Token expired; Claude Code renews it when it next runs. Vixen never refreshes it.
    Expired,
    /// Network or API error.
    Fetch,
}

/// Current Claude plan limits from the endpoint behind Claude Code's `/usage`, using
/// Claude Code's own saved login (Keychain on macOS, `~/.claude/.credentials.json`
/// elsewhere). The token is passed to curl on stdin, never on the command line.
pub(crate) async fn claude_live() -> Result<Vec<UsageLimit>, LiveError> {
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;

    let file = super::claude::root().join(".credentials.json");
    let creds = match std::fs::read(&file) {
        Ok(b) => b,
        Err(_) if cfg!(target_os = "macos") => {
            let out = Command::new("security")
                .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
                .output()
                .await
                .map_err(|_| LiveError::NoLogin)?;
            if !out.status.success() {
                return Err(LiveError::NoLogin);
            }
            out.stdout
        }
        Err(_) => return Err(LiveError::NoLogin),
    };
    let v: Value = serde_json::from_slice(&creds).map_err(|_| LiveError::NoLogin)?;
    let oauth = v.get("claudeAiOauth").ok_or(LiveError::NoLogin)?;
    let token = oauth.get("accessToken").and_then(Value::as_str).ok_or(LiveError::NoLogin)?;
    let expires = oauth.get("expiresAt").and_then(Value::as_i64).unwrap_or(i64::MAX) / 1000;
    if expires <= crate::monitor::now() + 30 {
        return Err(LiveError::Expired);
    }

    let mut child = Command::new("curl")
        .args(["-sS", "-m", "15", "-K", "-", "-w", "\n%{http_code}", "https://api.anthropic.com/api/oauth/usage"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| LiveError::Fetch)?;
    let config = format!(
        "header = \"Authorization: Bearer {token}\"\nheader = \"anthropic-beta: oauth-2025-04-20\"\nheader = \"Accept: application/json\"\n"
    );
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(config.as_bytes()).await.map_err(|_| LiveError::Fetch)?;
    }
    let out = child.wait_with_output().await.map_err(|_| LiveError::Fetch)?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, code) = text.rsplit_once('\n').ok_or(LiveError::Fetch)?;
    match code.trim() {
        "200" => {}
        "401" | "403" => return Err(LiveError::Expired),
        _ => return Err(LiveError::Fetch),
    }
    let v: Value = serde_json::from_str(body).map_err(|_| LiveError::Fetch)?;
    let limits = parse_utilization(&v);
    if limits.is_empty() { Err(LiveError::Fetch) } else { Ok(limits) }
}

/// Latest `rate_limits` in a Codex rollout.
pub(crate) fn codex_limits(path: &Path) -> Option<(Option<String>, Vec<UsageLimit>, Option<i64>)> {
    let tail = super::read_tail(path)?;
    tail.lines().rev().filter(|l| l.contains("\"rate_limits\"")).find_map(|l| {
        let v: Value = serde_json::from_str(l).ok()?;
        parse_codex(v.pointer("/payload/rate_limits")?, v.get("timestamp").and_then(Value::as_str).and_then(super::ts))
    })
}

pub(crate) fn parse_codex(r: &Value, at: Option<i64>) -> Option<(Option<String>, Vec<UsageLimit>, Option<i64>)> {
    let mut limits = Vec::new();
    for key in ["primary", "secondary"] {
        let Some(l) = r.get(key).filter(|l| l.get("used_percent").is_some()) else { continue };
        let minutes = l.get("window_minutes").and_then(Value::as_u64).unwrap_or(0);
        let label = match minutes {
            300 => "Session (5h)".to_string(),
            10080 => "Weekly".to_string(),
            m if m >= 1440 && m % 1440 == 0 => format!("{} days", m / 1440),
            m if m > 0 => format!("{}h", m.div_ceil(60)),
            _ => key.to_string(),
        };
        limits.push(UsageLimit {
            label,
            percent: l.get("used_percent").and_then(Value::as_f64).unwrap_or(0.0),
            resets_at: l.get("resets_at").and_then(Value::as_i64),
            severity: None,
        });
    }
    let plan = r.get("plan_type").and_then(Value::as_str).map(|p| p.replace('_', " "));
    (!limits.is_empty()).then_some((plan, limits, at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_dedupes_streamed_blocks() {
        let mut log = TokenLog::default();
        let line = |id: &str, ts: &str, out: u64| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"{id}","usage":{{"input_tokens":10,"output_tokens":{out},"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}}}}"#
            )
        };
        // Same message streamed as two blocks, then a second message.
        for l in [
            line("m1", "2026-11-14T10:00:00Z", 5),
            line("m1", "2026-11-14T10:00:01Z", 20),
            line("m2", "2026-11-14T12:00:00Z", 1),
        ] {
            log.claude_line(&l);
        }
        assert_eq!(log.since(0), 130 + 111);
        let hours = log.by_hour(super::super::ts("2026-11-14T12:10:00Z").unwrap());
        assert_eq!((hours[23], hours[21]), (111, 130));
        assert_eq!(log.since(super::super::ts("2026-11-14T11:00:00Z").unwrap()), 111);
    }

    #[test]
    fn codex_window_is_a_difference() {
        let mut log = TokenLog::default();
        for (ts, total) in
            [("2026-11-14T08:00:00Z", 1000), ("2026-11-14T10:00:00Z", 4000), ("2026-11-14T12:00:00Z", 9000)]
        {
            log.codex_line(&format!(
                r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":{total}}}}}}}}}"#
            ));
        }
        assert_eq!(log.since(super::super::ts("2026-11-14T09:00:00Z").unwrap()), 8000);
        assert_eq!(log.since(0), 9000);
        // 08:00 → 1000, 10:00 → +3000, 12:00 → +5000; "now" is 12:30.
        let hours = log.by_hour(super::super::ts("2026-11-14T12:30:00Z").unwrap());
        assert_eq!((hours[23], hours[21], hours[19], hours.iter().sum::<u64>()), (5000, 3000, 1000, 9000));
    }

    #[test]
    fn claude_limits_from_cache() {
        let v: Value = serde_json::from_str(
            r#"{"fetchedAtMs":1794524184890,"utilization":{"limits":[
              {"kind":"session","percent":23,"severity":"normal","resets_at":"2026-11-12T23:49:59Z"},
              {"kind":"weekly_all","percent":95,"severity":"critical","resets_at":"2026-11-13T13:59:58Z"},
              {"kind":"weekly_scoped","percent":50,"resets_at":"2026-11-13T13:59:59Z","scope":{"model":{"display_name":"Opus"}}}]}}"#,
        )
        .unwrap();
        let (_, limits, as_of) = parse_claude(&v).unwrap();
        assert_eq!(as_of, Some(1794524184));
        let labels: Vec<_> = limits.iter().map(|l| (l.label.as_str(), l.percent)).collect();
        assert_eq!(labels, [("Session (5h)", 23.0), ("Weekly", 95.0), ("Weekly · Opus", 50.0)]);
        assert_eq!(limits[1].severity.as_deref(), Some("critical"));
    }

    #[test]
    fn codex_limits_from_event() {
        let r: Value = serde_json::from_str(
            r#"{"primary":{"used_percent":57.0,"window_minutes":10080,"resets_at":1794583688},"secondary":{},"plan_type":"self_serve_business_prolite"}"#,
        )
        .unwrap();
        let (plan, limits, _) = parse_codex(&r, Some(1)).unwrap();
        assert_eq!(plan.as_deref(), Some("self serve business prolite"));
        assert_eq!(limits.len(), 1);
        assert_eq!(
            (limits[0].label.as_str(), limits[0].percent, limits[0].resets_at),
            ("Weekly", 57.0, Some(1794583688))
        );
    }
}
