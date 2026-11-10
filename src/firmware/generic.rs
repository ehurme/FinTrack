//! Fallback for any table with latitude / longitude / time columns (e.g. tracks flattened
//! from R `move2` / `aniMotum`, Lotek / Desert Star / CATS exports): an `lc` / `argos_lc`
//! column makes Argos fixes, `depth` and `temperature` columns become sensor streams as well.

use super::*;
use crate::model::LocationSource;
use crate::table::parse_time;

pub struct Generic;

impl Firmware for Generic {
    fn id(&self) -> &'static str {
        "generic"
    }
    fn label(&self) -> &'static str {
        "Generic · lat/lon/time"
    }
    fn description(&self) -> &'static str {
        "Any CSV with latitude, longitude and time columns (e.g. flattened move2 objects from R)"
    }
    fn detect(&self, t: &Table) -> f32 {
        if t.has(LAT_ALIASES) && t.has(LON_ALIASES) && t.has(TIME_ALIASES) {
            0.3
        } else {
            0.0
        }
    }

    fn parse(&self, t: &Table) -> Result<ParseOutput, String> {
        let c_lat = t.col(LAT_ALIASES);
        let c_lon = t.col(LON_ALIASES);
        let c_time = t.col(TIME_ALIASES);
        let c_id = t.col(ID_ALIASES);
        let c_tag = t.col(&["tag_type", "tag_model", "model"]);
        let c_sp = t.col(&["species", "individual_taxon_canonical_name", "taxon"]);
        let c_alt = t.col(&[
            "altitude_m",
            "max_altitude_m",
            "altitude",
            "height_above_msl",
            "height_above_ellipsoid",
            "alt",
            "height",
        ]);
        let c_pres = t.col(&[
            "min_pressure",
            "barometric_pressure",
            "pressure_hpa",
            "pressure",
        ]);
        let c_err = t.col(&[
            "sigfox_computed_location_radius",
            "location_error_m",
            "error_m",
            "radius_m",
            "radius",
        ]);
        let c_act = t.col(&["vedba_sum", "vedba", "activity"]);
        let c_temp = t.col(&["avg_temp", "external_temperature", "temperature", "temp_c"]);
        let c_speed = t.col(&["ground_speed", "speed_ms"]);
        let c_head = t.col(&["heading"]);
        let c_lc = t.col(&["argos_lc", "lc", "location_class", "quality"]);
        let c_depth = t.col(super::movebank::DEPTH_ALIASES);

        let fallback = file_stem(&t.name);
        let mut b = TrackBuilder::new("unknown");
        let mut out = ParseOutput::default();
        for row in &t.rows {
            let (Some(ts), Some(la), Some(lo)) = (
                parse_time(t.get(row, c_time)),
                t.num(row, c_lat),
                t.num(row, c_lon),
            ) else {
                continue;
            };
            if la.abs() > 90.0 || lo.abs() > 180.0 {
                continue;
            }
            let id = {
                let s = t.get(row, c_id).trim();
                if s.is_empty() {
                    fallback.clone()
                } else {
                    s.to_string()
                }
            };
            let tr = b.track(&id);
            let tag = t.get(row, c_tag).trim();
            if !tag.is_empty() {
                tr.tag = tag.to_string();
            }
            if tr.species.is_none() {
                let sp = t.get(row, c_sp).trim();
                if !sp.is_empty() {
                    tr.species = Some(sp.to_string());
                }
            }
            let lc = crate::model::ArgosLc::parse(t.get(row, c_lc));
            let src = if lc.is_some() {
                LocationSource::Argos
            } else {
                LocationSource::Other
            };
            let mut f = Fix::new(ts, la, lo, src);
            f.error_m = t
                .num(row, c_err)
                .map(|v| v as f32)
                .or(lc.map(|l| l.nominal_error_m()));
            if let Some(lc) = lc {
                f.argos = Some(crate::model::ArgosInfo {
                    lc,
                    semi_major_m: None,
                    semi_minor_m: None,
                    orientation_deg: None,
                });
            }
            let depth = t.num(row, c_depth);
            f.depth_m = depth.map(|v| v as f32);
            push_sensor(tr, ts, SensorKind::Depth, depth);
            push_sensor(tr, ts, SensorKind::Temperature, t.num(row, c_temp));
            if let Some(a) = t.num(row, c_alt) {
                f.alt_m = Some(a as f32);
                f.alt_source = Some(if c_alt.is_some_and(|c| t.norm[c].starts_with("height")) {
                    AltitudeSource::Gps
                } else {
                    AltitudeSource::PressureWindowMin
                });
            }
            if let Some(p) = t.num(row, c_pres) {
                if f.alt_m.is_none() {
                    set_pressure_alt(&mut f, p, AltitudeSource::PressureWindowMin);
                } else {
                    f.pressure_hpa = Some(p as f32);
                }
            }
            f.activity = t.num(row, c_act).map(|v| v as f32);
            f.temp_c = t.num(row, c_temp).map(|v| v as f32);
            f.reported_speed_ms = t.num(row, c_speed).map(|v| v as f32);
            f.reported_heading = t.num(row, c_head).map(|v| v as f32);
            import_weather(t, row, &mut f);
            tr.fixes.push(f);
        }
        let mut tracks = b.finish();
        for tr in &mut tracks {
            super::movebank::normalise_depth_sign(tr);
        }
        out.tracks = tracks;
        Ok(out)
    }
}
