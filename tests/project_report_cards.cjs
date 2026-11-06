"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const vm=require("node:vm");
const elements=new Map();
function element(key){
  if(!elements.has(key))elements.set(key,{innerHTML:"",textContent:"",hidden:false,
    classList:{toggle(){}},querySelectorAll(){return []}});
  return elements.get(key);
}
const context=vm.createContext({document:{querySelector:element,querySelectorAll(){return []},
  addEventListener(){}},window:{},console,URLSearchParams,setTimeout,clearTimeout});
vm.runInContext(fs.readFileSync("scripts/volleyball_ui/static/app.js","utf8"),context);
vm.runInContext(`state.aggregate={analyzed:true,total_touches:60,recognized_touches:30,
  analyzed_videos:2,total_videos:2,rallies:8,median_touches:10,unassigned_touches:0,
  counts:{attack:20,block:10},groups:Array.from({length:6},(_,i)=>({
    id:"p"+i,name:i===0?"<Игрок>":"Игрок "+i,touches:i===0?0:12,
    recognized_touches:i===0?0:6,activity:i===0?0:1.2,
    photo:i===1?"sample.jpg":null,actions:{attack:i===0?0:4,block:i===0?0:2}}))};
  renderReport();`,context);
const cards=element("#report-people").innerHTML;
assert.equal((cards.match(/class="report-player-card"/g)||[]).length,6);
assert.match(cards,/&lt;Игрок&gt;/);
assert.match(cards,/1,20/);
assert.match(cards,/0,00/);
assert.match(cards,/Всего касаний/);
assert.match(cards,/Верхние передачи/);
assert.match(cards,/album-photo\?player_id=p1/);
assert.match(element("#report-metrics").innerHTML,/60/);
assert.match(element("#report-metrics").innerHTML,/30/);
vm.runInContext('state.aggregate={analyzed:false,groups:[{id:"p",name:"Игрок",touches:0,activity:null,actions:{}}]};renderReport()',context);
assert.match(element("#report-people").innerHTML,/—/);
assert.doesNotMatch(element("#report-people").innerHTML,/NaN|Infinity/);
console.log("Project report cards: passed");
