const assert=require('node:assert/strict');
const tracks=require('../scripts/volleyball_ui/static/project-player-tracks.js');

async function main(){
  const scope=tracks.create({project:'p',caches:{'a.mp4':'cpu-a','b.mp4':'cpu-b'}});
  const calls=[];
  const request=async(url,body)=>{
    calls.push({url,body});
    if(url.startsWith('/api/report?'))return {
      raw:[{player_track_id:7,duration_sec:12}],replay_filtered_tracks:true,
      groups:url.includes('a.mp4')?[{id:'alice',name:'Alice',track_ids:[7],link_meta:{7:{reviewed:true}}}]:[]
    };
    return {ok:true};
  };
  const report=await scope.load(request);
  assert.equal(report.raw.length,2);
  const [a,b]=report.raw.map(row=>String(row.player_track_id));
  assert.notEqual(a,b);
  assert.deepEqual(report.groups[0].track_ids,[a]);
  assert.equal(scope.photoUrl(b,0).includes('video=b.mp4'),true);
  assert.equal(scope.photoUrl(b,0).includes('cache_id=cpu-b'),true);
  assert.equal(scope.photoUrl(b,0).includes('id=7'),true);
  calls.length=0;
  await scope.request('/api/assign',{project_id:'p',ids:[a,b],player_id:'alice'},request);
  assert.deepEqual(calls.map(call=>call.body),[
    {project_id:'p',ids:['7'],player_id:'alice',video:'a.mp4',cache_id:'cpu-a'},
    {project_id:'p',ids:['7'],player_id:'alice',video:'b.mp4',cache_id:'cpu-b'}
  ]);
  calls.length=0;
  await assert.rejects(()=>scope.request('/api/unassign',{ids:['unknown']},request));
  assert.equal(calls.length,0);
  const grouped=await scope.request('/api/player-groups?video=a.mp4',undefined,async()=>({status:'ready',groups:[{id:'g1',track_ids:[7,8]}],player_matches:{7:{similarity:.9}},auto_links:{alice:[7]}}));
  assert.equal(grouped.groups.length,2);
  assert.notEqual(grouped.groups[0].id,grouped.groups[1].id);
  assert.deepEqual(grouped.auto_links.alice,[a,b]);
  assert.equal(Object.keys(grouped.player_matches).length,2);
  const metadata=await scope.request('/api/player-photos?id='+encodeURIComponent(b),undefined,async url=>url);
  assert.equal(new URL(metadata,'http://localhost').searchParams.get('video'),'b.mp4');
  assert.equal(new URL(metadata,'http://localhost').searchParams.get('id'),'7');
  await assert.rejects(()=>scope.load(async()=>{throw Error('offline');}),/offline/);
  const empty=await tracks.create({project:'p',caches:{}}).load(request);
  assert.deepEqual(empty.raw,[]);
  console.log('project player tracks: OK');
}
main().catch(error=>{console.error(error);process.exitCode=1;});
