//! Precomputes admissible destination-cell potentials; geographic cells never restrict allowed journeys.
use crate::{
    model::Timetable,
    search_index::{BoundWorkspace, SearchIndex},
    weighted::Endpoint,
};
use rayon::prelude::*;

pub(crate) struct RegionalBounds {
    cell: Vec<usize>,
    rows: Vec<Vec<f32>>,
    blocks: Vec<Vec<f32>>,
    changes: Vec<Vec<f32>>,
    offsets: Vec<usize>,
}
impl RegionalBounds {
    pub fn build(
        t: &Timetable,
        index: &SearchIndex,
        count: usize,
        network: bool,
        blocks: bool,
        walk: f64,
    ) -> Self {
        let mut groups = Vec::new();
        let mut ids: Vec<_> = (0..t.stops.len()).collect();
        let mut graph = network.then(|| crate::network_partition::NetworkPartition::new(t));
        partition(
            t,
            &mut ids,
            count.min(t.stops.len()).max(1),
            &mut groups,
            &mut graph,
        );
        let mut cell = vec![0; t.stops.len()];
        for (i, group) in groups.iter().enumerate() {
            for &stop in group {
                cell[stop] = i;
            }
        }
        let build = |workspace: &mut BoundWorkspace, group: &Vec<usize>| {
            let targets: Vec<_> = group
                .iter()
                .map(|&stop| Endpoint {
                    stop: stop as u32,
                    duration: 0,
                })
                .collect();
            index.bounds(t.stops.len(), 0., &targets, &[], None, workspace);
            (0..t.stops.len())
                .map(|stop| {
                    let bound = workspace.get(stop as u32);
                    let value = bound as f32;
                    // Round toward minus infinity: even f32 potentials must remain lower bounds.
                    if bound.is_finite() && value as f64 > bound {
                        value.next_down()
                    } else {
                        value
                    }
                })
                .collect()
        };
        // Lazy cost initialization can occur inside a routing worker that holds a TLS borrow.
        // Nested Rayon work stealing there could re-enter another query on the same workspace.
        let rows: Vec<Vec<f32>> = if rayon::current_thread_index().is_some() {
            let mut workspace = BoundWorkspace::default();
            groups
                .iter()
                .map(|group| build(&mut workspace, group))
                .collect()
        } else {
            groups
                .par_iter()
                .map_init(BoundWorkspace::default, build)
                .collect()
        };
        let mut offsets = vec![0; t.stops.len() + 1];
        if blocks {
            for stop in 0..t.stops.len() {
                offsets[stop + 1] = offsets[stop] + index.transfers(stop).len().div_ceil(16);
            }
        }
        let suffix = if blocks {
            rows.iter()
                .map(|row: &Vec<f32>| {
                    let mut values = vec![f32::INFINITY; offsets[t.stops.len()]];
                    for stop in 0..t.stops.len() {
                        let mut lower = f64::INFINITY;
                        for (i, e) in index.transfers(stop).iter().enumerate().rev() {
                            lower = lower
                                .min(down(e.duration as f64 * walk + row[e.to as usize] as f64));
                            if i % 16 == 0 {
                                values[offsets[stop] + i / 16] = floor(lower);
                            }
                        }
                    }
                    values
                })
                .collect()
        } else {
            Vec::new()
        };
        let changes = if blocks {
            (0..index.mode_count())
                .map(|mode| {
                    let mut values = vec![f32::INFINITY; offsets[t.stops.len()]];
                    for stop in 0..t.stops.len() {
                        let mut lower = f64::INFINITY;
                        for (i, e) in index.transfers(stop).iter().enumerate().rev() {
                            lower = lower.min(index.minimum_change(e.to, mode));
                            if i % 16 == 0 {
                                values[offsets[stop] + i / 16] = floor(lower);
                            }
                        }
                    }
                    values
                })
                .collect()
        } else {
            Vec::new()
        };
        Self {
            cell,
            rows,
            changes,
            blocks: suffix,
            offsets,
        }
    }
    pub fn cell(&self, stop: u32) -> usize {
        self.cell[stop as usize]
    }
    pub fn lower(&self, stop: u32, targets: &[(usize, f64)]) -> f64 {
        targets
            .iter()
            .map(|&(cell, egress)| {
                let value = self.rows[cell][stop as usize] as f64 + egress;
                if value > 0. && value.is_finite() {
                    value.next_down()
                } else {
                    value
                }
            })
            .fold(f64::INFINITY, f64::min)
    }
    pub fn suffix(&self, stop: usize, position: usize, targets: &[(usize, f64)]) -> f64 {
        if self.blocks.is_empty() {
            return 0.0;
        }
        targets
            .iter()
            .map(|&(cell, egress)| {
                down(self.blocks[cell][self.offsets[stop] + position / 16] as f64 + egress)
            })
            .fold(f64::INFINITY, f64::min)
    }
    pub fn change_suffix(&self, stop: usize, position: usize, mode: Option<usize>) -> f64 {
        if self.changes.is_empty() {
            return 0.0;
        }
        mode.map_or(0.0, |m| {
            self.changes[m][self.offsets[stop] + position / 16] as f64
        })
    }
    pub fn allocated_bytes(&self) -> usize {
        self.changes
            .iter()
            .map(|r| r.capacity() * size_of::<f32>())
            .sum::<usize>()
            + self.offsets.capacity() * size_of::<usize>()
            + self
                .blocks
                .iter()
                .map(|v| v.capacity() * size_of::<f32>())
                .sum::<usize>()
            + self.cell.capacity() * size_of::<usize>()
            + self
                .rows
                .iter()
                .map(|r| r.capacity() * size_of::<f32>())
                .sum::<usize>()
    }
}
/// Balanced kd partitions follow stop density instead of forcing named city borders.
fn partition(
    t: &Timetable,
    ids: &mut [usize],
    count: usize,
    groups: &mut Vec<Vec<usize>>,
    graph: &mut Option<crate::network_partition::NetworkPartition>,
) {
    if count <= 1 || ids.len() <= 1 {
        groups.push(ids.to_vec());
        return;
    }
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for &i in ids.iter() {
        let s = &t.stops[i];
        min_x = min_x.min(s.x);
        max_x = max_x.max(s.x);
        min_y = min_y.min(s.y);
        max_y = max_y.max(s.y);
    }
    let x_axis = max_x - min_x >= max_y - min_y;
    ids.sort_unstable_by(|&a, &b| {
        let a_stop = &t.stops[a];
        let b_stop = &t.stops[b];
        let cmp = if x_axis {
            a_stop.x.total_cmp(&b_stop.x)
        } else {
            a_stop.y.total_cmp(&b_stop.y)
        };
        cmp.then_with(|| a.cmp(&b))
    });
    let left = count / 2;
    let split = (ids.len() * left / count).clamp(1, ids.len() - 1);
    let split = graph
        .as_mut()
        .map_or(split, |g| g.refine(ids, split, left, count - left));
    let (a, b) = ids.split_at_mut(split);
    partition(t, a, left, groups, graph);
    partition(t, b, count - left, groups, graph);
}

