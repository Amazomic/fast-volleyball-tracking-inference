import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";

const js=readFileSync(new URL("../../rust/volley-app/static/studio.js",import.meta.url),"utf8");
const start=js.indexOf("  const overlayWindowStart=");
const end=js.indexOf("  const setOverlaySource=",start);
assert.ok(start>=0&&end>start,"Preview overlay window helpers must exist");
const script=js.slice(start,end)+
  "\nthis.drawOverlay=drawOverlay;this.overlayWindowStart=overlayWindowStart;";

const oldRecord={
  start_frame:0,end_frame:180,
  frames:[{frame:90,players:[{id:1,box:[0.1,0.2,0.3,0.8]}],ball:null}]
};
const calls=[];
let outlines=0;
const actionColors=[];
const ctx={
  strokeRect(){outlines++;},
  beginPath(){},arc(){},fill(){},stroke(){actionColors.push(this.strokeStyle);},
  measureText(){return {width:20};},
  fillRect(){},fillText(){}
};
const s={project:"p",run:"r",video:"v"};
const ui={video:{currentTime:90/30,src:"/api/video-media"}};
const overlay={
  source:{events:[]},flags:{players:true,touches:false,trajectory:false},
  cache:new Map([["p:r:v:0",oldRecord]]),
  fetching:new Set(),failed:new Map()
};
const timeline={data:{total_frames:300}};
const scope={s,ui,overlay,timeline,
  overlayContext:()=>({ctx,x:0,y:0,width:100,height:100}),
  fps:()=>30,url:(path,args)=>path+"?start_frame="+args.start_frame,
  request:resource=>{calls.push(resource);return new Promise(()=>{});},
  overlayMessage:()=>{}
};
runInNewContext(script,scope,{filename:"studio-overlay-window.js"});
assert.equal(scope.overlayWindowStart(89),0);
assert.equal(scope.overlayWindowStart(90),30);
assert.equal(scope.overlayWindowStart(180),120);

// At the 3-second boundary, the new request is outstanding but the
// previous window still contains the frame. Its boxes must not disappear.
// Prefetch 1.5 seconds before the new window becomes active.
ui.video.currentTime=45/30;
scope.drawOverlay();
assert.equal(calls.filter(s=>s.includes("start_frame=30")).length,1,
  "Next window must be prefetched before the 3-second boundary");

// A slow request must not erase already available player boxes.
ui.video.currentTime=90/30;
scope.drawOverlay();
scope.drawOverlay();
assert.equal(outlines,2,"Prior window must render while the next is fetching");
assert.equal(calls.filter(s=>s.includes("start_frame=30")).length,1,
  "Overlapping request must be deduplicated");

// No fabricated contours when saved evidence does not cover the frame.
ui.video.currentTime=180/30;
scope.drawOverlay();
assert.equal(outlines,2,"No stale boxes outside source window");
assert.equal(calls.filter(s=>s.includes("start_frame=120")).length,1);
overlay.raw=true;
overlay.rawBall={value:"70"};overlay.rawPlayer={value:"60"};overlay.rawTouch={value:"80"};
overlay.flags.touches=true;
overlay.cache.set("p:r:v:0:raw:70:60:80",{
  start_frame:0,end_frame:180,frames:[{frame:45,raw:{
    balls:[],players:[{box:[0.1,0.2,0.3,0.8]}],touches:[{box:[0.1,0.2,0.3,0.8]}]
  }}]
});
ui.video.currentTime=45/30;
scope.drawOverlay();
assert.equal(outlines,4,"Raw player and touch boxes are drawn independently");
overlay.actionFlags={attack:true,block:false};
timeline.data.estimated_actions=[
  {frame_index:45,kind:"attack",ball_position_px:[20,20]},
  {frame_index:45,kind:"block",ball_position_px:[30,30]}
];
overlay.cache.get("p:r:v:0:raw:70:60:80").width=100;
overlay.cache.get("p:r:v:0:raw:70:60:80").height=100;
scope.drawOverlay();
assert.deepEqual(actionColors,["#ff6b6b"],"Only enabled action estimates receive their own color");
overlay.actionFlags.block=true;
actionColors.length=0;
scope.drawOverlay();
assert.deepEqual(actionColors,["#ff6b6b","#c38cff"],"Attack and block use distinct colors");
const initStart=js.indexOf("  if(overlay.mode){");
const initEnd=js.indexOf("  const startOverlayFrameLoop=",initStart);
let changed,redraws=0;
const checkbox={dataset:{overlayAction:"attack"},checked:false,addEventListener:(name,fn)=>{if(name==="change")changed=fn;}};
const input={addEventListener(){}};
const controls={querySelectorAll:()=>[checkbox],classList:{toggle(){}}};
const initOverlay={mode:{addEventListener(){}},rawControls:controls,rawBall:input,rawPlayer:input,rawTouch:input,actionFlags:{attack:true}};
runInNewContext(js.slice(initStart,initEnd),{overlay:initOverlay,$:()=>({}),drawOverlay:()=>{redraws++;},setTimeout,clearTimeout});
changed();
assert.equal(initOverlay.actionFlags.attack,false,"Checkbox disables the matching action");
assert.equal(redraws,1,"Changing the checkbox redraws the player");
console.log("Studio preview overlay frame continuity: OK");
