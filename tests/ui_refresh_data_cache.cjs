"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const vm=require("node:vm");
const path=require("node:path");

const source=fs.readFileSync(path.resolve(__dirname,
  "../scripts/volleyball_ui/static/data-cache.js"),"utf8");
const html=fs.readFileSync(path.resolve(__dirname,
  "../scripts/volleyball_ui/static/index.html"),"utf8");
assert(html.replace(/\r\n/g,"\n").includes(
  "<script>\n"+source.replace(/\r\n/g,"\n").trimEnd()+"\n</script>"),
  "the live inline bootstrap must match the standalone cache module");
const stores=new Map();
const caches={
  async open(name){
    if(!stores.has(name))stores.set(name,new Map());
    const entries=stores.get(name);
    return {
      async match(key){return entries.get(key)?.clone()},
      async put(key,value){entries.set(key,value.clone())},
      async keys(){return [...entries.keys()]},
      async delete(key){return entries.delete(key)},
    };
  },
  async delete(name){return stores.delete(name)}
};
const requests=[];
async function fetchMock(url){
  requests.push(url);
  const pending=url.includes("missing"),unavailable=url.includes("unavailable");
  const result={analyzed:!pending,available:!unavailable,
    video:new URL("http://local"+url).searchParams.get("video")};
  const json=JSON.stringify(result);
  return new Response(json,{headers:{"content-type":"application/json",
    "cache-control":"no-store","content-length":String(json.length)}});
}
const boot=()=>{
  const window={};
  vm.runInNewContext(source,{window,caches,fetch:fetchMock,
    URLSearchParams,console,Promise}, {filename:"data-cache.js"});
  return window.VolleyDataCache;
};
const pause=()=>new Promise(resolve=>setImmediate(resolve));
(async()=>{
  const a=boot();
  const one="/api/analysis-view?project_id=p&video=one.mp4&cache_id=run-1";
  const two="/api/analysis-view?project_id=p&video=two.mp4&cache_id=run-1";
  assert(a.supports(one));
  assert(!a.supports("/api/job"));
  assert(a.supports("/api/analysis-player-boxes?project_id=p&video=one.mp4&cache_id=run-1&start=0"));
  assert(a.supports("/api/analysis-model-touches?project_id=p&video=one.mp4&cache_id=run-1"));
  assert(!a.supports("/api/analysis-view?project_id=p&video=one.mp4"));
  const [r1,r2]=await Promise.all([a.get(one),a.get(one)]);
  assert.equal(r1.video,"one.mp4");
  assert.equal(r2.video,"one.mp4");
  assert.equal(requests.length,1,"concurrent duplicate requests are deduplicated");
  await pause();
  const refreshed=boot(); // Simulate F5: JS memory lost, persistent CacheStorage remains.
  assert.equal((await refreshed.get(one)).video,"one.mp4");
  assert.equal(requests.length,1,"F5 must reuse saved JSON without network");
  await refreshed.get(two);
  assert.equal(requests.length,2,"switching video fetches new source");
  const missing="/api/analysis-view?project_id=p&video=missing&cache_id=run-1";
  await refreshed.get(missing);
  await pause();
  await refreshed.get(missing);
  assert.equal(requests.length,4,"unfinished inference must never be persisted");
  const unavailable="/api/analysis-player-boxes?project_id=p&video=unavailable&cache_id=run-1";
  await refreshed.get(unavailable);
  await pause();
  await refreshed.get(unavailable);
  assert.equal(requests.length,6,"absent raw detections must not be persisted");
  await refreshed.invalidate();
  await refreshed.get(one);
  assert.equal(requests.length,7,"successful mutation invalidates old snapshots");
  await pause();
  assert.equal((await boot().get(one)).video,"one.mp4");
  assert.equal(requests.length,7);
  console.log("PASS: F5 persistence, source-scoped cache, unfinished results, invalidation");
})().catch(e=>{console.error(e);process.exitCode=1});
