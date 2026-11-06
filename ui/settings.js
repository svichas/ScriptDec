import { combo } from './combo.js';
import {
  call, groupScripts, h, icon, iconButton, invoke, listen, listProjects, MAC, missingLabel, runScript, saveProject,
  scriptLabel, scriptsOf, shortPackage, toast, togglePin,
} from './shared.js';
import {
  gitChip, gitDropdown, initGit, loadRepo, renderClone, resetGit, revealWorktrees, worktreesSection,
} from './settings-git.js';
import { group, segmented, setting, switchSetting } from './settings-ui.js';
import { checkForUpdates, updateBanner } from './update.js';

const MAX_ROWS = 80;
const GENERAL = 'general';
const CLONE = 'clone';
const FONT_SIZES = [10, 11, 12, 13, 14, 15, 16, 18, 20];
const KEY_SYMBOLS = { Control: '⌃', Alt: '⌥', Shift: '⇧', Super: '⌘' };

let projects = [];
let versions = [];
let settings = {};
let version = '';
// A project's id, GENERAL or CLONE. The menu bar can open the window on a project, and on a part of its page.
const opened = new URLSearchParams(location.search);
let selectedId = opened.get('project');
if (opened.get('section') === 'worktrees') revealWorktrees();
let filter = '';
let confirming = null; // the destructive action waiting for its second click
let editing = null; // the package.json script being changed: { key, package, old, name, body }; old is null for a new one
let recording = false; // the shortcut button is waiting for keys
let launchAtLogin = false;
let notifications = 'granted'; // whether macOS shows ScriptDeck's: 'granted', 'denied' or 'prompt' (not asked yet)
let checking = false; // Check for Updates is waiting on GitHub
let upToDate = false; // and found nothing newer
const mcpCommand = await invoke('mcp_command'); // null on Windows, where agents can't connect yet

const sidebar = h('nav', { class: 'sidebar', 'aria-label': 'Sections', 'data-tauri-drag-region': 'deep' });
const head = h('header', { class: 'page-head', 'data-tauri-drag-region': 'deep' });
const body = h('div', { class: 'page-body' });
document.body.append(h('div', { class: 'settings' }, sidebar, h('main', { class: 'detail' }, head, body, updateBanner)));

const selected = () => projects.find((p) => p.id === selectedId);

async function load() {
  // Read again at every focus, so coming back from System Settings shows the change.
  [projects, versions, launchAtLogin, settings, version, notifications] = await Promise.all([
    listProjects(), invoke('node_versions'), invoke('login_item'), invoke('settings'),
    window.__TAURI__.app.getVersion(), invoke('notification_permission'),
  ]);
  if (selectedId !== GENERAL && selectedId !== CLONE && !selected()) selectedId = projects[0]?.id ?? null;
  render();
  await showRepo();
}

/** The page draws git as last read; this reads it again and draws what changed. */
async function showRepo() {
  const id = selected()?.id;
  const editor = editing;
  await loadRepo(id);
  // Not for a page left meanwhile, nor while a script editor opened meanwhile: rebuilding it would take the cursor
  // out. The next render shows it.
  if (id === selected()?.id && editing === editor) render();
}

function select(id) {
  selectedId = id;
  filter = '';
  confirming = null;
  editing = null;
  recording = false;
  resetGit();
  body.scrollTop = 0;
  render();
  showRepo();
}

initGit({ render: () => render(), load, select, projects: () => projects });

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

/** Only while macOS holds the notifications back. It asks once; after a no, only System Settings can allow them. */
function permissionRow() {
  if (notifications === 'granted') return null;
  const denied = notifications === 'denied';
  return setting(denied ? 'Notifications are off for ScriptDeck' : 'Allow notifications',
    h('div', { class: 'warn' }, denied
      ? 'macOS is blocking them. Switch them on in System Settings to get the ones below.'
      : 'macOS shows the ones below only once you allow them.'),
    h('button', {
      class: 'btn primary',
      onClick: async () => {
        notifications = await call('allow_notifications').catch(() => notifications);
        render();
      },
    }, denied ? 'Open System Settings' : 'Allow'));
}

