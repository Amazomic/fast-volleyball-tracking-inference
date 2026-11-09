// Isolated DOM-less browser contract test for the Rust statistics panel.
// No dependencies, network or real user data.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";

const root=join(fileURLToPath(new URL("../..",import.meta.url)));
const source=readFileSync(join(root,"rust/volley-app/static/studio_stats.js"),"utf8");
class Element {
  constructor(tag="div"){
    this.tagName=tag;this.children=[];this.listeners=new Map();
    this.className="";this.classList={
      values:new Set(),add:(s)=>this.classList.values.add(s),
      remove:(s)=>this.classList.values.delete(s),
      toggle:(s,on)=>on?this.classList.values.add(s):this.classList.values.delete(s),
      contains:(s)=>this.classList.values.has(s)
    };
    this.value="";this.disabled=false;this.textContent="";
  }
  append(...items){this.children.push(...items);}
  replaceChildren(...items){this.children=[...items];}
  addEventListener(name,fn){this.listeners.set(name,fn);}
  dispatch(name){return this.listeners.get(name)?.({detail:{tab:"players"}});}
}
const ids=[
  "studio-project","studio-stats-refresh","studio-stats-preview","studio-stats-save",
  "studio-stats-status","studio-stats-sources","studio-stats-summary",
  "studio-stats-total","studio-stats-assigned","studio-stats-unknown",
  "studio-stats-players","studio-stats-diagnostics","studio-stats-source-details"
];
const nodes=new Map(ids.map(id=>[id,new Element()]));
nodes.get("studio-project").value="1234abcdef01";
const sha=c=>c.repeat(64);
const video="match.mp4";
const run="a".repeat(32);
const stats=()=>({
  schema:"volley-native-player-statistics/1",
  algorithm:"verified-touch-frame-and-manual-athlete-links-v1",
  project_id:"1234abcdef01",source_fingerprint:sha("f"),video_count:1,
  sources:[{
    video,rally_run_id:run,source_replay_id:"b".repeat(32),
    source_replay_sha256:sha("c"),source_touch_id:"d".repeat(32),
    source_touch_sha256:sha("e"),source_rally_sha256:sha("1"),
    identity_revision:"absent",total_touch_candidates:5,
    unassigned_touch_candidates:2
  }],
  total_touch_candidates:5,assigned_touch_candidates:3,
  unassigned_touch_candidates:2,
  players:[{
    player_id:"1234abcdef01",name:"Player One",touches:3,
    identity_source:"manually_reviewed_native_track_ids"
  }],
  raw_tracks:[],auto_player_identity_enabled:false,
  automatic_action_classification:false,all_contacts_physically_verified:true,
  snapshot_persisted:false,saved_revision:"absent"
});
let invalidCounts=false;
let calls=[];
const fetch=async(url,options={})=>{
  calls.push({url,options});
  if(url==="/api/projects")return {ok:true,json:async()=>({
    projects:[{id:"1234abcdef01",videos:[video]}]
  })};
  if(url.startsWith("/api/studio/rust/analysis-overview?")){
    return {ok:true,json:async()=>({
      schema:"volley-native-analysis-overview/1",
      metadata_only:true,project_id:"1234abcdef01",video,
      stages:[{key:"rallies",items:[{id:run,sha256:sha("2"),count:2}],truncated:false}]
    })};
  }
  assert.equal(url,"/api/players/native/statistics");
  const payload=JSON.parse(options.body);
  assert.deepEqual(payload.sources,[{rally_run_id:run}]);
  const result=stats();
  if(payload.mode==="recalculate"){
    assert.equal(payload.expected_revision,"absent");
    assert.equal(payload.expected_source_fingerprint,sha("f"));
    result.snapshot_persisted=true;result.saved_revision=sha("3");
  }else{
    assert.equal(payload.expected_revision,undefined);
  }
  if(invalidCounts)result.players[0].touches=4;
  return {ok:true,json:async()=>result};
};
const callbacks=new Map();
vm.runInNewContext(source,{
  document:{getElementById:id=>nodes.get(id),createElement:tag=>new Element(tag)},
  window:{addEventListener:(name,cb)=>callbacks.set(name,cb),
    VolleyballStudioActivity:{fetch:(...args)=>fetch(...args)}},
  location:{hash:"#players"},fetch,URLSearchParams,Set,Map,JSON,Number,String,
  Array,Boolean,Error,Math,RegExp
},{filename:"studio_stats.js"});
const flush=()=>new Promise(resolve=>setTimeout(resolve,0));
for(let i=0;i<5;i++)await flush();
const $=id=>nodes.get("studio-stats-"+id);
assert.equal($("preview").disabled,false,"Preview enabled when CPU source exists");
assert.equal($("save").disabled,true,"Cannot save without preview");
await $("preview").dispatch("click");
for(let i=0;i<3;i++)await flush();
assert.equal($("total").textContent,"5");
assert.equal($("assigned").textContent,"3");
assert.equal($("unknown").textContent,"2");
assert.equal($("save").disabled,false);
assert.equal($("players").children.length,3);
assert.equal(nodes.get("studio-stats-source-details").children.length,2);
await $("save").dispatch("click");
for(let i=0;i<3;i++)await flush();
assert.equal($("save").disabled,false);
assert.match($("status").textContent,/Сохранено/);
const select=$("sources").children[0].children[1];
select.value="";
select.dispatch("change");
assert.equal($("save").disabled,true,"Source change invalidates CAS preview");
assert.equal($("preview").disabled,true,"No source may be counted");
select.value=run;
select.dispatch("change");
invalidCounts=true;
await $("preview").dispatch("click");
for(let i=0;i<3;i++)await flush();
assert.equal($("save").disabled,true,"Inconsistent source totals cannot be saved");
assert.match($("status").textContent,/Количество касаний/);
assert(calls.filter(call=>call.url==="/api/players/native/statistics").length>=3);
console.log("Native Rust player-statistics UI: source selection, CAS, verified totals, missing identity revision and invalidation passed");
