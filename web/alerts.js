// Alert management: urgency-split "Needs you" stack, informational refuelling
// strip, muted fading routine ticker with duplicate collapse, header badge,
// live wait/age timers (updated in place without re-rendering), and click/hover
// hooks into the 3D scene.

import { formatTankerWait } from './tanker.js';

const ROUTINE_TTL_MS = 8000;
const ROUTINE_FADE_MS = 800;
const MAX_ROUTINE_LINES = 4;
const BURST_COLLAPSE_WINDOW_MS = 5000;

const PASSIVE_HOLD_RE = /^(?:waiting on (?:sub-agent|background tool|background command|monitor|delegated thread)|watching pull request checks)\b/i;
const FAILED_REASON_RE = /\bfail(?:ed|ure)?\b/i;
const APPROVAL_REASON_RE = /\bapprov(?:al|e)\b/i;
const SIGNIN_REASON_RE = /\bsign[- ]?in\b|\bauth/i;
const INPUT_REASON_RE = /\binput\b|\bquestion\b|\btool call\b|\bvector\b/i;

const BULK_COLLAPSE_KEYS = new Set([
  'queued', 'parked', 'taxi', 'wingman', 'cruising',
]);

const URGENCY_PRIORITY = {
  failed: 0,
  approval: 1,
  signin: 2,
  input: 3,
  offline: 4,
};

const ICON_SVG = {
  approval: '<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M8 1.5 14 4v4.2c0 3.2-2.5 5.6-6 6.8-3.5-1.2-6-3.6-6-6.8V4l6-2.5Z"/><path d="M8 5.2v3.3"/><circle cx="8" cy="11.2" r="0.6" fill="currentColor" stroke="none"/></svg>',
  input: '<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2.5 3.5h11v7.5H6.5L3.5 13.5V11h-1V3.5Z"/><path d="M6.4 6.5a1.6 1.6 0 1 1 2.4 1.4c-.5.3-.8.6-.8 1.1"/><circle cx="8" cy="10.2" r="0.5" fill="currentColor" stroke="none"/></svg>',
  signin: '<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="5.5" cy="8" r="2.8"/><path d="M8.3 8H14M11.8 8v2.2M14 8v1.8"/></svg>',
  failed: '<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="8" cy="8" r="6"/><path d="m5.7 5.7 4.6 4.6M10.3 5.7l-4.6 4.6"/></svg>',
  offline: '<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2 2l12 12"/><path d="M5.2 5.2A7 7 0 0 1 13.5 7"/><path d="M2.5 7c.6-.6 1.2-1 1.9-1.4"/><path d="M5.5 9.8a3.8 3.8 0 0 1 2.5-.9"/><circle cx="8" cy="12.6" r="0.9" fill="currentColor" stroke="none"/></svg>',
  refuel: '<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M8 2.2C6 5.2 4 7.4 4 9.7a4 4 0 0 0 8 0C12 7.4 10 5.2 8 2.2Z"/></svg>',
  takeoff: '<svg viewBox="0 0 16 16" width="13" height="13" fill="currentColor" aria-hidden="true"><path d="M8 3.2 13 11.8H3L8 3.2Z"/></svg>',
  landing: '<svg viewBox="0 0 16 16" width="13" height="13" fill="currentColor" aria-hidden="true"><path d="M8 12.8 13 4.2H3L8 12.8Z"/></svg>',
  routine: '<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><circle cx="8" cy="8" r="3.2"/></svg>',
};

function cleanTitle(raw) {
  return (raw || '').replace(/\*\*/g, '').replace(/\s+/g, ' ').trim();
}

