import {readFileSync} from "node:fs";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
import {runInNewContext} from "node:vm";
import assert from "node:assert/strict";

const root=join(fileURLToPath(new URL("../..",import.meta.url)));
const html=readFileSync(join(root,"rust/volley-app/static/studio.html"),"utf8");
const css=readFileSync(join(root,"rust/volley-app/static/studio.css"),"utf8");
const js=readFileSync(join(root,"rust/volley-app/static/studio.js"),"utf8");

const preview=html.slice(html.indexOf('id="studio-preview-view"'),
  html.indexOf('id="studio-montage-view"'));
assert.match(preview, /^id="studio-preview-view" aria-label="Предпросмотр матча"/,
  "Preview must expose an accessible label without a redundant visible header");
assert.doesNotMatch(preview, /class="intro"|id="studio-preview-heading"|id="studio-total"|id="studio-gold-count"/,
  "Preview must not render the top heading and statistics panel");
assert.doesNotMatch(js, /ui\.(?:total|goldCount)\.textContent/,
  "Removed preview counters must not be updated by JavaScript");
assert.ok(preview.indexOf('class="viewer-panel"')<preview.indexOf('class="rallies-panel"'),
  "Large player must precede the rally rail on the right");
assert.equal((preview.match(/id="studio-video"/g)||[]).length,1);
assert.equal((preview.match(/id="studio-rallies"/g)||[]).length,1);
assert.match(css, /#studio-preview-view \.viewer-layout\{\s*grid-template-columns:minmax\(0,1fr\)/);
assert.match(css, /#studio-preview-view \.rally-grid\{[\s\S]*?overflow-y:auto/);
assert.match(css, /@media\(max-width:780px\)\{[\s\S]*?#studio-preview-view \.viewer-layout\{\s*grid-template-columns:minmax\(0,1fr\)/);

const start=js.indexOf('  const previewCache=(()=>{');
const end=js.indexOf('  const make=(tag,klass,text)=>{',start);
assert.ok(start>=0&&end>start,"Cache must be declared in the Studio adapter");
const source=js.slice(start,end)+
  "\nthis.cache=previewCache;this.invalidateRun=invalidatePreviewRun;this.invalidateAll=invalidateCatalog;";

const values=new Map();
const sessionStorage={
  get length(){return values.size;},
  key(i){return [...values.keys()][i]??null;},
  getItem(k){return values.get(k)??null;},
  setItem(k,v){values.set(k,String(v));},
  removeItem(k){values.delete(k);}
};
let now=1000,requests=0,chainInvalidated=0;
const request=async path=>{
  requests++;
  await Promise.resolve(); // concurrent calls must share one request
  return {path,sequence:requests};
};
const loadContext=()=>{
  const context={request,sessionStorage,encodeURIComponent,Date:{now:()=>now},
    chainSnapshot:{drop:()=>{chainInvalidated++;}}};
  runInNewContext(source,context,{filename:"studio-preview-cache.js"});
  return context;
};

const previewAll="/api/cpu/native/rallies/preview?project_id=projectA&rally_run_id=run1&offset=0&limit=24&gold_filter=all";
const previewOther="/api/cpu/native/rallies/preview?project_id=projectA&rally_run_id=run2&offset=0&limit=24&gold_filter=all";
const overview="/api/studio/analysis-overview?project_id=projectA&video=match.mp4";
const first=loadContext();
await Promise.all([
  first.cache.load(previewAll,900000,true),
  first.cache.load(previewAll,900000,true),
  first.cache.load(previewAll,900000,true)
]);
assert.equal(requests,1,"Concurrent preview loads should be deduplicated");
await first.cache.load(previewOther,900000,true);
await first.cache.load(overview,60000,true);
assert.equal(requests,3);
const afterReload=loadContext();
await afterReload.cache.load(previewAll,900000,true);
assert.equal(requests,3,"Session refresh should reuse preview metadata");
afterReload.invalidateRun("projectA","run1");
await afterReload.cache.load(previewOther,900000,true);
assert.equal(requests,3,"Changing gold marks must not invalidate another CPU branch");
await afterReload.cache.load(previewAll,900000,true);
assert.equal(requests,4,"Gold revisions must invalidate matching cached preview");
afterReload.invalidateAll();
assert.equal(chainInvalidated,1,"A new analysis must invalidate the persistent chain snapshot");
await afterReload.cache.load(overview,60000,true);
assert.equal(requests,5,"Reanalysis/refresh must invalidate metadata");
now+=900001;
await afterReload.cache.load(previewAll,900000,true);
assert.equal(requests,6,"Expired preview must be refreshed");
assert.match(js,/ui\.video\.addEventListener\("loadedmetadata"/,
  "Player must seek latest chosen rally only once media metadata is available");
const toolbar=preview.slice(preview.indexOf('class="section-bar"'),
  preview.indexOf('class="viewer-layout"'));
assert.doesNotMatch(toolbar, /id="studio-(?:filter|prev|next|page-caption)"/,
  "The redundant top toolbar and pagination must be removed");
assert.match(preview, /class="rallies-panel-head"[\s\S]*?id="studio-filter"/,
  "Rally filtering belongs inside the right-hand rally list");
assert.match(js, /rallyListQuery\(\)/,
  "Rally list query should use the filter in its new location");
assert.match(js, /ui\.list\.addEventListener\("scroll"/,
  "The rally rail must fetch more items on scroll");
assert.match(js, /s\.items\.push\(\.\.\.raw\.items\)/,
  "Subsequent pages must append instead of replacing the rally list");
console.log("Studio preview layout and cache: OK");
