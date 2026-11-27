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
//! drag scoops, and scrolling or pressing `[` / `]` resizes the brush (or
//! steps magnification with Magnify selected). The palette in the top-left
//! corner is a two-level tree (`desktank_sim::tools::Tools::palette_tree`): its top row picks a
//! category (click a tab, or `G` / `Shift+G` for the next or previous one),
//! the row below it picks a sub-category when the category has several
//! (click one, or `H` / `Shift+H`), and the last row lists that
//! sub-category's materials, devices, tools, plants or creatures; click one
//! (or press 1–9, then 0 for the tenth) to select it. Scrolling over the
//! palette steps through the sub-categories, on into the next category. A device
//! (bubbler, filter outlet) is placed once per click. `F` cycles a debug
//! tint of the nutrient, CO₂ and light fields; `U` toggles the centimetre
//! ruler overlay (`--ruler` starts with it on); `W` changes the water and
//! `Q` shows the water quality in the title bar. `--time HH:MM` lights the
//! tank for that time of day (default noon). `--daylight-schedule location`
//! with `--latitude` and `--longitude` (or the `[daylight]` config) sets
//! sunrise and sunset for today at that place, so `--time` shows the
//! tank at that hour of its day; the window doesn't ask the OS where it
//! is (`run --overlay` does). Pointer moves only redraw
//! when the brush outline changes. Escape closes.
//!
//! Plants (T6b): each plant species has a slot under Plants; selected, it
//! pours its species as the propagule named in the title bar (seeds,
//! cuttings, fragments, runners or buds), and `P` picks the next one that
//! species grows from (remembered per species). Inspect pins the drawn
//! inspect card to what it finds (a creature, plant, device or cell); the
//! title bar keeps only the keys that act on it.
//!
//! Creatures (T7a): the palette's creature slots add one per click; the net
//! carries a creature and, let go above the water, lifts it out; inspect
//! shows its card beside it (T9: its name, species, stage, sex, age,
//! health, energy, morph and how long a carrying female has to go, then
//! its species). Its card has a Name row: `N`, or a click on the row, edits
//! the name in the card itself: Enter names it (a blank name removes it),
//! Escape cancels (a click elsewhere does too). Names are saved with the
//! tank. `--fish N` stocks the tank with N
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
    PauseReason, PowerDecision, PowerMonitor, PowerPolicy, PowerSource, local_date_time,
    process_stats,
};
use desktank_render::{
    BrushPreview, Card, Gpu, GpuOptions, Inspected, Magnifier, Palette, PaletteHit, PaletteMorphs,
    Tank, TankConfig, TankSurface, wgpu,
};
use desktank_sim::cheats::{self, CheatKey, Konami};
use desktank_sim::tools::{
    AddRefused, Button, Inspection, Outcome, PaletteItem, PaletteSub, PaletteTab, Tool, Tools,
};
use desktank_sim::{
    CreatureId, DeviceId, FieldKind, MAX_NAME_CHARS, MaterialTable, RinseKind, World, WorldConfig,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use crate::InteractState;
use crate::config::{self, CliOverrides, Config, Daylight, Life};
use crate::ipc::{self, Reply, Request};
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
    ruler: bool,
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
        ruler,
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
    /// Whether the centimetre ruler overlay starts on (`--ruler`).
    ruler: bool,
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

/// What the palette overlay last showed: the open tab and sub-category,
/// the selection (tab, sub-category, item) and the morph row under it.
type Shown = (
    usize,
    usize,
    Option<(usize, usize, usize)>,
    Option<PaletteMorphs>,
);

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
    /// The species card closed with a click; it stays closed until
    /// another species (or none) is selected.
    closed_card: Option<Card>,
    /// Watches key presses for the code that opens the cheat field.
    konami: Konami,
    /// The cheat field open in the title bar, if any.
    field: Option<TextField>,
    /// The creature inspect last showed, which `N` names; cleared when
    /// another tool is picked.
    inspected: Option<CreatureId>,
    /// The name being typed into the inspected creature's card (drawn there
    /// by the tank), if any.
    naming: Option<String>,
    /// The device inspect last showed, which `R` rinses
    /// and `I` toggles the intake guard on.
    inspected_device: Option<DeviceId>,
}

