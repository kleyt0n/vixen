//! Menu-bar item: live summary in the title; a click opens the summary panel.

use std::time::{Duration, Instant};
use tauri::image::Image;
use tauri::tray::{MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, Rect};
use vixen_core::Snapshot;
use vixen_core::timefmt::human;

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

/// Show the panel centred under the menu-bar icon, or hide it.
fn toggle_panel(app: &AppHandle, icon: Rect) {
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
    let pos = icon.position.to_physical::<f64>(scale);
    let size = icon.size.to_physical::<f64>(scale);
    let width = w.outer_size().map(|s| s.width as f64).unwrap_or(380.0 * scale);
    let mut x = pos.x + size.width / 2.0 - width / 2.0;
    if let Ok(Some(m)) = w.current_monitor().or_else(|_| w.primary_monitor()) {
        let (left, right) = (m.position().x as f64, (m.position().x + m.size().width as i32) as f64);
        x = x.clamp(left + 8.0 * scale, right - width - 8.0 * scale);
    }
    let _ = w.set_position(PhysicalPosition::new(x, pos.y + size.height + 6.0 * scale));
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

/// e.g. "3 ▸ 2h 33m" — running jobs and the soonest finish.
pub fn update(app: &AppHandle, snap: &Snapshot) {
    let Some(tray) = app.tray_by_id(ID) else { return };
    let jobs = snap.targets.iter().flat_map(|t| &t.jobs);
    let running = jobs.clone().filter(|j| j.state.is_active()).count();
    let next = jobs.filter_map(|j| j.eta.as_ref()).map(|e| e.remaining_s).min();
    let down = snap.targets.iter().filter(|t| !t.ok && t.updated_at == 0 && t.error.is_some()).count();
    let mut title = String::new();
    if running > 0 {
        title = running.to_string();
        if let Some(n) = next {
            title.push_str(&format!(" ▸ {}", human(n)));
        }
    }
    if down > 0 {
        title.push_str(" ⚠");
    }
    let _ = tray.set_title(Some(title.trim()));
    let _ = tray.set_tooltip(Some(format!("Vixen — {running} running")));
}
