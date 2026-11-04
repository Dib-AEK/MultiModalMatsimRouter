//! Imports dated GTFS directory/ZIP feeds, honoring calendars, stop sequence and boarding permissions.
use crate::{
    model::*,
    time::{parse_time, seconds},
};
use anyhow::{Context, Result, bail, ensure};
use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

enum Feed {
    Directory(PathBuf),
    Zip(zip::ZipArchive<File>),
}
impl Feed {
    fn rows<T: DeserializeOwned>(
        &mut self,
        name: &str,
        required: bool,
        mut accept: impl FnMut(T) -> Result<()>,
    ) -> Result<bool> {
        let input: Option<Box<dyn Read + '_>> = match self {
            Self::Directory(dir) => {
                if dir.join(name).exists() {
                    Some(Box::new(File::open(dir.join(name))?))
                } else {
                    None
                }
            }
            Self::Zip(zip) => match zip.by_name(name) {
                Ok(file) => Some(Box::new(file)),
                Err(zip::result::ZipError::FileNotFound) => None,
                Err(e) => return Err(e.into()),
            },
        };
        let Some(input) = input else {
            ensure!(!required, "GTFS missing {name}");
            return Ok(false);
        };
        for row in csv::Reader::from_reader(input).deserialize::<T>() {
            accept(row.with_context(|| format!("GTFS {name}"))?)
                .with_context(|| format!("GTFS {name}"))?;
        }
        Ok(true)
    }
}
#[derive(Deserialize)]
struct StopRow {
    stop_id: String,
    #[serde(default)]
    stop_name: String,
    #[serde(default)]
    stop_lon: String,
    #[serde(default)]
    stop_lat: String,
    #[serde(default)]
    location_type: String,
}
#[derive(Deserialize)]
struct Calendar {
    service_id: String,
    monday: u8,
    tuesday: u8,
    wednesday: u8,
    thursday: u8,
    friday: u8,
    saturday: u8,
    sunday: u8,
    start_date: String,
    end_date: String,
}
#[derive(Deserialize)]
struct Exception {
    service_id: String,
    date: String,
    exception_type: u8,
}
#[derive(Deserialize)]
struct RouteRow {
    route_id: String,
    route_type: String,
}
#[derive(Deserialize)]
struct TripRow {
    route_id: String,
    service_id: String,
    trip_id: String,
}
#[derive(Deserialize)]
struct TimeRow {
    trip_id: String,
    arrival_time: String,
    departure_time: String,
    stop_id: String,
    stop_sequence: u32,
    #[serde(default)]
    pickup_type: String,
    #[serde(default)]
    drop_off_type: String,
}
#[derive(Deserialize)]
struct Frequency {
    trip_id: String,
}
#[derive(Deserialize)]
struct Transfer {
    from_stop_id: String,
    to_stop_id: String,
    #[serde(default)]
    transfer_type: String,
    #[serde(default)]
    min_transfer_time: String,
    #[serde(default)]
    from_route_id: String,
    #[serde(default)]
    to_route_id: String,
    #[serde(default)]
    from_trip_id: String,
    #[serde(default)]
    to_trip_id: String,
}
fn date(text: &str) -> Result<NaiveDate> {
    let parsed = NaiveDate::parse_from_str(text, "%Y%m%d")
        .or_else(|_| NaiveDate::parse_from_str(text, "%Y-%m-%d"))?;
    Ok(parsed)
}
fn permission(value: &str) -> Result<bool> {
    match value {
        "" | "0" => Ok(true),
        "1" => Ok(false),
        _ => bail!("demand-responsive pickup/dropoff type {value} is not supported"),
    }
}

