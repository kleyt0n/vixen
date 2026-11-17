//! Concurrent poll loop: one task per target, each on its own interval,
//! publishing a merged [`Snapshot`] through a `watch` channel.

use crate::agents::Scanner;
use crate::config::{Config, Target};
use crate::exec::Exec;
use crate::insights::Engine;
use crate::model::{Agent, Insights, JobState, Snapshot, TargetStatus, ToolUsage};
use crate::progress::EtaTracker;
use crate::sources;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, watch};

/// Latest state of every target and agent, merged into each published snapshot.
struct Shared {
    targets: Vec<TargetStatus>,
    agents: Vec<Agent>,
    usage: Vec<ToolUsage>,
    insights: Option<Insights>,
}

impl Shared {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            taken_at: now(),
            targets: self.targets.clone(),
            agents: self.agents.clone(),
            usage: self.usage.clone(),
            insights: self.insights.clone(),
        }
    }

    /// Avoid republishing (and re-rendering) every few seconds when nothing changed.
    fn agents_changed(&self, new: &[Agent]) -> bool {
        let key = |a: &Agent| (a.tool, a.id.clone(), a.state, a.updated_at, a.title.clone());
        self.agents.len() != new.len() || self.agents.iter().zip(new).any(|(a, b)| key(a) != key(b))
    }
}

/// Handle to one target's poll task.
struct Waker {
    name: String,
    notify: Arc<Notify>,
    /// True while a poll is in flight, so stale-refreshes don't queue a second one.
    busy: Arc<AtomicBool>,
}

pub struct Monitor {
    rx: watch::Receiver<Snapshot>,
    wake: Vec<Waker>,
    /// Wakes the insights task for a summary now (when insights are on).
    summarize: Option<Arc<Notify>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.tasks.iter().for_each(|t| t.abort());
    }
}

impl Monitor {
    /// Spawn pollers for all enabled targets (requires a Tokio runtime).
    pub fn start(cfg: Config) -> Self {
        let cfg = Arc::new(cfg);
        let targets: Vec<Target> = cfg.targets.iter().filter(|t| t.enabled).cloned().collect();
        let state = Arc::new(Mutex::new(Shared {
            targets: targets
                .iter()
                .map(|t| TargetStatus { name: t.name.clone(), kind: t.describe(), ..Default::default() })
                .collect(),
            agents: Vec::new(),
            usage: Vec::new(),
            insights: cfg.insights.enabled.then(Insights::default),
        }));
        let (tx, rx) = watch::channel(state.lock().unwrap().snapshot());
        let tx = Arc::new(tx);

        let mut wake = Vec::new();
        let mut tasks = Vec::new();
        for (i, t) in targets.into_iter().enumerate() {
            let notify = Arc::new(Notify::new());
            let busy = Arc::new(AtomicBool::new(false));
            wake.push(Waker { name: t.name.clone(), notify: notify.clone(), busy: busy.clone() });
            let (cfg, state, tx) = (cfg.clone(), state.clone(), tx.clone());
            tasks.push(tokio::spawn(async move {
                let mut poller = Poller::new(t, cfg.tail_bytes);
                let every = Duration::from_secs(poller.target.interval(&cfg));
                loop {
                    busy.store(true, Ordering::Relaxed);
                    let status = poller.poll().await;
                    busy.store(false, Ordering::Relaxed);
                    {
                        let mut s = state.lock().unwrap();
                        s.targets[i] = status;
                        tx.send_replace(s.snapshot());
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(every) => {}
                        _ = notify.notified() => {}
                    }
                }
            }));
        }

        // Agents: local files, cheap to rescan, so on their own short interval.
        if cfg.agents.enabled {
            let (cfg, state, tx) = (cfg.clone(), state.clone(), tx.clone());
            tasks.push(tokio::spawn(async move {
                let mut scanner = Scanner::default();
                let every = Duration::from_secs(cfg.agents.interval.max(1));
                loop {
                    let scan = scanner.scan(&cfg.agents).await;
                    {
                        let mut s = state.lock().unwrap();
                        if s.agents_changed(&scan.agents) || s.usage != scan.usage {
                            s.agents = scan.agents;
                            s.usage = scan.usage;
                            tx.send_replace(s.snapshot());
                        }
                    }
                    tokio::time::sleep(every).await;
                }
            }));
        }
        // Insights: a summary every `interval` when something changed, or on request.
        let mut summarize = None;
        if cfg.insights.enabled {
            let notify = Arc::new(Notify::new());
            summarize = Some(notify.clone());
            let (cfg, state, tx) = (cfg.clone(), state.clone(), tx.clone());
            tasks.push(tokio::spawn(async move {
                let mut engine = Engine::default();
                let every = Duration::from_secs(cfg.insights.interval.max(60));
                // Give targets and agents a moment for their first poll.
                let mut forced = tokio::select! {
                    _ = tokio::time::sleep(INSIGHTS_WARMUP) => false,
                    _ = notify.notified() => true,
                };
                loop {
                    let snap = state.lock().unwrap().snapshot();
                    if forced || engine.due(&snap) {
                        let publish = |ins: Insights| {
                            let mut s = state.lock().unwrap();
                            s.insights = Some(ins);
                            tx.send_replace(s.snapshot());
                        };
                        publish(engine.start());
                        publish(engine.run(&snap, &cfg).await);
                    }
                    forced = tokio::select! {
                        _ = tokio::time::sleep(every) => false,
                        _ = notify.notified() => true,
                    };
                }
            }));
        }
        Self { rx, wake, summarize, tasks }
    }

