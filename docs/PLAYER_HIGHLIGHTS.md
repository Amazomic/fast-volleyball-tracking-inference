# Reels and player action statistics

Source videos: `project/*.mp4`.

All generated files: `results/`. Existing cached analysis remains in `results/analysis/<cache-id>/`.

## Build ~2 minutes of complete, highest-ranked rallies

From the repository root (PowerShell):

```powershell
python scripts/build_match_highlights.py
```

This creates `results/highlights_120s.mp4` (approximately 2 minutes; no rally trimming,
vertical 720x1280, original audio),
`player_statistics.csv`, `player_statistics.json`, `match_report.md` and
`highlights_manifest.json`.

It reuses `analysis.mp4`, `ranking.json`, `players.json`, and `analysis/events.json`;
it never reruns neural-network detection.

Rallies split by brief ball-track dropouts (45 frames or less) are joined.
The 2-second lead-in and 1.5-second tail are limited by neighboring detections.
The selection prioritizes the strongest match ranking scores and never shortens a
rally to hit exactly 120 seconds. The final duration can differ slightly.

**Important:** The supplied match lacks calibrated court geometry; the beginning
of a serve and the award of a point cannot be asserted precisely. Track boundaries
plus context are an approximation, not refereed point boundaries.

## Create new highlight videos from the existing event index

```powershell
python scripts/create_highlights_by_event.py --event attack --duration 30
python scripts/create_highlights_by_event.py --event dig --duration 45
python scripts/create_highlights_by_event.py --event attack --player-id 725 --duration 20
python scripts/create_highlights_by_event.py --event any --duration 60 --min-confidence 0.6
```

Supported events: attack, block, serve, dig, overhead_pass, low_touch, any.
For a quick selection check without rendering, append `--list-only`.
Event-based selections now include full detected rallies containing the chosen
action (not short windows around individual impacts). Output manifests list event
IDs and original frames, so selection is reproducible.

## Interpretation / limitations

Player IDs are tracker-generated IDs, not verified unique persons. The model frequently
loses and reacquires tracks, yielding a new ID for the same person. A zero number of
blocks/serves is not evidence that none occurred. Classifications are heuristic and
the provided match has no court calibration. For reliable per-athlete whole-match
statistics, identity reconciliation and validation of player contacts are required.

The cached analysis is source-specific. An unrelated video needs its own analysis
before these scripts can generate accurate player stats and highlights.
