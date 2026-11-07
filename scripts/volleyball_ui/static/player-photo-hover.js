/* Lazy per-track image cycling: no hover popups or broken-image flashes.
 * Show the next photo only after it has been downloaded and decoded.
 */
(function(){
  "use strict";
  const SELECTOR=".photo-peek,[data-photo-track-id]";
  const MAX_CACHED_TRACKS=80;
  const MAX_SAMPLES=6;
  let activeController=null;

  function init(root,getContext){
    if(!root||typeof getContext!=="function")return null;
    if(activeController?.root===root)return activeController;
    const metadataCache=new Map();
    const imageStates=new WeakMap();
    let generation=0;

    function findTarget(event){
      const node=event.target;
      if(!node||typeof node.closest!=="function")return null;
      const target=node.closest(SELECTOR);
      return target&&root.contains(target)?target:null;
    }
    function contextFor(id){
      const source=getContext(id);
      if(!source||!source.project||!source.video||!source.cache)return null;
      const track=String(source.id||id||"");
      if(!/^[0-9]{1,12}$/.test(track))return null;
      return {project:String(source.project),video:String(source.video),
        cache:String(source.cache).split(".",1)[0],id:track};
    }
    function keyFor(ctx){
      return JSON.stringify([ctx.project,ctx.video,ctx.cache,ctx.id]);
    }
    function urlFor(ctx,route,sample){
      const params=new URLSearchParams({
        project_id:ctx.project,video:ctx.video,cache_id:ctx.cache,id:ctx.id
      });
      if(sample!==undefined)params.set("sample",String(sample));
      return route+"?"+params;
    }
    function metadataFor(ctx,key){
      if(metadataCache.has(key))return metadataCache.get(key);
      const request=fetch(urlFor(ctx,"/api/player-photos"),{cache:"default"})
        .then(response=>{
          if(!response.ok)throw Error("HTTP "+response.status);
          return response.json();
        }).catch(error=>{metadataCache.delete(key);throw error;});
      metadataCache.set(key,request);
      if(metadataCache.size>MAX_CACHED_TRACKS)
        metadataCache.delete(metadataCache.keys().next().value);
      return request;
    }
    function preload(url){
      return new Promise((resolve,reject)=>{
        const candidate=new Image();
        candidate.decoding="async";
        candidate.onload=()=>{
          candidate.onload=candidate.onerror=null;
          resolve();
        };
        candidate.onerror=()=>{
          candidate.onload=candidate.onerror=null;
          reject(Error("Photo unavailable"));
        };
        candidate.src=url;
      });
    }
    function imageIndex(img){
      const direct=Number(img.dataset.photoSample);
      if(img.dataset.photoSample!==undefined&&Number.isInteger(direct)&&direct>0)
        return direct-1;
      const match=String(img.currentSrc||img.src||"").match(/[?&]sample=(\d+)/);
      return match?Number(match[1]):0;
    }
    function isCurrent(target,img,state,ctx,token){
      const now=contextFor(state.id);
      return token===generation&&root.contains(target)&&
        imageStates.get(img)===state&&now&&keyFor(now)===state.key;
    }
    function setBusy(target,img,busy){
      target.classList.toggle("is-photo-loading",busy);
      if(target!==img)img.classList.toggle("is-photo-loading",busy);
    }
    async function cycle(target,img,state,ctx){
      if(state.busy)return;
      state.busy=true;
      const token=generation;
      setBusy(target,img,true);
      target.title="\u0417\u0430\u0433\u0440\u0443\u0436\u0430\u0435\u043c \u043a\u0430\u0434\u0440\u2026";
      try{
        const meta=await metadataFor(ctx,state.key);
        if(!isCurrent(target,img,state,ctx,token))return;
        const samples=Array.isArray(meta?.samples)
          ?meta.samples.filter(sample=>Number.isInteger(sample.index)&&
              sample.index>=0&&sample.index<MAX_SAMPLES).sort((a,b)=>a.index-b.index):[];
        if(samples.length<2){
          state.pending=0;
          target.title=samples.length
            ?"\u0414\u043b\u044f \u044d\u0442\u043e\u0433\u043e ID \u0435\u0441\u0442\u044c \u0442\u043e\u043b\u044c\u043a\u043e \u043e\u0434\u0438\u043d \u043a\u0430\u0434\u0440"
            :"\u0414\u043b\u044f \u044d\u0442\u043e\u0433\u043e ID \u043d\u0435\u0442 \u043a\u0430\u0434\u0440\u043e\u0432";
          return;
        }
        // Keep every click, including those made while metadata or JPEG loads.
        while(state.pending>0){
          const steps=state.pending;
          state.pending=0;
          const current=Math.max(0,samples.findIndex(sample=>sample.index===state.index));
          const wanted=(current+steps)%samples.length;
          if(wanted===current){
            target.title="\u041a\u0430\u0434\u0440 "+(current+1)+"/"+samples.length+
              " \u00b7 \u043d\u0430\u0436\u043c\u0438\u0442\u0435 \u0434\u043b\u044f \u0441\u043b\u0435\u0434\u0443\u044e\u0449\u0435\u0433\u043e";
            continue; // A complete cycle returns to the same frame.
          }
          let loaded=null;
          // Skip a damaged/unavailable sample instead of leaving a broken photo.
          for(let offset=0;offset<samples.length-1;offset++){
            const sample=samples[(wanted+offset)%samples.length];
            if(sample.index===state.index)continue;
            const url=urlFor(ctx,"/api/player-photo",sample.index);
            try{
              await preload(url);
              if(!isCurrent(target,img,state,ctx,token))return;
              loaded={sample,url,position:(wanted+offset)%samples.length};
              break;
            }catch{
              if(!isCurrent(target,img,state,ctx,token))return;
            }
          }
          if(!loaded){
            state.pending=0;
            target.title="\u0414\u0440\u0443\u0433\u0438\u0435 \u043a\u0430\u0434\u0440\u044b \u043f\u043e\u043a\u0430 \u043d\u0435\u0434\u043e\u0441\u0442\u0443\u043f\u043d\u044b";
            break;
          }
          img.src=loaded.url;
          img.style.opacity="";
          state.index=loaded.sample.index;
          img.dataset.photoSample=String(loaded.sample.index+1);
          const label="\u041a\u0430\u0434\u0440 "+(loaded.position+1)+"/"+samples.length+
            (Number.isFinite(Number(loaded.sample.time_sec))
              ?" \u00b7 "+Number(loaded.sample.time_sec).toFixed(1)+" \u0441":"")+
            " \u00b7 \u043d\u0430\u0436\u043c\u0438\u0442\u0435 \u0434\u043b\u044f \u0441\u043b\u0435\u0434\u0443\u044e\u0449\u0435\u0433\u043e";
          target.title=label;
          img.title=label;
          img.alt="Track ID #"+ctx.id+" \u00b7 "+label;
        }
      }catch(error){
        if(isCurrent(target,img,state,ctx,token)){
          state.pending=0;
          target.title="\u041a\u0430\u0434\u0440\u044b \u043d\u0435 \u0437\u0430\u0433\u0440\u0443\u0437\u0438\u043b\u0438\u0441\u044c \u2014 \u043d\u0430\u0436\u043c\u0438\u0442\u0435 \u0435\u0449\u0451 \u0440\u0430\u0437";
          console.warn("Track photos",error);
        }
      }finally{
        state.busy=false;
        if(imageStates.get(img)===state)setBusy(target,img,false);
      }
    }
    function advance(target){
      const id=target.dataset.photoTrackId||target.dataset.id;
      const img=target.tagName==="IMG"?target:target.querySelector("img");
      const ctx=contextFor(id);
      if(!img||!ctx)return;
      const key=keyFor(ctx);
      let state=imageStates.get(img);
      if(!state||state.key!==key){
        state={key,id,index:imageIndex(img),pending:0,busy:false};
        imageStates.set(img,state);
      }
      state.pending++;
      if(!state.busy)void cycle(target,img,state,ctx);
    }
    root.addEventListener("click",event=>{
      const target=findTarget(event);
      if(!target)return;
      if(target.tagName==="A")event.preventDefault();
      advance(target);
    });
    root.addEventListener("keydown",event=>{
      const target=findTarget(event);
      if(target?.tagName!=="IMG"||!["Enter"," "].includes(event.key))return;
      event.preventDefault();
      advance(target);
    });
    activeController={root,dismiss(){generation++;}};
    return activeController;
  }
  window.VolleyballPlayerPhotoHover={
    init,
    dismiss(){activeController?.dismiss();}
  };
})();