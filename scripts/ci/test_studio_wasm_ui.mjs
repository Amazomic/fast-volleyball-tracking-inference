import {readFileSync} from "node:fs";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
import {runInNewContext} from "node:vm";
import assert from "node:assert/strict";

const root=join(fileURLToPath(new URL("../..",import.meta.url)));
const wasm=readFileSync(join(root,"rust/volley-app/static/volley_web.wasm"));
const boot=readFileSync(join(root,"rust/volley-app/static/studio_ui.js"),"utf8");
const tabsCode=readFileSync(join(root,"rust/volley-app/static/studio_tabs.js"),"utf8");
const tick=async()=>{for(let i=0;i<8;i++)await new Promise(r=>setImmediate(r));};
const names=["preview","analysis","players","montage","quick","legacy"];
class Node {
  constructor(tag="div"){
    this.nodeType=1;this.tagName=tag.toUpperCase();this.children=[];
    this.dataset={};this.listeners=new Map();this.attrs=new Map();
    this.textContent="";this.value="";this.disabled=false;this.className="";
    this.style={};this.options=[];this.files=[];this.scrollCount=0;
    this.classList={values:new Set(),toggle:(k,on)=>on?
      this.classList.values.add(k):this.classList.values.delete(k),
      add:k=>this.classList.values.add(k),
      remove:k=>this.classList.values.delete(k),
      contains:k=>this.classList.values.has(k)};
  }
  addEventListener(name,fn){this.listeners.set(name,fn);}
  dispatch(name,event={}){
    return this.listeners.get(name)?.({preventDefault(){},target:this,...event});
  }
  append(...nodes){for(const node of nodes){node.parentElement=this;this.children.push(node);}}
  replaceChildren(...nodes){this.children=[];this.append(...nodes);}
  get firstChild(){return this.children[0]||null;}
  remove(){if(this.parentElement){const siblings=this.parentElement.children;
    const i=siblings.indexOf(this);if(i>=0)siblings.splice(i,1);}this.removed=true;}
  setAttribute(k,v){this.attrs.set(k,v);}
  getAttribute(k){return this.attrs.get(k)??null;}
  hasAttribute(k){return this.attrs.has(k);}
  removeAttribute(k){this.attrs.delete(k);}
  querySelectorAll(){return [];}
  scrollIntoView(){this.scrollCount++;}
  focus(){}
}
const ids=[
  "studio-banner",
  "studio-preview-view","studio-analysis-view","studio-players-view",
  "studio-quick-view","studio-legacy-view",
  "studio-project","studio-analysis-video","studio-analysis-refresh",
  "studio-stage-list","studio-analysis-note","studio-gpu-ball",
  "studio-gpu-players","studio-gpu-cancel","studio-player-create",
  "studio-player-name","studio-player-list","studio-player-count",
  "studio-player-refresh","studio-montage",
  "studio-gpu-stride","studio-gpu-memory","studio-gpu-threads",
  "studio-gpu-settings-delta"
];
for(const kind of ["preview","analysis","players","montage","quick","legacy"])
  ids.push("studio-nav-"+kind);
