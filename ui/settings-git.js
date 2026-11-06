import { call, h, icon, invoke, saveProject, toast } from './shared.js';
import { branchChip, pull, pullLabel, shortPath, switcher } from './git.js';
import { group, setting } from './settings-ui.js';

/**
 * Settings' git: the branch chip in a project's header with its switcher, the project's worktrees at the bottom of
 * its page, and the Clone page. settings.js lends what it shares through initGit: { render, load, select, projects }.
 */
let app;
export const initGit = (settings) => { app = settings; };

let repo = null; // the selected project's repository, null outside one
let repoFor = null; // the project `repo` is for
let loads = 0; // the newest loadRepo, so an older one finishing late is dropped
let busy = null; // the git command running, as its button's label
let picker = null; // the open switcher
let removing = null; // the worktree whose Remove waits for its second click
let base = null; // where a new worktree's branch starts; null is the current branch
let treesOpen = false; // the Worktrees section: secondary, so folded away until opened
let reveal = false; // scroll to the Worktrees section once it's drawn
let cloneParent = null; // the folder chosen to clone into
// Kept across renders, with their text and focus.
const worktreeInput = h('input', {
  placeholder: 'Branch, new or existing', 'aria-label': 'Branch for the new worktree', list: 'worktree-branches',
  spellcheck: 'false', autocomplete: 'off',
});
const cloneInput = h('input', {
  class: 'mono url', placeholder: 'https://github.com/owner/repo, or owner/repo', 'aria-label': 'Repository URL',
  spellcheck: 'false', autocomplete: 'off',
});

// A click anywhere but the switcher or its chip puts the switcher away, as a menu goes.
document.addEventListener('mousedown', (event) => {
  if (picker && !event.target.closest('.dropdown, .page-head .branch')) {
    picker = null;
    app.render();
  }
});

/** Called when another page is picked. */
export function resetGit() {
  picker = null;
  removing = null;
}

/** Opens the Worktrees section and scrolls to it, once the project's page is drawn. */
export function revealWorktrees() {
  treesOpen = true;
  reveal = true;
}

/** The project's repository. Resolves to false when a newer load overtook it. */
export async function loadRepo(projectId) {
  const load = ++loads;
  const found = projectId && await invoke('git_repo', { projectId }).catch(() => null);
  if (load !== loads) return false;
  if (repoFor !== projectId) {
    // Another project's page: its own new worktree.
    worktreeInput.value = '';
    base = null;
    picker = null;
  }
  [repo, repoFor] = [found ?? null, projectId];
  return true;
}

/** A git command, one at a time. Says `done` when it worked; call() has said why when it didn't. */
async function gitAction(label, run, done) {
  busy = label;
  app.render();
  try {
    await run();
    if (done) toast(done, 'success');
    return true;
  } catch {
    return false;
  } finally {
    busy = null;
    await app.load();
  }
}

function togglePicker(project) {
  picker = picker ? null : switcher(project, {
    onChange: () => { picker = null; app.load(); },
    onClose: () => { picker = null; app.render(); },
  });
  app.render();
  picker?.focus();
}

/** For the project's header, beside its folder: none outside a repository. */
export function gitChip(project) {
  if (repoFor !== project.id || !repo) return null;
  return branchChip(project, repo.status, { open: Boolean(picker), onClick: () => togglePicker(project) });
}

/** The switcher, open under the chip, with Pull and a way to the worktrees. */
export function gitDropdown(project) {
  if (!picker || repoFor !== project.id || !repo) return null;
  const label = pullLabel(repo.status);
  const manage = () => {
    picker = null;
    revealWorktrees();
    app.render();
  };
  return h('div', { class: 'picker-box dropdown' },
    picker.el,
    h('div', { class: 'picker-foot' },
      label && h('button', {
        class: 'btn', disabled: busy != null, onClick: () => gitAction('Pulling…', () => pull(project.id)),
      }, busy === 'Pulling…' ? busy : label),
      h('span', { class: 'grow' }),
      h('button', { class: 'link', onClick: manage }, 'Manage worktrees…')));
}

function removeButton(project, tree) {
  const armed = removing === tree.path;
  const name = tree.branch ?? 'detached';
  return h('button', {
    class: `btn ${armed ? 'danger' : ''}`,
    disabled: busy != null,
    'data-tip': armed ? 'Click again to delete it' : 'Deletes its folder. Git refuses while it has changes.',
    onClick: () => {
      removing = armed ? null : tree.path;
      if (!armed) return app.render();
      gitAction('Removing…', () => call('git_remove_worktree', { projectId: project.id, path: tree.path }),
        `Removed the ${name} worktree`);
    },
  }, armed ? 'Delete its folder?' : 'Remove');
}

