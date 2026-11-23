// Skywatch browser client: Three.js scene, WS snapshot/delta consumption,
// smooth aircraft interpolation, device airports, detailed aircraft models,
// fighter wingmen, task labels, picking, radio panel, and ambient ticker.

import * as THREE from './vendor/three.module.js';
import { OrbitControls } from './vendor/OrbitControls.js';
import {
  setupRadio, openRadio, closeRadio, updateRadio, selectedAircraftId,
  appendHistory, stateColor,
} from './radio.js';

const WORLD_SIZE = 700;
const INTERP_RATE = 10; // exponential smoothing constant (per second)
const TARMAC_TOP_Y = 3.2; // elevated runway/apron deck height (well above sand y=2.0)

/* ---------------------------------------------------------------- scene */

const canvas = document.getElementById('scene');
const renderer = new THREE.WebGLRenderer({ canvas, antialias: true });
renderer.setSize(window.innerWidth, window.innerHeight);
renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
renderer.toneMapping = THREE.ACESFilmicToneMapping;
renderer.toneMappingExposure = 1.08;

const scene = new THREE.Scene();
// Sky gradient background.
{
  const skyCanvas = document.createElement('canvas');
  skyCanvas.width = 1;
  skyCanvas.height = 256;
  const ctx = skyCanvas.getContext('2d');
  const grad = ctx.createLinearGradient(0, 0, 0, 256);
  grad.addColorStop(0, '#09132b');
  grad.addColorStop(0.58, '#123a5c');
  grad.addColorStop(1, '#2b6e8c');
  ctx.fillStyle = grad;
  ctx.fillRect(0, 0, 1, 256);
  const tex = new THREE.CanvasTexture(skyCanvas);
  tex.colorSpace = THREE.SRGBColorSpace;
  scene.background = tex;
}
scene.fog = new THREE.Fog(0x123a5c, 500, 1550);

const camera = new THREE.PerspectiveCamera(
  50, window.innerWidth / window.innerHeight, 2, 3000,
);
camera.position.set(0, 410, 355);
camera.lookAt(0, 20, 0);

const controls = new OrbitControls(camera, canvas);
controls.target.set(0, 20, 0);
controls.enableDamping = true;
controls.dampingFactor = 0.08;
controls.maxPolarAngle = Math.PI / 2 - 0.05;
controls.minDistance = 35;
controls.maxDistance = 950;

// Three-point sky/sea lighting so curved aircraft fuselages and wings pop.
scene.add(new THREE.HemisphereLight(0x9ad4ff, 0x16384a, 0.85));
scene.add(new THREE.AmbientLight(0x556b84, 0.35));
const sun = new THREE.DirectionalLight(0xfff7ea, 1.55);
sun.position.set(280, 480, 220);
scene.add(sun);
const rim = new THREE.DirectionalLight(0x58b8e8, 0.45);
rim.position.set(-260, 180, -240);
scene.add(rim);

// Sea + radar grid.
{
  const sea = new THREE.Mesh(
    new THREE.PlaneGeometry(WORLD_SIZE + 260, WORLD_SIZE + 260),
    new THREE.MeshStandardMaterial({ color: 0x0c2938, roughness: 0.75, metalness: 0.15 }),
  );
  sea.rotation.x = -Math.PI / 2;
  sea.position.y = 0;
  scene.add(sea);

  const grid = new THREE.GridHelper(WORLD_SIZE, 35, 0x1f6f8f, 0x14455c);
  grid.position.y = 0.35;
  scene.add(grid);
}

/* ----------------------------------------------------------- airports */

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

// Raised concrete/asphalt deck slab whose top sits cleanly at `topY` —
// zero Z-fighting against the island sand (y=2.0) and guaranteed above terrain.
function makeDeckSlab(w, l, topY, thickness, mat) {
  const mesh = new THREE.Mesh(new THREE.BoxGeometry(w, thickness, l), mat);
  mesh.position.y = topY - thickness / 2;
  return mesh;
}

// Crisp painted tarmac marking hovering slightly above the deck with polygon offset.
function makeMarking(w, l, x, z, y, mat) {
  const mesh = new THREE.Mesh(new THREE.PlaneGeometry(w, l), mat);
  mesh.rotation.x = -Math.PI / 2;
  mesh.position.set(x, y, z);
  return mesh;
}

