import {readFileSync} from "node:fs";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";

const repo=join(fileURLToPath(new URL("../..",import.meta.url)));
const src=readFileSync(join(repo,"rust/volley-app/static/studio_legacy.js"),"utf8");
class Element{
  constructor(tag="div"){
    this.tagName=tag;this.children=[];this.listeners=new Map();
    this.value="";this.textContent="";this.disabled=false;this.className="";
    this.classList={states:new Set(),add:k=>this.classList.states.add(k),
      remove:k=>this.classList.states.delete(k),
      toggle:(k,v)=>v?this.classList.states.add(k):this.classList.states.delete(k),
      contains:k=>this.classList.states.has(k)};
  }
  addEventListener(k,fn){this.listeners.set(k,fn);}
  dispatch(k,data){return this.listeners.get(k)?.(data);}
  append(...children){this.children.push(...children);}
  replaceChildren(...children){this.children=[...children];}
  find(tag){return this.children.find(child=>child.tagName===tag);}
}
const id="391314fa0653",archiveId=id+"-"+"a".repeat(32);
const names=["projects","archives","refresh","archive-current","status"];
const nodes=new Map(names.map(n=>["studio-legacy-"+n,new Element()]));
nodes.set("studio-project",new Element("select"));
const $=id=>nodes.get("studio-legacy-"+id);
const handlers=new Map(),events=[];
const window={
  addEventListener:(name,fn)=>handlers.set(name,fn),
  dispatchEvent:event=>{
    events.push(event);
    return handlers.get(event.type)?.(event);
  }
};
const Event=class {constructor(type){this.type=type;}};
const CustomEvent=class {constructor(type,args){this.type=type;this.detail=args.detail;}};
const tick=async()=>{
  for(let i=0;i<8;i++)await new Promise(resolve=>setImmediate(resolve));
};
let imported=false,archived=false;
const calls=[];
const legacy=()=>({
  projects:[{
    id,name:"Old beach match",videos:["match.mp4"],
    historical_runs:["20261106_160721_394244_b9417c"],
    missing_videos:["match.mp4"],media_ready:false,imported:imported&&!archived
  }],
  original_results_immutable:true,metadata_only:true
});
const get=async(path,options={})=>{
  calls.push({path,options});
  if(path==="/api/project-legacy/native/catalog")return legacy();
  if(path==="/api/project-archive/native"&&options.method!=="POST")return {
    archives:archived?[{archive_id:archiveId,project_id:id,name:"Old beach match",
      legacy_imported:true,video_count:1}]:[],
    isolated_rust_only:true,recoverable:true
  };
  if(path==="/api/projects")return {projects:imported&&!archived?[{id,name:"Old beach match"}]:[]};
  if(path==="/api/project-legacy/native/import"){
    assert.deepEqual(JSON.parse(options.body),{project_id:id});
    assert.equal(options.method,"POST");
    assert.equal(archived,false);
    imported=true;
    return {imported:true,project_id:id,original_results_immutable:true,
      copied_media:false};
  }
  if(path==="/api/project-archive/native"){
    assert.equal(options.method,"POST");
    assert.deepEqual(JSON.parse(options.body),{project_id:id});
    archived=true;
    return {archived:true,deleted:false,project_id:id};
  }
  if(path==="/api/project-archive/native/restore"){
    assert.equal(options.method,"POST");
    assert.deepEqual(JSON.parse(options.body),{archive_id:archiveId});
    archived=false;
    return {restored:true,project_id:id,legacy_results_unchanged:true};
  }
  throw Error("Unexpected URL "+path);
};
const fetch=async(path,options={})=>({ok:true,json:async()=>get(path,options)});
window.VolleyballStudioActivity={fetch:(...args)=>fetch(...args)};
const context={document:{getElementById:id=>nodes.get(id),
    createElement:tag=>new Element(tag)},window,Event,CustomEvent,fetch,console};
vm.runInNewContext(src,context,{filename:"studio_legacy.js"});
assert.equal($("archive-current").disabled,true);
await window.dispatchEvent(new CustomEvent("volley:tab-change",{detail:{tab:"legacy"}}));
await tick();
assert.equal($("projects").children.length,1);
const legacyCard=$("projects").children[0];
const warnings=legacyCard.children.filter(c=>c.className.includes("quick-error"));
assert.equal(warnings.length,1);
assert.match(warnings[0].textContent,/Недоступны исходные видео/);
assert.match(legacyCard.children[1].textContent,/исторических записей/);
assert.equal(legacyCard.find("button").textContent,"Импортировать метаданные в Rust");
legacyCard.find("button").dispatch("click");
await tick();
assert.equal(imported,true);
assert.equal($("projects").children[0].find("button").textContent,"Открыть в Rust");
nodes.get("studio-project").value=id;
nodes.get("studio-project").dispatch("change");
assert.equal($("archive-current").disabled,false);
$("projects").children[0].find("button").dispatch("click");
assert.ok(events.some(e=>e.type==="volley:open-legacy-project"&&e.detail.project_id===id));
$("archive-current").dispatch("click");
await tick();
assert.equal(archived,true);
assert.equal($("archive-current").disabled,true);
assert.equal($("archives").children.length,1);
assert.equal($("projects").children[0].find("button").disabled,true);
assert.equal($("archives").children[0].find("button").textContent,"Восстановить Rust-проект");
$("archives").children[0].find("button").dispatch("click");
await tick();
assert.equal(archived,false);
assert.equal($("archives").children[0].textContent,"Восстанавливаемых архивов Rust пока нет.");
assert.ok(events.filter(e=>e.type==="volley:refresh-projects").length>=3);
assert.ok(calls.every(c=>c.options.method!=="DELETE"));
assert.ok(calls.some(c=>c.path==="/api/project-legacy/native/catalog"));
console.log("Legacy Rust Studio UI: old missing media, import/open, recoverable archive, restore, no source mutation passed");
