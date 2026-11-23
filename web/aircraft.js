// Aircraft meshes. Every builder takes the raw model id, paints the airframe
// in that model provider's livery (see livery.js), and returns a group whose
// nose points along +Z with +Y up. +X is the aircraft's left (port) side.
//
// Fuselages are lathed from a smoothed side profile with UVs that wrap the
// body (u = angle around, v = position along), so liveries are painted as one
// skin. Flying surfaces are lofted symmetric airfoils with real taper, sweep,
// thickness and dihedral.

import * as THREE from './vendor/three.module.js';
import { liveryFor, fuselageTexture, finTexture, logoTexture } from './livery.js';

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

// NACA 00xx half-thickness at chord fraction c (0 = leading edge).
function naca(c, t) {
  return 5 * t * (0.2969 * Math.sqrt(c) - 0.126 * c - 0.3516 * c * c
    + 0.2843 * c ** 3 - 0.1036 * c ** 4);
}

// Lofted flying surface. Span runs along +X (times `side`), chord along -Z
// from the leading edge, thickness along Y.
//   root/tip: { le: z of leading edge, chord, y: height offset }
// UVs: u runs leading -> trailing edge across the whole surface, v root -> tip.
function surfaceGeo({ span, root, tip, thickness = 0.12, side = 1, x0 = 0 }) {
  const N = 9; // stations along each airfoil surface
  const ring = [];
  for (let i = 0; i < N; i++) ring.push([1 - i / (N - 1), 1]); // TE -> LE (upper)
  for (let i = 1; i < N - 1; i++) ring.push([i / (N - 1), -1]); // LE -> TE (lower)
  const M = ring.length;
  const sections = [{ x: x0, ...root }, { x: x0 + span, ...tip }];
  const zFront = Math.max(root.le, tip.le);
  const zBack = Math.min(root.le - root.chord, tip.le - tip.chord);
  const uOf = (z) => (zFront - z) / (zFront - zBack);

  const verts = [];
  const uvs = [];
  sections.forEach((s, si) => {
    for (const [c, sgn] of ring) {
      const half = Math.max(naca(c, thickness), 0.0025) * s.chord;
      const z = s.le - c * s.chord;
      verts.push(side * s.x, (s.y ?? 0) + sgn * half, z);
      uvs.push(uOf(z), si);
    }
  });
  const capIndex = verts.length / 3;
  sections.forEach((s, si) => {
    const z = s.le - 0.4 * s.chord;
    verts.push(side * s.x, s.y ?? 0, z);
    uvs.push(uOf(z), si);
  });

  const idx = [];
  const tri = (a, b, c) => (side > 0 ? idx.push(a, c, b) : idx.push(a, b, c));
  for (let k = 0; k < M; k++) {
    const k2 = (k + 1) % M;
    tri(k, M + k, M + k2);
    tri(k, M + k2, k2);
  }
  for (let k = 0; k < M; k++) {
    const k2 = (k + 1) % M;
    tri(capIndex + 1, M + k2, M + k); // tip
    tri(capIndex, k, k2); // root
  }

  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.Float32BufferAttribute(verts, 3));
  geo.setAttribute('uv', new THREE.Float32BufferAttribute(uvs, 2));
  geo.setIndex(idx);
  const flat = geo.toNonIndexed();
  flat.computeVertexNormals();
  return flat;
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

function wheel(mat, x, y, z, r, w) {
  const m = new THREE.Mesh(new THREE.CylinderGeometry(r, r, w, 14), mat);
  m.rotation.z = Math.PI / 2;
  m.position.set(x, y, z);
  return m;
}

function dot(mat, x, y, z, r) {
  const m = new THREE.Mesh(new THREE.SphereGeometry(r, 8, 6), mat);
  m.position.set(x, y, z);
  return m;
}

