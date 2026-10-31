"use strict";

const $ = (selector) => document.querySelector(selector);
const $$ = (selector) => Array.from(document.querySelectorAll(selector));
const state = {videos: [], video: "", projects: [], project: "", aggregate: null, report: null, renders: [],
  selected: new Set(), limit: 60, previewId: null, photoToken: 0, jobStatus: "idle",
  dialogEdit: false, dialogBusy: false, dialogRequestId: 0, analysisContext: null};
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
  const names={analysis:"Анализ","analysis-results":"Результат анализа",players:"Игроки",report:"Отчёт проекта",benchmark:"Бенчмарк",results:"Монтаж"};
  $("#page-title").textContent=names[view] || names.analysis;
  if(view==="analysis-results"&&state.project){
    const project=selectedProject();
    window.VolleyballResultsUI.setProject(state.project,project.videos).catch(err=>alertBox(err.message));
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
    "&sample="+sample+"&project_id="+encodeURIComponent(state.project||"");
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
      const meta=await api("/api/player-photos?video="+encodeURIComponent(video)+"&id="+encodeURIComponent(id)+
        "&project_id="+encodeURIComponent(state.project||""));
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
  return p?.ranges?.[name]||{start:0,end:0};
}
function populatePlayersSelect() {
  const select=$("#players-video"),p=selectedProject();
  if(!p)return;
  select.innerHTML=p.videos.map(name=>'<option value="'+esc(name)+'">'+esc(name)+'</option>').join("");
  if(p.videos.includes(state.video))select.value=state.video;
}
function renderProjectPicker() {
  const current=selectedProject();
  $("#project-settings-trigger").disabled=!current;
  $("#project-picker-name").textContent=current?.name||"Выберите проект";
  const menu=$("#project-picker-menu");
  menu.innerHTML=state.projects.length?state.projects.map((p,i)=>{
    const duplicate=state.projects.filter(x=>x.name.toLocaleLowerCase()===p.name.toLocaleLowerCase()).length>1;
    const caption=p.name+(duplicate?" · "+p.id.slice(0,6):"");
    return '<button type="button" class="project-picker-option'+(p.id===state.project?' is-selected':'')+
      '" role="option" id="project-picker-option-'+i+'" data-project-id="'+esc(p.id)+
      '" aria-selected="'+(p.id===state.project)+'" title="'+esc(caption)+'">'+
      '<span class="project-picker-option-glyph">'+(p.id===state.project?'✓':'▣')+'</span>'+
      '<span class="project-picker-option-name">'+esc(caption)+'</span>'+
      '</button>';
  }).join(""):'<div class="project-picker-empty">Пока нет проектов</div>';
}
function closeProjectPicker(returnFocus=false){
  $("#project-picker-menu").hidden=true;
  $("#project-picker-trigger").setAttribute("aria-expanded","false");
  $("#project-picker").classList.remove("is-open");
  if(returnFocus)$("#project-picker-trigger").focus();
}
function openProjectPicker(focusOption=false){
  if(!state.projects.length)return;
  const menu=$("#project-picker-menu");
  menu.hidden=false;
  $("#project-picker-trigger").setAttribute("aria-expanded","true");
  $("#project-picker").classList.add("is-open");
  if(focusOption){
    const target=menu.querySelector('[aria-selected="true"]')||menu.querySelector('[role="option"]');
    target?.focus();
  }
}
function renderSources() {
  const p=selectedProject();
  $("#top-project").innerHTML='<option value="">Выберите проект</option>'+
    state.projects.map(p=>'<option value="'+esc(p.id)+'">'+esc(p.name)+
      (state.projects.filter(x=>x.name.toLocaleLowerCase()===p.name.toLocaleLowerCase()).length>1
       ?' · '+esc(p.id.slice(0,6)):'')+'</option>').join("");
  $("#top-project").value=state.project||"";
  renderProjectPicker();
  $("#no-project").hidden=!!p;
  $("#project-workspace").hidden=!p;
  for(const btn of $$(".nav-button"))btn.disabled=!p&&btn.dataset.view!=="analysis";
  if(!p) {
    $("#active-project-name").textContent="Анализ";
    $("#active-project-description").textContent="Создайте проект и добавьте видео. Настройки анализа доступны после создания.";
    return;
  }
  $("#active-project-name").textContent=p.name;
  $("#active-project-description").textContent="Выберите видео справа, задайте диапазон под проигрывателем и параметры запуска ниже.";
  $("#source-count").textContent="· "+p.videos.length;
  $("#project-source-list").innerHTML=p.videos.map((name,i)=>{
    const range=projectRange(name);
    const duration=range.end>0?clock(range.start)+" — "+clock(range.end):"Весь файл";
    return '<button type="button" class="project-source '+(state.video===name?"selected":"")+
      '" data-source="'+esc(name)+'"><span class="source-index">ВИДЕО '+String(i+1).padStart(2,"0")+'</span>'+
      '<span class="source-name">'+esc(name)+'</span>'+
      '<span class="source-range">'+esc(duration)+'</span></button>';
  }).join("");
  $("#analysis-project-hint").textContent="Анализ всех "+p.videos.length+
      " видео проекта. Параметры применяются ко всем файлам, временные диапазоны настраиваются отдельно.";
  $("#highlights-project-hint").textContent="Монтаж из "+p.videos.length+" видео проекта; полный выбранный розыгрыш не обрезается ради длительности.";
  populatePlayersSelect();
  $("#benchmark-video").innerHTML=p.videos.map(name=>
    '<option value="'+esc(name)+'">'+esc(name)+'</option>').join("");
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
    row.classList.toggle("is-selected",cb.checked);
  }
}
async function openProjectDialog(edit=false) {
  if(state.dialogBusy)return;
  const requestId=++state.dialogRequestId;
  state.dialogEdit=edit;
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
  $("#project-selected-count").textContent="Поиск доступных видео…";
  $("#new-project-video-options").innerHTML='<p class="muted">Проверяем папку project/…</p>';
  dlg.showModal();
  $("#new-project-name").focus();
  $("#new-project-name").select();

  // Check available source files only when this dialog is explicitly opened.
  try {
    const response=await api("/api/videos");
    if(requestId!==state.dialogRequestId||!dlg.open)return;
    state.videos=response.videos;
    const selected=new Set(edit&&p?p.videos:[]);
    $("#new-project-video-options").innerHTML=state.videos.length?state.videos.map(v=>
      '<div class="dialog-video-row" data-video="'+esc(v.name)+'">'+
        '<label class="dialog-video-heading"><input type="checkbox" class="dialog-source-check" value="'+esc(v.name)+'" '+(selected.has(v.name)?"checked":"")+'><span><strong>'+esc(v.name)+'</strong><small class="dialog-duration">'+esc(v.size_gb)+' ГБ</small></span></label>'+
      '</div>'
    ).join(""):'<p class="muted">В папке project/ нет подходящих видео. Добавьте файлы и откройте это окно снова.</p>';
    $("#confirm-project").disabled=!state.videos.length;
    updateDialogSelection();
  } catch(error) {
    if(requestId!==state.dialogRequestId||!dlg.open)return;
    $("#new-project-video-options").innerHTML="";
    $("#project-selected-count").textContent="Список видео не загружен";
    $("#project-dialog-error").textContent=error.message;
    $("#project-dialog-error").hidden=false;
  }
}
function closeProjectDialog() {
  ++state.dialogRequestId;
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
  if(!rows.length)return fail("Выберите хотя бы одно видео");
  const videos=[];
  for(const row of rows){
    const filename=row.dataset.video;
    videos.push(filename);
  }
  state.dialogBusy=true;$("#confirm-project").disabled=true;error.hidden=true;
  try {
    const payload=state.dialogEdit?{id:p.id,name,videos}:{name,videos};
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
  await loadProjects();
  if(selectedProject()){
    await Promise.all([loadReport(),loadRenders(),loadBenchmarkResults(),refreshAnalysisContext(),
      window.VolleyballAnalysisUI.setProjectRanges(selectedProject().ranges||{},state.project)]);
    if(state.video)await window.VolleyballAnalysisUI.setVideo(state.video);
    if($("#view-analysis-results").classList.contains("active"))
      await window.VolleyballResultsUI.setProject(state.project,selectedProject().videos);
  }else{
    state.report=null;state.aggregate=null;state.renders=[];state.analysisContext=null;
    window.VolleyballAnalysisUI.setProjectRanges({},"");
    $("#source-video-player").pause();
    $("#source-video-player").removeAttribute("src");
    $("#source-video-player").load();
    renderReport();renderRenders();displayAnalysisContext();
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
        '<span class="context-court '+(item?.court_saved?"is-saved":"is-missing")+'">'+court+'</span></div>';
    }).join("");
}
async function refreshAnalysisContext(){
  if(!state.project){state.analysisContext=null;displayAnalysisContext();return;}
  state.analysisContext=await api("/api/project-context?project_id="+encodeURIComponent(state.project));
  displayAnalysisContext();
}
async function saveCurrentRange(){
  const p=selectedProject();
  if(!p)return;
  const range=window.VolleyballAnalysisUI.currentRange();
  if(!range)throw Error("Диапазон не загружен");
  const draft=window.VolleyballCourtUI.snapshot();
  const active=state.video;
  $("#range-save-status").textContent="Сохраняем диапазон и разметку…";
  const updated=await api("/api/analysis-context",{
    project_id:p.id,video:active,range,court:draft.court,clear_court:draft.clear_court
  });
  p.ranges={...p.ranges,[active]:updated.range};
  window.VolleyballCourtUI.markSaved(updated.court);
  $("#range-save-status").textContent="Сохранено · "+clock(range.start)+"–"+clock(range.end)+
    (updated.court?" · площадка размечена":" · без разметки");
  await refreshAnalysisContext();
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
  $("#analysis-run-status").textContent=info.kind==="analyze"?
    ({running:"Анализ выполняется…",complete:"Анализ завершён. Смотрите вкладку «Результат анализа».",
      failed:"Ошибка анализа — откройте «Результат анализа» и проверьте журнал.",
      cancelled:"Анализ остановлен."}[info.status]||""):"Готово к запуску";
  $("#benchmark-state").textContent=info.kind==="benchmark"
    ? ({running:"Измерение…",complete:"Готово",failed:"Ошибка",cancelled:"Остановлен"}[info.status]||"Нет замера")
    : "Нет замера";
  $("#benchmark-cancel").disabled=info.status!=="running"||info.kind!=="benchmark";
  $("#benchmark-log").textContent=info.kind==="benchmark"?(info.log||"Подготовка…"):"Здесь появится журнал замера.";
  $("#benchmark-current").querySelector("b").textContent=info.kind==="benchmark"?
    (info.status==="running"?"Идёт бенчмарк":info.status==="complete"?"Измерение завершено":"Статус: "+info.status):
    "Ожидание запуска";
  $("#benchmark-current").querySelector("p").textContent=info.kind==="benchmark"?
    (info.stage||info.log.slice(-150)||""):
    "Выберите видео и режим.";
  if(previous==="running" && info.status!=="running"){
    await Promise.all([loadAll(),loadRenders()]);
    if(info.kind==="benchmark")await loadBenchmarkResults();
    if(info.status==="failed")alertBox("Обработка завершилась с ошибкой. Посмотрите журнал.");
  }
}
async function loadBenchmarkResults(){
  if(!state.project){
    $("#benchmark-results").innerHTML='<div class="empty">Выберите проект</div>';
    return;
  }
  const results=await api("/api/benchmark-results?project_id="+encodeURIComponent(state.project));
  const runs=(results.runs||[]).slice().reverse();
  $("#benchmark-results").innerHTML=runs.length?runs.map(r=>
    '<div class="benchmark-result"><div><strong>'+esc(r.video)+'</strong><small>'+
    esc(r.mode==="decode"?"FFmpeg / декодирование":"Нейросети / полный анализ")+
    ' · '+esc(r.created_at?.slice(0,19)||"")+' · '+esc(r.width)+' px</small></div>'+
    '<div class="benchmark-numbers"><b>'+Number(r.fps).toFixed(1)+' FPS</b><span>'+
    Number(r.realtime_multiplier).toFixed(2)+'× realtime</span><span>'+
    Number(r.wall_sec).toFixed(2)+' с работы</span></div></div>'
  ).join(""):'<div class="empty">Замеров пока нет.</div>';
}
async function submitTask(endpoint,payload,view="analysis"){
  try{
    const j=await api(endpoint,payload);
    state.jobStatus=j.status;go(view);alertBox("");
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
function closeSettingsMenu(returnFocus=false){
  const menu=$("#project-settings-menu");
  menu.hidden=true;
  $("#project-settings-trigger").setAttribute("aria-expanded","false");
  if(returnFocus)$("#project-settings-trigger").focus();
}
function openSettingsMenu(){
  if(!selectedProject())return;
  closeProjectPicker();
  $("#project-settings-menu").hidden=false;
  $("#project-settings-trigger").setAttribute("aria-expanded","true");
}
function closeProjectActionDialogs(){
  for(const name of ["#project-rename-dialog","#project-delete-dialog"]){
    const dlg=$(name);
    if(dlg.open)dlg.close();
  }
}
function beginRenameProject(){
  const p=selectedProject();
  if(!p)return;
  closeSettingsMenu();
  $("#project-rename-name").value=p.name;
  $("#project-rename-error").hidden=true;
  $("#project-rename-dialog").dataset.projectId=p.id;
  $("#project-rename-dialog").showModal();
  $("#project-rename-name").focus();
  $("#project-rename-name").select();
}
function beginDeleteProject(){
  const p=selectedProject();
  if(!p)return;
  closeSettingsMenu();
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
  try{
    await api("/api/project-archive",{id});
    closeProjectActionDialogs();
    if(state.project===id){state.project="";state.video="";}
    await loadAll();
    go("analysis");
  }catch(e){
    error.textContent=e.message;error.hidden=false;
  }finally{button.disabled=false;}
}
function bind() {
  $("#project-settings-trigger").addEventListener("click",()=>{
    const menu=$("#project-settings-menu");
    if(menu.hidden)openSettingsMenu();else closeSettingsMenu();
  });
  $("#project-settings-trigger").addEventListener("keydown",e=>{
    if(["ArrowDown","Enter"," "].includes(e.key)){
      e.preventDefault();openSettingsMenu();
      $("#project-settings-rename").focus();
    }else if(e.key==="Escape")closeSettingsMenu();
  });
  $("#project-settings-menu").addEventListener("keydown",e=>{
    if(e.key==="Escape"){e.preventDefault();closeSettingsMenu(true);}
    if(e.key==="ArrowDown"||e.key==="ArrowUp"){
      e.preventDefault();
      const choices=[...$("#project-settings-menu").querySelectorAll('[role="menuitem"]')];
      const position=choices.indexOf(document.activeElement);
      choices[(position+(e.key==="ArrowDown"?1:-1)+choices.length)%choices.length]?.focus();
    }
  });
  $("#project-settings-rename").addEventListener("click",beginRenameProject);
  $("#project-settings-delete").addEventListener("click",beginDeleteProject);
  $("#project-rename-form").addEventListener("submit",renameProject);
  $("#project-delete-confirm").addEventListener("click",deleteProject);
  $$("[data-close-project-action]").forEach(el=>
    el.addEventListener("click",closeProjectActionDialogs));
  $("#project-picker-trigger").addEventListener("click",()=>{
    const expanded=$("#project-picker-trigger").getAttribute("aria-expanded")==="true";
    if(expanded)closeProjectPicker();
    else openProjectPicker();
  });
  $("#project-picker-trigger").addEventListener("keydown",e=>{
    if(["ArrowDown","ArrowUp","Enter"," "].includes(e.key)){
      e.preventDefault();
      openProjectPicker(true);
    }else if(e.key==="Escape")closeProjectPicker();
  });
  $("#project-picker-menu").addEventListener("click",e=>{
    const option=e.target.closest("[data-project-id]");
    if(!option)return;
    const id=option.dataset.projectId;
    closeProjectPicker(true);
    if(state.project!==id){
      $("#top-project").value=id;
      $("#top-project").dispatchEvent(new Event("change",{bubbles:true}));
    }
  });
  $("#project-picker-menu").addEventListener("keydown",e=>{
    const options=[...$("#project-picker-menu").querySelectorAll('[role="option"]')];
    const index=options.indexOf(document.activeElement);
    if(e.key==="Escape"){
      e.preventDefault();closeProjectPicker(true);
    }else if(["ArrowDown","ArrowUp","Home","End"].includes(e.key)){
      e.preventDefault();
      const next=e.key==="Home"?0:e.key==="End"?options.length-1:
        (index+(e.key==="ArrowUp"?-1:1)+options.length)%options.length;
      options[next]?.focus();
    }
  });
  document.addEventListener("pointerdown",e=>{
    if(!e.target.closest("#project-picker"))closeProjectPicker();
    if(!e.target.closest("#project-settings-trigger,#project-settings-menu"))closeSettingsMenu();
  });
  document.addEventListener("focusin",e=>{
    if(!e.target.closest("#project-picker"))closeProjectPicker();
    if(!e.target.closest("#project-settings-trigger,#project-settings-menu"))closeSettingsMenu();
  });
  $("#top-project").addEventListener("change",async e=>{
    state.project=e.target.value;
    const p=selectedProject();
    state.video=p?.videos[0]||"";
    renderSources();
    await Promise.all([loadReport(),loadRenders(),loadBenchmarkResults(),refreshAnalysisContext(),
      window.VolleyballAnalysisUI.setProjectRanges(p?.ranges||{},state.project)]);
    if(state.video)await window.VolleyballAnalysisUI.setVideo(state.video);
    if($("#view-analysis-results").classList.contains("active") && p)
      await window.VolleyballResultsUI.setProject(state.project,p.videos);
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
  $("#analysis-form").addEventListener("submit",async e=>{
    e.preventDefault();
    if(!state.project)return alertBox("Сначала создайте проект");
    const params=Object.fromEntries(new FormData(e.target).entries());
    params.beach=e.target.querySelector('[name="beach"]').checked;
    params.project_id=state.project;
    try {
      Object.assign(params,window.VolleyballAnalysisUI.params());
      await saveCurrentRange();
      await submitTask("/api/analyze",params,"analysis-results");
    }catch(err){alertBox(err.message)}
  });
  $("#benchmark-mode").addEventListener("change",e=>{
    const neural=e.target.value==="pipeline";
    $("#benchmark-backend-field").hidden=!neural;
    $("#benchmark-mode-hint").textContent=neural
      ?"Выполняется свежий короткий анализ включая нейросети; измеряется общая скорость pipeline, не чистый FPS модели."
      :"Измеряется декодирование и масштабирование FFmpeg, без нейросети.";
  });
  $("#benchmark-form").addEventListener("submit",e=>{
    e.preventDefault();
    if(!state.project)return alertBox("Сначала выберите проект");
    const form=Object.fromEntries(new FormData(e.target).entries());
    form.project_id=state.project;
    form.video=$("#benchmark-video").value;
    submitTask("/api/benchmark",form,"benchmark");
  });
  $("#benchmark-cancel").addEventListener("click",()=>api("/api/cancel",{}).then(pollJob).catch(err=>alertBox(err.message)));
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
  window.setInterval(()=>pollJob().catch(e=>console.warn("status",e)),2500);
});
