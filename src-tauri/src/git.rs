//! A project's git repository: status, branches, worktrees, commits, sync and clone, through the git command line.

use serde::Serialize;
use std::{
    collections::HashSet,
    env,
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// What the menu bar shows for a project.
#[derive(Serialize, Clone, Default, Debug, PartialEq)]
pub struct Summary {
    /// None when HEAD is detached.
    pub branch: Option<String>,
    /// HEAD's commit, abbreviated; None before the first commit.
    pub commit: Option<String>,
    pub upstream: Option<String>,
    /// The upstream was deleted on the remote, so pushing would make it again.
    pub gone: bool,
    pub ahead: u32,
    pub behind: u32,
    /// Files with uncommitted changes, untracked ones included.
    pub changes: usize,
}

#[derive(Serialize, Clone, Default, Debug, PartialEq)]
pub struct Status {
    #[serde(flatten)]
    pub summary: Summary,
    pub files: Vec<Change>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Change {
    /// From the top of the repository.
    pub path: String,
    /// Where a renamed file came from.
    pub from: Option<String>,
    /// A added, M modified, D deleted, R renamed, U conflicted, ? untracked.
    pub kind: char,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Branch {
    /// "main", or "origin/main" for a remote branch.
    pub name: String,
    pub remote: bool,
    /// The worktree that has it checked out.
    pub worktree: Option<String>,
}

#[derive(Serialize, Clone, Default, Debug, PartialEq)]
pub struct Worktree {
    pub path: String,
    /// None when detached.
    pub branch: Option<String>,
    /// The repository's own folder, which can't be removed.
    pub main: bool,
    /// The one this project is in.
    pub current: bool,
    /// The ScriptDeck project in it; main.rs fills this in.
    pub project: Option<String>,
}

#[derive(Serialize)]
pub struct Repo {
    pub status: Status,
    pub branches: Vec<Branch>,
    pub worktrees: Vec<Worktree>,
    /// Where the project sits in its worktree, like "apps/web/", to find the same folder in another one.
    pub prefix: String,
}

/// Whether `dir` is in a git repository, checked without running git: on a Mac without the developer tools,
/// running it opens an installer.
pub fn is_repo(dir: &Path) -> bool {
    dir.ancestors().any(|dir| dir.join(".git").exists())
}

/// The PATH a terminal gets, asked of the login shell once, as scripts get it in sessions.rs. An app opened from
/// Finder has a bare one, where git hooks such as husky's can't find node or npx.
#[cfg(unix)]
pub fn login_path() -> Option<&'static str> {
    static PATH: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let out = Command::new(shell)
            .args(["-ilc", "printf '\\nSCRIPTDECK_PATH=%s\\n' \"$PATH\""])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        // The rc files can print too, so the marker finds the line.
        let text = String::from_utf8_lossy(&out.stdout);
        text.lines().rev().find_map(|line| line.strip_prefix("SCRIPTDECK_PATH=")).map(String::from)
    })
    .as_deref()
}

/// Windows apps get the user's whole PATH.
#[cfg(not(unix))]
pub fn login_path() -> Option<&'static str> {
    None
}

pub struct Git {
    dir: PathBuf,
    /// For git and the hooks it runs: the project's Node first, then the login shell's PATH.
    path: Option<OsString>,
}

impl Git {
    pub fn new(dir: impl Into<PathBuf>, node_bin: Option<PathBuf>) -> Self {
        let base = login_path().map(OsString::from).or_else(|| env::var_os("PATH"));
        let dirs = node_bin.into_iter().chain(base.iter().flat_map(env::split_paths));
        Self { dir: dir.into(), path: env::join_paths(dirs).ok().filter(|path| !path.is_empty()) }
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        self.run_with(args, None)
    }

    /// Runs git in the project's folder, with `input` on stdin, and returns its output or what went wrong.
    fn run_with(&self, args: &[&str], input: Option<String>) -> Result<String, String> {
        let mut cmd = Command::new("git");
        cmd.args(args)
            .current_dir(&self.dir)
            // A password prompt nobody can see would wait forever, so git fails and says why instead.
            .env("GIT_TERMINAL_PROMPT", "0")
            // Reading the status mustn't lock the index while a git command in the user's terminal wants it.
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(path) = &self.path {
            cmd.env("PATH", path);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| format!("Can't run git: {e}"))?;
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            // If git quit before reading it all, its own error below says why.
            let _ = stdin.write_all(input.as_bytes());
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
        if out.status.success() {
            return Ok(text(&out.stdout));
        }
        Err(failure(&format!("{}\n{}", text(&out.stdout), text(&out.stderr))))
    }

    pub fn status(&self) -> Result<Status, String> {
        Ok(parse_status(&self.run(&["status", "--porcelain=v2", "--branch", "-z"])?))
    }

