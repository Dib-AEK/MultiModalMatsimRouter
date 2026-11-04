# Tests benchmark input normalization and consistent interpretation of Rust/Raptor service segments.
import gzip
import json
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path
from common import (
    prepare_schedule,
    schedule_data,
    scheduled_distance,
    named_legs,
    validate_journey,
    millis,
)


class BenchmarkTests(unittest.TestCase):
    def test_signed_times(self):
        self.assertEqual(millis("-00:05:00"), -300000)
        self.assertEqual(millis("25:00:00"), 90000000)

    def test_explicit_normalization_preserves_original(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "input.xml"
            dest = root / "prepared.xml.gz"
            good = '<transitRoute id="good"><routeProfile><stop refId="A" departureOffset="00:00:00"/><stop refId="B" arrivalOffset="00:05:00" departureOffset="00:00:00"/></routeProfile><departures><departure id="d" departureTime="08:00:00"/></departures></transitRoute>'
            bad = '<transitRoute id="bad"><routeProfile><stop refId="A" departureOffset="00:00:00"/><stop refId="B" arrivalOffset="-00:01:00"/></routeProfile><departures><departure id="b" departureTime="08:00:00"/></departures></transitRoute>'
            xml = (
                '<transitSchedule><transitStops><stopFacility id="A" x="0" y="0"/><stopFacility id="B" x="1000" y="0"/></transitStops><transitLine id="L">'
                + good
                + bad
                + "</transitLine></transitSchedule>"
            )
            source.write_text(xml)
            before = source.read_bytes()
            result = prepare_schedule(source, dest)
            self.assertEqual(before, source.read_bytes())
            self.assertEqual(len(result["excluded_routes"]), 1)
            self.assertEqual(result["normalized_unused_endpoint_offsets"], 2)
            stops, routes, used = schedule_data(dest)
            self.assertEqual(set(routes), {("L", "good")})
            self.assertEqual(
                routes["L", "good"]["stops"][-1]["departureOffset"], "00:05:00"
            )

    def test_distance_and_java_waiting_time_semantics(self):
        stops = {"A": (0, 0), "B": (3, 4), "C": (6, 8)}
        routes = {
            ("L", "R"): {
                "stops": [
                    {"refId": "A", "departureOffset": "00:01:00"},
                    {
                        "refId": "B",
                        "arrivalOffset": "00:02:00",
                        "departureOffset": "00:02:00",
                    },
                    {"refId": "C", "arrivalOffset": "00:03:00"},
                ],
                "departures": {"d": millis("08:00:00")},
            }
        }
        # Raptor boardingTime may include waiting; its vehicleDepTime is the scheduled departure at A.
        j = {
            "arrival": millis("08:03:00"),
            "boardings": 1,
            "legs": [
                {
                    "kind": "transit",
                    "line": "L",
                    "route": "R",
                    "from": "A",
                    "to": "C",
                    "departure": millis("07:59:00"),
                    "vehicle_departure": millis("08:01:00"),
                    "arrival": millis("08:03:00"),
                }
            ],
        }
        legs = named_legs(j, {})
        self.assertEqual(legs[0]["departure"], millis("08:01:00"))
        self.assertEqual(scheduled_distance(legs, stops, routes), (10.0, 0))
        validate_journey({"from": "A", "to": "C", "departure": "07:59:00"}, j, legs, 5)
        legs[0]["arrival"] += 100000
        with self.assertRaises(ValueError):
            scheduled_distance(legs, stops, routes)


if __name__ == "__main__":
    unittest.main()
