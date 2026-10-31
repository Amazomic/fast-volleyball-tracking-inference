"use strict";
window.VolleyballAnalysisUI=(()=>{
  const E=x=>document.getElementById(x);
  let info=null, events={}, video="", start=0,end=0,head=0, token=0,drag=null,timer=null;
  const ranges=new Map();
  const colors={attack:"attack",block:"block",serve:"serve",dig:"dig",overhead_pass:"pass",low_touch:"other"};
  const types={attack:"Атака",block:"Блок",serve:"Подача",dig:"Приём",overhead_pass:"Передача",low_touch:"Касание"};
  const clamp=(v,a,b)=>Math.max(a,Math.min(b,v));
  const percent=t=>info&&info.duration_sec>0?clamp(100*t/info.duration_sec,0,100):0;
  const clock=t=>{t=Math.max(0,Math.round(t));return String(Math.floor(t/3600)).padStart(2,"0")+":"+String(Math.floor(t%3600/60)).padStart(2,"0")+":"+String(t%60).padStart(2,"0")};
  const width=()=>E("manual-width-enabled").checked?Number(E("analysis-width").value):info?.auto_width;
  function updateSize(){
    if(!info)return;
    const w=width();
    if(!Number.isInteger(w)||w<320||w>1920||w%2){E("resolution-explanation").textContent="Нужна чётная ширина 320–1920 px";return}
    const h=Math.max(2,2*Math.round(w*info.height/info.width/2));
    E("auto-size-value").textContent=info.auto_width+" × "+info.auto_height+" px";
    E("source-dims").textContent=info.width+" × "+info.height;
    E("analysis-dims").textContent=w+" × "+h;
    const quality=w<info.width?"уменьшение (обработка легче)":w>info.width?"увеличение без новых деталей":"без изменения разрешения";
    E("resolution-explanation").textContent="Исходное "+info.width+"×"+info.height+" → размер перед анализом "+
      w+"×"+h+" px; "+quality+". Высота пропорциональна ширине."+
      (info.cached_analysis_width?" Прошлый анализ использовал ширину "+info.cached_analysis_width+" px.":"");
  }
  function redrawRange(){
    if(!info)return;
    const a=percent(start),b=percent(end);
    E("timeline-selected-range").style.left=a+"%";E("timeline-selected-range").style.width=(b-a)+"%";
    E("timeline-start-handle").style.left=a+"%";E("timeline-end-handle").style.left=b+"%";
    E("timeline-playhead").style.left=percent(head)+"%";
    E("timeline-start-input").value=String(Math.round(start));
    E("timeline-end-input").value=String(Math.round(end));
    E("timeline-range-label").textContent=clock(start)+" — "+clock(end)+" · "+clock(end-start)+" для анализа";
    if(video)ranges.set(video,{start:start,end:end});
    E("preview-time-label").textContent=clock(head);
  }
  function boundary(side,v){
    if(!info||!Number.isFinite(Number(v)))return;
    v=Number(v);
    if(side==="start")start=clamp(v,0,Math.max(0,end-1));
    else end=clamp(v,Math.min(info.duration_sec,start+1),info.duration_sec);
    redrawRange();
    E("range-save-status").textContent="Диапазон изменён — сохраните перед запуском";
  }
  function paint(){
    if(!info)return;
    E("timeline-rallies").innerHTML=(events.rallies||[]).map(x=>{
      const left=percent(x.start),span=Math.max(.10,percent(x.end)-left);
      return '<button type="button" class="rally-block" data-seek="'+x.start+'" style="left:'+left+'%;width:'+span+
        '%" title="Розыгрыш '+clock(x.start)+' — '+clock(x.end)+' (границы приблизительные)"></button>';
    }).join("");
    E("timeline-actions").innerHTML=(events.actions||[]).map(x=>{
      const t=(types[x.type]||"Касание")+" · "+clock(x.time)+" · "+Math.round(x.confidence*100)+"%";
      return '<button type="button" class="event-mark event-'+(colors[x.type]||"other")+
        '" data-seek="'+x.time+'" style="left:'+percent(x.time)+'%" title="'+t+'" aria-label="'+t+'"></button>';
    }).join("");
    E("timeline-ticks").innerHTML=[0,.25,.5,.75,1].map(i=>"<span>"+clock(info.duration_sec*i)+"</span>").join("");
    const count=events.actions?.length||0;
    E("timeline-status").textContent=events.analyzed?(events.rallies.length+" розыгрышей · "+count+" событий"):"Анализ ещё не выполнен";
    redrawRange();
  }
  function image(id,placeholder,mode,seq){
    const elem=E(id),lbl=E(placeholder),query=new URLSearchParams({video:video,time:head,mode:mode,width:width()||"auto"});
    elem.style.display="none";lbl.style.display="flex";lbl.textContent="Загрузка…";
    elem.onload=()=>{if(seq===token){elem.style.display="block";lbl.style.display="none"}};
    elem.onerror=()=>{if(seq===token){lbl.textContent="Нет кадра";elem.style.display="none"}};
    elem.src="/api/video-preview?"+query.toString();
  }
  function preview(){
    if(!info)return;
    const seq=++token;
    image("source-preview-img","source-preview-placeholder","source",seq);
    image("analysis-preview-img","analysis-preview-placeholder","analysis",seq);
  }
  function seek(v){
    if(!info)return;
    head=clamp(Number(v)||0,0,Math.max(0,info.duration_sec-.1));
    redrawRange();
    clearTimeout(timer);timer=setTimeout(preview,200);
  }
  async function setVideo(name){
    const seq=++token;
    if(info&&video)ranges.set(video,{start:start,end:end});
    video=name;info=null;
    E("preview-source-details").textContent="Загружаем данные…";
    E("timeline-status").textContent="Читаем события…";
    try{
      const load=async p=>{let r=await fetch(p,{cache:"no-store"}),j=await r.json();if(!r.ok)throw Error(j.error);return j};
      const [meta,timeline]=await Promise.all([
        load("/api/video-info?video="+encodeURIComponent(name)),
        load("/api/timeline?video="+encodeURIComponent(name))]);
      if(seq!==token)return;
      info=meta;events=timeline;
      const remembered=ranges.get(name);
      start=remembered?clamp(remembered.start,0,Math.max(0,meta.duration_sec-1)):0;
      end=remembered?clamp(remembered.end,start+1,meta.duration_sec):meta.duration_sec;
      head=Math.min(Math.max(1,Math.round(end*.02)),Math.max(0,end-.1));
      E("timeline-start-input").max=end;E("timeline-end-input").max=end;
      E("preview-source-details").textContent=meta.width+"×"+meta.height+" · "+clock(end);
      updateSize();paint();preview();
    }catch(e){if(seq===token){E("timeline-status").textContent="Ошибка";E("preview-source-details").textContent=e.message}}
  }
  function position(ev){
    const r=E("match-timeline").getBoundingClientRect();
    return clamp((ev.clientX-r.left)/r.width,0,1)*info.duration_sec;
  }
  function setProjectRanges(saved={}) {
    ranges.clear();
    if(saved&&typeof saved==="object"){
      for(const [name,r] of Object.entries(saved)){
        if(r&&Number.isFinite(Number(r.start))&&Number.isFinite(Number(r.end)))
          ranges.set(name,{start:Number(r.start),end:Number(r.end)});
      }
    }
    // Avoid writing previous project's open frame into the newly selected project.
    info=null;video="";
  }
  function currentRange(){
    return info?{start:Math.round(start*1000)/1000,end:Math.round(end*1000)/1000}:null;
  }
  function init(){
    E("manual-width-enabled").addEventListener("change",e=>{
      E("analysis-width").disabled=!e.target.checked;updateSize();preview();
    });
    E("analysis-width").addEventListener("change",()=>{updateSize();preview()});
    E("refresh-preview").addEventListener("click",preview);
    E("timeline-reset").addEventListener("click",()=>{if(info){start=0;end=info.duration_sec;redrawRange();E("range-save-status").textContent="Диапазон изменён — сохраните перед запуском"}});
    E("timeline-start-input").addEventListener("change",e=>boundary("start",e.target.value));
    E("timeline-end-input").addEventListener("change",e=>boundary("end",e.target.value));
    const timeline=E("match-timeline");
    timeline.addEventListener("click",e=>{
      if(!info||e.target.closest(".timeline-handle"))return;
      const mark=e.target.closest("[data-seek]");
      seek(mark?Number(mark.dataset.seek):position(e));
    });
    for(const [side,id] of [["start","timeline-start-handle"],["end","timeline-end-handle"]]){
      const handle=E(id);
      handle.addEventListener("pointerdown",ev=>{
        if(!info)return;
        ev.preventDefault();ev.stopPropagation();drag=side;handle.setPointerCapture(ev.pointerId);
      });
      handle.addEventListener("pointermove",ev=>{if(drag===side)boundary(side,position(ev))});
      ["pointerup","pointercancel","lostpointercapture"].forEach(name=>
        handle.addEventListener(name,()=>{if(drag===side)drag=null}));
      handle.addEventListener("keydown",ev=>{
        if(!info||!["ArrowLeft","ArrowRight"].includes(ev.key))return;
        ev.preventDefault();boundary(side,(side==="start"?start:end)+(ev.key==="ArrowLeft"?-1:1));
      });
    }
  }
  function params(){
    if(!info)throw Error("Дождитесь загрузки данных видео");
    const w=width();
    if(!Number.isInteger(w)||w<320||w>1920||w%2)throw Error("Недопустимая ширина анализа");
    const entire=start===0&&Math.abs(end-info.duration_sec)<.01;
    const duration=entire?0:Math.round((end-start)*100)/100;
    if(duration!==0&&duration<1)throw Error("Минимальный диапазон — одна секунда");
    return {analysis_width:E("manual-width-enabled").checked?w:"auto",
      analysis_start:Math.round(start*100)/100,analysis_seconds:duration,
      ranges:Object.fromEntries(Array.from(ranges.entries()).map(([key,v])=>[key,{start:v.start,end:v.end}]))};
  }
  return {init,setVideo,params,setProjectRanges,currentRange};
})();
