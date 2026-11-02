use crate::project::{read_manifest, sh_quote};
use serde::Serialize;
use std::{
    cmp::Reverse,
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    /// Where `wanted` came from: "override", ".nvmrc", ".node-version" or "volta".
    pub source: Option<String>,
    pub wanted: Option<String>,
    /// The installed version `wanted` maps to; None means it needs installing.
    pub resolved: Option<String>,
    /// Command that installs a missing `wanted`, if there's a version manager to run it.
    pub install: Option<String>,
    pub engines: Option<String>,
}

/// A Node version on disk.
#[derive(Serialize, Clone)]
pub struct Install {
    pub version: String,
    pub manager: &'static str,
    #[serde(skip)]
    pub bin: PathBuf,
}

struct Manager {
    name: &'static str,
    /// Holds one folder per installed version.
    versions: PathBuf,
    /// From a version's folder to the one with the `node` binary in it.
    bin: &'static str,
    /// Install command, up to the version.
    install: &'static str,
}

fn env_dir(name: &str) -> Option<PathBuf> {
    env::var_os(name).map(PathBuf::from)
}

fn home(path: &str) -> Option<PathBuf> {
    Some(env::home_dir()?.join(path))
}

fn first_dir<const N: usize>(candidates: [Option<PathBuf>; N]) -> Option<PathBuf> {
    candidates.into_iter().flatten().find(|dir| dir.is_dir())
}

// shortcut: a custom NVM_DIR, FNM_DIR or VOLTA_HOME set only in shell rc files isn't seen when
// the app starts from Finder, ask the login shell for them if someone hits that.
fn nvm_dir() -> Option<PathBuf> {
    first_dir([env_dir("NVM_DIR"), home(".nvm")])
}

fn fnm_dir() -> Option<PathBuf> {
    first_dir([
        env_dir("FNM_DIR"),
        env_dir("XDG_DATA_HOME").map(|dir| dir.join("fnm")),
        home(".local/share/fnm"),
        home(".fnm"),
        home("Library/Application Support/fnm"),
        env_dir("APPDATA").map(|dir| dir.join("fnm")),
    ])
}

fn volta_dir() -> Option<PathBuf> {
    first_dir([env_dir("VOLTA_HOME"), home(".volta"), env_dir("LOCALAPPDATA").map(|dir| dir.join("Volta"))])
}

/// The version managers on this machine, preferred one first.
fn managers() -> Vec<Manager> {
    let windows = cfg!(windows);
    let nvm = if windows { env_dir("NVM_HOME") } else { nvm_dir().map(|dir| dir.join("versions/node")) };
    [
        ("nvm", nvm, if windows { "" } else { "bin" }, "nvm install "),
        (
            "fnm",
            fnm_dir().map(|dir| dir.join("node-versions")),
            if windows { "installation" } else { "installation/bin" },
            "fnm install ",
        ),
        // `fetch`, because `volta install` would also change the user's default Node.
        (
            "volta",
            volta_dir().map(|dir| dir.join("tools/image/node")),
            if windows { "" } else { "bin" },
            "volta fetch node@",
        ),
    ]
    .into_iter()
    .filter_map(|(name, versions, bin, install)| Some(Manager { name, versions: versions?, bin, install }))
    .collect()
}

