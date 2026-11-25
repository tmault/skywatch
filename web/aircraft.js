// Aircraft meshes. Every builder takes the raw model id, paints the airframe
// in that model provider's livery (see livery.js), and returns a group whose
// nose points along +Z with +Y up. +X is the aircraft's left (port) side.
//
// Fuselages are lathed from a smoothed side profile with UVs that wrap the
// body (u = angle around, v = position along), so liveries are painted as one
// skin. Flying surfaces are lofted symmetric airfoils with real taper, sweep,
// thickness and dihedral.

import * as THREE from './vendor/three.module.js';
import {
  TANKER_GREY, liveryFor, fuselageTexture, finTexture, logoTexture, surfaceTexture,
} from './livery.js';

/* --------------------------------------------------------------- helpers */

// Binary-search linear interpolation along a monotonic Z sample array.
function sampleAtZ(pts, z, getZ, getV) {
  if (z <= getZ(pts[0])) return getV(pts[0]);
  if (z >= getZ(pts[pts.length - 1])) return getV(pts[pts.length - 1]);
  let lo = 0;
  let hi = pts.length - 1;
  while (hi - lo > 1) {
    const mid = (lo + hi) >> 1;
    if (getZ(pts[mid]) <= z) lo = mid; else hi = mid;
  }
  const z0 = getZ(pts[lo]);
  const z1 = getZ(pts[hi]);
  const f = z1 > z0 ? (z - z0) / (z1 - z0) : 0;
  return getV(pts[lo]) + (getV(pts[hi]) - getV(pts[lo])) * f;
}

// Body of revolution along Z. `profile` is [[z, radius], ...] from tail to nose.
// `sx`/`sy` squash the cross-section; `warp(z)` returns a vertical offset so
// tails can sweep up and noses can droop. Also returns the paint layout basics
// and a `pointAt(z, phi)` surface sampler for fitted 3D cockpit glazing.
function fuselage(profile, { sx = 1, sy = 1, warp = null, segments = 28, smooth = true } = {}) {
  let pts = profile.map(([z, r]) => new THREE.Vector3(r, z, 0));
  if (smooth) {
    pts = new THREE.CatmullRomCurve3(pts, false, 'centripetal').getPoints(profile.length * 6);
  }
  const geo = new THREE.LatheGeometry(
    pts.map((p) => new THREE.Vector2(Math.max(p.x, 0.0001), p.y)), segments,
  );
  geo.rotateX(Math.PI / 2); // lathe axis +Y -> +Z
  const zmin = profile[0][0];
  const zmax = profile[profile.length - 1][0];
  const pos = geo.attributes.position;
  const uv = geo.attributes.uv;
  for (let i = 0; i < pos.count; i++) {
    const z = pos.getZ(i);
    pos.setXYZ(i, pos.getX(i) * sx, pos.getY(i) * sy + (warp ? warp(z) : 0), z);
    // Lathe u is already the angle around; make v linear in z for painting.
    uv.setY(i, (z - zmin) / (zmax - zmin));
  }
  geo.computeVertexNormals();
  const rmax = Math.max(...profile.map(([, r]) => r));
  // phi = 0 is belly, PI/2 is +X port side, PI is top spine.
  const pointAt = (z, phi) => {
    const r = Math.max(sampleAtZ(pts, z, (p) => p.y, (p) => p.x), 0.0001);
    const wy = warp ? warp(z) : 0;
    return [Math.sin(phi) * r * sx, -Math.cos(phi) * r * sy + wy, z];
  };
  return { geo, zmin, zmax, circ: 2 * Math.PI * rmax * (sx + sy) / 2, pointAt };
}

// Body with superellipse cross-sections, for airframes that are boxier than
// a body of revolution. `stations` are [z, halfWidth, top, bottom] from tail
// to nose; `power` sets how square the sections are (2 = ellipse). Same UV
// and paint layout as fuselage(); both ends are capped.
function loftBody(stations, { power = 2.5, segments = 36, samples = 8 } = {}) {
  const curve = (pts) => new THREE.CatmullRomCurve3(pts, false, 'catmullrom', 0.5);
  // Uniform Catmull-Rom on both curves keeps their samples at matching z.
  const upper = curve(stations.map(([z, w, t]) => new THREE.Vector3(z, w, t)));
  const lower = curve(stations.map(([z, , , b]) => new THREE.Vector3(z, b, 0)));
  const n = (stations.length - 1) * samples;
  const e = 2 / power;
  const zmin = stations[0][0];
  const zmax = stations[stations.length - 1][0];
  const S = segments;
  const pos = [];
  const uv = [];
  const idx = [];
  const rings = [];
  const secSamples = [];
  for (let i = 0; i <= n; i++) {
    const u = upper.getPoint(i / n);
    const l = lower.getPoint(i / n);
    const w = Math.max(u.y, 0.001);
    const yc = (u.z + l.y) / 2;
    const h = Math.max((u.z - l.y) / 2, 0.001);
    secSamples.push({ z: u.x, w, yc, h });
    const ring = [];
    for (let j = 0; j <= S; j++) {
      // phi = 0 is the belly, PI the spine, PI/2 the +X side (as LatheGeometry).
      const phi = (j / S) * Math.PI * 2;
      const sn = Math.sin(phi);
      const cs = Math.cos(phi);
      ring.push([w * Math.sign(sn) * Math.abs(sn) ** e, yc - h * Math.sign(cs) * Math.abs(cs) ** e, u.x]);
    }
    rings.push({ ring, yc, z: u.x });
  }
  rings.forEach(({ ring, z }) => ring.forEach((p, j) => {
    pos.push(...p);
    uv.push(j / S, (z - zmin) / (zmax - zmin));
  }));
  for (let i = 0; i < n; i++) {
    for (let j = 0; j < S; j++) {
      const a = i * (S + 1) + j;
      const b = a + S + 1;
      idx.push(a, a + 1, b, a + 1, b + 1, b);
    }
  }
  // End caps on their own vertices so the rims stay crisp.
  for (const [i, nose] of [[0, false], [n, true]]) {
    const { ring, yc, z } = rings[i];
    const base = pos.length / 3;
    ring.forEach((p) => { pos.push(...p); uv.push(0.5, nose ? 1 : 0); });
    pos.push(0, yc, z);
    uv.push(0.5, nose ? 1 : 0);
    const c = base + S + 1;
    for (let j = 0; j < S; j++) {
      if (nose) idx.push(c, base + j, base + j + 1);
      else idx.push(c, base + j + 1, base + j);
    }
  }
  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
  geo.setAttribute('uv', new THREE.Float32BufferAttribute(uv, 2));
  geo.setIndex(idx);
  geo.computeVertexNormals();
  const widest = stations.reduce((m, [, w, t, b]) => Math.max(m, Math.hypot(w, (t - b) / 2)), 0);
  const pointAt = (z, phi) => {
    const w = sampleAtZ(secSamples, z, (s) => s.z, (s) => s.w);
    const yc = sampleAtZ(secSamples, z, (s) => s.z, (s) => s.yc);
    const h = sampleAtZ(secSamples, z, (s) => s.z, (s) => s.h);
    const sn = Math.sin(phi);
    const cs = Math.cos(phi);
    return [w * Math.sign(sn) * Math.abs(sn) ** e, yc - h * Math.sign(cs) * Math.abs(cs) ** e, z];
  };
  return { geo, zmin, zmax, circ: 2 * Math.PI * widest / Math.SQRT2, pointAt };
}

// Multi-pane 3D cockpit glazing fitted directly to a fuselage's `pointAt(z, phi)`.
// Each pane is specified on the port (+X) side as `knots: [[s, yBot, yTop], ...]`
// where `y` is exact vertical height in model coordinates (so window sills stay
// crisp and horizontal instead of curving around the nose) and `s` is 3D arc
// length measured along the constant-y surface contour starting from the front
// centreline (s = 0). Leaving small gaps between adjacent panes lets the actual
// fuselage paint show through as razor-thin body-coloured frames, with distinct
// semi-flat glass reflections per pane and zero Z-fighting.
function cockpitGlazingMesh(body, mat, panes, {
  zSearchStart = body.zmax - 0.02,
  zSearchEnd = body.zmin + (body.zmax - body.zmin) * 0.5,
  offset = 0.03,
  subdivS = 4,
  subdivY = 2,
  flatBlend = 0.72,
  extraQuads = [],
} = {}) {
  const dzN = Math.max((body.zmax - body.zmin) * 0.0008, 0.004);
  const dphiN = 0.008;

  // Outward unit normal at (z, phi) on the port side (phi in [0, PI]).
  const normalAt = (z, phi) => {
    const za = Math.max(body.zmin + 0.001, z - dzN);
    const zb = Math.min(body.zmax - 0.001, z + dzN);
    const pa = Math.max(0.001, phi - dphiN);
    const pb = Math.min(Math.PI - 0.001, phi + dphiN);
    const pZ0 = body.pointAt(za, phi);
    const pZ1 = body.pointAt(zb, phi);
    const pP0 = body.pointAt(z, pa);
    const pP1 = body.pointAt(z, pb);
    // Tangent along +phi (belly -> port -> spine) crossed with +z (tail -> nose)
    // points outward on the port (+X) half.
    const tx = pP1[0] - pP0[0], ty = pP1[1] - pP0[1], tz = pP1[2] - pP0[2];
    const ux = pZ1[0] - pZ0[0], uy = pZ1[1] - pZ0[1], uz = pZ1[2] - pZ0[2];
    let nx = ty * uz - tz * uy;
    let ny = tz * ux - tx * uz;
    let nz = tx * uy - ty * ux;
    const len = Math.hypot(nx, ny, nz) || 1;
    nx /= len; ny /= len; nz /= len;
    // Sanity check: on the upper port nose, outward normal has nx >= -0.05 and (ny + nz) > -0.2.
    const p = body.pointAt(z, phi);
    const ctrY = (body.pointAt(z, 0)[1] + body.pointAt(z, Math.PI)[1]) * 0.5;
    if (nx * p[0] + ny * (p[1] - ctrY) < 0 && p[0] > 0.02) {
      nx = -nx; ny = -ny; nz = -nz;
    }
    return [nx, ny, nz];
  };

  // Horizontal contour (x >= 0, y, z) at constant height `y`, parameterized by
  // arc length `s` from the frontmost intersection on the centreline (s = 0).
  const contourCache = new Map();
  const contourAtY = (y) => {
    const key = Math.round(y * 10000);
    if (contourCache.has(key)) return contourCache.get(key);
    const topY = (z) => body.pointAt(z, Math.PI)[1];
    // Scan aft from nose tip to find where the upper spine rises to `y`.
    const scanSteps = 120;
    let zHi = zSearchStart;
    let zLo = zSearchEnd;
    let found = false;
    for (let i = 0; i <= scanSteps; i++) {
      const z = zSearchStart - (zSearchStart - zSearchEnd) * (i / scanSteps);
      if (topY(z) >= y) {
        zLo = z;
        zHi = i === 0 ? z : zSearchStart - (zSearchStart - zSearchEnd) * ((i - 1) / scanSteps);
        found = true;
        break;
      }
    }
    if (!found) {
      contourCache.set(key, null);
      return null;
    }
    // Refine exact front centreline intersection zFront where topY(zFront) == y.
    for (let k = 0; k < 18; k++) {
      const mid = (zLo + zHi) * 0.5;
      if (topY(mid) >= y) zLo = mid; else zHi = mid;
    }
    const zFront = zLo;
    const pts = [];
    const M = 90;
    let accumS = 0;
    let prev = null;
    for (let i = 0; i <= M; i++) {
      const t = i / M;
      // Quadratic clustering packs samples into the tight curve around the nose front.
      const z = zFront - (zFront - zSearchEnd) * (t * t);
      const yBot = body.pointAt(z, 0)[1];
      const yTop = topY(z);
      if (y < yBot || y > yTop + 1e-4) continue;
      let phi;
      if (i === 0) {
        phi = Math.PI;
      } else {
        let lo = 0;
        let hi = Math.PI;
        for (let k = 0; k < 18; k++) {
          const mid = (lo + hi) * 0.5;
          if (body.pointAt(z, mid)[1] < y) lo = mid; else hi = mid;
        }
        phi = (lo + hi) * 0.5;
      }
      const p = body.pointAt(z, phi);
      const n = normalAt(z, phi);
      if (prev) accumS += Math.hypot(p[0] - prev[0], p[1] - prev[1], p[2] - prev[2]);
      pts.push({ s: accumS, p, n });
      prev = p;
    }
    contourCache.set(key, pts);
    return pts;
  };

  const sampleContour = (y, s) => {
    const pts = contourAtY(y);
    if (!pts || pts.length < 2) return null;
    if (s <= pts[0].s) return pts[0];
    if (s >= pts[pts.length - 1].s) return pts[pts.length - 1];
    let lo = 0;
    let hi = pts.length - 1;
    while (hi - lo > 1) {
      const mid = (lo + hi) >> 1;
      if (pts[mid].s <= s) lo = mid; else hi = mid;
    }
    const a = pts[lo], b = pts[hi];
    const f = b.s > a.s ? (s - a.s) / (b.s - a.s) : 0;
    const p = [
      a.p[0] + (b.p[0] - a.p[0]) * f,
      a.p[1] + (b.p[1] - a.p[1]) * f,
      a.p[2] + (b.p[2] - a.p[2]) * f,
    ];
    let nx = a.n[0] + (b.n[0] - a.n[0]) * f;
    let ny = a.n[1] + (b.n[1] - a.n[1]) * f;
    let nz = a.n[2] + (b.n[2] - a.n[2]) * f;
    const l = Math.hypot(nx, ny, nz) || 1;
    return { p, n: [nx / l, ny / l, nz / l] };
  };

  const verts = [];
  const norms = [];
  const idx = [];

  const emitGrid = (grid, nxCols, nyRows, mirror) => {
    // Average pane normal for crisp semi-flat laminated glass reflections.
    let anx = 0, any = 0, anz = 0;
    for (const cell of grid) {
      anx += cell.n[0]; any += cell.n[1]; anz += cell.n[2];
    }
    const al = Math.hypot(anx, any, anz) || 1;
    anx /= al; any /= al; anz /= al;

    for (const side of (mirror ? [1, -1] : [1])) {
      const base = verts.length / 3;
      const sx = side;
      for (const cell of grid) {
        const px = sx * (cell.p[0] + cell.n[0] * offset);
        const py = cell.p[1] + cell.n[1] * offset;
        const pz = cell.p[2] + cell.n[2] * offset;
        let vx = sx * (cell.n[0] * (1 - flatBlend) + anx * flatBlend);
        let vy = cell.n[1] * (1 - flatBlend) + any * flatBlend;
        let vz = cell.n[2] * (1 - flatBlend) + anz * flatBlend;
        const vl = Math.hypot(vx, vy, vz) || 1;
        verts.push(px, py, pz);
        norms.push(vx / vl, vy / vl, vz / vl);
      }
      const stride = nyRows + 1;
      for (let i = 0; i < nxCols; i++) {
        for (let j = 0; j < nyRows; j++) {
          const a = base + i * stride + j;
          const b = base + (i + 1) * stride + j;
          const c = base + (i + 1) * stride + (j + 1);
          const d = base + i * stride + (j + 1);
          // Enforce outward CCW winding against the pane normal.
          const ax = verts[a * 3], ay = verts[a * 3 + 1], az = verts[a * 3 + 2];
          const bx = verts[b * 3] - ax, by = verts[b * 3 + 1] - ay, bz = verts[b * 3 + 2] - az;
          const cx = verts[c * 3] - ax, cy = verts[c * 3 + 1] - ay, cz = verts[c * 3 + 2] - az;
          const crx = by * cz - bz * cy, cry = bz * cx - bx * cz, crz = bx * cy - by * cx;
          const dot = crx * (sx * anx) + cry * any + crz * anz;
          if (dot >= 0) idx.push(a, b, c, a, c, d);
          else idx.push(a, c, b, a, d, c);
        }
      }
    }
  };

  for (const pane of panes) {
    const knots = pane.knots;
    const cols = [];
    for (let k = 0; k < knots.length - 1; k++) {
      const [s0, b0, t0] = knots[k];
      const [s1, b1, t1] = knots[k + 1];
      const steps = pane.subdivS ?? subdivS;
      const startI = k === 0 ? 0 : 1;
      for (let i = startI; i <= steps; i++) {
        const u = i / steps;
        cols.push([s0 + (s1 - s0) * u, b0 + (b1 - b0) * u, t0 + (t1 - t0) * u]);
      }
    }
    const ny = pane.subdivY ?? subdivY;
    const grid = [];
    let ok = true;
    for (const [s, yB, yT] of cols) {
      for (let j = 0; j <= ny; j++) {
        const y = yB + (yT - yB) * (j / ny);
        const sample = sampleContour(y, s);
        if (!sample) { ok = false; break; }
        grid.push(sample);
      }
      if (!ok) break;
    }
    if (ok) emitGrid(grid, cols.length - 1, ny, pane.mirror !== false);
  }

  // Optional direct (z, phi) quads (e.g. Cessna rear omni-vision wrap window).
  for (const q of extraQuads) {
    const [z0, z1, a0, a1] = [q.z0, q.z1, q.a0, q.a1];
    const nu = q.subdivU ?? 6;
    const nv = q.subdivV ?? 2;
    const grid = [];
    for (let i = 0; i <= nu; i++) {
      const u = i / nu;
      const a = -a0 + 2 * a0 * u; // symmetric across spine
      const aTop = -a1 + 2 * a1 * u;
      for (let j = 0; j <= nv; j++) {
        const v = j / nv;
        const z = z0 + (z1 - z0) * v;
        const ang = a + (aTop - a) * v;
        const phi = Math.PI - Math.abs(ang);
        const p = body.pointAt(z, phi);
        const n = normalAt(z, phi);
        const sign = ang < 0 ? -1 : 1;
        grid.push({ p: [sign * p[0], p[1], p[2]], n: [sign * n[0], n[1], n[2]] });
      }
    }
    emitGrid(grid, nu, nv, false);
  }

  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.Float32BufferAttribute(verts, 3));
  geo.setAttribute('normal', new THREE.Float32BufferAttribute(norms, 3));
  geo.setIndex(idx);
  const mesh = new THREE.Mesh(geo, mat);
  mesh.name = 'cockpit-glazing';
  return mesh;
}

