/* Self-documenting local UI settings: hover/focus any technical parameter name. */
(()=>{
  const help={
    ball_model:"Выбирает сеть, обнаруживающую мяч. V4c точнее на тестовом наборе, FastV1 быстрее; переключение требует нового анализа.",
    ball_threshold:"Порог тепловой карты VballNet. Ниже — больше потенциальных обнаружений и ложных срабатываний; выше — меньше ложных, но чаще пропуски.",
    backend:"Движок инференса мяча: CUDA на GPU, ONNX CPU или OpenVINO CPU. Меняет скорость и доступное оборудование.",
    analysis_width:"Ширина видео, подаваемого на анализ. Больше пикселей — выше нагрузка на декодирование, возможны более мелкие детали. Высота пересчитывается пропорционально.",
    threads:"Количество рабочих CPU-потоков для инференса/вспомогательных этапов. Не закрепляет ядра процессора.",
    beach:"Использует режим пляжного волейбола для классификации игровых эпизодов и обработки разметки площадки.",
    player_model:"VB-013 определяет игроков, мяч, головы и вероятность Touch; VB-012 не имеет головы действий, VB-011 быстрее, но не определяет головы.",
    player_backend:"ONNX Runtime CUDA использует NVIDIA GPU; ONNX CPU выполняет инференс на процессоре. Выбор влияет на скорость, а не на формат Touch.",
    player_device:"Для ONNX CUDA используется GPU, для ONNX CPU — CPU; режим OpenVINO обрабатывается отдельно.",
    player_stride:"Шаг между последовательными клипами модели игроков. Меньше — плотнее анализ и больше работы; больше — выше скорость, но возможны пропуски.",
    player_frame_step:"Пропуск кадров при подаче данных модели игроков. 1 — каждый кадр, 2 — каждый второй: быстрее, но хуже временная точность.",
    player_performance_hint:"Подсказка компилятору OpenVINO: LATENCY оптимизирует отклик одного клипа, THROUGHPUT — пропускную способность.",
    player_warmup_runs:"Число прогревочных прогонов RAVEL до измеряемой работы. Влияет на задержку запуска, не на точность.",
    player_score_threshold:"Порог уверенности обнаружения игроков RAVEL. Повышение уменьшает ложные рамки, но может потерять игроков.",
    player_close_threshold:"Порог сохранения уже отслеживаемого игрока при кратком падении уверенности. Влияет на стабильность ID.",
    player_hysteresis_frames:"Сколько кадров удерживать ID потерянного игрока до завершения трека. Больше — меньше разрывов, но возможны ложные продолжения.",
    player_match_center_distance:"Максимальное нормализованное расстояние между центрами рамок для связывания кадров одного игрока. Больше — меньше обрывов ID, но выше риск перепутать соседних людей.",
    player_match_iou:"Минимальное пересечение рамок для альтернативного связывания ID. Меньше позволяет соединять при сильных перемещениях, но повышает риск ошибочного объединения.",
    player_identity_mode:"После GPU-детекции RAVEL запускает CPU ReID по нескольким кадрам каждого ID, объединяет фрагменты между розыгрышами. Игроки, видимые одновременно, не могут быть одной личностью.",
    player_reid_threshold_percent:"Минимальное косинусное сходство кадров для автоматической группировки Track ID. 88% по умолчанию; не является вероятностью личности.",
    player_auto_assign:"После группировки автоматически сохраняет сопоставления только с уже известными игроками проекта, если существуют подтверждённые эталоны и отсутствуют конфликты.",
    player_auto_assign_threshold_percent:"Минимальное косинусное сходство с подтверждёнными игроками для автоматической привязки. По умолчанию 97%; также проверяется разница со вторым кандидатом.",
    player_session_gap_sec:"Пауза, в течение которой на видео нет ни одного игрока, после которой система перестаёт объединять Track ID между участками. Не гарантирует определение смены сторон.",
    player_ball_threshold:"Минимальная уверенность мяча, который дополнительно обнаруживает RAVEL. Это не основной порог VballNet.",
    player_head_threshold:"Порог определения головы RAVEL-VB-012/013. У VB-011 этой категории нет.",
    player_action_threshold:"Порог вероятности Touch модели RAVEL-013. При анализе и CPU-пересчёте касания выделяются по пикам сглаженной последовательности. Полные декодированные вероятности сохраняются для повторной обработки.",
    player_nms_iou:"IoU-порог подавления перекрывающихся рамок игроков (NMS). Больший порог оставляет больше пересечений, меньший сильнее удаляет дубликаты.",
    player_include_features:"Добавить подробные признаки обнаружения RAVEL в JSON для диагностики и изучения ошибок.",
    player_output_video:"Дополнительно записывать видео с рамками игроков и шкалой вероятности Touch (RAVEL-013, upstream 04.11.2026). Увеличивает расход диска и время вывода, не изменяет детекцию.",
    player_touch_smooth:"Окно сглаживания вероятностей Touch RAVEL-013 перед выделением отдельных пиков (по умолчанию 5 кадров). Используется в обычном анализе и CPU-пересчёте, не изменяет сырой кэш.",
    player_touch_min_gap:"Минимальное расстояние между отдельными пиками Touch RAVEL-013 (по умолчанию 10 кадров). Используется при расчёте касаний и в диагностическом MP4.",
    player_touch_hold:"На сколько кадров вокруг найденного пика подсвечивать игрока на диагностическом MP4.",
    player_strip_seconds:"Ширина временной ленты Touch на диагностическом видео в секундах по обе стороны кадра.",
    player_reader:"Новый FFmpeg YUV decoder от upstream; меняет чтение кадров модели, но не заменяет RAVEL-013.",
    player_decode_hwaccel:"NVIDIA NVDEC доступен через FFmpeg, если драйвер и видео поддерживают аппаратное декодирование. Для надёжной проверки оставьте none.",
    player_decode_threads:"Число потоков нового FFmpeg-декодера. 0 — выбрать автоматически.",
    ball_output_video:"Кроме ball.csv создавать аннотированный MP4 детектора мяча. Использует дополнительное время и место.",
    track_max_distance:"Максимально допустимое смещение мяча между наблюдениями при связывании траектории (пиксели). Слишком мало — разрывы; слишком много — неверные связи.",
    track_min_duration:"Минимальная длительность сохраняемого трека мяча. Большие значения отсеивают короткие фрагменты, но могут терять короткие действия.",
    track_player_threshold:"Порог подтверждения игрока при расчёте контактов. Влияет на сопоставление мяча с игроком и тип действия.",
    track_ball_threshold:"Порог обнаружений мяча RAVEL, применяемых при восстановлении пропусков в треках.",
    track_no_ball_fill:"Запретить алгоритмическое восполнение пропущенных точек мяча. Реальные детекции остаются, треки могут стать более разрывными.",
    tracks_export_mode:"Режим диагностического экспорта из track_processor.py: отключено, общий MP4, отдельные эпизоды или оба варианта.",
    tracks_padding:"Сколько секунд добавить до и после каждого трека при диагностическом экспорте. Не влияет на детекцию.",
    tracks_include_not_rally:"Включать в диагностическое видео также неигровые и не классифицированные как rally треки — полезно для поиска причин ошибок.",
    tracks_mark_ball:"Рисовать маркер обнаруженного мяча на диагностических MP4; не влияет на исходные координаты.",
    tracks_debug:"Накладывать дополнительные диагностические признаки на экспортируемое видео.",
    show_rally_snapshot:"Создать PNG-снимок выбранного трека через show_rally.py без открытия локального OpenCV-окна.",
    show_rally_track:"Номер трека, с которого show_rally.py создаёт диагностический снимок.",
    duration:"Желаемая длина итогового монтажа. Фактическая может отличаться из-за границ розыгрышей.",
    min_rally:"Самая короткая длительность розыгрыша, разрешённого для итогового монтажа. Не меняет сохранённые треки.",
    min_confidence:"Минимальная оценка уверенности розыгрыша при выборе моментов для монтажа. Выше — строгий отбор, меньше эпизодов.",
    montage_padding_sec:"Запас перед началом и после конца розыгрыша в общей подборке. Уменьшает вероятность резкой обрезки подачи или завершения очка.",
    reels_export:"Дополнительно создать вертикальные видео 9:16 штатным make_reels.py, не затрагивая основной монтаж.",
    reels_limit:"Максимум выбранных треков для отдельного вертикального экспорта. Ограничивает количество файлов и время монтажа.",
    reels_smoothing:"Алгоритм сглаживания движения виртуальной камеры вокруг мяча: нет, скользящее среднее, Савицкого–Голея или Калман.",
    reels_interpolation:"Как заполнять недостающие координаты мяча в вертикальном кропе: удерживать последнюю точку или линейно интерполировать.",
    reels_margin:"Дополнительное смещение центра кропа в направлении движения мяча (пиксели). Меняет композицию вертикального ролика.",
    reels_padding:"Дополнительный запас вокруг полного объединённого розыгрыша при вертикальном монтаже. Исходные треки остаются неизменными.",
    reels_smooth_window:"Размер окна сглаживания движения камеры (нечётное число). Большое окно — плавнее, но больше запаздывание.",
    reels_smooth_polyorder:"Степень полинома при сглаживании Savitzky–Golay. Должна быть меньше размера окна.",
    reels_fade_transition:"Добавить переходы через затемнение между отдельными вертикальными треками.",
    warmup:"Количество прогревочных инференсов до замера FPS. Их время не входит в результат бенчмарка.",
    runs:"Число измеряемых инференсов в каждом повторе. Больше — устойчивее статистика, дольше бенчмарк.",
    repeats:"Сколько раз заново загружать модель и измерять скорость. Итог основан на медиане повторов.",
    gap_sec:"Максимальный разрыв между двумя игровыми треками, при котором они объединяются в один предполагаемый розыгрыш.",
    min_sec:"Минимальная длительность итогового объединённого розыгрыша. Удаляет короткие обрывки из списка, без повторной детекции.",
    context_sec:"Дополнительные секунды до и после найденного игрового эпизода в просмотре. Границы не должны пересекать соседние розыгрыши.",
    manual_width_enabled:"Включить ручную ширину входного кадра вместо автоматически подобранного размера.",
    auto_size:"Реальный размер подготовленного видео рассчитывается из пропорций и ограничений анализа; исходный видеофайл не меняется.",
    court_toggle:"Необязательная разметка площадки. Помогает фильтровать неигровые траектории и учитывать положение сетки; неполная разметка может влиять на качество.",
    gpu_device_id:"Номер GPU для CUDA-инференса. Меняет видеокарту, на которой исполняется анализ; не изменяет уже сохранённые результаты.",
    gpu_memory_limit_mb:"Ограничение памяти CUDA в МБ. Ноль означает отсутствие программного лимита; слишком малое значение может привести к ошибке загрузки модели.",
    analysis_preset:"Сохранённый глобальный профиль параметров инференса. Выбор не меняет предыдущие запуски и не запускает пересчёт.",
    video:"Видео для измерения производительности. Бенчмарк не создаёт новую ветвь анализа проекта.",
    start:"Начальная позиция тестируемого видео в секундах.",
    follow_ball:"Создать вертикальный кроп, который следует за мячом. При выключении используется обычный монтаж из исходного видео.",
    rally_selection_mode:"Автоматически подобрать эпизоды или вручную отметить розыгрыши в предпросмотре. Доступность зависит от результатов выбранной CPU-версии.",
    selection_strategy:"Критерий ранжирования розыгрышей при автоматическом монтаже. Не изменяет классификацию и сохранённые события.",
    audio_path:"Локальный аудиофайл для итогового ролика. Оставьте пустым, чтобы сохранить исходный звук.",
    audio_volume:"Громкость добавленной музыки в процентах. Ноль отключает музыку; на классификацию игры параметр не влияет.",
    track_show_ball:"Добавить в экспортируемое видео технические метки мяча и идентификаторы треков; исходные данные не изменяются.",
    min_actions:"Минимальное число распознанных действий в розыгрыше для отбора в монтаж. Ноль отключает фильтр.",
    min_attack:"Минимальное число атак в розыгрыше. Ноль отключает этот фильтр.",
    min_block:"Минимальное число блоков в розыгрыше. Ноль отключает этот фильтр.",
    min_dig:"Минимальное число приёмов в розыгрыше. Ноль отключает этот фильтр.",
    min_overhead_pass:"Минимальное число верхних передач в розыгрыше. Ноль отключает этот фильтр.",
    min_serve:"Минимальное число подач в розыгрыше. Ноль отключает этот фильтр.",
    result_rally_sort:"Сортирует найденные розыгрыши в списке; не изменяет границы и сохранённые результаты.",
    result_model_touch_threshold:"Порог видимости модельных вероятностей Touch в предпросмотре. Только визуальный фильтр, CPU-касания не пересчитываются.",
    result_ball_threshold:"CPU-порог для сохранённых оценок VballNet. Предпросмотр меняется сразу, но новая версия событий появляется только после пересчёта.",
    result_player_threshold:"CPU-порог для сохранённых рамок RAVEL. Не восстанавливает игроков, отброшенных при инференсе на GPU.",
    result_reprocess_mode:"Алгоритм CPU-постобработки: геометрия VballNet, Touch RAVEL или комбинация. Не запускает повторный инференс.",
    result_reprocess_touch_threshold:"Нижний порог вероятности Touch RAVEL для новой CPU-версии. Применяется только к сохранённым модельным значениям.",
    result_touch_min_interval:"Минимальное время между соседними касаниями одного розыгрыша. Увеличение подавляет дубли, но может скрыть быстрые касания.",
    result_touch_max_reach:"Относительный предел расстояния от мяча до игрока при сопоставлении Touch. Больший порог допускает более далёкие касания.",
    result_track_min_duration:"Минимальная длительность трека мяча для CPU-классификации. Применяется при сохранении новой версии.",
    result_track_max_distance:"Максимальное расстояние между обнаружениями мяча при связывании траектории в CPU-версии.",
    result_track_player_threshold:"CPU-порог уверенности игроков, участвующих в поиске контактов мяча.",
    result_track_ball_threshold:"CPU-порог для обнаружений мяча RAVEL при восполнении пробелов VballNet.",
    result_track_no_ball_fill:"Отключает алгоритмическое заполнение пропусков мяча. Доступные сырые детекции сохраняются.",
    result_track_beach:"Использовать особенности пляжного волейбола при расчёте контактов и эпизодов в CPU.",
    player_reid_threshold:"Минимальное косинусное сходство визуальных признаков Track ID. Это не вероятность личности; ручная привязка к игроку обязательна.",
    players_video:"Выбор видеозаписи, для которой показаны Track ID. Ручные связи сохраняются в контексте проекта и выбранного GPU-анализа.",
    players_filter:"Фильтрация таблицы Track ID. Не удаляет треки из анализа.",
    montage_context_display:"Контекст из предпросмотра применяется автоматически и не редактируется в монтажной вкладке.",
    result_preview_mode:"Переключает сохранённые нейросетевые предсказания и события выбранной CPU-версии; не запускает новый анализ."
  };
  const override={
    "benchmark-form:backend":"Выбирает движок тестирования скорости двух моделей: ONNX CPU, ONNX CUDA или OpenVINO CPU. Замер не включает декодирование видео.",
    "benchmark-form:threads":"Количество потоков CPU только для бенчмарка на CPU. На CUDA число потоков не управляет вычислениями модели."
  };
  function original(control,key){
    if(key==="analysis_width")return "Автоматически (ручная ширина 1280 px отключена)";
    if(key==="auto_size")return "Автоматически по видео";
    if(key==="court_toggle")return "Выключено (разметка необязательна)";
    if(key==="manual_width_enabled")return "Выключено";
    if(control.type==="checkbox")return control.defaultChecked?"Включено":"Выключено";
    if(control.tagName==="SELECT"){
      const option=[...control.options].find(x=>x.defaultSelected)||control.options[0];
      if(key==="ball_model")return "VballNetV4c";
      return option?.textContent.trim()||"Не задано";
    }
    return control.getAttribute("value")||control.defaultValue||"Автоматически";
  }
  function settingKey(control){
    const id=control.id||"";
    return id.startsWith("rally-")?id.slice(6).replace(/-/g,"_"):
      (control.name||id).replace(/-/g,"_");
  }
  function attach(control,key,description){
    if(!description||control.type==="hidden")return false;
    const label=control.closest("label")||
      [...document.querySelectorAll("label[for]")].find(item=>item.htmlFor===control.id);
    const defaultValue=original(control,key);
    const detail="Влияние: "+description+"\nПо умолчанию: "+defaultValue;
    if(!label){
      // Keep unlabeled third-party/dynamic controls discoverable and accessible.
      control.title=detail;
      return false;
    }
    let caption=label.querySelector(":scope > span");
    if(!caption){
      // Also support <label for="id">Text</label> and labels with direct text nodes.
      const text=[...label.childNodes].find(node=>
        node.nodeType===3&&node.textContent.trim());
      if(text){
        caption=document.createElement("span");
        caption.textContent=text.textContent.trim();
        text.replaceWith(caption);
      }
    }
    if(!caption){
      label.title=detail;
      return false;
    }
    if(caption.classList.contains("setting-help-label"))return true;
    caption.classList.add("setting-help-label");
    caption.setAttribute("tabindex","0");
    caption.dataset.tooltip=detail;
    caption.setAttribute("aria-label",caption.textContent.trim()+". "+detail);
    return true;
  }
  // Render once above the document rather than inside scrollable setting tabs:
  // overflow:auto/hidden used to clip the previous pseudo-element tooltips.
  let portal=null,activeLabel=null;
  function showTooltip(label){
    const detail=label?.dataset?.tooltip;
    if(!detail||!portal)return;
    activeLabel=label;
    portal.textContent=detail;
    portal.hidden=false;
    const rect=label.getBoundingClientRect();
    const width=portal.getBoundingClientRect().width;
    const height=portal.getBoundingClientRect().height;
    portal.style.left=Math.max(12,
      Math.min(rect.left,window.innerWidth-width-12))+"px";
    portal.style.top=(rect.bottom+10+height<=window.innerHeight-12
      ?rect.bottom+8:Math.max(12,rect.top-height-8))+"px";
  }
  function hideTooltip(){
    activeLabel=null;
    if(portal)portal.hidden=true;
  }
  function initPortal(){
    if(portal)return;
    portal=document.createElement("div");
    portal.id="settings-help-portal";
    portal.className="settings-tooltip-portal";
    portal.setAttribute("role","tooltip");
    portal.hidden=true;
    document.body.appendChild(portal);
    document.body.classList.add("settings-tooltips-floating");
    document.addEventListener("pointerover",event=>{
      const label=event.target.closest?.(".setting-help-label");
      if(label&&label!==activeLabel)showTooltip(label);
    });
    document.addEventListener("pointerout",event=>{
      if(activeLabel&&activeLabel.contains(event.target)&&
         !activeLabel.contains(event.relatedTarget))hideTooltip();
    });
    document.addEventListener("focusin",event=>{
      const label=event.target.closest?.(".setting-help-label");
      if(label)showTooltip(label);
    });
    document.addEventListener("focusout",event=>{
      if(activeLabel?.contains(event.target))hideTooltip();
    });
    document.addEventListener("scroll",hideTooltip,true);
    window.addEventListener("resize",hideTooltip);
  }
  function setup(){
    initPortal();
    for(const form of ["analysis-form","diagnostics-form","benchmark-form","highlights-form"]){
      for(const control of document.querySelectorAll(
        "#"+form+" input[name], #"+form+" select[name]")){
        const key=settingKey(control);
        attach(control,key,override[form+":"+key]||help[key]);
      }
    }
    // These controls are outside forms but change the CPU result or preview.
    const auxiliary="#result-rally-settings input[id], .result-cpu-tools input[id], "+
      ".result-cpu-tools select[id], #result-model-touch-threshold, "+
      "#result-rally-sort, #player-reid-threshold, #players-video, "+
      "#players-filter, #montage-context-display, "+
      "input[name='result-preview-mode'], #analysis-preset";
    for(const control of document.querySelectorAll(auxiliary)){
      const key=settingKey(control);
      attach(control,key,help[key]);
    }
    const manual=document.getElementById("manual-width-enabled");
    if(manual)attach(manual,"manual_width_enabled",help.manual_width_enabled);
    const auto=document.getElementById("auto-size-value")?.closest("div.field");
    const autoTitle=auto?.querySelector(":scope > span");
    if(autoTitle){
      autoTitle.classList.add("setting-help-label");
      autoTitle.tabIndex=0;
      autoTitle.dataset.tooltip="Влияние: "+help.auto_size+"\nПо умолчанию: Автоматически по видео";
      // Only the styled tooltip: native title would show a second bubble.
      autoTitle.removeAttribute("title");
    }
    const court=document.getElementById("court-toggle");
    if(court)court.title=help.court_toggle+" По умолчанию: разметка не обязательна.";
    const manualSummary=document.querySelector("#analysis-advanced > summary");
    if(manualSummary)manualSummary.title=help.manual_width_enabled+" По умолчанию: выключено.";
  }
  if(document.readyState==="loading")document.addEventListener("DOMContentLoaded",setup,{once:true});
  else setup();
})();
