import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";
import assert from "node:assert/strict";

const source=readFileSync(new URL("../../rust/volley-app/static/studio_tracks.js",import.meta.url),"utf8");
const start=source.indexOf('  groupButton.addEventListener("click",async()=>{');
const end=source.indexOf('  const reviewEdit=',start);
assert.ok(start>=0&&end>start);
let click;
const requests=[],notes=[],modes=[],tasks=[],renders=[];
let finishFirst;
const first=new Promise(resolve=>{finishFirst=resolve;});
const state={busy:false,writable:true,token:1,rallyRun:"rally",unassignedIds:new Set([1,2,3,4,5,6,7,8,9])};
const scope={project_id:"project",video:"video",replay_id:"replay",expected_source_sha256:"sha",threshold_percent:75};
const context={state,groupButton:{addEventListener:(_,fn)=>{click=fn;}},reviewReady:()=>true,
  reviewScope:()=>scope,unlock:()=>{},reidUnlock:()=>{},note:(...args)=>notes.push(args),
  window:{VolleyballTasks:{set:(...args)=>tasks.push(args),clear:key=>tasks.push([key,"clear"])}},
  renderGroupCards:()=>renders.push(state.grouping),
  post:async(url,body)=>{requests.push({url,body});if(requests.length===1)await first;return {computed_count:body.track_ids.length,cache_persisted:true};},
  reviewLoad:async()=>true,setMode:mode=>modes.push(mode)};
runInNewContext(source.slice(start,end),context);
const running=click();
assert.equal(state.grouping,true,"group gallery displays its grouping state while work runs");
assert.equal(renders[0],true);
assert.equal(tasks[0][1].progress,0);
finishFirst();await running;
assert.equal(requests.length,2);
assert.deepEqual(Array.from(requests[0].body.track_ids),[1,2,3,4,5,6,7,8]);
assert.deepEqual(Array.from(requests[1].body.track_ids),[9]);
assert.equal(requests[0].body.rally_run_id,"rally");
assert.deepEqual(modes,["groups"]);
assert.equal(state.busy,false);
assert.equal(state.grouping,false);
assert.equal(renders.at(-1),false);
assert.ok(tasks.some(([,task])=>task.progress>0),"completed batches update sidebar progress");
assert.equal(tasks.at(-1)[1],"clear");
assert.equal(notes.at(-1)[1],undefined);
const renderStart=source.indexOf("  const renderGroupCards=()=>{");
const renderEnd=source.indexOf('  galleryMore.addEventListener',renderStart);
const items={children:["photo"],append(item){this.children.push(item);}};
const galleryContext={state:{grouping:true},galleryItems:items,galleryMore:{},
  clear:element=>{element.children=[];},node:(tag,klass,text)=>({tag,klass,text})};
runInNewContext(source.slice(renderStart,renderEnd)+"\nthis.renderGroupCards=renderGroupCards;",galleryContext);
galleryContext.renderGroupCards();
assert.equal(items.children.length,1);
assert.equal(items.children[0].text,"Идёт группировка похожих ID…");
assert.equal(galleryContext.galleryMore.hidden,true);
context.post=async()=>{throw Error("сеть недоступна");};
await click();
assert.equal(state.grouping,false,"failed grouping restores the gallery");
assert.equal(tasks.at(-1)[1],"clear","failed grouping removes the sidebar task");
assert.equal(notes.at(-1)[1],true);
console.log("Grouping prepares all unassigned IDs in bounded batches: OK");
