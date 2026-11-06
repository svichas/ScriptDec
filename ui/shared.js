export const { invoke, Channel } = window.__TAURI__.core;
export const { listen } = window.__TAURI__.event;

export const MAC = navigator.userAgent.includes('Mac');
document.documentElement.classList.toggle('mac', MAC); // windows there leave room for the traffic lights

/** Builds DOM nodes. Never use innerHTML with data: names come from package.json files. */
export function h(tag, props = {}, ...kids) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (key.startsWith('on')) el.addEventListener(key.slice(2).toLowerCase(), value);
    else if (key === 'value') el.value = value;
    else if (value !== false && value != null) el.setAttribute(key, value === true ? '' : value);
  }
  el.append(...kids.flat(Infinity).filter((kid) => kid != null && kid !== false));
  return el;
}

const STAR = 'm8 2.2 1.8 3.7 4 .6-2.9 2.8.7 4L8 11.5l-3.6 1.9.7-4L2.2 6.5l4-.6z';

const PATHS = {
  play: '<path d="M5 3.5v9l7.5-4.5z" fill="currentColor"/>',
  stop: '<rect x="4" y="4" width="8" height="8" rx="1.2" fill="currentColor"/>',
  restart: '<path d="M12.5 8a4.5 4.5 0 1 1-1.4-3.2M12.5 2v3h-3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>',
  terminal: '<path d="M3 4.5 7 8l-4 3.5M8.5 12H13" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>',
  close: '<path d="m4 4 8 8M12 4l-8 8" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>',
  // An eight-tooth cog: the old rays round a circle read as a sun.
  gear: '<g fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"><path d="M6.55 3.32L6.69 1.53L9.31 1.53L9.45 3.32L10.29 3.67L11.65 2.5L13.5 4.35L12.33 5.71L12.68 6.55L14.47 6.69L14.47 9.31L12.68 9.45L12.33 10.29L13.5 11.65L11.65 13.5L10.29 12.33L9.45 12.68L9.31 14.47L6.69 14.47L6.55 12.68L5.71 12.33L4.35 13.5L2.5 11.65L3.67 10.29L3.32 9.45L1.53 9.31L1.53 6.69L3.32 6.55L3.67 5.71L2.5 4.35L4.35 2.5L5.71 3.67Z"/><circle cx="8" cy="8" r="2.1"/></g>',
  star: `<path d="${STAR}" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/>`,
  starred: `<path d="${STAR}" fill="currentColor" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/>`,
  chevron: '<path d="m6 4 4 4-4 4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>',
  plus: '<path d="M8 3v10M3 8h10" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>',
  pencil: '<path d="M3 13v-2.4L10.6 3a1.1 1.1 0 0 1 1.6 0l.8.8a1.1 1.1 0 0 1 0 1.6L5.4 13z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>',
  search: '<circle cx="7" cy="7" r="4.3" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="m10.3 10.3 3.2 3.2" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>',
  folder: '<path d="M2.5 4.5a1 1 0 0 1 1-1h2.8l1.5 1.5h4.7a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/>',
  branch: '<g fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><circle cx="5" cy="3.6" r="1.6"/><circle cx="5" cy="12.4" r="1.6"/><circle cx="11" cy="4.6" r="1.6"/><path d="M5 5.2v5.6M11 6.2c0 3-6 2.2-6 4.6"/></g>',
  download: '<path d="M8 2.5v7.5M4.8 7 8 10.2 11.2 7M3 11.5v1a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1v-1" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>',
  check: '<path d="m3.5 8.4 3 3 6-6.8" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>',
  alert: '<circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M8 4.9v3.6M8 11.1v.1" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>',
  info: '<circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M8 7.4v3.7M8 4.9v.1" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>',
  // Two folders, one behind the other: more than one checkout of the same repository.
  worktree: '<g fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M4.5 6V3.5a1 1 0 0 1 1-1h2.4l1.3 1.3h4.3a1 1 0 0 1 1 1v3.7a1 1 0 0 1-1 1h-2"/><path d="M1.5 7a1 1 0 0 1 1-1h2.4l1.3 1.3h4.3a1 1 0 0 1 1 1v4.2a1 1 0 0 1-1 1h-8a1 1 0 0 1-1-1z"/></g>',
  // Down onto a line, where download's tray means a new copy.
  pull: '<path d="M8 2.5v7.6M4.7 6.9 8 10.2l3.3-3.3M3 13.2h10" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>',
  swap: '<path d="M2.5 5.5h10M10 3l2.5 2.5L10 8M13.5 10.5h-10M6 8l-2.5 2.5L6 13" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>',
  trash: '<path d="M2.5 4.5h11M6.3 4.5V3.1a.8.8 0 0 1 .8-.8h1.8a.8.8 0 0 1 .8.8v1.4M4 4.5l.7 8.4a1 1 0 0 0 1 .9h4.6a1 1 0 0 0 1-.9l.7-8.4M6.8 7v4.4M9.2 7v4.4" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>',
  // A heartbeat, for a script that's restarted when it crashes.
  pulse: '<path d="M1.5 8.5h2.8l1.6-4.5 3.2 8 1.6-3.5h3.8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>',
  // A box, for a package install.
  install: '<path d="M8 1.9 13.5 4.9v6.2L8 14.1l-5.5-3V4.9zM2.7 5 8 7.9 13.3 5M8 7.9v6" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/>',
};

