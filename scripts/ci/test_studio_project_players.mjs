import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";
import assert from "node:assert/strict";

const source=readFileSync(new URL("../../rust/volley-app/static/studio_tabs.js",import.meta.url),"utf8");
const start=source.indexOf("  const renderSidebarPlayers=()=>{");
const end=source.indexOf("  const renderPlayers=()=>{",start);
assert.ok(start>=0&&end>start);
const element=(tag,klass,text)=>({tag,klass,text,children:[],append(...items){this.children.push(...items);},
  addEventListener(){}});
const sidebar=element("div");
const state={players:[{id:"a",name:"Анна"},{id:"b",name:"Борис"}],
  projectMemberIds:new Set(["b"]),trackCovers:new Map()};
const context={state,$:()=>sidebar,clear:node=>{node.children=[];},make:element,URLSearchParams};
runInNewContext(source.slice(start,end)+"\nthis.renderSidebarPlayers=renderSidebarPlayers;",context);
context.renderSidebarPlayers();
assert.equal(sidebar.children.length,1);
assert.equal(sidebar.children[0].children[1].children[0].text,"Борис");
state.projectMemberIds=new Set(["a"]);
context.renderSidebarPlayers();
assert.equal(sidebar.children[0].children[1].children[0].text,"Анна");
state.projectMemberIds.clear();
context.renderSidebarPlayers();
assert.match(sidebar.children[0].text,/не добавлены/);
console.log("Player sidebar follows the selected project: OK");
