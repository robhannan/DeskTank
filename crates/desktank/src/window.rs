//! `desktank window`: the tank in a normal resizable winit window.
//!
//! Pacing (SPEC §4.2, §6.2): no continuous redraw. The loop sleeps with
//! `ControlFlow::WaitUntil` until the tank's next deadline (a 20 Hz
//! simulation tick, frames capped at 30 fps on AC or 15 fps on battery and
//! drawn only when the world changed). While the window is occluded or
//! minimised, the display sleeps, the session is locked, Low Power Mode is on,
//! or battery drops below 20%, the tank is paused and the loop waits on OS
//! events only, with no timers.
//!
//! Interaction (SPEC §2.5) uses the platform-free `desktank_sim::tools`:
//! left drag uses the selected tool (pour, scoop, net, inspect, glue), right
//! drag scoops, the scroll wheel or a two-finger trackpad scroll sets the
//! brush size. The palette in the top-left corner is a two-level tree
//! (`desktank_sim::tools::Tools::palette_tree`): its top row picks a
//! category (click a tab, or `G` / `Shift+G` for the next or previous one),
//! the row below it picks a sub-category when the category has several
//! (click one, or `H` / `Shift+H`), and the last row lists that
//! sub-category's materials, devices, tools, plants or creatures; click one
//! (or press 1–9, then 0 for the tenth) to select it. Scrolling over the
//! palette steps through the sub-categories, on into the next category. A device
//! (bubbler, filter outlet) is placed once per click. `F` cycles a debug
//! tint of the nutrient, CO₂ and light fields; `W` changes the water and
//! `Q` shows the water quality in the title bar. `--time HH:MM` lights the
//! tank for that time of day (default noon). Pointer moves only redraw
//! when the brush outline changes. Escape closes.
//!
//! Plants (T6b): each plant species has a slot under Plants; selected, it
//! pours its species as the propagule named in the title bar (seeds,
//! cuttings, fragments, runners or buds), and `P` picks the next one that
//! species grows from (remembered per species). Inspect shows a plant's
//! species, age, health, size and what limits its growth in the title bar.
//!
//! Creatures (T7a): the palette's creature slots add one per click; the net
//! carries a creature and, let go above the water, lifts it out; inspect
//! shows its card in the title bar (T9: its name, species, stage, sex,
//! age, health, energy, morph and how long a carrying female has to go;
//! [`desktank_sim::CreatureCard`]). `N` then opens a name field for it in
//! the title bar: Enter names it (a blank name removes it), Escape cancels.
//! Names are saved with the tank. `--fish N` stocks the tank with N
//! creatures to start with. Swimming creatures redraw the tank every tick
//! (20 fps, under the cap) even when every chunk sleeps.
//!
//! Cheats: the Konami code (↑ ↑ ↓ ↓ ← → ← → B A; the window has keyboard
//! focus, so no chord) opens a cheat field in the title bar. Typing fills
//! it, Enter runs the line ([`desktank_sim::cheats`]: `help`, `feed`,
//! `spawn SPECIES [N]`, `clean`), Escape closes it (and doesn't close the
//! window while it is open). Other key bindings are off while it is open.
//!
//! `--demo-pour N` feeds the same code a synthetic left drag sweeping
//! across the upper tank for N seconds, then prints the CPU it used (the
//! T4 acceptance measurement; real input can't be scripted without
//! accessibility permissions). It always starts a fresh tank and saves
//! nothing.
//!
//! Saves (T8, [`crate::persist`]): the window opens the saved tank (at its
//! saved size, letterboxed) or, with none, a new starter scene. It saves
//! every 5 minutes while visible, when hidden and on close. `--fresh`
//! starts a new tank (saved as the newest, the older saves kept);
//! `--no-save` neither loads nor saves. Optional catch-up applies elapsed
//! wall-clock time after reopening a save or resuming from a power pause
//! (not from `desktank pause`, which stops the tank's time).
//!
//! IPC (SPEC §4.3) is served by the first window holding the instance lock.
//! Commands wake the event loop, which owns and edits the tank.

