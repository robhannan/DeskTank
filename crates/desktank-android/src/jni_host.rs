//! The JNI entry points the Kotlin shim calls (`io.github.robhannan.desktank.Native`)
//! and the one process-wide host they drive.
//!
//! Every callback arrives on the app's main thread. The A0 skeleton does its
//! GPU work there too: it creates the device on the first surface and clears
//! the surface once per change, so nothing runs between callbacks. A1 moves
//! drawing to a render thread paced by the frame pacer.

use std::ffi::CString;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use desktank_render::{Gpu, GpuOptions, TankSurface, wgpu};
use jni::EnvUnowned;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JClass, JObject};
use jni::sys::jint;
use ndk::native_window::NativeWindow;

use crate::{Event, Lifecycle, backend_order, clear_colour, water_rgb};

/// Logcat tag for everything this crate logs.
const TAG: &str = "DeskTank";

/// System property that forces a backend (`vulkan` or `gl`) for testing.
const BACKEND_PROPERTY: &str = "debug.desktank.backend";

/// Tries to present a frame this many times per draw before giving up.
const DRAW_ATTEMPTS: usize = 3;

static HOST: Mutex<Option<Host>> = Mutex::new(None);

/// The Android display, for [`wgpu::InstanceDescriptor::display`]: wgpu 30
/// needs a display handle to create any surface (Vulkan and GLES alike).
#[derive(Debug)]
struct AndroidDisplay;

impl raw_window_handle::HasDisplayHandle for AndroidDisplay {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Ok(raw_window_handle::DisplayHandle::android())
    }
}

/// The process's GPU state and the Dream's surface.
struct Host {
    lifecycle: Lifecycle,
    /// The instance and device, created on the first surface and kept for
    /// the process's life so a later Dream starts faster.
    gpu: Option<(wgpu::Instance, Gpu)>,
    surface: Option<TankSurface>,
    water: [u8; 3],
    /// When the current surface was created, for the time to first frame.
    created_at: Option<Instant>,
}

fn host() -> MutexGuard<'static, Option<Host>> {
    // A panic while holding the lock leaves nothing half-updated that a
    // later callback can't recover from, so carry on with the data.
    HOST.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Host {
    fn new() -> Self {
        let water = water_rgb().unwrap_or_else(|e| {
            log(Priority::Error, &e.to_string());
            [0x1f, 0x4e, 0x79]
        });
        Self {
            lifecycle: Lifecycle::default(),
            gpu: None,
            surface: None,
            water,
            created_at: None,
        }
    }

    fn surface_created(&mut self, window: NativeWindow) {
        self.created_at = Some(Instant::now());
        let (width, height) = window_size(&window);
        match self.make_surface(window, width, height) {
            Ok(surface) => self.surface = Some(surface),
            Err(e) => log(Priority::Error, &format!("no GPU surface: {e}")),
        }
    }

    /// Makes a wgpu surface for `window`, creating the instance and device
    /// on first use: each backend in [`backend_order`] in turn until one
    /// gives an adapter that can present to it.
    fn make_surface(
        &mut self,
        window: NativeWindow,
        width: u32,
        height: u32,
    ) -> Result<TankSurface, String> {
        if let Some((instance, gpu)) = &self.gpu {
            let surface = instance
                .create_surface(wgpu::SurfaceTarget::from_window_without_display(window))
                .map_err(|e| e.to_string())?;
            return TankSurface::new(gpu, surface, width, height).map_err(|e| e.to_string());
        }
        let property = android_system_properties::AndroidSystemProperties::new()
            .get(BACKEND_PROPERTY)
            .filter(|p| !p.is_empty());
        let mut errors = Vec::new();
        for &backends in backend_order(property.as_deref()) {
            let mut desc =
                wgpu::InstanceDescriptor::new_with_display_handle(Box::new(AndroidDisplay));
            desc.backends = backends;
            let instance = wgpu::Instance::new(desc);
            let surface = match instance.create_surface(
                wgpu::SurfaceTarget::from_window_without_display(window.clone()),
            ) {
                Ok(s) => s,
                Err(e) => {
                    errors.push(format!("{backends:?}: {e}"));
                    continue;
                }
            };
            let gpu = match pollster::block_on(Gpu::new(
                &instance,
                Some(&surface),
                GpuOptions::default(),
            )) {
                Ok(gpu) => gpu,
                Err(e) => {
                    errors.push(format!("{backends:?}: {e}"));
                    continue;
                }
            };
            let info = gpu.adapter.get_info();
            log(
                Priority::Info,
                &format!(
                    "adapter: backend={:?} name={:?} type={:?} driver={:?} driver_info={:?} \
                     (property {BACKEND_PROPERTY}={property:?}; failed before it: {errors:?})",
                    info.backend, info.name, info.device_type, info.driver, info.driver_info
                ),
            );
            let surface =
                TankSurface::new(&gpu, surface, width, height).map_err(|e| e.to_string())?;
            self.gpu = Some((instance, gpu));
            return Ok(surface);
        }
        Err(format!("no backend worked: {errors:?}"))
    }

    fn handle(&mut self, event: Event) {
        let draw = self.lifecycle.handle(event);
        match event {
            Event::SurfaceChanged { width, height } => {
                if let (Some(surface), Some((_, gpu))) = (&mut self.surface, &self.gpu) {
                    surface.resize(gpu, width, height);
                }
            }
            // The surface (and the window it holds) must go before
            // `surfaceDestroyed` returns.
            Event::SurfaceDestroyed => {
                self.surface = None;
                self.created_at = None;
            }
            Event::SurfaceCreated | Event::DreamingStarted | Event::DreamingStopped => {}
        }
        if draw {
            self.draw();
        }
    }

    fn draw(&mut self) {
        let (Some(surface), Some((_, gpu))) = (&self.surface, &self.gpu) else {
            return;
        };
        let colour = clear_colour(self.water, surface.format());
        // An outdated or lost surface is reconfigured and skipped; the A0
        // skeleton draws only on callbacks, so retry it here (bounded) rather
        // than leave the Dream blank until the next one.
        for _ in 0..DRAW_ATTEMPTS {
            match surface.clear(gpu, colour) {
                Ok(true) => {
                    if let Some(created) = self.created_at.take() {
                        let (w, h) = surface.size();
                        log(
                            Priority::Info,
                            &format!(
                                "first frame {w}x{h} {:?} {} ms after surfaceCreated",
                                surface.format(),
                                created.elapsed().as_millis()
                            ),
                        );
                    }
                    return;
                }
                Ok(false) => {}
                Err(e) => {
                    log(Priority::Error, &format!("clear failed: {e}"));
                    return;
                }
            }
        }
        log(
            Priority::Error,
            &format!("no frame after {DRAW_ATTEMPTS} attempts (surface outdated, lost or busy)"),
        );
    }
}

