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
const source=fs.readFileSync(path.join(__dirname,"../scripts/volleyball_ui/static/quick-reel-ui.js"),"utf8");
const elements=new Map(),calls=[],pickers=[];
let counter=0;
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
  }else if(url==="/api/health")
    value={ok:true,api_version:12,reload_required:false};
  else if(url==="/api/quick-reel/start")
    value={reel_id:"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",status:"running"};
  else if(url.startsWith("/api/quick-reel/status?"))
    value={status:"running",stage:"Подготовка",progress:5,
      videos:[{video:"match.mp4",state:"queued",progress:0}]};
  else throw Error("Unexpected request: "+url);
  return {ok:true,status:200,json:async()=>value};
}
const saved=new Map();
vm.runInNewContext(source,{
  document:{readyState:"complete",getElementById:el,
    createElement:()=>makeElement("dynamic-"+(++counter))},
  window:browser,
  localStorage:{getItem:k=>saved.get(k)||null,setItem:(k,v)=>saved.set(k,v)},
  fetch:mockFetch,console,Math,
  URLSearchParams,
  setTimeout,clearTimeout,setInterval:()=>null,clearInterval:()=>{},
},{filename:"quick-reel-ui.js"});
(async()=>{
  await el("quick-reel-dropzone").click();
  assert.equal(pickers[0].startIn,"desktop","Picker opens at Desktop");
  assert.equal(pickers[0].multiple,true);
  assert.equal(el("quick-reel-video-count").textContent,"1");
  await el("quick-reel-music-dropzone").click();
  assert.equal(pickers[1].startIn,"desktop");
  assert.equal(pickers[1].multiple,false);
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
  assert.equal(saved.get("volley-quick-reel-id"),"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
  console.log("QUICK REEL: DESKTOP PICKER, MULTI-VIDEO DROP, MUSIC, NO BYTE UPLOAD: PASS");
})().catch(error=>{console.error(error);process.exitCode=1});
