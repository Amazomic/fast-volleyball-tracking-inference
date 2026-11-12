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
  "studio-stats-rallies","studio-stats-attacks","studio-stats-blocks","studio-stats-mean",
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
  rally_candidates:2,mean_player_touches:3,estimated_hits:2,
  unassigned_touch_candidates:2,
  players:[{
    player_id:"1234abcdef01",name:"Player One",touches:3,activity:1,unclassified_touches:3,
    identity_source:"manually_reviewed_native_track_ids"
  }],
  raw_tracks:[],auto_player_identity_enabled:false,
  automatic_action_classification:true,
  automatic_action_classification_verified:false,
  estimated_actions:{attack:1,block:2,serve:1},all_contacts_physically_verified:true,
  snapshot_persisted:false,saved_revision:"absent"
});
let invalidCounts=false;
let statsWithoutPredictions=false;
let calls=[];
const fetch=async(url,options={})=>{
  calls.push({url,options});
  if(url==="/api/projects")return {ok:true,json:async()=>({
    projects:[{id:"1234abcdef01",videos:[video]}]
  })};
  if(url.startsWith("/api/studio/analysis-overview?")){
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
  if(statsWithoutPredictions){result.automatic_action_classification=false;result.estimated_actions={};}
  return {ok:true,json:async()=>result};
};
const callbacks=new Map();
const overviewSource=readFileSync(join(root,"rust/volley-app/static/studio_overview.js"),"utf8");
const context={
  document:{getElementById:id=>nodes.get(id),createElement:tag=>new Element(tag)},
  window:{addEventListener:(name,cb)=>callbacks.set(name,cb),
    VolleyballStudioActivity:{fetch:(...args)=>fetch(...args)}},
  location:{hash:"#report"},fetch,URLSearchParams,Set,Map,JSON,Number,String,
  Array,Boolean,Error,Math,RegExp
};
vm.runInNewContext(overviewSource,context,{filename:"studio_overview.js"});
vm.runInNewContext(source,context,{filename:"studio_stats.js"});
const flush=()=>new Promise(resolve=>setTimeout(resolve,0));
for(let i=0;i<5;i++)await flush();
const $=id=>nodes.get("studio-stats-"+id);
assert.equal($("total").textContent,"5");
assert.equal($("rallies").textContent,"2");
assert.equal($("attacks").textContent,"1");
assert.equal($("blocks").textContent,"2");
assert.equal($("players").children.length,1);
assert.equal($("players").children[0].children[0].textContent,"Player One");
assert.match($("players").children[0].children[1].textContent,/3/);
assert.match($("players").children[0].children[2].textContent,/3/);
assert.equal(calls.filter(call=>call.url==="/api/players/native/statistics").length,1);
assert.equal(JSON.parse(calls.at(-1).options.body).mode,"preview");
statsWithoutPredictions=true;
await callbacks.get("volley:tab-change")({detail:{tab:"report"}});
for(let i=0;i<5;i++)await flush();
assert.equal($("attacks").textContent,"—");
assert.equal($("blocks").textContent,"—");
invalidCounts=true;
await callbacks.get("volley:tab-change")({detail:{tab:"report"}});
for(let i=0;i<5;i++)await flush();
assert.match($("status").textContent,/Количество касаний/);
assert.equal($("players").children.length,0);
console.log("Native report: automatic totals, player cards and invalid data passed");
