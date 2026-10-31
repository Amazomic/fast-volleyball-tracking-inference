#!/usr/bin/env python3
"""Render full-rally clips selected by detected action, without re-running any models.

Examples (from repository root):
    python scripts/create_highlights_by_event.py --event attack --duration 30
    python scripts/create_highlights_by_event.py --event dig --player-id 725 --duration 30

Every selected scene includes the entire detected rally and available context.
Without calibrated court, serve and point boundaries are estimates.
"""
from __future__ import annotations

import argparse
import re

from build_match_highlights import RESULTS, SOURCE_DIR, find_cache, render, save_json
from full_rallies import choose_whole_groups, load_groups

EVENT_TYPES = ("attack", "block", "serve", "dig", "overhead_pass", "low_touch")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--event", choices=("any", *EVENT_TYPES), default="attack")
    parser.add_argument("--player-id", default=None, help="Tracker ID, not a verified person identity")
    parser.add_argument("--duration", type=float, default=30, help="Approximate target; full rallies are never trimmed")
    parser.add_argument("--min-confidence", type=float, default=0.4)
    parser.add_argument("--list-only", action="store_true", help="Create selection manifest only")
    args = parser.parse_args()
    if args.duration <= 0 or not 0 <= args.min_confidence <= 1:
        parser.error("Duration must be positive and confidence within [0,1]")
    videos = sorted(SOURCE_DIR.glob("*.mp4"), key=lambda p: p.stat().st_mtime, reverse=True)
    if not videos:
        parser.error(f"No source MP4 in {SOURCE_DIR}")
    source = videos[0]
    cache, payload = find_cache(source)
    groups = load_groups(cache)
    detections = [
        e for e in payload.get("events", [])
        if e.get("type") in EVENT_TYPES
        and (args.event == "any" or e.get("type") == args.event)
        and (args.player_id is None or str(e.get("player_track_id")) == args.player_id)
        and float(e.get("confidence") or 0) >= args.min_confidence
    ]
    for group in groups:
        contained = [
            e for e in detections
            if group["detected_start_frame"] <= int(e["frame"]) <= group["detected_end_frame"]
        ]
        if not contained:
            continue
        group["ranked"] = True
        group["ranking_score"] = round(
            max(float(e.get("confidence") or 0) for e in contained) * 8
            + min(12, sum(float(e.get("confidence") or 0) for e in contained) * 2), 4
        )
        group["matched_event_ids"] = [e.get("id") for e in contained]
        group["matched_event_types"] = sorted(set(e.get("type") for e in contained))
        group["confidence"] = round(max(float(e.get("confidence") or 0) for e in contained), 4)
    try:
        selected = choose_whole_groups(groups, target_sec=args.duration, max_over_sec=6)
    except RuntimeError as exc:
        parser.error(str(exc))
    suffix = args.event + (("_player_" + re.sub(r"[^a-zA-Z0-9_-]", "", args.player_id)) if args.player_id else "")
    target = RESULTS / f"highlights_{suffix}.mp4"
    save_json(RESULTS / f"highlights_{suffix}_manifest.json", {
        "source": str(source), "cached_analysis": str(cache),
        "filter": vars(args), "clips": selected,
        "total_seconds": round(sum(g["duration_sec"] for g in selected), 2),
        "selection": "full detected rally with context; no fragment trimming",
        "caution": "Serve/point boundaries are estimated; classification is heuristic, ID may change for same player.",
    })
    print(f"Selected {len(selected)} complete candidate rallies, "
          f"{sum(g['duration_sec'] for g in selected):.1f}s; output: {target}", flush=True)
    if not args.list_only:
        render(source, cache / "analysis.mp4", selected, output_path=target,
               segments_folder=RESULTS / f"segments_{suffix}")


if __name__ == "__main__":
    main()
