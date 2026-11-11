"use strict";
(()=>{
  async function digest(file,onProgress=()=>{},isCancelled=()=>false){
    await window.VolleyballUi.ready;
    const e=window.VolleyballUi.engine;
    const handle=e.volley_hash_begin();
    if(!handle)throw Error("Слишком много одновременных вычислений SHA-256");
    try{
      const chunk=4*1024*1024;
      for(let offset=0;offset<file.size;offset+=chunk){
        if(isCancelled())throw Error("Вычисление хеша отменено");
        const bytes=new Uint8Array(await file.slice(offset,offset+chunk).arrayBuffer());
        if(isCancelled())throw Error("Вычисление хеша отменено");
        const ptr=e.volley_data_reserve(bytes.length);
        if(!ptr)throw Error("Не удалось выделить буфер SHA-256");
        new Uint8Array(e.memory.buffer,ptr,bytes.length).set(bytes);
        if(e.volley_hash_update(handle)!==1)throw Error("Ошибка SHA-256 Rust/WASM");
        onProgress(Math.min(100,Math.floor((offset+bytes.length)/file.size*100)));
      }
      if(isCancelled())throw Error("Вычисление хеша отменено");
      if(e.volley_hash_finish(handle)!==1)throw Error("Ошибка SHA-256 Rust/WASM");
      return new TextDecoder().decode(new Uint8Array(e.memory.buffer,
        e.volley_data_output_ptr(),e.volley_data_output_len()));
    }finally{e.volley_hash_abort(handle);}
  }
  window.VolleyballSha256={digest};
})();