function makePalm(seed) {
  const trunkMat = new THREE.MeshStandardMaterial({ color: 0x6b4f2f, roughness: 0.9 });
  const leafMat = new THREE.MeshStandardMaterial({ color: 0x2f8f4a, roughness: 0.8 });
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

const airportGroups = new Map();
const radars = [];

function buildHomeAirport(ap) {
  const island = new THREE.Group();
  const sandMat = new THREE.MeshStandardMaterial({ color: 0xd6c084, roughness: 0.95 });
  const grassMat = new THREE.MeshStandardMaterial({ color: 0x3f7a45, roughness: 0.9 });
  const grassDarkMat = new THREE.MeshStandardMaterial({ color: 0x35683b, roughness: 0.9 });
  const shoulderMat = new THREE.MeshStandardMaterial({ color: 0x565d65, roughness: 0.9 });
  const asphaltMat = new THREE.MeshStandardMaterial({ color: 0x262b32, roughness: 0.85 });
  const apronMat = new THREE.MeshStandardMaterial({ color: 0x394049, roughness: 0.85 });
  const terminalMat = new THREE.MeshStandardMaterial({ color: 0x4c6070, roughness: 0.6, metalness: 0.2 });
  const glassMat = new THREE.MeshStandardMaterial({ color: 0x7fd4f0, roughness: 0.2, metalness: 0.45 });
  const lineMat = new THREE.MeshBasicMaterial({
    color: 0xe6eae0, polygonOffset: true, polygonOffsetFactor: -2,
  });
  const yellowLineMat = new THREE.MeshBasicMaterial({
    color: 0xffc857, polygonOffset: true, polygonOffsetFactor: -2,
  });

  // Broad sandy island plateau centered at (16, -6) with radius ~136 so the
  // runway, apron, and terminal sit 100% on dry land with wide beach margins.
  const beach = extrudeFlat(coastShape(136, 10, 0), 10, 2.0, sandMat);
  beach.position.x = 16;
  beach.position.z = -6;
  island.add(beach);

  // Grass plateau + tower hill strictly on the west side (x <= 26), leaving
  // 30 full units of clearance before the runway west edge (x = 56).
  const grass1 = extrudeFlat(coastShape(56, 7, 0), 4, 3.4, grassMat);
  grass1.position.set(-38, 0, -4);
  island.add(grass1);

  const grass2 = extrudeFlat(coastShape(36, 5, 0), 3, 4.6, grassDarkMat);
  grass2.position.set(-38, 0, -4);
  island.add(grass2);

  const hill = new THREE.Mesh(
    new THREE.SphereGeometry(30, 24, 12, 0, Math.PI * 2, 0, Math.PI / 2),
    grassDarkMat,
  );
  hill.scale.y = 0.35;
  hill.position.set(-38, 5.9, -4);
  island.add(hill);

  // Control Tower on the hill.
  const towerMat = new THREE.MeshStandardMaterial({ color: 0x25333d, roughness: 0.6, metalness: 0.25 });
  const base = new THREE.Mesh(new THREE.CylinderGeometry(3.8, 5.5, 24, 12), towerMat);
  base.position.set(-38, 26, -4);
  island.add(base);

  const cabin = new THREE.Mesh(new THREE.CylinderGeometry(10, 7.5, 6.5, 12), glassMat);
  cabin.position.set(-38, 40.5, -4);
  island.add(cabin);

  const roof = new THREE.Mesh(
    new THREE.ConeGeometry(11, 4.5, 12),
    new THREE.MeshStandardMaterial({ color: 0x1d2830, roughness: 0.7 }),
  );
  roof.position.set(-38, 46, -4);
  island.add(roof);

  const radar = new THREE.Mesh(new THREE.BoxGeometry(8.5, 0.6, 1), towerMat);
  radar.position.set(-38, 49.5, -4);
  island.add(radar);
  radars.push(radar);

  const beaconMat = new THREE.MeshBasicMaterial({ color: 0x4dff9e });
  const beacon = new THREE.Mesh(new THREE.SphereGeometry(1.4, 10, 10), beaconMat);
  beacon.position.set(-38, 51.5, -4);
  island.add(beacon);

  // Palms strictly on the grass half (x <= 0).
  const palmSpots = [[-78, 14], [-72, -32], [-18, 36], [-20, -42], [-50, 42], [-84, -8]];
  palmSpots.forEach(([px, pz], idx) => {
    const palm = makePalm(idx + 1);
    palm.position.set(px, 3.4, pz);
    island.add(palm);
  });

  // Raised concrete foundation pad under runway + taxiways + apron (y top = 3.0).
  const foundation = makeDeckSlab(74, 134, 3.0, 1.4, shoulderMat);
  foundation.position.x = 84;
  foundation.position.z = -7;
  island.add(foundation);

  // Elevated runway deck: 20 wide x 112 long at [66, -10], top at TARMAC_TOP_Y (3.2).
  const runway = makeDeckSlab(20, 112, TARMAC_TOP_Y, 0.4, asphaltMat);
  runway.position.x = 66;
  runway.position.z = -10;
  island.add(runway);

  // Runway edge lines, centreline dashes & threshold piano keys.
  const markY = TARMAC_TOP_Y + 0.08;
  for (const side of [-9.0, 9.0]) {
    island.add(makeMarking(0.65, 108, 66 + side, -10, markY, lineMat));
  }
  for (let i = 0; i < 9; i++) {
    island.add(makeMarking(1.1, 5.8, 66, -50 + i * 10, markY, lineMat));
  }
  for (const endZ of [-60, 40]) {
    for (let k = -2; k <= 2; k++) {
      if (k === 0) continue;
      island.add(makeMarking(1.3, 5.0, 66 + k * 3.0, endZ, markY, lineMat));
    }
  }

  // Elevated terminal apron deck, top at TARMAC_TOP_Y - 0.05 (3.15).
  const apron = makeDeckSlab(38, 126, TARMAC_TOP_Y - 0.05, 0.35, apronMat);
  apron.position.x = 97;
  apron.position.z = -5;
  island.add(apron);

  // Taxiway lead-in yellow stripe between runway and gates.
  island.add(makeMarking(0.8, 112, 81, -5, markY, yellowLineMat));
  for (const tz of [-50, 36]) {
    island.add(makeMarking(15, 0.8, 73.5, tz, markY, yellowLineMat));
  }

  // Terminal building along the east edge of the apron.
  const terminal = new THREE.Mesh(new THREE.BoxGeometry(10, 6.8, 112), terminalMat);
  terminal.position.set(114, TARMAC_TOP_Y + 3.4, -5);
  island.add(terminal);
  const terminalGlass = new THREE.Mesh(new THREE.BoxGeometry(0.6, 2.8, 106), glassMat);
  terminalGlass.position.set(108.8, TARMAC_TOP_Y + 4.1, -5);
  island.add(terminalGlass);

  // Gate stand markings + jetbridges for each gate.
  const bridgeMat = new THREE.MeshStandardMaterial({ color: 0x9aa9b5, roughness: 0.5, metalness: 0.3 });
  for (const [gx, gz] of ap.gates) {
    island.add(makeMarking(15, 0.8, gx, gz, markY, yellowLineMat));
    island.add(makeMarking(0.8, 8, gx + 5, gz, markY, yellowLineMat));

    const bridge = new THREE.Mesh(new THREE.BoxGeometry(7.5, 1.5, 1.8), bridgeMat);
    bridge.position.set(gx + 9.5, TARMAC_TOP_Y + 2.8, gz - 3.5);
    island.add(bridge);
  }

  // Airport name banner above the tower.
  const label = makeBannerLabel(ap.name.toUpperCase(), 0x5be7ff);
  label.position.set(-38, 63, -4);
  label.scale.set(42, 10.5, 1);
  island.add(label);

  scene.add(island);
  return { group: island, beacon, online: ap.online };
}

function buildSatelliteAirport(ap, idx) {
  const island = new THREE.Group();
  const [cx, cz] = ap.center;
  const seed = (idx + 1) * 2.17;
  const sandMat = new THREE.MeshStandardMaterial({ color: 0xcab67c, roughness: 0.95 });
  const grassMat = new THREE.MeshStandardMaterial({ color: 0x3b7243, roughness: 0.9 });
  const shoulderMat = new THREE.MeshStandardMaterial({ color: 0x565d65, roughness: 0.9 });
  const asphaltMat = new THREE.MeshStandardMaterial({ color: 0x262b32, roughness: 0.85 });
  const apronMat = new THREE.MeshStandardMaterial({ color: 0x394049, roughness: 0.85 });
  const terminalMat = new THREE.MeshStandardMaterial({ color: 0x4c6070, roughness: 0.6, metalness: 0.2 });
  const glassMat = new THREE.MeshStandardMaterial({ color: 0x7fd4f0, roughness: 0.2, metalness: 0.45 });
  const lineMat = new THREE.MeshBasicMaterial({
    color: 0xe6eae0, polygonOffset: true, polygonOffsetFactor: -2,
  });
  const yellowLineMat = new THREE.MeshBasicMaterial({
    color: 0xffc857, polygonOffset: true, polygonOffsetFactor: -2,
  });

  // Generous sandy island (min radius >= 66) so runway, apron, and terminal
  // stay 100% on solid land with beach margins on every side.
  const beach = extrudeFlat(coastShape(74, 5, seed), 8, 2.0, sandMat);
  beach.position.set(2, 0, 0);
  island.add(beach);

  // Grass cap strictly west of the runway (center x = -36, max radius ~22 ->
  // eastern edge x <= -14, well clear of runway foundation left edge x = -6).
  const grass = extrudeFlat(coastShape(19, 2.5, seed), 3, 3.1, grassMat);
  grass.position.set(-36, 0, 0);
  island.add(grass);

  // Mini control tower on the grass hillock.
  const towerMat = new THREE.MeshStandardMaterial({ color: 0x25333d, roughness: 0.6, metalness: 0.25 });
  const base = new THREE.Mesh(new THREE.CylinderGeometry(2.6, 3.8, 16, 10), towerMat);
  base.position.set(-36, 11, 0);
  island.add(base);

  const cabin = new THREE.Mesh(new THREE.CylinderGeometry(6.5, 5, 4.5, 10), glassMat);
  cabin.position.set(-36, 21, 0);
  island.add(cabin);

  const beaconMat = new THREE.MeshBasicMaterial({
    color: ap.online ? 0x4dff9e : 0xff5a5a,
  });
  const beacon = new THREE.Mesh(new THREE.SphereGeometry(1.4, 10, 10), beaconMat);
  beacon.position.set(-36, 25.5, 0);
  island.add(beacon);

  // Raised foundation slab for runway + apron (top at 3.0).
  const foundation = makeDeckSlab(50, 82, 3.0, 1.4, shoulderMat);
  foundation.position.set(17, foundation.position.y, 0);
  island.add(foundation);

  // Elevated mini runway: 15 wide x 74 long at x = 4, top at TARMAC_TOP_Y (3.2).
  const runway = makeDeckSlab(15, 74, TARMAC_TOP_Y, 0.4, asphaltMat);
  runway.position.x = 4;
  island.add(runway);

  const markY = TARMAC_TOP_Y + 0.08;
  for (const side of [-6.7, 6.7]) {
    island.add(makeMarking(0.5, 70, 4 + side, 0, markY, lineMat));
  }
  for (let i = 0; i < 6; i++) {
    island.add(makeMarking(0.9, 4.6, 4, -27 + i * 10.5, markY, lineMat));
  }
  for (const endZ of [-33, 33]) {
    for (const k of [-2, -1, 1, 2]) {
      island.add(makeMarking(0.95, 3.6, 4 + k * 2.2, endZ, markY, lineMat));
    }
  }

  // Elevated apron + small terminal + 4 gates.
  const apron = makeDeckSlab(24, 70, TARMAC_TOP_Y - 0.05, 0.35, apronMat);
  apron.position.x = 25;
  island.add(apron);

  const terminal = new THREE.Mesh(new THREE.BoxGeometry(7, 5.2, 60), terminalMat);
  terminal.position.set(37, TARMAC_TOP_Y + 2.6, 0);
  island.add(terminal);
  const tGlass = new THREE.Mesh(new THREE.BoxGeometry(0.5, 2.0, 54), glassMat);
  tGlass.position.set(33.3, TARMAC_TOP_Y + 3.0, 0);
  island.add(tGlass);

  for (const [gx, gz] of ap.gates) {
    island.add(makeMarking(10, 0.7, gx - cx + 2, gz - cz, markY, yellowLineMat));
  }

  // Device name banner (green-cyan when online, muted red when offline).
  const tint = ap.online ? 0x6ef3c5 : 0xd97373;
  const suffix = ap.online ? '' : ' (OFF)';
  const label = makeBannerLabel(`${ap.name.toUpperCase()}${suffix}`, tint);
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

    const label = makeBannerLabel(s.name.toUpperCase(), color);
    label.position.set(s.center[0], 20, s.center[1]);
    label.scale.set(36, 9, 1);
    scene.add(label);
  }
}

