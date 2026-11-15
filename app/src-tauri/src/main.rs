//! Vixen for macOS: a window + menu-bar item over `vixen-core`.
//!
//! The Rust side owns polling; the UI receives `snapshot` events and calls a
//! handful of commands (refresh, job actions, logs, config).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod prefs;
mod tray;
mod watch;

use prefs::Prefs;
use serde::Serialize;
use std::sync::Mutex;
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, RunEvent, State, WindowEvent};
use tauri_plugin_window_state::{AppHandleExt, StateFlags};
use vixen_core::{Action, Config, Monitor, Snapshot, Target, TargetStatus, Tool, config, control, exec::shq};

#[derive(Default)]
pub struct AppState {
    monitor: Mutex<Option<Monitor>>,
    config: Mutex<Option<Config>>,
    error: Mutex<Option<String>>,
    /// When the menu-bar panel last lost focus (so the tray click that caused it doesn't reopen it).
    panel_hidden_at: Mutex<Option<Instant>>,
    /// When the panel was last shown: activation can blur it for a moment right after.
    panel_shown_at: Mutex<Option<Instant>>,
    prefs: Mutex<Prefs>,
    /// Whether the menu-bar icon currently shows the alert dot.
    tray_alert: Mutex<bool>,
    /// Set while the floating fox's window is enlarged for a message.
    float_expanded: Mutex<Option<Expanded>>,
}

/// How the fox window grew: which way, and by how much (physical pixels).
#[derive(Clone, Copy, serde::Serialize)]
struct Expanded {
    right: bool,
    down: bool,
    #[serde(skip)]
    dx: i32,
    #[serde(skip)]
    dy: i32,
}

/// Fox window size at rest, and with a message bubble (logical pixels).
const FLOAT_SIZE: f64 = 72.0;
const FLOAT_BUBBLE: (f64, f64) = (340.0, 150.0);

const FLOAT: &str = "float";

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

/// Agent session actions: resume in Terminal, open its folder, reveal its transcript, open the app.
#[tauri::command]
fn agent_action(state: State<AppState>, tool: Tool, id: String, action: String) -> Res<String> {
    let snap = state.monitor.lock().unwrap().as_ref().map(Monitor::snapshot).ok_or("not monitoring yet")?;
    let a = snap.agents.into_iter().find(|a| a.tool == tool && a.id == id).ok_or("session no longer listed")?;
    let open = |args: &[&str]| -> Res<()> {
        std::process::Command::new("open").args(args).spawn().map(|_| ()).map_err(|e| e.to_string())
    };
    match action.as_str() {
        "resume" => {
            let cmd = a.resume.ok_or("this tool can't resume from a terminal")?;
            let line = match &a.cwd {
                Some(dir) => format!("cd {} && {cmd}", shq(dir)),
                None => cmd,
            };
            let esc = line.replace('\\', "\\\\").replace('"', "\\\"");
            std::process::Command::new("osascript")
                .args([
                    "-e",
                    &format!("tell application \"Terminal\" to do script \"{esc}\""),
                    "-e",
                    "tell application \"Terminal\" to activate",
                ])
                .spawn()
                .map_err(|e| e.to_string())?;
            Ok(format!("Resuming in Terminal: {line}"))
        }
        "folder" => {
            open(&[a.cwd.as_deref().ok_or("no folder recorded")?])?;
            Ok("Opened folder".into())
        }
        "reveal" => {
            open(&["-R", a.file.as_deref().ok_or("no transcript file")?])?;
            Ok("Revealed transcript".into())
        }
        "app" => {
            let mut args = vec!["-a", "Antigravity"];
            args.extend(a.cwd.as_deref());
            open(&args)?;
            Ok("Opened Antigravity".into())
        }
        other => Err(format!("unknown action {other}")),
    }
}

#[tauri::command]
fn get_prefs(state: State<AppState>) -> Prefs {
    state.prefs.lock().unwrap().clone()
}

/// Apply and persist app preferences (floating button, menu-bar title).
#[tauri::command]
fn set_prefs(app: AppHandle, state: State<AppState>, prefs: Prefs) {
    let prefs = {
        let mut cur = state.prefs.lock().unwrap();
        // The button's position is owned by the app, not the settings form.
        *cur = Prefs { float_pos: cur.float_pos, ..prefs };
        cur.clone()
    };
    prefs::save(&app, &prefs);
    let _ = app.emit("prefs", &prefs);
    apply_float(&app);
    let snap = state.monitor.lock().unwrap().as_ref().map(Monitor::snapshot);
    if let Some(snap) = snap {
        tray::update(&app, &snap);
    }
}

