import test from 'node:test';
import assert from 'node:assert/strict';
import { resolveTheme, createThemeState } from '../web/theme-state.js';
test('validated default supports legacy snapshots and local preference', () => {
  assert.equal(resolveTheme(), 'alpine');
  assert.equal(resolveTheme('winter'), 'winter');
  assert.equal(resolveTheme('winter', 'tropical'), 'tropical');
  assert.equal(resolveTheme('invalid', 'invalid'), 'alpine');
});
test('switch only replaces scenery once, saves choice and releases subscriptions', () => {
  let saved; const seen=[];
  const state=createThemeState('winter', {getItem:()=>null,setItem:(_,v)=>saved=v});
  const unsubscribe=state.subscribe(theme=>seen.push(theme));
  state.select('tropical'); state.select('tropical'); state.select('bad');
  assert.deepEqual(seen,['winter','tropical']); assert.equal(saved,'tropical');
  unsubscribe(); state.select('alpine'); assert.deepEqual(seen,['winter','tropical']);
});
test('storage denial still permits switching', () => {
  const state=createThemeState('winter',{getItem(){throw Error();},setItem(){throw Error();}});
  state.select('tropical'); assert.equal(state.theme,'tropical');
});

test('scenery is deterministic and replaced resources are disposed once', async () => {
  const THREE = await import('../web/vendor/three.module.js');
  const { buildAlpineIsland } = await import('../web/alpine.js');
  const { disposeScenery } = await import('../web/environment.js');
  const spec={seed:1.3,center:[0,0],radius:45,pads:[{x:15,z:0,hw:12,hl:30}],peaks:[],trees:15,chalets:2,rocks:5};
  const roots=[];
  for(const theme of ['alpine','tropical','winter']) {
    const a=new THREE.Group(), b=new THREE.Group();
    buildAlpineIsland(a,{...spec,theme}); buildAlpineIsland(b,{...spec,theme});
    const first = a.children[2].geometry.attributes.position.array;
    const second = b.children[2].geometry.attributes.position.array;
    assert.equal(first.length, second.length);
    assert.equal(first.findIndex((value, i) => !Object.is(value, second[i])), -1);
    const position=a.children[0].geometry.attributes.position.array;
    if(roots.length) assert.deepEqual(position,roots[0].children[0].geometry.attributes.position.array);
    roots.push(a); disposeScenery(b);
  }
  let disposed=0;
  roots[0].children[0].geometry.addEventListener('dispose',()=>disposed++);
  disposeScenery(roots[0]); assert.equal(disposed,1);
  for(const root of roots.slice(1)) disposeScenery(root);
});

test('explicitly selecting the server default remembers that local preference', () => {
  let saved;
  const state=createThemeState('alpine',{getItem:()=>null,setItem:(_,value)=>saved=value});
  state.select('alpine');
  assert.equal(saved,'alpine');
});
