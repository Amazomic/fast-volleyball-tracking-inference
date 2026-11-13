import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";

const js=readFileSync(new URL("../../rust/volley-app/static/studio.js",import.meta.url),"utf8");
const start=js.indexOf("  // Left/right arrows scrub the one source-wide timeline");
const end=js.indexOf("  // Space controls the player",start);
assert.ok(start>=0&&end>start,"Global accelerated timeline scrub handler must exist");
assert.match(js,/const steps=\{PageDown:-30,PageUp:30\}/,
  "Focused timeline slider must not also apply an independent arrow seek");

let now=0,visible=true,nextFrame=1,duration=6000;
const queued=new Map(),documentHandlers=new Map(),windowHandlers=new Map(),seeks=[];
const s={project:"p",video:"match.mp4",videoKey:"p:match.mp4",seekAt:100};
const video={currentTime:100};
const document={
  hidden:false,
  addEventListener:(name,handler,capture)=>documentHandlers.set(name+":"+!!capture,handler)
};
// Exercise the real compiled Rust WASM policy, not a JavaScript reimplementation.
const wasmBytes=readFileSync(new URL("../../rust/volley-app/static/volley_web.wasm",import.meta.url));
const engine=new WebAssembly.Instance(new WebAssembly.Module(wasmBytes),{}).exports;
assert.equal(engine.volley_preview_seek_tap(1),0.1);
assert.equal(engine.volley_preview_seek_tap(-1),-0.1);
assert.equal(engine.volley_preview_seek_hold_delta(1,279,100),0);
assert.equal(engine.volley_preview_seek_hold_delta(1,280,100),0.25);
const window={VolleyballUi:{engine},
  addEventListener:(name,handler)=>windowHandlers.set(name,handler)};
const context={
  document,window,s,ui:{video},performance:{now:()=>now},
  previewIsVisible:()=>visible&&!document.hidden,
  timelineDuration:()=>duration,
  seekTimeline:position=>{
    s.seekAt=Math.min(duration-0.05,Math.max(0,position));
    video.currentTime=s.seekAt;
    seeks.push({time:now,position:s.seekAt});
  },
  requestAnimationFrame:callback=>{
    const id=nextFrame++;
    queued.set(id,callback);
    return id;
  },
  cancelAnimationFrame:id=>queued.delete(id)
};
runInNewContext(js.slice(start,end)+"\nthis.arrowScrub=arrowScrub;",context);
const down=documentHandlers.get("keydown:true");
const up=documentHandlers.get("keyup:true");
assert.equal(typeof down,"function");
assert.equal(typeof up,"function");
const key=(value,options={})=>{
  const event={
    key:value,target:{tagName:"DIV",isContentEditable:false},
    shiftKey:false,altKey:false,ctrlKey:false,metaKey:false,
    isComposing:false,repeat:false,prevented:0,stopped:0,
    preventDefault(){this.prevented++;},
    stopPropagation(){this.stopped++;},
    ...options
  };
  down(event);
  return event;
};
const release=value=>up({key:value});
const advanceTo=endTime=>{
  while(now<endTime){
    now=Math.min(now+16,endTime);
    const frames=[...queued.values()];
    queued.clear();
    for(const frame of frames)frame(now);
  }
};

let event=key("ArrowRight");
assert.equal(s.seekAt,100.1,"A quick right-arrow press moves exactly 0.1 seconds");
assert.equal(event.prevented,1);
assert.equal(event.stopped,1);
key("ArrowRight",{repeat:true});
assert.equal(seeks.length,1,"OS key-repeat must not cause repeated base seeks");
advanceTo(240);
assert.equal(s.seekAt,100.1,"A tap should not start a continuous seek");
release("ArrowRight");
advanceTo(600);
assert.equal(s.seekAt,100.1,"Seeking must stop on keyup");
assert.equal(queued.size,0);

now=700;
key("ArrowRight");
advanceTo(1600);
const afterEarly=s.seekAt;
advanceTo(3000);
const afterLate=s.seekAt;
const earlyPerSecond=(afterEarly-100.2)/(0.9);
const latePerSecond=(afterLate-afterEarly)/1.4;
assert.ok(earlyPerSecond>0);
assert.ok(latePerSecond>earlyPerSecond*2,
  "Holding a direction must accelerate exponentially over time");
release("ArrowRight");
const settled=s.seekAt;
advanceTo(3500);
assert.equal(s.seekAt,settled);

key("ArrowLeft",{shiftKey:true});
assert.ok(Math.abs(s.seekAt-(settled-0.1))<1e-6,
  "Shift+Left also moves exactly 0.1 seconds");
release("ArrowLeft");
const beforeIgnored=s.seekAt;
for(const target of [
  {tagName:"INPUT"}, {tagName:"TEXTAREA"}, {tagName:"SELECT"},
  {tagName:"DIV",isContentEditable:true}
]){
  const ignored=key("ArrowRight",{target});
  assert.equal(ignored.prevented,0,"Editable fields keep native arrow behavior");
}
assert.equal(s.seekAt,beforeIgnored);

key("ArrowLeft",{ctrlKey:true});
assert.equal(s.seekAt,beforeIgnored,"OS modifier combinations remain available");
visible=false;
key("ArrowRight");
assert.equal(s.seekAt,beforeIgnored,"A hidden preview must ignore arrow seeks");
visible=true;

key("ArrowRight");
advanceTo(4000);
windowHandlers.get("blur")();
const onBlur=s.seekAt;
advanceTo(5000);
assert.equal(s.seekAt,onBlur,"Window blur must cancel an active scrub");
key("ArrowLeft");
document.hidden=true;
documentHandlers.get("visibilitychange:false")();
const onHide=s.seekAt;
advanceTo(5600);
assert.equal(s.seekAt,onHide,"Background tabs must cancel an active scrub");

console.log("Studio preview arrow seek, exponential hold, focus and blur handling: OK");
