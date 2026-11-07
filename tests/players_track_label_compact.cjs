"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const vm=require("node:vm");
const assets=path.resolve(__dirname,"../scripts/volleyball_ui/static");
const source=fs.readFileSync(path.join(assets,"app.js"),"utf8");
const elements=new Map();
function element(selector){
  if(!elements.has(selector))elements.set(selector,{
    textContent:"",innerHTML:"",value:"",disabled:false,hidden:false,
    classList:{toggle(){},add(){},remove(){}},
    querySelectorAll(){return []},
    setAttribute(){},
    insertAdjacentHTML(_where,html){this.innerHTML+=html}
  });
  return elements.get(selector);
}
const ctx=vm.createContext({
  window:{},document:{querySelector:element,querySelectorAll:()=>[],activeElement:null,addEventListener(){}},
  URLSearchParams,console,setTimeout,clearTimeout
});
vm.runInContext(source,ctx,{filename:"app.js"});
const run=code=>vm.runInContext(code,ctx);
const fromA=JSON.stringify(["a-game.mp4","7"]);
const fromB=JSON.stringify(["b-game.mp4","7"]);
ctx.fromA=fromA;ctx.fromB=fromB;
run([
  'analysisCacheId=()=>"gpu";',
  'state.project="project";state.video="a-game.mp4";',
  'state.projectPlayerScope={key:"project",sources:[{video:"a-game.mp4"},{video:"b-game.mp4"}],',
  'context(id){',
  '  if(id===fromA)return {video:"a-game.mp4",id:"7",cache:"GPU-a",project:"project"};',
  '  if(id===fromB)return {video:"b-game.mp4",id:"7",cache:"GPU-b",project:"project"};',
  '  return null;',
  '},photoUrl(id,sample=0){',
  '  const c=this.context(id);return "/api/player-photo?video="+encodeURIComponent(c.video)+"&id="+c.id+"&sample="+sample;',
  '}};',
  'state.report={groups:[],raw:[{player_track_id:fromA},{player_track_id:fromB}]};',
  'state.players=[{id:"one",name:"Ilnur"}];',
  'state.playerGroupsKey=playerGroupsContext();',
  'state.playerGroups={status:"ready",threshold_percent:80,groups:[],player_matches:{}};'
].join("\n"));
assert.notEqual(fromA,fromB,"Technical identifiers from two videos remain distinct");
assert.equal(run('playerTrackLabel(fromA)'),"7");
assert.equal(run('playerTrackLabel(fromB)'),"7");
assert.equal(run('playerTrackLabel("123")'),"123");
assert.equal(run('playerGroupLabel(\'["a-game.mp4","group-12"]\')'),"group-12");
assert.equal(run('playerGroupLabel("none-"+fromA)'),"none-7");
assert.equal(run('playerGroupLabel("candidate-"+fromB)'),"candidate-7");
assert.equal(run('playerGroupLabel("group-x")'),"group-x");
run('state.playerGroups.groups=[{id:"g1",track_ids:[fromA,fromB],source:"automatic",similarity:.93}];renderPlayerGroups()');
const markup=element("#player-groups-list").innerHTML;
assert.match(markup,/<strong class="player-appearance-main-id">Track ID #7<\/strong>/);
assert(!markup.includes('class="player-appearance-main-id">Track ID #7 · '),
  "Video names must not appear in displayed track labels");
assert(markup.includes('data-group-link="g1"'),"Group binding must retain opaque ID");
assert(markup.includes("a-game.mp4"),"Photo source must still retain source video internally");
console.log("PASS: UI displays only numeric Track ID / short group ID; project-scoped identities preserved");
