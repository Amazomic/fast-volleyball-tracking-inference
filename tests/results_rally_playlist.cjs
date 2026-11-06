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
    this.scrollCount=0;this.scrollTop=0;this.htmlWrites=0;
  }
  set innerHTML(value){
    // Mimic native browser behavior: replacing the scroll container's
    // children loses its previous scroll position and element identity.
    this.htmlWrites++;
    if(this.id==="result-rally-list")this.scrollTop=0;
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
  constructor(){super("result-video-player");this.currentTime=0;this.duration=60;this.paused=true;this.playCalls=0;this.loadCalls=0;}
  pause(){this.paused=true;}
  load(){this.currentTime=0;this.loadCalls++;}
  play(){this.paused=false;this.playCalls++;this.fire("play");return Promise.resolve();}
}
const video=new Video();
function get(id){
  if(id==="result-video-player")return video;
  if(!nodes.has(id))nodes.set(id,new Element(id));
  return nodes.get(id);
}
const rallyA="a".repeat(24),rallyB="b".repeat(24),rallyC="c".repeat(24),rallyD="d".repeat(24);
const fixture={
  analyzed:true,analysis_offset_sec:0,duration_sec:60,media_url:"/media/test.mp4",
  rallies:[{start:0,end:10,rally_id:rallyA,player_count:3,player_track_ids:["1","2","3"]},{start:20,end:26,rally_id:rallyB,detected_start:20,detected_end:26},{start:40,end:55,rally_id:rallyC}],
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
const overlayControls=["touches","attacks","blocks","serves","digs","passes",
                       "trajectory","players"].map(key=>{
  const control=new Element("overlay-"+key);
  control.dataset.resultOverlay=key;
  control.checked=!["trajectory","players"].includes(key);
  return control;
});
const rawControls=["vballnet","ravelBall","players","touch"].map(key=>{
  const input=new Element("raw-"+key);input.dataset.resultRaw=key;input.checked=true;
  return input;
});
const document={
  getElementById:get,
  querySelector(selector){return selector===".result-confidence-options"?get("confidence-options"):null;},
  querySelectorAll(selector){
    if(selector==="[data-result-confidence]")return confidenceButtons;
    if(selector==="[data-result-overlay]")return overlayControls;
    if(selector==="[data-result-raw]")return rawControls;
    return [];
  }
};
const localStorage={
  getItem:key=>store.get(key)||null,
  setItem:(key,value)=>store.set(key,String(value))
};
const emitted=[];
let rawActive=false,touchEnabled=true,modelModeChanged=null,modelCache="";
const window={addEventListener(){},dispatchEvent(event){emitted.push(event);}};
window.VolleyballModelTouches={
  init(callback){modelModeChanged=callback;},
  setSource(source){modelCache=source.cache;},
  reset(){},update(){},active(){return rawActive;},
  setTouchVisible(value){touchEnabled=value;}
};
class CustomEvent{
  constructor(type,options){this.type=type;this.detail=options.detail;}
}
class Event{
  constructor(type){this.type=type;}
}
let rallyRefreshRequests=0;
const previewRebuild={
  analyzed:true,reconstruction_available:true,preview_only:true,
  rallies:fixture.rallies.map((r,i)=>i===1?{...r,end:28,rally_id:rallyD}:r),
  actions:fixture.actions,
  rally_summary:{raw_tracks:5,classified_tracks:3},
};
const fetch=async url=>{
  if(url.startsWith("/api/analysis-rallies?")){
    rallyRefreshRequests++;
    assert.ok(url.includes("context_sec=2"),url);
    return {ok:true,json:async()=>previewRebuild};
  }
  if(url.startsWith("/api/analysis-view?"))
    return {ok:true,json:async()=>fixture};
  if(url.startsWith("/api/analysis-player-boxes?"))
    return {ok:true,json:async()=>({available:false,frames:[]})};
  throw Error("Unexpected preview endpoint: "+url);
};
vm.runInNewContext(js,{document,window,localStorage,fetch,URLSearchParams,CustomEvent,Event,
  setTimeout,clearTimeout,console},{
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
  const initialVideoLoads=video.loadCalls;
  const initialRallyHTMLWrites=get("result-rally-list").htmlWrites;
  video.currentTime=13.5;
  // Navigating away and back with the same project/run/video must be instant.
  ui.setCacheSelection({"match.mp4":"cache-1"});
  await ui.setProject("project-1",["match.mp4"]);
  assert.equal(video.loadCalls,initialVideoLoads,"return to Preview must not reload video");
  assert.equal(video.currentTime,13.5,"return to Preview keeps playback position");
  assert.equal(get("result-rally-list").htmlWrites,initialRallyHTMLWrites,
    "cached preview must not rebuild the rally list");
  assert.deepEqual(order(),[0,1,2],"chronological list is default");
  assert.ok(get("result-rally-list").innerHTML.includes("игроков в кадре 3"),"preview displays player count");
  // Low-confidence events are retained: Preview displays already-saved
  // postprocessed events, not a second confidence-filtered event stream.
  assert.ok(get("result-rally-track").innerHTML.includes("Атака · 10%"),
    "Saved events are visible regardless of confidence");
  assert.ok(get("result-rally-track").innerHTML.includes("Приём · 20%"),
    "Even 20% detections from the saved version remain available");
  assert.equal(store.has("volley-result-confidence:project-1"),false,
    "Preview no longer persists a separate display confidence threshold");
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
  sort("touches");
  assert.deepEqual(order(),[1,0,2],"rally sorting uses saved event counts");

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

  // Visibility filters must not replace the rally list, its scroll offset,
  // current rally, or manual montage checkbox elements.
  sort("normal");
  ui.selectRally(1);
  const scrollList=get("result-rally-list");
  scrollList.scrollTop=187;
  const stableButtons=[...list()];
  const stableHtml=scrollList.innerHTML;
  const stableWrites=scrollList.htmlWrites;
  const selectedBefore=current();
  const eventsBefore=get("result-rally-track").innerHTML;
  const touches=overlayControls.find(x=>x.dataset.resultOverlay==="touches");
  const attacks=overlayControls.find(x=>x.dataset.resultOverlay==="attacks");
  touches.checked=false;touches.fire("change");
  assert.notEqual(get("result-rally-track").innerHTML,eventsBefore,
    "Timeline visibility markers respond to filter changes");
  assert.ok(!get("result-rally-track").innerHTML.includes("result-track-touch"),
    "Generic contact markers are hidden after disabling touches");
  attacks.checked=false;attacks.fire("change");
  assert.ok(!get("result-rally-track").innerHTML.includes("result-track-attack"),
    "Attack markers are hidden without modifying rally counts");
  for(const control of overlayControls.filter(x=>
      !["trajectory","players"].includes(x.dataset.resultOverlay))){
    control.checked=!control.checked;control.fire("change");
  }
  assert.equal(scrollList.scrollTop,187,"scroll position survives filter changes");
  assert.equal(scrollList.htmlWrites,stableWrites,
    "filter change never rebuilds the rally-list DOM");
  assert.equal(scrollList.innerHTML,stableHtml,"rally order/counters remain unchanged");
  assert.deepEqual(list(),stableButtons,"rally node identity is preserved");
  assert.equal(current(),selectedBefore,"active rally is not changed by filters");
  assert.equal(active().length,1,"selected rally stays highlighted");
  assert.equal(active()[0],stableButtons[1],"same rally node remains selected");

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
  const previewHtml=fs.readFileSync(path.join(__dirname,
    "../scripts/volleyball_ui/static/index.html"),"utf8");
  assert.ok(!previewHtml.includes('id="result-source-list"'),
    "Video selection is exclusively in the header chain");
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
  // The compact control cycles all four modes, without changing montage data.
  const modeButton=get("result-playback-mode");
  assert.equal(modeButton.dataset.mode,"sequence","playlist default");
  assert.equal(get("result-playback-mode-name").textContent,"По очереди");
  modeButton.fire("click");
  assert.equal(modeButton.dataset.mode,"loop");
  assert.equal(store.get("volley-result-playback-mode"),"loop");
  assert.match(modeButton.attributes["aria-label"],/повторяется по кругу/);
  ui.selectRally(0);
  nearEnd(10);
  assert.equal(current(),0,"loop must not jump to another rally");
  assert.equal(video.currentTime,0,"loop jumps to the beginning");
  assert.equal(video.paused,false);
  video.paused=true;
  video.currentTime=10;
  video.fire("ended");
  assert.equal(video.currentTime,0,"loop also restarts after native ended");
  assert.equal(video.paused,false,"loop resumes after native ended");

  modeButton.fire("click");
  assert.equal(modeButton.dataset.mode,"once");
  ui.selectRally(1);
  nearEnd(26);
  assert.equal(video.paused,true,"single rally mode pauses at its end");
  assert.equal(current(),1,"single rally does not advance");
  assert.equal(video.currentTime,26);
  video.play();
  assert.equal(current(),1,"pressing Play must retain one-shot boundaries");
  assert.equal(video.currentTime,20,"pressing Play replays the same rally");
  nearEnd(26);
  assert.equal(video.paused,true,"one-shot replay also stops at the end");
  modeButton.fire("click");
  assert.equal(modeButton.dataset.mode,"free");
  assert.equal(current(),null,"free mode detaches rally boundaries");
  ui.selectRally(0);
  assert.equal(current(),null,"clicking rally while free only seeks and plays");
  assert.equal(video.currentTime,0);
  nearEnd(10);
  assert.equal(video.paused,false,"free mode ignores rally end");
  assert.equal(video.currentTime,10);
  nearEnd(31);
  assert.equal(video.paused,false,"free mode keeps playing in gaps");

  modeButton.fire("click");
  assert.equal(modeButton.dataset.mode,"sequence","cycle returns to first mode");
  sort("normal");
  ui.selectRally(0);
  nearEnd(10);
  assert.equal(current(),1,"sequence mode auto-advances again");
  assert.equal(video.currentTime,20);
  modeButton.fire("click");
  assert.equal(modeButton.dataset.mode,"loop");
  nearEnd(26);
  assert.equal(current(),1,"changing mode while playing keeps current rally");
  assert.equal(video.currentTime,20,"changing mode takes effect at next boundary");
  assert.equal(video.paused,false);
  assert.equal(store.get("volley-result-playback-mode"),"loop");

  // Switching the provenance mode must not rebuild the selected CPU rally,
  // even though raw neural detections are drawn from the same inference.
  const modeList=get("result-rally-list");
  modeList.scrollTop=144;
  const modeWrites=modeList.htmlWrites;
  const rallyBeforeMode=current();
  const videoBeforeMode=video.currentTime;
  rawActive=true;modelModeChanged();
  assert.equal(get("result-raw-controls").hidden,false);
  assert.ok(get("result-source-explainer").textContent.includes("Нейросети"));
  assert.equal(modeList.htmlWrites,modeWrites);
  assert.equal(modeList.scrollTop,144);
  assert.equal(current(),rallyBeforeMode);
  assert.equal(video.currentTime,videoBeforeMode);
  assert.equal(modelCache,"cache-1","raw view stays tied to the original model cache");
  const rawTouch=rawControls.find(x=>x.dataset.resultRaw==="touch");
  rawTouch.checked=false;rawTouch.fire("change");
  assert.equal(touchEnabled,false,"raw Touch visibility is independent");
  rawTouch.checked=true;rawTouch.fire("change");
  assert.equal(touchEnabled,false,
    "VballNet-only raw mode must not show RAVEL Touch even if the checkbox is checked");
  assert.equal(modeList.htmlWrites,modeWrites);
  rawActive=false;modelModeChanged();
  assert.equal(get("result-raw-controls").hidden,true);
  assert.ok(get("result-source-explainer").textContent.includes("CPU:"));
  assert.equal(current(),rallyBeforeMode);
  assert.equal(modeList.scrollTop,144);

  // Changing rally grouping must request only CPU-derived boundaries,
  // without reloading video or deleting the selected rally/scroll position.
  ui.selectRally(1);
  ui.setMontageRallies([rallyB],"project-1","run-A");
  video.currentTime=24;
  const loadsBefore=video.loadCalls;
  const timeBefore=video.currentTime;
  const listBefore=get("result-rally-list");
  listBefore.scrollTop=139;
  const reloadEventsBefore=emitted.length;
  get("rally-context-sec").value="2";
  get("rally-context-sec").fire("change");
  await new Promise(resolve=>setTimeout(resolve,45));
  assert.equal(rallyRefreshRequests,1,"a setting change re-groups existing CPU tracks");
  assert.equal(video.loadCalls,loadsBefore,"no video.src reload when regrouping");
  assert.equal(video.currentTime,timeBefore,"keep the current playback frame");
  assert.equal(current(),1,"preserve active rally across boundary-only update");
  assert.deepEqual(selection("project-1","run-A"),[rallyD],
    "manual montage selection follows stable detected rally when padding changes");
  assert.ok(listBefore.innerHTML.includes('data-montage-rally="'+rallyD+'" checked'),
    "updated rally remains checked for manual montage");
  assert.equal(listBefore.scrollTop,139,"preserve rally list scroll position");
  assert.ok(listBefore.innerHTML.includes("00:00:28"),
    "updated boundary is visible in the rally list");
  assert.ok(get("result-rally-recalc-status").textContent.includes("Пересчитано"));
  assert.ok(emitted.slice(reloadEventsBefore).some(e=>e.type==="volley:rally-config-change"),
    "notify montage planning only after new preview boundaries are ready");
  get("rally-context-sec").fire("change");
  await new Promise(resolve=>setTimeout(resolve,20));
  assert.equal(rallyRefreshRequests,1,"unchanged values do not fetch again");

  ui.setCacheSelection({},"unfinished-run");
  await ui.setProject("project-1",["match.mp4","second.mp4"],"match.mp4");
  assert.ok(!previewHtml.includes('id="result-confidence-threshold"'),
    "No second confidence threshold in the preview");
  assert.ok(!previewHtml.includes('id="result-trajectory-status"'),
    "No transient trajectory-status text may shift the preview");
  assert.equal(get(".result-media-panel").hidden,false);
  assert.equal(get(".result-video-layout").hidden,true);
  assert.equal(get("result-unavailable").hidden,false);
  console.log("RESULT RALLY PLAYLIST / PER-RALLY MONTAGE / RUN PERSISTENCE: PASS");
})().catch(error=>{console.error(error);process.exitCode=1;});
