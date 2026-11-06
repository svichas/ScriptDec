use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use tauri::Theme;

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub projects: Vec<Project>,
    pub settings: Settings,
    /// The scripts running at the last quit, in the order they started, for `Settings::reopen_scripts`.
    pub reopen: Vec<ReopenScript>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Opens the menu bar window from anywhere, as an accelerator such as "Alt+Super+Space".
    pub shortcut: Option<String>,
    pub notify_on_failure: bool,
    /// When a script ends on its own, with exit 0, after 30 seconds or more.
    pub notify_on_finish: bool,
    /// When a test script ends on its own, passed or failed, however long it took.
    pub notify_on_tests: bool,
    /// None follows the system.
    pub theme: Option<Theme>,
    pub terminal_font_size: u8,
    /// Starts the scripts in `Config::reopen` at launch.
    pub reopen_scripts: bool,
    /// Lets AI agents read and restart scripts over MCP; see mcp.rs.
    pub mcp: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            shortcut: None,
            notify_on_failure: true,
            notify_on_finish: true,
            notify_on_tests: true,
            theme: None,
            terminal_font_size: 12,
            reopen_scripts: false,
            mcp: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    /// None follows .nvmrc, .node-version or a Volta pin.
    pub node_version: Option<String>,
    pub custom: Vec<CustomScript>,
    /// Script keys shown up front in the menu bar.
    pub pinned: Vec<String>,
    /// Script keys restarted when they crash.
    pub keep_alive: Vec<String>,
    /// Scripts started and stopped together from one row in the menu bar.
    pub groups: Vec<Group>,
    /// The worktree the project is switched to: its folder there, which scripts are read from and run in.
    /// None is `path`.
    pub worktree: Option<String>,
}

impl Project {
    /// Where its scripts run: its folder in the worktree it's switched to, or `path` once that's gone.
    pub fn dir(&self) -> &str {
        self.worktree.as_deref().filter(|dir| Path::new(dir).is_dir()).unwrap_or(&self.path)
    }
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct CustomScript {
    pub id: String,
    pub name: String,
    pub command: String,
}

/// What `run` takes to start a script again.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ReopenScript {
    pub project_id: String,
    pub label: String,
    pub command: String,
    /// The folder it ran in, which a worktree makes other than the project's; None from before worktrees.
    pub cwd: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Group {
    pub id: String,
    pub name: String,
    /// Script keys, as in `pinned`.
    pub scripts: Vec<String>,
}

pub fn load(path: &Path) -> Config {
    let Ok(text) = fs::read_to_string(path) else {
        return Config::default();
    };
    serde_json::from_str(&text).unwrap_or_else(|_| {
        // Move a broken file aside so the next save can't destroy the user's projects.
        let _ = fs::rename(path, path.with_extension("corrupt.json"));
        Config::default()
    })
}

pub fn save(path: &Path, config: &Config) -> Result<(), String> {
    let text = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    write_atomic(path, &text)
}

/// A failed write, such as a full disk, leaves the old file in place.
pub fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
    if let Ok(meta) = fs::metadata(path) {
        let _ = fs::set_permissions(&tmp, meta.permissions());
    }
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("scriptdeck-config-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("config.json")
    }

    #[test]
    fn round_trips() {
        let path = temp_file("trip");
        let project = Project { id: "a".into(), name: "ui".into(), worktree: Some("/src/ui-x".into()), ..Default::default() };
        let settings = Settings { reopen_scripts: true, ..Default::default() };
        let script = ReopenScript {
            project_id: "a".into(),
            label: "dev".into(),
            command: "pnpm run dev".into(),
            cwd: Some("/src/ui-x".into()),
        };
        save(&path, &Config { projects: vec![project], settings, reopen: vec![script] }).unwrap();
        let config = load(&path);
        assert_eq!(config.projects[0].name, "ui");
        assert_eq!(config.projects[0].worktree.as_deref(), Some("/src/ui-x"));
        assert!(config.settings.reopen_scripts);
        let script = &config.reopen[0];
        assert_eq!((&*script.project_id, &*script.label, &*script.command), ("a", "dev", "pnpm run dev"));
        assert_eq!(script.cwd.as_deref(), Some("/src/ui-x"));
    }

    #[test]
    fn a_project_runs_in_its_worktree_until_that_folder_is_gone() {
        let dir = temp_file("worktree-dir");
        let worktree = dir.parent().unwrap().to_string_lossy().into_owned();
        fs::create_dir_all(&worktree).unwrap();
        let project = Project { path: "/src/app".into(), worktree: Some(worktree.clone()), ..Default::default() };
        assert_eq!(project.dir(), worktree);
        fs::remove_dir_all(&worktree).unwrap();
        assert_eq!(project.dir(), "/src/app");
        assert_eq!(Project { path: "/src/app".into(), ..Default::default() }.dir(), "/src/app");
    }

    #[test]
    fn groups_survive_a_relaunch() {
        let path = temp_file("groups");
        let scripts = vec![":web".to_string(), "@acme/api:dev".to_string()];
        let project = Project { groups: vec![Group { name: "dev".into(), scripts, ..Default::default() }], ..Default::default() };
        save(&path, &Config { projects: vec![project], ..Default::default() }).unwrap();
        assert_eq!(load(&path).projects[0].groups[0].scripts, [":web", "@acme/api:dev"]);
    }

    #[test]
    fn a_config_from_before_settings_gets_the_defaults() {
        let path = temp_file("old");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"projects": []}"#).unwrap();
        let settings = load(&path).settings;
        assert!(settings.notify_on_failure && settings.notify_on_finish && settings.notify_on_tests);
        assert!(!settings.mcp);
        assert_eq!((settings.terminal_font_size, settings.theme, settings.shortcut), (12, None, None));
    }

    #[test]
    fn a_config_from_before_reopen_gets_the_defaults() {
        let path = temp_file("before-reopen");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"projects": [], "settings": {"terminalFontSize": 14}}"#).unwrap();
        let config = load(&path);
        assert_eq!(config.settings.terminal_font_size, 14);
        assert!(!config.settings.reopen_scripts);
        assert!(config.reopen.is_empty());
    }

    #[test]
    fn keeps_a_copy_of_a_corrupt_file() {
        let path = temp_file("corrupt");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ nope").unwrap();
        assert!(load(&path).projects.is_empty());
        assert_eq!(fs::read_to_string(path.with_extension("corrupt.json")).unwrap(), "{ nope");
    }
}
