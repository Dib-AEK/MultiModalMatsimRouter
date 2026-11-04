//! Verifies real readers, binary integrity, configuration strictness, geometry and ordered parallel routing.
use multimodal_matsim_router::{
    cache,
    config::{Cache, Config, Execution, Input, Preprocessing, Routing},
    engine::Engine,
    model::Coordinates,
    preprocess, readers,
    routing::Workspace,
    time,
};
use std::{fs, io::Write};
use tempfile::tempdir;
fn example() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/timetable.xml")
}
#[test]
fn matsim_gzip_binary_and_parallel_round_trip() {
    let dir = tempdir().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&fs::read(example()).unwrap()).unwrap();
    let path = dir.path().join("schedule.xml.gz");
    fs::write(&path, gz.finish().unwrap()).unwrap();
    let config = Config {
        input: Input::Matsim {
            path,
            coordinates: Coordinates::ProjectedMeters,
        },
        preprocessing: Preprocessing {
            walking_radius_m: 0.0,
            reachability_cells: 2,
            ..Default::default()
        },
        routing: Routing::default(),
        execution: Execution {
            threads: 12,
            ..Execution::default()
        },
        cache: Cache {
            path: dir.path().join("schedule.mmrt"),
            ..Default::default()
        },
    };
    let engine = Engine::load(&config).unwrap();
    assert!(engine.weighted_index_summary().is_none());
    let query = engine.query(&config, "A", "C", "07:59:00").unwrap();
    let first = engine
        .route(&query, &mut Workspace::default())
        .unwrap()
        .unwrap();
    assert_eq!(first.arrival, time::parse_time("08:10:00").unwrap());
    assert_eq!(first.boardings, 2);
    let queries = vec![query; 200];
    for answer in engine.batch(&queries) {
        assert_eq!(answer.unwrap().unwrap().arrival, first.arrival);
    }
    cache::write(&config.cache.path, &engine.timetable).unwrap();
    let restored = cache::read(&config.cache.path, config.cache.max_bytes).unwrap();
    assert_eq!(
        restored.connections.len(),
        engine.timetable.connections.len()
    );
    let mut bytes = fs::read(&config.cache.path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    fs::write(&config.cache.path, bytes).unwrap();
    assert!(cache::read(&config.cache.path, config.cache.max_bytes).is_err());
}

#[test]
fn twelve_workers_share_lazy_weighted_preprocessing_and_preserve_batch_order() {
    let config =
        Config::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config.example.toml"))
            .unwrap();
    let mut config = config;
    config.execution.threads = 12;
    let engine = Engine::load(&config).unwrap();
    assert!(engine.weighted_index_summary().is_none());
    let q = multimodal_matsim_router::weighted::Query {
        departure: time::parse_time("07:59:00").unwrap(),
        max_boardings: 5,
        max_duration: 3_600_000,
        access: vec![multimodal_matsim_router::weighted::Endpoint {
            stop: engine.stop_id("A").unwrap(),
            duration: 0,
        }],
        egress: vec![multimodal_matsim_router::weighted::Endpoint {
            stop: engine.stop_id("C").unwrap(),
            duration: 0,
        }],
    };
    let mut unreachable = q.clone();
    unreachable.departure = time::parse_time("23:00:00").unwrap();
    let queries: Vec<_> = (0..240)
        .map(|i| {
            if i % 3 == 0 {
                unreachable.clone()
            } else {
                q.clone()
            }
        })
        .collect();
    for (i, result) in engine.weighted_batch(&queries).into_iter().enumerate() {
        let result = result.unwrap();
        assert_eq!(result.is_none(), i % 3 == 0);
        if let Some(j) = result {
            assert_eq!(j.journey.arrival, time::parse_time("08:10:00").unwrap());
        }
    }
    let index = engine.weighted_index_summary().unwrap();
    assert!(index.relaxed_edges > 0 && index.boarding_events > 0);
}

