// Swiss alpine scenery: faceted low-poly island terrain (pebble shores,
// meadows, craggy snow-capped peaks), lake shallows, fir forests, chalets,
// boulders, waving Swiss flags, and a ring of mountains around the lake.
//
// Everything is deterministic (seeded), and vegetation per island is merged
// into a single vertex-coloured mesh so the whole archipelago costs only a
// handful of draw calls.

import * as THREE from './vendor/three.module.js';

/* ------------------------------------------------------------ palette */

export const SWISS_RED = 0xda291c;

const C = {
  pebble: new THREE.Color(0xb9b2a2),
  pebbleDark: new THREE.Color(0x8f897d),
  meadow: new THREE.Color(0x5e9b3c),
  meadowLush: new THREE.Color(0x4b8a33),
  meadowDry: new THREE.Color(0x93a94a),
  flowers: new THREE.Color(0xb6b94e),
  mown: new THREE.Color(0x6eab4c),
  forestFloor: new THREE.Color(0x3d6a2c),
  rock: new THREE.Color(0x857f76),
  rockDark: new THREE.Color(0x66625c),
  snow: new THREE.Color(0xf3f6f9),
  lakebed: new THREE.Color(0x56664f),
  fir: new THREE.Color(0x2b5634),
  firDark: new THREE.Color(0x1f4329),
  leaf: new THREE.Color(0x4f8f3a),
  trunk: new THREE.Color(0x5e432b),
  plaster: new THREE.Color(0xf1ece0),
  wood: new THREE.Color(0x8a5630),
  roof: new THREE.Color(0x4a3426),
  shutter: new THREE.Color(SWISS_RED),
};

/* ---------------------------------------------------------- noise utils */

