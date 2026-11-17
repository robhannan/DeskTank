//! `desktank window`: the tank in a normal resizable winit window.
//!
//! Pacing (SPEC §4.2, §6.2): no continuous redraw. The loop sleeps with
//! `ControlFlow::WaitUntil` until the tank's next deadline (a 20 Hz
//! simulation tick, frames capped at 30 fps and drawn only when the world
//! changed). While the window is occluded or minimised the tank is paused
//! and the loop waits on OS events only, with no timers.
//!
//! Interaction (SPEC §2.5) uses the platform-free `desktank_sim::tools`:
//! left drag uses the selected tool (pour, scoop, net, inspect), right drag
//! scoops, the scroll wheel or a two-finger trackpad scroll sets the brush
//! size, and clicking the palette in the top-left corner (or keys 1–9)
//! selects a material or tool. Pointer moves only redraw when the brush
//! outline changes. Escape closes.
//!
//! `--demo-pour N` feeds the same code a synthetic left drag sweeping
//! across the upper tank for N seconds, then prints the CPU it used (the
//! T4 acceptance measurement; real input can't be scripted without
//! accessibility permissions).

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use desktank_platform::process_stats;
use desktank_render::{
    BrushPreview, Gpu, GpuOptions, Palette, Tank, TankConfig, TankSurface, wgpu,
};
use desktank_sim::tools::{Button, Inspection, Outcome, PaletteItem, Tools};
use desktank_sim::{MaterialTable, World, WorldConfig};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::scene;

/// Screen pixels per cell (SPEC §2.1 default).
const CELL_SCALE: u32 = 4;
/// Largest grid side: WebGL2's texture limit, kept natively too.
const MAX_GRID_SIDE: u32 = 2048;
/// Pointer sample interval of the demo pour (a 60 Hz mouse).
const DEMO_SAMPLE: Duration = Duration::from_micros(16_667);
/// Trackpad scroll distance (physical pixels) per brush-size step.
const SCROLL_STEP_PX: f64 = 24.0;
/// World seed for the window's starter scene.
const SEED: u64 = 7;

/// Opens the window and runs until it is closed.
///
/// # Errors
///
/// Returns an error if the event loop, window, GPU or world can't be set up.
pub fn run(demo_pour: Option<u64>) -> anyhow::Result<()> {
    let event_loop = EventLoop::new().context("creating the event loop")?;
    let mut app = App {
        instance: wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
            Box::new(event_loop.owned_display_handle()),
        )),
        origin: Instant::now(),
        state: None,
        error: None,
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
    state: Option<Running>,
    error: Option<anyhow::Error>,
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
            s.cursor = Some((f64::from(width) * 0.15, f64::from(height) * 0.3));
            s.pressed(Button::Primary);
            return Some(now + DEMO_SAMPLE);
        };
        let t = now - t0;
        if t >= self.length {
            s.tools.release(Button::Primary);
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
        s.cursor = Some((
            f64::from(width) * f.mul_add(0.7, 0.15),
            f64::from(height) * 0.3,
        ));
        s.pointer_moved();
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
    /// Visibility as of the last `about_to_wait`.
    was_visible: bool,
    /// Pointer position in physical pixels.
    cursor: Option<(f64, f64)>,
    tools: Tools,
    palette: Vec<PaletteItem>,
    /// Trackpad scroll (pixels) not yet turned into a brush-size step.
    scroll: f64,
    /// Wheel scroll (lines) not yet turned into a step; Windows precision
    /// touchpads send fractions of a line.
    scroll_lines: f64,
    /// Palette selection last given to the tank (`usize::MAX` before the
    /// first).
    shown_selection: Option<usize>,
}

