"""Compile the self-hosted English locale and audit untranslated UI labels."""
from pathlib import Path
import json,re,sys

ROOT=Path(__file__).resolve().parents[2]
static=ROOT/"scripts/volleyball_ui/static"
sources=json.loads((ROOT/"results/i18n_extract_strings.json").read_text("utf-8"))
translations=json.loads((static/"i18n-en.json").read_text("utf-8"))
curated={
    "Розыгрыши":"Rallies","Найденные розыгрыши":"Detected rallies",
    "Выберите розыгрыши для монтажа":"Select rallies for montage",
    "Розыгрыш":"Rally","Розыгрыш №":"Rally #",
    "розыгрыши":"rallies","розыгрышей":"rallies","розыгрыш":"rally",
    "Предпросмотр":"Preview","Монтаж":"Montage","Анализ":"Analysis",
    "Игроки":"Players","игроков":"players","Игроков":"Players",
    "количество игроков":"player count","Количество игроков":"Player count",
    "Игроки в розыгрыше":"Players in rally",
    "Касания":"Touches","касаний":"touches",
    "Бенчмарк":"Benchmark","Приёмы":"Digs","Подачи":"Serves",
    "Атаки":"Attacks","Блоки":"Blocks","Передачи":"Sets",
    "Верхние передачи":"Overhead sets","Подача":"Serve",
    "Время":"Time","Сортировка":"Sort order",
    "Результат анализа":"Analysis result",
    "Создать проект":"Create project","+ Создать проект":"+ Create project",
    "Выберите проект":"Select a project",
    "В ручной монтаж":"Include in manual montage",
    "В монтаж":"For montage",
    "Добавить этот розыгрыш в ручной монтаж":"Add this rally to manual montage",
    "Открыть розыгрыши в предпросмотре":"Select rallies in Preview",
    "Для ручного монтажа отмечайте розыгрыши · видео проекта: ":"Select rallies for manual montage · project videos: ",
    "Видео для автоматического монтажа: ":"Videos for automatic montage: ",
    "Исходные видео":"Source videos","видео":"videos",
    "Выбрать вручную":"Select manually","Автоматически":"Automatic",
    "Автоматический монтаж":"Automatic montage",
    "Ручной монтаж":"Manual montage","Найдено":"Found",
    "Уверенность":"Confidence","уверенность":"confidence",
    "Видео проекта":"Project videos","Результаты":"Results",
    "Отчёт":"Report","Отчёт проекта":"Project report",
    "Полный экран":"Fullscreen","Отмена":"Cancel",
    "Сохранить":"Save","Удалить":"Delete","Повторить":"Retry",
    "Загрузка":"Loading","Траектория мяча":"Ball trajectory",
    "По касаниям ↓":"By touches ↓","По длительности ↓":"By duration ↓",
    "Обычная (по времени)":"Default (chronological)",
    "Новый":"New","Выберите видео":"Select a video",
    "Подготовка видео":"Video preparation","Настройки":"Settings",
    "Профиль":"Profile","Название проекта":"Project name",
    "Секунды":"Seconds","Кадры":"Frames",
    "Язык интерфейса":"Interface language",
    "Обработка видео":"Processing videos",
    "ХОД ОБРАБОТКИ":"PROCESSING STATUS",
    "РАБОЧЕЕ ПРОСТРАНСТВО":"WORKSPACE",
    "Ожидание анализа":"Waiting for analysis",
    "Видео для ручного монтажа":"Videos for manual montage",
    "Отметить в предпросмотре":"Select in Preview",
    "Длительность":"Duration","длительность":"duration",
    "действий":"actions","Действий":"Actions",
    "атак":"attacks","блоков":"blocks",
    "Включён автоматически":"Selected automatically",
    "Не вошёл в монтаж":"Not included in montage",
    "Выбран для монтажа":"Selected for montage",
    "Нажмите для выбора":"Click to select",
    "по времени":"by time","по длительности":"by duration",
    "Воспроизвести":"Play","Пауза":"Pause",
    "Подробности":"Details",
    "КАСАНИЕ":"TOUCH","Траектория":"Trajectory",
    "Результаты анализа":"Analysis results",
    "События анализа":"Analysis events",
}
curated.update({
    "Десять последовательных реальных измерений на вашем компьютере. Одни и те же кадры исходного видео, прогревы и число повторов; входной размер подбирается под каждую модель. FFmpeg и предварительная обработка не входят в FPS.":
    "Ten sequential, real measurements on your computer. All runs use the same original video frames, warm-ups, and repetitions. Input dimensions are specific to each model. FFmpeg and preprocessing are excluded from the FPS measurement.",
    "Измеряем синхронный инференс ONNX Runtime на реальных кадрах, без декодирования/resize в таймере. FPS = 9 кадров / медианная задержка. RAVEL и VballNet — разные задачи, поэтому результаты сравниваются внутри своих групп. CPU и GPU не работают одновременно.":
    "We measure synchronous ONNX Runtime inference on real frames, excluding decoding and resizing from the timer. FPS = 9 frames / median latency. RAVEL and VballNet perform different tasks, so results are compared within their own groups. CPU and GPU tests do not run simultaneously.",
    "Не влияет на распознавание. При необходимости отдельно запускает штатные track_processor.py и show_rally.py по уже сохранённым трекам текущего видео. Для обычного просмотра достаточно разметки поверх плеера, экспорт не нужен.":
    "Does not affect recognition. Optionally runs track_processor.py and show_rally.py using the current video's saved tracks. For normal viewing, the overlay on the player is sufficient; exporting is not necessary.",
    "Чем длиннее столбец, тем выше скорость. Для одной модели CPU и CUDA получают идентичный входной тензор. Входные разрешения разных моделей отличаются — это их штатные конфигурации. График отражает скорость инференса, не точность и не скорость полного видеоанализа.":
    "Longer bars indicate faster inference. For each model, CPU and CUDA use identical input tensors. Different models have different native input resolutions. The chart shows inference speed, not accuracy or full-video analysis performance.",
    "1. Создайте игрока → 2. Отметьте его ID → 3. Нажмите «Привязать»":
    "1. Create a player → 2. Select their IDs → 3. Click Link",
    "Границы розыгрышей и классификация действий определены автоматически и могут быть неточными. Метки на видео появляются в моменты обнаруженных событий.":
    "Rally boundaries and action labels are estimated automatically and may be inaccurate. Video markers appear at detected event times.",
    "Дополнительные условия отбора. Ноль — без ограничения. Считаются распознанные действия в каждом розыгрыше.":
    "Additional selection filters. Zero means no minimum. Recognized actions are counted per rally.",
    "Выбрать профиль анализа для монтажа":"Select analysis profile for montage",
    "Завершённые стадии монтажа":"Completed montage stages",
    "Загрузка профилей…":"Loading profiles…",
    "Запас до/после розыгрыша, с":"Padding before/after rally, sec",
    "Запуск анализа для монтажа":"Analysis run for montage",
    "Минимальная уверенность розыгрыша":"Minimum rally confidence",
    "Минимум розыгрыша, с":"Minimum rally length, sec",
    "Монтаж проекта":"Project montage",
    "Найденные розыгрыши для монтажа":"Detected rallies for montage",
    "Настройки формирования розыгрышей":"Rally grouping settings",
    "Остановить монтаж":"Stop montage",
    "Отбирать розыгрыши по":"Select rallies by",
    "Отдельные MP4 по розыгрышам":"Separate MP4 files per rally",
    "Отметьте исходные видео, розыгрыши из которых нужны для ручного монтажа. Нажмите название для предпросмотра.":
    "Select the source videos whose rallies you want in a manual montage. Click a video name to preview it.",
    "Отметьте нужные полные розыгрыши ниже. Общая длительность рассчитывается автоматически, без ограничения из автоматического режима.":
    "Select the complete rallies below. Total duration is calculated automatically, without the automatic mode's duration limit.",
    "Отмечайте нужные розыгрыши в «Предпросмотре» или ниже — выбор синхронизируется. Итоговая длительность рассчитывается автоматически.":
    "Select rallies in Preview or below; selections stay synchronized. Total duration is calculated automatically.",
    "Предыдущие розыгрыши":"Previous rallies",
    "При автоматическом отборе итоговое видео не превышает заданную длительность. Полные розыгрыши не обрезаются.":
    "Automatic montage stays within the requested duration without cutting rallies short.",
    "Прогресс монтажа":"Montage progress",
    "Прокрутка розыгрышей":"Scroll through rallies",
    "Режим выбора розыгрышей":"Rally selection mode",
    "Розыгрыши и игровые действия":"Rallies and game actions",
    "Розыгрыши и игровые события":"Rallies and game events",
    "Розыгрыши появятся после анализа.":"Rallies appear after analysis.",
    "Розыгрыши с атаками":"Rallies with attacks",
    "Розыгрыши с блоками":"Rallies with blocks",
    "Розыгрыши с передачами":"Rallies with sets",
    "Розыгрыши с подачами":"Rallies with serves",
    "Розыгрыши с приёмами":"Rallies with digs",
    "Самые длинные розыгрыши":"Longest rallies",
    "Самые зрелищные полные розыгрыши из всех записей проекта в одном итоговом ролике.":
    "The best complete rallies from all project recordings, combined into a single highlight video.",
    "Следующие розыгрыши":"Next rallies",
    "Сортировка найденных розыгрышей":"Sort detected rallies",
    "Ход монтажа":"Montage progress",
    "Ход обработки и журнал находятся во вкладке «Анализ». Видео с разметкой и розыгрыши появятся здесь после завершения анализа.":
    "Processing status and logs are available in the Analysis tab. Annotated video and rallies will appear here after analysis finishes.",
    "Касания RAVEL-013":"RAVEL-013 touches",
    "Касания RAVEL-013 · без эвристик":"RAVEL-013 touches · raw model output",
    "Загружаем базу игроков…":"Loading player database…",
    "Разметка событий":"Event overlays",
    "Исходные видео":"Source videos",
    "Добавить розыгрыш":"Add rally",
    "Видео с разметкой":"Annotated video",
    "Здесь появятся сообщения монтажа.":"Montage messages will appear here.",
    "Игроки доступны во всех проектах. Выберите человека, чтобы назначить ему ID; отсутствие ID не удаляет карточку.":
    "Player profiles are shared across projects. Choose a player to assign a Track ID. Removing all IDs does not delete the player profile.",
})

