"""Checks actual FFI ownership, independent errors, batch ordering and concurrent queries."""
from concurrent.futures import ThreadPoolExecutor
import os
from pathlib import Path
import unittest
from multimodal_router import Router

CONFIG = Path(__file__).resolve().parents[2] / "examples" / "interop" / "router.toml"


class RouterTests(unittest.TestCase):
    def test_journey_no_path_and_invalid_input(self):
        with Router(CONFIG) as router:
            result = router.route("A", "B", "07:59:00")
            self.assertEqual(result["arrival"], 29_400_000)
            self.assertEqual(result["boardings"], 1)
            self.assertAlmostEqual(result["generalized_cost"], 5 / 6 + 1 / 30)
            self.assertIsNone(router.route("A", "C", "07:59:00"))
            self.assertIsNone(router.route("A", "B", "09:00:00"))
            with self.assertRaisesRegex(ValueError, "unknown origin"):
                router.route("不存在", "B", "07:59:00")
            self.assertEqual(router.route("A", "B", "07:59:00"), result)
            self.assertEqual(router.metadata(include_entities=True)["stops"][0]["id"], "A")
        router.close()
        with self.assertRaisesRegex(RuntimeError, "closed"):
            router.route("A", "B", "07:59:00")

    def test_batch_preserves_errors_and_parallel_callers(self):
        with Router(CONFIG) as router:
            queries = [{"from": s, "to": t, "departure": "07:59:00"}
                       for s, t in [("A", "B"), ("A", "C"), ("bad", "B")]]
            batch = router.route_many(queries)
            self.assertTrue(batch[0]["ok"])
            self.assertIsNone(batch[1]["result"])
            self.assertFalse(batch[2]["ok"])
            with ThreadPoolExecutor(max_workers=12) as pool:
                results = list(pool.map(lambda _: router.route_many(queries), range(36)))
            self.assertTrue(all(r == batch for r in results))
            q = {"departure": 28_740_000, "max_boardings": 5, "max_duration": 3_600_000,
                 "access": [{"stop": 0, "duration": 0}], "egress": [{"stop": 1, "duration": 0}]}
            weighted = router.weighted_many([q])[0]
            self.assertTrue(weighted["ok"])
            self.assertEqual(weighted["result"]["arrival"], batch[0]["result"]["arrival"])

    def test_bad_configuration_has_actionable_error(self):
        with self.assertRaises(ValueError):
            Router(Path(__file__))


if __name__ == "__main__":
    unittest.main()
