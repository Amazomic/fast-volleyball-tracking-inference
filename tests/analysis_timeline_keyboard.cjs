"use strict";
// Deterministic keyboard-seek regression for Analysis timeline (no browser needed).
const fs=require("node:fs");
const path=require("node:path");
const vm=require("node:vm");
const assert=require("node:assert/strict");
const source=fs.readFileSync(path.resolve(__dirname,"../scripts/volleyball_ui/static/analysis-ui.js"),"utf8");
let now=0,nextTimer=0;
const scheduled=new Map();
function schedule(fn,delay,interval=0){
  const id=++nextTimer;
  scheduled.set(id,{at:now+delay,fn,interval});
  return id;
}
function advance(ms){
  const end=now+ms;
  for(;;){
    const due=[...scheduled].filter(([,timer])=>timer.at<=end)
      .sort((a,b)=>a[1].at-b[1].at)[0];
    if(!due)break;
    const [id,timer]=due;
    now=timer.at;
    if(timer.interval)timer.at+=timer.interval;else scheduled.delete(id);
    timer.fn();
  }
  now=end;
}
const timers={
  setTimeout:(fn,ms)=>schedule(fn,ms),
  clearTimeout:id=>scheduled.delete(id),
  setInterval:(fn,ms)=>schedule(fn,ms,ms),
  clearInterval:id=>scheduled.delete(id),
};
const nodes=new Map(),events=new Map(),windowEvents=new Map();
function on(map,type,fn){const callbacks=map.get(type)||[];callbacks.push(fn);map.set(type,callbacks);}
function emit(map,type,params){for(const fn of map.get(type)||[])fn(params);}
function element(id){
  if(!nodes.has(id)){
    const callbacks=new Map();
    const el={
      id,style:{},dataset:{},textContent:"",innerHTML:"",open:false,
      hidden:false,checked:false,disabled:false,value:"",currentTime:0,
      duration:600,paused:true,readyState:2,src:"",muted:false,
      classList:{contains(x){return x==="active"&&id==="view-analysis";},toggle(){},add(){},remove(){}},
      addEventListener:(type,fn)=>on(callbacks,type,fn),
      dispatch:(type,evt)=>emit(callbacks,type,evt),
      setAttribute(){},removeAttribute(){},load(){},
      pause(){this.paused=true;},play(){this.paused=false;return Promise.resolve();},
      closest(){return null;},
    };
    nodes.set(id,el);
  }
  return nodes.get(id);
}
const doc={
  hidden:false,
  getElementById:element,querySelector(){return null;},
  addEventListener:(type,fn)=>on(events,type,fn),
};
const win={
  addEventListener:(type,fn)=>on(windowEvents,type,fn),
  dispatchEvent(){},
  VolleyballAutoSave:{schedule(){}},
  VolleyballCourtUI:null,
};
const FakeDate=class extends Date{static now(){return now;}};
const context=vm.createContext({
  window:win,document:doc,console,
  Date:FakeDate,CustomEvent:function(type,details){this.type=type;this.detail=details?.detail;},
  ...timers,
  fetch:async(url)=>{
    if(url.startsWith("/api/video-info"))return {ok:true,json:async()=>({
      duration_sec:600,width:1920,height:1080,auto_width:960,auto_height:540
    })};
    if(url.startsWith("/api/video-view"))return {ok:true,json:async()=>({
      browser_preview:"prepared",preview_url:"video.mp4",poster_url:"poster.jpg"
    })};
    throw Error("Unexpected fetch: "+url);
  },
  encodeURIComponent,
});
vm.runInContext(source,context,{filename:"analysis-ui.js"});
const analysis=win.VolleyballAnalysisUI;
const player=element("source-video-player");
element("analysis-width").value="960";
function key(type,key,target=element("match-timeline"),props={}){
  let prevented=false;
  emit(events,type,{
    key,code:key,altKey:false,ctrlKey:false,metaKey:false,
    target,preventDefault(){prevented=true;},...props
  });
  return prevented;
}
(async()=>{
  analysis.init();
  analysis.setProjectRanges({},"p1");
  await analysis.setVideo("match.mp4");
  assert.equal(analysis.currentPosition(),0);

  assert.equal(key("keydown","ArrowRight"),true);
  assert.equal(analysis.currentPosition(),1,"first press = exactly +1 sec");
  key("keydown","ArrowRight",undefined,{repeat:true});
  assert.equal(analysis.currentPosition(),1,"OS auto-repeats cannot double seek");
  advance(359);
  assert.equal(analysis.currentPosition(),1,"short press stays +1 sec");
  advance(161); // repeat delay + first tick
  assert.equal(analysis.currentPosition(),2,"holding starts repeated seeking");

  const before=analysis.currentPosition();
  advance(1700);
  const accelerated=analysis.currentPosition()-before;
  assert(accelerated>12,"holding exponentially accelerates: "+accelerated);
  assert(accelerated<150,"acceleration is capped: "+accelerated);
  key("keyup","ArrowRight");
  const stopped=analysis.currentPosition();
  advance(2000);
  assert.equal(analysis.currentPosition(),stopped,"keyup cancels repeat");

  key("keydown","ArrowLeft");
  assert.equal(analysis.currentPosition(),stopped-1,"left arrow moves backward");
  key("keyup","ArrowLeft");

  const input={closest:()=>({})};
  assert.equal(key("keydown","ArrowRight",input),false,"typing in input is unaffected");
  assert.equal(analysis.currentPosition(),stopped-1);
  key("keydown","ArrowRight",element("timeline-playhead"));
  assert.equal(analysis.currentPosition(),stopped,"focused playhead moves once, not twice");
  key("keyup","ArrowRight");

  key("keydown","ArrowRight");
  advance(1000);
  emit(windowEvents,"blur",{});
  const afterBlur=analysis.currentPosition();
  advance(1500);
  assert.equal(analysis.currentPosition(),afterBlur,"blur cancels repeat");

  key("keydown","ArrowLeft");
  advance(600);
  analysis.setProjectRanges({},"p2");
  const afterSwitch=analysis.currentPosition();
  advance(1500);
  assert.equal(analysis.currentPosition(),afterSwitch,"project switch cancels repeat");
  console.log("PASS: 1s click, exponential hold, repeat guard, left/right, focus, blur, project switch");
})().catch(e=>{console.error(e);process.exitCode=1;});
