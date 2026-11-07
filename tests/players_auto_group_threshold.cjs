"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const vm=require("node:vm");
const root=path.resolve(__dirname,"../scripts/volleyball_ui/static");
const html=fs.readFileSync(path.join(root,"index.html"),"utf8");
const source=fs.readFileSync(path.join(root,"app.js"),"utf8");
assert(html.includes('data-player-gallery="unassigned"'));
assert(html.includes('>Неразмеченные ID</button>'));
assert(!html.includes("Ручная группировка"));
assert(!html.includes('id="player-groups-merge"'));
assert(!html.includes('id="player-groups-split"'));
assert(html.includes('id="player-reid-threshold-number"'));
assert(source.includes('thresholdNumber.addEventListener("change"'));
assert(source.includes('thresholdRange.addEventListener("change"'));

const elements=new Map();
function el(selector){
  if(!elements.has(selector))elements.set(selector,{
    textContent:"",value:"",innerHTML:"",disabled:false,hidden:false,
    classList:{toggle(){},add(){},remove(){}},
    querySelectorAll(){return []},setAttribute(){},
    insertAdjacentHTML(mode,content){this.innerHTML+=content}
  });
  return elements.get(selector);
}
const ctx=vm.createContext({window:{},document:{querySelector:el,querySelectorAll:()=>[],
  activeElement:null,addEventListener(){}},console,setTimeout,clearTimeout,URLSearchParams});
vm.runInContext(source,ctx,{filename:"app.js"});
const run=code=>vm.runInContext(code,ctx);
run([
  'analysisCacheId=()=>"gpu";',
  'state.project="p";state.video="match.mp4";',
  'state.players=[{id:"a",name:"Анна"}];',
  'state.libraryPlayers=state.players.slice();',
  'state.report={raw:Array.from({length:13},(_,i)=>({',
  'player_track_id:101+i,duration_sec:5,detected_frames:20})),',
  'groups:[{id:"a",name:"Анна",track_ids:["109"]}]};',
  'state.playerGroupsKey=playerGroupsContext();',
  'state.playerGroups={status:"ready",method:"openvino_reid",threshold_percent:85,',
  'groups:[',
  '{id:"strong",track_ids:["101","102"],similarity:.92,source:"automatic"},',
  '{id:"weak",track_ids:["103","104"],similarity:.83,source:"automatic"},',
  '{id:"no-reid",track_ids:["105","106"],similarity:null,source:"automatic_retrack"},',
  '{id:"manual",track_ids:["107","108"],similarity:.99,source:"manual"},',
  '{id:"partial",track_ids:["109","110"],similarity:.97,source:"automatic"},',
  '{id:"medium",track_ids:["112","113"],similarity:.87,source:"automatic_retrack_reid"}',
  '],unmatched_ids:["111"],player_matches:{}};',
  'renderPlayerGroups();'
].join("\n"));
const list=()=>el("#player-groups-list").innerHTML;
const status=()=>el("#player-groups-status").textContent;
assert(list().includes('data-group-link="strong"'),"Qualified strong group must be shown");
assert(list().includes('data-group-link="medium"'),"Qualified medium group must be shown");
for(const id of [103,104,105,106,107,108,109,110,111]){
  assert(!list().includes("#"+id),"invalid, unmatched or partial ID must stay hidden: "+id);
}
assert.equal((list().match(/class="player-appearance-card"/g)||[]).length,2);
assert.equal(status(),"","Ready groups should not show verbose diagnostics");


run('state.playerGroups.threshold_percent=90;renderPlayerGroups()');
assert(list().includes('data-group-link="strong"'),"Qualified strong group must be shown");
assert(!list().includes("#112"));
assert.equal((list().match(/class="player-appearance-card"/g)||[]).length,1);
run('state.playerGroups.threshold_percent=95;renderPlayerGroups()');
assert(!list().includes('class="player-appearance-card"'));
assert(list().includes("Групп, прошедших порог 95%"));
assert.equal(el("#player-reid-threshold").value,"95");
assert.equal(el("#player-reid-threshold-number").value,"95");
run('state.playerMode="manual";renderPlayers()');
const manual=el("#raw-players").innerHTML;
assert(manual.includes("#103")&&manual.includes("#111"),
  "all unassigned IDs remain accessible for manual binding");
console.log("PASS: automatic mode only scored groups above threshold; all others manually linkable");
