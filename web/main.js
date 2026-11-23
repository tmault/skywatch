// Skywatch browser client: Three.js scene, WS snapshot/delta consumption,
// smooth aircraft interpolation, device airports, fighter wingmen, picking,
// radio panel, and ambient ticker.

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
scene.fog = new THREE.Fog(0x123a5c, 480, 1500);

const camera = new THREE.PerspectiveCamera(
  52, window.innerWidth / window.innerHeight, 1, 3000,
);
camera.position.set(0, 420, 360);
camera.lookAt(0, 20, 0);

const controls = new OrbitControls(camera, canvas);
controls.target.set(0, 20, 0);
controls.enableDamping = true;
controls.dampingFactor = 0.08;
controls.maxPolarAngle = Math.PI / 2 - 0.05;
controls.minDistance = 40;
controls.maxDistance = 950;

scene.add(new THREE.AmbientLight(0x8899bb, 0.75));
const sun = new THREE.DirectionalLight(0xffffff, 1.4);
sun.position.set(300, 500, 200);
scene.add(sun);

// Sea + radar grid.
{
  const sea = new THREE.Mesh(
    new THREE.PlaneGeometry(WORLD_SIZE + 200, WORLD_SIZE + 200),
    new THREE.MeshLambertMaterial({ color: 0x0d2b3a }),
  );
  sea.rotation.x = -Math.PI / 2;
  sea.position.y = 0;
  scene.add(sea);

  const grid = new THREE.GridHelper(WORLD_SIZE, 35, 0x1f6f8f, 0x14455c);
  grid.position.y = 0.4;
  scene.add(grid);
}

/* ----------------------------------------------------------- airports */

// Shared terrain helpers for building organic island airports.
function coastRadius(angle, base, noise, seed = 0) {
  const a1 = Math.sin(angle * 3 + 1.7 + seed) * noise;
  const a2 = Math.sin(angle * 5 + 4.2 + seed * 1.3) * noise * 0.5;
  const a3 = Math.sin(angle * 8 + 2.9 + seed * 0.7) * noise * 0.25;
  return base + a1 + a2 + a3;
}

function coastShape(base, noise, seed = 0) {
  const shape = new THREE.Shape();
  const steps = 64;
  for (let i = 0; i <= steps; i++) {
    const a = (i / steps) * Math.PI * 2;
    const r = coastRadius(a, base, noise, seed);
    const x = Math.cos(a) * r;
    const y = Math.sin(a) * r;
    if (i === 0) shape.moveTo(x, y);
    else shape.lineTo(x, y);
  }
  shape.closePath();
  return shape;
}

function extrudeFlat(shape, thickness, topY, mat) {
  const geo = new THREE.ExtrudeGeometry(shape, {
    depth: thickness, bevelEnabled: false, curveSegments: 48,
  });
  geo.rotateX(-Math.PI / 2);
  const mesh = new THREE.Mesh(geo, mat);
  mesh.position.y = topY - thickness;
  return mesh;
}

function makePalm(seed) {
  const trunkMat = new THREE.MeshLambertMaterial({ color: 0x6b4f2f });
  const leafMat = new THREE.MeshLambertMaterial({ color: 0x2f8f4a });
  const palm = new THREE.Group();
  const trunk = new THREE.Mesh(new THREE.CylinderGeometry(0.45, 0.85, 14, 6), trunkMat);
  trunk.position.y = 7;
  trunk.rotation.z = Math.sin(seed * 7.3) * 0.14;
  palm.add(trunk);
  for (let i = 0; i < 6; i++) {
    const leaf = new THREE.Mesh(new THREE.ConeGeometry(1.0, 7.5, 4), leafMat);
    const la = (i / 6) * Math.PI * 2;
    leaf.position.set(Math.cos(la) * 3.1, 14, Math.sin(la) * 3.1);
    leaf.rotation.z = Math.cos(la) * 1.25;
    leaf.rotation.x = -Math.sin(la) * 1.25;
    palm.add(leaf);
  }
  return palm;
}

const airportGroups = new Map(); // airport.id -> { group, beacon, online }
const radars = [];

