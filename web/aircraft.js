// Aircraft meshes. Every builder returns a group whose nose points along +Z.

import * as THREE from './vendor/three.module.js';

// Little private plane (single prop): local models. Points along +Z.
export function makeAgentMesh() {
  const group = new THREE.Group();
  const mat = new THREE.MeshLambertMaterial({ color: 0xffffff });
  const darkMat = new THREE.MeshLambertMaterial({ color: 0x222831 });
  const propMat = new THREE.MeshLambertMaterial({
    color: 0x222831, transparent: true, opacity: 0.35, side: THREE.DoubleSide,
  });

  const fuselage = new THREE.Mesh(new THREE.CapsuleGeometry(1.3, 5, 8, 14), mat);
  fuselage.rotation.x = Math.PI / 2;
  fuselage.position.z = 0.6;
  group.add(fuselage);

  const boom = new THREE.Mesh(new THREE.ConeGeometry(1.3, 3, 14), mat);
  boom.rotation.x = -Math.PI / 2;
  boom.position.z = -4.6;
  group.add(boom);

  const cowl = new THREE.Mesh(new THREE.CylinderGeometry(1.0, 1.45, 2, 14), mat);
  cowl.rotation.x = Math.PI / 2;
  cowl.position.z = 4.9;
  group.add(cowl);

  const spinner = new THREE.Mesh(new THREE.ConeGeometry(0.55, 1.3, 12), mat);
  spinner.rotation.x = Math.PI / 2;
  spinner.position.z = 6.55;
  group.add(spinner);

  const prop = new THREE.Group();
  const disc = new THREE.Mesh(new THREE.CircleGeometry(2.3, 16), propMat);
  const blades = new THREE.Mesh(new THREE.BoxGeometry(0.28, 4.4, 0.14), darkMat);
  prop.add(disc, blades);
  prop.position.z = 7.05;
  prop.name = 'prop';
  group.add(prop);

  const cabin = new THREE.Mesh(new THREE.BoxGeometry(1.8, 1.1, 2.9), mat);
  cabin.position.set(0, 1.35, 0.5);
  group.add(cabin);

  for (const side of [1, -1]) {
    const wing = new THREE.Mesh(new THREE.BoxGeometry(6.4, 0.42, 2.6), mat);
    wing.position.set(side * 4.3, 2.05, -0.4);
    wing.rotation.z = side * 0.16;
    group.add(wing);
  }

  for (const side of [1, -1]) {
    const win = new THREE.Mesh(new THREE.BoxGeometry(0.16, 0.55, 2.3), darkMat);
    win.position.set(side * 0.92, 1.1, 0.7);
    group.add(win);
  }
  const glass = new THREE.Mesh(new THREE.BoxGeometry(1.5, 0.6, 0.18), darkMat);
  glass.position.set(0, 1.55, 2.05);
  glass.rotation.x = -0.55;
  group.add(glass);

  for (const side of [1, -1]) {
    const strut = new THREE.Mesh(new THREE.BoxGeometry(0.22, 2.1, 0.22), darkMat);
    strut.position.set(side * 1.95, 1.0, 0.35);
    strut.rotation.z = side * -0.75;
    group.add(strut);
  }

  const tail = new THREE.Mesh(new THREE.BoxGeometry(5.6, 0.36, 1.9), mat);
  tail.position.set(0, 0.55, -4.8);
  group.add(tail);

  const fin = new THREE.Mesh(new THREE.BoxGeometry(0.36, 2.3, 1.8), mat);
  fin.position.set(0, 1.55, -5.15);
  group.add(fin);

  for (const side of [1, -1]) {
    const leg = new THREE.Mesh(new THREE.CylinderGeometry(0.1, 0.1, 1.1, 8), darkMat);
    leg.position.set(side * 1.45, -1.2, 0.4);
    group.add(leg);

    const wheel = new THREE.Mesh(new THREE.CylinderGeometry(0.52, 0.52, 0.4, 14), darkMat);
    wheel.rotation.z = Math.PI / 2;
    wheel.position.set(side * 1.55, -1.72, 0.4);
    group.add(wheel);

    const spat = new THREE.Mesh(new THREE.CapsuleGeometry(0.4, 1.6, 4, 10), mat);
    spat.rotation.x = Math.PI / 2;
    spat.position.set(side * 1.75, -1.7, 0.4);
    group.add(spat);
  }

  const tailLeg = new THREE.Mesh(new THREE.CylinderGeometry(0.07, 0.07, 0.5, 8), darkMat);
  tailLeg.position.set(0, -0.55, -5.4);
  group.add(tailLeg);

  const tailWheel = new THREE.Mesh(new THREE.SphereGeometry(0.28, 10, 8), darkMat);
  tailWheel.position.set(0, -0.85, -5.4);
  group.add(tailWheel);

  return { group, tintMaterials: [mat], disposeMaterials: [mat, darkMat, propMat], scale: 0.9, wheelDrop: 2.0 };
}

