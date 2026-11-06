//! A project's git repository, through the git command line: its branches and worktrees, switching between them,
//! pulling, and cloning. A project runs its scripts in one worktree at a time; see `Project::dir`.

pub mod commands;
mod parse;

use parse::{branch_name, expand_url, failure, local_name, parse_branches, parse_status, parse_worktrees, repo_name};
use serde::Serialize;
use std::{
    env,
    ffi::OsString,
    fs,
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
    /// The upstream was deleted on the remote, so there's nothing to pull.
    pub gone: bool,
    pub ahead: u32,
    pub behind: u32,
    /// Files with uncommitted changes, untracked ones included.
    pub changes: usize,
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
    /// The one the project is switched to.
    pub current: bool,
    /// The project's folder in it, which its scripts run in once switched there: `path` itself unless the
    /// project is a folder inside the repository, like apps/web.
    pub dir: String,
    /// Scripts running in it; commands.rs counts them.
    pub running: usize,
}

#[derive(Serialize)]
pub struct Repo {
    /// Of the worktree the project is switched to.
    pub status: Summary,
    pub branches: Vec<Branch>,
    pub worktrees: Vec<Worktree>,
}

/// Whether `dir` is in a git repository, checked without running git: on a Mac without the developer tools,
/// running it opens an installer.
pub fn is_repo(dir: &Path) -> bool {
    dir.ancestors().any(|dir| dir.join(".git").exists())
}

