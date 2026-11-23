// Runway events: make takeoffs and landings at the home airport read as an
// event from anywhere on the map.
//
//  - Landing lights: a bright nose glow and forward beam on any aircraft
//    rolling, climbing out or on approach.
//  - Shockwave ring + light pillar at the moment of liftoff / touchdown.
//  - Runway chase lights: a pulse of light racing down both runway edges in
//    the direction of travel while the runway is in use.
//  - Glidepath beads: a string of amber lights in the sky tracing the final
//    approach, with a pulse running down toward the threshold.
//  - Tyre smoke at touchdown and dust thrown up behind a takeoff roll.

import * as THREE from './vendor/three.module.js';

export const TAKEOFF_COLOR = 0x3dff9a;
export const LANDING_COLOR = 0xffb020;

function radialTexture(stops) {
  const c = document.createElement('canvas');
  c.width = 64;
  c.height = 64;
  const ctx = c.getContext('2d');
  const g = ctx.createRadialGradient(32, 32, 0, 32, 32, 32);
  for (const [at, color] of stops) g.addColorStop(at, color);
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 64, 64);
  const tex = new THREE.CanvasTexture(c);
  tex.userData.shared = true; // never disposed with an aircraft
  return tex;
}

const glowTex = radialTexture([
  [0, 'rgba(255,255,255,1)'],
  [0.15, 'rgba(255,255,255,0.9)'],
  [0.4, 'rgba(255,255,255,0.25)'],
  [1, 'rgba(255,255,255,0)'],
]);
const smokeTex = radialTexture([
  [0, 'rgba(255,255,255,0.85)'],
  [0.5, 'rgba(255,255,255,0.35)'],
  [1, 'rgba(255,255,255,0)'],
]);

// Vertical fade (opaque at the bottom, clear at the top) for pillars & beams.
const fadeTex = (() => {
  const c = document.createElement('canvas');
  c.width = 4;
  c.height = 128;
  const ctx = c.getContext('2d');
  const g = ctx.createLinearGradient(0, 128, 0, 0);
  g.addColorStop(0, 'rgba(255,255,255,1)');
  g.addColorStop(0.35, 'rgba(255,255,255,0.45)');
  g.addColorStop(1, 'rgba(255,255,255,0)');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 4, 128);
  const tex = new THREE.CanvasTexture(c);
  tex.userData.shared = true;
  return tex;
})();

const easeOut = (t) => 1 - (1 - t) ** 3;

/**
 * Landing lights on an aircraft mesh: a nose glow (sized in world units so it
 * still reads from the overview camera) and a forward/down light beam.
 * Returns { group, setLevel(0..1, now) }.
 */
export function addLandingLights(built) {
  const s = built.scale;
  const box = new THREE.Box3().setFromObject(built.group);
  // Bounds are in world units of the unscaled group at creation time.
  const noseZ = box.max.z / s;
  const midY = ((box.min.y + box.max.y) * 0.5) / s;

  const group = new THREE.Group();
  group.name = 'landing-lights';
  group.visible = false;

  const glowMat = new THREE.SpriteMaterial({
    map: glowTex,
    color: 0xfffbe8,
    transparent: true,
    depthWrite: false,
    blending: THREE.AdditiveBlending,
    toneMapped: false,
  });
  const glow = new THREE.Sprite(glowMat);
  glow.position.set(0, midY, noseZ * 0.92);
  glow.scale.setScalar(16 / s);
  glow.renderOrder = 5;
  group.add(glow);

  // Beam: an open cone pointing forward and slightly down from the nose.
  const len = 46 / s;
  const beamGeo = new THREE.CylinderGeometry(0.3 / s, 7 / s, len, 18, 1, true);
  beamGeo.translate(0, -len / 2, 0); // apex at origin, opening along -Y
  const beamMat = new THREE.MeshBasicMaterial({
    map: fadeTex,
    color: 0xfff4d0,
    transparent: true,
    opacity: 0.35,
    depthWrite: false,
    side: THREE.DoubleSide,
    blending: THREE.AdditiveBlending,
    toneMapped: false,
  });
  const beam = new THREE.Mesh(beamGeo, beamMat);
  beam.position.set(0, midY, noseZ * 0.92);
  beam.rotation.x = -Math.PI / 2 - 0.16; // -Y -> +Z (forward), tipped down
  beam.castShadow = false;
  beam.receiveShadow = false;
  group.add(beam);

  built.group.add(group);

  let level = 0;
  return {
    group,
    setLevel(target, dt, now) {
      level += (target - level) * (1 - Math.exp(-4 * dt));
      group.visible = level > 0.02;
      if (!group.visible) return;
      const flicker = 0.92 + 0.08 * Math.sin(now * 23);
      glowMat.opacity = level * flicker;
      beamMat.opacity = 0.35 * level;
    },
  };
}

