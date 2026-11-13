import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";

const source=readFileSync(new URL("../../rust/volley-app/static/studio.js",import.meta.url),"utf8");
const html=readFileSync(new URL("../../rust/volley-app/static/studio.html",import.meta.url),"utf8");
assert.match(html,/id="studio-raw-people-visible"/);
assert.match(html,/id="studio-raw-pose"/);
assert.match(html,/id="studio-raw-pose-visible"/);
assert.match(html,/id="studio-raw-ball"[^>]*value="0"/,
  "Raw VballNet candidates should be visible before tuning confidence");
assert.doesNotMatch(html,/id="studio-raw-ravel-ball"/,
  "RAVEL ball candidates must not appear in the new UI");
const first=source.indexOf("  const rawFusion=()=>");
const last=source.indexOf("  const cpuTouchStyle=",first);
assert.ok(first>=0&&last>first);
const requests=[];
const messages=[];
const s={
  project:"p",analysis:"fusion-a",video:"match.mp4",run:"",
  chain:{choices:[
    {value:"fusion-a",kind:"fusion",id:"a".repeat(32)},
    {value:"fusion-b",kind:"fusion",id:"b".repeat(32)}
  ]}
};
const overlay={
  raw:true,flags:{pose:false},
  rawPoseVisible:{checked:true},
  cache:new Map(),rawFrames:new Map(),failed:new Map(),controllers:new Map(),fetching:new Set()
};
const ctx={
  s,overlay,timeline:{data:null},AbortController,Date,
  url:(route,params)=>route+"?"+new URLSearchParams(params),
  request:(path)=>new Promise((resolve,reject)=>requests.push({path,resolve,reject})),
  drawOverlay:()=>{},overlayMessage:message=>messages.push(message),
  fps:()=>30,
};
runInNewContext(source.slice(first,last)+
  "\nthis.raw={id:overlaySourceId,key:overlayWindowKey,request:requestOverlayWindow,prefetch:prefetchOverlayAtFrame}",ctx);
const {raw}=ctx;
const valid=(fusionId,start=0)=>({
  schema:"volley-native-gpu-overlay/1",project_id:"p",video:"match.mp4",
  fusion_id:fusionId,source_fusion_sha256:"c".repeat(64),
  fps:30,width:1280,height:720,start_frame:start,end_frame:start+90,total_frames:270,
  frames:Array.from({length:90},(_,i)=>({frame:start+i,raw:{balls:[],players:[],poses:[]}})),
  poses_available:false
});
const flush=()=>new Promise(done=>setImmediate(done));
const a=s.chain.choices[0].id,b=s.chain.choices[1].id;
raw.prefetch(0);
assert.equal(requests.length,1,"Unknown GPU frame counts prefetch only a safe initial window");
assert.ok(requests[0].path.startsWith("/api/gpu/native/overlays?"));
assert.ok(requests[0].path.includes("fusion_id="+a));
assert.ok(!requests[0].path.includes("rally_run_id"),"Never use a CPU source in raw mode");
s.analysis="fusion-b";
raw.request("p",b,"match.mp4",0);
requests[0].resolve(valid(a));
await flush();
assert.equal(overlay.cache.size,0,"Old fusion responses cannot populate cache");
requests[1].resolve(valid(b));
await flush();
assert.equal(overlay.cache.size,1,"Current fusion loads normally");
assert.ok(overlay.cache.has("p:gpu:"+b+":match.mp4:0"));
s.run="irrelevant-cpu-run";
assert.equal(raw.id(),b,"Switching CPU versions never changes raw fusion source");
overlay.raw=false;
assert.equal(raw.id(),"irrelevant-cpu-run");
assert.equal(raw.key("p",s.run,"match.mp4",0),"p:cpu:irrelevant-cpu-run:match.mp4:0");
overlay.raw=true;
s.run="";
raw.request("p",b,"match.mp4",120);
requests[2].resolve({...valid(b,120),source_fusion_sha256:"wrong"});
await flush();
assert.ok(!overlay.cache.has("p:gpu:"+b+":match.mp4:120"),
  "Mismatched source SHA metadata is rejected");
assert.ok(messages.some(message=>message.includes("RTMPose-M")),
  "Unavailable RTMPose evidence is shown explicitly");
console.log("Studio raw GPU source isolation and stale responses: OK");
