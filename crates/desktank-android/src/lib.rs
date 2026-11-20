//! DeskTank Android host: `libdesktank_android.so`, driven over JNI by the Kotlin
//! shim in `android/` (SPEC §3.6, D23).
//!
//! The shim's `DreamService` owns a `SurfaceView` and forwards its surface
//! callbacks and the Dream's start and stop here. This crate turns the
//! `Surface` into an `ANativeWindow`, makes a wgpu surface from it and
//! clears it to the tank's water colour. That is the A0 skeleton: it proves
//! the GPU path and records which backend the device gets. Drawing the real
//! tank on a render thread is A1.
//!
//! The lifecycle rules and colour maths are OS-agnostic and tested on the
//! desktop; only the JNI and NDK glue is Android-only. On other targets
//! this crate compiles to an empty library so that `cargo build
//! --workspace` works on every OS.
// FFI crate: `unsafe` is allowed here, only at the JNI and NDK boundary, and
// every block needs a `// SAFETY:` comment (SPEC §4.1).
#![allow(unsafe_code)]

mod lifecycle;

#[cfg(target_os = "android")]
mod jni_host;

use desktank_render::wgpu;
use desktank_sim::MaterialTable;

pub use lifecycle::{Event, Lifecycle};

/// The material whose colour the A0 skeleton clears to.
const WATER: &str = "water";

/// The tank's water colour, sRGB-encoded, from the built-in materials.
///
/// # Errors
///
/// Returns an error if the built-in materials don't load (a build defect,
/// covered by `desktank-sim`'s tests) or have no water.
pub fn water_rgb() -> Result<[u8; 3], WaterError> {
    let materials = MaterialTable::builtin().map_err(|_| WaterError)?;
    materials
        .id(WATER)
        .and_then(|id| materials.get(id))
        .map(|m| m.colour)
        .ok_or(WaterError)
}

/// The built-in materials have no water colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaterError;

impl std::fmt::Display for WaterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the built-in materials have no water colour")
    }
}

impl std::error::Error for WaterError {}

/// The clear colour that shows `rgb` (sRGB-encoded) on a surface of this
/// format. wgpu takes clear colours in the attachment's linear space, so an
/// sRGB surface needs them decoded; a non-sRGB (`Unorm`) one stores the
/// value as given.
#[must_use]
pub fn clear_colour(rgb: [u8; 3], format: wgpu::TextureFormat) -> wgpu::Color {
    let channel = |c: u8| {
        let v = f64::from(c) / 255.0;
        if !format.is_srgb() {
            v
        } else if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    wgpu::Color {
        r: channel(rgb[0]),
        g: channel(rgb[1]),
        b: channel(rgb[2]),
        a: 1.0,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn water_is_the_material_colour() {
        // assets/materials/water.toml: colour = "#1f4e79".
        assert_eq!(water_rgb(), Ok([0x1f, 0x4e, 0x79]));
    }

    #[test]
    fn clear_colour_decodes_srgb_only_for_srgb_surfaces() {
        let unorm = clear_colour([0, 128, 255], wgpu::TextureFormat::Rgba8Unorm);
        assert!((unorm.g - 128.0 / 255.0).abs() < 1e-12);
        assert!((unorm.b - 1.0).abs() < 1e-12);

        let srgb = clear_colour([0, 128, 255], wgpu::TextureFormat::Rgba8UnormSrgb);
        assert!(srgb.r.abs() < 1e-12);
        // sRGB 128 is about 21.6% linear.
        assert!((srgb.g - 0.2158).abs() < 1e-3, "{}", srgb.g);
        assert!((srgb.b - 1.0).abs() < 1e-12);
        assert!((srgb.a - 1.0).abs() < 1e-12);
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
}
