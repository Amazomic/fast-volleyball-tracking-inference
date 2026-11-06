"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const vm=require("node:vm");

const source=fs.readFileSync(path.join(__dirname,
  "../scripts/volleyball_ui/static/app.js"),"utf8").replace(/\r\n/g,"\n");
const start=source.indexOf("function playerFilterSet(");
const end=source.indexOf("\n}\n",start)+2;
assert(start>=0&&end>start,"playerFilterSet must be present");
const state={
  playerFilter:"unassigned",
  playerGroups:{
    status:"ready",
    player_matches:{
      "1":{name:"Alice",similarity:0.74},
      "2":{name:"Bob",similarity:0.96},
      "3":{name:"Alice",similarity:0.87},
      "4":{name:"Bob",similarity:0.96},
      "7":{name:"Unknown",similarity:null},
    },
  },
};
const search={value:""};
const sort=vm.runInNewContext(source.slice(start,end)+"\nplayerFilterSet",
  {state,$:()=>search,Number,String,Map});
const rows=[
  {player_track_id:5,duration_sec:3,detected_frames:1000,total_actions:15},
  {player_track_id:1,duration_sec:8,detected_frames:500,total_actions:7},
  {player_track_id:3,duration_sec:12,detected_frames:120,total_actions:3},
  {player_track_id:2,duration_sec:4,detected_frames:80,total_actions:1},
  {player_track_id:4,duration_sec:6,detected_frames:150,total_actions:2},
  {player_track_id:7,duration_sec:4,detected_frames:400,total_actions:4},
  {player_track_id:6,duration_sec:2,detected_frames:50,total_actions:1},
];
const ids=result=>Array.from(result,item=>Number(item.player_track_id));
assert.deepEqual(ids(sort(rows,new Map())),[4,2,3,1,5,7,6],
  "largest ReID similarities first, missing similarities last, equal scores by frames");
assert.deepEqual(ids(sort(rows,new Map([["4","Bob"]]))),[2,3,1,5,7,6],
  "assigned tracks must still be excluded");
state.playerFilter="main";
assert.deepEqual(ids(sort(rows,new Map())),[3],
  "filters must remain intact");
state.playerFilter="all";
search.value="Bob";
assert.deepEqual(ids(sort(rows,new Map())),[4,2],
  "player-name search must remain intact");
search.value="";
state.playerGroups={status:"running",player_matches:{"2":{similarity:0.99}}};
assert.deepEqual(ids(sort(rows,new Map())),[5,1,7,4,3,2,6],
  "before ReID is ready, sorting falls back to detected frames");
console.log("PASS: manual player linking orders ReID scores descending");
