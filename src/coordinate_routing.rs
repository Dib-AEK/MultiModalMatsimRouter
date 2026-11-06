//! Coordinates bounded work and quality-triggered candidate expansion without altering utility coefficients.
use crate::{
    coordinate_index::CoordinateIndex,
    cost::CostModel,
    model::{Stop, Timetable},
    spatial,
    time::Time,
    weighted::{self, Query, ResultJourney, Workspace},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    /// Maximum scanned connections plus transfers per candidate search; 0 is exact.
    pub work_budget: usize,
    /// Expand a found journey when its largest pre-departure gap exceeds this; 0 disables.
    pub quality_wait_seconds: f64,
    /// Also expand journeys longer than this; 0 disables.
    pub quality_journey_seconds: f64,
    /// Quality expansion limit only; legacy no-path searches retain their five retries.
    pub quality_radius_m: f64,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            work_budget: 2_000_000,
            quality_wait_seconds: 1800.,
            quality_journey_seconds: 10800.,
            quality_radius_m: 2000.,
        }
    }
}
impl Policy {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            [
                self.quality_wait_seconds,
                self.quality_journey_seconds,
                self.quality_radius_m
            ]
            .iter()
            .all(|v| v.is_finite() && *v >= 0.),
            "invalid coordinate search policy"
        );
        Ok(())
    }
    pub fn legacy() -> Self {
        Self {
            work_budget: 0,
            quality_wait_seconds: 0.,
            quality_journey_seconds: 0.,
            quality_radius_m: 0.,
        }
    }
}
pub struct Request {
    pub origin: [f64; 2],
    pub destination: [f64; 2],
    pub departure: Time,
    pub max_boardings: usize,
    pub max_duration: Time,
    pub radius: f64,
    pub extension: f64,
    pub speed: f64,
    pub walk_per_second: f64,
    pub direct_factor: f64,
}
pub struct Outcome {
    pub journey: Option<ResultJourney>,
    pub attempts: usize,
}
fn poor(journey: &ResultJourney, p: Policy) -> bool {
    if p.quality_journey_seconds > 0.
        && (journey.journey.arrival - journey.journey.departure) as f64
            > p.quality_journey_seconds * 1000.
    {
        return true;
    }
    let mut ready = journey
        .journey
        .departure
        .saturating_add(journey.access.duration);
    for leg in &journey.journey.legs {
        if let crate::routing::Leg::Transit { departure, .. } = leg
            && p.quality_wait_seconds > 0.
            && departure.saturating_sub(ready) as f64 > p.quality_wait_seconds * 1000.
        {
            return true;
        }
        ready = match leg {
            crate::routing::Leg::Transit { arrival, .. }
            | crate::routing::Leg::Walk { arrival, .. } => *arrival,
        };
    }
    false
}
/// Returns actual feasible journeys. Budget exhaustion alone is never treated as no path.
pub fn route(
    t: &Timetable,
    index: &CoordinateIndex,
    costs: &CostModel,
    q: &Request,
    policy: Policy,
    w: &mut Workspace,
) -> Result<Outcome> {
    policy.validate()?;
    let mut radius = q.radius;
    let mut attempts = 0;
    let mut incomplete = false;
    let mut best: Option<ResultJourney> = None;
    for attempt in 0..5 {
        attempts += 1;
        let (access, a) = index.candidates(t, q.origin, radius, q.extension, q.speed, true)?;
        let (egress, e) =
            index.candidates(t, q.destination, radius, q.extension, q.speed, false)?;
        let explored = radius.max(a).max(e);
        let query = Query {
            departure: q.departure,
            max_boardings: q.max_boardings,
            max_duration: q.max_duration,
            access,
            egress,
        };
        // Probe a potentially poor stop set cheaply before spending the full search budget.
        // An acceptable truncated probe must still receive the full configured budget.
        let can_expand = attempt < 4
            && explored < policy.quality_radius_m
            && (policy.quality_wait_seconds > 0. || policy.quality_journey_seconds > 0.);
        let budget = if can_expand && policy.work_budget > 250_000 {
            250_000
        } else {
            policy.work_budget
        };
        let mut outcome = weighted::route_with_budget(t, &query, costs, w, budget)?;
        if budget != policy.work_budget
            && outcome.budget_reached
            && outcome.journey.as_ref().is_some_and(|j| !poor(j, policy))
        {
            outcome = weighted::route_with_budget(t, &query, costs, w, policy.work_budget)?;
        }
        incomplete |= outcome.budget_reached;
        if let Some(result) = outcome.journey {
            let take = best.as_ref().is_none_or(|old| {
                let a = result.journey.generalized_cost.unwrap();
                let b = old.journey.generalized_cost.unwrap();
                a < b
                    || (a == b
                        && (result.journey.boardings, result.journey.arrival)
                            <= (old.journey.boardings, old.journey.arrival))
            });
            if take {
                best = Some(result);
            }
        }
        let next = (2. * explored).max(explored + q.extension.max(1.));
        if let Some(result) = &best {
            if !poor(result, policy) || explored >= policy.quality_radius_m {
                break;
            }
            radius = next.min(policy.quality_radius_m);
        } else if outcome.budget_reached {
            if explored >= policy.quality_radius_m {
                break;
            }
            radius = next.min(policy.quality_radius_m);
        } else {
            radius = next;
        }
    }
    // Resolve uncertainty exactly rather than manufacturing a missing PT route.
    if best.is_none() && incomplete {
        let mut fallback = route(t, index, costs, q, Policy::legacy(), w)?;
        fallback.attempts += attempts;
        return Ok(fallback);
    }
    if let Some(result) = &best {
        let from = Stop {
            id: String::new(),
            name: String::new(),
            x: q.origin[0],
            y: q.origin[1],
            transfer_buffer: 0,
        };
        let to = Stop {
            x: q.destination[0],
            y: q.destination[1],
            ..from.clone()
        };
        let direct = spatial::distance(&from, &to, t.coordinates) / q.speed
            * q.walk_per_second
            * q.direct_factor;
        if direct < result.journey.generalized_cost.unwrap() {
            if result.search_truncated {
                let mut fallback = route(
                    t,
                    index,
                    costs,
                    q,
                    Policy {
                        work_budget: 0,
                        ..policy
                    },
                    w,
                )?;
                fallback.attempts += attempts;
                return Ok(fallback);
            }
            best = None;
        }
    }
    Ok(Outcome {
        journey: best,
        attempts,
    })
}