/** Its scripts run in this worktree from now on; what's running elsewhere keeps running. */
function switchTo(project, tree) {
  project.worktree = tree.dir === project.path ? null : tree.dir;
  gitAction('Switching…', () => saveProject(project), `Scripts now run in the ${tree.branch ?? 'detached'} worktree`);
}

/** The repository's worktrees, at the bottom of the project's page and folded away until opened. */
export function worktreesSection(project) {
  if (repoFor !== project.id || !repo) return null;
  const main = repo.worktrees.find((tree) => tree.main) ?? repo.worktrees[0];
  const running = repo.worktrees.reduce((sum, tree) => sum + tree.running, 0);
  const rows = repo.worktrees.map((tree) => h('div', { class: `row worktree ${tree.current ? 'current' : ''}` },
    icon('folder'),
    h('div', { class: 'meta line' },
      h('span', { class: 'title' }, tree.branch ?? 'detached HEAD'),
      h('span', { class: 'sub', title: tree.path }, shortPath(tree, main))),
    tree.main && h('span', { class: 'tag' }, 'main'),
    tree.running > 0 && h('span', { class: 'tag on' }, `${tree.running} running`),
    tree.current
      ? h('span', { class: 'note', 'data-tip': "This project's scripts run in this folder" }, 'In use')
      : h('button', {
        class: 'btn', disabled: busy != null, 'data-tip': "Run this project's scripts in this folder",
        onClick: () => switchTo(project, tree),
      }, 'Use'),
    !tree.main && removeButton(project, tree)));

  const from = h('select', {
    'aria-label': 'Start a new branch from', 'data-tip': 'Where a new branch starts',
    onChange: (event) => { base = event.target.value; },
  },
  h('option', { value: '' }, 'HEAD'),
  repo.branches.map((b) => h('option', { value: b.name }, b.name)));
  from.value = base ?? repo.status.branch ?? '';
  const create = async (event) => {
    event.preventDefault();
    const branch = worktreeInput.value.trim();
    const done = await gitAction('Creating…',
      () => call('git_add_worktree', { projectId: project.id, branch, base: from.value || null }),
      `Made a worktree for ${branch}. Scripts now run there`);
    if (!done) return;
    worktreeInput.value = '';
    base = null;
  };

  const details = h('details', {
    class: 'worktrees', id: 'worktrees', open: treesOpen, onToggle: (event) => { treesOpen = event.target.open; },
  },
  h('summary', {},
    icon('chevron'),
    h('h2', {}, `Worktrees (${repo.worktrees.length})`),
    running > 0 && h('span', { class: 'tag on' }, `${running} running`)),
  h('div', { class: 'group trees' }, rows),
  h('form', { class: 'inline worktree-form', onSubmit: create },
    worktreeInput,
    h('datalist', { id: 'worktree-branches' }, repo.branches.map((b) => h('option', { value: b.name }))),
    h('span', { class: 'note' }, 'from'),
    from,
    h('button', { class: 'btn', disabled: busy != null }, busy === 'Creating…' ? busy : 'New Worktree')),
  h('p', { class: 'note' },
    'Each worktree is a folder of its own with a branch checked out, so the same scripts can run on two branches '
    + 'at once. Scripts run in the one in use, and keep running when you switch. '
    + `New ones go beside ${main.path.split(/[\\/]/).pop()} and need Install first.`));
  if (reveal) {
    reveal = false;
    queueMicrotask(() => details.scrollIntoView({ block: 'start', behavior: 'smooth' }));
  }
  return h('section', {}, details);
}

export function renderClone(head, body) {
  const parent = cloneParent ?? app.projects().at(-1)?.path.replace(/[\\/][^\\/]*$/, '') ?? null;
  const choose = async () => {
    const path = await call('pick_folder');
    if (path) { cloneParent = path; app.render(); }
  };
  const clone = async (event) => {
    event.preventDefault();
    if (!parent) return choose();
    let added;
    const done = await gitAction('Cloning…', async () => {
      added = await call('git_clone', { url: cloneInput.value, parent });
    });
    if (!done) return;
    toast(`Cloned ${added.name}`, 'success');
    cloneInput.value = '';
    app.select(added.id);
  };
  head.replaceChildren(h('h1', {}, 'Clone a Repository'));
  body.replaceChildren(h('form', { class: 'clone', onSubmit: clone },
    group('Repository',
      setting('URL', 'HTTPS or SSH, signed in as git is in your terminal.', cloneInput),
      setting('Into', parent ?? 'Choose a folder', h('button', { class: 'btn', type: 'button', onClick: choose }, 'Choose…'))),
    h('div', { class: 'inline actions' },
      h('button', { class: 'btn primary', disabled: busy != null }, busy ?? 'Clone'),
      h('span', { class: 'note' }, 'It gets a folder of its own in there, and opens as a project.'))));
}