/* ------------------------------------------------------ aircraft meshes */

const meshes = new Map();

// Helper: create an extruded flat airfoil/wing shape in the XZ plane (shape XY -> XZ, +Y -> +Z).
function makePlanform(points, thickness, mat) {
  const shape = new THREE.Shape();
  points.forEach(([x, z], idx) => {
    if (idx === 0) shape.moveTo(x, z);
    else shape.lineTo(x, z);
  });
  shape.closePath();
  const geo = new THREE.ExtrudeGeometry(shape, {
    depth: thickness, bevelEnabled: true, bevelThickness: thickness * 0.25,
    bevelSize: thickness * 0.2, bevelSegments: 2,
  });
  geo.rotateX(Math.PI / 2); // XY -> XZ with +Y mapping to +Z (forward)
  geo.translate(0, thickness * 0.5, 0);
  return new THREE.Mesh(geo, mat);
}

// Helper: extruded vertical fin in the YZ plane (shape X = Z forward, Y = up).
function makeVerticalFin(points, thickness, mat) {
  const shape = new THREE.Shape();
  points.forEach(([z, y], idx) => {
    if (idx === 0) shape.moveTo(z, y);
    else shape.lineTo(z, y);
  });
  shape.closePath();
  const geo = new THREE.ExtrudeGeometry(shape, {
    depth: thickness, bevelEnabled: true, bevelThickness: thickness * 0.2,
    bevelSize: thickness * 0.15, bevelSegments: 2,
  });
  geo.rotateY(-Math.PI / 2); // local +X -> +Z, local +Y -> +Y
  geo.translate(thickness * 0.5, 0, 0);
  return new THREE.Mesh(geo, mat);
}

