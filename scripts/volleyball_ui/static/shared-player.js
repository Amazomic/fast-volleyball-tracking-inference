"use strict";
(function(global){
  const frameDelta=fps=>1/(Number.isFinite(Number(fps))&&Number(fps)>0?Number(fps):30);
  const holdFrames=milliseconds=>Math.min(240,Math.pow(2,Math.max(0,milliseconds-300)/450));
  const clamp=(value,start,end)=>Math.max(start,Math.min(end,value));
  function step(adapter,direction,frames=1){
    const range=adapter.bounds();
    if(!(range.end>range.start))return;
    adapter.pause();
    adapter.seek(clamp(adapter.position()+direction*frames*frameDelta(adapter.fps()),range.start,range.end));
  }
  function createHold(adapter,clock={now:()=>performance.now(),
    setInterval:(callback,delay)=>setInterval(callback,delay),
    clearInterval:identifier=>clearInterval(identifier)}){
    let direction=0,started=0,timer=null;
    function stop(){
      if(timer!==null)clock.clearInterval(timer);
      timer=null;direction=0;
    }
    function start(next){
      if(direction===next)return;
      stop();direction=next;started=clock.now();step(adapter,next);
      timer=clock.setInterval(()=>{
        if(clock.now()-started>=300)step(adapter,direction,Math.round(holdFrames(clock.now()-started)));
      },80);
    }
    return {start,stop};
  }
  function format(value){
    const t=Math.max(0,Number(value)||0),whole=Math.floor(t);
    return [Math.floor(whole/3600),Math.floor(whole%3600/60),whole%60]
      .map(n=>String(n).padStart(2,"0")).join(":")+"."+String(Math.floor((t-whole)*100)).padStart(2,"0");
  }
  const mounted=[];
  let active=null;
  function mount(video,adapter){
    if(!video||video.dataset.sharedPlayer)return;
    video.dataset.sharedPlayer="true";
    const shell=adapter.shell;
    shell.classList.add("studio-player");
    const controls=document.createElement("div");
    controls.className="studio-player-controls";
    controls.setAttribute("aria-label","Управление видео");
    controls.innerHTML='<input class="studio-player-seek" type="range" min="0" max="0" step="any" value="0" aria-label="Позиция видео">'+
      '<div class="studio-player-row"><button type="button" data-control="play" aria-label="Воспроизвести" title="Воспроизвести / пауза · Пробел">▶</button>'+
      '<button type="button" data-control="back" aria-label="Предыдущий кадр" title="Предыдущий кадр · ← · удерживайте для ускорения">❮</button>'+
      '<button type="button" data-control="forward" aria-label="Следующий кадр" title="Следующий кадр · → · удерживайте для ускорения">❯</button>'+
      '<output class="studio-player-time"></output><span class="studio-player-extra"></span>'+
      '<button type="button" data-control="mute" aria-label="Выключить звук" title="Звук · M">♫</button>'+
      '<input class="studio-player-volume" type="range" min="0" max="1" step="0.05" aria-label="Громкость">'+
      '<select data-control="speed" aria-label="Скорость воспроизведения"><option value="0.25">0.25×</option><option value="0.5">0.5×</option><option value="0.75">0.75×</option><option value="1" selected>1×</option><option value="1.5">1.5×</option><option value="2">2×</option><option value="3">3×</option></select>'+
      '<button type="button" data-control="help" aria-label="Горячие клавиши" aria-expanded="false">?</button>'+
      '<button type="button" data-control="fullscreen" aria-label="Полный экран" title="Полный экран · F">⛶</button></div>'+
      '<div class="studio-player-help" hidden>Пробел / K — пауза · ← → — кадр, удержание ускоряет · J / L — −10 / +10 с · Home / End — границы · M — звук · F — полный экран</div>';
    shell.append(controls);
    const button=name=>controls.querySelector('[data-control="'+name+'"]');
    const seek=controls.querySelector(".studio-player-seek");
    const volume=controls.querySelector(".studio-player-volume");
    const hold=createHold(adapter);
    const item={video,adapter,hold,controls};
    mounted.push(item);
    function focus(){active=item;}
    shell.addEventListener("pointerdown",focus);
    shell.addEventListener("focusin",focus);
    function play(){
      hold.stop();
      if(video.paused){
        const result=adapter.toggle?adapter.toggle():video.play();
        result?.catch?.(()=>{});
      }else video.pause();
    }
    function fullscreen(){
      const result=document.fullscreenElement?document.exitFullscreen():shell.requestFullscreen?.();
      result?.catch?.(()=>{});
    }
    item.play=play;item.fullscreen=fullscreen;
    button("play").addEventListener("click",play);
    button("mute").addEventListener("click",()=>{video.muted=!video.muted;});
    button("speed").addEventListener("change",event=>{video.playbackRate=Number(event.target.value);});
    volume.addEventListener("input",()=>{video.volume=Number(volume.value);video.muted=video.volume===0;});
    button("fullscreen").addEventListener("click",fullscreen);
    button("help").addEventListener("click",()=>{
      const help=controls.querySelector(".studio-player-help");
      help.hidden=!help.hidden;button("help").setAttribute("aria-expanded",String(!help.hidden));
    });
    for(const [name,direction] of [["back",-1],["forward",1]]){
      const node=button(name);
      node.addEventListener("pointerdown",event=>{
        if(event.button!==0)return;
        event.preventDefault();focus();node.focus();node.setPointerCapture?.(event.pointerId);hold.start(direction);
      });
      for(const event of ["pointerup","pointercancel","lostpointercapture","blur"])
        node.addEventListener(event,hold.stop);
      node.addEventListener("click",event=>{if(event.detail===0)step(adapter,direction);});
    }
    seek.addEventListener("input",()=>{hold.stop();adapter.seek(Number(seek.value));});
    function sync(){
      const range=adapter.bounds(),ready=video.readyState>=1&&Number.isFinite(range.end)&&range.end>range.start;
      if(!ready)hold.stop();
      seek.min=String(range.start||0);seek.max=String(ready?range.end:0);
      seek.value=String(ready?adapter.position():0);seek.disabled=!ready;
      controls.querySelector(".studio-player-time").textContent=format(adapter.position())+" / "+format(range.end);
      seek.setAttribute("aria-valuetext",format(adapter.position()));
      button("play").textContent=video.paused?"▶":"Ⅱ";
      button("play").setAttribute("aria-label",video.paused?"Воспроизвести":"Пауза");
      for(const name of ["play","back","forward","speed"])button(name).disabled=!ready;
      button("mute").textContent=video.muted?"♫̸":"♫";
      button("mute").setAttribute("aria-label",video.muted?"Включить звук":"Выключить звук");
      volume.value=String(video.muted?0:video.volume);
      button("speed").value=String(video.playbackRate);
    }
    for(const event of ["timeupdate","seeking","seeked","play","pause","loadedmetadata","durationchange","emptied","volumechange","ratechange"])
      video.addEventListener(event,sync);
    for(const event of ["loadstart","emptied","error"])video.addEventListener(event,hold.stop);
    if(adapter.extra)controls.querySelector(".studio-player-extra").append(adapter.extra);
    video.addEventListener("dblclick",fullscreen);
    if(!adapter.handlesVideoClick)video.addEventListener("click",play);
    item.sync=sync;sync();
    return item;
  }
  function boot(){
    const E=id=>document.getElementById(id);
    const analysis=global.VolleyballAnalysisUI,results=global.VolleyballResultsUI,montage=global.VolleyballMontagePlayer;
    if(analysis)mount(E("source-video-player"),{
      shell:E("source-video-player").closest(".workspace-video-player"),
      position:analysis.currentPosition,bounds:()=>({start:0,end:analysis.duration()}),
      fps:analysis.fps,seek:analysis.seek,pause:()=>E("source-video-player").pause(),toggle:analysis.togglePlayback,
      handlesVideoClick:true
    });
    if(results)mount(E("result-video-player"),{
      shell:E("result-video-player").closest(".result-player-wrap"),
      position:results.currentPosition,bounds:results.playbackBounds,fps:results.fps,
      seek:results.seek,pause:()=>E("result-video-player").pause(),extra:E("result-playback-mode")
    });
    if(montage)mount(E("preview-player"),{
      shell:E("montage-preview-shell"),position:()=>E("preview-player").currentTime||0,
      bounds:montage.playbackBounds,fps:montage.fps,seek:montage.seek,
      pause:()=>E("preview-player").pause(),toggle:montage.togglePlayback,
      handlesVideoClick:true
    });
    function current(){
      if(active&&active.video.closest(".view")?.classList.contains("active"))return active;
      return mounted.find(item=>item.video.closest(".view")?.classList.contains("active"));
    }
    document.addEventListener("keydown",event=>{
      if(event.defaultPrevented||event.altKey||event.ctrlKey||event.metaKey||event.isComposing)return;
      if(event.target.closest?.("input,textarea,select,[contenteditable],dialog[open]"))return;
      const item=current();if(!item||item.video.readyState<1||!(item.adapter.bounds().end>item.adapter.bounds().start))return;
      if(global.VolleyballCourtUI?.isEditing())return;
      const key=event.key.toLowerCase();
      if(event.target.closest?.("button,a")&&(key===" "||key==="enter"))return;
      if(!["arrowleft","arrowright"," ","k","j","l","home","end","m","f",",","."].includes(key))return;
      event.preventDefault();
      if(key==="arrowleft"||key==="arrowright"){
        item.hold.start(key==="arrowleft"?-1:1);return;
      }
      if(event.repeat)return;
      if(key===" "||key==="k")item.play();
      else if(key===","||key===".")step(item.adapter,key===","?-1:1);
      else if(key==="j"||key==="l"){
        const range=item.adapter.bounds();
        item.adapter.seek(clamp(item.adapter.position()+(key==="j"?-10:10),range.start,range.end));
      }else if(key==="home"||key==="end")item.adapter.seek(item.adapter.bounds()[key==="home"?"start":"end"]);
      else if(key==="m")item.video.muted=!item.video.muted;
      else if(key==="f")item.fullscreen();
    });
    const stop=()=>mounted.forEach(item=>item.hold.stop());
    document.addEventListener("keyup",event=>{if(["ArrowLeft","ArrowRight"].includes(event.key))stop();});
    global.addEventListener("blur",stop);
    document.addEventListener("visibilitychange",()=>{if(document.hidden)stop();});
    const observer=new MutationObserver(()=>{
      for(const item of mounted){
        if(!item.video.closest(".view")?.classList.contains("active")){
          item.hold.stop();item.video.pause();
        }
        item.sync();
      }
    });
    document.querySelectorAll(".view").forEach(view=>observer.observe(view,{attributes:true,attributeFilter:["class"]}));
  }
  const api={frameDelta,holdFrames,step,createHold,mount};
  global.VolleyballSharedPlayer=api;
  if(typeof module!=="undefined"&&module.exports)module.exports=api;
  if(typeof document!=="undefined")document.addEventListener("DOMContentLoaded",boot);
})(typeof window!=="undefined"?window:globalThis);