export function formatAge(ms, isHold = false) {
  const secs = Math.max(0, Math.floor(ms / 1000));
  if (isHold) {
    if (secs < 5) return 'waiting now';
    if (secs < 60) return `waiting ${secs}s`;
    const mins = Math.floor(secs / 60);
    if (mins < 60) return `waiting ${mins}m`;
    const hrs = Math.floor(mins / 60);
    const rem = mins % 60;
    return rem ? `waiting ${hrs}h ${rem}m` : `waiting ${hrs}h`;
  }
  if (secs < 60) return 'now';
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m`;
  const hrs = Math.floor(mins / 60);
  const rem = mins % 60;
  return rem ? `${hrs}h ${rem}m` : `${hrs}h`;
}

/**
 * Classify whether an aircraft currently requires user attention ("needs you").
 * Returns null for routine, cruising, passive agent waits, and provider refuelling.
 */
export function classifyAircraftUrgency(a) {
  if (!a || a.kind === 'tanker') return null;
  if (a.state === 'refueling') return null;

  const reason = (a.hold_reason || '').trim();

  if (a.state === 'holding') {
    if (reason && PASSIVE_HOLD_RE.test(reason)) return null;
    if (APPROVAL_REASON_RE.test(reason)) {
      return {
        kind: 'approval',
        label: 'APPROVAL',
        severity: 'warn',
        isHold: true,
        reason: reason || 'waiting on approval',
      };
    }
    if (SIGNIN_REASON_RE.test(reason)) {
      return {
        kind: 'signin',
        label: 'SIGN-IN',
        severity: 'warn',
        isHold: true,
        reason: reason || 'waiting on sign-in',
      };
    }
    if (INPUT_REASON_RE.test(reason)) {
      return {
        kind: 'input',
        label: 'INPUT',
        severity: 'warn',
        isHold: true,
        reason: reason || 'awaiting your input',
      };
    }
    return {
      kind: 'input',
      label: 'INPUT',
      severity: 'warn',
      isHold: true,
      reason: reason || 'awaiting your input',
    };
  }

  if (a.state === 'ready' && FAILED_REASON_RE.test(reason)) {
    return {
      kind: 'failed',
      label: 'FAILED',
      severity: 'danger',
      isHold: false,
      reason,
    };
  }

  return null;
}

export function isPassiveHold(a) {
  return Boolean(a && a.state === 'holding' && a.hold_reason && PASSIVE_HOLD_RE.test(a.hold_reason.trim()));
}

export function createAlertManager({
  onSelectAircraft,
  onFocusAirport,
  onHoverAircraft,
  onHoverAirport,
}) {
  const stackEl = document.getElementById('attention-stack');
  const stackListEl = document.getElementById('attention-list');
  const stackCountEl = document.getElementById('attention-count');
  const stackToggleEl = document.getElementById('attention-toggle');
  const badgeBtnEl = document.getElementById('hud-urgent');
  const badgeTextEl = document.getElementById('hud-urgent-text');
  const refuelEl = document.getElementById('ticker-refuel');
  const routineEl = document.getElementById('ticker-lines');

  const urgentRecords = new Map(); // key -> { item, since, el, badgeEl, ageEl, callsignEl, titleEl, reasonEl }
  const refuelRecords = new Map(); // aircraftId -> { aircraft, since, el, ageEl, callsignEl, titleEl, reasonEl }
  const routineRecords = [];       // array of { key, eventKey, aircraftId, callsign, title, callsigns, reason, kind, count, createdAt, updatedAt, expiresAt, hovered, el, primaryEl, reasonEl, ageEl }

  let orderedUrgentItems = [];
  let collapsed = false;
  let lastJumpedKey = null;
  let pulseTimer = null;

  function setCollapsed(next) {
    collapsed = next;
    if (stackEl) stackEl.classList.toggle('collapsed', collapsed);
    if (stackToggleEl) {
      stackToggleEl.setAttribute('aria-expanded', String(!collapsed));
      stackToggleEl.textContent = collapsed ? '+' : '−';
      stackToggleEl.title = collapsed ? 'Expand needs-you stack' : 'Collapse needs-you stack';
    }
  }

  if (stackToggleEl) {
    stackToggleEl.addEventListener('click', () => setCollapsed(!collapsed));
  }

  if (badgeBtnEl) {
    badgeBtnEl.addEventListener('click', () => {
      if (orderedUrgentItems.length === 0) return;
      setCollapsed(false);
      if (stackEl) {
        stackEl.classList.remove('focus-pulse');
        void stackEl.offsetWidth;
        stackEl.classList.add('focus-pulse');
        clearTimeout(pulseTimer);
        pulseTimer = setTimeout(() => stackEl.classList.remove('focus-pulse'), 900);
      }
      const firstCard = stackListEl?.querySelector('.alert-card');
      if (firstCard) firstCard.focus({ preventScroll: false });
    });
  }

  function bindInteractive(el, getTarget) {
    el.addEventListener('click', () => {
      const target = getTarget();
      if (!target) return;
      if (target.aircraftId != null) {
        onSelectAircraft?.(target.aircraftId);
      } else if (target.airportId != null) {
        onFocusAirport?.(target.airportId);
      }
    });
    const enter = () => {
      const target = getTarget();
      if (!target) return;
      if (target.aircraftId != null) onHoverAircraft?.(target.aircraftId);
      else if (target.airportId != null) onHoverAirport?.(target.airportId);
    };
    const leave = () => {
      const target = getTarget();
      if (!target) return;
      if (target.aircraftId != null) onHoverAircraft?.(null, target.aircraftId);
      else if (target.airportId != null) onHoverAirport?.(null, target.airportId);
    };
    el.addEventListener('pointerenter', enter);
    el.addEventListener('pointerleave', leave);
    el.addEventListener('focus', enter);
    el.addEventListener('blur', leave);
  }

  function createUrgentCard(item, since) {
    const el = document.createElement('button');
    el.type = 'button';
    el.className = `alert-card severity-${item.severity} kind-${item.kind}`;
    el.dataset.alertKey = item.key;

    const head = document.createElement('div');
    head.className = 'alert-head';

    const badgeEl = document.createElement('span');
    badgeEl.className = `alert-badge severity-${item.severity}`;
    badgeEl.innerHTML = `${ICON_SVG[item.kind] || ICON_SVG.input}<span>${item.label}</span>`;

    const ageEl = document.createElement('span');
    ageEl.className = 'alert-age';
    ageEl.textContent = formatAge(Date.now() - since, item.isHold);

    head.append(badgeEl, ageEl);

    const primaryEl = document.createElement('div');
    primaryEl.className = 'alert-primary';
    const callsignEl = document.createElement('span');
    callsignEl.className = 'alert-callsign';
    callsignEl.textContent = item.callsign;
    const titleEl = document.createElement('span');
    titleEl.className = 'alert-title';
    titleEl.textContent = item.title;
    primaryEl.append(callsignEl);
    if (item.title) primaryEl.append(' ', titleEl);

    const reasonEl = document.createElement('div');
    reasonEl.className = 'alert-reason';
    reasonEl.textContent = item.reason;

    el.append(head, primaryEl, reasonEl);

    const rec = {
      item,
      since,
      el,
      badgeEl,
      ageEl,
      primaryEl,
      callsignEl,
      titleEl,
      reasonEl,
    };

    bindInteractive(el, () => ({
      aircraftId: rec.item.aircraftId,
      airportId: rec.item.airportId,
    }));

    return rec;
  }

  function updateUrgentCard(rec, nextItem, isSelected) {
    const prev = rec.item;
    if (prev.kind !== nextItem.kind || prev.severity !== nextItem.severity) {
      rec.since = Date.now();
      rec.el.className = `alert-card severity-${nextItem.severity} kind-${nextItem.kind}`;
      rec.badgeEl.className = `alert-badge severity-${nextItem.severity}`;
      rec.badgeEl.innerHTML = `${ICON_SVG[nextItem.kind] || ICON_SVG.input}<span>${nextItem.label}</span>`;
    }
    if (prev.callsign !== nextItem.callsign || prev.title !== nextItem.title) {
      rec.callsignEl.textContent = nextItem.callsign;
      rec.titleEl.textContent = nextItem.title;
      rec.primaryEl.replaceChildren(rec.callsignEl);
      if (nextItem.title) rec.primaryEl.append(' ', rec.titleEl);
    }
    if (prev.reason !== nextItem.reason) {
      rec.reasonEl.textContent = nextItem.reason;
    }
    rec.item = nextItem;
    rec.el.classList.toggle('active', isSelected);
  }

  function createRefuelCard(a, since, now) {
    const el = document.createElement('button');
    el.type = 'button';
    el.className = 'tick-line refuel-card';
    el.dataset.aircraftId = String(a.id);

    const primaryEl = document.createElement('div');
    primaryEl.className = 'tick-primary';

    const primaryMainEl = document.createElement('span');
    primaryMainEl.className = 'tick-primary-main';

    const tagEl = document.createElement('span');
    tagEl.className = 'tick-tag refuel';
    tagEl.innerHTML = `${ICON_SVG.refuel}<span>REFUELLING</span>`;

    const callsignEl = document.createElement('span');
    callsignEl.className = 'tick-callsign';
    callsignEl.textContent = a.callsign;

    const titleEl = document.createElement('span');
    titleEl.className = 'tick-title';
    titleEl.textContent = cleanTitle(a.title);

    const ageEl = document.createElement('span');
    ageEl.className = 'tick-age';
    ageEl.textContent = formatAge(now - since, false);

    primaryMainEl.append(tagEl, callsignEl, titleEl);
    primaryEl.append(primaryMainEl, ageEl);

    const reasonEl = document.createElement('div');
    reasonEl.className = 'tick-reason';
    reasonEl.textContent = formatTankerWait(a, now);

    el.append(primaryEl, reasonEl);

    const rec = {
      aircraft: a,
      since,
      el,
      callsignEl,
      titleEl,
      ageEl,
      reasonEl,
    };

    bindInteractive(el, () => ({ aircraftId: rec.aircraft.id, airportId: null }));
    return rec;
  }

  function syncSnapshot(snapshot, activeAircraftId = null) {
    const now = Date.now();
    const aircraft = snapshot.aircraft || [];
    const airports = snapshot.airports || [];

    // 1. Build current urgent ("needs you") items.
    const nextUrgent = [];
    const coveredStaleIds = new Set();

    for (const a of aircraft) {
      const cls = classifyAircraftUrgency(a);
      if (!cls) continue;
      nextUrgent.push({
        key: `aircraft:${a.id}`,
        aircraftId: a.id,
        airportId: null,
        kind: cls.kind,
        label: cls.label,
        severity: cls.severity,
        isHold: cls.isHold,
        callsign: a.callsign,
        title: cleanTitle(a.title),
        reason: cls.reason,
      });
    }

    for (const ap of airports) {
      if (ap.online !== false) continue;
      const stalePlanes = aircraft.filter(
        (a) => a.data_stale && (a.environment_id === ap.id || a.device === ap.name),
      );
      for (const p of stalePlanes) coveredStaleIds.add(p.id);
      const primaryPlane = stalePlanes[0] || null;
      const reason = stalePlanes.length > 0
        ? `${stalePlanes.length} aircraft showing last known state`
        : 'Airport unreachable · offline';
      nextUrgent.push({
        key: `airport:${ap.id}`,
        aircraftId: primaryPlane ? primaryPlane.id : null,
        airportId: ap.id,
        kind: 'offline',
        label: 'OFFLINE',
        severity: 'danger',
        isHold: false,
        callsign: (ap.name || ap.id).toUpperCase(),
        title: 'Device offline',
        reason,
      });
    }

    for (const a of aircraft) {
      if (!a.data_stale || coveredStaleIds.has(a.id)) continue;
      nextUrgent.push({
        key: `stale:${a.id}`,
        aircraftId: a.id,
        airportId: null,
        kind: 'offline',
        label: 'OFFLINE',
        severity: 'danger',
        isHold: false,
        callsign: a.callsign,
        title: cleanTitle(a.title) || (a.device ? a.device.toUpperCase() : 'Device offline'),
        reason: 'Device offline · last known state',
      });
    }

    nextUrgent.sort((a, b) => {
      const pa = URGENCY_PRIORITY[a.kind] ?? 99;
      const pb = URGENCY_PRIORITY[b.kind] ?? 99;
      if (pa !== pb) return pa - pb;
      const sa = urgentRecords.get(a.key)?.since ?? now;
      const sb = urgentRecords.get(b.key)?.since ?? now;
      if (sa !== sb) return sa - sb;
      return a.key.localeCompare(b.key);
    });

    orderedUrgentItems = nextUrgent;

    // Diff urgent DOM nodes in place (never rebuild unchanged cards).
    const nextKeys = new Set(nextUrgent.map((u) => u.key));
    for (const [key, rec] of [...urgentRecords.entries()]) {
      if (!nextKeys.has(key)) {
        if (rec.item.aircraftId != null) onHoverAircraft?.(null, rec.item.aircraftId);
        if (rec.item.airportId != null) onHoverAirport?.(null, rec.item.airportId);
        rec.el.remove();
        urgentRecords.delete(key);
      }
    }

    if (stackListEl) {
      for (let i = 0; i < nextUrgent.length; i++) {
        const item = nextUrgent[i];
        const isSelected = activeAircraftId != null && item.aircraftId === activeAircraftId;
        let rec = urgentRecords.get(item.key);
        if (!rec) {
          rec = createUrgentCard(item, now);
          rec.el.classList.toggle('active', isSelected);
          urgentRecords.set(item.key, rec);
        } else {
          updateUrgentCard(rec, item, isSelected);
        }
        const currentAtIdx = stackListEl.children[i];
        if (currentAtIdx !== rec.el) {
          stackListEl.insertBefore(rec.el, currentAtIdx || null);
        }
      }
    }

    // Update urgent stack container & header badge.
    const count = nextUrgent.length;
    if (stackEl) stackEl.hidden = count === 0;
    if (stackCountEl) stackCountEl.textContent = String(count);
    if (badgeBtnEl && badgeTextEl) {
      badgeBtnEl.hidden = count === 0;
      const text = count === 1 ? '1 needs you' : `${count} need you`;
      if (badgeTextEl.textContent !== text) badgeTextEl.textContent = text;
    }

    // 2. Sync informational refuelling strip (provider waits stay visible, not "needs you").
    const refuelling = aircraft.filter((a) => a.state === 'refueling' && a.kind !== 'tanker');
    const refuelIds = new Set(refuelling.map((a) => a.id));

    for (const [id, rec] of [...refuelRecords.entries()]) {
      if (!refuelIds.has(id)) {
        onHoverAircraft?.(null, id);
        rec.el.remove();
        refuelRecords.delete(id);
      }
    }

    if (refuelEl) {
      for (let i = 0; i < refuelling.length; i++) {
        const a = refuelling[i];
        const isSelected = activeAircraftId != null && a.id === activeAircraftId;
        let rec = refuelRecords.get(a.id);
        if (!rec) {
          rec = createRefuelCard(a, now, now);
          refuelRecords.set(a.id, rec);
        } else {
          rec.aircraft = a;
          const nextTitle = cleanTitle(a.title);
          if (rec.callsignEl.textContent !== a.callsign) rec.callsignEl.textContent = a.callsign;
          if (rec.titleEl.textContent !== nextTitle) rec.titleEl.textContent = nextTitle;
          const details = formatTankerWait(a, now);
          if (rec.reasonEl.textContent !== details) rec.reasonEl.textContent = details;
        }
        rec.el.classList.toggle('active', isSelected);
        const currentAtIdx = refuelEl.children[i];
        if (currentAtIdx !== rec.el) {
          refuelEl.insertBefore(rec.el, currentAtIdx || null);
        }
      }
      refuelEl.hidden = refuelling.length === 0;
    }
  }

  function renderRoutineContent(rec) {
    const iconHtml = ICON_SVG[rec.kind] || ICON_SVG.routine;
    const countHtml = rec.count > 1 ? `<span class="tick-repeat">×${rec.count}</span>` : '';
    const titleStr = rec.callsigns.length > 1
      ? ''
      : (rec.title ? ` <span class="tick-title">${escapeHtml(rec.title)}</span>` : '');
    rec.primaryMainEl.innerHTML =
      `<span class="tick-icon kind-${rec.kind}">${iconHtml}</span>`
      + `<span class="tick-callsign">${escapeHtml(rec.callsign)}</span>`
      + countHtml
      + titleStr;

    const reasonText = rec.callsigns.length > 1
      ? `${rec.callsigns.join(', ')} · ${rec.reason}`
      : rec.reason;
    if (rec.reasonEl.textContent !== reasonText) rec.reasonEl.textContent = reasonText;
    rec.ageEl.textContent = formatAge(Date.now() - rec.updatedAt, false);
  }

  function addRoutineEvent({ aircraftId = null, callsign = '', title = '', eventKey = '', reason = '', kind = 'routine' }) {
    if (!routineEl) return;
    const now = Date.now();
    const cleanedTitle = cleanTitle(title);

    // Collapse duplicate events (same aircraft+event, or burst of bulk routine events).
    const match = routineRecords.find((r) => {
      if (aircraftId != null && r.aircraftId === aircraftId && r.eventKey === eventKey) return true;
      if (eventKey && BULK_COLLAPSE_KEYS.has(eventKey) && r.eventKey === eventKey
          && (now - r.updatedAt) <= BURST_COLLAPSE_WINDOW_MS) {
        return true;
      }
      return false;
    });

    if (match) {
      match.count += 1;
      match.aircraftId = aircraftId ?? match.aircraftId;
      if (callsign) {
        match.callsign = callsign;
        if (!match.callsigns.includes(callsign)) match.callsigns.push(callsign);
      }
      if (cleanedTitle) match.title = cleanedTitle;
      if (reason) match.reason = reason;
      match.updatedAt = now;
      match.expiresAt = now + ROUTINE_TTL_MS;
      match.el.classList.remove('fading');
      renderRoutineContent(match);
      return;
    }

    const el = document.createElement(aircraftId != null ? 'button' : 'div');
    if (el.tagName === 'BUTTON') el.type = 'button';
    el.className = `tick-line routine-line kind-${kind}`;

    const primaryEl = document.createElement('div');
    primaryEl.className = 'tick-primary';
    const primaryMainEl = document.createElement('span');
    primaryMainEl.className = 'tick-primary-main';
    const ageEl = document.createElement('span');
    ageEl.className = 'tick-age';
    primaryEl.append(primaryMainEl, ageEl);

    const reasonEl = document.createElement('div');
    reasonEl.className = 'tick-reason';

    el.append(primaryEl, reasonEl);

    const rec = {
      eventKey,
      aircraftId,
      callsign,
      title: cleanedTitle,
      callsigns: callsign ? [callsign] : [],
      reason,
      kind,
      count: 1,
      createdAt: now,
      updatedAt: now,
      expiresAt: now + ROUTINE_TTL_MS,
      hovered: false,
      el,
      primaryMainEl,
      reasonEl,
      ageEl,
    };

    renderRoutineContent(rec);

    el.addEventListener('pointerenter', () => {
      rec.hovered = true;
      rec.el.classList.remove('fading');
    });
    el.addEventListener('pointerleave', () => {
      rec.hovered = false;
      rec.expiresAt = Math.max(rec.expiresAt, Date.now() + 2500);
    });
    el.addEventListener('focus', () => {
      rec.hovered = true;
      rec.el.classList.remove('fading');
    });
    el.addEventListener('blur', () => {
      rec.hovered = false;
      rec.expiresAt = Math.max(rec.expiresAt, Date.now() + 2500);
    });

    if (aircraftId != null) {
      bindInteractive(el, () => ({ aircraftId: rec.aircraftId, airportId: null }));
    }

    routineRecords.push(rec);
    routineEl.appendChild(el);

    while (routineRecords.length > MAX_ROUTINE_LINES) {
      const oldest = routineRecords.shift();
      if (oldest.aircraftId != null) onHoverAircraft?.(null, oldest.aircraftId);
      oldest.el.remove();
    }
  }

  function jumpToNextAttention(currentFollowId = null) {
    const aircraftItems = orderedUrgentItems.filter((item) => item.aircraftId != null);
    const pool = aircraftItems.length > 0 ? aircraftItems : orderedUrgentItems;
    if (pool.length === 0) return false;

    let idx = -1;
    if (currentFollowId != null) {
      idx = pool.findIndex((item) => item.aircraftId === currentFollowId);
    }
    if (idx === -1 && lastJumpedKey != null) {
      idx = pool.findIndex((item) => item.key === lastJumpedKey);
    }
    const next = pool[(idx + 1) % pool.length];
    lastJumpedKey = next.key;

    setCollapsed(false);
    const rec = urgentRecords.get(next.key);
    if (rec) {
      rec.el.scrollIntoView({ block: 'nearest' });
    }

    if (next.aircraftId != null) {
      onSelectAircraft?.(next.aircraftId);
      return true;
    }
    if (next.airportId != null) {
      onFocusAirport?.(next.airportId);
      return true;
    }
    return false;
  }

  function tickTimers() {
    const now = Date.now();

    // Update urgent alert ages in place (zero stack re-rendering).
    for (const rec of urgentRecords.values()) {
      const nextAge = formatAge(now - rec.since, rec.item.isHold);
      if (rec.ageEl.textContent !== nextAge) rec.ageEl.textContent = nextAge;
    }

    // Update refuelling ages + countdowns in place.
    for (const rec of refuelRecords.values()) {
      const nextAge = formatAge(now - rec.since, false);
      if (rec.ageEl.textContent !== nextAge) rec.ageEl.textContent = nextAge;
      const details = formatTankerWait(rec.aircraft, now);
      if (rec.reasonEl.textContent !== details) rec.reasonEl.textContent = details;
    }

    // Update routine ticker ages and fade/expire routine lines after ~8s.
    for (let i = routineRecords.length - 1; i >= 0; i--) {
      const rec = routineRecords[i];
      const nextAge = formatAge(now - rec.updatedAt, false);
      if (rec.ageEl.textContent !== nextAge) rec.ageEl.textContent = nextAge;
      if (rec.hovered) continue;
      const remaining = rec.expiresAt - now;
      if (remaining <= 0) {
        if (rec.aircraftId != null) onHoverAircraft?.(null, rec.aircraftId);
        rec.el.remove();
        routineRecords.splice(i, 1);
      } else if (remaining <= ROUTINE_FADE_MS) {
        rec.el.classList.add('fading');
      }
    }
  }

  const timerId = setInterval(tickTimers, 500);

  return {
    syncSnapshot,
    addRoutineEvent,
    jumpToNextAttention,
    getUrgentCount: () => orderedUrgentItems.length,
    destroy: () => clearInterval(timerId),
  };
}

function escapeHtml(str) {
  return String(str)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}
