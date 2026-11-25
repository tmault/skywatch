import * as THREE from './vendor/three.module.js';
import { buildAlpineIsland, buildAlpineBackdrop, makeLakeMaterial, buildWinterCountryside } from './alpine.js';

export const THEME_PALETTES = Object.freeze({
  alpine: { sky: 0xb9d9ef, fog: 0xc9e0ee, ground: 0x3f5a34, sun: 0xfff0d8, water: 0x1f6f8c },
  tropical: { sky: 0x8ddbe8, fog: 0xb6e8eb, ground: 0xa5a064, sun: 0xfff1cf, water: 0x29bdb5 },
  winter: { sky: 0xbacfe4, fog: 0xdce5ee, ground: 0xb4c8dc, sun: 0xf0f6ff, water: 0xe4edf5 },
});

// Disposes only owned scenery resources. Aircraft, flags, runways and controls
// are never children of these roots and survive a theme switch unchanged.
export function disposeScenery(root) {
  const geometries = new Set(), materials = new Set(), textures = new Set();
  root.traverse(obj => {
    if (obj.geometry) geometries.add(obj.geometry);
    const objMaterials = [obj.material].flat().filter(Boolean);
    for (const material of objMaterials) {
      materials.add(material);
      for (const value of Object.values(material)) if (value?.isTexture) textures.add(value);
    }
  });
  for (const value of [...geometries, ...materials, ...textures]) value.dispose();
  root.removeFromParent();
}

export function createEnvironment({ scene, sun, hemisphere, state }) {
  const islands = new Set();
  const surface = new THREE.Mesh(new THREE.CircleGeometry(1300, 96), makeLakeMaterial());
  surface.rotation.x = -Math.PI / 2;
  surface.receiveShadow = true;
  scene.add(surface);
  let backdrop;
  let countryside;
  function dress(entry, theme) {
    if (entry.root) disposeScenery(entry.root);
    entry.root = new THREE.Group();
    entry.parent.add(entry.root);
    entry.terrain = buildAlpineIsland(entry.root, { ...entry.spec, theme });
  }
  const unsubscribe = state.subscribe(theme => {
    const palette = THEME_PALETTES[theme];
    scene.background?.dispose?.();
    scene.background = new THREE.Color(palette.sky);
    scene.fog.color.setHex(palette.fog);
    sun.color.setHex(palette.sun);
    hemisphere.groundColor.setHex(palette.ground);
    surface.material.color.setHex(palette.water);
    surface.material.normalScale.setScalar(theme === 'winter' ? 0 : 0.26);
    surface.material.roughness = theme === 'winter' ? 1 : 0.3;
    if (countryside) disposeScenery(countryside);
    countryside = new THREE.Group();
    scene.add(countryside);
    if (theme === 'winter') buildWinterCountryside(countryside);
    if (backdrop) disposeScenery(backdrop);
    backdrop = buildAlpineBackdrop(scene, 620, 1500, theme);
    for (const entry of islands) dress(entry, theme);
    document.documentElement.dataset.theme = theme;
    window.dispatchEvent(new CustomEvent('skywatch-theme-change', { detail: { theme } }));
  });
  return {
    island(parent, spec) {
      const entry = { parent, spec };
      islands.add(entry); dress(entry, state.theme);
      return { heightAt: (x, z) => entry.terrain.heightAt(x, z), dispose() { islands.delete(entry); disposeScenery(entry.root); } };
    },
    dispose() {
      unsubscribe();
      for (const entry of islands) disposeScenery(entry.root);
      islands.clear(); disposeScenery(countryside); disposeScenery(backdrop); disposeScenery(surface);
    },
  };
}
