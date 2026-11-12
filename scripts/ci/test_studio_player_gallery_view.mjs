import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";
import assert from "node:assert/strict";

const source=readFileSync(new URL("../../rust/volley-app/static/studio_tabs.js",import.meta.url),"utf8");
const start=source.indexOf("  const renderSidebarPlayers=()=>{");
const end=source.indexOf("  const renderPlayers=()=>{",start);
const make=(tag,klass,text)=>({tag,klass,text,children:[],events:{},append(...items){this.children.push(...items);},
  addEventListener(type,fn){this.events[type]=fn;}});
const sidebar=make("div");
const events=[];
const context={state:{players:[{id:"a",name:"Анна"}],projectMemberIds:new Set(["a"]),trackCovers:new Map()},
  $:()=>sidebar,make,clear:element=>{element.children=[];},URLSearchParams,projectId:()=>"project",
  window:{dispatchEvent:event=>events.push(event)},CustomEvent:class{constructor(type,opts){this.type=type;this.detail=opts.detail;}}};
runInNewContext(source.slice(start,end)+"\nthis.render=renderSidebarPlayers;",context);
context.render();
const card=sidebar.children[0];
const button=card.children.find(item=>item.tag==="button"&&item.klass==="players-side-select");
assert.ok(button,"player card provides selection control");
button.events.click();
assert.equal(events[0].type,"volley:player-gallery-select");
assert.equal(events[0].detail.player_id,"a");
assert.equal(events[0].detail.project_id,"project");
const tracks=readFileSync(new URL("../../rust/volley-app/static/studio_tracks.js",import.meta.url),"utf8");
const viewStart=tracks.indexOf('  const viewBar=node("div","track-view-bar");');
const viewEnd=tracks.indexOf('  ui.list.before(modeSwitch);',viewStart);
const handlers={},inserted=[],loads=[];
const galleryState={project:"project",projectPlayerIds:new Set(["a"]),viewPlayer:"",offset:48};
const galleryContext={state:galleryState,node:make,ui:{list:{before:node=>inserted.push(node)}},
  window:{addEventListener:(name,fn)=>{handlers[name]=fn;}},groupsMode:make("button"),
  setMode:mode=>loads.push(mode),loadPage:()=>loads.push(galleryState.viewPlayer)};
runInNewContext(tracks.slice(viewStart,viewEnd),galleryContext);
handlers["volley:player-gallery-select"]({detail:{project_id:"project",player_id:"a",name:"Анна"}});
assert.equal(galleryState.viewPlayer,"a");
assert.equal(galleryState.offset,0);
assert.deepEqual(loads,["singles","a"]);
const back=inserted[0].children[1];
back.events.click();
assert.equal(galleryState.viewPlayer,"");
assert.equal(loads.at(-1),"");
console.log("Player card opens its Track ID gallery: OK");