// Wingtip navigation lights (port red, starboard green).
function addNavLights(group, wingX, wingY, wingZ, disposeMaterials) {
  const redMat = new THREE.MeshBasicMaterial({ color: 0xff3b3b });
  const greenMat = new THREE.MeshBasicMaterial({ color: 0x3bff7c });
  const port = new THREE.Mesh(new THREE.SphereGeometry(0.26, 8, 8), redMat);
  port.position.set(-wingX, wingY, wingZ);
  const stbd = new THREE.Mesh(new THREE.SphereGeometry(0.26, 8, 8), greenMat);
  stbd.position.set(wingX, wingY, wingZ);
  group.add(port, stbd);
  disposeMaterials.push(redMat, greenMat);
}

// Little private plane (single prop, Cessna/Cirrus style): local models. Points along +Z.
function makeAgentMesh() {
  const group = new THREE.Group();
  const bodyMat = new THREE.MeshStandardMaterial({ color: 0xffffff, roughness: 0.38, metalness: 0.15 });
  const accentMat = new THREE.MeshStandardMaterial({ color: 0x2b7cb3, roughness: 0.4, metalness: 0.2 });
  const darkMat = new THREE.MeshStandardMaterial({ color: 0x1e2630, roughness: 0.45, metalness: 0.35 });
  const glassMat = new THREE.MeshStandardMaterial({ color: 0x17314a, roughness: 0.12, metalness: 0.7 });
  const propMat = new THREE.MeshBasicMaterial({
    color: 0x1c2229, transparent: true, opacity: 0.28, side: THREE.DoubleSide,
  });

  // Sculpted lathe fuselage (nose -> cowl -> cabin -> tapered tailcone).
  const profile = [
    new THREE.Vector2(0.01, 6.7),
    new THREE.Vector2(0.55, 6.2),
    new THREE.Vector2(1.05, 5.2),
    new THREE.Vector2(1.38, 3.6),
    new THREE.Vector2(1.42, 0.8),
    new THREE.Vector2(1.18, -1.6),
    new THREE.Vector2(0.68, -4.4),
    new THREE.Vector2(0.32, -6.2),
    new THREE.Vector2(0.01, -6.6),
  ];
  const fuselageGeo = new THREE.LatheGeometry(profile, 18);
  fuselageGeo.rotateX(Math.PI / 2); // Lathe Y -> +Z (nose at +6.7)
  const fuselage = new THREE.Mesh(fuselageGeo, bodyMat);
  group.add(fuselage);

  // Subtle side livery cheatline.
  for (const side of [1, -1]) {
    const stripe = new THREE.Mesh(new THREE.BoxGeometry(0.08, 0.28, 7.6), accentMat);
    stripe.position.set(side * 1.36, 0.15, 0.2);
    group.add(stripe);
  }

  // Spinner cone + 3-blade tractor propeller.
  const spinner = new THREE.Mesh(new THREE.ConeGeometry(0.52, 1.25, 14), accentMat);
  spinner.rotation.x = Math.PI / 2;
  spinner.position.z = 7.15;
  group.add(spinner);

  const prop = new THREE.Group();
  const disc = new THREE.Mesh(new THREE.CircleGeometry(2.35, 20), propMat);
  prop.add(disc);
  for (let i = 0; i < 3; i++) {
    const blade = new THREE.Mesh(new THREE.BoxGeometry(0.24, 2.25, 0.08), darkMat);
    const a = (i / 3) * Math.PI * 2;
    blade.position.set(Math.cos(a) * 1.1, Math.sin(a) * 1.1, 0);
    blade.rotation.z = a - Math.PI / 2;
    prop.add(blade);
  }
  prop.position.z = 7.25;
  prop.name = 'prop';
  group.add(prop);

  // Fairing cabin roof + wrap-around cockpit glazing.
  const cabinRoof = new THREE.Mesh(new THREE.BoxGeometry(1.75, 0.95, 3.2), bodyMat);
  cabinRoof.position.set(0, 1.35, 0.6);
  group.add(cabinRoof);

  const windscreen = new THREE.Mesh(new THREE.BoxGeometry(1.62, 0.85, 0.18), glassMat);
  windscreen.position.set(0, 1.35, 2.45);
  windscreen.rotation.x = -0.68;
  group.add(windscreen);

  for (const side of [1, -1]) {
    const sideWin = new THREE.Mesh(new THREE.BoxGeometry(0.14, 0.62, 2.7), glassMat);
    sideWin.position.set(side * 0.92, 1.15, 0.55);
    group.add(sideWin);
  }

  // Tapered high wings with dihedral & rounded wingtips.
  for (const side of [1, -1]) {
    const wPts = side > 0
      ? [[0.4, 1.4], [7.6, 0.9], [8.0, 0.4], [7.6, -0.7], [0.4, -1.3]]
      : [[-0.4, 1.4], [-7.6, 0.9], [-8.0, 0.4], [-7.6, -0.7], [-0.4, -1.3]];
    const wing = makePlanform(wPts, 0.34, bodyMat);
    wing.position.set(0, 1.82, 0.3);
    wing.rotation.z = side * 0.05;
    group.add(wing);

    // Wing strut.
    const strut = new THREE.Mesh(new THREE.CylinderGeometry(0.08, 0.08, 2.9, 6), darkMat);
    strut.position.set(side * 2.45, 0.85, 0.4);
    strut.rotation.z = side * -0.92;
    group.add(strut);
  }

  // Tapered horizontal tailplane & swept vertical fin.
  const hTail = makePlanform([
    [0, 0.6], [3.1, -0.1], [3.0, -0.95], [0, -0.85],
    [-3.0, -0.95], [-3.1, -0.1],
  ], 0.24, bodyMat);
  hTail.position.set(0, 0.35, -5.2);
  group.add(hTail);

  const fin = makeVerticalFin([
    [-3.8, 0.35], [-5.3, 3.1], [-6.35, 3.0], [-6.2, 0.2],
  ], 0.26, accentMat);
  group.add(fin);

  // Fixed tricycle landing gear with wheel pants.
  for (const side of [1, -1]) {
    const leg = new THREE.Mesh(new THREE.CylinderGeometry(0.09, 0.09, 1.35, 8), darkMat);
    leg.position.set(side * 1.45, -1.25, 0.2);
    leg.rotation.z = side * 0.25;
    group.add(leg);

    const spat = new THREE.Mesh(new THREE.CapsuleGeometry(0.36, 1.35, 6, 10), bodyMat);
    spat.rotation.x = Math.PI / 2;
    spat.position.set(side * 1.65, -1.82, 0.2);
    group.add(spat);
  }
  const noseLeg = new THREE.Mesh(new THREE.CylinderGeometry(0.08, 0.08, 1.2, 8), darkMat);
  noseLeg.position.set(0, -1.2, 4.1);
  const noseWheel = new THREE.Mesh(new THREE.CapsuleGeometry(0.28, 0.9, 6, 8), bodyMat);
  noseWheel.rotation.x = Math.PI / 2;
  noseWheel.position.set(0, -1.78, 4.1);
  group.add(noseLeg, noseWheel);

  const disposeMaterials = [bodyMat, accentMat, darkMat, glassMat, propMat];
  addNavLights(group, 7.9, 2.2, 0.6, disposeMaterials);

  return { group, tintMaterials: [bodyMat], disposeMaterials, scale: 0.92, wheelDrop: 2.05 };
}

