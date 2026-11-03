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

export function statusText(session) {
  if (session.status === 'exited') return session.code === 0 ? 'finished' : `failed (exit ${session.code})`;
  return session.status;
}

export function dotClass(session) {
  if (session.status === 'running') return 'on';
  if (session.status === 'stopping') return 'busy';
  return session.status === 'exited' && session.code !== 0 ? 'bad' : '';
}

export function scriptsOf(project) {
  const custom = project.custom.map((c) => ({
    key: `custom:${c.id}`, name: c.name, command: c.command, body: c.command, custom: true,
  }));
  return [...custom, ...project.scan.scripts];
}

export const shortPackage = (name) => name.split('/').pop();
export const scriptLabel = (s) => (s.package ? `${shortPackage(s.package)} ${s.name}` : s.name);

export function runScript(project, script) {
  return call('run', { projectId: project.id, label: scriptLabel(script), command: script.command });
}

export function saveProject({ scan, ...project }) {
  return call('update_project', { project });
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

const LATEST_RELEASE = 'https://api.github.com/repos/svichas/ScriptDec/releases/latest';
const RECHECK_MS = 6 * 60 * 60 * 1000; // GitHub allows 60 unauthenticated requests an hour

const stored = (key) => { try { return JSON.parse(localStorage.getItem(key)); } catch { return null; } };
const store = (key, value) => { try { localStorage.setItem(key, JSON.stringify(value)); } catch { /* only a convenience */ } };

/** The newest release tag, asked of GitHub at most every few hours across all windows. */
async function latestRelease() {
  const cached = stored('latestRelease');
  if (cached && Date.now() - cached.checkedAt < RECHECK_MS) return cached.tag;
  const response = await fetch(LATEST_RELEASE, { headers: { Accept: 'application/vnd.github+json' } });
  if (!response.ok) return cached?.tag;
  const { tag_name: tag } = await response.json();
  store('latestRelease', { tag, checkedAt: Date.now() });
  return tag;
}

const isNewer = (tag, version) => tag.replace(/^v/, '').localeCompare(version, 'en', { numeric: true }) > 0;

async function updateBanner() {
  const [tag, version] = await Promise.all([latestRelease(), window.__TAURI__.app.getVersion()]);
  document.querySelector('.update')?.remove();
  if (!tag || !isNewer(tag, version) || stored('dismissedUpdate') === tag) return;
  const banner = h('div', { class: 'update', role: 'status' },
    h('span', {}, `ScriptDec ${tag} is available. You have ${version}.`),
    h('button', { class: 'btn primary', onClick: () => call('open_latest_release') }, 'Download'),
    iconButton('close', 'Dismiss', () => { store('dismissedUpdate', tag); banner.remove(); }));
  document.body.append(banner);
}

// Focus re-checks too: the popover lives for days, and a dismiss in another window should stick here.
const checkForUpdate = () => updateBanner().catch(() => {}); // offline is fine, try again later
window.addEventListener('focus', checkForUpdate);
checkForUpdate();
