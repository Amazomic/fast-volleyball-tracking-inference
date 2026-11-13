"use strict";
// Native Track ID review bridge. Automatic photo matching uses server-side
// evidence and preserves manual assignments and co-visible player constraints.
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
    token:0,project:"",video:"",replay:"",rallyRun:"",fallback:false,offset:0,limit:24,
    selected:new Set(),rallyIds:new Set(),revision:"absent",rows:[],total:0,
    roster:[],writable:false,busy:false,sourceSha:"",groupRevision:null,
    groups:[],grouped:new Set(),groupLoaded:false,groupShown:24,singleCount:0,
    playerMatches:{},libraryPlayers:[],projectPlayerIds:new Set(),
    rosterEpoch:0,rosterBusy:false,rosterLoaded:false,tracksLoaded:false,loadingKey:null,grouping:false,
    photoIndices:new Map(),viewPlayer:"",viewConflicts:false,filteredTotal:0,unassignedIds:new Set(),identityConflictIds:new Set(),comparisonIds:new Set()
  };
  const node=(tag,klass,text)=>{
    const n=document.createElement(tag);if(klass)n.className=klass;
    if(text!==undefined)n.textContent=String(text);
    return n;
  };
  const clear=n=>{while(n.firstChild)n.firstChild.remove();};
  const path=(base,args)=>base+"?"+new URLSearchParams(args);
  const read=async url=>{
    const response=await window.VolleyballStudioActivity.fetch(url,{credentials:"same-origin",cache:"no-store"});
    const body=await response.json().catch(()=>({}));
    if(!response.ok)throw Error(body.message||body.error||"HTTP "+response.status);
    return body;
  };
  const post=async (url,payload)=>{
    const response=await window.VolleyballStudioActivity.fetch(url,{method:"POST",cache:"no-store",
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
  // Library identities and project membership are different entities. Always
  // show the global library, but require an explicit project edit before a
  // library athlete becomes eligible for manual Track ID assignment.
  const rosterBar=node("section","track-roster-strip");
  rosterBar.setAttribute("aria-label","Игроки библиотеки и текущего проекта");
  const rosterHeader=node("div","track-roster-heading");
  const rosterTitle=node("strong","","Игроки для привязки");
  const rosterAddAll=node("button","secondary","Добавить всех в проект");
  rosterAddAll.type="button";rosterAddAll.disabled=true;
  rosterHeader.append(rosterTitle,rosterAddAll);
  const rosterChips=node("div","track-roster-chips");
  const rosterHint=node("small","track-roster-hint","Чтение состава проекта…");
  rosterBar.append(rosterHeader,rosterChips,rosterHint);
  ui.player.closest(".track-actions").before(rosterBar);
  const renderRosterBar=()=>{
    rosterBar.hidden=!state.project;
    if(!state.project)return;
    const linked=state.libraryPlayers.filter(p=>state.projectPlayerIds.has(p.id));
    const missing=state.libraryPlayers.filter(p=>!state.projectPlayerIds.has(p.id));
    rosterTitle.textContent="Игроки проекта · "+linked.length+
      " из "+state.libraryPlayers.length+" в библиотеке";
    rosterAddAll.hidden=!missing.length;
    rosterAddAll.disabled=state.rosterBusy||!state.rosterLoaded||!missing.length;
    rosterAddAll.textContent="Добавить всех ("+missing.length+")";
    clear(rosterChips);
    const sorted=[...linked,...missing].sort((a,b)=>
      (state.projectPlayerIds.has(b.id)?1:0)-(state.projectPlayerIds.has(a.id)?1:0)||
      a.name.localeCompare(b.name,"ru"));
    for(const person of sorted){
      const attached=state.projectPlayerIds.has(person.id);
      const chip=node("button","track-roster-chip"+(attached?" is-attached":""),
        (attached?"✓ ":"+ ")+person.name);
      chip.type="button";
      chip.disabled=attached||state.rosterBusy||!state.rosterLoaded;
      chip.title=attached?"Уже добавлен в проект":"Добавить "+person.name+" в состав проекта";
      if(!attached)chip.addEventListener("click",()=>attachPlayers([person.id]));
      rosterChips.append(chip);
    }
    rosterHint.textContent=!state.rosterLoaded?
      "Читаем библиотеку и состав проекта…":
      !state.libraryPlayers.length?"В библиотеке нет игроков. Добавьте их ниже.":
      !missing.length?"Все игроки библиотеки уже включены в проект.":
      "✓ В проекте · + Только в библиотеке. Нажмите +, чтобы разрешить привязку Track ID.";
  };
  const showPlayerChoices=()=>{
    const chosen=ui.player.value;
    const pool=state.libraryPlayers.length?state.libraryPlayers:state.roster;
    options(ui.player,pool,"id",p=>p.name+
      (state.projectPlayerIds.has(p.id)?"":" · в библиотеке"),"Выберите игрока");
    if(pool.some(p=>p.id===chosen))ui.player.value=chosen;
    const title=ui.player.closest(".track-actions")?.querySelector(".field-label");
    if(title)title.textContent="Игрок из библиотеки";
    unlock();
  };
  const loadLibraryRoster=async()=>{
    const project=state.project,token=++state.rosterEpoch;
    if(!project||state.rosterBusy)return;
    try{
      const [catalog,library]=await Promise.all([
        read("/api/projects"),post("/api/players/library/native",{action:"read"})
      ]);
      if(token!==state.rosterEpoch||project!==state.project)return;
      const projectDoc=(Array.isArray(catalog.projects)?catalog.projects:[]).
        find(row=>row.id===project);
      if(!projectDoc||!Array.isArray(projectDoc.player_ids)||
        !Array.isArray(library.players))throw Error("Список игроков проекта недоступен");
      state.libraryPlayers=library.players.filter(person=>
        typeof person.id==="string"&&typeof person.name==="string");
      state.projectPlayerIds=new Set(projectDoc.player_ids);
      window.dispatchEvent(new CustomEvent("volley:roster-updated",{detail:{
        project_id:project,player_ids:[...state.projectPlayerIds]
      }}));
      state.rosterLoaded=true;
      state.roster=state.libraryPlayers.filter(p=>state.projectPlayerIds.has(p.id));
      showPlayerChoices();
      renderRosterBar();
      if(state.rows.length){renderRows();renderGroupCards();unlock();}
    }catch(error){
      if(token!==state.rosterEpoch||project!==state.project)return;
      rosterHint.textContent="Ошибка чтения библиотеки: "+error.message;
      rosterHint.classList.add("track-error");
    }
  };
  const attachPlayers=async ids=>{
    const project=state.project;
    if(!project||state.rosterBusy||!state.rosterLoaded||!ids.length)return false;
    state.rosterBusy=true;state.rosterEpoch++;
    renderRosterBar();
    try{
      // Refresh authoritative membership before union so an update never
      // drops someone another tab just added.
      const catalog=await read("/api/projects");
      const projectDoc=(Array.isArray(catalog.projects)?catalog.projects:[]).
        find(row=>row.id===project);
      if(!projectDoc||!Array.isArray(projectDoc.player_ids))
        throw Error("Проект недоступен");
      const known=new Set(state.libraryPlayers.map(p=>p.id));
      if(ids.some(id=>!known.has(id)))throw Error("Игрок отсутствует в библиотеке");
      const updatedIds=[...new Set([...projectDoc.player_ids,...ids])];
      const response=await post("/api/project-update",{
        id:project,player_ids:updatedIds
      });
      if(project!==state.project)return false;
      if(response.project?.id!==project||
        !Array.isArray(response.project?.player_ids)||
        ids.some(id=>!response.project.player_ids.includes(id)))
        throw Error("Сервер не подтвердил добавление игроков");
      state.projectPlayerIds=new Set(response.project.player_ids);
      state.roster=state.libraryPlayers.filter(p=>state.projectPlayerIds.has(p.id));
      showPlayerChoices();
      renderRows();renderGroupCards();unlock();
      note(ids.length===1?"Игрок добавлен в состав проекта.":
        "Игроки добавлены в состав проекта.");
      return true;
    }catch(error){
      if(project===state.project)note("Не удалось добавить игроков: "+error.message,true);
      return false;
    }finally{
      if(project===state.project){
        state.rosterBusy=false;renderRosterBar();
        // Reconcile the roster and any concurrent project edits.
        void loadLibraryRoster();
      }
    }
  };
  window.addEventListener("volley:remove-project-player",async event=>{
    const detail=event.detail||{},project=state.project,id=detail.player_id;
    if(detail.project_id!==project||!state.projectPlayerIds.has(id)||state.rosterBusy)return;
    state.rosterBusy=true;state.rosterEpoch++;renderRosterBar();
    try{
      const catalog=await read("/api/projects");
      const projectDoc=(Array.isArray(catalog.projects)?catalog.projects:[]).find(row=>row.id===project);
      if(!projectDoc||!Array.isArray(projectDoc.player_ids))throw Error("Проект недоступен");
      const response=await post("/api/project-update",{id:project,player_ids:projectDoc.player_ids.filter(value=>value!==id)});
      if(project!==state.project)return;
      if(!Array.isArray(response.project?.player_ids)||response.project.player_ids.includes(id))throw Error("Сервер не подтвердил удаление игрока");
      if(state.viewPlayer===id)viewBack.click();
      note("Игрок удалён из проекта.");
    }catch(error){if(project===state.project)note("Не удалось удалить игрока: "+error.message,true);}
    finally{if(project===state.project){state.rosterBusy=false;void loadLibraryRoster();}}
  });
  rosterAddAll.addEventListener("click",()=>attachPlayers(
    state.libraryPlayers.filter(p=>!state.projectPlayerIds.has(p.id)).map(p=>p.id)
  ));
  const updateCardLink=controls=>{
    const select=controls.querySelector("select");
    const link=controls.querySelector("button");
    link.disabled=!state.writable||state.busy||state.rosterBusy||
      !select.value||Number(controls.dataset.trackCount)>200;
  };
  const unlock=()=>{
    const selected=state.selected.size>0 && !state.busy && state.writable;
    ui.assign.disabled=!selected||!ui.player.value||state.rosterBusy;
    ui.unassign.disabled=!selected;
    ui.exclude.disabled=!selected;
    ui.restore.disabled=!selected;
    ui.prev.disabled=state.busy||state.offset===0;
    ui.next.disabled=state.busy||state.offset+state.limit>=state.filteredTotal;
    tracksPanel.querySelectorAll(".track-card-link-actions").forEach(updateCardLink);
    tracksPanel.querySelectorAll(".track-remove-assignment").forEach(button=>{
      button.disabled=state.busy||!state.writable;
    });
    reidUnlock();
  };
  const reset=()=>{
    state.token++;state.selected.clear();state.rallyIds.clear();state.offset=0;state.rows=[];
    state.total=0;state.revision="absent";state.roster=[];state.writable=false;
    state.tracksLoaded=false;state.loadingKey=null;state.grouping=false;
    state.photoIndices.clear();
    state.sourceSha="";state.groupRevision=null;state.rallyRun="";state.fallback=false;
    state.groups=[];state.grouped.clear();state.groupLoaded=false;
    state.groupShown=24;state.singleCount=0;state.playerMatches={};
    state.viewPlayer="";state.viewConflicts=false;state.filteredTotal=0;state.unassignedIds.clear();state.identityConflictIds.clear();
    state.comparisonIds.clear();
    updateUnassignedLabel();viewBack.hidden=true;groupsMode.hidden=false;playerMode.hidden=true;setMode("singles");
    state.rosterEpoch++;state.libraryPlayers=[];state.projectPlayerIds.clear();
    state.rosterLoaded=false;state.rosterBusy=false;
    renderRosterBar();
    galleryStatus.textContent="Группы загружаются из выбранной CPU-версии.";
    clear(galleryItems);galleryMore.hidden=true;
    ui.total.textContent="—";ui.assigned.textContent="—";ui.excluded.textContent="—";
    ui.page.textContent="—";clear(ui.list);
    options(ui.player,[],"id",x=>x,"Игрок не выбран");
    ui.photo.classList.add("hidden");ui.photoImg.removeAttribute("src");
    clear(reviewGroups);reviewStatus.textContent="Выберите обработку CPU.";unlock();
  };
  // Manual Track ID tools follow the ONLY current source chain in the header.
  // Hidden selects remain as existing internal adapters, never alternate choices.
  const loadVideos=async()=>{
    const project=ui.project.value,selected=get("studio-run")?.value||"";
    const loadingKey=JSON.stringify([project,selected]);
    if(state.loadingKey===loadingKey)return;
    if(project&&state.project===project&&state.replay&&state.tracksLoaded&&
      (selected?state.rallyRun===selected:state.fallback)){
      void loadLibraryRoster();
      return;
    }
    reset();state.project=project;state.video="";state.replay="";
    state.loadingKey=loadingKey;
    if(project)void loadLibraryRoster();
    get("studio-track-source").textContent="Выберите постобработку в шапке.";
    options(ui.video,[],"name",x=>x,"Источник из шапки");
    options(ui.replay,[],"id",x=>x,"Источник из шапки");
    if(!project){
      state.loadingKey=null;note("Выберите проект в верхней панели.");return;
    }
    const token=state.token;
    note("Проверка CPU-источника выбранной постобработки…");
    try{
      const response=await read(path("/api/cpu/native/rallies/runs",{project_id:project}));
      if(token!==state.token||project!==ui.project.value||
          selected!==get("studio-run")?.value)return;
      const runs=(Array.isArray(response.runs)?response.runs:[]).filter(item=>
        typeof item.video==="string"&&
        /^[a-f0-9]{32}$/.test(String(item.rally_run_id||""))&&
        /^[a-f0-9]{32}$/.test(String(item.source_replay_id||"")));
      // With no GPU lineage selected in the header, player review remains
      // available from the newest explicitly named CPU snapshot.
      // This does not change the source chain used by preview or montage.
      const row=selected?runs.find(item=>item.rally_run_id===selected):
        [...runs].sort((a,b)=>Number(b.created_at||0)-Number(a.created_at||0))[0];
      if(!row){
        note(selected?"В этой постобработке нет Track ID.":
          "В проекте пока нет сохранённой CPU-версии для просмотра игроков.",true);
        return;
      }
      state.video=row.video;state.replay=row.source_replay_id;
      state.rallyRun=row.rally_run_id;state.fallback=!selected;
      options(ui.video,[{name:row.video}],"name",item=>item.name,"Нет видео");
      ui.video.value=row.video;
      options(ui.replay,[{id:row.source_replay_id}],"id",item=>item.id,"Нет Track ID");
      ui.replay.value=row.source_replay_id;
      const sourceText=get("studio-track-source");
      const shortVideo=row.video.length>43?
        row.video.slice(0,23)+"…"+row.video.slice(-14):row.video;
      sourceText.textContent=
        "CPU "+row.rally_run_id.slice(0,12)+" · "+shortVideo+
        " · только кадры розыгрышей"+
        (state.fallback?" · последняя версия проекта":"");
      sourceText.title=row.video+" · GPU/CPU версия "+row.rally_run_id+
        (state.fallback?" · источник не выбран в шапке":"");
      await loadPage();
    }catch(error){if(token===state.token)note(error.message,true);}
    finally{if(token===state.token)state.loadingKey=null;}
  };
  const publishTrackCovers=playerTrackCounts=>{
    if(!state.project||!state.video||!state.replay)return;
    window.dispatchEvent(new CustomEvent("volley:track-photo-covers",{detail:{
      project_id:state.project,video:state.video,replay_id:state.replay,
      player_track_counts:playerTrackCounts,
      tracks:state.rows.filter(row=>
        typeof row.assigned_player_id==="string"&&
        Number.isSafeInteger(Number(row.track_id))&&Number(row.track_id)>0
      ).map(row=>({player_id:row.assigned_player_id,track_id:Number(row.track_id)}))
    }}));
  };
  const animateTrackCards=update=>{
    const cards=()=>[...ui.list.querySelectorAll("[data-track-card-key]"),
      ...galleryItems.querySelectorAll("[data-track-card-key]")];
    const previous=new Map(cards().map(card=>[card.dataset.trackCardKey,{
      rect:card.getBoundingClientRect(),portraits:[...card.querySelectorAll(".track-portrait")]
    }]));
    update();
    const motion=document.body.dataset.uiMotion!=="reduce"&&
      !window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    for(const card of cards()){
      const old=previous.get(card.dataset.trackCardKey);
      if(old){
        const portraits=[...card.querySelectorAll(".track-portrait")];
        portraits.forEach((portrait,index)=>{
          if(old.portraits[index]?.title===portrait.title)portrait.replaceWith(old.portraits[index]);
        });
      }
      if(!motion||typeof card.animate!=="function")continue;
      if(old){
        const rect=card.getBoundingClientRect();
        const x=old.rect.left-rect.left,y=old.rect.top-rect.top;
        if(x||y)card.animate([{transform:`translate(${x}px, ${y}px)`},{transform:"translate(0, 0)"}],
          {duration:220,easing:"ease-out"});
      }else card.animate([{opacity:0},{opacity:1}],{duration:160,easing:"ease-out"});
    }
  };
  const loadPage=async({preserveCards=false}={})=>{
    const project=state.project,video=state.video,replay=state.replay;
    if(!project||!video||!replay)return;
    const token=++state.token;
    state.selected.clear();unlock();
    if(!preserveCards)ui.list.replaceChildren(node("div","empty-state","Проверка SHA-256 и загрузка Track ID…"));
    note("Проверяем исходный CPU-трекинг, получаем имена игроков…");
    try{
      const result=await read(path("/api/players/native/tracks",{
        project_id:project,video,replay_id:replay,rally_run_id:state.rallyRun,
        offset:state.offset,limit:state.limit,view:state.viewPlayer?"player":state.viewConflicts?"conflict":"unassigned",
        ...(state.viewPlayer?{view_player_id:state.viewPlayer}:{})
      }));
      if(token!==state.token||project!==state.project||
        video!==state.video||replay!==state.replay)return;
      if(result.schema!=="volley-native-player-track-links/1"||
        result.source_replay_sha256?.length!==64||
        typeof result.automatic_reid_binding!=="boolean")throw Error("Некорректный источник Track ID");
      state.rows=Array.isArray(result.tracks)?result.tracks:[];
      if(!Array.isArray(result.rally_track_ids)||result.rally_track_ids.length>20000)
        throw Error("Сервер не подтвердил ID внутри розыгрышей");
      state.rallyIds=new Set(result.rally_track_ids.map(Number));
      if(state.rallyIds.size!==Number(result.total_tracks))
        throw Error("Некорректная выборка ID выбранных розыгрышей");
      state.total=state.rallyIds.size;
      if(!Number.isSafeInteger(result.filtered_total)||result.filtered_total<0||
        !Array.isArray(result.unassigned_track_ids)||!Array.isArray(result.comparison_track_ids))
        throw Error("Сервер не подтвердил фильтр привязок Track ID");
      state.filteredTotal=result.filtered_total;
      state.unassignedIds=new Set(result.unassigned_track_ids.map(Number));
      state.identityConflictIds=new Set((result.identity_conflict_track_ids||[]).map(Number));
      state.comparisonIds=new Set(result.comparison_track_ids.map(Number));
      state.roster=Array.isArray(result.roster)?result.roster:[];
      const savedMatches=result.automatic_identity_matches;
      state.playerMatches=savedMatches&&typeof savedMatches==="object"&&!Array.isArray(savedMatches)?
        savedMatches:{};
      state.revision=String(result.revision||"absent");
      if(state.sourceSha&&state.sourceSha!==String(result.source_replay_sha256)){
        state.groupRevision=null;state.groupLoaded=false;
        state.groups=[];state.grouped.clear();state.playerMatches={};
      }
      state.sourceSha=String(result.source_replay_sha256);
      state.tracksLoaded=true;
      updateUnassignedLabel();
      state.writable=result.writable===true;
      ui.total.textContent=String(state.total);
      ui.assigned.textContent=String(result.assigned_count||0);
      ui.excluded.textContent=String(result.excluded_count||0);
      publishTrackCovers(result.player_track_counts);
      showPlayerChoices();
      if(preserveCards&&reviewReady())await reviewLoad({deferRender:true});
      if(token!==state.token||project!==state.project||video!==state.video||replay!==state.replay)return;
      const updateCards=()=>{renderRosterBar();renderRows();renderGroupCards();unlock();};
      if(preserveCards)animateTrackCards(updateCards);else updateCards();
      if(!preserveCards&&!state.groupLoaded&&reviewReady())await reviewLoad();
      note(state.roster.length?
        "Track ID получены. Выберите ID и вручную назначьте игрока из состава проекта.":
        "Добавьте игроков в состав проекта через управление проектом, чтобы разрешить ручную привязку.");
    }catch(error){if(!preserveCards)ui.list.replaceChildren(node("div","empty-state","Не удалось загрузить Track ID."));
      note(error.message,true);}
  };
  const select=(track,yes)=>{
    if(yes){
      if(state.selected.size>=200){note("Не более 200 Track ID за операцию.",true);return;}
      state.selected.add(track);
    }else state.selected.delete(track);
    unlock();
  };
  const openPhoto=(id,frame)=>{
    ui.photo.classList.remove("hidden");ui.photoId.textContent=String(id);
    ui.photoImg.alt="Обнаружение Track ID "+id+(Number.isSafeInteger(frame)?
      " · кадр "+frame:"");
    ui.photoImg.src=path("/api/players/native/track-photo",{
      project_id:state.project,video:state.video,
      replay_id:state.replay,rally_run_id:state.rallyRun,track_id:String(id)
    });
  };
  const portrait=(id,extra="")=>{
    const button=node("button","track-portrait "+extra);
    button.type="button";
    button.title="Следующее фото Track ID "+id;
    button.setAttribute("aria-label",button.title);
    const image=node("img");
    image.alt="Игрок, Track ID "+id;
    image.loading="lazy";image.decoding="async";
    const fallback=node("span","track-portrait-fallback","Фото недоступно");
    fallback.hidden=true;
    image.addEventListener("error",()=>{image.hidden=true;fallback.hidden=false;});
    let photoIndex=state.photoIndices.get(id)||0;
    const showPhoto=()=>{
      image.hidden=false;fallback.hidden=true;
      image.src=path("/api/players/native/track-photo",{
        project_id:state.project,video:state.video,replay_id:state.replay,
        rally_run_id:state.rallyRun,track_id:String(id),photo_index:String(photoIndex)
      });
    };
    showPhoto();
    button.append(image,fallback);
    button.addEventListener("click",()=>{
      photoIndex=(photoIndex+1)%13;state.photoIndices.set(id,photoIndex);showPhoto();
    });
    return button;
  };
  const playerCandidates=ids=>{
    const members=new Set(ids);
    const roster=new Map(state.roster.map(p=>[p.id,p.name]));
    const byPlayer=new Map();
    for(const id of members){
      const options=state.playerMatches[String(id)];
      for(const candidate of Array.isArray(options)?options:[]){
        const score=Number(candidate.cosine_similarity);
        if(!roster.has(candidate.player_id)||!Number.isFinite(score)||
          score<=0||score>1)continue;
        const current=byPlayer.get(candidate.player_id);
        if(!current||score>current.score){
          byPlayer.set(candidate.player_id,{
            playerId:candidate.player_id,name:roster.get(candidate.player_id),
            score,trackId:id,reference: candidate.reference_track_id
          });
        }
      }
    }
    return [...byPlayer.values()].sort((a,b)=>
      b.score-a.score||a.name.localeCompare(b.name,"ru")).slice(0,3);
  };
  const attachSuggestions=(card,ids)=>{
    const panel=node("div","track-link-suggestions");
    const candidates=playerCandidates(ids);
    const controls=node("div","track-card-link-actions");
    controls.dataset.trackCount=String(ids.length);
    const select=node("select");
    select.setAttribute("aria-label","Назначить игрока для "+ids.map(id=>"ID "+id).join(", "));
    if(!candidates.length){
      const empty=node("option","","Выберите игрока");empty.value="";select.append(empty);
    }
    for(const match of candidates){
      const option=node("option","",match.name+" · "+(match.score*100).toFixed(1)+"%");
      option.value=match.playerId;select.append(option);
    }
    const matched=new Set(candidates.map(match=>match.playerId));
    for(const person of state.roster){
      if(matched.has(person.id))continue;
      const option=node("option","",person.name);option.value=person.id;
      select.append(option);
    }
    if(candidates.length)select.value=candidates[0].playerId;
    const similarity=node("small","track-match-score",candidates.length?
      "Лучшее сходство: "+candidates[0].name+" · "+(candidates[0].score*100).toFixed(1)+"%":
      "Сходство не рассчитано");
    const link=node("button","primary","Добавить игроку");
    link.type="button";
    const updateLink=()=>updateCardLink(controls);
    select.addEventListener("change",updateLink);
    link.addEventListener("click",()=>bindSelected(ids,select.value));
    controls.append(select,link);
    updateLink();
    panel.append(similarity,controls);
    card.append(panel);
  };
  const renderRows=()=>{
    clear(ui.list);
    ui.page.textContent=state.filteredTotal?
      (state.offset+1)+"–"+Math.min(state.offset+state.limit,state.filteredTotal)+" из "+state.filteredTotal:
      "Нет Track ID";
    const singles=state.rows;
    singlesHeader.textContent="Отдельные Track ID · "+
      state.filteredTotal+
      " · на странице "+singles.length;
    if(!state.rows.length){
      ui.list.append(node("div","empty-state",state.viewPlayer?
        "У игрока нет привязанных Track ID в выбранной обработке.":
        "Нет непривязанных Track ID в выбранной обработке."));
    }else if(!singles.length){
      ui.list.append(node("div","empty-state",
        "На этой странице ID входят в группы. Перейдите к следующей странице."));
    }
    for(const row of singles){
      const id=Number(row.track_id);
      if(!Number.isSafeInteger(id)||id<=0)continue;
      const card=node("article","native-track-item native-track-photo-card"+
        (row.excluded?" excluded":""));
      card.dataset.trackCardKey="single:"+id;
      const media=node("div","track-photo-card-media");
      const check=node("input");check.type="checkbox";
      check.checked=state.selected.has(id);
      check.setAttribute("aria-label","Выбрать Track ID "+id);
      check.addEventListener("change",()=>select(id,check.checked));
      media.append(portrait(id),check);
      const info=node("div","track-card-content");
      const heading=node("div","track-card-title");
      heading.append(node("strong","","ID "+id),
        node("span","track-owner"+(row.assigned_player_name?" assigned":""),
          row.excluded?"Исключён":row.assigned_player_name?(row.assigned_player_name+(row.identity_source==="automatic_photo"?" · авто":"")):row.identity_conflict?"Конфликт одновременной видимости":"Не привязан"));
      const meta=node("small","track-card-observations",
        (row.observations??0)+" кадров · "+
        (row.first_frame??0)+"–"+(row.last_frame??0)+" · "+
        Math.round((row.best_score??0)*100)+"% детектор");
      info.append(heading,meta);
      card.append(media,info);
      // An assigned ID has one clear action: unlink it from its current player.
      // Do not offer "Add player" for IDs which already have an owner.
      if(row.assigned_player_id){
        const remove=node("button","secondary track-remove-assignment","✕ Снять привязку");
        remove.type="button";
        remove.disabled=state.busy||!state.writable;
        remove.title="Отвязать Track ID "+id+" от игрока "+row.assigned_player_name;
        remove.setAttribute("aria-label",remove.title);
        remove.addEventListener("click",()=>void edit("unassign",[id]));
        card.append(remove);
      }else{
        attachSuggestions(card,[id]);
      }
      ui.list.append(card);
    }
  };
  const edit=async (action,chosenIds=null,chosenOwner=null)=>{
    const ids=chosenIds||Array.from(state.selected);
    const owner=chosenOwner||ui.player.value;
    if(state.busy||!ids.length||ids.length>200||!state.replay)return;
    if(action==="assign"&&!owner){note("Выберите игрока.",true);return;}
    const project=state.project,video=state.video,replay=state.replay;
    const revision=state.revision,offset=state.offset;
    state.busy=true;unlock();
    try{
      const payload={project_id:project,video,replay_id:replay,
        rally_run_id:state.rallyRun,
        action,track_ids:ids,expected_revision:revision,offset,limit:state.limit};
      if(action==="assign")payload.player_id=owner;
      const result=await post("/api/players/native/tracks",payload);
      if(project!==state.project||video!==state.video||replay!==state.replay)return;
      state.revision=String(result.revision);
      window.dispatchEvent(new CustomEvent("volley:track-assignments-updated",{detail:{
        project_id:project,video,replay_id:replay
      }}));
      state.selected.clear();
      state.rows=Array.isArray(result.tracks)?result.tracks:[];
      ui.assigned.textContent=String(result.assigned_count||0);
      ui.excluded.textContent=String(result.excluded_count||0);
      state.offset=0;
      await loadPage({preserveCards:true});
      note("Привязки обновлены. Исходные GPU/CPU данные не изменены.");
      return true;
    }catch(e){
      note(e.message,true);
      // Preserve refusal, and refetch canonical state on conflict rather than
      // performing an optimistic hidden reassignment.
    }finally{state.busy=false;unlock();}
  };
  const bindSelected=async (ids,playerId)=>{
    if(!playerId||state.busy||state.rosterBusy||!ids.length||
      !state.projectPlayerIds.has(playerId))return;
    const scope={project_id:state.project,player_id:playerId,video:state.video,
      replay_id:state.replay,rally_run_id:state.rallyRun,
      photo_index:state.photoIndices.get(ids[0])||0,expected_source_sha256:state.sourceSha};
    if(!await edit("assign",ids,playerId))return;
    if(scope.project_id!==state.project||scope.replay_id!==state.replay)return;
    state.busy=true;unlock();
    try{
      const album=await post("/api/players/native/references",{
        project_id:scope.project_id,player_id:playerId,action:"read"
      });
      const existing=Array.isArray(album.photos)&&album.photos.some(photo=>
        photo.project_id===scope.project_id&&photo.video===scope.video&&
        photo.replay_id===scope.replay_id&&photo.track_id===ids[0]&&
        photo.rally_run_id===scope.rally_run_id&&
        (photo.photo_index||0)===scope.photo_index&&
        photo.source_replay_sha256===scope.expected_source_sha256);
      if(!existing)await post("/api/players/native/references",{
        ...scope,action:"enroll",track_id:ids[0],expected_revision:album.revision
      });
      window.dispatchEvent(new Event("volley:player-photo-saved"));
      if(scope.project_id===state.project&&scope.replay_id===state.replay)
        note("Track ID привязаны, фото добавлено игроку.");
    }catch(error){
      if(scope.project_id===state.project&&scope.replay_id===state.replay)
        note("Track ID привязаны, но фото не сохранено: "+error.message,true);
    }finally{state.busy=false;unlock();}
  };
  // Native appearance matches are only reviewed proposals, never athletes.
  window.addEventListener("volley:enroll-selected-photo",event=>{
    const detail=event.detail||{};
    if(detail.project_id!==state.project||!state.projectPlayerIds.has(detail.player_id))return;
    if(state.selected.size!==1){
      note("Выберите один Track ID, фотографию которого хотите привязать.",true);
      return;
    }
    bindSelected([...state.selected],detail.player_id);
  });
  const reviewPanel=node("section","native-reid-review");
  reviewPanel.setAttribute("aria-label","Ручная проверка ReID");
  const reviewTitle=node("h4","","Управление ReID-группами");
  const reviewText=node("p","hint","Cosine — сходство, не вероятность личности. Привязка к игроку выполняется отдельно.");
  const reviewTools=node("div","native-reid-controls");
  const reviewThreshold=node("input");
  reviewThreshold.type="range";reviewThreshold.min="1";reviewThreshold.max="99";
  reviewThreshold.step="1";reviewThreshold.value="30";
  reviewThreshold.setAttribute("aria-label","Порог ReID cosine в процентах");
  const reviewRead=node("button","secondary","Обновить группы");
  const reviewExtract=node("button","secondary","Извлечь признаки");
  const reviewMerge=node("button","secondary","Объединить ID");
  const reviewSplit=node("button","secondary","Разделить ID");
  for(const btn of [reviewRead,reviewExtract,reviewMerge,reviewSplit])btn.type="button";
  reviewTools.append(node("span","","Cosine %"),
    reviewRead,reviewExtract,reviewMerge,reviewSplit);
  const reviewStatus=node("p","hint","Выберите обработку CPU, затем получите предложения.");
  const reviewGroups=node("div","native-reid-results");
  reviewPanel.append(reviewTitle,reviewText,reviewTools,reviewStatus);
  ui.list.parentElement.append(reviewPanel);
  // Keep the Track ID photo gallery dominant, with a compact athlete
  // sidebar. The global library editor (including non-project athletes) stays
  // in one separate section below both columns.
  const playersView=get("studio-players-view");
  const libraryForm=get("studio-player-create");
  const tracksPanel=get("studio-native-track-panel");
  const libraryList=get("studio-player-list");
  const libraryBar=get("studio-player-refresh")?.closest(".section-bar");
  if(playersView&&libraryForm&&tracksPanel&&libraryList&&libraryBar){
    const layout=node("div","players-workspace-layout");
    const main=node("div","players-gallery-column");
    const sidebar=node("aside","players-sidebar-column");
    sidebar.setAttribute("aria-label","Игроки проекта");
    const title=node("h3","","Игроки проекта");
    const sidebarNote=node("p","players-sidebar-note",
      "Выберите игрока на карточке Track ID");
    const sidebarCards=node("div","players-sidebar-list");
    sidebarCards.id="studio-players-sidebar-list";
    sidebarCards.append(node("span","players-sidebar-loading","Загрузка игроков…"));
    const heading=node("div","players-sidebar-heading");
    const add=node("button","players-sidebar-add","+");
    add.type="button";add.setAttribute("aria-label","Добавить игроков в проект");
    const picker=node("div","players-sidebar-picker");picker.hidden=true;
    add.addEventListener("click",()=>{
      picker.hidden=!picker.hidden;
      add.setAttribute("aria-expanded",String(!picker.hidden));
      if(picker.hidden)return;
      clear(picker);
      const missing=state.libraryPlayers.filter(p=>!state.projectPlayerIds.has(p.id));
      if(!missing.length)picker.append(node("span","","Все игроки библиотеки уже в проекте."));
      for(const player of missing){
        const choice=node("button","",player.name);choice.type="button";
        choice.addEventListener("click",async()=>{
          choice.disabled=true;
          if(await attachPlayers([player.id])){closePicker();}
          else choice.disabled=false;
        });
        picker.append(choice);
      }
      const create=node("button","","Создать игрока");create.type="button";
      create.addEventListener("click",async()=>{
        const name=window.prompt("Имя нового игрока");
        if(name===null||!name.trim())return;
        create.disabled=true;
        try{
          const current=await post("/api/players/library/native",{action:"read"});
          const before=new Set((Array.isArray(current.players)?current.players:[]).map(p=>p.id));
          const result=await post("/api/players/library/native",{action:"create",name:name.trim(),expected_revision:current.revision});
          const players=Array.isArray(result.players)?result.players:[];
          const created=players.find(p=>!before.has(p.id));
          if(!created)throw Error("Сервер не подтвердил создание игрока");
          state.libraryPlayers=players;
          window.dispatchEvent(new Event("volley:player-library-updated"));
          window.dispatchEvent(new Event("volley:player-photo-saved"));
          if(await attachPlayers([created.id]))closePicker();
          else note("Игрок создан в библиотеке. Добавьте его в проект через +.",true);
        }catch(error){note("Не удалось создать игрока: "+error.message,true);}
        finally{create.disabled=false;}
      });
      picker.append(create);
    });
    heading.append(title,add);
    const resetLinks=node("button","secondary players-reset-track-links","Сброс привязки ИД");
    resetLinks.type="button";
    resetLinks.addEventListener("click",async()=>{
      if(!state.project||state.busy||state.rosterBusy)return;
      const project=state.project;
      if(!window.confirm("Сбросить все привязки Track ID к игрокам в проекте? Игроки и фотографии сохранятся."))return;
      resetLinks.disabled=true;
      state.busy=true;unlock();
      try{
        const result=await post("/api/players/native/tracks/reset-project",{project_id:project});
        if(project!==state.project)return;
        state.playerMatches={};
        state.selected.clear();
        window.dispatchEvent(new CustomEvent("volley:track-assignments-updated",{detail:{project_id:project}}));
        note("Сброшено привязок: "+result.reset_tracks+" · версий: "+result.reset_replays+". Игроки и фотографии сохранены.");
        await loadPage({preserveCards:true});
        await loadLibraryRoster();
      }catch(error){
        if(project===state.project)note("Не удалось сбросить привязки: "+error.message,true);
      }finally{
        resetLinks.disabled=false;state.busy=false;unlock();
      }
    });
    sidebar.append(heading,sidebarNote,picker,sidebarCards,resetLinks);
    add.setAttribute("aria-expanded","false");
    const closePicker=()=>{picker.hidden=true;add.setAttribute("aria-expanded","false");};
    document.addEventListener("pointerdown",event=>{
      if(!picker.hidden&&!sidebar.contains(event.target))closePicker();
    });
    document.addEventListener("keydown",event=>{if(event.key==="Escape")closePicker();});
    main.append(tracksPanel);
    layout.append(main,sidebar);
    playersView.insertBefore(layout,libraryForm);
    const management=node("section","players-library-management");
    management.setAttribute("aria-label","Управление общей библиотекой игроков");
    management.append(node("h2","","Управление игроками"));
    management.append(node("p","players-management-note",
      "Создание, переименование и удаление доступны для всех игроков библиотеки, включая не добавленных в текущий проект. Удаление сохраняет исторические привязки и фото."));
    management.append(libraryForm,libraryBar,libraryList);
    playersView.append(management);
  }
  // Both sets are visible without an extra "Suggestions" click. Avoid
  // rendering thousands of rows at once; standalone IDs stay paginated.
  const groupGallery=node("section","native-track-group-gallery");
  groupGallery.setAttribute("aria-label","Сгруппированные Track ID");
  const groupTitle=node("h3","","Сгруппированные ID");
  const galleryStatus=node("p","panel-note","Группы загружаются из выбранной CPU-версии.");
  const galleryItems=node("div","native-track-group-items");
  const galleryExtract=node("button","secondary","Извлечь ReID для выбранных ID");
  galleryExtract.type="button";galleryExtract.disabled=true;
  galleryExtract.addEventListener("click",()=>reviewExtract.click());
  const galleryRepeat=node("button","secondary","Повторить анализ");
  galleryRepeat.type="button";galleryRepeat.disabled=true;
  galleryRepeat.addEventListener("click",()=>repeatPhotoAnalysis());
  const galleryAuto=node("button","primary","Автопривязать похожие ID");
  galleryAuto.type="button";galleryAuto.disabled=true;
  galleryAuto.addEventListener("click",()=>autoAssignSimilar());
  const galleryMore=node("button","secondary track-group-more","Показать ещё группы");
  galleryMore.type="button";galleryMore.hidden=true;
  groupGallery.append(groupTitle,galleryStatus,galleryRepeat,galleryAuto,galleryExtract,galleryItems,galleryMore);
  const modeSwitch=node("div","track-mode-switch");
  const modeTabs=node("div","track-mode-tabs");
  modeTabs.setAttribute("role","group");
  modeTabs.setAttribute("aria-label","Вид Track ID");
  const singlesMode=node("button","track-mode-button","ID");
  const groupsMode=node("button","track-mode-button","Группы ID");
  for(const button of [singlesMode,groupsMode])button.type="button";
  const playerMode=node("button","track-mode-button","Игрок");
  playerMode.type="button";playerMode.hidden=true;
  const playerGallery=node("section","player-saved-gallery");playerGallery.hidden=true;
  const playerPhotos=node("div","player-saved-photos");
  const playerStatus=node("p","hint");
  playerGallery.append(playerStatus,playerPhotos);
  const loadAlbum=async()=>{
    const project=state.project,player=state.viewPlayer;
    clear(playerPhotos);playerStatus.textContent="Загрузка фотографий…";
    try{
      const album=await post("/api/players/native/references",{project_id:project,player_id:player,action:"read"});
      if(project!==state.project||player!==state.viewPlayer)return;
      const photos=Array.isArray(album.photos)?album.photos:[];
      playerStatus.textContent=photos.length?"Сохранённые фотографии":"Сохранённых фотографий пока нет.";
      for(const photo of photos){
        const card=node("article","player-saved-photo");
        const img=node("img");img.loading="lazy";img.alt="Сохранённое фото игрока";
        img.src=path("/api/players/library/photo",{player_id:player,reference_id:photo.id});
        const remove=node("button","secondary","Удалить фото");remove.type="button";
        remove.disabled=!album.writable;
        remove.addEventListener("click",async()=>{
          if(!window.confirm("Удалить эту фотографию из альбома игрока?"))return;
          remove.disabled=true;
          try{
            await post("/api/players/native/references",{project_id:project,player_id:player,
              action:"undo",reference_id:photo.id,expected_revision:album.revision});
            if(project!==state.project||player!==state.viewPlayer)return;
            window.dispatchEvent(new Event("volley:player-photo-saved"));
            await loadAlbum();
          }catch(error){playerStatus.textContent="Не удалось удалить фото: "+error.message;remove.disabled=false;}
        });
        card.append(img,remove);playerPhotos.append(card);
      }
    }catch(error){if(project===state.project&&player===state.viewPlayer)playerStatus.textContent="Не удалось загрузить фото: "+error.message;}
  };
  const setMode=mode=>{
    tracksPanel.dataset.trackMode=mode;
    singlesMode.setAttribute("aria-pressed",String(mode==="singles"));
    groupsMode.setAttribute("aria-pressed",String(mode==="groups"));
    playerMode.setAttribute("aria-pressed",String(mode==="player"));
    playerGallery.hidden=mode!=="player";
    groupingControls.hidden=mode!=="groups";
  };
  singlesMode.addEventListener("click",()=>setMode("singles"));
  groupsMode.addEventListener("click",()=>setMode("groups"));
  playerMode.addEventListener("click",()=>{setMode("player");void loadAlbum();});
  modeTabs.append(singlesMode,groupsMode,playerMode);
  modeSwitch.append(modeTabs);
  const groupingControls=node("div","track-grouping-controls");
  const thresholdLabel=node("label","","Сходство ID: ");
  const thresholdValue=node("output","","30%");
  thresholdLabel.append(thresholdValue,reviewThreshold);
  reviewThreshold.addEventListener("input",()=>{thresholdValue.textContent=reviewThreshold.value+"%";});
  const groupButton=node("button","primary","Сгруппировать ID");
  groupButton.type="button";groupButton.disabled=true;
  groupingControls.append(thresholdLabel,groupButton);
  const viewBar=node("div","track-view-bar");
  const viewLabel=node("strong","track-view-label","Непривязанные Track ID");
  const updateUnassignedLabel=()=>{
    if(!state.viewPlayer)viewLabel.textContent=state.viewConflicts?
      "Конфликты одновременной видимости · "+state.identityConflictIds.size:
      "Непривязанные Track ID · "+state.unassignedIds.size;
    if(typeof conflictToggle!=="undefined"){
      conflictToggle.hidden=Boolean(state.viewPlayer);
      conflictToggle.textContent=state.viewConflicts?
        "Показать непривязанные · "+state.unassignedIds.size:
        "Конфликты · "+state.identityConflictIds.size;
    }
  };  const viewBack=node("button","secondary","Все непривязанные ID");
  viewBack.type="button";viewBack.hidden=true;
  viewBack.addEventListener("click",()=>{
    state.viewPlayer="";state.viewConflicts=false;state.offset=0;updateUnassignedLabel();
    viewBack.hidden=true;groupsMode.hidden=false;playerMode.hidden=true;setMode("singles");void loadPage();
  });
  window.addEventListener("volley:player-gallery-select",event=>{
    const detail=event.detail||{};
    if(detail.project_id!==state.project||!state.projectPlayerIds.has(detail.player_id))return;
    state.viewPlayer=detail.player_id;state.offset=0;
    viewLabel.textContent=String(detail.name||"");
    viewBack.hidden=false;groupsMode.hidden=true;playerMode.hidden=false;setMode("player");void loadAlbum();void loadPage();
  });
  const conflictToggle=node("button","secondary","Конфликты");
  conflictToggle.type="button";
  conflictToggle.addEventListener("click",()=>{
    state.viewConflicts=!state.viewConflicts;state.offset=0;
    state.selected.clear();updateUnassignedLabel();void loadPage();
  });  viewBar.append(viewBack);
  viewBar.append(conflictToggle);
  modeSwitch.append(groupingControls,viewLabel);
  ui.list.before(viewBar);
  ui.list.before(modeSwitch);
  ui.list.before(playerGallery);
  setMode("singles");
  ui.list.before(groupGallery);
  const singlesBar=node("div","track-singles-bar");
  const singlesHeader=node("h3","native-track-singles-heading","Отдельные Track ID");
  const singlesExtract=node("button","secondary","Извлечь ReID для выбранных");
  singlesExtract.type="button";singlesExtract.disabled=true;
  singlesExtract.addEventListener("click",()=>reviewExtract.click());
  singlesBar.append(singlesHeader,singlesExtract);
  ui.list.before(singlesBar);
  const renderGroupCards=()=>{
    clear(galleryItems);
    if(state.grouping){
      galleryItems.append(node("div","empty-state","Идёт группировка похожих ID…"));
      galleryMore.hidden=true;
      return;
    }
    if(!state.groups.length&&!state.groupLoaded){
      galleryItems.append(node("div","empty-state",
        "Загрузка сохранённых и предложенных ReID-групп…"));
    }
    if(!state.groups.length&&state.groupLoaded){
      galleryItems.append(node("div","empty-state","Сгруппированных ID пока нет."));
    }
    const visibleGroups=state.groups.map(group=>({...group,originalCount:group.ids.length,
      ids:group.ids.filter(id=>state.unassignedIds.has(id))})).filter(group=>group.ids.length>=2);
    if(state.groups.length&&!visibleGroups.length&&state.groupLoaded)
      galleryItems.append(node("div","empty-state","Нет групп с непривязанными Track ID."));
    for(const group of visibleGroups.slice(0,state.groupShown)){
      const ids=group.ids;
      const item=node("article","native-track-group-card native-track-photo-card");
      item.dataset.trackCardKey="group:"+ids.join(",");
      const mosaics=node("div","track-group-photo-mosaic");
      for(const id of ids.slice(0,3))mosaics.append(portrait(id,"track-group-portrait"));
      const meta=node("div","native-track-group-meta");
      const name=node("strong","",group.source==="manual"?"Ручная группа":
        "ReID-группа");
      const average=ids.length===group.originalCount&&Number.isFinite(group.averageSimilarity)?
        (group.averageSimilarity*100).toFixed(1)+"%":"не рассчитано";
      const label=node("small","",ids.length+" Track ID · Среднее сходство ID: "+average);
      const members=node("span","native-track-group-members",
        ids.map(id=>"ID "+id).join(" · "));
      meta.append(name,label,members);
      const choose=node("button","secondary track-group-select",
        "Выбрать все "+ids.length+" ID");
      choose.type="button";choose.disabled=state.busy||ids.length>200;
      choose.addEventListener("click",()=>{
        state.selected=new Set(ids);renderRows();unlock();renderGroupCards();
        reviewNote("Выбрана группа из "+ids.length+" ID. Назначение требует подтверждения.");
      });
      item.append(mosaics,meta);
      attachSuggestions(item,ids);
      item.append(choose);
      galleryItems.append(item);
    }
    galleryMore.hidden=visibleGroups.length<=state.groupShown;
    galleryMore.textContent="Показать ещё · "+(visibleGroups.length-state.groupShown);
  };
  galleryMore.addEventListener("click",()=>{
    state.groupShown+=24;renderGroupCards();
  });
  const photoClose=node("button","secondary track-photo-close","×");
  photoClose.type="button";photoClose.title="Закрыть фотографию";
  photoClose.setAttribute("aria-label","Закрыть фотографию");
  photoClose.addEventListener("click",()=>ui.photo.classList.add("hidden"));
  ui.photo.prepend(photoClose);
  ui.photo.setAttribute("role","dialog");
  ui.photo.setAttribute("aria-label","Увеличенная фотография Track ID");
  window.addEventListener("keydown",event=>{
    if(event.key==="Escape")ui.photo.classList.add("hidden");
  });
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
    project_id:state.project,video:state.video,replay_id:state.replay,rally_run_id:state.rallyRun,
    expected_source_sha256:state.sourceSha,threshold_percent:Number(reviewThreshold.value)
  });
  const reviewReady=()=>Boolean(state.project&&state.video&&state.replay)&&
    /^[a-f0-9]{64}$/.test(state.sourceSha)&&
    Number.isInteger(+reviewThreshold.value)&&
    +reviewThreshold.value>=1&&+reviewThreshold.value<=99;
  const reviewNote=(message,error=false)=>{
    reviewStatus.textContent=message;
    reviewStatus.classList.toggle("track-error",error);
  };
  const reidUnlock=()=>{
    const ready=reviewReady()&&!state.busy;
    reviewThreshold.disabled=state.busy;
    groupButton.disabled=!ready||!state.writable||!state.unassignedIds.size||Boolean(state.viewPlayer);
    reviewRead.disabled=!ready;
    reviewExtract.disabled=!ready||state.selected.size<1||state.selected.size>8;
    galleryExtract.disabled=reviewExtract.disabled;
    galleryRepeat.disabled=!ready||!state.comparisonIds.size;
    galleryAuto.disabled=!ready||!state.writable||!state.comparisonIds.size||!state.roster.length;
    singlesExtract.disabled=reviewExtract.disabled;
    reviewMerge.disabled=!ready||!state.writable||!state.groupRevision||state.selected.size<2;
    reviewSplit.disabled=!ready||!state.writable||!state.groupRevision||state.selected.size<1;
  };
  const autoAssignSimilar=async()=>{
    if(!reviewReady()||state.busy||!state.writable||!state.comparisonIds.size)return;
    const scope=reviewScope(),token=state.token;
    state.busy=true;unlock();reidUnlock();
    galleryStatus.textContent="Сопоставляем похожие ID с фотографиями игроков…";
    try{
      const result=await post("/api/players/native/reid-review",{...scope,action:"auto_assign"});
      if(token!==state.token||scope.replay_id!==state.replay)return;
      if(!Number.isSafeInteger(result.automatic_count)||result.automatic_count<0 ||
         !result.automatic_assignments||typeof result.automatic_assignments!=="object")
        throw Error("Сервер вернул некорректные автоматические привязки");
      // Keep suggestions until the refreshed track page supplies saved scores.
      window.dispatchEvent(new CustomEvent("volley:track-assignments-updated",{detail:{
        project_id:scope.project_id,video:scope.video,replay_id:scope.replay_id
      }}));
      await loadPage();
      galleryStatus.textContent="Автоматически привязано ID: "+result.automatic_count;
      reviewNote("Автопривязка выполнена: "+result.automatic_count+
        ". Ручные назначения сохранены; одновременные ID одного игрока не объединяются.");
    }catch(error){galleryStatus.textContent="Ошибка автопривязки: "+String(error.message||error);
      reviewNote(String(error.message||error),true);}
    finally{state.busy=false;unlock();reidUnlock();}
  };
  const repeatPhotoAnalysis=async()=>{
    if(!reviewReady()||state.busy)return;
    const scope=reviewScope(),token=state.token,ids=[...state.comparisonIds];
    if(!ids.length){reviewNote("Нет непривязанных ID для сравнения.");return;}
    if(ids.length>1024){reviewNote("Для сравнения доступно не более 1024 Track ID одной обработки.",true);return;}
    state.busy=true;state.playerMatches={};unlock();reidUnlock();
    galleryStatus.textContent="Выполняется повторный анализ…";
    try{
      reviewNote("Извлекаем признаки и сравниваем ID с сохранёнными фотографиями игроков…");
      const result=await post("/api/players/native/reid-review",{
        ...scope,rally_run_id:state.rallyRun,action:"compare_photos"
      });
      if(token!==state.token||scope.replay_id!==state.replay)return;
      if(result.schema!=="volley-native-player-photo-comparison/1"||
        result.source_replay_sha256!==state.sourceSha||result.auto_assign_enabled!==false||
        result.confirmed!==false||!result.player_matches||typeof result.player_matches!=="object")
        throw Error("Некорректный результат сравнения фотографий");
      state.playerMatches=result.player_matches;renderRows();renderGroupCards();
      galleryStatus.textContent="Сравнено ID: "+ids.length+" · фото: "+result.photo_count;
      reviewNote("Сравнено ID: "+ids.length+" · фото: "+result.photo_count+
        ". Совпадения требуют ручной проверки.");
    }catch(error){galleryStatus.textContent="Анализ не завершён: "+String(error.message||error);
      reviewNote(String(error.message||error),true);}
    finally{state.busy=false;unlock();reidUnlock();}
  };
  const reviewRender=(record,{deferRender=false}={})=>{
    if(record.schema!=="volley-native-reid-group-review/1"||
      record.confirmed!==false||record.auto_assign_enabled!==false||
      record.source_replay_sha256!==state.sourceSha||
      !Array.isArray(record.groups)||!Array.isArray(record.unmatched_ids))
      throw Error("Неподдерживаемый источник ReID");
    const used=new Set();
    const groups=[];
    for(const group of record.groups){
      const ids=Array.isArray(group.track_ids)?
        group.track_ids.map(Number):[];
      if(ids.some(id=>!Number.isSafeInteger(id)||id<=0))
        throw Error("Некорректные ReID-группы");
      const scoped=ids.filter(id=>state.rallyIds.has(id));
      if(scoped.some(id=>used.has(id)))
        throw Error("Пересекающиеся ReID-группы");
      if(scoped.length<2)continue;
      scoped.forEach(id=>used.add(id));
      groups.push({ids:scoped,source:group.source,similarity:group.similarity,
        averageSimilarity:scoped.length===ids.length?group.average_similarity:null});
    }
    state.groups=groups;state.grouped=used;state.groupLoaded=true;
    // Grouping evidence must not override player-photo identity scores.
    state.singleCount=state.rallyIds.size-used.size;
    galleryStatus.textContent="";
    if(!deferRender){renderGroupCards();renderRows();}
    reviewNote("Групп: "+groups.length+" · признаков: "+record.embedded_count+
      " · отклонено: совместно в кадре "+(record.co_visible_veto_count||0)+
      ", разные подтверждённые игроки "+(record.confirmed_owner_veto_count||0)+
      ", визуальная неоднозначность "+(record.ambiguous_lookalike_veto_count||0)+
      ". Для новых групп выберите ID и извлеките признаки.");
  };
  const reviewLoad=async({deferRender=false}={})=>{
    if(!reviewReady()){reviewNote("Выберите CPU-версию и порог 1–99%.",true);return;}
    const scope=reviewScope(),token=state.token;
    reviewNote("Проверяем исходный SHA и ReID-кэш…");
    try{
      const record=await post("/api/players/native/reid-review",{...scope,action:"read"});
      if(token!==state.token||scope.replay_id!==state.replay)return;
      state.groupRevision=String(record.revision);reviewRender(record,{deferRender});
      return true;
    }catch(e){
      if(token!==state.token||scope.replay_id!==state.replay)return;
      state.groupRevision=null;state.groupLoaded=true;
      state.groups=[];state.grouped.clear();state.singleCount=state.total;
      // Preserve persisted photo scores when group loading fails.
      galleryStatus.textContent="Не удалось загрузить группы: "+e.message;
      if(!deferRender){renderGroupCards();renderRows();}reviewNote(e.message,true);
    }finally{reidUnlock();}
  };
  groupButton.addEventListener("click",async()=>{
    if(!reviewReady()||state.busy||!state.writable)return;
    const ids=[...state.unassignedIds],scope=reviewScope(),token=state.token;
    if(ids.length>2500){note("Для группировки доступно не более 2500 ID за одну обработку.",true);return;}
    state.busy=true;state.grouping=true;unlock();reidUnlock();renderGroupCards();
    window.VolleyballTasks?.set("track-grouping",{
      label:"Группировка ID",stage:"Подготовка признаков: 0 / "+ids.length,progress:0
    });
    try{
      for(let offset=0;offset<ids.length;offset+=8){
        if(token!==state.token)return;
        note("Подготовка признаков: "+Math.min(offset+8,ids.length)+" / "+ids.length);
        const result=await post("/api/players/native/embeddings",{
          project_id:scope.project_id,video:scope.video,replay_id:scope.replay_id,
          rally_run_id:state.rallyRun,expected_source_sha256:scope.expected_source_sha256,
          track_ids:ids.slice(offset,offset+8)
        });
        if(token!==state.token)return;
        if(result.computed_count>0&&!result.cache_persisted)throw Error("Не удалось сохранить признаки для группировки");
        const done=Math.min(offset+8,ids.length);
        window.VolleyballTasks?.set("track-grouping",{
          label:"Группировка ID",stage:"Подготовка признаков: "+done+" / "+ids.length,
          progress:ids.length?done/ids.length*100:0
        });
      }
      if(token!==state.token)return;
      window.VolleyballTasks?.set("track-grouping",{label:"Группировка ID",stage:"Группировка похожих ID",progress:null});
      if(!await reviewLoad())throw Error("Не удалось получить группы");
      if(token!==state.token)return;
      setMode("groups");
      note("Группировка завершена. Проверьте предложения перед привязкой к игроку.");
    }catch(error){if(token===state.token)note("Группировка не завершена: "+String(error.message||error),true);}
    finally{
      window.VolleyballTasks?.clear("track-grouping");
      state.busy=false;state.grouping=false;unlock();reidUnlock();
      if(token===state.token)renderGroupCards();
    }
  });
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
  reviewThreshold.addEventListener("change",()=>{
    state.groupRevision=null;state.groupLoaded=false;
    reidUnlock();if(reviewReady())reviewLoad();
  });
  reviewExtract.addEventListener("click",async()=>{
    if(!reviewReady()||state.busy||!state.selected.size||state.selected.size>8)return;
    const scope=reviewScope(),token=state.token;
    state.busy=true;unlock();reidUnlock();reviewNote("Вычисляем реальные признаки OpenVINO…");
    let extracted=false;
    try{
      const result=await post("/api/players/native/embeddings",{
        project_id:scope.project_id,video:scope.video,replay_id:scope.replay_id,
        rally_run_id:state.rallyRun,
        expected_source_sha256:scope.expected_source_sha256,track_ids:[...state.selected]
      });
      if(token!==state.token||scope.replay_id!==state.replay)return;
      extracted=true;
      reviewNote("Признаков: "+result.embedding_count+" · кэш: "+result.cache_hits);
    }catch(e){reviewNote(e.message,true);}
    finally{state.busy=false;unlock();reidUnlock();}
    if(extracted&&reviewReady())await reviewLoad();
  });
  ui.refresh.addEventListener("click",()=>loadVideos());
  ui.prev.addEventListener("click",()=>{
    if(state.offset>0){state.offset=Math.max(0,state.offset-state.limit);loadPage();}
  });
  ui.next.addEventListener("click",()=>{
    if(state.offset+state.limit<state.filteredTotal){
      state.offset+=state.limit;loadPage();
    }
  });
  ui.player.addEventListener("change",unlock);
  ui.assign.addEventListener("click",()=>bindSelected([...state.selected],ui.player.value));
  ui.unassign.addEventListener("click",()=>edit("unassign"));
  ui.exclude.addEventListener("click",()=>edit("exclude"));
  ui.restore.addEventListener("click",()=>edit("restore"));
  window.addEventListener("volley:tab-change",event=>{
    if(event.detail?.tab==="players")loadVideos();
  });
  window.addEventListener("volley:project-ready",()=>{
    if(active())loadVideos();
  });
  window.addEventListener("volley:postprocessing-change",()=>{
    if(active())loadVideos();
  });
  window.addEventListener("volley:player-library-updated",()=>{
    if(active())void loadLibraryRoster();
  });
  if(active())loadVideos();
})();