// NACA 00xx half-thickness at chord fraction c (0 = leading edge).
function naca(c, t) {
  return 5 * t * (0.2969 * Math.sqrt(c) - 0.126 * c - 0.3516 * c * c
    + 0.2843 * c ** 3 - 0.1036 * c ** 4);
}

// Lofted flying surface. Span runs along +X (times `side`), chord along -Z
// from the leading edge, thickness along Y.
//   root/tip: { le: z of leading edge, chord, y: height offset }
// UVs: u is the chord fraction (0 = leading edge), v runs root -> tip, so
// hinge lines and leading edges paint at the same place on any planform.
function surfaceGeo({ span, root, tip, thickness = 0.12, side = 1, x0 = 0 }) {
  const N = 13; // stations along each airfoil surface (clustered at the LE)
  const ring = [];
  // Cosine spacing packs stations into the rounded leading edge.
  const cAt = (i) => (1 - Math.cos((Math.PI * i) / (N - 1))) / 2;
  for (let i = 0; i < N; i++) ring.push([cAt(N - 1 - i), 1]); // TE -> LE (upper)
  for (let i = 1; i < N - 1; i++) ring.push([cAt(i), -1]); // LE -> TE (lower)
  const M = ring.length;
  const sections = [{ x: x0, ...root }, { x: x0 + span, ...tip }];

  // Per-section thickness (`t`) lets thick roots taper to thin tips.
  const rings = sections.map((s) => {
    const t = s.t ?? thickness;
    return ring.map(([c, sgn]) => [
      side * s.x, (s.y ?? 0) + sgn * Math.max(naca(c, t), 0.0025) * s.chord, s.le - c * s.chord, c,
    ]);
  });
  const verts = [];
  const uvs = [];
  const idx = [];
  const tri = (a, b, c) => (side > 0 ? idx.push(a, c, b) : idx.push(a, b, c));

  // Skin: shared vertices so the airfoil shades smoothly.
  rings.forEach((pts, si) => pts.forEach(([x, y, z, c]) => { verts.push(x, y, z); uvs.push(c, si); }));
  for (let k = 0; k < M; k++) {
    const k2 = (k + 1) % M;
    tri(k, M + k, M + k2);
    tri(k, M + k2, k2);
  }
  // Root and tip caps get their own vertices so their edges stay crisp.
  rings.forEach((pts, si) => {
    const base = verts.length / 3;
    pts.forEach(([x, y, z, c]) => { verts.push(x, y, z); uvs.push(c, si); });
    const s = sections[si];
    verts.push(side * s.x, s.y ?? 0, s.le - 0.4 * s.chord);
    uvs.push(0.4, si);
    const ctr = base + M;
    for (let k = 0; k < M; k++) {
      const k2 = (k + 1) % M;
      if (si === 0) tri(ctr, base + k, base + k2);
      else tri(ctr, base + k2, base + k);
    }
  });

  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.Float32BufferAttribute(verts, 3));
  geo.setAttribute('uv', new THREE.Float32BufferAttribute(uvs, 2));
  geo.setIndex(idx);
  geo.computeVertexNormals();
  return geo;
}

// Mirrored pair of surfaces. Dihedral (radians) is applied as a tip rise.
function surfacePair(mat, opts) {
  const { span, dihedral = 0, root, tip } = opts;
  const rise = Math.tan(dihedral) * span;
  return [1, -1].map((side) => new THREE.Mesh(surfaceGeo({
    ...opts, side, root, tip: { ...tip, y: (root.y ?? 0) + rise + (tip.y ?? 0) },
  }), mat));
}

// Vertical fin: a surface whose span points up +Y. `cant` tilts it outward
// toward `side` (radians from vertical). With `logo`, a decal is applied to
// both faces at `logoAt` of the fin height.
function finMesh(mat, {
  height, root, tip, thickness = 0.1, base = [0, 0], cant = 0, side = 1,
  logo = null, logoAt = 0.45, logoScale = 0.72,
}) {
  const geo = surfaceGeo({ span: height, root: { ...root, y: 0 }, tip: { ...tip, y: 0 }, thickness });
  geo.rotateZ(Math.PI / 2); // span +X -> +Y, thickness Y -> -X
  const mesh = new THREE.Mesh(geo, mat);
  mesh.position.set(base[0], base[1], 0);
  mesh.rotation.z = -side * cant;

  if (logo) {
    const f = logoAt;
    const le = root.le + (tip.le - root.le) * f;
    const chord = root.chord + (tip.chord - root.chord) * f;
    const size = Math.min(chord * logoScale, height * 0.55);
    const off = 0.5 * thickness * chord + 0.025;
    for (const face of [1, -1]) {
      const decal = new THREE.Mesh(new THREE.PlaneGeometry(size, size), logo);
      // Rotating the plane's +Z normal onto +/-X keeps the image unmirrored
      // from either side.
      decal.rotation.y = face * Math.PI / 2;
      decal.position.set(face * off, height * f, le - chord * 0.45);
      mesh.add(decal);
    }
  }
  return mesh;
}

// Cylinder spanning two points.
function rod(mat, a, b, radius, segs = 6) {
  const va = new THREE.Vector3(...a);
  const vb = new THREE.Vector3(...b);
  const mesh = new THREE.Mesh(new THREE.CylinderGeometry(radius, radius, va.distanceTo(vb), segs), mat);
  mesh.position.copy(va).add(vb).multiplyScalar(0.5);
  mesh.quaternion.setFromUnitVectors(new THREE.Vector3(0, 1, 0), vb.clone().sub(va).normalize());
  return mesh;
}

// Tyre with a metal hub showing on both faces.
function wheel(kit, x, y, z, r, w) {
  const g = new THREE.Group();
  g.add(new THREE.Mesh(new THREE.CylinderGeometry(r, r, w, 18), kit.rubber));
  g.add(new THREE.Mesh(new THREE.CylinderGeometry(r * 0.55, r * 0.55, w * 1.04, 12), kit.metal));
  g.rotation.z = Math.PI / 2;
  g.position.set(x, y, z);
  return g;
}

// Retractable gear rig. Parts are authored in gear coordinates in their
// deployed pose, then hung from hinge groups without moving, so progress 1 is
// exactly the authored geometry. Each hinge swings to its `stow` rotation as
// progress falls to 0.
function gearRig() {
  const gear = new THREE.Group();
  gear.name = 'gear';
  const joints = [];
  const braces = [];
  const doors = [];
  const UP = new THREE.Vector3(0, 1, 0);

  // `parent` is `gear` or a top-level hinge (for a bogie on the end of a leg).
  function hang(pivot, parts, parent = gear) {
    const hinge = new THREE.Group();
    const at = new THREE.Vector3(...pivot);
    for (const part of parts) {
      part.position.sub(at);
      hinge.add(part);
    }
    hinge.position.copy(at).sub(parent.position);
    parent.add(hinge);
    return hinge;
  }

  gear.userData.setProgress = (t) => {
    const down = THREE.MathUtils.clamp(t, 0, 1);
    for (const { hinge, stow } of joints) hinge.quaternion.identity().slerp(stow, 1 - down);
    // Doors stay open while the legs travel and close over the end of the cycle.
    const closing = 1 - THREE.MathUtils.smoothstep(down, 0, 0.35);
    for (const { hinge, stow } of doors) hinge.quaternion.identity().slerp(stow, closing);
    for (const { mesh, anchor, foot, hinge, length } of braces) {
      const end = foot.clone().sub(hinge.position).applyQuaternion(hinge.quaternion).add(hinge.position);
      const span = end.clone().sub(anchor);
      mesh.position.copy(anchor).add(end).multiplyScalar(0.5);
      mesh.scale.y = span.length() / length;
      mesh.quaternion.setFromUnitVectors(UP, span.normalize());
    }
    gear.visible = down > 0.005;
  };

  return {
    gear,
    leg(pivot, parts, stow, parent) {
      const hinge = hang(pivot, parts, parent);
      joints.push({ hinge, stow });
      return hinge;
    },
    door(pivot, mesh, stow) {
      doors.push({ hinge: hang(pivot, [mesh]), stow });
    },
    // Folding side/drag brace from a fixed airframe anchor to a point on a leg.
    brace(mat, anchor, foot, radius, hinge, segs) {
      const mesh = rod(mat, anchor, foot, radius, segs);
      gear.add(mesh);
      braces.push({
        mesh, hinge,
        anchor: new THREE.Vector3(...anchor),
        foot: new THREE.Vector3(...foot),
        length: mesh.geometry.parameters.height,
      });
    },
  };
}

const FOLD_AXES = { x: new THREE.Vector3(1, 0, 0), z: new THREE.Vector3(0, 0, 1) };

// Stow rotation about one axis (positive about X swings a hanging leg aft,
// negative about Z swings a port leg inboard).
function foldAbout(axis, angle) {
  return new THREE.Quaternion().setFromAxisAngle(FOLD_AXES[axis], angle);
}

// Stow rotation for a skewed trunnion: shortest swing that takes the deployed
// pivot-to-wheel direction onto `toward`.
function foldToward(pivot, wheelAt, toward) {
  return new THREE.Quaternion().setFromUnitVectors(
    new THREE.Vector3(...wheelAt).sub(new THREE.Vector3(...pivot)).normalize(),
    new THREE.Vector3(...toward).normalize(),
  );
}

