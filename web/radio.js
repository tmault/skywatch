// Radio panel: per-aircraft message history, text chat, approve/reject actions.

import { liveryFor } from './livery.js';
import { formatTankerWait } from './tanker.js';

const panel = document.getElementById('radio-panel');
const callsignEl = document.getElementById('radio-callsign');
const kindEl = document.getElementById('radio-kind');
const stateEl = document.getElementById('radio-state');
const holdReasonEl = document.getElementById('radio-hold-reason');
const logEl = document.getElementById('radio-log');
const textEl = document.getElementById('radio-text');
const sendBtn = document.getElementById('radio-send');
const inputRowEl = document.getElementById('radio-input-row');
const actionsEl = document.getElementById('radio-actions');
const approveBtn = document.getElementById('radio-approve');
const rejectBtn = document.getElementById('radio-reject');
const closeBtn = document.getElementById('radio-close');
const openWeb = document.getElementById('radio-open-web');
const openDesktop = document.getElementById('radio-open-desktop');
const conversationNote = document.getElementById('radio-conversation-note');
const threadReference = document.getElementById('radio-thread-reference');
const threadId = document.getElementById('radio-thread-id');

const history = new Map();

let selectedId = null;
let onSend = null;
let onApprove = null;
let onClose = null;

export function setupRadio({ send, approve, close }) {
  onSend = send;
  onApprove = approve;
  onClose = close || null;
}

export function selectedAircraftId() {
  return selectedId;
}

function getHistory(id) {
  if (!history.has(id)) history.set(id, []);
  return history.get(id);
}

export function appendHistory(id, who, text) {
  getHistory(id).push({ who, text, at: Date.now() });
  if (id === selectedId) renderLog();
}

export function openRadio(aircraft) {
  if (selectedId !== aircraft.id) threadReference.open = false;
  selectedId = aircraft.id;
  render(aircraft);
  panel.hidden = false;
  document.getElementById('app')?.classList.add('radio-open');
  // Do not steal keyboard focus: arrow keys must keep orbiting the camera.
  // Enter (handled in main.js) or a click focuses the message field.
}

export function focusRadioInput() {
  if (selectedId === null || panel.hidden || inputRowEl.hidden || textEl.disabled) return false;
  textEl.focus();
  return true;
}

export function closeRadio(note) {
  if (selectedId !== null && note) {
    appendHistory(selectedId, 'note', note);
  }
  const wasOpen = selectedId !== null;
  selectedId = null;
  panel.hidden = true;
  document.getElementById('app')?.classList.remove('radio-open');
  if (wasOpen && onClose) onClose();
}

export function updateRadio(aircraft) {
  if (selectedId === null || !aircraft || aircraft.id !== selectedId) return;
  render(aircraft);
}

function render(a) {
  callsignEl.textContent = a.title || a.callsign;
  callsignEl.title = a.title ? a.callsign : '';

  let airframe;
  if (a.kind === 'tanker') airframe = 'AERIAL TANKER';
  else if (a.kind === 'fighter') airframe = 'FIGHTER · SUB-AGENT';
  else if (a.kind === 'drone') airframe = 'REAPER DRONE';
  else if (a.model_class === 'frontier') airframe = 'AGENT · FRONTIER JET';
  else airframe = 'AGENT · LOCAL PROP';

  const parts = [airframe];
  if (a.model) {
    // Generic liveries use the model name as their title; don't repeat it.
    const { title, variant } = liveryFor(a.model);
    const name = a.kind === 'tanker'
      ? (title || variant || '')
      : (title === variant ? title : [title, variant].filter(Boolean).join(' '));
    if (name) parts.push(name.toUpperCase());
  }
  if (a.device) parts.push(a.device.toUpperCase());
  if (a.data_stale) parts.push("OFFLINE · LAST KNOWN STATE");
  if (a.topic) parts.push(a.topic.toUpperCase());
  kindEl.textContent = parts.join(' · ');

  let state = a.state;
  if (a.kind === 'tanker') state = 'refuelling track';
  else if (a.state === 'refueling') {
    if (a.data_stale) state = 'rendezvous suspended';
    else if (!a.refuel_connected) state = 'en route to tanker';
    else if (a.refuel_slot > 2) state = 'in formation';
    else state = 'refuelling';
  } else if (a.state === 'parked') state = 'at gate';
  else if (a.state === 'ready') state = 'taxiway queue';
  else if (a.state === 'departing') state = 'departing (done)';
  stateEl.textContent = state;
  stateEl.style.color = stateColor(a.state);

  if (a.kind === 'tanker') {
    holdReasonEl.textContent = a.hold_reason
      ? `AERIAL REFUEL: ${a.hold_reason} — orbiting above airspace zones`
      : 'AERIAL REFUEL: orbiting above airspace zones';
    holdReasonEl.hidden = false;
  } else if (a.state === 'refueling') {
    const details = formatTankerWait(a);
    if (holdReasonEl.textContent !== details) holdReasonEl.textContent = details;
    holdReasonEl.hidden = false;
  } else if (a.state === 'holding' && a.hold_reason) {
    holdReasonEl.textContent = `HOLD: ${a.hold_reason}`;
    holdReasonEl.hidden = false;
  } else if (a.state === 'ready') {
    holdReasonEl.textContent = a.hold_reason
      ? `TAXIWAY: ${a.hold_reason} — message to clear for takeoff`
      : 'TAXIWAY QUEUE: waiting for takeoff — send a message to depart';
    holdReasonEl.hidden = false;
  } else if (a.state === 'parked') {
    holdReasonEl.textContent = 'AT GATE: untouched >24h — send a message to wake & push back';
    holdReasonEl.hidden = false;
  } else if (a.state === 'departing') {
    holdReasonEl.textContent = 'DONE / ARCHIVED: departing into the horizon — bon voyage';
    holdReasonEl.hidden = false;
  } else {
    holdReasonEl.hidden = true;
  }

  if (inputRowEl) inputRowEl.hidden = a.kind === 'tanker';
  actionsEl.hidden = a.state !== 'holding';
  renderConversation(a);
  renderLog();
}

