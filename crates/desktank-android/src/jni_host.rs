//! The JNI entry points the Kotlin shim calls (`io.github.robhannan.desktank.Native`),
//! the render thread they feed, and the logcat logger.
//!
//! The shim supplies the files directory before forwarding Dream callbacks.
//! The render thread owns the GPU device, surfaces, tank and save store.
//! `surfaceDestroyed` alone waits for the surface to be dropped (up to
//! [`DESTROY_WAIT`]). The render thread blocks on its channel while idle
//! and wakes on the tank's existing frame schedule while drawing (20 Hz
//! simulation, frames capped at 30 fps). Autosave uses those same wakeups.

use std::collections::HashMap;
use std::ffi::CString;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Mutex, Once, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use desktank_render::{Gpu, GpuOptions, Tank, TankConfig, TankSurface, wgpu};
use jni::EnvUnowned;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JClass, JObject, JString};
use jni::sys::{jint, jlong};
use log::{Level, LevelFilter};
use ndk::native_window::NativeWindow;

use crate::persist::{Persistence, start_world};
use crate::{DreamId, Dreams, Event, backend_order, log_enabled, log_override, tv_grid};

/// Logcat tag for everything this crate logs, wgpu's lines included.
const TAG: &str = "DeskTank";

/// System property that forces a backend (`vulkan` or `gl`) for testing.
const BACKEND_PROPERTY: &str = "debug.desktank.backend";

/// System property that sets the log level for every crate.
const LOG_PROPERTY: &str = "debug.desktank.log";

/// Longest `surfaceDestroyed` waits for the render thread to let go of the
/// surface: well under the 5 s input-dispatch ANR, and longer than device
/// creation (up to about 450 ms on the devices tried).
const DESTROY_WAIT: Duration = Duration::from_secs(2);

/// The render thread's inbox; spawned on the first callback.
static RENDER: Mutex<Option<Sender<Command>>> = Mutex::new(None);
static FILES_DIR: OnceLock<PathBuf> = OnceLock::new();

/// One callback, for the render thread.
struct Command {
    dream: DreamId,
    event: Event,
    /// The new surface's window, with [`Event::SurfaceCreated`].
    window: Option<NativeWindow>,
    /// Signalled once the surface is dropped, with
    /// [`Event::SurfaceDestroyed`].
    done: Option<SyncSender<()>>,
}

/// Posts `command` to the render thread, starting it (and the logger) on
/// first use.
fn post(command: Command) {
    init_logging();
    let mut render = RENDER.lock().unwrap_or_else(PoisonError::into_inner);
    if render.is_none() {
        let (tx, rx) = mpsc::channel();
        match std::thread::Builder::new()
            .name("desktank-render".to_owned())
            .spawn(move || run(&rx))
        {
            Ok(_) => *render = Some(tx),
            Err(e) => {
                log::error!("could not start the render thread: {e}");
                return;
            }
        }
    }
    if let Some(tx) = render.as_ref()
        && tx.send(command).is_err()
    {
        // Only if the thread has gone, which a panic (an abort in release)
        // or a closed channel would mean.
        log::error!("the render thread has stopped");
    }
}

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

/// A Dream's configured surface.
struct DreamSurface {
    surface: TankSurface,
    /// When the surface was created, until its first frame is logged.
    created_at: Option<Instant>,
}

/// The render thread's state.
struct Host {
    dreams: Dreams,
    /// The instance and device, created on the first surface and kept for
    /// the process's life so a later Dream starts faster.
    gpu: Option<(wgpu::Instance, Gpu)>,
    surfaces: HashMap<DreamId, DreamSurface>,
    /// The shared tank, loaded or created on the first drawable surface
    /// and kept across Dreams for the process's life.
    tank: Option<Tank>,
    persistence: Option<Persistence>,
    /// The Dream and size last drawn, so a change redraws.
    drawn: Option<(DreamId, (u32, u32))>,
    /// Origin of the clock the tank's pacer runs on.
    epoch: Instant,
}