/// The branch checked out in the worktree that holds `dir`, read from its HEAD file without running git. None when
/// HEAD is detached or there's no repository.
pub fn branch_of(dir: &Path) -> Option<String> {
    let top = dir.ancestors().find(|dir| dir.join(".git").exists())?;
    let dot_git = top.join(".git");
    // A linked worktree's .git is a file that names its folder inside the repository's .git.
    let git_dir = match fs::read_to_string(&dot_git) {
        Ok(text) => top.join(text.strip_prefix("gitdir:")?.trim()),
        Err(_) => dot_git,
    };
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    head.trim().strip_prefix("ref: refs/heads/").map(String::from)
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

    /// Runs git in the project's folder and returns its output, or what went wrong.
    fn run(&self, args: &[&str]) -> Result<String, String> {
        let mut cmd = Command::new("git");
        cmd.args(args)
            .current_dir(&self.dir)
            // A password prompt nobody can see would wait forever, so git fails and says why instead.
            .env("GIT_TERMINAL_PROMPT", "0")
            // Reading the status mustn't lock the index while a git command in the user's terminal wants it.
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(Stdio::null());
        if let Some(path) = &self.path {
            cmd.env("PATH", path);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = cmd.output().map_err(|e| format!("Can't run git: {e}"))?;
        let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
        if out.status.success() {
            return Ok(text(&out.stdout));
        }
        Err(failure(&format!("{}\n{}", text(&out.stdout), text(&out.stderr))))
    }

    pub fn status(&self) -> Result<Summary, String> {
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

    /// Everything the branch menu and Settings show, with the project's folder in each worktree.
    pub fn repo(&self) -> Result<Repo, String> {
        let top = self.run(&["rev-parse", "--show-toplevel", "--show-prefix"])?;
        let mut lines = top.lines();
        let (root, prefix) = (lines.next().unwrap_or_default(), lines.next().unwrap_or_default());
        let here = fs::canonicalize(root).ok();
        let mut worktrees = self.worktrees()?;
        for tree in &mut worktrees {
            tree.dir = in_worktree(&tree.path, prefix).to_string_lossy().into_owned();
            tree.current = here.is_some() && fs::canonicalize(&tree.path).ok() == here;
        }
        Ok(Repo { status: self.status()?, branches: self.branches()?, worktrees })
    }

    /// Where the project's folder is in its worktree, like "apps/web/"; empty at the top.
    fn prefix(&self) -> Result<String, String> {
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
        if found.is_some() && self.status()?.branch.is_none() {
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
            // Tracking nothing: it's yours until you publish it from your terminal.
            None => self.run(&["switch", "--no-track", "--create", name]),
        }
        .map(drop)
    }

    /// Fast-forward only, so it never leaves a merge or a rebase half done. Returns how many commits it brought in.
    pub fn pull(&self) -> Result<u32, String> {
        let before = self.run(&["rev-parse", "HEAD"])?;
        self.run(&["pull", "--ff-only"])?;
        let new = self.run(&["rev-list", "--count", &format!("{}..HEAD", before.trim())])?;
        Ok(new.trim().parse().unwrap_or(0))
    }

    /// Checks a branch out in a new folder beside the main worktree: an existing branch, a remote one as a new
    /// local branch tracking it, or a new branch from `base` (HEAD when None). Returns the project's folder in it.
    pub fn add_worktree(&self, name: &str, base: Option<&str>) -> Result<PathBuf, String> {
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
        let tree = free_path(Path::new(&beside.path), local);
        let path = tree.to_string_lossy();
        match (&found, remote) {
            (_, Some(remote)) => self.run(&["worktree", "add", "--track", "-b", local, &path, remote]),
            (Some(_), None) => {
                self.forget_deleted_worktree(local)?;
                self.run(&["worktree", "add", &path, local])
            }
            (None, None) => {
                let base = base.map(branch_name).transpose()?;
                // Tracking nothing: from origin/main, it would track that.
                let args = ["worktree", "add", "--no-track", "-b", local, &path];
                self.run(&[&args[..], base.as_slice()].concat())
            }
        }?;
        Ok(in_worktree(&path, &self.prefix()?))
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

/// The folder at `prefix` in a worktree, like `apps/web/`. Through components, so there's no trailing slash.
fn in_worktree(tree: &str, prefix: &str) -> PathBuf {
    Path::new(tree).join(prefix).components().collect()
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

    /// `app`, a clone of an `origin` with one commit, configured so commits made to set tests up don't use the
    /// user's own identity, signing or hooks.
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

    fn commit(dir: &Path, file: &str) {
        fs::write(dir.join(file), format!("{file}\n")).unwrap();
        git(dir, &["add", file]);
        git(dir, &["commit", "-qm", file]);
    }

    #[test]
    fn new_worktrees_go_beside_the_main_one_under_a_free_name() {
        let base = temp("free-path");
        let app = base.join("app");
        assert_eq!(free_path(&app, "feature/login"), base.join("app-feature-login"));
        fs::create_dir(base.join("app-feature-login")).unwrap();
        assert_eq!(free_path(&app, "feature/login"), base.join("app-feature-login-2"));
        assert_eq!(free_path(&base.join("app.git"), "x"), base.join("app-x"));
        assert_eq!(in_worktree("/src/app-x", "apps/web/"), PathBuf::from("/src/app-x/apps/web"));
        assert_eq!(in_worktree("/src/app-x", ""), PathBuf::from("/src/app-x"));
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
    fn switches_to_local_remote_and_new_branches() {
        let (app, repo) = repo("switch");
        git(&app.join("../origin"), &["branch", "feature/x"]);
        git(&app, &["fetch", "-q"]);
        let names: Vec<_> = repo.branches().unwrap().into_iter().map(|b| b.name).collect();
        assert_eq!(names, ["main", "origin/feature/x"]);

        repo.switch("topic").unwrap();
        assert_eq!(repo.status().unwrap().branch.as_deref(), Some("topic"));
        assert_eq!(repo.status().unwrap().upstream, None); // a new branch tracks nothing
        repo.switch("feature/x").unwrap(); // by its local name
        let summary = repo.status().unwrap();
        assert_eq!((summary.branch.as_deref(), summary.upstream.as_deref()), (Some("feature/x"), Some("origin/feature/x")));
        repo.switch("main").unwrap();
        assert_eq!(repo.status().unwrap().branch.as_deref(), Some("main"));

        // origin/main isn't listed while main is, but typed, it means main, never a new branch named like it.
        repo.switch("topic").unwrap();
        repo.switch("origin/main").unwrap();
        assert_eq!(repo.status().unwrap().branch.as_deref(), Some("main"));
        assert!(repo.switch("origin/nope").unwrap_err().contains("no branch"));
        assert_eq!(git(&app, &["branch", "--list", "origin/*"]), "");
    }

    #[test]
    fn wont_leave_commits_on_a_detached_head_behind() {
        let (app, repo) = repo("detached");
        git(&app, &["switch", "-q", "--detach"]);
        repo.switch("main").unwrap(); // nothing to leave behind yet
        git(&app, &["switch", "-q", "--detach"]);
        commit(&app, "d.txt");
        assert!(repo.switch("main").unwrap_err().contains("no branch"));
        assert_eq!(repo.status().unwrap().branch, None);
        assert_eq!(branch_of(&app), None);
        repo.switch("keep").unwrap(); // a new branch keeps them
        repo.switch("main").unwrap();
        assert!(repo.branches().unwrap().iter().any(|b| b.name == "keep"));
    }

    #[test]
    fn worktrees_are_made_beside_the_repo_listed_and_removed_only_when_clean() {
        let (app, repo) = repo("worktrees");
        let dir = repo.add_worktree("feature/y", Some("main")).unwrap();
        assert_eq!(dir, app.with_file_name("app-feature-y"));
        let listed = repo.repo().unwrap().worktrees;
        let trees: Vec<_> = listed.iter().map(|t| (t.branch.as_deref(), t.main, t.current, t.dir.as_str())).collect();
        let feature = dir.to_string_lossy();
        assert_eq!(trees, [(Some("main"), true, true, app.to_str().unwrap()), (Some("feature/y"), false, false, &*feature)]);
        // Seen from the new one, it's the current one, and the branch reads straight from its files.
        let there = Git::new(&dir, None).repo().unwrap().worktrees;
        assert_eq!(there.iter().map(|t| t.current).collect::<Vec<_>>(), [false, true]);
        assert_eq!((branch_of(&dir).as_deref(), branch_of(&app).as_deref()), (Some("feature/y"), Some("main")));
        // Checked out over there, so it can't be here too.
        assert!(repo.switch("feature/y").unwrap_err().contains("worktree"));

        // A project in a folder of the repository has its folder in each worktree.
        fs::create_dir_all(app.join("apps/web")).unwrap();
        let web = Git::new(app.join("apps/web"), None);
        let in_y: Vec<_> = web.repo().unwrap().worktrees.into_iter().map(|t| PathBuf::from(t.dir)).collect();
        assert_eq!(in_y, [app.join("apps/web"), dir.join("apps/web")]);

        // An existing branch, and a remote one by its local name, into folders of their own.
        git(&app, &["branch", "other"]);
        assert_eq!(repo.add_worktree("other", None).unwrap(), app.with_file_name("app-other"));
        git(&app.with_file_name("origin"), &["branch", "shared/z"]);
        git(&app, &["fetch", "-q"]);
        let dir_z = repo.add_worktree("shared/z", None).unwrap();
        assert_eq!(dir_z, app.with_file_name("app-shared-z"));
        assert_eq!(Git::new(dir_z, None).status().unwrap().upstream.as_deref(), Some("origin/shared/z"));
        // A new branch from a remote one doesn't track it.
        let dir_r = repo.add_worktree("feature/r", Some("origin/main")).unwrap();
        assert_eq!(Git::new(dir_r, None).status().unwrap().upstream, None);

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
    fn clones_and_pulls_fast_forward_only() {
        let (app, repo) = repo("pull");
        let origin = app.with_file_name("origin");
        let elsewhere = app.with_file_name("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        let copy = clone(&origin.to_string_lossy(), &elsewhere).unwrap();
        assert_eq!(copy, elsewhere.join("origin"));
        assert!(copy.join("a.txt").exists());
        assert!(clone(&origin.to_string_lossy(), &elsewhere).unwrap_err().contains("already exists"));

        commit(&origin, "u.txt");
        commit(&origin, "u2.txt");
        assert_eq!(repo.pull().unwrap(), 2);
        assert!(app.join("u.txt").exists());
        assert_eq!(repo.status().unwrap().behind, 0);
        assert_eq!(repo.pull().unwrap(), 0); // nothing new
        // Diverged: a fast-forward can't do it, so nothing changes.
        commit(&origin, "v.txt");
        commit(&app, "w.txt");
        assert!(repo.pull().is_err());
        assert!(!app.join("v.txt").exists());
        assert_eq!(git(&app, &["status", "--porcelain"]), "");
    }

    #[cfg(unix)]
    #[test]
    fn finds_the_login_shells_path() {
        assert!(login_path().is_some_and(|path| path.split(':').any(|dir| dir == "/usr/bin")));
    }
}