    /// Local branches, then remote ones not already checked out locally, each most recent first.
    pub fn branches(&self) -> Result<Vec<Branch>, String> {
        let format = "--format=%(refname)%00%(worktreepath)";
        let refs = self.run(&["for-each-ref", "--sort=-committerdate", format, "refs/heads", "refs/remotes"])?;
        let mut branches = parse_branches(&refs);
        // A worktree whose folder was deleted, say in Finder, holds its branch only until switch() prunes it.
        for branch in &mut branches {
            if branch.worktree.as_deref().is_some_and(|path| !Path::new(path).exists()) {
                branch.worktree = None;
            }
        }
        Ok(branches)
    }

    pub fn worktrees(&self) -> Result<Vec<Worktree>, String> {
        Ok(parse_worktrees(&self.run(&["worktree", "list", "--porcelain", "-z"])?))
    }

    pub fn repo(&self) -> Result<Repo, String> {
        let here = fs::canonicalize(self.run(&["rev-parse", "--show-toplevel"])?.trim_end_matches(['\r', '\n'])).ok();
        let mut worktrees = self.worktrees()?;
        for tree in &mut worktrees {
            tree.current = here.is_some() && fs::canonicalize(&tree.path).ok() == here;
        }
        Ok(Repo { status: self.status()?, branches: self.branches()?, worktrees, prefix: self.prefix()? })
    }

    /// Where the project's folder is in its worktree, like "apps/web/"; empty at the top.
    pub fn prefix(&self) -> Result<String, String> {
        Ok(self.run(&["rev-parse", "--show-prefix"])?.trim_end_matches(['\r', '\n']).into())
    }

    /// A branch by name, or a remote one by the name it would have locally. `origin/main` means main when there's a
    /// local main, as it isn't listed then; a new branch can't take a remote branch's name, which would make it
    /// mean two things.
    fn find_branch(&self, name: &str) -> Result<Option<Branch>, String> {
        let branches = self.branches()?;
        let found = branches.iter().find(|b| b.name == name);
        let found = found.or_else(|| branches.iter().find(|b| b.remote && local_name(&b.name) == name));
        if found.is_some() || !name.contains('/') {
            return Ok(found.cloned());
        }
        let remotes = self.run(&["remote"])?;
        let Some((remote, short)) = name.split_once('/').filter(|(remote, _)| remotes.lines().any(|r| r == *remote))
        else {
            return Ok(None);
        };
        let local = branches.iter().find(|b| !b.remote && b.name == short);
        local.cloned().map(Some).ok_or_else(|| format!("{remote} has no branch {short}"))
    }

    /// A branch checked out in a worktree whose folder was deleted can't be checked out anywhere else until git
    /// forgets that worktree.
    fn forget_deleted_worktree(&self, branch: &str) -> Result<(), String> {
        let path = self.run(&["for-each-ref", "--format=%(worktreepath)", &format!("refs/heads/{branch}")])?;
        let path = path.trim_end_matches(['\r', '\n']);
        if path.is_empty() || Path::new(path).exists() {
            return Ok(());
        }
        self.run(&["worktree", "prune"]).map(drop)
    }

    /// Checks out a local branch, a remote one as a new local branch tracking it, or a new branch from HEAD.
    /// Uncommitted changes come along, unless they'd be overwritten: then git refuses and says which.
    pub fn switch(&self, name: &str) -> Result<(), String> {
        let name = branch_name(name)?;
        let found = self.find_branch(name)?;
        // Leaving a detached HEAD for another branch would strand the commits made on it; git only warns.
        if found.is_some() && self.status()?.summary.branch.is_none() {
            let stranded = self.run(&["rev-list", "--max-count=1", "HEAD", "--not", "--branches", "--remotes", "--tags"])?;
            if !stranded.trim().is_empty() {
                return Err("HEAD has commits on no branch, which switching would leave behind. \
                            Type a new branch name to keep them on that first."
                    .into());
            }
        }
        match found {
            Some(Branch { remote: true, name: remote, .. }) => self.run(&["switch", "--track", &remote]),
            Some(Branch { name: local, .. }) => {
                self.forget_deleted_worktree(&local)?;
                self.run(&["switch", &local])
            }
            // Tracking nothing, so the first push publishes it under its own name.
            None => self.run(&["switch", "--no-track", "--create", name]),
        }
        .map(drop)
    }

