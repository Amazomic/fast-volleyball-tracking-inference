// End-to-end execution of the actual Rust-compiled client binary.
// No fake WASM stubs, no browser runtime or npm dependencies required.
import { readFile } from "node:fs/promises";
import assert from "node:assert/strict";
const path = new URL("../../rust/volley-app/static/volley_web.wasm",import.meta.url);
const bytes = await readFile(path);
assert.equal(bytes.subarray(0,4).toString("hex"),"0061736d");
assert.ok(bytes.length>1000&&bytes.length<1048576);
const {instance} = await WebAssembly.instantiate(bytes,{});
const e=instance.exports;
assert.equal(e.volley_web_api_version(),1);
assert.equal(e.volley_selection_limit(),80);
const encoder=new TextEncoder(),decoder=new TextDecoder();
function buffer(){
  const ptr=e.volley_id_buffer_ptr();
  return new Uint8Array(e.memory.buffer,ptr,24);
}
function write(id){
  assert.equal(id.length,24);
  buffer().set(encoder.encode(id));
}
function read(index){
  assert.equal(e.volley_selection_at(index),1);
  return decoder.decode(buffer());
}
e.volley_selection_clear();
assert.equal(e.volley_selection_count(),0);
const a="0123456789abcdef01234567";
const b="abcdef0123456789abcdef01";
write(a);assert.equal(e.volley_selection_add(),1);
write(b);assert.equal(e.volley_selection_add(),1);
write(a);assert.equal(e.volley_selection_add(),1); // no duplicate
assert.equal(e.volley_selection_count(),2);
assert.equal(read(0),a);
assert.equal(read(1),b);
assert.equal(e.volley_selection_at(2),0);
write(a);assert.equal(e.volley_selection_contains(),1);
assert.equal(e.volley_selection_remove(),1);
assert.equal(e.volley_selection_count(),1);
assert.equal(read(0),b);
write(a.toUpperCase());
assert.equal(e.volley_selection_add(),0);
e.volley_selection_clear();
for(let i=0;i<80;i++){
  write(i.toString(16).padStart(24,"0"));
  assert.equal(e.volley_selection_add(),1);
}
write("f".repeat(24));
assert.equal(e.volley_selection_add(),-1);
assert.equal(e.volley_selection_count(),80);
assert.equal(read(0),"0".repeat(24));
assert.equal(read(79),(79).toString(16).padStart(24,"0"));
e.volley_selection_clear();
assert.equal(e.volley_selection_count(),0);
// The same real compiled WASM now validates six-tab routing, browser-only
// visual settings, stale async response epochs and every RU/EN text entry.
assert.equal(e.volley_ui_api_version(),1);
assert.equal(e.volley_ui_tab_count(),6);
e.volley_ui_reset();
assert.equal(e.volley_ui_get(0),0);
assert.equal(e.volley_ui_set(0,5),1);
assert.equal(e.volley_ui_get(0),5);
assert.equal(e.volley_ui_set(0,6),0);
assert.equal(e.volley_ui_get(0),5);
for(let key=1;key<=4;key++){
  assert.equal(e.volley_ui_set(key,1),1);
  assert.equal(e.volley_ui_get(key),1);
  assert.equal(e.volley_ui_set(key,2),0);
}
assert.equal(e.volley_ui_get(6),-1);
const epoch1=e.volley_ui_begin_load(2);
const epoch2=e.volley_ui_begin_load(2);
assert.ok(epoch1>0&&epoch2>epoch1);
assert.equal(e.volley_ui_current_load(2,epoch1),0);
assert.equal(e.volley_ui_current_load(2,epoch2),1);
assert.equal(e.volley_ui_begin_load(6),0);
const words=e.volley_ui_text_count();
assert.ok(words>160&&words<500,"Expected substantial Rust-owned bilingual UI strings");
const sourceKeys=new Set(),allWords=[];
function encodedText(index){
  assert.equal(e.volley_ui_text_copy(index),1);
  const ptr=e.volley_ui_text_ptr(),len=e.volley_ui_text_len();
  assert.ok(len>0&&len<=512);
  return decoder.decode(new Uint8Array(e.memory.buffer,ptr,len));
}
for(let i=0;i<words;i++){
  assert.equal(e.volley_ui_set(1,0),1);
  const ru=encodedText(i);
  assert.equal(e.volley_ui_set(1,1),1);
  const en=encodedText(i);
  assert.ok(!sourceKeys.has(ru),"Duplicate Russian source translation: "+ru);
  assert.ok(ru.length&&en.length);
  sourceKeys.add(ru);allWords.push([ru,en]);
}
assert.ok(allWords.some(pair=>pair[0]==="Быстрый рилс"&&pair[1]==="Quick Reel"));
assert.ok(allWords.some(pair=>pair[0].includes("Старые проекты просматриваются")
  &&pair[1].includes("Legacy projects")));
assert.ok(allWords.some(pair=>pair[0].includes("ID предварительные")
  &&pair[1].includes("IDs are provisional")));
assert.equal(e.volley_ui_text_copy(words),0);
assert.equal(e.volley_ui_text_len(),0);
e.volley_ui_reset();
assert.equal(e.volley_ui_get(1),0);
assert.equal(e.volley_ui_current_load(2,epoch2),0);
// Inventory the real static DOM: all concise visible Russian controls
// should be in the Rust/WASM-owned translation catalogue, not JS literals.
const html=await readFile(new URL("../../rust/volley-app/static/studio.html",import.meta.url),"utf8");
const visible=[...html.matchAll(/>([^<>]+)</gu)]
  .map(m=>m[1].replaceAll("&nbsp;"," ").trim())
  .filter(t=>/[А-Яа-яЁё]/u.test(t));
const translated=sourceKeys;
const normal=s=>s.replace(/^(?:[↻↗‹›]\s*|\d+[.]\s+)+/gu,"").trim();
const untranslated=[...new Set(visible.map(normal))].filter(t=>t&&!translated.has(t));
console.log("Rust/WASM static HTML coverage:",
  [...new Set(visible.map(normal))].length-untranslated.length,
  "/",[...new Set(visible.map(normal))].length,"texts");
if(untranslated.length)console.log("Untranslated static phrases:",untranslated.join(" || "));
assert.ok(untranslated.length<=12,"Untranslated top-level static UI text exceeds translation budget");
const attrs=[...html.matchAll(/(?:aria-label|title|placeholder)="([^"]+)"/gu)]
  .map(m=>m[1]).filter(t=>/[А-Яа-яЁё]/u.test(t));
const missingAccessibility=[...new Set(attrs)].filter(t=>!translated.has(t));
assert.deepEqual(missingAccessibility,[],
  "Every Russian aria-label, title and placeholder must have an embedded English translation");
assert.deepEqual(untranslated,[],
  "Every static visible Russian HTML label must be translated by the Rust/WASM catalogue");
console.log("WASM UI accessibility translation coverage: "+
  [...new Set(attrs)].length+"/"+[...new Set(attrs)].length+" labels");
console.log("Actual Rust/WASM UI: six tabs, validated preferences, load epochs, "+
  words+" full RU/EN text pairs and out-of-bounds checks passed");
console.log("Actual Rust/WebAssembly browser selection engine: 80-ID order, cap, dedup, revision-independent mutations and memory exports passed ("+bytes.length+" bytes)");
