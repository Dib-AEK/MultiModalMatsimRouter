//! Resolves client inputs and MATSim XML settings without putting host configuration in the algorithm.
use anyhow::{Context, Result, bail, ensure};
use multimodal_matsim_router::{
    config::{Cache, Config, Execution, Input, Objective, Preprocessing, Routing},
    cost::{Costs, ModeTransfer},
    engine::Engine,
    model::Coordinates,
    preprocess, readers, spatial, time,
};
use quick_xml::{Reader, events::Event};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
};

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    #[default]
    Auto,
    Config,
    MatsimConfig,
    Matsim,
    Gtfs,
    Binary,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub search_radius_m: f64,
    pub extension_radius_m: f64,
    pub direct_walk_factor: f64,
    pub distance_factor: f64,
    pub transfer_walk_margin_s: f64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            search_radius_m: 1000.,
            extension_radius_m: 200.,
            direct_walk_factor: 1.,
            distance_factor: 1.,
            transfer_walk_margin_s: 5.,
        }
    }
}
type Params = BTreeMap<String, String>;
struct Set {
    first_child: usize,
    module: String,
    kind: String,
    values: Params,
    parents: Vec<Params>,
}
#[derive(Default)]
struct XmlConfig {
    modules: BTreeMap<String, Params>,
    sets: Vec<Set>,
}
fn attrs(e: &quick_xml::events::BytesStart) -> Result<Params> {
    e.attributes()
        .map(|a| {
            let a = a?;
            Ok((
                a.key.as_ref().to_owned(),
                a.normalized_value(quick_xml::XmlVersion::Implicit1_0)?
                    .into_owned(),
            ))
        })
        .collect()
}
fn parse(path: &Path) -> Result<XmlConfig> {
    let mut r = Reader::from_reader(readers::xml_input(path)?);
    let mut buffer = Vec::new();
    let mut out = XmlConfig::default();
    let mut module = String::new();
    let mut stack: Vec<Set> = Vec::new();
    loop {
        match r
            .read_event_into(&mut buffer)
            .context("reading MATSim config")?
        {
            event @ (Event::Start(_) | Event::Empty(_)) => {
                let empty = matches!(event, Event::Empty(_));
                let e = match event {
                    Event::Start(e) | Event::Empty(e) => e,
                    _ => unreachable!(),
                };
                let name = e.name().as_ref().to_owned();
                let a = attrs(&e)?;
                match e.name().as_ref() {
                    "module" => {
                        module = a.get("name").context("module name")?.clone();
                    }
                    "parameterset" => stack.push(Set {
                        first_child: out.sets.len(),
                        module: module.clone(),
                        kind: a.get("type").context("parameter set type")?.clone(),
                        values: Params::new(),
                        parents: stack.iter().map(|s| s.values.clone()).collect(),
                    }),
                    "param" => {
                        let key = a.get("name").context("param name")?.clone();
                        let value = a.get("value").context("param value")?.clone();
                        if let Some(s) = stack.last_mut() {
                            s.values.insert(key, value);
                        } else {
                            out.modules
                                .entry(module.clone())
                                .or_default()
                                .insert(key, value);
                        }
                    }
                    _ => (),
                }
                if empty && name == "parameterset" {
                    finish_set(&mut stack, &mut out)?;
                }
            }
            Event::End(e) if e.name().as_ref() == "parameterset" => {
                finish_set(&mut stack, &mut out)?;
            }
            Event::Eof => break,
            _ => (),
        }
        buffer.clear();
    }
    ensure!(stack.is_empty(), "unclosed parameter set");
    Ok(out)
}
fn finish_set(stack: &mut Vec<Set>, out: &mut XmlConfig) -> Result<()> {
    let set = stack.pop().context("unbalanced parameter sets")?;
    let depth = stack.len();
    for child in &mut out.sets[set.first_child..] {
        if child.parents.len() > depth {
            child.parents[depth] = set.values.clone();
        }
    }
    out.sets.push(set);
    Ok(())
}
fn number(p: &Params, key: &str, default: f64) -> Result<f64> {
    p.get(key).map_or(Ok(default), |v| {
        v.parse().with_context(|| format!("invalid {key}"))
    })
}
fn module(x: &XmlConfig, key: &str) -> Params {
    x.modules.get(key).cloned().unwrap_or_default()
}
fn default_population(s: &Set) -> bool {
    s.parents.iter().all(|p| {
        p.get("subpopulation")
            .is_none_or(|v| v == "null" || v.is_empty())
    })
}
fn matsim(path: &Path) -> Result<(Config, Settings)> {
    let x = parse(path)?;
    let swiss = module(&x, "swissRailRaptor");
    let transit = module(&x, "transit");
    for key in [
        "useCapacityConstraints",
        "useRangeQuery",
        "useIntermodalAccessEgress",
        "useModeMappingForPassengers",
    ] {
        ensure!(
            swiss.get(key).is_none_or(|v| v == "false"),
            "MATSim {key} is unsupported by static routing"
        );
    }
    ensure!(
        swiss
            .get("scoringParameters")
            .is_none_or(|v| v == "Default"),
        "individual MATSim scoring is unsupported"
    );
    ensure!(
        transit.get("useTransit").is_none_or(|v| v == "true"),
        "MATSim transit is disabled"
    );
    let network = module(&x, "network");
    ensure!(
        network
            .get("timeVariantNetwork")
            .is_none_or(|v| v == "false"),
        "time-variant network is unsupported"
    );
    let routing = module(&x, "routing");
    ensure!(
        !routing
            .get("networkModes")
            .is_some_and(|m| m.split(',').any(|v| v.trim() == "walk")),
        "network walking is unsupported"
    );
    let schedule = transit
        .get("transitScheduleFile")
        .filter(|v| !v.is_empty() && v.as_str() != "null")
        .context("MATSim config requires transit.transitScheduleFile")?;
    ensure!(
        !schedule.contains("://"),
        "MATSim schedule must be a local file path"
    );
    let schedule = Path::new(schedule);
    let schedule = if schedule.is_absolute() {
        schedule.to_owned()
    } else {
        path.parent().unwrap_or(Path::new(".")).join(schedule)
    };
    let mut walk = Params::new();
    for mode in ["transit_walk", "non_network_walk", "walk"] {
        if let Some(s) = x.sets.iter().find(|s| {
            s.module == "routing"
                && (s.kind == "teleportedModeParameters" || s.kind == "modeRoutingParams")
                && s.values.get("mode").is_some_and(|v| v == mode)
        }) {
            walk = s.values.clone();
            break;
        }
    }
    ensure!(
        walk.get("teleportedModeFreespeedFactor")
            .is_none_or(|v| v == "null"),
        "freespeed-based walking is unsupported"
    );
    let factor = number(
        &walk,
        "beelineDistanceFactor",
        number(&routing, "beelineDistanceFactor", 1.3)?,
    )?;
    let speed = number(&walk, "teleportedModeSpeed", 1.3)? / factor;
    ensure!(
        speed.is_finite() && speed > 0. && factor.is_finite() && factor > 0.,
        "invalid MATSim walking speed/factor"
    );
    let tr = module(&x, "transitRouter");
    let global = module(&x, "global");
    let crs = global
        .get("coordinateSystem")
        .map(|s| s.to_ascii_uppercase().replace(':', ""))
        .unwrap_or_default();
    let coordinates = if crs == "EPSG4326" || crs == "WGS84" {
        Coordinates::Geographic
    } else {
        Coordinates::ProjectedMeters
    };
    let costs = if x.modules.contains_key("eqasim:raptor") {
        readers::matsim_config::costs(path)?
    } else {
        generic_costs(&x, &swiss)?
    };
    let config = Config {
        input: Input::Matsim {
            path: schedule,
            coordinates,
        },
        preprocessing: Preprocessing {
            walking_radius_m: number(&tr, "maxBeelineWalkConnectionDistance", 100.)?,
            walking_speed_m_s: speed,
            default_transfer_seconds: number(&tr, "additionalTransferTime", 0.)?,
            ..Default::default()
        },
        routing: Routing {
            objective: Objective::GeneralizedCost,
            costs,
            max_boardings: 64,
            ..Default::default()
        },
        execution: Execution {
            threads: global.get("numberOfThreads").map_or(Ok(4), |v| v.parse())?,
            ..Default::default()
        },
        cache: Cache::default(),
    };
    let settings = Settings {
        search_radius_m: number(&tr, "searchRadius", 1000.)?,
        extension_radius_m: number(&tr, "extensionRadius", 200.)?,
        direct_walk_factor: number(&tr, "directWalkFactor", 1.)?,
        distance_factor: factor,
        transfer_walk_margin_s: number(&swiss, "transferWalkMargin", 5.)?,
    };
    for value in [
        settings.search_radius_m,
        settings.extension_radius_m,
        settings.direct_walk_factor,
        settings.transfer_walk_margin_s,
    ] {
        ensure!(
            value.is_finite() && value >= 0.,
            "invalid MATSim stop-search setting"
        );
    }
    Ok((config, settings))
}
fn generic_costs(x: &XmlConfig, swiss: &Params) -> Result<Costs> {
    let mut score = module(x, "scoring");
    if score.is_empty() {
        score = module(x, "planCalcScore");
    }
    for s in &x.sets {
        if (s.module == "scoring" || s.module == "planCalcScore")
            && s.kind == "scoringParameters"
            && s.values
                .get("subpopulation")
                .is_none_or(|v| v == "null" || v.is_empty())
        {
            score.extend(s.values.clone());
        }
    }
    let performing = number(&score, "performing", 6.)?;
    let rate = |mode: &str| -> Result<f64> {
        let values = x
            .sets
            .iter()
            .find(|s| {
                (s.module == "scoring" || s.module == "planCalcScore")
                    && s.kind == "modeParams"
                    && default_population(s)
                    && s.values.get("mode").is_some_and(|v| v == mode)
            })
            .map(|s| s.values.clone())
            .unwrap_or_default();
        Ok(performing - number(&values, "marginalUtilityOfTraveling_util_hr", -6.)?)
    };
    let dynamic = number(swiss, "transferPenaltyCostPerTravelTimeHour", 0.)?;
    let mut c = Costs {
        walk_per_hour: rate("walk")?,
        wait_per_hour: performing - number(&score, "waitingPt", performing - rate("pt")?)?,
        other_per_hour: rate("pt")?,
        transfer_base: if dynamic == 0. {
            -number(&score, "utilityOfLineSwitch", -1.)?
        } else {
            number(swiss, "transferPenaltyBaseCost", 0.)?
        },
        transfer_per_travel_hour: dynamic,
        transfer_min: number(swiss, "transferPenaltyMinCost", f64::NEG_INFINITY)?,
        transfer_max: number(swiss, "transferPenaltyMaxCost", f64::INFINITY)?,
        ..Default::default()
    };
    for s in &x.sets {
        if s.module == "swissRailRaptor" && s.kind == "modeToModeTransferPenalty" {
            c.mode_transfers.push(ModeTransfer {
                from: s.values.get("fromMode").context("fromMode")?.clone(),
                to: s.values.get("toMode").context("toMode")?.clone(),
                cost: number(&s.values, "transferPenalty", 0.)?,
            });
        }
    }
    c.validate()?;
    Ok(c)
}
fn merge(base: &mut toml::Value, patch: toml::Value) {
    match (base, patch) {
        (toml::Value::Table(a), toml::Value::Table(b)) => {
            for (k, v) in b {
                if let Some(a) = a.get_mut(&k) {
                    merge(a, v)
                } else {
                    a.insert(k, v);
                }
            }
        }
        (a, b) => *a = b,
    }
}
pub fn load(path: &str, format: Format, parameters: Value) -> Result<(Config, Settings, bool)> {
    let path = Path::new(path);
    let mut p = parameters.as_object().cloned().unwrap_or_default();
    ensure!(
        parameters.is_null() || parameters.is_object(),
        "parameters must be an object"
    );
    let format = match format {
        Format::Auto => {
            if path.extension().is_some_and(|v| v == "toml") {
                Format::Config
            } else if path.is_dir() || path.extension().is_some_and(|v| v == "zip") {
                Format::Gtfs
            } else if path.extension().is_some_and(|v| v == "mmrt") {
                Format::Binary
            } else {
                let mut reader = Reader::from_reader(readers::xml_input(path)?);
                let mut bytes = Vec::new();
                loop {
                    match reader.read_event_into(&mut bytes)? {
                        Event::Start(e) | Event::Empty(e) => {
                            break match e.name().as_ref() {
                                "config" => Format::MatsimConfig,
                                "transitSchedule" => Format::Matsim,
                                _ => bail!("XML root must be config or transitSchedule"),
                            };
                        }
                        Event::Eof => bail!("empty input file"),
                        _ => (),
                    }
                    bytes.clear();
                }
            }
        }
        v => v,
    };
    let is_matsim_config = matches!(format, Format::MatsimConfig);
    let (mut config, mut settings) = match format {
        Format::Config => (Config::load(path)?, Settings::default()),
        Format::MatsimConfig => matsim(path)?,
        kind => {
            let coordinates = serde_json::from_value(
                p.remove("coordinates")
                    .unwrap_or(Value::String("projected_meters".into())),
            )?;
            let input = match kind {
                Format::Matsim => Input::Matsim {
                    path: path.to_owned(),
                    coordinates,
                },
                Format::Gtfs => Input::Gtfs {
                    path: path.to_owned(),
                    service_date: p
                        .remove("service_date")
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .context("GTFS loading requires service_date")?,
                },
                Format::Binary => Input::Binary {
                    path: path.to_owned(),
                },
                _ => bail!("invalid input format"),
            };
            (
                Config {
                    input,
                    preprocessing: Preprocessing::default(),
                    routing: Routing::default(),
                    execution: Execution::default(),
                    cache: Cache::default(),
                },
                Settings::default(),
            )
        }
    };
    let old_distance_factor = settings.distance_factor;
    if let Some(search) = p.remove("stop_search") {
        let mut s = toml::Value::try_from(&settings)?;
        merge(&mut s, serde_json::from_value(search)?);
        settings = s.try_into()?;
    }
    for v in [
        settings.search_radius_m,
        settings.extension_radius_m,
        settings.direct_walk_factor,
        settings.distance_factor,
        settings.transfer_walk_margin_s,
    ] {
        ensure!(v.is_finite() && v >= 0., "invalid stop-search setting");
    }
    ensure!(
        !p.contains_key("input"),
        "select the input using file/format, not parameters.input"
    );
    ensure!(
        settings.distance_factor > 0.,
        "walking distance factor must be positive"
    );
    // Import explicitly selected coefficient files before applying caller cost overrides.
    if let Some(import) = p
        .get("routing")
        .and_then(|v| v.get("matsim_config"))
        .and_then(Value::as_str)
    {
        let import = Path::new(import);
        let import = if import.is_absolute() {
            import.to_owned()
        } else {
            path.parent().unwrap_or(Path::new(".")).join(import)
        };
        config.routing.costs = readers::matsim_config::costs(&import)?;
    }
    let mut value = toml::Value::try_from(&config)?;
    let patch: toml::Value = serde_json::from_value(Value::Object(p))?;
    merge(&mut value, patch);
    config = value.try_into()?;
    config.preprocessing.walking_speed_m_s *= old_distance_factor / settings.distance_factor;
    if let Some(import) = config.routing.matsim_config.as_mut()
        && import.is_relative()
    {
        *import = path.parent().unwrap_or(Path::new(".")).join(&*import);
    }
    config.validate()?;
    Ok((config, settings, is_matsim_config))
}