// Twin-engine frontier airliner (A320 / 787 silhouette): frontier models. Points along +Z.
function makeJetMesh() {
  const group = new THREE.Group();
  const bodyMat = new THREE.MeshStandardMaterial({ color: 0xffffff, roughness: 0.32, metalness: 0.25 });
  const tailAccentMat = new THREE.MeshStandardMaterial({ color: 0x1e6da8, roughness: 0.35, metalness: 0.25 });
  const darkMat = new THREE.MeshStandardMaterial({ color: 0x1f2731, roughness: 0.4, metalness: 0.5 });
  const metalMat = new THREE.MeshStandardMaterial({ color: 0xb8c4cf, roughness: 0.25, metalness: 0.75 });
  const glassMat = new THREE.MeshStandardMaterial({ color: 0x102233, roughness: 0.1, metalness: 0.85 });

  // Sculpted continuous airliner fuselage (ogive nose -> cabin tube -> tapered tailcone).
  const profile = [
    new THREE.Vector2(0.01, 13.8),
    new THREE.Vector2(0.65, 13.0),
    new THREE.Vector2(1.25, 11.6),
    new THREE.Vector2(1.68, 9.6),
    new THREE.Vector2(1.78, 7.0),
    new THREE.Vector2(1.78, -5.0),
    new THREE.Vector2(1.45, -8.5),
    new THREE.Vector2(0.85, -11.4),
    new THREE.Vector2(0.28, -13.2),
    new THREE.Vector2(0.01, -13.5),
  ];
  const fuseGeo = new THREE.LatheGeometry(profile, 22);
  fuseGeo.rotateX(Math.PI / 2);
  const fuselage = new THREE.Mesh(fuseGeo, bodyMat);
  group.add(fuselage);

  // Curved wrap-around flight-deck windscreen.
  const visor = new THREE.Mesh(
    new THREE.CylinderGeometry(1.52, 1.66, 0.72, 16, 1, false, -Math.PI * 0.42, Math.PI * 0.84),
    glassMat,
  );
  visor.position.set(0, 0.55, 10.1);
  visor.rotation.x = 0.22;
  group.add(visor);

  // Swept tapered airliner wings + upturned blended winglets.
  for (const side of [1, -1]) {
    const s = side;
    const wing = makePlanform([
      [s * 1.1, 2.8],
      [s * 13.4, -3.5],
      [s * 13.4, -4.9],
      [s * 5.2, -3.2],
      [s * 1.1, -3.4],
    ], 0.44, bodyMat);
    wing.position.set(0, -0.45, 0.4);
    wing.rotation.z = s * 0.06;
    group.add(wing);

    // Blended winglet.
    const winglet = makeVerticalFin([
      [-3.1, -0.1], [-4.3, 2.1], [-4.95, 2.05], [-4.45, -0.1],
    ], 0.22, tailAccentMat);
    winglet.position.set(s * 13.25, 0.35, 0);
    winglet.rotation.z = s * -0.18;
    group.add(winglet);

    // Pylon + high-bypass turbofan nacelle + silver intake lip + dark fan face.
    const pylon = new THREE.Mesh(new THREE.BoxGeometry(0.42, 1.35, 2.6), metalMat);
    pylon.position.set(s * 5.2, -1.0, 0.55);
    group.add(pylon);

    const nacelle = new THREE.Mesh(new THREE.CylinderGeometry(1.05, 0.92, 3.6, 16), bodyMat);
    nacelle.rotation.x = Math.PI / 2;
    nacelle.position.set(s * 5.2, -1.85, 1.15);
    group.add(nacelle);

    const intakeRing = new THREE.Mesh(new THREE.TorusGeometry(0.96, 0.14, 10, 18), metalMat);
    intakeRing.position.set(s * 5.2, -1.85, 2.95);
    group.add(intakeRing);

    const fan = new THREE.Mesh(new THREE.CircleGeometry(0.88, 16), darkMat);
    fan.position.set(s * 5.2, -1.85, 2.88);
    group.add(fan);

    const exhaust = new THREE.Mesh(new THREE.ConeGeometry(0.65, 1.1, 12), darkMat);
    exhaust.rotation.x = -Math.PI / 2;
    exhaust.position.set(s * 5.2, -1.85, -1.0);
    group.add(exhaust);
  }

  // Swept horizontal tailplane.
  const hTail = makePlanform([
    [0, 1.1], [5.4, -1.4], [5.4, -2.5], [0, -1.7],
    [-5.4, -2.5], [-5.4, -1.4],
  ], 0.32, bodyMat);
  hTail.position.set(0, 0.55, -10.8);
  group.add(hTail);

  // Tall swept vertical tail fin with airline color.
  const vFin = makeVerticalFin([
    [-7.4, 1.1], [-11.5, 6.7], [-13.2, 6.5], [-12.2, 0.4],
  ], 0.42, tailAccentMat);
  group.add(vFin);

  const disposeMaterials = [bodyMat, tailAccentMat, darkMat, metalMat, glassMat];
  addNavLights(group, 13.4, 0.4, -3.5, disposeMaterials);

  return { group, tintMaterials: [bodyMat], disposeMaterials, scale: 0.72, wheelDrop: 2.15 };
}

