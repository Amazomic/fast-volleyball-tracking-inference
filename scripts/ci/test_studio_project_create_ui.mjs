// Dependency-free regression for creation modal: DnD upload + global players + linked project.
import {readFileSync} from "node:fs";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";

const repo=join(fileURLToPath(new URL("../..",import.meta.url)));
const html=readFileSync(join(repo,"rust/volley-app/static/studio.html"),"utf8");
const source=readFileSync(join(repo,"rust/volley-app/static/studio.js"),"utf8");
const css=readFileSync(join(repo,"rust/volley-app/static/studio.css"),"utf8");
assert.ok(html.includes('class="studio-project-create-grid"'));
assert.match(css,/\.studio-project-create-grid\{display:grid;grid-template-columns:minmax\(0,1\.12fr\) minmax\(0,1fr\)/);
assert.ok(!css.includes("max-height:min(80vh,680px)"));
assert.ok(html.includes('id="studio-project-drop"'));
assert.ok(html.includes('id="studio-project-file-input"'));
assert.ok(html.includes('id="studio-project-players"'));
assert.ok(html.includes('id="studio-project-add-player"'));
assert.ok(html.indexOf('id="studio-project-new"')<html.indexOf('id="studio-project"'));

class Elem {
  constructor(tag="div",text="") {
    this.tagName=tag;this.textContent=text;this.children=[];
    this.value="";this.files=[];this.checked=false;this.disabled=false;
    this.listeners=new Map();this.open=false;this.attributes=new Map();
    this.classList={active:new Set(),add:k=>this.classList.active.add(k),
      remove:k=>this.classList.active.delete(k),
      toggle:(k,enabled)=>enabled?this.classList.active.add(k):this.classList.active.delete(k)};
  }
  append(...nodes){this.children.push(...nodes);}
  replaceChildren(...nodes){this.children=[...nodes];}
  addEventListener(k,fn){this.listeners.set(k,fn);}
  setAttribute(k,v){this.attributes.set(k,v);}
  querySelectorAll(q){
    assert.equal(q,"input[type=checkbox]:checked");
    const all=[];
    const walk=n=>{if(n.tagName==="input"&&n.type==="checkbox"&&n.checked)all.push(n);
      for(const child of n.children)walk(child);};
    for(const c of this.children)walk(c);
    return all;
  }
  click(){return this.listeners.get("click")?.({preventDefault(){}});}
  dispatch(k,p={}){return this.listeners.get(k)?.({preventDefault(){},...p});}
  showModal(){this.open=true;}
  close(){this.open=false;}
  focus(){}
  reset(){}
}
const make=(tag,klass,text)=>{const el=new Elem(tag,text);el.className=klass;return el;};
const keys=["projectNew","projectDialog","createForm","projectName","projectVideos",
  "projectSubmit","projectCreateError","projectCancel","projectClose","projectDrop",
  "projectFiles","projectUploadStatus","projectPlayers","projectPlayerName","projectAddPlayer"];
const ui=Object.fromEntries(keys.map(k=>[k,new Elem()]));
ui.createForm.reset=()=>{ui.projectName.value="";ui.projectPlayerName.value="";};
const tick=async()=>{for(let i=0;i<12;i++)await new Promise(resolve=>setImmediate(resolve));};
const videoName="match--"+("a".repeat(32))+".mp4";
const athleteId="b".repeat(12),projectId="c".repeat(12);
let uploads=0,createdPayload=null,selectedProject=null,playerList=[];
const request=async path=>{
  if(path.startsWith("/api/project-source/lookup?"))return {duplicate:false};
  assert.equal(path,"/api/videos");
  return {videos:uploads?[{name:videoName,display_name:"match.mp4",origin:"uploaded",size_gb:0.2}]:[]};
};
const post=async(path,payload)=>{
  if(path==="/api/players/library/native"){
    if(payload.action==="read")return {players:playerList,revision:playerList.length?"x".repeat(64):"absent"};
    assert.equal(payload.action,"create");
    assert.equal(payload.name,"Ирек");
    assert.equal(payload.expected_revision,"absent");
    playerList=[{id:athleteId,name:"Ирек"}];
    return {players:playerList,player_id:athleteId,revision:"x".repeat(64)};
  }
  assert.equal(path,"/api/project-create");
  createdPayload=payload;
  return {project:{id:projectId}};
};
class FormData {
  append(k,file){assert.equal(k,"file");this.file=file;}
}
class XMLHttpRequest {
  constructor(){this.events=new Map();this.upload={addEventListener:(k,fn)=>this.onProgress=fn};this.status=200;}
  addEventListener(k,fn){this.events.set(k,fn);}
  open(method,path){assert.equal(method,"POST");assert.equal(path,"/api/project-source/upload");}
  setRequestHeader(){}
  send(form){
    assert.equal(form.file.name,"match.mp4");
    uploads++;
    this.onProgress?.({lengthComputable:true,total:100,loaded:100});
    this.responseText=JSON.stringify({name:videoName,display_name:"match.mp4"});
    this.events.get("load")();
  }
  abort(){this.events.get("abort")?.();}
}
const wEvents=[];
const window={dispatchEvent:ev=>wEvents.push(ev.type),
  VolleyballSha256:{digest:async()=>("a".repeat(64))}};
vm.runInNewContext(readFileSync(join(repo,"rust/volley-app/static/studio_upload.js"),"utf8"),{window,FormData,XMLHttpRequest});
const Event=class {constructor(type){this.type=type;}};
const projects=async id=>{selectedProject=id;};
const message=()=>{},failed=e=>{throw e;};
const start=source.indexOf("  // The project dialog accepts browser-owned");
const stop=source.indexOf('  ui.project.addEventListener("change"',start);
assert.ok(start>0&&stop>start,"project creation controller not found");
vm.runInNewContext(source.slice(start,stop),{
  ui,make,request,post,window,Event,projects,message,failed,
  invalidateCatalog:()=>{},FormData,XMLHttpRequest,TextEncoder,Set,URLSearchParams,
  document:{createElement:tag=>new Elem(tag)},
});
ui.projectNew.click();await tick();
assert.equal(ui.projectDialog.open,true);
assert.equal(ui.projectVideos.querySelectorAll("input[type=checkbox]:checked").length,0);
ui.projectPlayerName.value="Ирек";
ui.projectAddPlayer.click();await tick();
assert.deepEqual(ui.projectPlayers.querySelectorAll("input[type=checkbox]:checked").map(x=>x.value),[athleteId]);
ui.projectDrop.dispatch("drop",{dataTransfer:{files:[{name:"match.mp4",size:2048}]}});
await tick();
assert.deepEqual(ui.projectVideos.querySelectorAll("input[type=checkbox]:checked").map(x=>x.value),[videoName]);
ui.projectName.value="Тестовый матч";
ui.projectName.dispatch("input");
assert.equal(ui.projectSubmit.disabled,false);
await ui.createForm.dispatch("submit");
assert.deepEqual(JSON.parse(JSON.stringify(createdPayload)),{
  name:"Тестовый матч",videos:[videoName],player_ids:[athleteId]
});
assert.equal(selectedProject,projectId);
assert.equal(ui.projectDialog.open,false);
assert.ok(wEvents.includes("volley:player-library-updated"));
console.log("Rust Studio new project: drop/chooser upload, athlete create/select, project linkage passed");
