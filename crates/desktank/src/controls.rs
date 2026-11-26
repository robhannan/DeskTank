//! Interact input shared by the window and the overlay (SPEC §2.5).
//!
//! [`Controls`] holds the interaction state: the pointer, the platform-free
//! [`Tools`], the palette tree and what it shows, the species card, the
//! Konami detector and the text field (cheat or name). A host borrows it
//! with its [`Tank`] and surface size as a [`Session`], hands it winit
//! window events, and shows [`Controls::take_title`] wherever it shows text:
//! the window's title bar, or the overlay's log until the drawn in-tank card
//! takes over (O5).

use desktank_render::{BrushPreview, Card, Magnifier, Palette, PaletteHit, Tank};
use desktank_sim::UnmetNeed;
use desktank_sim::cheats::{self, CheatKey, Konami};
use desktank_sim::tools::{
    AddRefused, Button, Inspection, Outcome, PaletteItem, PaletteSub, PaletteTab, Tool, Tools,
};
use desktank_sim::{
    CreatureId, DeviceId, FieldKind, MAX_NAME_CHARS, Mount, RinseKind, World, WorldError, scene,
};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Trackpad scroll distance (physical pixels) per brush-size step.
const SCROLL_STEP_PX: f64 = 24.0;

/// What the palette overlay last showed: the open tab and sub-category and
/// the selection (tab, sub-category, item).
type Shown = (usize, usize, Option<(usize, usize, usize)>);

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

/// What a host should do after [`Session::window_event`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Response {
    /// Not an input event; the host handles it.
    Ignored,
    /// Taken by the controls.
    Handled,
    /// Escape with no text field open: the window closes, the overlay
    /// leaves toggled interact mode.
    Escape,
}

/// Interaction state for one tank.
pub struct Controls {
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
    /// Whether the overlay's interact chord is held: its modifiers don't
    /// make a key a shortcut, and a [`Konami::with_chord`] detector counts.
    chord: Option<ModifiersState>,
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
    /// The text field open (cheat or name), if any.
    field: Option<TextField>,
    /// The creature inspect last showed, which `N` names; cleared when
    /// another tool is picked.
    inspected: Option<CreatureId>,
    /// The device inspect last showed (`id`, cell `x`, `y`), which `R` rinses
    /// and `I` toggles the intake guard on.
    inspected_device: Option<(DeviceId, usize, usize)>,
    /// The title text last set and not yet taken by the host.
    title: Option<String>,
}

impl Controls {
    /// Controls for `world`, sand selected, with `konami` watching keys
    /// ([`Konami::new`] for a focused window, [`Konami::with_chord`] for the
    /// overlay).
    ///
    /// # Errors
    ///
    /// Returns an error if the world has no sand.
    pub fn new(world: &World, konami: Konami) -> Result<Self, WorldError> {
        let tools = Tools::new(scene::material(world, "sand")?);
        let mut c = Self {
            cursor: None,
            avoid_cursor: false,
            tools,
            tree: Tools::palette_tree(world),
            open: 0,
            open_sub: 0,
            modifiers: ModifiersState::empty(),
            chord: None,
            scroll: 0.0,
            scroll_lines: 0.0,
            shown: None,
            closed_card: None,
            konami,
            field: None,
            inspected: None,
            inspected_device: None,
            title: None,
        };
        // Open the tab and sub-category holding the starting material.
        (c.open, c.open_sub) = c.selection().map_or((0, 0), |(t, sub, _)| (t, sub));
        Ok(c)
    }

    /// Borrows the controls with the tank they drive and its surface size.
    pub fn session<'a>(&'a mut self, tank: &'a mut Tank, size: (u32, u32)) -> Session<'a> {
        Session {
            c: self,
            tank,
            size,
        }
    }

    /// Sets the title text: `"DeskTank"` alone, or `"DeskTank — "` and a
    /// message.
    fn title(&mut self, title: &str) {
        self.title = Some(title.to_owned());
    }

    /// The title text set since the last call, if any.
    pub fn take_title(&mut self) -> Option<String> {
        self.title.take()
    }

