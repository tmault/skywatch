// Skywatch browser client: Three.js scene, WS snapshot/delta consumption,
// smooth aircraft interpolation, device airports, detailed aircraft models,
// fighter wingmen, task labels, picking, radio panel, and ambient ticker.

import * as THREE from './vendor/three.module.js';
import { OrbitControls } from './vendor/OrbitControls.js';
import {
  setupRadio, openRadio, closeRadio, updateRadio, selectedAircraftId,
  appendHistory, stateColor,
} from './radio.js';
import {
  buildAlpineIsland, buildAlpineBackdrop, makeSwissFlag, updateAlpine, SWISS_RED,
  makeLakeMaterial, enableShadows,
} from './alpine.js';
import { addRunwayLights } from './runway-lights.js';
import { makeAgentMesh, makeJetMesh, makeFighterMesh, makeDroneMesh } from './aircraft.js';

const WORLD_SIZE = 820;
const INTERP_RATE = 10; // exponential smoothing constant (per second)
const TARMAC_TOP_Y = 3.2; // elevated runway/apron deck height (well above sand y=2.0)

/* ---------------------------------------------------------------- scene */

const canvas = document.getElementById('scene');
const renderer = new THREE.WebGLRenderer({ canvas, antialias: true });
renderer.setSize(window.innerWidth, window.innerHeight);
renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
renderer.toneMapping = THREE.ACESFilmicToneMapping;
renderer.toneMappingExposure = 1.08;
renderer.shadowMap.enabled = true;
renderer.shadowMap.type = THREE.PCFSoftShadowMap;

const scene = new THREE.Scene();
// Sky gradient background.
{
  const skyCanvas = document.createElement('canvas');
  skyCanvas.width = 1;
  skyCanvas.height = 256;
  const ctx = skyCanvas.getContext('2d');
  const grad = ctx.createLinearGradient(0, 0, 0, 256);
  // Crisp alpine day: deep zenith blue fading to a pale haze at the horizon.
  grad.addColorStop(0, '#2f6fb8');
  grad.addColorStop(0.45, '#6fa9dc');
  grad.addColorStop(0.8, '#b9d9ef');
  grad.addColorStop(1, '#dcecf6');
  ctx.fillStyle = grad;
  ctx.fillRect(0, 0, 1, 256);
  const tex = new THREE.CanvasTexture(skyCanvas);
  tex.colorSpace = THREE.SRGBColorSpace;
  scene.background = tex;
}
scene.fog = new THREE.Fog(0xc9e0ee, 760, 2300);

const camera = new THREE.PerspectiveCamera(
  50, window.innerWidth / window.innerHeight, 2, 4000,
);
camera.position.set(0, 410, 355);
camera.lookAt(0, 20, 0);

const controls = new OrbitControls(camera, canvas);
controls.target.set(0, 20, 0);
controls.enableDamping = true;
controls.dampingFactor = 0.08;
controls.maxPolarAngle = Math.PI / 2 - 0.05;
controls.minDistance = 18;
controls.maxDistance = 900;

// Alpine daylight: cool sky fill, meadow-green bounce, warm low sun.
scene.add(new THREE.HemisphereLight(0xcfe6ff, 0x3f5a34, 0.78));
scene.add(new THREE.AmbientLight(0x8094a8, 0.2));
// Mid-afternoon sun (~37° up) so peaks, trees and terminals throw long,
// readable shadows; aircraft shadows on the lake show their altitude.
const sun = new THREE.DirectionalLight(0xfff0d8, 1.85);
sun.position.set(360, 330, 240);
sun.castShadow = true;
sun.shadow.mapSize.set(4096, 4096);
{
  const cam = sun.shadow.camera; // covers the whole archipelago (±480)
  cam.left = -480;
  cam.right = 480;
  cam.top = 480;
  cam.bottom = -480;
  cam.near = 1;
  cam.far = 1200;
}
sun.shadow.bias = -0.0004;
sun.shadow.normalBias = 0.45;
sun.shadow.radius = 3;
scene.add(sun);
const rim = new THREE.DirectionalLight(0x9cc8f0, 0.4);
rim.position.set(-260, 180, -240);
scene.add(rim);

// Alpine lake, a faint Swiss-style layout grid, and the mountains around it.
{
  const lake = new THREE.Mesh(
    new THREE.CircleGeometry(1300, 96),
    makeLakeMaterial(),
  );
  lake.receiveShadow = true;
  lake.rotation.x = -Math.PI / 2;
  lake.position.y = 0;
  scene.add(lake);

  const grid = new THREE.GridHelper(WORLD_SIZE, 35, 0xffffff, 0xffffff);
  grid.material.transparent = true;
  grid.material.opacity = 0.1;
  grid.material.depthWrite = false;
  grid.position.y = 0.35;
  scene.add(grid);

  buildAlpineBackdrop(scene, 620, 1500);
}

/* ----------------------------------------------------------- airports */

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

const airportGroups = new Map();
const radars = [];

