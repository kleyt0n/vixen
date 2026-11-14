//! Concurrent poll loop: one task per target, each on its own interval,
//! publishing a merged [`Snapshot`] through a `watch` channel.

use crate::config::{Config, Target};
use crate::exec::Exec;
use crate::model::{JobState, Snapshot, TargetStatus};
use crate::progress::EtaTracker;
use crate::sources;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, watch};

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
        let state = Arc::new(Mutex::new(
            targets
                .iter()
                .map(|t| TargetStatus { name: t.name.clone(), kind: t.describe(), ..Default::default() })
                .collect::<Vec<_>>(),
        ));
        let (tx, rx) = watch::channel(Snapshot { taken_at: now(), targets: state.lock().unwrap().clone() });
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
                        s[i] = status;
                        tx.send_replace(Snapshot { taken_at: now(), targets: s.clone() });
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(every) => {}
                        _ = notify.notified() => {}
                    }
                }
            }));
        }
        Self { rx, wake, tasks }
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
        let mut targets = Vec::with_capacity(handles.len());
        for h in handles {
            targets.push(h.await.unwrap_or_default());
        }
        Snapshot { taken_at: now(), targets }
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

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
