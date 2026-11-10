use super::*;
use crate::model::{ArgosLc, LocationSource, SensorKind};
use crate::table::Table;

fn tbl(name: &str, s: &str) -> Table {
    Table::from_str(name, s).expect("table")
}

#[test]
fn movebank_gps_and_argos_with_depth_rows() {
    let csv = "event-id,visible,timestamp,location-long,location-lat,sensor-type,argos:lc,argos:semi-major,argos:semi-minor,argos:orientation,argos:error-radius,gps:hdop,depth,external-temperature,individual-local-identifier,individual-taxon-canonical-name\n\
1,true,2024-05-01 10:00:00.000,-120.0,34.0,gps,,,,,,1.2,,,Seal1,Mirounga angustirostris\n\
2,true,2024-05-01 11:00:00.000,-120.1,33.9,argos-doppler-shift,2,900,300,45,,,,,Seal1,Mirounga angustirostris\n\
3,true,2024-05-01 12:00:00.000,-120.2,33.8,argos-doppler-shift,B,,,,,,,,Seal1,Mirounga angustirostris\n\
4,true,2024-05-01 12:30:00.000,,,,,,,,,,350.5,8.2,Seal1,Mirounga angustirostris\n\
5,true,2024-05-01 12:31:00.000,,,,,,,,,,420.0,7.9,Seal1,Mirounga angustirostris\n\
6,false,2024-05-01 13:00:00.000,-125.0,30.0,argos-doppler-shift,Z,,,,,,,,Seal1,Mirounga angustirostris\n";
    let t = tbl("seal.csv", csv);
    let (out, id) = parse(&t, None).unwrap();
    assert_eq!(id, "movebank");
    let tr = &out.tracks[0];
    assert_eq!(tr.tag, "GPS + Argos");
    assert_eq!(tr.fixes.len(), 3, "invisible row dropped");
    assert_eq!(tr.fixes[0].source, LocationSource::Gps);
    assert!((tr.fixes[0].error_m.unwrap() - 6.0).abs() < 1e-4);
    let a = &tr.fixes[1];
    assert_eq!(a.source, LocationSource::Argos);
    let ai = a.argos.unwrap();
    assert_eq!(ai.lc, ArgosLc::Lc2);
    assert_eq!(ai.semi_major_m, Some(900.0));
    // RMS of the ellipse axes
    assert!((a.error_m.unwrap() - (450_000.0f32).sqrt()).abs() < 1.0);
    // class B without ellipse → nominal accuracy
    assert_eq!(tr.fixes[2].argos.unwrap().lc, ArgosLc::B);
    assert_eq!(tr.fixes[2].error_m, Some(ArgosLc::B.nominal_error_m()));
    let depths: Vec<f32> = tr
        .sensors
        .iter()
        .filter(|s| s.kind == SensorKind::Depth)
        .map(|s| s.value)
        .collect();
    assert_eq!(depths, vec![350.5, 420.0]);
    assert_eq!(
        tr.sensors
            .iter()
            .filter(|s| s.kind == SensorKind::Temperature)
            .count(),
        2
    );
}

#[test]
fn negative_depths_are_flipped() {
    let csv = "timestamp,location-long,location-lat,barometric-depth,individual-local-identifier\n\
2024-05-01 10:00:00,10.0,54.0,,P1\n\
2024-05-01 10:01:00,,,-12.5,P1\n\
2024-05-01 10:02:00,,,-30.0,P1\n\
2024-05-01 10:03:00,,,0.3,P1\n\
2024-05-01 11:00:00,10.1,54.1,,P1\n";
    let (out, _) = parse(&tbl("p.csv", csv), None).unwrap();
    let d: Vec<f32> = out.tracks[0]
        .sensors
        .iter()
        .filter(|s| s.kind == SensorKind::Depth)
        .map(|s| s.value)
        .collect();
    assert_eq!(d, vec![12.5, 30.0, -0.3]);
}

#[test]
fn generic_with_argos_class_and_depth() {
    let csv = "ptt,date,lat,lon,lc,depth,temperature\n\
12345,2023-02-01 00:00:00,-40.0,150.0,3,0,18\n\
12345,2023-02-01 06:00:00,-40.2,150.3,A,55,16\n\
12345,2023-02-01 12:00:00,-40.4,150.5,,,\n";
    let (out, id) = parse(&tbl("turtle.csv", csv), None).unwrap();
    assert_eq!(id, "generic");
    let tr = &out.tracks[0];
    assert_eq!(tr.id, "12345");
    assert_eq!(tr.fixes[0].source, LocationSource::Argos);
    assert_eq!(tr.fixes[1].argos.unwrap().lc, ArgosLc::A);
    assert_eq!(tr.fixes[2].source, LocationSource::Other);
    assert_eq!(tr.fixes[1].depth_m, Some(55.0));
}

#[test]
fn argos_class_parsing_and_order() {
    assert_eq!(ArgosLc::parse("LC3"), Some(ArgosLc::Lc3));
    assert_eq!(ArgosLc::parse("-2"), Some(ArgosLc::B));
    assert_eq!(ArgosLc::parse("x"), None);
    assert!(ArgosLc::Lc1 > ArgosLc::A && ArgosLc::A > ArgosLc::B);
    assert!(ArgosLc::Lc3.nominal_error_m() < ArgosLc::B.nominal_error_m());
}

#[test]
fn generic_flattened_r_with_imported_era5() {
    let csv = "individual_local_identifier,timestamp,lat,lon,era5_u10,era5_v10\n\
N1,2025-09-01T03:00:00Z,47.0,9.0,3.0,-4.0\n\
N1,2025-09-02T03:00:00Z,46.5,8.3,1.0,1.0\n";
    let t = tbl("flat.csv", csv);
    let (out, id) = parse(&t, None).unwrap();
    assert_eq!(id, "generic");
    let f = &out.tracks[0].fixes[0];
    assert_eq!(f.weather.level, Some("imported"));
    assert!((f.weather.wind_speed.unwrap() - 5.0).abs() < 1e-5);
}

#[test]
fn force_override_and_unknown() {
    let t = tbl("x.csv", "a,b\n1,2\n");
    assert!(parse(&t, None).is_err());
    let csv = "individual_local_identifier,timestamp,lat,lon\nA,2025-01-01 00:00:00,1,2\n";
    let t = tbl("y.csv", csv);
    let (_, id) = parse(&t, Some("generic")).unwrap();
    assert_eq!(id, "generic");
    assert!(parse(&t, Some("nope")).is_err());
}

#[test]
fn demo_files_parse() {
    for (name, csv) in crate::demo::all() {
        let t = tbl(name, &csv);
        let (out, id) = parse(&t, None).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(id, "movebank", "{name}");
        assert!(out.tracks.iter().all(|tr| tr.fixes.len() > 5), "{name}");
    }
}
