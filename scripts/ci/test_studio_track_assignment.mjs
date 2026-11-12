import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";
import assert from "node:assert/strict";

const source=readFileSync(new URL("../../rust/volley-app/static/studio_tracks.js",import.meta.url),"utf8");
const start=source.indexOf("  const bindSelected=async (ids,playerId)=>{");
const end=source.indexOf("  // Native appearance matches",start);
assert.ok(start>=0&&end>start);
const calls=[],messages=[];
const state={project:"p",video:"match.mp4",replay:"r",rallyRun:"run",sourceSha:"a".repeat(64),
  busy:false,rosterBusy:false,projectPlayerIds:new Set(["player"]),photoIndices:new Map([[7,2]])};
const context={state,edit:async(action,ids,player)=>{calls.push({action,ids,player});return true;},
  post:async(url,body)=>{calls.push({url,body});return {revision:"rev"};},
  note:(message,error)=>messages.push({message,error}),
  window:{dispatchEvent(){}},Event:class {}};
runInNewContext(source.slice(start,end)+"\nthis.bindSelected=bindSelected;",context);
await context.bindSelected([7,8],"player");
assert.equal(calls[0].action,"assign");
assert.deepEqual(calls[0].ids,[7,8]);
assert.equal(calls[1].body.action,"read");
assert.equal(calls[2].body.action,"enroll");
assert.equal(calls[2].body.track_id,7);
assert.equal(calls[2].body.rally_run_id,"run");
assert.equal(calls[2].body.photo_index,2);
assert.equal(calls[2].body.expected_source_sha256,"a".repeat(64));
assert.equal(calls[2].body.expected_revision,"rev");
calls.length=0;
await context.bindSelected([9],"other");
assert.equal(calls.length,0,"Only players in the project may be assigned");
context.edit=async()=>false;
await context.bindSelected([7],"player");
assert.equal(calls.length,0,"A failed assignment must not save a photo");
context.edit=async()=>true;
context.post=async(url,body)=>{
  if(body.action==="read")return {revision:"rev"};
  throw Error("album unavailable");
};
await context.bindSelected([7],"player");
assert.equal(messages.at(-1).error,true);
assert.match(messages.at(-1).message,/Track ID привязаны, но фото не сохранено/);
console.log("Manual assignment saves a group representative to the selected project player: OK");
