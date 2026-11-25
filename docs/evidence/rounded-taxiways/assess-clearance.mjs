import {readFileSync,writeFileSync} from 'node:fs';
import * as THREE from '../../../web/vendor/three.module.js';
import {makeJumboMesh} from '../../../web/aircraft.js';
const traces=JSON.parse(readFileSync(new URL('./production-trajectories.json',import.meta.url)));
// Canvas drawing does not affect airframe geometry in this Node assessment.
const ctx=new Proxy({}, {get:(_,key)=>key==='measureText'?()=>({width:1}):key==='createLinearGradient'?()=>({addColorStop(){}}):()=>{}});
globalThis.document={createElement:()=>({getContext:()=>ctx})};
const built=makeJumboMesh('gpt-6-astra'),vertices=[];built.group.updateMatrixWorld(true);
built.group.traverse(m=>{if(!m.isMesh||m.name==='hit-proxy')return;const p=m.geometry.attributes.position;for(let i=0;i<p.count;i++)vertices.push(new THREE.Vector3().fromBufferAttribute(p,i).applyMatrix4(m.matrixWorld).multiplyScalar(built.scale));});
const wings=vertices.filter(v=>Math.abs(v.x)>6*built.scale);
const halfSpan=Math.max(...wings.map(v=>Math.abs(v.x)));
const report={method:'Conservative planar swept vertex envelope of production jumbo mesh (scale 0.345) at every fourth 20 Hz frame; broad-phase object checks only, no continuous triangle collision solver.',span:2*halfSpan,homeGateSpacing:19,adjacentHomeJumboEnvelopeOverlap:2*halfSpan-19,satelliteGateSpacing:16,adjacentSatelliteJumboEnvelopeOverlap:2*halfSpan-16,routes:[]};
for(const trace of traces.filter(t=>t.name.startsWith('747'))) {
 let terminalGap=Infinity,bridgeCandidates=0;
 const termX=trace.satellite?40.05:133.5,termHalfZ=trace.satellite?30:93;
 for(let i=0;i<trace.frames.length;i+=4){const [x,z,h]=trace.frames[i],c=Math.cos(h),s=Math.sin(h);let minX=Infinity,maxX=-Infinity,minZ=Infinity,maxZ=-Infinity;
 for(const v of vertices){const px=x+v.z*c+v.x*s,pz=z+v.z*s-v.x*c;if(Math.abs(pz)<termHalfZ) terminalGap=Math.min(terminalGap,termX-px);}
 if(!trace.satellite){
  for(const v of wings){const px=x+v.z*c+v.x*s,pz=z+v.z*s-v.x*c;minX=Math.min(minX,px);maxX=Math.max(maxX,px);minZ=Math.min(minZ,pz);maxZ=Math.max(maxZ,pz);}
  for(let g=0;g<10;g++){const gz=-85.5+19*g;if(maxX>=126&&minX<=135&&maxZ>=gz-4.1&&minZ<=gz-2.3)bridgeCandidates++;}
 }
 }
 report.routes.push({name:trace.name,satellite:trace.satellite,minimumTerminalPlanGap:terminalGap,wingBridgeBroadPhaseCandidateFrames:bridgeCandidates});
}
report.limits=['Positive terminal gaps (home +0.57 u, satellite +3.12 u) are sampled planar envelope results, not continuous swept collision certification.','Jetbridge broad-phase candidates need a full 3D triangle test; these static decorations are not collision-resolved.','Adjacent parked 747 wing envelopes overlap at both home (19-unit spacing, +3.28 u overlap) and satellite (16-unit spacing, +6.28 u overlap) stands; do not interpret wheel-on-pavement proof as wing, object, or traffic clearance.','No steering-axle/off-tracking tyre dynamics; tyres use rigid displayed airframe pose. Contact patches sampled at 20 Hz, not continuously between frames.'];
writeFileSync(new URL('./clearance-assessment.json',import.meta.url),JSON.stringify(report,null,2)+'\n');
console.log(JSON.stringify({span:report.span,homeOverlap:report.adjacentHomeJumboEnvelopeOverlap,satelliteOverlap:report.adjacentSatelliteJumboEnvelopeOverlap,homeTerminalGap:Math.min(...report.routes.filter(r=>!r.satellite).map(r=>r.minimumTerminalPlanGap)),satelliteTerminalGap:Math.min(...report.routes.filter(r=>r.satellite).map(r=>r.minimumTerminalPlanGap)),bridgeCandidates:report.routes.reduce((n,r)=>n+r.wingBridgeBroadPhaseCandidateFrames,0)}));
