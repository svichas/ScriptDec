#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
#[cfg(unix)]
mod mcp;
mod node;
mod project;
mod sessions;
mod update;

use config::{Config, Project, Settings};
use project::Scan;
use serde::Serialize;
use sessions::{SessionView, Sessions, Spec, Status};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{
    image::Image,
    ipc::{Channel, InvokeResponseBody},
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, RunEvent, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

const HOMEPAGE: &str = "https://github.com/svichas/ScriptDeck";

struct App {
    config: Mutex<Config>,
    config_path: PathBuf,
    sessions: Arc<Sessions>,
    popover_hidden_at: Mutex<Instant>,
}

impl App {
    fn edit(&self, change: impl FnOnce(&mut Config) -> Result<(), String>) -> Result<(), String> {
        let mut config = self.config.lock().unwrap();
        change(&mut config)?;
        config::save(&self.config_path, &config)
    }

    fn project(&self, id: &str) -> Result<Project, String> {
        let config = self.config.lock().unwrap();
        config.projects.iter().find(|p| p.id == id).cloned().ok_or_else(|| "Unknown project".to_string())
    }
}

#[derive(Serialize)]
struct ProjectView {
    #[serde(flatten)]
    project: Project,
    scan: Scan,
}

#[tauri::command]
async fn list_projects(app: AppHandle) -> Vec<ProjectView> {
    let projects = app.state::<App>().config.lock().unwrap().projects.clone();
    projects
        .into_iter()
        .map(|project| {
            let scan = project::scan(&project);
            ProjectView { project, scan }
        })
        .collect()
}

#[tauri::command]
fn node_versions() -> Vec<node::Install> {
    node::installed()
}

#[tauri::command]
async fn pick_folder(app: AppHandle) -> Option<String> {
    app.dialog().file().blocking_pick_folder().map(|path| path.to_string())
}

#[tauri::command]
fn add_project(state: State<App>, path: String) -> Result<Project, String> {
    let dir = Path::new(&path);
    let manifest = project::read_manifest(dir).map_err(|e| format!("Not a Node project: {e}"))?;
    let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned());
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let project = Project {
        id: format!("{millis:x}"),
        name: folder.or(manifest["name"].as_str().map(String::from)).unwrap_or_else(|| path.clone()),
        path,
        ..Default::default()
    };
    state.edit(|config| {
        if config.projects.iter().any(|p| p.path == project.path) {
            return Err("That project is already added".to_string());
        }
        config.projects.push(project.clone());
        Ok(())
    })?;
    Ok(project)
}

#[tauri::command]
fn remove_project(state: State<App>, id: String) -> Result<(), String> {
    state.edit(|config| {
        config.projects.retain(|p| p.id != id);
        Ok(())
    })
}

#[tauri::command]
fn update_project(state: State<App>, project: Project) -> Result<(), String> {
    state.edit(|config| {
        let slot = config.projects.iter_mut().find(|p| p.id == project.id).ok_or("Unknown project")?;
        *slot = project;
        Ok(())
    })
}

/// Adds, changes or deletes one script in a package.json; see `project::write_script`.
#[tauri::command]
async fn edit_script(
    app: AppHandle,
    project_id: String,
    package: Option<String>,
    old: Option<String>,
    script: Option<(String, String)>,
) -> Result<(), String> {
    let state = app.state::<App>();
    let dir = project::package_dir(&state.project(&project_id)?, package.as_deref())?;
    let new = script.as_ref().map(|(name, body)| (name.trim(), body.trim()));
    project::write_script(&dir, old.as_deref(), new)?;
    let Some(old) = old else { return Ok(()) };
    let old_key = project::script_key(package.as_deref(), &old);
    let new_key = new.map(|(name, _)| project::script_key(package.as_deref(), name));
    state.edit(|config| {
        let project = config.projects.iter_mut().find(|p| p.id == project_id).ok_or("Unknown project")?;
        project::move_in_groups(project, &old_key, new_key.clone());
        project::move_pin(&mut project.pinned, &old_key, new_key.clone());
        project::move_pin(&mut project.keep_alive, &old_key, new_key);
        Ok(())
    })
}

