// Verify the full Rust migration stage map against the append-only diary.
// Node is only a local CI tool; the Rust application has no Node runtime dependency.
import {readFileSync,writeFileSync} from "node:fs";
import {dirname,join,resolve} from "node:path";
import {fileURLToPath} from "node:url";
const root=resolve(dirname(fileURLToPath(import.meta.url)),"../..");
const roadmapFile=join(root,"docs/RUST_ROADMAP.json");
const diaryFile=join(root,"docs/RUST_MIGRATION.md");
const outputFile=join(root,"docs/RUST_ROADMAP.md");
const data=JSON.parse(readFileSync(roadmapFile,"utf8"));
const diary=readFileSync(diaryFile,"utf8");
const fail=message=>{throw Error(message);};
const q=String.fromCharCode(96);
function diaryStages(text) {
  const result=new Map();
  for(const line of text.split(/\r?\n/)){
    const match=line.match(/^## (\d+)\. (.*)$/);
    if(!match)continue;
    const n=Number(match[1]);
    if(result.has(n))fail("Duplicate numbered diary entry: "+n);
    result.set(n,match[2].trim().replace(/\s+—\s+2026-\d{2}-\d{2}$/,""));
  }
  return result;
}
function check(roadmap,log){
  if(roadmap.schema!=="volley-rust-stage-roadmap/1" ||
    !Number.isSafeInteger(roadmap.plannedTotal) ||
    roadmap.plannedTotal<=0 || !Array.isArray(roadmap.stages) ||
    roadmap.plannedTotal!==roadmap.stages.length) {
    fail("Invalid total stage count or schema");
  }
  const actual=diaryStages(log);
  const last=Math.max(...actual.keys());
  if(actual.get(0)!=="Immutable checkpoint and repository state") {
    fail("Migration diary checkpoint 0 missing");
  }
  if(actual.size!==last+1)fail("Migration diary has missing stages");
  if(roadmap.lastRecorded!==last || roadmap.nextStage!==last+1) {
    fail("Stale stage counter: diary ends at "+last+
      ", roadmap at "+roadmap.lastRecorded+"/"+roadmap.plannedTotal);
  }
  if(roadmap.nextStage>roadmap.plannedTotal)fail("Plan exhausted: amend denominator");
  for(let i=0;i<roadmap.stages.length;i++){
    const item=roadmap.stages[i],n=i+1;
    if(item.number!==n || typeof item.title!=="string" || !item.title.trim() ||
      typeof item.area!=="string" || !item.area.trim() ||
      typeof item.acceptance!=="string" || !item.acceptance.trim()) {
      fail("Missing or duplicate roadmap stage "+n);
    }
    const expected=n<=6?"foundation":n<=last?"recorded":"planned";
    if(item.status!==expected)fail("Incorrect completion status at stage "+n);
    if(n<=last){
      if(item.journalSection!==n || actual.get(n)!==item.title){
        fail("Roadmap stage "+n+" does not match append-only diary title");
      }
    }else if("journalSection" in item){
      fail("Future stage falsely claims existing journal record: "+n);
    }
  }
  if(!roadmap.estimateNote?.includes("ориентир")){
    fail("Roadmap denominator must be explicitly provisional");
  }
  return last;
}
function render(roadmap){
  const last=roadmap.lastRecorded,total=roadmap.plannedTotal;
  const recorded=roadmap.stages.filter(s=>s.status==="recorded").length;
  const planned=roadmap.stages.filter(s=>s.status==="planned").length;
  const result=[
    "# Дорожная карта Rust-миграции Volleyball Studio",
    "",
    "Источник данных: "+q+"docs/RUST_ROADMAP.json"+q+". Команда генерации: "+
      q+"node scripts/ci/check_rust_roadmap.mjs --write"+q+
      "; проверка: "+q+"node scripts/ci/check_rust_roadmap.mjs --check --self-test"+q+".",
    "",
    "## Общий прогресс",
    "",
    "**Последний записанный этап: "+last+" / "+total+". Следующий: "+
      roadmap.nextStage+" / "+total+".**",
    "",
    "В журнале: "+recorded+" практических этапов и 6 начальных архитектурных разделов. "+
      "Осталось по текущему плану: "+planned+".",
    "",
    "**Важно:** счётчик описывает покрытие дорожной карты, "+
      "а НЕ процент готовности продукта. У старых этапов могли остаться "+
      "неподключённые адаптеры и незавершённая функциональность.",
    "",
    roadmap.estimateNote,
    "",
    "После тестов и коммита следующего этапа добавьте запись в "+
      q+"docs/RUST_MIGRATION.md"+q+", переведите соответствующий JSON-пункт в "+
      q+"recorded"+q+", обновите "+q+"lastRecorded"+q+"/"+q+"nextStage"+q+
      " и повторно сгенерируйте карту. Изменение числа этапов "+
      "требует явного пересмотра плана и общей проверки.",
    "",
    "## Все 85 этапов",
    "",
    "Статусы: **Основа** = архитектурная запись, **Записан** = результат/"+
      "частичный шаг в журнале, **План** = ещё не реализовано.",
    ""
  ];
  let lastArea=null;
  for(const item of roadmap.stages){
    if(item.area!==lastArea){
      if(lastArea!==null)result.push("");
      result.push("### "+item.area,"",
        "| Этап | Состояние | Задача / критерий |",
        "|---:|---|---|");
      lastArea=item.area;
    }
    const label=item.status==="foundation"?"Основа":
      item.status==="recorded"?"Записан":"План";
    const details=item.status==="planned"?
      item.title+". Критерий: "+item.acceptance:item.title;
    result.push("| "+item.number+" / "+total+" | "+label+" | "+
      details.replaceAll("|","\\|")+" |");
  }
  result.push("",
    "## Неизменяемые правила процесса",
    "",
    "- Перед каждым этапом проверять Git-историю, владельцев функций и уже написанный код.",
    "- Нельзя создавать второго владельца ReID, привязок, золотых отметок или монтажа.",
    "- Новые/старые форматы поддерживаются адаптерами, а не двумя бизнес-реализациями.",
    "- Завершение этапа требует тестов, коммита, записи в журнале и обновлённой карты.",
    "- The Python baseline is preserved only by the immutable Git checkpoint. Active application, launcher and UI are Rust-only; original recordings and user caches remain untouched.",
    "- Готовность приложения подтверждается только реальными матчами и критериями паритета.",
    "");
  return result.join("\n");
}
try {
  const last=check(data,diary);
  const readme=readFileSync(join(root,"README.md"),"utf8");
  const summary="Rust-миграция: "+last+"/"+data.plannedTotal+
    "; следующий "+data.nextStage+"/"+data.plannedTotal;
  if(!readme.includes(summary))fail("README.md roadmap progress label is outdated: "+summary);
  const generated=render(data);
  if(process.argv.includes("--self-test")){
    const rejects=(mutated,label)=>{
      try{check(mutated,diary);}catch{return;}
      fail("Stage guard missed "+label);
    };
    rejects({...data,plannedTotal:84},"denominator mismatch");
    rejects({...data,lastRecorded:last-1},"stale current stage");
    const gap=structuredClone(data);gap.stages.splice(11,1);
    rejects(gap,"missing stage");
    const duplicate=structuredClone(data);duplicate.stages[12].number=12;
    rejects(duplicate,"duplicate number");
    const falseCompletion=structuredClone(data);
    falseCompletion.stages[data.nextStage-1].status="recorded";
    rejects(falseCompletion,"false completion");
  }
  if(process.argv.includes("--write")){
    writeFileSync(outputFile,generated,"utf8");
  }else{
    let content;
    try{content=readFileSync(outputFile,"utf8");}
    catch{fail("Missing generated roadmap Markdown; run --write");}
    if(content.replace(/\r\n/g,"\n")!==generated){
      fail("Stale README stage map: run node scripts/ci/check_rust_roadmap.mjs --write");
    }
  }
  process.stdout.write("Rust roadmap OK: "+last+"/"+data.plannedTotal+
    " recorded; next "+data.nextStage+"/"+data.plannedTotal+
    (process.argv.includes("--self-test")?" (negative checks passed)":"")+"\n");
}catch(error){
  process.stderr.write("Rust roadmap FAILED: "+error.message+"\n");
  process.exitCode=1;
}