function setDestination(link, href) {
  if (href && link.getAttribute('href') !== href) link.setAttribute('href', href);
  if (!href && link.hasAttribute('href')) link.removeAttribute('href');
  link.hidden = !href;
}

function renderConversation(aircraft) {
  const links = aircraft.data_stale ? null : aircraft.chat_links;
  setDestination(openWeb, links?.web);
  setDestination(openDesktop, links?.desktop);

  const reference = aircraft.thread_id
    ? [aircraft.environment_id && `Environment ${aircraft.environment_id}`, `Thread ${aircraft.thread_id}`]
        .filter(Boolean).join('\n')
    : '';
  if (threadId.textContent !== reference) threadId.textContent = reference;
  threadReference.hidden = !reference;

  let note;
  if (!reference) note = 'No separate chat is available for this aircraft.';
  else if (aircraft.data_stale) note = 'Owning device is offline. Keep the thread reference to open it later.';
  else if (!links) note = 'This chat cannot be opened from Skywatch. Use the thread reference in T3.';
  else note = 'Web requires access to the owning device. The local harness requires the T3 Code desktop handler on this device.';
  if (conversationNote.textContent !== note) conversationNote.textContent = note;
}

function renderLog() {
  logEl.replaceChildren();
  const entries = getHistory(selectedId);
  if (entries.length === 0) {
    const empty = document.createElement('div');
    empty.className = 'empty';
    empty.textContent = 'no traffic on this frequency';
    logEl.appendChild(empty);
    return;
  }
  for (const e of entries) {
    const line = document.createElement('div');
    const who = document.createElement('span');
    who.className = 'who' + (e.who === 'You' ? ' you' : '');
    who.textContent = `${e.who}: `;
    line.appendChild(who);
    line.appendChild(document.createTextNode(e.text));
    logEl.appendChild(line);
  }
  logEl.scrollTop = logEl.scrollHeight;
}

function submit() {
  const text = textEl.value.trim();
  if (!text || selectedId === null || !onSend) return;
  onSend(selectedId, text);
  appendHistory(selectedId, 'You', text);
  textEl.value = '';
  textEl.focus();
}

sendBtn.addEventListener('click', submit);
textEl.addEventListener('keydown', (ev) => {
  if (ev.key === 'Enter') submit();
  // Escape leaves the field so camera keys work again.
  if (ev.key === 'Escape') textEl.blur();
  ev.stopPropagation();
});

approveBtn.addEventListener('click', () => {
  if (selectedId === null || !onApprove) return;
  appendHistory(selectedId, 'You', 'cleared to continue — approved.');
  onApprove(selectedId);
});

rejectBtn.addEventListener('click', () => {
  closeRadio('request rejected locally.');
});

closeBtn.addEventListener('click', () => closeRadio());

export function stateColor(state) {
  switch (state) {
    case 'holding':
    case 'refueling': return '#ffb020';
    case 'landed': return '#8a8f98';
    case 'parked': return '#7b8794';
    case 'taxi': return '#ffd54a';
    case 'ready': return '#ff8c42';
    case 'takeoff':
    case 'landing': return '#35d6e8';
    case 'departing': return '#4dff9e';
    default: return '#ffffff';
  }
}
