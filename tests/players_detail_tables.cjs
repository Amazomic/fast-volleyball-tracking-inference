"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const vm=require("node:vm");
const elements=new Map();
function element(key){
  if(!elements.has(key))elements.set(key,{innerHTML:"",textContent:"",hidden:false,
    disabled:false,value:"",dataset:{},style:{},setAttribute(name,value){this[name]=value},
    classList:{toggle(){}},querySelectorAll(){return []}});
  return elements.get(key);
}
const tabs=["tracks","photos"].map(mode=>({...element(mode),dataset:{playerTable:mode}}));
const panes=["tracks","photos"].map(mode=>({...element("pane-"+mode),dataset:{playerTablePane:mode}}));
const ctx=vm.createContext({document:{querySelector:element,querySelectorAll(selector){
  return selector==="[data-player-table]"?tabs:selector==="[data-player-table-pane]"?panes:[];
},addEventListener(){}},window:{},console,URLSearchParams,setTimeout,clearTimeout});
vm.runInContext(fs.readFileSync("scripts/volleyball_ui/static/app.js","utf8"),ctx);
vm.runInContext('state.project="project";state.video="match.mp4";state.activePlayerId="p1";'+
  'state.players=[{id:"p1",name:"Анна"},{id:"p2",name:"Борис"}];'+
  'state.report={groups:[{id:"p1",track_ids:["11","12"],link_meta:{"12":{reviewed:false}}},'+
  '{id:"p2",track_ids:["21"]}],raw:[{player_track_id:11,duration_sec:5,detected_frames:100},'+
  '{player_track_id:12,duration_sec:2},{player_track_id:21,duration_sec:8}]};'+
  'analysisCacheId=()=>"cache.cpu";',ctx);
let albumRequests=0;
ctx.api=async()=>{albumRequests++;return {photos:[]}};
vm.runInContext('setPlayerTable("tracks")',ctx);
assert.equal(panes[0].hidden,false);
assert.equal(panes[1].hidden,true);
assert.equal(tabs[0]["aria-pressed"],"true");
assert.match(element("#player-linked-tracks").innerHTML,/#11/);
assert.match(element("#player-linked-tracks").innerHTML,/#12/);
assert.doesNotMatch(element("#player-linked-tracks").innerHTML,/#21/);
assert.equal(albumRequests,0);
vm.runInContext('setPlayerTable("photos")',ctx);
assert.equal(panes[0].hidden,true);
assert.equal(panes[1].hidden,false);
assert.equal(albumRequests,1);
vm.runInContext('state.activePlayerId="p2";setPlayerTable("tracks")',ctx);
assert.match(element("#player-linked-tracks").innerHTML,/#21/);
assert.doesNotMatch(element("#player-linked-tracks").innerHTML,/#11/);
vm.runInContext('state.activePlayerId=null;renderPlayerDetails()',ctx);
assert.equal(element("#backfill-player-photos").disabled,true);
assert.match(element("#player-linked-tracks").innerHTML,/Выберите/);
(async()=>{
  await new Promise(resolve=>setImmediate(resolve));
  vm.runInContext('state.activePlayerId="p1";state.playerTable="tracks";',ctx);
  let request;
  ctx.api=async(endpoint,payload)=>{
    request={endpoint,payload};
    return {copied:3,count:8,errors:[]};
  };
  ctx.renderPlayers=()=>{};
  await vm.runInContext('savePlayerLinkedPhotos()',ctx);
  assert.equal(request.endpoint,"/api/players/photos/save");
  assert.equal(request.payload.player_id,"p1");
  assert.equal(request.payload.project_id,"project");
  assert.equal(request.payload.video,"match.mp4");
  assert.equal(request.payload.cache_id,"cache.cpu");
  assert.equal(vm.runInContext('state.photoCounts.p1',ctx),8);
  assert.match(element("#players-action-feedback").textContent,/3/);
  assert.equal(element("#backfill-player-photos").disabled,false);
  let finish;
  ctx.api=()=>new Promise(resolve=>{finish=resolve});
  const pending=vm.runInContext('savePlayerLinkedPhotos()',ctx);
  assert.equal(element("#backfill-player-photos").disabled,true);
  vm.runInContext('state.activePlayerId="p2";state.playerGroups={status:"ready"};',ctx);
  element("#players-action-feedback").textContent="Другой игрок";
  finish({copied:2,count:10,errors:[]});
  await pending;
  assert.equal(element("#players-action-feedback").textContent,"Другой игрок");
  assert.equal(vm.runInContext('state.playerGroups.status',ctx),"ready");
  ctx.api=async()=>{throw new Error("Нет исходного кадра")};
  await vm.runInContext('savePlayerLinkedPhotos()',ctx);
  assert.match(element("#players-action-feedback").textContent,/Нет исходного кадра/);
  assert.equal(element("#backfill-player-photos").disabled,false);
  vm.runInContext('state.activePlayerId="p1";',ctx);
  ctx.api=async()=>({copied:0,count:10,errors:["Track #11: нет кадра"]});
  await vm.runInContext('savePlayerLinkedPhotos()',ctx);
  assert.match(element("#players-action-feedback").textContent,/Track #11: нет кадра/);
  console.log("PASS: exclusive player tables, player filtering, scoped photo saving");
})().catch(error=>{console.error(error);process.exitCode=1});
