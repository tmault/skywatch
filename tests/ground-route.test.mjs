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
