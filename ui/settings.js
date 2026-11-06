import {
  call, groupScripts, h, icon, iconButton, invoke, listen, listProjects, MAC, missingLabel, runScript, saveProject,
  scriptLabel, scriptsOf, shortPackage, toast, togglePin,
} from './shared.js';
import { branchName, branchPicker, pullLabel, pushLabel, syncText } from './git.js';
import { checkForUpdates, updateBanner } from './update.js';

const MAX_ROWS = 80;
const GENERAL = 'general';
const CLONE = 'clone';
// Each change's letter, what it means and its colour.
const KINDS = {
  A: ['A', 'Added', 'ok'],
  '?': ['A', 'New, not tracked by git yet', 'ok'],
  M: ['M', 'Modified', 'warn'],
  D: ['D', 'Deleted', 'bad'],
  R: ['R', 'Renamed', 'accent'],
  U: ['!', 'Conflicted: resolve it in your editor, then commit', 'bad'],
};
const FONT_SIZES = [10, 11, 12, 13, 14, 15, 16, 18, 20];
const KEY_SYMBOLS = { Control: '⌃', Alt: '⌥', Shift: '⇧', Super: '⌘' };

let projects = [];
let versions = [];
let settings = {};
let version = '';
// A project's id, GENERAL or CLONE. The menu bar can open the window on a project.
let selectedId = new URLSearchParams(location.search).get('project');
let filter = '';
let confirming = null; // the destructive action waiting for its second click
let editing = null; // the package.json script being changed: { key, package, old, name, body }; old is null for a new one
let recording = false; // the shortcut button is waiting for keys
let launchAtLogin = false;
let checking = false; // Check for Updates is waiting on GitHub
let upToDate = false; // and found nothing newer
const mcpCommand = await invoke('mcp_command'); // null on Windows, where agents can't connect yet
let repo = null; // the selected project's git repository, null outside one
let repoFor = null; // the project `repo` is for
let gitBusy = null; // the git command running, as its button's label
let gitError = null; // what the last one said when it failed, and where: { text, at }
let picker = null; // the open branch menu
let unchecked = new Set(); // changed files left out of the next commit
let cloneParent = null; // the folder chosen to clone into
let worktreeBase = null; // where a new worktree's branch starts; null is the current branch
// Kept across renders, with their text and focus.
const commitInput = h('textarea', {
  placeholder: 'Commit message', 'aria-label': 'Commit message', rows: 3, spellcheck: 'true',
  onKeydown: (event) => {
    if (event.key === 'Enter' && (MAC ? event.metaKey : event.ctrlKey)) body.querySelector('.commit .primary')?.click();
  },
});
const cloneInput = h('input', {
  class: 'mono url', placeholder: 'https://github.com/owner/repo, or owner/repo', 'aria-label': 'Repository URL',
  spellcheck: 'false', autocomplete: 'off',
});
const worktreeInput = h('input', {
  placeholder: 'New or existing branch', 'aria-label': 'Branch for the new worktree', list: 'worktree-branches',
  spellcheck: 'false', autocomplete: 'off',
});

const sidebar = h('nav', { class: 'sidebar', 'aria-label': 'Sections', 'data-tauri-drag-region': 'deep' });
const head = h('header', { class: 'page-head', 'data-tauri-drag-region': 'deep' });
const body = h('div', { class: 'page-body' });
document.body.append(h('div', { class: 'settings' }, sidebar, h('main', { class: 'detail' }, head, body, updateBanner)));

const selected = () => projects.find((p) => p.id === selectedId);

async function load() {
  [projects, versions, launchAtLogin, settings, version] = await Promise.all([
    listProjects(), invoke('node_versions'), invoke('login_item'), invoke('settings'),
    window.__TAURI__.app.getVersion(),
  ]);
  if (selectedId !== GENERAL && selectedId !== CLONE && !selected()) selectedId = projects[0]?.id ?? null;
  render();
  await loadRepo();
}

/** The selected project's repository, for its Git sections. */
async function loadRepo() {
  const id = selected()?.id;
  const editor = editing;
  const found = id && await invoke('git_repo', { projectId: id }).catch((error) => {
    gitError = { text: String(error), at: 'branch' };
    return null;
  });
  if (id !== selected()?.id) return; // another project was picked meanwhile
  if (repoFor !== id) {
    // Another project's page: its own message, files and worktree branch.
    commitInput.value = '';
    worktreeInput.value = '';
    worktreeBase = null;
    unchecked = new Set();
    picker = null;
  }
  [repo, repoFor] = [found ?? null, id];
  // Not while a script editor opened meanwhile: rebuilding it would take the cursor out. The next render shows this.
  if (editing === editor) render();
}

