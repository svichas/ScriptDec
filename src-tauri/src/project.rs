use crate::{config::Project, node};
use glob::{MatchOptions, Pattern};
use serde::Serialize;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

const SKIP_DIRS: [&str; 7] = ["node_modules", "dist", "build", "target", "coverage", "src", "vendor"];

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Script {
    /// Stable id, used for pins.
    pub key: String,
    pub name: String,
    /// Workspace package; None for the project's own package.json.
    pub package: Option<String>,
    pub command: String,
    /// The script's body in package.json.
    pub body: String,
}

#[derive(Serialize, Clone)]
pub struct Tool {
    pub label: String,
    pub command: String,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Scan {
    pub pm: String,
    pub node: node::NodeInfo,
    pub scripts: Vec<Script>,
    pub tools: Vec<Tool>,
    pub error: Option<String>,
}

pub fn read_manifest(dir: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(dir.join("package.json")).map_err(|e| format!("package.json: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("package.json: {e}"))
}

fn detect_pm(root: &Path, manifest: &Value) -> String {
    if let Some(field) = manifest["packageManager"].as_str() {
        return field.split('@').next().unwrap_or("npm").to_string();
    }
    let locks = [
        ("pnpm-lock.yaml", "pnpm"),
        ("yarn.lock", "yarn"),
        ("bun.lock", "bun"),
        ("bun.lockb", "bun"),
        ("package-lock.json", "npm"),
    ];
    // Walk up so a package added on its own still sees the monorepo's lockfile.
    root.ancestors()
        .find_map(|dir| locks.iter().find(|(file, _)| dir.join(file).exists()))
        .map_or("npm", |(_, pm)| pm)
        .to_string()
}

/// shortcut: reads only the `packages:` list instead of full YAML, add a YAML parser if that stops being enough.
fn pnpm_workspace_patterns(yaml: &str) -> Vec<String> {
    yaml.lines()
        .skip_while(|l| l.trim_end() != "packages:")
        .skip(1)
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map_while(|l| l.strip_prefix('-'))
        .map(|l| l.trim().trim_matches(|c| c == '\'' || c == '"').to_string())
        .collect()
}

fn workspace_patterns(root: &Path, manifest: &Value) -> Vec<String> {
    if let Ok(yaml) = fs::read_to_string(root.join("pnpm-workspace.yaml")) {
        return pnpm_workspace_patterns(&yaml);
    }
    let field = &manifest["workspaces"];
    field
        .as_array()
        .or_else(|| field["packages"].as_array())
        .map(|list| list.iter().filter_map(|p| p.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

fn find_packages(root: &Path, patterns: &[String]) -> Vec<PathBuf> {
    let (excluded, included): (Vec<_>, Vec<_>) = patterns.iter().partition(|p| p.starts_with('!'));
    let compile = |list: &[&String]| -> Vec<Pattern> {
        list.iter().filter_map(|p| Pattern::new(p.trim_start_matches(['!', '.', '/'])).ok()).collect()
    };
    let (included_p, excluded_p) = (compile(&included), compile(&excluded));
    let depth = included
        .iter()
        .map(|p| if p.contains("**") { 6 } else { p.split('/').count() })
        .max()
        .unwrap_or(0);
    let mut found = vec![];
    walk(root, root, depth, &included_p, &excluded_p, &mut found);
    found.sort();
    found
}

fn walk(root: &Path, dir: &Path, depth: usize, inc: &[Pattern], exc: &[Pattern], found: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    let options = MatchOptions { require_literal_separator: true, ..Default::default() };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) || !entry.path().is_dir() {
            continue;
        }
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap_or(&path);
        let matches = |list: &[Pattern]| list.iter().any(|p| p.matches_path_with(rel, options));
        if matches(inc) && !matches(exc) && path.join("package.json").is_file() {
            found.push(path.clone());
        }
        walk(root, &path, depth - 1, inc, exc, found);
    }
}

/// Leaves ordinary names readable and quotes anything a shell could interpret.
pub fn sh_quote(text: &str) -> String {
    if !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || "@/:._-+=,".contains(c)) {
        text.to_string()
    } else if cfg!(windows) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

fn workspace_command(pm: &str, package: &str, script: &str) -> String {
    let (package, script) = (sh_quote(package), sh_quote(script));
    match pm {
        "pnpm" => format!("pnpm -F {package} run {script}"),
        "yarn" => format!("yarn workspace {package} run {script}"),
        "bun" => format!("bun run --filter {package} {script}"),
        _ => format!("npm run {script} -w {package}"),
    }
}

fn scripts_of(manifest: &Value, package: Option<&str>, pm: &str, out: &mut Vec<Script>) {
    let Some(scripts) = manifest["scripts"].as_object() else { return };
    for (name, body) in scripts {
        out.push(Script {
            key: format!("{}:{name}", package.unwrap_or("")),
            name: name.clone(),
            package: package.map(String::from),
            command: match package {
                Some(p) => workspace_command(pm, p, name),
                None => format!("{pm} run {}", sh_quote(name)),
            },
            body: body.as_str().unwrap_or("").to_string(),
        });
    }
}

fn tools(pm: &str) -> Vec<Tool> {
    let clean = if cfg!(windows) { "rmdir /s /q node_modules &" } else { "rm -rf node_modules &&" };
    let update = if pm == "yarn" { "upgrade" } else { "update" };
    [
        ("Install", format!("{pm} install")),
        ("Outdated", format!("{pm} outdated")),
        ("Update", format!("{pm} {update}")),
        ("Audit", format!("{pm} audit")),
        ("Clean reinstall", format!("{clean} {pm} install")),
    ]
    .map(|(label, command)| Tool { label: label.to_string(), command })
    .to_vec()
}

pub fn scan(project: &Project) -> Scan {
    let root = Path::new(&project.path);
    let manifest = match read_manifest(root) {
        Ok(m) => m,
        Err(error) => return Scan { error: Some(error), ..Default::default() },
    };
    let pm = detect_pm(root, &manifest);
    let mut scripts = vec![];
    scripts_of(&manifest, None, &pm, &mut scripts);
    for dir in find_packages(root, &workspace_patterns(root, &manifest)) {
        let Ok(package) = read_manifest(&dir) else { continue };
        // A package with no name can't be targeted by a workspace filter.
        if let Some(name) = package["name"].as_str() {
            scripts_of(&package, Some(name), &pm, &mut scripts);
        }
    }
    let engines = manifest["engines"]["node"].as_str().map(String::from);
    Scan {
        node: node::detect(root, project.node_version.as_deref(), engines),
        tools: tools(&pm),
        pm,
        scripts,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scriptdec-scan-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: PathBuf, value: Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value.to_string()).unwrap();
    }

    fn project(path: &Path) -> Project {
        Project { path: path.to_string_lossy().into(), ..Default::default() }
    }

    #[test]
    fn scans_a_pnpm_monorepo_like_the_real_one() {
        let dir = repo("pnpm");
        fs::write(
            dir.join("pnpm-workspace.yaml"),
            "packages:\n  - 'packages/**'\n  - 'apps/**'\n  - '!apps/ignored'\n\noverrides:\n  x: 1\n",
        )
        .unwrap();
        write(dir.join("package.json"), json!({"packageManager": "pnpm@11.3.0", "scripts": {"start": "x"}}));
        write(dir.join("apps/app/package.json"), json!({"name": "@acme/app", "scripts": {"start": "y"}}));
        write(dir.join("apps/nested/deep/package.json"), json!({"name": "deep", "scripts": {"a": "b"}}));
        write(dir.join("apps/ignored/package.json"), json!({"name": "ignored", "scripts": {"a": "b"}}));
        write(dir.join("apps/app/node_modules/dep/package.json"), json!({"name": "dep", "scripts": {"a": "b"}}));

        let scan = scan(&project(&dir));
        let commands: Vec<_> = scan.scripts.iter().map(|s| s.command.as_str()).collect();
        assert_eq!(scan.pm, "pnpm");
        assert_eq!(commands, ["pnpm run start", "pnpm -F @acme/app run start", "pnpm -F deep run a"]);
    }

    #[test]
    fn single_star_stays_one_level_deep() {
        let dir = repo("star");
        write(dir.join("package.json"), json!({"workspaces": ["packages/*"]}));
        write(dir.join("packages/a/package.json"), json!({"name": "a", "scripts": {"t": "x"}}));
        write(dir.join("packages/a/sub/package.json"), json!({"name": "sub", "scripts": {"t": "x"}}));
        let scan = scan(&project(&dir));
        let commands: Vec<_> = scan.scripts.iter().map(|s| s.command.as_str()).collect();
        assert_eq!(commands, ["npm run t -w a"]);
    }

    #[test]
    fn reads_the_yarn_workspaces_object_form() {
        let dir = repo("yarn");
        fs::write(dir.join("yarn.lock"), "").unwrap();
        write(dir.join("package.json"), json!({"workspaces": {"packages": ["libs/*"]}}));
        write(dir.join("libs/a/package.json"), json!({"name": "a", "scripts": {"t": "x"}}));
        assert_eq!(scan(&project(&dir)).scripts[0].command, "yarn workspace a run t");
    }

    #[test]
    fn a_package_inside_a_monorepo_finds_the_lockfile_above_it() {
        let dir = repo("up");
        fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        write(dir.join("apps/web/package.json"), json!({"scripts": {"dev": "x"}}));
        assert_eq!(scan(&project(&dir.join("apps/web"))).scripts[0].command, "pnpm run dev");
    }

    #[test]
    fn reports_a_missing_or_broken_manifest_without_failing() {
        let dir = repo("broken");
        assert!(scan(&project(&dir)).error.is_some());
        fs::write(dir.join("package.json"), "{ nope").unwrap();
        assert!(scan(&project(&dir)).error.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn quotes_hostile_script_names() {
        assert_eq!(sh_quote("test:unit"), "test:unit");
        assert_eq!(sh_quote("@scope/pkg"), "@scope/pkg");
        assert_eq!(sh_quote("$(rm -rf ~)"), "'$(rm -rf ~)'");
        assert_eq!(sh_quote("a'b"), "'a'\\''b'");
    }
}