// Big twin-engine jet: frontier models. Points along +Z.
export function makeJetMesh() {
  const group = new THREE.Group();
  const mat = new THREE.MeshLambertMaterial({ color: 0xffffff });
  const darkMat = new THREE.MeshLambertMaterial({ color: 0x222831 });

  const fuselage = new THREE.Mesh(new THREE.CapsuleGeometry(1.7, 18, 6, 12), mat);
  fuselage.rotation.x = Math.PI / 2;
  fuselage.position.z = -1.5;
  group.add(fuselage);

  const nose = new THREE.Mesh(new THREE.ConeGeometry(1.7, 4, 12), mat);
  nose.rotation.x = Math.PI / 2;
  nose.position.z = 10.5;
  group.add(nose);

  const cockpit = new THREE.Mesh(new THREE.BoxGeometry(2.2, 0.7, 2.0), darkMat);
  cockpit.position.set(0, 1.0, 7.6);
  group.add(cockpit);

  for (const side of [1, -1]) {
    const wing = new THREE.Mesh(new THREE.BoxGeometry(12.5, 0.5, 3.8), mat);
    wing.position.set(side * 6.2, -0.7, -1.8);
    wing.rotation.y = side * -0.35;
    group.add(wing);

    const engine = new THREE.Mesh(new THREE.CylinderGeometry(1.05, 1.05, 3.4, 10), darkMat);
    engine.rotation.x = Math.PI / 2;
    engine.position.set(side * 5.0, -1.9, 0.4);
    group.add(engine);
  }

  const tail = new THREE.Mesh(new THREE.BoxGeometry(8.6, 0.4, 2.4), mat);
  tail.position.set(0, 1.1, -9.5);
  group.add(tail);

  const fin = new THREE.Mesh(new THREE.BoxGeometry(0.5, 5.2, 3.4), mat);
  fin.position.set(0, 3.2, -10.0);
  fin.rotation.x = -0.25;
  group.add(fin);

  return { group, tintMaterials: [mat], disposeMaterials: [mat, darkMat], scale: 0.74, wheelDrop: 2.2 };
}

