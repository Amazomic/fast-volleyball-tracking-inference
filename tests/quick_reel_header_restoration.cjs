"use strict";
/* Regression: completed Quick Reels never reappear in the header on reload. */
const fs=require("node:fs"),vm=require("node:vm"),assert=require("node:assert/strict");
const path=require("node:path");
const source=fs.readFileSync(path.join(__dirname,
  "../scripts/volleyball_ui/static/quick-reel-ui.js"),"utf8");
const latest="a".repeat(32);
const dismissKey="volley-quick-reel-dismissed-id";

function boot({state,jobStatus,previouslyClosed=false}){
  const elements=new Map(),storage=new Map(),calls=[];
  if(previouslyClosed)storage.set(dismissKey,latest);
  function element(id){
    if(!elements.has(id)){
      const events={};
      elements.set(id,{
        id,events,hidden:false,disabled:false,open:false,value:"",textContent:"",
        children:[],classList:{add(){},remove(){},toggle(){}},
        addEventListener(type,fn){events[type]=fn;},
        replaceChildren(...children){this.children=children;},
        append(...children){this.children.push(...children);},
        setAttribute(name,value){this[name]=String(value);},
        removeAttribute(name){delete this[name]},
        showModal(){this.open=true;},
        close(){this.open=false;},
        click(){return events.click?.();}
      });
    }
    return elements.get(id);
  }
  async function fetchMock(url){
    calls.push(url);
    let payload;
    if(url==="/api/quick-reel/latest")payload={reel_id:latest};
    else if(url.startsWith("/api/quick-reel/status?id="))
      payload={status:state,stage:state==="complete"?"Рилс готов":"Подготовка",
        progress:state==="complete"?100:15,available:state==="complete",videos:[]};
    else if(url==="/api/job")
      payload={kind:"quick_reel",reel_id:latest,status:jobStatus};
    else if(url.startsWith("/api/quick-reel/result?id="))
      payload={available:true,
        download_url:"/api/quick-reel/download?id="+latest};
    else if(url==="/api/quick-reel/history")
      payload={results:[{
        id:latest,created_at:1793740000,duration:40,
        videos:["saved.mp4"],
        url:"/api/quick-reel/media?id="+latest,
        download_url:"/api/quick-reel/download?id="+latest
      }]};
    else if(url==="/api/health")
      payload={ok:true,api_version:15,reload_required:false};
    else throw Error("Unexpected HTTP request "+url);
    return {ok:true,status:200,json:async()=>payload};
  }
  vm.runInNewContext(source,{
    window:{confirm:()=>false},
    document:{readyState:"complete",documentElement:{lang:"ru"},
      getElementById:element,
      createElement:()=>element("generated-"+elements.size)},
    localStorage:{
      getItem:key=>storage.get(key)||null,
      setItem:(key,value)=>storage.set(key,value),
      removeItem:key=>storage.delete(key)
    },
    fetch:fetchMock,console,
    setTimeout,clearTimeout,setInterval:()=>1,clearInterval:()=>{},
    URLSearchParams,encodeURIComponent
  },{filename:"quick-reel-ui.js"});
  return {el:element,calls,storage};
}
const flush=()=>new Promise(resolve=>setImmediate(resolve));
(async()=>{
  {
    const b=boot({state:"complete",jobStatus:"complete"});
    await flush();await flush();
    assert.equal(b.el("quick-reel-activity").hidden,true,
      "Completed latest reel must not resurface in the header");
    assert.equal(b.calls.filter(x=>x.includes("/api/quick-reel/result")).length,0,
      "No completed reel polling when loading the page");
    assert.equal(b.calls.filter(x=>x==="/api/quick-reel/history").length,0,
      "History is fetched only when opening the create form");
    b.el("quick-reel-open").click();
    await flush();
    assert.equal(b.el("quick-reel-activity").hidden,true,
      "Opening create form must not restore completed notification");
    assert.equal(b.el("quick-reel-history-list").children.length,1,
      "Ready MP4 appears in archive list");
    assert.equal(b.el("quick-reel-history-list").children[0].children[2].textContent,
      "×","Every completed reel has a delete cross");
  }
  {
    const b=boot({state:"running",jobStatus:"running"});
    await flush();await flush();
    assert.equal(b.el("quick-reel-activity").hidden,false,
      "An actual running quick reel can recover its progress");
    assert.equal(b.el("quick-reel-header-state").textContent,"Выполняется");
  }
  {
    const b=boot({state:"running",jobStatus:"running",previouslyClosed:true});
    await flush();await flush();
    assert.equal(b.el("quick-reel-activity").hidden,true,
      "A dismissed running notification remains dismissed across reloads");
  }
  {
    const b=boot({state:"running",jobStatus:"idle"});
    await flush();await flush();
    assert.equal(b.el("quick-reel-activity").hidden,true,
      "A stale JSON running status cannot reopen a nonexistent background job");
  }
  console.log("QUICK REEL: NO COMPLETED HEADER AFTER RELOAD / CLOSE IS STICKY / ARCHIVE ONLY: PASS");
})().catch(error=>{console.error(error);process.exitCode=1;});
