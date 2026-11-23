// Skywatch browser client: Three.js scene, WS snapshot/delta consumption,
// smooth aircraft interpolation, picking, radio panel, ambient ticker.

import * as THREE from './vendor/three.module.js';
import { OrbitControls } from './vendor/OrbitControls.js';
import {
  setupRadio, openRadio, closeRadio, updateRadio, selectedAircraftId,
  appendHistory, stateColor,
} from './radio.js';

const WORLD_SIZE = 1000;
const INTERP_RATE = 10; // exponential smoothing constant (per second)

/* ---------------------------------------------------------------- scene */

const canvas = document.getElementById('scene');
const renderer = new THREE.WebGLRenderer({ canvas, antialias: true });
renderer.setSize(window.innerWidth, window.innerHeight);
renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));

const scene = new THREE.Scene();
// Sky gradient background.
{
  const skyCanvas = document.createElement('canvas');
  skyCanvas.width = 1;
  skyCanvas.height = 256;
  const ctx = skyCanvas.getContext('2d');
  const grad = ctx.createLinearGradient(0, 0, 0, 256);
  grad.addColorStop(0, '#0a1430');
  grad.addColorStop(0.6, '#123a5c');
  grad.addColorStop(1, '#2a6a86');
  ctx.fillStyle = grad;
  ctx.fillRect(0, 0, 1, 256);
  const tex = new THREE.CanvasTexture(skyCanvas);
  tex.colorSpace = THREE.SRGBColorSpace;
  scene.background = tex;
}
scene.fog = new THREE.Fog(0x123a5c, 400, 1400);

const camera = new THREE.PerspectiveCamera(
  55, window.innerWidth / window.innerHeight, 1, 3000,
);
camera.position.set(0, 480, 260);
camera.lookAt(0, 0, 0);

const controls = new OrbitControls(camera, canvas);
controls.target.set(0, 60, 0);
controls.enableDamping = true;
controls.dampingFactor = 0.08;
controls.maxPolarAngle = Math.PI / 2 - 0.05;
controls.minDistance = 40;
controls.maxDistance = 900;

scene.add(new THREE.AmbientLight(0x8899bb, 0.7));
const sun = new THREE.DirectionalLight(0xffffff, 1.4);
sun.position.set(300, 500, 200);
scene.add(sun);

// Ground plane + grid.
{
  const ground = new THREE.Mesh(
    new THREE.PlaneGeometry(WORLD_SIZE, WORLD_SIZE),
    new THREE.MeshLambertMaterial({ color: 0x0d2b22 }),
  );
  ground.rotation.x = -Math.PI / 2;
  ground.position.y = 0;
  scene.add(ground);

  const grid = new THREE.GridHelper(WORLD_SIZE, 40, 0x2f7f66, 0x1a4a3a);
  grid.position.y = 0.5;
  scene.add(grid);
}

/* ------------------------------------------------------ aircraft meshes */

// Group per aircraft: mesh + (optionally) hold ring. Meshes are created on
// first sight and driven by snapshots with exponential smoothing.
const meshes = new Map();

function makeAgentMesh() {
  const group = new THREE.Group();
  const mat = new THREE.MeshLambertMaterial({ color: 0xffffff });

  const fuselage = new THREE.Mesh(new THREE.ConeGeometry(2.2, 12, 8), mat);
  fuselage.rotation.x = Math.PI / 2; // point along +Z (heading axis)
  group.add(fuselage);

  const wings = new THREE.Mesh(new THREE.BoxGeometry(16, 0.6, 3), mat);
  wings.position.z = -1;
  group.add(wings);

  const tail = new THREE.Mesh(new THREE.BoxGeometry(6, 0.5, 2.4), mat);
  tail.position.set(0, 1.6, -5);
  group.add(tail);

  const fin = new THREE.Mesh(new THREE.BoxGeometry(0.5, 3, 2.4), mat);
  fin.position.set(0, 1.8, -5);
  group.add(fin);

  return { group, materials: [mat] };
}

function makeDroneMesh() {
  const group = new THREE.Group();
  const mat = new THREE.MeshLambertMaterial({ color: 0xffffff });
  const rotorMat = new THREE.MeshLambertMaterial({
    color: 0x222831, transparent: true, opacity: 0.6, side: THREE.DoubleSide,
  });

  const body = new THREE.Mesh(new THREE.BoxGeometry(4, 1.6, 4), mat);
  group.add(body);

  const arms = new THREE.Mesh(new THREE.BoxGeometry(10, 0.5, 0.8), mat);
  arms.rotation.y = Math.PI / 4;
  group.add(arms);

  const arms2 = arms.clone();
  arms2.rotation.y = -Math.PI / 4;
  group.add(arms2);

  const rotorGeo = new THREE.CircleGeometry(2.6, 16);
  rotorGeo.rotateX(-Math.PI / 2);
  for (const [x, z] of [[4, 4], [-4, 4], [4, -4], [-4, -4]]) {
    const rotor = new THREE.Mesh(rotorGeo, rotorMat);
    rotor.position.set(x, 1, z);
    group.add(rotor);
  }

  return { group, materials: [mat, rotorMat] };
}