// Sleek delta-wing fighter jet: sub-agent wingman! Points along +Z.
export function makeFighterMesh() {
  const group = new THREE.Group();
  const mat = new THREE.MeshLambertMaterial({ color: 0x7ce8ff });
  const darkMat = new THREE.MeshLambertMaterial({ color: 0x1b2430 });
  const canopyMat = new THREE.MeshLambertMaterial({ color: 0x1e3852 });
  const burnerMat = new THREE.MeshBasicMaterial({ color: 0xff9d2e });

  // Needle nose + sleek fuselage.
  const nose = new THREE.Mesh(new THREE.ConeGeometry(1.05, 5.5, 8), mat);
  nose.rotation.x = Math.PI / 2;
  nose.position.z = 7.2;
  group.add(nose);

  const body = new THREE.Mesh(new THREE.CylinderGeometry(1.05, 1.25, 10.5, 8), mat);
  body.rotation.x = Math.PI / 2;
  body.position.z = -0.7;
  group.add(body);

  // Bubble canopy.
  const canopy = new THREE.Mesh(new THREE.CapsuleGeometry(0.68, 2.4, 6, 10), canopyMat);
  canopy.rotation.x = Math.PI / 2;
  canopy.position.set(0, 0.95, 2.2);
  group.add(canopy);

  // Swept delta wings (extruded flat triangle pair).
  const wingShape = new THREE.Shape();
  wingShape.moveTo(0, 3.2);
  wingShape.lineTo(6.6, -4.8);
  wingShape.lineTo(6.0, -5.6);
  wingShape.lineTo(0, -4.6);
  wingShape.lineTo(-6.0, -5.6);
  wingShape.lineTo(-6.6, -4.8);
  wingShape.closePath();
  const wingGeo = new THREE.ExtrudeGeometry(wingShape, { depth: 0.32, bevelEnabled: false });
  wingGeo.rotateX(Math.PI / 2); // XY -> XZ, +Y maps to +Z
  const wings = new THREE.Mesh(wingGeo, mat);
  wings.position.set(0, 0.1, 0.2);
  group.add(wings);

  // Forward canards for aggressive fighter silhouette.
  for (const side of [1, -1]) {
    const canard = new THREE.Mesh(new THREE.BoxGeometry(2.4, 0.22, 1.2), mat);
    canard.position.set(side * 1.9, 0.15, 4.0);
    canard.rotation.y = side * -0.4;
    group.add(canard);
  }

  // Twin canted vertical tail fins (F-22 / F-18 style).
  for (const side of [1, -1]) {
    const fin = new THREE.Mesh(new THREE.BoxGeometry(0.28, 3.2, 2.6), mat);
    fin.position.set(side * 1.4, 1.5, -4.6);
    fin.rotation.z = side * -0.32;
    fin.rotation.x = -0.25;
    group.add(fin);
  }

  // Twin nozzles + glowing afterburners.
  for (const side of [1, -1]) {
    const nozzle = new THREE.Mesh(new THREE.CylinderGeometry(0.42, 0.52, 1.1, 8), darkMat);
    nozzle.rotation.x = Math.PI / 2;
    nozzle.position.set(side * 0.55, -0.05, -6.2);
    group.add(nozzle);

    const flame = new THREE.Mesh(new THREE.ConeGeometry(0.38, 2.2, 8), burnerMat);
    flame.rotation.x = -Math.PI / 2;
    flame.position.set(side * 0.55, -0.05, -7.8);
    flame.name = 'afterburner';
    group.add(flame);
  }

  return {
    group,
    tintMaterials: [mat],
    disposeMaterials: [mat, darkMat, canopyMat, burnerMat],
    scale: 0.72,
    wheelDrop: 1.0,
  };
}

// MQ-9-style drone: monitoring / scheduled tasks. Points along +Z.
export function makeDroneMesh() {
  const group = new THREE.Group();
  const mat = new THREE.MeshLambertMaterial({ color: 0xffffff });
  const darkMat = new THREE.MeshLambertMaterial({ color: 0x222831 });
  const rotorMat = new THREE.MeshLambertMaterial({
    color: 0x222831, transparent: true, opacity: 0.35, side: THREE.DoubleSide,
  });

  const nose = new THREE.Mesh(new THREE.ConeGeometry(1.4, 5, 8), mat);
  nose.rotation.x = Math.PI / 2;
  nose.position.z = 9;
  group.add(nose);

  const body = new THREE.Mesh(new THREE.CylinderGeometry(1.4, 1.1, 16, 8), mat);
  body.rotation.x = Math.PI / 2;
  body.position.z = -0.5;
  group.add(body);

  const turret = new THREE.Mesh(new THREE.SphereGeometry(1.1, 12, 12), darkMat);
  turret.position.set(0, -1.3, 6.5);
  group.add(turret);

  const wings = new THREE.Mesh(new THREE.BoxGeometry(24, 0.4, 2.6), mat);
  wings.position.set(0, 0.6, 1);
  group.add(wings);

  for (const side of [1, -1]) {
    const fin = new THREE.Mesh(new THREE.BoxGeometry(0.4, 5, 2.2), mat);
    fin.position.set(side * 1.6, 1.6, -7);
    fin.rotation.z = side * -0.6;
    group.add(fin);
  }

  const prop = new THREE.Mesh(new THREE.CircleGeometry(2.6, 16), rotorMat);
  prop.position.set(0, 0.4, -8.8);
  prop.name = 'prop';
  group.add(prop);

  return { group, tintMaterials: [mat], disposeMaterials: [mat, darkMat, rotorMat], scale: 0.76, wheelDrop: 1.8 };
}