fn parse(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.split('.').map(|p| p.parse().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

fn installs(managers: &[Manager]) -> Vec<Install> {
    let mut found: Vec<Install> = managers
        .iter()
        .flat_map(|manager| {
            fs::read_dir(&manager.versions).into_iter().flatten().flatten().filter_map(|entry| {
                let name = entry.file_name();
                let version = name.to_str()?.trim_start_matches('v');
                parse(version)?;
                let bin = entry.path().join(manager.bin);
                bin.is_dir().then(|| Install { version: version.to_string(), manager: manager.name, bin })
            })
        })
        .collect();
    // A stable sort, so of two copies of a version the preferred manager's is the one kept.
    found.sort_by_key(|install| Reverse(parse(&install.version)));
    found.dedup_by(|a, b| a.version == b.version);
    found
}

/// Installed versions from every manager, newest first.
pub fn installed() -> Vec<Install> {
    installs(&managers())
}

pub fn bin_dir(version: &str) -> Option<PathBuf> {
    installed().into_iter().find(|install| install.version == version).map(|install| install.bin)
}

/// nvm keeps LTS names in alias files, fnm as `aliases/lts-iron` symlinks into node-versions.
fn lts_target(name: &str, nvm_aliases: Option<&Path>, fnm_aliases: Option<&Path>) -> Option<String> {
    nvm_aliases.and_then(|dir| fs::read_to_string(dir.join("lts").join(name)).ok()).or_else(|| {
        let alias = format!("lts-{}", if name == "*" { "latest" } else { name });
        let link = fs::read_link(fnm_aliases?.join(alias)).ok()?;
        let parts = link.iter().rev().filter_map(|part| part.to_str());
        parts.into_iter().find(|part| part.strip_prefix('v').is_some_and(|v| parse(v).is_some())).map(String::from)
    })
}

/// Maps a version spec ("20", "v20.10", "lts/*", "node") to an installed version.
fn resolve(spec: &str, installed: &[String], lts: &dyn Fn(&str) -> Option<String>, hops: u8) -> Option<String> {
    let spec = spec.trim().trim_start_matches('v');
    if spec == "node" || spec == "stable" {
        return installed.first().cloned();
    }
    if let Some(name) = spec.strip_prefix("lts/") {
        let target = lts(name)?;
        return if hops > 0 { resolve(&target, installed, lts, hops - 1) } else { None };
    }
    installed.iter().find(|v| *v == spec || v.starts_with(&format!("{spec}."))).cloned()
}

/// `wanted` comes from a file in the repo, so it must not reach the shell unquoted.
fn install_command(managers: &[Manager], wanted: &str) -> Option<String> {
    managers.first().map(|manager| format!("{}{}", manager.install, sh_quote(wanted)))
}

pub fn detect(root: &Path, over: Option<&str>, engines: Option<String>) -> NodeInfo {
    let from_files = || {
        root.ancestors().find_map(|dir| {
            let file = [".nvmrc", ".node-version"].iter().find_map(|file| {
                let text = fs::read_to_string(dir.join(file)).ok()?;
                let line = text.lines().next()?.trim();
                (!line.is_empty()).then(|| (file.to_string(), line.to_string()))
            });
            file.or_else(|| {
                let manifest = read_manifest(dir).ok()?;
                Some(("volta".to_string(), manifest["volta"]["node"].as_str()?.to_string()))
            })
        })
    };
    let wanted = match over {
        Some(version) => Some(("override".to_string(), version.to_string())),
        None => from_files(),
    };
    let managers = managers();
    let versions: Vec<String> = installs(&managers).into_iter().map(|install| install.version).collect();
    let (nvm_aliases, fnm_aliases) = (nvm_dir().map(|dir| dir.join("alias")), fnm_dir().map(|dir| dir.join("aliases")));
    let lts = |name: &str| lts_target(name, nvm_aliases.as_deref(), fnm_aliases.as_deref());
    let resolved = wanted.as_ref().and_then(|(_, w)| resolve(w, &versions, &lts, 3));
    let install = wanted.as_ref().filter(|_| resolved.is_none()).and_then(|(_, w)| install_command(&managers, w));
    let (source, wanted) = wanted.unzip();
    NodeInfo { source, wanted, resolved, install, engines }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versions() -> Vec<String> {
        ["24.16.0", "20.19.5", "20.10.0", "9.11.2"].map(String::from).to_vec()
    }

    fn no_lts(_: &str) -> Option<String> {
        None
    }

    fn temp(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("scriptdec-node-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Creates `<versions>/<folder>/<bin>` the way a version manager lays it out.
    fn fake_install(versions: &Path, folder: &str, bin: &str) {
        fs::create_dir_all(versions.join(folder).join(bin)).unwrap();
    }

    #[test]
    fn resolves_prefixes_to_the_newest_match() {
        assert_eq!(resolve("20", &versions(), &no_lts, 3).as_deref(), Some("20.19.5"));
        assert_eq!(resolve("v20.10", &versions(), &no_lts, 3).as_deref(), Some("20.10.0"));
        assert_eq!(resolve("9.11.2", &versions(), &no_lts, 3).as_deref(), Some("9.11.2"));
        assert_eq!(resolve("node", &versions(), &no_lts, 3).as_deref(), Some("24.16.0"));
    }

    #[test]
    fn does_not_confuse_2_with_24() {
        assert_eq!(resolve("2", &versions(), &no_lts, 3), None);
    }

    #[test]
    fn follows_nvm_lts_aliases() {
        let dir = temp("nvm-lts");
        fs::create_dir_all(dir.join("lts")).unwrap();
        fs::write(dir.join("lts/*"), "lts/iron\n").unwrap();
        fs::write(dir.join("lts/iron"), "v20.19.5\n").unwrap();
        let lts = |name: &str| lts_target(name, Some(&dir), None);
        assert_eq!(resolve("lts/*", &versions(), &lts, 3).as_deref(), Some("20.19.5"));
    }

    #[cfg(unix)]
    #[test]
    fn follows_fnm_lts_symlinks() {
        let dir = temp("fnm-lts");
        let target = dir.join("v1.2.3-parent/node-versions/v20.19.5/installation");
        fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, dir.join("lts-iron")).unwrap();
        std::os::unix::fs::symlink(&target, dir.join("lts-latest")).unwrap();
        let lts = |name: &str| lts_target(name, None, Some(&dir));
        assert_eq!(resolve("lts/iron", &versions(), &lts, 3).as_deref(), Some("20.19.5"));
        assert_eq!(resolve("lts/*", &versions(), &lts, 3).as_deref(), Some("20.19.5"));
        assert_eq!(resolve("lts/jod", &versions(), &lts, 3), None);
    }

    #[test]
    fn merges_every_manager_newest_first_and_prefers_the_first_copy() {
        let dir = temp("merge");
        let (nvm, fnm, volta) = (dir.join("nvm"), dir.join("fnm"), dir.join("volta"));
        fake_install(&nvm, "v20.19.5", "bin");
        fake_install(&nvm, "v9.11.2", "bin");
        fake_install(&fnm, "v20.19.5", "installation/bin");
        fake_install(&fnm, "v22.1.0", "installation/bin");
        fake_install(&volta, "24.16.0", "bin");
        fs::create_dir_all(volta.join("0.0.1-half-unpacked")).unwrap();
        fs::create_dir_all(volta.join("18.0.0")).unwrap(); // no bin folder
        let manager = |name, versions, bin| Manager { name, versions, bin, install: "" };
        let found = installs(&[
            manager("nvm", nvm.clone(), "bin"),
            manager("fnm", fnm.clone(), "installation/bin"),
            manager("volta", volta.clone(), "bin"),
        ]);
        let listed: Vec<_> = found.iter().map(|i| (i.version.as_str(), i.manager)).collect();
        assert_eq!(
            listed,
            [("24.16.0", "volta"), ("22.1.0", "fnm"), ("20.19.5", "nvm"), ("9.11.2", "nvm")]
        );
        assert_eq!(found[1].bin, fnm.join("v22.1.0/installation/bin"));
        assert_eq!(found[0].bin, volta.join("24.16.0/bin"));
    }

    #[test]
    fn override_wins_over_nvmrc_and_nvmrc_is_found_in_parents() {
        let root = temp("rc");
        fs::create_dir_all(root.join("apps/web")).unwrap();
        fs::write(root.join(".nvmrc"), "20\n").unwrap();
        let nested = detect(&root.join("apps/web"), None, None);
        assert_eq!((nested.source.as_deref(), nested.wanted.as_deref()), (Some(".nvmrc"), Some("20")));
        let over = detect(&root, Some("22"), None);
        assert_eq!((over.source.as_deref(), over.wanted.as_deref()), (Some("override"), Some("22")));
    }

    #[test]
    fn reads_a_volta_pin_including_from_a_parent_package() {
        let root = temp("volta");
        fs::create_dir_all(root.join("packages/app")).unwrap();
        fs::write(root.join("package.json"), r#"{"volta": {"node": "20.10.0"}}"#).unwrap();
        fs::write(root.join("packages/app/package.json"), r#"{"volta": {"extends": "../../package.json"}}"#).unwrap();
        let node = detect(&root.join("packages/app"), None, None);
        assert_eq!((node.source.as_deref(), node.wanted.as_deref()), (Some("volta"), Some("20.10.0")));
    }

    #[test]
    fn nvmrc_beats_a_volta_pin_in_the_same_folder() {
        let root = temp("both");
        fs::write(root.join(".nvmrc"), "22\n").unwrap();
        fs::write(root.join("package.json"), r#"{"volta": {"node": "20.10.0"}}"#).unwrap();
        assert_eq!(detect(&root, None, None).wanted.as_deref(), Some("22"));
    }

    #[cfg(unix)]
    #[test]
    fn install_commands_never_pass_a_repo_file_to_the_shell_unquoted() {
        let manager = |name, install| Manager { name, versions: PathBuf::new(), bin: "", install };
        let fnm = [manager("fnm", "fnm install ")];
        let volta = [manager("volta", "volta fetch node@")];
        assert_eq!(install_command(&fnm, "20.10").as_deref(), Some("fnm install 20.10"));
        assert_eq!(install_command(&volta, "20.10.0").as_deref(), Some("volta fetch node@20.10.0"));
        assert_eq!(install_command(&fnm, "lts/*").as_deref(), Some("fnm install 'lts/*'"));
        assert_eq!(install_command(&fnm, "1; rm -rf ~").as_deref(), Some("fnm install '1; rm -rf ~'"));
        assert_eq!(install_command(&[], "20"), None);
    }
}
