//! Desktop click-through overlay.
//!
//! Interact mode (SPEC §2.5, §3.3, O5): holding the chord (default ⌃⌥,
//! `[interact] chord`) makes the overlay take the pointer, so the window's
//! tools, palette and species card ([`crate::controls`]) work over the
//! desktop; letting go makes it click-through again. The chord is polled at
//! 10 Hz, only while the tank is visible and running. `desktank interact
//! on|toggle` (IPC) turns it on until Esc, `desktank interact off|toggle`,
//! or focus moving to another app; it then also takes key focus for Esc,
//! the cheat field and naming. Focus goes back to the app that was in
//! front. The overlay serves IPC (`interact`, `feed`, `pause`, `status`)
//! when it holds the instance lock. Text the window puts in its title bar
//! goes to the log until the drawn in-tank card lands.

#[cfg(not(target_os = "macos"))]
pub fn run() -> anyhow::Result<()> {
    anyhow::bail!("overlay mode is not yet implemented on this OS");
}

#[cfg(target_os = "macos")]
pub use macos::run;

#[cfg(target_os = "macos")]
mod macos {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, mpsc::Sender};
    use std::time::{Duration, Instant, SystemTime};

    use anyhow::Context;
    use desktank_platform::{
        FullScreenMonitor, Interactivity, Modifiers, OverlayOptions, PauseReason, PowerDecision,
        PowerMonitor, PowerSource, configure_overlay_window, held_modifiers, hide_overlay,
        local_minute, set_overlay_capture_excluded, set_overlay_interactivity,
        show_overlay_without_focus,
    };
    use desktank_render::{Gpu, GpuOptions, Tank, TankConfig, TankSurface, wgpu};
    use desktank_sim::cheats::{self, Konami};
    use desktank_sim::{MAX_CATCH_UP_SECONDS, MaterialTable, World, WorldConfig, scene};
    use winit::application::ApplicationHandler;
    use winit::dpi::PhysicalSize;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
    use winit::keyboard::ModifiersState;
    use winit::platform::macos::{
        ActivationPolicy, EventLoopBuilderExtMacOS, WindowAttributesExtMacOS,
    };
    use winit::window::{Window, WindowId};

    use crate::InteractState;
    use crate::config::{self, Config, Life};
    use crate::controls::{Controls, Response, Session};
    use crate::ipc::{self, Reply, Request};
    use crate::persist::Persistence;

    enum Wake {
        Power,
        FullScreen,
        Config,
        Ipc(Request, Sender<Reply>),
    }

    /// How the overlay is taking input.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Mode {
        /// Click-through.
        Off,
        /// The chord is held: the pointer, no key focus asked for.
        Held,
        /// Toggled on (IPC): the pointer and key focus, until Esc.
        Toggled,
    }

    /// The chord poll interval (SPEC §3.3): with one frame, release restores
    /// click-through within 150 ms.
    const CHORD_POLL: Duration = Duration::from_millis(100);

    pub fn run() -> anyhow::Result<()> {
        let persist = Persistence::open(None)?;
        let event_loop = EventLoop::<Wake>::with_user_event()
            .with_activation_policy(ActivationPolicy::Accessory)
            .with_activate_ignoring_other_apps(false)
            .with_default_menu(false)
            .build()
            .context("creating the overlay event loop")?;
        let proxy = event_loop.create_proxy();
        let power = PowerMonitor::new(move || {
            let _ = proxy.send_event(Wake::Power);
        });
        let proxy = event_loop.create_proxy();
        let fullscreen = FullScreenMonitor::new(move || {
            let _ = proxy.send_event(Wake::FullScreen);
        });
        let config_dir = desktank_platform::config_dir();
        let config_path = config_dir.as_ref().map(|dir| dir.join(config::FILE_NAME));
        let _watcher = config_dir.as_deref().and_then(|dir| {
            let proxy = event_loop.create_proxy();
            config::watch(dir, move || proxy.send_event(Wake::Config).is_ok())
                .inspect_err(|error| {
                    eprintln!("desktank overlay: watching {}: {error}", dir.display());
                })
                .ok()
        });
        let proxy = event_loop.create_proxy();
        let ipc = match ipc::Instance::start(move |request, reply| {
            proxy.send_event(Wake::Ipc(request, reply)).is_ok()
        }) {
            Ok(Some(instance)) => Some(instance),
            Ok(None) => {
                eprintln!("desktank overlay: another DeskTank is running; IPC is off here");
                None
            }
            Err(error) => {
                eprintln!("desktank overlay: IPC unavailable: {error:#}");
                None
            }
        };
        let settings = config_path
            .as_deref()
            .and_then(|path| read_config(path).ok())
            .unwrap_or_default();
        let mut app = App {
            instance: wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(
                    event_loop.owned_display_handle(),
                )),
            ),
            origin: Instant::now(),
            power,
            fullscreen,
            config_path,
            settings,
            persist,
            ipc,
            user_paused: false,
            state: None,
            error: None,
        };
        event_loop
            .run_app(&mut app)
            .context("running the overlay")?;
        app.error.map_or(Ok(()), Err)
    }

    struct App {
        instance: wgpu::Instance,
        origin: Instant,
        power: PowerMonitor,
        fullscreen: FullScreenMonitor,
        config_path: Option<PathBuf>,
        settings: Config,
        persist: Persistence,
        ipc: Option<ipc::Instance>,
        /// `desktank pause`: the tank's time stops (no catch-up after).
        user_paused: bool,
        state: Option<Running>,
        error: Option<anyhow::Error>,
    }

    /// One-shot delay after a Space/full-screen notification so the bounds
    /// check also sees the post-animation window list.
    const FULLSCREEN_RECHECK_DELAY: Duration = Duration::from_millis(400);

    #[allow(clippy::struct_excessive_bools)] // independent visibility and input states
    struct Running {
        window: Arc<Window>,
        gpu: Gpu,
        surface: TankSurface,
        tank: Tank,
        size: PhysicalSize<u32>,
        fullscreen_hidden: bool,
        occluded: bool,
        paused_at: Option<SystemTime>,
        /// The pause began with `desktank pause`, so resuming doesn't catch up.
        user_pause: bool,
        next_clock_update: Instant,
        /// Draw one static frame even while power-paused (startup, resize,
        /// unhide, or tint change) so a visible overlay is never blank.
        needs_static_frame: bool,
        /// Optional one-shot re-check deadline after a Space/app transition.
        fullscreen_recheck_at: Option<Instant>,
        controls: Controls,
        mode: Mode,
        /// Toggled mode has had key focus, so losing it ends the mode.
        had_focus: bool,
        next_chord_poll: Instant,
    }

    impl Running {
        fn new(
            instance: &wgpu::Instance,
            event_loop: &ActiveEventLoop,
            settings: Config,
            fullscreen: &FullScreenMonitor,
            persist: &mut Persistence,
        ) -> anyhow::Result<Self> {
            let monitor = event_loop
                .primary_monitor()
                .or_else(|| event_loop.available_monitors().next())
                .context("no display for the overlay")?;
            let attrs = Window::default_attributes()
                .with_title("DeskTank Overlay")
                .with_decorations(false)
                .with_transparent(true)
                .with_has_shadow(false)
                .with_resizable(false)
                .with_active(false)
                .with_visible(false)
                .with_position(monitor.position())
                .with_inner_size(monitor.size());
            let window = Arc::new(
                event_loop
                    .create_window(attrs)
                    .context("creating the overlay window")?,
            );
            configure_overlay_window(
                &*window,
                OverlayOptions {
                    exclude_from_capture: settings.overlay.exclude_from_capture,
                    click_through: true,
                },
            )?;
            let fullscreen_hidden = fullscreen.is_fullscreen_app_active();
            if fullscreen_hidden {
                hide_overlay(&*window)?;
            }

            let surface = instance.create_surface(Arc::clone(&window))?;
            let gpu =
                pollster::block_on(Gpu::new(instance, Some(&surface), GpuOptions::default()))?;
            let size = window.inner_size();
            let surface = TankSurface::new_transparent(&gpu, surface, size.width, size.height)?;
            let (world, path) = if let Some((world, path)) = persist.load() {
                (world, Some(path))
            } else {
                const SEED: u64 = 7;
                let cells = |px: u32| (px / 4).clamp(1, 2048) as usize;
                let mut world = World::new(
                    WorldConfig {
                        width: cells(size.width),
                        height: cells(size.height),
                        seed: SEED,
                        ..WorldConfig::default()
                    },
                    MaterialTable::builtin()?,
                )?;
                scene::starter(&mut world, SEED)?;
                (world, None)
            };
            let controls = Controls::new(&world, Konami::with_chord())?;
            let mut tank = Tank::new(world, TankConfig::default());
            apply_settings(&mut tank, settings, Life::default());
            if settings.life.catch_up
                && persist.owns()
                && let Some(path) = path
            {
                match path.metadata().and_then(|metadata| metadata.modified()) {
                    Ok(modified) => catch_up(&mut tank, modified),
                    Err(e) => eprintln!(
                        "desktank overlay: no catch-up, can't read when {} was saved: {e}",
                        path.display()
                    ),
                }
            }
            Ok(Self {
                window,
                gpu,
                surface,
                tank,
                size,
                fullscreen_hidden,
                occluded: false,
                paused_at: None,
                user_pause: false,
                next_clock_update: Instant::now(),
                needs_static_frame: true,
                fullscreen_recheck_at: None,
                controls,
                mode: Mode::Off,
                had_focus: false,
                next_chord_poll: Instant::now(),
            })
        }

        /// The controls, borrowed with the tank.
        fn ui(&mut self) -> Session<'_> {
            let size = (self.size.width, self.size.height);
            self.controls.session(&mut self.tank, size)
        }

        /// Pushes the palette and brush to the tank, and logs any text the
        /// window would show in its title bar.
        fn sync(&mut self) {
            if self.mode != Mode::Off {
                self.ui().sync();
            }
            if let Some(title) = self.controls.take_title()
                && let Some(text) = title.strip_prefix("DeskTank — ")
            {
                eprintln!("desktank overlay: {text}");
            }
        }

        /// Switches interact mode; `chord` is the configured chord.
        fn set_mode(&mut self, mode: Mode, chord: Modifiers) -> anyhow::Result<()> {
            if mode == self.mode {
                return Ok(());
            }
            let interactivity = match mode {
                Mode::Off => Interactivity::ClickThrough,
                Mode::Held => Interactivity::Pointer,
                Mode::Toggled => Interactivity::Focused,
            };
            set_overlay_interactivity(&*self.window, interactivity)?;
            match mode {
                Mode::Off => {
                    self.ui().leave();
                    self.controls.set_chord(None);
                }
                Mode::Held => self.controls.set_chord(Some(winit_modifiers(chord))),
                Mode::Toggled => self.controls.set_chord(Some(ModifiersState::empty())),
            }
            self.had_focus = false;
            self.mode = mode;
            self.sync();
            eprintln!(
                "desktank overlay: interact {}",
                match mode {
                    Mode::Off => "off",
                    Mode::Held => "on (chord held)",
                    Mode::Toggled => "on (Esc ends it)",
                }
            );
            Ok(())
        }

        /// Reads the chord: holding it enters interact mode, letting go
        /// leaves it (a toggled mode stays).
        fn poll_chord(&mut self, chord: Modifiers) -> anyhow::Result<()> {
            let held = held_modifiers().is_some_and(|m| m.contains(chord));
            match (self.mode, held) {
                (Mode::Off, true) => self.set_mode(Mode::Held, chord),
                (Mode::Held, false) => self.set_mode(Mode::Off, chord),
                _ => Ok(()),
            }
        }

        fn update_fullscreen(&mut self, active: bool) -> anyhow::Result<()> {
            if active != self.fullscreen_hidden {
                if active {
                    self.set_mode(Mode::Off, Modifiers::NONE)?;
                    hide_overlay(&*self.window)?;
                } else {
                    show_overlay_without_focus(&*self.window)?;
                    self.tank.invalidate();
                    self.needs_static_frame = true;
                }
                self.fullscreen_hidden = active;
            }
            Ok(())
        }

        fn resize(&mut self, size: PhysicalSize<u32>) {
            self.size = size;
            if size.width != 0 && size.height != 0 {
                self.surface.resize(&self.gpu, size.width, size.height);
                self.tank.invalidate();
                self.needs_static_frame = true;
            }
        }
    }

    impl App {
        fn reload_config(&mut self) {
            if let Some(path) = &self.config_path
                && let Ok(mut settings) = read_config(path)
            {
                if let Some(s) = &mut self.state {
                    if settings.overlay.exclude_from_capture
                        != self.settings.overlay.exclude_from_capture
                        && let Err(error) = set_overlay_capture_excluded(
                            &*s.window,
                            settings.overlay.exclude_from_capture,
                        )
                    {
                        eprintln!("desktank overlay: failed to update capture exclusion: {error}");
                        settings.overlay.exclude_from_capture =
                            self.settings.overlay.exclude_from_capture;
                    }
                    if settings.overlay.tint.to_bits() != self.settings.overlay.tint.to_bits() {
                        s.needs_static_frame = true;
                    }
                    apply_settings(&mut s.tank, settings, self.settings.life);
                }
                self.settings = settings;
            }
        }

        fn evaluate_state(&mut self, event_loop: &ActiveEventLoop) -> anyhow::Result<()> {
            let Some(s) = &mut self.state else {
                return Ok(());
            };
            let now = Instant::now();
            let fullscreen_active = if s.fullscreen_recheck_at.is_some_and(|at| now >= at) {
                s.fullscreen_recheck_at = None;
                self.fullscreen.refresh()
            } else {
                self.fullscreen.is_fullscreen_app_active()
            };
            s.update_fullscreen(fullscreen_active)?;
            let surface_hidden =
                s.occluded || fullscreen_active || s.size.width == 0 || s.size.height == 0;
            let signals = self.power.signals();
            let decision = if self.user_paused {
                PowerDecision::Pause(PauseReason::User)
            } else {
                self.settings.power.evaluate(signals, surface_hidden)
            };
            let PowerDecision::Run { max_fps } = decision else {
                if !surface_hidden && s.needs_static_frame {
                    s.tank.draw(&s.gpu, &s.surface)?;
                    s.needs_static_frame = false;
                }
                if s.paused_at.is_none() {
                    s.paused_at = Some(SystemTime::now());
                    s.user_pause = self.user_paused;
                    self.persist.save(s.tank.world(), "saved while paused");
                    s.tank.set_hidden(true);
                    s.tank.pause();
                }
                if let Some(recheck_at) = s.fullscreen_recheck_at {
                    event_loop.set_control_flow(ControlFlow::WaitUntil(recheck_at));
                } else {
                    event_loop.set_control_flow(ControlFlow::Wait);
                }
                return Ok(());
            };

            if let Some(paused_at) = s.paused_at.take() {
                if self.settings.life.catch_up && self.persist.owns() && !s.user_pause {
                    catch_up(&mut s.tank, paused_at);
                }
                s.tank.set_hidden(false);
                s.tank.invalidate();
                s.next_clock_update = now;
            }
            s.tank.set_max_fps(max_fps);
            s.tank
                .set_on_battery(matches!(signals.source, PowerSource::Battery { .. }));
            if now >= s.next_clock_update {
                if let Some(minute) = local_minute() {
                    s.tank.set_time_of_day(minute);
                }
                s.next_clock_update = now + Duration::from_mins(1);
            }
            let chord = self.settings.interact.chord.0;
            if !chord.is_empty() && now >= s.next_chord_poll {
                s.poll_chord(chord)?;
                s.next_chord_poll = now + CHORD_POLL;
            }
            let req = s.tank.tick(self.origin.elapsed());
            if s.mode != Mode::Off {
                // Holding still keeps pouring, once per tick run.
                for _ in 0..req.sim_ticks {
                    s.ui().hold();
                }
            }
            if req.redraw || s.needs_static_frame {
                s.tank.draw(&s.gpu, &s.surface)?;
                s.needs_static_frame = false;
            }
            self.persist.autosave(s.tank.world(), Instant::now());
            let mut wake_at = self.origin + req.wake_at;
            if let Some(recheck_at) = s.fullscreen_recheck_at {
                wake_at = wake_at.min(recheck_at);
            }
            if !chord.is_empty() {
                wake_at = wake_at.min(s.next_chord_poll);
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(wake_at));
            Ok(())
        }

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
                    let on = match state {
                        InteractState::On => true,
                        InteractState::Off => false,
                        InteractState::Toggle => s.mode != Mode::Toggled,
                    };
                    if on && s.fullscreen_hidden {
                        return Err("the overlay is hidden behind a full-screen app");
                    }
                    let mode = if on { Mode::Toggled } else { Mode::Off };
                    s.set_mode(mode, self.settings.interact.chord.0)
                        .map_err(|error| {
                            eprintln!("desktank overlay: interact: {error:#}");
                            "couldn't switch interact mode"
                        })?;
                    format!("interact={}", if on { "on" } else { "off" })
                }
                Request::Status => {
                    let paused = s.paused_at.is_some();
                    let creatures = s.tank.world().creatures().len();
                    format!(
                        "paused={paused} interact={} creatures={creatures}",
                        s.mode != Mode::Off
                    )
                }
            };
            Ok(message)
        }

        fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
            self.error = Some(error);
            event_loop.exit();
        }
    }

    impl ApplicationHandler<Wake> for App {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.state.is_none() {
                match Running::new(
                    &self.instance,
                    event_loop,
                    self.settings,
                    &self.fullscreen,
                    &mut self.persist,
                ) {
                    Ok(state) => self.state = Some(state),
                    Err(error) => self.fail(event_loop, error),
                }
            }
        }

        fn user_event(&mut self, _event_loop: &ActiveEventLoop, wake: Wake) {
            match wake {
                Wake::Power => {}
                Wake::FullScreen => {
                    if let Some(s) = &mut self.state {
                        s.fullscreen_recheck_at = Some(Instant::now() + FULLSCREEN_RECHECK_DELAY);
                    }
                }
                Wake::Config => self.reload_config(),
                Wake::Ipc(request, reply) => {
                    let _ = reply.send(self.handle_ipc(request));
                }
            }
        }

        fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
            let Some(s) = &mut self.state else { return };
            match event {
                WindowEvent::Occluded(occluded) => s.occluded = occluded,
                WindowEvent::Resized(size) => s.resize(size),
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::RedrawRequested => s.tank.invalidate(),
                WindowEvent::Focused(true) if s.mode == Mode::Toggled => s.had_focus = true,
                WindowEvent::Focused(false) if s.mode == Mode::Toggled && s.had_focus => {
                    // Another app came to the front: Esc can't reach us now.
                    if let Err(error) = s.set_mode(Mode::Off, self.settings.interact.chord.0) {
                        self.fail(event_loop, error);
                    }
                }
                _ if s.mode != Mode::Off => match s.ui().window_event(&event) {
                    Response::Handled => s.sync(),
                    Response::Escape if s.mode == Mode::Toggled => {
                        if let Err(error) = s.set_mode(Mode::Off, self.settings.interact.chord.0) {
                            self.fail(event_loop, error);
                        }
                    }
                    Response::Ignored | Response::Escape => {}
                },
                _ => {}
            }
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            if !event_loop.exiting()
                && let Err(error) = self.evaluate_state(event_loop)
            {
                self.fail(event_loop, error);
            }
        }

        fn exiting(&mut self, _: &ActiveEventLoop) {
            if let Some(s) = &self.state {
                self.persist.save(s.tank.world(), "saved on exit");
            }
            // Released after the save, so a new instance starts once it's done.
            self.ipc.take();
        }
    }

    /// `chord` as winit's modifier state.
    fn winit_modifiers(chord: Modifiers) -> ModifiersState {
        let mut state = ModifiersState::empty();
        for (modifier, winit) in [
            (Modifiers::SHIFT, ModifiersState::SHIFT),
            (Modifiers::CONTROL, ModifiersState::CONTROL),
            (Modifiers::ALT, ModifiersState::ALT),
            (Modifiers::SUPER, ModifiersState::SUPER),
        ] {
            if chord.contains(modifier) {
                state |= winit;
            }
        }
        state
    }

    fn read_config(path: &Path) -> Result<Config, config::ConfigError> {
        config::load(path)
            .map(Option::unwrap_or_default)
            .inspect_err(|error| {
                eprintln!("desktank overlay: config {}: {error}", path.display());
            })
    }

    fn apply_settings(tank: &mut Tank, settings: Config, previous_life: Life) {
        tank.set_sway(settings.plants.sway);
        tank.set_allow_sway_on_battery(settings.plants.sway_on_battery);
        tank.set_overlay_water_alpha(Some(settings.overlay.tint));
        let life = settings.life;
        let world = tank.world_mut();
        if let Some(easy) = life.easy_mode
            && life.easy_mode != previous_life.easy_mode
        {
            world.set_easy_mode(easy);
        }
        if let Some(cap) = life.population_cap
            && life.population_cap != previous_life.population_cap
        {
            let cap = usize::from(cap);
            let set = world.set_creature_cap(cap);
            if set != cap {
                eprintln!(
                    "desktank overlay: the tank holds {} creatures, so its cap is {set}",
                    world.stock_count()
                );
            }
        }
    }

    fn catch_up(tank: &mut Tank, since: SystemTime) {
        let elapsed = since.elapsed().unwrap_or_default().as_secs();
        if elapsed >= 60 {
            let seconds = u32::try_from(elapsed)
                .unwrap_or(u32::MAX)
                .min(MAX_CATCH_UP_SECONDS);
            tank.world_mut().catch_up(seconds);
            tank.cells_edited();
        }
    }
}
