// Dependency-free real browser controller behavior contract (mocked DOM and
// HTTP only). Rust tests separately prove path, source and MP4 authenticity.
import {readFileSync} from "node:fs";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
import vm from "node:vm";
import assert from "node:assert/strict";
const root=join(fileURLToPath(new URL("../..",import.meta.url)));
const source=readFileSync(join(root,"rust/volley-app/static/studio_quick_reel.js"),"utf8");
class Element {
  constructor(tag="div"){
    this.tagName=tag;this.listeners=new Map();this.children=[];
    this.textContent="";this.disabled=false;this.value="";this.files=[];
    this.attributes=new Map();
    this.classList={members:new Set(),
      add:key=>this.classList.members.add(key),
      remove:key=>this.classList.members.delete(key),
      toggle:(key,on)=>on?this.classList.members.add(key):this.classList.members.delete(key),
      contains:key=>this.classList.members.has(key)};
  }
  addEventListener(event,fn){this.listeners.set(event,fn);}
  dispatch(event,detail={}){
    return this.listeners.get(event)?.({
      key:detail.key, dataTransfer:detail.dataTransfer,preventDefault(){},
    });
  }
  replaceChildren(...nodes){this.children=[...nodes];}
  append(...nodes){this.children.push(...nodes);}
  setAttribute(name,value){this.attributes.set(name,value);}
  click(){return this.dispatch("click");}
}
const ids=[
  "drop","browser","files","sources","selection-count","clear","music",
  "start","cancel","refresh","progress","percent","status","stages",
  "videos","download","error","summary"
];
const job="c".repeat(32), linked="a".repeat(32), copied="b".repeat(32);
const body=(data,status=200)=>({ok:status<400,status,json:async()=>data});
const tick=async()=>{for(let i=0;i<5;i++)await new Promise(r=>setImmediate(r));};
async function scene(initial="idle"){
  const nodes=new Map(ids.map(k=>["studio-quick-"+k,new Element()]));
  const $=key=>nodes.get("studio-quick-"+key);
  $("stages").children=[new Element(),new Element(),new Element(),new Element()];
  const requestLog=[];let state=initial,invalidOutput=false,selected=false;
  let pickCount=0;
  const fetch=async(url,opts={})=>{
    requestLog.push({url,opts});
    if(url.startsWith("/api/quick-reel/native/status")){
      if(state==="idle")return body({status:"idle",stage_total:4});
      if(state==="running")return body({id:job,status:"running",stage_index:2,
        stage:"GPU RAVEL 1",progress:33,videos:[{video:"rally.mp4",state:"gpu",gpu_percent:40}]});
      if(state==="complete")return body({
        id:job,status:"complete",stage_index:4,progress:100,
        output:invalidOutput?{file:"quick_reel.mp4",mp4_quality:{has_audio:false}}:{
          file:"quick_reel.mp4",native_original_clips_verified:true,
          mp4_quality:{has_audio:true}
        }
      });
      return body({id:job,status:"cancelled",stage_index:2,progress:34});
    }
    if(url==="/api/quick-reel/native/pick"){
      assert.equal(opts.method,"POST");selected=true;pickCount++;
      if(pickCount>1)return body({cancelled:true,videos:[]});
      return body({videos:[{id:linked,name:"match.mp4",size_gb:0.75,linked:true,copied:false}]});
    }
    if(url==="/api/quick-reel/native/upload"){
      assert.equal(opts.method,"POST");
      assert.equal(opts.body.items.length,1);
      return body({videos:[{id:copied,name:"dragged.mp4",size_gb:0.01,copied:true}]});
    }
    if(url==="/api/quick-reel/native/release"){
      const payload=JSON.parse(opts.body);
      assert.deepEqual(payload.selection_ids,[copied]);
      return body({released:1});
    }
    if(url==="/api/montage/native/audio"){
      assert.equal(opts.method,"POST");
      return body({audio_file:"track_"+"d".repeat(32)+".wav"});
    }
    if(url==="/api/quick-reel/native/start"){
      const payload=JSON.parse(opts.body);
      assert.deepEqual(payload.selection_ids,[linked]);
      assert.equal(payload.audio_file,"track_"+"d".repeat(32)+".wav");
      assert.equal(payload.project_id,undefined);
      state="running";
      return body({id:job,status:"queued",stage_total:4});
    }
    if(url==="/api/quick-reel/native/cancel"){
      assert.equal(JSON.parse(opts.body).id,job);
      state="cancelled";
      return body({id:job,cancel_requested:true});
    }
    throw Error("Unknown route "+url);
  };
  const form=class {constructor(){this.items=[];}append(...args){this.items.push(args);}};
  const timers=new Map();let timerId=0;
  const context={document:{getElementById:id=>nodes.get(id),
      createElement:tag=>new Element(tag)},fetch,FormData:form,URLSearchParams,
    setInterval:fn=>{const id=++timerId;timers.set(id,fn);return id;},
    clearInterval:id=>timers.delete(id),console};
  vm.runInNewContext(source,context,{filename:"studio_quick_reel.js"});
  await tick();
  return {$,requestLog,timers,selected:()=>selected,setState:value=>state=value,
    setInvalid:value=>invalidOutput=value};
}
{
  const a=await scene();
  const {$}=a;
  assert.equal($("start").disabled,true);
  await $("drop").dispatch("keydown",{key:"Enter"});
  await tick();
  assert.equal(a.selected(),true,"Native dialog is used for click/keyboard selection");
  assert.equal($("sources").children.length,1);
  assert.match($("sources").children[0].textContent,/match.mp4/);
  await $("drop").dispatch("click");
  assert.equal($("sources").children.length,1,"Cancel picker preserves selection");
  assert.equal(a.requestLog.filter(r=>r.url==="/api/quick-reel/native/pick").length,2);
  assert.equal($("start").disabled,true,"Quick Reel requires soundtrack");
  $("music").files=[{name:"music.wav",size:1500}];
  $("music").dispatch("change");
  assert.equal($("start").disabled,false);
  await $("start").dispatch("click");
  await $("start").dispatch("click");
  await $("drop").dispatch("click");
  assert.equal(a.requestLog.filter(r=>r.url==="/api/quick-reel/native/start").length,1,"No duplicate quick runs");
  assert.equal(a.requestLog.filter(r=>r.url==="/api/quick-reel/native/pick").length,2,"No stale file picker while processing");
  assert.equal($("cancel").classList.contains("hidden"),false);
  assert.equal($("progress").value,33);
  assert.match($("summary").textContent,/GPU/);
  assert.equal($("percent").textContent,"33.0%");
  assert.equal($("stages").children[1].classList.contains("current"),true);
  assert.equal(a.timers.size,1);
  await $("cancel").dispatch("click");
  await $("cancel").dispatch("click");
  assert.equal(a.requestLog.filter(r=>r.url==="/api/quick-reel/native/cancel").length,1);
  assert.equal($("cancel").classList.contains("hidden"),true);
  assert.equal($("download").classList.contains("hidden"),true);
  assert.equal(a.timers.size,0,"Cancellation stops status interval");
}
{
  const a=await scene();
  const {$}=a;
  $("drop").dispatch("drop",{dataTransfer:{files:[{name:"huge.mp4",size:2*1024**3+1}]}});
  await tick();
  assert.equal(a.requestLog.filter(r=>r.url==="/api/quick-reel/native/upload").length,0);
  const file={name:"dragged.mp4",size:1000};
  $("drop").dispatch("drop",{dataTransfer:{files:[file]}});
  await tick();
  assert.match($("sources").children[0].textContent,/временная копия/);
  assert.equal($("sources").children.length,1);
  await $("clear").dispatch("click");
  assert.equal($("sources").children[0].textContent,"Видео ещё не выбраны");
  assert.equal($("start").disabled,true);
  assert.ok(a.requestLog.some(r=>r.url==="/api/quick-reel/native/release"));
}
{
  const a=await scene("running");
  const {$}=a;
  assert.equal($("cancel").classList.contains("hidden"),false,
    "Reopening a page resumes the previously running job");
  assert.equal($("progress").value,33);
  assert.match($("summary").textContent,/GPU/);
  a.setState("complete");
  await $("refresh").dispatch("click");
  assert.equal($("download").classList.contains("hidden"),false);
  assert.equal($("download").href,
    "/api/quick-reel/native/media?id="+job);
  assert.equal(a.timers.size,0);
  a.setInvalid(true);
  await $("refresh").dispatch("click");
  assert.equal($("download").classList.contains("hidden"),true,
    "No completed audio/source verification means no download");
}
console.log("Quick Reel UI: native picker, drop upload/cleanup, music, GPU progress, cancel, reconnect, authenticated download passed");