// Tapered, twisted propeller blade along +Y from radius r0 to r1. Pitch is
// the blade angle from the disc plane, washing out toward the tip.
function propBlade(mat, { r0, r1, rootChord, tipChord, pitchRoot = 0.8, pitchTip = 0.32 }) {
  const geo = surfaceGeo({
    span: r1 - r0, x0: r0,
    root: { le: rootChord * 0.35, chord: rootChord, t: 0.16 },
    tip: { le: tipChord * 0.35, chord: tipChord, t: 0.08 },
  });
  geo.rotateZ(Math.PI / 2); // span +X -> +Y, chord stays along Z
  const pos = geo.attributes.position;
  for (let i = 0; i < pos.count; i++) {
    const [x, y, z] = [pos.getX(i), pos.getY(i), pos.getZ(i)];
    const f = Math.min(Math.max((y - r0) / (r1 - r0), 0), 1);
    const twist = Math.PI / 2 - (pitchRoot + (pitchTip - pitchRoot) * f);
    const c = Math.cos(twist);
    const sn = Math.sin(twist);
    pos.setXYZ(i, x * c + z * sn, y, -x * sn + z * c);
  }
  geo.computeVertexNormals();
  return new THREE.Mesh(geo, mat);
}

// Spinning propeller: blur disc plus `blades` lofted blades.
function propeller(kit, { blades, radius, rootChord, tipChord, hub = 0.3 }) {
  const prop = new THREE.Group();
  prop.name = 'prop';
  prop.add(new THREE.Mesh(new THREE.CircleGeometry(radius * 1.04, 32), kit.propDisc));
  for (let i = 0; i < blades; i++) {
    const blade = propBlade(kit.dark, { r0: hub, r1: radius, rootChord, tipChord });
    blade.rotation.z = (i * 2 * Math.PI) / blades;
    prop.add(blade);
  }
  return prop;
}

function dot(mat, x, y, z, r) {
  const m = new THREE.Mesh(new THREE.SphereGeometry(r, 8, 6), mat);
  m.position.set(x, y, z);
  return m;
}

// Livery materials for one aircraft. Painted textures are shared between
// aircraft with the same livery (see livery.js), materials are per aircraft.
// Tankers wear a matte grey military transport scheme (A400M / MRTT style)
// across fuselage, wings, and engines while keeping the AI lab's tail livery.
function paintKit(model, airframe, layout) {
  const livery = liveryFor(model);
  const military = airframe === 'tanker';
  const std = (opts) => new THREE.MeshStandardMaterial({ roughness: 0.42, metalness: 0.08, ...opts });
  const airframeFinish = military ? { roughness: 0.72, metalness: 0.04 } : {};
  const wingFinish = military ? { roughness: 0.74, metalness: 0.04 } : {};
  const bodyColor = military ? TANKER_GREY.body : livery.body;
  const wingColor = military ? TANKER_GREY.wing : livery.wing;
  const engineColor = military ? TANKER_GREY.engine : livery.engine;
  const kit = {
    livery,
    body: std({ map: fuselageTexture(livery, airframe, layout), ...airframeFinish }),
    plain: std({ color: bodyColor, ...airframeFinish }),
    wing: std({ color: wingColor, ...wingFinish }),
    // Vertical fin always carries the AI lab's full tail colours and emblem.
    fin: std({ map: finTexture(livery) }),
    // Flying-surface skins with hinge lines (see surfaceTexture).
    wingSkin: std({
      map: surfaceTexture(livery, wingColor, military ? 'tanker-wing' : 'wing'),
      ...wingFinish,
    }),
    wingPlainSkin: std({ map: surfaceTexture(livery, bodyColor, 'wing'), ...wingFinish }),
    tailSkin: std({
      map: surfaceTexture(livery, wingColor, military ? 'tanker-tail' : 'tail'),
      ...wingFinish,
    }),
    tailPlainSkin: std({ map: surfaceTexture(livery, bodyColor, 'tail'), ...wingFinish }),
    engine: std({
      color: engineColor,
      roughness: military ? 0.62 : 0.35,
      metalness: military ? 0.14 : 0.25,
    }),
    logo: new THREE.MeshBasicMaterial({
      map: logoTexture(livery), transparent: true, depthWrite: false,
      polygonOffset: true, polygonOffsetFactor: -2,
    }),
    dark: new THREE.MeshStandardMaterial({ color: 0x20262e, roughness: 0.7 }),
    metal: new THREE.MeshStandardMaterial({ color: 0x9aa4ae, roughness: 0.35, metalness: 0.6 }),
    // Dark tinted near-black aerospace glazing with subtle sky/sun sheen.
    glass: new THREE.MeshStandardMaterial({ color: 0x162332, roughness: 0.12, metalness: 0.18 }),
    windowGlass: new THREE.MeshStandardMaterial({
      color: 0x162332, roughness: 0.12, metalness: 0.18,
      polygonOffset: true, polygonOffsetFactor: -2, polygonOffsetUnits: -2,
    }),
    frame: new THREE.MeshStandardMaterial({ color: 0x3b4552, roughness: 0.5, metalness: 0.25 }),
    rubber: new THREE.MeshStandardMaterial({ color: 0x141619, roughness: 0.92 }),
    // Heat-stained exhaust metal.
    nozzle: new THREE.MeshStandardMaterial({
      color: 0x4c4f54, roughness: 0.45, metalness: 0.7, flatShading: true,
    }),
    ordnance: new THREE.MeshStandardMaterial({ color: 0xe9ebee, roughness: 0.5 }),
    red: new THREE.MeshBasicMaterial({ color: 0xff3030 }),
    green: new THREE.MeshBasicMaterial({ color: 0x30ff6a }),
    white: new THREE.MeshBasicMaterial({ color: 0xffffff }),
    // Spinning propeller blur.
    propDisc: new THREE.MeshBasicMaterial({
      color: 0x222831, transparent: true, opacity: 0.16, side: THREE.DoubleSide, depthWrite: false,
    }),
    // Status beacon on the spine: recoloured by flight state in main.js.
    beacon: new THREE.MeshBasicMaterial({ color: 0xffffff }),
  };
  kit.all = Object.values(kit).filter((m) => m instanceof THREE.Material);
  return kit;
}

// Port (left, +X) red, starboard green, white tail light.
function navLights(group, kit, tip, tail) {
  group.add(dot(kit.red, tip[0], tip[1], tip[2], 0.16));
  group.add(dot(kit.green, -tip[0], tip[1], tip[2], 0.16));
  group.add(dot(kit.white, tail[0], tail[1], tail[2], 0.13));
}

// Distance from the group origin to its lowest point (world units after
// scaling), so aircraft sit exactly on the apron. Also attaches an invisible
// airframe hit proxy fitted to the model bounds for reliable pointer picking.
function finish(group, scale, kit, extra = {}) {
  group.updateMatrixWorld(true);
  const box = new THREE.Box3().setFromObject(group);
  // Conservative support hull for deck clearance under pitch/roll. Capture
  // before adding labels/lights/hit-proxy; geometry bounds include the deployed gear.
  const contactPoints = [];
  group.traverse((obj) => {
    if (!obj.isMesh) return;
    obj.geometry.computeBoundingBox();
    const b = obj.geometry.boundingBox;
    for (const x of [b.min.x, b.max.x])
      for (const y of [b.min.y, b.max.y])
        for (const z of [b.min.z, b.max.z])
          contactPoints.push(new THREE.Vector3(x, y, z).applyMatrix4(obj.matrixWorld));
  });

  const size = new THREE.Vector3();
  const center = new THREE.Vector3();
  box.getSize(size);
  box.getCenter(center);

  const pad = 1.4 / scale;
  const hitMat = new THREE.MeshBasicMaterial({ visible: false, side: THREE.DoubleSide });
  kit.all.push(hitMat);
  const hitProxy = new THREE.Mesh(
    new THREE.BoxGeometry(
      Math.max(size.x + pad, 6 / scale),
      Math.max(size.y + pad, 4 / scale),
      Math.max(size.z + pad, 6 / scale),
    ),
    hitMat,
  );
  hitProxy.name = 'hit-proxy';
  hitProxy.position.copy(center);
  hitProxy.castShadow = false;
  hitProxy.receiveShadow = false;
  hitProxy.userData.isHitProxy = true;
  group.add(hitProxy);

  return {
    group, scale, contactPoints,
    tintMaterials: [kit.beacon], disposeMaterials: kit.all,
    wheelDrop: -box.min.y * scale,
    airframeCenter: center,
    hitProxy,
    ...extra,
  };
}

/* ------------------------------------------------ private plane (local) */

