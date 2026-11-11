import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
import vm from "node:vm";

const root=fileURLToPath(new URL("../..",import.meta.url));
const source=readFileSync(join(root,"rust/volley-app/static/studio_tasks.js"),"utf8");
const html=readFileSync(join(root,"rust/volley-app/static/studio.html"),"utf8");
const css=readFileSync(join(root,"rust/volley-app/static/studio.css"),"utf8");
const backend=readFileSync(join(root,"rust/volley-app/src/studio_restart.rs"),"utf8");
const routes=readFileSync(join(root,"rust/volley-app/src/main.rs"),"utf8");
assert.match(html,/<button id="studio-restart"[^>]*class="studio-sidebar-restart"/);
assert.match(html,/id="studio-restart-status"/);
assert.ok(html.indexOf('id="studio-restart"')>html.indexOf('id="studio-sidebar-task-list"'));
assert.match(css,/\.sidebar>.studio-sidebar-bottom\{[^}]*margin-top:auto/);
assert.match(routes,/\.route\("\/api\/studio\/restart", post\(studio_restart::restart\)\)/);
assert.match(backend,/runtime::active_job_types/);
assert.match(backend,/storage::validate_origin/);
assert.match(backend,/\.arg\("-f"\)/);

class Node {
  constructor(){
    this.children=[];this.attributes={};this.style={};this.value="";
    this.classNames=new Set(["hidden"]);this.listeners=new Map();this.disabled=false;
    this.classList={
      toggle:(key,on)=>on?this.classNames.add(key):this.classNames.delete(key),
      contains:key=>this.classNames.has(key)
    };
  }
  addEventListener(k,fn){this.listeners.set(k,fn);}
  click(){return this.listeners.get("click")?.();}
  append(...children){this.children.push(...children);}
  replaceChildren(...children){this.children=children;}
  setAttribute(key,value){this.attributes[key]=String(value);}
  removeAttribute(key){delete this.attributes[key];}
}
const ids=["studio-sidebar-tasks","studio-sidebar-task-list","studio-project",
  "studio-restart","studio-restart-status"];
const nodes=new Map(ids.map(id=>[id,new Node()]));
const doc={hidden:false,getElementById:id=>nodes.get(id),createElement:()=>new Node(),
  addEventListener(){}};
const button=nodes.get("studio-restart"),message=nodes.get("studio-restart-status");
nodes.get("studio-project").value="project-a";
let restartStatus=409,oldPid=201,newPid=201,reloaded=0,asked=0;
const win={
  location:{reload(){reloaded++;}},
  addEventListener(){},
  VolleyballStudioActivity:{
    async fetch(url){
      if(url==="/api/studio/restart"){
        asked++;
        if(restartStatus===409)return{ok:false,status:409,json:async()=>({error:"GPU still active"})};
        return{ok:true,json:async()=>({scheduled:true,previous_pid:oldPid})};
      }
      if(url==="/internal/rewrite-status")
        return {ok:true,json:async()=>({pid:newPid})};
      if(url.startsWith("/api/inference/native/full/active?"))
        return {ok:true,json:async()=>({schema:"volley-native-active-gpu/1",project_id:"project-a",active:false})};
      return {ok:true,json:async()=>({status:"idle"})};
    }
  }
};
let latestInterval=()=>{},nextId=0;
vm.runInNewContext(source,{document:doc,window:win,URLSearchParams,
  setInterval:fn=>{latestInterval=fn;return ++nextId;},
  clearInterval(){},
  setTimeout:()=>++nextId,clearTimeout(){}},{filename:"studio_tasks.js"});
await new Promise(r=>setImmediate(r));
await button.click();
assert.equal(asked,1);
assert.equal(button.disabled,false);
assert.match(message.textContent,/GPU still active/);
assert.ok(message.classList.contains("is-error"));
restartStatus=200;
await button.click();
assert.equal(asked,2);
assert.equal(button.disabled,true);
assert.match(message.textContent,/Ожидаем/);
await latestInterval();
assert.equal(reloaded,0,"Must not reload until a different process serves the API");
newPid=202;
await latestInterval();
assert.equal(reloaded,1,"Reload after a confirmed PID change");
console.log("PASS: pinned restart control, busy protection, full native restart request and reload only after new server PID");
