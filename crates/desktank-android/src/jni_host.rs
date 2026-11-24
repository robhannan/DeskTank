//! The JNI entry points the Kotlin shim calls (`io.github.robhannan.desktank.Native`),
//! the render thread they feed, and the logcat logger.
//!
//! The shim initializes the process before forwarding host callbacks.
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
use desktank_sim::scene;
use jni::EnvUnowned;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JClass, JObject, JString};
use jni::sys::{jboolean, jfloat, jint, jlong};
use log::{Level, LevelFilter};
use ndk::native_window::NativeWindow;

use crate::brush::Brush;
use crate::persist::{Persistence, start_world};
use crate::touch::{Touch, TouchEvent};
use crate::{
    Drawable, Event, HostId, HostKind, Hosts, backend_order, log_enabled, log_override, phone_grid,
    tv_grid,
};

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
static SETTINGS: OnceLock<Settings> = OnceLock::new();

struct Settings {
    files_dir: PathBuf,
    television: bool,
    /// Relates Android's uptime clock (`MotionEvent` timestamps) to `Instant`.
    uptime: Duration,
    epoch: Instant,
}

impl Settings {
    fn now(&self) -> Duration {
        self.uptime + self.epoch.elapsed()
    }
}

/// One callback, for the render thread.
struct Command {
    id: HostId,
    callback: Callback,
}

enum Callback {
    Lifecycle(Event),
    Created(Option<NativeWindow>),
    /// Acknowledged once the surface is dropped.
    Destroyed(SyncSender<()>),
    Touch(TouchEvent, Duration),
}

fn millis(time: jlong) -> Duration {
    Duration::from_millis(u64::try_from(time).unwrap_or(0))
}

