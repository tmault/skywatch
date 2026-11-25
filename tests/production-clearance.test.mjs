import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync,mkdtempSync,rmSync} from 'node:fs';
import {execFileSync} from 'node:child_process';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {productionScene,pavementTriangles,onPavement,jumboContacts} from '../docs/evidence/rounded-taxiways/production-scene.js';
const scene=productionScene(readFileSync(new URL('../web/main.js',import.meta.url),'utf8'));
const satelliteScene=productionScene(readFileSync(new URL('../web/main.js',import.meta.url),'utf8'),true);
const triangles=pavementTriangles(scene),satelliteTriangles=pavementTriangles(satelliteScene);
const dir=mkdtempSync(join(tmpdir(),'skywatch-ground-'));
let traces;
try {
  const path=join(dir,'traces.json');
  execFileSync('cargo',['test','--manifest-path',new URL('../server/Cargo.toml',import.meta.url).pathname,'production_ground_trajectories'],{env:{...process.env,SKYWATCH_PRODUCTION_EVIDENCE:path},stdio:'pipe'});
  traces=JSON.parse(readFileSync(path));
  assert.deepEqual(traces,JSON.parse(readFileSync(new URL('../docs/evidence/rounded-taxiways/production-trajectories.json',import.meta.url))), 'refresh committed evidence from current server');
} finally { rmSync(dir,{recursive:true,force:true}); }

test('all rotated 747 tyre contact patches stay on actual production pavement, both edges',()=>{
  const failures=[];
  for(const trace of traces.filter(t=>t.name.startsWith('747'))) for(let i=0;i<trace.frames.length;i++) {
    for(const p of jumboContacts(trace.frames[i])) if(!onPavement(p,trace.satellite?satelliteTriangles:triangles)) { failures.push({route:trace.name,frame:i,heading:trace.frames[i][2],contact:p});break; }
    if(failures.length>=10) break;
  }
  assert.deepEqual(failures,[]);
});
test('driven production routes coincide with painted curves including reversals and exit lead-in',()=>{
  const paint=[];
  const collect=m=>{
    const p=m.geometry?.attributes.position;
    if(!m.isMesh||m.geometry.type!=='BufferGeometry'||!p||p.count<4) return;
    const width=Math.hypot(p.getX(0)-p.getX(1),p.getZ(0)-p.getZ(1));
    if(Math.abs(width-.7)>.001) return;
    const points=[];
    for(let i=0;i<p.count;i+=2) points.push([(p.getX(i)+p.getX(i+1))/2,(p.getZ(i)+p.getZ(i+1))/2]);
    paint.push(...points.slice(1).map((b,i)=>[points[i],b]));
  };
  scene.traverse(collect);satelliteScene.traverse(collect);
  const distance=(p,a,b)=>{const dx=b[0]-a[0],dz=b[1]-a[1],t=Math.max(0,Math.min(1,((p[0]-a[0])*dx+(p[1]-a[1])*dz)/(dx*dx+dz*dz||1)));return Math.hypot(p[0]-a[0]-t*dx,p[1]-a[1]-t*dz);};
  for(const trace of traces) for(let i=0;i<trace.frames.length;i+=8) {
    const p=trace.frames[i];
    const d=Math.min(...paint.map(([a,b])=>distance(p,a,b)));
    assert.ok(d<.03,`${trace.name} frame ${i} is ${d} from paint`);
  }
});
test('contact check catches rotated nose gear across either edge',()=>{
  const strip=[[[0,-6],[20,-6],[20,6]],[[0,-6],[20,6],[0,6]]];
  assert.ok(jumboContacts([10,0,0]).every(p=>onPavement(p,strip)));
  for(const h of [-Math.PI/2,Math.PI/2]) assert.ok(jumboContacts([10,0,h]).some(p=>!onPavement(p,strip)));
});
