import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
import {runInNewContext} from "node:vm";

const root=fileURLToPath(new URL("../..",import.meta.url));
const html=readFileSync(join(root,"rust/volley-app/static/studio.html"),"utf8");
const css=readFileSync(join(root,"rust/volley-app/static/studio.css"),"utf8");
const js=readFileSync(join(root,"rust/volley-app/static/studio.js"),"utf8");
const wrap=html.slice(html.indexOf('<div class="video-wrap">'),
  html.indexOf('<div class="viewer-details"'));
for(const id of ["studio-video-play","studio-video-mute",
  "studio-video-fullscreen","studio-video-mode"]){
  assert.equal((html.match(new RegExp('id="'+id+'"','g'))||[]).length,1,
    id+" must appear exactly once");
  assert.ok(wrap.includes('id="'+id+'"'),id+" must be inside video");
}
for(const mode of ["sequence","loop","free","once"])
  assert.match(wrap,new RegExp('<option value="'+mode+'">'));
assert.match(css,/#studio-preview-view \.viewer-playback-controls\{\s*position:absolute/);
assert.match(css,/#studio-preview-view \.video-playback-mode\{\s*position:absolute/);
assert.match(css,/grid-template-areas:"player list" "timeline timeline"/);
assert.match(js,/ui\.video\.play\(\)\.catch\(error=>\s*message/);
assert.match(js,/document\.addEventListener\("keydown",event=>\{[\s\S]*?event\.stopPropagation\(\);[\s\S]*?toggleVideoPlayback\(\);\s*\},true\)/);
assert.match(js,/\["INPUT","TEXTAREA","SELECT"\]\.includes\(event\.target\?\.tagName\)/);

const start=js.indexOf('  const nextRally=()=>{');
const end=js.indexOf('  ui.video.addEventListener("timeupdate"',start);
assert.ok(start>=0&&end>start,"Rally boundary logic must be present");
const script=js.slice(start,end)+"\nthis.nextRally=nextRally;this.onRallyBoundary=onRallyBoundary;";
const rallies=[
  {candidate_id:"a",start_frame:0,end_frame:299},
  {candidate_id:"b",start_frame:450,end_frame:749},
  {candidate_id:"c",start_frame:900,end_frame:1199}
];
const s={project:"p",run:"r",current:"a",clipStart:0,clipEnd:10,
  stopAt:10,seekAt:0,playbackMode:"sequence",
  items:[{candidate_id:"a",detected_start_frame:0,detected_end_frame:299}]};
let paused=0;const played=[];
const ui={video:{currentTime:10,paused:false,play:()=>Promise.resolve(),
  pause:()=>{paused++;}}};
const timeline={data:{project_id:"p",rally_run_id:"r",rallies}};
const ctx={fps:()=>30,s,ui,timeline,view:item=>played.push(item.candidate_id),message:()=>{}};
runInNewContext(script,ctx,{filename:"studio-rally-boundary.js"});
ctx.onRallyBoundary();
assert.deepEqual(played,["b"],"Sequence must use full timeline, not paginated cards");
s.current="c";s.clipEnd=40;ctx.onRallyBoundary();
assert.equal(paused,1,"Sequence must stop after last rally");
s.playbackMode="loop";s.clipStart=30;s.clipEnd=40;
s.stopAt=40;ui.video.currentTime=40;
ctx.onRallyBoundary();
assert.equal(ui.video.currentTime,30,"Loop must return to selected rally start");
assert.equal(s.stopAt,40);
s.playbackMode="once";ctx.onRallyBoundary();
assert.equal(paused,2,"Once must pause at rally boundary");
assert.equal(s.stopAt,null);
s.playbackMode="free";s.stopAt=40;ctx.onRallyBoundary();
assert.equal(paused,2,"Free mode must not pause at rally boundary");
assert.equal(s.stopAt,null);
console.log("Studio playback modes and controls: OK");