function select(id) {
  selectedId = id;
  filter = '';
  confirming = null;
  editing = null;
  recording = false;
  picker = null;
  gitError = null;
  body.scrollTop = 0;
  render();
  loadRepo();
}

/**
 * A git command: one at a time, its failure shown in the section it came from, `at`. Resolves to whether it worked.
 */
async function gitAction(at, label, run) {
  gitBusy = label;
  gitError = null;
  render();
  try {
    await run();
    return true;
  } catch (error) {
    gitError = { text: String(error), at };
    return false;
  } finally {
    gitBusy = null;
    await load();
  }
}

/** Folders from the picker, or dropped on the window. */
async function addFolders(paths) {
  for (const path of paths) {
    const project = await call('add_project', { path }).catch(() => null);
    if (project) selectedId = project.id;
  }
  await load();
}

async function addProject() {
  const path = await call('pick_folder');
  if (path) await addFolders([path]);
}

window.__TAURI__.webview.getCurrentWebview().onDragDropEvent(({ payload }) => {
  document.body.classList.toggle('dropping', payload.type === 'enter' || payload.type === 'over');
  if (payload.type === 'drop') addFolders(payload.paths);
});

/** Runs something and takes the user straight to its output. */
async function launch(project, script) {
  const id = await runScript(project, script);
  await call('open_terminal', { id });
}

function confirmButton(id, label, confirmLabel, onConfirm) {
  const armed = confirming === id;
  return h('button', {
    class: `btn ${armed ? 'danger' : ''}`,
    onClick: () => {
      if (armed) { confirming = null; onConfirm(); } else { confirming = id; }
      render();
    },
  }, armed ? confirmLabel : label);
}

async function saveSettings(change) {
  const next = { ...settings, ...change };
  if (await call('update_settings', { settings: next }).then(() => true, () => false)) settings = next;
  render();
}

async function setLaunchAtLogin(enabled) {
  await call('set_login_item', { enabled }).catch(() => {});
  launchAtLogin = await invoke('login_item');
  render();
}

async function checkNow() {
  checking = true;
  render();
  const status = await checkForUpdates().catch(() => null);
  checking = false;
  upToDate = status?.state === 'idle';
  render();
}

listen('update-changed', ({ payload }) => {
  if (upToDate && payload.state !== 'idle') { upToDate = false; render(); }
});

function shortcutLabel(shortcut) {
  const keys = shortcut.split('+').map((key) => key.replace(/^(Key|Digit)/, ''));
  return MAC ? keys.map((key) => KEY_SYMBOLS[key] ?? key).join('') : keys.join('+');
}

window.addEventListener('keydown', (event) => {
  if (!recording) return;
  event.preventDefault();
  if (event.key === 'Escape') {
    recording = false;
    render();
  } else if (!['Control', 'Alt', 'Shift', 'Meta'].includes(event.key) && (event.ctrlKey || event.altKey || event.metaKey)) {
    // Without Ctrl, Alt or Cmd, a global shortcut would swallow that key everywhere.
    recording = false;
    const modifiers = [event.ctrlKey && 'Control', event.altKey && 'Alt', event.shiftKey && 'Shift', event.metaKey && 'Super'];
    saveSettings({ shortcut: [...modifiers, event.code].filter(Boolean).join('+') });
  }
}, true);

function shortcutField() {
  const label = recording ? 'Type a shortcut…' : settings.shortcut ? shortcutLabel(settings.shortcut) : 'Record shortcut';
  return h('div', { class: 'inline' },
    settings.shortcut && !recording && iconButton('close', 'Remove shortcut', () => saveSettings({ shortcut: null })),
    h('button', {
      class: `btn keys ${recording ? 'recording' : ''}`,
      onClick: () => { recording = !recording; render(); },
    }, label));
}

const settingText = (title, note) => h('div', { class: 'meta' },
  h('div', { class: 'title' }, title),
  note && h('div', { class: 'sub' }, note));

const setting = (title, note, control) => h('div', { class: 'setting' }, settingText(title, note), control);

