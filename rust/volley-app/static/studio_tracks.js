"use strict";
// Native Track ID manual identity-review bridge. No auto-ReID, ownership
// guessing, photo enrollment, or client-supplied source frame overrides.
(() => {
  const get=id=>document.getElementById(id);
  const ui={
    project:get("studio-project"),video:get("studio-track-video"),
    replay:get("studio-track-replay"),refresh:get("studio-track-refresh"),
    list:get("studio-track-list"),status:get("studio-track-status"),
    total:get("studio-track-total"),assigned:get("studio-track-assigned"),
    excluded:get("studio-track-excluded"),player:get("studio-track-player"),
    assign:get("studio-track-assign"),unassign:get("studio-track-unassign"),
    exclude:get("studio-track-exclude"),restore:get("studio-track-restore"),
    prev:get("studio-track-prev"),next:get("studio-track-next"),
    page:get("studio-track-page"),photo:get("studio-track-photo"),
    photoId:get("studio-track-photo-id"),photoImg:get("studio-track-photo-img")
  };
  const state={
    token:0,project:"",video:"",replay:"",offset:0,limit:24,
    selected:new Set(),revision:"absent",rows:[],total:0,
    roster:[],writable:false,busy:false
  };
  const node=(tag,klass,text)=>{
    const n=document.createElement(tag);if(klass)n.className=klass;
    if(text!==undefined)n.textContent=String(text);
    return n;
  };
  const clear=n=>{while(n.firstChild)n.firstChild.remove();};
  const path=(base,args)=>base+"?"+new URLSearchParams(args);
  const read=async url=>{
    const response=await fetch(url,{credentials:"same-origin",cache:"no-store"});
    const body=await response.json().catch(()=>({}));
    if(!response.ok)throw Error(body.message||body.error||"HTTP "+response.status);
    return body;
  };
  const post=async (url,payload)=>{
    const response=await fetch(url,{method:"POST",cache:"no-store",
      credentials:"same-origin",headers:{"Content-Type":"application/json"},
      body:JSON.stringify(payload)});
    const body=await response.json().catch(()=>({}));
    if(!response.ok)throw Error(body.message||body.error||"HTTP "+response.status);
    return body;
  };
  const note=(value,error=false)=>{
    ui.status.textContent=String(value);
    ui.status.classList.toggle("track-error",error);
  };
  const options=(select,rows,valueField,labelField,placeholder)=>{
    clear(select);
    const first=node("option","",placeholder);first.value="";select.append(first);
    for(const row of rows){
      const option=node("option","",labelField(row));
      option.value=String(row[valueField]);select.append(option);
    }
  };
  const active=()=>location.hash==="#players";
  const unlock=()=>{
    const selected=state.selected.size>0 && !state.busy && state.writable;
    ui.assign.disabled=!selected||!ui.player.value;
    ui.unassign.disabled=!selected;
    ui.exclude.disabled=!selected;
    ui.restore.disabled=!selected;
    ui.prev.disabled=state.busy||state.offset===0;
    ui.next.disabled=state.busy||state.offset+state.limit>=state.total;
  };
  const reset=()=>{
    state.token++;state.selected.clear();state.offset=0;state.rows=[];
    state.total=0;state.revision="absent";state.roster=[];state.writable=false;
    ui.total.textContent="—";ui.assigned.textContent="—";ui.excluded.textContent="—";
    ui.page.textContent="—";clear(ui.list);
    options(ui.player,[],"id",x=>x,"Игрок не выбран");
    ui.photo.classList.add("hidden");ui.photoImg.removeAttribute("src");unlock();
  };
  const loadVideos=async()=>{
    const project=ui.project.value;reset();
    state.project=project;state.video="";state.replay="";
    if(!project){
      options(ui.video,[],"name",x=>x,"Выберите проект");
      options(ui.replay,[],"id",x=>x,"Нет CPU-версий");
      note("Выберите проект с проведённым CPU-трекингом.");return;
    }
    const token=state.token;
    note("Проверка видео проекта…");
    try{
      const all=await read("/api/projects");
      if(token!==state.token||project!==ui.project.value)return;
      const item=(Array.isArray(all.projects)?all.projects:[]).find(p=>p.id===project);
      const videos=(Array.isArray(item?.videos)?item.videos:[])
        .filter(v=>typeof v==="string").map(name=>({name}));
      options(ui.video,videos,"name",v=>v.name,videos.length?"Выберите видео":"Нет видео");
      ui.video.value=videos.length===1?videos[0].name:"";
      await loadRuns();
    }catch(error){note(error.message,true);}
  };
  const loadRuns=async()=>{
    const project=ui.project.value,video=ui.video.value;
    reset();state.project=project;state.video=video;state.replay="";
    if(!project||!video){
      options(ui.replay,[],"id",x=>x,"Выберите видео");
      note("Выберите видео, чтобы найти CPU-версии Track ID.");return;
    }
    const token=state.token;
    note("Загрузка версий трекинга…");
    try{
      const result=await read(path("/api/studio/rust/analysis-overview",{
        project_id:project,video
      }));
      if(token!==state.token||project!==ui.project.value||video!==ui.video.value)return;
      if(result.schema!=="volley-native-analysis-overview/1"||result.metadata_only!==true)
        throw Error("Неизвестный формат обзора CPU");
      const stage=(Array.isArray(result.stages)?result.stages:[])
        .find(v=>v.key==="tracking");
      const versions=(Array.isArray(stage?.items)?stage.items:[])
        .filter(v=>typeof v.id==="string" && /^[a-f0-9]{32}$/.test(v.id));
      options(ui.replay,versions,"id",v=>
        "Track ID "+v.id.slice(0,9)+" · "+(v.count??0)+" ID",
        versions.length?"Выберите CPU-обработку":"Нет нативного CPU-трекинга");
      ui.replay.value=versions.length===1?versions[0].id:"";
      state.replay=ui.replay.value;
      if(state.replay)await loadPage();
      else note("Выберите сохранённую CPU-версию. ReID из старого players.json здесь не подставляется.");
    }catch(error){note(error.message,true);}
  };
  const loadPage=async()=>{
    const project=state.project,video=state.video,replay=state.replay;
    if(!project||!video||!replay)return;
    const token=++state.token;
    state.selected.clear();unlock();
    ui.list.replaceChildren(node("div","empty-state","Проверка SHA-256 и загрузка Track ID…"));
    note("Проверяем исходный CPU-трекинг, получаем имена игроков…");
    try{
      const result=await read(path("/api/players/native/tracks",{
        project_id:project,video,replay_id:replay,
        offset:state.offset,limit:state.limit
      }));
      if(token!==state.token||project!==state.project||
        video!==state.video||replay!==state.replay)return;
      if(result.schema!=="volley-native-player-track-links/1"||
        result.source_replay_sha256?.length!==64||
        result.automatic_reid_binding!==false)throw Error("Некорректный источник Track ID");
      state.rows=Array.isArray(result.tracks)?result.tracks:[];
      state.total=Number(result.total_tracks)||0;
      state.roster=Array.isArray(result.roster)?result.roster:[];
      state.revision=String(result.revision||"absent");
      state.writable=result.writable===true;
      ui.total.textContent=String(state.total);
      ui.assigned.textContent=String(result.assigned_count||0);
      ui.excluded.textContent=String(result.excluded_count||0);
      options(ui.player,state.roster,"id",p=>p.name,"Выберите игрока");
      renderRows();unlock();
      note(state.roster.length?
        "Track ID получены. Выберите ID и вручную назначьте игрока из состава проекта.":
        "Добавьте игроков в состав проекта через управление проектом, чтобы разрешить ручную привязку.");
    }catch(error){ui.list.replaceChildren(node("div","empty-state","Не удалось загрузить Track ID."));
      note(error.message,true);}
  };
  const select=(track,yes)=>{
    if(yes){
      if(state.selected.size>=200){note("Не более 200 Track ID за операцию.",true);return;}
      state.selected.add(track);
    }else state.selected.delete(track);
    unlock();
  };
  const renderRows=()=>{
    clear(ui.list);
    ui.page.textContent=state.total?
      (state.offset+1)+"–"+Math.min(state.offset+state.limit,state.total)+" из "+state.total:
      "Нет Track ID";
    if(!state.rows.length){
      ui.list.append(node("div","empty-state","Нет обнаруженных Track ID для этой CPU-версии."));
    }
    for(const row of state.rows){
      const id=Number(row.track_id);
      if(!Number.isSafeInteger(id)||id<=0)continue;
      const item=node("article","native-track-item"+(row.excluded?" excluded":""));
      const selection=node("input");selection.type="checkbox";
      selection.checked=state.selected.has(id);
      selection.setAttribute("aria-label","Выбрать Track ID "+id);
      selection.addEventListener("change",()=>select(id,selection.checked));
      const label=node("div","native-track-meta");
      label.append(node("strong","","Track ID "+id),
        node("small","",(row.observations??0)+" кадров · "+
          (row.first_frame??0)+"–"+(row.last_frame??0)+" · "+
          (Math.round((row.best_score??0)*100))+"% уверенность"));
      const status=node("span","track-owner",row.excluded?"Исключён":
        row.assigned_player_name||"Не привязан");
      if(row.excluded)status.classList.add("excluded");
      if(row.assigned_player_name)status.classList.add("assigned");
      const photo=node("button","secondary","Кадр");
      photo.type="button";photo.setAttribute("aria-label","Открыть фото Track ID "+id);
      photo.addEventListener("click",()=>{
        ui.photo.classList.remove("hidden");ui.photoId.textContent=String(id);
        ui.photoImg.alt="Обнаружение Track ID "+id+" · кадр "+row.best_frame;
        ui.photoImg.src=path("/api/players/native/track-photo",{
          project_id:state.project,video:state.video,
          replay_id:state.replay,track_id:String(id)
        });
      });
      item.append(selection,label,status,photo);
      ui.list.append(item);
    }
  };
  const edit=async action=>{
    if(state.busy||!state.selected.size||!state.replay)return;
    const ids=Array.from(state.selected);
    const owner=ui.player.value;
    if(action==="assign"&&!owner){note("Выберите игрока.",true);return;}
    const project=state.project,video=state.video,replay=state.replay;
    const revision=state.revision,offset=state.offset;
    state.busy=true;unlock();
    try{
      const payload={project_id:project,video,replay_id:replay,
        action,track_ids:ids,expected_revision:revision,offset,limit:state.limit};
      if(action==="assign")payload.player_id=owner;
      const result=await post("/api/players/native/tracks",payload);
      if(project!==state.project||video!==state.video||replay!==state.replay)return;
      state.revision=String(result.revision);
      state.selected.clear();
      state.rows=Array.isArray(result.tracks)?result.tracks:[];
      ui.assigned.textContent=String(result.assigned_count||0);
      ui.excluded.textContent=String(result.excluded_count||0);
      renderRows();
      note("Привязки обновлены. Исходные GPU/CPU данные не изменены.");
    }catch(e){
      note(e.message,true);
      // Preserve refusal, and refetch canonical state on conflict rather than
      // performing an optimistic hidden reassignment.
    }finally{state.busy=false;unlock();}
  };
  ui.video.addEventListener("change",()=>loadRuns().catch(e=>note(e.message,true)));
  ui.replay.addEventListener("change",()=>{
    reset();state.project=ui.project.value;state.video=ui.video.value;
    state.replay=ui.replay.value;
    loadPage().catch(e=>note(e.message,true));
  });
  ui.refresh.addEventListener("click",()=>loadVideos());
  ui.prev.addEventListener("click",()=>{
    if(state.offset>0){state.offset=Math.max(0,state.offset-state.limit);loadPage();}
  });
  ui.next.addEventListener("click",()=>{
    if(state.offset+state.limit<state.total){
      state.offset+=state.limit;loadPage();
    }
  });
  ui.player.addEventListener("change",unlock);
  ui.assign.addEventListener("click",()=>edit("assign"));
  ui.unassign.addEventListener("click",()=>edit("unassign"));
  ui.exclude.addEventListener("click",()=>edit("exclude"));
  ui.restore.addEventListener("click",()=>edit("restore"));
  window.addEventListener("volley:tab-change",event=>{
    if(event.detail?.tab==="players")loadVideos();
  });
  window.addEventListener("volley:project-ready",()=>{
    if(active())loadVideos();
  });
  if(active())loadVideos();
})();