import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";
import assert from "node:assert/strict";

const source=readFileSync(new URL("../../rust/volley-app/static/studio_tracks.js",import.meta.url),"utf8");
const start=source.indexOf("  const setMode=mode=>{");
const end=source.indexOf('  singlesMode.addEventListener',start);
const button=()=>({attributes:{},setAttribute(name,value){this.attributes[name]=value;}});
const context={tracksPanel:{dataset:{}},singlesMode:button(),groupsMode:button(),playerMode:button(),
  playerGallery:{},groupButton:{},groupingControls:{}};
runInNewContext(source.slice(start,end)+"\nthis.setMode=setMode;",context);
context.setMode("singles");assert.equal(context.groupingControls.hidden,true);
assert.equal(context.singlesMode.attributes["aria-pressed"],"true");
context.setMode("groups");assert.equal(context.groupingControls.hidden,false);
assert.equal(context.groupsMode.attributes["aria-pressed"],"true");
assert.equal(context.singlesMode.attributes["aria-pressed"],"false");
context.setMode("player");assert.equal(context.groupingControls.hidden,true);
console.log("Grouping controls are shown only in the grouped IDs view: OK");
