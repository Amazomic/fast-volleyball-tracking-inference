"use strict";
// Pure, testable selector model. Native stage metadata owns provenance;
// a CPU snapshot can only appear under its own GPU fusion lineage.
((root)=>{
  const id=x=>typeof x==="string"&&/^[a-f0-9]{32}$/.test(x);
  const validVideo=x=>typeof x==="string"&&x.length>0;
  const key=(video,kind,run)=>JSON.stringify([video,kind,run]);
  const stamp=seconds=>Number.isFinite(Number(seconds))&&Number(seconds)>0?
    new Date(Number(seconds)*1000).toLocaleString("ru-RU",{
      day:"2-digit",month:"2-digit",year:"2-digit",hour:"2-digit",minute:"2-digit"
    }):"";
  const stage=(overview,name)=>Array.isArray(overview?.stages)?
    overview.stages.find(item=>item.key===name)?.items||[]:[];
  const build=(videos,overviews,runs)=>{
    const choices=[],byAnalysis=new Map(),runToAnalysis=new Map();
    const allRuns=Array.isArray(runs)?runs:[];
    for(const video of (Array.isArray(videos)?videos:[])){
      if(!validVideo(video))continue;
      const overview=overviews?.get(video);
      const fusion=stage(overview,"fusion").filter(row=>id(row.id));
      const gpu=stage(overview,"gpu").filter(row=>id(row.id));
      const replays=stage(overview,"tracking").filter(row=>id(row.id));
      const replayFusion=new Map(replays.filter(row=>id(row.source_fusion_id)).
        map(row=>[row.id,row.source_fusion_id]));
      const fusionKeys=new Map();
      const add=(kind,run,label)=>{
        const value=key(video,kind,run);
        if(byAnalysis.has(value))return value;
        choices.push({value,video,kind,id:run,label});
        byAnalysis.set(value,[]);
        return value;
      };
      for(const row of fusion){
        const value=add("fusion",row.id,video+" · RAVEL + VballNet · "+(stamp(row.created_at)||row.id.slice(0,8)));
        fusionKeys.set(row.id,value);
      }
      const fusedGpuIds=new Set(fusion.flatMap(row=>
        [row.ravel_job_id,row.vball_job_id].filter(id)));
      for(const row of gpu){
        if(fusedGpuIds.has(row.id))continue;
        const model=row.model==="ball"?"VballNet":row.model==="players"?"RAVEL":"GPU";
        add("gpu",row.id,video+" · "+model+" (не объединён) · "+
          (stamp(row.created_at)||row.id.slice(0,8)));
      }
      for(const run of allRuns.filter(row=>row.video===video&&id(row.rally_run_id))){
        const source=replayFusion.get(run.source_replay_id);
        let value=source?fusionKeys.get(source):null;
        if(!value){
          // Never invent an upstream fusion for incomplete/missing metadata.
          value=add("unlinked",run.source_replay_id||run.rally_run_id,
            video+" · GPU-источник не установлен");
        }
        byAnalysis.get(value).push(run);
        runToAnalysis.set(run.rally_run_id,value);
      }
    }
    return {choices,byAnalysis,runToAnalysis};
  };
  const api={build,key,stamp};
  if(root)root.VolleyStudioChain=api;
  if(typeof module==="object"&&module.exports)module.exports=api;
})(typeof window!=="undefined"?window:null);
