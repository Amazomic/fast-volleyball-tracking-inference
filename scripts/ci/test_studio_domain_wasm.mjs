import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {createHash} from "node:crypto";
const {instance:{exports:e}}=await WebAssembly.instantiate(readFileSync(new URL("../../rust/volley-app/static/volley_web.wasm",import.meta.url)),{});
assert.equal(typeof e.volley_data_reserve,"function");
const enc=new TextEncoder(),dec=new TextDecoder();
const write=bytes=>{const ptr=e.volley_data_reserve(bytes.length);assert.ok(ptr);new Uint8Array(e.memory.buffer,ptr,bytes.length).set(bytes);};
const read=()=>dec.decode(new Uint8Array(e.memory.buffer,e.volley_data_output_ptr(),e.volley_data_output_len()));
function call(op,data){write(enc.encode(JSON.stringify(data)));assert.equal(e.volley_domain_call(op),1);return JSON.parse(read());}
const id=n=>n.toString(16).padStart(32,"0"),video="a.mp4";
const input={videos:[video,"b.mp4"],overviews:{[video]:{stages:[{key:"fusion",items:[{id:id(1),ravel_job_id:id(2)}]},{key:"gpu",items:[{id:id(2)}]},{key:"tracking",items:[{id:id(3),source_fusion_id:id(1)}]}]}},runs:[{video,rally_run_id:id(4),source_replay_id:id(3)},{video,rally_run_id:id(5),source_replay_id:id(99)}]};
const chain=call(1,input);
assert.equal(chain.choices.length,2);
assert.equal(chain.choices[0].kind,"fusion");
assert.equal(chain.choices[1].kind,"unlinked");
assert.equal(chain.byAnalysis[chain.choices[0].value][0].rally_run_id,id(4));
assert.deepEqual(call(2,[{id:"a",start:null},{id:"b",start:30},{id:"c",start:10},{id:"d",start:30}]),["c","b","d","a"]);
for(const size of [0,3,55,56,63,64,65,4194311]){
 const bytes=Buffer.alloc(size,97),hash=e.volley_hash_begin();
 for(let offset=0;offset<size;offset+=131071){write(bytes.subarray(offset,offset+131071));assert.equal(e.volley_hash_update(hash),1);}
 assert.equal(e.volley_hash_finish(hash),1);
 assert.equal(read(),createHash("sha256").update(bytes).digest("hex"));
 assert.equal(e.volley_hash_update(hash),0);
}
const a=e.volley_hash_begin(),b=e.volley_hash_begin();
write(enc.encode("abc"));e.volley_hash_update(a);e.volley_hash_finish(b);
assert.equal(read(),createHash("sha256").digest("hex"));e.volley_hash_finish(a);
assert.equal(read(),createHash("sha256").update("abc").digest("hex"));
e.volley_hash_abort(e.volley_hash_begin());
assert.equal(e.volley_data_reserve(8388609),0);
console.log("PASS: actual WASM lineage, stable montage order and incremental SHA-256 vectors/independent contexts");
