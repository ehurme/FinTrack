//! Ocean state at the fixes from the free Open-Meteo Marine API (no key).
//!
//! * From 2022: waves (best-match wave models), surface currents, sea surface temperature
//!   and sea level incl. tides from Météo-France SMOC (Copernicus Marine, 1/12°).
//! * Before 2022: waves only, from ERA5-Ocean (0.5°, since 1940) — currents and SST are not
//!   available there.
//!
//! Points are binned to 0.1° cells; one request covers up to [`CELLS_PER_REQUEST`] cells for
//! the days they need within one calendar month. Current speed comes back in km/h by
//! default; the `hourly_units` of the response decide the conversion to m/s. The current
//! direction is the direction the water flows *towards* (oceanographic convention).

use crate::model::{MarineAnno, Track};
use chrono::{DateTime, Datelike, Utc};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const API: &str = "https://marine-api.open-meteo.com/v1/marine";
pub const GRID: f64 = 0.1;
pub const CELLS_PER_REQUEST: usize = 40;
/// First day with SMOC currents / SST (2022-01-01).
pub const SMOC_FIRST_DAY: i64 = 18_993;

pub const VARS: [&str; 8] = [
    "wave_height",
    "wave_direction",
    "wave_period",
    "swell_wave_height",
    "ocean_current_velocity",
    "ocean_current_direction",
    "sea_surface_temperature",
    "sea_level_height_msl",
];
pub const NV: usize = VARS.len();
const WAVE_VARS: usize = 4;
pub type Values = [f32; NV];
pub type CellKey = (i32, i32);

pub fn cell_of(lat: f64, lon: f64) -> CellKey {
    ((lat / GRID).round() as i32, (lon / GRID).round() as i32)
}

pub fn cell_center(k: CellKey) -> (f64, f64) {
    (k.0 as f64 * GRID, k.1 as f64 * GRID)
}

#[derive(Default)]
pub struct MarineStore {
    pub cells: HashMap<CellKey, BTreeMap<i64, Values>>,
    pub days: HashSet<(CellKey, i64)>,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub cells: Vec<CellKey>,
    pub start_day: i64,
    pub end_day: i64,
    /// ERA5-Ocean waves only (before SMOC).
    pub waves_only: bool,
    pub url: String,
}

fn day_str(d: i64) -> String {
    crate::weather::day_str(d)
}

impl Request {
    pub fn new(cells: Vec<CellKey>, start_day: i64, end_day: i64) -> Self {
        let waves_only = end_day < SMOC_FIRST_DAY;
        let lats: Vec<String> = cells
            .iter()
            .map(|k| format!("{:.2}", cell_center(*k).0))
            .collect();
        let lons: Vec<String> = cells
            .iter()
            .map(|k| format!("{:.2}", cell_center(*k).1))
            .collect();
        let vars = if waves_only {
            VARS[..WAVE_VARS].join(",")
        } else {
            VARS.join(",")
        };
        let mut url = format!(
            "{API}?latitude={}&longitude={}&start_date={}&end_date={}&hourly={vars}&timezone=GMT&timeformat=unixtime",
            lats.join(","),
            lons.join(","),
            day_str(start_day),
            day_str(end_day),
        );
        if waves_only {
            url.push_str("&models=era5_ocean");
        }
        Request {
            cells,
            start_day,
            end_day,
            waves_only,
            url,
        }
    }
}

/// Factor to m/s for a speed unit string of Open-Meteo.
fn to_ms(unit: &str) -> f32 {
    match unit.trim() {
        "m/s" => 1.0,
        "kn" => 0.514_444,
        "mp/h" | "mph" => 0.447_04,
        _ => 1.0 / 3.6, // km/h, the default
    }
}

impl MarineStore {
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn hours(&self) -> usize {
        self.cells.values().map(|c| c.len()).sum()
    }

    /// Requests for the cell-days `points` (lat, lon, t) need, skipping cached ones; months
    /// before 2022 and from 2022 never share a request.
    pub fn plan(&self, points: &[(f64, f64, i64)], latest_day: i64) -> Vec<Request> {
        let mut need: HashSet<(CellKey, i64)> = HashSet::new();
        for &(lat, lon, t) in points {
            let k = cell_of(lat, lon);
            for d in [t.div_euclid(86400), (t + 3600).div_euclid(86400)] {
                if d <= latest_day && !self.days.contains(&(k, d)) {
                    need.insert((k, d));
                }
            }
        }
        let mut by_month: BTreeMap<(i32, u32), BTreeMap<CellKey, (i64, i64)>> = BTreeMap::new();
        for (k, d) in need {
            let date = DateTime::<Utc>::from_timestamp(d * 86400, 0)
                .expect("valid day")
                .date_naive();
            let e = by_month
                .entry((date.year(), date.month()))
                .or_default()
                .entry(k)
                .or_insert((d, d));
            e.0 = e.0.min(d);
            e.1 = e.1.max(d);
        }
        let mut out = Vec::new();
        for (_, cells) in by_month {
            let cells: Vec<_> = cells.into_iter().collect();
            for chunk in cells.chunks(CELLS_PER_REQUEST) {
                let start = chunk.iter().map(|c| c.1.0).min().expect("non-empty");
                let end = chunk.iter().map(|c| c.1.1).max().expect("non-empty");
                out.push(Request::new(
                    chunk.iter().map(|c| c.0).collect(),
                    start,
                    end,
                ));
            }
        }
        out
    }