// High-wing four-seat tourer in the Cessna 172 mould: local models.
// Built in metres (8.3 m long, 11 m span) around the thrust line, with z
// measured back from the spinner tip by `Z`; the airframe sits in a 1.5x
// group so the outer model space matches the other airframes.
export function makeAgentMesh(model) {
  const group = new THREE.Group();
  const g = new THREE.Group();
  g.scale.setScalar(1.5);
  group.add(g);
  const Z = (m) => 4.2 - m;

  // Side profile from the cowl face back: [station, half width, top, bottom].
  // Cowl, windscreen rising to the roof, flat cabin under the wing, then the
  // belly sweeping up into a deep, narrow tail cone.
  const body = loftBody([
    [8.1, 0.04, 0.52, 0.4], [7.7, 0.1, 0.55, 0.3], [6.8, 0.2, 0.6, 0.14],
    [5.6, 0.32, 0.7, -0.15], [4.6, 0.44, 0.82, -0.42], [3.85, 0.55, 1.0, -0.62],
    [3.0, 0.57, 1.02, -0.68], [2.15, 0.56, 1.0, -0.68], [1.35, 0.52, 0.6, -0.66],
    [0.6, 0.46, 0.5, -0.58], [0.25, 0.38, 0.36, -0.42],
  ].map(([m, ...r]) => [Z(m), ...r]));
  const kit = paintKit(model, 'prop', {
    zmin: body.zmin, zmax: body.zmax, circ: body.circ,
    bellyA: 2.25, stripeA: 1.55, stripeW: 0.1, stripeZ1: Z(0.6), swooshZ: Z(5.8),
    titles: { a: 1.25, z: Z(5.25), size: 0.36 },
    nose: { a: 1.3, z: Z(0.95), size: 0.13 },
  });
  const bodyMesh = new THREE.Mesh(body.geo, kit.body);
  bodyMesh.name = 'fuselage';
  g.add(bodyMesh);

  // Realistic Cessna 172 multi-pane cabin glazing fitted directly onto the
  // superellipse cabin body in true (z, y) coordinates so the raked 2-pane
  // windshield, vertical A/B pillars, rectangular door windows, tapered rear
  // side windows, and rear omni-vision window stay straight and crisp.
  g.add((() => {
    const verts = [];
    const norms = [];
    const idx = [];
    const eps = 0.014;
    const dzN = 0.01;
    const dpN = 0.01;

    const surfNorm = (z, phi) => {
      const pZ0 = body.pointAt(Math.max(body.zmin + 0.01, z - dzN), phi);
      const pZ1 = body.pointAt(Math.min(body.zmax - 0.01, z + dzN), phi);
      const pP0 = body.pointAt(z, Math.max(0.002, phi - dpN));
      const pP1 = body.pointAt(z, Math.min(Math.PI - 0.002, phi + dpN));
      const tx = pP1[0] - pP0[0], ty = pP1[1] - pP0[1], tz = pP1[2] - pP0[2];
      const ux = pZ1[0] - pZ0[0], uy = pZ1[1] - pZ0[1], uz = pZ1[2] - pZ0[2];
      let nx = ty * uz - tz * uy, ny = tz * ux - tx * uz, nz = tx * uy - ty * ux;
      const l = Math.hypot(nx, ny, nz) || 1;
      nx /= l; ny /= l; nz /= l;
      const p = body.pointAt(z, phi);
      const cy = (body.pointAt(z, 0)[1] + body.pointAt(z, Math.PI)[1]) * 0.5;
      if (nx * p[0] + ny * (p[1] - cy) < 0 && p[0] > 0.02) {
        nx = -nx; ny = -ny; nz = -nz;
      }
      return { p, n: [nx, ny, nz] };
    };

    const phiForY = (z, y) => {
      let lo = 0, hi = Math.PI;
      for (let k = 0; k < 18; k++) {
        const mid = (lo + hi) * 0.5;
        if (body.pointAt(z, mid)[1] < y) lo = mid; else hi = mid;
      }
      return (lo + hi) * 0.5;
    };

    const emitPatch = (grid, nu, nv, mirror = true) => {
      let anx = 0, any = 0, anz = 0;
      for (const c of grid) { anx += c.n[0]; any += c.n[1]; anz += c.n[2]; }
      const al = Math.hypot(anx, any, anz) || 1;
      anx /= al; any /= al; anz /= al;
      for (const side of (mirror ? [1, -1] : [1])) {
        const base = verts.length / 3;
        for (const c of grid) {
          verts.push(side * (c.p[0] + c.n[0] * eps), c.p[1] + c.n[1] * eps, c.p[2] + c.n[2] * eps);
          let vx = side * (c.n[0] * 0.25 + anx * 0.75);
          let vy = c.n[1] * 0.25 + any * 0.75;
          let vz = c.n[2] * 0.25 + anz * 0.75;
          const vl = Math.hypot(vx, vy, vz) || 1;
          norms.push(vx / vl, vy / vl, vz / vl);
        }
        const stride = nv + 1;
        for (let i = 0; i < nu; i++) {
          for (let j = 0; j < nv; j++) {
            const a = base + i * stride + j;
            const b = base + (i + 1) * stride + j;
            const c = base + (i + 1) * stride + (j + 1);
            const d = base + i * stride + (j + 1);
            const ax = verts[a * 3], ay = verts[a * 3 + 1], az = verts[a * 3 + 2];
            const bx = verts[b * 3] - ax, by = verts[b * 3 + 1] - ay, bz = verts[b * 3 + 2] - az;
            const cx = verts[c * 3] - ax, cy = verts[c * 3 + 1] - ay, cz = verts[c * 3 + 2] - az;
            const crx = by * cz - bz * cy, cry = bz * cx - bx * cz, crz = bx * cy - by * cx;
            if (crx * (side * anx) + cry * any + crz * anz >= 0) idx.push(a, b, c, a, c, d);
            else idx.push(a, c, b, a, d, c);
          }
        }
      }
    };

    // 1) Front 2-pane raked windshield (z runs cowl -> roof, angle from centre post -> A-pillar).
    {
      const nu = 4, nv = 4;
      const grid = [];
      for (let i = 0; i <= nu; i++) {
        const u = i / nu;
        for (let j = 0; j <= nv; j++) {
          const v = j / nv;
          const z = Z(1.42) + (Z(2.10) - Z(1.42)) * v;
          // Angle from top spine: thin centre strip (0.045 rad) out to raked A-pillar corner.
          const aMax = 0.62 + 0.14 * (1 - v);
          const a = 0.045 + (aMax - 0.045) * u;
          grid.push(surfNorm(z, Math.PI - a));
        }
      }
      emitPatch(grid, nu, nv, true);
    }

    // 2) Cabin door side window (raked front A-pillar, crisp horizontal sill & header, vertical B-pillar).
    const emitSideQuad = (zFBot, zFTop, zABot, zATop, yBotF, yTopF, yBotA, yTopA) => {
      const nu = 4, nv = 2;
      const grid = [];
      for (let i = 0; i <= nu; i++) {
        const u = i / nu;
        for (let j = 0; j <= nv; j++) {
          const v = j / nv;
          const zBot = zFBot + (zABot - zFBot) * u;
          const zTop = zFTop + (zATop - zFTop) * u;
          const z = zBot + (zTop - zBot) * v;
          const yBot = yBotF + (yBotA - yBotF) * u;
          const yTop = yTopF + (yTopA - yTopF) * u;
          const y = yBot + (yTop - yBot) * v;
          grid.push(surfNorm(z, phiForY(z, y)));
        }
      }
      emitPatch(grid, nu, nv, true);
    };
    // Door window (front edge parallels the raked A-pillar from Z(1.78) bottom to Z(2.16) top).
    emitSideQuad(Z(1.78), Z(2.16), Z(2.94), Z(2.94), 0.36, 0.90, 0.36, 0.91);
    // Rear cabin side window (aft of crisp B-pillar, gently tapering aft).
    emitSideQuad(Z(3.04), Z(3.04), Z(3.84), Z(3.84), 0.36, 0.91, 0.50, 0.85);

    // 3) Rear omni-vision wraparound window behind the wing.
    {
      const nu = 6, nv = 2;
      const grid = [];
      for (let i = 0; i <= nu; i++) {
        const u = i / nu;
        for (let j = 0; j <= nv; j++) {
          const v = j / nv;
          const z = Z(4.36) + (Z(3.92) - Z(4.36)) * v;
          const aLim = 0.34 + (0.44 - 0.34) * v;
          const ang = -aLim + 2 * aLim * u;
          const s = surfNorm(z, Math.PI - Math.abs(ang));
          const sgn = ang < 0 ? -1 : 1;
          grid.push({ p: [sgn * s.p[0], s.p[1], s.p[2]], n: [sgn * s.n[0], s.n[1], s.n[2]] });
        }
      }
      emitPatch(grid, nu, nv, false);
    }

    const geo = new THREE.BufferGeometry();
    geo.setAttribute('position', new THREE.Float32BufferAttribute(verts, 3));
    geo.setAttribute('normal', new THREE.Float32BufferAttribute(norms, 3));
    geo.setIndex(idx);
    const mesh = new THREE.Mesh(geo, kit.windowGlass);
    mesh.name = 'cockpit-glazing';
    return mesh;
  })());

  // Spinner, two-blade 1.9 m prop, round cooling inlets beside the spinner,
  // exhaust stack under the cowl.
  const spinner = new THREE.Mesh(new THREE.ConeGeometry(0.2, 0.42, 20), kit.engine);
  spinner.rotation.x = Math.PI / 2;
  spinner.position.set(0, -0.03, Z(0.25) + 0.21);
  g.add(spinner);
  const prop = propeller(kit, { blades: 2, radius: 0.95, rootChord: 0.17, tipChord: 0.1, hub: 0.12 });
  prop.position.set(0, -0.03, Z(0.25) + 0.08);
  g.add(prop);
  for (const side of [1, -1]) {
    const inlet = new THREE.Mesh(new THREE.CircleGeometry(0.075, 16), kit.dark);
    inlet.position.set(side * 0.27, 0.06, Z(0.25) + 0.003);
    g.add(inlet);
  }
  g.add(rod(kit.nozzle, [-0.22, -0.6, Z(1.05)], [-0.24, -0.72, Z(1.3)], 0.035));

  // Wing on the cabin roof: constant-chord centre section, tapered outer
  // panels, 1.7 degrees of dihedral. Single lift strut each side.
  const wingY = 1.04;
  const dihedral = 0.03;
  const rise = Math.tan(dihedral);
  const inner = { le: Z(2.15), chord: 1.63, t: 0.125 };
  const tip = { le: Z(2.22), chord: 1.12, t: 0.11 };
  for (const w of surfacePair(kit.wingPlainSkin, {
    span: 2.6, dihedral, root: { ...inner, y: wingY, t: 0.13 }, tip: inner,
  })) g.add(w);
  for (const w of surfacePair(kit.wingPlainSkin, {
    span: 2.9, x0: 2.6, dihedral, root: { ...inner, y: wingY + rise * 2.6 }, tip,
  })) g.add(w);
  for (const side of [1, -1]) {
    g.add(rod(kit.plain, [side * 0.5, -0.45, Z(3.05)], [side * 2.9, wingY + rise * 2.9 - 0.06, Z(2.75)], 0.04));
  }

  // Stabiliser low on the tail cone, swept fin with a long dorsal fillet.
  for (const t of surfacePair(kit.tailPlainSkin, {
    span: 1.72, thickness: 0.1,
    root: { le: Z(6.85), chord: 1.25, y: 0.38 }, tip: { le: Z(7.2), chord: 0.8 },
  })) g.add(t);
  const finBase = 0.55;
  const finH = 1.45;
  g.add(finMesh(kit.fin, {
    height: finH, thickness: 0.1, base: [0, finBase],
    root: { le: Z(6.55), chord: 1.72 }, tip: { le: Z(7.55), chord: 0.85 },
    logo: kit.logo, logoAt: 0.42, logoScale: 0.8,
  }));
  g.add(finMesh(kit.fin, {
    height: 0.32, thickness: 0.12, base: [0, 0.5],
    root: { le: Z(4.9), chord: 1.75 }, tip: { le: Z(6.45), chord: 0.35 },
  }));

  // Fixed tricycle gear: spring-steel main legs and an oleo nose leg, all
  // with wheel fairings. Wheels put the thrust line 1.27 m off the ground.
  const spatGeo = (len, r) => fuselage([
    [-len * 0.5, 0.02], [-len * 0.28, r * 0.75], [len * 0.05, r], [len * 0.32, r * 0.72], [len * 0.5, 0.0001],
  ], { sx: 0.5, sy: 1.2, segments: 16 }).geo;
  for (const side of [1, -1]) {
    g.add(rod(kit.dark, [side * 0.5, -0.62, Z(3.55)], [side * 1.2, -0.98, Z(3.65)], 0.035));
    g.add(wheel(kit, side * 1.24, -1.02, Z(3.65), 0.25, 0.14));
    const spat = new THREE.Mesh(spatGeo(0.95, 0.23), kit.engine);
    spat.position.set(side * 1.24, -0.99, Z(3.62));
    g.add(spat);
  }
  g.add(rod(kit.metal, [0, -0.5, Z(0.85)], [0, -1.08, Z(0.95)], 0.035));
  g.add(wheel(kit, 0, -1.08, Z(0.95), 0.19, 0.11));
  const noseSpat = new THREE.Mesh(spatGeo(0.7, 0.18), kit.engine);
  noseSpat.position.set(0, -1.05, Z(0.93));
  g.add(noseSpat);

  // Antenna, nav lights at the wingtips and tail cone, beacon atop the fin.
  g.add(rod(kit.dark, [0, 0.95, Z(4.2)], [0, 1.22, Z(4.4)], 0.012));
  const tipY = wingY + rise * 5.5;
  group.add(dot(kit.red, 5.53 * 1.5, tipY * 1.5, (tip.le - 0.3) * 1.5, 0.16));
  group.add(dot(kit.green, -5.53 * 1.5, tipY * 1.5, (tip.le - 0.3) * 1.5, 0.16));
  group.add(dot(kit.white, 0, 0.47 * 1.5, Z(8.12) * 1.5, 0.13));
  group.add(dot(kit.beacon, 0, (finBase + finH + 0.05) * 1.5, Z(7.95) * 1.5, 0.15));

  return finish(group, 0.92, kit, {
    refuelPort: new THREE.Vector3(0, 1.56 * 1.5, Z(2.3) * 1.5),
    landingLightPos: new THREE.Vector3(0, -0.22 * 1.5, (Z(0.25) + 0.14) * 1.5),
  });
}

/* -------------------------------------------- twin-engine jet (frontier) */

// Turbofan in model space with the intake facing +Z: fan cowl in the
// livery's engine colour, polished intake lip, fan face, core nozzle and
// exhaust plug.
function turbofan(kit) {
  const g = new THREE.Group();
  g.add(new THREE.Mesh(fuselage([
    [-0.9, 0.62], [-0.3, 0.8], [0.6, 0.93], [1.8, 0.96], [2.3, 0.92], [2.42, 0.86],
  ], { segments: 24 }).geo, kit.engine));
  const lip = new THREE.Mesh(new THREE.TorusGeometry(0.84, 0.07, 8, 28), kit.metal);
  lip.position.z = 2.42;
  g.add(lip);
  const fan = new THREE.Mesh(new THREE.CircleGeometry(0.84, 24), kit.dark);
  fan.position.z = 2.2;
  g.add(fan);
  const spinner = new THREE.Mesh(new THREE.ConeGeometry(0.24, 0.45, 14), kit.metal);
  spinner.rotation.x = Math.PI / 2;
  spinner.position.z = 2.42;
  g.add(spinner);
  // Bypass exit ring, then the core nozzle and plug behind it.
  const bypass = new THREE.Mesh(new THREE.RingGeometry(0.5, 0.63, 24), kit.dark);
  bypass.rotation.y = Math.PI;
  bypass.position.z = -0.88;
  g.add(bypass);
  g.add(new THREE.Mesh(fuselage([
    [-1.85, 0.36], [-1.4, 0.45], [-0.9, 0.55], [-0.6, 0.56],
  ], { segments: 18, smooth: false }).geo, kit.nozzle));
  const core = new THREE.Mesh(new THREE.RingGeometry(0.2, 0.36, 18), kit.dark);
  core.rotation.y = Math.PI;
  core.position.z = -1.84;
  g.add(core);
  g.add(new THREE.Mesh(fuselage([
    [-2.5, 0.0001], [-2.1, 0.14], [-1.7, 0.24], [-1.4, 0.26],
  ], { segments: 14 }).geo, kit.nozzle));
  return g;
}

