//! Imports Eqasim/SwissRailRaptor routing coefficients without loading a Java simulation.
use crate::{
    cost::{Costs, ModeTransfer},
    readers::xml_input,
};
use anyhow::{Context, Result, ensure};
use quick_xml::{Reader, events::Event};
use std::{collections::BTreeMap, path::Path};

/// Imports the effective Eqasim overrides, preserving disutility units and transfer bounds.
/// Physical transfer/access settings remain explicit preprocessing/query inputs.
pub fn costs(path: &Path) -> Result<Costs> {
    let mut reader = Reader::from_reader(xml_input(path)?);
    let mut buffer = Vec::new();
    let mut module = String::new();
    let mut set: Option<BTreeMap<String, String>> = None;
    let mut modules: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut transfers = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .context("reading MATSim routing config")?
        {
            Event::Start(e) | Event::Empty(e) => {
                let attrs: BTreeMap<String, String> = e
                    .attributes()
                    .map(|a| {
                        let a = a?;
                        Ok((
                            a.key.as_ref().to_owned(),
                            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)?
                                .into_owned(),
                        ))
                    })
                    .collect::<Result<_>>()?;
                match e.name().as_ref() {
                    "module" => module = attrs.get("name").context("module name")?.clone(),
                    "parameterset" => {
                        set = Some(BTreeMap::from([(
                            "type".into(),
                            attrs.get("type").context("parameter set type")?.clone(),
                        )]))
                    }
                    "param" => {
                        let name = attrs.get("name").context("param name")?.clone();
                        let value = attrs.get("value").context("param value")?.clone();
                        if let Some(s) = &mut set {
                            s.insert(name, value);
                        } else {
                            modules
                                .entry(module.clone())
                                .or_default()
                                .insert(name, value);
                        }
                    }
                    _ => {}
                }
            }
            Event::End(e) if e.name().as_ref() == "parameterset" => {
                if let Some(s) = set.take()
                    && module == "swissRailRaptor"
                    && s.get("type")
                        .is_some_and(|v| v == "modeToModeTransferPenalty")
                {
                    transfers.push(ModeTransfer {
                        from: s.get("fromMode").context("transfer fromMode")?.clone(),
                        to: s.get("toMode").context("transfer toMode")?.clone(),
                        cost: s
                            .get("transferPenalty")
                            .context("transferPenalty")?
                            .parse()?,
                    });
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let eqasim = modules
        .get("eqasim:raptor")
        .context("Eqasim cost import requires eqasim:raptor module")?;
    let swiss = modules.get("swissRailRaptor").cloned().unwrap_or_default();
    let value = |map: &BTreeMap<String, String>, key: &str, default: f64| -> Result<f64> {
        map.get(key).map_or(Ok(default), |v| {
            v.parse().with_context(|| format!("invalid {key}"))
        })
    };
    // Defaults are those in EqasimRaptorConfigGroup, not generic MATSim scoring defaults.
    let mut result = Costs {
        walk_per_hour: -value(eqasim, "walkTime_u_h", -7.0)?,
        wait_per_hour: -value(eqasim, "waitTime_u_h", -6.0)?,
        other_per_hour: -value(eqasim, "travelTimeOther_u_h", -7.0)?,
        transfer_base: -value(eqasim, "perTransfer_u", -1.0)?,
        transfer_per_travel_hour: value(&swiss, "transferPenaltyCostPerTravelTimeHour", 0.0)?,
        transfer_min: value(&swiss, "transferPenaltyMinCost", f64::NEG_INFINITY)?,
        transfer_max: value(&swiss, "transferPenaltyMaxCost", f64::INFINITY)?,
        mode_transfers: transfers,
        ..Costs::default()
    };
    for (mode, key) in [
        ("rail", "travelTimeRail_u_h"),
        ("subway", "travelTimeSubway_u_h"),
        ("bus", "travelTimeBus_u_h"),
        ("tram", "travelTimeTram_u_h"),
    ] {
        result
            .modes_per_hour
            .insert(mode.into(), -value(eqasim, key, -7.0)?);
    }
    ensure!(
        swiss
            .get("useCapacityConstraints")
            .is_none_or(|s| s == "false"),
        "capacity-dependent costs cannot be reproduced by static routing"
    );
    result.validate()?;
    Ok(result)
}
