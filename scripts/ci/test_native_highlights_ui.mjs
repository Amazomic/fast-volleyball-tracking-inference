// Dependency-free browser workflow test for verified attack reels.
// Simulates only DOM/network; source/action authority remains in Rust.
import {readFileSync} from "node:fs";
import {fileURLToPath} from "node:url";
import {join} from "node:path";
import vm from "node:vm";
import assert from "node:assert/strict";

const root=join(fileURLToPath(new URL("../..",import.meta.url)));
const source=readFileSync(join(root,"rust/volley-app/static/studio_highlights.js"),"utf8");
class Element {
  constructor(tag="div"){
    this.tagName=tag;this.children=[];this.listeners=new Map();
    this.className="";this.value="";this.disabled=false;
    this.textContent="";this.checked=false;
    this.classList={
      set:new Set(),
      add:k=>this.classList.set.add(k),
      remove:k=>this.classList.set.delete(k),
      toggle:(k,on)=>on?this.classList.set.add(k):this.classList.set.delete(k),
      contains:k=>this.classList.set.has(k),
    };
  }
  get firstChild(){return this.children[0]||null;}
  append(...items){this.children.push(...items);}
  replaceChildren(...items){this.children=[...items];}
  addEventListener(name,fn){this.listeners.set(name,fn);}
  dispatch(name){return this.listeners.get(name)?.({target:this});}
  querySelector(name){return this.children.find(n=>n.tagName===name)||null;}
  removeAttribute(name){if(name==="href")this.href=undefined;}
}
const ids=[
  "studio-project","studio-highlights-player","studio-highlights-mode",
  "studio-highlights-count","studio-highlights-refresh",
  "studio-highlights-preview","studio-highlights-start",
  "studio-highlights-cancel","studio-highlights-status",
  "studio-highlights-sources","studio-highlights-candidates",
  "studio-highlights-job","studio-highlights-download","studio-encoder"
];
const nodes=new Map(ids.map(id=>[id,new Element()]));
const $=id=>nodes.get("studio-highlights-"+id);
nodes.get("studio-project").value="1234abcdef01";
nodes.get("studio-encoder").value="cpu";
$("mode").value="auto";$("count").value="12";
const run="a".repeat(32), keyA=run+":"+"b".repeat(24),keyB=run+":"+"c".repeat(24);
const candidate=(key,count)=>({
  key,candidate_id:key.split(":")[1],rally_run_id:run,
  video:"match.mp4",detected_start_frame:10,detected_end_frame:70,
  manually_verified_player_attacks:count,
  action_label_source:"explicit_human_review",
  athlete_link_source:"manually_reviewed_native_track_ids"
});
let forgeAction=false;
let noAttacks=false;
let currentJob="idle";
let requests=[];
const responseBody=payload=>{
  const candidates=noAttacks?[]:[candidate(keyA,2),candidate(keyB,1)];
  if(forgeAction)candidates[0].action_label_source="automatic_reid";
  const selected=payload.selection_mode==="manual"?
    payload.manual_rally_keys.map(k=>candidates.find(c=>c.key===k)):
    [candidates[0]];
  return {
    schema:"volley-native-reviewed-player-highlights/1",
    project_id:"1234abcdef01",player_id:"athlete1",player_name:"Athlete 1",
    source_fingerprint:"f".repeat(64),sources:[],candidates,selected,
    total_eligible_rallies:2,selected_clips:selected.length,
    selected_verified_attacks:selected.reduce((n,c)=>n+c.manually_verified_player_attacks,0),
    reviewed_actions_only:true,manually_linked_athlete_only:true,
    automatic_action_classification:false,render_source:"original_recording",
  };
};
const fetch=async(url,options={})=>{
  requests.push({url,options});
  if(url==="/api/projects")return {ok:true,json:async()=>({
    projects:[{id:"1234abcdef01",videos:["match.mp4"],player_ids:["athlete1"]}]
  })};
  if(url==="/api/players/library/native"){
    assert.equal(JSON.parse(options.body).action,"read");
    return {ok:true,json:async()=>({
      players:[{id:"athlete1",name:"Athlete 1"}],revision:"absent"
    })};
  }
  if(url.startsWith("/api/studio/rust/analysis-overview?"))
    return {ok:true,json:async()=>({
      schema:"volley-native-analysis-overview/1",
      project_id:"1234abcdef01",video:"match.mp4",metadata_only:true,
      stages:[{key:"rallies",items:[{id:run,count:4}],truncated:false}]
    })};
  if(url==="/api/montage/native/player-highlights"){
    const payload=JSON.parse(options.body);
    assert.deepEqual(payload.rally_run_ids,[run]);
    assert.equal(payload.project_id,"1234abcdef01");
    assert.equal(payload.player_id,"athlete1");
    const data=responseBody(payload);
    if(payload.operation==="start"){
      assert.equal(payload.expected_source_fingerprint,"f".repeat(64));
      data.job_id="montage_"+"1".repeat(32);
      data.status="queued";currentJob="running";
    }else assert.equal(payload.expected_source_fingerprint,undefined);
    return {ok:true,json:async()=>data};
  }
  if(url==="/api/montage/native/status")
    return {ok:true,json:async()=>({
      id:"montage_"+"1".repeat(32),state:currentJob,
      stage:currentJob,overall_percent:45,
    })};
  if(url==="/api/montage/native/cancel"){
    assert.equal(JSON.parse(options.body).job_id,"montage_"+"1".repeat(32));
    currentJob="cancelled";
    return {ok:true,json:async()=>({
      job_id:"montage_"+"1".repeat(32),cancel_requested:true,
    })};
  }
  throw Error("Unexpected network call "+url);
};
const eventCallbacks=new Map();
vm.runInNewContext(source,{
  document:{getElementById:id=>nodes.get(id),createElement:tag=>new Element(tag)},
  window:{addEventListener:(n,fn)=>eventCallbacks.set(n,fn)},
  location:{hash:"#montage"},fetch,URLSearchParams,Set,Map,JSON,Number,
  String,Boolean,Array,Error,Math,RegExp,Promise,
  setInterval:()=>1,clearInterval:()=>{},
},{filename:"studio_highlights.js"});
const tick=async()=>{for(let i=0;i<5;i++)await new Promise(r=>setTimeout(r,0));};
await tick();
assert.equal($("player").value,"athlete1");
assert.equal($("preview").disabled,false);
assert.equal($("start").disabled,true);
await $("preview").dispatch("click");await tick();
assert.equal($("start").disabled,false);
assert.equal($("candidates").children.length,2);
assert.match($("status").textContent,/Подтверждённых атак: 2/);
$("mode").value="manual";$("mode").dispatch("change");
assert.equal($("start").disabled,true,"Switching selection mode invalidates preview");
const second=$("candidates").children[1].querySelector("input");
second.checked=true;second.dispatch("change");
await $("preview").dispatch("click");await tick();
let last=requests.filter(r=>r.url==="/api/montage/native/player-highlights").at(-1);
assert.equal(JSON.parse(last.options.body).selection_mode,"manual");
assert.deepEqual(JSON.parse(last.options.body).manual_rally_keys,[keyA,keyB]);
assert.equal($("start").disabled,false);
await $("start").dispatch("click");await tick();
last=requests.filter(r=>r.url==="/api/montage/native/player-highlights").at(-1);
assert.equal(JSON.parse(last.options.body).operation,"start");
assert.equal(JSON.parse(last.options.body).expected_source_fingerprint,"f".repeat(64));
assert.equal($("cancel").classList.contains("hidden"),false);
await $("cancel").dispatch("click");await tick();
assert.match($("job").textContent,/отменён/);
assert.equal($("download").classList.contains("hidden"),true);
assert.equal($("cancel").classList.contains("hidden"),true);
forgeAction=true;
await $("preview").dispatch("click");await tick();
assert.equal($("start").disabled,true,"Forged automatic action must veto montage");
assert.match($("status").textContent,/недостоверные события/);
forgeAction=false;noAttacks=true;
$("mode").value="auto";$("mode").dispatch("change");
await $("preview").dispatch("click");await tick();
assert.equal($("start").disabled,true,"Empty verified attack preview cannot launch an empty job");
console.log("Reviewed-player attack UI: project/roster, SHA preview, manual override, start, cancel and forged-action veto passed");
