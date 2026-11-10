//! FinTrack — interactive tracker for marine animals: GPS and Argos tracks, dive profiles in
//! 3D over the seafloor, and ocean / weather annotation (waves, currents, SST, wind,
//! bathymetry) with satellite layers, ship traffic and offshore wind farms.
//!
//! The crate is split so the data side can be used without the GUI:
//! * [`table`] + [`firmware`] — read CSV exports and turn them into [`model::Track`]s
//! * [`weather`] — download ERA5 via Open-Meteo and annotate fixes with wind support etc.
//! * [`dives`] — dive detection and statistics from depth records
//! * [`marine`] — Open-Meteo Marine (waves, currents, SST) annotation
//! * [`movement`] — on-demand home range, speed and segmentation
//! * [`export`] — write annotated tracks back to CSV (readable by R)
//! * [`video`] — MP4 / GIF export of the animated map and charts
//! * [`app`] — the egui application (native and web)

pub mod acc;
pub mod app;
pub mod cds;
pub mod cyclones;
pub mod demo;
pub mod dives;
pub mod export;
pub mod firmware;
pub mod geo;
pub mod marine;
pub mod model;
pub mod movement;
pub mod prefs;
pub mod share;
pub mod structures;
pub mod table;
pub mod terrain;
pub mod tiles;
pub mod video;
pub mod view3d;
pub mod weather;
pub mod wind3d;
pub mod wxfield;

pub use app::FinApp;
