"use strict";

// One playback clock drives both the seek knob and the painted progress.
// The source video has native controls as a no-JS fallback; initialization
// replaces them only after all custom controls have been wired.
window.VolleyballMontagePlayer=(()=>{
  const MAX=10000;
  let video, seek, played, timeLabel, toolbar, playButton, muteButton, fullscreenButton;
  let drag=false, requestId=null;
  let rallyRange=null, pendingRallyPlay=false;

  const $=id=>document.getElementById(id);
  function fitAspect(sourceWidth,sourceHeight,areaWidth,areaHeight){
    if(![sourceWidth,sourceHeight,areaWidth,areaHeight].every(
        value=>Number.isFinite(value)&&value>0))return null;
    const scale=Math.min(areaWidth/sourceWidth,areaHeight/sourceHeight);
    return {width:Math.max(1,Math.floor(sourceWidth*scale)),
            height:Math.max(1,Math.floor(sourceHeight*scale))};
  }
  function format(seconds){
    if(!Number.isFinite(seconds))return "00:00";
    const value=Math.max(0,Math.floor(seconds));
    const mm=String(Math.floor(value/60)%60).padStart(2,"0");
    const ss=String(value%60).padStart(2,"0");
    return value>=3600?String(Math.floor(value/3600)).padStart(2,"0")+":"+mm+":"+ss:mm+":"+ss;
  }
  function mediaDuration(){
    return video&&Number.isFinite(video.duration)&&video.duration>0?video.duration:0;
  }
  function bounds(){
    const total=mediaDuration();
    if(!rallyRange)return {start:0,end:total};
    const start=Math.min(total,Math.max(0,rallyRange.start));
    return {start,end:Math.min(total,Math.max(start,rallyRange.end))};
  }
  function duration(){
    const range=bounds();
    return range.end-range.start;
  }
  function relativeTime(){
    return Math.max(0,(video?.currentTime||0)-bounds().start);
  }
  function repaint(position){
    const max=duration();
    const seconds=Math.max(0,Math.min(max,Number(position)||0));
    const fraction=max?seconds/max:0;
    const ticks=Math.round(fraction*MAX);
    seek.value=String(ticks);
    // The rail starts/ends at the thumb centers (half a thumb from each edge).
    // Thus this same ratio positions the knob and the visible filled rail.
    played.style.width=(fraction*100)+"%";
    seek.setAttribute("aria-valuetext",format(seconds)+" из "+format(max));
    timeLabel.value=format(seconds)+" / "+format(max);
    timeLabel.textContent=timeLabel.value;
  }
  function refresh(){
    if(!video)return;
    toolbar.hidden=!duration()||!video.currentSrc;
    seek.disabled=!duration();
    if(!drag)repaint(relativeTime());
    playButton.textContent=video.paused?"▶":"❚❚";
    playButton.setAttribute("aria-label",video.paused?"Воспроизвести":"Пауза");
    muteButton.textContent=video.muted||video.volume===0?"♫̸":"♫";
    muteButton.setAttribute("aria-label",video.muted||video.volume===0?"Включить звук":"Выключить звук");
  }
  function frame(){
    requestId=null;
    // timeupdate is infrequent; guard the end of a selected rally every frame.
    if(!drag){enforceRallyEnd();refresh();}
    if(video&&!video.paused&&!video.ended)
      requestId=requestAnimationFrame(frame);
  }
  function startFrames(){
    if(requestId===null)requestId=requestAnimationFrame(frame);
  }
  function stopFrames(){
    if(requestId!==null){cancelAnimationFrame(requestId);requestId=null;}
    refresh();
  }
  function seekFraction(){
    const max=duration();
    if(!max)return;
    const fraction=Math.max(0,Math.min(1,Number(seek.value)/MAX));
    const seconds=fraction*max;
    repaint(seconds);
    const absolute=bounds().start+seconds;
    if(Math.abs(video.currentTime-absolute)>.02)video.currentTime=absolute;
  }
  function togglePlay(){
    if(!duration())return;
    if(video.paused){
      if(rallyRange&&video.currentTime>=bounds().end-.08)
        video.currentTime=bounds().start;
      video.play().catch(error=>{
        $("montage-preview-status").textContent="Не удалось воспроизвести: "+error.message;
      });
    }else video.pause();
  }
  function setRallyRange(start=null,end=null){
    if(start===null||end===null){
      rallyRange=null;pendingRallyPlay=false;
    }else{
      start=Number(start);end=Number(end);
      if(!Number.isFinite(start)||!Number.isFinite(end)||start<0||end<=start)
        throw new Error("Недопустимые границы выбранного розыгрыша");
      rallyRange={start,end};
      pendingRallyPlay=false;
    }
    if(video)refresh();
  }
  function playRally(start,end){
    setRallyRange(start,end);
    pendingRallyPlay=true;
    // Request playback synchronously from the user's click. The initial seek
    // is re-applied after metadata loads for sources not yet buffered.
    if(video.readyState>=1){
      video.currentTime=bounds().start;
      pendingRallyPlay=false;
    }else{
      try{video.currentTime=start}catch{}
    }
    video.play().catch(error=>{
      $("montage-preview-status").textContent="Нажмите ▶ для воспроизведения: "+error.message;
    });
    refresh();
  }
  function enforceRallyEnd(){
    if(!rallyRange||!mediaDuration())return;
    const range=bounds();
    if(video.currentTime>=range.end-.035){
      if(!video.paused)video.pause();
      if(video.currentTime>range.end)video.currentTime=range.end;
    }
  }
  function reset(){
    drag=false;
    stopFrames();
    toolbar.hidden=true;
    repaint(0);
  }
  function init(){
    video=$("preview-player");
    seek=$("montage-seek");
    played=$("montage-seek-fill");
    timeLabel=$("montage-play-time");
    toolbar=$("montage-playback");
    playButton=$("montage-play-toggle");
    muteButton=$("montage-mute-toggle");
    fullscreenButton=$("montage-fullscreen");
    if(!video||!seek||!played||!toolbar)return;
    const shell=$("montage-preview-shell");
    const stage=shell.querySelector(".preview-stage");
    function syncFullscreenVideo(){
      if(document.fullscreenElement!==shell){
        video.style.width="";
        video.style.height="";
        return;
      }
      // Size the element itself, not just the image inside a stretched
      // 100% x 100% player. Preserve its display aspect ratio at any viewport.
      const size=fitAspect(video.videoWidth,video.videoHeight,
                           stage.clientWidth,stage.clientHeight);
      if(!size)return;
      video.style.width=size.width+"px";
      video.style.height=size.height+"px";
    }
    if(typeof ResizeObserver!=="undefined"){
      new ResizeObserver(syncFullscreenVideo).observe(stage);
    }
    window.addEventListener("resize",syncFullscreenVideo);
    playButton.addEventListener("click",togglePlay);
    document.addEventListener("keydown",event=>{
      if(event.code!=="Space"||event.repeat||event.altKey||event.ctrlKey||
          event.metaKey||event.isComposing||!duration())return;
      const active=document.activeElement;
      if(active?.closest("input,textarea,select,button,a,[contenteditable],dialog[open]"))
        return;
      if(!document.getElementById("view-results")?.classList.contains("active")&&
          document.fullscreenElement!==shell)return;
      event.preventDefault();
      togglePlay();
    });
    seek.addEventListener("pointerdown",()=>{drag=true;});
    seek.addEventListener("input",seekFraction);
    seek.addEventListener("change",()=>{seekFraction();drag=false;refresh();});
    const finishDrag=()=>{if(drag){drag=false;refresh();}};
    seek.addEventListener("pointerup",finishDrag);
    seek.addEventListener("pointercancel",finishDrag);
    seek.addEventListener("blur",finishDrag);
    muteButton.addEventListener("click",()=>{video.muted=!video.muted;refresh();});
    fullscreenButton.addEventListener("click",()=>{
      if(document.fullscreenElement){
        document.exitFullscreen().catch(()=>{});
      }else if(shell.requestFullscreen){
        shell.requestFullscreen().catch(()=>{});
      }else if(video.webkitEnterFullscreen){
        video.webkitEnterFullscreen();
      }
    });
    document.addEventListener("fullscreenchange",()=>{
      fullscreenButton.setAttribute("aria-label",
        document.fullscreenElement===shell?"Выйти из полного экрана":"Полный экран");
      syncFullscreenVideo();
    });
    video.addEventListener("click",()=>{
      if(!duration())return;
      if(video.paused)video.play().catch(()=>{});
      else video.pause();
    });
    video.addEventListener("loadedmetadata",()=>{
      if(pendingRallyPlay&&rallyRange){
        video.currentTime=bounds().start;
        pendingRallyPlay=false;
        if(video.paused)video.play().catch(error=>{
          $("montage-preview-status").textContent="Нажмите ▶ для воспроизведения: "+error.message;
        });
      }
      syncFullscreenVideo();
      refresh();
    });
    video.addEventListener("durationchange",refresh);
    video.addEventListener("timeupdate",()=>{enforceRallyEnd();refresh();});
    video.addEventListener("seeking",refresh);
    video.addEventListener("seeked",()=>{enforceRallyEnd();refresh();});
    video.addEventListener("volumechange",refresh);
    video.addEventListener("play",()=>{refresh();startFrames();});
    video.addEventListener("pause",stopFrames);
    video.addEventListener("ended",stopFrames);
    video.addEventListener("emptied",()=>{
      syncFullscreenVideo();
      reset();
    });
    video.addEventListener("error",()=>{
      stopFrames();
      toolbar.hidden=true;
      const code=video.error?.code;
      if(code&&video.currentSrc)
        $("montage-preview-status").textContent="Ошибка воспроизведения ("+code+"). Можно скачать оригинал.";
    });
    video.controls=false;
    reset();
  }
  return {init,reset,fitAspect,setRallyRange,playRally};
})();
document.addEventListener("DOMContentLoaded",()=>window.VolleyballMontagePlayer.init());