function buildHomeAirport(ap) {
  const island = new THREE.Group();
  const shoulderMat = new THREE.MeshStandardMaterial({ color: 0x565d65, roughness: 0.9 });
  const medianMat = new THREE.MeshStandardMaterial({ color: 0x4c823b, roughness: 0.95 });
  const asphaltMat = new THREE.MeshStandardMaterial({ color: 0x242930, roughness: 0.85 });
  const taxiMat = new THREE.MeshStandardMaterial({ color: 0x2c323b, roughness: 0.85 });
  const apronMat = new THREE.MeshStandardMaterial({ color: 0x3a414b, roughness: 0.85 });
  const pierWallMat = new THREE.MeshStandardMaterial({ color: 0x626b75, roughness: 0.85 });
  const terminalMat = new THREE.MeshStandardMaterial({ color: 0x4c6070, roughness: 0.6, metalness: 0.2 });
  const glassMat = new THREE.MeshStandardMaterial({ color: 0x7fd4f0, roughness: 0.2, metalness: 0.45 });
  const lineMat = new THREE.MeshLambertMaterial({
    color: 0xe6eae0, polygonOffset: true, polygonOffsetFactor: -2,
  });
  const yellowLineMat = new THREE.MeshLambertMaterial({
    color: 0xffc857, polygonOffset: true, polygonOffsetFactor: -2,
  });
  const redBoxMat = new THREE.MeshLambertMaterial({
    color: 0xd93829, polygonOffset: true, polygonOffsetFactor: -2,
  });

  // Alpine island: pebble shore, meadows and fir forest, the Tower on a
  // flat-topped summit (west), snowy peaks behind it, and flattened airfield
  // pads under Runway 36, parallel Taxiway Alpha, holding bay, and gate pier.
  const towerSpot = { x: -46, z: -4 };
  const flagSpot = { x: -34, z: 7 };
  const terrain = buildAlpineIsland(island, {
    seed: 1.3,
    center: [14, -4],
    radius: 138,
    hilliness: 4.5,
    snowLine: 17,
    pads: [
      { x: 70, z: -8, hw: 30, hl: 75 },  // Runway 36 + grass median + Taxiway Alpha
      { x: 123, z: 0, hw: 26, hl: 102 }, // Extended 10-gate concourse pier
    ],
    peaks: [
      { ...towerSpot, h: 15, r: 44, flat: 7, crag: 0.25 },
      { x: -90, z: 50, h: 32, r: 36, crag: 0.55 },
      { x: -78, z: -62, h: 19, r: 30, crag: 0.45 },
    ],
    keepClear: [{ ...towerSpot, r: 11 }, { ...flagSpot, r: 3 }],
    trees: 280,
    chalets: 6,
    rocks: 70,
  });

  const flag = makeSwissFlag(6, 13);
  flag.position.set(flagSpot.x, terrain.heightAt(flagSpot.x, flagSpot.z) - 0.3, flagSpot.z);
  island.add(flag);
  const apronFlag = makeSwissFlag(5, 12);
  apronFlag.position.set(140, TARMAC_TOP_Y, -95);
  island.add(apronFlag);

  // Control Tower on the hill.
  const towerMat = new THREE.MeshStandardMaterial({ color: 0x25333d, roughness: 0.6, metalness: 0.25 });
  const base = new THREE.Mesh(new THREE.CylinderGeometry(3.8, 5.5, 24, 12), towerMat);
  base.position.set(towerSpot.x, 26, towerSpot.z);
  island.add(base);

  const cabin = new THREE.Mesh(new THREE.CylinderGeometry(10, 7.5, 6.5, 12), glassMat);
  cabin.position.set(towerSpot.x, 40.5, towerSpot.z);
  island.add(cabin);

  const roof = new THREE.Mesh(
    new THREE.ConeGeometry(11, 4.5, 12),
    new THREE.MeshStandardMaterial({ color: 0x1d2830, roughness: 0.7 }),
  );
  roof.position.set(towerSpot.x, 46, towerSpot.z);
  island.add(roof);

  const radar = new THREE.Mesh(new THREE.BoxGeometry(8.5, 0.6, 1), towerMat);
  radar.position.set(towerSpot.x, 49.5, towerSpot.z);
  island.add(radar);
  radars.push(radar);

  const beaconMat = new THREE.MeshBasicMaterial({ color: 0x4dff9e });
  const beacon = new THREE.Mesh(new THREE.SphereGeometry(1.4, 10, 10), beaconMat);
  beacon.position.set(towerSpot.x, 51.5, towerSpot.z);
  island.add(beacon);

  // --- Runway 36/18 + Parallel Taxiway Alpha + South Departure Holding Bay ---
  const airfieldPad = makeDeckSlab(60, 148, 3.0, 2.2, shoulderMat);
  airfieldPad.position.x = 70;
  airfieldPad.position.z = -8;
  island.add(airfieldPad);

  // Mown grass safety median between Runway 36 (x=56) and Taxiway Alpha (x=84).
  const median = makeDeckSlab(11, 126, 3.08, 0.2, medianMat);
  median.position.x = 71.5;
  median.position.z = -10;
  island.add(median);

  // Elevated Runway 36/18: 20 wide x 134 long at [56, -10], top at TARMAC_TOP_Y (3.2).
  const runway = makeDeckSlab(20, 134, TARMAC_TOP_Y, 0.4, asphaltMat);
  runway.position.x = 56;
  runway.position.z = -10;
  island.add(runway);

  const lights = addRunwayLights(island, {
    x: 56, zMin: -76, zMax: 56, halfWidth: 10.6, y: TARMAC_TOP_Y,
    groundY: 1.8, spacing: 8, approachLength: 24,
  });
  lights.setOnline(ap.online);

  // Runway edge lines, centreline dashes & threshold piano keys.
  const markY = TARMAC_TOP_Y + 0.08;
  for (const side of [-9.0, 9.0]) {
    island.add(makeMarking(0.65, 130, 56 + side, -10, markY, lineMat));
  }
  for (let i = 0; i < 11; i++) {
    island.add(makeMarking(1.1, 6.0, 56, -58 + i * 10, markY, lineMat));
  }
  for (const endZ of [-70, 50]) {
    for (let k = -2; k <= 2; k++) {
      if (k === 0) continue;
      island.add(makeMarking(1.3, 5.4, 56 + k * 3.0, endZ, markY, lineMat));
    }
  }

  // Dedicated Parallel Taxiway Alpha: 14 wide x 134 long at [84, -8].
  const taxiway = makeDeckSlab(14, 134, TARMAC_TOP_Y, 0.4, taxiMat);
  taxiway.position.x = 84;
  taxiway.position.z = -8;
  island.add(taxiway);

  // Taxiway Alpha continuous yellow centerline + blue taxiway edge lights.
  island.add(makeMarking(0.85, 126, 84, -10, markY, yellowLineMat));
  const blueLightMat = new THREE.MeshBasicMaterial({ color: 0x38a6ff });
  const amberLightMat = new THREE.MeshBasicMaterial({ color: 0xffb830 });
  for (let tz = -68; tz <= 52; tz += 15) {
    for (const side of [-7.3, 7.3]) {
      const bL = new THREE.Mesh(new THREE.SphereGeometry(0.32, 6, 6), blueLightMat);
      bL.position.set(84 + side, markY + 0.3, tz);
      island.add(bL);
    }
  }

  // Three paved taxiway connectors linking Taxiway Alpha <-> Runway 36:
  // North exit (z=-66), Midfield (z=-10), and wide South Departure Holding Bay (z=46).
  for (const cz of [-66, -10]) {
    const conn = makeDeckSlab(16, 12, TARMAC_TOP_Y, 0.4, taxiMat);
    conn.position.set(71.5, conn.position.y, cz);
    island.add(conn);
    island.add(makeMarking(20, 0.8, 72, cz, markY, yellowLineMat));
  }

  // Wide South Departure Holding Bay at [74, 46] with hold-short bars & guard lights.
  const holdBay = makeDeckSlab(24, 22, TARMAC_TOP_Y, 0.4, taxiMat);
  holdBay.position.set(74, holdBay.position.y, 46);
  island.add(holdBay);
  island.add(makeMarking(24, 0.85, 72, 46, markY, yellowLineMat));
  island.add(makeMarking(0.65, 18, 68.2, 46, markY, yellowLineMat));
  island.add(makeMarking(0.65, 18, 69.3, 46, markY, yellowLineMat));
  island.add(makeMarking(3.6, 5.0, 72.5, 53.5, markY, redBoxMat));
  for (const gz of [37.5, 54.5]) {
    const guard = new THREE.Mesh(new THREE.SphereGeometry(0.55, 8, 8), amberLightMat);
    guard.position.set(68.8, markY + 0.55, gz);
    island.add(guard);
  }

  // --- Long Extending Gate Concourse Pier (10 gates for >24h idle threads) ---
  const pierWall = makeDeckSlab(50, 200, 3.0, 5.2, pierWallMat);
  pierWall.position.set(123, pierWall.position.y, 0);
  island.add(pierWall);

  const apron = makeDeckSlab(46, 196, TARMAC_TOP_Y - 0.04, 0.35, apronMat);
  apron.position.set(122, apron.position.y, 0);
  island.add(apron);

  // Apron taxi connectors linking the gate concourse to Taxiway Alpha.
  for (const cz of [-66, -10, 46]) {
    const bridgePad = makeDeckSlab(16, 12, TARMAC_TOP_Y - 0.02, 0.4, taxiMat);
    bridgePad.position.set(95, bridgePad.position.y, cz);
    island.add(bridgePad);
    island.add(makeMarking(18, 0.75, 93, cz, markY, yellowLineMat));
  }
  island.add(makeMarking(0.75, 178, 103, 0, markY, yellowLineMat));

  // Extended Terminal Concourse + central glass atrium along x = 139.
  const terminal = new THREE.Mesh(new THREE.BoxGeometry(10, 6.4, 186), terminalMat);
  terminal.position.set(139, TARMAC_TOP_Y + 3.2, 0);
  island.add(terminal);
  const atrium = new THREE.Mesh(new THREE.BoxGeometry(13, 9.2, 34), terminalMat);
  atrium.position.set(140, TARMAC_TOP_Y + 4.6, 0);
  island.add(atrium);
  const terminalGlass = new THREE.Mesh(new THREE.BoxGeometry(0.6, 2.8, 180), glassMat);
  terminalGlass.position.set(133.8, TARMAC_TOP_Y + 3.8, 0);
  island.add(terminalGlass);

  // Gate stand markings + jetbridges for all 10 gates.
  const bridgeMat = new THREE.MeshStandardMaterial({ color: 0x9aa9b5, roughness: 0.5, metalness: 0.3 });
  for (const [gx, gz] of ap.gates) {
    island.add(makeMarking(16, 0.75, gx, gz, markY, yellowLineMat));
    island.add(makeMarking(0.75, 8, gx + 5.5, gz, markY, yellowLineMat));

    const bridge = new THREE.Mesh(new THREE.BoxGeometry(9.0, 1.5, 1.8), bridgeMat);
    bridge.position.set(gx + 10.5, TARMAC_TOP_Y + 2.8, gz - 3.2);
    island.add(bridge);
  }

  // Airport name banner above the tower.
  const label = makeBannerLabel(ap.name.toUpperCase(), 0xffffff, SWISS_RED, 10);
  label.position.set(towerSpot.x, 63, towerSpot.z);
  island.add(label);

  enableShadows(island);
  scene.add(island);
  return { group: island, beacon, lights, online: ap.online };
}

