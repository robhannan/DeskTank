//! `desktank window`: the tank in a normal resizable winit window.
//!
//! Pacing (SPEC §4.2, §6.2): no continuous redraw. The loop sleeps with
//! `ControlFlow::WaitUntil` until the tank's next deadline (a 20 Hz
//! simulation tick, frames capped at 30 fps and drawn only when the world
//! changed). While the window is occluded or minimised the tank is paused
//! and the loop waits on OS events only, with no timers.
//!
//! Interaction (SPEC §2.5) uses the platform-free `desktank_sim::tools`:
//! left drag uses the selected tool (pour, scoop, net, inspect, glue), right
//! drag scoops, the scroll wheel or a two-finger trackpad scroll sets the
//! brush size. The palette in the top-left corner is grouped: its top row
//! picks a group (click a tab, scroll over the palette, or `G` / `Shift+G`
//! for the next or previous group) and the row below lists that group's
//! materials, devices, tools or creatures; click one (or press 1–9, then 0
//! for the tenth) to select it. A device
//! (bubbler, filter outlet) is placed once per click. `F` cycles a debug
//! tint of the nutrient, CO₂ and light fields; `W` changes the water and
//! `Q` shows the water quality in the title bar. `--time HH:MM` lights the
//! tank for that time of day (default noon). Pointer moves only redraw
//! when the brush outline changes. Escape closes.
//!
//! Plants (T6b): the palette's plants slot pours the species named in the
//! title bar as the propagule named there (seeds, cuttings, fragments or runners);
//! clicking it again (or `P`) picks the next species, `O` the next
//! propagule that species grows from. Inspect shows a plant's species, age, health, size and
//! what limits its growth in the title bar.
//!
//! Creatures (T7a): the palette's creature slots add one per click; the net
//! carries a creature and, let go above the water, lifts it out; inspect
//! names the species in the title bar. `--fish N` stocks the tank with N
//! creatures to start with. Swimming creatures redraw the tank every tick
//! (20 fps, under the cap) even when every chunk sleeps.
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
//! `--no-save` neither loads nor saves.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use desktank_platform::process_stats;
use desktank_render::{
    BrushPreview, Gpu, GpuOptions, Palette, PaletteHit, Tank, TankConfig, TankSurface, wgpu,
};
use desktank_sim::UnmetNeed;
use desktank_sim::tools::{Button, Inspection, Outcome, PaletteGroup, PaletteItem, Tool, Tools};
use desktank_sim::{FieldKind, MaterialTable, World, WorldConfig};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::persist::Persistence;
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

/// Life-cycle settings given on the command line (SPEC §2.4, T9); `None`
/// keeps what the tank has (a new tank: easy mode off, a cap of 60).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LifeSettings {
    /// Easy mode: only old age kills.
    pub easy: Option<bool>,
    /// The population cap.
    pub cap: Option<usize>,
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
    life: LifeSettings,
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
    let event_loop = EventLoop::new().context("creating the event loop")?;
    let mut app = App {
        instance: wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
            Box::new(event_loop.owned_display_handle()),
        )),
        origin: Instant::now(),
        time,
        fish,
        life,
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
    /// Life-cycle settings to apply to the tank.
    life: LifeSettings,
    state: Option<Running>,
    error: Option<anyhow::Error>,
    /// The saved tank, unless saving is off.
    persist: Option<Persistence>,
    /// The tank loaded from the last save, until the window opens.
    saved: Option<World>,
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
            s.tools.release(s.tank.world_mut(), Button::Primary);
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

