// Retractable landing-gear geometry and frame-rate-independent animation
// for large aircraft (frontier jets and widebody aerial tankers).
//
// Gear phase convention:
//   0 = fully retracted (struts stowed inside fuselage/wing bays, doors shut, group hidden)
//   1 = fully extended  (doors open, struts locked down, wheels at ground-contact height)

import * as THREE from './vendor/three.module.js';

export const LIFTOFF_RETRACT_ALT = 6;    // retract once safely airborne after rotation
export const APPROACH_EXTEND_ALT = 72;   // extend on landing approach before final
export const APPROACH_RETRACT_ALT = 82;  // hysteresis ceiling for go-arounds
export const GEAR_RETRACT_DURATION = 1.75; // seconds (1 -> 0)
export const GEAR_EXTEND_DURATION = 1.85;  // seconds (0 -> 1)

const GROUND_STATES = new Set(['parked', 'ready', 'taxi', 'landed']);
const AIRBORNE_CLEAN_STATES = new Set(['cruising', 'holding', 'refueling']);

function clamp01(x) {
  return x <= 0 ? 0 : (x >= 1 ? 1 : x);
}

function remap(edge0, edge1, x) {
  return clamp01((x - edge0) / (edge1 - edge0));
}

function smoothstep(edge0, edge1, x) {
  const t = remap(edge0, edge1, x);
  return t * t * (3 - 2 * t);
}

function easeInOutCubic(t) {
  const c = clamp01(t);
  return c < 0.5 ? 4 * c * c * c : 1 - ((-2 * c + 2) ** 3) / 2;
}

/**
 * Decide desired landing-gear target (1 = down, 0 = up) from server state
 * and altitude. Pass `prevTarget` (0, 1, or null/undefined on spawn) for
 * approach hysteresis so minor altitude fluctuations never chatter.
 */
export function gearTargetFor(state, altitude = 0, prevTarget = null) {
  const alt = Number.isFinite(altitude) ? altitude : 0;
  if (GROUND_STATES.has(state)) return 1;
  if (AIRBORNE_CLEAN_STATES.has(state)) return 0;
  if (state === 'takeoff' || state === 'departing') {
    return alt <= LIFTOFF_RETRACT_ALT ? 1 : 0;
  }
  if (state === 'landing') {
    if (prevTarget === 1) return alt <= APPROACH_RETRACT_ALT ? 1 : 0;
    return alt <= APPROACH_EXTEND_ALT ? 1 : 0;
  }
  return alt <= LIFTOFF_RETRACT_ALT ? 1 : 0;
}

/**
 * Advance `phase` toward `target` (0 or 1) at a constant rate independent of
 * frame rate. `dt` is clamped so background-tab pauses never jump wildly.
 */
export function stepGearPhase(phase, target, dt) {
  const step = Math.min(Math.max(dt || 0, 0), 0.1);
  if (step === 0 || phase === target) return clamp01(phase);
  const duration = target < phase ? GEAR_RETRACT_DURATION : GEAR_EXTEND_DURATION;
  const maxDelta = step / duration;
  const diff = target - phase;
  if (Math.abs(diff) <= maxDelta) return clamp01(target);
  return clamp01(phase + Math.sign(diff) * maxDelta);
}

/**
 * Map linear `phase` in [0, 1] into choreographed, eased door and strut curves:
 *   - Extending (0 -> 1): bay doors open first, then nose & main struts swing
 *     down with a subtle realistic stagger and lock at 1.
 *   - Retracting (1 -> 0): struts fold cleanly into the bays first, then bay
 *     doors swing flush and the entire gear subtree hides at phase ~ 0.
 */
export function computeGearKinematics(phase) {
  const p = clamp01(phase);
  const visible = p > 0.0005;
  const doorOpen = smoothstep(0.0, 0.30, p);
  const noseStrut = easeInOutCubic(remap(0.14, 0.90, p));
  const mainLeftStrut = easeInOutCubic(remap(0.18, 0.97, p));
  const mainRightStrut = easeInOutCubic(remap(0.21, 1.00, p));
  return {
    visible,
    doorOpen,
    noseStrut,
    mainLeftStrut,
    mainRightStrut,
  };
}

