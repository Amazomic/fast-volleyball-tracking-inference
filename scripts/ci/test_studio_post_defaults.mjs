import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";

const source=readFileSync(new URL("../../rust/volley-app/static/studio_tabs.js",import.meta.url),"utf8");
const extract=(begin,end)=>{
  const start=source.indexOf(begin),stop=source.indexOf(end,start);
  assert.ok(start>=0&&stop>start,`Cannot locate ${begin}`);
  return source.slice(start,stop);
};
const storage=new Map();
const localStorage={
  getItem:key=>storage.get(key)??null,
  setItem:(key,value)=>storage.set(key,value)
};
const project="f39dff74992f";
const key=`volley:studio-analysis-form:v1:${project}`;
const inputs=[
  {id:"studio-post-require-pose",value:"0",dataset:{default:"0"},checkValidity:()=>true},
  {id:"studio-post-ball-score",value:"0.35",dataset:{default:"0.35"},checkValidity:()=>true},
  {id:"studio-post-touch-algorithm",value:"legacy",dataset:{default:"legacy"},checkValidity:()=>true}
];
const ui={analysis:{querySelectorAll:()=>inputs},video:{value:"match.mp4"}};
const env={ui,localStorage,$:id=>inputs.find(x=>x.id===id),projectId:()=>project};
runInNewContext(extract("  const analysisFormCache=(()=>{","  const make=(tag,klass,text)=>")+ "\nthis.cache=analysisFormCache;",env);
runInNewContext(extract("  const allProfileFields=()=>","  const selectedGpuSource=()=>")+
  "\nthis.restore=restoreAnalysisSettings;this.save=saveAnalysisSettings;",env);

storage.set(key,JSON.stringify({version:1,video:"match.mp4",fields:{
  "studio-post-require-pose":["0","1"],
  "studio-post-ball-score":["0.35","0.44"]
}}));
assert.equal(env.restore(project),"match.mp4");
assert.equal(inputs[0].value,"0","Strict legacy override must not leak into the calibrated profile");
assert.equal(inputs[1].value,"0.44","Unrelated saved settings must survive migration");
assert.equal(JSON.parse(storage.get(key)).version,2);
assert.equal(JSON.parse(storage.get(key)).fields["studio-post-require-pose"],undefined);
inputs[0].value="1";
env.save();
assert.equal(JSON.parse(storage.get(key)).fields["studio-post-require-pose"][1],"1",
  "A deliberate strict-mode change in v2 must persist");
assert.equal(env.restore(project),"match.mp4");
assert.equal(inputs[0].value,"1","A user-selected v2 strict mode must not be erased");
console.log("PASS: migrated stale RTMPose gate and preserved user settings in v2.");
