"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const vm=require("node:vm");
const js=fs.readFileSync(path.join(__dirname,
  "../scripts/volleyball_ui/static/app.js"),"utf8")
  .split('document.addEventListener("DOMContentLoaded",async()=>{')[0];
const nodes=new Map(),store=new Map();
function el(selector){
  if(!nodes.has(selector))nodes.set(selector,{
    hidden:false,disabled:false,value:"",checked:false,
    innerHTML:"",textContent:"",selectedOptions:[{textContent:"Лучшие моменты"}],
    replaceChildren(){this.innerHTML="";},
    querySelectorAll(){return [];},
    checkValidity(){return true;},
  });
  return nodes.get(selector);
}
class FormDataMock{
  entries(){return Object.entries({
    mode:"montage",duration:"120",selection_strategy:"highlights",
    min_rally:"1.5",min_confidence:"0.5",min_actions:"0",
    min_attack:"0",min_block:"0",min_dig:"0",
    min_overhead_pass:"0",min_serve:"0",
  })[Symbol.iterator]();}
}
const document={querySelector:el,querySelectorAll(){return [];},
  getElementById(id){return el("#"+id);}};
const localStorage={getItem:k=>store.get(k)||null,
  setItem:(k,v)=>store.set(k,String(v))};
const window={addEventListener(){},dispatchEvent(){}};
window.VolleyballResultsUI={
  montageRallies:(pid,run)=>JSON.parse(store.get("volley-montage-rallies:"+pid+":"+run)||"[]"),
  setMontageRallies:(ids,pid,run)=>store.set(
    "volley-montage-rallies:"+pid+":"+run,JSON.stringify(ids))
};
const context=vm.createContext({document,window,FormData:FormDataMock,
  localStorage,URL,URLSearchParams,console,setTimeout,clearTimeout});
vm.runInContext(js,context,{filename:"app.js"});
function run(script){return vm.runInContext(script,context);}
run('state.project="project-one"; state.analysisRunId="run-one";');
run('selectedAnalysisRun=()=>({id:state.analysisRunId,profile:"Profile A"})');
el("#montage-follow-ball").checked=true;
const a="a".repeat(24),b="b".repeat(24);
const candidates=[
  {rally_id:a,video:"match.mp4",start:0,end:12.5,duration:12.5,
    joined_parts:1,confidence:.9,score:8,player_count:4,actions_total:5,action_counts:{}},
  {rally_id:b,video:"match.mp4",start:20,end:40.25,duration:20.25,
    joined_parts:1,confidence:.8,score:7,actions_total:4,action_counts:{}}
];
const requests=[];
context.testApi=async(url,params)=>{
  assert.equal(url,"/api/montage/plan");
  requests.push(params);
  if(params.rally_selection_mode==="manual"){
    const selected=candidates.filter(c=>params.selected_rally_ids.includes(c.rally_id));
    const seconds=selected.reduce((n,c)=>n+c.duration,0);
    return {candidates,selected,selected_duration:seconds,requested_duration:seconds,
      source_count:1};
  }
  return {candidates,selected:[candidates[0]],selected_duration:12.5,
    requested_duration:120,source_count:1};
};
run("api=testApi");
(async()=>{
  el("#montage-selection-mode").value="manual";
  run("syncMontageSelectionMode()");
  assert.equal(el("#montage-auto-duration").hidden,true);
  assert.equal(el("#montage-auto-strategy").hidden,true);
  assert.equal(el("#montage-manual-hint").hidden,false);
  await run("refreshMontagePlan()");
  assert.equal(el("#montage-create").disabled,true);
  assert.ok(el("#montage-plan-list").innerHTML.includes('data-montage-rally="'+a+'"'));
  assert.ok(el("#montage-plan-list").innerHTML.includes("is-editable"));
  assert.ok(el("#montage-plan-list").innerHTML.includes("Игроков 4"));
  const callsBeforeSelection=requests.length;
  run('montageManualSelected.add("'+a+'")');
  run("updateManualRallies()");
  assert.equal(requests.length,callsBeforeSelection,
    "checking one rally should update instantly without network request");
  assert.deepEqual(JSON.parse(store.get("volley-montage-rallies:project-one:run-one")),[a],
    "montage checkbox selection is also available in Preview");
  assert.equal(el("#montage-create").disabled,false);
  assert.ok(el("#montage-plan-summary").textContent.includes("12.50 с"));
  assert.equal(run("montageChosenRows(montagePayload()).length"),1);
  run('montageManualSelected.add("'+b+'")');
  run("updateManualRallies()");
  assert.ok(el("#montage-plan-summary").textContent.includes("32.75 с"));
  assert.equal(requests.length,callsBeforeSelection);
  assert.equal(run("montagePayload().selected_rally_ids.length"),2);
  run('state.analysisRunId="run-two"');
  await run("refreshMontagePlan()");
  assert.equal(el("#montage-create").disabled,true,
    "changing the source analysis clears stale checks");
  assert.equal(requests.at(-1).selected_rally_ids.length,0);
  run('state.analysisRunId="run-one"');
  await run("refreshMontagePlan()");
  assert.equal(run("montagePayload().selected_rally_ids.length"),2,
    "returning to an analysis run restores checked rallies");
  el("#montage-selection-mode").value="auto";
  run("syncMontageSelectionMode()");
  await run("refreshMontagePlan()");
  assert.equal(el("#montage-auto-duration").hidden,false);
  assert.equal(el("#montage-create").disabled,false);
  assert.equal(requests.at(-1).rally_selection_mode,"auto");
  const autoHTML=el("#montage-plan-list").innerHTML;
  assert.ok(autoHTML.includes("is-readonly"),"automatic mode is read-only");
  assert.ok(autoHTML.includes("Розыгрыш №2"),"automatic mode lists non-selected rallies");
  assert.ok(!autoHTML.includes('data-montage-rally="'),
    "automatic mode has no selectable checkboxes");
  assert.ok(autoHTML.includes('data-montage-preview="'+a+'"'),
    "every rally can be clicked to preview it in the montage tab");
  run('state.projects=[{id:"project-one",videos:["match.mp4"]}]');
  run('selectedAnalysisRun=()=>({id:"run-one",profile:"Profile A",videos:{"match.mp4":"cache"}})');
  const previewVideo=el("#preview-player");
  previewVideo.style={};previewVideo.src="";
  el("#preview-empty").style={};
  previewVideo.pause=()=>{};
  previewVideo.getAttribute=name=>name==="src"?previewVideo.src:null;
  previewVideo.load=()=>{};
  let played=null;
  window.VolleyballMontagePlayer={
    playRally:(start,end)=>{played={start,end}},
    setRallyRange:()=>{}
  };
  run('previewMontageRally("'+a+'")');
  assert.deepEqual(played,{start:0,end:12.5},
    "clicking the chosen rally plays its exact time interval");
  assert.ok(previewVideo.src.includes("/api/video-media?"),
    "reuses project video preview, no new analysis or montage");
  assert.ok(previewVideo.src.includes("match.mp4"));
  assert.ok(el("#preview-title").textContent.includes("Розыгрыш №1"));
  assert.equal(run("montageActiveRallyId"),a);
  console.log("MONTAGE HORIZONTAL CATALOGUE / MANUAL / AUTO / RALLY PREVIEW: PASS");
})().catch(error=>{console.error(error);process.exitCode=1;});
