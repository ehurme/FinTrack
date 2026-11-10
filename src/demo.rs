//! Synthetic demo datasets, written as Movebank exports so they exercise the same parser as
//! real data. **Simulated, not real animals:**
//!
//! * a northern elephant seal leaving Año Nuevo (California) on its post-breeding trip:
//!   Argos locations (classes 3 … B with error ellipses for the better classes) and 1-min
//!   depth with continuous deep foraging dives (300–700 m, ~20 min);
//! * a grey seal in the German Bight around Helgoland: Fastloc GPS every ~2 h and 1-min
//!   depth with shallow benthic dives (20–45 m) between offshore wind farms.

use crate::geo::destination;
use chrono::{DateTime, Utc};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
    fn normal(&mut self) -> f64 {
        let (u1, u2) = (self.next().max(1e-12), self.next());
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

fn iso(t: i64) -> String {
    DateTime::<Utc>::from_timestamp(t, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M:%S.000").to_string())
        .unwrap_or_default()
}

/// Dive behaviour of a simulated animal.
struct DiveStyle {
    /// Mean / SD of the dive's maximum depth (m) by day and by night.
    depth_day: (f64, f64),
    depth_night: (f64, f64),
    /// Mean dive duration (min) and surface interval (min).
    dive_min: f64,
    surface_min: f64,
    /// Vertical speed (m/min).
    rate: f64,
}

/// Depth every minute (m, positive down), capped by the local seafloor depth.
fn dive_series(
    rng: &mut Rng,
    t0: i64,
    minutes: i64,
    style: &DiveStyle,
    floor: impl Fn(i64) -> f64,
) -> Vec<(i64, f64)> {
    let mut out = Vec::with_capacity(minutes as usize);
    let mut m = 0i64;
    while m < minutes {
        // surface interval
        let si = (style.surface_min * (0.6 + 0.8 * rng.next()))
            .round()
            .max(1.0) as i64;
        for _ in 0..si {
            if m >= minutes {
                break;
            }
            out.push((t0 + m * 60, (0.3 * rng.next()).max(0.0)));
            m += 1;
        }
        let t = t0 + m * 60;
        let hour = (t.rem_euclid(86400)) / 3600;
        // local solar time roughly UTC − 8 h off California, UTC + 0.5 h in the North Sea
        let night = !(7..19).contains(&hour);
        let (mu, sd) = if night {
            style.depth_night
        } else {
            style.depth_day
        };
        let maxd = (mu + sd * rng.normal())
            .max(8.0)
            .min(floor(t) - 1.0)
            .max(3.0);
        let dur = (style.dive_min * (0.7 + 0.6 * rng.next())).round().max(3.0) as i64;
        let desc = (maxd / style.rate).ceil().max(1.0) as i64;
        let asc = desc;
        let bottom = (dur - desc - asc).max(1);
        for k in 0..desc + bottom + asc {
            if m >= minutes {
                break;
            }
            let d = if k < desc {
                maxd * (k + 1) as f64 / desc as f64
            } else if k < desc + bottom {
                maxd * (0.85 + 0.15 * rng.next())
            } else {
                maxd * (desc + bottom + asc - k - 1) as f64 / asc as f64
            };
            out.push((t0 + m * 60, d + 0.5 * rng.normal()));
            m += 1;
        }
    }
    out
}

/// Elephant seal: Argos + deep dives, heading WNW from Año Nuevo into the Pacific.
pub fn elephant_seal_csv() -> String {
    let mut s = String::from(
        "event-id,visible,timestamp,location-long,location-lat,sensor-type,argos:lc,argos:semi-major,argos:semi-minor,argos:orientation,argos:error-radius,depth,external-temperature,individual-local-identifier,individual-taxon-canonical-name,tag-local-identifier\n",
    );
    let t0: i64 = 1_709_251_200; // 2024-03-01 00:00 UTC
    let days = 18;
    let mut rng = Rng(42);
    // true path every hour
    let (mut lat, mut lon) = (37.10, -122.35);
    let mut path = Vec::new();
    for h in 0..days * 24 {
        let hd = 280.0 + 25.0 * rng.normal();
        let km = 2.6 + 0.8 * rng.normal();
        let (la, lo) = destination(lat, lon, hd, (km * 1000.0).max(0.0));
        lat = la;
        lon = lo;
        path.push((t0 + h * 3600, lat, lon));
    }
    let style = DiveStyle {
        depth_day: (520.0, 90.0),
        depth_night: (380.0, 80.0),
        dive_min: 21.0,
        surface_min: 2.2,
        rate: 70.0,
    };
    // over the continental slope / abyssal plain: floor ~ 3000–4000 m except near the coast
    let depth = dive_series(&mut rng, t0, days * 24 * 60, &style, |t| {
        if t - t0 < 6 * 3600 { 80.0 } else { 3500.0 }
    });
    let mut id = 0;
    let mut rows: Vec<(i64, String)> = Vec::new();
    for &(t, lat, lon) in &path {
        // ~6 Argos locations a day, mostly at the surface between dives
        if rng.next() > 0.26 {
            continue;
        }
        let r = rng.next();
        let (lc, err) = match r {
            x if x < 0.07 => ("3", 250.0),
            x if x < 0.17 => ("2", 500.0),
            x if x < 0.32 => ("1", 1500.0),
            x if x < 0.45 => ("0", 4000.0),
            x if x < 0.75 => ("A", 6000.0),
            _ => ("B", 10000.0),
        };
        let (nla, nlo) = destination(lat, lon, rng.next() * 360.0, err * rng.normal().abs());
        let ellipse = if ["3", "2", "1"].contains(&lc) {
            let a = err * (1.5 + rng.next());
            let b = a * (0.15 + 0.3 * rng.next());
            format!(
                "{a:.0},{b:.0},{:.0},{:.0}",
                rng.next() * 180.0,
                ((a * a + b * b) / 2.0).sqrt()
            )
        } else {
            ",,,".to_string()
        };
        let tt = t + (rng.next() * 1800.0) as i64;
        id += 1;
        rows.push((
            tt,
            format!(
                "{id},true,{},{nlo:.5},{nla:.5},argos-doppler-shift,{lc},{ellipse},,,NES-2024-07,Mirounga angustirostris,PTT-2070\n",
                iso(tt)
            ),
        ));
    }
    for (t, d) in depth {
        id += 1;
        let temp = if d < 50.0 {
            13.5 - d * 0.02
        } else {
            9.5 - (d - 50.0) * 0.009
        };
        rows.push((
            t,
            format!(
                "{id},true,{},,,accessory-measurements,,,,,,{d:.1},{temp:.2},NES-2024-07,Mirounga angustirostris,PTT-2070\n",
                iso(t)
            ),
        ));
    }
    rows.sort_by_key(|r| r.0);
    for (_, r) in rows {
        s.push_str(&r);
    }
    s
}

/// Grey seal: Fastloc GPS + shallow benthic dives in the German Bight.
pub fn grey_seal_csv() -> String {
    let mut s = String::from(
        "event-id,visible,timestamp,location-long,location-lat,sensor-type,gps:satellite-count,depth,external-temperature,individual-local-identifier,individual-taxon-canonical-name,tag-local-identifier\n",
    );
    let t0: i64 = 1_719_792_000; // 2024-07-01 00:00 UTC
    let days = 14;
    let mut rng = Rng(7);
    // foraging trips from Helgoland Düne towards the offshore wind farms N and NW of the island
    let home = (54.187, 7.915);
    let targets = [(54.38, 7.68), (54.32, 7.30), (54.46, 7.95), (54.05, 7.45)];
    let (mut lat, mut lon) = home;
    let mut path = Vec::new();
    let mut trip = 0usize;
    let mut leg_out = true;
    for h in 0..days * 24 {
        let tgt = if leg_out {
            targets[trip % targets.len()]
        } else {
            home
        };
        let d = crate::geo::haversine_m(lat, lon, tgt.0, tgt.1);
        if d < 2500.0 {
            if leg_out {
                // forage locally for a while, then head home
                if rng.next() < 0.12 {
                    leg_out = false;
                }
            } else {
                // haul out for a few hours before the next trip
                if rng.next() < 0.2 {
                    leg_out = true;
                    trip += 1;
                }
            }
        }
        let step = if d < 2500.0 { 600.0 } else { 3500.0 };
        let hd = crate::geo::bearing_deg(lat, lon, tgt.0, tgt.1) + 20.0 * rng.normal();
        let (la, lo) = destination(lat, lon, hd, step * (0.6 + 0.6 * rng.next()));
        lat = la;
        lon = lo;
        path.push((t0 + h * 3600, lat, lon, d < 2500.0 && !leg_out));
    }
    let style = DiveStyle {
        depth_day: (34.0, 6.0),
        depth_night: (30.0, 7.0),
        dive_min: 6.0,
        surface_min: 1.5,
        rate: 45.0,
    };
    let depth = dive_series(&mut rng, t0, days * 24 * 60, &style, |_| 42.0);
    let mut rows: Vec<(i64, String)> = Vec::new();
    let mut id = 0;
    for &(t, lat, lon, hauled) in &path {
        if rng.next() > 0.55 {
            continue;
        }
        let (nla, nlo) = destination(lat, lon, rng.next() * 360.0, 25.0 * rng.normal().abs());
        let tt = t + (rng.next() * 900.0) as i64;
        id += 1;
        rows.push((
            tt,
            format!(
                "{id},true,{},{nlo:.6},{nla:.6},gps,{},,,GS-HEL-03,Halichoerus grypus,FL-0331\n",
                iso(tt),
                5 + (rng.next() * 6.0) as i32 + if hauled { 2 } else { 0 }
            ),
        ));
    }
    // no diving while hauled out near Helgoland
    let hauled_at = |t: i64| {
        let i = ((t - t0) / 3600) as usize;
        path.get(i)
            .is_some_and(|p| !p.3 && crate::geo::haversine_m(p.1, p.2, home.0, home.1) < 2500.0)
    };
    for (t, d) in depth {
        id += 1;
        let d = if hauled_at(t) { 0.0 } else { d };
        rows.push((
            t,
            format!(
                "{id},true,{},,,accessory-measurements,,{d:.1},{:.2},GS-HEL-03,Halichoerus grypus,FL-0331\n",
                iso(t),
                if d < 15.0 { 17.0 } else { 14.0 }
            ),
        ));
    }
    rows.sort_by_key(|r| r.0);
    for (_, r) in rows {
        s.push_str(&r);
    }
    s
}

pub fn all() -> Vec<(&'static str, String)> {
    vec![
        ("demo_movebank_elephant_seal_argos.csv", elephant_seal_csv()),
        ("demo_movebank_grey_seal_gps.csv", grey_seal_csv()),
    ]
}
