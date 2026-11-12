"use strict";
((root)=>{
  let engine=null;
  const stamp=seconds=>Number.isFinite(Number(seconds))&&Number(seconds)>0?
    new Date(Number(seconds)*1000).toLocaleString("ru-RU",{
      day:"2-digit",month:"2-digit",year:"2-digit",hour:"2-digit",minute:"2-digit"
    }):"";
  const key=(video,kind,run)=>JSON.stringify([video,kind,run]);
  const call=(operation,value)=>{
    if(!engine&&root?.VolleyballUi?.isReady())engine=root.VolleyballUi.engine;
    if(!engine)throw Error("Rust/WASM module is not ready");
    const bytes=new TextEncoder().encode(JSON.stringify(value));
    const ptr=engine.volley_data_reserve(bytes.length);
    if(!ptr)throw Error("Данные превышают лимит Rust/WASM");
    new Uint8Array(engine.memory.buffer,ptr,bytes.length).set(bytes);
    if(engine.volley_domain_call(operation)!==1)throw Error("Rust/WASM отклонил данные");
    return JSON.parse(new TextDecoder().decode(new Uint8Array(engine.memory.buffer,
      engine.volley_data_output_ptr(),engine.volley_data_output_len())));
  };
  const build=(videos,overviews,runs)=>{
    const result=call(1,{videos,overviews:Object.fromEntries(overviews||[]),runs});
    for(const choice of result.choices){
      const stage=(overviews?.get(choice.video)?.stages||[]).find(row=>row.key===(choice.kind==="unlinked"?"tracking":choice.kind));
      const saved=stage?.items?.find(row=>row.id===choice.id)?.display_name||
        (choice.kind==="unlinked"?(result.byAnalysis[choice.value]||[])[0]?.unlinked_display_name:null);
      if(saved)choice.display_name=saved;
      const date=stamp(choice.created_at)||"Без даты";
      choice.label=date+" · "+choice.display_name+
        (choice.kind==="unlinked"?" · GPU-источник не установлен":
          choice.kind==="gpu"?" · не объединён":"");
    }
    result.byAnalysis=new Map(Object.entries(result.byAnalysis));
    result.runToAnalysis=new Map(Object.entries(result.runToAnalysis));
    return result;
  };
  const api={build,key,stamp,call,setRuntime:exports=>{engine=exports;}};
  if(root)root.VolleyStudioChain=api;
  if(typeof module==="object"&&module.exports)module.exports=api;
})(typeof window!=="undefined"?window:null);
