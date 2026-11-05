import {
  call, dotClass, groupScripts, h, icon, iconButton, invoke, isLive, listen, missingLabel, nodeLabel, runScript,
  scriptLabel, scriptsOf, statusText, togglePin, urlLabel,
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
  return h('div', { class: `row ${selected ? 'selected' : ''}`, onClick: () => act(project, script) },
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
      : iconButton('play', 'Run', () => runScript(project, script), 'go'));
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
      ? results.map(([project, item], i) => (item.scripts ? groupRow : scriptRow)(
        project, item, projects.length > 1, i === cursor))
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
    const [project, item] = results[cursor];
    (item.scripts ? startGroup : act)(project, item);
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
refresh();
