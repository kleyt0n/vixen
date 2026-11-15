//! App-only preferences (not shared with the CLI): `<app config dir>/prefs.json`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrayTitle {
    /// Fox icon only.
    Icon,
    /// Running-job count.
    Count,
    /// Count and soonest ETA, e.g. `5 · 2h31`.
    #[default]
    CountEta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub tray_title: TrayTitle,
    /// Always-on-top fox that opens the panel.
    pub float_button: bool,
    /// Which buddy the floating button shows: `kit`, `snow`, `sleepy`, `mini`, `arara`, `squirrel` or `onca`.
    pub float_style: String,
    /// Last position of the floating button (physical pixels).
    pub float_pos: Option<(i32, i32)>,
    /// Alert when any agent plan limit reaches this percent (0 = off).
    pub limit_alert: u8,
    /// Which events the fox announces.
    pub events: Events,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Events {
    pub started: bool,
    pub finished: bool,
    pub failed: bool,
    pub limits: bool,
}

impl Default for Events {
    fn default() -> Self {
        Self { started: true, finished: true, failed: true, limits: true }
    }
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            tray_title: TrayTitle::default(),
            float_button: false,
            float_style: "kit".into(),
            float_pos: None,
            limit_alert: 80,
            events: Events::default(),
        }
    }
}

fn path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("prefs.json"))
}

pub fn load(app: &AppHandle) -> Prefs {
    path(app).and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(app: &AppHandle, prefs: &Prefs) {
    let Some(p) = path(app) else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(prefs) {
        let _ = std::fs::write(p, json);
    }
}
