"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const path=require("node:path");
const vm=require("node:vm");

const src=fs.readFileSync(path.resolve(__dirname,"../scripts/volleyball_ui/static/app.js"),"utf8");
let removeCalls=0,requestCount=0,lastCancel=null,wasRemoved=false;
const actions={
  classList:{add(){},remove(){}},
  querySelector(selector){return selector==="[data-cancel-album-delete]"?lastCancel:null}
};
const button={
  dataset:{removeAlbumPhoto:"frame.jpg"},disabled:false,textContent:"Удалить фото",
  classList:{add(){},remove(){}},
  closest(selector){
    if(selector===".player-album-actions")return actions;
    if(selector===".player-album-card")return {remove(){wasRemoved=true}};
    return null;
  },
  after(element){lastCancel=element}
};
const gallery={style:{minHeight:""},offsetHeight:420,innerHTML:"preserved",
  querySelectorAll:()=>[button],querySelector:()=>({})};
const roster={querySelectorAll:()=>[]},feedback={textContent:""};
const els={"#player-album-gallery":gallery,"#named-list":roster,"#players-action-feedback":feedback};
const ctx=vm.createContext({
  document:{querySelector:selector=>els[selector]||null,querySelectorAll:()=>[],
    addEventListener(){},activeElement:null,
    createElement(){return {type:"",className:"",dataset:{},disabled:false,
      textContent:"",remove(){removeCalls++;}}}},
  window:{confirm(){throw Error("A browser confirmation dialog was opened")}},
  clearTimeout,setTimeout,URLSearchParams,console
});
vm.runInContext(src,ctx,{filename:"app.js"});
vm.runInContext('state.project="p";state.activePlayerId="one";',ctx);
ctx.api=async(endpoint)=>{requestCount++;assert.equal(endpoint,"/api/players/photos/remove");return {count:1}};
ctx.button=button;
(async()=>{
  vm.runInContext("requestPlayerAlbumDelete(button)",ctx);
  assert.equal(requestCount,0,"first click must not delete");
  assert.equal(button.textContent,"Да, удалить");
  assert.equal(lastCancel.textContent,"Отмена");
  vm.runInContext("cancelPlayerAlbumDelete(button)",ctx);
  assert.equal(button.textContent,"Удалить фото");
  assert.equal(button.dataset.confirmDelete,undefined);
  assert.equal(requestCount,0,"cancellation must not delete");
  assert.equal(removeCalls,1);
  vm.runInContext("requestPlayerAlbumDelete(button)",ctx);
  assert.equal(requestCount,0,"fresh first click still needs confirmation");
  await vm.runInContext("requestPlayerAlbumDelete(button)",ctx);
  assert.equal(requestCount,1,"second click removes the photo");
  assert.equal(wasRemoved,true);
  assert.equal(gallery.innerHTML,"preserved");
  console.log("PASS: inline confirm, cancel, and second-click deletion without modal");
})().catch(error=>{console.error(error);process.exitCode=1});