// Sleek delta-wing fighter jet (F-22 / Rafale style): sub-agent wingman! Points along +Z.
function makeFighterMesh() {
  const group = new THREE.Group();
  const bodyMat = new THREE.MeshStandardMaterial({ color: 0x7ce8ff, roughness: 0.3, metalness: 0.45 });
  const darkMat = new THREE.MeshStandardMaterial({ color: 0x1a232e, roughness: 0.45, metalness: 0.55 });
  const canopyMat = new THREE.MeshStandardMaterial({ color: 0xc29838, roughness: 0.12, metalness: 0.85 });
  const burnerOuterMat = new THREE.MeshBasicMaterial({
    color: 0x38d9ff, transparent: true, opacity: 0.8,
  });
  const burnerCoreMat = new THREE.MeshBasicMaterial({ color: 0xfff0b3 });

  // Sharp needle radome + sleek chined fuselage.
  const profile = [
    new THREE.Vector2(0.01, 10.6),
    new THREE.Vector2(0.42, 8.8),
    new THREE.Vector2(0.95, 6.2),
    new THREE.Vector2(1.25, 3.0),
    new THREE.Vector2(1.32, -2.2),
    new THREE.Vector2(1.18, -6.2),
    new THREE.Vector2(0.01, -6.4),
  ];
  const fuseGeo = new THREE.LatheGeometry(profile, 14);
  fuseGeo.rotateX(Math.PI / 2);
  fuseGeo.scale(1.15, 0.78, 1.0); // wide sleek stealth cross-section
  const fuselage = new THREE.Mesh(fuseGeo, bodyMat);
  group.add(fuselage);

  // Gold-tinted stealth bubble canopy.
  const canopy = new THREE.Mesh(new THREE.CapsuleGeometry(0.65, 2.8, 8, 14), canopyMat);
  canopy.rotation.x = Math.PI / 2;
  canopy.scale.set(0.92, 0.75, 1.0);
  canopy.position.set(0, 0.88, 3.0);
  group.add(canopy);

  // Side engine air intakes.
  for (const side of [1, -1]) {
    const intake = new THREE.Mesh(new THREE.BoxGeometry(0.85, 0.72, 4.6), darkMat);
    intake.position.set(side * 1.4, -0.12, 1.1);
    group.add(intake);
  }

  // Cropped-delta swept wings + wingtip rails.
  const wings = makePlanform([
    [0, 3.4], [2.4, 2.2], [7.0, -3.2], [7.0, -4.6], [2.2, -4.2],
    [0, -4.8],
    [-2.2, -4.2], [-7.0, -4.6], [-7.0, -3.2], [-2.4, 2.2],
  ], 0.28, bodyMat);
  wings.position.set(0, -0.05, 0.1);
  group.add(wings);

  for (const side of [1, -1]) {
    const rail = new THREE.Mesh(new THREE.CylinderGeometry(0.1, 0.1, 2.5, 6), darkMat);
    rail.rotation.x = Math.PI / 2;
    rail.position.set(side * 7.05, -0.05, -3.6);
    group.add(rail);
  }

  // Forward swept canards.
  const canards = makePlanform([
    [0.9, 5.2], [3.2, 3.5], [3.0, 2.8], [0.9, 3.6],
  ], 0.18, bodyMat);
  const canardsL = makePlanform([
    [-0.9, 5.2], [-3.2, 3.5], [-3.0, 2.8], [-0.9, 3.6],
  ], 0.18, bodyMat);
  canards.position.y = 0.08;
  canardsL.position.y = 0.08;
  group.add(canards, canardsL);

  // Twin outward-canted swept vertical stabilizers (extruded trapezoidal fins).
  for (const side of [1, -1]) {
    const fin = makeVerticalFin([
      [-2.8, 0.3], [-5.2, 3.4], [-6.5, 3.2], [-5.9, 0.2],
    ], 0.24, bodyMat);
    fin.position.set(side * 1.35, 0.15, 0);
    fin.rotation.z = side * -0.34;
    group.add(fin);

    // Twin nozzles + layered shock-diamond afterburner plumes.
    const nozzle = new THREE.Mesh(new THREE.CylinderGeometry(0.45, 0.56, 1.3, 10), darkMat);
    nozzle.rotation.x = Math.PI / 2;
    nozzle.position.set(side * 0.62, -0.05, -6.5);
    group.add(nozzle);

    const burner = new THREE.Group();
    const outer = new THREE.Mesh(new THREE.ConeGeometry(0.44, 3.0, 10), burnerOuterMat);
    outer.rotation.x = -Math.PI / 2;
    outer.position.z = -1.5;
    const inner = new THREE.Mesh(new THREE.ConeGeometry(0.24, 1.8, 8), burnerCoreMat);
    inner.rotation.x = -Math.PI / 2;
    inner.position.z = -0.9;
    burner.add(outer, inner);
    burner.position.set(side * 0.62, -0.05, -7.1);
    burner.name = 'afterburner';
    group.add(burner);
  }

  const disposeMaterials = [bodyMat, darkMat, canopyMat, burnerOuterMat, burnerCoreMat];
  addNavLights(group, 7.05, 0.05, -3.9, disposeMaterials);

  return { group, tintMaterials: [bodyMat], disposeMaterials, scale: 0.72, wheelDrop: 1.05 };
}

