"use strict";
// Source-scoped project statistics UI. Server owns all counts and source
// verification. Browser never derives identity, Touch or technique labels.
(() => {
  const get=id=>document.getElementById(id);
  const ui={
    project:get("studio-project"),refresh:get("studio-stats-refresh"),
    preview:get("studio-stats-preview"),save:get("studio-stats-save"),
    status:get("studio-stats-status"),sources:get("studio-stats-sources"),
    summary:get("studio-stats-summary"),total:get("studio-stats-total"),
    assigned:get("studio-stats-assigned"),unknown:get("studio-stats-unknown"),
    players:get("studio-stats-players"),diagnostics:get("studio-stats-diagnostics"),
    details:get("studio-stats-source-details")
  };
  const state={token:0,project:"",sources:new Map(),selections:new Map(),
    busy:false,revision:null,previewFingerprint:null,previewSelection:null};
  const el=(tag,klass,value)=>{
    const n=document.createElement(tag);if(klass)n.className=klass;
    if(value!==undefined)n.textContent=String(value);
    return n;
  };
  const clear=node=>node.replaceChildren();
  const note=(message,error=false)=>{
    ui.status.textContent=String(message);
    ui.status.classList.toggle("track-error",error);
  };
  const hash=value=>typeof value==="string"&&/^[a-f0-9]{64}$/.test(value);
  const finiteCount=value=>Number.isSafeInteger(value)&&value>=0;
  const selection=()=>[...state.selections.entries()].filter(([,id])=>id)
    .sort(([a],[b])=>a.localeCompare(b))
    .map(([,rally_run_id])=>({rally_run_id}));
  const selectionKey=()=>JSON.stringify(selection());
  const invalidate=()=>{
    state.revision=null;state.previewFingerprint=null;state.previewSelection=null;
    clear(ui.players);clear(ui.details);
    ui.summary.classList.add("hidden");ui.diagnostics.classList.add("hidden");
    ui.save.disabled=true;
  };
  const controls=()=>{
    const ready=Boolean(state.project&&selection().length&&!state.busy);
    ui.refresh.disabled=state.busy;
    ui.preview.disabled=!ready;
    ui.save.disabled=!ready||!state.revision||!hash(state.previewFingerprint)||
      state.previewSelection!==selectionKey();
  };
  const query=async url=>{
    const response=await window.VolleyballStudioActivity.fetch(url,{credentials:"same-origin",cache:"no-store"});
    const data=await response.json().catch(()=>({}));
    if(!response.ok)throw Error(data.message||data.error||"HTTP "+response.status);
    return data;
  };
  const post=async body=>{
    const response=await window.VolleyballStudioActivity.fetch("/api/players/native/statistics",{
      method:"POST",credentials:"same-origin",cache:"no-store",
      headers:{"Content-Type":"application/json"},body:JSON.stringify(body)
    });
    const data=await response.json().catch(()=>({}));
    if(!response.ok)throw Error(data.message||data.error||"HTTP "+response.status);
    return data;
  };
  const show=(data,mode)=>{
    if(data.schema!=="volley-native-player-statistics/1"||
      !hash(data.source_fingerprint)||!Array.isArray(data.sources)||
      !Array.isArray(data.players)||!Array.isArray(data.raw_tracks)||
      data.auto_player_identity_enabled!==false||
      data.automatic_action_classification!==false||
      data.all_contacts_physically_verified!==true)
      throw Error("Сервер вернул неподтверждённую статистику");
    const selected=selection();
    if(data.project_id!==state.project||
      data.video_count!==selected.length||data.sources.length!==selected.length)
      throw Error("Не совпадает набор видео в статистике");
    const distinct=new Set();
    let perSourceTotal=0,perSourceUnknown=0;
    for(const src of data.sources){
      if(!state.selections.has(src.video)||state.selections.get(src.video)!==src.rally_run_id||
        distinct.has(src.video)||
        !hash(src.source_replay_sha256)||!hash(src.source_touch_sha256)||
        !hash(src.source_rally_sha256)||
        !(hash(src.identity_revision)||src.identity_revision==="absent")||
        !finiteCount(src.total_touch_candidates)||
        !finiteCount(src.unassigned_touch_candidates)||
        src.unassigned_touch_candidates>src.total_touch_candidates)
        throw Error("Нарушена версия или целостность источника статистики");
      distinct.add(src.video);
      perSourceTotal+=src.total_touch_candidates;
      perSourceUnknown+=src.unassigned_touch_candidates;
    }
    const total=data.total_touch_candidates,assigned=data.assigned_touch_candidates,
      unknown=data.unassigned_touch_candidates;
    if(![total,assigned,unknown,perSourceTotal,perSourceUnknown].every(finiteCount)||
      total!==assigned+unknown||total!==perSourceTotal||unknown!==perSourceUnknown||
      data.players.some(p=>!finiteCount(p.touches))||
      data.players.reduce((sum,p)=>sum+p.touches,0)!==assigned)
      throw Error("Количество касаний не согласуется с проверенными источниками");
    ui.total.textContent=String(total);
    ui.assigned.textContent=String(assigned);
    ui.unknown.textContent=String(unknown);
    ui.summary.classList.remove("hidden");
    clear(ui.players);
    const header=el("div","native-stats-row native-stats-heading");
    header.append(el("strong","","Игрок"),el("strong","","Касания"));
    ui.players.append(header);
    for(const player of data.players){
      if(player.identity_source!=="manually_reviewed_native_track_ids"||
        typeof player.name!=="string")throw Error("Недостоверная привязка игрока");
      const row=el("div","native-stats-row");
      row.append(el("span","",player.name),el("strong","",player.touches));
      ui.players.append(row);
    }
    const unknownRow=el("div","native-stats-row native-stats-unknown");
    unknownRow.append(el("span","","Без подтверждённого игрока"),
      el("strong","",unknown));ui.players.append(unknownRow);
    clear(ui.details);
    const description=el("p","",
      "Проверенный набор источников: "+data.source_fingerprint+
      " · алгоритм: "+data.algorithm);
    ui.details.append(description);
    for(const src of data.sources){
      const card=el("article","native-stats-source");
      card.append(el("strong","",src.video),
        el("p","","CPU розыгрыши: "+src.rally_run_id),
        el("p","","Track: "+src.source_replay_id+" · SHA "+src.source_replay_sha256),
        el("p","","Touch: "+src.source_touch_id+" · SHA "+src.source_touch_sha256),
        el("p","","Розыгрыши SHA: "+src.source_rally_sha256),
        el("p","","Ревизия привязок: "+src.identity_revision),
        el("p","","Касаний: "+src.total_touch_candidates+
          " · без игрока: "+src.unassigned_touch_candidates));
      ui.details.append(card);
    }
    ui.diagnostics.classList.remove("hidden");
    state.revision=String(data.saved_revision);
    state.previewFingerprint=data.source_fingerprint;
    state.previewSelection=selectionKey();
    note((mode==="recalculate"?"Сохранено. ":"Предпросмотр. ")+
      "Видео: "+data.video_count+" · касаний: "+total+
      " · непривязанных: "+unknown+
      " · снимок: "+state.revision.slice(0,12));
  };
  const discover=async()=>{
    const project=ui.project.value;
    state.token++;const token=state.token;
    state.project=project;state.sources.clear();state.selections.clear();
    invalidate();clear(ui.sources);controls();
    if(!project){note("Выберите проект для статистики.");return;}
    state.busy=true;controls();
    note("Ищем сохранённые версии CPU для каждого видео…");
    try{
      const result=await query("/api/projects");
      if(token!==state.token)return;
      const entry=(Array.isArray(result.projects)?result.projects:[])
        .find(item=>item.id===project);
      if(!entry)throw Error("Проект отсутствует в реестре");
      const videos=Array.isArray(entry.videos)?entry.videos:[];
      if(videos.length>40)throw Error("Более 40 видео: выберите меньший проект");
      for(const video of videos){
        if(typeof video!=="string")continue;
        const overview=await query("/api/studio/rust/analysis-overview?"+
          new URLSearchParams({project_id:project,video}));
        if(token!==state.token)return;
        if(overview.schema!=="volley-native-analysis-overview/1"||
          overview.project_id!==project||overview.video!==video||
          overview.metadata_only!==true)
          throw Error("Неизвестный индекс стадий CPU для "+video);
        const stage=(Array.isArray(overview.stages)?overview.stages:[])
          .find(row=>row.key==="rallies");
        const versions=(Array.isArray(stage?.items)?stage.items:[])
          .filter(row=>/^[a-f0-9]{32}$/.test(row.id)&&hash(row.sha256));
        if(stage?.truncated)throw Error("Слишком много версий CPU для "+video);
        state.sources.set(video,versions);
        if(versions.length)state.selections.set(video,versions[0].id);
      }
      clear(ui.sources);
      for(const [video,versions] of state.sources){
        const row=el("label","native-stats-source-choice");
        row.append(el("span","",video));
        const select=el("select");
        const none=el("option","","Не учитывать видео");
        none.value="";select.append(none);
        for(const item of versions){
          const option=el("option","",
            item.id.slice(0,12)+" · "+(item.count??"—")+" розыгрышей");
          option.value=item.id;select.append(option);
        }
        select.value=state.selections.get(video)||"";
        select.addEventListener("change",()=>{
          if(select.value)state.selections.set(video,select.value);
          else state.selections.delete(video);
          invalidate();controls();
          note("Состав CPU-источников изменён. Повторите предпросмотр.");
        });
        row.append(select);ui.sources.append(row);
      }
      note("Видео: "+videos.length+" · с результатами CPU: "+
        state.selections.size+". Выберите версии и нажмите «Предпросмотр».");
    }catch(e){
      if(token===state.token){clear(ui.sources);state.selections.clear();
        invalidate();note(e.message,true);}
    }finally{
      if(token===state.token){state.busy=false;controls();}
    }
  };
  const execute=async mode=>{
    if(state.busy||!state.project||!selection().length)return;
    if(mode==="recalculate"&&(!state.revision||
      state.previewSelection!==selectionKey()))return;
    const project=state.project,token=state.token;
    const selected=selectionKey();
    const body={project_id:project,sources:JSON.parse(selected),mode};
    if(mode==="recalculate"){
      body.expected_revision=state.revision;
      body.expected_source_fingerprint=state.previewFingerprint;
    }
    state.busy=true;controls();
    note(mode==="preview"?"Проверяем исходные SHA и касания…":
      "Пересчитываем по подтверждённым источникам и сохраняем…");
    try{
      const data=await post(body);
      if(token!==state.token||project!==state.project||
        selected!==selectionKey())return;
      if(mode==="recalculate"&&data.snapshot_persisted!==true)
        throw Error("Сервер не подтвердил сохранение статистики");
      if(mode==="preview"&&data.snapshot_persisted!==false)
        throw Error("Предпросмотр изменил статистику");
      show(data,mode);
    }catch(e){
      if(token===state.token){invalidate();
        note(e.message+" · обновите источники и повторите предпросмотр.",true);}
    }finally{if(token===state.token){state.busy=false;controls();}}
  };
  ui.refresh.addEventListener("click",discover);
  ui.preview.addEventListener("click",()=>execute("preview"));
  ui.save.addEventListener("click",()=>execute("recalculate"));
  window.addEventListener("volley:tab-change",event=>{
    if(event.detail?.tab==="players"&&ui.project.value!==state.project)discover();
  });
  window.addEventListener("volley:project-ready",()=>{
    if(location.hash==="#players")discover();
  });
  if(location.hash==="#players")discover();
})();