    /// Commits these files as they are on disk. When they're every change there is, it's `git add --all` and
    /// `git commit`, as in a terminal, which also finishes a merge. Fewer are a commit of only those paths: other
    /// staged changes stay staged, and if a hook stops it, the index is as it was.
    pub fn commit(&self, message: &str, paths: &[String]) -> Result<(), String> {
        if message.trim().is_empty() {
            return Err("Write a commit message first".into());
        }
        let wanted: HashSet<&str> = paths.iter().map(String::as_str).collect();
        let (files, others): (Vec<Change>, Vec<Change>) =
            self.status()?.files.into_iter().partition(|f| wanted.contains(f.path.as_str()));
        if files.is_empty() {
            return Err("Nothing to commit: those files have no changes any more".into());
        }
        if others.is_empty() {
            self.run(&["add", "--all"])?;
            return self.run(&["commit", "--message", message]).map(drop);
        }
        // From the top and literal, since status paths are, and a file called `*.js` is only that file.
        let spec = |path: &str| format!(":(top,literal){path}\0");
        let stdin = ["--pathspec-from-file=-", "--pathspec-file-nul"];
        // Git only commits paths it knows, so new files are announced, but not staged.
        let new: String = files.iter().filter(|f| f.kind == '?').map(|f| spec(&f.path)).collect();
        if !new.is_empty() {
            self.run_with(&["add", "--intent-to-add", stdin[0], stdin[1]], Some(new))?;
        }
        // A rename commits the old name going away too.
        let only: String = files.iter().flat_map(|f| std::iter::once(&f.path).chain(&f.from)).map(|p| spec(p)).collect();
        self.run_with(&["commit", "--only", "--message", message, stdin[0], stdin[1]], Some(only)).map(drop)
    }

    pub fn fetch(&self) -> Result<(), String> {
        self.run(&["fetch", "--all", "--prune"]).map(drop)
    }

    /// Fast-forward only, so it never leaves a merge or a rebase half done.
    pub fn pull(&self) -> Result<(), String> {
        self.run(&["pull", "--ff-only"]).map(drop)
    }

    /// The first push of a branch publishes it to origin, or to the only remote there is.
    pub fn push(&self) -> Result<(), String> {
        if self.status()?.summary.upstream.is_some() {
            return self.run(&["push"]).map(drop);
        }
        let remotes = self.run(&["remote"])?;
        let remote = remotes.lines().find(|name| *name == "origin").or_else(|| remotes.lines().next());
        let remote = remote.ok_or("This repository has no remote to push to")?;
        self.run(&["push", "--set-upstream", remote, "HEAD"]).map(drop)
    }

    /// Checks a branch out in a new folder beside the main worktree: an existing branch, a remote one as a new
    /// local branch tracking it, or a new branch from `base` (HEAD when None). Returns the folder and the branch.
    pub fn add_worktree(&self, name: &str, base: Option<&str>) -> Result<(PathBuf, String), String> {
        let name = branch_name(name)?;
        let found = self.find_branch(name)?;
        let remote = found.as_ref().filter(|branch| branch.remote).map(|branch| branch.name.as_str());
        let local = match (&found, remote) {
            (_, Some(remote)) => local_name(remote),
            (Some(branch), None) => branch.name.as_str(),
            (None, None) => name,
        };
        // The main worktree is listed first. A bare repository's isn't listed, so beside the first one there is.
        let trees = self.worktrees()?;
        let beside = trees.iter().find(|tree| tree.main).or(trees.first()).ok_or("This repository has no worktree")?;
        let dir = free_path(Path::new(&beside.path), local);
        let path = dir.to_string_lossy();
        match (&found, remote) {
            (_, Some(remote)) => self.run(&["worktree", "add", "--track", "-b", local, &path, remote]),
            (Some(_), None) => {
                self.forget_deleted_worktree(local)?;
                self.run(&["worktree", "add", &path, local])
            }
            (None, None) => {
                let base = base.map(branch_name).transpose()?;
                // Tracking nothing: from origin/main, it would track that, and a push could land there.
                let args = ["worktree", "add", "--no-track", "-b", local, &path];
                self.run(&[&args[..], base.as_slice()].concat())
            }
        }?;
        Ok((dir, local.to_string()))
    }

    /// Deletes a worktree's folder. Git refuses while it has changes or untracked files, so nothing unsaved is lost.
    pub fn remove_worktree(&self, path: &str) -> Result<(), String> {
        if !Path::new(path).is_absolute() {
            return Err(format!("Not a worktree: {path}"));
        }
        // Git checks only this worktree's files, so another worktree in an ignored folder inside it, like
        // .claude/worktrees, would go too, unsaved work and all.
        let real = |path: &str| fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
        let target = real(path);
        let trees = self.worktrees()?;
        let inner = trees.iter().map(|tree| real(&tree.path)).find(|tree| *tree != target && tree.starts_with(&target));
        if let Some(inner) = inner {
            return Err(format!("Remove the worktree inside it first: {}", inner.display()));
        }
        self.run(&["worktree", "remove", path]).map(drop)
    }
}

/// Clones into a new folder in `parent`, named as git would name it, and returns that folder.
pub fn clone(url: &str, parent: &Path) -> Result<PathBuf, String> {
    let url = expand_url(url.trim());
    // A leading dash would be read as an option.
    if url.is_empty() || url.starts_with('-') {
        return Err("Paste the repository's URL".into());
    }
    let name = repo_name(&url).ok_or("Can't tell the folder's name from that URL")?;
    let dir = parent.join(name);
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }
    Git::new(parent, None).run(&["clone", "--", &url, &dir.to_string_lossy()])?;
    Ok(dir)
}

