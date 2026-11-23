// Radio panel: per-aircraft message history, text chat, approve/reject actions.

const panel = document.getElementById('radio-panel');
const callsignEl = document.getElementById('radio-callsign');
const kindEl = document.getElementById('radio-kind');
const stateEl = document.getElementById('radio-state');
const holdReasonEl = document.getElementById('radio-hold-reason');
const logEl = document.getElementById('radio-log');
const textEl = document.getElementById('radio-text');
const sendBtn = document.getElementById('radio-send');
const actionsEl = document.getElementById('radio-actions');
const approveBtn = document.getElementById('radio-approve');
const rejectBtn = document.getElementById('radio-reject');
const closeBtn = document.getElementById('radio-close');

// Client-side radio history keyed by aircraft id. Persisted across panel
// open/close but not across page reloads.
const history = new Map();

let selectedId = null;
let selectedState = null;
let onSend = null;   // (aircraftId, text) => void
let onApprove = null; // (aircraftId) => void

export function setupRadio({ send, approve }) {
  onSend = send;
  onApprove = approve;
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
  selectedId = aircraft.id;
  render(aircraft);
  panel.hidden = false;
  textEl.focus();
}

export function closeRadio(note) {
  if (selectedId !== null && note) {
    appendHistory(selectedId, 'note', note);
  }
  selectedId = null;
  panel.hidden = true;
}

// Called from the main loop with the latest snapshot entry so the panel
// stays in sync with the live world state.
export function updateRadio(aircraft) {
  if (selectedId === null || !aircraft || aircraft.id !== selectedId) return;
  render(aircraft);
}

function render(a) {
  selectedState = a.state;
  callsignEl.textContent = a.callsign;
  kindEl.textContent =
    (a.kind === 'drone' ? 'DRONE' : a.model_class === 'frontier' ? 'AGENT · FRONTIER JET' : 'AGENT · LOCAL PROP')
    + (a.topic ? ` · ${a.topic.toUpperCase()}` : '');
  stateEl.textContent = a.state;
  stateEl.style.color = stateColor(a.state);

  if (a.state === 'holding' && a.hold_reason) {
    holdReasonEl.textContent = `HOLD: ${a.hold_reason}`;
    holdReasonEl.hidden = false;
  } else {
    holdReasonEl.hidden = true;
  }

  actionsEl.hidden = a.state !== 'holding';
  renderLog();
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
  ev.stopPropagation(); // don't let typing drive orbit controls
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
    case 'holding': return '#ffb020';
    case 'landed': return '#8a8f98';
    case 'takeoff':
    case 'landing': return '#35d6e8';
    default: return '#ffffff';
  }
}