/// Posts `command` to the render thread, starting it (and the logger) on
/// first use.
fn post(command: Command) {
    init_logging();
    let mut render = RENDER.lock().unwrap_or_else(PoisonError::into_inner);
    if render.is_none() {
        let Some(settings) = SETTINGS.get() else {
            log::error!("host callback before initialization");
            return;
        };
        let (tx, rx) = mpsc::channel();
        match std::thread::Builder::new()
            .name("desktank-render".to_owned())
            .spawn(move || run(&rx, settings))
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

/// A host's configured surface.
struct HostSurface {
    surface: TankSurface,
    /// When the surface was created, until its first frame is logged.
    created_at: Option<Instant>,
}

/// The render thread's state.
struct Host {
    hosts: Hosts,
    /// The instance and device, created on the first surface and kept for
    /// the process's life so later surfaces start faster.
    gpu: Option<(wgpu::Instance, Gpu)>,
    surfaces: HashMap<HostId, HostSurface>,
    /// The shared tank, loaded or created on the first drawable surface
    /// and kept across hosts for the process's life.
    tank: Option<Tank>,
    persistence: Option<Persistence>,
    /// The host and size last drawn, so a change redraws.
    drawn: Option<Drawable>,
    settings: &'static Settings,
    touch: Touch,
    brush: Option<Brush>,
}

/// The render thread: applies commands and draws until every sender is
/// gone (never, in practice: the inbox is static).
fn run(rx: &Receiver<Command>, settings: &'static Settings) {
    let mut host = Host::new(settings);
    let mut wake_at: Option<Duration> = None;
    loop {
        let first = match wake_at {
            // Idle: no host draws, so block until a callback arrives.
            None => match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return,
            },
            Some(at) => match rx.recv_timeout(at.saturating_sub(settings.now())) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
        };
        if let Some(command) = first {
            host.apply(command);
            // Bound each pass so a stream of touch samples can't starve a
            // frame. Keep sample order for the slop and long-press checks.
            for command in rx.try_iter().take(127) {
                host.apply(command);
            }
        }
        wake_at = host.frame();
    }
}

impl Host {
    fn new(settings: &'static Settings) -> Self {
        Self {
            hosts: Hosts::default(),
            gpu: None,
            surfaces: HashMap::new(),
            tank: None,
            persistence: None,
            drawn: None,
            settings,
            touch: Touch::default(),
            brush: None,
        }
    }

    fn active(&self) -> Option<Drawable> {
        self.hosts
            .active_with_surface(|id| self.surfaces.contains_key(&id))
    }

    fn apply(&mut self, Command { id, callback }: Command) {
        let previous = self.active();
        let event = match callback {
            Callback::Touch(event, at) => {
                if let Some(host) = previous.filter(|h| h.id == id && h.kind == HostKind::App) {
                    self.apply_touch(host.size, event, at);
                }
                return;
            }
            Callback::Created(window) => {
                if let Some(window) = window {
                    self.surface_created(id, window);
                }
                Event::SurfaceCreated
            }
            Callback::Destroyed(done) => {
                // Release the native window before acknowledging the callback
                // or starting disk I/O.
                self.surfaces.remove(&id);
                let _ = done.send(());
                Event::SurfaceDestroyed
            }
            Callback::Lifecycle(event) => event,
        };
        if let Event::SurfaceChanged { width, height } = event
            && let (Some(s), Some((_, gpu))) = (self.surfaces.get_mut(&id), &self.gpu)
        {
            s.surface.resize(gpu, width, height);
        }
        self.hosts.handle(id, event);
        let active = self.active();
        if previous.map(|h| h.id) != active.map(|h| h.id) {
            self.drawn = None;
            if let Some(old) = previous.filter(|h| h.kind == HostKind::App) {
                self.cancel_touch(old.size);
            }
            if active.is_none()
                && let Some(tank) = &mut self.tank
            {
                tank.pause();
            }
        }

        let save_reason = match event {
            Event::Stopped => Some("host stopped"),
            Event::SurfaceDestroyed if previous.is_some_and(|h| h.id == id) => {
                Some("active surface destroyed")
            }
            _ => None,
        };
        if let (Some(why), Some(tank), Some(persist)) =
            (save_reason, &self.tank, &mut self.persistence)
        {
            persist.save(tank.world(), why, self.settings.now());
        }
    }

    fn surface_created(&mut self, id: HostId, window: NativeWindow) {
        let created_at = Instant::now();
        let (width, height) = window_size(&window);
        match self.make_surface(window, width, height) {
            Ok(surface) => {
                self.surfaces.insert(
                    id,
                    HostSurface {
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

    fn init_tank(&mut self, (width, height): (u32, u32)) {
        if self.tank.is_some() {
            return;
        }
        let grid = if self.settings.television {
            tv_grid(width, height)
        } else {
            phone_grid(width, height)
        };
        let dir = &self.settings.files_dir;
        self.persistence = match Persistence::open(dir, self.settings.now()) {
            Ok(persist) => Some(persist),
            Err(e) => {
                log::warn!("could not open the tank store under {}: {e}", dir.display());
                None
            }
        };
        match start_world(self.persistence.as_mut(), grid) {
            Ok(world) => {
                match scene::material(&world, "sand") {
                    Ok(sand) => self.brush = Some(Brush::new(sand)),
                    Err(e) => log::error!("touch tools unavailable: {e}"),
                }
                self.tank = Some(Tank::new(world, TankConfig::default()));
            }
            Err(e) => log::error!("no tank: {e}"),
        }
    }

    fn apply_touch(&mut self, size: (u32, u32), event: TouchEvent, at: Duration) {
        self.init_tank(size);
        self.touch_actions(size, |touch, emit| touch.handle(event, at, emit));
    }

    fn cancel_touch(&mut self, size: (u32, u32)) {
        self.touch_actions(size, |touch, emit| touch.cancel(emit));
    }

    fn touch_actions(
        &mut self,
        size: (u32, u32),
        generate: impl FnOnce(&mut Touch, &mut dyn FnMut(crate::touch::TouchAction)),
    ) {
        if let (Some(brush), Some(tank)) = (&mut self.brush, &mut self.tank) {
            let mut edited = false;
            generate(&mut self.touch, &mut |action| {
                edited |= brush.apply(action, tank, size);
            });
            if edited && let Some(persist) = &mut self.persistence {
                persist.mark_edited();
            }
        }
    }

    /// Advances and draws the active host's tank. Returns the next wake
    /// time, or `None` to idle until a callback.
    fn frame(&mut self) -> Option<Duration> {
        let active = self.active()?;
        self.init_tank(active.size);
        let now = self.settings.now();
        if active.kind == HostKind::App {
            self.touch_actions(active.size, |touch, emit| touch.advance(now, emit));
        }
        let wake_at = self.draw_active(active, now);
        if wake_at.is_none() {
            // A draw failed; let another drawable host take over if present.
            self.cancel_touch(active.size);
            self.drawn = None;
            if let Some(tank) = &mut self.tank {
                tank.pause();
            }
            return self.active().filter(|h| h.id != active.id).map(|_| now);
        }
        wake_at.map(|wake| self.touch.deadline().map_or(wake, |touch| wake.min(touch)))
    }

    fn draw_active(&mut self, active: Drawable, now: Duration) -> Option<Duration> {
        let tank = self.tank.as_mut()?;
        let (_, gpu) = self.gpu.as_ref()?;
        let s = self.surfaces.get_mut(&active.id)?;
        if self.drawn != Some(active) {
            // A different host or size needs a frame on this surface.
            tank.set_saver_mode(active.kind == HostKind::Dream);
            tank.invalidate();
            self.drawn = Some(active);
        }
        let request = tank.tick(now);
        if active.kind == HostKind::App
            && let Some(brush) = &mut self.brush
            && brush.hold(tank.world_mut(), request.sim_ticks)
            && let Some(persist) = &mut self.persistence
        {
            persist.mark_edited();
        }
        if request.redraw {
            let started = Instant::now();
            match tank.draw(gpu, &s.surface) {
                Ok(true) => {
                    if let Some(created) = s.created_at.take() {
                        let (w, h) = s.surface.size();
                        log::info!(
                            "first frame {w}x{h} {:?} at {}x, {} ms after surfaceCreated \
                             (the draw took {} ms, host {:?})",
                            s.surface.format(),
                            tank.layout((w, h)).scale,
                            created.elapsed().as_millis(),
                            started.elapsed().as_millis(),
                            active.kind,
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
                    self.surfaces.remove(&active.id);
                    return None;
                }
            }
        }
        if let Some(persist) = &mut self.persistence {
            persist.autosave(tank.world(), self.settings.now());
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

/// `Native.init(path, television, uptimeMs)`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_init<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    path: JString<'local>,
    television: jboolean,
    uptime_ms: jlong,
) {
    let epoch = Instant::now();
    env.with_env(|env| -> jni::errors::Result<()> {
        let settings = Settings {
            files_dir: PathBuf::from(path.try_to_string(env)?),
            television,
            uptime: millis(uptime_ms),
            epoch,
        };
        let _ = SETTINGS.set(settings);
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

/// `Native.surfaceCreated(id, Surface)`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_surfaceCreated<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    id: jlong,
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
        if window.is_none() {
            init_logging();
            log::error!("surfaceCreated: no ANativeWindow for the Surface");
        }
        post(Command {
            id,
            callback: Callback::Created(window),
        });
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

/// `Native.surfaceChanged(id, width, height)`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_surfaceChanged<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    id: jlong,
    width: jint,
    height: jint,
) {
    let side = |v: jint| u32::try_from(v).unwrap_or(0);
    forward(
        &mut env,
        id,
        Callback::Lifecycle(Event::SurfaceChanged {
            width: side(width),
            height: side(height),
        }),
    );
}

/// `Native.surfaceDestroyed(id)`: returns once the render thread has
/// dropped the surface (or after [`DESTROY_WAIT`]).
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_surfaceDestroyed<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    id: jlong,
) {
    let (done, dropped) = mpsc::sync_channel(1);
    forward(&mut env, id, Callback::Destroyed(done));
    // A disconnect means the render thread is gone, and its surfaces
    // with it.
    if let Err(RecvTimeoutError::Timeout) = dropped.recv_timeout(DESTROY_WAIT) {
        log::error!("surfaceDestroyed: the render thread didn't release the surface in time");
    }
}

/// `Native.started(id, app)`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_started<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    id: jlong,
    app: jboolean,
) {
    let kind = if app { HostKind::App } else { HostKind::Dream };
    forward(&mut env, id, Callback::Lifecycle(Event::Started(kind)));
}

/// `Native.stopped(id)`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_stopped<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    id: jlong,
) {
    forward(&mut env, id, Callback::Lifecycle(Event::Stopped));
}

/// `Native.touch(id, action, pointerId, x, y, eventTimeMs, density)`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_robhannan_desktank_Native_touch<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    id: jlong,
    action: jint,
    pointer_id: jint,
    x: jfloat,
    y: jfloat,
    event_time_ms: jlong,
    density: jfloat,
) {
    let point = (f64::from(x), f64::from(y));
    let event = match action {
        // MotionEvent.ACTION_DOWN / UP / MOVE / CANCEL; Kotlin maps pointer
        // down/up to the same codes so each sample describes one pointer.
        0 => TouchEvent::Down {
            id: pointer_id,
            point,
            density: f64::from(density),
        },
        1 => TouchEvent::Up(pointer_id),
        2 => TouchEvent::Move {
            id: pointer_id,
            point,
        },
        3 => TouchEvent::Cancel,
        _ => return,
    };
    forward(&mut env, id, Callback::Touch(event, millis(event_time_ms)));
}

/// Posts the callback inside `with_env`, so a panic becomes a Java exception
/// instead of unwinding into the JVM.
fn forward(env: &mut EnvUnowned<'_>, id: HostId, callback: Callback) {
    env.with_env(|_| -> jni::errors::Result<()> {
        post(Command { id, callback });
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
