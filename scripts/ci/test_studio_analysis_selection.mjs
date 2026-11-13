import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {runInNewContext} from "node:vm";

const src=readFileSync(new URL("../../rust/volley-app/static/studio.js",import.meta.url),"utf8");
const slice=(start,end)=>{
  const a=src.indexOf(start),b=src.indexOf(end,a);
  assert.ok(a>=0&&b>a,"Cannot locate "+start);
  return src.slice(a,b);
};
const primaryA="[\"match.mp4\",\"fusion\",\"a\"]",primaryB="[\"match.mp4\",\"fusion\",\"b\"]";
const runA="a".repeat(32),runB="b".repeat(32);
const choice=value=>({value,id:value===primaryA?"a":"b",kind:"fusion",video:"match.mp4",
  label:value===primaryA?"Current GPU":"Other GPU",display_name:value===primaryA?"Current GPU":"Other GPU"});
const run=id=>({rally_run_id:id,display_name:id===runA?"Current CPU":"Other CPU",video:"match.mp4",created_at:0});
const chain=(values,runs)=>({
  choices:values.map(choice),
  byAnalysis:new Map(values.map(value=>[value,runs[value]||[]]))
});

// Test the actual picker click callbacks, not just static markup.
function element(tag,cls="",text=""){
  const classes=new Set(cls.split(" ").filter(Boolean));
  return {
    tag,cls,textContent:text,value:"",disabled:false,children:[],handlers:{},
    classList:{
      add:x=>classes.add(x),contains:x=>classes.has(x),
      toggle:(x,force)=>{if(force)classes.add(x);else classes.delete(x);}
    },
    setAttribute(k,v){this[k]=v;},getAttribute(k){return this[k]??"";},
    append(...nodes){this.children.push(...nodes);},
    addEventListener(k,fn){(this.handlers[k]??=[]).push(fn);},
    click:async function(){for(const fn of this.handlers.click||[])await fn();}
  };
}
const state={project:"abc",analysis:primaryA,run:runA,
  chain:chain([primaryA,primaryB],{[primaryA]:[run(runA),run(runB)],[primaryB]:[run(runB)]})};
const parent={insertBefore(){}};
const select=()=>Object.assign(element("select"),{parentNode:parent,
  options:[{value:"",textContent:"Choose"},{value:primaryA,textContent:"Current GPU"},
    {value:primaryB,textContent:"Other GPU"}],
  selectedOptions:[{textContent:"Current GPU"}]});
const ui={analysis:select(),run:select()};
ui.analysis.value=primaryA;
ui.run.options=[{value:"",textContent:"Choose"},{value:runA,textContent:"Current CPU"},
  {value:runB,textContent:"Other CPU"}];
ui.run.value=runA;
const calls=[],posts=[];
const pickerCtx={s:state,ui,make:element,wipe:x=>{x.children=[];},
  document:{addEventListener(){}},window:{confirm:()=>true,prompt:()=> "New name"},
  post:async(path,body)=>posts.push({path,body}),
  invalidateCatalog(){},message(){},failed:e=>{throw e;},
  loadChain:async(...args)=>calls.push(args),pickers:new Map()};
runInNewContext(slice("  const makeAnalysisPicker=","  // Shared by the analysis tab")+
  "\nthis.makeAnalysisPicker=makeAnalysisPicker;",pickerCtx);
pickerCtx.makeAnalysisPicker(ui.analysis,true);
// Capture the picker hosts that are inserted next to their native selects.
let hosts=[];
parent.insertBefore=(node)=>hosts.push(node);
pickerCtx.makeAnalysisPicker(ui.run,false);
const cpuHost=hosts[0];
assert.equal(cpuHost.children[1].children[0].children.length,3,
  "Every row needs choice, delete and rename buttons");
const actionButton=(row,action)=>row.children.find(item=>item.cls?.includes("analysis-picker-"+action));
assert.ok(actionButton(cpuHost.children[1].children[0],"delete").title.startsWith("Удалить:"));
await actionButton(cpuHost.children[1].children[1],"delete").click(); // delete a nonselected CPU run
assert.equal(calls.length,1);
assert.deepEqual(Array.from(calls.pop()),[primaryA,runA,true],
  "Deleting another CPU version must retain the active source");
