"use strict";
// Frontend contract: one click requests restart and reloads only the new process.
const assert=require("node:assert/strict");
const fs=require("node:fs");
const vm=require("node:vm");
const path=require("node:path");
const root=path.resolve(__dirname,"../scripts/volleyball_ui/static");
const html=fs.readFileSync(path.join(root,"index.html"),"utf8");
const source=fs.readFileSync(path.join(root,"app.js"),"utf8");
assert(html.includes('id="server-restart"'));
const objects=new Map();
function el(selector){
  if(!objects.has(selector))objects.set(selector,{
    disabled:false,textContent:"",addEventListener(name,fn){this[name]=fn}
  });
  return objects.get(selector);
}
let reloads=0,posts=0;
const context=vm.createContext({
  document:{querySelector:el,querySelectorAll:()=>[],addEventListener(){}},
  window:{location:{reload(){reloads++}}},
  fetch:async()=>({ok:true,json:async()=>({
    ok:true,api_version:28,instance_id:"new",restarting:false,reload_required:false
  })}),
  setTimeout:fn=>{Promise.resolve().then(fn);return 1},
  clearTimeout(){},URL,URLSearchParams,Date,console
});
vm.runInContext(source,context,{filename:"app.js"});
const run=code=>vm.runInContext(code,context);
(async()=>{
  run('state.latestTask={status:"running"};updateServerRestartButton()');
  assert.equal(el("#server-restart").disabled,true,"active work must block restart");
  run('state.latestTask={status:"idle"};updateServerRestartButton()');
  assert.equal(el("#server-restart").disabled,false);
  context.__mock=async(url,payload)=>{
    assert.equal(url,"/api/system/restart");
    assert.equal(JSON.stringify(payload),"{}");
    posts++;
    return {accepted:true,instance_id:"old"};
  };
  run('api=__mock');
  await run('restartStudio()');
  assert.equal(posts,1);
  assert.equal(reloads,1,"reload only after new server generation is observed");
  assert.equal(el("#server-restart").disabled,false);
  console.log("PASS: restart button, active-job guard, one POST, generation-based refresh");
})().catch(e=>{console.error(e);process.exitCode=1});
