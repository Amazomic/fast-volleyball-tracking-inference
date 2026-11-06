"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const vm=require("node:vm");
const root=path.resolve(__dirname,"../scripts/volleyball_ui/static");
const source=fs.readFileSync(path.join(root,"app.js"),"utf8");
const from=source.indexOf("function preferredContextRun(");
const end=source.indexOf("function analysisCacheMap()",from);
assert(from>0&&end>from,"run chooser must exist");
const run={
  id:"run_inference",profile:"Любительский матч · ReID",
  videos:{"game.mp4":"aaa"}
};
const cpu={
  id:"run_inference_pp_recommended",parent_run_id:run.id,
  videos:{"game.mp4":"aaa.bbb"},
  postprocess_settings:{mode:"combined",player_threshold:.30,touch_threshold:.45}
};
const other={
  id:"run_inference_pp_custom",parent_run_id:run.id,
  videos:{"game.mp4":"aaa.ccc"},
  postprocess_settings:{mode:"baseline",player_threshold:.5,touch_threshold:.5}
};
const stored=new Map();
const state={
  project:"p",video:"game.mp4",analysisRuns:[run,other,cpu],
  analysisRunId:""
};
const context=vm.createContext({
  state,
  window:{VolleyballHeaderChain:require(path.join(root,"header-chain.js"))},
  localStorage:{getItem:k=>stored.get(k)||"",setItem:(k,v)=>stored.set(k,v)},
});
vm.runInContext([
  'let activeHeaderContext="";',
  'function contextIdentity(){return "project\0game.mp4"}',
  'function headerRunKey(){return "volley-selected"}',
  source.slice(from,end)
].join("\n"),context);
function select(newest=false){
  return vm.runInContext("preferredContextRun({newest:"+newest+"})",context);
}
assert.equal(select(),cpu.id,"default preview should choose completed recommended CPU");
state.analysisRunId=run.id;
assert.equal(select(true),cpu.id,"newly completed analysis selects recommended CPU");
stored.set("volley-selected",other.id);
state.analysisRunId="";
assert.equal(select(),other.id,"explicit user selection should be preserved");
stored.set("volley-selected",run.id);
assert.equal(select(),run.id,"explicit GPU selection should be preserved");
state.analysisRuns=[run,other];
stored.clear();
state.analysisRunId="";
assert.equal(select(),run.id,"GPU remains accessible before CPU replay exists");
console.log("PASS: recommended CPU is default; explicit selection and GPU fallback retained");
