import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
import vm from "node:vm";
const root=fileURLToPath(new URL("../..",import.meta.url));
const src=readFileSync(join(root,"rust/volley-app/static/studio_tasks.js"),"utf8");
const html=readFileSync(join(root,"rust/volley-app/static/studio.html"),"utf8");
const css=readFileSync(join(root,"rust/volley-app/static/studio.css"),"utf8");
const web=readFileSync(join(root,"rust/volley-app/src/studio_web.rs"),"utf8");
const main=readFileSync(join(root,"rust/volley-app/src/main.rs"),"utf8");
const tabs=readFileSync(join(root,"rust/volley-app/static/studio_tabs.js"),"utf8");
const quick=readFileSync(join(root,"rust/volley-app/static/studio_quick_reel.js"),"utf8");

assert.match(html,/<section id="studio-sidebar-tasks"[^>]*class="[^"]*hidden"/);
assert.match(html,/<div id="studio-sidebar-task-list"/);
assert.match(html,/\/studio\/tasks\.js/);
assert.match(css,/\.studio-sidebar-tasks\{margin-top:auto;flex:0 0 auto/);
assert.match(css,/@media\(max-width:650px\)[\s\S]*?\.studio-sidebar-tasks/);
assert.match(main,/\.route\("\/studio\/tasks\.js", get\(studio_web::tasks_js\)\)/);
assert.match(web,/static\/studio_tasks\.js/);
assert.match(tabs,/VolleyballTasks\?\.set\("analysis"/);
assert.match(tabs,/VolleyballTasks\?\.clear\("analysis"\)/);
assert.match(quick,/VolleyballTasks\?\.set\("quick"/);

class Node {
  constructor(){
    this.children=[];this.attributes={};this.style={};this.classNames=new Set(["hidden"]);
    this.classList={
      toggle:(name,on)=>on?this.classNames.add(name):this.classNames.delete(name),
      contains:name=>this.classNames.has(name)
    };
  }
  append(...children){this.children.push(...children);}
  replaceChildren(...children){this.children=[...children];}
  setAttribute(k,v){this.attributes[k]=String(v);}
  removeAttribute(k){delete this.attributes[k];}
}
const container=new Node(),list=new Node(),project=new Node();
project.value="project-a";
const nodes=new Map([["studio-sidebar-tasks",container],
  ["studio-sidebar-task-list",list],["studio-project",project]]);
for(const el of nodes.values())el.addEventListener=()=>{};
let gpu={schema:"volley-native-active-gpu/1",project_id:"project-a",active:false};
let reel={status:"idle"};
let failQuick=false;
const hits=[];
const doc={hidden:false,getElementById:id=>nodes.get(id),createElement:()=>new Node(),
  addEventListener(){}};
const win={addEventListener(){},VolleyballStudioActivity:{
  async fetch(url){
    hits.push(url);
    if(url.startsWith("/api/quick-reel/")){
      if(failQuick)throw Error("network");
      return {ok:true,json:async()=>reel};
    }
    return {ok:true,json:async()=>gpu};
  }
}};
let interval=null,timeout=null;
vm.runInNewContext(src,{document:doc,window:win,URLSearchParams,
  setInterval:fn=>{interval=fn;return 1;},
  setTimeout:fn=>{timeout=fn;return 2;},clearTimeout:()=>{}},{filename:"studio_tasks.js"});
assert.ok(timeout,"active-job card must refresh periodically");
// The module starts its first probe asynchronously at initialization.
await new Promise(resolve=>setImmediate(resolve));
await win.VolleyballTasks.refresh();
assert.ok(container.classList.contains("hidden"),"idle card must be hidden");
assert.ok(hits.some(s=>s.includes("project_id=project-a")));

gpu={schema:"volley-native-active-gpu/1",project_id:"project-a",
  active:true,model:"players",phase:"running",progress_percent:35.6};
reel={status:"running",stage:"Монтаж",progress:88,stage_index:4};
await win.VolleyballTasks.refresh();
assert.equal(list.children.length,2);
assert.ok(!container.classList.contains("hidden"));
assert.equal(list.children[0].children[0].children[0].textContent,"Анализ GPU · RAVEL");
assert.equal(list.children[0].children[0].children[1].textContent,"36%");
assert.equal(list.children[1].children[0].children[1].textContent,"88%");

win.VolleyballTasks.set("cpu",{label:"Постобработка CPU",stage:"Касания",progress:null});
assert.equal(list.children.length,3);
const cpu=list.children[1];
assert.ok(cpu.children[2].classList.contains("is-indeterminate"));
assert.equal(cpu.children[2].attributes["aria-valuenow"],undefined);
assert.equal(cpu.children[1].textContent,"Касания");
win.VolleyballTasks.clear("cpu");
assert.equal(list.children.length,2);

failQuick=true;
await win.VolleyballTasks.refresh();
assert.equal(list.children.length,2,"network error must not falsely clear running job");
failQuick=false;
gpu={schema:"volley-native-active-gpu/1",project_id:"project-a",active:false};
reel={status:"complete",progress:100};
await win.VolleyballTasks.refresh();
assert.ok(container.classList.contains("hidden"),"after completion card should disappear");
assert.equal(list.children.length,0);
console.log("PASS: anchored sidebar task card, GPU+Quick server status, CPU indeterminate bar, reload polling, network failures and idle cleanup");