function buildHomeAirport(ap) {
  const island = new THREE.Group();
  const sandMat = new THREE.MeshLambertMaterial({ color: 0xd6c084 });
  const grassMat = new THREE.MeshLambertMaterial({ color: 0x3f7a45 });
  const grassDarkMat = new THREE.MeshLambertMaterial({ color: 0x35683b });
  const asphaltMat = new THREE.MeshLambertMaterial({ color: 0x2b2f36 });
  const apronMat = new THREE.MeshLambertMaterial({ color: 0x3a4048 });
  const terminalMat = new THREE.MeshLambertMaterial({ color: 0x4b5d6c });
  const glassMat = new THREE.MeshLambertMaterial({ color: 0x8fd0e8 });

  // Broad sandy island plateau (top at y = 2.0).
  island.add(extrudeFlat(coastShape(128, 16, 0), 10, 2.0, sandMat));

  // Grass plateau + hill shifted slightly west so east side is airport flat.
  const grass1 = extrudeFlat(coastShape(66, 10, 0), 4, 3.4, grassMat);
  grass1.position.x = -26;
  island.add(grass1);

  const grass2 = extrudeFlat(coastShape(42, 6, 0), 3, 4.6, grassDarkMat);
  grass2.position.x = -26;
  island.add(grass2);

  const hill = new THREE.Mesh(
    new THREE.SphereGeometry(34, 24, 12, 0, Math.PI * 2, 0, Math.PI / 2),
    grassDarkMat,
  );
  hill.scale.y = 0.35;
  hill.position.set(-26, 6.0, 0);
  island.add(hill);

  // Control Tower on the hill.
  const towerMat = new THREE.MeshLambertMaterial({ color: 0x25333d });
  const base = new THREE.Mesh(new THREE.CylinderGeometry(3.8, 5.5, 24, 12), towerMat);
  base.position.set(-26, 26, 0);
  island.add(base);

  const cabin = new THREE.Mesh(new THREE.CylinderGeometry(10, 7.5, 6.5, 12), glassMat);
  cabin.position.set(-26, 40.5, 0);
  island.add(cabin);

  const roof = new THREE.Mesh(
    new THREE.ConeGeometry(11, 4.5, 12),
    new THREE.MeshLambertMaterial({ color: 0x1d2830 }),
  );
  roof.position.set(-26, 46, 0);
  island.add(roof);

  const radar = new THREE.Mesh(new THREE.BoxGeometry(8.5, 0.6, 1), towerMat);
  radar.position.set(-26, 49.5, 0);
  island.add(radar);
  radars.push(radar);

  const beaconMat = new THREE.MeshBasicMaterial({ color: 0x4dff9e });
  const beacon = new THREE.Mesh(new THREE.SphereGeometry(1.4, 10, 10), beaconMat);
  beacon.position.set(-26, 51.5, 0);
  island.add(beacon);

  // Palms on the grass side.
  const palmSpots = [[-66, 18], [-58, -36], [-8, 42], [-12, -44], [-42, 46], [-72, -10]];
  palmSpots.forEach(([px, pz], idx) => {
    const palm = makePalm(idx + 1);
    palm.position.set(px, 3.4, pz);
    island.add(palm);
  });

  // Runway: 20 wide x 105 long at [66, -10].
  const runway = new THREE.Mesh(new THREE.PlaneGeometry(20, 105), asphaltMat);
  runway.rotation.x = -Math.PI / 2;
  runway.position.set(66, 2.2, -10);
  island.add(runway);

  // Runway centreline dashes & threshold piano keys.
  const lineMat = new THREE.MeshBasicMaterial({ color: 0xd8dcd0 });
  for (let i = 0; i < 9; i++) {
    const dash = new THREE.Mesh(new THREE.PlaneGeometry(1.1, 5.5), lineMat);
    dash.rotation.x = -Math.PI / 2;
    dash.position.set(66, 2.3, -50 + i * 10);
    island.add(dash);
  }
  for (const endZ of [-58, 38]) {
    for (let k = -2; k <= 2; k++) {
      if (k === 0) continue;
      const key = new THREE.Mesh(new THREE.PlaneGeometry(1.3, 4.5), lineMat);
      key.rotation.x = -Math.PI / 2;
      key.position.set(66 + k * 3.0, 2.3, endZ);
      island.add(key);
    }
  }

  // Taxiway connectors + concrete terminal apron.
  const apron = new THREE.Mesh(new THREE.PlaneGeometry(40, 124), apronMat);
  apron.rotation.x = -Math.PI / 2;
  apron.position.set(96, 2.15, -5);
  island.add(apron);

  // Terminal building along the east side of the apron.
  const terminal = new THREE.Mesh(new THREE.BoxGeometry(10, 6.5, 110), terminalMat);
  terminal.position.set(113, 5.4, -5);
  island.add(terminal);
  const terminalGlass = new THREE.Mesh(new THREE.BoxGeometry(0.6, 2.8, 104), glassMat);
  terminalGlass.position.set(107.8, 6.0, -5);
  island.add(terminalGlass);

  // Gate stand markings + jetbridges for each gate.
  const gateLineMat = new THREE.MeshBasicMaterial({
    color: 0xffc857, transparent: true, opacity: 0.65, side: THREE.DoubleSide,
  });
  const bridgeMat = new THREE.MeshLambertMaterial({ color: 0x9aa9b5 });
  for (const [gx, gz] of ap.gates) {
    const lead = new THREE.Mesh(new THREE.PlaneGeometry(14, 0.8), gateLineMat);
    lead.rotation.x = -Math.PI / 2;
    lead.position.set(gx, 2.25, gz);
    island.add(lead);

    const stop = new THREE.Mesh(new THREE.PlaneGeometry(0.8, 8), gateLineMat);
    stop.rotation.x = -Math.PI / 2;
    stop.position.set(gx + 5, 2.25, gz);
    island.add(stop);

    const bridge = new THREE.Mesh(new THREE.BoxGeometry(7, 1.4, 1.8), bridgeMat);
    bridge.position.set(gx + 9.5, 4.8, gz - 3.5);
    island.add(bridge);
  }

  // Airport name banner above the tower.
  const label = makeLabel(ap.name.toUpperCase(), 0x5be7ff);
  label.position.set(-26, 63, 0);
  label.scale.set(42, 10.5, 1);
  island.add(label);

  scene.add(island);
  return { group: island, beacon, online: ap.online };
}