// MQ-9 Reaper drone: monitoring / scheduled tasks. Points along +Z.
function makeDroneMesh() {
  const group = new THREE.Group();
  const bodyMat = new THREE.MeshStandardMaterial({ color: 0xd8e0e6, roughness: 0.45, metalness: 0.2 });
  const darkMat = new THREE.MeshStandardMaterial({ color: 0x1d252e, roughness: 0.45, metalness: 0.45 });
  const lensMat = new THREE.MeshBasicMaterial({ color: 0x35e8ff });
  const rotorMat = new THREE.MeshBasicMaterial({
    color: 0x1d252e, transparent: true, opacity: 0.3, side: THREE.DoubleSide,
  });

  // Slim fuselage tapering cleanly fore and aft.
  const profile = [
    new THREE.Vector2(0.01, 10.6),
    new THREE.Vector2(0.62, 9.6),
    new THREE.Vector2(1.08, 7.6),
    new THREE.Vector2(1.18, 4.0),
    new THREE.Vector2(1.02, -1.5),
    new THREE.Vector2(0.75, -7.5),
    new THREE.Vector2(0.42, -9.2),
    new THREE.Vector2(0.01, -9.5),
  ];
  const fuseGeo = new THREE.LatheGeometry(profile, 16);
  fuseGeo.rotateX(Math.PI / 2);
  const fuselage = new THREE.Mesh(fuseGeo, bodyMat);
  group.add(fuselage);

  // Iconic MQ-9 bulbous satcom dome on the forward upper fuselage.
  const dome = new THREE.Mesh(new THREE.SphereGeometry(1.38, 16, 12), bodyMat);
  dome.scale.set(0.86, 0.72, 2.35);
  dome.position.set(0, 0.55, 5.5);
  group.add(dome);

  // Dorsal turboprop air intake scoop at rear.
  const scoop = new THREE.Mesh(new THREE.BoxGeometry(0.75, 0.65, 2.2), darkMat);
  scoop.position.set(0, 1.05, -4.5);
  group.add(scoop);

  // Chin EO/IR gimbal sensor ball + glowing optical lens.
  const turret = new THREE.Mesh(new THREE.SphereGeometry(0.85, 14, 12), darkMat);
  turret.position.set(0, -1.15, 6.2);
  group.add(turret);
  const lens = new THREE.Mesh(new THREE.SphereGeometry(0.28, 10, 10), lensMat);
  lens.position.set(0, -1.25, 6.95);
  group.add(lens);

  // High-aspect-ratio tapered Reaper wings + underwing pods.
  const wings = makePlanform([
    [0, 1.5], [13.2, 0.2], [13.2, -0.75], [0, -0.95],
    [-13.2, -0.75], [-13.2, 0.2],
  ], 0.26, bodyMat);
  wings.position.set(0, 0.15, 0.4);
  group.add(wings);

  for (const side of [1, -1]) {
    const pod = new THREE.Mesh(new THREE.CapsuleGeometry(0.28, 1.6, 6, 8), darkMat);
    pod.rotation.x = Math.PI / 2;
    pod.position.set(side * 4.4, -0.45, 0.4);
    group.add(pod);
  }

  // Upward V-tail (signature MQ-9) + ventral fin.
  for (const side of [1, -1]) {
    const vfin = makeVerticalFin([
      [-6.5, 0.2], [-8.4, 3.8], [-9.4, 3.7], [-8.6, 0.2],
    ], 0.2, bodyMat);
    vfin.position.set(side * 0.4, 0.2, 0);
    vfin.rotation.z = side * -0.68;
    group.add(vfin);
  }
  const ventral = makeVerticalFin([
    [-6.8, -0.3], [-8.5, -2.0], [-9.2, -1.9], [-8.6, -0.3],
  ], 0.18, bodyMat);
  group.add(ventral);

  // 4-blade pusher propeller at the rear.
  const prop = new THREE.Group();
  const disc = new THREE.Mesh(new THREE.CircleGeometry(2.3, 18), rotorMat);
  prop.add(disc);
  for (let i = 0; i < 4; i++) {
    const blade = new THREE.Mesh(new THREE.BoxGeometry(0.2, 2.2, 0.08), darkMat);
    const a = (i / 4) * Math.PI * 2;
    blade.position.set(Math.cos(a) * 1.05, Math.sin(a) * 1.05, 0);
    blade.rotation.z = a - Math.PI / 2;
    prop.add(blade);
  }
  prop.position.set(0, 0.0, -9.6);
  prop.name = 'prop';
  group.add(prop);

  const disposeMaterials = [bodyMat, darkMat, lensMat, rotorMat];
  addNavLights(group, 13.2, 0.25, -0.2, disposeMaterials);

  return { group, tintMaterials: [bodyMat], disposeMaterials, scale: 0.78, wheelDrop: 1.9 };
}

/* --------------------------------------------------------------- labels */

// Clean banner sprite for airport and sector names.
function makeBannerLabel(text, tint = 0xe8ffff) {
  const c = document.createElement('canvas');
  c.width = 384;
  c.height = 72;
  const ctx = c.getContext('2d');
  ctx.fillStyle = 'rgba(4, 18, 28, 0.68)';
  ctx.fillRect(0, 0, c.width, c.height);
  ctx.font = 'bold 34px "IBM Plex Mono", monospace';
  ctx.fillStyle = '#' + new THREE.Color(tint).getHexString();
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText(text, c.width / 2, c.height / 2 + 1);
  const sprite = new THREE.Sprite(new THREE.SpriteMaterial({
    map: new THREE.CanvasTexture(c), transparent: true, depthTest: false,
  }));
  sprite.scale.set(36, 7, 1);
  sprite.renderOrder = 10;
  return sprite;
}

function formatTaskTitle(a) {
  const raw = (a.title || '').replace(/\*\*/g, '').replace(/\s+/g, ' ').trim();
  if (!raw) return a.callsign;
  const max = a.kind === 'fighter' ? 22 : 28;
  return raw.length > max ? raw.slice(0, max - 1).trimEnd() + '…' : raw;
}

