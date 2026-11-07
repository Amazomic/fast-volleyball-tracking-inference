const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const elements=new Map();
function element(key){
  if(!elements.has(key)){
    let html='';
    elements.set(key,{writes:0,value:'',textContent:'',hidden:false,dataset:{},style:{},scrollTop:0,
      get innerHTML(){return html},set innerHTML(value){html=value;this.writes++;this.scrollTop=0},
      querySelectorAll(){return []},setAttribute(){},classList:{toggle(){},add(){},remove(){}}});
  }
  return elements.get(key);
}
const ctx=vm.createContext({document:{querySelector:element,querySelectorAll(){return []},addEventListener(){}},window:{},console,setTimeout,clearTimeout,URLSearchParams});
vm.runInContext(fs.readFileSync('scripts/volleyball_ui/static/app.js','utf8'),ctx);
vm.runInContext('state.project="project";state.video="match";state.players=[{id:"p",name:"Player"}];state.libraryPlayers=state.players;state.report={groups:[{id:"p",track_ids:[1]}],raw:[{player_track_id:1,duration_sec:5}]};state.activePlayerId="p";',ctx);
vm.runInContext('renderPlayers()',ctx);
const roster=element('#named-list'),tracks=element('#raw-players'),details=element('#player-linked-tracks');
const writes=[roster.writes,tracks.writes,details.writes];
roster.scrollTop=100;tracks.scrollTop=200;details.scrollTop=300;
vm.runInContext('renderPlayers()',ctx);
assert.deepEqual([roster.writes,tracks.writes,details.writes],writes);
assert.deepEqual([roster.scrollTop,tracks.scrollTop,details.scrollTop],[100,200,300]);
vm.runInContext('state.photoCounts.p=8;renderPlayers()',ctx);
assert.match(roster.innerHTML,/8 фото/);
assert.equal(tracks.writes,writes[1]);
for(const [render,selector] of [['renderReport','#report-people'],['renderRenders','#all-renders']]){
  vm.runInContext(render+'()',ctx);
  const count=element(selector).writes;
  vm.runInContext(render+'()',ctx);
  assert.equal(element(selector).writes,count);
}
let detailCalls=0;
ctx.renderPlayerDetails=()=>{detailCalls++};
ctx.api=async()=>({photos:[]});
vm.runInContext('setPlayerGallery("tracks")',ctx);
assert.equal(detailCalls,1);
(async()=>{
  let calls=0,finish;
  ctx.api=()=>{calls++;return new Promise(resolve=>{finish=resolve})};
  const loading=vm.runInContext('renderPlayerAlbum()',ctx);
  await vm.runInContext('renderPlayerAlbum()',ctx);
  assert.equal(calls,1);
  finish({photos:[],count:0});
  await loading;
  ctx.api=async()=>({results:[]});
  await vm.runInContext('loadBenchmarkResults()',ctx);
  const benchmarkWrites=element('#benchmark-results').writes;
  await vm.runInContext('loadBenchmarkResults()',ctx);
  assert.equal(element('#benchmark-results').writes,benchmarkWrites);
  console.log('PASS: unchanged lists retain DOM and scroll; gallery switch renders once; album request deduplicated');
})().catch(error=>{console.error(error);process.exitCode=1});
