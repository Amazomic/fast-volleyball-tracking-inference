#!/usr/bin/env python3
"""Read-only audit of native Touch provenance; does not mutate snapshots.

Usage: python3 scripts/ci/audit_native_touch_refs.py TRACKED.jsonl TOUCH.jsonl
Reports violations as JSON and exits nonzero for an invalid source.
"""
import argparse
import hashlib
import json
from pathlib import Path

def audit(track_path: Path, touch_path: Path) -> dict:
    summary = {
        "tracked_frames": 0, "touches": 0, "matched_track": 0,
        "absent_track": 0, "no_ball": 0, "unknown_player": 0,
        "out_of_order": 0, "samples": [], "track_sha256": "", "touch_sha256": "",
    }
    frame_hash = hashlib.sha256()
    touch_hash = hashlib.sha256()
    contact_index = 0
    prev_contact = -1
    with track_path.open("rb") as tracks, touch_path.open("rb") as touches:
        raw_touch = touches.readline()
        if raw_touch:
            touch_hash.update(raw_touch)
        contact = json.loads(raw_touch) if raw_touch else None
        for frame_index, raw in enumerate(tracks):
            frame_hash.update(raw)
            frame = json.loads(raw)
            if frame["frame_index"] != frame_index:
                raise ValueError(f"tracked frame index mismatch at {frame_index}")
            while contact is not None and contact["frame_index"] <= frame_index:
                summary["touches"] += 1
                f = contact["frame_index"]
                track_id = contact.get("provisional_track_id")
                if f <= prev_contact or f != frame_index:
                    summary["out_of_order"] += 1
                if frame.get("selected_ball") is None:
                    summary["no_ball"] += 1
                if track_id is None:
                    summary["unknown_player"] += 1
                elif any(player["track_id"] == track_id for player in frame.get("tracked_players", [])):
                    summary["matched_track"] += 1
                else:
                    summary["absent_track"] += 1
                    if len(summary["samples"]) < 8:
                        summary["samples"].append({"frame": f, "id": track_id, "frame_players": len(frame.get("tracked_players", []))})
                prev_contact = f
                contact_index += 1
                raw_touch = touches.readline()
                if raw_touch:
                    touch_hash.update(raw_touch)
                contact = json.loads(raw_touch) if raw_touch else None
            summary["tracked_frames"] = frame_index + 1
        if contact is not None:
            raise ValueError("Touch references frame beyond tracked source")
    summary["track_sha256"] = frame_hash.hexdigest()
    summary["touch_sha256"] = touch_hash.hexdigest()
    return summary

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("tracked",type=Path)
    parser.add_argument("touches",type=Path)
    args = parser.parse_args()
    result = audit(args.tracked, args.touches)
    print(json.dumps(result, ensure_ascii=False, indent=2))
    raise SystemExit(1 if any(result[k] for k in ("absent_track", "no_ball", "out_of_order")) else 0)
