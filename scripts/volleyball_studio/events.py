from .common import key


def event_timeline(tracks, source, offset, width, height, court):
    events = []
    for track in tracks:
        track_id = key([str(source), track["start_frame"], track["last_frame"]])
        boundaries = [(track["start_frame"], "rally_start"), (track["last_frame"], "rally_end")]
        confidence = float(track.get("rally_classification", {}).get("rally_confidence", 0))
        for frame, kind in boundaries:
            events.append({"id": key([track_id, frame, kind]), "track_id": track_id, "source": str(source), "frame": frame, "frame_domain": "analysis_cfr_30", "time_sec": frame / 30 + offset, "type": kind, "ball": None, "player_track_id": None, "confidence": confidence, "status": "heuristic", "recommended_effect": "none"})
        for contact in track.get("player_interaction", {}).get("contacts", []):
            frame = int(contact["frame"])
            kind = contact["contact_type"]
            if not court and kind in ("serve", "block"):
                kind = "unknown_touch"
            confidence = float(contact["confidence"]) * (1. if court else .85)
            speed = float(contact.get("speed_after_px_frame", 0)) * 30
            strong = kind == "attack" and confidence >= .75 and speed / width >= .3
            events.append({"id": key([track_id, frame, kind]), "track_id": track_id, "source": str(source), "frame": frame, "frame_domain": "analysis_cfr_30", "time_sec": frame / 30 + offset, "type": kind, "ball": {"x": contact["ball_x"], "y": contact["ball_y"], "space": "analysis_pixels"}, "player_track_id": contact.get("player_track_id"), "speed_px_sec": speed, "direction_change_deg": contact.get("direction_change_deg"), "confidence": confidence, "status": "heuristic", "recommended_effect": "fire" if strong else "flash", "contact": {**contact, "contact_type": kind, "confidence": confidence}})
    return {"schema": "volleyball-events/2", "source": str(source), "fps": 30, "analysis_offset_sec": offset, "coordinate_space": {"width": width, "height": height}, "court_available": court, "events": sorted(events, key=lambda e: e["frame"])}


def contacts_for(timeline, track):
    track_id = key([timeline["source"], track["start_frame"], track["last_frame"]])
    return [dict(event["contact"], event_id=event["id"], recommended_effect=event["recommended_effect"]) for event in timeline["events"] if event["track_id"] == track_id and "contact" in event]
