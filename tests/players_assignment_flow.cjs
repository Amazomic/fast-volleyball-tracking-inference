"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const vm=require("node:vm");

async function main(){
  const staticDir=path.resolve(__dirname,"../scripts/volleyball_ui/static");
  const html=fs.readFileSync(path.join(staticDir,"index.html"),"utf8");
  const css=fs.readFileSync(path.join(staticDir,"players-workspace.css"),"utf8");
  assert(html.includes('class="players-stack"'));
  assert(html.indexOf('id="named-list"')<html.indexOf('id="raw-players"'));
  assert(html.includes('id="library-player-select"'));
  assert(!html.includes('id="assignment-player-select"'));
  assert(css.includes(".players-stack .named-list{display:flex;flex-wrap:nowrap"));
  const elements=new Map();
  function element(selector){
    if(!elements.has(selector)){
      elements.set(selector,{
        value:"",innerHTML:"",textContent:"",checked:false,disabled:false,
        hidden:false,indeterminate:false,
        classList:{toggle(){},add(){},remove(){}},
        querySelectorAll(){return []},
        setAttribute(){}
      });
    }
    return elements.get(selector);
  }
  const doc={
    querySelector:element,
    querySelectorAll(){return []},
    addEventListener(){},
    activeElement:null
  };
  const ctx=vm.createContext({
    document:doc,window:{},console,setTimeout,clearTimeout,URLSearchParams,
  });
  vm.runInContext(fs.readFileSync(path.join(staticDir,"app.js"),"utf8"),ctx,{filename:"app.js"});
  vm.runInContext([
    'state.project="project-1";',
    'state.video="match.mp4";',
    'state.players=[{id:"player-1",name:"Мария"}];',
    'state.libraryPlayers=[{id:"player-1",name:"Мария"}];',
    'state.report={raw:[',
    '{player_track_id:101,duration_sec:8,detected_frames:200},',
    '{player_track_id:102,duration_sec:6,detected_frames:180}',
    '],groups:[]};',
    'state.selected.add("101");',
    'state.selected.add("102");',
    'state.limit=30;',
    'state.playerFilter="unassigned";',
    'renderPlayers();'
  ].join("\n"),ctx);
  assert(element("#raw-players").innerHTML.includes("#101"));
  assert(element("#raw-players").innerHTML.includes("#102"));
  assert(element("#named-list").innerHTML.includes("is-assign-target"));
  assert(element("#named-list").innerHTML.includes("Привязать"));
  vm.runInContext([
    'globalThis.saved=[];',
    'api=async function(endpoint,payload){',
    '  saved.push({endpoint,payload});',
    '  return {ok:true};',
    '};',
    'loadReport=async function(){',
    '  state.report.groups=[{id:"player-1",name:"Мария",track_ids:["101","102"]}];',
    '  state.selected.clear();',
    '  renderPlayers();',
    '};'
  ].join("\n"),ctx);
  await vm.runInContext('assignSelected(false,"player-1")',ctx);
  const saved=vm.runInContext("saved",ctx);
  assert.equal(saved.length,1);
  assert.equal(saved[0].endpoint,"/api/assign");
  assert.deepEqual([...saved[0].payload.ids],["101","102"]);
  assert.equal(saved[0].payload.project_id,"project-1");
  assert.equal(saved[0].payload.video,"match.mp4");
  assert.equal(element("#raw-players").innerHTML.includes("#101"),false);
  assert.equal(element("#raw-players").innerHTML.includes("#102"),false);
  assert.equal(vm.runInContext("state.selected.size",ctx),0);
  assert(element("#players-action-feedback").textContent.includes("2 ID привязано"));
  console.log("PASS: horizontal roster, batch assignment, disappearance from unrecognized list");
}

main().catch(error=>{console.error(error);process.exitCode=1});