/* ----------------------------------------------------- geometry helpers */

function makeRod(mat, a, b, radius, segs = 8) {
  const va = new THREE.Vector3(...a);
  const vb = new THREE.Vector3(...b);
  const mesh = new THREE.Mesh(
    new THREE.CylinderGeometry(radius, radius, va.distanceTo(vb), segs),
    mat,
  );
  mesh.position.copy(va).add(vb).multiplyScalar(0.5);
  mesh.quaternion.setFromUnitVectors(
    new THREE.Vector3(0, 1, 0),
    vb.clone().sub(va).normalize(),
  );
  return mesh;
}

// Wheel oriented with its axle along X; positive rotation.x rolls forward (+Z).
function makeGearWheel(kit, radius, width) {
  const spinGroup = new THREE.Group();
  const tyre = new THREE.Mesh(new THREE.CylinderGeometry(radius, radius, width, 18), kit.rubber);
  tyre.rotation.z = Math.PI / 2;
  spinGroup.add(tyre);

  const hub = new THREE.Mesh(
    new THREE.CylinderGeometry(radius * 0.55, radius * 0.55, width * 1.04, 12),
    kit.metal,
  );
  hub.rotation.z = Math.PI / 2;
  spinGroup.add(hub);

  // Subtle hub spoke so wheel rolling on tarmac & spin-down after liftoff read cleanly.
  const spoke = new THREE.Mesh(
    new THREE.BoxGeometry(width * 1.08, radius * 0.78, radius * 0.14),
    kit.dark,
  );
  spinGroup.add(spoke);
  return spinGroup;
}

function makeDoorPanel(mat, hingePos, panelOffset, size, openAngleZ) {
  const pivot = new THREE.Group();
  pivot.position.set(...hingePos);
  const panel = new THREE.Mesh(new THREE.BoxGeometry(...size), mat);
  panel.position.set(...panelOffset);
  pivot.add(panel);
  return { pivot, openAngleZ };
}

