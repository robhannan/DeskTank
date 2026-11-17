//! `desktank window`: the tank in a normal resizable winit window.
//!
//! Pacing (SPEC §4.2, §6.2): no continuous redraw. The loop sleeps with
//! `ControlFlow::WaitUntil` until the tank's next deadline (a 20 Hz
//! simulation tick, frames capped at 30 fps and drawn only when the world
//! changed). While the window is occluded or minimised the tank is paused
//! and the loop waits on OS events only, with no timers.
//!
//! Debug input: left drag pours the selected material (keys 1–5: sand,
//! gravel, rock, bubbles, flakes), right drag erases. Escape closes.

use std::sync::Arc;
use std::time::Instant;

use anyhow::Context as _;
use desktank_render::{Gpu, GpuOptions, Layout, Tank, TankConfig, TankSurface, wgpu};
use desktank_sim::{Brush, MaterialId, MaterialTable, World, WorldConfig};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::scene;

/// Screen pixels per cell (SPEC §2.1 default).
const CELL_SCALE: u32 = 4;
/// Largest grid side: WebGL2's texture limit, kept natively too.
const MAX_GRID_SIDE: u32 = 2048;
/// Brush radius in cells.
const BRUSH_RADIUS: u32 = 3;
/// Materials on keys 1–5.
const PALETTE: [&str; 5] = ["sand", "gravel", "rock", "air_bubble", "flake_food"];
/// World seed for the window's starter scene.
const SEED: u64 = 7;

/// Opens the window and runs until it is closed.
///
/// # Errors
///
/// Returns an error if the event loop, window, GPU or world can't be set up.
pub fn run() -> anyhow::Result<()> {
    let event_loop = EventLoop::new().context("creating the event loop")?;
    let mut app = App {
        instance: wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
            Box::new(event_loop.owned_display_handle()),
        )),
        origin: Instant::now(),
        state: None,
        error: None,
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

/// The debug brush while a mouse button is held.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stroke {
    Pour,
    Erase,
}

struct Running {
    window: Arc<Window>,
    gpu: Gpu,
    surface: TankSurface,
    tank: Tank,
    hidden: Hidden,
    /// Visibility as of the last `about_to_wait`.
    was_visible: bool,
    cursor: Option<(f64, f64)>,
    stroke: Option<Stroke>,
    material: MaterialId,
    water: MaterialId,
    air: MaterialId,
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
        let [material, water, air] = ["sand", "water", "air"].map(|n| scene::material(&world, n));
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
        Ok(Self {
            window,
            gpu,
            surface,
            tank: Tank::new(world, TankConfig::default()),
            hidden: Hidden::default(),
            was_visible: true,
            cursor: None,
            stroke: None,
            material: material?,
            water: water?,
            air: air?,
        })
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

    /// Applies the debug brush under the cursor.
    fn brush(&mut self) {
        let (Some((px, py)), Some(stroke)) = (self.cursor, self.stroke) else {
            return;
        };
        let world = self.tank.world();
        let grid = (
            u32::try_from(world.width()).unwrap_or(u32::MAX),
            u32::try_from(world.height()).unwrap_or(u32::MAX),
        );
        let (cx, cy) = Layout::fit(grid, self.surface.size()).cell_at(px, py);
        let (material, brush) = if stroke == Stroke::Pour {
            (self.material, Brush::Pour)
        } else if usize::try_from(cy).unwrap_or(0) < world.surface_row() {
            (self.air, Brush::Replace)
        } else {
            (self.water, Brush::Replace)
        };
        // Brushes clip to the grid and the materials are built in, so this
        // can't fail; ignore the count.
        let _ = self
            .tank
            .world_mut()
            .paint(cx, cy, BRUSH_RADIUS, material, brush);
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
            }
            WindowEvent::CursorLeft { .. } => s.cursor = None,
            WindowEvent::MouseInput { state, button, .. } => {
                let stroke = match button {
                    MouseButton::Left => Stroke::Pour,
                    MouseButton::Right => Stroke::Erase,
                    _ => return,
                };
                if state == ElementState::Pressed {
                    s.stroke = Some(stroke);
                } else if s.stroke == Some(stroke) {
                    s.stroke = None;
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Character(c) => {
                        let slot = c.parse::<usize>().ok().and_then(|n| n.checked_sub(1));
                        if let Some(name) = slot.and_then(|i| PALETTE.get(i))
                            && let Ok(id) = scene::material(s.tank.world(), name)
                        {
                            s.material = id;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
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
        s.brush();
        let req = s.tank.tick(self.origin.elapsed());
        if req.redraw {
            s.window.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.origin + req.wake_at));
    }
}
