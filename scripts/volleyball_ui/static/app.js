"use strict";

const $ = (selector) => document.querySelector(selector);
const $$ = (selector) => Array.from(document.querySelectorAll(selector));
const state = {videos: [], video: "", projects: [], project: "", aggregate: null, report: null, renders: [], selected: new Set(), limit: 60, previewId: null, photoToken: 0, jobStatus: "idle"};
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
  $$(".nav-button").forEach(b => b.classList.toggle("active",b.dataset.view===view));
  $$(".view").forEach(v => v.classList.toggle("active",v.id==="view-"+view));
  const names={dashboard:"Обзор матча",analysis:"Анализ видео",players:"Игроки и ID",report:"Отчёт по матчу",results:"Готовые ролики"};
  $("#page-title").textContent=names[view] || names.dashboard;
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
function renderDashboard() {
  const r=state.aggregate||state.report||{};
  const c=r.counts||{};
  $("#cache-status").textContent=r.analyzed?(state.aggregate?"Проект: "+r.analyzed_videos+"/"+r.total_videos+" видео":"Анализ сохранён • "+r.cache_id):"Нет завершённого анализа";
  $("#metrics").innerHTML=[
    metric("Фрагменты розыгрышей",r.rallies??"—","Эвристические границы"),
    metric("Обнаруженные атаки",c.attack??"—","По сохранённым событиям"),
    metric("Track ID",r.track_count??"—","Не уникальные спортсмены"),
    metric("Названные игроки",r.groups?.length??0,"Объединённые ID")
  ].join("");
  $("#event-bars").innerHTML=barRows(c);
  $("#recent-renders").innerHTML=renderFiles(state.renders.slice(0,3));
}
function renderFiles(files) {
  if (!files || !files.length) return '<div class="empty">Пока нет готовых роликов</div>';
  return files.map(f=>'<button type="button" class="file-row render-file" data-file="'+esc(f.name)+'" data-url="'+esc(f.url||"")+'"><span class="file-icon">▣</span><span class="file-info"><strong>'+esc(f.name)+'</strong><small>MP4 · '+esc(f.size_mb)+' МБ</small></span><span class="file-arrow">↗</span></button>').join("");
}
function renderReport() {
  const r=state.aggregate||state.report||{};
  const c=r.counts||{};
  $("#report-notice").textContent=r.analyzed
    ? "Статистика приблизительная: один спортсмен может получить несколько track ID. Нулевое значение не доказывает отсутствие действия. "+
       (r.court_available?"Разметка площадки найдена.":"Разметка площадки отсутствует: подачи и блоки могут не распознаваться.")
    : "Для этого видео пока нет завершённого анализа. Перейдите в «Анализ видео».";
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
function fillSelect(selector) {
  const sel=$(selector);
  const current=sel.value||state.video;
  const project=selectedProject();
  const candidates=project?.videos?.length?state.videos.filter(v=>project.videos.includes(v.name)):state.videos;
  sel.innerHTML=candidates.map(v=>
    '<option value="'+esc(v.name)+'">'+esc(v.name)+(v.analyzed?" ✓":"")+'</option>'
  ).join("");
  sel.value=state.videos.some(v=>v.name===current)?current:state.video;
}
function selectedProject() {
  return state.projects.find(p=>p.id===state.project)||null;
}
function drawProjectControls() {
  const select=$("#top-project");
  select.innerHTML='<option value="">Без проекта — одно видео</option>'+
    state.projects.map(p=>'<option value="'+esc(p.id)+'">'+esc(p.name)+'</option>').join("");
  select.value=state.project;
  const p=selectedProject(),included=new Set(p?.videos||[]);
  $("#project-video-options").innerHTML=state.videos.map(v=>
    '<label class="project-check"><input class="project-video-check" type="checkbox" value="'+esc(v.name)+'" '+
    (included.has(v.name)?"checked":"")+'> <span>'+esc(v.name)+(v.analyzed?" · анализ готов":" · ожидает анализа")+'</span></label>'
  ).join("");
  $("#project-summary").textContent=p?(p.videos.length+" исходных видео · "+p.analyses.filter(x=>x.analyzed).length+" обработано"):
    "Создайте проект и отметьте исходные записи";
  $("#analysis-project-hint").textContent=p
    ? 'Проект «'+p.name+'»: обработка всех '+p.videos.length+' видео по очереди. Диапазон на шкале задаётся отдельно для каждой записи.'
    : "Без проекта анализируется только выбранный файл. Для нескольких видео создайте проект на вкладке «Обзор».";
  $("#highlights-project-hint").textContent=p
    ? 'Проект «'+p.name+'»: лучшие полные розыгрыши будут отобраны из всех '+p.videos.length+' видео.'
    : "Без проекта подборка строится из одного выбранного видео.";
  $("#save-project-videos").disabled=!p;
}
async function loadProjects() {
  state.projects=(await api("/api/projects")).projects;
  if(!state.projects.some(p=>p.id===state.project))
    state.project=state.projects[0]?.id||"";
  const p=selectedProject();
  if(p?.videos.length&&!p.videos.includes(state.video))state.video=p.videos[0];
  drawProjectControls();
}
function todayProjectName(date=new Date()) {
  const day=String(date.getDate()).padStart(2,"0");
  const month=String(date.getMonth()+1).padStart(2,"0");
  return day+"."+month+"."+date.getFullYear();
}
function showProjectForm() {
  if(!$("#new-project-form").hidden){hideProjectForm();return;}
  $("#new-project-name").value=todayProjectName();
  $("#create-project").setAttribute("aria-expanded","true");
  $("#new-project-form").hidden=false;
  $("#new-project-name").focus();
  $("#new-project-name").select();
}
function hideProjectForm() {
  $("#new-project-form").hidden=true;
  $("#create-project").setAttribute("aria-expanded","false");
}
async function createNewProject() {
  const name=$("#new-project-name").value.trim();
  if(!name){alertBox("Введите название проекта");return}
  const selected=Array.from(document.querySelectorAll(".project-video-check:checked")).map(x=>x.value);
  if(!selected.length&&state.video)selected.push(state.video);
  try{
    const result=await api("/api/project-create",{name,videos:selected});
    state.project=result.project.id;
    hideProjectForm();
    $("#new-project-name").value="";
    await loadAll();
    $("#project-save-status").textContent="Проект создан";
  }catch(e){alertBox(e.message)}
}
async function saveProjectVideos() {
  if(!state.project){alertBox("Сначала создайте проект");return}
  const videos=Array.from(document.querySelectorAll(".project-video-check:checked")).map(x=>x.value);
  if(!videos.length){alertBox("Отметьте хотя бы одно видео для проекта");return}
  try{
    await api("/api/project-update",{id:state.project,videos});
    await loadAll();
    $("#project-save-status").textContent="Список сохранён";
  }catch(e){alertBox(e.message)}
}
async function loadAll() {
  const videos=await api("/api/videos");
  state.videos=videos.videos;
  if (!state.videos.length) {
    alertBox("В папке project нет видео. Добавьте файл MP4 и обновите страницу.");
    return;
  }
  if(!state.videos.some(v=>v.name===state.video))state.video=state.videos[0].name;
  await loadProjects();
  ["#top-video","#analysis-video","#highlight-video"].forEach(fillSelect);
  await Promise.all([loadReport(),loadRenders()]);
  alertBox("");
}
async function loadReport() {
  if (!state.video) return;
  state.report=await api("/api/report?video="+encodeURIComponent(state.video)+(state.project?"&project_id="+encodeURIComponent(state.project):""));
  state.aggregate=state.project?await api("/api/project-report?id="+encodeURIComponent(state.project)):null;
  state.selected.clear();
  state.previewId=state.report?.raw?.[0]?.player_track_id!=null?String(state.report.raw[0].player_track_id):null;
  renderDashboard();renderReport();renderPlayers();
  renderPlayerPhotos().catch(e=>console.warn("photos",e));
  if(window.VolleyballAnalysisUI)window.VolleyballAnalysisUI.setVideo(state.video);
}
async function loadRenders() {
  state.renders=(await api("/api/renders")).videos;
  const project=state.projects.find(p=>p.id===state.project);
  if(project?.highlight_ready)state.renders.unshift({name:project.name+" / лучшие моменты",size_mb:"проект",url:"/api/project-media?id="+encodeURIComponent(project.id)});
  renderRenders();renderDashboard();
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
    if(p?.videos.length)state.video=p.videos[0];
    drawProjectControls();
    ["#top-video","#analysis-video","#highlight-video"].forEach(fillSelect);
    try{await Promise.all([loadReport(),loadRenders()])}catch(error){alertBox(error.message)}
  });
  $("#create-project").addEventListener("click",showProjectForm);
  $("#new-project-form").addEventListener("submit",e=>{
    e.preventDefault();
    createNewProject();
  });
  $("#cancel-create-project").addEventListener("click",()=>{
    hideProjectForm();
    $("#create-project").focus();
  });
  document.addEventListener("pointerdown",e=>{
    if(!$("#new-project-form").hidden && !e.target.closest(".topbar-create-area")){
      hideProjectForm();
    }
  });
  $("#new-project-form").addEventListener("keydown",e=>{
    if(e.key==="Escape"){e.preventDefault();hideProjectForm();}
  });
  $("#save-project-videos").addEventListener("click",saveProjectVideos);
  $$(".nav-button").forEach(b=>b.addEventListener("click",()=>go(b.dataset.view)));
  $$("[data-go]").forEach(b=>b.addEventListener("click",()=>go(b.dataset.go)));
  $("#top-video").addEventListener("change",async e=>{
    state.video=e.target.value;fillSelect("#analysis-video");fillSelect("#highlight-video");
    try{await loadReport()}catch(e){alertBox(e.message)}
  });
  $("#analysis-video").addEventListener("change",e=>{
    state.video=e.target.value;fillSelect("#top-video");fillSelect("#highlight-video");loadReport().catch(e=>alertBox(e.message));
  });
  $("#highlight-video").addEventListener("change",e=>{
    state.video=e.target.value;fillSelect("#top-video");fillSelect("#analysis-video");loadReport().catch(x=>alertBox(x.message));
  });
  $("#refresh").addEventListener("click",()=>loadAll().catch(e=>alertBox(e.message)));
  $("#analysis-form").addEventListener("submit",e=>{
    e.preventDefault();
    const form=e.target;
    const params=Object.fromEntries(new FormData(form).entries());
    params.video=$("#analysis-video").value;
    params.beach=form.querySelector('[name="beach"]').checked;
    try{Object.assign(params,window.VolleyballAnalysisUI.params());}
    catch(error){alertBox(error.message);return;}
    if(state.project)params.project_id=state.project;
    submitTask("/api/analyze",params);
  });
  $("#highlights-form").addEventListener("submit",e=>{
    e.preventDefault();
    submitTask("/api/highlights",{video:$("#highlight-video").value,duration:Number(e.target.elements.duration.value),...(state.project?{project_id:state.project}:{})});
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
  if(["dashboard","analysis","players","report","results"].includes(requestedView))go(requestedView);
  try{await loadAll();await pollJob()}catch(e){alertBox(e.message)}
  window.setInterval(()=>pollJob().catch(e=>console.warn("status",e)),2500);
});