curated.update({
    "нет":"missing","Ошибка":"Error","Ошибка:":"Error:",
    "Запущен":"Started","Завершён":"Completed",
    "Завершено":"Completed","Выполняется":"In progress",
    "Стадия":"Stage","Стадии с замером":"Timed stages",
    "Тип запуска":"Run type","Весь проект":"Entire project",
    "Общее время анализа":"Total analysis time",
    "Обнаружение мяча":"Ball detection",
    "Обнаружение и отслеживание игроков":"Player detection and tracking",
    "Построение траекторий и розыгрышей":"Ball trajectories and rally grouping",
    "Классификация игровых событий":"Game event classification",
    "Подготовка результатов анализа":"Preparing analysis results",
    "Проверка подготовленного видео / диапазона":"Validating prepared video and time range",
    "Проверка подготовленных видео / диапазона":"Validating prepared videos and time range",
    "Проверка параметров и результатов анализа":"Validating montage settings and analysis results",
    "Обнаружение":"Detection",
    "Стадия анализа":"Analysis stage",
    "Стадия монтажа":"Montage stage",
    "Подготовка видео":"Video preparation",
    "Сохранённый контекст по":"Saved context for",
    "Сохранённый контекст по видео":"Saved context for videos",
    "Без разметки площадки":"No court markings",
    "Анализ всех":"Analyze all",
    "всех":"all","всех видео":"all videos",
    "видео проекта":"project videos",
    "проекта":"of the project",
    "Параметры применяются ко всем файлам":"Settings apply to all files",
    "временные диапазоны настраиваются отдельно":"time ranges are configured separately",
    "Настройки обнаружения мяча VballNet для полного анализа проекта.":
    "VballNet ball detection settings for analyzing the entire project.",
    "Настройки обнаружения мяча VballNet":"VballNet ball detection settings",
    "обнаружения мяча VballNet для полного анализа проекта.":
    "VballNet ball detection for the entire project.",
    "Выберите видео справа, задайте диапазон под проигрывателем и параметры запуска ниже.":
    "Select a video on the right, set the range below the player and configure analysis settings.",
    "справа, задайте диапазон под проигрывателем и параметры запуска ниже.":
    "on the right, set the range below the player and configure analysis settings.",
    "Видео не найдено в project":"Video not found in project",
    "Видео не найдено в проекте":"Video not found in project",
    "Старт":"Started","Всего":"Total",
    "кадр/с":"fps","кадра/с":"fps","кадров/с":"fps",
    "запуск":"run","запуска":"runs","запусков":"runs",
    "потоков":"threads",
    "Ожидание":"Waiting",
    "Найденные розыгрыши":"Detected rallies",
    "Описание розыгрыша":"Rally details",
    "Количество игроков":"Player count",
    "игроков":"players",
})

