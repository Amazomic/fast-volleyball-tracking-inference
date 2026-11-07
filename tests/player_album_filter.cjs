const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const elements=new Map();
function element(key){
  if(!elements.has(key))elements.set(key,{innerHTML:'',textContent:'',hidden:false,disabled:false,value:'',dataset:{},style:{},setAttribute(k,v){this[k]=v},classList:{toggle(){}},querySelectorAll(){return []}});
  return elements.get(key);
}
const ctx=vm.createContext({document:{querySelector:element,querySelectorAll(){return []},addEventListener(){}},window:{},console,URLSearchParams,setTimeout,clearTimeout});
vm.runInContext(fs.readFileSync('scripts/volleyball_ui/static/app.js','utf8'),ctx);
vm.runInContext('state.project="project";state.video="match";state.players=[{id:"p",name:"Player"}];state.activePlayerId="p";analysisCacheId=()=>"cache";',ctx);
(async()=>{
  const urls=[];
  ctx.api=async url=>{urls.push(url);return {photos:[{filename:'one.jpg',track_id:1}],count:1,total_count:3,hidden_count:2}};
  await vm.runInContext('togglePlayerAlbumFilter()',ctx);
  assert.match(urls[0],/distinct=1/);
  assert.equal(element('#filter-player-album')['aria-pressed'],'true');
  assert.match(element('#player-album-filter-status').textContent,/2/);
  await vm.runInContext('togglePlayerAlbumFilter()',ctx);
  assert.doesNotMatch(urls[1],/distinct=1/);
  assert.equal(element('#filter-player-album')['aria-pressed'],'false');
  await vm.runInContext('togglePlayerAlbumFilter()',ctx);
  vm.runInContext('state.activePlayerId="other";state.players.push({id:"other",name:"Other"})',ctx);
  await vm.runInContext('renderPlayerAlbum()',ctx);
  assert.doesNotMatch(urls.at(-1),/distinct=1/);
  let finish;
  ctx.api=()=>new Promise(resolve=>{finish=resolve});
  const stale=vm.runInContext('renderPlayerAlbum(true)',ctx);
  vm.runInContext('state.activePlayerId="p"',ctx);
  ctx.api=async()=>({photos:[],count:0});
  await vm.runInContext('renderPlayerAlbum(true)',ctx);
  finish({photos:[{filename:'stale.jpg'}],count:1});
  await stale;
  assert.doesNotMatch(element('#player-album-gallery').innerHTML,/stale/);
  console.log('PASS: reversible photo filtering, player switch and stale response protection');
})().catch(error=>{console.error(error);process.exitCode=1});