const switchSetting = (title, note, checked, onChange) => h('label', { class: 'setting' },
  settingText(title, note),
  h('input', {
    type: 'checkbox', class: 'switch', role: 'switch', checked, onChange: (event) => onChange(event.target.checked),
  }));

const group = (title, ...rows) => h('section', {}, h('h2', {}, title), h('div', { class: 'group' }, rows));

function segmented(label, options, value, onPick) {
  return h('div', { class: 'segmented', role: 'radiogroup', 'aria-label': label },
    options.map(([text, option]) => h('button', {
      class: option === value ? 'on' : '',
      role: 'radio',
      'aria-checked': String(option === value),
      onClick: () => onPick(option),
    }, text)));
}

function agentsGroup() {
  const copy = h('button', {
    class: 'btn',
    onClick: () => navigator.clipboard.writeText(mcpCommand).then(() => { copy.textContent = 'Copied'; }, toast),
  }, 'Copy');
  return group('AI agents',
    switchSetting('Let AI agents read and restart scripts (MCP)',
      'They can read output and start, restart or stop scripts, but never type into a terminal.',
      settings.mcp, (mcp) => saveSettings({ mcp })),
    setting('Add to Claude Code', h('span', { class: 'command mono' }, mcpCommand), copy));
}

function renderGeneral() {
  const fontSize = h('select', {
    'aria-label': 'Terminal font size',
    onChange: (event) => saveSettings({ terminalFontSize: Number(event.target.value) }),
  }, FONT_SIZES.map((size) => h('option', { value: size }, `${size} px`)));
  fontSize.value = settings.terminalFontSize;
  const themes = [['System', null], ['Light', 'light'], ['Dark', 'dark']];

  head.replaceChildren(h('h1', {}, 'General'));
  body.replaceChildren(
    group('Startup',
      switchSetting('Launch at login', 'ScriptDeck starts in the menu bar.', launchAtLogin, setLaunchAtLogin),
      switchSetting('Reopen running scripts', 'Starts the scripts that were still running when ScriptDeck quit.',
        settings.reopenScripts, (reopenScripts) => saveSettings({ reopenScripts }))),
    group('Keyboard',
      setting('Open ScriptDeck', 'Shows the menu bar window from any app.', shortcutField())),
    group('Notifications',
      switchSetting('Notify when a script fails', 'Not when you stop it, or while its terminal is in front.',
        settings.notifyOnFailure, (notifyOnFailure) => saveSettings({ notifyOnFailure })),
      switchSetting('Notify when a long script finishes', 'After 30 seconds or more. Not while its terminal is in front.',
        settings.notifyOnFinish, (notifyOnFinish) => saveSettings({ notifyOnFinish }))),
    group('Appearance',
      setting('Theme', null, segmented('Theme', themes, settings.theme ?? null, (theme) => saveSettings({ theme }))),
      setting('Terminal font size', null, fontSize)),
    mcpCommand ? agentsGroup() : '', // not null, which replaceChildren would print
    group('About',
      setting(`ScriptDeck ${version}`,
        upToDate ? 'You have the latest version.' : 'Free and open source under the MIT License.',
        h('div', { class: 'inline' },
          h('button', { class: 'btn', disabled: checking, onClick: checkNow }, checking ? 'Checking…' : 'Check for Updates'),
          h('button', { class: 'btn', onClick: () => call('open_homepage') }, 'GitHub')))));
}

function nodeSection(project) {
  const { node } = project.scan;
  const auto = node.source && node.source !== 'override' ? `Auto (${node.source}: ${node.wanted})` : 'Auto';
  const select = h('select', {
    'aria-label': 'Node version',
    disabled: !versions.length,
    onChange: async (event) => {
      project.nodeVersion = event.target.value || null;
      await saveProject(project);
      await load();
    },
  },
  h('option', { value: '' }, auto),
  versions.map((v) => h('option', { value: v.version }, `${v.version} (${v.manager})`)));
  select.value = project.nodeVersion ?? '';

  const missing = node.wanted && !node.resolved;
  const note = missing
    ? `${node.wanted} (from ${node.source}) is not installed.`
    : node.resolved
      ? `Scripts run with node ${node.resolved}.`
      : versions.length
        ? 'No .nvmrc, .node-version or Volta pin, so scripts use your shell default.'
        : 'No nvm, fnm or Volta found, so scripts use whatever node your shell gives them.';
  return h('section', {},
    h('h2', {}, 'Node version'),
    h('div', { class: 'inline' }, select,
      node.install && h('button', {
        class: 'btn primary',
        title: node.install,
        onClick: () => launch(project, { name: node.install, command: node.install }),
      }, `Install ${node.wanted}`)),
    h('p', { class: `note ${missing ? 'warn' : ''}` }, note),
    node.engines && h('p', { class: 'note' }, `package.json engines: ${node.engines}`));
}

