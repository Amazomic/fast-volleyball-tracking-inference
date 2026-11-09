"use strict";
// Native reviewed-attack highlights UI. No client-supplied frame bounds,
// automatic action/identity claims or proxy-video extraction.
(() => {
  const get=id=>document.getElementById(id);
  const ui={
    project:get("studio-project"),player:get("studio-highlights-player"),
    mode:get("studio-highlights-mode"),count:get("studio-highlights-count"),
    refresh:get("studio-highlights-refresh"),preview:get("studio-highlights-preview"),
    start:get("studio-highlights-start"),cancel:get("studio-highlights-cancel"),
    status:get("studio-highlights-status"),sources:get("studio-highlights-sources"),
    candidates:get("studio-highlights-candidates"),
    job:get("studio-highlights-job"),download:get("studio-highlights-download"),
    encoder:get("studio-encoder"),
    layout:get("studio-highlights-layout"),music:get("studio-highlights-music"),
    volume:get("studio-highlights-volume")
  };
  const state={
    token:0,project:"",busy:false,sources:new Map(),selectedRuns:new Map(),
    candidates:[],chosen:new Set(),previewFingerprint:null,previewKey:null,
    jobId:"",timer:null,previewClips:0
  };
  const el=(tag,klass,text)=>{
    const n=document.createElement(tag);if(klass)n.className=klass;
    if(text!==undefined)n.textContent=String(text);return n;
  };
  const clear=node=>node.replaceChildren();
  const note=(message,error=false)=>{
    ui.status.textContent=String(message);
    ui.status.classList.toggle("track-error",error);
  };
  const validRun=id=>typeof id==="string"&&/^[a-f0-9]{32}$/.test(id);
  const validSha=id=>typeof id==="string"&&/^[a-f0-9]{64}$/.test(id);
  const selectedRunIds=()=>Array.from(state.selectedRuns.values()).filter(validRun).sort();
  const manualKeys=()=>state.mode==="manual"?Array.from(state.chosen):[];
  // Shared URL route is source-aware and re-verifies full SHA before rendering.
  const api=async(url,options={})=>{
    const response=await fetch(url,{credentials:"same-origin",cache:"no-store",...options});
    const body=await response.json().catch(()=>({}));
    if(!response.ok)throw Error(body.message||body.error||"HTTP "+response.status);
    return body;
  };
  const post=(url,body)=>api(url,{method:"POST",
    headers:{"Content-Type":"application/json"},body:JSON.stringify(body)});
  const payload=operation=>({
    project_id:state.project,player_id:ui.player.value,
    rally_run_ids:selectedRunIds(),operation,selection_mode:ui.mode.value,
    manual_rally_keys:ui.mode.value==="manual"?Array.from(state.chosen):[],
    max_clips:Number(ui.count.value),encoder:ui.encoder.value,
    layout:ui.layout.value,audio_volume:Number(ui.volume.value)
  });
  const selectionKey=()=>JSON.stringify(payload("preview"));
  const clearPreview=()=>{
    state.previewFingerprint=null;state.previewKey=null;state.previewClips=0;
    ui.start.disabled=true;
    ui.download.classList.add("hidden");ui.download.removeAttribute("href");
  };
  const controls=()=>{
    const ok=!state.busy && state.project && ui.player.value &&
      selectedRunIds().length>0 &&
      Number.isSafeInteger(Number(ui.count.value))&&
      Number(ui.count.value)>=1&&Number(ui.count.value)<=80;
    ui.preview.disabled=!ok || (ui.mode.value==="manual" &&
      (state.chosen.size===0||state.chosen.size>Number(ui.count.value)));
    ui.start.disabled=!ok || !validSha(state.previewFingerprint) ||
      state.previewKey!==selectionKey() ||state.previewClips===0 ||
      (ui.mode.value==="manual"&&state.chosen.size===0);
    ui.refresh.disabled=state.busy;
  };
  const reset=()=>{
    state.token++;state.sources.clear();state.selectedRuns.clear();
    state.candidates=[];state.chosen.clear();
    clearPreview();clear(ui.sources);clear(ui.candidates);
    ui.player.replaceChildren(el("option","","Выберите игрока"));
    ui.player.firstChild.value="";
    if(state.timer!==null)clearInterval(state.timer);
    state.timer=null;state.jobId="";ui.cancel.classList.add("hidden");
    clear(ui.job);state.project=ui.project.value;controls();
  };
  const load=async()=>{
    reset();const token=state.token,project=state.project;
    if(!project){note("Выберите проект.");return;}
    state.busy=true;controls();note("Ищем сохранённые CPU-версии розыгрышей и состав игроков…");
    try{
      const [projects,registry]=await Promise.all([
        api("/api/projects"),post("/api/players/library/native",{action:"read"})
      ]);
      if(token!==state.token)return;
      const item=(Array.isArray(projects.projects)?projects.projects:[])
        .find(row=>row.id===project);
      if(!item)throw Error("Проект отсутствует в реестре");
      const roster=Array.isArray(item.player_ids)?new Set(item.player_ids):null;
      const players=(Array.isArray(registry.players)?registry.players:[])
        .filter(p=>typeof p.id==="string"&&typeof p.name==="string"&&
          (roster===null||roster.has(p.id)));
      for(const player of players){
        const opt=el("option","",player.name);opt.value=player.id;ui.player.append(opt);
      }
      ui.player.value=players.length===1?players[0].id:"";
      const videos=Array.isArray(item.videos)?item.videos:[];
      if(videos.length>40)throw Error("Максимум 40 видео для нарезки. Выберите другой проект.");
      for(const video of videos){
        if(typeof video!=="string")continue;
        const overview=await api("/api/studio/rust/analysis-overview?"+
          new URLSearchParams({project_id:project,video}));
        if(token!==state.token)return;
        if(overview.schema!=="volley-native-analysis-overview/1" ||
          overview.project_id!==project||overview.video!==video||
          overview.metadata_only!==true)throw Error("Неизвестная версия CPU-индекса");
        const stage=(Array.isArray(overview.stages)?overview.stages:[])
          .find(row=>row.key==="rallies");
        if(stage?.truncated)throw Error("Индекс CPU-версий "+video+" неполный");
        const versions=(Array.isArray(stage?.items)?stage.items:[])
          .filter(row=>validRun(row.id));
        state.sources.set(video,versions);
        if(versions.length)state.selectedRuns.set(video,versions[0].id);
      }
      clear(ui.sources);
      for(const [video,versions] of state.sources){
        const label=el("label","native-stats-source-choice");
        label.append(el("span","",video));
        const select=el("select");
        const none=el("option","","Пропустить видео");
        none.value="";select.append(none);
        for(const v of versions){
          const option=el("option","",
            v.id.slice(0,10)+" · "+(v.count??0)+" розыгрышей");
          option.value=v.id;select.append(option);
        }
        select.value=state.selectedRuns.get(video)||"";
        select.addEventListener("change",()=>{
          if(select.value)state.selectedRuns.set(video,select.value);
          else state.selectedRuns.delete(video);
          state.candidates=[];state.chosen.clear();
          clear(ui.candidates);clearPreview();controls();
          note("Выбраны новые CPU-версии. Повторите проверку атак.");
        });
        label.append(select);ui.sources.append(label);
      }
      note("CPU-видео: "+state.selectedRuns.size+
        " · игроков: "+players.length+
        ". Для автоатаки нужны подтверждённые действия и ручные Track ID.");
    }catch(e){
      if(token===state.token){
        state.selectedRuns.clear();state.sources.clear();
        clear(ui.sources);clearPreview();note(e.message,true);
      }
    }finally{if(token===state.token){state.busy=false;controls();}}
  };
  const showCandidates=data=>{
    if(data.schema!=="volley-native-reviewed-player-highlights/1"||
      data.project_id!==state.project||data.player_id!==ui.player.value||
      !validSha(data.source_fingerprint)||data.reviewed_actions_only!==true||
      data.manually_linked_athlete_only!==true||
      data.automatic_action_classification!==false||
      data.render_source!=="original_recording"||
      !Array.isArray(data.candidates)||!Array.isArray(data.selected)||
      data.selected_clips!==data.selected.length)
      throw Error("Непроверенный источник атак в ответе сервера");
    const selectedRallies=new Set(data.selected.map(row=>row.key));
    if(data.selected.some(row=>!data.candidates.some(c=>c.key===row.key))||
      data.candidates.some(c=>!validRun(c.rally_run_id)||
        !/^[a-f0-9]{24}$/.test(c.candidate_id)||
        c.key!==c.rally_run_id+":"+c.candidate_id||
        c.action_label_source!=="explicit_human_review"||
        c.athlete_link_source!=="manually_reviewed_native_track_ids"||
        !Number.isSafeInteger(c.manually_verified_player_attacks)||
        c.manually_verified_player_attacks<1))
      throw Error("Список атак содержит недостоверные события");
    const attackCount=data.selected.reduce((n,r)=>n+r.manually_verified_player_attacks,0);
    if(attackCount!==data.selected_verified_attacks)
      throw Error("Количество атак не согласуется с источником");
    state.candidates=data.candidates;
    state.chosen=new Set(data.selected.map(row=>row.key));
    clear(ui.candidates);
    for(const row of data.candidates){
      const label=el("label","native-highlights-candidate");
      const check=el("input");
      check.type="checkbox";check.checked=selectedRallies.has(row.key);
      check.disabled=ui.mode.value!=="manual";
      check.addEventListener("change",()=>{
        if(check.checked)state.chosen.add(row.key);
        else state.chosen.delete(row.key);
        clearPreview();controls();
        note("Ручной список изменён. Проверьте его заново перед монтажом.");
      });
      label.append(check,el("span","",row.video+" · кадры "+
        row.detected_start_frame+"–"+row.detected_end_frame+
        " · подтверждённых атак: "+row.manually_verified_player_attacks));
      ui.candidates.append(label);
    }
    state.previewClips=data.selected_clips;
    state.previewFingerprint=data.source_fingerprint;
    state.previewKey=selectionKey();
    note(data.selected_clips?
      "Подтверждённых атак: "+data.selected_verified_attacks+
      " · розыгрышей для монтажа: "+data.selected_clips+
      ". Монтаж будет выполнен из оригинала.":
      "Подтверждённых атак этого игрока нет. Сначала вручную разметьте действия во вкладке анализа.");
  };
  const preview=async()=>{
    if(state.busy||!selectedRunIds().length||!ui.player.value)return;
    const request=payload("preview"),token=state.token;
    state.busy=true;clearPreview();controls();
    note("Проверяем SHA исходников, события Touch и ручную привязку игрока…");
    try{
      const response=await post("/api/montage/native/player-highlights",request);
      if(token!==state.token||request.project_id!==state.project)return;
      showCandidates(response);
    }catch(e){if(token===state.token){clearPreview();note(e.message,true);}}
    finally{if(token===state.token){state.busy=false;controls();}}
  };
  const stopPolling=()=>{
    if(state.timer!==null)clearInterval(state.timer);
    state.timer=null;
  };
  const poll=async()=>{
    if(!state.jobId)return;
    try{
      const response=await api("/api/montage/native/status");
      if(response.id!==state.jobId){
        ui.job.textContent="Сейчас выполняется другой монтаж.";
        stopPolling();ui.cancel.classList.add("hidden");return;
      }
      ui.job.textContent=(response.stage||response.state||"Монтаж")+
        " · "+Math.round(Number(response.overall_percent)||0)+"%";
      if(response.state==="complete"){
        stopPolling();ui.cancel.classList.add("hidden");
        if(typeof response.media_url==="string"&&
          response.media_url.startsWith("/api/montage/native/media?")){
          ui.download.href=response.media_url;ui.download.classList.remove("hidden");
        }
      }else if(response.state==="cancelled"||response.state==="failed"){
        stopPolling();ui.cancel.classList.add("hidden");
        ui.download.classList.add("hidden");ui.download.removeAttribute("href");
        ui.job.textContent=response.state==="cancelled"?
          "Монтаж отменён, готовый MP4 не опубликован.":
          "Ошибка монтажа: "+(response.error||"неизвестная ошибка");
      }
    }catch(e){ui.job.textContent=e.message;}
  };
  const uploadAudio=async()=>{
    const file=ui.music.files?.[0];
    if(!file)return null;
    if(file.size>90*1024*1024)throw Error("Музыкальный файл превышает 90 МиБ");
    const form=new FormData();
    form.append("file",file,file.name);
    const result=await api("/api/montage/native/audio",{method:"POST",body:form});
    if(typeof result.audio_file!=="string"||
       !/^track_[a-f0-9]{32}\.(mp3|m4a|wav|flac|aac|ogg|opus)$/.test(result.audio_file))
      throw Error("Сервер вернул недопустимую музыкальную дорожку");
    return result.audio_file;
  };
  const start=async()=>{
    if(state.busy||!state.previewFingerprint||
      state.previewKey!==selectionKey())return;
    const request={...payload("start"),
      expected_source_fingerprint:state.previewFingerprint};
    const token=state.token;
    state.busy=true;controls();ui.download.classList.add("hidden");
    note("Повторная проверка ручной разметки и запуск оригинального видео…");
    try{
      request.audio_file=await uploadAudio();
      const result=await post("/api/montage/native/player-highlights",request);
      if(token!==state.token)return;
      if(result.schema!=="volley-native-reviewed-player-highlights/1"||
        result.source_fingerprint!==request.expected_source_fingerprint||
        !/^montage_[a-f0-9]{32}$/.test(result.job_id)||
        result.status!=="queued")throw Error("Сервер не подтвердил запуск нарезки");
      state.jobId=result.job_id;
      ui.job.textContent="Монтаж запущен: "+state.jobId.slice(0,18);
      ui.cancel.classList.remove("hidden");
      stopPolling();state.timer=setInterval(poll,1000);
      await poll();
      note("Выбрано "+result.selected_clips+" розыгрышей. Отмена доступна до завершения.");
    }catch(e){if(token===state.token)note(e.message,true);}
    finally{if(token===state.token){state.busy=false;controls();}}
  };
  ui.refresh.addEventListener("click",load);
  ui.preview.addEventListener("click",preview);
  ui.start.addEventListener("click",start);
  ui.player.addEventListener("change",()=>{
    state.candidates=[];state.chosen.clear();clear(ui.candidates);
    clearPreview();controls();
  });
  for(const control of [ui.layout,ui.volume,ui.encoder]) {
    control.addEventListener("change",()=>{clearPreview();controls();
      note("Формат или звук изменён. Повторите предпросмотр перед монтажом.");});
  }
  for(const control of [ui.mode,ui.count]){
    control.addEventListener("change",()=>{
      clearPreview();
      for(const row of ui.candidates.children){
        const check=row.querySelector("input");
        if(check)check.disabled=ui.mode.value!=="manual";
      }
      controls();
      note("Настройки выбора изменены. Повторите предпросмотр.");
    });
  }
  ui.cancel.addEventListener("click",async()=>{
    if(!state.jobId)return;
    ui.cancel.disabled=true;
    try{
      const result=await post("/api/montage/native/cancel",{job_id:state.jobId});
      if(result.job_id!==state.jobId)throw Error("Не совпадает задача отмены");
      ui.job.textContent="Отмена отправлена. Проверяем остановку FFmpeg…";
      await poll();
    }catch(e){ui.job.textContent=e.message;}
    finally{ui.cancel.disabled=false;}
  });
  ui.project.addEventListener("change",()=>{
    if(ui.project.value!==state.project)load();
  });
  window.addEventListener("volley:project-ready",()=>{
    if(ui.project.value!==state.project)load();
  });
  if(ui.project.value)load();
})();
