// Keep tangent offsets and subdivision in step with world/ground_route.rs.
const distance = (a,b) => Math.hypot(a[0]-b[0],a[1]-b[1]);
const mix = (a,b,t) => [a[0]+(b[0]-a[0])*t,a[1]+(b[1]-a[1])*t];
export function roundedRoute(vertices, { startHeading, endHeading } = {}) {
  const v=vertices.filter((p,i)=>!i || distance(p,vertices[i-1])>.001);
  if(v.length<2) return v;
  const points=[v[0]];
  for(let i=1;i<v.length-1;i++) {
    const [a,b,c]=[v[i-1],v[i],v[i+1]], ab=distance(a,b),bc=distance(b,c);
    const u=[(b[0]-a[0])/ab,(b[1]-a[1])/ab],w=[(c[0]-b[0])/bc,(c[1]-b[1])/bc];
    const width=Math.abs(b[0]-103)<.01 && Math.abs(b[1])<80 ? 8 : 5;
    const trim=Math.min(width,ab*.45,bc*.45);
    if(u[0]*w[0]+u[1]*w[1]<-.99) {points.push(b);continue;}
    const entry=[b[0]-u[0]*trim,b[1]-u[1]*trim],exit=[b[0]+w[0]*trim,b[1]+w[1]*trim];
    points.push(entry);
    for(let j=1;j<=128;j++) {const t=j/128;points.push(mix(mix(entry,b,t),mix(b,exit,t),t));}
  }
  points.push(v.at(-1));
  // Subdivide straight legs too, so endpoint blends occupy five units.
  const dense=[points[0]];
  for(let i=1;i<points.length;i++) {
    const a=points[i-1], b=points[i], n=Math.max(1,Math.ceil(distance(a,b)/.25));
    for(let j=1;j<=n;j++) dense.push(mix(a,b,j/n));
  }
  for(const first of [true,false]) {
    const heading=first?startHeading:endHeading;
    if(!Number.isFinite(heading)) continue;
    const index=first?0:dense.length-1;
    let span=0,k=index;
    while(span<5 && (first?k+1<dense.length:k>0)) {const next=first?k+1:k-1;span+=distance(dense[k],dense[next]);k=next;}
    if(span<.01) continue;
    const a=first?dense[0]:dense[k],b=first?dense[k]:dense[index];
    const tangent=(i)=>{const p=dense[Math.max(0,i-1)],q=dense[Math.min(dense.length-1,i+1)];return Math.atan2(q[1]-p[1],q[0]-p[0]);};
    const ha=first?heading:tangent(k),hb=first?tangent(k):heading;
    if(Math.abs(Math.atan2(Math.sin(ha-hb),Math.cos(ha-hb)))<.001) continue;
    const d=Math.min(span,5)*.45,p1=[a[0]+Math.cos(ha)*d,a[1]+Math.sin(ha)*d],p2=[b[0]-Math.cos(hb)*d,b[1]-Math.sin(hb)*d];
    const controls=Math.cos(ha-hb)<-.95?[a,p1,[(a[0]+b[0])/2-Math.sin(ha)*d*3,(a[1]+b[1])/2+Math.cos(ha)*d*3],p2,b]:[a,p1,p2,b];
    const blend=[];
    for(let j=0;j<=256;j++) {let ps=controls;while(ps.length>1) ps=ps.slice(1).map((p,i)=>mix(ps[i],p,j/256));blend.push(ps[0]);}
    if(first) dense.splice(0,k+1,...blend);else dense.splice(k,dense.length-k,...blend);
  }
  return dense;
}

// Flat ribbons give both pavement and paint the identical ground geometry.
export function routeRibbon(THREE, points, width, y, material) {
  const positions=[],indices=[];
  for(let i=0;i<points.length;i++) {
    const a=points[Math.max(0,i-1)], b=points[Math.min(points.length-1,i+1)];
    const len=distance(a,b)||1, nx=-(b[1]-a[1])/len*width/2,nz=(b[0]-a[0])/len*width/2;
    positions.push(points[i][0]+nx,y,points[i][1]+nz,points[i][0]-nx,y,points[i][1]-nz);
    if(i) {const k=i*2;indices.push(k-2,k,k-1,k-1,k,k+1);}
  }
  const geometry=new THREE.BufferGeometry();
  geometry.setAttribute('position',new THREE.Float32BufferAttribute(positions,3));
  // Offset shoulders can fold across a tight reversing centerline. Keep all
  // pavement triangles upward facing instead of lighting folded faces black.
  for(let i=0;i<indices.length;i+=3) {
    const [a,b,c]=indices.slice(i,i+3).map(k=>[positions[k*3],positions[k*3+2]]);
    if((b[0]-a[0])*(c[1]-a[1])-(b[1]-a[1])*(c[0]-a[0])>0) [indices[i+1],indices[i+2]]=[indices[i+2],indices[i+1]];
  }
  geometry.setIndex(indices);geometry.computeVertexNormals();
  return new THREE.Mesh(geometry,material);
}

// Production arrivals hand off at the fixed runway lead-in, ten units before
// the exit vertex. Use this for both pavement and centerline (and evidence).
export function productionRoute(vertices, {startHeading, endHeading} = {}) {
  const [x,z] = vertices[0];
  const arrival = (x === 56 && z === -66) || (x === 4 && z === -30.5);
  return roundedRoute(arrival ? [[x,z+10], ...vertices] : vertices, {
    startHeading: startHeading ?? (arrival ? -Math.PI/2 : undefined), endHeading,
  });
}
