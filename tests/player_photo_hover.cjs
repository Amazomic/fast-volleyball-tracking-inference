"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const vm=require("node:vm");

class Node{
  constructor(tag="div"){
    this.tagName=tag.toUpperCase();
    this.children=[];this.dataset={};this.style={};
    this.events=new Map();this.parentNode=null;
    this.src="";this.title="";
    this.className="";
    const names=new Set();
    this.classList={
      toggle(name,enabled){
        if(enabled)names.add(name);else names.delete(name);
        return !!enabled;
      },
      contains(name){return names.has(name);}
    };
  }
  addEventListener(type,handler){
    const list=this.events.get(type)||[];
    list.push(handler);this.events.set(type,list);
  }
  fire(type,options={}){
    const event={
      type,target:options.target||this,key:options.key,
      defaultPrevented:false,preventDefault(){this.defaultPrevented=true;}
    };
    for(const fn of this.events.get(type)||[])fn(event);
    return event;
  }
  appendChild(node){node.parentNode=this;this.children.push(node);return node;}
  contains(other){for(let node=other;node;node=node.parentNode)if(node===this)return true;return false;}
  querySelector(selector){return selector==="img"?this.children.find(item=>item.tagName==="IMG")||null:null;}
  closest(selector){
    for(let item=this;item;item=item.parentNode){
      if(selector.includes(".photo-peek")&&item.className==="photo-peek")return item;
      if(selector.includes("[data-photo-track-id]")&&item.dataset.photoTrackId)return item;
    }
    return null;
  }
}
const sampleOf=url=>new URL(String(url),"http://localhost").searchParams.get("sample");
const flush=()=>new Promise(resolve=>setImmediate(resolve));
async function test(){
  const dir=path.resolve(__dirname,"../scripts/volleyball_ui/static");
  const html=fs.readFileSync(path.join(dir,"index.html"),"utf8");
  const css=fs.readFileSync(path.join(dir,"players-workspace.css"),"utf8");
  assert.match(html,/src="\/player-photo-hover\.js\?v=\d+"/);
  assert(!css.includes(".player-track-hover-preview"),"Hover gallery must not return");
  const root=new Node();
  const button=new Node("button");
  button.className="photo-peek";button.dataset.id="101";
  const photo=button.appendChild(new Node("img"));
  photo.src="/api/player-photo?sample=0";
  root.appendChild(button);
  const secondary=new Node("img");
  secondary.dataset.photoTrackId='["match B.mp4","103"]';
  secondary.src="/api/player-photo?sample=0";
  root.appendChild(secondary);
  const link=new Node("a");
  link.dataset.photoTrackId="104";
  const linkImage=link.appendChild(new Node("img"));
  linkImage.src="/api/player-photo?sample=0";
  root.appendChild(link);

  const context={project:"project-1",video:"match 1.mp4",cache:"base123.cpu123"};
  let requests=0,pendingResolver=null;
  let photoLoads=0,holdSample=null,releaseHeld=null;
  const failedSamples=new Set();
  class FakeImage{
    set src(url){
      this._src=url;
      photoLoads++;
      const sample=Number(sampleOf(url));
      const done=()=>queueMicrotask(()=>failedSamples.has(sample)
        ?this.onerror?.():this.onload?.());
      if(holdSample===sample){
        holdSample=null;releaseHeld=done;
      }else done();
    }
    get src(){return this._src;}
  }
  const samples=Array.from({length:6},(_,i)=>({
    index:i,frame:i*55+25,time_sec:i*2+.4
  }));
  const fetch=async url=>{
    requests++;
    const parsed=new URL(url,"http://localhost");
    assert.equal(parsed.pathname,"/api/player-photos");
    assert.equal(parsed.searchParams.get("project_id"),context.project);
    assert.equal(parsed.searchParams.get("cache_id"),"base123");
    if(parsed.searchParams.get("id")==="103"){
      assert.equal(parsed.searchParams.get("video"),"match B.mp4");
      return new Promise(resolve=>{pendingResolver=resolve;});
    }
    return {ok:true,json:async()=>({samples})};
  };
  const window={};
  vm.runInNewContext(fs.readFileSync(path.join(dir,"player-photo-hover.js"),"utf8"),{
    window,document:{},fetch,Image:FakeImage,URLSearchParams,console
  },{filename:"player-photo-hover.js"});
  let secondaryVideo="match B.mp4";
  const api=window.VolleyballPlayerPhotoHover;
  const controller=api.init(root,id=>id===secondary.dataset.photoTrackId
    ?{...context,video:secondaryVideo,id:"103"}:context);
  assert.equal(typeof api.dismiss,"function");
  root.fire("pointerover",{target:button});
  root.fire("focusin",{target:button});
  await flush();
  assert.equal(requests,0,"Hover must never fetch metadata");
  assert.equal(root.children.length,3,"Hover must not create overlays");

  root.fire("click",{target:photo});
  await flush();
  assert.equal(requests,1);
  assert.equal(sampleOf(photo.src),"1");
  assert.equal(photo.dataset.photoSample,"2");
  assert.match(button.title,/2\/6/);
  for(let i=0;i<5;i++){
    root.fire("click",{target:button});
    await flush();
  }
  assert.equal(requests,1,"Metadata should be fetched once");
  assert.equal(sampleOf(photo.src),"0","Six clicks must return to first sample");

  // Multiple immediate clicks combine to the actual requested future frame.
  root.fire("click",{target:button});
  root.fire("click",{target:button});
  await flush();
  assert.equal(sampleOf(photo.src),"2","Rapid clicks must not collapse into one");

  // Image is swapped only after the candidate JPEG finishes loading.
  holdSample=3;
  const old=photo.src;
  root.fire("click",{target:button});
  await flush();
  assert.equal(photo.src,old,"Unloaded photo must never replace visible image");
  assert.equal(button.classList.contains("is-photo-loading"),true);
  root.fire("click",{target:button}); // Queued while sample 3 is downloading.
  releaseHeld();
  await flush();
  assert.equal(sampleOf(photo.src),"4","Click during loading should not be lost");
  assert.equal(button.classList.contains("is-photo-loading"),false);

  // Bad sample 5 is skipped and the last successful frame remains visible.
  failedSamples.add(5);
  root.fire("click",{target:button});
  await flush();
  assert.equal(sampleOf(photo.src),"0","Failed JPEG should fall back to next healthy sample");
  assert.equal(photo.style.opacity,"");
  assert(photoLoads>=8);

  // Delayed metadata for old video must never change another video image.
  const keyEvent=root.fire("keydown",{target:secondary,key:"Enter"});
  assert.equal(keyEvent.defaultPrevented,true);
  assert.equal(requests,2);
  secondaryVideo="match 2.mp4";
  pendingResolver({ok:true,json:async()=>({samples})});
  await flush();
  assert.equal(secondary.src,"/api/player-photo?sample=0");

  const linkEvent=root.fire("click",{target:linkImage});
  assert.equal(linkEvent.defaultPrevented,true);
  await flush();
  assert.equal(sampleOf(linkImage.src),"1");
  assert.equal(controller,api.init(root,()=>context),"Repeated init must not duplicate listeners");
  api.dismiss();
  console.log("PASS: fast repeat clicks, successful-image swap, broken-frame fallback, caching, stale requests");
}
test().catch(error=>{console.error(error);process.exitCode=1});