curated.update({
    "Недоступны исходные видео:":"Unavailable source videos:",
    "Проверьте расположение исходных файлов.":"Check the source file paths.",
    "из":"of","в монтаж":"for montage",
    "монтаж":"montage","монтажа":"montage","монтажей":"montages",
    "игрока":"player","игроков":"players",
    "Игроков создано":"Player profiles created",
    "Создано":"Created","создано":"created",
    "Привязано ID":"Track IDs linked",
    "Осталось разобрать":"Pending review",
    "Пока нет ID на этом видео":"No Track IDs for this video yet",
    "фото в личной базе":"photos in the player library",
    "На этом видео пока нет найденных ID. Сначала запустите анализ игроков во вкладке «Анализ».":
    "No Track IDs have been found for this video. Run player analysis in the Analysis tab first.",
    "По этому фильтру ничего не найдено.":"No results match this filter.",
    "Показано":"Shown","всего":"total",
    "Выберите игрока из состава":"Select a player from the roster",
    "Нажмите на фото или отметьте ID в таблице":
    "Click a photo or select Track IDs in the table",
    "Выберите ID, чтобы проверить продолжения":
    "Select a Track ID to review possible continuations",
    "Нет обработанных записей проекта. Перейдите во вкладку «Анализ».":
    "This project has no analyzed videos. Go to the Analysis tab.",
    "Нет обработанных записей":"No analyzed recordings",
    "Фрагменты игры":"Game segments",
    "Не подтверждённые очки":"Unverified points",
    "Распознанные":"Detected",
    "Пока нет событий. Запустите анализ.":
    "No game events yet. Run analysis.",
    "Пока нет подписанных игроков. Перейдите во вкладку «Игроки и ID».":
    "No associated players yet. Go to Players & IDs.",
    "Вертикальный рилс 9:16: камера удерживает мяч в кадре.":
    "Vertical 9:16 reel: the virtual camera keeps the ball in frame.",
    "Розыгрыши из":"Rallies from",
    "один общий отбор для обоих вариантов кадрирования.":
    "one selection shared by both framing modes.",
    "Результаты профиля не выбраны.":"Analysis profile results not selected.",
    "Найденные розыгрыши · автоматический отбор":
    "Detected rallies · automatic selection",
    "Выберите проект и завершённый профиль анализа.":
    "Select a project and a completed analysis run.",
    "Видео для автоматического монтажа:":"Videos for automatic montage:",
    "Видео для ручного монтажа:":"Videos for manual montage:",
    "Для выбранного запуска анализа монтажей пока нет.":
    "No montages exist for the selected analysis run.",
    "Все пять моделей установлены. Замеры: CPU, затем CUDA, по очереди.":
    "All five models are installed. Benchmarks run sequentially on CPU, then CUDA.",
    "Выберите движок и запустите тест.":
    "Choose an inference engine and start the benchmark.",
    "Нужен один полный набор измерений":"One complete set of measurements is required",
    "Нет результатов пяти моделей. Запустите последовательный тест CPU и CUDA.":
    "No benchmark results for all five models. Run the sequential CPU and CUDA tests.",
    "Новых серий ещё нет. Старые измерения двух моделей сохранены в истории на диске.":
    "No new benchmark runs yet. Previous two-model results are kept in the on-disk history.",
})

