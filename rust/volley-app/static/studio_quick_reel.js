"use strict";
// Independent Quick Reel. Only the server chooses original paths and source IDs.
(() => {
  const el=id=>document.getElementById(id);
  const ui={
    drop:el("studio-quick-drop"),browser:el("studio-quick-browser"),
    files:el("studio-quick-files"),sources:el("studio-quick-sources"),
    count:el("studio-quick-selection-count"),clear:el("studio-quick-clear"),
    music:el("studio-quick-music"),start:el("studio-quick-start"),
    cancel:el("studio-quick-cancel"),refresh:el("studio-quick-refresh"),
    progress:el("studio-quick-progress"),percent:el("studio-quick-percent"),
    status:el("studio-quick-status"),stages:el("studio-quick-stages"),
    videos:el("studio-quick-videos"),download:el("studio-quick-download"),
    error:el("studio-quick-error"),sidebar:el("studio-quick-summary")
  };
  const s={sources:[],jobId:"",active:false,busy:false,polling:false,
    timer:null,cancelRequested:false};
  const valid=id=>typeof id==="string"&&/^[a-f0-9]{32}$/.test(id);
  const make=(tag,text)=>{const n=document.createElement(tag);n.textContent=String(text);return n;};
  const api=async(url,options={})=>{
    const response=await fetch(url,{cache:"no-store",credentials:"same-origin",...options});
    const data=await response.json().catch(()=>({}));
    if(!response.ok)throw Error(typeof data.error==="string"?data.error:"HTTP "+response.status);
    return data;
  };
  const post=(url,data)=>api(url,{method:"POST",headers:{"Content-Type":"application/json"},
    body:JSON.stringify(data)});
  const msg=error=>error instanceof Error?error.message:String(error);
  const note=(text,error=false)=>{
    ui.status.textContent=text;ui.error.textContent=error?text:"";
    ui.error.classList.toggle("hidden",!error);
  };
  const stop=()=>{if(s.timer!==null)clearInterval(s.timer);s.timer=null;};
  const controls=()=>{
    const blocked=s.busy||s.active;
    ui.start.disabled=blocked||!s.sources.length||!ui.music.files?.length;
    ui.clear.disabled=blocked;ui.browser.disabled=blocked;
    ui.files.disabled=blocked;ui.music.disabled=blocked;
    ui.drop.setAttribute("aria-disabled",blocked?"true":"false");
    ui.cancel.classList.toggle("hidden",!s.active);
    ui.cancel.disabled=s.cancelRequested;
    ui.refresh.disabled=s.polling||s.busy;
  };
  const renderSources=()=>{
    ui.sources.replaceChildren();
    ui.count.textContent=s.sources.length+" / 30";
    if(!s.sources.length)ui.sources.append(make("li","Видео ещё не выбраны"));
    for(const item of s.sources){
      ui.sources.append(make("li",item.name+" · "+
        Number(item.size_gb||0).toFixed(2)+" ГБ · "+
        (item.copied?"временная копия":"ссылка на оригинал")));
    }
    controls();
  };
  const addSources=items=>{
    if(!Array.isArray(items)||items.some(row=>!valid(row.id)||
      typeof row.name!=="string"||typeof row.size_gb!=="number"))
      throw Error("Некорректные данные выбранных видео");
    if(s.sources.length+items.length>30)throw Error("Максимум 30 видео");
    const ids=new Set(s.sources.map(row=>row.id));
    for(const row of items){if(ids.has(row.id))throw Error("Повторный ID видео");ids.add(row.id);}
    s.sources.push(...items);renderSources();
    if(items.length)note("Выбрано "+s.sources.length+" видео. Добавьте музыку и запустите рилс.");
  };
  // Always open the file chooser in the user's BROWSER, synchronously inside
  // a click/keyboard gesture. A server-side PowerShell dialog is not portable:
  // Linux/WSL services may have no interactive desktop or user session.
  // Browsers never reveal local absolute paths, so this flow streams an
  // isolated temporary copy (see native/upload); it cannot link by path.
  const openFilePicker=()=>{
    if(s.busy||s.active||ui.files.disabled)return;
    // Reset BEFORE showing the dialog: reselecting the same file must fire
    // change again after a cancelled or failed upload.
    ui.files.value="";
    try{
      if(typeof ui.files.showPicker==="function")ui.files.showPicker();
      else ui.files.click();
    }catch(error){
      // File input click is supported in browsers without showPicker().
      try{ui.files.click();}
      catch(fallbackError){note("Не удалось открыть выбор файла: "+msg(fallbackError),true);}
    }
  };
  const browserFiles=async(list)=>{
    if(s.busy||s.active)return;
    const files=[...list].filter(Boolean);
    if(!files.length)return;
    if(s.sources.length+files.length>30){note("Максимум 30 видео.",true);return;}
    if(files.some(file=>file.size<=0||file.size>2*1024**3)){
      note("Файл больше 2 ГиБ. Выбор на Windows/Linux работает, но загрузка больших файлов пока не поддерживается.",true);
      return;
    }
    s.busy=true;controls();
    try{
      for(let index=0;index<files.length;index++){
        const file=files[index];
        note("Копирование видео "+(index+1)+"/"+files.length+": "+file.name);
        const form=new FormData();form.append("file",file,file.name);
        const result=await api("/api/quick-reel/native/upload",{method:"POST",body:form});
        addSources(result.videos);
      }
      note("Видео добавлены в изолированное хранилище. Исходники не изменены.");
    }catch(e){note("Загрузка видео: "+msg(e),true);}
    finally{s.busy=false;ui.files.value="";controls();}
  };
  const uploadMusic=async()=>{
    const file=ui.music.files?.[0];
    if(!file)throw Error("Для Quick Reel нужна музыка");
    if(!file.size||file.size>90*1024*1024)throw Error("Музыка: от 1 байта до 90 МиБ");
    const form=new FormData();form.append("file",file,file.name);
    const result=await api("/api/montage/native/audio",{method:"POST",body:form});
    if(!/^track_[a-f0-9]{32}\.(mp3|m4a|wav|flac|aac|ogg|opus)$/.test(result.audio_file||""))
      throw Error("Некорректное подтверждение загруженной музыки");
    return result.audio_file;
  };
  const names=["Подготовка","GPU","CPU","Монтаж"];
  const renderStatus=result=>{
    const progress=Math.max(0,Math.min(100,Number(result.progress)||0));
    const phase=Number(result.stage_index)||0;
    ui.progress.value=progress;ui.percent.textContent=progress.toFixed(1)+"%";
    [...ui.stages.children].forEach((node,i)=>{
      node.classList.toggle("current",result.status==="running"&&phase===i+1);
      node.classList.toggle("finished",result.status==="complete"||phase>i+1);
      node.textContent=(i+1)+". "+names[i];
    });
    ui.videos.replaceChildren();
    for(const item of Array.isArray(result.videos)?result.videos:[]){
      const fraction=Number(item.gpu_percent??item.progress);
      const amount=Number.isFinite(fraction)?" · "+Math.max(0,Math.min(100,fraction)).toFixed(0)+"%":"";
      ui.videos.append(make("li",String(item.video||"Видео")+": "+
        String(item.model||item.state||"ожидание")+amount));
    }
    const status=String(result.status||"idle");
    ui.sidebar.textContent="Быстрый рилс: "+(status==="complete"?"готово":
      status==="running"?(progress.toFixed(0)+"% · "+names[Math.max(0,Math.min(3,phase-1))]):
      status==="queued"?"в очереди":status==="failed"?"ошибка":
      status==="cancelled"?"отменён":status==="interrupted"?"прерван":"ожидание");
    const stage=String(result.stage||"Ожидание запуска");
    ui.download.classList.add("hidden");
    if(status==="complete"){
      if(valid(result.id)&&result.output?.file==="quick_reel.mp4"&&
        result.output?.native_original_clips_verified===true&&
        result.output?.mp4_quality?.has_audio===true){
        ui.download.href="/api/quick-reel/native/media?"+
          new URLSearchParams({id:result.id}).toString();
        ui.download.classList.remove("hidden");
        note("Quick Reel готов. Скачайте проверенный MP4.");
      }else note("Готовый MP4 не прошёл проверку качества.",true);
    }else if(status==="failed"||status==="interrupted")
      note(stage+". "+String(result.error||"Обработка не завершена"),true);
    else if(status==="cancelled")note("Quick Reel отменён. MP4 не опубликован.");
    else note(status==="idle"?"Выберите видео и музыку.":stage+
      (s.cancelRequested?" · отмена запрошена":""));
    s.jobId=valid(result.id)?result.id:"";
    s.active=status==="queued"||status==="running";
    if(!s.active)s.cancelRequested=false;
    controls();
    if(s.active&&s.timer===null)s.timer=setInterval(()=>
      poll().catch(e=>note("Ошибка обновления: "+msg(e),true)),1600);
    if(!s.active)stop();
  };
  const poll=async()=>{
    if(s.polling)return;
    s.polling=true;controls();
    try{
      const path="/api/quick-reel/native/status"+
        (s.jobId?"?"+new URLSearchParams({id:s.jobId}):"");
      const result=await api(path);
      if(s.jobId&&result.id!==s.jobId)throw Error("Статус другого рилса");
      renderStatus(result);
    }finally{s.polling=false;controls();}
  };
  const start=async()=>{
    if(s.busy||s.active||!s.sources.length)return;
    s.busy=true;controls();note("Загрузка музыки и запуск свежего GPU/CPU анализа…");
    try{
      const audio_file=await uploadMusic();
      const result=await post("/api/quick-reel/native/start",{
        selection_ids:s.sources.map(row=>row.id),audio_file
      });
      if(!valid(result.id)||result.status!=="queued")
        throw Error("Сервер не подтвердил запуск");
      s.jobId=result.id;s.cancelRequested=false;
      renderStatus({id:result.id,status:"queued",stage_index:0,
        stage:"Ожидание подготовки",progress:0});
      await poll();
    }catch(e){note("Запуск Quick Reel: "+msg(e),true);}
    finally{s.busy=false;controls();}
  };
  const cancel=async()=>{
    if(!s.active||!valid(s.jobId)||s.cancelRequested)return;
    s.cancelRequested=true;controls();
    try{
      const result=await post("/api/quick-reel/native/cancel",{id:s.jobId});
      if(result.id!==s.jobId||result.cancel_requested!==true)
        throw Error("Сервер не подтвердил отмену");
      note("Отмена запрошена. Останавливается активный GPU/FFmpeg-процесс.");
      await poll();
    }catch(e){s.cancelRequested=false;note("Отмена: "+msg(e),true);}
    finally{controls();}
  };
  ui.drop.addEventListener("click",openFilePicker);
  ui.drop.addEventListener("keydown",e=>{
    if(e.key==="Enter"||e.key===" "){e.preventDefault();openFilePicker();}
  });
  ui.drop.addEventListener("dragover",e=>{
    e.preventDefault();if(!s.active&&!s.busy)ui.drop.classList.add("drag-over");
  });
  ui.drop.addEventListener("dragleave",()=>ui.drop.classList.remove("drag-over"));
  ui.drop.addEventListener("drop",e=>{
    e.preventDefault();ui.drop.classList.remove("drag-over");
    browserFiles(e.dataTransfer?.files||[]).catch(error=>note(msg(error),true));
  });
  ui.browser.addEventListener("click",openFilePicker);
  ui.files.addEventListener("change",()=>browserFiles(ui.files.files||[]));
  ui.clear.addEventListener("click",async()=>{
    if(s.busy||s.active||!s.sources.length)return;
    s.busy=true;controls();
    try{
      const tokens=s.sources.map(row=>row.id);
      const result=await post("/api/quick-reel/native/release",{selection_ids:tokens});
      if(result.released!==tokens.length)throw Error("Не все временные файлы освобождены");
      s.sources=[];renderSources();
      note("Выбор очищен. Исходные видео не изменены.");
    }catch(error){note("Очистка выбора: "+msg(error),true);}
    finally{s.busy=false;controls();}
  });
  ui.music.addEventListener("change",controls);
  ui.start.addEventListener("click",start);
  ui.cancel.addEventListener("click",cancel);
  ui.refresh.addEventListener("click",()=>poll().catch(e=>
    note("Состояние: "+msg(e),true)));
  renderSources();
  poll().catch(e=>note("Quick Reel недоступен: "+msg(e),true));
})();