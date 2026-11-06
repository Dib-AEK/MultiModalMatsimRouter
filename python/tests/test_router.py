"""Checks actual FFI ownership, independent errors, batch ordering and concurrent queries."""

from concurrent.futures import ThreadPoolExecutor
import os
from pathlib import Path
import unittest
import tempfile
import threading
from multimodal_router.session import AsyncRouter
from multimodal_router import Router

CONFIG = Path(__file__).resolve().parents[2] / "examples" / "interop" / "router.toml"


class RouterTests(unittest.TestCase):
    def test_coordinate_policy_config_override_and_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            xml = CONFIG.with_name("matsim_config.xml").read_text().replace(
                "</config>", '<module name="swissRustRouting">'
                '<param name="coordinateWorkBudget" value="123"/>'
                '<param name="qualityWaitSeconds" value="60"/>'
                '<param name="qualityJourneySeconds" value="120"/>'
                '<param name="qualityRadiusMeters" value="800"/></module></config>')
            (p / "config.xml").write_text(xml)
            (p / "timetable.xml").write_bytes(CONFIG.with_name("timetable.xml").read_bytes())
            with Router(p / "config.xml", parameters={"stop_search": {"policy": {"work_budget": 0}}}) as router:
                self.assertEqual(router.metadata()["stop_search"]["policy"], {
                    "work_budget": 0, "quality_wait_seconds": 60.,
                    "quality_journey_seconds": 120., "quality_radius_m": 800.})
                self.assertIsNotNone(router.route_coordinates((0, 0), (1000, 0), "07:59:00"))
            with self.assertRaisesRegex(ValueError, "coordinate search policy"):
                Router(p / "config.xml", parameters={"stop_search": {"policy": {"quality_radius_m": -1}}})

    def test_matsim_declared_geographic_coordinates(self):
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            config = (
                CONFIG.with_name("matsim_config.xml")
                .read_text()
                .replace(
                    '<module name="global">',
                    '<module name="global"><param name="coordinateSystem" value="EPSG:4326"/>',
                )
            )
            (p / "config.xml").write_text(config)
            schedule = (
                CONFIG.with_name("timetable.xml")
                .read_text()
                .replace('x="1000"', 'x="0.01"')
                .replace('x="10000"', 'x="0.1"')
            )
            (p / "timetable.xml").write_text(schedule)
            with Router.load(p / "config.xml") as router:
                self.assertEqual(router.metadata()["coordinates"], "geographic")
                self.assertEqual(
                    router.route_coordinates((0, 0), (0.01, 0), "07:59:00")["arrival"], 29_400_000
                )

    def test_source_checkout_discovers_its_own_library(self):
        from unittest.mock import patch

        with patch.dict(os.environ, {}, clear=False):
            os.environ.pop("MMROUTER_LIBRARY", None)
            with Router.load(CONFIG) as router:
                self.assertIsNotNone(router.route("A", "B", "07:59:00"))

    def test_async_matsim_config_and_shared_workers(self):
        with Router.load(CONFIG.with_name("matsim_config.xml")) as router:
            router.wait(timeout=20)
            self.assertTrue(router.ready)
            metadata = router.metadata()
            self.assertEqual(metadata["threads"], 12)
            self.assertEqual(metadata["preprocessing"]["default_transfer_seconds"], 12)
            self.assertEqual(metadata["preprocessing"]["walking_speed_m_s"], 1)
            self.assertEqual(metadata["stop_search"]["search_radius_m"], 10)
            self.assertEqual(metadata["costs"]["wait_per_hour"], 2)
            with ThreadPoolExecutor(max_workers=12) as pool:
                results = list(
                    pool.map(lambda _: router.worker().route("A", "B", "07:59:00"), range(120))
                )
            self.assertTrue(all(r == results[0] for r in results))
            self.assertEqual(
                router.route_coordinates((0, 0), (1000, 0), "07:59:00")["arrival"], 29_400_000
            )

    def test_raw_schedule_options_and_async_failure(self):
        with Router.start(
            CONFIG.with_name("timetable.xml"),
            format="matsim",
            parameters={"preprocessing": {"walking_radius_m": 0}},
            threads=2,
        ) as router:
            self.assertEqual(router.metadata()["threads"], 2)
            self.assertEqual(router.route("A", "B", "07:59:00")["arrival"], 29_400_000)
        with Router.load(CONFIG.with_name("absent.xml")) as failed:
            with self.assertRaises(FileNotFoundError):
                failed.wait()

    def test_gtfs_loading_directory_and_zip(self):
        import zipfile

        feed = CONFIG.parent / "gtfs"

        def check(path):
            with Router.load(path, service_date="2026-11-05", threads=2) as router:
                self.assertEqual(router.route("A", "B", "07:59:00")["arrival"], 29_400_000)

        check(feed)
        with Router.load(feed, service_date="2026-11-06") as empty:
            self.assertIsNone(empty.route_coordinates((6.63, 46.52), (6.64, 46.52), "07:59:00"))
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "feed.zip"
            with zipfile.ZipFile(archive, "w") as z:
                for p in feed.iterdir():
                    z.write(p, p.name)
            check(archive)
        with Router.load(feed) as failed:
            with self.assertRaisesRegex(ValueError, "service_date"):
                failed.wait()

    def test_unsupported_matsim_feature_and_partial_overrides(self):
        with Router.load(
            CONFIG.with_name("matsim_config.xml"),
            parameters={"preprocessing": {"walking_radius_m": 50}, "execution": {"threads": 2}},
        ) as router:
            metadata = router.metadata()
            self.assertEqual(metadata["preprocessing"]["walking_radius_m"], 50)
            self.assertEqual(metadata["preprocessing"]["default_transfer_seconds"], 12)
            self.assertEqual(metadata["threads"], 2)
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory) / "config.xml"
            text = (
                CONFIG.with_name("matsim_config.xml")
                .read_text()
                .replace(
                    "</config>",
                    '<module name="swissRailRaptor"><param name="useRangeQuery" value="true"/></module></config>',
                )
            )
            p.write_text(text)
            with Router.load(p) as failed:
                with self.assertRaisesRegex(ValueError, "useRangeQuery"):
                    failed.wait()

    def test_stop_access_attributes_and_explicit_zero_transfer(self):
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            config = CONFIG.with_name("matsim_config.xml").read_text()
            (p / "config.xml").write_text(config)
            schedule = CONFIG.with_name("timetable.xml").read_text()
            schedule = schedule.replace(
                '<stopFacility id="A" x="0" y="0" name="Origin"/>',
                '<stopFacility id="A" x="0" y="0" name="Origin"><attributes><attribute name="accessTime" class="java.lang.Double">10.0</attribute></attributes></stopFacility>',
            )
            schedule = schedule.replace(
                "</transitSchedule>",
                """
              <transitLine id="L2"><transitRoute id="R2"><transportMode>bus</transportMode>
                <routeProfile><stop refId="B" departureOffset="00:00:00"/><stop refId="C" arrivalOffset="00:09:54" departureOffset="-01:00:00"/></routeProfile>
                <departures><departure id="D2" departureTime="08:10:06" vehicleRefId="bus2"/></departures>
              </transitRoute></transitLine></transitSchedule>""",
            )
            (p / "timetable.xml").write_text(schedule)
            with Router.load(p / "config.xml") as router:
                journey = router.route_coordinates((0, 0), (1000, 0), "07:59:00")
                self.assertAlmostEqual(journey["generalized_cost"], 0.875)
                self.assertIsNone(
                    router.route("A", "C", "07:59:00")
                )  # 12-second minimum misses the 6-second change.
            schedule = schedule.replace(
                "</transitStops>",
                '</transitStops><minimalTransferTimes><relation fromStop="B" toStop="B" transferTime="0"/></minimalTransferTimes>',
            )
            (p / "timetable.xml").write_text(schedule)
            with Router.load(p / "config.xml") as router:
                self.assertEqual(router.route("A", "C", "07:59:00")["arrival"], 30_000_000)

    def test_startup_is_nonblocking_and_close_during_load_releases_result(self):
        started = threading.Event()
        release = threading.Event()
        closed = threading.Event()

        class Delayed:
            def __init__(self, *args, **kwargs):
                started.set()
                if not release.wait(10):
                    raise RuntimeError("test loader was never released")

            def close(self):
                closed.set()

        session = AsyncRouter(Delayed, "unused")
        self.assertTrue(started.wait(5))
        self.assertFalse(session.ready)
        session.close()  # Must not wait for the deliberately blocked loader.
        release.set()
        self.assertTrue(closed.wait(5))
        with self.assertRaises(RuntimeError):
            session.wait()

    def test_generic_matsim_config_and_relative_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory) / "config.xml"
            p.write_text(
                CONFIG.with_name("matsim_config.xml")
                .read_text()
                .split('<module name="eqasim:raptor">')[0]
                + "</config>"
            )
            (Path(directory) / "timetable.xml").write_text(
                CONFIG.with_name("timetable.xml").read_text()
            )
            with Router.load(p) as router:
                self.assertEqual(router.metadata()["costs"]["other_per_hour"], 12)
                self.assertAlmostEqual(router.route("A", "B", "07:59:00")["generalized_cost"], 2.2)
            text = p.read_text().replace(
                "</config>",
                """<module name="scoring"><parameterset type="scoringParameters">
                <parameterset type="modeParams"><param name="mode" value="pt"/><param name="marginalUtilityOfTraveling_util_hr" value="-8"/></parameterset>
                <param name="performing" value="4"/><param name="waitingPt" value="-3"/><parameterset type="unused"/>
                </parameterset></module></config>""",
            )
            p.write_text(text)
            with Router.load(p) as router:
                self.assertEqual(router.metadata()["costs"]["other_per_hour"], 12)
                self.assertEqual(router.metadata()["costs"]["wait_per_hour"], 7)

    def test_explicit_cost_overrides_win_over_imported_coefficients(self):
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory) / "router.toml"
            xml = CONFIG.with_name("matsim_config.xml").resolve().as_posix()
            schedule = CONFIG.with_name("timetable.xml").resolve().as_posix()
            p.write_text(
                f'[input]\nformat="matsim"\npath="{schedule}"\ncoordinates="projected_meters"\n[routing]\nobjective="generalized_cost"\nmatsim_config="{xml}"\n'
            )
            with Router.load(p, parameters={"routing": {"costs": {"wait_per_hour": 9}}}) as router:
                self.assertEqual(router.metadata()["costs"]["wait_per_hour"], 9)

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
            queries = [
                {"from": s, "to": t, "departure": "07:59:00"}
                for s, t in [("A", "B"), ("A", "C"), ("bad", "B")]
            ]
            batch = router.route_many(queries)
            self.assertTrue(batch[0]["ok"])
            self.assertIsNone(batch[1]["result"])
            self.assertFalse(batch[2]["ok"])
            with ThreadPoolExecutor(max_workers=12) as pool:
                results = list(pool.map(lambda _: router.route_many(queries), range(36)))
            self.assertTrue(all(r == batch for r in results))
            q = {
                "departure": 28_740_000,
                "max_boardings": 5,
                "max_duration": 3_600_000,
                "access": [{"stop": 0, "duration": 0}],
                "egress": [{"stop": 1, "duration": 0}],
            }
            weighted = router.weighted_many([q])[0]
            self.assertTrue(weighted["ok"])
            self.assertEqual(weighted["result"]["arrival"], batch[0]["result"]["arrival"])

    def test_bad_configuration_has_actionable_error(self):
        with self.assertRaises(ValueError):
            Router(Path(__file__))


if __name__ == "__main__":
    unittest.main()
