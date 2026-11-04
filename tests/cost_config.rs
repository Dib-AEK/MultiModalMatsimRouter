//! Verifies effective Eqasim config overrides, relative import paths, defaults and unsupported penalties.
use multimodal_matsim_router::{config::Config, cost::Costs, readers::matsim_config};
use std::fs;
use tempfile::tempdir;
#[test]
fn import_uses_eqasim_units_defaults_and_mode_matrix() {
    let dir = tempdir().unwrap();
    let xml = dir.path().join("matsim.xml");
    fs::write(&xml,r#"<config><module name="eqasim:raptor"><param name="travelTimeBus_u_h" value="-1.584"/><param name="perTransfer_u" value="-0.05"/></module>
      <module name="swissRailRaptor"><param name="transferPenaltyBaseCost" value="999"/>
      <param name="transferPenaltyMinCost" value="-Infinity"/><param name="transferPenaltyMaxCost" value="Infinity"/>
      <parameterset type="modeToModeTransferPenalty"><param name="fromMode" value="bus"/><param name="toMode" value="rail"/><param name="transferPenalty" value="0.218"/></parameterset></module></config>"#).unwrap();
    let c = matsim_config::costs(&xml).unwrap();
    assert_eq!(c.transfer_base, 0.05);
    assert_eq!(c.walk_per_hour, 7.0);
    assert_eq!(c.wait_per_hour, 6.0);
    assert_eq!(c.modes_per_hour["bus"], 1.584);
    assert_eq!(c.modes_per_hour["rail"], 7.0);
    assert_eq!(c.mode_transfers[0].cost, 0.218);
    assert_eq!(c.transfer_min, f64::NEG_INFINITY);
    let toml = dir.path().join("router.toml");
    fs::write(&toml,"[input]\nformat='matsim'\npath='schedule.xml'\ncoordinates='projected_meters'\n[routing]\nobjective='generalized_cost'\nmatsim_config='matsim.xml'\n").unwrap();
    let imported = Config::load(&toml).unwrap();
    assert_eq!(imported.routing.costs.transfer_base, 0.05);
    assert_eq!(imported.routing.matsim_config.unwrap(), xml);
}
#[test]
fn invalid_or_unsupported_costs_are_rejected() {
    let c = Costs {
        walk_per_hour: -1.0,
        ..Costs::default()
    };
    assert!(c.validate().is_err());
    let c = Costs {
        transfer_base: -1.0,
        transfer_min: f64::NEG_INFINITY,
        ..Costs::default()
    };
    assert!(c.validate().is_err());
    let mut c = Costs {
        transfer_per_travel_hour: 1.0,
        ..Costs::default()
    };
    c.mode_transfers
        .push(multimodal_matsim_router::cost::ModeTransfer {
            from: "bus".into(),
            to: "rail".into(),
            cost: 0.2,
        });
    assert!(c.validate().is_err());
}
