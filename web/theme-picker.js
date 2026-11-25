import { THEMES } from './theme-state.js';
export function setupThemePicker(state) {
  const trigger = document.getElementById('theme-trigger');
  const dialog = document.getElementById('theme-picker');
  const choices = dialog.querySelectorAll('input[name="theme"]');
  const unsubscribe = state.subscribe(theme => {
    for (const choice of choices) choice.checked = choice.value === theme;
  });
  const open = () => { dialog.showModal(); dialog.querySelector(':checked').focus(); };
  const close = () => { trigger.focus(); };
  const change = event => { if (THEMES.includes(event.target.value)) state.select(event.target.value); };
  const key = event => { event.stopPropagation(); };
  trigger.addEventListener('click', open);
  // Click also covers re-selecting the already-checked theme, which fires no
  // change event but must still save it as the local preference.
  dialog.addEventListener('change', change);
  dialog.addEventListener('click', change);
  dialog.addEventListener('keydown', key);
  dialog.addEventListener('close', close);
  return () => {
    unsubscribe();
    trigger.removeEventListener('click', open);
    dialog.removeEventListener('change', change);
    dialog.removeEventListener('click', change);
    dialog.removeEventListener('keydown', key);
    dialog.removeEventListener('close', close);
  };
}
