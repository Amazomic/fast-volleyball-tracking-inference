"""CPU thread counts must not be confused with OS CPU affinity masks."""
import os
import unittest
from unittest.mock import patch

from scripts.volleyball_studio.resources import configure_cpu_threads


class CpuThreadsTest(unittest.TestCase):
    def test_configures_math_library_threads_without_affinity(self):
        original_affinity = set(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None
        with patch.dict(os.environ, {}, clear=False):
            with patch.object(os, "sched_setaffinity", side_effect=AssertionError("Do not pin CPU affinity"), create=True) as setter:
                configure_cpu_threads(10)
                setter.assert_not_called()
                for key in ("OMP_NUM_THREADS","OPENBLAS_NUM_THREADS","MKL_NUM_THREADS","NUMEXPR_NUM_THREADS"):
                    self.assertEqual(os.environ[key], "10")
        if original_affinity is not None:
            self.assertEqual(set(os.sched_getaffinity(0)), original_affinity)

    def test_bad_thread_count(self):
        for count in (0, -1):
            with self.subTest(count=count), self.assertRaises(ValueError):
                configure_cpu_threads(count)


if __name__ == "__main__":
    unittest.main()