export function icon(name) {
  const span = h('span', { class: 'glyph', 'aria-hidden': 'true' });
  span.innerHTML = `<svg viewBox="0 0 16 16" width="16" height="16">${PATHS[name]}</svg>`;
  return span;
}

export function iconButton(name, title, onClick, extra = '') {
  return h('button', {
    class: `icon ${extra}`,
    'data-tip': title,
    'aria-label': title,
    onClick: (event) => { event.stopPropagation(); onClick(event); },
  }, icon(name));
}

const calm = matchMedia('(prefers-reduced-motion: reduce)');

/** Runs `then` once, whether the transition ends or not: a rebuilt list can drop the element mid-way. */
function afterTransition(el, then) {
  let done = false;
  const finish = (event) => {
    // A row's own colour transition bubbles up here too.
    if (done || (event && (event.target !== el || event.propertyName !== 'height'))) return;
    done = true;
    el.removeEventListener('transitionend', finish);
    el.style.height = el.style.overflow = el.style.transition = '';
    then?.();
  };
  el.addEventListener('transitionend', finish);
  setTimeout(finish, 320);
}

/**
 * Eases `el` from the height it had, `from`, to the one its content gives it now, so what's below slides instead
 * of jumping. Call it right after changing the content.
 */
export function easeHeight(el, from) {
  const to = el.offsetHeight;
  if (calm.matches || Math.abs(to - from) < 2) return;
  el.style.overflow = 'hidden';
  el.style.height = `${from}px`;
  void el.offsetHeight; // lays the start out, or the browser would skip straight to the end
  el.style.transition = 'height 0.18s ease';
  el.style.height = `${to}px`;
  afterTransition(el);
}

/** Eases `el` shut, then runs `then`, which hides it for good. */
export function easeShut(el, then) {
  if (calm.matches) return then();
  el.style.overflow = 'hidden';
  el.style.height = `${el.offsetHeight}px`;
  void el.offsetHeight;
  el.style.transition = 'height 0.16s ease';
  el.style.height = '0px';
  afterTransition(el, then);
}

// Notes stack at the bottom of the window and go by themselves; hovering one holds it.
const notes = h('div', { class: 'pills' });
document.body.append(notes);

