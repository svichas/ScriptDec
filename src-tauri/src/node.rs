use serde::Serialize;
use std::{
    cmp::Reverse,
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    /// Where `wanted` came from: "override", ".nvmrc" or ".node-version".
    pub source: Option<String>,
    pub wanted: Option<String>,
    /// The installed version `wanted` maps to; None means it needs installing.
    pub resolved: Option<String>,
    pub engines: Option<String>,
}

fn nvm_dir() -> Option<PathBuf> {
    env::var_os("NVM_DIR").map(PathBuf::from).or_else(|| Some(env::home_dir()?.join(".nvm")))
}

fn versions_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return env::var_os("NVM_HOME").map(PathBuf::from);
    }
    Some(nvm_dir()?.join("versions/node"))
}

fn parse(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.split('.').map(|p| p.parse().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// Installed nvm versions, newest first.
pub fn installed() -> Vec<String> {
    let Some(entries) = versions_dir().and_then(|d| fs::read_dir(d).ok()) else {
        return vec![];
    };
    let mut versions: Vec<String> = entries
        .flatten()
        .filter_map(|e| Some(e.file_name().to_str()?.strip_prefix('v')?.to_string()))
        .filter(|v| parse(v).is_some())
        .collect();
    versions.sort_by_key(|v| Reverse(parse(v)));
    versions
}

/// Maps an nvm-style spec ("20", "v20.10", "lts/*", "node") to an installed version.
fn resolve(spec: &str, installed: &[String], aliases: Option<&Path>, hops: u8) -> Option<String> {
    let spec = spec.trim().trim_start_matches('v');
    if spec == "node" || spec == "stable" {
        return installed.first().cloned();
    }
    if spec.starts_with("lts/") {
        let target = fs::read_to_string(aliases?.join(spec)).ok()?;
        return if hops > 0 { resolve(&target, installed, aliases, hops - 1) } else { None };
    }
    installed.iter().find(|v| *v == spec || v.starts_with(&format!("{spec}."))).cloned()
}

pub fn detect(root: &Path, over: Option<&str>, engines: Option<String>) -> NodeInfo {
    let from_files = || {
        root.ancestors().find_map(|dir| {
            [".nvmrc", ".node-version"].iter().find_map(|file| {
                let text = fs::read_to_string(dir.join(file)).ok()?;
                let line = text.lines().next()?.trim();
                (!line.is_empty()).then(|| (file.to_string(), line.to_string()))
            })
        })
    };
    let wanted = match over {
        Some(version) => Some(("override".to_string(), version.to_string())),
        None => from_files(),
    };
    let aliases = nvm_dir().map(|d| d.join("alias"));
    let resolved = wanted.as_ref().and_then(|(_, w)| resolve(w, &installed(), aliases.as_deref(), 3));
    let (source, wanted) = wanted.unzip();
    NodeInfo { source, wanted, resolved, engines }
}

pub fn bin_dir(version: &str) -> Option<PathBuf> {
    let dir = versions_dir()?.join(format!("v{version}"));
    Some(if cfg!(windows) { dir } else { dir.join("bin") })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versions() -> Vec<String> {
        ["24.16.0", "20.19.5", "20.10.0", "9.11.2"].map(String::from).to_vec()
    }

    #[test]
    fn resolves_prefixes_to_the_newest_match() {
        assert_eq!(resolve("20", &versions(), None, 3).as_deref(), Some("20.19.5"));
        assert_eq!(resolve("v20.10", &versions(), None, 3).as_deref(), Some("20.10.0"));
        assert_eq!(resolve("9.11.2", &versions(), None, 3).as_deref(), Some("9.11.2"));
        assert_eq!(resolve("node", &versions(), None, 3).as_deref(), Some("24.16.0"));
    }

    #[test]
    fn does_not_confuse_2_with_24() {
        assert_eq!(resolve("2", &versions(), None, 3), None);
    }

    #[test]
    fn follows_lts_aliases() {
        let dir = env::temp_dir().join(format!("scriptdec-nvm-{}", std::process::id()));
        fs::create_dir_all(dir.join("lts")).unwrap();
        fs::write(dir.join("lts/*"), "lts/iron\n").unwrap();
        fs::write(dir.join("lts/iron"), "v20.19.5\n").unwrap();
        assert_eq!(resolve("lts/*", &versions(), Some(&dir), 3).as_deref(), Some("20.19.5"));
    }

    #[test]
    fn sorts_numerically() {
        let mut v = versions();
        v.reverse();
        v.sort_by_key(|v| Reverse(parse(v)));
        assert_eq!(v, versions());
    }

    #[test]
    fn override_wins_over_nvmrc_and_nvmrc_is_found_in_parents() {
        let root = env::temp_dir().join(format!("scriptdec-rc-{}", std::process::id()));
        fs::create_dir_all(root.join("apps/web")).unwrap();
        fs::write(root.join(".nvmrc"), "20\n").unwrap();
        let nested = detect(&root.join("apps/web"), None, None);
        assert_eq!((nested.source.as_deref(), nested.wanted.as_deref()), (Some(".nvmrc"), Some("20")));
        let over = detect(&root, Some("22"), None);
        assert_eq!((over.source.as_deref(), over.wanted.as_deref()), (Some("override"), Some("22")));
    }
}
