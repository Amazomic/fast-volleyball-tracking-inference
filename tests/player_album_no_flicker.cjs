"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const vm=require("node:vm");
const path=require("node:path");

const app=fs.readFileSync(path.resolve(__dirname,"../scripts/volleyball_ui/static/app.js"),"utf8");
const counter={textContent:"2 фото в личной базе"};
const roster={querySelectorAll:()=>[{
  dataset:{playerId:"player-one"},querySelector:()=>counter
}]};
const feedback={textContent:""};
let removed=false;
const card={remove(){removed=true}};
const button={dataset:{removeAlbumPhoto:"photo.jpg",confirmDelete:"true"},textContent:"Да, удалить",
  disabled:false,closest:selector=>selector===".player-album-card"?card:null};
const gallery={offsetHeight:400,style:{minHeight:""},innerHTML:"old cards",
  querySelectorAll:()=>removed?[]:[button],
  querySelector:()=>removed?{className:"other-photo"}:{className:"player-album-card"}};
const elements={"#player-album-gallery":gallery,"#named-list":roster,
  "#players-action-feedback":feedback};
const context=vm.createContext({
  document:{querySelector:sel=>elements[sel]||null,querySelectorAll:()=>[],addEventListener(){},activeElement:null},
  window:{confirm:()=>true,scrollTo:()=>{throw Error("Unexpected page scroll")}},
  console,clearTimeout,setTimeout,URLSearchParams
});
vm.runInContext(app,context,{filename:"app.js"});
vm.runInContext('state.project="project-one";state.activePlayerId="player-one";'+
  'state.playerGroups={status:"ready"};state.playerGroupsKey="old-key";',context);
let request=null;
context.api=async(endpoint,payload)=>{
  request={endpoint,payload};
  return {count:1};
};
context.renderPlayers=()=>{throw Error("Full roster rerender")};
context.renderPlayerAlbum=()=>{throw Error("Whole gallery reload")};
context.loadPlayerGroups=()=>{throw Error("Unexpected background ReID refresh")};
context.button=button;

(async()=>{
  await vm.runInContext("removePlayerAlbumPhoto(button)",context);
  assert.equal(request.endpoint,"/api/players/photos/remove");
  assert.equal(request.payload.filename,"photo.jpg");
  assert(removed,"only the removed photo should disappear");
  assert.equal(gallery.style.minHeight,"400px","height must be held to prevent scrolling");
  assert.equal(gallery.innerHTML,"old cards","gallery DOM must not be rebuilt");
  assert.equal(counter.textContent,"1 фото в личной базе");
  assert.equal(vm.runInContext("state.photoCounts['player-one']",context),1);
  assert.equal(vm.runInContext("state.playerGroups",context),null);
  assert.match(feedback.textContent,/удалена/);
  console.log("PASS: album photo removal updates only one card and its counter");
})().catch(error=>{console.error(error);process.exitCode=1});
