// Local-model visual flair: towed sky-banners and cheeky "free-fuel"
// aerobatic barrel rolls with bursts of coloured airshow smoke for local
// Cessna prop planes (kind === 'agent' && model_class === 'local').

import * as THREE from './vendor/three.module.js';

/* ----------------------------------------------------------- banner text */

const BANNERS = [
  { badge: '$0', text: 'FREE TOKENS', accent: '#10b981', ink: '#0d2b22' },
  { badge: '$0', text: 'NO API BILL', accent: '#ef4444', ink: '#2b1014' },
  { badge: 'LOC', text: '127.0.0.1', accent: '#2563eb', ink: '#0f1f3d' },
  { badge: '$0', text: '100% LOCAL', accent: '#059669', ink: '#0a291f' },
  { badge: 'GPU', text: 'GPU GO BRRR', accent: '#d97706', ink: '#331c05' },
  { badge: '$0', text: '$0.00 / 1M', accent: '#10b981', ink: '#0d2b22' },
  { badge: 'DIY', text: 'HOMEGROWN AI', accent: '#7c3aed', ink: '#1f103b' },
];

const SMOKE_PALETTE = [
  0x2ef295, // free-tier emerald
  0xff4f8b, // airshow magenta
  0xffcb3d, // alpine gold
  0x38d9ff, // electric cyan
  0xff7a2f, // tangerine
];

const bannerTexCache = new Map();

function bannerTexture(spec) {
  const key = `${spec.badge}|${spec.text}|${spec.accent}`;
  if (bannerTexCache.has(key)) return bannerTexCache.get(key);

  const W = 512;
  const H = 128;
  const c = document.createElement('canvas');
  c.width = W;
  c.height = H;
  const ctx = c.getContext('2d');

  // Warm woven banner canvas background.
  ctx.fillStyle = '#fbf8f1';
  ctx.fillRect(0, 0, W, H);

  // Subtle vertical cloth panel seams.
  ctx.strokeStyle = 'rgba(0, 0, 0, 0.06)';
  ctx.lineWidth = 2;
  for (let x = 64; x < W; x += 64) {
    ctx.beginPath();
    ctx.moveTo(x, 0);
    ctx.lineTo(x, H);
    ctx.stroke();
  }

  // Bold top & bottom accent trim stripes + end caps.
  ctx.fillStyle = spec.accent;
  ctx.fillRect(0, 0, W, 14);
  ctx.fillRect(0, H - 14, W, 14);
  ctx.fillRect(0, 0, 12, H);
  ctx.fillRect(W - 12, 0, 12, H);

  // Inner fine border.
  ctx.strokeStyle = 'rgba(0, 0, 0, 0.14)';
  ctx.lineWidth = 2;
  ctx.strokeRect(18, 20, W - 36, H - 40);

  // Left pill badge ("$0", "LOC", etc.).
  const bx = 28;
  const by = 29;
  const bw = 78;
  const bh = H - 58;
  ctx.fillStyle = spec.accent;
  ctx.beginPath();
  ctx.roundRect(bx, by, bw, bh, 12);
  ctx.fill();

  ctx.fillStyle = '#ffffff';
  ctx.font = '900 34px "Inter", "Helvetica Neue", Arial, sans-serif';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText(spec.badge, bx + bw / 2, H / 2 + 2);

  // Main slogan text, auto-fit into the remaining banner area.
  const textAreaX = bx + bw + 14;
  const textAreaW = W - textAreaX - 24;
  let fontSize = 52;
  ctx.font = `900 ${fontSize}px "Inter", "Helvetica Neue", Arial, sans-serif`;
  while (ctx.measureText(spec.text).width > textAreaW && fontSize > 28) {
    fontSize -= 2;
    ctx.font = `900 ${fontSize}px "Inter", "Helvetica Neue", Arial, sans-serif`;
  }
  ctx.fillStyle = spec.ink;
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText(spec.text, textAreaX + textAreaW / 2, H / 2 + 2);

  const tex = new THREE.CanvasTexture(c);
  tex.colorSpace = THREE.SRGBColorSpace;
  tex.anisotropy = 4;
  tex.userData.shared = true;
  bannerTexCache.set(key, tex);
  return tex;
}

const smokePuffTex = (() => {
  const c = document.createElement('canvas');
  c.width = 64;
  c.height = 64;
  const ctx = c.getContext('2d');
  const g = ctx.createRadialGradient(32, 32, 0, 32, 32, 32);
  g.addColorStop(0, 'rgba(255,255,255,0.95)');
  g.addColorStop(0.42, 'rgba(255,255,255,0.55)');
  g.addColorStop(0.78, 'rgba(255,255,255,0.14)');
  g.addColorStop(1, 'rgba(255,255,255,0)');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 64, 64);
  const tex = new THREE.CanvasTexture(c);
  tex.userData.shared = true;
  return tex;
})();