function buildAircraftLabelTexture(a, accentHex) {
  const c = document.createElement('canvas');
  c.width = 512;
  c.height = 104;
  const ctx = c.getContext('2d');

  // Rounded dark pill background.
  const r = 14;
  ctx.beginPath();
  ctx.moveTo(r, 4);
  ctx.lineTo(c.width - r, 4);
  ctx.quadraticCurveTo(c.width - 4, 4, c.width - 4, r);
  ctx.lineTo(c.width - 4, c.height - r);
  ctx.quadraticCurveTo(c.width - 4, c.height - 4, c.width - r, c.height - 4);
  ctx.lineTo(r, c.height - 4);
  ctx.quadraticCurveTo(4, c.height - 4, 4, c.height - r);
  ctx.lineTo(4, r);
  ctx.quadraticCurveTo(4, 4, r, 4);
  ctx.closePath();
  ctx.fillStyle = a.state === 'parked' ? 'rgba(10, 18, 26, 0.68)' : 'rgba(6, 16, 26, 0.82)';
  ctx.fill();

  const accent = '#' + new THREE.Color(accentHex).getHexString();
  ctx.strokeStyle = accent;
  ctx.globalAlpha = a.state === 'parked' ? 0.45 : 0.72;
  ctx.lineWidth = 3;
  ctx.stroke();
  ctx.globalAlpha = 1;

  // Left accent bar.
  ctx.fillStyle = accent;
  ctx.fillRect(10, 16, 7, c.height - 32);

  // Primary line: the task title!
  const taskText = formatTaskTitle(a);
  ctx.font = 'bold 28px "IBM Plex Mono", ui-monospace, monospace';
  ctx.fillStyle = a.state === 'parked' ? '#b3bec8' : '#eef8ff';
  ctx.textAlign = 'left';
  ctx.textBaseline = 'middle';
  ctx.fillText(taskText, 28, 37);

  // Secondary line: callsign + role/state.
  const stateLabel = a.state === 'parked'
    ? 'AT GATE'
    : a.kind === 'fighter'
      ? 'WINGMAN'
      : a.state.toUpperCase();
  const sub = `${a.callsign} · ${stateLabel}`;
  ctx.font = 'bold 21px "IBM Plex Mono", ui-monospace, monospace';
  ctx.fillStyle = accent;
  ctx.fillText(sub, 28, 74);

  const tex = new THREE.CanvasTexture(c);
  tex.colorSpace = THREE.SRGBColorSpace;
  return tex;
}

function labelAccentFor(a) {
  if (a.state === 'holding') return 0xffb020;
  if (a.kind === 'fighter') return 0x7ce8ff;
  if (a.state === 'parked' || a.callsign.startsWith('GT')) return 0x8f9ba6;
  if (a.state === 'landed') return 0xff7b6b;
  if (a.topic) return topicColor(a.topic);
  return 0x5be7ff;
}

function updateAircraftLabel(entry, a) {
  const key = `${a.title || ''}|${a.state}|${a.topic || ''}|${a.callsign}`;
  if (entry.labelKey === key) return;
  entry.labelKey = key;
  const accent = labelAccentFor(a);
  const newTex = buildAircraftLabelTexture(a, accent);
  if (entry.label.material.map) entry.label.material.map.dispose();
  entry.label.material.map = newTex;
  entry.label.material.needsUpdate = true;
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

  const label = new THREE.Sprite(new THREE.SpriteMaterial({
    transparent: true, depthTest: false,
  }));
  const lw = a.kind === 'fighter' ? 28 : 38;
  const lh = lw * (104 / 512);
  label.scale.set(lw / s, lh / s, 1);
  label.position.y = (a.kind === 'fighter' ? 9.5 : 13) / s;
  label.renderOrder = 10;
  built.group.add(label);
  built.label = label;
  built.labelKey = null;
  updateAircraftLabel(built, a);

  const ring = makeHoldRing(s);
  ring.visible = false;
  built.group.add(ring);

  // Server position is [x, z, altitude]; Three.js is (x, y=altitude, z).
  const onGround = ['landed', 'taxi', 'ready', 'parked'].includes(a.state);
  const groundY = onGround ? TARMAC_TOP_Y + built.wheelDrop : 0;
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
    const groundY = onGround ? TARMAC_TOP_Y + entry.wheelDrop : 0;
    entry.target.set(a.position[0], a.position[2] + groundY, a.position[1]);
    entry.targetHeading = a.heading;

    if (lastStates.get(a.id) !== a.state) {
      handleStateChange(a, lastStates.get(a.id), a.state);
      lastStates.set(a.id, a.state);
    }
    entry.state = a.state;
    setColor(entry, a);
    updateAircraftLabel(entry, a);
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
  const tag = a.title ? `${a.callsign} (${formatTaskTitle(a)})` : a.callsign;
  if (a.kind === 'fighter' && from === undefined) {
    addTickerLine(`${tag}: fighter wingman on station.`);
    return;
  }
  if (to === 'holding') {
    addTickerLine(`${tag}: entering hold — awaiting approval`, true, a.id);
  } else {
    unpinHold(a.id);
    if (to === 'takeoff') addTickerLine(`${tag}: departing.`);
    else if (to === 'landing') addTickerLine(`${tag}: on approach.`);
    else if (to === 'landed') addTickerLine(`${tag}: landed, off frequency.`);
    else if (to === 'taxi') addTickerLine(`${tag}: taxiing.`);
    else if (to === 'parked') addTickerLine(`${tag}: parked at the gate.`);
    else if (to === 'ready') addTickerLine(`${tag}: lined up, ready for takeoff.`, true, a.id);
    else if (to === 'cruising' && from !== undefined) addTickerLine(`${tag}: cruising.`);
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
    if (entry.group.position.distanceToSquared(entry.target) > 40000) {
      entry.group.position.copy(entry.target);
    } else {
      entry.group.position.lerp(entry.target, k);
    }

    let dh = entry.targetHeading - entry.heading;
    while (dh > Math.PI) dh -= 2 * Math.PI;
    while (dh < -Math.PI) dh += 2 * Math.PI;
    entry.heading += dh * k;
    entry.group.rotation.y = Math.PI / 2 - entry.heading;

    const prop = entry.group.getObjectByName('prop');
    if (prop && entry.state !== 'parked' && entry.state !== 'landed') {
      prop.rotation.z = now * 30;
    }

    if (entry.kind === 'fighter') {
      const flying = entry.state !== 'parked' && entry.state !== 'landed';
      entry.group.traverse((o) => {
        if (o.name === 'afterburner') {
          o.visible = flying;
          if (flying) {
            const flicker = 0.82 + 0.32 * Math.sin(now * 28);
            o.scale.set(1, 1, flicker);
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
