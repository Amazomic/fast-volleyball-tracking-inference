import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
import {runInNewContext} from "node:vm";

const root=join(fileURLToPath(new URL("../..",import.meta.url)));
const js=readFileSync(join(root,"rust/volley-app/static/studio.js"),"utf8");
const css=readFileSync(join(root,"rust/volley-app/static/studio.css"),"utf8");
const segment=(from,to)=>{
  const a=js.indexOf(from),b=js.indexOf(to,a);
  assert.ok(a>=0&&b>a,"Missing preview segment "+from);
  return js.slice(a,b);
};
const helpers=segment("  const length=item=>","  const valid=id=>");
const estimates=segment("  const estimatedTypes=item=>","  const view=item=>");
const renderer=segment("  const cards=()=>{","  const uploadAudio=async()=>{");
const element=(tag,klass,text)=>({
  tag,klass,textContent:text===undefined?"":String(text),children:[],listeners:{},
  append(...parts){this.children.push(...parts);},
  replaceChildren(...parts){this.children=parts;},
  addEventListener(name,fn){this.listeners[name]=fn;},
  setAttribute(){},
});
const list=element("div","rally-grid");
const ui={list,caption:{textContent:""},previous:{disabled:false},next:{disabled:false}};
const s={items:[{
  candidate_id:"a".repeat(24),detected_start_frame:1263,detected_end_frame:1592,
  motion_event_count:7,motion_supported_touch_proposals:7,
  provisional_track_ids:[4,4,9],ball_coverage:0.8,
  estimated_action_counts:{attack:3,dig:2},unclassified_action_count:2,
}],offset:0,limit:24,filtered:1,gold:new Set(),current:"",
selected:{has:()=>false}};
const context={fps:()=>30,ui,s,wasmReady:true,
  make:element, wipe:el=>el.replaceChildren(),
  valid:id=>/^[a-f0-9]{24}$/.test(id),clock:n=>String(n),
  document:{createTextNode:s=>({textContent:s})}, view(){},toggleGold(){},toggleSelected(){}};
runInNewContext(helpers+"\n"+estimates+"\n"+renderer+"\nthis.render=cards;",context);
context.render();
const card=list.children[0];
assert.equal(card.klass,"rally-card");
const values=(node)=>[node.textContent,...(node.children||[]).flatMap(values)];
const text=values(card).join(" ");
assert.match(text,/11\.0 с/,"duration should use every frame including final");
assert.match(text,/42,1–53,1 с/,
  "Start and end must show absolute seconds as a readable interval");
assert.match(text,/Касаний 7/);
assert.match(text,/Игроков 2/,"unique source Track IDs only");
assert.match(text,/Атака ×3/,"Show all provisional attacks in the card");
assert.match(text,/Приём снизу ×2/,"Show provisional digs in the card");
assert.match(css,/grid-template-areas:"head" "foot"/);
assert.match(css,/#studio-preview-view \.rally-card\{[\s\S]*?min-height:0;/,
  "Rally card must use the compact preview grid sizing");
assert.match(css,/#studio-preview-view \.rally-card-time-value\{/,
  "Absolute start and end seconds should share one readable line");
assert.match(css,/#studio-preview-view \.rally-card \.pick\{[\s\S]*?font-size:10px/,
  "Montage label must stay readable");
assert.match(text,/☆/,"Favorite toggle marks rallies for montage");
assert.match(css,/#studio-preview-view \.rally-card\.active\{[\s\S]*?inset 3px 0 #70aaff/,
  "Selected rally uses a restrained left rail, not a neon outline");
assert.match(css,/#studio-preview-view \.rally-card \.card-bottom\{[\s\S]*?display:flex/);
assert.match(css,/#studio-preview-view \.rally-card-time-range/);
s.items[0].provisional_track_ids=undefined;
context.render();
assert.match(values(list.children[0]).join(" "),/Игроков —/,
  "No Track ID evidence must not be represented as zero players");
// A fast bookmarks fetch / WASM initialization must never replace a pending
// CPU page load with the misleading "No rallies found" placeholder.
ui.filter={value:"all"};
s.run="r";s.total=76;s.filtered=76;s.items=[];
s.pageLoading=true;s.pageError="";
context.render();
assert.match(list.children[0].textContent,/Загрузка розыгрышей/i,
  "An in-flight CPU request is not an empty CPU result");
s.pageLoading=false;s.pageError="Source checksum mismatch";
context.render();
assert.match(list.children[0].textContent,/Source checksum mismatch/,
  "The real backend/render error remains visible");
s.pageError="";s.filtered=0;ui.filter.value="marked";
context.render();
assert.match(list.children[0].textContent,/нет избранных розыгрышей/i,
  "Empty favorites must not say the CPU analysis has no rallies");
ui.filter.value="all";s.total=0;
context.render();
assert.match(list.children[0].textContent,/нет обнаруженных розыгрышей/i,
  "Only the genuinely empty run may show the no-rallies message");
console.log("Studio rally cards: duration, actions, players and loading/empty states: OK");
