import {
  call, Channel, dotClass, freePortButton, h, iconButton, invoke, isLive, listen, portOwners, portText, statusText,
  urlLabel,
} from './shared.js';

const tabs = document.getElementById('tabs');
const actions = document.getElementById('actions');
const screen = document.getElementById('screen');

const token = (name) => getComputedStyle(screen).getPropertyValue(name).trim();

const term = new window.Terminal({
  fontFamily: 'ui-monospace, Menlo, Consolas, monospace',
  fontSize: (await invoke('settings')).terminalFontSize,
  scrollback: 10000,
  convertEol: false,
  theme: {
    background: token('--bg'),
    foreground: token('--fg'),
    cursor: token('--accent'),
    selectionBackground: '#ff804759',
    // All 4.5:1 or better on --bg, except black and the deliberately dim bright black.
    black: '#2e2b28',
    red: '#f2697f',
    green: '#7ed492',
    yellow: '#f2c14e',
    blue: '#7fb0f5',
    magenta: '#d897e0',
    cyan: '#62cdc3',
    white: '#d9d4cb',
    brightBlack: '#7d776e',
    brightRed: '#ff8c9e',
    brightGreen: '#a0e7ae',
    brightYellow: '#ffd873',
    brightBlue: '#a3c7ff',
    brightMagenta: '#ecb4f2',
    brightCyan: '#8ce6dd',
    brightWhite: '#fbf8f2',
  },
});
const fit = new window.FitAddon.FitAddon();
term.loadAddon(fit);
term.open(screen);

let sessions = [];
let active = null;
let owners = new Map(); // session id → what holds its busy port
let confirming = null; // the Free port button waiting for its second click
const arm = (key) => { confirming = key; render(); };

term.onData((data) => {
  if (active != null) invoke('send_input', { id: active, data });
});
term.onResize(({ cols, rows }) => {
  if (active != null) invoke('resize', { id: active, cols, rows });
});
new ResizeObserver(() => fit.fit()).observe(screen);

async function select(id) {
  if (id === active) return;
  active = id;
  term.reset();
  const channel = new Channel();
  channel.onmessage = (bytes) => {
    if (active === id) term.write(new Uint8Array(bytes));
  };
  await invoke('attach', { id, onData: channel });
  fit.fit();
  invoke('resize', { id, cols: term.cols, rows: term.rows });
  render();
  term.focus();
}

function render() {
  tabs.replaceChildren(...sessions.map((session) => h('button', {
    class: `tab ${session.id === active ? 'active' : ''}`,
    role: 'tab',
    'aria-selected': String(session.id === active),
    title: `${session.project}: ${session.command} (${statusText(session)})`,
    onClick: () => select(session.id),
  }, h('span', { class: `dot ${dotClass(session)}` }), session.label)));

  const session = sessions.find((s) => s.id === active);
  actions.replaceChildren(...(session ? [
    session.busyPort && h('span', { class: 'warn' }, portText(session, owners.get(session.id))),
    freePortButton(session, owners.get(session.id), confirming, arm),
    isLive(session) && session.url && h('button', {
      class: 'chip', title: `Open ${session.url}`, onClick: () => call('open_url', { url: session.url }),
    }, urlLabel(session.url)),
    iconButton('restart', 'Restart', () => call('restart', { id: session.id })),
    isLive(session)
      ? iconButton('stop', 'Stop', () => call('stop', { id: session.id }), 'danger')
      : iconButton('close', 'Dismiss', () => call('clear_session', { id: session.id })),
  ].filter(Boolean) : []));
}

async function load() {
  sessions = await invoke('list_sessions');
  if (!sessions.some((s) => s.id === active)) {
    active = null;
    term.reset();
    if (sessions.length) await select(sessions[sessions.length - 1].id);
  }
  render();
  owners = await portOwners(sessions); // lsof can take a moment, so this fills in after
  if (owners.size) render();
}

listen('sessions-changed', load);
listen('select-session', ({ payload }) => select(payload));
listen('settings-changed', ({ payload }) => {
  term.options.fontSize = payload.terminalFontSize;
  fit.fit();
});

const wanted = Number(new URLSearchParams(location.search).get('session'));
if (wanted) await select(wanted);
load();