/** A note on what just happened: 'success', 'info', or an 'error', which stays longer. A click puts it away. */
export function toast(message, kind = 'error') {
  const text = String(message);
  const glyph = { success: 'check', info: 'info' }[kind] ?? 'alert';
  const pill = h('div', { class: `pill ${kind}`, role: kind === 'error' ? 'alert' : 'status' },
    icon(glyph), h('span', { class: 'text' }, text));
  let timer;
  const close = () => {
    pill.classList.add('leaving');
    setTimeout(() => pill.remove(), 180);
  };
  const wait = (ms) => { clearTimeout(timer); timer = setTimeout(close, ms); };
  pill.addEventListener('click', close);
  pill.addEventListener('mouseenter', () => clearTimeout(timer));
  pill.addEventListener('mouseleave', () => wait(2500));
  notes.append(pill);
  // Errors stay long enough to read, longer for git's longer ones.
  wait(kind === 'error' ? Math.min(14000, 5000 + text.length * 35) : 3200);
}

// Tooltips, sooner than the system's, which a menu bar window may not show at all. Any `title` becomes one.
const tip = h('div', { class: 'tip', role: 'tooltip', hidden: true });
document.body.append(tip);
let tipFor = null;
let tipTimer;

function hideTip() {
  clearTimeout(tipTimer);
  tipFor = null;
  tip.hidden = true;
}

/** Moves a title into data-tip, so the system's tooltip doesn't show as well. Some are set again on hover. */
function adopt(el) {
  if (!el.title) return;
  el.dataset.tip = el.title;
  el.removeAttribute('title');
}

function showTip(el) {
  adopt(el);
  if (!el.isConnected || !el.dataset.tip) return;
  tip.textContent = el.dataset.tip;
  tip.hidden = false;
  const box = el.getBoundingClientRect();
  const left = Math.min(Math.max(6, box.left + box.width / 2 - tip.offsetWidth / 2), innerWidth - tip.offsetWidth - 6);
  const below = box.bottom + 6 + tip.offsetHeight <= innerHeight;
  tip.style.left = `${left}px`;
  tip.style.top = `${below ? box.bottom + 6 : box.top - tip.offsetHeight - 6}px`;
}

document.addEventListener('mouseover', (event) => {
  const el = event.target.closest?.('[data-tip], [title]');
  if (el === tipFor) return;
  hideTip();
  if (!el) return;
  adopt(el);
  tipFor = el;
  tipTimer = setTimeout(() => showTip(el), 450);
});
document.documentElement.addEventListener('mouseleave', hideTip);
for (const type of ['mousedown', 'keydown', 'blur']) window.addEventListener(type, hideTip, true);
window.addEventListener('scroll', hideTip, true);

/** invoke, with failures shown to the user. */
export const call = (command, args) => invoke(command, args).catch((error) => { toast(error); throw error; });

export const isLive = (session) => session.status === 'running' || session.status === 'stopping';

