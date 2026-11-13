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
const markup=readFileSync(join(root,"rust/volley-app/static/studio.html"),"utf8");
const navMarkup=markup.match(/<nav class="menu">([\s\S]*?)<\/nav>/)?.[1]||"";
const navOrder=[...navMarkup.matchAll(/id="studio-nav-([^"]+)"/g)].map(row=>row[1]);
assert.deepEqual(navOrder,["players","analysis","preview","report","montage"],
  "Sidebar must follow the requested five-step navigation order");
const reportStart=markup.indexOf('id="studio-report-view"');
const statsStart=markup.indexOf('id="studio-native-stats-panel"');
assert.ok(reportStart>0&&statsStart>reportStart,
  "Existing player statistics must be accessible inside the dedicated Report view");

// The unified full-pipeline form is parsed from the real markup so the test
// exercises the same defaults, bounds and steps the browser receives.
const profileInputs=[...markup.matchAll(/<input[^>]*id="(studio-(?:gpu|post)[^"]*)"[^>]*>/g)]
  .map(row=>{
    const attrs={};
    for(const attr of row[0].matchAll(/([\w-]+)="([^"]*)"/g))attrs[attr[1]]=attr[2];
    return attrs;
  });
assert.equal(profileInputs.length,27,"GPU + CPU post-processing profile fields");
const profileSelects=[...markup.matchAll(/<select[^>]*id="(studio-(?:gpu|post)[^"]*)"[^>]*>/g)]
  .map(row=>Object.fromEntries([...row[0].matchAll(/([\w-]+)="([^"]*)"/g)]
    .map(attr=>[attr[1],attr[2]])));
assert.deepEqual(profileSelects.map(attrs=>attrs.id).sort(),
  ["studio-gpu-pose-enabled","studio-post-pose-classify","studio-post-require-pose","studio-post-touch-algorithm"]);