function renderGeneral() {
  const fontSize = h('select', {
    'aria-label': 'Terminal font size',
    onChange: (event) => saveSettings({ terminalFontSize: Number(event.target.value) }),
  }, FONT_SIZES.map((size) => h('option', { value: size }, `${size} px`)));
  fontSize.value = settings.terminalFontSize;
  const themes = [['System', null], ['Light', 'light'], ['Dark', 'dark']];

  head.replaceChildren(h('h1', {}, 'Preferences'));
  body.replaceChildren(
    group('Startup',
      switchSetting('Launch at login', 'ScriptDeck starts in the menu bar.', launchAtLogin, setLaunchAtLogin),
      switchSetting('Reopen running scripts', 'Starts the scripts that were still running when ScriptDeck quit.',
        settings.reopenScripts, (reopenScripts) => saveSettings({ reopenScripts }))),
    group('Keyboard',
      setting('Open ScriptDeck', 'Shows the menu bar window from any app.', shortcutField())),
    group('Notifications',
      permissionRow(),
      switchSetting('Notify when a script fails', 'Not when you stop it, or while its terminal is in front.',
        settings.notifyOnFailure, (notifyOnFailure) => saveSettings({ notifyOnFailure })),
      switchSetting('Notify when tests pass or fail',
        'However long they take: scripts with test, e2e, vitest or jest in their name or command.',
        settings.notifyOnTests, (notifyOnTests) => saveSettings({ notifyOnTests })),
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
  return group('Node',
    setting('Version',
      [h('div', { class: missing ? 'warn' : '' }, note), node.engines && h('div', {}, `package.json engines: ${node.engines}`)],
      h('div', { class: 'inline' },
        node.install && h('button', {
          class: 'btn primary',
          title: node.install,
          onClick: () => launch(project, { name: node.install, command: node.install }),
        }, `Install ${node.wanted}`),
        select)));
}

/** Each tool says what it does and the command it runs, so nothing is a surprise. */
function toolkitSection(project) {
  const run = (tool) => launch(project, { name: tool.label, command: tool.command });
  return group(`Toolkit · ${project.scan.pm}`, project.scan.tools.map((tool) => h('div', { class: 'setting tool' },
    h('div', { class: 'meta' },
      h('div', { class: 'title' }, tool.label, h('code', { class: 'mono', title: tool.command }, tool.command)),
      h('div', { class: 'sub' }, tool.note)),
    tool.label === 'Clean reinstall'
      ? confirmButton('clean', 'Run', 'Delete node_modules?', () => run(tool))
      : h('button', { class: 'btn', onClick: () => run(tool) }, 'Run'))));
}

const customKey = (script) => `custom:${script.id}`; // the key scriptsOf gives it

const sectionHead = (title, ...actions) => h('div', { class: 'section-head' }, h('h2', {}, title), actions);

/** An x that asks first: its second click, on "Delete?", deletes. */
function deleteButton(id, title, onConfirm) {
  if (confirming === id) return confirmButton(id, '', 'Delete?', onConfirm);
  return iconButton('close', title, () => { confirming = id; render(); }, 'danger reveal');
}

/** A toggle, lit while on, like the pin. Takes effect from the script's next start. */
function keepAliveButton(project, key) {
  const on = project.keepAlive.includes(key);
  const button = iconButton('pulse', 'Restart if it crashes', async () => {
    project.keepAlive = on ? project.keepAlive.filter((k) => k !== key) : [...project.keepAlive, key];
    await saveProject(project);
    render();
  }, on ? 'alive' : 'reveal');
  button.setAttribute('aria-pressed', String(on));
  return button;
}

async function deleteCustom(project, script) {
  project.custom = project.custom.filter((c) => customKey(c) !== script.key);
  project.keepAlive = project.keepAlive.filter((k) => k !== script.key);
  for (const g of project.groups) g.scripts = g.scripts.filter((key) => key !== script.key);
  await saveProject(project);
  render();
}

/** Only the command is needed: the name defaults to its last two words. */
async function saveCustom(project, edit) {
  const command = edit.body.trim();
  if (!command) return body.querySelector('.script-editor .mono')?.focus();
  const name = edit.name.trim() || command.split(/\s+/).slice(-2).join(' ');
  const script = project.custom.find((c) => customKey(c) === edit.key);
  if (script) Object.assign(script, { name, command });
  else project.custom.push({ id: crypto.randomUUID(), name, command });
  editing = null;
  await saveProject(project);
  render();
}

function customSection(project) {
  const scripts = scriptsOf(project).filter((s) => s.custom);
  const adding = editing?.key === 'new-custom';
  return h('section', {},
    sectionHead('Custom scripts',
      iconButton('plus', 'Add a custom script', () => openEditor({ key: 'new-custom', custom: true, name: '', body: '' }))),
    scripts.length || adding
      ? h('div', { class: 'group list' }, scripts.map((s) => scriptRow(project, s)), adding && scriptEditor(project))
      : h('p', { class: 'note' }, "Commands that aren't in package.json, like docker compose up. They show in the menu bar."));
}

/** Starts a group's scripts, leaving the ones already running be, and shows the first one's output. */
async function runGroup(project, scripts) {
  const [id] = await Promise.all(scripts.map((script) => runScript(project, script)));
  await call('open_terminal', { id });
}

// Each group's Add script menu, kept across renders so it stays open, with what's typed, as the page redraws.
const pickers = new Map();

function addPicker(projectId, groupId) {
  if (!pickers.has(groupId)) {
    pickers.set(groupId, combo({
      label: 'Add a script to this group', glyph: 'plus', placeholder: 'Find a script…',
      // The page may have reloaded since the menu was made, so the project is looked up afresh.
      onPick: async (key) => {
        const project = projects.find((p) => p.id === projectId);
        const target = project?.groups.find((g) => g.id === groupId);
        if (!target) return;
        target.scripts.push(key);
        await saveProject(project);
        render();
      },
    }));
  }
  return pickers.get(groupId);
}

function groupBlock(project, group) {
  const { scripts, missing } = groupScripts(project, group);
  const save = async () => { await saveProject(project); render(); };
  const member = (key, label, gone = false) => h('span', {
    class: `member ${gone ? 'warn' : ''}`, title: gone && 'Not in package.json any more',
  },
    label,
    iconButton('close', `Take ${label} out of the group`, () => {
      group.scripts = group.scripts.filter((k) => k !== key);
      save();
    }));
  const picker = addPicker(project.id, group.id);
  const others = scriptsOf(project).filter((s) => !group.scripts.includes(s.key));
  picker.set(others.map((s) => ({ value: s.key, text: scriptLabel(s) })), 'Add script');
  return h('div', { class: 'script-group', 'data-group': group.id },
    h('div', { class: 'group-head' },
      h('input', {
        class: 'group-name', value: group.name, 'aria-label': 'Group name', spellcheck: 'false',
        onChange: (e) => { group.name = e.target.value.trim() || group.name; e.target.value = group.name; save(); },
      }),
      scripts.length > 0 && iconButton('play', 'Run group', () => runGroup(project, scripts), 'go'),
      deleteButton(`group:${group.id}`, 'Delete group', () => {
        project.groups = project.groups.filter((g) => g !== group);
        save();
      })),
    h('div', { class: 'members' },
      scripts.map((s) => member(s.key, scriptLabel(s))),
      missing.map((key) => member(key, missingLabel(key), true)),
      picker.el));
}

function groupsSection(project) {
  const add = async () => {
    const id = crypto.randomUUID();
    project.groups.push({ id, name: 'New group', scripts: [] });
    await saveProject(project);
    render();
    body.querySelector(`[data-group="${id}"] .group-name`)?.select();
  };
  return h('section', {},
    sectionHead('Groups', iconButton('plus', 'New group', add)),
    project.groups.length
      ? h('div', { class: 'group groups' }, project.groups.map((g) => groupBlock(project, g)))
      : h('p', { class: 'note' }, 'Start and stop several scripts from one row in the menu bar.'));
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
    onSubmit: (event) => {
      event.preventDefault();
      if (edit.custom) saveCustom(project, edit);
      else saveScript(project, edit, [edit.name, edit.body]);
    },
    onKeydown: (event) => { if (event.key === 'Escape') closeEditor(); },
  },
  field('name', { placeholder: edit.custom ? 'Name (optional)' : 'Script name' }),
  field('body', { placeholder: edit.custom ? 'docker compose up' : 'Command', class: 'mono' }),
  h('div', { class: 'actions' },
    h('button', { class: 'btn primary' }, 'Save'),
    h('button', { class: 'btn', type: 'button', onClick: closeEditor }, 'Cancel')));
}

