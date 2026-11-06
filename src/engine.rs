//! Orchestrates loading and shares immutable timetables across bounded parallel query workers.
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};
thread_local! { static WORKSPACE: RefCell<Workspace> = RefCell::default(); }
thread_local! { static WEIGHTED_WORKSPACE: RefCell<crate::weighted::Workspace> = RefCell::default(); }
use crate::{
    cache,
    config::{Config, Input},
    model::Timetable,
    preprocess, readers,
    routing::{self, Journey, Query, Workspace},
    time,
};
use anyhow::{Context, Result};
pub struct Engine {
    pub timetable: Arc<Timetable>,
    ids: HashMap<String, u32>,
    pool: rayon::ThreadPool,
    costs: OnceLock<Result<crate::cost::CostModel>>,
    cost_settings: crate::cost::Costs,
    search: crate::cost::SearchOptions,
    objective: crate::config::Objective,
    scheduling: crate::batching::Scheduling,
    work_history: Mutex<crate::work_history::WorkHistory>,
    work_estimate_capacity: usize,
}
impl Engine {
    pub fn load(config: &Config) -> Result<Self> {
        config.validate()?;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(config.execution.threads)
            .build()?;
        let timetable = match &config.input {
            Input::Binary { path } => cache::read(path, config.cache.max_bytes)?,
            source => {
                let raw = match source {
                    Input::Matsim { path, coordinates } => {
                        readers::matsim::read(path, *coordinates)?
                    }
                    Input::Gtfs { path, service_date } => readers::gtfs::read(
                        path,
                        service_date,
                        config.preprocessing.walking_speed_m_s,
                    )?,
                    Input::Binary { .. } => unreachable!(),
                };
                pool.install(|| preprocess::prepare(raw, &config.preprocessing))?
            }
        };
        Self::assemble(config, timetable, pool)
    }
    /// Accepts a prepared timetable, e.g. with a shared transfer graph in a differential benchmark.
    pub fn from_timetable(config: &Config, timetable: Timetable) -> Result<Self> {
        config.validate()?;
        timetable.validate()?;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(config.execution.threads)
            .build()?;
        Self::assemble(config, timetable, pool)
    }
    fn assemble(config: &Config, timetable: Timetable, pool: rayon::ThreadPool) -> Result<Self> {
        let ids = timetable
            .stops
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.clone(), i as u32))
            .collect();
        let costs = OnceLock::new();
        // Weighted engines prepare once during loading. Arrival-only users incur no weighted
        // index overhead; an explicit weighted query then initializes it once, including on races.
        if config.routing.objective == crate::config::Objective::GeneralizedCost {
            let model = crate::cost::CostModel::with_search_options(
                &timetable,
                &config.routing.costs,
                config.routing.search,
            )?;
            let _ = costs.set(Ok(model));
        }
        Ok(Self {
            timetable: Arc::new(timetable),
            ids,
            pool,
            costs,
            cost_settings: config.routing.costs.clone(),
            search: config.routing.search,
            objective: config.routing.objective,
            scheduling: config.execution.scheduling,
            work_history: Mutex::new(crate::work_history::WorkHistory::new(
                config.execution.work_estimate_capacity,
            )),
            work_estimate_capacity: config.execution.work_estimate_capacity,
        })
    }
    pub fn query(&self, config: &Config, from: &str, to: &str, departure: &str) -> Result<Query> {
        Ok(Query {
            from: *self
                .ids
                .get(from)
                .with_context(|| format!("unknown origin stop {from}"))?,
            to: *self
                .ids
                .get(to)
                .with_context(|| format!("unknown destination stop {to}"))?,
            departure: time::parse_time(departure)?,
            max_boardings: config.routing.max_boardings,
            return_pareto: config.routing.return_pareto,
            max_duration: time::seconds(config.routing.max_duration_seconds)?,
        })
    }
    pub fn route(&self, query: &Query, workspace: &mut Workspace) -> Result<Option<Journey>> {
        if self.objective == crate::config::Objective::GeneralizedCost {
            let query = crate::weighted::Query {
                departure: query.departure,
                max_boardings: query.max_boardings,
                max_duration: query.max_duration,
                access: vec![crate::weighted::Endpoint {
                    stop: query.from,
                    duration: 0,
                }],
                egress: vec![crate::weighted::Endpoint {
                    stop: query.to,
                    duration: 0,
                }],
            };
            WEIGHTED_WORKSPACE
                .with(|w| self.weighted_route(&query, &mut w.borrow_mut()))
                .map(|j| j.map(|j| j.journey))
        } else {
            routing::route(&self.timetable, query, workspace)
        }
    }
    pub fn weighted_route(
        &self,
        query: &crate::weighted::Query,
        workspace: &mut crate::weighted::Workspace,
    ) -> Result<Option<crate::weighted::ResultJourney>> {
        crate::weighted::route(&self.timetable, query, self.cost_model()?, workspace)
    }
    /// Prepares/borrows the immutable weighted model shared by all workers.
    pub fn cost_model(&self) -> Result<&crate::cost::CostModel> {
        self.costs
            .get_or_init(|| {
                crate::cost::CostModel::with_search_options(
                    &self.timetable,
                    &self.cost_settings,
                    self.search,
                )
            })
            .as_ref()
            .map_err(|error| anyhow::anyhow!("preparing weighted search: {error:#}"))
    }
    pub fn weighted_index_summary(&self) -> Option<crate::search_index::IndexSummary> {
        self.costs
            .get()?
            .as_ref()
            .ok()
            .map(|costs| costs.index_summary())
    }
    pub fn weighted_batch(
        &self,
        queries: &[crate::weighted::Query],
    ) -> Vec<Result<Option<crate::weighted::ResultJourney>>> {
        if self.scheduling == crate::batching::Scheduling::Auto && self.work_estimate_capacity > 0 {
            return self.auto_batch(queries, |i| {
                WEIGHTED_WORKSPACE.with(|w| self.weighted_route(&queries[i], &mut w.borrow_mut()))
            });
        }
        let policy = self.batch_policy();
        let estimates = self.weighted_estimates(queries, policy);
        crate::batching::execute(&self.pool, queries.len(), policy, &estimates, |i| {
            WEIGHTED_WORKSPACE.with(|w| self.weighted_route(&queries[i], &mut w.borrow_mut()))
        })
    }
    /// Work estimates from a previous corresponding batch affect execution order only.
    pub fn weighted_batch_with_estimates(
        &self,
        queries: &[crate::weighted::Query],
        estimates: &[u64],
    ) -> Result<Vec<Result<Option<crate::weighted::ResultJourney>>>> {
        anyhow::ensure!(
            queries.len() == estimates.len(),
            "one work estimate per query is required"
        );
        Ok(crate::batching::execute(
            &self.pool,
            queries.len(),
            crate::batching::Scheduling::EstimatedWork,
            estimates,
            |i| WEIGHTED_WORKSPACE.with(|w| self.weighted_route(&queries[i], &mut w.borrow_mut())),
        ))
    }
    /// Optional detailed diagnostics; auto scheduling records only lightweight cost hints normally.
    pub fn weighted_batch_profiled(
        &self,
        queries: &[crate::weighted::Query],
        estimates: Option<&[u64]>,
    ) -> Result<crate::batching::ProfiledBatch<Result<Option<crate::weighted::ResultJourney>>>>
    {
        let started = std::time::Instant::now();
        let mut history_keys = None;
        let mut history_hits = 0;
        let default;
        let (policy, estimates) = if let Some(estimates) = estimates {
            anyhow::ensure!(
                queries.len() == estimates.len(),
                "one work estimate per query is required"
            );
            (crate::batching::Scheduling::EstimatedWork, estimates)
        } else {
            if self.scheduling == crate::batching::Scheduling::Auto
                && self.work_estimate_capacity > 0
            {
                let keys = crate::work_history::fingerprints(queries);
                let (hints, hits) = self
                    .work_history
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .snapshot(&keys);
                let policy = Self::history_policy(hits, queries.len());
                default = hints;
                history_keys = Some(keys);
                history_hits = hits;
                (policy, default.as_slice())
            } else {
                let policy = self.batch_policy();
                default = self.weighted_estimates(queries, policy);
                (policy, default.as_slice())
            }
        };
        let mut batch =
            crate::batching::profile(&self.pool, queries.len(), policy, estimates, |i| {
                WEIGHTED_WORKSPACE.with(|w| self.weighted_route(&queries[i], &mut w.borrow_mut()))
            });
        if let Some(keys) = history_keys {
            let mut history = self.work_history.lock().unwrap_or_else(|e| e.into_inner());
            history.observe(&keys, &batch.metrics.query_nanoseconds);
            batch.metrics.advisory_history_hits = history_hits;
            batch.metrics.advisory_history_entries = history.summary().entries;
        }
        batch.metrics.total_seconds = started.elapsed().as_secs_f64();
        Ok(batch)
    }
    fn batch_policy(&self) -> crate::batching::Scheduling {
        match self.scheduling {
            crate::batching::Scheduling::Auto => crate::batching::Scheduling::Rayon,
            other => other,
        }
    }
    pub fn work_history_summary(&self) -> crate::work_history::HistorySummary {
        self.work_history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .summary()
    }
    fn history_policy(hits: usize, total: usize) -> crate::batching::Scheduling {
        if hits > 0 && hits >= total.div_ceil(2) {
            crate::batching::Scheduling::EstimatedWork
        } else {
            crate::batching::Scheduling::Rayon
        }
    }
    fn auto_batch<Q: std::hash::Hash, T: Send + Sync, F: Fn(usize) -> T + Sync>(
        &self,
        queries: &[Q],
        run: F,
    ) -> Vec<T> {
        if queries.is_empty() {
            return Vec::new();
        }
        let keys = crate::work_history::fingerprints(queries);
        let (hints, hits) = self
            .work_history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot(&keys);
        let records = crate::batching::execute(
            &self.pool,
            queries.len(),
            Self::history_policy(hits, queries.len()),
            &hints,
            |i| {
                let started = std::time::Instant::now();
                let result = run(i);
                (
                    result,
                    started.elapsed().as_nanos().min(u64::MAX as u128) as u64,
                )
            },
        );
        let mut results = Vec::with_capacity(records.len());
        let mut elapsed = Vec::with_capacity(records.len());
        for (result, cost) in records {
            results.push(result);
            elapsed.push(cost);
        }
        self.work_history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .observe(&keys, &elapsed);
        results
    }
    fn estimated_work(
        &self,
        from: Option<u32>,
        to: Option<u32>,
        departure: u32,
        duration: u32,
        boardings: usize,
    ) -> u64 {
        let distance = from
            .and_then(|s| self.timetable.stops.get(s as usize))
            .zip(to.and_then(|s| self.timetable.stops.get(s as usize)))
            .map_or(0.0, |(a, b)| {
                crate::spatial::distance(a, b, self.timetable.coordinates)
            });
        let first = self
            .timetable
            .connections
            .partition_point(|c| c.departure < departure);
        let last = self
            .timetable
            .connections
            .partition_point(|c| c.departure <= departure.saturating_add(duration));
        (last.saturating_sub(first) as u64)
            .saturating_mul(((distance / 1000.0) as u64).saturating_add(1))
            .saturating_mul(boardings.min(64) as u64)
    }
    fn weighted_estimates(
        &self,
        queries: &[crate::weighted::Query],
        policy: crate::batching::Scheduling,
    ) -> Vec<u64> {
        if policy != crate::batching::Scheduling::EstimatedWork {
            return Vec::new();
        }
        queries
            .iter()
            .map(|q| {
                self.estimated_work(
                    q.access.first().map(|e| e.stop),
                    q.egress.first().map(|e| e.stop),
                    q.departure,
                    q.max_duration,
                    q.max_boardings,
                )
            })
            .collect()
    }
    pub fn stop_id(&self, id: &str) -> Result<u32> {
        self.ids
            .get(id)
            .copied()
            .with_context(|| format!("unknown stop {id}"))
    }
    /// Workspaces are thread-local and reused by each pool worker; result order matches input.
    pub fn batch(&self, queries: &[Query]) -> Vec<Result<Option<Journey>>> {
        if self.scheduling == crate::batching::Scheduling::Auto
            && self.objective == crate::config::Objective::GeneralizedCost
            && self.work_estimate_capacity > 0
        {
            return self.auto_batch(queries, |i| {
                WORKSPACE.with(|w| self.route(&queries[i], &mut w.borrow_mut()))
            });
        }
        let policy = self.batch_policy();
        let estimates = if policy == crate::batching::Scheduling::EstimatedWork {
            queries
                .iter()
                .map(|q| {
                    self.estimated_work(
                        Some(q.from),
                        Some(q.to),
                        q.departure,
                        q.max_duration,
                        q.max_boardings,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        crate::batching::execute(&self.pool, queries.len(), policy, &estimates, |i| {
            WORKSPACE.with(|w| self.route(&queries[i], &mut w.borrow_mut()))
        })
    }
}
