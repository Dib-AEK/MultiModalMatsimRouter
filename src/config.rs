//! Loads strict TOML settings and resolves all file paths relative to the configuration file.
use crate::model::Coordinates;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "format", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
    Matsim {
        path: PathBuf,
        coordinates: Coordinates,
    },
    Gtfs {
        path: PathBuf,
        service_date: String,
    },
    Binary {
        path: PathBuf,
    },
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preprocessing {
    pub walking_radius_m: f64,
    pub walking_speed_m_s: f64,
    pub default_transfer_seconds: f64,
    pub use_explicit_transfers: bool,
    pub reachability_cells: usize,
}
impl Default for Preprocessing {
    fn default() -> Self {
        Self {
            walking_radius_m: 500.0,
            walking_speed_m_s: 1.3,
            default_transfer_seconds: 0.0,
            use_explicit_transfers: true,
            reachability_cells: 0,
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Routing {
    pub matsim_config: Option<PathBuf>,
    pub objective: Objective,
    pub costs: crate::cost::Costs,
    pub search: crate::cost::SearchOptions,
    pub max_boardings: usize,
    pub max_duration_seconds: f64,
    pub return_pareto: bool,
}
impl Default for Routing {
    fn default() -> Self {
        Self {
            matsim_config: None,
            objective: Objective::EarliestArrival,
            costs: crate::cost::Costs::default(),
            search: crate::cost::SearchOptions::default(),
            max_boardings: 5,
            max_duration_seconds: 108000.0,
            return_pareto: false,
        }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Objective {
    #[default]
    EarliestArrival,
    GeneralizedCost,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Execution {
    pub threads: usize,
    pub batch_size: usize,
    pub scheduling: crate::batching::Scheduling,
    pub work_estimate_capacity: usize,
}
impl Default for Execution {
    fn default() -> Self {
        Self {
            threads: 4,
            batch_size: 256,
            scheduling: crate::batching::Scheduling::default(),
            work_estimate_capacity: 65_536,
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cache {
    pub path: PathBuf,
    pub max_bytes: u64,
}
impl Default for Cache {
    fn default() -> Self {
        Self {
            path: "timetable.mmrt".into(),
            max_bytes: 2_147_483_648,
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub input: Input,
    #[serde(default)]
    pub preprocessing: Preprocessing,
    #[serde(default)]
    pub routing: Routing,
    #[serde(default)]
    pub execution: Execution,
    #[serde(default)]
    pub cache: Cache,
}
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let mut config: Self = toml::from_str(&text).context("parsing config")?;
        let directory = path.parent().unwrap_or(Path::new("."));
        let input_path = match &mut config.input {
            Input::Matsim { path, .. } | Input::Gtfs { path, .. } | Input::Binary { path } => path,
        };
        if input_path.is_relative() {
            *input_path = directory.join(&*input_path);
        }
        if config.cache.path.is_relative() {
            config.cache.path = directory.join(&config.cache.path);
        }
        if let Some(path) = &mut config.routing.matsim_config {
            if path.is_relative() {
                *path = directory.join(&*path);
            }
            config.routing.costs = crate::readers::matsim_config::costs(path)?;
        }
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        self.routing.costs.validate()?;
        ensure!(
            !self.routing.return_pareto || self.routing.objective == Objective::EarliestArrival,
            "arrival/boarding Pareto output is only supported with earliest_arrival"
        );
        let p = &self.preprocessing;
        ensure!(
            p.walking_radius_m.is_finite() && p.walking_radius_m >= 0.0,
            "walking radius must be finite and nonnegative"
        );
        ensure!(
            p.walking_speed_m_s.is_finite() && p.walking_speed_m_s > 0.0,
            "walking speed must be positive"
        );
        crate::time::seconds(p.default_transfer_seconds)?;
        crate::time::seconds(self.routing.max_duration_seconds)?;
        ensure!(
            p.reachability_cells <= 4096,
            "reachability_cells must be 0..4096"
        );
        ensure!(
            (1..=64).contains(&self.routing.max_boardings),
            "max_boardings must be 1..64"
        );
        ensure!(
            (1..=1024).contains(&self.execution.threads),
            "threads must be 1..1024"
        );
        ensure!(
            (1..=1_000_000).contains(&self.execution.batch_size),
            "batch_size must be 1..1000000"
        );
        ensure!(self.cache.max_bytes > 0, "cache max_bytes must be positive");
        Ok(())
    }
}