/** A package.json script, or a custom one, which is always in the menu so has no pin. */
function scriptRow(project, script) {
  if (editing?.key === script.key) return scriptEditor(project);
  const pinned = project.pinned.includes(script.key);
  return h('div', { class: 'row' },
    script.custom
      ? h('span', { class: 'kind' }, icon('terminal'))
      : iconButton(pinned ? 'starred' : 'star', pinned ? 'Unpin' : 'Pin to menu', async () => {
        await togglePin(project, script);
        render();
      }, pinned ? 'pinned' : ''),
    h('div', { class: 'meta' },
      h('div', { class: 'title' }, script.name),
      h('div', { class: 'sub mono', title: script.body }, script.body)),
    keepAliveButton(project, script.key),
    iconButton('pencil', 'Edit script', () => openEditor({
      key: script.key, custom: script.custom, package: script.package, old: script.name, name: script.name, body: script.body,
    }), 'reveal'),
    deleteButton(`delete:${script.key}`, script.custom ? 'Delete script' : 'Delete from package.json', () => (script.custom
      ? deleteCustom(project, script)
      : saveScript(project, { package: script.package, old: script.name }, null))),
    iconButton('play', `Run: ${script.command}`, () => launch(project, script), 'go'));
}

const addScript = (pkg) => openEditor({ key: 'new', package: pkg, old: null, name: '', body: '' });

