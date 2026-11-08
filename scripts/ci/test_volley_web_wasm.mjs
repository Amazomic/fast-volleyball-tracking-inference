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
console.log("Actual Rust/WebAssembly browser selection engine: 80-ID order, cap, dedup, revision-independent mutations and memory exports passed ("+bytes.length+" bytes)");
