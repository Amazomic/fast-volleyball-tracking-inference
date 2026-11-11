# Локальные проверки без GitHub Actions

GitHub Actions отключены (workflow-файлов нет), автоматических проверок при
push или PR не существует. Весь CI запускается на своей машине; платный
hosted runner не предполагается. Python-эпоха (`scripts.ci.run_checks`,
`pytest tests`) удалена — единственный исполняемый Studio теперь Rust
workspace `rust/Cargo.toml` (контракт: `AGENTS.md`).

## Быстрый прогон (перед каждым коммитом/PR)

Windows native, требуется Rust (stable, clippy, таргет
`wasm32-unknown-unknown`), Node.js 22+ и git-lfs (модели `models/`, `ov/` и
медиа `examples/` хранятся в LFS; после клона — `git lfs install && git lfs pull`):

```powershell
powershell -ExecutionPolicy Bypass -File scripts\check-all.ps1
```

Гейт выполняет:

1. сборку WASM-модуля UI (`cargo build -p volley-web --target
   wasm32-unknown-unknown --release --offline --locked` + копия в
   `rust/volley-app/static/volley_web.wasm`); wasm больше не коммитится,
   а `volley-app` встраивает его через `include_bytes!`
2. `cargo fmt --all -- --check`
3. `cargo clippy --workspace --all-targets --offline --locked -- -D warnings`
4. `cargo test --workspace --offline --locked` (пропустить: `-SkipRustTests`,
   тогда в журнале обязательно записать **RUST TESTS NOT RUN**)
5. все не-GPU `scripts/ci/*.mjs`: упаковка и CRLF-контракты, ownership и
   roadmap self-tests, WASM-мост, браузерные DOM/UI-контракты, включая
   статическую проверку «каждый id из JS существует в studio.html»
   (`test_studio_dom_ids.mjs`).

Эквивалент в WSL (bash):

```bash
cd rust
cargo build -p volley-web --target wasm32-unknown-unknown --release --offline --locked
cp target/wasm32-unknown-unknown/release/volley_web.wasm volley-app/static/volley_web.wasm
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline --locked -- -D warnings
cargo test --workspace --offline --locked
cd ..
for gate in scripts/ci/*.mjs; do node "$gate" || exit 1; done  # кроме stage84, см. ниже
```

## Отдельные проверки (не входят в check-all)

| Скрипт | Что нужно | Когда запускать |
| --- | --- | --- |
| `scripts/ci/test_rust_lifecycle.sh` (WSL) | свободный репозиторный lock: живой сервер на 8875 должен быть остановлен | изменения lifecycle/lock/shutdown |
| `scripts/ci/test_rust_faults.sh`, `test_rust_shutdown_race.sh` (WSL) | изолированный /tmp, тестовые порты | изменения обработки ошибок/остановки |
| `scripts/ci/test_rust_cuda_runtime.sh`, `smoke_rust_ort.sh` (WSL) | `libonnxruntime.so` + CUDA provider (`scripts/setup_rust_ort_cuda.sh`, `ORT_DYLIB_PATH`) | изменения GPU-инференса |
| `scripts/ci/test_rust_real_media_84.mjs`, `test_rust_parity_84.mjs`, `test_rust_benchmark_84.mjs`, `benchmark_rust_parity_84.mjs`, `audit_python_baseline_84.mjs` | реальные видео, ffmpeg, GPU, замороженный Python-чекпойнт | изменения распознавания/качества; приёмка этапа 84/85 |
| `scripts/ci/test_rust_docker_smoke.sh` | Docker | изменения Dockerfile/упаковки |

Известные средовые блокеры этой машины: в WSL отсутствует
`libonnxruntime.so` (GPU-тесты не готовы), длинные GPU-прогоны падают с
«CUDA failure 999» на RTX 5070 Ti. Пропуск GPU/медиа-проверок обязательно
фиксировать в журнале как **NOT TESTED**, а не как PASS.

## Матрица проверки

| Изменения | Обязательные дополнительные проверки |
| --- | --- |
| UI / статика | `node scripts/ci/test_studio_dom_ids.mjs`, WASM-сьюты, ручной F5 и переключение вкладок |
| HTTP API / хранилище | `cargo test -p volley-app api_http_tests` (in-process `/api/*` на временном корне) |
| Touch / траектория / ралли | Rust-тесты владельца + реальные сцены (stage84, отдельно) |
| Игроки / ReID | `cargo test -p volley-core reid`, `cargo test -p volley-app reid` |
| Quick Reel / монтаж | Rust-тесты + просмотр MP4; медиа-гейты отдельно |
| Lifecycle / запуск | `test_rust_lifecycle.sh` в WSL при остановленном живом сервере |
| Модель / GPU | ручной бенчмарк на разрешённом видео, подтверждение CUDA provider |

## Журнал и слияние

Каждый проверенный инкремент = коммит в migration-ветку + запись в
`docs/RUST_MIGRATION.md` с фактическими командами и результатами
PASS/FAIL/NOT TESTED. production-переключение запрещено до приёмки этапа 85
(`docs/RUST_ROADMAP.md`). Review и merge в `master`; автоматического merge
gate нет.