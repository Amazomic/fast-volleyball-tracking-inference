import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";

const js=readFileSync(new URL("../../rust/volley-app/static/studio.js",import.meta.url),"utf8");
const html=readFileSync(new URL("../../rust/volley-app/static/studio.html",import.meta.url),"utf8");
assert.match(html,/id="studio-overlay-processed"[^>]*role="tab"/);
assert.match(html,/id="studio-overlay-raw"[^>]*role="tab"/);
assert.match(html,/id="studio-raw-people-visible"/);
const start=js.indexOf("  const rawFusion=()=>");
const end=js.indexOf("  const setOverlaySource=",start);
assert.ok(start>=0&&end>start,"Preview overlay window helpers must exist");
const script=js.slice(start,end)+
  "\nthis.drawOverlay=drawOverlay;this.overlayWindowStart=overlayWindowStart;this.prefetchOverlayAtFrame=prefetchOverlayAtFrame;";

const oldRecord={
  start_frame:0,end_frame:180,
  frames:[{frame:90,players:[{id:1,box:[0.1,0.2,0.3,0.8]}],ball:null}]
};
const calls=[];
let outlines=0;
const renderedLabels=[];
const labelPositions=[];
const actionColors=[];
const ballColors=[];
const poseFills=[];
const ctx={
  strokeRect(){outlines++;},
  beginPath(){},arc(){},moveTo(){},lineTo(){},fill(){
    if(["#47e2ff","#ff8f55"].includes(this.fillStyle))ballColors.push(this.fillStyle);
    if(this.fillStyle==="#f9d977")poseFills.push(this.globalAlpha);
  },stroke(){actionColors.push(this.strokeStyle);},
  measureText(){return {width:20};},
  fillRect(){},fillText(value,x,y){renderedLabels.push(value);labelPositions.push({value,x,y});}
};
const s={project:"p",analysis:"a",chain:{choices:[{value:"a",kind:"fusion",id:"f"}]},run:"r",video:"v",videoKey:"p:v"};
const ui={video:{currentTime:90/30,src:"/api/video-media"}};
const overlayLabels={key:"",names:new Map(),loaded:false};
const overlay={
  source:{events:[]},flags:{players:true,touches:false,trajectory:false},
  cache:new Map([["p:cpu:r:v:0",oldRecord]]),rawFrames:new Map(),
  fetching:new Set(),failed:new Map()
};
const timeline={data:{project_id:"p",rally_run_id:"r",video:"v",total_frames:300}};
const scope={s,ui,overlay,overlayLabels,timeline,
  playerLabelKey:(project,run,video,sha)=>[project,run,video,sha].join(":"),
  requestOverlayLabels:()=>{},
  overlayContext:()=>({ctx,x:0,y:0,width:100,height:100}),
  fps:()=>30,url:(path,args)=>path+"?start_frame="+args.start_frame,
  request:resource=>{calls.push(resource);return new Promise(()=>{});},
  overlayMessage:()=>{}
};
runInNewContext(script,scope,{filename:"studio-overlay-window.js"});
assert.equal(scope.overlayWindowStart(89),0);
assert.equal(scope.overlayWindowStart(90),30);
assert.equal(scope.overlayWindowStart(180),120);

// At the 3-second boundary, the new request is outstanding but the
// previous window still contains the frame. Its boxes must not disappear.
// Prefetch 1.5 seconds before the new window becomes active.
ui.video.currentTime=45/30;
scope.drawOverlay();
assert.equal(calls.filter(s=>s.includes("start_frame=30")).length,1,
  "Next window must be prefetched before the 3-second boundary");

// A slow request must not erase already available player boxes.
ui.video.currentTime=90/30;
scope.drawOverlay();
scope.drawOverlay();
assert.equal(outlines,2,"Prior window must render while the next is fetching");
assert.equal(calls.filter(s=>s.includes("start_frame=30")).length,1,
  "Overlapping request must be deduplicated");

// No fabricated contours when saved evidence does not cover the frame.
ui.video.currentTime=180/30;
scope.drawOverlay();
assert.equal(outlines,2,"No stale boxes outside source window");
assert.equal(calls.filter(s=>s.includes("start_frame=120")).length,1);
// GPU overlays must remain useful without a CPU run or saved Touchs.
overlay.raw=true;s.run="";
overlay.rawBall={value:"70"};overlay.rawRavelBall={value:"55"};
overlay.rawVballVisible={checked:true};overlay.rawPeopleVisible={checked:true};
overlay.rawPlayer={value:"60"};overlay.rawBall={value:"0"};
overlay.rawPoseVisible={checked:false};overlay.rawPose={value:"35"};
overlay.flags.touches=true;
overlay.cache.set("p:gpu:f:v:0",{
  start_frame:0,end_frame:180,frames:[{frame:45,raw:{
    balls:[{source:"vballnet",point:[0.2,0.2],score:0.8},
      {source:"ravel",point:[0.3,0.3],score:0.7}],
    players:[{box:[0.1,0.2,0.3,0.8],score:0.9,touch_probability:0.9}],poses:[]
  }}]
});
ui.video.currentTime=45/30;
scope.drawOverlay();
assert.deepEqual(ballColors,["#47e2ff"],"Only VBallNet supplies the ball");
assert.equal(outlines,3,"Raw player boxes are independent of a CPU replay");
assert.equal(actionColors.length,0,"Raw contact probabilities cannot draw CPU Touchs");
assert.ok(!renderedLabels.some(label=>String(label).includes("RAVEL p(contact)")),
  "RAVEL probabilities are not displayed in the new player overlay");