#[test]
fn all_batch_schedulers_preserve_routes_errors_hints_and_concurrent_requests() {
    use multimodal_matsim_router::{
        batching::Scheduling,
        weighted::{Endpoint, Query},
    };
    for scheduling in [
        Scheduling::Auto,
        Scheduling::Rayon,
        Scheduling::FineGrained,
        Scheduling::Dynamic,
        Scheduling::EstimatedWork,
    ] {
        let mut config = Config::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config.example.toml"),
        )
        .unwrap();
        config.execution.threads = 12;
        config.execution.scheduling = scheduling;
        let engine = Engine::load(&config).unwrap();
        let q = Query {
            departure: time::parse_time("07:59:00").unwrap(),
            max_boardings: 5,
            max_duration: 3_600_000,
            access: vec![Endpoint {
                stop: engine.stop_id("A").unwrap(),
                duration: 0,
            }],
            egress: vec![Endpoint {
                stop: engine.stop_id("C").unwrap(),
                duration: 0,
            }],
        };
        let mut missing = q.clone();
        missing.departure = time::parse_time("23:00:00").unwrap();
        let mut invalid = q.clone();
        invalid.access[0].stop = u32::MAX;
        let queries: Vec<_> = (0..96)
            .map(|i| match i % 3 {
                0 => q.clone(),
                1 => missing.clone(),
                _ => invalid.clone(),
            })
            .collect();
        let check = |results: Vec<
            anyhow::Result<Option<multimodal_matsim_router::weighted::ResultJourney>>,
        >| {
            assert_eq!(results.len(), queries.len());
            for (i, r) in results.into_iter().enumerate() {
                match i % 3 {
                    0 => assert_eq!(
                        r.unwrap().unwrap().journey.arrival,
                        time::parse_time("08:10:00").unwrap()
                    ),
                    1 => assert!(r.unwrap().is_none()),
                    _ => assert!(r.is_err()),
                }
            }
        };
        assert!(engine.weighted_batch_with_estimates(&queries, &[]).is_err());
        assert!(engine.weighted_batch_profiled(&queries, Some(&[])).is_err());
        let profiled = engine.weighted_batch_profiled(&queries, None).unwrap();
        let estimates = profiled.metrics.query_nanoseconds;
        check(profiled.results);
        check(
            engine
                .weighted_batch_with_estimates(&queries, &estimates)
                .unwrap(),
        );
        std::thread::scope(|scope| {
            let a = scope.spawn(|| engine.weighted_batch(&queries));
            let b = scope.spawn(|| {
                engine
                    .weighted_batch_with_estimates(&queries, &estimates)
                    .unwrap()
            });
            check(a.join().unwrap());
            check(b.join().unwrap());
        });
    }
}