function buildSatelliteAirport(ap, idx) {
  const island = new THREE.Group();
  const [cx, cz] = ap.center;
  const seed = (idx + 1) * 2.17;
  const sandMat = new THREE.MeshLambertMaterial({ color: 0xcab67c });
  const grassMat = new THREE.MeshLambertMaterial({ color: 0x3b7243 });
  const asphaltMat = new THREE.MeshLambertMaterial({ color: 0x2b2f36 });
  const apronMat = new THREE.MeshLambertMaterial({ color: 0x3a4048 });
  const terminalMat = new THREE.MeshLambertMaterial({ color: 0x4b5d6c });
  const glassMat = new THREE.MeshLambertMaterial({ color: 0x8fd0e8 });

  // Compact sandy island + grass cap.
  island.add(extrudeFlat(coastShape(58, 8, seed), 8, 2.0, sandMat));
  const grass = extrudeFlat(coastShape(30, 5, seed), 3, 3.2, grassMat);
  grass.position.set(-22, 0, 0);
  island.add(grass);

  // Mini control tower.
  const towerMat = new THREE.MeshLambertMaterial({ color: 0x25333d });
  const base = new THREE.Mesh(new THREE.CylinderGeometry(2.6, 3.8, 16, 10), towerMat);
  base.position.set(-24, 11, 0);
  island.add(base);

  const cabin = new THREE.Mesh(new THREE.CylinderGeometry(6.5, 5, 4.5, 10), glassMat);
  cabin.position.set(-24, 21, 0);
  island.add(cabin);

  const beaconMat = new THREE.MeshBasicMaterial({
    color: ap.online ? 0x4dff9e : 0xff5a5a,
  });
  const beacon = new THREE.Mesh(new THREE.SphereGeometry(1.3, 10, 10), beaconMat);
  beacon.position.set(-24, 25.5, 0);
  island.add(beacon);

  // Mini runway: 15 wide x 72 long.
  const runway = new THREE.Mesh(new THREE.PlaneGeometry(15, 72), asphaltMat);
  runway.rotation.x = -Math.PI / 2;
  runway.position.set(2, 2.2, 0);
  island.add(runway);

  const lineMat = new THREE.MeshBasicMaterial({ color: 0xd8dcd0 });
  for (let i = 0; i < 6; i++) {
    const dash = new THREE.Mesh(new THREE.PlaneGeometry(0.9, 4.5), lineMat);
    dash.rotation.x = -Math.PI / 2;
    dash.position.set(2, 2.3, -27 + i * 10.5);
    island.add(dash);
  }

  // Apron + small terminal + 4 gates.
  const apron = new THREE.Mesh(new THREE.PlaneGeometry(24, 68), apronMat);
  apron.rotation.x = -Math.PI / 2;
  apron.position.set(22, 2.15, 0);
  island.add(apron);

  const terminal = new THREE.Mesh(new THREE.BoxGeometry(7, 5, 58), terminalMat);
  terminal.position.set(35, 4.7, 0);
  island.add(terminal);

  const gateLineMat = new THREE.MeshBasicMaterial({
    color: 0xffc857, transparent: true, opacity: 0.55, side: THREE.DoubleSide,
  });
  for (const [gx, gz] of ap.gates) {
    const lead = new THREE.Mesh(new THREE.PlaneGeometry(10, 0.7), gateLineMat);
    lead.rotation.x = -Math.PI / 2;
    lead.position.set(gx - cx, 2.25, gz - cz);
    island.add(lead);
  }

  // Device name banner (green-cyan when online, muted red when offline).
  const tint = ap.online ? 0x6ef3c5 : 0xd97373;
  const suffix = ap.online ? '' : ' (OFF)';
  const label = makeLabel(`${ap.name.toUpperCase()}${suffix}`, tint);
  label.position.set(0, 38, 0);
  label.scale.set(44, 11, 1);
  island.add(label);

  island.position.set(cx, 0, cz);
  scene.add(island);
  return { group: island, beacon, online: ap.online };
}

