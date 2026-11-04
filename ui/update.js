import { call, h, iconButton, invoke, isLive, listen } from './shared.js';

/** The update banner. The popover and Settings put it at the bottom of their window. */
export const updateBanner = h('div', { class: 'update', hidden: true });
// The banner comes and goes, so screen readers hear each step from here instead.
const live = h('div', { class: 'sr-only', role: 'status' });
document.body.append(live);

let status = { state: 'idle' };
let current = '';
let running = 0; // scripts that restarting would stop
let drawn = ''; // what the banner shows, so progress ticks only move the bar

const stored = (key) => { try { return JSON.parse(localStorage.getItem(key)); } catch { return null; } };
const store = (key, value) => { try { localStorage.setItem(key, JSON.stringify(value)); } catch { /* only a convenience */ } };

const action = (label, command) => h('button', { class: 'btn primary', onClick: () => call(command) }, label);

function render() {
  const { state, version, percent } = status;
  const dismissed = stored('dismissedUpdate') === version && !['downloading', 'installing'].includes(state);
  const shows = state === 'idle' || dismissed ? '' : `${state} ${version} ${running}`;
  const bar = updateBanner.querySelector('progress');
  if (shows === drawn) {
    if (bar && percent != null) bar.value = percent;
    return;
  }
  drawn = shows;
  updateBanner.hidden = !shows;
  if (!shows) return updateBanner.replaceChildren();
  const progress = h('progress', { max: 100, 'aria-label': 'Download progress' });
  if (percent != null) progress.value = percent; // no value until the size is known: an empty track
  const scripts = `${running} running script${running === 1 ? '' : 's'}`;
  const [title, sub, button] = {
    available: [`ScriptDec ${version} is available`, `You have ${current}.`, action('Update', 'download_update')],
    downloading: [`Downloading ScriptDec ${version}…`, progress],
    ready: [`ScriptDec ${version} is ready`, running ? `Restarting stops ${scripts}.` : 'Restart to finish updating.',
      action('Restart Now', 'install_update')],
    installing: [`Installing ScriptDec ${version}…`, 'ScriptDec reopens in a moment.'],
    failed: [`Couldn't update to ${version}`,
      h('button', { class: 'link', onClick: () => call('open_latest_release') }, 'Download it from GitHub'),
      action('Try Again', 'download_update')],
  }[state];
  updateBanner.replaceChildren(...[
    h('div', { class: 'meta' }, h('div', { class: 'title' }, title), h('div', { class: 'sub' }, sub)),
    button,
    button && iconButton('close', 'Dismiss', () => { store('dismissedUpdate', version); render(); }),
  ].filter(Boolean));
  if (live.textContent !== title) live.textContent = title;
}

async function show(next) {
  status = next;
  if (status.state === 'ready') running = (await invoke('list_sessions')).filter(isLive).length;
  render();
}

/** Checks now, even if a check ran lately, and brings back a dismissed banner. Resolves to the status. */
export async function checkForUpdates() {
  store('dismissedUpdate', null);
  const next = await call('check_for_update', { force: true });
  await show(next);
  return next;
}

listen('update-changed', ({ payload }) => show(payload));
listen('sessions-changed', () => status.state === 'ready' && show(status));

// Rust skips checks that aren't due yet, so every focus may ask.
const autoCheck = () => invoke('check_for_update', { force: false }).then(show, () => {});
window.__TAURI__.app.getVersion().then((version) => {
  current = version;
  window.addEventListener('focus', autoCheck);
  autoCheck();
});
