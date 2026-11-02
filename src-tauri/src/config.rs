use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub projects: Vec<Project>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    /// None follows .nvmrc / .node-version.
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
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
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
        save(&path, &Config { projects: vec![project] }).unwrap();
        assert_eq!(load(&path).projects[0].name, "ui");
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