/* ----------------------------------------------------------- sky banner */

const SEG_X = 16;
const SEG_Y = 2;
const ROPE_LEN = 7.5;
const BRIDLE_LEN = 2.0;
const BANNER_LEN = 26.0;
const BANNER_H = 6.2;
const SAMPLE_STEP = 0.7;
const MAX_PATH_LEN = ROPE_LEN + BANNER_LEN + 10;

// Tail cone hook in Cessna (makeAgentMesh) local model space.
const TAIL_HOOK_LOCAL = new THREE.Vector3(0, 0.68, -5.9);
// Exhaust / smoke-oil injector just aft of the Cessna tail belly.
const SMOKE_LOCAL = new THREE.Vector3(0, -0.45, -5.6);

const _hookWorld = new THREE.Vector3();
const _smokeWorld = new THREE.Vector3();
const _fwd = new THREE.Vector3();
const _side = new THREE.Vector3();
const _up = new THREE.Vector3(0, 1, 0);
const _tmp = new THREE.Vector3();

function buildBannerGeometry() {
  // Two opposed FrontSide sheets in one BufferGeometry so both port and
  // starboard sides read left-to-right (never mirrored).
  const vertsPerSheet = (SEG_X + 1) * (SEG_Y + 1);
  const positions = new Float32Array(vertsPerSheet * 2 * 3);
  const uvs = new Float32Array(vertsPerSheet * 2 * 2);
  const indices = [];

  for (let sheet = 0; sheet < 2; sheet++) {
    const base = sheet * vertsPerSheet;
    for (let i = 0; i <= SEG_X; i++) {
      const s = i / SEG_X;
      // Sheet 0 (+Side / starboard): rear (s=1) is left -> u = 1 - s.
      // Sheet 1 (-Side / port):      front (s=0) is left -> u = s.
      const u = sheet === 0 ? 1 - s : s;
      for (let r = 0; r <= SEG_Y; r++) {
        const v = r / SEG_Y;
        const idx = base + i * (SEG_Y + 1) + r;
        uvs[idx * 2] = u;
        uvs[idx * 2 + 1] = v;
      }
    }
    for (let i = 0; i < SEG_X; i++) {
      for (let r = 0; r < SEG_Y; r++) {
        const a = base + i * (SEG_Y + 1) + r;
        const b = base + (i + 1) * (SEG_Y + 1) + r;
        const c = base + (i + 1) * (SEG_Y + 1) + (r + 1);
        const d = base + i * (SEG_Y + 1) + (r + 1);
        if (sheet === 0) {
          indices.push(a, d, c, a, c, b);
        } else {
          indices.push(a, b, c, a, c, d);
        }
      }
    }
  }

  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.BufferAttribute(positions, 3));
  geo.setAttribute('uv', new THREE.BufferAttribute(uvs, 2));
  geo.setIndex(indices);
  return geo;
}

function hashId(id, seedText) {
  let h = Number(id) || 0;
  const s = seedText || '';
  for (let i = 0; i < s.length; i++) h = ((h * 31) + s.charCodeAt(i)) >>> 0;
  return h;
}

function smootherstep(t) {
  return t * t * t * (t * (t * 6 - 15) + 10);
}

/**
 * Create local-model flair controller for one Cessna aircraft:
 *  - Towed sky-banner in world space with live cloth wave & smooth turn following.
 *  - Periodic cheeky barrel rolls with bursts of coloured airshow smoke.
 */