function buildSatelliteAirport(ap, idx) {
  const island = new THREE.Group();
  const [cx, cz] = ap.center;
  const seed = (idx + 1) * 2.17;
  const shoulderMat = new THREE.MeshStandardMaterial({ color: 0x565d65, roughness: 0.9 });
  const asphaltMat = new THREE.MeshStandardMaterial({ color: 0x262b32, roughness: 0.85 });
  const apronMat = new THREE.MeshStandardMaterial({ color: 0x394049, roughness: 0.85 });
  const terminalMat = new THREE.MeshStandardMaterial({ color: 0x4c6070, roughness: 0.6, metalness: 0.2 });
  const glassMat = new THREE.MeshStandardMaterial({ color: 0x7fd4f0, roughness: 0.2, metalness: 0.45 });
  const lineMat = new THREE.MeshLambertMaterial({
    color: 0xe6eae0, polygonOffset: true, polygonOffsetFactor: -2,
  });
  const yellowLineMat = new THREE.MeshLambertMaterial({
    color: 0xffc857, polygonOffset: true, polygonOffsetFactor: -2,
  });

  // Alpine islet: the tower on a grassy knoll west of the runway, a small
  // rocky peak (snow-dusted) to one side, firs and a chalet or two.
  const knollZ = (idx % 2 === 0 ? 1 : -1) * 36;
  const flagSpot = { x: -26, z: -knollZ * 0.3 };
  const terrain = buildAlpineIsland(island, {
    seed,
    center: [2, 0],
    radius: 72,
    hilliness: 3,
    snowLine: 11,
    pads: [{ x: 17, z: 0, hw: 25, hl: 41 }],
    peaks: [
      { x: -36, z: 0, h: 4.6, r: 20, flat: 5, crag: 0.15 },
      { x: -34, z: knollZ, h: 14, r: 24, crag: 0.5 },
    ],
    keepClear: [{ x: -36, z: 0, r: 8 }, { ...flagSpot, r: 3 }],
    trees: 80,
    chalets: 2,
    rocks: 30,
  });

  const flag = makeSwissFlag(4.5, 10);
  flag.position.set(flagSpot.x, terrain.heightAt(flagSpot.x, flagSpot.z) - 0.3, flagSpot.z);
  island.add(flag);

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

  const lights = addRunwayLights(island, {
    x: 4, zMin: -36.5, zMax: 36.5, halfWidth: 8.1, y: TARMAC_TOP_Y,
    groundY: 2.0, spacing: 7.3, approachLength: 13,
  });
  lights.setOnline(ap.online);

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

  // Device name banner (updated dynamically when online/offline changes).
  const label = makeBannerLabel(
    ap.online ? ap.name.toUpperCase() : `${ap.name.toUpperCase()} (OFF)`,
    ap.online ? 0xffffff : 0xcbd2d9,
    ap.online ? SWISS_RED : 0x4b525a,
    11,
  );
  label.position.set(0, 38, 0);
  label.userData.isAirportLabel = true;
  island.add(label);

  // Tag beacon & runway lights so island-wide offline dimming leaves them alone.
  beacon.userData.skipOfflineDim = true;
  if (lights && lights.group) {
    lights.group.traverse((o) => { o.userData.skipOfflineDim = true; });
  }

  island.position.set(cx, 0, cz);
  enableShadows(island);
  scene.add(island);
  const entry = {
    group: island,
    beacon,
    lights,
    label,
    glassMat,
    name: ap.name,
    online: ap.online,
  };
  applySatelliteOnlineState(entry, ap.online);
  return entry;
}