function buildRetractableGear(kit, spec) {
  const root = new THREE.Group();
  root.name = 'gear';

  const wheels = [];
  const doors = [];

  /* ---- nose gear (retracts forward & upward into nose bay) ---- */
  const n = spec.nose;
  const nosePivot = new THREE.Group();
  nosePivot.name = 'noseGear';
  nosePivot.position.set(...n.pivot);

  // Upper oleo housing & forward-angled drag brace.
  nosePivot.add(makeRod(kit.metal, [0, 0, 0], [0, -n.upperLen, 0], n.upperR));
  nosePivot.add(makeRod(kit.metal, [0, -n.braceTopY, n.braceTopZ], [0, -n.braceBotY, 0], n.braceR, 6));

  // Lower telescoping oleo piston + axle + twin wheels.
  const noseLower = new THREE.Group();
  noseLower.name = 'noseGearLower';
  noseLower.add(makeRod(kit.metal, [0, -n.upperLen * 0.82, 0], [0, -n.axleY, 0], n.lowerR));
  noseLower.add(makeRod(kit.metal, [-n.axleHalfW, -n.axleY, 0], [n.axleHalfW, -n.axleY, 0], n.axleR, 6));
  for (const sx of [-1, 1]) {
    const w = makeGearWheel(kit, n.wheelR, n.wheelW);
    w.position.set(sx * n.wheelOffsetX, -n.axleY, 0);
    noseLower.add(w);
    wheels.push({ group: w, radius: n.wheelR });
  }
  nosePivot.add(noseLower);
  root.add(nosePivot);

  // Twin hinged nose-bay doors flanking the nose wheel well.
  const nd = n.door;
  const noseDoorMat = kit.belly || kit.plain;
  for (const side of [1, -1]) {
    const door = makeDoorPanel(
      noseDoorMat,
      [side * nd.hingeX, nd.hingeY, nd.z],
      [-side * (nd.width * 0.5), 0, 0],
      [nd.width, nd.thickness, nd.length],
      side * nd.openAngle,
    );
    door.pivot.name = side > 0 ? 'noseDoorLeft' : 'noseDoorRight';
    root.add(door.pivot);
    doors.push(door);
  }

  /* ---- main gear left (+1) & right (-1) (retract inward into wing-box belly) ---- */
  const m = spec.main;
  const mainRig = {};
  for (const side of [1, -1]) {
    const pivot = new THREE.Group();
    pivot.name = side > 0 ? 'mainGearLeft' : 'mainGearRight';
    pivot.position.set(side * m.pivotX, m.pivotY, m.pivotZ);

    // Upper oleo strut.
    pivot.add(makeRod(kit.metal, [0, 0, 0], [0, -m.upperLen, 0], m.upperR));

    // Folding inboard side brace (folds flat alongside the strut as it retracts).
    const bracePivot = new THREE.Group();
    bracePivot.name = side > 0 ? 'mainBraceLeft' : 'mainBraceRight';
    bracePivot.position.set(0, -m.braceAttachY, 0);
    bracePivot.add(makeRod(kit.metal, [0, 0, 0], [0, m.braceLen, 0], m.braceR, 6));
    pivot.add(bracePivot);

    // Outer strut-mounted gear door.
    const od = m.outerDoor;
    const outerDoor = new THREE.Mesh(
      new THREE.BoxGeometry(od.thickness, od.height, od.length),
      kit.wing,
    );
    outerDoor.position.set(side * od.offsetX, -od.offsetY, 0);
    pivot.add(outerDoor);

    // Lower telescoping oleo piston + axle / tandem bogie + wheels.
    const lower = new THREE.Group();
    lower.name = side > 0 ? 'mainGearLowerLeft' : 'mainGearLowerRight';
    lower.add(makeRod(kit.metal, [0, -m.upperLen * 0.82, 0], [0, -m.axleY, 0], m.lowerR));

    let bogiePivot = null;
    if (m.bogie) {
      bogiePivot = new THREE.Group();
      bogiePivot.position.set(0, -m.axleY, 0);
      bogiePivot.add(makeRod(kit.metal, [0, 0, -m.bogie.halfLen], [0, 0, m.bogie.halfLen], m.axleR * 1.15, 8));
      for (const sz of [-1, 1]) {
        const az = sz * m.bogie.axleZ;
        bogiePivot.add(makeRod(kit.metal, [-m.axleHalfW, 0, az], [m.axleHalfW, 0, az], m.axleR, 6));
        for (const sx of [-1, 1]) {
          const w = makeGearWheel(kit, m.wheelR, m.wheelW);
          w.position.set(sx * m.wheelOffsetX, 0, az);
          bogiePivot.add(w);
          wheels.push({ group: w, radius: m.wheelR });
        }
      }
      lower.add(bogiePivot);
    } else {
      lower.add(makeRod(kit.metal, [-m.axleHalfW, -m.axleY, 0], [m.axleHalfW, -m.axleY, 0], m.axleR, 6));
      for (const sx of [-1, 1]) {
        const w = makeGearWheel(kit, m.wheelR, m.wheelW);
        w.position.set(sx * m.wheelOffsetX, -m.axleY, 0);
        lower.add(w);
        wheels.push({ group: w, radius: m.wheelR });
      }
    }
    pivot.add(lower);
    root.add(pivot);

    // Inboard belly bay door hinged near the wing-box centerline.
    const md = m.bellyDoor;
    const bellyDoor = makeDoorPanel(
      kit.wing,
      [side * md.hingeX, md.hingeY, m.pivotZ],
      [side * (md.width * 0.5), 0, 0],
      [md.width, md.thickness, md.length],
      -side * md.openAngle,
    );
    bellyDoor.pivot.name = side > 0 ? 'mainDoorLeft' : 'mainDoorRight';
    root.add(bellyDoor.pivot);
    doors.push(bellyDoor);

    const slot = {
      side,
      pivot,
      lower,
      bracePivot,
      bogiePivot,
      baseY: m.pivotY,
      liftY: m.retractLiftY,
      compressY: m.retractCompressY,
      retractAngle: -side * m.retractAngle,
      braceOpenAngle: side * m.braceOpenAngle,
      bogieTilt: m.bogie ? m.bogie.retractTilt : 0,
    };
    if (side > 0) mainRig.left = slot;
    else mainRig.right = slot;
  }

  const rig = {
    group: root,
    nose: {
      pivot: nosePivot,
      lower: noseLower,
      baseY: n.pivot[1],
      liftY: n.retractLiftY,
      compressY: n.retractCompressY,
      retractAngle: -n.retractAngle,
    },
    mainLeft: mainRig.left,
    mainRight: mainRig.right,
    doors,
    wheels,
    wheelSpinRate: 0,
  };

  // Start in the fully extended pose so finish() measures the exact wheelDrop.
  applyLandingGearPose(rig, 1);
  return rig;
}

