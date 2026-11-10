//! Dives from depth records, and the Argos speed filter.
//!
//! **Dives.** A dive is a run of depth samples deeper than the surface threshold; its start
//! and end are interpolated at the threshold crossing, and a data gap longer than
//! [`DiveParams::max_gap_s`] ends it. For each dive: maximum depth and its time, duration,
//! bottom time (time deeper than 80 % of the maximum), descent / ascent rates, the
//! following surface interval, a shape class (U: long bottom phase, V: no bottom phase), the
//! position interpolated between the (filtered) location fixes at the time of maximum depth
//! with the time to the nearest fix as a measure of uncertainty, the sun's elevation there,
//! and — once the seafloor has been sampled — whether the dive reached the seafloor
//! (benthic).
//!
//! **Argos speed filter** (McConnell, Chambers & Fedak 1992): a fix is an outlier when the
//! root mean square of the speeds to its two previous and two next retained fixes exceeds
//! the maximum swimming speed; the worst fix is removed and the test repeated.

use crate::model::{Fix, SensorKind, Track};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiveParams {
    /// Depth (m) separating the surface from a dive.
    pub surface_m: f32,
    /// Dives shallower than this (m) are ignored.
    pub min_depth_m: f32,
    /// Dives shorter than this (s) are ignored.
    pub min_duration_s: f32,
    /// A gap between depth samples longer than this (s) ends a dive.
    pub max_gap_s: f32,
    /// "Deep" dives for highlighting (m).
    pub deep_m: f32,
}

