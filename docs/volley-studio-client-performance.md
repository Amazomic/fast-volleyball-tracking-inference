# Volley Studio — client-side performance audit

Measured 2026-11-02 on HOME-PC in headless Chrome. Synthetic UI workloads do not measure inference FPS or video decoding.

| Bottleneck | Change | Acceptance target |
| --- | --- | --- |
| Repeated stage DOM renders every 2.5 s | Signature-based rebuild only when stage state changes | No more than 1 DOM rebuild per new state; 100 identical updates under 5 ms |
| Entire Track ID table recreated on checkbox toggles | Patch existing checkboxes and row classes; debounce photo requests 120 ms | 100 selections under 20 ms, 0 child-list mutations, 1 photo-request batch after a burst |
| Rally count O(rallies × actions) and video overlay O(actions log actions) per frame | Sorted event index, binary search of time ranges, nearest-event lookup | 200 rallies and 5,000 actions render in under 50 ms; actual playback overlay p95 under 2 ms/frame |
| Large timeline and history DOM trees | Max 500 visual event markers; content-visibility for off-screen result/history/player cards | At most 500 timeline event nodes regardless of action count |
| Two permanent polling intervals | One adaptive, single-flight status timer, preparation timer stops when all videos ready | Idle status <=6 requests/min; background <=2/min; video preparation = 0 requests/min when ready |
| Chart rebuild on unchanged benchmark results | Skip render for unchanged run signature; skip polling chart when benchmark tab inactive | 0 chart DOM rebuilds when persisted data is unchanged |

## Browser measurements

- Idle 13.2 seconds: 2 status requests (scheduler alignment), 0 preparation requests.
- 100 Track ID checkbox changes, 30 visible rows: 8.2 ms, 0 child mutations, original rows preserved.
- 100 repeated live-stage updates: 1.7 ms, 1 initial stage-list rebuild.
- 200 rallies, 5,000 events: 5 render durations 10.1, 7.4, 7.1, 6.9, 6.3 ms; 500 visual timeline markers. All underlying events remain available.
- 300 synthetic timeupdate callbacks: 1.9 ms total. Not a substitute for real video overlay traces.

## Future verification budgets (not yet measured)

- Retained browser heap growth below 5 MB after 10 minutes of repeated tab navigation with constant project data; exclude media decoder memory.
- Actual playback overlay work below 2 ms per frame at p95 with a long match; verify in Chrome Performance.

Source files: scripts/volleyball_ui/static/app.js, scripts/volleyball_ui/static/results-ui.js, scripts/volleyball_ui/static/style.css, scripts/volleyball_ui/static/analysis-presets.js.
Client regression checks: tests/test_ui_client_performance.py.