/**
 * Event layer for one runway. `opts` = { x, zMin, zMax, deckY,
 * glideFrom: [z, alt] (final approach fix), touchdownZ }.
 */
export function createRunwayEvents(scene, opts) {
  const { x, zMin, zMax, deckY, glideFrom, touchdownZ } = opts;
  const root = new THREE.Group();
  root.name = 'runway-events';
  scene.add(root);

  const transient = []; // { obj, born, life, update(t) }
  const addTransient = (obj, life, update) => {
    root.add(obj);
    transient.push({ obj, born: performance.now() / 1000, life, update });
  };

  // ---- shockwave ring + light pillar ----------------------------------
  const ringGeo = new THREE.RingGeometry(0.86, 1, 96);
  ringGeo.rotateX(-Math.PI / 2);
  const pillarGeo = new THREE.CylinderGeometry(1, 1, 1, 32, 1, true);
  pillarGeo.translate(0, 0.5, 0);

  function shockwave(pos, color, { radius = 75, life = 2.2, delay = 0 } = {}) {
    const mat = new THREE.MeshBasicMaterial({
      color,
      transparent: true,
      depthWrite: false,
      side: THREE.DoubleSide,
      toneMapped: false,
    });
    const ring = new THREE.Mesh(ringGeo, mat);
    ring.position.set(pos.x, pos.y + 0.4, pos.z);
    ring.visible = false;
    ring.renderOrder = 3;
    addTransient(ring, life + delay, (t) => {
      const k = (t - delay) / life;
      ring.visible = k >= 0;
      if (k < 0) return;
      const r = 3 + radius * easeOut(k);
      ring.scale.set(r, 1, r);
      mat.opacity = 0.9 * (1 - k) ** 1.5;
    });
  }

  function pillar(pos, color, { height = 150, radius = 5, life = 2.8 } = {}) {
    const mat = new THREE.MeshBasicMaterial({
      map: fadeTex,
      color,
      transparent: true,
      depthWrite: false,
      side: THREE.DoubleSide,
      blending: THREE.AdditiveBlending,
      toneMapped: false,
    });
    const col = new THREE.Mesh(pillarGeo, mat);
    col.position.set(pos.x, pos.y, pos.z);
    col.renderOrder = 3;
    addTransient(col, life, (t) => {
      const k = t / life;
      const rise = easeOut(Math.min(1, k * 3));
      col.scale.set(radius * (1 + k * 0.6), height * rise, radius * (1 + k * 0.6));
      mat.opacity = 0.85 * (k < 0.15 ? k / 0.15 : (1 - k) / 0.85);
    });
  }

  // ---- smoke / dust puffs ---------------------------------------------
  function puff(pos, { size = 5, grow = 3, life = 2.2, color = 0xf4f4f0, drift = [0, 0.8, 0], opacity = 0.75 } = {}) {
    const mat = new THREE.SpriteMaterial({
      map: smokeTex,
      color,
      transparent: true,
      depthWrite: false,
      opacity,
    });
    const sp = new THREE.Sprite(mat);
    sp.position.copy(pos);
    const vel = new THREE.Vector3(...drift);
    addTransient(sp, life, (t, dt) => {
      const k = t / life;
      sp.position.addScaledVector(vel, dt);
      sp.scale.setScalar(size * (1 + grow * easeOut(k)));
      mat.opacity = opacity * (1 - k) ** 1.3;
    });
  }

  // ---- runway chase lights --------------------------------------------
  // Two rows of bright beads that race along the runway edges.
  const CHASE_PER_SIDE = 7;
  const chasePos = new Float32Array(CHASE_PER_SIDE * 2 * 3);
  const chaseGeo = new THREE.BufferGeometry();
  chaseGeo.setAttribute('position', new THREE.BufferAttribute(chasePos, 3));
  const chaseMat = new THREE.PointsMaterial({
    map: glowTex,
    color: TAKEOFF_COLOR,
    size: 11,
    sizeAttenuation: true,
    transparent: true,
    depthWrite: false,
    blending: THREE.AdditiveBlending,
    toneMapped: false,
  });
  const chase = new THREE.Points(chaseGeo, chaseMat);
  chase.visible = false;
  chase.frustumCulled = false;
  chase.renderOrder = 4;
  root.add(chase);
  const halfW = opts.halfWidth ?? 10.6;
  let chaseUntil = 0;
  let chaseStart = 0;

  function runwayChase(color, duration) {
    const now = performance.now() / 1000;
    chaseMat.color.setHex(color);
    chaseStart = now;
    chaseUntil = now + duration;
  }

  function updateChase(now) {
    const active = now < chaseUntil;
    chase.visible = active;
    if (!active) return 0;
    const len = zMax - zMin;
    const sweep = 1.1; // seconds per runway length
    const head = ((now - chaseStart) / sweep) % 1; // 0 at threshold (+Z) -> 1 at far end
    for (let i = 0; i < CHASE_PER_SIDE; i++) {
      const f = (((head - i * 0.035) % 1) + 1) % 1;
      const z = zMax - len * f;
      for (const [side, sx] of [[0, -halfW], [1, halfW]]) {
        const o = (side * CHASE_PER_SIDE + i) * 3;
        chasePos[o] = x + sx;
        chasePos[o + 1] = deckY + 0.6;
        chasePos[o + 2] = z;
      }
    }
    chaseGeo.attributes.position.needsUpdate = true;
    const remaining = chaseUntil - now;
    chaseMat.opacity = Math.min(1, remaining / 0.6, (now - chaseStart) / 0.3);
    return chaseMat.opacity;
  }

  // ---- glidepath beads ------------------------------------------------
  const BEADS = 22;
  const beadPos = new Float32Array(BEADS * 3);
  const beadCol = new Float32Array(BEADS * 3);
  const [fafZ, fafAlt] = glideFrom;
  for (let i = 0; i < BEADS; i++) {
    const f = i / (BEADS - 1); // 0 at the fix, 1 at touchdown
    beadPos[i * 3] = x;
    beadPos[i * 3 + 1] = (1 - f) * fafAlt + f * deckY + 1.5;
    beadPos[i * 3 + 2] = fafZ + (touchdownZ - fafZ) * f;
  }
  const beadGeo = new THREE.BufferGeometry();
  beadGeo.setAttribute('position', new THREE.BufferAttribute(beadPos, 3));
  beadGeo.setAttribute('color', new THREE.BufferAttribute(beadCol, 3));
  const beadMat = new THREE.PointsMaterial({
    map: glowTex,
    size: 9,
    sizeAttenuation: true,
    vertexColors: true,
    transparent: true,
    depthWrite: false,
    blending: THREE.AdditiveBlending,
    toneMapped: false,
  });
  const beads = new THREE.Points(beadGeo, beadMat);
  beads.visible = false;
  beads.frustumCulled = false;
  beads.renderOrder = 4;
  root.add(beads);
  let glideLevel = 0;
  const beadBase = new THREE.Color(LANDING_COLOR);

  function updateGlidepath(active, dt, now) {
    glideLevel += ((active ? 1 : 0) - glideLevel) * (1 - Math.exp(-2.5 * dt));
    beads.visible = glideLevel > 0.02;
    if (!beads.visible) return;
    const head = (now * 0.9) % 1.3; // pulse running down toward the runway
    for (let i = 0; i < BEADS; i++) {
      const f = i / (BEADS - 1);
      const d = head - f;
      const pulse = d >= 0 && d < 0.18 ? 1 - d / 0.18 : 0;
      const b = glideLevel * (0.35 + 0.65 * pulse);
      beadCol[i * 3] = beadBase.r * b + pulse * 0.4 * glideLevel;
      beadCol[i * 3 + 1] = beadBase.g * b + pulse * 0.4 * glideLevel;
      beadCol[i * 3 + 2] = beadBase.b * b + pulse * 0.4 * glideLevel;
    }
    beadGeo.attributes.color.needsUpdate = true;
  }

  // ---- public event triggers ------------------------------------------
  const v = (px, pz, y = deckY) => new THREE.Vector3(px, y, pz);

  return {
    /** Aircraft starts its takeoff roll at runway position (px, pz). */
    takeoffRoll(px, pz) {
      shockwave(v(px, pz), TAKEOFF_COLOR, { radius: 30, life: 1.4 });
      runwayChase(TAKEOFF_COLOR, 9);
    },
    /** Wheels leave the runway. */
    liftoff(px, pz, y) {
      const p = v(px, pz);
      shockwave(p, TAKEOFF_COLOR, { radius: 80, life: 2.2 });
      shockwave(p, 0xffffff, { radius: 55, life: 1.8, delay: 0.25 });
      pillar(p, TAKEOFF_COLOR, { height: Math.max(140, y + 120) });
      for (let i = 0; i < 6; i++) {
        puff(v(px + (Math.random() - 0.5) * 10, pz + 4 + i * 3, deckY + 1.2), {
          size: 4, grow: 2.5, life: 1.6, color: 0xe9e3d6, opacity: 0.55,
          drift: [(Math.random() - 0.5) * 3, 1.2, 4],
        });
      }
      runwayChase(TAKEOFF_COLOR, 2.5);
    },
    /** Main gear touches down, rolling toward -Z. */
    touchdown(px, pz) {
      const p = v(px, pz);
      shockwave(p, LANDING_COLOR, { radius: 85, life: 2.4 });
      shockwave(p, 0xffffff, { radius: 55, life: 1.8, delay: 0.25 });
      pillar(p, LANDING_COLOR, { height: 150 });
      for (const side of [-1, 1]) {
        for (let i = 0; i < 5; i++) {
          puff(v(px + side * (2.4 + Math.random()), pz + i * 2.2, deckY + 1), {
            size: 3.5 + i * 0.6, grow: 3.2, life: 2.6 + Math.random(),
            drift: [side * 1.5, 1.4, 3 + i],
          });
        }
      }
      runwayChase(LANDING_COLOR, 5);
    },
    /** Dust kicked up behind an aircraft accelerating down the runway. */
    rollDust(px, pz, heading) {
      const back = new THREE.Vector3(-Math.cos(heading), 0, -Math.sin(heading));
      puff(v(px + back.x * 8 + (Math.random() - 0.5) * 4, pz + back.z * 8, deckY + 0.8), {
        size: 2.5, grow: 2.2, life: 1.1, color: 0xe6e0d2, opacity: 0.4,
        drift: [back.x * 6, 0.9, back.z * 6],
      });
    },
    update(dt, now, { approachActive }) {
      for (let i = transient.length - 1; i >= 0; i--) {
        const tr = transient[i];
        const t = now - tr.born;
        if (t >= tr.life) {
          root.remove(tr.obj);
          tr.obj.material.dispose();
          transient.splice(i, 1);
          continue;
        }
        tr.update(t, dt);
      }
      updateGlidepath(approachActive, dt, now);
      return updateChase(now);
    },
  };
}
