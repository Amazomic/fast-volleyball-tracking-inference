import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
import {runInNewContext} from "node:vm";

const root=fileURLToPath(new URL("../..",import.meta.url));
const tabs=readFileSync(join(root,"rust/volley-app/static/studio_tabs.js"),"utf8");
const preview=readFileSync(join(root,"rust/volley-app/static/studio.js"),"utf8");
const start=tabs.indexOf("  const views={");
const end=tabs.indexOf("  const projectId=()=>",start);
assert.ok(start>=0&&end>start,"Central media lifecycle must be present");

const handlers={document:new Map(),window:new Map()};
const makeView=()=>{
  const hidden={value:false};
  const view={
    hidden,
    contains:media=>media.parent===view,
    classList:{toggle:(_name,value)=>{hidden.value=value;}}
  };
  return view;
};
const ui=Object.fromEntries(
  ["preview","montage","quick","analysis","players","report"].map(tab=>[tab,makeView()])
);
const makeMedia=(parent,tagName="VIDEO")=>({
  parent,tagName,paused:true,currentTime:17.25,pauseCount:0,
  pause(){this.paused=true;this.pauseCount++;}
});
const previewVideo=makeMedia(ui.preview);
const editorVideo=makeMedia(ui.analysis);
const reportAudio=makeMedia(ui.report,"AUDIO");
const media=[previewVideo,editorVideo,reportAudio];
const document={
  hidden:false,
  querySelectorAll:selector=>selector==="video, audio"?media:[],
  addEventListener:(name,handler)=>handlers.document.set(name,handler)
};
const window={
  addEventListener:(name,handler)=>handlers.window.set(name,handler),
  dispatchEvent(){}
};
const state={tab:"preview",formProject:"project-1",playersLoaded:true};
const runtime={navigate:tab=>tab,beginLoad(){}};
const context={
  document,window,ui,state,runtime,projectId:()=>"project-1",
  loadVideos:()=>Promise.resolve(),loadPlayers:()=>Promise.resolve(),
  CustomEvent:class{constructor(name,config){this.type=name;this.detail=config.detail;}},
  failure:error=>{throw error;}
};
runInNewContext(tabs.slice(start,end)+"\nthis.switchTab=switchTab;",context);
const fire=(name,target)=>handlers.document.get(name)({target});

previewVideo.paused=false;
editorVideo.paused=false;
context.switchTab("analysis");
assert.equal(previewVideo.paused,true,"Leaving preview must pause its player");
assert.equal(editorVideo.paused,false,"Active player must not be interrupted");
assert.equal(previewVideo.currentTime,17.25,"Switching tabs must preserve position");

context.switchTab("players");
assert.equal(editorVideo.paused,true,"Leaving analysis must pause its editor player");
context.switchTab("preview");
assert.equal(previewVideo.paused,true,"Reopening preview must not auto-resume");

editorVideo.paused=false;
fire("play",editorVideo);
assert.equal(editorVideo.paused,true,"Delayed play in hidden analysis must be rejected");
previewVideo.paused=false;
fire("play",previewVideo);
assert.equal(previewVideo.paused,false,"Playing in active preview is allowed");
document.hidden=true;
handlers.document.get("visibilitychange")();
assert.equal(previewVideo.paused,true,"Background browser tab must pause video");
document.hidden=false;
assert.equal(previewVideo.paused,true,"Returning to browser tab must not auto-resume");

context.switchTab("report");
reportAudio.paused=false;
handlers.window.get("pagehide")();
assert.equal(reportAudio.paused,true,"Leaving the page must pause active audio/video");
assert.match(preview,/const previewIsVisible=\(\)=>!document\.hidden&&/);
assert.match(preview,/if\(!s\.videoKey\|\|!previewIsVisible\(\)\)return;/);
assert.match(preview,/if\(previewIsVisible\(\)\)ui\.video\.play\(\)/);
assert.match(preview,/event\.target\?\.isContentEditable\|\|!s\.videoKey\|\|!previewIsVisible\(\)\)return;/);

console.log("Studio media lifecycle: hidden tabs, browser background and page exit: OK");
