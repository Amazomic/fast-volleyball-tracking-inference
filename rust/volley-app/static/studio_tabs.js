"use strict";
// Browser-only DOM and same-origin network adapters. Rust/WASM validates
// navigation, language, settings and async load epochs; native Rust owns
// evidence checksums, stage isolation and revision-safe mutations.
(() => {
  const $ = id => document.getElementById(id);
  const ui = {
    preview:$("studio-preview-view"),montage:$("studio-montage-view"),
    analysis:$("studio-analysis-view"),
    players:$("studio-players-view"),report:$("studio-report-view"),quick:$("studio-quick-view"),
    project:$("studio-project"),video:$("studio-analysis-video"),
    note:$("studio-analysis-note"),start:$("studio-analysis-start"),
    gpuOnly:$("studio-analysis-gpu-only"),postOnly:$("studio-analysis-post-only"),
    reset:$("studio-analysis-reset"),
    progress:$("studio-analysis-progress"),profileDelta:$("studio-analysis-profile-delta"),
    logPanel:$("studio-analysis-log-panel"),logOutput:$("studio-analysis-log-output"),
    logCurrent:$("studio-analysis-log-current"),logCount:$("studio-analysis-log-count"),
    historyList:$("studio-history-list"),historyRefresh:$("studio-history-refresh"),historyMore:$("studio-history-more"),
    playerForm:$("studio-player-create"),playerName:$("studio-player-name"),
    playerList:$("studio-player-list"),playerCount:$("studio-player-count"),
    playerRefresh:$("studio-player-refresh")
  };
  const runtime=window.VolleyballUi;
  if(!runtime?.ready)throw Error("Studio UI initialization missing");
  const state={
    tab:"preview",analysisGeneration:0,playerGeneration:0,formProject:"",
    pipelineId:"",pipelineProject:"",pipelineTimer:null,
    starting:false,stopRequested:false,polling:false,pollErrors:0,completedId:"",
    logKey:"",logOffset:0,logBusy:false,logLastCall:"",logPending:null,
    historyRows:[],historyFilter:"all",historyEpoch:0,historyOffset:0,
    revision:"absent",players:[],mutating:false,playersLoaded:false,
    projectMemberIds:new Set(),trackCoverScope:"",trackCovers:new Map()
  };
  // Per-project form snapshot: store only deviations from the versioned gold
  // defaults. A changed native default never gets silently overwritten.
  const analysisFormCache=(()=>{
    const prefix="volley:studio-analysis-form:v1:";
    const read=project=>{
      if(!/^[a-f0-9]{12}$/.test(project||""))return null;
      try{
        const raw=localStorage.getItem(prefix+project);
        if(!raw||raw.length>8192)return null;
        const data=JSON.parse(raw);
        return data&&data.version===1&&typeof data.fields==="object"?data:null;
      }catch{return null;}
    };
    const write=(project,video,fields)=>{
      if(!/^[a-f0-9]{12}$/.test(project||""))return;
      try{localStorage.setItem(prefix+project,JSON.stringify({
        version:1,video,fields
      }));}catch{}
    };
    return {read,write};
  })();
  const make=(tag,klass,text)=>{
    const node=document.createElement(tag);
    if(klass)node.className=klass;
    if(text!==undefined)node.textContent=String(text);
    return node;
  };
  const clear=node=>{while(node.firstChild)node.firstChild.remove();};
  const api=async(path,opts={})=>{
    const res=await window.VolleyballStudioActivity.fetch(path,{cache:"no-store",credentials:"same-origin",...opts});
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
  const requestId=()=>{
    const id=window.crypto?.randomUUID?.().replace(/-/g,"");
    if(!/^[a-f0-9]{32}$/.test(id||""))
      throw Error("Для безопасного запуска нужен криптографический идентификатор запроса");
    return id;
  };
  // A lost POST response is ambiguous: the Rust pipeline may already be
  // queued. Check the durable server status before reporting an error.
  const startWithRecovery=async payload=>{
    try{return await post("/api/studio/pipeline/start",payload);}
    catch(error){
      if(!/failed to fetch|networkerror|network request failed|load failed/i.test(
        String(error?.message||error)))throw error;
      saveMessage("Связь прервана. Проверяем, был ли запуск принят сервером…");
      for(let attempt=0;attempt<5;attempt++){
        try{
          const status=await get("/api/studio/pipeline/status",{
            project_id:payload.project_id
          });
          if(status.schema==="volley-studio-pipeline/1"&&
              status.project_id===payload.project_id&&
              status.client_request_id===payload.client_request_id&&
              status.video===payload.video&&status.mode===payload.mode&&
              /^[a-f0-9]{32}$/.test(String(status.id||""))){
            return {schema:status.schema,id:status.id,status:status.status,recovered:true};
          }
        }catch{/* WSL port forwarding may briefly disconnect. */}
        if(attempt<4)await new Promise(resolve=>setTimeout(resolve,500));
      }
      // Same client token: the Rust endpoint is idempotent even if the
      // original POST was accepted but its response disappeared.
      try{return await post("/api/studio/pipeline/start",payload);}
      catch{
        throw Error("Связь с сервером потеряна. Запуск мог продолжиться. "+
          "Обновите страницу, чтобы восстановить задачу, прежде чем запускать её снова.");
      }
    }
  };
  const announce=(text,error=false)=>{
    const banner=$("studio-banner");
    banner.textContent=String(text);
    banner.className=error?"banner error":"banner ok";
  };
  const failure=e=>announce(e instanceof Error?e.message:String(e),true);
  const stopPipelinePolling=()=>{
    if(state.pipelineTimer!==null)clearInterval(state.pipelineTimer);
    state.pipelineTimer=null;
  };
  const switchTab=(requested)=>{
    const previous=state.tab;
    const next=runtime.navigate(requested);
    if(previous!==next)runtime.beginLoad(previous);
    state.tab=next;
    ui.preview.classList.toggle("hidden",next!=="preview");
    ui.montage.classList.toggle("hidden",next!=="montage");
    ui.quick.classList.toggle("hidden",next!=="quick");
    ui.analysis.classList.toggle("hidden",next!=="analysis");
    ui.players.classList.toggle("hidden",next!=="players");
    ui.report.classList.toggle("hidden",next!=="report");
    window.dispatchEvent(new CustomEvent("volley:tab-change",{detail:{tab:next}}));
    for(const nav of document.querySelectorAll("[data-studio-tab]")){
      const active=nav.dataset.studioTab===next;
      nav.classList.toggle("active",active);
      if(active)nav.setAttribute("aria-current","page");
      else nav.removeAttribute("aria-current");
    }
    if(next==="analysis"&&state.formProject!==projectId())loadVideos().catch(failure);
    if(next==="players"&&!state.playersLoaded)loadPlayers().catch(failure);
  };
  const projectId=()=>ui.project.value;
  const inputValue=(id)=>{
    const input=$(id),raw=input.value.trim(),value=Number(raw);
    const low=Number(input.min),high=Number(input.max);
    if(!raw||!Number.isFinite(value)||value<low||value>high||
        (input.step==="1"&&!Number.isSafeInteger(value))||!input.checkValidity()){
      throw Error(input.labels?.[0]?.textContent?.trim()+": недопустимое значение");
    }
    return value;
  };
  const gpuProfile=()=>({
    stride:inputValue("studio-gpu-stride"),
    gpu_memory_limit_mb:inputValue("studio-gpu-memory"),
    cpu_threads:inputValue("studio-gpu-threads"),
    pose_enabled:$("studio-gpu-pose-enabled").value==="1",
    pose_stride:inputValue("studio-gpu-pose-stride")
  });
  // Mirrors volley-core's validated native defaults (no JS-only processing).
  const postProfile=()=>({
    pose_classification:$("studio-post-pose-classify").value==="1",
    tracking:{
      min_player_score:inputValue("studio-post-player-score"),
      nms_iou:inputValue("studio-post-nms"),
      max_gap_frames:inputValue("studio-post-max-gap"),
      min_track_observations:inputValue("studio-post-min-track-frames"),
      min_assignment_score:inputValue("studio-post-assignment"),
      ambiguity_margin:inputValue("studio-post-margin")
    },
    touches:{
      threshold:inputValue("studio-post-touch-threshold"),
      smooth:inputValue("studio-post-smooth"),
      min_gap:inputValue("studio-post-touch-gap"),
      max_reach_ratio:inputValue("studio-post-reach"),
      min_interval_sec:inputValue("studio-post-interval")
    },
    rallies:{
      min_ball_score:inputValue("studio-post-ball-score"),
      max_ball_gap_frames:inputValue("studio-post-ball-gap"),
      max_idle_frames:inputValue("studio-post-idle"),
      stationary_px_per_frame:inputValue("studio-post-stationary"),
      min_rally_frames:inputValue("studio-post-rally-min"),
      min_ball_samples:inputValue("studio-post-samples"),
      min_ball_coverage:inputValue("studio-post-coverage"),
      min_touch_candidates:inputValue("studio-post-min-touches"),
      max_without_touch_frames:inputValue("studio-post-no-touch"),
      suggested_context_frames:inputValue("studio-post-context")
    }
  });
  const allProfileFields=()=>[...ui.analysis.querySelectorAll("input[data-default],select[data-default]")];
  const saveAnalysisSettings=()=>{
    const fields={};
    for(const input of allProfileFields()){
      if(input.value!==input.dataset.default)
        fields[input.id]=[input.dataset.default,input.value];
    }
    analysisFormCache.write(projectId(),ui.video.value,fields);
  };
  const restoreAnalysisSettings=project=>{
    const saved=analysisFormCache.read(project);
    for(const input of allProfileFields()){
      input.value=input.dataset.default;
      const row=saved?.fields?.[input.id];
      if(Array.isArray(row)&&row.length===2&&
          row[0]===input.dataset.default&&typeof row[1]==="string"&&
          row[1].length<=64){
        input.value=row[1];
        if(!input.checkValidity())input.value=input.dataset.default;
      }
    }
    return typeof saved?.video==="string"?saved.video:"";
  };
  const selectedGpuSource=()=>{
    const raw=$("studio-inference")?.value;
    if(!raw)return "";
    try{
      const tuple=JSON.parse(raw);
      return Array.isArray(tuple)&&tuple.length===3&&tuple[0]===ui.video.value&&
        tuple[1]==="fusion"&&/^[a-f0-9]{32}$/.test(tuple[2])?tuple[2]:"";
    }catch{return "";}
  };
  const updateProfile=()=>{
    let gpuValid=true,postValid=true,invalid="";
    try{gpuProfile();}catch(e){gpuValid=false;invalid=String(e.message||e);}
    try{postProfile();}catch(e){postValid=false;invalid=String(e.message||e);}
    const valid=gpuValid&&postValid;
    if(!valid)ui.profileDelta.textContent=invalid;
    if(valid){
      const changed=allProfileFields().filter(item=>item.value!==item.dataset.default);
      ui.profileDelta.textContent=changed.length?
        "Изменено параметров: "+changed.length+
        ". При изменении GPU создаётся новый анализ; при изменении только CPU — новая постобработка.":
        "Эталонный профиль активен. Совпадающие готовые версии используются повторно.";
    }
    const running=Boolean(state.pipelineId);
    ui.start.textContent=running?
      (state.stopRequested?"Остановка анализа…":"■ Остановить анализ"):
      "▶ Полный анализ";
    ui.start.classList.toggle("is-stop",running);
    ui.start.disabled=state.starting||(running?state.stopRequested:
      (!valid||!projectId()||!ui.video.value));
    const blocked=running||state.starting||!projectId()||!ui.video.value;
    ui.gpuOnly.disabled=blocked||!gpuValid;
    ui.postOnly.disabled=blocked||!postValid||!selectedGpuSource();
    ui.postOnly.title=selectedGpuSource()?
      "Создать новый CPU-вариант выбранного GPU-анализа без повторного GPU":
      "Сначала выберите объединённый GPU-анализ в шапке";
  };
  const saveMessage=(message,error=false)=>{
    ui.note.textContent=message;
    ui.note.classList.toggle("analysis-error",error);
  };
  const resetLog=()=>{
    state.logKey="";state.logOffset=0;state.logLastCall="";state.logPending=null;
    ui.logCount.textContent="0 записей";
    ui.logCurrent.textContent="Ожидание запуска";
    ui.logOutput.textContent="Пока нет записей.";
  };
  const formatLogRow=entry=>{
    const date=new Date(Number(entry.timestamp_ms));
    const time=Number.isFinite(date.getTime())?date.toLocaleTimeString("ru-RU",{hour12:false}):"--:--:--";
    const label=String(entry.level||"info").toUpperCase();
    const stage=String(entry.stage||"Обработка");
    const message=String(entry.message||"");
    const heading="["+time+"] "+label+" ["+stage+"] "+message;
    const call=entry.call;
    if(!call||typeof call!=="object")return heading;
    if(call.type==="process"){
      const argv=Array.isArray(call.argv)?call.argv.map(String):[];
      return heading+"\n  EXEC: "+String(call.name||argv[0]||"process")+
        "\n  argv: "+JSON.stringify(argv,null,2);
    }
    return heading+"\n  RUST: "+String(call.name||"unknown")+
      "\n  args: "+JSON.stringify(call.arguments??{},null,2);
  };
  const loadLog=async(status)=>{
    const project=String(status.project_id||""),id=String(status.id||"");
    if(!project||!id||project!==projectId())return;
    if(state.logBusy){state.logPending=status;return;}
    const key=project+":"+id;
    if(state.logKey!==key){
      state.logKey=key;state.logOffset=0;state.logLastCall="";
      ui.logCount.textContent="0 записей";
      ui.logOutput.textContent="Журнал загружается…";
    }
    const after=state.logOffset;
    let more=false;
    state.logBusy=true;
    try{
      const data=await get("/api/studio/pipeline/log",{project_id:project,id,after});
      if(state.logKey!==key||projectId()!==project)return;
      if(data.schema!=="volley-studio-pipeline-log/1"||data.id!==id||
        data.project_id!==project||!Array.isArray(data.entries)||
        !Number.isSafeInteger(data.next_offset)||data.next_offset<after)
        throw Error("Некорректный ответ журнала");
      const atBottom=ui.logOutput.scrollTop+ui.logOutput.clientHeight>=
        ui.logOutput.scrollHeight-55;
      const additions=[];
      for(const entry of data.entries){
        additions.push(formatLogRow(entry));
        if(entry.call&&typeof entry.call==="object")
          state.logLastCall=entry.call.type==="process"?
            "FFmpeg (внешний процесс)":"Rust: "+String(entry.call.name||"unknown");
      }
      if(additions.length){
        if(after===0)ui.logOutput.textContent="";
        ui.logOutput.textContent+=(ui.logOutput.textContent?"\n\n":"")+additions.join("\n\n");
        if(atBottom)ui.logOutput.scrollTop=ui.logOutput.scrollHeight;
      }else if(after===0){
        ui.logOutput.textContent="Ожидание первых событий сервера…";
      }
      state.logOffset=data.next_offset;
      more=data.has_more===true&&data.entries.length>0;
      ui.logCount.textContent=state.logOffset+" записей"+(data.has_more?" · загрузка…":"");
      ui.logCurrent.textContent="Стадия: "+String(status.stage||"—")+
        " · "+(state.logLastCall||"ожидание вызова");
    }catch(error){
      if(state.logKey===key){
        ui.logCurrent.textContent="Журнал: "+String(error.message||error);
      }
    }finally{state.logBusy=false;}
    const pending=state.logPending;state.logPending=null;
    if(pending&&pending.project_id===projectId())await loadLog(pending);
    else if(more&&state.logKey===key)await loadLog(status);
  };
  const historyDuration=ms=>{
    if(!Number.isFinite(ms)||ms<0)return "—";
    const seconds=Math.floor(ms/1000),hours=Math.floor(seconds/3600);
    return hours?hours+" ч "+Math.floor(seconds%3600/60)+" мин":
      seconds>=60?Math.floor(seconds/60)+" мин "+seconds%60+" с":seconds+" с";
  };
  const historyLog=async(project,id,output)=>{
    let after=0,more=true,rows=[];
    try{
      while(more){
        const data=await get("/api/studio/pipeline/log",{project_id:project,id,after});
        if(projectId()!==project)return;
        if(data.schema!=="volley-studio-pipeline-log/1"||data.id!==id||
          data.project_id!==project||!Array.isArray(data.entries)||
          !Number.isSafeInteger(data.next_offset)||data.next_offset<after)
          throw Error("Некорректный ответ журнала");
        rows.push(...data.entries.map(formatLogRow));
        more=data.has_more===true&&data.next_offset>after;
        after=data.next_offset;
      }
      output.textContent=rows.length?rows.join("\n\n"):"Записей нет.";
      return true;
    }catch(error){output.textContent="Не удалось загрузить журнал: "+String(error.message||error);return false;}
  };
  const renderHistory=()=>{
    clear(ui.historyList);
    const rows=state.historyRows.filter(row=>state.historyFilter==="all"||
      row.mode===state.historyFilter||!row.mode||row.mode==="full");
    if(!rows.length){ui.historyList.textContent=state.historyRows.length?"На загруженных страницах запусков нет.":"Запусков пока нет.";return;}
    const labels={gpu:"Нейросеть",post:"Постобработка",full:"Полный анализ"};
    const statuses={queued:"В очереди",running:"Выполняется",complete:"Завершён",failed:"Ошибка",cancelled:"Отменён",interrupted:"Прерван"};
    for(const row of rows){
      const details=make("details","analysis-history-run"),summary=make("summary");
      summary.append(make("strong","",(labels[row.mode]||labels.full)+" · "+String(row.video||"—")));
      const date=Number.isFinite(row.started_ms)?new Date(row.started_ms).toLocaleString():"Дата недоступна";
      summary.append(make("span","",date),make("span","",statuses[row.status]||String(row.status||"—")),
        make("span","",historyDuration(row.duration_ms)));
      if(row.result_reused===true)summary.append(make("span","","Готовый результат"));
      else if(row.gpu_reused===true)summary.append(make("span","","GPU использован повторно"));
      details.append(summary);
      const body=make("div","analysis-history-detail"),stages=make("div","analysis-history-stages");
      if(Array.isArray(row.stages)&&row.stages.length){
        for(const stage of row.stages)stages.append(make("span","",String(stage.name||"Этап")+": "+historyDuration(stage.duration_ms)));
      }else stages.textContent="Длительность этапов для этого запуска недоступна.";
      body.append(stages);
      if(row.error)body.append(make("p","",String(row.error)));
      const log=make("details"),title=make("summary","","Журнал выполнения"),output=make("pre","analysis-run-log-output","Загрузка…");
      log.append(title,output);body.append(log);details.append(body);
      let loaded=false;
      log.addEventListener("toggle",()=>{
        if(log.open&&!loaded){loaded=true;historyLog(projectId(),row.id,output).then(ok=>{if(!ok)loaded=false;});}
      });
      ui.historyList.append(details);
    }
  };
  const loadHistory=async(append=false)=>{
    const project=projectId(),epoch=++state.historyEpoch,offset=append?state.historyOffset:0;
    if(!append){state.historyRows=[];state.historyOffset=0;}
    ui.historyMore.classList.add("hidden");
    if(!project){ui.historyList.textContent="Выберите проект.";return;}
    if(!append)ui.historyList.textContent="Загрузка истории…";
    try{
      const data=await get("/api/studio/pipeline/history",{project_id:project,offset});
      if(epoch!==state.historyEpoch||project!==projectId())return;
      if(data.schema!=="volley-studio-pipeline-history/1"||data.project_id!==project||!Array.isArray(data.runs)||
        !Number.isSafeInteger(data.next_offset)||data.next_offset<offset)
        throw Error("Некорректный ответ истории");
      state.historyRows=append?state.historyRows.concat(data.runs):data.runs;
      state.historyOffset=data.next_offset;
      ui.historyMore.classList.toggle("hidden",data.has_more!==true);
      renderHistory();
    }catch(error){if(epoch===state.historyEpoch){
      if(append)ui.historyMore.classList.remove("hidden");
      else ui.historyList.textContent="Не удалось загрузить историю: "+String(error.message||error);
    }}
  };
  ui.historyRefresh.addEventListener("click",()=>loadHistory());
  ui.historyMore.addEventListener("click",()=>loadHistory(true));
  for(const button of document.querySelectorAll("[data-history-filter]")){
    button.addEventListener("click",()=>{
      state.historyFilter=button.dataset.historyFilter;
      for(const tab of document.querySelectorAll("[data-history-filter]"))
        tab.setAttribute("aria-selected",String(tab===button));
      renderHistory();
    });
  }
  const displayPipeline=(status,notifyCompletion=false)=>{
    if(!status||status.schema!=="volley-studio-pipeline/1")return;
    const percent=Number(status.progress_percent);
    ui.progress.value=Number.isFinite(percent)?Math.max(0,Math.min(percent,100)):0;
    const active=["queued","running","cancelling"].includes(status.status);
    if(status.status==="cancelling")state.stopRequested=true;
    const stage=String(status.stage||"Обработка");
    const project=String(status.project_id||"");
    const mode=["full","gpu","post"].includes(status.mode)?status.mode:"full";
    const runLabel=mode==="gpu"?"Первичный GPU-анализ":
      mode==="post"?"CPU-постобработка":"Полный анализ · GPU/CPU";
    if(active){
      window.VolleyballTasks?.set("analysis",{label:runLabel,
        projectId:project,stage,progress:percent});
      saveMessage(status.status==="cancelling"?
        "Отмена запрошена · ожидаем остановки текущего GPU/CPU-вызова":
        stage+" · "+(Number.isFinite(percent)?percent.toFixed(1)+"%":"—"));
    }else{
      window.VolleyballTasks?.clear("analysis");
      saveMessage(status.status==="complete"?
        (mode==="gpu"?"Новый GPU-анализ сохранён. Версия доступна в шапке.":
         mode==="post"?"Создан новый вариант CPU-постобработки для выбранного GPU.":
         status.result_reused===true?
          "Настройки совпали: выбраны готовые версии GPU и CPU.":
          status.gpu_reused===true?
          "GPU использован повторно. Новая CPU-постобработка доступна в шапке.":
          "GPU и CPU завершены. Новые версии доступны в шапке страницы."):
        status.status==="cancelled"?"Анализ отменён. Частичные версии не выбраны.":
        status.status==="failed"||status.status==="interrupted"?
          "Ошибка анализа: "+String(status.error||stage):
          "Готово к новому запуску.",
        status.status==="failed"||status.status==="interrupted");
    }
    if(notifyCompletion&&status.status==="complete"&&status.id!==state.completedId&&
        status.project_id===projectId()){
      state.completedId=status.id;
      window.dispatchEvent(new CustomEvent("volley:pipeline-complete",{
        detail:{project_id:status.project_id,video:status.video,
          analysis_id:status.analysis_id,postprocessing_id:status.postprocessing_id}
      }));
    }
    if(!active&&status.id===state.pipelineId){
      stopPipelinePolling();
      state.pipelineId="";state.pipelineProject="";state.stopRequested=false;
      loadHistory();
      if(status.status==="failed"||status.status==="interrupted")
        announce(runLabel+": "+String(status.error||stage),true);
    }
    updateProfile();
    loadLog(status).catch(failure);
  };
  const pollPipeline=async()=>{
    if(!state.pipelineId||state.polling)return;
    state.polling=true;
    const id=state.pipelineId,project=state.pipelineProject;
    try{
      const status=await get("/api/studio/pipeline/status",{project_id:project,id});
      if(state.pipelineId===id){
        state.pollErrors=0;
        displayPipeline(status,true);
      }
    }catch(e){
      if(state.pipelineId===id&&++state.pollErrors>=3){
        window.VolleyballTasks?.clear("analysis");
        saveMessage("Не удалось получить состояние анализа: "+String(e.message||e),true);
        stopPipelinePolling();
      }
    }finally{state.polling=false;}
  };
  const restorePipeline=async()=>{
    const project=projectId();
    if(!project||state.pipelineId)return;
    const epoch=state.analysisGeneration;
    const status=await get("/api/studio/pipeline/status",{project_id:project});
    if(project!==projectId()||epoch!==state.analysisGeneration)return;
    if(["queued","running","cancelling"].includes(status.status)){
      state.pipelineId=status.id;
      state.pipelineProject=project;
      state.stopRequested=status.status==="cancelling";
      ui.logPanel.open=true;
      saveMessage("Подключение к уже запущенному анализу…");
      stopPipelinePolling();
      await pollPipeline();
      if(state.pipelineId)state.pipelineTimer=setInterval(pollPipeline,1800);
    }else if(status.status!=="idle"){
      // Restoring an old completed job must not invalidate GPU/CPU snapshots.
      if(status.status==="complete")state.completedId=status.id;
      displayPipeline(status);
    }
    updateProfile();
  };
  const loadVideos=async()=>{
    const project=projectId(),old=ui.video.value;
    const token=runtime.beginLoad("analysis");
    state.analysisGeneration++;
    loadHistory();
    if(!project){
      resetLog();
      clear(ui.video);ui.video.append(make("option","","Сначала выберите проект"));
      window.VolleyballEditor?.load("","");
      updateProfile();
      return;
    }
    const catalog=window.VolleyballStudioProjectCatalog?
      await window.VolleyballStudioProjectCatalog():await api("/api/projects");
    const rows=Array.isArray(catalog.projects)?catalog.projects:[];
    if(state.tab!=="analysis"||project!==projectId()||
        !runtime.currentLoad("analysis",token))return;
    const current=rows.find(p=>p.id===project);
    const videos=Array.isArray(current?.videos)?
      current.videos.filter(name=>typeof name==="string"):[];
    clear(ui.video);
    const option=make("option","","Выберите видео");option.value="";ui.video.append(option);
    for(const name of videos){
      const item=make("option","",name);item.value=name;ui.video.append(item);
    }
    const savedVideo=restoreAnalysisSettings(project);
    const selected=$("studio-inference")?.value||"";
    let chainVideo="";
    try{
      const scope=JSON.parse(selected);
      if(Array.isArray(scope)&&scope[1]==="fusion"&&videos.includes(scope[0]))
        chainVideo=scope[0];
    }catch{}
    ui.video.value=chainVideo||(
      state.formProject===project&&videos.includes(old)?old:
      videos.includes(savedVideo)?savedVideo:
      videos.length===1?videos[0]:"");
    state.formProject=project;
    window.VolleyballEditor?.load(project,ui.video.value,true);
    updateProfile();
    await restorePipeline();
  };
  const launchPipeline=async(mode="full")=>{
    if(state.pipelineId||state.starting)return;
    const project=projectId(),video=ui.video.value;
    if(!project||!video){saveMessage("Выберите проект и исходное видео.",true);return;}
    const source=mode==="post"?selectedGpuSource():"";
    if(mode==="post"&&!source){
      saveMessage("Для постобработки выберите объединённый GPU-анализ в шапке.",true);
      return;
    }
    state.starting=true;updateProfile();
    try{
      const payload={project_id:project,video,mode,client_request_id:requestId()};
      if(mode!=="post")payload.gpu=gpuProfile();
      if(mode!=="gpu")payload.post=postProfile();
      if(mode==="post")payload.source_analysis_id=source;
      const response=await startWithRecovery(payload);
      if(response.schema!=="volley-studio-pipeline/1"||
          !/^[a-f0-9]{32}$/.test(String(response.id||"")))
        throw Error("Сервер не подтвердил создание анализа");
      state.pipelineId=response.id;state.pipelineProject=project;
      loadHistory();
      resetLog();ui.logPanel.open=true;
      ui.progress.value=0;state.stopRequested=false;
      saveMessage(mode==="gpu"?"Запущен отдельный анализ: VballNet → RAVEL → объединение.":
        mode==="post"?"Запущена новая CPU-постобработка выбранного GPU-анализа.":
        "Запущен полный анализ: VballNet → RAVEL → CPU.");
      stopPipelinePolling();
      await pollPipeline();
      if(state.pipelineId)state.pipelineTimer=setInterval(pollPipeline,1800);
    }catch(e){saveMessage(String(e.message||e),true);failure(e);}
    finally{state.starting=false;updateProfile();}
  };
  const cancelPipeline=async()=>{
    if(!state.pipelineId)return;
    if(state.stopRequested)return;
    state.stopRequested=true;updateProfile();
    try{
      await post("/api/studio/pipeline/cancel",{
        project_id:state.pipelineProject,id:state.pipelineId
      });
      saveMessage("Запрошена отмена. Ожидаем остановки текущего GPU/CPU-вызова.");
    }catch(e){state.stopRequested=false;updateProfile();saveMessage(String(e.message||e),true);}
  };
  const resetProfile=()=>{
    for(const input of allProfileFields())input.value=input.dataset.default;
    saveAnalysisSettings();
    updateProfile();
  };
  const renderSidebarPlayers=()=>{
    const sidebar=$("studio-players-sidebar-list");
    if(!sidebar)return;
    clear(sidebar);
    const rows=[...state.players].filter(p=>
      typeof p.id==="string"&&typeof p.name==="string"&&state.projectMemberIds.has(p.id)
    ).sort((a,b)=>String(a.name).localeCompare(String(b.name),"ru"));
    if(!rows.length){
      sidebar.append(make("div","players-sidebar-empty","В проект пока не добавлены игроки."));
      return;
    }
    for(const player of rows){
      const card=make("article","players-side-card");
      const avatar=make("div","players-side-avatar");
      const count=Number.isSafeInteger(player.saved_photo_count)&&
        player.saved_photo_count>=0?player.saved_photo_count:0;
      const refId=player.cover_reference_id;
      const cover=state.trackCovers.get(player.id);
      const saved=typeof refId==="string"&&/^[a-f0-9]{24}$/.test(refId);
      const url=saved?"/api/players/library/photo?"+new URLSearchParams({
        player_id:player.id,reference_id:refId
      }):cover?"/api/players/native/track-photo?"+new URLSearchParams(cover):null;
      if(url){
        const img=make("img");img.loading="lazy";img.decoding="async";
        img.alt=(saved?"Сохранённое фото ":"Кадр привязанного Track ID ")+player.name;
        img.title=saved?"Сохранённый фотоэталон":
          "Кадр подтверждённого Track ID, ещё не сохранённый фотоэталон";
        img.src=url;
        img.addEventListener("error",()=>{
          img.remove();avatar.textContent=player.name.slice(0,1).toUpperCase();
        });
        avatar.append(img);
      }else avatar.textContent=player.name.slice(0,1).toUpperCase();
      const meta=make("div","players-side-meta");
      meta.append(make("strong","",player.name),
        make("small","",count+" сохранённых фото"),
        make("span","players-side-membership"+
          (state.projectMemberIds.has(player.id)?" is-linked":""),
          state.projectMemberIds.has(player.id)?"В проекте":"В библиотеке"));
      const menu=make("details","players-side-menu");
      const selectPlayer=make("button","players-side-select");
      selectPlayer.type="button";
      selectPlayer.ariaLabel="Показать привязанные Track ID игрока "+player.name;
      selectPlayer.addEventListener("click",()=>window.dispatchEvent(
        new CustomEvent("volley:player-gallery-select",{
          detail:{project_id:projectId(),player_id:player.id,name:player.name}
        })));
      const toggle=make("summary","","⋮");
      toggle.ariaLabel="Действия с фотографиями "+player.name;
      const show=make("button","","Привязанные фото");show.type="button";
      const attach=make("button","","Привязать фото от Track ID");attach.type="button";
      attach.title="Выберите один Track ID. Он также будет привязан к этому игроку.";
      const gallery=make("div","players-side-gallery");gallery.hidden=true;
      show.addEventListener("click",async()=>{
        menu.open=false;
        if(!gallery.hidden){gallery.hidden=true;return;}
        gallery.hidden=false;clear(gallery);
        gallery.append(make("span","","Загрузка фото…"));
        const project=projectId();
        try{
          const album=await post("/api/players/native/references",{
            project_id:project,player_id:player.id,action:"read"
          });
          if(project!==projectId())return;
          clear(gallery);
          const seen=new Set();
          for(const photo of Array.isArray(album.photos)?album.photos:[]){
            if(seen.has(photo.image_sha256))continue;
            seen.add(photo.image_sha256);
            const img=make("img");img.loading="lazy";
            img.alt="Фото игрока "+player.name+" · Track ID "+photo.track_id;
            img.src="/api/players/library/photo?"+new URLSearchParams({
              player_id:player.id,reference_id:photo.id
            });
            gallery.append(img);
          }
          if(!seen.size)gallery.append(make("span","","Сохранённых фото пока нет."));
        }catch(error){clear(gallery);gallery.append(make("span","",String(error.message||error)));}
      });
      attach.addEventListener("click",()=>{
        menu.open=false;
        window.dispatchEvent(new CustomEvent("volley:enroll-selected-photo",{
          detail:{project_id:projectId(),player_id:player.id}
        }));
      });
      menu.append(toggle,show,attach);
      card.append(avatar,meta,menu,gallery,selectPlayer);
      sidebar.append(card);
    }
  };
  const renderPlayers=()=>{
    clear(ui.playerList);
    renderSidebarPlayers();
    ui.playerCount.textContent=String(state.players.length)+" записей";
    if(!state.players.length){
      ui.playerList.append(make("div","empty-state","В библиотеке пока нет игроков."));
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
        make("small","","ID "+player.id+" · "+
          (Number.isSafeInteger(player.saved_photo_count)?
            player.saved_photo_count:0)+" сохранённых фото · "+
          (state.projectMemberIds.has(player.id)?"в проекте":"не в проекте")));
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
            window.dispatchEvent(new Event("volley:player-library-updated"));
          }catch(e){
            failure(e);
            await loadPlayers().catch(failure);
          }finally{state.mutating=false;}
        });
        form.append(input,save,cancel);row.append(form);input.focus();
      });
      const remove=make("button","secondary player-delete","Удалить");
      remove.type="button";
      remove.title="Исключить из активной библиотеки; исторические фото и привязки сохранятся";
      remove.addEventListener("click",async()=>{
        if(state.mutating)return;
        if(!window.confirm("Удалить игрока «"+player.name+
          "» из активной библиотеки? Исторические фото и привязки сохранятся."))return;
        state.mutating=true;remove.disabled=true;edit.disabled=true;
        try{
          const result=await post("/api/players/library/native",{
            action:"delete",player_id:player.id,
            expected_revision:state.revision
          });
          state.revision=String(result.revision);
          state.players=Array.isArray(result.players)?result.players:[];
          renderPlayers();announce("Игрок исключён из активной библиотеки.");
          window.dispatchEvent(new Event("volley:player-library-updated"));
        }catch(e){
          failure(e);
          await loadPlayers().catch(failure);
        }finally{state.mutating=false;}
      });
      const actions=make("div","player-library-actions");
      actions.append(edit,remove);
      row.append(meta,actions);
      ui.playerList.append(row);
    }
  };
  const loadPlayers=async()=>{
    const token=++state.playerGeneration;
    const wasmToken=runtime.beginLoad("players");
    ui.playerList.replaceChildren(make("div","empty-state","Загрузка библиотеки игроков…"));
    const result=await post("/api/players/library/native",{action:"read"});
    if(token!==state.playerGeneration||!runtime.currentLoad("players",wasmToken))return;
    state.players=Array.isArray(result.players)?result.players:[];
    state.revision=String(result.revision||"absent");
    state.playersLoaded=true;
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
      renderPlayers();announce("Игрок добавлен в библиотеку студии.");
      window.dispatchEvent(new Event("volley:player-library-updated"));
    }catch(e){
      failure(e);
      await loadPlayers().catch(failure);
    }finally{state.mutating=false;}
  });
  ui.playerRefresh.addEventListener("click",()=>loadPlayers().catch(failure));
  for(const field of allProfileFields())field.addEventListener(field.tagName==="SELECT"?"change":"input",()=>{
    updateProfile();saveAnalysisSettings();
  });
  ui.reset.addEventListener("click",event=>{event.preventDefault();resetProfile();});
  ui.start.addEventListener("click",()=>state.pipelineId?cancelPipeline():launchPipeline());
  ui.gpuOnly.addEventListener("click",()=>launchPipeline("gpu"));
  ui.postOnly.addEventListener("click",()=>launchPipeline("post"));
  ui.video.addEventListener("change",()=>{
    saveAnalysisSettings();
    window.VolleyballEditor?.load(projectId(),ui.video.value);
    updateProfile();
  });
  updateProfile();
  window.addEventListener("volley:chain-change",event=>{
    const detail=event.detail||{};
    if(detail.project_id!==projectId()||!detail.video||state.tab!=="analysis")return;
    if([...ui.video.options].some(option=>option.value===detail.video)&&
        ui.video.value!==detail.video){
      ui.video.value=detail.video;
      window.VolleyballEditor?.load(projectId(),ui.video.value);
    }
    updateProfile();
  });
  ui.project.addEventListener("change",()=>{
    resetLog();
    state.historyEpoch++;state.historyRows=[];state.historyOffset=0;ui.historyMore.classList.add("hidden");ui.historyList.textContent="Выберите проект.";
    state.analysisGeneration++;
    // project-ready is the canonical data refresh event (no double fetch).
  });
  window.addEventListener("volley:track-photo-covers",event=>{
    const detail=event.detail||{};
    if(detail.project_id!==projectId())return;
    const scope=[detail.project_id,detail.video,detail.replay_id].join("/");
    if(state.trackCoverScope!==scope){
      state.trackCoverScope=scope;
      state.trackCovers.clear();
    }
    for(const row of Array.isArray(detail.tracks)?detail.tracks:[]){
      if(typeof row.player_id==="string"&&Number.isSafeInteger(row.track_id)&&
        row.track_id>0&&!state.trackCovers.has(row.player_id)){
        state.trackCovers.set(row.player_id,{
          project_id:detail.project_id,video:detail.video,
          replay_id:detail.replay_id,track_id:String(row.track_id)
        });
      }
    }
    renderSidebarPlayers();
  });
  window.addEventListener("volley:roster-updated",event=>{
    const detail=event.detail||{};
    if(detail.project_id!==projectId())return;
    state.projectMemberIds=new Set(Array.isArray(detail.player_ids)?
      detail.player_ids:[]);
    renderSidebarPlayers();
    if(state.playersLoaded)renderPlayers();
  });
  window.addEventListener("volley:player-photo-saved",()=>{
    if(state.tab==="players")loadPlayers().catch(failure);
  });
  window.addEventListener("volley:project-ready",()=>{
    state.playersLoaded=false;
    state.projectMemberIds=new Set();
    state.trackCovers.clear();state.trackCoverScope="";
    resetLog();
    state.analysisGeneration++;
    if(state.tab==="analysis")loadVideos().catch(failure);
  });
  for(const nav of document.querySelectorAll("[data-studio-tab]")){
    nav.addEventListener("click",event=>{
      event.preventDefault();
      const requested=nav.dataset.studioTab;
      runtime.ready.then(()=>{
        switchTab(requested);
        history.replaceState(null,"","#"+runtime.tabs[runtime.get("tab")]);
      }).catch(failure);
    });
  }
  window.addEventListener("hashchange",()=>{
    runtime.ready.then(()=>switchTab(location.hash.slice(1))).catch(failure);
  });
  runtime.ready.then(()=>switchTab(location.hash.slice(1))).catch(failure);
})();
