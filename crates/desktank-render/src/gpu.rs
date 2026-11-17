//! Adapter and device selection.

use thiserror::Error;

/// What the host wants from the device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuOptions {
    /// Request timestamp queries (for `desktank bench`), if the adapter
    /// supports them.
    pub timestamps: bool,
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