// Livery materials for one aircraft. Painted textures are shared between
// aircraft with the same livery (see livery.js), materials are per aircraft.
function paintKit(model, airframe, layout) {
  const livery = liveryFor(model);
  const std = (opts) => new THREE.MeshStandardMaterial({ roughness: 0.42, metalness: 0.08, ...opts });
  const kit = {
    livery,
    body: std({ map: fuselageTexture(livery, airframe, layout) }),
    plain: std({ color: livery.body }),
    wing: std({ color: livery.wing }),
    fin: std({ map: finTexture(livery) }),
    engine: std({ color: livery.engine, roughness: 0.35, metalness: 0.25 }),
    logo: new THREE.MeshBasicMaterial({
      map: logoTexture(livery), transparent: true, depthWrite: false,
      polygonOffset: true, polygonOffsetFactor: -2,
    }),
    dark: new THREE.MeshStandardMaterial({ color: 0x20262e, roughness: 0.7 }),
    metal: new THREE.MeshStandardMaterial({ color: 0x9aa4ae, roughness: 0.35, metalness: 0.6 }),
    glass: new THREE.MeshStandardMaterial({ color: 0x1b2a3a, roughness: 0.1, metalness: 0.5 }),
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
// scaling), so aircraft sit exactly on the apron.
function finish(group, scale, kit) {
  const box = new THREE.Box3().setFromObject(group);
  return {
    group, scale,
    tintMaterials: [kit.beacon], disposeMaterials: kit.all,
    wheelDrop: -box.min.y * scale,
  };
}

/* ------------------------------------------------ private plane (local) */

// High-wing single-prop tourer: local models.
export function makeAgentMesh(model) {
  const group = new THREE.Group();
  const tailRise = (z) => (z < -0.5 ? (-0.5 - z) * 0.17 : 0);
  const body = fuselage([
    [-6.6, 0.18], [-5.6, 0.42], [-3.5, 0.78], [-1.2, 1.15], [0.6, 1.3],
    [2.8, 1.3], [4.3, 1.18], [5.6, 0.98], [6.1, 0.88], [6.12, 0.0001],
  ], { sx: 0.88, sy: 1.12, warp: tailRise });
  const kit = paintKit(model, 'prop', {
    zmin: body.zmin, zmax: body.zmax, circ: body.circ,
    bellyA: 2.2, stripeA: 1.62, stripeW: 0.1, stripeZ1: 5.4, swooshZ: -3.4,
    windows: { kind: 'cabin', a: 1.05, span: 0.62, z0: -0.4, z1: 2.8 },
    cockpit: { z0: 2.9, z1: 4.0, a: 0.78 },
    titles: { a: 1.25, z: -2.3, size: 0.62 },
    nose: { a: 1.25, z: 4.75, size: 0.24 },
  });
  group.add(new THREE.Mesh(body.geo, kit.body));

  // Spinner + two-blade prop.
  const spinner = new THREE.Mesh(new THREE.ConeGeometry(0.42, 1.0, 14), kit.engine);
  spinner.rotation.x = Math.PI / 2;
  spinner.position.z = 6.55;
  group.add(spinner);
  const prop = new THREE.Group();
  prop.position.z = 6.3;
  prop.name = 'prop';
  prop.add(new THREE.Mesh(new THREE.CircleGeometry(2.3, 24), kit.propDisc));
  for (const s of [1, -1]) {
    const blade = new THREE.Mesh(new THREE.BoxGeometry(0.34, 2.1, 0.08), kit.dark);
    blade.position.y = s * 1.1;
    blade.rotation.y = s * 0.35;
    prop.add(blade);
  }
  group.add(prop);

  // High wing on the cabin roof, slight dihedral; struts to mid-span.
  const wingY = 1.5;
  for (const w of surfacePair(kit.plain, {
    span: 7.8, dihedral: 0.03, thickness: 0.14,
    root: { le: 2.6, chord: 2.2, y: wingY }, tip: { le: 2.35, chord: 1.55 },
  })) group.add(w);
  for (const side of [1, -1]) {
    group.add(rod(kit.dark, [side * 0.9, -0.55, 1.3], [side * 4.1, wingY + 0.1, 1.5], 0.07));
  }

  const tailY = tailRise(-5.4);
  for (const t of surfacePair(kit.plain, {
    span: 2.9, thickness: 0.1,
    root: { le: -4.5, chord: 1.6, y: tailY }, tip: { le: -5.1, chord: 0.95 },
  })) group.add(t);
  group.add(finMesh(kit.fin, {
    height: 2.4, thickness: 0.1, base: [0, tailY + 0.45],
    root: { le: -4.2, chord: 2.2 }, tip: { le: -5.5, chord: 1.1 },
    logo: kit.logo, logoAt: 0.42, logoScale: 0.8,
  }));
  group.add(finMesh(kit.fin, {
    height: 0.5, thickness: 0.1, base: [0, tailY + 0.3],
    root: { le: -2.4, chord: 2.0 }, tip: { le: -4.1, chord: 0.6 },
  }));

  // Fixed tricycle gear with wheel spats.
  for (const side of [1, -1]) {
    group.add(rod(kit.dark, [side * 0.6, -1.15, 0.6], [side * 1.75, -1.95, 0.55], 0.08));
    group.add(wheel(kit.dark, side * 1.8, -2.05, 0.55, 0.42, 0.28));
    const spat = new THREE.Mesh(fuselage([
      [-0.8, 0.05], [-0.5, 0.36], [0.1, 0.48], [0.6, 0.34], [0.9, 0.0001],
    ], { sx: 0.55, segments: 16 }).geo, kit.engine);
    spat.position.set(side * 1.8, -2.0, 0.6);
    group.add(spat);
  }
  group.add(rod(kit.dark, [0, -0.7, 5.0], [0, -2.0, 5.15], 0.08));
  group.add(wheel(kit.dark, 0, -2.1, 5.15, 0.34, 0.22));

  navLights(group, kit, [7.8, wingY + 0.25, 1.4], [0, tailY + 2.9, -6.55]);
  group.add(dot(kit.beacon, 0, 1.6, -0.6, 0.15));

  return finish(group, 0.9, kit);
}

/* -------------------------------------------- twin-engine jet (frontier) */

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
  group.add(new THREE.Mesh(body.geo, kit.body));

  // Low swept wings with dihedral, belly fairing.
  const wingY = -0.95;
  const wingSpan = 12.5;
  const wingDihedral = 0.09;
  for (const w of surfacePair(kit.wing, {
    span: wingSpan, dihedral: wingDihedral, thickness: 0.13,
    root: { le: 3.0, chord: 6.2, y: wingY }, tip: { le: -3.6, chord: 1.6 },
  })) group.add(w);
  // Winglets in the tail colour.
  const tipY = wingY + Math.tan(wingDihedral) * wingSpan;
  for (const side of [1, -1]) {
    const winglet = finMesh(kit.fin, {
      height: 1.4, thickness: 0.08, base: [side * wingSpan, tipY], cant: 0.35, side,
      root: { le: -3.6, chord: 1.6 }, tip: { le: -4.6, chord: 0.6 },
    });
    group.add(winglet);
  }
  group.add(new THREE.Mesh(fuselage([
    [-3.6, 0.2], [-2.5, 0.9], [1.5, 1.0], [3.4, 0.5], [3.9, 0.0001],
  ], { sx: 1.7, sy: 0.6, warp: () => -1.05 }).geo, kit.wing));

  // Engines on pylons, nacelles in the brand colour.
  const ex = 4.6;
  const wingLeAt = 3.0 + (-3.6 - 3.0) * (ex / wingSpan);
  const wingYAt = wingY + Math.tan(wingDihedral) * ex;
  for (const side of [1, -1]) {
    const nacelle = new THREE.Mesh(fuselage([
      [-2.0, 0.0001], [-1.9, 0.35], [-1.3, 0.62], [0.6, 0.92],
      [1.8, 0.95], [2.4, 0.9], [2.45, 0.0001],
    ], { segments: 20 }).geo, kit.engine);
    nacelle.position.set(side * ex, wingYAt - 1.25, wingLeAt + 0.3);
    group.add(nacelle);
    const fan = new THREE.Mesh(new THREE.CircleGeometry(0.8, 20), kit.dark);
    fan.position.set(side * ex, wingYAt - 1.25, wingLeAt + 0.3 + 2.44);
    group.add(fan);
    const cone = new THREE.Mesh(new THREE.ConeGeometry(0.22, 0.5, 12), kit.metal);
    cone.rotation.x = Math.PI / 2;
    cone.position.set(side * ex, wingYAt - 1.25, wingLeAt + 0.3 + 2.5);
    group.add(cone);
    const pylon = new THREE.Mesh(new THREE.BoxGeometry(0.24, 0.9, 2.8), kit.wing);
    pylon.position.set(side * ex, wingYAt - 0.4, wingLeAt - 0.6);
    group.add(pylon);
  }

  // Swept stabilisers and a tall branded fin.
  const tailY = warp(-10);
  for (const t of surfacePair(kit.wing, {
    span: 4.8, dihedral: 0.1, thickness: 0.1,
    root: { le: -8.4, chord: 3.0, y: tailY }, tip: { le: -11.0, chord: 1.2 },
  })) group.add(t);
  group.add(finMesh(kit.fin, {
    height: 5.0, thickness: 0.11, base: [0, tailY + 0.9],
    root: { le: -6.6, chord: 4.6 }, tip: { le: -10.6, chord: 1.8 },
    logo: kit.logo, logoAt: 0.42,
  }));

  // Retractable gear (hidden in flight by main.js).
  const gear = new THREE.Group();
  gear.name = 'gear';
  for (const side of [1, -1]) {
    gear.add(rod(kit.metal, [side * 2.2, wingY - 0.2, -1.2], [side * 2.2, -3.3, -1.2], 0.14));
    for (const dx of [2.0, 2.4]) {
      for (const dz of [-0.85, -1.55]) gear.add(wheel(kit.dark, side * dx, -3.4, dz, 0.5, 0.35));
    }
  }
  gear.add(rod(kit.metal, [0, -1.4, 9.2], [0, -3.4, 9.2], 0.12));
  gear.add(wheel(kit.dark, 0.2, -3.5, 9.2, 0.38, 0.25));
  gear.add(wheel(kit.dark, -0.2, -3.5, 9.2, 0.38, 0.25));
  group.add(gear);

  navLights(group, kit, [wingSpan - 0.2, tipY + 0.1, -3.9], [0, tailY + 0.2, -12.7]);
  group.add(dot(kit.beacon, 0, 1.86, 0.5, 0.24));
  group.add(dot(kit.beacon, 0, -1.86, 4.5, 0.24));

  return finish(group, 0.74, kit);
}

/* --------------------------------------------- fighter (sub-agent wingman) */

// Twin-tail fighter: sub-agents flying wingman.
export function makeFighterMesh(model) {
  const group = new THREE.Group();
  const body = fuselage([
    [-6.4, 0.85], [-5.6, 1.1], [-3, 1.3], [1, 1.3], [3.5, 1.05],
    [5.5, 0.72], [7.5, 0.4], [9.0, 0.12], [9.5, 0.0001],
  ], { sx: 1.35, sy: 0.72, warp: (z) => (z > 4 ? (z - 4) * -0.03 : 0) });
  const kit = paintKit(model, 'fighter', {
    zmin: body.zmin, zmax: body.zmax, circ: body.circ,
    bellyA: 2.0, stripeA: 1.45, stripeW: 0.09, stripeZ1: 7.0, swooshZ: -2.2,
    titles: { a: 1.0, z: -1.6, size: 0.5 },
  });
  group.add(new THREE.Mesh(body.geo, kit.body));
  const burnerMat = new THREE.MeshBasicMaterial({
    color: 0xff9d2e, transparent: true, opacity: 0.85, depthWrite: false,
  });
  kit.all.push(burnerMat);

  for (const side of [1, -1]) {
    const intake = new THREE.Mesh(new THREE.BoxGeometry(0.6, 0.75, 3.4), kit.plain);
    intake.position.set(side * 1.45, -0.15, 1.6);
    intake.rotation.y = side * 0.06;
    group.add(intake);
    const mouth = new THREE.Mesh(new THREE.PlaneGeometry(0.5, 0.65), kit.dark);
    mouth.position.set(side * 1.5, -0.15, 3.31);
    group.add(mouth);
  }

  const canopy = new THREE.Mesh(fuselage([
    [1.6, 0.0001], [2.0, 0.35], [3.3, 0.55], [4.6, 0.42], [5.6, 0.0001],
  ], { sy: 1.15, segments: 16 }).geo, kit.glass);
  canopy.position.y = 0.72;
  group.add(canopy);

  for (const w of surfacePair(kit.plain, {
    span: 6.6, dihedral: -0.04, thickness: 0.05,
    root: { le: 2.8, chord: 7.6, y: 0 }, tip: { le: -2.6, chord: 1.5 },
  })) group.add(w);
  for (const w of surfacePair(kit.plain, {
    span: 1.6, thickness: 0.04,
    root: { le: 5.4, chord: 3.0, y: 0.05 }, tip: { le: 2.2, chord: 0.5 },
  })) group.add(w);
  for (const t of surfacePair(kit.plain, {
    span: 3.6, dihedral: -0.03, thickness: 0.05,
    root: { le: -3.8, chord: 2.7, y: -0.05 }, tip: { le: -5.6, chord: 1.0 },
  })) group.add(t);

  for (const side of [1, -1]) {
    group.add(finMesh(kit.fin, {
      height: 3.0, thickness: 0.06, base: [side * 1.0, 0.45], cant: 0.4, side,
      root: { le: -2.4, chord: 3.4 }, tip: { le: -4.8, chord: 1.2 },
      logo: kit.logo, logoAt: 0.4, logoScale: 0.62,
    }));
  }

  for (const side of [1, -1]) {
    const nozzle = new THREE.Mesh(new THREE.CylinderGeometry(0.5, 0.42, 1.0, 12), kit.dark);
    nozzle.rotation.x = Math.PI / 2;
    nozzle.position.set(side * 0.62, -0.05, -6.6);
    group.add(nozzle);
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
    gear.add(wheel(kit.dark, side * 1.5, -2.0, -1.0, 0.38, 0.26));
  }
  gear.add(rod(kit.metal, [0, -0.5, 4.6], [0, -1.9, 4.8], 0.08));
  gear.add(wheel(kit.dark, 0, -2.05, 4.8, 0.28, 0.2));
  group.add(gear);

  navLights(group, kit, [6.6, -0.2, -2.9], [0, 0.0, -6.5]);
  group.add(dot(kit.beacon, 0, 0.93, -1.2, 0.18));

  return finish(group, 0.72, kit);
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
  group.add(new THREE.Mesh(body.geo, kit.body));

  const mount = new THREE.Mesh(new THREE.CylinderGeometry(0.28, 0.34, 0.5, 10), kit.plain);
  mount.position.set(0, -1.0, 5.7);
  group.add(mount);
  group.add(dot(kit.glass, 0, -1.4, 5.7, 0.5));

  for (const w of surfacePair(kit.plain, {
    span: 12.5, dihedral: 0.025, thickness: 0.14,
    root: { le: 1.8, chord: 2.3, y: 0.35 }, tip: { le: 1.0, chord: 0.95 },
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

  const hub = new THREE.Mesh(new THREE.ConeGeometry(0.4, 0.9, 12), kit.engine);
  hub.rotation.x = -Math.PI / 2;
  hub.position.z = -7.4;
  group.add(hub);
  const prop = new THREE.Group();
  prop.position.z = -7.25;
  prop.name = 'prop';
  prop.add(new THREE.Mesh(new THREE.CircleGeometry(1.85, 24), kit.propDisc));
  for (let i = 0; i < 3; i++) {
    const blade = new THREE.Group();
    const b = new THREE.Mesh(new THREE.BoxGeometry(0.26, 1.75, 0.07), kit.dark);
    b.position.y = 0.95;
    b.rotation.y = 0.35;
    blade.add(b);
    blade.rotation.z = (i * 2 * Math.PI) / 3;
    prop.add(blade);
  }
  group.add(prop);

  const gear = new THREE.Group();
  gear.name = 'gear';
  for (const side of [1, -1]) {
    gear.add(rod(kit.metal, [side * 0.5, -0.6, 0.6], [side * 2.0, -2.1, 0.6], 0.08));
    gear.add(wheel(kit.dark, side * 2.05, -2.2, 0.6, 0.36, 0.22));
  }
  gear.add(rod(kit.metal, [0, -0.9, 4.4], [0, -2.15, 4.4], 0.07));
  gear.add(wheel(kit.dark, 0, -2.25, 4.4, 0.28, 0.18));
  group.add(gear);

  navLights(group, kit, [12.4, 0.75, 0.6], [0, 0.0, -6.9]);
  group.add(dot(kit.beacon, 0, 1.4, 2.0, 0.2));

  return finish(group, 0.76, kit);
}
