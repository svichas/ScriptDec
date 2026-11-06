import {
  call, dotClass, freePortButton, groupScripts, h, icon, iconButton, invoke, isLive, listen, listProjects, missingLabel,
  nodeLabel, portOwners, portText, runScript, saveProject, scriptLabel, scriptsOf, sessionFor as findSession, statusText,
  toast, togglePin, urlLabel, usageLabel, withArgs,
} from './shared.js';
import { branchChip, pull, pullLabel, switcher } from './git.js';
import { updateBanner } from './update.js';

const MAX_RESULTS = 60;

let projects = [];
let sessions = [];
let usage = {}; // session id → { cpu, memory }, polled while the popover is shown
let poll;
let query = '';
let cursor = 0;
let results = [];
let owners = new Map(); // session id → what holds its busy port
let confirming = null; // the Free port button waiting for its second click
const arm = (key) => { confirming = key; render(); };
let git = {}; // project id → the branch it's on and its changes, for projects in a git repository
let picker = null; // the open switcher, under its project: { id, el, focus }
let pulling = null; // the project whose Pull is running

function loadCollapsed() {
  try { return new Set(JSON.parse(localStorage.getItem('collapsed')) ?? []); } catch { return new Set(); }
}
const collapsed = loadCollapsed();
function toggleCollapsed(id) {
  collapsed[collapsed.has(id) ? 'delete' : 'add'](id);
  try { localStorage.setItem('collapsed', JSON.stringify([...collapsed])); } catch { /* only a convenience */ }
  render();
}

// The arguments field under a script's row: Shift+Enter on a search result, or Option-click on Run.
let argsFor = null; // [project, script]
const argsKey = (project, script) => `${project.id}:${script.key}`; // script keys repeat across projects
function loadLastArgs() {
  try { return JSON.parse(localStorage.getItem('lastArgs')) ?? {}; } catch { return {}; }
}
const lastArgs = loadLastArgs();
const argsInput = h('input', {
  class: 'mono', placeholder: '--watch', 'aria-label': 'Arguments', spellcheck: 'false', onKeydown: onArgsKey,
});
const argsField = h('div', { class: 'args' }, argsInput,
  h('button', { class: 'link', onClick: saveArgs }, 'Save as custom script'));

const search = h('input', {
  type: 'search',
  placeholder: 'Search scripts…',
  'aria-label': 'Search scripts',
  spellcheck: 'false',
  onInput: () => { query = search.value; cursor = 0; render(); },
  onKeydown: onSearchKey,
});
const list = h('div', { class: 'list' });
const header = h('header', {},
  h('label', { class: 'search' }, icon('search'), search),
  iconButton('gear', 'Settings', () => call('open_settings')));
document.body.append(h('div', { class: 'panel' }, header, list, updateBanner));

const sessionFor = (project, script) => findSession(sessions, project, script);

const openTerminal = (id) => call('open_terminal', { id });

function act(project, script) {
  const session = sessionFor(project, script);
  if (session && isLive(session)) return openTerminal(session.id);
  return runScript(project, script);
}

function openArgs(project, script) {
  argsFor = [project, script];
  argsInput.value = lastArgs[argsKey(project, script)] ?? '';
  render();
  argsInput.focus();
  argsInput.select();
}

function closeArgs() {
  argsFor = null;
  render();
  search.focus();
}

function onArgsKey(event) {
  if (event.key === 'Escape') return closeArgs();
  if (event.key !== 'Enter') return;
  const [project, script] = argsFor;
  const args = argsInput.value.trim();
  lastArgs[argsKey(project, script)] = args;
  try { localStorage.setItem('lastArgs', JSON.stringify(lastArgs)); } catch { /* only a convenience */ }
  closeArgs();
  act(project, withArgs(project, script, args));
}

async function saveArgs() {
  if (!argsInput.value.trim()) return argsInput.focus();
  const [project, script] = argsFor;
  const run = withArgs(project, script, argsInput.value);
  project.custom.push({ id: crypto.randomUUID(), name: scriptLabel(run), command: run.command });
  await saveProject(project);
  closeArgs();
}