    pub fn mark_done(&mut self, req: &Request) {
        for k in &req.cells {
            for d in req.start_day..=req.end_day {
                self.days.insert((*k, d));
            }
        }
    }

    /// Store a JSON response. Returns the number of hours with any value.
    pub fn ingest(&mut self, req: &Request, body: &str) -> Result<usize, String> {
        let json: serde_json::Value =
            serde_json::from_str(body).map_err(|e| format!("bad JSON: {e}"))?;
        if json.get("error").and_then(|e| e.as_bool()) == Some(true) {
            return Err(json
                .get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or("API error")
                .to_string());
        }
        let items: Vec<&serde_json::Value> = match &json {
            serde_json::Value::Array(a) => a.iter().collect(),
            o => vec![o],
        };
        if items.len() != req.cells.len() {
            return Err(format!(
                "expected {} locations, got {}",
                req.cells.len(),
                items.len()
            ));
        }
        let mut n = 0;
        for (k, item) in req.cells.iter().zip(items) {
            let Some(hourly) = item.get("hourly") else {
                continue;
            };
            let times: Vec<i64> = hourly
                .get("time")
                .and_then(|t| t.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
                .unwrap_or_default();
            let cur_k = item
                .get("hourly_units")
                .and_then(|u| u.get("ocean_current_velocity"))
                .and_then(|u| u.as_str())
                .map(to_ms)
                .unwrap_or(1.0 / 3.6);
            let cols: Vec<Vec<f32>> = VARS
                .iter()
                .map(|name| {
                    hourly
                        .get(*name)
                        .and_then(|a| a.as_array())
                        .map(|a| {
                            a.iter()
                                .map(|v| v.as_f64().map(|x| x as f32).unwrap_or(f32::NAN))
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .collect();
            let series = self.cells.entry(*k).or_default();
            for (i, &t) in times.iter().enumerate() {
                let mut v = series.get(&t).copied().unwrap_or([f32::NAN; NV]);
                let mut any = false;
                for j in 0..NV {
                    if let Some(x) = cols[j].get(i).copied().filter(|x| x.is_finite()) {
                        v[j] = if j == 4 { x * cur_k } else { x };
                        any = true;
                    }
                }
                if any {
                    series.insert(t, v);
                    n += 1;
                }
            }
        }
        self.mark_done(req);
        Ok(n)
    }

    /// Values at a point and time, linearly interpolated between the bracketing hours
    /// (directions on the circle). Falls back to the 8 neighbouring cells when the point's
    /// own cell has no sea data (coast).
    pub fn sample(&self, lat: f64, lon: f64, t: i64) -> Option<Values> {
        let k = cell_of(lat, lon);
        let try_cell = |k: CellKey| -> Option<Values> {
            let s = self.cells.get(&k)?;
            let h0 = t.div_euclid(3600) * 3600;
            let a = s.get(&h0)?;
            let Some(b) = s.get(&(h0 + 3600)) else {
                return Some(*a);
            };
            let w = (t - h0) as f32 / 3600.0;
            let mut o = [f32::NAN; NV];
            for i in 0..NV {
                o[i] = if i == 1 || i == 5 {
                    let d = ((b[i] - a[i] + 540.0) % 360.0) - 180.0;
                    (a[i] + d * w).rem_euclid(360.0)
                } else {
                    a[i] + (b[i] - a[i]) * w
                };
            }
            Some(o)
        };
        try_cell(k).or_else(|| {
            (-1..=1)
                .flat_map(|a| (-1..=1).map(move |b| (a, b)))
                .filter(|o| *o != (0, 0))
                .find_map(|(a, b)| try_cell((k.0 + a, k.1 + b)))
        })
    }
}

fn opt(v: f32) -> Option<f32> {
    v.is_finite().then_some(v)
}

/// Annotate every fix of the tracks (`keep` decides which fixes); returns the count.
pub fn annotate(tracks: &mut [Track], store: &MarineStore) -> usize {
    let mut n = 0;
    for tr in tracks.iter_mut() {
        for f in tr.fixes.iter_mut() {
            let Some(v) = store.sample(f.lat, f.lon, f.t) else {
                f.weather.marine = None;
                continue;
            };
            let mut m = MarineAnno {
                wave_height_m: opt(v[0]),
                wave_from_deg: opt(v[1]),
                wave_period_s: opt(v[2]),
                swell_height_m: opt(v[3]),
                current_ms: opt(v[4]),
                current_to_deg: opt(v[5]),
                current_support: None,
                current_cross: None,
                sst_c: opt(v[6]),
                sea_level_m: opt(v[7]),
            };
            if let (Some(s), Some(dir), Some(h)) = (m.current_ms, m.current_to_deg, f.bearing_deg) {
                let rel = (dir - h).to_radians();
                m.current_support = Some(s * rel.cos());
                m.current_cross = Some(s * rel.sin());
            }
            f.weather.marine = Some(m);
            n += 1;
        }
    }
    n
}

/// Fix locations to download for (lat, lon, unix s).
pub fn points(tracks: &[Track]) -> Vec<(f64, f64, i64)> {
    tracks
        .iter()
        .filter(|t| t.visible)
        .flat_map(|t| t.fixes.iter().map(|f| (f.lat, f.lon, f.t)))
        .collect()
}

/// 10 m wind → 2 m with a neutral logarithmic profile over the sea (roughness 0.2 mm).
pub fn wind_2m_from_10m(ws10: f32) -> f32 {
    const Z0: f32 = 0.0002;
    ws10 * (2.0f32 / Z0).ln() / (10.0f32 / Z0).ln()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Fix, LocationSource};

    #[test]
    fn plans_by_month_and_era() {
        let st = MarineStore::default();
        let t2021 = 1_633_046_400; // 2021-10-01
        let t2024 = 1_719_792_000; // 2024-07-01
        let pts = vec![
            (54.2, 7.9, t2021),
            (54.21, 7.91, t2021 + 3600),
            (54.2, 7.9, t2024),
        ];
        let r = st.plan(&pts, 30_000);
        assert_eq!(r.len(), 2);
        assert!(r[0].waves_only && r[0].url.contains("models=era5_ocean"));
        assert!(!r[0].url.contains("ocean_current"));
        assert!(!r[1].waves_only && r[1].url.contains("sea_surface_temperature"));
        assert!(r[1].url.starts_with(API) && r[1].url.contains("latitude=54.20"));
        assert_eq!(r[0].cells.len(), 1, "same 0.1° cell");
    }

    #[test]
    fn ingest_converts_units_and_annotates() {
        let mut st = MarineStore::default();
        let t0 = 1_719_792_000;
        let req = Request::new(vec![cell_of(54.2, 7.9)], t0 / 86400, t0 / 86400);
        let body = format!(
            r#"{{"hourly_units":{{"ocean_current_velocity":"km/h"}},"hourly":{{"time":[{t0},{}],"wave_height":[1.0,2.0],"wave_direction":[350,10],"wave_period":[5,6],"swell_wave_height":[0.5,0.5],"ocean_current_velocity":[3.6,3.6],"ocean_current_direction":[90,90],"sea_surface_temperature":[15.0,16.0],"sea_level_height_msl":[0.2,null]}}}}"#,
            t0 + 3600
        );
        assert_eq!(st.ingest(&req, &body).unwrap(), 2);
        let v = st.sample(54.2, 7.9, t0 + 1800).unwrap();
        assert!((v[0] - 1.5).abs() < 1e-6);
        assert!(
            (v[1] - 0.0).abs() < 1e-3 || (v[1] - 360.0).abs() < 1e-3,
            "{}",
            v[1]
        );
        assert!((v[4] - 1.0).abs() < 1e-6, "3.6 km/h = 1 m/s");
        // neighbour fallback
        assert!(st.sample(54.3, 7.9, t0).is_some());
        assert!(st.sample(55.0, 7.9, t0).is_none());
        // annotation: heading east → the eastward current fully assists
        let mut tr = Track::new("a", "GPS", "test");
        tr.fixes
            .push(Fix::new(t0 - 3600, 54.2, 7.85, LocationSource::Gps));
        tr.fixes.push(Fix::new(t0, 54.2, 7.9, LocationSource::Gps));
        tr.compute_steps();
        let mut tracks = vec![tr];
        assert_eq!(annotate(&mut tracks, &st), 1);
        let m = tracks[0].fixes[1].weather.marine.unwrap();
        assert!((m.current_support.unwrap() - 1.0).abs() < 0.01);
        assert!(m.current_cross.unwrap().abs() < 0.02);
        assert_eq!(m.sst_c, Some(15.0));
        assert!(st.days.contains(&(cell_of(54.2, 7.9), t0 / 86400)));
        assert!(
            st.ingest(&req, r#"{"error":true,"reason":"nope"}"#)
                .is_err()
        );
    }

    #[test]
    fn two_metre_wind() {
        let w = wind_2m_from_10m(10.0);
        assert!(w > 8.3 && w < 8.7, "{w}");
    }
}
