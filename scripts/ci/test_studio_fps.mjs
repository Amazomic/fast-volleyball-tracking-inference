import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";
const bytes=readFileSync(new URL("../../rust/volley-app/static/volley_web.wasm",import.meta.url));
const {instance:{exports:e}}=await WebAssembly.instantiate(bytes,{});
assert.equal(typeof e.volley_evidence_fps,"function");
const js=readFileSync(new URL("../../rust/volley-app/static/studio.js",import.meta.url),"utf8");
const segment=js.slice(js.indexOf("  const fps="),js.indexOf("  // Count only players"));
for(const rate of [25,30,30000/1001,60]){
 const context={s:{fps:rate},window:{VolleyballUi:{engine:e}}};
 runInNewContext(segment+'\nthis.second=second;this.length=length;',context);
 assert.equal(context.second(rate*10),"10,0");
 assert.equal(context.length({detected_start_frame:0,detected_end_frame:rate*10-1}),"10.0 с");
}
assert.equal(e.volley_evidence_fps(0),30);
assert.equal(e.volley_evidence_fps(NaN),30);
console.log("PASS: source-qualified FPS math 25/30/29.97/60 and legacy native evidence compatibility");
