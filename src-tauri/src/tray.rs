//! The menu bar icon and its popover, and the Settings and Terminal windows it opens, which put ScriptDeck in the
//! Dock while they're open.

use crate::{sessions::Status, App};
use std::time::{Duration, Instant};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Window,
    WindowEvent,
};

fn show_window(app: &AppHandle, label: &str, url: String, title: &str, size: (f64, f64)) -> Result<(), String> {
    let window = match app.get_webview_window(label) {
        Some(window) => window,
        None => {
            let theme = app.state::<App>().config.lock().unwrap().settings.theme;
            let builder = WebviewWindowBuilder::new(app, label, WebviewUrl::App(url.into()))
                .title(title)
                .theme(theme)
                .inner_size(size.0, size.1)
                .min_inner_size(520.0, 320.0);
            // The page draws the title bar itself; the lights sit centred in the terminal's 40px tab bar.
            #[cfg(target_os = "macos")]
            let builder = builder
                .title_bar_style(tauri::TitleBarStyle::Overlay)
                .hidden_title(true)
                .traffic_light_position(tauri::LogicalPosition::new(16.0, 13.0));
            builder.build().map_err(|e| e.to_string())?
        }
    };
    show_in_dock(app, true);
    window.show().and_then(|_| window.set_focus()).map_err(|e| e.to_string())
}

/// Async: creating a window from a sync command deadlocks on Windows. `project` is the page it opens on, and
/// `section` a part of that page to show, like "worktrees".
#[tauri::command]
pub async fn open_settings(app: AppHandle, project: Option<String>, section: Option<String>) -> Result<(), String> {
    let query = |id: &String| match &section {
        Some(section) => format!("settings.html?project={id}&section={section}"),
        None => format!("settings.html?project={id}"),
    };
    show_window(&app, "settings", project.as_ref().map_or("settings.html".into(), query), "ScriptDeck", (860.0, 620.0))?;
    // The URL only counts for a new window.
    let Some(id) = project else { return Ok(()) };
    app.emit_to("settings", "select-project", (id, section)).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_terminal(app: AppHandle, id: u32) -> Result<(), String> {
    show_window(&app, "terminal", format!("terminal.html?session={id}"), "ScriptDeck Terminal", (900.0, 580.0))?;
    app.emit_to("terminal", "select-session", id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn hide_popover(app: AppHandle) {
    if let Some(window) = app.get_webview_window("popover") {
        let _ = window.hide();
    }
}

/// The menu-bar-only look, except while a real window is open so it can be reached with Cmd-Tab.
pub fn show_in_dock(app: &AppHandle, visible: bool) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(if visible {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    });
    #[cfg(not(target_os = "macos"))]
    let _ = (app, visible);
}

fn place_at_tray(app: &AppHandle, window: &WebviewWindow) -> Option<()> {
    let rect = app.tray_by_id("main")?.rect().ok()??;
    let scale = window.scale_factor().ok()?;
    let (tray, tray_size) = (rect.position.to_physical::<f64>(scale), rect.size.to_physical::<f64>(scale));
    let size = window.outer_size().ok()?;
    let (width, height) = (size.width as f64, size.height as f64);
    let monitor = app.monitor_from_point(tray.x, tray.y).ok()??;
    let (left, screen) = (monitor.position().x as f64, monitor.size());
    let right = left + screen.width as f64 - width - 8.0;
    let x = (tray.x + tray_size.width / 2.0 - width / 2.0).min(right).max(left + 8.0);
    // macOS has the bar on top, Windows' taskbar is usually at the bottom.
    let bottom_half = tray.y - monitor.position().y as f64 > screen.height as f64 / 2.0;
    let y = if bottom_half { tray.y - height - 8.0 } else { tray.y + tray_size.height + 6.0 };
    window.set_position(PhysicalPosition::new(x, y)).ok()
}

fn show_popover(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("popover") {
        place_at_tray(app, &window);
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn toggle_popover(app: &AppHandle) {
    // Clicking the icon blurs the open popover first, so it would just reopen.
    let hidden_at = *app.state::<App>().popover_hidden_at.lock().unwrap();
    let visible = app.get_webview_window("popover").is_some_and(|w| w.is_visible().unwrap_or(false));
    if visible {
        if let Some(window) = app.get_webview_window("popover") {
            let _ = window.hide();
        }
    } else if hidden_at.elapsed() > Duration::from_millis(250) {
        show_popover(app);
    }
}

/// Tells the windows to look again, and shows how many scripts run beside the icon.
pub fn sessions_changed(app: &AppHandle) {
    let _ = app.emit("sessions-changed", ());
    let list = app.state::<App>().sessions.list();
    let running = list.iter().filter(|s| matches!(s.status, Status::Running | Status::Stopping)).count();
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_title((running > 0).then(|| running.to_string()));
        let _ = tray.set_tooltip(Some(format!("ScriptDeck: {running} running")));
    }
}

// Only macOS tints template icons; a black glyph would vanish on a dark Windows taskbar.
#[cfg(target_os = "macos")]
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray.png");
#[cfg(not(target_os = "macos"))]
const TRAY_ICON: &[u8] = include_bytes!("../icons/32x32.png");

pub fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open ScriptDeck", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let stop = MenuItem::with_id(app, "stop_all", "Stop All Scripts", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit ScriptDeck", true, None::<&str>)?;
    let line = PredefinedMenuItem::separator(app)?;
    TrayIconBuilder::with_id("main")
        .icon(Image::from_bytes(TRAY_ICON)?)
        .icon_as_template(true)
        .tooltip("ScriptDeck")
        .menu(&Menu::with_items(app, &[&open, &settings, &stop, &line, &quit])?)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_popover(app),
            "settings" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move { open_settings(app, None, None).await });
            }
            "stop_all" => app.state::<App>().sessions.stop_all(),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                toggle_popover(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// The popover hides when it loses focus, as a menu does, and ScriptDeck leaves the Dock with its last window.
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    let app = window.app_handle();
    match (window.label(), event) {
        ("popover", WindowEvent::Focused(false)) => {
            let _ = window.hide();
            *app.state::<App>().popover_hidden_at.lock().unwrap() = Instant::now();
        }
        ("terminal", WindowEvent::Destroyed) => {
            app.state::<App>().sessions.detach();
            show_in_dock(app, app.get_webview_window("settings").is_some());
        }
        ("settings", WindowEvent::Destroyed) => {
            show_in_dock(app, app.get_webview_window("terminal").is_some());
        }
        _ => {}
    }
}