// Canvas-sprite callsign label that stays screen-facing and constant-ish size.
function makeLabel(callsign) {
  const canvas = document.createElement('canvas');
  canvas.width = 256;
  canvas.height = 64;
  const ctx = canvas.getContext('2d');
  ctx.fillStyle = 'rgba(0, 20, 20, 0.55)';
  ctx.fillRect(0, 0, 256, 64);
  ctx.font = 'bold 34px monospace';
  ctx.fillStyle = '#e8ffff';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText(callsign, 128, 34);
  const sprite = new THREE.Sprite(new THREE.SpriteMaterial({
    map: new THREE.CanvasTexture(canvas), transparent: true, depthTest: false,
  }));
  sprite.scale.set(24, 6, 1);
  sprite.position.y = 14;
  sprite.renderOrder = 10;
  return sprite;
}

function makeHoldRing() {  const ring = new THREE.Mesh(
    new THREE.RingGeometry(7, 9, 32),
    new THREE.MeshBasicMaterial({
      color: 0xffb020, transparent: true, opacity: 0.8, side: THREE.DoubleSide,
    }),
  );
  ring.rotation.x = -Math.PI / 2;
  return ring;
}

function ensureMesh(a) {
  let entry = meshes.get(a.id);
  if (entry) return entry;
  const built = a.kind === 'drone' ? makeDroneMesh() : makeAgentMesh();
  built.group.scale.setScalar(5); // aircraft are visible from the tower view
  const label = makeLabel(a.callsign);
  built.group.add(label);
  const ring = makeHoldRing();
  ring.visible = false;
  built.group.add(ring);
  // Server positions are world coords centered at origin; y is altitude.
  built.group.position.set(a.position[0], a.position[1], a.position[2]);
  built.target = built.group.position.clone();
  built.heading = a.heading;
  built.targetHeading = a.heading;
  built.state = a.state;
  built.ring = ring;
  scene.add(built.group);
  meshes.set(a.id, entry = built);
  return entry;
}

function setColor(entry, state) {
  const hex = new THREE.Color(stateColor(state));
  for (const m of entry.materials) {
    if (m.transparent && m.opacity < 1) continue; // keep rotors dark
    m.color.copy(hex);
  }
}

function disposeMesh(id) {
  const entry = meshes.get(id);
  if (!entry) return;
  scene.remove(entry.group);
  entry.group.traverse((o) => {
    if (o.geometry) o.geometry.dispose();
  });
  for (const m of entry.materials) m.dispose();
  meshes.delete(id);
}

/* --------------------------------------------------------------- ticker */

const tickerEl = document.getElementById('ticker-lines');
const pinnedHold = new Map(); // aircraft id -> line element, pinned while holding

function addTickerLine(text, holding = false, aircraftId = null) {
  const line = document.createElement('div');
  line.className = 'tick-line' + (holding ? ' hold' : '');
  line.textContent = text;
  line._aircraftId = aircraftId;
  tickerEl.appendChild(line);
  if (holding && aircraftId !== null) {
    pinnedHold.set(aircraftId, line);
  }
  while (tickerEl.children.length > 5) {
    // Never drop a pinned holding line; drop the oldest unpinned one.
    const first = tickerEl.firstElementChild;
    if (pinnedHold.has(first._aircraftId) && pinnedHold.get(first._aircraftId) === first) {
      tickerEl.appendChild(first);
      break; // all remaining lines are pinned
    }
    tickerEl.removeChild(first);
  }
}

function unpinHold(aircraftId) {
  const line = pinnedHold.get(aircraftId);
  if (line) {
    line.classList.remove('hold');
    pinnedHold.delete(aircraftId);
  }
}

/* ---------------------------------------------------------- websockets */

const statusEl = document.getElementById('hud-status');
const countEl = document.getElementById('hud-count');

let ws = null;
const lastStates = new Map(); // aircraft id -> state, for chatter lines

function sendJson(obj) {
  if (ws && ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(obj));
  }
}

setupRadio({
  send: (id, text) => sendJson({ type: 'message', aircraft_id: id, text }),
  approve: (id) => sendJson({ type: 'approve', aircraft_id: id }),
});

function connect() {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  ws = new WebSocket(`${proto}//${location.host}/ws`);

  ws.addEventListener('open', () => {
    statusEl.textContent = 'connected';
    statusEl.classList.remove('error');
  });
  ws.addEventListener('close', () => {
    statusEl.textContent = 'disconnected — reconnecting…';
    statusEl.classList.add('error');
    setTimeout(connect, 1500);
  });
  ws.addEventListener('error', () => ws.close());

  ws.addEventListener('message', (ev) => {
    let frame;
    try { frame = JSON.parse(ev.data); } catch { return; }
    if (frame.type !== 'snapshot' && frame.type !== 'delta') return;
    applySnapshot(frame.snapshot);
  });
}