await actionButton(cpuHost.children[1].children[0],"delete").click(); // delete active CPU run
assert.deepEqual(Array.from(calls.pop()),[primaryA,"",false],
  "Deleting active CPU should reset only dependent selection");
await actionButton(cpuHost.children[1].children[1],"rename").click(); // rename another CPU run
assert.deepEqual(Array.from(calls.pop()),[primaryA,runA,true]);
assert.equal(posts.at(-1).path,"/api/studio/analysis-rename");

pickerCtx.makeAnalysisPicker(ui.analysis,true);
const gpuHost=hosts.at(-1);
await actionButton(gpuHost.children[1].children[1],"delete").click(); // delete other GPU version
assert.deepEqual(Array.from(calls.pop()),[primaryA,runA,true],
  "Deleting another primary analysis must not blank current analysis");
await actionButton(gpuHost.children[1].children[0],"delete").click(); // delete selected GPU
assert.deepEqual(Array.from(calls.pop()),["","",false],
  "Deleting selected GPU correctly clears the affected selections");
assert.equal(src.includes('make("details","analysis-picker-actions")'),false,
  "Nested menu must not be reintroduced");

// Test metadata refresh without destroying the running preview.
const counters={reset:0,page:0,timeline:0,events:0};
const s={project:"abc",projectVideos:["match.mp4"],analysis:primaryA,run:runA,
  chain:chain([primaryA,primaryB],{[primaryA]:[run(runA)],[primaryB]:[run(runB)]}),
  chainEpoch:0};
const analysis={options:[],value:primaryA,disabled:false};
const cpu={options:[],value:runA,disabled:false};
const refreshed=chain([primaryA],{[primaryA]:[run(runA)]});
const overview={schema:"volley-native-analysis-overview/1",project_id:"abc",
  video:"match.mp4",metadata_only:true};
const options=(target,rows,key,label)=>{
  target.options=rows.map(row=>({value:row[key],textContent:label(row)}));
  target.value="";
};
const context={s,ui:{analysis,run:cpu},overlay:{raw:false},
  ensurePreviewVideo(){},
  window:{VolleyballUi:{ready:Promise.resolve()},
    VolleyStudioChain:{stamp:()=>"",build:()=>refreshed},
    dispatchEvent:()=>counters.events++},
  CustomEvent:class{constructor(type){this.type=type;}},
  reset(){counters.reset++;s.run="";},
  options,refreshPicker(){},chainSnapshot:{read:()=>null,save(){}},
  previewCache:{load:async path=>path.includes("/runs")?{runs:[]}:overview},
  url:(path,params)=>path+"?"+new URLSearchParams(params),
  sourceSelection:{save(){}},
  loadPage:()=>{counters.page++;return Promise.resolve();},
  loadTimeline:()=>{counters.timeline++;return Promise.resolve();},
  message(){},failed:e=>{throw e;},console};
runInNewContext(slice("  const setRuns=async(","  const projects=async(")+
  "\nthis.loadChain=loadChain;",context);
await context.loadChain(primaryA,runA,true);
assert.equal(counters.reset,0,"Refreshing other versions must never reset preview");
assert.equal(counters.page,0,"No preview reload for unrelated delete");
assert.equal(counters.events,0,"No GPU/CPU change event for unrelated delete");
assert.equal(s.analysis,primaryA);
assert.equal(s.run,runA);
assert.equal(analysis.value,primaryA);
assert.equal(cpu.value,runA);
assert.equal(cpu.options.length,1,"Deleted result must disappear from picker");

// Removing the active GPU version must select a surviving GPU and its linked CPU.
context.window.VolleyStudioChain.build=()=>chain([primaryB],{[primaryB]:[run(runB)]});
await context.loadChain("", "", false);
assert.ok(counters.reset>0,"Active GPU deletion should reset old preview");
assert.equal(s.analysis,primaryB);
assert.equal(s.run,runB);
console.log("PASS: version buttons and unrelated deletion preserve selected GPU, CPU and preview.");
