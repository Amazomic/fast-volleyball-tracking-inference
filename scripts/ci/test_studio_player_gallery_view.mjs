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
const context={state:{players:[{id:"a",name:"Анна",saved_photo_count:3}],projectMemberIds:new Set(["a"]),trackCovers:new Map(),playerTrackCounts:{a:2}},
  $:()=>sidebar,make,clear:element=>{element.children=[];},URLSearchParams,projectId:()=>"project",
  window:{dispatchEvent:event=>events.push(event)},CustomEvent:class{constructor(type,opts){this.type=type;this.detail=opts.detail;}}};
runInNewContext(source.slice(start,end)+"\nthis.render=renderSidebarPlayers;",context);
context.render();
const card=sidebar.children[0];
assert.equal(card.children[1].children[1].text,"3 сохранённых фото · 2 привязанных ID");
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
const galleryState={project:"project",projectPlayerIds:new Set(["a"]),viewPlayer:"",offset:48,tracksLoaded:true,unassignedIds:new Set([1,2,3])};
const galleryContext={state:galleryState,node:make,ui:{list:{before:node=>inserted.push(node)}},
  window:{addEventListener:(name,fn)=>{handlers[name]=fn;}},groupsMode:make("button"),
  playerMode:make("button"),modeSwitch:make("div"),groupingControls:make("div"),setMode:mode=>loads.push(mode),loadAlbum:()=>loads.push("album"),loadPage:()=>loads.push(galleryState.viewPlayer)};
runInNewContext(tracks.slice(viewStart,viewEnd)+"\nthis.updateUnassignedLabel=updateUnassignedLabel;",galleryContext);
galleryContext.updateUnassignedLabel();
assert.equal(galleryContext.modeSwitch.children[0],galleryContext.groupingControls);
assert.equal(galleryContext.modeSwitch.children[1].textContent,"Непривязанные Track ID · 3");
galleryState.unassignedIds.delete(1);galleryContext.updateUnassignedLabel();
assert.equal(galleryContext.modeSwitch.children[1].textContent,"Непривязанные Track ID · 2");
handlers["volley:player-gallery-select"]({detail:{project_id:"project",player_id:"a",name:"Анна"}});
assert.equal(galleryState.viewPlayer,"a");
assert.equal(galleryState.offset,0);
assert.deepEqual(loads,["player","album","a"]);
const back=inserted[0].children[0];
back.events.click();
assert.equal(galleryState.viewPlayer,"");
assert.equal(galleryContext.modeSwitch.children[1].textContent,"Непривязанные Track ID · 2");
assert.equal(loads.at(-1),"");
console.log("Player card opens its photo album: OK");
const menu=card.children.find(item=>item.tag==="details");
assert.ok(menu.children.some(item=>item.text==="Переименовать"));
assert.ok(menu.children.some(item=>item.text==="Удалить из проекта"));
