//! Plain-text rendering of a snapshot.

use chrono::{Local, TimeZone};
use std::fmt::Write;
use vixen_core::timefmt::human;
use vixen_core::{Agent, AgentState, EtaMethod, Gpu, Insights, Job, JobState, Snapshot, TargetStatus, Tone, ToolUsage};

struct Paint(bool);

impl Paint {
    fn c(&self, code: &str, s: &str) -> String {
        if self.0 { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
    }
    fn bold(&self, s: &str) -> String {
        self.c("1", s)
    }
    fn dim(&self, s: &str) -> String {
        self.c("2", s)
    }
    fn red(&self, s: &str) -> String {
        self.c("31", s)
    }
    fn green(&self, s: &str) -> String {
        self.c("32", s)
    }
    fn yellow(&self, s: &str) -> String {
        self.c("33", s)
    }
    fn blue(&self, s: &str) -> String {
        self.c("34", s)
    }
}

pub fn dim(s: &str, color: bool) -> String {
    Paint(color).dim(s)
}

/// `queued_limit`: max pending/held rows per target (None = all).
pub fn snapshot(s: &Snapshot, color: bool, queued_limit: Option<usize>) -> String {
    let p = Paint(color);
    let mut out = String::new();
    for t in &s.targets {
        target(&mut out, t, &p, s.taken_at, queued_limit);
    }
    out
}

fn target(out: &mut String, t: &TargetStatus, p: &Paint, now: i64, queued_limit: Option<usize>) {
    let dot = match (t.ok, t.updated_at) {
        (true, _) => p.green("●"),
        (false, 0) => p.red("●"),
        (false, _) => p.yellow("●"),
    };
    let _ = writeln!(out, "{dot} {}  {}  {}", p.bold(&t.name), p.dim(&t.kind), p.dim(&format!("{}ms", t.latency_ms)));
    if let Some(e) = &t.error {
        let stale = if t.updated_at > 0 {
            format!(" (showing data from {} ago)", human((now - t.updated_at).max(0) as u64))
        } else {
            String::new()
        };
        let _ = writeln!(out, "  {}{}", p.red(e), p.dim(&stale));
    }
    if let Some(h) = &t.host {
        let _ = writeln!(
            out,
            "  {}  load {:.1}/{}  ram {}",
            p.dim(&h.hostname),
            h.load1,
            h.cpus,
            mem(Some(h.mem_used_mb), Some(h.mem_total_mb))
        );
    }
    for g in &t.gpus {
        gpu(out, g, p);
    }
    if !t.jobs.is_empty() {
        let _ = writeln!(
            out,
            "  {}",
            p.dim(&format!(
                "{:<20} {:<26} {:<10} {:>8} {:>9}  {:<22} {:>8}  {}",
                "JOB", "NAME", "STATE", "ELAPSED", "LIMIT", "PROGRESS", "ETA", "FINISH"
            ))
        );
        let mut queued = 0;
        let mut hidden: std::collections::BTreeMap<&str, usize> = Default::default();
        for j in &t.jobs {
            if !j.state.is_active() {
                queued += 1;
                if queued_limit.is_some_and(|l| queued > l) {
                    *hidden.entry(j.state.label()).or_default() += 1;
                    continue;
                }
            }
            job(out, j, p);
        }
        if !hidden.is_empty() {
            let parts: Vec<String> = hidden.iter().map(|(s, n)| format!("{n} {s}")).collect();
            let _ = writeln!(out, "  {}", p.dim(&format!("… +{} (--all to show)", parts.join(", "))));
        }
    } else if t.ok {
        let _ = writeln!(out, "  {}", p.dim("no jobs"));
    }
    out.push('\n');
}

fn bar(frac: f64, width: usize) -> String {
    let cells = frac.clamp(0.0, 1.0) * width as f64;
    let full = cells.floor() as usize;
    let parts = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let rem = parts[((cells - full as f64) * 8.0) as usize % 8];
    let mut s = "█".repeat(full);
    s.push_str(rem);
    let used = full + usize::from(!rem.is_empty());
    s.push_str(&" ".repeat(width.saturating_sub(used)));
    s
}

fn mem(used: Option<u64>, total: Option<u64>) -> String {
    match (used, total) {
        (Some(u), Some(t)) => format!("{:.1}/{:.0}G", u as f64 / 1024.0, t as f64 / 1024.0),
        (Some(u), None) => format!("{:.1}G", u as f64 / 1024.0),
        _ => "–".into(),
    }
}

fn gpu(out: &mut String, g: &Gpu, p: &Paint) {
    let util = g.util.unwrap_or(0.0);
    let b = bar(util as f64 / 100.0, 12);
    let b = if util >= 80.0 {
        p.green(&b)
    } else if util >= 30.0 {
        p.yellow(&b)
    } else {
        p.dim(&b)
    };
    let mut extra = Vec::new();
    extra.push(format!("mem {}{}", mem(g.mem_used_mb, g.mem_total_mb), if g.unified { " unified" } else { "" }));
    if let Some(t) = g.temp_c {
        extra.push(format!("{t:.0}°C"));
    }
    if let Some(w) = g.power_w {
        extra.push(match g.power_limit_w {
            Some(l) => format!("{w:.0}/{l:.0}W"),
            None => format!("{w:.0}W"),
        });
    }
    if let Some(j) = &g.job_id {
        extra.push(format!("job {j}{}", g.node.as_deref().map(|n| format!(" @ {n}")).unwrap_or_default()));
    }
    let util_s = g.util.map(|u| format!("{u:>3.0}%")).unwrap_or_else(|| "  –".into());
    let _ = writeln!(out, "  gpu{} {:<18} {b} {util_s}  {}", g.index, trunc(&g.name, 18), p.dim(&extra.join("  ")));
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n - 1).chain(['…']).collect() }
}

