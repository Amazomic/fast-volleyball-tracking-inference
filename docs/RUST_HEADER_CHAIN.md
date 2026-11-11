# Связанные версии Rust Studio

В шапке постоянно расположена цепочка **Проект → Анализ GPU → Постобработка CPU**,
аналогичная `header-dependency-chain` из замороженной Python Studio.

- **Проект:** выбирает один из зарегистрированных Rust-проектов. При смене
  сбрасываются только нижестоящие версии, а не страница.
- **Анализ:** конкретное видео и сохранённый GPU fusion (RAVEL + VballNet).
  Незавершённый, ещё не объединённый GPU-запуск можно выбрать, но
  постобработок он не наследует. Объединённые GPU-запуски не показываются
  вторично как «не объединённые».
- **Постобработка:** только CPU-розыгрыши, у которых
  `rally.source_replay_id → replay.source_fusion_id → fusion.id`
  совпадает с выбранным анализом и видео. Нельзя показывать CPU-версию
  чужого анализа или соседнего видео. Если метаданные происхождения
  отсутствуют, запись явно помечается «GPU-источник не установлен»,
  без выдуманной связи.
- При единственном доступном варианте допустим автовыбор; при нескольких
  пользователь выбирает вручную. Обновление цепочки сохраняет
  существующий выбор, пока его источник остаётся доступным.
- Обновление делается через `fetch` и DOM, без полной перезагрузки.
  Устаревшие ответы игнорируются. Вкладки используют тот же `studio-project`
  и тот же `studio-run`, а выбор видео анализа синхронизируется.
- Самостоятельный Quick Reel не привязан к выбранной цепочке.

Файлы: `rust/volley-app/static/studio.html`,
`rust/volley-app/static/studio_chain.js`,
`rust/volley-app/static/studio.js`,
`rust/volley-app/static/studio_tabs.js`,
`rust/volley-app/static/studio.css`,
`rust/volley-app/src/studio_analysis.rs` и
`rust/volley-app/src/studio_web.rs`.

Проверки: `node scripts/ci/test_studio_header_chain.mjs`,
`node scripts/ci/test_studio_wasm_ui.mjs`, а также
`cargo test -p volley-app --offline axum_serves_standalone_rust_ui_assets_over_http`.
При активных задачах GPU/Quick Reel сервер не перезапускать: менять
бинарник и перезапускать лишь когда `shutdown_safe=true`.