fn down(v: f64) -> f64 {
    if v > 0. && v.is_finite() {
        v.next_down()
    } else {
        v
    }
}
fn floor(v: f64) -> f32 {
    let f = v as f32;
    if v.is_finite() && f as f64 > v {
        f.next_down()
    } else {
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Preprocessing,
        cost::{CostModel, Costs, SearchOptions},
        model::*,
        preprocess,
        weighted::{self, Query, Workspace},
    };
    #[test]
    fn suffix_bounds_are_admissible_and_skip_uncompetitive_transfers() {
        let t = preprocess::prepare(
            RawTimetable {
                coordinates: Coordinates::ProjectedMeters,
                stops: (0..40)
                    .map(|i| Stop {
                        id: i.to_string(),
                        name: String::new(),
                        x: i as f64 * 1000.,
                        y: 0.,
                        transfer_buffer: 0,
                    })
                    .collect(),
                trips: (0..39)
                    .map(|i| {
                        let (from, dep, arr) = if i == 0 {
                            (0, 1000, 2000)
                        } else {
                            (i + 1, 1_000_000, 2_000_000)
                        };
                        RawTrip {
                            trip: Trip {
                                id: i.to_string(),
                                line: String::new(),
                                route: String::new(),
                                departure: String::new(),
                                mode: "bus".into(),
                            },
                            calls: vec![
                                Call {
                                    stop: from,
                                    arrival: dep,
                                    departure: dep,
                                    pickup: true,
                                    dropoff: true,
                                },
                                Call {
                                    stop: 1,
                                    arrival: arr,
                                    departure: arr,
                                    pickup: true,
                                    dropoff: true,
                                },
                            ],
                        }
                    })
                    .collect(),
                footpaths: (2..40).map(|i| (1, i, 0)).collect(),
            },
            &Preprocessing {
                walking_radius_m: 0.,
                ..Default::default()
            },
        )
        .unwrap();
        let costs = Costs::default();
        let yes = CostModel::with_search_options(
            &t,
            &costs,
            SearchOptions {
                network_partition: true,
                transfer_blocks: true,
                ..Default::default()
            },
        )
        .unwrap();
        let no = CostModel::with_search_options(
            &t,
            &costs,
            SearchOptions {
                network_partition: true,
                transfer_blocks: false,
                ..Default::default()
            },
        )
        .unwrap();
        let regional = yes.index.regional.as_ref().unwrap();
        for target in 0..40 {
            let targets = vec![(regional.cell(target), 0.1)];
            for pos in [0, 16, 32] {
                let expected = yes.index.transfers(1)[pos..]
                    .iter()
                    .map(|e| e.duration as f64 * yes.walk + regional.lower(e.to, &targets))
                    .fold(f64::INFINITY, f64::min);
                assert!(regional.suffix(1, pos, &targets) <= expected + 1e-12);
            }
        }
        let q = Query {
            departure: 0,
            max_boardings: 4,
            max_duration: 3_000_000,
            access: vec![Endpoint {
                stop: 0,
                duration: 0,
            }],
            egress: vec![Endpoint {
                stop: 1,
                duration: 0,
            }],
        };
        let mut workspace = Workspace::default();
        let a = weighted::route(&t, &q, &yes, &mut workspace)
            .unwrap()
            .unwrap();
        let b = weighted::route(&t, &q, &no, &mut workspace)
            .unwrap()
            .unwrap();
        assert_eq!(a.journey.generalized_cost, b.journey.generalized_cost);
        assert_eq!(a.journey.legs, b.journey.legs);
        assert!(
            a.journey.statistics.transfers_considered < b.journey.statistics.transfers_considered
        );
    }
}