ballColors.length=0;overlay.rawPeopleVisible.checked=false;
scope.drawOverlay();
assert.deepEqual(ballColors,["#47e2ff"],"People overlays can be hidden independently");
overlay.rawPeopleVisible.checked=true;overlay.rawVballVisible.checked=false;
ballColors.length=0;scope.drawOverlay();
assert.deepEqual(ballColors,[],"VBallNet can be hidden independently");
overlay.rawVballVisible.checked=true;overlay.rawBall.value="90";
ballColors.length=0;scope.drawOverlay();
assert.deepEqual(ballColors,[],"Ball confidence is filtered locally");
// Interpolated raw poses are visually distinct and never overwrite actual
// model samples on the six-frame RTMPose cadence.
const rawFrame=overlay.cache.get("p:gpu:f:v:0").frames[0].raw;
const joints=Array.from({length:17},()=>[0.3,0.4,0.95]);
rawFrame.poses_interpolated=[{keypoints:joints}];
rawFrame.poses=[];
overlay.rawPoseVisible.checked=true;
scope.drawOverlay();
assert.equal(poseFills.length,17);
assert.ok(poseFills.every(value=>value===0.78));
poseFills.length=0;
rawFrame.poses=[{keypoints:joints}];
scope.drawOverlay();
assert.equal(poseFills.length,17);
assert.ok(poseFills.every(value=>value===1));
overlay.rawPoseVisible.checked=false;
actionColors.length=0;
overlay.raw=false;s.run="r";
timeline.data.schema="volley-native-rally-timeline/1";
timeline.data.source_replay_sha256="a".repeat(64);
timeline.data.touch_annotations=[
  {frame_index:45,kind:"attack",ball_position_px:[20,20]},
  {frame_index:45,kind:"block",ball_position_px:[30,30]},
  {frame_index:45,kind:null,ball_position_px:[40,40]}
];
overlay.actionFlags={attack:true,block:true};
oldRecord.width=100;oldRecord.height=100;
oldRecord.replay_sha256=timeline.data.source_replay_sha256;
actionColors.length=0;
scope.drawOverlay();
assert.deepEqual(actionColors,["#ff6b6b","#c38cff","#c6bf96"],
  "Processed mode alone shows classified CPU Touchs");
overlay.flags.players=false;overlay.actionFlags={attack:false,block:true};
oldRecord.width=100;oldRecord.height=100;
oldRecord.replay_sha256=timeline.data.source_replay_sha256;
overlay.source.events=[
  {frame_index:45,ball_position_px:[20,20],action_prediction:{kind:"attack"}},
  {frame_index:45,ball_position_px:[30,30],rally_context:{estimated_kind:"block"}},
  {frame_index:45,ball_position_px:[40,40]}
];
actionColors.length=0;
renderedLabels.length=0;
scope.drawOverlay();
assert.deepEqual(actionColors,["#c38cff","#c6bf96"],
  "Processed mode hides disabled attack but renders CPU block and unclassified touch");
assert.ok(renderedLabels.includes("≈ Блок"),"Block must be labeled as such, not generic touch");
assert.ok(!renderedLabels.includes("Касание?"),"Legacy card touch labels are forbidden");
timeline.data.touch_annotations=[];
actionColors.length=0;
scope.drawOverlay();
assert.equal(actionColors.length,0,
  "Legacy card motion_events alone cannot create a Touch annotation");
const initStart=js.indexOf("  if(overlay.mode){");
const initEnd=js.indexOf("  const startOverlayFrameLoop=",initStart);
let changed,redraws=0;
const checkbox={dataset:{overlayAction:"attack"},checked:false,addEventListener:(name,fn)=>{if(name==="change")changed=fn;}};
const input={addEventListener(){}};
const controls={querySelectorAll:()=>[checkbox],classList:{toggle(){}}};
const tab=()=>({attributes:{},closest:()=>controls,addEventListener(name,fn){this[name]=fn;},setAttribute(name,value){this.attributes[name]=value;}});
const initOverlay={mode:tab(),rawMode:tab(),rawControls:controls,actionControls:controls,
  rawBall:input,rawVballVisible:input,rawPeopleVisible:input,
  rawPlayer:input,rawPose:input,rawPoseVisible:input,actionFlags:{attack:true}};