function mulberry32(seed) {
  let a = Math.floor(seed * 1e6) >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function hash2(ix, iz, seed) {
  let h = Math.imul(ix, 374761393) + Math.imul(iz, 668265263) + Math.imul(Math.floor(seed * 1000), 982451653);
  h = Math.imul(h ^ (h >>> 13), 1274126177);
  h ^= h >>> 16;
  return (h >>> 0) / 4294967296;
}

function valueNoise(x, z, seed) {
  const ix = Math.floor(x);
  const iz = Math.floor(z);
  const fx = x - ix;
  const fz = z - iz;
  const ux = fx * fx * (3 - 2 * fx);
  const uz = fz * fz * (3 - 2 * fz);
  const a = hash2(ix, iz, seed);
  const b = hash2(ix + 1, iz, seed);
  const c = hash2(ix, iz + 1, seed);
  const d = hash2(ix + 1, iz + 1, seed);
  return a + (b - a) * ux + (c - a) * uz + (a - b - c + d) * ux * uz;
}

function fbm(x, z, seed, octaves = 4) {
  let sum = 0;
  let amp = 0.5;
  let freq = 1;
  let norm = 0;
  for (let i = 0; i < octaves; i++) {
    sum += valueNoise(x * freq, z * freq, seed + i * 17.3) * amp;
    norm += amp;
    amp *= 0.5;
    freq *= 2.03;
  }
  return sum / norm;
}

// Ridged noise for craggy mountain crests.
function ridged(x, z, seed, octaves = 4) {
  let sum = 0;
  let amp = 0.5;
  let freq = 1;
  let norm = 0;
  for (let i = 0; i < octaves; i++) {
    const n = 1 - Math.abs(valueNoise(x * freq, z * freq, seed + i * 11.1) * 2 - 1);
    sum += n * n * amp;
    norm += amp;
    amp *= 0.5;
    freq *= 2.1;
  }
  return sum / norm;
}

const smoothstep = (e0, e1, x) => {
  const t = Math.min(1, Math.max(0, (x - e0) / (e1 - e0)));
  return t * t * (3 - 2 * t);
};

const lerp = (a, b, t) => a + (b - a) * t;

// Polynomial smooth max (k = blend width).
function smax(a, b, k) {
  const h = Math.max(k - Math.abs(a - b), 0) / k;
  return Math.max(a, b) + h * h * k * 0.25;
}

// Signed distance to an axis-aligned rectangle (negative inside).
function sdRect(px, pz, r) {
  const dx = Math.abs(px - r.x) - r.hw;
  const dz = Math.abs(pz - r.z) - r.hl;
  return Math.hypot(Math.max(dx, 0), Math.max(dz, 0)) + Math.min(Math.max(dx, dz), 0);
}

/* ------------------------------------------------------- island terrain */

const SHORE_WIDTH = 7; // units from waterline to the meadow lip
const SHORE_TOP = 1.7; // meadow lip height above the lake
const PAD_Y = 2.0; // airfield ground height (deck foundations sit on this)

/**
 * Build a field for one island, in the island group's local coordinates.
 *
 * spec = {
 *   seed, center: [x, z], radius,
 *   pads:  [{ x, z, hw, hl }]            flattened airfield footprints,
 *   peaks: [{ x, z, h, r, flat, crag }]  hills / mountains (flat top radius),
 *   hilliness, snowLine,
 *   keepClear: [{ x, z, r }]             no vegetation (towers, flags),
 * }
 */
function makeField(spec) {
  const [cx, cz] = spec.center;
  const s = spec.seed;
  const hilliness = spec.hilliness ?? 4;

  // Distance inland from the coastline (negative = in the lake).
  const coastDist = (x, z) => {
    const dx = x - cx;
    const dz = z - cz;
    const a = Math.atan2(dz, dx);
    const wobble = 1
      + 0.09 * Math.sin(a * 3 + s * 1.9)
      + 0.05 * Math.sin(a * 5 + s * 3.1)
      + 0.03 * Math.sin(a * 9 + s * 0.7)
      + 0.16 * (fbm(x * 0.018, z * 0.018, s + 3) - 0.5);
    let c = spec.radius * wobble - Math.hypot(dx, dz);
    // Airfields are always surrounded by at least 16 units of dry land.
    for (const p of spec.pads) c = smax(c, 16 - sdRect(x, z, p), 14);
    return c;
  };

  const height = (x, z) => {
    const c = coastDist(x, z);
    let h;
    if (c < 0) {
      h = Math.max(-9, c * 0.45); // shelving lakebed
    } else if (c < SHORE_WIDTH) {
      const t = c / SHORE_WIDTH;
      h = SHORE_TOP * t * t * (3 - 2 * t);
    } else {
      const inland = smoothstep(SHORE_WIDTH, SHORE_WIDTH + 35, c);
      const roll = (fbm(x * 0.022, z * 0.022, s + 9) - 0.42) * hilliness * 2;
      h = SHORE_TOP + Math.max(0, roll) * inland;
    }

    // Peaks: flat-topped where a tower stands, craggy everywhere else.
    for (const p of spec.peaks) {
      const d = Math.hypot(x - p.x, z - p.z);
      if (d > p.r * 1.3) continue;
      const crag = p.crag ?? 0.35;
      const warp = 1 + (fbm(x * 0.06, z * 0.06, s + 21) - 0.5) * crag;
      const t = 1 - smoothstep(p.flat ?? 0, p.r * warp, d);
      const rough = (ridged(x * 0.05, z * 0.05, s + 33) - 0.5) * p.h * crag * 0.5;
      // Skirt starts 2.5 below the lake so peaks rise out of the water
      // cleanly instead of leaving a shallow, foamy apron.
      const ph = -2.5 + (p.h + 2.5) * t * t * (3 - 2 * t)
        + (d > (p.flat ?? 0) + 2 ? rough * t : 0);
      h = smax(h, ph, 3);
    }

    // Flatten airfield footprints (with a soft verge into the countryside).
    for (const p of spec.pads) {
      const w = 1 - smoothstep(4, 22, sdRect(x, z, p));
      if (w > 0) h = lerp(h, PAD_Y, w);
    }
    return h;
  };

  const padWeight = (x, z) => {
    let w = 0;
    for (const p of spec.pads) w = Math.max(w, 1 - smoothstep(0, 18, sdRect(x, z, p)));
    return w;
  };

  return { coastDist, height, padWeight, spec };
}

// Colour for a terrain face from its centroid height, slope and position.
function faceColor(field, x, z, h, normalY, out) {
  const s = field.spec.seed;
  const n = fbm(x * 0.045, z * 0.045, s + 51);
  const fine = hash2(Math.floor(x * 0.7), Math.floor(z * 0.7), s + 7);
  const snowLine = field.spec.snowLine ?? Infinity;

  if (h < -0.15) {
    out.copy(C.lakebed);
  } else if (h < SHORE_TOP - 0.25) {
    out.copy(C.pebble).lerp(C.pebbleDark, fine * 0.6);
  } else if (h > snowLine + (n - 0.5) * 6 && normalY > 0.55) {
    out.copy(C.snow);
  } else if (normalY < 0.72) {
    out.copy(C.rock).lerp(C.rockDark, fine);
  } else {
    // Meadow: lush and dry patches, wildflower speckle, forest floor clumps.
    out.copy(C.meadow).lerp(C.meadowLush, smoothstep(0.35, 0.65, n));
    const dry = fbm(x * 0.02, z * 0.02, s + 77);
    out.lerp(C.meadowDry, smoothstep(0.55, 0.8, dry) * 0.6);
    if (fine > 0.93) out.lerp(C.flowers, 0.55);
    out.lerp(C.mown, field.padWeight(x, z) * 0.85);
    if (h > snowLine - 8) out.lerp(C.rock, smoothstep(snowLine - 8, snowLine, h) * 0.5);
  }
  // Subtle per-face shimmer so flat areas still read as facets.
  const jitter = 0.94 + fine * 0.1;
  out.r *= jitter;
  out.g *= jitter;
  out.b *= jitter;
  return out;
}

// Triangulate the field on a jittered grid; flat-shaded, vertex-coloured.
function buildTerrainMesh(field, extent, cell) {
  const [cx, cz] = field.spec.center;
  const seg = Math.ceil((extent * 2) / cell);
  const s = field.spec.seed;
  const vx = new Float32Array((seg + 1) * (seg + 1));
  const vz = new Float32Array((seg + 1) * (seg + 1));
  const vy = new Float32Array((seg + 1) * (seg + 1));
  for (let j = 0; j <= seg; j++) {
    for (let i = 0; i <= seg; i++) {
      const k = j * (seg + 1) + i;
      const edge = i === 0 || j === 0 || i === seg || j === seg;
      const jx = edge ? 0 : (hash2(i, j, s + 1) - 0.5) * cell * 0.55;
      const jz = edge ? 0 : (hash2(i, j, s + 2) - 0.5) * cell * 0.55;
      const x = cx - extent + i * cell + jx;
      const z = cz - extent + j * cell + jz;
      vx[k] = x;
      vz[k] = z;
      vy[k] = field.height(x, z);
    }
  }

  const pos = [];
  const col = [];
  const shallow = [];
  const color = new THREE.Color();
  const a = new THREE.Vector3();
  const b = new THREE.Vector3();
  const c = new THREE.Vector3();
  const nrm = new THREE.Vector3();
  const edge = new THREE.Vector3();
  const tri = (k0, k1, k2) => {
    const y0 = vy[k0];
    const y1 = vy[k1];
    const y2 = vy[k2];
    const maxY = Math.max(y0, y1, y2);
    const mx = (vx[k0] + vx[k1] + vx[k2]) / 3;
    const mz = (vz[k0] + vz[k1] + vz[k2]) / 3;
    const my = (y0 + y1 + y2) / 3;
    // Shallows ribbon: lake triangles near the shore get a turquoise wash.
    if (maxY < 0.4 && my > -7) shallow.push(k0, k1, k2);
    if (maxY < -1.2) return; // hidden under the opaque lake: skip
    a.set(vx[k0], y0, vz[k0]);
    b.set(vx[k1], y1, vz[k1]);
    c.set(vx[k2], y2, vz[k2]);
    nrm.subVectors(c, b).cross(edge.subVectors(a, b)).normalize();
    faceColor(field, mx, mz, my, Math.abs(nrm.y), color);
    for (const v of [a, b, c]) {
      pos.push(v.x, v.y, v.z);
      col.push(color.r, color.g, color.b);
    }
  };
  for (let j = 0; j < seg; j++) {
    for (let i = 0; i < seg; i++) {
      const k00 = j * (seg + 1) + i;
      const k10 = k00 + 1;
      const k01 = k00 + seg + 1;
      const k11 = k01 + 1;
      if ((i + j) % 2 === 0) {
        tri(k00, k01, k10);
        tri(k10, k01, k11);
      } else {
        tri(k00, k01, k11);
        tri(k00, k11, k10);
      }
    }
  }

  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
  geo.setAttribute('color', new THREE.Float32BufferAttribute(col, 3));
  geo.computeVertexNormals();
  const terrain = new THREE.Mesh(geo, new THREE.MeshStandardMaterial({
    vertexColors: true, flatShading: true, roughness: 0.95, metalness: 0,
  }));
  terrain.name = 'terrain';
  terrain.castShadow = true; // peaks shade their own meadows
  terrain.receiveShadow = true;

  // Shallows + foam: translucent overlay floating just above the lake.
  const sp = [];
  const sc = [];
  const shallowCol = new THREE.Color(0x4fc2c0);
  const foamCol = new THREE.Color(0xeef8f6);
  const tmp = new THREE.Color();
  for (const k of shallow) {
    const depth = vy[k];
    sp.push(vx[k], 0.12, vz[k]);
    const foam = smoothstep(-0.55, -0.05, depth) * (1 - smoothstep(0.2, 0.4, depth));
    tmp.copy(shallowCol).lerp(foamCol, foam * 0.85);
    const alpha = Math.max(foam * 0.8, smoothstep(-7, -0.5, depth) * 0.62);
    sc.push(tmp.r, tmp.g, tmp.b, alpha);
  }
  const sgeo = new THREE.BufferGeometry();
  sgeo.setAttribute('position', new THREE.Float32BufferAttribute(sp, 3));
  sgeo.setAttribute('color', new THREE.Float32BufferAttribute(sc, 4));
  const shallows = new THREE.Mesh(sgeo, new THREE.MeshBasicMaterial({
    vertexColors: true, transparent: true, depthWrite: false,
  }));
  shallows.renderOrder = 1;
  shallows.name = 'shallows';

  return { terrain, shallows };
}

/* ---------------------------------------------------------- vegetation */

// Accumulates transformed, coloured, non-indexed geometry into one mesh.
class MergeBuffer {
  constructor() {
    this.pos = [];
    this.col = [];
  }

  add(geo, matrix, color, variance = 0, rng = Math.random) {
    const g = geo.index ? geo.toNonIndexed() : geo;
    const p = g.attributes.position;
    const v = new THREE.Vector3();
    const tint = new THREE.Color();
    for (let i = 0; i < p.count; i++) {
      if (i % 3 === 0) {
        const f = 1 + (rng() - 0.5) * variance;
        tint.setRGB(color.r * f, color.g * f, color.b * f);
      }
      v.fromBufferAttribute(p, i).applyMatrix4(matrix);
      this.pos.push(v.x, v.y, v.z);
      this.col.push(tint.r, tint.g, tint.b);
    }
    if (g !== geo) g.dispose();
  }

  build() {
    const geo = new THREE.BufferGeometry();
    geo.setAttribute('position', new THREE.Float32BufferAttribute(this.pos, 3));
    geo.setAttribute('color', new THREE.Float32BufferAttribute(this.col, 3));
    geo.computeVertexNormals();
    return new THREE.Mesh(geo, new THREE.MeshStandardMaterial({
      vertexColors: true, flatShading: true, roughness: 0.85, metalness: 0,
    }));
  }
}

// Shared unit primitives (cloned into the merge buffers via matrices).
const PRIM = {
  trunk: new THREE.CylinderGeometry(0.28, 0.4, 1, 5),
  cone: new THREE.ConeGeometry(1, 1, 7),
  blob: new THREE.IcosahedronGeometry(1, 0),
  rock: new THREE.DodecahedronGeometry(1, 0),
  box: new THREE.BoxGeometry(1, 1, 1),
  roof: (() => {
    // Triangular prism along Z: a gable roof with overhanging eaves.
    const shape = new THREE.Shape();
    shape.moveTo(-0.5, 0);
    shape.lineTo(0.5, 0);
    shape.lineTo(0, 0.5);
    shape.closePath();
    const g = new THREE.ExtrudeGeometry(shape, { depth: 1, bevelEnabled: false });
    g.translate(0, 0, -0.5);
    return g;
  })(),
};

const M = new THREE.Matrix4();
const Q = new THREE.Quaternion();
const E = new THREE.Euler();
const P = new THREE.Vector3();
const S = new THREE.Vector3();

function place(x, y, z, sx, sy, sz, rx = 0, ry = 0, rz = 0) {
  E.set(rx, ry, rz);
  Q.setFromEuler(E);
  P.set(x, y, z);
  S.set(sx, sy, sz);
  return M.compose(P, Q, S);
}

function addFir(buf, x, y, z, size, rng, snowy) {
  const trunkH = 1.6 * size;
  buf.add(PRIM.trunk, place(x, y + trunkH / 2 - 0.3, z, size, trunkH, size), C.trunk, 0.1, rng);
  const tiers = 3;
  const green = rng() < 0.5 ? C.fir : C.firDark;
  for (let t = 0; t < tiers; t++) {
    const r = (2.3 - t * 0.55) * size;
    const h = (3.4 - t * 0.5) * size;
    const ty = y + trunkH + t * 1.9 * size + h / 2 - 0.4 * size;
    const tierColor = snowy && t === tiers - 1 ? C.snow : green;
    buf.add(PRIM.cone, place(x, ty, z, r, h, r, 0, rng() * 6.28), tierColor, 0.12, rng);
  }
}

function addLeafTree(buf, x, y, z, size, rng) {
  const trunkH = 2.2 * size;
  buf.add(PRIM.trunk, place(x, y + trunkH / 2 - 0.3, z, size * 1.1, trunkH, size * 1.1), C.trunk, 0.1, rng);
  buf.add(PRIM.blob, place(x, y + trunkH + 1.6 * size, z, 2.2 * size, 2.0 * size, 2.2 * size, rng(), rng(), rng()), C.leaf, 0.18, rng);
  buf.add(PRIM.blob, place(x + 0.9 * size, y + trunkH + 1.0 * size, z + 0.5 * size, 1.4 * size, 1.3 * size, 1.4 * size, rng(), rng(), 0), C.leaf, 0.18, rng);
}

function addRock(buf, x, y, z, size, rng) {
  buf.add(PRIM.rock, place(x, y + size * 0.15, z, size * (0.8 + rng() * 0.5), size * (0.5 + rng() * 0.4), size * (0.8 + rng() * 0.5), rng() * 3, rng() * 3, rng() * 3), rng() < 0.5 ? C.rock : C.pebbleDark, 0.15, rng);
}

// A small Swiss chalet: stone/plaster ground floor, timber upper storey,
// deep overhanging gable roof and red shutters.
function addChalet(buf, x, y, z, size, rot, rng) {
  const w = 5.5 * size;
  const l = 7 * size;
  const cos = Math.cos(rot);
  const sin = Math.sin(rot);
  const at = (lx, lz) => [x + lx * cos + lz * sin, z - lx * sin + lz * cos];
  buf.add(PRIM.box, place(x, y + 1.0 * size - 0.6, z, w, 2.0 * size + 1.2, l, 0, rot), C.plaster, 0.04, rng);
  buf.add(PRIM.box, place(x, y + 3.1 * size, z, w * 0.98, 2.2 * size, l * 0.98, 0, rot), C.wood, 0.08, rng);
  buf.add(PRIM.roof, place(x, y + 4.2 * size, z, w * 1.45, 3.0 * size, l * 1.25, 0, rot), C.roof, 0.06, rng);
  // Shutters on the long walls.
  for (const side of [-1, 1]) {
    for (const lz of [-l * 0.22, l * 0.22]) {
      const [sx, sz] = at(side * (w / 2 + 0.06), lz);
      buf.add(PRIM.box, place(sx, y + 3.2 * size, sz, 0.15, 1.0 * size, 1.3 * size, 0, rot), C.shutter, 0.05, rng);
    }
  }
}

/* --------------------------------------------------------- Swiss flags */

let flagTexture = null;
function swissFlagTexture() {
  if (flagTexture) return flagTexture;
  const c = document.createElement('canvas');
  c.width = 128;
  c.height = 128;
  const ctx = c.getContext('2d');
  ctx.fillStyle = '#da291c';
  ctx.fillRect(0, 0, 128, 128);
  // Official proportions on a 32-unit square: arms 6 wide, 20 tip to tip.
  ctx.fillStyle = '#ffffff';
  const t = 128 * (6 / 32);
  const len = 128 * (20 / 32);
  ctx.fillRect(64 - t / 2, 64 - len / 2, t, len);
  ctx.fillRect(64 - len / 2, 64 - t / 2, len, t);
  flagTexture = new THREE.CanvasTexture(c);
  flagTexture.colorSpace = THREE.SRGBColorSpace;
  flagTexture.anisotropy = 4;
  return flagTexture;
}

const flags = [];

export function makeSwissFlag(size = 6, poleHeight = 14) {
  const group = new THREE.Group();
  const pole = new THREE.Mesh(
    new THREE.CylinderGeometry(0.18, 0.24, poleHeight, 6),
    new THREE.MeshStandardMaterial({ color: 0xf2f2f2, roughness: 0.4, metalness: 0.4 }),
  );
  pole.position.y = poleHeight / 2;
  pole.castShadow = true;
  group.add(pole);
  const knob = new THREE.Mesh(
    new THREE.SphereGeometry(0.35, 8, 6),
    new THREE.MeshStandardMaterial({ color: 0xd9b45a, roughness: 0.3, metalness: 0.8 }),
  );
  knob.position.y = poleHeight + 0.2;
  group.add(knob);

  const geo = new THREE.PlaneGeometry(size, size, 10, 4);
  geo.translate(size / 2, 0, 0);
  const cloth = new THREE.Mesh(geo, new THREE.MeshStandardMaterial({
    map: swissFlagTexture(), side: THREE.DoubleSide, roughness: 0.8,
  }));
  cloth.position.set(0.2, poleHeight - size / 2 - 0.2, 0);
  cloth.castShadow = true;
  group.add(cloth);
  flags.push({ cloth, base: Float32Array.from(geo.attributes.position.array), size, phase: flags.length * 1.7 });
  return group;
}

/** Wave every Swiss flag and drift the lake ripples; call once per frame. */
export function updateAlpine(now) {
  if (lakeNormals) {
    lakeNormals.offset.set(now * 0.0035, now * 0.0021);
  }
  for (const f of flags) {
    const pos = f.cloth.geometry.attributes.position;
    for (let i = 0; i < pos.count; i++) {
      const bx = f.base[i * 3];
      const by = f.base[i * 3 + 1];
      const along = bx / f.size;
      const wave = Math.sin(now * 5 + f.phase - bx * 0.9) * along * 0.55
        + Math.sin(now * 3.1 + f.phase + by * 0.6) * along * 0.2;
      pos.setZ(i, wave);
      pos.setY(i, by - along * along * 0.25);
    }
    pos.needsUpdate = true;
    f.cloth.geometry.computeVertexNormals();
  }
}

/* ----------------------------------------------------------- the island */

/**
 * Build an alpine island in local coordinates and add it to `group`.
 * Returns { heightAt(x, z) } so callers can seat buildings on the terrain.
 */
export function buildAlpineIsland(group, spec) {
  const field = makeField(spec);
  const extent = spec.radius * 1.32 + 24;
  const cell = spec.cell ?? (spec.radius > 100 ? 3.2 : 2.6);
  const { terrain, shallows } = buildTerrainMesh(field, extent, cell);
  group.add(terrain, shallows);

  // ---- vegetation, rocks and chalets (one merged mesh) ----
  const rng = mulberry32(spec.seed * 0.137 + 0.5);
  const buf = new MergeBuffer();
  const [cx, cz] = spec.center;
  const clear = spec.keepClear ?? [];
  const occupied = [];
  const free = (x, z, r) => {
    for (const k of clear) if (Math.hypot(x - k.x, z - k.z) < k.r + r) return false;
    for (const p of spec.pads) if (sdRect(x, z, p) < 9 + r) return false;
    for (const o of occupied) if (Math.hypot(x - o[0], z - o[1]) < o[2] + r) return false;
    return true;
  };
  const slopeAt = (x, z) => {
    const hx = field.height(x + 1.5, z) - field.height(x - 1.5, z);
    const hz = field.height(x, z + 1.5) - field.height(x, z - 1.5);
    return Math.hypot(hx, hz) / 3;
  };
  const sample = () => {
    const a = rng() * Math.PI * 2;
    const r = Math.sqrt(rng()) * spec.radius * 1.2;
    return [cx + Math.cos(a) * r, cz + Math.sin(a) * r];
  };
  const snowLine = spec.snowLine ?? Infinity;

  // Chalets first, on gentle meadow ground.
  let chalets = 0;
  for (let tries = 0; tries < 600 && chalets < (spec.chalets ?? 2); tries++) {
    const [x, z] = sample();
    const h = field.height(x, z);
    if (h < SHORE_TOP + 0.1 || h > snowLine - 6 || slopeAt(x, z) > 0.2) continue;
    if (!free(x, z, 8)) continue;
    const size = 0.85 + rng() * 0.3;
    const ground = Math.min(
      field.height(x - 3, z - 3), field.height(x + 3, z - 3),
      field.height(x - 3, z + 3), field.height(x + 3, z + 3),
    );
    addChalet(buf, x, ground, z, size, rng() * Math.PI, rng);
    occupied.push([x, z, 7]);
    chalets++;
  }

  // Fir forests cluster where a low-frequency forest mask is high.
  const treeTarget = spec.trees ?? 120;
  let trees = 0;
  for (let tries = 0; tries < treeTarget * 14 && trees < treeTarget; tries++) {
    const [x, z] = sample();
    const h = field.height(x, z);
    if (h < SHORE_TOP + 0.05 || h > snowLine - 1 || slopeAt(x, z) > 0.75) continue;
    const mask = fbm(x * 0.03, z * 0.03, spec.seed + 101);
    if (rng() > smoothstep(0.38, 0.62, mask) + 0.06) continue;
    if (!free(x, z, 1.6)) continue;
    const size = 0.75 + rng() * 0.6 - smoothstep(snowLine - 14, snowLine, h) * 0.3;
    if (rng() < 0.82) addFir(buf, x, h, z, size, rng, h > snowLine - 7);
    else addLeafTree(buf, x, h, z, size * 0.9, rng);
    occupied.push([x, z, 1.4 * size]);
    trees++;
  }

  // Boulders: along the pebble shore and scattered on steep ground.
  for (let i = 0, placed = 0; i < 900 && placed < (spec.rocks ?? 40); i++) {
    const [x, z] = sample();
    const h = field.height(x, z);
    const onShore = h > -0.4 && h < SHORE_TOP;
    const onCrag = slopeAt(x, z) > 0.5 && h < snowLine;
    if (!onShore && !onCrag) continue;
    if (!free(x, z, 1)) continue;
    addRock(buf, x, h, z, 0.8 + rng() * (onShore ? 1.6 : 2.4), rng);
    placed++;
  }

  const scenery = buf.build();
  scenery.name = 'scenery';
  scenery.castShadow = true;
  scenery.receiveShadow = true;
  group.add(scenery);

  return { heightAt: field.height };
}

/* -------------------------------------------------- mountain backdrop */

/**
 * A ring of snow-capped Alps surrounding the lake, from `inner` (shoreline)
 * out to `outer`. One flat-shaded mesh; fog softens the far ranges.
 */
export function buildAlpineBackdrop(scene, inner = 600, outer = 1500) {
  const seed = 4.2;
  const radial = 34;
  const around = 220;
  const ringR = (i) => inner + (outer - inner) * Math.pow(i / radial, 1.35);
  const heightAt = (x, z, r) => {
    const t = smoothstep(inner, inner + 260, r);
    const ridge = ridged(x * 0.0042, z * 0.0042, seed, 5);
    const big = fbm(x * 0.0016, z * 0.0016, seed + 5, 3);
    let h = (ridge * 330 + big * 220 - 90) * t;
    // A lone Matterhorn-like horn to the north-west.
    const dh = Math.hypot(x + 520, z + 760);
    h += Math.max(0, 1 - dh / 210) ** 1.6 * 420 * t;
    return Math.max(h, (r - inner) * 0.06 - 6);
  };

  const vx = [];
  const vy = [];
  const vz = [];
  for (let i = 0; i <= radial; i++) {
    const r = ringR(i);
    for (let j = 0; j < around; j++) {
      const a = (j / around) * Math.PI * 2 + (i % 2) * (Math.PI / around);
      const rr = r + (hash2(i, j, seed) - 0.5) * 18 * (i > 0 ? 1 : 0);
      const x = Math.cos(a) * rr;
      const z = Math.sin(a) * rr;
      vx.push(x);
      vz.push(z);
      vy.push(heightAt(x, z, rr));
    }
  }

  const pos = [];
  const col = [];
  const color = new THREE.Color();
  const a = new THREE.Vector3();
  const b = new THREE.Vector3();
  const c = new THREE.Vector3();
  const nrm = new THREE.Vector3();
  const edge = new THREE.Vector3();
  const forest = new THREE.Color(0x2f5a33);
  const alp = new THREE.Color(0x6f9a4a);
  const scree = new THREE.Color(0x8b8780);
  const cliff = new THREE.Color(0x5f5c58);
  const tri = (k0, k1, k2) => {
    a.set(vx[k0], vy[k0], vz[k0]);
    b.set(vx[k1], vy[k1], vz[k1]);
    c.set(vx[k2], vy[k2], vz[k2]);
    nrm.subVectors(b, a).cross(edge.subVectors(c, a)).normalize();
    if (nrm.y < 0) {
      // Flip the winding so every face points up (front-side rendering).
      nrm.negate();
      edge.copy(b);
      b.copy(c);
      c.copy(edge);
    }
    const my = (a.y + b.y + c.y) / 3;
    const n = fbm(a.x * 0.01, a.z * 0.01, seed + 9);
    const snowLine = 170 + (n - 0.5) * 70;
    if (my > snowLine && nrm.y > 0.42) color.copy(C.snow);
    else if (nrm.y < 0.6) color.copy(cliff).lerp(scree, n);
    else if (my > snowLine - 60) color.copy(scree).lerp(alp, 0.3);
    else if (my > 45) color.copy(alp).lerp(forest, smoothstep(0.4, 0.7, n));
    else color.copy(forest).lerp(alp, n * 0.5);
    const f = 0.94 + hash2(Math.floor(a.x), Math.floor(a.z), 3) * 0.1;
    color.multiplyScalar(f);
    for (const v of [a, b, c]) {
      pos.push(v.x, v.y, v.z);
      col.push(color.r, color.g, color.b);
    }
  };
  for (let i = 0; i < radial; i++) {
    for (let j = 0; j < around; j++) {
      const j1 = (j + 1) % around;
      const k00 = i * around + j;
      const k01 = i * around + j1;
      const k10 = (i + 1) * around + j;
      const k11 = (i + 1) * around + j1;
      tri(k00, k10, k01);
      tri(k01, k10, k11);
    }
  }
  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
  geo.setAttribute('color', new THREE.Float32BufferAttribute(col, 3));
  geo.computeVertexNormals();
  const mesh = new THREE.Mesh(geo, new THREE.MeshStandardMaterial({
    vertexColors: true, flatShading: true, roughness: 1, metalness: 0,
  }));
  mesh.name = 'alps';
  scene.add(mesh);
  return mesh;
}

/* ------------------------------------------------------------- lake */

let lakeNormals = null;

// Tileable ripple normal map from periodic value noise (no external assets).
function makeRippleNormalMap(size = 256, period = 16) {
  const h = new Float32Array(size * size);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let v = 0;
      let amp = 1;
      let p = period;
      for (let o = 0; o < 4; o++) {
        const fx = (x / size) * p;
        const fy = (y / size) * p;
        const ix = Math.floor(fx);
        const iy = Math.floor(fy);
        const tx = fx - ix;
        const ty = fy - iy;
        const ux = tx * tx * (3 - 2 * tx);
        const uy = ty * ty * (3 - 2 * ty);
        const g = (i, j) => hash2(((i % p) + p) % p, ((j % p) + p) % p, 9 + o);
        const a = g(ix, iy);
        const b = g(ix + 1, iy);
        const c = g(ix, iy + 1);
        const d = g(ix + 1, iy + 1);
        v += (a + (b - a) * ux + (c - a) * uy + (a - b - c + d) * ux * uy) * amp;
        amp *= 0.5;
        p *= 2;
      }
      h[y * size + x] = v;
    }
  }
  const data = new Uint8Array(size * size * 4);
  const strength = 2.2;
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const at = (i, j) => h[((j + size) % size) * size + ((i + size) % size)];
      const dx = (at(x + 1, y) - at(x - 1, y)) * strength;
      const dy = (at(x, y + 1) - at(x, y - 1)) * strength;
      const len = Math.hypot(dx, dy, 1);
      const k = (y * size + x) * 4;
      data[k] = ((-dx / len) * 0.5 + 0.5) * 255;
      data[k + 1] = ((-dy / len) * 0.5 + 0.5) * 255;
      data[k + 2] = ((1 / len) * 0.5 + 0.5) * 255;
      data[k + 3] = 255;
    }
  }
  const tex = new THREE.DataTexture(data, size, size, THREE.RGBAFormat);
  tex.wrapS = THREE.RepeatWrapping;
  tex.wrapT = THREE.RepeatWrapping;
  tex.magFilter = THREE.LinearFilter;
  tex.minFilter = THREE.LinearMipmapLinearFilter;
  tex.generateMipmaps = true;
  tex.anisotropy = 8;
  tex.needsUpdate = true;
  return tex;
}

/** Alpine lake surface: deep blue-green with drifting ripples that catch the sun. */
export function makeLakeMaterial() {
  lakeNormals = makeRippleNormalMap();
  lakeNormals.repeat.set(90, 90);
  return new THREE.MeshStandardMaterial({
    color: 0x1f6f8c,
    roughness: 0.3,
    metalness: 0.1,
    normalMap: lakeNormals,
    normalScale: new THREE.Vector2(0.26, 0.26),
  });
}

/** Opt every lit, opaque mesh under `root` into casting and receiving shadows. */
export function enableShadows(root, { receive = true } = {}) {
  root.traverse((o) => {
    if (!o.isMesh || o.isInstancedMesh) return;
    const mats = Array.isArray(o.material) ? o.material : [o.material];
    const lit = mats.every((m) => m && !m.isMeshBasicMaterial && !m.transparent);
    if (!lit) return;
    // Painted decals (polygon-offset markings) only receive: casting from a
    // plane hovering 0.08 above the deck would just add acne.
    o.castShadow = !mats.some((m) => m.polygonOffset);
    o.receiveShadow = receive;
  });
}