fn clock(ts: i64) -> String {
    let Some(dt) = Local.timestamp_opt(ts, 0).single() else { return "?".into() };
    let today = Local::now().date_naive();
    if dt.date_naive() == today { dt.format("%H:%M").to_string() } else { dt.format("%a %H:%M").to_string() }
}

fn job(out: &mut String, j: &Job, p: &Paint) {
    let state = {
        let s = j.state.label();
        match j.state {
            JobState::Running => p.green(s),
            JobState::Pending => p.blue(s),
            JobState::Held | JobState::Suspended => p.yellow(s),
            JobState::Failed => p.red(s),
            _ => p.dim(s),
        }
    };
    let pad = 10 + state.len() - j.state.label().len();
    let mut id = j.id.clone();
    if let Some(n) = j.array_tasks {
        id = format!("{id} ×{n}");
    }
    let elapsed = j.elapsed_s.map(human).unwrap_or_default();
    let limit = match (j.time_left_s(), j.time_limit_s) {
        (Some(l), _) => format!("{} left", human(l)),
        (None, Some(l)) => human(l),
        _ => String::new(),
    };
    let (progress, eta, finish) = match (&j.progress, &j.eta) {
        (Some(pr), eta) => {
            let pct = format!("{:>5.1}%", pr.fraction * 100.0);
            let prog = format!("{} {pct}", bar(pr.fraction, 14));
            match eta {
                Some(e) => {
                    let mark = if e.method == EtaMethod::Elapsed { "~" } else { "" };
                    let eta_s = format!("{mark}{}", human(e.remaining_s));
                    let fin = clock(e.finish_at);
                    let fin = if e.exceeds_limit { p.red(&format!("{fin} ⚠ exceeds limit")) } else { fin };
                    (prog, eta_s, fin)
                }
                None => (prog, String::new(), String::new()),
            }
        }
        (None, _) if j.state == JobState::Pending => (
            String::new(),
            String::new(),
            j.est_start_at.map(|t| p.dim(&format!("starts ~{}", clock(t)))).unwrap_or_default(),
        ),
        _ => (String::new(), String::new(), String::new()),
    };
    let _ = writeln!(
        out,
        "  {:<20} {:<26} {state:<pad$} {elapsed:>8} {limit:>9}  {progress:<22} {eta:>8}  {finish}",
        trunc(&id, 20),
        trunc(&j.name, 26),
    );
    let mut sub = Vec::new();
    if let Some(pr) = &j.progress {
        let mut s = pr.label.clone();
        if let Some(r) = pr.it_per_s {
            s.push_str(&format!(" · {r:.2} it/s"));
        }
        sub.push(s);
    }
    if let Some(r) = &j.resources {
        sub.push(r.clone());
    }
    if let Some(d) = &j.detail {
        sub.push(d.clone());
    }
    if let Some(a) = j.log_age_s.filter(|a| *a > 600 && j.state == JobState::Running) {
        sub.push(p.yellow(&format!("log silent {}", human(a))));
    }
    if !sub.is_empty() {
        let _ = writeln!(out, "  {:<20} {}", "", p.dim(&sub.join("  ·  ")));
    }
}