runInNewContext(js.slice(initStart,initEnd),{
  overlay:initOverlay,$:()=>({}),drawOverlay:()=>{redraws++;},drawTimeline:()=>{},
  setOverlaySource:()=>{},rawFusion:()=>({id:"f"}),s:{run:"",video:""},
  timeline:{status:{textContent:""},data:null},
  ensurePreviewVideo:()=>{},prefetchOverlayAtFrame:()=>{},overlayMessage:()=>{}
});
changed();
assert.equal(initOverlay.actionFlags.attack,false,"Checkbox disables the matching action");
assert.equal(redraws,1,"Changing the checkbox redraws the player");
initOverlay.rawMode.click();
assert.equal(initOverlay.raw,true);
assert.equal(initOverlay.rawMode.attributes["aria-selected"],"true");
initOverlay.mode.click();
assert.equal(initOverlay.raw,false);
assert.equal(initOverlay.mode.attributes["aria-selected"],"true");
// The actual presented timestamp wins over currentTime while decoding.
overlay.presentedTime=90/30;
ui.video.currentTime=180/30;
overlay.flags.players=true;overlay.flags.touches=false;
const beforePresented=outlines;
scope.drawOverlay();
assert.equal(outlines,beforePresented+1,"Presentation timestamp must control boxes during decode lag");
overlay.presentedTime=null;
s.videoKey="p:other";
scope.drawOverlay();
assert.equal(outlines,beforePresented+1,"Never draw a new CPU version on media from another source");
s.videoKey="p:v";
overlay.source.events=[];
timeline.data.touch_annotations=[
  {frame_index:45,kind:"block",ball_position_px:[30,30]}
];
overlay.flags.players=false;overlay.flags.touches=true;
ui.video.currentTime=45/30;
actionColors.length=0;
scope.drawOverlay();
assert.deepEqual(actionColors,["#c38cff"],
  "CPU timeline block survives changing the rally card and keeps its classification");
// Contacts are independently available while the large replay index is still
// loading. They must not wait for a player-box response.
const savedWindow=overlay.cache.get("p:cpu:r:v:0");
overlay.cache.delete("p:cpu:r:v:0");
ui.video.videoWidth=100;ui.video.videoHeight=100;
actionColors.length=0;
scope.drawOverlay();
assert.deepEqual(actionColors,["#c38cff"],
  "CPU contacts must render while the replay-window request is still outstanding");
overlay.cache.set("p:cpu:r:v:0",savedWindow);
// Opening a rally/seek primes both the immediate frame and the following
// overlapping window, rather than waiting for the first decoded video frame.
overlay.fetching.clear();
const requestsBefore=calls.length;
scope.prefetchOverlayAtFrame(180);
const early=calls.slice(requestsBefore);
assert.ok(early.some(path=>path.includes("start_frame=120")),
  "Seek should request the target player window immediately");
assert.ok(early.some(path=>path.includes("start_frame=210")),
  "Seek should also prefetch the next overlapping player window");
timeline.data.rally_run_id="wrong";
actionColors.length=0;
scope.drawOverlay();
assert.equal(actionColors.length,0,
  "Touch annotations from a foreign CPU version must never be displayed");
timeline.data.rally_run_id="r";
overlay.flags.touches=false;overlay.flags.players=true;
ui.video.currentTime=90/30;
timeline.data.source_replay_sha256="a".repeat(64);
oldRecord.replay_sha256="b".repeat(64);
const beforeDigest=outlines;
scope.drawOverlay();
assert.equal(outlines,beforeDigest,
  "A cached overlay from another replay SHA must never be drawn");
oldRecord.replay_sha256=timeline.data.source_replay_sha256;
scope.drawOverlay();
assert.equal(outlines,beforeDigest+1,
  "Matching source and replay SHA restores correct overlay display");

