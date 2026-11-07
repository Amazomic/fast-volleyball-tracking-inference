"use strict";
window.VolleyballAnalysisPresets=(()=>{
  // Neural profiles differ in detector configuration; amateur matching and
  // automatic association run on cached predictions after GPU inference.
  const presets={
    accuracy:{
      name:"Точность",
      description:"VballNetV4c и RAVEL-VB-012 — NVIDIA GPU (ONNX Runtime CUDA). Остальные параметры стандартные.",
      tradeoff:"При недоступности CUDA инференс RAVEL остановится без скрытого перехода на CPU.",
      values:{
        ball_model:"VballNetV4c_seq9_grayscale_20260908_213829.onnx",
        player_model:"RAVEL-VB-012-9f",
        backend:"onnx",
        player_backend:"onnx-cuda",
        player_device:"GPU",
        gpu_device_id:0,
        gpu_memory_limit_mb:0
      }
    },
    speed:{
      name:"Скорость",
      description:"VballNetFastV1 и RAVEL-VB-011 на NVIDIA GPU (CUDA); CPU можно выбрать вручную для каждой модели.",
      tradeoff:"Модели легче, но качество обнаружения мяча ниже. По умолчанию обе используют GPU.",
      values:{
        ball_model:"VballNetFastV1_seq9_grayscale_233_h288_w512.onnx",
        player_model:"RAVEL-VB-011-9f",
        backend:"onnx",
        player_backend:"onnx-cuda",
        player_device:"GPU",
        gpu_device_id:0,
        gpu_memory_limit_mb:0
      }
    },
    new:{
      name:"Новый",
      description:"VballNetV4c + RAVEL-VB-013: обнаружение мяча, игроков и касаний Touch. Обе модели на NVIDIA GPU (CUDA).",
      tradeoff:"RAVEL-013 определяет касания игроков; для стабильной работы требуется CUDA.",
      values:{
        ball_model:"VballNetV4c_seq9_grayscale_20260908_213829.onnx",
        player_model:"RAVEL-VB-013-9f",
        backend:"onnx",
        player_backend:"onnx-cuda",
        player_device:"GPU",
        gpu_device_id:0,
        gpu_memory_limit_mb:0,
        player_action_threshold:0.5,
        player_identity_mode:"amateur",
        player_reid_threshold_percent:88,
        player_auto_assign:true,
        player_auto_assign_threshold_percent:90,
        player_session_gap_sec:120
      }
    },
    amateur:{
      name:"Любительский матч · ReID",
      description:"RAVEL-013 + VballNetV4c, сохранение игроков между розыгрышами и ReID с автоматической привязкой при сходстве выше 90%.",
      tradeoff:"Более чувствительная детекция может добавить ложные рамки; CPU ReID дополнительно обрабатывает снимки игроков. Привязки без уверенного совпадения остаются ручными.",
      values:{
        ball_model:"VballNetV4c_seq9_grayscale_20260908_213829.onnx",
        player_model:"RAVEL-VB-013-9f",
        backend:"onnx",
        player_backend:"onnx-cuda",
        player_device:"GPU",
        gpu_device_id:0,
        gpu_memory_limit_mb:0,
        player_score_threshold:0.35,
        player_close_threshold:0.18,
        player_hysteresis_frames:120,
        player_match_center_distance:0.14,
        player_match_iou:0.06,
        player_identity_mode:"amateur",
        player_reid_threshold_percent:88,
        player_auto_assign:true,
        player_auto_assign_threshold_percent:90,
        player_session_gap_sec:120
      }
    }
  };

  const all={...presets};
  const $=id=>document.getElementById(id);
  const form=()=>$("analysis-form");
  const select=()=>$("analysis-preset");
  const editor=()=>$("analysis-technical-settings");
  const factory={};
  let schema={};
  let schemaError="Ограничения параметров ещё загружаются.";
  let sourceId="new";
  let applying=false;
  const labels={
    ball_model:"Модель мяча",ball_threshold:"Порог мяча",player_model:"Модель игроков",
    player_score_threshold:"Порог игроков",player_action_threshold:"Порог сохранения Touch (GPU)",player_close_threshold:"Закрытие трека",
    player_reader:"Декодер RAVEL",player_decode_hwaccel:"Аппаратный декодер",
    player_decode_threads:"Потоки декодирования",
    player_hysteresis_frames:"Удержание ID",player_match_center_distance:"Дистанция ID",
    player_match_iou:"IoU сопоставления",player_frame_step:"Пропуск кадров",
    player_identity_mode:"Распознавание игроков (CPU)",player_reid_threshold_percent:"Группировка ReID, %",
    player_auto_assign:"Автоматическая привязка",player_auto_assign_threshold_percent:"Автопривязка ReID, %",
    player_session_gap_sec:"Граница игровой сессии, с",
    player_stride:"Шаг клипа",backend:"Движок",
    player_device:"OpenVINO (устаревшая настройка)",player_backend:"Устройство игроков",
    gpu_device_id:"GPU ID",gpu_memory_limit_mb:"Арена VRAM, МБ",
    threads:"Потоки CPU",analysis_width:"Ширина кадра",
    manual_width_enabled:"Ручная ширина"
  };
  function controls(){
    return Array.from(editor().querySelectorAll("[name]")).filter(el=>
      el.name && !(el instanceof HTMLButtonElement));
  }
  function control(name){
    return form().elements.namedItem(name);
  }
  function cast(el){
    if(el.type==="checkbox")return el.checked;
    if(el.type==="number")return Number(el.value);
    return el.value;
  }
  function readValues(includeInactive=false){
    const values={};
    controls().filter(el=>includeInactive||!el.disabled).forEach(el=>{values[el.name]=cast(el);});
    values.manual_width_enabled=$("manual-width-enabled").checked;
    return values;
  }
  function validationMessage(el){
    const spec=schema[el.name];
    if(!spec||el.disabled||el.type==="checkbox"||el.type==="hidden")return "";
    const label=labelFor(el.name);
    if(spec.choices){
      return spec.choices.includes(el.value)?"":label+": выберите допустимое значение.";
    }
    if(el.type!=="number")return "";
    const raw=el.value.trim(),number=Number(raw);
    if(!raw||!Number.isFinite(number))return label+": укажите число.";
    if(spec.type==="int"&&!Number.isInteger(number))
      return label+": требуется целое число.";
    if(spec.min!==null&&number<spec.min||spec.max!==null&&number>spec.max)
      return label+": допустимый диапазон "+spec.min+"–"+spec.max+".";
    if(spec.multiple_of&&number%spec.multiple_of!==0)
      return label+": требуется кратность "+spec.multiple_of+".";
    if(spec.odd&&number%2!==1)
      return label+": требуется нечётное число.";
    const other=spec.less_than?control(spec.less_than):null;
    if(other&&number>=Number(other.value))
      return label+": должно быть меньше «"+labelFor(spec.less_than)+"».";
    return "";
  }
  function revealField(el){
    const stage=el.closest(".analysis-stage");
    const disclosure=$("analysis-technical-settings");
    if(disclosure)disclosure.open=true;
    if(stage){
      Array.from(editor().querySelectorAll(".analysis-stage")).forEach(section=>{
        section.open=section===stage;
      });
      document.querySelectorAll(".analysis-stage-tab").forEach(tab=>{
        const active=tab.dataset.analysisTab===stage.id;
        tab.setAttribute("aria-selected",String(active));
        tab.tabIndex=active?0:-1;
      });
    }
    const details=el.closest("#analysis-advanced");
    if(details)details.open=true;
  }
  function validateAll({focus=false}={}){
    if(schemaError){
      const indicator=$("analysis-profile-validation");
      if(indicator){
        indicator.textContent=schemaError;
        indicator.hidden=false;
      }
      return {valid:false,message:schemaError};
    }
    let first=null;
    for(const el of controls()){
      const message=validationMessage(el);
      el.setCustomValidity?.(message);
      if(message&&!first)first={el,message};
      if(el.type==="number"&&schema[el.name]){
        const hint=el.closest("label")?.querySelector(".analysis-field-error");
        if(hint){
          hint.textContent=message;
          hint.hidden=!message;
        }
      }
      if(message)el.setAttribute("aria-invalid","true");
      else el.removeAttribute("aria-invalid");
    }
    const indicator=$("analysis-profile-validation");
    if(indicator){
      indicator.textContent=first?first.message:"";
      indicator.hidden=!first;
    }
    if(first&&focus){
      revealField(first.el);
      first.el.focus();
      first.el.reportValidity?.();
    }
    return first?{valid:false,message:first.message,field:first.el.name}:{valid:true,message:""};
  }
  function correctNumber(el){
    const spec=schema[el.name];
    if(!spec||el.type!=="number"||el.disabled)return;
    const raw=el.value.trim();
    if(!raw||!Number.isFinite(Number(raw))){
      if(el.dataset.lastValid!==undefined)el.value=el.dataset.lastValid;
      return;
    }
    let num=Number(raw);
    if(spec.min!==null)num=Math.max(spec.min,num);
    if(spec.max!==null)num=Math.min(spec.max,num);
    if(spec.type==="int")num=Math.round(num);
    if(spec.multiple_of)num=Math.round(num/spec.multiple_of)*spec.multiple_of;
    if(spec.odd&&num%2===0)num=num+1<=(spec.max??Infinity)?num+1:num-1;
    const other=spec.less_than?control(spec.less_than):null;
    if(other&&num>=Number(other.value))num=Math.max(spec.min??0,Number(other.value)-1);
    el.value=String(num);
    if(!validationMessage(el))el.dataset.lastValid=el.value;
  }
  async function loadValidationSchema(){
    const response=await fetch("/api/analysis-profile-schema",{cache:"no-store"});
    const payload=await response.json();
    if(!response.ok||!payload.fields)throw Error(payload.error||"Не загружены ограничения параметров");
    schema=payload.fields;
    for(const el of controls()){
      const spec=schema[el.name];
      if(!spec)throw Error("Нет правил проверки поля: "+el.name);
      if(el.type!=="number")continue;
      if(spec.min!==null)el.min=String(spec.min);
      if(spec.max!==null)el.max=String(spec.max);
      el.step=spec.type==="int"?(spec.multiple_of?String(spec.multiple_of):"1"):"any";
      if(!validationMessage(el))el.dataset.lastValid=el.value;
      const label=el.closest("label");
      if(label&&!label.querySelector(".analysis-field-error")){
        const hint=document.createElement("small");
        hint.className="analysis-field-error";
        hint.hidden=true;
        hint.setAttribute("role","status");
        label.append(hint);
      }
    }
    schemaError="";
    validateAll();
  }

  function snapshotDefaults(){
    Object.assign(factory,readValues(true));
  }
  function profileValues(id){
    return {...factory,...(all[id]?.values||{})};
  }
  function labelFor(key){
    return labels[key] || control(key)?.closest("label")?.querySelector("span")
      ?.textContent?.trim() || key.replaceAll("_"," ");
  }
  function display(value){
    if(typeof value==="boolean")return value?"Вкл.":"Выкл.";
    return String(value).replace(/^VballNetFastV1.*/,"FastV1")
      .replace(/^VballNetV4c.*/,"V4c");
  }
  function difference(current,base){
    // Older saved profiles can include CPU-only/diagnostic flags no longer
    // exposed by the primary analysis form. They must not create fake diffs.
    return Object.keys(current).filter(key=>!control(key)?.disabled&&String(current[key])!==String(base[key]));
  }
  function clearNotice(){
    $("analysis-preset-applied").hidden=true;
    $("analysis-profile-status").textContent="";
  }
  function renderChoices(){
    select().replaceChildren();
    for(const [id,profile] of Object.entries(all)){
      const option=document.createElement("option");
      option.value=id;
      option.textContent=profile.name;
      select().append(option);
    }
  }
  function updateState(){
    if(applying||!editor())return;
    const current=readValues();
    const base=profileValues(sourceId);
    const changes=difference(current,base);
    const custom=changes.length>0;
    let option=select().querySelector('option[value="custom"]');
    if(custom&&!option){
      option=document.createElement("option");
      option.value="custom";
      option.textContent="Пользовательский · не сохранён";
      select().prepend(option);
    }else if(!custom&&option){
      option.remove();
    }
    select().value=custom?"custom":sourceId;
    const source=all[sourceId];
    $("analysis-preset-description").textContent=custom?
      "Изменено параметров: "+changes.length+" · исходный профиль «"+source.name+
      "». Можно запустить сейчас или сохранить новый профиль.":
      source.description||"Пользовательский профиль готов к запуску.";
    $("analysis-preset-reset").hidden=!custom;
    // The delete control stays visible; built-in profiles are protected.
    const deletable=sourceId.startsWith("user_")&&!custom;
    const deleteButton=$("analysis-preset-delete");
    deleteButton.disabled=!deletable;
    deleteButton.title=custom&&sourceId.startsWith("user_")
      ?"Сначала сбросьте или сохраните изменения профиля"
      :deletable?"Удалить выбранный профиль":"Встроенные профили удалить нельзя";
    $("analysis-save-panel").hidden=!custom;
    // The reference is the actual startup preset, not the previously selected
    // profile. Show all differences for built-in, saved and unsaved profiles.
    const defaults=profileValues("new");
    const differing=difference(current,defaults);
    const diff=$("analysis-preset-diff");
    diff.replaceChildren();
    diff.hidden=false;
    $("analysis-preset-diff-count").textContent=String(differing.length);
    if(!differing.length){
      const message=document.createElement("p");
      message.className="analysis-diff-empty";
      message.textContent="Все параметры совпадают с профилем по умолчанию.";
      diff.append(message);
    }else{
      for(const key of differing){
        const row=document.createElement("div");
        row.className="analysis-diff-row";
        row.setAttribute("role","listitem");
        const title=document.createElement("span");
        title.textContent=labelFor(key);
        const values=document.createElement("strong");
        values.textContent=display(defaults[key])+" → "+display(current[key]);
        row.append(title,values);
        diff.append(row);
      }
    }
  }
  function applyProfile(id){
    if(!all[id])return;
    applying=true;
    sourceId=id;
    try{
      const values=profileValues(id);
      for(const [key,value] of Object.entries(values)){
        if(key==="manual_width_enabled")continue;
        const el=control(key);
        if(!el)continue;
        if(el.type==="checkbox")el.checked=Boolean(value);
        else el.value=String(value);
      }
      $("manual-width-enabled").checked=Boolean(values.manual_width_enabled);
      // Refresh dependent controls only after restoring the entire profile.
      for(const name of ["backend","ball_model","player_backend","manual_width_enabled"]){
        const el=name==="manual_width_enabled"?$("manual-width-enabled"):control(name);
        el?.dispatchEvent(new Event("change",{bubbles:true}));
      }
    }finally{
      applying=false;
    }
    clearNotice();
    validateAll();
    updateState();
  }
  async function refresh(keepSelection=sourceId,applySelection=false){
    const response=await fetch("/api/analysis-profiles",{cache:"no-store"});
    const data=await response.json();
    if(!response.ok)throw Error(data.error||"Не удалось загрузить профили");
    for(const id of Object.keys(all))if(id.startsWith("user_"))delete all[id];
    for(const profile of data.profiles||[])all[profile.id]=profile;
    if(!all[keepSelection])keepSelection="new";
    renderChoices();
    if(applySelection)applyProfile(keepSelection);
    else{
      sourceId=keepSelection;
      updateState();
    }
  }
  function suggestedName(){
    const now=new Date().toLocaleString(window.VolleyI18n?.locale()||"ru-RU",{
      day:"2-digit",month:"2-digit",year:"numeric",
      hour:"2-digit",minute:"2-digit",hour12:false
    });
    return "Мой профиль "+now.replace(",", "");
  }
  async function save(){
    const button=$("analysis-profile-save");
    const name=$("analysis-profile-name").value.trim();
    if(!name){
      $("analysis-profile-status").textContent="Введите название профиля.";
      $("analysis-profile-name").focus();
      return;
    }
    const check=validateAll({focus:true});
    if(!check.valid){
      $("analysis-profile-status").textContent=check.message;
      return;
    }
    if(!form().reportValidity()){
      $("analysis-profile-status").textContent="Проверьте выделенное поле.";
      return;
    }
    button.disabled=true;
    try{
      const response=await fetch("/api/analysis-profiles/save",{
        method:"POST",headers:{"Content-Type":"application/json"},
        body:JSON.stringify({name,values:readValues()})
      });
      const result=await response.json();
      if(!response.ok)throw Error(result.error||"Не удалось сохранить профиль");
      await refresh(result.profile.id,true);
      $("analysis-profile-name").value=suggestedName();
      $("analysis-preset-applied").textContent="Профиль «"+name+"» сохранён и выбран.";
      $("analysis-preset-applied").hidden=false;
    }catch(error){
      $("analysis-profile-status").textContent=error.message;
    }finally{
      button.disabled=false;
    }
  }
  async function remove(){
    const id=sourceId,profile=all[id];
    if(!id.startsWith("user_")||select().value==="custom"||!profile)return;
    if(!confirm("Удалить профиль «"+profile.name+"»?"))return;
    const button=$("analysis-preset-delete");
    button.disabled=true;
    try{
      const response=await fetch("/api/analysis-profiles/delete",{
        method:"POST",headers:{"Content-Type":"application/json"},
        body:JSON.stringify({id})
      });
      const result=await response.json();
      if(!response.ok)throw Error(result.error||"Не удалось удалить профиль");
      await refresh("new",true);
      $("analysis-preset-applied").textContent="Профиль удалён. Выбран рекомендуемый.";
      $("analysis-preset-applied").hidden=false;
    }catch(error){
      $("analysis-preset-applied").textContent=error.message;
      $("analysis-preset-applied").hidden=false;
    }finally{
      button.disabled=false;
    }
  }
  async function init(){
    if(!select()||!form()||!editor())return;
    // Set 80% of the logical CPUs available to the inference server.
    // Do this before snapshotDefaults so both profiles inherit it.
    try{
      const response=await fetch("/api/analysis-resources",{cache:"no-store"});
      if(!response.ok)throw Error("CPU resources unavailable");
      const resources=await response.json();
      const threads=control("threads");
      if(threads){
        threads.max="128";
        threads.value=String(Math.min(128,Math.max(1,Number(resources.default_threads)||1)));
        // The default RAVEL-013 GPU profile leaves CPU helper stages unrestricted.
        presets.new.values.threads=Math.min(128,Math.max(1,
          Number(resources.logical_cpu_count)||Number(resources.default_threads)||1));
      }
    }catch(error){
      const threads=control("threads");
      if(threads)threads.value=String(Math.min(128,Math.max(1,Math.round((navigator.hardwareConcurrency||8)*0.8))));
      presets.new.values.threads=Math.min(128,Math.max(1,navigator.hardwareConcurrency||8));
    }
    try{
      await loadValidationSchema();
    }catch(error){
      schemaError="Не удалось проверить ограничения: "+error.message+". Перезапустите сервер.";
      $("analysis-profile-validation").textContent=schemaError;
      $("analysis-profile-validation").hidden=false;
    }
    snapshotDefaults();
    $("analysis-profile-name").value=suggestedName();
    renderChoices();
    applyProfile("new");
    select().addEventListener("change",event=>{
      if(event.target.value==="custom"){updateState();return;}
      const hasDraft=!!select().querySelector('option[value="custom"]');
      if(hasDraft&&!confirm("Несохранённые изменения будут сброшены. Выбрать другой профиль?")){
        select().value="custom";
        return;
      }
      applyProfile(event.target.value);
    });
    editor().addEventListener("input",event=>{
      if(event.target.matches("input,select")){
        clearNotice();validateAll();updateState();
      }
    });
    editor().addEventListener("change",event=>{
      if(event.target.matches("input,select")){
        correctNumber(event.target);clearNotice();validateAll();updateState();
      }
    });
    form().addEventListener("invalid",event=>{
      if(event.target.closest(".analysis-profile-editor")){
        revealField(event.target);
        validateAll();
      }
    },true);
    $("analysis-preset-reset").addEventListener("click",()=>applyProfile(sourceId));
    $("analysis-profile-save").addEventListener("click",save);
    $("analysis-profile-name").addEventListener("keydown",event=>{
      if(event.key==="Enter"){event.preventDefault();save();}
    });
    $("analysis-preset-delete").addEventListener("click",remove);
    refresh(sourceId).catch(error=>{
      $("analysis-preset-applied").textContent="Не удалось загрузить свои профили: "+error.message;
      $("analysis-preset-applied").hidden=false;
    });
  }
  if(document.readyState==="loading")document.addEventListener("DOMContentLoaded",init,{once:true});
  else init();
  function currentName(){
    if(select().value==="custom"){
      const values=readValues();
      const serialized=JSON.stringify(Object.keys(values).sort().map(key=>[key,values[key]]));
      let hash=2166136261;
      for(let i=0;i<serialized.length;i++){
        hash=Math.imul(hash^serialized.charCodeAt(i),16777619);
      }
      return "Авто · "+(hash>>>0).toString(16).padStart(8,"0");
    }
    return all[sourceId]?.name||"Новый";
  }
  // Profile catalog is global; the selected ID is only a reference.
  // Custom unsaved settings receive an explicit non-catalog identity.
  function currentId(){return select().value==="custom"?"custom":sourceId;}
  return {presets:all,preview:updateState,apply:()=>applyProfile(select().value),
    readValues,refresh,currentName,currentId,validate:validateAll};
})();