/**
 * Retractable tricycle gear for the frontier narrow-body twin jet (makeJetMesh).
 * Extended lowest point matches y = -3.98 so built.wheelDrop stays unchanged.
 */
export function buildJetLandingGear(kit, { wingY = -0.95 } = {}) {
  return buildRetractableGear(kit, {
    nose: {
      pivot: [0, -1.40, 9.20],
      upperLen: 1.25,
      upperR: 0.13,
      lowerR: 0.095,
      axleY: 2.10, // world y = -3.50; wheel bottom = -3.88
      axleHalfW: 0.27,
      axleR: 0.065,
      wheelOffsetX: 0.22,
      wheelR: 0.38,
      wheelW: 0.25,
      braceTopY: 0.20,
      braceTopZ: 0.70,
      braceBotY: 1.15,
      braceR: 0.05,
      retractAngle: 1.66,
      retractLiftY: 0.55,
      retractCompressY: 0.48,
      door: {
        hingeX: 0.48,
        hingeY: -1.58,
        z: 9.95,
        width: 0.46,
        thickness: 0.05,
        length: 2.30,
        openAngle: 1.48,
      },
    },
    main: {
      pivotX: 2.20,
      pivotY: wingY - 0.15, // -1.10
      pivotZ: -1.20,
      upperLen: 1.35,
      upperR: 0.145,
      lowerR: 0.105,
      axleY: 2.30, // world y = -3.40; wheel bottom = -3.98 (exact match)
      axleHalfW: 0.32,
      axleR: 0.09,
      wheelOffsetX: 0.28,
      wheelR: 0.58,
      wheelW: 0.34,
      braceAttachY: 1.25,
      braceLen: 1.35,
      braceR: 0.06,
      braceOpenAngle: 0.78,
      retractAngle: 1.62,
      retractLiftY: 0.22,
      retractCompressY: 0.62,
      outerDoor: {
        offsetX: 0.16,
        offsetY: 0.58,
        thickness: 0.06,
        height: 1.05,
        length: 0.78,
      },
      bellyDoor: {
        hingeX: 0.20,
        hingeY: -1.56,
        width: 1.15,
        thickness: 0.06,
        length: 1.55,
        openAngle: 1.48,
      },
    },
  });
}

/**
 * Retractable heavy widebody gear for the 4-engine aerial tanker (makeTankerMesh),
 * featuring tandem 4-wheel main bogies and twin nose wheels.
 */
export function buildTankerLandingGear(kit, { wingY = -1.10 } = {}) {
  return buildRetractableGear(kit, {
    nose: {
      pivot: [0, -1.75, 12.20],
      upperLen: 1.20,
      upperR: 0.15,
      lowerR: 0.11,
      axleY: 2.02, // world y = -3.77; wheel bottom = -4.22
      axleHalfW: 0.34,
      axleR: 0.08,
      wheelOffsetX: 0.28,
      wheelR: 0.45,
      wheelW: 0.28,
      braceTopY: 0.22,
      braceTopZ: 0.82,
      braceBotY: 1.15,
      braceR: 0.06,
      retractAngle: 1.66,
      retractLiftY: 0.55,
      retractCompressY: 0.46,
      door: {
        hingeX: 0.58,
        hingeY: -2.02,
        z: 13.00,
        width: 0.56,
        thickness: 0.06,
        length: 2.70,
        openAngle: 1.48,
      },
    },
    main: {
      pivotX: 3.10,
      pivotY: wingY - 0.15, // -1.25
      pivotZ: -1.50,
      upperLen: 1.35,
      upperR: 0.18,
      lowerR: 0.13,
      axleY: 2.35, // world y = -3.60; wheel bottom = -4.22
      axleHalfW: 0.38,
      axleR: 0.10,
      wheelOffsetX: 0.34,
      wheelR: 0.62,
      wheelW: 0.34,
      braceAttachY: 1.30,
      braceLen: 1.60,
      braceR: 0.075,
      braceOpenAngle: 0.82,
      retractAngle: 1.62,
      retractLiftY: 0.25,
      retractCompressY: 0.68,
      bogie: {
        halfLen: 0.65,
        axleZ: 0.58,
        retractTilt: -0.35,
      },
      outerDoor: {
        offsetX: 0.20,
        offsetY: 0.62,
        thickness: 0.07,
        height: 1.18,
        length: 0.95,
      },
      bellyDoor: {
        hingeX: 0.26,
        hingeY: -2.06,
        width: 1.55,
        thickness: 0.07,
        length: 2.35,
        openAngle: 1.48,
      },
    },
  });
}

