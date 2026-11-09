# Дорожная карта Rust-миграции Volleyball Studio

Источник данных: `docs/RUST_ROADMAP.json`. Команда генерации: `node scripts/ci/check_rust_roadmap.mjs --write`; проверка: `node scripts/ci/check_rust_roadmap.mjs --check --self-test`.

## Общий прогресс

**Последний записанный этап: 75 / 85. Следующий: 76 / 85.**

В журнале: 69 практических этапов и 6 начальных архитектурных разделов. Осталось по текущему плану: 10.

**Важно:** счётчик описывает покрытие дорожной карты, а НЕ процент готовности продукта. У старых этапов могли остаться неподключённые адаптеры и незавершённая функциональность.

85 — текущий ориентир планирования, не обещание неизменного объема; этап != готовая подсистема. При пересмотре плана обновлять JSON, Markdown и журнал в одном коммите.

После тестов и коммита следующего этапа добавьте запись в `docs/RUST_MIGRATION.md`, переведите соответствующий JSON-пункт в `recorded`, обновите `lastRecorded`/`nextStage` и повторно сгенерируйте карту. Изменение числа этапов требует явного пересмотра плана и общей проверки.

## Все 85 этапов

Статусы: **Основа** = архитектурная запись, **Записан** = результат/частичный шаг в журнале, **План** = ещё не реализовано.

### Основание и правила

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 1 / 85 | Основа | Target architecture and safety contract |
| 2 / 85 | Основа | Complete rewrite plan — replaces incremental test gates |
| 3 / 85 | Основа | Final evaluation and rollback protocol |
| 4 / 85 | Основа | Stage log (append, never silently rewrite historic results) |
| 5 / 85 | Основа | Handoff to the next AI / engineer |
| 6 / 85 | Основа | Superseding decision |

### Существующие CPU-функции

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 7 / 85 | Записан | R01 initial implementation checkpoint |
| 8 / 85 | Записан | Full rewrite continuation: native project/catalog/history/media layers |
| 9 / 85 | Записан | Native Rust video-preparation lifecycle |
| 10 / 85 | Записан | Rust-native HTTP preview and prepared MP4 streaming |
| 11 / 85 | Записан | Native-source manual montage engine and sandbox API |
| 12 / 85 | Записан | Native CPU-evidence rally reconstruction and selection |
| 13 / 85 | Записан | Rust-only RAVEL Touch peak and motion diagnostic |
| 14 / 85 | Записан | Native provisional player continuity and safe Touch grouping |
| 15 / 85 | Записан | Rust-native cached ReID comparison |
| 16 / 85 | Записан | Isolated Rust manual player identities |
| 17 / 85 | Записан | Native manual ReID candidate group merge/split |
| 18 / 85 | Записан | Native CPU player touch/action statistics |
| 19 / 85 | Записан | Native personalized full-rally montage selection |
| 20 / 85 | Записан | Native gold-marked rallies and montage filters |
| 21 / 85 | Записан | Rust Quick Reel independent selection, preparation and job state |

### Нативный GPU/ONNX

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 22 / 85 | Записан | Native ONNX Runtime one-clip GPU inference kernel |
| 23 / 85 | Записан | Native global player catalog editor |
| 24 / 85 | Записан | Safe WSL Rust developer launch command |
| 25 / 85 | Записан | Isolated Rust project archival and task cancellation |
| 26 / 85 | Записан | Bounded raw-frame streaming windows and shared ONNX input layout |
| 27 / 85 | Записан | Single-FFmpeg raw-frame window streaming |
| 28 / 85 | Записан | Persistent ONNX session over bounded native FFmpeg stream |
| 29 / 85 | Записан | Versioned isolated native ONNX proposal evidence |
| 30 / 85 | Записан | Touch smoothing regression fixture corrected; full Rust unit suite green |
| 31 / 85 | Записан | Native RAVEL-013/VballNet ONNX detection decoder |
| 32 / 85 | Записан | Full prepared-video FFmpeg streaming input for native ONNX |
| 33 / 85 | Записан | Expose unconfirmed per-frame detections from native ONNX scans |
| 34 / 85 | Записан | Native full-video model detection job lifecycle |
| 35 / 85 | Записан | Actual ORT CPU/CUDA inference and isolated full-video GPU smoke |
| 36 / 85 | Записан | Verified RAVEL + VballNet source fusion into isolated CPU replay frames |
| 37 / 85 | Записан | End-to-end native CUDA model fusion smoke |

### CPU-трекинг и предпросмотр

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 38 / 85 | Записан | Provisional CPU Track ID reconstruction from RAVEL boxes |
| 39 / 85 | Записан | Isolated CPU replay of provisional player tracks from immutable GPU fusion |
| 40 / 85 | Записан | CPU physical Touch gating for native fused detections |
| 41 / 85 | Записан | Full native CPU motion-gated Touch proposal replay |
| 42 / 85 | Записан | Streaming ball-supported provisional rally reconstruction |
| 43 / 85 | Записан | Native CPU rally snapshot linked to verified Track ID and Touch inputs |
| 44 / 85 | Записан | Conservative event evidence labels and safe provisional clip boundaries |
| 45 / 85 | Записан | Verified native rally preview and manual original-source montage route |
| 46 / 85 | Записан | Rust-served first standalone preview/montage UI, native gold marks and global filtered paging |

