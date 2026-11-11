import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {fileURLToPath} from "node:url";
import {join} from "node:path";

const root=fileURLToPath(new URL("../..",import.meta.url));
const html=readFileSync(join(root,"rust/volley-app/static/studio.html"),"utf8");
const css=readFileSync(join(root,"rust/volley-app/static/studio.css"),"utf8");
assert.doesNotMatch(html,/Анализ матчей и монтаж|Данные исходного анализа не изменяются|Классификация игровых действий требует ручного подтверждения/);
assert.doesNotMatch(html,/class="sidebar-foot"/);
assert.match(css,/\.shell\{display:flex;width:100%;height:100vh;height:100dvh;min-height:0;overflow:hidden\}/);
assert.match(css,/\.content\{flex:1 1 auto;min-height:0;overflow-x:hidden;overflow-y:auto/);
assert.match(css,/\.sr-only\{width:1px!important;height:1px!important;min-width:0!important;max-width:1px!important\}/);
assert.match(css,/\.native-stats-source-choice\{grid-template-columns:minmax\(0,1fr\)\}/);
assert.match(css,/\.tab-workspace\{min-width:0;min-height:0;max-width:100%\}/);
assert.match(css,/@media \(max-width:650px\)\{[\s\S]*?\.sidebar \.menu\{display:grid;grid-template-columns:repeat\(2,minmax\(0,1fr\)\)/);
console.log("PASS: no footer disclaimer; viewport shell, flexible cards, invisible upload input and compact mobile navigation are constrained");
