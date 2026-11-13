"use strict";
// Transitional same-origin browser bridge. Rust validates all source IDs,
// contact evidence, gold revisions, original clip ranges and montage jobs.
(() => {
  const $ = id => document.getElementById(id);
  const ui = {
    project:$("studio-project"),analysis:$("studio-inference"),run:$("studio-run"),
    projectNew:$("studio-project-new"),projectDialog:$("studio-project-dialog"),
    projectDrop:$("studio-project-drop"),projectFiles:$("studio-project-file-input"),
    projectUploadStatus:$("studio-project-upload-progress"),
    projectPlayers:$("studio-project-players"),projectPlayerName:$("studio-project-player-name"),
    projectAddPlayer:$("studio-project-add-player"),
    createForm:$("studio-project-create-form"),projectName:$("studio-project-name"),
    projectVideos:$("studio-project-videos"),projectSubmit:$("studio-project-submit"),
    projectCreateError:$("studio-project-create-error"),
    projectCancel:$("studio-project-cancel"),projectClose:$("studio-project-close"),

    list:$("studio-rallies"),filter:$("studio-filter"),
    selectedCount:$("studio-selected-count"),banner:$("studio-banner"),
    video:$("studio-video"),
    emptyVideo:$("studio-video-empty"),videoTime:$("studio-video-time"),
    viewTitle:$("studio-view-title"),viewDescription:$("studio-view-description"),
    events:$("studio-event-summary"),selectionText:$("studio-selection-text"),
    picked:$("studio-picked"),encoder:$("studio-encoder"),music:$("studio-music"),
    layout:$("studio-layout"),volume:$("studio-volume"),
    render:$("studio-render"),job:$("studio-job"),cancel:$("studio-cancel"),
    download:$("studio-download")
  };
  const previewIsVisible=()=>!document.hidden&&
    !$("studio-preview-view").classList.contains("hidden");
  const s = {
    project:"",analysis:"",run:"",video:"",items:[],total:0,offset:0,limit:24,
    projectCatalog:[],projectVideos:[],chain:null,chainEpoch:0,catalogEpoch:0,
    filtered:0,gold:new Set(),revision:"absent",writable:false,marksLoaded:false,selected:new Map(),
    current:"",videoKey:"",seekAt:0,stopAt:null,clipStart:null,clipEnd:null,
    playbackMode:"sequence",generation:0,busy:false,jobId:"",timer:null,
    loadingMore:false,canLoadMore:true,pageLoading:false,pageError:""
  };
  // Persist selected source IDs only; restored IDs are validated against each catalog.
  const sourceSelection=(()=>{
    const key="volley:studio-source-selection:v1";
    const read=()=>{
      try{
        const raw=localStorage.getItem(key);
        if(!raw||raw.length>2048)return {};
        const value=JSON.parse(raw);
        return value&&typeof value==="object"&&!Array.isArray(value)?value:{};
      }catch{return {};}
    };
    const save=(project,analysis="",run="")=>{
      try{localStorage.setItem(key,JSON.stringify({project,analysis,run}));}catch{}
    };
    return {read,save};
  })();
  // Persist the small, source-scoped GPU -> CPU index. The UI can restore
  // selectors synchronously on refresh while metadata is checked in background.
  // Never cache video bytes or claim this snapshot verifies source evidence.
  const chainSnapshot=(()=>{
    const key="volley:studio-chain:v1";
    const read=(project,videos)=>{
      try{
        const raw=localStorage.getItem(key);
        if(!raw||raw.length>350000)return null;
        const row=JSON.parse(raw);
        if(row.project!==project||row.expires<=Date.now()||
           JSON.stringify(row.videos)!==JSON.stringify(videos)||
           !Array.isArray(row.overviews)||!Array.isArray(row.runs))return null;
        const overviews=new Map();
        for(const entry of row.overviews){
          if(!Array.isArray(entry)||entry.length!==2||!videos.includes(entry[0])||
             entry[1]?.schema!=="volley-native-analysis-overview/1"||
             entry[1].project_id!==project||entry[1].video!==entry[0])return null;
          overviews.set(entry[0],entry[1]);
        }
        return window.VolleyStudioChain?.build(videos,overviews,row.runs)||null;
      }catch{return null;}
    };
    const save=(project,videos,overviews,runs)=>{
      try{
        const packed=JSON.stringify({
          project,videos,overviews:[...overviews],runs,
          expires:Date.now()+86400000
        });
        if(packed.length<=350000)localStorage.setItem(key,packed);
      }catch{} // Storage may be disabled or full.
    };
    const drop=()=>{try{localStorage.removeItem(key);}catch{}};
    return {read,save,drop};
  })();
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
  // Cache only small derived metadata and immutable CPU preview pages, never video bytes.
  // A versioned session cache survives refresh; all writes / new pipeline results invalidate it.
  const previewCache=(()=>{
    const prefix="volley:studio-preview:v1:";
    const memory=new Map(),inflight=new Map(),limit=24;
    let epoch=0;
    const storage=()=>{
      try{return typeof sessionStorage==="undefined"?null:sessionStorage;}catch{return null;}
    };
    const read=path=>{
      let row=memory.get(path);
      if(!row){
        const store=storage();
        if(store){
          try{row=JSON.parse(store.getItem(prefix+path)||"null");}catch{}
        }
      }
      if(!row)return null;
      if(row.expires<=Date.now()){
        memory.delete(path);
        try{storage()?.removeItem(prefix+path);}catch{}
        return null;
      }
      memory.delete(path);memory.set(path,row);
      return row.value;
    };
    const save=(path,value,ttl,persist)=>{
      const row={expires:Date.now()+ttl,value};
      memory.delete(path);memory.set(path,row);
      while(memory.size>limit)memory.delete(memory.keys().next().value);
      if(!persist)return;
      try{
        const store=storage();
        if(!store)return;
        const packed=JSON.stringify(row);
        if(packed.length>240000)return; // No multi-MB evidence arrays in sessionStorage.
        const entries=[];
        for(let i=0;i<store.length;i++){
          const key=store.key(i);
          if(key?.startsWith(prefix))entries.push(key);
        }
        if(entries.length>=12){
          // Keep this bounded even across dozens of analysis branches.
          for(const key of entries.slice(0,entries.length-9))store.removeItem(key);
        }
        store.setItem(prefix+path,packed);
      }catch{} // Storage quotas / private mode must not break preview.
    };
    const drop=predicate=>{
      epoch++;
      for(const key of memory.keys())if(predicate(key))memory.delete(key);
      for(const key of inflight.keys())if(predicate(key))inflight.delete(key);
      const store=storage();
      if(!store)return;
      try{
        const keys=[];
        for(let i=0;i<store.length;i++){
          const key=store.key(i);
          if(key?.startsWith(prefix)&&predicate(key.slice(prefix.length)))keys.push(key);
        }
        for(const key of keys)store.removeItem(key);
      }catch{}
    };
    const load=(path,ttl,persist=false)=>{
      const value=read(path);
      if(value!==null)return Promise.resolve(value);
      if(inflight.has(path))return inflight.get(path);
      const started=epoch;
      const task=request(path).then(value=>{
        if(epoch===started)save(path,value,ttl,persist);
        return value;
      }).finally(()=>{if(inflight.get(path)===task)inflight.delete(path);});
      inflight.set(path,task);
      return task;
    };
    return {load,drop,peek:read};
  })();
  const invalidatePreviewRun=(project,run)=>{
    previewCache.drop(path=>path.startsWith("/api/cpu/native/rallies/preview?")&&
      path.includes("project_id="+encodeURIComponent(project))&&
      path.includes("rally_run_id="+encodeURIComponent(run)));
  };
  const invalidateCatalog=()=>{
    chainSnapshot.drop();
    previewCache.drop(path=>
      path==="/api/projects"||path.startsWith("/api/cpu/native/rallies/runs?")||
      path.startsWith("/api/studio/analysis-overview?")||
      path.startsWith("/api/cpu/native/rallies/marks?")||
      path.startsWith("/api/cpu/native/rallies/preview?")||
      path.startsWith("/api/cpu/native/rallies/timeline?"));
  };
  const make=(tag,klass,text)=>{
    const node=document.createElement(tag);
    if(klass)node.className=klass;
    if(text!==undefined)node.textContent=String(text);
    return node;
  };
  const pickers=new Map();
  const refreshPicker=select=>pickers.get(select)?.refresh();
  const makeAnalysisPicker=(select,primary)=>{
    const isProject=primary==="project";
    const host=make("div","analysis-picker");
    const trigger=make("button","analysis-picker-trigger");
    const list=make("div","analysis-picker-list hidden");
    trigger.type="button";trigger.setAttribute("aria-expanded","false");
    trigger.setAttribute("aria-label",select.getAttribute("aria-label"));
    select.classList.add("analysis-picker-native");
    select.parentNode.insertBefore(host,select.nextSibling);
    host.append(trigger,list);
    let editing=false;
    const close=()=>{list.classList.add("hidden");trigger.setAttribute("aria-expanded","false");};
    const animateRemoval=async row=>{
      if(!row.animate||window.matchMedia?.("(prefers-reduced-motion: reduce)").matches)return;
      const height=row.getBoundingClientRect().height;
      const animation=row.animate([
        {height:height+"px",opacity:1,overflow:"hidden",boxSizing:"border-box"},
        {height:"0px",opacity:0,paddingTop:"0px",paddingBottom:"0px",borderTopWidth:"0px",overflow:"hidden",boxSizing:"border-box"}
      ],{duration:220,easing:"ease-in-out",fill:"forwards"});
      await animation.finished.catch(()=>{});
    };
    const refresh=()=>{
      trigger.disabled=select.disabled;
      trigger.textContent=select.selectedOptions[0]?.textContent||"Выберите результат";
      wipe(list);
      for(const option of select.options){
        if(!option.value)continue;
        const row=make("div","analysis-picker-row");
        const choose=make("button","analysis-picker-choice",option.textContent);
        choose.type="button";choose.setAttribute("aria-pressed",String(option.value===select.value));
        choose.addEventListener("click",()=>{select.value=option.value;select.dispatchEvent(new Event("change",{bubbles:true}));close();refresh();});
        row.append(choose);
        for(const [action,label,symbol] of [["rename","Переименовать","✎"],["delete","Удалить","×"]]){
          const button=make("button","analysis-picker-action analysis-picker-"+action,symbol);
          button.type="button";
          button.title=label+": "+option.textContent;
          button.setAttribute("aria-label",button.title);
          button.addEventListener("click",async()=>{
            if(editing)return;
            if(isProject){
              const name=action==="rename"?window.prompt("Новое название проекта",option.textContent):null;
              if(action==="rename"&&name===null)return;
              if(action==="delete"&&!window.confirm("Удалить проект «"+option.textContent+"» и его результаты? Исходные видео сохранятся."))return;
              editing=true;button.disabled=true;
              try{
                if(action==="delete"){
                  await post("/api/project-delete",{id:option.value});
                  invalidateCatalog();
                  await animateRemoval(row);
                  if(s.project===option.value)await projects();
                  else{
                    s.projectCatalog=s.projectCatalog.filter(project=>project.id!==option.value);
                    option.remove();refresh();
                  }
                  message("Проект удалён.","ok");
                }else{
                  const result=await post("/api/project-update",{id:option.value,name});
                  const updated=result.project?.name||name.trim();
                  option.textContent=updated;
                  const item=s.projectCatalog.find(project=>project.id===option.value);
                  if(item)item.name=updated;
                  invalidateCatalog();refresh();
                  message("Название сохранено.","ok");
                }
              }catch(error){failed(error);}
              finally{editing=false;button.disabled=false;}
              return;
            }
            const source=primary?s.chain?.choices.find(item=>item.value===option.value):s.chain?.choices.find(item=>item.value===s.analysis);
            if(!source)return;
            const id=primary?source.id:option.value,kind=primary?source.kind:"rallies";
            const project=s.project;
            let name=null;
            if(action==="rename"){
              name=window.prompt("Новое название результата",primary?source.display_name:(s.chain?.byAnalysis.get(s.analysis)||[]).find(item=>item.rally_run_id===id)?.display_name||"");
              if(name===null)return;
            }
            editing=true;button.disabled=true;
            try{
              await post(action==="rename"?"/api/studio/analysis-rename":"/api/studio/analysis-edit",{project_id:project,video:source.video,kind,id,action,name});
              invalidateCatalog();
              if(project!==s.project)return;
              if(action==="delete")await animateRemoval(row);
              const deletesActiveAnalysis=action==="delete"&&primary&&s.analysis===option.value;
              const deletesActiveRun=action==="delete"&&!primary&&s.run===id;
              const analysis=deletesActiveAnalysis?"":s.analysis;
              const run=deletesActiveAnalysis||deletesActiveRun?"":s.run;
              // Refresh metadata without resetting the player when editing another version.
              if(action!=="delete")close();
              await loadChain(analysis,run,!deletesActiveAnalysis&&!deletesActiveRun);
              message(action==="rename"?"Название сохранено.":"Результат удалён.","ok");
            }catch(error){failed(error);}
            finally{editing=false;button.disabled=false;}
          });
          row.append(button);
        }
        list.append(row);
      }
    };
    trigger.addEventListener("click",()=>{const opening=list.classList.contains("hidden");refresh();list.classList.toggle("hidden",!opening);trigger.setAttribute("aria-expanded",String(opening));});
    select.addEventListener("change",refresh);
    document.addEventListener("click",event=>{if(!host.contains(event.target))close();});
    document.addEventListener("keydown",event=>{if(event.key==="Escape")close();});
    pickers.set(select,{refresh});refresh();
  };
  // Shared by the analysis tab and header: one fetch, cached across page refreshes.
  window.VolleyballStudioProjectCatalog=()=>previewCache.load("/api/projects",300000,true);
  // Canvas uses saved CPU frame evidence; toggles are local viewing preferences.
  const overlay = {
    canvas:$("studio-video-overlay"),status:$("studio-overlay-status"),
    rallies:$("studio-overlay-rallies"),touches:$("studio-overlay-touches"),
    trajectory:$("studio-overlay-trajectory"),players:$("studio-overlay-players"),
    pose:$("studio-overlay-pose"),court:$("studio-overlay-court"),
    mode:$("studio-overlay-processed"),rawMode:$("studio-overlay-raw"),rawControls:$("studio-overlay-raw-controls"),actionControls:$("studio-overlay-actions"),
    rawBall:$("studio-raw-ball"),rawVballVisible:$("studio-raw-vball-visible"),rawPeopleVisible:$("studio-raw-people-visible"),rawPlayer:$("studio-raw-player"),
    rawPose:$("studio-raw-pose"),rawPoseVisible:$("studio-raw-pose-visible"),
    raw:false,
    actionFlags:{attack:true,block:true,serve:true,overhead_pass:true,dig:true,low_touch:true},
    source:null,cache:new Map(),rawFrames:new Map(),presentedTime:null,
    fetching:new Set(),controllers:new Map(),failed:new Map(),loop:0,
    flags:{rallies:true,touches:true,trajectory:true,players:true,pose:false,court:true}
  };
  try {
    const saved=JSON.parse(localStorage.getItem("volley:preview-overlays:v1")||"null");
    if(saved&&typeof saved==="object")
      for(const k of Object.keys(overlay.flags))
        if(typeof saved[k]==="boolean")overlay.flags[k]=saved[k];
  }catch{}
  const updateOverlayToggles=()=>{
    for(const key of ["rallies","touches","trajectory","players","pose","court"]){
      const button=overlay[key];
      button.setAttribute("aria-pressed",String(overlay.flags[key]));
    }
  };
  updateOverlayToggles();
  const overlayMessage=text=>{
    overlay.status.textContent=text;
    overlay.status.classList.toggle("hidden",!text);
  };
  const overlayContext=()=>{
    const canvas=overlay.canvas;
    const cw=canvas.clientWidth,ch=canvas.clientHeight;
    if(!cw||!ch)return null;
    const dpr=Math.min(window.devicePixelRatio||1,2);
    const w=Math.round(cw*dpr),h=Math.round(ch*dpr);
    if(canvas.width!==w||canvas.height!==h){canvas.width=w;canvas.height=h;}
    const ctx=canvas.getContext("2d");
    if(!ctx)return null;
    ctx.setTransform(dpr,0,0,dpr,0,0);
    ctx.clearRect(0,0,cw,ch);
    const vw=ui.video.videoWidth,vh=ui.video.videoHeight;
    if(!vw||!vh)return null;
    const scale=Math.min(cw/vw,ch/vh);
    const width=vw*scale,height=vh*scale;
    return {ctx,x:(cw-width)/2,y:(ch-height)/2,width,height};
  };
  const overlayLabels={
    key:"",names:new Map(),loaded:false,loading:false,failedUntil:0,
    epoch:0,controller:null
  };
  const playerLabelKey=(project,run,video,sha)=>[project,run,video,sha].join(":");
  const invalidateOverlayLabels=()=>{
    overlayLabels.epoch++;
    overlayLabels.controller?.abort();
    overlayLabels.key="";overlayLabels.names.clear();
    overlayLabels.loaded=false;overlayLabels.loading=false;
    overlayLabels.failedUntil=0;overlayLabels.controller=null;
  };
  const requestOverlayLabels=(project,run,video,sha)=>{
    if(typeof sha!=="string"||!/^[a-f0-9]{64}$/.test(sha))return;
    const key=playerLabelKey(project,run,video,sha);
    if(overlayLabels.key!==key){
      invalidateOverlayLabels();
      overlayLabels.key=key;
    }
    if(overlayLabels.loaded||overlayLabels.loading||overlayLabels.failedUntil>Date.now())return;
    const epoch=overlayLabels.epoch;
    const controller=typeof AbortController==="function"?new AbortController():null;
    overlayLabels.loading=true;overlayLabels.controller=controller;
    request(url("/api/cpu/native/rallies/overlay-player-labels",{
      project_id:project,rally_run_id:run
    }),controller?{signal:controller.signal}:{}).then(value=>{
      if(epoch!==overlayLabels.epoch||key!==overlayLabels.key)return;
      if(value.schema!=="volley-native-preview-player-labels/1"||
         value.project_id!==project||value.rally_run_id!==run||
         value.video!==video||value.source_replay_sha256!==sha||
         !["manual","manual_and_automatic"].includes(value.identity_source)||
         !value.names||typeof value.names!=="object"||Array.isArray(value.names)||
         Object.keys(value.names).length>20000)
        throw Error("Имена игроков не соответствуют выбранному анализу");
      const names=new Map();
      for(const [track,name] of Object.entries(value.names)){
        if(/^[1-9]\d*$/.test(track)&&
           typeof name==="string"&&name.trim()&&name.length<=150)
          names.set(track,name.trim());
      }
      overlayLabels.names=names;
      overlayLabels.loaded=true;
      if(s.project===project&&s.run===run&&s.video===video)drawOverlay();
    }).catch(error=>{
      if(epoch!==overlayLabels.epoch||controller?.signal.aborted||
         error?.name==="AbortError")return;
      overlayLabels.failedUntil=Date.now()+5000;
      if(s.project===project&&s.run===run&&s.video===video)
        overlayMessage("Имена игроков недоступны: "+error.message);
    }).finally(()=>{
      if(epoch===overlayLabels.epoch){
        overlayLabels.loading=false;overlayLabels.controller=null;
      }
    });
  };
  const rawFusion=()=>s.chain?.choices.find(row=>row.value===s.analysis&&row.kind==="fusion")||null;
  const overlaySourceId=()=>overlay.raw?rawFusion()?.id||"":s.run;
  const overlayWindowStart=frame=>Math.max(0,Math.floor(frame/90)*90-60);
  const overlayWindowKey=(project,run,video,start)=>
    [project,overlay.raw?"gpu":"cpu",run,video,start,...(!overlay.raw&&overlay.flags.pose?["pose"]:[])].join(":");
  const rawFrameLimit=(project,run,video)=>overlay.rawFrames.get([project,run,video].join(":"));
  // The API returns overlapping 180-frame windows every 90 frames.
  // Keep drawing from a previous window while the next one loads.
  const cachedOverlayForFrame=(project,run,video,frame)=>{
    for(const [key,record] of [...overlay.cache].reverse()){
      if(key===overlayWindowKey(project,run,video,record.start_frame)&&
         record.start_frame<=frame&&frame<record.end_frame)return record;
    }
    return null;
  };
  // Fail closed: only provenance-labelled VballNet detections or their
  // bounded interpolation may enter the processed overlay canvas.
  const processedBallOverlayIsValid=value=>
    ["vballnet_only_short_gap_interpolation_v1",
      "vballnet_physical_teleport_repair_player_kink_v3"].includes(value.ball_policy)&&
    Array.isArray(value.frames)&&value.frames.every((row,index)=>{
      if(row.frame!==value.start_frame+index)return false;
      if(row.ball===null)
        return row.ball_origin===null&&row.ball_interpolated===false;
      if(!Array.isArray(row.ball)||row.ball.length!==2||
         row.ball.some(coordinate=>!Number.isFinite(coordinate)||coordinate<0||coordinate>1))
        return false;
      return (row.ball_origin==="vballnet"&&row.ball_interpolated===false)||
        (row.ball_origin==="vballnet_interpolated"&&row.ball_interpolated===true);
    });
  const requestOverlayWindow=(project,run,video,start)=>{
    if(overlay.raw&&Number.isSafeInteger(rawFrameLimit(project,run,video))&&
       start>=rawFrameLimit(project,run,video))return;
    const key=overlayWindowKey(project,run,video,start);
    if(overlay.cache.has(key)||overlay.fetching.has(key)||
       (overlay.failed.get(key)||0)>Date.now())return;
    // Obsolete seeks must not queue unbounded expensive verified replay reads.
    if(overlay.fetching.size>=3){
      const stale=overlay.controllers?.keys().next().value;
      if(!stale)return;
      overlay.controllers.get(stale)?.abort();
      overlay.controllers.delete(stale);
      overlay.fetching.delete(stale);
    }
    overlay.failed.delete(key);
    const controller=typeof AbortController==="function"?new AbortController():null;
    overlay.controllers?.set(key,controller);
    overlay.fetching.add(key);
    const isRaw=overlay.raw;
    const path=isRaw?url("/api/gpu/native/overlays",{
      project_id:project,video,fusion_id:run,start_frame:start,count:180
    }):url("/api/cpu/native/rallies/overlays",{
      project_id:project,rally_run_id:run,start_frame:start,count:180,
      ...(overlay.flags.pose?{pose:true}:{})
    });
    request(path,controller?{signal:controller.signal}:{}).then(value=>{
      if(controller?.signal.aborted)return;
      const selected=timeline.data?.project_id===project&&
        timeline.data?.rally_run_id===run&&timeline.data?.video===video?
        timeline.data:null;
      if(value.schema!==(isRaw?"volley-native-gpu-overlay/1":"volley-native-preview-overlay/1")||
         (!isRaw&&selected?.source_replay_sha256&&
          value.replay_sha256!==selected.source_replay_sha256)||
         value.fps!==fps()||!Number.isSafeInteger(value.width)||value.width<=0||
         !Number.isSafeInteger(value.height)||value.height<=0||
         value.project_id!==project||
         (isRaw?value.fusion_id!==run:value.rally_run_id!==run)||
         (isRaw?!/^[a-f0-9]{64}$/.test(value.source_fusion_sha256):false)||
         value.video!==video||value.start_frame!==start||
         !Number.isSafeInteger(value.end_frame)||value.end_frame<=start||
         (isRaw&&(!Number.isSafeInteger(value.total_frames)||
           value.total_frames<value.end_frame))||
         !Array.isArray(value.frames)||value.frames.length!==value.end_frame-start)
        throw Error("Несовместимые кадры предпросмотра");
      if(!isRaw&&!processedBallOverlayIsValid(value))
        throw Error("Processed ball source is not VballNet-only");
      if(s.project!==project||s.video!==video||overlay.raw!==isRaw||
         overlaySourceId()!==run)return;
      if(isRaw)overlay.rawFrames.set([project,run,video].join(":"),value.total_frames);
      overlay.cache.set(key,value);
      while(overlay.cache.size>16)overlay.cache.delete(overlay.cache.keys().next().value);
      overlayMessage((isRaw?overlay.rawPoseVisible.checked:overlay.flags.pose)&&
        value.poses_available===false?
          "RTMPose-M: для выбранного GPU-анализа нет сохранённых поз.":"");
      drawOverlay();
    }).catch(error=>{
      if(controller?.signal.aborted||error?.name==="AbortError")return;
      // Transient failures must not permanently disable overlays in a window.
      overlay.failed.set(key,Date.now()+5000);
      if(s.project===project&&s.video===video&&overlay.raw===isRaw&&overlaySourceId()===run)
        overlayMessage("Разметка недоступна: "+error.message);
    }).finally(()=>{
      if(overlay.controllers?.get(key)===controller){
        overlay.controllers.delete(key);
        overlay.fetching.delete(key);
      }
    });
  };
  const prefetchOverlayAtFrame=frame=>{
    const sourceId=overlaySourceId();
    if(!s.project||!sourceId||!s.video||!Number.isSafeInteger(frame)||frame<0)return;
    const total=overlay.raw?rawFrameLimit(s.project,sourceId,s.video):
      timeline.data?.project_id===s.project&&
      timeline.data?.rally_run_id===s.run?timeline.data.total_frames:null;
    const start=overlayWindowStart(frame);
    if(!Number.isSafeInteger(total)||start<total)
      requestOverlayWindow(s.project,sourceId,s.video,start);
    // Two adjacent windows cover seeking and the first seconds of playback.
    const next=overlayWindowStart((Math.floor(frame/90)+1)*90);
    if(next!==start&&(!overlay.raw||Number.isSafeInteger(total))&&
       (!Number.isSafeInteger(total)||next<total))
      requestOverlayWindow(s.project,sourceId,s.video,next);
  };
  const cpuTouchStyle={
    attack:["#ff6b6b","≈ Атака"],
    block:["#c38cff","≈ Блок"],
    serve:["#f29d49","≈ Подача"],
    overhead_pass:["#55d6ff","≈ Передача"],
    dig:["#71e49e","≈ Приём"],
    low_touch:["#e5c55d","≈ Нижнее касание"]
  };
  // The timeline belongs to the selected saved CPU postprocessing snapshot.
  // Raw GPU detections and limited rally-card motion_events are never Touchs.
  const cpuTouchesForFrame=(project,run,video,sha,frame)=>{
    const data=timeline.data;
    if(!data||data.schema!=="volley-native-rally-timeline/1"||
       data.project_id!==project||data.rally_run_id!==run||
       data.video!==video||data.source_replay_sha256!==sha||
       !Array.isArray(data.touch_annotations))return [];
    const events=data.touch_annotations;
    let left=0,right=events.length;
    while(left<right){
      const mid=(left+right)>>>1;
      if(events[mid].frame_index<frame-8)left=mid+1;else right=mid;
    }
    const near=[];
    for(let i=left;i<events.length&&events[i].frame_index<=frame+8;i++)
      near.push(events[i]);
    return near;
  };
  const drawCpuTouches=(ctx,record,project,run,video,frame,x,y,width,height)=>{
    if(!overlay.flags.touches||!record.width||!record.height)return;
    const events=cpuTouchesForFrame(project,run,video,record.replay_sha256,frame);
    for(const event of events){
      const kind=event.kind;
      if(kind&&(!Object.hasOwn(cpuTouchStyle,kind)||!overlay.actionFlags[kind]))continue;
      if(!Array.isArray(event.ball_position_px)||event.ball_position_px.length!==2)continue;
      const [bx,by]=event.ball_position_px;
      if(!Number.isFinite(bx)||!Number.isFinite(by)||
         bx<0||by<0||bx>record.width||by>record.height)continue;
      const [px,py]=[x+bx/record.width*width,y+by/record.height*height];
      const [color,label]=cpuTouchStyle[kind]||["#c6bf96","Касание"];
      ctx.beginPath();ctx.arc(px,py,kind?12:8,0,2*Math.PI);
      ctx.strokeStyle=color;ctx.lineWidth=kind?3:1.7;ctx.stroke();
      ctx.font="bold 12px Inter, sans-serif";
      ctx.textBaseline="bottom";
      const textWidth=ctx.measureText(label).width;
      const tx=Math.min(x+width-textWidth-9,Math.max(x+5,px+15));
      const ty=Math.max(y+17,Math.min(y+height-5,py-6));
      ctx.fillStyle="rgba(9,20,35,.94)";
      ctx.fillRect(tx-4,ty-14,textWidth+8,17);
      ctx.fillStyle=color;ctx.fillText(label,tx,ty);
    }
  };
  const courtPreview={key:"",data:null,loading:false,failed:false};
  const drawCourtPreview=box=>{
    if(!overlay.flags.court||!s.project||!s.video)return;
    const key=s.project+":"+s.video;
    if(courtPreview.key!==key){
      courtPreview.key=key;courtPreview.data=null;
      courtPreview.loading=false;courtPreview.failed=false;
    }
    if(!courtPreview.data&&!courtPreview.loading&&!courtPreview.failed){
      courtPreview.loading=true;
      request(url("/api/video-edit",{project_id:s.project,video:s.video})).then(data=>{
        if(courtPreview.key!==key)return;
        courtPreview.data=data;
        drawOverlay();
      }).catch(()=>{
        if(courtPreview.key===key)courtPreview.failed=true;
      }).finally(()=>{if(courtPreview.key===key)courtPreview.loading=false;});
    }
    const data=courtPreview.data;
    if(!data)return;
    const {ctx,x,y,width,height}=box;
    const position=p=>[x+p[0]*width,y+p[1]*height];
    if(Array.isArray(data.court)&&data.court.length===4){
      ctx.beginPath();
      data.court.forEach((p,i)=>{const [px,py]=position(p);if(i)ctx.lineTo(px,py);else ctx.moveTo(px,py);});
      ctx.closePath();ctx.strokeStyle="#ffd166";ctx.lineWidth=2.25;ctx.stroke();
    }
    if(Array.isArray(data.net)&&data.net.length===2){
      const [ax,ay]=position(data.net[0]),[bx,by]=position(data.net[1]);
      ctx.beginPath();ctx.moveTo(ax,ay);ctx.lineTo(bx,by);
      ctx.strokeStyle="#35e0ef";ctx.lineWidth=2.5;ctx.stroke();
    }
  };
  const drawOverlay=()=>{
    const box=overlayContext(),source=overlay.source;
    if(box)drawCourtPreview(box);
    if(!box||!source||(!overlay.raw&&!s.run)||!s.video||!ui.video.src||ui.video.seeking||
       s.videoKey!==s.project+":"+s.video||
       (ui.video.readyState!==undefined&&ui.video.readyState<2))return;
    // Until the next decoded frame arrives, keep the canvas transparent.
    if(!ui.video.paused&&typeof ui.video.requestVideoFrameCallback==="function"&&
       !Number.isFinite(overlay.presentedTime))return;
    if(!overlay.raw&&!overlay.flags.touches&&!overlay.flags.trajectory&&!overlay.flags.players&&!overlay.flags.pose)return;
    // HTMLVideoElement.currentTime can advance before a frame is decoded.
    // requestVideoFrameCallback.mediaTime is the actual frame on screen.
    const shown=Number.isFinite(overlay.presentedTime)?
      overlay.presentedTime:ui.video.currentTime;
    const frame=Math.max(0,Math.floor(shown*fps()+0.001));
    const project=s.project,run=overlaySourceId(),video=s.video;
    if(!run){if(overlay.raw)overlayMessage("Для просмотра сырых данных выберите объединённый GPU-анализ (fusion).");return;}
    const start=overlayWindowStart(frame);
    const key=overlayWindowKey(project,run,video,start);
    const active=overlay.cache.get(key);
    const record=active&&active.start_frame<=frame&&frame<active.end_frame?
      active:cachedOverlayForFrame(project,run,video,frame);
    const total=overlay.raw?rawFrameLimit(project,run,video):timeline.data?.total_frames;
    if(!active&&(!Number.isSafeInteger(total)||start<total))
      requestOverlayWindow(project,run,video,start);
    // Prefetch before a 90-frame boundary; do not request past video end.
    const nextBoundary=(Math.floor(frame/90)+1)*90;
    if(active&&frame>=nextBoundary-45){
      const nextStart=overlayWindowStart(nextBoundary);
      if(nextStart!==start&&(!Number.isSafeInteger(total)||nextStart<total))
        requestOverlayWindow(project,run,video,nextStart);
    }
    if(!record){
      // Saved CPU Touch positions already belong to the validated timeline.
      // Don't make them wait for the much larger, SHA-indexed player replay.
      const index=timeline.data;
      if(!overlay.raw&&index?.project_id===project&&index?.rally_run_id===run&&
         index?.video===video&&typeof index.source_replay_sha256==="string"){
        const {ctx,x,y,width,height}=box;
        drawCpuTouches(ctx,{
          width:ui.video.videoWidth,height:ui.video.videoHeight,
          replay_sha256:index.source_replay_sha256
        },project,run,video,frame,x,y,width,height);
      }
      return;
    }
    // The timeline and window must describe the same immutable CPU replay.
    const digest=!overlay.raw&&timeline.data?.project_id===project&&
      timeline.data?.rally_run_id===run&&timeline.data?.video===video?
      timeline.data.source_replay_sha256:null;
    if(digest&&record.replay_sha256!==digest){
      overlayMessage("Наложение не соответствует выбранной CPU-версии.");
      return;
    }
    if(record.width&&record.height&&ui.video.videoWidth&&ui.video.videoHeight&&
      Math.abs((ui.video.videoWidth/ui.video.videoHeight)/
        (record.width/record.height)-1)>0.025){
      overlayMessage("Размеры видео и наложения несовместимы.");
      return;
    }
    if(!overlay.raw&&overlay.flags.players)
      requestOverlayLabels(project,run,video,record.replay_sha256);
    const {ctx,x,y,width,height}=box;
    const point=ball=>[x+ball[0]*width,y+ball[1]*height];
    const frames=record.frames;
    if(overlay.raw){
      const current=frames.find(row=>row.frame===frame);
      if(!current?.raw)return;
      const raw=current.raw;
      const enough=(score,input)=>Number.isFinite(score)&&score>=Number(input.value)/100;
      ctx.font="bold 11px Inter, sans-serif";ctx.textBaseline="top";ctx.lineWidth=1.8;
      for(const ball of raw.balls||[]){
        if(ball.source!=="vballnet"||!overlay.rawVballVisible.checked||
          !enough(ball.score,overlay.rawBall)||!Array.isArray(ball.point))continue;
        const [px,py]=point(ball.point);
        ctx.beginPath();ctx.arc(px,py,5,0,Math.PI*2);
        ctx.fillStyle="#47e2ff";ctx.fill();
        ctx.fillText(Math.round(ball.score*100)+"%",px+7,py-6);
      }
      if(overlay.rawPeopleVisible.checked){
        for(const player of raw.players||[]){
          if(!Array.isArray(player.box)||player.box.length!==4)continue;
          const [x1,y1,x2,y2]=player.box,px=x+x1*width,py=y+y1*height;
          if(enough(player.score,overlay.rawPlayer)){
            ctx.strokeStyle="#71acff";ctx.strokeRect(px,py,(x2-x1)*width,(y2-y1)*height);
            ctx.fillStyle="#d9eaff";ctx.fillText(Math.round(player.score*100)+"%",px+2,py+2);
          }
        }
      }
      if(overlay.rawPoseVisible.checked){
        const edges=[[5,7],[7,9],[6,8],[8,10],[5,6],[5,11],[6,12],
          [11,12],[11,13],[13,15],[12,14],[14,16]];
        const threshold=Number(overlay.rawPose.value)/100;
        // The original RTMPose observations remain in raw.poses. Only draw
        // visual-only interpolation when no observation exists on this frame.
        const interpolated=!raw.poses?.length && Array.isArray(raw.poses_interpolated);
        const poseRows=interpolated?raw.poses_interpolated:(raw.poses||[]);
        ctx.globalAlpha=interpolated?0.78:1;
        for(const person of poseRows){
          const pose=person.keypoints;
          if(!Array.isArray(pose)||pose.length!==17)continue;
          const valid=i=>Array.isArray(pose[i])&&pose[i].length===3&&
            pose[i].every(Number.isFinite)&&pose[i][2]>=threshold;
          ctx.strokeStyle="#6af6ae";ctx.fillStyle="#f9d977";ctx.lineWidth=2;
          for(const [a,b] of edges){
            if(!valid(a)||!valid(b))continue;
            const [ax,ay]=point(pose[a]),[bx,by]=point(pose[b]);
            ctx.beginPath();ctx.moveTo(ax,ay);ctx.lineTo(bx,by);ctx.stroke();
          }
          for(let i=0;i<17;i++)if(valid(i)){
            const [px,py]=point(pose[i]);
            ctx.beginPath();ctx.arc(px,py,2.5,0,Math.PI*2);ctx.fill();
          }
        }
        ctx.globalAlpha=1;
      }
      return;
    }
    if(overlay.flags.trajectory&&!overlay.raw){
      // Only connect consecutive observed frames; never fabricate a long missing trajectory.
      ctx.beginPath();let previous=-100;
      for(const row of frames){
        if(row.frame<frame-44||row.frame>frame||!Array.isArray(row.ball)){
          if(row.frame>=frame-44&&row.frame<=frame)previous=-100;
          continue;
        }
        const [px,py]=point(row.ball);
        if(row.frame-previous>2)ctx.moveTo(px,py);
        else ctx.lineTo(px,py);
        previous=row.frame;
      }
      ctx.strokeStyle="#47e2ff";ctx.lineWidth=2.5;ctx.lineJoin="round";
      ctx.shadowColor="#1cb9ff";ctx.shadowBlur=5;ctx.stroke();ctx.shadowBlur=0;
    }
    const current=frames.find(row=>row.frame===frame);
    if(overlay.flags.pose && current?.players?.length){
      const edges=[[5,7],[7,9],[6,8],[8,10],[5,6],[5,11],[6,12],
        [11,12],[11,13],[13,15],[12,14],[14,16]];
      ctx.strokeStyle="#6af6ae";ctx.fillStyle="#f9d977";
      ctx.lineWidth=2.25;ctx.lineCap="round";
      for(const person of current.players){
        const pose=person.pose;
        if(!Array.isArray(pose)||pose.length!==17)continue;
        const valid=i=>Array.isArray(pose[i])&&pose[i].length===3&&
          pose[i].every(Number.isFinite)&&pose[i][2]>=0.3;
        for(const [a,b] of edges){
          if(!valid(a)||!valid(b))continue;
          const [ax,ay]=point(pose[a]),[bx,by]=point(pose[b]);
          ctx.beginPath();ctx.moveTo(ax,ay);ctx.lineTo(bx,by);ctx.stroke();
        }
        for(let i=0;i<pose.length;i++){
          if(!valid(i))continue;
          const [px,py]=point(pose[i]);
          ctx.beginPath();ctx.arc(px,py,i===9||i===10?3.5:2.5,0,2*Math.PI);ctx.fill();
        }
      }
    }
    // Only CPU-processed mode may display verified timeline touch annotations.
    drawCpuTouches(ctx,record,project,run,video,frame,x,y,width,height);
    if(overlay.flags.players&&current){
      ctx.strokeStyle="#71acff";ctx.lineWidth=1.8;
      ctx.font="bold 11px Inter, sans-serif";ctx.textBaseline="top";
      for(const player of current.players||[]){
        if(!Array.isArray(player.box)||player.box.length!==4)continue;
        const [x1,y1,x2,y2]=player.box;
        const px=x+x1*width,py=y+y1*height;
        const rw=(x2-x1)*width,rh=(y2-y1)*height;
        ctx.strokeRect(px,py,rw,rh);
        const labelsKey=playerLabelKey(project,run,video,record.replay_sha256);
        const owner=overlayLabels.loaded&&overlayLabels.key===labelsKey?
          overlayLabels.names.get(String(player.id)):null;
        const name=owner||"ID "+player.id,tw=ctx.measureText(name).width+10;
        ctx.fillStyle="rgba(12,47,91,.88)";
        ctx.fillRect(px,py+rh,tw,16);
        ctx.fillStyle="#f1f8ff";ctx.fillText(name,px+5,py+rh+1);
      }
    }
    if(current&&Array.isArray(current.ball)){
      const [px,py]=point(current.ball);
      if(overlay.flags.trajectory){
        ctx.beginPath();ctx.arc(px,py,5,0,2*Math.PI);
        ctx.fillStyle="#e7fbff";ctx.fill();
        ctx.strokeStyle="#21b4fc";ctx.lineWidth=2;ctx.stroke();
      }
    }
  };
  const setOverlaySource=item=>{
    overlay.source=item?{events:[]}:null;
    overlayMessage("");
    if(!item){
      invalidateOverlayLabels();
      for(const controller of overlay.controllers.values())controller?.abort();
      overlay.controllers.clear();overlay.fetching.clear();
      overlay.cache.clear();overlay.rawFrames.clear();overlay.failed.clear();overlay.presentedTime=null;
    }
    drawOverlay();
  };
  window.addEventListener("volley:court-calibration-updated",event=>{
    if(courtPreview.key===event.detail?.project_id+":"+event.detail?.video){
      courtPreview.data=null;courtPreview.failed=false;drawOverlay();
    }
  });  window.addEventListener("volley:track-assignments-updated",event=>{
    const detail=event.detail||{};
    if(detail.project_id===s.project&&detail.video===s.video){
      invalidateOverlayLabels();
      drawOverlay();
    }
  });
  for(const eventName of ["volley:player-library-updated","volley:roster-updated"]){
    window.addEventListener(eventName,()=>{
      invalidateOverlayLabels();
      drawOverlay();
    });
  }
  for(const key of ["rallies","touches","trajectory","players","pose","court"]){
    overlay[key].addEventListener("click",()=>{
      overlay.flags[key]=!overlay.flags[key];
      updateOverlayToggles();
      try{localStorage.setItem("volley:preview-overlays:v1",JSON.stringify(overlay.flags));}catch{}
      drawOverlay();
      drawTimeline();
    });
  }
  if(overlay.mode){
    const selectMode=raw=>{
      if(overlay.raw===raw)return;
      overlay.raw=raw;
      overlay.mode.setAttribute("aria-selected",String(!raw));
      overlay.rawMode.setAttribute("aria-selected",String(raw));
      overlay.rawControls.classList.toggle("hidden",!raw);
      overlay.actionControls.classList.toggle("hidden",raw);
      overlay.mode.closest(".video-overlay-toggles").classList.toggle("raw-selected",raw);
      setOverlaySource(raw||s.run?{}:null);
      if(raw&&s.video){ensurePreviewVideo();prefetchOverlayAtFrame(0);}
      if(raw&&!rawFusion())overlayMessage("У выбранного GPU-источника нет сохранённого объединённого fusion.");
      if(!raw&&!s.run)overlayMessage("Для обработанного режима выберите CPU-постобработку.");
      if(raw)timeline.status.textContent="Сырые GPU-данные · без CPU-разметки";
      else if(!s.run)timeline.status.textContent="Выберите CPU-версию";
      else if(timeline.data)timeline.status.textContent=
        timeline.data.rallies.length+" розыгрышей · "+timeline.data.touch_frames.length+" касаний (CPU)";
      drawTimeline();
      drawOverlay();
    };
    overlay.mode.addEventListener("click",()=>selectMode(false));
    overlay.rawMode.addEventListener("click",()=>selectMode(true));
    for(const [input,value] of [[overlay.rawBall,$("studio-raw-ball-value")],
      [overlay.rawPlayer,$("studio-raw-player-value")],
      [overlay.rawPose,$("studio-raw-pose-value")]]){
      input.addEventListener("input",()=>{value.textContent=input.value+"%";drawOverlay();});
    }
    for(const input of [overlay.rawVballVisible,overlay.rawPeopleVisible,overlay.rawPoseVisible])
      input.addEventListener("change",drawOverlay);
    for(const input of overlay.actionControls.querySelectorAll("[data-overlay-action]")){
      input.addEventListener("change",()=>{overlay.actionFlags[input.dataset.overlayAction]=input.checked;drawOverlay();});
    }
  }
  const startOverlayFrameLoop=()=>{
    const token=++overlay.loop;
    if(typeof ui.video.requestVideoFrameCallback!=="function")return;
    const tick=(_now,metadata)=>{
      if(token!==overlay.loop||ui.video.seeking)return;
      if(Number.isFinite(metadata?.mediaTime))
        overlay.presentedTime=metadata.mediaTime;
      drawOverlay();
      if(!ui.video.paused)ui.video.requestVideoFrameCallback(tick);
    };
    ui.video.requestVideoFrameCallback(tick);
  };
  ui.video.addEventListener("playing",startOverlayFrameLoop);
  ui.video.addEventListener("pause",()=>{
    overlay.loop++;overlay.presentedTime=null;drawOverlay();
  });
  ui.video.addEventListener("seeking",()=>{
    overlay.loop++;overlay.presentedTime=null;
    overlayContext(); // Never leave previous-frame boxes on a pending seek.
  });
  ui.video.addEventListener("seeked",()=>{
    overlay.presentedTime=null;drawOverlay();
    if(!ui.video.paused)startOverlayFrameLoop();
  });
  ui.video.addEventListener("timeupdate",()=>{
    if(typeof ui.video.requestVideoFrameCallback!=="function"&&!ui.video.seeking)
      drawOverlay();
  });
  ui.video.addEventListener("loadedmetadata",()=>{
    overlay.presentedTime=null;drawOverlay();
  });
  window.addEventListener("resize",drawOverlay);
  const wipe=node=>{while(node.firstChild)node.firstChild.remove();};
  const message=(text,mood)=>{
    ui.banner.className="banner"+(mood?" "+mood:"");
    ui.banner.textContent=String(text);
  };
  const failed=e=>message(e instanceof Error?e.message:String(e),"error");
  const fps=()=>window.VolleyballUi.engine.volley_evidence_fps(s.fps||0);
  const clock=frames=>{
    const sec=Math.max(0,Math.floor((Number(frames)||0)/fps()));
    return String(Math.floor(sec/60)).padStart(2,"0")+":"+String(sec%60).padStart(2,"0");
  };
  const length=item=>(Math.max(0,
    (Number(item.detected_end_frame)-Number(item.detected_start_frame)+1)/fps())).toFixed(1)+" с";
  const second=frame=>(Math.max(0,Number(frame)||0)/fps())
    .toLocaleString("ru-RU",{minimumFractionDigits:1,maximumFractionDigits:1});
  // Count only players with a source-bound Touch Track ID; other people
  // visible on court are not necessarily participants in the rally.
  const touchingPlayers=item=>Array.isArray(item.provisional_track_ids)?
    new Set(item.provisional_track_ids.filter(id=>Number.isSafeInteger(id)&&id>0)).size:null;
  const valid=id=>typeof id==="string"&&/^[a-f0-9]{24}$/.test(id);
  // The Rust timeline index is source-verified and spans the complete original
  // video; unlike rally cards it is independent of pagination and gold filters.
  const timeline={
    track:$("studio-timeline-track"),canvas:$("studio-timeline-canvas"),
    playhead:$("studio-timeline-playhead"),status:$("studio-timeline-status"),
    middle:$("studio-timeline-middle"),end:$("studio-timeline-end"),
    data:null,epoch:0,dragging:false
  };
  const timeLabel=value=>{
    const seconds=Math.max(0,Math.floor(Number(value)||0));
    const minutes=Math.floor(seconds/60),s=String(seconds%60).padStart(2,"0");
    return minutes>=60?Math.floor(minutes/60)+":"+
      String(minutes%60).padStart(2,"0")+":"+s:
      String(minutes).padStart(2,"0")+":"+s;
  };
  const timelineDuration=()=>{
    const data=timeline.data;
    const actual=s.videoKey===s.project+":"+s.video?ui.video.duration:0;
    // The decoded media is authoritative even when no CPU timeline exists.
    // Otherwise raw-only GPU previews could play but never seek.
    if(Number.isFinite(actual)&&actual>0)return actual;
    if(!data)return 0;
    const source=Number(data.video_duration_sec);
    return Number.isFinite(source)&&source>0?source:
      Math.max(0,Number(data.total_frames)/fps()||0);
  };
  const updateTimelineProgress=()=>{
    const duration=timelineDuration();
    const position=Math.min(duration,Math.max(0,
      Number.isFinite(ui.video.currentTime)&&s.videoKey===s.project+":"+s.video?
        ui.video.currentTime:s.seekAt));
    timeline.playhead.style.left=duration?(100*position/duration)+"%":"0%";
    timeline.middle.textContent=timeLabel(Math.floor(position*10+0.5)/10)+"."+String(Math.floor(position*10+0.5)%10);
    timeline.track.setAttribute("aria-valuemax",String(Math.ceil(duration)));
    timeline.track.setAttribute("aria-valuenow",String(Math.floor(position)));
    timeline.track.setAttribute("aria-valuetext",
      duration?timeLabel(position)+" из "+timeLabel(duration):"Нет видео");
  };
  const drawTimeline=()=>{
    const canvas=timeline.canvas,ctx=canvas.getContext("2d");
    if(!ctx)return;
    const width=canvas.clientWidth,height=canvas.clientHeight;
    if(width<=0||height<=0)return;
    const dpr=Math.min(window.devicePixelRatio||1,2);
    const dw=Math.round(width*dpr),dh=Math.round(height*dpr);
    if(canvas.width!==dw||canvas.height!==dh){canvas.width=dw;canvas.height=dh;}
    ctx.setTransform(dpr,0,0,dpr,0,0);
    ctx.clearRect(0,0,width,height);
    const y=3,bandHeight=Math.max(1,height-6);
    ctx.fillStyle="#192e49";ctx.fillRect(0,y,width,bandHeight);
    const data=timeline.data,duration=timelineDuration();
    if(!overlay.raw&&data&&duration){
      // A single shared seek lane: Touch ticks are overlaid on rally bands.
      if(overlay.flags.rallies){
        for(const rally of data.rallies){
          const start=Number(rally.start_frame)/fps(),end=(Number(rally.end_frame)+1)/fps();
          const x=Math.max(0,Math.min(width,start/duration*width));
          const x2=Math.max(0,Math.min(width,end/duration*width));
          ctx.fillStyle=s.gold.has(rally.candidate_id)?"#dfb14b":
            s.current===rally.candidate_id?"#75d5ff":"#2fa8e1";
          ctx.fillRect(x,y,Math.max(1,x2-x),bandHeight);
        }
      }
      if(overlay.flags.touches&&Array.isArray(data.touch_annotations)){
        for(const classified of [false,true]){
          for(const touch of data.touch_annotations){
            const style=cpuTouchStyle[touch.kind];
            if(Boolean(style)!==classified||style&&!overlay.actionFlags[touch.kind])continue;
            const x=Math.floor(Number(touch.frame_index)/fps()/duration*width);
            if(!Number.isFinite(x)||x<0||x>=width)continue;
            ctx.fillStyle=style?style[0]:"#c6bf96";
            ctx.fillRect(x,y,Math.max(2,Math.ceil(dpr)),bandHeight);
          }
        }
      }
    }
    $("studio-timeline-rally-key").hidden=overlay.raw||!overlay.flags.rallies;
    $("studio-timeline-touch-key").hidden=overlay.raw||!overlay.flags.touches;
    updateTimelineProgress();
  };
  const clearTimeline=()=>{
    timeline.epoch++;timeline.dragging=false;timeline.data=null;
    timeline.status.textContent="Выберите CPU-версию";
    timeline.middle.textContent="—";timeline.end.textContent="—";
    timeline.track.title="";drawTimeline();
  };
  const loadTimeline=async()=>{
    const project=s.project,run=s.run,epoch=++timeline.epoch;
    if(!project||!run)return;
    timeline.status.textContent="Загрузка временных меток…";
    try{
      const raw=await previewCache.load(url("/api/cpu/native/rallies/timeline",{
        project_id:project,rally_run_id:run
      }),900000,true);
      if(epoch!==timeline.epoch||s.project!==project||s.run!==run)return;
      if(raw.schema!=="volley-native-rally-timeline/1"||
        raw.project_id!==project||raw.rally_run_id!==run||
        typeof raw.video!=="string"||
        !Number.isSafeInteger(raw.total_frames)||raw.total_frames<=0||
        !Number.isFinite(raw.video_duration_sec)||raw.video_duration_sec<=0||
        !Array.isArray(raw.rallies)||!Array.isArray(raw.touch_frames)||
        !Array.isArray(raw.touch_annotations)||
        raw.touch_annotations.length!==raw.touch_frames.length||
        !raw.touch_annotations.every((event,index)=>
          event.frame_index===raw.touch_frames[index]&&
          (event.kind===null||Object.hasOwn(cpuTouchStyle,event.kind)))||
        raw.authoritative_game_events!==false||
        (s.video&&raw.video!==s.video))
        throw Error("Недостоверный индекс временной шкалы");
      timeline.data=raw;
      s.fps=Number(raw.fps)||0;
      if(!s.video)s.video=raw.video;
      ensurePreviewVideo();
      const attacks=raw.touch_annotations.filter(e=>e.kind==="attack").length;
      const blocks=raw.touch_annotations.filter(e=>e.kind==="block").length;
      timeline.status.textContent=raw.rallies.length+" розыгрышей · "+
        raw.touch_frames.length+" касаний (CPU) · "+
        attacks+" атак · "+blocks+" блоков";
      const duration=timelineDuration();
      timeline.middle.textContent="00:00.0";
      timeline.end.textContent=timeLabel(duration);
      drawTimeline();syncFavoritesSelection();drawOverlay();
      if(s.current){
        const selected=s.items.find(item=>item.candidate_id===s.current);
        if(selected){
          const start=Number(selected.manual_clip?.start_frame??
            selected.detected_start_frame??selected.start_frame??0);
          const end=Number(selected.manual_clip?.end_frame??
            selected.detected_end_frame??selected.end_frame??0);
          renderRallyTouches(selected,start,end);
        }
      }
    }catch(error){
      if(epoch!==timeline.epoch)return;
      timeline.data=null;drawTimeline();
      timeline.status.textContent="Шкала недоступна: "+String(error?.message||error);
    }
  };
  const rallyAtTimelineTime=seconds=>{
    const rows=timeline.data?.rallies||[];
    let left=0,right=rows.length;
    const frame=seconds*fps();
    while(left<right){
      const mid=(left+right)>>>1;
      if(rows[mid].start_frame<=frame)left=mid+1;else right=mid;
    }
    return left>0&&rows[left-1].end_frame>=frame?rows[left-1]:null;
  };
  const ensurePreviewVideo=()=>{
    if(!s.project||!s.video)return;
    const key=s.project+":"+s.video;
    if(s.videoKey===key)return;
    s.videoKey=key;
    ui.emptyVideo.classList.add("hidden");
    ui.video.src=url("/api/video-media",{project_id:s.project,video:s.video});
    ui.video.load();
  };
  const seekTimeline=seconds=>{
    if((!overlay.raw&&!timeline.data)||!s.video||(!overlay.raw&&!s.run))return;
    const duration=timelineDuration();
    if(!duration)return;
    const position=Math.max(0,Math.min(Math.max(0,duration-0.05),seconds));
    s.stopAt=null;s.clipStart=null;s.clipEnd=null;s.seekAt=position;
    timeline.playhead.style.left=(100*position/duration)+"%";
    if(s.current){s.current="";cards();}
    // Preserve player boxes and trajectory when seeking outside the selected card.
    if(!overlay.source)setOverlaySource({motion_events:[]});
    prefetchOverlayAtFrame(Math.floor(position*fps()));
    const rally=rallyAtTimelineTime(position);
    ui.viewTitle.textContent=rally?
      "Розыгрыш · "+timeLabel(rally.start_frame/fps())+" – "+
        timeLabel(rally.end_frame/fps()):
      "Просмотр полного видео";
    ui.viewDescription.textContent=rally?
      "Положение на общей шкале. Розыгрыш предварительный; касания не подтверждены.":
      "Просмотр исходного видео вне выделенного розыгрыша.";
    ui.videoTime.textContent=timeLabel(position)+" / "+timeLabel(duration);
    wipe(ui.events);ui.emptyVideo.classList.add("hidden");
    const key=s.project+":"+s.video;
    if(s.videoKey!==key){
      s.videoKey=key;
      ui.video.src=url("/api/video-media",{project_id:s.project,video:s.video});
      ui.video.load();
    }else if(ui.video.readyState>=1){
      try{ui.video.currentTime=position;}catch{}
    }
    drawTimeline();
  };
  const timelinePosition=event=>{
    const box=timeline.track.getBoundingClientRect();
    return box.width?Math.max(0,Math.min(1,(event.clientX-box.left)/box.width))*
      timelineDuration():0;
  };
  timeline.track.addEventListener("pointerdown",event=>{
    if(event.button!==0||(!timeline.data&&!overlay.raw))return;
    timeline.dragging=true;
    timeline.track.setPointerCapture?.(event.pointerId);
    seekTimeline(timelinePosition(event));
  });
  timeline.track.addEventListener("pointermove",event=>{
    if(timeline.dragging){seekTimeline(timelinePosition(event));return;}
    const row=rallyAtTimelineTime(timelinePosition(event));
    timeline.track.title=row?"Розыгрыш: "+
      timeLabel(row.start_frame/fps())+" – "+timeLabel(row.end_frame/fps()):
      "Нажмите для перехода к моменту видео";
  });
  for(const type of ["pointerup","pointercancel","lostpointercapture"]){
    timeline.track.addEventListener(type,()=>{timeline.dragging=false;});
  }
  timeline.track.addEventListener("keydown",event=>{
    const steps={PageDown:-30,PageUp:30};
    let target;
    if(event.key==="Home")target=0;
    else if(event.key==="End")target=timelineDuration();
    else if(Object.hasOwn(steps,event.key))
      target=(Number(ui.video.currentTime)||s.seekAt)+
        steps[event.key]*(event.shiftKey?0.2:1);
    else return;
    event.preventDefault();seekTimeline(target);
  });
  window.addEventListener("resize",drawTimeline);
  if(typeof ResizeObserver==="function"){
    new ResizeObserver(drawTimeline).observe(timeline.track);
  }
  // Pin the right rally rail to the video's lower edge, excluding the
  // playback controls and timeline. A flexible grid row alone grows with cards.
  const previewLayout=document.querySelector("#studio-preview-view .viewer-layout");
  const previewPanel=previewLayout?.querySelector(".viewer-panel");
  const previewFrame=previewPanel?.querySelector(".video-wrap");
  const previewHead=previewPanel?.querySelector(".viewer-head");
  const previewRail=previewLayout?.querySelector(".rallies-panel");
  const syncPreviewRailHeight=()=>{
    if(!previewPanel||!previewFrame||!previewRail)return;
    if(window.matchMedia("(max-width: 780px)").matches){
      previewRail.style.removeProperty("--preview-rally-height");
      return;
    }
    const panel=previewPanel.getBoundingClientRect();
    const frame=previewFrame.getBoundingClientRect();
    if(!frame.width||!frame.height)return;
    const next=Math.max(1,Math.round(frame.bottom-panel.top))+"px";
    if(previewRail.style.getPropertyValue("--preview-rally-height")!==next)
      previewRail.style.setProperty("--preview-rally-height",next);
  };
  window.addEventListener("resize",syncPreviewRailHeight);
  if(typeof ResizeObserver==="function"&&previewFrame){
    const railObserver=new ResizeObserver(syncPreviewRailHeight);
    railObserver.observe(previewFrame);
    if(previewHead)railObserver.observe(previewHead);
  }
  requestAnimationFrame(syncPreviewRailHeight);
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
        throw Error("Неверная версия модуля выбора фрагментов");
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
        if(this.exports.volley_selection_at(i)!==1)throw Error("Ошибка чтения выбранных фрагментов");
        const ptr=this.exports.volley_id_buffer_ptr();
        const text=this.decoder.decode(new Uint8Array(this.exports.memory.buffer,ptr,24));
        ids.push(text);
      }
      return ids.values();
    }
    values(){return Array.from(this.keys(),id=>this.labels.get(id)).values();}
  }
  const loadRustSelection=async()=>{
    await window.VolleyballUi.ready;
    s.selected=new RustSelection(window.VolleyballUi.engine);
    wasmReady=true;syncFavoritesSelection();cards();
  };
  const clearVideo=()=>{
    ui.video.pause();ui.video.removeAttribute("src");ui.video.load();
    s.videoKey="";s.current="";s.stopAt=null;s.clipStart=null;s.clipEnd=null;s.seekAt=0;
    setOverlaySource(null);
    ui.emptyVideo.classList.remove("hidden");
    ui.viewTitle.textContent="Розыгрыш не выбран";
    ui.viewDescription.textContent="Откройте розыгрыш для просмотра.";
    ui.videoTime.textContent="—";wipe(ui.events);
    updateTimelineProgress();
  };
  const stopPoll=()=>{if(s.timer)clearInterval(s.timer);s.timer=null;};
  // Persisted favorites are the only source of montage candidates. WebAssembly
  // remains the bounded (80 IDs) source-order selection engine, never a second
  // user-facing toggle. A timeline index supplies labels for all preview pages.
  const syncFavoritesSelection=()=>{
    if(!wasmReady)return;
    s.selected.clear();
    if(s.marksLoaded&&s.run&&s.gold.size<=80){
      const all=timeline.data?.project_id===s.project&&
        timeline.data?.rally_run_id===s.run?timeline.data.rallies:[];
      const indexed=new Map(all.map(rally=>[rally.candidate_id,rally]));
      const page=new Map(s.items.map(item=>[item.candidate_id,item]));
      const start=id=>Number(indexed.get(id)?.start_frame??
        page.get(id)?.detected_start_frame??NaN);
      const ordered=window.VolleyStudioChain.call(2,[...s.gold].map(id=>({id,start:Number.isFinite(start(id))?start(id):null})));
      for(const id of ordered){
        const rally=indexed.get(id),item=page.get(id);
        const first=start(id);
        const last=Number(rally?.end_frame??item?.detected_end_frame??NaN);
        s.selected.set(id,{
          time:Number.isFinite(first)?clock(first):"ID "+id.slice(0,8),
          duration:Number.isFinite(first)&&Number.isFinite(last)?
            (Math.max(0,last-first+1)/fps()).toFixed(1)+" с":"—"
        });
      }
    }
    renderSelection();
  };
  const renderSelection=()=>{
    wipe(ui.picked);
    const count=s.marksLoaded?s.gold.size:0;
    ui.selectedCount.textContent=count+" / 80";
    for(const item of s.selected.values()){
      ui.picked.append(make("span","picked",item.time+" · "+item.duration));
    }
    ui.selectionText.textContent=!s.marksLoaded?
      "Загрузка сохранённого избранного…":
      count>80?
        "В избранном "+count+" розыгрышей. Для одного монтажа допускается не больше 80 — уберите лишние отметки.":
      count?
        "В избранном "+count+" розыгрышей. Монтаж из оригинального видео в хронологическом порядке.":
        "Отмечайте розыгрыши как «Избранное» в предпросмотре — они появятся здесь для монтажа.";
    ui.render.disabled=!wasmReady||s.busy||!s.run||!s.marksLoaded||
      count===0||count>80||s.selected.size!==count;
  };
  const reset=()=>{
    s.generation++;s.run="";s.video="";s.items=[];s.total=0;s.filtered=0;s.offset=0;
    s.loadingMore=false;s.canLoadMore=true;s.pageLoading=false;s.pageError="";s.fps=0;
    s.gold=new Set();s.revision="absent";s.writable=false;s.marksLoaded=false;s.selected.clear();
    s.jobId="";stopPoll();clearVideo();clearTimeline();wipe(ui.list);
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
    const source=s.chain?.choices.find(row=>row.value===s.analysis);
    if(source?.video){
      s.video=source.video;
      s.fps=30;
      ensurePreviewVideo();
      if(overlay.raw)setOverlaySource({});
    }
    const rows=s.chain?.byAnalysis.get(s.analysis)||[];
    options(ui.run,rows,"rally_run_id",row=>
      (window.VolleyStudioChain.stamp(row.created_at)||"CPU "+row.rally_run_id.slice(0,8))+
      " · "+(row.display_name||((row.candidate_count||0)+" розыгрышей")),
      !s.analysis?"Сначала выберите анализ":
        rows.length?"Выберите постобработку":"Нет CPU-версий для анализа");
    ui.run.disabled=!s.analysis||!rows.length;
    ui.run.value=rows.some(row=>row.rally_run_id===preferred)?preferred:
      rows.length===1?rows[0].rally_run_id:"";
    refreshPicker(ui.run);
    s.run=ui.run.value;
    // Overlay playback follows the selected CPU snapshot, not a rally card.
    // reset() clears the canvas source; restore it for full-video preview too.
    setOverlaySource(overlay.raw||s.run?{}:null);
    if(s.run&&s.video)
      prefetchOverlayAtFrame(Math.max(0,Math.floor((ui.video.currentTime||0)*fps())));
    if(s.analysis)sourceSelection.save(s.project,s.analysis,s.run);
    window.dispatchEvent(new CustomEvent("volley:postprocessing-change",{
      detail:{project_id:s.project,rally_run_id:s.run}
    }));
    // SHA-verifying a long Touch file may take seconds. Never block the
    // already-resolved header and source chain on that independent operation.
    if(s.run){
      void loadPage().catch(failed);
      void loadTimeline().catch(failed);
    }
    else message(!s.analysis?"Выберите первичный GPU-анализ.":
      overlay.raw&&rawFusion()?"Сырые GPU-данные доступны без CPU-постобработки.":
      rows.length?"Выберите версию постобработки.":
      "У выбранного анализа пока нет связанной CPU-постобработки.");
    if(overlay.raw&&rawFusion())prefetchOverlayAtFrame(0);
  };
  const setAnalysis=async(preferredRun="")=>{
    s.analysis=ui.analysis.value;
    const source=s.chain?.choices.find(row=>row.value===s.analysis);
    window.dispatchEvent(new CustomEvent("volley:chain-change",{
      detail:{project_id:s.project,video:source?.video||"",analysis:s.analysis}
    }));
    await setRuns(preferredRun);
  };
  const chainSignature=chain=>JSON.stringify([
    chain.choices,[...chain.byAnalysis].map(([key,rows])=>[key,rows])
  ]);
  const renderChain=async(chain,preferredAnalysis="",preferredRun="",retainCurrent=false)=>{
    const unchanged=retainCurrent&&s.analysis===preferredAnalysis&&s.run===preferredRun&&
      chain.choices.some(row=>row.value===s.analysis)&&
      (!s.run||(chain.byAnalysis.get(s.analysis)||[]).some(row=>row.rally_run_id===s.run));
    s.chain=chain;
    options(ui.analysis,chain.choices,"value",item=>item.label,
      chain.choices.length?"Выберите первичный анализ":"Нет GPU-анализов");
    ui.analysis.disabled=!chain.choices.length;
    ui.analysis.value=chain.choices.some(row=>row.value===preferredAnalysis)?
      preferredAnalysis:chain.choices.length===1?chain.choices[0].value:"";
    refreshPicker(ui.analysis);
    if(unchanged){
      // Only labels and available versions changed. Do not discard the active
      // preview, playback position, marks or in-flight requests.
      const rows=chain.byAnalysis.get(s.analysis)||[];
      options(ui.run,rows,"rally_run_id",row=>
        (window.VolleyStudioChain.stamp(row.created_at)||"CPU "+row.rally_run_id.slice(0,8))+
        " · "+(row.display_name||((row.candidate_count||0)+" розыгрышей")),
        rows.length?"Выберите постобработку":"Нет CPU-версий для анализа");
      ui.run.disabled=!rows.length;
      ui.run.value=s.run;
      refreshPicker(ui.run);
      return;
    }
    // If the selected source was deleted (including dependent data), switch
    // to the remaining available versions using the normal reset flow.
    await setAnalysis(preferredRun);
  };
  const loadChain=async(preferredAnalysis="",preferredRun="",retainCurrent=false)=>{
    await window.VolleyballUi?.ready;
    const token=++s.chainEpoch,project=s.project;
    if(!retainCurrent){
      reset();
      s.chain=null;
      options(ui.analysis,[],"value",x=>x,"Загрузка анализов…");
      options(ui.run,[],"rally_run_id",x=>x,"Сначала выберите анализ");
      ui.analysis.disabled=true;ui.run.disabled=true;
    }
    if(!project){
      options(ui.analysis,[],"value",x=>x,"Сначала выберите проект");
      message("Выберите проект для просмотра цепочки обработок.");
      return;
    }
    const videos=s.projectVideos.slice(0,40);
    const cached=chainSnapshot.read(project,videos);
    if(cached){
      await renderChain(cached,retainCurrent?s.analysis:preferredAnalysis,
        retainCurrent?s.run:preferredRun,retainCurrent);
      if(token!==s.chainEpoch||s.project!==project)return;
    }else{
      message("Первичная загрузка списка GPU- и CPU-версий…");
    }
    try{
      const runs=await previewCache.load(
        url("/api/cpu/native/rallies/runs",{project_id:project}),300000,true);
      if(token!==s.chainEpoch||s.project!==project)return;
      const overviews=new Map();
      let unavailable=0;
      // Index requests are metadata-only, with a maximum of four in flight.
      for(let offset=0;offset<videos.length;offset+=4){
        const batch=await Promise.all(videos.slice(offset,offset+4).map(async video=>{
          try{
            const value=await previewCache.load(url("/api/studio/analysis-overview",{
              project_id:project,video
            }),300000,true);
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
      if(!builder?.build)throw Error("Не удалось загрузить связанную цепочку данных");
      const fresh=builder.build(videos,overviews,runs.runs);
      // A transient partial API failure should not erase the last usable
      // index. A later manual refresh retries the authoritative metadata.
      if(unavailable&&cached)return;
      chainSnapshot.save(project,videos,overviews,runs.runs);
      if(!s.chain||chainSignature(s.chain)!==chainSignature(fresh)){
        // Preserve edits to the current selection made while revalidating.
        await renderChain(fresh,retainCurrent?s.analysis:
          cached?(s.analysis||preferredAnalysis):preferredAnalysis,
          retainCurrent?s.run:cached?(s.run||preferredRun):preferredRun,
          retainCurrent);
      }
      if(unavailable)message("Не удалось прочитать стадии "+unavailable+
        " видео. Для части CPU-версий происхождение GPU не подтверждено.","error");
    }catch(error){
      if(!cached)throw error;
      console.warn("Обновление индекса обработок отложено:",error);
    }
  };
  const projects=async(preferredProject="")=>{
    const prev=s.project,selectedAnalysis=s.analysis,selectedRun=s.run;
    const stored=sourceSelection.read();
    const epoch=++s.catalogEpoch;
    const raw=await window.VolleyballStudioProjectCatalog();
    if(epoch!==s.catalogEpoch)return;
    const rows=(Array.isArray(raw.projects)?raw.projects:[]).
      filter(row=>typeof row.id==="string");
    s.projectCatalog=rows;
    options(ui.project,rows,"id",row=>row.name||row.title||row.id,"Выберите проект");
    ui.project.value=rows.some(row=>row.id===preferredProject)?preferredProject:
      rows.some(row=>row.id===prev)?prev:
      rows.some(row=>row.id===stored.project)?stored.project:
      rows.length===1?rows[0].id:"";
    refreshPicker(ui.project);
    s.project=ui.project.value;
    const current=rows.find(row=>row.id===s.project);
    s.projectVideos=Array.isArray(current?.videos)?
      current.videos.filter(video=>typeof video==="string"):[];
    window.dispatchEvent(new Event("volley:project-ready"));
    await loadChain(s.project===prev?selectedAnalysis:
      s.project===stored.project?stored.analysis:"",
      s.project===prev?selectedRun:
      s.project===stored.project?stored.run:"");
  };
  const loadMarks=async(token)=>{
    const raw=await previewCache.load(url("/api/cpu/native/rallies/marks",{
      project_id:s.project,rally_run_id:s.run
    }),7000);
    if(token!==s.generation)return;
    s.gold=new Set((Array.isArray(raw.candidate_ids)?raw.candidate_ids:[]).filter(valid));
    s.revision=typeof raw.revision==="string"?raw.revision:"absent";
    s.writable=raw.writable===true;s.marksLoaded=true;
    syncFavoritesSelection();drawTimeline();
  };
  const rallyListQuery=()=>{
    const choice=ui.filter.value;
    return {
      gold_filter:choice==="marked"||choice==="unmarked"?choice:"all",
      sort_by:choice==="duration_desc"||choice==="touches_desc"?choice:"time"
    };
  };
  const loadPage=async()=>{
    if(!s.project||!s.run)return;
    s.offset=0;s.loadingMore=false;s.canLoadMore=true;
    s.pageLoading=true;s.pageError="";
    const token=++s.generation;
    const path=url("/api/cpu/native/rallies/preview",{
      project_id:s.project,rally_run_id:s.run,offset:s.offset,limit:s.limit,
      ...rallyListQuery()
    });
    // Reopening a tab reuses the rendered list and the cached page silently.
    if(previewCache.peek(path)===null)
      ui.list.replaceChildren(make("div","empty-state","Проверка источников и загрузка розыгрышей…"));
    try{
      // Bookmarks have a separate revision and a separate metadata request.
      // A slow or unavailable bookmark endpoint must never block cached rally
      // cards. Apply the marks as soon as they arrive, without reloading video.
      void loadMarks(token).then(()=>{
        if(token===s.generation){cards();drawTimeline();}
      }).catch(error=>{
        if(token===s.generation)
          message("Избранное недоступно: "+String(error?.message||error),"error");
      });
      const raw=await previewCache.load(path,900000,true);
      if(token!==s.generation)return;
      if(raw.schema!=="volley-native-rally-preview/1"||
        raw.authoritative_game_events!==false||
        raw.automatic_montage_eligible!==false||
        !Array.isArray(raw.items))
        throw Error("Неподдерживаемая версия предварительного CPU-анализа");
      s.pageLoading=false;
      s.fps=Number(raw.fps)||0;
      s.items=Array.isArray(raw.items)?raw.items:[];
      s.total=Number(raw.total_candidates)||0;
      s.filtered=Number(raw.filtered_candidates??s.total)||0;
      s.canLoadMore=s.items.length<s.filtered;
      s.video=typeof raw.video==="string"?raw.video:"";
      ensurePreviewVideo();
      if(s.items.length&&previewIsVisible()){
        const first=Number(s.items[0].detected_start_frame);
        if(Number.isSafeInteger(first)&&first>=0)
          prefetchOverlayAtFrame(first);
      }
      syncFavoritesSelection();cards();
      if(!$("studio-preview-view").classList.contains("hidden"))
        message("CPU-данные загружены. Розыгрыши предварительные; проверьте их в видеоплеере.","ok");
    }catch(e){
      if(token!==s.generation)return;
      s.pageLoading=false;
      s.pageError=e instanceof Error?e.message:String(e);
      s.items=[];s.total=0;s.canLoadMore=false;cards();
      failed(e);
    }
  };
  const toggleGold=async id=>{
    if(!valid(id))return;
    if(!s.writable){
      message("Избранное можно сохранять только в рабочем хранилище.","error");return;
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
      invalidatePreviewRun(project,run);
      previewCache.drop(path=>path.startsWith("/api/cpu/native/rallies/marks?"));
      s.gold=new Set((Array.isArray(raw.candidate_ids)?raw.candidate_ids:[]).filter(valid));
      s.revision=String(raw.revision);s.marksLoaded=true;
      syncFavoritesSelection();drawTimeline();
      if(ui.filter.value==="marked"||ui.filter.value==="unmarked"){
        await loadPage();
      }else cards();
      message(marking?"Добавлено в избранное.":"Удалено из избранного.","ok");
    }catch(e){
      try{
        previewCache.drop(path=>path.startsWith("/api/cpu/native/rallies/marks?"));
        await loadMarks(s.generation);cards();
      }catch{}
      failed(e);
    }finally{s.busy=false;renderSelection();cards();}
  };
  // The *complete* server-side counts, not the 14-event card preview,
  // determine which action types are shown on a rally card.
  const estimatedTypes=item=>{
    const labels={attack:"Атака",block:"Блок",serve:"Подача",
      overhead_pass:"Передача сверху",dig:"Приём снизу",low_touch:"Нижнее касание"};
    const counts=item?.estimated_action_counts;
    const known=counts&&typeof counts==="object"&&!Array.isArray(counts)?
      Object.entries(labels).filter(([kind])=>
        Number.isSafeInteger(counts[kind])&&counts[kind]>0)
        .map(([kind,label])=>label+" ×"+counts[kind]):[];
    const unknown=item?.unclassified_action_count;
    if(Number.isSafeInteger(unknown)&&unknown>0)
      known.push("Без типа ×"+unknown);
    return known.length?"≈ "+known.join(" · "):"Типы не определены";
  };
  // Event pills and summary are derived from the same selected postprocessing
  // timeline as the canvas. The 14-event card sample is never used here.
  const renderRallyTouches=(item,start,end)=>{
    const data=timeline.data;
    const valid=data?.project_id===s.project&&data?.rally_run_id===s.run&&
      data?.video===s.video&&Array.isArray(data.touch_annotations);
    wipe(ui.events);
    if(!valid){
      ui.viewDescription.textContent=
        "Загрузка касаний выбранной CPU-постобработки…";
      return;
    }
    const touches=data.touch_annotations.filter(event=>
      event.frame_index>=start&&event.frame_index<=end);
    const labels={attack:"Атака",block:"Блок",serve:"Подача",
      overhead_pass:"Передача сверху",dig:"Приём снизу",low_touch:"Нижнее касание"};
    const counts=new Map();
    for(const touch of touches){
      const kind=touch.kind||"unclassified";
      counts.set(kind,(counts.get(kind)||0)+1);
    }
    const breakdown=Object.entries(labels)
      .filter(([kind])=>counts.has(kind))
      .map(([kind,label])=>label+" ×"+counts.get(kind));
    if(counts.has("unclassified"))
      breakdown.push("Без типа ×"+counts.get("unclassified"));
    ui.viewDescription.textContent="Фрагмент "+clock(start)+" – "+clock(end)+
      ". Касаний из CPU-постобработки: "+touches.length+
      ". Оценка типов: "+(breakdown.join(" · ")||"нет данных")+
      ". Классификация автоматическая, не подтверждена вручную.";
    for(const touch of touches.slice(0,32)){
      const label=touch.kind&&labels[touch.kind]?"≈ "+labels[touch.kind]:"Касание без типа";
      const pill=make("span","event-pill",clock(touch.frame_index)+" · "+label);
      pill.title="Результат выбранной CPU-постобработки. Автоматическая оценка.";
      if(cpuTouchStyle[touch.kind])
        pill.style.borderColor=cpuTouchStyle[touch.kind][0];
      ui.events.append(pill);
    }
  };
  const view=item=>{
    if(!s.project||!s.video)return;
    s.current=item.candidate_id;cards();
    // Full timeline entries may not belong to the currently loaded (or sorted) card page.
    const start=Number(item.manual_clip?.start_frame??item.detected_start_frame??item.start_frame??0);
    const end=Number(item.manual_clip?.end_frame??item.detected_end_frame??item.end_frame??0);
    const key=s.project+":"+s.video;
    s.clipStart=start/fps();s.clipEnd=(end+1)/fps();
    s.stopAt=s.playbackMode==="free"?null:s.clipEnd;
    ui.viewTitle.textContent="Розыгрыш · "+clock(start)+" – "+clock(end);
    ui.videoTime.textContent=clock(start)+" – "+clock(end);
    renderRallyTouches(item,start,end);
    ui.emptyVideo.classList.add("hidden");
    s.seekAt=s.clipStart;
    setOverlaySource(item);
    prefetchOverlayAtFrame(start);
    drawTimeline();
    if(s.videoKey!==key){
      s.videoKey=key;
      ui.video.src=url("/api/video-media",{project_id:s.project,video:s.video});
      ui.video.load();
    }else if(ui.video.readyState>=1){
      try{ui.video.currentTime=s.seekAt;}catch{}
    }
    // A card click starts playback only while the preview is visible.
    // Metadata/seek is restored by loadedmetadata without background playback.
    if(previewIsVisible())ui.video.play().catch(error=>
      message("Воспроизведение недоступно: "+error.message,"error"));
  };
  const cards=()=>{
    const previousScroll=ui.list.scrollTop;
    wipe(ui.list);
    const visible=s.items;
    if(!visible.length){
      const text=s.pageLoading?"Проверка источников и загрузка розыгрышей…":
        s.pageError?"Предпросмотр не загружен: "+s.pageError+
          " · Выбранная CPU-версия не удалена.":
        !s.run?(overlay.raw&&rawFusion()?
          "Сырые GPU-данные доступны в плеере. Розыгрыши появятся после CPU-постобработки.":
          "Выберите версию CPU-постобработки.") :
        ui.filter.value==="marked"&&s.filtered===0?
          "В этой версии пока нет избранных розыгрышей. Выберите «Все».":
        ui.filter.value==="unmarked"&&s.filtered===0?
          "Все розыгрыши этой версии добавлены в избранное. Выберите «Все».":
        s.total===0?"В выбранной CPU-версии нет обнаруженных розыгрышей.":
        "Не удалось отобразить карточки этой CPU-версии.";
      ui.list.append(make("div","empty-state",text));
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
      const idLabel=make("span","id-label","ID "+id.slice(0,6));
      idLabel.title="Полный ID: "+id;
      const duration=make("div","rally-card-duration");
      duration.title="Длительность: "+length(item);
      duration.append(make("span","","Длительность"),make("strong","",length(item)));
      top.append(idLabel);
      const when=make("div","rally-card-time-range");
      when.append(make("strong","rally-card-time-value",
        second(item.detected_start_frame)+"–"+second(item.detected_end_frame)+" с"));
      when.title="На видео: "+second(item.detected_start_frame)+"–"+
        second(item.detected_end_frame)+" с";
      top.append(when);
      const stats=make("div","card-stats");
      const touches=make("div","rally-card-stat");
      const count=Number.isSafeInteger(item.motion_event_count)?
        item.motion_event_count:item.motion_supported_touch_proposals;
      touches.append(make("span","","Касаний"),
        make("strong","",Number.isSafeInteger(count)&&count>=0?count:"—"));
      const players=make("div","rally-card-stat");
      const countPlayers=touchingPlayers(item);
      players.title="Число уникальных Track ID с обнаруженными касаниями, не всех игроков в кадре";
      players.append(make("span","","Игроков"),
        make("strong","",countPlayers===null?"—":countPlayers));
      stats.append(duration,touches,players);
      const bottom=make("div","card-bottom");
      const favorite=make("button","gold-toggle",gold?"★":"☆");
      favorite.type="button";
      favorite.title=gold?"Убрать из избранного":"Добавить в избранное";
      favorite.setAttribute("aria-pressed",String(gold));
      favorite.setAttribute("aria-label",favorite.title);
      favorite.disabled=!s.writable||s.busy;
      favorite.addEventListener("click",e=>{e.stopPropagation();toggleGold(id);});
      const predictions=(Array.isArray(item.motion_events)?item.motion_events:[])
        .map(event=>event.rally_context ? event.rally_context.estimated_kind : event.action_prediction?.kind).filter(kind=>typeof kind==="string");
      if(predictions.length)card.title="Автоклассификация (предварительная): "+
        [...new Set(predictions)].map(kind=>kind.replaceAll("_"," ")).join(", ");
      const foot=make("div","rally-card-foot");
      bottom.append(favorite);
      foot.append(stats,bottom);
      const types=make("div","rally-card-techniques",estimatedTypes(item));
      types.title="Автоматические оценки по всем кандидатам касаний в розыгрыше; не подтверждены человеком";
      card.append(top,foot,types);ui.list.append(card);
    }
    ui.list.scrollTop=previousScroll;
  };
  // Fetch next rally pages when the right-hand list is scrolled to its end.
  const loadMore=async()=>{
    if(s.loadingMore||!s.canLoadMore||!s.project||!s.run)return;
    s.loadingMore=true;
    const token=s.generation,project=s.project,run=s.run;
    const offset=s.items.length;
    try{
      const raw=await previewCache.load(url("/api/cpu/native/rallies/preview",{
        project_id:project,rally_run_id:run,offset,limit:s.limit,
        ...rallyListQuery()
      }),900000,true);
      if(token!==s.generation||project!==s.project||run!==s.run)return;
      if(raw.schema!=="volley-native-rally-preview/1"||
        raw.authoritative_game_events!==false||
        raw.automatic_montage_eligible!==false||
        !Array.isArray(raw.items))
        throw Error("Неподдерживаемая версия предварительного CPU-анализа");
      s.items.push(...raw.items);
      s.canLoadMore=raw.items.length>0&&s.items.length<s.filtered;
      cards();
      drawTimeline();
    }catch(error){
      if(token===s.generation)
        message("Не удалось догрузить розыгрыши: "+String(error?.message||error),"error");
    }finally{s.loadingMore=false;}
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
    if(!wasmReady||s.busy||!s.run||!s.marksLoaded||!s.gold.size||
      s.gold.size>80||s.selected.size!==s.gold.size)return;
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
      message("Сборка избранных розыгрышей из оригинального видео.","ok");
      stopPoll();await updateJob();
      if(s.jobId&&!ui.cancel.classList.contains("hidden")){
        s.timer=setInterval(updateJob,1500);
      }
    }catch(e){failed(e);ui.job.textContent="Монтаж не запущен.";ui.render.disabled=false;}
    finally{s.busy=false;}
  };
  // The project dialog accepts browser-owned drop/chooser uploads and athlete links.
  // Both use the existing Rust API and isolated storage; never infer local file paths.
  if(ui.projectNew){
    let creating=false,uploading=false,addingPlayer=false,activeUpload=null;
    let playerRevision="absent",dialogEpoch=0;
    const maxVideoBytes=window.VolleyballUpload.maxVideoBytes;
    const selectedVideos=()=>[...ui.projectVideos.querySelectorAll("input[type=checkbox]:checked")]
      .map(input=>input.value);
    const selectedPlayers=()=>[...ui.projectPlayers.querySelectorAll("input[type=checkbox]:checked")]
      .map(input=>input.value);
    const updateSubmit=()=>{
      const count=selectedVideos().length;
      ui.projectSubmit.disabled=creating||uploading||!ui.projectName.value.trim()||
        count<1||count>30;
      ui.projectDrop.setAttribute("aria-busy",String(uploading));
      ui.projectDrop.classList.toggle("uploading",uploading);
    };
    const alertCreate=error=>{
      ui.projectCreateError.textContent=String(error?.message||error);
    };
    const makeChoice=(row,chosen)=>{
      const label=make("label","studio-project-video-option");
      label.title=String(row.display_name||row.name);
      const box=document.createElement("input");
      box.type="checkbox";box.value=row.name;box.checked=chosen;
      box.addEventListener("change",updateSubmit);
      label.append(box,make("span","",
        (row.display_name||row.name)+(row.origin==="uploaded"?" · загружено":" · локально")+
        (Number.isFinite(row.size_gb)?" · "+row.size_gb+" ГБ":"")));
      return label;
    };
    const refreshVideos=async(autoChecked=new Set(),token=dialogEpoch)=>{
      const previous=new Set(selectedVideos());
      const raw=await request("/api/videos");
      if(!ui.projectDialog.open||token!==dialogEpoch)return;
      const list=(Array.isArray(raw.videos)?raw.videos:[]).filter(row=>
        typeof row.name==="string"&&row.name.length>0);
      ui.projectVideos.replaceChildren();
      for(const row of list){
        ui.projectVideos.append(makeChoice(row,previous.has(row.name)||autoChecked.has(row.name)));
      }
      if(!list.length)ui.projectVideos.append(make("p","panel-note",
        "Пока нет видео. Перетащите MP4 сюда или выберите его через проводник."));
      updateSubmit();
    };
    const renderPlayers=(players,selected)=>{
      ui.projectPlayers.replaceChildren();
      if(!players.length){
        ui.projectPlayers.append(make("p","panel-note","Добавьте первого игрока ниже."));
      }
      for(const player of players){
        if(typeof player.id!=="string"||typeof player.name!=="string")continue;
        const label=make("label","studio-project-video-option");
        label.title=player.name;
        const checkbox=document.createElement("input");
        checkbox.type="checkbox";checkbox.value=player.id;
        checkbox.checked=selected.has(player.id);
        label.append(checkbox,make("span","",player.name));
        ui.projectPlayers.append(label);
      }
    };
    const refreshPlayers=async(extraChecked=new Set(),token=dialogEpoch)=>{
      const selected=new Set([...selectedPlayers(),...extraChecked]);
      const raw=await post("/api/players/library/native",{action:"read"});
      if(token!==dialogEpoch||!ui.projectDialog.open)return;
      playerRevision=String(raw.revision||"absent");
      renderPlayers(Array.isArray(raw.players)?raw.players:[],selected);
    };
    const loadDialog=async()=>{
      if(creating||uploading)return;
      const token=++dialogEpoch;
      ui.createForm.reset();
      playerRevision="absent";
      ui.projectCreateError.textContent="";
      ui.projectUploadStatus.textContent="";
      ui.projectVideos.replaceChildren(make("p","panel-note","Загрузка видео…"));
      ui.projectPlayers.replaceChildren(make("p","panel-note","Загрузка игроков…"));
      ui.projectSubmit.disabled=true;
      ui.projectDialog.showModal();
      ui.projectName.focus();
      const results=await Promise.allSettled([
        refreshVideos(new Set(),token),refreshPlayers(new Set(),token)
      ]);
      if(token!==dialogEpoch)return;
      for(const result of results)if(result.status==="rejected")alertCreate(result.reason);
    };
    const closeDialog=()=>{
      if(creating)return;
      dialogEpoch++;
      if(activeUpload)activeUpload.abort();
      ui.projectDialog.close();
    };
    const uploadOne=async(file,sha256)=>{
      const transfer=window.VolleyballUpload.start("/api/project-source/upload",file,{
        headers:{"X-Content-SHA256":sha256,"X-Content-Size":file.size},
        progress:percent=>{
          if(ui.projectDialog.open)ui.projectUploadStatus.textContent=
            "Загрузка «"+file.name+"»: "+(percent===null?"…":percent+"%");
        }
      });
      activeUpload=transfer;
      try{
        const result=await transfer.promise;
        if(typeof result.name!=="string")throw Error("Сервер не подтвердил загрузку видео");
        return result;
      }finally{if(activeUpload===transfer)activeUpload=null;}
    };
    const addDroppedFiles=async(files)=>{
      if(uploading||creating||!ui.projectDialog.open)return;
      const inputs=Array.from(files||[]);
      if(!inputs.length)return;
      if(inputs.length>30||selectedVideos().length+inputs.length>30){
        alertCreate("Максимум 30 видео в одном проекте.");return;
      }
      const encode=new TextEncoder();
      for(const file of inputs){
        if(!/\.(mp4|mov|mkv|avi|webm|m4v|mts)$/i.test(file.name)||
          encode.encode(file.name).length>180||file.size<1024||file.size>maxVideoBytes){
          alertCreate("Неверный формат, имя или размер: "+file.name+" (до 32 ГиБ)");
          return;
        }
      }
      const epoch=dialogEpoch,added=new Set();
      let uploadedCount=0,reusedCount=0;
      uploading=true;ui.projectCreateError.textContent="";updateSubmit();
      try{
        for(const file of inputs){
          if(epoch!==dialogEpoch)break;
          const sha256=await window.VolleyballSha256.digest(file,percent=>{
            if(epoch===dialogEpoch)ui.projectUploadStatus.textContent=
              "SHA-256 «"+file.name+"»: "+percent+"%";
          },()=>epoch!==dialogEpoch);
          if(epoch!==dialogEpoch)break;
          const existing=await request("/api/project-source/lookup?"+
            new URLSearchParams({sha256,size:String(file.size)}));
          if(epoch!==dialogEpoch)break;
          const result=existing.duplicate?existing:await uploadOne(file,sha256);
          if(!result?.name)throw Error("Сервер не вернул имя видео");
          added.add(result.name);
          if(result.duplicate)reusedCount++;else uploadedCount++;
          ui.projectUploadStatus.textContent=result.duplicate?
            "Совпадение SHA-256: «"+file.name+"» уже есть, загрузка пропущена.":
            "Загружено: "+file.name;
        }
      }catch(error){
        if(epoch===dialogEpoch)alertCreate(error);
      }finally{
        uploading=false;activeUpload=null;
        if(epoch===dialogEpoch&&ui.projectDialog.open){
          await refreshVideos(added,epoch).catch(alertCreate);
          ui.projectUploadStatus.textContent=added.size?
            "Новых: "+uploadedCount+" · Повторно использовано по SHA-256: "+reusedCount:"";
          updateSubmit();
        }
      }
    };
    ui.projectNew.addEventListener("click",()=>loadDialog().catch(failed));
    ui.projectClose.addEventListener("click",closeDialog);
    ui.projectCancel.addEventListener("click",closeDialog);
    ui.projectDialog.addEventListener("cancel",event=>{event.preventDefault();closeDialog();});
    ui.projectName.addEventListener("input",updateSubmit);
    ui.projectDrop.addEventListener("click",()=>{
      if(uploading)return;
      ui.projectFiles.value="";
      ui.projectFiles.click();
    });
    ui.projectDrop.addEventListener("keydown",event=>{
      if(event.key==="Enter"||event.key===" "){
        event.preventDefault();ui.projectDrop.click();
      }
    });
    ui.projectFiles.addEventListener("change",()=>{
      const files=Array.from(ui.projectFiles.files||[]);
      ui.projectFiles.value="";
      addDroppedFiles(files).catch(alertCreate);
    });
    ui.projectDrop.addEventListener("dragenter",event=>{
      event.preventDefault();ui.projectDrop.classList.add("drag-over");
    });
    ui.projectDrop.addEventListener("dragover",event=>{
      event.preventDefault();if(event.dataTransfer)event.dataTransfer.dropEffect="copy";
      ui.projectDrop.classList.add("drag-over");
    });
    ui.projectDrop.addEventListener("dragleave",()=>{
      ui.projectDrop.classList.remove("drag-over");
    });
    ui.projectDrop.addEventListener("drop",event=>{
      event.preventDefault();ui.projectDrop.classList.remove("drag-over");
      addDroppedFiles(event.dataTransfer?.files).catch(alertCreate);
    });
    ui.projectAddPlayer.addEventListener("click",async()=>{
      const name=ui.projectPlayerName.value.trim();
      if(!name||addingPlayer||creating)return;
      addingPlayer=true;ui.projectAddPlayer.disabled=true;
      ui.projectCreateError.textContent="";
      const epoch=dialogEpoch;
      try{
        const created=await post("/api/players/library/native",{
          action:"create",name,expected_revision:playerRevision
        });
        if(epoch!==dialogEpoch)return;
        playerRevision=String(created.revision);
        renderPlayers(Array.isArray(created.players)?created.players:[],
          new Set([...selectedPlayers(),created.player_id]));
        ui.projectPlayerName.value="";
        window.dispatchEvent(new Event("volley:player-library-updated"));
      }catch(error){
        if(epoch===dialogEpoch){
          alertCreate(error);
          await refreshPlayers(new Set(),epoch).catch(()=>{});
        }
      }finally{
        addingPlayer=false;ui.projectAddPlayer.disabled=false;
      }
    });
    ui.projectPlayerName.addEventListener("keydown",event=>{
      if(event.key==="Enter"){event.preventDefault();ui.projectAddPlayer.click();}
    });
    ui.createForm.addEventListener("submit",async event=>{
      event.preventDefault();
      if(creating||uploading)return;
      const name=ui.projectName.value.trim();
      const videos=selectedVideos(),player_ids=selectedPlayers();
      if(!name||!videos.length||videos.length>30){updateSubmit();return;}
      creating=true;updateSubmit();
      ui.projectCancel.disabled=true;ui.projectClose.disabled=true;
      ui.projectCreateError.textContent="";
      try{
        const created=await post("/api/project-create",{name,videos,player_ids});
        const id=created.project?.id;
        if(typeof id!=="string"||!id)
          throw Error("Проект создан, но его ID не получен. Обновите список.");
        ui.projectDialog.close();
        dialogEpoch++;
        invalidateCatalog();
        await projects(id);
        document.getElementById("studio-nav-analysis").click();
        message("Проект «"+name+"» создан и выбран.","ok");
      }catch(error){
        alertCreate(error);
      }finally{
        creating=false;
        ui.projectCancel.disabled=false;ui.projectClose.disabled=false;
        updateSubmit();
      }
    });
  }
  ui.project.addEventListener("change",async()=>{
    s.catalogEpoch++;
    s.project=ui.project.value;s.analysis="";
    sourceSelection.save(s.project);
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
    sourceSelection.save(s.project,s.analysis,s.run);
    window.dispatchEvent(new CustomEvent("volley:postprocessing-change",{
      detail:{project_id:s.project,rally_run_id:s.run}
    }));
    void loadTimeline().catch(failed);
    try{await loadPage();}catch(e){failed(e);}
  });
  window.addEventListener("volley:refresh-projects",()=>{invalidateCatalog();projects().catch(failed);});
  window.addEventListener("volley:refresh-chain",()=>{invalidateCatalog();projects().catch(failed);});
  // One authoritative place to select the new GPU -> CPU lineage: header selects.
  window.addEventListener("volley:pipeline-complete",async event=>{
    const {project_id,video,analysis_id,postprocessing_id}=event.detail||{};
    if(project_id!==s.project||typeof analysis_id!=="string")return;
    try{
      invalidateCatalog();
      await loadChain();
      if(s.project!==project_id)return;
      const choice=s.chain?.choices.find(item=>item.kind==="fusion"&&
        item.id===analysis_id&&item.video===video);
      if(!choice)throw Error("Новая GPU-версия не найдена в индексе стадий");
      ui.analysis.value=choice.value;
      await setAnalysis(typeof postprocessing_id==="string"?postprocessing_id:"");
      if(typeof postprocessing_id==="string"&&s.run!==postprocessing_id)
        throw Error("Новая CPU-постобработка не привязана к GPU-версии");
    }catch(e){failed(e);}
  });
  window.addEventListener("volley:open-legacy-project",async event=>{
    const id=event.detail?.project_id;
    if(typeof id!=="string"||!/^[a-f0-9]{12}$/.test(id))return;
    try{
      await projects();
      const has=[...ui.project.options].some(row=>row.value===id);
      if(!has)throw Error("Проект не найден в текущем реестре");
      ui.project.value=id;ui.project.dispatchEvent(new Event("change"));
      document.getElementById("studio-nav-preview").click();
    }catch(e){failed(e);}
  });
  ui.filter.addEventListener("change",()=>{
    ui.list.scrollTop=0;
    void loadPage().catch(failed);
  });
  ui.list.addEventListener("scroll",()=>{
    if(ui.list.scrollTop+ui.list.clientHeight>=ui.list.scrollHeight-240)
      void loadMore();
  },{passive:true});
  ui.render.addEventListener("click",startMontage);
  ui.cancel.addEventListener("click",async()=>{
    if(!s.jobId)return;
    try{
      await post("/api/montage/native/cancel",{job_id:s.jobId});
      message("Отмена отправлена.","ok");await updateJob();
    }catch(e){failed(e);}
  });
  ui.video.addEventListener("loadedmetadata",()=>{
    if(s.videoKey)try{
      const duration=ui.video.duration;
      ui.video.currentTime=Number.isFinite(duration)&&duration>0?
        Math.min(s.seekAt,Math.max(0,duration-0.05)):s.seekAt;
    }catch{}
    const duration=timelineDuration();
    if(duration){
      timeline.middle.textContent="00:00.0";
      timeline.end.textContent=timeLabel(duration);
    }
    drawTimeline();
  });
  const nextRally=()=>{
    // The full CPU timeline is authoritative; filtered/paginated cards are only a fallback.
    const rows=timeline.data?.project_id===s.project&&
      timeline.data?.rally_run_id===s.run?timeline.data.rallies:s.items;
    const start=row=>Number(row.manual_clip?.start_frame??
      row.detected_start_frame??row.start_frame??Infinity);
    const ordered=[...rows].filter(row=>Number.isFinite(start(row)))
      .sort((a,b)=>start(a)-start(b));
    const index=ordered.findIndex(row=>row.candidate_id===s.current);
    const row=index>=0?ordered[index+1]:
      ordered.find(entry=>start(entry)/fps()>=(s.clipEnd??ui.video.currentTime)-0.02);
    return row?(s.items.find(item=>item.candidate_id===row.candidate_id)||row):null;
  };
  const onRallyBoundary=()=>{
    if(s.playbackMode==="free"){s.stopAt=null;return;}
    if(s.playbackMode==="loop"&&s.clipStart!==null){
      s.seekAt=s.clipStart;
      s.stopAt=s.clipEnd;
      ui.video.currentTime=s.clipStart;
      if(ui.video.paused)ui.video.play().catch(error=>
        message("Воспроизведение недоступно: "+error.message,"error"));
      return;
    }
    if(s.playbackMode==="sequence"){
      const next=nextRally();
      if(next){view(next);return;}
    }
    s.stopAt=null;
    ui.video.pause();
  };
  ui.video.addEventListener("timeupdate",()=>{
    updateTimelineProgress();
    if(s.stopAt!==null&&ui.video.currentTime>=s.stopAt)
      onRallyBoundary();
  });
  // Media can end before the rounded final-frame boundary; loop must still repeat.
  ui.video.addEventListener("ended",()=>{
    if(s.stopAt!==null&&s.playbackMode!=="free")onRallyBoundary();
  });
  ui.video.addEventListener("error",()=>{
    overlay.presentedTime=null;overlayContext();
    if(s.videoKey)overlayMessage("Видео не декодируется (код "+
      (ui.video.error?.code||"?")+"). Нажмите «Воспроизвести» для повторной загрузки.");
    if(s.videoKey)ui.viewDescription.textContent=
      "Видео недоступно для воспроизведения. Проверьте подготовку видео.";
  });
  // Native controls are intentionally disabled: the source-wide annotated
  // timeline is the ONE seek slider. Keep transport controls accessible.
  const playback={
    play:$("studio-video-play"),mute:$("studio-video-mute"),
    fullscreen:$("studio-video-fullscreen"),mode:$("studio-video-mode"),
    wrap:ui.video.closest(".video-wrap")
  };
  try{
    const mode=localStorage.getItem("volley:studio-playback-mode:v1");
    if(["sequence","loop","free","once"].includes(mode))playback.mode.value=mode;
  }catch{}
  s.playbackMode=playback.mode.value;
  const updatePlaybackControls=()=>{
    const playing=!ui.video.paused&&!ui.video.ended;
    playback.play.disabled=!s.videoKey;
    playback.play.textContent=playing?"❚❚ Пауза":"▶ Воспроизвести";
    playback.play.setAttribute("aria-label",playing?"Пауза":"Воспроизвести");
    playback.mute.disabled=!s.videoKey;
    playback.mute.textContent=ui.video.muted?"🔇 Без звука":"♫ Звук";
    playback.mute.setAttribute("aria-pressed",String(ui.video.muted));
    playback.fullscreen.disabled=!s.videoKey;
    playback.fullscreen.textContent=document.fullscreenElement?"⛶ Свернуть":"⛶ На весь экран";
  };
  const toggleVideoPlayback=()=>{
    if(!s.videoKey||!previewIsVisible())return;
    if(ui.video.paused){
      if(ui.video.error){
        // A failed byte-range request must not permanently poison this player.
        overlay.presentedTime=null;
        ui.video.load();
      }
      // Replay a completed selection from its beginning, not a fraction past its end.
      if(s.playbackMode!=="free"&&s.clipStart!==null&&s.clipEnd!==null&&
         (ui.video.ended||ui.video.currentTime>=s.clipEnd-0.04)){
        s.seekAt=s.clipStart;s.stopAt=s.clipEnd;
        try{ui.video.currentTime=s.clipStart;}catch{}
      }
      ui.video.play().catch(error=>message("Воспроизведение недоступно: "+error.message,"error"));
    }else ui.video.pause();
  };
  playback.mode.addEventListener("change",()=>{
    s.playbackMode=playback.mode.value;
    s.stopAt=s.playbackMode==="free"?null:s.clipEnd;
    try{localStorage.setItem("volley:studio-playback-mode:v1",s.playbackMode);}catch{}
    if(s.stopAt!==null&&ui.video.currentTime>=s.stopAt&&s.clipStart!==null){
      s.seekAt=s.clipStart;
      try{ui.video.currentTime=s.seekAt;}catch{}
    }
  });
  playback.play.addEventListener("click",toggleVideoPlayback);
  playback.mute.addEventListener("click",()=>{
    ui.video.muted=!ui.video.muted;
    updatePlaybackControls();
  });
  playback.fullscreen.addEventListener("click",async()=>{
    try{
      if(document.fullscreenElement)await document.exitFullscreen();
      else await playback.wrap.requestFullscreen();
    }catch(error){message("Полноэкранный режим недоступен: "+error.message,"error");}
    updatePlaybackControls();
  });
  ui.video.addEventListener("click",toggleVideoPlayback);
  // Left/right arrows scrub the one source-wide timeline, including raw GPU.
  // Rust/WASM owns the precise 0.5 s tap and exponential hold acceleration.
  // JS only adapts keyboard events, requestAnimationFrame and media seeking.
  const arrowScrub={direction:0,startedAt:0,lastSeekAt:0,frame:0};
  const stopArrowScrub=()=>{
    arrowScrub.direction=0;
    if(arrowScrub.frame)cancelAnimationFrame(arrowScrub.frame);
    arrowScrub.frame=0;
  };
  const tickArrowScrub=timestamp=>{
    arrowScrub.frame=0;
    if(!arrowScrub.direction)return;
    if(!previewIsVisible()||s.videoKey!==s.project+":"+s.video||
       !timelineDuration()){stopArrowScrub();return;}
    if(timestamp-arrowScrub.lastSeekAt>=85){
      const delta=window.VolleyballUi.engine.volley_preview_seek_hold_delta(
        arrowScrub.direction,timestamp-arrowScrub.startedAt,
        timestamp-arrowScrub.lastSeekAt);
      if(delta)seekTimeline(s.seekAt+delta);
      arrowScrub.lastSeekAt=timestamp;
    }
    arrowScrub.frame=requestAnimationFrame(tickArrowScrub);
  };
  document.addEventListener("keydown",event=>{
    if(event.key!=="ArrowLeft"&&event.key!=="ArrowRight")return;
    if(event.altKey||event.ctrlKey||event.metaKey||event.isComposing||
       ["INPUT","TEXTAREA","SELECT"].includes(event.target?.tagName)||
       event.target?.isContentEditable||!s.videoKey||!previewIsVisible()||
       s.videoKey!==s.project+":"+s.video||!timelineDuration())return;
    event.preventDefault();
    event.stopPropagation(); // No duplicate seek from the focused timeline slider.
    const direction=event.key==="ArrowRight"?1:-1;
    if(arrowScrub.direction===direction)return;
    stopArrowScrub();
    arrowScrub.direction=direction;
    arrowScrub.startedAt=performance.now();
    arrowScrub.lastSeekAt=arrowScrub.startedAt;
    const current=Number.isFinite(ui.video.currentTime)?
      ui.video.currentTime:s.seekAt;
    seekTimeline(current+window.VolleyballUi.engine.volley_preview_seek_tap(direction));
    arrowScrub.frame=requestAnimationFrame(tickArrowScrub);
  },true);
  document.addEventListener("keyup",event=>{
    if((event.key==="ArrowLeft"&&arrowScrub.direction===-1)||
       (event.key==="ArrowRight"&&arrowScrub.direction===1))
      stopArrowScrub();
  },true);
  window.addEventListener("blur",stopArrowScrub);
  document.addEventListener("visibilitychange",()=>{
    if(document.hidden)stopArrowScrub();
  });
  // Space controls the player from anywhere on the page, except editable controls.
  // Capture prevents a focused card/button from also processing the same keypress.
  document.addEventListener("keydown",event=>{
    if(event.code!=="Space"&&event.key!==" ")return;
    if(event.repeat||event.altKey||event.ctrlKey||event.metaKey)return;
    if(["INPUT","TEXTAREA","SELECT"].includes(event.target?.tagName)||
       event.target?.isContentEditable||!s.videoKey||!previewIsVisible())return;
    event.preventDefault();
    event.stopPropagation();
    toggleVideoPlayback();
  },true);
  ui.video.addEventListener("keydown",event=>{
    if(event.key==="k"||event.key==="K"){
      event.preventDefault();toggleVideoPlayback();
    }
  });
  for(const type of ["play","pause","ended","loadedmetadata","emptied","volumechange"]){
    ui.video.addEventListener(type,updatePlaybackControls);
  }
  document.addEventListener("fullscreenchange",updatePlaybackControls);
  updatePlaybackControls();
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
  request("/api/studio/analysis-capabilities").then(value=>{
    if(value.analysis_edit){makeAnalysisPicker(ui.analysis,true);makeAnalysisPicker(ui.run,false);}
  }).catch(()=>{});
  makeAnalysisPicker(ui.project,"project");
  loadRustSelection().catch(e=>{
    wasmReady=false;
    failed(new Error("Модуль выбора фрагментов недоступен: "+String(e.message||e)));
    renderSelection();cards();
  });
  projects().catch(failed);
})();