/// The render thread: applies commands and draws until every sender is
/// gone (never, in practice: the inbox is static).
fn run(rx: &Receiver<Command>) {
    let mut host = Host::new();
    let mut wake_at: Option<Duration> = None;
    loop {
        let first = match wake_at {
            // Idle: no Dream draws, so block until a callback arrives.
            None => match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return,
            },
            Some(at) => match rx.recv_timeout(at.saturating_sub(host.now())) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
        };
        if let Some(command) = first {
            host.apply(command);
            while let Ok(command) = rx.try_recv() {
                host.apply(command);
            }
        }
        wake_at = host.frame();
    }
}

impl Host {
    fn new() -> Self {
        Self {
            dreams: Dreams::default(),
            gpu: None,
            surfaces: HashMap::new(),
            tank: None,
            persistence: None,
            drawn: None,
            epoch: Instant::now(),
        }
    }

    fn now(&self) -> Duration {
        self.epoch.elapsed()
    }

    fn apply(&mut self, command: Command) {
        let Command {
            dream,
            event,
            window,
            done,
        } = command;
        let save_reason = match event {
            Event::DreamingStopped => Some("dreaming stopped"),
            Event::SurfaceDestroyed if self.dreams.active().is_some_and(|(id, _)| id == dream) => {
                Some("active surface destroyed")
            }
            _ => None,
        };
        self.dreams.handle(dream, event);
        match event {
            Event::SurfaceCreated => {
                if let Some(window) = window {
                    self.surface_created(dream, window);
                }
            }
            Event::SurfaceChanged { width, height } => {
                if let (Some(s), Some((_, gpu))) = (self.surfaces.get_mut(&dream), &self.gpu) {
                    s.surface.resize(gpu, width, height);
                }
            }
            Event::SurfaceDestroyed => {
                // The surface (and the window it holds) must go before
                // `surfaceDestroyed` returns.
                self.surfaces.remove(&dream);
            }
            Event::DreamingStarted | Event::DreamingStopped => {}
        }
        if let Some(done) = done {
            // The caller may have stopped waiting; nothing to do then.
            let _ = done.send(());
        }
        // Acknowledge surface release before starting disk I/O.
        if let (Some(why), Some(tank), Some(persist)) =
            (save_reason, &self.tank, &mut self.persistence)
        {
            persist.save(tank.world(), why, self.epoch.elapsed());
        }
    }

    fn surface_created(&mut self, dream: DreamId, window: NativeWindow) {
        let created_at = Instant::now();
        let (width, height) = window_size(&window);
        match self.make_surface(window, width, height) {
            Ok(surface) => {
                self.surfaces.insert(
                    dream,
                    DreamSurface {
                        surface,
                        created_at: Some(created_at),
                    },
                );
            }
            Err(e) => log::error!("no GPU surface: {e}"),
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
        let property = system_property(BACKEND_PROPERTY);
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
            log::info!(
                "adapter: backend={:?} name={:?} type={:?} driver={:?} driver_info={:?} \
                 (property {BACKEND_PROPERTY}={property:?}; failed before it: {errors:?})",
                info.backend,
                info.name,
                info.device_type,
                info.driver,
                info.driver_info
            );
            let surface =
                TankSurface::new(&gpu, surface, width, height).map_err(|e| e.to_string())?;
            self.gpu = Some((instance, gpu));
            return Ok(surface);
        }
        Err(format!("no backend worked: {errors:?}"))
    }

    /// Advances and draws the active Dream's tank, if a Dream is drawing.
    /// Returns when to call again, or `None` to idle until a callback.
    fn frame(&mut self) -> Option<Duration> {
        let wake_at = self.draw_active();
        if wake_at.is_none() {
            // Nothing drawing: world time stops (SPEC §6.2) and nothing runs.
            if let Some(tank) = &mut self.tank {
                tank.pause();
            }
            self.drawn = None;
        }
        wake_at
    }

