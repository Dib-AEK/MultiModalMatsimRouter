//! Reads the versioned, big-endian snapshot exported from the actual in-memory MATSim scenario.
use anyhow::{Context, Result, ensure};
use multimodal_matsim_router::{config::Preprocessing, model::*, preprocess};
use std::{
    fs::File,
    io::{BufReader, Read},
};

fn integer(input: &mut impl Read) -> Result<u32> {
    let mut b = [0; 4];
    input.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}
fn number(input: &mut impl Read) -> Result<f64> {
    let mut b = [0; 8];
    input.read_exact(&mut b)?;
    Ok(f64::from_be_bytes(b))
}
fn string(input: &mut impl Read) -> Result<String> {
    let count = integer(input)? as usize;
    ensure!(count <= 1_048_576, "oversized snapshot string");
    let mut bytes = vec![0; count];
    input.read_exact(&mut bytes)?;
    Ok(String::from_utf8(bytes)?)
}
pub fn read(path: &str) -> Result<Timetable> {
    let mut input = BufReader::with_capacity(1024 * 1024, File::open(path)?);
    decode(&mut input).context("invalid MATSim native snapshot")
}
fn decode(input: &mut impl Read) -> Result<Timetable> {
    let mut magic = [0; 8];
    input.read_exact(&mut magic)?;
    ensure!(&magic == b"MMJNI001", "unsupported snapshot version");
    let stop_count = integer(input)?;
    ensure!(
        stop_count > 0 && stop_count <= 10_000_000,
        "invalid snapshot stop count"
    );
    let mut stops = Vec::new();
    for id in 0..stop_count {
        stops.push(Stop {
            id: id.to_string(),
            name: String::new(),
            x: number(input)?,
            y: number(input)?,
            transfer_buffer: integer(input)?,
        });
    }
    let count = integer(input)?;
    ensure!(count <= 100_000_000, "invalid snapshot trip count");
    let mut trips = Vec::new();
    for id in 0..count {
        let mode = string(input)?;
        let count = integer(input)?;
        ensure!(
            (2..=1_000_000).contains(&count),
            "invalid snapshot call count"
        );
        let mut calls = Vec::new();
        for _ in 0..count {
            let stop = integer(input)?;
            let arrival = integer(input)?;
            let departure = integer(input)?;
            let mut flags = [0];
            input.read_exact(&mut flags)?;
            ensure!(flags[0] <= 3, "invalid call flags");
            calls.push(Call {
                stop,
                arrival,
                departure,
                pickup: flags[0] & 1 != 0,
                dropoff: flags[0] & 2 != 0,
            });
        }
        trips.push(RawTrip {
            trip: Trip {
                id: id.to_string(),
                line: String::new(),
                route: String::new(),
                departure: String::new(),
                mode,
            },
            calls,
        });
    }
    let count = integer(input)?;
    let mut footpaths = Vec::new();
    for _ in 0..count {
        footpaths.push((integer(input)?, integer(input)?, integer(input)?));
    }
    let mut trailing = [0];
    ensure!(input.read(&mut trailing)? == 0, "trailing snapshot bytes");
    preprocess::prepare(
        RawTimetable {
            coordinates: Coordinates::ProjectedMeters,
            stops,
            trips,
            footpaths,
        },
        &Preprocessing {
            walking_radius_m: 0.,
            default_transfer_seconds: 0.,
            use_explicit_transfers: true,
            ..Default::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_bad_header_and_truncation() {
        assert!(decode(&mut &b"BADMAGIC"[..]).is_err());
        assert!(decode(&mut &b"MMJNI001\0\0\0\x01"[..]).is_err());
    }
}
