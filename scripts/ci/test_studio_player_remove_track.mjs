import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";
import assert from "node:assert/strict";

const source=readFileSync(new URL("../../rust/volley-app/static/studio_tracks.js",import.meta.url),"utf8");
const start=source.indexOf("  const renderRows=()=>{");
const end=source.indexOf("  const edit=async",start);
const make=(tag,klass,text)=>({tag,klass,text,children:[],dataset:{},events:{},classList:{add(){},remove(){}},
  append(...items){this.children.push(...items);},setAttribute(){},addEventListener(name,fn){this.events[name]=fn;}});
const list=make("div"),calls=[];
const state={rows:[{track_id:7,assigned_player_id:"player",assigned_player_name:"Игрок",identity_source:"automatic_photo"}],
  viewPlayer:"player",filteredTotal:1,offset:0,limit:24,selected:new Set(),busy:false,writable:true};
const context={state,ui:{list,page:{}},singlesHeader:{},clear:item=>{item.children=[];},node:make,
  portrait:()=>make("img"),select:()=>{},attachSuggestions:()=>{},edit:(...args)=>calls.push(args)};
runInNewContext(source.slice(start,end)+"\nthis.renderRows=renderRows;",context);
context.renderRows();
const card=list.children[0];
const button=card.children.find(child=>child.tag==="button"&&child.text==="Удалить ID у игрока");
assert.ok(button,"player ID card must include a removal button");
button.events.click();
assert.equal(calls.length,1);
assert.equal(calls[0][0],"unassign");
assert.equal(calls[0][1][0],7);
console.log("Player track card removes its assignment: OK");
