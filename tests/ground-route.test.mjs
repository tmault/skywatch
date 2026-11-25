import test from 'node:test';
import assert from 'node:assert/strict';
import { roundedRoute } from '../web/ground-route.js';
test('paint follows tangent bends inside a six-unit paved corridor', () => {
  const points=roundedRoute([[0,0],[0,30],[30,30]]);
  assert.deepEqual(points[0],[0,0]);
  assert.deepEqual(points.at(-1),[30,30]);
  assert.ok(points.some(([x,z])=>x>0 && x<5 && z>25 && z<30));
  for (const [x,z] of points) assert.ok(Math.min(Math.abs(x),Math.abs(z-30))<=1.5);
});
test('short, duplicate and empty routes stay finite and preserve destinations', () => {
  for (const route of [[],[[2,3]],[[0,0],[0,0],[0,.1],[.1,.1]],[[0,0],[0,2],[0,0]]]) {
    const points=roundedRoute(route);
    for(const p of points) assert.ok(p.every(Number.isFinite));
    if(route.length) assert.deepEqual(points.at(-1),route.at(-1));
  }
});
test('broad home apron uses wider bends than narrow connectors', () => {
  const points=roundedRoute([[103,-40],[103,0],[120,0]]);
  assert.ok(points.some(([x,z])=>x>103.01 && z < -5));
});

test('production gate departure paints heading-aware reversal and arrival lead-in', async () => {
  const { productionRoute } = await import('../web/ground-route.js');
  const departure = productionRoute([[120,0],[103,0],[103,46],[84,46],[56,46]], {startHeading:0,endHeading:-Math.PI/2});
  assert.ok(departure[1][0] > 120, 'nose initially moves east before reversing');
  const arrival = productionRoute([[56,-66],[70,-66],[70,-88],[103,-88],[103,0],[120,0]], {endHeading:0});
  assert.deepEqual(arrival[0],[56,-56]);
  assert.ok(arrival.some(([x,z])=>x>56 && z>-66), 'exit is rounded interior vertex');
});
test('reversal pavement stays upward facing where offset shoulders fold', async()=>{
  const THREE=await import('../web/vendor/three.module.js');
  const {routeRibbon}=await import('../web/ground-route.js');
  const route=roundedRoute([[120,0],[103,0],[103,46]],{startHeading:0});
  const mesh=routeRibbon(THREE,route,12,3.2,new THREE.MeshBasicMaterial());
  const normals=mesh.geometry.attributes.normal;
  for(let i=0;i<normals.count;i++) assert.ok(normals.getY(i)>=0,`normal ${i} faces down`);
});