use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc::Sender};
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context as _;
use desktank_platform::{
    PauseReason, PowerDecision, PowerMonitor, PowerPolicy, PowerSource, process_stats,
};
use desktank_render::{Gpu, GpuOptions, Tank, TankConfig, TankSurface, wgpu};
use desktank_sim::cheats::{self, Konami};
use desktank_sim::tools::Button;
use desktank_sim::{MaterialTable, World, WorldConfig};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::InteractState;
use crate::config::{self, CliOverrides, Config, Life};
use crate::controls::{Controls, Response, Session};
use crate::ipc::{self, Reply, Request};
use crate::persist::Persistence;
use desktank_sim::scene;

/// Screen pixels per cell (SPEC §2.1 default).
const CELL_SCALE: u32 = 4;
/// Largest grid side: WebGL2's texture limit, kept natively too.
const MAX_GRID_SIDE: u32 = 2048;
/// Pointer sample interval of the demo pour (a 60 Hz mouse).
const DEMO_SAMPLE: Duration = Duration::from_micros(16_667);
/// World seed for the window's starter scene.
const SEED: u64 = 7;

enum Wake {
    Power,
    Config,
    Ipc(Request, Sender<Reply>),
}

/// Which tank the window opens, and whether it saves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Saves {
    /// Open the saved tank (or a new one) and save it.
    Load,
    /// Start a new tank and save it.
    Fresh,
    /// Start a new tank and save nothing.
    Off,
}

/// Opens the window and runs until it is closed.
///
/// # Errors
///
/// Returns an error if the event loop, window, GPU or world can't be set up.
pub fn run(
    demo_pour: Option<u64>,
    time: Option<u16>,
    fish: usize,
    saves: Saves,
    cli: CliOverrides,
) -> anyhow::Result<()> {
    let saves = if demo_pour.is_some() {
        Saves::Off
    } else {
        saves
    };
    let mut persist = match saves {
        Saves::Off => None,
        Saves::Load | Saves::Fresh => Some(Persistence::open(None)?),
    };
    let loaded = match (&mut persist, saves) {
        (Some(p), Saves::Load) => p.load(),
        _ => None,
    };
    let event_loop = EventLoop::<Wake>::with_user_event()
        .build()
        .context("creating the event loop")?;
    let proxy = event_loop.create_proxy();
    let power = PowerMonitor::new(move || {
        let _ = proxy.send_event(Wake::Power);
    });
    let config_dir = desktank_platform::config_dir();
    let config_path = config_dir.as_ref().map(|dir| dir.join(config::FILE_NAME));
    // Register before reading so edits during startup are delivered too.
    let _watcher = config_dir.as_deref().and_then(|dir| {
        let proxy = event_loop.create_proxy();
        match config::watch(dir, move || proxy.send_event(Wake::Config).is_ok()) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                eprintln!(
                    "desktank window: watching config directory {}: {error}",
                    dir.display()
                );
                None
            }
        }
    });
    let proxy = event_loop.create_proxy();
    let ipc = match ipc::Instance::start(move |request, reply| {
        proxy.send_event(Wake::Ipc(request, reply)).is_ok()
    }) {
        Ok(Some(instance)) => Some(instance),
        Ok(None) => {
            eprintln!("desktank window: another DeskTank is running; IPC is off for this window");
            None
        }
        Err(error) => {
            eprintln!("desktank window: IPC unavailable: {error:#}");
            None
        }
    };
    let settings = config_path
        .as_deref()
        .and_then(|path| read_config(path).ok());
    let mut app = App {
        instance: wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
            Box::new(event_loop.owned_display_handle()),
        )),
        origin: Instant::now(),
        time,
        fish,
        cli,
        config_path,
        settings: config::merge(settings.unwrap_or_default(), cli),
        power,
        ipc,
        user_paused: false,
        interact: false,
        state: None,
        error: None,
        persist,
        saved: loaded,
        demo: demo_pour.map(|secs| Demo {
            length: Duration::from_secs(secs),
            start: None,
        }),
    };
    event_loop
        .run_app(&mut app)
        .context("running the event loop")?;
    app.error.map_or(Ok(()), Err)
}

struct App {
    instance: wgpu::Instance,
    origin: Instant,
    /// Time of day to light the tank for (minutes after midnight).
    time: Option<u16>,
    /// Creatures to stock the tank with.
    fish: usize,
    cli: CliOverrides,
    config_path: Option<PathBuf>,
    settings: Config,
    power: PowerMonitor,
    ipc: Option<ipc::Instance>,
    user_paused: bool,
    interact: bool,
    state: Option<Running>,
    error: Option<anyhow::Error>,
    /// The saved tank, unless saving is off.
    persist: Option<Persistence>,
    /// The loaded tank and its save path, until the window opens.
    saved: Option<(World, PathBuf)>,
    demo: Option<Demo>,
}