const OFFLINE_ISLAND_TINT = new THREE.Color(0x66707d);
const GLASS_ONLINE_COLOR = new THREE.Color(0x7fd4f0);
const GLASS_OFFLINE_COLOR = new THREE.Color(0x26323d);

function applySatelliteOnlineState(entry, online) {
  entry.online = online;
  entry.beacon.material.color.setHex(online ? 0x4dff9e : 0xc94a4a);
  if (entry.lights) entry.lights.setOnline(online);

  // Dim terrain, trees, buildings, and markings with a cool overcast cast when offline.
  const seenMats = new Set();
  entry.group.traverse((obj) => {
    if (!obj.isMesh || obj.userData.skipOfflineDim) return;
    const mats = Array.isArray(obj.material) ? obj.material : [obj.material];
    for (const m of mats) {
      if (!m || !m.color || seenMats.has(m)) continue;
      seenMats.add(m);
      if (m === entry.glassMat) {
        m.color.copy(online ? GLASS_ONLINE_COLOR : GLASS_OFFLINE_COLOR);
        continue;
      }
      if (!m.userData.baseColor) {
        m.userData.baseColor = m.color.clone();
      }
      if (online) {
        m.color.copy(m.userData.baseColor);
      } else {
        m.color.copy(m.userData.baseColor).multiply(OFFLINE_ISLAND_TINT);
      }
    }
  });

  // Update the floating airport banner live when online state flips.
  if (entry.label) {
    const fresh = makeBannerLabel(
      online ? entry.name.toUpperCase() : `${entry.name.toUpperCase()} (OFF)`,
      online ? 0xffffff : 0xcbd2d9,
      online ? SWISS_RED : 0x4b525a,
      entry.label.scale.y,
    );
    if (entry.label.material.map) entry.label.material.map.dispose();
    entry.label.material.map = fresh.material.map;
    entry.label.scale.copy(fresh.scale); // "(OFF)" changes the width
    entry.label.material.needsUpdate = true;
    fresh.material.dispose();
  }
}

