"use strict";

const $ = (selector) => document.querySelector(selector);
const $$ = (selector) => Array.from(document.querySelectorAll(selector));
const state = {videos: [], video: "", projects: [], project: "", aggregate: null, report: null, renders: [],
  selected: new Set(), playerAssignBusy: false, limit: 30, previewId: null, photoToken: 0, jobStatus: "idle", latestTask: null,
  players: [], libraryPlayers: [], photoCounts: {}, activePlayerId: null, playerFilter: "unassigned", suggestionsToken: 0,
  playerGroups: null, playerGroupsKey: "", playerMode: "view", albumToken: 0, reportDirty: false,
  playerReidThresholdPending: null,
  dialogEdit: false, dialogBusy: false, dialogRequestId: 0, analysisContext: null,
  preparationProject: "", preparationPending: false, preparationActive: false,
  benchmarkReady: false, benchmarkRenderedId: null, analysisRuns: [], analysisRunId: "", reportEpoch: 0};
// Context changes use one shared, delayed overlay instead of blank intermediate screens.
const workspaceLoadingTokens=new Set();
let workspaceLoadingSerial=0, playerCatalogCache=null;
function beginWorkspaceLoading(message="Загрузка данных…"){
  const token=++workspaceLoadingSerial;
  workspaceLoadingTokens.add(token);
  const overlay=$("#workspace-loading-overlay");
  const show=()=>{
    if(token!==workspaceLoadingSerial||!workspaceLoadingTokens.has(token)||!overlay)return;
    overlay.hidden=false;
    if(token===workspaceLoadingSerial)
      $("#workspace-loading-text").textContent=message;
    document.querySelector("main")?.setAttribute("aria-busy","true");
  };
  const timer=setTimeout(show,90); // No visible flash for cache-speed transitions.
  return ()=>{
    clearTimeout(timer);
    workspaceLoadingTokens.delete(token);
    if(token===workspaceLoadingSerial){
      if(overlay)overlay.hidden=true;
      document.querySelector("main")?.removeAttribute("aria-busy");
    }
  };
}
async function sharedPlayerCatalogs(){
  if(playerCatalogCache && Date.now()-playerCatalogCache.at<15000)
    return playerCatalogCache.promise;
  const promise=Promise.all([
    api("/api/players/library"),api("/api/players/photos")
  ]).then(([library,photos])=>({
    libraryPlayers:library.players||[],photoCounts:photos.counts||{}
  }));
  playerCatalogCache={at:Date.now(),promise};
  try{return await promise;}
  catch(error){
    if(playerCatalogCache?.promise===promise)playerCatalogCache=null;
    throw error;
  }
}
let pendingPreviewReplay=null;
let activeHeaderContext="";
let videoSelectionGeneration=0;
const headerVideoKey=projectId=>"volley-header-video:"+projectId;
const headerRunKey=(projectId=state.project,video=state.video)=>
  "volley-header-run:"+projectId+":"+encodeURIComponent(video);
const contextIdentity=(projectId=state.project,video=state.video)=>
  projectId+"\u0000"+video;
const label = {attack:"Атаки", block:"Блоки", serve:"Подачи", dig:"Приёмы", overhead_pass:"Верхние передачи", low_touch:"Другие касания"};
const eventOrder = ["attack", "block", "serve", "dig", "overhead_pass", "low_touch"];