function urlChip(session) {
  return isLive(session) && session.url && h('button', {
    class: 'chip',
    title: `Open ${session.url}`,
    onClick: (event) => { event.stopPropagation(); call('open_url', { url: session.url }); },
  }, urlLabel(session.url));
}

// Only past 500 MB or 10% CPU (about one busy core), so quiet scripts stay uncluttered.
function rowUsage(id) {
  const used = usage[id];
  return used && (used.memory > 500 * 2 ** 20 || used.cpu > 10) ? usageLabel(used) : '';
}

let tickers = []; // one per live row on screen, redrawing its running time

function sessionRow(session) {
  const live = isLive(session);
  const dot = h('span', { class: `dot ${dotClass(session)}` });
  const sub = h('div', { class: 'sub' });
  // A tick only sets text: rebuilding the row every second would drop clicks and keyboard focus.
  const showStatus = () => {
    const status = statusText(session);
    dot.title = status;
    sub.textContent = `${session.project} · ${status}`;
  };
  showStatus();
  if (live) tickers.push(showStatus);
  return h('div', { class: 'row', onClick: () => openTerminal(session.id) },
    dot,
    h('div', { class: 'meta' },
      h('div', { class: 'title' }, session.label),
      sub),
    live && h('span', { class: 'usage', 'data-session': session.id }, rowUsage(session.id)),
    urlChip(session),
    iconButton('restart', 'Restart', () => call('restart', { id: session.id })),
    live || session.restartIn // stopping a pending restart cancels it
      ? iconButton('stop', 'Stop', () => call('stop', { id: session.id }), 'danger')
      : iconButton('close', 'Dismiss', () => call('clear_session', { id: session.id })));
}

function portNotice(session) {
  const owner = owners.get(session.id);
  return session.busyPort && h('div', { class: 'hint inline port' },
    h('span', { class: 'warn' }, portText(session, owner)), freePortButton(session, owner, confirming, arm));
}

function scriptRow(project, script, showProject, selected) {
  const session = sessionFor(project, script);
  const live = session && isLive(session);
  const pinned = project.pinned.includes(script.key);
  const sub = showProject ? `${project.name} · ${script.command}` : script.command;
  const row = h('div', { class: `row ${selected ? 'selected' : ''}`, onClick: () => act(project, script) },
    h('span', { class: `dot ${session ? dotClass(session) : ''}` }),
    h('div', { class: 'meta' },
      h('div', { class: 'title' }, scriptLabel(script)),
      h('div', { class: 'sub mono', title: script.body }, sub)),
    !script.custom && iconButton(pinned ? 'starred' : 'star', pinned ? 'Unpin' : 'Pin to menu', async () => {
      await togglePin(project, script);
      render();
    }, pinned ? 'pinned' : ''),
    live
      ? iconButton('stop', 'Stop', () => call('stop', { id: session.id }), 'danger')
      : iconButton('play', 'Run', (event) => (event.altKey ? openArgs(project, script) : runScript(project, script)), 'go'));
  return argsFor && argsKey(...argsFor) === argsKey(project, script) ? [row, argsField] : row;
}

/** The running sessions of a group's scripts, and the scripts that aren't running. */
function groupState(project, group) {
  const { scripts, missing } = groupScripts(project, group);
  const live = [];
  const idle = [];
  for (const script of scripts) {
    const session = sessionFor(project, script);
    if (session && isLive(session)) live.push(session);
    else idle.push(script);
  }
  return { scripts, missing, live, idle };
}

/** Starts what isn't running; when it all is, shows the first terminal, as act() does for one script. */
function startGroup(project, group) {
  const { live, idle } = groupState(project, group);
  if (!idle.length && live.length) return openTerminal(live[0].id);
  idle.forEach((script) => runScript(project, script));
}

