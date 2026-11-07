"use strict";
/* Drag and drop and desktop-first picker resolve sources without byte uploads. */
const fs=require("node:fs"),vm=require("node:vm"),assert=require("node:assert/strict");
const path=require("node:path");
const html=fs.readFileSync(path.join(__dirname,"../scripts/volleyball_ui/static/index.html"),"utf8");
assert.match(html,/<button[^>]*id="quick-reel-dropzone"[^>]*>/);
assert.match(html,/<button[^>]*id="quick-reel-music-dropzone"[^>]*>/);
assert.ok(!html.includes('id="quick-reel-existing"'));
assert.ok(!html.includes('id="quick-reel-video-paths"'));
assert.ok(!html.includes('id="quick-reel-audio-path"'));
assert.match(html,/id="quick-reel-header-cancel"/);
assert.match(html,/<a id="quick-reel-header-download"[^>]* hidden>Скачать MP4<\/a>/);
assert.match(html,/id="quick-reel-header-close"/);
assert.equal((html.match(/id="quick-reel-header-download"/g)||[]).length,1);
assert.ok(!html.includes('id="quick-reel-header-result"'));
const source=fs.readFileSync(path.join(__dirname,"../scripts/volleyball_ui/static/quick-reel-ui.js"),"utf8");
const elements=new Map(),calls=[],pickers=[];
let counter=0,completed=false,historyDeleted=false;
function makeElement(id){
  const handlers={};
  return {id,handlers,style:{},children:[],value:"",textContent:"",
    hidden:false,disabled:false,open:false,
    classList:{toggle(){},add(){},remove(){}},
    addEventListener(name,fn){handlers[name]=fn},
    replaceChildren(...children){this.children=children},
    append(...children){this.children.push(...children)},
    showModal(){this.open=true},
    close(){this.open=false},
    getAttribute(){return ""},
    removeAttribute(name){delete this[name]},
    setAttribute(name,value){this[name]=String(value)},
    click(){return handlers.click?.()},
    contains(){return false},
  };
}
function el(id){if(!elements.has(id))elements.set(id,makeElement(id));return elements.get(id);}
const video={name:"match.mp4",size:9550000000,lastModified:1793740000000};
const second={name:"training.mov",size:25000000,lastModified:1793740001000};
const music={name:"song.mp3",size:4500000,lastModified:1793740002000};
const choices=[[video],[music]];
const browser={
  location:{assign(){}},
  confirm(){return true},
  async showOpenFilePicker(options){
    pickers.push(options);
    return choices.shift().map(file=>({getFile:async()=>file}));
  }
};
async function mockFetch(url,options={}){
  const body=options.body?JSON.parse(options.body):null;
  calls.push({url,method:options.method||"GET",body});
  let value;
  if(url==="/api/quick-reel/resolve-files"){
    if(body.kind==="video")value={videos:body.files.map(file=>({
      id:(file.name==="match.mp4"?"a":"b").repeat(32),name:file.name,source_path:"/mnt/c/Users/test/Desktop/"+file.name
    }))};
    else value={audio:{name:"song.mp3",path:"/mnt/c/Users/test/Desktop/song.mp3",duration:45}};
  }else if(url==="/api/quick-reel/latest")
    value={reel_id:null};
  else if(url==="/api/quick-reel/history")
    value={results:historyDeleted?[]:[{
      id:"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",created_at:1793740000,
      duration:25,videos:["older-match.mp4"],
      url:"/api/quick-reel/media?id=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      download_url:"/api/quick-reel/download?id=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }]};
  else if(url==="/api/quick-reel/delete"){
    assert.equal(body.id,"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    historyDeleted=true;
    value={deleted:body.id};
  }
  else if(url==="/api/health")
    value={ok:true,api_version:15,reload_required:false};
  else if(url==="/api/quick-reel/start")
    value={reel_id:"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",status:"running"};
  else if(url==="/api/quick-reel/cancel")
    value={status:"cancelled"};
  else if(url.startsWith("/api/quick-reel/status?"))
    value=completed
      ?{status:"complete",stage:"Рилс готов",progress:100,available:true,videos:[]}
      :{status:"running",stage:"Подготовка",progress:5,
        videos:[{video:"match.mp4",state:"queued",progress:0}]};
  else if(url.startsWith("/api/quick-reel/result?"))
    value={available:true,duration:30,music_duration:45,
      url:"/api/quick-reel/video?id=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      download_url:"/api/quick-reel/download?id=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"};
  else throw Error("Unexpected request: "+url);
  return {ok:true,status:200,json:async()=>value};
}
const saved=new Map();
vm.runInNewContext(source,{
  document:{readyState:"complete",documentElement:{lang:"ru"},getElementById:el,
    createElement:()=>makeElement("dynamic-"+(++counter))},
  window:browser,
  localStorage:{getItem:k=>saved.get(k)||null,setItem:(k,v)=>saved.set(k,v),
    removeItem:k=>saved.delete(k)},
  fetch:mockFetch,console,Math,
  URLSearchParams,
  setTimeout,clearTimeout,setInterval:()=>null,clearInterval:()=>{},
},{filename:"quick-reel-ui.js"});
(async()=>{
  assert.equal(el("quick-reel-activity").hidden,true,
    "No completed notification at page load");
  assert.equal(calls.filter(row=>row.url==="/api/quick-reel/history").length,0,
    "History loads on opening the creation dialog");
  el("quick-reel-open").click();
  await new Promise(resolve=>setImmediate(resolve));
  const oldRow=el("quick-reel-history-list").children[0];
  assert(oldRow.children[0].href.includes("bbbbbbbbbbbb"),
    "The completed archive is read when create dialog opens");
  assert.equal(oldRow.children[2].textContent,"×");
  assert.match(oldRow.children[2]["aria-label"],/Удалить рилс/);
  await oldRow.children[2].click();
  assert.equal(el("quick-reel-history-list").children[0].textContent,
    "Готовых рилсов пока нет.","Deleting refreshes the history list");
  assert.equal(calls.filter(row=>row.url==="/api/quick-reel/delete").length,1);
  await el("quick-reel-dropzone").click();
  assert.equal(pickers[0].startIn,"desktop","Picker opens at Desktop");
  assert.equal(pickers[0].multiple,true);
  assert.equal(el("quick-reel-video-count").textContent,"1");
  await new Promise(resolve=>setImmediate(resolve));
  assert.notEqual(el("quick-reel-open").textContent,"✦ Рилс готов");
  assert.equal(el("quick-reel-history-list").children.length,1);
  await el("quick-reel-music-dropzone").click();
  assert.equal(pickers[1].startIn,"desktop");
  assert.equal(pickers[1].multiple,false);
  browser.VolleyballQuickReelUI.setTask({status:"running",kind:"analyze"});
  assert.equal(el("quick-reel-submit").disabled,true);
  browser.VolleyballQuickReelUI.setTask({status:"complete"});
  assert.equal(el("quick-reel-submit").disabled,false);
  assert.equal(el("quick-reel-duration").textContent,"00:45");
  await el("quick-reel-dropzone").handlers.drop({
    preventDefault(){},dataTransfer:{files:[second]},
  });
  assert.equal(el("quick-reel-video-count").textContent,"2","Drop adds source");
  const uploads=calls.filter(row=>/video-upload|chunk|audio\/start/.test(row.url));
  assert.equal(uploads.length,0,"No file content was uploaded");
  const resolution=calls.filter(row=>row.url==="/api/quick-reel/resolve-files");
  assert.equal(resolution.length,3);
  assert.deepEqual(Object.keys(resolution[0].body.files[0]).sort(),
    ["last_modified","name","size"]);
  await el("quick-reel-submit").click();
  assert.equal(el("quick-reel-activity").hidden,false);
  assert.equal(saved.size,0,"Latest Quick Reel is stored only on the server");
  assert.equal(el("quick-reel-header-cancel").hidden,false);
  assert.equal(el("quick-reel-header-download").hidden,true);
  el("quick-reel-header-close").click();
  assert.equal(el("quick-reel-activity").hidden,true,"Close hides only the progress block");
  assert.equal(saved.get("volley-quick-reel-dismissed-id"),
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "Close persists dismissal across reloads");
  assert.ok(!calls.some(row=>row.url==="/api/quick-reel/cancel"),
    "Closing the block must not cancel the running job");
  await el("quick-reel-header-cancel").click();
  const cancellation=calls.find(row=>row.url==="/api/quick-reel/cancel");
  assert.equal(cancellation.body.id,"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
  assert.ok(!calls.some(row=>row.url==="/api/cancel"),"Do not cancel other jobs");
  completed=true;
  el("quick-reel-header-open").click();
  await new Promise(resolve=>setImmediate(resolve));
  assert.equal(el("quick-reel-header-download").hidden,false,
    "Download appears in header once the reel is ready");
  assert.equal(el("quick-reel-open").textContent,"✦ Рилс",
    "Completing a reel is a notification, not a different create button");
  assert.match(el("quick-reel-header-download").href,/\/api\/quick-reel\/download\?id=/);
  assert.equal(el("quick-reel-activity").hidden,true,
    "Previously dismissed task remains hidden when status changes");
  console.log("QUICK REEL: PICKER, DOWNLOAD HEADER, DISMISS WITHOUT CANCEL, NO BYTE UPLOAD: PASS");
})().catch(error=>{console.error(error);process.exitCode=1});