const stored=new Map(),calls=[];
function createTestEnvironment(initialHash="#preview",flags={}){
  const nodes=new Map(ids.map(id=>[id,new Node()]));
  const byId=id=>nodes.get(id);
  byId("studio-gpu-stride").value="9";
  byId("studio-gpu-memory").value="0";
  byId("studio-gpu-threads").value="8";
  byId("studio-project").value="1234abcdef01";
  byId("studio-analysis-video").value="";
  for(const name of names)byId("studio-nav-"+name).dataset.studioTab=name;
  const listener=new Map();
  const brandBall=new Node("svg");
  const win={
    addEventListener:(name,handler)=>listener.set(name,handler),
    dispatchEvent:event=>{listener.get(event.type)?.(event);return true;},
    matchMedia:()=>({matches:false})
  };
  const document={
    body:new Node("body"),documentElement:{lang:"ru"},
    getElementById:byId,createElement:tag=>new Node(tag),
    querySelector:selector=>selector===".brand .volleyball-ball"?brandBall:null,
    createTextNode:text=>({nodeType:3,nodeValue:text}),
    querySelectorAll:selector=>selector==="[data-studio-tab]"?
      names.map(name=>byId("studio-nav-"+name)):[],
    createTreeWalker:()=>({currentNode:null,nextNode:()=>false})
  };
  const requests=[];
  const fetch=async(path,options={})=>{
    requests.push({path,options});calls.push({path,options});
    if(path==="/studio/rust/selection.wasm"){
      return {ok:true,arrayBuffer:async()=>wasm.buffer.slice(wasm.byteOffset,wasm.byteOffset+wasm.byteLength)};
    }
    if(path==="/api/projects"){
      return {ok:true,json:async()=>({projects:[{id:"1234abcdef01",name:"Match",videos:["rally.mp4"]}]})};
    }
    if(path.startsWith("/api/studio/rust/analysis-overview?")){
      return {ok:true,json:async()=>({schema:"volley-native-analysis-overview/1",
        metadata_only:true,authoritative_game_events:false,stages:[]})};
    }
    if(path.startsWith("/api/inference/native/full/active?")){
      return {ok:true,json:async()=>({schema:"volley-native-active-gpu/1",
        project_id:"1234abcdef01",active:false})};
    }
    if(path==="/api/inference/native/full/start"){
      assert.equal(options.method,"POST");
      return {ok:true,json:async()=>({job_id:"a".repeat(32),state:"queued"})};
    }
    if(path.startsWith("/api/inference/native/full/status?")){
      return {ok:true,json:async()=>({phase:"complete",progress_percent:100})};
    }
    if(path==="/api/players/library/native"){
      assert.equal(options.method,"POST");
      assert.equal(JSON.parse(options.body).action,"read");
      if(flags.delayPlayers){
        return {ok:true,json:async()=>new Promise(resolve=>{state.releasePlayers=resolve;})};
      }
      return {ok:true,json:async()=>({players:[],revision:"absent"})};
    }
    throw Error("Unexpected external request: "+path);
  };
  win.fetch=(...args)=>fetch(...args);
  const history={replaceState:(_a,_b,hash)=>{state.location.hash=hash;}};
  const location={hash:initialHash};
  const state={location,nodes,requests,window:win,document,history};
  const context={window:win,document,fetch,history,location,WebAssembly,
    TextDecoder,TextEncoder,URLSearchParams,console,queueMicrotask,
    localStorage:{
      getItem:key=>stored.get(key)??null,
      setItem:(key,value)=>stored.set(key,value),
      removeItem:key=>stored.delete(key)
    },
    MutationObserver:class{observe(){} disconnect(){}},
    NodeFilter:{SHOW_TEXT:4,FILTER_ACCEPT:1,FILTER_REJECT:2},
    Event:class{constructor(type){this.type=type;}},
    CustomEvent:class{constructor(type,args){this.type=type;this.detail=args?.detail;}},
    setInterval:()=>1,clearInterval(){}, setTimeout,clearTimeout,
    requestAnimationFrame:()=>1
  };
  runInNewContext(boot,context,{filename:"studio_ui.js"});
  runInNewContext(tabsCode,context,{filename:"studio_tabs.js"});
  state.runtime=win.VolleyballUi;
  state.brandBall=brandBall;
  assert.equal(typeof win.VolleyballStudioActivity?.begin,"function");
  return state;
}
const a=createTestEnvironment("#preview");
await a.runtime.ready;await tick();
assert.equal(a.runtime.isReady(),true);
assert.equal(a.runtime.get("locale"),0);
assert.equal(a.runtime.get("tab"),0);
assert.equal(a.nodes.get("studio-preview-view").classList.contains("hidden"),false);
assert.equal(a.nodes.get("studio-analysis-view").classList.contains("hidden"),true);
assert.equal(a.runtime.translate("Монтаж"),"Монтаж");
assert.equal(a.runtime.set("locale",1),true);
assert.equal(a.runtime.translate("Монтаж"),"Montage");
assert.equal(a.document.documentElement.lang,"en");
assert.equal(a.nodes.has("studio-ui-settings-label"),false,
  "Removed settings must not be present in the browser UI");
