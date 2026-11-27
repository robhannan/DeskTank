//! Desktop click-through overlay.

#[cfg(not(target_os = "macos"))]
pub fn run() -> anyhow::Result<()> {
    anyhow::bail!("overlay mode is not yet implemented on this OS");
}

#[cfg(target_os = "macos")]
pub use macos::run;

#[cfg(target_os = "macos")]
mod macos {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant, SystemTime};

    use anyhow::Context;
    use desktank_platform::{
        FullScreenMonitor, OverlayOptions, PowerDecision, PowerMonitor, PowerSource,
        configure_overlay_window, hide_overlay, local_minute, set_overlay_capture_excluded,
        show_overlay_without_focus,
    };
    use desktank_render::{Gpu, GpuOptions, Tank, TankConfig, TankSurface, wgpu};
    use desktank_sim::{MAX_CATCH_UP_SECONDS, MaterialTable, World, WorldConfig, scene};
    use winit::application::ApplicationHandler;
    use winit::dpi::PhysicalSize;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
    use winit::platform::macos::{
        ActivationPolicy, EventLoopBuilderExtMacOS, WindowAttributesExtMacOS,
    };
    use winit::window::{Window, WindowId};

    use crate::config::{self, Config, Life};
    use crate::persist::Persistence;

    enum Wake {
        Power,
        FullScreen,
        Config,
    }

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
        state: Option<Running>,
        error: Option<anyhow::Error>,
    }

    /// One-shot delay after a Space/full-screen notification so the bounds
    /// check also sees the post-animation window list.
    const FULLSCREEN_RECHECK_DELAY: Duration = Duration::from_millis(400);

    struct Running {
        window: Arc<Window>,
        gpu: Gpu,
        surface: TankSurface,
        tank: Tank,
        size: PhysicalSize<u32>,
        fullscreen_hidden: bool,
        occluded: bool,
        paused_at: Option<SystemTime>,
        next_clock_update: Instant,
        /// Draw one static frame even while power-paused (startup, resize,
        /// unhide, or tint change) so a visible overlay is never blank.
        needs_static_frame: bool,
        /// Optional one-shot re-check deadline after a Space/app transition.
        fullscreen_recheck_at: Option<Instant>,
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
                next_clock_update: Instant::now(),
                needs_static_frame: true,
                fullscreen_recheck_at: None,
            })
        }

        fn update_fullscreen(&mut self, active: bool) -> anyhow::Result<()> {
            if active != self.fullscreen_hidden {
                if active {
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
            let PowerDecision::Run { max_fps } =
                self.settings.power.evaluate(signals, surface_hidden)
            else {
                if !surface_hidden && s.needs_static_frame {
                    s.tank.draw(&s.gpu, &s.surface)?;
                    s.needs_static_frame = false;
                }
                if s.paused_at.is_none() {
                    s.paused_at = Some(SystemTime::now());
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
                if self.settings.life.catch_up && self.persist.owns() {
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
                    s.tank.update_local_time(minute);
                }
                s.next_clock_update = now + Duration::from_mins(1);
            }
            let req = s.tank.tick(self.origin.elapsed());
            if req.redraw || s.needs_static_frame {
                s.tank.draw(&s.gpu, &s.surface)?;
                s.needs_static_frame = false;
            }
            self.persist.autosave(s.tank.world(), Instant::now());
            let mut wake_at = self.origin + req.wake_at;
            if let Some(recheck_at) = s.fullscreen_recheck_at {
                wake_at = wake_at.min(recheck_at);
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(wake_at));
            Ok(())
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
            }
        }

        fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
            let Some(s) = &mut self.state else { return };
            match event {
                WindowEvent::Occluded(occluded) => s.occluded = occluded,
                WindowEvent::Resized(size) => s.resize(size),
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::RedrawRequested => s.tank.invalidate(),
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
        }
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
        if let Some(ratio) = life.time_ratio
            && life.time_ratio != previous_life.time_ratio
        {
            world.set_time_ratio(ratio);
        }
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
            tank.catch_up(seconds);
            tank.cells_edited();
        }
    }
}
