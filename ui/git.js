import { call, easeHeight, h, icon, invoke, saveProject, toast } from './shared.js';

const MAX_BRANCHES = 50;

// What git said last about each project's repository, by project id, so the switcher and Settings can draw it at
// once and then what git says now, instead of a blank that jumps when it fills. Each project's read on its way
// too, so a read started on hover is the one the click then uses.
const known = new Map(); // project id → { repo, ticket }
const reading = new Map(); // project id → { read, ticket }
let tickets = 0;

/** A project's repository as last read: undefined before the first read, null outside a repository. */
export const knownRepo = (projectId) => known.get(projectId)?.repo;

/**
 * Reads a project's repository and remembers it. A read already on its way is shared, unless `fresh` asks for one
 * that starts now, as after a change. A read never overwrites one that started after it, however they finish.
 */
export function readRepo(projectId, { fresh = false } = {}) {
  const going = reading.get(projectId);
  if (going && !fresh) return going.read;
  const ticket = ++tickets;
  const newest = () => reading.get(projectId)?.ticket === ticket;
  const read = invoke('git_repo', { projectId }).then(
    (repo) => {
      // An earlier read that lands first is still newer than what's known, as when two removals end close together.
      if (ticket > (known.get(projectId)?.ticket ?? 0)) known.set(projectId, { repo, ticket });
      if (newest()) reading.delete(projectId);
      return repo;
    },
    (error) => {
      if (newest()) reading.delete(projectId);
      throw error;
    },
  );
  reading.set(projectId, { read, ticket });
  return read;
}

/** "main", or "detached at 1a2b3c4". `git` is a summary from `git_overview`, or a repository's status. */
export const branchName = (git) => git.branch ?? (git.commit ? `detached at ${git.commit}` : 'no commits yet');

/** "2 to pull from origin/main", "up to date with origin/main", or why there's nothing to pull. */
export function syncText(git) {
  if (!git.branch) return '';
  if (!git.upstream) return 'not on a remote';
  if (git.gone) return `${git.upstream} was deleted on the remote`;
  return git.behind ? `${git.behind} to pull from ${git.upstream}` : `up to date with ${git.upstream}`;
}

/** Null when there's nothing to pull from. */
export const pullLabel = (git) => (!git.upstream || git.gone ? null : git.behind ? `Pull ${git.behind}` : 'Pull');

/** Pulls into the worktree the project is on, and says what came in. Rejects, with the error shown, if it can't. */
export async function pull(projectId) {
  const count = await call('git_pull', { projectId });
  toast(count ? `Pulled ${count} commit${count === 1 ? '' : 's'}` : 'Already up to date', count ? 'success' : 'info');
}

/**
 * The branch a project is on, a dot for uncommitted changes and what's left to pull. The worktree icon in place of
 * the branch, and a tint, say it's switched to a worktree other than its own folder. `git` is its summary or status.
 */
export function branchChip(project, git, { open, onClick }) {
  const away = project.dir !== project.path;
  const changes = git.changes && `${git.changes} uncommitted change${git.changes === 1 ? '' : 's'}`;
  const where = away ? `In the worktree at ${project.dir}, on` : 'On';
  const about = [`${where} ${branchName(git)}`, changes, syncText(git)].filter(Boolean).join(' · ');
  return h('button', {
    class: `branch ${away ? 'away' : ''} ${open ? 'open' : ''}`,
    'data-tip': `${about}. Click to switch worktree or branch.`,
    'aria-label': `${about}. Switch worktree or branch`,
    'aria-expanded': String(open),
    // Pointing at it starts the read, so the switcher usually opens with its lists already there.
    onMouseenter: () => readRepo(project.id).catch(() => {}),
    onClick,
  },
  icon(away ? 'worktree' : 'branch'),
  h('span', { class: 'name' }, branchName(git)),
  git.changes > 0 && h('span', { class: 'dirty', 'aria-hidden': 'true' }),
  git.behind > 0 && h('span', { class: 'sync' }, `↓${git.behind}`));
}

