"use strict";
/* Pure navigation model: project -> original inference (video + run) -> CPU replay.
 * Each run can contain multiple video caches; the primary inference choice
 * identifies both dimensions without a second video dropdown. */
(function (global) {
  const owns=(object,key)=>Boolean(object&&Object.prototype.hasOwnProperty.call(object,key));
  function originalRuns(runs,video){
    return (runs||[]).filter(run=>!run.parent_run_id&&owns(run.videos,video));
  }
  function branches(runs,video,parentId){
    const original=originalRuns(runs,video).find(run=>run.id===parentId);
    if(!original)return [];
    return [original,...(runs||[]).filter(run=>
      run.parent_run_id===parentId&&owns(run.videos,video))];
  }
  function selected(runs,video,preferredId){
    const possible=originalRuns(runs,video);
    if(!possible.length)return {parent:null,version:null};
    const found=(runs||[]).find(run=>run.id===preferredId&&owns(run.videos,video));
    const parent=possible.find(run=>run.id===(found?.parent_run_id||found?.id))||possible[0];
    const version=branches(runs,video,parent.id).find(run=>run.id===found?.id)||parent;
    return {parent,version};
  }
  function clock(seconds){
    if(!Number.isFinite(Number(seconds))||Number(seconds)<0)return "";
    const n=Math.floor(Number(seconds));
    return [Math.floor(n/3600),Math.floor(n%3600/60),n%60]
      .map(v=>String(v).padStart(2,"0")).join(":");
  }
  function rangeText(range){
    if(!range||range.start==null)return "диапазон не записан";
    const start=Number(range.start), end=range.end==null?null:Number(range.end);
    if(!Number.isFinite(start))return "диапазон не записан";
    if(end===null)return "от "+clock(start)+" · конец не записан";
    if(start===0&&!(end>0))return "всё видео";
    return "фрагмент "+clock(start)+"–"+(end>start?clock(end):"до конца");
  }
  function initialLabel(run,video){
    if(!run)return "Нет первичного анализа";
    const range=run.video_ranges?.[video];
    const datetime=run.started_at?new Date(run.started_at):null;
    const date=datetime&&!Number.isNaN(datetime.getTime())
      ?datetime.toLocaleString(documentLocale(),{day:"2-digit",month:"2-digit",hour:"2-digit",minute:"2-digit"})
      :"";
    const cache=run.videos?.[video]||"";
    return [run.display_name||run.profile,rangeText(range),date,
      "запуск "+run.id.slice(-6),cache?"кэш "+cache.slice(-6):""].filter(Boolean).join(" · ");
  }
  function documentLocale(){
    return typeof document!=="undefined"&&document.documentElement.lang==="en"?"en-US":"ru-RU";
  }
  const modeNames={
    baseline:"VballNet + геометрия",
    "ravel-touch":"RAVEL Touch",
    "ravel-geometry":"RAVEL + геометрия",
    combined:"VballNet + RAVEL Touch + геометрия"
  };
  function versionLabel(run){
    if(!run)return "Нет обработки";
    const settings=run.postprocess_settings;
    if(run.display_name)return run.display_name;
    if(!settings)return run.inference_only
      ?"Первичный инференс нейросетей · CPU ещё не выполнен"
      :"Старый полный анализ · треки, касания и розыгрыши";
    const parts=[modeNames[settings.mode]||settings.mode];
    for(const [key,name] of [["ball_threshold","мяч"],["player_threshold","игроки"],
                             ["touch_threshold","Touch"]]){
      const value=settings[key];
      if(value!=null&&(key!=="touch_threshold"||
         ["ravel-touch","combined"].includes(settings.mode)))
        parts.push(name+" "+Math.round(Number(value)*100)+"%");
    }
    return parts.join(" · ");
  }
  function choiceId(video,runId){
    return encodeURIComponent(video)+"|"+runId;
  }
  function parseChoiceId(value){
    const boundary=String(value||"").lastIndexOf("|");
    if(boundary<0)return {video:"",runId:""};
    try{return {video:decodeURIComponent(value.slice(0,boundary)),
                runId:value.slice(boundary+1)}}
    catch{return {video:"",runId:""}}
  }
  const api={originalRuns,branches,selected,rangeText,initialLabel,versionLabel,
             choiceId,parseChoiceId};
  global.VolleyballHeaderChain=api;
  if(typeof module!=="undefined"&&module.exports)module.exports=api;
})(typeof window!=="undefined"?window:globalThis);
