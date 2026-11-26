// Subscription allowance presentation. No provider credentials or thread-driver dependencies.
import * as THREE from './vendor/three.module.js';

export const DEPOT_POSITION = [160, 155];
export const DEPOT_RADIUS = 26;
const unavailable = { status: 'unavailable', source_kind: 'live-transport-unavailable', accounts: [] };
export function windowSummary(w, status, now = Date.now()) {
  const value = w.remaining_percent;
  const known = typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= 100;
  const amount = known ? `${Math.round(value * 10) / 10}% remaining` : 'Unknown';
  const reset = Date.parse(w.resets_at);
  const mins = Math.ceil((reset - now) / 60000);
  const countdown = Number.isFinite(reset) ? (mins <= 0 ? 'refresh pending' : `resets in ${Math.floor(mins / 60)}h ${mins % 60}m`) : 'reset unknown';
  return `${status === 'stale' ? 'Stale · ' : status === 'unavailable' ? 'Unavailable · ' : ''}${amount} · ${countdown}`;
}
export function effectiveAccountStatus(account, now = Date.now()) {
  const checked = Date.parse(account?.checked_at);
  return account?.status !== 'unavailable' && Number.isFinite(checked) && now - checked > 600000
    ? 'stale'
    : (account?.status || 'unknown');
}
export function effectiveSnapshotStatus(snapshot, now = Date.now()) {
  const accounts = snapshot?.accounts || [];
  if (!accounts.length) return snapshot?.status || 'unavailable';
  const statuses = accounts.map(a => effectiveAccountStatus(a, now));
  if (statuses.every(s => s === 'unavailable')) return 'unavailable';
  if (statuses.some(s => s === 'stale')) return 'stale';
  if (statuses.every(s => s === 'available')) return 'available';
  return 'unknown';
}
export function emptyCardLabel(windows, status) {
  if ((windows || []).length > 0) return 'Scoped windows · see details';
  return status === 'unavailable' ? 'Unavailable' : 'No reading';
}
export function visibleWindows(windows) {
  const rank = w => w.duration_minutes === 300 && w.kind !== 'monthly' ? 0 : w.kind === 'weekly' ? 1 : 2;
  return windows.filter(w => !(w.model_ids?.length || w.provider_ids?.length)).sort((a, b) => rank(a) - rank(b)).slice(0, 2);
}
export function disposeGroup(group) {
  const geometries = new Set(), materials = new Set(), textures = new Set();
  group.traverse(o => {
    if (o.geometry) geometries.add(o.geometry);
    for (const m of Array.isArray(o.material) ? o.material : o.material ? [o.material] : []) {
      materials.add(m);
      for (const v of Object.values(m)) if (v?.isTexture) textures.add(v);
    }
  });
  for (const t of textures) t.dispose();
  for (const m of materials) m.dispose();
  for (const g of geometries) g.dispose();
}
export function createFuelDepot(scene, { theme = document.documentElement.dataset.theme || 'alpine', decorate } = {}) {
  const group = new THREE.Group();
  group.name = 'subscription-fuel-depot';
  group.position.set(DEPOT_POSITION[0], 0, DEPOT_POSITION[1]);
  scene.add(group);
  const padMaterial = new THREE.MeshStandardMaterial({ color: 0x9b9b86, roughness: 1 });
  const pad = new THREE.Mesh(new THREE.CylinderGeometry(DEPOT_RADIUS, DEPOT_RADIUS + 2, 4, 32), padMaterial);
  pad.position.y = 0.8; group.add(pad);
  const deck = new THREE.Mesh(new THREE.CylinderGeometry(23, 24, 0.5, 32), new THREE.MeshStandardMaterial({ color: 0x69747a, roughness: 1 }));
  deck.position.y = 3; group.add(deck);
  const tanks = new THREE.Group(); group.add(tanks);
  let dressing, snapshot = unavailable, signature = '', minute = -1;
  const button = document.createElement('button');
  button.id = 'fuel-depot-button'; button.type = 'button'; button.textContent = 'Fuel depot';
  button.setAttribute('aria-controls', 'fuel-depot-panel'); button.setAttribute('aria-expanded', 'false');
  const panel = document.createElement('section');
  panel.id = 'fuel-depot-panel'; panel.hidden = true; panel.tabIndex = 0; panel.setAttribute('aria-label', 'Subscription fuel depot');
  const heading = document.createElement('h2'); heading.textContent = 'Subscription fuel depot';
  const close = document.createElement('button'); close.type = 'button'; close.textContent = 'Close';
  const body = document.createElement('div');
  panel.append(heading, close, body);
  const hudMeta = document.getElementById('hud-right-meta');
  if (hudMeta) hudMeta.append(button); else document.body.append(button);
  document.body.append(panel);
  function toggle(open) { panel.hidden = !open; button.setAttribute('aria-expanded', String(open)); if (open) panel.focus(); else button.focus(); }
  button.onclick = () => toggle(panel.hidden); close.onclick = () => toggle(false);
  panel.onkeydown = e => {
    if (e.key === 'Escape') { e.stopPropagation(); toggle(false); return; }
    if (['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' '].includes(e.key)) e.stopPropagation();
  };
  function label(lines, x, y, z, width = 25) {
    const canvas = document.createElement('canvas'); canvas.width = 768; canvas.height = 26 + lines.length * 46;
    const ctx = canvas.getContext('2d');
    ctx.fillStyle = 'rgba(14,27,37,.9)'; ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.textAlign = 'center'; ctx.fillStyle = '#f5fafc'; ctx.font = 'bold 34px system-ui';
    lines.forEach((s, i) => ctx.fillText(s, 384, 42 + i * 46, 744));
    const texture = new THREE.CanvasTexture(canvas); texture.colorSpace = THREE.SRGBColorSpace;
    const sprite = new THREE.Sprite(new THREE.SpriteMaterial({ map: texture, depthTest: false }));
    sprite.position.set(x, y, z); sprite.scale.set(width, width * canvas.height / canvas.width, 1); tanks.add(sprite);
  }
  function render(now) {
    disposeGroup(tanks); tanks.clear(); body.replaceChildren();
    const aggregateStatus = effectiveSnapshotStatus(snapshot, now);
    const note = document.createElement('p');
    note.textContent = snapshot.source_kind === 'synthetic-demo' ? 'SYNTHETIC DEMO · no live account data' : `Source: ${snapshot.source_kind || 'unavailable'} · ${aggregateStatus}`;
    body.append(note);
    label(['FUEL DEPOT', snapshot.source_kind === 'synthetic-demo' ? 'SYNTHETIC DEMO' : aggregateStatus.toUpperCase()], 0, 31, -17, 32);
    if (!snapshot.accounts?.length) {
      const p = document.createElement('p');
      p.textContent = aggregateStatus === 'unavailable'
        ? 'Live usage unavailable. No authenticated quota source is connected.'
        : 'No authoritative account readings reported.';
      body.append(p);
      label(
        aggregateStatus === 'unavailable'
          ? ['Usage unavailable', 'No authoritative reading']
          : ['No account readings', 'Waiting for authoritative data'],
        0, 10, 2, 33,
      );
    }
    (snapshot.accounts || []).forEach((account, index) => {
      const status = effectiveAccountStatus(account, now);
      const section = document.createElement('article');
      const title = document.createElement('h3'); title.textContent = `${account.provider_label} · ${account.account_label}`;
      const freshness = document.createElement('p'); freshness.textContent = `${status} · source checked: ${account.checked_at || 'unknown'}`;
      section.append(title, freshness);
      const windows = account.windows || [];
      for (const w of windows) {
        const p = document.createElement('p');
        const scope = [...(w.model_ids || []), ...(w.provider_ids || [])];
        p.textContent = `${w.label} · ${windowSummary(w, status, now)}${scope.length ? ` · scope: ${scope.join(', ')}` : ''}${w.resets_at ? ` · reset: ${w.resets_at}` : ''}`;
        section.append(p);
      }
      if (!windows.length) { const p = document.createElement('p'); p.textContent = 'No authoritative allowance windows'; section.append(p); }
      body.append(section);
      // Four account displays fit the island. All accounts/windows stay in the DOM panel.
      if (index >= 4) return;
      const x = index % 2 ? 11 : -11, z = Math.floor(index / 2) * 18 - 6;
      const visible = visibleWindows(windows);
      visible.forEach((w, j) => {
        const tx = x + (j - (visible.length - 1) / 2) * 7;
        const shell = new THREE.Mesh(new THREE.CylinderGeometry(2.8, 2.8, 10, 16), new THREE.MeshStandardMaterial({ color: 0xd4e5e9, transparent: true, opacity: 0.25, depthWrite: false, roughness: 0.65 }));
        shell.position.set(tx, 8.3, z); tanks.add(shell);
        const value = w.remaining_percent;
        if (typeof value === 'number' && Number.isFinite(value) && value > 0 && value <= 100 && status !== 'unavailable') {
          const height = 9.8 * value / 100;
          const fill = new THREE.Mesh(new THREE.CylinderGeometry(2.55, 2.55, 1, 16), new THREE.MeshStandardMaterial({ color: status === 'stale' ? 0x87949b : 0x45b6ad, roughness: 0.45 }));
          fill.scale.y = height; fill.position.set(tx, 3.4 + height / 2, z); tanks.add(fill);
        }
        const ring = new THREE.Mesh(new THREE.CylinderGeometry(3, 3, 0.5, 16), new THREE.MeshStandardMaterial({ color: 0xb4bdc4, roughness: 0.7 }));
        ring.position.set(tx, 3.4, z); tanks.add(ring);
      });
      const lines = [`${account.provider_label} · ${account.account_label}`];
      for (const w of visible) {
        const parts = windowSummary(w, status, now).split(' · ');
        const amount = parts.find(p => p.includes('% remaining') || p === 'Unknown') || 'Unknown';
        lines.push(`${w.label} · ${amount}`, `${status === 'stale' ? 'Stale · ' : ''}${parts.at(-1)}`);
      }
      if (!visible.length) lines.push(emptyCardLabel(windows, status));
      label(lines, x, 20, z, 20);
    });
    if ((snapshot.accounts?.length || 0) > 4) label(['More accounts in depot details'], 0, 5, 22, 30);
  }
  function setTheme(next) {
    theme = next;
    padMaterial.color.set(theme === 'winter' ? 0xe4eaf0 : theme === 'tropical' ? 0xd4c291 : 0x969c80);
    if (dressing) { group.remove(dressing); disposeGroup(dressing); }
    dressing = decorate?.({ theme, THREE, radius: DEPOT_RADIUS });
    if (dressing) group.add(dressing);
  }
  const themeListener = e => setTheme(e.detail?.theme || 'alpine');
  window.addEventListener('skywatch-theme-change', themeListener); setTheme(theme);
  function update(next = snapshot, now = Date.now()) {
    snapshot = next || unavailable;
    const nextSignature = JSON.stringify({
      status: snapshot.status,
      source_kind: snapshot.source_kind,
      accounts: snapshot.accounts,
    });
    const nextMinute = Math.floor(now / 60000);
    if (nextSignature !== signature || nextMinute !== minute) {
      signature = nextSignature; minute = nextMinute; render(now);
    }
  }
  update();
  const timer = setInterval(() => update(), 1000);
  return { group, update, setTheme, dispose() {
    clearInterval(timer); window.removeEventListener('skywatch-theme-change', themeListener);
    button.remove(); panel.remove(); scene.remove(group); disposeGroup(group);
  } };
}