/// Click on the floating button: open the panel next to it.
#[tauri::command]
fn float_click(app: AppHandle) {
    let Some(w) = app.get_webview_window(FLOAT) else { return };
    let (Ok(p), Ok(s)) = (w.outer_position(), w.outer_size()) else { return };
    let anchor = tray::Anchor { x: p.x as f64, y: p.y as f64, w: s.width as f64, h: s.height as f64 };
    tray::toggle_panel_at(&app, anchor);
}

/// Show or hide the floating button per prefs; first show goes to the bottom-right corner.
fn apply_float(app: &AppHandle) {
    let Some(w) = app.get_webview_window(FLOAT) else { return };
    let prefs = app.state::<AppState>().prefs.lock().unwrap().clone();
    if !prefs.float_button {
        let _ = w.hide();
        return;
    }
    let pos = prefs.float_pos.map(|(x, y)| PhysicalPosition::new(x, y)).or_else(|| {
        let m = w.primary_monitor().ok()??;
        let size = w.outer_size().ok()?;
        let margin = (32.0 * m.scale_factor()) as i32;
        Some(PhysicalPosition::new(
            m.position().x + m.size().width as i32 - size.width as i32 - margin,
            m.position().y + m.size().height as i32 - size.height as i32 - margin * 3,
        ))
    });
    if let Some(p) = pos {
        let _ = w.set_position(p);
    }
    let _ = w.show();
}

/// Grow the fox window to fit a message bubble (towards free screen space), or shrink it back
/// keeping the fox where it was. Returns which way it grew so the page can lay out the bubble.
#[tauri::command]
fn float_expand(app: AppHandle, state: State<AppState>, on: bool) -> Option<Expanded> {
    use tauri::PhysicalSize;
    let w = app.get_webview_window(FLOAT)?;
    let scale = w.scale_factor().ok()?;
    let pos = w.outer_position().ok()?;
    let mut exp = state.float_expanded.lock().unwrap();
    if on {
        if exp.is_some() {
            return *exp;
        }
        let size = w.outer_size().ok()?;
        let (nw, nh) = ((FLOAT_BUBBLE.0 * scale) as i32, (FLOAT_BUBBLE.1 * scale) as i32);
        let (dx, dy) = (nw - size.width as i32, nh - size.height as i32);
        let (mut right, mut down) = (false, false);
        if let Ok(Some(m)) = w.current_monitor() {
            right = pos.x - dx < m.position().x;
            down = pos.y - dy < m.position().y + (40.0 * scale) as i32; // keep clear of the menu bar
        }
        let e = Expanded { right, down, dx, dy };
        *exp = Some(e);
        let _ = w.set_size(PhysicalSize::new(nw, nh));
        let _ = w.set_position(PhysicalPosition::new(
            if right { pos.x } else { pos.x - dx },
            if down { pos.y } else { pos.y - dy },
        ));
        Some(e)
    } else {
        let e = exp.take()?;
        let fox = (if e.right { pos.x } else { pos.x + e.dx }, if e.down { pos.y } else { pos.y + e.dy });
        let px = (FLOAT_SIZE * scale) as u32;
        let _ = w.set_size(PhysicalSize::new(px, px));
        let _ = w.set_position(PhysicalPosition::new(fox.0, fox.1));
        state.prefs.lock().unwrap().float_pos = Some(fox);
        None
    }
}

/// Open a web link (https only) in the default browser.
#[tauri::command]
fn open_url(url: String) -> Res<()> {
    if !url.starts_with("https://") {
        return Err("only https links can be opened".into());
    }
    std::process::Command::new("open").arg(&url).spawn().map(|_| ()).map_err(|e| e.to_string())
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
                .with_denylist(&[tray::PANEL, FLOAT])
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
            open_url,
            panel_action,
            agent_action,
            get_prefs,
            set_prefs,
            float_click,
            float_expand,
            reload
        ])
        .setup(|app| {
            *app.state::<AppState>().prefs.lock().unwrap() = prefs::load(app.handle());
            tray::build(app.handle())?;
            apply_float(app.handle());
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
            WindowEvent::Moved(p) if window.label() == FLOAT => {
                let state = window.state::<AppState>();
                // While enlarged for a message the window origin isn't the fox's; collapse saves it.
                if state.float_expanded.lock().unwrap().is_none() {
                    state.prefs.lock().unwrap().float_pos = Some((p.x, p.y));
                }
            }
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
        .run(|app, event| match event {
            RunEvent::Reopen { .. } => show_main(app),
            RunEvent::Exit => prefs::save(app, &app.state::<AppState>().prefs.lock().unwrap()),
            _ => {}
        });
}
