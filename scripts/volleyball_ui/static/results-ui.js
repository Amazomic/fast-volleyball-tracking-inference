"use strict";
/* Analysis results: watch cached H.264, display event overlays and play rally bounds. */
window.VolleyballResultsUI=(()=>{
  const E=id=>document.getElementById(id);
  // Keep legacy running servers usable until their static route is restarted.
  const modelTouches=window.VolleyballModelTouches||{
    init:()=>{const bar=document.querySelector(".result-preview-mode-bar");if(bar)bar.hidden=true;},
    reset:()=>{},setSource:()=>{},active:()=>false,update:()=>{}
  };
  const v=()=>E("result-video-player");
  const esc=s=>String(s??"").replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
  async function readResultJson(url,fallback="Не удалось загрузить результаты"){
    if(window.VolleyDataCache?.supports(url))return window.VolleyDataCache.get(url);
    const response=await fetch(url,{cache:"no-store"});
    const body=await response.json();
    if(!response.ok)throw Error(body.error||fallback);
    return body;
  }
  const clock=t=>{
    t=Math.max(0,Math.floor(t));
    return [Math.floor(t/3600),Math.floor(t%3600/60),t%60].map(x=>String(x).padStart(2,"0")).join(":");
  };
  const actionText={attack:"Атака",block:"Блок",serve:"Подача",dig:"Приём",overhead_pass:"Верхняя передача",low_touch:"Касание",unknown_touch:"Касание"};
  const typeToControl={attack:"attacks",block:"blocks",serve:"serves",dig:"digs",overhead_pass:"passes",low_touch:"touches",unknown_touch:"touches"};
  const overlayDefaults={touches:true,attacks:true,blocks:true,serves:true,digs:true,passes:true,trajectory:false,players:true};
  const overlays={...overlayDefaults};
  const rawOverlays={vballnet:true,ravelBall:true,players:true,touch:true};
  const rawThresholds={vballnet:.35,ravelBall:.35,players:.3};
  // Golden CPU values are version-controlled and shared with automatic GPU/quick-reel replay.
  let goldenCpuPromise=null;
  let loadedGoldenCpu=null;
  let forcedCpuThresholds=null;
  function goldenCpuDefaults(){
    if(!goldenCpuPromise)goldenCpuPromise=fetch("/cpu-defaults.json",{cache:"no-store"})
      .then(async response=>{
        if(!response.ok)throw Error("Не удалось получить эталон CPU");
        const payload=await response.json();
        if(payload.schema!=="volley-cpu-golden-defaults/1"||!payload.settings)
          throw Error("Неизвестный формат эталона CPU");
        loadedGoldenCpu=payload.settings;
        return payload.settings;
      }).catch(error=>{goldenCpuPromise=null;throw error;});
    return goldenCpuPromise;
  }
  function applyCpuSettings(settings){
    for(const [id,key] of [
      ["result-reprocess-mode","mode"],
      ["result-reprocess-touch-threshold","touch_threshold"],
      ["result-touch-min-interval","touch_min_interval_sec"],
      ["result-touch-max-reach","touch_max_reach_ratio"],
      ["result-track-min-duration","track_min_duration"],
      ["result-track-max-distance","track_max_distance"],
      ["result-player-hold-frames","player_hold_frames"],
      ["result-player-match-distance","player_match_distance"],
      ["result-track-player-threshold","track_player_threshold"],
      ["result-track-ball-threshold","track_ball_threshold"]]){
      if(settings[key]!=null)E(id).value=String(settings[key]);
    }
    E("result-track-no-ball-fill").checked=settings.track_no_ball_fill===true;
    E("result-track-beach").checked=settings.beach===true;
    syncReprocessControls();
  }
  const rawTrail={seconds:2};
  let rawFilterTimer=null;
  // Raw GPU predictions are mutually exclusive by model; never combine sources.
  let rawSource="vballnet";
  const rawEnabled=kind=>rawOverlays[kind]&&!(kind==="touch"&&data?.capabilities?.model_touch===false)&&
    (kind==="vballnet"?rawSource==="vballnet":rawSource==="ravel");
  const rawWindows={vballnet:null,ravelBall:null};
  const rawRequests={vballnet:false,ravelBall:false};
  let rawEpoch=0;
  const rawCacheId=()=>String(selectedCaches[source]||"").split(".",1)[0];
  const inRawMode=()=>modelTouches.active();
  const showingPlayers=()=>inRawMode()?rawEnabled("players"):overlays.players;
  function resetRawWindows(){
    rawEpoch++;
    for(const key of Object.keys(rawWindows)){
      rawWindows[key]=null;rawRequests[key]=false;
    }
    E("result-raw-ball-trail")?.setAttribute("hidden","");
    for(const id of ["result-raw-vball","result-raw-ravel"]){
      const element=E(id);
      if(element)element.hidden=true;
    }
  }
  function cpuSourceLabel(){
    if(data?.inference_ready&&!data.analyzed)
      return "CPU: обработка ещё не выполнена";
    const modes={
      combined:"CPU: мяч VballNet; игроки и Touch RAVEL-013; геометрия касаний",
      baseline:"CPU: мяч VballNet; игроки RAVEL-013; геометрия касаний",
      "ravel-touch":"CPU: мяч, игроки и Touch RAVEL-013",
      "ravel-geometry":"CPU: мяч и игроки RAVEL-013; геометрия касаний"
    };
    return selectedPostprocessSettings
      ?modes[selectedPostprocessSettings.mode]||"CPU: сохранённая версия обработки"
      :"CPU: старый полный анализ (алгоритм событий не указан в метаданных)";
  }
  function updateSourceMode(){
    const raw=inRawMode();
    const sources=data?.capabilities?.raw_sources;
    if(sources&&!sources[rawSource])rawSource=Object.keys(sources).find(key=>sources[key])||"vballnet";
    const rawControls=E("result-raw-controls");
    if(rawControls)rawControls.hidden=!raw;
    E("result-raw-source-switch").hidden=!raw;
    for(const input of document.querySelectorAll('[name="result-raw-source"]')){
      input.checked=input.value===rawSource;
      input.disabled=!data||(sources&&!sources[input.value]);
    }
    for(const group of document.querySelectorAll("[data-result-raw-group]"))
      group.hidden=group.dataset.resultRawGroup!==rawSource;
    const description=E("result-source-explainer");
    if(description)description.textContent=raw
      ?"Нейросети (GPU): "+(rawSource==="vballnet"
        ?"VballNetV4c: только сохранённые обнаружения мяча. Данные RAVEL-013 скрыты."
        :"RAVEL-013: сохранённые обнаружения мяча, игроков и Touch. Данные VballNetV4c скрыты. Это не подтверждённые CPU-касания.")
      :cpuSourceLabel()+". Показанные события взяты из сохранённой CPU-версии.";
    const badge=document.querySelector(".result-annotation-badge");
    if(badge)badge.textContent=raw
      ?"Обнаружения · "+(rawSource==="vballnet"?"VballNetV4c":"RAVEL-013")
      :"События CPU";
    for(const key of Object.keys(rawOverlays)){
      const box=document.querySelector('[data-result-raw="'+key+'"]');
      if(box){
        box.checked=rawOverlays[key]&&!(key==="touch"&&data?.capabilities?.model_touch===false);
        box.disabled=!data||(key==="touch"&&data?.capabilities?.model_touch===false);
      }
    }
    E("result-raw-trail-seconds").disabled=!rawOverlays[rawSource==="vballnet"?"vballnet":"ravelBall"];
    modelTouches.setTouchVisible?.(raw&&rawEnabled("touch"));
    resetRawWindows();
    syncRawFilterStatus();
    resetPlayerBoxes();
    updateOverlay();
  }
  let selectedCaches={}, montageRunId="",selectedPostprocessSettings=null;
  let selectionFingerprint="",loadedFingerprint="";
  let reprocessBusy=false;
  // Individual rally selections are scoped to the exact project and analysis run.
  const montageSelectionKey=(pid=project,runId=montageRunId)=>
    "volley-montage-rallies:"+pid+":"+runId;
  function montageRallies(pid=project,runId=montageRunId){
    if(!pid||!runId)return [];
    let saved=[];
    try{saved=JSON.parse(localStorage.getItem(montageSelectionKey(pid,runId))||"[]")}catch{}
    return Array.isArray(saved)?[...new Set(saved.filter(id=>
      typeof id==="string"&&/^[0-9a-f]{24}$/.test(id)))]:[];
  }
  function setMontageRallies(ids,pid=project,runId=montageRunId){
    if(!pid||!runId)return;
    const next=[...new Set(ids)].filter(id=>
      typeof id==="string"&&/^[0-9a-f]{24}$/.test(id)).slice(0,500).sort();
    const previous=montageRallies(pid,runId).sort();
    if(next.length===previous.length&&next.every((id,i)=>id===previous[i]))return;
    try{localStorage.setItem(montageSelectionKey(pid,runId),JSON.stringify(next))}catch{}
    if(project===pid&&montageRunId===runId){
      E("result-rally-list").querySelectorAll("[data-rally-mark]").forEach(entry=>{
        entry.classList.toggle("is-selected",next.includes(entry.dataset.rallyMark));
      });
    }
    window.dispatchEvent(new CustomEvent("volley:montage-rallies-change",{
      detail:{project_id:pid,run_id:runId,rally_ids:next}
    }));
  }
  let trailWindow=null,trailRequestKey="",trailRequest=false,trailEpoch=0,trailUnavailable=false;
  // Null means the original detector output; a number filters stored raw candidates.
  let ballThreshold=null,ballThresholdTimer=null;
  let playerWindow=null,playerRequestKey="",playerRequesting=false;
  let playerEpoch=0,playerUnavailable=false,lastPlayerFrame=-1;
  let playerThreshold=null,playerThresholdTimer=null;
  function resetPlayerBoxes(){
    playerEpoch++;playerWindow=null;playerRequestKey="";
    playerRequesting=false;playerUnavailable=false;lastPlayerFrame=-1;
    E("result-player-boxes").replaceChildren();
  }
  async function requestPlayerBoxes(seconds){
    if(!showingPlayers()||playerUnavailable||!project||!source||!data)return;
    if(playerWindow&&seconds>=playerWindow.start&&seconds<playerWindow.end-1)return;
    const start=Math.max(0,Math.floor(seconds/5)*5);
    const key=project+"|"+source+"|"+start+"|"+
      (inRawMode()?rawThresholds.players:playerThreshold)+"|"+inRawMode();
    if(playerRequesting&&playerRequestKey===key)return;
    if(playerRequesting)return;
    playerRequesting=true;playerRequestKey=key;
    const epoch=playerEpoch;
    const q=new URLSearchParams({project_id:project,video:source,
      cache_id:inRawMode()?rawCacheId():(selectedCaches[source]||""),
      start:String(start),length:"6"});
    if(inRawMode())q.set("threshold",String(rawThresholds.players));
    else if(playerThreshold!==null)
      q.set("threshold",String(playerThreshold));
    try{
      const result=await readResultJson("/api/analysis-player-boxes?"+q,
        "Не удалось загрузить рамки");
      if(epoch!==playerEpoch||!showingPlayers()||!data)return;
      if(!result.available){playerUnavailable=true;return;}
      playerWindow={start,end:start+6,fps:Number(result.fps)||30,
        width:Number(result.width),height:Number(result.height),
        frames:new Map(result.frames||[]),
        labels:result.player_labels||{},
        groups:result.candidate_groups||{}};
      lastPlayerFrame=-1;
      updateOverlay();
    }catch(e){
      if(epoch===playerEpoch){playerUnavailable=true;console.warn("Рамки RAVEL:",e);}
    }finally{
      if(epoch===playerEpoch&&playerRequestKey===key)playerRequesting=false;
    }
  }
  function paintPlayerBoxes(time,geometry){
    const panel=E("result-player-boxes");
    if(!showingPlayers()||!geometry||!playerWindow||playerUnavailable){
      if(panel.childElementCount)panel.replaceChildren();
      lastPlayerFrame=-1;
      return;
    }
    const win=playerWindow;
    if(time<win.start||time>=win.end){panel.replaceChildren();lastPlayerFrame=-1;return;}
    const frame=Math.round(time*win.fps);
    let selectedFrame=frame,items=win.frames.get(frame);
    if(!items){
      // One or two absent frames are normal for sparse RAVEL sampling.
      const tolerance=Math.max(2,Math.round(win.fps*.10));
      for(let delta=1;delta<=tolerance;delta++){
        const before=win.frames.get(frame-delta),after=win.frames.get(frame+delta);
        if(before||after){selectedFrame=before?frame-delta:frame+delta;items=before||after;break;}
      }
    }
    if(lastPlayerFrame===selectedFrame)return;
    lastPlayerFrame=selectedFrame;
    const width=win.width,height=win.height;
    if(!(width>0&&height>0)){panel.replaceChildren();return;}
    const children=[];
    for(const item of (items||[]).slice(0,16)){
      if(!Array.isArray(item)||item.length<6)continue;
      const [id,x1,y1,x2,y2,score]=item.map(Number);
      if(![id,x1,y1,x2,y2,score].every(Number.isFinite)||x2<=x1||y2<=y1)continue;
      if(inRawMode()&&score<rawThresholds.players)continue;
      const box=document.createElement("div");
      box.className="result-player-box";
      const hue=id<0?36:(Math.floor(id)*53+178)%360;
      box.style.setProperty("--player-box-hue",String(hue));
      box.style.left=(100*x1/width)+"%";
      box.style.top=(100*y1/height)+"%";
      box.style.width=(100*(x2-x1)/width)+"%";
      box.style.height=(100*(y2-y1)/height)+"%";
      const tag=document.createElement("span");
      tag.className="result-player-box-tag";
      const identity=id>=0?win.labels[String(Math.floor(id))]:null;
      const candidate=id>=0?win.groups[String(Math.floor(id))]:null;
      tag.textContent=identity
        ?identity.name+(identity.source==="reid_auto"?" (авто)":"")+" · #"+Math.floor(id)
        :id<0?"Кандидат · "+Math.round(score*100)+"%"
          :"ID "+Math.floor(id)+" · "+Math.round(score*100)+"%";
      tag.title=id<0
        ?"RAVEL · сырая рамка без подтверждённого Track ID"
        :"RAVEL · Track ID "+Math.floor(id)+
          (identity?" · игрок: "+identity.name+
            (identity.source==="manual_merge"?" (ручное объединение)":
              identity.source==="reid_auto"?" (автопривязка не проверена)":""):"")+
          (candidate&&!identity?" · группа: "+candidate:"")+
          " · "+Math.round(score*100)+"%";
      box.append(tag);children.push(box);
    }
    panel.replaceChildren(...children);
  }
  function syncRawFilterStatus(){
    const target=E("result-raw-filter-status");
    if(!target)return;
    const kind=rawSource==="vballnet"?"vballnet":"ravelBall";
    const win=rawWindows[kind];
    // Old analyses can lack confidence sidecars: do not offer an inert slider.
    E("result-raw-vball-threshold").disabled=
      rawSource==="vballnet"&&Boolean(win?.available&&win.supports_threshold===false);
    const threshold=Math.round(rawThresholds[kind]*100)+"%";
    if(!win){
      target.textContent="Фильтры действуют только на отображение сохранённых обнаружений. След без интерполяции.";
    }else if(!win.available){
      target.textContent="Нет сохранённых обнаружений выбранной модели для этого видео.";
    }else if(kind==="vballnet"&&win.supports_threshold===false){
      target.textContent="Нет сохранённых оценок VballNet: фильтр уверенности недоступен для этого анализа.";
    }else{
      const stored=Number(win.stored_threshold);
      const floor=Number.isFinite(stored)?Math.round(stored*100)+"%":"";
      target.textContent="Порог мяча: "+threshold+
        (kind==="ravelBall"&&floor?" · порог записи RAVEL: "+floor:"")+
        ". Ниже порога сохранения обнаружения восстановить нельзя. След показывает только точки нейросети.";
    }
  }
  function rawBallPoints(kind,win){
    const threshold=rawThresholds[kind];
    if(kind==="vballnet"){
      return (win.points||[]).filter(p=>Array.isArray(p)&&p.length>=3&&
        Number.isFinite(p[0])&&Number.isFinite(p[1])&&Number.isFinite(p[2])&&
        (p.length<4||Number.isFinite(p[3])&&p[3]>=threshold));
    }
    return (win.frames||[]).flatMap(row=>{
      if(!Array.isArray(row)||!Number.isFinite(row[0]))return [];
      // Each RAVEL frame may contain multiple candidate balls; use the best.
      const p=(row[1]||[]).find(p=>Array.isArray(p)&&p.length>=3&&
        p.slice(0,3).every(Number.isFinite)&&p[2]>=threshold);
      return p?[[row[0],p[0],p[1],p[2]]]:[];
    });
  }
  function drawRawBallTrail(kind,win,time,geometry,points){
    const svg=E("result-raw-ball-trail");
    svg.setAttribute("hidden","");
    if(!rawEnabled(kind)||!win?.available||!geometry)return;
    const fps=Number(win.fps)||30,currentFrame=Math.round(time*fps);
    const first=currentFrame-Math.round(rawTrail.seconds*fps);
    const width=Number(win.width)||geometry.sourceW;
    const height=Number(win.height)||geometry.sourceH;
    const history=points.filter(p=>p[0]<=currentFrame&&p[0]>=first&&
      p[1]>=0&&p[1]<=width&&p[2]>=0&&p[2]<=height);
    if(history.length<2||currentFrame-history[history.length-1][0]>Math.max(2,Math.round(fps*.15)))return;
    const segments=[];
    let prev=null;
    for(const p of history){
      const command=prev!==null&&p[0]-prev<=Math.max(3,Math.round(fps*.12))?"L":"M";
      segments.push(command+p[1].toFixed(1)+" "+p[2].toFixed(1));
      prev=p[0];
    }
    svg.setAttribute("viewBox","0 0 "+width+" "+height);
    svg.dataset.source=kind;
    E("result-raw-ball-trail-path").setAttribute("d",segments.join(" "));
    const last=history[history.length-1];
    E("result-raw-ball-trail-head").setAttribute("cx",last[1]);
    E("result-raw-ball-trail-head").setAttribute("cy",last[2]);
    svg.removeAttribute("hidden");
  }
  async function requestRawWindow(kind,time){
    if(!inRawMode()||!rawEnabled(kind)||!project||!source||!data||
       rawRequests[kind])return;
    const current=rawWindows[kind];
    if(current&&time>=current.start&&time<current.start+current.length-.1)return;
    rawRequests[kind]=true;
    // Include a lookback so the neural trail survives chunk boundaries.
    const epoch=rawEpoch,start=Math.max(0,Math.floor(time/8)*8-3);
    const query=new URLSearchParams({
      project_id:project,video:source,cache_id:rawCacheId(),
      start:String(start),length:"12"
    });
    const endpoint=kind==="ravelBall"
      ?"/api/analysis-ravel-ball":"/api/analysis-trajectory";
    if(kind==="ravelBall"){
      query.set("threshold",String(rawThresholds.ravelBall));
    }else{
      query.set("threshold",String(rawThresholds.vballnet));
      query.set("scores","1");
    }
    try{
      let result=await readResultJson(endpoint+"?"+query,
        "Не удалось прочитать обнаружения мяча");
      if(kind==="vballnet"&&!result.available&&result.reason==="no_raw_candidates"){
        // Legacy runs still show the original saved points, but cannot
        // claim that a confidence filter was applied.
        query.delete("threshold");
        query.delete("scores");
        result=await readResultJson(endpoint+"?"+query,
          "Не удалось прочитать обнаружения мяча");
        result.supports_threshold=false;
      }
      if(epoch!==rawEpoch||!inRawMode()||!rawEnabled(kind))return;
      rawWindows[kind]=result.available?{...result,start,length:12}:{
        available:false,start,length:12,reason:result.reason
      };
      syncRawFilterStatus();
      updateOverlay();
    }catch(error){
      if(epoch===rawEpoch){
        rawWindows[kind]={available:false,start,length:12,reason:error.message};
        syncRawFilterStatus();
        console.warn("Сохранённые обнаружения",kind,error);
      }
    }finally{
      if(epoch===rawEpoch)rawRequests[kind]=false;
    }
  }
  function paintRawBalls(time,geometry){
    E("result-raw-ball-trail").setAttribute("hidden","");
    for(const [kind,id] of [["vballnet","result-raw-vball"],
                             ["ravelBall","result-raw-ravel"]]){
      const badge=E(id);
      badge.hidden=true;
      if(!rawEnabled(kind)||!geometry)continue;
      const win=rawWindows[kind];
      if(!win?.available||time<win.start||time>=win.start+win.length){
        requestRawWindow(kind,time);
        continue;
      }
      const frame=Math.round(time*(Number(win.fps)||30));
      const points=rawBallPoints(kind,win);
      drawRawBallTrail(kind,win,time,geometry,points);
      const near=points.filter(p=>Math.abs(p[0]-frame)<=2);
      if(!near.length)continue;
      const p=near.reduce((a,b)=>Math.abs(a[0]-frame)<Math.abs(b[0]-frame)?a:b);
      const width=Number(win.width)||geometry.sourceW;
      const height=Number(win.height)||geometry.sourceH;
      if(!(width>0&&height>0&&p[1]>=0&&p[1]<=width&&p[2]>=0&&p[2]<=height))continue;
      const confidence=p.length>3&&Number.isFinite(p[3])?p[3]:null;
      badge.style.left=(100*p[1]/width)+"%";
      badge.style.top=(100*p[2]/height)+"%";
      badge.textContent=(kind==="vballnet"?"VballNet":"RAVEL")+
        (confidence!==null?" · "+Math.round(confidence*100)+"%":" · мяч");
      badge.title=(kind==="vballnet"?"VballNetV4c":"RAVEL-013")+
        " · обнаружение мяча, сохранённое нейросетью; не CPU-трек";
      badge.hidden=false;
    }
  }
  const overlayKey=()=>"volley-result-overlays:"+project;
  const colors={touches:"#f7fcff",attacks:"#ff745c",blocks:"#b98cff",serves:"#25dbea",digs:"#ffca4c",passes:"#72a8ff"};
  // Display every event from the selected, already processed analysis run.
  // The checkboxes only control visibility, never re-filter confidence.
  const classifiedVisible=a=>
    !["low_touch","unknown_touch"].includes(a.type)&&Boolean(overlays[typeToControl[a.type]]);
  // Generic touches include every inferred contact, not only "low_touch".
  const actionVisible=a=>overlays.touches||Boolean(overlays[typeToControl[a.type]]);
  // CPU replay always applies a single threshold to every video in the selected run.
  const ballThresholdKey=()=> "volley-ball-threshold:"+project+":"+montageRunId;
  const replayModes={
    baseline:"Мяч: VballNet. Касания: геометрия. Предсказания Touch не используются.",
    "ravel-touch":"Мяч: RAVEL-013. Касания: пики Touch после сглаживания 5 кадров и подавления соседних пиков; рядом с игроком и мячом.",
    "ravel-geometry":"Мяч: RAVEL-013. Касания: геометрия мяча и игроков, без предсказаний Touch.",
    combined:"Мяч: VballNet. Касания: геометрия + пики Touch RAVEL-013 с проверкой близости к мячу и игроку."
  };
  function syncReprocessControls(){
    E("analysis-postprocessing").hidden=!project;
    const button=E("result-reprocess-start");
    const ready=Boolean(project&&source&&data&&montageRunId);
    const modes=data?.capabilities?.reprocess_modes||["baseline","ravel-touch","ravel-geometry","combined"];
    const canReplay=ready&&modes.length>0;
    const modeSelect=E("result-reprocess-mode");
    for(const option of Array.from(modeSelect.options||[]))option.disabled=!modes.includes(option.value);
    if(ready&&!modes.includes(modeSelect.value))modeSelect.value=modes[0]||"";
    for(const id of ["result-reprocess-mode","result-track-min-duration","result-track-max-distance",
      "result-player-hold-frames","result-player-match-distance","result-track-player-threshold",
      "result-track-ball-threshold","result-track-no-ball-fill","result-track-beach","result-reprocess-reset"])
      E(id).disabled=!canReplay;
    for(const id of rallyFields)E(id).disabled=!ready||!data?.analyzed;
    for(const id of ["result-rally-sort","result-playback-mode"])
      E(id).disabled=!ready||!data?.analyzed||!data.rallies?.length;
    for(const control of document.querySelectorAll('[name="result-preview-mode"]'))
      control.disabled=!ready||(control.value==="events"&&!data?.analyzed)||
        (control.value==="model"&&data?.capabilities&&!Object.values(data.capabilities.raw_sources).some(Boolean));
    for(const control of document.querySelectorAll("[data-result-overlay]"))
      control.disabled=!ready||!data?.analyzed;
    const mode=E("result-reprocess-mode")?.value||"combined";
    const usesTouch=mode==="combined"||mode==="ravel-touch";
    const usesVball=mode==="baseline"||mode==="combined";
    E("result-track-no-ball-fill").disabled=!canReplay||mode==="combined";
    const touchInput=E("result-reprocess-touch-threshold");
    E("result-touch-validation-settings").hidden=!usesTouch;
    const floor=Number(data?.capabilities?.touch_floor)||0;
    touchInput.min=String(floor);
    if(ready&&usesTouch&&Number(touchInput.value)<floor)touchInput.value=String(floor);
    E("result-touch-min-interval").disabled=!canReplay||!usesTouch;
    E("result-touch-max-reach").disabled=!canReplay||!usesTouch;
    touchInput.disabled=!canReplay||!usesTouch;
    E("result-ball-threshold").disabled=!canReplay||!usesVball||!replayBallCapability()?.available;
    E("result-ball-threshold-reset").disabled=E("result-ball-threshold").disabled||ballThreshold===null;
    touchInput.closest(".field")?.classList.toggle("is-irrelevant",!usesTouch);
    E("result-reprocess-mode-hint").textContent=replayModes[mode]||"";
    if(button)button.disabled=reprocessBusy||!canReplay;
    const target=E("result-reprocess-status");
    if(!target||!data||reprocessBusy)return;
    if(!canReplay){target.textContent="В выбранной версии нет полного кэша нейросетей для пересчёта. Выполните первичный анализ.";return;}
    const baselineBall=Number(replayBallCapability()?.analysis_threshold)||.5;
    const baselinePlayer=Number(data.player_detection?.analysis_threshold)||.5;
    const notes=[
      "Мяч: "+(usesVball?"VballNet "+Math.round((ballThreshold??baselineBall)*100)+"%":"RAVEL (исходные предсказания)"),
      "Игроки: RAVEL "+Math.round((playerThreshold??baselinePlayer)*100)+"%",
      "Touch: "+(usesTouch?Math.round(Number(touchInput.value)*100)+"%":"не применяется")
    ];
    if(usesTouch)notes.push("Touch: слияние "+E("result-touch-min-interval").value+
      " с · допуск "+E("result-touch-max-reach").value+" высоты игрока");
    if(!usesVball)notes.push("Порог VballNet на итог не влияет");
    else if(ballThreshold!==null)notes.push("VballNet: приближённый пересчёт по пикам");
    target.textContent=notes.join(" · ")+
      ". Нажмите «Пересчитать проект», чтобы сохранить отдельную версию со всеми розыгрышами.";
  }
  function setReprocessTask(info){
    reprocessBusy=info?.status==="running";
    syncReprocessControls();
  }
  const replayBallCapability=()=>data?.raw_ball_detection||data?.ball_detection;
  function restoreBallThreshold(){
    const available=Boolean(replayBallCapability()?.available);
    const baseline=Number(replayBallCapability()?.analysis_threshold)||.5;
    let saved=null;
    try{
      const raw=localStorage.getItem(ballThresholdKey());
      if(raw!==null&&raw!=="")saved=Number(raw);
    }catch{}
    ballThreshold=available&&saved!==null&&Number.isFinite(saved)
      &&saved>=.05&&saved<=.95?saved:null;
    E("result-ball-threshold").disabled=!available;
    E("result-ball-threshold").value=String(ballThreshold??baseline);
    E("result-ball-threshold-reset").disabled=!available||ballThreshold===null;
    E("result-ball-threshold-value").textContent=
      Math.round((ballThreshold??baseline)*100)+"%";
    E("result-ball-threshold-status").textContent=available
      ?"Меняет детекции мяча и траекторию без нейросети. События и границы розыгрышей не пересчитываются."
      :data?.ball_detection===undefined
        ?"Сервер работает со старым API. Перезапустите Volley Studio, чтобы загрузить сохранённые оценки мяча."
        :"В выбранном запуске нет сырых оценок мяча. Проверьте выбранный запуск анализа.";
    if(available&&ballThreshold!==null&&!overlays.trajectory){
      overlays.trajectory=true;
      const toggle=document.querySelector('[data-result-overlay="trajectory"]');
      if(toggle)toggle.checked=true;
    }
    invalidateTrail();
    syncReprocessControls();
  }
  function setBallThreshold(value){
    if(ballThresholdTimer!==null){clearTimeout(ballThresholdTimer);ballThresholdTimer=null;}
    if(!replayBallCapability()?.available)return;
    if(value!==null){
      value=Number(value);
      if(!Number.isFinite(value)||value<.05||value>.95)return;
      value=Math.round(value*100)/100;
    }
    if(value!==null&&ballThreshold===value)return;
    ballThreshold=value;
    const baseline=Number(replayBallCapability().analysis_threshold)||.5;
    E("result-ball-threshold").value=String(value??baseline);
    E("result-ball-threshold-value").textContent=Math.round((value??baseline)*100)+"%";
    E("result-ball-threshold-reset").disabled=value===null;
    try{
      if(value===null)localStorage.removeItem(ballThresholdKey());
      else localStorage.setItem(ballThresholdKey(),String(value));
    }catch{}
    if(!overlays.trajectory){
      overlays.trajectory=true;
      document.querySelector('[data-result-overlay="trajectory"]').checked=true;
      persistOverlays();
    }
    // The raw RAVEL Touch view suppresses the ball overlay; switch to the
    // event/trajectory view when adjusting the VballNet detector threshold.
    if(modelTouches.active()){
      const eventsMode=document.querySelector('[name="result-preview-mode"][value="events"]');
      if(eventsMode&&!eventsMode.disabled&&!eventsMode.checked){
        eventsMode.checked=true;
        eventsMode.dispatchEvent(new Event("change",{bubbles:true}));
      }
    }
    invalidateTrail();
    E("result-ball-threshold-status").textContent=value===null
      ?"Исходные детекции анализа восстановлены. События и розыгрыши не пересчитываются."
      :"Порог "+Math.round(value*100)+"%: показываем сохранённые кандидаты. События и розыгрыши не пересчитываются.";
    updateOverlay();
    if(data?.ball_detection?.available)requestTrail(v().currentTime||0);
    syncReprocessControls();
  }
  const playerThresholdKey=()=>"volley-player-threshold:"+project+":"+montageRunId;
  function restorePlayerThreshold(){
    const available=Boolean(data?.player_detection?.available);
    const baseline=Number(data?.player_detection?.analysis_threshold)||.5;
    const floor=Number(data?.player_detection?.minimum_confidence)||.05;
    let saved=null;
    try{
      const raw=localStorage.getItem(playerThresholdKey());
      if(raw!==null&&raw!=="")saved=Number(raw);
    }catch{}
    playerThreshold=available&&saved!==null&&Number.isFinite(saved)
      &&saved>=floor&&saved<=.95?saved:null;
    const rawField=E("result-raw-player-threshold");
    if(rawField){
      rawField.min=String(floor);
      if(rawThresholds.players<floor)rawThresholds.players=floor;
      rawField.value=String(rawThresholds.players);
      E("result-raw-player-value").textContent=
        Math.round(rawThresholds.players*100)+"%";
    }
    const field=E("result-player-threshold");
    field.min=String(floor);
    field.disabled=!available;
    field.value=String(playerThreshold??baseline);
    E("result-player-threshold-value").textContent=
      Math.round((playerThreshold??baseline)*100)+"%";
    E("result-player-threshold-reset").disabled=!available||playerThreshold===null;
    E("result-player-threshold-status").textContent=available
      ?"Сырые обнаружения RAVEL доступны. Порог можно менять без инференса; новые кандидаты пока без Track ID."
      :data?.player_detection===undefined
        ?"Сервер работает со старым API и не передаёт оценки RAVEL. Перезапустите Volley Studio — повторный анализ не требуется."
        :"В выбранном запуске нет сырых рамок RAVEL. Проверьте выбор запуска анализа слева.";
    if(available&&playerThreshold!==null){
      overlays.players=true;
      document.querySelector('[data-result-overlay="players"]').checked=true;
    }
    resetPlayerBoxes();
    syncReprocessControls();
  }
  function setPlayerThreshold(value){
    if(playerThresholdTimer!==null){clearTimeout(playerThresholdTimer);playerThresholdTimer=null;}
    if(!data?.player_detection?.available)return;
    if(value!==null){
      value=Number(value);
      const floor=Number(data.player_detection.minimum_confidence)||.05;
      if(!Number.isFinite(value)||value<floor||value>.95)return;
      value=Math.round(value*100)/100;
    }
    if(value!==null&&value===playerThreshold)return;
    playerThreshold=value;
    const baseline=Number(data.player_detection.analysis_threshold)||.5;
    E("result-player-threshold").value=String(value??baseline);
    E("result-player-threshold-value").textContent=Math.round((value??baseline)*100)+"%";
    E("result-player-threshold-reset").disabled=value===null;
    try{
      if(value===null)localStorage.removeItem(playerThresholdKey());
      else localStorage.setItem(playerThresholdKey(),String(value));
    }catch{}
    if(!overlays.players){
      overlays.players=true;
      document.querySelector('[data-result-overlay="players"]').checked=true;
      persistOverlays();
    }
    if(modelTouches.active()){
      const events=document.querySelector('[name="result-preview-mode"][value="events"]');
      if(events&&!events.disabled&&!events.checked){
        events.checked=true;
        events.dispatchEvent(new Event("change",{bubbles:true}));
      }
    }
    E("result-player-threshold-status").textContent=value===null
      ?"Восстановлены исходные Track ID анализа."
      :"Сырые рамки от "+Math.round(value*100)+"%. Непривязанные кандидаты без ID; события и розыгрыши не пересчитываются.";
    resetPlayerBoxes();
    updateOverlay();
    if(data)requestPlayerBoxes(v().currentTime||0);
    syncReprocessControls();
  }
  function renderEventCounts(){
    const counts={touches:0,attacks:0,blocks:0,serves:0,digs:0,passes:0};
    for(const a of data?.actions||[]){
      counts.touches++;
      const k=typeToControl[a.type];
      if(k&&k!=="touches")counts[k]++;
    }
    for(const [k,n] of Object.entries(counts)){
      const el=document.querySelector('[data-result-count="'+k+'"]');
      if(el)el.textContent=String(n);
    }
  }
  function trailStatus(message){
    // Keep the trail indicator stable. Transient "no ball" messages must
    // never add rows or change the player geometry during playback.
    const indicator=E("result-trajectory-indicator");
    if(!indicator)return;
    indicator.hidden=!overlays.trajectory;
    const caption=overlays.trajectory?"● Траектория включена":"";
    if(indicator.textContent!==caption)indicator.textContent=caption;
    indicator.title=message||"";
  }
  function suggestNextBall(time,win){
    const button=E("result-trajectory-jump");
    if(!button||!overlays.trajectory||!win){if(button)button.hidden=true;return}
    const frame=Math.floor(time*win.fps);
    const future=win.points.find(p=>p[0]>frame+6);
    const next=future?.[0]??win.nextAfterWindow;
    if(next!=null&&next>frame+6){
      button.dataset.frame=String(next);
      button.dataset.fps=String(win.fps);
      button.textContent="К мячу · "+clock(next/win.fps);
      button.hidden=false;
    }else button.hidden=true;
  }
  function restoreOverlays(){
    let saved={};
    try{saved=JSON.parse(localStorage.getItem(overlayKey())||"{}")||{}}catch{}
    for(const input of document.querySelectorAll("[data-result-overlay]")){
      const key=input.dataset.resultOverlay;
      overlays[key]=typeof saved[key]==="boolean"?saved[key]:overlayDefaults[key];
      input.checked=overlays[key];
    }
  }
  function persistOverlays(){
    try{localStorage.setItem(overlayKey(),JSON.stringify(overlays))}catch{}
  }
  function invalidateTrail(){
    trailEpoch++;trailWindow=null;trailRequest=false;trailRequestKey="";trailUnavailable=false;
    const svg=E("result-ball-trail");
    if(svg)svg.setAttribute("hidden","");
    E("result-trajectory-jump").hidden=true;
    trailStatus("Ожидание координат мяча…");
  }
  async function requestTrail(time){
    if(!overlays.trajectory||trailUnavailable||!data||!project||!source)return;
    if(trailWindow&&time>=trailWindow.start&&time<trailWindow.end-1)return;
    const start=Math.max(0,Math.floor(time/8)*8-1);
    const key=project+"|"+source+"|"+start+"|"+ballThreshold;
    if(trailRequest&&trailRequestKey===key)return;
    trailRequest=true;trailRequestKey=key;
    const epoch=trailEpoch;
    const params=new URLSearchParams({project_id:project,video:source,
      cache_id:selectedCaches[source]||"",start:String(start),length:"10"});
    if(ballThreshold!==null)params.set("threshold",String(ballThreshold));
    try{
      const result=await readResultJson("/api/analysis-trajectory?"+params,
        "Не удалось прочитать траекторию");
      if(epoch!==trailEpoch||!overlays.trajectory||!data)return;
      if(!result.available){
        trailUnavailable=true;trailWindow=null;
        trailStatus("Нет сохранённой траектории");return;
      }
      trailWindow={start,end:start+10,fps:Number(result.fps)||30,
        points:Array.isArray(result.points)?result.points:[],
        nextAfterWindow:result.next_after_window??null};
      updateOverlay();
    }catch(error){
      if(epoch===trailEpoch){
        trailUnavailable=true;trailStatus("Ошибка чтения траектории");
        console.warn("Траектория мяча недоступна:",error);
      }
    }finally{
      if(epoch===trailEpoch&&trailRequestKey===key)trailRequest=false;
    }
  }
  function ballGeometry(){
    const player=v(),layer=E("result-ball-layer"),overlay=E("result-overlay");
    if(!player.videoWidth||!player.videoHeight||!data||!overlay.offsetWidth)return null;
    const shell=overlay.getBoundingClientRect(),video=player.getBoundingClientRect();
    const zoomX=shell.width/Math.max(1,overlay.offsetWidth),zoomY=shell.height/Math.max(1,overlay.offsetHeight);
    if(!(zoomX>0&&zoomY>0))return null;
    const naturalW=player.videoWidth,naturalH=player.videoHeight;
    const sourceW=Number(data.coordinate_space?.width)||naturalW;
    const sourceH=Number(data.coordinate_space?.height)||naturalH;
    const shownW=Math.min(video.width,video.height*naturalW/naturalH);
    const shownH=shownW*naturalH/naturalW;
    if(!(sourceW>0&&sourceH>0&&shownW>0&&shownH>0))return null;
    layer.style.left=((video.left-shell.left+(video.width-shownW)/2)/zoomX)+"px";
    layer.style.top=((video.top-shell.top+(video.height-shownH)/2)/zoomY)+"px";
    layer.style.width=(shownW/zoomX)+"px";
    layer.style.height=(shownH/zoomY)+"px";
    return {sourceW,sourceH,shownW:shownW/zoomX,shownH:shownH/zoomY};
  }
  function drawTrajectory(time,geometry){
    const svg=E("result-ball-trail");
    svg.setAttribute("hidden","");
    if(!overlays.trajectory||!geometry||!trailWindow||trailUnavailable)return;
    const win=trailWindow;
    if(time<win.start||time>=win.end-.02)return;
    const fps=win.fps,currentFrame=Math.floor(time*fps+.01);
    const lastFrame=currentFrame-Math.round(fps*2.0);
    const pts=win.points.filter(p=>Array.isArray(p)&&p.length===3&&
      Number.isFinite(p[1])&&Number.isFinite(p[2])&&
      p[0]>=lastFrame&&p[0]<=currentFrame&&
      p[1]>=0&&p[1]<=geometry.sourceW&&p[2]>=0&&p[2]<=geometry.sourceH);
    if(!pts.length||currentFrame-pts[pts.length-1][0]>Math.round(fps*.6)){
      trailStatus("");
      suggestNextBall(time,win);
      return;
    }
    E("result-trajectory-jump").hidden=true;
    let path="",previousFrame=-100;
    for(const [frame,x,y] of pts){
      path+=(frame-previousFrame>4?"M":" L")+x.toFixed(2)+" "+y.toFixed(2);
      previousFrame=frame;
    }
    svg.setAttribute("viewBox","0 0 "+geometry.sourceW+" "+geometry.sourceH);
    E("result-ball-trail-path").setAttribute("d",path);
    const last=pts[pts.length-1];
    E("result-ball-trail-head").setAttribute("cx",last[1]);
    E("result-ball-trail-head").setAttribute("cy",last[2]);
    // SVGElement.hidden can be an expando on some browsers: it does NOT
    // reliably remove the HTML hidden attribute from an inline SVG.
    svg.removeAttribute("hidden");
    trailStatus("");
  }
  let project="",source="",data=null,segment=null,loadId=0,playing=false,rafId=null;
  let totalDuration=0,offset=0,sortedActions=[];
  let rallyOrder=[],rallySort="normal";
  let rallyRefreshTimer=null,rallyRefreshEpoch=0,appliedRallyKey="";
  const playbackModes=[
    {id:"sequence",icon:"▶▶",label:"По очереди",
     description:"Розыгрыши воспроизводятся один за другим"},
    {id:"loop",icon:"↻",label:"Повтор розыгрыша",
     description:"Выбранный розыгрыш повторяется по кругу"},
    {id:"once",icon:"■",label:"Один и стоп",
     description:"После выбранного розыгрыша воспроизведение остановится"},
    {id:"free",icon:"∞",label:"Свободно",
     description:"Видео воспроизводится без привязки к розыгрышам"}
  ];
  const playbackKey="volley-result-playback-mode";
  let playbackMode="sequence";
  function renderPlaybackMode(){
    const button=E("result-playback-mode");
    const active=playbackModes.find(mode=>mode.id===playbackMode)||playbackModes[0];
    const name=active.label;
    button.dataset.mode=active.id;
    E("result-playback-mode-icon").textContent=active.icon;
    E("result-playback-mode-name").textContent=name;
    button.title="Режим просмотра: "+name+". "+active.description+
      ". Нажмите, чтобы переключить режим.";
    button.setAttribute("aria-label",button.title);
  }
  function setPlaybackMode(mode,persist=true){
    if(!playbackModes.some(row=>row.id===mode))mode="sequence";
    playbackMode=mode;
    if(mode==="free"&&segment){
      segment=null;
      E("result-overlay-rally").hidden=true;
      syncRallySelection();
    }
    if(mode!=="free"&&!segment&&data?.rallies?.length){
      const time=sourceTime(v().currentTime||0);
      const selected=data.rallies.findIndex(r=>time>=r.start&&time<r.end);
      if(selected>=0){
        const r=data.rallies[selected];
        segment={index:selected,start:mediaTime(r.start),end:mediaTime(r.end)};
        E("result-overlay-rally").textContent="РОЗЫГРЫШ № "+(selected+1);
        E("result-overlay-rally").hidden=false;
        syncRallySelection();
      }
    }
    if(persist)try{localStorage.setItem(playbackKey,mode)}catch{}
    renderPlaybackMode();
  }
  function restorePlaybackMode(){
    let stored="sequence";
    try{stored=localStorage.getItem(playbackKey)||"sequence"}catch{}
    setPlaybackMode(stored,false);
  }
  function cyclePlaybackMode(){
    const index=playbackModes.findIndex(mode=>mode.id===playbackMode);
    setPlaybackMode(playbackModes[(index+1)%playbackModes.length].id);
  }
  function restoreRallySort(){
    try{rallySort=localStorage.getItem("volley-result-rally-sort")||"normal"}catch{}
    if(!["normal","duration","touches"].includes(rallySort))rallySort="normal";
    E("result-rally-sort").value=rallySort;
  }
  const rallyFields=["rally-gap-sec","rally-min-sec","rally-context-sec"];
  function rallyKey(cfg=rallyConfig()){
    return JSON.stringify([cfg.gap_sec,cfg.min_sec,cfg.context_sec]);
  }
  function rallyStatus(message){
    const box=E("result-rally-recalc-status");
    if(box)box.textContent=message;
  }
  function cancelRallyRefresh(){
    rallyRefreshEpoch++;
    if(rallyRefreshTimer!==null)clearTimeout(rallyRefreshTimer);
    rallyRefreshTimer=null;
  }
  function validRallyFields(){
    return rallyFields.every((id,i)=>{
      const input=E(id),value=String(input.value).trim(),number=Number(value);
      return value!==""&&Number.isFinite(number)&&number>=0&&
        number<=[12,40,10][i];
    });
  }
  // Preserve manual montage choices across a context-only boundary shift.
  // When the same detected game track is shown with different padding its
  // old rally_id changes; match stable detected bounds instead.
  function preserveSelectedRallies(before,after){
    const previous=new Set(montageRallies());
    if(!previous.size)return;
    const current=new Set(after.map(row=>row.rally_id).filter(Boolean));
    const next=new Set(previous);
    for(const old of before){
      if(!previous.has(old.rally_id)||current.has(old.rally_id))continue;
      const matches=after.filter(r=>r.rally_id&&
        old.detected_start!=null&&old.detected_end!=null&&
        r.detected_start===old.detected_start&&
        r.detected_end===old.detected_end);
      if(matches.length!==1)continue;
      next.delete(old.rally_id);
      next.add(matches[0].rally_id);
    }
    if(next.size!==previous.size||[...next].some(id=>!previous.has(id)))
      setMontageRallies([...next]);
  }
  function sameRallyWindows(before,after){
    return before.length===after.length&&before.every((r,i)=>
      Math.abs(Number(r.start)-Number(after[i].start))<.001&&
      Math.abs(Number(r.end)-Number(after[i].end))<.001);
  }
  async function refreshRallyBoundaries(){
    const epoch=++rallyRefreshEpoch;
    if(!project||!source||!data?.analyzed||!selectedCaches[source]){
      rallyStatus("Нужна готовая CPU-версия: сначала запустите CPU-постобработку.");
      return;
    }
    if(!validRallyFields()){
      rallyStatus("Введите корректные значения: разрыв 0–12 с, минимум 0–40 с, контекст 0–10 с.");
      return;
    }
    const cfg=rallyConfig(),key=rallyKey(cfg);
    if(key===appliedRallyKey){
      rallyStatus("Границы уже рассчитаны с этими параметрами.");
      return;
    }
    const pid=project,video=source,cache=selectedCaches[source],loaded=loadId;
    rallyStatus("Перестраиваем границы розыгрышей по сохранённым трекам CPU…");
    const query=new URLSearchParams({project_id:pid,video,cache_id:cache,...cfg});
    try{
      const result=await readResultJson("/api/analysis-rallies?"+query,
        "Не удалось пересчитать границы");
      if(epoch!==rallyRefreshEpoch||loaded!==loadId||pid!==project||
         video!==source||cache!==selectedCaches[source])return;
      if(!result.analyzed)throw Error("Для выбранной версии нет событий CPU");
      if(!result.reconstruction_available){
        // Legacy events may already contain fixed start/end records.
        rallyStatus("Нет сохранённых треков для перестройки границ в этой версии. "+
          "Запустите CPU-пересчёт из блока ниже.");
        return;
      }
      const oldRallies=data.rallies||[];
      const nextRallies=result.rallies||[];
      const unchanged=sameRallyWindows(oldRallies,nextRallies);
      const oldSelection=segment?oldRallies[segment.index]:null;
      const previousScroll=E("result-rally-list").scrollTop;
      // Update only metadata; never touch <video>.src/load/currentTime.
      data.rallies=nextRallies;
      data.actions=result.actions||[];
      data.rally_summary=result.rally_summary;
      E("result-meta").textContent=nextRallies.length+
        " возможных розыгрышей · "+data.actions.length+
        " игровых событий · "+clock(totalDuration);
      const report=result.rally_summary;
      E("result-rally-summary").textContent=Number.isFinite(report?.raw_tracks)
        ?"Траекторий мяча: "+report.raw_tracks+" · игровых: "+
         report.classified_tracks+" · после объединения и фильтрации: "+
         nextRallies.length+". Границы предположительные; подачи и очки не подтверждены."
        :"Для этой версии границы записаны в событиях CPU.";
      sortedActions=data.actions.filter(action=>Number.isFinite(Number(action.time)))
        .slice().sort((a,b)=>Number(a.time)-Number(b.time));
      appliedRallyKey=key;
      preserveSelectedRallies(oldRallies,nextRallies);
      if(oldSelection&&playbackMode!=="free"){
        let index=nextRallies.findIndex(r=>r.rally_id&&r.rally_id===oldSelection.rally_id);
        if(index<0&&oldSelection.detected_start!=null)
          index=nextRallies.findIndex(r=>
            r.detected_start===oldSelection.detected_start&&
            r.detected_end===oldSelection.detected_end);
        if(index<0){
          segment=null;
          E("result-overlay-rally").hidden=true;
        }else{
          const r=nextRallies[index];
          segment={index,start:mediaTime(r.start),end:mediaTime(r.end)};
          if(v().currentTime<segment.start-.1||v().currentTime>segment.end+.1)
            segment=null;
        }
      }
      renderEventCounts();
      renderList();
      E("result-rally-list").scrollTop=previousScroll;
      updateOverlay();
      window.dispatchEvent(new Event("volley:rally-config-change"));
      const count=nextRallies.length;
      rallyStatus(unchanged
        ?"Применено. Розыгрышей: "+count+
         ". Границы не изменились: нет подходящих разрывов или коротких эпизодов. "+
         "Классификацию меняет только CPU-пересчёт."
        :"Пересчитано без перезапуска видео: "+oldRallies.length+
         " → "+count+" розыгрышей. Обновлены границы и список; "+
         "исходные касания и классификация CPU не изменялись.");
    }catch(error){
      if(epoch!==rallyRefreshEpoch||loaded!==loadId)return;
      rallyStatus("Ошибка расчёта границ: "+error.message);
    }
  }
  function scheduleRallyRefresh(immediate=false){
    // Invalidate a prior response as soon as the user edits a setting,
    // not only when the debounced replacement request starts.
    cancelRallyRefresh();
    if(!validRallyFields()){
      rallyStatus("Проверьте диапазоны: разрыв 0–12 с, минимум 0–40 с, контекст 0–10 с.");
      return;
    }
    storeRallyConfig();
    if(!data?.analyzed){
      rallyStatus("Сначала создайте CPU-версию, содержащую треки розыгрышей.");
      return;
    }
    if(rallyKey()===appliedRallyKey)return;
    // Input can change many times while typing. Fetch only the latest value.
    rallyRefreshTimer=setTimeout(()=>{
      rallyRefreshTimer=null;
      refreshRallyBoundaries();
    },immediate?0:420);
  }
  const defaults=[3,3,1.5];
  function rallyConfig(){
    return Object.fromEntries(["gap_sec","min_sec","context_sec"].map((key,i)=>{
      const n=Number(E(rallyFields[i]).value);
      const max=[12,40,10][i];
      return [key,Number.isFinite(n)?Math.max(0,Math.min(max,n)):defaults[i]];
    }));
  }
  function restoreRallyConfig(){
    let saved=null;
    try{saved=JSON.parse(localStorage.getItem("volley-rally-settings:"+project)||"null")}catch{}
    rallyFields.forEach((id,i)=>{
      const value=saved&&saved[id];
      E(id).value=Number.isFinite(value)&&value>=0&&value<=[12,40,10][i]?value:defaults[i];
    });
  }
  function storeRallyConfig(){
    try{localStorage.setItem("volley-rally-settings:"+project,
      JSON.stringify(Object.fromEntries(rallyFields.map(id=>[id,Number(E(id).value)]))))}catch{}
  }

  function fitResultVideo(){
    const player=v(),container=E("view-analysis-results").querySelector(".result-video-main");
    if(!container||!player.videoWidth||!player.videoHeight)return;
    const h=Math.min(window.innerHeight*.70,780);
    container.style.setProperty("--result-display-width",h*player.videoWidth/player.videoHeight+"px");
    container.style.setProperty("--result-aspect",player.videoWidth+"/"+player.videoHeight);
    requestAnimationFrame(updateOverlay);
  }
  const mediaTime=t=>Math.max(0,t-offset);
  const sourceTime=t=>t+offset;
  const percent=t=>Math.max(0,Math.min(100,(t-offset)*100/Math.max(1,totalDuration)));
  function setAvailable(yes){
    if(!yes){
      const diagnostic=E("result-upstream-touch-video");
      if(diagnostic){diagnostic.hidden=true;diagnostic.removeAttribute("href");}
    }
    const view=E("view-analysis-results");
    // Keep project source checkboxes available even before a run finishes.
    view.querySelector(".result-media-panel").hidden=false;
    view.querySelector(".result-video-layout").hidden=!yes;
    view.querySelector(".result-timeline").hidden=!yes;
    view.querySelector(".result-rallies-panel").hidden=!yes;
    E("result-unavailable").hidden=yes;
  }
  function clear(){
    cancelRallyRefresh();
    appliedRallyKey="";
    rallyStatus("Границы текущей CPU-версии ещё не загружены.");
    const player=v();
    player.pause();
    player.removeAttribute("src");
    player.load();
    segment=null;data=null;offset=0;totalDuration=0;rallyOrder=[];sortedActions=[];
    if(ballThresholdTimer!==null){clearTimeout(ballThresholdTimer);ballThresholdTimer=null;}
    ballThreshold=null;
    if(playerThresholdTimer!==null){clearTimeout(playerThresholdTimer);playerThresholdTimer=null;}
    playerThreshold=null;
    E("result-player-threshold").disabled=true;
    E("result-player-threshold-reset").disabled=true;
    E("result-player-threshold-status").textContent="Проверяем исходные рамки игроков…";
    E("result-ball-threshold").disabled=true;
    E("result-ball-threshold-reset").disabled=true;
    E("result-ball-threshold-status").textContent="Проверяем наличие исходных оценок мяча…";
    E("result-reprocess-start").disabled=true;
    E("result-reprocess-status").textContent="Выберите завершённый анализ и настройте пороги.";
    modelTouches.reset();
    resetRawWindows();
    E("result-rally-list").innerHTML='<div class="empty">Нет сохранённых розыгрышей.</div>';
    E("result-rally-track").innerHTML="";
    E("result-timeline-ticks").innerHTML="";
    E("result-rally-count").textContent="0";
    E("result-current-time").textContent="00:00:00";
    syncPlayerControls();
    E("result-rally-summary").textContent="";
    E("result-overlay-ball").hidden=true;
    E("result-overlay-event").hidden=true;
    E("result-overlay-rally").hidden=true;
    E("result-overlay-touch").hidden=true;
    invalidateTrail();
    resetPlayerBoxes();
    renderEventCounts();
    syncReprocessControls();
  }
  // Each block shows its own waiting state; no page-wide overlay.
  function showLoading(){
    ++loadId; // An earlier result request may not overwrite the new project.
    clear();
    setAvailable(true);
    E("result-meta").textContent="Данные загружаются…";
    E("result-video-placeholder").hidden=false;
    E("result-video-placeholder").textContent="Данные загружаются…";
    E("result-rally-list").innerHTML='<div class="empty" role="status">Данные загружаются…</div>';
    E("result-rally-count").textContent="…";
    E("result-rally-summary").textContent="Данные загружаются…";
  }
  function rallyPlayersHtml(r){
    return (r.player_rows||[]).map(player=>{
      const params=new URLSearchParams({video:source,id:String(player.track_id),project_id:project});
      if(rawCacheId())params.set("cache_id",rawCacheId());
      const similarity=Number.isFinite(player.similarity)?" · "+(100*player.similarity).toFixed(1)+"%":"";
      const status={automatic:"авто",confirmed:"подтверждён",suggested:"подтвердить",unknown:"выбрать вручную"}[player.status]||"";
      return '<span class="result-rally-person"><img loading="lazy" src="/api/player-photo?'+esc(params.toString())+
        '" alt="Игрок ID '+esc(player.track_id)+'"><span><b>ID '+esc(player.track_id)+
        '</b><small>'+esc((player.name||"Не привязан")+similarity+" · "+status)+'</small></span></span>';
    }).join("");
  }
  function renderList(){
    if(!data)return;
    const rallies=data.rallies||[];
    // Rally order and counters derive from the saved analysis, not display filters.
    // Display toggles must never reconstruct this scrollable list.
    // Sort by every saved contact in the selected postprocessing version.
    const times=sortedActions.map(a=>Number(a.time));
    const bound=(value,includeEqual)=>{
      let lo=0,hi=times.length;
      while(lo<hi){
        const mid=(lo+hi)>>>1;
        if(times[mid]<value||(includeEqual&&times[mid]===value))lo=mid+1;
        else hi=mid;
      }
      return lo;
    };
    rallyOrder=rallies.map((r,index)=>({
      index,duration:Math.max(0,r.end-r.start),
      touches:bound(r.end,true)-bound(r.start,false)
    }));
    if(rallySort==="duration")rallyOrder.sort((a,b)=>b.duration-a.duration||a.index-b.index);
    if(rallySort==="touches")rallyOrder.sort((a,b)=>b.touches-a.touches||a.index-b.index);
    E("result-rally-count").textContent=rallies.length+" эпизодов";
    const selectedForMontage=new Set(montageRallies());
    E("result-rally-list").innerHTML=rallyOrder.length?rallyOrder.map(({index,duration,touches})=>{
      const r=rallies[index];
      const people=(r.player_rows||[]).map(p=>(p.name||'Не привязан')+(Number.isFinite(p.similarity)?' · '+(100*p.similarity).toFixed(1)+'%':'')+' · '+({automatic:'авто',confirmed:'подтверждён',suggested:'подтвердить',unknown:'выбрать вручную'}[p.status]||'')).join('; ');
      const evidence=r.start_evidence==="detected_serve"?" · подача найдена":"";
      const joined=r.joined_parts>1?" · "+r.joined_parts+" трека":"";
      const chosen=selectedForMontage.has(r.rally_id);
      return '<div class="result-rally-entry'+(chosen?' is-selected':'')+'"'+
        (r.rally_id?' data-rally-mark="'+esc(r.rally_id)+'"':'')+
        ' title="Правая кнопка мыши — отметить розыгрыш">'+
        '<button type="button" class="result-rally-item" data-rally="'+index+
        '" title="Воспроизвести розыгрыш" aria-current="false"><span class="result-rally-number">№ '+(index+1)+'</span>'+
        '<span class="result-rally-details"><b>'+clock(r.start)+' – '+clock(r.end)+'</b>'+
        '<span class="result-rally-metrics"><span><strong>'+duration.toFixed(1)+'</strong> с</span>'+
        '<span aria-label="'+touches+' касаний"><strong>'+touches+'</strong> касаний</span>'+
        '<span aria-label="игроков в кадре '+(r.player_count==null?'—':Number(r.player_count))+'"><strong>'+(r.player_count==null?'—':Number(r.player_count))+'</strong> игроков</span></span>'+
        '<span class="result-rally-people" title="'+esc(people+evidence+joined)+'">'+
        esc((r.linked_players||[]).map(p=>p.name).join(', '))+'</span></span>'+
        '<span class="result-rally-play">▶</span></button></div>';
    }).join(""):'<div class="empty">В выбранном видео модель не нашла завершённых розыгрышей.</div>';
    renderTimeline();
  }
  function renderTimeline(){
    if(!data)return;
    const rallies=data.rallies||[];
    const visibleActions=(data.actions||[]).filter(actionVisible);
    const blocks=rallies.map((r,i)=>
      '<button type="button" data-rally="'+i+'" class="result-track-rally" title="Розыгрыш № '+(i+1)+
      '" style="left:'+percent(r.start)+'%;width:'+Math.max(.2,percent(r.end)-percent(r.start))+'%"></button>').join("");
    // Timeline marks are visual only. Keep all actions in data and exact
    // rally counters; cap the DOM markers to 500 on unusually dense videos.
    const stride=Math.max(1,Math.ceil(visibleActions.length/500));
    const marks=visibleActions.filter((_,i)=>i%stride===0).map(a=>{
      const kind=String(a.type).replace(/[^a-z_]/g,"");
      const pos=percent(a.time),score=Math.round(Number(a.confidence)*100);
      const label=esc(actionText[a.type]||a.type)+" · "+score+"% · "+clock(a.time);
      return (overlays.touches
          ? '<span class="result-track-event result-track-touch" style="left:'+pos+'%" title="Касание · '+label+'"></span>' : '')+
        (classifiedVisible(a)
          ? '<span class="result-track-event result-track-'+kind+'" style="left:'+pos+'%" title="'+label+'"></span>' : '');
    }).join("");
    E("result-rally-track").innerHTML=blocks+marks+
      '<div id="result-track-playhead" class="result-track-playhead"></div>';
    E("result-timeline-ticks").innerHTML=[0,.25,.5,.75,1].map(t=>
      '<span>'+clock(sourceTime(t*totalDuration))+'</span>').join("");
    syncRallySelection();
  }
  function syncRallySelection(scroll=false){
    const index=segment?.index;
    let active=null;
    E("result-rally-list").querySelectorAll("[data-rally]").forEach(button=>{
      const selected=index!=null&&Number(button.dataset.rally)===index;
      button.classList.toggle("active",selected);
      button.setAttribute("aria-current",String(selected));
      if(selected)active=button;
    });
    E("result-rally-track").querySelectorAll(".result-track-rally").forEach(button=>{
      button.classList.toggle("active",index!=null&&Number(button.dataset.rally)===index);
    });
    if(scroll&&active?.scrollIntoView)active.scrollIntoView({block:"nearest",behavior:"smooth"});
  }
  function setCacheSelection(map,runId="",settings=null){
    const nextFingerprint=JSON.stringify([map||{},runId,settings||null]);
    // Re-entering the preview must not reset existing timeline/player/video data.
    if(nextFingerprint===selectionFingerprint)return;
    selectionFingerprint=nextFingerprint;
    cancelRallyRefresh();
    appliedRallyKey="";
    selectedCaches={...(map||{})};
    montageRunId=runId;
    selectedPostprocessSettings=settings&&typeof settings==="object"?settings:null;
    forcedCpuThresholds=null;
    const context=selectionFingerprint;
    goldenCpuDefaults().then(defaults=>{
      if(selectionFingerprint===context)
        applyCpuSettings({...defaults,...(selectedPostprocessSettings||{})});
    }).catch(error=>console.warn("Эталон CPU:",error));
    const replayMode=E("result-reprocess-mode");
    if(replayMode)replayMode.value=selectedPostprocessSettings?.mode||"combined";
    const replayTouch=E("result-reprocess-touch-threshold");
    if(replayTouch)replayTouch.value=String(selectedPostprocessSettings?.touch_threshold??0.45);
    E("result-touch-min-interval").value=String(
      selectedPostprocessSettings?.touch_min_interval_sec??0.35);
    E("result-touch-max-reach").value=String(
      selectedPostprocessSettings?.touch_max_reach_ratio??0.35);
    for(const [id,key,value] of [
      ["result-track-min-duration","track_min_duration",.5],
      ["result-track-max-distance","track_max_distance",200],
      ["result-player-hold-frames","player_hold_frames",90],
      ["result-player-match-distance","player_match_distance",.14],
      ["result-track-player-threshold","track_player_threshold",.30],
      ["result-track-ball-threshold","track_ball_threshold",.35],
    ]){
      E(id).value=String(selectedPostprocessSettings?.[key]??value);
    }
    E("result-track-no-ball-fill").checked=Boolean(
      selectedPostprocessSettings?.track_no_ball_fill);
    E("result-track-beach").checked=Boolean(selectedPostprocessSettings?.beach??true);
    syncReprocessControls();
    invalidateTrail();
    resetPlayerBoxes();
    resetRawWindows();
    updateSourceMode();
  }
  async function setProject(projectId,videos=[],selected=null){
    const previousProject=project;
    project=projectId||"";
    const picker=E("result-video-select");
    if(picker){
      const choices=videos.filter(name=>Boolean(selectedCaches[name]));
      picker.innerHTML=choices.map(name=>'<option value="'+esc(name)+'">'+esc(name)+'</option>').join("");
      picker.hidden=choices.length<2;
      picker.value=selected||source||videos[0]||"";
    }
    if(previousProject!==project){
      restoreRallyConfig();
      restoreOverlays();
    }
    // The header chain is the single source of project/video selection.
    const preferred=selected&&videos.includes(selected)?selected:
      previousProject===project&&videos.includes(source)?source:videos[0];
    if(preferred){
      if(previousProject===project&&preferred===source&&data&&
         loadedFingerprint===selectionFingerprint)return;
      source=preferred;
      await setVideo(preferred);
    }else{
      source="";clear();setAvailable(false);
      E("result-video-placeholder").hidden=false;
      E("result-video-placeholder").textContent="Создайте проект и запустите анализ";
    }
  }
  async function setVideo(name){
    const selectionAtStart=selectionFingerprint;
    const seq=++loadId;
    source=name;
    if(E("result-video-select"))E("result-video-select").value=name;
    clear();
    setAvailable(true);
    const diagnostic=E("result-upstream-touch-video");
    if(diagnostic){diagnostic.hidden=true;diagnostic.removeAttribute("href");}
    E("result-meta").textContent="Данные загружаются…";
    E("result-video-placeholder").hidden=false;
    E("result-video-placeholder").textContent="Данные загружаются…";
    E("result-rally-list").innerHTML='<div class="empty" role="status">Данные загружаются…</div>';
    E("result-rally-count").textContent="…";
    E("result-rally-summary").textContent="Данные загружаются…";
    if(!selectedCaches[name]){
      setAvailable(false);
      E("result-video-placeholder").hidden=false;
      E("result-video-placeholder").textContent="В выбранном запуске нет анализа этого видео.";
      E("result-meta").textContent="Нет данных выбранного запуска";
      return;
    }
    try{
      const params=new URLSearchParams({project_id:project,video:name,
        cache_id:selectedCaches[name],...rallyConfig()});
      const query="?"+params.toString();
      const result=await readResultJson("/api/analysis-view"+query,
        "Не удалось загрузить анализ");
      if(seq!==loadId)return;
      // Read the selected profile's immutable cache. A different profile
      // may be running at the same time; global job status must never hide it.
      if(!result.analyzed){
        data=result.inference_ready?result:null;
        loadedFingerprint=data?selectionAtStart:"";
        const message=result.inference_ready
          ?"Нейросети завершены. Выберите алгоритм ниже и нажмите «Пересчитать проект» — GPU повторно не запускается."
          :"Для выбранного профиля ещё нет завершённого анализа этого видео.";
        E("result-video-placeholder").textContent=message;
        E("result-meta").textContent=result.inference_ready
          ?"Инференс GPU готов · CPU-обработка ещё не выполнена":"Нет готового анализа";
        E("result-rally-list").innerHTML='<div class="empty">'+esc(message)+'</div>';
        E("result-rally-count").textContent="0";
        if(result.inference_ready && result.media_url){
          // Raw predictions can be inspected without fabricating any game
          // events. Video and GPU model overlays are available immediately.
          offset=Number(result.analysis_offset_sec)||0;
          totalDuration=Number(result.duration_sec)||0;
          const player=v();
          player.src=result.media_url;
          player.load();
          restoreBallThreshold();
          restorePlayerThreshold();
          modelTouches.setSource({
            project,video:source,cache:rawCacheId(),
            duration:totalDuration,offset
          });
          // A neural-only result has no CPU events; select actual GPU view.
          const rawRadio=document.querySelector(
            '[name="result-preview-mode"][value="model"]');
          if(rawRadio&&!rawRadio.checked){
            rawRadio.checked=true;
            rawRadio.dispatchEvent(new Event("change",{bubbles:true}));
          }
          E("result-rally-summary").textContent=message;
          renderEventCounts();
          updateSourceMode();
          updateOverlay();
        }
        syncReprocessControls();
        return;
      }
      setAvailable(true);
      if(diagnostic){
        diagnostic.hidden=!result.upstream_touch_video_url;
        if(result.upstream_touch_video_url)
          diagnostic.href=result.upstream_touch_video_url;
      }
      data=result;
      loadedFingerprint=selectionAtStart;
      appliedRallyKey=rallyKey();
      rallyStatus(Number.isFinite(result.rally_summary?.raw_tracks)
        ?"Границы готовы: "+result.rallies.length+" розыгрышей из "+
         result.rally_summary.raw_tracks+" треков. Измените значения для быстрого пересчёта."
        :"В этой старой версии нет треков для быстрого пересчёта. "+
         "Для новых границ запустите CPU-постобработку.");
      restoreBallThreshold();
      restorePlayerThreshold();
      syncReprocessControls();
      sortedActions=(result.actions||[]).filter(action=>Number.isFinite(Number(action.time)))
        .slice().sort((a,b)=>Number(a.time)-Number(b.time));
      offset=Number(result.analysis_offset_sec)||0;
      totalDuration=Number(result.duration_sec)||0;
      E("result-meta").textContent=(result.rallies||[]).length+" возможных розыгрышей · "+
        (result.actions||[]).length+" игровых событий · "+clock(totalDuration);
      const summary=result.rally_summary;
      E("result-rally-summary").textContent=summary&&Number.isFinite(summary.raw_tracks)
        ? "Траекторий мяча: "+summary.raw_tracks+
          " · игровых: "+summary.classified_tracks+
          " · после объединения и фильтрации: "+result.rallies.length+
          ". Границы предположительные, не подтверждённые подачи и очки."
        : "Границы розыгрышей определены эвристически.";
      const player=v();
      player.src=result.media_url;
      player.load();
      // The first frame may be delayed after the JSON response; do not hide
      // the message until the browser has actually loaded image data.
      modelTouches.setSource({
        project,video:source,cache:rawCacheId(),
        duration:totalDuration,offset
      });
      updateSourceMode();
      renderEventCounts();
      renderList();
      updateOverlay();
    }catch(err){
      if(seq!==loadId)return;
      loadedFingerprint="";
      E("result-video-placeholder").hidden=false;
      E("result-video-placeholder").textContent=err.message;
      E("result-meta").textContent="Не удалось открыть результат";
      E("result-rally-list").innerHTML='<div class="empty">'+esc(err.message)+'</div>';
      E("result-rally-count").textContent="0";
      E("result-rally-summary").textContent="Проверьте настройки объединения и длительности.";
    }
  }
  function selectRally(index){
    if(!data)return;
    const r=data.rallies[index];
    if(!r)return;
    segment=playbackMode==="free"?null:
      {index,start:mediaTime(r.start),end:mediaTime(r.end)};
    const player=v();
    player.pause();
    player.currentTime=Math.max(0,mediaTime(r.start));
    syncRallySelection(true);
    E("result-overlay-rally").hidden=playbackMode==="free";
    E("result-overlay-rally").textContent="РОЗЫГРЫШ № "+(index+1);
    // play() may require interaction (button click counts as gesture).
    const promise=player.play();
    // Switching rapidly between rallies can reject a superseded play request.
    if(promise&&promise.catch)promise.catch(()=>{});
  }
  function enforceEnd(mediaEnded=false){
    const player=v(),current=segment;
    if(inRawMode()||playbackMode==="free"||!current||
       (!mediaEnded&&(player.paused||player.seeking))||
       !Number.isFinite(player.currentTime))return;
    if(mediaEnded||player.currentTime>=current.end-.035){
      if(playbackMode==="loop"){
        player.currentTime=Math.max(0,current.start);
        if(player.paused||player.ended){
          const promise=player.play();
          if(promise&&promise.catch)promise.catch(()=>{});
        }
        return;
      }
      if(playbackMode==="sequence"){
        const position=rallyOrder.findIndex(item=>item.index===current.index);
        const next=position>=0?rallyOrder[position+1]:null;
        if(next){selectRally(next.index);return;}
      }
      // One-shot mode, or the last rally in sequence mode.
      player.pause();
      player.currentTime=Math.min(current.end,
        Number.isFinite(player.duration)?player.duration:current.end);
      updateOverlay();
    }
  }
  function closestVisibleAction(t){
    // O(log(actions) + nearby actions) instead of filtering and sorting every
    // event on each video frame (30-60 calls/sec).
    const from=t-.42,to=t+.42;
    let low=0,high=sortedActions.length;
    while(low<high){
      const mid=(low+high)>>>1;
      if(Number(sortedActions[mid].time)<from)low=mid+1;
      else high=mid;
    }
    let closest=null,distance=Infinity;
    for(let i=low;i<sortedActions.length&&Number(sortedActions[i].time)<=to;i++){
      const action=sortedActions[i];
      const delta=Math.abs(Number(action.time)-t);
      if(actionVisible(action)&&delta<distance){closest=action;distance=delta;}
    }
    return closest;
  }
  function syncPlayerControls(){
    const player=v(),time=sourceTime(player.currentTime||0);
    const seek=E("result-player-seek");
    if(!seek)return;
    seek.min=String(offset);
    seek.max=String(offset+totalDuration);
    seek.value=String(time);
    seek.disabled=!data||totalDuration<=0;
    seek.setAttribute("aria-valuetext",clock(time));
    E("result-player-time").textContent=clock(time);
    E("result-current-time").textContent=clock(time);
    E("result-player-end").textContent=clock(offset+totalDuration);
    const toggle=E("result-player-toggle");
    toggle.textContent=player.paused?"▶":"Ⅱ";
    toggle.disabled=!data;
    toggle.setAttribute("aria-label",player.paused?"Воспроизвести":"Пауза");
    E("result-player-mute").textContent=player.muted?"Без звука":"Звук";
    E("result-player-mute").setAttribute("aria-label",player.muted?"Включить звук":"Выключить звук");
  }
  function updateOverlay(){
    syncPlayerControls();
    if(!data||!E("view-analysis-results").classList.contains("active"))return;
    const player=v(),elapsed=player.currentTime||0,t=sourceTime(elapsed);
    const marker=E("result-track-playhead");
    if(marker)marker.style.left=(100*elapsed/Math.max(1,totalDuration))+"%";
    E("result-current-time").textContent=clock(t);
    if(modelTouches.active()){
      E("result-overlay-event").hidden=true;
      E("result-overlay-ball").hidden=true;
      E("result-overlay-touch").hidden=true;
      E("result-overlay-rally").hidden=true;
      const geometry=ballGeometry();
      paintRawBalls(elapsed,geometry);
      paintPlayerBoxes(elapsed,geometry);
      if(rawEnabled("players"))requestPlayerBoxes(elapsed);
      modelTouches.update(elapsed);
      return;
    }
    const badge=E("result-overlay-event"),ball=E("result-overlay-ball");
    const geometry=ballGeometry();
    const near=closestVisibleAction(t);
    const touch=E("result-overlay-touch");
    badge.hidden=true;ball.hidden=true;touch.hidden=true;
    if(near){
      const typed=classifiedVisible(near),kind=typed?typeToControl[near.type]:"touches";
      const color=colors[kind]||colors.touches,score=Math.round(Number(near.confidence)*100);
      badge.textContent="CPU · "+(typed?actionText[near.type]:"Касание")+" · "+score+"%"+
        (near.player_id!=null?" · ID "+near.player_id:"")+
        (score<50?" · низкая уверенность":"");
      badge.title=cpuSourceLabel();
      badge.style.setProperty("--result-event-color",color);
      badge.hidden=false;
      if(typed&&overlays.touches){touch.textContent="КАСАНИЕ";touch.hidden=false;}
      const x=Number(near.ball?.x),y=Number(near.ball?.y);
      if(geometry&&near.ball?.space==="analysis_pixels"&&Number.isFinite(x)&&Number.isFinite(y)&&
         x>=0&&y>=0&&x<=geometry.sourceW&&y<=geometry.sourceH){
        ball.style.setProperty("--result-event-color",color);
        ball.classList.toggle("result-ball-with-touch",overlays.touches&&typed);
        ball.style.left=(x/geometry.sourceW*geometry.shownW)+"px";
        ball.style.top=(y/geometry.sourceH*geometry.shownH)+"px";
        ball.hidden=false;
      }
    }
    drawTrajectory(elapsed,geometry);
    paintPlayerBoxes(elapsed,geometry);
    if(overlays.trajectory)requestTrail(elapsed);
    if(overlays.players)requestPlayerBoxes(elapsed);
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
    E("result-video-select")?.addEventListener("change",event=>{
      window.dispatchEvent(new CustomEvent("volley:select-video",{detail:{video:event.target.value}}));
    });
    E("result-player-toggle")?.addEventListener("click",()=>{
      const player=v();
      if(player.paused){const promise=player.play();if(promise?.catch)promise.catch(()=>{});}
      else player.pause();
      syncPlayerControls();
    });
    E("result-player-seek")?.addEventListener("input",event=>{
      if(!data)return;
      const time=Number(event.target.value);
      if(!Number.isFinite(time))return;
      segment=null;
      syncRallySelection();
      E("result-overlay-rally").hidden=true;
      v().currentTime=Math.min(totalDuration,mediaTime(time));
      updateOverlay();
    });
    E("result-player-mute")?.addEventListener("click",()=>{
      v().muted=!v().muted;syncPlayerControls();
    });
    E("result-player-speed")?.addEventListener("change",event=>{
      v().playbackRate=Number(event.target.value);
    });
    E("result-player-fullscreen")?.addEventListener("click",()=>{
      const promise=document.fullscreenElement?document.exitFullscreen():
        v().closest(".result-player-wrap")?.requestFullscreen?.();
      if(promise?.catch)promise.catch(()=>{});
    });
    for(const event of ["play","pause","ended","volumechange","loadedmetadata"])
      v().addEventListener(event,syncPlayerControls);
    restoreRallySort();
    restorePlaybackMode();
    E("result-playback-mode").addEventListener("click",cyclePlaybackMode);
    E("result-rally-sort").addEventListener("change",e=>{
      rallySort=["normal","duration","touches"].includes(e.target.value)?e.target.value:"normal";
      try{localStorage.setItem("volley-result-rally-sort",rallySort)}catch{}
      if(data)renderList();
      syncRallySelection(true);
    });
    modelTouches.init(updateSourceMode);
    for(const [kind,sliderId,valueId] of [
      ["vballnet","result-raw-vball-threshold","result-raw-vball-value"],
      ["ravelBall","result-raw-ravel-threshold","result-raw-ravel-value"],
      ["players","result-raw-player-threshold","result-raw-player-value"]]){
      const input=E(sliderId),output=E(valueId);
      const change=()=>{
        rawThresholds[kind]=Number(input.value);
        output.textContent=Math.round(rawThresholds[kind]*100)+"%";
        if(kind==="players"){
          resetPlayerBoxes(); // Different raw threshold needs a fresh server window.
          updateOverlay();
        }else{
          resetRawWindows();
          syncRawFilterStatus();
          updateOverlay();
        }
      };
      input.addEventListener("input",()=>{
        output.textContent=Math.round(Number(input.value)*100)+"%";
        if(rawFilterTimer!==null)clearTimeout(rawFilterTimer);
        rawFilterTimer=setTimeout(()=>{rawFilterTimer=null;change();},140);
      });
      input.addEventListener("change",()=>{
        if(rawFilterTimer!==null){clearTimeout(rawFilterTimer);rawFilterTimer=null}
        change();
      });
    }
    E("result-raw-trail-seconds").addEventListener("change",e=>{
      rawTrail.seconds=Number(e.target.value)||2;
      updateOverlay();
    });
    for(const input of document.querySelectorAll('[name="result-raw-source"]')){
      input.addEventListener("change",()=>{
        if(!input.checked)return;
        rawSource=input.value;
        const modelRadio=document.querySelector('[name="result-preview-mode"][value="model"]');
        if(modelRadio&&!modelRadio.checked){
          modelRadio.checked=true;
          modelRadio.dispatchEvent(new Event("change",{bubbles:true}));
        }else updateSourceMode();
      });
    }
    for(const input of document.querySelectorAll("[data-result-raw]")){
      input.addEventListener("change",()=>{
        const key=input.dataset.resultRaw;
        rawOverlays[key]=input.checked;
        if(key==="touch")modelTouches.setTouchVisible?.(rawEnabled("touch")&&inRawMode());
        if(key==="players")resetPlayerBoxes();
        if(key==="vballnet"||key==="ravelBall"){
          resetRawWindows();
          E("result-raw-trail-seconds").disabled=!rawOverlays[key];
        }
        updateOverlay();
      });
    }
    E("result-trajectory-jump").addEventListener("click",()=>{
      const button=E("result-trajectory-jump");
      const seconds=Number(button.dataset.frame)/Number(button.dataset.fps);
      if(Number.isFinite(seconds)&&seconds>=0&&Number.isFinite(totalDuration)&&seconds<totalDuration){
        // Jumping from a selected rally must not enforce its old end boundary.
        segment=null;
        E("result-overlay-rally").hidden=true;
        syncRallySelection();
        v().currentTime=seconds;
      }
    });
    restoreOverlays();
    E("result-ball-threshold").addEventListener("input",e=>{
      forcedCpuThresholds=null;
      const value=e.target.value;
      E("result-ball-threshold-value").textContent=Math.round(Number(value)*100)+"%";
      if(ballThresholdTimer!==null)clearTimeout(ballThresholdTimer);
      ballThresholdTimer=setTimeout(()=>setBallThreshold(value),180);
    });
    E("result-ball-threshold").addEventListener("change",e=>setBallThreshold(e.target.value));
    E("result-ball-threshold-reset").addEventListener("click",()=>{
      forcedCpuThresholds=null;setBallThreshold(null);
    });
    E("result-player-threshold").addEventListener("input",e=>{
      forcedCpuThresholds=null;
      const value=e.target.value;
      E("result-player-threshold-value").textContent=Math.round(Number(value)*100)+"%";
      if(playerThresholdTimer!==null)clearTimeout(playerThresholdTimer);
      playerThresholdTimer=setTimeout(()=>setPlayerThreshold(value),180);
    });
    E("result-player-threshold").addEventListener("change",e=>setPlayerThreshold(e.target.value));
    E("result-player-threshold-reset").addEventListener("click",()=>{
      forcedCpuThresholds=null;setPlayerThreshold(null);
    });
    E("result-reprocess-reset").addEventListener("click",async()=>{
      const button=E("result-reprocess-reset");
      button.disabled=true;
      try{
        const defaults=await goldenCpuDefaults();
        applyCpuSettings(defaults);
        if(data?.ball_detection?.available)setBallThreshold(defaults.ball_threshold);
        if(data?.player_detection?.available)setPlayerThreshold(defaults.player_threshold);
        // An explicit reset must ignore the selected CPU version's old thresholds.
        forcedCpuThresholds={
          ball_threshold:defaults.ball_threshold,
          player_threshold:defaults.player_threshold
        };
        E("result-reprocess-status").textContent=
          "Золотые настройки восстановлены. Для новой CPU-версии нажмите «Пересчитать проект».";
      }catch(error){
        E("result-reprocess-status").textContent=error.message;
      }finally{button.disabled=false;}
    });
    E("result-reprocess-mode").addEventListener("change",syncReprocessControls);
    E("result-reprocess-touch-threshold").addEventListener("input",syncReprocessControls);
    E("result-touch-min-interval").addEventListener("change",syncReprocessControls);
    E("result-touch-max-reach").addEventListener("change",syncReprocessControls);
    E("result-reprocess-start").addEventListener("click",()=>{
      if(!data||!project||!montageRunId)return;
      const mode=E("result-reprocess-mode").value;
      const touch=["combined","ravel-touch"].includes(mode)
        ?Number(E("result-reprocess-touch-threshold").value):0.5;
      const usesTouch=["combined","ravel-touch"].includes(mode);
      const touchGap=usesTouch?Number(E("result-touch-min-interval").value):.35;
      const touchReach=usesTouch?Number(E("result-touch-max-reach").value):.35;
      const track={
        track_min_duration:Number(E("result-track-min-duration").value),
        track_max_distance:Number(E("result-track-max-distance").value),
        player_hold_frames:Number(E("result-player-hold-frames").value),
        player_match_distance:Number(E("result-player-match-distance").value),
        track_player_threshold:Number(E("result-track-player-threshold").value),
        track_ball_threshold:Number(E("result-track-ball-threshold").value),
        track_no_ball_fill:mode!=="combined"&&E("result-track-no-ball-fill").checked,
        beach:E("result-track-beach").checked,
      };
      const bounds=[
        [track.track_min_duration,.05,30],
        [track.track_max_distance,1,2000],
        [track.player_hold_frames,0,180],
        [track.player_match_distance,.02,.30],
        [track.track_player_threshold,0,1],
        [track.track_ball_threshold,0,1],
      ];
      if(!Number.isInteger(track.player_hold_frames)||
         bounds.some(([value,lo,hi])=>!Number.isFinite(value)||value<lo||value>hi)){
        E("result-reprocess-status").textContent="Проверьте диапазоны параметров трекинга CPU.";
        E("result-cpu-tracking").open=true;
        return;
      }
      if(!Number.isFinite(touch)||touch<0||touch>1||
         !Number.isFinite(touchGap)||touchGap<.1||touchGap>1.5||
         !Number.isFinite(touchReach)||touchReach<.1||touchReach>1){
        E("result-reprocess-status").textContent=
          "Порог Touch: 0–1; интервал: 0.1–1.5 с; допуск мяча: 0.1–1.";
        return;
      }
      E("result-reprocess-start").disabled=true;
      E("result-reprocess-status").textContent="Запуск пересчёта…";
      window.dispatchEvent(new CustomEvent("volley:reprocess-request",{detail:{
        project_id:project,run_id:montageRunId,mode,touch_threshold:touch,
        touch_min_interval_sec:touchGap,touch_max_reach_ratio:touchReach,
        ...track,
        ball_threshold:forcedCpuThresholds
          ?forcedCpuThresholds.ball_threshold
          :(["baseline","combined"].includes(mode)
            ?(ballThreshold??(["baseline","combined"].includes(selectedPostprocessSettings?.mode)
               ?selectedPostprocessSettings?.ball_threshold:null)??null):null),
        player_threshold:forcedCpuThresholds
          ?forcedCpuThresholds.player_threshold
          :playerThreshold??selectedPostprocessSettings?.player_threshold
            ??(data?.player_detection?.available?loadedGoldenCpu?.player_threshold:null)??null
      }}));
    });
    for(const control of document.querySelectorAll("[data-result-overlay]")){
      control.addEventListener("change",()=>{
        overlays[control.dataset.resultOverlay]=control.checked;
        persistOverlays();
        if(control.dataset.resultOverlay==="trajectory")invalidateTrail();
        if(control.dataset.resultOverlay==="players")resetPlayerBoxes();
        // Only display markers change. Keep the rally list DOM, its scroll
        // position, checked montage items, and current selection intact.
        if(data){renderTimeline();updateOverlay();}
      });
    }
    rallyFields.forEach(id=>{
      E(id).addEventListener("input",()=>scheduleRallyRefresh());
      E(id).addEventListener("change",()=>scheduleRallyRefresh(true));
    });
    const rallyMenu=document.createElement("div");
    rallyMenu.className="result-rally-context-menu";
    rallyMenu.hidden=true;
    rallyMenu.setAttribute("role","menu");
    rallyMenu.innerHTML='<button type="button" role="menuitem" data-rally-toggle></button>';
    document.body.appendChild(rallyMenu);
    let contextRallyId=null;
    const closeRallyMenu=()=>{rallyMenu.hidden=true;contextRallyId=null;};
    E("result-rally-list").addEventListener("contextmenu",e=>{
      const entry=e.target.closest("[data-rally-mark]");
      if(!entry)return;
      e.preventDefault();
      contextRallyId=entry.dataset.rallyMark;
      const selected=montageRallies().includes(contextRallyId);
      rallyMenu.querySelector("button").textContent=selected?"Снять отметку":"★ Отметить";
      rallyMenu.hidden=false;
      rallyMenu.style.left=Math.max(4,Math.min(e.clientX,window.innerWidth-rallyMenu.offsetWidth-8))+"px";
      rallyMenu.style.top=Math.max(4,Math.min(e.clientY,window.innerHeight-rallyMenu.offsetHeight-8))+"px";
      rallyMenu.querySelector("button").focus();
    });
    rallyMenu.addEventListener("click",()=>{
      if(!contextRallyId)return;
      const selected=new Set(montageRallies());
      if(selected.has(contextRallyId))selected.delete(contextRallyId);
      else selected.add(contextRallyId);
      setMontageRallies([...selected]);
      closeRallyMenu();
    });
    document.addEventListener("pointerdown",e=>{
      if(!rallyMenu.hidden&&!rallyMenu.contains(e.target))closeRallyMenu();
    });
    document.addEventListener("keydown",e=>{
      if(e.key==="Escape"&&!rallyMenu.hidden)closeRallyMenu();
    });
    E("result-rally-list").addEventListener("scroll",closeRallyMenu,{passive:true});
    E("result-rally-list").addEventListener("click",e=>{
      const b=e.target.closest("[data-rally]");
      if(b)selectRally(Number(b.dataset.rally));
    });
    E("result-rally-track").addEventListener("click",e=>{
      const b=e.target.closest("[data-rally]");
      if(b){
        selectRally(Number(b.dataset.rally));
      }else if(data&&totalDuration>0){
        const bounds=e.currentTarget.getBoundingClientRect();
        const fraction=Math.max(0,Math.min(1,(e.clientX-bounds.left)/Math.max(1,bounds.width)));
        segment=null;
        syncRallySelection();
        v().currentTime=fraction*totalDuration;
        E("result-overlay-rally").hidden=true;
        updateOverlay();
      }
    });
    v().addEventListener("loadedmetadata",fitResultVideo);
    v().addEventListener("loadeddata",()=>{
      if(data)E("result-video-placeholder").hidden=true;
    });
    window.addEventListener("resize",fitResultVideo);
    window.addEventListener("volley:player-identities-change",event=>{
      const detail=event.detail||{};
      if(detail.project_id!==project||detail.video!==source)return;
      resetPlayerBoxes();
      if(showingPlayers()&&data)requestPlayerBoxes(v()?.currentTime||0);
    });
    if(typeof ResizeObserver!=="undefined"){
      const observer=new ResizeObserver(()=>{if(data)updateOverlay()});
      observer.observe(v());
      observer.observe(E("result-overlay"));
    }
    v().addEventListener("timeupdate",()=>{enforceEnd();updateOverlay()});
    v().addEventListener("ended",()=>enforceEnd(true));
    v().addEventListener("seeked",()=>{
      // A manual seek outside the selected interval switches to free playback.
      if(segment&&(v().currentTime<segment.start-.1||v().currentTime>segment.end+.1)){
        segment=null;
        syncRallySelection();
        E("result-overlay-rally").hidden=true;
      }
      updateOverlay();
    });
    v().addEventListener("play",()=>{
      // Pressing native Play after a one-shot stop replays the same rally,
      // rather than accidentally continuing past its end.
      if(segment&&v().currentTime>=segment.end-.035&&
         ["once","loop"].includes(playbackMode)){
        v().currentTime=Math.max(0,segment.start);
        return;
      }
      // A manual seek beyond the current rally allows unbounded playback.
      if(segment&&(v().currentTime<segment.start-.1||v().currentTime>=segment.end)){
        segment=null;
        E("result-overlay-rally").hidden=true;
        syncRallySelection();
      }
    });
    v().addEventListener("error",()=>{
      if(data){
        E("result-video-placeholder").hidden=false;
        E("result-video-placeholder").textContent="Не удалось загрузить видео для предпросмотра";
      }
    });
    startFrameMonitor();
    syncReprocessControls();
  }
  function activeRally(){return segment?{...segment}:null}
  return {init,setProject,setVideo,showLoading,setCacheSelection,setReprocessTask,
          selectRally,activeRally,montageRallies,setMontageRallies,
          currentPosition:()=>sourceTime(v().currentTime||0),
          playbackBounds:()=>({start:offset,end:offset+totalDuration}),fps:()=>30,
          seek:time=>{
            segment=null;syncRallySelection();E("result-overlay-rally").hidden=true;
            v().currentTime=Math.max(0,Math.min(totalDuration,mediaTime(time)));updateOverlay();
          }};
})();
