import { h } from './shared.js';

/** The Settings window's building blocks: titled groups of rows, each a title and note with a control. */

export const settingText = (title, note) => h('div', { class: 'meta' },
  h('div', { class: 'title' }, title),
  note && h('div', { class: 'sub' }, note));

export const setting = (title, note, control) => h('div', { class: 'setting' }, settingText(title, note), control);

export const switchSetting = (title, note, checked, onChange) => h('label', { class: 'setting' },
  settingText(title, note),
  h('input', {
    type: 'checkbox', class: 'switch', role: 'switch', checked, onChange: (event) => onChange(event.target.checked),
  }));

export const group = (title, ...rows) => h('section', {}, h('h2', {}, title), h('div', { class: 'group' }, rows));

export function segmented(label, options, value, onPick) {
  return h('div', { class: 'segmented', role: 'radiogroup', 'aria-label': label },
    options.map(([text, option]) => h('button', {
      class: option === value ? 'on' : '',
      role: 'radio',
      'aria-checked': String(option === value),
      onClick: () => onPick(option),
    }, text)));
}