export function createLocalFlair(scene, a) {
  const hash = hashId(a.id, a.callsign);
  const spec = BANNERS[hash % BANNERS.length];
  const phase = (hash % 97) * 0.37;

  const root = new THREE.Group();
  root.name = 'local-flair';
  root.visible = false;
  scene.add(root);

  // Banner cloth mesh (double-sheet unmirrored).
  const tex = bannerTexture(spec);
  const clothGeo = buildBannerGeometry();
  const clothMat = new THREE.MeshStandardMaterial({
    map: tex,
    emissive: 0xffffff,
    emissiveMap: tex,
    emissiveIntensity: 0.28,
    roughness: 0.82,
    metalness: 0.02,
  });
  const cloth = new THREE.Mesh(clothGeo, clothMat);
  cloth.frustumCulled = false;
  cloth.castShadow = true;
  cloth.receiveShadow = false;
  root.add(cloth);

  // Weighted vertical spreader bar at the banner's leading edge.
  const barGeo = new THREE.CylinderGeometry(0.11, 0.14, BANNER_H + 0.5, 8);
  const barMat = new THREE.MeshStandardMaterial({ color: 0x2a313a, roughness: 0.5, metalness: 0.6 });
  const bar = new THREE.Mesh(barGeo, barMat);
  bar.frustumCulled = false;
  root.add(bar);

  // Tow cable + V-bridle (3 line segments = 6 vertices).
  const ropePos = new Float32Array(6 * 3);
  const ropeGeo = new THREE.BufferGeometry();
  ropeGeo.setAttribute('position', new THREE.BufferAttribute(ropePos, 3));
  const ropeMat = new THREE.LineBasicMaterial({
    color: 0xe6edf5,
    transparent: true,
    opacity: 0.78,
  });
  const rope = new THREE.LineSegments(ropeGeo, ropeMat);
  rope.frustumCulled = false;
  root.add(rope);

  // World-space path history for the tail hook (index 0 = live tail hook).
  const path = [];
  const spine = Array.from({ length: SEG_X + 1 }, () => new THREE.Vector3());
  let unfurl = 0;

  // Barrel roll & airshow smoke state.
  let rollCooldown = 4 + (hash % 7); // first stunt arrives quickly!
  let rollActive = false;
  let rollElapsed = 0;
  let rollDuration = 2.2;
  let rollDir = hash % 2 === 0 ? 1 : -1;
  let smokeIdx = hash % SMOKE_PALETTE.length;
  let smokeTimer = 0;
  let smokeBurstTail = 0; // brief smoke linger right after roll finishes
  const puffs = [];

  function seedPathBehind(hookPos, heading) {
    path.length = 0;
    const bx = -Math.cos(heading);
    const bz = -Math.sin(heading);
    const count = Math.ceil(MAX_PATH_LEN / SAMPLE_STEP);
    for (let i = 0; i <= count; i++) {
      path.push(new THREE.Vector3(
        hookPos.x + bx * i * SAMPLE_STEP,
        hookPos.y,
        hookPos.z + bz * i * SAMPLE_STEP,
      ));
    }
  }

  // Walk backward along `path` to exact arc-length `dist` from `path[0]`.
  function samplePath(dist, out) {
    if (path.length === 0) return out.set(0, 0, 0);
    if (path.length === 1 || dist <= 0) return out.copy(path[0]);
    let accum = 0;
    for (let i = 0; i < path.length - 1; i++) {
      const a0 = path[i];
      const b0 = path[i + 1];
      const seg = a0.distanceTo(b0);
      if (accum + seg >= dist && seg > 1e-5) {
        const f = (dist - accum) / seg;
        return out.lerpVectors(a0, b0, f);
      }
      accum += seg;
    }
    // Extrapolate past end if path is shorter than requested distance.
    const last = path[path.length - 1];
    const prev = path[path.length - 2];
    _tmp.subVectors(last, prev);
    const len = _tmp.length();
    if (len > 1e-5) _tmp.multiplyScalar((dist - accum) / len);
    else _tmp.set(0, 0, 0);
    return out.copy(last).add(_tmp);
  }

  function spawnSmokePuff(worldPos, colorHex, heading) {
    const mat = new THREE.SpriteMaterial({
      map: smokePuffTex,
      color: colorHex,
      transparent: true,
      opacity: 0.82,
      depthWrite: false,
    });
    const sp = new THREE.Sprite(mat);
    sp.position.copy(worldPos);
    sp.position.x += (Math.random() - 0.5) * 0.7;
    sp.position.y += (Math.random() - 0.5) * 0.7;
    sp.position.z += (Math.random() - 0.5) * 0.7;
    sp.scale.setScalar(2.4);
    sp.renderOrder = 3;
    scene.add(sp);
    puffs.push({
      sprite: sp,
      age: 0,
      life: 4.2 + Math.random() * 1.1,
      baseSize: 2.3 + Math.random() * 0.5,
      grow: 3.1 + Math.random() * 0.7,
      vel: new THREE.Vector3(
        -Math.cos(heading) * 1.2 + (Math.random() - 0.5) * 1.1,
        0.5 + Math.random() * 0.7,
        -Math.sin(heading) * 1.2 + (Math.random() - 0.5) * 1.1,
      ),
    });
  }

  function updatePuffs(dt) {
    for (let i = puffs.length - 1; i >= 0; i--) {
      const p = puffs[i];
      p.age += dt;
      if (p.age >= p.life) {
        scene.remove(p.sprite);
        p.sprite.material.dispose();
        puffs.splice(i, 1);
        continue;
      }
      const k = p.age / p.life;
      p.sprite.position.addScaledVector(p.vel, dt);
      p.vel.multiplyScalar(Math.exp(-0.5 * dt));
      const growCurve = 1 - (1 - k) ** 2.4;
      p.sprite.scale.setScalar(p.baseSize * (1 + p.grow * growCurve));
      p.sprite.material.opacity = 0.82 * (1 - k) ** 1.35;
    }
  }

  return {
    reset() {
      path.length = 0;
    },

    /**
     * Advance stunt timer and return extra { roll, pitch } radians to layer on
     * top of the aircraft's normal flight attitude. Call BEFORE setting group
     * rotation each frame.
     */
    updateStunt(entry, dt, airborne) {
      const canStunt = airborne
        && (entry.state === 'cruising' || entry.state === 'holding')
        && (entry.altitude || 0) > 40;

      if (rollActive) {
        // Only the ground cuts a roll short. Dropping it mid-air when the state
        // changes would snap the wings level in a single frame.
        if (!airborne) {
          rollActive = false;
          rollElapsed = 0;
          smokeBurstTail = 0;
          return { roll: 0, pitch: 0 };
        }
        rollElapsed += dt;
        const t = Math.min(1, rollElapsed / rollDuration);
        const eased = smootherstep(t);
        const extraRoll = eased * Math.PI * 2 * rollDir;
        // Subtle celebratory nose-up entry bump.
        const extraPitch = -0.14 * Math.sin(t * Math.PI);
        if (rollElapsed >= rollDuration) {
          rollActive = false;
          rollElapsed = 0;
          smokeBurstTail = 0.55;
          rollCooldown = 14 + Math.random() * 11;
          rollDir = Math.random() < 0.5 ? 1 : -1;
        }
        return { roll: extraRoll, pitch: extraPitch };
      }

      if (smokeBurstTail > 0) smokeBurstTail = Math.max(0, smokeBurstTail - dt);

      if (canStunt) {
        rollCooldown -= dt;
        if (rollCooldown <= 0) {
          rollActive = true;
          rollElapsed = 0;
          rollDuration = 2.1 + Math.random() * 0.45;
          smokeIdx = (smokeIdx + 1) % SMOKE_PALETTE.length;
        }
      }
      return { roll: 0, pitch: 0 };
    },

    /**
     * Update world-space towed banner & airshow smoke puffs. Call AFTER
     * entry.group position & rotation have been updated for this frame.
     */
    updateVisuals(entry, dt, now, airborne) {
      updatePuffs(dt);

      const alt = entry.altitude || 0;
      const showBanner = airborne && alt > 8;
      const targetUnfurl = showBanner ? 1 : 0;
      unfurl += (targetUnfurl - unfurl) * (1 - Math.exp(-3.2 * dt));

      entry.group.updateMatrixWorld();

      // Emit coloured airshow smoke during barrel rolls + brief flourish tail.
      if ((rollActive || smokeBurstTail > 0) && airborne) {
        smokeTimer += dt;
        if (smokeTimer >= 0.042) {
          smokeTimer = 0;
          _smokeWorld.copy(SMOKE_LOCAL).applyMatrix4(entry.group.matrixWorld);
          spawnSmokePuff(_smokeWorld, SMOKE_PALETTE[smokeIdx], entry.heading);
        }
      } else {
        smokeTimer = 0;
      }

      if (unfurl < 0.02) {
        root.visible = false;
        path.length = 0;
        return;
      }
      root.visible = true;

      // Live world-space tail hook.
      _hookWorld.copy(TAIL_HOOK_LOCAL).applyMatrix4(entry.group.matrixWorld);

      if (path.length === 0 || path[0].distanceToSquared(_hookWorld) > 14400) {
        seedPathBehind(_hookWorld, entry.heading);
      } else {
        path[0].copy(_hookWorld);
        if (path.length < 2 || path[0].distanceTo(path[1]) >= SAMPLE_STEP) {
          path.splice(1, 0, _hookWorld.clone());
          // Keep polyline bounded to MAX_PATH_LEN.
          let total = 0;
          for (let i = 0; i < path.length - 1; i++) {
            total += path[i].distanceTo(path[i + 1]);
            if (total > MAX_PATH_LEN) {
              path.length = i + 2;
              break;
            }
          }
        }
      }

      // Sample banner spine from front (i=0) to rear (i=SEG_X).
      const activeRope = ROPE_LEN * unfurl;
      const activeLen = BANNER_LEN * unfurl;
      const activeH = BANNER_H * unfurl;
      for (let i = 0; i <= SEG_X; i++) {
        const s = i / SEG_X;
        samplePath(activeRope + s * activeLen, spine[i]);
      }

      // Update spreader bar at leading edge spine[0].
      bar.position.copy(spine[0]);
      bar.scale.set(1, unfurl, 1);

      // Update tow cable + V-bridle connecting tail hook -> bridle apex -> spreader bar top/bottom.
      const bridleApex = samplePath(Math.max(0, activeRope - BRIDLE_LEN * unfurl), _tmp);
      const halfH = activeH * 0.5;
      const setSeg = (idx, p0, p1) => {
        const o = idx * 6;
        ropePos[o] = p0.x; ropePos[o + 1] = p0.y; ropePos[o + 2] = p0.z;
        ropePos[o + 3] = p1.x; ropePos[o + 4] = p1.y; ropePos[o + 5] = p1.z;
      };
      setSeg(0, _hookWorld, bridleApex);
      ropePos[6] = bridleApex.x; ropePos[7] = bridleApex.y; ropePos[8] = bridleApex.z;
      ropePos[9] = spine[0].x; ropePos[10] = spine[0].y + halfH; ropePos[11] = spine[0].z;
      ropePos[12] = bridleApex.x; ropePos[13] = bridleApex.y; ropePos[14] = bridleApex.z;
      ropePos[15] = spine[0].x; ropePos[16] = spine[0].y - halfH; ropePos[17] = spine[0].z;
      ropeGeo.attributes.position.needsUpdate = true;

      // Update double-sheet cloth vertices with travelling fabric flutter.
      const posAttr = clothGeo.attributes.position;
      const vertsPerSheet = (SEG_X + 1) * (SEG_Y + 1);
      for (let i = 0; i <= SEG_X; i++) {
        const s = i / SEG_X;
        const p = spine[i];
        // Forward tangent points from rear toward nose.
        if (i === 0) _fwd.subVectors(spine[0], spine[1]);
        else if (i === SEG_X) _fwd.subVectors(spine[SEG_X - 1], spine[SEG_X]);
        else _fwd.subVectors(spine[i - 1], spine[i + 1]);
        _fwd.y = 0;
        if (_fwd.lengthSq() < 1e-6) _fwd.set(Math.cos(entry.heading), 0, Math.sin(entry.heading));
        else _fwd.normalize();

        // Starboard normal = F x Up.
        _side.crossVectors(_fwd, _up).normalize();

        // Smooth ripple growing toward the free trailing edge.
        const wave = (
          Math.sin(s * 10.5 - now * 9.5 + phase) * 0.72
          + Math.sin(s * 18.0 - now * 14.5 + phase * 1.7) * 0.22
        ) * (0.06 + 0.94 * s * s) * unfurl;
        const droop = -0.35 * s * s * unfurl;

        for (let r = 0; r <= SEG_Y; r++) {
          const v = r / SEG_Y;
          const yOff = (v - 0.5) * activeH + droop;
          // Slight vertical ripple phase shift makes fabric twist organically.
          const edgeWave = wave + Math.sin(s * 9.0 - now * 8.2 + phase + v * 1.1) * 0.12 * s * unfurl;
          const cx = p.x + _side.x * edgeWave;
          const cy = p.y + yOff;
          const cz = p.z + _side.z * edgeWave;

          const off = i * (SEG_Y + 1) + r;
          // Sheet 0 (+Side) and Sheet 1 (-Side) separated by tiny thickness.
          posAttr.setXYZ(off, cx + _side.x * 0.03, cy, cz + _side.z * 0.03);
          posAttr.setXYZ(vertsPerSheet + off, cx - _side.x * 0.03, cy, cz - _side.z * 0.03);
        }
      }
      posAttr.needsUpdate = true;
      clothGeo.computeVertexNormals();

      // Fade banner cleanly with distant departing haze.
      if (entry.state === 'departing') {
        const dist = Math.hypot(entry.group.position.x, entry.group.position.z);
        const fade = Math.max(0, Math.min(1, 1 - (dist - 850) / 650));
        clothMat.transparent = fade < 1;
        clothMat.opacity = fade;
        ropeMat.opacity = 0.78 * fade;
      } else if (clothMat.transparent) {
        clothMat.transparent = false;
        clothMat.opacity = 1;
        ropeMat.opacity = 0.78;
      }
    },

    dispose() {
      scene.remove(root);
      clothGeo.dispose();
      clothMat.dispose();
      barGeo.dispose();
      barMat.dispose();
      ropeGeo.dispose();
      ropeMat.dispose();
      for (const p of puffs) {
        scene.remove(p.sprite);
        p.sprite.material.dispose();
      }
      puffs.length = 0;
    },
  };
}