function ensureAirports(airports) {
  if (!airports || airports.length === 0) return;
  let satIdx = 0;
  for (const ap of airports) {
    const existing = airportGroups.get(ap.id);
    if (existing) {
      if (existing.online !== ap.online) {
        if (existing.glassMat) {
          applySatelliteOnlineState(existing, ap.online);
        } else {
          existing.online = ap.online;
          existing.beacon.material.color.setHex(ap.online ? 0x4dff9e : 0xc94a4a);
          if (existing.lights) existing.lights.setOnline(ap.online);
        }
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
  Build: 0x35c4d6,
  Homelab: 0xf59e0b,
  'AI & Models': 0xa855f7,
  Research: 0x34d399,
};
const PROJECT_PALETTE = [0x60a5fa, 0xf472b6, 0x38bdf8, 0xa3e635, 0xfbbf24];
const sectorMeshes = new Map(); // sector.name -> { group, key }

function topicColor(name) {
  if (TOPIC_COLORS[name]) return TOPIC_COLORS[name];
  let hash = 0;
  for (let i = 0; i < name.length; i++) hash = (hash * 31 + name.charCodeAt(i)) >>> 0;
  return PROJECT_PALETTE[hash % PROJECT_PALETTE.length];
}

function buildSectorGroup(s) {
  const group = new THREE.Group();
  const color = topicColor(s.name);
  const isProject = s.kind === 'project';

  const ring = new THREE.Mesh(
    new THREE.RingGeometry(s.radius - 1.5, s.radius, 56),
    new THREE.MeshBasicMaterial({
      color, transparent: true, opacity: isProject ? 0.55 : 0.35, side: THREE.DoubleSide,
    }),
  );
  ring.rotation.x = -Math.PI / 2;
  ring.position.set(s.center[0], 0.72, s.center[1]);
  group.add(ring);

  // Project zones get an inner accent ring so they stand out from base domains.
  if (isProject) {
    const inner = new THREE.Mesh(
      new THREE.RingGeometry(s.radius - 5.5, s.radius - 4.4, 56),
      new THREE.MeshBasicMaterial({
        color, transparent: true, opacity: 0.32, side: THREE.DoubleSide,
      }),
    );
    inner.rotation.x = -Math.PI / 2;
    inner.position.set(s.center[0], 0.71, s.center[1]);
    group.add(inner);
  }

  const fill = new THREE.Mesh(
    new THREE.CircleGeometry(s.radius, 56),
    new THREE.MeshBasicMaterial({
      color, transparent: true, opacity: isProject ? 0.08 : 0.05, side: THREE.DoubleSide,
    }),
  );
  fill.rotation.x = -Math.PI / 2;
  fill.position.set(s.center[0], 0.6, s.center[1]);
  group.add(fill);

  const tag = isProject ? `PROJECT · ${s.name.toUpperCase()}` : s.name.toUpperCase();
  const label = makeBannerLabel(tag, color, null, 7.5);
  label.position.set(s.center[0], 20, s.center[1]);
  group.add(label);

  scene.add(group);
  return group;
}

function disposeSectorGroup(group) {
  scene.remove(group);
  group.traverse((o) => {
    if (o.geometry) o.geometry.dispose();
    if (o.material) {
      if (o.material.map) o.material.map.dispose();
      o.material.dispose();
    }
  });
}

function ensureSectors(sectors) {
  const seen = new Set();
  for (const s of sectors) {
    seen.add(s.name);
    const key = `${s.center[0]},${s.center[1]},${s.radius},${s.kind || ''}`;
    const existing = sectorMeshes.get(s.name);
    if (existing && existing.key === key) continue;
    if (existing) disposeSectorGroup(existing.group);
    sectorMeshes.set(s.name, { group: buildSectorGroup(s), key });
  }
  for (const [name, entry] of [...sectorMeshes.entries()]) {
    if (!seen.has(name)) {
      disposeSectorGroup(entry.group);
      sectorMeshes.delete(name);
    }
  }
}

/* ------------------------------------------------------ aircraft meshes */

const meshes = new Map();

/* --------------------------------------------------------------- labels */

// Clean banner sprite for airport and sector names. Airports pass a solid
// `fill` (Swiss red); sectors keep the translucent charcoal plate.
// The canvas is sized to the measured text and the sprite keeps that exact
// aspect ratio (`height` in world units), so letters are never stretched.
const BANNER_FONT = '700 64px "Helvetica Neue", Helvetica, "Nimbus Sans", Arial, sans-serif';
const BANNER_TRACKING = 4; // px of letter-spacing at 64px — airy Swiss caps

function makeBannerLabel(text, tint = 0xe8ffff, fill = null, height = 7) {
  const c = document.createElement('canvas');
  const ctx = c.getContext('2d');
  ctx.font = BANNER_FONT;
  ctx.letterSpacing = `${BANNER_TRACKING}px`;
  const textW = ctx.measureText(text).width;
  const padX = 40;
  c.width = Math.ceil(textW + padX * 2);
  c.height = 112;
  // Resizing the canvas resets context state, so set it again.
  ctx.fillStyle = fill === null
    ? 'rgba(20, 22, 24, 0.66)'
    : '#' + new THREE.Color(fill).getHexString();
  ctx.fillRect(0, 0, c.width, c.height);
  ctx.font = BANNER_FONT;
  ctx.letterSpacing = `${BANNER_TRACKING}px`;
  ctx.fillStyle = '#' + new THREE.Color(tint).getHexString();
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  // letterSpacing adds trailing space after the last glyph; nudge to centre.
  ctx.fillText(text, c.width / 2 + BANNER_TRACKING / 2, c.height / 2 + 3);
  // sRGB canvas + no tone mapping so Swiss red renders as #da291c, not pink.
  const map = new THREE.CanvasTexture(c);
  map.colorSpace = THREE.SRGBColorSpace;
  map.anisotropy = 4;
  const sprite = new THREE.Sprite(new THREE.SpriteMaterial({
    map, transparent: true, depthTest: false, toneMapped: false,
  }));
  sprite.scale.set(height * (c.width / c.height), height, 1);
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
    ? 'AT GATE · >24H'
    : a.state === 'ready'
      ? 'TAXIWAY · WAITING'
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
  if (a.state === 'ready') return 0xff943b;
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

/* ----------------------------------------------------------- contrails */

// Shared 2D vapor texture: soft Gaussian cross-section across V and smooth
// dissipation fade along U (0 = fresh behind nozzle, 1 = dispersed tail).
const contrailTexture = (() => {
  const w = 128;
  const h = 32;
  const c = document.createElement('canvas');
  c.width = w;
  c.height = h;
  const ctx = c.getContext('2d');
  const img = ctx.createImageData(w, h);
  for (let y = 0; y < h; y++) {
    const v = (y + 0.5) / h;
    const cross = Math.exp(-Math.pow((v - 0.5) / 0.23, 2));
    for (let x = 0; x < w; x++) {
      const u = x / (w - 1);
      const fadeIn = Math.min(1, u / 0.045);
      const fadeOut = Math.pow(1 - u, 1.55);
      const alpha = Math.round(255 * cross * fadeIn * fadeOut);
      const idx = (y * w + x) * 4;
      img.data[idx] = 255;
      img.data[idx + 1] = 255;
      img.data[idx + 2] = 255;
      img.data[idx + 3] = alpha;
    }
  }
  ctx.putImageData(img, 0, 0);
  const tex = new THREE.CanvasTexture(c);
  tex.wrapS = THREE.ClampToEdgeWrapping;
  tex.wrapT = THREE.ClampToEdgeWrapping;
  return tex;
})();

const _trailWorldPt = new THREE.Vector3();
const _trailTan = new THREE.Vector3();
const _trailToCam = new THREE.Vector3();
const _trailSide = new THREE.Vector3();

class ContrailRibbon {
  constructor({ localOffset, maxPoints, sampleInterval, startWidth, endWidth, color, opacity }) {
    this.localOffset = new THREE.Vector3(...localOffset);
    this.maxPoints = maxPoints;
    this.sampleInterval = sampleInterval;
    this.startWidth = startWidth;
    this.endWidth = endWidth;
    this.points = [];
    this.accum = 0;

    const verts = new Float32Array(maxPoints * 2 * 3);
    const uvs = new Float32Array(maxPoints * 2 * 2);
    const indices = [];
    for (let i = 0; i < maxPoints - 1; i++) {
      const a = i * 2;
      const b = a + 1;
      const c = a + 2;
      const d = a + 3;
      indices.push(a, b, c, b, d, c);
    }

    const geo = new THREE.BufferGeometry();
    geo.setAttribute('position', new THREE.BufferAttribute(verts, 3));
    geo.setAttribute('uv', new THREE.BufferAttribute(uvs, 2));
    geo.setIndex(indices);
    geo.setDrawRange(0, 0);

    const mat = new THREE.MeshBasicMaterial({
      map: contrailTexture,
      color,
      transparent: true,
      opacity,
      depthWrite: false,
      side: THREE.DoubleSide,
    });

    this.mesh = new THREE.Mesh(geo, mat);
    this.mesh.frustumCulled = false;
    this.mesh.renderOrder = 2;
    scene.add(this.mesh);
  }

  clear() {
    this.points.length = 0;
    this.mesh.geometry.setDrawRange(0, 0);
  }

  update(group, dt, emit) {
    if (!emit) {
      // Gently evaporate existing trail when on the ground.
      this.accum += dt;
      if (this.accum >= this.sampleInterval * 0.6 && this.points.length > 0) {
        this.accum = 0;
        this.points.pop();
      }
    } else {
      group.updateMatrixWorld();
      _trailWorldPt.copy(this.localOffset).applyMatrix4(group.matrixWorld);

      if (this.points.length > 0 && this.points[0].distanceToSquared(_trailWorldPt) > 25000) {
        this.clear(); // snapped across map wrap
      }

      if (this.points.length === 0) {
        this.points.unshift(_trailWorldPt.clone());
        this.accum = 0;
      } else {
        // Keep the head pinned live to the engine nozzle between samples.
        this.points[0].copy(_trailWorldPt);
        this.accum += dt;
        if (this.accum >= this.sampleInterval) {
          this.accum = 0;
          this.points.unshift(_trailWorldPt.clone());
          if (this.points.length > this.maxPoints) {
            this.points.pop();
          }
        }
      }
    }

    const n = this.points.length;
    if (n < 2) {
      this.mesh.geometry.setDrawRange(0, 0);
      return;
    }

    const posAttr = this.mesh.geometry.attributes.position;
    const uvAttr = this.mesh.geometry.attributes.uv;
    for (let i = 0; i < n; i++) {
      const p = this.points[i];
      const u = i / (n - 1);
      if (i === 0) _trailTan.subVectors(this.points[0], this.points[1]);
      else if (i === n - 1) _trailTan.subVectors(this.points[n - 2], this.points[n - 1]);
      else _trailTan.subVectors(this.points[i - 1], this.points[i + 1]);
      if (_trailTan.lengthSq() < 1e-6) _trailTan.set(0, 0, 1);
      else _trailTan.normalize();

      _trailToCam.subVectors(camera.position, p);
      if (_trailToCam.lengthSq() < 1e-6) _trailToCam.set(0, 1, 0);
      else _trailToCam.normalize();

      _trailSide.crossVectors(_trailTan, _trailToCam);
      if (_trailSide.lengthSq() < 1e-6) _trailSide.set(1, 0, 0);
      else _trailSide.normalize();

      const halfW = 0.5 * (this.startWidth + (this.endWidth - this.startWidth) * Math.pow(u, 0.75));
      const v0 = i * 2;
      const v1 = v0 + 1;
      posAttr.setXYZ(v0, p.x - _trailSide.x * halfW, p.y - _trailSide.y * halfW, p.z - _trailSide.z * halfW);
      posAttr.setXYZ(v1, p.x + _trailSide.x * halfW, p.y + _trailSide.y * halfW, p.z + _trailSide.z * halfW);
      uvAttr.setXY(v0, u, 0);
      uvAttr.setXY(v1, u, 1);
    }

    posAttr.needsUpdate = true;
    uvAttr.needsUpdate = true;
    this.mesh.geometry.setDrawRange(0, (n - 1) * 6);
  }

  dispose() {
    scene.remove(this.mesh);
    this.mesh.geometry.dispose();
    this.mesh.material.dispose();
  }
}

function makeContrailsFor(a) {
  if (a.kind === 'fighter') {
    return [-0.62, 0.62].map((x) => new ContrailRibbon({
      localOffset: [x, -0.05, -8.5],
      maxPoints: 58,
      sampleInterval: 0.08,
      startWidth: 0.5,
      endWidth: 2.3,
      color: 0xdcf6ff,
      opacity: 0.56,
    }));
  }
  if (a.kind === 'drone') {
    return [new ContrailRibbon({
      localOffset: [0, 0, -10.2],
      maxPoints: 38,
      sampleInterval: 0.14,
      startWidth: 0.35,
      endWidth: 1.4,
      color: 0xeef6ff,
      opacity: 0.24,
    })];
  }
  if (a.model_class === 'frontier') {
    // High-altitude frontier jets stream twin rich white contrails from both engines!
    // Offsets are in aircraft.js model space: nacelle exhausts / wingtips.
    return [-4.6, 4.6].map((x) => new ContrailRibbon({
      localOffset: [x, -1.8, -1.3],
      maxPoints: 120,
      sampleInterval: 0.13,
      startWidth: 0.85,
      endWidth: 5.0,
      color: 0xf7fbff,
      opacity: 0.68,
    }));
  }
  // Local prop: subtle wingtip wisps.
  return [-7.8, 7.8].map((x) => new ContrailRibbon({
    localOffset: [x, 1.75, 0.8],
    maxPoints: 34,
    sampleInterval: 0.12,
    startWidth: 0.25,
    endWidth: 1.15,
    color: 0xffffff,
    opacity: 0.22,
  }));
}

function ensureMesh(a) {
  let entry = meshes.get(a.id);
  if (entry) return entry;

  let built;
  // Each airframe is painted in its model provider's livery.
  if (a.kind === 'fighter') built = makeFighterMesh(a.model);
  else if (a.kind === 'drone') built = makeDroneMesh(a.model);
  else if (a.model_class === 'frontier') built = makeJetMesh(a.model);
  else built = makeAgentMesh(a.model);
  built.model = a.model ?? null;

  const s = built.scale;
  built.group.scale.setScalar(s);
  built.group.rotation.order = 'YXZ';

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
  built.roll = 0;
  built.group.rotation.y = Math.PI / 2 - a.heading;
  built.state = a.state;
  built.kind = a.kind;
  built.model_class = a.model_class;
  built.ring = ring;
  built.contrails = makeContrailsFor(a);
  built.prop = built.group.getObjectByName('prop');
  built.gear = built.group.getObjectByName('gear');

  enableShadows(built.group, { receive: false });
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
      if (o.material.map && !o.material.map.userData.shared) o.material.map.dispose();
      o.material.dispose();
    }
  });
  // Livery textures are shared between aircraft; only materials are owned.
  for (const m of entry.disposeMaterials) m.dispose();
  if (entry.contrails) {
    for (const c of entry.contrails) c.dispose();
  }
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
const followEl = document.getElementById('hud-follow');

/* ----------------------------------------------------------- follow cam */

const OVERVIEW_CAM = new THREE.Vector3(0, 410, 355);
const OVERVIEW_TARGET = new THREE.Vector3(0, 20, 0);

let followId = null;
let followLastPos = null;   // aircraft's position on the previous frame
let followSwoop = false;    // smoothly approach chase offset until arrival or user drag
let returnOverview = false; // smoothly glide back when exiting follow cam

function chaseOffsetFor(entry) {
  // Position camera slightly behind-and-above the aircraft's tail/starboard quarter.
  const dist = entry.kind === 'fighter' ? 40 : 54;
  const lift = entry.kind === 'fighter' ? 18 : 24;
  // Heading h moves along (cos h, sin h); tail is opposite (-cos h, -sin h),
  // starboard right is (-sin h, cos h).
  const h = entry.heading || 0;
  const backX = -Math.cos(h);
  const backZ = -Math.sin(h);
  const rightX = -Math.sin(h);
  const rightZ = Math.cos(h);
  return new THREE.Vector3(
    backX * dist + rightX * (dist * 0.42),
    lift,
    backZ * dist + rightZ * (dist * 0.42),
  );
}

function updateFollowBadge(a) {
  if (!a) {
    followEl.hidden = true;
    return;
  }
  followEl.textContent = `FOLLOWING: ${formatTaskTitle(a)}  ×`;
  followEl.hidden = false;
}

function startFollow(id) {
  const entry = meshes.get(id);
  const a = currentSnapshot.get(id);
  if (!entry || !a) return;
  followId = id;
  followLastPos = entry.group.position.clone();
  followSwoop = true;
  returnOverview = false;
  updateFollowBadge(a);
}

function stopFollow(glideToOverview = false) {
  if (followId === null) return;
  followId = null;
  followLastPos = null;
  followSwoop = false;
  returnOverview = glideToOverview;
  updateFollowBadge(null);
}

// Manual camera orbit/zoom cancels automatic swoop/return so controls feel responsive.
controls.addEventListener('start', () => {
  followSwoop = false;
  returnOverview = false;
});

followEl.addEventListener('click', () => {
  stopFollow(true);
  closeRadio();
});

window.addEventListener('keydown', (ev) => {
  if (ev.key === 'Escape' && followId !== null) {
    stopFollow(true);
    closeRadio();
  }
});

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
  close: () => stopFollow(true),
});

