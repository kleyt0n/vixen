//! Forwards snapshots to the UI and turns transitions into events: macOS notifications,
//! and messages for the floating fox (job started / finished / failed, plan limits).

use crate::AppState;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::watch;
use vixen_core::{AgentState, JobState, Snapshot, Tool, control};

#[derive(Default)]
struct Seen {
    /// target → job id → (name, state) in the previous snapshot.
    jobs: HashMap<String, HashMap<String, (String, JobState)>>,
    /// Jobs already warned about exceeding their wall-time.
    warned: HashSet<(String, String)>,
    /// Agent states in the previous snapshot.
    agents: HashMap<(Tool, String), AgentState>,
    /// Limit windows already alerted: (tool, label, resets_at).
    limits: HashSet<(Tool, String, Option<i64>)>,
}

/// A message for the floating fox.
#[derive(Clone, Serialize)]
struct FoxEvent {
    /// `start` · `done` · `failed` · `limit`
    kind: &'static str,
    title: String,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    percent: Option<f64>,
}

pub async fn forward(app: AppHandle, mut rx: watch::Receiver<Snapshot>) {
    let mut seen = Seen::default();
    loop {
        let snap = rx.borrow_and_update().clone();
        let _ = app.emit("snapshot", &snap);
        crate::tray::update(&app, &snap);
        jobs(&app, &snap, &mut seen);
        agents(&app, &snap, &mut seen);
        limits(&app, &snap, &mut seen);
        if rx.changed().await.is_err() {
            break; // monitor replaced or stopped
        }
    }
}

fn prefs(app: &AppHandle) -> crate::prefs::Prefs {
    app.state::<AppState>().prefs.lock().unwrap().clone()
}

fn fox(app: &AppHandle, e: FoxEvent) {
    let p = prefs(app);
    let wanted = match e.kind {
        "start" => p.events.started,
        "done" => p.events.finished,
        "failed" => p.events.failed,
        _ => p.events.limits,
    };
    if p.float_button && wanted {
        let _ = app.emit_to("float", "float-event", e);
    }
}

/// One message for a batch: "train-vit started" or "12 jobs started".
fn batch(kind: &'static str, verb: &str, target: &str, items: &[(String, String)]) -> FoxEvent {
    match items {
        [(id, name)] => {
            FoxEvent { kind, title: format!("{name} {verb}"), detail: format!("{target} · job {id}"), percent: None }
        }
        _ => {
            let mut names: Vec<&str> = items.iter().map(|(_, n)| n.as_str()).collect();
            names.sort_unstable();
            names.dedup();
            FoxEvent {
                kind,
                title: format!("{} jobs {verb}", items.len()),
                detail: format!("{target} · {}", names.join(", ")),
                percent: None,
            }
        }
    }
}

fn jobs(app: &AppHandle, snap: &Snapshot, seen: &mut Seen) {
    for t in snap.targets.iter().filter(|t| t.ok) {
        let now: HashMap<String, (String, JobState)> =
            t.jobs.iter().map(|j| (j.id.clone(), (j.name.clone(), j.state))).collect();
        if let Some(before) = seen.jobs.get(&t.name) {
            let queued = |s: JobState| matches!(s, JobState::Pending | JobState::Held);
            let mut started = Vec::new();
            let mut failed = Vec::new();
            for (id, (name, state)) in &now {
                let prev = before.get(id).map(|(_, s)| *s);
                if *state == JobState::Running && prev.is_none_or(queued) {
                    started.push((id.clone(), name.clone()));
                }
                if *state == JobState::Failed && prev != Some(JobState::Failed) {
                    failed.push((id.clone(), name.clone()));
                }
            }
            // Running jobs that left the queue: ask the scheduler how they ended.
            let gone: Vec<(String, String)> = before
                .iter()
                .filter(|(id, (_, s))| s.is_active() && !now.contains_key(*id))
                .map(|(id, (n, _))| (id.clone(), n.clone()))
                .collect();

            started.sort();
            if !started.is_empty() {
                fox(app, batch("start", "started", &t.name, &started));
            }
            if !failed.is_empty() {
                let e = batch("failed", "failed", &t.name, &failed);
                notify(app, &e.title, &e.detail);
                fox(app, e);
            }
            if !gone.is_empty() {
                let (app, target) = (app.clone(), t.name.clone());
                tauri::async_runtime::spawn(async move { finished(app, target, gone).await });
            }
        }
        for j in t.jobs.iter().filter(|j| j.eta.as_ref().is_some_and(|e| e.exceeds_limit)) {
            if seen.warned.insert((t.name.clone(), j.id.clone())) {
                notify(
                    app,
                    &format!("{} will hit its time limit", j.name),
                    &format!("{} · job {} — ETA exceeds wall-time", t.name, j.id),
                );
            }
        }
        seen.jobs.insert(t.name.clone(), now);
    }
}

