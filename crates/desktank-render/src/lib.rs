//! DeskTank renderer: grid texture and palette shader (sprites and lighting
//! come later). Draws a [`desktank_sim`] world to a surface or an offscreen
//! target.
//!
//! The core is loop-agnostic (SPEC §4.2): a host owns the window or canvas,
//! the event loop and surface creation, and drives a [`Tank`] by calling
//! [`Tank::tick`] with its clock and [`Tank::draw`] when asked to. The same
//! `Tank` serves the winit window, the saver hosts and the web.
//!
//! The renderer stays within wgpu's WebGL2 downlevel limits (SPEC §3.5): no
//! compute shaders, no storage buffers, `Rg8Unorm` / `Rgba8Unorm` textures
//! read with `textureLoad` (nearest by construction), and a device created
//! with [`wgpu::Limits::downlevel_webgl2_defaults`].
//!
//! OS-agnostic; any `unsafe` needed for raw-handle surface creation belongs in
//! `desktank-platform` (SPEC §4.1).
#![forbid(unsafe_code)]

mod gpu;
mod grid;
mod hud;
mod layout;
mod offscreen;
mod pacing;
mod palette;
mod renderer;
mod surface;
mod tank;
mod timer;

/// The wgpu version this crate is built against, for hosts that create the
/// instance and surfaces.
pub use wgpu;

pub use gpu::{Gpu, GpuError, GpuOptions};
pub use hud::{BrushPreview, Palette, PaletteLayout};
pub use layout::Layout;
pub use offscreen::CaptureError;
pub use pacing::FrameRequest;
pub use surface::{SurfaceError, TankSurface};
pub use tank::{DrawError, Tank, TankConfig};