function toolkitSection(project) {
  const run = (tool) => launch(project, { name: tool.label, command: tool.command });
  return h('section', {},
    h('h2', {}, `Toolkit (${project.scan.pm})`),
    h('div', { class: 'inline' }, project.scan.tools.map((tool) => (tool.label === 'Clean reinstall'
      ? confirmButton('clean', tool.label, 'Delete node_modules?', () => run(tool))
      : h('button', { class: 'btn', title: tool.command, onClick: () => run(tool) }, tool.label)))));
}

const customKey = (script) => `custom:${script.id}`; // the key scriptsOf gives it

/** Takes effect from the script's next start. */
const keepAliveSwitch = (project, key) => h('input', {
  type: 'checkbox', class: 'switch', role: 'switch', checked: project.keepAlive.includes(key),
  title: 'Restart if it crashes', 'aria-label': 'Restart if it crashes',
  onChange: (event) => {
    project.keepAlive = project.keepAlive.filter((k) => k !== key).concat(event.target.checked ? [key] : []);
    saveProject(project);
  },
});

function customSection(project) {
  const save = () => saveProject(project);
  const rows = project.custom.map((script) => h('div', { class: 'custom-row' },
    h('input', {
      value: script.name, 'aria-label': 'Script name', placeholder: 'Name',
      onChange: (e) => { script.name = e.target.value; save(); },
    }),
    h('input', {
      class: 'mono', value: script.command, 'aria-label': 'Command', placeholder: 'Command', spellcheck: 'false',
      onChange: (e) => { script.command = e.target.value; save(); },
    }),
    h('div', { class: 'actions' },
      keepAliveSwitch(project, customKey(script)),
      iconButton('play', 'Run', () => launch(project, { ...script, key: customKey(script) }), 'go'),
      iconButton('close', 'Delete', async () => {
        project.custom = project.custom.filter((s) => s !== script);
        project.keepAlive = project.keepAlive.filter((k) => k !== customKey(script));
        for (const group of project.groups) group.scripts = group.scripts.filter((key) => key !== customKey(script));
        await save();
        render();
      }, 'danger'))));

  const name = h('input', { placeholder: 'Name', 'aria-label': 'New script name' });
  const command = h('input', {
    class: 'mono', placeholder: 'pnpm -F @acme/app run start', 'aria-label': 'New script command', spellcheck: 'false',
  });
  const add = async (event) => {
    event.preventDefault();
    if (!command.value.trim()) return command.focus();
    const label = name.value.trim() || command.value.trim().split(/\s+/).slice(-2).join(' ');
    project.custom.push({ id: crypto.randomUUID(), name: label, command: command.value.trim() });
    await save();
    render();
  };
  return h('section', {},
    h('h2', {}, 'Custom scripts'),
    rows,
    h('form', { class: 'add-row', onSubmit: add }, name, command, h('button', { class: 'btn' }, 'Add')));
}

function groupsSection(project) {
  const save = () => saveProject(project);
  const scripts = scriptsOf(project);
  const blocks = project.groups.map((group) => {
    const tick = (key, label, props) => h('label', props,
      h('input', {
        type: 'checkbox',
        checked: group.scripts.includes(key),
        onChange: (e) => {
          group.scripts = e.target.checked ? [...group.scripts, key] : group.scripts.filter((k) => k !== key);
          save();
        },
      }),
      label);
    return h('div', { class: 'group script-group' },
      h('div', { class: 'inline' },
        h('input', {
          value: group.name, 'aria-label': 'Group name', placeholder: 'Name',
          onChange: (e) => { group.name = e.target.value.trim() || group.name; e.target.value = group.name; save(); },
        }),
        iconButton('close', 'Delete group', async () => {
          project.groups = project.groups.filter((g) => g !== group);
          await save();
          render();
        }, 'danger')),
      h('div', { class: 'ticks' },
        scripts.map((s) => tick(s.key, scriptLabel(s), { title: s.command })),
        groupScripts(project, group).missing.map((key) => tick(key, missingLabel(key), { class: 'warn' }))));
  });

  const name = h('input', { placeholder: 'Group name', 'aria-label': 'New group name' });
  const add = async (event) => {
    event.preventDefault();
    if (!name.value.trim()) return name.focus();
    project.groups.push({ id: crypto.randomUUID(), name: name.value.trim(), scripts: [] });
    await save();
    render();
  };
  return h('section', {},
    h('h2', {}, 'Groups'),
    blocks,
    h('form', { class: 'inline', onSubmit: add }, name, h('button', { class: 'btn' }, 'Add')),
    !project.groups.length && h('p', { class: 'note' }, 'Start and stop several scripts from one row in the menu bar.'));
}

