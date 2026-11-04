//! Validates imports and compiles sorted connections, CSR footpaths, and optional conservative cell flags.
use crate::{config::Preprocessing, model::*, spatial, time};
use anyhow::{Context, Result, ensure};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashSet, VecDeque};

pub fn prepare(mut raw: RawTimetable, settings: &Preprocessing) -> Result<Timetable> {
    ensure!(
        !raw.stops.is_empty() && raw.stops.len() < u32::MAX as usize,
        "invalid stop count"
    );
    let n = raw.stops.len();
    let buffer = time::seconds(settings.default_transfer_seconds)?;
    for stop in &mut raw.stops {
        stop.transfer_buffer = if settings.use_explicit_transfers {
            stop.transfer_buffer.max(buffer)
        } else {
            buffer
        };
    }
    let mut timetable = Timetable {
        coordinates: raw.coordinates,
        stops: raw.stops,
        trips: Vec::new(),
        connections: Vec::new(),
        walk_offsets: vec![0; n + 1],
        walks: Vec::new(),
        components: (0..n as u32).collect(),
        cells: Vec::new(),
        reachability: Vec::new(),
        cell_words: 0,
    };
    timetable.validate()?;
    let mut seen = HashSet::new();
    for raw_trip in raw.trips {
        ensure!(
            seen.insert(raw_trip.trip.id.clone()),
            "duplicate trip {}",
            raw_trip.trip.id
        );
        ensure!(
            raw_trip.calls.len() >= 2,
            "trip {} has fewer than two stops",
            raw_trip.trip.id
        );
        for call in &raw_trip.calls {
            ensure!(
                (call.stop as usize) < n
                    && call.arrival <= call.departure
                    && call.departure < time::NEVER,
                "invalid stop or offsets in trip {}",
                raw_trip.trip.id
            );
        }
        let id: u32 = timetable.trips.len().try_into().context("too many trips")?;
        for pair in raw_trip.calls.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            ensure!(
                a.departure <= b.arrival,
                "non-monotonic times in trip {}",
                raw_trip.trip.id
            );
            timetable.connections.push(Connection {
                from: a.stop,
                to: b.stop,
                departure: a.departure,
                boarding_arrival: a.arrival,
                arrival: b.arrival,
                trip: id,
                pickup: a.pickup,
                dropoff: b.dropoff,
            });
        }
        timetable.trips.push(raw_trip.trip);
    }
    timetable.connections.sort_by_key(|c| (c.departure, c.trip));
    let mut edges = BTreeMap::new();
    for (from, to, duration) in raw
        .footpaths
        .into_iter()
        .filter(|_| settings.use_explicit_transfers)
    {
        ensure!(
            (from as usize) < n && (to as usize) < n && duration < time::NEVER,
            "invalid explicit footpath"
        );
        edges
            .entry((from, to))
            .and_modify(|d: &mut u32| *d = (*d).max(duration))
            .or_insert(duration);
    }
    if settings.walking_radius_m > 0.0 {
        let neighbors = spatial::neighbors(
            &timetable.stops,
            timetable.coordinates,
            settings.walking_radius_m,
        );
        for (from, targets) in neighbors.iter().enumerate() {
            for &to in targets {
                let duration = time::seconds(
                    spatial::distance(
                        &timetable.stops[from],
                        &timetable.stops[to],
                        timetable.coordinates,
                    ) / settings.walking_speed_m_s,
                )?;
                edges.entry((from as u32, to as u32)).or_insert(duration);
            }
        }
    }
    for ((from, to), duration) in edges {
        timetable.walk_offsets[from as usize + 1] += 1;
        timetable.walks.push(Footpath { to, duration });
    }
    for i in 1..=n {
        timetable.walk_offsets[i] = timetable.walk_offsets[i]
            .checked_add(timetable.walk_offsets[i - 1])
            .context("too many footpaths")?;
    }
    prepare_components(&mut timetable);
    if settings.reachability_cells > 0 {
        prepare_reachability(&mut timetable, settings.reachability_cells);
    }
    timetable.validate()?;
    Ok(timetable)
}

/// Reverse graph reachability is a safe, time-independent lower-information filter, not FLASH-TB.
fn prepare_reachability(t: &mut Timetable, requested: usize) {
    let n = t.stops.len();
    let count = requested.min(n);
    let mut order: Vec<_> = (0..n).collect();
    order.sort_by(|&a, &b| {
        t.stops[a]
            .x
            .total_cmp(&t.stops[b].x)
            .then(t.stops[a].y.total_cmp(&t.stops[b].y))
    });
    t.cells = vec![0; n];
    for (rank, &stop) in order.iter().enumerate() {
        t.cells[stop] = (rank * count / n) as u16;
    }
    let mut reverse = vec![Vec::new(); n];
    for c in &t.connections {
        reverse[c.to as usize].push(c.from);
    }
    for from in 0..n {
        for edge in t.walking_from(from as u32) {
            reverse[edge.to as usize].push(from as u32);
        }
    }
    let columns: Vec<Vec<bool>> = (0..count)
        .into_par_iter()
        .map(|cell| {
            let mut seen = vec![false; n];
            let mut queue = VecDeque::new();
            for (stop, &group) in t.cells.iter().enumerate() {
                if group as usize == cell {
                    seen[stop] = true;
                    queue.push_back(stop);
                }
            }
            while let Some(stop) = queue.pop_front() {
                for &prior in &reverse[stop] {
                    if !seen[prior as usize] {
                        seen[prior as usize] = true;
                        queue.push_back(prior as usize);
                    }
                }
            }
            seen
        })
        .collect();
    t.cell_words = count.div_ceil(64);
    t.reachability = vec![0; n * t.cell_words];
    for (cell, column) in columns.iter().enumerate() {
        for (stop, &reachable) in column.iter().enumerate() {
            if reachable {
                t.reachability[stop * t.cell_words + cell / 64] |= 1_u64 << (cell % 64);
            }
        }
    }
}

/// A cheap necessary condition for any journey, independent of time and boarding permissions.
fn prepare_components(t: &mut Timetable) {
    let n = t.stops.len();
    let mut parent: Vec<_> = (0..n).collect();
    fn root(parent: &mut [usize], mut node: usize) -> usize {
        while parent[node] != node {
            parent[node] = parent[parent[node]];
            node = parent[node];
        }
        node
    }
    fn join(parent: &mut [usize], a: usize, b: usize) {
        let a = root(parent, a);
        let b = root(parent, b);
        if a != b {
            parent[a.max(b)] = a.min(b);
        }
    }
    for c in &t.connections {
        join(&mut parent, c.from as usize, c.to as usize);
    }
    for from in 0..n {
        for edge in t.walking_from(from as u32) {
            join(&mut parent, from, edge.to as usize);
        }
    }
    t.components = (0..n).map(|node| root(&mut parent, node) as u32).collect();
}
