/* Self-documenting local UI settings: hover/focus any technical parameter name. */
(()=>{
  const help={
    ball_model:"Выбирает сеть, обнаруживающую мяч. V4c точнее на тестовом наборе, FastV1 быстрее; переключение требует нового анализа.",
    ball_threshold:"Порог тепловой карты VballNet. Ниже — больше потенциальных обнаружений и ложных срабатываний; выше — меньше ложных, но чаще пропуски.",
    backend:"Движок инференса мяча: CUDA на GPU, ONNX CPU или OpenVINO CPU. Меняет скорость и доступное оборудование.",
    analysis_width:"Ширина видео, подаваемого на анализ. Больше пикселей — выше нагрузка на декодирование, возможны более мелкие детали. Высота пересчитывается пропорционально.",
    threads:"Количество рабочих CPU-потоков для инференса/вспомогательных этапов. Не закрепляет ядра процессора.",
    beach:"Использует режим пляжного волейбола для классификации игровых эпизодов и обработки разметки площадки.",
    player_model:"Выбор локального RAVEL: VB-012 видит игроков, мяч и головы; VB-011 быстрее, но не определяет головы.",
    player_device:"Устройство выполнения RAVEL через OpenVINO. GPU доступен только с поддерживаемым устройством OpenVINO; это отдельный движок от VballNet.",
    player_stride:"Шаг между последовательными клипами модели игроков. Меньше — плотнее анализ и больше работы; больше — выше скорость, но возможны пропуски.",
    player_frame_step:"Пропуск кадров при подаче данных модели игроков. 1 — каждый кадр, 2 — каждый второй: быстрее, но хуже временная точность.",
    player_performance_hint:"Подсказка компилятору OpenVINO: LATENCY оптимизирует отклик одного клипа, THROUGHPUT — пропускную способность.",
    player_warmup_runs:"Число прогревочных прогонов RAVEL до измеряемой работы. Влияет на задержку запуска, не на точность.",
    player_score_threshold:"Порог уверенности обнаружения игроков RAVEL. Повышение уменьшает ложные рамки, но может потерять игроков.",
    player_close_threshold:"Порог сохранения уже отслеживаемого игрока при кратком падении уверенности. Влияет на стабильность ID.",
    player_hysteresis_frames:"Сколько кадров удерживать ID потерянного игрока до завершения трека. Больше — меньше разрывов, но возможны ложные продолжения.",
    player_match_center_distance:"Максимальное нормализованное расстояние между центрами рамок для связывания кадров одного игрока. Больше — меньше обрывов ID, но выше риск перепутать соседних людей.",
    player_match_iou:"Минимальное пересечение рамок для альтернативного связывания ID. Меньше позволяет соединять при сильных перемещениях, но повышает риск ошибочного объединения.",
    player_ball_threshold:"Минимальная уверенность мяча, который дополнительно обнаруживает RAVEL. Это не основной порог VballNet.",
    player_head_threshold:"Порог определения головы RAVEL-VB-012. У VB-011 этой категории нет.",
    player_nms_iou:"IoU-порог подавления перекрывающихся рамок игроков (NMS). Больший порог оставляет больше пересечений, меньший сильнее удаляет дубликаты.",
    player_include_features:"Добавить подробные признаки обнаружения RAVEL в JSON для диагностики и изучения ошибок.",
    player_output_video:"Дополнительно записывать видео с рамками игроков. Увеличивает расход диска и время вывода, не изменяет детекцию.",
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
    court_toggle:"Необязательная разметка площадки. Помогает фильтровать неигровые траектории и учитывать положение сетки; неполная разметка может влиять на качество."
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
  function attach(control,key,description){
    const label=control.closest("label");
    if(!label)return false;
    const caption=label.querySelector(":scope > span");
    if(!caption)return false;
    if(caption.classList.contains("setting-help-label"))return true;
    const defaultValue=original(control,key);
    const detail="Влияние: "+description+"\nПо умолчанию: "+defaultValue;
    caption.classList.add("setting-help-label");
    caption.setAttribute("tabindex","0");
    caption.dataset.tooltip=detail;
    caption.setAttribute("aria-label",caption.textContent.trim()+". "+detail);
    const align=()=>{
      caption.classList.toggle("setting-help-right",caption.getBoundingClientRect().left+320>innerWidth-14);
    };
    caption.addEventListener("mouseenter",align);
    caption.addEventListener("focus",align);
    return true;
  }
  function setup(){
    for(const form of ["analysis-form","diagnostics-form","benchmark-form","highlights-form"]){
      for(const control of document.querySelectorAll("#"+form+" input[name], #"+form+" select[name]")){
        const key=control.name;
        attach(control,key,override[form+":"+key]||help[key]||"Настройка параметра обработки.");
      }
    }
    for(const control of document.querySelectorAll("#result-rally-settings input[id]")){
      const key=control.id.replace(/^rally-/,"").replace(/-/g,"_");
      attach(control,key,help[key]||"Настройка отбора игровых эпизодов.");
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
