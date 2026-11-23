// Skywatch browser client: Three.js scene, WS snapshot/delta consumption,
// smooth aircraft interpolation, picking, radio panel, ambient ticker.

import * as THREE from './vendor/three.module.js';
import { OrbitControls } from './vendor/OrbitControls.js';
import {
  setupRadio, openRadio, closeRadio, updateRadio, selectedAircraftId,
  appendHistory, stateColor,
} from './radio.js';

const WORLD_SIZE = 700;
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
camera.position.set(0, 620, 400);
camera.lookAt(0, 0, 0);

const controls = new OrbitControls(camera, canvas);
controls.target.set(0, 30, 0);
controls.enableDamping = true;
controls.dampingFactor = 0.08;
controls.maxPolarAngle = Math.PI / 2 - 0.05;
controls.minDistance = 40;
controls.maxDistance = 900;

scene.add(new THREE.AmbientLight(0x8899bb, 0.7));
const sun = new THREE.DirectionalLight(0xffffff, 1.4);
sun.position.set(300, 500, 200);
scene.add(sun);

// Sea + grid, and the island (home of the Tower).
{
  const sea = new THREE.Mesh(
    new THREE.PlaneGeometry(WORLD_SIZE, WORLD_SIZE),
    new THREE.MeshLambertMaterial({ color: 0x0d2b3a }),
  );
  sea.rotation.x = -Math.PI / 2;
  sea.position.y = 0;
  scene.add(sea);

  const grid = new THREE.GridHelper(WORLD_SIZE, 40, 0x1f6f8f, 0x14455c);
  grid.position.y = 0.5;
  scene.add(grid);

  // Island: irregular noise coastline, raised beach with cliffs, layered
  // grass plateau, hill, tower, palms and a runway on the sand flat.
  const island = new THREE.Group();

  // Deterministic radial-noise coastline (smooth, no per-load variation).
  const coastRadius = (angle, base, noise) => {
    const a1 = Math.sin(angle * 3 + 1.7) * noise;
    const a2 = Math.sin(angle * 5 + 4.2) * noise * 0.5;
    const a3 = Math.sin(angle * 8 + 2.9) * noise * 0.25;
    return base + a1 + a2 + a3;
  };

  // Shape from a radial-noise outline; XY plane, extruded along +Z then
  // rotated flat so the extrusion becomes vertical thickness (cliffs).
  const coastShape = (base, noise) => {
    const shape = new THREE.Shape();
    const steps = 64;
    for (let i = 0; i <= steps; i++) {
      const a = (i / steps) * Math.PI * 2;
      const r = coastRadius(a, base, noise);
      const x = Math.cos(a) * r;
      const y = Math.sin(a) * r;
      if (i === 0) shape.moveTo(x, y);
      else shape.lineTo(x, y);
    }
    shape.closePath();
    return shape;
  };

  const extrudeFlat = (shape, thickness, topY, mat) => {
    const geo = new THREE.ExtrudeGeometry(shape, {
      depth: thickness, bevelEnabled: false, curveSegments: 48,
    });
    geo.rotateX(-Math.PI / 2); // shape XY -> XZ; extrusion runs UP
    const mesh = new THREE.Mesh(geo, mat);
    mesh.position.y = topY - thickness; // top face lands exactly at topY
    return mesh;
  };

  const sandMat = new THREE.MeshLambertMaterial({ color: 0xd6c084 });
  const cliffMat = new THREE.MeshLambertMaterial({ color: 0xb59a63 });
  const grassMat = new THREE.MeshLambertMaterial({ color: 0x3f7a45 });
  const grassDarkMat = new THREE.MeshLambertMaterial({ color: 0x35683b });

  // Beach plateau: top at y = 2, cliffs down to -8 under the sea.
  island.add(extrudeFlat(coastShape(115, 22), 10, 2, sandMat));

  // Grass plateau on top: slightly smaller, top at y = 3.4.
  island.add(extrudeFlat(coastShape(82, 14), 4, 3.4, grassMat));

  // A second, darker grass tier (the hill's base).
  island.add(extrudeFlat(coastShape(52, 8), 3, 4.6, grassDarkMat));

  // Rounded hill under the tower (smooth dome instead of a hard cone).
  const hill = new THREE.Mesh(
    new THREE.SphereGeometry(42, 24, 12, 0, Math.PI * 2, 0, Math.PI / 2),
    grassDarkMat,
  );
  hill.scale.y = 0.35; // squash into a dome
  hill.position.y = 6.1;
  island.add(hill);

  // The Tower itself: dark base, cabin disc, radar bar.
  const towerMat = new THREE.MeshLambertMaterial({ color: 0x25333d });
  const base = new THREE.Mesh(new THREE.CylinderGeometry(4, 6, 26, 12), towerMat);
  base.position.set(0, 28, 0);
  island.add(base);

  const cabin = new THREE.Mesh(
    new THREE.CylinderGeometry(11, 8, 7, 12),
    new THREE.MeshLambertMaterial({ color: 0x8fb6c9 }),
  );
  cabin.position.set(0, 44, 0);
  island.add(cabin);

  const roof = new THREE.Mesh(
    new THREE.ConeGeometry(12, 5, 12),
    new THREE.MeshLambertMaterial({ color: 0x1d2830 }),
  );
  roof.position.set(0, 50, 0);
  island.add(roof);

  const radar = new THREE.Mesh(new THREE.BoxGeometry(9, 0.6, 1), towerMat);
  radar.position.set(0, 54, 0);
  radar.name = 'radar';
  island.add(radar);

  // Palms for scale: tilted trunk + a fan of leaf cones.
  const trunkMat = new THREE.MeshLambertMaterial({ color: 0x6b4f2f });
  const leafMat = new THREE.MeshLambertMaterial({ color: 0x2f8f4a });
  const palmSpots = [[-58, 12], [-40, -46], [24, 52], [48, 38], [-8, 62], [64, -6]];
  for (const [px, pz] of palmSpots) {
    const palm = new THREE.Group();
    const trunk = new THREE.Mesh(new THREE.CylinderGeometry(0.5, 0.9, 16, 6), trunkMat);
    trunk.position.y = 8;
    trunk.rotation.z = (Math.random() - 0.5) * 0.25;
    palm.add(trunk);
    for (let i = 0; i < 6; i++) {
      const leaf = new THREE.Mesh(new THREE.ConeGeometry(1.1, 8, 4), leafMat);
      const la = (i / 6) * Math.PI * 2;
      leaf.position.set(Math.cos(la) * 3.4, 16, Math.sin(la) * 3.4);
      leaf.rotation.z = Math.cos(la) * 1.25;
      leaf.rotation.x = -Math.sin(la) * 1.25;
      palm.add(leaf);
    }
    const grassTopY = 3.4;
    palm.position.set(px, grassTopY, pz);
    island.add(palm);
  }

  // Rocks on the cliffs for texture.
  const rockMat = new THREE.MeshLambertMaterial({ color: 0x8c8f94 });
  for (const [rx, rz, rs] of [[-98, 30, 4], [-88, -55, 3], [95, 55, 4.5], [70, 78, 2.6], [20, -100, 3.4]]) {
    const rock = new THREE.Mesh(new THREE.DodecahedronGeometry(rs, 0), rockMat);
    rock.position.set(rx, 0.8, rz);
    rock.rotation.set(rx, rz, rx * 0.5);
    island.add(rock);
  }

  // Runway strip on the sand flat, east side (clear of the grass plateau).
  const runway = new THREE.Mesh(
    new THREE.PlaneGeometry(16, 95),
    new THREE.MeshLambertMaterial({ color: 0x2b2f36 }),
  );
  runway.rotation.x = -Math.PI / 2;
  runway.position.set(88, 2.2, -28);
  island.add(runway);

  // Centreline dashes.
  for (let i = 0; i < 8; i++) {
    const dash = new THREE.Mesh(
      new THREE.PlaneGeometry(1.2, 6),
      new THREE.MeshBasicMaterial({ color: 0xd8dcd0 }),
    );
    dash.rotation.x = -Math.PI / 2;
    dash.position.set(88, 2.3, -62 + i * 10);
    island.add(dash);
  }

  island.position.set(0, 0, 0);
  scene.add(island);
}