assert.equal(a.runtime.set("theme",1),true);
assert.equal(a.document.body.dataset.uiTheme,"light");
assert.equal(a.runtime.set("density",1),true);
assert.equal(a.document.body.dataset.uiDensity,"compact");
assert.equal(a.runtime.set("reduced_motion",1),true);
assert.equal(a.document.body.dataset.uiMotion,"reduce");
assert.equal(a.runtime.set("density",2),false);
assert.equal(a.runtime.get("density"),1);
assert.ok(stored.has("volley.rust.ui.preferences.v1"));
for(let i=0;i<names.length;i++){
  const name=names[i];
  a.nodes.get("studio-nav-"+name).dispatch("click");
  await tick();
  assert.equal(a.runtime.get("tab"),i,"WASM tab authority "+name);
  const correct=name==="montage"?"studio-preview-view":"studio-"+name+"-view";
  assert.equal(a.nodes.get(correct).classList.contains("hidden"),false,
    "Tab DOM visibility "+name);
  assert.equal(a.nodes.get("studio-nav-"+name).classList.contains("active"),true);
  assert.equal(a.location.hash,"#"+name);
}
assert.ok(a.requests.some(x=>x.path==="/api/projects"),"Analysis fetched project list");
assert.ok(a.requests.some(x=>x.path==="/api/players/library/native"),"Players fetched library");
a.nodes.get("studio-nav-analysis").dispatch("click");
await tick();
assert.equal(a.nodes.get("studio-analysis-video").value,"rally.mp4");
a.nodes.get("studio-gpu-ball").dispatch("click");
await tick();
const nativeGpuRequests=()=>a.requests.filter(x=>x.path==="/api/inference/native/full/start");
assert.equal(nativeGpuRequests().length,1,"One explicit GPU launch");
const golden=JSON.parse(nativeGpuRequests()[0].options.body);
assert.equal(golden.stride,9);
assert.equal(golden.gpu_memory_limit_mb,0);
assert.equal(golden.cpu_threads,8);
assert.equal(golden.device,"cuda");
assert.equal(golden.model,"ball");
a.nodes.get("studio-gpu-stride").value="3";
a.nodes.get("studio-gpu-memory").value="8192";
a.nodes.get("studio-gpu-threads").value="12";
a.nodes.get("studio-gpu-players").dispatch("click");
await tick();
assert.equal(nativeGpuRequests().length,2,"Second explicit GPU launch only");
const override=JSON.parse(nativeGpuRequests()[1].options.body);
assert.equal(override.stride,3);
assert.equal(override.gpu_memory_limit_mb,8192);
assert.equal(override.cpu_threads,12);
assert.equal(override.gpu_device_id,0);
assert.equal(override.device,"cuda");
assert.equal(override.model,"players");
a.nodes.get("studio-gpu-memory").value="-1";
a.nodes.get("studio-gpu-ball").dispatch("click");
await tick();
assert.equal(nativeGpuRequests().length,2,"Invalid GPU settings cannot launch job");
a.nodes.get("studio-gpu-memory").value="0";
const old=a.runtime.beginLoad("analysis");
const newer=a.runtime.beginLoad("analysis");
assert.equal(a.runtime.currentLoad("analysis",old),false);
assert.equal(a.runtime.currentLoad("analysis",newer),true);
assert.equal(a.runtime.navigate("invalid"),"preview");
assert.equal(a.runtime.get("tab"),0);
assert.equal(a.nodes.get("studio-gpu-settings-delta").textContent.includes("Эталонные"),true);
a.nodes.get("studio-gpu-stride").value="3";
a.nodes.get("studio-gpu-stride").dispatch("input");
assert.match(a.nodes.get("studio-gpu-settings-delta").textContent,/шаг 3 вместо 9/);
a.nodes.get("studio-gpu-stride").value="10";
a.nodes.get("studio-gpu-stride").dispatch("input");
assert.match(a.nodes.get("studio-gpu-settings-delta").textContent,/допустимого диапазона/);
const b=createTestEnvironment("#players");
await b.runtime.ready;await tick();
assert.equal(b.runtime.get("locale"),1,"Language persisted across reload");
assert.equal(b.runtime.get("theme"),1,"Theme persisted across reload");
assert.equal(b.runtime.get("density"),1,"Density persisted across reload");
assert.equal(b.runtime.get("tab"),2,"Deep-link route set by WASM");
assert.equal(b.nodes.has("studio-ui-reset"),false,
  "Removed header must not expose the old preferences reset control");
stored.delete("volley.rust.ui.preferences.v1");
const delayed=createTestEnvironment("#players",{delayPlayers:true});
await delayed.runtime.ready;await tick();
assert.equal(typeof delayed.releasePlayers,"function","A real delayed players request is pending");
delayed.nodes.get("studio-nav-quick").dispatch("click");
await tick();
assert.equal(delayed.runtime.get("tab"),4);
delayed.releasePlayers({players:[{id:"f".repeat(12),name:"Stale user"}],revision:"new"});
await tick();
assert.equal(delayed.nodes.get("studio-player-list").children.length,1);
assert.equal(delayed.nodes.get("studio-player-list").children[0].textContent,
  "Загрузка библиотеки игроков…","Old player response must not replace the new tab state");
console.log("Real Rust/WASM browser bridge: six routes/deep links, safe loading, bilingual labels, persisted visual settings and golden GPU defaults passed");