/// `owner/repo` is short for that repository on GitHub.
fn expand_url(url: &str) -> String {
    let short = url.split('/').count() == 2
        && !url.starts_with('.')
        && url.chars().all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c));
    if short {
        format!("https://github.com/{}.git", url.trim_end_matches(".git"))
    } else {
        url.to_string()
    }
}

/// The folder `git clone` would make: `repo` from `git@github.com:owner/repo.git`.
fn repo_name(url: &str) -> Option<String> {
    let last = url.trim_end_matches(['/', '\\']).rsplit(['/', '\\', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty() && name != "." && name != "..").then(|| name.to_string())
}

fn branch_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Type a branch name".into());
    }
    // A leading dash would be read as an option.
    if name.starts_with('-') {
        return Err(format!("{name} isn't a valid branch name"));
    }
    Ok(name)
}

/// "origin/feature/x" is "feature/x" locally.
fn local_name(remote: &str) -> &str {
    remote.split_once('/').map_or(remote, |(_, name)| name)
}

/// `app` and `feature/login` give `app-feature-login` beside `app`, or `app-feature-login-2` when that's taken.
fn free_path(root: &Path, branch: &str) -> PathBuf {
    let slug: String = branch.chars().map(|c| if c.is_alphanumeric() || "._-".contains(c) { c } else { '-' }).collect();
    let folder = root.file_name().map_or("repo".into(), |name| name.to_string_lossy());
    let folder = folder.trim_end_matches(".git");
    let parent = root.parent().unwrap_or(root);
    let name = |n: u32| if n == 1 { format!("{folder}-{slug}") } else { format!("{folder}-{slug}-{n}") };
    (1..).map(|n| parent.join(name(n))).find(|dir| !dir.exists()).unwrap_or_default()
}

/// Git's message, or a failed hook's output. Long output ends with what went wrong, so the end is kept.
fn failure(output: &str) -> String {
    let lines: Vec<&str> = output.trim().lines().collect();
    match lines.len() {
        0 => "git failed".into(),
        1..=20 => lines.join("\n"),
        n => format!("…\n{}", lines[n - 20..].join("\n")),
    }
}

/// `git status --porcelain=v2 --branch -z`.
fn parse_status(text: &str) -> Status {
    let mut status = Status::default();
    let mut compared = false; // an upstream with no ahead and behind counts is gone
    let mut entries = text.split('\0');
    while let Some(entry) = entries.next() {
        if let Some(header) = entry.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            let summary = &mut status.summary;
            match key {
                "branch.oid" if value != "(initial)" => summary.commit = Some(value.chars().take(7).collect()),
                "branch.head" if value != "(detached)" => summary.branch = Some(value.into()),
                "branch.upstream" => summary.upstream = Some(value.into()),
                "branch.ab" => {
                    let count = |sign: char| value.split(' ').find_map(|n| n.strip_prefix(sign)?.parse().ok());
                    summary.ahead = count('+').unwrap_or(0);
                    summary.behind = count('-').unwrap_or(0);
                    compared = true;
                }
                _ => {}
            }
            continue;
        }
        let xy = entry.get(2..4).unwrap_or("");
        // Each kind has a fixed number of fields before the path, which can hold spaces.
        let change = match entry.chars().next() {
            Some('1') => entry.splitn(9, ' ').nth(8).map(|path| (path, None, ordinary_kind(xy))),
            Some('2') => entry.splitn(10, ' ').nth(9).map(|path| (path, entries.next(), 'R')),
            Some('u') => entry.splitn(11, ' ').nth(10).map(|path| (path, None, 'U')),
            Some('?') => entry.get(2..).map(|path| (path, None, '?')),
            _ => None,
        };
        if let Some((path, from, kind)) = change {
            status.files.push(Change { path: path.into(), from: from.map(String::from), kind });
        }
    }
    status.summary.changes = status.files.len();
    status.summary.gone = status.summary.upstream.is_some() && !compared;
    status
}

/// From the staged and unstaged letters of a changed file. ".A" is a new file git was told about, not staged.
fn ordinary_kind(xy: &str) -> char {
    if xy.contains('D') {
        'D'
    } else if xy.contains('A') {
        'A'
    } else {
        'M'
    }
}

/// `git for-each-ref` lines of a refname and a worktree path, separated by NUL.
fn parse_branches(text: &str) -> Vec<Branch> {
    let refs: Vec<(&str, &str)> = text.lines().filter_map(|line| line.split_once('\0')).collect();
    let local: HashSet<&str> = refs.iter().filter_map(|(name, _)| name.strip_prefix("refs/heads/")).collect();
    let local_first = refs.iter().filter(|(name, _)| name.starts_with("refs/heads/"));
    let remote_after = refs.iter().filter(|(name, _)| !name.starts_with("refs/heads/"));
    local_first
        .chain(remote_after)
        .filter_map(|(name, worktree)| {
            let worktree = (!worktree.is_empty()).then(|| worktree.to_string());
            if let Some(name) = name.strip_prefix("refs/heads/") {
                return Some(Branch { name: name.into(), remote: false, worktree });
            }
            let name = name.strip_prefix("refs/remotes/")?;
            let short = local_name(name);
            // origin/HEAD only points at another branch, and one you have locally needn't be listed twice.
            let shown = name.contains('/') && short != "HEAD" && !local.contains(short);
            shown.then(|| Branch { name: name.into(), remote: true, worktree: None })
        })
        .collect()
}

