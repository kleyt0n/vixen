//! Vixen for macOS: a window + menu-bar item over `vixen-core`.
//!
//! The Rust side owns polling; the UI receives `snapshot` events and calls a
//! handful of commands (refresh, job actions, logs, config).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod tray;
mod watch;

use serde::Serialize;
use std::sync::Mutex;
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};
use tauri_plugin_window_state::{AppHandleExt, StateFlags};
use vixen_core::{Action, Config, Monitor, Snapshot, Target, TargetStatus, config, control};

#[derive(Default)]
pub struct AppState {
    monitor: Mutex<Option<Monitor>>,
    config: Mutex<Option<Config>>,
    error: Mutex<Option<String>>,
    /// When the menu-bar panel last lost focus (so the tray click that caused it doesn't reopen it).
    panel_hidden_at: Mutex<Option<Instant>>,
    /// When the panel was last shown: activation can blur it for a moment right after.
    panel_shown_at: Mutex<Option<Instant>>,
}

type Res<T> = Result<T, String>;

fn target(state: &State<AppState>, name: &str) -> Res<vixen_core::Target> {
    let cfg = state.config.lock().unwrap();
    let cfg = cfg.as_ref().ok_or("no config loaded")?;
    cfg.target(name).cloned().map_err(|e| e.to_string())
}

#[tauri::command]
fn snapshot(state: State<AppState>) -> Res<Snapshot> {
    if let Some(e) = state.error.lock().unwrap().clone() {
        return Err(e);
    }
    Ok(state.monitor.lock().unwrap().as_ref().map(Monitor::snapshot).unwrap_or_default())
}

#[tauri::command]
fn refresh(state: State<AppState>, target: Option<String>) {
    if let Some(m) = state.monitor.lock().unwrap().as_ref() {
        m.refresh(target.as_deref());
    }
}

#[tauri::command]
async fn job_action(state: State<'_, AppState>, target: String, id: String, action: Action) -> Res<String> {
    let t = self::target(&state, &target)?;
    let out = control::act(&t, &id, action).await.map_err(|e| format!("{e:#}"))?;
    refresh(state, Some(target));
    Ok(out)
}

#[tauri::command]
async fn job_logs(
    state: State<'_, AppState>,
    target: String,
    id: String,
    path: Option<String>,
    lines: u32,
) -> Res<String> {
    let t = self::target(&state, &target)?;
    control::logs(&t, &id, path, lines).await.map_err(|e| format!("{e:#}"))
}

#[derive(Serialize)]
struct Settings {
    path: String,
    /// False when no config file exists yet; `config` is then the bundled example.
    exists: bool,
    config: Config,
    ssh_hosts: Vec<String>,
}

#[tauri::command]
fn settings() -> Res<Settings> {
    let path = Config::path();
    let exists = path.exists();
    let config = if exists { Config::load() } else { Config::parse(config::EXAMPLE) }.map_err(|e| format!("{e:#}"))?;
    Ok(Settings { path: path.display().to_string(), exists, config, ssh_hosts: config::ssh_hosts() })
}

#[tauri::command]
fn save_settings(app: AppHandle, config: Config) -> Res<Config> {
    let config = config.normalized();
    config.save().map_err(|e| format!("{e:#}"))?;
    start(&app);
    Ok(config)
}

/// Poll a (possibly unsaved) target once.
#[tauri::command]
async fn test_target(target: Target) -> Res<TargetStatus> {
    let mut cfg = Config::parse("").map_err(|e| e.to_string())?;
    cfg.targets = vec![Target { enabled: true, ..target }];
    let cfg = cfg.normalized();
    cfg.validate().map_err(|e| format!("{e:#}"))?;
    Monitor::once(&cfg).await.targets.pop().ok_or_else(|| "no result".into())
}

#[tauri::command]
fn open_config() -> Res<()> {
    let (path, _) = Config::init().map_err(|e| e.to_string())?;
    std::process::Command::new("open").arg("-t").arg(&path).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

/// Buttons in the menu-bar panel.
#[tauri::command]
fn panel_action(app: AppHandle, action: String, target: Option<String>) {
    if let Some(p) = app.get_webview_window(tray::PANEL) {
        let _ = p.hide();
    }
    match action.as_str() {
        "open" => {
            show_main(&app);
            if let Some(t) = target {
                let _ = app.emit_to("main", "focus-target", t);
            }
        }
        "settings" => show_settings(&app),
        "quit" => app.exit(0),
        _ => {}
    }
}

#[tauri::command]
fn reload(app: AppHandle) {
    start(&app);
}

/// (Re)load the config and (re)start polling.
pub fn start(app: &AppHandle) {
    let state = app.state::<AppState>();
    let _ = state.monitor.lock().unwrap().take(); // stops old pollers
    match Config::load() {
        Ok(cfg) => {
            *state.error.lock().unwrap() = None;
            *state.config.lock().unwrap() = Some(cfg.clone());
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let monitor = Monitor::start(cfg);
                let rx = monitor.subscribe();
                *app.state::<AppState>().monitor.lock().unwrap() = Some(monitor);
                watch::forward(app, rx).await;
            });
        }
        Err(e) => {
            let msg = format!("{e:#}");
            *state.error.lock().unwrap() = Some(msg.clone());
            let _ = app.emit("config-error", msg);
        }
    }
}

pub fn show_settings(app: &AppHandle) {
    show_main(app);
    let _ = app.emit("open-settings", ());
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    refresh_stale(app);
}

/// Data older than this is re-polled when the user opens the window or panel.
const STALE_ON_VIEW_S: i64 = 60;

/// Polls run every few minutes; re-poll what's stale whenever someone looks.
pub fn refresh_stale(app: &AppHandle) {
    if let Some(m) = app.state::<AppState>().monitor.lock().unwrap().as_ref() {
        m.refresh_stale(STALE_ON_VIEW_S);
    }
}

/// Main window size, position and zoom survive restarts; the panel is anchored to the tray.
const WINDOW_STATE: StateFlags = StateFlags::SIZE.union(StateFlags::POSITION).union(StateFlags::MAXIMIZED);

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(WINDOW_STATE)
                .with_denylist(&[tray::PANEL])
                .build(),
        )
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            snapshot,
            refresh,
            job_action,
            job_logs,
            settings,
            save_settings,
            test_target,
            open_config,
            panel_action,
            reload
        ])
        .setup(|app| {
            tray::build(app.handle())?;
            start(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Closing hides to the menu bar; quit from the tray menu or ⌘Q.
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.app_handle().save_window_state(WINDOW_STATE);
                let _ = window.hide();
            }
            WindowEvent::Focused(true) if window.label() == "main" => refresh_stale(window.app_handle()),
            WindowEvent::Focused(false) if window.label() == tray::PANEL => {
                let state = window.state::<AppState>();
                let shown = *state.panel_shown_at.lock().unwrap();
                // Activating the app makes the main window key and steals focus; take it back.
                if shown.is_some_and(|t| t.elapsed().as_millis() < 800) {
                    let _ = window.set_focus();
                    return;
                }
                let _ = window.hide();
                *state.panel_hidden_at.lock().unwrap() = Some(Instant::now());
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("failed to build app")
        .run(|app, event| {
            if let RunEvent::Reopen { .. } = event {
                show_main(app);
            }
        });
}