/// A scripted pour: a synthetic left drag for `length`.
struct Demo {
    length: Duration,
    /// When it started, and the process CPU time then.
    start: Option<(Instant, Duration)>,
}

impl Demo {
    /// Moves the synthetic pointer; returns when to sample next, or `None`
    /// once the pour is over.
    fn step(&mut self, s: &mut Running) -> Option<Instant> {
        let now = Instant::now();
        let (width, height) = s.surface.size();
        let Some((t0, cpu0)) = self.start else {
            self.start = Some((now, process_stats().cpu_time));
            let mut ui = s.ui();
            ui.point_at(Some((f64::from(width) * 0.15, f64::from(height) * 0.3)));
            ui.pressed(Button::Primary);
            return Some(now + DEMO_SAMPLE);
        };
        let t = now - t0;
        if t >= self.length {
            s.ui().release(Button::Primary);
            let cpu = process_stats().cpu_time.saturating_sub(cpu0);
            eprintln!(
                "desktank window: demo pour {:.1} s, CPU {:.2} s = {:.1}% of a core",
                t.as_secs_f64(),
                cpu.as_secs_f64(),
                100.0 * cpu.as_secs_f64() / t.as_secs_f64()
            );
            return None;
        }
        // Sweep back and forth across the middle 70%, about once per 3 s.
        let f = (t.as_secs_f64() * 2.0).sin().mul_add(0.5, 0.5);
        s.ui().point_at(Some((
            f64::from(width) * f.mul_add(0.7, 0.15),
            f64::from(height) * 0.3,
        )));
        Some(now + DEMO_SAMPLE)
    }
}

/// Why the window might not be visible.
#[derive(Clone, Copy, Default)]
#[allow(clippy::struct_excessive_bools)] // independent OS signals
struct Hidden {
    occluded: bool,
    /// Zero-sized (how Windows reports minimising).
    zero_size: bool,
    /// Minimised, as the window state reports it.
    minimised: bool,
}

struct Running {
    window: Arc<Window>,
    gpu: Gpu,
    surface: TankSurface,
    tank: Tank,
    hidden: Hidden,
    /// Power/visibility decision as of the last `about_to_wait`.
    last_decision: PowerDecision,
    /// Start of the current power pause, on a clock that includes system sleep.
    paused_at: Option<SystemTime>,
    controls: Controls,
}

impl Running {
    /// Opens the window on `saved`, or on a new starter scene sized to it.
    fn new(
        instance: &wgpu::Instance,
        event_loop: &ActiveEventLoop,
        saved: Option<World>,
    ) -> anyhow::Result<Self> {
        let attrs = Window::default_attributes()
            .with_title("DeskTank")
            .with_inner_size(LogicalSize::new(1280.0, 720.0))
            .with_min_inner_size(LogicalSize::new(160.0, 90.0));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .context("creating the window")?,
        );
        let surface = instance
            .create_surface(Arc::clone(&window))
            .context("creating the GPU surface")?;
        let gpu = pollster::block_on(Gpu::new(instance, Some(&surface), GpuOptions::default()))?;
        let size = window.inner_size();
        let surface = TankSurface::new(&gpu, surface, size.width, size.height)?;
        let world = if let Some(world) = saved {
            world
        } else {
            // The grid covers the window as first opened; later resizes
            // rescale.
            let cells = |px: u32| (px / CELL_SCALE).clamp(16, MAX_GRID_SIDE) as usize;
            let config = WorldConfig {
                width: cells(size.width),
                height: cells(size.height),
                seed: SEED,
                ..WorldConfig::default()
            };
            let mut world = World::new(config, MaterialTable::builtin()?)?;
            scene::starter(&mut world, SEED)?;
            world
        };
        let controls = Controls::new(&world, Konami::new())?;
        let info = gpu.adapter.get_info();
        eprintln!(
            "desktank window: {} ({:?}), {}x{} cells, {}x{} px, {:?}",
            info.name,
            info.backend,
            world.width(),
            world.height(),
            size.width,
            size.height,
            surface.format(),
        );
        let mut s = Self {
            window,
            gpu,
            surface,
            tank: Tank::new(world, TankConfig::default()),
            hidden: Hidden::default(),
            last_decision: PowerDecision::Run {
                max_fps: TankConfig::default().max_fps,
            },
            paused_at: None,
            controls,
        };
        s.sync();
        Ok(s)
    }

    fn surface_visible(&self) -> bool {
        let h = self.hidden;
        !h.occluded && !h.zero_size && !h.minimised
    }

    fn decision(
        &self,
        policy: &PowerPolicy,
        power: &PowerMonitor,
        user_paused: bool,
    ) -> PowerDecision {
        if user_paused {
            PowerDecision::Pause(PauseReason::User)
        } else {
            policy.evaluate(power.signals(), !self.surface_visible())
        }
    }

    fn resized(&mut self, size: PhysicalSize<u32>) {
        self.hidden.zero_size = size.width == 0 || size.height == 0;
        if self.hidden.zero_size {
            return;
        }
        self.surface.resize(&self.gpu, size.width, size.height);
        self.tank.invalidate();
    }

    /// The controls, borrowed with the tank.
    fn ui(&mut self) -> Session<'_> {
        let size = self.surface.size();
        self.controls.session(&mut self.tank, size)
    }

    /// Pushes the palette and brush to the tank, and any new text to the
    /// title bar.
    fn sync(&mut self) {
        self.ui().sync();
        if let Some(title) = self.controls.take_title() {
            self.window.set_title(&title);
        }
    }
}