### Rust/WASM UI и игроки

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 47 / 85 | Записан | First real Rust/WebAssembly client engine compiled and executed |
| 48 / 85 | Записан | Native Analysis and Players tabs with explicit GPU/CPU version flow |
| 49 / 85 | Записан | GPU job progress survives Rust Studio page reload |
| 50 / 85 | Записан | Standalone native provisional Track ID ↔ project player manual registry |
| 51 / 85 | Записан | Native Track ID manual review UI and verified crop-on-demand photo |

### Защита архитектуры

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 52 / 85 | Записан | Duplication audit, canonical ownership map and mandatory no-repeat gate |
| 53 / 85 | Записан | Remove duplicated mutable sidecar implementations, preserve legacy/native schemas |

### Контракты

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 54 / 85 | Записан | Типизированный EvidenceSource и проверка происхождения |

### Игроки

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 55 / 85 | Записан | Два адаптера чтения Track ID для одного identity-сервиса |
| 56 / 85 | Записан | Rust-only executable entrypoint and retirement of Python application tree |
| 57 / 85 | Записан | Перевод старых и нативных API на общий identity-сервис |

### ReID

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 58 / 85 | Записан | Проверка совпадений и конфликтов визуальных ReID предложений |

### Игроки

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 59 / 85 | Записан | Общие подтверждённые identity решения для статистики и монтажа |

### ReID

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 60 / 85 | Записан | Извлечение ReID embeddings из новых native Track ID |
| 61 / 85 | Записан | Сохранение фотоэталонов игрока с проверкой источника |
| 62 / 85 | Записан | Версионирование модели embeddings и инвалидирование кеша |
| 63 / 85 | Записан | Общий ReID cosine и предложения между группами Track ID |
| 64 / 85 | Записан | Ручная проверка ReID групп и порога автопредложений |
| 65 / 85 | Записан | Проверка одновременной видимости и отсутствия подмены игроков |

### Трекинг

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 66 / 85 | Записан | Консервативная непрерывность Track ID при разрыве 3–4 секунды |

### Касания

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 67 / 85 | Записан | Единая траектория мяча и анализ экстремумов изменения движения |
| 68 / 85 | Записан | Подтверждение физически обоснованных касаний вместо геометрии |

### Розыгрыши

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 69 / 85 | Записан | Сопоставление границ розыгрышей native/legacy |
| 70 / 85 | Записан | Единая политика золотых отметок и прежние сохранения |
| 71 / 85 | Записан | Единый selector розыгрышей для предпросмотра и монтажа |

### События

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 72 / 85 | Записан | Классификация подач, атак и блоков по подтверждённым событиям |

### Статистика

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 73 / 85 | Записан | Пересчёт касаний конкретных игроков по всему проекту |
| 74 / 85 | Записан | Панели статистики игрока и качества событий в UI |

### Монтаж

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 75 / 85 | Записан | Единый рендер/монтаж из оригинала для обоих форматов |
| 76 / 85 | План | Автонарезка лучших атак выбранного игрока. Критерий: Uses verified identity/action and manual override; cancels cleanly |
| 77 / 85 | План | Вертикальный монтаж, аудиодорожка и проверка качества MP4. Критерий: Correct aspect and synchronized audio with 100% original clip extraction |

### Quick Reel

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 78 / 85 | План | Независимый Quick Reel: свежий GPU→CPU→монтаж. Критерий: Reuses only media preparation; no project inference reuse |
| 79 / 85 | План | Quick Reel: выбор файла, отмена, прогресс и скачивание. Критерий: Full client flow, no stale file picker or duplicate jobs |

### Совместимость

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 80 / 85 | План | Совместимость старых проектов и безопасная очистка/архивация. Критерий: Reopen real old project; no user data destruction |

### Интерфейс

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 81 / 85 | План | Полноценный Rust/WASM интерфейс с RU/EN и настройками. Критерий: No legacy JS feature gaps; browser navigation and loading tests |

### Развёртывание

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 82 / 85 | План | Windows/WSL/Docker сборка, ресурсы и один рабочий экземпляр. Критерий: Stable launch/stop/restart, no Python service dependency |

### Приёмка

| Этап | Состояние | Задача / критерий |
|---:|---|---|
| 83 / 85 | План | Регрессионный аудит security, SHA, cancellation и кеша. Критерий: All CI gates, fault injection, cancellation, bounded RAM/VRAM |
| 84 / 85 | План | Полный тест на размеченных матчах и сравнение с Python. Критерий: Measured touch/Track ID/rally quality, speed, GPU/RAM, montage fidelity |
| 85 / 85 | План | Финальный выбор Rust-релиза или безопасного отката. Критерий: Accepted results, selected commit, reversible user-data-safe rollout |

## Неизменяемые правила процесса

- Перед каждым этапом проверять Git-историю, владельцев функций и уже написанный код.
- Нельзя создавать второго владельца ReID, привязок, золотых отметок или монтажа.
- Новые/старые форматы поддерживаются адаптерами, а не двумя бизнес-реализациями.
- Завершение этапа требует тестов, коммита, записи в журнале и обновлённой карты.
- The Python baseline is preserved only by the immutable Git checkpoint. Active application, launcher and UI are Rust-only; original recordings and user caches remain untouched.
- Готовность приложения подтверждается только реальными матчами и критериями паритета.