/* ------------------------------------------------------------- sectors */

// Sector rings + labels on the water, built once from the first snapshot.
const TOPIC_COLORS = {
  Deploy: 0x35c4d6,
  Refactor: 0x7fd635,
  Monitor: 0xb07fd6,
};
let sectorsBuilt = false;

function topicColor(name) {
  return TOPIC_COLORS[name] || 0x88aacc;
}

function ensureSectors(sectors) {
  if (sectorsBuilt || sectors.length === 0) return;
  sectorsBuilt = true;
  for (const s of sectors) {
    const color = topicColor(s.name);

    // Thin boundary ring on the water.
    const ring = new THREE.Mesh(
      new THREE.RingGeometry(s.radius - 1.5, s.radius, 48),
      new THREE.MeshBasicMaterial({
        color, transparent: true, opacity: 0.35, side: THREE.DoubleSide,
      }),
    );
    ring.rotation.x = -Math.PI / 2;
    ring.position.set(s.center[0], 0.8, s.center[1]);
    scene.add(ring);

    // Faint filled disc so the sector reads as area, not just line.
    const fill = new THREE.Mesh(
      new THREE.CircleGeometry(s.radius, 48),
      new THREE.MeshBasicMaterial({
        color, transparent: true, opacity: 0.05, side: THREE.DoubleSide,
      }),
    );
    fill.rotation.x = -Math.PI / 2;
    fill.position.set(s.center[0], 0.7, s.center[1]);
    scene.add(fill);

    // Sector name label.
    const label = makeLabel(s.name.toUpperCase(), color);
    label.position.set(s.center[0], 26, s.center[1]);
    label.scale.set(42, 10.5, 1);
    scene.add(label);
  }
}

