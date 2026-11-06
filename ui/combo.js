import { h, icon, matcher } from './shared.js';

/**
 * A select you can search, drawn like the branch switcher: a button showing the choice, and under it a field and
 * the options that match. Build it once and keep it, so it stays open, with what's typed, while the page around it
 * is drawn again; `set` gives it the options and the choice each time.
 */
export function combo({ label, glyph, placeholder, onPick }) {
  let options = []; // { value, text }
  let value = null;
  let open = false;
  let cursor = 0;
  let shown = [];

  const text = h('span', { class: 'name' });
  const button = h('button', {
    type: 'button',
    class: 'btn combo-button',
    'aria-label': label,
    'aria-haspopup': 'listbox',
    'aria-expanded': 'false',
    'data-tip': label,
    onClick: () => (open ? close() : show()),
  }, icon(glyph), text, icon('chevron'));
  const input = h('input', {
    placeholder, 'aria-label': label, spellcheck: 'false', autocomplete: 'off',
    onInput: () => { cursor = 0; draw(); },
    onKeydown: onKey,
  });
  const list = h('div', { class: 'picker-list', role: 'listbox', 'aria-label': label });
  const menu = h('div', { class: 'picker-box combo-menu', hidden: true }, h('div', { class: 'picker' }, input, list));
  const el = h('div', { class: 'combo' }, button, menu);

  // A click anywhere else puts it away, as a menu goes.
  document.addEventListener('mousedown', (event) => {
    if (open && !el.contains(event.target)) close();
  });

  function draw() {
    const query = input.value.trim();
    const matches = matcher(query);
    // The name typed comes first, so Enter on "main" can't pick a longer one.
    shown = options
      .filter((option) => matches(option.text))
      .sort((a, b) => (b.text === query) - (a.text === query));
    cursor = Math.min(cursor, Math.max(shown.length - 1, 0));
    list.replaceChildren(...(shown.length
      ? shown.map((option, i) => h('div', {
        class: `row option ${i === cursor ? 'selected' : ''}`,
        role: 'option',
        'aria-selected': String(option.value === value),
        // Keeps the field focused through the click.
        onMousedown: (event) => event.preventDefault(),
        onClick: () => pick(option),
      },
      icon(glyph),
      h('div', { class: 'meta line' }, h('span', { class: 'title' }, option.text)),
      option.value === value && h('span', { class: 'chosen' }, icon('check'))))
      : [h('div', { class: 'hint' }, 'Nothing matches.')]));
  }

  function show() {
    open = true;
    // Upwards when the window has no room for it below, as at the bottom of a page.
    menu.classList.toggle('up', innerHeight - button.getBoundingClientRect().bottom < 320);
    menu.hidden = false;
    button.setAttribute('aria-expanded', 'true');
    input.value = '';
    cursor = Math.max(0, options.findIndex((option) => option.value === value));
    draw();
    input.focus();
    list.querySelector('.selected')?.scrollIntoView({ block: 'nearest' });
  }

  function close() {
    open = false;
    menu.hidden = true;
    button.setAttribute('aria-expanded', 'false');
  }

  function pick(option) {
    close();
    button.focus();
    if (option.value === value) return;
    value = option.value;
    text.textContent = option.text;
    onPick(option.value);
  }

  function onKey(event) {
    if (event.key === 'Escape') {
      event.stopPropagation();
      close();
      button.focus();
    } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      cursor = Math.max(0, Math.min(shown.length - 1, cursor + (event.key === 'ArrowDown' ? 1 : -1)));
      draw();
      list.querySelector('.selected')?.scrollIntoView({ block: 'nearest' });
    } else if (event.key === 'Enter') {
      event.preventDefault(); // in a form, Enter would submit it
      if (shown[cursor]) pick(shown[cursor]);
    }
  }

  return {
    el,
    set(next, chosen) {
      options = next;
      value = chosen;
      text.textContent = next.find((option) => option.value === chosen)?.text ?? chosen;
      if (open) draw();
    },
  };
}
