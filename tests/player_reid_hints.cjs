"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const vm=require("node:vm");
const path=require("node:path");

const staticRoot=path.resolve(__dirname,"../scripts/volleyball_ui/static");
const html=fs.readFileSync(path.join(staticRoot,"index.html"),"utf8");
assert(html.includes('id="player-reid-threshold"'));
assert(html.includes('ВЕРОЯТНЫЙ ИГРОК (ReID)'));
const elements=new Map();
const el=selector=>{
  if(!elements.has(selector)){
    elements.set(selector,{
      value:"",innerHTML:"",textContent:"",disabled:false,hidden:false,
      checked:false,indeterminate:false,
      classList:{toggle(){},add(){},remove(){}},
      querySelectorAll(){return []},setAttribute(){}
    });
  }
  return elements.get(selector);
};
const context=vm.createContext({
  document:{querySelector:el,querySelectorAll:()=>[],activeElement:null,addEventListener(){}},
  window:{},console,setTimeout,clearTimeout,URLSearchParams
});
vm.runInContext(fs.readFileSync(path.join(staticRoot,"app.js"),"utf8"),context);
vm.runInContext([
  'state.project="proj";state.video="match.mp4";',
  'state.players=[{id:"p1",name:"Игрок А"},{id:"p2",name:"Игрок Б"}];',
  'state.libraryPlayers=state.players.slice();',
  'state.playerFilter="all";',
  'state.report={groups:[],raw:[',
  '{player_track_id:101,duration_sec:3,detected_frames:60},',
  '{player_track_id:102,duration_sec:4,detected_frames:80},',
  '{player_track_id:103,duration_sec:1,detected_frames:2}',
  ']};',
  'state.playerGroupsKey=playerGroupsContext();',
  'state.playerGroups={status:"ready",threshold_percent:85,',
  'groups:[],unmatched_ids:["101","102","103"],missing_visual_ids:["103"],',
  'reference_counts:{p1:12,p2:6},',
  'player_matches:{',
  '  "101":{player_id:"p1",name:"Игрок А",similarity:.93,above_threshold:true,ambiguous:false,reference_count:12},',
  '  "102":{player_id:"p2",name:"Игрок Б",similarity:.66,above_threshold:false,ambiguous:false,reference_count:6}',
  '},',
  'nearest_tracks:{"101":[{track_id:"102",similarity:.86}]}',
  '};renderPlayers();'
].join("\n"),context);
const rows=el("#raw-players").innerHTML;
assert(rows.includes("Игрок А"),"strong candidate is visible");
assert(rows.includes("Игрок Б"),"weak candidate is visible");
assert(rows.includes("93%"),"similarity is shown");
assert(rows.includes("66%"),"low similarity is not suppressed");
assert(rows.includes("ниже порога"),"weak evidence is labelled");
assert(rows.includes("Нет пригодного кадра"),"missing video crop is explicit");
assert(rows.includes("Ближайший ID: #102"),"the other track was compared");
assert.equal(vm.runInContext("state.selected.size",context),0);
assert.equal(vm.runInContext("state.report.groups.length",context),0,
             "hints must not assign any player");
console.log("PASS: all-ID similarity hints and project player suggestions remain informational");
