//! Menu-bar item: live summary in the title; a click opens the summary panel.

use std::time::{Duration, Instant};
use tauri::image::Image;
use tauri::tray::{MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, Rect};
use vixen_core::Snapshot;
use vixen_core::timefmt::human;

use crate::prefs::TrayTitle;

const ID: &str = "vixen";
pub const PANEL: &str = "panel";

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    // No native menu: every click opens the panel, which has Open / Settings / Quit.
    TrayIconBuilder::with_id(ID)
        .icon(Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .icon_as_template(true)
        .tooltip("Vixen")
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button_state: MouseButtonState::Up, rect, .. } = e {
                toggle_panel(tray.app_handle(), rect);
            }
        })
        .build(app)?;
    Ok(())
}

/// Physical-pixel rectangle the panel opens next to (tray icon or floating button).
#[derive(Clone, Copy)]
pub struct Anchor {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

fn toggle_panel(app: &AppHandle, icon: Rect) {
    let scale = app.get_webview_window(PANEL).and_then(|w| w.scale_factor().ok()).unwrap_or(2.0);
    let (pos, size) = (icon.position.to_physical::<f64>(scale), icon.size.to_physical::<f64>(scale));
    toggle_panel_at(app, Anchor { x: pos.x, y: pos.y, w: size.width, h: size.height });
}

/// Show the panel centred on `anchor` (below it, or above when there's no room), or hide it.
pub fn toggle_panel_at(app: &AppHandle, anchor: Anchor) {
    let Some(w) = app.get_webview_window(PANEL) else { return };
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
        return;
    }
    // The click that blurred (and hid) the panel arrives right after; don't reopen.
    let hidden = *app.state::<crate::AppState>().panel_hidden_at.lock().unwrap();
    if hidden.is_some_and(|t| t.elapsed() < Duration::from_millis(300)) {
        return;
    }
    let scale = w.scale_factor().unwrap_or(2.0);
    let gap = 6.0 * scale;
    let (width, height) =
        w.outer_size().map(|s| (s.width as f64, s.height as f64)).unwrap_or((600.0 * scale, 480.0 * scale));
    let mut x = anchor.x + anchor.w / 2.0 - width / 2.0;
    let mut y = anchor.y + anchor.h + gap;
    if let Ok(Some(m)) = w.current_monitor().or_else(|_| w.primary_monitor()) {
        let (mx, my) = (m.position().x as f64, m.position().y as f64);
        let (mw, mh) = (m.size().width as f64, m.size().height as f64);
        x = x.clamp(mx + 8.0 * scale, mx + mw - width - 8.0 * scale);
        if y + height > my + mh - 8.0 * scale {
            y = (anchor.y - height - gap).max(my + 8.0 * scale);
        }
    }
    let _ = w.set_position(PhysicalPosition::new(x, y));
    *app.state::<crate::AppState>().panel_shown_at.lock().unwrap() = Some(Instant::now());
    let _ = w.show();
    let _ = w.set_focus();
    let _ = w.emit("panel-shown", ());
    crate::refresh_stale(app);
    // App activation finishes asynchronously and may hand focus to the main window.
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        if w.is_visible().unwrap_or(false) {
            let _ = w.set_focus();
        }
    });
}

/// Compact duration for the menu bar: `2h31`, `42m`, `1d4h`.
fn short(s: u64) -> String {
    let (d, h, m) = (s / 86400, s / 3600 % 24, s / 60 % 60);
    match (d, h) {
        (0, 0) => format!("{}m", m.max(1)),
        (0, _) => format!("{h}h{m:02}"),
        _ => format!("{d}d{h}h"),
    }
}

/// Title (per prefs) and icon (alert dot when a target is unreachable).
pub fn update(app: &AppHandle, snap: &Snapshot) {
    let Some(tray) = app.tray_by_id(ID) else { return };
    let state = app.state::<crate::AppState>();
    let mode = state.prefs.lock().unwrap().tray_title;
    let jobs = snap.targets.iter().flat_map(|t| &t.jobs);
    let running = jobs.clone().filter(|j| j.state.is_active()).count();
    let next = jobs.filter_map(|j| j.eta.as_ref()).map(|e| e.remaining_s).min();
    let down: Vec<&str> = snap
        .targets
        .iter()
        .filter(|t| !t.ok && t.updated_at == 0 && t.error.is_some())
        .map(|t| t.name.as_str())
        .collect();

    let title = match (mode, running) {
        (TrayTitle::Icon, _) | (_, 0) => String::new(),
        (TrayTitle::Count, n) => n.to_string(),
        (TrayTitle::CountEta, n) => match next {
            Some(s) => format!("{n} · {}", short(s)),
            None => n.to_string(),
        },
    };
    let _ = tray.set_title(Some(title));

    let alert = !down.is_empty();
    let mut shown = state.tray_alert.lock().unwrap();
    if *shown != alert {
        *shown = alert;
        let png: &[u8] =
            if alert { include_bytes!("../icons/tray-alert.png") } else { include_bytes!("../icons/tray.png") };
        if let Ok(img) = Image::from_bytes(png) {
            let _ = tray.set_icon(Some(img));
            let _ = tray.set_icon_as_template(true);
        }
    }

    let working = snap.agents.iter().filter(|a| a.state == vixen_core::AgentState::Working).count();
    let waiting = snap.agents.iter().filter(|a| a.state == vixen_core::AgentState::Waiting).count();
    let mut tip = format!("Vixen — {running} jobs running · {working} agents working, {waiting} waiting");
    if let Some(s) = next {
        tip.push_str(&format!(", next done in {}", human(s)));
    }
    if alert {
        tip.push_str(&format!("\nUnreachable: {}", down.join(", ")));
    }
    let _ = tray.set_tooltip(Some(tip));
}
