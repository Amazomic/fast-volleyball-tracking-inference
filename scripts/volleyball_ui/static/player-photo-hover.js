/* Lazy large multi-frame inspection for one RAVEL Track ID.
 *
 * The list holds only small thumbnails. Metadata and enlarged crops are
 * loaded on hover/focus, scoped to the exact project, video and neural cache.
 * Hover never selects, groups, or assigns a real player identity.
 */
(function(){
  "use strict";

  const SHOW_DELAY_MS=90;
  const HIDE_DELAY_MS=160;
  const MAX_CACHED_TRACKS=80;
  const SELECTOR=".photo-peek,[data-photo-track-id]";

  let activeController=null;
  function init(root, getContext){
    if(!root||typeof getContext!=="function")return null;
    const panel=document.createElement("aside");
    panel.className="player-track-hover-preview";
    panel.id="player-track-hover-preview";
    panel.setAttribute("role","tooltip");
    panel.setAttribute("aria-label","Увеличенные кадры Track ID");
    panel.hidden=true;
    document.body.appendChild(panel);

    const cache=new Map();
    let anchor=null,activeKey="",requestToken=0,showTimer=null,hideTimer=null;

    function clearTimers(){
      clearTimeout(showTimer);
      clearTimeout(hideTimer);
      showTimer=hideTimer=null;
    }
    function getTarget(event){
      const node=event.target;
      if(!node||typeof node.closest!=="function")return null;
      const target=node.closest(SELECTOR);
      return target&&root.contains(target)?target:null;
    }
    function getId(target){
      return target?.dataset?.photoTrackId||target?.dataset?.id||"";
    }
    function current(){
      const ctx=getContext();
      if(!ctx||!ctx.project||!ctx.video||!ctx.cache)return null;
      return {project:String(ctx.project),video:String(ctx.video),
              cache:String(ctx.cache).split(".",1)[0]};
    }
    function keyFor(id,ctx){
      return [ctx.project,ctx.video,ctx.cache,id].join("\u0000");
    }
    function photoUrl(id,index,ctx){
      return "/api/player-photo?"+new URLSearchParams({
        project_id:ctx.project,video:ctx.video,cache_id:ctx.cache,
        id,sample:String(index)
      });
    }
    function metadataUrl(id,ctx){
      return "/api/player-photos?"+new URLSearchParams({
        project_id:ctx.project,video:ctx.video,cache_id:ctx.cache,id
      });
    }
    function element(tag,cls,text){
      const node=document.createElement(tag);
      if(cls)node.className=cls;
      if(text!==undefined)node.textContent=text;
      return node;
    }
    function place(){
      if(!anchor||panel.hidden)return;
      const a=anchor.getBoundingClientRect();
      const viewportWidth=window.innerWidth||1200;
      const viewportHeight=window.innerHeight||800;
      const panelWidth=Math.min(736,viewportWidth-24);
      panel.style.width=panelWidth+"px";
      const panelHeight=panel.getBoundingClientRect().height;
      let left=a.right+12;
      if(left+panelWidth>viewportWidth-12)left=a.left-panelWidth-12;
      if(left<12)left=Math.max(12,Math.min(a.left,viewportWidth-panelWidth-12));
      const top=Math.max(12,Math.min(a.top-12,viewportHeight-panelHeight-12));
      panel.style.left=left+"px";
      panel.style.top=top+"px";
    }
    function showSkeleton(id){
      panel.replaceChildren(
        element("strong","player-track-hover-title","Track ID #"+id),
        element("p","player-track-hover-message","Загрузка дополнительных кадров…")
      );
      panel.hidden=false;
      place();
    }
    function render(id,ctx,metadata){
      const samples=Array.isArray(metadata?.samples)
        ?metadata.samples.filter(s=>Number.isInteger(s.index)&&s.index>=0&&s.index<10)
          .slice(0,6):[];
      const title=element("div","player-track-hover-heading");
      title.appendChild(element("strong","player-track-hover-title","Track ID #"+id));
      title.appendChild(element("small","player-track-hover-count",
        samples.length+" кадр(а) · сохранённые обнаружения RAVEL"));
      if(!samples.length){
        panel.replaceChildren(title,element("p","player-track-hover-message",
          "Для этого Track ID дополнительные фотографии отсутствуют."));
        place();
        return;
      }
      const strip=element("div","player-track-hover-images");
      for(const sample of samples){
        const photo=element("figure","player-track-hover-photo");
        const img=document.createElement("img");
        img.src=photoUrl(id,sample.index,ctx);
        img.alt="Track ID #"+id+", кадр "+String(sample.frame??"—");
        img.loading="eager";
        img.decoding="async";
        img.addEventListener("error",()=>{
          photo.classList.add("is-unavailable");
          img.alt="Кадр недоступен";
        });
        const time=Number(sample.time_sec);
        const score=Number(sample.confidence);
        const label=element("figcaption","",
          (Number.isFinite(time)?time.toFixed(1)+" с":"Кадр "+String(sample.frame??"—"))+
          (Number.isFinite(score)?" · "+Math.round(score*100)+"%":""));
        photo.append(img,label);
        strip.appendChild(photo);
      }
      panel.replaceChildren(title,strip,
        element("small","player-track-hover-hint",
          "Кадры одного технического ID. Это не подтверждение личности."));
      place();
    }
    function dismiss(){
      clearTimers();
      requestToken++;
      if(anchor)anchor.removeAttribute("aria-describedby");
      anchor=null;activeKey="";
      panel.hidden=true;
    }
    function scheduleDismiss(){
      clearTimeout(hideTimer);
      hideTimer=setTimeout(dismiss,HIDE_DELAY_MS);
    }
    async function show(trigger){
      const ctx=current(),id=getId(trigger);
      if(!ctx||!/^[0-9]{1,12}$/.test(id))return;
      const key=keyFor(id,ctx);
      clearTimers();
      if(anchor&&anchor!==trigger)anchor.removeAttribute("aria-describedby");
      anchor=trigger;
      anchor.setAttribute("aria-describedby",panel.id);
      const token=++requestToken;
      activeKey=key;
      showSkeleton(id);
      try{
        let meta=cache.get(key);
        if(!meta){
          const response=await fetch(metadataUrl(id,ctx),{cache:"default"});
          if(!response.ok)throw Error("HTTP "+response.status);
          meta=await response.json();
          cache.set(key,meta);
          if(cache.size>MAX_CACHED_TRACKS)cache.delete(cache.keys().next().value);
        }
        const latest=current();
        if(token!==requestToken||activeKey!==key||!latest||
           keyFor(id,latest)!==key||panel.hidden||!root.contains(trigger))return;
        render(id,ctx,meta);
      }catch(error){
        if(token!==requestToken||panel.hidden)return;
        panel.replaceChildren(
          element("strong","player-track-hover-title","Track ID #"+id),
          element("p","player-track-hover-message",
            "Не удалось загрузить фотографии: "+error.message)
        );
        place();
      }
    }
    function scheduleShow(target){
      clearTimeout(showTimer);
      clearTimeout(hideTimer);
      if(anchor===target&&!panel.hidden)return;
      // The pointer can traverse many rows quickly: only the last triggers
      // a request, rather than decoding crops for every passed-over row.
      showTimer=setTimeout(()=>{showTimer=null;show(target);},SHOW_DELAY_MS);
    }
    root.addEventListener("pointerover",event=>{
      const target=getTarget(event);
      if(!target||target.contains(event.relatedTarget))return;
      scheduleShow(target);
    });
    root.addEventListener("pointerout",event=>{
      const target=getTarget(event);
      if(!target||target.contains(event.relatedTarget))return;
      clearTimeout(showTimer);
      if(anchor===target)scheduleDismiss();
    });
    root.addEventListener("focusin",event=>{
      const target=getTarget(event);
      if(target)scheduleShow(target);
    });
    root.addEventListener("focusout",event=>{
      const target=getTarget(event);
      if(target&&!target.contains(event.relatedTarget))scheduleDismiss();
    });
    root.addEventListener("click",event=>{
      if(getTarget(event))dismiss();
    });
    panel.addEventListener("pointerenter",()=>clearTimeout(hideTimer));
    panel.addEventListener("pointerleave",scheduleDismiss);
    panel.addEventListener("focusin",()=>clearTimeout(hideTimer));
    panel.addEventListener("focusout",scheduleDismiss);
    document.addEventListener("keydown",event=>{
      if(event.key==="Escape")dismiss();
    });
    document.addEventListener("scroll",event=>{
      if(panel.hidden||panel.contains(event.target))return;
      dismiss();
    },true);
    window.addEventListener("resize",place);
    activeController={dismiss};
    return activeController;
  }
  window.VolleyballPlayerPhotoHover={
    init,
    dismiss(){activeController?.dismiss();}
  };
})();
