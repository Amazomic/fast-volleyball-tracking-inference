import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";
import assert from "node:assert/strict";

const source=readFileSync(new URL("../../rust/volley-app/static/studio_tracks.js",import.meta.url),"utf8");
const start=source.indexOf("  const updateCardLink=controls=>{");
const end=source.indexOf("  const reset=",start);
const button={disabled:true};
const select={value:"player"};
const card={querySelector:selector=>selector==="select"?select:button,dataset:{trackCount:"2"}};
const state={selected:new Set(),busy:true,writable:true,rosterBusy:false,offset:0,limit:24,filteredTotal:0};
const ui={assign:{},unassign:{},exclude:{},restore:{},prev:{},next:{},player:{value:""}};
const context={state,ui,tracksPanel:{querySelectorAll:()=>[card]},reidUnlock:()=>{}};
runInNewContext(source.slice(start,end)+"\nthis.unlock=unlock;",context);
context.unlock();
assert.equal(button.disabled,true);
state.busy=false;
context.unlock();
assert.equal(button.disabled,false,"selected player becomes assignable after grouping finishes");
state.busy=true;
context.unlock();
assert.equal(button.disabled,true,"assignment is disabled while work is running");
console.log("Track assignment controls follow processing state: OK");

