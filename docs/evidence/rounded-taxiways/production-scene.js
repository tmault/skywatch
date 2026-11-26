// Evidence harness: execute the production airport builder verbatim. Only
// scenery outside the movement area (terrain, labels, lights) is stubbed.
import * as THREE from '../../../web/vendor/three.module.js';
import {productionRoute, routeRibbon, dedupePolylines} from '../../../web/ground-route.js';
export function productionScene(source, satellite=false) {
  const code = source.slice(source.indexOf('function makeDeckSlab('), source.indexOf('const OFFLINE_ISLAND_TINT'));
  const scene = new THREE.Scene();
  const stub = () => new THREE.Group();
  const build = new Function('THREE','productionRoute','routeRibbon','dedupePolylines','renderer','scene','TARMAC_TOP_Y','SWISS_RED','environment','window','makeSwissFlag','makeBannerLabel','addRunwayLights','enableShadows','runwayEvents','createRunwayEvents','applySatelliteOnlineState',`let homeRunwayLights; ${code}; return [buildHomeAirport,buildSatelliteAirport];`)(THREE,productionRoute,routeRibbon,dedupePolylines,{capabilities:{getMaxAnisotropy:()=>1}},scene,3.2,0xe53935,{island:()=>({heightAt:()=>3})},{addEventListener(){}},stub,stub,()=>({setOnline(){}}),()=>{},{},()=>({}),()=>{});
  build[satellite?1:0]({id:'synthetic',name:'SYNTHETIC',online:true,center:[0,0],gates:satellite?Array.from({length:4},(_,i)=>[24,-24+16*i]):Array.from({length:10},(_,i)=>[120,-85.5+19*i])},0);
  scene.updateMatrixWorld(true);
  return scene;
}
export function pavementTriangles(scene) {
  const triangles=[];
  scene.traverse(mesh=>{
    if(!mesh.isMesh) return;
    const p=mesh.geometry.attributes.position,idx=mesh.geometry.index;
    for(let i=0;i<(idx?.count??p.count);i+=3) {
      const v=[0,1,2].map(k=>new THREE.Vector3().fromBufferAttribute(p,idx?idx.getX(i+k):i+k).applyMatrix4(mesh.matrixWorld));
      if(v.every(p=>p.y>=3.14 && p.y<=3.23)) triangles.push(v.map(p=>[p.x,p.z]));
    }
  });
  return triangles;
}
const gridCache = new WeakMap();
function triangleGrid(triangles) {
  let grid = gridCache.get(triangles);
  if (grid) return grid;
  grid = new Map();
  const cell = 8;
  for (const t of triangles) {
    const minX = Math.min(t[0][0], t[1][0], t[2][0]) - 0.001;
    const maxX = Math.max(t[0][0], t[1][0], t[2][0]) + 0.001;
    const minZ = Math.min(t[0][1], t[1][1], t[2][1]) - 0.001;
    const maxZ = Math.max(t[0][1], t[1][1], t[2][1]) + 0.001;
    const entry = { t, minX, maxX, minZ, maxZ };
    for (let gx = Math.floor(minX / cell); gx <= Math.floor(maxX / cell); gx++) {
      for (let gz = Math.floor(minZ / cell); gz <= Math.floor(maxZ / cell); gz++) {
        const key = `${gx},${gz}`;
        let bucket = grid.get(key);
        if (!bucket) { bucket = []; grid.set(key, bucket); }
        bucket.push(entry);
      }
    }
  }
  gridCache.set(triangles, grid);
  return grid;
}
export function onPavement(p, triangles) {
  const bucket = triangleGrid(triangles).get(`${Math.floor(p[0] / 8)},${Math.floor(p[1] / 8)}`);
  if (!bucket) return false;
  const px = p[0], pz = p[1];
  for (const { t, minX, maxX, minZ, maxZ } of bucket) {
    if (px < minX || px > maxX || pz < minZ || pz > maxZ) continue;
    const [a, b, c] = t;
    const s0 = (b[0] - a[0]) * (pz - a[1]) - (b[1] - a[1]) * (px - a[0]);
    const s1 = (c[0] - b[0]) * (pz - b[1]) - (c[1] - b[1]) * (px - b[0]);
    const s2 = (a[0] - c[0]) * (pz - c[1]) - (a[1] - c[1]) * (px - c[0]);
    if ((s0 >= -0.001 && s1 >= -0.001 && s2 >= -0.001) || (s0 <= 0.001 && s1 <= 0.001 && s2 <= 0.001)) return true;
  }
  return false;
}
// Contact patch corners of every production 747 tyre (makeJumboMesh scale 0.345),
// including the 4x4 main bogies and twin nose gear.
export function jumboContacts(frame) {
  const scale = 0.345;
  const contacts=[];
  const add=(x,m,halfWidth)=>{for(const dx of [-halfWidth,halfWidth]) for(const dz of [-.1,.1]) {
    const lateral=(x+dx)*scale,forward=(34.3-m+dz)*scale,h=frame[2];
    contacts.push([frame[0]+forward*Math.cos(h)+lateral*Math.sin(h),frame[1]+forward*Math.sin(h)-lateral*Math.cos(h)]);
  }};
  for(const side of [-1,1]) for(const [track,m] of [[5.5,33.9],[1.95,37.1]]) for(const dx of [-.46,.46]) for(const dz of [-.78,.78]) add(side*track+dx,m+dz,.22);
  for(const side of [-1,1]) add(side*.42,9.9,.18);
  return contacts;
}