    /// Write a new insights summary now (no-op when insights are off).
    pub fn summarize(&self) {
        if let Some(n) = &self.summarize {
            n.notify_one();
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.rx.clone()
    }

    pub fn snapshot(&self) -> Snapshot {
        self.rx.borrow().clone()
    }

    /// Poll now: one target, or all when `None`.
    pub fn refresh(&self, target: Option<&str>) {
        for w in &self.wake {
            if target.is_none_or(|t| t == w.name) {
                w.notify.notify_one();
            }
        }
    }

    /// Poll targets whose data is older than `max_age_s` and that aren't already polling.
    /// Cheap to call whenever the user looks at the app.
    pub fn refresh_stale(&self, max_age_s: i64) {
        let snap = self.rx.borrow();
        let now = now();
        for w in &self.wake {
            let fresh = snap.targets.iter().any(|t| t.name == w.name && now - t.updated_at <= max_age_s);
            if !fresh && !w.busy.load(Ordering::Relaxed) {
                w.notify.notify_one();
            }
        }
    }

    /// Poll every enabled target once, concurrently.
    pub async fn once(cfg: &Config) -> Snapshot {
        let handles: Vec<_> = cfg
            .targets
            .iter()
            .filter(|t| t.enabled)
            .map(|t| {
                let mut poller = Poller::new(t.clone(), cfg.tail_bytes);
                tokio::spawn(async move { poller.poll().await })
            })
            .collect();
        let scan = Scanner::default().scan(&cfg.agents).await;
        let mut targets = Vec::with_capacity(handles.len());
        for h in handles {
            targets.push(h.await.unwrap_or_default());
        }
        Snapshot { taken_at: now(), targets, agents: scan.agents, usage: scan.usage, insights: None }
    }
}

/// Polls one target and carries state between polls (SSH mux, ETA history, last good data).
pub struct Poller {
    pub target: Target,
    exec: Exec,
    tail_bytes: u32,
    eta: EtaTracker,
    last: TargetStatus,
}

impl Poller {
    pub fn new(target: Target, tail_bytes: u32) -> Self {
        let exec = Exec::new(target.host.clone(), target.ssh_args.clone());
        let last = TargetStatus { name: target.name.clone(), kind: target.describe(), ..Default::default() };
        Self { target, exec, tail_bytes, eta: EtaTracker::default(), last }
    }

    pub async fn poll(&mut self) -> TargetStatus {
        let started = Instant::now();
        match sources::poll(&self.target, &self.exec, self.tail_bytes).await {
            Ok(mut p) => {
                let now = p.clock.map(|c| c.now).unwrap_or_else(now);
                let mut live = HashSet::new();
                for job in &mut p.jobs {
                    live.insert(job.id.clone());
                    if let Some(prog) = &job.progress {
                        job.eta = self.eta.estimate(&job.id, now, prog, job.elapsed_s, job.time_left_s());
                    }
                }
                self.eta.retain(&live);
                p.jobs.sort_by_key(|j| {
                    let rank = match j.state {
                        JobState::Running | JobState::Completing => 0,
                        JobState::Suspended => 1,
                        JobState::Pending => 2,
                        JobState::Held => 3,
                        _ => 4,
                    };
                    (rank, j.est_start_at.unwrap_or(i64::MAX), j.started_at.unwrap_or(i64::MAX))
                });
                self.last = TargetStatus {
                    name: self.target.name.clone(),
                    kind: self.target.describe(),
                    ok: true,
                    error: None,
                    updated_at: now,
                    latency_ms: started.elapsed().as_millis() as u64,
                    host: p.host,
                    gpus: p.gpus,
                    jobs: p.jobs,
                };
            }
            Err(e) => {
                // Keep the last good data, flagged as stale.
                self.last.ok = false;
                self.last.error = Some(format!("{e:#}"));
                self.last.latency_ms = started.elapsed().as_millis() as u64;
            }
        }
        self.last.clone()
    }
}

/// Delay before the first summary, so it sees polled targets.
const INSIGHTS_WARMUP: Duration = Duration::from_secs(15);

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
