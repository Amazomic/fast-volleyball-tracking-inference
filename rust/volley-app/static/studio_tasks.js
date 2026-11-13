"use strict";
// Read-only active-job strip. The browser owns visibility; Rust owns progress.
// CPU endpoints currently run synchronously, so their bar is indeterminate.
(() => {
  const $=id=>document.getElementById(id);
  const root=$("studio-sidebar-tasks"),list=$("studio-sidebar-task-list");
  if(!root||!list)return;
  const tasks=new Map(),rows=new Map();
  let pollTimer=null;
  const labels={analysis:"Полный анализ",gpu:"Анализ GPU",cpu:"Постобработка CPU",quick:"Быстрый рилс"};
  let trackedGpuProject="",trackedAnalysisProject="",polling=false;
  const clamp=p=>Math.max(0,Math.min(100,p));
  const create=(tag,css,text)=>{
    const el=document.createElement(tag);
    if(css)el.className=css;
    if(text!==undefined)el.textContent=text;
    return el;
  };
  function render(){
    const active=[...tasks.entries()].filter(([,task])=>task.active);
    window.VolleyballStudioActivity?.setTaskActive?.("studio-job",active.length>0);
    root.classList.toggle("hidden",active.length===0);
    if(!active.length){
      list.replaceChildren();
      rows.clear();
      return;
    }
    const ordered=active.sort(([a],[b])=>{
      const order=key=>key.startsWith("gpu")?0:key.startsWith("cpu")?1:2;
      return order(a)-order(b);
    });
    for(const [key,task] of ordered){
      let item=rows.get(key);
      if(!item){
        const el=create("div","studio-sidebar-task");
        const head=create("div","studio-sidebar-task-head");
        const name=create("strong","studio-sidebar-task-name");
        const percent=create("span","studio-sidebar-task-percent");
        const stage=create("div","studio-sidebar-task-stage");
        const rail=create("div","studio-sidebar-task-rail");
        rail.setAttribute("role","progressbar");
        rail.setAttribute("aria-valuemin","0");
        rail.setAttribute("aria-valuemax","100");
        const fill=create("span","studio-sidebar-task-fill");
        rail.append(fill);
        head.append(name,percent);
        el.append(head,stage,rail);
        item={el,name,percent,stage,rail,fill};
        rows.set(key,item);
      }
      item.name.textContent=task.label||labels[key]||"Обработка";
      item.stage.textContent=task.stage||"Выполняется";
      const pct=Number(task.progress);
      const measured=task.progress!==null&&task.progress!==undefined&&Number.isFinite(pct);
      item.rail.classList.toggle("is-indeterminate",!measured);
      if(measured){
        const amount=clamp(pct);
        item.fill.style.width=amount+"%";
        item.percent.textContent=amount.toFixed(0)+"%";
        item.rail.setAttribute("aria-valuenow",String(amount));
        item.rail.setAttribute("aria-valuetext",amount.toFixed(0)+"%");
      }else{
        item.fill.style.width="40%";
        item.percent.textContent="•••";
        item.rail.removeAttribute("aria-valuenow");
        item.rail.setAttribute("aria-valuetext","Выполняется, процент неизвестен");
      }
      item.rail.setAttribute("aria-label",item.name.textContent);
    }
    list.replaceChildren(...ordered.map(([key])=>rows.get(key).el));
    for(const key of [...rows.keys()]){
      if(!tasks.get(key)?.active)rows.delete(key);
    }
  }
  function set(kind,detail={}){
    if(typeof kind!=="string"||!kind)return;
    if(detail.active===false){clear(kind);return;}
    const task={
      active:true,
      label:typeof detail.label==="string"?detail.label:labels[kind]||"Обработка",
      stage:typeof detail.stage==="string"?detail.stage:"Выполняется",
      progress:Number.isFinite(detail.progress)?clamp(detail.progress):null
    };
    tasks.set(kind,task);
    if(kind==="gpu"&&typeof detail.projectId==="string"&&detail.projectId){
      trackedGpuProject=detail.projectId;
    }
    if(kind==="analysis"&&typeof detail.projectId==="string"&&detail.projectId){
      trackedAnalysisProject=detail.projectId;
    }
    render();
    schedule();
  }
  function clear(kind){
    if(!tasks.has(kind))return;
    tasks.delete(kind);
    render();
    schedule();
  }
  // An idle studio must not hammer three status endpoints every 2.6 s around
  // the clock. Poll fast only while a task is active; otherwise back off to
  // 30 s. User actions and server events wake the loop immediately.
  function schedule(){
    if(pollTimer!==null)clearTimeout(pollTimer);
    pollTimer=setTimeout(async()=>{
      pollTimer=null;
      await refresh();
      schedule();
    },tasks.size?2600:30000);
  }
  const api=async url=>{
    const res=await window.VolleyballStudioActivity.fetch(url,{cache:"no-store",credentials:"same-origin"});
    if(!res.ok)throw Error("HTTP "+res.status);
    return res.json();
  };
  const query=(url,params)=>url+"?"+new URLSearchParams(params).toString();
  async function readGpu(){
    if(tasks.has("analysis")){clear("gpu");return;}
    const current=$("studio-project")?.value||"";
    const project=trackedGpuProject||current;
    if(!project){clear("gpu");return;}
    const data=await api(query("/api/inference/native/full/active",{project_id:project}));
    if(data.project_id!==project||data.schema!=="volley-native-active-gpu/1")return;
    if(!data.active){
      if(project===trackedGpuProject)trackedGpuProject="";
      clear("gpu");
      return;
    }
    const model=data.model==="players"?"RTMDet + RTMPose":data.model==="ball"?"VballNet":"GPU";
    set("gpu",{projectId:project,label:"Анализ GPU · "+model,
      stage:{"queued":"В очереди","loading_model":"Загрузка модели","running":"Инференс"}[data.phase]||
        String(data.phase||"Обработка"),progress:Number.isFinite(data.progress_percent)?data.progress_percent:null});
  }
  async function readPipeline(){
    const current=$("studio-project")?.value||"";
    const project=trackedAnalysisProject||current;
    if(!project){clear("analysis");return;}
    const data=await api(query("/api/studio/pipeline/status",{project_id:project}));
    if(data.schema!=="volley-studio-pipeline/1"||data.project_id!==project)return;
    if(!["queued","running"].includes(data.status)){
      if(project===trackedAnalysisProject)trackedAnalysisProject="";
      clear("analysis");return;
    }
    set("analysis",{projectId:project,label:"Анализ и постобработка",
      stage:String(data.stage||"Обработка"),
      progress:Number.isFinite(data.progress_percent)?data.progress_percent:null});
  }
  async function readQuick(){
    const data=await api("/api/quick-reel/native/status");
    if(data.status!=="queued"&&data.status!=="running"){clear("quick");return;}
    const stages=["Подготовка","GPU","CPU","Монтаж"];
    const idx=Number(data.stage_index);
    const stage=typeof data.stage==="string"&&data.stage.trim()?data.stage:
      (idx>=1&&idx<=4?stages[idx-1]:"В очереди");
    set("quick",{label:"Быстрый рилс",stage,
      progress:Number.isFinite(data.progress)?data.progress:null});
  }
  async function refresh(){
    if(polling||document.hidden)return;
    if(!window.VolleyballStudioActivity?.fetch)return;
    polling=true;
    try{
      // A failing status request never hides a still-running task.
      await readPipeline().catch(()=>{});
      await Promise.allSettled([readGpu(),readQuick()]);
    }finally{polling=false;}
  }
  const restartButton=$("studio-restart"),restartMessage=$("studio-restart-status");
  let restarting=false,restartWatcher=null;
  const restartNotice=(message,isError=false)=>{
    if(!restartMessage)return;
    restartMessage.textContent=message;
    restartMessage.classList.toggle("hidden",!message);
    restartMessage.classList.toggle("is-error",isError);
  };
  const waitForRestart=previousPid=>{
    let attempts=0,checking=false;
    restartWatcher=setInterval(async()=>{
      if(checking)return;
      checking=true;
      try{
        const result=await api("/internal/rewrite-status");
        if(Number.isSafeInteger(result.pid)&&result.pid!==previousPid){
          clearInterval(restartWatcher);
          restartNotice("Студия перезапущена. Обновляем интерфейс…");
          window.location.reload();
          return;
        }
      }catch{/* A short connection loss is expected during the restart. */}
      finally{checking=false;}
      if(++attempts>=100){
        clearInterval(restartWatcher);
        restarting=false;
        restartButton.disabled=false;
        restartNotice("Не удалось подтвердить перезапуск. Проверьте состояние сервера.",true);
      }
    },1500);
  };
  restartButton?.addEventListener("click",async()=>{
    if(restarting)return;
    restarting=true;
    restartButton.disabled=true;
    restartNotice("Проверяем задачи и запускаем безопасный перезапуск…");
    try{
      const response=await window.VolleyballStudioActivity.fetch("/api/studio/restart",{
        method:"POST",cache:"no-store",credentials:"same-origin",
        headers:{"Content-Type":"application/json"},body:"{}"
      });
      const result=await response.json().catch(()=>({}));
      if(!response.ok)throw Error(result.error||result.message||"HTTP "+response.status);
      if(result.scheduled!==true||!Number.isSafeInteger(result.previous_pid)){
        throw Error("Сервер не подтвердил перезапуск");
      }
      restartNotice("Ожидаем запуска новой версии студии…");
      waitForRestart(result.previous_pid);
    }catch(error){
      restarting=false;
      restartButton.disabled=false;
      restartNotice(error.message||String(error),true);
    }
  });
  window.VolleyballTasks={set,clear,refresh};
  const wake=()=>{schedule();refresh();};
  $("studio-project")?.addEventListener("change",()=>{if(!trackedGpuProject&&!trackedAnalysisProject)wake();});
  window.addEventListener("volley:project-ready",wake);
  document.addEventListener("visibilitychange",()=>{if(!document.hidden)wake();});
  schedule();
  refresh();
})();