function connect() {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  ws = new WebSocket(`${proto}//${location.host}/ws`);
/* ------------------------------------------------------ keyboard camera */
// Arrows orbit around the current target (the followed aircraft when in
// follow cam), +/- or PageUp/PageDown zoom, WASD pans the overview, Home
// returns to the overview. Keys are held-state so motion is smooth per frame.

const CAMERA_KEYS = new Set([
  'ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown',
  'PageUp', 'PageDown', '+', '=', '-', '_',
  'w', 'a', 's', 'd',
]);
const heldKeys = new Set();

function isTypingTarget(el) {
  if (!el) return false;
  const tag = el.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || el.isContentEditable;
}


  ws.addEventListener('open', () => {
    statusEl.textContent = 'connected';
    statusEl.classList.remove('error');
    return;
  });
  if (isTypingTarget(ev.target) || ev.ctrlKey || ev.metaKey || ev.altKey) return;
  if (ev.key === 'Home') {
    ev.preventDefault();
    stopFollow(false);
    returnOverview = true;
    return;
  }
  const key = ev.key.length === 1 ? ev.key.toLowerCase() : ev.key;
  if (!CAMERA_KEYS.has(key)) return;
  ev.preventDefault();
  heldKeys.add(key);
  // Manual input takes over from automatic swoop/return glides.
  followSwoop = false;
  returnOverview = false;
  ws.addEventListener('close', () => {
    statusEl.textContent = 'disconnected — reconnecting…';
window.addEventListener('keyup', (ev) => {
  const key = ev.key.length === 1 ? ev.key.toLowerCase() : ev.key;
  heldKeys.delete(key);
});
window.addEventListener('blur', () => heldKeys.clear());

const _kbOffset = new THREE.Vector3();
const _kbSpherical = new THREE.Spherical();
const _kbForward = new THREE.Vector3();
const _kbRight = new THREE.Vector3();

function applyKeyboardCamera(dt) {
  if (heldKeys.size === 0) return;
  const has = (k) => heldKeys.has(k);

  _kbOffset.copy(camera.position).sub(controls.target);
  _kbSpherical.setFromVector3(_kbOffset);

  const yaw = (has('ArrowRight') ? 1 : 0) - (has('ArrowLeft') ? 1 : 0);
  const pitch = (has('ArrowDown') ? 1 : 0) - (has('ArrowUp') ? 1 : 0);
  const zoom = (has('-') || has('_') || has('PageDown') ? 1 : 0)
    - (has('+') || has('=') || has('PageUp') ? 1 : 0);

  _kbSpherical.theta += yaw * 1.6 * dt;
  _kbSpherical.phi = THREE.MathUtils.clamp(
    _kbSpherical.phi + pitch * 1.0 * dt,
    0.08,
    controls.maxPolarAngle,
  );
  _kbSpherical.radius = THREE.MathUtils.clamp(
    _kbSpherical.radius * Math.exp(zoom * 1.4 * dt),
    controls.minDistance,
    controls.maxDistance,
  );
  _kbOffset.setFromSpherical(_kbSpherical);
  camera.position.copy(controls.target).add(_kbOffset);

  // Ground-plane pan only in overview; in follow cam the target is the aircraft.
  if (followId === null) {
    const fwd = (has('w') ? 1 : 0) - (has('s') ? 1 : 0);
    const side = (has('d') ? 1 : 0) - (has('a') ? 1 : 0);
    if (fwd || side) {
      camera.getWorldDirection(_kbForward);
      _kbForward.y = 0;
      if (_kbForward.lengthSq() < 1e-6) _kbForward.set(0, 0, -1);
      _kbForward.normalize();
      _kbRight.crossVectors(_kbForward, camera.up).normalize();
      const speed = _kbSpherical.radius * 0.9 * dt;
      const move = _kbForward.multiplyScalar(fwd * speed).addScaledVector(_kbRight, side * speed);
      camera.position.add(move);
      controls.target.add(move);
    }
  }
}

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
    // Repaint when the thread switches model (new livery).
    if (meshes.has(a.id) && meshes.get(a.id).model !== (a.model ?? null)) disposeMesh(a.id);
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
      if (followId === id) stopFollow(true);
    }
  }

  if (followId !== null) {
    const followed = aircraft.find((a) => a.id === followId);
    if (followed) updateFollowBadge(followed);
    else stopFollow(true);
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
  if (hits.length === 0) {
    // Clicking empty sky/water disengages follow cam.
    if (followId !== null) {
      stopFollow(false);
      closeRadio();
    }
    return;
  }
  let obj = hits[0].object;
  while (obj.parent && !groups.includes(obj)) obj = obj.parent;
  if (!groups.includes(obj)) return;

  const id = [...meshes.entries()].find(([, e]) => e.group === obj)?.[0];
  if (id === undefined) return;

  const a = currentSnapshot.get(id);
  if (a) {
    openRadio(a);
    startFollow(id);
  }
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
    const wrapped = entry.group.position.distanceToSquared(entry.target) > 40000;
    if (wrapped) {
      entry.group.position.copy(entry.target);
      if (entry.contrails) for (const c of entry.contrails) c.clear();
    } else {
      entry.group.position.lerp(entry.target, k);
    }

    let dh = entry.targetHeading - entry.heading;
    while (dh > Math.PI) dh -= 2 * Math.PI;
    while (dh < -Math.PI) dh += 2 * Math.PI;
    const stepH = dh * k;
    entry.heading += stepH;
    entry.group.rotation.y = Math.PI / 2 - entry.heading;

    // Bank smoothly into turns when airborne; keep wings level on the ground.
    const airborne = ['cruising', 'holding', 'takeoff', 'landing'].includes(entry.state);
    const omega = dt > 0.001 ? stepH / dt : 0;
    const maxBank = entry.kind === 'fighter' ? 0.52 : (entry.model_class === 'frontier' ? 0.32 : 0.4);
    const targetRoll = airborne
      ? Math.max(-maxBank, Math.min(maxBank, omega * 1.15))
      : 0;
    entry.roll += (targetRoll - entry.roll) * (1 - Math.exp(-5 * dt));
    entry.group.rotation.z = entry.roll;

    // Update contrail ribbons (emit only when airborne above ground).
    if (entry.contrails) {
      const emit = airborne && entry.group.position.y > 25;
      for (const c of entry.contrails) {
        c.update(entry.group, dt, emit);
      }
    }

    if (entry.prop && entry.state !== 'parked' && entry.state !== 'landed') {
      entry.prop.rotation.z = now * 30;
    }

    // Retract landing gear while en route.
    if (entry.gear) {
      entry.gear.visible = entry.state !== 'cruising' && entry.state !== 'holding';
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
  updateAlpine(now);

  for (const ap of airportGroups.values()) {
    const pulse = ap.online ? 0.85 + 0.3 * Math.sin(now * 3) : 0.7;
    ap.beacon.scale.setScalar(pulse);
    ap.lights.update(now);
  }

  // Follow camera: track the selected aircraft while keeping OrbitControls active.
  if (followId !== null) {
    const entry = meshes.get(followId);
    if (entry) {
      const pos = entry.group.position;
      if (followLastPos) {
        const move = pos.clone().sub(followLastPos);
        // Carry camera + target along with the aircraft (including map wraps).
        camera.position.add(move);
        controls.target.add(move);
      }
      followLastPos = pos.clone();

      const camK = 1 - Math.exp(-5.5 * dt);
      controls.target.lerp(pos, camK);

      if (followSwoop) {
        const desiredCam = pos.clone().add(chaseOffsetFor(entry));
        camera.position.lerp(desiredCam, camK);
        if (camera.position.distanceTo(desiredCam) < 1.5 && controls.target.distanceTo(pos) < 0.8) {
          followSwoop = false;
        }
      }
    }
  } else if (returnOverview) {
    const camK = 1 - Math.exp(-4.5 * dt);
    camera.position.lerp(OVERVIEW_CAM, camK);
    controls.target.lerp(OVERVIEW_TARGET, camK);
    if (camera.position.distanceTo(OVERVIEW_CAM) < 2.0 && controls.target.distanceTo(OVERVIEW_TARGET) < 1.0) {
      returnOverview = false;
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
  applyKeyboardCamera(dt);