    /// Says whether the interact chord (`modifiers`) is held, for a host
    /// that sees keys only through it; `None` when it is let go, which
    /// starts the Konami code over.
    pub fn set_chord(&mut self, chord: Option<ModifiersState>) {
        if chord.is_none() && self.chord.is_some() {
            self.konami.reset();
        }
        self.chord = chord;
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
}

/// [`Controls`] borrowed with their tank and its surface size.
pub struct Session<'a> {
    c: &'a mut Controls,
    tank: &'a mut Tank,
    size: (u32, u32),
}

impl Session<'_> {
    /// Takes a winit window event: the pointer, buttons, wheel, modifiers,
    /// focus and keys. Call [`Session::sync`] after a handled one.
    pub fn window_event(&mut self, event: &WindowEvent) -> Response {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.c.cursor = Some((position.x, position.y));
                self.pointer_moved();
            }
            WindowEvent::CursorLeft { .. } => {
                self.c.cursor = None;
                self.c.tools.leave(self.tank.world_mut());
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    MouseButton::Left => Button::Primary,
                    MouseButton::Right => Button::Secondary,
                    _ => return Response::Handled,
                };
                if *state == ElementState::Pressed {
                    self.pressed(button);
                } else {
                    self.release(button);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => self.scrolled(*delta),
            WindowEvent::ModifiersChanged(m) => self.c.modifiers = m.state(),
            WindowEvent::Focused(false) => self.c.konami.reset(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                return self.key(event);
            }
            _ => return Response::Ignored,
        }
        Response::Handled
    }

    /// Moves the pointer to `cursor` (physical pixels), or off the tank.
    pub fn point_at(&mut self, cursor: Option<(f64, f64)>) {
        self.c.cursor = cursor;
        if cursor.is_some() {
            self.pointer_moved();
        } else {
            self.c.tools.leave(self.tank.world_mut());
        }
    }

    /// Lets go of `button`, reporting what the tool did.
    pub fn release(&mut self, button: Button) {
        let out = self.c.tools.release(self.tank.world_mut(), button);
        self.show(out);
    }

    /// Holding still keeps pouring: call once per simulation tick run.
    pub fn hold(&mut self) {
        self.c.tools.hold(self.tank.world_mut());
    }

    /// Ends interaction for now (the overlay going click-through): any
    /// stroke ends, the pointer and brush outline go, and an open text
    /// field closes. The selection and palette stay as they were.
    pub fn leave(&mut self) {
        self.c.cursor = None;
        self.c.tools.leave(self.tank.world_mut());
        self.c.field = None;
        self.c.konami.reset();
        self.c.modifiers = ModifiersState::empty();
        self.sync();
    }

    /// Pushes the palette, card and brush outline to the tank.
    pub fn sync(&mut self) {
        self.sync_overlay();
    }

    /// A key press: the text field and Konami code first, then the
    /// bindings.
    #[allow(clippy::too_many_lines)] // one arm per key binding
    fn key(&mut self, event: &KeyEvent) -> Response {
        if self.field_input(event) {
            return Response::Handled;
        }
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => return Response::Escape,
            Key::Character(c) if c.eq_ignore_ascii_case("a") && !event.repeat => {
                self.c.avoid_cursor = !self.c.avoid_cursor;
                let state = if self.c.avoid_cursor { "on" } else { "off" };
                self.c
                    .title(&format!("DeskTank — cursor avoidance {state}"));
            }
            Key::Character(c) if c.eq_ignore_ascii_case("s") && !event.repeat => {
                let (label, next) = if self.c.modifiers.shift_key() {
                    let next = !self.tank.allows_sway_on_battery();
                    self.tank.set_allow_sway_on_battery(next);
                    ("plant sway on battery", next)
                } else {
                    let next = !self.tank.sway();
                    self.tank.set_sway(next);
                    ("plant sway", next)
                };
                let state = if next { "on" } else { "off" };
                self.c.title(&format!("DeskTank — {label} {state}"));
            }
            Key::Character(c) if c.eq_ignore_ascii_case("f") => {
                use desktank_render::FieldOverlay::{Flow, Scalar};
                let next = match self.tank.field_overlay() {
                    None => Some(Scalar(FieldKind::Nutrient)),
                    Some(Scalar(FieldKind::Nutrient)) => Some(Scalar(FieldKind::Co2)),
                    Some(Scalar(FieldKind::Co2)) => Some(Scalar(FieldKind::Light)),
                    Some(Scalar(FieldKind::Light)) => Some(Flow),
                    Some(Flow) => None,
                };
                self.tank.set_field_overlay(next);
                let title = next.map_or("DeskTank".to_owned(), |f| {
                    format!("DeskTank — {} field", f.name())
                });
                self.c.title(&title);
            }
            // X mirrors the next flippable device placed onto the other
            // glass (D27); the ghost shows where it goes.
            Key::Character(c)
                if c.eq_ignore_ascii_case("x")
                    && !event.repeat
                    && !self.command()
                    && !self.c.modifiers.alt_key()
                    && self.c.tools.can_flip(self.tank.world()) =>
            {
                let side = if self.c.tools.flip() {
                    "other"
                } else {
                    "nearer"
                };
                self.c
                    .title(&format!("DeskTank — devices go on the {side} glass"));
            }
            Key::Character(c) if c.eq_ignore_ascii_case("w") => {
                let siphoned = self.tank.world_mut().water_change();
                self.tank.cells_edited();
                let q = self.tank.world().ecosystem().quality;
                self.c.title(&format!(
                    "DeskTank — water changed, {siphoned} cells siphoned, quality {:.0}%",
                    q * 100.0
                ));
            }
            Key::Character(c) if c.eq_ignore_ascii_case("q") => {
                let e = self.tank.world().ecosystem();
                self.c.title(&format!(
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
                self.step_tab(if self.c.modifiers.shift_key() { -1 } else { 1 });
            }
            Key::Character(c) if c.eq_ignore_ascii_case("h") => {
                self.step_sub(if self.c.modifiers.shift_key() { -1 } else { 1 });
            }
            // N names the creature inspect last showed.
            Key::Character(c)
                if c.eq_ignore_ascii_case("n") && !event.repeat && !self.command() =>
            {
                self.open_name_field();
            }
            // R rinses the inspected filter (Shift+R hard rinse),
            // M replaces its media, and I toggles its intake guard
            // (D27, DV3).
            Key::Character(c)
                if c.eq_ignore_ascii_case("r") && !event.repeat && !self.command() =>
            {
                let kind = if self.c.modifiers.shift_key() {
                    RinseKind::Hard
                } else {
                    RinseKind::Gentle
                };
                self.rinse_inspected_device(kind);
            }
            Key::Character(c)
                if c.eq_ignore_ascii_case("m") && !event.repeat && !self.command() =>
            {
                self.rinse_inspected_device(RinseKind::Replace);
            }
            Key::Character(c)
                if c.eq_ignore_ascii_case("i") && !event.repeat && !self.command() =>
            {
                self.toggle_inspected_device_guard();
            }
            Key::Character(c) => {
                // 1–9 are the open sub-category's first nine slots,
                // 0 the tenth; P picks the selected plant's next way
                // to start (seeds, cuttings, fragments, runners, buds).
                if let Ok(n) = c.parse::<usize>() {
                    self.select_slot((n + 9) % 10);
                } else if c.eq_ignore_ascii_case("p") {
                    self.next_plant_propagule();
                } else if self.c.tools.tool() == Tool::Magnify
                    && matches!(c.as_str(), "+" | "=" | "]")
                {
                    if self.c.tools.step_zoom(1) {
                        self.show_magnify();
                    }
                } else if self.c.tools.tool() == Tool::Magnify
                    && matches!(c.as_str(), "-" | "[")
                    && self.c.tools.step_zoom(-1)
                {
                    self.show_magnify();
                }
            }
            _ => {}
        }
        Response::Handled
    }

    /// The cell under the cursor, unless the cursor is over the palette
    /// or the species card.
    fn cursor_cell(&self) -> Option<(i64, i64)> {
        let (px, py) = self.c.cursor?;
        let size = self.size;
        if self
            .tank
            .palette_layout(size)
            .is_some_and(|p| p.contains(px, py))
            || self
                .tank
                .card_layout(size)
                .is_some_and(|c| c.contains(px, py))
        {
            return None;
        }
        Some(self.tank.layout(size).cell_at(px, py))
    }

    fn pointer_moved(&mut self) {
        match self.cursor_cell() {
            Some(cell) => {
                let out = self.c.tools.moved(self.tank.world_mut(), cell);
                self.show(out);
            }
            // Over the palette (or gone): hide the outline; a stroke ends.
            None => self.c.tools.leave(self.tank.world_mut()),
        }
    }

    /// Presses `button` where the pointer is: on the card or palette, or
    /// with the selected tool in the tank.
    pub fn pressed(&mut self, button: Button) {
        let size = self.size;
        // A click on the species card closes it.
        if let (Some((px, py)), Some(c)) = (self.c.cursor, self.tank.card_layout(size))
            && c.contains(px, py)
        {
            self.c.closed_card = self.tank.card();
            return;
        }
        if let (Some((px, py)), Some(p)) = (self.c.cursor, self.tank.palette_layout(size)) {
            match p.hit(px, py) {
                Some(PaletteHit::Tab(t)) => {
                    self.open_tab(t);
                    return;
                }
                Some(PaletteHit::Sub(sub)) => {
                    self.c.open_sub = sub;
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
            let out = self.c.tools.press(self.tank.world_mut(), button, cell);
            self.show(out);
        }
    }

    /// Selects a palette item. A plant slot names its species and
    /// propagule in the title.
    fn choose(&mut self, item: PaletteItem) {
        self.c.tools.select(item);
        if matches!(item, PaletteItem::Tool(Tool::Plant(_))) {
            self.show_plant();
        } else if item == PaletteItem::Tool(Tool::Magnify) {
            self.show_magnify();
        }
    }

    /// Shows the active magnification factor in the title bar.
    fn show_magnify(&mut self) {
        let zoom = self.c.tools.zoom();
        self.sync_overlay();
        self.c.title(&format!(
            "DeskTank — magnify {zoom}× (scroll, click or +/- to zoom)"
        ));
    }

    /// Names the species the selected plant slot pours, and as what, in
    /// the title bar.
    fn show_plant(&mut self) {
        let world = self.tank.world();
        let Some(s) = self
            .c
            .tools
            .selected_plant()
            .and_then(|id| world.plant_species().get(id))
        else {
            return;
        };
        let way = self
            .c
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
        self.c.title(&title);
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
            self.c.title(&format!("DeskTank — {text}"));
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
                self.c.inspected_device = Some((id, x, y));
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
                self.c.inspected = Some(id);
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
        self.c.title(&format!("DeskTank — {text}"));
    }

    /// Shows the open text field in the title bar, after `note` (the last
    /// cheat's error) if any.
    fn show_field(&mut self, note: Option<&str>) {
        let Some(field) = &self.c.field else { return };
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
        self.c.title(&title);
    }

    /// Gives a key press to the text fields first: an open field takes
    /// every key, and the press completing the Konami code opens the cheat
    /// field instead of doing its usual binding (A). Returns whether it
    /// took it.
    fn field_input(&mut self, event: &KeyEvent) -> bool {
        if self.c.field.is_some() {
            self.field_key(event);
            return true;
        }
        if let Some(key) = konami_key(&event.logical_key)
            && !event.repeat
            && !self.command()
            && self.c.konami.key(key, self.c.chord.is_some())
        {
            self.open_field(Purpose::Cheat, String::new());
            return true;
        }
        false
    }

    fn open_field(&mut self, purpose: Purpose, line: String) {
        self.c.field = Some(TextField { purpose, line });
        self.show_field(None);
    }

    /// Forgets the creature or device inspect last showed, closing its name field.
    fn forget_inspected(&mut self) {
        self.c.inspected = None;
        self.c.inspected_device = None;
        if self
            .c
            .field
            .as_ref()
            .is_some_and(|f| matches!(f.purpose, Purpose::Name(_)))
        {
            self.c.field = None;
        }
    }

    /// Rinses the device inspect last showed (`R` gentle, `Shift+R` hard).
    fn rinse_inspected_device(&mut self, kind: RinseKind) {
        let Some((id, x, y)) = self.c.inspected_device else {
            return;
        };
        self.tank.world_mut().rinse_device(id, kind);
        if let Some(line) = device_line(self.tank.world(), id, x, y) {
            self.c.title(&format!("DeskTank — {line}"));
        } else {
            self.c.inspected_device = None;
            self.c
                .title("DeskTank — that device is no longer in the tank");
        }
    }

    /// Toggles the intake guard on the device inspect last showed (`I`).
    fn toggle_inspected_device_guard(&mut self) {
        let Some((id, x, y)) = self.c.inspected_device else {
            return;
        };
        if self
            .tank
            .world_mut()
            .toggle_device_intake_guard(id)
            .is_some()
            && let Some(line) = device_line(self.tank.world(), id, x, y)
        {
            self.c.title(&format!("DeskTank — {line}"));
        } else {
            self.c.inspected_device = None;
            self.c
                .title("DeskTank — that device is no longer in the tank");
        }
    }

    /// Opens the name field for the creature inspect last showed (`N`),
    /// holding its name so far.
    fn open_name_field(&mut self) {
        let world = self.tank.world();
        if let Some(c) = self.c.inspected.and_then(|id| world.creature(id)) {
            let (id, name) = (c.id(), c.name().unwrap_or("").to_owned());
            self.open_field(Purpose::Name(id), name);
        } else {
            self.c.inspected = None;
            self.c
                .title("DeskTank — inspect a creature, then N names it");
        }
    }

    /// Whether Ctrl or Command is held, besides the interact chord.
    fn command(&self) -> bool {
        let held = self
            .c
            .modifiers
            .difference(self.c.chord.unwrap_or_else(ModifiersState::empty));
        held.control_key() || held.super_key()
    }

    /// Takes a key press while a text field is open: Enter runs the cheat
    /// (closing the field if it ran) or names the creature, Escape closes
    /// the field, Backspace deletes, and text is typed in.
    fn field_key(&mut self, event: &KeyEvent) {
        let command = self.command();
        let Some(field) = self.c.field.as_mut() else {
            return;
        };
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.c.field = None;
                self.c.title("DeskTank");
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
                            self.c.field = None;
                            self.c.title(&format!("DeskTank — cheat: {}", out.message));
                        }
                        Err(e) => self.show_field(Some(&e.to_string())),
                    },
                    Purpose::Name(id) => {
                        self.c.field = None;
                        let world = self.tank.world_mut();
                        let title = if world.set_creature_name(id, &line).is_some() {
                            let card = world.creature_card(id).map(|c| c.summary());
                            format!("DeskTank — {} · N renames it", card.unwrap_or_default())
                        } else {
                            "DeskTank — that creature has left the tank".to_owned()
                        };
                        self.c.title(&title);
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
                self.c.scroll_lines += f64::from(y);
                let steps = self.c.scroll_lines.trunc();
                self.c.scroll_lines -= steps;
                steps
            }
            MouseScrollDelta::PixelDelta(p) => {
                self.c.scroll += p.y;
                let steps = (self.c.scroll / SCROLL_STEP_PX).trunc();
                self.c.scroll -= steps * SCROLL_STEP_PX;
                steps
            }
        };
        #[allow(clippy::cast_possible_truncation)] // clamped to a small range
        let steps = steps.clamp(-100.0, 100.0) as i32;
        let over_palette = self.c.cursor.zip(self.tank.palette_layout(self.size));
        if over_palette.is_some_and(|((px, py), p)| p.contains(px, py)) {
            // Over the palette: scroll down for the next sub-category.
            self.step_through(-steps);
        } else if self.c.tools.tool() == Tool::Magnify {
            if self.c.tools.step_zoom(steps) {
                self.show_magnify();
            }
        } else {
            self.c.tools.resize(steps);
        }
    }

    /// Opens top tab `t` on the sub-category holding the selection, if it
    /// is in that tab, else on its first.
    fn open_tab(&mut self, t: usize) {
        self.c.open = t;
        self.c.open_sub = self
            .selection()
            .filter(|&(st, _, _)| st == t)
            .map_or(0, |(_, sub, _)| sub);
    }

    /// Opens the top tab `steps` after (or before) the open one, wrapping.
    fn step_tab(&mut self, steps: i32) {
        self.open_tab(wrap(self.c.open, steps, self.c.tree.len()));
    }

    /// Opens the open tab's sub-category `steps` after (or before) the
    /// open one, wrapping within the tab.
    fn step_sub(&mut self, steps: i32) {
        let n = self.c.tree.get(self.c.open).map_or(1, |t| t.subs.len());
        self.c.open_sub = wrap(self.c.open_sub, steps, n);
    }

    /// Steps through every sub-category in palette order, `steps` on (or
    /// back), on into the next (or previous) tab and wrapping: what
    /// scrolling over the palette does.
    fn step_through(&mut self, steps: i32) {
        let all: Vec<(usize, usize)> = self
            .c
            .tree
            .iter()
            .enumerate()
            .flat_map(|(t, tab)| (0..tab.subs.len()).map(move |sub| (t, sub)))
            .collect();
        let at = all
            .iter()
            .position(|&p| p == (self.c.open, self.c.open_sub))
            .unwrap_or(0);
        if let Some(&(t, sub)) = all.get(wrap(at, steps, all.len())) {
            (self.c.open, self.c.open_sub) = (t, sub);
        }
    }

    /// The open sub-category's items.
    fn open_items(&self) -> &[PaletteItem] {
        self.c
            .tree
            .get(self.c.open)
            .and_then(|t| t.subs.get(self.c.open_sub))
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
        if self
            .c
            .tools
            .next_plant_propagule(self.tank.world())
            .is_some()
        {
            self.show_plant();
        } else {
            self.c
                .title("DeskTank — select a plant, then P picks how it starts");
        }
    }

    /// The selection's tab, sub-category and index there.
    fn selection(&self) -> Option<(usize, usize, usize)> {
        self.c.selection()
    }

    /// Pushes the palette (open tabs and selection) and brush outline to
    /// the tank; it only asks for a frame if they changed.
    fn sync_overlay(&mut self) {
        let selected = self.selection();
        let shown = (self.c.open, self.c.open_sub, selected);
        if self.c.shown != Some(shown) {
            if self.c.shown.is_none_or(|(_, _, s)| s != selected) {
                // A new tool clears the last inspection from the title.
                self.c.title("DeskTank");
                self.forget_inspected();
            }
            self.c.shown = Some(shown);
            let first = |s: &PaletteSub| s.items.first().copied();
            let open = self.c.tree.get(self.c.open);
            // A tab with one sub-category has no sub-tab row.
            let subs = open
                .filter(|t| t.subs.len() > 1)
                .map_or_else(Vec::new, |t| t.subs.iter().filter_map(first).collect());
            self.tank.set_palette(Some(Palette {
                tabs: self
                    .c
                    .tree
                    .iter()
                    .filter_map(|t| t.subs.first().and_then(first))
                    .collect(),
                open: self.c.open,
                subs,
                open_sub: self.c.open_sub,
                items: self.open_items().to_vec(),
                selected,
            }));
        }
        // The selected creature's or plant's species card, unless closed.
        let card = selected
            .and_then(|(t, sub, i)| self.c.tree.get(t)?.subs.get(sub)?.items.get(i).copied())
            .and_then(Card::of);
        if self.c.closed_card != card {
            // Another species (or none) is selected: its card opens.
            self.c.closed_card = None;
        }
        self.tank
            .set_card(card.filter(|&c| self.c.closed_card != Some(c)));
        let pointer = self.c.tools.pointer().filter(|_| self.c.avoid_cursor);
        self.tank.world_mut().set_creature_cursor(pointer);
        let cursor = self.c.tools.preview().map(|(x, y, radius)| BrushPreview {
            cell: (x, y),
            radius,
        });
        self.tank.set_cursor(cursor);
        let ghost = self.c.tools.ghost(self.tank.world());
        self.tank.set_device_ghost(ghost);
        let magnifier = self
            .c
            .tools
            .magnifier()
            .map(|(x, y, zoom)| Magnifier { cell: (x, y), zoom });
        self.tank.set_magnifier(magnifier);
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

/// The inspect line for device `id` under cell `(x, y)`, including its mount (D27, DV3–DV7).
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
        Mount::Floor | Mount::FloorPanel => "on the floor",
    };
    if !status.is_filter {
        let detail = match (art.mount, status.connected) {
            (Mount::FloorPanel, true) => "connected",
            (Mount::FloorPanel, false) => "disconnected",
            _ => "air diffuser",
        };
        return Some(format!(
            "{label} #{} · {place} · {detail} · at ({x}, {y})",
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
