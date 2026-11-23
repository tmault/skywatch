// Runway lighting for an airport island: white edge lights, green threshold
// bar, red end bar, a PAPI on the left of the touchdown zone, and a
// sequenced approach strobe ("the rabbit") leading in to the threshold.
//
// Lamp housings are instanced meshes; the glow is one additive point cloud
// per colour. When the airport goes offline every lamp goes dark and the
// glow is switched off.

import * as THREE from './vendor/three.module.js';

const LAMP_OFF = 0x34383c;
const COLORS = {
  edge: 0xfff4d6,
  threshold: 0x3dff7a,
  end: 0xff3a2e,
  papiWhite: 0xffffff,
  papiRed: 0xff3a2e,
  approach: 0xffffff,
};

let glowTexture = null;
function getGlowTexture() {
  if (glowTexture) return glowTexture;
  const c = document.createElement('canvas');
  c.width = 64;
  c.height = 64;
  const ctx = c.getContext('2d');
  const g = ctx.createRadialGradient(32, 32, 0, 32, 32, 32);
  g.addColorStop(0, 'rgba(255,255,255,1)');
  g.addColorStop(0.18, 'rgba(255,255,255,0.85)');
  g.addColorStop(0.45, 'rgba(255,255,255,0.25)');
  g.addColorStop(1, 'rgba(255,255,255,0)');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 64, 64);
  glowTexture = new THREE.CanvasTexture(c);
  return glowTexture;
}

const lampGeo = new THREE.CylinderGeometry(0.28, 0.34, 0.35, 6);
const postGeo = new THREE.CylinderGeometry(0.12, 0.12, 1, 5);

function makeGroup(parent, points, color, glowSize) {
  const lampMat = new THREE.MeshBasicMaterial({ color });
  const lamps = new THREE.InstancedMesh(lampGeo, lampMat, points.length);
  const m = new THREE.Matrix4();
  points.forEach((p, i) => {
    m.makeTranslation(p.x, p.y, p.z);
    lamps.setMatrixAt(i, m);
  });
  lamps.instanceMatrix.needsUpdate = true;
  parent.add(lamps);

  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.Float32BufferAttribute(
    points.flatMap((p) => [p.x, p.y + 0.3, p.z]), 3,
  ));
  const glowMat = new THREE.PointsMaterial({
    color,
    map: getGlowTexture(),
    size: glowSize,
    sizeAttenuation: true,
    transparent: true,
    depthWrite: false,
    blending: THREE.AdditiveBlending,
    toneMapped: false,
  });
  const glow = new THREE.Points(geo, glowMat);
  glow.renderOrder = 2;
  parent.add(glow);
  return { lampMat, glow, glowMat, color };
}

/**
 * Light a north–south runway (aircraft land rolling towards -Z, so the
 * approach end is at +Z).
 *
 * opts = { x, zMin, zMax, halfWidth, y, groundY, spacing, approachLength }
 */
export function addRunwayLights(parent, opts) {
  const {
    x, zMin, zMax, halfWidth, y, groundY = 2.0,
    spacing = 8, approachLength = 24,
  } = opts;
  const lampY = y + 0.15;
  // Everything lives in one group so callers can tag or hide it as a unit.
  const group = new THREE.Group();
  group.name = 'runway-lights';
  parent.add(group);
  parent = group;

  // Edge lights down both sides.
  const edge = [];
  const n = Math.max(2, Math.round((zMax - zMin) / spacing));
  for (let i = 0; i <= n; i++) {
    const z = zMin + ((zMax - zMin) * i) / n;
    edge.push({ x: x - halfWidth, y: lampY, z }, { x: x + halfWidth, y: lampY, z });
  }

  // Threshold (green, approach end) and runway end (red, far end) bars.
  const threshold = [];
  const end = [];
  const barCount = 7;
  for (let i = 0; i < barCount; i++) {
    const bx = x - halfWidth + (2 * halfWidth * i) / (barCount - 1);
    threshold.push({ x: bx, y: lampY, z: zMax + 0.9 });
    end.push({ x: bx, y: lampY, z: zMin - 0.9 });
  }

  // PAPI: four boxes left of the touchdown zone — two white over two red
  // reads as "on glidepath".
  const papiZ = zMax - (zMax - zMin) * 0.2;
  const papiWhite = [];
  const papiRed = [];
  for (let i = 0; i < 4; i++) {
    const p = { x: x - halfWidth - 2.4 - i * 1.6, y: lampY, z: papiZ };
    (i < 2 ? papiWhite : papiRed).push(p);
  }

  const groups = [
    makeGroup(parent, edge, COLORS.edge, 4.2),
    makeGroup(parent, threshold, COLORS.threshold, 5.2),
    makeGroup(parent, end, COLORS.end, 5.2),
    makeGroup(parent, papiWhite, COLORS.papiWhite, 4.6),
    makeGroup(parent, papiRed, COLORS.papiRed, 4.6),
  ];

  // Approach strobes on posts beyond the threshold, flashed in sequence.
  const postMat = new THREE.MeshStandardMaterial({ color: 0x7a7f86, roughness: 0.6 });
  const strobes = [];
  const strobeCount = 5;
  for (let i = 0; i < strobeCount; i++) {
    const z = zMax + 5 + (approachLength * (strobeCount - 1 - i)) / (strobeCount - 1);
    const postH = lampY - groundY + 0.4;
    const post = new THREE.Mesh(postGeo, postMat);
    post.scale.y = postH;
    post.position.set(x, groundY + postH / 2, z);
    parent.add(post);
    const s = makeGroup(parent, [{ x, y: groundY + postH, z }], COLORS.approach, 9);
    strobes.push(s);
  }

  let online = true;
  const setOnline = (on) => {
    online = on;
    for (const g of [...groups, ...strobes]) {
      g.lampMat.color.setHex(on ? g.color : LAMP_OFF);
      g.glow.visible = on;
    }
  };

  // Sequenced flash: each strobe fires in turn towards the threshold.
  const update = (now) => {
    if (!online) return;
    const cycle = 1.6;
    const t = (now % cycle) / cycle;
    strobes.forEach((s, i) => {
      const phase = t * (strobeCount + 3) - i;
      const lit = phase > 0 && phase < 0.9;
      s.glow.visible = lit;
      s.lampMat.color.setHex(lit ? COLORS.approach : 0x9aa0a6);
    });
    // Gentle shimmer on the steady lights.
    const shimmer = 0.92 + 0.08 * Math.sin(now * 7.3);
    for (const g of groups) g.glowMat.opacity = shimmer;
  };

  setOnline(true);
  return { group, setOnline, update };
}
