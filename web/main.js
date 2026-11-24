// Skywatch browser client: Three.js scene, WS snapshot/delta consumption,
// smooth aircraft interpolation, device airports, detailed aircraft models,
// fighter wingmen, task labels, picking, radio panel, and ambient ticker.

import * as THREE from './vendor/three.module.js';
import { OrbitControls } from './vendor/OrbitControls.js';
import {
  setupRadio, openRadio, closeRadio, updateRadio, selectedAircraftId, focusRadioInput,
  appendHistory, stateColor,
} from './radio.js';
import {
  buildAlpineIsland, buildAlpineBackdrop, makeSwissFlag, updateAlpine, SWISS_RED,
  makeLakeMaterial, enableShadows,
} from './alpine.js';
import { addRunwayLights } from './runway-lights.js';
import { addLandingLights, createRunwayEvents } from './runway-events.js';
import {
  makeAgentMesh, makeJetMesh, makeJumboMesh, makeFighterMesh, makeDroneMesh, makeTankerMesh,
} from './aircraft.js';
import { isFlagshipAgent } from './flagship.js';
import { liveryFor } from './livery.js';
import { createLocalFlair } from './local-flair.js';
import { createAlertManager, classifyAircraftUrgency, isPassiveHold } from './alerts.js';

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
const shadowRes = Math.min(4096, renderer.capabilities.maxTextureSize || 2048);
sun.shadow.mapSize.set(shadowRes, shadowRes);
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

/* ----------------------------------------------------- runway events */

// Home Runway 36 (mirrors the server's RUNWAY_THRESHOLD / FINAL_APPROACH_FIX /
// TOUCHDOWN_Z); takeoffs and landings light it up so they draw the eye.
let homeRunwayLights = null;
const runwayEvents = createRunwayEvents(scene, {
  x: 56, zMin: -76, zMax: 56, halfWidth: 10.6, deckY: TARMAC_TOP_Y,
  glideFrom: [276], touchdownZ: 34,
});

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
  homeRunwayLights = lights;

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

  // Arrival return loop passes north of Alpha's six holding slots.
  // Matches the server route: (56,-66) -> (70,-66) -> (70,-88)
  // -> (103,-88) -> apron. Departures keep the south threshold entrance.
  for (const [w, h, x, z] of [[14, 22, 70, -77], [33, 12, 86.5, -88]]) {
    const loop = makeDeckSlab(w, h, TARMAC_TOP_Y, 0.4, taxiMat);
    loop.position.set(x, loop.position.y, z);
    island.add(loop);
  }
  island.add(makeMarking(14, 0.8, 63, -66, markY, yellowLineMat));
  island.add(makeMarking(0.8, 22, 70, -77, markY, yellowLineMat));
  island.add(makeMarking(33, 0.8, 86.5, -88, markY, yellowLineMat));

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
  return {
    id: ap.id,
    name: ap.name,
    isHome: true,
    center: [0, 0],
    group: island,
    beacon,
    lights,
    runwayEvents,
    online: ap.online,
  };
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
    pads: [{ x: 20, z: 0, hw: 30, hl: 41 }],
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

  // Painted lead-in/lead-off lines matching the server's satellite routes:
  // departures join at the south threshold (z = 30) from the apron lane
  // (x = 16); arrivals vacate at the far (north) end (z = -30.5).
  for (const z of [30, -30.5]) {
    const connector = makeDeckSlab(12, 6, TARMAC_TOP_Y - 0.03, 0.35, apronMat);
    connector.position.set(10, connector.position.y, z);
    island.add(connector);
  }
  island.add(makeMarking(12, 0.7, 10, 30, markY, yellowLineMat));
  island.add(makeMarking(12, 0.7, 10, -30.5, markY, yellowLineMat));
  island.add(makeMarking(0.7, 61.2, 16, -0.25, markY, yellowLineMat));

  // Elevated apron + small terminal + 4 gates.
  // Apron and terminal sit far enough east that a nose-in jet (gate x = +24,
  // nose up to ~15u ahead of it) stops well short of the terminal glass.
  const apron = makeDeckSlab(31, 70, TARMAC_TOP_Y - 0.05, 0.35, apronMat);
  apron.position.x = 28.5;
  island.add(apron);

  const terminal = new THREE.Mesh(new THREE.BoxGeometry(7, 5.2, 60), terminalMat);
  terminal.position.set(44, TARMAC_TOP_Y + 2.6, 0);
  island.add(terminal);
  const tGlass = new THREE.Mesh(new THREE.BoxGeometry(0.5, 2.0, 54), glassMat);
  tGlass.position.set(40.3, TARMAC_TOP_Y + 3.0, 0);
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
  const satRunwayEvents = createRunwayEvents(scene, {
    x: cx + 4, zMin: cz - 36.5, zMax: cz + 36.5, halfWidth: 8.1, deckY: TARMAC_TOP_Y,
    glideFrom: [cz + 192], touchdownZ: cz + 22,
  });
  const entry = {
    id: ap.id,
    isHome: false,
    center: [cx, cz],
    group: island,
    beacon,
    lights,
    runwayEvents: satRunwayEvents,
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
  let stateLabel;
  if (a.kind === 'tanker') {
    const match = (a.hold_reason || '').match(/(\d+)/);
    stateLabel = match ? `TANKER · ON HOLD (${match[1]})` : 'TANKER · ON HOLD';
  } else if (a.state === 'refueling') {
    stateLabel = a.data_stale ? 'RENDEZVOUS SUSPENDED' : a.refuel_connected ? 'TANKER FORMATION' : 'TANKER RENDEZVOUS';
  } else if (a.state === 'parked') {
    stateLabel = 'AT GATE · >24H';
  } else if (a.state === 'ready') {
    stateLabel = 'TAXIWAY · WAITING';
  } else if (a.state === 'departing') {
    stateLabel = 'DEPARTING · DONE';
  } else if (a.kind === 'fighter') {
    stateLabel = 'WINGMAN';
  } else {
    stateLabel = a.state.toUpperCase();
  }
  const sub = `${a.callsign} · ${stateLabel}`;
  ctx.font = 'bold 21px "IBM Plex Mono", ui-monospace, monospace';
  ctx.fillStyle = accent;
  ctx.fillText(sub, 28, 74);

  const tex = new THREE.CanvasTexture(c);
  tex.colorSpace = THREE.SRGBColorSpace;
  return tex;
}

function labelAccentFor(a) {
  if (a.kind === 'tanker') return 0xffb020;
  if (a.state === 'refueling') return 0xffb020;
  if (a.state === 'departing') return 0x4dff9e;
  if (a.state === 'holding') return 0xffb020;
  if (a.state === 'ready') return 0xff943b;
  if (a.kind === 'fighter') return 0x7ce8ff;
  if (a.state === 'parked' || a.callsign.startsWith('GT')) return 0x8f9ba6;
  if (a.state === 'landed') return 0xff7b6b;
  if (a.topic) return topicColor(a.topic);
  return 0x5be7ff;
}

