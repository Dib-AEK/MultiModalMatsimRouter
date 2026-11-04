# Reads MATSim benchmark inputs and resolves selected service segments into a shared distance measure.
import gzip
import math
import urllib.parse
import xml.etree.ElementTree as ET
from pathlib import Path


def millis(text):
    sign = -1 if text.startswith("-") else 1
    h, m, s = text.lstrip("-").split(":")
    return sign * round((int(h) * 3600 + int(m) * 60 + float(s)) * 1000)


def input_paths(config):
    modules = {
        m.get("name"): {p.get("name"): p.get("value") for p in m.findall("param")}
        for m in ET.parse(config).getroot().findall("module")
    }

    def resolve(text):
        if text.startswith("file:"):
            text = urllib.parse.unquote(urllib.parse.urlparse(text).path)
            if len(text) > 2 and text[0] == "/" and text[2] == ":":
                text = text[1:]
        path = Path(text)
        return (
            path.resolve() if path.is_absolute() else (config.parent / path).resolve()
        )

    return (
        resolve(modules["transit"]["transitScheduleFile"]),
        resolve(modules["network"]["inputNetworkFile"]),
        modules,
    )


def schedule_data(path):
    stops, routes, used = {}, {}, set()
    line, current = None, None
    with (gzip.open if path.suffix == ".gz" else open)(path, "rb") as stream:
        for event, e in ET.iterparse(stream, events=("start", "end")):
            if event == "start":
                if e.tag == "transitLine":
                    line = e.get("id")
                elif e.tag == "transitRoute":
                    current = {"id": e.get("id"), "stops": [], "departures": {}}
                continue
            if e.tag == "stopFacility":
                stops[e.get("id")] = (float(e.get("x")), float(e.get("y")))
            elif e.tag == "stop" and current is not None:
                current["stops"].append(dict(e.attrib))
                used.add(e.get("refId"))
            elif e.tag == "departure" and current is not None:
                current["departures"][e.get("id")] = millis(e.get("departureTime"))
            elif e.tag == "transportMode" and current is not None:
                current["mode"] = e.text.strip()
            elif e.tag == "transitRoute":
                routes[line, current["id"]] = current
                current = None
            e.clear()
    return stops, routes, sorted(used)


def named_legs(journey, metadata):
    if journey is None:
        return []
    result = []
    for source in journey["legs"]:
        leg = dict(source)
        if isinstance(leg["from"], int):
            leg["from"] = metadata["stops"][leg["from"]]["id"]
            leg["to"] = metadata["stops"][leg["to"]]["id"]
            if leg["kind"] == "transit":
                trip = metadata["trips"][leg["trip"]]
                leg.update(
                    line=trip["line"],
                    route=trip["route"],
                    departure_id=trip["departure"],
                )
        elif leg["kind"] == "transit" and "vehicle_departure" in leg:
            # Raptor's boardingTime includes waiting at the stop; vehicleDepTime is the scheduled leave time.
            leg["departure"] = leg["vehicle_departure"]
        result.append(leg)
    return result


def segment_candidates(leg, stops, routes):
    """Match a service segment using its boarding/alighting times, including repeated stops."""
    route = routes[leg["line"], leg["route"]]
    calls = route["stops"]
    fixed_start = route["departures"].get(leg.get("departure_id"))
    choices = []
    for i, a in enumerate(calls):
        if a["refId"] != leg["from"] or a.get("allowBoarding") == "false":
            continue
        departure = millis(a.get("departureOffset", a.get("arrivalOffset")))
        # Raptor's RoutePart.vehicleDepTime is at the boarding stop, not the route's service origin.
        start = fixed_start if fixed_start is not None else leg["departure"] - departure
        service_error = min(abs(start - t) for t in route["departures"].values())
        for j in range(i + 1, len(calls)):
            b = calls[j]
            if b["refId"] != leg["to"] or b.get("allowAlighting") == "false":
                continue
            arrival = millis(b.get("arrivalOffset", b.get("departureOffset")))
            penalty = (
                abs(start + departure - leg["departure"])
                + abs(start + arrival - leg["arrival"])
                + service_error
            )
            distance = sum(
                math.dist(stops[calls[k]["refId"]], stops[calls[k + 1]["refId"]])
                for k in range(i, j)
            )
            choices.append((penalty, distance))
    return sorted(choices)


