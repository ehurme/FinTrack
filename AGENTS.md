# FinTrack — agent guide

FinTrack is an interactive tracker for marine animals written in Rust — the ocean twin of WingTrack
(github.com/ehurme/WingTrack), from which it was copied in Nov 2026 and now evolves independently. It reads GPS and
Argos tracks with depth / temperature / acceleration data (Movebank and generic CSV), detects dives and shows them in
3D over the seafloor, and annotates fixes with bathymetry, Open-Meteo Marine ocean state (waves, currents, SST, sea
level) and ERA5 weather; map layers add GIBS SST / chlorophyll, EMODnet ship density and offshore wind farms. It runs
as a native desktop app (Linux, macOS, Windows) and, from the same source, in the browser via WebAssembly
(https://ehurme.github.io/FinTrack/).

This file is written for AI coding agents. Assume the reader knows nothing about the project.

## Technology stack

- **Language:** Rust, edition 2024, minimum toolchain 1.95.
- **Build system:** Cargo.
- **UI framework:** egui + eframe. The map is rendered with `walkers`; charts with `egui_plot`.
- **Web build:** `trunk`, target `wasm32-unknown-unknown`.
- **HTTP:** `ehttp` (works on native and WASM).
- **CSV parsing:** `csv` + custom tolerant header normalisation in `src/table.rs`.
- **Date/time:** `chrono`.
- **Video export:** `gif` crate for animated GIF; MP4 is produced by piping raw RGBA frames to `ffmpeg` (desktop only).
- **Ocean data:** Open-Meteo Marine API (`marine.rs`; SMOC currents / SST / sea level from 2022, ERA5-Ocean waves
  from 1940); bathymetry from the AWS Terrarium tiles (`terrain.rs`, negative below sea level).
- **Weather data:** Open-Meteo Historical Weather API (ERA5) or the Copernicus CDS with the user's key; 2 m wind is
  derived from 10 m with a neutral log profile over the sea (`marine::wind_2m_from_10m`).
- **Tile sources:** Esri World Imagery / Topo, OpenStreetMap, CARTO dark; NASA GIBS (true colour WMTS; SST,
  chlorophyll, precipitation via WMS GetMap); EMODnet Human Activities and GEBCO WMS overlays (`tiles::Overlay`).
- `wind3d.rs` (pressure-level winds) is inherited from WingTrack and kept compiling, but its UI is not exposed.

## Repository layout

```
.
├── Cargo.toml              # Rust package manifest
├── Cargo.lock              # Pinned dependencies
├── index.html              # trunk entry point for the WASM build
├── .github/workflows/ci.yml# CI: test, clippy, desktop builds, GitHub Pages deploy
├── README.md               # User-facing documentation
├── assets/screenshot.png   # README asset
├── src/
│   ├── main.rs             # Native / WASM entry points
│   ├── lib.rs              # Public module exports
│   ├── acc.rs              # Acceleration: e-obs / per-sample / FleaTag readers, burst features (flea port), k-means, per-fix summaries
│   ├── app.rs              # egui application: map, timeline, charts, export, video
│   ├── app/movement_ui.rs  # Movement-analysis side panel, summary exports
│   ├── app/ocean_ui.rs     # Dives / bathymetry / ocean panel: Argos filters, dive cache, seafloor sampling, Marine queue
│   ├── dives.rs            # Dive detection & stats, dive positions, by-hour summary, dives CSV, McConnell speed filter
│   ├── marine.rs           # Open-Meteo Marine: request planning (0.1° cells, per month), cache, annotation, 2 m wind
│   ├── model.rs            # Core data model: Track, Fix, SensorSample, WeatherAnno
│   ├── structures.rs       # Wind turbines / tall structures: Overpass + CSV import, encounter analysis, CSV exports
│   ├── table.rs            # Tolerant CSV reader (delimiter sniffing, header normalisation, timestamp parsing)
│   ├── terrain.rs          # AWS Terrarium elevation tiles for the 3D view: fetch, decode, bilinear sampling
│   ├── cds.rs              # Copernicus CDS ERA5 time-series client (user's API key)
│   ├── movement.rs         # On-demand movement analyses: variogram, IID/OU/OUF ML fits (Kalman), home ranges, FPT, HMM, change points
│   ├── prefs.rs            # Per-device settings kept out of share links (API keys)
│   ├── firmware/           # One module per data source
│   │   ├── mod.rs          # Firmware trait, registry, shared helpers
│   │   ├── movebank.rs     # Movebank GPS + Argos (class, ellipse) + depth/temperature/light rows
│   │   ├── generic.rs      # Any lat/lon/time CSV (optional lc / depth / temperature)
│   │   └── tests.rs        # Parser unit tests
│   ├── weather.rs          # Open-Meteo request planning, cache, ERA5 annotation
│   ├── wxfield.rs          # ERA5 colour fields, isobars (marching squares), lows/highs + storm flag
│   ├── cyclones.rs         # IBTrACS tropical cyclones: ERDDAP subset / CSV import, interpolation, categories
│   ├── geo.rs              # Great-circle maths, solar elevation, barometric altitude, wind vectors
│   ├── tiles.rs            # Basemap and NASA GIBS tile sources
│   ├── video.rs            # Frame capture/cropping, MP4 (ffmpeg) and GIF encoding
│   ├── wind3d.rs           # Pressure-level profiles: Open-Meteo + ERA5-cube sources, flight-level annotation, curtains
│   ├── view3d.rs           # CPU-projected 3D view: ground tiles, wind raster, level planes, particles, tracks at altitude
│   ├── share.rs            # Share links: URL-fragment encoding, data embedding (gzip + base64url)
│   ├── export.rs           # Annotated CSV export (Movebank-like column names)
│   └── demo.rs             # Simulated demo datasets in raw export formats
└── tools/ibtracs_years.py # Python (CI): per-year IBTrACS CSVs published with the web app
```

## Build, run, and test commands

All commands run from the repository root.

### Native desktop

```bash
# Build and run (release mode is strongly recommended; debug is very slow)
cargo run --release

# Compile only
cargo build --release

# Run with the simulated demo files already loaded
FINTRACK_DEMO=1 cargo run --release        # macOS / Linux / Git Bash
$env:FINTRACK_DEMO=1; cargo run --release  # PowerShell
set FINTRACK_DEMO=1 && cargo run --release # cmd

# Offline synthetic wind field for UI testing (not ERA5)
FINTRACK_SYNTHETIC_WIND=1 cargo run --release

# Demo with the dive views open (3D tab or map)
FINTRACK_DEMO=1 FINTRACK_DIVES=3d cargo run --release
```

The compiled binary is written to `target/release/fintrack` (`target/release/fintrack.exe` on Windows).

### WebAssembly

```bash
rustup target add wasm32-unknown-unknown
cargo install --locked trunk
trunk serve --release
```

Then open http://127.0.0.1:8080.

### Tests

```bash
# Library unit tests only (parsers, weather batching/annotation, geodesy, export round-trip, video encoding)
cargo test --lib

# Clippy (CI-enforced)
cargo clippy --all-targets -- -D warnings
```

### Video export

MP4 export needs `ffmpeg` on PATH. GIF export works everywhere. On Windows the app spawns `ffmpeg` with the `CREATE_NO_WINDOW` flag.

## Code organisation and architecture

### Data flow

1. `table.rs` reads raw bytes into a `Table` with normalised headers and tolerant timestamp parsing.
2. `firmware/mod.rs` ranks all `Firmware` profiles by `detect()` confidence and calls the best match's `parse()`.
3. Each parser produces `model::Track`s: one track per animal/tag, containing `Fix`es (locations) and `SensorSample`s (non-location sensor streams).
4. `Track::finalize()` sorts and deduplicates fixes/sensors and computes step distances, bearings, ground speeds and `step_significant`.
5. `weather.rs` plans Open-Meteo requests, caches responses in `WeatherStore`, and annotates every fix with wind, weather, along-step wind support and ERA5-corrected barometric altitude.
6. `wind3d.rs` keeps pressure-level profiles in a `ProfileStore` (cell → hour → per-level u, v, geopotential height,
   temperature, plus surface pressure) and annotates fixes with `FlightWind` (flight-level and best-level wind,
   named as in `SigfoxTagPrep::annotate_era5()`). `view3d.rs` draws the *3D winds* tab from a `FieldSlice`.
7. `app.rs` renders the map, timeline, charts and export UI; it drives asynchronous HTTP weather fetches and video recording.
8. `export.rs` writes a single CSV with all derived columns (including the flight-level ones).

### Key abstractions

- `firmware::Firmware` trait (`src/firmware/mod.rs:26`): the extension point for new tag types. Implement `id`, `label`, `description`, `detect` and `parse`, then add the struct to `registry()`.
- `model::Fix`: one location fix. Carries source (`Gps`, `Argos`, `Station`, `Other`; every non-GPS source draws its error: the Argos ellipse when `argos` has one, else the `error_m` circle), `argos: Option<ArgosInfo>` (class, ellipse), `depth_m`, `speed_outlier` (McConnell filter), step metrics and a `WeatherAnno` (incl. `marine: Option<MarineAnno>` and `seafloor_m`, negative = depth).
- `model::SensorSample`: non-location measurement (`Depth` positive down, `Temperature`, `Light`, `Vedba`, …) with a timestamp so it can be plotted.
- `model::ArgosLc` is ordered Z < B < A < 0 < 1 < 2 < 3 < G; `fix_visible` / `ViewCfg::visible` hide fixes below `min_lc`, beyond the max error, or `speed_outlier`.
- `weather::WeatherStore`: cell → hour → values cache. Bins points into 0.25° ERA5 grid cells; requests are grouped by calendar month, up to 25 cells per call.
- `wind3d::ProfileStore`: pressure-level cache shared by both 3D-wind sources. `sample()` tries the exact 0.25° cell,
  then bilinear interpolation between regional-field nodes spaced `node_step` cells apart. Levels below the ground
  (level pressure > surface pressure) are masked.
- `view3d::View3D`: camera, particles and display options; `show()` takes a `Scene` borrowed from the app each frame.
  Everything is projected on the CPU and painted with egui meshes/lines, so it needs no GPU code and runs on WASM.
- Wind fields (2D surface and 3D pressure levels) are fetched lazily around the animals with
  `weather::window_points()` (cells within a radius of each animal's position, a week at a time from the cursor day).
  `app.rs::pump_weather()` throttles requests by their estimated Open-Meteo call weight (`weather::call_weight`:
  locations × max(1, vars/10) × max(1, days/14)) to 450/min and 4200/h, and re-queues jobs after HTTP 429.
- Share links (`src/share.rs` + `FinApp::share_map` / `apply_share_settings` / `apply_share_view` in `app.rs`):
  the dashboard state as `key=value` pairs in the URL fragment. **When adding a user-visible setting, add it to
  `share_map` and the matching apply function** (enums are encoded by their `Debug` name; unknown/missing keys are
  ignored, so links stay backward compatible). `app::share_tests` round-trips a full state.
  Exception: API keys and the ERA5 source (`cds_key`, `om_key`, `era5_source`) are per-user secrets kept in
  `prefs.rs` (localStorage on the web, a config file on desktop) and must never go into share links.
- ERA5 via Copernicus CDS (`cds.rs`): one async job per 0.25° cell (`cds::plan`), driven by `FinApp::pump_cds` /
  `on_cds_response` (submit → poll → results → download ZIP/CSV → `WeatherStore`). Open-Meteo requests have a
  watchdog (`OM_TIMEOUT_S`); HTTP 429 reasons "daily"/"hourly" stop or pause the queue (`om_blocked_until`, `pause_until`).
- `table::Table`: tolerant column lookup by aliases (`t.col(&[...])`) and prefix (`t.col_prefix(...)`).

## Development conventions

- **Edition and MSRV:** Rust 2024, `rust-version = "1.95"` in `Cargo.toml`.
- **Spelling:** British English in comments and documentation (`colour`, `behaviour`, `centre`), although egui API names use American spelling.
- **Formatting:** Use `cargo fmt`. CI runs `cargo clippy --all-targets -- -D warnings`.
- **Documentation:** Modules have `//!` doc comments explaining their role. Keep them in sync with code changes.
- **Error handling:** Parser errors are returned as `String`. Background tasks send status messages through an `mpsc` channel to the UI thread.
- **Platform conditionals:** Use `#[cfg(not(target_arch = "wasm32"))]` for native-only code (file dialogs, `ffmpeg`, threads) and `#[cfg(target_arch = "wasm32")]` for web-only code (`rfd` async file dialog, `web_sys` downloads).
- **Dependencies:** confirm a crate is in `Cargo.toml` before using it; match the version and feature set already in use. The project avoids heavy web frameworks and uses egui's immediate-mode GUI idioms.

## Testing strategy

- Unit tests live inline in `#[cfg(test)] mod tests` blocks within each source file.
- `src/firmware/tests.rs` exercises every firmware profile with small synthetic CSVs, including real-world header variants (semicolon delimiters, unit suffixes, BOMs).
- `src/table.rs` tests header normalisation, delimiter sniffing, split-header merging and timestamp formats.
- `src/geo.rs` tests haversine distances, bearings, solar elevation, wind components and barometric altitude.
- `src/weather.rs` tests request planning, JSON ingestion, annotation and synthetic-field generation.
- `src/export.rs` tests a round-trip through the generic parser.
- `src/video.rs` tests frame cropping/downscaling, GIF round-trip and (if `ffmpeg` is available) MP4 encoding.
- `src/tiles.rs` tests tile URL generation.
- `src/wind3d.rs` tests Open-Meteo request URLs/planning, JSON ingestion, ln-p and height interpolation, underground
  masking, flight-level annotation, curtains, field slices and CSV/gzip cube import. An ignored test reads a real cube:
  `FINTRACK_CUBE=cube.csv.gz cargo test --lib -- --ignored cube_file --nocapture`.
- `src/view3d.rs` tests the camera projection and tile maths.
- There are no integration tests or UI tests; the desktop and web apps are exercised manually.

## CI and deployment

`.github/workflows/ci.yml` runs on every push to `main` and on pull requests:

1. **test** job on Ubuntu: `cargo test --lib` and `cargo clippy --all-targets -- -D warnings`.
2. **desktop** job on Ubuntu/macOS/Windows: `cargo build --release` and uploads binaries as artifacts (`fintrack-<os>`).
3. **web** job on Ubuntu: installs the WASM target and `trunk`, runs `trunk build --release --public-url ./`, and uploads `dist/` as a Pages artifact.
4. **deploy-pages** job deploys the web artifact to GitHub Pages (only from `main`).

Enable GitHub Pages via *Settings → Pages → Source: GitHub Actions*.

## Security considerations

- The app fetches remote tile images, satellite imagery and Open-Meteo weather data over HTTPS. No API keys are required.
- File I/O is limited to user-selected CSV files via `rfd` and explicit export paths.
- `ffmpeg` is spawned as a child process for MP4 export; the path is fixed to `"ffmpeg"` and no user input reaches the command line beyond the output file path chosen by the save dialog.
- Web downloads use `Blob`/`URL.createObjectURL` in the browser; no server-side component exists.
- No secrets, credentials or private keys are stored in the repository. `.gitignore` excludes `/target` and `/dist`.

## How to add a firmware profile

1. Add a new module under `src/firmware/`.
2. Implement `firmware::Firmware`:
   - `id()`: stable identifier used in overrides and exports.
   - `label()`: shown in the per-file dropdown.
   - `description()`: one-line tooltip.
   - `detect(t: &Table) -> f32`: confidence 0..1 from headers/rows.
   - `parse(t: &Table) -> Result<ParseOutput, String>`: build `Track`s.
3. Add the struct to `firmware::registry()` in `src/firmware/mod.rs`.
4. Add a unit test in `src/firmware/tests.rs` with a minimal representative CSV.
5. Update this `AGENTS.md` and `README.md` if the feature is user-facing.

## Current state

`main` builds and passes `cargo test --lib` and `cargo clippy --all-targets -- -D warnings`. The 3D-winds feature
(`wind3d.rs`, `view3d.rs`, curtain/profile charts, `tools/era5_wind_cube.py`) was added on top of the
"wind tracks and video" commit.

## Useful environment variables

| Variable | Effect |
| --- | --- |
| `FINTRACK_DEMO=1` | Load simulated demo files on startup |
| `FINTRACK_SYNTHETIC_WIND=1` | Fill the weather store with a rotating synthetic wind field, and the pressure-level store with a sheared synthetic profile field, for offline UI testing (not real data) |
| `FINTRACK_3D=1` / `=profile` | With the synthetic wind: open the *3D winds* tab with the curtain (or profile) chart |
| `FINTRACK_CUBE=path` | Path for the ignored real-cube import test |
| `FINTRACK_MOVE=1` | Run all movement analyses on the longest track at start and open the panel (UI testing) |
| `FINTRACK_DIVES=3d` / `=map` | With the demo: select the first track, depth charts, the cursor at a deep dive; `3d` opens the 3D dives tab close to the animal |

## External resources

- Repository: https://github.com/ehurme/FinTrack
- Open-Meteo Marine API: https://open-meteo.com/en/docs/marine-weather-api
- EMODnet Human Activities WMS: https://ows.emodnet-humanactivities.eu/wms
- Open-Meteo Historical Weather API: https://open-meteo.com/en/docs/historical-weather-api
- Open-Meteo Historical Forecast API (pressure levels): https://open-meteo.com/en/docs/historical-forecast-api
- NASA GIBS: https://earthdata.nasa.gov/gibs
- egui: https://github.com/emilk/egui
- walkers: https://github.com/podusowski/walkers

- `table::Table` repairs R `write.csv` rows where an unquoted `c(...)`/`list(...)` vector split across fields (only when a row has more fields than headers).
- `Track::set_date_filter` moves fixes/sensors outside the range into `excluded_fixes`/`excluded_sensors` (restored on change) and recomputes steps; the app wrapper `FinApp::set_date_filter` also fixes the time range, selection and caches. Shared as `dr=key:from-to;…`.
- 3D terrain (`View3D::terrain`, share `v3` field 11): `terrain::Terrain::sampler()` borrows loaded tiles for one frame and records missing ones; `finish()` downloads them. The CPU painter has no depth buffer, so ground cells/tiles are drawn far-to-near (`ground_grid`).
- `structures.rs`: OSM structures per 0.5° corridor tile (`corridor_tiles`, one Overpass request at a time in
  `FinApp::pump_structures`), a 0.05° grid index, `analyse()` → `Analysis` (encounters + per-fix counts), cached in
  `FinApp::st_cache` keyed on tracks / store version / params. Drawn by `TrackLayer` (2D) and `view3d` (`Scene::structures`).
  Shared as `st=radius,other,min_height,highlight`.
- `acc.rs`: `AccSet` per animal and file (bursts in g; FleaTag times relative to a user-set start × clock rate),
  `features()` mirrors flea's `burst_features()`, `analyse()` → features + flight + clusters (one k-means over all
  animals, fitted on ≤ 20 000 bursts), `summarise_track()` → `Fix::acc`. In the app, ACC files never go through the
  firmware registry (`acc::sniff` first in `add_file_from`), live in `acc_files` / `acc_sets`, and `ensure_acc_cache`
  separates the expensive analysis (sets + params) from per-fix annotation (tracks). Share: `acc=` params, `accs=`
  per-set start / clock rate (ppm) / track; local ACC files are not embedded.
- Weather layers: `wxfield::grid()` builds the lattice of ERA5 cells at the wind-field spacing for one hour; `WxDraw`
  (app) is rebuilt per frame for the 2D map. Turning a layer on also drives `maybe_request_surface_field`. Cyclones
  (`cyclones.rs`) come from ERDDAP `IBTrACS_since1980_1` (CSV with a units row) or an imported NCEI file; share keys
  `wxl=` (field,isobars,centres) and `tc=1` (re-fetch). GIBS weather imagery (`SatLayer::ImergPrecip`, AIRS) uses WMS
  GetMap per tile (`tiles.rs`); true colour stays on WMTS.
- Open-Meteo queue errors are classified by `om_failure()`: daily / hourly 429 stop or pause, transient errors (5xx,
  overloaded, network, timeouts, minute limit) back off globally and re-queue (≤ `OM_RETRIES`, `om_parallel` → 1),
  then go to `failed_jobs` (never `mark_done`); only permanent 4xx are marked done. IBTrACS per-year CSVs are built in
  CI (`tools/ibtracs_years.py` → `dist/data/ibtracs/`, weekly schedule) and read before ERDDAP.
- Charts (`series_plot`): the x axis is managed by the app, not by egui_plot's linked axes — `chart_x_view` is the
  user's zoom / pan window (`None` = fit all data, double-click resets), `chart_full_x` the union of everything plotted
  last frame (a change resets the zoom); y is set every frame to the data inside the visible window. Zoom / drag are x only.
- Movement analyses (`movement.rs`, UI in `app/movement_ui.rs`) never run automatically. Per-track
  results live in `FinApp::mv_ui.results` keyed by `track_key`, invalidated by a fingerprint of the fixes
  (count, first / last time); per-fix output is `Fix::mv` (`MoveAnno`), cleared by `set_date_filter`. ctmm conventions:
  σ is the per-axis variance, area = −2 ln(1−q) π σ, OUF speed = √(π/2)√(σ/(τp τv)), error radius = RMS (per-axis
  variance r²/2), DOF(area) = 1/Var(ln σ̂).  Share key `mv=` (settings).
  `FINTRACK_DEMO=1 FINTRACK_MOVE=1` runs every analysis on the longest demo track for UI checks.
- Dives (`dives.rs`, `app/ocean_ui.rs`): `FinApp::ensure_dives` runs every frame and rebuilds `dive_cache`
  (track key → (fingerprint, dives)) when the depth samples, fixes, dive parameters or location filters change; a dive
  must start from a surface sample and end at one (gaps > `max_gap_s` drop it). Positions use the *filtered* fixes
  (`located_fixes`). `dive_paths_3d` (window `dive_window_h` around the cursor, profiles thinned to ~40 points) feeds
  `view3d::Scene::dives`; `dive_markers` feeds the 2D `TrackLayer`. Depth colours: `dives::depth_rgb` (log scale).
- Seafloor sampling: *Sample seafloor depth* sets `bathy_pending`; `pump_bathy` samples fixes and dives with
  `terrain::Sampler::elev_exact` at zoom 10 (None until the tile loads; failed tiles give NaN) and stops when nothing is left.
- Open-Meteo Marine: `start_marine` plans requests (`MarineStore::plan`, ≤ 40 cells, one calendar month, pre-2022 months
  waves-only with `models=era5_ocean`), `pump_marine` keeps ≤ 2 in flight, `on_marine` retries 429 / 5xx / network
  errors 4 times with a pause, then annotates all fixes (`marine::annotate`; current speed converted with the response's
  `hourly_units`, direction = towards).
- 3D view in FinTrack: tracks are drawn at the sea surface (`max(ground, 0)`), terrain is on by default and keeps
  negative elevations (seafloor), auto exaggeration targets the deepest dive at ~15 % of the camera distance.
- Share keys added in FinTrack: `oc=` (min Argos class index, speed filter, dive params, show dives, deep only, dive
  window) and `ov=` (overlay name, opacity).
