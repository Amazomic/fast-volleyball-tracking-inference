import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";
import assert from "node:assert/strict";

const source=readFileSync(new URL("../../rust/volley-app/static/studio_tracks.js",import.meta.url),"utf8");
const start=source.indexOf("  const loadAlbum=async()=>{");
const end=source.indexOf("  const setMode=mode=>{",start);
const node=(tag,klass,text)=>({tag,klass,text,children:[],events:{},
  append(...items){this.children.push(...items);},addEventListener(name,fn){this.events[name]=fn;}});
const playerPhotos=node("div"),playerStatus=node("p"),calls=[],events=[];
const state={project:"project",viewPlayer:"player"};
const context={state,node,playerPhotos,playerStatus,clear:element=>{element.children=[];},
  path:(base,args)=>base+"?"+new URLSearchParams(args),
  post:async(url,body)=>{calls.push({url,body});return body.action==="read"?
    {writable:true,revision:"r1",photos:calls.length===1?[{id:"photo1"}]:[]}:{revision:"r2"};},
  window:{confirm:()=>true,dispatchEvent:event=>events.push(event.type)},Event:class{constructor(type){this.type=type;}}};
runInNewContext(source.slice(start,end)+"\nthis.loadAlbum=loadAlbum;",context);
await context.loadAlbum();
assert.equal(playerPhotos.children.length,1);
const card=playerPhotos.children[0];
assert.match(card.children[0].src,/player_id=player&reference_id=photo1/);
await card.children[1].events.click();
assert.equal(calls[1].body.action,"undo");
assert.equal(calls[1].body.reference_id,"photo1");
assert.equal(calls[1].body.expected_revision,"r1");
assert.equal(calls[1].body.project_id,"project");
assert.equal(playerPhotos.children.length,0);
assert.deepEqual(events,["volley:player-photo-saved"]);
console.log("Player album displays and removes a selected saved photo: OK");
