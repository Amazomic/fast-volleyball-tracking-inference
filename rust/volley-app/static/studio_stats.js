"use strict";
// Source-scoped project statistics UI. Server owns all counts and source
// verification. Browser never derives identity, Touch or technique labels.
(() => {
  const get=id=>document.getElementById(id);
  const ui={project:get("studio-project"),status:get("studio-stats-status"),
    summary:get("studio-stats-summary"),total:get("studio-stats-total"),
    rallies:get("studio-stats-rallies"),attacks:get("studio-stats-attacks"),
    blocks:get("studio-stats-blocks"),players:get("studio-stats-players")};
  const state={token:0,project:"",selections:new Map()};
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
  const selection=()=>[...state.selections.entries()].sort(([a],[b])=>a.localeCompare(b))
    .map(([,rally_run_id])=>({rally_run_id}));
  const invalidate=()=>{clear(ui.players);ui.summary.classList.add("hidden");};
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
      typeof data.automatic_action_classification!=="boolean"||
      data.automatic_action_classification_verified!==false||
      !data.estimated_actions||typeof data.estimated_actions!=="object"||
      !finiteCount(data.rally_candidates)||!finiteCount(data.estimated_hits)||
      typeof data.mean_player_touches!=="number"||!Number.isFinite(data.mean_player_touches)||data.mean_player_touches<0||
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
      data.players.some(p=>!finiteCount(p.touches)||!finiteCount(p.unclassified_touches)||
        !(p.activity===null||(typeof p.activity==="number"&&Number.isFinite(p.activity)&&p.activity>=0)))||
      data.players.reduce((sum,p)=>sum+p.touches,0)!==assigned)
      throw Error("Количество касаний не согласуется с проверенными источниками");
    if (![data.estimated_actions.attack ?? 0,data.estimated_actions.block ?? 0].every(finiteCount))
      throw Error("Некорректное количество игровых действий");
    ui.total.textContent=String(total);
    ui.rallies.textContent=String(data.rally_candidates);
    ui.attacks.textContent=data.automatic_action_classification?String(data.estimated_actions.attack ?? 0):"—";
    ui.blocks.textContent=data.automatic_action_classification?String(data.estimated_actions.block ?? 0):"—";
    ui.summary.classList.remove("hidden");
    clear(ui.players);
    const mean=Number(data.mean_player_touches).toLocaleString("ru-RU",{maximumFractionDigits:2});
    for(const player of data.players){
      if(player.identity_source!=="manually_reviewed_native_track_ids"||
        typeof player.name!=="string")throw Error("Недостоверная привязка игрока");
      const card=el("article","report-player");
      card.append(el("h4","",player.name),el("p","","Касания: "+player.touches),
        el("p","","Активность: "+mean));
      ui.players.append(card);
    }
    note(data.players.length?"Активность — среднее число касаний на игрока.":
      "Добавьте игроков и подтвердите их привязки, чтобы увидеть персональную статистику.");
  };
  const discover=async()=>{
    const project=ui.project.value;
    state.token++;const token=state.token;
    state.project=project;state.selections.clear();invalidate();
    if(!project){note("Выберите проект для просмотра статистики.");return;}
    note("Загружаем статистику игры…");
    try{
      const result=await query("/api/projects");
      if(token!==state.token)return;
      const entry=(Array.isArray(result.projects)?result.projects:[]).find(item=>item.id===project);
      if(!entry)throw Error("Проект отсутствует в реестре");
      const videos=Array.isArray(entry.videos)?entry.videos:[];
      if(videos.length>40)throw Error("Более 40 видео: выберите меньший проект");
      const overviews=await window.VolleyballOverview.loadMany(project,videos);
      if(token!==state.token)return;
      for(const [video,overview] of overviews){
        const stage=(Array.isArray(overview.stages)?overview.stages:[]).find(row=>row.key==="rallies");
        const versions=(Array.isArray(stage?.items)?stage.items:[])
          .filter(row=>/^[a-f0-9]{32}$/.test(row.id)&&hash(row.sha256));
        if(stage?.truncated)throw Error("Слишком много версий CPU для "+video);
        if(versions.length)state.selections.set(video,versions[0].id);
      }
      if(!state.selections.size){note("Сначала выполните анализ видео, чтобы увидеть статистику.");return;}
      const data=await post({project_id:project,sources:selection(),mode:"preview"});
      if(token!==state.token)return;
      if(data.snapshot_persisted!==false)throw Error("Предпросмотр изменил статистику");
      show(data,"preview");
      if(state.selections.size<videos.length)note("Учтены не все видео: для некоторых нет результатов анализа.");
    }catch(e){if(token===state.token){invalidate();note(e.message,true);}}
  };
  window.addEventListener("volley:tab-change",event=>{
    if(event.detail?.tab==="report")discover();
  });
  window.addEventListener("volley:project-ready",()=>{
    if(location.hash==="#report")discover();
  });
  window.addEventListener("volley:pipeline-complete",()=>{
    if(location.hash==="#report")discover();
  });
  if(location.hash==="#report")discover();
})();
