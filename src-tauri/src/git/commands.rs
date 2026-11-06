//! What the windows ask of git, for a project in the worktree it's switched to.

use super::{Git, Repo, Summary};
use crate::{
    config::Project,
    node,
    sessions::{SessionView, Status},
    App,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Manager};

/// Git in the project's folder. Its hooks get the project's Node, as its scripts do.
fn project_git(state: &App, id: &str) -> Result<(Project, Git), String> {
    let project = state.project(id)?;
    let dir = project.dir();
    let node = node::detect(Path::new(dir), project.node_version.as_deref(), None);
    let git = Git::new(dir, node.resolved.as_deref().and_then(node::bin_dir));
    Ok((project, git))
}

fn real(path: &str) -> PathBuf {
    Path::new(path).canonicalize().unwrap_or_else(|_| PathBuf::from(path))
}

/// Running, stopping, or about to restart after a crash.
fn active(session: &SessionView) -> bool {
    matches!(session.status, Status::Running | Status::Stopping) || session.restart_in.is_some()
}

/// Each project's branch and changes, for the menu bar. Projects outside a repository aren't in it.
#[tauri::command]
pub async fn git_overview(app: AppHandle) -> HashMap<String, Summary> {
    let projects = app.state::<App>().config.lock().unwrap().projects.clone();
    std::thread::scope(|scope| {
        let reads: Vec<_> = projects
            .iter()
            .filter(|project| super::is_repo(Path::new(project.dir())))
            .map(|project| scope.spawn(move || (project.id.clone(), Git::new(project.dir(), None).status())))
            .collect();
        reads.into_iter().filter_map(|read| read.join().ok()).filter_map(|(id, status)| Some((id, status.ok()?))).collect()
    })
}

/// A project's repository, and the scripts running in each of its worktrees; None when it isn't in one.
#[tauri::command]
pub async fn git_repo(app: AppHandle, project_id: String) -> Result<Option<Repo>, String> {
    let state = app.state::<App>();
    let project = state.project(&project_id)?;
    if !super::is_repo(Path::new(project.dir())) {
        return Ok(None);
    }
    let mut repo = Git::new(project.dir(), None).repo()?;
    for tree in &mut repo.worktrees {
        // By the name it was added with, so switching back there clears the switch.
        if real(&tree.dir) == real(&project.path) {
            tree.dir = project.path.clone();
        }
    }
    // Each script counts for the innermost worktree it runs in: one can sit inside another, like .claude/worktrees.
    let trees: Vec<PathBuf> = repo.worktrees.iter().map(|tree| real(&tree.path)).collect();
    for session in state.sessions.list().iter().filter(|s| active(s)) {
        let cwd = real(&session.cwd);
        let inside = trees.iter().enumerate().filter(|(_, tree)| cwd.starts_with(tree));
        if let Some((i, _)) = inside.max_by_key(|(_, tree)| tree.components().count()) {
            repo.worktrees[i].running += 1;
        }
    }
    Ok(Some(repo))
}

#[tauri::command]
pub async fn git_switch(app: AppHandle, project_id: String, branch: String) -> Result<(), String> {
    project_git(&app.state::<App>(), &project_id)?.1.switch(&branch)
}

/// How many commits came in.
#[tauri::command]
pub async fn git_pull(app: AppHandle, project_id: String) -> Result<u32, String> {
    project_git(&app.state::<App>(), &project_id)?.1.pull()
}

/// Checks a branch out in a new worktree beside the repository, and switches the project to it.
#[tauri::command]
pub async fn git_add_worktree(
    app: AppHandle,
    project_id: String,
    branch: String,
    base: Option<String>,
) -> Result<(), String> {
    let state = app.state::<App>();
    let (_, git) = project_git(&state, &project_id)?;
    let dir = git.add_worktree(&branch, base.as_deref())?;
    state.edit(|config| {
        let project = config.projects.iter_mut().find(|p| p.id == project_id).ok_or("Unknown project")?;
        project.worktree = Some(dir.to_string_lossy().into_owned());
        Ok(())
    })
}

/// Deletes a worktree's folder, not while scripts run in it. Projects added from inside it go too, and projects
/// switched to it go back to their own folder.
#[tauri::command]
pub async fn git_remove_worktree(app: AppHandle, project_id: String, path: String) -> Result<(), String> {
    let state = app.state::<App>();
    let (_, git) = project_git(&state, &project_id)?;
    let tree = real(&path);
    let running = state.sessions.list().iter().filter(|s| active(s) && real(&s.cwd).starts_with(&tree)).count();
    if running > 0 {
        let scripts = if running == 1 { "script" } else { "scripts" };
        return Err(format!("Stop the {running} {scripts} running in it first"));
    }
    // Before the folder goes, while its paths still resolve.
    let projects = state.config.lock().unwrap().projects.clone();
    let inside = |dir: &str| real(dir).starts_with(&tree);
    let gone: Vec<String> = projects.iter().filter(|p| inside(&p.path)).map(|p| p.id.clone()).collect();
    let moved: Vec<String> =
        projects.iter().filter(|p| p.worktree.as_deref().is_some_and(inside)).map(|p| p.id.clone()).collect();
    git.remove_worktree(&path)?;
    state.edit(|config| {
        config.projects.retain(|p| !gone.contains(&p.id));
        for project in config.projects.iter_mut().filter(|p| moved.contains(&p.id)) {
            project.worktree = None;
        }
        Ok(())
    })
}

/// Clones a repository into a new folder in `parent` and adds it as a project.
#[tauri::command]
pub async fn git_clone(app: AppHandle, url: String, parent: String) -> Result<Project, String> {
    let dir = super::clone(&url, Path::new(&parent))?;
    let added = crate::add_project(app.state(), dir.to_string_lossy().into_owned());
    added.map_err(|e| format!("Cloned into {}, but it wasn't added: {e}", dir.display()))
}
