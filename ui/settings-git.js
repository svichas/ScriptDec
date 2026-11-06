import { call, easeHeight, easeShut, h, icon, saveProject, toast } from './shared.js';
import { combo } from './combo.js';
import { branchChip, knownRepo, pull, pullLabel, readRepo, shortPath, switcher } from './git.js';
import { group, setting } from './settings-ui.js';

/**
 * Settings' git: the branch chip in a project's header with its switcher, the project's worktrees at the bottom of
 * its page, and the Clone page. settings.js lends what it shares through initGit: { render, load, select, projects }.
 */
let app;
export const initGit = (settings) => { app = settings; };

// A project's repository is knownRepo(project.id): drawn at once from the last read, then from the next.
const seen = new Set(); // projects whose git has been drawn, so it fades in only the first time
let busy = null; // the git command running, as its button's label
let picker = null; // the open switcher
let removing = null; // the worktree whose Remove waits for its second click
const removals = new Set(); // worktrees whose folders are being deleted, by path: each on its own, so several go at once
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
const from = combo({
  label: 'Start the new branch from', glyph: 'branch', placeholder: 'Find a branch…',
  onPick: (value) => { base = value; },
});

// A click anywhere but the switcher or its chip puts the switcher away, as a menu goes.
document.addEventListener('mousedown', (event) => {
  if (picker && !event.target.closest('.dropdown, .page-head .branch')) {
    picker = null;
    app.render();
  }
});

/** Called when another page is picked: its own switcher, and its own new worktree. */
export function resetGit() {
  picker = null;
  removing = null;
  worktreeInput.value = '';
  base = null;
}

/** Opens the Worktrees section and scrolls to it, once the project's page is drawn. */
export function revealWorktrees() {
  treesOpen = true;
  reveal = true;
}

/** Reads the project's repository afresh; the next render draws it. */
export async function loadRepo(projectId) {
  if (projectId) await readRepo(projectId, { fresh: true }).catch(() => {});
}

/** What the page draws for a project's git: as last read, so it doesn't blink while git is asked again. */
const repoOf = (project) => knownRepo(project.id);

/** Fades `el` in the first time the project's git is drawn, rather than popping in. */
function firstTime(project, el) {
  if (el && !seen.has(project.id)) el.classList.add('appear');
  return el;
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

/** For the project's header, beside its folder: none outside a repository, a placeholder until the first read. */
export function gitChip(project) {
  if (!project.isRepo) return null;
  const repo = repoOf(project);
  if (repo === undefined) return h('span', { class: 'branch placeholder', 'aria-hidden': 'true' }, h('span', { class: 'bar' }));
  if (!repo) return null;
  return firstTime(project,
    branchChip(project, repo.status, { open: Boolean(picker), onClick: () => togglePicker(project) }));
}

/** The switcher, open under the chip, with Pull and a way to the worktrees. */
export function gitDropdown(project) {
  const repo = repoOf(project);
  if (!picker || !repo) return null;
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
        class: 'btn', disabled: busy != null, 'data-tip': 'Fast-forward only',
        onClick: () => gitAction('Pulling…', () => pull(project.id)),
      }, icon('pull'), busy === 'Pulling…' ? busy : label),
      h('span', { class: 'grow' }),
      h('button', { class: 'link', onClick: manage }, icon('worktree'), 'Manage worktrees…')));
}

function removeButton(project, tree) {
  const armed = removing === tree.path;
  return h('button', {
    class: `btn ${armed ? 'danger' : ''}`,
    disabled: busy != null,
    'data-tip': armed ? 'Click again to delete it' : 'Deletes its folder. Git refuses while it has changes.',
    onClick: () => {
      removing = armed ? null : tree.path;
      if (armed) removeWorktree(project, tree);
      else app.render();
    },
  }, icon('trash'), armed ? 'Delete its folder?' : 'Remove');
}

/** Deletes a worktree's folder, slow with node_modules in it. Only its row waits, saying so; the rest carries on. */
async function removeWorktree(project, tree) {
  removals.add(tree.path);
  app.render();
  try {
    await call('git_remove_worktree', { projectId: project.id, path: tree.path });
    toast(`Removed the ${tree.branch ?? 'detached'} worktree`, 'success');
  } catch {
    // call() has said why
  }
  // Read again before the row stops waiting, or a removed one would be back for a moment.
  await app.load().finally(() => {
    removals.delete(tree.path);
    app.render();
  });
}

/** A worktree row's end: Use and Remove, or that it's in use, or that it's going. */
function treeEnd(project, tree) {
  if (removals.has(tree.path)) {
    return h('span', { class: 'note removing', role: 'status' },
      h('span', { class: 'spinner', 'aria-hidden': 'true' }), 'Removing…');
  }
  return [
    tree.current
      ? h('span', { class: 'note in-use', 'data-tip': "This project's scripts run in this folder" }, icon('check'), 'In use')
      : h('button', {
        class: 'btn', disabled: busy != null, 'data-tip': "Run this project's scripts in this folder",
        onClick: () => switchTo(project, tree),
      }, icon('swap'), 'Use'),
    !tree.main && removeButton(project, tree),
  ];
}

