"use strict";
/* Display RAVEL-013's saved per-player Touch predictions, not contact heuristics.
   UI and cache queries are intentionally separate from analysis/events.json. */
window.VolleyballModelTouches=(()=>{
  const E=id=>document.getElementById(id);
  const player=()=>E("result-video-player");
  const fmt=time=>{
    const t=Math.max(0,Number(time)||0);
    const m=Math.floor(t/60),s=t%60;
    return String(Math.floor(m/60)).padStart(2,"0")+":"+
      String(m%60).padStart(2,"0")+":"+s.toFixed(2).padStart(5,"0");
  };
  let context=null,summary=null,windowData=null,epoch=0,pending=null;
  let mode="events",touchVisible=true,onChange=()=>{},lastFrame=-1;
  const threshold=()=>Number(E("result-model-touch-threshold").value)||0.5;
  const active=()=>mode==="model";

  function reset(){
    epoch++;
    if(pending){pending.abort();pending=null}
    context=null;summary=null;windowData=null;lastFrame=-1;
    E("result-model-touch-boxes").replaceChildren();
    E("result-model-touch-current").textContent="";
    E("result-model-touch-nearby").replaceChildren();
    E("result-model-touch-summary").textContent="";
    E("result-model-touch-threshold").disabled=true;
    const canvas=E("result-model-touch-histogram");
    canvas.getContext("2d").clearRect(0,0,canvas.width,canvas.height);
  }
  function visibility(){
    E("view-analysis-results").classList.toggle("raw-touch-mode",active());
    E("result-model-touch-section").hidden=!(active()&&touchVisible);
    E("result-model-touch-controls").hidden=!(active()&&touchVisible);
    if(active()){
      E("result-overlay-ball").hidden=true;
      E("result-overlay-event").hidden=true;
      E("result-overlay-touch").hidden=true;
      E("result-overlay-rally").hidden=true;
      E("result-player-boxes").replaceChildren();
      E("result-ball-trail").setAttribute("hidden","");
    }else{
      E("result-model-touch-boxes").replaceChildren();
    }
    onChange();
  }
  const query=(extra={})=>{
    const args=new URLSearchParams({project_id:context.project,
      video:context.video,cache_id:context.cache,threshold:String(threshold()),...extra});
    return "/api/analysis-model-touches?"+args;
  };
  async function fetchJson(url,controller){
    if(window.VolleyDataCache?.supports(url))
      return window.VolleyDataCache.get(url,{signal:controller.signal});
    const response=await fetch(url,{cache:"no-store",signal:controller.signal});
    const json=await response.json();
    if(!response.ok)throw Error(json.error||"Ошибка запроса предсказаний Touch");
    return json;
  }
  function histogram(){
    const canvas=E("result-model-touch-histogram");
    const ctx=canvas.getContext("2d");
    ctx.clearRect(0,0,canvas.width,canvas.height);
    if(!summary?.available)return;
    const bins=summary.bins||[],max=Math.max(1,...bins);
    const cell=canvas.width/Math.max(1,bins.length);
    ctx.fillStyle="#e6ac58";
    for(let i=0;i<bins.length;i++){
      const h=bins[i]>0?Math.max(2,Math.sqrt(bins[i]/max)*(canvas.height-8)):0;
      if(h)ctx.fillRect(i*cell,canvas.height-h,Math.max(1,cell-.4),h);
    }
  }
  function presentSummary(){
    const info=E("result-model-touch-summary"),header=E("result-model-touch-info");
    const filter=E("result-model-touch-threshold");
    filter.disabled=!summary?.available;
    if(!summary?.available){
      const reason=summary?.reason==="different_model"
        ?"В выбранном результате использована не RAVEL-013. Выберите результат профиля «Новый»."
        :"Нет сохранённых предсказаний RAVEL-013 для этого видео.";
      info.textContent=reason;
      header.textContent="Касания RAVEL-013 недоступны";
      return;
    }
    const floor=Number(summary.stored_threshold)||0;
    for(const option of Array.from(filter.options))option.disabled=Number(option.value)<floor;
    if(!Array.from(filter.options).some(option=>Number(option.value)===summary.threshold)){
      const option=document.createElement("option");
      option.value=String(summary.threshold);
      option.textContent=Math.round(summary.threshold*100)+"%";
      filter.append(option);
    }
    filter.value=String(summary.threshold);
    const count=Number(summary.count).toLocaleString(window.VolleyI18n?.locale()||"ru-RU");
    header.textContent=count+" предсказаний Touch · порог "+
      Math.round(summary.threshold*100)+"%";
    info.textContent="Это отдельные ответы нейросети на кадрах игроков, а не "+
      "уникальные касания. Нет фильтра розыгрышей и эвристической классификации. "+
      "Порог исходной записи: "+Math.round(summary.stored_threshold*100)+"%.";
    histogram();
  }
  async function loadSummary(){
    if(!active()||!touchVisible||!context)return;
    const token=++epoch;
    if(pending){pending.abort();pending=null}
    windowData=null;summary=null;lastFrame=-1;
    E("result-model-touch-boxes").replaceChildren();
    E("result-model-touch-nearby").replaceChildren();
    E("result-model-touch-current").textContent="";
    E("result-model-touch-summary").textContent="Загружаются предсказания RAVEL-013…";
    const ctl=new AbortController();pending=ctl;
    try{
      const payload=await fetchJson(query(),ctl);
      if(token!==epoch)return;
      summary=payload;presentSummary();
    }catch(error){
      if(token===epoch&&error.name!=="AbortError"){
        E("result-model-touch-summary").textContent=error.message;
      }
    }finally{
      if(pending===ctl)pending=null;
      if(token===epoch&&summary?.available)update(player().currentTime||0);
    }
  }
  function drawWindowList(){
    const target=E("result-model-touch-nearby");
    if(!windowData)return;
    const events=windowData.events||[];
    const shown=events.slice(0,48);
    target.replaceChildren();
    const frag=document.createDocumentFragment();
    for(const item of shown){
      const button=document.createElement("button");
      button.type="button";button.className="result-model-touch-item";
      button.dataset.touchTime=String(item.time);
      button.textContent=fmt(item.time+(context.offset||0))+
        " · ID "+(item.player_id??"—")+" · Touch "+
        Math.round(item.score*100)+"%";
      frag.append(button);
    }
    target.append(frag);
    E("result-model-touch-window-count").textContent=
      "В интервале "+windowData.window_total+" предсказаний"+
      (events.length>shown.length?" · первые "+shown.length+" в списке":"");
  }
  async function loadWindow(time){
    if(!active()||!touchVisible||!context||!summary?.available)return;
    const start=Math.max(0,Math.floor(time/8)*8);
    if(windowData&&windowData.start===start)return;
    if(pending)return;
    const token=epoch,ctl=new AbortController();
    pending=ctl;
    try{
      const payload=await fetchJson(query({start:String(start),length:"8"}),ctl);
      if(epoch!==token||!active())return;
      windowData=payload;lastFrame=-1;
      drawWindowList();drawAt(time);
    }catch(error){
      if(epoch===token&&error.name!=="AbortError")
        E("result-model-touch-window-count").textContent=error.message;
    }finally{
      if(pending===ctl)pending=null;
      // Seeking while a previous window was loading must not leave stale boxes.
      if(epoch===token&&active()&&windowData&&
         (player().currentTime<windowData.start||
          player().currentTime>=windowData.start+windowData.length))
        update(player().currentTime);
    }
  }
  function drawAt(time){
    const head=E("result-model-touch-playhead");
    if(context)head.style.left=(Math.max(0,Math.min(1,time/Math.max(1,context.duration)))*100)+"%";
    if(!windowData||!summary?.available||!context)return;
    const frame=Math.round(time*summary.fps);
    if(frame===lastFrame)return;
    lastFrame=frame;
    const matches=(windowData.events||[]).filter(item=>Math.abs(item.frame-frame)<=1).slice(0,16);
    const panel=E("result-model-touch-boxes");
    const frag=document.createDocumentFragment();
    for(const item of matches){
      const b=item.box,box=document.createElement("div");
      box.className="result-model-touch-box";
      box.style.left=(100*b[0]/summary.width)+"%";
      box.style.top=(100*b[1]/summary.height)+"%";
      box.style.width=(100*(b[2]-b[0])/summary.width)+"%";
      box.style.height=(100*(b[3]-b[1])/summary.height)+"%";
      const tag=document.createElement("span");
      tag.textContent="RAVEL Touch "+Math.round(item.score*100)+"% · ID "+(item.player_id??"—");
      box.append(tag);frag.append(box);
    }
    panel.replaceChildren(frag);
    E("result-model-touch-current").textContent=matches.length
      ?matches.length+" предсказаний Touch в этом кадре"
      :"На этом кадре нет сохранённого Touch";
  }
  function update(time){
    if(!active()||!touchVisible)return;
    drawAt(time);
    if(summary?.available&&(!windowData||time<windowData.start||
      time>=windowData.start+windowData.length-.1))loadWindow(time);
  }
  function setTouchVisible(value){
    value=Boolean(value);
    if(touchVisible===value)return;
    touchVisible=value;
    if(!touchVisible){
      // A late response from a previous RAVEL selection must not draw Touch
      // boxes over the VballNet-only view.
      epoch++;
      if(pending){pending.abort();pending=null}
      windowData=null;summary=null;lastFrame=-1;
      E("result-model-touch-boxes").replaceChildren();
    }
    visibility();
    if(touchVisible&&active()&&context)loadSummary();
  }
  function setSource(source){
    reset();
    context=source;
    if(active())loadSummary();
  }
  function init(callback){
    onChange=callback||(()=>{});
    for(const input of document.querySelectorAll('[name="result-preview-mode"]')){
      input.addEventListener("change",()=>{
        if(!input.checked)return;
        mode=input.value;
        visibility();
        if(active()&&context)loadSummary();
      });
    }
    E("result-model-touch-threshold").addEventListener("change",()=>{
      if(active()&&context)loadSummary();
    });
    E("result-model-touch-histogram").addEventListener("click",event=>{
      if(!summary?.available||!context)return;
      const box=event.currentTarget.getBoundingClientRect();
      const fraction=Math.max(0,Math.min(1,(event.clientX-box.left)/Math.max(1,box.width)));
      player().currentTime=fraction*context.duration;
    });
    E("result-model-touch-nearby").addEventListener("click",event=>{
      const button=event.target.closest("[data-touch-time]");
      if(button)player().currentTime=Number(button.dataset.touchTime);
    });
    visibility();
  }
  return {init,setSource,reset,active,update,setTouchVisible};
})();