function openEditor(state) {
  editing = state;
  confirming = null;
  render();
  body.querySelector('.script-editor input')?.focus();
}

const closeEditor = () => { editing = null; render(); };

/** script is [name, body], or null to delete. */
async function saveScript(project, target, script) {
  await call('edit_script', { projectId: project.id, package: target.package ?? null, old: target.old, script });
  editing = null;
  await load();
}

function scriptEditor(project) {
  const edit = editing;
  const field = (key, props) => h('input', {
    ...props, value: edit[key], 'aria-label': props.placeholder, spellcheck: 'false',
    onInput: (event) => { edit[key] = event.target.value; },
  });
  return h('form', {
    class: 'custom-row script-editor',
    onSubmit: (event) => { event.preventDefault(); saveScript(project, edit, [edit.name, edit.body]); },
    onKeydown: (event) => { if (event.key === 'Escape') closeEditor(); },
  },
  field('name', { placeholder: 'Script name' }),
  field('body', { placeholder: 'Command', class: 'mono' }),
  h('div', { class: 'actions' },
    h('button', { class: 'btn primary' }, 'Save'),
    h('button', { class: 'btn', type: 'button', onClick: closeEditor }, 'Cancel')));
}

function scriptRow(project, script) {
  if (editing?.key === script.key) return scriptEditor(project);
  const pinned = project.pinned.includes(script.key);
  const deleting = `delete:${script.key}`;
  return h('div', { class: 'row' },
    iconButton(pinned ? 'starred' : 'star', pinned ? 'Unpin' : 'Pin to menu', async () => {
      await togglePin(project, script);
      render();
    }, pinned ? 'pinned' : ''),
    h('div', { class: 'meta' },
      h('div', { class: 'title' }, script.name),
      h('div', { class: 'sub mono', title: script.body }, script.body)),
    keepAliveSwitch(project, script.key),
    iconButton('pencil', 'Edit script', () => openEditor({
      key: script.key, package: script.package, old: script.name, name: script.name, body: script.body,
    })),
    confirming === deleting
      ? confirmButton(deleting, '', 'Delete?', () => saveScript(project, { package: script.package, old: script.name }, null))
      : iconButton('close', 'Delete from package.json', () => { confirming = deleting; render(); }, 'danger'),
    iconButton('play', `Run: ${script.command}`, () => launch(project, script), 'go'));
}

const addScript = (pkg) => openEditor({ key: 'new', package: pkg, old: null, name: '', body: '' });

const packageTitle = (pkg) => h('div', { class: 'package-title', title: pkg },
  h('span', {}, shortPackage(pkg)),
  iconButton('plus', `Add a script to ${pkg}`, () => addScript(pkg)));

function scriptList(project) {
  const terms = filter.toLowerCase().split(/\s+/).filter(Boolean);
  const found = project.scan.scripts.filter((s) => {
    const text = `${s.package ?? ''} ${s.name} ${s.body}`.toLowerCase();
    return terms.every((term) => text.includes(term));
  });
  const adding = editing?.old === null;
  let last;
  // shortcut: a workspace package with no scripts gets no heading to add one from, list packages in the scan if that matters.
  const rows = found.slice(0, MAX_ROWS).flatMap((script) => {
    const first = script.package !== last && script.package;
    last = script.package;
    return [
      first && packageTitle(script.package),
      first && adding && editing.package === script.package && scriptEditor(project),
      scriptRow(project, script),
    ].filter(Boolean);
  });
  if (adding && !editing.package) rows.unshift(scriptEditor(project));
  if (found.length > MAX_ROWS) {
    rows.push(h('p', { class: 'note' }, `Showing ${MAX_ROWS} of ${found.length}. Filter to narrow it down.`));
  }
  return rows.length ? rows : [h('p', { class: 'note' }, 'No matching scripts.')];
}

