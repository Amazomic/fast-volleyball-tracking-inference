"""Persistent, global user-defined analysis profiles shared by all projects."""
from __future__ import annotations

import math
import re
import threading
import uuid

from . import data
from scripts.volleyball_studio.advanced import SPECS, validate_one, validated

_LOCK = threading.RLock()
_EXTRA = {
    "ball_threshold": (float, 0.05, 0.95),
    "threads": (int, 1, 128),
    "analysis_width": (int, 320, 1920),
    "gpu_device_id": (int, 0, 15),
    "gpu_memory_limit_mb": (int, 0, 131072),
}
_BOOLS = {"beach", "manual_width_enabled"}
_CHOICE = {
    "backend": {"onnx", "onnx-cpu", "openvino"},
    "ball_model": {
        "VballNetV4c_seq9_grayscale_20260908_213829.onnx",
        "VballNetFastV1_seq9_grayscale_233_h288_w512.onnx",
    },
}
_ALLOWED = set(SPECS) | set(_EXTRA) | _BOOLS | set(_CHOICE)


def profile_schema() -> dict:
    # Browser constraints are derived from these same server-side validators.
    types = {bool: "bool", int: "int", float: "float", str: "choice"}
    result = {
        name: {
            "type": types[spec[0]], "min": spec[2], "max": spec[3],
            "choices": list(spec[4]) if spec[4] is not None else None,
        }
        for name, spec in SPECS.items()
    }
    for name, (kind, minimum, maximum) in _EXTRA.items():
        result[name] = {"type": types[kind], "min": minimum, "max": maximum,
                        "choices": None}
    for name in _BOOLS:
        result[name] = {"type": "bool", "min": None, "max": None, "choices": None}
    for name, choices in _CHOICE.items():
        result[name] = {"type": "choice", "min": None, "max": None,
                        "choices": sorted(choices)}
    result["analysis_width"]["multiple_of"] = 2
    result["reels_smooth_window"]["odd"] = True
    result["reels_smooth_polyorder"]["less_than"] = "reels_smooth_window"
    return result



# Built-in presets are global too; their definitions are bundled with the UI.
BUILTIN_PROFILES = {"accuracy": "Точность", "speed": "Скорость", "new": "Новый"}


def resolve_profile_reference(profile_id, profile_name):
    """Validate a global profile reference without storing profiles per project.

    The saved run keeps its own immutable settings snapshot so deleting a
    global preset never removes or changes the completed analysis.
    """
    name = str(profile_name or "Рекомендуемый").strip()
    if not 1 <= len(name) <= 100:
        raise ValueError("Некорректное название профиля анализа")
    if profile_id is None:
        return "legacy", name  # Older clients can still launch analyses.
    if profile_id == "custom":
        return "custom", name
    if profile_id in BUILTIN_PROFILES:
        if name != BUILTIN_PROFILES[profile_id]:
            raise ValueError("Название профиля не совпадает с глобальным профилем")
        return profile_id, name
    if isinstance(profile_id, str) and re.fullmatch(r"user_[a-f0-9]{14}", profile_id):
        selected = next((item for item in all_profiles() if item["id"] == profile_id), None)
        if selected is None or selected.get("name") != name:
            raise ValueError("Глобальный профиль изменён или удалён. Обновите список.")
        return profile_id, name
    raise ValueError("Неизвестный идентификатор профиля анализа")


def _file():
    return data.RESULTS / "analysis_profiles.json"


def all_profiles() -> list[dict]:
    with _LOCK:
        contents = data.read_json(_file(), {"schema": 1, "profiles": []})
        if not isinstance(contents, dict) or not isinstance(contents.get("profiles"), list):
            return []
        return [entry for entry in contents["profiles"] if isinstance(entry, dict)
                and isinstance(entry.get("id"), str)
                and isinstance(entry.get("values"), dict)]


def _clean_values(values: dict) -> dict:
    if not isinstance(values, dict) or not values:
        raise ValueError("Выберите параметры профиля")
    if len(values) > 80:
        raise ValueError("Слишком много параметров профиля (максимум 80)")
    unknown = set(values) - _ALLOWED
    if unknown:
        raise ValueError("Неизвестный параметр профиля: " + ", ".join(sorted(unknown)))
    cleaned = {}
    for name, raw in values.items():
        if name in SPECS:
            try:
                cleaned[name] = validate_one(name, raw)
            except (ValueError, TypeError, OverflowError) as exc:
                message = str(exc)
                raise ValueError(message if message.startswith(name + ":")
                                 else name + ": некорректное значение") from exc
        elif name in _BOOLS:
            if not isinstance(raw, bool):
                raise ValueError(name + ": ожидается да/нет")
            cleaned[name] = raw
        elif name in _CHOICE:
            if not isinstance(raw, str) or raw not in _CHOICE[name]:
                raise ValueError(name + ": допустимые значения: " +
                                 ", ".join(sorted(_CHOICE[name])))
            cleaned[name] = raw
        elif name in _EXTRA:
            kind, lower, upper = _EXTRA[name]
            try:
                if isinstance(raw, bool):
                    raise ValueError()
                number = float(raw)
            except (TypeError, ValueError, OverflowError) as exc:
                raise ValueError(name + ": ожидается число") from exc
            if not math.isfinite(number) or not lower <= number <= upper:
                raise ValueError(f"{name}: допустимый диапазон {lower}–{upper}")
            if kind is int and not number.is_integer():
                raise ValueError(name + ": ожидается целое число")
            if name == "analysis_width" and int(number) % 2:
                raise ValueError(name + ": ширина должна быть чётной")
            cleaned[name] = int(number) if kind is int else number
    try:
        validated({name: value for name, value in cleaned.items() if name in SPECS})
    except (ValueError, TypeError, OverflowError) as exc:
        raise ValueError(str(exc)) from exc
    return cleaned


def save_profile(name: str, values: dict) -> dict:
    if not isinstance(name, str) or not 1 <= len(name.strip()) <= 80:
        raise ValueError("Имя профиля должно содержать от 1 до 80 символов")
    name = name.strip()
    cleaned = _clean_values(values)
    with _LOCK:
        profiles = all_profiles()
        if len(profiles) >= 100:
            raise ValueError("Достигнут лимит 100 профилей")
        if any(p["name"].casefold() == name.casefold() for p in profiles):
            raise ValueError("Профиль с таким названием уже существует")
        result = {"id": "user_" + uuid.uuid4().hex[:14], "name": name, "values": cleaned,
                  "description": "Сохранённые настройки пользователя", "created_by": "user"}
        profiles.append(result)
        data.save_json(_file(), {"schema": 1, "profiles": profiles})
        return result


def remove_profile(profile_id: str):
    if not isinstance(profile_id, str) or not re.fullmatch(r"user_[a-f0-9]{14}", profile_id):
        raise ValueError("Можно удалить только пользовательский профиль")
    with _LOCK:
        profiles = all_profiles()
        remaining = [p for p in profiles if p["id"] != profile_id]
        if len(remaining) == len(profiles):
            raise ValueError("Профиль не найден")
        data.save_json(_file(), {"schema": 1, "profiles": remaining})
        return {"deleted": profile_id}