fn read_config(path: &Path) -> Result<Config, config::ConfigError> {
    config::load(path)
        .map(Option::unwrap_or_default)
        .inspect_err(|error| {
            eprintln!("desktank window: config {}: {error}", path.display());
        })
}

fn apply_life(world: &mut World, life: Life, previous: Life) {
    if let Some(easy) = life.easy_mode
        && life.easy_mode != previous.easy_mode
    {
        world.set_easy_mode(easy);
    }
    if let Some(cap) = life.population_cap
        && life.population_cap != previous.population_cap
    {
        let cap = usize::from(cap);
        let set = world.set_creature_cap(cap);
        if set != cap {
            eprintln!(
                "desktank window: the tank holds {} creatures, so its cap is {set}, not {cap}",
                world.stock_count()
            );
        }
    }
}

/// Bounds a wall-clock gap and skips short absences or a clock that went back.
fn catch_up_seconds(since: SystemTime, now: SystemTime) -> u32 {
    let elapsed = now.duration_since(since).unwrap_or_default().as_secs();
    desktank_sim::catch_up_seconds(elapsed)
}

fn catch_up(tank: &mut Tank, since: SystemTime) {
    let seconds = catch_up_seconds(since, SystemTime::now());
    if seconds == 0 {
        return;
    }
    // Catch-up advances the world tick, which persistence uses to detect changes.
    let report = tank.world_mut().catch_up(seconds);
    tank.cells_edited();
    let minutes = report.seconds / 60;
    let elapsed = if minutes < 60 {
        format!("{minutes}m")
    } else {
        format!("{}h {}m", minutes / 60, minutes % 60)
    };
    let births = report.births;
    let deaths = report.old_age_deaths + report.starvation_deaths + report.poor_water_deaths;
    let birth_suffix = if births == 1 { "" } else { "s" };
    let death_suffix = if deaths == 1 { "" } else { "s" };
    eprintln!(
        "desktank window: caught up {elapsed} ({births} birth{birth_suffix}, {deaths} death{death_suffix})"
    );
}

impl App {
    fn handle_ipc(&mut self, request: Request) -> Reply {
        let s = self.state.as_mut().ok_or("starting up")?;
        let message = match request {
            Request::Feed(food) => {
                let changed = cheats::feed_with(s.tank.world_mut(), food);
                if changed > 0 {
                    s.tank.cells_edited();
                }
                if self.user_paused {
                    format!("fed {changed} cells (shown when resumed)")
                } else {
                    format!("fed {changed} cells")
                }
            }
            Request::Pause => {
                self.user_paused = !self.user_paused;
                format!("paused={}", self.user_paused)
            }
            Request::Interact(state) => {
                self.interact = match state {
                    InteractState::On => true,
                    InteractState::Off => false,
                    InteractState::Toggle => !self.interact,
                };
                let state = if self.interact { "on" } else { "off" };
                format!("interact={state} (the window is always interactive)")
            }
            Request::Status => {
                let (paused, fps) =
                    match s.decision(&self.settings.power, &self.power, self.user_paused) {
                        PowerDecision::Pause(_) => (true, 0),
                        PowerDecision::Run { max_fps } => (false, max_fps),
                    };
                let creatures = s.tank.world().creatures().len();
                format!(
                    "paused={paused} interact={} fps={fps} creatures={creatures}",
                    self.interact
                )
            }
        };
        Ok(message)
    }
}

