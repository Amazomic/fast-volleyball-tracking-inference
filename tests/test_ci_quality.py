"""Zero-dependency tests for the PR policy and objective quality report."""
from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts.ci import quality_eval, repo_policy


class EvaluationTests(unittest.TestCase):
    def setUp(self):
        self.gt = {"schema": quality_eval.SCHEMA, "clip_id": "clip-1",
                   "dataset_id": "reviewed-match-1",
                   "touches": [{"time_sec": 1.0}, {"time_sec": 3.0}],
                   "rallies": [{"start_sec": .5, "end_sec": 5.0}]}

    def test_touch_precision_and_recall_are_one_to_one(self):
        detected = {"clip_id": "clip-1",
                    "touches": [{"time_sec": 1.05}, {"time_sec": 1.1},
                                {"time_sec": 3.01}]}
        metrics = quality_eval.evaluate(self.gt, detected)
        self.assertEqual(metrics["touch_false_positive"], 1)
        self.assertEqual(metrics["touch_missed"], 0)
        self.assertAlmostEqual(metrics["touch_precision"], 2 / 3, places=5)
        self.assertAlmostEqual(metrics["touch_recall"], 1.0, places=5)

    def test_straight_miss_does_not_look_like_a_perfect_score(self):
        metrics = quality_eval.evaluate(self.gt, {"clip_id": "clip-1", "touches": []})
        self.assertEqual(metrics["touch_f1"], 0.0)
        self.assertEqual(metrics["rally_missed"], 1)
        self.assertIsNone(metrics["rally_boundary_mae_sec"])

    def test_rally_boundary_error_and_missing_count(self):
        pred = {"clip_id": "clip-1", "touches": [],
                "rallies": [{"start_sec": .7, "end_sec": 4.6},
                            {"start_sec": 20, "end_sec": 30}]}
        metrics = quality_eval.evaluate(self.gt, pred)
        self.assertAlmostEqual(metrics["rally_boundary_mae_sec"], .3)
        self.assertEqual(metrics["rally_extra"], 1)

    def test_rally_matching_maximizes_count_not_greedy_nearest(self):
        gold = [{"start_sec": 1.0, "end_sec": 5.0},
                {"start_sec": 1.4, "end_sec": 5.4}]
        proposed = [{"start_sec": .5, "end_sec": 4.7},
                    {"start_sec": 1.1, "end_sec": 5.2}]
        matched, _ = quality_eval.match_rallies(gold, proposed, .6)
        self.assertEqual(matched, 2)

    def test_three_second_player_switch_and_simultaneous_merge(self):
        samples = [
            {"time_sec": 0, "person_id": "A", "track_id": "1"},
            {"time_sec": 1, "person_id": "A", "track_id": "2"},
            {"time_sec": 1, "person_id": "B", "track_id": "2"},
            {"time_sec": 5, "person_id": "A", "track_id": "3"},
        ]
        self.assertEqual(quality_eval.identity_errors(samples), (1, 1))
        self.assertEqual(quality_eval.identity_errors([]), (None, None))

    def test_reports_record_provenance_not_fabricated_benchmarks(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            anno = root / "annotated.json"
            old = root / "baseline.json"
            new = root / "candidate.json"
            anno.write_text(json.dumps(self.gt))
            old.write_text(json.dumps({"clip_id": "clip-1", "touches": []}))
            new.write_text(json.dumps({"clip_id": "clip-1", "touches": [
                {"time_sec": 1}, {"time_sec": 3}]}))
            report = quality_eval.evidence(anno, old, new, 3, 4)
            self.assertEqual(report["schema"], "volley-quality-evidence/1")
            self.assertEqual(report["metrics"]["touch_f1"],
                             {"before": 0.0, "after": 1.0})
            self.assertEqual(len(report["annotation_sha256"]), 64)
            self.assertIsNone(report["metrics"]["identity_switches"]["after"])


class RepositoryPolicyTests(unittest.TestCase):
    def test_current_default_config_and_tree(self):
        repo_policy.check_repository()

    def test_runtime_outputs_cannot_be_added(self):
        with self.assertRaisesRegex(repo_policy.PolicyError, "Runtime media"):
            repo_policy.check_changes({"results/private-match.json"}, "some-sha")

    def test_golden_cannot_change_without_evidence(self):
        with patch.object(repo_policy, "git",
                          return_value=json.dumps({"revision": 3})):
            with self.assertRaisesRegex(repo_policy.PolicyError, "revision"):
                repo_policy.check_changes({repo_policy.GOLD}, "base-sha")

    def test_golden_evidence_rejects_regression_and_requires_improvement(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            gold = root / repo_policy.GOLD
            gold.parent.mkdir(parents=True, exist_ok=True)
            gold.write_text(json.dumps({"revision": 4}))
            report_path = "docs/quality/reports/clip-evidence.json"
            report_file = root / report_path
            report_file.parent.mkdir(parents=True, exist_ok=True)
            report = {
                "schema": "volley-quality-evidence/1",
                "baseline_revision": 3, "candidate_revision": 4,
                "dataset_id": "real-annotated-clip", "annotation_sha256": "a" * 64,
                "metrics": {
                    "touch_f1": {"before": .7, "after": .8},
                    "rally_boundary_mae_sec": {"before": .4, "after": .3},
                    "identity_switches": {"before": 2, "after": 2},
                    "identity_collisions": {"before": 0, "after": 0}
                }
            }
            diff = {repo_policy.GOLD, repo_policy.GOLD_DOC, report_path}
            with patch.object(repo_policy, "git",
                              return_value=json.dumps({"revision": 3})):
                report_file.write_text(json.dumps(report))
                self.assertFalse(repo_policy.check_changes(diff, "base", root))
                report["metrics"]["identity_switches"]["after"] = 3
                report_file.write_text(json.dumps(report))
                with self.assertRaisesRegex(repo_policy.PolicyError, "No report"):
                    repo_policy.check_changes(diff, "base", root)

    def test_docker_scope(self):
        self.assertTrue(repo_policy.check_changes({"Dockerfile"}, "base-sha"))
        self.assertFalse(repo_policy.check_changes({"docs/DEVELOPMENT_WORKFLOW.md"},
                                                   "base-sha"))
        self.assertTrue(repo_policy.check_changes(None, None))


if __name__ == "__main__":
    unittest.main()