/// Agent sessions: working and waiting always shown; idle ones up to `idle_limit`.
pub fn agents(list: &[Agent], color: bool, idle_limit: Option<usize>) -> String {
    let p = Paint(color);
    let now = vixen_core::monitor::now();
    let mut out = String::new();
    if list.is_empty() {
        return p.dim("no agent sessions in the window\n");
    }
    let idle = list.iter().filter(|a| a.state == AgentState::Idle).count();
    let mut shown_idle = 0;
    for a in list {
        if a.state == AgentState::Idle {
            if idle_limit.is_some_and(|n| shown_idle >= n) {
                continue;
            }
            shown_idle += 1;
        }
        let state = match a.state {
            AgentState::Working => p.green("● working"),
            AgentState::Waiting => p.yellow("◆ waiting"),
            AgentState::Idle => p.dim("○ idle   "),
        };
        let place = [a.project.as_deref(), a.branch.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        let ctx = a.context_tokens.map(|t| format!("{}k ctx", t / 1000)).unwrap_or_default();
        let _ = writeln!(
            out,
            "{state}  {:<12} {}  {}",
            a.tool.label(),
            p.bold(&trunc(&a.title, 56)),
            p.dim(&format!("{} ago", human((now - a.updated_at).max(0) as u64)))
        );
        let detail =
            [place, a.model.clone().unwrap_or_default(), ctx].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>();
        if !detail.is_empty() {
            let _ = writeln!(out, "           {}", p.dim(&detail.join(" · ")));
        }
    }
    if idle > shown_idle {
        let _ = writeln!(out, "{}", p.dim(&format!("… +{} idle (--all to show)", idle - shown_idle)));
    }
    out
}

fn tokens(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{}k", n / 1000),
        n => n.to_string(),
    }
}

/// Plan limits and token totals, one block per tool.
pub fn usage(list: &[ToolUsage], color: bool) -> String {
    let p = Paint(color);
    let now = vixen_core::monitor::now();
    let mut out = String::new();
    for u in list {
        let plan = u.plan.as_deref().map(|s| format!(" · {s}")).unwrap_or_default();
        let as_of = u
            .limits_as_of
            .map(|t| format!("  limits as of {} ago", human((now - t).max(0) as u64)))
            .unwrap_or_default();
        let _ = writeln!(out, "{}{}{}", p.bold(u.tool.label()), p.dim(&plan), p.dim(&as_of));
        for l in &u.limits {
            let reset = match l.resets_at {
                Some(t) if t <= now => p.dim("reset since"),
                Some(t) => p.dim(&format!("resets in {}", human((t - now) as u64))),
                None => String::new(),
            };
            let pct = format!("{:>3.0}%", l.percent);
            let pct = if l.percent >= 90.0 {
                p.red(&pct)
            } else if l.percent >= 70.0 {
                p.yellow(&pct)
            } else {
                p.green(&pct)
            };
            let _ = writeln!(out, "  {:<18} {} {pct}  {reset}", l.label, bar(l.percent / 100.0, 12));
        }
        let _ = writeln!(
            out,
            "  {}\n",
            p.dim(&format!(
                "{} tokens last 5h · {} today · {} sessions today",
                tokens(u.tokens_5h),
                tokens(u.tokens_today),
                u.sessions_today
            ))
        );
    }
    out
}

/// Headline, then one line per note: `● train-vit  loss plateaued at 0.31`.
pub fn insights(i: &Insights, color: bool) -> String {
    let p = Paint(color);
    let mut out = format!("{}\n", p.bold(&i.headline));
    let width = i.notes.iter().map(|n| n.label.chars().count()).max().unwrap_or(0).min(28);
    for n in &i.notes {
        let dot = match n.tone {
            Tone::Bad => p.red("●"),
            Tone::Warn => p.yellow("●"),
            Tone::Ok => p.green("●"),
        };
        let label = format!("{:<width$}", trunc(&n.label, width));
        let _ = writeln!(out, "  {dot} {}  {}", p.bold(&label), n.text);
    }
    if let Some(m) = &i.model {
        let _ = writeln!(out, "{}", p.dim(&format!("written by {m} via Ollama")));
    }
    out
}
