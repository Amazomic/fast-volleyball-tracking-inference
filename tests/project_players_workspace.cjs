const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const tracks=require('../scripts/volleyball_ui/static/project-player-tracks.js');
const elements=new Map();
function element(key){
  if(!elements.has(key))elements.set(key,{value:'',dataset:{},classList:{contains:()=>key==='#view-players'}});
  return elements.get(key);
}
const ctx=vm.createContext({document:{querySelector:element,querySelectorAll:()=>[],addEventListener(){}},window:{VolleyballProjectPlayerTracks:tracks},console,setTimeout,clearTimeout,URLSearchParams});
vm.runInContext(fs.readFileSync('scripts/volleyball_ui/static/app.js','utf8'),ctx);
const calls=[];
ctx.apiRequest=async(url)=>{
  calls.push(url);
  if(url.startsWith('/api/report?'))return {raw:[{player_track_id:7}],groups:[],replay_filtered_tracks:true};
  return {};
};
ctx.api=async(url)=>{
  if(url.startsWith('/api/players?'))return {players:[{id:'p1',name:'Player'}]};
  return ctx.apiRequest(url);
};
ctx.sharedPlayerCatalogs=async()=>({libraryPlayers:[],photoCounts:{}});
for(const name of ['renderReport','renderPlayers','populatePlayersSelect'])ctx[name]=()=>{};
vm.runInContext('state.project="p";state.video="a.mp4";state.projects=[{id:"p",videos:["a.mp4","b.mp4"]}];state.analysisRuns=[{id:"cpu",videos:{"a.mp4":"cpu-a","b.mp4":"cpu-b"}}];state.analysisRunId="cpu";',ctx);
(async()=>{
  await vm.runInContext('loadReport()',ctx);
  assert.equal(vm.runInContext('playerReport().raw.length',ctx),2);
  assert.equal(vm.runInContext('state.report.raw.length',ctx),1);
  const first=vm.runInContext('JSON.stringify(playerReport().raw.map(row=>row.player_track_id))',ctx);
  const key=vm.runInContext('playerGroupsContext()',ctx);
  vm.runInContext('state.video="b.mp4";',ctx);
  await vm.runInContext('loadReport()',ctx);
  assert.equal(vm.runInContext('JSON.stringify(playerReport().raw.map(row=>row.player_track_id))',ctx),first);
  assert.equal(vm.runInContext('playerGroupsContext()',ctx),key);
  assert.ok(calls.some(url=>url.includes('video=b.mp4')&&url.includes('cache_id=cpu-b')));
  console.log('PASS: project tracks stay complete across video changes; preview report remains video-scoped');
})().catch(error=>{console.error(error);process.exitCode=1;});
