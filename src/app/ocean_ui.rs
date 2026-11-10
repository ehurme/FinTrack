//! Side-panel section *Dives & ocean*: Argos filters, dive detection (always on, cached per
//! track), seafloor depth from the Terrarium bathymetry, and the Open-Meteo Marine download
//! (waves, currents, SST, sea level). Downloads only start from their buttons.

use super::*;
use crate::dives::{self, Dive};
use crate::marine;

/// Dive window of the 3D view.
pub(super) const DIVE_WINDOWS_H: [(f32, &str); 5] = [
    (3.0, "± 3 h"),
    (12.0, "± 12 h"),
    (24.0, "± 1 day"),
    (72.0, "± 3 days"),
    (0.0, "all dives"),
];

impl FinApp {
    /// Fixes used to place dives: the filtered location fixes of a track.
    fn located_fixes(&self, ti: usize) -> Vec<&Fix> {
        self.tracks[ti]
            .fixes
            .iter()
            .filter(|f| self.fix_visible(f))
            .collect()
    }

    fn dive_fp(&self, ti: usize) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let tr = &self.tracks[ti];
        tr.sensors.len().hash(&mut h);
        tr.fixes.len().hash(&mut h);
        tr.fixes.first().map(|f| f.t).hash(&mut h);
        tr.fixes.last().map(|f| f.t).hash(&mut h);
        format!("{:?}", self.dive_params).hash(&mut h);
        self.min_lc.hash(&mut h);
        self.speed_filter_ms.to_bits().hash(&mut h);
        self.max_error_km.to_bits().hash(&mut h);
        tr.fixes
            .iter()
            .filter(|f| f.speed_outlier)
            .count()
            .hash(&mut h);
        h.finish()
    }

    /// Re-run the speed filter on every track (GPS fixes are never removed).
    pub(super) fn apply_speed_filter(&mut self) {
        let v = self.speed_filter_ms as f64;
        for tr in self.tracks.iter_mut() {
            let out = dives::speed_filter(&tr.fixes, v, |f| f.source == LocationSource::Gps);
            for (f, o) in tr.fixes.iter_mut().zip(out) {
                f.speed_outlier = o;
            }
        }
    }

    /// Keep the per-track dives up to date (cheap; runs every frame on changed tracks).
    pub(super) fn ensure_dives(&mut self) {
        let keys: Vec<(usize, String)> = (0..self.tracks.len())
            .map(|i| (i, self.track_key(i)))
            .collect();
        self.dive_cache
            .retain(|k, _| keys.iter().any(|(_, kk)| kk == k));
        for (ti, key) in keys {
            let fp = self.dive_fp(ti);
            if self.dive_cache.get(&key).is_some_and(|c| c.0 == fp) {
                continue;
            }
            let samples = dives::depth_samples(&self.tracks[ti]);
            let mut d = dives::detect(&samples, &self.dive_params);
            dives::locate(&mut d, &self.located_fixes(ti));
            self.dive_cache.insert(key, (fp, d));
        }
    }

    /// Dives of the visible tracks inside the 3D dive window.
    pub(super) fn dive_paths_3d(&self) -> Vec<crate::view3d::DivePath> {
        let mut out = Vec::new();
        let win = self.dive_window_h as f64 * 3600.0;
        for ti in 0..self.tracks.len() {
            let tr = &self.tracks[ti];
            if !tr.visible || self.selected_track.is_some_and(|s| s != ti) {
                continue;
            }
            let Some((_, ds)) = self.dive_cache.get(&self.track_key(ti)) else {
                continue;
            };
            let fixes = self.located_fixes(ti);
            let col = Color32::from_rgb(tr.color[0], tr.color[1], tr.color[2]);
            for d in ds {
                if win > 0.0 && (d.t_max as f64 - self.t_now).abs() > win {
                    continue;
                }
                if self.trail_to_cursor && d.start > self.t_now {
                    continue;
                }
                if self.deep_only && d.max_depth < self.dive_params.deep_m {
                    continue;
                }
                // thin long profiles to ~40 points
                let step = (d.profile.len() / 40).max(1);
                let pts: Vec<(f64, f64, f32)> = d
                    .profile
                    .iter()
                    .step_by(step)
                    .chain(d.profile.last())
                    .filter_map(|&(t, z)| {
                        dives::position_at(&fixes, t).map(|(la, lo, _)| (la, lo, z))
                    })
                    .collect();
                if pts.len() < 2 {
                    continue;
                }
                out.push(crate::view3d::DivePath {
                    pts,
                    max_depth: d.max_depth,
                    color: col,
                    deep: d.max_depth >= self.dive_params.deep_m,
                    current: d.start <= self.t_now && d.end >= self.t_now,
                });
            }
        }
        out
    }

    /// Dive markers for the 2D map: (lat, lon, max depth, duration min, deep, track colour).
    pub(super) fn dive_markers(&self) -> Vec<(f64, f64, f32, f32, bool)> {
        if !self.show_dives {
            return Vec::new();
        }
        let mut out = Vec::new();
        for ti in 0..self.tracks.len() {
            let tr = &self.tracks[ti];
            if !tr.visible || self.selected_track.is_some_and(|s| s != ti) {
                continue;
            }
            let Some((_, ds)) = self.dive_cache.get(&self.track_key(ti)) else {
                continue;
            };
            for d in ds {
                if self.trail_to_cursor && d.start > self.t_now {
                    continue;
                }
                let deep = d.max_depth >= self.dive_params.deep_m;
                if self.deep_only && !deep {
                    continue;
                }
                if let (Some(la), Some(lo)) = (d.lat, d.lon) {
                    out.push((la, lo, d.max_depth, d.duration_s() / 60.0, deep));
                }
            }
        }
        out
    }

    /// Sample the seafloor under every fix and dive once the user asked for it.
    pub(super) fn pump_bathy(&mut self, ctx: &egui::Context) {
        if !self.bathy_pending {
            return;
        }
        let bathy = self.bathy.clone();
        let mut s = bathy.sampler(BATHY_ZOOM);
        let mut todo = 0usize;
        for tr in self.tracks.iter_mut().filter(|t| t.visible) {
            for f in tr
                .fixes
                .iter_mut()
                .filter(|f| f.weather.seafloor_m.is_none())
            {
                match s.elev_exact(f.lat, f.lon) {
                    Some(e) => f.weather.seafloor_m = Some(e),
                    None => todo += 1,
                }
            }
        }
        for (_, ds) in self.dive_cache.values_mut() {
            for d in ds.iter_mut().filter(|d| d.seafloor_m.is_none()) {
                if let (Some(la), Some(lo)) = (d.lat, d.lon) {
                    match s.elev_exact(la, lo) {
                        Some(e) => d.seafloor_m = Some(e),
                        None => todo += 1,
                    }
                }
            }
        }
        let missing = s.missing();
        bathy.finish(s, ctx);
        if todo == 0 && bathy.loading() == 0 {
            self.bathy_pending = false;
            let n: usize = self
                .tracks
                .iter()
                .flat_map(|t| t.fixes.iter())
                .filter(|f| f.weather.seafloor_m.is_some_and(|v| v.is_finite()))
                .count();
            self.status(format!("Seafloor depth sampled for {n} fixes"));
        } else if missing > 0 || bathy.loading() > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    fn start_marine(&mut self) {
        let pts = marine::points(&self.tracks);
        let latest = chrono::Utc::now().timestamp().div_euclid(86400);
        let reqs = self.marine.plan(&pts, latest);
        if reqs.is_empty() {
            let n = marine::annotate(&mut self.tracks, &self.marine);
            self.status(format!("Ocean data cached – annotated {n} fixes"));
            return;
        }
        self.marine_failed = 0;
        let n = reqs.len();
        self.marine_queue.extend(reqs.into_iter().map(|r| (r, 0)));
        self.status(format!(
            "Downloading ocean data (Open-Meteo Marine): {n} request(s)"
        ));
    }

    pub(super) fn pump_marine(&mut self, ctx: &egui::Context) {
        if self.now < self.marine_pause_until {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
            return;
        }
        while self.marine_inflight < 2 {
            let Some((req, tries)) = self.marine_queue.pop_front() else {
                return;
            };
            self.marine_inflight += 1;
            let (tx, ctx2) = (self.tx.clone(), ctx.clone());
            let r = ehttp::Request::get(&req.url);
            ehttp::fetch(r, move |res| {
                let result = match res {
                    Ok(r) if r.ok => r
                        .text()
                        .map(str::to_string)
                        .ok_or_else(|| "non-UTF8 body".into()),
                    Ok(r) => Err(format!(
                        "HTTP {} {}",
                        r.status,
                        r.text()
                            .map(|t| t.chars().take(300).collect::<String>())
                            .unwrap_or_default()
                    )),
                    Err(e) => Err(e),
                };
                let _ = tx.send(Msg::Marine { req, tries, result });
                ctx2.request_repaint();
            });
        }
    }

    pub(super) fn on_marine(
        &mut self,
        req: marine::Request,
        tries: u8,
        result: Result<String, String>,
    ) {
        self.marine_inflight = self.marine_inflight.saturating_sub(1);
        match result.and_then(|b| self.marine.ingest(&req, &b)) {
            Ok(_) => {}
            Err(e) => {
                let transient =
                    e.starts_with("HTTP 429") || e.starts_with("HTTP 5") || !e.starts_with("HTTP");
                if transient && tries < 4 {
                    self.marine_pause_until = self.now + 20.0 * (tries as f64 + 1.0);
                    self.marine_queue.push_back((req, tries + 1));
                } else {
                    self.marine.mark_done(&req);
                    self.marine_failed += 1;
                    self.status(format!("Ocean data request failed: {}", short_err(&e)));
                }
            }
        }
        if self.marine_queue.is_empty() && self.marine_inflight == 0 {
            let n = marine::annotate(&mut self.tracks, &self.marine);
            self.status(format!(
                "Ocean data: annotated {n} fixes{}",
                if self.marine_failed > 0 {
                    format!(" ({} request(s) failed)", self.marine_failed)
                } else {
                    String::new()
                }
            ));
        }
    }

    pub(super) fn ocean_panel(&mut self, ui: &mut Ui) {
        // ---- location filters
        ui.strong("Location filters");
        ui.horizontal(|ui| {
            ui.label("Argos classes ≥");
            egui::ComboBox::from_id_salt("min_lc")
                .selected_text(self.min_lc.label())
                .show_ui(ui, |ui| {
                    for lc in ArgosLc::ALL {
                        ui.selectable_value(&mut self.min_lc, lc, lc.label());
                    }
                });
        })
        .response
        .on_hover_text("Hide Argos fixes below this location class (3 best … B worst, Z invalid)");
        ui.horizontal(|ui| {
            ui.label("speed filter");
            let old = self.speed_filter_ms;
            ui.add(
                egui::DragValue::new(&mut self.speed_filter_ms)
                    .range(0.0..=30.0)
                    .speed(0.05)
                    .suffix(" m/s"),
            )
            .on_hover_text(
                "McConnell et al. (1992): drop Argos fixes whose RMS speed to the two previous and \
                 two next fixes exceeds this maximum swimming speed (0 = off; GPS fixes are kept).",
            );
            if old != self.speed_filter_ms {
                self.apply_speed_filter();
            }
            let n: usize = self
                .tracks
                .iter()
                .map(|t| t.fixes.iter().filter(|f| f.speed_outlier).count())
                .sum();
            if n > 0 {
                ui.weak(format!("{n} removed"));
            }
        });

        // ---- dives
        ui.separator();
        ui.strong("Dives");
        let n_depth: usize = self
            .tracks
            .iter()
            .map(|t| {
                t.sensors
                    .iter()
                    .filter(|s| s.kind == SensorKind::Depth)
                    .count()
            })
            .sum();
        if n_depth == 0 {
            ui.weak("No depth data loaded (Movebank 'depth' / 'tdr-depth' or a depth column).");
        }
        let p = &mut self.dive_params;
        egui::Grid::new("dive_params")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("surface above");
                ui.add(
                    egui::DragValue::new(&mut p.surface_m)
                        .range(0.1..=50.0)
                        .speed(0.1)
                        .suffix(" m"),
                );
                ui.end_row();
                ui.label("min. dive depth");
                ui.add(
                    egui::DragValue::new(&mut p.min_depth_m)
                        .range(0.5..=2000.0)
                        .speed(0.5)
                        .suffix(" m"),
                );
                ui.end_row();
                ui.label("min. duration");
                ui.add(
                    egui::DragValue::new(&mut p.min_duration_s)
                        .range(0.0..=3600.0)
                        .speed(1.0)
                        .suffix(" s"),
                );
                ui.end_row();
                ui.label("gap ends a dive");
                ui.add(
                    egui::DragValue::new(&mut p.max_gap_s)
                        .range(10.0..=86400.0)
                        .speed(10.0)
                        .suffix(" s"),
                );
                ui.end_row();
                ui.label("deep dives from");
                ui.add(
                    egui::DragValue::new(&mut p.deep_m)
                        .range(1.0..=3000.0)
                        .speed(1.0)
                        .suffix(" m"),
                );
                ui.end_row();
            });
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.show_dives, "dives on the map");
            ui.checkbox(&mut self.deep_only, "deep dives only");
        });
        ui.horizontal(|ui| {
            ui.label("3D dive window");
            egui::ComboBox::from_id_salt("dive_win")
                .selected_text(
                    DIVE_WINDOWS_H
                        .iter()
                        .find(|w| w.0 == self.dive_window_h)
                        .map(|w| w.1)
                        .unwrap_or("custom"),
                )
                .show_ui(ui, |ui| {
                    for (h, l) in DIVE_WINDOWS_H {
                        ui.selectable_value(&mut self.dive_window_h, h, l);
                    }
                });
        });
        // summary for the selected (or each visible) track
        let rows: Vec<(String, Vec<Dive>)> = (0..self.tracks.len())
            .filter(|&i| self.tracks[i].visible && self.selected_track.is_none_or(|s| s == i))
            .filter_map(|i| {
                let d = self.dive_cache.get(&self.track_key(i))?;
                (!d.1.is_empty()).then(|| (self.tracks[i].id.clone(), d.1.clone()))
            })
            .collect();
        if !rows.is_empty() {
            egui::Grid::new("dive_summary")
                .striped(true)
                .show(ui, |ui| {
                    for h in ["", "dives", "max", "median max", "deep", "night"] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    for (id, d) in &rows {
                        let mut m: Vec<f32> = d.iter().map(|x| x.max_depth).collect();
                        m.sort_by(|a, b| a.total_cmp(b));
                        let deep = d
                            .iter()
                            .filter(|x| x.max_depth >= self.dive_params.deep_m)
                            .count();
                        let night = d.iter().filter(|x| x.is_night() == Some(true)).count();
                        ui.label(id);
                        ui.label(d.len().to_string());
                        ui.label(format!("{:.0} m", m.last().copied().unwrap_or(0.0)));
                        ui.label(format!("{:.0} m", m[m.len() / 2]));
                        ui.label(format!("{:.0} %", 100.0 * deep as f32 / d.len() as f32));
                        ui.label(format!("{:.0} %", 100.0 * night as f32 / d.len() as f32));
                        ui.end_row();
                    }
                });
            // depth by hour of the day, local solar time at the dives
            if let Some((_, d)) = rows.first() {
                let lon = d.iter().filter_map(|x| x.lon).sum::<f64>()
                    / d.iter().filter(|x| x.lon.is_some()).count().max(1) as f64;
                let h = dives::by_hour(d, lon / 15.0);
                let bars: Vec<egui_plot::Bar> = h
                    .iter()
                    .filter(|x| x.1 > 0)
                    .map(|x| {
                        egui_plot::Bar::new(x.0 as f64 + 0.5, -(x.2 as f64))
                            .width(0.9)
                            .fill(crate::view3d::depth_color(x.2, 230))
                    })
                    .collect();
                ui.weak(format!(
                    "{}: mean maximum dive depth by local solar hour",
                    rows[0].0
                ));
                Plot::new("dive_hours")
                    .height(110.0)
                    .allow_drag(false)
                    .allow_zoom(false)
                    .allow_scroll(false)
                    .include_y(0.0)
                    .x_axis_label("hour")
                    .y_axis_label("m")
                    .show(ui, |pui| {
                        pui.bar_chart(egui_plot::BarChart::new("depth", bars));
                    });
            }
            if ui
                .button("Export dives (CSV)")
                .on_hover_text("One row per dive: depth, duration, bottom time, rates, shape, position, sun, seafloor, benthic")
                .clicked()
            {
                let mut csv = String::new();
                for (id, d) in &rows {
                    let part = dives::dives_csv(id, d);
                    if csv.is_empty() {
                        csv = part;
                    } else {
                        csv.extend(part.lines().skip(1).map(|l| format!("{l}\n")));
                    }
                }
                self.save_csv("fintrack_dives.csv", csv);
            }
        }

        // ---- bathymetry
        ui.separator();
        ui.strong("Bathymetry");
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!self.bathy_pending, egui::Button::new("Sample seafloor depth"))
                .on_hover_text(
                    "Seafloor under every fix and dive from the free AWS Terrain Tiles \
                     (ETOPO1 / SRTM15+ / GEBCO-derived in the ocean, ~150 m tiles). Marks dives that \
                     reached the seafloor (benthic).",
                )
                .clicked()
            {
                self.bathy_pending = true;
            }
            if self.bathy_pending {
                ui.spinner();
                ui.weak(format!("{} tiles loading", self.bathy.loading()));
            }
        });
        let benthic: usize = self
            .dive_cache
            .values()
            .flat_map(|d| d.1.iter())
            .filter(|d| d.benthic() == Some(true))
            .count();
        if benthic > 0 {
            ui.weak(format!(
                "{benthic} dives reached the seafloor (within 15 m / 10 %)"
            ));
        }

        // ---- ocean state
        ui.separator();
        ui.strong("Waves, currents & SST");
        ui.weak(
            "Open-Meteo Marine (free, no key): waves, swell, surface currents, SST and sea level \
             since 2022 (Météo-France SMOC, 1/12°); before 2022 waves only (ERA5-Ocean).",
        );
        ui.horizontal_wrapped(|ui| {
            let busy = !self.marine_queue.is_empty() || self.marine_inflight > 0;
            if ui
                .add_enabled(!busy, egui::Button::new("Annotate with ocean data"))
                .clicked()
            {
                self.start_marine();
            }
            if busy {
                ui.spinner();
                ui.weak(format!(
                    "{} request(s) to go",
                    self.marine_queue.len() + self.marine_inflight
                ));
                if ui.small_button("Cancel").clicked() {
                    self.marine_queue.clear();
                }
            } else if !self.marine.is_empty() {
                ui.weak(format!("{} cell-hours cached", self.marine.hours()));
            }
        });
        ui.weak(
            "Wind at 10 m and 2 m: 'Wind & weather → Download & annotate (ERA5)'; 2 m uses a \
             logarithmic profile over the sea.",
        );
    }

    /// Share value of the ocean settings.
    pub(super) fn ocean_share(&self) -> String {
        let p = &self.dive_params;
        format!(
            "{},{},{},{},{},{},{},{},{},{}",
            ArgosLc::ALL
                .iter()
                .position(|l| *l == self.min_lc)
                .unwrap_or(6),
            self.speed_filter_ms,
            p.surface_m,
            p.min_depth_m,
            p.min_duration_s,
            p.max_gap_s,
            p.deep_m,
            self.show_dives as u8,
            self.deep_only as u8,
            self.dive_window_h,
        )
    }

    pub(super) fn ocean_apply(&mut self, v: &[f64]) {
        if let [lc, sp, su, md, mdur, gap, deep, show, donly, win, ..] = v[..] {
            self.min_lc = ArgosLc::ALL[(lc as usize).min(7)];
            self.speed_filter_ms = sp.clamp(0.0, 30.0) as f32;
            self.dive_params = dives::DiveParams {
                surface_m: su as f32,
                min_depth_m: md as f32,
                min_duration_s: mdur as f32,
                max_gap_s: gap as f32,
                deep_m: deep as f32,
            };
            self.show_dives = show != 0.0;
            self.deep_only = donly != 0.0;
            self.dive_window_h = win as f32;
            self.apply_speed_filter();
        }
    }
}

