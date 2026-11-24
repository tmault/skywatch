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

// Body of revolution along Z. `profile` is [[z, radius], ...] from tail to nose.
// `sx`/`sy` squash the cross-section; `warp(z)` returns a vertical offset so
// tails can sweep up and noses can droop. Also returns the paint layout basics.
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
  return { geo, zmin, zmax, circ: 2 * Math.PI * rmax * (sx + sy) / 2 };
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
  for (let i = 0; i <= n; i++) {
    const u = upper.getPoint(i / n);
    const l = lower.getPoint(i / n);
    const w = Math.max(u.y, 0.001);
    const yc = (u.z + l.y) / 2;
    const h = Math.max((u.z - l.y) / 2, 0.001);
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
  return { geo, zmin, zmax, circ: 2 * Math.PI * widest / Math.SQRT2 };
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
    glass: new THREE.MeshStandardMaterial({ color: 0x1d2c3d, roughness: 0.06, metalness: 0.35 }),
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
    windows: {
      kind: 'highwing', top: 0.5, sill: 1.3, post: 0.06,
      front: Z(1.42), pillarTop: Z(2.12), pillarBottom: Z(1.72),
      doorEnd: Z(2.95), rearEnd: Z(3.85), rearTaper: 0.28,
      skylight: [Z(3.88), Z(4.35), 0.42],
    },
    titles: { a: 1.25, z: Z(5.25), size: 0.36 },
    nose: { a: 1.3, z: Z(0.95), size: 0.13 },
  });
  const bodyMesh = new THREE.Mesh(body.geo, kit.body);
  bodyMesh.name = 'fuselage';
  g.add(bodyMesh);

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
  ], { sy: 1.05, warp, segments: 32 });
  const kit = paintKit(model, 'jet', {
    zmin: body.zmin, zmax: body.zmax, circ: body.circ,
    bellyA: 2.05, stripeA: 1.5, stripeW: 0.1, stripeZ1: 11.2, swooshZ: -5.0,
    windows: { kind: 'row', a: 1.16, z0: -5.6, z1: 8.4, pitch: 0.62, w: 0.24, h: 0.32 },
    cockpit: { z0: 10.35, z1: 11.0, a: 0.78 },
    titles: { a: 0.6, z: 2.2, size: 1.05 },
    operator: { a: 0.72, z: 7.7, size: 0.34 },
    nose: { a: 1.3, z: 9.9, size: 0.34 },
  });
  const bodyMesh = new THREE.Mesh(body.geo, kit.body);
  bodyMesh.name = 'fuselage';
  group.add(bodyMesh);
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

  // Retractable gear (hidden in flight by main.js): twin-wheel main legs
  // with side braces, twin-wheel nose leg.
  const gear = new THREE.Group();
  gear.name = 'gear';
  for (const side of [1, -1]) {
    gear.add(rod(kit.metal, [side * 2.2, wingY - 0.2, -1.2], [side * 2.2, -3.3, -1.2], 0.14));
    gear.add(rod(kit.metal, [side * 1.2, -1.4, -1.2], [side * 2.2, -2.5, -1.2], 0.06));
    gear.add(rod(kit.metal, [side * 1.9, -3.4, -1.2], [side * 2.5, -3.4, -1.2], 0.09));
    for (const dx of [1.92, 2.48]) gear.add(wheel(kit, side * dx, -3.4, -1.2, 0.58, 0.34));
  }
  gear.add(rod(kit.metal, [0, -1.4, 9.2], [0, -3.4, 9.2], 0.12));
  gear.add(rod(kit.metal, [0, -1.6, 8.3], [0, -2.6, 9.2], 0.05));
  gear.add(wheel(kit, 0.22, -3.5, 9.2, 0.38, 0.25));
  gear.add(wheel(kit, -0.22, -3.5, 9.2, 0.38, 0.25));
  group.add(gear);

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
  const h = Math.max(k - Math.abs(a - b), 0) / k;
  return Math.max(a, b) + h * h * k * 0.25;
}