/** Its scripts run in this worktree from now on; what's running elsewhere keeps running. */
function switchTo(project, tree) {
  project.worktree = tree.dir === project.path ? null : tree.dir;
  gitAction('Switching…', () => saveProject(project), `Scripts now run in the ${tree.branch ?? 'detached'} worktree`);
}

/** Opens or folds the Worktrees section, sliding rather than jumping. */
function toggleTrees(details, body) {
  treesOpen = !details.open;
  if (treesOpen) {
    details.open = true;
    easeHeight(body, 0);
  } else {
    easeShut(body, () => { details.open = false; });
  }
}

/** Rows the size of real ones, until the first read: the section is there, and can be scrolled to, at once. */
const placeholderRows = () => [58, 44, 66].map((width) => h('div', { class: 'row skeleton', 'aria-hidden': 'true' },
  h('span', { class: 'bar', style: `width: ${width}%` })));

/** The section's frame: a summary that folds it, and what it folds. */
function treesFrame(summary, body) {
  const details = h('details', { class: 'worktrees', id: 'worktrees', open: treesOpen },
    // The click is ours, so the section can slide; Enter and Space on the summary come here too.
    h('summary', { onClick: (event) => { event.preventDefault(); toggleTrees(details, body); } },
      icon('chevron'), icon('worktree'), summary),
    body);
  if (reveal) {
    reveal = false;
    queueMicrotask(() => details.scrollIntoView({ block: 'start', behavior: 'smooth' }));
  }
  return h('section', {}, details);
}

/**
 * The repository's worktrees, at the bottom of the project's page and folded away until opened. Laid out before git
 * answers, from the project being in a repository at all, so "Manage worktrees…" has somewhere to scroll to.
 */
export function worktreesSection(project) {
  if (!project.isRepo) return null;
  const repo = repoOf(project);
  if (repo === undefined) {
    return treesFrame(h('h2', {}, 'Worktrees'), h('div', { class: 'trees-body' }, h('div', { class: 'group trees' }, placeholderRows())));
  }
  if (!repo) return null;
  seen.add(project.id);
  const main = repo.worktrees.find((tree) => tree.main) ?? repo.worktrees[0];
  const running = repo.worktrees.reduce((sum, tree) => sum + tree.running, 0);
  const rows = repo.worktrees.map((tree) => h('div', {
    class: `row worktree ${tree.current ? 'current' : ''} ${removals.has(tree.path) ? 'removing' : ''}`,
  },
    icon('worktree'),
    h('div', { class: 'meta line' },
      h('span', { class: 'title' }, tree.branch ?? 'detached HEAD'),
      h('span', { class: 'sub', title: tree.path }, shortPath(tree, main))),
    tree.main && h('span', { class: 'tag' }, 'main'),
    tree.running > 0 && h('span', { class: 'tag on' }, `${tree.running} running`),
    treeEnd(project, tree)));

  // HEAD, then every branch, the one checked out here chosen until another is.
  const start = base ?? repo.status.branch ?? '';
  from.set([{ value: '', text: 'HEAD' }, ...repo.branches.map((b) => ({ value: b.name, text: b.name }))], start);
  const create = async (event) => {
    event.preventDefault();
    const branch = worktreeInput.value.trim();
    const done = await gitAction('Creating…',
      () => call('git_add_worktree', { projectId: project.id, branch, base: start || null }),
      `Made a worktree for ${branch}. Scripts now run there`);
    if (!done) return;
    worktreeInput.value = '';
    base = null;
  };

  const body = h('div', { class: 'trees-body' },
    h('div', { class: 'group trees' }, rows),
    h('form', { class: 'inline worktree-form', onSubmit: create },
      worktreeInput,
      h('datalist', { id: 'worktree-branches' }, repo.branches.map((b) => h('option', { value: b.name }))),
      h('span', { class: 'note' }, 'from'),
      from.el,
      h('button', { class: 'btn', disabled: busy != null }, icon('plus'), busy === 'Creating…' ? busy : 'New Worktree')),
    h('p', { class: 'note' },
      'Each worktree is a folder of its own with a branch checked out, so the same scripts can run on two branches '
      + 'at once. Scripts run in the one in use, and keep running when you switch. '
      + `New ones go beside ${main.path.split(/[\\/]/).pop()} and need Install first.`));
  return treesFrame([
    h('h2', {}, `Worktrees (${repo.worktrees.length})`),
    running > 0 && h('span', { class: 'tag on' }, `${running} running`),
  ], body);
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
      setting('Into', parent ?? 'Choose a folder',
        h('button', { class: 'btn', type: 'button', onClick: choose }, icon('folder'), 'Choose…'))),
    h('div', { class: 'inline actions' },
      h('button', { class: 'btn primary', disabled: busy != null }, icon('download'), busy ?? 'Clone'),
      h('span', { class: 'note' }, 'It gets a folder of its own in there, and opens as a project.'))));
}