/// What the palette overlay last showed: the open group, the selection
/// (group, item) and the plant species the plants slot pours.
type Shown = (
    usize,
    Option<(usize, usize)>,
    Option<desktank_sim::PlantSpeciesId>,
);

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
    avoid_cursor: bool,
    tools: Tools,
    /// The palette's groups and their items.
    groups: Vec<(PaletteGroup, Vec<PaletteItem>)>,
    /// Index of the group the palette shows.
    open: usize,
    /// Whether Shift is held.
    shift: bool,
    /// Trackpad scroll (pixels) not yet turned into a brush-size step.
    scroll: f64,
    /// Wheel scroll (lines) not yet turned into a step; Windows precision
    /// touchpads send fractions of a line.
    scroll_lines: f64,
    /// Open group, selection and plant species last given to the tank
    /// (`None` before the first).
    shown: Option<Shown>,
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
        let tools = Tools::new(scene::material(&world, "sand")?);
        let groups = Tools::palette_groups(&world);
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
            avoid_cursor: false,
            tools,
            groups,
            open: 0,
            shift: false,
            scroll: 0.0,
            scroll_lines: 0.0,
            shown: None,
        };
        // Open the group holding the starting material.
        s.open = s.selection().map_or(0, |(g, _)| g);
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
            None => self.tools.leave(self.tank.world_mut()),
        }
    }

    fn pressed(&mut self, button: Button) {
        let size = self.surface.size();
        if let (Some((px, py)), Some(p)) = (self.cursor, self.tank.palette_layout(size)) {
            match p.hit(px, py) {
                Some(PaletteHit::Tab(g)) => {
                    self.open = g;
                    return;
                }
                Some(PaletteHit::Item(i)) => {
                    self.select_slot(i);
                    return;
                }
                None if p.contains(px, py) => return,
                None => {}
            }
        }
        if let Some(cell) = self.cursor_cell() {
            let out = self.tools.press(self.tank.world_mut(), button, cell);
            self.show(out);
        }
    }

    /// Selects a palette item; choosing the plants slot again picks the
    /// next plant species. The plants slot names its species in the title.
    fn choose(&mut self, item: PaletteItem) {
        let plants = PaletteItem::Tool(Tool::Plant);
        if item == plants && self.tools.is_selected(plants) {
            self.tools.next_plant_species(self.tank.world());
        }
        self.tools.select(item);
        if item == plants {
            self.show_plant_species();
        }
    }

    /// Names the species the plants slot pours, and as what, in the title
    /// bar.
    fn show_plant_species(&mut self) {
        let world = self.tank.world();
        if let Some(s) = self
            .tools
            .plant_species(world)
            .and_then(|id| world.plant_species().get(id))
        {
            let way = self
                .tools
                .plant_propagule(world)
                .map_or("", desktank_sim::Propagule::name);
            let title = format!("DeskTank — plants: {} as {way} ({})", s.name, s.description);
            self.sync_overlay();
            self.window.set_title(&title);
        }
    }

    /// Reports an inspection in the title bar (a drawn card comes later).
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
            Inspection::Creature {
                id,
                species,
                x,
                y,
                material,
            } => {
                let world = self.tank.world();
                let name = world
                    .species()
                    .get(species)
                    .map_or("?", |s| s.name.as_str());
                let over = world
                    .materials()
                    .get(material)
                    .map_or("?", |m| m.name.as_str());
                let need = match world.unmet_need(species) {
                    Some(UnmetNeed::Depth { needs, .. }) => {
                        format!(", needs {needs} cells of water")
                    }
                    Some(UnmetNeed::Air) => ", needs air at the surface".to_owned(),
                    None => String::new(),
                };
                format!("{name} #{}{need} over {over} at ({x}, {y})", id.0)
            }
            Inspection::Plant { id, x, y, .. } => {
                let world = self.tank.world();
                let Some(p) = world.plant(id) else { return };
                let name = world
                    .plant_species()
                    .get(p.species)
                    .map_or("?", |s| s.name.as_str());
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // ages ≥ 0
                let age = p.age as u64;
                let [roots, stem, leaves] = p.parts;
                format!(
                    "{name} #{}, {}m {:02}s old, health {:.0}%, {} cells \
                     (roots {roots}, stem {stem}, leaves {leaves}), {} at ({x}, {y})",
                    id.0,
                    age / 60,
                    age % 60,
                    p.health * 100.0,
                    p.size(),
                    p.limit.describe(),
                )
            }
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
        let steps = steps.clamp(-100.0, 100.0) as i32;
        let over_palette = self
            .cursor
            .zip(self.tank.palette_layout(self.surface.size()));
        if over_palette.is_some_and(|((px, py), p)| p.contains(px, py)) {
            // Over the palette: scroll down for the next group.
            self.step_group(-steps);
        } else {
            self.tools.resize(steps);
        }
    }

    /// Opens the group `steps` after (or before) the open one, wrapping.
    fn step_group(&mut self, steps: i32) {
        let n = i64::try_from(self.groups.len()).unwrap_or(1).max(1);
        let open = i64::try_from(self.open).unwrap_or(0);
        self.open = usize::try_from((open + i64::from(steps)).rem_euclid(n)).unwrap_or(0);
    }

    /// Selects slot `i` of the open group, if it has one.
    fn select_slot(&mut self, i: usize) {
        if let Some(&item) = self
            .groups
            .get(self.open)
            .and_then(|(_, items)| items.get(i))
        {
            self.choose(item);
        }
    }

    /// Picks the next plant species and selects the plants slot, opening
    /// its group.
    fn next_plant_species(&mut self) {
        self.tools.next_plant_species(self.tank.world());
        self.select_plants_slot();
    }

    /// Picks the next propagule the plant species grows from (seeds,
    /// cuttings, fragments, runners) and selects the plants slot, opening its group.
    fn next_plant_propagule(&mut self) {
        self.tools.next_plant_propagule(self.tank.world());
        self.select_plants_slot();
    }

    /// Selects the plants slot, opening its group, and names its species.
    fn select_plants_slot(&mut self) {
        let slot = PaletteItem::Tool(Tool::Plant);
        if let Some(g) = self
            .groups
            .iter()
            .position(|(_, items)| items.contains(&slot))
        {
            self.open = g;
        }
        self.tools.select(slot);
        self.show_plant_species();
    }

    /// The selection's group and its index in that group.
    fn selection(&self) -> Option<(usize, usize)> {
        self.groups.iter().enumerate().find_map(|(g, (_, items))| {
            let i = items.iter().position(|&i| self.tools.is_selected(i))?;
            Some((g, i))
        })
    }

    /// Pushes the palette (open group and selection) and brush outline to
    /// the tank; it only asks for a frame if they changed.
    fn sync_overlay(&mut self) {
        let selected = self.selection();
        let plant = self.tools.plant_species(self.tank.world());
        let shown = (self.open, selected, plant);
        if self.shown != Some(shown) {
            if self.shown.is_none_or(|(_, s, _)| s != selected) {
                // A new tool clears the last inspection from the title.
                self.window.set_title("DeskTank");
            }
            self.shown = Some(shown);
            self.tank.set_palette(Some(Palette {
                tabs: self
                    .groups
                    .iter()
                    .filter_map(|(_, items)| items.first().copied())
                    .collect(),
                open: self.open,
                items: self
                    .groups
                    .get(self.open)
                    .map_or_else(Vec::new, |(_, items)| items.clone()),
                selected,
                plant,
            }));
        }
        let pointer = self.tools.pointer().filter(|_| self.avoid_cursor);
        self.tank.world_mut().set_creature_cursor(pointer);
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
        let saved = self.saved.take();
        let fresh = saved.is_none();
        match Running::new(&self.instance, event_loop, saved) {
            Ok(mut s) => {
                if let Some(minute) = self.time {
                    s.tank.set_time_of_day(minute);
                }
                let world = s.tank.world_mut();
                if let Some(easy) = self.life.easy {
                    world.set_easy_mode(easy);
                }
                if let Some(cap) = self.life.cap {
                    world.set_creature_cap(cap);
                }
                // A loaded tank keeps the creatures it had.
                let fish = if fresh { self.fish } else { 0 };
                if let Err(e) = scene::stock(s.tank.world_mut(), fish, SEED) {
                    self.error = Some(e);
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
                s.tools.leave(s.tank.world_mut());
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
                    let out = s.tools.release(s.tank.world_mut(), button);
                    s.show(out);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => s.scrolled(delta),
            WindowEvent::ModifiersChanged(m) => s.shift = m.state().shift_key(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Character(c) if c.eq_ignore_ascii_case("a") && !event.repeat => {
                        s.avoid_cursor = !s.avoid_cursor;
                        let state = if s.avoid_cursor { "on" } else { "off" };
                        s.window
                            .set_title(&format!("DeskTank — cursor avoidance {state}"));
                    }
                    Key::Character(c) if c.eq_ignore_ascii_case("f") => {
                        let next = match s.tank.field_overlay() {
                            None => Some(FieldKind::Nutrient),
                            Some(FieldKind::Nutrient) => Some(FieldKind::Co2),
                            Some(FieldKind::Co2) => Some(FieldKind::Light),
                            Some(FieldKind::Light) => None,
                        };
                        s.tank.set_field_overlay(next);
                        let title = next.map_or("DeskTank".to_owned(), |f| {
                            format!("DeskTank — {} field", f.name())
                        });
                        s.window.set_title(&title);
                    }
                    Key::Character(c) if c.eq_ignore_ascii_case("w") => {
                        let siphoned = s.tank.world_mut().water_change();
                        s.tank.cells_edited();
                        let q = s.tank.world().ecosystem().quality;
                        s.window.set_title(&format!(
                            "DeskTank — water changed, {siphoned} cells siphoned, quality {:.0}%",
                            q * 100.0
                        ));
                    }
                    Key::Character(c) if c.eq_ignore_ascii_case("q") => {
                        let e = s.tank.world().ecosystem();
                        s.window.set_title(&format!(
                            "DeskTank — water quality {:.0}%, nutrients {:.2}, algae {:.1}%",
                            e.quality * 100.0,
                            e.mean_nutrient,
                            e.algae_coverage * 100.0
                        ));
                    }
                    // G opens the next group, Shift+G the previous one
                    // (Shift, not the letter's case: Caps Lock doesn't flip it).
                    Key::Character(c) if c.eq_ignore_ascii_case("g") => {
                        s.step_group(if s.shift { -1 } else { 1 });
                    }
                    Key::Character(c) => {
                        // 1–9 are the open group's first nine slots, 0 the
                        // tenth; P picks the next plant species, O the
                        // next way to start it (seeds, cuttings, fragments, runners).
                        if let Ok(n) = c.parse::<usize>() {
                            s.select_slot((n + 9) % 10);
                        } else if c.eq_ignore_ascii_case("p") {
                            s.next_plant_species();
                        } else if c.eq_ignore_ascii_case("o") {
                            s.next_plant_propagule();
                        }
                    }
                    _ => {}
                }
            }
            _ => return,
        }
        s.sync_overlay();
    }

    /// Saves on the way out. On macOS, quitting from the menu or the Dock
    /// ends the process right after this, without returning from
    /// `run_app`.
    fn exiting(&mut self, _: &ActiveEventLoop) {
        if let (Some(p), Some(s)) = (&mut self.persist, &self.state) {
            p.save(s.tank.world(), "saved on exit");
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
            if !visible && let Some(p) = &mut self.persist {
                // Paused from here on, so nothing changes until it shows
                // again.
                p.save(s.tank.world(), "saved while hidden");
            }
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
        if let Some(p) = &mut self.persist {
            p.autosave(s.tank.world(), Instant::now());
        }
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
