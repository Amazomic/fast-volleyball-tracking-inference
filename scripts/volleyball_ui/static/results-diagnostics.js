/* Diagnostics are a separate, opt-in job. Never reruns VballNet/RAVEL. */
(()=>{
  const E=id=>document.getElementById(id);
  const esc=s=>String(s??"").replace(/[&<>"']/g,x=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[x]));
  const project=()=>E("top-project")?.value||"";
  const video=()=>E("diagnostics-source-select")?.value||"";
  let jobId="",active=false;
  const detail=E("results-diagnostics");
  const status=E("diagnostics-status");
  const button=E("diagnostics-submit");
  async function fileList(){
    if(!project()||!video()){
      E("diagnostics-files").textContent="Выберите видео для диагностики.";
      return;
    }
    try{
      const q=new URLSearchParams({project_id:project(),video:video()});
      const response=await fetch("/api/diagnostic-outputs?"+q,{cache:"no-store"});
      const data=await response.json();
      if(!response.ok)throw Error(data.error||"Не удалось прочитать диагностические файлы");
      const files=Array.isArray(data.files)?data.files:[];
      E("diagnostics-files").innerHTML=files.length?
        "<strong>Созданные файлы ("+files.length+")</strong>"+
        files.map(f=>'<a href="'+esc(f.url)+'" target="_blank" rel="noopener" class="diagnostics-file">'+
          '<span>'+esc(f.name)+'</span><small>'+esc(f.size_mb)+' МБ</small></a>').join("")
        :'<p class="footnote">Для этого видео диагностических файлов пока нет.</p>';
    }catch(e){E("diagnostics-files").textContent=e.message;}
  }
  async function poll(){
    if(!active)return;
    try{
      const response=await fetch("/api/job",{cache:"no-store"});
      if(!response.ok)return;
      const state=await response.json();
      if(state.id!==jobId||state.kind!=="diagnostics"){active=false;return}
      E("diagnostics-log").hidden=false;
      E("diagnostics-log").textContent=state.log||state.stage||"Подготовка…";
      if(state.status==="running"){
        status.textContent="Создание диагностических файлов…";
      }else{
        active=false;button.disabled=false;
        status.textContent=state.status==="complete"?"Диагностика завершена":
          "Диагностика не завершена — проверьте журнал";
        await fileList();
      }
    }catch(e){console.warn("Diagnostics progress",e)}
  }
  async function submit(event){
    event.preventDefault();
    if(!project()||!video()){status.textContent="Выберите проанализированное видео";return}
    const values=Object.fromEntries(new FormData(event.currentTarget));
    for(const name of ["tracks_include_not_rally","tracks_mark_ball","tracks_debug","show_rally_snapshot"])
      values[name]=event.currentTarget.elements.namedItem(name)?.checked===true;
    if(values.tracks_export_mode==="none"&&!values.show_rally_snapshot){
      status.textContent="Выберите формат MP4 или включите снимок трека";return;
    }
    values.project_id=project();values.video=video();
    button.disabled=true;
    try{
      const response=await fetch("/api/diagnostics",{
        method:"POST",headers:{"Content-Type":"application/json"},
        body:JSON.stringify(values),cache:"no-store"
      });
      const result=await response.json();
      if(!response.ok)throw Error(result.error||"Не удалось запустить диагностику");
      jobId=result.id;active=true;
      status.textContent="Запуск диагностики…";
      await poll();
    }catch(e){status.textContent=e.message;button.disabled=false}
  }
  function setSources(pid,videos=[],preferred=""){
    const select=E("diagnostics-source-select");
    if(!select)return;
    const previous=select.dataset.project===pid?select.value:"";
    const available=Array.isArray(videos)?videos:[];
    select.innerHTML=available.map(name=>
      '<option value="'+esc(name)+'">'+esc(name)+'</option>').join("");
    select.dataset.project=pid;
    select.value=available.includes(previous)?previous:
      available.includes(preferred)?preferred:(available[0]||"");
    if(detail.open)fileList();
  }
  function init(){
    E("diagnostics-form").addEventListener("submit",submit);
    detail.addEventListener("toggle",()=>{if(detail.open)fileList()});
    E("diagnostics-source-select").addEventListener("change",()=>{if(detail.open)fileList()});
    window.setInterval(poll,1600);
  }
  window.VolleyballDiagnosticsUI={setSources};
  if(document.readyState==="loading")document.addEventListener("DOMContentLoaded",init,{once:true});
  else init();
})();
