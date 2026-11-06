# Upstream: влияние обновления от 04.11.2026

Источник: https://github.com/asigatchov/fast-volleyball-tracking-inference
Общий предок: 3aad1b9; новая вершина origin/master: e9c94bc.
Внесены изменения из 52e7b5f, dd8d35e, e9c94bc. Обновлены 46 из 47 upstream-путей, один уже совпадал.

История не слита: рабочий HEAD = ca01007. Коммита и push не было. Файлы Volley Studio сохранены.

## Что меняется

| Область | Файлы | Эффект |
| --- | --- | --- |
| RAVEL-011 | models/RAVEL-VB-011-9f.*, ov/RAVEL-VB-011-9f.* | Обновлены веса и метаданные 011. Новые результаты с этой моделью могут отличаться. |
| RAVEL-013 | ov/RAVEL-VB-013-9f.*, src/inference_player_ball_openvino.py | Добавлена OpenVINO-версия 013, сохранение вероятностей Touch и их временная визуализация. ONNX-веса 013 уже были в нашем форке. |
| Инференс | src/inference_player_ball_openvino.py, src/inference_player_ball_onnx.py, src/inference_onnx_seq_gray_v2.py | Новый необязательный декодер FFmpeg/YUV и NVDEC, потоковый JSONL и возобновление записи. Режим по умолчанию — OpenCV. Сохранены наши ONNX CPU/CUDA-флаги. |
| Касания/розыгрыши | src/player_contacts.py, src/track_calculator_with_court.py, src/predictions_io.py | Добавлена логика объединения подброса мяча и удара при подаче, устранение повторных касаний подающего, чтение новых форматов. При новом пересчёте события могут измениться. |
| Рилсы | src/make_reels.py, src/show_rally.py | Новые параметры визуализации событий/траекторий. Локальные CRF/NVENC и качество нарезки сохранены. В UI новые эффекты не включаются автоматически. |
| Игроки | src/player_reid.py, src/jersey_numbers.py, src/pose_side.py, src/node_agent.py, src/classify_contacts.py | Доступные новые модули, пока не подключены автоматически к вкладке «Игроки». |
| Очередь задач | src/celery_worker.py | Расширения для внешнего Celery-контура; локальные задания Volley Studio от этого напрямую не меняются. |

## Совместимость и ограничения

- По-прежнему доступен входной скрипт RAVEL с --backend openvino / onnx-cuda / onnx-cpu.
- Ключевой контракт CPU-пересчёта сохранён: --save-raw-players создаёт players.raw.jsonl и players.raw.v2.jsonl с сырыми кандидатами и action_scores; стандартный players.json остаётся компактным.
- Одновременные --resume и --save-raw-players явно запрещены, чтобы при возобновлении JSONL не потерять старую часть sidecar. Отдельный --resume без сырых sidecar поддерживается.
- find_touch_events в upstream — сглаженные пики для видеоленты Touch. Он **пока не заменяет** CPU-алгоритм в scripts/volleyball_studio/reprocess.py. Потребуются отдельные замеры на реальных матчах.
- Дефолтный GPU-профиль Volley Studio и существующие кэши не менялись; новая модель и пороги применяются при новом анализе. Существующие версии обработки сохраняются.

## Проверки

- Синтаксис изменённых Python-модулей: OK.
- Прошли тесты RAVEL Touch (6), ONNX backend (3), CPU replay (5), версии постобработки (4), Studio V2 (9), quick reel (5), качество монтажа (9), rally reconstruction (6), целостность результатов (4), стабильность ID (4), Touch preview (5), Sidebar CPU (3).
- Смоук-тест RAVEL-013 ONNX CPU на синтетическом видео: создан players.json и players.raw.v2.jsonl. Обнаружений игроков на тестовых кадрах не было; точность Touch не проверялась.
- Смоук-тест make_reels: MP4, H.264 CRF, два объединённых фрагмента открываются успешно.
- Старый test_studio_ball_threshold.py: 1 ошибка из-за отсутствующего фиктивного проекта; этот тест не проверил передачу порога. Полный upstream-набор pytest не выполнялся: pytest отсутствует в WSL-окружении.

## Безопасность переноса

Бэкап заменённых файлов: C:\Users\amazomic\Desktop\github\volley-upstream-review-20261104\pre-integration.
Манифест хэшей: C:\Users\amazomic\Desktop\github\volley-upstream-review-20261104\applied-paths.json.
Смена HEAD, удаление Volley Studio, коммит и push не выполнялись.
