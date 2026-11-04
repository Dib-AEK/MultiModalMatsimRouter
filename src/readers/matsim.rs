//! Streams MATSim schedule XML/gzip and expands route-relative stop offsets into absolute connections.
use crate::{
    model::*,
    readers::xml_input,
    time::{NEVER, parse_time, seconds},
};
use anyhow::{Context, Result, ensure};
use quick_xml::{
    Reader,
    events::{BytesStart, Event},
};
use std::{collections::HashMap, path::Path};

fn attributes(event: &BytesStart<'_>) -> Result<HashMap<String, String>> {
    event
        .attributes()
        .map(|a| {
            let a = a?;
            Ok((
                a.key.as_ref().to_owned(),
                a.normalized_value(quick_xml::XmlVersion::Implicit1_0)?
                    .into_owned(),
            ))
        })
        .collect()
}
fn required<'a>(a: &'a HashMap<String, String>, key: &str) -> Result<&'a str> {
    a.get(key)
        .map(String::as_str)
        .with_context(|| format!("missing MATSim attribute {key}"))
}
#[derive(Default)]
struct Template {
    id: String,
    mode: String,
    calls: Vec<HashMap<String, String>>,
    departures: Vec<(String, u32)>,
}

pub fn read(path: &Path, coordinates: Coordinates) -> Result<RawTimetable> {
    let mut reader = Reader::from_reader(xml_input(path)?);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut stops = Vec::new();
    let mut ids = HashMap::new();
    let mut trips = Vec::new();
    let mut line = String::new();
    let mut route = Template::default();
    let mut in_mode = false;
    let mut footpaths = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .context("reading MATSim schedule XML")?
        {
            Event::Start(e) | Event::Empty(e) => {
                let a = attributes(&e)?;
                match e.name().as_ref() {
                    "stopFacility" => {
                        let id = required(&a, "id")?.to_owned();
                        ensure!(!ids.contains_key(&id), "duplicate MATSim stop {id}");
                        let index = stops.len().try_into()?;
                        ids.insert(id.clone(), index);
                        stops.push(Stop {
                            id: id.clone(),
                            name: a.get("name").cloned().unwrap_or(id),
                            x: required(&a, "x")?.parse()?,
                            y: required(&a, "y")?.parse()?,
                            transfer_buffer: 0,
                        });
                    }
                    "relation" if a.contains_key("fromStop") && a.contains_key("toStop") => {
                        let from = *ids
                            .get(required(&a, "fromStop")?)
                            .context("unknown transfer origin")?;
                        let to = *ids
                            .get(required(&a, "toStop")?)
                            .context("unknown transfer destination")?;
                        let duration = seconds(required(&a, "transferTime")?.parse()?)?;
                        if from == to {
                            stops[from as usize].transfer_buffer = duration;
                        } else {
                            footpaths.push((from, to, duration));
                        }
                    }
                    "transitLine" => line = required(&a, "id")?.to_owned(),
                    "transitRoute" => {
                        route = Template {
                            id: required(&a, "id")?.to_owned(),
                            ..Template::default()
                        }
                    }
                    "transportMode" => in_mode = true,
                    "stop" => route.calls.push(a),
                    "departure" => route.departures.push((
                        required(&a, "id")?.to_owned(),
                        parse_time(required(&a, "departureTime")?)?,
                    )),
                    _ => {}
                }
            }
            Event::Text(text) if in_mode => route.mode.push_str(&text.xml10_content()),
            Event::End(e) => match e.name().as_ref() {
                "transportMode" => in_mode = false,
                "transitRoute" => {
                    ensure!(
                        !route.mode.is_empty(),
                        "missing mode on MATSim route {}",
                        route.id
                    );
                    let count = route.calls.len();
                    let mut calls = Vec::new();
                    for (i, a) in route.calls.iter().enumerate() {
                        let stop = *ids
                            .get(required(a, "refId")?)
                            .context("MATSim route references unknown stop")?;
                        let arrival = a
                            .get("arrivalOffset")
                            .filter(|v| v.as_str() != "undefined")
                            .map(|v| parse_time(v))
                            .transpose()?;
                        let departure = a
                            .get("departureOffset")
                            .filter(|v| v.as_str() != "undefined")
                            .map(|v| parse_time(v))
                            .transpose()?;
                        let arrival = arrival
                            .or(if i == 0 { departure } else { None })
                            .context("missing interior/last arrivalOffset")?;
                        let departure = departure
                            .or(if i + 1 == count { Some(arrival) } else { None })
                            .context("missing interior/first departureOffset")?;
                        let flag = |name: &str| -> Result<bool> {
                            match a.get(name).map(String::as_str) {
                                None | Some("true") => Ok(true),
                                Some("false") => Ok(false),
                                Some(v) => anyhow::bail!("invalid {name}: {v}"),
                            }
                        };
                        calls.push(Call {
                            stop,
                            arrival,
                            departure,
                            pickup: flag("allowBoarding")?,
                            dropoff: flag("allowAlighting")?,
                        });
                    }
                    for (departure_id, start) in &route.departures {
                        let mut absolute = calls.clone();
                        for call in &mut absolute {
                            call.arrival = start
                                .checked_add(call.arrival)
                                .filter(|&t| t < NEVER)
                                .context("MATSim arrival overflow")?;
                            call.departure = start
                                .checked_add(call.departure)
                                .filter(|&t| t < NEVER)
                                .context("MATSim departure overflow")?;
                        }
                        let id = format!(
                            "{}:{}|{}:{}|{}:{}",
                            line.len(),
                            line,
                            route.id.len(),
                            route.id,
                            departure_id.len(),
                            departure_id
                        );
                        trips.push(RawTrip {
                            trip: Trip {
                                id,
                                line: line.clone(),
                                route: route.id.clone(),
                                departure: departure_id.clone(),
                                mode: route.mode.clone(),
                            },
                            calls: absolute,
                        });
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(RawTimetable {
        coordinates,
        stops,
        trips,
        footpaths,
    })
}
