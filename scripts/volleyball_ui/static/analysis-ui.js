"use strict";
/* One unified timeline for source playback, scrubbing and analysis range selection. */
window.VolleyballAnalysisUI=(()=>{
  const E=id=>document.getElementById(id);
  const player=()=>E("source-video-player");
  const clamp=(value,min,max)=>Math.min(max,Math.max(min,value));
  const stamp=value=>{
    const seconds=Math.max(0,Math.floor(Number(value)||0));
    return [Math.floor(seconds/3600),Math.floor(seconds%3600/60),seconds%60]
      .map(n=>String(n).padStart(2,"0")).join(":");
  };
  let meta=null,video="",projectId="",start=0,end=0,current=0;
  let loadSeq=0,drag=null,scrubTimer=null;
  const ranges=new Map();
  const pct=t=>meta?.duration_sec?100*clamp(t,0,meta.duration_sec)/meta.duration_sec:0;
  const width=()=>E("manual-width-enabled").checked?Number(E("analysis-width").value):meta?.auto_width;
  function fitSourceFrame(){
    if(!meta)return;
    const main=document.querySelector(".workspace-video-main");
    if(!main)return;
    // Fit the actual display orientation, not the full horizontal workspace.
    const maxHeight=Math.min(window.innerHeight*.54,570);
    const fitWidth=maxHeight*meta.width/meta.height;
    main.style.setProperty("--source-display-width",Math.max(1,fitWidth)+"px");
    main.style.setProperty("--source-aspect",meta.width+"/"+meta.height);
  }

  function updatePlayButton(){
    const playing=!!(player().src&&!player().paused);
    const button=E("source-play-toggle");
    button.textContent=playing?"❚❚":"▶";
    button.setAttribute("aria-label",playing?"Пауза":"Воспроизвести");
    const mute=E("source-mute-toggle");
    mute.textContent=player().muted?"🔇":"🔊";
    mute.setAttribute("aria-label",player().muted?"Включить звук":"Выключить звук");
  }
  function renderRange(){
    if(!meta)return;
    const left=pct(start),right=pct(end);
    E("timeline-selected-range").style.left=left+"%";
    E("timeline-selected-range").style.width=Math.max(0,right-left)+"%";
    E("timeline-start-handle").style.left=left+"%";
    E("timeline-end-handle").style.left=right+"%";
    E("timeline-playhead").style.left=pct(current)+"%";
    for(const [id,value] of [["timeline-start-input",start],["timeline-end-input",end]]){
      const input=E(id);
      if(document.activeElement!==input)input.value=String(Math.round(value*10)/10);
    }
    E("timeline-range-label").textContent=stamp(start)+" — "+stamp(end)+" · "+stamp(end-start)+" для анализа";
    E("timeline-status").textContent=stamp(current)+" / "+stamp(meta.duration_sec);
    E("timeline-playhead").setAttribute("aria-valuetext",stamp(current));
    ranges.set(video,{start,end});
  }
  function markUnsaved(){
    E("range-save-status").textContent="Диапазон изменён — сохраните перед анализом";
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
  function boundary(side,value,adjustOther=false){
    if(!meta||!Number.isFinite(Number(value)))return;
    const t=Number(value),duration=meta.duration_sec;
    if(side==="start"){
      if(adjustOther){
        start=clamp(t,0,Math.max(0,duration-1));
        if(start>=end)end=Math.min(duration,start+1);
      }else start=clamp(t,0,Math.max(0,end-1));
    }else{
      if(adjustOther){
        end=clamp(t,1,duration);
        if(end<=start)start=Math.max(0,end-1);
      }else end=clamp(t,Math.min(duration,start+1),duration);
    }
    renderRange();
    markUnsaved();
  }
  function cancelScheduledSeek(){
    if(scrubTimer!==null){clearTimeout(scrubTimer);scrubTimer=null;}
  }
  function seek(time,final=false){
    if(!meta)return;
    current=clamp(Number(time)||0,0,Math.max(0,meta.duration_sec-.05));
    renderRange();
    cancelScheduledSeek();
    if(final)performSeek();
    else scrubTimer=setTimeout(()=>{scrubTimer=null;performSeek();},100);
  }
  function performSeek(){
    if(meta&&player().src&&player().readyState>0){
      try{player().currentTime=current;}catch(_){}
    }
  }
  function atPointer(e){
    const rect=E("match-timeline").getBoundingClientRect();
    return clamp((e.clientX-rect.left)/Math.max(1,rect.width),0,1)*meta.duration_sec;
  }
  function beginDrag(e,type,target){
    if(!meta||e.button!==0)return;
    e.preventDefault();e.stopPropagation();
    const wasPlaying=type==="playhead"&&!player().paused;
    if(wasPlaying)player().pause();
    drag={type,pointer:e.pointerId,target,wasPlaying};
    target.setPointerCapture(e.pointerId);
    moveDrag(e);
  }
  function moveDrag(e){
    if(!drag||e.pointerId!==drag.pointer||!meta)return;
    const t=atPointer(e);
    if(drag.type==="playhead")seek(t);
    else boundary(drag.type,t);
  }
  function endDrag(e){
    if(!drag||e.pointerId!==drag.pointer)return;
    const previous=drag;
    moveDrag(e);
    drag=null;
    if(previous.type==="playhead"){
      seek(atPointer(e),true);
      if(previous.wasPlaying)player().play().catch(()=>{});
    }
    if(previous.target.hasPointerCapture(e.pointerId))
      previous.target.releasePointerCapture(e.pointerId);
  }
  function attachDrag(target,type){
    target.addEventListener("pointerdown",e=>beginDrag(e,type,target));
    target.addEventListener("pointermove",moveDrag);
    target.addEventListener("pointerup",endDrag);
    target.addEventListener("pointercancel",endDrag);
    target.addEventListener("keydown",e=>{
      if(!meta||!["ArrowLeft","ArrowRight"].includes(e.key))return;
      e.preventDefault();
      const delta=(e.key==="ArrowLeft"?-1:1)*(e.shiftKey?5:1);
      if(type==="playhead")seek(current+delta,true);
      else boundary(type,(type==="start"?start:end)+delta);
    });
  }
  function togglePlayback(){
    const v=player();
    if(!meta||!v.src)return;
    if(v.paused)v.play().catch(()=>{
      E("source-playback-note").textContent="Не удалось воспроизвести этот видеоформат в браузере.";
    });
    else v.pause();
  }
  async function setVideo(name){
    if(!name)return;
    const request=++loadSeq;
    if(meta&&video)ranges.set(video,{start,end});
    video=name;meta=null;current=0;drag=null;cancelScheduledSeek();
    const v=player();
    v.pause();v.removeAttribute("src");v.load();updatePlayButton();
    E("source-video-placeholder").hidden=false;
    E("source-video-placeholder").textContent="Открываем видео…";
    E("preview-source-details").textContent="Читаем параметры файла…";
    E("source-playback-note").textContent="Белая линия — позиция просмотра; зелёные — границы анализа.";
    try{
      const get=async(path)=>{
        const response=await fetch(path,{cache:"no-store"});
        const data=await response.json();
        if(!response.ok)throw Error(data.error||"Ошибка доступа к видео");
        return data;
      };
      const q="project_id="+encodeURIComponent(projectId)+"&video="+encodeURIComponent(name);
      const [info,playback]=await Promise.all([
        get("/api/video-info?video="+encodeURIComponent(name)),
        get("/api/video-view?"+q),
      ]);
      if(request!==loadSeq)return;
      meta=info;
      fitSourceFrame();
      if(window.VolleyballCourtUI)
        await window.VolleyballCourtUI.setVideo(projectId,name,info);
      if(request!==loadSeq)return;
      const saved=ranges.get(name);
      start=saved?clamp(saved.start,0,Math.max(0,info.duration_sec-1)):0;
      end=saved?clamp(saved.end,start+1,info.duration_sec):info.duration_sec;
      E("timeline-start-input").max=info.duration_sec;
      E("timeline-end-input").max=info.duration_sec;
      E("preview-source-details").textContent=name+" · "+info.width+"×"+info.height+" · "+stamp(info.duration_sec);
      E("timeline-ticks").innerHTML=[0,.25,.5,.75,1]
        .map(n=>"<span>"+stamp(n*info.duration_sec)+"</span>").join("");
      v.poster=playback.poster_url;
      v.src=playback.preview_url;
      v.load();
      v.addEventListener("loadedmetadata",()=>{
        if(request!==loadSeq||!meta)return;
        if(v.currentTime<.2)
          seek(Math.min(Math.max(start,2),Math.max(0,meta.duration_sec-.2)),true);
      },{once:true});
      E("source-video-placeholder").hidden=true;
      E("source-playback-note").textContent=playback.browser_preview==="normalized"
        ?"Используется совместимая копия H.264; оригинал не меняется. Перемотка — только на шкале под видео."
        :"Используется оригинал. Если браузер не поддерживает кодек, для просмотра потребуется совместимый формат.";
      current=clamp(start,0,info.duration_sec);
      showWidth();renderRange();updatePlayButton();
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
      if(!meta)return;
      start=0;end=meta.duration_sec;renderRange();markUnsaved();
    });
    E("timeline-set-start").addEventListener("click",()=>boundary("start",current,true));
    E("timeline-set-end").addEventListener("click",()=>boundary("end",current,true));
    E("timeline-start-input").addEventListener("change",e=>boundary("start",e.target.value,true));
    E("timeline-end-input").addEventListener("change",e=>boundary("end",e.target.value,true));
    const timebar=E("match-timeline");
    attachDrag(timebar,"playhead");
    attachDrag(E("timeline-playhead"),"playhead");
    attachDrag(E("timeline-start-handle"),"start");
    attachDrag(E("timeline-end-handle"),"end");
    E("source-play-toggle").addEventListener("click",togglePlayback);
    E("source-mute-toggle").addEventListener("click",()=>{
      player().muted=!player().muted;updatePlayButton();
    });
    player().addEventListener("click",()=>{if(!window.VolleyballCourtUI?.isEditing())togglePlayback();});
    player().addEventListener("play",updatePlayButton);
    player().addEventListener("pause",updatePlayButton);
    player().addEventListener("volumechange",updatePlayButton);
    player().addEventListener("timeupdate",()=>{
      if(meta&&!drag){
        current=player().currentTime;
        renderRange();
      }
    });
    window.addEventListener("resize",fitSourceFrame);
    player().addEventListener("error",()=>{
      if(meta)E("source-playback-note").textContent=
        "Браузер не поддерживает кодек этого видео. Диапазон можно задавать вручную.";
    });
    document.addEventListener("keydown",e=>{
      if(!E("view-analysis").classList.contains("active")||!meta)return;
      if(e.target.closest("input,textarea,select,button,[contenteditable]"))return;
      if(e.code==="Space"){e.preventDefault();togglePlayback();}
      else if(e.key==="ArrowRight"||e.key==="ArrowLeft"){
        e.preventDefault();seek(current+(e.key==="ArrowRight"?1:-1)*(e.shiftKey?5:1),true);
      }
    });
    updatePlayButton();
  }
  function setProjectRanges(saved={},identifier=""){
    ++loadSeq;cancelScheduledSeek();
    if(window.VolleyballCourtUI)window.VolleyballCourtUI.reset();
    projectId=identifier;video="";meta=null;ranges.clear();drag=null;
    player().pause();
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
