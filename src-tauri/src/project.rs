use crate::{
    config::{self, Project},
    node,
};
use glob::{MatchOptions, Pattern};
use serde::Serialize;
use serde_json::{json, ser::PrettyFormatter, Map, Serializer, Value};
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

pub fn script_key(package: Option<&str>, name: &str) -> String {
    format!("{}:{name}", package.unwrap_or(""))
}

fn scripts_of(manifest: &Value, package: Option<&str>, pm: &str, out: &mut Vec<Script>) {
    let Some(scripts) = manifest["scripts"].as_object() else { return };
    for (name, body) in scripts {
        out.push(Script {
            key: script_key(package, name),
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

/// The folder of the project's own package.json, or of one of its workspace packages.
pub fn package_dir(project: &Project, package: Option<&str>) -> Result<PathBuf, String> {
    let root = PathBuf::from(&project.path);
    let Some(name) = package else { return Ok(root) };
    find_packages(&root, &workspace_patterns(&root, &read_manifest(&root)?))
        .into_iter()
        .find(|dir| read_manifest(dir).is_ok_and(|manifest| manifest["name"] == name))
        .ok_or_else(|| format!("Package {name} not found"))
}

/// Applies one change to a package.json: add (`old` is None), edit or rename (both set), or delete
/// (`new` is None).
/// shortcut: re-serialises the whole file, so hand-aligned JSON is normalised exactly as
/// `npm pkg set` would; patch only the `scripts` span if that diff noise ever matters.
fn edit_manifest(text: &str, old: Option<&str>, new: Option<(&str, &str)>) -> Result<String, String> {
    let mut manifest: Value = serde_json::from_str(text).map_err(|e| format!("package.json: {e}"))?;
    let root = manifest.as_object_mut().ok_or("package.json is not a JSON object")?;
    let scripts = root.entry("scripts").or_insert_with(|| json!({}));
    let scripts = scripts.as_object_mut().ok_or("\"scripts\" in package.json is not an object")?;

    if let Some((name, body)) = new {
        if name.trim().is_empty() || body.trim().is_empty() {
            return Err("A script needs a name and a command".to_string());
        }
        if old != Some(name) && scripts.contains_key(name) {
            return Err(format!("There is already a script called {name}"));
        }
    }
    match (old, new) {
        (None, None) => return Err("Nothing to change".to_string()),
        (None, Some((name, body))) => {
            scripts.insert(name.to_string(), json!(body));
        }
        (Some(old), _) => {
            if !scripts.contains_key(old) {
                return Err(format!("Script {old} is no longer in package.json"));
            }
            // Rebuilt rather than removed and re-added, so a renamed script keeps its place.
            let edited: Map<String, Value> = scripts
                .iter()
                .filter_map(|(key, value)| match new {
                    _ if key != old => Some((key.clone(), value.clone())),
                    Some((name, body)) => Some((name.to_string(), json!(body))),
                    None => None,
                })
                .collect();
            *scripts = edited;
        }
    }

    let indent: String = text.lines().nth(1).unwrap_or("  ").chars().take_while(|c| c.is_whitespace()).collect();
    let mut out = vec![];
    let formatter = PrettyFormatter::with_indent(indent.as_bytes());
    manifest.serialize(&mut Serializer::with_formatter(&mut out, formatter)).map_err(|e| e.to_string())?;
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = String::from_utf8_lossy(&out).replace('\n', eol);
    if text.ends_with('\n') {
        out.push_str(eol);
    }
    Ok(out)
}

/// Reads fresh from disk, so a change made in an editor since the last scan isn't overwritten.
pub fn write_script(dir: &Path, old: Option<&str>, new: Option<(&str, &str)>) -> Result<(), String> {
    let path = fs::canonicalize(dir.join("package.json")).map_err(|e| format!("package.json: {e}"))?;
    let text = fs::read_to_string(&path).map_err(|e| format!("package.json: {e}"))?;
    config::write_atomic(&path, &edit_manifest(&text, old, new)?)
}

/// A pin is keyed by script name, so it follows a rename and goes away with a delete.
pub fn move_pin(pinned: &mut Vec<String>, old: &str, new: Option<String>) {
    if let Some(at) = pinned.iter().position(|key| key == old) {
        match new {
            Some(key) => pinned[at] = key,
            None => drop(pinned.remove(at)),
        }
    }
}

/// Groups hold script keys too, so they follow a rename and lose a delete the same way.
pub fn move_in_groups(project: &mut Project, old: &str, new: Option<String>) {
    for group in &mut project.groups {
        move_pin(&mut group.scripts, old, new.clone());
    }
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

    const MANIFEST: &str = "{\n    \"name\": \"demo\",\n    \"version\": \"1.0.0\",\n    \"scripts\": {\n        \"zeta\": \"z\",\n        \"alpha\": \"a\",\n        \"mid\": \"m\"\n    },\n    \"author\": \"Zoë\",\n    \"files\": [],\n    \"dependencies\": {\n        \"z-dep\": \"1\",\n        \"a-dep\": \"2\"\n    }\n}\n";

    fn script_names(text: &str) -> Vec<String> {
        let manifest: Value = serde_json::from_str(text).unwrap();
        manifest["scripts"].as_object().unwrap().keys().cloned().collect()
    }

    #[test]
    fn adding_changes_only_that_line_and_keeps_order_and_formatting() {
        let edited = edit_manifest(MANIFEST, None, Some(("build", "tsc"))).unwrap();
        let expected = MANIFEST.replace(
            "\"mid\": \"m\"\n",
            "\"mid\": \"m\",\n        \"build\": \"tsc\"\n",
        );
        assert_eq!(edited, expected);
    }

    #[test]
    fn editing_a_body_changes_nothing_else() {
        let edited = edit_manifest(MANIFEST, Some("alpha"), Some(("alpha", "b"))).unwrap();
        assert_eq!(edited, MANIFEST.replace("\"alpha\": \"a\"", "\"alpha\": \"b\""));
    }

    #[test]
    fn renaming_keeps_the_scripts_position() {
        let edited = edit_manifest(MANIFEST, Some("alpha"), Some(("first", "a"))).unwrap();
        assert_eq!(script_names(&edited), ["zeta", "first", "mid"]);
    }

    #[test]
    fn deleting_removes_only_that_script() {
        let edited = edit_manifest(MANIFEST, Some("alpha"), None).unwrap();
        assert_eq!(edited, MANIFEST.replace("        \"alpha\": \"a\",\n", ""));
    }

    #[test]
    fn keeps_tabs_crlf_and_a_missing_final_newline() {
        let tabs = "{\n\t\"scripts\": {\n\t\t\"a\": \"b\"\n\t}\n}";
        assert_eq!(edit_manifest(tabs, Some("a"), Some(("a", "c"))).unwrap(), tabs.replace("\"b\"", "\"c\""));
        let crlf = MANIFEST.replace('\n', "\r\n");
        let edited = edit_manifest(&crlf, Some("alpha"), Some(("alpha", "b"))).unwrap();
        assert_eq!(edited, crlf.replace("\"alpha\": \"a\"", "\"alpha\": \"b\""));
    }

    #[test]
    fn creates_the_scripts_block_when_there_is_none() {
        let edited = edit_manifest("{\n  \"name\": \"x\"\n}\n", None, Some(("dev", "vite"))).unwrap();
        assert_eq!(edited, "{\n  \"name\": \"x\",\n  \"scripts\": {\n    \"dev\": \"vite\"\n  }\n}\n");
    }

    #[test]
    fn refuses_changes_that_would_lose_something() {
        let err = |result: Result<String, String>| result.unwrap_err();
        assert!(err(edit_manifest(MANIFEST, None, Some(("alpha", "x")))).contains("already"));
        assert!(err(edit_manifest(MANIFEST, Some("zeta"), Some(("alpha", "x")))).contains("already"));
        assert!(err(edit_manifest(MANIFEST, Some("gone"), None)).contains("no longer"));
        assert!(err(edit_manifest(MANIFEST, None, Some((" ", "x")))).contains("needs"));
        assert!(err(edit_manifest(MANIFEST, None, Some(("x", "")))).contains("needs"));
        assert!(err(edit_manifest(MANIFEST, None, None)).contains("Nothing"));
        assert!(err(edit_manifest("{ nope", None, Some(("a", "b")))).contains("package.json"));
        assert!(err(edit_manifest("[]", None, Some(("a", "b")))).contains("not a JSON object"));
        assert!(err(edit_manifest("{\"scripts\": 1}", None, Some(("a", "b")))).contains("not an object"));
    }

    #[test]
    fn a_failed_edit_leaves_the_file_as_it_was_and_no_temp_file() {
        let dir = repo("failed-edit");
        fs::write(dir.join("package.json"), MANIFEST).unwrap();
        assert!(write_script(&dir, Some("gone"), None).is_err());
        assert_eq!(fs::read_to_string(dir.join("package.json")).unwrap(), MANIFEST);
        write_script(&dir, None, Some(("new", "x"))).unwrap();
        assert!(fs::read_to_string(dir.join("package.json")).unwrap().contains("\"new\": \"x\""));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn finds_a_workspace_package_by_name_and_refuses_an_unknown_one() {
        let dir = repo("package-dir");
        write(dir.join("package.json"), json!({"workspaces": ["packages/*"]}));
        write(dir.join("packages/a/package.json"), json!({"name": "@x/a"}));
        let project = project(&dir);
        assert_eq!(package_dir(&project, None).unwrap(), dir);
        assert_eq!(package_dir(&project, Some("@x/a")).unwrap(), dir.join("packages/a"));
        assert!(package_dir(&project, Some("../../etc")).is_err());
    }

    #[test]
    fn pins_follow_a_rename_and_go_with_a_delete() {
        let mut pinned = vec![":a".to_string(), ":b".to_string()];
        move_pin(&mut pinned, ":a", Some(":c".to_string()));
        assert_eq!(pinned, [":c", ":b"]);
        move_pin(&mut pinned, ":b", None);
        assert_eq!(pinned, [":c"]);
        move_pin(&mut pinned, ":missing", None);
        assert_eq!(pinned, [":c"]);
    }

    #[test]
    fn groups_follow_a_rename_and_lose_a_delete() {
        let group = |keys: &[&str]| config::Group {
            scripts: keys.iter().map(|k| k.to_string()).collect(),
            ..Default::default()
        };
        let mut project = Project { groups: vec![group(&[":a", ":b"]), group(&[":b", "web:b"])], ..Default::default() };
        move_in_groups(&mut project, ":b", Some(":c".to_string()));
        move_in_groups(&mut project, ":a", None);
        let keys: Vec<_> = project.groups.into_iter().map(|g| g.scripts).collect();
        assert_eq!(keys, [vec![":c"], vec![":c", "web:b"]]);
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
