"use strict";

const $ = (selector) => document.querySelector(selector);
const $$ = (selector) => Array.from(document.querySelectorAll(selector));
const state = {videos: [], video: "", projects: [], project: "", aggregate: null, report: null, renders: [],
  selected: new Set(), limit: 60, previewId: null, photoToken: 0, jobStatus: "idle",
  infoCache: new Map(), dialogEdit: false, dialogBusy: false};
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
  const data = await response.json();
  if (!response.ok) throw new Error(data.error || "Ошибка запроса");
  return data;
}
function alertBox(message) {
  $("#alerts").textContent = message || "";
  if (message) window.scrollTo({top:0,behavior:"smooth"});
}
function go(view) {
  if(!state.project&&view!=="analysis")view="analysis";
  $$(".nav-button").forEach(b => b.classList.toggle("active",b.dataset.view===view));
  $$(".view").forEach(v => v.classList.toggle("active",v.id==="view-"+view));
  const names={analysis:"Видео и анализ",players:"Игроки",report:"Отчёт проекта",results:"Монтаж"};
  $("#page-title").textContent=names[view] || names.analysis;
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
    : "Нет обработанных записей проекта. Перейдите во вкладку «Видео и анализ».";
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
  return "/api/player-photo?video="+encodeURIComponent(state.video)+"&id="+encodeURIComponent(id)+"&sample="+sample;
}
function updateChosen() {
  $("#selected-ids").textContent=state.selected.size
    ?"Выбраны ID: "+Array.from(state.selected).join(", "):"ID не выбраны";
}
async function renderPlayerPhotos() {
  const gallery=$("#player-photo-gallery");
  const ids=state.selected.size?Array.from(state.selected).slice(0,6):(state.previewId?[state.previewId]:[]);
  const token=++state.photoToken, video=state.video;
  if(!ids.length){gallery.innerHTML='<div class="empty">Нажмите на фото или отметьте ID в таблице</div>';return}
  gallery.innerHTML='<div class="muted">Загрузка фотографий из сохранённых кадров…</div>';
  const cards=await Promise.all(ids.map(async id=>{
    try {
      const meta=await api("/api/player-photos?video="+encodeURIComponent(video)+"&id="+encodeURIComponent(id));
      if(!meta.samples.length)return '<div class="empty">#'+esc(id)+': фото пока отсутствуют</div>';
      return '<div class="photo-person"><div class="photo-person-head"><strong>Track #'+esc(id)+'</strong><span>'+meta.samples.length+' кадр(а)</span></div><div class="photo-strip">'
        +meta.samples.map(s=>'<a href="'+esc(s.url)+'" target="_blank" rel="noopener" title="Увеличить кадр"><img loading="lazy" src="'+esc(s.url)+'" alt="Игрок, track #'+esc(id)+', кадр '+s.frame+'"><small>'+s.time_sec.toFixed(1)+' сек</small></a>').join("")
        +'</div></div>';
    }catch(error){return '<div class="empty">#'+esc(id)+': '+esc(error.message)+'</div>'}
  }));
  if(token!==state.photoToken||video!==state.video)return;
  gallery.innerHTML=cards.join("")+(state.selected.size>6?'<div class="muted">Показаны первые 6 выбранных ID. Остальные останутся выбранными при сохранении.</div>':'');
}
function associatedMap() {
  const map=new Map();
  (state.report?.groups||[]).forEach(g=>g.track_ids.forEach(id=>map.set(String(id),g.name)));
  return map;
}
function renderPlayers() {
  const r=state.report||{}, raw=r.raw||[], groups=r.groups||[];
  const q=$("#track-search").value.trim().toLowerCase();
  const association=associatedMap();
  const matched=raw.filter(p=>String(p.player_track_id).includes(q) || (association.get(String(p.player_track_id))||"").toLowerCase().includes(q));
  const shown=matched.slice(0,state.limit);
  $("#raw-count").textContent=raw.length+" ID";
  $("#raw-players").innerHTML=shown.map(p=>{
    const id=String(p.player_track_id);
    return '<tr><td><input type="checkbox" class="row-select" data-id="'+esc(id)+'" '+(state.selected.has(id)?"checked":"")+'></td><td><button type="button" class="photo-peek" data-id="'+esc(id)+'" title="Показать фото #'+esc(id)+'"><img loading="lazy" src="'+esc(playerPhotoUrl(id))+'" alt="Фото track ID '+esc(id)+'" onerror="this.style.opacity=0.15"></button></td><td><strong>#'+esc(id)+'</strong></td><td>'+(p.attack||0)+'</td><td>'+(p.dig||0)+'</td><td>'+(p.overhead_pass||0)+'</td><td>'+(association.has(id)?'<span class="tag">'+esc(association.get(id))+'</span>':'<span class="muted">—</span>')+'</td></tr>';
  }).join("") || '<tr><td colspan="7" class="empty">Нет совпадений</td></tr>';
  $("#table-hint").textContent="Показано "+shown.length+" из "+matched.length+" ID";
  $("#show-more").disabled=state.limit>=matched.length;
  updateChosen();
  $("#groups-count").textContent=groups.length+" игроков";
  $("#named-list").innerHTML=groups.map(g=>
    '<div class="person-card"><div class="person-line"><strong>'+esc(g.name)+'</strong><span class="person-count">'+g.total_actions+'</span></div><small>'+esc(g.track_ids.join(", "))+'</small></div>'
  ).join("")||'<div class="empty">Выберите ID и присвойте имя</div>';
  $("#known-names").innerHTML=groups.map(g=>'<option value="'+esc(g.name)+'"></option>').join("");
}
function renderRenders() {
  $("#all-renders").innerHTML=renderFiles(state.renders);
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
  return p?.ranges?.[name]||{start:0,end:state.infoCache.get(name)?.duration_sec||0};
}
async function getVideoInfo(name) {
  if(state.infoCache.has(name))return state.infoCache.get(name);
  const meta=await api("/api/video-info?video="+encodeURIComponent(name));
  state.infoCache.set(name,meta);
  return meta;
}
function populatePlayersSelect() {
  const select=$("#players-video"),p=selectedProject();
  if(!p)return;
  select.innerHTML=p.videos.map(name=>'<option value="'+esc(name)+'">'+esc(name)+'</option>').join("");
  if(p.videos.includes(state.video))select.value=state.video;
}
function renderSources() {
  const p=selectedProject();
  $("#top-project").innerHTML='<option value="">Выберите проект</option>'+
    state.projects.map(p=>'<option value="'+esc(p.id)+'">'+esc(p.name)+'</option>').join("");
  $("#top-project").value=state.project||"";
  $("#no-project").hidden=!!p;
  $("#project-workspace").hidden=!p;
  for(const btn of $$(".nav-button"))btn.disabled=!p&&btn.dataset.view!=="analysis";
  if(!p) {
    $("#active-project-name").textContent="Видео и анализ";
    $("#active-project-description").textContent="Нажмите «Создать проект», выберите записи и задайте индивидуальные диапазоны анализа.";
    return;
  }
  $("#active-project-name").textContent=p.name;
  $("#active-project-description").textContent="Выберите запись для предпросмотра и настройки диапазона. Анализ запускается сразу для всех видео проекта.";
  $("#source-count").textContent="· "+p.videos.length;
  $("#current-source-name").textContent=state.video||"";
  $("#project-source-list").innerHTML=p.videos.map(name=>{
    const analyzed=p.analyses?.find(i=>i.video===name)?.analyzed;
    const range=projectRange(name);
    const duration=range.end>0?clock(range.start)+" — "+clock(range.end):"Весь файл";
    return '<button type="button" class="project-source '+(state.video===name?"selected":"")+
      '" data-source="'+esc(name)+'"><span class="source-name">'+esc(name)+'</span>'+
      '<span class="source-range">'+esc(duration)+'</span>'+
      '<span class="source-status '+(analyzed?"done":"pending")+'">'+(analyzed?"Анализ готов":"Ожидает анализа")+'</span></button>';
  }).join("");
  $("#analysis-project-hint").textContent=p.videos.length+" видео в проекте. Диапазоны сохраняются отдельно для каждой записи; запуск обработает все выбранные источники.";
  $("#highlights-project-hint").textContent="Монтаж из "+p.videos.length+" видео проекта; полный выбранный розыгрыш не обрезается ради длительности.";
  populatePlayersSelect();
}
async function loadProjects() {
  state.projects=(await api("/api/projects")).projects;
  if(!state.projects.some(p=>p.id===state.project))state.project=state.projects[0]?.id||"";
  const p=selectedProject();
  if(!p?.videos?.includes(state.video))state.video=p?.videos?.[0]||"";
  renderSources();
}
async function selectSource(name) {
  if(!selectedProject()?.videos.includes(name))return;
  state.video=name;
  renderSources();
  populatePlayersSelect();
  await Promise.all([loadReport(),window.VolleyballAnalysisUI.setVideo(name)]);
}
function updateDialogSelection() {
  const boxes=$$(".dialog-source-check");
  const picked=boxes.filter(b=>b.checked).length;
  $("#project-selected-count").textContent=picked+" видео выбрано";
  for(const cb of boxes) {
    const row=cb.closest(".dialog-video-row");
    for(const input of row.querySelectorAll(".dialog-range-input"))input.disabled=!cb.checked;
    row.classList.toggle("is-selected",cb.checked);
  }
}
async function fillDialogVideoMeta(name) {
  const row=$$(".dialog-video-row").find(x=>x.dataset.video===name);
  if(!row)return;
  try {
    const meta=await getVideoInfo(name);
    if(!row.isConnected)return;
    row.querySelector(".dialog-duration").textContent=clock(meta.duration_sec)+" · "+meta.width+"×"+meta.height;
    const start=row.querySelector(".dialog-start"),end=row.querySelector(".dialog-end");
    start.max=meta.duration_sec;end.max=meta.duration_sec;
    const range=state.dialogEdit?selectedProject()?.ranges?.[name]:null;
    start.value=range?.start??0;
    end.value=range?.end??meta.duration_sec;
    row.dataset.ready="yes";
  }catch(error){
    row.dataset.ready="error";
    row.querySelector(".dialog-duration").textContent="Не удалось определить длительность: "+error.message;
  }
}
async function openProjectDialog(edit=false) {
  if(state.dialogBusy)return;
  state.dialogEdit=edit;
  const p=selectedProject(),dlg=$("#project-dialog");
  $("#project-dialog-error").hidden=true;
  $("#project-dialog-title").textContent=edit?"Видео и название проекта":"Создать проект";
  $("#confirm-project").textContent=edit?"Сохранить изменения":"Создать проект";
  $("#new-project-name").value=edit&&p?p.name:todayProjectName();
  const selected=new Set(edit&&p?p.videos:[]);
  $("#new-project-video-options").innerHTML=state.videos.length?state.videos.map(v=>
    '<div class="dialog-video-row" data-video="'+esc(v.name)+'">'+
      '<label class="dialog-video-heading"><input type="checkbox" class="dialog-source-check" value="'+esc(v.name)+'" '+(selected.has(v.name)?"checked":"")+'><span><strong>'+esc(v.name)+'</strong><small class="dialog-duration">Считываем длительность…</small></span></label>'+
      '<div class="dialog-range-row"><label>Начало, сек. <input type="number" class="dialog-range-input dialog-start" min="0" step="any" value="0"></label>'+
      '<label>Конец, сек. <input type="number" class="dialog-range-input dialog-end" min="1" step="any" value="0"></label>'+
      '<button type="button" class="secondary small dialog-full" data-video="'+esc(v.name)+'">Весь файл</button></div></div>'
  ).join(""):'<p class="muted">В каталоге project/ пока нет видео. Скопируйте файлы туда и нажмите обновление страницы.</p>';
  dlg.showModal();
  $("#new-project-name").focus();
  $("#new-project-name").select();
  updateDialogSelection();
  await Promise.all(state.videos.map(v=>fillDialogVideoMeta(v.name)));
}
function closeProjectDialog() {
  $("#project-dialog").close();
}
async function saveProjectDialog() {
  if(state.dialogBusy)return;
  const p=selectedProject(),name=$("#new-project-name").value.trim();
  const rows=$$(".dialog-video-row").filter(row=>row.querySelector(".dialog-source-check").checked);
  const error=$("#project-dialog-error");
  const fail=msg=>{error.textContent=msg;error.hidden=false;};
  if(!name)return fail("Введите название проекта");
  if(!rows.length)return fail("Выберите хотя бы одно видео");
  const videos=[],ranges={};
  for(const row of rows){
    const filename=row.dataset.video;
    if(row.dataset.ready!=="yes")return fail("Не удалось получить параметры видео: "+filename);
    const start=Number(row.querySelector(".dialog-start").value);
    const end=Number(row.querySelector(".dialog-end").value);
    const max=state.infoCache.get(filename).duration_sec;
    if(!Number.isFinite(start)||!Number.isFinite(end)||start<0||end<=start||
       end-start<1||end>max+.01)return fail("Некорректные границы для "+filename+" (0–"+max+" сек; не менее 1 сек)");
    videos.push(filename);ranges[filename]={start,end:Math.min(max,end)};
  }
  state.dialogBusy=true;$("#confirm-project").disabled=true;error.hidden=true;
  try {
    const payload=state.dialogEdit?{id:p.id,name,videos,ranges}:{name,videos,ranges};
    const result=await api(state.dialogEdit?"/api/project-update":"/api/project-create",payload);
    state.project=result.project.id;
    if(!videos.includes(state.video))state.video=videos[0];
    closeProjectDialog();
    await loadAll();
    go("analysis");
  }catch(err){fail(err.message)}
  finally{state.dialogBusy=false;$("#confirm-project").disabled=false}
}
async function loadAll() {
  const v=await api("/api/videos");
  state.videos=v.videos;
  await loadProjects();
  if(selectedProject()){
    await Promise.all([loadReport(),loadRenders(),window.VolleyballAnalysisUI.setProjectRanges(selectedProject().ranges||{})]);
    if(state.video)await window.VolleyballAnalysisUI.setVideo(state.video);
  }else{
    state.report=null;state.aggregate=null;state.renders=[];
    renderReport();renderRenders();
  }
  alertBox("");
}
async function loadReport() {
  if(!state.project||!state.video)return;
  const projectId=encodeURIComponent(state.project);
  state.report=await api("/api/report?video="+encodeURIComponent(state.video)+"&project_id="+projectId);
  state.aggregate=await api("/api/project-report?id="+projectId);
  state.selected.clear();
  state.previewId=state.report?.raw?.[0]?.player_track_id!=null?String(state.report.raw[0].player_track_id):null;
  renderReport();renderPlayers();populatePlayersSelect();
  // Load photo gallery only on the player page to avoid decoding unused crops.
  if($("#view-players").classList.contains("active"))renderPlayerPhotos().catch(console.warn);
}
async function loadRenders(){
  const p=selectedProject();
  state.renders=p?.highlight_ready?[{name:p.name+" — итоговый ролик",size_mb:"готов",url:"/api/project-media?id="+encodeURIComponent(p.id)}]:[];
  renderRenders();
}
async function saveCurrentRange(){
  const p=selectedProject();
  if(!p)return;
  const range=window.VolleyballAnalysisUI.currentRange();
  if(!range)throw Error("Диапазон не загружен");
  const ranges={...p.ranges,[state.video]:range};
  $("#range-save-status").textContent="Сохраняем…";
  const updated=await api("/api/project-update",{id:p.id,ranges});
  p.ranges=updated.project.ranges;
  $("#range-save-status").textContent="Сохранено · "+clock(range.start)+"–"+clock(range.end);
  renderSources();
}
async function pollJob() {
  const info=await api("/api/job");
  const previous=state.jobStatus;
  state.jobStatus=info.status;
  $("#job-badge").textContent={
    idle:"Нет задачи",running:"Выполняется",complete:"Завершено",failed:"Ошибка",cancelled:"Остановлено"
  }[info.status]||info.status;
  $("#job-head").textContent=info.status==="running"?"Обработка выполняется":info.status==="complete"?"Задача завершена":info.status==="failed"?"Ошибка обработки":info.status==="cancelled"?"Обработка остановлена":"Готово к запуску";
  $("#job-sub").textContent=info.stage || (info.kind==="highlights"?"Сборка лучших моментов":info.kind==="analyze"?"Анализ мяча и игроков":"Выберите видео и параметры.");
  $("#job-percent").textContent=info.progress==null?(info.status==="complete"?"100%":"—"):String(info.progress)+"%";
  $("#job-progress").style.width=String(info.progress==null?(info.status==="complete"?100:0):info.progress)+"%";
  $("#cancel-job").disabled=info.status!=="running";
  $("#job-log").textContent=info.log || "Здесь появятся сообщения pipeline.";
  if(info.status==="running")$("#job-log").scrollTop=$("#job-log").scrollHeight;
  if(previous==="running" && info.status!=="running"){
    await Promise.all([loadAll(),loadRenders()]);
    if(info.status==="failed")alertBox("Обработка завершилась с ошибкой. Посмотрите журнал.");
  }
}
async function submitTask(endpoint,payload){
  try{
    const j=await api(endpoint,payload);
    state.jobStatus=j.status;go("analysis");alertBox("");
    await pollJob();
  }catch(error){alertBox(error.message)}
}
async function assignSelected(remove=false) {
  const ids=Array.from(state.selected);
  if(!ids.length){alertBox("Сначала выберите хотя бы один track ID.");return}
  const name=$("#player-name").value.trim();
  if(!remove && !name){alertBox("Укажите имя игрока.");return}
  try {
    await api(remove?"/api/unassign":"/api/assign",{video:state.video,ids,name,project_id:state.project||null});
    await loadReport();
    state.previewId=ids[0];
    renderPlayerPhotos().catch(e=>console.warn("photos",e));
    $("#player-name").value=name;
  } catch(error){alertBox(error.message)}
}
function bind() {
  $("#top-project").addEventListener("change",async e=>{
    state.project=e.target.value;
    const p=selectedProject();
    state.video=p?.videos[0]||"";
    renderSources();
    await Promise.all([loadReport(),loadRenders(),
      window.VolleyballAnalysisUI.setProjectRanges(p?.ranges||{})]);
    if(state.video)await window.VolleyballAnalysisUI.setVideo(state.video);
  });
  $("#create-project").addEventListener("click",()=>openProjectDialog(false));
  $$("[data-create-project]").forEach(button=>button.addEventListener("click",()=>openProjectDialog(false)));
  $("#edit-project-videos").addEventListener("click",()=>openProjectDialog(true));
  $("#new-project-form").addEventListener("submit",event=>{event.preventDefault();saveProjectDialog()});
  $("#cancel-create-project").addEventListener("click",closeProjectDialog);
  $("#close-project-dialog").addEventListener("click",closeProjectDialog);
  $("#new-project-video-options").addEventListener("change",e=>{
    if(e.target.matches(".dialog-source-check"))updateDialogSelection();
  });
  $("#new-project-video-options").addEventListener("click",e=>{
    const button=e.target.closest(".dialog-full");
    if(!button)return;
    const row=button.closest(".dialog-video-row"),meta=state.infoCache.get(row.dataset.video);
    if(meta){row.querySelector(".dialog-start").value=0;row.querySelector(".dialog-end").value=meta.duration_sec}
  });
  $$(".nav-button").forEach(b=>b.addEventListener("click",()=>go(b.dataset.view)));
  $$("[data-go]").forEach(b=>b.addEventListener("click",()=>go(b.dataset.go)));
  $("#project-source-list").addEventListener("click",e=>{
    const button=e.target.closest("[data-source]");
    if(button)selectSource(button.dataset.source).catch(err=>alertBox(err.message));
  });
  $("#players-video").addEventListener("change",e=>{
    state.video=e.target.value;
    renderSources();
    loadReport().catch(err=>alertBox(err.message));
    window.VolleyballAnalysisUI.setVideo(state.video);
  });
  $("#save-timeline-range").addEventListener("click",()=>saveCurrentRange().catch(err=>alertBox(err.message)));
  $("#refresh").addEventListener("click",()=>loadAll().catch(err=>alertBox(err.message)));
  $("#analysis-form").addEventListener("submit",async e=>{
    e.preventDefault();
    if(!state.project)return alertBox("Сначала создайте проект");
    const params=Object.fromEntries(new FormData(e.target).entries());
    params.beach=e.target.querySelector('[name="beach"]').checked;
    params.project_id=state.project;
    try {
      Object.assign(params,window.VolleyballAnalysisUI.params());
      await saveCurrentRange();
      await submitTask("/api/analyze",params);
    }catch(err){alertBox(err.message)}
  });
  $("#highlights-form").addEventListener("submit",e=>{
    e.preventDefault();
    if(!state.project)return alertBox("Сначала создайте проект");
    submitTask("/api/highlights",{project_id:state.project,duration:Number(e.target.elements.duration.value)});
  });
  $("#cancel-job").addEventListener("click",()=>api("/api/cancel",{}).then(pollJob).catch(e=>alertBox(e.message)));
  $("#track-search").addEventListener("input",()=>{state.limit=60;renderPlayers()});
  $("#show-more").addEventListener("click",()=>{state.limit+=100;renderPlayers()});
  $("#raw-players").addEventListener("change",e=>{
    if(!e.target.classList.contains("row-select"))return;
    const id=e.target.dataset.id;
    if(e.target.checked)state.selected.add(id);else state.selected.delete(id);
    updateChosen();
    renderPlayerPhotos().catch(e=>console.warn("photos",e));
  });
  $("#raw-players").addEventListener("click",e=>{
    const button=e.target.closest(".photo-peek");
    if(!button)return;
    state.previewId=button.dataset.id;
    renderPlayerPhotos().catch(err=>console.warn("photos",err));
  });
  $("#select-all-ids").addEventListener("change",e=>{
    $$(".row-select").forEach(check=>{check.checked=e.target.checked;if(e.target.checked)state.selected.add(check.dataset.id);else state.selected.delete(check.dataset.id)});
    renderPlayers();
    renderPlayerPhotos().catch(e=>console.warn("photos",e));
  });
  $("#assign-ids").addEventListener("click",()=>assignSelected());
  $("#unassign-ids").addEventListener("click",()=>assignSelected(true));
  document.body.addEventListener("click",e=>{
    const btn=e.target.closest(".render-file");
    if(!btn)return;
    const name=btn.dataset.file;
    const player=$("#preview-player");
    player.src=btn.dataset.url||"/media/"+encodeURIComponent(name);
    player.style.display="block";
    $("#preview-empty").style.display="none";
    $("#preview-title").textContent=name;
    go("results");
    player.load();
  });
}
document.addEventListener("DOMContentLoaded",async()=>{
  window.VolleyballAnalysisUI.init();
  bind();
  const requestedView=new URLSearchParams(window.location.search).get("view");
  if(["analysis","players","report","results"].includes(requestedView))go(requestedView);
  try{await loadAll();await pollJob()}catch(e){alertBox(e.message)}
  window.setInterval(()=>pollJob().catch(e=>console.warn("status",e)),2500);
});