// Single seamless double-bubble Boeing 747 fuselage: circular widebody main
// deck smoothly fused via polynomial smooth-max with the iconic raised
// stretched upper-deck hump, raked flight-deck forehead, protruding lower nose
// cone, and upswept aft belly. Same UV layout as fuselage().
function jumboFuselage(profile, humpProfile, { sy = 1.04, warp = null, segments = 48 } = {}) {
  const mainCurve = new THREE.CatmullRomCurve3(
    profile.map(([z, r]) => new THREE.Vector3(r, z, 0)), false, 'centripetal',
  );
  const humpCurve = new THREE.CatmullRomCurve3(
    humpProfile.map(([z, s]) => new THREE.Vector3(z, s, 0)), false, 'catmullrom', 0.5,
  );
  const zmin = profile[0][0];
  const zmax = profile[profile.length - 1][0];
  const N = 96;
  const S = segments;

  // Sample both curves on a uniform Z grid (with extra density across nose & hump).
  const mainSamples = mainCurve.getPoints(240);
  const humpSamples = humpCurve.getPoints(160);
  const sampleAtZ = (pts, z, getZ, getV) => {
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
  };

  // Ray distance from (0, 0) along (dx, dy) to ellipse centered at (0, cy) with radii (a, b).
  const rayEllipse = (dx, dy, cy, a, b) => {
    const A = (dx * dx) / (a * a) + (dy * dy) / (b * b);
    const B = (-2 * cy * dy) / (b * b);
    const C = (cy * cy) / (b * b) - 1;
    const disc = B * B - 4 * A * C;
    if (disc < 0) return 0;
    return (-B + Math.sqrt(disc)) / (2 * A);
  };

  const pos = [];
  const uv = [];
  const idx = [];
  for (let i = 0; i <= N; i++) {
    const t = i / N;
    // Slight cosine clustering near tail & nose for silky nose cone curvature.
    const z = zmin + (zmax - zmin) * (0.5 - 0.5 * Math.cos(Math.PI * t));
    const r = Math.max(sampleAtZ(mainSamples, z, (p) => p.y, (p) => p.x), 0.0001);
    const s = Math.max(sampleAtZ(humpSamples, z, (p) => p.x, (p) => p.y), 0);
    const wy = warp ? warp(z) : 0;
    const a1 = r;
    const b1 = r * sy;
    // Upper-deck ellipse scales smoothly with hump envelope s ∈ [0..1].
    // Center sits at cy = 1.56 * s with b2 = 2.08 * s (top at +3.64 * s, bottom
    // at -0.52 * s < 0 so the origin is always strictly inside the ellipse).
    const useHump = s > 0.02 && r > 0.3;
    const cy2 = 1.56 * s;
    const a2 = Math.max(1.58 * s, 0.01);
    const b2 = Math.max(2.08 * s, 0.01);
    const blendK = 0.26 * Math.min(1, s * 1.4);

    for (let j = 0; j <= S; j++) {
      // phi = 0 is belly, PI/2 is +X port, PI is top spine (matches LatheGeometry).
      const phi = (j / S) * Math.PI * 2;
      const dx = Math.sin(phi);
      const dy = -Math.cos(phi);
      let rad = rayEllipse(dx, dy, 0, a1, b1);
      if (useHump && dy > -0.15) {
        const r2 = rayEllipse(dx, dy, cy2, a2, b2);
        rad = smax(rad, r2, blendK);
      }
      pos.push(dx * rad, wy + dy * rad, z);
      uv.push(j / S, (z - zmin) / (zmax - zmin));
    }
  }
  for (let i = 0; i < N; i++) {
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
  const rmax = Math.max(...profile.map(([, r]) => r));
  return { geo, zmin, zmax, circ: 2 * Math.PI * rmax * (1 + sy) / 2 };
}

// Quad-engine widebody Boeing 747 ("Queen of the Sky"): reserved exclusively
// for top-tier flagship models (see flagship.js). Features a single seamless
// double-bubble fuselage with the iconic raised stretched upper-deck hump and
// high 6-pane flight deck, strongly swept wings with four underwing turbofans
// on pylons, canted 747-400 winglets, tall swept vertical tail, and the
// signature 18-wheel (4x4 main bogies + twin nose) landing gear.
export function makeJumboMesh(model) {
  const group = new THREE.Group();
  const warp = (z) => (z < -5.8 ? (-5.8 - z) * 0.145 : 0) - (z > 10.6 ? (z - 10.6) * 0.085 : 0);
  const body = jumboFuselage([
    [-16.0, 0.18], [-14.8, 0.72], [-12.2, 1.44], [-8.8, 2.04], [-5.8, 2.26],
    [10.2, 2.26], [12.0, 2.12], [13.6, 1.76], [14.9, 1.24], [15.8, 0.66],
    [16.25, 0.28], [16.45, 0.0001],
  ], [
    // Upper-deck hump envelope s(z): 0 aft of wing box, rising smoothly into
    // the long 747-400 upper deck, then sloping steeply down across the
    // forward-facing cockpit forehead into the lower nose cone.
    [-16.0, 0.0], [-2.8, 0.0], [-1.4, 0.50], [0.2, 0.74], [1.8, 0.90],
    [3.8, 0.99], [6.8, 1.0], [9.8, 0.99], [11.1, 0.92], [12.0, 0.77],
    [12.8, 0.58], [13.5, 0.32], [14.2, 0.0], [16.45, 0.0],
  ], { sy: 1.04, warp, segments: 52 });

  const kit = paintKit(model, 'jumbo', {
    zmin: body.zmin, zmax: body.zmax, circ: body.circ,
    texW: 1024, texH: 1024,
    bellyA: 2.02, stripeA: 1.56, stripeW: 0.11, stripeZ1: 14.4, swooshZ: -7.2,
    // Iconic 6-pane flight deck high on the forward-facing brow of the hump.
    cockpit: { z0: 11.45, z1: 12.65, a: 0.50, frames: [-0.54, -0.27, 0, 0.27, 0.54] },
    // Both upper-deck cabin windows AND full-length main-deck windows extending
    // all the way forward beneath the cockpit into the nose.
    windows: [
      { kind: 'row', a: 1.26, z0: -9.8, z1: 12.8, pitch: 0.56, w: 0.21, h: 0.27 },
      { kind: 'row', a: 0.55, z0: 3.4, z1: 10.2, pitch: 0.56, w: 0.22, h: 0.16 },
    ],
    doors: { a: 1.32, zs: [-9.2, -3.6, 2.4, 8.2, 12.9], w: 0.52, h: 1.02 },
    titles: { a: 0.74, z: -2.4, size: 1.36 },
    operator: { a: 0.84, z: 2.2, size: 0.35 },
    nose: { a: 1.02, z: 11.2, size: 0.34 },
  });
  const bodyMesh = new THREE.Mesh(body.geo, kit.body);
  bodyMesh.name = 'fuselage';
  group.add(bodyMesh);

  // APU exhaust in the tail cone.
  const apu = new THREE.Mesh(new THREE.CircleGeometry(0.16, 12), kit.dark);
  apu.rotation.y = Math.PI;
  apu.position.set(0, warp(-16.0), -16.02);
  group.add(apu);

  // Strongly swept 747 wing (~37 deg) with broad inboard Yehudi crank,
  // pronounced dihedral, wing-body belly fairing, and canted 747-400 winglets.
  const wingY = -1.25;
  const wingSpan = 14.9;
  const kinkX = 5.3;
  const wingDihedral = 0.095;
  const rise = Math.tan(wingDihedral);
  const kink = { le: 0.6, chord: 4.7, t: 0.12 };
  const tip = { le: -6.2, chord: 1.52, t: 0.095 };
  for (const w of surfacePair(kit.wingSkin, {
    span: kinkX, dihedral: wingDihedral,
    root: { le: 4.8, chord: 8.8, y: wingY, t: 0.145 }, tip: kink,
  })) group.add(w);
  for (const w of surfacePair(kit.wingSkin, {
    span: wingSpan - kinkX, x0: kinkX, dihedral: wingDihedral,
    root: { ...kink, y: wingY + rise * kinkX }, tip,
  })) group.add(w);

  const wingYAt = (x) => wingY + rise * x;
  const kinkTe = kink.le - kink.chord;
  const tipTe = tip.le - tip.chord;
  const leAt = (x) => (x < kinkX
    ? 4.8 + (kink.le - 4.8) * (x / kinkX)
    : kink.le + (tip.le - kink.le) * ((x - kinkX) / (wingSpan - kinkX)));
  const teAt = (x) => (x < kinkX
    ? -4.0 + (kinkTe + 4.0) * (x / kinkX)
    : kinkTe + (tipTe - kinkTe) * ((x - kinkX) / (wingSpan - kinkX)));

  // Canted 747-400 winglets in the provider's tail livery.
  const tipY = wingYAt(wingSpan);
  for (const side of [1, -1]) {
    group.add(finMesh(kit.fin, {
      height: 1.9, thickness: 0.08, base: [side * wingSpan, tipY], cant: 0.36, side,
      root: { le: tip.le, chord: tip.chord }, tip: { le: -7.5, chord: 0.62 },
    }));
  }

  // Prominent wing-body belly fairing bridging fuselage and wing roots.
  group.add(new THREE.Mesh(fuselage([
    [-5.2, 0.28], [-3.6, 1.18], [2.2, 1.28], [4.8, 0.72], [5.8, 0.0001],
  ], { sx: 1.9, sy: 0.72, warp: () => -1.42 }).geo, kit.wing));

  // Flap-track fairings under the trailing edge (4 per wing).
  for (const side of [1, -1]) {
    for (const x of [3.1, 6.6, 9.5, 12.1]) {
      const pod = new THREE.Mesh(fuselage([
        [-1.4, 0.0001], [-0.95, 0.14], [0.22, 0.18], [1.15, 0.0001],
      ], { sx: 0.6, segments: 10 }).geo, kit.wing);
      pod.position.set(side * x, wingYAt(x) - 0.12, teAt(x) + 0.55);
      group.add(pod);
    }
  }

  // Four high-bypass turbofan engines slung well forward on swept pylons.
  const engines = [
    { x: 5.1, drop: 1.22, fwd: 0.95, scale: 0.94, pylonH: 0.62 },
    { x: 9.3, drop: 1.06, fwd: 0.80, scale: 0.88, pylonH: 0.54 },
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
        height: e.pylonH, thickness: 0.16, base: [side * e.x, wingYAt(e.x) + 0.04], cant: Math.PI,
        root: { le: leAt(e.x) + 0.15, chord: 2.7 }, tip: { le: ez + 1.55, chord: 3.5 },
      }));
    }
  }

  // Swept horizontal stabilisers and tall, majestic 747 vertical tail fin.
  const tailY = warp(-12.6);
  for (const t of surfacePair(kit.tailSkin, {
    span: 6.0, dihedral: 0.11,
    root: { le: -10.2, chord: 3.9, y: tailY, t: 0.11 }, tip: { le: -14.0, chord: 1.45, t: 0.09 },
  })) group.add(t);
  group.add(finMesh(kit.fin, {
    height: 6.9, thickness: 0.11, base: [0, tailY + 1.15],
    root: { le: -7.8, chord: 6.2 }, tip: { le: -13.6, chord: 2.35 },
    logo: kit.logo, logoAt: 0.44, logoScale: 0.76,
  }));
  group.add(finMesh(kit.fin, {
    height: 0.85, thickness: 0.12, base: [0, 2.08],
    root: { le: -5.2, chord: 3.2 }, tip: { le: -7.8, chord: 1.0 },
  }));

  // Iconic 18-wheel retractable 747 landing gear:
  // 4 four-wheel main bogies (2 wing gear + 2 aft body gear) + twin-wheel nose gear.
  const gear = new THREE.Group();
  gear.name = 'gear';
  const mainWheelY = -3.70;
  const mainWheelR = 0.56; // bottom contact plane at y = -4.26
  const addMainBogie = (x, z, strutTopY, braceX) => {
    gear.add(rod(kit.metal, [x, strutTopY, z], [x, mainWheelY, z], 0.15, 8));
    gear.add(rod(kit.metal, [braceX, -1.65, z], [x, -2.75, z], 0.07, 6));
    // Longitudinal bogie beam carrying forward and aft dual-wheel pairs.
    gear.add(rod(kit.metal, [x, mainWheelY, z - 0.62], [x, mainWheelY, z + 0.62], 0.09, 6));
    for (const dz of [-0.56, 0.56]) {
      gear.add(rod(kit.metal, [x - 0.34, mainWheelY, z + dz], [x + 0.34, mainWheelY, z + dz], 0.07, 6));
      for (const dx of [-0.34, 0.34]) {
        gear.add(wheel(kit, x + dx, mainWheelY, z + dz, mainWheelR, 0.28));
      }
    }
  };
  for (const side of [1, -1]) {
    // Outboard wing main gear bogie.
    addMainBogie(side * 3.55, -1.15, wingYAt(3.55) - 0.15, side * 2.1);
    // Inboard body main gear bogie (staggered slightly aft, like the real 747).
    addMainBogie(side * 1.42, -2.65, -1.65, side * 0.45);
  }
  // Twin-wheel nose gear (bottom contact matches main gear at y = -4.26).
  gear.add(rod(kit.metal, [0, -1.8, 11.8], [0, -3.84, 11.8], 0.13, 8));
  gear.add(rod(kit.metal, [0, -1.9, 10.6], [0, -2.9, 11.8], 0.06, 6));
  gear.add(rod(kit.metal, [-0.32, -3.84, 11.8], [0.32, -3.84, 11.8], 0.07, 6));
  gear.add(wheel(kit, 0.32, -3.84, 11.8, 0.42, 0.25));
  gear.add(wheel(kit, -0.32, -3.84, 11.8, 0.42, 0.25));
  group.add(gear);

  // Nav lights on swept wingtips & tail cone, beacons on upper-deck crown & belly.
  navLights(group, kit, [wingSpan - 0.2, tipY + 0.12, -6.5], [0, tailY + 0.32, -16.1]);
  group.add(dot(kit.beacon, 0, 3.72, 6.2, 0.22));
  group.add(dot(kit.beacon, 0, -2.42, 4.5, 0.24));

  return finish(group, 0.67, kit, {
    refuelPort: new THREE.Vector3(0, 1.92, 13.8),
    landingLightPos: new THREE.Vector3(0, -0.65, 17.3), // just ahead of nose tip (z=16.45)
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

  const canopy = new THREE.Mesh(fuselage([
    [1.6, 0.0001], [2.0, 0.35], [3.3, 0.55], [4.6, 0.42], [5.6, 0.0001],
  ], { sy: 1.15, segments: 20 }).geo, kit.glass);
  canopy.position.y = 0.72;
  group.add(canopy);
  // Canopy bow frame between windscreen and bubble.
  const bow = new THREE.Mesh(new THREE.TorusGeometry(0.5, 0.035, 6, 20, Math.PI), kit.dark);
  bow.scale.y = 1.15;
  bow.position.set(0, 0.72, 4.0);
  group.add(bow);
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

  const gear = new THREE.Group();
  gear.name = 'gear';
  for (const side of [1, -1]) {
    gear.add(rod(kit.metal, [side * 1.3, -0.4, -1.0], [side * 1.5, -1.9, -1.0], 0.09));
    gear.add(wheel(kit, side * 1.5, -2.0, -1.0, 0.38, 0.26));
  }
  gear.add(rod(kit.metal, [0, -0.5, 4.6], [0, -1.9, 4.8], 0.08));
  gear.add(wheel(kit, 0, -2.05, 4.8, 0.28, 0.2));
  group.add(gear);

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

  // Sensor turret under the nose, SATCOM and data-link antennas on the spine.
  const mount = new THREE.Mesh(new THREE.CylinderGeometry(0.28, 0.34, 0.5, 12), kit.plain);
  mount.position.set(0, -1.0, 5.7);
  group.add(mount);
  group.add(dot(kit.glass, 0, -1.4, 5.7, 0.5));
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

  const gear = new THREE.Group();
  gear.name = 'gear';
  for (const side of [1, -1]) {
    gear.add(rod(kit.metal, [side * 0.5, -0.6, 0.6], [side * 2.0, -2.1, 0.6], 0.08));
    gear.add(wheel(kit, side * 2.05, -2.2, 0.6, 0.36, 0.22));
  }
  gear.add(rod(kit.metal, [0, -0.9, 4.4], [0, -2.15, 4.4], 0.07));
  gear.add(wheel(kit, 0, -2.25, 4.4, 0.28, 0.18));
  group.add(gear);

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
  ], { sy: 1.06, warp, segments: 36 });
  const kit = paintKit(model, 'tanker', {
    zmin: body.zmin, zmax: body.zmax, circ: body.circ,
    bodyColor: TANKER_GREY.body, bellyColor: TANKER_GREY.belly, bellyA: 1.98,
    radomeZ: 14.95, radomeColor: TANKER_GREY.radome,
    antiGlare: { z0: 14.3, z1: 15.0, a: 0.44, color: TANKER_GREY.antiGlare },
    cockpit: { z0: 13.5, z1: 14.35, a: 0.78, frameColor: TANKER_GREY.body },
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
  const gear = new THREE.Group();
  gear.name = 'gear';
  for (const side of [1, -1]) {
    gear.add(rod(kit.metal, [side * 2.4, -1.6, -2.0], [side * 2.4, -5.2, -2.0], 0.16));
    gear.add(wheel(kit, side * 2.4, -5.2, -2.0, 0.65, 0.38));
  }
  gear.add(rod(kit.metal, [0, -1.6, 10.0], [0, -5.35, 10.0], 0.14));
  gear.add(wheel(kit, 0, -5.35, 10.0, 0.5, 0.3));
  group.add(gear);

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