/** A worktree's folder as you'd look for it, from the folder the repository is in: app-feature-x. */
export function shortPath(tree, main) {
  const parent = main.path.replace(/[\\/][^\\/]*$/, '');
  return tree.path.startsWith(parent) && /[\\/]/.test(tree.path[parent.length] ?? '')
    ? tree.path.slice(parent.length + 1)
    : tree.path;
}

/** origin/feature/x is feature/x once checked out. */
const localName = (name) => name.slice(name.indexOf('/') + 1);

// The switcher's Worktrees group, collapsed until opened: they'd crowd out the branches. Typing opens it too.
let treesOpen = false;

/**
 * The menu behind a project's branch chip. Picking a worktree switches the project there: its scripts run in that
 * folder from then on, and what's already running keeps running where it is. Picking a branch checks it out in the
 * worktree the project is on. A name that matches nothing makes a worktree for it, or a branch here.
 * `onChange` runs after any of them, for the window to reload.
 */
export function switcher(project, { onChange, onClose }) {
  // As last read, drawn at once; then as read now. Undefined until the first read, null outside a repository.
  let repo = knownRepo(project.id);
  let error = null;
  let busy = null; // what's being done, shown instead of the list
  let cursor = 0;
  let rows = [];

  const input = h('input', {
    placeholder: 'Find or create a worktree or branch…',
    'aria-label': 'Worktree or branch',
    spellcheck: 'false',
    autocomplete: 'off',
    onInput: () => { cursor = 0; draw(); },
    onKeydown: onKey,
  });
  const list = h('div', { class: 'picker-list', role: 'listbox', 'aria-label': 'Worktrees and branches' });
  const el = h('div', { class: 'picker' }, input, list);

  readRepo(project.id).then(
    (found) => { repo = found; },
    // What was last read still holds; only with nothing to show is the error the list.
    (e) => { if (repo === undefined) error = String(e); },
  ).then(() => draw({ ease: true }));

  /** [heading, rows] for each kind that has something to show. */
  function sections() {
    const query = input.value.trim();
    const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
    const matches = (text) => terms.every((term) => text.toLowerCase().includes(term));
    // The name typed comes first, so Enter on "main" can't pick a more recent "maintenance".
    const exactFirst = (exact) => (a, b) => exact(b) - exact(a);
    const isBranch = (b) => b.name === query || (b.remote && localName(b.name) === query);
    const main = repo.worktrees.find((tree) => tree.main) ?? repo.worktrees[0];
    const trees = repo.worktrees
      .filter((tree) => matches(`${tree.branch ?? ''} ${shortPath(tree, main)}`))
      .sort(exactFirst((tree) => tree.branch === query));
    // A branch a worktree has is reached through that worktree.
    const free = repo.branches.filter((b) => !b.worktree && matches(b.name)).sort(exactFirst(isBranch));
    const exists = repo.branches.some(isBranch);
    const fresh = query && !repo.worktrees.some((tree) => tree.branch === query);
    const running = repo.worktrees.reduce((sum, tree) => sum + tree.running, 0);
    return [
      {
        heading: `Worktrees (${trees.length})`,
        running,
        // Open while typing, as a match there is what you're after.
        open: treesOpen || Boolean(query),
        items: trees.map((tree) => ({ tree, main })),
      },
      { heading: 'Check out here', open: true, items: free.slice(0, MAX_BRANCHES).map((branch) => ({ branch })) },
      {
        heading: 'New',
        open: true,
        items: [fresh && { worktree: query, exists }, fresh && !exists && { create: query }].filter(Boolean),
      },
    ].filter((section) => section.items.length);
  }

  function option(row, index) {
    const selected = index === cursor;
    const props = {
      class: `row option ${selected ? 'selected' : ''}`,
      role: 'option',
      'aria-selected': String(selected),
      onClick: () => pick(row),
    };
    // One line each: the name, then where, dimmed.
    const line = (text, where) => h('div', { class: 'meta line' },
      h('span', { class: 'title' }, text), where && h('span', { class: 'sub', title: where }, where));
    if (row.tree) {
      const { tree } = row;
      return h('div', props,
        icon('worktree'),
        line(tree.branch ?? 'detached HEAD', shortPath(tree, row.main)),
        tree.running > 0 && h('span', { class: 'tag on' }, `${tree.running} running`),
        tree.current && h('span', { class: 'tag' }, 'current'));
    }
    if (row.branch) return h('div', props, icon('branch'), line(row.branch.name));
    const text = row.create
      ? `New branch “${row.create}” here`
      : row.exists ? `Open “${row.worktree}” in a new worktree` : `New worktree with branch “${row.worktree}”`;
    return h('div', props, icon('plus'), line(text));
  }

  function heading(section) {
    if (!section.heading.startsWith('Worktrees')) return h('div', { class: 'picker-heading', role: 'presentation' }, section.heading);
    // A button, so the keyboard reaches it: Tab from the field, then Space.
    return h('button', {
      class: `picker-heading toggle ${section.open ? 'open' : ''}`,
      'aria-expanded': String(section.open),
      onClick: () => { treesOpen = !treesOpen; draw({ ease: true }); input.focus(); },
    },
    icon('chevron'), icon('worktree'), section.heading,
    section.running > 0 && h('span', { class: 'tag on' }, `${section.running} running`));
  }

  /** Rows the size of real ones, until the first read: the list fills in instead of growing from one line. */
  const placeholders = () => [52, 70, 44, 62].map((width) => h('div', { class: 'row skeleton', 'aria-hidden': 'true' },
    h('span', { class: 'bar', style: `width: ${width}%` })));

  function content() {
    if (error || repo === null) return [h('div', { class: 'hint warn' }, error ?? 'Not a git repository.')];
    if (repo === undefined) return placeholders();
    const found = sections();
    rows = found.filter((section) => section.open).flatMap((section) => section.items);
    cursor = Math.min(cursor, Math.max(rows.length - 1, 0));
    let index = 0;
    return found.length
      ? found.flatMap((section) => [heading(section), ...(section.open ? section.items.map((row) => option(row, index++)) : [])])
      : [h('div', { class: 'hint' }, 'Nothing matches.')];
  }

  /** `ease` slides the list to its new height: as a read lands, or the worktrees fold. Typing doesn't, as it's quicker. */
  function draw({ ease = false } = {}) {
    if (busy) return; // the list stays as it was, dimmed, while git works
    const from = list.offsetHeight;
    list.replaceChildren(...content());
    if (ease && list.isConnected) easeHeight(list, from);
  }

  /** Names what git is doing in the field, and dims the list, so nothing moves while it works. */
  function working(label) {
    const typed = input.value;
    busy = label;
    el.classList.add('busy');
    Object.assign(input, { value: '', placeholder: label, disabled: true });
    return () => {
      busy = null;
      el.classList.remove('busy');
      Object.assign(input, { value: typed, placeholder: 'Find or create a worktree or branch…', disabled: false });
      draw();
      input.focus();
    };
  }

  async function pick(row) {
    if (busy) return;
    if (row.tree?.current) return onClose();
    const projectId = project.id;
    let work;
    if (row.tree) {
      const name = row.tree.branch ?? 'detached';
      // The project's own folder clears the switch.
      project.worktree = row.tree.dir === project.path ? null : row.tree.dir;
      work = ['Switching…', () => saveProject(project), `Scripts now run in the ${name} worktree`];
    } else if (row.branch) {
      const branch = row.branch.name;
      work = [`Checking out ${branch}…`, () => call('git_switch', { projectId, branch }), `Checked out ${localName(branch)}`];
    } else if (row.worktree) {
      const branch = row.worktree;
      work = ['Making the worktree…', () => call('git_add_worktree', { projectId, branch, base: null }),
        `Made a worktree for ${branch}. Scripts now run there`];
    } else {
      const branch = row.create;
      work = [`Creating ${branch}…`, () => call('git_switch', { projectId, branch }), `Created ${branch} and checked it out`];
    }
    const [label, run, done] = work;
    const finished = working(label);
    try {
      await run();
      readRepo(projectId, { fresh: true }).catch(() => {}); // what changed, for the next time it opens
      toast(done, 'success');
      onChange();
    } catch { /* call() showed it */ } finally {
      finished();
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

  draw(); // what's known now, or placeholders; the read in flight eases it to what git says
  return { el, focus: () => input.focus() };
}
