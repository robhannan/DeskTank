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
//! `--no-save` neither loads nor saves.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use desktank_platform::{PowerDecision, PowerMonitor, PowerPolicy, PowerSource, process_stats};
use desktank_render::{
    BrushPreview, Gpu, GpuOptions, Magnifier, Palette, PaletteHit, Tank, TankConfig, TankSurface,
    wgpu,
};
use desktank_sim::UnmetNeed;
use desktank_sim::cheats::{self, CheatKey, Konami};
use desktank_sim::tools::{
    AddRefused, Button, Inspection, Outcome, PaletteItem, PaletteSub, PaletteTab, Tool, Tools,
};
use desktank_sim::{
    CreatureId, DeviceId, FieldKind, MAX_NAME_CHARS, MaterialTable, Mount, RinseKind, World,
    WorldConfig,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use crate::config::{self, CliOverrides, Config, Life};
use crate::persist::Persistence;
use desktank_sim::scene;

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

enum Wake {
    Power,
    Config,
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

/// What the palette overlay last showed: the open tab and sub-category and
/// the selection (tab, sub-category, item).
type Shown = (usize, usize, Option<(usize, usize, usize)>);

struct Running {
    window: Arc<Window>,
    gpu: Gpu,
    surface: TankSurface,
    tank: Tank,
    hidden: Hidden,
    /// Power/visibility decision as of the last `about_to_wait`.
    last_decision: PowerDecision,
    /// Pointer position in physical pixels.
    cursor: Option<(f64, f64)>,
    avoid_cursor: bool,
    tools: Tools,
    /// The palette's tree: top tabs, their sub-categories and items.
    tree: Vec<PaletteTab>,
    /// Index of the top tab the palette shows.
    open: usize,
    /// Index of the sub-category it shows within that tab.
    open_sub: usize,
    /// Modifier keys held (Shift steps tabs backwards; with Ctrl or
    /// Command a key is a shortcut, which neither types into the cheat
    /// field nor counts towards the code).
    modifiers: ModifiersState,
    /// Trackpad scroll (pixels) not yet turned into a brush-size step.
    scroll: f64,
    /// Wheel scroll (lines) not yet turned into a step; Windows precision
    /// touchpads send fractions of a line.
    scroll_lines: f64,
    /// Open tab and sub-category and the selection last given to the tank
    /// (`None` before the first).
    shown: Option<Shown>,
    /// Watches key presses for the code that opens the cheat field.
    konami: Konami,
    /// The text field open in the title bar (cheat or name), if any.
    field: Option<TextField>,
    /// The creature inspect last showed, which `N` names; cleared when
    /// another tool is picked.
    inspected: Option<CreatureId>,
    /// The device inspect last showed (`id`, cell `x`, `y`), which `R` rinses
    /// and `I` toggles the intake guard on.
    inspected_device: Option<(DeviceId, usize, usize)>,
}

/// A one-line text field in the title bar.
struct TextField {
    purpose: Purpose,
    line: String,
}

/// What a [`TextField`] is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Purpose {
    /// A cheat ([`desktank_sim::cheats`]).
    Cheat,
    /// A name for this creature.
    Name(CreatureId),
}

impl Purpose {
    /// Longest line it takes, in characters.
    fn max_chars(self) -> usize {
        match self {
            Self::Cheat => cheats::MAX_LINE,
            Self::Name(_) => MAX_NAME_CHARS,
        }
    }
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
        let tree = Tools::palette_tree(&world);
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
            cursor: None,
            avoid_cursor: false,
            tools,
            tree,
            open: 0,
            open_sub: 0,
            modifiers: ModifiersState::empty(),
            scroll: 0.0,
            scroll_lines: 0.0,
            shown: None,
            konami: Konami::new(),
            field: None,
            inspected: None,
            inspected_device: None,
        };
        // Open the tab and sub-category holding the starting material.
        (s.open, s.open_sub) = s.selection().map_or((0, 0), |(t, sub, _)| (t, sub));
        s.sync_overlay();
        Ok(s)
    }

    fn surface_visible(&self) -> bool {
        let h = self.hidden;
        !h.occluded && !h.zero_size && !h.minimised
    }

    fn decision(&self, policy: &PowerPolicy, power: &PowerMonitor) -> PowerDecision {
        policy.evaluate(power.signals(), !self.surface_visible())
    }

    fn resized(&mut self, size: PhysicalSize<u32>) {
        self.hidden.zero_size = size.width == 0 || size.height == 0;
        if self.hidden.zero_size {
            return;
        }
        self.surface.resize(&self.gpu, size.width, size.height);
        self.tank.invalidate();
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
                Some(PaletteHit::Tab(t)) => {
                    self.open_tab(t);
                    return;
                }
                Some(PaletteHit::Sub(sub)) => {
                    self.open_sub = sub;
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

    /// Selects a palette item. A plant slot names its species and
    /// propagule in the title.
    fn choose(&mut self, item: PaletteItem) {
        self.tools.select(item);
        if matches!(item, PaletteItem::Tool(Tool::Plant(_))) {
            self.show_plant();
        } else if item == PaletteItem::Tool(Tool::Magnify) {
            self.show_magnify();
        }
    }

    /// Shows the active magnification factor in the title bar.
    fn show_magnify(&mut self) {
        let zoom = self.tools.zoom();
        self.sync_overlay();
        self.window.set_title(&format!(
            "DeskTank — magnify {zoom}× (scroll, click or +/- to zoom)"
        ));
    }

    /// Names the species the selected plant slot pours, and as what, in
    /// the title bar.
    fn show_plant(&mut self) {
        let world = self.tank.world();
        let Some(s) = self
            .tools
            .selected_plant()
            .and_then(|id| world.plant_species().get(id))
        else {
            return;
        };
        let way = self
            .tools
            .plant_propagule(world)
            .map_or("", desktank_sim::Propagule::name);
        let ways: Vec<_> = s.propagules().map(desktank_sim::Propagule::name).collect();
        let more = if ways.len() > 1 {
            format!(", P for {}", ways.join("/"))
        } else {
            String::new()
        };
        let title = format!(
            "DeskTank — {} as {way} ({}{more})",
            s.name.replace('_', " "),
            s.description
        );
        self.sync_overlay();
        self.window.set_title(&title);
    }

    /// Reports an inspection or refused creature add in the title bar (the
    /// window's card; a drawn one comes with the overlay, which has no title
    /// bar).
    fn show(&mut self, out: Outcome) {
        if let Outcome::Zoomed(_) = out {
            self.show_magnify();
            return;
        }
        if let Outcome::AddRefused { species, reason } = out {
            self.forget_inspected();
            let world = self.tank.world();
            let name = world
                .species()
                .get(species)
                .map_or_else(|| "creature".to_owned(), |s| s.name.replace('_', " "));
            let text = match reason {
                AddRefused::PopulationCap {
                    cap,
                    live_food: true,
                } => format!("can't add {name}: live food limit reached ({cap} max)"),
                AddRefused::PopulationCap {
                    cap,
                    live_food: false,
                } => format!(
                    "can't add {name}: tank is full ({}/{cap} creatures) — raise --creature-cap or net one out",
                    world.stock_count(),
                ),
                AddRefused::NoRoom => format!("can't add {name} here: no room (click open water)"),
            };
            self.window.set_title(&format!("DeskTank — {text}"));
            return;
        }
        let Outcome::Inspected(found) = out else {
            return;
        };
        // A new inspection replaces the last: N names only a creature
        // shown now, and an open name field is for the one shown before.
        self.forget_inspected();
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
            Inspection::Device { id, x, y } => {
                let Some(line) = device_line(self.tank.world(), id, x, y) else {
                    return;
                };
                self.inspected_device = Some((id, x, y));
                line
            }
            Inspection::Creature {
                id,
                species,
                x,
                y,
                material,
            } => {
                let world = self.tank.world();
                let Some(card) = world.creature_card(id) else {
                    return;
                };
                self.inspected = Some(id);
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
                format!(
                    "{}{need} · over {over} at ({x}, {y}) · N names it",
                    card.summary()
                )
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

    /// Shows the open text field in the title bar, after `note` (the last
    /// cheat's error) if any.
    fn show_field(&self, note: Option<&str>) {
        let Some(field) = &self.field else { return };
        let note = note.map_or(String::new(), |n| format!("{n} · "));
        let line = &field.line;
        let title = match field.purpose {
            Purpose::Cheat => {
                format!("DeskTank — {note}cheat: {line}_ (Enter runs, Esc closes)")
            }
            Purpose::Name(id) => {
                let world = self.tank.world();
                let species = world
                    .creature(id)
                    .and_then(|c| world.species().get(c.species()))
                    .map_or("?".to_owned(), |s| s.name.replace('_', " "));
                format!(
                    "DeskTank — {note}name {species} #{}: {line}_ (Enter names it, Esc cancels)",
                    id.0
                )
            }
        };
        self.window.set_title(&title);
    }

    /// Gives a key press to the text fields first: an open field takes
    /// every key, and the press completing the Konami code opens the cheat
    /// field instead of doing its usual binding (A). Returns whether it
    /// took it.
    fn field_input(&mut self, event: &KeyEvent) -> bool {
        if self.field.is_some() {
            self.field_key(event);
            return true;
        }
        if let Some(key) = konami_key(&event.logical_key)
            && !event.repeat
            && !self.command()
            && self.konami.key(key, false)
        {
            self.open_field(Purpose::Cheat, String::new());
            return true;
        }
        false
    }

    fn open_field(&mut self, purpose: Purpose, line: String) {
        self.field = Some(TextField { purpose, line });
        self.show_field(None);
    }

    /// Forgets the creature or device inspect last showed, closing its name field.
    fn forget_inspected(&mut self) {
        self.inspected = None;
        self.inspected_device = None;
        if self
            .field
            .as_ref()
            .is_some_and(|f| matches!(f.purpose, Purpose::Name(_)))
        {
            self.field = None;
        }
    }

    /// Rinses the device inspect last showed (`R` gentle, `Shift+R` hard).
    fn rinse_inspected_device(&mut self, kind: RinseKind) {
        let Some((id, x, y)) = self.inspected_device else {
            return;
        };
        self.tank.world_mut().rinse_device(id, kind);
        if let Some(line) = device_line(self.tank.world(), id, x, y) {
            self.window.set_title(&format!("DeskTank — {line}"));
        } else {
            self.inspected_device = None;
            self.window
                .set_title("DeskTank — that device is no longer in the tank");
        }
    }

    /// Toggles the intake guard on the device inspect last showed (`I`).
    fn toggle_inspected_device_guard(&mut self) {
        let Some((id, x, y)) = self.inspected_device else {
            return;
        };
        if self
            .tank
            .world_mut()
            .toggle_device_intake_guard(id)
            .is_some()
            && let Some(line) = device_line(self.tank.world(), id, x, y)
        {
            self.window.set_title(&format!("DeskTank — {line}"));
        } else {
            self.inspected_device = None;
            self.window
                .set_title("DeskTank — that device is no longer in the tank");
        }
    }

    /// Opens the name field for the creature inspect last showed (`N`),
    /// holding its name so far.
    fn open_name_field(&mut self) {
        let world = self.tank.world();
        if let Some(c) = self.inspected.and_then(|id| world.creature(id)) {
            let (id, name) = (c.id(), c.name().unwrap_or("").to_owned());
            self.open_field(Purpose::Name(id), name);
        } else {
            self.inspected = None;
            self.window
                .set_title("DeskTank — inspect a creature, then N names it");
        }
    }

    /// Whether Ctrl or Command is held.
    fn command(&self) -> bool {
        self.modifiers.control_key() || self.modifiers.super_key()
    }

    /// Takes a key press while a text field is open: Enter runs the cheat
    /// (closing the field if it ran) or names the creature, Escape closes
    /// the field, Backspace deletes, and text is typed in.
    fn field_key(&mut self, event: &KeyEvent) {
        let command = self.command();
        let Some(field) = self.field.as_mut() else {
            return;
        };
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.field = None;
                self.window.set_title("DeskTank");
            }
            Key::Named(NamedKey::Enter) => {
                // Kept on failure, to fix a typo.
                let line = field.line.clone();
                match field.purpose {
                    Purpose::Cheat => match cheats::run(self.tank.world_mut(), &line) {
                        Ok(out) => {
                            if out.cells_changed > 0 {
                                self.tank.cells_edited();
                            }
                            self.field = None;
                            self.window
                                .set_title(&format!("DeskTank — cheat: {}", out.message));
                        }
                        Err(e) => self.show_field(Some(&e.to_string())),
                    },
                    Purpose::Name(id) => {
                        self.field = None;
                        let world = self.tank.world_mut();
                        let title = if world.set_creature_name(id, &line).is_some() {
                            let card = world.creature_card(id).map(|c| c.summary());
                            format!("DeskTank — {} · N renames it", card.unwrap_or_default())
                        } else {
                            "DeskTank — that creature has left the tank".to_owned()
                        };
                        self.window.set_title(&title);
                    }
                }
            }
            Key::Named(NamedKey::Backspace) => {
                field.line.pop();
                self.show_field(None);
            }
            _ if command => {}
            _ => {
                let max = field.purpose.max_chars();
                let typed = event.text.as_ref().map_or("", |t| t.as_str());
                for c in typed.chars().filter(|c| !c.is_control()) {
                    if field.line.chars().count() < max {
                        field.line.push(c);
                    }
                }
                self.show_field(None);
            }
        }
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
            // Over the palette: scroll down for the next sub-category.
            self.step_through(-steps);
        } else if self.tools.tool() == Tool::Magnify {
            if self.tools.step_zoom(steps) {
                self.show_magnify();
            }
        } else {
            self.tools.resize(steps);
        }
    }

    /// Opens top tab `t` on the sub-category holding the selection, if it
    /// is in that tab, else on its first.
    fn open_tab(&mut self, t: usize) {
        self.open = t;
        self.open_sub = self
            .selection()
            .filter(|&(st, _, _)| st == t)
            .map_or(0, |(_, sub, _)| sub);
    }

    /// Opens the top tab `steps` after (or before) the open one, wrapping.
    fn step_tab(&mut self, steps: i32) {
        self.open_tab(wrap(self.open, steps, self.tree.len()));
    }

    /// Opens the open tab's sub-category `steps` after (or before) the
    /// open one, wrapping within the tab.
    fn step_sub(&mut self, steps: i32) {
        let n = self.tree.get(self.open).map_or(1, |t| t.subs.len());
        self.open_sub = wrap(self.open_sub, steps, n);
    }

    /// Steps through every sub-category in palette order, `steps` on (or
    /// back), on into the next (or previous) tab and wrapping: what
    /// scrolling over the palette does.
    fn step_through(&mut self, steps: i32) {
        let all: Vec<(usize, usize)> = self
            .tree
            .iter()
            .enumerate()
            .flat_map(|(t, tab)| (0..tab.subs.len()).map(move |sub| (t, sub)))
            .collect();
        let at = all
            .iter()
            .position(|&p| p == (self.open, self.open_sub))
            .unwrap_or(0);
        if let Some(&(t, sub)) = all.get(wrap(at, steps, all.len())) {
            (self.open, self.open_sub) = (t, sub);
        }
    }

    /// The open sub-category's items.
    fn open_items(&self) -> &[PaletteItem] {
        self.tree
            .get(self.open)
            .and_then(|t| t.subs.get(self.open_sub))
            .map_or(&[], |s| &s.items)
    }

    /// Selects slot `i` of the open sub-category, if it has one.
    fn select_slot(&mut self, i: usize) {
        if let Some(&item) = self.open_items().get(i) {
            self.choose(item);
        }
    }

    /// Picks the next propagule the selected plant's species grows from
    /// (seeds, cuttings, fragments, runners, buds) and names it.
    fn next_plant_propagule(&mut self) {
        if self.tools.next_plant_propagule(self.tank.world()).is_some() {
            self.show_plant();
        } else {
            self.window
                .set_title("DeskTank — select a plant, then P picks how it starts");
        }
    }

    /// The selection's tab, sub-category and index there.
    fn selection(&self) -> Option<(usize, usize, usize)> {
        self.tree.iter().enumerate().find_map(|(t, tab)| {
            tab.subs.iter().enumerate().find_map(|(sub, s)| {
                let i = s.items.iter().position(|&i| self.tools.is_selected(i))?;
                Some((t, sub, i))
            })
        })
    }

    /// Pushes the palette (open tabs and selection) and brush outline to
    /// the tank; it only asks for a frame if they changed.
    fn sync_overlay(&mut self) {
        let selected = self.selection();
        let shown = (self.open, self.open_sub, selected);
        if self.shown != Some(shown) {
            if self.shown.is_none_or(|(_, _, s)| s != selected) {
                // A new tool clears the last inspection from the title.
                self.window.set_title("DeskTank");
                self.forget_inspected();
            }
            self.shown = Some(shown);
            let first = |s: &PaletteSub| s.items.first().copied();
            let open = self.tree.get(self.open);
            // A tab with one sub-category has no sub-tab row.
            let subs = open
                .filter(|t| t.subs.len() > 1)
                .map_or_else(Vec::new, |t| t.subs.iter().filter_map(first).collect());
            self.tank.set_palette(Some(Palette {
                tabs: self
                    .tree
                    .iter()
                    .filter_map(|t| t.subs.first().and_then(first))
                    .collect(),
                open: self.open,
                subs,
                open_sub: self.open_sub,
                items: self.open_items().to_vec(),
                selected,
            }));
        }
        let pointer = self.tools.pointer().filter(|_| self.avoid_cursor);
        self.tank.world_mut().set_creature_cursor(pointer);
        let cursor = self.tools.preview().map(|(x, y, radius)| BrushPreview {
            cell: (x, y),
            radius,
        });
        self.tank.set_cursor(cursor);
        let ghost = self.tools.ghost(self.tank.world());
        self.tank.set_device_ghost(ghost);
        let magnifier = self
            .tools
            .magnifier()
            .map(|(x, y, zoom)| Magnifier { cell: (x, y), zoom });
        self.tank.set_magnifier(magnifier);
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

impl ApplicationHandler<Wake> for App {
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
                s.tank.set_sway(self.settings.plants.sway);
                s.tank
                    .set_allow_sway_on_battery(self.settings.plants.sway_on_battery);
                s.tank.set_on_battery(matches!(
                    self.power.signals().source,
                    PowerSource::Battery { .. }
                ));
                apply_life(s.tank.world_mut(), self.settings.life, Life::default());
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
                    s.decision(&self.settings.power, &self.power),
                    PowerDecision::Run { .. }
                ) {
                    s.window.request_redraw();
                }
            }
            WindowEvent::Occluded(occluded) => {
                s.hidden.occluded = occluded;
                if matches!(
                    s.decision(&self.settings.power, &self.power),
                    PowerDecision::Run { .. }
                ) {
                    s.tank.invalidate();
                    s.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if matches!(
                    s.decision(&self.settings.power, &self.power),
                    PowerDecision::Run { .. }
                ) && let Err(e) = s.tank.draw(&s.gpu, &s.surface)
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
            WindowEvent::ModifiersChanged(m) => s.modifiers = m.state(),
            WindowEvent::Focused(false) => s.konami.reset(),
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && s.field_input(&event) => {}
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Character(c) if c.eq_ignore_ascii_case("a") && !event.repeat => {
                        s.avoid_cursor = !s.avoid_cursor;
                        let state = if s.avoid_cursor { "on" } else { "off" };
                        s.window
                            .set_title(&format!("DeskTank — cursor avoidance {state}"));
                    }
                    Key::Character(c) if c.eq_ignore_ascii_case("s") && !event.repeat => {
                        let (label, next) = if s.modifiers.shift_key() {
                            let next = !s.tank.allows_sway_on_battery();
                            s.tank.set_allow_sway_on_battery(next);
                            ("plant sway on battery", next)
                        } else {
                            let next = !s.tank.sway();
                            s.tank.set_sway(next);
                            ("plant sway", next)
                        };
                        let state = if next { "on" } else { "off" };
                        s.window.set_title(&format!("DeskTank — {label} {state}"));
                    }
                    Key::Character(c) if c.eq_ignore_ascii_case("f") => {
                        use desktank_render::FieldOverlay::{Flow, Scalar};
                        let next = match s.tank.field_overlay() {
                            None => Some(Scalar(FieldKind::Nutrient)),
                            Some(Scalar(FieldKind::Nutrient)) => Some(Scalar(FieldKind::Co2)),
                            Some(Scalar(FieldKind::Co2)) => Some(Scalar(FieldKind::Light)),
                            Some(Scalar(FieldKind::Light)) => Some(Flow),
                            Some(Flow) => None,
                        };
                        s.tank.set_field_overlay(next);
                        let title = next.map_or("DeskTank".to_owned(), |f| {
                            format!("DeskTank — {} field", f.name())
                        });
                        s.window.set_title(&title);
                    }
                    // X mirrors the next flippable device placed onto the other
                    // glass (D27); the ghost shows where it goes.
                    Key::Character(c)
                        if c.eq_ignore_ascii_case("x")
                            && !event.repeat
                            && !s.command()
                            && !s.modifiers.alt_key()
                            && s.tools.can_flip(s.tank.world()) =>
                    {
                        let side = if s.tools.flip() { "other" } else { "nearer" };
                        s.window
                            .set_title(&format!("DeskTank — devices go on the {side} glass"));
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
                    // G opens the next tab, Shift+G the previous one, and H
                    // / Shift+H the open tab's sub-categories (Shift, not
                    // the letter's case: Caps Lock doesn't flip it).
                    Key::Character(c) if c.eq_ignore_ascii_case("g") => {
                        s.step_tab(if s.modifiers.shift_key() { -1 } else { 1 });
                    }
                    Key::Character(c) if c.eq_ignore_ascii_case("h") => {
                        s.step_sub(if s.modifiers.shift_key() { -1 } else { 1 });
                    }
                    // N names the creature inspect last showed.
                    Key::Character(c)
                        if c.eq_ignore_ascii_case("n") && !event.repeat && !s.command() =>
                    {
                        s.open_name_field();
                    }
                    // R rinses the inspected filter (Shift+R hard rinse),
                    // M replaces its media, and I toggles its intake guard
                    // (D27, DV3).
                    Key::Character(c)
                        if c.eq_ignore_ascii_case("r") && !event.repeat && !s.command() =>
                    {
                        let kind = if s.modifiers.shift_key() {
                            RinseKind::Hard
                        } else {
                            RinseKind::Gentle
                        };
                        s.rinse_inspected_device(kind);
                    }
                    Key::Character(c)
                        if c.eq_ignore_ascii_case("m") && !event.repeat && !s.command() =>
                    {
                        s.rinse_inspected_device(RinseKind::Replace);
                    }
                    Key::Character(c)
                        if c.eq_ignore_ascii_case("i") && !event.repeat && !s.command() =>
                    {
                        s.toggle_inspected_device_guard();
                    }
                    Key::Character(c) => {
                        // 1–9 are the open sub-category's first nine slots,
                        // 0 the tenth; P picks the selected plant's next way
                        // to start (seeds, cuttings, fragments, runners, buds).
                        if let Ok(n) = c.parse::<usize>() {
                            s.select_slot((n + 9) % 10);
                        } else if c.eq_ignore_ascii_case("p") {
                            s.next_plant_propagule();
                        } else if s.tools.tool() == Tool::Magnify
                            && matches!(c.as_str(), "+" | "=" | "]")
                        {
                            if s.tools.step_zoom(1) {
                                s.show_magnify();
                            }
                        } else if s.tools.tool() == Tool::Magnify
                            && matches!(c.as_str(), "-" | "[")
                            && s.tools.step_zoom(-1)
                        {
                            s.show_magnify();
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
        let on_battery = matches!(self.power.signals().source, PowerSource::Battery { .. });
        s.tank.set_on_battery(on_battery);
        let decision = s.decision(&self.settings.power, &self.power);
        s.tank
            .set_hidden(matches!(decision, PowerDecision::Pause(_)));
        if decision != s.last_decision {
            match (s.last_decision, decision) {
                (PowerDecision::Run { .. }, PowerDecision::Pause(reason)) => {
                    eprintln!("desktank window: {reason}, paused");
                    if let Some(p) = &mut self.persist {
                        // Paused from here on, so nothing changes until it runs
                        // again.
                        p.save(s.tank.world(), "saved while paused");
                    }
                }
                (PowerDecision::Pause(_), PowerDecision::Run { max_fps }) => {
                    eprintln!("desktank window: visible ({max_fps} fps)");
                    s.tank.set_max_fps(max_fps);
                    s.tank.invalidate();
                }
                (PowerDecision::Run { .. }, PowerDecision::Run { max_fps }) => {
                    eprintln!("desktank window: frame cap {max_fps} fps");
                    s.tank.set_max_fps(max_fps);
                }
                (PowerDecision::Pause(_), PowerDecision::Pause(reason)) => {
                    eprintln!("desktank window: {reason}, paused");
                }
            }
            s.last_decision = decision;
        }
        if matches!(decision, PowerDecision::Pause(_)) {
            // Paused: world time stops and nothing wakes us but OS events.
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

/// The Konami detector's view of a key; `None` for a modifier on its own
/// (Shift for a capital B doesn't break the code).
fn konami_key(key: &Key) -> Option<CheatKey> {
    Some(match key {
        Key::Named(NamedKey::ArrowUp) => CheatKey::Up,
        Key::Named(NamedKey::ArrowDown) => CheatKey::Down,
        Key::Named(NamedKey::ArrowLeft) => CheatKey::Left,
        Key::Named(NamedKey::ArrowRight) => CheatKey::Right,
        Key::Character(c) if c.eq_ignore_ascii_case("b") => CheatKey::B,
        Key::Character(c) if c.eq_ignore_ascii_case("a") => CheatKey::A,
        Key::Named(
            NamedKey::Shift
            | NamedKey::Control
            | NamedKey::Alt
            | NamedKey::Super
            | NamedKey::Meta
            | NamedKey::Hyper
            | NamedKey::Fn
            | NamedKey::CapsLock
            | NamedKey::AltGraph,
        ) => return None,
        _ => CheatKey::Other,
    })
}

/// `at` moved `steps` along a ring of `n`.
fn wrap(at: usize, steps: i32, n: usize) -> usize {
    let n = i64::try_from(n).unwrap_or(1).max(1);
    let at = i64::try_from(at).unwrap_or(0);
    usize::try_from((at + i64::from(steps)).rem_euclid(n)).unwrap_or(0)
}

/// The inspect line for device `id` under cell `(x, y)` (D27, DV3–DV7).
fn device_line(world: &World, id: DeviceId, x: usize, y: usize) -> Option<String> {
    let status = world.device_status(id)?;
    let art = world.device_kinds().get(status.kind);
    let label = &art.label;
    let place = match art.mount {
        Mount::SideGlass => {
            if status.facing_left {
                "on the right glass"
            } else {
                "on the left glass"
            }
        }
        Mount::Bed => "on the bed",
        Mount::Rim => "on the rim",
    };
    if !status.is_filter {
        return Some(format!(
            "{label} #{} · {place} · air diffuser · at ({x}, {y})",
            id.0
        ));
    }
    let guard = if status.fry_safe {
        "fry-safe"
    } else if status.intake_guard {
        "guard on"
    } else {
        "guard off"
    };
    let hint = if status.fry_safe {
        "R rinse · Shift+R hard rinse · M replace media"
    } else {
        "R rinse · Shift+R hard rinse · M replace media · I guard"
    };
    Some(format!(
        "{label} #{} · {place} · flow {:.1} cells/s · load {:.0}% · maturity {:.0}% · {guard} · at ({x}, {y}) · {hint}",
        id.0,
        status.flow,
        status.media_load * 100.0,
        status.maturity * 100.0,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktank_sim::{DeviceKind, MaterialTable, WorldConfig};

    #[test]
    fn device_line_omits_filter_fields_for_bubbler() {
        let mut w = World::new(
            WorldConfig {
                width: 120,
                height: 80,
                seed: 1,
                water_level: 0.9,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let bid = w
            .add_device(DeviceKind::Bubbler, 50, 40, false)
            .unwrap()
            .unwrap();
        let line = device_line(&w, bid, 50, 76).unwrap();
        assert_eq!(line, "Bubbler #1 · on the bed · air diffuser · at (50, 76)");

        let fid = w
            .add_device(DeviceKind::InternalFilter, 4, 40, false)
            .unwrap()
            .unwrap();
        let filter_line = device_line(&w, fid, 4, 40).unwrap();
        assert!(filter_line.contains("Internal filter #2"));
        assert!(filter_line.contains("R rinse"));
    }
}
