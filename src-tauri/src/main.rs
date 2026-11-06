#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod git;
#[cfg(unix)]
mod mcp;
mod node;
mod notify;
mod permission;
mod project;
mod sessions;
mod tray;
mod update;

use config::{Config, Project, Settings};
use project::Scan;
use serde::Serialize;
use sessions::{SessionView, Sessions, Spec, Status};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{
    ipc::{Channel, InvokeResponseBody},
    AppHandle, Emitter, Manager, RunEvent, State,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
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

    /// `edit`, for one project.
    fn edit_project(&self, id: &str, change: impl FnOnce(&mut Project)) -> Result<(), String> {
        self.edit(|config| {
            change(config.projects.iter_mut().find(|p| p.id == id).ok_or("Unknown project")?);
            Ok(())
        })
    }

    fn project(&self, id: &str) -> Result<Project, String> {
        let config = self.config.lock().unwrap();
        config.projects.iter().find(|p| p.id == id).cloned().ok_or_else(|| "Unknown project".to_string())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectView {
    #[serde(flatten)]
    project: Project,
    scan: Scan,
    /// Where its scripts run: `Project::dir`.
    dir: String,
    /// In a git repository, from its files alone: the windows lay out its git before git is asked about it.
    is_repo: bool,
}

#[tauri::command]
async fn list_projects(app: AppHandle) -> Vec<ProjectView> {
    let projects = app.state::<App>().config.lock().unwrap().projects.clone();
    projects
        .into_iter()
        .map(|project| {
            let scan = project::scan(&project);
            let dir = project.dir().to_string();
            ProjectView { is_repo: git::is_repo(Path::new(&dir)), dir, project, scan }
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

fn new_project_id() -> String {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    format!("{millis:x}")
}

#[tauri::command]
fn add_project(state: State<App>, path: String) -> Result<Project, String> {
    let dir = Path::new(&path);
    let manifest = project::read_manifest(dir).map_err(|e| format!("Not a Node project: {e}"))?;
    let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned());
    let project = Project {
        id: new_project_id(),
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

/// `base` is the copy the window loaded, so only its own changes are saved; see `project::merge`.
#[tauri::command]
fn update_project(state: State<App>, base: Project, project: Project) -> Result<(), String> {
    let id = project.id.clone();
    state.edit_project(&id, |stored| project::merge(stored, &base, project))
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
    state.edit_project(&project_id, |project| {
        project::move_in_groups(project, &old_key, new_key.clone());
        project::move_pin(&mut project.pinned, &old_key, new_key.clone());
        project::move_pin(&mut project.keep_alive, &old_key, new_key);
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

/// `key` picks up the script's settings; the toolkit and Node installs have none. `cwd` is the folder to run in:
/// the project's in the worktree the window showed it on, or where a reopened script ran. None is `Project::dir`.
/// Off the main thread, which draws every window: it starts a login shell.
#[tauri::command(async)]
fn run(
    state: State<App>,
    project_id: String,
    label: String,
    command: String,
    key: Option<String>,
    cwd: Option<String>,
) -> Result<u32, String> {
    let project = state.project(&project_id)?;
    let cwd = cwd.unwrap_or_else(|| project.dir().to_string());
    let node = node::detect(Path::new(&cwd), project.node_version.as_deref(), None);
    // Away from the project's own folder, it's named after the worktree's branch too: "web · feature-x".
    let worktree = (cwd != project.path).then(|| {
        let folder = || Path::new(&cwd).file_name().map(|name| name.to_string_lossy().into_owned());
        git::branch_of(Path::new(&cwd)).or_else(folder).unwrap_or_default()
    });
    state.sessions.start(Spec {
        project_id,
        project: worktree.as_ref().map_or(project.name.clone(), |branch| format!("{} · {branch}", project.name)),
        label,
        command,
        node_bin: node.resolved.as_deref().and_then(node::bin_dir),
        keep_alive: key.is_some_and(|key| project.keep_alive.contains(&key)),
        cwd,
        worktree,
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

/// Off the main thread, like `run`.
#[tauri::command(async)]
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
            .map(|s| config::ReopenScript { project_id: s.project_id, label: s.label, command: s.command, cwd: Some(s.cwd) })
            .collect();
        let _ = state.edit(|config| {
            config.reopen = reopen;
            Ok(())
        });
    }
    state.sessions.shutdown();
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
                        tray::toggle_popover(app);
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
                    Box::new(move || tray::sessions_changed(&changed)),
                    Box::new(move |session, failed| notify::script_ended(&ended, session, failed)),
                ),
                popover_hidden_at: Mutex::new(Instant::now()),
            });
            tray::show_in_dock(app.handle(), false);
            // Asked once, now, so the menu bar's first look at git doesn't wait for the login shell.
            std::thread::spawn(git::login_path);
            tray::build_tray(app.handle())?;
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
                    // Through run, for the project's Node version, in the folder it ran in. A removed project's
                    // scripts fail and are skipped.
                    // shortcut: no script key is saved, so a reopened script doesn't restart on crash until it's
                    // started from the menu again; save the key with it to keep that.
                    let _ = run(app.state(), script.project_id, script.label, script.command, None, script.cwd);
                }
            }
            if first_run {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move { tray::open_settings(handle, None, None).await });
            }
            Ok(())
        })
        .on_window_event(tray::on_window_event)
        .invoke_handler(tauri::generate_handler![
            list_projects,
            node_versions,
            pick_folder,
            add_project,
            remove_project,
            update_project,
            edit_script,
            git::commands::git_overview,
            git::commands::git_repo,
            git::commands::git_switch,
            git::commands::git_pull,
            git::commands::git_add_worktree,
            git::commands::git_remove_worktree,
            git::commands::git_clone,
            login_item,
            set_login_item,
            settings,
            update_settings,
            notify::notification_permission,
            notify::allow_notifications,
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
            tray::open_settings,
            tray::open_terminal,
            open_url,
            open_link,
            open_homepage,
            open_latest_release,
            update::check_for_update,
            update::download_update,
            update::install_update,
            reveal,
            tray::hide_popover,
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
