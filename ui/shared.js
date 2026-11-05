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
  gear: '<path d="M8 10.2a2.2 2.2 0 1 0 0-4.4 2.2 2.2 0 0 0 0 4.4Z" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M8 1.8v1.7M8 12.5v1.7M1.8 8h1.7M12.5 8h1.7M3.6 3.6l1.2 1.2M11.2 11.2l1.2 1.2M12.4 3.6l-1.2 1.2M4.8 11.2l-1.2 1.2" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>',
  star: `<path d="${STAR}" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/>`,
  starred: `<path d="${STAR}" fill="currentColor" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/>`,
  chevron: '<path d="m6 4 4 4-4 4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>',
  plus: '<path d="M8 3v10M3 8h10" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>',
  pencil: '<path d="M3 13v-2.4L10.6 3a1.1 1.1 0 0 1 1.6 0l.8.8a1.1 1.1 0 0 1 0 1.6L5.4 13z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>',
  search: '<circle cx="7" cy="7" r="4.3" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="m10.3 10.3 3.2 3.2" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>',
  folder: '<path d="M2.5 4.5a1 1 0 0 1 1-1h2.8l1.5 1.5h4.7a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/>',
};

export function icon(name) {
  const span = h('span', { class: 'glyph', 'aria-hidden': 'true' });
  span.innerHTML = `<svg viewBox="0 0 16 16" width="16" height="16">${PATHS[name]}</svg>`;
  return span;
}

export function iconButton(name, title, onClick, extra = '') {
  return h('button', {
    class: `icon ${extra}`,
    title,
    'aria-label': title,
    onClick: (event) => { event.stopPropagation(); onClick(event); },
  }, icon(name));
}

export function toast(message) {
  const el = h('div', { class: 'toast', role: 'alert' }, String(message));
  document.body.append(el);
  setTimeout(() => el.remove(), 5000);
}

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

/** A group's scripts in menu order, and its keys that match none: deleted or renamed outside ScriptDec. */
export function groupScripts(project, group) {
  const all = scriptsOf(project);
  return {
    scripts: all.filter((s) => group.scripts.includes(s.key)),
    missing: group.scripts.filter((key) => !all.some((s) => s.key === key)),
  };
}

export function runScript(project, script) {
  return call('run', { projectId: project.id, label: scriptLabel(script), command: script.command, key: script.key });
}

/** The script with arguments for one run. Not quoted: the user typed them, as in a terminal. */
export function withArgs(project, script, args) {
  const typed = args.trim();
  if (!typed) return script;
  // npm would take --watch as its own flag; pnpm, yarn and bun pass it on, and pnpm 7+ would pass a `--` on too.
  const dashes = !script.custom && project.scan.pm === 'npm' ? ' --' : '';
  return { ...script, name: `${script.name} ${typed}`, command: `${script.command}${dashes} ${typed}` };
}

// This window's copy of each project as last loaded or saved, by id: a save sends its changes since then.
const loaded = new Map();
const remember = ({ scan, ...project }) => loaded.set(project.id, structuredClone(project));

export async function listProjects() {
  const projects = await invoke('list_projects');
  projects.forEach(remember);
  return projects;
}

/** Another window may have changed the project since this one loaded it, so Rust applies only this window's changes. */
export function saveProject({ scan, ...project }) {
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
