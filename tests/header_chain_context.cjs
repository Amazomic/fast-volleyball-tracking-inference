"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs"),vm=require("node:vm"),path=require("node:path");
const model=require("../scripts/volleyball_ui/static/header-chain.js");
const src=fs.readFileSync(path.join(__dirname,"../scripts/volleyball_ui/static/app.js"),"utf8")
  .split('document.addEventListener("DOMContentLoaded",async()=>{')[0];
const elements=new Map(),stored=new Map(),calls=[];
function element(selector){
  if(!elements.has(selector))elements.set(selector,{
    value:"",textContent:"",innerHTML:"",hidden:false,disabled:false,
    style:{},dataset:{},classList:{toggle(){},add(){},remove(){},contains(){return false}},
    replaceChildren(){this.options=[]},add(o){(this.options??=[]).push(o)},
    setAttribute(){},querySelector(){return null},pause(){},load(){},removeAttribute(){}
  });
  return elements.get(selector);
}
const runSelector={value:"",options:[],replaceChildren(){this.options=[]},add(o){this.options.push(o)}};
const document={
  querySelector:element,getElementById(id){return element("#"+id)},
  querySelectorAll(selector){return selector==="[data-analysis-run-select]"?[runSelector]:[]}
};
const localStorage={
  getItem(key){return stored.get(key)||null},
  setItem(key,value){stored.set(key,String(value))},
  removeItem(key){stored.delete(key)}
};
const window={
  VolleyballHeaderChain:model,
  VolleyballAnalysisUI:{setProjectRanges(){},async setVideo(name){calls.push(["video",name])}},
  VolleyballResultsUI:{setCacheSelection(){},async setProject(){}},
  VolleyballAutoSave:{async flush(){calls.push(["flush"])}},
  VolleyballCourtUI:{reset(){}},
  addEventListener(){},dispatchEvent(){},scrollTo(){}
};
const ctx=vm.createContext({window,document,localStorage,console,URL,URLSearchParams,
  Option:function(text,value){return {text,value}},setTimeout,clearTimeout,
  Math,Date,Number,String,Map,Set});
vm.runInContext(src,ctx,{filename:"app.js"});
const state=vm.runInContext("state",ctx);
for(const name of ["renderSidebarActivity","renderReport","renderRenders",
  "displayAnalysisContext","scheduleMontagePlan","loadReport","loadRenders",
  "loadBenchmarkResults","loadBenchmarkModels","refreshAnalysisContext",
  "loadAnalysisHistory","refreshVideoPreparation"]){
  vm.runInContext(name+"=async()=>{};",ctx);
}
const p={id:"project1",name:"Tournament",videos:["first.mp4","second.mp4"],ranges:{}};
const q={id:"project2",name:"Training",videos:["third.mp4"],ranges:{}};
function orig(id,profile,start,end){
  return {id,profile,started_at:"2026-11-04T11:00:00Z",
    videos:{"first.mp4":id+"a","second.mp4":id+"b"},
    video_ranges:{"first.mp4":{start,end},"second.mp4":{start:0,end:0}}};
}
const base1=orig("base1","Short replay",0,50);
const base2=orig("base2","Later fragment",150,200);
const processed1={...base1,id:"base1_pp_aa",parent_run_id:"base1",
  postprocess_settings:{mode:"combined",touch_threshold:.65}};
const processed2={...base2,id:"base2_pp_bb",parent_run_id:"base2",
  postprocess_settings:{mode:"ravel-touch",touch_threshold:.8}};
const runQ={id:"run_q",profile:"Local",started_at:"2026-11-04T12:00:00Z",
  videos:{"third.mp4":"qcache"}};
const projectRuns={project1:[processed2,base2,processed1,base1],project2:[runQ]};
ctx.mockApi=async(url)=>{
  if(url==="/api/projects")return {projects:[p,q]};
  if(url.startsWith("/api/analysis-runs?")){
    const id=new URL("http://local"+url).searchParams.get("project_id");
    return {runs:projectRuns[id]||[]};
  }
  throw Error("Unexpected "+url);
};
vm.runInContext("api=mockApi",ctx);
const evaluate=code=>vm.runInContext(code,ctx);
(async()=>{
  assert.equal(await evaluate("loadAll()"),true);
  assert.equal(state.video,"first.mp4");
  assert.equal(state.analysisRunId,"base2");
  assert.equal(element("#chain-inference").options.length,4);
  assert.equal(element("#chain-postprocess").options.length,2);
  assert(element("#chain-inference").options[0].text.includes("00:02:30"));
  await evaluate('chooseAnalysisRun("base1_pp_aa")');
  assert.equal(element("#chain-inference").value,model.choiceId("first.mp4","base1"));
  assert.equal(element("#chain-postprocess").value,"base1_pp_aa");
  assert.equal(evaluate('analysisCacheId("first.mp4")'),"base1a");
  await evaluate('selectSource("second.mp4")');
  assert.equal(state.video,"second.mp4");
  assert.equal(state.analysisRunId,"base2");
  assert.equal(element("#chain-inference").value,model.choiceId("second.mp4","base2"));
  await evaluate('chooseAnalysisRun("base2_pp_bb")');
  assert.equal(element("#chain-postprocess").value,"base2_pp_bb");
  await evaluate('selectSource("first.mp4")');
  assert.equal(state.analysisRunId,"base1_pp_aa");
  assert.equal(element("#chain-inference").value,model.choiceId("first.mp4","base1"));
  assert.equal(element("#chain-postprocess").value,"base1_pp_aa");
  assert.equal(await evaluate('loadAll("project2")'),true);
  assert.equal(state.video,"third.mp4");
  assert.equal(state.analysisRunId,"run_q");
  assert.equal(element("#chain-inference").options.length,1);
  assert.equal(element("#chain-postprocess").options.length,1);
  assert.equal(await evaluate('loadAll("project1")'),true);
  assert.equal(state.video,"first.mp4");
  assert.equal(state.analysisRunId,"base1_pp_aa");
  assert.equal(element("#chain-postprocess").value,"base1_pp_aa");
  assert.equal(stored.get("volley-header-run:project1:first.mp4"),"base1_pp_aa");
  assert.equal(stored.get("volley-header-run:project1:second.mp4"),"base2_pp_bb");
  assert(calls.some(([kind,name])=>kind==="video"&&name==="second.mp4"));
  assert(!fs.readFileSync(path.join(__dirname,"../scripts/volleyball_ui/static/index.html"),"utf8")
    .includes('id="chain-video"'));
  console.log("HEADER THREE-LEVEL VIDEO-IN-INFERENCE CASCADE: PASS");
})().catch(e=>{console.error(e);process.exitCode=1});
