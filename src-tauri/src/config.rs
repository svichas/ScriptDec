use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use tauri::Theme;

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub projects: Vec<Project>,
    pub settings: Settings,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Opens the menu bar window from anywhere, as an accelerator such as "Alt+Super+Space".
    pub shortcut: Option<String>,
    pub notify_on_failure: bool,
    /// When a script ends on its own, with exit 0, after 30 seconds or more.
    pub notify_on_finish: bool,
    /// None follows the system.
    pub theme: Option<Theme>,
    pub terminal_font_size: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self { shortcut: None, notify_on_failure: true, notify_on_finish: true, theme: None, terminal_font_size: 12 }
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
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct CustomScript {
    pub id: String,
    pub name: String,
    pub command: String,
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
        let dir = std::env::temp_dir().join(format!("scriptdec-config-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("config.json")
    }

    #[test]
    fn round_trips() {
        let path = temp_file("trip");
        let project = Project { id: "a".into(), name: "ui".into(), ..Default::default() };
        save(&path, &Config { projects: vec![project], ..Default::default() }).unwrap();
        assert_eq!(load(&path).projects[0].name, "ui");
    }

    #[test]
    fn a_config_from_before_settings_gets_the_defaults() {
        let path = temp_file("old");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"projects": []}"#).unwrap();
        let settings = load(&path).settings;
        assert!(settings.notify_on_failure && settings.notify_on_finish);
        assert_eq!((settings.terminal_font_size, settings.theme, settings.shortcut), (12, None, None));
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