function scriptsSection(project) {
  const total = project.scan.scripts.length;
  const results = h('div', {}, scriptList(project));
  return h('section', {},
    h('div', { class: 'section-head' },
      h('h2', {}, `package.json scripts (${total})`),
      iconButton('plus', 'Add a script', () => addScript(null))),
    total > 10 && h('input', {
      class: 'filter', type: 'search', placeholder: 'Filter by package or script…', value: filter,
      'aria-label': 'Filter scripts', spellcheck: 'false',
      onInput: (e) => { filter = e.target.value; results.replaceChildren(...scriptList(project)); },
    }),
    results);
}

/** The last git failure, when it came from the section `at`. */
function errorBox(at) {
  return gitError?.at === at && h('div', { class: 'git-error', role: 'alert' },
    h('pre', {}, gitError.text),
    iconButton('close', 'Dismiss', () => { gitError = null; render(); }));
}

/** A button for a git command from the section `at`; it shows `busy` while it runs. */
function gitButton(at, label, busy, run, extra = '') {
  return h('button', {
    class: `btn ${extra}`, disabled: gitBusy != null, onClick: () => gitAction(at, busy, run),
  }, gitBusy === busy ? busy : label);
}

function togglePicker(project) {
  picker = picker ? null : branchPicker(project, {
    onSwitched: () => { picker = null; load(); },
    onWorktree: (opened) => { picker = null; load().then(() => select(opened.id)); },
    onClose: () => { picker = null; render(); },
  });
  render();
  picker?.focus();
}

function branchSection(project) {
  const { status } = repo;
  const sync = (action, label, busy) => gitButton('branch', label, busy,
    () => invoke('git_sync', { projectId: project.id, action }));
  return h('section', {},
    h('h2', {}, 'Git'),
    h('div', { class: 'group' },
      h('div', { class: 'setting' },
        icon('branch'),
        settingText(branchName(status), syncText(status) || null),
        h('div', { class: 'inline' },
          h('button', { class: 'btn', 'aria-expanded': String(!!picker), onClick: () => togglePicker(project) },
            'Switch Branch'),
          sync('fetch', 'Fetch', 'Fetching…'),
          pullLabel(status) && sync('pull', pullLabel(status), 'Pulling…'),
          status.branch && sync('push', pushLabel(status), 'Pushing…'))),
      picker && h('div', { class: 'picker-box' }, picker.el)),
    errorBox('branch'));
}

/** The file's name, then its folder, dimmed: the name is what you look for. */
function fileLabel(path) {
  const cut = path.replace(/\/$/, '').lastIndexOf('/') + 1;
  return [h('span', { class: 'file-name' }, path.slice(cut)), cut > 0 && h('span', { class: 'dir' }, path.slice(0, cut - 1))];
}

function changesSection(project) {
  const { files } = repo.status;
  if (!files.length) {
    return h('section', {}, h('h2', {}, 'Changes'), h('p', { class: 'note' }, 'No uncommitted changes.'));
  }
  const chosen = files.filter((file) => !unchecked.has(file.path));
  const all = h('input', {
    type: 'checkbox', checked: chosen.length === files.length, 'aria-label': 'Commit every file',
    onChange: (event) => { unchecked = new Set(event.target.checked ? [] : files.map((f) => f.path)); render(); },
  });
  all.indeterminate = chosen.length > 0 && chosen.length < files.length;
  const rows = files.slice(0, MAX_ROWS).map((file) => {
    const [letter, meaning, colour] = KINDS[file.kind] ?? KINDS.M;
    return h('label', { class: 'change', title: file.from ? `${meaning}: ${file.from} → ${file.path}` : `${meaning}: ${file.path}` },
      h('input', {
        type: 'checkbox', checked: !unchecked.has(file.path),
        onChange: (event) => { unchecked[event.target.checked ? 'delete' : 'add'](file.path); render(); },
      }),
      h('span', { class: `kind ${colour}`, 'aria-label': meaning }, letter),
      h('span', { class: 'file' }, fileLabel(file.path)));
  });
  // The ticked files, by name: files that turned up after this list loaded don't go in unseen.
  const commit = (push) => async () => {
    const paths = chosen.map((file) => file.path);
    await invoke('git_commit', { projectId: project.id, message: commitInput.value, paths });
    commitInput.value = '';
    unchecked = new Set();
    if (push) await invoke('git_sync', { projectId: project.id, action: 'push' });
  };
  const count = `${chosen.length} file${chosen.length === 1 ? '' : 's'}`;
  const commitButton = gitButton('changes', `Commit ${count}`, 'Committing…', commit(false), 'primary');
  const pushButton = repo.status.branch
    && gitButton('changes', 'Commit & Push', 'Committing and pushing…', commit(true));
  for (const button of [commitButton, pushButton]) if (button && !chosen.length) button.disabled = true;
  return h('section', {},
    h('div', { class: 'section-head' },
      h('h2', {}, `Changes (${files.length})`),
      h('label', { class: 'inline all' }, all, 'All')),
    h('div', { class: 'group changes' },
      h('div', { class: 'files' }, rows,
        files.length > MAX_ROWS && h('p', { class: 'note' }, `And ${files.length - MAX_ROWS} more, all committed unless you untick All.`)),
      h('div', { class: 'commit' },
        commitInput,
        h('div', { class: 'inline' }, commitButton, pushButton,
          h('span', { class: 'note' }, MAC ? '⌘↩ commits' : 'Ctrl+Enter commits')))),
    errorBox('changes'));
}

