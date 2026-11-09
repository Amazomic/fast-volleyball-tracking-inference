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
    roster:[],writable:false,busy:false,sourceSha:"",groupRevision:null
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
    reidUnlock();
  };
  const reset=()=>{
    state.token++;state.selected.clear();state.offset=0;state.rows=[];
    state.total=0;state.revision="absent";state.roster=[];state.writable=false;
    state.sourceSha="";state.groupRevision=null;
    ui.total.textContent="—";ui.assigned.textContent="—";ui.excluded.textContent="—";
    ui.page.textContent="—";clear(ui.list);
    options(ui.player,[],"id",x=>x,"Игрок не выбран");
    ui.photo.classList.add("hidden");ui.photoImg.removeAttribute("src");
    clear(reviewGroups);reviewStatus.textContent="Выберите обработку CPU.";unlock();
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
      state.sourceSha=String(result.source_replay_sha256);state.groupRevision=null;
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
  // Native appearance matches are only reviewed proposals, never athletes.
  const reviewPanel=node("section","native-reid-review");
  reviewPanel.setAttribute("aria-label","Ручная проверка ReID");
  const reviewTitle=node("h4","","ReID · Группы Track ID");
  const reviewText=node("p","hint","Cosine — сходство, не вероятность личности. Привязка к игроку выполняется отдельно.");
  const reviewTools=node("div","native-reid-controls");
  const reviewThreshold=node("input");
  reviewThreshold.type="number";reviewThreshold.min="55";reviewThreshold.max="99";
  reviewThreshold.step="1";reviewThreshold.value="85";
  reviewThreshold.setAttribute("aria-label","Порог ReID cosine в процентах");
  const reviewRead=node("button","secondary","Предложения");
  const reviewExtract=node("button","secondary","Извлечь признаки");
  const reviewMerge=node("button","secondary","Объединить ID");
  const reviewSplit=node("button","secondary","Разделить ID");
  for(const btn of [reviewRead,reviewExtract,reviewMerge,reviewSplit])btn.type="button";
  reviewTools.append(node("span","","Cosine %"),reviewThreshold,
    reviewRead,reviewExtract,reviewMerge,reviewSplit);
  const reviewStatus=node("p","hint","Выберите обработку CPU, затем получите предложения.");
  const reviewGroups=node("div","native-reid-results");
  reviewPanel.append(reviewTitle,reviewText,reviewTools,reviewStatus,reviewGroups);
  ui.list.parentElement.append(reviewPanel);
  const motionControls=node("div","native-reid-controls");
  const motionStart=node("input");motionStart.type="number";
  motionStart.min="0";motionStart.step="1";motionStart.value="0";
  motionStart.setAttribute("aria-label","Начальный кадр просмотра мяча");
  const motionFrames=node("input");motionFrames.type="number";
  motionFrames.min="1";motionFrames.max="1200";motionFrames.step="1";
  motionFrames.value="300";motionFrames.setAttribute("aria-label","Кадры мяча");
  const motionRead=node("button","secondary","Экстремумы траектории");
  motionRead.type="button";
  motionControls.append(node("span","","Кадр"),motionStart,node("span","","Длина"),motionFrames,motionRead);
  const motionResult=node("div","native-reid-results");
  reviewPanel.append(motionControls,motionResult);
  const continuityControls=node("div","native-reid-controls");
  const continuityGap=node("input");continuityGap.type="number";
  continuityGap.min="0.7";continuityGap.max="4";continuityGap.step="0.1";
  continuityGap.value="4";
  continuityGap.setAttribute("aria-label","Максимальный разрыв в Track ID, секунды");
  const continuityRead=node("button","secondary","Показать разрывы Track ID");
  continuityRead.type="button";
  continuityControls.append(node("span","","Разрыв, с"),continuityGap,continuityRead);
  const continuityOutput=node("div","native-reid-results");
  reviewPanel.append(continuityControls,continuityOutput);
  const reviewScope=()=>({
    project_id:state.project,video:state.video,replay_id:state.replay,
    expected_source_sha256:state.sourceSha,threshold_percent:Number(reviewThreshold.value)
  });
  const reviewReady=()=>Boolean(state.project&&state.video&&state.replay)&&
    /^[a-f0-9]{64}$/.test(state.sourceSha)&&
    Number.isInteger(+reviewThreshold.value)&&
    +reviewThreshold.value>=55&&+reviewThreshold.value<=99;
  const reviewNote=(message,error=false)=>{
    reviewStatus.textContent=message;
    reviewStatus.classList.toggle("track-error",error);
  };
  const reidUnlock=()=>{
    const ready=reviewReady()&&!state.busy;
    reviewRead.disabled=!ready;
    reviewExtract.disabled=!ready||state.selected.size<1||state.selected.size>8;
    reviewMerge.disabled=!ready||!state.writable||!state.groupRevision||state.selected.size<2;
    reviewSplit.disabled=!ready||!state.writable||!state.groupRevision||state.selected.size<1;
  };
  const reviewRender=record=>{
    if(record.schema!=="volley-native-reid-group-review/1"||
      record.confirmed!==false||record.auto_assign_enabled!==false)
      throw Error("Неподдерживаемый формат ReID");
    clear(reviewGroups);
    const groups=Array.isArray(record.groups)?record.groups:[];
    for(const group of groups){
      const ids=(Array.isArray(group.track_ids)?group.track_ids:[])
        .map(Number).filter(id=>Number.isSafeInteger(id)&&id>0);
      if(ids.length<2||ids.length>200)continue;
      const item=node("article","native-reid-group");
      const similarity=Number.isFinite(group.similarity)?
        " · cosine "+Math.round(group.similarity*100)+"%":"";
      const label=node("span","",ids.join(", ")+" · "+
        (group.source==="manual"?"вручную":"предложение")+similarity);
      const choose=node("button","secondary","Выбрать ID");
      choose.type="button";
      choose.addEventListener("click",()=>{
        state.selected=new Set(ids);renderRows();unlock();reidUnlock();
        reviewNote("Выбрано "+ids.length+" ID. Для изменения нажмите нужную кнопку.");
      });
      item.append(label,choose);reviewGroups.append(item);
    }
    reviewNote("Групп: "+groups.length+" · признаков: "+record.embedded_count+
      " · отклонено: совместно в кадре "+(record.co_visible_veto_count||0)+
      ", разные подтверждённые игроки "+(record.confirmed_owner_veto_count||0)+
      ", визуальная неоднозначность "+(record.ambiguous_lookalike_veto_count||0)+
      " · автопривязка отключена.");
  };
  const reviewLoad=async()=>{
    if(!reviewReady()){reviewNote("Выберите CPU-версию и порог 55–99%.",true);return;}
    const scope=reviewScope(),token=state.token;
    reviewNote("Проверяем исходный SHA и ReID-кэш…");
    try{
      const record=await post("/api/players/native/reid-review",{...scope,action:"read"});
      if(token!==state.token||scope.replay_id!==state.replay)return;
      state.groupRevision=String(record.revision);reviewRender(record);
    }catch(e){state.groupRevision=null;clear(reviewGroups);reviewNote(e.message,true);}
    finally{reidUnlock();}
  };
  const reviewEdit=async action=>{
    if(!reviewReady()||state.busy||!state.groupRevision)return;
    const ids=[...state.selected];
    if(!ids.length||ids.length>200||(action==="merge"&&ids.length<2))return;
    const scope=reviewScope(),token=state.token;
    state.busy=true;unlock();reidUnlock();
    try{
      const record=await post("/api/players/native/reid-review",{
        ...scope,action,track_ids:ids,expected_revision:state.groupRevision
      });
      if(token!==state.token||scope.replay_id!==state.replay)return;
      state.groupRevision=String(record.revision);
      state.selected.clear();renderRows();reviewRender(record);
      reviewNote("Ручная группа сохранена. Привязка к игроку не изменена.");
    }catch(e){
      state.groupRevision=null;
      reviewNote(e.message+" Обновите предложения перед новой попыткой.",true);
    }finally{state.busy=false;unlock();reidUnlock();}
  };
  continuityRead.addEventListener("click",async()=>{
    if(!reviewReady()||state.busy)return;
    const gap=Number(continuityGap.value);
    if(!Number.isFinite(gap)||gap<0.7||gap>4){
      reviewNote("Допустимый разрыв: 0.7–4 секунды.",true);return;
    }
    const scope=reviewScope(),token=state.token;
    continuityRead.disabled=true;clear(continuityOutput);
    reviewNote("Проверяем SHA и вычисляем безопасные продолжения Track ID…");
    try{
      const result=await post("/api/players/native/continuity",{
        project_id:scope.project_id,video:scope.video,replay_id:scope.replay_id,
        expected_source_sha256:scope.expected_source_sha256,extended_gap_sec:gap
      });
      if(token!==state.token||scope.replay_id!==state.replay)return;
      if(result.schema!=="volley-native-continuity-preview/1"||
        result.confirmed!==false||result.auto_assign_enabled!==false)
        throw Error("Неподдерживаемый результат непрерывности Track ID");
      const suggestions=Array.isArray(result.report?.extended_suggestions)?
        result.report.extended_suggestions:[];
      for(const pair of suggestions.slice(0,80)){
        const from=Number(pair.from_track_id),to=Number(pair.to_track_id);
        if(!Number.isSafeInteger(from)||!Number.isSafeInteger(to))continue;
        const item=node("div","native-reid-group");
        item.append(node("span","",
          "ID "+from+" → "+to+" · геометрия "+Math.round(Number(pair.score||0)*100)+"%"));
        const choose=node("button","secondary","Выбрать оба");
        choose.type="button";
        choose.addEventListener("click",()=>{
          state.selected=new Set([from,to]);renderRows();unlock();
          reviewNote("Два ID выбраны. Это только кандидат; объединение требует подтверждения.");
        });
        item.append(choose);continuityOutput.append(item);
      }
      reviewNote("Продолжения после 0.7 с: "+suggestions.length+
        " · автоматически подтверждённых игроков: 0.");
    }catch(e){reviewNote(e.message,true);}
    finally{continuityRead.disabled=false;}
  });  motionRead.addEventListener("click",async()=>{
    if(!reviewReady()||state.busy)return;
    const start=Number(motionStart.value),count=Number(motionFrames.value);
    if(!Number.isSafeInteger(start)||start<0||
       !Number.isSafeInteger(count)||count<1||count>1200) {
      reviewNote("Проверка мяча: начальный кадр >= 0, длина 1–1200.",true);return;
    }
    const scope=reviewScope(),token=state.token;
    motionRead.disabled=true;clear(motionResult);
    reviewNote("Проверяем источник мяча и изменение направления/скорости…");
    try{
      const result=await post("/api/cpu/native/ball-extrema",{
        project_id:scope.project_id,video:scope.video,
        replay_id:scope.replay_id,
        expected_source_sha256:scope.expected_source_sha256,
        start_frame:start,count
      });
      if(token!==state.token||scope.replay_id!==state.replay)return;
      if(result.schema!=="volley-native-ball-extrema/1"||
        result.touches_confirmed!==false||result.gpu_inference_rerun!==false)
        throw Error("Неподдерживаемые физические экстремумы мяча");
      for(const impact of (Array.isArray(result.motion_extrema)?result.motion_extrema:[])){
        const row=node("div","native-reid-group");
        row.append(node("span","","Кадр "+impact.frame_index+
          " · отклонение "+Math.round(impact.direction_change_deg)+"° · "+
          "скорость "+impact.speed_before_px_frame.toFixed(1)+" → "+
          impact.speed_after_px_frame.toFixed(1)+" px/кадр"));
        motionResult.append(row);
      }
      reviewNote("Мяч: "+result.observations+" кадров · изменений движения: "+
        result.motion_extrema.length+
        ". Не является подтверждением касания или определения игрока.");
    }catch(e){reviewNote(e.message,true);}
    finally{motionRead.disabled=false;}
  });  reviewRead.addEventListener("click",reviewLoad);
  reviewMerge.addEventListener("click",()=>reviewEdit("merge"));
  reviewSplit.addEventListener("click",()=>reviewEdit("split"));
  reviewThreshold.addEventListener("change",()=>{state.groupRevision=null;reidUnlock();});
  reviewExtract.addEventListener("click",async()=>{
    if(!reviewReady()||state.busy||!state.selected.size||state.selected.size>8)return;
    const scope=reviewScope(),token=state.token;
    state.busy=true;unlock();reidUnlock();reviewNote("Вычисляем реальные признаки OpenVINO…");
    try{
      const result=await post("/api/players/native/embeddings",{
        project_id:scope.project_id,video:scope.video,replay_id:scope.replay_id,
        expected_source_sha256:scope.expected_source_sha256,track_ids:[...state.selected]
      });
      if(token!==state.token||scope.replay_id!==state.replay)return;
      reviewNote("Признаков: "+result.embedding_count+" · кэш: "+result.cache_hits);
    }catch(e){reviewNote(e.message,true);}
    finally{state.busy=false;unlock();reidUnlock();}
  });  ui.video.addEventListener("change",()=>loadRuns().catch(e=>note(e.message,true)));
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