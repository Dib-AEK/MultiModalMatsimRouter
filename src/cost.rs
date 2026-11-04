//! Defines explicit disutility coefficients and compiles string modes into compact routing indices.
use crate::{model::Timetable, time::Time};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Independent exact accelerations; disabling them provides a reference search for differential tests.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchOptions {
    pub destination_bounds: bool,
    pub cross_round_dominance: bool,
    pub sorted_transfers: bool,
    pub boarding_index: bool,
}
impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            destination_bounds: true,
            cross_round_dominance: true,
            sorted_transfers: true,
            boarding_index: true,
        }
    }
}
impl SearchOptions {
    pub fn disabled() -> Self {
        Self {
            destination_bounds: false,
            cross_round_dominance: false,
            sorted_transfers: false,
            boarding_index: false,
        }
    }
}

/// Positive disutility units (the negative of MATSim utility); time rates are per hour.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Costs {
    pub walk_per_hour: f64,
    pub wait_per_hour: f64,
    pub other_per_hour: f64,
    pub modes_per_hour: BTreeMap<String, f64>,
    pub transfer_base: f64,
    pub transfer_per_travel_hour: f64,
    pub transfer_min: f64,
    pub transfer_max: f64,
    pub mode_transfers: Vec<ModeTransfer>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModeTransfer {
    pub from: String,
    pub to: String,
    pub cost: f64,
}
impl Default for Costs {
    fn default() -> Self {
        Self {
            walk_per_hour: 1.0,
            wait_per_hour: 1.0,
            other_per_hour: 1.0,
            modes_per_hour: BTreeMap::new(),
            transfer_base: 0.0,
            transfer_per_travel_hour: 0.0,
            transfer_min: 0.0,
            transfer_max: f64::INFINITY,
            mode_transfers: Vec::new(),
        }
    }
}
impl Costs {
    pub fn validate(&self) -> Result<()> {
        for rate in [
            self.walk_per_hour,
            self.wait_per_hour,
            self.other_per_hour,
            self.transfer_per_travel_hour,
        ]
        .into_iter()
        .chain(self.modes_per_hour.values().copied())
        {
            ensure!(
                rate.is_finite() && rate >= 0.0,
                "cost rates must be finite and nonnegative"
            );
        }
        ensure!(
            self.transfer_base.is_finite(),
            "transfer_base must be finite"
        );
        ensure!(
            !self.transfer_min.is_nan()
                && !self.transfer_max.is_nan()
                && self.transfer_min <= self.transfer_max,
            "invalid transfer bounds"
        );
        ensure!(
            self.mode_transfers.is_empty() || self.transfer_per_travel_hour == 0.0,
            "SwissRailRaptor forbids time-dependent and mode-specific transfer penalties together"
        );
        let mut pairs = std::collections::HashSet::new();
        for p in &self.mode_transfers {
            ensure!(
                !p.from.is_empty()
                    && !p.to.is_empty()
                    && p.cost.is_finite()
                    && pairs.insert((&p.from, &p.to)),
                "invalid/duplicate mode transfer"
            );
        }
        // Nonnegative increments are needed for bounded-cost pruning and cycle dominance.
        for value in std::iter::once(self.transfer_base).chain(
            self.mode_transfers
                .iter()
                .map(|p| self.transfer_base + p.cost),
        ) {
            let clamped = value.clamp(self.transfer_min, self.transfer_max);
            ensure!(
                clamped.is_finite() && clamped >= 0.0,
                "effective transfer penalties must be finite and nonnegative"
            );
        }
        Ok(())
    }
}

/// Immutable compiled coefficients shared by every query worker.
pub struct CostModel {
    pub(crate) index: crate::search_index::SearchIndex,
    pub(crate) search: SearchOptions,
    pub(crate) walk: f64,
    pub(crate) wait: f64,
    pub(crate) max_time_rate: f64,
    pub(crate) min_time_rate: f64,
    pub(crate) trip_modes: Vec<usize>,
    pub(crate) rates: Vec<f64>,
    pub(crate) transfers: Vec<f64>,
    pub(crate) dynamic: bool,
    settings: Costs,
}
impl CostModel {
    pub fn new(t: &Timetable, settings: &Costs) -> Result<Self> {
        Self::with_search_options(t, settings, SearchOptions::default())
    }
    pub fn with_search_options(
        t: &Timetable,
        settings: &Costs,
        search: SearchOptions,
    ) -> Result<Self> {
        settings.validate()?;
        let mut modes = BTreeMap::new();
        let mut rates = Vec::new();
        let trip_modes: Vec<usize> = t
            .trips
            .iter()
            .map(|trip| {
                let next = modes.len();
                *modes.entry(trip.mode.clone()).or_insert_with(|| {
                    rates.push(
                        settings
                            .modes_per_hour
                            .get(&trip.mode)
                            .copied()
                            .unwrap_or(settings.other_per_hour)
                            / 3_600_000.0,
                    );
                    next
                })
            })
            .collect();
        let mut transfers = vec![
            settings
                .transfer_base
                .clamp(settings.transfer_min, settings.transfer_max);
            modes.len() * modes.len()
        ];
        for p in &settings.mode_transfers {
            if let (Some(&from), Some(&to)) = (modes.get(&p.from), modes.get(&p.to)) {
                transfers[from * modes.len() + to] = (settings.transfer_base + p.cost)
                    .clamp(settings.transfer_min, settings.transfer_max);
            }
        }
        let walk = settings.walk_per_hour / 3_600_000.0;
        let wait = settings.wait_per_hour / 3_600_000.0;
        let index = crate::search_index::SearchIndex::new(
            t,
            walk,
            &trip_modes,
            &rates,
            &transfers,
            &search,
        );
        Ok(Self {
            index,
            search,
            walk,
            wait,
            max_time_rate: rates.iter().copied().fold(wait, f64::max),
            min_time_rate: rates.iter().copied().fold(wait.min(walk), f64::min),
            trip_modes,
            rates,
            transfers,
            dynamic: settings.transfer_per_travel_hour > 0.0,
            settings: settings.clone(),
        })
    }
    pub fn index_summary(&self) -> crate::search_index::IndexSummary {
        self.index.summary()
    }
    pub(crate) fn transfer(&self, from: usize, to: usize) -> f64 {
        self.transfers[from * self.rates.len() + to]
    }
    pub(crate) fn total_dynamic_penalty(&self, transfers: usize, first: Time, last: Time) -> f64 {
        if !self.dynamic {
            return 0.0;
        }
        transfers as f64
            * (self.settings.transfer_base
                + self.settings.transfer_per_travel_hour * last.saturating_sub(first) as f64
                    / 3_600_000.0)
                .clamp(self.settings.transfer_min, self.settings.transfer_max)
    }
}
