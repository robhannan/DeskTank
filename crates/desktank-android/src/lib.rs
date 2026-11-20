//! DeskTank Android host: `libdesktank_android.so`, driven over JNI by the Kotlin
//! shim in `android/` (SPEC §3.6, D23).
//!
//! The shim's `DreamService` owns a `SurfaceView` and forwards its surface
//! callbacks and the Dream's start and stop here, each tagged with the
//! Dream's token. This crate turns the `Surface` into an `ANativeWindow`,
//! makes a wgpu surface from it and draws the tank on a render thread it
//! owns, paced by the tank's frame pacer (A1).
//!
//! The lifecycle rules, grid choice and log filter are OS-agnostic and
//! tested on the desktop; only the JNI, NDK and render-thread glue is
//! Android-only. On other targets this crate compiles to an empty library
//! so that `cargo build --workspace` works on every OS.
// FFI crate: `unsafe` is allowed here, only at the JNI and NDK boundary, and
// every block needs a `// SAFETY:` comment (SPEC §4.1).
#![allow(unsafe_code)]

mod lifecycle;

#[cfg(target_os = "android")]
mod jni_host;

use desktank_render::{even_scale, wgpu};
use desktank_sim::{MaterialError, MaterialTable, World, WorldConfig, WorldError, scene};
use log::{Level, LevelFilter};

pub use lifecycle::{DreamId, Dreams, Event, Lifecycle};

/// Cells across and down at the default 4× scale on a 1080p TV (D1, SPEC
/// §3.6): the grid a TV tank aims for at any resolution.
pub const TV_CELLS: (u32, u32) = (480, 270);

/// Smallest and largest TV cell scale (D19: even scales from 2 to 8).
const SCALES: (u32, u32) = (2, 8);

/// Fewest cells per side, as the desktop window allows.
const MIN_SIDE: u32 = 16;

/// Seed for a new tank, the desktop's default.
pub const SEED: u64 = 7;

/// Creatures a new tank is stocked with (the §6.1 reference load).
pub const STOCK: usize = 30;

/// A grid and the scale it is drawn at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TvGrid {
    /// Screen pixels per cell side: even, 2 to 8.
    pub scale: u32,
    /// Cells across and down.
    pub cells: (u32, u32),
}

/// The grid for a new tank on a `width`×`height` pixel surface (SPEC §3.6):
/// the largest even scale (2 to 8) at which [`TV_CELLS`] fit, and as many
/// cells as cover the surface at that scale. 1080p gives 4× and 480×270
/// cells, 4K 8× and the same cells, so a tank looks the same on any TV.
#[must_use]
pub fn tv_grid(width: u32, height: u32) -> TvGrid {
    let fit = (width / TV_CELLS.0).min(height / TV_CELLS.1);
    let scale = even_scale(fit.clamp(SCALES.0, SCALES.1));
    let max = u32::try_from(desktank_sim::MAX_SIDE).unwrap_or(u32::MAX);
    let cells = |px: u32| (px / scale).clamp(MIN_SIDE, max);
    TvGrid {
        scale,
        cells: (cells(width), cells(height)),
    }
}

/// A new tank of `grid`'s cells: the starter scene (sand, gravel, rocks)
/// stocked with [`STOCK`] creatures, from [`SEED`].
///
/// # Errors
///
/// Returns an error if the built-in tables don't load (a build defect,
/// covered by `desktank-sim`'s tests) or the grid is out of range (never for
/// a [`tv_grid`]).
pub fn new_world(grid: TvGrid) -> Result<World, NewWorldError> {
    let materials = MaterialTable::builtin()?;
    let config = WorldConfig {
        width: grid.cells.0 as usize,
        height: grid.cells.1 as usize,
        seed: SEED,
        ..WorldConfig::default()
    };
    let mut world = World::new(config, materials)?;
    scene::starter(&mut world, SEED)?;
    scene::stock(&mut world, STOCK, SEED)?;
    Ok(world)
}

