import {
  call, dotClass, h, icon, iconButton, invoke, isLive, listen, nodeLabel, runScript, saveProject, scriptLabel,
  scriptsOf, statusText, togglePin, urlLabel, withArgs,
} from './shared.js';
import { updateBanner } from './update.js';

const MAX_RESULTS = 60;

let projects = [];
let sessions = [];
let query = '';
let cursor = 0;
let results = [];

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

const sessionFor = (project, script) =>
  sessions.find((s) => s.projectId === project.id && s.command === script.command);

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

function sessionRow(session) {
  const live = isLive(session);
  return h('div', { class: 'row', onClick: () => openTerminal(session.id) },
    h('span', { class: `dot ${dotClass(session)}`, title: statusText(session) }),
    h('div', { class: 'meta' },
      h('div', { class: 'title' }, session.label),
      h('div', { class: 'sub' }, `${session.project} · ${statusText(session)}`)),
    urlChip(session),
    iconButton('restart', 'Restart', () => call('restart', { id: session.id })),
    live
      ? iconButton('stop', 'Stop', () => call('stop', { id: session.id }), 'danger')
      : iconButton('close', 'Dismiss', () => call('clear_session', { id: session.id })));
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

function projectBlock(project) {
  const open = !collapsed.has(project.id);
  const all = scriptsOf(project);
  const pinned = all.filter((s) => s.custom || project.pinned.includes(s.key));
  const shown = pinned.length ? pinned : all.filter((s) => !s.package);
  const hidden = all.length - shown.length;
  const { error, pm, node } = project.scan;
  return [
    h('button', { class: `project-head ${open ? 'open' : ''}`, onClick: () => toggleCollapsed(project.id) },
      icon('chevron'),
      h('span', { class: 'name' }, project.name),
      h('span', { class: `info ${node.wanted && !node.resolved ? 'warn' : ''}` }, error ? '' : `${pm} · ${nodeLabel(node)}`)),
    open && error && h('div', { class: 'hint warn' }, error),
    open && shown.map((script) => scriptRow(project, script, false, false)),
    open && hidden > 0 && h('div', { class: 'hint' }, `${hidden} more scripts, search to find them`),
  ];
}

function matches(project, script, terms) {
  const text = `${project.name} ${script.package ?? ''} ${script.name} ${script.command}`.toLowerCase();
  return terms.every((term) => text.includes(term));
}

function searchResults() {
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  const pinnedFirst = ([p, s]) => (s.custom || p.pinned.includes(s.key) ? 0 : 1);
  return projects
    .flatMap((project) => scriptsOf(project).map((script) => [project, script]))
    .filter(([project, script]) => matches(project, script, terms))
    .sort((a, b) => pinnedFirst(a) - pinnedFirst(b))
    .slice(0, MAX_RESULTS);
}

function render() {
  const scroll = list.scrollTop;
  // Rebuilding the list detaches the arguments field, which drops its focus.
  if (document.activeElement === argsInput) queueMicrotask(() => argsInput.focus());
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
      ? results.flatMap(([project, script], i) => scriptRow(project, script, projects.length > 1, i === cursor))
      : [h('div', { class: 'empty' }, 'No matching scripts.')]));
    return;
  }
  const live = sessions.some(isLive);
  list.replaceChildren(...[
    sessions.length > 0 && h('div', { class: 'group-title' },
      h('span', { class: 'grow' }, `Scripts (${sessions.filter(isLive).length} running)`),
      live && h('button', { class: 'link', onClick: () => call('stop_all') }, 'Stop all')),
    sessions.map(sessionRow),
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
    if (event.shiftKey) openArgs(...results[cursor]); else act(...results[cursor]);
  }
}

async function loadSessions() {
  sessions = await invoke('list_sessions');
  render();
}

async function refresh() {
  projects = await invoke('list_projects');
  await loadSessions();
}

listen('sessions-changed', loadSessions);
window.addEventListener('focus', () => { search.focus(); search.select(); refresh(); });
// Hiding cancels the field: the next showing's refresh() replaces the projects argsFor holds.
window.addEventListener('blur', () => argsFor && closeArgs());
refresh();