    /// [`Host::frame`]'s work while a Dream can draw; `None` if none can.
    fn draw_active(&mut self) -> Option<Duration> {
        let (dream, size) = self.dreams.active()?;
        // No device or no surface means creating them failed (logged), or a
        // draw failed; nothing can draw until the next surface.
        let (_, gpu) = self.gpu.as_ref()?;
        let s = self.surfaces.get_mut(&dream)?;
        if self.tank.is_none() {
            let grid = tv_grid(size.0, size.1);
            if let Some(dir) = FILES_DIR.get() {
                self.persistence = match Persistence::open(dir, self.epoch.elapsed()) {
                    Ok(persist) => Some(persist),
                    Err(e) => {
                        log::warn!("could not open the tank store under {}: {e}", dir.display());
                        None
                    }
                };
            } else {
                log::warn!("no app files directory; running without saves");
            }
            match start_world(self.persistence.as_mut(), grid) {
                Ok(world) => {
                    let mut tank = Tank::new(world, TankConfig::default());
                    // A Dream is a screensaver: no optional sway (T6d).
                    tank.set_saver_mode(true);
                    self.tank = Some(tank);
                }
                Err(e) => {
                    log::error!("no tank: {e}");
                    return None;
                }
            }
        }
        let tank = self.tank.as_mut()?;
        if self.drawn != Some((dream, size)) {
            // A new Dream or size: the old frame isn't on this surface.
            tank.invalidate();
            self.drawn = Some((dream, size));
        }
        let request = tank.tick(self.epoch.elapsed());
        if request.redraw {
            let started = Instant::now();
            match tank.draw(gpu, &s.surface) {
                Ok(true) => {
                    if let Some(created) = s.created_at.take() {
                        let (w, h) = s.surface.size();
                        log::info!(
                            "first frame {w}x{h} {:?} at {}x, {} ms after surfaceCreated \
                             (the draw took {} ms)",
                            s.surface.format(),
                            tank.layout((w, h)).scale,
                            created.elapsed().as_millis(),
                            started.elapsed().as_millis()
                        );
                    }
                }
                // Outdated, lost or busy: the frame stays wanted and the
                // next tick tries again.
                Ok(false) => {}
                Err(e) => {
                    // A validation error, not a transient: drop the surface
                    // rather than retry it every tick.
                    log::error!("draw failed, dropping the surface: {e}");
                    self.surfaces.remove(&dream);
                    return None;
                }
            }
        }
        if let Some(persist) = &mut self.persistence {
            persist.autosave(tank.world(), self.epoch.elapsed());
        }
        Some(request.wake_at)
    }
}

fn window_size(window: &NativeWindow) -> (u32, u32) {
    let side = |v: i32| u32::try_from(v).unwrap_or(0);
    (side(window.width()), side(window.height()))
}

fn system_property(name: &str) -> Option<String> {
    android_system_properties::AndroidSystemProperties::new()
        .get(name)
        .filter(|p| !p.is_empty())
}

