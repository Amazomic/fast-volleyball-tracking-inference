"use strict";
// Interim same-origin UI controls. Rust enforces data ownership, immutable
// evidence checksums, stage isolation and registry compare-and-swap revisions.
// The future wasm32 app will replace this bridge, not bypass the native API.
(() => {
  const $ = id => document.getElementById(id);
  const ui = {
    preview:$("studio-preview-view"),analysis:$("studio-analysis-view"),
    players:$("studio-players-view"),quick:$("studio-quick-view"),
    legacy:$("studio-legacy-view"),title:$("studio-section-name"),
    project:$("studio-project"),video:$("studio-analysis-video"),
    refresh:$("studio-analysis-refresh"),stages:$("studio-stage-list"),
    note:$("studio-analysis-note"),ball:$("studio-gpu-ball"),
    ravel:$("studio-gpu-players"),gpuCancel:$("studio-gpu-cancel"),
    playerForm:$("studio-player-create"),playerName:$("studio-player-name"),
    playerList:$("studio-player-list"),playerCount:$("studio-player-count"),
    playerRefresh:$("studio-player-refresh")
  };
  const state={
    tab:"preview",analysisGeneration:0,playerGeneration:0,
    video:"",gpuJob:"",gpuProject:"",gpuTimer:null,
    revision:"absent",players:[],mutating:false
  };
  const make=(tag,klass,text)=>{
    const node=document.createElement(tag);
    if(klass)node.className=klass;
    if(text!==undefined)node.textContent=String(text);
    return node;
  };
  const clear=node=>{while(node.firstChild)node.firstChild.remove();};
  const api=async(path,opts={})=>{
    const res=await fetch(path,{cache:"no-store",credentials:"same-origin",...opts});
    const data=await res.json().catch(()=>({}));
    if(!res.ok){
      const msg=typeof data.message==="string"?data.message:
        typeof data.error==="string"?data.error:"HTTP "+res.status;
      throw Error(msg);
    }
    return data;
  };
  const get=(path,params)=>api(path+"?"+new URLSearchParams(params));
  const post=(path,body)=>api(path,{method:"POST",headers:{"Content-Type":"application/json"},
    body:JSON.stringify(body)});
  const announce=(text,error=false)=>{
    const banner=$("studio-banner");
    banner.textContent=String(text);
    banner.className=error?"banner error":"banner ok";
  };
  const failure=e=>announce(e instanceof Error?e.message:String(e),true);
  const stopGpuPolling=()=>{
    if(state.gpuTimer!==null)clearInterval(state.gpuTimer);
    state.gpuTimer=null;
  };
  const switchTab=(requested)=>{
    const next=requested==="analysis"||requested==="players"||requested==="montage"||requested==="quick"||requested==="legacy"?
      requested:"preview";
    state.tab=next;
    ui.preview.classList.toggle("hidden",next==="analysis"||next==="players"||next==="quick"||next==="legacy");
    ui.quick.classList.toggle("hidden",next!=="quick");
    ui.legacy.classList.toggle("hidden",next!=="legacy");
    ui.analysis.classList.toggle("hidden",next!=="analysis");
    ui.players.classList.toggle("hidden",next!=="players");
    const labels={preview:"Предпросмотр",analysis:"Анализ",players:"Игроки",montage:"Монтаж",quick:"Быстрый рилс",legacy:"Старые проекты и архив"};
    ui.title.textContent=labels[next];
    window.dispatchEvent(new CustomEvent("volley:tab-change",{detail:{tab:next}}));
    for(const nav of document.querySelectorAll("[data-studio-tab]")){
      const active=nav.dataset.studioTab===next;
      nav.classList.toggle("active",active);
      if(active)nav.setAttribute("aria-current","page");
      else nav.removeAttribute("aria-current");
    }
    if(next==="analysis")loadVideos().catch(failure);
    if(next==="players")loadPlayers().catch(failure);
    if(next==="montage")$("studio-montage").scrollIntoView({behavior:"smooth"});
  };
  const projectId=()=>ui.project.value;
  const stageNames={
    gpu:"01 · GPU: нейросети",fusion:"02 · Объединение RAVEL и VballNet",
    tracking:"03 · CPU: Track ID",touches:"04 · CPU: касания",
    rallies:"05 · CPU: розыгрыши"
  };
  const runStage=async(stage,request)=>{
    if(state.stageBusy)return;
    const project=projectId(),video=ui.video.value;
    if(!project||!video){announce("Выберите проект и видео.",true);return;}
    state.stageBusy=true;
    for(const button of ui.stages.querySelectorAll("button"))button.disabled=true;
    const urls={
      fusion:"/api/inference/native/fuse",
      tracking:"/api/cpu/native/retrack",
      touches:"/api/cpu/native/touches",
      rallies:"/api/cpu/native/rallies"
    };
    const names={
      fusion:"Объединение GPU",
      tracking:"Track ID",
      touches:"Касания",
      rallies:"Розыгрыши"
    };
    try{
      ui.note.textContent=names[stage]+": проверка источников и пересчёт на CPU…";
      const result=await post(urls[stage],{project_id:project,video,...request});
      if(result.project_id!==project||result.video!==video){
        throw Error("Некорректная версия ответа CPU-сервера");
      }
      announce(names[stage]+": создана новая сохранённая версия.");
      if(project===projectId()&&video===ui.video.value&&state.tab==="analysis"){
        await loadOverview();
      }
    }catch(e){failure(e);}
    finally{
      state.stageBusy=false;
      for(const button of ui.stages.querySelectorAll("button"))button.disabled=false;
    }
  };
  const action=(caption,callback)=>{
    const button=make("button","secondary",caption);button.type="button";
    button.addEventListener("click",callback);
    return button;
  };
  const renderStages=data=>{
    clear(ui.stages);
    if(!Array.isArray(data.stages))return;
    for(const stage of data.stages){
      const card=make("section","stage-card");
      const head=make("h3");
      head.append(make("span","",stageNames[stage.key]||stage.key),
        make("small","",String(stage.total||0)+" версий"));
      card.append(head);
      const items=Array.isArray(stage.items)?stage.items:[];
      if(!items.length){
        card.append(make("div","stage-empty","Для этого видео нет сохранённой версии."));
      }else{
        for(const item of items){
          const row=make("div","stage-row");
          const info=make("span");
          info.append(make("strong","",item.model==="ball"?"VballNet":
            item.model==="players"?"RAVEL":String(item.state||"Сохранено")));
          info.append(make("div","",String(item.id||"").slice(0,12)+"…"));
          const amount=make("span");
          amount.append(make("strong","",item.count??"—"),
            document.createTextNode(" · "+String(item.count_unit||"")));
          row.append(info,amount);
          if(stage.key==="fusion"){
            row.append(action("Пересчитать Track ID",()=>
              runStage("tracking",{fusion_id:item.id})));
          }else if(stage.key==="tracking"){
            row.append(action("Рассчитать касания",()=>
              runStage("touches",{replay_id:item.id})));
          }else if(stage.key==="touches"&&typeof item.source_replay_id==="string"){
            row.append(action("Собрать розыгрыши",()=>
              runStage("rallies",{replay_id:item.source_replay_id,touch_id:item.id})));
          }else if(stage.key==="rallies"){
            row.append(action("Открыть",()=>{
              window.dispatchEvent(new CustomEvent("volley:open-native-rally",{
                detail:{project:projectId(),run:item.id}
              }));
            }));
          }
          card.append(row);
        }
      }
      if(stage.key==="gpu"){
        const ballRows=items.filter(x=>x.model==="ball");
        const playerRows=items.filter(x=>x.model==="players");
        if(ballRows.length&&playerRows.length){
          const selection=make("div","stage-merge");
          const ball=make("select");ball.setAttribute("aria-label","VballNet GPU-версия");
          const players=make("select");players.setAttribute("aria-label","RAVEL GPU-версия");
          for(const item of ballRows){
            const option=make("option","",item.id.slice(0,12));
            option.value=item.id;ball.append(option);
          }
          for(const item of playerRows){
            const option=make("option","",item.id.slice(0,12));
            option.value=item.id;players.append(option);
          }
          selection.append(make("span","","VballNet:"),ball,make("span","","RAVEL:"),players,
            action("Объединить GPU",()=>runStage("fusion",{
              vball_job_id:ball.value,ravel_job_id:players.value
            })));
          card.append(selection);
        }
      }
      if(stage.truncated){
        card.append(make("div","stage-empty","Показаны последние 60 версий."));
      }
      ui.stages.append(card);
    }
  };
  const loadOverview=async()=>{
    if(state.tab!=="analysis")return;
    const project=projectId(),video=ui.video.value;
    state.video=video;
    const token=++state.analysisGeneration;
    if(!project||!video){
      clear(ui.stages);
      ui.note.textContent="Выберите проект и видео для просмотра сохранённых стадий.";
      ui.ball.disabled=true;ui.ravel.disabled=true;
      return;
    }
    ui.note.textContent="Проверка метаданных стадий…";
    ui.ball.disabled=!!state.gpuJob;
    ui.ravel.disabled=!!state.gpuJob;
    const result=await get("/api/studio/rust/analysis-overview",{
      project_id:project,video
    });
    if(token!==state.analysisGeneration||state.tab!=="analysis")return;
    if(result.schema!=="volley-native-analysis-overview/1"
        || result.metadata_only!==true
        || result.authoritative_game_events!==false){
      throw Error("Неверная версия обзора анализа");
    }
    renderStages(result);
    ui.note.textContent="Показаны версии сохранённых стадий. Контрольные суммы входных данных повторно проверяются при CPU-пересчёте, не в этом списке.";
  };
  const loadVideos=async()=>{
    const project=projectId(),old=ui.video.value;
    if(!project){
      clear(ui.video);ui.video.append(make("option","","Сначала выберите проект"));
      await loadOverview();return;
    }
    const rows=(await api("/api/projects")).projects||[];
    if(state.tab!=="analysis"||projectId()!==project)return;
    const current=rows.find(p=>p.id===project);
    const videos=Array.isArray(current?.videos)?current.videos.filter(x=>typeof x==="string"):[];
    clear(ui.video);
    const option=make("option","","Выберите видео");option.value="";ui.video.append(option);
    for(const name of videos){
      const item=make("option","",name);item.value=name;ui.video.append(item);
    }
    ui.video.value=videos.includes(old)?old:(videos.length===1?videos[0]:"");
    await loadOverview();
    await restoreGpu();
  };
  const pollGpu=async()=>{
    if(!state.gpuJob)return;
    const id=state.gpuJob,project=state.gpuProject;
    try{
      const status=await get("/api/inference/native/full/status",{
        project_id:project,job_id:id
      });
      if(id!==state.gpuJob)return;
      const progress=Number(status.progress_percent);
      ui.note.textContent="GPU · "+String(status.phase||"обработка")+
        " · "+(Number.isFinite(progress)?progress.toFixed(1)+"%":"—");
      if(["complete","cancelled","failed","interrupted"].includes(status.phase)){
        stopGpuPolling();state.gpuJob="";
        ui.gpuCancel.disabled=true;
        if(status.phase==="complete")announce("GPU-инференс завершён.");
        else announce("GPU: "+String(status.error||status.phase),true);
        await loadOverview();
      }
    }catch(e){
      stopGpuPolling();
      state.gpuJob="";
      ui.gpuCancel.disabled=true;
      failure(e);
    }
  };
  // Reattach to an already-running native GPU job after a browser refresh.
  // No start request or GPU operation happens during this read-only probe.
  const restoreGpu=async()=>{
    if(state.gpuJob)return;
    const project=projectId();
    if(!project||state.tab!=="analysis")return;
    const generation=state.analysisGeneration;
    const currentVideo=ui.video.value;
    const reply=await get("/api/inference/native/full/active",{project_id:project});
    if(generation!==state.analysisGeneration||state.tab!=="analysis"||
      project!==projectId()||currentVideo!==ui.video.value)return;
    if(reply.schema!=="volley-native-active-gpu/1"||
      reply.project_id!==project)throw Error("Неизвестный статус GPU-задачи");
    if(reply.active!==true)return;
    if(typeof reply.job_id!=="string"||!/^[a-f0-9]{32}$/.test(reply.job_id)){
      throw Error("Некорректный ID активного GPU-запуска");
    }
    state.gpuJob=reply.job_id;
    state.gpuProject=project;
    ui.ball.disabled=true;ui.ravel.disabled=true;
    ui.gpuCancel.disabled=false;
    announce("Подключено к уже работающему GPU-инференсу, новый запуск не требуется.");
    stopGpuPolling();
    await pollGpu();
    if(state.gpuJob)state.gpuTimer=setInterval(pollGpu,1800);
  };
  const startGpu=async model=>{
    if(state.gpuJob){
      announce("GPU-задача уже выполняется.",true);return;
    }
    const project=projectId(),video=ui.video.value;
    if(!project||!video){
      announce("Выберите проект и видео.",true);return;
    }
    ui.ball.disabled=true;ui.ravel.disabled=true;
    try{
      const job=await post("/api/inference/native/full/start",{
        project_id:project,video,model,device:"cuda",stride:9
      });
      if(typeof job.job_id!=="string")throw Error("GPU не вернул идентификатор задачи");
      state.gpuJob=job.job_id;state.gpuProject=project;
      ui.gpuCancel.disabled=false;
      announce("Запущен "+(model==="players"?"RAVEL":"VballNet")+
        " GPU-инференс. Результат будет сохранён отдельно.");
      stopGpuPolling();
      await pollGpu();
      if(state.gpuJob)state.gpuTimer=setInterval(pollGpu,1800);
    }catch(e){
      ui.ball.disabled=false;ui.ravel.disabled=false;failure(e);
    }
  };
  const cancelGpu=async()=>{
    if(!state.gpuJob)return;
    try{
      await post("/api/inference/native/full/cancel",{
        project_id:state.gpuProject,job_id:state.gpuJob
      });
      announce("Запрошена отмена GPU-задачи.");
    }catch(e){failure(e);}
  };
  const renderPlayers=()=>{
    clear(ui.playerList);
    ui.playerCount.textContent=String(state.players.length)+" записей";
    if(!state.players.length){
      ui.playerList.append(make("div","empty-state","В библиотеке Rust пока нет игроков."));
      return;
    }
    const rows=[...state.players].sort((a,b)=>
      String(a.name||"").localeCompare(String(b.name||""),"ru"));
    for(const player of rows){
      if(typeof player.id!=="string"||typeof player.name!=="string")continue;
      const row=make("article","player-row");
      row.append(make("div","player-avatar",player.name.slice(0,1).toUpperCase()));
      const meta=make("div","player-meta");
      meta.append(make("strong","",player.name),
        make("small","","ID "+player.id+" · сохранённый профиль"));
      const edit=make("button","secondary","Переименовать");edit.type="button";
      edit.addEventListener("click",()=>{
        edit.disabled=true;
        const form=make("form","player-edit");
        const input=make("input");input.type="text";
        input.maxLength=80;input.required=true;input.value=player.name;
        input.setAttribute("aria-label","Новое имя игрока");
        const save=make("button","primary","Сохранить");save.type="submit";
        const cancel=make("button","secondary","Отмена");cancel.type="button";
        cancel.addEventListener("click",()=>form.remove());
        form.addEventListener("submit",async event=>{
          event.preventDefault();
          if(state.mutating)return;
          state.mutating=true;save.disabled=true;
          try{
            const result=await post("/api/players/library/native",{
              action:"rename",player_id:player.id,
              name:input.value,expected_revision:state.revision
            });
            state.revision=String(result.revision);
            state.players=Array.isArray(result.players)?result.players:[];
            renderPlayers();announce("Имя игрока сохранено.");
          }catch(e){
            failure(e);
            await loadPlayers().catch(failure);
          }finally{state.mutating=false;}
        });
        form.append(input,save,cancel);row.append(form);input.focus();
      });
      row.append(meta,edit);
      ui.playerList.append(row);
    }
  };
  const loadPlayers=async()=>{
    const token=++state.playerGeneration;
    ui.playerList.replaceChildren(make("div","empty-state","Загрузка библиотеки игроков…"));
    const result=await post("/api/players/library/native",{action:"read"});
    if(token!==state.playerGeneration)return;
    state.players=Array.isArray(result.players)?result.players:[];
    state.revision=String(result.revision||"absent");
    renderPlayers();
  };
  ui.playerForm.addEventListener("submit",async event=>{
    event.preventDefault();
    if(state.mutating)return;
    const name=ui.playerName.value.trim();
    if(!name)return;
    state.mutating=true;
    try{
      const result=await post("/api/players/library/native",{
        action:"create",name,expected_revision:state.revision
      });
      state.players=Array.isArray(result.players)?result.players:[];
      state.revision=String(result.revision);
      ui.playerName.value="";
      renderPlayers();announce("Игрок добавлен в изолированную библиотеку Rust.");
    }catch(e){
      failure(e);
      await loadPlayers().catch(failure);
    }finally{state.mutating=false;}
  });
  ui.playerRefresh.addEventListener("click",()=>loadPlayers().catch(failure));
  ui.refresh.addEventListener("click",()=>loadOverview().catch(failure));
  ui.video.addEventListener("change",()=>loadOverview().catch(failure));
  ui.ball.addEventListener("click",()=>startGpu("ball"));
  ui.ravel.addEventListener("click",()=>startGpu("players"));
  ui.gpuCancel.addEventListener("click",cancelGpu);
  ui.project.addEventListener("change",()=>{
    state.analysisGeneration++;
    if(state.tab==="analysis")loadVideos().catch(failure);
  });
  window.addEventListener("volley:project-ready",()=>{
    state.analysisGeneration++;
    if(state.tab==="analysis")loadVideos().catch(failure);
  });
  for(const nav of document.querySelectorAll("[data-studio-tab]")){
    nav.addEventListener("click",event=>{
      event.preventDefault();
      const next=nav.dataset.studioTab;
      history.replaceState(null,"","#"+next);
      switchTab(next);
    });
  }
  window.addEventListener("hashchange",()=>{
    switchTab(location.hash.slice(1));
  });
  switchTab(location.hash.slice(1));
})();