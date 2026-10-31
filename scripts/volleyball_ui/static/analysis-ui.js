"use strict";
/* Analysis workspace deliberately has NO results/events rendering. */
window.VolleyballAnalysisUI=(()=>{
  const E=id=>document.getElementById(id);
  const player=()=>E("source-video-player");
  const clamp=(v,min,max)=>Math.max(min,Math.min(max,v));
  const stamp=s=>{
    s=Math.max(0,Math.round(s));
    return [Math.floor(s/3600),Math.floor(s%3600/60),s%60].map(x=>String(x).padStart(2,"0")).join(":");
  };
  let meta=null,video="",projectId="",start=0,end=0,current=0,drag=null,loadSeq=0;
  const ranges=new Map();
  const pct=t=>meta?.duration_sec?100*clamp(t,0,meta.duration_sec)/meta.duration_sec:0;
  const width=()=>E("manual-width-enabled").checked?Number(E("analysis-width").value):meta?.auto_width;

  function renderRange(){
    if(!meta)return;
    const left=pct(start),right=pct(end);
    E("timeline-selected-range").style.left=left+"%";
    E("timeline-selected-range").style.width=Math.max(0,right-left)+"%";
    E("timeline-start-handle").style.left=left+"%";
    E("timeline-end-handle").style.left=right+"%";
    E("timeline-playhead").style.left=pct(current)+"%";
    E("timeline-start-input").value=String(Math.round(start));
    E("timeline-end-input").value=String(Math.round(end));
    E("timeline-range-label").textContent=stamp(start)+" — "+stamp(end)+" · "+stamp(end-start);
    E("timeline-status").textContent="Время просмотра "+stamp(current);
    ranges.set(video,{start,end});
  }
  function showWidth(){
    if(!meta)return;
    const w=width();
    if(!Number.isInteger(w)||w<320||w>1920||w%2){
      E("resolution-explanation").textContent="Укажите чётную ширину от 320 до 1920 пикселей.";
      return;
    }
    const h=Math.max(2,2*Math.round(w*meta.height/meta.width/2));
    E("auto-size-value").textContent=meta.auto_width+" × "+meta.auto_height+" px";
    E("resolution-explanation").textContent="Источник: "+meta.width+"×"+meta.height+
      " → перед нейросетью: "+w+"×"+h+" пикселей. "+(w>meta.width?
        "Масштабирование вверх не добавляет деталей.":"Высота сохраняет пропорции.");
  }
  function markUnsaved(){
    E("range-save-status").textContent="Изменено — сохраните диапазон";
  }
  function boundary(side,value){
    if(!meta||!Number.isFinite(Number(value)))return;
    const t=Number(value);
    if(side==="start")start=clamp(t,0,Math.max(0,end-1));
    else end=clamp(t,Math.min(start+1,meta.duration_sec),meta.duration_sec);
    renderRange();markUnsaved();
  }
  function seek(t){
    if(!meta)return;
    current=clamp(Number(t)||0,0,Math.max(0,meta.duration_sec-.05));
    const v=player();
    if(v.src) {
      try{v.currentTime=current}catch(_){}
    }
    renderRange();
  }
  async function setVideo(name){
    if(!name)return;
    const request=++loadSeq;
    if(meta&&video)ranges.set(video,{start,end});
    video=name;meta=null;current=0;
    const v=player();
    v.pause();v.removeAttribute("src");v.load();
    E("source-video-placeholder").hidden=false;
    E("source-video-placeholder").textContent="Открываем видео…";
    E("preview-source-details").textContent="Читаем параметры файла…";
    E("source-playback-note").textContent="Шкала позволяет выделить часть для анализа.";
    try{
      const get=async(path)=>{
        const resp=await fetch(path,{cache:"no-store"});
        const data=await resp.json();
        if(!resp.ok)throw Error(data.error||"Ошибка доступа к видео");
        return data;
      };
      const q="project_id="+encodeURIComponent(projectId)+"&video="+encodeURIComponent(name);
      const [info,playback]=await Promise.all([
        get("/api/video-info?video="+encodeURIComponent(name)),
        get("/api/video-view?"+q),
      ]);
      if(request!==loadSeq)return;
      meta=info;
      const saved=ranges.get(name);
      start=saved?clamp(saved.start,0,Math.max(0,info.duration_sec-1)):0;
      end=saved?clamp(saved.end,start+1,info.duration_sec):info.duration_sec;
      E("timeline-start-input").max=info.duration_sec;
      E("timeline-end-input").max=info.duration_sec;
      E("preview-source-details").textContent=name+" · "+info.width+"×"+info.height+" · "+stamp(info.duration_sec);
      E("timeline-ticks").innerHTML=[0,.25,.5,.75,1].map(n=>"<span>"+stamp(n*info.duration_sec)+"</span>").join("");
      const url=playback.preview_url;
      v.poster=playback.poster_url;
      v.src=url;
      v.load();
      // Show an actual frame instead of a black player on first opening.
      v.addEventListener("loadedmetadata",()=>{
        if(request===loadSeq && v.currentTime<.2 && meta)
          v.currentTime=Math.min(Math.max(start,2),Math.max(0,meta.duration_sec-.2));
      },{once:true});
      E("source-video-placeholder").hidden=true;
      E("source-playback-note").textContent=playback.browser_preview==="normalized"
        ?"Воспроизводится совместимая H.264-копия исходного матча. Исходное видео не изменяется."
        :"Воспроизводится оригинал. Если браузер не поддерживает кодек, используйте предпросмотр кадра или установите системный декодер.";
      current=clamp(start,0,info.duration_sec);
      showWidth();renderRange();
      E("range-save-status").textContent="Диапазон загружен из проекта";
    }catch(err){
      if(request!==loadSeq)return;
      E("source-video-placeholder").hidden=false;
      E("source-video-placeholder").textContent="Не удалось открыть видео";
      E("preview-source-details").textContent=err.message;
    }
  }
  function init(){
    E("manual-width-enabled").addEventListener("change",e=>{
      E("analysis-width").disabled=!e.target.checked;showWidth();
    });
    E("analysis-width").addEventListener("change",showWidth);
    E("timeline-reset").addEventListener("click",()=>{
      if(!meta)return;start=0;end=meta.duration_sec;renderRange();markUnsaved();
    });
    E("timeline-start-input").addEventListener("change",e=>boundary("start",e.target.value));
    E("timeline-end-input").addEventListener("change",e=>boundary("end",e.target.value));
    const timebar=E("match-timeline");
    const position=e=>{
      const box=timebar.getBoundingClientRect();
      return clamp((e.clientX-box.left)/box.width,0,1)*meta.duration_sec;
    };
    timebar.addEventListener("click",e=>{
      if(meta&&!e.target.closest(".timeline-handle"))seek(position(e));
    });
    for(const [side,id] of [["start","timeline-start-handle"],["end","timeline-end-handle"]]){
      const h=E(id);
      h.addEventListener("pointerdown",e=>{
        if(!meta)return;
        e.preventDefault();e.stopPropagation();
        drag=side;h.setPointerCapture(e.pointerId);
      });
      h.addEventListener("pointermove",e=>{if(drag===side)boundary(side,position(e))});
      for(const action of ["pointerup","pointercancel","lostpointercapture"])
        h.addEventListener(action,()=>{if(drag===side)drag=null});
      h.addEventListener("keydown",e=>{
        if(!meta||!["ArrowLeft","ArrowRight"].includes(e.key))return;
        e.preventDefault();
        boundary(side,(side==="start"?start:end)+(e.key==="ArrowLeft"?-1:1));
      });
    }
    player().addEventListener("timeupdate",()=>{
      if(meta){current=player().currentTime;renderRange();}
    });
    player().addEventListener("error",()=>{
      if(meta)E("source-playback-note").textContent=
        "Браузер не смог воспроизвести кодек видео. Можно продолжать настройку диапазона по времени; для просмотра необходима совместимая H.264-копия.";
    });
  }
  function setProjectRanges(saved={},identifier=""){
    ++loadSeq;
    projectId=identifier;video="";meta=null;ranges.clear();
    for(const [name,range] of Object.entries(saved||{})){
      if(range&&Number.isFinite(range.start)&&Number.isFinite(range.end))
        ranges.set(name,{start:range.start,end:range.end});
    }
  }
  function currentRange(){
    return meta?{start:Math.round(start*1000)/1000,end:Math.round(end*1000)/1000}:null;
  }
  function params(){
    if(!meta)throw Error("Дождитесь загрузки видео");
    const w=width();
    if(!Number.isInteger(w)||w<320||w>1920||w%2)throw Error("Некорректная ширина кадра");
    return {analysis_width:E("manual-width-enabled").checked?w:"auto"};
  }
  return {init,setVideo,setProjectRanges,currentRange,params};
})();