const names=["preview","analysis","players","montage","quick","report"];
class Node {
  constructor(tag="div"){
    this.nodeType=1;this.tagName=tag.toUpperCase();this.children=[];
    this.dataset={};this.listeners=new Map();this.attrs=new Map();
    this.textContent="";this.value="";this.disabled=false;this.className="";
    this.style={};this.options=[];this.files=[];this.scrollCount=0;
    this.scrollTop=0;this.clientHeight=0;this.scrollHeight=0;
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
  querySelectorAll(selector){
    if(selector==="input[data-default],select[data-default]")
      return this.children.filter(child=>child.dataset.default!==undefined);
    return [];
  }
  checkValidity(){
    const raw=String(this.value).trim();
    if(!raw)return false;
    const value=Number(raw);
    if(!Number.isFinite(value))return false;
    if(this.min!==undefined&&value<Number(this.min))return false;
    if(this.max!==undefined&&value>Number(this.max))return false;
    if(this.step==="1"&&!Number.isSafeInteger(value))return false;
    return true;
  }
  scrollIntoView(){this.scrollCount++;}
  focus(){}
}
const ids=[
  "studio-banner",
  "studio-preview-view","studio-montage-view","studio-analysis-view","studio-players-view","studio-report-view",
  "studio-quick-view",
  "studio-project","studio-analysis-video","studio-analysis-note",
  "studio-analysis-start","studio-analysis-gpu-only","studio-analysis-post-only",
  "studio-inference","studio-analysis-reset","studio-analysis-progress",
  "studio-analysis-profile-delta","studio-analysis-log-panel",
  "studio-analysis-log-output","studio-analysis-log-current","studio-analysis-log-count",
  "studio-history-list","studio-history-refresh","studio-history-more","studio-history-clear",
  "studio-player-create","studio-player-name","studio-player-list","studio-player-count",
  "studio-player-refresh"
];
for(const kind of ["preview","analysis","players","report","montage"])
  ids.push("studio-nav-"+kind);
ids.push("studio-quick-open");
const stored=new Map(),calls=[];
function createTestEnvironment(initialHash="#preview",flags={}){
  const nodes=new Map(ids.map(id=>[id,new Node()]));
  const byId=id=>nodes.get(id);
  for(const attrs of profileInputs){
    const input=new Node("input");
    input.id=attrs.id;input.type=attrs.type;
    input.min=attrs.min;input.max=attrs.max;input.step=attrs.step;
    input.value=attrs.value;input.dataset.default=attrs["data-default"];
    nodes.set(attrs.id,input);
    byId("studio-analysis-view").append(input);
  }
  for(const attrs of profileSelects){
    const select=new Node("select");
    select.id=attrs.id;select.value=attrs["data-default"];select.dataset.default=attrs["data-default"];
    nodes.set(attrs.id,select);
    byId("studio-analysis-view").append(select);
  }
  byId("studio-project").value="1234abcdef01";
  byId("studio-analysis-video").value="";
  for(const name of names.filter(name=>name!=="quick"))
    byId("studio-nav-"+name).dataset.studioTab=name;
  byId("studio-quick-open").dataset.studioTab="quick";
  const listener=new Map();
  const brandBall=new Node("svg");
  let uuidCounter=0;
  const win={
    crypto:{randomUUID:()=>"00000000-0000-4000-8000-"+
      (++uuidCounter).toString(16).padStart(12,"0")},
    addEventListener:(name,handler)=>listener.set(name,handler),
    dispatchEvent:event=>{listener.get(event.type)?.(event);return true;},
    matchMedia:()=>({matches:false})
  };
  const document={
    body:new Node("body"),documentElement:{lang:"ru"},
    getElementById:byId,createElement:tag=>new Node(tag),
    querySelector:selector=>selector===".brand .volleyball-ball"?brandBall:null,
    createTextNode:text=>({nodeType:3,nodeValue:text}),
    addEventListener:()=>{},
    querySelectorAll:selector=>selector==="[data-studio-tab]"?
      [...names.filter(name=>name!=="quick").map(name=>byId("studio-nav-"+name)),
        byId("studio-quick-open")]:[],
    createTreeWalker:()=>({currentNode:null,nextNode:()=>false})
  };
  const requests=[];
  const pipelineId="a".repeat(32);
  const fetch=async(path,options={})=>{
    requests.push({path,options});calls.push({path,options});
    if(path==="/studio/selection.wasm"){
      return {ok:true,arrayBuffer:async()=>wasm.buffer.slice(wasm.byteOffset,wasm.byteOffset+wasm.byteLength)};
    }
    if(path==="/api/projects"){
      return {ok:true,json:async()=>({projects:[{id:"1234abcdef01",name:"Match",videos:["rally.mp4"]}]})};
    }
    if(path.startsWith("/api/studio/analysis-overview?")){
      return {ok:true,json:async()=>({schema:"volley-native-analysis-overview/1",
        metadata_only:true,authoritative_game_events:false,stages:[]})};
    }
    if(path==="/api/studio/pipeline/start"){
      assert.equal(options.method,"POST");
      if(flags.failStartOnce&&!state.droppedStart){
        state.droppedStart=true;
        state.acceptedRequest=JSON.parse(options.body);
        throw new TypeError("Failed to fetch");
      }
      return {ok:true,json:async()=>({schema:"volley-studio-pipeline/1",
        id:pipelineId,status:"queued"})};
    }
    if(path.startsWith("/api/studio/pipeline/status?")){
      const params=new URLSearchParams(path.split("?")[1]);
      if(!params.get("id")){
        if(flags.failStartOnce&&state.acceptedRequest){
          return {ok:true,json:async()=>({
            schema:"volley-studio-pipeline/1",id:pipelineId,
            project_id:state.acceptedRequest.project_id,
            video:state.acceptedRequest.video,mode:state.acceptedRequest.mode,
            client_request_id:state.acceptedRequest.client_request_id,
            status:"running",stage:"Постобработка",progress_percent:86
          })};
        }
        return {ok:true,json:async()=>({status:"idle"})};
      }
      return {ok:true,json:async()=>({schema:"volley-studio-pipeline/1",
        id:pipelineId,project_id:"1234abcdef01",video:"rally.mp4",
        status:"complete",progress_percent:100,stage:"Готово",
        analysis_id:"b".repeat(32),postprocessing_id:"c".repeat(32)})};
    }
    if(path.startsWith("/api/studio/pipeline/log?")){
      return {ok:true,json:async()=>({schema:"volley-studio-pipeline-log/1",
        id:pipelineId,project_id:"1234abcdef01",entries:[],next_offset:0,has_more:false})};
    }
    if(path==="/api/studio/pipeline/cancel"){
      assert.equal(options.method,"POST");
      return {ok:true,json:async()=>({schema:"volley-studio-pipeline/1",
        id:pipelineId,status:"cancelled"})};
    }
    if(path==="/api/studio/pipeline/history"){
      assert.equal(options.method,"POST");
      return {ok:true,json:async()=>({project_id:"1234abcdef01",hidden:2})};
    }
    if(path.startsWith("/api/studio/pipeline/history?")){
      return {ok:true,json:async()=>({schema:"volley-studio-pipeline-history/1",project_id:"1234abcdef01",runs:[],next_offset:0,has_more:false})};
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
  a.nodes.get(name==="quick"?"studio-quick-open":"studio-nav-"+name).dispatch("click");
  await tick();
  assert.equal(a.runtime.get("tab"),i,"WASM tab authority "+name);
  const correct="studio-"+name+"-view";
  assert.equal(a.nodes.get(correct).classList.contains("hidden"),false,
    "Tab DOM visibility "+name);
  assert.equal(a.nodes.get(name==="quick"?"studio-quick-open":"studio-nav-"+name).classList.contains("active"),true);
  assert.equal(a.location.hash,"#"+name);
}
assert.ok(a.requests.some(x=>x.path==="/api/projects"),"Analysis fetched project list");
 a.nodes.get("studio-history-clear").dispatch("click");
 await tick();
 assert.deepEqual(JSON.parse(a.requests.find(x=>x.path==="/api/studio/pipeline/history"&&x.options.method==="POST").options.body),{project_id:"1234abcdef01"});
assert.ok(a.requests.some(x=>x.path==="/api/players/library/native"),"Players fetched library");
a.nodes.get("studio-nav-analysis").dispatch("click");
await tick();
assert.equal(a.nodes.get("studio-analysis-video").value,"rally.mp4");
assert.ok(a.nodes.get("studio-analysis-profile-delta").textContent.includes("Эталонный профиль активен"),
  "Golden reference profile must be reported as active");
const pipelineStarts=()=>a.requests.filter(x=>x.path==="/api/studio/pipeline/start");
a.nodes.get("studio-analysis-start").dispatch("click");
await tick();
assert.equal(pipelineStarts().length,1,"One explicit full-pipeline launch");
const golden=JSON.parse(pipelineStarts()[0].options.body);
assert.equal(golden.project_id,"1234abcdef01");
assert.equal(golden.video,"rally.mp4");
assert.equal(golden.mode,"full");
assert.deepEqual(golden.gpu,{stride:9,gpu_memory_limit_mb:0,cpu_threads:8,pose_enabled:true,pose_stride:3});
assert.equal(golden.post.pose_classification,true);
assert.equal(golden.post.tracking.min_player_score,0.28);
assert.equal(golden.post.tracking.nms_iou,0.72);
assert.equal(golden.post.tracking.min_track_observations,12);
assert.equal(golden.post.touches.threshold,0.48);
assert.equal(golden.post.touches.min_ball_score,0.35);
assert.equal(golden.post.rallies.min_ball_score,0.35);
assert.equal(golden.post.rallies.min_rally_frames,45);
assert.equal(golden.post.rallies.min_touch_candidates,2);
assert.equal(a.nodes.get("studio-analysis-start").textContent,"▶ Полный анализ",
  "Completed pipeline must release the start button");
a.nodes.get("studio-gpu-stride").value="3";
a.nodes.get("studio-gpu-stride").dispatch("input");
a.nodes.get("studio-gpu-memory").value="8192";
a.nodes.get("studio-gpu-memory").dispatch("input");
a.nodes.get("studio-gpu-threads").value="12";
a.nodes.get("studio-gpu-threads").dispatch("input");
assert.match(a.nodes.get("studio-analysis-profile-delta").textContent,/Изменено параметров: 3/);
a.nodes.get("studio-analysis-start").dispatch("click");
await tick();
assert.equal(pipelineStarts().length,2,"Second explicit pipeline launch only");
const override=JSON.parse(pipelineStarts()[1].options.body);
assert.deepEqual(override.gpu,{stride:3,gpu_memory_limit_mb:8192,cpu_threads:12,pose_enabled:true,pose_stride:3});
assert.equal(override.post.tracking.min_player_score,0.28);
a.nodes.get("studio-gpu-memory").value="-1";
a.nodes.get("studio-gpu-memory").dispatch("input");
assert.match(a.nodes.get("studio-analysis-profile-delta").textContent,/недопустимое значение/);
a.nodes.get("studio-analysis-start").dispatch("click");
await tick();
assert.equal(pipelineStarts().length,2,"Invalid GPU settings cannot launch pipeline");
a.nodes.get("studio-analysis-reset").dispatch("click");
await tick();
assert.equal(a.nodes.get("studio-gpu-stride").value,"9");
assert.equal(a.nodes.get("studio-gpu-memory").value,"0");
assert.equal(a.nodes.get("studio-gpu-threads").value,"8");
assert.ok(a.nodes.get("studio-analysis-profile-delta").textContent.includes("Эталонный профиль активен"),
  "Reset must restore the reference profile");
// Each card starts only its own stage, with the CPU source taken from
// the explicitly selected header fusion and no implied GPU rerun.
assert.equal(a.nodes.get("studio-analysis-post-only").disabled,true,
  "CPU-only cannot start without a selected fusion");
a.nodes.get("studio-analysis-gpu-only").dispatch("click");
await tick();
assert.equal(pipelineStarts().length,3,"GPU-only button must launch the pipeline");
const gpuOnly=JSON.parse(pipelineStarts()[2].options.body);
assert.equal(gpuOnly.mode,"gpu");
assert.equal(gpuOnly.post,undefined,"GPU-only must not include CPU settings");
assert.deepEqual(gpuOnly.gpu,{stride:9,gpu_memory_limit_mb:0,cpu_threads:8,pose_enabled:true,pose_stride:3});
a.nodes.get("studio-inference").value=JSON.stringify([
  "rally.mp4","fusion","b".repeat(32)]);
a.window.dispatchEvent({type:"volley:chain-change",
  detail:{project_id:"1234abcdef01",video:"rally.mp4"}});
await tick();
assert.equal(a.nodes.get("studio-analysis-post-only").disabled,false);
a.nodes.get("studio-analysis-post-only").dispatch("click");
await tick();
assert.equal(pipelineStarts().length,4,"CPU-only button starts a separate pipeline");
const postOnly=JSON.parse(pipelineStarts()[3].options.body);
assert.equal(postOnly.mode,"post");
assert.equal(postOnly.source_analysis_id,"b".repeat(32));
assert.equal(postOnly.gpu,undefined,"CPU-only must not request a new GPU run");
assert.equal(postOnly.post.tracking.max_gap_frames,90);
assert.equal(postOnly.post.tracking.min_track_observations,12);
assert.match(postOnly.client_request_id,/^[a-f0-9]{32}$/,
  "Pipeline requests must carry a unique idempotency token");
const recovered=createTestEnvironment("#analysis",{failStartOnce:true});
await recovered.runtime.ready;await tick();
recovered.nodes.get("studio-nav-analysis").dispatch("click");
await tick();
recovered.nodes.get("studio-inference").value=JSON.stringify([
  "rally.mp4","fusion","b".repeat(32)]);
recovered.window.dispatchEvent({type:"volley:chain-change",
  detail:{project_id:"1234abcdef01",video:"rally.mp4"}});
await tick();
recovered.nodes.get("studio-analysis-post-only").dispatch("click");
await tick();
assert.equal(recovered.requests.filter(req=>req.path==="/api/studio/pipeline/start").length,1,
  "A lost response must recover the accepted CPU run without a duplicate POST");
assert.ok(recovered.requests.some(req=>
  req.path.startsWith("/api/studio/pipeline/status?")&&
  !new URLSearchParams(req.path.split("?")[1]).has("id")),
  "Browser must probe durable Rust status after a lost response");
assert.equal(recovered.nodes.get("studio-analysis-note").classList.contains("analysis-error"),false,
  "Recovered CPU runs must not show Failed to fetch");
const old=a.runtime.beginLoad("analysis");
const newer=a.runtime.beginLoad("analysis");
assert.equal(a.runtime.currentLoad("analysis",old),false);
assert.equal(a.runtime.currentLoad("analysis",newer),true);
assert.equal(a.runtime.navigate("invalid"),"preview");
assert.equal(a.runtime.get("tab"),0);
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
delayed.nodes.get("studio-quick-open").dispatch("click");
await tick();
assert.equal(delayed.runtime.get("tab"),4);
delayed.releasePlayers({players:[{id:"f".repeat(12),name:"Stale user"}],revision:"new"});
await tick();
assert.equal(delayed.nodes.get("studio-player-list").children.length,1);
assert.equal(delayed.nodes.get("studio-player-list").children[0].textContent,
  "Загрузка библиотеки игроков…","Old player response must not replace the new tab state");
console.log("Real Rust/WASM browser bridge: six routes/deep links, safe loading, bilingual labels, persisted visual settings and golden full-pipeline defaults passed");
