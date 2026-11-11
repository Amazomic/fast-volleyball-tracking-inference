import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {createRequire} from "node:module";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
import {runInNewContext} from "node:vm";

const root=join(fileURLToPath(new URL("../..",import.meta.url)));
const require=createRequire(import.meta.url);
const chain=require(join(root,"rust/volley-app/static/studio_chain.js"));
chain.setRuntime((await WebAssembly.instantiate(readFileSync(join(root,"rust/volley-app/static/volley_web.wasm")),{})).instance.exports);
const js=readFileSync(join(root,"rust/volley-app/static/studio.js"),"utf8");
const segment=(start,end)=>{
  const a=js.indexOf(start), b=js.indexOf(end,a);
  assert.ok(a>=0&&b>a,"Missing block "+start);
  return js.slice(a,b);
};
const kv=new Map();
const storage={
  getItem:key=>kv.get(key)||null,
  setItem:(key,value)=>kv.set(key,value),
  removeItem:key=>kv.delete(key)
};
const env={localStorage:storage,window:{VolleyStudioChain:chain},console};
runInNewContext(segment("  const chainSnapshot=(()=>{","  const url=(path,params)=>")+
  "\nthis.chainSnapshot=chainSnapshot;",env);
const {chainSnapshot}=env;
const project="0123456789ab",video="match.mp4";
const gpu="a".repeat(32),replay="b".repeat(32),rally="c".repeat(32);
const overview={
  schema:"volley-native-analysis-overview/1",project_id:project,video,
  metadata_only:true,stages:[
    {key:"fusion",items:[{id:gpu,created_at:1}]},
    {key:"tracking",items:[{id:replay,source_fusion_id:gpu}]}
  ]
};
const runs=[{
  video,rally_run_id:rally,source_replay_id:replay,candidate_count:10
}];
chainSnapshot.save(project,[video],new Map([[video,overview]]),runs);
const restored=chainSnapshot.read(project,[video]);
assert.ok(restored,"Stored chain should hydrate on reload");
assert.equal(restored.choices.length,1);
assert.equal(restored.runToAnalysis.get(rally),restored.choices[0].value);
assert.equal(chainSnapshot.read(project,["other.mp4"]),null,
  "A changed project video list must invalidate its source-scoped cache");
assert.equal(chainSnapshot.read("bbbbbbbbbbbb",[video]),null);
const options=(select,rows,value,label,placeholder)=>{
  select.options=rows.map(row=>({value:String(row[value]),textContent:label(row)}));
  select.value="";
  select.placeholder=placeholder;
};
const s={
  project,projectVideos:[video],chain:null,chainEpoch:0,analysis:"",run:"",generation:0,
  items:[],selected:{clear(){}}
};
const ui={analysis:{value:"",disabled:true},run:{value:"",disabled:true}};
const msgs=[],events=[];
const saved={project,analysis:restored.choices[0].value,run:rally};
let resolveRuns,cardsRequested=0,metadataReads=0;
const metadata=new Promise(resolve=>{resolveRuns=resolve;});
const context={
  s,ui,window:{VolleyStudioChain:chain,dispatchEvent:e=>events.push(e.type)},
  CustomEvent:class {constructor(type,options){this.type=type;this.detail=options.detail;}},
  sourceSelection:{save:(p,a,r)=>{saved.project=p;saved.analysis=a;saved.run=r;}},
  chainSnapshot,url:(path,params)=>path+"?"+new URLSearchParams(params),
  previewCache:{load:path=>{
    metadataReads++;
    return path.includes("/runs?")?metadata:Promise.resolve(overview);
  }},
  reset:()=>{s.run="";s.generation++;s.items=[];},
  options,loadPage:()=>{cardsRequested++;return new Promise(()=>{});},
  loadTimeline:()=>Promise.resolve(),
  message:value=>msgs.push(value),failed:error=>{throw error;},
  console
};
runInNewContext(segment("  const setRuns=async(","  const projects=async(")+
  "\nthis.loadChain=loadChain;",context);
let releaseReady;
context.window.VolleyballUi={ready:new Promise(resolve=>{releaseReady=resolve;})};
const pending=context.loadChain(saved.analysis,saved.run);
await new Promise(resolve=>setImmediate(resolve));
assert.equal(metadataReads,0,"Chain waits for Rust/WASM readiness before cache or API access");
releaseReady();
await new Promise(resolve=>setImmediate(resolve));
assert.equal(s.run,rally,"Cached CPU run must be usable before metadata requests finish");
assert.equal(ui.analysis.disabled,false);
assert.equal(ui.run.disabled,false);
assert.equal(cardsRequested,1,"CPU preview should run without blocking catalog UI");
assert.ok(!msgs.some(x=>x.includes("Поиск")||x.includes("Первичная загрузка")),
  "Warm reload should not show the catalog search banner");
resolveRuns({runs});
await pending;
assert.equal(cardsRequested,1,"Matching metadata must not reset or reload preview");
assert.equal(metadataReads,2,"A warm load still validates the source index in background");
chainSnapshot.drop();
assert.equal(chainSnapshot.read(project,[video]),null);
console.log("Studio GPU/CPU chain cache: warm restoration, scoped invalidation and async preview: OK");