curated.update({
    "недоступно":"unavailable",
    "Сбросить":"Reset",
    "Сбросить диапазон и разметку для":"Reset the time range and court markings for",
    "Сбросить контекст этого видео":"Reset saved context for this video",
    "Потоки CPU (зависит от профиля)":"CPU threads (profile-dependent)",
    "Отличия от профиля по умолчанию «Новый»":"Differences from default profile 'New'",
    "Количество отличий":"Number of differences",
    "Отличия настроек профиля":"Profile settings differences",
    "Все параметры совпадают с профилем по умолчанию.":"All settings match the default profile.",
    "VballNetV4c — точнее (F1 90,2%)":"VballNetV4c — more accurate (F1 90.2%)",
    "VballNetFastV1 — быстрее в инференсе (F1 79,9%)":"VballNetFastV1 — faster inference (F1 79.9%)",
    "VballNet находит мяч; RAVEL анализирует игроков отдельно. Порог ниже — больше находок и ложных срабатываний. Реальную скорость нейросетей на CPU и GPU смотрите в «Бенчмарке»: ускорение всего анализа зависит также от декодирования и RAVEL.":
    "VballNet detects the ball; RAVEL analyzes players separately. A lower threshold yields more detections and false positives. See Benchmark for actual CPU and GPU inference speed; complete analysis also depends on decoding and RAVEL.",
    "VballNet находит мяч; RAVEL анализирует игроков отдельно. Порог ниже — больше находок и ложных срабатываний. Указанные FPS измерены на CPU: ускорение полного анализа на GPU зависит также от декодирования и RAVEL.":
    "VballNet detects the ball; RAVEL analyzes players separately. A lower threshold yields more detections and false positives. The listed FPS values are CPU-only benchmarks; total GPU analysis speed also depends on video decoding and RAVEL.",
    "Число CPU-потоков зависит от профиля: GPU RAVEL-013 использует все доступные логические CPU (до 128), остальные профили — 80%. Настройку можно изменить. Потоки используются и вспомогательными этапами при работе на GPU.":
    "CPU threads depend on the profile: GPU RAVEL-013 uses all available logical CPUs (up to 128); other profiles default to 80%. This can be changed. CPU threads are also used by supporting stages during GPU inference.",
    "Диагностика сохранённого анализа":"Saved analysis diagnostics",
    "Видео для диагностики":"Video for diagnostics",
    "Видео для диагностического экспорта":"Video for diagnostic export",
    "Выберите исходное видео для просмотра. Для ручного монтажа отмечайте отдельные розыгрыши справа.":
    "Choose a source video to preview. For manual montage, check individual rallies on the right.",
    "контекст этого видео":"saved context for this video",
    "Точки разметки волейбольной площадки":"Volleyball court calibration points",
    "Плотность предсказаний касаний RAVEL-013 по всему видео. Нажмите, чтобы перейти к моменту.":
    "RAVEL-013 touch prediction density across the full video. Click to jump to a moment.",
})

