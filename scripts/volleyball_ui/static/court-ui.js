"use strict";
window.VolleyballCourtUI=(()=>{
  const E=id=>document.getElementById(id);
  const NS="http://www.w3.org/2000/svg";
  const NAMES=["1_back_left","2_back_left","3_back_right","4_back_right","5_center_left","6_center_right","7_net_left","8_net_right"];
  const LABELS=["Ближний левый угол","Дальний левый угол","Дальний правый угол","Ближний правый угол",
    "Средняя точка слева (условная)","Средняя точка справа (условная)","Сетка слева","Сетка справа"];
  const LINKS=[[0,4],[4,1],[1,2],[2,5],[5,3],[3,0],[4,5],[4,6],[6,7],[7,5]];
  let meta=null,video="",projectId="",saved=null,points=[],editing=false,dirty=false,frameSec=0,loadSeq=0,drag=null;
  const svg=()=>E("court-overlay"),player=()=>E("source-video-player");
  function make(tag,attrs={},label=null){
    const el=document.createElementNS(NS,tag);
    for(const [k,v] of Object.entries(attrs))el.setAttribute(k,String(v));
    if(label!==null)el.textContent=label;
    return el;
  }
  function state(){
    const n=points.length;
    E("court-progress").textContent=editing?
      (n===8?"8/8 точек — сохраните":"Точек: "+n+"/8"):
      (saved?"Площадка размечена · 8 точек":"Площадка не размечена");
    E("court-mode-toggle").textContent=editing?"Завершить разметку":"Разметить площадку";
    E("court-undo").hidden=!editing;
    E("court-clear").hidden=!editing;
    E("court-instruction").hidden=!editing;
    E("court-overlay-badge").hidden=!editing&&!saved;
    E("court-point-prompt").textContent=n===8?"Разметка готова — сохраните настройки видео":
      "Точка "+(n+1)+"/8: "+LABELS[n];
  }
  function render(){
    const s=svg();s.replaceChildren();
    if(!meta){s.setAttribute("hidden","");state();return;}
    s.setAttribute("viewBox","0 0 "+meta.width+" "+meta.height);
    s.classList.toggle("is-editing",editing);
    const visible=editing||!!(saved&&Math.abs(player().currentTime-(saved.frame_sec||0))<1.5);
    s.toggleAttribute("hidden",!visible);
    E("court-overlay-badge").hidden=!visible;
    const stroke=Math.max(2,meta.width/350);
    // SVG blank area is not a painted hit target; use a transparent rectangle.
    s.appendChild(make("rect",{x:0,y:0,width:meta.width,height:meta.height,
      fill:"transparent","pointer-events":editing?"all":"none"}));
    for(const [a,b] of LINKS){
      if(!points[a]||!points[b])continue;
      s.appendChild(make("line",{x1:points[a].x,y1:points[a].y,x2:points[b].x,y2:points[b].y,
        stroke:"#a6ee80","stroke-width":stroke,"stroke-linecap":"round"}));
    }
    points.forEach((p,i)=>{
      s.appendChild(make("circle",{cx:p.x,cy:p.y,r:stroke*3.5,fill:"#d0f489",
        stroke:"#102418","stroke-width":stroke,"data-point":i}));
      s.appendChild(make("text",{x:p.x+stroke*5,y:p.y-stroke*2,
        fill:"#fff","font-size":stroke*9,"font-weight":"800",
        stroke:"#091411","stroke-width":stroke*1.5,"paint-order":"stroke","pointer-events":"none"},String(i+1)));
    });
    state();
  }
  function coords(e){
    const rect=svg().getBoundingClientRect();
    const scale=Math.min(rect.width/meta.width,rect.height/meta.height);
    const dx=rect.left+(rect.width-meta.width*scale)/2;
    const dy=rect.top+(rect.height-meta.height*scale)/2;
    const x=(e.clientX-dx)/scale,y=(e.clientY-dy)/scale;
    if(!Number.isFinite(x)||!Number.isFinite(y)||x<0||y<0||x>meta.width||y>meta.height)return null;
    return{x:Math.round(x*1000)/1000,y:Math.round(y*1000)/1000};
  }
  function toggle(){
    if(!meta)return;
    if(!editing){
      player().pause();editing=true;dirty=false;
      points=saved?saved.keypoints.map(x=>({x:x.x,y:x.y})):[];
      frameSec=saved?saved.frame_sec:player().currentTime;
      if(saved)player().currentTime=saved.frame_sec;
    }else{
      if(dirty&&points.length>0&&points.length<8){
        E("court-progress").textContent="Для сохранения нужны все 8 точек";
        return;
      }
      editing=false;
    }
    render();
  }
  function init(){
    const s=svg();
    s.addEventListener("pointerdown",e=>{
      if(!editing||!meta||e.button!==0)return;
      const pt=coords(e);if(!pt)return;
      e.preventDefault();e.stopPropagation();
      const selected=e.target.closest("[data-point]");
      if(selected)drag=Number(selected.getAttribute("data-point"));
      else if(points.length<8){
        points.push(pt);dirty=true;drag=points.length-1;
      }else return;
      s.setPointerCapture(e.pointerId);render();
    });
    s.addEventListener("pointermove",e=>{
      if(drag===null)return;
      const pt=coords(e);
      if(pt){points[drag]=pt;dirty=true;render();}
    });
    const release=e=>{
      if(drag===null)return;
      const pt=coords(e);
      if(pt){points[drag]=pt;dirty=true;}
      drag=null;render();
      if(s.hasPointerCapture(e.pointerId))s.releasePointerCapture(e.pointerId);
    };
    s.addEventListener("pointerup",release);
    s.addEventListener("pointercancel",release);
    E("court-mode-toggle").addEventListener("click",toggle);
    E("court-undo").addEventListener("click",()=>{
      if(!editing||!points.length)return;
      points.pop();dirty=true;render();
    });
    E("court-clear").addEventListener("click",()=>{
      if(!editing)return;
      points=[];dirty=true;frameSec=player().currentTime;render();
    });
    player().addEventListener("timeupdate",()=>{if(!editing&&meta)render();});
  }
  async function setVideo(id,name,info){
    const seq=++loadSeq;
    projectId=id;video=name;meta=info;editing=false;dirty=false;saved=null;points=[];render();
    if(!id||!name)return;
    try{
      const resp=await fetch("/api/court?project_id="+encodeURIComponent(id)+"&video="+encodeURIComponent(name),{cache:"no-store"});
      const obj=await resp.json();
      if(!resp.ok)throw Error(obj.error||"Ошибка разметки");
      if(seq!==loadSeq)return;
      saved=obj.court;
      points=saved?saved.keypoints.map(p=>({x:p.x,y:p.y})):[];
      frameSec=saved?.frame_sec||0;render();
    }catch(error){
      if(seq===loadSeq)E("court-progress").textContent=error.message;
    }
  }
  function snapshot(){
    if(!meta)throw Error("Видео ещё загружается");
    if(!dirty)return{court:null,clear_court:false};
    if(points.length>0&&points.length<8)throw Error("Отметьте все восемь точек или сбросьте разметку");
    if(!points.length)return{court:null,clear_court:true};
    return{court:{
      frame_width:meta.width,frame_height:meta.height,frame_sec:frameSec,
      keypoints:NAMES.map((name,id)=>({id,name,x:points[id].x,y:points[id].y,visible:true}))
    },clear_court:false};
  }
  function markSaved(court){
    saved=court||null;
    points=saved?saved.keypoints.map(p=>({x:p.x,y:p.y})):[];
    frameSec=saved?.frame_sec||0;dirty=false;editing=false;render();
  }
  function reset(){++loadSeq;projectId="";video="";meta=null;saved=null;points=[];editing=false;dirty=false;render();}
  return{init,setVideo,snapshot,markSaved,reset};
})();