fn window_size(window: &NativeWindow) -> (u32, u32) {
    let side = |v: i32| u32::try_from(v).unwrap_or(0);
    (side(window.width()), side(window.height()))
}

/// Runs `f` on the host, creating it on first use.
fn with_host(f: impl FnOnce(&mut Host)) {
    let mut guard = host();
    f(guard.get_or_insert_with(Host::new));
}

/// `Native.surfaceCreated(Surface)`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_surfaceCreated<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    surface: JObject<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        // SAFETY: `env` is this native call's JNIEnv, valid on this thread for
        // the call, and `surface` is the `android.view.Surface` the shim
        // passed (a live local reference for the call). ndk's `JNIEnv` is
        // jni-sys 0.3's, the same C struct as jni 0.22's, so the cast only
        // changes the Rust type. The returned window holds its own reference
        // (`ANativeWindow_fromSurface` acquires one), so it outlives `surface`.
        let window =
            unsafe { NativeWindow::from_surface(env.get_raw().cast(), surface.as_raw().cast()) };
        let Some(window) = window else {
            log(
                Priority::Error,
                "surfaceCreated: no ANativeWindow for the Surface",
            );
            return Ok(());
        };
        with_host(|h| {
            h.handle(Event::SurfaceCreated);
            h.surface_created(window);
        });
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

/// `Native.surfaceChanged(width, height)`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_surfaceChanged<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    width: jint,
    height: jint,
) {
    let side = |v: jint| u32::try_from(v).unwrap_or(0);
    forward(
        &mut env,
        Event::SurfaceChanged {
            width: side(width),
            height: side(height),
        },
    );
}

/// `Native.surfaceDestroyed()`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_surfaceDestroyed<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) {
    forward(&mut env, Event::SurfaceDestroyed);
}

/// `Native.dreamingStarted()`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_dreamingStarted<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) {
    forward(&mut env, Event::DreamingStarted);
}

/// `Native.dreamingStopped()`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_dreamingStopped<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) {
    forward(&mut env, Event::DreamingStopped);
}

/// Handles `event` inside `with_env`, so a panic becomes a Java exception
/// instead of unwinding into the JVM (where unwinding is enabled).
fn forward(env: &mut EnvUnowned<'_>, event: Event) {
    env.with_env(|_| -> jni::errors::Result<()> {
        with_host(|h| h.handle(event));
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

#[derive(Clone, Copy)]
enum Priority {
    Info,
    Error,
}

/// Writes one line to logcat under [`TAG`].
fn log(priority: Priority, message: &str) {
    let prio = match priority {
        Priority::Info => ndk_sys::android_LogPriority::ANDROID_LOG_INFO,
        Priority::Error => ndk_sys::android_LogPriority::ANDROID_LOG_ERROR,
    };
    let Ok(text) = CString::new(message.replace('\0', " ")) else {
        return;
    };
    let tag = CString::new(TAG).unwrap_or_default();
    // SAFETY: `tag` and `text` are NUL-terminated C strings that live until
    // the call returns; `__android_log_write` only reads them.
    unsafe {
        ndk_sys::__android_log_write(prio.0.cast_signed(), tag.as_ptr(), text.as_ptr());
    }
}
