const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const {pathToFileURL}=require('node:url');
const {spawnSync}=require('node:child_process');
const chrome='C:/Program Files/Google/Chrome/Application/chrome.exe';
const root=path.resolve(__dirname,'..');
const dir=fs.mkdtempSync(path.join(root,'.players-layout-'));
const html=fs.readFileSync(path.join(root,'scripts/volleyball_ui/static/index.html'),'utf8');
const app=fs.readFileSync(path.join(root,'scripts/volleyball_ui/static/app.js'),'utf8');
const markupHelper=app.slice(app.indexOf('const renderedMarkup='),app.indexOf('function renderReport()'));
const checks=`
const firstPaintView=document.querySelector('#view-players').getBoundingClientRect();
document.querySelectorAll('.view').forEach(view=>view.classList.toggle('active',view.id==='view-players'));
const hydratedView=document.querySelector('#view-players').getBoundingClientRect();
const errors=[];
const check=(value,message)=>{if(!value)errors.push(message)};
if(matchMedia('(min-width:901px)').matches){
  for(const axis of ['left','top','width','height'])
    check(Math.abs(firstPaintView[axis]-hydratedView[axis])<=2,'Hydration shifted players view '+axis);
}
const panel=document.querySelector('.players-unified');
const aside=document.querySelector('.players-identity-panel');
const list=document.querySelector('.player-roster-scroll');
const named=document.querySelector('#named-list');
const add=document.querySelector('.player-add-disclosure');
const table=document.querySelector('#player-linked-tracks');
const rect=node=>node.getBoundingClientRect();
const health=document.querySelector('#players-health');
health.innerHTML=[['6','Игроков в проекте'],['161','Активных Track ID (CPU)'],['29','Привязано ID'],['132','Осталось разобрать']].map(([count,label])=>'<div class="player-health-metric"><strong>'+count+'</strong><span>'+label+'</span></div>').join('');
check(rect(health).bottom<=rect(panel).top,'Project counters are not above player workspace');
if(matchMedia('(min-width:901px)').matches)check(rect(health).top>=0&&rect(health).bottom<innerHeight,'Project counters are outside viewport');
check(getComputedStyle(health).display!=='none','Project counters are hidden');
check(!document.querySelector('#players-video'),'Video picker remains in project workspace');
check(!document.querySelector('.player-workspace-settings'),'Tools disclosure remains in project workspace');
const height=rect(aside).height;
if(matchMedia('(min-width:901px)').matches){
  const content=document.querySelector('.content');
  const initial=rect(content);
  const insets=getComputedStyle(content).padding;
  for(const id of ['analysis','analysis-results','players','report','benchmark','results']){
    document.documentElement.dataset.initialView=id;
    document.querySelectorAll('.view').forEach(view=>view.classList.toggle('active',view.id==='view-'+id));
    const active=document.querySelector('#view-'+id);
    check(getComputedStyle(content).padding===insets,'Inconsistent workspace padding in '+id);
    check(Math.abs(rect(content).width-initial.width)<2,'Inconsistent workspace width in '+id);
    check(Math.abs(rect(content).height-initial.height)<2,'Inconsistent workspace height in '+id);
    check(getComputedStyle(active).overflowY==='auto'||getComputedStyle(active).overflowY==='hidden','View has no contained scroll: '+id);
  }
  document.documentElement.dataset.initialView='players';
  document.querySelectorAll('.view').forEach(view=>view.classList.toggle('active',view.id==='view-players'));
}
if(matchMedia('(min-width:901px)').matches){
  const main=document.querySelector('.main');
  const sidebar=document.querySelector('.shell>.sidebar');
  const footer=main.querySelector('footer');
  check(rect(panel).left-rect(sidebar).right<=20,'Excessive unused space left of workspace');
  check(rect(main).right-rect(panel).right<=20,'Excessive unused space right of workspace');
  check(rect(footer).top-rect(panel).bottom<=20,'Excessive unused space below workspace');
  check(rect(panel).bottom<=rect(footer).top,'Workspace overlaps footer');
}
named.innerHTML=Array.from({length:40},(_,i)=>'<div class="player-roster-item"><button class="player-identity-slot"><span class="player-slot-avatar">'+i+'</span><span class="player-slot-text"><strong>Player '+i+'</strong><small>20 ID</small></span></button></div>').join('');
const rosterMarkup=named.innerHTML;
setMarkupIfChanged(named,rosterMarkup);
const firstCard=named.firstElementChild;
list.scrollTop=100;
setMarkupIfChanged(named,rosterMarkup);
check(named.firstElementChild===firstCard,'Unchanged roster recreated existing DOM nodes');
check(list.scrollTop===100,'Unchanged roster reset scroll');
check(Math.abs(rect(aside).height-height)<2,'Roster height changed after players loaded');
check(aside.offsetHeight>=Math.min(400,Math.max(200,panel.clientHeight-32)),'Empty tracks pane collapsed the roster');
check(list.scrollHeight>list.clientHeight,'Long roster has no internal scroll');
check(!list.contains(add),'Add card scrolls away with roster');
check(rect(add).top>=rect(aside).top&&rect(add).bottom<=rect(aside).bottom+1,'Add card is clipped');
const workspaceHeight=rect(panel).height;
table.innerHTML=Array.from({length:70},()=>'<tr><td><div style="height:190px">Photo</div></td><td>Track 123</td><td>20 s</td><td>Confirmed</td></tr>').join('');
check(Math.abs(rect(panel).height-workspaceHeight)<2,'Long tracks changed workspace height');
const scroll=table.closest('.table-scroll');
check(scroll.scrollHeight>scroll.clientHeight,'Long track gallery has no internal scroll');
list.scrollTop=list.scrollHeight;
check(rect(named.lastElementChild).bottom<=rect(list).bottom+1,'Last player cannot be reached');
check(rect(add).bottom<=rect(aside).bottom+1,'Add card disappeared after scrolling');
table.innerHTML='<tr><td class="empty">No IDs</td></tr>';
check(Math.abs(rect(aside).height-height)<2,'Empty tracks changed roster height');
add.open=true;
check(rect(add).bottom<=rect(aside).bottom+1,'Expanded add form overflows sidebar');
check(list.clientHeight>0,'Expanded add form consumes entire roster');
const manage=document.querySelector('#player-manage');
manage.hidden=false;
manage.open=true;
check(list.clientHeight>0,'Player management consumes entire roster');
for(const mode of ['manual','auto','view']){
  document.querySelectorAll('[data-player-pane]').forEach(pane=>{pane.hidden=pane.dataset.playerPane!==mode});
  check(Math.abs(rect(aside).height-height)<2,'Gallery tab '+mode+' changed roster height');
  check(rect(add).bottom<=rect(aside).bottom+1,'Gallery tab '+mode+' clipped add card');
}
const text=table.querySelector('td');
text.textContent='Long text '.repeat(200);
check(text.scrollHeight>text.clientHeight,'Long text does not scroll');
manage.open=false;
add.open=false;
document.body.insertAdjacentHTML('beforeend','<pre id="layout-results" hidden>'+JSON.stringify({errors,heights:{roster:height,workspace:workspaceHeight,asideOffset:aside.offsetHeight,panelClient:panel.clientHeight,viewport:innerHeight,shellH:rect(document.querySelector('.shell')).height,sidebarH:rect(document.querySelector('.shell>.sidebar')).height,mainH:rect(document.querySelector('.main')).height,footerH:rect(document.querySelector('.main footer')).height,bodyH:rect(document.body).height,rightGap:rect(document.querySelector('.main')).right-rect(panel).right,footerY:rect(document.querySelector('.main footer')).top,panelBottom:rect(panel).bottom,header:rect(document.querySelector('.header-dependency-chain')).height,view:rect(document.querySelector('#view-players')).height,health:rect(health).height,intro:rect(document.querySelector('#view-players>.section-intro')).height,content:rect(document.querySelector('.content')).height}})+'</pre>');
`;
try{
  const fixture=path.join(dir,'fixture.html');
  const page=html.replace('<html lang="ru">','<html lang="ru" data-initial-view="players">').replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi,'').replace(/href="\/([^"?]+\.css)(?:\?[^" ]*)?"/g,(_,name)=>'href="'+pathToFileURL(path.join(root,'scripts/volleyball_ui/static',name)).href+'"');
  fs.writeFileSync(fixture,page.replace('</body>','<script>'+markupHelper+checks+'</script></body>'));
  for(const size of ['1440,900','1440,1000','1920,1080','800,900','480,812']){
    const result=spawnSync(chrome,['--headless=new','--no-sandbox','--disable-gpu','--disable-extensions','--no-first-run','--no-default-browser-check','--user-data-dir='+path.join(dir,'profile-'+size.replace(',','-')),'--window-size='+size,'--dump-dom',...(size==='1440,900'&&process.env.PLAYERS_LAYOUT_SCREENSHOT?['--screenshot='+process.env.PLAYERS_LAYOUT_SCREENSHOT]:[]),pathToFileURL(fixture).href],{encoding:'utf8',timeout:45000,maxBuffer:3*1024*1024,windowsHide:true});
    if(result.error)throw result.error;
    const match=Array.from(result.stdout.matchAll(/<pre id="layout-results"[^>]*>([^<]+)<\/pre>/g)).at(-1);
    assert(match,'Chrome did not return layout results: '+result.stderr.slice(-600));
    const report=JSON.parse(match[1]);
    console.log(size,JSON.stringify(report));
    assert.deepEqual(report.errors,[]);
  }
  console.log('PASS: stable roster, visible add card and independent gallery scroll in Chrome');
}finally{
  assert(path.resolve(dir).startsWith(path.resolve(root)+path.sep));
  fs.rmSync(dir,{recursive:true,force:true});
}
