"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const vm=require("node:vm");
const root=path.resolve(__dirname,"../scripts/volleyball_ui/static");
const source=fs.readFileSync(path.join(root,"app.js"),"utf8");
const html=fs.readFileSync(path.join(root,"index.html"),"utf8");
assert(!source.includes("Проверить ID"),"obsolete Verify ID button must be removed");
assert(!source.includes("data-group-select"),"obsolete navigation handler must be removed");
assert(html.includes('id="workspace-loading-overlay"'));
const elements=new Map();
function el(key){
  if(!elements.has(key))elements.set(key,{
    hidden:true, textContent:"",innerHTML:"", value:"",
    setAttribute(name,value){this[name]=value},
    removeAttribute(name){delete this[name]},
    classList:{contains:()=>key==="#view-players"},
  });
  return elements.get(key);
}
const ctx=vm.createContext({
  document:{querySelector:el,querySelectorAll:()=>[],addEventListener(){}},
  window:{},console,setTimeout,clearTimeout,URLSearchParams,Date,URL
});
vm.runInContext(source,ctx,{filename:"app.js"});
const run=code=>vm.runInContext(code,ctx);
(async()=>{
  const end1=run('beginWorkspaceLoading("Первый запрос")');
  const end2=run('beginWorkspaceLoading("Второй запрос")');
  await new Promise(resolve=>setTimeout(resolve,120));
  assert.equal(el("#workspace-loading-overlay").hidden,false);
  assert.equal(el("#workspace-loading-text").textContent,"Второй запрос");
  end1();
  assert.equal(el("#workspace-loading-overlay").hidden,false,
    "an older request must not hide an active loading state");
  end2();
  assert.equal(el("#workspace-loading-overlay").hidden,true);
  const obsolete=run('beginWorkspaceLoading("Устаревший запрос")');
  const current=run('beginWorkspaceLoading("Текущий запрос")');
  await new Promise(resolve=>setTimeout(resolve,110));
  current();
  assert.equal(el("#workspace-loading-overlay").hidden,true,
    "a stale request must not block the completed current selection");
  obsolete();
  const fast=run('beginWorkspaceLoading("Быстро")');
  fast();
  await new Promise(resolve=>setTimeout(resolve,110));
  assert.equal(el("#workspace-loading-overlay").hidden,true,
    "fast switching must never flash the overlay");

  const requests=[];
  const apiMock=async (url)=>{
    requests.push(url);
    if(url.startsWith("/api/report?"))return {raw:[],groups:[],counts:{}};
    if(url.startsWith("/api/project-report?"))return {counts:{}};
    if(url.startsWith("/api/players?"))return {players:[]};
    if(url.startsWith("/api/players/actions?"))return {players:[]};
    if(url==="/api/players/library")return {players:[]};
    if(url==="/api/players/photos")return {counts:{}};
    throw Error("Unexpected API: "+url);
  };
  ctx.__mockApi=apiMock;
  run('api=__mockApi;renderReport=()=>{};renderPlayers=()=>{};populatePlayersSelect=()=>{};'+
      'analysisCacheId=()=>"cache";state.project="project";state.video="match.mp4";'+
      'state.analysisRunId="version";');
  await run('loadReport()');
  await run('loadReport()');
  assert.equal(requests.filter(x=>x==="/api/players/library").length,1);
  assert.equal(requests.filter(x=>x==="/api/players/photos").length,1);
  assert.equal(requests.filter(x=>x.startsWith("/api/players?")).length,2);
  el("#view-players").classList.contains=()=>false;
  await run('loadReport()');
  assert.equal(requests.filter(x=>x==="/api/players/library").length,1);
  assert.equal(requests.filter(x=>x==="/api/players/photos").length,1);
  assert.equal(requests.filter(x=>x.startsWith("/api/players?")).length,2);
  console.log("PASS: nonblocking loader lifecycle; player catalogs are lazy and cached");
})().catch(error=>{console.error(error);process.exitCode=1});