function esc(value) {
  return String(value ?? "").replace(/[&<>"']/g, ch => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[ch]));
}
async function api(url, payload) {
  const opts = payload === undefined ? {cache:"no-store"} : {
    method:"POST", headers:{"Content-Type":"application/json"},
    body:JSON.stringify(payload), cache:"no-store"
  };
  const response = await fetch(url, opts);
  let data;
  try { data=await response.json(); }
  catch { throw new Error("Сервер вернул неожиданный ответ: "+url); }
  if(!response.ok){
    const error=data.error||"Ошибка запроса";
    if(response.status===404&&url.startsWith("/api/")&&
      (error==="Страница не найдена"||error==="Неизвестная команда")){
      throw new Error("Сервер Volley Studio устарел. Перезапустите программу через start-volley-ui.cmd.");
    }
    throw new Error(error);
  }
  if(payload!==undefined && (/^\/api\/(players\/|assign$|unassign$|player-groups\/link$)/).test(url))
    playerCatalogCache=null;
  return data;
}
function alertBox(message) {
  $("#alerts").textContent = message || "";
  if (message) window.scrollTo({top:0,behavior:"smooth"});
}
const WORKSPACE_VIEWS=new Set(["analysis","analysis-results","players","report","benchmark","results"]);
const SELECTED_PROJECT_KEY="volley-selected-project";
let projectLoadEpoch=0;
let analysisRunsRequest=0;
function go(view) {
  if(!WORKSPACE_VIEWS.has(view))view="analysis";
  if(!state.project&&view!=="analysis")view="analysis";
  // The active route controls first-paint CSS as well as interactive navigation.
  document.documentElement.dataset.initialView=view;
  // Persist the active tab in the address, preserving other query parameters
  // and hash. Reload (F5 / Ctrl+F5) must restore the same workspace tab.
  const url=new URL(window.location.href);
  if(url.searchParams.get("view")!==view){
    url.searchParams.set("view",view);
    window.history.replaceState(window.history.state,"",url.href);
  }
  $$(".nav-button").forEach(b => b.classList.toggle("active",b.dataset.view===view));
  $$(".view").forEach(v => v.classList.toggle("active",v.id==="view-"+view));
  const names={analysis:"Анализ","analysis-results":"Предпросмотр",players:"Игроки",report:"Отчёт проекта",benchmark:"Бенчмарк",results:"Монтаж"};

  if(view==="players"&&state.project){
    setPlayerMode(state.playerMode);
    if(state.playerMetaProject!==state.project){
      const finish=beginWorkspaceLoading("Загрузка игроков…");
      loadReport({playersOnly:true})
        .catch(error=>alertBox(error.message)).finally(finish);
    }else{
      renderPlayers();
      if(state.playerMode==="manual"){
        loadPlayerSuggestions().catch(console.warn);
        renderPlayerPhotos().catch(console.warn);
      }
    }
  }
  if(view==="report"&&state.project&&state.reportDirty){
    state.reportDirty=false;
    loadReport().catch(error=>{
      state.reportDirty=true;
      $("#report-notice").textContent="Не удалось обновить статистику: "+error.message;
    });
  }
  if(view==="benchmark"&&state.project)
    loadBenchmarkResults().catch(console.warn);
  if(view==="results"&&state.project)scheduleMontagePlan();
  if(view==="analysis-results"&&state.project){
    const project=selectedProject();
    window.VolleyballResultsUI.setCacheSelection(analysisCacheMap(),state.analysisRunId,
      selectedAnalysisRun()?.postprocess_settings||null);
    const finish=beginWorkspaceLoading("Загрузка предпросмотра…");
    window.VolleyballResultsUI.setProject(state.project,project.videos,state.video)
      .catch(err=>alertBox(err.message)).finally(finish);
  } else if(view!=="analysis-results"){
    const player=$("#result-video-player");
    if(player)player.pause();
  }
}
function metric(name,value,sub) {
  return '<div class="metric"><div class="metric-name">'+esc(name)+'</div><div class="metric-num">'+esc(value)+'</div><div class="metric-sub">'+esc(sub)+'</div></div>';
}
function barRows(counts) {
  if (!counts || !Object.values(counts).some(Boolean)) return '<div class="empty">Пока нет событий. Запустите анализ.</div>';
  const max=Math.max(1,...eventOrder.map(k=>counts[k]||0));
  return eventOrder.map(k=>{
    const count=counts[k]||0;
    const pct=Math.round(count/max*100);
    return '<div class="bar-item"><div class="bar-label"><span>'+esc(label[k])+'</span><strong>'+count+'</strong></div><div class="bar-track"><div class="bar-fill" style="width:'+pct+'%"></div></div></div>';
  }).join("");
}
function renderFiles(files) {
  if (!files || !files.length) return '<div class="empty">Пока нет готовых роликов</div>';
  return files.map(f=>'<button type="button" class="file-row render-file" data-file="'+esc(f.name)+'" data-url="'+esc(f.url||"")+'"><span class="file-icon">▣</span><span class="file-info"><strong>'+esc(f.name)+'</strong><small>MP4 · '+esc(f.size_mb)+' МБ</small></span><span class="file-arrow">↗</span></button>').join("");
}
function renderReport() {
  const r=state.aggregate||state.report||{};
  const c=r.counts||{};
  $("#report-notice").textContent=r.analyzed
    ? "Статистика суммируется по всем обработанным видео проекта. Track ID относятся к конкретной записи; отсутствие метки не доказывает отсутствие действия. Без разметки площадки подачи и блоки могут не распознаваться."
    : "Нет обработанных записей проекта. Перейдите во вкладку «Анализ».";
  $("#report-metrics").innerHTML=[
    metric("Фрагменты игры",r.rallies??"—","Не подтверждённые очки"),
    metric("Атаки",c.attack??"—","Распознанные"),
    metric("Приёмы",c.dig??"—","Распознанные"),
    metric("Передачи",c.overhead_pass??"—","Распознанные")
  ].join("");
  $("#report-bars").innerHTML=barRows(c);
  $("#report-people").innerHTML=r.groups?.length ? r.groups.map(g=>
    '<div class="person-card"><div class="person-line"><strong>'+esc(g.name)+'</strong><span class="person-count">'+g.total_actions+' действий</span></div><small>Track ID: '+esc(g.track_ids.join(", "))+'</small><small>Атаки '+(g.actions.attack||0)+' · Приёмы '+(g.actions.dig||0)+' · Передачи '+(g.actions.overhead_pass||0)+'</small></div>'
  ).join("") : '<div class="empty">Пока нет подписанных игроков. Перейдите во вкладку «Игроки и ID».</div>';
}
function playerPhotoUrl(id,sample=0) {
  return "/api/player-photo?video="+encodeURIComponent(state.video)+"&id="+encodeURIComponent(id)+
    "&sample="+sample+"&project_id="+encodeURIComponent(state.project||"")+
    "&cache_id="+encodeURIComponent(analysisCacheId());
}
function updateChosen() {
  const size=state.selected.size;
  const active=state.players.find(p=>p.id===state.activePlayerId);
  $("#selected-ids").textContent=size
    ?size+" ID выбрано · #"+Array.from(state.selected).slice(0,8).join(", #")+(size>8?"…":"")
    :"0 ID выбрано";
  $("#assigned-player-name").textContent=state.playerAssignBusy
    ?"Сохраняем привязку Track ID…"
    :size?"Выбрано "+size+" ID — выберите игрока и сохраните"
    :"Отметьте ID для объединения или привязки";
  const assignedIds=associatedMap();
  const hasAssigned=Array.from(state.selected).some(id=>assignedIds.has(id));
  $("#unassign-ids").hidden=!hasAssigned;
  $("#unassign-ids").disabled=!size||!hasAssigned||state.playerAssignBusy;
  $("#named-list").classList.toggle("has-pending-selection",size>0);
  $("#named-list").querySelectorAll("[data-player-id]").forEach(button=>{
    button.disabled=state.playerAssignBusy;
    button.setAttribute("aria-label",size
      ?"Выбрать игрока "+button.querySelector(".player-slot-text strong").textContent+
        " как цель для "+size+" Track ID (сохранение отдельной кнопкой)"
      :"Выбрать игрока "+button.querySelector(".player-slot-text strong").textContent);
  });
  $("#clear-selected").disabled=!size;
  $("#save-selected-ids").disabled=!size||!$("#bulk-player-target").value||state.playerAssignBusy;
  const visible=$$(".row-select");
  const selectedVisible=visible.filter(el=>state.selected.has(el.dataset.id)).length;
  const all=$("#select-all-ids");
  all.checked=visible.length>0&&selectedVisible===visible.length;
  all.indeterminate=selectedVisible>0&&selectedVisible<visible.length;
}
// Batch rapid checkbox changes; stale photo requests cannot overwrite new ones.
let photoRefreshTimer=null;
function schedulePlayerPhotos(){
  state.photoToken++;
  clearTimeout(photoRefreshTimer);
  photoRefreshTimer=setTimeout(()=>{
    photoRefreshTimer=null;
    renderPlayerPhotos().catch(error=>console.warn("photos",error));
  },120);
}
async function renderPlayerPhotos() {
  const gallery=$("#player-photo-gallery");
  const ids=state.selected.size?Array.from(state.selected).slice(0,6):(state.previewId?[state.previewId]:[]);
  const token=++state.photoToken, video=state.video;
  if(!ids.length){gallery.innerHTML='<div class="empty">Нажмите на фото или отметьте ID в таблице</div>';return}
  gallery.innerHTML='<div class="muted">Загрузка фотографий из сохранённых кадров…</div>';
  const cards=await Promise.all(ids.map(async id=>{
    try {
      const meta=await api("/api/player-photos?video="+encodeURIComponent(video)+"&id="+encodeURIComponent(id)+
        "&project_id="+encodeURIComponent(state.project||"")+
        "&cache_id="+encodeURIComponent(analysisCacheId()));
      if(!meta.samples.length)return '<div class="empty">#'+esc(id)+': фото пока отсутствуют</div>';
      return '<div class="photo-person"><div class="photo-person-head"><strong>Track #'+esc(id)+'</strong><span>'+meta.samples.length+' кадр(а)</span></div><div class="photo-strip">'
        +meta.samples.map(s=>'<a data-photo-track-id="'+esc(id)+'" href="'+esc(s.url)+'" target="_blank" rel="noopener" title="Открыть кадр"><img loading="lazy" src="'+esc(s.url)+'" alt="Игрок, track #'+esc(id)+', кадр '+s.frame+'"><small>'+s.time_sec.toFixed(1)+' сек</small></a>').join("")
        +'</div></div>';
    }catch(error){return '<div class="empty">#'+esc(id)+': '+esc(error.message)+'</div>'}
  }));
  if(token!==state.photoToken||video!==state.video)return;
  gallery.innerHTML=cards.join("")+(state.selected.size>6?'<div class="muted">Показаны первые 6 выбранных ID. Остальные останутся выбранными при сохранении.</div>':'');
}
let playerGroupsPoll=null,playerGroupsSequence=0;
let commitPlayerReidThreshold=null,playerReidThresholdSaving=false;
function playerGroupsContext(){
  return [state.project,state.video,analysisCacheId()||""].join("\u0000");
}
function appearanceGroupMap(){
  const map=new Map();
  for(const group of state.playerGroups?.status==="ready"?(state.playerGroups.groups||[]):[]){
    for(const tid of group.track_ids)map.set(String(tid),group);
  }
  return map;
}
async function loadPlayerGroups(){
  clearTimeout(playerGroupsPoll);
  const seq=++playerGroupsSequence, context=playerGroupsContext();
  const [project_id,video,cache_id]=context.split("\u0000");
  if(!project_id||!video||!cache_id){
    state.playerGroups=null;state.playerGroupsKey="";renderPlayerGroups();return;
  }
  if(state.playerGroupsKey!==context){
    state.playerGroups=null;state.playerGroupsKey=context;
    renderPlayerGroups();
  }
  try{
    const params=new URLSearchParams({project_id,video,cache_id});
    const result=await api("/api/player-groups?"+params);
    if(seq!==playerGroupsSequence||context!==playerGroupsContext())return;
    state.playerGroups=result;
    if(result.status==="ready"&&state.report&&Object.keys(result.auto_links||{}).length){
      const associated=associatedMap();
      for(const [pid,ids] of Object.entries(result.auto_links)){
        const player=state.players.find(entry=>entry.id===pid);
        const free=(ids||[]).filter(id=>!associated.has(String(id)));
        if(player&&free.length){
          patchAssignedTrackData(free,player);
          const stored=(state.report.groups||[]).find(group=>group.id===pid);
          if(stored){
            const meta=stored.link_meta||(stored.link_meta={});
            for(const tid of free)meta[String(tid)]={source:"reid_auto",reviewed:false};
          }
          state.reportDirty=true;
        }
      }
    }
    const deferred=state.playerReidThresholdPending;
    if(result.status==="ready"&&deferred?.context===context){
      state.playerReidThresholdPending=null;
      if(deferred.value!==Number(result.threshold_percent))
        commitPlayerReidThreshold?.(deferred.value).catch(console.warn);
    }
    // The manual table needs ReID hints; the automatic list needs only its own
    // cards. Never rebuild the entire players workspace for an auto refresh.
    if(state.playerMode==="manual")renderPlayers();
    else renderPlayerGroups();
    if(result.status==="running"){
      playerGroupsPoll=setTimeout(()=>loadPlayerGroups().catch(console.warn),1200);
    }
  }catch(error){
    if(seq!==playerGroupsSequence||context!==playerGroupsContext())return;
    state.playerGroups={status:"error",error:error.message};
    renderPlayerGroups();
  }
}
// ReID supplies the best cosine similarity per Track ID, not a calibrated
// probability. For a group, prefer the player supported by the most IDs,
// breaking ties by mean similarity (never by the first ID alone).
const playerPickerOverrides=new Map();
function suggestedPlayerForTracks(ids){
  const matches=state.playerGroups?.status==="ready"?state.playerGroups.player_matches||{}:{};
  const roster=new Map(state.players.map(player=>[player.id,player]));
  const votes=new Map();
  for(const id of ids){
    const match=matches[String(id)],player=roster.get(match?.player_id);
    if(!player||match.similarity==null||!Number.isFinite(Number(match.similarity)))continue;
    const item=votes.get(player.id)||{playerId:player.id,name:player.name,
      count:0,total:0,weak:0,ambiguous:0};
    item.count++;
    item.total+=Number(match.similarity);
    if(!match.above_threshold)item.weak++;
    if(match.ambiguous)item.ambiguous++;
    votes.set(player.id,item);
  }
  const sorted=[...votes.values()].sort((a,b)=>
    b.count-a.count||(b.total/b.count)-(a.total/a.count));
  if(!sorted.length)return null;
  const best=sorted[0],coverage=best.count+"/"+ids.length;
  return {playerId:best.playerId,name:best.name,
    similarity:best.total/best.count,coverage,
    weak:best.weak>0,ambiguous:best.ambiguous>0||
      !!(sorted[1]&&sorted[1].count===best.count)};
}
function playerPickerHtml(className,ids=null,pickKey=""){
  const suggestion=ids?suggestedPlayerForTracks(ids):null;
  const overrideKey=pickKey?playerGroupsContext()+"\\0"+pickKey:"";
  const selected=overrideKey&&playerPickerOverrides.has(overrideKey)
    ?playerPickerOverrides.get(overrideKey)
    :suggestion?.playerId||(ids? "":state.activePlayerId||"");
  const ordered=suggestion?[
    ...state.players.filter(p=>p.id===suggestion.playerId),
    ...state.players.filter(p=>p.id!==suggestion.playerId)
  ]:state.players;
  return '<select class="'+esc(className)+'" aria-label="Игрок проекта"'+
    (pickKey?' data-player-pick-key="'+esc(pickKey)+'"':'')+'>'+
    '<option value=""'+(!selected?' selected':'')+'>Выберите игрока…</option>'+
    ordered.map(p=>'<option value="'+esc(p.id)+'"'+
      (p.id===selected?' selected':'')+'>'+esc(p.name)+
      (p.id===suggestion?.playerId?' · '+Math.round(suggestion.similarity*100)+'% сходство':'')+
      '</option>').join("")+'</select>';
}
function suggestedPlayerHint(ids){
  const suggestion=suggestedPlayerForTracks(ids);
  if(!suggestion)return '<small class="player-reid-pick-hint">Нет подсказки ReID — выберите игрока вручную</small>';
  const uncertain=suggestion.ambiguous?' · неоднозначно, проверьте':
    suggestion.weak?' · ниже порога, проверьте':'';
  return '<small class="player-reid-pick-hint">ReID: '+esc(suggestion.name)+
    ' · '+Math.round(suggestion.similarity*100)+'% сходство'+
    (ids.length>1?' · '+suggestion.coverage+' ID':'')+uncertain+
    '</small>';
}
function rememberPlayerPickerChange(select){
  if(!select?.dataset.playerPickKey)return;
  playerPickerOverrides.set(playerGroupsContext()+"\\0"+select.dataset.playerPickKey,select.value);
}
function setPlayerMode(mode){
  state.playerMode=["view","auto","manual"].includes(mode)?mode:"view";
  $$("[data-player-mode]").forEach(button=>{
    const active=button.dataset.playerMode===state.playerMode;
    button.classList.toggle("is-active",active);
    button.setAttribute("aria-pressed",String(active));
  });
  $$("[data-player-pane]").forEach(section=>{
    section.hidden=section.dataset.playerPane!==state.playerMode;
  });
  if(state.playerMode==="view")renderPlayerAlbum().catch(console.warn);
  if(state.playerMode!=="view"&&state.project&&
      (!state.playerGroups||state.playerGroups.status==="running"))
    loadPlayerGroups().catch(console.warn);
  else if(state.playerMode==="auto")renderPlayerGroups();
}
function renderPlayerGroups(){
  if(state.playerGroupsKey!==playerGroupsContext())state.playerGroups=null;
  const status=$("#player-groups-status"),list=$("#player-groups-list");
  if(!status||!list)return;
  const result=state.playerGroups;
  const reidThreshold=$("#player-reid-threshold");
  const autoEnable=$("#player-auto-assign-enabled");
  const autoThreshold=$("#player-auto-assign-threshold");
  if(autoEnable&&document.activeElement!==autoEnable)
    autoEnable.checked=!!result?.auto_assign_enabled;
  if(autoThreshold&&document.activeElement!==autoThreshold)
    autoThreshold.value=String(result?.auto_assign_threshold_percent??97);
  if(autoEnable)autoEnable.disabled=!state.project||!analysisCacheId()||result?.status==="running";
  if(autoThreshold)autoThreshold.disabled=!state.project||!analysisCacheId()||result?.status==="running";
  const thresholdNumber=$("#player-reid-threshold-number");
  const deferred=state.playerReidThresholdPending;
  const threshold=Number(
    deferred?.context===playerGroupsContext()?deferred.value:(result?.threshold_percent??85));
  if(document.activeElement!==reidThreshold&&document.activeElement!==thresholdNumber){
    reidThreshold.value=String(threshold);
    thresholdNumber.value=String(threshold);
  }
  $("#player-reid-threshold-value").textContent=threshold+"%";
  const editingAllowed=!!state.project&&!!analysisCacheId()&&!playerReidThresholdSaving;
  reidThreshold.disabled=!editingAllowed;
  thresholdNumber.disabled=!editingAllowed;
  $("#player-groups-rebuild").disabled=!analysisCacheId()||result?.status==="running";
  if(!state.project||!analysisCacheId()){
    status.textContent="Выберите завершённый анализ с обнаруженными игроками.";
    list.innerHTML="";return;
  }
  if(!result){
    status.textContent="Загрузка сохранённых групп…";
    list.innerHTML='<p class="muted">Ожидаем результаты ReID без повторного GPU-инференса.</p>';return;
  }
  if(result.status==="error"){
    status.textContent="Ошибка группировки: "+(result.error||"неизвестная ошибка");
    list.innerHTML="";return;
  }
  const assigned=associatedMap(),groups=result.groups||[];
  const cards=[];
  for(const group of groups){
    // A group must contain at least two still-unassigned IDs. Only groups
    // with an actual ReID score at or above the configured threshold qualify.
    // Legacy manual merges and geometry-only retracking are not ReID groups.
    if(group.source==="manual"||group.source==="automatic_retrack"||
       group.similarity==null||!Number.isFinite(Number(group.similarity))||
       Number(group.similarity)*100+0.001<threshold)continue;
    const free=[...new Set((group.track_ids||[]).map(String))]
      .filter(id=>!assigned.has(id));
    if(free.length<2)continue;
    cards.push({groupId:group.id,ids:free,source:group.source,similarity:group.similarity});
  }
  const grouped=new Set(cards.flatMap(card=>card.ids));
  const unassigned=(state.report?.raw||[]).filter(row=>
    !assigned.has(String(row.player_track_id))).length;
  const manualOnly=Math.max(0,unassigned-grouped.size);
  const progress=result.status==="running"
    ?"Пересчитываем группы: "+(result.stage||"обработка")+" · "+(result.progress||0)+"%. ":"";
  const queued=deferred?.context===playerGroupsContext()&&result.status!=="ready"
    ?"Порог "+threshold+"% ожидает завершения текущего пересчёта. ":"";
  const autoLinked=Object.values(result.auto_links||{}).reduce((n,ids)=>n+ids.length,0);
  const autoLabel=result.auto_assign_enabled
    ?" · Автопривязка ≥"+result.auto_assign_threshold_percent+"%: "+autoLinked+" новых ID"
    :" · Автопривязка отключена";
  status.textContent=queued+progress+"Групп, прошедших порог "+threshold+"%: "+cards.length+
    " · ID вне групп: "+manualOnly+" (доступны в «Ручной привязке»)."+
    autoLabel+" · Непривязанные группы подтверждаются вручную.";
  list.innerHTML=cards.slice(0,120).map(item=>{
    const ids=item.ids;
    const first=ids[0];
    const likely=suggestedPlayerHint(ids);
    const source="Сходство внутри группы ReID: "+
      Math.round(Number(item.similarity)*100)+"%";
    return '<div class="player-appearance-card">'+
      '<img loading="lazy" data-photo-track-id="'+esc(first)+'" tabindex="0" src="'+
        esc(playerPhotoUrl(first))+'" alt="Фото Track ID #'+esc(first)+'" onerror="this.style.opacity=.15">'+
      '<div class="player-appearance-info"><strong>'+
        ids.length+" сгруппированных Track ID</strong>"+
        '<small>'+esc(ids.slice(0,15).map(id=>"#"+id).join(", "))+
        (ids.length>15?" …":"")+'</small><small>'+source+'</small>'+likely+'</div>'+
      '<div class="player-appearance-actions">'+playerPickerHtml(
        "group-player-picker",ids,"g:"+item.groupId)+
        '<button type="button" class="primary small" data-group-link="'+
          esc(item.groupId)+'"'+
          (state.players.length?'':' disabled')+'>Привязать</button>'+
      '</div></div>';
  }).join("")||'<div class="empty">Групп, прошедших порог '+threshold+
    '%, нет. Одиночные и неподходящие Track ID доступны в «Ручной привязке».</div>';
  if(cards.length>120)list.insertAdjacentHTML("beforeend",
    '<p class="muted">Показаны 120 из '+cards.length+
    ' групп. Все остальные ID доступны в «Ручной привязке».</p>');
}
function associatedMap() {
  const map=new Map();
  (state.report?.groups||[]).forEach(g=>g.track_ids.forEach(id=>map.set(String(id),g.name)));
  return map;
}
function playerFilterSet(raw,association){
  const filter=state.playerFilter;
  const search=$("#track-search").value.trim().toLowerCase();
  return raw.filter(item=>{
    const id=String(item.player_track_id),assigned=association.has(id);
    const seconds=Number(item.duration_sec||0);
    const guess=state.playerGroups?.player_matches?.[id]?.name||"";
    if(search&&!id.includes(search)&&!(association.get(id)||"").toLowerCase().includes(search)
      &&!guess.toLowerCase().includes(search))return false;
    if(filter==="main")return seconds>=10;
    if(filter==="assigned")return assigned;
    if(filter==="unassigned")return !assigned;
    if(filter==="short")return seconds<3;
    return true;
  }).sort((a,b)=>(Number(b.detected_frames||0)-Number(a.detected_frames||0))||
    (Number(b.total_actions||0)-Number(a.total_actions||0)));
}
// Selecting a checkbox only updates existing rows and selection controls,
 // not hundreds of <tr>, <img> and player roster nodes.
 function syncTrackSelection(){
  for(const el of $("#raw-players").querySelectorAll(".row-select")){
    const selected=state.selected.has(el.dataset.id);
    if(el.checked!==selected)el.checked=selected;
    el.closest("tr")?.classList.toggle("is-selected",selected);
  }
  updateChosen();
  renderPlayerGroups();
}
function renderPlayers(){
  window.VolleyballPlayerPhotoHover?.dismiss?.();
  // Group IDs belong to a precise GPU inference, never to the adjacent video
  // or CPU reprocessing version selected while another request is in flight.
  if(state.playerGroupsKey!==playerGroupsContext())state.playerGroups=null;
  const r=state.report||{},raw=r.raw||[],groups=r.groups||[];
  const autoLinked=new Set(groups.flatMap(group=>
    Object.entries(group.link_meta||{}).filter(([,meta])=>
      meta?.source==="reid_auto"&&!meta.reviewed).map(([id])=>String(id))));
  const association=associatedMap();
  const appearance=appearanceGroupMap();
  const missingVisual=new Set(state.playerGroups?.missing_visual_ids||[]);
  const matched=playerFilterSet(raw,association);
  const shown=matched.slice(0,state.limit);
  const assignedCount=raw.filter(x=>association.has(String(x.player_track_id))).length;
  const groupCounts=new Map();
  for(const item of raw){
    const gid=appearance.get(String(item.player_track_id))?.id||
      (state.playerGroups?.status==="ready"?"none-"+item.player_track_id:
        String(item.candidate_group_id||("candidate-"+item.player_track_id)));
    groupCounts.set(gid,(groupCounts.get(gid)||0)+1);
  }
  $("#raw-count").textContent=matched.length+" из "+raw.length+" ID";
  $("#tracks-heading").textContent=state.playerFilter==="unassigned"
    ?"Нераспознанные Track ID":"Найденные Track ID";
  $("#players-health").innerHTML=[
    '<div class="player-health-metric"><strong>'+state.players.length+'</strong><span>Игроков в проекте</span></div>',
    '<div class="player-health-metric"><strong>'+raw.length+'</strong><span>'+
      (r.replay_filtered_tracks?'Активных Track ID (CPU)':'Технических Track ID')+'</span></div>',
    '<div class="player-health-metric"><strong>'+assignedCount+'</strong><span>Привязано ID</span></div>',
    '<div class="player-health-metric"><strong>'+(raw.length-assignedCount)+'</strong><span>Осталось разобрать</span></div>'
  ].join("");
  const cpuTrackNote=r.replay_filtered_tracks
    ?"Активных Track ID в CPU-версии: "+raw.length+" из "+
      (r.source_track_count??raw.length)+" исходных GPU. Исходные ID сохранены. "
    :"Технических Track ID: "+raw.length+". ";
  $("#players-filter-note").textContent=raw.length
    ?cpuTrackNote+"Предварительных групп: "+
      (state.playerGroups?.status==="ready"?(state.playerGroups.groups||[]).length:
        r.candidate_groups_count??raw.length)+
      ". Группы — предположения, не количество реальных игроков. Привяжите каждую проверенную группу к игроку."
    :r.replay_filtered_tracks
      ?"В выбранной CPU-версии фильтр исключил все Track ID. Исходные ID сохранены в GPU-анализе."
      :"На этом видео пока нет найденных ID. Сначала запустите анализ игроков во вкладке «Анализ».";
  $("#raw-players").innerHTML=shown.map(row=>{
    const id=String(row.player_track_id),name=association.get(id);
    const matchedGroup=appearance.get(id);
    const guess=state.playerGroups?.status==="ready"?state.playerGroups.player_matches?.[id]:null;
    const neighbor=state.playerGroups?.status==="ready"?state.playerGroups.nearest_tracks?.[id]?.[0]:null;
    const guessName=state.players.find(p=>p.id===guess?.player_id)?.name||guess?.name||"";
    const guessCell=guess
      ?'<strong class="player-reid-guess'+(guess.above_threshold&&!guess.ambiguous?' is-likely':'')+'">'+
       esc(guessName)+'</strong><small class="track-subline">'+(guess.confirmed
         ?'Подтверждено вручную'
         :'Сходство '+Math.round(guess.similarity*100)+'% · '+
           (guess.ambiguous?'неоднозначно':guess.above_threshold?'выше порога':'ниже порога'))+
       ' · образцов: '+Number(guess.reference_count||0)+'</small>'
      :'<span class="muted">'+(state.playerGroups?.status==="running"?"Сравнение…":
        state.playerGroups?.status==="ready"
          ?missingVisual.has(id)?"Нет пригодного кадра":"Нет эталонов игроков"
          :"Ожидание ReID")+'</span>';
    const candidate=matchedGroup?.id||
      (state.playerGroups?.status==="ready"?"none-"+id:
        String(row.candidate_group_id||("candidate-"+id)));
    const linkedSize=matchedGroup?.track_ids?.length||(state.playerGroups?.status==="ready"
      ?1:groupCounts.get(candidate)||1);
    const duration=Number(row.duration_sec||0),score=Number(row.mean_score||0);
    const frames=Number(row.detected_frames||0);
    const actions=Number(row.total_actions||0);
    const time=Number.isFinite(Number(row.start_sec))&&Number.isFinite(Number(row.end_sec))
      ?clock(row.start_sec)+" – "+clock(row.end_sec):"—";
    return '<tr class="player-track-row'+(name?' is-assigned':'')+(state.selected.has(id)?' is-selected':'')+'">'+
      '<td><input type="checkbox" class="row-select" data-id="'+esc(id)+'" aria-label="Выбрать Track ID '+esc(id)+'" '+(state.selected.has(id)?'checked':'')+'></td>'+
      '<td><button type="button" class="photo-peek" data-id="'+esc(id)+'" aria-label="Выбрать Track ID #'+esc(id)+' · наведите для увеличенных фотографий">'+
        '<img loading="lazy" src="'+esc(playerPhotoUrl(id))+'" alt="Кадр Track #'+esc(id)+'" onerror="this.style.opacity=0.15"></button></td>'+
      '<td><strong>#'+esc(id)+'</strong><small class="track-subline">'+
        (matchedGroup?"Группа ReID "+esc(candidate)+" · "+linkedSize+" ID":
          state.playerGroups?.status==="ready"?"Отдельный ID (без надёжной группы)":
            "Геометрическая цепочка "+esc(candidate)+" · "+linkedSize+" ID")+'</small>'+
        (linkedSize>1?'<button type="button" class="secondary small" data-select-candidate="'+
          esc(candidate)+'" title="Добавить все временные ID цепочки к выбору; привязку к игроку нужно подтвердить">'+
          'Выбрать цепочку</button>':'')+
        '<small class="track-subline">'+(score?Math.round(score*100)+'% уверенность детекции':'Без оценки детекции')+'</small>'+
        (neighbor?'<small class="track-subline">Ближайший ID: #'+esc(neighbor.track_id)+
           ' · сходство '+Math.round(neighbor.similarity*100)+'%</small>':'')+'</td>'+
      '<td><strong>'+duration.toFixed(1)+' с</strong><small class="track-subline">'+frames+' кадров · '+esc(time)+'</small></td>'+
      '<td><strong>'+actions+'</strong><small class="track-subline">'+(row.attack||0)+' атак · '+(row.dig||0)+' приёмов</small></td>'+
      '<td>'+(name?'<span class="tag">'+esc(name)+'</span>'+
        (autoLinked.has(id)?'<small class="track-subline" title="Автоматическое совпадение не является подтверждённым эталоном. Чтобы подтвердить, выберите ID и сохраните привязку вручную.">Авто ReID · не проверено</small>':''):
        '<div class="player-track-assign">'+playerPickerHtml("track-player-picker",[id],"t:"+id)+
        suggestedPlayerHint([id])+
        '<button type="button" class="primary small" data-track-link="'+esc(id)+'" '+
        (state.players.length?'':'disabled')+'>Привязать ID</button></div>')+'</td>'+
      '<td class="player-guess-cell">'+guessCell+'</td></tr>';
  }).join("")||'<tr><td colspan="7" class="empty">По этому фильтру ничего не найдено.</td></tr>';
  $("#table-hint").textContent="Показано "+shown.length+" из "+matched.length+" · всего "+raw.length+" ID";
  $("#show-more").disabled=shown.length>=matched.length;
  $("#select-filtered").disabled=!matched.length;
  $("#select-filtered").textContent=matched.length>200?"Выбрать первые 200 ID":"Выбрать все найденные ("+matched.length+")";
  $("#groups-count").textContent=state.players.length+" игроков";
  const assignedPlayerIds=new Set(state.players.map(player=>player.id));
  const options=state.libraryPlayers.filter(player=>!assignedPlayerIds.has(player.id));
  const picker=$("#library-player-select");
  const previous=picker.value;
  picker.innerHTML='<option value="">Выберите существующего игрока…</option>'+options.map(p=>
    '<option value="'+esc(p.id)+'">'+esc(p.name)+'</option>').join("");
  if(options.some(p=>p.id===previous))picker.value=previous;
  $("#add-library-player").disabled=!options.length;
  const bulk=$("#bulk-player-target"),prior=bulk.value;
  bulk.innerHTML=playerPickerHtml("bulk-picker")
    .replace(/^<select[^>]*>/,"").replace(/<\/select>$/,"");
  bulk.value=state.players.some(p=>p.id===prior)?prior:(state.activePlayerId||"");
  const groupById=new Map((state.report?.groups||[]).map(group=>[group.id,group]));
  $("#named-list").innerHTML=state.players.map(p=>{
    const current=groupById.get(p.id)?.track_ids||[];
    const active=p.id===state.activePlayerId;
    const preview=current.slice(0,5).map(x=>'#'+esc(x)).join(", ");
    return '<div class="player-roster-item"><button type="button" class="player-identity-slot'+(active?' is-active':'')+(state.selected.size&&active?' is-assign-target':'')+'" data-player-id="'+esc(p.id)+
      '" aria-pressed="'+active+'"><span class="player-slot-avatar">'+esc((p.name.trim()[0]||"?").toUpperCase())+
      '</span><span class="player-slot-text"><strong>'+esc(p.name)+'</strong><small>'+
      (current.length?current.length+' ID на этом видео · '+preview+(current.length>5?'…':''):'Пока нет ID на этом видео')+
      '</small><small>'+Number(state.photoCounts[p.id]||0)+' фото в личной базе</small></span><span class="player-slot-actions">'+
      (state.selected.size?(active?'Цель привязки':'Выбрать как цель'):current.length+' ID')+'</span></button>'+
      '<button type="button" class="player-roster-remove" data-remove-project-player="'+esc(p.id)+
        '" title="Убрать '+esc(p.name)+' из проекта (общая база сохранится)" aria-label="Убрать '+esc(p.name)+
        ' из проекта">×</button></div>';
  }).join("")||'<div class="player-roster-empty"><strong>Пока нет игроков</strong><p>Добавьте игрока из общей базы или создайте нового, затем отмечайте Track ID ниже.</p></div>';
  const active=state.players.find(p=>p.id===state.activePlayerId);
  $("#player-manage").hidden=!active;
  if(active&&document.activeElement!==$("#rename-player-name"))$("#rename-player-name").value=active.name;
  updateChosen();
  if(state.playerMode==="auto")renderPlayerGroups();
}
async function renderPlayerAlbum(force=false){
  const gallery=$("#player-album-gallery"),playerId=state.activePlayerId;
  if(!force&&gallery.dataset.playerId===String(playerId)&&gallery.dataset.loaded==="true")return;
  gallery.dataset.playerId=String(playerId);
  gallery.dataset.loaded="false";
  // Drop the height reservation only on an explicit album reload/navigation.
  gallery.style.minHeight="";
  const token=++state.albumToken,project=state.project;
  const player=state.players.find(p=>p.id===playerId);
  if(!player){
    gallery.innerHTML='<div class="empty">Выберите карточку игрока сверху, чтобы посмотреть его сохранённые фотографии.</div>';
    return;
  }
  gallery.innerHTML='<div class="muted">Загружаем личную фотобазу '+esc(player.name)+'…</div>';
  try{
    const album=await api("/api/players/album?player_id="+encodeURIComponent(playerId));
    if(token!==state.albumToken||project!==state.project||playerId!==state.activePlayerId)return;
    gallery.dataset.loaded="true";
    gallery.innerHTML=album.photos?.length?album.photos.map(photo=>{
      const url="/api/players/album-photo?player_id="+encodeURIComponent(playerId)+
        "&filename="+encodeURIComponent(photo.filename);
      return '<figure class="player-album-card"><a href="'+esc(url)+'" target="_blank" rel="noopener">'+
        '<img loading="lazy" src="'+esc(url)+'" alt="Сохранённое фото игрока '+esc(player.name)+'"></a>'+
        '<figcaption>Track #'+esc(photo.track_id)+' · '+esc(photo.video||"Видео")+
        ' · кадр '+esc(photo.frame??"—")+'</figcaption>'+
        '<div class="player-album-actions"><button type="button" class="secondary small" data-remove-album-photo="'+
        esc(photo.filename)+'">Удалить фото</button></div></figure>';
    }).join(""):'<div class="empty">В общей базе у этого игрока пока нет фото. После подтверждения Track ID образцы появятся здесь.</div>';
  }catch(error){
    if(token===state.albumToken)gallery.innerHTML='<div class="empty">'+esc(error.message)+'</div>';
  }
}
// The second click confirms deletion within the same photo card; no browser modal.
function cancelPlayerAlbumDelete(button){
  if(!button||button.disabled)return;
  delete button.dataset.confirmDelete;
  button.textContent="Удалить фото";
  button.classList.remove("is-confirm-delete");
  const actions=button.closest(".player-album-actions");
  actions?.querySelector("[data-cancel-album-delete]")?.remove();
  actions?.classList.remove("is-confirming");
}
function requestPlayerAlbumDelete(button){
  if(button.disabled)return;
  if(button.dataset.confirmDelete==="true"){
    return removePlayerAlbumPhoto(button).catch(console.warn);
  }
  // Only one inline confirmation can be active at a time.
  $("#player-album-gallery").querySelectorAll("[data-remove-album-photo]").forEach(other=>{
    if(other!==button)cancelPlayerAlbumDelete(other);
  });
  button.dataset.confirmDelete="true";
  button.textContent="Да, удалить";
  button.classList.add("is-confirm-delete");
  const actions=button.closest(".player-album-actions");
  actions.classList.add("is-confirming");
  const cancel=document.createElement("button");
  cancel.type="button";
  cancel.className="secondary small";
  cancel.dataset.cancelAlbumDelete="true";
  cancel.textContent="Отмена";
  button.after(cancel);
}
// Deletion updates only the affected album card and its roster counter.
// Rebuilding the roster/gallery here would destroy loaded images and jump the scroll.
async function removePlayerAlbumPhoto(button){
  if(button.disabled||!state.activePlayerId)return;
  const playerId=state.activePlayerId,projectId=state.project;
  const filename=button.dataset.removeAlbumPhoto;
  if(!filename||button.dataset.confirmDelete!=="true")return;
  button.disabled=true;
  const cancel=button.closest(".player-album-actions")?.querySelector("[data-cancel-album-delete]");
  if(cancel)cancel.disabled=true;
  button.textContent="Удаляем…";
  try{
    const result=await api("/api/players/photos/remove",{player_id:playerId,filename});
    state.photoCounts[playerId]=result.count;
    // References used by ReID have changed. Recalculate when the user next
    // opens a grouping mode, not while they are reviewing the album.
    clearTimeout(playerGroupsPoll);
    ++playerGroupsSequence;
    state.playerGroups=null;
    state.playerGroupsKey="";
    if(projectId!==state.project||playerId!==state.activePlayerId)return;

    const gallery=$("#player-album-gallery");
    const currentButton=Array.from(gallery.querySelectorAll("[data-remove-album-photo]"))
      .find(item=>item.dataset.removeAlbumPhoto===filename);
    const card=currentButton?.closest(".player-album-card");
    if(card){
      // Reserve the current height to avoid page-scroll anchoring and layout
      // jumps, even when deleting the last card at the bottom of the page.
      const height=gallery.offsetHeight;
      if(height>0)gallery.style.minHeight=
        Math.max(height,parseFloat(gallery.style.minHeight)||0)+"px";
      card.remove();
      if(!gallery.querySelector(".player-album-card")){
        gallery.innerHTML='<div class="empty">В базе этого игрока больше нет фотографий.</div>';
      }
    }
    const roster=Array.from($("#named-list").querySelectorAll("[data-player-id]"))
      .find(item=>item.dataset.playerId===playerId);
    const counter=roster?.querySelector(".player-slot-text small:last-child");
    if(counter)counter.textContent=Number(result.count||0)+" фото в личной базе";
    $("#players-action-feedback").textContent="Фотография удалена из общей базы.";
  }catch(error){
    button.disabled=false;
    if(cancel)cancel.disabled=false;
    cancelPlayerAlbumDelete(button);
    // Show an inline error: alertBox() would scroll the whole page to the top.
    $("#players-action-feedback").textContent="Не удалось удалить фотографию: "+error.message;
  }
}
// Serialize disk writes, but allow multiple cards to be linked immediately.
let playerAssignmentQueue=Promise.resolve();
let pendingPlayerAssignments=0;
// Minimal optimistic update: no report/photo fetch and no full DOM rerender.
function patchAssignedTrackData(trackIds,player){
  if(!state.report)return;
  const keys=new Set(trackIds.map(String));
  const groups=state.report.groups||(state.report.groups=[]);
  for(const group of groups)group.track_ids=group.track_ids.filter(id=>!keys.has(String(id)));
  const target=groups.find(group=>group.id===player.id);
  if(target){
    target.track_ids=[...new Set([...target.track_ids.map(String),...keys])]
      .sort((a,b)=>Number(a)-Number(b));
    target.name=player.name;
  }else groups.push({id:player.id,name:player.name,
    track_ids:[...keys].sort((a,b)=>Number(a)-Number(b))});
}
function undoOptimisticAssignedIds(trackIds){
  const keys=new Set(trackIds.map(String));
  if(!state.report?.groups)return;
  for(const group of state.report.groups){
    group.track_ids=group.track_ids.filter(id=>!keys.has(String(id)));
  }
  state.report.groups=state.report.groups.filter(group=>group.track_ids.length);
}
function updateAssignmentIndicators(playerId){
  const raw=state.report?.raw||[],assigned=associatedMap();
  const named=$("#named-list");
  const roster=Array.from(named?.querySelectorAll?.("[data-player-id]")||[])
    .find(el=>el.dataset.playerId===playerId);
  if(roster){
    const player=(state.report?.groups||[]).find(group=>group.id===playerId);
    const ids=player?.track_ids||[];
    const description=roster.querySelector?.(".player-slot-text small:first-child");
    if(description)description.textContent=ids.length
      ?ids.length+" ID на этом видео · "+ids.slice(0,5).map(x=>"#"+x).join(", ")+(ids.length>5?"…":"")
      :"Пока нет ID на этом видео";
    const label=roster.querySelector?.(".player-slot-actions");
    if(label)label.textContent=ids.length+" ID";
  }
  const stat=$("#players-health")?.querySelectorAll?.(".player-health-metric strong");
  const linked=raw.filter(row=>assigned.has(String(row.player_track_id))).length;
  if(stat?.length>=4){
    stat[2].textContent=String(linked);
    stat[3].textContent=String(raw.length-linked);
  }
  const visible=playerFilterSet(raw,assigned);
  $("#raw-count").textContent=visible.length+" из "+raw.length+" ID";
  $("#table-hint").textContent="Показано "+Math.min(visible.length,state.limit)+
    " из "+visible.length+" · всего "+raw.length+" ID";
}
function takeAssignmentCard(card){
  if(!card?.parentNode)return ()=>{};
  const parent=card.parentNode,next=card.nextSibling;
  // Keep the list's height stable while photos and groups remain untouched.
  const currentHeight=parent.offsetHeight||0;
  if(currentHeight&&parent.style){
    const reserved=parseFloat(parent.style.minHeight)||0;
    parent.style.minHeight=Math.max(currentHeight,reserved)+"px";
  }
  card.remove();
  return ()=>{
    if(parent.isConnected===false)return;
    parent.insertBefore(card,next?.parentNode===parent?next:null);
  };
}
function patchManualTrackRows(trackIds,player){
  const keys=new Set(trackIds.map(String)), changed=[];
  for(const checkbox of $("#raw-players")?.querySelectorAll?.(".row-select")||[]){
    const id=String(checkbox.dataset.id);
    if(!keys.has(id))continue;
    const row=checkbox.closest("tr");
    if(!row)continue;
    if(state.playerFilter==="unassigned"){
      const parent=row.parentNode,next=row.nextSibling;
      row.remove();
      changed.push(()=>parent?.insertBefore?.(row,next?.parentNode===parent?next:null));
    }else{
      const cell=row.querySelector("td:nth-child(6)");
      const previous=cell?.innerHTML;
      if(cell)cell.innerHTML='<span class="tag">'+esc(player.name)+'</span>';
      row.classList.add("is-assigned");
      changed.push(()=>{
        if(cell)cell.innerHTML=previous;
        row.classList.remove("is-assigned");
      });
    }
  }
  return ()=>changed.reverse().forEach(fn=>fn());
}
async function assignSpecificTracks(ids,playerId,options={}){
  if(state.playerAssignBusy)return;
  const player=state.players.find(p=>p.id===playerId);
  if(!player){
    $("#players-action-feedback").textContent="Выберите игрока, добавленного в проект.";
    return;
  }
  const already=associatedMap();
  const free=[...new Set(ids.map(String))].filter(id=>!already.has(id));
  if(!free.length){
    $("#players-action-feedback").textContent="Эти ID уже привязаны к игрокам.";
    return;
  }
  const projectId=state.project,video=state.video,cacheId=analysisCacheId();
  const report=state.report;
  const isCurrent=()=>projectId===state.project&&video===state.video&&
    cacheId===analysisCacheId()&&report===state.report;
  ++pendingPlayerAssignments;
  ++state.reportEpoch; // Invalidate a report read started before the optimistic edit.
  // Invalidate in-flight ReID updates that could redraw the list mid-save.
  clearTimeout(playerGroupsPoll);
  ++playerGroupsSequence;
  patchAssignedTrackData(free,player);
  const restoreCard=takeAssignmentCard(options.card);
  const restoreRows=patchManualTrackRows(free,player);
  for(const id of free)state.selected.delete(id);
  updateAssignmentIndicators(player.id);
  updateChosen();
  $("#players-action-feedback").textContent="Сохраняем "+free.length+
    " Track ID для «"+player.name+"»…";
  if(state.playerMode==="auto")$("#player-groups-status").textContent=
    "Сохраняем привязку "+free.length+" ID к «"+player.name+"»…";
  let saved=0;
  const persist=async()=>{
    for(let i=0;i<free.length;i+=200){
      const batch=free.slice(i,i+200);
      await api("/api/assign",{project_id:projectId,video,cache_id:cacheId,
        ids:batch,player_id:player.id});
      saved+=batch.length;
    }
  };
  // Each save gets a queue slot; the next card is still interactive.
  const request=playerAssignmentQueue.then(persist,persist);
  playerAssignmentQueue=request.catch(()=>{});
  try{
    await request;
    if(!isCurrent())return;
    state.reportDirty=true;
    $("#players-action-feedback").textContent="Сохранено: "+saved+
      " Track ID привязано к «"+player.name+"».";
    if(state.playerMode==="auto")$("#player-groups-status").textContent=
      "Сохранено: "+saved+" Track ID для «"+player.name+"». Остальные доступны для привязки.";
  }catch(error){
    if(!isCurrent())return;
    // Restore only the unsaved IDs in a partially completed batch.
    undoOptimisticAssignedIds(free.slice(saved));
    if(saved)state.reportDirty=true;
    if(saved){
      // A multi-batch partial failure is rare: redraw just to expose the
      // unsaved portion while keeping unrelated successful links intact.
      renderPlayers();
    }else{
      restoreCard();
      restoreRows();
      for(const id of free)state.selected.add(id);
      updateAssignmentIndicators(player.id);
      updateChosen();
    }
    $("#players-action-feedback").textContent="Не удалось сохранить привязку: "+
      error.message+(saved?" · сохранено "+saved+" ID":"");
    if(state.playerMode==="auto")$("#player-groups-status").textContent=
      "Ошибка сохранения: "+error.message+". Непривязанные ID возвращены в список.";
  }finally{
    --pendingPlayerAssignments;
    if(isCurrent()){
      updateChosen();
      if(pendingPlayerAssignments>0&&!$("#players-action-feedback").textContent.startsWith("Не удалось"))
        $("#players-action-feedback").textContent="Сохраняем оставшиеся привязки ("+
          pendingPlayerAssignments+")…";
    }
  }
}
async function loadPlayerSuggestions(){
  const target=$("#player-suggestions"),id=state.previewId,video=state.video;
  const request=++state.suggestionsToken;
  if(!id||!state.project||!video){
    target.innerHTML='<div class="empty">Выберите ID, чтобы проверить продолжения</div>';
    return;
  }
  target.innerHTML='<div class="muted">Поиск возможных продолжений по сохранённым рамкам…</div>';
  try{
    const params=new URLSearchParams({video,project_id:state.project,id,cache_id:analysisCacheId()});
    const response=await api("/api/player-suggestions?"+params.toString());
    if(request!==state.suggestionsToken||state.video!==video)return;
    const candidates=response.candidates||[];
    target.innerHTML=candidates.length?candidates.map(c=>
      '<div class="player-suggestion"><img loading="lazy" tabindex="0" data-photo-track-id="'+esc(c.track_id)+'" src="'+esc(playerPhotoUrl(c.track_id))+
      '" alt="Предполагаемое продолжение #'+esc(c.track_id)+'" onerror="this.style.opacity=0.15">'+
      '<div><strong>Track #'+esc(c.track_id)+'</strong>'+
      '<small>'+c.direction+' · разрыв '+c.gap_sec+' с · '+c.duration_sec+' с трек</small>'+
      '<small>Близость '+Math.round(c.score*100)+'% · не уверенность личности</small></div>'+
      '<button type="button" class="secondary small" data-compare-id="'+esc(c.track_id)+'">Сравнить</button></div>'
    ).join(""):'<div class="empty">Близкие продолжения не найдены. Это не означает, что их не было.</div>';
  }catch(error){
    if(request!==state.suggestionsToken)return;
    target.innerHTML='<div class="empty">'+esc(error.message)+'</div>';
  }
}
function renderRenders() {
  const target=$("#all-renders");
  const runId=state.analysisRunId;
  const all=[...(state.renders||[])].sort((a,b)=>
    Number(b.sort_time||b.modified||0)-Number(a.sort_time||a.modified||0));
  const items=all.filter(f=>runId&&f.analysis_run_id===runId);
  const older=all.filter(f=>!f.analysis_run_id);
  const outputRow=f=>'<div class="montage-output-row">'+
    '<div class="montage-output-info"><strong>'+esc(f.title||f.name)+'</strong>'+
    '<small>'+esc(f.group)+' · '+esc(f.name)+' · '+esc(f.size_mb)+' МБ'+
    (f.playable?'':' · требуется совместимый просмотр')+'</small></div>'+
    '<div class="montage-output-actions">'+
      '<button class="secondary small render-file" type="button" data-id="'+esc(f.id)+'" data-file="'+esc(f.name)+'">Смотреть</button>'+
      '<a class="secondary small montage-download" href="'+esc(f.download_url)+'" download>Скачать</a>'+
      '<button class="secondary small montage-folder" type="button" data-id="'+esc(f.id)+'">Папка</button>'+
      '<button class="secondary small montage-delete" type="button" data-id="'+esc(f.id)+'" data-name="'+esc(f.name)+'">Удалить</button>'+
    '</div></div>';
  target.innerHTML=(items.length?items.map(outputRow).join(""):
    '<p class="empty">Для выбранного запуска анализа монтажей пока нет.</p>')+
    (older.length?'<details class="montage-legacy-library"><summary>Архив проекта · монтажи без привязки к запуску ('+
      older.length+')</summary>'+older.map(outputRow).join("")+'</details>':"");
}
const montageSettingsKeys=["rally-gap-sec","rally-min-sec","rally-context-sec"];
let montagePlanTimer=null, montagePlanToken=0, montagePlanReadyKey="";
const montageManualSelected=new Set();
let montageActiveRallyId=null;
let montageManualCandidates=[],montageManualContext="";
let montageCurrentPlan=null, montagePlanContext="";
function montageContextKey(params){
  const keys=["project_id","run_id","gap_sec","rally_min_sec","context_sec","selected_videos"];
  return JSON.stringify(keys.map(key=>params[key]??""));
}
function syncMontageSelectionMode(){
  const manual=$("#montage-selection-mode").value==="manual";
  $("#montage-auto-strategy").hidden=manual;
  $("#montage-auto-duration").hidden=manual;
  $$("[data-montage-auto-filter]").forEach(field=>{
    field.hidden=manual;
    field.querySelectorAll("input,select").forEach(input=>input.disabled=manual);
  });
  $("#montage-manual-hint").hidden=!manual;
  $("#montage-plan-heading").textContent=manual?"Выберите розыгрыши для монтажа":"Найденные розыгрыши · автоматический отбор";
  $("#montage-manual-actions").hidden=!manual;
  $("#montage-duration-hint").textContent=manual
    ?"Итоговая длительность равна сумме выбранных полных розыгрышей."
    :"При автоматическом отборе итоговое видео не превышает заданную длительность. Полные розыгрыши не обрезаются.";
  $("#montage-duration").disabled=manual;
}
function currentRallySettings(){
  const defaults=[3,3,1.5], max=[12,40,10];
  let values=null;
  try{values=JSON.parse(localStorage.getItem("volley-rally-settings:"+state.project)||"null")}catch{}
  const numbers=montageSettingsKeys.map((key,i)=>{
    const value=values?.[key];
    return typeof value==="number"&&Number.isFinite(value)&&value>=0&&value<=max[i]
      ?value:defaults[i];
  });
  return {gap_sec:numbers[0],rally_min_sec:numbers[1],context_sec:numbers[2]};
}
function montagePayload(){
  const f=$("#highlights-form"),params=Object.fromEntries(new FormData(f).entries());
  f.querySelectorAll("[data-advanced-check]").forEach(el=>params[el.name]=el.checked);
  params.follow_ball=$("#montage-follow-ball").checked;
  params.rally_selection_mode=$("#montage-selection-mode").value;
  params.selected_rally_ids=params.rally_selection_mode==="manual"
    ?Array.from(montageManualSelected).sort():[];
  const project=selectedProject();
  // Source marks belong to the project + analysis run, in both modes.
  // Automatic export ignores this list, but keeping it stabilizes manual
  // selections when toggling the mode.
  // All analyzed videos are available; only checked rally IDs enter manual montage.
  params.selected_videos=project?.videos||[];
  // Manual selection ignores hidden automatic ranking and duration controls.
  if(params.rally_selection_mode==="manual"){
    params.duration="120";
    params.selection_strategy="highlights";
  }
  return {...params,project_id:state.project,run_id:selectedAnalysisRun()?.id||"",
    ...currentRallySettings()};
}
let activeMontagePickerId=null;
function cancelActiveMontagePicker(){
  const id=activeMontagePickerId;
  activeMontagePickerId=null;
  if(id)api("/api/local-videos/picker/cancel",{picker_id:id}).catch(()=>{});
}
async function chooseMontageAudio(){
  const button=$("#montage-audio-choose");
  const status=$("#montage-audio-status");
  button.disabled=true;
  status.textContent="Открываем проводник для выбора музыки...";
  try{
    const started=await api("/api/local-videos/picker/start",{kind:"audio"});
    activeMontagePickerId=started.picker_id;
    for(let attempt=0;attempt<280;attempt++){
      await new Promise(resolve=>setTimeout(resolve,650));
      const reply=await api("/api/local-videos/picker/status?picker_id="+
        encodeURIComponent(started.picker_id));
      if(reply.status==="pending")continue;
      if(reply.status==="error")throw new Error(reply.error||"Проводник недоступен");
      if(reply.paths?.length){
        $("#montage-audio-path").value=reply.paths[0];
        status.textContent="Музыка: "+reply.paths[0].split(/[\\/]/).pop()+
          ". Оригинальный файл останется на месте.";
        scheduleMontagePlan();
      }else{
        status.textContent="Выбор музыки отменён.";
      }
      return;
    }
    throw new Error("Время выбора музыки истекло");
  }catch(error){
    status.textContent="Ошибка выбора аудио: "+error.message;
  }finally{
    cancelActiveMontagePicker();
    button.disabled=false;
  }
}
function montageKey(params){
  return JSON.stringify(Object.entries(params).sort(([a],[b])=>a.localeCompare(b)));
}
function montageTime(sec){
  const t=Math.max(0,Math.floor(Number(sec)||0));
  return String(Math.floor(t/60)).padStart(2,"0")+":"+String(t%60).padStart(2,"0");
}
function renderMontageSourceSelection(params){
  const manual=params.rally_selection_mode==="manual";
  const chosen=manual?params.selected_videos:(selectedProject()?.videos||[]);
  $("#montage-source-selection").innerHTML=
    '<div class="montage-source-heading"><strong>'+ (manual
      ?'Для ручного монтажа отмечайте розыгрыши · видео проекта: '
      :'Видео для автоматического монтажа: ')+chosen.length+'</strong>'+
    '<button class="secondary small" type="button" data-montage-go-preview>Открыть розыгрыши в предпросмотре</button></div>'+
    (chosen.length?chosen.map(video=>'<span class="montage-source-chip">'+esc(video)+'</span>').join("")
      :'<span class="muted">Добавьте видео в проект и выполните анализ, чтобы увидеть розыгрыши.</span>');
}
function clearMontagePlan(message){
  montagePlanReadyKey="";
  montageCurrentPlan=null;
  montagePlanContext="";
  montageManualCandidates=[];
  montageActiveRallyId=null;
  $("#montage-create").disabled=true;
  $("#montage-plan-summary").textContent=message;
  $("#montage-plan-list").replaceChildren();
}
function montageChosenRows(params){
  if(!montageCurrentPlan)return [];
  return params.rally_selection_mode==="manual"
    ?montageManualCandidates.filter(row=>montageManualSelected.has(row.rally_id))
    :(montageCurrentPlan.selected||[]);
}
function updateMontageSelectionSummary(params=montagePayload()){
  const manual=params.rally_selection_mode==="manual";
  const selected=montageChosenRows(params);
  const total=selected.reduce((sum,row)=>sum+Number(row.duration||0),0);
  const available=montageManualCandidates.length;
  const criterion=$("#montage-selection-strategy").selectedOptions[0]?.textContent||"";
  $("#montage-plan-summary").textContent=manual
    ?"Отмечено "+selected.length+" из "+available+
      " розыгрышей · итоговая длительность "+total.toFixed(2)+" с."
    :"Найдено "+available+" розыгрышей · автоматически отобрано "+selected.length+
      " · "+total.toFixed(2)+" с из "+Number(params.duration).toFixed(0)+
      " с · "+criterion+"."+
      (montageCurrentPlan?.selection_error?" "+montageCurrentPlan.selection_error:"");
  $("#montage-create").disabled=!selected.length;
  if(montageCurrentPlan)montagePlanReadyKey=montageKey(params);
}
function renderMontageRallyCards(params){
  const manual=params.rally_selection_mode==="manual";
  const autoSelected=new Set((montageCurrentPlan?.selected||[]).map(row=>row.rally_id));
  const list=$("#montage-plan-list");
  const oldScroll=list.scrollLeft;
  list.innerHTML=montageManualCandidates.map((item,i)=>{
    const selected=manual?montageManualSelected.has(item.rally_id):autoSelected.has(item.rally_id);
    const status=manual?(selected?"Выбран для монтажа":"Отметьте галочкой для монтажа")
      :(selected?"Включён автоматически":"Не вошёл в монтаж");
    const controls=manual
      ?'<input class="montage-plan-checkbox" type="checkbox" data-montage-rally="'+
        esc(item.rally_id)+'" '+(selected?"checked":"")+
        ' aria-label="Розыгрыш '+(i+1)+' · добавить в монтаж">'
      :"";
    return '<div class="montage-plan-item'+(manual?" is-editable":" is-readonly")+
      (selected?" is-selected":"")+
      (item.rally_id===montageActiveRallyId?" is-previewing":"")+
      '" role="listitem" data-montage-preview="'+esc(item.rally_id)+
      '" tabindex="0" aria-label="Воспроизвести розыгрыш №'+(i+1)+
      '" title="Нажмите для просмотра розыгрыша">'+
      '<div class="montage-plan-card-head">'+controls+
      '<strong>Розыгрыш №'+(i+1)+' ▶</strong></div>'+
      '<b class="montage-plan-video" title="'+esc(item.video)+'">'+esc(item.video)+'</b>'+
      '<span class="montage-plan-time">'+montageTime(item.start)+'–'+montageTime(item.end)+
        ' · '+Number(item.duration).toFixed(1)+' с</span>'+
      '<div class="montage-plan-stats">'+
        '<span>Касаний '+Number(item.actions_total||0)+'</span>'+
        '<span>Атак '+Number(item.action_counts?.attack||0)+'</span>'+
        '<span>Блоков '+Number(item.action_counts?.block||0)+'</span>'+
        '<span>Игроков '+(item.player_count==null?'—':Number(item.player_count))+'</span>'+
        '<span>Уверенность '+Math.round(Number(item.confidence||0)*100)+'%</span>'+
      '</div><span class="montage-plan-state">'+status+'</span></div>';
  }).join("") || '<p class="empty">Розыгрыши не найдены. Проверьте настройки границ во вкладке «Предпросмотр».</p>';
  list.scrollLeft=oldScroll;
  updateMontageSelectionSummary(params);
}
function updateManualRallies(){
  if(!montageCurrentPlan)return scheduleMontagePlan();
  const list=$("#montage-plan-list");
  for(const input of list.querySelectorAll("[data-montage-rally]")){
    const selected=montageManualSelected.has(input.dataset.montageRally);
    input.checked=selected;
    const card=input.closest(".montage-plan-item");
    if(!card)continue;
    card.classList.toggle("is-selected",selected);
    const status=card.querySelector(".montage-plan-state");
    if(status)status.textContent=selected?"Выбран для монтажа":"Отметьте галочкой для монтажа";
  }
  window.VolleyballResultsUI?.setMontageRallies?.(
    [...montageManualSelected],state.project,state.analysisRunId);
  updateMontageSelectionSummary();
}
async function refreshMontagePlan(){
  const token=++montagePlanToken;
  let params=montagePayload();
  const context=montageContextKey(params);
  if(context!==montageManualContext){
    montageManualSelected.clear();
    for(const id of window.VolleyballResultsUI?.montageRallies?.(
      state.project,state.analysisRunId)||[])montageManualSelected.add(id);
    montageManualContext=context;
    params=montagePayload();
  }
  renderMontageSourceSelection(params);
  const run=selectedAnalysisRun();
  $("#montage-context-display").value=String(params.context_sec);
  $("#montage-source-info").textContent=run
    ?"Профиль: "+run.profile+" · запуск "+run.id.slice(-6)+" · объединение "+params.gap_sec+
      " с · минимальная длительность "+params.rally_min_sec+
      " с · контекст "+params.context_sec+" с"
    :"Результаты профиля не выбраны.";
  if(!state.project||!run){
    clearMontagePlan("Выберите проект и завершённый профиль анализа.");
    return false;
  }
  if(!$("#highlights-form").checkValidity()){
    clearMontagePlan("Проверьте длительность и фильтры монтажа.");
    return false;
  }
  const key=montageKey(params);
  if(montagePlanContext!==context){
    clearMontagePlan("Загружаем найденные розыгрыши выбранного анализа…");
  }else{
    montagePlanReadyKey="";
    $("#montage-create").disabled=true;
    $("#montage-plan-summary").textContent="Обновляем автоматический отбор и найденные розыгрыши…";
  }
  try{
    const plan=await api("/api/montage/plan",params);
    if(token!==montagePlanToken||key!==montageKey(montagePayload()))return false;
    montageCurrentPlan=plan;
    montageManualCandidates=plan.candidates||plan.selected||[];
    // Changed boundaries invalidate old rally IDs. Never send stale IDs to export.
    const validIds=new Set(montageManualCandidates.map(row=>row.rally_id));
    if([...montageManualSelected].some(id=>!validIds.has(id))){
      for(const id of [...montageManualSelected])
        if(!validIds.has(id))montageManualSelected.delete(id);
      window.VolleyballResultsUI?.setMontageRallies?.(
        [...montageManualSelected],state.project,state.analysisRunId);
    }
    montagePlanContext=context;
    renderMontageSourceSelection(params);
    renderMontageRallyCards(params);
    return true;
  }catch(error){
    if(token!==montagePlanToken)return false;
    clearMontagePlan("Не удалось загрузить розыгрыши: "+error.message);
    return false;
  }
}
function scheduleMontagePlan(){
  clearTimeout(montagePlanTimer);
  if(!document.getElementById("montage-plan-summary"))return;
  $("#montage-context-display").value=String(currentRallySettings().context_sec);
  montagePlanReadyKey="";
  $("#montage-create").disabled=true;
  montagePlanTimer=setTimeout(()=>refreshMontagePlan().catch(error=>
    clearMontagePlan(error.message)),250);
}
function syncMontageMode(){
  const follow=$("#montage-follow-ball").checked;
  $("#montage-mode-note").textContent=follow
    ?"Вертикальный рилс 9:16: камера удерживает мяч в кадре."
    :"Видео с исходным кадрированием: без движения виртуальной камеры.";
  const project=selectedProject();
  $("#highlights-project-hint").textContent=
    "Розыгрыши из "+(project?.videos.length||0)+
    " видео проекта · один общий отбор для обоих вариантов кадрирования.";
  $$("[data-montage-tracks]").forEach(el=>el.hidden=follow);
  $$("[data-montage-vertical]").forEach(el=>el.hidden=!follow);
}
let montagePreviewToken=0;
function previewMontageRally(rallyId){
  const item=montageManualCandidates.find(row=>row.rally_id===rallyId);
  const project=selectedProject();
  const run=selectedAnalysisRun();
  if(!item||!project||!run?.videos?.[item.video]||
      !project.videos.includes(item.video))return;
  const start=Number(item.start),end=Number(item.end);
  if(!Number.isFinite(start)||!Number.isFinite(end)||start<0||end<=start){
    $("#montage-preview-status").textContent="Неправильные границы выбранного розыгрыша.";
    return;
  }
  // Cancel any asynchronous H.264-library preview that was still loading.
  montagePreviewToken++;
  const video=$("#preview-player");
  const mediaUrl="/api/video-media?"+
    new URLSearchParams({project_id:project.id,video:item.video}).toString();
  video.pause();
  const sameMedia=video.getAttribute("src")===mediaUrl;
  if(!sameMedia){
    video.src=mediaUrl;
    video.load();
  }
  video.style.display="block";
  $("#preview-empty").style.display="none";
  $("#preview-title").textContent="Розыгрыш №"+
    (montageManualCandidates.indexOf(item)+1)+" · "+item.video+
    " · "+montageTime(start)+"–"+montageTime(end);
  $("#montage-preview-status").textContent="Воспроизводится выбранный розыгрыш. Пробел — пауза / продолжить.";
  montageActiveRallyId=item.rally_id;
  $("#montage-plan-list").querySelectorAll("[data-montage-preview]").forEach(card=>{
    card.classList.toggle("is-previewing",card.dataset.montagePreview===rallyId);
  });
  window.VolleyballMontagePlayer?.playRally(start,end);
}
async function previewMontageFile(item){
  const token=++montagePreviewToken;
  montageActiveRallyId=null;
  $("#montage-plan-list").querySelectorAll(".is-previewing").forEach(card=>
    card.classList.remove("is-previewing"));
  window.VolleyballMontagePlayer?.setRallyRange();
  const video=$("#preview-player");
  const status=$("#montage-preview-status");
  video.pause();video.removeAttribute("src");video.load();
  video.style.display="none";
  $("#preview-empty").style.display="block";
  $("#preview-title").textContent=item.title||item.name;
  status.textContent="Проверяем совместимость видео…";
  const query="?project_id="+encodeURIComponent(state.project)+"&id="+encodeURIComponent(item.id);
  try {
    for(let tries=0;tries<240;tries++){
      if(token!==montagePreviewToken)return;
      const response=await api("/api/montage/preview"+query);
      if(response.status==="failed"){
        status.textContent=response.error||"Не удалось подготовить видео для просмотра.";
        return;
      }
      if(response.status==="ready"){
        if(token!==montagePreviewToken)return;
        video.src=response.url;
        video.style.display="block";
        $("#preview-empty").style.display="none";
        status.textContent=item.playable?"Готово к воспроизведению.":"Для просмотра используется H.264; скачать можно оригинал.";
        video.load();
        return;
      }
      status.textContent="Создаём совместимую H.264-версию для браузера. Оригинал не меняется…";
      await new Promise(resolve=>setTimeout(resolve,2500));
    }
    status.textContent="Подготовка занимает слишком долго. Оригинал можно скачать из библиотеки.";
  }catch(error){status.textContent="Не удалось открыть видео: "+error.message;}
}

function selectedProject() {
  return state.projects.find(p=>p.id===state.project)||null;
}
function todayProjectName(date=new Date()) {
  return [date.getDate(),date.getMonth()+1].map(v=>String(v).padStart(2,"0")).join(".")+"."+date.getFullYear();
}
function clock(s) {
  s=Math.max(0,Math.round(Number(s)||0));
  return [Math.floor(s/3600),Math.floor(s%3600/60),s%60].map(v=>String(v).padStart(2,"0")).join(":");
}
function projectRange(name) {
  const p=selectedProject();
  return p?.ranges?.[name]||{start:0,end:0};
}
function populatePlayersSelect() {
  const select=$("#players-video"),p=selectedProject();
  if(!p)return;
  select.innerHTML=p.videos.map(name=>'<option value="'+esc(name)+'">'+esc(name)+'</option>').join("");
  if(p.videos.includes(state.video))select.value=state.video;
}
function selectedAnalysisRun(){
  // The currently selected CPU version must belong to this video and project.
  // A run from another video can never become the data source for its preview.
  return state.analysisRuns.find(run=>run.id===state.analysisRunId&&
    Object.prototype.hasOwnProperty.call(run.videos||{},state.video))||null;
}
function renderHeaderChain(){
  const model=window.VolleyballHeaderChain;
  if(!model)return;
  const fill=(id,options,chosen,empty)=>{
    const control=$("#"+id);
    control.replaceChildren();
    if(!options.length){
      control.add(new Option(empty,""));
      control.disabled=true;
    }else{
      for(const row of options){
        const option=new Option(row.label,row.id);
        if(row.title)option.title=row.title;
        option.cpuVersion=Boolean(row.isCpuVersion);
        control.add(option);
      }
      control.disabled=false;
      control.value=options.some(row=>row.id===chosen)?chosen:options[0].id;
    }
  };
  const project=selectedProject();
  fill("chain-project",state.projects.map(p=>({
    id:p.id,label:p.name+(state.projects.filter(other=>
      other.name.toLocaleLowerCase()===p.name.toLocaleLowerCase()).length>1
      ?" · "+p.id.slice(0,6):""),
    title:p.name+" · "+p.id
  })),state.project,"Нет проектов");
  const chosen=model.selected(state.analysisRuns,state.video,state.analysisRunId);
  const projectVideos=new Set(project?.videos||[]);
  const originals=state.analysisRuns.filter(run=>!run.parent_run_id)
    .flatMap(run=>Object.keys(run.videos||{})
      .filter(video=>projectVideos.has(video))
      .map(video=>({run,video})));
  fill("chain-inference",originals.map(({run,video})=>({
    id:model.choiceId(video,run.id),
    label:video+" · "+model.initialLabel(run,video),
    title:video+" · "+run.profile+" · "+model.rangeText(run.video_ranges?.[video])+
      " · "+run.started_at+" · "+run.id
  })),chosen.parent?model.choiceId(state.video,chosen.parent.id):"",
  "Нет первичного анализа — запустите его");
  const versions=chosen.parent
    ?model.branches(state.analysisRuns,state.video,chosen.parent.id):[];
  fill("chain-postprocess",versions.map(run=>({
    id:run.id,label:model.versionLabel(run),
    isCpuVersion:Boolean(run.parent_run_id),
    title:run.postprocess_settings
      ?JSON.stringify(run.postprocess_settings,null,2)
      :"Исходный результат анализа · "+run.id
  })),chosen.version?.id,chosen.parent?"Нет обработок":"Сначала запустите нейросети");
  window.VolleyballHeaderMenu?.render();
}
function preferredContextRun({newest=false}={}){
  const model=window.VolleyballHeaderChain;
  if(!model||!state.project||!state.video)return "";
  const key=contextIdentity();
  const originals=model.originalRuns(state.analysisRuns,state.video);
  let saved="";
  try{saved=localStorage.getItem(headerRunKey())||""}catch{}
  const current=activeHeaderContext===key&&!newest?state.analysisRunId:"";
  const preferred=newest?originals[0]?.id:
    current||saved||originals[0]?.id;
  activeHeaderContext=key;
  const choice=model.selected(state.analysisRuns,state.video,preferred);
  if(choice.parent&&choice.version?.id===choice.parent.id&&(newest||(!current&&!saved))){
    const recommended=model.branches(state.analysisRuns,state.video,choice.parent.id)
      .find(run=>run.parent_run_id&&
        run.postprocess_settings?.mode==="combined"&&
        Number(run.postprocess_settings?.player_threshold)===.30&&
        Number(run.postprocess_settings?.touch_threshold)===.45);
    if(recommended)return recommended.id;
  }
  return choice.version?.id||"";
}
function analysisCacheMap(){
  return selectedAnalysisRun()?.videos||{};
}
function analysisCacheId(name=state.video){
  return analysisCacheMap()[name]||"";
}
function updateReprocessControls(){
  const selected=selectedAnalysisRun();
  const button=document.getElementById("reprocess-start");
  if(button)button.disabled=!state.project||!selected||
    state.latestTask?.status==="running";
  const status=document.getElementById("reprocess-status");
  if(status&&!selected)status.textContent="Сначала выполните анализ проекта.";
  if(status&&selected&&state.latestTask?.kind!=="reprocess")
    status.textContent="Настройки пересчёта в «Предпросмотре», ход выполнения — в левой панели.";
}
function renderAnalysisRunChoices(){
  // The dependency chain in the header is the only run selector.
  renderHeaderChain();
  updateReprocessControls();
}
async function loadAnalysisRuns(preferNewest=false){
  const request=++analysisRunsRequest;
  if(!state.project){
    state.analysisRuns=[];state.analysisRunId="";renderAnalysisRunChoices();return true;
  }
  const project=state.project;
  const answer=await api("/api/analysis-runs?project_id="+encodeURIComponent(project));
  // Runs cover every video of this project. A video switch during the HTTP
  // request must still populate the list, using the CURRENT video for selection.
  // Only a superseded request or a different project invalidates the reply.
  if(request!==analysisRunsRequest||project!==state.project)
    return false;
  state.analysisRuns=answer.runs||[];
  const latest=preferNewest||state.analysisRunId==="__new__";
  state.analysisRunId=preferredContextRun({newest:latest});
  try{
    if(state.analysisRunId){
      localStorage.setItem("volley-analysis-run:"+project,state.analysisRunId);
      localStorage.setItem(headerRunKey(),state.analysisRunId);
    }
  }catch{}
  renderAnalysisRunChoices();
  renderRenders();
  scheduleMontagePlan();
  return true;
}
async function chooseAnalysisRun(id){
  if(id===state.analysisRunId)return;
  const finish=beginWorkspaceLoading("Загрузка выбранного анализа…");
  try{return await chooseAnalysisRunCore(id);}
  finally{finish();}
}
async function chooseAnalysisRunCore(id){
  if(id===state.analysisRunId)return;
  if(!state.analysisRuns.some(run=>run.id===id&&
    Object.prototype.hasOwnProperty.call(run.videos||{},state.video)))return;
  const projectId=state.project;
  state.analysisRunId=id;
  try{
    localStorage.setItem("volley-analysis-run:"+projectId,id);
    localStorage.setItem(headerRunKey(projectId,state.video),id);
    localStorage.setItem("volley-analysis-profile:"+projectId,selectedAnalysisRun().profile);
  }catch{}
  renderAnalysisRunChoices();
  renderRenders();
  scheduleMontagePlan();
  await loadReport();
  if(projectId!==state.project||id!==state.analysisRunId)return;
  const p=selectedProject();
  if(p&&$("#view-analysis-results").classList.contains("active")){
    window.VolleyballResultsUI.setCacheSelection(analysisCacheMap(),state.analysisRunId,
      selectedAnalysisRun()?.postprocess_settings||null);
    await window.VolleyballResultsUI.setProject(state.project,p.videos,state.video);
  }
}
let preparationTimer=null;
async function refreshVideoPreparation(){
  const panel=$("#video-preparation");
  const pid=state.project;
  panel.hidden=!pid;
  if(!pid)state.preparationActive=false;
  if(!pid||state.preparationPending)return;
  clearTimeout(preparationTimer);
  state.preparationPending=true;
  try{
    const result=await api("/api/project-preparation?project_id="+encodeURIComponent(pid));
    if(state.project!==pid)return;
    const wasReady=panel.dataset.state==="ready"&&state.preparationProject===pid;
    state.preparationProject=pid;
    panel.classList.remove("has-request-error");
    const rows=result.videos||[];
    const ready=rows.filter(item=>item.state==="ready").length;
    const failed=rows.filter(item=>item.state==="failed").length;
    const overall=rows.length
      ?Math.round(rows.reduce((sum,row)=>sum+
        (row.state==="ready"?100:row.state==="preparing"?
          Math.max(0,Math.min(100,Number(row.progress)||0)):0),0)/rows.length)
      :0;
    const active=rows.some(item=>["queued","preparing","missing"].includes(item.state));
    state.preparationActive=active;
    panel.dataset.state=failed?"failed":active?"working":"ready";
    $("#video-preparation-count").textContent=
      failed?failed+" ошибок":active?overall+"%":ready+"/"+rows.length;
    $("#video-preparation-total-fill").style.width=overall+"%";
    $("#video-preparation-total-meter").setAttribute("aria-valuenow",String(overall));
    $("#video-preparation-summary").textContent=active
      ?"В очереди/работе: "+rows.filter(item=>item.state!=="ready").length
      :failed?"Требуется повтор":"Файлы";
    const detail=panel.querySelector(".sidebar-preparation-detail");
    if(detail&&!active&&!failed&&!wasReady)detail.open=false;
    $("#video-preparation-list").innerHTML=rows.map(item=>{
      const states={ready:"Готово",preparing:"Подготавливается",
        queued:"В очереди",failed:"Ошибка",missing:"Ожидает"};
      const percent=Math.max(0,Math.min(100,Number(item.progress)||0));
      return '<div class="video-preparation-row">'+
        '<div class="video-preparation-meta"><strong>'+esc(item.video)+'</strong>'+
        '<span>'+esc(states[item.state]||item.state)+
          (item.state==="preparing"?" · "+percent.toFixed(0)+"%":"")+
          (item.state==="ready"?" · "+esc(item.width)+" px · 30 FPS":"")+
        '</span></div>'+
        '<div class="video-preparation-meter" role="progressbar" aria-label="'+esc(item.video)+
        '" aria-valuemin="0" aria-valuemax="100" aria-valuenow="'+percent.toFixed(0)+'">'+
        '<div style="width:'+percent+'%"></div></div>'+
        (item.error?'<small class="video-preparation-error">'+esc(item.error)+'</small>':'')+
      '</div>';
    }).join("");
    $("#video-preparation-retry").hidden=!failed;
    window.dispatchEvent(new CustomEvent("volley-preparation-update",{
      detail:{project:pid,videos:rows}
    }));
  }catch(error){
    if(state.project===pid){
      state.preparationActive=false;
      panel.dataset.state="failed";
      panel.classList.add("has-request-error");
      $("#video-preparation-count").textContent="Ошибка";
      $("#video-preparation-summary").textContent="Подробности ошибки";
      $("#video-preparation-list").innerHTML=
        '<div class="video-preparation-row"><small class="video-preparation-error">'+
        esc(error.message)+'</small></div>';
      $("#video-preparation-retry").hidden=true;
      const detail=panel.querySelector(".sidebar-preparation-detail");
      if(detail)detail.open=true;
    }
  }finally{
    state.preparationPending=false;
    clearTimeout(preparationTimer);
    if(state.project&&state.project!==pid){
      // A project switch must not leave the next project's preparation panel
      // waiting forever for the old project's in-flight HTTP request.
      preparationTimer=setTimeout(()=>refreshVideoPreparation().catch(console.warn),0);
    }else if(state.preparationActive&&state.project===pid){
      preparationTimer=setTimeout(()=>refreshVideoPreparation().catch(console.warn),
        document.hidden?15000:3000);
    }
  }
}
function renderSources() {
  const p=selectedProject();
  window.VolleyballDiagnosticsUI?.setSources(state.project,p?.videos||[],state.video);
  const counts=new Map();
  for(const project of state.projects){
    const name=project.name.toLocaleLowerCase();
    counts.set(name,(counts.get(name)||0)+1);
  }
  $("#top-project").innerHTML='<option value="">Выберите проект</option>'+
    state.projects.map(p=>'<option value="'+esc(p.id)+'">'+esc(p.name)+
      ((counts.get(p.name.toLocaleLowerCase())||0)>1
       ?' · '+esc(p.id.slice(0,6)):'')+'</option>').join("");
  $("#top-project").value=state.project||"";
  renderHeaderChain();
  $("#no-project").hidden=!!p;
  $("#video-preparation").hidden=!p;
  $("#project-workspace").hidden=!p;
  for(const btn of $$(".nav-button"))btn.disabled=!p&&btn.dataset.view!=="analysis";
  if(!p) {
    $("#active-project-name").textContent="Анализ";
    $("#active-project-description").textContent="Создайте проект и добавьте видео. Настройки анализа доступны после создания.";
    return;
  }
  $("#active-project-name").textContent="Анализ";
  $("#active-project-description").textContent="Выберите видео справа, задайте диапазон под проигрывателем и параметры запуска ниже.";
  $("#project-source-list").innerHTML=p.videos.map((name,i)=>{
    const range=projectRange(name);
    const duration=(p.missing_videos||[]).includes(name)?"Источник недоступен":
      range.end>0?clock(range.start)+" — "+clock(range.end):"Весь файл";
    return '<button type="button" class="project-source '+(state.video===name?"selected":"")+
      '" data-source="'+esc(name)+'"><span class="source-index">ВИДЕО '+String(i+1).padStart(2,"0")+'</span>'+
      '<span class="source-name">'+esc(name)+'</span>'+
      '<span class="source-range">'+esc(duration)+'</span></button>';
  }).join("");
  $("#analysis-project-hint").textContent="Анализ всех "+p.videos.length+
      " видео проекта. Параметры применяются ко всем файлам, временные диапазоны настраиваются отдельно."+
      (p.missing_videos?.length?" Недоступны исходные видео: "+p.missing_videos.join(", ")+
        ". Проверьте расположение исходных файлов.":"");
  $("#highlights-project-hint").textContent="Монтаж из "+p.videos.length+" видео проекта; полный выбранный розыгрыш не обрезается ради длительности.";
  populatePlayersSelect();
}
async function loadProjects(preferredId=null,epoch=projectLoadEpoch) {
  const response=await api("/api/projects");
  if(epoch!==projectLoadEpoch)return false;
  const projects=response.projects||[];
  if(preferredId&&!projects.some(p=>p.id===preferredId))
    throw new Error("Созданный или выбранный проект не найден в реестре: "+preferredId);
  state.projects=projects;
  let chosen=preferredId||state.project;
  if(!chosen){
    try{chosen=localStorage.getItem(SELECTED_PROJECT_KEY)||"";}catch{}
  }
  const priorProject=state.project;
  state.project=projects.some(p=>p.id===chosen)?chosen:projects[0]?.id||"";
  if(priorProject!==state.project){
    // Clear the old project hierarchy before showing the new one.
    state.analysisRuns=[];state.analysisRunId="";activeHeaderContext="";
  }
  const p=selectedProject();
  if(p){
    let storedVideo="";
    try{storedVideo=localStorage.getItem(headerVideoKey(p.id))||""}catch{}
    if(!p.videos.includes(state.video)||state.project!==activeHeaderContext.split("\u0000")[0]){
      state.video=p.videos.includes(storedVideo)?storedVideo:p.videos[0]||"";
    }
    try{if(state.video)localStorage.setItem(headerVideoKey(p.id),state.video)}catch{}
  }else state.video="";
  try{
    if(state.project)localStorage.setItem(SELECTED_PROJECT_KEY,state.project);
    else localStorage.removeItem(SELECTED_PROJECT_KEY);
  }catch{}
  renderSources();
  renderSidebarActivity();
  return true;
}
async function selectSource(name) {
  if(!selectedProject()?.videos.includes(name)||state.video===name)return;
  const finish=beginWorkspaceLoading("Переключение видео и загрузка данных…");
  try{return await selectSourceCore(name);}
  finally{finish();}
}
async function selectSourceCore(name) {
  if(!selectedProject()?.videos.includes(name)||state.video===name)return;
  const projectId=state.project;
  const selection=++videoSelectionGeneration;
  await window.VolleyballAutoSave.flush();
  if(selection!==videoSelectionGeneration||projectId!==state.project)return;
  state.video=name;
  try{localStorage.setItem(headerVideoKey(projectId),name)}catch{}
  state.analysisRunId=preferredContextRun();
  renderAnalysisRunChoices();
  renderSources();
  renderRenders();
  scheduleMontagePlan();
  populatePlayersSelect();
  const refresh=[loadReport(),window.VolleyballAnalysisUI.setVideo(name)];
  if($("#view-analysis-results").classList.contains("active")){
    window.VolleyballResultsUI.setCacheSelection(
      analysisCacheMap(),state.analysisRunId,
      selectedAnalysisRun()?.postprocess_settings||null);
    refresh.push(window.VolleyballResultsUI.setProject(
      projectId,selectedProject().videos,name));
  }
  await Promise.all(refresh);
}
let projectSourceGeneration=0;
let projectSourcePending=0;
let activeProjectPickerId=null;
function cancelActiveProjectPicker(){
  const id=activeProjectPickerId;
  activeProjectPickerId=null;
  if(id)api("/api/local-videos/picker/cancel",{picker_id:id}).catch(()=>{});
}
let projectImportedSources=new Set();

function renderDialogVideos(picked=new Set()){
  const target=$("#new-project-video-options"),videos=state.videos||[];
  target.hidden=videos.length===0;
  target.innerHTML=videos.length?videos.map(v=>
    '<div class="dialog-video-row" data-video="'+esc(v.name)+'">'+
    '<label class="dialog-video-heading"><input type="checkbox" class="dialog-source-check" value="'+esc(v.name)+'" '+
    (picked.has(v.name)?"checked":"")+'><span><strong>'+esc(v.name)+
    '</strong><small class="dialog-duration">'+esc(v.size_gb)+' ГБ'+
    (v.source_path?' · '+esc(v.source_path):'')+
    '</small></span></label></div>'
  ).join(""):"";
  updateDialogSelection();
}
window.addEventListener?.("pagehide",()=>{
  cancelActiveMontagePicker();
  cancelActiveProjectPicker();
});
function resetProjectUploadDialog(){
  cancelActiveProjectPicker();
  ++projectSourceGeneration;
  projectSourcePending=0;
  projectImportedSources=new Set();
  $("#project-source-feedback").textContent="";
  $("#new-project-video-options").hidden=true;
  $("#new-project-video-options").replaceChildren();
  $("#project-video-dropzone").disabled=false;
}
function localSourceFeedback(message,isError=false){
  const node=$("#project-source-feedback");
  node.textContent=message;
  node.style.color=isError?"#ffac9b":"";
}
function sourceOperationStart(){
  projectSourcePending++;
  updateDialogSelection();
}
function sourceOperationEnd(generation){
  if(generation!==projectSourceGeneration)return;
  projectSourcePending=Math.max(0,projectSourcePending-1);
  updateDialogSelection();
}
async function registerProjectLocalSources(paths,generation){
  if(generation!==projectSourceGeneration)return;
  const reply=await api("/api/local-videos/add",{paths});
  if(generation!==projectSourceGeneration)return;
  const picked=new Set($$(".dialog-source-check").filter(el=>el.checked).map(el=>el.value));
  for(const video of reply.videos||[]){
    picked.add(video.name);
    projectImportedSources.add(video.name);
    const previous=state.videos.findIndex(row=>row.name===video.name);
    if(previous<0)state.videos.push(video);
    else state.videos[previous]={...state.videos[previous],...video};
  }
  renderDialogVideos(picked);
  localSourceFeedback("Добавлено источников: "+(reply.videos||[]).length+
    ". Оригинальные файлы не копируются.");
}
const PROJECT_VIDEO_TYPES=new Set(["mp4","mov","mkv","avi","webm","m4v","mts"]);
async function acceptProjectFiles(fileList){
  if(state.dialogBusy||projectSourcePending)return;
  const selected=Array.from(fileList||[]);
  if(!selected.length)return;
  const generation=projectSourceGeneration;
  const existing=new Set($$(".dialog-source-check").filter(el=>el.checked).map(el=>el.value));
  if(selected.length+existing.size>30){
    localSourceFeedback("Максимум 30 видео для одного проекта.",true);return;
  }
  if(selected.some(file=>!PROJECT_VIDEO_TYPES.has((file.name.split(".").pop()||"").toLowerCase()))){
    localSourceFeedback("Выбран неподдерживаемый формат видео.",true);return;
  }
  sourceOperationStart();
  $("#project-video-dropzone").disabled=true;
  localSourceFeedback("Определяем расположение оригиналов на компьютере. Видео не загружаются…");
  try{
    const reply=await api("/api/project/resolve-files",{
      files:selected.map(file=>({name:file.name,size:file.size,
        last_modified:file.lastModified}))
    });
    if(generation!==projectSourceGeneration)return;
    const picked=new Set($$(".dialog-source-check").filter(el=>el.checked).map(el=>el.value));
    for(const video of reply.videos||[]){
      picked.add(video.name);
      projectImportedSources.add(video.name);
      const previous=state.videos.findIndex(row=>row.name===video.name);
      if(previous<0)state.videos.push(video);
      else state.videos[previous]={...state.videos[previous],...video};
    }
    renderDialogVideos(picked);
    localSourceFeedback("Добавлено видео: "+(reply.videos||[]).length+
      ". Оригиналы остаются на месте, без копирования.");
  }catch(error){
    if(generation===projectSourceGeneration)localSourceFeedback(error.message,true);
  }finally{
    if(generation===projectSourceGeneration)$("#project-video-dropzone").disabled=false;
    sourceOperationEnd(generation);
  }
}
async function chooseProjectSourcePaths(){
  if(state.dialogBusy||projectSourcePending)return;
  // The native browser dialog is attached to the visible desktop session,
  // unlike a PowerShell dialog launched inside a headless WSL server.
  if(typeof window.showOpenFilePicker==="function"){
    try{
      const handles=await window.showOpenFilePicker({multiple:true,startIn:"desktop"});
      return acceptProjectFiles(await Promise.all(handles.map(item=>item.getFile())));
    }catch(error){
      if(error?.name==="AbortError")return;
      if(!["TypeError","NotSupportedError","SecurityError"].includes(error?.name)){
        localSourceFeedback("Не удалось открыть выбор видео: "+error.message,true);
        return;
      }
    }
  }
  $("#project-video-file-input").click();
}
function updateDialogSelection() {
  const boxes=$$(".dialog-source-check");
  const picked=boxes.filter(b=>b.checked).length;
  $("#project-selected-count").textContent=projectSourcePending
    ?picked+" выбрано · выбирается "+projectSourcePending
    :picked+" видео выбрано";
  $("#confirm-project").disabled=projectSourcePending>0||picked===0||picked>30||state.dialogBusy;
  for(const cb of boxes) {
    const row=cb.closest(".dialog-video-row");
    row.classList.toggle("is-selected",cb.checked);
  }
}
async function openProjectDialog(edit=false) {
  if(state.dialogBusy)return;
  const requestId=++state.dialogRequestId;
  state.dialogEdit=edit;
  resetProjectUploadDialog();
  state.videos=[];
  const p=selectedProject(),dlg=$("#project-dialog");
  $("#project-dialog-error").hidden=true;
  $("#project-dialog-title").textContent=edit?"Видео и название проекта":"Создать проект";
  $("#confirm-project").textContent=edit?"Сохранить изменения":"Создать проект";
  const baseName=todayProjectName();
  let proposed=baseName, next=2;
  while(!edit&&state.projects.some(x=>x.name.toLocaleLowerCase()===proposed.toLocaleLowerCase()))
    proposed=baseName+" ("+(next++)+")";
  $("#new-project-name").value=edit&&p?p.name:proposed;
  $("#confirm-project").disabled=true;
  $("#project-selected-count").textContent="Выберите новые видео через проводник";
  renderDialogVideos(new Set());
  $("#new-project-player-options").innerHTML='<p class="muted">Загружаем базу игроков…</p>';
  dlg.showModal();
  $("#new-project-name").focus();
  $("#new-project-name").select();

  // Creation lists only files picked in this dialog, never the old project/ contents.
  // Keep the full existing-video selector only for the legacy edit flow.
  try {
    const [response, library, roster]=await Promise.all([
      edit?api("/api/videos"):Promise.resolve({videos:[]}),
      api("/api/players/library"),
      edit&&p?api("/api/players?project_id="+encodeURIComponent(p.id)):Promise.resolve({players:[]})
    ]);
    if(requestId!==state.dialogRequestId||!dlg.open)return;
    const uploaded=(state.videos||[]).filter(v=>projectImportedSources.has(v.name));
    state.videos=edit?[...(response.videos||[])]:[];
    for(const item of uploaded){
      if(!state.videos.some(v=>v.name===item.name))state.videos.push(item);
    }
    state.libraryPlayers=library.players||[];
    const selectedPlayers=new Set(roster.players.map(player=>player.id));
    $("#new-project-player-options").innerHTML=state.libraryPlayers.length?
      state.libraryPlayers.map(player=>
        '<label class="dialog-player-option"><input type="checkbox" class="dialog-player-check" value="'+esc(player.id)+'" '+
        (selectedPlayers.has(player.id)?'checked':'')+'><span class="dialog-player-avatar">'+
        esc((player.name[0]||"?").toUpperCase())+'</span><span>'+esc(player.name)+'</span></label>'
      ).join(""):'<p class="muted">В базе пока нет игроков. Создать первого можно во вкладке «Игроки» после создания проекта.</p>';
    const selected=new Set(edit&&p?p.videos:[]);
    for(const checkbox of $$(".dialog-source-check")){
      if(checkbox.checked)selected.add(checkbox.value);
    }
    renderDialogVideos(selected);
  } catch(error) {
    if(requestId!==state.dialogRequestId||!dlg.open)return;
    // An asynchronous picker may have completed while the library was loading.
    // Keep those manually selected videos even if the library request fails.
    renderDialogVideos(new Set($$(".dialog-source-check")
      .filter(input=>input.checked).map(input=>input.value)));
    $("#new-project-player-options").innerHTML="";
    $("#project-selected-count").textContent="Списки не загружены";
    $("#project-dialog-error").textContent=error.message;
    $("#project-dialog-error").hidden=false;
  }
}
function closeProjectDialog() {
  ++state.dialogRequestId;
  resetProjectUploadDialog();
  $("#project-dialog").close();
}
async function saveProjectDialog() {
  if(state.dialogBusy)return;
  const p=selectedProject(),name=$("#new-project-name").value.trim();
  const rows=$$(".dialog-video-row").filter(row=>row.querySelector(".dialog-source-check").checked);
  const error=$("#project-dialog-error");
  const fail=msg=>{error.textContent=msg;error.hidden=false;};
  if(!name)return fail("Введите название проекта");
  const identical=state.projects.find(x=>x.id!==(state.dialogEdit?p?.id:null)&&
    x.name.normalize("NFKC").trim().replace(/\\s+/g," ").toLocaleLowerCase()===
    name.normalize("NFKC").trim().replace(/\\s+/g," ").toLocaleLowerCase());
  if(identical)return fail("Проект «"+identical.name+"» уже существует. Введите другое название.");
  if(projectSourcePending)return fail("Дождитесь завершения загрузки видео");
  if(!rows.length)return fail("Выберите хотя бы одно видео");
  const videos=[];
  for(const row of rows){
    const filename=row.dataset.video;
    videos.push(filename);
  }
  state.dialogBusy=true;$("#confirm-project").disabled=true;error.hidden=true;
  try {
    const player_ids=$$(".dialog-player-check").filter(input=>input.checked).map(input=>input.value);
    const payload=state.dialogEdit?{id:p.id,name,videos,player_ids}:{name,videos,player_ids};
    const result=await api(state.dialogEdit?"/api/project-update":"/api/project-create",payload);
    const createdId=result.project?.id;
    if(!createdId)throw new Error("Сервер не вернул ID сохранённого проекта");
    closeProjectDialog();
    try{
      const applied=await loadAll(createdId);
      if(applied&&state.project===createdId){
        const detail=$("#video-preparation .sidebar-preparation-detail");
        if(detail&&state.preparationActive)detail.open=true;
        go("analysis");
      }
    }catch(error){
      // The project is already persisted; never show a misleading creation
      // failure or encourage a second POST for the same project.
      alertBox("Проект «"+name+"» создан, но не удалось открыть его данные: "+error.message);
    }
  }catch(err){fail(err.message)}
  finally{state.dialogBusy=false;$("#confirm-project").disabled=false}
}
async function loadAnalysisHistory(){
  const target=$("#analysis-timing-history");
  if(!state.project){
    target.textContent="Выберите проект, чтобы посмотреть историю запусков.";
    return;
  }
  const projectId=state.project;
  try{
    const reply=await api("/api/analysis-history?project_id="+encodeURIComponent(projectId));
    if(projectId!==state.project)return;
    // Filter legacy records even if a still-running UI server has not been
    // restarted yet. Backend uses the same stronger schema checks.
    const history=(reply.history||[]).filter(run=>
      run.status==="complete"&&
      typeof run.profile==="string"&&run.profile.trim()!==""&&
      run.profile!=="Профиль старого запуска не записан"&&
      Array.isArray(run.videos)&&run.videos.length>0&&
      run.videos.every(video=>video.status==="complete"&&
        Array.isArray(video.stages)&&video.stages.length>0&&
        video.stages.every(stage=>stage.stage_index>0&&
          typeof stage.stage==="string"&&stage.stage.trim()!==""&&
          stage.duration_sec!=null&&Number.isFinite(Number(stage.duration_sec)))));
    if(!history.length){
      target.innerHTML='<p class="analysis-runs-empty">Запусков анализа пока нет. После первого запуска появятся профиль, время старта, общее время и этапы.</p>';
      return;
    }
    const dateTime=value=>{
      if(!value)return "Не записано";
      const date=new Date(value);
      return Number.isNaN(date.getTime())?"Не записано":
        date.toLocaleString(window.VolleyI18n?.locale()||"ru-RU",{year:"numeric",month:"2-digit",day:"2-digit",
          hour:"2-digit",minute:"2-digit",second:"2-digit"});
    };
    const duration=value=>value==null||!Number.isFinite(Number(value))
      ?"Не записано":clockElapsed(Number(value));
    const statusLabel={
      complete:"Завершён",failed:"Ошибка",running:"Выполняется",
      cancelled:"Остановлен",unknown:"Неизвестно"
    };
    const profiles=new Map();
    for(const run of history){
      const profile=run.profile||"Профиль не записан";
      if(!profiles.has(profile))profiles.set(profile,[]);
      profiles.get(profile).push(run);
    }
    target.innerHTML=Array.from(profiles,([profile,runs])=>
      '<section class="analysis-run-profile-group">'+
      '<div class="analysis-run-profile-heading"><h4>'+esc(profile)+'</h4>'+
      '<small>'+runs.length+' '+(runs.length===1?"запуск":runs.length<5?"запуска":"запусков")+'</small></div>'+
      runs.map((run,i)=>{
        const videos=run.videos||[];
        const stages=videos.reduce((total,video)=>total+(video.stages||[])
          .filter(stage=>stage.duration_sec!=null).length,0);
        const expected=Number(run.videos_total||videos.length)*
          Number(run.stage_names?.length||0);
        const status=statusLabel[run.status]||"Неизвестно";
        const rows=videos.map(video=>{
          const stagesHtml=(video.stages||[]).map(stage=>
            '<tr><td>'+esc(stage.stage||"Этап "+stage.stage_index)+'</td>'+
            '<td>'+(stage.duration_sec==null?"Не записано":duration(stage.duration_sec))+'</td>'+
            '<td>'+(stage.fps==null?"—":Number(stage.fps).toFixed(1)+" кадр/с")+'</td></tr>'
          ).join("");
          const interval=video.start!=null && video.seconds!=null
            ?' · фрагмент '+Number(video.start).toFixed(1)+'–'+
                (Number(video.start)+Number(video.seconds)).toFixed(1)+' с':'';
          return '<section class="analysis-run-video">'+
            '<div class="analysis-run-video-head"><h5>'+esc(video.video||"Видео")+esc(interval)+'</h5>'+
            '<span>Время видео: <b>'+duration(video.duration_sec)+'</b></span></div>'+
            '<div class="analysis-run-table-wrap"><table class="analysis-run-stages">'+
            '<thead><tr><th>Стадия</th><th>Длительность</th><th>Скорость</th></tr></thead>'+
            '<tbody>'+ (stagesHtml||'<tr><td colspan="3">Время стадий не было записано в этом запуске</td></tr>')+
            '</tbody></table></div></section>';
        }).join("");
        return '<details class="analysis-run-record" '+(!i?'open':'')+'>'+
          '<summary><span class="analysis-run-record-title"><strong>'+esc(profile)+'</strong>'+
          '<small>Старт: '+esc(dateTime(run.started_at))+'</small></span>'+
          '<span class="analysis-run-summary-meta">'+
          '<span class="analysis-run-total">Всего '+duration(run.duration_sec)+'</span>'+
          '<span class="analysis-run-status" data-status="'+esc(run.status||"unknown")+'">'+esc(status)+'</span>'+
          '</span></summary>'+
          '<div class="analysis-run-record-content">'+
          '<div class="analysis-run-facts">'+
          '<span><small>Запущен</small><strong>'+esc(dateTime(run.started_at))+'</strong></span>'+
          '<span><small>Завершён</small><strong>'+esc(dateTime(run.finished_at))+'</strong></span>'+
          '<span><small>Общее время анализа</small><strong>'+duration(run.duration_sec)+'</strong></span>'+
          '<span><small>Стадии с замером</small><strong>'+stages+
          (expected?'/'+expected:"")+'</strong></span>'+
          '<span><small>Тип запуска</small><strong>'+esc(run.kind==="fragment"
            ?run.mode_label||"Частичный анализ":"Весь проект")+'</strong></span>'+
          '</div>'+
          (rows||'<p class="muted">Подробности по видео не записаны.</p>')+
          (run.error?'<p class="analysis-run-error">'+esc(run.error)+'</p>':'')+
          '</div></details>';
      }).join("")+'</section>'
    ).join("");
  }catch(error){
    if(projectId===state.project)
      target.textContent="Не удалось прочитать историю: "+error.message;
  }
}
async function loadAll(preferredId=null,epoch=++projectLoadEpoch) {
  const finish=beginWorkspaceLoading("Загрузка данных проекта…");
  try{return await loadAllCore(preferredId,epoch);}
  finally{finish();}
}
async function loadAllCore(preferredId,epoch) {
  const previousProject=state.project;
  if(!await loadProjects(preferredId,epoch)||epoch!==projectLoadEpoch)return false;
  const project=selectedProject();
  if(previousProject!==state.project){
    // Never display a completed run or report from another project while
    // the new project's network requests are in flight.
    ++state.reportEpoch;
    state.report=null;state.aggregate=null;state.reportDirty=false;state.renders=[];state.analysisContext=null;
    state.analysisRuns=[];state.analysisRunId="";
    state.players=[];state.photoCounts={};state.activePlayerId=null;
    state.playerMetaProject="";state.reportContext="";
    state.playerGroups=null;state.playerGroupsKey="";state.playerReidThresholdPending=null;
    clearTimeout(playerGroupsPoll);
    renderAnalysisRunChoices();renderReport();renderRenders();displayAnalysisContext();
  }
  if(project){
    // Set the owner before loading the preview, report or run-dependent UI.
    window.VolleyballAnalysisUI.setProjectRanges(project.ranges||{},project.id);
    if(!await loadAnalysisRuns())return false;
    if(epoch!==projectLoadEpoch)return false;
    // Critical data is loaded concurrently. Benchmark/history/secondary
    // metadata no longer hold the entire workspace behind a loading screen.
    const previewActive=$("#view-analysis-results").classList.contains("active");
    const montageActive=$("#view-results").classList.contains("active");
    const critical=[loadReport()];
    if(state.video)critical.push(window.VolleyballAnalysisUI.setVideo(state.video));
    if(montageActive)critical.push(loadRenders());
    if(previewActive){
      window.VolleyballResultsUI.setCacheSelection(analysisCacheMap(),state.analysisRunId,
        selectedAnalysisRun()?.postprocess_settings||null);
      critical.push(window.VolleyballResultsUI.setProject(project.id,project.videos,state.video));
    }
    const optional=[loadBenchmarkResults,loadBenchmarkModels,refreshAnalysisContext,
      loadAnalysisHistory,refreshVideoPreparation];
    if(!montageActive)optional.push(loadRenders);
    await Promise.all(critical);
    if(epoch!==projectLoadEpoch)return false;
    // Background requests are started only after primary data is ready.
    // This avoids competing with the preview/report on a single local server.
    for(const task of optional)
      Promise.resolve().then(()=>epoch===projectLoadEpoch&&project.id===state.project?
        task():null).catch(error=>console.warn("Background workspace data:",error));
  }else{
    state.report=null;state.aggregate=null;state.reportDirty=false;state.renders=[];state.analysisContext=null;
    state.analysisRuns=[];state.analysisRunId="";renderAnalysisRunChoices();
    state.players=[];state.libraryPlayers=[];state.photoCounts={};state.activePlayerId=null;
    state.playerMetaProject="";state.reportContext="";
    state.playerGroups=null;state.playerGroupsKey="";clearTimeout(playerGroupsPoll);
    window.VolleyballAnalysisUI.setProjectRanges({},"");
    $("#source-video-player").pause();
    $("#source-video-player").removeAttribute("src");
    $("#source-video-player").load();
    renderReport();renderRenders();displayAnalysisContext();
    await loadAnalysisHistory();
  }
  if(epoch!==projectLoadEpoch)return false;
  alertBox("");
  return true;
}
async function loadReport({playersOnly=false}={}) {
  if(!state.project||!state.video)return;
  const epoch=++state.reportEpoch;
  const project=state.project,video=state.video,runId=state.analysisRunId;
  const projectId=encodeURIComponent(project),cacheId=analysisCacheId();
  const context=[project,video,runId,cacheId].join("\u0000");
  const reuse=playersOnly&&state.reportContext===context&&
    state.report&&state.aggregate&&!state.reportDirty;
  const needPlayers=$("#view-players").classList.contains("active");
  // Project/video/CPU switches do not need the complete photo library.
  // Fetch it only in Players; cache identical global catalog calls briefly.
  const [report,aggregate,roster,catalog]=await Promise.all([
    reuse?Promise.resolve(state.report):
      cacheId?api("/api/report?video="+encodeURIComponent(video)+"&project_id="+projectId+
        "&cache_id="+encodeURIComponent(cacheId)):Promise.resolve({
          analyzed:false,source:video,raw:[],groups:[],counts:{},rallies:0}),
    reuse?Promise.resolve(state.aggregate):api("/api/project-report?id="+projectId+
      (runId?"&run_id="+encodeURIComponent(runId):"")),
    needPlayers?api("/api/players?project_id="+projectId):Promise.resolve(null),
    needPlayers?sharedPlayerCatalogs():Promise.resolve(null)
  ]);
  if(epoch!==state.reportEpoch||project!==state.project||
     video!==state.video||runId!==state.analysisRunId)return;
  state.report=report;
  state.aggregate=aggregate;
  state.reportContext=context;
  state.reportDirty=false;
  if(needPlayers){
    state.players=roster.players||[];
    state.libraryPlayers=catalog.libraryPlayers;
    state.photoCounts=catalog.photoCounts;
    state.playerMetaProject=project;
    if(!state.players.some(p=>p.id===state.activePlayerId))state.activePlayerId=null;
  }
  if(!reuse){
    state.selected.clear();
    state.previewId=report?.raw?.[0]?.player_track_id!=null?
      String(report.raw[0].player_track_id):null;
    renderReport();
  }
  if(needPlayers){
    renderPlayers();
    if(state.playerMode!=="view")loadPlayerGroups().catch(console.warn);
    if(state.playerMode==="manual"){
      loadPlayerSuggestions().catch(console.warn);
      renderPlayerPhotos().catch(console.warn);
    }else if(state.playerMode==="view"&&state.activePlayerId){
      renderPlayerAlbum().catch(console.warn);
    }
  }
  populatePlayersSelect();
}
async function loadRenders(){
  const p=selectedProject();
  if(!p){state.renders=[];renderRenders();return}
  syncMontageMode();
  try{
    const response=await api("/api/montage/outputs?project_id="+encodeURIComponent(p.id));
    if(state.project!==p.id)return;
    state.renders=response.videos||[];
  }catch(error){
    if(state.project!==p.id)return;
    console.warn("Не удалось прочитать видео:",error);
    $("#montage-preview-status").textContent="Ошибка библиотеки: "+error.message;
    state.renders=[];
  }
  if(state.project===p.id)renderRenders();
}
function displayAnalysisContext(){
  const el=$("#analysis-context-summary");
  const p=selectedProject();
  if(!p){el.replaceChildren();return;}
  const contexts=new Map((state.analysisContext?.videos||[]).map(c=>[c.video,c]));
  el.innerHTML='<div class="analysis-context-heading">Сохранённый контекст по видео</div>'+
    p.videos.map(name=>{
      const item=contexts.get(name);
      const range=item?.range||p.ranges[name];
      const period=range?clock(range.start)+" – "+clock(range.end):"Не задан";
      const court=item?.court_saved?
        ("Площадка: "+(item.court_estimated||0)+" за кадром, "+
         (item.court_unknown||0)+" неизвестны"+
         (item.court_homography_ready===false?" · перспектива недоступна":"")):
        "Без разметки площадки";
      return '<div class="analysis-context-row"><span class="context-video">'+esc(name)+'</span>'+
        '<span class="context-range">'+esc(period)+'</span>'+
        '<span class="context-court '+(item?.court_saved?"is-saved":"is-missing")+'">'+court+'</span>'+
        '<button type="button" class="context-reset-button" data-reset-context="'+esc(name)+
        '" aria-label="Сбросить диапазон и разметку для '+esc(name)+
        '" title="Сбросить контекст этого видео">×</button></div>';
    }).join("");
}
async function refreshAnalysisContext(){
  if(!state.project){state.analysisContext=null;displayAnalysisContext();return;}
  const pid=state.project;
  const context=await api("/api/project-context?project_id="+encodeURIComponent(pid));
  if(pid!==state.project)return;
  state.analysisContext=context;
  displayAnalysisContext();
}
async function saveCurrentRange(){
  const project=selectedProject();
  const active=state.video;
  if(!project||window.VolleyballAnalysisUI.activeProject()!==project.id||
    window.VolleyballAnalysisUI.activeVideo()!==active)return;
  const range=window.VolleyballAnalysisUI.currentRange();
  if(!range)return;
  const courtUI=window.VolleyballCourtUI;
  const revision=courtUI.revision();
  const draft=courtUI.snapshot();
  const result=await api("/api/analysis-context",{
    project_id:project.id,video:active,range,court:draft.court,clear_court:draft.clear_court
  });
  project.ranges={...project.ranges,[active]:result.range};
  // A response may arrive after another edit. Never discard that edit.
  if(state.project===project.id&&state.video===active){
    courtUI.acceptSaved(result.court,revision);
    $("#range-save-status").textContent="Изменения сохранены автоматически";
    $("#autosave-error").hidden=true;
    renderSources();
    await refreshAnalysisContext();
  }
}
// Serialized, debounced writes: a video is never switched before its latest
// unsaved range and annotation have been flushed to the project.
window.VolleyballAutoSave=(()=>{
  let timer=null;
  let pending=false;
  let queue=Promise.resolve();
  let changed=0;
  const status=()=>$("#range-save-status");
  function schedule(delay=750){
    changed++;
    pending=true;
    if(timer!==null)clearTimeout(timer);
    timer=setTimeout(()=>{timer=null;flush().catch(console.warn);},delay);
  }
  function flush(){
    if(timer!==null){clearTimeout(timer);timer=null;}
    if(!pending)return queue;
    pending=false;
    const version=changed;
    queue=queue.catch(()=>{}).then(async()=>{
      try{
        await saveCurrentRange();
        if(version===changed){
          $("#autosave-error").hidden=true;
          status().textContent="Сохранено автоматически";
        }
      }catch(error){
        if(version===changed){
          const el=$("#autosave-error");
          el.textContent="Не удалось сохранить: "+error.message;el.hidden=false;
          status().textContent=el.textContent;
          pending=true;
        }
        throw error;
      }
    });
    return queue;
  }
  // Pending write is obsolete when explicitly resetting the same recording.
  async function idleAndCancel(){
    if(timer!==null){clearTimeout(timer);timer=null;}
    pending=false;
    changed++;
    await queue.catch(()=>{});
  }
  return {schedule,flush,idleAndCancel};
})();
async function resetVideoContext(name){
  const project=selectedProject();
  if(!project||!project.videos.includes(name))return;
  const button=$$(".context-reset-button").find(x=>x.dataset.resetContext===name);
  if(button)button.disabled=true;
  try{
    if(state.video===name)await window.VolleyballAutoSave.idleAndCancel();
    else await window.VolleyballAutoSave.flush();
    const reset=await api("/api/analysis-context-reset",{
      project_id:project.id,video:name
    });
    project.ranges={...project.ranges,[name]:reset.range};
    if(state.project===project.id){
      await refreshAnalysisContext();
      renderSources();
      if(state.video===name){
        window.VolleyballAnalysisUI.setProjectRanges(project.ranges,project.id);
        await window.VolleyballAnalysisUI.setVideo(name);
      }
    }
  }catch(error){
    const el=$("#autosave-error");
    el.textContent="Ошибка сброса: "+error.message;
    el.hidden=false;
    throw error;
  }finally{
    if(button)button.disabled=false;
  }
}
function clockElapsed(seconds){
  if(seconds==null||!Number.isFinite(Number(seconds)))return "—";
  const value=Math.max(0,Math.floor(Number(seconds)));
  const hh=Math.floor(value/3600),mm=Math.floor(value/60)%60,ss=value%60;
  return (hh?String(hh).padStart(2,"0")+":":"")+
    String(mm).padStart(2,"0")+":"+String(ss).padStart(2,"0");
}
// Mirror the current project's analysis, CPU replay and montage jobs in the sidebar.
// The same /api/job response drives the detailed tab panels and these compact cards.
function renderSidebarActivity(info=state.latestTask){
  const kind=info?.kind==="highlights"?"montage":
    info?.kind==="reprocess"?"reprocess":
    ["analyze","experiment"].includes(info?.kind)?"analysis":null;
  const visible=Boolean(state.project&&info?.project_id===state.project&&
    kind&&info.status!=="idle");
  const container=$("#sidebar-tasks");
  container.hidden=!visible;
  $("#sidebar-reprocess-cancel").hidden=!(visible&&kind==="reprocess"&&info.status==="running");
  for(const type of ["analysis","montage","reprocess"]){
    const card=$("#sidebar-"+type+"-task");
    card.hidden=!visible||kind!==type;
    if(card.hidden)continue;
    const status={running:"Выполняется",complete:"Готово",
      failed:"Ошибка",cancelled:"Остановлено"}[info.status]||info.status;
    const raw=info.status==="complete"?100:Number(info.progress);
    const percent=Number.isFinite(raw)?Math.max(0,Math.min(100,raw)):0;
    const total=Math.max(0,Number(info.total_stages)||0);
    const completed=info.status==="complete"?total:
      Math.min(total,Math.max(0,Number(info.completed_stages)||0));
    const stage=info.stage||
      (info.status==="complete"?"Обработка завершена":
       info.status==="failed"?"Ошибка обработки":
       info.status==="cancelled"?"Задача остановлена":"Ожидание запуска");
    card.dataset.status=info.status;
    $("#sidebar-"+type+"-state").textContent=status;
    const stageNode=$("#sidebar-"+type+"-stage");
    const visibleStage=type==="reprocess"&&info.video_index
      ?"Видео "+info.video_index+"/"+(info.videos_total||"?")+
        (info.video_name?" · "+info.video_name:"")+" · "+stage
      :stage;
    stageNode.textContent=visibleStage;
    stageNode.title=visibleStage;
    $("#sidebar-"+type+"-fill").style.width=percent+"%";
    const meter=card.querySelector('[role="progressbar"]');
    meter.setAttribute("aria-valuenow",String(percent));
    $("#sidebar-"+type+"-count").textContent=
      (total?completed+"/"+total+(type==="reprocess"?" шагов · ":" этапов · "):"")+
      percent.toFixed(1)+"%";
    $("#sidebar-"+type+"-time").textContent=clockElapsed(info.elapsed_sec);
  }
}
function updateMontageTask(info){
  const panel=$("#montage-job-panel");
  panel.hidden=!(info.kind==="highlights"&&info.status!=="idle"&&
    state.project&&info.project_id===state.project);
  if(panel.hidden)return;
  const statusNames={running:"Выполняется",complete:"Завершено",
    failed:"Ошибка",cancelled:"Остановлено"};
  $("#montage-job-badge").textContent=statusNames[info.status]||info.status;
  const total=Math.max(1,Number(info.total_stages)||4);
  const complete=info.status==="complete";
  const completed=complete?total:Math.min(total,Math.max(0,Number(info.completed_stages)||0));
  $("#montage-job-count").textContent=completed+"/"+total+" стадий";
  $("#montage-job-elapsed").textContent=(complete?"Выполнено за: ":"Время выполнения: ")+
    clockElapsed(info.elapsed_sec);
  const rail=$("#montage-job-progress");
  rail.style.width=(completed/total*100)+"%";
  const control=rail.parentNode;
  control.setAttribute("aria-valuenow",String(completed/total*100));
  control.setAttribute("aria-valuetext",completed+" из "+total+" стадий");
  const stageLabel=info.stage||(
    info.status==="running"?"Ожидание начала обработки":
    info.status==="failed"?"Монтаж завершился с ошибкой":
    info.status==="cancelled"?"Монтаж остановлен":"");
  $("#montage-job-stage").textContent=complete?"Готовое видео сохранено":stageLabel;
  $("#montage-cancel").disabled=info.status!=="running";
  const terminal=$("#montage-job-log");
  const output=info.log||"Журнал пока пуст.";
  if(terminal.textContent!==output){
    const follow=terminal.scrollHeight-terminal.clientHeight-terminal.scrollTop<=36;
    terminal.textContent=output;
    if(follow)terminal.scrollTop=terminal.scrollHeight;
  }
}
function updateTaskPanel(info,kind,prefix){
  if(kind==="highlights"){updateMontageTask(info);return;}
  const E=id=>document.getElementById(prefix+id);
  const status={idle:"Нет задачи",running:"Выполняется",complete:"Завершено",
    failed:"Ошибка",cancelled:"Остановлено"}[info.status]||info.status;
  const relevant=kind==="analyze"
    ?["analyze","experiment"].includes(info.kind)
    :info.kind===kind;
  const showLive=relevant&&info.status==="running"&&Boolean(state.project)&&
    (!info.project_id||info.project_id===state.project);
  const livePanel=document.getElementById("analysis-job-panel");
  livePanel.hidden=!showLive;
  if(!showLive)return;
  if(showLive){
    // The active analysis is temporary UI in the history section, not a
    // persisted history record. Automatically expose progress while running.
    const container=document.querySelector(".analysis-history-collapsible");
    if(state.liveHistoryOpenedJob!==info.id){
      state.liveHistoryOpenedJob=info.id;
      if(container&&!container.open)container.open=true;
    }
    document.getElementById("job-profile").textContent=
      "Профиль: "+(info.profile_name||state.activeAnalysisProfile||"Текущий запуск");
  }
  E("job-badge").textContent=status;
  E("job-head").textContent=info.status==="running"
    ?kind==="analyze"
      ?info.kind==="experiment"?"Выбранные этапы обрабатываются":"Анализ выполняется"
      :"Монтаж выполняется"
    :info.status==="complete"?"Задача завершена":
     info.status==="failed"?"Ошибка выполнения":
     info.status==="cancelled"?"Обработка остановлена":"";
  E("job-sub").textContent=info.stage||
    (kind==="analyze"?"Подготовка видео и моделей":"Отбор игровых эпизодов и монтаж");
  const complete=info.status==="complete";
  const overall=complete?100:Math.max(0,Math.min(99.9,Number(info.progress)||0));
  const stageRaw=info.stage_percent;
  const stagePercent=complete?100:
    stageRaw==null?0:Math.max(0,Math.min(100,Number(stageRaw)||0));
  const completed=Math.max(0,Number(info.completed_stages)||0);
  const total=Math.max(0,Number(info.total_stages)||0);
  const stepProgress=total?Math.max(0,Math.min(100,completed/total*100)):0;
  E("job-percent").textContent=kind==="analyze"
    ?completed+"/"+(total||"—")+" · "+overall.toFixed(1)+"%"
    :overall.toFixed(1)+"%";
  E("job-progress").style.width=overall+"%";
  E("job-progress").parentNode.setAttribute("aria-valuenow",String(overall));
  if(kind==="analyze"){
    E("job-progress").parentNode.setAttribute("aria-valuetext",
      completed+" из "+(total||"неизвестного числа")+" стадий, "+overall.toFixed(1)+"%");
  }else{
    E("job-progress").parentNode.removeAttribute("aria-valuetext");
  }
  E("job-stage-percent").textContent=kind==="analyze"
    ?complete?"Завершено":info.status==="running"?"Выполняется":"—"
    :stageRaw==null&&!complete?"—":stagePercent.toFixed(1)+"%";
  E("job-stage-progress").style.width=stagePercent+"%";
  E("job-stage-progress").parentNode.setAttribute("aria-valuenow",String(stagePercent));
  E("job-completed").textContent=kind==="analyze"
    ?"Пройдено "+completed+"/"+(total||"—")+" стадий"
    :"Выполнено "+completed+" из "+(total||"—")+" этапов";
  const stageIndex=Math.max(0,Number(info.stage_index)||0);
  const stageTotal=Math.max(0,Number(info.stage_total)||0);
  let caption=stageTotal?"Этап "+stageIndex+" из "+stageTotal:"Ожидание стадий";
  if(Number.isFinite(Number(info.frames))&&Number(info.total_frames)>0)
    caption+=" · "+Number(info.frames).toLocaleString(window.VolleyI18n?.locale()||"ru-RU")+" / "+
      Number(info.total_frames).toLocaleString(window.VolleyI18n?.locale()||"ru-RU")+" кадров";
  if(info.status==="running"&&Number.isFinite(Number(info.eta_sec))&&info.eta_sec!=null)
    caption+=" · осталось ~"+clockElapsed(info.eta_sec);
  E("job-stage-count").textContent=caption;
  E("job-stage-title").textContent=info.stage||"Текущая стадия";
  if(kind==="analyze"){
    const stageNames=["Проверка подготовленного видео / диапазона","Обнаружение мяча",
      "Обнаружение и отслеживание игроков","Построение траекторий и розыгрышей",
      "Классификация игровых событий","Подготовка результатов анализа"];
    const stageTotal=Math.max(1,Number(info.stage_total)||6);
    const known=total>0?Math.min(total,42):0;
    const next=E("analysis-live-stages");
    const durations=info.stage_durations||[];
    const stageSignature=[info.id,known,completed,info.status,info.videos_total,
      ...durations.map(row=>[row.video_index,row.stage_index,row.duration_sec].join(":"))].join("|");
    if(next.dataset.stageSignature!==stageSignature){
    next.innerHTML=known?Array.from({length:known},(_,i)=>{
      const video=Math.floor(i/stageTotal)+1;
      const pos=i%stageTotal;
      const title=stageNames[pos]||"Стадия "+(pos+1);
      const done=i<completed || info.status==="complete";
      const running=!done&&info.status==="running"&&i===completed;
      const timing=(info.stage_durations||[]).find(row=>
        Number(row.video_index)===video&&Number(row.stage_index)===pos+1);
      const elapsed=timing&&Number.isFinite(Number(timing.duration_sec))
        ?(Number(timing.duration_sec)<1
          ?Number(timing.duration_sec).toFixed(2)+" с"
          :Number(timing.duration_sec)<60
            ?Number(timing.duration_sec).toFixed(1)+" с"
            :clockElapsed(Number(timing.duration_sec))):null;
      return '<div class="analysis-live-stage '+(done?'done':running?'active':'pending')+'">'+
        '<span class="analysis-live-stage-marker">'+(done?'✓':running?'●':'·')+'</span>'+
        '<span class="analysis-live-stage-label">'+(Number(info.videos_total)>1?'Видео '+video+' · ':'')+
        esc(title)+'</span><span class="analysis-live-stage-status">'+
        (done?(elapsed?'✓ '+elapsed:'Завершено'):running?'Выполняется':'Ожидает')+'</span></div>';
    }).join(""):"";
    next.dataset.stageSignature=stageSignature;
    }
  }
  E("job-video-count").textContent=kind==="analyze"?
    (info.kind==="experiment"?"Выбранный фрагмент одного видео":
      info.videos_total?"Видео "+(info.video_index||1)+" из "+info.videos_total:"Видео проекта"):
    "Монтаж по готовому анализу";
  E("job-elapsed").textContent=(complete?"Выполнено за: ":"Время выполнения: ")+
    clockElapsed(info.elapsed_sec);
  const button=kind==="analyze"?E("cancel-job"):document.getElementById("montage-cancel");
  if(button)button.disabled=info.status!=="running";
  const journal=E("job-log");
  const text=info.log||"Здесь появятся сообщения обработки.";
  if(journal.textContent!==text){
    const follow=journal.scrollHeight-journal.clientHeight-journal.scrollTop<=36;
    journal.textContent=text;
    if(follow)journal.scrollTop=journal.scrollHeight;
  }
}
// Prevent overlapping status requests from navigation, buttons and scheduled polls.
let jobPollPending=null;
function pollJob(){
  if(jobPollPending)return jobPollPending;
  const current=pollJobCore().finally(()=>{
    if(jobPollPending===current)jobPollPending=null;
  });
  jobPollPending=current;
  return current;
}
async function pollJobCore() {
  const info=await api("/api/job");
  state.latestTask=info;
  renderSidebarActivity(info);
  // An already-running job may predate the new stage timing API.
  // Read only the latest in-progress project's stored stage measurements;
  // these are for the live progress panel, never for completed-run history.
  if(info.kind==="analyze"&&["running","complete"].includes(info.status)&&state.project){
    if(state.liveTimingJob!==info.id){
      state.liveTimingJob=info.id;
      state.liveTimingCount=-1;
      state.liveTimingRows=[];
    }
    const completed=Math.max(0,Number(info.completed_stages)||0);
    if((info.stage_durations||[]).length<completed){
      if(state.liveTimingCount!==completed){
        state.liveTimingCount=completed;
        try{
          const result=await api("/api/analysis-history-legacy?project_id="+
            encodeURIComponent(state.project));
          const current=(result.history||[]).find(row=>row.status===info.status);
          if(current){
            state.liveTimingRows=(current.videos||[]).flatMap((video,index)=>
              (video.stages||[]).filter(stage=>stage.duration_sec!=null)
                .map(stage=>({
                  video_index:index+1,stage_index:stage.stage_index,
                  duration_sec:stage.duration_sec
                })));
          }
        }catch(error){console.warn("Live stage durations:",error)}
      }
      info.stage_durations=(info.stage_durations||[]).concat(
        (state.liveTimingRows||[]).filter(row=>
          !(info.stage_durations||[]).some(other=>
            Number(other.video_index)===Number(row.video_index)&&
            Number(other.stage_index)===Number(row.stage_index))));
    }
  }
  const previous=state.jobStatus;
  state.jobStatus=info.status;
  $("#job-badge").textContent={
    idle:"Нет задачи",running:"Выполняется",complete:"Завершено",failed:"Ошибка",cancelled:"Остановлено"
  }[info.status]||info.status;
  const analysisJob=info.kind==="analyze";
  const partialAnalysisJob=info.kind==="experiment";
  const analysisFailure=(()=>{
    if(!analysisJob||info.status!=="failed")return "";
    const log=String(info.log||"");
    const explicit=log.match(/Недостаточно места для создания фрагмента анализа:[^\r\n]+/);
    if(explicit)return explicit[0].slice(0,420);
    if(/Error muxing a packet|Error writing trailer|No space left on device/.test(log))
      return "FFmpeg не смог записать видеофрагмент. Проверьте свободное место на диске и выберите меньший диапазон анализа.";
    return "";
  })();
  const replayStatus=document.getElementById("reprocess-status");
  if(replayStatus&&info.kind==="reprocess"){
    replayStatus.textContent=(
      {running:"Пересчёт выполняется · прогресс в левой панели",
       complete:"Готово · новая версия доступна в выборе запусков слева.",
       failed:"Ошибка пересчёта · проверьте журнал задачи.",
       cancelled:"Пересчёт остановлен."}[info.status]||"Пересчёт CPU")+
      (info.status==="running"&&info.stage?" · "+info.stage:"");
  }
  const replayButton=document.getElementById("reprocess-start");
  if(replayButton)replayButton.disabled=(info.kind==="reprocess"&&info.status==="running")||!selectedAnalysisRun();
  if(info.kind==="reprocess"){
    const previewStatus=document.getElementById("result-reprocess-status");
    const previewButton=document.getElementById("result-reprocess-start");
    if(previewButton)previewButton.disabled=info.status==="running"||!selectedAnalysisRun();
    if(previewStatus){
      const messages={
        running:"Пересчитываем события и розыгрыши на CPU…",
        complete:"Готово. Выберите новую версию анализа в левой панели.",
        failed:"Ошибка пересчёта. Проверьте журнал задачи.",
        cancelled:"Пересчёт отменён."
      };
      previewStatus.textContent=(messages[info.status]||"Повторная обработка")+
        (info.status==="running"&&info.stage?" · "+info.stage:"");
    }
  }
  window.VolleyballResultsUI.setReprocessTask?.(info);
  updateTaskPanel(info,"analyze","");
  updateTaskPanel(info,"highlights","montage-");
  // Older completed runs remain viewable while a new GPU run is in progress.
  $("#open-analysis-results").disabled=!state.analysisRuns.some(run=>
    Object.prototype.hasOwnProperty.call(run.videos||{},state.video));
  $("#analysis-run-status").textContent=analysisJob?
    ({running:"Анализ выполняется · прогресс в истории запусков.",
      complete:"Анализ завершён. Результаты доступны в следующей вкладке.",
      failed:analysisFailure||"Анализ завершился с ошибкой · подробности в журнале текущего запуска.",
      cancelled:"Анализ остановлен."}[info.status]||""):
    partialAnalysisJob?
    ({running:"Выбранный этап обрабатывается · ход и журнал ниже.",
      complete:"Выбранный этап обработан. Файлы результата сохранены отдельно.",
      failed:"Выбранный этап завершился с ошибкой · подробности ниже.",
      cancelled:"Частичный анализ остановлен."}[info.status]||""):
    "";
  $("#benchmark-state").textContent=info.kind==="benchmark"
    ? ({running:"Измерение…",complete:"Готово",failed:"Ошибка",cancelled:"Остановлен"}[info.status]||"Нет замера")
    : "Нет замера";
  $("#benchmark-cancel").disabled=info.status!=="running"||info.kind!=="benchmark";
  $("#benchmark-submit").disabled=!state.benchmarkReady ||
    (info.kind==="benchmark" && info.status==="running");
  const benchmarkPercent=info.kind==="benchmark"?
    (info.progress==null?(info.status==="complete"?100:0):Number(info.progress)):0;
  $("#benchmark-percent").textContent=info.kind==="benchmark"?benchmarkPercent.toFixed(1)+"%":"—";
  $("#benchmark-progress").style.width=Math.min(100,Math.max(0,benchmarkPercent))+"%";
  $("#benchmark-log").textContent=info.kind==="benchmark"?(info.log||"Подготовка…"):"Здесь появится журнал замера.";
  if(info.kind==="benchmark" && info.status==="failed")$("#benchmark-log-details").open=true;
  $("#benchmark-current").querySelector("b").textContent=info.kind==="benchmark"?
    (info.status==="running"?"10 последовательных CPU/GPU тестов":
     info.status==="complete"?"Серия из восьми измерений завершена":"Статус: "+info.status):
    "Ожидание запуска";
  $("#benchmark-current").querySelector("p").textContent=info.kind==="benchmark"?
    (info.stage||info.log?.slice(-150)||"Запуск восьми измерений"):
    "Выберите движок и запустите тест.";
  if(info.kind==="benchmark"){
    if(info.status==="running"){
      // Every completed device is persisted immediately; render real partial
      // bars while the next model runs. No simulated measurements.
      if($("#view-benchmark").classList.contains("active"))
        await loadBenchmarkResults().catch(error=>console.warn(error));
      $("#benchmark-compare-settings").textContent+=" · выполняется "+benchmarkPercent.toFixed(1)+"%";
      state.benchmarkRenderedId=null;
    }else if(info.id && info.id!==state.benchmarkRenderedId &&
             ["complete","failed","cancelled"].includes(info.status)){
      await loadBenchmarkResults().catch(error=>console.warn(error));
      if(info.status!=="complete")
        $("#benchmark-compare-settings").textContent+=" · прервано";
      state.benchmarkRenderedId=info.id;
    }
  }
  if(previous==="running" && info.status!=="running"){
    if(info.kind==="analyze"||info.kind==="experiment")
      await loadAnalysisHistory();
    if(info.kind!=="benchmark"&&info.kind!=="experiment"){
      if(info.kind==="analyze"&&info.status==="complete"&&
         info.project_id===state.project){
        state.analysisRunId="__new__";
        const profile=info.profile_name||state.activeAnalysisProfile;
        if(profile)try{
          localStorage.setItem("volley-analysis-profile:"+state.project,profile);
        }catch{}
      }
      await Promise.all([loadAll(),loadRenders()]);
      if(info.kind==="reprocess"&&pendingPreviewReplay){
        const pending=pendingPreviewReplay;
        pendingPreviewReplay=null;
        if(info.status==="complete"&&state.project===pending.project&&
           state.video===pending.video&&state.analysisRunId===pending.selectedRun){
          const same=(a,b)=>b==null?a==null:Number(a)===Number(b);
          const variant=state.analysisRuns.find(run=>
            run.parent_run_id===pending.parent &&
            run.postprocess_settings?.mode===pending.mode &&
            same(run.postprocess_settings?.touch_threshold,pending.touch) &&
            same(run.postprocess_settings?.ball_threshold,pending.ball) &&
            same(run.postprocess_settings?.player_threshold,pending.player));
          if(variant)await chooseAnalysisRun(variant.id);
        }
      }
    }
    if(info.status==="failed")alertBox(
      info.kind==="benchmark"?"Бенчмарк завершился с ошибкой. Журнал на вкладке «Бенчмарк».":
      info.kind==="highlights"?"Монтаж завершился с ошибкой. Журнал находится во вкладке «Монтаж».":
      analysisFailure||"Анализ завершился с ошибкой. Журнал находится во вкладке «Анализ».");
  }
}
const BENCHMARK_NAMES = {
  "VballNetV4c_seq9_grayscale_20260908_213829.onnx":"VballNetV4c",
  "VballNetFastV1_seq9_grayscale_233_h288_w512.onnx":"VballNetFastV1",
  "RAVEL-VB-011-9f.onnx":"RAVEL-VB-011",
  "RAVEL-VB-012-9f.onnx":"RAVEL-VB-012",
  "RAVEL-VB-013-9f.onnx":"RAVEL-VB-013"
};
const BENCHMARK_FAMILIES=[
  {id:"VballNet",title:"Детекция мяча · VballNet",description:"Модели мяча"},
  {id:"RAVEL",title:"Игроки, головы и мяч · RAVEL",description:"Модели игроков"}
];
const benchNumber=(value,digits=1)=>Number(value).toLocaleString(window.VolleyI18n?.locale()||"ru-RU",{
  minimumFractionDigits:digits,maximumFractionDigits:digits
});
const benchmarkDate=row=>{
  const date=new Date(row.created_at);
  return Number.isNaN(date.getTime())?"":date.toLocaleString(window.VolleyI18n?.locale()||"ru-RU",{dateStyle:"short",timeStyle:"short"});
};
async function loadBenchmarkModels(){
  const data=await api("/api/benchmark-models");
  const names=(data.models||[]).filter(name=>Object.hasOwn(BENCHMARK_NAMES,name));
  state.benchmarkReady=names.length===5;
  $("#benchmark-model-availability").textContent=state.benchmarkReady
    ?"Все пять моделей установлены. Замеры: CPU, затем CUDA, по очереди."
    :"Не найдены все пять моделей (VballNetV4c, FastV1, RAVEL-011, RAVEL-012, RAVEL-013).";
  const selector=$("#benchmark-video");
  const old=selector.value;
  const videos=selectedProject()?.videos||[];
  selector.replaceChildren(...videos.map(name=>new Option(name,name)));
  selector.value=videos.includes(old)?old:videos.includes(state.video)?state.video:videos[0]||"";
  $("#benchmark-submit").disabled=!state.benchmarkReady||!videos.length;
  try{
    const resources=await api("/api/analysis-resources");
    const threads=$("#benchmark-form [name='threads']");
    if(!threads.dataset.initialized){
      threads.value=String(resources.default_threads);
      threads.dataset.initialized="true";
    }
  }catch{}
}
function renderBenchmarkChart(rows){
  const complete=rows.filter(row=>row.status==="complete"&&Number.isFinite(Number(row.fps))&&Number(row.fps)>0);
  if(!rows.length)return '<div class="empty">Нет измерений для графика.</div>';
  const first=rows[0],hw=first.hardware||{},expected=Number(first.expected_tests)||8;
  const hardware='<div class="bench-hardware">'+
    '<span>CPU: '+esc(hw.cpu||"Неизвестно")+'</span>'+
    '<span>GPU: '+esc(hw.gpu||"Не обнаружена")+'</span>'+
    '<span>'+esc(first.threads||"—")+' CPU-потоков</span>'+
    '<span>ONNX Runtime '+esc(hw.ort_version||"")+'</span></div>';
  const maxFor=family=>Math.max(1,...complete.filter(r=>r.family===family).map(r=>Number(r.fps)));
  const families=BENCHMARK_FAMILIES.map(family=>{
    const max=maxFor(family.id);
    const subset=Object.keys(BENCHMARK_NAMES).filter(name=>
      (name.startsWith("RAVEL")?"RAVEL":"VballNet")===family.id);
    return '<section class="bench-family">'+
      '<div class="bench-family-heading"><h4>'+esc(family.title)+'</h4>'+
      '<small>Шкала FPS общая внутри группы</small></div>'+
      subset.map(name=>{
        const data=rows.filter(r=>r.model===name);
        const cpu=data.find(r=>r.device==="cpu"),gpu=data.find(r=>r.device==="cuda");
        const metric=row=>{
          if(!row)return '<span class="bench-plot-metric">Нет данных</span>';
          if(row.status!=="complete")return '<span class="bench-plot-metric" title="'+esc(row.error||"Ошибка")+'">Ошибка · см. журнал</span>';
          return '<span class="bench-plot-metric">'+benchNumber(row.fps)+' FPS'+
            '<small>'+benchNumber(row.latency_ms,2)+' мс / 9 кадров</small></span>';
        };
        const bar=(row,device)=>{
          const good=row?.status==="complete" && Number(row.fps)>0;
          const width=good?Math.max(1,Math.min(100,100*Number(row.fps)/max)):0;
          return '<div class="bench-plot-row '+device+(!good?' failed':'')+'">'+
            '<span class="bench-plot-label">'+(device==="cuda"?"GPU":"CPU")+'</span>'+
            '<div class="bench-plot-bar" role="img" aria-label="'+esc(
              BENCHMARK_NAMES[name]+", "+(device==="cuda"?"GPU":"CPU")+
              (good?", "+benchNumber(row.fps)+" FPS":", данных нет"))+'">'+
            '<span style="width:'+width.toFixed(1)+'%"></span></div>'+metric(row)+'</div>';
        };
        const ratio=cpu?.status==="complete"&&gpu?.status==="complete"
          ?'<small>CUDA / CPU: '+benchNumber(gpu.fps/cpu.fps,2)+'×</small>'
          :'<small>Оба устройства · последовательные измерения</small>';
        return '<div class="bench-plot-model">'+
          '<div class="bench-plot-title"><span>'+esc(BENCHMARK_NAMES[name])+'</span>'+ratio+'</div>'+
          bar(cpu,"cpu")+bar(gpu,"cuda")+'</div>';
      }).join("")+'</section>';
  }).join("");
  return hardware+'<div class="bench-snapshot-meta">'+
    esc(first.video||"Видео не указано")+' · начало '+benchNumber(first.start_sec||0,1)+' с'+
    ' · '+esc(benchmarkDate(first))+' · кадры SHA-256 '+esc(String(first.input_hash||"").slice(0,12))+
    '</div><div class="bench-chart">'+families+'</div>'+
    '<p class="bench-plot-note">Завершено '+complete.length+' из '+expected+' измерений. '+
    'У всех моделей одинаковые исходные девять кадров и повторения; разрешение входа соответствует архитектуре.</p>';
}
async function loadBenchmarkResults(){
  const summary=$("#benchmark-results"),settings=$("#benchmark-compare-settings");
  const history=$("#benchmark-history");
  if(!state.project){
    summary.innerHTML='<div class="empty">Выберите проект для замера.</div>';
    settings.textContent="Пять моделей · CPU и GPU";
    history.innerHTML='<div class="empty">Нет выбранного проекта.</div>';
    return;
  }
  const project=state.project;
  const payload=await api("/api/benchmark-results?project_id="+encodeURIComponent(project));
  if(state.project!==project)return;
  const rows=(payload.runs||[]).filter(row=>row && row.mode==="video-inference" &&
    Object.hasOwn(BENCHMARK_NAMES,row.model) && row.batch_id);
  // All benchmark DOM trees depend only on persisted measurements, not polling.
  const signature=project+"|"+JSON.stringify(rows.map(row=>
    [row.batch_id,row.id,row.status,row.fps,row.error]));
  if(state.benchmarkResultsSignature===signature)return;
  state.benchmarkResultsSignature=signature;
  const batches=new Map();
  for(const row of rows){
    if(!batches.has(row.batch_id))batches.set(row.batch_id,[]);
    batches.get(row.batch_id).push(row);
  }
  history.innerHTML=[...batches.entries()].reverse().slice(0,10).map(([id,items])=>{
    const completed=items.filter(row=>row.status==="complete");
    const first=items[0];
    const best=completed.length?Math.max(...completed.map(row=>Number(row.fps))):0;
    return '<div class="bench-history-item"><div><strong>'+esc(benchmarkDate(first))+
      ' · '+esc(first.video||"Видео")+'</strong><small>'+completed.length+
      '/'+(first.expected_tests||8)+' результатов · '+esc(first.hardware?.gpu||"GPU не определена")+
      ' · '+esc(String(first.threads))+' потоков</small></div>'+
      '<div class="bench-history-speed"><strong>'+benchNumber(best)+' FPS</strong>'+
      '<small>Максимальное значение в серии</small></div></div>';
  }).join("")||'<div class="empty">Новых серий ещё нет. Старые измерения двух моделей сохранены в истории на диске.</div>';
  if(!batches.size){
    summary.innerHTML='<div class="empty">Нет результатов пяти моделей. Запустите последовательный тест CPU и CUDA.</div>';
    settings.textContent="Нужен один полный набор измерений";
    return;
  }
  const [id,latest]=[...batches.entries()].at(-1);
  settings.textContent="Серия "+id+" · "+latest.filter(r=>r.status==="complete").length+"/"+(latest[0].expected_tests||8)+" замеров";
  summary.innerHTML=renderBenchmarkChart(latest);
}
async function submitTask(endpoint,payload,view="analysis"){
  try{
    const j=await api(endpoint,payload);
    state.jobStatus=j.status;go(view);alertBox("");
    await pollJob();
  }catch(error){alertBox(error.message)}
}
async function assignSelected(remove=false,playerId=state.activePlayerId) {
  if(state.playerAssignBusy)return;
  const ids=Array.from(state.selected);
  if(!ids.length){alertBox("Сначала отметьте хотя бы один Track ID.");return}
  if(ids.length>200){alertBox("За одно сохранение можно привязать до 200 ID.");return}
  const player=state.players.find(p=>p.id===playerId);
  if(!remove&&!player){alertBox("Выберите игрока из верхнего блока.");return}
  const projectId=state.project,video=state.video,cacheId=analysisCacheId();
  state.playerAssignBusy=true;
  updateChosen();
  try {
    await api(remove?"/api/unassign":"/api/assign",{
      video,ids,player_id:player?.id||null,project_id:projectId,
      cache_id:cacheId
    });
    if(projectId!==state.project||video!==state.video)return;
    await loadReport();
    if(remove){
      // Explicitly rejected auto links must not be repainted from stale ReID.
      clearTimeout(playerGroupsPoll);
      ++playerGroupsSequence;
      state.playerGroups=null;
      state.playerGroupsKey="";
    }
    $("#players-action-feedback").textContent=remove
      ?ids.length+" ID откреплено · фотобаза обновляется"
      :ids.length+" ID привязано к «"+player.name+"» · они убраны из списка нераспознанных";
    alertBox("");
  }catch(error){alertBox(error.message)}
  finally{
    state.playerAssignBusy=false;
    renderPlayers();
  }
}
function closeProjectActionDialogs(){
  for(const name of ["#project-rename-dialog","#project-delete-dialog"]){
    const dlg=$(name);
    if(dlg.open)dlg.close();
  }
}
function beginRenameProject(projectId=null){
  const p=projectId?state.projects.find(row=>row.id===projectId):selectedProject();
  if(!p)return;
  $("#project-rename-name").value=p.name;
  $("#project-rename-error").hidden=true;
  $("#project-rename-dialog").dataset.projectId=p.id;
  $("#project-rename-dialog").showModal();
  $("#project-rename-name").focus();
  $("#project-rename-name").select();
}
function beginDeleteProject(projectId=null){
  const p=projectId?state.projects.find(row=>row.id===projectId):selectedProject();
  if(!p)return;
  $("#delete-project-name").textContent=p.name;
  $("#project-delete-error").hidden=true;
  $("#project-delete-dialog").dataset.projectId=p.id;
  $("#project-delete-dialog").showModal();
}
async function renameProject(event){
  event.preventDefault();
  const id=$("#project-rename-dialog").dataset.projectId;
  const name=$("#project-rename-name").value.trim();
  const button=$("#project-rename-confirm");
  const error=$("#project-rename-error");
  button.disabled=true;error.hidden=true;
  try{
    await api("/api/project-update",{id,name});
    closeProjectActionDialogs();
    await loadAll();
  }catch(e){
    error.textContent=e.message;error.hidden=false;
  }finally{button.disabled=false;}
}
async function deleteProject(){
  const id=$("#project-delete-dialog").dataset.projectId;
  const button=$("#project-delete-confirm");
  const error=$("#project-delete-error");
  button.disabled=true;error.hidden=true;
  const originalLabel=button.textContent;
  button.textContent="Останавливаем задачи и удаляем…";
  try{
    await api("/api/project-delete",{id});
    closeProjectActionDialogs();
    const wasActive=state.project===id;
    if(wasActive){state.project="";state.video="";}
    await loadAll();
    if(wasActive)go("analysis");
  }catch(e){
    error.textContent=e.message;error.hidden=false;
  }finally{button.disabled=false;button.textContent=originalLabel;}
}
function openHeaderEntryAction(kind,operation,entryId,label){
  window.VolleyballHeaderMenu?.close();
  if(kind==="project"){
    if(operation==="rename")beginRenameProject(entryId);
    else beginDeleteProject(entryId);
    return;
  }
  const model=window.VolleyballHeaderChain;
  const runId=kind==="inference"?model.parseChoiceId(entryId).runId:entryId;
  const run=state.analysisRuns.find(item=>item.id===runId);
  if(!run||!state.project)return;
  if(kind==="postprocess"&&!run.parent_run_id){
    alertBox("Исходный результат нельзя удалить из CPU-списка. "+
      "Управляйте первичным анализом в соседнем списке.");
    return;
  }
  if(kind==="inference"&&run.parent_run_id){
    alertBox("CPU-версия не относится к первичному анализу.");
    return;
  }
  const dialog=$("#header-entry-action-dialog");
  const deleting=operation==="delete";
  dialog.dataset.projectId=state.project;
  dialog.dataset.runId=runId;
  dialog.dataset.kind=kind;
  dialog.dataset.operation=operation;
  const isPrimary=!run.parent_run_id;
  $("#header-entry-action-title").textContent=(deleting?"Удалить":"Переименовать")+
    (isPrimary?" первичный анализ":" обработку результатов")+(deleting?"?":"");
  $("#header-entry-action-description").textContent=deleting
    ?"Запись «"+label+"» исчезнет из списка. "+
      (isPrimary?"Связанные CPU-версии также исчезнут из цепочки. ":"")+
      "Общие GPU-кэши, видео и уже созданные монтажи сохранятся."
    :"Новое название будет применено только к этой записи, не меняя глобальный профиль и результаты.";
  $("#header-entry-name-field").hidden=deleting;
  const input=$("#header-entry-name");
  input.required=!deleting;
  input.value=run.display_name||(isPrimary?run.profile:model.versionLabel(run));
  $("#header-entry-action-error").hidden=true;
  const confirm=$("#header-entry-action-confirm");
  confirm.textContent=deleting?"Удалить запись":"Сохранить";
  confirm.classList.toggle("project-danger-button",deleting);
  confirm.classList.toggle("primary",!deleting);
  dialog.showModal();
  if(!deleting){input.focus();input.select();}else confirm.focus();
}
async function submitHeaderEntryAction(event){
  event.preventDefault();
  const dialog=$("#header-entry-action-dialog");
  const {projectId,runId,operation,kind}=dialog.dataset;
  const button=$("#header-entry-action-confirm");
  const error=$("#header-entry-action-error");
  if(projectId!==state.project){
    error.textContent="Выбран другой проект. Повторите действие.";
    error.hidden=false;return;
  }
  button.disabled=true;error.hidden=true;
  try{
    await api(operation==="delete"?"/api/analysis-run/delete":"/api/analysis-run/rename",{
      project_id:projectId,run_id:runId,kind,
      ...(operation==="rename"?{name:$("#header-entry-name").value.trim()}:{})
    });
    dialog.close();
    const previous=state.analysisRunId;
    if(operation==="delete"&&previous===runId){
      // The CPU result was selected: retain its original neural run in this
      // project/video rather than jumping to a different latest GPU analysis.
      state.analysisRunId=kind==="postprocess"
        ?state.analysisRuns.find(row=>row.id===runId)?.parent_run_id||""
        :"";
    }
    await loadAnalysisRuns();
    if(state.analysisRunId!==previous){
      await loadReport();
      if($("#view-analysis-results").classList.contains("active")){
        window.VolleyballResultsUI.setCacheSelection(
          analysisCacheMap(),state.analysisRunId,
          selectedAnalysisRun()?.postprocess_settings||null);
        const p=selectedProject();
        if(p)await window.VolleyballResultsUI.setProject(state.project,p.videos,state.video);
      }
    }
  }catch(exc){
    error.textContent=exc.message;error.hidden=false;
  }finally{button.disabled=false;}
}
function updateAnalysisCpuThreads(){
  const backend=$("#analysis-form [name='backend']").value;
  const usesCpu=backend==="onnx-cpu"||backend==="openvino";
  const field=$("#analysis-cpu-field");
  field.hidden=false;
  $("#analysis-threads").disabled=false;
  $("#analysis-cpu-row").classList.remove("gpu-only");
  $("#analysis-cpu-note").textContent=
    "Число CPU-потоков зависит от профиля: GPU RAVEL-013 использует все доступные " +
    "логические CPU (до 128), остальные профили — 80%. Настройку можно изменить. " +
    "Потоки используются и вспомогательными этапами при работе на GPU.";
}
function updateRavelBackendSettings(){
  const player=$("#analysis-form [name='player_backend']").value;
  const ball=$("#analysis-form [name='backend']").value;
  $("#analysis-gpu-resources").hidden=player!=="onnx-cuda"&&ball!=="onnx";
  // Keep the legacy device flag consistent with the actual ONNX provider.
  $("#analysis-form [name='player_device']").value=player==="onnx-cuda"?"GPU":"CPU";
}
function bind() {
  window.VolleyballPlayerPhotoHover?.init($("#view-players"),()=>({
    project:state.project,video:state.video,cache:analysisCacheId()
  }));
  $("#chain-project").addEventListener("change",event=>{
    const id=event.target.value;
    if(!id||id===state.project)return;
    for(const selector of ["#chain-inference","#chain-postprocess"])
      $(selector).disabled=true;
    $("#top-project").value=id;
    $("#top-project").dispatchEvent(new Event("change",{bubbles:true}));
  });
  $("#chain-inference").addEventListener("change",async event=>{
    const {video,runId}=window.VolleyballHeaderChain.parseChoiceId(event.target.value);
    if(!video||!runId)return;
    $("#chain-postprocess").disabled=true;
    try{
      if(video!==state.video)await selectSource(video);
      await chooseAnalysisRun(runId);
    }catch(error){
      renderHeaderChain();alertBox(error.message);
    }
  });
  $("#chain-postprocess").addEventListener("change",event=>
    chooseAnalysisRun(event.target.value).catch(error=>alertBox(error.message)));
  window.addEventListener("volley:select-video",event=>
    selectSource(event.detail?.video).catch(error=>alertBox(error.message)));
  async function submitReprocess(settings, fromPreview=false){
    const run=selectedAnalysisRun();
    const status=document.getElementById("result-reprocess-status");
    const previewButton=$("#result-reprocess-start");
    if(!state.project||!run)throw Error("Сначала выберите результаты анализа");
    if(fromPreview && settings.project_id!==state.project)
      throw Error("Проект изменился, выберите текущий анализ");
    if(!Number.isFinite(settings.touch_threshold)||
       settings.touch_threshold<0||settings.touch_threshold>1)
      throw Error("Порог Touch должен быть от 0 до 1");
    try{
      if(previewButton)previewButton.disabled=true;
      if(status)status.textContent="Запуск повторной обработки…";
      const response=await api("/api/reprocess",{
        project_id:state.project,run_id:run.parent_run_id||run.id,
        mode:settings.mode,touch_threshold:settings.touch_threshold,
        touch_min_interval_sec:settings.touch_min_interval_sec??.35,
        touch_max_reach_ratio:settings.touch_max_reach_ratio??.35,
        track_min_duration:settings.track_min_duration??.5,
        track_max_distance:settings.track_max_distance??200,
        track_player_threshold:settings.track_player_threshold??.35,
        track_ball_threshold:settings.track_ball_threshold??.35,
        track_no_ball_fill:settings.track_no_ball_fill===true,
        beach:settings.beach===true,
        ball_threshold:settings.ball_threshold??null,
        player_threshold:settings.player_threshold??null
      });
      if(fromPreview){
        pendingPreviewReplay={
          project:state.project,video:state.video,
          selectedRun:state.analysisRunId,parent:run.parent_run_id||run.id,
          mode:settings.mode,touch:settings.touch_threshold,
          ball:settings.ball_threshold??null,player:settings.player_threshold??null
        };
      }
      state.jobStatus=response.status;
      await pollJob();
    }finally{
      updateReprocessControls();
      if(previewButton)previewButton.disabled=
        (state.latestTask?.kind==="reprocess"&&state.latestTask?.status==="running")||!selectedAnalysisRun();
    }
  }
  $("#sidebar-reprocess-cancel").addEventListener("click",()=>
    api("/api/cancel",{}).then(pollJob).catch(e=>alertBox(e.message)));
  window.addEventListener("volley:reprocess-request",async event=>{
    try{
      await submitReprocess(event.detail||{},true);
    }catch(error){
      $("#result-reprocess-status").textContent=error.message;
      alertBox(error.message);
    }
  });
  const backend=$("#analysis-form [name='backend']");
  backend.addEventListener("change",updateAnalysisCpuThreads);
  backend.addEventListener("change",updateRavelBackendSettings);
  $("#analysis-form [name='player_backend']").addEventListener("change",updateRavelBackendSettings);
  updateAnalysisCpuThreads();
  updateRavelBackendSettings();
  window.VolleyballHeaderMenu?.init({
    onSelect:(kind,id)=>{
      const selector={project:"#chain-project",inference:"#chain-inference",
                      postprocess:"#chain-postprocess"}[kind];
      const native=$(selector);
      native.value=id;
      native.dispatchEvent(new Event("change",{bubbles:true}));
    },
    onAction:(kind,operation,id,label)=>openHeaderEntryAction(kind,operation,id,label),
  });
  $("#project-rename-form").addEventListener("submit",renameProject);
  $("#project-delete-confirm").addEventListener("click",deleteProject);
  $$("[data-close-project-action]").forEach(el=>
    el.addEventListener("click",closeProjectActionDialogs));
  $("#header-entry-action-form").addEventListener("submit",submitHeaderEntryAction);
  $("#header-entry-action-cancel").addEventListener("click",()=>
    $("#header-entry-action-dialog").close());
  $("#top-project").addEventListener("change",async e=>{
    // Capture the ID before awaiting autosave; later changes must win.
    const targetId=e.target.value;
    if(!state.projects.some(p=>p.id===targetId)||targetId===state.project)return;
    const epoch=++projectLoadEpoch;
    const finish=beginWorkspaceLoading("Переключение проекта…");
    try{
      await window.VolleyballAutoSave.flush();
      if(epoch!==projectLoadEpoch)return;
      await loadAll(targetId,epoch);
    }catch(error){
      if(epoch===projectLoadEpoch){
        renderSources(); // Restore the actual project if autosave failed.
        alertBox("Не удалось переключить проект: "+error.message);
      }
    }finally{finish();}
  });
  $("#video-preparation-retry").addEventListener("click",async()=>{
    if(!state.project)return;
    const button=$("#video-preparation-retry");
    button.disabled=true;
    try{
      await api("/api/project-preparation/retry",{project_id:state.project});
      await refreshVideoPreparation();
    }catch(error){alertBox(error.message)}
    finally{button.disabled=false}
  });
  const projectDropzone=$("#project-video-dropzone");
  projectDropzone.addEventListener("click",chooseProjectSourcePaths);
  $("#project-video-file-input").addEventListener("change",event=>{
    acceptProjectFiles(event.target.files);
    event.target.value="";
  });
  projectDropzone.addEventListener("dragenter",event=>{
    event.preventDefault();projectDropzone.classList.add("is-dragover");
  });
  projectDropzone.addEventListener("dragover",event=>{
    event.preventDefault();event.dataTransfer.dropEffect="copy";
    projectDropzone.classList.add("is-dragover");
  });
  projectDropzone.addEventListener("dragleave",event=>{
    if(!projectDropzone.contains(event.relatedTarget))
      projectDropzone.classList.remove("is-dragover");
  });
  projectDropzone.addEventListener("drop",event=>{
    event.preventDefault();projectDropzone.classList.remove("is-dragover");
    acceptProjectFiles(event.dataTransfer?.files);
  });
  $("#new-project-video-options").addEventListener("change",event=>{
    if(event.target.matches(".dialog-source-check"))updateDialogSelection();
  });
  $("#create-project").addEventListener("click",()=>openProjectDialog(false));
  $$("[data-create-project]").forEach(button=>button.addEventListener("click",()=>openProjectDialog(false)));
  $("#new-project-form").addEventListener("submit",event=>{event.preventDefault();saveProjectDialog()});
  $("#cancel-create-project").addEventListener("click",closeProjectDialog);
  $("#close-project-dialog").addEventListener("click",closeProjectDialog);
  $("#project-dialog").addEventListener("cancel",event=>{
    event.preventDefault();
    closeProjectDialog();
  });
  $$(".nav-button").forEach(b=>b.addEventListener("click",()=>go(b.dataset.view)));
  $$("[data-task-view]").forEach(b=>b.addEventListener("click",()=>go(b.dataset.taskView)));
  $$("[data-go]").forEach(b=>b.addEventListener("click",()=>go(b.dataset.go)));
  $("#project-source-list").addEventListener("click",e=>{
    const button=e.target.closest("[data-source]");
    if(button)selectSource(button.dataset.source).catch(err=>alertBox(err.message));
  });
  $("#players-video").addEventListener("change",e=>
    selectSource(e.target.value).catch(err=>alertBox(err.message)));
  $("#analysis-context-summary").addEventListener("click",e=>{
    const button=e.target.closest("[data-reset-context]");
    if(button)resetVideoContext(button.dataset.resetContext).catch(err=>console.warn(err));
  });
  const typeDescriptions={
    ball:"Настройки обнаружения мяча VballNet для полного анализа проекта.",
    players:"Настройки модели RAVEL и отслеживания игроков.",
    performance:"Параметры использования вычислительных ресурсов для нейросетей.",
    outputs:"Дополнительные файлы с обнаружениями нейросетей."
  };
  function selectAnalysisStage(id){
    $$(".analysis-stage-tab").forEach(button=>{
      const active=button.dataset.analysisTab===id;
      button.setAttribute("aria-selected",String(active));
      button.tabIndex=active?0:-1;
      const section=document.getElementById(button.dataset.analysisTab);
      if(section)section.open=active;
    });
    const type=$('[data-analysis-tab="'+id+'"]')?.closest(".analysis-stage-nav-row")
      ?.querySelector("[data-analysis-run]")?.dataset.analysisRun;
    $("#analysis-stage-run-hint").textContent=typeDescriptions[type]||"";
    // Each tab shares one fixed-height viewport; do not retain the old tab's scroll.
    const settingsList=$(".analysis-stage-content-panel .analysis-stage-list");
    if(settingsList)settingsList.scrollTop=0;
  }
  $$(".analysis-stage-tab").forEach(button=>{
    button.addEventListener("click",()=>selectAnalysisStage(button.dataset.analysisTab));
    button.addEventListener("keydown",event=>{
      if(!["ArrowLeft","ArrowRight","Home","End"].includes(event.key))return;
      event.preventDefault();
      const tabs=$$(".analysis-stage-tab");
      const i=tabs.indexOf(button);
      const index=event.key==="Home"?0:event.key==="End"?tabs.length-1:
        (i+(event.key==="ArrowRight"?1:-1)+tabs.length)%tabs.length;
      selectAnalysisStage(tabs[index].dataset.analysisTab);
      tabs[index].focus();
    });
  });
  selectAnalysisStage("analysis-stage-ball");
  async function launchAnalysis(){
    if(!state.project)throw Error("Сначала выберите проект");
    const analysisForm=$("#analysis-form");
    const validation=window.VolleyballAnalysisPresets?.validate?.({focus:true});
    if(validation&&!validation.valid)throw Error(validation.message);
    if(!analysisForm.reportValidity())throw Error("Проверьте выделенное поле анализа");
    const params=Object.fromEntries(new FormData(analysisForm).entries());
    analysisForm.querySelectorAll("[data-advanced-check]").forEach(el=>{
      params[el.name]=el.checked;
    });
    params.project_id=state.project;
    params.profile_name=window.VolleyballAnalysisPresets?.currentName?.()||"Новый";
    params.profile_id=window.VolleyballAnalysisPresets?.currentId?.()||"new";
    state.activeAnalysisProfile=params.profile_name;
    Object.assign(params,window.VolleyballAnalysisUI.params());
    await window.VolleyballAutoSave.flush();
    // All tabs configure the same immutable primary neural inference.
    // CPU tracking and event processing start only in Preview.
    await submitTask("/api/analyze",params,"analysis");
    const panel=$("#analysis-job-panel");
    if(!panel.hidden)panel.scrollIntoView({behavior:"smooth",block:"start"});
  }
  $("#analysis-form").addEventListener("submit",async event=>{
    event.preventDefault();
    try{await launchAnalysis()}
    catch(error){alertBox(error.message)}
  });
  $("#benchmark-form").addEventListener("submit",async e=>{
    e.preventDefault();
    if(!state.project)return alertBox("Сначала выберите проект");
    if(!state.benchmarkReady)return alertBox("Для теста нужны две установленные модели");
    const form=Object.fromEntries(new FormData(e.target).entries());
    form.project_id=state.project;
    delete form.model; // The server always runs both, even if an old client submits a model.
    try{
      const job=await api("/api/benchmark",form);
      state.jobStatus=job.status;
      state.benchmarkRenderedId=null;
      $("#benchmark-results").innerHTML=
        '<div class="empty">Этап 1/2: VballNetV4c → Этап 2/2: VballNetFastV1. Два FPS появятся после завершения всего теста.</div>';
      $("#benchmark-compare-settings").textContent="Сравнение выполняется...";
      $("#benchmark-submit").disabled=true;
      go("benchmark");
      alertBox("");
      await pollJob();
    }catch(error){alertBox(error.message)}
  });
  $("#benchmark-cancel").addEventListener("click",()=>api("/api/cancel",{}).then(pollJob).catch(err=>alertBox(err.message)));
  $("#montage-audio-choose").addEventListener("click",chooseMontageAudio);
  $("#montage-audio-clear").addEventListener("click",()=>{
    $("#montage-audio-path").value="";
    $("#montage-audio-status").textContent="Без фоновой музыки.";
    scheduleMontagePlan();
  });
  $("#montage-follow-ball").addEventListener("change",()=>{
    syncMontageMode();scheduleMontagePlan();
  });
  $("#montage-selection-mode").addEventListener("change",()=>{
    syncMontageSelectionMode();scheduleMontagePlan();
  });
  $("#montage-plan-list").addEventListener("click",event=>{
    if(event.target.closest("[data-montage-rally]"))return;
    const card=event.target.closest("[data-montage-preview]");
    if(card)previewMontageRally(card.dataset.montagePreview);
  });
  $("#montage-plan-list").addEventListener("keydown",event=>{
    if(event.target.closest("[data-montage-rally]"))return;
    const card=event.target.closest("[data-montage-preview]");
    if(!card||event.code!=="Enter")return;
    event.preventDefault();
    event.stopPropagation();
    previewMontageRally(card.dataset.montagePreview);
  });
  $("#montage-plan-list").addEventListener("change",event=>{
    const input=event.target.closest("[data-montage-rally]");
    if(!input||$("#montage-selection-mode").value!=="manual")return;
    if(input.checked)montageManualSelected.add(input.dataset.montageRally);
    else montageManualSelected.delete(input.dataset.montageRally);
    updateManualRallies();
  });
  $("#montage-manual-all").addEventListener("click",()=>{
    montageManualCandidates.forEach(item=>montageManualSelected.add(item.rally_id));
    updateManualRallies();
  });
  $("#montage-manual-none").addEventListener("click",()=>{
    montageManualSelected.clear();updateManualRallies();
  });
  $("#montage-source-selection").addEventListener("click",event=>{
    if(event.target.closest("[data-montage-go-preview]"))go("analysis-results");
  });
  $("#montage-plan-prev").addEventListener("click",()=>{
    $("#montage-plan-list").scrollBy({left:-550,behavior:"smooth"});
  });
  $("#montage-plan-next").addEventListener("click",()=>{
    $("#montage-plan-list").scrollBy({left:550,behavior:"smooth"});
  });
  syncMontageSelectionMode();
  $("#highlights-form").addEventListener("input",scheduleMontagePlan);
  $("#highlights-form").addEventListener("change",scheduleMontagePlan);
  window.addEventListener("volley:rally-config-change",scheduleMontagePlan);
  window.addEventListener("volley:montage-rallies-change",event=>{
    if(event.detail?.project_id!==state.project||
       event.detail?.run_id!==state.analysisRunId)return;
    montageManualSelected.clear();
    for(const id of event.detail.rally_ids||[])montageManualSelected.add(id);
    // Checking a rally in Preview opts into manual selection without
    // changing an explicit automatic-mode choice made in the Montage tab.
    if(montageManualSelected.size&&
       document.documentElement.dataset.initialView==="analysis-results"){
      $("#montage-selection-mode").value="manual";
      syncMontageSelectionMode();
    }
    if(montageCurrentPlan)updateManualRallies();
  });
  $("#montage-refresh").addEventListener("click",()=>loadRenders().catch(error=>alertBox(error.message)));
  $("#highlights-form").addEventListener("submit",async e=>{
    e.preventDefault();
    if(!state.project)return alertBox("Select project");
    let params=montagePayload();
    if(!params.run_id)return alertBox("Выберите запуск анализа");
    if(montagePlanReadyKey!==montageKey(params)){
      if(!await refreshMontagePlan())return alertBox("Не удалось подготовить список розыгрышей");
      params=montagePayload();
    }
    if(!montageChosenRows(params).length){
      return alertBox(params.rally_selection_mode==="manual"
        ?"Отметьте хотя бы один розыгрыш для монтажа"
        :"Автоматический отбор не нашёл подходящих розыгрышей");
    }
    try{
      await submitTask("/api/highlights",params,"results");
      const panel=$("#montage-job-panel");
      if(!panel.hidden)panel.scrollIntoView({behavior:"smooth",block:"start"});
    }catch(error){alertBox(error.message)}
  });
  $("#cancel-job").addEventListener("click",()=>api("/api/cancel",{}).then(pollJob).catch(e=>alertBox(e.message)));
  $("#montage-cancel").addEventListener("click",()=>api("/api/cancel",{}).then(pollJob).catch(e=>alertBox(e.message)));
  $$("[data-player-mode]").forEach(button=>button.addEventListener("click",()=>{
    setPlayerMode(button.dataset.playerMode);
  }));
  $("#bulk-player-target").addEventListener("change",updateChosen);
  $("#save-selected-ids").addEventListener("click",()=>assignSpecificTracks(
    Array.from(state.selected),$("#bulk-player-target").value));
  $("#refresh-player-album").addEventListener("click",()=>renderPlayerAlbum(true).catch(console.warn));
  $("#player-album-gallery").addEventListener("click",event=>{
    const cancel=event.target.closest("[data-cancel-album-delete]");
    if(cancel){
      cancelPlayerAlbumDelete(cancel.closest(".player-album-actions")
        ?.querySelector("[data-remove-album-photo]"));
      return;
    }
    const button=event.target.closest("[data-remove-album-photo]");
    if(button)requestPlayerAlbumDelete(button);
  });
  const thresholdRange=$("#player-reid-threshold");
  const thresholdNumber=$("#player-reid-threshold-number");
  function showThresholdValue(value){
    const candidate=Number(value);
    if(!Number.isInteger(candidate)||candidate<55||candidate>99)return false;
    thresholdRange.value=String(candidate);
    thresholdNumber.value=String(candidate);
    $("#player-reid-threshold-value").textContent=candidate+"%";
    return true;
  }
  async function updateReidThreshold(value){
    if(!showThresholdValue(value)){
      $("#players-action-feedback").textContent="Порог сходства должен быть от 55 до 99%.";
      showThresholdValue(state.playerGroups?.threshold_percent??85);
      renderPlayerGroups();return;
    }
    const threshold_percent=Number(value),context=playerGroupsContext();
    if(!state.project||!state.video||!analysisCacheId())return;
    if(state.playerGroups?.status!=="ready"||playerReidThresholdSaving){
      state.playerReidThresholdPending={context,value:threshold_percent};
      $("#players-action-feedback").textContent="Порог "+threshold_percent+
        "% будет применён после текущего пересчёта ReID.";
      renderPlayerGroups();
      return;
    }
    state.playerReidThresholdPending=null;
    if(Number(state.playerGroups.threshold_percent)===threshold_percent)return;
    playerReidThresholdSaving=true;
    state.playerReidThresholdPending={context,value:threshold_percent};
    thresholdRange.disabled=true;
    thresholdNumber.disabled=true;
    try{
      const [project_id,video,cache_id]=context.split("\u0000");
      const reply=await api("/api/player-groups/threshold",{
        project_id,video,cache_id,threshold_percent});
      if(context!==playerGroupsContext())return;
      state.playerGroups=reply;
      state.playerReidThresholdPending=null;
      renderPlayerGroups();
      await loadPlayerGroups();
    }catch(error){
      if(context===playerGroupsContext()){
        state.playerReidThresholdPending=null;
        showThresholdValue(state.playerGroups?.threshold_percent??85);
        $("#players-action-feedback").textContent="Не удалось сохранить порог ReID: "+error.message;
      }
    }finally{
      playerReidThresholdSaving=false;
      if(context===playerGroupsContext())renderPlayerGroups();
    }
  }
  commitPlayerReidThreshold=updateReidThreshold;
  thresholdRange.addEventListener("input",event=>showThresholdValue(event.target.value));
  thresholdNumber.addEventListener("input",event=>{
    const value=Number(event.target.value);
    if(Number.isInteger(value)&&value>=55&&value<=99){
      thresholdRange.value=String(value);
      $("#player-reid-threshold-value").textContent=value+"%";
    }
  });
  thresholdRange.addEventListener("change",event=>updateReidThreshold(event.target.value));
  thresholdNumber.addEventListener("change",event=>updateReidThreshold(event.target.value));
  const autoEnable=$("#player-auto-assign-enabled");
  const autoThreshold=$("#player-auto-assign-threshold");
  let autoPolicyRequest=false;
  async function saveAutoPolicy(){
    const threshold=Number(autoThreshold.value);
    if(autoPolicyRequest||!state.project||!state.video||!analysisCacheId())return;
    if(!Number.isInteger(threshold)||threshold<55||threshold>99){
      $("#players-action-feedback").textContent="Порог автопривязки: от 55 до 99%.";
      renderPlayerGroups();return;
    }
    const context=playerGroupsContext();
    autoPolicyRequest=true;
    autoEnable.disabled=true;
    autoThreshold.disabled=true;
    try{
      const [project_id,video,cache_id]=context.split("\u0000");
      state.playerGroups=await api("/api/player-groups/auto-assign",{
        project_id,video,cache_id,threshold_percent:threshold,enabled:autoEnable.checked});
      if(context!==playerGroupsContext())return;
      renderPlayerGroups();
      loadPlayerGroups().catch(console.warn);
    }catch(error){
      if(context===playerGroupsContext()){
        $("#players-action-feedback").textContent="Не удалось настроить автопривязку: "+error.message;
        renderPlayerGroups();
      }
    }finally{
      autoPolicyRequest=false;
      if(context===playerGroupsContext())renderPlayerGroups();
    }
  }
  autoEnable.addEventListener("change",saveAutoPolicy);
  autoThreshold.addEventListener("change",saveAutoPolicy);
  $("#player-groups-rebuild").addEventListener("click",async()=>{
    if(!state.project||!state.video||!analysisCacheId())return;
    if(state.playerGroups?.groups?.length&&!window.confirm(
      "Пересчитать группы с нуля? Ручные исправления групп будут сброшены, привязки к игрокам сохранятся."))return;
    const button=$("#player-groups-rebuild");
    button.disabled=true;
    try{
      state.playerGroups=await api("/api/player-groups/start",{
        project_id:state.project,video:state.video,cache_id:analysisCacheId()});
      renderPlayerGroups();
      loadPlayerGroups().catch(console.warn);
    }catch(error){alertBox(error.message)}
    finally{button.disabled=false}
  });
  $("#player-groups-list").addEventListener("change",event=>{
    if(event.target.matches(".group-player-picker"))rememberPlayerPickerChange(event.target);
  });
  $("#player-groups-list").addEventListener("click",async event=>{
    const button=event.target.closest("[data-group-link]");
    if(!button)return;
    const picker=button.closest(".player-appearance-card")?.querySelector(".group-player-picker");
    const group=(state.playerGroups?.groups||[]).find(g=>g.id===button.dataset.groupLink);
    if(!group)return;
    button.disabled=true;
    try{await assignSpecificTracks(group.track_ids,picker?.value,
      {card:button.closest(".player-appearance-card")})}
    finally{button.disabled=false}
  });
  let trackSearchTimer=null;
  $("#track-search").addEventListener("input",()=>{
    clearTimeout(trackSearchTimer);
    trackSearchTimer=setTimeout(()=>{
      state.limit=30;renderPlayers();
    },120);
  });
  $("#players-filter").addEventListener("change",event=>{
    state.playerFilter=event.target.value;state.limit=30;renderPlayers();
  });
  $("#add-library-player").addEventListener("click",async()=>{
    const player_id=$("#library-player-select").value;
    if(!player_id)return;
    try{
      const reply=await api("/api/players/attach",{player_id,project_id:state.project});
      state.players=reply.players;
      state.activePlayerId=player_id;
      renderPlayers();
      if(state.playerMode==="view")renderPlayerAlbum().catch(console.warn);
      loadPlayerGroups().catch(console.warn);
      $("#players-action-feedback").textContent="Игрок добавлен в состав этого проекта.";
      alertBox("");
    }catch(error){alertBox(error.message)}
  });
  $("#create-player-form").addEventListener("submit",async event=>{
    event.preventDefault();
    const name=$("#new-player-name").value.trim();
    if(!name)return;
    try{
      const result=await api("/api/players/create",{project_id:state.project,name});
      state.players=result.players;
      state.libraryPlayers=await api("/api/players/library").then(x=>x.players||[]);
      const created=state.players.find(p=>p.name===name);
      state.activePlayerId=created?.id||null;
      $("#new-player-name").value="";
      $("#players-action-feedback").textContent="Игрок «"+name+"» создан. Теперь выберите его Track ID.";
      renderPlayers();
      if(state.playerMode==="view")renderPlayerAlbum().catch(console.warn);
      loadPlayerGroups().catch(console.warn);alertBox("");
    }catch(error){alertBox(error.message)}
  });
  $("#named-list").addEventListener("click",event=>{
    const remove=event.target.closest("[data-remove-project-player]");
    if(remove){
      state.activePlayerId=remove.dataset.removeProjectPlayer;
      $("#delete-player").click();
      return;
    }
    const button=event.target.closest("[data-player-id]");
    if(!button||state.playerAssignBusy)return;
    state.activePlayerId=button.dataset.playerId;
    $("#player-manage").open=false;
    renderPlayers();
    if(state.playerMode==="view")renderPlayerAlbum().catch(console.warn);
  });
  $("#rename-player-form").addEventListener("submit",async event=>{
    event.preventDefault();
    const name=$("#rename-player-name").value.trim();
    try{
      await api("/api/players/rename",{project_id:state.project,player_id:state.activePlayerId,name});
      await loadReport();
      $("#players-action-feedback").textContent="Имя игрока изменено.";
      alertBox("");
    }catch(error){alertBox(error.message)}
  });
  $("#backfill-player-photos").addEventListener("click",async()=>{
    const player_id=state.activePlayerId;
    if(!player_id)return;
    const button=$("#backfill-player-photos");
    button.disabled=true;
    try{
      await api("/api/players/photos/collect",{player_id});
      $("#players-action-feedback").textContent="Сбор фотографий запущен. Они появятся в просмотре после сохранения.";
      setTimeout(async()=>{
        if(player_id!==state.activePlayerId)return;
        try{
          const counts=await api("/api/players/photos");
          state.photoCounts=counts.counts||state.photoCounts;
          renderPlayers();
          if(state.playerMode==="view")await renderPlayerAlbum(true);
        }catch(error){console.warn("album refresh",error)}
      },2500);
      alertBox("");
    }catch(error){alertBox(error.message)}
    finally{button.disabled=false}
  });
  $("#delete-player").addEventListener("click",async()=>{
    const player=state.players.find(p=>p.id===state.activePlayerId);
    if(!player||!window.confirm("Убрать «"+player.name+"» из этого проекта? В общей базе игрок и его фото останутся."))return;
    try{
      await api("/api/players/delete",{project_id:state.project,player_id:player.id});
      state.activePlayerId=null;
      await loadReport();
      $("#players-action-feedback").textContent="Игрок убран из состава. Его карточка сохранена в общей базе.";
      alertBox("");
    }catch(error){alertBox(error.message)}
  });
  $("#player-suggestions").addEventListener("click",event=>{
    const button=event.target.closest("[data-compare-id]");
    if(!button)return;
    if(state.previewId)state.selected.add(String(state.previewId));
    state.selected.add(button.dataset.compareId);
    renderPlayers();
    renderPlayerPhotos().catch(console.warn);
  });
  $("#show-more").addEventListener("click",()=>{state.limit+=100;renderPlayers()});
  $("#raw-players").addEventListener("change",e=>{
    if(e.target.matches(".track-player-picker")){
      rememberPlayerPickerChange(e.target);
      return;
    }
    if(!e.target.classList.contains("row-select"))return;
    const id=e.target.dataset.id;
    if(e.target.checked){
      if(state.selected.size>=200){
        e.target.checked=false;alertBox("За раз можно выбрать до 200 ID.");return;
      }
      state.selected.add(id);
    }else state.selected.delete(id);
    syncTrackSelection();
    schedulePlayerPhotos();
  });
  $("#raw-players").addEventListener("click",e=>{
    const attach=e.target.closest("[data-track-link]");
    if(attach){
      const picker=attach.closest("tr")?.querySelector(".track-player-picker");
      assignSpecificTracks([attach.dataset.trackLink],picker?.value).catch(console.warn);
      return;
    }
    const candidateButton=e.target.closest("[data-select-candidate]");
    if(candidateButton){
      const group=candidateButton.dataset.selectCandidate;
      const match=(state.playerGroups?.groups||[]).find(item=>item.id===group);
      const ids=match?match.track_ids.map(String):(state.report?.raw||[]).filter(row=>
        row.candidate_group_id===group).map(row=>String(row.player_track_id));
      for(const id of ids){
        if(state.selected.size>=200)break;
        state.selected.add(id);
      }
      syncTrackSelection();
      schedulePlayerPhotos();
      return;
    }
    const button=e.target.closest(".photo-peek");
    if(!button)return;
    state.previewId=button.dataset.id;
    renderPlayerPhotos().catch(err=>console.warn("photos",err));
    loadPlayerSuggestions().catch(console.warn);
  });
  $("#select-all-ids").addEventListener("change",e=>{
    const checks=$$(".row-select");
    if(e.target.checked){
      for(const check of checks){
        if(state.selected.size>=200)break;
        state.selected.add(check.dataset.id);
      }
    }else checks.forEach(check=>state.selected.delete(check.dataset.id));
    syncTrackSelection();
    schedulePlayerPhotos();
  });
  $("#select-filtered").addEventListener("click",()=>{
    const matches=playerFilterSet(state.report?.raw||[],associatedMap());
    for(const row of matches){
      if(state.selected.size>=200)break;
      state.selected.add(String(row.player_track_id));
    }
    syncTrackSelection();
    schedulePlayerPhotos();
  });
  $("#clear-selected").addEventListener("click",()=>{
    state.selected.clear();
    syncTrackSelection();
    schedulePlayerPhotos();
  });
  $("#unassign-ids").addEventListener("click",()=>assignSelected(true));
  $("#all-renders").addEventListener("click",async e=>{
    const button=e.target.closest("button");
    if(!button)return;
    const item=state.renders.find(f=>f.id===button.dataset.id);
    if(!item)return;
    if(button.classList.contains("render-file")){
      go("results");
      previewMontageFile(item);
      return;
    }
    if(button.classList.contains("montage-folder")){
      try{
        await api("/api/montage/folder",{project_id:state.project,id:item.id});
        $("#montage-preview-status").textContent="Открыта папка: "+item.folder;
      }catch(error){alertBox(error.message)}
    }else if(button.classList.contains("montage-delete")){
      if(!window.confirm("Удалить ролик «"+item.name+"» с диска? Действие необратимо."))return;
      try{
        await api("/api/montage/delete",{project_id:state.project,id:item.id});
        const video=$("#preview-player");
        if($("#preview-title").textContent===(item.title||item.name)){
          montagePreviewToken++;
          video.pause();video.removeAttribute("src");video.load();
          video.style.display="none";$("#preview-empty").style.display="block";
          $("#preview-title").textContent="—";
          $("#montage-preview-status").textContent="Ролик удалён.";
        }
        await loadRenders();
      }catch(error){alertBox(error.message)}
    }
  });
}
document.addEventListener("DOMContentLoaded",async()=>{
  window.VolleyballCourtUI.init();
  window.VolleyballAnalysisUI.init();
  window.VolleyballResultsUI.init();
  bind();
  const requestedView=new URLSearchParams(window.location.search).get("view");
  try{
    await loadAll();
    if(["analysis","analysis-results","players","report","benchmark","results"].includes(requestedView))go(requestedView);
    await pollJob();
  }catch(e){alertBox(e.message)}
  // One adaptive timer instead of two permanent overlapping subscriptions.
  // Idle: 12 s, active: 2.5 s, background: 30 s; prepared videos stop polling.
  let statusTimer=null;
  const scheduleStatus=async()=>{
    try{await pollJob()}catch(error){console.warn("status",error)}
    statusTimer=setTimeout(scheduleStatus,
      document.hidden?30000:state.jobStatus==="running"?2500:12000);
  };
  statusTimer=setTimeout(scheduleStatus,2500);
  document.addEventListener("visibilitychange",()=>{
    if(document.hidden)return;
    clearTimeout(statusTimer);
    scheduleStatus();
    if(state.preparationActive)
      refreshVideoPreparation().catch(console.warn);
  });
});