// Narrow-body twin jet: frontier models.
export function makeJetMesh(model) {
  const group = new THREE.Group();
  const warp = (z) => (z < -5.5 ? (-5.5 - z) * 0.13 : 0) - (z > 9.5 ? (z - 9.5) * 0.1 : 0);
  const body = fuselage([
    [-12.6, 0.12], [-11.6, 0.55], [-9.5, 1.15], [-6.5, 1.65], [-4, 1.75],
    [8, 1.75], [9.8, 1.6], [11.2, 1.25], [12.2, 0.8], [12.8, 0.35], [13.0, 0.0001],
  ], { sy: 1.05, warp, segments: 40 });
  const kit = paintKit(model, 'jet', {
    zmin: body.zmin, zmax: body.zmax, circ: body.circ,
    bellyA: 2.05, stripeA: 1.5, stripeW: 0.1, stripeZ1: 11.2, swooshZ: -5.0,
    windows: { kind: 'row', a: 1.16, z0: -5.6, z1: 8.4, pitch: 0.62, w: 0.22, h: 0.3 },
    titles: { a: 0.68, z: 2.2, size: 1.05 },
    operator: { a: 0.78, z: 7.7, size: 0.34 },
    nose: { a: 1.34, z: 9.4, size: 0.34 },
  });
  const bodyMesh = new THREE.Mesh(body.geo, kit.body);
  bodyMesh.name = 'fuselage';
  group.add(bodyMesh);

  // Realistic 6-pane airliner flight-deck glazing (A320 / 737 style): two main
  // forward panes with subtle centre brow peak, two side-forward sliding panes,
  // and two slim tapered aft side windows with crisp thin body-coloured frames.
  group.add(cockpitGlazingMesh(body, kit.windowGlass, [
    { knots: [[0.035, 0.42, 0.89], [0.42, 0.43, 0.88], [0.78, 0.44, 0.86]], subdivS: 3, subdivY: 2 },
    { knots: [[0.845, 0.44, 0.855], [1.46, 0.47, 0.825]], subdivS: 3, subdivY: 2 },
    { knots: [[1.525, 0.47, 0.82], [2.08, 0.59, 0.75]], subdivS: 3, subdivY: 2 },
  ], {
    zSearchStart: 12.95,
    zSearchEnd: 8.8,
    offset: 0.022,
    flatBlend: 0.78,
  }));
  // APU exhaust in the tail cone.
  const apu = new THREE.Mesh(new THREE.CircleGeometry(0.11, 12), kit.dark);
  apu.rotation.y = Math.PI;
  apu.position.set(0, warp(-12.6), -12.62);
  group.add(apu);

  // Low swept wing with a trailing-edge crank at the engine station: thick,
  // broad inboard panel, thinner swept outboard panel, belly fairing.
  const wingY = -0.95;
  const wingSpan = 12.5;
  const kinkX = 4.6;
  const wingDihedral = 0.09;
  const rise = Math.tan(wingDihedral);
  const kink = { le: 0.55, chord: 3.75, t: 0.12 };
  const tip = { le: -3.6, chord: 1.35, t: 0.1 };
  for (const w of surfacePair(kit.wingSkin, {
    span: kinkX, dihedral: wingDihedral,
    root: { le: 3.2, chord: 6.6, y: wingY, t: 0.15 }, tip: kink,
  })) group.add(w);
  for (const w of surfacePair(kit.wingSkin, {
    span: wingSpan - kinkX, x0: kinkX, dihedral: wingDihedral,
    root: { ...kink, y: wingY + rise * kinkX }, tip,
  })) group.add(w);
  const wingYAt = (x) => wingY + rise * x;
  // Trailing-edge z at spanwise station x.
  const kinkTe = kink.le - kink.chord;
  const tipTe = tip.le - tip.chord;
  function teAt(x) {
    if (x < kinkX) return -3.4 + (kinkTe + 3.4) * (x / kinkX);
    return kinkTe + (tipTe - kinkTe) * ((x - kinkX) / (wingSpan - kinkX));
  }

  // Winglets in the tail colour.
  const tipY = wingYAt(wingSpan);
  for (const side of [1, -1]) {
    group.add(finMesh(kit.fin, {
      height: 1.4, thickness: 0.08, base: [side * wingSpan, tipY], cant: 0.35, side,
      root: { le: tip.le, chord: tip.chord }, tip: { le: -4.6, chord: 0.55 },
    }));
  }
  group.add(new THREE.Mesh(fuselage([
    [-3.6, 0.2], [-2.5, 0.9], [1.5, 1.0], [3.4, 0.5], [3.9, 0.0001],
  ], { sx: 1.7, sy: 0.6, warp: () => -1.05 }).geo, kit.wing));

  // Flap-track fairings under the trailing edge.
  for (const side of [1, -1]) {
    for (const x of [2.9, 6.9, 9.3]) {
      const pod = new THREE.Mesh(fuselage([
        [-1.3, 0.0001], [-0.9, 0.13], [0.2, 0.17], [1.1, 0.0001],
      ], { sx: 0.6, segments: 10 }).geo, kit.wing);
      pod.position.set(side * x, wingYAt(x) - 0.1, teAt(x) + 0.55);
      group.add(pod);
    }
  }

  // Engines slung ahead of the wing on swept pylons.
  const ex = kinkX;
  const ey = wingYAt(ex) - 1.25;
  const ez = kink.le + 0.3;
  for (const side of [1, -1]) {
    const engine = turbofan(kit);
    engine.position.set(side * ex, ey, ez);
    group.add(engine);
    group.add(finMesh(kit.wing, {
      height: 0.55, thickness: 0.16, base: [side * ex, wingYAt(ex) + 0.05], cant: Math.PI,
      root: { le: kink.le + 0.2, chord: 2.6 }, tip: { le: ez + 1.7, chord: 3.6 },
    }));
  }

  // Swept stabilisers, a dorsal fillet and a tall branded fin.
  const tailY = warp(-10);
  for (const t of surfacePair(kit.tailSkin, {
    span: 4.8, dihedral: 0.1,
    root: { le: -8.4, chord: 3.0, y: tailY, t: 0.11 }, tip: { le: -11.0, chord: 1.2, t: 0.09 },
  })) group.add(t);
  group.add(finMesh(kit.fin, {
    height: 5.0, thickness: 0.11, base: [0, tailY + 0.9],
    root: { le: -6.6, chord: 4.6 }, tip: { le: -10.6, chord: 1.8 },
    logo: kit.logo, logoAt: 0.42,
  }));
  group.add(finMesh(kit.fin, {
    height: 0.55, thickness: 0.12, base: [0, 1.55],
    root: { le: -4.4, chord: 2.6 }, tip: { le: -6.5, chord: 0.8 },
  }));

  // Retractable gear (cycled by main.js): twin-wheel main legs with side
  // braces folding inboard and aft into the belly, twin-wheel nose leg folding
  // aft.
  const rig = gearRig();
  for (const side of [1, -1]) {
    const pivot = [side * 2.2, wingY - 0.2, -1.2];
    const leg = rig.leg(pivot, [
      rod(kit.metal, pivot, [side * 2.2, -3.3, -1.2], 0.14),
      rod(kit.metal, [side * 1.9, -3.4, -1.2], [side * 2.5, -3.4, -1.2], 0.09),
      ...[1.92, 2.48].map((dx) => wheel(kit, side * dx, -3.4, -1.2, 0.58, 0.34)),
    ], foldToward(pivot, [side * 2.2, -3.4, -1.2], [-side * 0.67, 0.05, -0.74]));
    rig.brace(kit.metal, [side * 1.2, -1.4, -1.2], [side * 2.2, -2.5, -1.2], 0.06, leg);
  }
  const noseLeg = rig.leg([0, -1.4, 9.2], [
    rod(kit.metal, [0, -1.4, 9.2], [0, -3.4, 9.2], 0.12),
    wheel(kit, 0.22, -3.5, 9.2, 0.38, 0.25),
    wheel(kit, -0.22, -3.5, 9.2, 0.38, 0.25),
  ], foldAbout('x', 1.66));
  rig.brace(kit.metal, [0, -1.6, 8.3], [0, -2.6, 9.2], 0.05, noseLeg);
  group.add(rig.gear);

  navLights(group, kit, [wingSpan - 0.2, tipY + 0.1, -3.9], [0, tailY + 0.2, -12.7]);
  group.add(dot(kit.beacon, 0, 1.86, 0.5, 0.24));
  group.add(dot(kit.beacon, 0, -1.86, 4.5, 0.24));

  return finish(group, 0.74, kit, {
    refuelPort: new THREE.Vector3(0, 1.82, 8.4),
    landingLightPos: new THREE.Vector3(0, -0.5, 13.9), // just ahead of the nose tip (z=13.0)
    landingGlowScale: 0.6,
  });
}

/* ------------------------------- Boeing 747 jumbo jet (flagship models) */

// Smooth polynomial maximum: equals max(a, b) everywhere except a C1-smooth
// fillet of width k where a and b cross. Used to blend the 747's upper-deck
// lobe seamlessly into the main widebody cylinder with zero crease or seam.
function smax(a, b, k) {
  if (k <= 1e-6) return Math.max(a, b);
  const h = Math.max(k - Math.abs(a - b), 0) / k;
  return Math.max(a, b) + h * h * k * 0.25;
}

// Single seamless double-bubble Boeing 747 fuselage lofted from explicit
// longitudinal stations `[z, w1, yBot, yTop1, yTop2, w2]` (tail -> nose):
//   w1, yBot, yTop1 : main widebody ellipse half-width, keel, and main crown
//   yTop2, w2       : upper-deck hump crown and lobe half-width (w2 = 0 aft)
// Using explicit top/bottom heights keeps the aft spine dead-straight and
// horizontal while ONLY the aft belly sweeps upward into the tailcone, and
// produces the authentic narrow-top / wide-bottom 747 double-bubble section.
function jumboFuselage(stations, { segments = 56 } = {}) {
  // Linear/cubic-Hermite (monotone-safe Catmull-Rom with clamped overshoot)
  // sampling across explicit stations so straight sections stay dead flat.
  const zmin = stations[0][0];
  const zmax = stations[stations.length - 1][0];
  const nSt = stations.length;

  // Monotone Fritsch-Carlson cubic Hermite interpolation per channel prevents
  // spline ringing on straight keel/spine segments and sharp nose radomes.
  function makeHermite(col) {
    const xs = stations.map((s) => s[0]);
    const ys = stations.map((s) => s[col]);
    const d = [];
    for (let i = 0; i < nSt - 1; i++) {
      d.push((ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]));
    }
    const m = new Array(nSt).fill(0);
    m[0] = d[0];
    m[nSt - 1] = d[nSt - 2];
    for (let i = 1; i < nSt - 1; i++) {
      if (d[i - 1] * d[i] <= 0) m[i] = 0;
      else m[i] = (d[i - 1] + d[i]) * 0.5;
    }
    for (let i = 0; i < nSt - 1; i++) {
      if (Math.abs(d[i]) < 1e-9) {
        m[i] = 0; m[i + 1] = 0;
      } else {
        const a = m[i] / d[i];
        const b = m[i + 1] / d[i];
        const s = Math.hypot(a, b);
        if (s > 3) {
          m[i] = (3 / s) * a * d[i];
          m[i + 1] = (3 / s) * b * d[i];
        }
      }
    }
    return (z) => {
      if (z <= xs[0]) return ys[0];
      if (z >= xs[nSt - 1]) return ys[nSt - 1];
      let lo = 0, hi = nSt - 1;
      while (hi - lo > 1) {
        const mid = (lo + hi) >> 1;
        if (xs[mid] <= z) lo = mid; else hi = mid;
      }
      const h = xs[hi] - xs[lo];
      const t = (z - xs[lo]) / h;
      const t2 = t * t, t3 = t2 * t;
      return (2 * t3 - 3 * t2 + 1) * ys[lo]
        + (t3 - 2 * t2 + t) * h * m[lo]
        + (-2 * t3 + 3 * t2) * ys[hi]
        + (t3 - t2) * h * m[hi];
    };
  }

  const evalW1 = makeHermite(1);
  const evalYBot = makeHermite(2);
  const evalYTop1 = makeHermite(3);
  const evalYTop2 = makeHermite(4);
  const evalW2 = makeHermite(5);

  // Ray distance from (0, 0) along (dx, dy) to ellipse centered at (0, cy) with radii (a, b).
  const rayEllipse = (dx, dy, cy, a, b) => {
    const A = (dx * dx) / (a * a) + (dy * dy) / (b * b);
    const B = (-2 * cy * dy) / (b * b);
    const C = (cy * cy) / (b * b) - 1;
    const disc = B * B - 4 * A * C;
    if (disc < 0) return 0;
    return (-B + Math.sqrt(disc)) / (2 * A);
  };

  const pointAt = (z, phi) => {
    const w1 = Math.max(evalW1(z), 0.0005);
    const yBot = evalYBot(z);
    const yTop1 = Math.max(evalYTop1(z), yBot + 0.001);
    const yTop2 = Math.max(evalYTop2(z), yTop1);
    const w2 = Math.max(evalW2(z), 0);
    const cy1 = (yTop1 + yBot) * 0.5;
    const b1 = Math.max((yTop1 - yBot) * 0.5, 0.0005);
    const hRise = yTop2 - yTop1;
    const useHump = hRise > 0.012 && w2 > 0.08 && w1 > 0.5;

    const dx = Math.sin(phi);
    const dy = -Math.cos(phi);
    let rad = rayEllipse(dx, dy, 0, w1, b1);
    if (useHump && dy > -0.15) {
      // Upper-deck ellipse anchored so its top reaches yTop2 exactly while its
      // bottom sits strictly below cy1 (guaranteeing single positive ray hit).
      const spanUp = yTop2 - cy1;
      const cy2Rel = 0.49 * spanUp;
      const b2 = 0.51 * spanUp;
      const r2 = rayEllipse(dx, dy, cy2Rel, w2, b2);
      const blendK = Math.min(0.24, 0.22 * Math.min(1, hRise / 0.45));
      rad = smax(rad, r2, blendK);
    }
    return [dx * rad, cy1 + dy * rad, z];
  };

  // Build dense longitudinal rings with extra clustering across the blunt nose
  // radome, raked cockpit forehead, hump taper, and tailcone.
  const zRings = [];
  for (let i = 0; i < nSt - 1; i++) {
    const z0 = stations[i][0];
    const z1 = stations[i + 1][0];
    const span = z1 - z0;
    const sub = z1 > zmax - 10 ? 6 : Math.max(3, Math.round(span / 1.4));
    for (let k = 0; k < sub; k++) zRings.push(z0 + span * (k / sub));
  }
  zRings.push(zmax);

  const S = segments;
  const pos = [];
  const uv = [];
  const idx = [];
  for (let i = 0; i < zRings.length; i++) {
    const z = zRings[i];
    const v = (z - zmin) / (zmax - zmin);
    for (let j = 0; j <= S; j++) {
      const phi = (j / S) * Math.PI * 2;
      const p = pointAt(z, phi);
      pos.push(p[0], p[1], p[2]);
      uv.push(j / S, v);
    }
  }
  for (let i = 0; i < zRings.length - 1; i++) {
    for (let j = 0; j < S; j++) {
      const a = i * (S + 1) + j;
      const b = a + S + 1;
      idx.push(a, a + 1, b, a + 1, b + 1, b);
    }
  }
  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
  geo.setAttribute('uv', new THREE.Float32BufferAttribute(uv, 2));
  geo.setIndex(idx);
  geo.computeVertexNormals();
  const maxW = Math.max(...stations.map((s) => s[1]));
  const maxH = Math.max(...stations.map((s) => (s[3] - s[2]) * 0.5));
  return { geo, zmin, zmax, circ: 2 * Math.PI * (maxW + maxH) / 2, pointAt };
}

