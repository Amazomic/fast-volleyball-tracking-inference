import html
import json
from pathlib import Path

from .common import cached, key, run, save


def review_filter(candidates, path):
    if path is None:
        return candidates
    review = json.loads(Path(path).read_text(encoding="utf-8"))
    include = set(review.get("include", []))
    exclude = set(review.get("exclude", []))
    known = {c["id"] for c in candidates}
    unknown = (include | exclude) - known
    if unknown:
        raise ValueError("Неизвестные ID эпизодов: " + ", ".join(sorted(unknown)))
    return [c for c in candidates if (not include or c["id"] in include) and c["id"] not in exclude]


def previews(candidates, folder):
    folder.mkdir(parents=True, exist_ok=True)
    records, cards = [], []
    for item in candidates:
        identifier = item["id"]
        thumbnail = folder / f"{identifier}.jpg"
        signature = key([identifier, item["cache"], item["score"], item["analysis_offset"]])
        def create():
            middle = (item["start"]+item["end"])/60 + item["analysis_offset"]
            run(["ffmpeg", "-v", "error", "-y", "-ss", str(middle), "-i", item["source"], "-frames:v", "1", "-vf", "scale=480:-2", thumbnail])
            return [thumbnail]
        cached(folder, identifier, signature, create)
        record = {"id": identifier, "source": item["source"], "start_sec": item["start"]/30+item["analysis_offset"], "end_sec": (item["end"]+1)/30+item["analysis_offset"], "score": item["score"], "components": item["components"], "confidence": item["confidence"], "thumbnail": thumbnail.name}
        records.append(record)
        cards.append(f'<article><img src="{thumbnail.name}"><p>{html.escape(Path(item["source"]).name)} · {record["start_sec"]:.2f}–{record["end_sec"]:.2f} s · score {item["score"]:.2f}</p><code>{identifier}</code><p>confidence {item["confidence"]:.2f}</p></article>')
    save(folder / "highlights.json", records)
    review_path = folder / "selection.json"
    if not review_path.exists():
        save(review_path, {"include": [], "exclude": []})
    (folder / "index.html").write_text('<!doctype html><html lang="ru"><meta charset="utf-8"><title>Волейбольные эпизоды</title><style>body{background:#111;color:#eee;font:16px sans-serif;padding:24px}main{display:flex;flex-wrap:wrap;gap:20px}article{width:480px;background:#222;padding:16px}img{width:100%}code{overflow-wrap:anywhere}</style><h1>Отбор эпизодов</h1><p>Укажите ID в include/exclude файла selection.json и передайте --selection-file. Пустой include означает все эпизоды.</p><main>' + ''.join(cards) + '</main></html>', encoding="utf-8")
    return folder / "index.html"