function applySnapshot(snapshot) {
  currentSnapshot.clear();
  for (const a of snapshot.aircraft || []) currentSnapshot.set(a.id, a);
  const aircraft = snapshot.aircraft || [];
  const seen = new Set();

  for (const a of aircraft) {
    seen.add(a.id);
    const entry = ensureMesh(a);
    entry.target.set(a.position[0], a.position[1], a.position[2]);
    entry.targetHeading = a.heading;

    if (lastStates.get(a.id) !== a.state) {
      handleStateChange(a, lastStates.get(a.id), a.state);
      lastStates.set(a.id, a.state);
    }
    entry.state = a.state;
    setColor(entry, a.state);
    entry.ring.visible = a.state === 'holding';
  }

  for (const id of [...meshes.keys()]) {
    if (!seen.has(id)) {
      disposeMesh(id);
      lastStates.delete(id);
      unpinHold(id);
    }
  }

  countEl.textContent = aircraft.length === 0
    ? 'airspace empty'
    : `${aircraft.length} aircraft`;

  // Keep the radio panel in sync with the selected aircraft.
  const selectedId = selectedAircraftId();
  if (selectedId !== null) {
    updateRadio(aircraft.find((a) => a.id === selectedId));
    if (aircraft.find((a) => a.id === selectedId) === undefined) {
      closeRadio();
    }
  }
}

function handleStateChange(a, from, to) {
  const cs = a.callsign;
  if (to === 'holding') {
    addTickerLine(`${cs}: entering hold — awaiting approval`, true, a.id);
  } else {
    unpinHold(a.id);
    if (to === 'takeoff') addTickerLine(`${cs}: departing.`);
    else if (to === 'landing') addTickerLine(`${cs}: on approach.`);
    else if (to === 'landed') addTickerLine(`${cs}: landed, off frequency.`);
    else if (to === 'cruising' && from !== undefined) addTickerLine(`${cs}: cruising.`);
  }
  // Local note in radio history for visible state changes.
  if (selectedAircraftId() === a.id && from !== undefined) {
    const note = {
      holding: 'entering hold',
      takeoff: 'taking off',
      landing: 'landing',
      landed: 'landed',
      cruising: 'cruising',
    }[to];
    if (note && to !== 'holding') appendHistory(a.id, '·', `${a.callsign} ${note}`);
  }
}

/* -------------------------------------------------------------- picking */

const raycaster = new THREE.Raycaster();
const pointer = new THREE.Vector2();
let downPos = null;

canvas.addEventListener('pointerdown', (ev) => { downPos = [ev.clientX, ev.clientY]; });
canvas.addEventListener('pointerup', (ev) => {
  if (!downPos) return;
  const [dx, dy] = [ev.clientX - downPos[0], ev.clientY - downPos[1]];
  downPos = null;
  if (dx * dx + dy * dy > 25) return; // drag, not a click

  pointer.set(
    (ev.clientX / window.innerWidth) * 2 - 1,
    -(ev.clientY / window.innerHeight) * 2 + 1,
  );
  raycaster.setFromCamera(pointer, camera);
  const groups = [...meshes.values()].map((e) => e.group);
  const hits = raycaster.intersectObjects(groups, true);
  if (hits.length === 0) return;
  let obj = hits[0].object;
  while (obj.parent && !groups.includes(obj)) obj = obj.parent;
  if (!groups.includes(obj)) return;

  const id = [...meshes.entries()].find(([, e]) => e.group === obj)?.[0];
  if (id === undefined) return;

  const a = currentSnapshot.get(id);
  if (a) openRadio(a);
});

/* ----------------------------------------------------------- main loop */

const currentSnapshot = new Map(); // latest per-aircraft data, for picking
const clock = new THREE.Clock();

function animate() {
  requestAnimationFrame(animate);
  const dt = clock.getDelta();
  const k = 1 - Math.exp(-INTERP_RATE * dt);

  const now = performance.now() / 1000;
  for (const entry of meshes.values()) {
    entry.group.position.lerp(entry.target, k);
    let dh = entry.targetHeading - entry.heading;
    while (dh > Math.PI) dh -= 2 * Math.PI;
    while (dh < -Math.PI) dh += 2 * Math.PI;
    entry.heading += dh * k;
    entry.group.rotation.y = -entry.heading; // heading: radians, CCW in world

    if (entry.ring.visible) {
      const pulse = 1 + 0.25 * Math.sin(now * 4);
      entry.ring.scale.setScalar(pulse);
      entry.ring.position.y = 0.5 - entry.group.position.y; // hug the ground
      entry.ring.material.opacity = 0.5 + 0.35 * Math.sin(now * 4);
    }
  }

  controls.update();
  renderer.render(scene, camera);
}

window.addEventListener('resize', () => {
  camera.aspect = window.innerWidth / window.innerHeight;
  camera.updateProjectionMatrix();
  renderer.setSize(window.innerWidth, window.innerHeight);
});

connect();
animate();
