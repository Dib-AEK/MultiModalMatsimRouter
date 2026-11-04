//! Defines source-neutral import records and the validated compact timetable consumed by routing.
use crate::time::{NEVER, Time};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
pub type StopId = u32;
pub type TripId = u32;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Coordinates {
    Geographic,
    ProjectedMeters,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stop {
    pub id: String,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub transfer_buffer: Time,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trip {
    pub id: String,
    pub line: String,
    pub route: String,
    pub departure: String,
    pub mode: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Call {
    pub stop: StopId,
    pub arrival: Time,
    pub departure: Time,
    pub pickup: bool,
    pub dropoff: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawTrip {
    pub trip: Trip,
    pub calls: Vec<Call>,
}
#[derive(Debug, Clone)]
pub struct RawTimetable {
    pub coordinates: Coordinates,
    pub stops: Vec<Stop>,
    pub trips: Vec<RawTrip>,
    pub footpaths: Vec<(StopId, StopId, Time)>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Connection {
    pub from: StopId,
    pub to: StopId,
    pub departure: Time,
    /// Vehicle arrival at the boarding stop: onboard dwell has in-vehicle, not waiting, cost.
    pub boarding_arrival: Time,
    pub arrival: Time,
    pub trip: TripId,
    pub pickup: bool,
    pub dropoff: bool,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Footpath {
    pub to: StopId,
    pub duration: Time,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timetable {
    pub coordinates: Coordinates,
    pub stops: Vec<Stop>,
    pub trips: Vec<Trip>,
    pub connections: Vec<Connection>,
    pub walk_offsets: Vec<u32>,
    pub walks: Vec<Footpath>,
    pub components: Vec<u32>,
    pub cells: Vec<u16>,
    pub reachability: Vec<u64>,
    pub cell_words: usize,
}
impl Timetable {
    pub fn walking_from(&self, stop: StopId) -> &[Footpath] {
        &self.walks[self.walk_offsets[stop as usize] as usize
            ..self.walk_offsets[stop as usize + 1] as usize]
    }
    pub fn may_reach(&self, from: StopId, target: StopId) -> bool {
        self.cell_words == 0 || {
            let cell = self.cells[target as usize] as usize;
            self.reachability[from as usize * self.cell_words + cell / 64] & (1_u64 << (cell % 64))
                != 0
        }
    }
    pub fn validate(&self) -> Result<()> {
        let n = self.stops.len();
        ensure!(
            self.connections.len() < u32::MAX as usize && self.trips.len() < u32::MAX as usize,
            "timetable exceeds compact index limits"
        );
        ensure!(
            n > 0 && n < u32::MAX as usize,
            "timetable needs a nonempty, addressable stop set"
        );
        let mut ids = std::collections::HashSet::new();
        for s in &self.stops {
            ensure!(
                !s.id.is_empty() && ids.insert(&s.id),
                "empty/duplicate stop ID: {}",
                s.id
            );
            ensure!(
                s.x.is_finite() && s.y.is_finite() && s.transfer_buffer < NEVER,
                "invalid stop {}",
                s.id
            );
            if self.coordinates == Coordinates::Geographic {
                ensure!(
                    s.x.abs() <= 180.0 && s.y.abs() <= 90.0,
                    "invalid geographic coordinate: {}",
                    s.id
                );
            }
        }
        ids.clear();
        for t in &self.trips {
            ensure!(
                !t.id.is_empty() && ids.insert(&t.id),
                "empty/duplicate trip ID: {}",
                t.id
            );
        }
        ensure!(
            self.walk_offsets.len() == n + 1
                && self.walk_offsets[0] == 0
                && self.walk_offsets[n] as usize == self.walks.len()
                && self.walk_offsets.windows(2).all(|p| p[0] <= p[1]),
            "invalid walking CSR offsets"
        );
        ensure!(
            self.walks
                .iter()
                .all(|w| (w.to as usize) < n && w.duration < NEVER),
            "invalid walking edge"
        );
        ensure!(
            self.connections
                .windows(2)
                .all(|p| p[0].departure <= p[1].departure),
            "unsorted connections"
        );
        ensure!(
            self.components.len() == n && self.components.iter().all(|&c| (c as usize) < n),
            "invalid component index"
        );
        let mut previous = vec![None; self.trips.len()];
        for c in &self.connections {
            ensure!(
                (c.from as usize) < n
                    && (c.to as usize) < n
                    && (c.trip as usize) < self.trips.len()
                    && c.departure <= c.arrival
                    && c.boarding_arrival <= c.departure
                    && c.arrival < NEVER,
                "invalid connection"
            );
        }
        for c in &self.connections {
            ensure!(
                self.components[c.from as usize] == self.components[c.to as usize],
                "component index cuts a transit edge"
            );
            if let Some((stop, arrival)) = previous[c.trip as usize] {
                ensure!(
                    stop == c.from && arrival <= c.departure,
                    "disconnected or non-monotonic trip connections"
                );
            }
            previous[c.trip as usize] = Some((c.to, c.arrival));
        }
        for from in 0..n {
            for edge in self.walking_from(from as u32) {
                ensure!(
                    self.components[from] == self.components[edge.to as usize],
                    "component index cuts a walking edge"
                );
            }
        }
        ensure!(
            self.cell_words <= 1024
                && (self.cell_words == 0
                    || (self.cells.len() == n
                        && self.reachability.len() == n.saturating_mul(self.cell_words)
                        && self
                            .cells
                            .iter()
                            .all(|&c| (c as usize) < self.cell_words * 64))),
            "invalid reachability index"
        );
        Ok(())
    }
}