/// Error making a new tank.
#[derive(Debug, thiserror::Error)]
pub enum NewWorldError {
    /// The built-in materials don't load.
    #[error("built-in materials: {0}")]
    Materials(#[from] MaterialError),
    /// The world rejected the grid or the starter scene.
    #[error("new world: {0}")]
    World(#[from] WorldError),
}

/// The wgpu backends to try, in order, from the `debug.desktank.backend`
/// system property (`adb shell setprop debug.desktank.backend gl`): `vulkan`
/// or `gl` forces one; anything else (or unset) tries Vulkan, then GLES
/// (SPEC §3.6).
#[must_use]
pub fn backend_order(property: Option<&str>) -> &'static [wgpu::Backends] {
    match property.map(str::trim) {
        Some("vulkan") => &[wgpu::Backends::VULKAN],
        Some("gl" | "gles") => &[wgpu::Backends::GL],
        _ => &[wgpu::Backends::VULKAN, wgpu::Backends::GL],
    }
}

/// The log level the `debug.desktank.log` system property asks for
/// (`adb shell setprop debug.desktank.log debug`): `off`, `error`, `warn`,
/// `info`, `debug` or `trace` applies to every crate; unset (or anything
/// else) keeps the default of [`log_enabled`].
#[must_use]
pub fn log_override(property: Option<&str>) -> Option<LevelFilter> {
    property.and_then(|p| p.trim().parse().ok())
}

/// Whether a `level` record from module `target` goes to logcat: with an
/// override, anything at or above it; by default this crate's info and up
/// and everything else's (wgpu's) warnings and errors.
#[must_use]
pub fn log_enabled(target: &str, level: Level, over: Option<LevelFilter>) -> bool {
    let max = over.unwrap_or(if target.starts_with(env!("CARGO_CRATE_NAME")) {
        LevelFilter::Info
    } else {
        LevelFilter::Warn
    });
    level <= max
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tv_grid_keeps_the_1080p_tank_on_any_tv() {
        let hd = TvGrid {
            scale: 4,
            cells: (480, 270),
        };
        assert_eq!(tv_grid(1920, 1080), hd);
        assert_eq!(
            tv_grid(3840, 2160),
            TvGrid {
                scale: 8,
                cells: (480, 270)
            }
        );
        // 1440p: 5× fits, rounded down to an even 4×.
        assert_eq!(
            tv_grid(2560, 1440),
            TvGrid {
                scale: 4,
                cells: (640, 360)
            }
        );
        // 720p and smaller: never below 2×.
        assert_eq!(
            tv_grid(1280, 720),
            TvGrid {
                scale: 2,
                cells: (640, 360)
            }
        );
        assert_eq!(
            tv_grid(10, 10),
            TvGrid {
                scale: 2,
                cells: (16, 16)
            }
        );
        // Never above 8×.
        assert_eq!(tv_grid(7680, 4320).scale, 8);
    }

    #[test]
    fn new_world_is_the_stocked_starter_scene() {
        let world = new_world(tv_grid(1920, 1080)).unwrap();
        assert_eq!((world.width(), world.height()), (480, 270));
        assert_eq!(world.creatures().len(), STOCK);
        let sand = scene::material(&world, "sand").unwrap();
        assert!(world.material_counts()[usize::from(sand.0)] > 0);
    }

    #[test]
    fn backend_property_forces_one_or_tries_vulkan_first() {
        use wgpu::Backends;
        assert_eq!(backend_order(Some("vulkan")), [Backends::VULKAN]);
        assert_eq!(backend_order(Some("gl")), [Backends::GL]);
        assert_eq!(backend_order(Some(" gles\n")), [Backends::GL]);
        let both = [Backends::VULKAN, Backends::GL];
        assert_eq!(backend_order(None), both);
        assert_eq!(backend_order(Some("")), both);
        assert_eq!(backend_order(Some("metal")), both);
    }

    #[test]
    fn logs_our_info_and_wgpu_warnings_unless_overridden() {
        let ours = "desktank_android::jni_host";
        assert!(log_enabled(ours, Level::Info, None));
        assert!(!log_enabled(ours, Level::Debug, None));
        assert!(log_enabled("wgpu_hal::gles::egl", Level::Warn, None));
        assert!(!log_enabled("wgpu_core::device", Level::Info, None));

        let debug = log_override(Some("debug\n"));
        assert_eq!(debug, Some(LevelFilter::Debug));
        assert!(log_enabled("wgpu_core::device", Level::Debug, debug));
        assert!(!log_enabled(ours, Level::Trace, debug));
        assert!(!log_enabled(ours, Level::Error, log_override(Some("off"))));
        assert_eq!(log_override(Some("loud")), None);
        assert_eq!(log_override(None), None);
    }
}
