import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";
import assert from "node:assert/strict";

const source=readFileSync(new URL("../../rust/volley-app/static/studio_tracks.js",import.meta.url),"utf8");
const start=source.indexOf("  const loadVideos=async()=>{");
const end=source.indexOf("  const publishTrackCovers=",start);
assert.ok(start>=0&&end>start);
const project={value:"project"},run={value:"a".repeat(32)},sourceText={};
const state={project:"",video:"",replay:"",rallyRun:"",rows:[],token:0};
const pending=[];
let requests=0,pages=0;
const context={state,ui:{project,video:{},replay:{}},
  get:id=>id==="studio-run"?run:sourceText,
  reset:()=>{state.token++;state.rows=[];},loadLibraryRoster:async()=>{},
  options(){},note(){},path:()=>"runs",
  read:()=>{requests++;return new Promise(resolve=>pending.push(resolve));},
  loadPage:async()=>{pages++;state.rows=[];state.tracksLoaded=true;}
};
runInNewContext(source.slice(start,end)+"\nthis.loadVideos=loadVideos;",context);
const first=context.loadVideos();
const duplicate=context.loadVideos();
assert.equal(requests,1,"Repeated events during loading must share the same request");
pending[0]({runs:[{video:"match.mp4",source_replay_id:"b".repeat(32),rally_run_id:run.value}]});
await Promise.all([first,duplicate]);
await context.loadVideos();
assert.equal(requests,1,"An empty Track ID result must remain loaded");
assert.equal(pages,1);
run.value="c".repeat(32);
const changed=context.loadVideos();
assert.equal(requests,2,"A different analysis must load its own data");
pending[1]({runs:[{video:"other.mp4",source_replay_id:"d".repeat(32),rally_run_id:run.value}]});
await changed;
assert.equal(pages,2);
console.log("Track loading coalesces repeated events and changes with analysis: OK");