/** 48s, 2m 3s, 1h 5m: the same as the long-task notification. */
function duration(ms) {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`;
  return `${Math.floor(s / 3600)}h ${Math.floor(s / 60) % 60}m`;
}

export function statusText(session) {
  if (session.restartIn) return `crashed, restarting in ${session.restartIn}s`;
  const took = duration((session.endedAt ?? Date.now()) - session.startedAt);
  if (session.status === 'running') return `running · ${took}`;
  if (session.status !== 'exited') return session.status;
  return session.code === 0 ? `finished in ${took}` : `failed after ${took} (exit ${session.code})`;
}

export function dotClass(session) {
  if (session.status === 'running') return 'on';
  if (session.status === 'stopping') return 'busy';
  return session.status === 'exited' && session.code !== 0 ? 'bad' : '';
}

/** '1.2 GB · 42%', from `session_usage`. */
export function usageLabel({ memory, cpu }) {
  const mb = memory / 2 ** 20;
  const size = mb < 1024 ? `${Math.round(mb)} MB` : `${(mb / 1024).toFixed(1)} GB`;
  return `${size} · ${Math.round(cpu)}%`;
}

export function scriptsOf(project) {
  const custom = project.custom.map((c) => ({
    key: `custom:${c.id}`, name: c.name, command: c.command, body: c.command, custom: true,
  }));
  return [...custom, ...project.scan.scripts];
}

export const shortPackage = (name) => name.split('/').pop();
export const scriptLabel = (s) => (s.package ? `${shortPackage(s.package)} ${s.name}` : s.name);
export const missingLabel = (key) => `${key.replace(/^:/, '')} (missing)`;

/** A group's scripts in menu order, and its keys that match none: deleted or renamed outside ScriptDeck. */
export function groupScripts(project, group) {
  const all = scriptsOf(project);
  return {
    scripts: all.filter((s) => group.scripts.includes(s.key)),
    missing: group.scripts.filter((key) => !all.some((s) => s.key === key)),
  };
}

/** In the worktree the project is switched to, as this window shows it. */
export function runScript(project, script) {
  const run = { projectId: project.id, label: scriptLabel(script), command: script.command, key: script.key };
  return call('run', { ...run, cwd: project.dir });
}

/** A script's session in the folder the project is switched to: one runs per worktree. */
export const sessionFor = (sessions, project, script) =>
  sessions.find((s) => s.projectId === project.id && s.command === script.command && s.cwd === project.dir);

/** The script with arguments for one run. Not quoted: the user typed them, as in a terminal. */
export function withArgs(project, script, args) {
  const typed = args.trim();
  if (!typed) return script;
  // npm would take --watch as its own flag; pnpm, yarn and bun pass it on, and pnpm 7+ would pass a `--` on too.
  const dashes = !script.custom && project.scan.pm === 'npm' ? ' --' : '';
  return { ...script, name: `${script.name} ${typed}`, command: `${script.command}${dashes} ${typed}` };
}

// This window's copy of each project as last loaded or saved, by id: a save sends its changes since then.
// `scan`, `dir` and `isRepo` come from list_projects and are never saved.
const loaded = new Map();
const remember = ({ scan, dir, isRepo, ...project }) => loaded.set(project.id, structuredClone(project));

export async function listProjects() {
  const projects = await invoke('list_projects');
  projects.forEach(remember);
  return projects;
}

/** Another window may have changed the project since this one loaded it, so Rust applies only this window's changes. */
export function saveProject({ scan, dir, isRepo, ...project }) {
  const base = loaded.get(project.id);
  remember(project); // now, so a second save before this one returns sends only its own change
  return call('update_project', { base, project });
}

export function togglePin(project, script) {
  const pinned = project.pinned.includes(script.key);
  project.pinned = pinned ? project.pinned.filter((k) => k !== script.key) : [...project.pinned, script.key];
  return saveProject(project);
}

export function nodeLabel(node) {
  if (node.resolved) return `node ${node.resolved}`;
  return node.wanted ? `node ${node.wanted} missing` : 'shell node';
}

export const urlLabel = (url) => `:${new URL(url).port}`;

/** What holds each busy port, by session id: null when lsof can't see it. */
export async function portOwners(sessions) {
  const busy = sessions.filter((s) => s.busyPort);
  const owners = await Promise.all(busy.map((s) => invoke('port_owner', { port: s.busyPort })));
  return new Map(busy.map((s, i) => [s.id, owners[i]]));
}

/** "Port 3000 is in use by node (pid 4120)", or by one of our scripts. */
export function portText(session, owner) {
  const script = owner?.session;
  const by = script ? `${script.label} in ${script.project}` : owner && `${owner.command} (pid ${owner.pid})`;
  return `Port ${session.busyPort} is in use${by ? ` by ${by}` : ''}`;
}

/**
 * Stops what holds the port, then restarts the script. It asks first, like Remove:
 * `arm` gets the key of the button waiting for its second click, or null.
 */
export function freePortButton(session, owner, confirming, arm) {
  if (!session.busyPort || !owner) return null; // owners can still hold a restarted script's old entry
  const key = `${session.id}:${owner.pid}`; // a new owner needs a new confirmation
  const armed = confirming === key;
  return h('button', {
    class: `btn ${armed ? 'danger' : ''}`,
    onClick: (event) => {
      event.stopPropagation();
      arm(armed ? null : key);
      if (!armed) return;
      // A refusal names the new owner in a toast, and the windows reload.
      call('free_port', { port: session.busyPort, pid: owner.pid })
        .then(() => call('restart', { id: session.id }), () => {});
    },
  }, armed ? 'Free port and restart?' : 'Free port');
}