function groupRow(project, group, showProject, selected) {
  const { scripts, missing, live, idle } = groupState(project, group);
  const names = [...scripts.map(scriptLabel), ...missing.map(missingLabel)].join(', ');
  const sub = [showProject && project.name, `${live.length} of ${scripts.length} running`, names];
  return h('div', { class: `row ${selected ? 'selected' : ''}`, onClick: () => startGroup(project, group) },
    h('span', { class: `dot ${!live.length ? '' : idle.length ? 'some' : 'on'}` }),
    h('div', { class: 'meta' },
      h('div', { class: 'title' }, group.name),
      h('div', { class: 'sub', title: names }, sub.filter(Boolean).join(' · '))),
    live.length > 0 && iconButton('stop', 'Stop group', () => live.forEach((s) => call('stop', { id: s.id })), 'danger'),
    idle.length > 0 && iconButton('play', 'Run group', () => startGroup(project, group), 'go'));
}

/** Switched to a worktree other than the project's own folder. */
const away = (project) => project.dir !== project.path;

function chip(project) {
  return git[project.id] && branchChip(project, git[project.id], {
    open: picker?.id === project.id,
    onClick: (event) => { event.stopPropagation(); togglePicker(project); },
  });
}

function togglePicker(project) {
  if (picker?.id === project.id) return closePicker();
  picker = {
    id: project.id,
    ...switcher(project, { onChange: () => { closePicker(); refresh(); }, onClose: closePicker }),
  };
  render();
  picker.focus();
}

function closePicker() {
  picker = null;
  render();
  search.focus();
}

async function runPull(project) {
  pulling = project.id;
  render();
  await pull(project.id).catch(() => {});
  pulling = null;
  await refresh();
}

/** Under the switcher: Pull, and Settings for the worktrees. */
function pickerFoot(project) {
  const label = pullLabel(git[project.id]);
  return h('div', { class: 'picker-foot' },
    label && h('button', { class: 'btn', disabled: pulling != null, onClick: () => runPull(project) },
      pulling === project.id ? 'Pulling…' : label),
    h('span', { class: 'grow' }),
    h('button', {
      class: 'link', onClick: () => call('open_settings', { project: project.id, section: 'worktrees' }),
    }, 'Manage worktrees…'));
}

/** A new worktree or a fresh clone has no node_modules yet, and its scripts would fail until it's installed. */
function installNotice(project) {
  const install = project.scan.tools.find((tool) => tool.label === 'Install');
  const start = () => runScript(project, { name: install.label, command: install.command })
    .then(() => toast(`Started ${install.command}. It's in Scripts above.`, 'info'), () => {});
  return install && h('div', { class: 'hint inline install' },
    h('span', { class: 'warn' }, away(project) ? 'Not installed in this worktree yet' : 'Not installed yet'),
    h('button', { class: 'btn', 'data-tip': 'Runs in its own terminal', onClick: start }, install.command));
}

function projectBlock(project) {
  const open = !collapsed.has(project.id);
  const all = scriptsOf(project);
  const pinned = all.filter((s) => s.custom || project.pinned.includes(s.key));
  const shown = pinned.length ? pinned : all.filter((s) => !s.package);
  const hidden = all.length - shown.length;
  const { error, pm, node } = project.scan;
  return [
    // Not a button, as it holds the branch button; the toggle inside is what the keyboard reaches.
    h('div', { class: `project-head ${open ? 'open' : ''}`, onClick: () => toggleCollapsed(project.id) },
      h('button', { class: 'toggle', 'aria-expanded': String(open) },
        icon('chevron'),
        h('span', { class: 'name' }, project.name)),
      chip(project),
      h('span', { class: `info ${node.wanted && !node.resolved ? 'warn' : ''}` }, error ? '' : `${pm} · ${nodeLabel(node)}`)),
    picker?.id === project.id && git[project.id] && h('div', { class: 'picker-box' }, picker.el, pickerFoot(project)),
    open && error && h('div', { class: 'hint warn' }, error),
    open && !error && !project.scan.installed && installNotice(project),
    open && project.groups.map((group) => groupRow(project, group, false, false)),
    open && shown.map((script) => scriptRow(project, script, false, false)),
    open && hidden > 0 && h('div', { class: 'hint' }, `${hidden} more scripts, search to find them`),
  ];
}

function matches(project, script, terms) {
  const text = `${project.name} ${script.package ?? ''} ${script.name} ${script.command ?? ''}`.toLowerCase();
  return terms.every((term) => text.includes(term));
}

