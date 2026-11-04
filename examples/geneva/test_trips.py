# Tests real-trip sampling, preservation of first-stop dwell, and the independent weighted scorer.
import csv
import gzip
import tempfile
import unittest
from pathlib import Path
from common import prepare_schedule, schedule_data
from run_trips import sample_trips
from plot_trips import score


class TripBenchmarkTests(unittest.TestCase):
    def test_reservoir_uses_observed_coordinates_times_and_mode(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trips.csv.gz"
            fields = [
                "trip_id",
                "person",
                "dep_time",
                "main_mode",
                "start_x",
                "start_y",
                "end_x",
                "end_y",
            ]
            with gzip.open(path, "wt", newline="") as f:
                writer = csv.DictWriter(f, fields, delimiter=";")
                writer.writeheader()
                for i in range(50):
                    writer.writerow(
                        dict(
                            zip(
                                fields,
                                [
                                    f"p{i}_1",
                                    f"p{i}",
                                    "25:01:02",
                                    "pt" if i % 2 else "car",
                                    i,
                                    0,
                                    i + 1,
                                    1,
                                ],
                            )
                        )
                    )
            rows, eligible = sample_trips(path, 7, 42, "pt")
            self.assertEqual(eligible, 25)
            self.assertEqual(rows, sample_trips(path, 7, 42, "pt")[0])
            self.assertTrue(all(r["departure"] == "25:01:02" for r in rows))
            self.assertTrue(
                all(r["destination"][0] == r["origin"][0] + 1 for r in rows)
            )
            with self.assertRaises(ValueError):
                sample_trips(path, 26, 42, "pt")

    def test_weighted_input_keeps_valid_first_stop_dwell(self):
        with tempfile.TemporaryDirectory() as directory:
            source, dest = [
                Path(directory) / n for n in ("input.xml", "derived.xml.gz")
            ]
            source.write_text(
                '<transitSchedule><transitStops><stopFacility id="A" x="0" y="0"/><stopFacility id="B" x="1" y="0"/></transitStops><transitLine id="L"><transitRoute id="R"><transportMode>rail</transportMode><routeProfile><stop refId="A" arrivalOffset="00:00:00" departureOffset="00:01:00"/><stop refId="B" arrivalOffset="00:05:00" departureOffset="00:00:00"/></routeProfile><departures><departure id="d" departureTime="08:00:00"/></departures></transitRoute></transitLine></transitSchedule>'
            )
            prepare_schedule(source, dest, preserve_first_arrival=True)
            _, routes, _ = schedule_data(dest)
            self.assertEqual(routes["L", "R"]["stops"][0]["arrivalOffset"], "00:00:00")
            self.assertEqual(routes["L", "R"]["mode"], "rail")

    def test_scorer_distinguishes_dwell_waiting_and_transfer_cost(self):
        stops = {"A": (0, 0), "B": (10, 0), "C": (20, 0)}
        routes = {
            ("L", "bus"): dict(
                mode="bus",
                departures={"d": 0},
                stops=[
                    dict(
                        refId="A", arrivalOffset="00:00:10", departureOffset="00:00:20"
                    ),
                    dict(
                        refId="B", arrivalOffset="00:00:40", departureOffset="00:00:40"
                    ),
                ],
            ),
            ("L", "rail"): dict(
                mode="rail",
                departures={"e": 0},
                stops=[
                    dict(
                        refId="B", arrivalOffset="00:01:00", departureOffset="00:01:10"
                    ),
                    dict(
                        refId="C", arrivalOffset="00:01:30", departureOffset="00:01:30"
                    ),
                ],
            ),
        }
        query = dict(
            departure="00:00:00",
            origin=[0, 0],
            destination=[20, 0],
            access=[dict(stop="A", duration=0)],
            egress=[dict(stop="C", duration=0)],
        )
        journey = dict(
            arrival=90000,
            boardings=2,
            generalized_cost=0.68,
            access=query["access"][0],
            egress=query["egress"][0],
            legs=[
                dict(
                    kind="transit",
                    **{"from": "A", "to": "B"},
                    departure=20000,
                    arrival=40000,
                    line="L",
                    route="bus",
                ),
                dict(
                    kind="walk",
                    **{"from": "B", "to": "B"},
                    departure=40000,
                    arrival=45000,
                ),
                dict(
                    kind="transit",
                    **{"from": "B", "to": "C"},
                    departure=70000,
                    arrival=90000,
                    line="L",
                    route="rail",
                ),
            ],
        )
        profile = dict(
            modes_per_hour=dict(bus=3.6, rail=1.8),
            other_per_hour=3.6,
            walk_per_hour=7.2,
            wait_per_hour=3.6,
            transfer_base=0.1,
            transfer_min="-Infinity",
            transfer_max="Infinity",
            transfer_per_travel_hour=0,
            mode_transfers=[dict(from_="bus", to="rail", cost=0.2)],
            walk_distance_factor=1.3,
        )
        profile["mode_transfers"][0]["from"] = profile["mode_transfers"][0].pop("from_")
        result = score(query, journey, {}, routes, stops, profile, {("B", "B"): 5000})
        self.assertAlmostEqual(result["cost"], 0.38)
        self.assertAlmostEqual(result["native_cost_error"], 0.3)
        self.assertEqual(result["waiting_s"], 25)


if __name__ == "__main__":
    unittest.main()