/// Jobs gone from the queue: completed or failed, per the scheduler's accounting.
async fn finished(app: AppHandle, target: String, gone: Vec<(String, String)>) {
    let cfg_target = {
        let state = app.state::<AppState>();
        let cfg = state.config.lock().unwrap();
        cfg.as_ref().and_then(|c| c.target(&target).ok().cloned())
    };
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for (id, name) in gone {
        let outcome = match &cfg_target {
            Some(t) => control::outcome(t, &id).await,
            None => None,
        };
        match outcome {
            Some(o) if !o.ok => bad.push((id, name, o.detail)),
            _ => ok.push((id, name)),
        }
    }
    if !ok.is_empty() {
        let e = batch("done", "finished", &target, &ok);
        notify(&app, &e.title, &e.detail);
        fox(&app, e);
    }
    if let [(id, name, detail)] = bad.as_slice() {
        notify(&app, &format!("{name} failed"), &format!("{target} · job {id} · {detail}"));
        fox(
            &app,
            FoxEvent {
                kind: "failed",
                title: format!("{name} failed"),
                detail: format!("{target} · job {id} · {detail}"),
                percent: None,
            },
        );
    } else if !bad.is_empty() {
        let items: Vec<(String, String)> = bad.iter().map(|(i, n, _)| (i.clone(), n.clone())).collect();
        let e = batch("failed", "failed", &target, &items);
        notify(&app, &e.title, &e.detail);
        fox(&app, e);
    }
}

/// An agent that was working and is now waiting for you.
fn agents(app: &AppHandle, snap: &Snapshot, seen: &mut Seen) {
    let now: HashMap<(Tool, String), AgentState> =
        snap.agents.iter().map(|a| ((a.tool, a.id.clone()), a.state)).collect();
    for a in &snap.agents {
        let before = seen.agents.get(&(a.tool, a.id.clone()));
        if a.state == AgentState::Waiting && before == Some(&AgentState::Working) {
            let place = a.project.as_deref().unwrap_or("");
            notify(app, &format!("{} is waiting for you", a.tool.label()), &format!("{} · {place}", a.title));
        }
    }
    seen.agents = now;
}

/// Plan limits at or above the threshold: once per limit window.
fn limits(app: &AppHandle, snap: &Snapshot, seen: &mut Seen) {
    let threshold = prefs(app).limit_alert;
    if threshold == 0 {
        return;
    }
    let now = vixen_core::monitor::now();
    for u in &snap.usage {
        for l in &u.limits {
            let current = l.resets_at.is_none_or(|r| r > now);
            if !current || l.percent < f64::from(threshold) {
                continue;
            }
            if !seen.limits.insert((u.tool, l.label.clone(), l.resets_at)) {
                continue;
            }
            let reset = l
                .resets_at
                .map(|r| format!("Resets in {}", vixen_core::timefmt::human((r - now).max(0) as u64)))
                .unwrap_or_default();
            let title = format!("{} {} at {:.0}%", u.tool.label(), l.label.to_lowercase(), l.percent);
            let detail = [reset, format!("alert at {threshold}%")]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            notify(app, &title, &detail);
            fox(app, FoxEvent { kind: "limit", title, detail, percent: Some(l.percent) });
        }
    }
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}
