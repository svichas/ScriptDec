import { call, h, iconButton, invoke, runScript, saveProject, shortPackage, togglePin } from './shared.js';

const MAX_ROWS = 80;

let projects = [];
let versions = [];
let selectedId = null;
let filter = '';
let confirming = null; // the destructive action waiting for its second click
let editing = null; // the package.json script being changed: { key, package, old, name, body }; old is null for a new one
let launchAtLogin = false;

const sidebar = h('nav', { class: 'sidebar', 'aria-label': 'Projects' });
const detail = h('main', { class: 'detail' });
document.body.append(h('div', { class: 'settings' }, sidebar, detail));

const selected = () => projects.find((p) => p.id === selectedId);

async function load() {
  [projects, versions, launchAtLogin] = await Promise.all([
    invoke('list_projects'), invoke('node_versions'), invoke('login_item'),
  ]);
  if (!selected()) selectedId = projects[0]?.id ?? null;
  render();
}

async function addProject() {
  const path = await call('pick_folder');
  if (!path) return;
  const project = await call('add_project', { path });
  selectedId = project.id;
  await load();
}

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
      iconButton('play', 'Run', () => launch(project, script), 'go'),
      iconButton('close', 'Delete', async () => {
        project.custom = project.custom.filter((s) => s !== script);
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

function openEditor(state) {
  editing = state;
  confirming = null;
  render();
  detail.querySelector('.script-editor input')?.focus();
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

function renderDetail(project) {
  const remove = async () => {
    await call('remove_project', { id: project.id });
    await load();
  };
  const { error } = project.scan;
  detail.replaceChildren(
    h('input', {
      class: 'name-input', value: project.name, 'aria-label': 'Project name',
      onChange: async (e) => {
        project.name = e.target.value.trim() || project.name;
        await saveProject(project);
        render();
      },
    }),
    h('div', { class: 'path-row' },
      h('span', { class: 'path', title: project.path }, project.path),
      h('button', { class: 'link', onClick: () => call('reveal', { path: project.path }) }, 'Show in folder'),
      confirmButton('remove', 'Remove', 'Remove from ScriptDec?', remove)),
    ...(error
      ? [h('p', { class: 'note warn' }, error)]
      : [nodeSection(project), toolkitSection(project), customSection(project), scriptsSection(project)]),
  );
}

function render() {
  sidebar.replaceChildren(
    h('div', { class: 'projects' }, projects.map((project) => h('button', {
      class: `item ${project.id === selectedId ? 'active' : ''}`,
      onClick: () => { selectedId = project.id; filter = ''; confirming = null; editing = null; render(); },
    }, project.name))),
    h('div', { class: 'foot' },
      h('label', { class: 'check' },
        h('input', {
          type: 'checkbox',
          checked: launchAtLogin,
          onChange: async (event) => {
            await call('set_login_item', { enabled: event.target.checked }).catch(() => {});
            launchAtLogin = await invoke('login_item');
            render();
          },
        }),
        'Launch at login'),
      h('button', { class: 'btn primary', onClick: addProject }, 'Add project'),
      h('button', { class: 'btn', onClick: () => invoke('quit') }, 'Quit ScriptDec')),
  );
  const project = selected();
  if (project) renderDetail(project);
  else {
    detail.replaceChildren(h('div', { class: 'empty' },
      h('div', {}, 'Add a folder with a package.json to get started.'),
      h('button', { class: 'btn primary', onClick: addProject }, 'Add project')));
  }
}

// Reloading rebuilds the page, which would knock the cursor out of an open script editor.
window.addEventListener('focus', () => { if (!editing) load(); });
load();
