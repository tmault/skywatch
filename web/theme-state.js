export const THEMES = Object.freeze(['alpine', 'tropical', 'winter']);
export const THEME_STORAGE_KEY = 'skywatch.theme';
export function resolveTheme(server, local) {
  return THEMES.includes(local) ? local : THEMES.includes(server) ? server : 'alpine';
}
export function createThemeState(server, storage) {
  let local;
  try { local = storage?.getItem(THEME_STORAGE_KEY); } catch { /* private browsing */ }
  let theme = resolveTheme(server, local);
  const listeners = new Set();
  return {
    get theme() { return theme; },
    select(next) {
      if (!THEMES.includes(next)) return;
      try { storage?.setItem(THEME_STORAGE_KEY, next); } catch { /* session still works */ }
      if (next === theme) return;
      theme = next;
      for (const listener of listeners) listener(theme);
    },
    subscribe(listener) {
      listeners.add(listener); listener(theme);
      return () => listeners.delete(listener);
    },
  };
}
