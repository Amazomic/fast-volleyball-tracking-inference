"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const vm=require("node:vm");

class Node{
  constructor(tag="div"){
    this.tagName=tag;this.children=[];this.dataset={};this.attrs={};
    this.events=new Map();this.hidden=false;this.textContent="";
    this.style={};this.className="";
    this.classList={add(){}};
    this.parentNode=null;
  }
  addEventListener(type,handler){
    const handlers=this.events.get(type)||[];handlers.push(handler);this.events.set(type,handlers);
  }
  fire(type,options={}){
    for(const handler of this.events.get(type)||[]){
      handler({type,target:options.target||this,
        relatedTarget:options.relatedTarget||null,key:options.key,
        currentTarget:this});
    }
  }
  appendChild(node){this.children.push(node);node.parentNode=this;return node;}
  append(...nodes){nodes.forEach(node=>this.appendChild(node));}
  replaceChildren(...nodes){
    this.children=[];
    this.append(...nodes);
  }
  setAttribute(name,value){this.attrs[name]=value;}
  removeAttribute(name){delete this.attrs[name];}
  contains(other){
    for(let item=other;item;item=item.parentNode){
      if(item===this)return true;
    }
    return false;
  }
  closest(selector){
    for(let item=this;item;item=item.parentNode){
      if(selector.includes(".photo-peek")&&item.className==="photo-peek")return item;
      if(selector.includes("[data-photo-track-id]")&&item.dataset.photoTrackId)return item;
    }
    return null;
  }
  getBoundingClientRect(){
    return this.className==="photo-peek"
      ?{left:30,right:85,top:110,height:62}
      :{left:0,right:0,top:0,height:325};
  }
}

async function test(){
  const document=new Node("document");
  document.createElement=tag=>new Node(tag);
  document.body=new Node("body");
  document.body.parentNode=document;
  const root=new Node("div");
  document.body.appendChild(root);
  const a=new Node("button");
  a.className="photo-peek";
  a.dataset.id="101";
  root.appendChild(a);
  const b=new Node("img");
  b.dataset.photoTrackId="102";
  root.appendChild(b);
  const ctx={project:"project-1",video:"match 1.mp4",cache:"base123.cpu123"};
  let requests=0;
  let pendingResolver=null;
  const sampleRows=Array.from({length:3},(_,i)=>({
    index:i,frame:i*30+12,time_sec:i+.4,confidence:.8+i*.05
  }));
  const fetch=async url=>{
    requests++;
    assert.ok(String(url).startsWith("/api/player-photos?"));
    const parsed=new URL(String(url),"http://localhost:8765");
    assert.equal(parsed.searchParams.get("project_id"),ctx.project);
    assert.equal(parsed.searchParams.get("video"),ctx.video);
    assert.equal(parsed.searchParams.get("cache_id"),"base123");
    if(parsed.searchParams.get("id")==="102"){
      return new Promise(resolve=>{pendingResolver=resolve;});
    }
    return {ok:true,json:async()=>({samples:sampleRows})};
  };
  const window=new Node("window");
  window.innerWidth=1200;window.innerHeight=800;
  const staticDir=path.resolve(__dirname,"../scripts/volleyball_ui/static");
  const html=fs.readFileSync(path.join(staticDir,"index.html"),"utf8");
  const css=fs.readFileSync(path.join(staticDir,"players-workspace.css"),"utf8");
  assert.ok(html.includes('src="/player-photo-hover.js?v=2"'));
  assert.ok(css.includes(".player-track-hover-preview"));
  const sandbox={document,window,fetch,URLSearchParams,
    clearTimeout,setTimeout,console};
  vm.runInNewContext(fs.readFileSync(path.join(staticDir,"player-photo-hover.js"),"utf8"),
    sandbox,{filename:"player-photo-hover.js"});
  const hover=window.VolleyballPlayerPhotoHover.init(root,()=>ctx);
  assert.equal(typeof window.VolleyballPlayerPhotoHover.dismiss,"function",
    "renderPlayers must be able to close the active hover gallery");
  assert.equal(requests,0,"Do not eagerly load samples for all 1000 track IDs");

  root.fire("pointerover",{target:a});
  await new Promise(r=>setTimeout(r,140));
  await new Promise(r=>setImmediate(r));
  assert.equal(requests,1);
  const panel=document.body.children.find(n=>n.id==="player-track-hover-preview");
  assert.ok(panel&&!panel.hidden);
  assert.equal(panel.attrs.role,"tooltip");
  const strip=panel.children.find(n=>n.className==="player-track-hover-images");
  assert.ok(strip,"A gallery, not a single image");
  assert.equal(strip.children.length,3);
  assert.ok(strip.children.every(n=>n.children[0].src.includes("/api/player-photo?")));
  assert.ok(strip.children.every(n=>n.children[0].loading==="eager"));
  assert.equal(a.attrs["aria-describedby"],panel.id);
  assert.ok(strip.children[2].children[1].textContent.includes("2.4 с"));

  // Fast repeat hover reuses metadata; only photo image requests use browser cache.
  root.fire("pointerout",{target:a});
  window.VolleyballPlayerPhotoHover.dismiss();
  assert.equal(panel.hidden,true,"closing gallery on list re-render must be safe");
  root.fire("pointerover",{target:a});
  await new Promise(r=>setTimeout(r,140));
  assert.equal(requests,1,"Metadata for an ID should be cached");

  // Do not render stale frames after switching videos in the meantime.
  root.fire("pointerover",{target:b});
  await new Promise(r=>setTimeout(r,120));
  assert.equal(requests,2);
  ctx.video="different.mp4";
  pendingResolver({ok:true,json:async()=>({samples:sampleRows})});
  await new Promise(r=>setImmediate(r));
  assert.ok(!panel.children.some(n=>n.className==="player-track-hover-images"),
    "Stale metadata must never paint frames from an old video");

  document.fire("keydown",{key:"Escape"});
  assert.equal(panel.hidden,true);
  assert.equal(a.attrs["aria-describedby"],undefined);

  root.fire("pointerover",{target:a});
  root.fire("pointerout",{target:a});
  await new Promise(r=>setTimeout(r,130));
  assert.equal(requests,2,"Moving the mouse away before the delay must not fetch");
  console.log("PASS: lazy Track ID photo hover, 3 large samples, cache and stale-request protection");
}
test().catch(error=>{console.error(error);process.exitCode=1});
