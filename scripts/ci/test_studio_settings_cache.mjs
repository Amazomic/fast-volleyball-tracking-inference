// Regression: refreshing the Studio must preserve source choice and per-project
// analysis form values without re-running GPU/CPU or reloading heavy metadata.
import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
import {runInNewContext} from "node:vm";

const root=join(fileURLToPath(new URL("../..",import.meta.url)));
const studio=readFileSync(join(root,"rust/volley-app/static/studio.js"),"utf8");
const tabs=readFileSync(join(root,"rust/volley-app/static/studio_tabs.js"),"utf8");
const cut=(source,start,end)=>{
  const a=source.indexOf(start),b=source.indexOf(end,a+start.length);
  assert.ok(a>=0&&b>a,"Expected Studio cache adapter missing: "+start);
  return source.slice(a,b);
};
const selectionSource=cut(studio,"  const sourceSelection=(()=>{","  const url=(path,params)=>")+
  "\nthis.sourceSelection=sourceSelection;";
const formSource=cut(tabs,"  const analysisFormCache=(()=>{","  const make=(tag,klass,text)=>")+
  "\nthis.analysisFormCache=analysisFormCache;";
const functionsSource=cut(tabs,"  const allProfileFields=()=>","  const updateProfile=()=>")+
  "\nthis.saveAnalysisSettings=saveAnalysisSettings;this.restoreAnalysisSettings=restoreAnalysisSettings;";
const store=new Map();
const localStorage={
  getItem:key=>store.get(key)??null,
  setItem:(key,value)=>store.set(key,String(value)),
  removeItem:key=>store.delete(key)
};
const scope=()=>{
  const context={localStorage,JSON};
  runInNewContext(selectionSource+"\n"+formSource,context);
  return context;
};
const first=scope();
const project="a".repeat(12),analysis="video::fusion-1",run="f".repeat(32);
first.sourceSelection.save(project,analysis,run);
const refreshed=scope();
assert.deepEqual(JSON.parse(JSON.stringify(refreshed.sourceSelection.read())),
  {project,analysis,run},"Refresh should retain selected project/GPU/CPU IDs");
const field=(id,baseline,value=baseline)=>({
  id,value,dataset:{default:baseline},checkValidity(){return Number(this.value)>=0;}
});
const stride=field("studio-gpu-stride","9","7");
const touch=field("studio-post-touch-threshold","0.5","0.75");
const ui={analysis:{querySelectorAll:()=>[stride,touch]},video:{value:"match.mp4"}};
const context={localStorage,ui,projectId:()=>project,JSON};
runInNewContext(formSource+"\n"+functionsSource,context);
context.saveAnalysisSettings();
let saved=refreshed.analysisFormCache.read(project);
assert.equal(saved.video,"match.mp4");
assert.deepEqual(JSON.parse(JSON.stringify(saved.fields["studio-gpu-stride"])),["9","7"]);
stride.value="9";touch.value="0.5";ui.video.value="";
assert.equal(context.restoreAnalysisSettings(project),"match.mp4");
assert.equal(stride.value,"7");
assert.equal(touch.value,"0.75");
stride.dataset.default="10";stride.value="0";
context.restoreAnalysisSettings(project);
assert.equal(stride.value,"10","A new gold default must override an old snapshot");
assert.equal(touch.value,"0.75","Unchanged defaults must restore user overrides");
assert.equal(refreshed.analysisFormCache.read("b".repeat(12)),null,
  "Settings are isolated by project");
assert.match(studio,/VolleyballStudioProjectCatalog=\(\)=>previewCache\.load\("\/api\/projects",300000,true\)/);
assert.match(studio,/rallies\/runs",\{project_id:project\}\),300000,true\)/);
assert.match(tabs,/await window\.VolleyballStudioProjectCatalog\(\)/);
assert.match(tabs,/window\.VolleyballEditor\?\.load\(project,ui\.video\.value,true\)/);
console.log("PASS: Studio refresh restores source chain and per-project settings using bounded metadata caches");