/// `git worktree list --porcelain -z`: NUL-terminated lines, and an empty one after each worktree.
fn parse_worktrees(text: &str) -> Vec<Worktree> {
    text.split("\0\0")
        .enumerate()
        .filter_map(|(i, record)| {
            let mut tree = Worktree { main: i == 0, ..Default::default() };
            for line in record.split('\0') {
                let (key, value) = line.split_once(' ').unwrap_or((line, ""));
                match key {
                    "worktree" => tree.path = value.into(),
                    "branch" => tree.branch = value.strip_prefix("refs/heads/").map(String::from),
                    // A bare repository has no files to show, and a prunable worktree's folder is gone.
                    "bare" | "prunable" => return None,
                    _ => {}
                }
            }
            (!tree.path.is_empty()).then_some(tree)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty folder for one test. Canonical: macOS's temp folder is a symlink, and git reports real paths.
    fn temp(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("scriptdeck-git-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::canonicalize(dir).unwrap()
    }

    /// Plain git, for setting a test up.
    fn git(dir: &Path, args: &[&str]) -> String {
        let id = ["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false"];
        let out = Command::new("git").args(id).args(args).current_dir(dir).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }

    /// `app`, a clone of an `origin` with one commit, configured so it commits without the user's own identity,
    /// signing or hooks.
    fn repo(name: &str) -> (PathBuf, Git) {
        let base = temp(name);
        let origin = base.join("origin");
        fs::create_dir(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        fs::write(origin.join("a.txt"), "a\n").unwrap();
        fs::write(origin.join(".gitignore"), "node_modules\n").unwrap();
        git(&origin, &["add", "-A"]);
        git(&origin, &["commit", "-qm", "first"]);
        git(&base, &["clone", "-q", "origin", "app"]);
        let app = base.join("app");
        let hooks = base.join("no-hooks");
        fs::create_dir(&hooks).unwrap();
        let config = [
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", hooks.to_str().unwrap()),
        ];
        for (key, value) in config {
            git(&app, &["config", key, value]);
        }
        (app.clone(), Git::new(app, None))
    }

    fn committed(dir: &Path) -> String {
        git(dir, &["show", "--name-status", "--format=", "HEAD"])
    }

    fn paths(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn parses_branch_state_and_every_kind_of_change() {
        let text = [
            "# branch.oid 46d33b4f41f4dbb10aed702b0c76f4e5188da42e",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -3",
            "1 .M N... 100644 100644 100644 6178079 6178079 src/a file.js",
            "1 A. N... 000000 100644 100644 0000000 2fa992c new.js",
            "1 D. N... 100644 000000 000000 2fa992c 0000000 gone.js",
            "2 R. N... 100644 100644 100644 2fa992c 2fa992c R100 renamed.js",
            "old name.js",
            "u UU N... 100644 100644 100644 100644 a b c both.js",
            "? untracked dir/",
            "",
        ]
        .join("\0");
        let status = parse_status(&text);
        let summary = Summary {
            branch: Some("main".into()),
            commit: Some("46d33b4".into()),
            upstream: Some("origin/main".into()),
            gone: false,
            ahead: 2,
            behind: 3,
            changes: 6,
        };
        assert_eq!(status.summary, summary);
        let files: Vec<_> = status.files.iter().map(|f| (f.kind, f.path.as_str(), f.from.as_deref())).collect();
        assert_eq!(
            files,
            [
                ('M', "src/a file.js", None),
                ('A', "new.js", None),
                ('D', "gone.js", None),
                ('R', "renamed.js", Some("old name.js")),
                ('U', "both.js", None),
                ('?', "untracked dir/", None),
            ]
        );
    }

    #[test]
    fn a_detached_head_or_a_repo_with_no_commits_has_no_branch_or_no_commit() {
        let detached = parse_status("# branch.oid 46d33b4f41f4\0# branch.head (detached)\0");
        assert_eq!((detached.summary.branch, detached.summary.commit.as_deref()), (None, Some("46d33b4")));
        let fresh = parse_status("# branch.oid (initial)\0# branch.head main\0");
        assert_eq!((fresh.summary.branch.as_deref(), fresh.summary.commit), (Some("main"), None));
        // Deleted on the remote: git names the upstream but can't compare with it.
        let gone = parse_status("# branch.oid 46d33b4f41f4\0# branch.head x\0# branch.upstream origin/x\0");
        assert!(gone.summary.gone && !fresh.summary.gone);
        // A new file git was told about but that isn't staged.
        assert_eq!(parse_status("1 .A N... 000000 000000 100644 0 0 n.txt\0").files[0].kind, 'A');
    }

    #[test]
    fn lists_local_branches_then_remote_ones_without_duplicates() {
        let text = [
            "refs/remotes/origin/HEAD\0",
            "refs/remotes/origin/feature/x\0",
            "refs/heads/main\0/src/app",
            "refs/remotes/origin/main\0",
            "refs/heads/fix\0",
        ]
        .join("\n");
        let branches: Vec<_> =
            parse_branches(&text).into_iter().map(|b| (b.name, b.remote, b.worktree)).collect();
        assert_eq!(
            branches,
            [
                ("main".into(), false, Some("/src/app".into())),
                ("fix".into(), false, None),
                ("origin/feature/x".into(), true, None),
            ]
        );
    }

    #[test]
    fn lists_worktrees_but_not_a_bare_repo_or_a_deleted_folder() {
        let text = "worktree /src/app\0HEAD 46d33b4\0branch refs/heads/main\0\0\
                    worktree /src/app-x\0HEAD 533a3f5\0detached\0\0\
                    worktree /src/gone\0HEAD 533a3f5\0branch refs/heads/old\0prunable gitdir file points nowhere\0\0";
        let trees: Vec<_> = parse_worktrees(text).into_iter().map(|t| (t.path, t.branch, t.main)).collect();
        assert_eq!(trees, [("/src/app".into(), Some("main".into()), true), ("/src/app-x".into(), None, false)]);
        let bare = parse_worktrees("worktree /src/app.git\0bare\0\0worktree /src/app-x\0HEAD 1\0branch refs/heads/x\0\0");
        assert_eq!(bare.iter().map(|t| (t.path.as_str(), t.main)).collect::<Vec<_>>(), [("/src/app-x", false)]);
    }

    #[test]
    fn names_the_clone_folder_as_git_would() {
        assert_eq!(repo_name("https://github.com/svichas/ScriptDeck.git").as_deref(), Some("ScriptDeck"));
        assert_eq!(repo_name("git@github.com:svichas/ScriptDeck.git").as_deref(), Some("ScriptDeck"));
        assert_eq!(repo_name("https://example.com/repo/").as_deref(), Some("repo"));
        assert_eq!(repo_name("git@host:repo.git").as_deref(), Some("repo"));
        assert_eq!(repo_name("https://example.com/.."), None);
        assert_eq!(expand_url("svichas/ScriptDeck"), "https://github.com/svichas/ScriptDeck.git");
        assert_eq!(expand_url("svichas/ScriptDeck.git"), "https://github.com/svichas/ScriptDeck.git");
        assert_eq!(expand_url("git@github.com:svichas/ScriptDeck.git"), "git@github.com:svichas/ScriptDeck.git");
        assert_eq!(expand_url("../repo"), "../repo");
    }

    #[test]
    fn new_worktrees_go_beside_the_main_one_under_a_free_name() {
        let base = temp("free-path");
        let app = base.join("app");
        assert_eq!(free_path(&app, "feature/login"), base.join("app-feature-login"));
        fs::create_dir(base.join("app-feature-login")).unwrap();
        assert_eq!(free_path(&app, "feature/login"), base.join("app-feature-login-2"));
        assert_eq!(free_path(&base.join("app.git"), "x"), base.join("app-x"));
    }

    #[test]
    fn refuses_names_git_would_read_as_options() {
        let (app, repo) = repo("dashes");
        assert!(repo.switch("--detach").is_err());
        assert!(repo.switch("  ").is_err());
        assert!(repo.add_worktree("-f", None).is_err());
        assert!(repo.add_worktree("x", Some("--orphan")).is_err());
        assert!(repo.remove_worktree("relative/path").is_err());
        assert!(clone("--upload-pack=touch /tmp/x", &app).is_err());
        assert_eq!(git(&app, &["branch", "--format=%(refname:short)"]), "main\n");
    }

    #[test]
    fn commits_only_the_chosen_files_and_a_rename_as_a_whole() {
        let (app, repo) = repo("commit");
        fs::write(app.join("b.txt"), "b\n").unwrap();
        fs::write(app.join("*.txt"), "a star\n").unwrap();
        git(&app, &["add", "b.txt"]);
        git(&app, &["commit", "-qm", "b"]);
        git(&app, &["mv", "a.txt", "moved.txt"]);
        fs::write(app.join("b.txt"), "changed\n").unwrap();
        fs::write(app.join("new.txt"), "new\n").unwrap();
        fs::create_dir(app.join("zdocs")).unwrap();
        fs::write(app.join("zdocs/d.txt"), "d\n").unwrap();
        fs::write(app.join("staged.txt"), "staged but left out\n").unwrap();
        git(&app, &["add", "staged.txt"]);

        repo.commit("Pick some", &paths(&["moved.txt", "new.txt", "*.txt", "zdocs/"])).unwrap();
        assert_eq!(committed(&app), "A\t*.txt\nR100\ta.txt\tmoved.txt\nA\tnew.txt\nA\tzdocs/d.txt\n");
        let left: Vec<_> = repo.status().unwrap().files.into_iter().map(|f| (f.kind, f.path)).collect();
        assert_eq!(left, [('M', "b.txt".into()), ('A', "staged.txt".into())]);

        assert!(repo.commit(" ", &paths(&["b.txt"])).is_err());
        assert!(repo.commit("Gone", &paths(&["new.txt"])).is_err());
        repo.commit("The rest", &paths(&["b.txt", "staged.txt"])).unwrap();
        assert_eq!(committed(&app), "M\tb.txt\nA\tstaged.txt\n");
        assert_eq!(repo.status().unwrap().summary.changes, 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_hook_that_stops_a_commit_leaves_the_index_as_it_was() {
        use std::os::unix::fs::PermissionsExt;
        let (app, repo) = repo("hook");
        fs::write(app.join("b.txt"), "one\n").unwrap();
        git(&app, &["add", "b.txt"]);
        git(&app, &["commit", "-qm", "b"]);
        fs::write(app.join("b.txt"), "two\n").unwrap();
        git(&app, &["add", "b.txt"]);
        fs::write(app.join("b.txt"), "three\n").unwrap(); // partly staged
        fs::write(app.join("gone.txt"), "x\n").unwrap();
        git(&app, &["add", "gone.txt"]);
        fs::remove_file(app.join("gone.txt")).unwrap(); // added, then deleted
        fs::write(app.join("new.txt"), "n\n").unwrap();
        fs::write(app.join("other.txt"), "left out\n").unwrap();
        let hook = app.with_file_name("no-hooks").join("pre-commit");
        fs::write(&hook, "#!/bin/sh\necho 'lint failed' >&2\nexit 1\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

        let chosen = paths(&["b.txt", "gone.txt", "new.txt"]);
        assert!(repo.commit("Blocked", &chosen).unwrap_err().contains("lint failed"));
        assert_eq!(git(&app, &["status", "--porcelain"]), "MM b.txt\nAD gone.txt\n A new.txt\n?? other.txt\n");
        fs::remove_file(&hook).unwrap();
        repo.commit("Through", &chosen).unwrap();
        assert_eq!(committed(&app), "M\tb.txt\nA\tnew.txt\n");
        assert_eq!(git(&app, &["status", "--porcelain"]), "?? other.txt\n");
    }

    #[test]
    fn wont_leave_commits_on_a_detached_head_behind() {
        let (app, repo) = repo("detached");
        git(&app, &["switch", "-q", "--detach"]);
        repo.switch("main").unwrap(); // nothing to leave behind yet
        git(&app, &["switch", "-q", "--detach"]);
        fs::write(app.join("d.txt"), "d\n").unwrap();
        repo.commit("On no branch", &paths(&["d.txt"])).unwrap();
        assert!(repo.switch("main").unwrap_err().contains("no branch"));
        assert_eq!(repo.status().unwrap().summary.branch, None);
        repo.switch("keep").unwrap(); // a new branch keeps them
        repo.switch("main").unwrap();
        assert!(repo.branches().unwrap().iter().any(|b| b.name == "keep"));
    }

    #[test]
    fn switches_to_local_remote_and_new_branches() {
        let (app, repo) = repo("switch");
        git(&app.join("../origin"), &["branch", "feature/x"]);
        repo.fetch().unwrap();
        let names: Vec<_> = repo.branches().unwrap().into_iter().map(|b| b.name).collect();
        assert_eq!(names, ["main", "origin/feature/x"]);

        repo.switch("topic").unwrap();
        assert_eq!(repo.status().unwrap().summary.branch.as_deref(), Some("topic"));
        repo.switch("feature/x").unwrap(); // by its local name
        let summary = repo.status().unwrap().summary;
        assert_eq!((summary.branch.as_deref(), summary.upstream.as_deref()), (Some("feature/x"), Some("origin/feature/x")));
        repo.switch("main").unwrap();
        assert_eq!(repo.status().unwrap().summary.branch.as_deref(), Some("main"));
        // A new branch tracks nothing, so its first push publishes it.
        assert_eq!(repo.status().unwrap().summary.upstream.as_deref(), Some("origin/main"));
        repo.switch("topic").unwrap();
        assert_eq!(repo.status().unwrap().summary.upstream, None);

        // origin/main isn't listed while main is, but typed, it means main, never a new branch named like it.
        repo.switch("origin/main").unwrap();
        assert_eq!(repo.status().unwrap().summary.branch.as_deref(), Some("main"));
        assert!(repo.switch("origin/nope").unwrap_err().contains("no branch"));
        assert_eq!(git(&app, &["branch", "--list", "origin/*"]), "");
    }

    #[test]
    fn worktrees_are_made_beside_the_repo_listed_and_removed_only_when_clean() {
        let (app, repo) = repo("worktrees");
        let (dir, branch) = repo.add_worktree("feature/y", Some("main")).unwrap();
        assert_eq!((dir.clone(), branch.as_str()), (app.with_file_name("app-feature-y"), "feature/y"));
        let found = repo.repo().unwrap();
        let listed: Vec<_> = found.worktrees.iter().map(|t| (t.branch.as_deref(), t.main, t.current)).collect();
        assert_eq!(listed, [(Some("main"), true, true), (Some("feature/y"), false, false)]);
        assert_eq!(found.prefix, "");
        fs::create_dir(app.join("sub")).unwrap();
        assert_eq!(Git::new(app.join("sub"), None).prefix().unwrap(), "sub/");
        let feature = repo.branches().unwrap().into_iter().find(|b| b.name == "feature/y").unwrap();
        assert_eq!(feature.worktree.map(PathBuf::from), Some(dir.clone()));
        // Checked out over there, so it can't be here too.
        assert!(repo.switch("feature/y").unwrap_err().contains("worktree"));
        // An existing branch, and a remote one by its local name, into folders of their own.
        git(&app, &["branch", "other"]);
        assert_eq!(repo.add_worktree("other", None).unwrap().0, app.with_file_name("app-other"));
        git(&app.with_file_name("origin"), &["branch", "shared/z"]);
        repo.fetch().unwrap();
        let (dir_z, branch_z) = repo.add_worktree("shared/z", None).unwrap();
        assert_eq!((dir_z, branch_z.as_str()), (app.with_file_name("app-shared-z"), "shared/z"));
        let z = Git::new(app.with_file_name("app-shared-z"), None).status().unwrap().summary;
        assert_eq!(z.upstream.as_deref(), Some("origin/shared/z"));
        // A new branch from a remote one doesn't track it, or pushing could land on it.
        let (dir_r, _) = repo.add_worktree("feature/r", Some("origin/main")).unwrap();
        assert_eq!(Git::new(dir_r, None).status().unwrap().summary.upstream, None);

        // A folder deleted by hand frees its branch.
        fs::remove_dir_all(app.with_file_name("app-other")).unwrap();
        assert_eq!(repo.branches().unwrap().into_iter().find(|b| b.name == "other").unwrap().worktree, None);
        repo.switch("other").unwrap();
        repo.switch("main").unwrap();

        // Ignored files don't count as changes; a new file does.
        fs::create_dir_all(dir.join("node_modules/dep")).unwrap();
        fs::write(dir.join("node_modules/dep/index.js"), "").unwrap();
        fs::write(dir.join("notes.txt"), "unsaved\n").unwrap();
        let path = dir.to_string_lossy();
        assert!(repo.remove_worktree(&path).is_err());
        assert!(dir.join("notes.txt").exists());
        fs::remove_file(dir.join("notes.txt")).unwrap();
        // Nor does another worktree inside it, in a folder every worktree ignores, though it would go too.
        fs::write(app.join(".git/info/exclude"), ".claude/\n").unwrap();
        let inner = dir.join(".claude/worktrees/inner");
        git(&app, &["worktree", "add", "-q", "-b", "inner", inner.to_str().unwrap()]);
        fs::write(inner.join("a.txt"), "unsaved\n").unwrap();
        assert!(repo.remove_worktree(&path).unwrap_err().contains("inside"));
        assert!(inner.join("a.txt").exists());
        git(&app, &["worktree", "remove", "--force", inner.to_str().unwrap()]);
        repo.remove_worktree(&path).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn clones_publishes_a_branch_and_pulls() {
        let (app, _) = repo("sync");
        let origin = app.with_file_name("origin");
        let elsewhere = app.with_file_name("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        let copy = clone(&origin.to_string_lossy(), &elsewhere).unwrap();
        assert_eq!(copy, elsewhere.join("origin"));
        assert!(copy.join("a.txt").exists());
        assert!(clone(&origin.to_string_lossy(), copy.parent().unwrap()).unwrap_err().contains("already exists"));

        let repo = Git::new(&app, None);
        repo.switch("topic").unwrap();
        fs::write(app.join("t.txt"), "t\n").unwrap();
        repo.commit("Topic", &paths(&["t.txt"])).unwrap();
        repo.push().unwrap();
        let summary = repo.status().unwrap().summary;
        assert_eq!((summary.upstream.as_deref(), summary.ahead), (Some("origin/topic"), 0));

        git(&origin, &["switch", "-q", "topic"]);
        fs::write(origin.join("u.txt"), "u\n").unwrap();
        git(&origin, &["add", "u.txt"]);
        git(&origin, &["commit", "-qm", "upstream"]);
        git(&origin, &["switch", "-q", "main"]); // a pushed-to repository can't have the branch checked out
        repo.fetch().unwrap();
        assert_eq!(repo.status().unwrap().summary.behind, 1);
        repo.pull().unwrap();
        assert_eq!(repo.status().unwrap().summary.behind, 0);
        assert!(app.join("u.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn finds_the_login_shells_path() {
        assert!(login_path().is_some_and(|path| path.split(':').any(|dir| dir == "/usr/bin")));
    }
}