// Boeing 747-400 ("Queen of the Sky") — flagship frontier models (flagship.js).
// Built in true metres (z = 34.3 - m, where m is metres aft of nose tip) and
// scaled uniformly by 0.355 to fit Skywatch's gates, taxiways, and Runway 36
// while keeping exact real-world proportions.
//
// Verified Boeing 747-400 reference dimensions (Boeing D6-58326-1 Airport
// Planning Manual, Boeing 747 Structural Repair Manual, ATA Ch. 56, Wikipedia):
//   Overall length        : 70.66 m (231 ft 10 in); fuselage body 68.63 m
//   Wingspan (w/ winglets): 64.44 m (211 ft 5 in) = 0.912 x length; 6 ft (1.83 m) canted winglets
//   Overall tail height   : 19.41 m (63 ft 8 in); top of upper deck 9.82-10.24 m above ground
//   Main fuselage section : 6.50 m wide (256 in) x 6.81 m tall (268 in), L:W = 10.87:1
//   Hump keel-to-crown    : 7.85 m structural shell (309 in, WL 91 to WL 400) / ~8.0 m w/ fairing;
//                           upper-deck cabin ~15.8-16.5 m interior length (10-11 windows/side),
//                           3.45 m interior floor width (~4.16 m exterior width, approx. — exact
//                           exterior BL unconfirmed); aft fairing taper end (~35 m) approx. from 3-views
//   Wing geometry         : 37.5 deg quarter-chord sweep (~39.8 deg LE), 7 deg dihedral,
//                           ~14.84 m theoretical root chord, 3.70 m tip chord, Yehudi TE kink at
//                           inboard engine station (~37% semi-span)
//   Engines (4x CF6/PW4k) : 2.74 m cowl OD (93-94 in fan); inboard at 11.9 m (39 ft = 36.9%
//                           semi-span), outboard at 21.2 m (69 ft 6 in = 65.8% semi-span),
//                           slung well forward of and below wing LE on swept pylons
//   Horizontal stabiliser : 22.17 m span (72 ft 9 in), 37 deg quarter-chord sweep, 7 deg dihedral
//   Landing gear (18 whl) : 25.60 m wheelbase (84 ft 0 in), 11.00 m outer track (36 ft 1 in),
//                           4x 4-wheel main bogies (2 wing + 2 aft body, 1.24 m H49x19-22 tyres) +
//                           twin-wheel nose gear
//   Flight deck windows   : 6 primary panes (No. 1 L/R forward, No. 2 L/R sliding side, No. 3 L/R
//                           raked aft side) high on the upper-deck brow above the blunt rounded chin
export function makeJumboMesh(model) {
  const group = new THREE.Group();
  const M = (m) => 34.3 - m; // m = metres aft of nose tip (0 -> 68.63 m tailcone)

  // Fuselage stations from tailcone (m=68.63) forward to nose tip (m=0.0):
  // [z, w1, yBot, yTop1, yTop2, w2]
  // Main constant cylinder: 6.50 m wide x 6.60 m tall (±3.30 m); stretched
  // upper-deck hump rises cleanly to +4.75 m (8.05 m keel-to-crown, +1.45 m
  // proud of the straight horizontal aft spine) with a distinct nose-to-brow
  // inflection at the 43 deg raked flight-deck windscreen and a proud ~1.0 m
  // curved forehead rising above the cockpit glass.
  const stations = [
    [68.63, 0.22, 1.66, 2.10, 2.10, 0.0],
    [67.80, 0.42, 1.48, 2.26, 2.26, 0.0],
    [65.00, 0.98, 0.84, 2.78, 2.78, 0.0],
    [61.00, 1.78, -0.26, 3.24, 3.24, 0.0],
    [56.50, 2.54, -1.55, 3.30, 3.30, 0.0],
    [52.00, 3.04, -2.60, 3.30, 3.30, 0.0],
    [47.50, 3.25, -3.30, 3.30, 3.30, 0.0],
    [42.00, 3.25, -3.30, 3.30, 3.30, 0.0],
    // Long graceful 747-400 aft hump fairing tapering smoothly into straight spine.
    [35.20, 3.25, -3.30, 3.30, 3.30, 0.0],
    [32.50, 3.25, -3.30, 3.30, 3.48, 1.12],
    [29.50, 3.25, -3.30, 3.30, 3.84, 1.50],
    [26.50, 3.25, -3.30, 3.30, 4.26, 1.78],
    [23.50, 3.25, -3.30, 3.30, 4.60, 1.96],
    // Stretched 747-400 upper-deck level roof (~13 m full-height hump crest).
    [21.00, 3.25, -3.30, 3.30, 4.75, 2.02],
    [16.00, 3.25, -3.30, 3.30, 4.75, 2.02],
    [11.20, 3.25, -3.30, 3.30, 4.75, 2.02],
    [8.20, 3.25, -3.30, 3.30, 4.72, 1.99],
    // Proud curved upper forehead rounding down into the 43 deg windscreen face.
    [7.00, 3.22, -3.30, 3.26, 4.56, 1.92],
    [6.00, 3.14, -3.30, 3.16, 4.16, 1.80],
    [5.10, 3.00, -3.28, 2.98, 3.46, 1.62],
    [4.30, 2.84, -3.22, 2.66, 2.68, 1.38],
    // Blunt rounded lower chin and drooping nose radome below the windscreen.
    [3.30, 2.58, -3.08, 2.18, 2.18, 0.0],
    [2.20, 2.20, -2.80, 1.62, 1.62, 0.0],
    [1.25, 1.72, -2.40, 1.02, 1.02, 0.0],
    [0.58, 1.20, -1.94, 0.36, 0.36, 0.0],
    [0.18, 0.70, -1.48, -0.22, -0.22, 0.0],
    [0.00, 0.001, -0.90, -0.899, -0.899, 0.0],
  ].map(([m, ...rest]) => [M(m), ...rest]);

  const body = jumboFuselage(stations, { segments: 56 });
  const doorZs = [M(9.2), M(21.4), M(35.0), M(46.8), M(56.8)];

  const kit = paintKit(model, 'jumbo', {
    zmin: body.zmin, zmax: body.zmax, circ: body.circ,
    texW: 1024, texH: 2048,
    bellyA: 2.08, stripeA: 1.60, stripeW: 0.085, stripeZ1: M(4.5), swooshZ: M(51.5),
    // One row of small oval main-deck cabin windows (with door gaps) plus a
    // crisp short row of 11 small upper-deck windows; none on the hump top.
    windows: [
      {
        kind: 'row', a: 1.38, z0: M(57.8), z1: M(8.2), pitch: 0.94, w: 0.32, h: 0.44,
        skipZs: doorZs, skipMargin: 1.05,
      },
      { kind: 'row', a: 0.49, z0: M(20.6), z1: M(9.8), pitch: 1.05, w: 0.28, h: 0.36 },
    ],
    doors: { a: 1.44, zs: doorZs, w: 1.05, h: 2.05 },
    // Airline titles squarely on the main-fuselage side flank (never on the hump).
    titles: { a: 1.05, z: M(40.5), size: 2.55 },
    operator: { a: 1.13, z: M(29.8), size: 0.68 },
    nose: { a: 1.18, z: M(14.5), size: 0.68 },
  });
  const bodyMesh = new THREE.Mesh(body.geo, kit.body);
  bodyMesh.name = 'fuselage';
  group.add(bodyMesh);

  // Iconic 6-pane Boeing 747-400 flight-deck glazing set on the raked front
  // face of the upper deck below the proud curved forehead brow:
  // No. 1 L/R central windscreens with subtle pointed eyebrow peak at the
  // centre post, No. 2 L/R side-forward sliding windows, No. 3 L/R slim raked
  // aft side windows tapering to a sharp rear corner.
  group.add(cockpitGlazingMesh(body, kit.windowGlass, [
    // No. 1 forward windshield (slight pointed eyebrow brow near centre post).
    { knots: [[0.06, 2.94, 3.70], [0.46, 2.96, 3.73], [1.16, 3.01, 3.67]], subdivS: 3, subdivY: 2 },
    // No. 2 side-forward sliding window.
    { knots: [[1.25, 3.02, 3.66], [2.24, 3.10, 3.61]], subdivS: 3, subdivY: 2 },
    // No. 3 aft fixed side window with signature 747 swept-back rear corner.
    { knots: [[2.33, 3.11, 3.60], [3.32, 3.38, 3.50]], subdivS: 3, subdivY: 2 },
  ], {
    zSearchStart: M(3.6),
    zSearchEnd: M(11.5),
    offset: 0.045,
    flatBlend: 0.78,
  }));

  // APU exhaust in the tapered tailcone.
  const apu = new THREE.Mesh(new THREE.CircleGeometry(0.22, 12), kit.dark);
  apu.rotation.y = Math.PI;
  apu.position.set(0, 1.88, M(68.65));
  group.add(apu);

  // Strongly swept 747-400 wing (37.5 deg quarter-chord / ~39.8 deg LE sweep),
  // 7 deg dihedral, huge inboard chord with Yehudi trailing-edge kink at the
  // inboard engine station (x = 11.9 m = 36.9% semi-span), 3.70 m tip chord,
  // and 6 ft (1.83 m) canted winglets bringing total span to 64.44 m.
  const wingY = -2.15;
  const wingSpan = 31.4; // + canted winglets = 64.44 m total span
  const kinkX = 11.9;    // Yehudi crank aligned with inboard engine pylon (39 ft)
  const wingDihedral = 0.122; // 7.0 deg
  const rise = Math.tan(wingDihedral);
  const rootLe = M(21.3);     // z = +13.0 m (interacts w/ fuselage side at ~24 m)
  const rootChord = 16.5;
  const kink = { le: rootLe - kinkX * 0.833, chord: 8.20, t: 0.118 };
  const tip = { le: rootLe - wingSpan * 0.833, chord: 3.70, t: 0.088 };
  for (const w of surfacePair(kit.wingSkin, {
    span: kinkX, dihedral: wingDihedral,
    root: { le: rootLe, chord: rootChord, y: wingY, t: 0.138 }, tip: kink,
  })) group.add(w);
  for (const w of surfacePair(kit.wingSkin, {
    span: wingSpan - kinkX, x0: kinkX, dihedral: wingDihedral,
    root: { ...kink, y: wingY + rise * kinkX }, tip,
  })) group.add(w);

  const wingYAt = (x) => wingY + rise * x;
  const rootTe = rootLe - rootChord;
  const kinkTe = kink.le - kink.chord;
  const tipTe = tip.le - tip.chord;
  const leAt = (x) => (x < kinkX
    ? rootLe + (kink.le - rootLe) * (x / kinkX)
    : kink.le + (tip.le - kink.le) * ((x - kinkX) / (wingSpan - kinkX)));
  const teAt = (x) => (x < kinkX
    ? rootTe + (kinkTe - rootTe) * (x / kinkX)
    : kinkTe + (tipTe - kinkTe) * ((x - kinkX) / (wingSpan - kinkX)));

  // Canted 747-400 winglets (~6 ft / 1.83 m vertical rise, ~24 deg outward cant).
  const tipY = wingYAt(wingSpan);
  for (const side of [1, -1]) {
    group.add(finMesh(kit.fin, {
      height: 2.05, thickness: 0.08, base: [side * wingSpan, tipY], cant: 0.42, side,
      root: { le: tip.le, chord: tip.chord }, tip: { le: tip.le - 2.35, chord: 1.35 },
    }));
  }

  // Wing-body belly fairing bridging the widebody keel and inboard wing roots.
  group.add(new THREE.Mesh(fuselage([
    [M(44.0), 0.35], [M(39.0), 1.65], [M(25.0), 1.85], [M(19.5), 1.15], [M(17.5), 0.0001],
  ], { sx: 2.05, sy: 0.62, warp: () => -2.55, segments: 24 }).geo, kit.wing));

  // Flap-track canoes under the wing trailing edge (4 per wing).
  for (const side of [1, -1]) {
    for (const x of [7.2, 14.5, 20.0, 25.0]) {
      const pod = new THREE.Mesh(fuselage([
        [-2.2, 0.0001], [-1.45, 0.22], [0.35, 0.28], [1.75, 0.0001],
      ], { sx: 0.58, segments: 10 }).geo, kit.wing);
      pod.position.set(side * x, wingYAt(x) - 0.22, teAt(x) + 0.9);
      group.add(pod);
    }
  }

  // Four high-bypass turbofans (CF6-80C2 / PW4056: ~2.7 m cowl OD) slung well
  // forward of and below the strongly swept wing LE on slender pylons.
  const engines = [
    { x: 11.9, drop: 1.78, fwd: 2.35, scale: 1.38, pylonH: 0.96 }, // inboard (39 ft)
    { x: 21.2, drop: 1.52, fwd: 1.85, scale: 1.36, pylonH: 0.80 }, // outboard (69.5 ft)
  ];
  for (const side of [1, -1]) {
    for (const e of engines) {
      const ey = wingYAt(e.x) - e.drop;
      const ez = leAt(e.x) + e.fwd;
      const engine = turbofan(kit);
      engine.scale.setScalar(e.scale);
      engine.position.set(side * e.x, ey, ez);
      group.add(engine);
      group.add(finMesh(kit.wing, {
        height: e.pylonH, thickness: 0.12, base: [side * e.x, wingYAt(e.x) + 0.05], cant: Math.PI,
        root: { le: leAt(e.x) + 0.4, chord: 5.4 },
        tip: { le: ez + 2.55, chord: 6.2 },
      }));
    }
  }

  // Swept horizontal stabilisers (22.17 m span, 37 deg quarter-chord sweep,
  // 7 deg dihedral) and tall swept vertical tail fin (19.41 m overall height
  // from runway ground plane at y = -5.45 m -> fin tip at y = +13.96 m).
  for (const t of surfacePair(kit.tailSkin, {
    span: 11.08, dihedral: 0.122,
    root: { le: M(58.1), chord: 7.4, y: 1.70, t: 0.105 },
    tip: { le: M(67.6), chord: 2.95, t: 0.085 },
  })) group.add(t);
  group.add(finMesh(kit.fin, {
    height: 11.11, thickness: 0.105, base: [0, 2.85],
    root: { le: M(52.8), chord: 12.2 },
    tip: { le: M(64.1), chord: 4.25 },
    logo: kit.logo, logoAt: 0.44, logoScale: 0.78,
  }));
  // Graceful dorsal fin fillet rising smoothly from the straight aft spine.
  group.add(finMesh(kit.fin, {
    height: 1.35, thickness: 0.11, base: [0, 3.25],
    root: { le: M(46.8), chord: 6.4 },
    tip: { le: M(52.8), chord: 1.6 },
  }));

  // Iconic 18-wheel retractable 747-400 landing gear (25.60 m wheelbase,
  // 11.00 m outer track, 1.24 m H49x19-22 main tyres, ground plane y = -5.45 m
  // giving 2.05 m belly clearance) with wheel-well doors that close behind it.
  const rig = gearRig();
  const mainWheelR = 0.62;
  const mainWheelY = -5.45 + mainWheelR; // -4.83 m -> bottom contact y = -5.45 m
  // `levelBogie` counter-rotates the truck so it stays flat as the leg swings.
  const addMainBogie = (x, z, strutTopY, braceX, doorSide, stow, levelBogie) => {
    const leg = rig.leg([x, strutTopY, z], [
      rod(kit.metal, [x, strutTopY, z], [x, mainWheelY, z], 0.20, 8),
    ], stow);
    const truck = [rod(kit.metal, [x, mainWheelY, z - 0.86], [x, mainWheelY, z + 0.86], 0.12, 6)];
    for (const dz of [-0.78, 0.78]) {
      truck.push(rod(kit.metal, [x - 0.46, mainWheelY, z + dz], [x + 0.46, mainWheelY, z + dz], 0.09, 6));
      for (const dx of [-0.46, 0.46]) {
        truck.push(wheel(kit, x + dx, mainWheelY, z + dz, mainWheelR, 0.44));
      }
    }
    rig.leg([x, mainWheelY, z], truck,
      levelBogie ? stow.clone().invert() : new THREE.Quaternion(), leg);
    rig.brace(kit.metal, [braceX, strutTopY - 0.25, z], [x, -3.75, z], 0.09, leg, 6);
    // Thin strut door hinged along its top edge, folding over the bay.
    const door = new THREE.Mesh(new THREE.BoxGeometry(0.08, 1.45, 2.0), kit.plain);
    door.position.set(x + doorSide * 0.42, strutTopY - 0.72, z);
    rig.door([x + doorSide * 0.42, strutTopY, z], door, foldAbout('z', -doorSide * Math.PI / 2));
  };
  for (const side of [1, -1]) {
    // Wing-gear bogies at exact 11.00 m track (x = ±5.50 m), folding inboard.
    addMainBogie(side * 5.50, M(33.9), wingYAt(5.50) - 0.15, side * 3.4, side,
      foldAbout('z', -side * Math.PI / 2), false);
    // Body-gear bogies tucked inboard and staggered 3.2 m aft (x = ±1.95 m),
    // folding forward.
    addMainBogie(side * 1.95, M(37.1), -2.85, side * 0.65, -side,
      foldAbout('x', -Math.PI / 2), true);
  }
  // Twin-wheel nose gear (25.60 m ahead of main gear midpoint -> m = 9.9 m).
  const noseGearZ = M(9.9);
  const noseWheelR = 0.50;
  const noseWheelY = -5.45 + noseWheelR; // -4.95 m -> bottom contact y = -5.45 m
  const noseLeg = rig.leg([0, -3.15, noseGearZ], [
    rod(kit.metal, [0, -3.15, noseGearZ], [0, noseWheelY, noseGearZ], 0.17, 8),
    rod(kit.metal, [-0.42, noseWheelY, noseGearZ], [0.42, noseWheelY, noseGearZ], 0.09, 6),
    wheel(kit, 0.42, noseWheelY, noseGearZ, noseWheelR, 0.36),
    wheel(kit, -0.42, noseWheelY, noseGearZ, noseWheelR, 0.36),
  ], foldAbout('x', -1.83));
  rig.brace(kit.metal, [0, -3.20, noseGearZ - 1.5], [0, -4.15, noseGearZ], 0.08, noseLeg, 6);
  for (const side of [1, -1]) {
    const nDoor = new THREE.Mesh(new THREE.BoxGeometry(0.08, 1.05, 2.2), kit.plain);
    nDoor.position.set(side * 0.62, -3.65, noseGearZ - 0.3);
    rig.door([side * 0.62, -3.125, noseGearZ - 0.3], nDoor, foldAbout('z', -side * Math.PI / 2));
  }
  group.add(rig.gear);

  // Wingtip nav lights, tailcone white light, upper-deck crown & belly beacons.
  group.add(dot(kit.red, wingSpan - 0.2, tipY + 0.16, tip.le - 0.5, 0.34));
  group.add(dot(kit.green, -(wingSpan - 0.2), tipY + 0.16, tip.le - 0.5, 0.34));
  group.add(dot(kit.white, 0, 1.94, M(68.75), 0.28));
  group.add(dot(kit.beacon, 0, 4.90, M(16.0), 0.42));
  group.add(dot(kit.beacon, 0, -3.46, M(20.0), 0.42));

  return finish(group, 0.345, kit, {
    refuelPort: new THREE.Vector3(0, 2.25, M(3.6)),
    landingLightPos: new THREE.Vector3(0, -1.2, M(-1.2)), // just ahead of nose radome (z=+34.3)
    landingGlowScale: 0.6,
  });
}

