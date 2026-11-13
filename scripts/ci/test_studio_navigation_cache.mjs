import {readFileSync} from "node:fs";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
import {runInNewContext} from "node:vm";
import assert from "node:assert/strict";

const root=join(fileURLToPath(new URL("../..",import.meta.url)));
const tabs=readFileSync(join(root,"rust/volley-app/static/studio_tabs.js"),"utf8");
const preview=readFileSync(join(root,"rust/volley-app/static/studio.js"),"utf8");
const section=(code,from,to)=> {
  const start=code.indexOf(from),end=code.indexOf(to,start);
  assert.ok(start>=0&&end>start,"Missing code section: "+from);
  return code.slice(start,end);
};

let videoLoads=0,playerLoads=0;
const state={tab:"preview",formProject:"project-1",playersLoaded:true};
const panel=()=>({classList:{toggle(){}}});
const ui={preview:panel(),montage:panel(),quick:panel(),
  analysis:panel(),players:panel(),report:panel()};
const runtime={navigate:tab=>tab,beginLoad(){}};
const context={state,ui,runtime,projectId:()=> "project-1",
  loadVideos:()=>{videoLoads++;return Promise.resolve();},
  loadPlayers:()=>{playerLoads++;return Promise.resolve();},
  document:{querySelectorAll:()=>[],addEventListener(){}},
  window:{dispatchEvent(){},addEventListener(){}},
  CustomEvent:class {constructor(name,config){this.type=name;this.detail=config.detail;}},
  failure:err=>{throw err;}
};
runInNewContext(section(tabs,"  const views={","  const projectId=()=>")+
  "\nthis.switchTab=switchTab;",context);
context.switchTab("analysis");
context.switchTab("preview");
context.switchTab("players");
context.switchTab("analysis");
assert.equal(videoLoads,0,"Returning to an initialized analysis tab must not reload data");
assert.equal(playerLoads,0,"Returning to an initialized players tab must not reload data");
state.formProject="other-project";
state.playersLoaded=false;
context.switchTab("analysis");
context.switchTab("players");
assert.equal(videoLoads,1,"Changing project must reload analysis data");
assert.equal(playerLoads,1,"Invalidating players cache must reload library");

let completionEvents=0;
const pipelineState={completedId:"",pipelineId:"",pipelineProject:""};
const pipelineContext={
  state:pipelineState,
  ui:{progress:{value:0}},
  projectId:()=> "project-1",
  window:{VolleyballTasks:{clear(){},set(){}},
    dispatchEvent:()=>completionEvents++},
  CustomEvent:class{constructor(name,config){this.type=name;this.detail=config.detail;}},
  saveMessage(){},stopPipelinePolling(){},announce(){},
  updateProfile(){},loadLog:()=>Promise.resolve(),failure:err=>{throw err;}
};
runInNewContext(section(tabs,"  const displayPipeline=(status,notifyCompletion=false)=>{",
  "  const pollPipeline=async()=>{")+"\nthis.displayPipeline=displayPipeline;",pipelineContext);
const status={schema:"volley-studio-pipeline/1",status:"complete",
  id:"old-result",project_id:"project-1",progress_percent:100};
pipelineContext.displayPipeline(status);
assert.equal(completionEvents,0,
  "Passive restoration of an old result must not refresh GPU/CPU snapshots");
pipelineContext.displayPipeline(status,true);
assert.equal(completionEvents,1,"A newly completed pipeline must refresh snapshots");
pipelineContext.displayPipeline(status,true);
assert.equal(completionEvents,1,"Completion must be processed only once");

assert.ok(preview.includes("if(previewCache.peek(path)===null)"),
  "Previously cached rally preview must not display an intermediate loading state");
assert.ok(!preview.includes('message("Загрузка кадровых границ и Touch-данных…")'),
  "Navigation must not display the obsolete Touch-data loading banner");
console.log("Studio navigation cache and pipeline restoration: OK");
