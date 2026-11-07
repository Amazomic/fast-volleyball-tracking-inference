"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const vm=require("node:vm");

const source=fs.readFileSync(path.resolve(__dirname,"../scripts/volleyball_ui/static/app.js"),"utf8");
const elements=new Map();
function el(selector){
  if(!elements.has(selector))elements.set(selector,{
    innerHTML:"",textContent:"",value:"",checked:false,disabled:false,hidden:false,
    style:{},classList:{toggle(){},add(){},remove(){}},setAttribute(){},
    querySelectorAll(){return []}
  });
  return elements.get(selector);
}
const ctx=vm.createContext({window:{},document:{querySelector:el,querySelectorAll:()=>[],
  activeElement:null,addEventListener(){}},URLSearchParams,console,setTimeout,clearTimeout});
vm.runInContext(source,ctx,{filename:"app.js"});
vm.runInContext([
  'analysisCacheId=()=>"gpu";',
  'state.project="project";state.video="match.mp4";',
  'state.players=[{id:"p1",name:"Анна"},{id:"p2",name:"Борис"},{id:"p3",name:"Вера"}];',
  'state.activePlayerId="p1";',
  'state.libraryPlayers=state.players.slice();',
  'state.report={groups:[],raw:[',
  '{player_track_id:201,duration_sec:20,detected_frames:90},',
  '{player_track_id:202,duration_sec:21,detected_frames:90},',
  '{player_track_id:203,duration_sec:22,detected_frames:90},',
  '{player_track_id:204,duration_sec:10,detected_frames:90}]};',
  'state.playerGroupsKey=playerGroupsContext();',
  'state.playerGroups={status:"ready",method:"openvino_reid",threshold_percent:85,',
  'groups:[{id:"g1",track_ids:["201","202","203"],similarity:0.9}],unmatched_ids:["204"],',
  'player_matches:{',
  '201:{player_id:"p1",name:"Анна",similarity:0.97,above_threshold:true,ambiguous:false},',
  '202:{player_id:"p2",name:"Борис",similarity:0.88,above_threshold:true,ambiguous:false},',
  '203:{player_id:"p2",name:"Борис",similarity:0.89,above_threshold:true,ambiguous:false},',
  '204:{player_id:"p3",name:"Вера",similarity:0.7,above_threshold:false,ambiguous:true}}};'
].join("\n"),ctx);
const run=code=>vm.runInContext(code,ctx);
const picked=run('suggestedPlayerForTracks(["201","202","203"])');
assert.equal(picked.playerId,"p2","group majority wins over higher first-ID score");
assert.equal(picked.coverage,"2/3");
assert.equal(Math.round(picked.similarity*100),89);
const groupHtml=run('playerPickerHtml("group-player-picker",["201","202","203"],"g:g1")');
assert.match(groupHtml,/<option value="p2" selected>Борис · 89% сходство<\/option>/);
assert(!groupHtml.includes('<option value="p1" selected>'),
  "active player must not override model recommendation");
assert.match(run('suggestedPlayerHint(["201","202","203"])'),/2\/3 ID/);
assert.match(run('playerPickerHtml("track-player-picker",["204"],"t:204")'),
  /<option value="p3" selected>Вера · 70% сходство<\/option>/);
assert.match(run('suggestedPlayerHint(["204"])'),/неоднозначно, проверьте/);
assert.match(run('playerPickerHtml("track-player-picker",["205"],"t:205")'),
  /<option value="" selected>Выберите игрока…<\/option>/);

ctx.override={dataset:{playerPickKey:"g:g1"},value:"p1"};
run('rememberPlayerPickerChange(override)');
assert.match(run('playerPickerHtml("group-player-picker",["201","202","203"],"g:g1")'),
  /<option value="p1" selected>Анна<\/option>/,
  "manual override must survive rerender");
ctx.override.value="";
run('rememberPlayerPickerChange(override)');
assert.match(run('playerPickerHtml("group-player-picker",["201","202","203"],"g:g1")'),
  /<option value="" selected>Выберите игрока…<\/option>/,
  "user must be able to clear the suggested selection");
run('state.video="other.mp4"');
assert.match(run('playerPickerHtml("group-player-picker",["201","202","203"],"g:g1")'),
  /<option value="p2" selected>Борис · 89% сходство<\/option>/,
  "override must be scoped to selected video");

run('state.video="match.mp4";renderPlayerGroups()');
assert.match(el("#player-groups-list").innerHTML, /data-player-pick-key="g:g1"/);
assert(!el("#player-groups-list").innerHTML.includes("2/3 ID"),"Standalone ReID hint should not be shown in group card");
run('renderPlayers()');
assert.match(el("#raw-players").innerHTML, /data-player-pick-key="t:204"/);
assert.match(el("#raw-players").innerHTML, /Вера · 70% сходство/);
console.log("PASS: strongest ReID suggestion for groups and tracks, manual overrides retained");
