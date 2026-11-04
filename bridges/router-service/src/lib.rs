//! Hosts immutable engines with versioned requests, ordered batch errors, and safe concurrent close.
use anyhow::{Context, Result, anyhow, ensure};
use multimodal_matsim_router::{config::Config, engine::Engine, routing::Workspace, weighted};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::HashMap,
    path::Path,
    sync::{
        Arc, OnceLock, RwLock,
        atomic::{AtomicI64, Ordering},
    },
};

pub const PROTOCOL_VERSION: i32 = 1;
struct Service {
    config: Config,
    engine: Engine,
}
static ENGINES: OnceLock<RwLock<HashMap<i64, Arc<Service>>>> = OnceLock::new();
static NEXT: AtomicI64 = AtomicI64::new(1);
thread_local! { static WORKSPACE: RefCell<Workspace> = RefCell::default(); }
fn engines() -> &'static RwLock<HashMap<i64, Arc<Service>>> {
    ENGINES.get_or_init(Default::default)
}

/// Load once; an in-flight request holds its own Arc even if another caller closes the handle.
pub fn open(path: &str) -> Result<i64> {
    let config = Config::load(Path::new(path))?;
    let engine = Engine::load(&config)?;
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    ensure!(id > 0, "router handle overflow");
    engines()
        .write()
        .map_err(|_| anyhow!("engine registry poisoned"))?
        .insert(id, Arc::new(Service { config, engine }));
    Ok(id)
}
pub fn close(id: i64) -> Result<()> {
    engines()
        .write()
        .map_err(|_| anyhow!("engine registry poisoned"))?
        .remove(&id);
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StopQuery {
    from: String,
    to: String,
    departure: String,
}
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Version,
    Open {
        config: String,
    },
    Close,
    Route {
        from: String,
        to: String,
        departure: String,
    },
    Batch {
        queries: Vec<StopQuery>,
    },
    WeightedBatch {
        queries: Vec<weighted::Query>,
    },
    Metadata {
        #[serde(default)]
        include_entities: bool,
    },
}
fn envelope(result: Result<Value>) -> Value {
    match result {
        Ok(value) => json!({"ok":true,"result":value}),
        Err(error) => json!({"ok":false,"error":format!("{error:#}")}),
    }
}
fn execute(id: i64, command: Value) -> Result<Value> {
    let operation: Operation = serde_json::from_value(command)?;
    match operation {
        Operation::Version => return Ok(json!(PROTOCOL_VERSION)),
        Operation::Open { config } => return Ok(json!(open(&config)?)),
        Operation::Close => {
            close(id)?;
            return Ok(Value::Null);
        }
        _ => (),
    }
    let service = engines()
        .read()
        .map_err(|_| anyhow!("engine registry poisoned"))?
        .get(&id)
        .cloned()
        .context("router is closed or handle is unknown")?;
    let engine = &service.engine;
    match operation {
        Operation::Route {
            from,
            to,
            departure,
        } => {
            let q = engine.query(&service.config, &from, &to, &departure)?;
            WORKSPACE.with(|w| {
                Ok(serde_json::to_value(
                    engine.route(&q, &mut w.borrow_mut())?,
                )?)
            })
        }
        Operation::Batch { queries } => {
            // Preserve per-item validation errors without preventing valid neighbors from routing.
            let parsed: Vec<_> = queries
                .iter()
                .map(|q| engine.query(&service.config, &q.from, &q.to, &q.departure))
                .collect();
            let valid: Vec<_> = parsed
                .iter()
                .filter_map(|q| q.as_ref().ok().cloned())
                .collect();
            let mut results = engine.batch(&valid).into_iter();
            Ok(Value::Array(
                parsed
                    .into_iter()
                    .map(|q| {
                        envelope(match q {
                            Ok(_) => results
                                .next()
                                .expect("one result per valid query")
                                .and_then(|r| Ok(serde_json::to_value(r)?)),
                            Err(e) => Err(e),
                        })
                    })
                    .collect(),
            ))
        }
        Operation::WeightedBatch { queries } => Ok(Value::Array(
            engine
                .weighted_batch(&queries)
                .into_iter()
                .map(|r| envelope(r.and_then(|r| Ok(serde_json::to_value(r)?))))
                .collect(),
        )),
        Operation::Metadata { include_entities } => {
            let t = &engine.timetable;
            let mut meta = json!({"protocol_version":PROTOCOL_VERSION,"stop_count":t.stops.len(),"trip_count":t.trips.len(),"connection_count":t.connections.len(),"objective":service.config.routing.objective,"threads":service.config.execution.threads});
            if include_entities {
                meta["stops"] = serde_json::to_value(&t.stops)?;
                meta["trips"] = serde_json::to_value(&t.trips)?;
            }
            Ok(meta)
        }
        _ => unreachable!(),
    }
}
/// Every request owns its response; errors are never stored in a shared last-error slot.
pub fn request(id: i64, json: &str) -> String {
    envelope(
        serde_json::from_str(json)
            .map_err(Into::into)
            .and_then(|v| execute(id, v)),
    )
    .to_string()
}
