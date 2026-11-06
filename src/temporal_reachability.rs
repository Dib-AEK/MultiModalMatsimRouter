//! Rejects difficult no-path searches using latest feasible PT boardings, without cost-frontier expansion.
use crate::{
    local_search::LocalIndex,
    model::Timetable,
    time::{NEVER, Time},
    weighted::Query,
};

/// Ignores only the boarding-count limit. Pickup/dropoff, stay-seated travel, transfer buffers,
/// one physical transfer walk and final egress are preserved. A negative answer is safe to prune.
#[derive(Default)]
pub(crate) struct Workspace {
    latest: Vec<Time>,
    boarding: Vec<Time>,
    access: Vec<Time>,
    touched: Vec<usize>,
    touched_access: Vec<usize>,
    touched_boarding: Vec<usize>,
    reached: Vec<bool>,
    touched_trips: Vec<usize>,
}
impl Workspace {
    fn update(&mut self, stop: usize, time: Time) -> bool {
        if self.latest[stop] == NEVER {
            self.touched.push(stop);
            self.latest[stop] = time;
            true
        } else if self.latest[stop] < time {
            self.latest[stop] = time;
            true
        } else {
            false
        }
    }
    fn before_boarding(&mut self, t: &Timetable, index: &LocalIndex, stop: usize, time: Time) {
        let buffer = t.stops[stop].transfer_buffer;
        if time >= buffer {
            self.update(stop, time - buffer);
        }
        for edge in index.incoming(stop) {
            if time >= edge.duration {
                self.update(edge.from, time - edge.duration);
            }
        }
    }
    pub fn may_reach(&mut self, t: &Timetable, index: &LocalIndex, q: &Query) -> bool {
        for i in self.touched.drain(..) {
            self.latest[i] = NEVER;
        }
        for i in self.touched_boarding.drain(..) {
            self.boarding[i] = NEVER;
        }
        for i in self.touched_access.drain(..) {
            self.access[i] = NEVER;
        }
        for i in self.touched_trips.drain(..) {
            self.reached[i] = false;
        }
        self.latest.resize(t.stops.len(), NEVER);
        self.boarding.resize(t.stops.len(), NEVER);
        self.access.resize(t.stops.len(), NEVER);
        self.reached.resize(t.trips.len(), false);
        for e in &q.access {
            let stop = e.stop as usize;
            let time = q.departure.saturating_add(e.duration);
            if self.access[stop] == NEVER {
                self.touched_access.push(stop);
                self.access[stop] = time;
            } else {
                self.access[stop] = self.access[stop].min(time);
            }
        }
        let horizon = q.departure.saturating_add(q.max_duration).min(NEVER - 1);
        for e in &q.egress {
            if e.duration <= q.max_duration {
                let time = horizon - e.duration;
                self.update(e.stop as usize, time);
                for edge in index.incoming(e.stop as usize) {
                    if time >= edge.duration {
                        self.update(edge.from, time - edge.duration);
                    }
                }
            }
        }
        let start = t.connections.partition_point(|c| c.departure < q.departure);
        let mut end = t.connections.partition_point(|c| c.departure <= horizon);
        while end > start {
            let time = t.connections[end - 1].departure;
            let mut begin = end - 1;
            while begin > start && t.connections[begin - 1].departure == time {
                begin -= 1;
            }
            loop {
                let mut changed = false;
                for c in &t.connections[begin..end] {
                    let latest = self.latest[c.to as usize];
                    if self.reached[c.trip as usize]
                        || (c.dropoff && latest != NEVER && c.arrival <= latest)
                    {
                        if !self.reached[c.trip as usize] {
                            self.touched_trips.push(c.trip as usize);
                            self.reached[c.trip as usize] = true;
                            changed = true;
                        }
                        if c.pickup {
                            let access = self.access[c.from as usize];
                            if access != NEVER && access <= time {
                                return true;
                            }
                            if self.boarding[c.from as usize] == NEVER {
                                self.touched_boarding.push(c.from as usize);
                                self.boarding[c.from as usize] = time;
                                self.before_boarding(t, index, c.from as usize, time);
                                changed = true;
                            }
                        }
                    }
                }
                if !changed {
                    break;
                }
            }
            end = begin;
        }
        false
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
        weighted::{self, Endpoint},
    };
    fn table(calls: Vec<Vec<(u32, u32)>>, walks: Vec<(u32, u32, u32)>) -> Timetable {
        preprocess::prepare(
            RawTimetable {
                coordinates: Coordinates::ProjectedMeters,
                stops: (0..4)
                    .map(|i| Stop {
                        id: i.to_string(),
                        name: String::new(),
                        x: i as f64 * 1000.,
                        y: 0.,
                        transfer_buffer: 0,
                    })
                    .collect(),
                trips: calls
                    .into_iter()
                    .enumerate()
                    .map(|(i, calls)| RawTrip {
                        trip: Trip {
                            id: i.to_string(),
                            line: String::new(),
                            route: String::new(),
                            departure: String::new(),
                            mode: "bus".into(),
                        },
                        calls: calls
                            .into_iter()
                            .map(|(stop, time)| Call {
                                stop,
                                arrival: time,
                                departure: time,
                                pickup: true,
                                dropoff: true,
                            })
                            .collect(),
                    })
                    .collect(),
                footpaths: walks,
            },
            &Preprocessing {
                walking_radius_m: 0.,
                ..Default::default()
            },
        )
        .unwrap()
    }
    fn query() -> Query {
        Query {
            departure: 0,
            max_duration: 1000,
            max_boardings: 4,
            access: vec![Endpoint {
                stop: 0,
                duration: 0,
            }],
            egress: vec![Endpoint {
                stop: 3,
                duration: 0,
            }],
        }
    }
    #[test]
    fn zero_time_batch_and_directional_walks_preserve_feasibility() {
        let t = table(
            vec![
                vec![(2, 100), (3, 100)],
                vec![(1, 100), (2, 100)],
                vec![(0, 100), (1, 100)],
            ],
            vec![],
        );
        let mut w = Workspace::default();
        assert!(w.may_reach(&t, &crate::local_search::LocalIndex::new(&t), &query()));
        let t = table(vec![vec![(1, 100), (3, 200)]], vec![(1, 0, 999)]);
        assert!(!w.may_reach(&t, &crate::local_search::LocalIndex::new(&t), &query()));
    }
    #[test]
    fn rejects_services_that_have_already_finished() {
        let t = table(vec![vec![(0, 100), (3, 200)]], vec![]);
        let mut q = query();
        q.departure = 300;
        assert!(!Workspace::default().may_reach(&t, &crate::local_search::LocalIndex::new(&t), &q));
    }
    #[test]
    fn relaxed_negative_answers_agree_with_reference_on_generated_timetables() {
        let mut seed = 6127u64;
        let mut random = |n: u32| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((seed >> 32) as u32) % n
        };
        let mut reachability = Workspace::default();
        let mut work = weighted::Workspace::default();
        for _ in 0..200 {
            let calls = (0..12)
                .map(|_| {
                    let a = random(4);
                    let b = random(4);
                    let depart = random(500);
                    let arrive = depart + random(200);
                    vec![(a, depart), (b, arrive)]
                })
                .collect();
            let t = table(calls, vec![]);
            let mut q = query();
            q.departure = random(600);
            q.max_duration = 100 + random(400);
            let model =
                CostModel::with_search_options(&t, &Costs::default(), SearchOptions::disabled())
                    .unwrap();
            let actual = weighted::route(&t, &q, &model, &mut work).unwrap();
            if actual.is_some() {
                assert!(reachability.may_reach(&t, &crate::local_search::LocalIndex::new(&t), &q));
            }
        }
    }
}
