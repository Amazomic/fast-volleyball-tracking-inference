"use strict";
// Transitional same-origin browser bridge. Rust validates all source IDs,
// contact evidence, gold revisions, original clip ranges and montage jobs.
(() => {
  const $ = id => document.getElementById(id);
  const ui = {
    project:$("studio-project"),analysis:$("studio-inference"),run:$("studio-run"),refresh:$("studio-refresh"),
    filter:$("studio-filter"),previous:$("studio-prev"),next:$("studio-next"),
    list:$("studio-rallies"),total:$("studio-total"),goldCount:$("studio-gold-count"),
    selectedCount:$("studio-selected-count"),banner:$("studio-banner"),
    caption:$("studio-page-caption"),video:$("studio-video"),
    emptyVideo:$("studio-video-empty"),videoTime:$("studio-video-time"),
    viewTitle:$("studio-view-title"),viewDescription:$("studio-view-description"),
    events:$("studio-event-summary"),selectionText:$("studio-selection-text"),
    picked:$("studio-picked"),encoder:$("studio-encoder"),music:$("studio-music"),
    layout:$("studio-layout"),volume:$("studio-volume"),
    render:$("studio-render"),job:$("studio-job"),cancel:$("studio-cancel"),
    download:$("studio-download")
  };
  const s = {
    project:"",analysis:"",run:"",video:"",items:[],total:0,offset:0,limit:24,
    projectCatalog:[],projectVideos:[],chain:null,chainEpoch:0,catalogEpoch:0,
    filtered:0,gold:new Set(),revision:"absent",writable:false,selected:new Map(),
    current:"",videoKey:"",stopAt:null,generation:0,busy:false,jobId:"",timer:null
  };
  const url=(path,params)=>path+(params?"?"+new URLSearchParams(params).toString():"");
  const request=async(path,opts={})=>{
    const res=await window.VolleyballStudioActivity.fetch(path,{cache:"no-store",credentials:"same-origin",...opts});
    let data={};try{data=await res.json();}catch{}
    if(!res.ok)throw Error(typeof data.message==="string"?data.message:
      typeof data.error==="string"?data.error:"Ошибка HTTP "+res.status);
    return data;
  };
  const post=(path,payload)=>request(path,{method:"POST",
    headers:{"Content-Type":"application/json"},body:JSON.stringify(payload)});
  const make=(tag,klass,text)=>{
    const node=document.createElement(tag);
    if(klass)node.className=klass;
    if(text!==undefined)node.textContent=String(text);
    return node;
  };
  const wipe=node=>{while(node.firstChild)node.firstChild.remove();};
  const message=(text,mood)=>{
    ui.banner.className="banner"+(mood?" "+mood:"");
    ui.banner.textContent=String(text);
  };
  const failed=e=>message(e instanceof Error?e.message:String(e),"error");
  const clock=frames=>{
    const sec=Math.max(0,Math.floor((Number(frames)||0)/30));
    return String(Math.floor(sec/60)).padStart(2,"0")+":"+String(sec%60).padStart(2,"0");
  };
  const length=item=>(Math.max(0,
    (Number(item.detected_end_frame)-Number(item.detected_start_frame)+1)/30)).toFixed(1)+" с";
  const valid=id=>typeof id==="string"&&/^[a-f0-9]{24}$/.test(id);
  // Manual selection is stored and ordered inside real Rust/WebAssembly.
  // JavaScript stores only the human-readable labels for selected IDs.
  let wasmReady=false;
  class RustSelection {
    constructor(exports){
      this.exports=exports;
      this.labels=new Map();
      this.encoder=new TextEncoder();
      this.decoder=new TextDecoder();
      if(exports.volley_web_api_version()!==1||
        !exports.memory ||exports.volley_selection_limit()!==80){
        throw Error("Неверная версия Rust/WASM модуля выбора");
      }
      exports.volley_selection_clear();
    }
    use(id,fn){
      if(!valid(id))return 0;
      const ptr=this.exports.volley_id_buffer_ptr();
      const destination=new Uint8Array(this.exports.memory.buffer,ptr,24);
      destination.set(this.encoder.encode(id));
      return fn();
    }
    get size(){return this.exports.volley_selection_count();}
    has(id){return this.use(id,()=>this.exports.volley_selection_contains())===1;}
    set(id,label){
      const rc=this.use(id,()=>this.exports.volley_selection_add());
      if(rc!==1)throw Error(rc===-1?
        "Максимум 80 розыгрышей для одного монтажа":"Неверный ID розыгрыша");
      this.labels.set(id,label);return this;
    }
    delete(id){
      const found=this.has(id);
      this.use(id,()=>this.exports.volley_selection_remove());
      this.labels.delete(id);return found;
    }
    clear(){this.exports.volley_selection_clear();this.labels.clear();}
    keys(){
      const ids=[];
      for(let i=0;i<this.size;i++){
        if(this.exports.volley_selection_at(i)!==1)throw Error("Rust/WASM: ошибка чтения выбора");
        const ptr=this.exports.volley_id_buffer_ptr();
        const text=this.decoder.decode(new Uint8Array(this.exports.memory.buffer,ptr,24));
        ids.push(text);
      }
      return ids.values();
    }
    values(){return Array.from(this.keys(),id=>this.labels.get(id)).values();}
  }
  const loadRustSelection=async()=>{
    const response=await window.VolleyballStudioActivity.fetch("/studio/rust/selection.wasm",{
      credentials:"same-origin",cache:"no-store"
    });
    if(!response.ok)throw Error("Модуль Rust/WASM недоступен: HTTP "+response.status);
    const bytes=await response.arrayBuffer();
    const module=await WebAssembly.instantiate(bytes,{});
    s.selected=new RustSelection(module.instance.exports);
    wasmReady=true;renderSelection();cards();
  };
  const clearVideo=()=>{
    ui.video.pause();ui.video.removeAttribute("src");ui.video.load();
    s.videoKey="";s.current="";s.stopAt=null;
    ui.emptyVideo.classList.remove("hidden");
    ui.viewTitle.textContent="Розыгрыш не выбран";
    ui.viewDescription.textContent="Откройте розыгрыш для просмотра.";
    ui.videoTime.textContent="—";wipe(ui.events);
  };
  const stopPoll=()=>{if(s.timer)clearInterval(s.timer);s.timer=null;};
  const renderSelection=()=>{
    wipe(ui.picked);
    ui.selectedCount.textContent=String(s.selected.size);
    for(const item of s.selected.values()) {
      ui.picked.append(make("span","picked",item.time+" · "+item.duration));
    }
    ui.selectionText.textContent=s.selected.size?
      "Выбрано "+s.selected.size+" розыгрышей. Монтаж из оригинального видео в порядке выбора.":
      "Выберите один или несколько розыгрышей. Классификация атак и подач пока недоступна.";
    ui.render.disabled=!wasmReady||s.busy||!s.run||s.selected.size===0;
  };
  const reset=()=>{
    s.generation++;s.run="";s.video="";s.items=[];s.total=0;s.filtered=0;s.offset=0;
    s.gold=new Set();s.revision="absent";s.writable=false;s.selected.clear();
    s.jobId="";stopPoll();clearVideo();wipe(ui.list);
    ui.total.textContent="—";ui.goldCount.textContent="—";ui.caption.textContent="";
    ui.job.textContent="";ui.cancel.classList.add("hidden");
    ui.download.classList.add("hidden");renderSelection();
  };
  const options=(select,rows,value,label,placeholder)=>{
    wipe(select);
    const blank=make("option","",placeholder);blank.value="";select.append(blank);
    for(const item of rows){
      const entry=make("option","",label(item));entry.value=String(item[value]);
      select.append(entry);
    }
  };
  // The linked source chain is project -> GPU analysis (video + lineage) -> CPU snapshot.
  // Preserve selections on refresh; reset only dependent selections when an upstream
  // source changes. Stale async responses must never reselect a different project.
  const setRuns=async(preferred="")=>{
    reset();
    const rows=s.chain?.byAnalysis.get(s.analysis)||[];
    options(ui.run,rows,"rally_run_id",row=>
      (window.VolleyStudioChain.stamp(row.created_at)||"CPU "+row.rally_run_id.slice(0,8))+
      " · "+(row.candidate_count||0)+" розыгрышей",
      !s.analysis?"Сначала выберите анализ":
        rows.length?"Выберите постобработку":"Нет CPU-версий для анализа");
    ui.run.disabled=!s.analysis||!rows.length;
    ui.run.value=rows.some(row=>row.rally_run_id===preferred)?preferred:
      rows.length===1?rows[0].rally_run_id:"";
    s.run=ui.run.value;
    if(s.run)await loadPage();
    else message(!s.analysis?"Выберите первичный GPU-анализ.":
      rows.length?"Выберите версию постобработки.":
      "У выбранного анализа пока нет связанной CPU-постобработки.");
  };
  const setAnalysis=async(preferredRun="")=>{
    s.analysis=ui.analysis.value;
    const source=s.chain?.choices.find(row=>row.value===s.analysis);
    window.dispatchEvent(new CustomEvent("volley:chain-change",{
      detail:{project_id:s.project,video:source?.video||"",analysis:s.analysis}
    }));
    await setRuns(preferredRun);
  };
  const loadChain=async(preferredAnalysis="",preferredRun="")=>{
    const token=++s.chainEpoch,project=s.project;
    reset();
    s.chain=null;
    options(ui.analysis,[],"value",x=>x,"Загрузка анализов…");
    options(ui.run,[],"rally_run_id",x=>x,"Сначала выберите анализ");
    ui.analysis.disabled=true;ui.run.disabled=true;
    if(!project){
      options(ui.analysis,[],"value",x=>x,"Сначала выберите проект");
      message("Выберите проект для просмотра цепочки обработок.");
      return;
    }
    message("Поиск сохранённых GPU-анализов и CPU-версий…");
    const videos=s.projectVideos.slice(0,40);
    const runs=await request(url("/api/cpu/native/rallies/runs",{project_id:project}));
    const overviews=new Map();
    let unavailable=0;
    // Limit metadata requests for multi-video projects to four in flight.
    for(let offset=0;offset<videos.length;offset+=4){
      const batch=await Promise.all(videos.slice(offset,offset+4).map(async video=>{
        try{
          const value=await request(url("/api/studio/rust/analysis-overview",{
            project_id:project,video
          }));
          if(value.schema!=="volley-native-analysis-overview/1"||
              value.project_id!==project||value.video!==video||value.metadata_only!==true)
            throw Error("Несовместимая версия индекса анализа");
          return [video,value];
        }catch{return [video,null];}
      }));
      if(token!==s.chainEpoch||s.project!==project)return;
      for(const [video,value] of batch){
        if(value)overviews.set(video,value);else unavailable++;
      }
    }
    if(token!==s.chainEpoch||s.project!==project)return;
    const builder=window.VolleyStudioChain;
    if(!builder?.build)throw Error("Связанная цепочка Rust Studio недоступна");
    s.chain=builder.build(videos,overviews,runs.runs);
    options(ui.analysis,s.chain.choices,"value",item=>item.label,
      s.chain.choices.length?"Выберите первичный анализ":"Нет GPU-анализов");
    ui.analysis.disabled=!s.chain.choices.length;
    ui.analysis.value=s.chain.choices.some(row=>row.value===preferredAnalysis)?
      preferredAnalysis:s.chain.choices.length===1?s.chain.choices[0].value:"";
    await setAnalysis(preferredRun);
    if(unavailable)message("Не удалось прочитать стадии "+unavailable+
      " видео. Для части CPU-версий происхождение GPU не подтверждено.","error");
  };
  const projects=async()=>{
    const prev=s.project,selectedAnalysis=s.analysis,selectedRun=s.run;
    const epoch=++s.catalogEpoch;
    const raw=await request("/api/projects");
    if(epoch!==s.catalogEpoch)return;
    const rows=(Array.isArray(raw.projects)?raw.projects:[]).
      filter(row=>typeof row.id==="string");
    s.projectCatalog=rows;
    options(ui.project,rows,"id",row=>row.name||row.title||row.id,"Выберите проект");
    ui.project.value=rows.some(row=>row.id===prev)?prev:
      rows.length===1?rows[0].id:"";
    s.project=ui.project.value;
    const current=rows.find(row=>row.id===s.project);
    s.projectVideos=Array.isArray(current?.videos)?
      current.videos.filter(video=>typeof video==="string"):[];
    window.dispatchEvent(new Event("volley:project-ready"));
    await loadChain(s.project===prev?selectedAnalysis:"",
      s.project===prev?selectedRun:"");
  };
  const loadMarks=async(token)=>{
    const raw=await request(url("/api/cpu/native/rallies/marks",{
      project_id:s.project,rally_run_id:s.run
    }));
    if(token!==s.generation)return;
    s.gold=new Set((Array.isArray(raw.candidate_ids)?raw.candidate_ids:[]).filter(valid));
    s.revision=typeof raw.revision==="string"?raw.revision:"absent";
    s.writable=raw.writable===true;ui.goldCount.textContent=String(s.gold.size);
  };
  const loadPage=async()=>{
    if(!s.project||!s.run)return;
    const token=++s.generation;
    ui.list.replaceChildren(make("div","empty-state","Проверка источников и загрузка розыгрышей…"));
    message("Загрузка кадровых границ и Touch-данных…");
    try{
      const result=await Promise.all([
        request(url("/api/cpu/native/rallies/preview",{
          project_id:s.project,rally_run_id:s.run,offset:s.offset,limit:s.limit,
          gold_filter:ui.filter.value
        })),loadMarks(token)
      ]);
      if(token!==s.generation)return;
      const raw=result[0];
      if(raw.schema!=="volley-native-rally-preview/1"||
        raw.authoritative_game_events!==false||
        raw.automatic_montage_eligible!==false)
        throw Error("Неподдерживаемая версия предварительного CPU-анализа");
      s.items=Array.isArray(raw.items)?raw.items:[];
      s.total=Number(raw.total_candidates)||0;
      s.filtered=Number(raw.filtered_candidates??s.total)||0;
      s.video=typeof raw.video==="string"?raw.video:"";
      ui.total.textContent=String(s.total);
      cards();renderSelection();
      message("CPU-данные загружены. Розыгрыши предварительные; проверьте их в видеоплеере.","ok");
    }catch(e){
      if(token!==s.generation)return;
      s.items=[];s.total=0;cards();failed(e);
    }
  };
  const toggleGold=async id=>{
    if(!valid(id))return;
    if(!s.writable){
      message("Золотые отметки требуют изолированного Rust-хранилища.","error");return;
    }
    if(s.busy)return;
    s.busy=true;
    const marking=!s.gold.has(id);
    const run=s.run,project=s.project;
    try{
      const raw=await post("/api/cpu/native/rallies/marks",{
        project_id:project,rally_run_id:run,candidate_id:id,
        marked:marking,expected_revision:s.revision
      });
      if(run!==s.run||project!==s.project)return;
      s.gold=new Set((Array.isArray(raw.candidate_ids)?raw.candidate_ids:[]).filter(valid));
      s.revision=String(raw.revision);ui.goldCount.textContent=String(s.gold.size);
      if(ui.filter.value==="all")cards();
      else{s.offset=0;await loadPage();}
      message(marking?"Розыгрыш отмечен золотым.":"Золотая отметка снята.","ok");
    }catch(e){
      try{await loadMarks(s.generation);cards();}catch{}
      failed(e);
    }finally{s.busy=false;}
  };
  const toggleSelected=(item,add)=>{
    if(!wasmReady){message("Модуль Rust/WASM ещё не загружен.","error");return;}
    const id=item.candidate_id;if(!valid(id))return;
    if(add&&s.selected.size>=80&&!s.selected.has(id)){
      message("В одном монтаже допускается не более 80 розыгрышей.","error");return;
    }
    if(add)s.selected.set(id,{time:clock(item.detected_start_frame),duration:length(item)});
    else s.selected.delete(id);
    cards();renderSelection();
  };
  const view=item=>{
    s.current=item.candidate_id;cards();
    const start=Number(item.manual_clip?.start_frame??item.detected_start_frame??0);
    const end=Number(item.manual_clip?.end_frame??item.detected_end_frame??0);
    const key=s.project+":"+s.video;
    s.stopAt=(end+1)/30;
    ui.viewTitle.textContent="Розыгрыш · "+clock(item.detected_start_frame)+" – "+clock(item.detected_end_frame);
    ui.viewDescription.textContent="Фрагмент "+clock(start)+" – "+clock(end)+
      ". Кандидатов касания: "+(item.motion_event_count||0)+
      ". Игровые действия и личности игроков не подтверждены.";
    ui.videoTime.textContent=clock(start)+" – "+clock(end);
    wipe(ui.events);
    for(const evidence of (Array.isArray(item.motion_events)?item.motion_events:[]).slice(0,14)){
      ui.events.append(make("span","event-pill",clock(evidence.frame_index)+" · "+
        String(evidence.motion_pattern||"касание").replaceAll("_"," ")));
    }
    ui.emptyVideo.classList.add("hidden");
    if(s.videoKey!==key){
      s.videoKey=key;
      ui.video.src=url("/api/video-media",{project_id:s.project,video:s.video});
      ui.video.load();
      ui.video.addEventListener("loadedmetadata",()=>{
        ui.video.currentTime=start/30;
      },{once:true});
    }else{
      try{ui.video.currentTime=start/30;}catch{}
    }
  };
  const cards=()=>{
    wipe(ui.list);
    const visible=s.items;
    ui.caption.textContent="· "+(s.filtered?s.offset+1:0)+"–"+
      Math.min(s.offset+s.limit,s.filtered)+" из "+s.filtered;
    ui.previous.disabled=s.offset===0;
    ui.next.disabled=s.offset+s.limit>=s.filtered;
    if(!visible.length){
      ui.list.append(make("div","empty-state",s.items.length?
        "В выбранном фильтре нет карточек на этой странице.":
        "Розыгрыши не найдены. Выберите другую версию CPU-анализа."));
      return;
    }
    for(const item of visible){
      const id=item.candidate_id;
      if(!valid(id))continue;
      const gold=s.gold.has(id);
      const card=make("article","rally-card"+(gold?" gold":"")+
        (s.current===id?" active":""));
      card.tabIndex=0;card.setAttribute("role","button");
      card.setAttribute("aria-label","Просмотр розыгрыша "+clock(item.detected_start_frame));
      card.addEventListener("click",e=>{
        if(!e.target.closest("button,label,input"))view(item);
      });
      card.addEventListener("keydown",e=>{
        if((e.key==="Enter"||e.key===" ")&&e.target===card){
          e.preventDefault();view(item);
        }
      });
      card.addEventListener("contextmenu",e=>{e.preventDefault();toggleGold(id);});
      const top=make("div","card-top");
      top.append(make("span","id-label","ID "+id.slice(0,8)),
        make("span","card-num",length(item)));
      const when=make("div");
      when.append(make("div","card-time",clock(item.detected_start_frame)),
        make("div","card-timespan",clock(item.detected_start_frame)+
          " — "+clock(item.detected_end_frame)));
      const stats=make("div","card-stats"),touch=make("span"),ball=make("span");
      touch.append(make("strong","",item.motion_supported_touch_proposals||0),
        document.createTextNode(" касаний"));
      ball.append(make("strong","",Math.round((Number(item.ball_coverage)||0)*100)+"%"),
        document.createTextNode(" мяч"));
      stats.append(touch,ball);
      const bottom=make("div","card-bottom"),label=make("label","pick");
      const check=make("input");check.type="checkbox";check.checked=s.selected.has(id);
      check.disabled=!wasmReady;
      check.setAttribute("aria-label","Добавить в монтаж");
      check.addEventListener("change",e=>{e.stopPropagation();toggleSelected(item,check.checked);});
      label.append(check,document.createTextNode("В монтаж"));
      const star=make("button","gold-toggle",gold?"★":"☆");
      star.type="button";star.title="Золотая отметка";
      star.setAttribute("aria-pressed",String(gold));
      star.setAttribute("aria-label",gold?"Убрать отметку":"Отметить золотым");
      star.addEventListener("click",e=>{e.stopPropagation();toggleGold(id);});
      bottom.append(label,star);card.append(top,when,stats,bottom);ui.list.append(card);
    }
  };
  const uploadAudio=async()=>{
    const file=ui.music.files&&ui.music.files[0];
    if(!file)return null;
    if(file.size>90*1024*1024)throw Error("Файл музыки превышает 90 МиБ.");
    const form=new FormData();form.append("file",file,file.name);
    const data=await request("/api/montage/native/audio",{method:"POST",body:form});
    const name=data.audio_file||data.file||data.filename;
    if(typeof name!=="string"||!name)throw Error("Сервер не вернул имя аудиотрека.");
    return name;
  };
  const updateJob=async()=>{
    if(!s.jobId)return;
    try{
      const result=await request("/api/montage/native/status");
      if(result.id&&result.id!==s.jobId){
        stopPoll();ui.job.textContent="Запущен другой монтаж.";return;
      }
      const percentage=Number(result.overall_percent);
      ui.job.textContent=(result.stage||result.state||"Монтаж")+
        " · "+(Number.isFinite(percentage)?percentage.toFixed(0)+"%":"—");
      if(["complete","completed","done"].includes(result.state)){
        stopPoll();ui.cancel.classList.add("hidden");ui.render.disabled=false;
        if(typeof result.media_url==="string"&&
          result.media_url.startsWith("/api/montage/native/media?")){
          ui.download.href=result.media_url;ui.download.classList.remove("hidden");
        }
        message("Монтаж завершён. Видео доступно для скачивания.","ok");
      }else if(["cancelled","failed","error"].includes(result.state)){
        stopPoll();ui.cancel.classList.add("hidden");ui.render.disabled=false;
        message(result.error||"Ошибка монтажа.","error");
      }
    }catch(e){failed(e);}
  };
  const startMontage=async()=>{
    if(!wasmReady||s.busy||!s.run||!s.selected.size)return;
    const project=s.project,run=s.run,ids=Array.from(s.selected.keys());
    s.busy=true;ui.render.disabled=true;ui.download.classList.add("hidden");
    try{
      ui.job.textContent="Загрузка музыки и проверка исходников…";
      const layout=ui.layout.value;
      const volume=Number(ui.volume.value);
      const encoder=ui.encoder.value;
      const music=await uploadAudio();
      const result=await post("/api/montage/native/start-from-rally-candidates",{
        project_id:project,rally_run_id:run,candidate_ids:ids,
        audio_file:music,audio_volume:volume,
        layout,encoder
      });
      const id=result.id||result.job_id;
      if(typeof id!=="string"||!id)throw Error("Сервер не вернул ID монтажа.");
      s.jobId=id;ui.cancel.classList.remove("hidden");
      ui.job.textContent="Монтаж запущен.";
      message("Сборка выбранных розыгрышей из оригинального видео.","ok");
      stopPoll();await updateJob();
      if(s.jobId&&!ui.cancel.classList.contains("hidden")){
        s.timer=setInterval(updateJob,1500);
      }
    }catch(e){failed(e);ui.job.textContent="Монтаж не запущен.";ui.render.disabled=false;}
    finally{s.busy=false;}
  };
  ui.project.addEventListener("change",async()=>{
    s.catalogEpoch++;
    s.project=ui.project.value;s.analysis="";
    const entry=s.projectCatalog.find(row=>row.id===s.project);
    s.projectVideos=Array.isArray(entry?.videos)?
      entry.videos.filter(video=>typeof video==="string"):[];
    window.dispatchEvent(new Event("volley:project-ready"));
    try{await loadChain();}catch(e){failed(e);}
  });
  ui.analysis.addEventListener("change",()=>setAnalysis().catch(failed));
  ui.run.addEventListener("change",async()=>{
    const id=ui.run.value;reset();
    if(!(s.chain?.byAnalysis.get(s.analysis)||[]).some(row=>row.rally_run_id===id))return;
    s.run=id;
    try{await loadPage();}catch(e){failed(e);}
  });
  ui.refresh.addEventListener("click",()=>projects().catch(failed));
  window.addEventListener("volley:refresh-projects",()=>projects().catch(failed));
  window.addEventListener("volley:refresh-chain",()=>projects().catch(failed));
  window.addEventListener("volley:open-legacy-project",async event=>{
    const id=event.detail?.project_id;
    if(typeof id!=="string"||!/^[a-f0-9]{12}$/.test(id))return;
    try{
      await projects();
      const has=[...ui.project.options].some(row=>row.value===id);
      if(!has)throw Error("Проект не зарегистрирован в Rust");
      ui.project.value=id;ui.project.dispatchEvent(new Event("change"));
      document.getElementById("studio-nav-preview").click();
    }catch(e){failed(e);}
  });
  ui.filter.addEventListener("change",()=>{s.offset=0;loadPage().catch(failed);});
  ui.previous.addEventListener("click",()=>{
    if(s.offset>=s.limit){s.offset-=s.limit;loadPage().catch(failed);}
  });
  ui.next.addEventListener("click",()=>{
    if(s.offset+s.limit<s.filtered){s.offset+=s.limit;loadPage().catch(failed);}
  });
  ui.render.addEventListener("click",startMontage);
  ui.cancel.addEventListener("click",async()=>{
    if(!s.jobId)return;
    try{
      await post("/api/montage/native/cancel",{job_id:s.jobId});
      message("Отмена отправлена.","ok");await updateJob();
    }catch(e){failed(e);}
  });
  ui.video.addEventListener("timeupdate",()=>{
    if(s.stopAt!==null&&ui.video.currentTime>=s.stopAt){ui.video.pause();}
  });
  ui.video.addEventListener("error",()=>{
    if(s.videoKey)ui.viewDescription.textContent=
      "Видео недоступно для воспроизведения в этой Rust-версии. Проверьте подготовку видео.";
  });
  window.addEventListener("volley:open-native-rally",async event=>{
    const target=event.detail||{};
    if(typeof target.project!=="string"||typeof target.run!=="string"
      ||target.project!==s.project)return;
    try{
      if(!s.chain?.runToAnalysis.has(target.run))await projects();
      const analysis=s.chain?.runToAnalysis.get(target.run);
      if(analysis){
        ui.analysis.value=analysis;
        await setAnalysis(target.run);
      }
      if(s.run===target.run){
        document.getElementById("studio-nav-preview").click();
        message("Открыта выбранная CPU-версия розыгрышей.","ok");
      }else {
        message("Выбранная версия не найдена в списке. Обновите данные.","error");
      }
    }catch(e){failed(e);}
  });
  loadRustSelection().catch(e=>{
    wasmReady=false;
    failed(new Error("Rust/WASM выбор недоступен: "+String(e.message||e)));
    renderSelection();cards();
  });
  projects().catch(failed);
})();