#!/usr/bin/env python3
"""Compare CPU postprocessing on the *same* manually annotated match clip.

Evaluation is independent of GPU execution and never invents ground truth.
Synthetic/unit examples must NOT be presented as measured model accuracy.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from math import isfinite
from pathlib import Path
import sys

SCHEMA = "volley-quality-annotations/1"
EVIDENCE_SCHEMA = "volley-quality-evidence/1"


def _point(value: dict, key: str) -> float:
    number = float(value[key])
    if not isfinite(number) or number < 0:
        raise ValueError(f"Invalid nonnegative time: {value!r}")
    return number


def match_times(reference: list[float], candidate: list[float],
                tolerance: float) -> tuple[int, float]:
    """Maximize one-to-one matches, then minimize timing error."""
    a, b = sorted(reference), sorted(candidate)
    n, m = len(a), len(b)
    dp = [[(0, 0.0)] * (m + 1) for _ in range(n + 1)]
    for i in range(n - 1, -1, -1):
        for j in range(m - 1, -1, -1):
            options = [dp[i + 1][j], dp[i][j + 1]]
            delta = abs(a[i] - b[j])
            if delta <= tolerance:
                count, neg_error = dp[i + 1][j + 1]
                options.append((count + 1, neg_error - delta))
            dp[i][j] = max(options)
    count, neg_error = dp[0][0]
    return count, -neg_error


def identity_errors(observations: list[dict]) -> tuple[int | None, int | None]:
    """Count within-player ID switches and simultaneous cross-player collisions.

    person_id must come from a human-reviewed association, not ReID guesses.
    Missing observations are *not* counted as correct matches.
    """
    if not observations:
        return None, None
    per_person: dict[str, list[tuple[float, str]]] = {}
    simultaneous: dict[tuple[float, str], set[str]] = {}
    for sample in observations:
        person = str(sample["person_id"])
        track = str(sample["track_id"])
        timestamp = _point(sample, "time_sec")
        if not person or not track:
            raise ValueError("Identity sample is missing person_id or track_id")
        per_person.setdefault(person, []).append((timestamp, track))
        simultaneous.setdefault((timestamp, track), set()).add(person)
    switches = 0
    for samples in per_person.values():
        samples.sort()
        switches += sum(earlier_id != later_id and 0 < later_t - earlier_t <= 3.0
                        for (earlier_t, earlier_id), (later_t, later_id)
                        in zip(samples, samples[1:]))
    collisions = sum(len(people) - 1 for people in simultaneous.values())
    return switches, collisions


def evaluate(annotation: dict, prediction: dict, *, touch_window: float = .20,
             rally_window: float = 2.0) -> dict:
    if annotation.get("schema") != SCHEMA:
        raise ValueError("Unknown annotation schema")
    if annotation.get("clip_id") != prediction.get("clip_id"):
        raise ValueError("Comparison requires the same clip_id")
    gt = [_point(item, "time_sec") for item in annotation.get("touches", [])]
    predicted = [_point(item, "time_sec") for item in prediction.get("touches", [])]
    true_positive, _ = match_times(gt, predicted, touch_window)
    fp, fn = len(predicted) - true_positive, len(gt) - true_positive
    precision = true_positive / len(predicted) if predicted else (1.0 if not gt else 0.0)
    recall = true_positive / len(gt) if gt else (1.0 if not predicted else 0.0)
    f1 = 2 * precision * recall / (precision + recall) if precision + recall else 0.0

    # Rally matching requires both start and end within a tolerable range.
    gt_rallies = annotation.get("rallies", [])
    pr_rallies = prediction.get("rallies", [])
    matched = 0
    total_error = 0.0
    used = set()
    for gt_rally in sorted(gt_rallies, key=lambda r: r["start_sec"]):
        start, end = _point(gt_rally, "start_sec"), _point(gt_rally, "end_sec")
        if end < start:
            raise ValueError("Rally end precedes start")
        options = []
        for j, pr_rally in enumerate(pr_rallies):
            if j in used:
                continue
            p_start, p_end = _point(pr_rally, "start_sec"), _point(pr_rally, "end_sec")
            if p_end < p_start:
                raise ValueError("Predicted rally end precedes start")
            if abs(start - p_start) <= rally_window and abs(end - p_end) <= rally_window:
                options.append((abs(start - p_start) + abs(end - p_end), j))
        if options:
            error, index = min(options)
            used.add(index)
            matched += 1
            total_error += error
    switches, collisions = identity_errors(prediction.get("identity_samples", []))
    return {
        "touch_precision": round(precision, 6),
        "touch_recall": round(recall, 6),
        "touch_f1": round(f1, 6),
        "touch_false_positive": fp,
        "touch_missed": fn,
        "rally_boundary_mae_sec": (round(total_error / (2 * matched), 6)
                                    if matched else None),
        "rally_missed": len(gt_rallies) - matched,
        "rally_extra": len(pr_rallies) - matched,
        "identity_switches": switches,
        "identity_collisions": collisions,
    }


def evidence(annotations: Path, baseline: Path, candidate: Path,
             base_revision: int, new_revision: int) -> dict:
    src = annotations.read_bytes()
    expected = json.loads(src)
    if not expected.get("clip_id") or not expected.get("dataset_id"):
        raise ValueError("Annotations require clip_id and dataset_id")
    old = evaluate(expected, json.loads(baseline.read_text(encoding="utf-8")))
    new = evaluate(expected, json.loads(candidate.read_text(encoding="utf-8")))
    metrics = {k: {"before": old[k], "after": new[k]} for k in old}
    return {
        "schema": EVIDENCE_SCHEMA,
        "baseline_revision": base_revision,
        "candidate_revision": new_revision,
        "dataset_id": expected["dataset_id"],
        "clip_id": expected["clip_id"],
        "annotation_sha256": hashlib.sha256(src).hexdigest(),
        "baseline_sha256": hashlib.sha256(baseline.read_bytes()).hexdigest(),
        "candidate_sha256": hashlib.sha256(candidate.read_bytes()).hexdigest(),
        "metrics": metrics,
        "limitations": ("Identity metrics require manually assigned person_id samples; "
                        "an unmeasured metric is null, never automatically considered good."),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Evaluate matched annotation and CPU outputs")
    parser.add_argument("--annotations", type=Path, required=True)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--baseline-revision", type=int, required=True)
    parser.add_argument("--candidate-revision", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        report = evidence(args.annotations, args.baseline, args.candidate,
                          args.baseline_revision, args.candidate_revision)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n",
                               encoding="utf-8")
        print(f"Quality report saved: {args.output} ({report['dataset_id']})")
        return 0
    except (OSError, ValueError, KeyError, TypeError) as exc:
        print(f"Evaluation failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
