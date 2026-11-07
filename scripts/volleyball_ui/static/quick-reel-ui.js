"use strict";
/* Quick Reel: path-only local sources, shared caches, persistent header progress. */
(() => {
  const E=id=>document.getElementById(id);
  const files=new Map();
  let music=null, busy=false, running=false, reelId="",serverBusy=false;
  let pollTimer=null, loadingStatus=false, cancelPending=false;
  const dismissKey="volley-quick-reel-dismissed-id";
  let dismissedReelId="";
  try{dismissedReelId=localStorage.getItem(dismissKey)||""}catch{}
  // Completed exports live on the server; do not persist result IDs in the browser.
  let historyRequest=0;

  async function refreshHistory(){
    const request=++historyRequest;
    const list=E("quick-reel-history-list");
    try{
      const payload=await api("/api/quick-reel/history");
      if(request!==historyRequest)return;
      list.replaceChildren();
      const rows=Array.isArray(payload.results)?payload.results:[];
      if(!rows.length){
        const empty=document.createElement("p");
        empty.className="quick-reel-empty";
        empty.textContent="Готовых рилсов пока нет.";
        list.append(empty);
        return;
      }
      for(const item of rows){
        const row=document.createElement("div");
        row.className="quick-reel-history-row";
        const primary=document.createElement("a");
        primary.href=item.url;
        primary.target="_blank";
        primary.rel="noopener";
        const when=new Date(Number(item.created_at)*1000);
        const date=Number.isNaN(when.getTime())?"Рилс":when.toLocaleString(
          document.documentElement.lang==="en"?"en-US":"ru-RU",
          {day:"2-digit",month:"2-digit",year:"numeric",hour:"2-digit",minute:"2-digit"});
        const names=(item.videos||[]).map(String);
        primary.textContent=date+" · "+(names.length
          ?names.slice(0,2).join(", ")+(names.length>2?" +"+(names.length-2):"")
          :"Готовый рилс")+(Number.isFinite(Number(item.duration))
             ?" · "+fmt(item.duration):"");
        primary.title="Открыть готовое видео";
        const download=document.createElement("a");
        download.href=item.download_url;
        download.download="quick_reel_"+String(item.id).slice(0,8)+".mp4";
        download.textContent="Скачать MP4";
        download.className="quick-reel-history-download";
        const remove=document.createElement("button");
        remove.type="button";
        remove.className="quick-reel-history-delete";
        remove.textContent="×";
        remove.title="Удалить готовый рилс";
        remove.setAttribute("aria-label","Удалить рилс от "+date);
        remove.addEventListener("click",async()=>{
          if(!window.confirm("Удалить этот готовый рилс без возможности восстановления?"))
            return;
          remove.disabled=true;
          try{
            await api("/api/quick-reel/delete",{id:item.id});
            await refreshHistory();
          }catch(error){
            remove.disabled=false;
            status("Не удалось удалить рилс: "+error.message,true);
          }
        });
        row.append(primary,download,remove);
        list.append(row);
      }
    }catch(error){
      if(request===historyRequest){
        list.replaceChildren();
        const message=document.createElement("p");
        message.className="quick-reel-empty";
        message.textContent="Не удалось загрузить историю: "+error.message;
        list.append(message);
      }
    }
  }

  async function api(url,payload){
    let response;
    try{
      response=await fetch(url,payload===undefined
        ?{cache:"no-store"}
        :{method:"POST",headers:{"Content-Type":"application/json"},
          body:JSON.stringify(payload),cache:"no-store"});
    }catch{
      throw Error("Нет связи с Volley Studio. Проверьте локальный сервер.");
    }
    let result;
    try{result=await response.json()}catch{throw Error("Сервер вернул некорректный ответ")}
    if(!response.ok)throw Error(result.error||"Ошибка "+response.status);
    return result;
  }
  async function checkBackend(){
    const state=await api("/api/health");
    if(!state.ok||state.reload_required||Number(state.api_version)<15)
      throw Error("Сервер не обновлён. Перезапустите Volley Studio и обновите страницу.");
  }
  function status(message="",error=false){
    const node=E("quick-reel-status");
    node.textContent=message;
    node.classList.toggle("is-error",error);
  }
  const fmt=value=>{
    const n=Math.max(0,Math.round(Number(value)||0));
    return String(Math.floor(n/60)).padStart(2,"0")+":"+
      String(n%60).padStart(2,"0");
  };
  function fileRow(container,name,remove){
    const line=document.createElement("div");
    line.className="quick-reel-file-row";
    const text=document.createElement("span");text.textContent=name;
    const button=document.createElement("button");
    button.type="button";button.textContent="Убрать";
    button.disabled=busy||running;
    button.addEventListener("click",remove);
    line.append(text,button);
    container.append(line);
  }
  function render(){
    const list=E("quick-reel-videos");list.replaceChildren();
    for(const [id,item] of files){
      fileRow(list,item.name,()=>{files.delete(id);render()});
    }
    if(!files.size){
      const empty=document.createElement("span");
      empty.className="quick-reel-empty";empty.textContent="Нет выбранных видео";
      list.append(empty);
    }
    E("quick-reel-video-count").textContent=String(files.size);
    E("quick-reel-video-section").classList.toggle("is-ready",files.size>0);
    const audio=E("quick-reel-music");audio.replaceChildren();
    if(music)fileRow(audio,music.name,()=>{music=null;render()});
    else{
      const empty=document.createElement("span");
      empty.className="quick-reel-empty";empty.textContent="Трек ещё не выбран";
      audio.append(empty);
    }
    E("quick-reel-duration").textContent=music?fmt(music.duration):"—";
    E("quick-reel-music-section").classList.toggle("is-ready",Boolean(music));
    E("quick-reel-submit").disabled=busy||running||serverBusy||!files.size||!music;
    E("quick-reel-cancel").hidden=!running;
    E("quick-reel-cancel").disabled=!running||cancelPending;
    for(const id of ["quick-reel-dropzone","quick-reel-music-dropzone"])
      E(id).disabled=busy||running;
  }
  function setBusy(value){busy=value;render()}
  const VIDEO_TYPES=new Set(["mp4","mov","mkv","avi","webm","m4v","mts"]);
  const AUDIO_TYPES=new Set(["mp3","m4a","wav","flac","aac","ogg","opus"]);
  async function acceptFiles(input,kind){
    if(busy||running)return;
    const selected=Array.from(input||[]);
    if(!selected.length)return;
    const video=kind==="video", allowed=video?VIDEO_TYPES:AUDIO_TYPES;
    if(selected.some(item=>!allowed.has((item.name.split(".").pop()||"").toLowerCase()))){
      status("Выбраны файлы неподдерживаемого формата.",true);return;
    }
    if(video&&selected.length+files.size>30){
      status("Максимум 30 видеозаписей.",true);return;
    }
    if(!video&&selected.length!==1){
      status("Выберите один музыкальный трек.",true);return;
    }
    setBusy(true);
    try{
      status("Ищем оригиналы выбранных файлов на диске. Видео не загружаются…");
      const response=await api("/api/quick-reel/resolve-files",{
        kind,files:selected.map(f=>({
          name:f.name,size:f.size,last_modified:f.lastModified
        }))
      });
      if(video){
        for(const item of response.videos||[])
          files.set(item.id,{name:item.name});
        status("Выбрано видео: "+selected.length+
          ". Используются оригиналы без копирования.");
      }else{
        music={path:response.audio.path,name:response.audio.name,
               duration:response.audio.duration};
        status("Музыка выбрана. Продолжительность: "+fmt(music.duration)+".");
      }
    }catch(error){status(error.message,true)}
    finally{setBusy(false)}
  }
  async function chooseFiles(kind){
    if(busy||running)return;
    // Secure localhost browsers support the native desktop-first picker.
    if(typeof window.showOpenFilePicker==="function"){
      try{
        const handles=await window.showOpenFilePicker({
          multiple:kind==="video",startIn:"desktop"
        });
        const selected=await Promise.all(handles.map(item=>item.getFile()));
        return acceptFiles(selected,kind);
      }catch(error){
        if(error?.name==="AbortError")return;
        // Browsers without this picker fall back to the standard file input.
        if(error?.name!=="TypeError"&&error?.name!=="NotSupportedError"&&
            error?.name!=="SecurityError"){
          status("Не удалось открыть Проводник: "+error.message,true);
          return;
        }
      }
    }
    E(kind==="video"?"quick-reel-video-input":"quick-reel-audio-input").click();
  }
  async function acceptDrop(event,kind){
    event.preventDefault();
    const zone=E(kind==="video"?"quick-reel-dropzone":"quick-reel-music-dropzone");
    zone.classList.remove("drag-over");
    if(event.dataTransfer?.files?.length){
      return acceptFiles(event.dataTransfer.files,kind);
    }
    status("Перетащите файл с диска компьютера.",true);
  }
  function header(update,results){
    if(!reelId)return;
    E("quick-reel-activity").hidden=dismissedReelId===reelId;
    const progress=Math.max(0,Math.min(100,Number(update.progress)||0));
    const state=String(update.status||"queued");
    const labels={running:"Выполняется",queued:"В очереди",complete:"Готово",
      failed:"Ошибка",cancelled:"Остановлено"};
    E("quick-reel-header-state").textContent=labels[state]||state;
    const cancellable=state==="running"||state==="queued";
    E("quick-reel-header-cancel").hidden=!cancellable;
    E("quick-reel-header-cancel").disabled=cancelPending;
    E("quick-reel-header-stage").textContent=
      update.error||update.stage||"Ожидание подготовки";
    E("quick-reel-header-meter").value=state==="complete"?100:progress;
    const list=E("quick-reel-header-videos");
    list.replaceChildren();
    const videos=update.videos||update.source_names?.map(v=>({video:v}))||[];
    const names={ready:"Подготовлено",preparing:"Подготовка",queued:"В очереди",
      missing:"Ожидает",failed:"Ошибка",analyzing:"Анализируется",
      analyzed:"Проанализировано"};
    for(const file of videos){
      const row=document.createElement("span");
      row.className="quick-reel-activity-video";
      const label=names[file.state]||"Анализ / монтаж";
      row.textContent=file.video+" · "+label+
        (file.state==="preparing"?" "+Math.round(Number(file.progress)||0)+"%":"");
      row.title=file.error||file.video;
      list.append(row);
    }
    const ready=Boolean(results?.available||update.available);
    const download=E("quick-reel-header-download");
    download.hidden=!ready||!results?.download_url;
    if(!download.hidden)download.href=results.download_url;
    else download.removeAttribute("href");
    // The sidebar action always starts a new quick reel. Readiness is merely
    // a dismissible notification in the header, not a persistent button state.
    E("quick-reel-open").textContent="✦ Рилс";
  }
  function progress(percent,stage){
    E("quick-reel-progress").hidden=false;
    E("quick-reel-meter").value=Math.max(0,Math.min(100,Number(percent)||0));
    E("quick-reel-percent").textContent=Math.round(Number(percent)||0)+"%";
    E("quick-reel-stage").textContent=stage||"Обработка";
  }
  async function revealResult(result){
    if(!result.available)return;
    progress(100,"Обработка завершена");
    status("Рилс готов. Ссылка на MP4 добавлена в список внизу.");
    await refreshHistory();
  }
  async function poll(){
    if(!reelId||loadingStatus)return;
    loadingStatus=true;
    try{
      const response=await api("/api/quick-reel/status?id="+
        encodeURIComponent(reelId));
      const state=String(response.status||"queued");
      running=state==="running"||state==="queued";
      let result=null;
      if(response.available){
        result=await api("/api/quick-reel/result?id="+
          encodeURIComponent(reelId));
      }
      header(response,result);
      if(running){
        progress(response.progress,response.stage);
        status(response.stage||"Обработка…");
      }else if(result?.available){
        await revealResult(result);
      }else if(state==="failed"||state==="cancelled"){
        status(response.error||response.stage||"Обработка прервана",true);
      }
      render();
      if(!running)stopPoll();
    }catch(error){
      status("Статус быстрого рилса: "+error.message,true);
      stopPoll();
    }finally{loadingStatus=false}
  }
  function stopPoll(){
    if(pollTimer!==null){clearInterval(pollTimer);pollTimer=null}
  }
  function startPoll(){
    stopPoll();
    poll().catch(console.warn);
    pollTimer=setInterval(()=>poll().catch(console.warn),1450);
  }
  async function launch(){
    if(busy||running||serverBusy||!files.size||!music)return;
    setBusy(true);
    try{
      await checkBackend();
      status("Регистрируем задачу обработки…");
      const job=await api("/api/quick-reel/start",{
        videos:[...files.keys()],audio_path:music.path
      });
      reelId=job.reel_id;
      dismissedReelId="";
      try{localStorage.removeItem(dismissKey)}catch{}
      running=true;
      header({status:"queued",progress:0,stage:"Проверяем исходные файлы",
        videos:[...files.values()].map(item=>({video:item.name,state:"queued"}))});
      progress(0,"Проверка общей подготовки видео");
      E("quick-reel-dialog").close();
      startPoll();
    }catch(error){status(error.message,true)}
    finally{setBusy(false)}
  }
  async function cancelReel(){
    if(!running||!reelId||cancelPending)return;
    cancelPending=true;render();
    E("quick-reel-header-cancel").disabled=true;
    try{
      await api("/api/quick-reel/cancel",{id:reelId});
      status("Останавливаем обработку…");
    }catch(error){status(error.message,true)}
    finally{
      cancelPending=false;
      await poll();
    }
  }
  function showDialog(){
    const dialog=E("quick-reel-dialog");
    if(!running){
      // A fresh creation form must not display the previous finished task.
      E("quick-reel-progress").hidden=true;
      status("");
      files.clear();
      music=null;
      render();
    }
    if(!dialog.open)dialog.showModal();
    checkBackend().catch(error=>status(error.message,true));
    if(running)poll();
    // The single completed archive is the sole source of the history list.
    refreshHistory().catch(console.warn);
  }
  function init(){
    E("quick-reel-open").addEventListener("click",showDialog);
    E("quick-reel-header-open").addEventListener("click",showDialog);
    E("quick-reel-header-close").addEventListener("click",()=>{
      dismissedReelId=reelId;
      try{localStorage.setItem(dismissKey,reelId)}catch{}
      E("quick-reel-activity").hidden=true;
    });
    E("quick-reel-close").addEventListener("click",()=>E("quick-reel-dialog").close());
    for(const [id,kind,inputId] of [
      ["quick-reel-dropzone","video","quick-reel-video-input"],
      ["quick-reel-music-dropzone","audio","quick-reel-audio-input"]
    ]){
      const zone=E(id),input=E(inputId);
      zone.addEventListener("click",()=>chooseFiles(kind));
      zone.addEventListener("dragenter",e=>{
        e.preventDefault();zone.classList.add("drag-over");
      });
      zone.addEventListener("dragover",e=>{
        e.preventDefault();e.dataTransfer.dropEffect="copy";
        zone.classList.add("drag-over");
      });
      zone.addEventListener("dragleave",e=>{
        if(!zone.contains(e.relatedTarget))zone.classList.remove("drag-over");
      });
      zone.addEventListener("drop",e=>acceptDrop(e,kind));
      input.addEventListener("change",e=>{
        acceptFiles(e.target.files,kind);
        e.target.value="";
      });
    }
    E("quick-reel-submit").addEventListener("click",launch);
    E("quick-reel-cancel").addEventListener("click",cancelReel);
    E("quick-reel-header-cancel").addEventListener("click",cancelReel);
    render();
    E("quick-reel-activity").hidden=true;
    // Recover progress ONLY for an actually running task, not the last
    // completed result. A finished reel belongs in history, never the header.
    (async()=>{
      let activeId="";
      try{activeId=(await api("/api/quick-reel/latest")).reel_id||""}
      catch(error){console.warn("Quick Reel latest:",error)}
      if(reelId||!activeId)return; // Another creation was launched meanwhile.
      try{
        const [current,job]=await Promise.all([
          api("/api/quick-reel/status?id="+encodeURIComponent(activeId)),
          api("/api/job")
        ]);
        if(reelId)return;
        if(["queued","running"].includes(current.status)&&
           job.kind==="quick_reel"&&job.reel_id===activeId&&
           job.status==="running"){
          reelId=activeId;
          running=true;
          startPoll();
        }
      }catch(error){console.warn("Quick Reel recovery:",error)}
    })().catch(error=>console.warn(error));
  }
  window.VolleyballQuickReelUI={setTask:info=>{
    const next=info?.status==="running";
    if(next===serverBusy)return;
    serverBusy=next;render();
  }};
  if(document.readyState==="loading")
    document.addEventListener("DOMContentLoaded",init,{once:true});
  else init();
})();
