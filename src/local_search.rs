//! Shares boarding/continuation indices and merges only boardable trips in canonical connection order.
use crate::{model::*, time::Time};
use std::{cmp::Reverse, collections::BinaryHeap};
const NONE: u32 = u32::MAX;

pub(crate) struct IncomingWalk {
    pub from: usize,
    pub duration: Time,
    pub order: u32,
}
pub(crate) struct LocalIndex {
    boarding_offsets: Vec<usize>,
    boardings: Vec<u32>,
    following: Vec<u32>,
    incoming_offsets: Vec<usize>,
    incoming: Vec<IncomingWalk>,
}
impl LocalIndex {
    pub fn new(t: &Timetable) -> Self {
        let n = t.stops.len();
        let mut boarding_offsets = vec![0; n + 1];
        let mut incoming_offsets = vec![0; n + 1];
        for c in &t.connections {
            if c.pickup {
                boarding_offsets[c.from as usize + 1] += 1;
            }
        }
        for e in &t.walks {
            incoming_offsets[e.to as usize + 1] += 1;
        }
        for i in 0..n {
            boarding_offsets[i + 1] += boarding_offsets[i];
            incoming_offsets[i + 1] += incoming_offsets[i];
        }
        let mut positions = boarding_offsets[..n].to_vec();
        let mut boardings = vec![0; boarding_offsets[n]];
        let mut following = vec![NONE; t.connections.len()];
        let mut last = vec![NONE; t.trips.len()];
        for (i, c) in t.connections.iter().enumerate() {
            if c.pickup {
                let stop = c.from as usize;
                boardings[positions[stop]] = i as u32;
                positions[stop] += 1;
            }
            let prev = last[c.trip as usize];
            if prev != NONE {
                following[prev as usize] = i as u32;
            }
            last[c.trip as usize] = i as u32;
        }
        let mut incoming: Vec<_> = (0..t.walks.len())
            .map(|_| IncomingWalk {
                from: 0,
                duration: 0,
                order: 0,
            })
            .collect();
        positions.clone_from_slice(&incoming_offsets[..n]);
        for from in 0..n {
            for order in t.walk_offsets[from]..t.walk_offsets[from + 1] {
                let e = &t.walks[order as usize];
                let to = e.to as usize;
                incoming[positions[to]] = IncomingWalk {
                    from,
                    duration: e.duration,
                    order: order + 1,
                };
                positions[to] += 1;
            }
        }
        Self {
            boarding_offsets,
            boardings,
            following,
            incoming_offsets,
            incoming,
        }
    }
    pub fn boardings(&self, t: &Timetable, stop: usize, earliest: Time, horizon: Time) -> &[u32] {
        let ids = &self.boardings[self.boarding_offsets[stop]..self.boarding_offsets[stop + 1]];
        let first = ids.partition_point(|&i| t.connections[i as usize].departure < earliest);
        let last = ids.partition_point(|&i| t.connections[i as usize].departure <= horizon);
        &ids[first..last]
    }
    pub fn incoming(&self, stop: usize) -> &[IncomingWalk] {
        &self.incoming[self.incoming_offsets[stop]..self.incoming_offsets[stop + 1]]
    }
    pub fn allocated_bytes(&self) -> usize {
        (self.boarding_offsets.capacity() + self.incoming_offsets.capacity()) * size_of::<usize>()
            + (self.boardings.capacity() + self.following.capacity()) * size_of::<u32>()
            + self.incoming.capacity() * size_of::<IncomingWalk>()
    }
}
#[derive(Default)]
pub(crate) struct ConnectionScan {
    first: Vec<u32>,
    active: Vec<usize>,
    heap: BinaryHeap<Reverse<u32>>,
    dense: Option<usize>,
}
impl ConnectionScan {
    pub fn reset(&mut self, trips: usize) {
        for i in self.active.drain(..) {
            self.first[i] = NONE;
        }
        self.first.resize(trips, NONE);
        self.heap.clear();
        self.dense = None;
    }
    pub fn activate(&mut self, trip: usize, connection: u32) {
        if self.first[trip] == NONE {
            self.active.push(trip);
        }
        self.first[trip] = self.first[trip].min(connection);
    }
    pub fn start(&mut self) {
        self.heap
            .extend(self.active.iter().map(|&i| Reverse(self.first[i])));
    }
    pub fn start_dense(&mut self, first: usize) {
        self.dense = Some(first);
    }
    pub fn next(&mut self, index: &LocalIndex, count: usize) -> Option<usize> {
        if let Some(i) = self.dense.as_mut() {
            if *i >= count {
                return None;
            }
            let result = *i;
            *i += 1;
            return Some(result);
        }
        let Reverse(i) = self.heap.pop()?;
        let next = index.following[i as usize];
        if next != NONE {
            self.heap.push(Reverse(next));
        }
        Some(i as usize)
    }
}