/* ------------------------------------------------------ aircraft meshes */

// Group per aircraft: mesh + (optionally) hold ring. Meshes are created on
// first sight and driven by snapshots with exponential smoothing.
const meshes = new Map();

// Little private plane (single prop): local models. Points along +Z.
function makeAgentMesh() {
  const group = new THREE.Group();
  const mat = new THREE.MeshLambertMaterial({ color: 0xffffff });
  const darkMat = new THREE.MeshLambertMaterial({ color: 0x222831 });
  const propMat = new THREE.MeshLambertMaterial({
    color: 0x222831, transparent: true, opacity: 0.35, side: THREE.DoubleSide,
  });

  // Cabin: rounded fuselage.
  const fuselage = new THREE.Mesh(new THREE.CapsuleGeometry(1.3, 7, 6, 10), mat);
  fuselage.rotation.x = Math.PI / 2; // along Z
  fuselage.position.z = -1;
  group.add(fuselage);

  // Nose cone + spinner.
  const nose = new THREE.Mesh(new THREE.ConeGeometry(1.3, 3.5, 10), mat);
  nose.rotation.x = Math.PI / 2; // point along +Z
  nose.position.z = 5.5;
  group.add(nose);

  // Tractor propeller at the nose (spinning disc).
  const prop = new THREE.Mesh(new THREE.CircleGeometry(2.2, 14), propMat);
  prop.position.z = 7.2;
  group.add(prop);
  prop.name = 'prop';

  // High wing over the cabin.
  const wings = new THREE.Mesh(new THREE.BoxGeometry(15, 0.4, 2.4), mat);
  wings.position.set(0, 1.3, -0.5);
  group.add(wings);

  // Struts.
  for (const side of [1, -1]) {
    const strut = new THREE.Mesh(new THREE.BoxGeometry(0.3, 0.3, 2), darkMat);
    strut.position.set(side * 4.5, 0.2, -0.5);
    strut.rotation.z = side * 0.45;
    group.add(strut);
  }

  // Tailplane + fin.
  const tail = new THREE.Mesh(new THREE.BoxGeometry(6, 0.4, 2), mat);
  tail.position.set(0, 0.6, -5.5);
  group.add(tail);

  const fin = new THREE.Mesh(new THREE.BoxGeometry(0.4, 2.6, 2), mat);
  fin.position.set(0, 1.8, -5.8);
  group.add(fin);

  return { group, materials: [mat, darkMat, propMat] };
}

// Big twin-engine jet: frontier models. Points along +Z.
function makeJetMesh() {
  const group = new THREE.Group();
  const mat = new THREE.MeshLambertMaterial({ color: 0xffffff });
  const darkMat = new THREE.MeshLambertMaterial({ color: 0x222831 });

  // Long airliner fuselage.
  const fuselage = new THREE.Mesh(new THREE.CapsuleGeometry(1.7, 20, 6, 10), mat);
  fuselage.rotation.x = Math.PI / 2; // along Z
  fuselage.position.z = -2;
  group.add(fuselage);

  // Cockpit nose cone.
  const nose = new THREE.Mesh(new THREE.ConeGeometry(1.7, 4, 10), mat);
  nose.rotation.x = Math.PI / 2;
  nose.position.z = 11.5;
  group.add(nose);

  // Swept wings.
  for (const side of [1, -1]) {
    const wing = new THREE.Mesh(new THREE.BoxGeometry(13, 0.5, 4), mat);
    wing.position.set(side * 6.5, -0.8, -2);
    wing.rotation.y = side * -0.35; // sweep back
    group.add(wing);
  }

  // Engines under the wings.
  for (const side of [1, -1]) {
    const engine = new THREE.Mesh(new THREE.CylinderGeometry(1.1, 1.1, 3.4, 10), darkMat);
    engine.rotation.x = Math.PI / 2; // along Z
    engine.position.set(side * 5.5, -2, 0.5);
    group.add(engine);
  }

  // Tailplane + tall swept fin.
  const tail = new THREE.Mesh(new THREE.BoxGeometry(9, 0.4, 2.6), mat);
  tail.position.set(0, 1.2, -10);
  group.add(tail);

  const fin = new THREE.Mesh(new THREE.BoxGeometry(0.5, 5.5, 3.6), mat);
  fin.position.set(0, 3.4, -10.5);
  fin.rotation.x = -0.25;
  group.add(fin);

  return { group, materials: [mat, darkMat] };
}

