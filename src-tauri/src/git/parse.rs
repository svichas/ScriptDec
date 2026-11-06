//! Reading what git prints, and checking what goes to it.

use super::{Branch, Summary, Worktree};
use std::collections::HashSet;

/// `git status --porcelain=v2 --branch -z`: the branch and its upstream, and how many files changed.
pub fn parse_status(text: &str) -> Summary {
    let mut summary = Summary::default();
    let mut compared = false; // an upstream with no ahead and behind counts is gone
    let mut entries = text.split('\0');
    while let Some(entry) = entries.next() {
        let Some(header) = entry.strip_prefix("# ") else {
            match entry.chars().next() {
                Some('1' | 'u' | '?') => summary.changes += 1,
                // A rename's entry is followed by the name it had.
                Some('2') => {
                    summary.changes += 1;
                    entries.next();
                }
                _ => {}
            }
            continue;
        };
        let (key, value) = header.split_once(' ').unwrap_or((header, ""));
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
    }
    summary.gone = summary.upstream.is_some() && !compared;
    summary
}

/// `git for-each-ref` lines of a refname and a worktree path, separated by NUL. Local branches, then remote ones
/// you don't have locally.
pub fn parse_branches(text: &str) -> Vec<Branch> {
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
pub fn parse_worktrees(text: &str) -> Vec<Worktree> {
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

/// Git's message, or a failed hook's output. Long output ends with what went wrong, so the end is kept.
pub fn failure(output: &str) -> String {
    let lines: Vec<&str> = output.trim().lines().collect();
    match lines.len() {
        0 => "git failed".into(),
        1..=20 => lines.join("\n"),
        n => format!("…\n{}", lines[n - 20..].join("\n")),
    }
}

/// `owner/repo` is short for that repository on GitHub.
pub fn expand_url(url: &str) -> String {
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
pub fn repo_name(url: &str) -> Option<String> {
    let last = url.trim_end_matches(['/', '\\']).rsplit(['/', '\\', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty() && name != "." && name != "..").then(|| name.to_string())
}

pub fn branch_name(name: &str) -> Result<&str, String> {
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
pub fn local_name(remote: &str) -> &str {
    remote.split_once('/').map_or(remote, |(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_branch_its_upstream_and_how_many_files_changed() {
        let text = [
            "# branch.oid 46d33b4f41f4dbb10aed702b0c76f4e5188da42e",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -3",
            "1 .M N... 100644 100644 100644 6178079 6178079 src/a file.js",
            "2 R. N... 100644 100644 100644 2fa992c 2fa992c R100 renamed.js",
            "old name.js",
            "u UU N... 100644 100644 100644 100644 a b c both.js",
            "? untracked dir/",
            "",
        ]
        .join("\0");
        let summary = Summary {
            branch: Some("main".into()),
            commit: Some("46d33b4".into()),
            upstream: Some("origin/main".into()),
            gone: false,
            ahead: 2,
            behind: 3,
            changes: 4,
        };
        assert_eq!(parse_status(&text), summary);
    }

    #[test]
    fn a_detached_head_a_repo_with_no_commits_and_a_deleted_upstream() {
        let detached = parse_status("# branch.oid 46d33b4f41f4\0# branch.head (detached)\0");
        assert_eq!((detached.branch, detached.commit.as_deref()), (None, Some("46d33b4")));
        let fresh = parse_status("# branch.oid (initial)\0# branch.head main\0");
        assert_eq!((fresh.branch.as_deref(), fresh.commit, fresh.gone), (Some("main"), None, false));
        // Deleted on the remote: git names the upstream but can't compare with it.
        assert!(parse_status("# branch.oid 46d33b4f41f4\0# branch.head x\0# branch.upstream origin/x\0").gone);
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
        let branches: Vec<_> = parse_branches(&text).into_iter().map(|b| (b.name, b.remote, b.worktree)).collect();
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
    fn keeps_long_output_to_its_end() {
        assert_eq!(failure("  \n"), "git failed");
        assert_eq!(failure("fatal: no\n"), "fatal: no");
        let long: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        assert!(failure(&long).starts_with("…\nline 11\n") && failure(&long).ends_with("line 30"));
    }
}
