//! Movebank CSV exports (hyphenated `location-lat` or R/move2 underscored `location_lat`) of
//! marine studies: GPS and Argos locations in one file, plus depth / temperature / light rows.
//!
//! * Argos rows (`sensor-type` = `argos-doppler-shift`, or any row with `argos:lc`) keep the
//!   location class and, for Kalman-filter locations, the error ellipse
//!   (`argos:semi-major`, `argos:semi-minor`, `argos:orientation`); the error radius is
//!   `argos:error-radius`, else the RMS of the ellipse, else the class's nominal accuracy.
//! * Rows without a position are sensor samples: `depth` / `tdr-depth` / `barometric-depth`
//!   (m, positive down — a mostly negative column is flipped), `external-temperature`,
//!   `light-level`.
//! * Rows marked `visible = false` or as outliers are dropped.

use super::*;
use crate::model::{ArgosInfo, ArgosLc, LocationSource};
use crate::table::parse_time;

pub struct Movebank;

fn has_movebank_core(t: &Table) -> bool {
    t.has(&["location_lat"]) && t.has(&["location_long", "location_lon"]) && t.has(&["timestamp"])
}

/// Time of a Movebank row: `timestamp`, else midpoint of `start-timestamp`/`end-timestamp`.
fn row_time(
    t: &Table,
    row: &[String],
    c_ts: Option<usize>,
    c_s: Option<usize>,
    c_e: Option<usize>,
) -> Option<i64> {
    parse_time(t.get(row, c_ts)).or_else(|| {
        let s = parse_time(t.get(row, c_s))?;
        let e = parse_time(t.get(row, c_e)).unwrap_or(s);
        Some((s + e) / 2)
    })
}

fn id_of(
    t: &Table,
    row: &[String],
    c_ind: Option<usize>,
    c_tag: Option<usize>,
    fallback: &str,
) -> String {
    let a = t.get(row, c_ind).trim();
    if !a.is_empty() {
        return a.to_string();
    }
    let b = t.get(row, c_tag).trim();
    if !b.is_empty() {
        b.to_string()
    } else {
        fallback.to_string()
    }
}

pub(crate) const DEPTH_ALIASES: &[&str] = &[
    "depth",
    "depth_m",
    "depth_meters",
    "tdr_depth",
    "barometric_depth",
    "pressure_depth",
    "dive_depth",
];

pub(crate) const WATER_TEMP_ALIASES: &[&str] = &[
    "external_temperature",
    "water_temperature",
    "tdr_temperature",
    "temperature",
    "temp_c",
    "eobs_temperature",
];