/* --------------------------------------------- fighter (sub-agent wingman) */

// Short-range missile on a wingtip rail, nose along +Z.
function missile(kit) {
  const g = new THREE.Group();
  const body = new THREE.Mesh(new THREE.CylinderGeometry(0.1, 0.1, 2.6, 10), kit.ordnance);
  body.rotation.x = Math.PI / 2;
  g.add(body);
  const seeker = new THREE.Mesh(new THREE.ConeGeometry(0.1, 0.35, 10), kit.nozzle);
  seeker.rotation.x = Math.PI / 2;
  seeker.position.z = 1.47;
  g.add(seeker);
  for (const [z, span, chord] of [[0.95, 0.2, 0.22], [-1.1, 0.26, 0.38]]) {
    for (const rot of [0, Math.PI / 2]) {
      const fin = new THREE.Mesh(new THREE.BoxGeometry(0.02, span * 2 + 0.2, chord), kit.ordnance);
      fin.rotation.z = rot + Math.PI / 4;
      fin.position.z = z;
      g.add(fin);
    }
  }
  return g;
}

// Twin-tail fighter: sub-agents flying wingman.
export function makeFighterMesh(model) {
  const group = new THREE.Group();
  const noseDroop = (z) => (z > 4 ? (z - 4) * -0.03 : 0);
  const body = fuselage([
    [-6.4, 0.85], [-5.6, 1.1], [-3, 1.3], [1, 1.3], [3.5, 1.05],
    [5.5, 0.72], [7.5, 0.4], [9.0, 0.12], [9.5, 0.0001],
  ], { sx: 1.35, sy: 0.72, warp: noseDroop });
  const kit = paintKit(model, 'fighter', {
    zmin: body.zmin, zmax: body.zmax, circ: body.circ,
    bellyA: 2.0, stripeA: 1.45, stripeW: 0.09, stripeZ1: 7.0, swooshZ: -2.2,
    titles: { a: 1.0, z: -1.6, size: 0.5 },
  });
  const bodyMesh = new THREE.Mesh(body.geo, kit.body);
  bodyMesh.name = 'fuselage';
  group.add(bodyMesh);
  const burnerMat = new THREE.MeshBasicMaterial({
    color: 0xff9d2e, transparent: true, opacity: 0.85, depthWrite: false,
  });
  kit.all.push(burnerMat);

  // Rounded side intakes with dark mouths and splitter plates.
  for (const side of [1, -1]) {
    const intake = new THREE.Mesh(fuselage([
      [-1.6, 0.0001], [-0.9, 0.3], [0.6, 0.42], [2.6, 0.44], [3.35, 0.44],
    ], { sx: 0.72, segments: 18 }).geo, kit.plain);
    intake.position.set(side * 1.48, -0.16, 0);
    group.add(intake);
    const mouth = new THREE.Mesh(new THREE.CircleGeometry(0.4, 18), kit.dark);
    mouth.scale.x = 0.72;
    mouth.position.set(side * 1.48, -0.16, 3.3);
    group.add(mouth);
    const splitter = new THREE.Mesh(new THREE.BoxGeometry(0.03, 0.7, 1.0), kit.metal);
    splitter.position.set(side * 1.12, -0.16, 3.0);
    group.add(splitter);
  }

  // Sleek dark-tinted fighter bubble canopy with thin swept windscreen bow
  // arch and slim canopy sill frame rails.
  const canopy = new THREE.Mesh(fuselage([
    [1.3, 0.0001], [1.9, 0.28], [2.9, 0.48], [3.9, 0.50], [4.9, 0.38], [5.85, 0.0001],
  ], { sx: 0.92, sy: 1.08, segments: 28 }).geo, kit.glass);
  canopy.position.y = 0.62;
  group.add(canopy);
  const bow = new THREE.Mesh(new THREE.TorusGeometry(0.47, 0.022, 8, 24, Math.PI), kit.frame);
  bow.scale.set(0.92, 1.08, 1);
  bow.rotation.x = -0.22; // gently raked windscreen frame arch
  bow.position.set(0, 0.63, 4.25);
  group.add(bow);
  for (const side of [1, -1]) {
    group.add(rod(kit.frame, [side * 0.41, 0.68, 2.2], [side * 0.34, 0.64, 5.1], 0.024, 6));
  }
  // Nose pitot probe.
  group.add(rod(kit.metal, [0, noseDroop(9.3), 9.3], [0, noseDroop(9.3), 10.5], 0.025));

  for (const w of surfacePair(kit.wingPlainSkin, {
    span: 6.6, dihedral: -0.04,
    root: { le: 2.8, chord: 7.6, y: 0, t: 0.055 }, tip: { le: -2.6, chord: 1.5, t: 0.04 },
  })) group.add(w);
  for (const w of surfacePair(kit.plain, {
    span: 1.6, thickness: 0.04,
    root: { le: 5.4, chord: 3.0, y: 0.05 }, tip: { le: 2.2, chord: 0.5 },
  })) group.add(w);
  for (const t of surfacePair(kit.tailPlainSkin, {
    span: 3.6, dihedral: -0.03, thickness: 0.05,
    root: { le: -3.8, chord: 2.7, y: -0.05 }, tip: { le: -5.6, chord: 1.0 },
  })) group.add(t);

  // Wingtip missiles on launch rails.
  for (const side of [1, -1]) {
    const m = missile(kit);
    m.position.set(side * 6.85, -0.32, -2.9);
    group.add(m);
    group.add(rod(kit.metal, [side * 6.6, -0.24, -2.4], [side * 6.6, -0.24, -3.9], 0.06));
  }

  for (const side of [1, -1]) {
    group.add(finMesh(kit.fin, {
      height: 3.0, thickness: 0.06, base: [side * 1.0, 0.45], cant: 0.4, side,
      root: { le: -2.4, chord: 3.4 }, tip: { le: -4.8, chord: 1.2 },
      logo: kit.logo, logoAt: 0.4, logoScale: 0.62,
    }));
  }

  // Faceted convergent nozzles with dark throats.
  for (const side of [1, -1]) {
    const nozzle = new THREE.Mesh(fuselage([
      [-7.15, 0.44], [-6.7, 0.48], [-6.2, 0.53], [-5.9, 0.54],
    ], { segments: 12, smooth: false }).geo, kit.nozzle);
    nozzle.position.set(side * 0.62, -0.05, 0);
    group.add(nozzle);
    const throat = new THREE.Mesh(new THREE.CircleGeometry(0.42, 12), kit.dark);
    throat.rotation.y = Math.PI;
    throat.position.set(side * 0.62, -0.05, -6.95);
    group.add(throat);
    const flame = new THREE.Mesh(new THREE.ConeGeometry(0.38, 2.4, 12), burnerMat);
    flame.rotation.x = -Math.PI / 2;
    flame.position.set(side * 0.62, -0.05, -8.2);
    flame.name = 'afterburner';
    group.add(flame);
  }

  // Main legs fold forward and inboard under the intakes, nose leg aft.
  const rig = gearRig();
  for (const side of [1, -1]) {
    const pivot = [side * 1.3, -0.4, -1.0];
    rig.leg(pivot, [
      rod(kit.metal, pivot, [side * 1.5, -1.9, -1.0], 0.09),
      wheel(kit, side * 1.5, -2.0, -1.0, 0.38, 0.26),
    ], foldToward(pivot, [side * 1.5, -2.0, -1.0], [-side * 0.47, 0, 0.88]));
  }
  rig.leg([0, -0.5, 4.6], [
    rod(kit.metal, [0, -0.5, 4.6], [0, -1.9, 4.8], 0.08),
    wheel(kit, 0, -2.05, 4.8, 0.28, 0.2),
  ], foldAbout('x', 1.75));
  group.add(rig.gear);

  navLights(group, kit, [6.5, 0.05, -3.3], [0, 0.0, -6.5]);
  group.add(dot(kit.beacon, 0, 0.93, -1.2, 0.18));

  return finish(group, 0.72, kit, {
    refuelPort: new THREE.Vector3(0, 0.92, 5.2),
    landingLightPos: new THREE.Vector3(0, -0.35, 9.0),
  });
}

/* ------------------------------------------- drone (monitoring / jobs) */

