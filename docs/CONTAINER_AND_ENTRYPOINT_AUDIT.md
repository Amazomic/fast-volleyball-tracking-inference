# Контейнер и точки входа — аудит (06.11.2026)

## Структура и рабочий маршрут

- `scripts/volleyball_ui/server.py` — HTTP-сервер Studio, `scripts/volleyball_ui/static/` — интерфейс.
- `scripts/run_project_analysis.py` — запуск первичного нейросетевого анализа проекта.
- `scripts/run_project_reprocess.py` и `scripts/volleyball_studio/reprocess.py` — независимый CPU-пересчёт сохранённых результатов.
- `scripts/volleyball_ui/player_*` — привязка Track ID и база игроков; `montage_*` — экспорт.
- `src/inference_onnx_seq_gray_v2.py` — самостоятельный инференс мяча; `src/make_reels.py` — CLI монтажа.
- `results/` — результаты анализа, `project/` — видео; **они не должны попадать внутрь образа**.

Контракт: `project -> primary inference cache -> CPU postprocessing version -> preview/player links/montage`. Профили анализа общие, результаты разделены по проектам.

## Исправленные неисправности

1. `Dockerfile` устанавливал зависимости без `--frozen`/`--no-dev`, дублировал `apt-get`, не задавал запуск/порт/healthcheck и оставлял root-пользователя. Новый образ запускает UI, проверяет `/api/health`, хранит данные в томах и использует непривилегированного пользователя. Это **CPU-only** образ.
2. Локальный HTTP-сервер запрещал привязку к контейнерному адресу `0.0.0.0`. Теперь она допускается **только** при явном `VOLLEY_UI_ALLOW_REMOTE=1` (установлен в Dockerfile), без изменения безопасного поведения установки на компьютере.
3. Корневой `main.py` импортировал удалённый `src.inference_onnx` и мог вывести `track`/`analyze` без реальной работы. Теперь `track` запускает действующий `src/inference_onnx_seq_gray_v2.py`, `analyze --project-id` — `scripts.run_project_analysis`; возвращается код дочернего процесса.
4. `src/check_setup.py` проверял отсутствующие старые YOLO-демо. Теперь он проверяет актуальные модели, команды FFmpeg и Python-пакеты; отсутствующие зависимости вызывают ненулевой код завершения.
5. Режим `--mode pose` **не является рабочим**: `src/pose_detector.py` использует пустой список `player_boxes` и фактически не извлекает позы. Совместимый CLI теперь явно отклоняет режим, вместо ложного успешного завершения. Отдельная реализация поз требует реальных боксов игроков и совместимого MediaPipe.

## Docker (локальный доступ)

```bash
docker build -t volley-studio:cpu .
docker run --rm --name volley-studio \
  -p 127.0.0.1:8765:8765 \
  -v volley-results:/app/results \
  -v volley-project:/app/project \
  volley-studio:cpu
```

UI: `http://127.0.0.1:8765`. Не публиковать `0.0.0.0:8765` на хосте: Studio управляет локальными файлами и анализом **без аутентификации**. Для GPU нужен отдельный согласованный образ с `onnxruntime-gpu`, CUDA и NVIDIA Container Toolkit, этот CPU-образ его не обеспечивает.

## Проверки и ограничения

- `python -m unittest discover -s tests -p test_entrypoints_smoke.py` — CLI, валидация, коды возврата, актуальный doctor.
- `python -m compileall -q main.py src/check_setup.py scripts/volleyball_ui/server.py` — компиляция затронутых Python-модулей.
- `python src/check_setup.py` — диагностика на машине с установленными зависимостями.
- `docker build` и `docker inspect --format '{{json .State.Health}}' volley-studio` — обязательный smoke в Docker-среде.

Проверки CLI используют моки дочерних процессов и не означают запуск ONNX/CUDA. В изолированной среде без Docker и зависимостей настоящий контейнер и полный GPU/E2E не проверялись. Ранее существующие большие модули UI не подвергались массовой перестройке без самостоятельных интеграционных тестов.