const packageTitle = (pkg) => h('div', { class: 'package-title', title: pkg },
  h('span', {}, shortPackage(pkg)),
  iconButton('plus', `Add a script to ${pkg}`, () => addScript(pkg), 'reveal'));

function scriptList(project) {
  const terms = filter.toLowerCase().split(/\s+/).filter(Boolean);
  const found = project.scan.scripts.filter((s) => {
    const text = `${s.package ?? ''} ${s.name} ${s.body}`.toLowerCase();
    return terms.every((term) => text.includes(term));
  });
  const adding = editing?.key === 'new';
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
  return rows.length ? rows : [h('p', { class: 'note' }, terms.length ? 'No matching scripts.' : 'No scripts yet.')];
}

function scriptsSection(project) {
  const total = project.scan.scripts.length;
  const results = h('div', { class: 'group list' }, scriptList(project));
  return h('section', {},
    sectionHead(`package.json scripts (${total})`,
      total > 10 && h('label', { class: 'search' }, icon('search'), h('input', {
        type: 'search', placeholder: 'Filter', value: filter, 'aria-label': 'Filter scripts', spellcheck: 'false',
        onInput: (e) => { filter = e.target.value; results.replaceChildren(...scriptList(project)); },
      })),
      iconButton('plus', 'Add a script', () => addScript(null))),
    results);
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
      // The folder its scripts run in: another worktree's, once it's switched there.
      h('span', { class: 'path', title: project.dir, 'data-tauri-drag-region': 'false' }, project.dir),
      gitChip(project),
      h('button', { class: 'link', onClick: () => call('reveal', { path: project.dir }) }, 'Show in folder'),
      confirmButton('remove', 'Remove', 'Remove from ScriptDeck?', remove)),
    gitDropdown(project) ?? '');
  // Git is secondary here: the chip up top, the worktrees last. Both stay when there's no package.json, say on a
  // branch without one.
  body.replaceChildren(...[
    ...(error
      ? [h('p', { class: 'note warn' }, error)]
      : [nodeSection(project), toolkitSection(project), customSection(project), groupsSection(project),
        scriptsSection(project)]),
    worktreesSection(project),
  ].filter(Boolean));
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
    navItem(GENERAL, 'gear', 'Preferences'),
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
  else if (selectedId === CLONE) renderClone(head, body);
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

// From the menu bar: [project id, the part of its page to show, like "worktrees"]. At once, from what this window
// has; the reload after it brings anything newer. Waiting for it first would wait on git too.
listen('select-project', ({ payload: [id, section] }) => {
  if (section === 'worktrees') revealWorktrees();
  select(id);
  load();
});

// Reloading rebuilds the page, which would knock the cursor out of an open script editor.
window.addEventListener('focus', () => { if (!editing) load(); });
load();
