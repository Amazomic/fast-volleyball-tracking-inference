import re
from .common import key


NUMBERS = {"один": 1, "одна": 1, "одно": 1, "два": 2, "две": 2, "три": 3, "четыре": 4, "пять": 5, "десять": 10, "пятнадцать": 15, "двадцать": 20, "тридцать": 30, "сорок": 40, "сорок пять": 45, "шестьдесят": 60, "полминуты": 30}


def parse_request(text):
    text = text.lower().replace("ё", "е")
    for word, value in sorted(NUMBERS.items(), key=lambda pair: -len(pair[0])):
        text = re.sub(r"\b" + word + r"\b", str(value), text)
    result = {}
    durations = list(re.finditer(r"(\d+(?:[.,]\d+)?)[\s-]*(секунд\w*|сек\b|с\b|минут\w*|мин\b)", text))
    duration = next((match for match in durations if not re.search(r"(?:не короче|минимум|от)\s*$", text[:match.start()])), None)
    if duration:
        result["duration"] = float(duration[1].replace(",", ".")) * (60 if duration[2].startswith("мин") else 1)
    count = re.search(r"(\d+)\s*(?:итогов\w*\s+)?(?:ролик\w*|видео|монтаж\w*)", text)
    if count:
        result["count"] = int(count[1])
    elif re.search(r"\b(?:ролик|монтаж|видео)\b", text):
        result["count"] = 1
    themes = []
    for pattern, event in ((r"атак\w*|нападающ\w*\s+удар\w*", "attack"), (r"блок\w*", "block"), (r"подач\w*", "serve")):
        if re.search(pattern, text):
            if re.search(r"(?:без|кроме|исключи\w*)\s+(?:" + pattern + ")", text):
                raise ValueError("Исключение типа события требует уточнения; используйте положительный список событий")
            themes.append(event)
    if themes:
        result["events"] = themes
    if re.search(r"длинн\w*\s+розыгрыш\w*|долг\w*\s+розыгрыш\w*", text):
        result["long_rallies"] = True
    minimum = re.search(r"(?:не короче|минимум|от)\s+(\d+(?:[.,]\d+)?)\s*(?:секунд|сек|с)\w*", text)
    if minimum:
        result["min_rally"] = float(minimum[1].replace(",", "."))
    if re.search(r"кинематограф\w*|cinematic", text):
        result["style"] = "cinematic"
    elif re.search(r"классическ\w*|classic", text):
        result["style"] = "classic"
    elif re.search(r"динамич\w*|dynamic", text):
        result["style"] = "dynamic"
    if re.search(r"без эффект\w*", text):
        result["effects"] = "off"
    elif re.search(r"огнен\w*|\bогнем\b|огонь", text):
        result["effects"] = "cinematic"
    if re.search(r"без музык\w*", text):
        result["music"] = "no"
    elif re.search(r"(?:с|добавь|добавить)\s+музык\w*", text):
        result["music"] = "yes"
    if re.search(r"без звук\w*|без аудио", text):
        result.update(match_volume=0., music_volume=0., accent_volume=0.)
    elif re.search(r"только музык\w*|без (?:исходного|оригинального|естественного) звук\w*", text):
        result["match_volume"] = 0.
    if re.search(r"только (?:исходн\w*|оригинальн\w*|естественн\w*)\s+звук\w*", text):
        result.update(music="no", accent_volume=0.)
    if re.search(r"без звуковых (?:акцент\w*|эффект\w*)", text):
        result["accent_volume"] = 0.
    return result


def themed_candidates(candidates, events, threshold, long_rallies=False):
    selected = []
    for candidate in candidates:
        if long_rallies and candidate["duration"] < 10:
            continue
        contacts = [c for c in candidate["contacts"] if c["confidence"] >= threshold and (not events or c["contact_type"] in events) and (c["contact_type"] not in ("block", "serve") or candidate.get("court_available"))]
        if events and not contacts:
            continue
        if not events:
            selected.append(candidate)
            continue
        for contact in contacts:
            item = dict(candidate)
            item["id"] = key([candidate["id"], contact["frame"], contact["contact_type"]])
            item["start"] = max(candidate["start"], contact["frame"]-30)
            item["end"] = min(candidate["end"], contact["frame"]+45)
            item["duration"] = (item["end"]-item["start"]+1)/30
            item["safe_context_start"], item["safe_context_end"] = item["start"], item["end"]
            item["selection_contacts"] = [contact]
            item["theme"] = list(events)
            selected.append(item)
    return selected
