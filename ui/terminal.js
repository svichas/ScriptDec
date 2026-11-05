import { call, Channel, dotClass, h, iconButton, invoke, isLive, listen, MAC, statusText, urlLabel } from './shared.js';

const tabs = document.getElementById('tabs');
const actions = document.getElementById('actions');
const screen = document.getElementById('screen');

const token = (name) => getComputedStyle(screen).getPropertyValue(name).trim();

// Called on every click on a link, so a plain click is left to select text.
const openLink = (event, url) => { if (MAC ? event.metaKey : event.ctrlKey) call('open_link', { url }); };

const term = new window.Terminal({
  fontFamily: 'ui-monospace, Menlo, Consolas, monospace',
  fontSize: (await invoke('settings')).terminalFontSize,
  scrollback: 10000,
  convertEol: false,
  allowProposedApi: true, // the search addon highlights matches with decorations
  linkHandler: { activate: openLink }, // OSC 8 links, which xterm finds itself
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
const search = new window.SearchAddon.SearchAddon();
term.loadAddon(search);
term.loadAddon(new window.WebLinksAddon.WebLinksAddon(openLink));
term.open(screen);

let sessions = [];
let active = null;

term.onData((data) => {
  if (active != null) invoke('send_input', { id: active, data });
});
term.onResize(({ cols, rows }) => {
  if (active != null) invoke('resize', { id: active, cols, rows });
});
new ResizeObserver(() => fit.fit()).observe(screen);

// Highlighting is also what makes the addon count. The newest output is at the bottom, so Enter searches up.
const FIND = { decorations: { matchBackground: token('--tint'), activeMatchBorder: token('--accent') } };
const findInput = h('input', {
  placeholder: 'Find',
  'aria-label': 'Find in output',
  spellcheck: 'false',
  onInput: () => findMatch(true),
  onKeydown: (event) => {
    if (event.key === 'Enter') findMatch(!event.shiftKey);
    else if (event.key === 'Escape') closeFind();
  },
});
const findCount = h('span', { class: 'count', 'aria-live': 'polite' });
const findBar = h('div', { class: 'find', role: 'search', hidden: true },
  findInput,
  findCount,
  iconButton('chevron', 'Previous match (Enter)', () => findMatch(true), 'up'),
  iconButton('chevron', 'Next match (Shift+Enter)', () => findMatch(false), 'down'));
screen.append(findBar);

function findMatch(previous) {
  if (previous) search.findPrevious(findInput.value, FIND);
  else search.findNext(findInput.value, FIND);
  findInput.focus();
}

function closeFind() {
  findBar.hidden = true;
  findInput.value = '';
  findCount.textContent = '';
  search.clearDecorations();
  term.focus();
}

search.onDidChangeResults(({ resultIndex, resultCount }) => {
  // shortcut: the addon counts and highlights the first 1000 matches only; pass it a highlightLimit if that's too few.
  const total = resultCount < 1000 ? resultCount : '1000+';
  if (!findInput.value) findCount.textContent = '';
  else if (!resultCount) findCount.textContent = 'No results';
  else findCount.textContent = resultIndex < 0 ? `${total} matches` : `${resultIndex + 1} of ${total}`;
});

// Caught before xterm sees it, which on Windows would send Ctrl+F to the script.
// Not with Alt: Windows reports AltGr as Ctrl+Alt, and AltGr+F types [ on Hungarian and Czech layouts.
window.addEventListener('keydown', (event) => {
  if (event.code !== 'KeyF' || event.altKey || !(MAC ? event.metaKey : event.ctrlKey)) return;
  event.preventDefault();
  event.stopPropagation();
  findBar.hidden = false;
  findInput.focus();
  findInput.select();
}, true);

async function select(id) {
  if (id === active) return;
  active = id;
  term.reset();
  closeFind();
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
    closeFind();
    if (sessions.length) await select(sessions[sessions.length - 1].id);
  }
  render();
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