/// Depth columns are positive down; flip tracks whose depths are mostly negative.
pub(crate) fn normalise_depth_sign(tr: &mut Track) {
    let mut v: Vec<f32> = tr
        .sensors
        .iter()
        .filter(|s| s.kind == SensorKind::Depth)
        .map(|s| s.value)
        .chain(tr.fixes.iter().filter_map(|f| f.depth_m))
        .collect();
    if v.is_empty() {
        return;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    if v[v.len() / 2] < 0.0 {
        for s in tr
            .sensors
            .iter_mut()
            .filter(|s| s.kind == SensorKind::Depth)
        {
            s.value = -s.value;
        }
        for f in tr.fixes.iter_mut() {
            if let Some(d) = f.depth_m.as_mut() {
                *d = -*d;
            }
        }
    }
}

impl Firmware for Movebank {
    fn id(&self) -> &'static str {
        "movebank"
    }
    fn label(&self) -> &'static str {
        "Movebank · GPS + Argos"
    }
    fn description(&self) -> &'static str {
        "Movebank export: GPS and Argos locations (location class, error ellipse), depth, temperature and light rows"
    }
    fn detect(&self, t: &Table) -> f32 {
        if !has_movebank_core(t) {
            return 0.0;
        }
        let marine = t.norm.iter().any(|n| {
            n.starts_with("argos_") || n.starts_with("gps_") || n.starts_with("height_above")
        }) || t.has(DEPTH_ALIASES);
        if marine { 0.9 } else { 0.75 }
    }

    fn parse(&self, t: &Table) -> Result<ParseOutput, String> {
        let c_ts = t.col(&["timestamp"]);
        let c_start = t.col(&["start_timestamp"]);
        let c_end = t.col(&["end_timestamp"]);
        let c_lat = t.col(&["location_lat"]);
        let c_lon = t.col(&["location_long", "location_lon"]);
        let c_ind = t.col(&["individual_local_identifier"]);
        let c_tag = t.col(&["tag_local_identifier"]);
        let c_sp = t.col(&[
            "individual_taxon_canonical_name",
            "taxon_canonical_name",
            "species",
        ]);
        let c_sensor = t.col(&["sensor_type", "sensor_type_id"]);
        let c_lc = t.col(&["argos_lc", "argos_location_class", "lc", "location_class"]);
        let c_arad = t.col(&["argos_error_radius"]);
        let c_smaj = t.col(&["argos_semi_major"]);
        let c_smin = t.col(&["argos_semi_minor"]);
        let c_orient = t.col(&["argos_orientation", "argos_ellipse_orientation"]);
        let c_nmes = t.col(&["argos_nb_mes"]);
        let c_alt = t.col(&[
            "height_above_msl",
            "height_above_mean_sea_level",
            "height_above_ellipsoid",
        ]);
        let c_speed = t.col(&["ground_speed"]);
        let c_head = t.col(&["heading"]);
        let c_hdop = t.col(&["gps_hdop"]);
        let c_err = t.col(&[
            "location_error_numerical",
            "gps_horizontal_accuracy_estimate",
        ]);
        let c_sats = t.col(&["gps_satellite_count"]);
        let c_depth = t.col(DEPTH_ALIASES);
        let c_temp = t.col(WATER_TEMP_ALIASES);
        let c_light = t.col(&["light_level", "light"]);
        let c_vedba = t.col(&["vedba", "acceleration_vedba", "odba"]);
        let c_vis = t.col(&["visible"]);
        let c_outlier = t.col(&["manually_marked_outlier", "algorithm_marked_outlier"]);

        let fallback = file_stem(&t.name);
        let mut b = TrackBuilder::new("Movebank");
        let mut out = ParseOutput::default();
        let (mut dropped, mut skipped) = (0, 0);
        for row in &t.rows {
            if t.get(row, c_vis).eq_ignore_ascii_case("false")
                || t.get(row, c_outlier).eq_ignore_ascii_case("true")
            {
                dropped += 1;
                continue;
            }
            let Some(ts) = row_time(t, row, c_ts, c_start, c_end) else {
                skipped += 1;
                continue;
            };
            let id = id_of(t, row, c_ind, c_tag, &fallback);
            let tr = b.track(&id);
            if tr.species.is_none() {
                let sp = t.get(row, c_sp).trim();
                if !sp.is_empty() {
                    tr.species = Some(sp.to_string());
                }
            }
            let sensor = t.get(row, c_sensor).to_lowercase();
            let depth = t.num(row, c_depth);
            let temp = t.num(row, c_temp);
            let lat = t.num(row, c_lat);
            let lon = t.num(row, c_lon);
            let located = matches!((lat, lon), (Some(la), Some(lo))
                if !(la == 0.0 && lo == 0.0) && la.abs() <= 90.0 && lo.abs() <= 180.0)
                && !sensor.contains("accel");
            if !located {
                push_sensor(tr, ts, SensorKind::Depth, depth);
                push_sensor(tr, ts, SensorKind::Temperature, temp);
                push_sensor(tr, ts, SensorKind::Light, t.num(row, c_light));
                push_sensor(tr, ts, SensorKind::Vedba, t.num(row, c_vedba));
                continue;
            }
            let (la, lo) = (lat.unwrap_or_default(), lon.unwrap_or_default());
            let lc = ArgosLc::parse(t.get(row, c_lc));
            let is_argos = sensor.contains("argos") || (lc.is_some() && !sensor.contains("gps"));
            let mut f = Fix::new(
                ts,
                la,
                lo,
                if is_argos {
                    LocationSource::Argos
                } else {
                    LocationSource::Gps
                },
            );
            if is_argos {
                let lc = lc.unwrap_or(ArgosLc::Z);
                let (a, bm) = (
                    t.num(row, c_smaj).map(|v| v as f32),
                    t.num(row, c_smin).map(|v| v as f32),
                );
                f.argos = Some(ArgosInfo {
                    lc,
                    semi_major_m: a,
                    semi_minor_m: bm,
                    orientation_deg: t.num(row, c_orient).map(|v| v as f32),
                });
                f.error_m = t
                    .num(row, c_arad)
                    .map(|v| v as f32)
                    .filter(|v| *v > 0.0)
                    .or(match (a, bm) {
                        (Some(a), Some(bm)) if a > 0.0 => Some(((a * a + bm * bm) / 2.0).sqrt()),
                        _ => None,
                    })
                    .or(Some(lc.nominal_error_m()));
                if let Some(n) = t.num(row, c_nmes) {
                    f.extra.push(("argos_nb_mes".into(), format!("{n}")));
                }
                tr.tag = "Argos".into();
            } else {
                if let Some(a) = t.num(row, c_alt) {
                    f.alt_m = Some(a as f32);
                    f.alt_source = Some(AltitudeSource::Gps);
                }
                f.reported_speed_ms = t.num(row, c_speed).map(|v| v as f32);
                f.reported_heading = t.num(row, c_head).map(|v| v as f32);
                // ~5 m user-equivalent range error per unit HDOP
                f.error_m = t
                    .num(row, c_err)
                    .or_else(|| t.num(row, c_hdop).map(|h| h * 5.0))
                    .map(|v| v as f32);
                if let Some(s) = t.num(row, c_sats) {
                    f.extra.push(("gps_satellite_count".into(), format!("{s}")));
                }
                if tr.tag == "Movebank" {
                    tr.tag = "GPS".into();
                }
            }
            f.depth_m = depth.map(|v| v as f32);
            f.temp_c = temp.map(|v| v as f32);
            push_sensor(tr, ts, SensorKind::Depth, depth);
            push_sensor(tr, ts, SensorKind::Temperature, temp);
            import_weather(t, row, &mut f);
            tr.fixes.push(f);
        }
        let mut tracks = b.finish();
        let mut tags = std::collections::HashMap::new();
        for tr in &mut tracks {
            normalise_depth_sign(tr);
            let gps = tr.fixes.iter().any(|f| f.source == LocationSource::Gps);
            let argos = tr.fixes.iter().any(|f| f.source == LocationSource::Argos);
            tr.tag = match (gps, argos) {
                (true, true) => "GPS + Argos".into(),
                (false, true) => "Argos".into(),
                (true, false) => "GPS".into(),
                _ => "sensors only".into(),
            };
            *tags.entry(tr.tag.clone()).or_insert(0) += 1;
        }
        // tracks with sensor rows but no position can't be shown on the map
        let n0 = tracks.len();
        tracks.retain(|tr| !tr.fixes.is_empty());
        if tracks.len() < n0 {
            out.warnings.push(format!(
                "{}: {} individual(s) had sensor data but no locations",
                t.name,
                n0 - tracks.len()
            ));
        }
        if dropped > 0 {
            out.warnings.push(format!(
                "{}: dropped {dropped} rows flagged invisible/outlier",
                t.name
            ));
        }
        if skipped > 0 {
            out.warnings.push(format!(
                "{}: skipped {skipped} rows without a timestamp",
                t.name
            ));
        }
        out.tracks = tracks;
        Ok(out)
    }
}
