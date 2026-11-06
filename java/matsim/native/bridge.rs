//! Bridges MATSim snapshots and numeric queries to shared immutable native routing data.
use crate::boundary;
mod snapshot;

use anyhow::{Context, Result, anyhow, ensure};
use jni::{
    JNIEnv,
    objects::{JClass, JIntArray, JString},
    sys::{jdoubleArray, jint, jlong},
};
use multimodal_matsim_router::{
    cost::CostModel,
    model::Timetable,
    routing::Leg,
    weighted::{self, Endpoint, Query, Workspace},
};
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        Arc, OnceLock, RwLock,
        atomic::{AtomicI64, Ordering},
    },
};

struct Router {
    timetable: Timetable,
    costs: CostModel,
}
static ROUTERS: OnceLock<RwLock<HashMap<i64, Arc<Router>>>> = OnceLock::new();
static NEXT: AtomicI64 = AtomicI64::new(1);
thread_local! { static WORKSPACE: RefCell<Workspace> = RefCell::default(); }
fn routers() -> &'static RwLock<HashMap<i64, Arc<Router>>> {
    ROUTERS.get_or_init(Default::default)
}

// SAFETY: These names and system ABI correspond exactly to NativePtRouter's static native methods.
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_eqasim_switzerland_ch_1cmdp_rustRouting_NativePtRouter_open(
    mut env: JNIEnv,
    _: JClass,
    file: JString,
    costs: JString,
) -> jlong {
    boundary(&mut env, |env| {
        let file: String = env.get_string(&file)?.into();
        let costs: String = env.get_string(&costs)?.into();
        let timetable = snapshot::read(&file)?;
        let mut value: serde_json::Value =
            serde_json::from_str(&costs).context("invalid MATSim routing costs")?;
        let search = value
            .as_object_mut()
            .and_then(|v| v.remove("_search"))
            .map(serde_json::from_value)
            .transpose()
            .context("invalid MATSim search options")?
            .unwrap_or_default();
        let settings = serde_json::from_value(value).context("invalid MATSim routing costs")?;
        let costs = CostModel::with_search_options(&timetable, &settings, search)?;
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        ensure!(id > 0, "native router handle overflow");
        routers()
            .write()
            .map_err(|_| anyhow!("router registry poisoned"))?
            .insert(id, Arc::new(Router { timetable, costs }));
        Ok(id)
    })
}

fn endpoints(env: &JNIEnv, array: JIntArray) -> Result<Vec<Endpoint>> {
    let length = env.get_array_length(&array)? as usize;
    ensure!(
        length.is_multiple_of(2),
        "endpoint array must contain stop/duration pairs"
    );
    let mut values = vec![0; length];
    env.get_int_array_region(&array, 0, &mut values)?;
    values
        .as_chunks::<2>()
        .0
        .iter()
        .map(|v| {
            ensure!(v[0] >= 0 && v[1] >= 0, "negative endpoint");
            Ok(Endpoint {
                stop: v[0] as u32,
                duration: v[1] as u32,
            })
        })
        .collect()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_eqasim_switzerland_ch_1cmdp_rustRouting_NativePtRouter_route(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    departure: jlong,
    boardings: jint,
    duration: jlong,
    access: JIntArray,
    egress: JIntArray,
) -> jdoubleArray {
    boundary(&mut env, |env| {
        let router = routers()
            .read()
            .map_err(|_| anyhow!("router registry poisoned"))?
            .get(&handle)
            .cloned()
            .context("native PT router is closed")?;
        let query = Query {
            departure: departure.try_into()?,
            max_boardings: boardings.try_into()?,
            max_duration: duration.try_into()?,
            access: endpoints(env, access)?,
            egress: endpoints(env, egress)?,
        };
        let result = WORKSPACE.with(|w| {
            weighted::route(
                &router.timetable,
                &query,
                &router.costs,
                &mut w.borrow_mut(),
            )
        })?;
        // Empty array is an ordinary no-path result, never a Java exception.
        let mut output = Vec::new();
        if let Some(result) = result {
            output.extend([
                result
                    .journey
                    .generalized_cost
                    .context("missing weighted cost")?,
                result.access.stop as f64,
                result.access.duration as f64,
                result.egress.stop as f64,
                result.egress.duration as f64,
            ]);
            for leg in result.journey.legs {
                let values = match leg {
                    Leg::Walk {
                        from,
                        to,
                        departure,
                        arrival,
                    } => [
                        0.,
                        -1.,
                        from as f64,
                        to as f64,
                        departure as f64,
                        arrival as f64,
                    ],
                    Leg::Transit {
                        trip,
                        from,
                        to,
                        departure,
                        arrival,
                    } => [
                        1.,
                        trip as f64,
                        from as f64,
                        to as f64,
                        departure as f64,
                        arrival as f64,
                    ],
                };
                output.extend(values);
            }
        }
        let array = env.new_double_array(output.len().try_into()?)?;
        env.set_double_array_region(&array, 0, &output)?;
        Ok(array.into_raw())
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_eqasim_switzerland_ch_1cmdp_rustRouting_NativePtRouter_close(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    boundary(&mut env, |_| {
        // An in-flight query holds its own Arc; closing cannot free memory under that query.
        routers()
            .write()
            .map_err(|_| anyhow!("router registry poisoned"))?
            .remove(&handle);
        Ok(())
    });
}
