// Deterministic synthetic capture; no websocket or production data sources.
import {spawn} from 'node:child_process';
import {mkdir,writeFile} from 'node:fs/promises';
const out=new URL('.',import.meta.url).pathname;
const frames='/tmp/skywatch-turn-frames';await mkdir(frames,{recursive:true});
const chrome=spawn('chromium',['--headless','--disable-extensions','--no-sandbox','--enable-unsafe-swiftshader','--remote-debugging-port=9223','--user-data-dir=/tmp/skywatch-taxi-evidence','about:blank'],{stdio:'ignore',env:{...process.env,TMPDIR:'/tmp'}});
try {
 let tabs;for(let i=0;i<100;i++){try{tabs=await(await fetch('http://127.0.0.1:9223/json')).json();break;}catch{await new Promise(r=>setTimeout(r,100));}}
 const ws=new WebSocket(tabs.find(t=>t.url==='about:blank').webSocketDebuggerUrl);await new Promise(r=>ws.onopen=r);
 let id=0;const pending=new Map();ws.onmessage=e=>{const m=JSON.parse(e.data);if(m.id){const {resolve,reject}=pending.get(m.id);pending.delete(m.id);m.error?reject(m.error):resolve(m.result);}};
 const call=(method,params={})=>new Promise((resolve,reject)=>{const n=++id;pending.set(n,{resolve,reject});ws.send(JSON.stringify({id:n,method,params}));});
 await call('Page.enable');await call('Emulation.setDeviceMetricsOverride',{width:1400,height:800,deviceScaleFactor:1,mobile:false});
 await call('Page.navigate',{url:'http://127.0.0.1:8765/docs/evidence/rounded-taxiways/replay.html'});
 for(let i=0;i<100;i++){const r=await call('Runtime.evaluate',{expression:'window.evidenceReady'});if(r.result.value)break;await new Promise(r=>setTimeout(r,100));}
 for(let i=0;i<128;i++){
   await call('Runtime.evaluate',{expression:`window.demoSeek(${i/8})`});
   const {data}=await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false});const bytes=Buffer.from(data,'base64');
   await writeFile(`${frames}/${String(i).padStart(4,'0')}.png`,bytes);
   if(i===16)await writeFile(out+'approaching-turns-top-down.png',bytes);
   if(i===48)await writeFile(out+'moving-turns-top-down.png',bytes);
 }
 ws.close();console.log(frames);
}finally{chrome.kill();}