function ensureAirports(airports) {
  if (!airports || airports.length === 0) return;
  let satIdx = 0;
  for (const ap of airports) {
    const existing = airportGroups.get(ap.id);
    if (existing) {
      if (existing.online !== ap.online) {
        existing.online = ap.online;
        existing.beacon.material.color.setHex(ap.online ? 0x4dff9e : 0xff5a5a);
      }
      continue;
    }
    const built = ap.is_home
      ? buildHomeAirport(ap)
      : buildSatelliteAirport(ap, satIdx++);
    airportGroups.set(ap.id, built);
  }
}

/* ------------------------------------------------------------- sectors */

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

    const ring = new THREE.Mesh(
      new THREE.RingGeometry(s.radius - 1.4, s.radius, 48),
      new THREE.MeshBasicMaterial({
        color, transparent: true, opacity: 0.35, side: THREE.DoubleSide,
      }),
    );
    ring.rotation.x = -Math.PI / 2;
    ring.position.set(s.center[0], 0.7, s.center[1]);
    scene.add(ring);

    const fill = new THREE.Mesh(
      new THREE.CircleGeometry(s.radius, 48),
      new THREE.MeshBasicMaterial({
        color, transparent: true, opacity: 0.05, side: THREE.DoubleSide,
      }),
    );
    fill.rotation.x = -Math.PI / 2;
    fill.position.set(s.center[0], 0.6, s.center[1]);
    scene.add(fill);

    const label = makeLabel(s.name.toUpperCase(), color);
    label.position.set(s.center[0], 20, s.center[1]);
    label.scale.set(36, 9, 1);
    scene.add(label);
  }
}

/* ------------------------------------------------------ aircraft meshes */

const meshes = new Map();

// Little private plane (single prop): local models. Points along +Z.
function makeAgentMesh() {
  const group = new THREE.Group();
  const mat = new THREE.MeshLambertMaterial({ color: 0xffffff });
  const darkMat = new THREE.MeshLambertMaterial({ color: 0x222831 });
  const propMat = new THREE.MeshLambertMaterial({
    color: 0x222831, transparent: true, opacity: 0.35, side: THREE.DoubleSide,
  });

  const fuselage = new THREE.Mesh(new THREE.CapsuleGeometry(1.3, 5, 8, 14), mat);
  fuselage.rotation.x = Math.PI / 2;
  fuselage.position.z = 0.6;
  group.add(fuselage);

  const boom = new THREE.Mesh(new THREE.ConeGeometry(1.3, 3, 14), mat);
  boom.rotation.x = -Math.PI / 2;
  boom.position.z = -4.6;
  group.add(boom);

  const cowl = new THREE.Mesh(new THREE.CylinderGeometry(1.0, 1.45, 2, 14), mat);
  cowl.rotation.x = Math.PI / 2;
  cowl.position.z = 4.9;
  group.add(cowl);

  const spinner = new THREE.Mesh(new THREE.ConeGeometry(0.55, 1.3, 12), mat);
  spinner.rotation.x = Math.PI / 2;
  spinner.position.z = 6.55;
  group.add(spinner);

  const prop = new THREE.Group();
  const disc = new THREE.Mesh(new THREE.CircleGeometry(2.3, 16), propMat);
  const blades = new THREE.Mesh(new THREE.BoxGeometry(0.28, 4.4, 0.14), darkMat);
  prop.add(disc, blades);
  prop.position.z = 7.05;
  prop.name = 'prop';
  group.add(prop);

  const cabin = new THREE.Mesh(new THREE.BoxGeometry(1.8, 1.1, 2.9), mat);
  cabin.position.set(0, 1.35, 0.5);
  group.add(cabin);

  for (const side of [1, -1]) {
    const wing = new THREE.Mesh(new THREE.BoxGeometry(6.4, 0.42, 2.6), mat);
    wing.position.set(side * 4.3, 2.05, -0.4);
    wing.rotation.z = side * 0.16;
    group.add(wing);
  }

  for (const side of [1, -1]) {
    const win = new THREE.Mesh(new THREE.BoxGeometry(0.16, 0.55, 2.3), darkMat);
    win.position.set(side * 0.92, 1.1, 0.7);
    group.add(win);
  }
  const glass = new THREE.Mesh(new THREE.BoxGeometry(1.5, 0.6, 0.18), darkMat);
  glass.position.set(0, 1.55, 2.05);
  glass.rotation.x = -0.55;
  group.add(glass);

  for (const side of [1, -1]) {
    const strut = new THREE.Mesh(new THREE.BoxGeometry(0.22, 2.1, 0.22), darkMat);
    strut.position.set(side * 1.95, 1.0, 0.35);
    strut.rotation.z = side * -0.75;
    group.add(strut);
  }

  const tail = new THREE.Mesh(new THREE.BoxGeometry(5.6, 0.36, 1.9), mat);
  tail.position.set(0, 0.55, -4.8);
  group.add(tail);

  const fin = new THREE.Mesh(new THREE.BoxGeometry(0.36, 2.3, 1.8), mat);
  fin.position.set(0, 1.55, -5.15);
  group.add(fin);

  for (const side of [1, -1]) {
    const leg = new THREE.Mesh(new THREE.CylinderGeometry(0.1, 0.1, 1.1, 8), darkMat);
    leg.position.set(side * 1.45, -1.2, 0.4);
    group.add(leg);

    const wheel = new THREE.Mesh(new THREE.CylinderGeometry(0.52, 0.52, 0.4, 14), darkMat);
    wheel.rotation.z = Math.PI / 2;
    wheel.position.set(side * 1.55, -1.72, 0.4);
    group.add(wheel);

    const spat = new THREE.Mesh(new THREE.CapsuleGeometry(0.4, 1.6, 4, 10), mat);
    spat.rotation.x = Math.PI / 2;
    spat.position.set(side * 1.75, -1.7, 0.4);
    group.add(spat);
  }

  const tailLeg = new THREE.Mesh(new THREE.CylinderGeometry(0.07, 0.07, 0.5, 8), darkMat);
  tailLeg.position.set(0, -0.55, -5.4);
  group.add(tailLeg);

  const tailWheel = new THREE.Mesh(new THREE.SphereGeometry(0.28, 10, 8), darkMat);
  tailWheel.position.set(0, -0.85, -5.4);
  group.add(tailWheel);

  return { group, tintMaterials: [mat], disposeMaterials: [mat, darkMat, propMat], scale: 0.9, wheelDrop: 2.0 };
}

