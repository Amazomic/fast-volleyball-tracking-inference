"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs"),vm=require("node:vm"),path=require("node:path");
const source=fs.readFileSync(path.join(__dirname,"../scripts/volleyball_ui/static/app.js"),"utf8")
  .split('document.addEventListener("DOMContentLoaded",async()=>{')[0];
const nodes=new Map();
const get=id=>{
  if(!nodes.has(id))nodes.set(id,{disabled:false,dataset:{}});
  return nodes.get(id);
};
const document={querySelector:selector=>nodes.get(selector),querySelectorAll:()=>[],getElementById:get};
const context=vm.createContext({document,window:{addEventListener(){}},localStorage:{getItem:()=>null},console,setTimeout,clearTimeout,URL,URLSearchParams});
vm.runInContext(source,context);
function flags(values){context.values=values;return vm.runInContext('analysisFieldAvailability(values)',context);}
let f=flags({player_model:"RAVEL-VB-011-9f",player_reader:"opencv",player_identity_mode:"off",backend:"onnx-cpu",player_backend:"onnx-cpu"});
for(const key of ["player_head_threshold","player_action_threshold","player_decode_hwaccel","player_decode_threads","player_reid_threshold_percent","player_auto_assign","player_auto_assign_threshold_percent","player_session_gap_sec","gpu_device_id","gpu_memory_limit_mb"])
  assert.equal(f[key],false,key+" must be unavailable when it has no effect");
f=flags({player_model:"RAVEL-VB-013-9f",player_reader:"ffmpeg",player_identity_mode:"amateur",player_auto_assign:true,backend:"onnx",player_backend:"onnx-cuda"});
assert.ok(Object.values(f).every(Boolean));
assert.equal(flags({player_identity_mode:"amateur",player_auto_assign:false}).player_auto_assign_threshold_percent,false);
assert.equal(flags({player_model:"RAVEL-VB-012-9f"}).player_action_threshold,false);
assert.equal(flags({player_model:"RAVEL-VB-012-9f"}).player_head_threshold,true);
vm.runInContext('state.projects=[{id:"project",videos:["match.mp4"]}];state.project="project";state.preparationProject="project";state.benchmarkReady=true;state.latestTask={status:"running"};',context);
get("video-preparation").dataset.state="ready";
vm.runInContext('syncTaskStarts()',context);
assert.equal(get("analysis-start-submit").disabled,true);
assert.equal(get("benchmark-submit").disabled,true);
vm.runInContext('state.latestTask={status:"complete"};syncTaskStarts()',context);
assert.equal(get("analysis-start-submit").disabled,false);
get("video-preparation").dataset.state="working";
vm.runInContext('syncTaskStarts()',context);
assert.equal(get("analysis-start-submit").disabled,true,"analysis requires prepared inputs");
console.log("UI CONTROL DEPENDENCIES: model, decoder, ReID and GPU applicability PASS");
