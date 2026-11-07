(function(root){
  'use strict';
  function create({project,caches}){
    const sources=Object.entries(caches).filter(([,cache])=>cache).map(([video,cache])=>({video,cache}));
    const contexts=new Map();
    const key=(source,id)=>JSON.stringify([source.video,String(id)]);
    function remember(source,id){
      const value=key(source,id);
      contexts.set(value,{project,video:source.video,cache:source.cache,id:String(id)});
      return value;
    }
    function context(id){return contexts.get(String(id));}
    function query(source){return new URLSearchParams({project_id:project,video:source.video,cache_id:source.cache});}
    function photoUrl(id,sample=0){
      const ctx=context(id);
      if(!ctx)return '';
      return '/api/player-photo?'+new URLSearchParams({project_id:project,video:ctx.video,cache_id:ctx.cache,id:ctx.id,sample:String(sample)});
    }
    async function load(send){
      const reports=await Promise.all(sources.map(async source=>({source,report:await send('/api/report?'+query(source))})));
      const raw=[],groups=new Map();
      for(const {source,report} of reports){
        for(const row of report.raw||[])raw.push({...row,player_track_id:remember(source,row.player_track_id),source_video:source.video,source_track_id:String(row.player_track_id),candidate_group_id:key(source,row.candidate_group_id||row.player_track_id)});
        for(const group of report.groups||[]){
          const merged=groups.get(group.id)||{...group,track_ids:[],link_meta:{}};
          for(const id of group.track_ids||[]){
            const value=remember(source,id);
            merged.track_ids.push(value);
            if(group.link_meta?.[id])merged.link_meta[value]=group.link_meta[id];
          }
          groups.set(group.id,merged);
        }
      }
      return {raw,groups:[...groups.values()],replay_filtered_tracks:reports.length>0&&reports.every(({report})=>report.replay_filtered_tracks),source_track_count:reports.reduce((sum,{report})=>sum+Number(report.source_track_count??report.raw?.length??0),0)};
    }
    function mapGroups(source,result){
      const ids=values=>(values||[]).map(id=>remember(source,id));
      const keyed=(object,convert=value=>value)=>Object.fromEntries(Object.entries(object||{}).map(([id,value])=>[remember(source,id),convert(value)]));
      return {...result,groups:(result.groups||[]).map(group=>({...group,id:key(source,group.id),track_ids:ids(group.track_ids)})),missing_visual_ids:ids(result.missing_visual_ids),player_matches:keyed(result.player_matches),nearest_tracks:keyed(result.nearest_tracks,rows=>rows.map(row=>({...row,track_id:remember(source,row.track_id)}))),auto_links:Object.fromEntries(Object.entries(result.auto_links||{}).map(([id,values])=>[id,ids(values)]))};
    }
    async function request(url,payload,send){
      const parsed=new URL(url,'http://localhost');
      const path=parsed.pathname;
      if(['/api/assign','/api/unassign','/api/players/exclude-tracks'].includes(path)&&payload){
        const batches=new Map();
        for(const id of payload.ids||[]){
          const ctx=context(id);
          if(!ctx)throw Error('Track ID не принадлежит текущему проекту');
          if(!batches.has(ctx.video))batches.set(ctx.video,{...payload,video:ctx.video,cache_id:ctx.cache,ids:[]});
          batches.get(ctx.video).ids.push(ctx.id);
        }
        let reply;
        const savedIds=[];
        for(const body of batches.values()){
          try{reply=await send(url,body);}
          catch(error){error.savedIds=savedIds;throw error;}
          savedIds.push(...body.ids.map(id=>key({video:body.video},id)));
        }
        return reply;
      }
      if(['/api/player-photos','/api/player-suggestions'].includes(path)){
        const ctx=context(parsed.searchParams.get('id'));
        if(!ctx)throw Error('Track ID не принадлежит текущему проекту');
        for(const [name,value] of Object.entries({video:ctx.video,cache_id:ctx.cache,id:ctx.id}))parsed.searchParams.set(name,value);
        const reply=await send(path+'?'+parsed.searchParams);
        return path==='/api/player-suggestions'?{...reply,candidates:(reply.candidates||[]).map(row=>({...row,track_id:remember({video:ctx.video,cache:ctx.cache},row.track_id)}))}:reply;
      }
      if(['/api/player-groups','/api/player-groups/start','/api/player-groups/threshold'].includes(path)){
        const results=await Promise.all(sources.map(async source=>mapGroups(source,await send(payload?path:path+'?'+query(source),payload?{...payload,video:source.video,cache_id:source.cache}:undefined))));
        const result={...results[0],status:results.some(row=>row.status==='error')?'error':results.some(row=>row.status==='running')?'running':results.every(row=>row.status==='ready')?'ready':results.find(row=>row.status!=='ready')?.status||'empty',groups:[],missing_visual_ids:[],player_matches:{},nearest_tracks:{},auto_links:{},threshold_percent:results[0]?.threshold_percent??85};
        result.auto_assign_enabled=results.length>0&&results.every(row=>row.auto_assign_enabled);
        result.progress=results.length?Math.round(results.reduce((sum,row)=>sum+Number(row.status==='ready'?100:row.progress||0),0)/results.length):0;
        result.error=results.filter(row=>row.error).map(row=>row.error).join('; ');
        for(const row of results){
          result.groups.push(...row.groups);result.missing_visual_ids.push(...row.missing_visual_ids);
          Object.assign(result.player_matches,row.player_matches);Object.assign(result.nearest_tracks,row.nearest_tracks);
          for(const [id,values] of Object.entries(row.auto_links))result.auto_links[id]=[...(result.auto_links[id]||[]),...values];
        }
        return result;
      }
      if(['/api/players/photos/pending','/api/players/photos/save'].includes(path)&&payload){
        const results=[];
        for(const source of sources)results.push(await send(path,{...payload,video:source.video,cache_id:source.cache}));
        if(path.endsWith('/save'))return {count:results.at(-1)?.count||0,copied:results.reduce((sum,row)=>sum+Number(row.copied||0),0),errors:results.flatMap(row=>row.errors||[])};
        const counts={};
        for(const row of results)for(const [id,count] of Object.entries(row.counts||{}))counts[id]=(counts[id]||0)+Number(count);
        return {counts};
      }
      return send(url,payload);
    }
    return {load,request,context,photoUrl,sources,key:JSON.stringify([project,caches])};
  }
  const api={create};
  if(typeof module==='object'&&module.exports)module.exports=api;
  else root.VolleyballProjectPlayerTracks=api;
})(globalThis);