// Big twin-engine jet: frontier models. Points along +Z.
function makeJetMesh() {
  const group = new THREE.Group();
  const mat = new THREE.MeshLambertMaterial({ color: 0xffffff });
  const darkMat = new THREE.MeshLambertMaterial({ color: 0x222831 });

  const fuselage = new THREE.Mesh(new THREE.CapsuleGeometry(1.7, 18, 6, 12), mat);
  fuselage.rotation.x = Math.PI / 2;
  fuselage.position.z = -1.5;
  group.add(fuselage);

  const nose = new THREE.Mesh(new THREE.ConeGeometry(1.7, 4, 12), mat);
  nose.rotation.x = Math.PI / 2;
  nose.position.z = 10.5;
  group.add(nose);

  const cockpit = new THREE.Mesh(new THREE.BoxGeometry(2.2, 0.7, 2.0), darkMat);
  cockpit.position.set(0, 1.0, 7.6);
  group.add(cockpit);

  for (const side of [1, -1]) {
    const wing = new THREE.Mesh(new THREE.BoxGeometry(12.5, 0.5, 3.8), mat);
    wing.position.set(side * 6.2, -0.7, -1.8);
    wing.rotation.y = side * -0.35;
    group.add(wing);

    const engine = new THREE.Mesh(new THREE.CylinderGeometry(1.05, 1.05, 3.4, 10), darkMat);
    engine.rotation.x = Math.PI / 2;
    engine.position.set(side * 5.0, -1.9, 0.4);
    group.add(engine);
  }

  const tail = new THREE.Mesh(new THREE.BoxGeometry(8.6, 0.4, 2.4), mat);
  tail.position.set(0, 1.1, -9.5);
  group.add(tail);

  const fin = new THREE.Mesh(new THREE.BoxGeometry(0.5, 5.2, 3.4), mat);
  fin.position.set(0, 3.2, -10.0);
  fin.rotation.x = -0.25;
  group.add(fin);

  return { group, tintMaterials: [mat], disposeMaterials: [mat, darkMat], scale: 0.74, wheelDrop: 2.2 };
}