#[tauri::command]
fn login_item(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_login_item(app: AppHandle, enabled: bool) -> Result<(), String> {
    let login = app.autolaunch();
    if enabled { login.enable() } else { login.disable() }.map_err(|e| e.to_string())
}

#[tauri::command]
fn settings(state: State<App>) -> Settings {
    state.config.lock().unwrap().settings.clone()
}

/// Async: the shortcut is registered on the main thread, and this waits for it.
#[tauri::command]
async fn update_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let state = app.state::<App>();
    let old = state.config.lock().unwrap().settings.clone();
    if settings.shortcut != old.shortcut {
        if let Err(error) = set_shortcut(&app, settings.shortcut.as_deref()) {
            let _ = set_shortcut(&app, old.shortcut.as_deref());
            return Err(error);
        }
    }
    // Only on a change, so a socket that can't open doesn't block every other setting.
    #[cfg(unix)]
    if settings.mcp != old.mcp {
        mcp::enable(&app, settings.mcp)?;
    }
    app.set_theme(settings.theme);
    state.edit(|config| {
        config.settings = settings.clone();
        Ok(())
    })?;
    app.emit("settings-changed", settings).map_err(|e| e.to_string())
}

fn set_shortcut(app: &AppHandle, shortcut: Option<&str>) -> Result<(), String> {
    let shortcuts = app.global_shortcut();
    shortcuts.unregister_all().map_err(|e| e.to_string())?;
    let Some(shortcut) = shortcut else { return Ok(()) };
    shortcuts.register(shortcut).map_err(|e| format!("Can't use {shortcut} as a shortcut: {e}"))
}

/// The command that adds ScriptDeck to Claude Code. None on Windows, where agents can't connect yet.
#[tauri::command]
fn mcp_command() -> Option<String> {
    let exe = std::env::current_exe().ok().filter(|_| cfg!(unix))?;
    Some(format!("claude mcp add --scope user scriptdeck -- {} --mcp", project::sh_quote(&exe.to_string_lossy())))
}

/// `key` picks up the script's settings; the toolkit and Node installs have none.
#[tauri::command]
fn run(
    state: State<App>,
    project_id: String,
    label: String,
    command: String,
    key: Option<String>,
) -> Result<u32, String> {
    let project = state.project(&project_id)?;
    let node = node::detect(Path::new(&project.path), project.node_version.as_deref(), None);
    state.sessions.start(Spec {
        project_id,
        project: project.name,
        label,
        command,
        cwd: project.path,
        node_bin: node.resolved.as_deref().and_then(node::bin_dir),
        keep_alive: key.is_some_and(|key| project.keep_alive.contains(&key)),
    })
}

#[tauri::command]
fn list_sessions(state: State<App>) -> Vec<SessionView> {
    state.sessions.list()
}

/// Async: it runs `ps`, which the main thread shouldn't wait for.
#[tauri::command]
async fn session_usage(app: AppHandle) -> std::collections::HashMap<u32, sessions::Usage> {
    app.state::<App>().sessions.usage()
}

#[tauri::command]
fn stop(state: State<App>, id: u32) {
    state.sessions.stop(id);
}

#[tauri::command]
fn stop_all(state: State<App>) {
    state.sessions.stop_all();
}

#[tauri::command]
fn restart(state: State<App>, id: u32) -> Result<(), String> {
    state.sessions.restart(id)
}

/// Async: lsof and ps take a moment.
#[tauri::command]
async fn port_owner(app: AppHandle, port: u16) -> Option<sessions::PortOwner> {
    app.state::<App>().sessions.port_owner(port)
}

/// Async: waits, a few seconds at most, for the port to be let go.
#[tauri::command]
async fn free_port(app: AppHandle, port: u16, pid: u32) -> Result<(), String> {
    app.state::<App>().sessions.free_port(port, pid)
}

#[tauri::command]
fn clear_session(state: State<App>, id: u32) {
    state.sessions.clear(id);
}