impl Default for DiveParams {
    fn default() -> Self {
        DiveParams {
            surface_m: 2.0,
            min_depth_m: 5.0,
            min_duration_s: 30.0,
            max_gap_s: 600.0,
            deep_m: 100.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    /// Long bottom phase (≥ 40 % of the dive).
    U,
    /// Bottom phase < 25 %.
    V,
    Intermediate,
}

impl Shape {
    pub fn label(self) -> &'static str {
        match self {
            Shape::U => "U",
            Shape::V => "V",
            Shape::Intermediate => "U/V",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dive {
    /// Start / end (unix s, interpolated at the surface threshold).
    pub start: f64,
    pub end: f64,
    pub max_depth: f32,
    pub t_max: i64,
    pub mean_depth: f32,
    pub bottom_s: f32,
    /// Vertical speeds (m/s) to / from 80 % of the maximum depth.
    pub descent_ms: f32,
    pub ascent_ms: f32,
    /// Time at the surface after the dive (s), until the next dive.
    pub post_surface_s: Option<f32>,
    pub shape: Shape,
    /// Position at the time of maximum depth (interpolated between fixes).
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// Time (s) to the nearest location fix — how well the position is known.
    pub fix_gap_s: Option<f32>,
    pub sun_elev: Option<f32>,
    /// Seafloor elevation at the dive position (m, negative), when sampled.
    pub seafloor_m: Option<f32>,
    /// Depth samples of the dive (unix s, m), surface crossings included.
    pub profile: Vec<(f64, f32)>,
}

impl Dive {
    pub fn duration_s(&self) -> f32 {
        (self.end - self.start) as f32
    }
    /// Reached the seafloor: within 15 m or 10 % of the local depth.
    pub fn benthic(&self) -> Option<bool> {
        let floor = -self.seafloor_m.filter(|f| f.is_finite() && *f < 0.0)?;
        Some(self.max_depth >= floor - (0.1 * floor).max(15.0))
    }
    pub fn is_night(&self) -> Option<bool> {
        self.sun_elev.map(|e| e < -6.0)
    }
}

/// Depth (m, positive down) → RGB: light cyan at the surface to deep navy, log scale (6000 m).
pub fn depth_rgb(d: f32) -> [u8; 3] {
    const STOPS: [[f32; 3]; 5] = [
        [190.0, 245.0, 255.0],
        [70.0, 200.0, 230.0],
        [30.0, 120.0, 210.0],
        [60.0, 70.0, 200.0],
        [70.0, 30.0, 140.0],
    ];
    let x = ((d.max(0.0) + 1.0).log10() / 3.778).clamp(0.0, 1.0) * (STOPS.len() - 1) as f32;
    let i = (x.floor() as usize).min(STOPS.len() - 2);
    let w = x - i as f32;
    let c = |k: usize| (STOPS[i][k] + (STOPS[i + 1][k] - STOPS[i][k]) * w) as u8;
    [c(0), c(1), c(2)]
}

/// Depth samples of a track (unix s, m positive down), sorted.
pub fn depth_samples(tr: &Track) -> Vec<(i64, f32)> {
    let mut v: Vec<(i64, f32)> = tr
        .sensors
        .iter()
        .filter(|s| s.kind == SensorKind::Depth && s.value.is_finite())
        .map(|s| (s.t, s.value))
        .collect();
    v.sort_by_key(|s| s.0);
    v.dedup_by_key(|s| s.0);
    v
}

/// Detect dives in a depth series.
pub fn detect(samples: &[(i64, f32)], p: &DiveParams) -> Vec<Dive> {
    let mut out: Vec<Dive> = Vec::new();
    let mut cur: Vec<(f64, f32)> = Vec::new();
    // a dive must start from a surface sample (not at the start of the record or after a gap)
    let mut cur_ok = false;
    let finish = |cur: &mut Vec<(f64, f32)>, out: &mut Vec<Dive>, ok: bool| {
        if cur.len() < 2 || !ok {
            cur.clear();
            return;
        }
        let pts = std::mem::take(cur);
        let (start, end) = (pts[0].0, pts[pts.len() - 1].0);
        let (imax, &(tmax, dmax)) = pts
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.1.total_cmp(&b.1.1))
            .unwrap();
        if dmax < p.min_depth_m || ((end - start) as f32) < p.min_duration_s {
            return;
        }
        // time-weighted mean depth and bottom time
        let thr = 0.8 * dmax;
        let (mut area, mut bottom) = (0.0f64, 0.0f64);
        for w in pts.windows(2) {
            let dt = w[1].0 - w[0].0;
            area += dt * 0.5 * (w[0].1 + w[1].1) as f64;
            if w[0].1 >= thr && w[1].1 >= thr {
                bottom += dt;
            }
        }
        let dur = (end - start).max(1.0);
        let first_bottom = pts.iter().find(|q| q.1 >= thr).map_or(tmax, |q| q.0);
        let last_bottom = pts.iter().rev().find(|q| q.1 >= thr).map_or(tmax, |q| q.0);
        let descent = thr as f64 / (first_bottom - start).max(1.0);
        let ascent = thr as f64 / (end - last_bottom).max(1.0);
        let frac = bottom / dur;
        let shape = if frac >= 0.4 {
            Shape::U
        } else if frac < 0.25 {
            Shape::V
        } else {
            Shape::Intermediate
        };
        let _ = imax;
        out.push(Dive {
            start,
            end,
            max_depth: dmax,
            t_max: tmax.round() as i64,
            mean_depth: (area / dur) as f32,
            bottom_s: bottom as f32,
            descent_ms: descent as f32,
            ascent_ms: ascent as f32,
            post_surface_s: None,
            shape,
            lat: None,
            lon: None,
            fix_gap_s: None,
            sun_elev: None,
            seafloor_m: None,
            profile: pts,
        });
    };
    let s = p.surface_m;
    for k in 0..samples.len() {
        let (t, d) = (samples[k].0 as f64, samples[k].1);
        let prev = k
            .checked_sub(1)
            .map(|j| (samples[j].0 as f64, samples[j].1));
        let gap = prev.is_some_and(|q| t - q.0 > p.max_gap_s as f64);
        if gap {
            // a data gap ends (and drops) an open dive: its end is unknown
            cur.clear();
        }
        if d > s {
            if cur.is_empty() {
                cur_ok = matches!(prev, Some((_, dp)) if !gap && dp <= s);
                // interpolate the start at the threshold
                let t0 = match prev {
                    Some((tp, dp)) if !gap && dp <= s && d > dp => {
                        tp + (t - tp) * ((s - dp) / (d - dp)) as f64
                    }
                    _ => t,
                };
                cur.push((t0, s));
            }
            cur.push((t, d));
        } else if !cur.is_empty() {
            let (tp, dp) = *cur.last().unwrap();
            let t1 = if dp > d {
                tp + (t - tp) * ((dp - s) / (dp - d)) as f64
            } else {
                t
            };
            cur.push((t1, s));
            finish(&mut cur, &mut out, cur_ok);
        }
    }
    // a dive still open at the end of the record is incomplete: dropped
    for i in 0..out.len().saturating_sub(1) {
        let gap = out[i + 1].start - out[i].end;
        if gap >= 0.0 && gap <= p.max_gap_s as f64 * 6.0 {
            out[i].post_surface_s = Some(gap as f32);
        }
    }
    out
}

/// Position at time `t` interpolated between the given fixes (sorted by time), and the time
/// to the nearest one.
pub fn position_at(fixes: &[&Fix], t: f64) -> Option<(f64, f64, f32)> {
    if fixes.is_empty() || t < fixes[0].t as f64 || t > fixes[fixes.len() - 1].t as f64 {
        return None;
    }
    let i = fixes.partition_point(|f| (f.t as f64) <= t);
    let (a, b) = if i == 0 {
        (fixes[0], fixes[0])
    } else if i >= fixes.len() {
        (fixes[fixes.len() - 1], fixes[fixes.len() - 1])
    } else {
        (fixes[i - 1], fixes[i])
    };
    let span = (b.t - a.t) as f64;
    let w = if span > 0.0 {
        (t - a.t as f64) / span
    } else {
        0.0
    };
    let (lat, lon) = crate::geo::interpolate(a.lat, a.lon, b.lat, b.lon, w);
    let gap = (t - a.t as f64).abs().min((b.t as f64 - t).abs());
    Some((lat, lon, gap as f32))
}

/// Give every dive a position and the sun's elevation there.
pub fn locate(dives: &mut [Dive], fixes: &[&Fix]) {
    for d in dives.iter_mut() {
        if let Some((la, lo, gap)) = position_at(fixes, d.t_max as f64) {
            d.lat = Some(la);
            d.lon = Some(lo);
            d.fix_gap_s = Some(gap);
            d.sun_elev = Some(crate::geo::sun_elevation_deg(d.t_max, la, lo) as f32);
        }
    }
}

/// Mean and maximum dive depth by hour of the day (UTC + `offset_h`), with dive counts.
pub fn by_hour(dives: &[Dive], offset_h: f64) -> Vec<(u8, usize, f32, f32)> {
    let mut acc = vec![(0usize, 0.0f32, 0.0f32); 24];
    for d in dives {
        let h = (((d.t_max as f64 / 3600.0 + offset_h).rem_euclid(24.0)) as usize).min(23);
        acc[h].0 += 1;
        acc[h].1 += d.max_depth;
        acc[h].2 = acc[h].2.max(d.max_depth);
    }
    acc.into_iter()
        .enumerate()
        .map(|(h, (n, s, m))| (h as u8, n, if n > 0 { s / n as f32 } else { f32::NAN }, m))
        .collect()
}

/// CSV of the dives (one row per dive).
pub fn dives_csv(track: &str, dives: &[Dive]) -> String {
    let mut s = String::from(
        "individual_local_identifier,dive,start,end,duration_s,max_depth_m,time_of_max,mean_depth_m,bottom_time_s,descent_rate_ms,ascent_rate_ms,post_dive_surface_s,shape,lat,lon,time_to_nearest_fix_s,sun_elevation_deg,seafloor_depth_m,benthic\n",
    );
    let ts = |t: f64| {
        chrono::DateTime::<chrono::Utc>::from_timestamp(t.round() as i64, 0)
            .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default()
    };
    let o = |v: Option<f32>, p: usize| v.map(|x| format!("{x:.p$}")).unwrap_or_default();
    for (k, d) in dives.iter().enumerate() {
        s.push_str(&format!(
            "{track},{},{},{},{:.0},{:.1},{},{:.1},{:.0},{:.3},{:.3},{},{},{},{},{},{},{},{}\n",
            k + 1,
            ts(d.start),
            ts(d.end),
            d.duration_s(),
            d.max_depth,
            ts(d.t_max as f64),
            d.mean_depth,
            d.bottom_s,
            d.descent_ms,
            d.ascent_ms,
            o(d.post_surface_s, 0),
            d.shape.label(),
            d.lat.map(|v| format!("{v:.5}")).unwrap_or_default(),
            d.lon.map(|v| format!("{v:.5}")).unwrap_or_default(),
            o(d.fix_gap_s, 0),
            o(d.sun_elev, 1),
            o(d.seafloor_m.filter(|f| f.is_finite()).map(|f| -f), 1),
            d.benthic().map(|b| b.to_string()).unwrap_or_default(),
        ));
    }
    s
}

/// McConnell et al. (1992) speed filter: `true` for fixes to drop. Fixes in `keep` (e.g.
/// GPS) are never dropped but still serve as neighbours.
pub fn speed_filter(fixes: &[Fix], vmax_ms: f64, protect: impl Fn(&Fix) -> bool) -> Vec<bool> {
    let n = fixes.len();
    let mut out = vec![false; n];
    if n < 5 || vmax_ms <= 0.0 {
        return out;
    }
    let speed = |a: &Fix, b: &Fix| {
        let dt = (b.t - a.t).abs().max(1) as f64;
        crate::geo::haversine_m(a.lat, a.lon, b.lat, b.lon) / dt
    };
    loop {
        let idx: Vec<usize> = (0..n).filter(|&i| !out[i]).collect();
        let mut worst: Option<(usize, f64)> = None;
        for k in 0..idx.len() {
            let i = idx[k];
            if protect(&fixes[i]) {
                continue;
            }
            let mut sq = 0.0;
            let mut m = 0;
            for o in [-2isize, -1, 1, 2] {
                let j = k as isize + o;
                if j >= 0 && (j as usize) < idx.len() {
                    sq += speed(&fixes[i], &fixes[idx[j as usize]]).powi(2);
                    m += 1;
                }
            }
            if m < 2 {
                continue;
            }
            let rms = (sq / m as f64).sqrt();
            if rms > vmax_ms && worst.is_none_or(|w| rms > w.1) {
                worst = Some((i, rms));
            }
        }
        match worst {
            Some((i, _)) => out[i] = true,
            None => return out,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LocationSource;

    /// 1 sample / 10 s: surface 60 s, V dive to 50 m (100 s down, 100 s up), surface, U dive
    /// to 200 m with a 400 s bottom.
    fn series() -> Vec<(i64, f32)> {
        let mut v = Vec::new();
        let mut t = 0;
        let mut push = |d: f32, t: &mut i64| {
            v.push((*t, d));
            *t += 10;
        };
        for _ in 0..6 {
            push(0.0, &mut t);
        }
        for k in 1..=10 {
            push(5.0 * k as f32, &mut t);
        }
        for k in (0..10).rev() {
            push(5.0 * k as f32, &mut t);
        }
        for _ in 0..12 {
            push(0.0, &mut t);
        }
        for k in 1..=20 {
            push(10.0 * k as f32, &mut t);
        }
        for _ in 0..40 {
            push(195.0, &mut t);
        }
        for k in (0..20).rev() {
            push(10.0 * k as f32, &mut t);
        }
        for _ in 0..6 {
            push(0.0, &mut t);
        }
        v
    }

    #[test]
    fn detects_and_classifies_dives() {
        let d = detect(&series(), &DiveParams::default());
        assert_eq!(d.len(), 2, "{d:?}");
        assert_eq!(d[0].max_depth, 50.0);
        assert_eq!(d[0].shape, Shape::V);
        assert_eq!(d[1].max_depth, 200.0);
        assert_eq!(d[1].shape, Shape::U);
        // start interpolated at 2 m between t = 50 (0 m) and 60 (5 m)
        assert!((d[0].start - 54.0).abs() < 1e-6, "{}", d[0].start);
        assert!(
            d[1].bottom_s > 400.0 && d[1].bottom_s < 500.0,
            "{}",
            d[1].bottom_s
        );
        assert!((d[1].descent_ms - 1.0).abs() < 0.05, "{}", d[1].descent_ms);
        assert!(d[0].post_surface_s.unwrap() > 100.0);
        assert!(d[1].post_surface_s.is_none());
        // too shallow
        let p = DiveParams {
            min_depth_m: 60.0,
            ..Default::default()
        };
        assert_eq!(detect(&series(), &p).len(), 1);
        let csv = dives_csv("seal", &d);
        assert_eq!(csv.lines().count(), 3);
        assert!(csv.lines().nth(2).unwrap().contains(",U,"));
    }

    #[test]
    fn gaps_drop_open_dives() {
        let mut s = series();
        // a 12-min gap in the U dive's bottom phase, then the record resumes mid-dive
        s.retain(|x| !(800..1520).contains(&x.0));
        s.extend([(1520, 120.0), (1530, 60.0), (1540, 0.0), (1550, 0.0)]);
        let d = detect(&s, &DiveParams::default());
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn location_benthic_and_hours() {
        let mut d = detect(&series(), &DiveParams::default());
        let a = Fix::new(0, 54.0, 7.0, LocationSource::Gps);
        let b = Fix::new(2000, 54.1, 7.0, LocationSource::Gps);
        locate(&mut d, &[&a, &b]);
        let la = d[1].lat.unwrap();
        assert!(la > 54.0 && la < 54.1);
        assert!(d[1].fix_gap_s.unwrap() > 0.0);
        d[1].seafloor_m = Some(-205.0);
        d[0].seafloor_m = Some(-3000.0);
        assert_eq!(d[1].benthic(), Some(true));
        assert_eq!(d[0].benthic(), Some(false));
        let h = by_hour(&d, 0.0);
        assert_eq!(h.len(), 24);
        assert_eq!(h.iter().map(|x| x.1).sum::<usize>(), 2);
    }

    #[test]
    fn speed_filter_removes_spikes() {
        let mut fx: Vec<Fix> = (0..10)
            .map(|i| Fix::new(i * 3600, 50.0 + i as f64 * 0.02, 0.0, LocationSource::Argos))
            .collect();
        fx[5].lat += 2.0; // a 220 km spike
        let out = speed_filter(&fx, 2.5, |_| false);
        assert_eq!(out.iter().filter(|x| **x).count(), 1);
        assert!(out[5]);
        // protected fixes stay
        let out = speed_filter(&fx, 2.5, |f| f.t == 5 * 3600);
        assert!(!out[5]);
    }
}