// Sleek delta-wing fighter jet: sub-agent wingman! Points along +Z.
function makeFighterMesh() {
  const group = new THREE.Group();
  const mat = new THREE.MeshLambertMaterial({ color: 0x7ce8ff });
  const darkMat = new THREE.MeshLambertMaterial({ color: 0x1b2430 });
  const canopyMat = new THREE.MeshLambertMaterial({ color: 0x1e3852 });
  const burnerMat = new THREE.MeshBasicMaterial({ color: 0xff9d2e });

  // Needle nose + sleek fuselage.
  const nose = new THREE.Mesh(new THREE.ConeGeometry(1.05, 5.5, 8), mat);
  nose.rotation.x = Math.PI / 2;
  nose.position.z = 7.2;
  group.add(nose);

  const body = new THREE.Mesh(new THREE.CylinderGeometry(1.05, 1.25, 10.5, 8), mat);
  body.rotation.x = Math.PI / 2;
  body.position.z = -0.7;
  group.add(body);

  // Bubble canopy.
  const canopy = new THREE.Mesh(new THREE.CapsuleGeometry(0.68, 2.4, 6, 10), canopyMat);
  canopy.rotation.x = Math.PI / 2;
  canopy.position.set(0, 0.95, 2.2);
  group.add(canopy);

  // Swept delta wings (extruded flat triangle pair).
  const wingShape = new THREE.Shape();
  wingShape.moveTo(0, 3.2);
  wingShape.lineTo(6.6, -4.8);
  wingShape.lineTo(6.0, -5.6);
  wingShape.lineTo(0, -4.6);
  wingShape.lineTo(-6.0, -5.6);
  wingShape.lineTo(-6.6, -4.8);
  wingShape.closePath();
  const wingGeo = new THREE.ExtrudeGeometry(wingShape, { depth: 0.32, bevelEnabled: false });
  wingGeo.rotateX(Math.PI / 2); // XY -> XZ, +Y maps to +Z
  const wings = new THREE.Mesh(wingGeo, mat);
  wings.position.set(0, 0.1, 0.2);
  group.add(wings);

  // Forward canards for aggressive fighter silhouette.
  for (const side of [1, -1]) {
    const canard = new THREE.Mesh(new THREE.BoxGeometry(2.4, 0.22, 1.2), mat);
    canard.position.set(side * 1.9, 0.15, 4.0);
    canard.rotation.y = side * -0.4;
    group.add(canard);
  }

  // Twin canted vertical tail fins (F-22 / F-18 style).
  for (const side of [1, -1]) {
    const fin = new THREE.Mesh(new THREE.BoxGeometry(0.28, 3.2, 2.6), mat);
    fin.position.set(side * 1.4, 1.5, -4.6);
    fin.rotation.z = side * -0.32;
    fin.rotation.x = -0.25;
    group.add(fin);
  }

  // Twin nozzles + glowing afterburners.
  for (const side of [1, -1]) {
    const nozzle = new THREE.Mesh(new THREE.CylinderGeometry(0.42, 0.52, 1.1, 8), darkMat);
    nozzle.rotation.x = Math.PI / 2;
    nozzle.position.set(side * 0.55, -0.05, -6.2);
    group.add(nozzle);

    const flame = new THREE.Mesh(new THREE.ConeGeometry(0.38, 2.2, 8), burnerMat);
    flame.rotation.x = -Math.PI / 2;
    flame.position.set(side * 0.55, -0.05, -7.8);
    flame.name = 'afterburner';
    group.add(flame);
  }

  return {
    group,
    tintMaterials: [mat],
    disposeMaterials: [mat, darkMat, canopyMat, burnerMat],
    scale: 0.72,
    wheelDrop: 1.0,
  };
}

// MQ-9-style drone: monitoring / scheduled tasks. Points along +Z.
function makeDroneMesh() {
  const group = new THREE.Group();
  const mat = new THREE.MeshLambertMaterial({ color: 0xffffff });
  const darkMat = new THREE.MeshLambertMaterial({ color: 0x222831 });
  const rotorMat = new THREE.MeshLambertMaterial({
    color: 0x222831, transparent: true, opacity: 0.35, side: THREE.DoubleSide,
  });

  const nose = new THREE.Mesh(new THREE.ConeGeometry(1.4, 5, 8), mat);
  nose.rotation.x = Math.PI / 2;
  nose.position.z = 9;
  group.add(nose);

  const body = new THREE.Mesh(new THREE.CylinderGeometry(1.4, 1.1, 16, 8), mat);
  body.rotation.x = Math.PI / 2;
  body.position.z = -0.5;
  group.add(body);

  const turret = new THREE.Mesh(new THREE.SphereGeometry(1.1, 12, 12), darkMat);
  turret.position.set(0, -1.3, 6.5);
  group.add(turret);

  const wings = new THREE.Mesh(new THREE.BoxGeometry(24, 0.4, 2.6), mat);
  wings.position.set(0, 0.6, 1);
  group.add(wings);

  for (const side of [1, -1]) {
    const fin = new THREE.Mesh(new THREE.BoxGeometry(0.4, 5, 2.2), mat);
    fin.position.set(side * 1.6, 1.6, -7);
    fin.rotation.z = side * -0.6;
    group.add(fin);
  }

  const prop = new THREE.Mesh(new THREE.CircleGeometry(2.6, 16), rotorMat);
  prop.position.set(0, 0.4, -8.8);
  prop.name = 'prop';
  group.add(prop);

  return { group, tintMaterials: [mat], disposeMaterials: [mat, darkMat, rotorMat], scale: 0.76, wheelDrop: 1.8 };
}

