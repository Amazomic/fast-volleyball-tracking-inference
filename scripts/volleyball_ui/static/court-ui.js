"use strict";
window.VolleyballCourtUI=(()=>{
  const E=id=>document.getElementById(id);
  const NS="http://www.w3.org/2000/svg";
  const NAMES=["1_back_left","2_back_left","3_back_right","4_back_right","5_center_left","6_center_right","7_net_left","8_net_right"];
  const LABELS=["Ближний левый","Дальний левый","Дальний правый","Ближний правый","Средняя слева","Средняя справа","Сетка слева","Сетка справа"];
  const LINKS=[[0,4],[4,1],[1,2],[2,5],[5,3],[3,0],[4,5],[4,6],[6,7],[7,5]];
  const PAD=.3;
  let meta=null,video="",projectId="",saved=null,points=Array(8).fill(null);
  let mode="view",selected=0,dirty=false,frameSec=0,loadSeq=0,drag=null,history=[];
  const svg=()=>E("court-overlay"),player=()=>E("source-video-player"),stage=()=>E("source-video-player").parentElement;
  const clone=()=>points.map(p=>p?{...p}:null);
  const known=p=>p&&p.status!=="unknown"&&Number.isFinite(p.x)&&Number.isFinite(p.y);
  const editing=()=>mode!=="view";
  function mark(){dirty=true;frameSec=player().currentTime;}
  function make(tag,attrs={},label=null){
    const element=document.createElementNS(NS,tag);
    for(const [k,v] of Object.entries(attrs))element.setAttribute(k,String(v));
    if(label!==null)element.textContent=label;
    return element;
  }
  function progress(){
    const count=points.filter(Boolean).length;
    E("court-progress").textContent=editing()?
      (count+"/8 · "+(dirty?"не сохранено":"разметка")):
      (saved?(saved.keypoints.filter(p=>p.status==="estimated").length+
        " за кадром · "+saved.keypoints.filter(p=>p.status==="unknown").length+" неизвестны"):
        "Площадка не размечена");
    E("court-point-wrap").hidden=!editing();
    E("court-undo").hidden=!editing();
    E("court-clear").hidden=!editing();
    E("court-mark-unknown").hidden=mode!=="unknown";
    E("court-instruction").hidden=!editing();
    E("court-overlay-badge").hidden=!(editing()||saved);
    E("court-point-prompt").textContent="Точка "+(selected+1)+"/8: "+LABELS[selected];
    E("court-mode-hint").textContent=mode==="visible"?"Нажмите на точку в видео":
      mode==="outside"?"Видео уменьшено: нажмите за его границей":
      mode==="unknown"?"Отметьте неизвестной без указания координат":"";
    E("court-point-select").value=String(selected);
    E("court-undo").disabled=history.length===0;
    for(const option of E("court-point-select").options){
      const i=Number(option.value),p=points[i];
      const icon=!p?"○":p.status==="visible"?"✓":p.status==="estimated"?"≈":"?";
      option.textContent=icon+" "+(i+1)+" · "+LABELS[i];
    }
  }
  function render(){
    const s=svg();s.replaceChildren();
    s.classList.toggle("is-editing",editing());
    stage().classList.toggle("court-outside-mode",mode==="outside");
    if(!meta){s.setAttribute("hidden","");progress();return;}
    const w=meta.width,h=meta.height,extra=mode==="outside"?PAD:0;
    s.setAttribute("viewBox",[-extra*w,-extra*h,w*(1+extra*2),h*(1+extra*2)].join(" "));
    s.toggleAttribute("hidden",!(editing()||!!saved&&Math.abs(player().currentTime-(saved.frame_sec||0))<1.5));
    const line=Math.max(2,w/450);
    s.appendChild(make("rect",{x:-extra*w,y:-extra*h,width:w*(1+2*extra),
      height:h*(1+2*extra),fill:"transparent","pointer-events":editing()?"all":"none"}));
    if(mode==="outside")s.appendChild(make("rect",{x:0,y:0,width:w,height:h,fill:"none",
      stroke:"#b4eb98","stroke-width":line*2,"stroke-dasharray":(line*6)+" "+(line*4),
      "pointer-events":"none"}));
    for(const [a,b] of LINKS){
      if(!known(points[a])||!known(points[b]))continue;
      const uncertain=points[a].status==="estimated"||points[b].status==="estimated";
      s.appendChild(make("line",{x1:points[a].x,y1:points[a].y,x2:points[b].x,y2:points[b].y,
        stroke:uncertain?"#ffb36f":"#9cf075","stroke-width":line,
        "stroke-dasharray":uncertain?"9 7":"none","pointer-events":"none"}));
    }
    for(let i=0;i<8;i++){
      const p=points[i];if(!known(p))continue;
      if((p.x<0||p.x>w||p.y<0||p.y>h)&&mode!=="outside")continue;
      const estimated=p.status==="estimated";
      s.appendChild(make("circle",{cx:p.x,cy:p.y,r:line*(i===selected&&editing()?5.7:4),
        fill:estimated?"#ffc281":"#bff987",stroke:i===selected&&editing()?"#fff":"#0f1c17",
        "stroke-width":line*1.2,"data-point":i}));
      s.appendChild(make("text",{x:p.x+line*6,y:p.y-line*3,
        fill:estimated?"#ffdbaf":"#fff","font-size":line*9,"font-weight":"800",
        stroke:"#081510","stroke-width":line*1.5,"paint-order":"stroke",
        "pointer-events":"none"},String(i+1)));
    }
    progress();
  }
  function coordinate(e){
    const rect=svg().getBoundingClientRect(),vb=svg().viewBox.baseVal;
    const scale=Math.min(rect.width/vb.width,rect.height/vb.height);
    const x0=rect.left+(rect.width-vb.width*scale)/2;
    const y0=rect.top+(rect.height-vb.height*scale)/2;
    const x=vb.x+(e.clientX-x0)/scale,y=vb.y+(e.clientY-y0)/scale;
    if(!Number.isFinite(x)||!Number.isFinite(y)||
       x<vb.x||x>vb.x+vb.width||y<vb.y||y>vb.y+vb.height)return null;
    const outside=x<0||y<0||x>meta.width||y>meta.height;
    if(mode==="visible"&&outside)return null;
    if(mode==="outside"&&!outside)return null;
    return{x:Math.round(x*1000)/1000,y:Math.round(y*1000)/1000,
      status:outside?"estimated":"visible"};
  }
  function pushHistory(){history.push(clone());if(history.length>30)history.shift();}
  function advance(){
    for(let step=1;step<=8;step++){
      const index=(selected+step)%8;
      if(!points[index]){selected=index;return;}
    }
  }
  function assignPoint(point){
    pushHistory();points[selected]=point;mark();advance();render();
  }
  function setMode(next){
    if(!meta){E("court-mode-select").value="view";return;}
    if(!["view","visible","outside","unknown"].includes(next))return;
    if(mode==="view"&&next!=="view"){
      player().pause();
      if(saved&&Math.abs(player().currentTime-saved.frame_sec)>.5)
        player().currentTime=saved.frame_sec;
      frameSec=saved?saved.frame_sec:player().currentTime;
    }
    mode=next;E("court-mode-select").value=mode;render();
  }
  function init(){
    const surface=svg();
    surface.addEventListener("pointerdown",e=>{
      if(!editing()||!meta||e.button!==0)return;
      if(mode==="unknown"){e.preventDefault();assignPoint({x:null,y:null,status:"unknown"});return;}
      const pt=coordinate(e);if(!pt)return;
      e.preventDefault();e.stopPropagation();
      const picked=e.target.closest("[data-point]");
      if(picked)selected=Number(picked.getAttribute("data-point"));
      pushHistory();points[selected]=pt;mark();drag={id:e.pointerId,index:selected};
      surface.setPointerCapture(e.pointerId);render();
    });
    surface.addEventListener("pointermove",e=>{
      if(!drag||drag.id!==e.pointerId)return;
      const pt=coordinate(e);
      if(pt){points[drag.index]=pt;mark();render();}
    });
    function release(e){
      if(!drag||drag.id!==e.pointerId)return;
      const index=drag.index,pt=coordinate(e);
      if(pt){points[index]=pt;mark();}
      drag=null;
      if(surface.hasPointerCapture(e.pointerId))surface.releasePointerCapture(e.pointerId);
      advance();render();
    }
    surface.addEventListener("pointerup",release);
    surface.addEventListener("pointercancel",release);
    E("court-mode-select").addEventListener("change",e=>setMode(e.target.value));
    E("court-point-select").addEventListener("change",e=>{selected=Number(e.target.value);render();});
    E("court-mark-unknown").addEventListener("click",()=>{
      if(mode==="unknown")assignPoint({x:null,y:null,status:"unknown"});
    });
    E("court-undo").addEventListener("click",()=>{
      if(!history.length)return;points=history.pop();dirty=true;render();
    });
    E("court-clear").addEventListener("click",()=>{
      if(!editing())return;
      pushHistory();points=Array(8).fill(null);selected=0;mark();render();
    });
    player().addEventListener("timeupdate",()=>{if(!editing()&&meta)render();});
  }
  async function setVideo(id,name,info){
    const seq=++loadSeq;
    projectId=id;video=name;meta=info;mode="view";selected=0;
    dirty=false;saved=null;points=Array(8).fill(null);history=[];
    E("court-mode-select").value="view";render();
    if(!id||!name)return;
    try{
      const resp=await fetch("/api/court?project_id="+encodeURIComponent(id)+
        "&video="+encodeURIComponent(name),{cache:"no-store"});
      const data=await resp.json();
      if(!resp.ok)throw Error(data.error||"Ошибка чтения разметки");
      if(seq!==loadSeq)return;
      saved=data.court;
      points=saved?saved.keypoints.map(p=>({
        x:p.x??null,y:p.y??null,status:p.status||(p.visible?"visible":"unknown")
      })):Array(8).fill(null);
      frameSec=saved?.frame_sec||0;render();
    }catch(error){
      if(seq===loadSeq)E("court-progress").textContent="Ошибка разметки: "+error.message;
    }
  }
  function snapshot(){
    if(!meta)throw Error("Видео ещё загружается");
    if(!dirty)return{court:null,clear_court:false};
    const missing=points.findIndex(p=>!p);
    if(missing>=0&&points.some(Boolean))
      throw Error("Точка "+(missing+1)+" не отмечена. Укажите её или выберите «Неизвестная точка».");
    if(points.every(p=>!p))return{court:null,clear_court:true};
    return{
      court:{frame_width:meta.width,frame_height:meta.height,frame_sec:frameSec,
        keypoints:NAMES.map((name,id)=>{
          const p=points[id];
          return{id,name,x:p.x,y:p.y,visible:p.status==="visible",status:p.status};
        })},clear_court:false
    };
  }
  function markSaved(court){
    saved=court||null;
    points=saved?saved.keypoints.map(p=>({
      x:p.x??null,y:p.y??null,status:p.status||(p.visible?"visible":"unknown")
    })):Array(8).fill(null);
    frameSec=saved?.frame_sec||0;dirty=false;history=[];mode="view";
    E("court-mode-select").value="view";render();
  }
  function reset(){
    ++loadSeq;projectId="";video="";meta=null;saved=null;points=Array(8).fill(null);
    mode="view";dirty=false;history=[];E("court-mode-select").value="view";render();
  }
  return{init,setVideo,snapshot,markSaved,reset};
})();