function searchResults() {
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  // A group, told apart by its list of scripts, ranks with the pinned scripts and ahead of its project's.
  const pinnedFirst = ([p, s]) => (s.scripts || s.custom || p.pinned.includes(s.key) ? 0 : 1);
  return projects
    .flatMap((project) => [...project.groups, ...scriptsOf(project)].map((item) => [project, item]))
    .filter(([project, script]) => matches(project, script, terms))
    .sort((a, b) => pinnedFirst(a) - pinnedFirst(b))
    .slice(0, MAX_RESULTS);
}

function render() {
  const scroll = list.scrollTop;
  tickers = [];
  // Rebuilding the list detaches the arguments field and the branch menu, which drops their focus.
  if (document.activeElement === argsInput) queueMicrotask(() => argsInput.focus());
  if (picker?.el.contains(document.activeElement)) queueMicrotask(() => picker?.focus());
  if (!projects.length) {
    list.replaceChildren(h('div', { class: 'empty' },
      h('div', {}, 'No projects yet.'),
      h('button', { class: 'btn primary', onClick: () => call('open_settings') }, 'Add a project')));
    return;
  }
  if (query.trim()) {
    results = searchResults();
    cursor = Math.min(cursor, Math.max(results.length - 1, 0));
    list.replaceChildren(...(results.length
      ? results.flatMap(([project, item], i) => (item.scripts ? groupRow : scriptRow)(
        project, item, projects.length > 1, i === cursor))
      : [h('div', { class: 'empty' }, 'No matching scripts.')]));
    return;
  }
  const live = sessions.some(isLive);
  list.replaceChildren(...[
    sessions.length > 0 && h('div', { class: 'group-title' },
      h('span', { class: 'grow' }, `Scripts (${sessions.filter(isLive).length} running)`),
      live && h('button', { class: 'link', onClick: () => call('stop_all') }, 'Stop all')),
    sessions.map((session) => [sessionRow(session), portNotice(session)]),
    projects.map(projectBlock),
  ].flat(Infinity).filter(Boolean));
  list.scrollTop = scroll;
}

function onSearchKey(event) {
  if (event.key === 'Escape') {
    if (search.value) { search.value = ''; query = ''; render(); } else invoke('hide_popover');
  } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault();
    cursor = Math.max(0, Math.min(results.length - 1, cursor + (event.key === 'ArrowDown' ? 1 : -1)));
    render();
    list.querySelector('.selected')?.scrollIntoView({ block: 'nearest' });
  } else if (event.key === 'Enter' && query.trim() && results[cursor]) {
    const [project, item] = results[cursor];
    if (item.scripts) startGroup(project, item); // a group has no arguments field
    else if (event.shiftKey) openArgs(project, item);
    else act(project, item);
  }
}

async function loadSessions() {
  sessions = await invoke('list_sessions');
  render();
  owners = await portOwners(sessions); // lsof can take a moment, so this fills in after
  if (owners.size) render();
}

async function refresh() {
  projects = await listProjects();
  await loadSessions();
  git = await invoke('git_overview'); // a git status per project, so it fills in after
  render();
}

async function loadUsage() {
  usage = await invoke('session_usage');
  // In place: rebuilding the list every 2 s would drop a click that straddles it.
  for (const el of list.querySelectorAll('.usage')) el.textContent = rowUsage(el.dataset.session);
}

listen('sessions-changed', loadSessions);
window.addEventListener('focus', () => { search.focus(); search.select(); refresh(); });
// WebKit throttles timers in the hidden popover, so poll from focus to blur, which hides it.
window.addEventListener('focus', () => { clearInterval(poll); poll = setInterval(loadUsage, 2000); loadUsage(); });
window.addEventListener('blur', () => clearInterval(poll));
// Times tick only while the window is shown: it hides on blur, and WebKit throttles timers in hidden windows.
let ticker;
window.addEventListener('focus', () => {
  clearInterval(ticker);
  ticker = setInterval(() => tickers.forEach((tick) => tick()), 1000);
});
window.addEventListener('blur', () => clearInterval(ticker));
// Hiding cancels the field: the next showing's refresh() replaces the projects argsFor holds.
window.addEventListener('blur', () => argsFor && closeArgs());
window.addEventListener('blur', () => picker && closePicker());
refresh();
