# Volleyball Studio

**AI-assisted volleyball match analysis and video highlights.**
**Студия анализа волейбольных матчей и создания видеонарезок с помощью ИИ.**

[Русский](#русский) · [English](#english)

---

## Русский

### О проекте

**Volleyball Studio** — локальное приложение для анализа видеозаписей волейбольных матчей. Оно помогает находить мяч и игроков, восстанавливать траектории и розыгрыши, анализировать касания, сопоставлять треки с игроками и собирать лучшие моменты в видео.

### Что умеет студия

- **Проекты и видео:** загрузка записей, выбор фрагмента для анализа, разметка площадки и подготовка видео.
- **Первичный GPU-анализ:** распознавание мяча и игроков с помощью моделей VballNet и RAVEL; отдельный слой оценки поз RTMPose.
- **CPU-постобработка:** трекинг, геометрия площадки, траектории мяча, кандидаты касаний, их типы и границы розыгрышей.
- **Игроки:** Track ID, предложения по сходству ReID, ручная привязка к конкретным игрокам и фотоальбом игроков.
- **Предпросмотр и статистика:** просмотр розыгрышей, наложения траекторий, контуров и точек тела, фильтры и проверка результатов.
- **Монтаж:** избранные эпизоды, нарезки и отдельный сценарий быстрого рилса.
- **Версии анализа:** сохранение первичного анализа и отдельных вариантов постобработки с разными параметрами.

Автоматическое определение личности игроков и классификация касаний **не гарантируют точности**: ReID выдаёт предположения, а спорные события требуют проверки человеком.

### Как устроено

| Компонент | Назначение |
| --- | --- |
| `rust/volley-app` | Локальный сервер, API, задачи, интерфейс студии |
| `rust/volley-inference` | Работа с моделями и нативными средами инференса |
| `rust/volley-core` | Трекинг, ReID, касания, розыгрыши и CPU-логика |
| `rust/volley-media` | Подготовка видео и монтаж через FFmpeg |
| `rust/volley-web` | WebAssembly-часть интерфейса |

Основное приложение разрабатывается **на Rust**; Python-версия сохранена в истории Git как исходная точка миграции. ONNX Runtime, OpenVINO, CUDA и FFmpeg остаются внешними зависимостями.

### Локальный запуск на Windows / WSL

Нужны Windows с WSL (по умолчанию `Ubuntu-25.04`), Rust toolchain в WSL, FFmpeg/FFprobe и совместимые Linux-библиотеки для используемых моделей. Для GPU-инференса необходимы совместимые NVIDIA/CUDA и ONNX Runtime; ReID на OpenVINO требует соответствующего нативного runtime.

Из корня репозитория:

```bat
start-volley-ui.cmd
start-volley-ui.cmd /doctor
start-volley-ui.cmd /status
```

Либо через PowerShell:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/start-volley-rust-dev.ps1
```

Интерфейс: **http://127.0.0.1:8875/studio**. Приложение предназначено для локального использования; не публикуйте API напрямую в интернет.

Проверка Rust-кода из каталога `rust/`: `cargo test --workspace --offline`. Проверки UI из корня: `node scripts/ci/test_studio_wasm_ui.mjs`. Подробности установки и диагностики — в [руководстве по развёртыванию](docs/RUST_DEPLOYMENT.md).

### Статус

**Активная разработка / экспериментальная версия.** Наличие функции в интерфейсе не означает подтверждённой точности на реальных матчах. Полная приёмка, сопоставление с исторической Python-реализацией и измерения качества ещё не завершены. Исходные видеозаписи и исторические результаты не должны автоматически изменяться при анализе.

**Документация:** [Архитектура](docs/RUST_ARCHITECTURE.md) · [Карта разработки](docs/RUST_ROADMAP.md) · [Журнал миграции](docs/RUST_MIGRATION.md) · [Развёртывание](docs/RUST_DEPLOYMENT.md) · [Руководство разработчика](rust/README.md).

---

## English

### About

**Volleyball Studio** is a local application for analyzing recorded volleyball matches. It helps detect the ball and players, reconstruct trajectories and rallies, examine touches, associate tracked identities with known athletes, and assemble video highlights.

### Features

- **Projects and video:** import recordings, select the analysis interval, annotate court geometry, and prepare footage.
- **Primary GPU analysis:** ball and player detection using VballNet and RAVEL, with an additional RTMPose body-keypoint layer.
- **CPU post-processing:** tracking, court-aware geometry, ball trajectories, touch candidates and types, and rally segmentation.
- **Players:** Track IDs, ReID similarity suggestions, manually confirmed player assignments, and a player photo album.
- **Preview and statistics:** rally playback, trajectory/player/keypoint overlays, filters, and human review.
- **Editing:** favorite rallies, video highlights, and a separate Quick Reel workflow.
- **Analysis versions:** saved GPU evidence with independently configurable post-processing variants.

Automated player identification and touch classification are **not guaranteed to be accurate**. ReID results are suggestions, and uncertain events should be reviewed by a human.

### Architecture

| Component | Responsibility |
| --- | --- |
| `rust/volley-app` | Local server, API, jobs, and studio UI |
| `rust/volley-inference` | Models and native inference runtimes |
| `rust/volley-core` | Tracking, ReID, touches, rallies, and CPU logic |
| `rust/volley-media` | Video preparation and FFmpeg-based editing |
| `rust/volley-web` | WebAssembly UI components |

The active application is developed in **Rust**. The original Python implementation remains available in Git history as the migration baseline. ONNX Runtime, OpenVINO, CUDA, and FFmpeg are external dependencies.

### Run locally on Windows / WSL

Requirements include Windows with WSL (`Ubuntu-25.04` by default), a Rust toolchain inside WSL, FFmpeg/FFprobe, and compatible Linux libraries for the selected models. GPU inference requires a compatible NVIDIA/CUDA setup and ONNX Runtime; OpenVINO ReID requires its native runtime.

From the repository root:

```bat
start-volley-ui.cmd
start-volley-ui.cmd /doctor
start-volley-ui.cmd /status
```

Or use PowerShell:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/start-volley-rust-dev.ps1
```

Studio UI: **http://127.0.0.1:8875/studio**. This is a localhost application; do not expose its API directly to the public internet.

Run Rust tests from `rust/` with `cargo test --workspace --offline`; run the UI check from the repository root with `node scripts/ci/test_studio_wasm_ui.mjs`. See the [deployment guide](docs/RUST_DEPLOYMENT.md) for setup and diagnostics.

### Project status

**Active development / experimental release.** A visible feature is not proof of accuracy on real matches. End-to-end acceptance, comparison with the historical Python implementation, and comprehensive quality measurements are still pending. Original footage and historical results are not intended to be silently overwritten.

**Documentation:** [Architecture](docs/RUST_ARCHITECTURE.md) · [Roadmap](docs/RUST_ROADMAP.md) · [Migration log](docs/RUST_MIGRATION.md) · [Deployment](docs/RUST_DEPLOYMENT.md) · [Developer guide](rust/README.md).