// MQ-9-style drone: long fuselage, wide straight wings, inverted V-tail,
// pusher prop at the rear, sensor turret under the nose. Points along +Z.
function makeDroneMesh() {
  const group = new THREE.Group();
  const mat = new THREE.MeshLambertMaterial({ color: 0xffffff });
  const darkMat = new THREE.MeshLambertMaterial({ color: 0x222831 });
  const rotorMat = new THREE.MeshLambertMaterial({
    color: 0x222831, transparent: true, opacity: 0.35, side: THREE.DoubleSide,
  });

  // Fuselage: tapered nose cone + long body.
  const nose = new THREE.Mesh(new THREE.ConeGeometry(1.4, 5, 8), mat);
  nose.rotation.x = Math.PI / 2; // point along +Z
  nose.position.z = 9;
  group.add(nose);

  const body = new THREE.Mesh(new THREE.CylinderGeometry(1.4, 1.1, 16, 8), mat);
  body.rotation.x = Math.PI / 2; // along Z
  body.position.z = -0.5;
  group.add(body);

  // Sensor turret under the nose.
  const turret = new THREE.Mesh(new THREE.SphereGeometry(1.1, 12, 12), darkMat);
  turret.position.set(0, -1.3, 6.5);
  group.add(turret);

  // Wide straight wings.
  const wings = new THREE.Mesh(new THREE.BoxGeometry(24, 0.4, 2.6), mat);
  wings.position.set(0, 0.6, 1);
  group.add(wings);

  // Inverted V-tail at the rear.
  for (const side of [1, -1]) {
    const fin = new THREE.Mesh(new THREE.BoxGeometry(0.4, 5, 2.2), mat);
    fin.position.set(side * 1.6, 1.6, -7);
    fin.rotation.z = side * -0.6; // lean outward → inverted V
    group.add(fin);
  }

  // Pusher propeller at the rear (spinning disc).
  const prop = new THREE.Mesh(new THREE.CircleGeometry(2.6, 16), rotorMat);
  prop.position.set(0, 0.4, -8.8);
  group.add(prop);
  prop.name = 'prop';

  return { group, materials: [mat, darkMat, rotorMat] };
}

// Canvas-sprite callsign label that stays screen-facing and constant-ish size.
function makeLabel(callsign, tint = 0xe8ffff) {
  const canvas = document.createElement('canvas');
  canvas.width = 256;
  canvas.height = 64;
  const ctx = canvas.getContext('2d');
  ctx.fillStyle = 'rgba(0, 20, 20, 0.55)';
  ctx.fillRect(0, 0, 256, 64);
  ctx.font = 'bold 34px monospace';
  ctx.fillStyle = '#' + new THREE.Color(tint).getHexString();
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
  const built = a.kind === 'drone'
    ? makeDroneMesh()
    : (a.model_class === 'frontier' ? makeJetMesh() : makeAgentMesh());
  built.group.scale.setScalar(5); // aircraft are visible from the tower view
  const label = makeLabel(a.callsign, a.topic ? topicColor(a.topic) : 0xe8ffff);
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
    if (o.material && !entry.materials.includes(o.material)) {
      if (o.material.map) o.material.map.dispose();
      o.material.dispose();
    }
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

  ensureSectors(snapshot.sectors || []);

  for (const a of aircraft) {
    seen.add(a.id);
    const entry = ensureMesh(a);
    // Landed aircraft sit on the island sand (top ~2 units above the sea).
    const groundLift = a.state === 'landed' ? 1.6 : 0;
    entry.target.set(a.position[0], a.position[1] + groundLift, a.position[2]);
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

    // Spin the pusher prop on drones and the tower radar.
    const prop = entry.group.getObjectByName('prop');
    if (prop) prop.rotation.y = now * 30;

    if (entry.ring.visible) {
      const pulse = 1 + 0.25 * Math.sin(now * 4);
      entry.ring.scale.setScalar(pulse);
      entry.ring.position.y = 0.5 - entry.group.position.y; // hug the ground
      entry.ring.material.opacity = 0.5 + 0.35 * Math.sin(now * 4);
    }
  }

  controls.update();

  const radar = scene.getObjectByName('radar');
  if (radar) radar.rotation.y = now * 0.8;

  renderer.render(scene, camera);
}

window.addEventListener('resize', () => {
  camera.aspect = window.innerWidth / window.innerHeight;
  camera.updateProjectionMatrix();
  renderer.setSize(window.innerWidth, window.innerHeight);
});

connect();
animate();
