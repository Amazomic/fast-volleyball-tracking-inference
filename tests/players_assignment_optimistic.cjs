"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const vm=require("node:vm");
const path=require("node:path");
const source=fs.readFileSync(path.resolve(__dirname,"../scripts/volleyball_ui/static/app.js"),"utf8");

let hidden=0,restored=0;
const parent={offsetHeight:480,style:{},isConnected:true,
  insertBefore(card){restored++;card.parentNode=this}};
const card={parentNode:parent,nextSibling:null,
  remove(){hidden++;this.parentNode=null}};
const description={textContent:""},actionLabel={textContent:""};
const roster={dataset:{playerId:"p1"},
  querySelector(sel){
    if(sel===".player-slot-text small:first-child")return description;
    if(sel===".player-slot-actions")return actionLabel;
    return {textContent:""};
  },setAttribute(){},disabled:false};
const health={querySelectorAll:()=>Array.from({length:4},()=>({textContent:"0"}))};
const elements=new Map();
const create=()=>({value:"",textContent:"",innerHTML:"",disabled:false,checked:false,
  style:{},dataset:{},classList:{toggle(){},add(){},remove(){}},
  querySelectorAll(){return []},querySelector(){return null},setAttribute(){}});
function get(selector){
  if(selector==="#named-list")return {...(elements.get(selector)||create()),
    querySelectorAll:()=>[roster]};
  if(selector==="#players-health")return health;
  if(!elements.has(selector))elements.set(selector,create());
  return elements.get(selector);
}
const ctx=vm.createContext({window:{},document:{
  querySelector:get,querySelectorAll:()=>[],activeElement:null,addEventListener(){}
},console,setTimeout,clearTimeout,URLSearchParams});
vm.runInContext(source,ctx,{filename:"app.js"});
vm.runInContext([
  'analysisCacheId=()=>"cache";',
  'state.project="p";state.video="match.mp4";',
  'state.playerFilter="unassigned";',
  'state.players=[{id:"p1",name:"Анна"}];',
  'state.report={raw:[{player_track_id:101},{player_track_id:102}],groups:[]};',
  'state.playerGroupsKey=playerGroupsContext();',
  'state.playerGroups={status:"ready",groups:[]};'
].join("\n"),ctx);
ctx.card=card;
ctx.renderPlayers=()=>{throw Error("Full renderPlayers() must not run")};
ctx.loadReport=()=>{throw Error("Full loadReport() must not run")};
ctx.renderPlayerGroups=()=>{throw Error("Full renderPlayerGroups() must not run")};
let finish;
let calls=0;
ctx.api=async()=>{
  calls++;
  return new Promise((resolve,reject)=>{finish={resolve,reject}});
};
(async()=>{
  const task=vm.runInContext('assignSpecificTracks(["101"],"p1",{card})',ctx);
  assert.equal(hidden,1,"card disappears before the HTTP response");
  await Promise.resolve(); // Queued save starts in the next microtask.
  assert.equal(calls,1);
  assert.match(get("#players-action-feedback").textContent,/Сохраняем/);
  assert.equal(vm.runInContext('state.report.groups[0].track_ids[0]',ctx),"101");
  finish.resolve({groups:[]});
  await task;
  assert.equal(restored,0);
  assert.equal(vm.runInContext('state.reportDirty',ctx),true);
  assert.match(get("#players-action-feedback").textContent,/Сохранено/);
  assert.equal(description.textContent.includes("1 ID"),true);

  // Another unassigned card is still visible in the real grid.
  card.parentNode=parent;
  const task2=vm.runInContext('assignSpecificTracks(["102"],"p1",{card})',ctx);
  await Promise.resolve();
  assert.equal(calls,2);
  finish.reject(Error("test network failure"));
  await task2;
  assert.equal(restored,1,"failed assignment restores the original card");
  assert.equal(vm.runInContext('state.report.groups[0].track_ids.includes("102")',ctx),false);
  assert.match(get("#players-action-feedback").textContent,/test network failure/);
  console.log("PASS: optimistic assign, no full reload, server failure rollback");
})().catch(error=>{console.error(error);process.exitCode=1});
