"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const vm=require("node:vm");
const path=require("node:path");

const source=fs.readFileSync(path.resolve(__dirname,"../scripts/volleyball_ui/static/app.js"),"utf8");
const elements=new Map();
function el(selector){
  if(!elements.has(selector)){
    elements.set(selector,{value:"",innerHTML:"",textContent:"",checked:false,
      disabled:false,hidden:false,classList:{toggle(){},add(){},remove(){}},
      querySelectorAll(){return []},setAttribute(){}});
  }
  return elements.get(selector);
}
const ctx=vm.createContext({window:{},document:{querySelector:el,querySelectorAll:()=>[],
  activeElement:null,addEventListener(){}},URLSearchParams,console,setTimeout,clearTimeout});
vm.runInContext(source,ctx);
vm.runInContext([
  'analysisCacheId=()=>"gpu-run";',
  'state.project="project";state.video="match.mp4";',
  'state.players=[{id:"p1",name:"Анна"},{id:"p2",name:"Борис"}];',
  'state.libraryPlayers=state.players.slice();',
  'state.activePlayerId="p1";',
  'state.report={groups:[{id:"p1",name:"Анна",track_ids:["101"]}],raw:[',
  '{player_track_id:101,duration_sec:10,detected_frames:80},',
  '{player_track_id:102,duration_sec:8,detected_frames:75},',
  '{player_track_id:103,duration_sec:2,detected_frames:20},',
  '{player_track_id:104,duration_sec:2,detected_frames:21} ]};',
  'state.playerGroupsKey=playerGroupsContext();',
  'state.playerGroups={status:"ready",method:"openvino_reid",threshold_percent:85,',
  'groups:[{id:"g1",track_ids:["101","102","104"],similarity:.93}],unmatched_ids:["103"],',
  'player_matches:{}};',
  'renderPlayerGroups();'
].join("\n"),ctx);
assert(el("#player-groups-list").innerHTML.includes("#102"));
assert(el("#player-groups-list").innerHTML.includes("2 ID в группе"),"Group should include both unassigned tracks without listing every ID");
assert(!el("#player-groups-list").innerHTML.includes("#103"),"unmatched ID belongs only to manual binding");
assert(!el("#player-groups-list").innerHTML.includes("Track #101"),
  "confirmed track is removed from automatic unassigned candidates");
assert(el("#player-groups-list").innerHTML.includes("group-player-picker"));
vm.runInContext([
  'globalThis.saved=[];',
  'api=async(endpoint,payload)=>{saved.push({endpoint,payload});return {}};',
  'loadReport=async()=>{state.report.groups=[',
  '{id:"p1",name:"Анна",track_ids:["101","102"]}];renderPlayers();};'
].join("\n"),ctx);
(async()=>{
  await vm.runInContext('assignSpecificTracks(["101","102","102"],"p1")',ctx);
  const saved=vm.runInContext("saved",ctx);
  assert.equal(saved.length,1);
  assert.equal(saved[0].endpoint,"/api/assign");
  assert.deepEqual([...saved[0].payload.ids],["102"]);
  assert.equal(saved[0].payload.player_id,"p1");
  assert.equal(vm.runInContext("state.reportDirty",ctx),true);
  assert.equal(vm.runInContext("state.report.groups[0].track_ids.includes('102')",ctx),true);
  vm.runInContext("renderPlayerGroups()",ctx); // Re-render only when revisiting automatic mode.
  assert(!el("#player-groups-list").innerHTML.includes("#102"));
  assert(!el("#player-groups-list").innerHTML.includes("#104"),
    "a partial group with one unassigned ID is not an automatic group");
  assert(!el("#player-groups-list").innerHTML.includes("#103"));
  console.log("PASS: unified players modes, direct link, confirmed-ID protection");
})().catch(error=>{console.error(error);process.exitCode=1});
