//! Progress extraction from training logs, and ETA estimation.
//!
//! Recognised forms (case-insensitive, last occurrence wins):
//! * tqdm bars            `45%|████▌ | 450/1000 [01:23<01:40, 5.40it/s]`
//! * epoch counters       `Epoch 3/10`, `[Epoch 136/310]`, `epoch 3 of 10`
//! * step counters        `step 150/500`, `iter: 150/500`, `batch 3/40`
//! * throughput           `5.2 it/s`, `1.8s/it`
//!
//! Nested epoch + step counters are combined into one overall fraction.

use crate::model::{Eta, EtaMethod, Progress};
use regex::Regex;
use std::collections::{HashMap, VecDeque};
use std::sync::LazyLock;

static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").unwrap());
static EPOCH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bepoch\s*[:=#]?\s*\[?\s*(\d+)\s*(?:/|of)\s*(\d+)").unwrap());
static STEP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:global[_ ]?)?(?:step|iter(?:ation)?|batch|it|update)s?\s*[:=#]?\s*\[?\s*(\d+)\s*(?:/|of)\s*(\d+)",
    )
    .unwrap()
});
static TQDM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+)/(\d+)\s*\[(?:[\d:]+)<([\d:]+|\?)").unwrap());
static RATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+(?:\.\d+)?)\s*(it/s|s/it)\b").unwrap());

pub fn strip_ansi(s: &str) -> String {
    ANSI.replace_all(s, "").into_owned()
}

/// Clean log tail: ANSI removed, carriage-return updates split, blank lines dropped.
pub fn clean_lines(raw: &str) -> Vec<String> {
    strip_ansi(raw).split(['\n', '\r']).map(str::trim_end).filter(|l| !l.trim().is_empty()).map(String::from).collect()
}

#[derive(Debug, Clone, Copy)]
struct Counter {
    cur: u64,
    total: u64,
}

impl Counter {
    fn from(c: &regex::Captures) -> Option<Self> {
        let cur = c[1].parse().ok()?;
        let total: u64 = c[2].parse().ok()?;
        (total > 0 && cur <= total).then_some(Self { cur, total })
    }
}

/// Parse the most recent progress information from log lines (oldest first).
pub fn parse(lines: &[String]) -> Option<Progress> {
    let mut step: Option<Counter> = None;
    let mut tool_left: Option<u64> = None;
    let mut rate: Option<f64> = None;
    let mut latest_epoch: Option<Counter> = None; // newest epoch line anywhere
    let mut epoch: Option<Counter> = None; // epoch line at or above the newest step line

    for line in lines.iter().rev().take(200) {
        if step.is_none() {
            if let Some(c) = TQDM.captures(line) {
                step = Counter::from(&c);
                tool_left = crate::timefmt::parse_duration(&c[3]);
            } else if let Some(c) = STEP.captures(line) {
                step = Counter::from(&c);
            }
            if step.is_some() {
                rate = RATE.captures(line).and_then(|r| {
                    let v: f64 = r[1].parse().ok()?;
                    (v > 0.0).then(|| if &r[2] == "s/it" { 1.0 / v } else { v })
                });
            }
        }
        if let Some(e) = EPOCH.captures(line).and_then(|c| Counter::from(&c)) {
            latest_epoch.get_or_insert(e);
            if step.is_some() {
                epoch = Some(e);
                break;
            }
        }
    }
    // A newer epoch has begun but hasn't logged a step yet ("Epoch 141/310" header,
    // or "Epoch 140/310 done" summary): count the finished epochs, keep the step size.
    if let (Some(le), Some(e), Some(s)) = (latest_epoch, epoch, step)
        && le.cur > e.cur
    {
        epoch = Some(le);
        step = Some(Counter { cur: 0, total: s.total });
    }
    let epoch = epoch.or(latest_epoch);

    let (fraction, label, done, total) = match (epoch, step) {
        (Some(e), Some(s)) => {
            let done_epochs = e.cur.saturating_sub(1);
            let done = done_epochs * s.total + s.cur;
            let total = e.total * s.total;
            (
                done as f64 / total as f64,
                format!("epoch {}/{} · step {}/{}", e.cur, e.total, s.cur, s.total),
                done,
                total,
            )
        }
        (Some(e), None) => {
            (e.cur.saturating_sub(1) as f64 / e.total as f64, format!("epoch {}/{}", e.cur, e.total), 0, 0)
        }
        (None, Some(s)) => (s.cur as f64 / s.total as f64, format!("step {}/{}", s.cur, s.total), s.cur, s.total),
        (None, None) => return None,
    };

    let throughput_remaining_s = match rate {
        Some(r) if total > 0 => Some(((total - done) as f64 / r).round() as u64),
        _ => None,
    };
    Some(Progress {
        fraction: fraction.clamp(0.0, 1.0),
        label,
        it_per_s: rate,
        // tqdm's own estimate only covers its bar; meaningless when nested in epochs.
        tool_remaining_s: if epoch.is_none() { tool_left } else { None },
        throughput_remaining_s,
    })
}

/// Tracks progress across polls to derive an observed completion rate.
#[derive(Debug, Default)]
pub struct EtaTracker {
    history: HashMap<String, VecDeque<(i64, f64)>>,
}

const WINDOW_S: i64 = 30 * 60;
const MIN_SPAN_S: i64 = 45;

