"use strict";
// Browser-local media preview, precise trim timeline and image-relative court geometry.
// Rust persists source edits; source video bytes remain unchanged.
(() => {
  const $=id=>document.getElementById(id);
  const wrapper=$("studio-analysis-editor");
  const video=$("studio-analysis-player"),canvas=$("studio-analysis-court");
  const stage=$("studio-analysis-stage"),frame=$("studio-analysis-frame");
  const media=stage.parentElement,context=canvas.getContext("2d");
  const begin=$("studio-analysis-trim-start"),end=$("studio-analysis-trim-end");
  const seek=$("studio-analysis-seek"),clock=$("studio-analysis-time");
  const rangeLabel=$("studio-analysis-range-label");
  const band=$("studio-analysis-selected-range");
  const startMarker=$("studio-analysis-start-marker"),endMarker=$("studio-analysis-end-marker");
  const playhead=$("studio-analysis-playhead"),playButton=$("studio-analysis-toggle-play");
  const status=$("studio-analysis-edit-status"),save=$("studio-analysis-edit-save");
  const modeButton=$("studio-analysis-court-mode");
  const zoomOut=$("studio-analysis-zoom-out"),zoomIn=$("studio-analysis-zoom-in");
  const zoomLabel=$("studio-analysis-zoom-label");
  const minZoom=0.52,maxZoom=1.24,zoomStep=0.08;
  let project="",source="",generation=0,corners=[],drag=-1,duration=0;
  let marking=false,zoom=0.84,settingsLoaded=false,settingsLoading=false,net=null,netDrag=-1;

  let baseRevision="",dirty=false,draftTimer=null,editEpoch=0;
  const revision=data=>data.edit_revision||JSON.stringify([data.trim_start_sec??0,data.trim_end_sec??null,data.court||[],data.net||null,data.source_revision??null]);
  const draftKey=(id,name)=>"volley:editor-draft:v1:"+JSON.stringify([id,name]);
  const readDraft=(id,name)=>{
    try{
      const raw=localStorage.getItem(draftKey(id,name));
      if(!raw||raw.length>4096)return null;
      const row=JSON.parse(raw),value=row.value;
      if(!Number.isFinite(row.expires)||row.expires<=Date.now()||typeof row.base!=="string"||!value||
        !Number.isFinite(value.trim_start_sec)||value.trim_start_sec<0||
        !(value.trim_end_sec===null||Number.isFinite(value.trim_end_sec))||
        !Array.isArray(value.court)||value.court.length>4||
        value.court.some(p=>!Array.isArray(p)||p.length!==2||p.some(v=>!Number.isFinite(v)||v<-.5||v>1.5)))return null;
      return row;
    }catch{return null;}
  };
  const removeDraft=()=>{try{localStorage.removeItem(draftKey(project,source));}catch{}};
  const flushDraft=()=>{
    if(draftTimer!==null)clearTimeout(draftTimer);
    draftTimer=null;
    if(!dirty||!project||!source||!baseRevision)return;
    const row={base:baseRevision,expires:Date.now()+14*86400000,
      value:{trim_start_sec:Number(begin.value),trim_end_sec:Number(end.value)||null,court:corners,net}};
    try{const text=JSON.stringify(row);if(text.length<=4096)localStorage.setItem(draftKey(project,source),text);}catch{}
  };
  window.addEventListener("beforeunload",flushDraft);
  const clamp=(n,lo,hi)=>Math.min(hi,Math.max(lo,n));
  const time=n=>{
    if(!Number.isFinite(n))return "00:00.00";
    const cs=Math.floor(Math.max(0,n)*100+0.000001);
    const seconds=Math.floor(cs/100),hours=Math.floor(seconds/3600);
    const mm=String(Math.floor(seconds%3600/60)).padStart(2,"0");
    const ss=String(seconds%60).padStart(2,"0");
    const cc=String(cs%100).padStart(2,"0");
    return (hours?String(hours)+":":"")+mm+":"+ss+"."+cc;
  };
  const message=(text,failed=false)=>{status.textContent=text;status.classList.toggle("is-error",failed);};
  const url=(path,params)=>path+"?"+new URLSearchParams(params).toString();
  const api=async(path,options={})=>{
    const result=await window.VolleyballStudioActivity.fetch(path,{cache:"no-store",credentials:"same-origin",...options});
    const body=await result.json().catch(()=>({}));
    if(!result.ok)throw Error(body.message||body.error||"HTTP "+result.status);
    return body;
  };
  // Small edit metadata only. Never cache the original video or frame bytes.
  const editCache=(()=>{
    const key=(id,name)=>"volley:studio-video-edit:v1:"+id+":"+encodeURIComponent(name);
    const read=(id,name)=>{
      try{
        const raw=sessionStorage.getItem(key(id,name));
        if(!raw||raw.length>4096)return null;
        const item=JSON.parse(raw);
        return item.expires>Date.now()?item.value:null;
      }catch{return null;}
    };
    const write=(id,name,value)=>{
      try{sessionStorage.setItem(key(id,name),JSON.stringify({
        expires:Date.now()+600000,value
      }));}catch{}
    };
    return {read,write};
  })();
  const valid=()=>Boolean(project&&source);

  const updateZoomControls=()=>{
    zoomLabel.textContent=Math.round((marking?zoom:1)*100)+"%";
    zoomOut.disabled=!valid()||!marking||zoom<=minZoom+0.001;
    zoomIn.disabled=!valid()||!marking||zoom>=maxZoom-0.001;
  };
  const setZoom=next=>{
    zoom=clamp(Math.round(next*100)/100,minZoom,maxZoom);
    stage.style.setProperty("--editor-zoom",marking?String(zoom):"1");
    updateZoomControls();
    draw();
  };
  const setMarking=active=>{
    marking=Boolean(active);
    stage.classList.toggle("marking",marking);
    video.controls=!marking;
    modeButton.textContent=marking?"Завершить разметку":"Разметить поле";
    modeButton.setAttribute("aria-pressed",String(marking));
    stage.style.setProperty("--editor-zoom",marking?String(zoom):"1");
    updateZoomControls();
    draw();
  };
  const syncControls=()=>{
    for(const node of wrapper.querySelectorAll("button,input"))node.disabled=!valid();
    updateZoomControls();
  };
  const pointXY=([x,y],stageRect,frameRect)=>[
    frameRect.left-stageRect.left+x*frameRect.width,
    frameRect.top-stageRect.top+y*frameRect.height
  ];
  const draw=()=>{
    const rect=canvas.getBoundingClientRect(),image=frame.getBoundingClientRect();
    if(!rect.width||!rect.height)return;
    const dpr=Math.min(2,window.devicePixelRatio||1);
    canvas.width=Math.round(rect.width*dpr);
    canvas.height=Math.round(rect.height*dpr);
    context.setTransform(dpr,0,0,dpr,0,0);
    context.clearRect(0,0,rect.width,rect.height);
    if((!corners.length&&!net)||!image.width||!image.height)return;
    const points=corners.map(p=>pointXY(p,rect,image));
    context.beginPath();
    points.forEach(([x,y],i)=>i?context.lineTo(x,y):context.moveTo(x,y));
    if(points.length===4)context.closePath();
    context.fillStyle="rgba(255,198,48,.13)";
    if(points.length===4)context.fill();
    context.strokeStyle="#ffd166";context.lineWidth=2.5;
    context.stroke();
    if(net){
      const endpoints=net.map(p=>pointXY(p,rect,image));
      context.beginPath();
      context.moveTo(...endpoints[0]);context.lineTo(...endpoints[1]);
      context.strokeStyle="#35e0ef";context.lineWidth=3;context.stroke();
      endpoints.forEach(([x,y])=>{
        context.beginPath();context.arc(x,y,6,0,Math.PI*2);
        context.fillStyle="#35e0ef";context.fill();
      });
    }    points.forEach(([x,y],i)=>{
      const [nx,ny]=corners[i];
      context.beginPath();context.arc(x,y,7,0,Math.PI*2);
      context.fillStyle=nx<0||nx>1||ny<0||ny>1?"#ff984f":"#ffd166";
      context.fill();context.lineWidth=2;context.strokeStyle="#121b2b";context.stroke();
      context.fillStyle="#0b1421";context.font="bold 11px sans-serif";
      context.textAlign="center";context.textBaseline="middle";
      context.fillText(String(i+1),x,y);
    });
  };
  const point=event=>{
    const rect=frame.getBoundingClientRect();
    if(!rect.width||!rect.height)return [0,0];
    // Image-relative coordinates can be outside [0,1] in the margin.
    // Guard extreme values to match the bounded Rust API.
    return [clamp((event.clientX-rect.left)/rect.width,-0.5,1.5),
      clamp((event.clientY-rect.top)/rect.height,-0.5,1.5)];
  };
  const markModified=()=>{
    updateClock();
    dirty=true;editEpoch++;
    if(draftTimer!==null)clearTimeout(draftTimer);
    draftTimer=setTimeout(flushDraft,250);
    if(valid())message("Есть несохранённые изменения");
  };
  canvas.addEventListener("pointerdown",event=>{
    if(!marking||!valid()||!video.videoWidth)return;
    const at=point(event),rect=canvas.getBoundingClientRect();
    const image=frame.getBoundingClientRect();
    netDrag=net?net.findIndex(p=>{const [x,y]=pointXY(p,rect,image);return Math.hypot(x-(event.clientX-rect.left),y-(event.clientY-rect.top))<17;}):-1;
    if(netDrag>=0){canvas.setPointerCapture(event.pointerId);return;}
    drag=corners.findIndex(p=>{
      const [x,y]=pointXY(p,rect,image);
      return Math.hypot(x-(event.clientX-rect.left),y-(event.clientY-rect.top))<17;
    });
    if(drag<0&&corners.length<4){
      corners.push(at);drag=corners.length-1;
      markModified();draw();
    }
    if(drag>=0)canvas.setPointerCapture(event.pointerId);
  });
  canvas.addEventListener("pointermove",event=>{
    if(netDrag>=0){net[netDrag]=point(event);draw();return;}
    if(drag<0)return;
    corners[drag]=point(event);draw();
  });
  const finishDrag=()=>{if(drag>=0||netDrag>=0)markModified();drag=-1;netDrag=-1;};
  canvas.addEventListener("pointerup",finishDrag);
  canvas.addEventListener("pointercancel",finishDrag);
  $("studio-analysis-court-undo").addEventListener("click",()=>{
    corners.pop();draw();markModified();
  });
  $("studio-analysis-court-clear").addEventListener("click",()=>{
    corners=[];net=null;draw();markModified();
  });
  $("studio-analysis-court-auto").addEventListener("click",async()=>{
    if(!valid())return;
    const token=generation,epoch=editEpoch,button=$("studio-analysis-court-auto");
    button.disabled=true;message("Поиск линий площадки и сетки на нескольких кадрах…");
    try{
      const result=await api("/api/video-court/detect",{
        method:"POST",headers:{"Content-Type":"application/json"},
        body:JSON.stringify({project_id:project,video:source,start_sec:Math.max(0,video.currentTime||0)})
      });
      if(token!==generation||epoch!==editEpoch)return;
      if(!result.detected){message(result.message||"Не удалось уверенно распознать разметку. Отметьте углы вручную.",true);return;}
      corners=result.court;net=result.net||null;setMarking(true);markModified();draw();
      message("Найдена разметка ("+result.supporting_frames+"/"+result.sampled_frames+" кадров). Проверьте углы и сетку, при необходимости перетащите точки, затем сохраните.");
    }catch(error){if(token===generation)message(error.message,true);}
    finally{button.disabled=!valid();}
  });  modeButton.addEventListener("click",()=>setMarking(!marking));
  zoomOut.addEventListener("click",()=>{if(marking)setZoom(zoom-zoomStep);});
  zoomIn.addEventListener("click",()=>{if(marking)setZoom(zoom+zoomStep);});

  const updateClock=()=>{
    const cursor=Number.isFinite(video.currentTime)?video.currentTime:0;
    const from=Number(begin.value),to=Number(end.value);
    const pct=value=>duration>0?clamp(value/duration*100,0,100):0;
    const left=pct(from),right=pct(to);
    clock.textContent=time(cursor)+" / "+time(duration);
    rangeLabel.textContent=duration>0?"Отрезок: "+time(from)+" – "+time(to):"Отрезок: —";
    startMarker.style.left=left+"%";
    endMarker.style.left=right+"%";
    playhead.style.left=pct(cursor)+"%";
    band.style.left=left+"%";
    band.style.width=Math.max(0,right-left)+"%";
    seek.value=duration>0?String(clamp(Math.round(cursor*1000),0,Math.round(duration*1000))):"0";
  };
  video.addEventListener("loadedmetadata",()=>{
    duration=Number.isFinite(video.duration)?video.duration:0;
    if(video.videoWidth>0&&video.videoHeight>0)
      media.style.setProperty("--frame-ratio",String(video.videoWidth/video.videoHeight));
    begin.max=String(duration);end.max=String(duration);
    seek.max=String(Math.max(1,Math.round(duration*1000)));
    if(!end.value||Number(end.value)<=0||Number(end.value)>duration)
      end.value=duration.toFixed(2);
    updateClock();draw();
    message("Выберите отрезок и при необходимости разметьте 4 угла площадки.");
  });
  video.addEventListener("error",()=>{
    if(valid())message("Формат/кодек не поддерживается браузером. Для разметки нужен воспроизводимый MP4 или WebM.",true);
  });
  video.addEventListener("timeupdate",updateClock);
  video.addEventListener("play",()=>{playButton.textContent="Пауза";});
  video.addEventListener("pause",()=>{playButton.textContent="▶ Воспроизвести";});
  playButton.addEventListener("click",()=>{
    if(!valid())return;
    if(video.paused){
      const started=video.play();
      if(started&&started.catch)started.catch(error=>message("Не удалось воспроизвести: "+error.message,true));
    }else video.pause();
  });
  seek.addEventListener("input",()=>{
    if(duration>0)video.currentTime=clamp(Number(seek.value)/1000,0,duration);
    updateClock();
  });
  const current=()=>clamp(Number.isFinite(video.currentTime)?video.currentTime:0,0,duration).toFixed(2);
  $("studio-analysis-set-start").addEventListener("click",()=>{
    begin.value=current();markModified();
  });
  $("studio-analysis-set-end").addEventListener("click",()=>{
    end.value=current();markModified();
  });
  [begin,end].forEach(node=>node.addEventListener("input",markModified));
  save.addEventListener("click",async()=>{
    if(!valid())return;
    const from=Number(begin.value),to=Number(end.value);
    if(!Number.isFinite(from)||from<0||!Number.isFinite(to)||to<=from
      ||(duration>0&&to>duration+.5)){
      message("Конец должен быть позже начала и не выходить за длительность видео.",true);return;
    }
    if(corners.length!==0&&corners.length!==4){
      message("Отметьте все 4 угла площадки или сбросьте разметку.",true);return;
    }
    const token=generation,epoch=editEpoch;
    const court=corners.map(p=>[...p]);
    save.disabled=true;
    try{
      const result=await api("/api/video-edit",{method:"POST",headers:{"Content-Type":"application/json"},
        body:JSON.stringify({project_id:project,video:source,trim_start_sec:from,
          trim_end_sec:to,court,net})});
      if(token===generation){
        const data={...result,trim_start_sec:from,trim_end_sec:to,court,net};
        editCache.write(project,source,data);
        window.dispatchEvent(new CustomEvent("volley:court-calibration-updated",{detail:{project_id:project,video:source}}));
        baseRevision=revision(data);
        if(epoch===editEpoch){dirty=false;removeDraft();}else{flushDraft();}
        message("Обрезка и разметка площадки сохранены.");
      }
    }catch(error){
      if(token===generation)message(error.message,true);
    }finally{save.disabled=!valid();}
  });
  const load=async(projectId,videoName,reuse=false)=>{
    if(reuse&&project===projectId&&source===videoName&&(settingsLoaded||settingsLoading))return;
    flushDraft();
    const token=++generation;
    dirty=false;baseRevision="";
    settingsLoaded=false;settingsLoading=false;
    video.pause();
    video.removeAttribute("src");
    video.load();
    project=projectId||"";source=videoName||"";corners=[];net=null;
    duration=0;begin.value="0";end.value="0";seek.value="0";seek.max="1000";
    media.style.setProperty("--frame-ratio","1.77778");
    zoom=0.84;setMarking(false);
    updateClock();syncControls();draw();
    if(!valid()){message("Выберите видео проекта.");return;}
    const pending=readDraft(project,source);
    const cached=pending?null:editCache.read(project,source);
    message(cached?"Восстановлены сохранённые настройки видео.":"Загрузка настроек видео…");
    settingsLoading=true;
    try{
      let data=cached||await api(url("/api/video-edit",{project_id:project,video:source}));
      if(token!==generation)return;
      if(!cached)editCache.write(project,source,data);
      baseRevision=revision(data);
      const restored=pending&&pending.base===baseRevision;
      if(restored){data=pending.value;dirty=true;}else if(pending){removeDraft();}
      begin.value=String(data.trim_start_sec??0);
      end.value=data.trim_end_sec==null?"0":String(data.trim_end_sec);
      corners=Array.isArray(data.court)?data.court.filter(p=>
        Array.isArray(p)&&p.length===2&&p.every(v=>Number.isFinite(v)&&v>=-0.5&&v<=1.5)):[];
      net=Array.isArray(data.net)&&data.net.length===2?data.net:null;
      video.src=url("/api/video-source",{project_id:project,video:source});
      video.load();
      settingsLoaded=true;
      updateClock();draw();message(restored?"Восстановлен несохранённый черновик. Нажмите «Сохранить» для применения.":"Разметка загружена. Видео читается по частям.");
    }catch(error){
      if(token===generation)message(error.message,true);
    }finally{
      if(token===generation)settingsLoading=false;
    }
  };
  window.addEventListener("volley:tab-change",event=>{
    if(event.detail?.tab!=="analysis")video.pause();
  });
  window.addEventListener("resize",draw);
  if(window.ResizeObserver){
    const ro=new ResizeObserver(draw);
    ro.observe(stage);ro.observe(frame);
  }
  syncControls();
  window.VolleyballEditor={load};
})();