function updateAircraftLabel(entry, a) {
  const extra = a.kind === 'tanker' ? (a.hold_reason || '') : `${a.refuel_connected}|${a.data_stale}`;
  const key = `${a.title || ''}|${a.state}|${a.topic || ''}|${a.callsign}|${extra}`;
  if (entry.labelKey === key) return;
  entry.labelKey = key;
  const accent = labelAccentFor(a);
  const newTex = buildAircraftLabelTexture(a, accent);
  if (entry.label.material.map) entry.label.material.map.dispose();
  entry.label.material.map = newTex;
  entry.label.material.needsUpdate = true;
}

const NOOP_RAYCAST = () => {};

function makeHoldRing(scale) {
  const ring = new THREE.Mesh(
    new THREE.RingGeometry(14 / scale, 17 / scale, 32),
    new THREE.MeshBasicMaterial({
      color: 0xffb020, transparent: true, opacity: 0.8, side: THREE.DoubleSide,
    }),
  );
  ring.rotation.x = -Math.PI / 2;
  ring.raycast = NOOP_RAYCAST;
  return ring;
}

function makeHoverRing(scale) {
  const ring = new THREE.Mesh(
    new THREE.RingGeometry(16 / scale, 21 / scale, 36),
    new THREE.MeshBasicMaterial({
      color: 0x5cf2ff, transparent: true, opacity: 0.92, side: THREE.DoubleSide, depthTest: false,
    }),
  );
  ring.rotation.x = -Math.PI / 2;
  ring.renderOrder = 11;
  ring.visible = false;
  ring.raycast = NOOP_RAYCAST;
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
  if (a.kind === 'tanker') {
    // Heavy 4-engine tanker: four crisp high-altitude contrails!
    return [-10.2, -5.6, 5.6, 10.2].map((x) => new ContrailRibbon({
      localOffset: [x, -1.65, -1.8],
      maxPoints: 130,
      sampleInterval: 0.14,
      startWidth: 0.9,
      endWidth: 5.4,
      color: 0xf7fbff,
      opacity: 0.65,
    }));
  }
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
  if (isFlagshipAgent(a)) {
    // Flagship 747-400 streams four crisp high-altitude contrails from its
    // four underwing turbofan exhausts (model space is in true metres).
    return [
      [-21.2, -1.07, -7.0],
      [-11.9, -2.47, 1.2],
      [11.9, -2.47, 1.2],
      [21.2, -1.07, -7.0],
    ].map((localOffset) => new ContrailRibbon({
      localOffset,
      maxPoints: 130,
      sampleInterval: 0.13,
      startWidth: 0.88,
      endWidth: 5.2,
      color: 0xf7fbff,
      opacity: 0.68,
    }));
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

/* ----------------------------------------------- aerial refuelling pipes */

// Dynamic 3D catenary refuelling hose + flared drogue basket + animated token
// pulse beads connecting a provider tanker to each aircraft waiting for tokens.
const PIPE_SEGS = 20;
const PIPE_RADIAL = 6;
const PIPE_RADIUS = 0.42;
const PIPE_PULSES = 4;
const PIPE_MAX_DIST = 105;

const _pipeP0 = new THREE.Vector3();
const _pipeP1 = new THREE.Vector3();
const _pipeC1 = new THREE.Vector3();
const _pipeC2 = new THREE.Vector3();
const _pipeBack = new THREE.Vector3();
const _pipeFwd = new THREE.Vector3();
const _pipeTan = new THREE.Vector3();
const _pipeN = new THREE.Vector3();
const _pipeB = new THREE.Vector3();
const _pipeUp = new THREE.Vector3(0, 1, 0);
const _pipeAltUp = new THREE.Vector3(1, 0, 0);
const _pipeQuat = new THREE.Quaternion();

function cubicBezierPoint(out, p0, c1, c2, p1, t) {
  const u = 1 - t;
  const tt = t * t;
  const uu = u * u;
  const uuu = uu * u;
  const ttt = tt * t;
  out.set(
    uuu * p0.x + 3 * uu * t * c1.x + 3 * u * tt * c2.x + ttt * p1.x,
    uuu * p0.y + 3 * uu * t * c1.y + 3 * u * tt * c2.y + ttt * p1.y,
    uuu * p0.z + 3 * uu * t * c1.z + 3 * u * tt * c2.z + ttt * p1.z,
  );
  return out;
}

class RefuelPipe {
  constructor(accentColor) {
    this.group = new THREE.Group();
    this.group.visible = false;

    const ringCount = PIPE_SEGS + 1;
    const verts = new Float32Array(ringCount * PIPE_RADIAL * 3);
    const indices = [];
    for (let i = 0; i < PIPE_SEGS; i++) {
      for (let j = 0; j < PIPE_RADIAL; j++) {
        const j2 = (j + 1) % PIPE_RADIAL;
        const a = i * PIPE_RADIAL + j;
        const b = i * PIPE_RADIAL + j2;
        const c = (i + 1) * PIPE_RADIAL + j;
        const d = (i + 1) * PIPE_RADIAL + j2;
        indices.push(a, c, b, b, c, d);
      }
    }
    this.tubeGeo = new THREE.BufferGeometry();
    this.tubeGeo.setAttribute('position', new THREE.BufferAttribute(verts, 3));
    this.tubeGeo.setIndex(indices);
    this.tubeMat = new THREE.MeshStandardMaterial({
      color: 0x262c34,
      emissive: new THREE.Color(accentColor),
      emissiveIntensity: 0.35,
      roughness: 0.45,
      metalness: 0.4,
    });
    this.tubeMesh = new THREE.Mesh(this.tubeGeo, this.tubeMat);
    this.tubeMesh.frustumCulled = false;
    this.group.add(this.tubeMesh);

    // Flared high-visibility drogue basket opening toward the receiver receptacle.
    this.drogueGeo = new THREE.CylinderGeometry(1.38, 0.45, 2.4, 12, 1, true);
    this.drogueMat = new THREE.MeshBasicMaterial({
      color: 0xffa824,
      side: THREE.DoubleSide,
    });
    this.drogue = new THREE.Mesh(this.drogueGeo, this.drogueMat);
    this.group.add(this.drogue);
    this.accent = accentColor;

    // Glowing token-refuel pulse beads gliding down the hose toward the receiver.
    this.pulseGeo = new THREE.SphereGeometry(0.72, 8, 6);
    this.pulseMat = new THREE.MeshBasicMaterial({ color: 0xffd166 });
    this.pulses = [];
    for (let i = 0; i < PIPE_PULSES; i++) {
      const m = new THREE.Mesh(this.pulseGeo, this.pulseMat);
      this.group.add(m);
      this.pulses.push(m);
    }

    this.pts = Array.from({ length: ringCount }, () => new THREE.Vector3());
    scene.add(this.group);
  }

  update(tankerEntry, receiverEntry, slot, now) {
    if (!tankerEntry || !receiverEntry || !tankerEntry.refuelAnchors) {
      this.group.visible = false;
      return;
    }
    tankerEntry.group.updateMatrixWorld();
    receiverEntry.group.updateMatrixWorld();

    const anchorKey = slot === 0 ? 'center' : (slot % 2 === 1 ? 'port' : 'starboard');
    const localAnchor = tankerEntry.refuelAnchors[anchorKey] || tankerEntry.refuelAnchors.center;
    _pipeP0.copy(localAnchor).applyMatrix4(tankerEntry.group.matrixWorld);

    const localPort = receiverEntry.refuelPort || new THREE.Vector3(0, 1.5, 6);
    _pipeP1.copy(localPort).applyMatrix4(receiverEntry.group.matrixWorld);

    const dist = _pipeP0.distanceTo(_pipeP1);
    if (dist < 1 || dist > PIPE_MAX_DIST) {
      this.group.visible = false;
      return;
    }
    this.group.visible = true;

    // Tanker aft vector (-Z in model space) & receiver forward vector (+Z).
    _pipeBack.set(0, 0, -1).transformDirection(tankerEntry.group.matrixWorld);
    _pipeFwd.set(0, 0, 1).transformDirection(receiverEntry.group.matrixWorld);

    const sag = Math.min(8.0, dist * 0.14);
    _pipeC1.copy(_pipeP0).addScaledVector(_pipeBack, dist * 0.34);
    _pipeC1.y -= sag;
    _pipeC2.copy(_pipeP1).addScaledVector(_pipeFwd, dist * 0.28);
    _pipeC2.y -= sag * 0.45;

    for (let i = 0; i <= PIPE_SEGS; i++) {
      cubicBezierPoint(this.pts[i], _pipeP0, _pipeC1, _pipeC2, _pipeP1, i / PIPE_SEGS);
    }

    const posAttr = this.tubeGeo.attributes.position;
    for (let i = 0; i <= PIPE_SEGS; i++) {
      const p = this.pts[i];
      if (i === 0) _pipeTan.subVectors(this.pts[1], this.pts[0]);
      else if (i === PIPE_SEGS) _pipeTan.subVectors(this.pts[PIPE_SEGS], this.pts[PIPE_SEGS - 1]);
      else _pipeTan.subVectors(this.pts[i + 1], this.pts[i - 1]);
      if (_pipeTan.lengthSq() < 1e-6) _pipeTan.set(0, 0, 1);
      else _pipeTan.normalize();

      const refUp = Math.abs(_pipeTan.y) > 0.92 ? _pipeAltUp : _pipeUp;
      _pipeN.crossVectors(_pipeTan, refUp).normalize();
      _pipeB.crossVectors(_pipeTan, _pipeN).normalize();

      for (let j = 0; j < PIPE_RADIAL; j++) {
        const ang = (j / PIPE_RADIAL) * Math.PI * 2;
        const c = Math.cos(ang) * PIPE_RADIUS;
        const s = Math.sin(ang) * PIPE_RADIUS;
        posAttr.setXYZ(
          i * PIPE_RADIAL + j,
          p.x + _pipeN.x * c + _pipeB.x * s,
          p.y + _pipeN.y * c + _pipeB.y * s,
          p.z + _pipeN.z * c + _pipeB.z * s,
        );
      }
    }
    posAttr.needsUpdate = true;
    this.tubeGeo.computeVertexNormals();

    // Place flared drogue basket just ahead of the receiver's receptacle.
    _pipeTan.subVectors(this.pts[PIPE_SEGS], this.pts[PIPE_SEGS - 1]).normalize();
    this.drogue.position.copy(_pipeP1).addScaledVector(_pipeTan, -1.1);
    _pipeQuat.setFromUnitVectors(_pipeUp, _pipeTan);
    this.drogue.quaternion.copy(_pipeQuat);

    // Animate glowing token pulses flowing from tanker (t=0) to waiting plane (t=1).
    for (let i = 0; i < this.pulses.length; i++) {
      const t = (now * 0.42 + i / this.pulses.length) % 1;
      cubicBezierPoint(this.pulses[i].position, _pipeP0, _pipeC1, _pipeC2, _pipeP1, t);
    }
  }

  dispose() {
    scene.remove(this.group);
    this.tubeGeo.dispose();
    this.tubeMat.dispose();
    this.drogueGeo.dispose();
    this.drogueMat.dispose();
    this.pulseGeo.dispose();
    this.pulseMat.dispose();
  }
}

const refuelPipes = new Map(); // receiverId -> RefuelPipe

function updateRefuelPipes(now) {
  const activeReceivers = new Set();
  for (const a of currentSnapshot.values()) {
    if (!a.refuel_connected || a.tanker_id == null || a.refuel_slot > 2) continue;
    const tankerEntry = meshes.get(a.tanker_id);
    const receiverEntry = meshes.get(a.id);
    if (!tankerEntry || !receiverEntry) continue;

    activeReceivers.add(a.id);
    const liv = liveryFor(a.model);
    const accent = liv.swoosh || '#ffb020';
    let pipe = refuelPipes.get(a.id);
    if (!pipe) {
      pipe = new RefuelPipe(accent);
      refuelPipes.set(a.id, pipe);
    } else if (pipe.accent !== accent) {
      pipe.accent = accent;
      pipe.tubeMat.emissive.set(accent);
    }
    pipe.update(tankerEntry, receiverEntry, a.refuel_slot ?? 0, now);
  }

  for (const [id, pipe] of [...refuelPipes.entries()]) {
    if (!activeReceivers.has(id)) {
      pipe.dispose();
      refuelPipes.delete(id);
    }
  }
}

function worldYForAircraft(a, wheelDrop) {
  const groundOffset = TARMAC_TOP_Y + wheelDrop;
  if (['landed', 'taxi', 'ready', 'parked'].includes(a.state)) {
    return a.position[2] + groundOffset;
  }
  if (a.state === 'landing') {
    // Keep a constant deck datum throughout final; blend only during the
    // high-altitude circuit so entering landing from cruise remains smooth.
    const liftBlend = THREE.MathUtils.smoothstep(a.position[2], 15, 45);
    return a.position[2] + groundOffset * (1 - liftBlend);
  }
  if (a.state === 'departing' || a.state === 'takeoff') {
    // Seamlessly blend wheel contact on the elevated runway deck into world altitude during climb-out.
    const liftBlend = Math.min(1, Math.max(0, a.position[2] / 15));
    return a.position[2] + groundOffset * (1 - liftBlend);
  }
  return a.position[2];
}

function ensureMesh(a) {
  let entry = meshes.get(a.id);
  if (entry) return entry;

  let built;
  const flagship = isFlagshipAgent(a);
  // Each airframe is painted in its model provider's livery.
  if (a.kind === 'tanker') built = makeTankerMesh(a.model);
  else if (a.kind === 'fighter') built = makeFighterMesh(a.model);
  else if (a.kind === 'drone') built = makeDroneMesh(a.model);
  else if (flagship) built = makeJumboMesh(a.model);
  else if (a.model_class === 'frontier') built = makeJetMesh(a.model);
  else built = makeAgentMesh(a.model);
  built.model = a.model ?? null;
  built.isFlagship = flagship;

  const s = built.scale;
  built.group.scale.setScalar(s);
  built.group.rotation.order = 'YXZ';
  if (a.kind !== 'fighter' && a.kind !== 'tanker') {
    built.landingLights = addLandingLights(built, { groundY: TARMAC_TOP_Y + 0.02 });
  }

  const label = new THREE.Sprite(new THREE.SpriteMaterial({
    transparent: true, depthTest: false,
  }));
  const lw = a.kind === 'tanker' ? 44 : (flagship ? 42 : (a.kind === 'fighter' ? 28 : 38));
  const lh = lw * (104 / 512);
  label.scale.set(lw / s, lh / s, 1);
  label.position.y = (a.kind === 'tanker' ? 16 : (flagship ? 14.8 : (a.kind === 'fighter' ? 9.5 : 13))) / s;
  label.renderOrder = 10;
  built.group.add(label);
  built.label = label;
  built.labelBaseY = label.position.y;
  built.labelBaseScale = label.scale.clone();
  built.labelKey = null;
  updateAircraftLabel(built, a);

  const ring = makeHoldRing(s);
  ring.visible = false;
  built.group.add(ring);

  const hoverRing = makeHoverRing(s);
  built.group.add(hoverRing);
  built.hoverRing = hoverRing;

  // Server position is [x, z, altitude]; Three.js is (x, y=altitude, z).
  const y = worldYForAircraft(a, built.wheelDrop);
  built.group.position.set(a.position[0], y, a.position[1]);
  built.target = built.group.position.clone();
  built.heading = a.heading;
  built.targetHeading = a.heading;
  built.snapX = a.position[0];
  built.snapZ = a.position[1];
  built.altitude = a.position[2];
  built.speed = a.speed ?? 0;
  built.dustClock = 0;
  built.roll = 0;
  built.pitch = 0;
  built.flightPitch = 0;
  built.group.rotation.y = Math.PI / 2 - a.heading;
  built.state = a.state;
  built.kind = a.kind;
  built.model_class = a.model_class;
  built.ring = ring;
  built.contrails = makeContrailsFor(a);
  if (a.kind === 'agent' && a.model_class === 'local') {
    built.localFlair = createLocalFlair(scene, a);
  }
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
  if (entry.landingLights) {
    entry.landingLights.dispose();
  }
  scene.remove(entry.group);
  entry.group.traverse((o) => {
    // THREE.Sprite shares a singleton BufferGeometry across all sprites.
    if (o.geometry && !o.isSprite) o.geometry.dispose();
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
  if (entry.localFlair) {
    entry.localFlair.dispose();
  }
  meshes.delete(id);
}

/* --------------------------------------------------------------- alerts */

const reducedMotionQuery = window.matchMedia('(prefers-reduced-motion: reduce)');
let hoveredAlertAircraftId = null;
let hoveredAlertAirportId = null;
let airportGlideTarget = null;

function selectAircraft(id) {
  if (id === null || id === undefined) return false;
  const a = currentSnapshot.get(id);
  if (!a || !meshes.has(id)) return false;
  openRadio(a);
  startFollow(id);
  alerts.setActiveAircraft(id);
  return true;
}

function focusAirport(airportId) {
  const ap = airportGroups.get(airportId);
  if (!ap) return false;
  stopFollow(false);
  closeRadio();
  returnOverview = false;
  const [cx, cz] = ap.center;
  airportGlideTarget = {
    target: new THREE.Vector3(cx, 12, cz),
    cam: new THREE.Vector3(cx, 185, cz + 175),
  };
  return true;
}

const alerts = createAlertManager({
  onSelectAircraft: (id) => selectAircraft(id),
  onFocusAirport: (airportId) => focusAirport(airportId),
  onHoverAircraft: (id, prevId = null) => {
    if (id != null) {
      hoveredAlertAircraftId = id;
      hoveredAlertAirportId = null;
    } else if (prevId == null || hoveredAlertAircraftId === prevId) {
      hoveredAlertAircraftId = null;
    }
  },
  onHoverAirport: (airportId, prevId = null) => {
    if (airportId != null) {
      hoveredAlertAirportId = airportId;
      hoveredAlertAircraftId = null;
    } else if (prevId == null || hoveredAlertAirportId === prevId) {
      hoveredAlertAirportId = null;
    }
  },
});

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
  // Tankers and flagship 747s get a wider vantage so their larger airframes stay in frame.
  const dist = entry.kind === 'tanker' ? 92 : (entry.isFlagship ? 66 : (entry.kind === 'fighter' ? 40 : 54));
  const lift = entry.kind === 'tanker' ? 38 : (entry.isFlagship ? 28 : (entry.kind === 'fighter' ? 18 : 24));
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
  airportGlideTarget = null;
  updateFollowBadge(a);
}

function stopFollow(glideToOverview = false) {
  if (followId === null) return;
  followId = null;
  followLastPos = null;
  followSwoop = false;
  returnOverview = glideToOverview;
  airportGlideTarget = null;
  updateFollowBadge(null);
  alerts.setActiveAircraft(selectedAircraftId());
}

// Manual camera orbit/zoom cancels automatic swoop/return so controls feel responsive.
controls.addEventListener('start', () => {
  followSwoop = false;
  returnOverview = false;
  airportGlideTarget = null;
});

followEl.addEventListener('click', () => {
  stopFollow(true);
  closeRadio();
});

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
  if (!el || el.nodeType !== 1) return false;
  const tag = el.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || el.isContentEditable;
}

function isInteractiveTarget(el) {
  if (!el || el.nodeType !== 1) return false;
  return Boolean(el.closest('button, a[href], summary, [role="button"]'));
}

function releaseHeldKeys() {
  heldKeys.clear();
}

window.addEventListener('keydown', (ev) => {
  if (ev.key === 'Escape' && followId !== null) {
    stopFollow(true);
    closeRadio();
    return;
  }
  if (isTypingTarget(ev.target) || isTypingTarget(document.activeElement)) {
    releaseHeldKeys();
    return;
  }
  if (ev.ctrlKey || ev.metaKey || ev.altKey) return;
  if (ev.key === 'Enter'
      && !isInteractiveTarget(ev.target)
      && !isInteractiveTarget(document.activeElement)
      && focusRadioInput()) {
    ev.preventDefault();
    return;
  }
  if (ev.key === 'Home') {
    ev.preventDefault();
    stopFollow(false);
    airportGlideTarget = null;
    returnOverview = true;
    return;
  }
  const key = ev.key.length === 1 ? ev.key.toLowerCase() : ev.key;
  if (key === 'n') {
    ev.preventDefault();
    alerts.jumpToNextAttention(followId ?? selectedAircraftId());
    return;
  }
  if (!CAMERA_KEYS.has(key)) return;
  ev.preventDefault();
  heldKeys.add(key);
  // Manual input takes over from automatic swoop/return glides.
  followSwoop = false;
  returnOverview = false;
  airportGlideTarget = null;
});

window.addEventListener('keyup', (ev) => {
  const key = ev.key.length === 1 ? ev.key.toLowerCase() : ev.key;
  heldKeys.delete(key);
});
// Never leave rotation stuck: drop held keys whenever focus or visibility changes.
window.addEventListener('blur', releaseHeldKeys);
window.addEventListener('focusin', (ev) => { if (isTypingTarget(ev.target)) releaseHeldKeys(); });
document.addEventListener('visibilitychange', () => { if (document.hidden) releaseHeldKeys(); });

const _kbOffset = new THREE.Vector3();
const _kbSpherical = new THREE.Spherical();
const _kbForward = new THREE.Vector3();
const _kbRight = new THREE.Vector3();

function applyKeyboardCamera(frameDt) {
  if (heldKeys.size === 0) return;
  // Cap the step so a stalled frame cannot make the camera jump.
  const dt = Math.min(frameDt, 0.1);
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
    Math.max(0.08, controls.minPolarAngle),
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
    // Repaint when the thread switches model or airframe class (new livery / flair).
    if (meshes.has(a.id) && (
      meshes.get(a.id).model !== (a.model ?? null)
      || meshes.get(a.id).model_class !== a.model_class
    )) disposeMesh(a.id);
    const entry = ensureMesh(a);
    const y = worldYForAircraft(a, entry.wheelDrop);
    entry.target.set(a.position[0], y, a.position[1]);
    entry.targetHeading = a.heading;
    detectRunwayEvent(a, entry, lastStates.get(a.id), entry.altitude);
    const snapDx = a.position[0] - entry.snapX;
    const snapDz = a.position[1] - entry.snapZ;
    const snapDh = Math.hypot(snapDx, snapDz);
    const snapDAlt = a.position[2] - entry.altitude;
    const onGround = ['parked', 'ready', 'taxi', 'landed'].includes(a.state)
      || ((a.state === 'takeoff' || a.state === 'departing') && a.position[2] <= 0);
    if (onGround) {
      entry.flightPitch = 0;
    } else if (snapDh >= 0.05 && snapDh < 200 && Math.abs(snapDAlt) < 25) {
      // Negative rotation.x pitches the +Z nose UP; positive pitches nose DOWN.
      entry.flightPitch = -Math.atan2(snapDAlt, snapDh);
    }
    entry.snapX = a.position[0];
    entry.snapZ = a.position[1];
    entry.altitude = a.position[2];
    entry.speed = a.speed ?? 0;
    entry.landingPitch = a.landing_pitch ?? null;

    if (lastStates.get(a.id) !== a.state) {
      handleStateChange(a, lastStates.get(a.id), a.state);
      lastStates.set(a.id, a.state);
    }
    entry.state = a.state;
    setColor(entry, a);
    updateAircraftLabel(entry, a);
    entry.ring.visible = a.state === 'holding' || a.kind === 'tanker';
  }

  for (const id of [...meshes.keys()]) {
    if (!seen.has(id)) {
      disposeMesh(id);
      lastStates.delete(id);
      if (hoveredAlertAircraftId === id) hoveredAlertAircraftId = null;
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

  if (snapshot.omitted_threads) countEl.textContent += ` · ${snapshot.omitted_threads} threads outside display capacity`;

  const selectedId = selectedAircraftId();
  if (selectedId !== null) {
    const selected = aircraft.find((a) => a.id === selectedId);
    updateRadio(selected);
    if (selected === undefined) {
      closeRadio();
    }
  }

  alerts.syncSnapshot(snapshot, followId ?? selectedAircraftId());
}

/* ------------------------------------------------- takeoff / landing */

const eventBanner = document.getElementById('event-banner');
let eventBannerTimer = null;

function showEventBanner(a, kind, verb) {
  if (!eventBanner) return;
  eventBanner.className = kind;
  eventBanner.querySelector('.event-kind').textContent = verb;
  eventBanner.querySelector('.event-callsign').textContent = a.callsign;
  eventBanner.querySelector('.event-title').textContent = a.title ? formatTaskTitle(a) : '';
  eventBanner.dataset.aircraftId = String(a.id);
  eventBanner.hidden = false;
  // Restart the entrance animation for back-to-back events.
  void eventBanner.offsetWidth;
  eventBanner.classList.add('show');
  clearTimeout(eventBannerTimer);
  eventBannerTimer = setTimeout(() => eventBanner.classList.remove('show'), 5200);
}

if (eventBanner) {
  eventBanner.addEventListener('click', () => {
    selectAircraft(Number(eventBanner.dataset.aircraftId));
  });
}

function runwayEventsForAircraft(a) {
  if (a?.environment_id && airportGroups.has(a.environment_id)) {
    return airportGroups.get(a.environment_id).runwayEvents || runwayEvents;
  }
  if (a?.device) {
    for (const ap of airportGroups.values()) {
      if (ap.name === a.device && ap.runwayEvents) return ap.runwayEvents;
    }
  }
  return runwayEvents;
}

// Fire one-shot effects on takeoff roll, liftoff, approach and touchdown.
// `from` is undefined the first time an aircraft is seen (no event then).
function detectRunwayEvent(a, entry, from, prevAlt) {
  if (from === undefined) return;
  const [x, z, alt] = a.position;
  const rolling = a.state === 'takeoff' || a.state === 'departing';
  const rwy = runwayEventsForAircraft(a);

  if (rolling && from !== a.state && from !== 'takeoff' && alt <= 0) {
    rwy.takeoffRoll(x, z);
    showEventBanner(a, 'takeoff', a.state === 'departing' ? 'Final departure' : 'Departure');
  }
  if (rolling && prevAlt <= 0 && alt > 0) {
    rwy.liftoff(x, z, entry.group.position.y);
  }
  if (a.state === 'landing' && from !== 'landing') {
    showEventBanner(a, 'landing', 'Arrival · on approach');
  }
  if (from === 'landing' && a.state === 'landed') {
    rwy.touchdown(x, z);
    showEventBanner(a, 'landing', 'Touchdown');
  }
}

function handleStateChange(a, from, to) {
  const base = {
    aircraftId: a.id,
    callsign: a.callsign,
    title: a.title || '',
  };
  if (a.kind === 'tanker' && from === undefined) {
    alerts.addRoutineEvent({
      ...base,
      eventKey: 'tanker-station',
      reason: 'aerial refuelling tanker on station above airspace',
      kind: 'refuel',
    });
    return;
  }
  if (a.kind === 'fighter' && from === undefined) {
    alerts.addRoutineEvent({
      ...base,
      eventKey: 'wingman',
      reason: 'fighter wingman on station',
      kind: 'routine',
    });
    return;
  }
  if (to === 'holding') {
    if (isPassiveHold(a)) {
      alerts.addRoutineEvent({
        ...base,
        eventKey: `wait:${a.hold_reason}`,
        reason: a.hold_reason,
        kind: 'routine',
      });
    }
    // Approval / input / sign-in holds are displayed in the urgent Needs You stack.
  } else if (to === 'refueling') {
    // Active provider-limit waits stay visible in the informational refuelling strip.
  } else if (to === 'ready') {
    if (!classifyAircraftUrgency(a)) {
      alerts.addRoutineEvent({
        ...base,
        eventKey: 'queued',
        reason: a.hold_reason || 'queued on taxiway',
        kind: 'routine',
      });
    }
    // Failed runs (state=ready with failed reason) are displayed in the urgent Needs You stack.
  } else if (to === 'takeoff') {
    alerts.addRoutineEvent({ ...base, eventKey: 'takeoff', reason: 'rolling, Runway 36', kind: 'takeoff' });
  } else if (to === 'departing') {
    alerts.addRoutineEvent({ ...base, eventKey: 'departing', reason: 'thread done — departing into the horizon', kind: 'takeoff' });
  } else if (to === 'landing') {
    alerts.addRoutineEvent({ ...base, eventKey: 'landing', reason: 'on approach, Runway 36', kind: 'landing' });
  } else if (to === 'landed') {
    alerts.addRoutineEvent({ ...base, eventKey: 'landed', reason: 'touchdown', kind: 'landing' });
  } else if (to === 'taxi') {
    alerts.addRoutineEvent({ ...base, eventKey: 'taxi', reason: 'taxiing', kind: 'routine' });
  } else if (to === 'parked') {
    alerts.addRoutineEvent({ ...base, eventKey: 'parked', reason: 'parked at the gate', kind: 'routine' });
  } else if (to === 'cruising' && from === 'refueling') {
    alerts.addRoutineEvent({ ...base, eventKey: 'refuel-done', reason: 'provider wait ended — resuming patrol', kind: 'refuel' });
  } else if (to === 'cruising' && from === 'holding') {
    alerts.addRoutineEvent({ ...base, eventKey: 'hold-cleared', reason: 'hold cleared — resuming patrol', kind: 'takeoff' });
  } else if (to === 'cruising' && from !== undefined) {
    alerts.addRoutineEvent({ ...base, eventKey: 'cruising', reason: 'cruising', kind: 'routine' });
  }
  if (selectedAircraftId() === a.id && from !== undefined) {
    const note = {
      holding: 'entering hold',
      refueling: 'connecting to aerial tanker (waiting for tokens)',
      takeoff: 'taking off',
      departing: 'departing into the horizon',
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
const CLICK_DRAG_SQ = 36; // 6px movement threshold distinguishing click from orbit drag
const MIN_SCREEN_PICK_PX = 20;
const _pickWorld = new THREE.Vector3();
const _pickNdc = new THREE.Vector3();

let downPos = null;
let downPointerId = null;
let pointerDragged = false;

function isVisibleInHierarchy(obj, root) {
  let cur = obj;
  while (cur) {
    if (!cur.visible) return false;
    if (cur === root) return true;
    cur = cur.parent;
  }
  return true;
}

function pickAircraftAt(clientX, clientY) {
  const rect = canvas.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return null;
  const localX = clientX - rect.left;
  const localY = clientY - rect.top;

  pointer.set(
    (localX / rect.width) * 2 - 1,
    -(localY / rect.height) * 2 + 1,
  );
  camera.updateMatrixWorld();
  raycaster.setFromCamera(pointer, camera);

  // Tier 1: exact raycast on visible airframe meshes + visible labels.
  // Tier 2: per-aircraft invisible airframe hit proxy (fuselage/wings/tail envelope).
  // Both tiers choose the NEAREST aircraft along the ray.
  let bestDirectId = null;
  let bestDirectDist = Infinity;
  let bestProxyId = null;
  let bestProxyDist = Infinity;

  for (const [id, entry] of meshes.entries()) {
    if (!entry.group.visible) continue;
    entry.group.updateMatrixWorld();

    const hits = raycaster.intersectObject(entry.group, true);
    for (const h of hits) {
      const obj = h.object;
      if (obj === entry.ring || obj === entry.hoverRing) continue;
      if (obj === entry.hitProxy || obj.userData?.isHitProxy) {
        if (h.distance < bestProxyDist) {
          bestProxyDist = h.distance;
          bestProxyId = id;
        }
        continue;
      }
      if (obj === entry.label && entry.label.material.opacity <= 0.05) continue;
      if (!isVisibleInHierarchy(obj, entry.group)) continue;
      if (h.distance < bestDirectDist) {
        bestDirectDist = h.distance;
        bestDirectId = id;
      }
      break;
    }
  }

  if (bestDirectId !== null) return bestDirectId;
  if (bestProxyId !== null) return bestProxyId;

  // Tier 3: minimum screen-space pick radius for small/distant aircraft in overview.
  let bestScreenId = null;
  let bestScreenDepth = Infinity;
  const maxSq = MIN_SCREEN_PICK_PX * MIN_SCREEN_PICK_PX;

  for (const [id, entry] of meshes.entries()) {
    if (!entry.group.visible) continue;
    if (entry.airframeCenter) {
      _pickWorld.copy(entry.airframeCenter).applyMatrix4(entry.group.matrixWorld);
    } else {
      _pickWorld.copy(entry.group.position);
    }
    _pickNdc.copy(_pickWorld).project(camera);
    if (_pickNdc.z < -1 || _pickNdc.z > 1) continue;
    const sx = (_pickNdc.x * 0.5 + 0.5) * rect.width;
    const sy = (-_pickNdc.y * 0.5 + 0.5) * rect.height;
    const ddx = sx - localX;
    const ddy = sy - localY;
    if (ddx * ddx + ddy * ddy <= maxSq) {
      const camDist = camera.position.distanceTo(_pickWorld);
      if (camDist < bestScreenDepth) {
        bestScreenDepth = camDist;
        bestScreenId = id;
      }
    }
  }

  return bestScreenId;
}

canvas.addEventListener('pointerdown', (ev) => {
  if (ev.button !== 0) return;
  if (downPointerId !== null && ev.pointerId !== downPointerId) {
    pointerDragged = true;
    return;
  }
  downPos = [ev.clientX, ev.clientY];
  downPointerId = ev.pointerId;
  pointerDragged = false;
});
canvas.addEventListener('pointermove', (ev) => {
  if (!downPos || ev.pointerId !== downPointerId || pointerDragged) return;
  const dx = ev.clientX - downPos[0];
  const dy = ev.clientY - downPos[1];
  if (dx * dx + dy * dy > CLICK_DRAG_SQ) pointerDragged = true;
});
canvas.addEventListener('pointercancel', (ev) => {
  if (ev.pointerId === downPointerId) {
    downPos = null;
    downPointerId = null;
    pointerDragged = false;
  }
});
canvas.addEventListener('pointerup', (ev) => {
  if (!downPos || ev.pointerId !== downPointerId) return;
  const dx = ev.clientX - downPos[0];
  const dy = ev.clientY - downPos[1];
  const wasDrag = pointerDragged || (dx * dx + dy * dy > CLICK_DRAG_SQ);
  downPos = null;
  downPointerId = null;
  pointerDragged = false;
  if (wasDrag) return;

  const id = pickAircraftAt(ev.clientX, ev.clientY);
  if (id === null) {
    // Clicking empty sky/water disengages follow cam.
    if (followId !== null) {
      stopFollow(false);
      closeRadio();
    }
    return;
  }
  selectAircraft(id);
});

/* ----------------------------------------------------------- main loop */

const currentSnapshot = new Map();
const clock = new THREE.Clock();

function animate() {
  requestAnimationFrame(animate);
  const dt = clock.getDelta();
  const k = 1 - Math.exp(-INTERP_RATE * dt);

  const now = performance.now() / 1000;
  const reduceMotion = reducedMotionQuery.matches;
  for (const [id, entry] of meshes.entries()) {
    entry.group.position.y -= entry.clearanceAdjustment || 0;
    entry.clearanceAdjustment = 0;
    const wrapped = entry.state !== 'departing' && entry.group.position.distanceToSquared(entry.target) > 40000;
    if (wrapped) {
      entry.group.position.copy(entry.target);
      if (entry.contrails) for (const c of entry.contrails) c.clear();
      if (entry.localFlair) entry.localFlair.reset();
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
    const alt = entry.altitude || 0;
    const takeoffAirborne = entry.state === 'takeoff' && alt > 0;
    const departingAirborne = entry.state === 'departing' && alt > 0;
    const airborne = ['cruising', 'holding', 'refueling', 'landing'].includes(entry.state)
      || takeoffAirborne || departingAirborne;
    const omega = dt > 0.001 ? stepH / dt : 0;
    const maxBank = entry.kind === 'tanker'
      ? 0.22
      : (entry.isFlagship ? 0.26 : (entry.kind === 'fighter' ? 0.52 : (entry.model_class === 'frontier' ? 0.32 : 0.4)));
    const targetRoll = airborne
      ? Math.max(-maxBank, Math.min(maxBank, omega * 1.15))
      : 0;
    entry.roll += (targetRoll - entry.roll) * (1 - Math.exp(-5 * dt));

    const stunt = entry.localFlair ? entry.localFlair.updateStunt(entry, dt, airborne) : null;
    const totalRoll = entry.roll + (stunt ? stunt.roll : 0);
    entry.group.rotation.z = totalRoll;

    // Pitch along the actual climb/descent flight path (negative X pitches +Z nose up,
    // positive X pitches nose down), with pre-liftoff rotation and a short touchdown flare.
    const maxClimb = entry.kind === 'fighter' ? 0.5 : (entry.isFlagship ? 0.34 : (entry.model_class === 'frontier' ? 0.38 : 0.42));
    const maxDescent = entry.kind === 'fighter' ? 0.44 : (entry.isFlagship ? 0.30 : (entry.model_class === 'frontier' ? 0.34 : 0.36));
    let targetPitch = 0;
    if (airborne) {
      targetPitch = THREE.MathUtils.clamp((entry.flightPitch || 0) * 1.15, -maxClimb, maxDescent);
      if (entry.state === 'landing' && entry.landingPitch !== null) {
        targetPitch = -entry.landingPitch;
      } else if (entry.state === 'landing' && alt < 6 && targetPitch > 0) {
        // Flare gently out of the nose-down glideslope over the threshold so main gear touches first.
        const f = Math.max(0, alt / 6);
        targetPitch = -0.05 * (1 - f) + targetPitch * f;
      }
    } else if ((entry.state === 'takeoff' || entry.state === 'departing') && alt <= 0) {
      // Begin rotating nose up at the end of the takeoff roll just before liftoff.
      const rotateSpeed = (entry.model_class === 'frontier' ? 34 : 25) * 0.82;
      if (entry.speed >= rotateSpeed) {
        targetPitch = entry.kind === 'drone' ? -0.06 : -0.11;
      }
    }
    const pitchRate = (entry.state === 'takeoff' || entry.state === 'departing' || entry.state === 'landing' || entry.state === 'landed')
      ? 5.5
      : 4.0;
    const pitchDelta = (targetPitch - (entry.pitch || 0)) * (1 - Math.exp(-pitchRate * dt));
    const landingRate = entry.state === 'landed' ? 0.035 : 0.12;
    entry.pitch = (entry.pitch || 0) + (['landing', 'landed'].includes(entry.state)
      ? THREE.MathUtils.clamp(pitchDelta, -landingRate * dt, landingRate * dt)
      : pitchDelta);
    entry.group.rotation.x = entry.pitch + (stunt ? stunt.pitch : 0);

    // Preserve wheel/deck clearance as the airframe rotates around its origin.
    // Remove this adjustment before the next interpolation step to avoid drift.
    if (['landing', 'landed'].includes(entry.state)) {
      const sx = Math.sin(entry.pitch), cx = Math.cos(entry.pitch);
      const sz = Math.sin(totalRoll), cz = Math.cos(totalRoll);
      let bottom = Infinity;
      for (const p of entry.contactPoints) {
        bottom = Math.min(bottom, cx * (sz * p.x + cz * p.y) - sx * p.z);
      }
      entry.clearanceAdjustment = -bottom * entry.scale - entry.wheelDrop;
      entry.group.position.y += entry.clearanceAdjustment;
    }

    // Keep floating label hovering upright above the fuselage during barrel rolls.
    if (stunt && stunt.roll !== 0) {
      entry.label.position.set(
        Math.sin(totalRoll) * entry.labelBaseY,
        Math.cos(totalRoll) * entry.labelBaseY,
        0,
      );
    } else if (entry.label.position.x !== 0 || entry.label.position.y !== entry.labelBaseY) {
      entry.label.position.set(0, entry.labelBaseY, 0);
    }

    // Fade the floating label smoothly as a departing aircraft vanishes into the distant haze.
    if (entry.state === 'departing') {
      const dist = Math.hypot(entry.group.position.x, entry.group.position.z);
      entry.label.material.opacity = Math.max(0, Math.min(1, 1 - (dist - 850) / 650));
    } else if (entry.label.material.opacity !== 1) {
      entry.label.material.opacity = 1;
    }

    // Landing & taxi lights: soft taxi beam while moving on taxiway or lined
    // up at the runway, full bright beam during takeoff, climb-out, approach,
    // and landing rollout.
    if (entry.landingLights) {
      const st = entry.state;
      let lightTarget = 0;
      if (st === 'takeoff' || st === 'landing'
        || (st === 'departing' && alt < 160)
        || (st === 'landed' && entry.speed > 0.5)) {
        lightTarget = 1;
      } else if (st === 'taxi') {
        lightTarget = 0.45;
      } else if (st === 'ready'
        && Math.abs(entry.group.position.x - 56) < 22
        && entry.group.position.z > -80
        && entry.group.position.z < 65) {
        lightTarget = 0.55;
      }
      entry.landingLights.setLevel(lightTarget, dt);
    }
    // Dust and spray thrown up behind a fast ground roll or rollout.
    const groundRoll = ((entry.state === 'takeoff' || entry.state === 'departing') && alt <= 0)
      || entry.state === 'landed';
    if (groundRoll && entry.speed > 6) {
      entry.dustClock += dt;
      if (entry.dustClock > 0.06) {
        entry.dustClock = 0;
        const p = entry.group.position;
        runwayEvents.rollDust(p.x, p.z, entry.heading);
      }
    }

    // Update contrail ribbons (emit only when airborne above ground).
    if (entry.contrails) {
      const emit = airborne && entry.group.position.y > 25;
      for (const c of entry.contrails) {
        c.update(entry.group, dt, emit);
      }
    }

    // Update local-model towed sky-banner & coloured airshow smoke bursts.
    if (entry.localFlair) {
      entry.localFlair.updateVisuals(entry, dt, now, airborne);
    }

    if (entry.prop && entry.state !== 'parked' && entry.state !== 'landed') {
      entry.prop.rotation.z = now * 30;
    }

    // Retract landing gear while en route or climbing away after takeoff.
    if (entry.gear) {
      entry.gear.visible =
        entry.state !== 'cruising'
        && entry.state !== 'holding'
        && entry.state !== 'refueling'
        && !((entry.state === 'departing' || entry.state === 'takeoff') && alt > 18)
        && !(entry.state === 'landing' && alt > 70);
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
      const pulse = reduceMotion ? 1.05 : 1 + 0.25 * Math.sin(now * 4);
      const baseScale = entry.kind === 'tanker' ? 1.85 : 1;
      entry.ring.scale.setScalar(pulse * baseScale);
      entry.ring.position.y = (0.8 - entry.group.position.y) / entry.scale;
      entry.ring.material.opacity = reduceMotion ? 0.72 : 0.5 + 0.35 * Math.sin(now * 4);
    }

    const isAlertHovered = hoveredAlertAircraftId === id;
    if (entry.hoverRing) {
      entry.hoverRing.visible = isAlertHovered;
      if (isAlertHovered) {
        const hoverPulse = reduceMotion ? 1.08 : 1.05 + 0.16 * Math.sin(now * 6);
        const hoverBase = entry.kind === 'tanker' ? 1.55 : 1;
        entry.hoverRing.scale.setScalar(hoverPulse * hoverBase);
        entry.hoverRing.material.opacity = reduceMotion ? 0.92 : 0.76 + 0.22 * Math.sin(now * 6);
      }
    }
    if (entry.labelBaseScale) {
      const factor = isAlertHovered ? 1.28 : 1;
      const targetX = entry.labelBaseScale.x * factor;
      const targetY = entry.labelBaseScale.y * factor;
      if (reduceMotion) {
        entry.label.scale.set(targetX, targetY, 1);
      } else {
        const lk = 1 - Math.exp(-12 * dt);
        entry.label.scale.x += (targetX - entry.label.scale.x) * lk;
        entry.label.scale.y += (targetY - entry.label.scale.y) * lk;
      }
    }
  }

  updateRefuelPipes(now);

  const approaching = new Set();
  for (const a of currentSnapshot.values()) {
    if (a.state === 'landing') {
      const rwy = runwayEventsForAircraft(a);
      approaching.add(rwy);
    }
  }
  for (const ap of airportGroups.values()) {
    const approachActive = ap.runwayEvents ? approaching.has(ap.runwayEvents) : false;
    const busy = ap.runwayEvents
      ? ap.runwayEvents.update(dt, now, { approachActive })
      : 0;
    if (ap.lights) ap.lights.setBoost(Math.max(busy, approachActive ? 0.35 : 0));
  }
  if (airportGroups.size === 0) {
    runwayEvents.update(dt, now, { approachActive: false });
  }

  for (const radar of radars) {
    radar.rotation.y = now * 0.8;
  }
  updateAlpine(now);

  for (const ap of airportGroups.values()) {
    const isApHovered = hoveredAlertAirportId === ap.id;
    const basePulse = ap.online ? (reduceMotion ? 1 : 0.85 + 0.3 * Math.sin(now * 3)) : 0.7;
    const pulse = isApHovered ? (reduceMotion ? 1.7 : 1.5 + 0.35 * Math.sin(now * 6)) : basePulse;
    ap.beacon.scale.setScalar(pulse);
    ap.lights.update(now);
  }

  // Follow camera: track the selected aircraft while keeping OrbitControls active.
  const camDt = Math.min(dt, 0.1);
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

      const camK = 1 - Math.exp(-5.5 * camDt);
      controls.target.lerp(pos, camK);

      if (followSwoop) {
        const desiredCam = pos.clone().add(chaseOffsetFor(entry));
        camera.position.lerp(desiredCam, camK);
        if (camera.position.distanceTo(desiredCam) < 1.5 && controls.target.distanceTo(pos) < 0.8) {
          followSwoop = false;
        }
      }
    }
  } else if (airportGlideTarget) {
    const camK = 1 - Math.exp(-5.0 * camDt);
    camera.position.lerp(airportGlideTarget.cam, camK);
    controls.target.lerp(airportGlideTarget.target, camK);
    if (camera.position.distanceTo(airportGlideTarget.cam) < 2.0
        && controls.target.distanceTo(airportGlideTarget.target) < 1.0) {
      airportGlideTarget = null;
    }
  } else if (returnOverview) {
    const camK = 1 - Math.exp(-4.5 * camDt);
    camera.position.lerp(OVERVIEW_CAM, camK);
    controls.target.lerp(OVERVIEW_TARGET, camK);
    if (camera.position.distanceTo(OVERVIEW_CAM) < 2.0 && controls.target.distanceTo(OVERVIEW_TARGET) < 1.0) {
      returnOverview = false;
    }
  }

  applyKeyboardCamera(dt);
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