/// `Native.setFilesDir(path)`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_setFilesDir<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    path: JString<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let path = PathBuf::from(path.try_to_string(env)?);
        let dir = FILES_DIR.get_or_init(|| path.clone());
        if dir != &path {
            init_logging();
            log::warn!("ignoring change to the app files directory");
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

/// `Native.surfaceCreated(dream, Surface)`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_surfaceCreated<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    dream: jlong,
    surface: JObject<'local>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        // SAFETY: `env` is this native call's JNIEnv, valid on this thread for
        // the call, and `surface` is the `android.view.Surface` the shim
        // passed (a live local reference for the call). ndk's `JNIEnv` is
        // jni-sys 0.3's, the same C struct as jni 0.22's, so the cast only
        // changes the Rust type. The returned window holds its own reference
        // (`ANativeWindow_fromSurface` acquires one), so it outlives `surface`
        // and may move to the render thread (`NativeWindow` is `Send`).
        let window =
            unsafe { NativeWindow::from_surface(env.get_raw().cast(), surface.as_raw().cast()) };
        let missing = window.is_none();
        // Posted even without a window, so the Dream's state stays right.
        post(Command {
            dream,
            event: Event::SurfaceCreated,
            window,
            done: None,
        });
        if missing {
            log::error!("surfaceCreated: no ANativeWindow for the Surface");
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

/// `Native.surfaceChanged(dream, width, height)`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_surfaceChanged<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    dream: jlong,
    width: jint,
    height: jint,
) {
    let side = |v: jint| u32::try_from(v).unwrap_or(0);
    forward(
        &mut env,
        dream,
        Event::SurfaceChanged {
            width: side(width),
            height: side(height),
        },
    );
}

/// `Native.surfaceDestroyed(dream)`: returns once the render thread has
/// dropped the surface (or after [`DESTROY_WAIT`]).
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_surfaceDestroyed<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    dream: jlong,
) {
    env.with_env(|_| -> jni::errors::Result<()> {
        let (done, dropped) = mpsc::sync_channel(1);
        post(Command {
            dream,
            event: Event::SurfaceDestroyed,
            window: None,
            done: Some(done),
        });
        // A disconnect means the render thread is gone, and its surfaces
        // with it.
        if let Err(RecvTimeoutError::Timeout) = dropped.recv_timeout(DESTROY_WAIT) {
            log::error!("surfaceDestroyed: the render thread didn't release the surface in time");
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

/// `Native.dreamingStarted(dream)`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_dreamingStarted<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    dream: jlong,
) {
    forward(&mut env, dream, Event::DreamingStarted);
}

/// `Native.dreamingStopped(dream)`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_dreamingStopped<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    dream: jlong,
) {
    forward(&mut env, dream, Event::DreamingStopped);
}

/// Posts `event` inside `with_env`, so a panic becomes a Java exception
/// instead of unwinding into the JVM (where unwinding is enabled).
fn forward(env: &mut EnvUnowned<'_>, dream: DreamId, event: Event) {
    env.with_env(|_| -> jni::errors::Result<()> {
        post(Command {
            dream,
            event,
            window: None,
            done: None,
        });
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

/// Routes `log` records (this crate's and wgpu's) to logcat under [`TAG`].
struct Logcat {
    over: Option<LevelFilter>,
}

impl log::Log for Logcat {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        log_enabled(metadata.target(), metadata.level(), self.over)
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let target = record.target();
        if target.starts_with(env!("CARGO_CRATE_NAME")) {
            write(record.level(), &record.args().to_string());
        } else {
            write(record.level(), &format!("[{target}] {}", record.args()));
        }
    }

    fn flush(&self) {}
}

/// Installs the logcat logger and a panic hook that logs the panic (stderr
/// goes nowhere on Android), once per process.
fn init_logging() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let over = log_override(system_property(LOG_PROPERTY).as_deref());
        if log::set_boxed_logger(Box::new(Logcat { over })).is_ok() {
            log::set_max_level(over.unwrap_or(LevelFilter::Info));
        }
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            write(Level::Error, &format!("panic: {info}"));
            default_hook(info);
        }));
    });
}

/// Writes one line to logcat under [`TAG`].
fn write(level: Level, message: &str) {
    use ndk_sys::android_LogPriority as P;
    let prio = match level {
        Level::Error => P::ANDROID_LOG_ERROR,
        Level::Warn => P::ANDROID_LOG_WARN,
        Level::Info => P::ANDROID_LOG_INFO,
        Level::Debug => P::ANDROID_LOG_DEBUG,
        Level::Trace => P::ANDROID_LOG_VERBOSE,
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
