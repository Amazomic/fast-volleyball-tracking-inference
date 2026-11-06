# Проверки Pull Request без GitHub Actions

GitHub Actions отключены удалением workflow-файлов, поэтому никаких
автоматических проверок при push или PR нет. Запускаем тесты на своём
компьютере без облачного runner и платёжной карты.

## Быстрая проверка (перед каждым PR)

Требуются Python 3.14+ и Node.js 22+.

```bash
python -m scripts.ci.run_checks
```

Выполняются синтаксис Python и JavaScript, тесты правил архитектуры,
золотые настройки, unit-тесты проверки качества и UI-контракты Node.
Если Node.js отсутствует, можно явно запустить
`python -m scripts.ci.run_checks --skip-ui`, но записать **UI NOT TESTED**.

## Полный прогон

При изменениях распознавания, CPU-постобработки, проектов или монтажа:

```bash
python -m scripts.ci.run_checks --full
```

Дополнительно нужны `uv`, `ffmpeg`/`ffprobe` и зависимости проекта.
Полный прогон синхронизирует lockfile и запускает весь `pytest -q tests`.
Он может нагружать CPU; не запускать тяжёлые тесты на компьютере с
активным GPU-анализом без учёта нагрузки.

## Матрица проверки

| Изменения | Обязательные дополнительные проверки |
| --- | --- |
| UI / фильтры | `node --test tests/*.cjs`, ручное переключение проектов и F5 |
| Touch / траектория мяча | `tests/test_ravel_touch_motion.py`, реальные сцены без касания |
| Игроки / ReID | `tests/test_player_three_second_gap.py`, сцены с пересечением игроков |
| Версии CPU и проекты | `tests/test_gpu_cpu_chain.py`, `tests/test_reprocess_pipeline.py` |
| Quick Reel / монтаж | `tests/test_quick_reel.py`, `tests/test_montage_original_quality.py`, просмотр MP4 |
| Docker | Ручной `docker build` и `/api/health` |
| Модель / GPU | Ручной бенчмарк на разрешённом видео, подтверждение CUDA provider |

Для изменения золотых CPU-настроек необходимы сравнимые реальные
аннотации и отчёт `scripts/ci/quality_eval.py` — см.
[docs/quality/README.md](quality/README.md).

## Слияние

В описание PR вставить команды и фактический результат PASS/FAIL/NOT TESTED.
Сверить архитектурные инварианты, качество исходного MP4 и отсутствие
изменений живых проектов. Затем review и merge в `master`.
**Автоматического merge gate нет.**
