from .vfx import effect_state, playback_speed


def output_seconds(item, args):
    phase, previous, frames = 0., None, 0
    for frame in range(item["start"], item["end"]+1):
        state = effect_state(item, frame, args.effects, args.min_confidence)
        speed = playback_speed(item, frame, args, state)
        if speed != previous:
            phase = 0.
            previous = speed
        phase += 1/speed
        count = int(phase+1e-9)
        frames += count
        phase -= count
    return frames/30


def protect_timing(groups, duration, args):
    result = []
    for group in groups:
        planned, total = [], 0.
        for candidate in group:
            item = dict(candidate)
            contacts = item.get("selection_contacts", item["contacts"])
            protected_end = max((min(item["end"], c["frame"]+15) for c in contacts if item["start"] <= c["frame"] <= item["end"]), default=item["start"]+29)
            seconds = output_seconds(item, args)
            while total+seconds > duration+1e-6 and item["end"] > protected_end:
                item["end"] -= 1
                seconds = output_seconds(item, args)
            if total+seconds > duration+1e-6:
                continue
            item["duration"] = (item["end"]-item["start"]+1)/30
            item["planned_output_duration"] = seconds
            planned.append(item)
            total += seconds
        if planned:
            result.append(planned)
    return result