def scheduled_distance(legs, stops, routes):
    total, ambiguous = 0.0, 0
    for leg in legs:
        if leg["kind"] == "walk":
            total += math.dist(stops[leg["from"]], stops[leg["to"]])
            continue
        choices = segment_candidates(leg, stops, routes)
        if not choices or choices[0][0] > 2000:
            raise ValueError(f"No time-consistent schedule segment: {leg}")
        best = [distance for penalty, distance in choices if penalty == choices[0][0]]
        ambiguous += len(best) > 1
        total += min(best)
    return total, ambiguous


def validate_journey(query, journey, legs, transfer_seconds):
    """Verify continuity, transfer gaps, timing and counts outside the timed engine calls."""
    stop = query["from"]
    time = millis(query["departure"])
    boardings = 0
    for leg in legs:
        if leg["from"] != stop or leg["departure"] < time:
            raise ValueError("Broken itinerary chronology/continuity")
        if leg["kind"] == "transit":
            if boardings and leg["departure"] < time + transfer_seconds * 1000:
                raise ValueError("Transfer buffer violated")
            boardings += 1
        if leg["arrival"] < leg["departure"]:
            raise ValueError("Negative leg duration")
        stop = leg["to"]
        time = leg["arrival"]
    if (
        stop != query["to"]
        or abs(journey["arrival"] - time) > 1000
        or boardings != journey["boardings"]
    ):
        raise ValueError("Itinerary endpoint/timing/count mismatch")


def prepare_schedule(source, destination, preserve_first_arrival=False):
    """Create a derived benchmark input, explicitly excluding malformed operating chronology."""
    excluded, normalized = [], 0
    stack = []
    with (gzip.open if source.suffix == ".gz" else open)(
        source, "rb"
    ) as input_stream, gzip.open(destination, "wb") as output:
        output.write(
            b'<?xml version="1.0" encoding="utf-8"?>\n<!DOCTYPE transitSchedule SYSTEM "http://www.matsim.org/files/dtd/transitSchedule_v2.dtd">\n<transitSchedule>\n'
        )
        for event, element in ET.iterparse(input_stream, events=("start", "end")):
            if event == "start":
                stack.append(element)
                continue
            if element.tag == "transitRoute":
                calls = element.findall("./routeProfile/stop")
                reason = None
                route_normalizations = 0
                try:
                    if len(calls) < 2:
                        raise ValueError("fewer than two stops")
                    # First arrival affects dwell costs in weighted routing; preserve valid values there.
                    if calls[0].get("departureOffset") not in (None, "undefined"):
                        if (
                            not preserve_first_arrival
                            or calls[0].get("arrivalOffset") in (None, "undefined")
                        ) and calls[0].get("arrivalOffset") != calls[0].get(
                            "departureOffset"
                        ):
                            calls[0].set(
                                "arrivalOffset", calls[0].get("departureOffset")
                            )
                            route_normalizations += 1
                    if calls[-1].get("arrivalOffset") not in (None, "undefined"):
                        if calls[-1].get("departureOffset") != calls[-1].get(
                            "arrivalOffset"
                        ):
                            calls[-1].set(
                                "departureOffset", calls[-1].get("arrivalOffset")
                            )
                            route_normalizations += 1
                    previous = None
                    for call in calls:
                        arrival = millis(call.get("arrivalOffset"))
                        departure = millis(call.get("departureOffset"))
                        if (
                            arrival < 0
                            or departure < arrival
                            or (previous is not None and arrival < previous)
                        ):
                            raise ValueError(
                                f"non-monotonic offset at {call.get('refId')}: {call.attrib}"
                            )
                        previous = departure
                except (ValueError, TypeError, AttributeError, KeyError) as error:
                    reason = str(error)
                if reason:
                    excluded.append(
                        {
                            "line": stack[-2].get("id"),
                            "route": element.get("id"),
                            "departures": len(
                                element.findall("./departures/departure")
                            ),
                            "reason": reason,
                        }
                    )
                    stack[-2].remove(element)
                else:
                    normalized += route_normalizations
            if len(stack) == 2:
                output.write(ET.tostring(element, encoding="utf-8"))
                stack[0].remove(element)
                element.clear()
            stack.pop()
        output.write(b"\n</transitSchedule>\n")
    return {
        "excluded_routes": excluded,
        "normalized_unused_endpoint_offsets": normalized,
    }
