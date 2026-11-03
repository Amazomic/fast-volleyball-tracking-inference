"use strict";
/* Quick Reel: path-only local sources, shared caches, persistent header progress. */
(() => {
  const E=id=>document.getElementById(id);
  const files=new Map();
  let music=null, busy=false, running=false, reelId="";
  let pollTimer=null, loadingStatus=false;

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
    if(!state.ok||state.reload_required||Number(state.api_version)<12)
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
    E("quick-reel-submit").disabled=busy||running||!files.size||!music;
    E("quick-reel-cancel").hidden=!running;
    E("quick-reel-cancel").disabled=!running;
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
    E("quick-reel-activity").hidden=false;
    const progress=Math.max(0,Math.min(100,Number(update.progress)||0));
    const state=String(update.status||"queued");
    const labels={running:"Выполняется",queued:"В очереди",complete:"Готово",
      failed:"Ошибка",cancelled:"Остановлено"};
    E("quick-reel-header-state").textContent=labels[state]||state;
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
    E("quick-reel-header-result").hidden=!ready;
    if(ready&&results?.download_url)
      E("quick-reel-header-download").href=results.download_url;
    E("quick-reel-open").textContent=ready?"✦ Рилс готов":
      state==="running"?"✦ Быстрый рилс · "+Math.round(progress)+"%":
      "✦ Быстрый рилс";
  }
  function progress(percent,stage){
    E("quick-reel-progress").hidden=false;
    E("quick-reel-meter").value=Math.max(0,Math.min(100,Number(percent)||0));
    E("quick-reel-percent").textContent=Math.round(Number(percent)||0)+"%";
    E("quick-reel-stage").textContent=stage||"Обработка";
  }
  async function revealResult(result){
    if(!result.available)return;
    E("quick-reel-result").hidden=false;
    const video=E("quick-reel-video");
    if(E("quick-reel-dialog").open&&video.getAttribute("src")!==result.url)
      video.src=result.url;
    E("quick-reel-download").href=result.download_url;
    E("quick-reel-result-note").textContent=
      "Видео: "+fmt(result.duration)+" · музыка: "+fmt(result.music_duration)+
      (result.duration+0.4<result.music_duration?
        ". Недостаточно полных розыгрышей для длины трека.":"");
    progress(100,"Рилс готов");
    status("Готово. Результат доступен также в верхнем блоке страницы.");
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
    if(busy||running||!files.size||!music)return;
    setBusy(true);
    try{
      await checkBackend();
      status("Регистрируем задачу обработки…");
      E("quick-reel-result").hidden=true;
      const job=await api("/api/quick-reel/start",{
        videos:[...files.keys()],audio_path:music.path
      });
      reelId=job.reel_id;
      localStorage.setItem("volley-quick-reel-id",reelId);
      running=true;
      header({status:"queued",progress:0,stage:"Проверяем исходные файлы",
        videos:[...files.values()].map(item=>({video:item.name,state:"queued"}))});
      progress(0,"Проверка общей подготовки видео");
      E("quick-reel-dialog").close();
      startPoll();
    }catch(error){status(error.message,true)}
    finally{setBusy(false)}
  }
  function showDialog(){
    const dialog=E("quick-reel-dialog");
    if(!dialog.open)dialog.showModal();
    checkBackend().catch(error=>status(error.message,true));
    if(reelId)poll();
  }
  function init(){
    E("quick-reel-open").addEventListener("click",showDialog);
    E("quick-reel-header-open").addEventListener("click",showDialog);
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
    E("quick-reel-cancel").addEventListener("click",async()=>{
      if(!running)return;
      try{await api("/api/cancel",{});status("Останавливаем обработку…")}
      catch(error){status(error.message,true)}
      await poll();
    });
    try{reelId=localStorage.getItem("volley-quick-reel-id")||""}catch{}
    render();
    if(reelId)startPoll();
  }
  if(document.readyState==="loading")
    document.addEventListener("DOMContentLoaded",init,{once:true});
  else init();
})();
