"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const vm=require("node:vm");
const code=fs.readFileSync(path.resolve(__dirname,"../scripts/volleyball_ui/static/app.js"),"utf8");
const elements=new Map();
function el(sel){
  if(!elements.has(sel))elements.set(sel,{
    textContent:"",innerHTML:"",value:"",disabled:false,
    classList:{toggle(){},add(){},remove(){}},
    querySelectorAll(){return []},setAttribute(){}
  });
  return elements.get(sel);
}
const ctx=vm.createContext({window:{},document:{querySelector:el,querySelectorAll:()=>[],
  activeElement:null,addEventListener(){}},
  console,clearTimeout,setTimeout,URLSearchParams});
vm.runInContext(code,ctx,{filename:"app.js"});
vm.runInContext([
  'analysisCacheId=()=>"gpu";state.project="p";state.video="game.mp4";',
  'state.players=[];state.report={groups:[],raw:[]};',
  'state.playerGroupsKey=playerGroupsContext();',
  'state.playerGroups={status:"running",threshold_percent:85,groups:[],progress:30};',
  'state.playerReidThresholdPending={context:playerGroupsContext(),value:95};',
  'renderPlayerGroups();'
].join("\n"),ctx);
assert.equal(el("#player-reid-threshold").disabled,false,
  "range should remain adjustable during an ongoing computation");
assert.equal(el("#player-reid-threshold-number").disabled,false,
  "numeric value should remain adjustable during an ongoing computation");
assert.equal(el("#player-reid-threshold").value,"95");
assert.equal(el("#player-reid-threshold-number").value,"95");
assert.match(el("#player-groups-status").textContent,/ожидает завершения/);
let committed=null;
ctx.api=async()=>({status:"ready",threshold_percent:85,groups:[],unmatched_ids:[]});
ctx.submit=async value=>{committed=value};
vm.runInContext('commitPlayerReidThreshold=submit',ctx);
(async()=>{
  await vm.runInContext("loadPlayerGroups()",ctx);
  assert.equal(committed,95);
  assert.equal(vm.runInContext("state.playerReidThresholdPending",ctx),null);
  console.log("PASS: threshold can be selected during ReID processing and applied at completion");
})().catch(error=>{console.error(error);process.exitCode=1});