// Long-endurance pusher drone: monitoring / scheduled jobs.
export function makeDroneMesh(model) {
  const group = new THREE.Group();
  const body = fuselage([
    [-7.0, 0.45], [-5.5, 0.72], [-2, 0.95], [2, 1.05], [4.5, 1.25],
    [6.4, 1.2], [7.6, 0.85], [8.3, 0.35], [8.5, 0.0001],
  ], { sx: 0.95, warp: (z) => (z > 2 ? Math.min(z - 2, 3) * 0.1 : 0) });
  const kit = paintKit(model, 'drone', {
    zmin: body.zmin, zmax: body.zmax, circ: body.circ,
    bellyA: 2.1, stripeA: 1.6, stripeW: 0.1, stripeZ1: 7.6, swooshZ: -3.8,
    titles: { a: 1.15, z: 3.4, size: 0.62 },
  });
  const bodyMesh = new THREE.Mesh(body.geo, kit.body);
  bodyMesh.name = 'fuselage';
  group.add(bodyMesh);

  // EO/IR sensor gimbal turret under the nose (matte housing + dark optical
  // lens aperture), plus SATCOM and data-link antennas on the spine.
  const mount = new THREE.Mesh(new THREE.CylinderGeometry(0.24, 0.30, 0.42, 12), kit.dark);
  mount.position.set(0, -0.98, 5.7);
  group.add(mount);
  group.add(dot(kit.dark, 0, -1.32, 5.7, 0.42));
  const lens = new THREE.Mesh(new THREE.CircleGeometry(0.18, 14), kit.glass);
  lens.position.set(0, -1.35, 6.11);
  group.add(lens);
  group.add(rod(kit.dark, [0, 0.95, -1.5], [0, 1.45, -1.9], 0.04));
  group.add(rod(kit.dark, [0, -0.9, -2.5], [0, -1.35, -2.8], 0.04));

  for (const w of surfacePair(kit.wingPlainSkin, {
    span: 12.5, dihedral: 0.025,
    root: { le: 1.8, chord: 2.3, y: 0.35, t: 0.15 }, tip: { le: 1.0, chord: 0.95, t: 0.12 },
  })) group.add(w);

  for (const side of [1, -1]) {
    group.add(finMesh(kit.fin, {
      height: 3.2, thickness: 0.1, base: [side * 0.25, 0.3], cant: 0.75, side,
      root: { le: -4.6, chord: 2.0 }, tip: { le: -5.9, chord: 0.9 },
      logo: kit.logo, logoAt: 0.38, logoScale: 0.75,
    }));
  }
  group.add(finMesh(kit.fin, {
    height: 1.8, thickness: 0.1, base: [0, -0.4], cant: Math.PI, side: 1,
    root: { le: -4.8, chord: 1.6 }, tip: { le: -5.8, chord: 0.8 },
  }));

  const hub = new THREE.Mesh(new THREE.ConeGeometry(0.4, 0.9, 14), kit.engine);
  hub.rotation.x = -Math.PI / 2;
  hub.position.z = -7.4;
  group.add(hub);
  const prop = propeller(kit, { blades: 3, radius: 1.85, rootChord: 0.32, tipChord: 0.16, hub: 0.25 });
  prop.position.z = -7.25;
  group.add(prop);

  // Splayed main legs swing in and aft along the belly, nose leg aft.
  const rig = gearRig();
  for (const side of [1, -1]) {
    const pivot = [side * 0.5, -0.6, 0.6];
    rig.leg(pivot, [
      rod(kit.metal, pivot, [side * 2.0, -2.1, 0.6], 0.08),
      wheel(kit, side * 2.05, -2.2, 0.6, 0.36, 0.22),
    ], foldToward(pivot, [side * 2.05, -2.2, 0.6], [-side * 0.1, 0.05, -1]));
  }
  rig.leg([0, -0.9, 4.4], [
    rod(kit.metal, [0, -0.9, 4.4], [0, -2.15, 4.4], 0.07),
    wheel(kit, 0, -2.25, 4.4, 0.28, 0.18),
  ], foldAbout('x', 1.83));
  group.add(rig.gear);

  navLights(group, kit, [12.4, 0.75, 0.6], [0, 0.0, -6.9]);
  group.add(dot(kit.beacon, 0, 1.4, 2.0, 0.2));

  return finish(group, 0.76, kit, {
    refuelPort: new THREE.Vector3(0, 1.25, 5.0),
    landingLightPos: new THREE.Vector3(0, 0.08, 8.25),
  });
}

/* ------------------------------------ aerial refuelling tanker (provider) */

// Heavy 4-engine widebody aerial refuelling tanker (A400M / A330-MRTT class)
// in a matte grey military paint scheme with the AI lab's tail livery, an aft
// flying boom, and twin underwing hose-and-drogue pods.
export function makeTankerMesh(model) {
  const group = new THREE.Group();
  const warp = (z) => (z < -7.2 ? (-7.2 - z) * 0.14 : 0) - (z > 12.2 ? (z - 12.2) * 0.1 : 0);
  const body = fuselage([
    [-16.4, 0.16], [-15.0, 0.72], [-12.2, 1.48], [-8.4, 2.08], [-5.0, 2.22],
    [10.2, 2.22], [12.6, 2.04], [14.4, 1.6], [15.8, 1.0], [16.6, 0.44], [17.0, 0.0001],
  ], { sy: 1.06, warp, segments: 44 });
  const kit = paintKit(model, 'tanker', {
    zmin: body.zmin, zmax: body.zmax, circ: body.circ,
    bodyColor: TANKER_GREY.body, bellyColor: TANKER_GREY.belly, bellyA: 1.98,
    radomeZ: 15.98, radomeColor: TANKER_GREY.radome,
    antiGlare: { z0: 15.0, z1: 15.90, a: 0.25, color: TANKER_GREY.antiGlare },
    // Small crew observation ports only (no civil passenger window row).
    windows: { kind: 'row', a: 1.16, z0: 10.6, z1: 12.2, pitch: 0.8, w: 0.22, h: 0.26 },
    military: {
      roundelZ: -5.8, roundelA: 1.12, roundelR: 0.94,
      stencilZ: 2.6, stencilA: 1.06, stencilSize: 0.46,
      forwardStencilZ: 8.6, forwardStencilA: 1.24, forwardStencilSize: 0.26,
    },
  });
  const bodyMesh = new THREE.Mesh(body.geo, kit.body);
  bodyMesh.name = 'fuselage';
  group.add(bodyMesh);

  // Realistic 6-pane angular military transport flight-deck glazing
  // (A330 MRTT / A400M class) in dark tinted glass with thin grey frames.
  group.add(cockpitGlazingMesh(body, kit.windowGlass, [
    { knots: [[0.045, 0.56, 1.18], [0.52, 0.57, 1.16], [1.02, 0.59, 1.14]], subdivS: 3, subdivY: 2 },
    { knots: [[1.10, 0.59, 1.13], [1.94, 0.63, 1.08]], subdivS: 3, subdivY: 2 },
    { knots: [[2.02, 0.64, 1.08], [2.78, 0.80, 0.98]], subdivS: 3, subdivY: 2 },
  ], {
    zSearchStart: 16.92,
    zSearchEnd: 11.8,
    offset: 0.028,
    flatBlend: 0.8,
  }));

  // Dorsal SATCOM & fuel-control fairing hump along the upper spine.
  const satcom = new THREE.Mesh(fuselage([
    [-3.2, 0.0001], [-1.8, 0.46], [1.4, 0.56], [4.2, 0.42], [5.8, 0.0001],
  ], { sx: 1.35, sy: 0.75, segments: 18 }).geo, kit.plain);
  satcom.position.set(0, 2.18, 1.2);
  group.add(satcom);

  // APU exhaust in the tail cone.
  const apu = new THREE.Mesh(new THREE.CircleGeometry(0.16, 12), kit.dark);
  apu.rotation.y = Math.PI;
  apu.position.set(0, warp(-16.4), -16.42);
  group.add(apu);

  // Broad swept wings with inboard crank, belly wing-box fairing, and branded winglets.
  const wingY = -1.1;
  const wingSpan = 17.5;
  const kinkX = 6.2;
  const wingDihedral = 0.08;
  const rise = Math.tan(wingDihedral);
  const kink = { le: 1.0, chord: 4.8, t: 0.125 };
  const tip = { le: -4.8, chord: 1.75, t: 0.1 };
  for (const w of surfacePair(kit.wingSkin, {
    span: kinkX, dihedral: wingDihedral,
    root: { le: 4.4, chord: 8.4, y: wingY, t: 0.15 }, tip: kink,
  })) group.add(w);
  for (const w of surfacePair(kit.wingSkin, {
    span: wingSpan - kinkX, x0: kinkX, dihedral: wingDihedral,
    root: { ...kink, y: wingY + rise * kinkX }, tip,
  })) group.add(w);

  const wingYAt = (x) => wingY + rise * x;
  const kinkTe = kink.le - kink.chord;
  const tipTe = tip.le - tip.chord;
  const leAt = (x) => (x < kinkX
    ? 4.4 + (kink.le - 4.4) * (x / kinkX)
    : kink.le + (tip.le - kink.le) * ((x - kinkX) / (wingSpan - kinkX)));
  const teAt = (x) => (x < kinkX
    ? -4.0 + (kinkTe + 4.0) * (x / kinkX)
    : kinkTe + (tipTe - kinkTe) * ((x - kinkX) / (wingSpan - kinkX)));

  // Winglets in matte military grey (tail fin carries the AI lab livery).
  const tipY = wingYAt(wingSpan);
  for (const side of [1, -1]) {
    group.add(finMesh(kit.wing, {
      height: 1.9, thickness: 0.08, base: [side * wingSpan, tipY], cant: 0.32, side,
      root: { le: tip.le, chord: tip.chord }, tip: { le: -6.1, chord: 0.72 },
    }));
  }

  // Belly wing-box fairing.
  group.add(new THREE.Mesh(fuselage([
    [-4.8, 0.28], [-3.2, 1.15], [2.0, 1.25], [4.6, 0.68], [5.2, 0.0001],
  ], { sx: 1.8, sy: 0.65, warp: () => -1.35 }).geo, kit.wing));

  // Four underwing turbofan engines (inboard + outboard on each wing).
  const engineXs = [5.6, 10.2];
  for (const side of [1, -1]) {
    for (const ex of engineXs) {
      const ey = wingYAt(ex) - 1.32;
      const ez = leAt(ex) + 0.55;
      const engine = turbofan(kit);
      engine.scale.setScalar(0.92);
      engine.position.set(side * ex, ey, ez);
      group.add(engine);
      group.add(finMesh(kit.wing, {
        height: 0.65, thickness: 0.16, base: [side * ex, wingYAt(ex) + 0.05], cant: Math.PI,
        root: { le: leAt(ex) + 0.1, chord: 2.5 }, tip: { le: ez + 1.5, chord: 3.3 },
      }));
    }
  }

  // Twin underwing hose-and-drogue refuelling pods outboard of the outer engines.
  const podX = 13.4;
  const podY = wingYAt(podX) - 0.85;
  const podZ = (leAt(podX) + teAt(podX)) * 0.5 - 0.5;
  const amberMat = new THREE.MeshBasicMaterial({ color: 0xffb020 });
  kit.all.push(amberMat);
  for (const side of [1, -1]) {
    const pod = new THREE.Mesh(fuselage([
      [-2.3, 0.36], [-1.5, 0.48], [0.6, 0.5], [1.9, 0.32], [2.4, 0.0001],
    ], { segments: 16 }).geo, kit.engine);
    pod.position.set(side * podX, podY, podZ);
    group.add(pod);
    // Strut attaching pod to wing underside.
    group.add(rod(kit.wing, [side * podX, wingYAt(podX), podZ + 0.3], [side * podX, podY + 0.35, podZ + 0.2], 0.16));
    // Dark drogue housing rim at the rear of the pod.
    const rim = new THREE.Mesh(new THREE.RingGeometry(0.18, 0.38, 16), kit.dark);
    rim.rotation.y = Math.PI;
    rim.position.set(side * podX, podY, podZ - 2.32);
    group.add(rim);
    // Amber status light on pod belly.
    group.add(dot(amberMat, side * podX, podY - 0.5, podZ - 0.6, 0.18));
  }

  // Swept horizontal stabilisers and tall branded tail fin with provider logo.
  const tailY = warp(-13.2);
  for (const t of surfacePair(kit.tailSkin, {
    span: 6.4, dihedral: 0.1,
    root: { le: -10.8, chord: 3.8, y: tailY, t: 0.11 }, tip: { le: -14.2, chord: 1.55, t: 0.09 },
  })) group.add(t);
  group.add(finMesh(kit.fin, {
    height: 6.4, thickness: 0.11, base: [0, tailY + 1.15],
    root: { le: -8.6, chord: 5.8 }, tip: { le: -13.6, chord: 2.3 },
    logo: kit.logo, logoAt: 0.44, logoScale: 0.78,
  }));
  group.add(finMesh(kit.fin, {
    height: 0.75, thickness: 0.12, base: [0, 2.0],
    root: { le: -5.8, chord: 3.2 }, tip: { le: -8.6, chord: 1.0 },
  }));

  // Iconic aft flying refuelling boom with V-shaped ruddervators and high-vis tip.
  const boomRoot = [0, -0.7, -11.2];
  const boomMid = [0, -2.5, -16.8];
  const boomTip = [0, -3.4, -19.6];
  group.add(rod(kit.metal, boomRoot, boomMid, 0.24, 10));
  group.add(rod(amberMat, boomMid, boomTip, 0.15, 10));
  group.add(dot(amberMat, boomTip[0], boomTip[1], boomTip[2], 0.24));
  for (const side of [1, -1]) {
    group.add(finMesh(kit.engine, {
      height: 1.55, thickness: 0.08, base: [side * 0.18, boomMid[1] + 0.08], cant: 0.82, side,
      root: { le: boomMid[2] + 0.55, chord: 1.1 }, tip: { le: boomMid[2] - 0.15, chord: 0.55 },
    }));
  }

  // Nav lights, belly refuelling director lights, and spine/belly beacons.
  navLights(group, kit, [wingSpan - 0.2, tipY + 0.12, -5.1], [0, tailY + 0.28, -16.5]);
  group.add(dot(kit.beacon, 0, 2.55, 1.4, 0.32));
  group.add(dot(kit.beacon, 0, -2.3, 6.2, 0.3));
  group.add(dot(amberMat, 0, -2.3, 9.6, 0.22));

  // Deployable tricycle gear keeps the boom clear during landing rotation.
  // Long main legs fold aft and inboard (straight inboard they would cross),
  // nose leg aft.
  const rig = gearRig();
  for (const side of [1, -1]) {
    const pivot = [side * 2.4, -1.6, -2.0];
    rig.leg(pivot, [
      rod(kit.metal, pivot, [side * 2.4, -5.2, -2.0], 0.16),
      wheel(kit, side * 2.4, -5.2, -2.0, 0.65, 0.38),
    ], foldToward(pivot, [side * 2.4, -5.2, -2.0], [-side * 0.42, 0.08, -0.9]));
  }
  rig.leg([0, -1.6, 10.0], [
    rod(kit.metal, [0, -1.6, 10.0], [0, -5.35, 10.0], 0.14),
    wheel(kit, 0, -5.35, 10.0, 0.5, 0.3),
  ], foldAbout('x', 1.6));
  group.add(rig.gear);

  return finish(group, 0.85, kit, {
    refuelAnchors: {
      center: new THREE.Vector3(...boomTip),
      port: new THREE.Vector3(podX, podY, podZ - 2.35),
      starboard: new THREE.Vector3(-podX, podY, podZ - 2.35),
    },
    landingLightPos: new THREE.Vector3(0, -0.7, 17.9), // just ahead of the nose tip (z=17.0)
    landingGlowScale: 0.6,
  });
}