impl EtaTracker {
    pub fn estimate(
        &mut self,
        key: &str,
        now: i64,
        p: &Progress,
        elapsed_s: Option<u64>,
        limit_left_s: Option<u64>,
    ) -> Option<Eta> {
        let hist = self.history.entry(key.to_string()).or_default();
        if hist.back().is_some_and(|&(_, f)| p.fraction + 1e-9 < f) {
            hist.clear(); // restarted
        }
        if hist.back().is_none_or(|&(_, f)| p.fraction > f) {
            hist.push_back((now, p.fraction));
        }
        while hist.len() > 2 && hist.front().is_some_and(|&(t, _)| now - t > WINDOW_S) {
            hist.pop_front();
        }

        let observed = hist.front().and_then(|&(t0, f0)| {
            let span = now - t0;
            let df = p.fraction - f0;
            (span >= MIN_SPAN_S && df > 0.0).then(|| ((1.0 - p.fraction) * span as f64 / df) as u64)
        });

        let (remaining_s, method) = if p.fraction >= 1.0 {
            (0, EtaMethod::Observed)
        } else if let Some(r) = observed {
            (r, EtaMethod::Observed)
        } else if let Some(r) = p.throughput_remaining_s {
            (r, EtaMethod::Throughput)
        } else if let Some(r) = p.tool_remaining_s {
            (r, EtaMethod::Tool)
        } else {
            let e = elapsed_s.filter(|_| p.fraction >= 0.01)?;
            ((e as f64 * (1.0 - p.fraction) / p.fraction) as u64, EtaMethod::Elapsed)
        };

        Some(Eta {
            remaining_s,
            finish_at: now + remaining_s as i64,
            method,
            exceeds_limit: limit_left_s.is_some_and(|left| remaining_s > left),
        })
    }

    /// Forget jobs that are no longer reported.
    pub fn retain(&mut self, live: &std::collections::HashSet<String>) {
        self.history.retain(|k, _| live.contains(k));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        clean_lines(s)
    }

    #[test]
    fn nested_epoch_step() {
        let log = "17:16:02 | INFO | saved\n[Epoch 136/310] step 100/500 (5.1 it/s)\n\
                   [2026-11-14T17:16:26.423] error: _handle_getgr: incomplete data\n\
                   [Epoch 136/310] step 150/500 (5.2 it/s)\n";
        let p = parse(&lines(log)).unwrap();
        let expect = (135.0 * 500.0 + 150.0) / (310.0 * 500.0);
        assert!((p.fraction - expect).abs() < 1e-9);
        assert_eq!(p.label, "epoch 136/310 · step 150/500");
        assert_eq!(p.it_per_s, Some(5.2));
        let left = (310 * 500 - (135 * 500 + 150)) as f64 / 5.2;
        assert_eq!(p.throughput_remaining_s, Some(left.round() as u64));
    }

    #[test]
    fn tqdm_bar() {
        let log = "Training:  45%|████▌     | 450/1000 [01:23<01:40,  5.40it/s]\r\
                   Training:  46%|████▌     | 460/1000 [01:25<01:38,  5.41it/s]";
        let p = parse(&lines(log)).unwrap();
        assert!((p.fraction - 0.46).abs() < 1e-9);
        assert_eq!(p.tool_remaining_s, Some(98));
        assert_eq!(p.it_per_s, Some(5.41));
    }

    #[test]
    fn epoch_line_above_tqdm() {
        let log = "Epoch 2/4\n 10/100 [00:02<00:18, 4.9it/s]";
        let p = parse(&lines(log)).unwrap();
        assert!((p.fraction - 110.0 / 400.0).abs() < 1e-9);
        assert_eq!(p.tool_remaining_s, None);
    }

    #[test]
    fn epoch_summary_after_steps() {
        let log = "[Epoch 140/310] step 450/500 (2.6 it/s)\n[Epoch 140/310] step 500/500 (2.7 it/s)\n\
                   [Epoch 140/310] done in 183.1s\n17:26:20 | INFO | saved";
        let p = parse(&lines(log)).unwrap();
        assert_eq!(p.label, "epoch 140/310 · step 500/500");
        assert_eq!(p.it_per_s, Some(2.7));
        let p = parse(&lines(&format!("{log}\n[Epoch 141/310] starting"))).unwrap();
        assert_eq!(p.label, "epoch 141/310 · step 0/500");
        assert!((p.fraction - 140.0 / 310.0).abs() < 1e-9);
    }

    #[test]
    fn seconds_per_iteration_and_ansi() {
        let p = parse(&lines("\x1b[32mstep: 10/20\x1b[0m 2.0s/it")).unwrap();
        assert_eq!(p.it_per_s, Some(0.5));
        assert_eq!(p.throughput_remaining_s, Some(20));
    }

    #[test]
    fn ignores_noise() {
        assert!(parse(&lines("GPU 96% util\ndate 2026/11/14\nloss=0.3")).is_none());
    }

    #[test]
    fn observed_rate_wins_after_window() {
        let mut t = EtaTracker::default();
        let mut p = parse(&lines("step 100/1000 10 it/s")).unwrap();
        let e = t.estimate("j", 0, &p, Some(10), Some(10_000)).unwrap();
        assert_eq!(e.method, EtaMethod::Throughput);
        p.fraction = 0.2; // 10% per 100s → 800s left
        let e = t.estimate("j", 100, &p, Some(110), Some(500)).unwrap();
        assert_eq!(e.method, EtaMethod::Observed);
        assert_eq!(e.remaining_s, 800);
        assert!(e.exceeds_limit);
    }
}