impl Running {
    fn new(instance: &wgpu::Instance, event_loop: &ActiveEventLoop) -> anyhow::Result<Self> {
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
        // The grid covers the window as first opened; later resizes rescale.
        let cells = |px: u32| (px / CELL_SCALE).clamp(16, MAX_GRID_SIDE) as usize;
        let config = WorldConfig {
            width: cells(size.width),
            height: cells(size.height),
            seed: SEED,
            ..WorldConfig::default()
        };
        let mut world = World::new(config, MaterialTable::builtin()?)?;
        scene::starter(&mut world, SEED)?;
        let tools = Tools::new(scene::material(&world, "sand")?);
        let palette = Tools::palette(&world);
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
            was_visible: true,
            cursor: None,
            tools,
            palette,
            scroll: 0.0,
            scroll_lines: 0.0,
            shown_selection: Some(usize::MAX),
        };
        s.sync_overlay();
        Ok(s)
    }

    fn visible(&self) -> bool {
        let h = self.hidden;
        !h.occluded && !h.zero_size && !h.minimised
    }

    fn resized(&mut self, size: PhysicalSize<u32>) {
        self.hidden.zero_size = size.width == 0 || size.height == 0;
        if self.hidden.zero_size {
            return;
        }
        self.surface.resize(&self.gpu, size.width, size.height);
        self.tank.invalidate();
        if self.visible() {
            self.window.request_redraw();
        }
    }

    /// The cell under the cursor, unless the cursor is over the palette.
    fn cursor_cell(&self) -> Option<(i64, i64)> {
        let (px, py) = self.cursor?;
        let size = self.surface.size();
        if self
            .tank
            .palette_layout(size)
            .is_some_and(|p| p.contains(px, py))
        {
            return None;
        }
        Some(self.tank.layout(size).cell_at(px, py))
    }

    fn pointer_moved(&mut self) {
        match self.cursor_cell() {
            Some(cell) => {
                let out = self.tools.moved(self.tank.world_mut(), cell);
                self.show(out);
            }
            // Over the palette (or gone): hide the outline; a stroke ends.
            None => self.tools.leave(),
        }
    }

    fn pressed(&mut self, button: Button) {
        let size = self.surface.size();
        if let (Some((px, py)), Some(p)) = (self.cursor, self.tank.palette_layout(size)) {
            if let Some(&item) = p.hit(px, py).and_then(|i| self.palette.get(i)) {
                self.tools.select(item);
                return;
            }
            if p.contains(px, py) {
                return;
            }
        }
        if let Some(cell) = self.cursor_cell() {
            let out = self.tools.press(self.tank.world_mut(), button, cell);
            self.show(out);
        }
    }

    /// Reports an inspection in the title bar (the creature card is T7).
    fn show(&self, out: Outcome) {
        let Outcome::Inspected(found) = out else {
            return;
        };
        let text = match found {
            Inspection::Cell { x, y, material } => {
                let name = self
                    .tank
                    .world()
                    .materials()
                    .get(material)
                    .map_or("?", |m| m.name.as_str());
                format!("{name} at ({x}, {y})")
            }
            Inspection::Creature(id) => format!("creature {}", id.0),
        };
        self.window.set_title(&format!("DeskTank — {text}"));
    }

    /// Scroll up grows the brush, down shrinks it.
    fn scrolled(&mut self, delta: MouseScrollDelta) {
        let steps = match delta {
            MouseScrollDelta::LineDelta(_, y) => {
                self.scroll_lines += f64::from(y);
                let steps = self.scroll_lines.trunc();
                self.scroll_lines -= steps;
                steps
            }
            MouseScrollDelta::PixelDelta(p) => {
                self.scroll += p.y;
                let steps = (self.scroll / SCROLL_STEP_PX).trunc();
                self.scroll -= steps * SCROLL_STEP_PX;
                steps
            }
        };
        #[allow(clippy::cast_possible_truncation)] // clamped to a small range
        self.tools.resize(steps.clamp(-100.0, 100.0) as i32);
    }

    /// Pushes the palette selection and brush outline to the tank; it only
    /// asks for a frame if they changed.
    fn sync_overlay(&mut self) {
        let selected = self.palette.iter().position(|&i| self.tools.is_selected(i));
        if self.shown_selection != selected {
            self.shown_selection = selected;
            self.tank.set_palette(Some(Palette {
                items: self.palette.clone(),
                selected,
            }));
            // A new tool clears the last inspection from the title.
            self.window.set_title("DeskTank");
        }
        let cursor = self.tools.preview().map(|(x, y, radius)| BrushPreview {
            cell: (x, y),
            radius,
        });
        self.tank.set_cursor(cursor);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match Running::new(&self.instance, event_loop) {
            Ok(s) => self.state = Some(s),
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(s) = self.state.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => s.resized(size),
            WindowEvent::Occluded(occluded) => {
                s.hidden.occluded = occluded;
                if s.visible() {
                    s.tank.invalidate();
                    s.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if s.visible()
                    && let Err(e) = s.tank.draw(&s.gpu, &s.surface)
                {
                    self.error = Some(e.into());
                    event_loop.exit();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                s.cursor = Some((position.x, position.y));
                s.pointer_moved();
            }
            WindowEvent::CursorLeft { .. } => {
                s.cursor = None;
                s.tools.leave();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    MouseButton::Left => Button::Primary,
                    MouseButton::Right => Button::Secondary,
                    _ => return,
                };
                if state == ElementState::Pressed {
                    s.pressed(button);
                } else {
                    s.tools.release(button);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => s.scrolled(delta),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Character(c) => {
                        let slot = c.parse::<usize>().ok().and_then(|n| n.checked_sub(1));
                        if let Some(&item) = slot.and_then(|i| s.palette.get(i)) {
                            s.tools.select(item);
                        }
                    }
                    _ => {}
                }
            }
            _ => return,
        }
        s.sync_overlay();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(s) = self.state.as_mut() else { return };
        // Not every platform sends an event for minimising, so ask.
        s.hidden.minimised = s.window.is_minimized() == Some(true);
        let visible = s.visible();
        if visible != s.was_visible {
            eprintln!(
                "desktank window: {}",
                if visible { "visible" } else { "hidden, paused" }
            );
            s.was_visible = visible;
            s.tank.invalidate();
        }
        if !s.visible() {
            // Hidden: world time stops and nothing wakes us but OS events.
            s.tank.pause();
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        let demo_wake = self.demo.as_mut().and_then(|d| {
            let wake = d.step(s);
            s.sync_overlay();
            wake
        });
        if demo_wake.is_none() {
            self.demo = None;
        }
        let req = s.tank.tick(self.origin.elapsed());
        // Holding still keeps pouring, once per tick run.
        for _ in 0..req.sim_ticks {
            s.tools.hold(s.tank.world_mut());
        }
        if req.redraw {
            s.window.request_redraw();
        }
        let wake = self.origin + req.wake_at;
        let wake = demo_wake.map_or(wake, |d| d.min(wake));
        event_loop.set_control_flow(ControlFlow::WaitUntil(wake));
    }
}
