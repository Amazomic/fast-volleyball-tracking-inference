"use strict";
// The sidebar ball and favicon share one angle and the same volleyball PNG.
// Slow network requests and real studio jobs both count as activity.
(()=>{
  const ball=document.querySelector(".brand .volleyball-ball");
  if(!ball)return;
  const favicon=document.querySelector('link[rel="icon"]');
  const prefersReducedMotion=window.matchMedia("(prefers-reduced-motion: reduce)");
  const activeTasks=new Set();
  let pending=0,frame=0,hiddenTimer=null,lastTime=0,angle=0,speed=0,lastIconTime=0;
  const canvas=favicon?document.createElement("canvas"):null;
  if(canvas){canvas.width=64;canvas.height=64;}
  const ctx=canvas?.getContext?.("2d");
  let iconImage=null;
  const motionAllowed=()=>!prefersReducedMotion.matches &&
    document.body.dataset.uiMotion!=="reduce";
  const working=()=>pending>0||activeTasks.size>0;

  function paintFavicon(time,force=false){
    if(!favicon||!ctx||!iconImage||(!force&&time-lastIconTime<32))return;
    try{
      ctx.clearRect(0,0,64,64);
      ctx.save();
      ctx.translate(32,32);
      ctx.rotate(angle*Math.PI/180);
      ctx.drawImage(iconImage,-32,-32,64,64);
      ctx.restore();
      favicon.href=canvas.toDataURL("image/png");
      lastIconTime=time;
    }catch{/* Keep the original SVG favicon when canvas is unavailable. */}
  }
  if(ctx&&typeof Image==="function"){
    const image=new Image();
    image.onload=()=>{
      iconImage=image;
      if(working())paintFavicon(performance.now(),true);
    };
    image.src=ball.currentSrc||ball.src||"/studio/volleyball-photo.png";
  }
  function tick(time){
    const dt=lastTime?Math.min((time-lastTime)/1000,document.hidden?0.7:0.05):0;
    lastTime=time;
    const moving=working()&&motionAllowed();
    const target=moving?145+130*(0.5+0.5*Math.sin(time/900)):0;
    speed=motionAllowed()?speed+(target-speed)*Math.min(1,dt*(moving?3:5)):0;
    angle=(angle+speed*dt)%360;
    ball.style.transform="rotate("+angle.toFixed(2)+"deg)";
    if(moving||speed>0.2)paintFavicon(time);
    else paintFavicon(time,true);
    if(!document.hidden&&(moving||speed>0.2))frame=requestAnimationFrame(tick);
    else {frame=0;if(!moving&&speed<=0.2)lastTime=0;}
  }
  function ensure(){
    if(document.hidden){
      if(hiddenTimer===null&&((working()&&motionAllowed())||speed>0.2)){
        hiddenTimer=setInterval(()=>{
          tick(performance.now());
          if(!working()&&speed<=0.2){
            clearInterval(hiddenTimer);hiddenTimer=null;
          }
        },50);
      }
    }else if(!frame&&(working()||speed>0.2)){
      frame=requestAnimationFrame(tick);
    }
  }
  // An impulse, a gravity-driven arc and perspective depth, clipped by the
  // unchanged icon frame. The activity spinner runs on the inner ball.
  const logo=ball.closest?.(".logo");
  const stage=logo?.querySelector?.(".volleyball-ball-stage");
  let kicking=false;
  logo?.addEventListener("click",()=>{
    if(kicking||!stage)return;
    kicking=true;
    logo.classList.remove("is-returning");
    logo.classList.add("is-kicked");
    const fly=motionAllowed()&&!document.hidden;
    if(fly){
      const launchTime=performance.now();
      const flight=(now)=>{
        if(!logo.classList.contains("is-kicked"))return;
        const elapsed=Math.max(0,(now-launchTime)/1000);
        if(elapsed<0.065){
          // Contact compression, followed by a rapid release.
          const impulse=Math.sin(Math.PI*elapsed/0.065);
          stage.style.opacity="1";
          stage.style.transform="scale("+(1+0.17*impulse).toFixed(3)+","+
            (1-0.22*impulse).toFixed(3)+")";
        }else{
          const t=Math.min(0.68,elapsed-0.065);
          const progress=t/0.68;
          // Drag slows horizontal motion. Gravity pulls the arc down.
          const x=24/1.15*(1-Math.exp(-1.15*t));
          const y=-35*t+0.5*115*t*t;
          // Growing distance makes the same-size ball look smaller.
          const z=100*t+360*t*t;
          const scale=27/(27+z);
          const spin=560*t;
          stage.style.transform="translate3d("+x.toFixed(2)+"px,"+
            y.toFixed(2)+"px,0) rotate("+spin.toFixed(1)+"deg) scale("+
            scale.toFixed(4)+")";
          stage.style.opacity=progress>.82?
            String(Math.max(0,1-(progress-.82)/.18)):"1";
        }
        if(elapsed<0.745)requestAnimationFrame(flight);
        else stage.style.opacity="0";
      };
      requestAnimationFrame(flight);
    }else{
      // Both browser and in-app reduced-motion preferences are respected.
      stage.style.opacity="0";
    }
    setTimeout(()=>{
      logo.classList.remove("is-kicked");
      stage.style.transform="";
      stage.style.opacity="";
      logo.classList.add("is-returning");
      setTimeout(()=>{
        logo.classList.remove("is-returning");
        kicking=false;
      },410);
    },fly?1250:400);
  });
  prefersReducedMotion.addEventListener?.("change",ensure);
  window.addEventListener?.("volley:ui-language",ensure);
  document.addEventListener?.("visibilitychange",()=>{
    if(!document.hidden){
      if(hiddenTimer!==null){clearInterval(hiddenTimer);hiddenTimer=null;}
      lastTime=0;
    }
    ensure();
  });
  const begin=()=>{
    let done=false,started=false;
    const delay=setTimeout(()=>{
      if(done)return;
      started=true;
      pending++;
      ensure();
    },170);
    return ()=>{
      if(done)return;
      done=true;
      clearTimeout(delay);
      if(started){pending=Math.max(0,pending-1);ensure();}
    };
  };
  window.VolleyballStudioActivity={
    begin,
    setTaskActive(kind,active){
      if(active)activeTasks.add(kind);
      else activeTasks.delete(kind);
      ensure();
    },
    fetch(...args){
      const end=begin();
      try{return window.fetch(...args).finally(end);}
      catch(error){end();throw error;}
    }
  };
})();
// Minimal browser adapters for the canonical Rust-compiled WASM UI state.
// Browser APIs (DOM, fetch, storage) must remain JS: user-visible decisions,
// navigation validation, loading epochs and locale values come from WASM.
(()=>{
  const tabs=["preview","analysis","players","montage","quick","report"];
  const $=id=>document.getElementById(id);
  const KEY="volley.rust.ui.preferences.v1";
  const KEYS={tab:0,locale:1,theme:2,density:3,reduced_motion:4};
  const textNodes=new WeakMap();
  const localizedAttributes=new WeakMap();
  const safeStorage={
    read(){
      try{
        const raw=localStorage.getItem(KEY);
        if(!raw||raw.length>4096)return {};
        const object=JSON.parse(raw);
        if(!object||Array.isArray(object)||typeof object!=="object")return {};
        return object;
      }catch{return {};}
    },
    write(value){
      try{localStorage.setItem(KEY,JSON.stringify(value));}catch{/* private mode */}
    }
  };
  let wasm=null,words=[],translation=new Map(),reverse=new Map(),observer=null;
  let scheduled=false,suspend=false,initialized=false;
  const pendingRoots=new Set();
  const output={
    tabs,ready:null,
    get engine(){if(!wasm)throw Error("Rust/WASM module is not ready");return wasm;},
    get(name){
      if(!wasm||!(name in KEYS))return -1;
      return wasm.volley_ui_get(KEYS[name]);
    },
    navigate(name){
      if(!wasm)throw Error("Studio UI module is not loaded");
      const index=tabs.indexOf(name);
      if(wasm.volley_ui_set(KEYS.tab,index<0?0:index)!==1){
        throw Error("Invalid studio navigation");
      }
      return tabs[wasm.volley_ui_get(KEYS.tab)];
    },
    beginLoad(tab){
      if(!wasm)throw Error("WASM UI is unavailable");
      const index=tabs.indexOf(tab);
      return index<0?0:wasm.volley_ui_begin_load(index);
    },
    currentLoad(tab,token){
      if(!wasm)return false;
      const index=tabs.indexOf(tab);
      return index>=0&&wasm.volley_ui_current_load(index,token)===1;
    },
    set(name,value){
      if(!wasm||!(name in KEYS)||name==="tab")return false;
      if(!Number.isInteger(value)||wasm.volley_ui_set(KEYS[name],value)!==1)return false;
      save();apply();return true;
    },
    text(index){
      if(!wasm||wasm.volley_ui_text_copy(index)!==1)return "";
      const ptr=wasm.volley_ui_text_ptr(),len=wasm.volley_ui_text_len();
      if(!len||len>512||ptr+len>wasm.memory.buffer.byteLength)return "";
      return new TextDecoder().decode(new Uint8Array(wasm.memory.buffer,ptr,len));
    },
    translate(value){
      const string=String(value),trimmed=string.trim();
      if(!trimmed)return string;
      const replacement=(this.get("locale")===1?translation:reverse).get(trimmed);
      if(!replacement)return string;
      const front=string.match(/^\s*/u)?.[0]||"";
      const end=string.match(/\s*$/u)?.[0]||"";
      return front+replacement+end;
    },
    isReady:()=>initialized
  };
  window.VolleyballUi=output;
  function savedValues(){
    return {locale:output.get("locale"),theme:output.get("theme"),
      density:output.get("density"),reduced_motion:output.get("reduced_motion")};
  }
  function save(){safeStorage.write(savedValues());}
  function sourceFor(node){
    const raw=node.nodeValue;
    const old=textNodes.get(node);
    if(old&&raw===old.rendered)return old.original;
    return raw;
  }
  function updateText(node){
    const raw=sourceFor(node);
    if(!raw||!raw.trim())return;
    const trimmed=raw.trim();
    const decorated=trimmed.match(/^(?:(?:↻|↗|‹|›)\s*|\d+[.]\s+)(.+)$/u);
    const phrase=translation.has(trimmed)?trimmed:decorated?.[1];
    if(!phrase||!translation.has(phrase))return;
    // Preserve the original Russian text across language changes, even
    // when the browser script rewrites a dynamic status message.
    const offset=raw.indexOf(phrase);
    const before=raw.slice(0,offset),after=raw.slice(offset+phrase.length);
    const outputText=output.get("locale")===1?translation.get(phrase):phrase;
    const rendered=before+outputText+after;
    textNodes.set(node,{original:raw,rendered});
    if(node.nodeValue!==rendered)node.nodeValue=rendered;
  }
  function updateAttribute(node,name){
    const raw=node.getAttribute(name);
    if(raw===null)return;
    const previous=localizedAttributes.get(node)||{};
    const base=raw===previous[name]?.rendered?previous[name].original:raw;
    const result=(output.get("locale")===1?translation:reverse).get(base)||base;
    previous[name]={original:base,rendered:result};
    localizedAttributes.set(node,previous);
    if(raw!==result)node.setAttribute(name,result);
  }
  function walk(root){
    if(!root||!initialized)return;
    const tag=root.nodeType===1?root.tagName.toLowerCase():"";
    if(["script","style","code","pre","textarea","option"].includes(tag)&&
        !["option"].includes(tag))return;
    if(root.nodeType===3){updateText(root);return;}
    if(root.nodeType!==1&&root.nodeType!==9&&root.nodeType!==11)return;
    if(root.nodeType===1){
      for(const attribute of ["aria-label","title","placeholder"])
        updateAttribute(root,attribute);
    }
    const walker=document.createTreeWalker(root,NodeFilter.SHOW_TEXT,{
      acceptNode(node){
        const parent=node.parentElement;
        if(parent&&["SCRIPT","STYLE","TEXTAREA","CODE","PRE"].includes(parent.tagName))
          return NodeFilter.FILTER_REJECT;
        return NodeFilter.FILTER_ACCEPT;
      }
    });
    while(walker.nextNode())updateText(walker.currentNode);
    if(root.querySelectorAll){
      for(const element of root.querySelectorAll("[aria-label],[title],[placeholder]")){
        for(const attribute of ["aria-label","title","placeholder"])
          updateAttribute(element,attribute);
      }
    }
  }
  function queueTranslation(root=document.body){
    if(!initialized||!root)return;
    pendingRoots.add(root);
    if(scheduled)return;
    scheduled=true;
    queueMicrotask(()=>{
      scheduled=false;
      suspend=true;
      const pending=[...pendingRoots];
      pendingRoots.clear();
      try{
        if(pending.includes(document.body)){walk(document.body);}
        else for(const subtree of pending)walk(subtree);
      }finally{suspend=false;}
    });
  }
  function apply(){
    if(!wasm)return;
    const locale=output.get("locale"),theme=output.get("theme"),
      density=output.get("density"),motion=output.get("reduced_motion");
    document.documentElement.lang=locale===1?"en":"ru";
    document.body.dataset.uiTheme=theme===1?"light":"dark";
    document.body.dataset.uiDensity=density===1?"compact":"normal";
    document.body.dataset.uiMotion=motion===1?"reduce":"normal";
    queueTranslation();
    window.dispatchEvent(new CustomEvent("volley:ui-language",{
      detail:{locale:locale===1?"en":"ru",theme:theme===1?"light":"dark"}
    }));
  }
  function buildWordMaps(){
    const count=wasm.volley_ui_text_count();
    if(count<80||count>500)throw Error("Unexpected UI translation catalogue");
    const previous=wasm.volley_ui_get(KEYS.locale);
    for(let i=0;i<count;i++){
      wasm.volley_ui_set(KEYS.locale,0);
      const ru=output.text(i);
      wasm.volley_ui_set(KEYS.locale,1);
      const en=output.text(i);
      if(!ru||!en||translation.has(ru))
        throw Error("Duplicate or invalid UI translation "+i);
      words.push([ru,en]);translation.set(ru,en);
      if(!reverse.has(en))reverse.set(en,ru);
    }
    wasm.volley_ui_set(KEYS.locale,previous);
  }
  async function boot(){
    const response=await window.VolleyballStudioActivity.fetch("/studio/selection.wasm",{
      cache:"no-store",credentials:"same-origin"
    });
    if(!response.ok)throw Error("Studio interface module unavailable ("+response.status+")");
    const raw=await response.arrayBuffer();
    if(raw.byteLength<1024||raw.byteLength>1048576)
      throw Error("Unsafe interface module size");
    const {instance}=await WebAssembly.instantiate(raw,{});
    const exports=instance.exports;
    for(const key of ["volley_ui_api_version","volley_ui_tab_count",
        "volley_ui_get","volley_ui_set","volley_ui_text_copy",
        "volley_ui_text_ptr","volley_ui_text_len","volley_ui_begin_load",
        "volley_ui_current_load"]){
      if(typeof exports[key]!=="function")throw Error("Missing interface module export "+key);
    }
    if(exports.volley_ui_api_version()!==1||exports.volley_ui_tab_count()!==6)
      throw Error("Unsupported interface module contract");
    wasm=exports;
    window.VolleyStudioChain?.setRuntime(exports);
    buildWordMaps();
    const stored=safeStorage.read();
    for(const name of ["locale","theme","density","reduced_motion"]){
      const value=stored[name];
      if(Number.isInteger(value))wasm.volley_ui_set(KEYS[name],value);
    }
    initialized=true;
    observer=new MutationObserver(records=>{
      if(suspend||output.get("locale")!==1)return;
      for(const record of records){
        if(record.type==="characterData")queueTranslation(record.target);
        else if(record.type==="childList"){
          for(const added of record.addedNodes){
            if(added.nodeType===1||added.nodeType===3)queueTranslation(added);
          }
        }
      }
    });    observer.observe(document.body,{subtree:true,childList:true,characterData:true});
    apply();
    return output;
  }
  output.ready=boot().catch(error=>{
    initialized=false;
    console.error("Studio UI initialization failed:",error);
    const message=$("studio-banner");
    if(message){
      message.textContent="Интерфейс студии недоступен: "+error.message;
      message.className="banner error";
    }
    throw error;
  });
})();
