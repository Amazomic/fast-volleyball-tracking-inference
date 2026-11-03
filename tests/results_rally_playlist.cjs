"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const vm=require("node:vm");

const js=fs.readFileSync(path.join(__dirname,"../scripts/volleyball_ui/static/results-ui.js"),"utf8");
const nodes=new Map(),store=new Map();
class Classes{
  constructor(){this.items=new Set();}
  add(name){this.items.add(name);}
  remove(name){this.items.delete(name);}
  contains(name){return this.items.has(name);}
  toggle(name,on){if(on)this.add(name);else this.remove(name);}
}
class Element{
  constructor(id){
    this.id=id;this._html="";this.textContent="";this.hidden=false;
    this.value="";this.dataset={};this.classList=new Classes();this.style={};
    this.events=new Map();this.buttons=[];this.attributes={};
    this.scrollCount=0;
  }
  set innerHTML(value){
    this._html=String(value);this.buttons=[];
    for(const match of this._html.matchAll(/<button\b[^>]*data-rally="(\d+)"[^>]*>/g)){
      const child=new Element("button");child.dataset.rally=match[1];this.buttons.push(child);
    }
  }
  get innerHTML(){return this._html;}
  querySelectorAll(selector){
    return selector==="[data-rally]"||selector===".result-track-rally"?this.buttons:[];
  }
  querySelector(selector){
    if([".result-media-panel",".result-rallies-panel",".result-video-layout",
        ".analysis-video-divider",".result-timeline"].includes(selector))return get(selector);
    return null;
  }
  addEventListener(event,callback){
    const callbacks=this.events.get(event)||[];callbacks.push(callback);this.events.set(event,callbacks);
  }
  fire(event,custom={}){
    for(const callback of this.events.get(event)||[])callback({target:this,currentTarget:this,...custom});
  }
  closest(selector){
    if(selector==="[data-result-confidence]"&&this.dataset.resultConfidence!==undefined)return this;
    if(selector==="[data-montage-rally]"&&this.dataset.montageRally!==undefined)return this;
    return null;
  }
  setAttribute(k,v){this.attributes[k]=v;}
  removeAttribute(k){delete this.attributes[k];}
  replaceChildren(){this.buttons=[];}
  scrollIntoView(){this.scrollCount++;}
}
class Video extends Element{
  constructor(){super("result-video-player");this.currentTime=0;this.duration=60;this.paused=true;this.playCalls=0;}
  pause(){this.paused=true;}
  load(){this.currentTime=0;}
  play(){this.paused=false;this.playCalls++;this.fire("play");return Promise.resolve();}
}
const video=new Video();
function get(id){
  if(id==="result-video-player")return video;
  if(!nodes.has(id))nodes.set(id,new Element(id));
  return nodes.get(id);
}
const rallyA="a".repeat(24),rallyB="b".repeat(24),rallyC="c".repeat(24);
const fixture={
  analyzed:true,analysis_offset_sec:0,duration_sec:60,media_url:"/media/test.mp4",
  rallies:[{start:0,end:10,rally_id:rallyA,player_count:3,player_track_ids:["1","2","3"]},{start:20,end:26,rally_id:rallyB},{start:40,end:55,rally_id:rallyC}],
  actions:[
    {time:1,type:"serve",confidence:.99},{time:9,type:"dig",confidence:.2},
    {time:20,type:"serve",confidence:.95},{time:22,type:"attack",confidence:.1},
    {time:24,type:"dig",confidence:.85},{time:26,type:"low_touch",confidence:.9},
    {time:45,type:"block",confidence:.7}
  ]
};
const confidenceButtons=["0","0.5","0.65","0.8"].map(value=>{
  const button=new Element("confidence-"+value);button.dataset.resultConfidence=value;
  return button;
});
const document={
  getElementById:get,
  querySelector(selector){return selector===".result-confidence-options"?get("confidence-options"):null;},
  querySelectorAll(selector){return selector==="[data-result-confidence]"?confidenceButtons:[];}
};
const localStorage={
  getItem:key=>store.get(key)||null,
  setItem:(key,value)=>store.set(key,String(value))
};
const emitted=[];
const window={addEventListener(){},dispatchEvent(event){emitted.push(event);}};
class CustomEvent{
  constructor(type,options){this.type=type;this.detail=options.detail;}
}
const fetch=async url=>{
  assert.ok(url.startsWith("/api/analysis-view?"),url);
  return {ok:true,json:async()=>fixture};
};
vm.runInNewContext(js,{document,window,localStorage,fetch,URLSearchParams,CustomEvent,console},{
  filename:"results-ui.js"
});
const ui=window.VolleyballResultsUI;
const list=()=>get("result-rally-list").buttons;
const order=()=>list().map(row=>Number(row.dataset.rally));
const current=()=>ui.activeRally()?.index??null;
const active=()=>list().filter(button=>button.classList.contains("active"));
function sort(value){
  get("result-rally-sort").value=value;
  get("result-rally-sort").fire("change");
}
function nearEnd(t){video.currentTime=t;video.fire("timeupdate");}
(async()=>{
  ui.init();ui.setCacheSelection({"match.mp4":"cache-1"});
  await ui.setProject("project-1",["match.mp4"]);
  assert.deepEqual(order(),[0,1,2],"chronological list is default");
  assert.ok(get("result-rally-list").innerHTML.includes("игроков 3"),"preview displays player count");
  assert.equal(confidenceButtons[0].attributes["aria-pressed"],"true");
  get("confidence-options").fire("click",{target:confidenceButtons[2]});
  assert.equal(store.get("volley-result-confidence:project-1"),"0.65");
  assert.equal(get("result-confidence-threshold").value,"0.65");
  assert.equal(confidenceButtons[2].attributes["aria-pressed"],"true");
  assert.equal(confidenceButtons[0].attributes["aria-pressed"],"false");
  get("confidence-options").fire("click",{target:confidenceButtons[0]});
  ui.selectRally(0);
  assert.equal(current(),0);
  assert.equal(active().length,1);
  assert.equal(active()[0].dataset.rally,"0");
  assert.equal(active()[0].attributes["aria-current"],"true");
  assert.equal(get("result-rally-track").buttons[0].classList.contains("active"),true);
  nearEnd(10);
  assert.equal(current(),1,"next chronological rally starts automatically");
  assert.equal(video.currentTime,20);
  assert.equal(video.paused,false);
  assert.equal(active()[0].dataset.rally,"1");
  nearEnd(26);
  assert.equal(current(),2);
  nearEnd(55);
  assert.equal(video.paused,true,"stop after last rally only");
  assert.equal(current(),2);
  assert.equal(active()[0].dataset.rally,"2");
  nearEnd(55);
  assert.equal(video.paused,true,"do not loop after end");

  sort("duration");
  assert.deepEqual(order(),[2,0,1],"longest first, stable ties");
  ui.selectRally(2);
  nearEnd(55);
  assert.equal(current(),0,"after latest timeline point, next follows sorted list");
  assert.equal(video.currentTime,0);

  sort("touches");
  assert.deepEqual(order(),[1,0,2],"most contacts first");
  assert.ok(get("result-rally-list").innerHTML.includes("4 касаний"));
  assert.equal(current(),0,"sorting preserves currently playing rally");
  nearEnd(10);
  assert.equal(current(),2,"next item in changed sort order");
  assert.equal(active().length,1);

  sort("normal");
  ui.selectRally(0);
  get("result-confidence-threshold").value=".8";
  get("result-confidence-threshold").fire("change");
  sort("touches");
  assert.deepEqual(order(),[1,0,2],"touches unaffected by confidence filter");

  // Native video ended event can fire after the browser has already paused.
  sort("duration");
  ui.selectRally(2);
  video.paused=true;
  video.fire("ended");
  assert.equal(current(),0,"ended event still advances from selected last video frame");
  assert.equal(video.paused,false);

  ui.selectRally(0);
  video.seeking=true;
  video.currentTime=42;
  video.fire("timeupdate");
  assert.equal(current(),0,"do not skip rallies during an in-progress seek");
  video.seeking=false;
  video.fire("seeked");
  assert.equal(current(),null,"manual seek outside selected rally clears playlist mode");
  assert.equal(active().length,0);

  const oldCalls=video.playCalls;
  sort("touches");
  ui.selectRally(2);
  video.paused=true;
  video.fire("ended");
  assert.equal(video.paused,true,"last item does not restart");
  assert.equal(video.playCalls,oldCalls+1);
  await ui.setVideo("match.mp4");
  assert.equal(current(),null,"changing results resets playlist");
  assert.equal(list().filter(row=>row.classList.contains("active")).length,0);
  assert.equal(store.get("volley-result-rally-sort"),"touches");
  // Return values created in a VM realm must be copied for strict equality.
  const selection=(pid,run)=>Array.from(ui.montageRallies(pid,run));
  // Rally selections survive project/run switching and do not mark whole videos.
  ui.setCacheSelection({"match.mp4":"cache-1","second.mp4":"cache-2"},"run-A");
  await ui.setProject("project-1",["match.mp4","second.mp4"],"match.mp4");
  assert.deepEqual(selection("project-1","run-A"),[]);
  assert.ok(!get("result-source-list").innerHTML.includes("data-montage-video="),
    "source cards are for playback, not manual montage selection");
  assert.ok(get("result-rally-list").innerHTML.includes('data-montage-rally="'+rallyA+'"'));
  const checkbox=new Element("rally-selection");
  checkbox.dataset.montageRally=rallyA;
  checkbox.checked=true;
  get("result-rally-list").fire("change",{target:checkbox});
  assert.deepEqual(selection("project-1","run-A"),[rallyA]);
  assert.equal(emitted.at(-1).type,"volley:montage-rallies-change");
  assert.equal(emitted.at(-1).detail.run_id,"run-A");
  assert.equal(emitted.at(-1).detail.project_id,"project-1");
  ui.setCacheSelection({"match.mp4":"cache-1","second.mp4":"cache-2"},"run-B");
  await ui.setProject("project-1",["match.mp4","second.mp4"],"match.mp4");
  assert.deepEqual(selection("project-1","run-B"),[],
    "separate analysis runs must not share selected rally IDs");
  ui.setCacheSelection({"match.mp4":"cache-1","second.mp4":"cache-2"},"run-A");
  await ui.setProject("project-1",["match.mp4","second.mp4"],"match.mp4");
  assert.ok(get("result-rally-list").innerHTML.includes(
    'data-montage-rally="'+rallyA+'" checked'),
    "selected rally checkbox must be restored on return");
  ui.setMontageRallies([rallyB],"project-1","run-A");
  assert.deepEqual(selection("project-1","run-A"),[rallyB]);
  ui.setMontageRallies([],"project-1","run-A");
  assert.deepEqual(selection("project-1","run-A"),[],
    "explicit zero selections do not default to all rallies");
  ui.setCacheSelection({},"unfinished-run");
  await ui.setProject("project-1",["match.mp4","second.mp4"],"match.mp4");
  assert.ok(!get("result-source-list").innerHTML.includes("data-montage-video="));
  assert.equal(get(".result-media-panel").hidden,false);
  assert.equal(get(".result-video-layout").hidden,true);
  assert.equal(get("result-unavailable").hidden,false);
  console.log("RESULT RALLY PLAYLIST / PER-RALLY MONTAGE / RUN PERSISTENCE: PASS");
})().catch(error=>{console.error(error);process.exitCode=1;});
