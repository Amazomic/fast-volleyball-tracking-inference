"use strict";
const assert=require("node:assert/strict");
const fs=require("node:fs");
const os=require("node:os");
const path=require("node:path");
const vm=require("node:vm");
const {spawnSync}=require("node:child_process");
const {pathToFileURL}=require("node:url");
const root=path.resolve(__dirname,"..");
const assets=path.join(root,"scripts/volleyball_ui/static");
const chrome="C:/Program Files/Google/Chrome/Application/chrome.exe";
const source=fs.readFileSync(path.join(assets,"app.js"),"utf8");
const elements=new Map();
const get=id=>{
  if(!elements.has(id))elements.set(id,{
    textContent:"",value:"",innerHTML:"",disabled:false,
    insertAdjacentHTML(_position,html){this.innerHTML+=html}
  });
  return elements.get(id);
};
const context=vm.createContext({
  window:{},document:{querySelector:get,querySelectorAll:()=>[],activeElement:null,addEventListener(){}},
  URLSearchParams,console,setTimeout,clearTimeout
});
vm.runInContext(source,context,{filename:"app.js"});
vm.runInContext([
  'analysisCacheId=()=>"gpu";',
  'state.project="project";state.video="match.mp4";',
  'state.players=[{id:"p1",name:"Irek"},{id:"p2",name:"Ilnur"}];',
  'state.report={raw:Array.from({length:144},(_,i)=>({player_track_id:101+i})),groups:[]};',
  'state.playerGroupsKey=playerGroupsContext();',
  'state.playerGroups={status:"ready",threshold_percent:85,',
  'groups:Array.from({length:62},(_,i)=>({id:"g"+i,track_ids:[String(101+i*2),String(102+i*2)],similarity:0.92,source:"automatic"})),',
  'player_matches:Object.fromEntries(Array.from({length:124},(_,i)=>[String(101+i),',
  '{player_id:"p1",name:"Irek",similarity:.92,above_threshold:true,ambiguous:false}]))};',
  'renderPlayerGroups();'
].join("\n"),context);
const markup=get("#player-groups-list").innerHTML;
assert.match(markup,/class="player-appearance-main-id"/);
assert(!markup.includes('class="player-appearance-members"'),"Track list must be removed from cards");
assert(!markup.includes('class="player-reid-pick-hint"'),"ReID suggestion text must be removed from cards");
assert(!markup.includes('class="player-appearance-pick-caption"'),"Player selection heading must be removed");
assert.match(markup,/class="group-player-picker"/);
assert.match(markup,/data-group-link="g0"/);
assert.match(markup,/data-photo-track-id="101"/);
const dir=fs.mkdtempSync(path.join(os.tmpdir(),"players-group-card-"));
try{
  const fixture=path.join(dir,"fixture.html");
  const checks=`
  const errors=[];
  const check=(ok,message)=>{if(!ok)errors.push(message)};
  const boxes=[...document.querySelectorAll('.player-appearance-card')];
  const size=node=>node.getBoundingClientRect();
  const gallery=document.querySelector('.player-groups-list');
  const threshold=document.querySelector('.player-reid-controls');
  const recalculate=document.querySelector('.player-recalculate-bar');
  const groupRecalculate=document.querySelector('#player-groups-rebuild');
  const autoPane=document.querySelector('[data-player-pane="auto"]');
  check(boxes.length===62,'62 card fixture missing');
  for(const card of boxes){
    const image=card.querySelector('img');
    const select=card.querySelector('select');
    const button=card.querySelector('[data-group-link]');

    check(size(image).width>=size(card).width-22,'Photo is not full-width');
    check(size(image).height>=160,'Photo is too short');
    check(size(select).width>=size(card).width-22,'Player select is too narrow');
    check(size(button).width>=size(card).width-22,'Link button is too narrow');
    check(size(button).top>size(select).bottom-1,'Controls overlap');
    check(size(select).bottom<=size(card).bottom,'Select outside card');
    check(size(button).bottom<=size(card).bottom+1,'Button outside card');
    check(size(card).height<445,'Long ID text enlarges the card');

    check(getComputedStyle(image).objectFit==='contain','Photo crop mode incorrect');
    check(card.scrollWidth<=card.clientWidth+2,'Horizontal overflow within card');
  }
  check(gallery.scrollWidth<=gallery.clientWidth+2,'Gallery horizontal overflow');
  const firstRow=boxes.filter(card=>Math.abs(size(card).top-size(boxes[0]).top)<2).length;
  if(innerWidth>=1300)check(firstRow===6,'Desktop must show six cards per row, got '+firstRow);
  check(firstRow<boxes.length,'Cannot find second row');
  check(size(boxes[firstRow]).top>=size(boxes[0]).bottom+6,'Rows overlap');
  check(size(threshold).top>=size(gallery).bottom-2,'ReID threshold must be below group list');
  check(size(threshold).height>40,'Threshold settings are hidden');
  check(size(groupRecalculate).top>=size(gallery).bottom-2,'Group recompute must be below list');
  check(size(recalculate).top>=size(autoPane).bottom-2,'Touch recalculation must be below group pane');
  check(size(recalculate).height>20,'Touch recalculation was collapsed');
  check(getComputedStyle(gallery).gridAutoRows==='max-content','Grid rows must fit the entire card content');
  document.body.insertAdjacentHTML('beforeend','<pre id="check-results">'+JSON.stringify({errors,width:innerWidth,cards:boxes.length,cardHeight:Math.round(size(boxes[0]).height),cardsPerRow:firstRow,photoHeight:Math.round(size(boxes[0].querySelector('img')).height)})+'</pre>');
  `;

  const index=fs.readFileSync(path.join(assets,"index.html"),"utf8");
  const page=index
    .replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi,"")
    .replace(/href="\/([^"?]+\.css)(?:\?[^" ]*)?"/g,(_,name)=>'href="'+pathToFileURL(path.join(assets,name)).href+'"')
    .replace('<html lang="ru">','<html lang="ru" data-initial-view="players">');
  const inject=
    "document.querySelectorAll('.view').forEach(view=>view.classList.toggle('active',view.id==='view-players'));"+
    "document.querySelectorAll('[data-player-pane]').forEach(pane=>pane.hidden=pane.dataset.playerPane!=='auto');"+
    "document.querySelector('#player-groups-list').innerHTML="+JSON.stringify(markup)+";";
  fs.writeFileSync(fixture,page.replace('</body>','<script>'+inject+checks+'</script></body>'),"utf8");
  for(const resolution of ["1608,508","1440,560","1440,900","1920,1080","800,900","480,812"]){
    const name=resolution.replace(",","-");
    const result=spawnSync(chrome,[
      "--headless=new","--no-sandbox","--disable-gpu","--disable-extensions",
      "--no-first-run","--no-default-browser-check",
      "--user-data-dir="+path.join(dir,"profile-"+name),
      "--window-size="+resolution,"--dump-dom",pathToFileURL(fixture).href
    ],{encoding:"utf8",timeout:40000,maxBuffer:3*1024*1024,windowsHide:true});
    if(result.error)throw result.error;
    const resultMatch=[...result.stdout.matchAll(/<pre id="check-results">([^<]+)<\/pre>/g)].at(-1);
    assert(resultMatch,"Chrome failed to render cards: "+result.stderr.slice(-500));
    const report=JSON.parse(resultMatch[1]);
    assert.deepEqual(report.errors,[],resolution+": "+report.errors.join("; "));
    console.log(resolution,JSON.stringify(report));
  }
  console.log("PASS: ReID cards display large photos, clear IDs and full-width player controls");
}finally{
  fs.rmSync(dir,{recursive:true,force:true});
}
