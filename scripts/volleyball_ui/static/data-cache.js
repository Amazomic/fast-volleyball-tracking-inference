/* Persist immutable, source-scoped analysis JSON across F5 without HTTP re-fetches.
   Live job status, project registry and analysis-run lists are intentionally NOT cached. */
window.VolleyDataCache=(()=>{
  "use strict";
  const STORE="volley-analysis-json-v2",MAX_ITEMS=60,MAX_BYTES=8*1024*1024;
  const pending=new Map();
  let epoch=0,clearing=Promise.resolve();
  function supports(url){
    if(typeof url!=="string"||!url.startsWith("/api/"))return false;
    const [route,query=""]=url.split("?",2),p=new URLSearchParams(query);
    if(["/api/analysis-view","/api/report","/api/analysis-rallies",
        "/api/analysis-player-boxes","/api/analysis-trajectory",
        "/api/analysis-ravel-ball","/api/analysis-model-touches"].includes(route))
      return !!(p.get("project_id")&&p.get("video")&&p.get("cache_id"));
    if(route==="/api/project-report")
      return !!(p.get("id")&&p.get("run_id"));
    return false;
  }
  async function open(){
    if(typeof caches==="undefined"||!caches.open)return null;
    try{await clearing;return await caches.open(STORE)}
    catch(error){console.warn("Volley cache unavailable:",error);return null}
  }
  async function get(url,{signal}={}){
    if(!supports(url))throw Error("Unsupported cached endpoint: "+url);
    if(signal?.aborted){const e=Error("Aborted");e.name="AbortError";throw e;}
    if(pending.has(url))return pending.get(url);
    const generation=epoch;
    const task=(async()=>{
      const store=await open();
      if(store&&generation===epoch){
        try{
          const hit=await store.match(url);
          if(hit&&generation===epoch)return await hit.json();
        }catch(error){console.warn("Volley cache read:",error)}
      }
      const response=await fetch(url,{cache:"no-store",...(signal?{signal}:{})});
      const length=Number(response.headers?.get("content-length"))||0;
      const backup=store&&response.ok&&generation===epoch&&
        length>0&&length<=MAX_BYTES?response.clone():null;
      const body=await response.json();
      if(!response.ok){
        const error=new Error(body.error||"Ошибка запроса");
        error.status=response.status;
        throw error;
      }
      if(backup&&generation===epoch&&body?.analyzed!==false&&body?.available!==false){
        // Rendering does not wait for disk writes. Failed/quota-full cache is optional.
        Promise.resolve().then(async()=>{
          if(generation!==epoch)return;
          await store.put(url,backup);
          const keys=await store.keys();
          for(const oldest of keys.slice(0,Math.max(0,keys.length-MAX_ITEMS)))
            await store.delete(oldest);
        }).catch(error=>console.warn("Volley cache write:",error));
      }
      return body;
    })().finally(()=>{if(pending.get(url)===task)pending.delete(url)});
    pending.set(url,task);
    return task;
  }
  function invalidate(){
    ++epoch;
    pending.clear();
    if(typeof caches!=="undefined"&&caches.delete)
      clearing=clearing.then(()=>caches.delete(STORE)).catch(error=>
        console.warn("Volley cache invalidate:",error));
    return clearing;
  }
  return {supports,get,invalidate};
})();