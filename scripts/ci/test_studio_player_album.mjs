import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";
import assert from "node:assert/strict";

const source=readFileSync(new URL("../../rust/volley-app/static/studio_tabs.js",import.meta.url),"utf8");
const start=source.indexOf("  const renderSidebarPlayers=()=>{");
const end=source.indexOf("  const renderPlayers=()=>{",start);
const element=(tag,klass,text)=>({tag,klass,text,children:[],events:{},
  append(...items){this.children.push(...items);},
  addEventListener(name,fn){this.events[name]=fn;},setAttribute(){}});
const sidebar=element("div");
const state={players:[{id:"a",name:"Анна"}],projectMemberIds:new Set(["a"]),trackCovers:new Map()};
const calls=[];
const window={dispatchEvent:event=>calls.push(event),CustomEvent:class{constructor(type,options){this.type=type;this.detail=options.detail;}}};
const context={state,$:()=>sidebar,clear:node=>{node.children=[];},make:element,URLSearchParams,window,
  projectId:()=>"project",post:async()=>({photos:[
    {id:"1",image_sha256:"same"},{id:"2",image_sha256:"same"},{id:"3",image_sha256:"other"}]}),
  CustomEvent:window.CustomEvent};
runInNewContext(source.slice(start,end)+"\nthis.renderSidebarPlayers=renderSidebarPlayers;",context);
context.renderSidebarPlayers();
const card=sidebar.children[0];
const menu=card.children.find(item=>item.tag==="details");
assert.ok(menu,"project player has an actions menu");
const show=menu.children.find(item=>item.tag==="button"&&item.text.includes("фото"));
await show.events.click();
const gallery=card.children.find(item=>item.klass==="players-side-gallery");
assert.equal(gallery.children.filter(item=>item.tag==="img").length,2);
const attach=menu.children.find(item=>item.tag==="button"&&item.text.includes("Track ID"));
attach.events.click();
assert.equal(calls[0].detail.player_id,"a");
console.log("Project player album actions and exact duplicate filtering: OK");