function makeLabel(text, tint = 0xe8ffff) {
  const canvas = document.createElement('canvas');
  canvas.width = 256;
  canvas.height = 64;
  const ctx = canvas.getContext('2d');
  ctx.fillStyle = 'rgba(0, 18, 26, 0.62)';
  ctx.fillRect(0, 0, 256, 64);
  ctx.font = 'bold 30px monospace';
  ctx.fillStyle = '#' + new THREE.Color(tint).getHexString();
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText(text, 128, 34);
  const sprite = new THREE.Sprite(new THREE.SpriteMaterial({
    map: new THREE.CanvasTexture(canvas), transparent: true, depthTest: false,
  }));
  sprite.scale.set(24, 6, 1);
  sprite.position.y = 14;
  sprite.renderOrder = 10;
  return sprite;
}

function makeHoldRing(scale) {
  const ring = new THREE.Mesh(
    new THREE.RingGeometry(14 / scale, 17 / scale, 32),
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

  let built;
  if (a.kind === 'fighter') built = makeFighterMesh();
  else if (a.kind === 'drone') built = makeDroneMesh();
  else if (a.model_class === 'frontier') built = makeJetMesh();
  else built = makeAgentMesh();

  const s = built.scale;
  built.group.scale.setScalar(s);

  const isGateParked = a.callsign.startsWith('GT') || a.state === 'parked';
  const labelTint = a.kind === 'fighter'
    ? 0x7ce8ff
    : (isGateParked ? 0x8f9ba6 : (a.topic ? topicColor(a.topic) : 0xe8ffff));
  const label = makeLabel(a.callsign, labelTint);
  const lw = a.kind === 'fighter' ? 16 : 21;
  const lh = a.kind === 'fighter' ? 4 : 5.25;
  label.scale.set(lw / s, lh / s, 1);
  label.position.y = (a.kind === 'fighter' ? 9 : 12) / s;
  built.group.add(label);

  const ring = makeHoldRing(s);
  ring.visible = false;
  built.group.add(ring);

  // Server position is [x, z, altitude]; Three.js is (x, y=altitude, z).
  const onGround = ['landed', 'taxi', 'ready', 'parked'].includes(a.state);
  const groundY = onGround ? 2.2 + built.wheelDrop : 0;
  built.group.position.set(a.position[0], a.position[2] + groundY, a.position[1]);
  built.target = built.group.position.clone();
  built.heading = a.heading;
  built.targetHeading = a.heading;
  built.group.rotation.y = Math.PI / 2 - a.heading;
  built.state = a.state;
  built.kind = a.kind;
  built.ring = ring;

  scene.add(built.group);
  meshes.set(a.id, entry = built);
  return entry;
}

function setColor(entry, a) {
  let colorStr = stateColor(a.state);
  if (a.kind === 'fighter' && a.state === 'cruising') {
    colorStr = '#7ce8ff';
  }
  const hex = new THREE.Color(colorStr);
  for (const m of entry.tintMaterials) {
    m.color.copy(hex);
  }
}

function disposeMesh(id) {
  const entry = meshes.get(id);
  if (!entry) return;
  scene.remove(entry.group);
  entry.group.traverse((o) => {
    if (o.geometry) o.geometry.dispose();
    if (o.material && !entry.disposeMaterials.includes(o.material)) {
      if (o.material.map) o.material.map.dispose();
      o.material.dispose();
    }
  });
  for (const m of entry.disposeMaterials) m.dispose();
  meshes.delete(id);
}

/* --------------------------------------------------------------- ticker */

const tickerEl = document.getElementById('ticker-lines');
const pinnedHold = new Map();

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
    const first = tickerEl.firstElementChild;
    if (pinnedHold.has(first._aircraftId) && pinnedHold.get(first._aircraftId) === first) {
      tickerEl.appendChild(first);
      break;
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
const lastStates = new Map();

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
  ensureAirports(snapshot.airports || []);

  for (const a of aircraft) {
    seen.add(a.id);
    const entry = ensureMesh(a);
    const onGround = ['landed', 'taxi', 'ready', 'parked'].includes(a.state);
    const groundY = onGround ? 2.2 + entry.wheelDrop : 0;
    // Correct coordinate mapping: server [x, z, alt] -> Three.js (x, alt, z).
    entry.target.set(a.position[0], a.position[2] + groundY, a.position[1]);
    entry.targetHeading = a.heading;

    if (lastStates.get(a.id) !== a.state) {
      handleStateChange(a, lastStates.get(a.id), a.state);
      lastStates.set(a.id, a.state);
    }
    entry.state = a.state;
    setColor(entry, a);
    entry.ring.visible = a.state === 'holding';
  }

  for (const id of [...meshes.keys()]) {
    if (!seen.has(id)) {
      disposeMesh(id);
      lastStates.delete(id);
      unpinHold(id);
    }
  }

  const airportCount = (snapshot.airports || []).length;
  countEl.textContent = aircraft.length === 0
    ? `${airportCount} airports · airspace empty`
    : `${airportCount} airports · ${aircraft.length} aircraft`;

  const selectedId = selectedAircraftId();
  if (selectedId !== null) {
    const selected = aircraft.find((a) => a.id === selectedId);
    updateRadio(selected);
    if (selected === undefined) {
      closeRadio();
    }
  }
}

function handleStateChange(a, from, to) {
  const cs = a.callsign;
  if (a.kind === 'fighter' && from === undefined) {
    addTickerLine(`${cs}: fighter wingman on station.`);
    return;
  }
  if (to === 'holding') {
    addTickerLine(`${cs}: entering hold — awaiting approval`, true, a.id);
  } else {
    unpinHold(a.id);
    if (to === 'takeoff') addTickerLine(`${cs}: departing.`);
    else if (to === 'landing') addTickerLine(`${cs}: on approach.`);
    else if (to === 'landed') addTickerLine(`${cs}: landed, off frequency.`);
    else if (to === 'taxi') addTickerLine(`${cs}: taxiing.`);
    else if (to === 'parked') addTickerLine(`${cs}: parked at the gate.`);
    else if (to === 'ready') addTickerLine(`${cs}: lined up, ready for takeoff.`, true, a.id);
    else if (to === 'cruising' && from !== undefined) addTickerLine(`${cs}: cruising.`);
  }
  if (selectedAircraftId() === a.id && from !== undefined) {
    const note = {
      holding: 'entering hold',
      takeoff: 'taking off',
      landing: 'landing',
      landed: 'landed',
      taxi: 'taxiing',
      parked: 'parked at the gate',
      ready: 'ready for takeoff',
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
  if (dx * dx + dy * dy > 25) return;

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

const currentSnapshot = new Map();
const clock = new THREE.Clock();

function animate() {
  requestAnimationFrame(animate);
  const dt = clock.getDelta();
  const k = 1 - Math.exp(-INTERP_RATE * dt);

  const now = performance.now() / 1000;
  for (const entry of meshes.values()) {
    // Snap across map wrap jumps instead of lerping through the entire world.
    if (entry.group.position.distanceToSquared(entry.target) > 40000) {
      entry.group.position.copy(entry.target);
    } else {
      entry.group.position.lerp(entry.target, k);
    }

    let dh = entry.targetHeading - entry.heading;
    while (dh > Math.PI) dh -= 2 * Math.PI;
    while (dh < -Math.PI) dh += 2 * Math.PI;
    entry.heading += dh * k;
    // Server heading h moves along (cos h, sin h); mesh nose is +Z -> rotation.y = PI/2 - h.
    entry.group.rotation.y = Math.PI / 2 - entry.heading;

    // Spin prop around Z axis while engine is running (not parked/landed).
    const prop = entry.group.getObjectByName('prop');
    if (prop && entry.state !== 'parked' && entry.state !== 'landed') {
      prop.rotation.z = now * 30;
    }

    // Pulse fighter afterburners in flight.
    if (entry.kind === 'fighter') {
      const flying = entry.state !== 'parked' && entry.state !== 'landed';
      entry.group.traverse((o) => {
        if (o.name === 'afterburner') {
          o.visible = flying;
          if (flying) {
            o.scale.set(1, 0.8 + 0.35 * Math.sin(now * 24), 1);
          }
        }
      });
    }

    if (entry.ring.visible) {
      const pulse = 1 + 0.25 * Math.sin(now * 4);
      entry.ring.scale.setScalar(pulse);
      entry.ring.position.y = (0.8 - entry.group.position.y) / entry.scale;
      entry.ring.material.opacity = 0.5 + 0.35 * Math.sin(now * 4);
    }
  }

  for (const radar of radars) {
    radar.rotation.y = now * 0.8;
  }

  for (const ap of airportGroups.values()) {
    const pulse = ap.online ? 0.85 + 0.3 * Math.sin(now * 3) : 0.7;
    ap.beacon.scale.setScalar(pulse);
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
