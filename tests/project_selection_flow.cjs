"use strict";
// The actual app.js project flow, executed with controlled asynchronous APIs.
// No project on disk and no real browser are modified.
const assert=require("node:assert/strict");
const choice=(video,id)=>encodeURIComponent(video)+"|"+id;
const fs=require("node:fs");
const vm=require("node:vm");
const path=require("node:path");
const app=fs.readFileSync(path.join(__dirname,"../scripts/volleyball_ui/static/app.js"),"utf8")
  .split('document.addEventListener("DOMContentLoaded",async()=>{')[0];
const nodes=new Map(),store=new Map(),calls=[];
function element(selector){
  if(!nodes.has(selector)){
    nodes.set(selector,{
      innerHTML:"",textContent:"",hidden:false,disabled:false,value:"",open:true,
      style:{},dataset:{},classList:{toggle(){},add(){},remove(){},contains(){return false}},
      setAttribute(){},replaceChildren(){this.options=[];},
      add(option){(this.options??=[]).push(option);},
      close(){this.open=false;},pause(){},removeAttribute(){},load(){},
      querySelector(){return null;},
    });
  }
  return nodes.get(selector);
}
const document={
  querySelector:element,
  getElementById(id){return element("#"+id);},
  querySelectorAll(selector){
    if(selector===".dialog-video-row")return this.selectedDialogRows||[];
    return [];
  }
};
const localStorage={
  getItem(key){return store.get(key)||null;},
  setItem(key,val){store.set(key,String(val));},
  removeItem(key){store.delete(key);}
};
const ranges=[];
const window={
  VolleyballHeaderChain:require("../scripts/volleyball_ui/static/header-chain.js"),
  VolleyballAnalysisUI:{
    setProjectRanges(map,id){ranges.push({id,range:map});},
    async setVideo(video){calls.push(["video",video]);}
  },
  VolleyballResultsUI:{setCacheSelection(){},async setProject(){}},
  VolleyballCourtUI:{reset(){}},
  addEventListener(){},dispatchEvent(){},
  scrollTo(){},
};
const context=vm.createContext({
  window,document,localStorage,Option:function(text,value){return {text,value};},
  URL,URLSearchParams,console,setTimeout,clearTimeout,
  CustomEvent:function(){},Math,Date,Number,String,Map,Set,
});
vm.runInContext(app,context,{filename:"app.js"});
const state=vm.runInContext("state",context);
window.VolleyballAutoSave={async flush(){calls.push(["autosave",state.project]);}};
context.reportProject=null;
vm.runInContext(`
  renderSidebarActivity=()=>{};
  renderReport=()=>{};
  renderRenders=()=>{};
  displayAnalysisContext=()=>{};
  scheduleMontagePlan=()=>{};
  loadReport=async()=>{reportProject=state.project;};
  loadRenders=async()=>{};
  loadBenchmarkResults=async()=>{};
  loadBenchmarkModels=async()=>{};
  refreshAnalysisContext=async()=>{};
  loadAnalysisHistory=async()=>{};
  refreshVideoPreparation=async()=>{};
  go=(view)=>{lastView=view;};
`,context);
const a={id:"aaa111",name:"Match A",videos:["a.mp4"],ranges:{"a.mp4":{start:0,end:6}}};
const b={id:"bbb222",name:"Match B",videos:["b.mp4"],ranges:{"b.mp4":{start:1,end:7}}};
const c={id:"ccc333",name:"Match C",videos:["c.mp4"],ranges:{"c.mp4":{start:2,end:8}}};
const projects=[a,b];
const runA={id:"run_a",profile:"Global preset",started_at:"2026-11-02T11:00:00Z",videos:{"a.mp4":"cacheA"}};
const runB={id:"run_b",profile:"Global preset",started_at:"2026-11-02T11:10:00Z",videos:{"b.mp4":"cacheB"}};
const runB2={id:"run_b2",profile:"Accurate preset",started_at:"2026-11-02T12:00:00Z",videos:{"b.mp4":"cacheB2"}};
let runsB=[runB];
let pendingProjects=null;
context.globalApi=async(url,payload)=>{
  if(url==="/api/projects"){
    if(pendingProjects){const previous=pendingProjects;pendingProjects=null;return previous;}
    return {projects:[...projects]};
  }
  if(url.startsWith("/api/analysis-runs?")){
    const id=new URL("http://localhost"+url).searchParams.get("project_id");
    return {runs:id===a.id?[runA]:id===b.id?runsB:[]};
  }
  if(url==="/api/project-create"){
    assert.equal(payload.name,"Match C");
    assert.equal(payload.videos[0],"c.mp4");
    projects.push(c);
    return {project:c};
  }
  throw Error("unexpected API: "+url);
};
vm.runInContext("api=globalApi",context);
function call(code){return vm.runInContext(code,context);}
(async()=>{
  assert.equal(await call("loadAll()"),true);
  assert.equal(state.project,a.id);
  assert.equal(state.analysisRunId,runA.id);
  assert.equal(element("#chain-project").options.find(row=>row.value===state.project).text,"Match A");

  assert.equal(await call('loadAll("bbb222")'),true);
  assert.equal(state.project,b.id);
  assert.equal(state.video,"b.mp4");
  assert.equal(state.analysisRunId,runB.id);
  assert.equal(context.reportProject,b.id);
  assert.equal(element("#chain-project").options.find(row=>row.value===state.project).text,"Match B");
  assert.equal(element("#top-project").value,b.id);
  assert.deepEqual(element("#chain-inference").options.map(row=>row.value),[choice("b.mp4",runB.id)]);
  assert(element("#chain-inference").options[0].text.includes("Global preset"));
  assert.equal(element("#chain-inference").value,choice("b.mp4",runB.id));
  assert.equal(element("#chain-postprocess").value,runB.id);
  assert.equal(store.get("volley-selected-project"),b.id);
  assert.equal(ranges.at(-1).id,b.id);

  // Reload in a fresh project selection state preserves the last choice.
  state.project="";
  assert.equal(await call("loadAll()"),true);
  assert.equal(state.project,b.id);
  assert.equal(state.analysisRunId,runB.id);

  // The server-confirmed newly created project becomes the active selection.
  element("#new-project-name").value="Match C";
  document.selectedDialogRows=[{
    dataset:{video:"c.mp4"},
    querySelector(){return {checked:true};}
  }];
  await call("saveProjectDialog()");
  assert.equal(state.project,c.id);
  assert.equal(state.video,"c.mp4");
  assert.equal(state.analysisRuns.length,0);
  assert.equal(state.analysisRunId,"");
  assert.equal(element("#chain-project").options.find(row=>row.value===state.project).text,"Match C");
  assert.equal(store.get("volley-selected-project"),c.id);
  assert.equal(context.reportProject,c.id);
  assert.equal(ranges.at(-1).id,c.id);
  assert.deepEqual(element("#chain-inference").options.map(row=>row.value),[""]);
  assert.equal(element("#chain-inference").disabled,true);
  assert.equal(element("#project-dialog").open,false);

  // A delayed previous response must never steal selection from a later click.
  let release;
  pendingProjects=new Promise(resolve=>{release=resolve;});
  const stale=call('loadAll("aaa111")');
  const current=call('loadAll("bbb222")');
  assert.equal(await current,true);
  release({projects:[...projects]});
  assert.equal(await stale,false);
  assert.equal(state.project,b.id);
  assert.equal(state.analysisRunId,runB.id);
  assert.equal(store.get("volley-selected-project"),b.id);
  assert.equal(element("#chain-project").options.find(row=>row.value===state.project).text,"Match B");

  // The header selector switches the analysis branch for all dependent views.
  runsB=[runB2,runB];
  await call("loadAnalysisRuns()");
  assert.deepEqual(element("#chain-inference").options.map(row=>row.value),[choice("b.mp4",runB2.id),choice("b.mp4",runB.id)]);
  await call('chooseAnalysisRun("run_b2")');
  assert.equal(state.analysisRunId,runB2.id);
  assert.equal(call('analysisCacheId("b.mp4")'),"cacheB2");
  assert.equal(store.get("volley-analysis-run:"+b.id),runB2.id);
  assert.equal(element("#chain-inference").value,choice("b.mp4",runB2.id));
  assert.equal(element("#chain-postprocess").value,runB2.id);
  assert(element("#chain-inference").options[0].text.includes("Accurate preset"));
  assert(element("#chain-inference").options[0].title.includes("run_b2"));

  // Each project preserves its own run selection, even when profiles share names.
  assert.equal(await call('loadAll("aaa111")'),true);
  assert.equal(state.analysisRunId,runA.id);
  assert.equal(await call('loadAll("bbb222")'),true);
  assert.equal(state.analysisRunId,runB2.id);
  assert.equal(element("#chain-inference").value,choice("b.mp4",runB2.id));
  assert.equal(store.get("volley-analysis-run:"+a.id),runA.id);
  console.log("PROJECT -> HEADER RUN -> PREVIEW CACHE -> RESTORE PER PROJECT: PASS");
})().catch(error=>{console.error(error);process.exitCode=1;});