/// Reproduce MATSim stop-pair walking/minimum times without route-stop transfer expansion.
pub fn engine(config: &Config, matsim: bool, self_times: &StopTimes) -> Result<Engine> {
    if !matsim {
        return Engine::load(config);
    }
    let Input::Matsim { path, coordinates } = &config.input else {
        bail!("MATSim config must provide a schedule");
    };
    let mut raw = readers::matsim::read(path, *coordinates)?;
    let neighbors = spatial::neighbors(
        &raw.stops,
        *coordinates,
        config.preprocessing.walking_radius_m,
    );
    let explicit = config.preprocessing.use_explicit_transfers;
    let mut edges = BTreeMap::new();
    for (a, b, t) in raw.footpaths.drain(..).filter(|_| explicit) {
        edges.insert((a, b), time::seconds((t as f64 / 1000.).ceil())?);
    }
    for stop in &mut raw.stops {
        stop.transfer_buffer = time::seconds(
            self_times
                .get(&stop.id)
                .copied()
                .filter(|_| explicit)
                .unwrap_or(config.preprocessing.default_transfer_seconds)
                .ceil(),
        )?;
    }
    for (from, targets) in neighbors.iter().enumerate() {
        for &to in targets {
            let d = spatial::distance(&raw.stops[from], &raw.stops[to], *coordinates)
                / config.preprocessing.walking_speed_m_s;
            edges
                .entry((from as u32, to as u32))
                .or_insert(time::seconds(
                    d.max(config.preprocessing.default_transfer_seconds).ceil(),
                )?);
        }
    }
    raw.footpaths = edges.into_iter().map(|((a, b), t)| (a, b, t)).collect();
    let settings = Preprocessing {
        walking_radius_m: 0.,
        default_transfer_seconds: 0.,
        use_explicit_transfers: true,
        ..config.preprocessing.clone()
    };
    Engine::from_timetable(config, preprocess::prepare(raw, &settings)?)
}
pub type StopTimes = HashMap<String, f64>;
pub fn schedule_options(path: &Path) -> Result<(StopTimes, StopTimes, StopTimes)> {
    let mut r = Reader::from_reader(readers::xml_input(path)?);
    let mut buffer = Vec::new();
    let (mut id, mut attribute) = (String::new(), String::new());
    let (mut same, mut access, mut egress) = (StopTimes::new(), StopTimes::new(), StopTimes::new());
    loop {
        match r.read_event_into(&mut buffer)? {
            event @ (Event::Start(_) | Event::Empty(_)) => {
                let empty = matches!(event, Event::Empty(_));
                let e = match event {
                    Event::Start(e) | Event::Empty(e) => e,
                    _ => unreachable!(),
                };
                let name = e.name().as_ref().to_owned();
                let a = attrs(&e)?;
                match e.name().as_ref() {
                    "stopFacility" => id = a.get("id").context("stop id")?.clone(),
                    "attribute" => attribute = a.get("name").cloned().unwrap_or_default(),
                    "relation" => {
                        if a.get("fromStop") == a.get("toStop") {
                            let v: f64 = a.get("transferTime").context("transferTime")?.parse()?;
                            same.insert(a.get("fromStop").context("fromStop")?.clone(), v);
                        }
                    }
                    "chainedDeparture" => bail!("chained PT departures are unsupported"),
                    _ => (),
                }
                if empty && name == "stopFacility" {
                    id.clear();
                }
                if empty && name == "attribute" {
                    attribute.clear();
                }
            }
            Event::Text(t)
                if !id.is_empty() && (attribute == "accessTime" || attribute == "egressTime") =>
            {
                let v: f64 = t.xml10_content().trim().parse()?;
                ensure!(v.is_finite() && v >= 0., "invalid stop access/egress time");
                if attribute == "accessTime" {
                    access.insert(id.clone(), v);
                } else {
                    egress.insert(id.clone(), v);
                }
            }
            Event::End(e) => match e.name().as_ref() {
                "stopFacility" => id.clear(),
                "attribute" => attribute.clear(),
                _ => (),
            },
            Event::Eof => break,
            _ => (),
        }
        buffer.clear();
    }
    Ok((same, access, egress))
}