// Manually assigned names replace numbers, and stale CPU identities never leak.
s.videoKey="p:v";
timeline.data.rally_run_id="r";
timeline.data.source_replay_sha256="a".repeat(64);
oldRecord.replay_sha256=timeline.data.source_replay_sha256;
overlay.flags.players=true;
overlay.flags.touches=false;
ui.video.currentTime=90/30;
overlayLabels.key=scope.playerLabelKey("p","r","v",oldRecord.replay_sha256);
overlayLabels.loaded=true;
overlayLabels.names.set("1","Ильнур");
renderedLabels.length=0;
scope.drawOverlay();
assert.ok(renderedLabels.includes("Ильнур"),"Reviewed Track ID displays player name");
assert.ok(labelPositions.some(label=>label.value==="Ильнур"&&label.y>=80),"Player name appears below the box");
overlayLabels.names.clear();
renderedLabels.length=0;
scope.drawOverlay();
assert.ok(renderedLabels.includes("ID 1"),"Unassigned Track ID keeps numeric fallback");
overlayLabels.names.set("1","Ильнур");
overlayLabels.key=scope.playerLabelKey("p","old-run","v",oldRecord.replay_sha256);
renderedLabels.length=0;
scope.drawOverlay();
assert.ok(renderedLabels.includes("ID 1"),"Previous CPU run names cannot appear in new run");
overlayLabels.key=scope.playerLabelKey("p","r","v",oldRecord.replay_sha256);
overlayLabels.names.clear();

// A mismatched evidence duration must not let the seek bar run past EOF.
const durationStart=js.indexOf("  const timelineDuration=()=>{");
const durationEnd=js.indexOf("  const updateTimelineProgress=",durationStart);
assert.ok(durationStart>=0&&durationEnd>durationStart);
const durationScope={
  s:{project:"p",video:"v",videoKey:"p:v"},
  ui:{video:{duration:8.5}},
  timeline:{data:{total_frames:300,video_duration_sec:10}},
  fps:()=>30
};
runInNewContext(js.slice(durationStart,durationEnd)+
  "\nthis.timelineDuration=timelineDuration;",durationScope);
assert.equal(durationScope.timelineDuration(),8.5,
  "Decoded media duration, not maximum of estimates, must bound seeking");
durationScope.timeline.data=null;
assert.equal(durationScope.timelineDuration(),8.5,
  "Raw GPU-only preview must remain seekable even without a CPU timeline");

// Player labels are an independent, validated, source-bound read. A manual edit
// should refresh them without reloading or reprocessing the player boxes.
const labelsStart=js.indexOf("  const overlayLabels={");
const labelsEnd=js.indexOf("  const overlayWindowStart=",labelsStart);
assert.ok(labelsStart>=0&&labelsEnd>labelsStart);
let labelResponses=[];
let renderedOnNameChange=0;
const labelRuntime={
  request:()=>new Promise(resolve=>labelResponses.push(resolve)),
  url:(route)=>route,
  AbortController,
  s:{project:"p",run:"r",video:"v"},
  drawOverlay:()=>{renderedOnNameChange++;},
  overlayMessage:()=>{}
};
runInNewContext(js.slice(labelsStart,labelsEnd)+
  "\nthis.labelsApi={overlayLabels,requestOverlayLabels,invalidateOverlayLabels,playerLabelKey};",
  labelRuntime);
const labelsApi=labelRuntime.labelsApi;
const labelSha="a".repeat(64);
const labelResponse=(name,run="r",sha=labelSha)=>({
  schema:"volley-native-preview-player-labels/1",
  project_id:"p",rally_run_id:run,video:"v",
  source_replay_sha256:sha,identity_source:"manual",
  names:{"1":name}
});
const flush=()=>new Promise(resolve=>setImmediate(resolve));
labelsApi.requestOverlayLabels("p","r","v",labelSha);
labelsApi.requestOverlayLabels("p","r","v",labelSha);
assert.equal(labelResponses.length,1,"Name lookup must be deduplicated");
labelResponses.shift()(labelResponse("Ильнур"));
await flush();
assert.equal(labelsApi.overlayLabels.names.get("1"),"Ильнур");
assert.equal(renderedOnNameChange,1,"Loaded labels repaint existing frames");

labelsApi.invalidateOverlayLabels();
labelsApi.requestOverlayLabels("p","r","v",labelSha);
labelResponses.shift()(labelResponse("Ирек"));
await flush();
assert.equal(labelsApi.overlayLabels.names.get("1"),"Ирек",
  "A reviewed assignment change must refresh the name");

labelsApi.invalidateOverlayLabels();
labelsApi.requestOverlayLabels("p","r","v",labelSha);
const stale=labelResponses.shift();
labelsApi.invalidateOverlayLabels();
labelsApi.requestOverlayLabels("p","r","v",labelSha);
const current=labelResponses.shift();
stale(labelResponse("Неверный игрок"));
current(labelResponse("Ильнур"));
await flush();
assert.equal(labelsApi.overlayLabels.names.get("1"),"Ильнур",
  "A slow response from an invalidated assignment cannot overwrite a new one");

labelsApi.invalidateOverlayLabels();
labelsApi.requestOverlayLabels("p","r","v",labelSha);
labelResponses.shift()(labelResponse("Ильнур","r","b".repeat(64)));
await flush();
assert.equal(labelsApi.overlayLabels.names.size,0,
  "Foreign replay hashes must never produce athlete names");
console.log("Studio preview overlay frame continuity: OK");