pub fn read(path: &Path, service_date: &str, walking_speed: f64) -> Result<RawTimetable> {
    let day = date(service_date)?;
    let mut feed = if path.is_dir() {
        Feed::Directory(path.to_owned())
    } else {
        Feed::Zip(zip::ZipArchive::new(File::open(path)?)?)
    };
    let mut active = HashSet::new();
    let calendar = feed.rows("calendar.txt", false, |row: Calendar| {
        let weekdays = [
            row.monday,
            row.tuesday,
            row.wednesday,
            row.thursday,
            row.friday,
            row.saturday,
            row.sunday,
        ];
        ensure!(weekdays.iter().all(|&n| n <= 1), "invalid weekday flag");
        if date(&row.start_date)? <= day
            && day <= date(&row.end_date)?
            && weekdays[day.weekday().num_days_from_monday() as usize] == 1
        {
            active.insert(row.service_id);
        }
        Ok(())
    })?;
    let exceptions = feed.rows("calendar_dates.txt", false, |row: Exception| {
        if date(&row.date)? == day {
            match row.exception_type {
                1 => {
                    active.insert(row.service_id);
                }
                2 => {
                    active.remove(&row.service_id);
                }
                _ => bail!("invalid calendar exception"),
            }
        }
        Ok(())
    })?;
    ensure!(
        calendar || exceptions,
        "GTFS needs calendar.txt or calendar_dates.txt"
    );
    let mut stops = Vec::new();
    let mut ids = HashMap::new();
    feed.rows("stops.txt", true, |row: StopRow| {
        if !matches!(row.location_type.as_str(), "" | "0") {
            return Ok(());
        }
        let index = stops.len().try_into()?;
        ensure!(
            ids.insert(row.stop_id.clone(), index).is_none(),
            "duplicate stop {}",
            row.stop_id
        );
        stops.push(Stop {
            id: row.stop_id,
            name: row.stop_name,
            x: row
                .stop_lon
                .parse()
                .context("missing/invalid stop longitude")?,
            y: row
                .stop_lat
                .parse()
                .context("missing/invalid stop latitude")?,
            transfer_buffer: 0,
        });
        Ok(())
    })?;
    let mut modes = HashMap::new();
    feed.rows("routes.txt", true, |row: RouteRow| {
        ensure!(
            modes.insert(row.route_id, row.route_type).is_none(),
            "duplicate route ID"
        );
        Ok(())
    })?;
    let mut trips = BTreeTrips::new();
    feed.rows("trips.txt", true, |row: TripRow| {
        if active.contains(&row.service_id) {
            let mode = modes
                .get(&row.route_id)
                .context("unknown route ID")?
                .clone();
            ensure!(
                trips
                    .insert(
                        row.trip_id.clone(),
                        (
                            Trip {
                                id: row.trip_id.clone(),
                                line: row.route_id.clone(),
                                route: row.route_id,
                                departure: row.trip_id,
                                mode
                            },
                            Vec::new()
                        )
                    )
                    .is_none(),
                "duplicate trip ID"
            );
        }
        Ok(())
    })?;
    feed.rows("frequencies.txt", false, |row: Frequency| {
        ensure!(
            !trips.contains_key(&row.trip_id),
            "frequency-based service is not supported; materialize departures first ({})",
            row.trip_id
        );
        Ok(())
    })?;
    feed.rows("stop_times.txt", true, |row: TimeRow| {
        if let Some((_, calls)) = trips.get_mut(&row.trip_id) {
            let stop = *ids
                .get(&row.stop_id)
                .context("stop_times references unknown/non-platform stop")?;
            calls.push((
                row.stop_sequence,
                Call {
                    stop,
                    arrival: parse_time(&row.arrival_time)
                        .context("missing/interpolated stop times are not supported")?,
                    departure: parse_time(&row.departure_time)?,
                    pickup: permission(&row.pickup_type)?,
                    dropoff: permission(&row.drop_off_type)?,
                },
            ));
        }
        Ok(())
    })?;
    let mut footpaths = Vec::new();
    feed.rows("transfers.txt",false,|row:Transfer| {
        ensure!([&row.from_route_id,&row.to_route_id,&row.from_trip_id,&row.to_trip_id].iter().all(|v|v.is_empty()),"route/trip-scoped transfers are not supported");
        let from=*ids.get(&row.from_stop_id).context("transfer references unknown/platform-group stop")?;
        let to=*ids.get(&row.to_stop_id).context("transfer references unknown/platform-group stop")?;
        match row.transfer_type.as_str() {
            ""|"0"=>{if from!=to {let duration=seconds(crate::spatial::distance(&stops[from as usize],&stops[to as usize],Coordinates::Geographic)/walking_speed)?;footpaths.push((from,to,duration));}},
            "2" if from==to => stops[from as usize].transfer_buffer=seconds(row.min_transfer_time.parse()?)?,
            _=>bail!("transfer type {} between {} and {} is not supported; scoped/minimum-time rules across distinct stops need a richer transfer model",row.transfer_type,row.from_stop_id,row.to_stop_id),
        }Ok(())
    })?;
    let mut output = Vec::new();
    for (_, (trip, mut calls)) in trips {
        calls.sort_by_key(|(sequence, _)| *sequence);
        ensure!(
            calls.windows(2).all(|p| p[0].0 < p[1].0),
            "duplicate stop_sequence in {}",
            trip.id
        );
        output.push(RawTrip {
            trip,
            calls: calls.into_iter().map(|(_, call)| call).collect(),
        });
    }
    Ok(RawTimetable {
        coordinates: Coordinates::Geographic,
        stops,
        trips: output,
        footpaths,
    })
}
type BTreeTrips = std::collections::BTreeMap<String, (Trip, Vec<(u32, Call)>)>;
