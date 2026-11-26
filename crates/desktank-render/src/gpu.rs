//! Adapter and device selection.

use thiserror::Error;

/// What the host wants from the device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuOptions {
    /// Request timestamp queries (for `desktank bench`), if the adapter
    /// supports them.
    pub timestamps: bool,
    /// Draw the grid and sprites with smaller shaders in more passes (the
    /// split grid, `tank.wgsl`), for GPUs that can't compile the one-pass
    /// shaders: Chrome 68's `SwiftShader` caps a fragment shader at 4096
    /// temporary registers (webOS, D26). Same picture, except where a
    /// translucent plant texel blends over the ground on an sRGB target.
    /// DX12 always draws it ([`Gpu::split_grid`]).
    pub split_grid: bool,
}

/// Error finding an adapter or opening a device.
#[derive(Debug, Error)]
pub enum GpuError {
    /// No adapter matched (none at all, or none can present to the surface).
    #[error("no suitable GPU adapter: {0}")]
    Adapter(#[from] wgpu::RequestAdapterError),
    /// The adapter refused the device request.
    #[error("could not open the GPU device: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
}

/// An adapter with its device and queue. One per process, shared by every
/// surface (SPEC §4.2).
#[derive(Debug)]
pub struct Gpu {
    /// The chosen adapter.
    pub adapter: wgpu::Adapter,
    /// The device.
    pub device: wgpu::Device,
    /// The device's queue.
    pub queue: wgpu::Queue,
    /// [`GpuOptions::split_grid`], or a DX12 adapter: wgpu's default DX12
    /// compiler there, FXC, miscompiles the one-pass grid shader (plants
    /// lose their leaves, the water stripes), but not the split passes.
    pub split_grid: bool,
}

impl Gpu {
    /// Picks a low-power adapter that can present to `surface` (required on
    /// WebGL2) and opens a device limited to WebGL2's downlevel limits, so a
    /// native run catches anything the web fallback couldn't do. The
    /// exception is texture size, taken from the adapter so surfaces larger
    /// than 2048 px work; hosts cap the grid at 2048 cells themselves.
    ///
    /// # Errors
    ///
    /// Returns an error if no adapter is suitable or the device request fails.
    pub async fn new(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        options: GpuOptions,
    ) -> Result<Self, GpuError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                force_fallback_adapter: false,
                compatible_surface: surface,
                apply_limit_buckets: false,
            })
            .await?;
        let mut required_features = wgpu::Features::empty();
        if options.timestamps && adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            required_features |= wgpu::Features::TIMESTAMP_QUERY;
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("desktank"),
                required_features,
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                ..Default::default()
            })
            .await?;
        Ok(Self {
            split_grid: options.split_grid || adapter.get_info().backend == wgpu::Backend::Dx12,
            adapter,
            device,
            queue,
        })
    }

    /// Whether the device has timestamp queries enabled.
    #[must_use]
    pub fn has_timestamps(&self) -> bool {
        self.device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
    }
}
