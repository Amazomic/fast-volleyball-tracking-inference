"use strict";
// Read-only old-project discovery, explicit metadata import and recoverable
// isolated Rust archive. No legacy filesystem path is ever sent by the UI.
(()=>{
  const $=id=>document.getElementById(id);
  const ui={
    projects:$("studio-legacy-projects"),archives:$("studio-legacy-archives"),
    refresh:$("studio-legacy-refresh"),archive:$("studio-legacy-archive-current"),
    status:$("studio-legacy-status"),select:$("studio-project")
  };
  let busy=false,loaded=false,available=[];
  const valid=id=>typeof id==="string"&&/^[a-f0-9]{12}$/.test(id);
  const validArchive=id=>typeof id==="string"&&/^[a-f0-9]{12}-[a-f0-9]{32}$/.test(id);
  const node=(tag,cls,text)=>{
    const el=document.createElement(tag);
    if(cls)el.className=cls;
    if(text!==undefined)el.textContent=String(text);
    return el;
  };
  const api=async(path,options={})=>{
    const response=await fetch(path,{cache:"no-store",credentials:"same-origin",...options});
    const result=await response.json().catch(()=>({}));
    if(!response.ok)throw Error(typeof result.error==="string"?
      result.error:"Ошибка HTTP "+response.status);
    return result;
  };
  const post=(path,payload)=>api(path,{
    method:"POST",headers:{"Content-Type":"application/json"},
    body:JSON.stringify(payload)
  });
  const message=(text,failed=false)=>{
    ui.status.textContent=String(text);
    ui.status.classList.toggle("quick-error",failed);
  };
  const controls=()=>{
    ui.refresh.disabled=busy;
    const selected=ui.select.value;
    ui.archive.disabled=busy||!valid(selected)||!available.includes(selected);
  };
  const openProject=id=>{
    if(!valid(id))return;
    window.dispatchEvent(new CustomEvent("volley:open-legacy-project",{
      detail:{project_id:id}
    }));
  };
  const run=async(work,what)=>{
    if(busy)return;
    busy=true;controls();message(what+"…");
    try{
      const result=await work();
      if(result===false)throw Error("Сервер не подтвердил изменение");
      window.dispatchEvent(new Event("volley:refresh-projects"));
      await refresh(true);
      message(what+" успешно. Оригинальные данные прежней студии не изменены.");
    }catch(error){
      message((error instanceof Error?error.message:String(error)),true);
    }finally{busy=false;controls();}
  };
  const legacyEntries=(data,archives)=>{
    const archivedIds=new Set((Array.isArray(archives.archives)?archives.archives:[])
      .map(row=>row.project_id).filter(valid));
    ui.projects.replaceChildren();
    const rows=Array.isArray(data.projects)?data.projects:[];
    if(!rows.length)ui.projects.append(node("p","panel-note","В старом реестре проекты не найдены."));
    for(const project of rows){
      if(!valid(project.id)||typeof project.name!=="string")continue;
      const card=node("article","legacy-entry");
      card.append(node("strong","",project.name));
      card.append(node("p","panel-note",
        "ID "+project.id+" · "+
        (Array.isArray(project.videos)?project.videos.length:0)+" видео · "+
        (Array.isArray(project.historical_runs)?project.historical_runs.length:0)+
        " исторических записей (не нативный Rust-инференс)"));
      const historic=Array.isArray(project.historical_runs)?project.historical_runs:[];
      if(historic.length)card.append(node("p","panel-note",
        "Сохранённые старые запуски (только для просмотра метаданных): "+historic.join(", ")));
      const missing=Array.isArray(project.missing_videos)?project.missing_videos:[];
      if(missing.length){
        card.append(node("p","panel-note quick-error",
          "Недоступны исходные видео: "+missing.join(", ")+
          ". Старые результаты сохранены, но повторный анализ требует настоящих исходников."));
      }else card.append(node("p","panel-note",
        "Исходные файлы обнаружены; перед обработкой выполняется повторная проверка."));
      const archived=archivedIds.has(project.id);
      const button=node("button","secondary",
        project.imported?"Открыть в Rust":archived?
        "В архиве — восстановите ниже":"Импортировать метаданные в Rust");
      button.type="button";button.disabled=busy||archived&&!project.imported;
      button.addEventListener("click",()=>{
        if(project.imported){openProject(project.id);return;}
        run(async()=>{
          const saved=await post("/api/project-legacy/native/import",
            {project_id:project.id});
          return saved.imported===true&&saved.project_id===project.id&&
            saved.original_results_immutable===true&&saved.copied_media===false;
        },"Импорт старого проекта");
      });
      card.append(button);
      ui.projects.append(card);
    }
  };
  const archiveEntries=data=>{
    ui.archives.replaceChildren();
    const rows=Array.isArray(data.archives)?data.archives:[];
    if(!rows.length)ui.archives.append(node("p","panel-note",
      "Восстанавливаемых архивов Rust пока нет."));
    for(const item of rows){
      if(!validArchive(item.archive_id)||!valid(item.project_id))continue;
      const card=node("article","legacy-entry");
      card.append(node("strong","",String(item.name||item.project_id)));
      card.append(node("p","panel-note",
        "Rust-архив · ID "+item.project_id+" · "+Number(item.video_count||0)+" видео"));
      const button=node("button","secondary","Восстановить Rust-проект");
      button.type="button";button.disabled=busy;
      button.addEventListener("click",()=>{
        run(async()=>{
          const result=await post("/api/project-archive/native/restore",
            {archive_id:item.archive_id});
          return result.restored===true&&result.project_id===item.project_id&&
            result.legacy_results_unchanged===true;
        },"Восстановление архива");
      });
      card.append(button);
      ui.archives.append(card);
    }
  };
  const refresh=async(force=false)=>{
    if(busy&&!force)return;
    ui.projects.replaceChildren(node("p","panel-note","Проверка старого реестра…"));
    ui.archives.replaceChildren(node("p","panel-note","Проверка Rust-архива…"));
    const [catalog,archives,projects]=await Promise.all([
      api("/api/project-legacy/native/catalog"),
      api("/api/project-archive/native"),
      api("/api/projects")
    ]);
    if(catalog.original_results_immutable!==true||archives.isolated_rust_only!==true)
      throw Error("Сервер не подтвердил изоляцию старых проектов");
    available=(Array.isArray(projects.projects)?projects.projects:[])
      .map(p=>p.id).filter(valid);
    loaded=true;
    legacyEntries(catalog,archives);archiveEntries(archives);controls();
    if(!busy)message("Проверено: старые результаты только для чтения, Rust-архив восстанавливаемый.");
  };
  ui.refresh.addEventListener("click",()=>refresh().catch(err=>
    message("Ошибка списка проектов: "+err.message,true)));
  ui.archive.addEventListener("click",()=>{
    const id=ui.select.value;
    if(!valid(id)||!available.includes(id))return;
    run(async()=>{
      const result=await post("/api/project-archive/native",{project_id:id});
      return result.archived===true&&result.deleted===false&&
        result.project_id===id;
    },"Архивация Rust-проекта");
  });
  ui.select.addEventListener("change",controls);
  window.addEventListener("volley:tab-change",event=>{
    if(event.detail?.tab==="legacy"){
      refresh().catch(err=>message("Не удалось прочитать старые проекты: "+err.message,true));
    }
  });
  controls();
})();