impl ApplicationHandler<Wake> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let (saved, path) = self.saved.take().unzip();
        let fresh = saved.is_none();
        match Running::new(&self.instance, event_loop, saved) {
            Ok(mut s) => {
                if let Some(minute) = self.time {
                    s.tank.set_time_of_day(minute);
                }
                s.tank.set_sway(self.settings.plants.sway);
                s.tank
                    .set_allow_sway_on_battery(self.settings.plants.sway_on_battery);
                s.tank.set_on_battery(matches!(
                    self.power.signals().source,
                    PowerSource::Battery { .. }
                ));
                apply_life(s.tank.world_mut(), self.settings.life, Life::default());
                if self.settings.life.catch_up
                    && self.persist.as_ref().is_some_and(Persistence::owns)
                    && let Some(path) = path
                {
                    match path.metadata().and_then(|metadata| metadata.modified()) {
                        Ok(modified) => catch_up(&mut s.tank, modified),
                        Err(e) => eprintln!(
                            "desktank window: no catch-up, can't read when {} was saved: {e}",
                            path.display()
                        ),
                    }
                }
                // A loaded tank keeps the creatures it had.
                let fish = if fresh { self.fish } else { 0 };
                if let Err(e) = scene::stock(s.tank.world_mut(), fish, SEED) {
                    self.error = Some(e.into());
                    event_loop.exit();
                    return;
                }
                self.state = Some(s);
            }
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, _: &ActiveEventLoop, wake: Wake) {
        if let Wake::Ipc(request, reply) = wake {
            let _ = reply.send(self.handle_ipc(request));
            return;
        }
        // Power wakes and config changes both go through the existing power
        // decision and frame-cap update in about_to_wait.
        if let Wake::Config = wake
            && let Some(path) = &self.config_path
            && let Ok(config) = read_config(path)
        {
            let settings = config::merge(config, self.cli);
            if let Some(s) = &mut self.state {
                // Only changed keys apply, so an unrelated edit keeps the
                // sway the keyboard toggles chose.
                let (new, old) = (settings.plants, self.settings.plants);
                if new.sway != old.sway {
                    s.tank.set_sway(new.sway);
                }
                if new.sway_on_battery != old.sway_on_battery {
                    s.tank.set_allow_sway_on_battery(new.sway_on_battery);
                }
                apply_life(s.tank.world_mut(), settings.life, self.settings.life);
            }
            self.settings = settings;
            if path.exists() {
                eprintln!("desktank window: reloaded config {}", path.display());
            } else {
                eprintln!(
                    "desktank window: config {} removed, using defaults",
                    path.display()
                );
            }
        }
    }

    #[allow(clippy::too_many_lines)] // one arm per event and key binding
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(s) = self.state.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                s.resized(size);
                if matches!(
                    s.decision(&self.settings.power, &self.power, self.user_paused),
                    PowerDecision::Run { .. }
                ) {
                    s.window.request_redraw();
                }
            }
            WindowEvent::Occluded(occluded) => {
                s.hidden.occluded = occluded;
                if matches!(
                    s.decision(&self.settings.power, &self.power, self.user_paused),
                    PowerDecision::Run { .. }
                ) {
                    s.tank.invalidate();
                    s.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                // The transition in about_to_wait catches up before drawing again.
                if s.paused_at.is_none()
                    && matches!(
                        s.decision(&self.settings.power, &self.power, self.user_paused),
                        PowerDecision::Run { .. }
                    )
                    && let Err(e) = s.tank.draw(&s.gpu, &s.surface)
                {
                    self.error = Some(e.into());
                    event_loop.exit();
                }
            }
            _ => {
                let size = s.surface.size();
                match s.controls.session(&mut s.tank, size).window_event(&event) {
                    Response::Ignored => return,
                    Response::Escape => event_loop.exit(),
                    Response::Handled => {}
                }
            }
        }
        s.sync();
    }

    /// Saves on the way out. On macOS, quitting from the menu or the Dock
    /// ends the process right after this, without returning from
    /// `run_app`.
    fn exiting(&mut self, _: &ActiveEventLoop) {
        if let (Some(p), Some(s)) = (&mut self.persist, &self.state) {
            p.save(s.tank.world(), "saved on exit");
        }
        // Released after the save, so a new instance starts once it's done.
        self.ipc.take();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(s) = self.state.as_mut() else { return };
        // Not every platform sends an event for minimising, so ask.
        s.hidden.minimised = s.window.is_minimized() == Some(true);
        let on_battery = matches!(self.power.signals().source, PowerSource::Battery { .. });
        s.tank.set_on_battery(on_battery);
        let decision = s.decision(&self.settings.power, &self.power, self.user_paused);
        s.tank
            .set_hidden(matches!(decision, PowerDecision::Pause(_)));
        if decision != s.last_decision {
            match (s.last_decision, decision) {
                (previous, PowerDecision::Pause(reason)) => {
                    if reason == PauseReason::User {
                        eprintln!("desktank window: paused");
                    } else {
                        eprintln!("desktank window: {reason}, paused");
                    }
                    // Catch-up counts from here, so time under a user pause
                    // (which stops the tank) is never caught up.
                    if matches!(
                        previous,
                        PowerDecision::Run { .. } | PowerDecision::Pause(PauseReason::User)
                    ) {
                        s.paused_at = Some(SystemTime::now());
                    }
                    if matches!(previous, PowerDecision::Run { .. })
                        && let Some(p) = &mut self.persist
                    {
                        // Paused from here on, so nothing changes until it runs
                        // again.
                        p.save(s.tank.world(), "saved while paused");
                    }
                }
                (PowerDecision::Pause(reason), PowerDecision::Run { max_fps }) => {
                    let paused_at = s.paused_at.take();
                    // `desktank pause` stops the tank's time, so leaving it
                    // doesn't catch up.
                    if reason != PauseReason::User
                        && self.settings.life.catch_up
                        && self.persist.as_ref().is_some_and(Persistence::owns)
                        && let Some(paused_at) = paused_at
                    {
                        catch_up(&mut s.tank, paused_at);
                    }
                    let how = if reason == PauseReason::User {
                        "resumed"
                    } else {
                        "visible"
                    };
                    eprintln!("desktank window: {how} ({max_fps} fps)");
                    s.tank.set_max_fps(max_fps);
                    s.tank.invalidate();
                }
                (PowerDecision::Run { .. }, PowerDecision::Run { max_fps }) => {
                    eprintln!("desktank window: frame cap {max_fps} fps");
                    s.tank.set_max_fps(max_fps);
                }
            }
            s.last_decision = decision;
        }
        if matches!(decision, PowerDecision::Pause(_)) {
            // Paused: world time stops and only events wake the loop.
            s.tank.pause();
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        let demo_wake = self.demo.as_mut().and_then(|d| {
            let wake = d.step(s);
            s.sync();
            wake
        });
        if demo_wake.is_none() {
            self.demo = None;
        }
        let req = s.tank.tick(self.origin.elapsed());
        if let Some(p) = &mut self.persist {
            p.autosave(s.tank.world(), Instant::now());
        }
        // Holding still keeps pouring, once per tick run.
        for _ in 0..req.sim_ticks {
            s.ui().hold();
        }
        if req.redraw {
            s.window.request_redraw();
        }
        let wake = self.origin + req.wake_at;
        let wake = demo_wake.map_or(wake, |d| d.min(wake));
        event_loop.set_control_flow(ControlFlow::WaitUntil(wake));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catch_up_ignores_short_gaps_and_backward_clocks() {
        let since = SystemTime::UNIX_EPOCH + Duration::from_hours(1);
        assert_eq!(catch_up_seconds(since, SystemTime::UNIX_EPOCH), 0);
        assert_eq!(
            catch_up_seconds(since, since + Duration::from_millis(59_999)),
            0
        );
        assert_eq!(catch_up_seconds(since, since + Duration::from_mins(1)), 60);
        assert_eq!(catch_up_seconds(since, since + Duration::from_secs(87)), 87);
    }

    #[test]
    fn catch_up_caps_long_gaps_at_24_hours() {
        let since = SystemTime::UNIX_EPOCH;
        assert_eq!(
            catch_up_seconds(since, since + Duration::from_hours(72)),
            desktank_sim::MAX_CATCH_UP_SECONDS
        );
    }
}