translations.update(curated)
for key,value in list(translations.items()):
    if not isinstance(value,str):continue
    if re.search("розыгрыш",key,re.I) and key not in curated:
        for a,b in (("Draws","Rallies"),("Draw","Rally"),("draws","rallies"),
                    ("draw","rally"),("Raffles","Rallies"),("Raffle","Rally"),
                    ("raffles","rallies"),("raffle","rally"),("lottery","rally"),
                    ("prize draws","rallies"),("pranks","rallies"),("prank","rally"),
                    ("Pranks","Rallies"),("Prank","Rally"),
                    ("drawings","rallies"),("drawing","rally"),
                    ("Drawings","Rallies"),("Drawing","Rally"),
                    ("Practise","Rallies"),("practise","rallies")):
            value=value.replace(a,b)
    if re.search("монтаж",key,re.I) and key not in curated:
        for a,b in (("installation","montage"),("Installation","Montage"),
                    ("mounting","editing"),("Mounting","Editing")):
            value=value.replace(a,b)
    translations[key]=value
output=static/"i18n-en.js"
output.write_text("/* Generated offline English UI catalogue. */\nwindow.VolleyEnglish="+
    json.dumps(translations,ensure_ascii=False,separators=(",",":"))+";\n",encoding="utf-8")
html_untranslated=[x for x in sources["html"] if re.search(r"[а-яё]",translations.get(x,x),re.I)]
js_untranslated=[x for x in sources["js"] if re.search(r"[а-яё]",translations.get(x,x),re.I)]
print("EN",len(translations),"HTML untranslated",len(html_untranslated),
      "JS fragments untranslated",len(js_untranslated),"bytes",output.stat().st_size)
for x in html_untranslated[:50]:print("UNTRANSLATED HTML",x[:130])
