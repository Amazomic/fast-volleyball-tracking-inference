import math

import cv2
import numpy as np


def valid_ball(item, frame, threshold):
    point = item["ball"].get(frame)
    if not point or point["confidence"] < threshold:
        return None
    csv_point = item["ball_rows"].get(frame)
    if csv_point and math.hypot(point["x"] - csv_point[0], point["y"] - csv_point[1]) > item["analysis_width"] * .025:
        return None
    previous = item["ball"].get(frame - 1)
    if previous and math.hypot(point["x"] - previous["x"], point["y"] - previous["y"]) > item["analysis_width"] * .12:
        return None
    return point


def strong_contacts(item, threshold):
    return [c for c in item["contacts"] if c["contact_type"] == "attack" and c["confidence"] >= max(.75, threshold) and c.get("speed_after_px_frame", 0) * 30 / item["analysis_width"] >= .3 and c.get("recommended_effect", "fire") == "fire"]


def body_overlap(item, frame):
    ball = item["ball"].get(frame)
    if not ball:
        return False
    return any(p.get("score", 0) >= .7 and p["bbox_xyxy"][0] <= ball["x"] <= p["bbox_xyxy"][2] and p["bbox_xyxy"][1] <= ball["y"] <= p["bbox_xyxy"][3] for p in item.get("players", {}).get(frame, []))


def effect_state(item, frame, intensity, threshold):
    empty = {"trail": False, "fire": False, "flash": False, "slow": False, "shake": False, "strength": 0.}
    if intensity == "off" or not valid_ball(item, frame, threshold):
        return empty
    attacks = strong_contacts(item, threshold)
    ages = [frame-c["frame"] for c in attacks if 0 <= frame-c["frame"] <= 30]
    attack = bool(ages)
    hit = any(c["confidence"] >= threshold and 0 <= frame-c["frame"] <= 7 for c in item["contacts"])
    current, previous = item["ball"][frame], valid_ball(item, frame-1, threshold)
    fast = bool(previous and math.hypot(current["x"]-previous["x"], current["y"]-previous["y"]) * 30 / item["analysis_width"] > .3)
    overlap = body_overlap(item, frame)
    return {"trail": (fast or attack) and not overlap, "fire": attack and intensity == "cinematic" and not overlap, "flash": hit, "slow": attack and intensity == "cinematic" and not overlap, "shake": any(0 <= frame-c["frame"] <= 8 for c in attacks) and intensity == "cinematic", "strength": max((1-age/31 for age in ages), default=0.), "occlusion_guard": overlap}


def playback_speed(item, frame, args, state):
    if not getattr(args, "speed_ramp", False):
        return .5 if state["slow"] else 1.
    if args.effects == "off" or not valid_ball(item, frame, args.min_confidence) or body_overlap(item, frame):
        return 1.
    attacks = strong_contacts(item, args.min_confidence)
    if any(-8 <= frame-c["frame"] < 0 for c in attacks):
        return .5
    if any(9 <= frame-c["frame"] <= 18 for c in attacks):
        return 1.5
    return 1.


def apply_effects(image, point, history, state, intensity, frame):
    if not point:
        history.clear()
        return image
    history.append((frame, point))
    layer = np.zeros_like(image)
    scale = image.shape[1] / 1080
    if state["trail"] and len(history) > 1:
        for i, ((fa, a), (fb, b)) in enumerate(zip(history, list(history)[1:])):
            if fb-fa == 1:
                cv2.line(layer, a, b, (255, 220, 100), max(1, int(14*scale*(i+1)/len(history))), cv2.LINE_AA)
    strength = state.get("strength", 1.)
    if state["fire"]:
        for radius, color in ((42, (0, 65, 155)), (26, (0, 140, 255)), (11, (170, 250, 255))):
            cv2.circle(layer, point, max(1, int(radius*scale)), color, -1, cv2.LINE_AA)
        for i in range(8):
            dx = int(math.sin(frame*.7+i*2.1)*(12+i*2)*scale)
            dy = int((-15-(frame*7+i*13)%45)*scale)
            cv2.circle(layer, (point[0]+dx, point[1]+dy), max(1, int((2+i%3)*scale)), (0, 110, 240), -1)
    if state["flash"]:
        cv2.circle(layer, point, max(3, int(58*scale)), (200, 240, 255), max(1, int(5*scale)), cv2.LINE_AA)
    glow = cv2.GaussianBlur(layer, (0, 0), max(.6, 7*scale))
    return cv2.addWeighted(cv2.addWeighted(image, 1., glow, 1.4*max(.4, strength), 0), 1., layer, (.95 if intensity == "cinematic" else .55)*max(.4, strength), 0)


def finish_frame(image, args, state, index, total, label):
    if getattr(args, "grade", False):
        image = cv2.convertScaleAbs(image, alpha=1.035 if args.effects == "subtle" else 1.07, beta=-3)
    if state.get("shake"):
        amount = max(1, round(image.shape[1]*.003))
        dx = round(math.sin(index*2.3)*amount)
        dy = round(math.cos(index*2.7)*amount)
        image = cv2.warpAffine(image, np.float32([[1, 0, dx], [0, 1, dy]]), (image.shape[1], image.shape[0]), borderMode=cv2.BORDER_REFLECT)
    if getattr(args, "titles", False) and index < 45:
        scale = image.shape[1]/1080
        cv2.putText(image, label, (round(image.shape[1]*.07), round(image.shape[0]*.12)), cv2.FONT_HERSHEY_SIMPLEX, max(.3, 1.1*scale), (245, 245, 245), max(1, round(2*scale)), cv2.LINE_AA)
    return image