function worktreesSection(project) {
  const open = (tree) => async () => {
    let opened;
    const done = await gitAction('worktrees', 'Adding…', async () => {
      opened = await invoke('git_open_worktree', { projectId: project.id, path: tree.path });
    });
    if (done) select(opened.id);
  };
  const rows = repo.worktrees.map((tree) => {
    const added = projects.find((p) => p.id === tree.project);
    const removing = `worktree:${tree.path}`;
    return h('div', { class: 'setting' },
      icon('folder'),
      h('div', { class: 'meta' },
        h('div', { class: 'title' }, tree.branch ?? 'detached HEAD', tree.main && h('span', { class: 'tag' }, 'main')),
        h('div', { class: 'sub path', title: tree.path }, tree.path)),
      h('div', { class: 'inline' },
        tree.current
          ? h('span', { class: 'note' }, 'This project')
          : added
            ? h('button', { class: 'btn', onClick: () => select(added.id) }, 'Open')
            : gitButton('worktrees', 'Add to ScriptDeck', 'Adding…', open(tree)),
        !tree.main && confirmButton(removing, 'Remove', 'Delete its folder?',
          () => gitAction('worktrees', 'Removing…',
            () => invoke('git_remove_worktree', { projectId: project.id, path: tree.path })))));
  });

  const base = h('select', {
    'aria-label': 'Start a new branch from', title: 'Where a new branch starts',
    onChange: (event) => { worktreeBase = event.target.value; },
  },
  h('option', { value: '' }, 'HEAD'),
  repo.branches.map((b) => h('option', { value: b.name }, b.name)));
  base.value = worktreeBase ?? repo.status.branch ?? '';
  const create = async (event) => {
    event.preventDefault();
    let opened;
    const done = await gitAction('worktrees', 'Creating…', async () => {
      opened = await invoke('git_add_worktree', {
        projectId: project.id, branch: worktreeInput.value, base: base.value || null,
      });
    });
    if (!done) return;
    worktreeInput.value = '';
    worktreeBase = null;
    select(opened.id);
  };
  const main = repo.worktrees.find((tree) => tree.main)?.path.split(/[\\/]/).pop() ?? 'the repository';
  return h('section', {},
    h('h2', {}, `Worktrees (${repo.worktrees.length})`),
    h('div', { class: 'group' }, rows),
    h('form', { class: 'inline worktree-form', onSubmit: create },
      worktreeInput,
      h('datalist', { id: 'worktree-branches' }, repo.branches.map((b) => h('option', { value: b.name }))),
      h('span', { class: 'note' }, 'from'),
      base,
      h('button', { class: 'btn', disabled: gitBusy != null }, gitBusy === 'Creating…' ? 'Creating…' : 'New Worktree')),
    errorBox('worktrees'),
    h('p', { class: 'note' },
      `A worktree is another folder with its own branch checked out, so two branches can run side by side. New ones go beside ${main} and open as projects with this one's settings. Run Install in them first.`));
}

function gitSections(project) {
  if (repoFor !== project.id) return [];
  if (!repo) return gitError?.at === 'branch' ? [h('section', {}, h('h2', {}, 'Git'), errorBox('branch'))] : [];
  return [branchSection(project), changesSection(project), worktreesSection(project)];
}