/// Seafloor sampling zoom (~150 m pixels).
pub(super) const BATHY_ZOOM: u8 = 10;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dives_ocean_and_bathymetry_end_to_end() {
        let mut a = FinApp::blank();
        a.load_demo();
        assert_eq!(a.tracks.len(), 2);
        a.ensure_dives();
        let seal = (0..2)
            .find(|&i| a.tracks[i].species.as_deref() == Some("Mirounga angustirostris"))
            .unwrap();
        let ds = &a.dive_cache[&a.track_key(seal)].1;
        assert!(ds.len() > 500, "{} dives", ds.len());
        let max = ds.iter().map(|d| d.max_depth).fold(0.0f32, f32::max);
        assert!(max > 450.0, "{max}");
        assert!(ds.iter().filter(|d| d.lat.is_some()).count() > ds.len() / 2);
        // the cache is reused, and rebuilt when a parameter changes
        let fp = a.dive_cache[&a.track_key(seal)].0;
        a.ensure_dives();
        assert_eq!(a.dive_cache[&a.track_key(seal)].0, fp);
        a.dive_params.min_depth_m = 300.0;
        a.ensure_dives();
        assert!(a.dive_cache[&a.track_key(seal)].1.len() < ds_len(&a, seal, 0));
        // 3D paths and map markers
        a.dive_window_h = 0.0;
        a.t_now = a.t_max;
        assert!(!a.dive_paths_3d().is_empty());
        assert!(!a.dive_markers().is_empty());
        a.deep_only = true;
        a.dive_params.deep_m = 10_000.0;
        assert!(a.dive_markers().is_empty());
        a.deep_only = false;
        // Argos class filter and speed filter
        let all = a.tracks[seal]
            .fixes
            .iter()
            .filter(|f| a.fix_visible(f))
            .count();
        a.min_lc = ArgosLc::Lc1;
        let good = a.tracks[seal]
            .fixes
            .iter()
            .filter(|f| a.fix_visible(f))
            .count();
        assert!(good < all && good > 0);
        a.min_lc = ArgosLc::Z;
        a.speed_filter_ms = 2.0;
        a.apply_speed_filter();
        let gps = (0..2).find(|&i| i != seal).unwrap();
        assert!(a.tracks[gps].fixes.iter().all(|f| !f.speed_outlier));

        // ocean data: answer every planned request with a constant field
        a.start_marine();
        let reqs: Vec<_> = a.marine_queue.drain(..).collect();
        assert!(!reqs.is_empty());
        for (req, _) in reqs {
            let one = |t0: i64| {
                format!(
                    r#"{{"hourly_units":{{"ocean_current_velocity":"km/h"}},"hourly":{{"time":[{}],"wave_height":[{}],"wave_direction":[{}],"wave_period":[{}],"swell_wave_height":[{}],"ocean_current_velocity":[{}],"ocean_current_direction":[{}],"sea_surface_temperature":[{}],"sea_level_height_msl":[{}]}}}}"#,
                    (0..(req.end_day - req.start_day + 1) * 24)
                        .map(|h| (t0 + h * 3600).to_string())
                        .collect::<Vec<_>>()
                        .join(","),
                    rep("1.5", req.end_day - req.start_day + 1),
                    rep("270", req.end_day - req.start_day + 1),
                    rep("7", req.end_day - req.start_day + 1),
                    rep("1.0", req.end_day - req.start_day + 1),
                    rep("1.8", req.end_day - req.start_day + 1),
                    rep("90", req.end_day - req.start_day + 1),
                    rep("12.5", req.end_day - req.start_day + 1),
                    rep("0.3", req.end_day - req.start_day + 1),
                )
            };
            let body = format!(
                "[{}]",
                req.cells
                    .iter()
                    .map(|_| one(req.start_day * 86400))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            a.marine_inflight += 1;
            a.on_marine(req, 0, Ok(body));
        }
        let m = a.tracks[seal].fixes[3].weather.marine.unwrap();
        assert_eq!(m.sst_c, Some(12.5));
        assert!((m.current_ms.unwrap() - 0.5).abs() < 1e-5);
        assert!(a.log.last().unwrap().contains("annotated"));

        // settings round-trip through a share link
        a.dive_params.surface_m = 3.0;
        a.dive_window_h = 72.0;
        let mut b = FinApp::blank();
        b.apply_share_settings(&a.share_map(false));
        assert_eq!(b.ocean_share(), a.ocean_share());
    }

    fn rep(s: &str, days: i64) -> String {
        vec![s; (days * 24) as usize].join(",")
    }

    fn ds_len(a: &FinApp, ti: usize, _: u8) -> usize {
        let samples = dives::depth_samples(&a.tracks[ti]);
        dives::detect(&samples, &dives::DiveParams::default()).len()
    }
}
