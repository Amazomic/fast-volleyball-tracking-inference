"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const vm=require("node:vm");
const path=require("node:path");
const src=fs.readFileSync(path.resolve(__dirname,"../scripts/volleyball_ui/static/app.js"),"utf8");
const els=new Map();
function el(sel){
  if(!els.has(sel))els.set(sel,{textContent:"",innerHTML:"",value:"",disabled:false,
    classList:{toggle(){},add(){},remove(){}},querySelector(){return null},
    querySelectorAll(){return []},setAttribute(){}});
  return els.get(sel);
}
const ctx=vm.createContext({document:{querySelector:el,querySelectorAll:()=>[],
  addEventListener(){},activeElement:null},window:{},console,
  URLSearchParams,setTimeout,clearTimeout});
vm.runInContext(src,ctx);
vm.runInContext([
  'analysisCacheId=()=>"gpu";state.project="p";state.video="game.mp4";',
  'state.players=[{id:"p1",name:"А"},{id:"p2",name:"Б"}];',
  'state.report={raw:[{player_track_id:"1"},{player_track_id:"2"}],groups:[]};',
  'state.playerFilter="unassigned";state.playerGroupsKey=playerGroupsContext();'
].join("\n"),ctx);
ctx.loadReport=()=>{throw Error("Unexpected report reload")};
ctx.renderPlayers=()=>{throw Error("Unexpected full render")};
function makeCard(){
  const parent={style:{},offsetHeight:280,isConnected:true,
    insertBefore(card){card.parentNode=this}};
  return {parentNode:parent,nextSibling:null,removed:false,
    remove(){this.removed=true;this.parentNode=null}};
}
const first=makeCard(),second=makeCard();
ctx.first=first;ctx.second=second;
const deferred=[];
ctx.api=async(endpoint,payload)=>{
  assert.equal(endpoint,"/api/assign");
  return new Promise((resolve,reject)=>deferred.push({resolve,reject,payload}));
};
(async()=>{
  const one=vm.runInContext('assignSpecificTracks(["1"],"p1",{card:first})',ctx);
  const two=vm.runInContext('assignSpecificTracks(["2"],"p2",{card:second})',ctx);
  assert(first.removed&&second.removed,"both cards disappear immediately");
  assert.equal(vm.runInContext("state.report.groups.length",ctx),2);
  await Promise.resolve();
  assert.equal(deferred.length,1,"disk requests are serialized");
  deferred[0].resolve({});
  await one;
  for(let n=0;n<6&&deferred.length<2;n++)await Promise.resolve();
  assert.equal(deferred.length,2);
  deferred[1].reject(Error("second save failed"));
  await two;
  assert.equal(vm.runInContext("state.report.groups.length",ctx),1);
  assert.equal(vm.runInContext("state.report.groups[0].id",ctx),"p1");
  assert.equal(vm.runInContext("state.report.groups[0].track_ids[0]",ctx),"1");
  assert.equal(second.parentNode===null,false,"failed item is restored");
  assert.equal(first.parentNode,null,"saved item remains removed");
  console.log("PASS: rapid links queue without UI locking, isolated rollback");
})().catch(error=>{console.error(error);process.exitCode=1});
