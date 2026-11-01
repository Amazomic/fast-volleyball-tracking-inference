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
  let playbackMode="original",segmentSeconds=18,segmentStart=0,segmentToken=0;
  let prefetchTarget=null;
  const ranges=new Map();
  const pct=t=>meta?.duration_sec?100*clamp(t,0,meta.duration_sec)/meta.duration_sec:0;
  const width=()=>E("manual-width-enabled").checked?Number(E("analysis-width").value):meta?.auto_width;
  function fitSourceFrame(){
    if(!meta)return;
    const stage=document.querySelector(".workspace-video-player");
    const main=document.querySelector(".workspace-video-main");
    if(!stage||!main)return;
    // Size the visible image inside the FIXED gallery canvas. Do not alter
    // the canvas's dimensions or the layout when annotation is switched on.
    const availableWidth=Math.max(1,stage.clientWidth-12);
    const availableHeight=Math.max(1,stage.clientHeight-12);
    const w=Math.min(availableWidth,availableHeight*meta.width/meta.height);
    const h=w*meta.height/meta.width;
    main.style.setProperty("--source-video-width",w+"px");
    main.style.setProperty("--source-video-height",h+"px");
    main.style.setProperty("--source-aspect",meta.width+"/"+meta.height);
    if(window.VolleyballCourtUI)window.VolleyballCourtUI.refreshGeometry();
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
    E("timeline-start-handle").classList.toggle("is-at-start",start<=0.0001);
    E("timeline-end-handle").classList.toggle("is-at-end",end>=meta.duration_sec-0.0001);
    E("timeline-playhead").classList.toggle("is-at-start",current<=0.0001);
    E("timeline-playhead").classList.toggle("is-at-end",current>=meta.duration_sec-0.1);
    for(const [id,value] of [["timeline-start-input",start],["timeline-end-input",end]]){
      const input=E(id);
      // A dragged handle always updates its corresponding numeric field.
      // Do not overwrite a partially typed value during ordinary video playback.
      if(document.activeElement!==input||drag)
        input.value=String(Math.round(value*10)/10);
    }
    E("timeline-playhead").setAttribute("aria-valuetext",stamp(current));
    ranges.set(video,{start,end});
  }
  function markUnsaved(){
    E("range-save-status").textContent="Настройки сохраняются автоматически";
    window.VolleyballAutoSave?.schedule(750);
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
    else if(!(playbackMode==="segments"&&drag?.type==="playhead"))
      scrubTimer=setTimeout(()=>{scrubTimer=null;performSeek();},160);
  }
  function segmentUrl(startAt){
    return "/api/video-segment?project_id="+encodeURIComponent(projectId)+
      "&video="+encodeURIComponent(video)+"&start="+startAt;
  }
  function alignedStart(time){
    return Math.floor(Math.max(0,time)/segmentSeconds)*segmentSeconds;
  }
  function loadSegmentAt(time,resume=false){
    if(!meta||playbackMode!=="segments")return;
    const v=player(),startAt=alignedStart(time);
    const token=++segmentToken,request=loadSeq;
    segmentStart=startAt;prefetchTarget=null;
    v.pause();
    v.src=segmentUrl(startAt);
    v.load();
    v.addEventListener("loadedmetadata",()=>{
      if(request!==loadSeq||token!==segmentToken||playbackMode!=="segments")return;
      const local=clamp(time-startAt,0,Math.max(0,v.duration-.075));
      if(local>.02)try{v.currentTime=local;}catch(_){}
      current=time;renderRange();
      if(resume)v.play().catch(()=>{});
    },{once:true});
    updatePlayButton();
  }
  function prefetchSegment(){
    if(!meta||playbackMode!=="segments")return;
    const next=segmentStart+segmentSeconds;
    if(next>=meta.duration_sec||prefetchTarget===next)return;
    prefetchTarget=next;
    fetch(segmentUrl(next),{cache:"force-cache"})
      .then(response=>{if(!response.ok)throw Error("Next segment unavailable");})
      .catch(()=>{if(prefetchTarget===next)prefetchTarget=null;});
  }
  function performSeek(){
    if(!meta)return;
    if(playbackMode==="segments"){
      if(!player().src||alignedStart(current)!==segmentStart){
        loadSegmentAt(current,!player().paused);
      }else if(player().readyState>0){
        try{player().currentTime=clamp(current-segmentStart,0,
          Math.max(0,player().duration-.075));}catch(_){}
      }
    }else if(player().src&&player().readyState>0){
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
      E("source-playback-note").textContent=
        "Не удалось начать просмотр. Попробуйте переместить позицию на шкале.";
    });
    else v.pause();
  }
  async function setVideo(name){
    if(!name)return;
    const request=++loadSeq;
    if(meta&&video)ranges.set(video,{start,end});
    video=name;meta=null;current=0;drag=null;cancelScheduledSeek();
    segmentToken++;segmentStart=0;prefetchTarget=null;playbackMode="original";
    const v=player();
    v.pause();v.removeAttribute("src");v.load();updatePlayButton();
    E("source-video-placeholder").hidden=false;
    E("source-video-placeholder").textContent="Открываем видео…";
    E("preview-source-details").textContent="Читаем параметры файла…";
    E("source-playback-note").textContent="Белая линия — позиция просмотра; зелёные — границы анализа.";
    E("source-video-note").open=false;
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
      playbackMode=playback.browser_preview==="segments"?"segments":"original";
      segmentSeconds=Number(playback.segment_seconds)||18;
      if(playbackMode==="segments"){
        loadSegmentAt(start,false);
      }else{
        v.src=playback.preview_url;
        v.load();
        v.addEventListener("loadedmetadata",()=>{
          if(request!==loadSeq||!meta)return;
          if(v.currentTime<.2)
            seek(Math.min(start,Math.max(0,meta.duration_sec-.2)),true);
        },{once:true});
      }
      E("source-video-placeholder").hidden=true;
      E("source-playback-note").textContent=playbackMode==="segments"
        ?"Видео открывается напрямую из локальной папки: по мере просмотра подгружаются только нужные короткие участки. Полная копия не создаётся."
        :"Используется оригинальный файл без перекодирования.";
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
    for(const [id,side] of [["timeline-start-input","start"],["timeline-end-input","end"]]){
      const input=E(id);
      input.addEventListener("input",()=>{
        if(!meta||!input.value.trim()||!Number.isFinite(input.valueAsNumber))return;
        // While typing "320" the intermediate "3" must not move an
        // already selected 180-second start boundary backwards.
        const value=input.valueAsNumber;
        if(side==="end"&&value<=start||side==="start"&&value>=end)return;
        boundary(side,value,false);
      });
      input.addEventListener("change",()=>{
        if(input.value.trim()&&Number.isFinite(input.valueAsNumber))
          boundary(side,input.valueAsNumber,true);
        renderRange();
      });
      input.addEventListener("blur",()=>{
        if(meta)input.value=String(Math.round((side==="start"?start:end)*10)/10);
      });
    }
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
        current=playbackMode==="segments"?
          Math.min(meta.duration_sec,segmentStart+player().currentTime):
          player().currentTime;
        renderRange();
        if(playbackMode==="segments"&&
          Number.isFinite(player().duration)&&
          player().duration-player().currentTime<7)prefetchSegment();
      }
    });
    player().addEventListener("ended",()=>{
      if(meta&&playbackMode==="segments"&&
         segmentStart+segmentSeconds<meta.duration_sec-.05){
        loadSegmentAt(segmentStart+segmentSeconds,true);
      }
    });
    window.addEventListener("resize",fitSourceFrame);
    if(typeof ResizeObserver!=="undefined"){
      const stage=document.querySelector(".workspace-video-player");
      new ResizeObserver(fitSourceFrame).observe(stage);
    }
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
    segmentToken++;segmentStart=0;prefetchTarget=null;playbackMode="original";
    player().pause();
    for(const [name,range] of Object.entries(saved||{})){
      if(range&&Number.isFinite(range.start)&&Number.isFinite(range.end))
        ranges.set(name,{start:range.start,end:range.end});
    }
  }
  function currentRange(){
    return meta?{start:Math.round(start*1000)/1000,end:Math.round(end*1000)/1000}:null;
  }
  function activeVideo(){return video;}
  function activeProject(){return projectId;}
  function params(){
    if(!meta)throw Error("Дождитесь загрузки видео");
    const w=width();
    if(!Number.isInteger(w)||w<320||w>1920||w%2)throw Error("Некорректная ширина кадра");
    return {analysis_width:E("manual-width-enabled").checked?w:"auto"};
  }
  return {init,setVideo,setProjectRanges,currentRange,activeVideo,activeProject,params};
})();