function renderClone() {
  const parent = cloneParent ?? projects.at(-1)?.path.replace(/[\\/][^\\/]*$/, '') ?? null;
  const choose = async () => {
    const path = await call('pick_folder');
    if (path) { cloneParent = path; render(); }
  };
  const clone = async (event) => {
    event.preventDefault();
    if (!parent) return choose();
    let added;
    const done = await gitAction('clone', 'Cloning…', async () => {
      added = await invoke('git_clone', { url: cloneInput.value, parent });
    });
    if (done) { cloneInput.value = ''; select(added.id); }
  };
  head.replaceChildren(h('h1', {}, 'Clone a Repository'));
  body.replaceChildren(h('form', { class: 'clone', onSubmit: clone },
    group('Repository',
      setting('URL', 'HTTPS or SSH, signed in as git is in your terminal.', cloneInput),
      setting('Into', parent ?? 'Choose a folder', h('button', { class: 'btn', type: 'button', onClick: choose }, 'Choose…'))),
    h('div', { class: 'inline actions' },
      h('button', { class: 'btn primary', disabled: gitBusy != null }, gitBusy ?? 'Clone'),
      h('span', { class: 'note' }, 'It gets a folder of its own in there, and opens as a project.')),
    errorBox('clone')));
}

function renderProject(project) {
  const remove = async () => {
    await call('remove_project', { id: project.id });
    await load();
  };
  const { error } = project.scan;
  head.replaceChildren(
    h('input', {
      class: 'name-input', value: project.name, 'aria-label': 'Project name',
      onChange: async (e) => {
        project.name = e.target.value.trim() || project.name;
        await saveProject(project);
        render();
      },
    }),
    h('div', { class: 'path-row' },
      h('span', { class: 'path', title: project.path, 'data-tauri-drag-region': 'false' }, project.path),
      h('button', { class: 'link', onClick: () => call('reveal', { path: project.path }) }, 'Show in folder'),
      confirmButton('remove', 'Remove', 'Remove from ScriptDeck?', remove)));
  // Git stays reachable when there's no package.json, say on a branch without one.
  body.replaceChildren(...(error
    ? [h('p', { class: 'note warn' }, error), ...gitSections(project)]
    : [nodeSection(project), toolkitSection(project), ...gitSections(project), customSection(project),
      groupsSection(project), scriptsSection(project)]));
}

function navItem(id, glyph, label) {
  return h('button', {
    class: `item ${id === selectedId ? 'active' : ''}`,
    'aria-current': id === selectedId ? 'page' : false,
    onClick: () => select(id),
  }, icon(glyph), h('span', { class: 'label' }, label));
}

function render() {
  const focused = document.activeElement; // kept if it survives the rebuild, like the commit message
  sidebar.replaceChildren(
    navItem(GENERAL, 'gear', 'General'),
    h('div', { class: 'nav-title' },
      h('span', {}, 'Projects'),
      h('div', { class: 'tools' },
        iconButton('download', 'Clone a repository', () => select(CLONE), selectedId === CLONE ? 'on' : ''),
        iconButton('plus', 'Add project', addProject))),
    h('div', { class: 'projects' }, projects.map((project) => navItem(project.id, 'folder', project.name))),
    h('div', { class: 'foot' }, h('button', { class: 'btn', onClick: () => invoke('quit') }, 'Quit ScriptDeck')),
  );
  const project = selected();
  if (selectedId === GENERAL) renderGeneral();
  else if (selectedId === CLONE) renderClone();
  else if (project) renderProject(project);
  else {
    head.replaceChildren(h('h1', {}, 'Welcome to ScriptDeck'));
    body.replaceChildren(h('div', { class: 'empty' },
      h('div', {}, 'Add a folder with a package.json to get started.'),
      h('div', { class: 'inline center' },
        h('button', { class: 'btn primary', onClick: addProject }, 'Add project'),
        h('button', { class: 'btn', onClick: () => select(CLONE) }, 'Clone a repository')),
      h('p', { class: 'note' }, 'Or drop folders on this window.')));
  }
  if (focused?.isConnected && focused !== document.activeElement) focused.focus();
}

listen('select-project', ({ payload }) => load().then(() => select(payload)));

// Reloading rebuilds the page, which would knock the cursor out of an open script editor.
window.addEventListener('focus', () => { if (!editing) load(); });
load();
