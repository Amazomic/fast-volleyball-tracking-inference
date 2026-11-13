import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";
import assert from "node:assert/strict";

const source=readFileSync(new URL("../../rust/volley-app/static/studio_tracks.js",import.meta.url),"utf8");
const start=source.indexOf("  const loadPage=async");
const end=source.indexOf("  const select=(track",start);
let finish;
const request=new Promise(resolve=>{finish=resolve;});
const list={children:["existing cards"],replaceChildren(...children){this.children=children;}};
const state={project:"p",video:"v",replay:"r",rallyRun:"rally",token:0,selected:new Set(),
  offset:0,limit:24,viewPlayer:"",sourceSha:"",groupLoaded:true,roster:[]};
const calls=[];
const context={state,ui:{list,total:{},assigned:{},excluded:{}},node:()=>"loading",
  unlock:()=>{},note:()=>{},path:()=>"url",read:()=>request,
  document:{body:{dataset:{}},startViewTransition:()=>{throw Error("whole page must not animate");}},
  window:{matchMedia:()=>({matches:false})},
  animateTrackCards:fn=>{calls.push("cards");fn();},
  publishTrackCovers:()=>{},showPlayerChoices:()=>{},updateUnassignedLabel:()=>calls.push("count"),renderRosterBar:()=>{},
  renderRows:()=>calls.push("rows"),renderGroupCards:()=>calls.push("groups"),reviewReady:()=>false};
runInNewContext(source.slice(start,end)+"\nthis.loadPage=loadPage;",context);
const pending=context.loadPage({preserveCards:true});
assert.deepEqual(list.children,["existing cards"],"cards remain visible while refreshing after assignment");
finish({schema:"volley-native-player-track-links/1",source_replay_sha256:"a".repeat(64),
  automatic_reid_binding:false,tracks:[],rally_track_ids:[],total_tracks:0,filtered_total:0,
  unassigned_track_ids:[],comparison_track_ids:[],roster:[]});
await pending;
assert.deepEqual(calls,["count","cards","rows","groups"],"count updates and only cards animate during refresh");
const animationStart=source.indexOf("  const animateTrackCards=update=>{");
const animationEnd=source.indexOf("  const loadPage=async",animationStart);
const oldPortrait={title:"Track ID 7"};
let reused;
const newPortrait={title:"Track ID 7",replaceWith:portrait=>{reused=portrait;}};
const motions=[];
const card=(left,portrait)=>({dataset:{trackCardKey:"single:7"},
  getBoundingClientRect:()=>({left,top:0}),querySelectorAll:()=>[portrait],
  animate:frames=>motions.push(frames)});
let current=[card(100,oldPortrait)];
const animationContext={ui:{list:{querySelectorAll:()=>current}},galleryItems:{querySelectorAll:()=>[]},
  document:{body:{dataset:{}}},window:{matchMedia:()=>({matches:false})}};
runInNewContext(source.slice(animationStart,animationEnd)+"\nthis.animateTrackCards=animateTrackCards;",animationContext);
animationContext.animateTrackCards(()=>{current=[card(50,newPortrait)];});
assert.equal(reused,oldPortrait,"existing loaded photo stays in the card");
assert.equal(motions.length,1);
assert.equal(motions[0][0].transform,"translate(50px, 0px)");
animationContext.document.body.dataset.uiMotion="reduce";
animationContext.animateTrackCards(()=>{current=[card(25,newPortrait)];});
assert.equal(motions.length,1,"reduced motion disables card animation");
console.log("Assignment refresh keeps cards visible and animates the update: OK");
