//! Forwards snapshots to the UI and turns transitions into notifications.

use std::collections::{HashMap, HashSet};
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::watch;
use vixen_core::Snapshot;

#[derive(Default)]
struct Seen {
    /// target → (job id → name) of active jobs in the previous snapshot.
    active: HashMap<String, HashMap<String, String>>,
    /// Jobs already warned about exceeding their wall-time.
    warned: HashSet<(String, String)>,
}

pub async fn forward(app: AppHandle, mut rx: watch::Receiver<Snapshot>) {
    let mut seen = Seen::default();
    loop {
        let snap = rx.borrow_and_update().clone();
        let _ = app.emit("snapshot", &snap);
        crate::tray::update(&app, &snap);
        notify(&app, &snap, &mut seen);
        if rx.changed().await.is_err() {
            break; // monitor replaced or stopped
        }
    }
}

fn notify(app: &AppHandle, snap: &Snapshot, seen: &mut Seen) {
    for t in snap.targets.iter().filter(|t| t.ok) {
        let now: HashMap<String, String> =
            t.jobs.iter().filter(|j| j.state.is_active()).map(|j| (j.id.clone(), j.name.clone())).collect();
        if let Some(before) = seen.active.get(&t.name) {
            for (id, name) in before.iter().filter(|(id, _)| !now.contains_key(*id)) {
                // Still listed (e.g. requeued) is not a finish.
                if t.jobs.iter().any(|j| &j.id == id) {
                    continue;
                }
                send(app, &format!("{name} finished"), &format!("{} · job {id}", t.name));
            }
        }
        for j in t.jobs.iter().filter(|j| j.eta.as_ref().is_some_and(|e| e.exceeds_limit)) {
            if seen.warned.insert((t.name.clone(), j.id.clone())) {
                send(
                    app,
                    &format!("{} will hit its time limit", j.name),
                    &format!("{} · job {} — ETA exceeds wall-time", t.name, j.id),
                );
            }
        }
        seen.active.insert(t.name.clone(), now);
    }
}

fn send(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}
