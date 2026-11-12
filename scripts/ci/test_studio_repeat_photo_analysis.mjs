import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";
import assert from "node:assert/strict";

const source=readFileSync(new URL("../../rust/volley-app/static/studio_tracks.js",import.meta.url),"utf8");
const start=source.indexOf("  const repeatPhotoAnalysis=async()=>{");
const end=source.indexOf("  const reviewRender=record=>{",start);
assert.ok(start>=0&&end>start,"photo analysis action is defined");
const calls=[];
const state={project:"p",video:"v",replay:"r",rallyRun:"run",sourceSha:"a".repeat(64),
  token:1,busy:false,comparisonIds:new Set(Array.from({length:10},(_,i)=>i+1)),playerMatches:{}};
const context={state,reviewReady:()=>true,reviewScope:()=>({project_id:"p",video:"v",replay_id:"r",expected_source_sha256:state.sourceSha}),
  unlock(){},reidUnlock(){},reviewNote(){},renderRows(){},renderGroupCards(){},
  galleryStatus:{textContent:""},
  post:async(url,body)=>{calls.push({url,body});return body.action==="compare_photos"?
    {schema:"volley-native-player-photo-comparison/1",source_replay_sha256:state.sourceSha,auto_assign_enabled:false,confirmed:false,player_matches:{"1":[]},photo_count:1}:{};}};
runInNewContext(source.slice(start,end)+"\nthis.repeat=repeatPhotoAnalysis;",context);
await context.repeat();
assert.deepEqual(calls.map(call=>call.body.action),["compare_photos"]);
assert.equal(calls[0].body.rally_run_id,"run");
assert.ok("1" in state.playerMatches);
assert.equal(state.busy,false);
console.log("Repeat photo analysis requests a scoped server comparison: OK");