/// The cheat's one-line text field in the title bar.
struct TextField {
    line: String,
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
            paused_at: None,
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
            closed_card: None,
            konami: Konami::new(),
            field: None,
            naming: None,
            inspected: None,
            inspected_device: None,
        };
        // Every window can name creatures, in the inspect card.
        s.tank.set_inspect_naming(true);
        // Open the tab and sub-category holding the starting material.
        (s.open, s.open_sub) = s.selection().map_or((0, 0), |(t, sub, _)| (t, sub));
        s.sync_overlay();
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

    /// The cell under the cursor, unless the cursor is over the palette,
    /// the species card or the inspect card.
    fn cursor_cell(&self) -> Option<(i64, i64)> {
        let (px, py) = self.cursor?;
        let size = self.surface.size();
        if self
            .tank
            .palette_layout(size)
            .is_some_and(|p| p.contains(px, py))
            || self
                .tank
                .card_layout(size)
                .is_some_and(|c| c.contains(px, py))
            || self
                .tank
                .inspect_layout(size)
                .is_some_and(|c| c.contains(px, py))
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
        // A click on the Name row edits the name; any other click ends it.
        let on_name = self.cursor.is_some_and(|(px, py)| {
            self.tank
                .inspect_name_rect(size)
                .is_some_and(|r| r.contains(px, py))
        });
        if on_name {
            self.begin_naming();
            return;
        }
        self.cancel_naming();
        // A click on the species card closes it.
        if let (Some((px, py)), Some(c)) = (self.cursor, self.tank.card_layout(size))
            && c.contains(px, py)
        {
            self.closed_card = self.tank.card().map(species_card);
            return;
        }
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
                Some(PaletteHit::Morph(slot)) => {
                    self.pick_morph(slot);
                    return;
                }
                None if p.contains(px, py) => return,
                None => {}
            }
        }
        // A click on the inspect card turns its page.
        if let (Some((px, py)), Some(c)) = (self.cursor, self.tank.inspect_layout(size))
            && c.contains(px, py)
        {
            let page = self.tank.inspect_page().turned();
            self.tank.set_inspect_page(page);
            return;
        }
        if let Some(cell) = self.cursor_cell() {
            let out = self.tools.press(self.tank.world_mut(), button, cell);
            self.show(out);
        }
    }

    /// Selects a palette item. A plant slot names its species and
    /// propagule in the title, a multi-morph creature slot its morph.
    fn choose(&mut self, item: PaletteItem) {
        self.tools.select(item);
        if matches!(item, PaletteItem::Tool(Tool::Plant(_))) {
            self.show_plant();
        } else if matches!(item, PaletteItem::Tool(Tool::AddCreature(_))) {
            self.show_creature();
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

    /// Names the colour morph the selected creature slot adds in the title
    /// bar, if its species has more than one.
    fn show_creature(&mut self) {
        let world = self.tank.world();
        let Some(sp) = self
            .tools
            .selected_creature()
            .and_then(|id| world.species().get(id))
            .filter(|sp| sp.morphs.len() > 1)
        else {
            return;
        };
        let morph = match self.tools.creature_morph(world) {
            Some(m) => sp.morphs.get(usize::from(m)),
            None if sp.cycle_palette => None,
            None => sp.morphs.get(usize::from(sp.palette_morph)),
        };
        let pick = morph.map_or_else(|| "cycling morphs".to_owned(), |m| m.replace('_', " "));
        let title = format!(
            "DeskTank — {} ({pick}, C to change morph)",
            sp.name.replace('_', " ")
        );
        self.sync_overlay();
        self.window.set_title(&title);
    }

    /// Picks the morph in slot `slot` of the palette's morph row: the
    /// automatic slot first if the species cycles.
    fn pick_morph(&mut self, slot: usize) {
        let world = self.tank.world();
        let Some(sp) = self
            .tools
            .selected_creature()
            .and_then(|id| world.species().get(id))
        else {
            return;
        };
        // The automatic slot picks no morph; a slot beyond a u8 is no pick.
        let morph = match (sp.cycle_palette, slot) {
            (true, 0) => Some(None),
            (true, k) => u8::try_from(k - 1).ok().map(Some),
            (false, k) => u8::try_from(k).ok().map(Some),
        };
        if let Some(morph) = morph
            && self.tools.set_creature_morph(world, morph)
        {
            self.show_creature();
        }
    }

    /// Picks the next colour morph the selected creature slot adds (and
    /// automatic stocking, for species that cycle) and names it.
    fn next_creature_morph(&mut self) {
        if self.tools.next_creature_morph(self.tank.world()).is_some() {
            self.show_creature();
        } else {
            self.window
                .set_title("DeskTank — select a creature with morphs, then C picks its colour");
        }
    }

    /// Reports an inspection or refused creature add in the title bar, and
    /// pins the drawn inspect card to an inspected creature or plant.
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
        // shown now, an open name field is for the one shown before, and
        // the drawn card moves to this creature or plant (or goes).
        self.forget_inspected();
        self.tank.set_inspected(Some(Inspected::of(found)));
        // The card shows what was found (and offers the name); the title
        // keeps only the keys that need it.
        let hint = match found {
            Inspection::Creature { id, .. } => {
                self.inspected = Some(id);
                None
            }
            Inspection::Device { id, .. } => {
                self.inspected_device = Some(id);
                device_hint(self.tank.world(), id)
            }
            Inspection::Cell { .. } | Inspection::Plant { .. } => None,
        };
        self.window.set_title(&hint.map_or_else(
            || "DeskTank".to_owned(),
            |hint| format!("DeskTank — {hint}"),
        ));
    }

    /// Shows the open cheat field in the title bar, after `note` (the last
    /// cheat's error) if any.
    fn show_field(&self, note: Option<&str>) {
        let Some(field) = &self.field else { return };
        let note = note.map_or(String::new(), |n| format!("{n} · "));
        let line = &field.line;
        self.window.set_title(&format!(
            "DeskTank — {note}cheat: {line}_ (Enter runs, Esc closes)"
        ));
    }

    /// Gives a key press to the text fields first: an open field (or the
    /// name being typed in the card) takes every key, and the press
    /// completing the Konami code opens the cheat field instead of doing
    /// its usual binding (A). Returns whether it took it.
    fn field_input(&mut self, event: &KeyEvent) -> bool {
        if self.field.is_some() {
            self.field_key(event);
            return true;
        }
        if self.naming.is_some() {
            self.name_key(event);
            return true;
        }
        if let Some(key) = konami_key(&event.logical_key)
            && !event.repeat
            && !self.command()
            && self.konami.key(key, false)
        {
            self.open_field();
            return true;
        }
        false
    }

    /// Opens the cheat field (ending a name being typed).
    fn open_field(&mut self) {
        self.cancel_naming();
        self.field = Some(TextField {
            line: String::new(),
        });
        self.show_field(None);
    }

    /// Forgets the creature or device inspect last showed, ending a name
    /// being typed for it.
    fn forget_inspected(&mut self) {
        self.inspected = None;
        self.inspected_device = None;
        self.cancel_naming();
    }

    /// Rinses the device inspect last showed (`R` gentle, `Shift+R` hard).
    fn rinse_inspected_device(&mut self, kind: RinseKind) {
        let Some(id) = self.inspected_device else {
            return;
        };
        self.tank.world_mut().rinse_device(id, kind);
        // Redraw so the card shows it, even while paused.
        self.tank.invalidate();
        if self.tank.world().device(id).is_none() {
            self.inspected_device = None;
            self.window
                .set_title("DeskTank — that device is no longer in the tank");
        }
    }

    /// Toggles the intake guard on the device inspect last showed (`I`).
    fn toggle_inspected_device_guard(&mut self) {
        let Some(id) = self.inspected_device else {
            return;
        };
        self.tank.invalidate();
        if self
            .tank
            .world_mut()
            .toggle_device_intake_guard(id)
            .is_none()
        {
            self.inspected_device = None;
            self.window
                .set_title("DeskTank — that device is no longer in the tank");
        }
    }

    /// Starts editing the name of the creature inspect last showed (`N`,
    /// or a click on the card's Name row), from its name so far; the card
    /// shows the text with a caret.
    fn begin_naming(&mut self) {
        let world = self.tank.world();
        if let Some(c) = self.inspected.and_then(|id| world.creature(id)) {
            let name = c.name().unwrap_or("").to_owned();
            self.tank.set_inspect_name_edit(Some(&name));
            self.naming = Some(name);
        } else {
            self.inspected = None;
            self.window
                .set_title("DeskTank — inspect a creature, then N names it");
        }
    }

    /// Ends a name being typed without naming the creature.
    fn cancel_naming(&mut self) {
        if self.naming.take().is_some() {
            self.tank.set_inspect_name_edit(None);
        }
    }

    /// Takes a key press while a name is typed in the card: Enter names the
    /// creature (a blank name removes it), Escape cancels, Backspace
    /// deletes, and text is typed in up to [`MAX_NAME_CHARS`].
    fn name_key(&mut self, event: &KeyEvent) {
        let command = self.command();
        let Some(line) = self.naming.as_mut() else {
            return;
        };
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => self.cancel_naming(),
            Key::Named(NamedKey::Enter) => {
                let line = line.clone();
                self.cancel_naming();
                if let Some(id) = self.inspected {
                    if self.tank.world_mut().set_creature_name(id, &line).is_none() {
                        self.inspected = None;
                        self.window
                            .set_title("DeskTank — that creature has left the tank");
                    }
                    // Redraw so the card shows the name, even while paused.
                    self.tank.invalidate();
                }
            }
            Key::Named(NamedKey::Backspace) => {
                line.pop();
                let line = line.clone();
                self.tank.set_inspect_name_edit(Some(&line));
            }
            _ if command => {}
            _ => {
                let typed = event.text.as_ref().map_or("", |t| t.as_str());
                for c in typed.chars().filter(|c| !c.is_control()) {
                    if line.chars().count() < MAX_NAME_CHARS {
                        line.push(c);
                    }
                }
                let line = line.clone();
                self.tank.set_inspect_name_edit(Some(&line));
            }
        }
    }

    /// Whether Ctrl or Command is held.
    fn command(&self) -> bool {
        self.modifiers.control_key() || self.modifiers.super_key()
    }

    /// Takes a key press while the cheat field is open: Enter runs the
    /// cheat (closing the field if it ran), Escape closes the field,
    /// Backspace deletes, and text is typed in.
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
                match cheats::run(self.tank.world_mut(), &line) {
                    Ok(out) => {
                        if out.cells_changed > 0 {
                            self.tank.cells_edited();
                        }
                        self.field = None;
                        self.window
                            .set_title(&format!("DeskTank — cheat: {}", out.message));
                    }
                    Err(e) => self.show_field(Some(&e.to_string())),
                }
            }
            Key::Named(NamedKey::Backspace) => {
                field.line.pop();
                self.show_field(None);
            }
            _ if command => {}
            _ => {
                let typed = event.text.as_ref().map_or("", |t| t.as_str());
                for c in typed.chars().filter(|c| !c.is_control()) {
                    if field.line.chars().count() < cheats::MAX_LINE {
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

    /// The morph row for the selection: shown under the items while the
    /// selected creature's sub-category is open and its species has more
    /// than one morph.
    fn palette_morphs(&self, selected: Option<(usize, usize, usize)>) -> Option<PaletteMorphs> {
        selected.filter(|&(t, sub, _)| (t, sub) == (self.open, self.open_sub))?;
        let id = self.tools.selected_creature()?;
        let world = self.tank.world();
        let sp = world.species().get(id).filter(|sp| sp.morphs.len() > 1)?;
        let pick = self.tools.morph_for(world, id);
        Some(PaletteMorphs {
            species: id,
            count: sp.morphs.len(),
            cycle: sp.cycle_palette,
            selected: if sp.cycle_palette {
                pick
            } else {
                Some(pick.unwrap_or(sp.palette_morph))
            },
        })
    }

    /// Pushes the palette (open tabs and selection) and brush outline to
    /// the tank; it only asks for a frame if they changed.
    fn sync_overlay(&mut self) {
        let selected = self.selection();
        let morphs = self.palette_morphs(selected);
        let shown = (self.open, self.open_sub, selected, morphs);
        if self.shown != Some(shown) {
            if self.shown.is_none_or(|(_, _, s, _)| s != selected) {
                // A new tool clears the last inspection from the title and
                // the drawn card, so it can't catch the new tool's clicks.
                self.window.set_title("DeskTank");
                self.forget_inspected();
                self.tank.set_inspected(None);
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
                morphs,
            }));
        }
        // The selected creature's or plant's species card, unless closed.
        // A closed card stays closed through morph picks, which only change
        // the preview.
        let card = selected
            .and_then(|(t, sub, i)| self.tree.get(t)?.subs.get(sub)?.items.get(i).copied())
            .and_then(Card::of);
        if self.closed_card != card {
            // Another species (or none) is selected: its card opens.
            self.closed_card = None;
        }
        let world = self.tank.world();
        let card = card
            .filter(|&c| self.closed_card != Some(c))
            .map(|c| match c {
                Card::Creature(id, _) => Card::Creature(id, self.tools.morph_for(world, id)),
                plant @ Card::Plant(_) => plant,
            });
        self.tank.set_card(card);
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

/// `card` with no morph preview: the species alone, for telling whether a
/// card is the one that was closed.
fn species_card(card: Card) -> Card {
    match card {
        Card::Creature(id, _) => Card::Creature(id, None),
        plant @ Card::Plant(_) => plant,
    }
}

fn read_config(path: &Path) -> Result<Config, config::ConfigError> {
    config::load(path)
        .map(Option::unwrap_or_default)
        .inspect_err(|error| {
            eprintln!("desktank window: config {}: {error}", path.display());
        })
}

/// Lights the tank for the sun at the configured coordinates today, or with
/// the fixed schedule. The window has no clock tick, so this runs on start
/// and when the config's `[daylight]` table changes.
fn apply_daylight(tank: &mut Tank, daylight: Daylight) {
    tank.set_location_sync(daylight.location_sync());
    tank.set_daylight_schedule(
        daylight.schedule_at(local_date_time(), daylight.manual_coordinates()),
    );
}

fn apply_life(world: &mut World, life: Life, previous: Life) {
    if let Some(ratio) = life.time_ratio
        && life.time_ratio != previous.time_ratio
    {
        world.set_time_ratio(ratio);
    }
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
    let report = tank.catch_up(seconds);
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
                apply_daylight(&mut s.tank, self.settings.daylight);
                s.tank.set_oled(self.settings.display.oled);
                s.tank.set_ruler(self.ruler);
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
                if settings.display.oled != self.settings.display.oled {
                    s.tank.set_oled(settings.display.oled);
                }
                if settings.daylight != self.settings.daylight {
                    apply_daylight(&mut s.tank, settings.daylight);
                }
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
                    // U toggles the centimetre ruler overlay (R is the filter
                    // rinse).
                    Key::Character(c)
                        if c.eq_ignore_ascii_case("u")
                            && !event.repeat
                            && !s.command()
                            && !s.modifiers.alt_key() =>
                    {
                        let next = !s.tank.ruler();
                        s.tank.set_ruler(next);
                        s.window.set_title(if next {
                            "DeskTank — ruler overlay (cm)"
                        } else {
                            "DeskTank"
                        });
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
                            "DeskTank — water quality {:.0}%, nutrients {:.2}, surface algae {:.1}%",
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
                        s.begin_naming();
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
                    // C picks the selected creature's next colour morph.
                    Key::Character(c)
                        if c.eq_ignore_ascii_case("c")
                            && !event.repeat
                            && !s.command()
                            && !s.modifiers.alt_key() =>
                    {
                        s.next_creature_morph();
                    }
                    Key::Character(c) => {
                        // 1–9 are the open sub-category's first nine slots,
                        // 0 the tenth; P picks the selected plant's next way
                        // to start (seeds, cuttings, fragments, runners, buds).
                        if let Ok(n) = c.parse::<usize>() {
                            s.select_slot((n + 9) % 10);
                        } else if c.eq_ignore_ascii_case("p") {
                            s.next_plant_propagule();
                        } else {
                            match size_key(s.tools.tool(), c.as_str(), s.modifiers) {
                                Some(SizeKey::Brush(steps)) => {
                                    s.tools.resize(steps);
                                }
                                Some(SizeKey::Zoom(steps)) if s.tools.step_zoom(steps) => {
                                    s.show_magnify();
                                }
                                _ => {}
                            }
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

#[derive(Debug, PartialEq, Eq)]
enum SizeKey {
    Brush(i32),
    Zoom(i32),
}

/// Maps `[`/`]` (and `+`/`=`/`-` for Magnify) to a brush or zoom step.
/// Ignores `Super` and bare `Ctrl` shortcuts while allowing `Alt` (macOS
/// Option) and `Ctrl+Alt` (Windows/Linux `AltGr`), which non-US layouts need to
/// type `[` and `]`.
fn size_key(tool: Tool, key: &str, modifiers: ModifiersState) -> Option<SizeKey> {
    if modifiers.super_key() || (modifiers.control_key() && !modifiers.alt_key()) {
        return None;
    }
    match (tool, key) {
        (Tool::Magnify, "[" | "-") => Some(SizeKey::Zoom(-1)),
        (Tool::Magnify, "]" | "+" | "=") => Some(SizeKey::Zoom(1)),
        (_, "[") => Some(SizeKey::Brush(-1)),
        (_, "]") => Some(SizeKey::Brush(1)),
        _ => None,
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

/// The keys for the device inspect last showed: a filter's rinse, media
/// and guard keys (the guard only if it isn't fry-safe); `None` otherwise.
fn device_hint(world: &World, id: DeviceId) -> Option<&'static str> {
    let status = world.device_status(id)?;
    if !status.is_filter {
        None
    } else if status.fry_safe {
        Some("R rinse · Shift+R hard rinse · M replace media")
    } else {
        Some("R rinse · Shift+R hard rinse · M replace media · I guard")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktank_sim::tools::{MAX_RADIUS, MIN_RADIUS, ZOOM_LEVELS};
    use desktank_sim::{DeviceKind, MaterialId, MaterialTable, WorldConfig};

    #[test]
    fn bracket_keys_resize_and_clamp_the_brush() {
        let plain = ModifiersState::empty();
        for tool in [Tool::Pour, Tool::Scoop, Tool::Glue, Tool::Inspect] {
            let mut tools = Tools::new(MaterialId(1));
            tools.select(PaletteItem::Tool(tool));
            tools.resize(i32::MIN);
            for (key, radius) in [("[", MIN_RADIUS), ("]", MIN_RADIUS + 1), ("[", MIN_RADIUS)] {
                let Some(SizeKey::Brush(steps)) = size_key(tools.tool(), key, plain) else {
                    panic!("{key} should resize the brush for {tool:?}");
                };
                tools.resize(steps);
                assert_eq!(tools.radius(), radius);
            }
            tools.resize(i32::MAX);
            let Some(SizeKey::Brush(steps)) = size_key(tools.tool(), "]", plain) else {
                panic!("] should resize the brush for {tool:?}");
            };
            assert!(!tools.resize(steps));
            assert_eq!(tools.radius(), MAX_RADIUS);
        }
    }

    #[test]
    fn magnify_size_keys_step_zoom() {
        let plain = ModifiersState::empty();
        let mut tools = Tools::new(MaterialId(1));
        tools.select(PaletteItem::Tool(Tool::Magnify));
        for (key, level) in [("]", 1), ("+", 2), ("=", 3), ("-", 2), ("[", 1)] {
            let Some(SizeKey::Zoom(steps)) = size_key(tools.tool(), key, plain) else {
                panic!("{key} should step magnification");
            };
            assert!(tools.step_zoom(steps));
            assert_eq!(tools.zoom(), ZOOM_LEVELS[level]);
        }
    }

    #[test]
    fn zoom_only_keys_do_nothing_for_other_tools() {
        let plain = ModifiersState::empty();
        for tool in [Tool::Pour, Tool::Scoop, Tool::Glue, Tool::Inspect] {
            for key in ["+", "=", "-"] {
                assert_eq!(size_key(tool, key, plain), None);
            }
        }
    }

    #[test]
    fn size_keys_allow_alt_and_altgr_but_ignore_command_shortcuts() {
        let alt = ModifiersState::ALT;
        let altgr = ModifiersState::CONTROL | ModifiersState::ALT;
        let ctrl = ModifiersState::CONTROL;
        let super_key = ModifiersState::SUPER;

        assert_eq!(size_key(Tool::Pour, "[", alt), Some(SizeKey::Brush(-1)));
        assert_eq!(size_key(Tool::Pour, "]", altgr), Some(SizeKey::Brush(1)));
        assert_eq!(size_key(Tool::Magnify, "[", altgr), Some(SizeKey::Zoom(-1)));
        assert_eq!(size_key(Tool::Pour, "[", ctrl), None);
        assert_eq!(size_key(Tool::Pour, "]", super_key), None);
        assert_eq!(size_key(Tool::Magnify, "=", ctrl), None);
        assert_eq!(size_key(Tool::Magnify, "]", super_key), None);
    }

    #[test]
    fn apply_daylight_lights_the_tank_for_the_sun_only_when_synced_with_a_place() {
        use crate::config::DaylightMode;
        use desktank_sim::daylight::DaylightSchedule;

        let world = World::new(
            WorldConfig {
                width: 32,
                height: 24,
                seed: 3,
                water_level: 0.8,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let mut tank = Tank::new(world, TankConfig::default());
        let london = Daylight {
            schedule: DaylightMode::Location,
            latitude: Some(51.5),
            longitude: Some(0.0),
        };
        apply_daylight(&mut tank, london);
        assert!(tank.location_sync());
        let schedule = tank.daylight_schedule();
        assert_ne!(schedule, DaylightSchedule::FIXED);
        assert_eq!(tank.world().daylight_schedule(), schedule);
        // Without a place the window can't know the sun, so it stays fixed.
        let unlocated = Daylight {
            latitude: None,
            longitude: None,
            ..london
        };
        apply_daylight(&mut tank, unlocated);
        assert_eq!(tank.daylight_schedule(), DaylightSchedule::FIXED);
        apply_daylight(&mut tank, london);
        apply_daylight(&mut tank, Daylight::default());
        assert!(!tank.location_sync());
        assert_eq!(tank.daylight_schedule(), DaylightSchedule::FIXED);
    }

    #[test]
    fn apply_life_sets_the_time_ratio_only_when_it_changes() {
        let mut world = World::new(
            WorldConfig {
                width: 32,
                height: 24,
                seed: 3,
                water_level: 0.8,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let life = Life {
            time_ratio: Some(60.0),
            ..Life::default()
        };
        apply_life(&mut world, life, Life::default());
        assert!((world.time_ratio() - 60.0).abs() < f32::EPSILON);
        // Unchanged since the last config: a reload leaves the saved ratio.
        world.set_time_ratio(5.0);
        apply_life(&mut world, life, life);
        assert!((world.time_ratio() - 5.0).abs() < f32::EPSILON);
        // Absent from the config: the saved ratio stands.
        apply_life(&mut world, Life::default(), life);
        assert!((world.time_ratio() - 5.0).abs() < f32::EPSILON);
    }

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

    #[test]
    fn only_a_filter_has_device_keys() {
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
        assert_eq!(device_hint(&w, bid), None);
        let fid = w
            .add_device(DeviceKind::InternalFilter, 4, 40, false)
            .unwrap()
            .unwrap();
        assert!(device_hint(&w, fid).unwrap().starts_with("R rinse"));
        assert_eq!(device_hint(&w, DeviceId(99)), None);
    }
}