#[test]
fn auto_scheduling_learns_bounded_cost_hints_and_can_be_disabled() {
    use multimodal_matsim_router::{
        batching::Scheduling,
        weighted::{Endpoint, Query},
    };
    let mut config =
        Config::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config.example.toml"))
            .unwrap();
    config.execution.threads = 12;
    config.execution.work_estimate_capacity = 2;
    let engine = Engine::load(&config).unwrap();
    let q = Query {
        departure: time::parse_time("07:59:00").unwrap(),
        max_boardings: 5,
        max_duration: 3_600_000,
        access: vec![Endpoint {
            stop: engine.stop_id("A").unwrap(),
            duration: 0,
        }],
        egress: vec![Endpoint {
            stop: engine.stop_id("C").unwrap(),
            duration: 0,
        }],
    };
    let queries = vec![q.clone(); 30];
    let first = engine.weighted_batch_profiled(&queries, None).unwrap();
    assert_eq!(first.metrics.scheduling, Scheduling::Rayon);
    assert_eq!(first.metrics.advisory_history_hits, 0);
    let repeated = engine.weighted_batch_profiled(&queries, None).unwrap();
    assert_eq!(repeated.metrics.scheduling, Scheduling::EstimatedWork);
    assert_eq!(repeated.metrics.advisory_history_hits, 30);
    for r in repeated.results {
        assert_eq!(
            r.unwrap().unwrap().journey.arrival,
            time::parse_time("08:10:00").unwrap()
        );
    }
    let distinct: Vec<_> = (0..10)
        .map(|i| {
            let mut q = q.clone();
            q.departure += i;
            q
        })
        .collect();
    for r in engine.weighted_batch(&distinct) {
        r.unwrap();
    }
    assert_eq!(engine.work_history_summary().entries, 2);
    config.execution.work_estimate_capacity = 0;
    let disabled = Engine::load(&config).unwrap();
    for _ in 0..2 {
        let batch = disabled.weighted_batch_profiled(&queries, None).unwrap();
        assert_eq!(batch.metrics.scheduling, Scheduling::Rayon);
        assert_eq!(disabled.work_history_summary().entries, 0);
    }
}
#[test]
fn gtfs_calendar_exceptions_sequence_overnight_permissions_and_zip() {
    let dir = tempdir().unwrap();
    let files = [
        (
            "stops.txt",
            "stop_id,stop_name,stop_lat,stop_lon\nA,Origin,46,6\nB,Destination,46.1,6.1\n",
        ),
        ("routes.txt", "route_id,route_type\nR,3\n"),
        (
            "trips.txt",
            "route_id,service_id,trip_id\nR,week,T\nR,removed,U\n",
        ),
        (
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\nweek,1,1,1,1,1,0,0,20260101,20261231\nremoved,1,1,1,1,1,1,1,20260101,20261231\n",
        ),
        (
            "calendar_dates.txt",
            "service_id,date,exception_type\nremoved,20261030,2\n",
        ),
        (
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence,pickup_type,drop_off_type\nT,25:10:00,25:10:00,B,20,1,0\nT,25:00:00,25:00:00,A,10,0,1\nU,00:00:00,00:00:00,A,0,0,0\nU,00:10:00,00:10:00,B,1,0,0\n",
        ),
    ];
    let zip_path = dir.path().join("feed.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
    for (name, text) in files {
        fs::write(dir.path().join(name), text).unwrap();
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    for path in [dir.path(), zip_path.as_path()] {
        let input = readers::gtfs::read(path, "2026-10-30", 1.3).unwrap();
        assert_eq!(input.trips.len(), 1);
        assert_eq!(input.trips[0].calls[0].stop, 0);
        assert!(input.trips[0].calls[0].pickup);
        assert!(!input.trips[0].calls[0].dropoff);
        assert!(!input.trips[0].calls[1].pickup);
        assert_eq!(
            input.trips[0].calls[1].arrival,
            time::parse_time("25:10:00").unwrap()
        );
        preprocess::prepare(input, &Preprocessing::default()).unwrap();
    }
    fs::write(
        dir.path().join("frequencies.txt"),
        "trip_id,start_time,end_time,headway_secs
T,25:00:00,26:00:00,600
",
    )
    .unwrap();
    assert!(readers::gtfs::read(dir.path(), "2026-10-30", 1.3).is_err());
}
#[test]
fn config_is_relative_and_rejects_unknown_or_invalid_values() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("router.toml");
    fs::write(
        &path,
        "[input]
format='matsim'
path='feed.xml'
coordinates='projected_meters'
",
    )
    .unwrap();
    let config = Config::load(&path).unwrap();
    assert!(
        matches!(config.input,Input::Matsim{path:input,..} if input==dir.path().join("feed.xml"))
    );
    fs::write(
        &path,
        "[input]
format='matsim'
path='feed.xml'
coordinates='projected_meters'
typo=1
",
    )
    .unwrap();
    assert!(Config::load(&path).is_err());
    assert!(time::parse_time("25:59:59.999").is_ok());
    assert!(time::parse_time("00:60:00").is_err());
    assert!(time::parse_time("00:00:NaN").is_err());
    assert!(time::parse_time("9999999999:00:00").is_err());
}
#[test]
fn matsim_invalid_interior_offsets_are_not_silently_repaired() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad.xml");
    let xml = fs::read_to_string(example()).unwrap().replace(
        r#"arrivalOffset="00:05:00""#,
        r#"arrivalOffset="undefined""#,
    );
    fs::write(&path, xml).unwrap();
    assert!(readers::matsim::read(&path, Coordinates::ProjectedMeters).is_err());
}

#[test]
fn imported_matsim_transfers_can_be_honored_or_explicitly_disabled() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("transfers.xml");
    let source = fs::read_to_string(example()).unwrap().replace(
        "</transitStops>",
        r#"</transitStops>
    <minimalTransferTimes><relation fromStop="A" toStop="C" transferTime="600.0"/>
    <relation fromStop="B" toStop="B" transferTime="120.0"/></minimalTransferTimes>"#,
    );
    fs::write(&path, source).unwrap();
    let raw = readers::matsim::read(&path, Coordinates::ProjectedMeters).unwrap();
    let settings = Preprocessing {
        walking_radius_m: 0.0,
        ..Default::default()
    };
    let enabled = preprocess::prepare(raw.clone(), &settings).unwrap();
    assert_eq!(enabled.walks.len(), 1);
    assert_eq!(enabled.stops[1].transfer_buffer, 120000);
    let disabled = preprocess::prepare(
        raw,
        &Preprocessing {
            use_explicit_transfers: false,
            ..settings
        },
    )
    .unwrap();
    assert!(disabled.walks.is_empty());
    assert_eq!(disabled.stops[1].transfer_buffer, 0);
}
