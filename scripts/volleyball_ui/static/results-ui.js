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
  const clock=t=>{
    t=Math.max(0,Math.round(t));
    return [Math.floor(t/3600),Math.floor(t%3600/60),t%60].map(x=>String(x).padStart(2,"0")).join(":");
  };
  const actionText={attack:"Атака",block:"Блок",serve:"Подача",dig:"Приём",overhead_pass:"Верхняя передача",low_touch:"Касание",unknown_touch:"Касание"};
  const typeToControl={attack:"attacks",block:"blocks",serve:"serves",dig:"digs",overhead_pass:"passes",low_touch:"touches",unknown_touch:"touches"};
  const overlayDefaults={touches:true,attacks:true,blocks:true,serves:true,digs:true,passes:true,trajectory:false,players:false};
  const overlays={...overlayDefaults};
  let selectedCaches={}, montageRunId="";
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
      E("result-rally-list").querySelectorAll("[data-montage-rally]").forEach(input=>{
        input.checked=next.includes(input.dataset.montageRally);
        input.closest(".result-rally-entry")?.classList.toggle("is-selected",input.checked);
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
    if(!overlays.players||playerUnavailable||!project||!source||!data)return;
    if(playerWindow&&seconds>=playerWindow.start&&seconds<playerWindow.end-1)return;
    const start=Math.max(0,Math.floor(seconds/5)*5);
    const key=project+"|"+source+"|"+start+"|"+playerThreshold;
    if(playerRequesting&&playerRequestKey===key)return;
    if(playerRequesting)return;
    playerRequesting=true;playerRequestKey=key;
    const epoch=playerEpoch;
    const q=new URLSearchParams({project_id:project,video:source,
      cache_id:selectedCaches[source]||"",start:String(start),length:"6"});
    if(playerThreshold!==null)q.set("threshold",String(playerThreshold));
    try{
      const result=await fetch("/api/analysis-player-boxes?"+q,
        {cache:"no-store"}).then(async r=>{
          const body=await r.json();
          if(!r.ok)throw Error(body.error||"Не удалось загрузить рамки");
          return body;
        });
      if(epoch!==playerEpoch||!overlays.players||!data)return;
      if(!result.available){playerUnavailable=true;return;}
      playerWindow={start,end:start+6,fps:Number(result.fps)||30,
        width:Number(result.width),height:Number(result.height),
        frames:new Map(result.frames||[])};
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
    if(!overlays.players||!geometry||!playerWindow||playerUnavailable){
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
      tag.textContent=id<0?"Кандидат · "+Math.round(score*100)+"%":"ID "+Math.floor(id)+" · "+Math.round(score*100)+"%";
      tag.title=id<0
        ?"RAVEL · сырая рамка без подтверждённого Track ID"
        :"RAVEL · исходный track ID "+Math.floor(id)+" · "+Math.round(score*100)+"%";
      box.append(tag);children.push(box);
    }
    panel.replaceChildren(...children);
  }
  const overlayKey=()=>"volley-result-overlays:"+project;
  const colors={touches:"#f7fcff",attacks:"#ff745c",blocks:"#b98cff",serves:"#25dbea",digs:"#ffca4c",passes:"#72a8ff"};
  let confidenceThreshold=0;
  const passesConfidence=a=>Number(a.confidence)>=confidenceThreshold;
  const classifiedVisible=a=>passesConfidence(a)&&
    !["low_touch","unknown_touch"].includes(a.type)&&Boolean(overlays[typeToControl[a.type]]);
  // Generic touches are ALL inferred contacts, not just "low_touch".
  const actionVisible=a=>passesConfidence(a)&&(overlays.touches||Boolean(overlays[typeToControl[a.type]]));
  function restoreConfidence(){
    let n=0;
    try{n=Number(localStorage.getItem("volley-result-confidence:"+project)||"0")}catch{}
    confidenceThreshold=[0,.5,.65,.8].includes(n)?n:0;
    E("result-confidence-threshold").value=String(confidenceThreshold);
    syncConfidenceButtons();
  }
  function syncConfidenceButtons(){
    for(const button of document.querySelectorAll("[data-result-confidence]")){
      const chosen=Number(button.dataset.resultConfidence)===confidenceThreshold;
      button.classList.toggle("active",chosen);
      button.setAttribute("aria-pressed",String(chosen));
    }
  }
  function setConfidence(value){
    const threshold=Number(value);
    confidenceThreshold=[0,.5,.65,.8].includes(threshold)?threshold:0;
    E("result-confidence-threshold").value=String(confidenceThreshold);
    syncConfidenceButtons();
    try{localStorage.setItem("volley-result-confidence:"+project,String(confidenceThreshold))}catch{}
    if(data){renderEventCounts();renderList();updateOverlay();}
  }
  const ballThresholdKey=()=> "volley-ball-threshold:"+project+":"+(selectedCaches[source]||"");
  function restoreBallThreshold(){
    const available=Boolean(data?.ball_detection?.available);
    const baseline=Number(data?.ball_detection?.analysis_threshold)||.5;
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
  }
  function setBallThreshold(value){
    if(ballThresholdTimer!==null){clearTimeout(ballThresholdTimer);ballThresholdTimer=null;}
    if(!data?.ball_detection?.available)return;
    if(value!==null){
      value=Number(value);
      if(!Number.isFinite(value)||value<.05||value>.95)return;
      value=Math.round(value*100)/100;
    }
    if(value!==null&&ballThreshold===value)return;
    ballThreshold=value;
    const baseline=Number(data.ball_detection.analysis_threshold)||.5;
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
      if(eventsMode&&!eventsMode.checked){
        eventsMode.checked=true;
        eventsMode.dispatchEvent(new Event("change",{bubbles:true}));
      }
    }
    invalidateTrail();
    E("result-ball-threshold-status").textContent=value===null
      ?"Исходные детекции анализа восстановлены. События и розыгрыши не пересчитываются."
      :"Порог "+Math.round(value*100)+"%: показываем сохранённые кандидаты. События и розыгрыши не пересчитываются.";
    updateOverlay();
    if(data)requestTrail(v().currentTime||0);
  }
  const playerThresholdKey=()=>"volley-player-threshold:"+project+":"+(selectedCaches[source]||"");
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
      if(events&&!events.checked){
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
  }
  function renderEventCounts(){
    const counts={touches:0,attacks:0,blocks:0,serves:0,digs:0,passes:0};
    for(const a of data?.actions||[]){
      if(!passesConfidence(a))continue;
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
    const text=overlays.trajectory?message:"";
    const el=E("result-trajectory-status"),indicator=E("result-trajectory-indicator");
    if(el&&el.textContent!==text)el.textContent=text;
    if(indicator){
      indicator.hidden=!overlays.trajectory;
      indicator.textContent=overlays.trajectory?"● "+(message||"Траектория включена"):"";
    }
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
      const response=await fetch("/api/analysis-trajectory?"+params,{cache:"no-store"});
      if(!response.ok)throw Error("Trajectory HTTP "+response.status);
      const result=await response.json();
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
      trailStatus("Зелёный след: мяч пока не обнаружен");
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
  let totalDuration=0,offset=0,sources=[],sortedActions=[];
  let rallyOrder=[],rallySort="normal";
  function restoreRallySort(){
    try{rallySort=localStorage.getItem("volley-result-rally-sort")||"normal"}catch{}
    if(!["normal","duration","touches"].includes(rallySort))rallySort="normal";
    E("result-rally-sort").value=rallySort;
  }
  const rallyFields=["rally-gap-sec","rally-min-sec","rally-context-sec"];
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
    const view=E("view-analysis-results");
    // Keep project source checkboxes available even before a run finishes.
    view.querySelector(".result-media-panel").hidden=false;
    view.querySelector(".result-video-layout").hidden=!yes;
    view.querySelector(".analysis-video-divider").hidden=!yes;
    view.querySelector(".result-timeline").hidden=!yes;
    view.querySelector(".result-rallies-panel").hidden=!yes;
    E("result-unavailable").hidden=yes;
  }
  function renderSources(){
    E("result-source-count").textContent=sources.length+" видео";
    E("result-source-list").innerHTML=sources.length?sources.map((name,i)=>
      '<div class="result-source-entry">'+
      '<button type="button" class="result-source'+(name===source?' active':'')+
      '" data-result-source="'+esc(name)+'" aria-pressed="'+(name===source)+'" title="Предпросмотр: '+esc(name)+'">'+
      '<span class="result-source-index">'+String(i+1).padStart(2,"0")+'</span>'+
      '<span class="result-source-name">'+esc(name)+'</span></button></div>'
    ).join(""):'<div class="empty">В проекте нет исходных видео.</div>';
    const select=E("result-source-select");
    if(sources.includes(source))select.value=source;
  }
  function clear(){
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
    modelTouches.reset();
    E("result-rally-list").innerHTML='<div class="empty">Нет сохранённых розыгрышей.</div>';
    E("result-rally-track").innerHTML="";
    E("result-timeline-ticks").innerHTML="";
    E("result-rally-count").textContent="0";
    E("result-current-time").textContent="00:00:00";
    E("result-rally-summary").textContent="";
    E("result-overlay-ball").hidden=true;
    E("result-overlay-event").hidden=true;
    E("result-overlay-rally").hidden=true;
    E("result-overlay-touch").hidden=true;
    invalidateTrail();
    resetPlayerBoxes();
    renderEventCounts();
  }
  // Each block shows its own waiting state; no page-wide overlay.
  function showLoading(){
    ++loadId; // An earlier result request may not overwrite the new project.
    clear();
    setAvailable(true);
    E("result-source-list").innerHTML='<div class="empty" role="status">Данные загружаются…</div>';
    E("result-source-count").textContent="…";
    E("result-meta").textContent="Данные загружаются…";
    E("result-video-placeholder").hidden=false;
    E("result-video-placeholder").textContent="Данные загружаются…";
    E("result-rally-list").innerHTML='<div class="empty" role="status">Данные загружаются…</div>';
    E("result-rally-count").textContent="…";
    E("result-rally-summary").textContent="Данные загружаются…";
  }
  function renderList(){
    if(!data)return;
    const rallies=data.rallies||[];
    // One pass, then logarithmic range counts (rather than R x A filtering).
    const visibleActions=(data.actions||[]).filter(actionVisible);
    // Sort by detected contacts, independent of visibility and confidence toggles.
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
      const evidence=r.start_evidence==="detected_serve"?" · подача найдена":"";
      const joined=r.joined_parts>1?" · "+r.joined_parts+" трека":"";
      const chosen=selectedForMontage.has(r.rally_id);
      return '<div class="result-rally-entry'+(chosen?' is-selected':'')+'">'+
        '<button type="button" class="result-rally-item" data-rally="'+index+
        '" title="Воспроизвести розыгрыш" aria-current="false"><span class="result-rally-number">№ '+(index+1)+'</span>'+
        '<span class="result-rally-details"><b>'+clock(r.start)+' – '+clock(r.end)+'</b>'+
        '<small>'+duration.toFixed(1)+' с · '+touches+' касаний'+
        ' · игроков '+(r.player_count==null?'—':Number(r.player_count))+evidence+joined+
        '</small></span>'+
        '<span class="result-rally-play">▶</span></button>'+
        (r.rally_id?'<label class="result-rally-montage" title="Добавить этот розыгрыш в ручной монтаж">'+
        '<input type="checkbox" data-montage-rally="'+esc(r.rally_id)+'"'+
        (chosen?' checked':'')+' aria-label="Розыгрыш №'+(index+1)+' в ручной монтаж">'+
        '<span>В монтаж</span></label>':'')+'</div>';
    }).join(""):'<div class="empty">В выбранном видео модель не нашла завершённых розыгрышей.</div>';
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
  function setCacheSelection(map,runId=""){
    selectedCaches={...(map||{})};
    montageRunId=runId;
    invalidateTrail();
    resetPlayerBoxes();
  }
  async function setProject(projectId,videos=[],selected=null){
    const previousProject=project;
    project=projectId||"";
    if(previousProject!==project){
      restoreRallyConfig();
      restoreOverlays();
      restoreConfidence();
    }
    sources=videos.slice();
    const select=E("result-source-select");
    select.innerHTML=sources.map(name=>'<option value="'+esc(name)+'">'+esc(name)+'</option>').join("");
    const preferred=selected&&videos.includes(selected)?selected:(previousProject===project&&videos.includes(source)?source:videos[0]);
    if(preferred){
      source=preferred;
      renderSources();
      await setVideo(preferred);
    }else{
      source="";renderSources();clear();setAvailable(false);
      E("result-video-placeholder").hidden=false;
      E("result-video-placeholder").textContent="Создайте проект и запустите анализ";
    }
  }
  async function setVideo(name){
    const seq=++loadId;
    source=name;
    clear();
    setAvailable(true);
    renderSources();
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
      const resp=await fetch("/api/analysis-view"+query,{cache:"no-store"});
      const result=await resp.json();
      if(!resp.ok)throw Error(result.error||"Не удалось загрузить анализ");
      if(seq!==loadId)return;
      // Read the selected profile's immutable cache. A different profile
      // may be running at the same time; global job status must never hide it.
      if(!result.analyzed){
        E("result-video-placeholder").textContent=
          "Для выбранного профиля ещё нет завершённого анализа этого видео.";
        E("result-meta").textContent="Нет готового анализа";
        E("result-rally-list").innerHTML='<div class="empty">Для этого видео розыгрыши ещё не найдены.</div>';
        return;
      }
      setAvailable(true);
      data=result;
      restoreBallThreshold();
      restorePlayerThreshold();
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
        project,video:source,cache:selectedCaches[source]||"",
        duration:totalDuration,offset
      });
      renderEventCounts();
      renderList();
      updateOverlay();
    }catch(err){
      if(seq!==loadId)return;
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
    segment={index,start:mediaTime(r.start),end:mediaTime(r.end)};
    const player=v();
    player.pause();
    player.currentTime=Math.max(0,segment.start);
    syncRallySelection(true);
    E("result-overlay-rally").hidden=false;
    E("result-overlay-rally").textContent="РОЗЫГРЫШ № "+(index+1);
    // play() may require interaction (button click counts as gesture).
    const promise=player.play();
    // Switching rapidly between rallies can reject a superseded play request.
    if(promise&&promise.catch)promise.catch(()=>{});
  }
  function enforceEnd(mediaEnded=false){
    const player=v(),current=segment;
    if(!current||(!mediaEnded&&(player.paused||player.seeking))||!Number.isFinite(player.currentTime))return;
    if(mediaEnded||player.currentTime>=current.end-.035){
      const position=rallyOrder.findIndex(item=>item.index===current.index);
      const next=position>=0?rallyOrder[position+1]:null;
      if(next){selectRally(next.index);return;}
      player.pause();
      player.currentTime=Math.min(current.end,Number.isFinite(player.duration)?player.duration:current.end);
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
  function updateOverlay(){
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
      ballGeometry();
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
      badge.textContent=(typed?actionText[near.type]:"Касание")+" · "+score+"%"+
        (near.player_id!=null?" · ID "+near.player_id:"")+
        (score<50?" · низкая уверенность":"");
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
    restoreRallySort();
    E("result-rally-sort").addEventListener("change",e=>{
      rallySort=["normal","duration","touches"].includes(e.target.value)?e.target.value:"normal";
      try{localStorage.setItem("volley-result-rally-sort",rallySort)}catch{}
      if(data)renderList();
      syncRallySelection(true);
    });
    modelTouches.init(()=>{
      segment=null;
      syncRallySelection();
      updateOverlay();
    });
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
    restoreConfidence();
    E("result-ball-threshold").addEventListener("input",e=>{
      const value=e.target.value;
      E("result-ball-threshold-value").textContent=Math.round(Number(value)*100)+"%";
      if(ballThresholdTimer!==null)clearTimeout(ballThresholdTimer);
      ballThresholdTimer=setTimeout(()=>setBallThreshold(value),180);
    });
    E("result-ball-threshold").addEventListener("change",e=>setBallThreshold(e.target.value));
    E("result-ball-threshold-reset").addEventListener("click",()=>setBallThreshold(null));
    E("result-player-threshold").addEventListener("input",e=>{
      const value=e.target.value;
      E("result-player-threshold-value").textContent=Math.round(Number(value)*100)+"%";
      if(playerThresholdTimer!==null)clearTimeout(playerThresholdTimer);
      playerThresholdTimer=setTimeout(()=>setPlayerThreshold(value),180);
    });
    E("result-player-threshold").addEventListener("change",e=>setPlayerThreshold(e.target.value));
    E("result-player-threshold-reset").addEventListener("click",()=>setPlayerThreshold(null));
    E("result-confidence-threshold").addEventListener("change",e=>setConfidence(e.target.value));
    document.querySelector(".result-confidence-options").addEventListener("click",e=>{
      const button=e.target.closest("[data-result-confidence]");
      if(button)setConfidence(button.dataset.resultConfidence);
    });
    for(const control of document.querySelectorAll("[data-result-overlay]")){
      control.addEventListener("change",()=>{
        overlays[control.dataset.resultOverlay]=control.checked;
        persistOverlays();
        if(control.dataset.resultOverlay==="trajectory")invalidateTrail();
        if(control.dataset.resultOverlay==="players")resetPlayerBoxes();
        if(data){renderEventCounts();renderList();updateOverlay();}
      });
    }
    rallyFields.forEach(id=>E(id).addEventListener("change",()=>{
      const cfg=rallyConfig();
      E(id).value=cfg[{"rally-gap-sec":"gap_sec","rally-min-sec":"min_sec","rally-context-sec":"context_sec"}[id]];
      storeRallyConfig();
      window.dispatchEvent(new Event("volley:rally-config-change"));
      if(source&&project)setVideo(source);
    }));
    E("result-source-select").addEventListener("change",e=>setVideo(e.target.value));
    E("result-source-list").addEventListener("click",e=>{
      const button=e.target.closest("[data-result-source]");
      if(button&&button.dataset.resultSource!==source)
        setVideo(button.dataset.resultSource);
    });
    E("result-rally-list").addEventListener("change",e=>{
      const input=e.target.closest("[data-montage-rally]");
      if(!input)return;
      const selected=new Set(montageRallies());
      if(input.checked)selected.add(input.dataset.montageRally);
      else selected.delete(input.dataset.montageRally);
      setMontageRallies([...selected]);
    });
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
      // When user starts manual playback beyond an old selection, start regular playback.
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
  }
  function activeRally(){return segment?{...segment}:null}
  return {init,setProject,setVideo,showLoading,setCacheSelection,selectRally,activeRally,montageRallies,setMontageRallies};
})();
