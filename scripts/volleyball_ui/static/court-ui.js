"use strict";
/* Single, optional court calibration mode. Coordinates belong to the original
 * portrait/landscape source, never to a letterboxed video player. */
window.VolleyballCourtUI=(()=>{
  const E=id=>document.getElementById(id),NS="http://www.w3.org/2000/svg";
  const NAMES=["1_back_left","2_back_left","3_back_right","4_back_right","5_center_left","6_center_right","7_net_left","8_net_right"];
  const LABELS=["Ближний левый угол","Дальний левый угол","Дальний правый угол",
    "Ближний правый угол","Условная середина слева","Условная середина справа",
    "Сетка слева","Сетка справа"];
  const LINKS=[[0,4],[4,1],[1,2],[2,5],[5,3],[3,0],[4,5],[4,6],[6,7],[7,5]];
  let meta=null,projectId="",video="",saved=null,points=Array(8).fill(null),
      editing=false,dirty=false,index=0,frameSec=0,seq=0,drag=null,history=[];
  const overlay=()=>E("court-overlay"),player=()=>E("source-video-player");
  const known=p=>p&&p.status!=="unknown"&&Number.isFinite(p.x)&&Number.isFinite(p.y);
  const copy=()=>points.map(p=>p?{...p}:null);
  function element(tag,attrs={},label=null){
    const e=document.createElementNS(NS,tag);
    for(const [key,value] of Object.entries(attrs))e.setAttribute(key,String(value));
    if(label!==null)e.textContent=label;
    return e;
  }
  function feedback(){
    const count=points.filter(Boolean).length;
    E("court-toggle-title").textContent=editing?"Завершить разметку":"Разметить площадку";
    E("court-skip").hidden=!editing;
    E("court-undo").hidden=!editing;
    E("court-undo").disabled=!history.length;
    E("court-clear").hidden=!editing;
    E("court-instruction").hidden=!editing;
    E("court-point-prompt").textContent=count===8?
      "Все 8 точек отмечены — сохраните разметку":
      "Точка "+(index+1)+"/8: "+LABELS[index];
    E("court-progress").textContent=editing?
      (count+"/8"+(dirty?" · не сохранено":"")):
      saved?("сохранено · "+saved.keypoints.filter(p=>p.status==="unknown").length+" неизвестных"):
        "необязательно";
    E("court-overlay-badge").hidden=!editing&&!saved;
  }
  function render(){
    const surface=overlay();surface.replaceChildren();
    surface.classList.toggle("is-editing",editing);
    player().parentElement.classList.toggle("court-outside-mode",editing);
    if(!meta){surface.setAttribute("hidden","");feedback();return;}
    const w=meta.width,h=meta.height;
    // Extend source-image coordinates across the WHOLE stable gallery canvas.
    // The measured rendered video rectangle determines the real image-to-canvas
    // scale. There is no second small annotation box with independent padding.
    const show=editing||Boolean(saved&&Math.abs(player().currentTime-saved.frame_sec)<1.5);
    if(!show){surface.setAttribute("hidden","");feedback();return;}
    // Unhide BEFORE measuring: a hidden SVG has a 0×0 bounding box.
    surface.removeAttribute("hidden");
    const stageRect=surface.getBoundingClientRect();
    const imageRect=player().getBoundingClientRect();
    const scale=Math.min(imageRect.width/w,imageRect.height/h);
    if(!Number.isFinite(scale)||scale<=0||stageRect.width<=0||stageRect.height<=0){
      surface.setAttribute("hidden","");feedback();return;
    }
    const vx=(stageRect.left-imageRect.left)/scale;
    const vy=(stageRect.top-imageRect.top)/scale;
    const vw=stageRect.width/scale,vh=stageRect.height/scale;
    surface.setAttribute("viewBox",[vx,vy,vw,vh].join(" "));
    // Keep point markers comfortably clickable even when a portrait source
    // is displayed small inside a wide annotation workspace.
    const line=Math.max(2,w/450,1.8/scale);
    surface.appendChild(element("rect",{x:vx,y:vy,width:vw,height:vh,
      fill:"transparent","pointer-events":editing?"all":"none"}));
    if(editing)surface.appendChild(element("rect",{
      x:0,y:0,width:w,height:h,fill:"none",stroke:"#a5e98c",
      "stroke-width":line*2,"stroke-dasharray":(line*6)+" "+(line*4),
      "pointer-events":"none"
    }));
    for(const [a,b] of LINKS){
      if(!known(points[a])||!known(points[b]))continue;
      const estimate=points[a].status==="estimated"||points[b].status==="estimated";
      surface.appendChild(element("line",{x1:points[a].x,y1:points[a].y,
        x2:points[b].x,y2:points[b].y,stroke:estimate?"#ffb170":"#b0eb86",
        "stroke-width":line,"stroke-dasharray":estimate?"9 7":"none",
        "pointer-events":"none"}));
    }
    points.forEach((point,i)=>{
      if(!known(point))return;
      if(!editing&&(point.x<0||point.x>w||point.y<0||point.y>h))return;
      const estimated=point.status==="estimated";
      surface.appendChild(element("circle",{cx:point.x,cy:point.y,
        r:line*(editing&&i===index?5.6:4.1),
        fill:estimated?"#ffc181":"#c5ff8c",stroke:editing&&i===index?"#fff":"#0c1914",
        "stroke-width":line*1.2,"data-point":i}));
      surface.appendChild(element("text",{x:point.x+line*5,y:point.y-line*2,
        fill:estimated?"#ffdbb0":"#fff","font-size":line*9,"font-weight":"800",
        stroke:"#0a1511","stroke-width":line*1.5,
        "paint-order":"stroke","pointer-events":"none"},String(i+1)));
    });
    feedback();
  }
  function coords(e){
    const rect=overlay().getBoundingClientRect(),vb=overlay().viewBox.baseVal;
    const scale=Math.min(rect.width/vb.width,rect.height/vb.height);
    const ox=rect.left+(rect.width-vb.width*scale)/2;
    const oy=rect.top+(rect.height-vb.height*scale)/2;
    const x=vb.x+(e.clientX-ox)/scale,y=vb.y+(e.clientY-oy)/scale;
    if(!Number.isFinite(x)||!Number.isFinite(y)||
      x<vb.x||x>vb.x+vb.width||y<vb.y||y>vb.y+vb.height)return null;
    return {x:Math.round(x*1000)/1000,y:Math.round(y*1000)/1000,
      status:x<0||y<0||x>meta.width||y>meta.height?"estimated":"visible"};
  }
  function remember(){history.push(copy());if(history.length>30)history.shift();}
  function next(){
    for(let step=1;step<=8;step++){
      const i=(index+step)%8;
      if(!points[i]){index=i;return;}
    }
    // Already fully annotated: cycle through existing points for correction.
    index=(index+1)%8;
  }
  function markDirty(){dirty=true;frameSec=player().currentTime;}
  function toggle(){
    if(!meta)return;
    if(!editing){
      player().pause();
      if(saved&&Math.abs(player().currentTime-saved.frame_sec)>.5)
        player().currentTime=saved.frame_sec;
      frameSec=saved?.frame_sec??player().currentTime;
      index=points.findIndex(p=>!p);
      if(index<0)index=0;
      editing=true;
    }else{
      editing=false;
    }
    render();
  }
  function skip(){
    if(!editing)return;
    remember();
    points[index]={x:null,y:null,status:"unknown"};
    markDirty();next();render();
  }
  function init(){
    const s=overlay();
    s.addEventListener("pointerdown",e=>{
      if(!editing||!meta||e.button!==0)return;
      const p=coords(e);if(!p)return;
      e.preventDefault();e.stopPropagation();
      const selected=e.target.closest("[data-point]");
      if(selected)index=Number(selected.getAttribute("data-point"));
      remember();points[index]=p;markDirty();
      drag={id:e.pointerId,index};
      s.setPointerCapture(e.pointerId);render();
    });
    s.addEventListener("pointermove",e=>{
      if(!drag||drag.id!==e.pointerId)return;
      const p=coords(e);
      if(p){points[drag.index]=p;markDirty();render();}
    });
    function release(e){
      if(!drag||drag.id!==e.pointerId)return;
      const p=coords(e);
      if(p){points[drag.index]=p;markDirty();}
      drag=null;
      if(s.hasPointerCapture(e.pointerId))s.releasePointerCapture(e.pointerId);
      next();render();
    }
    s.addEventListener("pointerup",release);
    s.addEventListener("pointercancel",release);
    E("court-toggle").addEventListener("click",toggle);
    E("court-skip").addEventListener("click",skip);
    E("court-undo").addEventListener("click",()=>{
      if(!history.length)return;
      points=history.pop();dirty=true;
      index=points.findIndex(p=>!p);
      if(index<0)index=0;
      render();
    });
    E("court-clear").addEventListener("click",()=>{
      if(!editing)return;
      remember();points=Array(8).fill(null);index=0;dirty=true;render();
    });
    player().addEventListener("timeupdate",()=>{if(!editing&&meta)render();});
  }
  async function setVideo(id,name,info){
    const token=++seq;
    projectId=id;video=name;meta=info;editing=false;dirty=false;saved=null;
    points=Array(8).fill(null);history=[];index=0;render();
    if(!id||!name)return;
    try{
      const response=await fetch("/api/court?project_id="+encodeURIComponent(id)+
        "&video="+encodeURIComponent(name),{cache:"no-store"});
      const obj=await response.json();
      if(!response.ok)throw Error(obj.error||"Не удалось загрузить разметку");
      if(token!==seq)return;
      saved=obj.court;
      points=saved?saved.keypoints.map(p=>({
        x:p.x??null,y:p.y??null,status:p.status||(p.visible?"visible":"unknown")
      })):Array(8).fill(null);
      frameSec=saved?.frame_sec||0;render();
    }catch(error){if(token===seq)E("court-progress").textContent=error.message;}
  }
  function snapshot(){
    if(!meta)throw Error("Видео ещё не загружено");
    if(!dirty)return{court:null,clear_court:false};
    if(points.every(p=>p===null))return{court:null,clear_court:true};
    // Unmarked points have no inferred coordinates. Saving an incomplete
    // annotation is allowed and the rest are explicitly UNKNOWN.
    return{court:{
      frame_width:meta.width,frame_height:meta.height,frame_sec:frameSec,
      keypoints:NAMES.map((name,id)=>{
        const p=points[id]||{x:null,y:null,status:"unknown"};
        return{id,name,x:p.x,y:p.y,visible:p.status==="visible",status:p.status};
      })},clear_court:false};
  }
  function markSaved(court){
    saved=court||null;
    points=saved?saved.keypoints.map(p=>({
      x:p.x??null,y:p.y??null,status:p.status||(p.visible?"visible":"unknown")
    })):Array(8).fill(null);
    frameSec=saved?.frame_sec||0;dirty=false;history=[];editing=false;index=0;render();
  }
  function reset(){
    ++seq;meta=null;video="";projectId="";saved=null;points=Array(8).fill(null);
    editing=false;dirty=false;history=[];index=0;render();
  }
  return{init,setVideo,snapshot,markSaved,reset,isEditing:()=>editing,refreshGeometry:render};
})();