#[tauri::command]
fn attach(state: State<App>, id: u32, on_data: Channel<InvokeResponseBody>) {
    let sink = move |bytes: &[u8]| {
        let _ = on_data.send(InvokeResponseBody::Raw(bytes.to_vec()));
    };
    state.sessions.attach(id, Box::new(sink));
}

#[tauri::command]
fn send_input(state: State<App>, id: u32, data: String) {
    state.sessions.send_input(id, data.as_bytes());
}

#[tauri::command]
fn resize(state: State<App>, id: u32, cols: u16, rows: u16) {
    state.sessions.resize(id, cols, rows);
}

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

/// Async: creating a window from a sync command deadlocks on Windows.
#[tauri::command]
async fn open_settings(app: AppHandle) -> Result<(), String> {
    show_window(&app, "settings", "settings.html".into(), "ScriptDeck", (860.0, 620.0))
}

#[tauri::command]
async fn open_terminal(app: AppHandle, id: u32) -> Result<(), String> {
    show_window(&app, "terminal", format!("terminal.html?session={id}"), "ScriptDeck Terminal", (900.0, 580.0))?;
    app.emit_to("terminal", "select-session", id).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    // URLs originate from script output, so only ever hand local ones to the OS.
    if !(url.starts_with("http://localhost:") || url.starts_with("https://localhost:")) {
        return Err(format!("Refusing to open {url}"));
    }
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

/// Cmd+click on a link in the terminal. Unlike `open_url`, any host.
#[tauri::command]
fn open_link(app: AppHandle, url: String) -> Result<(), String> {
    // Script output can print any scheme, and file: or an app's own scheme could run something local.
    if !is_web_link(&url) {
        return Err(format!("Refusing to open {url}"));
    }
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

fn is_web_link(url: &str) -> bool {
    let scheme = url.split_once("://").map_or("", |(scheme, _)| scheme);
    scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
}

#[tauri::command]
fn open_homepage(app: AppHandle) -> Result<(), String> {
    app.opener().open_url(HOMEPAGE, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_latest_release(app: AppHandle) -> Result<(), String> {
    app.opener().open_url(format!("{HOMEPAGE}/releases/latest"), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn reveal(app: AppHandle, path: String) -> Result<(), String> {
    app.opener().reveal_item_in_dir(path).map_err(|e| e.to_string())
}

#[tauri::command]
fn hide_popover(app: AppHandle) {
    if let Some(window) = app.get_webview_window("popover") {
        let _ = window.hide();
    }
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

/// Quitting and Windows' updater both end here: saves the running scripts for "Reopen running scripts", then stops them.
fn stop_scripts_for_exit(app: &AppHandle) {
    let state = app.state::<App>();
    if state.config.lock().unwrap().settings.reopen_scripts {
        // Running only, so a script the user is stopping stays stopped. list() is in the order they started.
        let running = state.sessions.list().into_iter().filter(|s| s.status == Status::Running);
        let reopen = running
            .map(|s| config::ReopenScript { project_id: s.project_id, label: s.label, command: s.command })
            .collect();
        let _ = state.edit(|config| {
            config.reopen = reopen;
            Ok(())
        });
    }
    state.sessions.shutdown();
}

/// The menu-bar-only look, except while a real window is open so it can be reached with Cmd-Tab.
fn show_in_dock(app: &AppHandle, visible: bool) {
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

fn toggle_popover(app: &AppHandle) {
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

fn sessions_changed(app: &AppHandle) {
    let _ = app.emit("sessions-changed", ());
    let list = app.state::<App>().sessions.list();
    let running = list.iter().filter(|s| matches!(s.status, Status::Running | Status::Stopping)).count();
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_title((running > 0).then(|| running.to_string()));
        let _ = tray.set_tooltip(Some(format!("ScriptDeck: {running} running")));
    }
}

/// How long a clean run took, as "2m 3s", when it's long enough that you've likely looked away.
fn long_task(code: Option<i32>, millis: u64) -> Option<String> {
    let secs = millis / 1000;
    (code == Some(0) && secs >= 30).then(|| match secs {
        ..60 => format!("{secs}s"),
        60..3600 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, secs / 60 % 60),
    })
}

/// The title and body for a run that ended on its own: a crash, or a long task finishing, each behind its own switch.
fn notification(session: &SessionView, failed: bool, settings: &Settings) -> Option<(String, String)> {
    let ran = session.ended_at.map_or(0, |end| end.saturating_sub(session.started_at));
    match (failed, long_task(session.code, ran)) {
        (true, _) if settings.notify_on_failure => {
            // A keep-alive script's first crash; if it gives up, the plain notice follows.
            let restarting = if session.restart_in.is_some() { ", restarting" } else { "" };
            Some((
                format!("{} failed", session.label),
                format!("{}: exit code {}{restarting}", session.project, session.code.unwrap_or_default()),
            ))
        }
        (false, Some(took)) if settings.notify_on_finish => {
            Some((format!("{} finished in {took}", session.label), session.project.clone()))
        }
        _ => None,
    }
}

/// A crash, or a long task finishing. Stopped runs never get here.
fn script_ended(app: &AppHandle, session: &SessionView, failed: bool) {
    // The output is already in front of them, and `pnpm outdated` exiting 1 isn't news.
    let watching = app.get_webview_window("terminal").is_some_and(|window| window.is_focused().unwrap_or(false));
    if watching {
        return;
    }
    let settings = app.state::<App>().config.lock().unwrap().settings.clone();
    if let Some((title, body)) = notification(session, failed, &settings) {
        let _ = app.notification().builder().title(title).body(body).show();
    }
}

// Only macOS tints template icons; a black glyph would vanish on a dark Windows taskbar.
#[cfg(target_os = "macos")]
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray.png");
#[cfg(not(target_os = "macos"))]
const TRAY_ICON: &[u8] = include_bytes!("../icons/32x32.png");

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
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
                tauri::async_runtime::spawn(async move { open_settings(app).await });
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

fn main() {
    let context = tauri::generate_context!();
    // An agent launched `ScriptDeck --mcp`: be a pipe to the running app, not a second app.
    #[cfg(unix)]
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "--mcp") {
        std::process::exit(mcp::bridge(&context.config().identifier));
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _, event| {
                    if event.state == ShortcutState::Pressed {
                        toggle_popover(app);
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(update::Updates::default())
        .setup(|app| {
            let config_path = app.path().app_config_dir()?.join("config.json");
            let mut config = config::load(&config_path);
            // Cleared at every launch, switch on or off, so an old list never starts scripts later.
            let reopen = std::mem::take(&mut config.reopen);
            if !reopen.is_empty() {
                let _ = config::save(&config_path, &config);
            }
            let first_run = config.projects.is_empty();
            let settings = config.settings.clone();
            let (changed, ended) = (app.handle().clone(), app.handle().clone());
            app.manage(App {
                config: Mutex::new(config),
                config_path,
                sessions: Sessions::new(
                    Box::new(move || sessions_changed(&changed)),
                    Box::new(move |session, failed| script_ended(&ended, session, failed)),
                ),
                popover_hidden_at: Mutex::new(Instant::now()),
            });
            show_in_dock(app.handle(), false);
            build_tray(app.handle())?;
            app.set_theme(settings.theme);
            if let Err(error) = set_shortcut(app.handle(), settings.shortcut.as_deref()) {
                eprintln!("{error}");
            }
            #[cfg(unix)]
            if let Err(error) = mcp::enable(app.handle(), settings.mcp) {
                eprintln!("{error}");
            }
            // The app was called ScriptDec, and a login item goes by the app's name: move an old one over.
            // Release builds only, so a dev build can't take the installed app's login item.
            #[cfg(target_os = "macos")]
            if let Some(old) = std::env::home_dir().map(|home| home.join("Library/LaunchAgents/ScriptDec.plist")) {
                if !cfg!(debug_assertions) && old.exists() && app.autolaunch().enable().is_ok() {
                    let _ = std::fs::remove_file(old);
                }
            }
            if settings.reopen_scripts {
                for script in reopen {
                    // Through run, for the project's Node version. A removed project's scripts fail and are skipped.
                    // shortcut: no script key is saved, so a reopened script doesn't restart on crash until it's
                    // started from the menu again; save the key with it to keep that.
                    let _ = run(app.state(), script.project_id, script.label, script.command, None);
                }
            }
            if first_run {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move { open_settings(handle).await });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
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
        })
        .invoke_handler(tauri::generate_handler![
            list_projects,
            node_versions,
            pick_folder,
            add_project,
            remove_project,
            update_project,
            edit_script,
            login_item,
            set_login_item,
            settings,
            update_settings,
            mcp_command,
            run,
            list_sessions,
            session_usage,
            stop,
            stop_all,
            restart,
            port_owner,
            free_port,
            clear_session,
            attach,
            send_input,
            resize,
            open_settings,
            open_terminal,
            open_url,
            open_link,
            open_homepage,
            open_latest_release,
            update::check_for_update,
            update::download_update,
            update::install_update,
            reveal,
            hide_popover,
            quit,
        ])
        .build(context)
        .expect("failed to start ScriptDeck")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                // First, so an agent can't start a script that the shutdown would miss.
                #[cfg(unix)]
                let _ = mcp::enable(app, false);
                stop_scripts_for_exit(app);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announces_clean_runs_of_30_seconds_or_more() {
        assert_eq!(long_task(Some(0), 31_000).as_deref(), Some("31s"));
        assert_eq!(long_task(Some(0), 123_400).as_deref(), Some("2m 3s"));
        assert_eq!(long_task(Some(0), 3_725_000).as_deref(), Some("1h 2m"));
        assert_eq!(long_task(Some(0), 29_999), None);
        assert_eq!(long_task(Some(0), 5_000), None);
        assert_eq!(long_task(Some(1), 60_000), None);
        assert_eq!(long_task(None, 60_000), None);
    }

    #[test]
    fn each_switch_silences_only_its_own_notification() {
        let ended = |code: i32, secs: u64| SessionView {
            id: 1,
            project_id: "p".into(),
            project: "web".into(),
            label: "build".into(),
            command: "pnpm build".into(),
            status: Status::Exited,
            code: Some(code),
            started_at: 1_000,
            ended_at: Some(1_000 + secs * 1000),
            url: None,
            busy_port: None,
            restart_in: None,
        };
        let (long, short, crash) = (ended(0, 123), ended(0, 5), ended(1, 3));
        let all = Settings::default();
        let no_finish = Settings { notify_on_finish: false, ..Settings::default() };
        let no_crash = Settings { notify_on_failure: false, ..Settings::default() };
        let post = |s: &SessionView, settings| notification(s, s.code != Some(0), settings);
        let title = |s: &SessionView, settings| post(s, settings).map(|(title, _)| title);

        assert_eq!(post(&long, &all), Some(("build finished in 2m 3s".into(), "web".into())));
        assert_eq!(post(&crash, &all), Some(("build failed".into(), "web: exit code 1".into())));
        assert_eq!(post(&short, &all), None);
        assert_eq!(title(&long, &no_finish), None);
        assert_eq!(title(&crash, &no_finish).as_deref(), Some("build failed"));
        assert_eq!(title(&long, &no_crash).as_deref(), Some("build finished in 2m 3s"));
        assert_eq!(title(&crash, &no_crash), None);

        let restarting = SessionView { restart_in: Some(1), ..crash.clone() };
        let body = post(&restarting, &all).map(|(_, body)| body);
        assert_eq!(body.as_deref(), Some("web: exit code 1, restarting"));
    }

    #[test]
    fn opens_only_web_links() {
        assert!(is_web_link("https://example.com/docs?q=1#top"));
        assert!(is_web_link("http://localhost:3000"));
        assert!(is_web_link("HTTPS://EXAMPLE.COM"));
        assert!(!is_web_link("file:///etc/passwd"));
        assert!(!is_web_link("javascript:alert(1)//https://example.com"));
        assert!(!is_web_link("vscode://file/Users/me/app.js"));
        assert!(!is_web_link("ftp://example.com"));
        assert!(!is_web_link("example.com"));
    }
}
