import { call, h, icon, invoke } from './shared.js';

const MAX_BRANCHES = 50;

/** "main", or "detached at 1a2b3c4". `git` is a summary from `git_overview`, or a repo's status. */
export const branchName = (git) => git.branch ?? (git.commit ? `detached at ${git.commit}` : 'no commits yet');

/** "2 to push, 1 to pull", "up to date with origin/main" or "not published". */
export function syncText(git) {
  if (!git.branch) return '';
  if (!git.upstream) return 'not published';
  if (git.gone) return `${git.upstream} was deleted on the remote`;
  const counts = [git.ahead && `${git.ahead} to push`, git.behind && `${git.behind} to pull`].filter(Boolean);
  return counts.length ? `${counts.join(', ')} · ${git.upstream}` : `up to date with ${git.upstream}`;
}

/** Push, or Publish for a branch that isn't on the remote, or no longer is. */
export const pushLabel = (git) => (!git.upstream || git.gone ? 'Publish' : git.ahead ? `Push ${git.ahead}` : 'Push');
/** Null when there's nothing to pull from. */
export const pullLabel = (git) => (!git.upstream || git.gone ? null : git.behind ? `Pull ${git.behind}` : 'Pull');

/** origin/feature/x is feature/x once checked out. */
const localName = (name) => name.slice(name.indexOf('/') + 1);

/**
 * A filterable menu of a project's branches. Picking one switches to it, and a name that matches none creates it.
 * A branch that another worktree has checked out opens that worktree's project instead: onWorktree(project).
 */
export function branchPicker(project, { onSwitched, onWorktree, onClose }) {
  let repo; // undefined while loading, null outside a repository
  let error = null;
  let cursor = 0;
  let rows = [];
  let busy = false;

  const input = h('input', {
    placeholder: 'Switch to or create a branch…',
    'aria-label': 'Branch',
    spellcheck: 'false',
    autocomplete: 'off',
    onInput: () => { cursor = 0; draw(); },
    onKeydown: onKey,
  });
  const list = h('div', { class: 'picker-list', role: 'listbox', 'aria-label': 'Branches' });
  const el = h('div', { class: 'picker' }, input, list);

  invoke('git_repo', { projectId: project.id })
    .then((found) => { repo = found; }, (e) => { error = String(e); })
    .then(draw);

  function choices() {
    const query = input.value.trim();
    const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
    const exact = (b) => b.name === query || (b.remote && localName(b.name) === query);
    const found = repo.branches.filter((b) => terms.every((term) => b.name.toLowerCase().includes(term)));
    // The name typed comes first, so Enter on "main" can't pick a more recent "maintenance".
    found.sort((a, b) => exact(b) - exact(a));
    const taken = repo.branches.some(exact);
    return [...found.slice(0, MAX_BRANCHES).map((branch) => ({ branch })), query && !taken && { create: query }]
      .filter(Boolean);
  }

  function option(row, selected) {
    const current = row.branch?.name === repo.status.branch;
    const elsewhere = !current && row.branch?.worktree;
    return h('div', {
      class: `row option ${selected ? 'selected' : ''}`,
      role: 'option',
      'aria-selected': String(selected),
      title: elsewhere ? `Checked out in ${row.branch.worktree}` : '',
      onClick: () => pick(row),
    },
    icon(row.create ? 'plus' : 'branch'),
    h('div', { class: 'meta' }, h('div', { class: 'title' }, row.create ? `Create branch “${row.create}”` : row.branch.name)),
    (current || elsewhere) && h('span', { class: 'tag' }, current ? 'current' : 'open worktree'));
  }

  function draw() {
    if (error || repo === null) return list.replaceChildren(h('div', { class: 'hint warn' }, error ?? 'Not a git repository.'));
    if (!repo) return list.replaceChildren(h('div', { class: 'hint' }, 'Loading branches…'));
    rows = choices();
    cursor = Math.min(cursor, Math.max(rows.length - 1, 0));
    list.replaceChildren(...(rows.length
      ? rows.map((row, i) => option(row, i === cursor))
      : [h('div', { class: 'hint' }, 'Type a name to create a branch.')]));
  }

  async function pick(row) {
    if (busy) return;
    if (row.branch?.name === repo.status.branch) return onClose();
    busy = true;
    el.classList.add('busy');
    try {
      const worktree = row.branch?.worktree;
      if (worktree) {
        onWorktree(await call('git_open_worktree', { projectId: project.id, path: worktree }));
      } else {
        await call('git_switch', { projectId: project.id, branch: row.create ?? row.branch.name });
        onSwitched();
      }
    } catch { /* call() showed it */ } finally {
      busy = false;
      el.classList.remove('busy');
    }
  }

  function onKey(event) {
    if (event.key === 'Escape') {
      event.stopPropagation();
      onClose();
    } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      cursor = Math.max(0, Math.min(rows.length - 1, cursor + (event.key === 'ArrowDown' ? 1 : -1)));
      draw();
      list.querySelector('.selected')?.scrollIntoView({ block: 'nearest' });
    } else if (event.key === 'Enter' && rows[cursor]) {
      pick(rows[cursor]);
    }
  }

  return { el, focus: () => input.focus() };
}
