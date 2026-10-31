"use strict";
/* Analysis results: watch cached H.264, display event overlays and play rally bounds. */
window.VolleyballResultsUI=(()=>{
  const E=id=>document.getElementById(id);
  const v=()=>E("result-video-player");
  const esc=s=>String(s??"").replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
  const clock=t=>{
    t=Math.max(0,Math.round(t));
    return [Math.floor(t/3600),Math.floor(t%3600/60),t%60].map(x=>String(x).padStart(2,"0")).join(":");
  };
  const actionText={attack:"Атака",block:"Блок",serve:"Подача",dig:"Приём",overhead_pass:"Передача",low_touch:"Касание"};
  let project="",source="",data=null,segment=null,loadId=0,playing=false,rafId=null;
  let totalDuration=0,offset=0;

  function fitResultVideo(){
    const player=v(),container=E("view-analysis-results").querySelector(".result-video-main");
    if(!container||!player.videoWidth||!player.videoHeight)return;
    const h=Math.min(window.innerHeight*.70,780);
    container.style.setProperty("--result-display-width",h*player.videoWidth/player.videoHeight+"px");
    container.style.setProperty("--result-aspect",player.videoWidth+"/"+player.videoHeight);
  }
  const mediaTime=t=>Math.max(0,t-offset);
  const sourceTime=t=>t+offset;
  const percent=t=>Math.max(0,Math.min(100,(t-offset)*100/Math.max(1,totalDuration)));
  function clear(){
    const player=v();
    player.pause();
    player.removeAttribute("src");
    player.load();
    segment=null;data=null;offset=0;totalDuration=0;
    E("result-rally-list").innerHTML='<div class="empty">Нет сохранённых розыгрышей.</div>';
    E("result-rally-track").innerHTML="";
    E("result-timeline-ticks").innerHTML="";
    E("result-rally-now").textContent="Выберите розыгрыш справа";
    E("result-overlay-ball").hidden=true;
    E("result-overlay-event").hidden=true;
    E("result-overlay-rally").hidden=true;
  }
  function renderList(){
    if(!data)return;
    const rallies=data.rallies||[];
    E("result-rally-count").textContent=rallies.length+" эпизодов";
    E("result-rally-list").innerHTML=rallies.length?rallies.map((r,i)=>{
      const duration=r.end-r.start;
      const count=(data.actions||[]).filter(a=>a.time>=r.start&&a.time<=r.end).length;
      return '<button type="button" class="result-rally-item" data-rally="'+i+
        '" title="Воспроизвести только этот розыгрыш"><span class="result-rally-number">№ '+(i+1)+'</span>'+
        '<span class="result-rally-details"><b>'+clock(r.start)+' – '+clock(r.end)+'</b>'+
        '<small>'+duration.toFixed(1)+' с · '+count+' действий</small></span>'+
        '<span class="result-rally-play">▶</span></button>';
    }).join(""):'<div class="empty">В выбранном видео модель не нашла завершённых розыгрышей.</div>';
    const blocks=rallies.map((r,i)=>
      '<button type="button" data-rally="'+i+'" class="result-track-rally" title="Розыгрыш № '+(i+1)+
      '" style="left:'+percent(r.start)+'%;width:'+Math.max(.2,percent(r.end)-percent(r.start))+'%"></button>').join("");
    const marks=(data.actions||[]).map(a=>{
      const kind=String(a.type).replace(/[^a-z_]/g,"");
      return '<span class="result-track-event result-track-'+kind+'" style="left:'+percent(a.time)+'%" title="'+
        esc(actionText[a.type]||a.type)+" · "+clock(a.time)+'"></span>';
    }).join("");
    E("result-rally-track").innerHTML=blocks+marks+
      '<div id="result-track-playhead" class="result-track-playhead"></div>';
    E("result-timeline-ticks").innerHTML=[0,.25,.5,.75,1].map(t=>
      '<span>'+clock(sourceTime(t*totalDuration))+'</span>').join("");
  }
  async function setProject(projectId,videos=[],selected=null){
    const previousProject=project;
    project=projectId||"";
    const select=E("result-source-select");
    const options=videos.map(name=>'<option value="'+esc(name)+'">'+esc(name)+'</option>').join("");
    select.innerHTML=options;
    const preferred=selected&&videos.includes(selected)?selected:(previousProject===project&&videos.includes(source)?source:videos[0]);
    if(preferred){
      select.value=preferred;
      await setVideo(preferred);
    }else{
      source="";clear();
      E("result-video-placeholder").hidden=false;
      E("result-video-placeholder").textContent="Создайте проект и запустите анализ";
    }
  }
  async function setVideo(name){
    const seq=++loadId;
    source=name;
    clear();
    E("result-video-name").textContent=name;
    E("result-meta").textContent="Загрузка результатов…";
    E("result-video-placeholder").hidden=false;
    E("result-video-placeholder").textContent="Загрузка результатов анализа…";
    try{
      const query="?project_id="+encodeURIComponent(project)+"&video="+encodeURIComponent(name);
      const resp=await fetch("/api/analysis-view"+query,{cache:"no-store"});
      const result=await resp.json();
      if(!resp.ok)throw Error(result.error||"Не удалось загрузить анализ");
      if(seq!==loadId)return;
      if(!result.analyzed){
        E("result-video-placeholder").textContent="Для этого видео ещё нет завершённого анализа";
        E("result-meta").textContent="Результат не найден";
        return;
      }
      data=result;
      offset=Number(result.analysis_offset_sec)||0;
      totalDuration=Number(result.duration_sec)||0;
      E("result-meta").textContent=(result.rallies||[]).length+" розыгрышей · "+
        (result.actions||[]).length+" событий · "+clock(totalDuration);
      const player=v();
      player.src=result.media_url;
      player.load();
      E("result-video-placeholder").hidden=true;
      renderList();
      updateOverlay();
    }catch(err){
      if(seq!==loadId)return;
      E("result-video-placeholder").hidden=false;
      E("result-video-placeholder").textContent=err.message;
      E("result-meta").textContent="Не удалось открыть результат";
    }
  }
  function selectRally(index){
    if(!data)return;
    const r=data.rallies[index];
    if(!r)return;
    segment={index,start:mediaTime(r.start),end:mediaTime(r.end)};
    const player=v();
    player.pause();
    player.currentTime=Math.max(0,segment.start);
    const buttons=E("result-rally-list").querySelectorAll("[data-rally]");
    buttons.forEach(b=>b.classList.toggle("active",Number(b.dataset.rally)===index));
    E("result-rally-now").textContent="Розыгрыш № "+(index+1)+" · "+clock(r.start)+" – "+clock(r.end);
    E("result-overlay-rally").hidden=false;
    E("result-overlay-rally").textContent="РОЗЫГРЫШ № "+(index+1);
    // play() may require interaction (button click counts as gesture).
    const promise=player.play();
    if(promise&&promise.catch)promise.catch(()=>{
      // A seek or an intentional end-of-rally pause may abort play(): not a playback error.
      if(segment&&player.currentTime<segment.end-.15&&player.paused)
        E("result-rally-now").textContent="Розыгрыш № "+(index+1)+" · Нажмите ▶ для воспроизведения";
    });
  }
  function enforceEnd(){
    const player=v();
    if(!segment||player.paused||!Number.isFinite(player.currentTime))return;
    if(player.currentTime>=segment.end-.035){
      player.pause();
      player.currentTime=Math.max(segment.start,segment.end);
      E("result-rally-now").textContent="Розыгрыш № "+(segment.index+1)+" завершён · "+clock(sourceTime(segment.end));
      updateOverlay();
    }
  }
  function updateOverlay(){
    if(!data)return;
    const player=v(),t=sourceTime(player.currentTime||0);
    const elapsed=player.currentTime||0;
    const marker=E("result-track-playhead");
    if(marker)marker.style.left=(100*elapsed/Math.max(1,totalDuration))+"%";
    const near=(data.actions||[]).filter(a=>Math.abs(a.time-t)<=.42)
      .sort((a,b)=>Math.abs(a.time-t)-Math.abs(b.time-t))[0];
    const badge=E("result-overlay-event"),ball=E("result-overlay-ball");
    if(near){
      badge.textContent=(actionText[near.type]||near.type)+
        (near.player_id!=null?" · ID "+near.player_id:"");
      badge.hidden=false;
      ball.hidden=true;
      if(near.ball&&near.ball.space==="analysis_pixels"&&player.videoWidth>0&&player.videoHeight>0){
        const shell=E("result-overlay");
        const rect=shell.getBoundingClientRect();
        const scale=Math.min(rect.width/player.videoWidth,rect.height/player.videoHeight);
        const displayedW=player.videoWidth*scale,displayedH=player.videoHeight*scale;
        const left=(rect.width-displayedW)/2+Number(near.ball.x)*scale;
        const top=(rect.height-displayedH)/2+Number(near.ball.y)*scale;
        if(left>=0&&left<=rect.width&&top>=0&&top<=rect.height){
          ball.style.left=left+"px";ball.style.top=top+"px";ball.hidden=false;
        }
      }
    } else {badge.hidden=true;ball.hidden=true;}
  }
  function startFrameMonitor(){
    const player=v();
    if(player.requestVideoFrameCallback){
      const loop=()=>{
        rafId=player.requestVideoFrameCallback(()=>{
          enforceEnd();updateOverlay();loop();
        });
      };
      loop();
    }
  }
  function init(){
    E("result-source-select").addEventListener("change",e=>setVideo(e.target.value));
    E("result-rally-list").addEventListener("click",e=>{
      const b=e.target.closest("[data-rally]");
      if(b)selectRally(Number(b.dataset.rally));
    });
    E("result-rally-track").addEventListener("click",e=>{
      const b=e.target.closest("[data-rally]");
      if(b)selectRally(Number(b.dataset.rally));
    });
    v().addEventListener("loadedmetadata",fitResultVideo);
    window.addEventListener("resize",fitResultVideo);
    v().addEventListener("timeupdate",()=>{enforceEnd();updateOverlay()});
    v().addEventListener("seeked",updateOverlay);
    v().addEventListener("play",()=>{
      // When user starts manual playback beyond an old selection, start regular playback.
      if(segment&&(v().currentTime<segment.start-.1||v().currentTime>=segment.end)){
        segment=null;E("result-rally-now").textContent="Свободное воспроизведение";
      }
    });
    v().addEventListener("error",()=>{
      if(data)E("result-video-placeholder").textContent="Видео не поддерживается браузером";
    });
    startFrameMonitor();
  }
  function activeRally(){return segment?{...segment}:null}
  return {init,setProject,setVideo,selectRally,activeRally};
})();
