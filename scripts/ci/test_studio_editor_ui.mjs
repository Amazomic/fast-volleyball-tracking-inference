import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import vm from "node:vm";
import {fileURLToPath} from "node:url";
import {join} from "node:path";

const root=fileURLToPath(new URL("../..",import.meta.url));
const html=readFileSync(join(root,"rust/volley-app/static/studio.html"),"utf8");
const js=readFileSync(join(root,"rust/volley-app/static/studio_editor.js"),"utf8");
const css=readFileSync(join(root,"rust/volley-app/static/studio.css"),"utf8");

assert.match(html,/id="studio-analysis-player"[\s\S]*id="studio-analysis-court"[\s\S]*class="analysis-editor-timeline"/);
assert.match(html,/id="studio-analysis-selected-range"/);
assert.match(html,/id="studio-analysis-zoom-out"/);
assert.match(html,/id="studio-analysis-zoom-in"/);
assert.match(css,/\.analysis-editor-media\{[^}]*--frame-ratio/);
assert.match(css,/\.analysis-editor-stage\.marking/);

const nodes=new Map();
const ids=[...html.matchAll(/id="([^"]+)"/g)].map(m=>m[1]);
function el(id){
  if(nodes.has(id))return nodes.get(id);
  const listeners=new Map(),classes=new Set();
  const node={
    id,value:"0",textContent:"",disabled:false,controls:true,paused:true,
    style:{setProperty(key,value){this[key]=value;}},
    classList:{
      toggle(name,on){if(on)classes.add(name);else classes.delete(name);},
      contains(name){return classes.has(name);}
    },
    addEventListener(name,handler){listeners.set(name,[...(listeners.get(name)||[]),handler]);},
    async fire(name,event={}){for(const fn of listeners.get(name)||[])await fn(event);},
    setAttribute(){},removeAttribute(){},setPointerCapture(){},
    getBoundingClientRect(){
      if(id==="studio-analysis-frame"){
        const zoom=Number(el("studio-analysis-stage").style["--editor-zoom"]||1);
        return {left:(1000-1000*zoom)/2,top:(562.5-562.5*zoom)/2,
          width:1000*zoom,height:562.5*zoom};
      }
      return {left:0,top:0,width:1000,height:562.5};
    },
    querySelectorAll(){return [...nodes.values()];}
  };
  nodes.set(id,node);
  return node;
}
ids.forEach(el);
const ctx=new Proxy({}, {get(){return ()=>{};},set(){return true;}});
el("studio-analysis-court").getContext=()=>ctx;
el("studio-analysis-stage").parentElement=el("studio-analysis-media-fixture");
const video=el("studio-analysis-player");
video.currentTime=0;
video.duration=3600;
video.videoWidth=1920;
video.videoHeight=1080;
video.pause=function(){this.paused=true;void this.fire("pause");};
video.play=function(){this.paused=false;void this.fire("play");return Promise.resolve();};
video.load=function(){};
let saved=null,posts=0;
const windowMock={
  devicePixelRatio:1,
  ResizeObserver:class{observe(){}},
  addEventListener(){},
  VolleyballStudioActivity:{
    async fetch(path,options={}){
      const data=options.method==="POST"?(posts++,saved=JSON.parse(options.body),saved):
        saved||{trim_start_sec:0,trim_end_sec:null,court:[]};
      return {ok:true,json:async()=>data};
    }
  }
};
const drafts=new Map();
vm.runInNewContext(js,{
  localStorage:{getItem:k=>drafts.get(k)||null,setItem:(k,v)=>drafts.set(k,v),removeItem:k=>drafts.delete(k)},
  setTimeout,clearTimeout,
  document:{getElementById:el},
  window:windowMock,
  URLSearchParams,
  ResizeObserver:windowMock.ResizeObserver,
  console
},{filename:"studio_editor.js"});

await windowMock.VolleyballEditor.load("project-1","match.mp4");
await video.fire("loadedmetadata");
assert.equal(el("studio-analysis-seek").max,"3600000");
assert.equal(el("studio-analysis-trim-end").value,"3600.00");

el("studio-analysis-seek").value="150000";
await el("studio-analysis-seek").fire("input");
await el("studio-analysis-set-start").fire("click");
assert.equal(el("studio-analysis-trim-start").value,"150.00");
assert.ok(Math.abs(parseFloat(el("studio-analysis-start-marker").style.left)-150/3600*100)<0.001);

el("studio-analysis-seek").value="300000";
await el("studio-analysis-seek").fire("input");
await el("studio-analysis-set-end").fire("click");
assert.equal(el("studio-analysis-trim-end").value,"300.00");
assert.ok(el("studio-analysis-range-label").textContent.includes("05:00.00"));

await el("studio-analysis-court-mode").fire("click");
assert.equal(el("studio-analysis-stage").style["--editor-zoom"],"0.84");
assert.equal(video.controls,false);
await el("studio-analysis-zoom-out").fire("click");
assert.equal(el("studio-analysis-zoom-label").textContent,"76%");
await el("studio-analysis-zoom-in").fire("click");
assert.equal(el("studio-analysis-zoom-label").textContent,"84%");

// At 84%, the actual image begins at x=80. The x=30 mark is off-frame.
const canvas=el("studio-analysis-court");
for(const [x,y] of [[30,160],[330,160],[770,350],[170,430]]){
  await canvas.fire("pointerdown",{clientX:x,clientY:y,pointerId:1});
  await canvas.fire("pointerup",{pointerId:1});
}
await el("studio-analysis-edit-save").fire("click");
assert.equal(posts,1);
assert.equal(saved.court.length,4);
assert.ok(saved.court[0][0]<0,"left off-frame point must remain negative");
assert.ok(saved.court[0][0]>=-0.5);
assert.equal(saved.trim_start_sec,150);
assert.equal(saved.trim_end_sec,300);
assert.ok(saved.court.every(([x,y])=>x>=-0.5&&x<=1.5&&y>=-0.5&&y<=1.5));

await el("studio-analysis-court-mode").fire("click");
assert.equal(video.controls,true);
assert.equal(el("studio-analysis-stage").style["--editor-zoom"],"1");
assert.equal(el("studio-analysis-zoom-in").disabled,true);
assert.equal(el("studio-analysis-zoom-label").textContent,"100%");
await windowMock.VolleyballEditor.load("project-1","match.mp4");
await video.fire("loadedmetadata");
assert.equal(el("studio-analysis-trim-start").value,"150");
assert.equal(el("studio-analysis-trim-end").value,"300");
await el("studio-analysis-edit-save").fire("click");
assert.equal(posts,2);
assert.equal(saved.court.length,4);

console.log("PASS: video timeline, cursor trim markers, adjustable marking zoom, off-frame normalized points and source-specific persistence");

el("studio-analysis-trim-start").value="160";
await el("studio-analysis-trim-start").fire("input");
await windowMock.VolleyballEditor.load("project-2","other.mp4");
await windowMock.VolleyballEditor.load("project-1","match.mp4");
assert.equal(el("studio-analysis-trim-start").value,"160","Unsaved trim survives source switch and reload");
assert.equal(posts,2,"Draft must not save server edits automatically");
saved={...saved,trim_start_sec:170};
await windowMock.VolleyballEditor.load("project-1","match.mp4");
assert.equal(el("studio-analysis-trim-start").value,"170","Changed server revision invalidates old draft");
console.log("PASS: editor drafts persist only metadata and reject stale server revisions");
