"use strict";
// Minimal browser adapters for the canonical Rust-compiled WASM UI state.
// Browser APIs (DOM, fetch, storage) must remain JS: user-visible decisions,
// navigation validation, loading epochs and locale values come from WASM.
(()=>{
  const tabs=["preview","analysis","players","montage","quick","legacy"];
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
    },
    clear(){try{localStorage.removeItem(KEY);}catch{/* private mode */}}
  };
  let wasm=null,words=[],translation=new Map(),reverse=new Map(),observer=null;
  let scheduled=false,suspend=false,initialized=false;
  const pendingRoots=new Set();
  const output={
    tabs,ready:null,
    get(name){
      if(!wasm||!(name in KEYS))return -1;
      return wasm.volley_ui_get(KEYS[name]);
    },
    navigate(name){
      if(!wasm)throw Error("Rust/WASM UI is not loaded");
      const index=tabs.indexOf(name);
      if(wasm.volley_ui_set(KEYS.tab,index<0?0:index)!==1){
        throw Error("Invalid Rust UI navigation");
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
    for(const name of ["locale","theme","density","reduced_motion"]){
      const control=$("studio-ui-"+name.replace("_","-"));
      if(control)control.value=String(output.get(name));
    }
    const heading=$("studio-ui-settings-label");
    if(heading)heading.textContent=output.text(6);
    const summary=$("studio-ui-settings-summary");
    if(summary)summary.setAttribute("aria-label",output.text(6));
    queueTranslation();
    window.dispatchEvent(new CustomEvent("volley:ui-language",{
      detail:{locale:locale===1?"en":"ru",theme:theme===1?"light":"dark"}
    }));
  }
  function buildWordMaps(){
    const count=wasm.volley_ui_text_count();
    if(count<80||count>500)throw Error("Unexpected Rust/WASM translation catalogue");
    const previous=wasm.volley_ui_get(KEYS.locale);
    for(let i=0;i<count;i++){
      wasm.volley_ui_set(KEYS.locale,0);
      const ru=output.text(i);
      wasm.volley_ui_set(KEYS.locale,1);
      const en=output.text(i);
      if(!ru||!en||translation.has(ru))
        throw Error("Duplicate or invalid Rust translation "+i);
      words.push([ru,en]);translation.set(ru,en);
      if(!reverse.has(en))reverse.set(en,ru);
    }
    wasm.volley_ui_set(KEYS.locale,previous);
  }
  async function boot(){
    const response=await fetch("/studio/rust/selection.wasm",{
      cache:"no-store",credentials:"same-origin"
    });
    if(!response.ok)throw Error("Rust/WASM binary unavailable ("+response.status+")");
    const raw=await response.arrayBuffer();
    if(raw.byteLength<1024||raw.byteLength>1048576)
      throw Error("Unsafe Rust/WASM binary size");
    const {instance}=await WebAssembly.instantiate(raw,{});
    const exports=instance.exports;
    for(const key of ["volley_ui_api_version","volley_ui_tab_count",
        "volley_ui_get","volley_ui_set","volley_ui_text_copy",
        "volley_ui_text_ptr","volley_ui_text_len","volley_ui_begin_load",
        "volley_ui_current_load"]){
      if(typeof exports[key]!=="function")throw Error("Missing Rust/WASM export "+key);
    }
    if(exports.volley_ui_api_version()!==1||exports.volley_ui_tab_count()!==6)
      throw Error("Unsupported Rust/WASM UI contract");
    wasm=exports;
    buildWordMaps();
    const stored=safeStorage.read();
    for(const name of ["locale","theme","density","reduced_motion"]){
      const value=stored[name];
      if(Number.isInteger(value))wasm.volley_ui_set(KEYS[name],value);
    }
    initialized=true;
    for(const name of ["locale","theme","density","reduced_motion"]){
      const input=$("studio-ui-"+name.replace("_","-"));
      input?.addEventListener("change",()=>{
        if(!output.set(name,Number(input.value)))input.value=String(output.get(name));
      });
    }
    $("studio-ui-reset")?.addEventListener("click",()=>{
      for(const name of ["locale","theme","density","reduced_motion"])
        wasm.volley_ui_set(KEYS[name],0);
      safeStorage.clear();apply();
    });
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
    console.error("Rust/WASM UI initialization failed:",error);
    const message=$("studio-banner");
    if(message){
      message.textContent="Rust/WASM интерфейс недоступен: "+error.message;
      message.className="banner error";
    }
    throw error;
  });
})();
