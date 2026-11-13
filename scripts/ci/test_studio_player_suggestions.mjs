import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";
import assert from "node:assert/strict";

const source=readFileSync(new URL("../../rust/volley-app/static/studio_tracks.js",import.meta.url),"utf8");
const start=source.indexOf("  const playerCandidates=ids=>{");
const end=source.indexOf("  const renderRows=()=>{",start);
const node=(tag,klass,text)=>({tag,klass,text,children:[],dataset:{},events:{},value:"",
  append(...items){this.children.push(...items);},setAttribute(){},addEventListener(name,fn){this.events[name]=fn;}});
const state={roster:[{id:"a",name:"Анна"},{id:"b",name:"Борис"},{id:"c",name:"Вера"}],
  playerMatches:{"7":[{player_id:"a",cosine_similarity:.37},{player_id:"b",cosine_similarity:.79}]}};
const binds=[];
const context={state,node,updateCardLink:()=>{},bindSelected:(ids,player)=>binds.push({ids,player})};
runInNewContext(source.slice(start,end)+"\nthis.attachSuggestions=attachSuggestions;",context);
const card=node("article");
context.attachSuggestions(card,[7]);
const panel=card.children[0],controls=panel.children[0];
assert.equal(panel.children.length,1,"no separate similarity blocks");
const select=controls.children[0],button=controls.children[1];
assert.equal(select.value,"b","highest similarity is selected by default");
assert.deepEqual(select.children.map(option=>option.text),["Борис · 79.0%","Анна · 37.0%","Вера"]);
assert.equal(binds.length,0,"selecting a suggestion never assigns automatically");
button.events.click();
assert.equal(binds[0].player,"b");
state.playerMatches={};
const unmatched=node("article");context.attachSuggestions(unmatched,[7]);
assert.equal(unmatched.children[0].children[0].children[0].value,"", "no guess when similarity is unavailable");
console.log("Player dropdown selects the best match without automatic assignment: OK");