/* ----------------------------------------------------- per-frame driver */

export function applyLandingGearPose(rig, phase) {
  const k = computeGearKinematics(phase);
  rig.group.visible = k.visible;
  if (!k.visible) return;

  for (const door of rig.doors) {
    door.pivot.rotation.z = door.openAngleZ * k.doorOpen;
  }

  // Nose strut swings forward/up into nose bay and telescopes slightly as it retracts.
  const n = rig.nose;
  const noseUp = 1 - k.noseStrut;
  n.pivot.rotation.x = n.retractAngle * noseUp;
  n.pivot.position.y = n.baseY + n.liftY * noseUp;
  n.lower.position.y = n.compressY * noseUp;
  n.pivot.visible = k.noseStrut > 0.001;

  // Main struts swing inward into wing-box belly with folding side braces & bogie tilt.
  for (const [slot, strutT] of [[rig.mainLeft, k.mainLeftStrut], [rig.mainRight, k.mainRightStrut]]) {
    const up = 1 - strutT;
    slot.pivot.rotation.z = slot.retractAngle * up;
    slot.pivot.position.y = slot.baseY + slot.liftY * up;
    slot.lower.position.y = slot.compressY * up;
    slot.bracePivot.rotation.z = slot.braceOpenAngle * strutT;
    if (slot.bogiePivot) {
      slot.bogiePivot.rotation.x = slot.bogieTilt * up;
    }
    slot.pivot.visible = strutT > 0.001;
  }
}

/**
 * Initialize a newly-built aircraft entry to the exact gear end-state for its
 * spawn state & altitude so aircraft spawned mid-flight or on ground never snap.
 */
export function initLandingGear(entry, state, altitude) {
  if (!entry.landingGear) return;
  const initial = gearTargetFor(state, altitude, null);
  entry.gearTarget = initial;
  entry.gearPhase = initial;
  applyLandingGearPose(entry.landingGear, initial);
}

/**
 * Advance landing-gear retraction/extension and ground wheel roll for one frame.
 */
export function updateLandingGear(entry, dt) {
  const rig = entry.landingGear;
  if (!rig) return;
  const alt = entry.altitude || 0;
  const target = gearTargetFor(entry.state, alt, entry.gearTarget);
  entry.gearTarget = target;
  entry.gearPhase = stepGearPhase(entry.gearPhase ?? target, target, dt);
  applyLandingGearPose(rig, entry.gearPhase);

  if (!rig.group.visible) {
    rig.wheelSpinRate = 0;
    return;
  }

  // Spin wheels proportionally to ground speed on tarmac; brake smoothly to a stop after liftoff.
  const step = Math.min(Math.max(dt || 0, 0), 0.1);
  const onGround = entry.gearPhase > 0.95 && alt <= 0.1;
  const speed = entry.speed || 0;
  const scale = entry.scale || 1;
  if (onGround && speed > 0.05) {
    rig.wheelSpinRate = speed / scale;
  } else {
    rig.wheelSpinRate *= Math.exp(-3.5 * step);
    if (rig.wheelSpinRate < 0.02) rig.wheelSpinRate = 0;
  }
  if (rig.wheelSpinRate > 0) {
    for (const w of rig.wheels) {
      w.group.rotation.x += (rig.wheelSpinRate / w.radius) * step;
    }
  }
}
