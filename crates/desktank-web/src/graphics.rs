//! The GPU side of the web host: backend selection and the canvas surface
//! (SPEC §3.5).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use desktank_render::{Gpu, GpuOptions, TankSurface, wgpu};
use wasm_bindgen::prelude::{JsValue, wasm_bindgen};
use web_sys::HtmlCanvasElement;

/// A browser graphics backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    WebGpu,
    WebGl2,
}

impl Backend {
    fn name(self) -> &'static str {
        match self {
            Self::WebGpu => "webgpu",
            Self::WebGl2 => "webgl2",
        }
    }
}

/// A wgpu device and a configured surface on a canvas.
#[wasm_bindgen]
pub struct Graphics {
    gpu: Gpu,
    surface: TankSurface,
    backend: Backend,
    /// Set by the device-lost callback (WebGPU). WebGL context loss arrives
    /// as a canvas event, which the page handles.
    lost: Arc<AtomicBool>,
}

#[wasm_bindgen]
impl Graphics {
    /// Opens graphics on `canvas` (sized in device pixels already).
    /// `backend` is `"webgpu"` or `"webgl2"` to force one; anything else
    /// tries WebGPU and falls back to WebGL2 if there is no adapter or the
    /// device request fails.
    ///
    /// # Errors
    ///
    /// Returns a message if no backend works.
    pub async fn open(canvas: HtmlCanvasElement, backend: String) -> Result<Graphics, JsValue> {
        let result = match backend.as_str() {
            "webgpu" => open_with(&canvas, Backend::WebGpu).await,
            "webgl2" => open_with(&canvas, Backend::WebGl2).await,
            _ => match open_with(&canvas, Backend::WebGpu).await {
                Ok(g) => Ok(g),
                Err(e) => {
                    web_sys::console::info_1(
                        &format!("desktank: WebGPU unavailable ({e}); using WebGL2").into(),
                    );
                    open_with(&canvas, Backend::WebGl2).await
                }
            },
        };
        result.map_err(|e| JsValue::from_str(&e))
    }

    /// The backend in use: `"webgpu"` or `"webgl2"`.
    #[must_use]
    pub fn backend(&self) -> String {
        self.backend.name().to_owned()
    }

    /// The adapter's name, for the stats line.
    #[wasm_bindgen(js_name = adapterName)]
    #[must_use]
    pub fn adapter_name(&self) -> String {
        self.gpu.adapter.get_info().name
    }
}

impl Graphics {
    pub(crate) fn gpu(&self) -> &Gpu {
        &self.gpu
    }

    pub(crate) fn surface(&self) -> &TankSurface {
        &self.surface
    }

    pub(crate) fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Acquire)
    }

    pub(crate) fn resize(&mut self, (width, height): (u32, u32)) {
        self.surface.resize(&self.gpu, width, height);
    }
}

async fn open_with(canvas: &HtmlCanvasElement, backend: Backend) -> Result<Graphics, String> {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = match backend {
        Backend::WebGpu => wgpu::Backends::BROWSER_WEBGPU,
        Backend::WebGl2 => wgpu::Backends::GL,
    };
    let instance = wgpu::Instance::new(desc);
    let target = || wgpu::SurfaceTarget::Canvas(canvas.clone());
    let (gpu, surface) = match backend {
        // Adapter and device first: a canvas keeps the first context type it
        // hands out, so a failed WebGPU attempt must not touch the canvas or
        // the WebGL2 fallback couldn't use it.
        Backend::WebGpu => {
            let gpu = Gpu::new(&instance, None, GpuOptions::default())
                .await
                .map_err(|e| e.to_string())?;
            let surface = instance
                .create_surface(target())
                .map_err(|e| e.to_string())?;
            (gpu, surface)
        }
        // WebGL2 needs the surface to pick an adapter.
        Backend::WebGl2 => {
            let surface = instance
                .create_surface(target())
                .map_err(|e| e.to_string())?;
            let gpu = Gpu::new(&instance, Some(&surface), GpuOptions::default())
                .await
                .map_err(|e| e.to_string())?;
            (gpu, surface)
        }
    };
    let lost = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&lost);
    gpu.device.set_device_lost_callback(move |reason, message| {
        flag.store(true, Ordering::Release);
        web_sys::console::warn_1(
            &format!("desktank: GPU device lost ({reason:?}): {message}").into(),
        );
    });
    // The default handler panics, which would abort the module.
    gpu.device.on_uncaptured_error(Arc::new(|e| {
        web_sys::console::error_1(&format!("desktank: GPU error: {e}").into());
    }));
    let surface = TankSurface::new(&gpu, surface, canvas.width(), canvas.height())
        .map_err(|e| e.to_string())?;
    Ok(Graphics {
        gpu,
        surface,
        backend,
        lost,
    })
}
