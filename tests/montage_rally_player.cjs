"use strict";
const fs=require("node:fs");
const vm=require("node:vm");
const assert=require("node:assert/strict");
const path=require("node:path");
const source=fs.readFileSync(path.join(__dirname,"../scripts/volleyball_ui/static/montage-player.js"),"utf8");
const nodes=new Map(),handlers={};
function node(id){
  if(!nodes.has(id)){
    const callbacks={};
    nodes.set(id,{
      id,handlers:callbacks,style:{},value:"0",textContent:"",hidden:false,
      addEventListener(name,fn){callbacks[name]=fn;},
      setAttribute(name,value){this[name]=value;},
      closest(){return null;},
      classList:{contains:()=>id==="view-results"}
    });
  }
  return nodes.get(id);
}
const video=node("preview-player");
video.duration=3000;
video.readyState=1;
video.currentSrc="/api/video-media?project_id=one&video=match.mp4";
video.currentTime=0;
video.paused=true;
video.muted=false;
video.volume=1;
video.videoWidth=1920;
video.videoHeight=1080;
video.play=function(){this.paused=false;this.handlers.play?.();return Promise.resolve()};
video.pause=function(){this.paused=true;this.handlers.pause?.()};
const stage={clientWidth:1200,clientHeight:700};
const shell=node("montage-preview-shell");
shell.querySelector=()=>stage;
const document={
  activeElement:node("body"),fullscreenElement:null,
  getElementById:node,
  addEventListener(name,fn){handlers[name]=fn;}
};
const window={addEventListener(){}};
const context={document,window,console,requestAnimationFrame:()=>1,cancelAnimationFrame:()=>{}};
vm.runInNewContext(source,context,{filename:"montage-player.js"});
const player=window.VolleyballMontagePlayer;
player.init();
player.playRally(2367,2390);
assert.equal(video.currentTime,2367,"start at exact source position");
assert.equal(video.paused,false,"click starts playback");
assert.equal(node("montage-play-time").textContent,"00:00 / 00:23","use relative timeline");
video.currentTime=2372;
video.handlers.timeupdate();
assert.equal(node("montage-play-time").textContent,"00:05 / 00:23");
node("montage-seek").value="5000";
node("montage-seek").handlers.input();
assert.equal(video.currentTime,2378.5,"seek constrained to selected rally");
video.currentTime=2390;
video.handlers.timeupdate();
assert.equal(video.paused,true,"stop at exact rally end");
assert.equal(node("montage-play-time").textContent,"00:23 / 00:23");
let prevented=false;
handlers.keydown({code:"Space",repeat:false,altKey:false,ctrlKey:false,
  metaKey:false,isComposing:false,preventDefault(){prevented=true}});
assert.equal(prevented,true,"space must prevent page scroll");
assert.equal(video.paused,false,"space replays from rally start");
assert.equal(video.currentTime,2367);
handlers.keydown({code:"Space",repeat:false,altKey:false,ctrlKey:false,
  metaKey:false,isComposing:false,preventDefault(){}});
assert.equal(video.paused,true,"second space pauses video");
document.activeElement={closest:()=>({tagName:"INPUT"})};
handlers.keydown({code:"Space",repeat:false,altKey:false,ctrlKey:false,
  metaKey:false,isComposing:false,preventDefault(){throw Error("Typing interrupted")}});
assert.equal(video.paused,true,"space in a form must not control player");
document.activeElement=node("body");
player.setRallyRange();
assert.equal(node("montage-play-time").textContent,"39:27 / 50:00",
  "library playback restores unrestricted source timeline");
video.readyState=0;video.duration=0;video.currentTime=0;
player.playRally(105,118);
video.duration=3000;video.readyState=1;
video.handlers.loadedmetadata();
assert.equal(video.currentTime,105,"loading another source seeks to requested rally");
console.log("MONTAGE RALLY PLAYER / RELATIVE CLOCK / END / SPACE / SEEK: PASS");
