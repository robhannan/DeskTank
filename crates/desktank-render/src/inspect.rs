//! The in-tank inspect card (SPEC §2.5): a card pinned beside whatever was
//! last inspected (a creature, a plant, a device or a cell), following it
//! as it moves.
//!
//! Page 1 is what inspect found right now (a creature's name, stage, age,
//! health and so on from [`World::creature_card`]; a plant's growth; a
//! device's mount and filter state; a cell's material). Creatures and
//! plants have a second page, their species card ([`crate::card`]), and
//! then each page carries its number in the top-right corner; devices and
//! materials have no encyclopedia entry yet, so their card is one page. The card is built on the CPU like the species card and
//! drawn from a third region of the HUD texture, so it needs nothing a host
//! provides beyond the tank: a screensaver or wallpaper can show it as well
//! as an interactive window.
//!
//! A creature's live page also has a **Name** row (SPEC §2.5): its name, or,
//! where the host allows naming, a prompt. The host starts editing by a click
//! or tap on the row ([`InspectCard::name_rect`] says where it is) or the `N`
//! key and then mirrors what is typed into [`InspectCard::set_name_edit`];
//! the row shows it with a caret (steady, so a blink never asks for frames)
//! and a hint. The card never reads the keyboard itself.
//!
//! A creature of a species with needs also shows a **Needs** section of
//! labelled meters (SPEC §2.5, [`crate::needs`]): how well each need is met
//! and which way it is moving, taken from the simulation's own per-creature
//! satisfaction and a short history of it that lasts only while it is
//! inspected. A plant's live page has the same section for its care: how
//! well its light, CO₂ and nutrients suit it ([`World::plant_needs`]),
//! beside the **Growth** line that names what holds it back most.
//!
//! The live page is rebuilt only when what it shows changes (energy moves a
//! percent, a meter's fill or trend moves a step, a day passes, a letter is
//! typed), never per frame; while nothing is inspected nothing is built,
//! uploaded or drawn.

use desktank_sim::tools::Inspection;
use desktank_sim::{
    CreatureId, DeviceId, MAX_NAME_CHARS, Mount, MovementClass, NeedReading, PlantId, UnmetNeed,
    World,
};

use crate::camera::ViewRect;
use crate::card::{
    self, CARD_MAX, CARD_W, Card, CardImage, CardLayout, Page, card_scale, display_name,
};
use crate::hud::PaletteLayout;
use crate::layout::Layout;
use crate::needs::{self, NeedBar, Trends};

/// The row of the HUD texture where the inspect card's region starts (below
/// the species card's; its left edge is column 0).
pub const TEXTURE_Y: u32 = card::TEXTURE_Y + CARD_MAX.1;

/// Target pixels between the inspected thing and the card, per card pixel.
const GAP: u32 = 3;

/// Points a plant's need reading may drift from the one shown before the
/// meter follows it (see [`InspectCard::hold`]).
const HOLD: u8 = 2;

/// What the inspect card is pinned to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inspected {
    /// A live creature.
    Creature(CreatureId),
    /// A plant.
    Plant(PlantId),
    /// A device (D27).
    Device(DeviceId),
    /// A cell, whatever material is in it.
    Cell {
        /// Column.
        x: usize,
        /// Row.
        y: usize,
    },
}

impl Inspected {
    /// What an inspection pins the card to: the creature, plant or device
    /// found, else the cell.
    #[must_use]
    pub fn of(found: Inspection) -> Self {
        match found {
            Inspection::Creature { id, .. } => Self::Creature(id),
            Inspection::Plant { id, .. } => Self::Plant(id),
            Inspection::Device { id, .. } => Self::Device(id),
            Inspection::Cell { x, y, .. } => Self::Cell { x, y },
        }
    }

    /// Whether its card has a species page: creatures and plants do;
    /// devices and materials have no encyclopedia entry yet.
    #[must_use]
    pub fn has_species_page(self) -> bool {
        matches!(self, Self::Creature(_) | Self::Plant(_))
    }
}

/// The inspect card's two pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InspectPage {
    /// What inspect found now.
    #[default]
    Live,
    /// Its species card.
    Species,
}

impl InspectPage {
    /// The other page.
    #[must_use]
    pub fn turned(self) -> Self {
        match self {
            Self::Live => Self::Species,
            Self::Species => Self::Live,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Self::Live => "1/2",
            Self::Species => "2/2",
        }
    }
}

/// The inspect card a tank shows: what it is pinned to, its page and its
/// image, kept up to date with the world by [`InspectCard::refresh`].
#[derive(Debug, Default)]
pub(crate) struct InspectCard {
    /// What it is pinned to, if anything.
    pub(crate) what: Option<Inspected>,
    /// The page shown.
    pub(crate) page: InspectPage,
    /// The live page's text the image was last built from.
    live: Option<Live>,
    /// The image (`None` while nothing is inspected).
    pub(crate) image: Option<CardImage>,
    /// The image needs uploading.
    pub(crate) dirty: bool,
    /// Whether the host lets the viewer name creatures.
    can_name: bool,
    /// The name being typed, while editing.
    edit: Option<String>,
    /// The name row in the image, as `[x, y, width, height]` in card
    /// pixels (none unless the live page shows it and naming is allowed).
    name_row: Option<[u32; 4]>,
    /// Whether the card is narrow: the needs then take one column.
    narrow: bool,
    /// The history the needs' trends are read from.
    trends: Trends,
    /// The plant readings last shown, which a reading that has only
    /// wobbled does not replace (see [`HOLD`]).
    held: Vec<NeedReading>,
}

impl InspectCard {
    /// Pins the card to `what` on its live page; returns whether that was a
    /// change (it then needs a frame).
    pub(crate) fn set(&mut self, world: &World, what: Option<Inspected>) -> bool {
        if self.what == what {
            return false;
        }
        self.what = what;
        self.page = InspectPage::Live;
        self.edit = None;
        // A trend belongs to one creature or plant: the next starts afresh.
        self.trends.reset();
        self.held.clear();
        self.refresh(world, true);
        true
    }

    /// Lays the needs out in one column (a narrow card) or two; returns
    /// whether a frame is needed.
    pub(crate) fn set_narrow(&mut self, world: &World, narrow: bool) -> bool {
        if self.narrow == narrow {
            return false;
        }
        self.narrow = narrow;
        self.what.is_some() && self.refresh(world, false)
    }

    /// Allows (or stops) naming creatures, which shows (or hides) the
    /// prompt on an unnamed creature's Name row; stopping drops any edit.
    /// Returns whether a frame is needed.
    pub(crate) fn set_can_name(&mut self, world: &World, can_name: bool) -> bool {
        if self.can_name == can_name {
            return false;
        }
        self.can_name = can_name;
        if !can_name {
            self.edit = None;
        }
        self.what.is_some() && self.refresh(world, false)
    }

    /// Starts editing the creature's name from `edit` (`None`: stops),
    /// showing the live page if it was on the species page. The text is cut
    /// to [`MAX_NAME_CHARS`] and has no control characters; it only counts
    /// for a creature when naming is allowed. Returns whether a frame is
    /// needed.
    pub(crate) fn set_name_edit(&mut self, world: &World, edit: Option<&str>) -> bool {
        let edit = edit
            .filter(|_| self.can_name && matches!(self.what, Some(Inspected::Creature(_))))
            .map(|text| {
                text.chars()
                    .filter(|c| !c.is_control())
                    .take(MAX_NAME_CHARS)
                    .collect::<String>()
            });
        if self.edit == edit {
            return false;
        }
        self.edit = edit;
        if self.edit.is_some() && self.page != InspectPage::Live {
            self.page = InspectPage::Live;
            return self.refresh(world, true);
        }
        self.what.is_some() && self.refresh(world, false)
    }

    /// The name being typed, if editing.
    pub(crate) fn name_edit(&self) -> Option<&str> {
        self.edit.as_deref()
    }

    /// Where the name row is when the card is drawn at `at` (its
    /// [`Self::layout`]), for hit tests; `None` unless naming is allowed
    /// and the live page of a creature's card is shown.
    pub(crate) fn name_rect(&self, at: &CardLayout) -> Option<ScreenRect> {
        let [left, top, width, height] = self.name_row.filter(|_| self.can_name)?;
        let scale = at.scale;
        Some(ScreenRect {
            origin: (at.origin.0 + left * scale, at.origin.1 + top * scale),
            size: (width * scale, height * scale),
        })
    }

    /// Shows `page` (only the live page for a card without a species
    /// page); returns whether a frame is needed.
    pub(crate) fn set_page(&mut self, world: &World, page: InspectPage) -> bool {
        let one_page = self.what.is_some_and(|w| !w.has_species_page());
        if self.page == page || (one_page && page == InspectPage::Species) {
            return false;
        }
        self.page = page;
        self.what.is_some() && self.refresh(world, true)
    }

    /// Brings the card up to date with `world`: drops it if its creature or
    /// plant has gone, and rebuilds the image if `force` or the live page
    /// shown has changed. Returns whether a frame is needed.
    pub(crate) fn refresh(&mut self, world: &World, force: bool) -> bool {
        let Some(what) = self.what else {
            self.live = None;
            return self.image.take().is_some() || force;
        };
        let Some((mut live, readings)) = live_with_readings(world, what) else {
            *self = Self {
                can_name: self.can_name,
                narrow: self.narrow,
                ..Self::default()
            };
            return true;
        };
        if let Some(row) = &mut live.name {
            row.can_name = self.can_name;
            row.editing.clone_from(&self.edit);
        }
        live.narrow = self.narrow;
        self.read_trends(world, what, readings, &mut live);
        let changed = self.live.as_ref() != Some(&live);
        let rebuild = force || (changed && self.page == InspectPage::Live);
        if rebuild {
            let built = build(world, what, self.page, &live);
            (self.image, self.name_row) = built.map_or((None, None), |(i, r)| (Some(i), r));
            self.dirty = true;
        }
        if changed {
            self.live = Some(live);
        }
        rebuild
    }

    /// Fills in the trend of each of `live`'s need meters from the history
    /// (a creature's or plant's own; nothing else has needs).
    fn read_trends(
        &mut self,
        world: &World,
        what: Inspected,
        readings: LiveReadings,
        live: &mut Live,
    ) {
        let observed: Vec<NeedReading> = match readings {
            LiveReadings::None => return,
            LiveReadings::Creature(r) => r,
            LiveReadings::Plant(mut r) => {
                self.hold(&mut r);
                live.needs = r
                    .iter()
                    .take(needs::MAX_NEEDS)
                    .map(|&r| NeedBar::of(r))
                    .collect();
                Vec::from(r)
            }
        };
        let trends = self.trends.observe(what, world.tick_count(), &observed);
        for (bar, trend) in live.needs.iter_mut().zip(trends) {
            bar.trend = trend;
        }
    }

    /// Steadies a plant's `now` against what was last shown: the light,
    /// CO₂ and nutrient fields a plant reads shimmer by a point or two from
    /// tick to tick, which would flicker a meter's end between two steps
    /// and rebuild the card for nothing. A reading within [`HOLD`] points of
    /// the one shown stays as it was, except that full and empty are shown
    /// as soon as they are reached.
    fn hold(&mut self, now: &mut [NeedReading; 3]) {
        if self.held.len() == now.len()
            && self.held.iter().zip(&*now).all(|(a, b)| a.need == b.need)
        {
            for (n, shown) in now.iter_mut().zip(&self.held) {
                let end = n.satisfaction == 0 || n.satisfaction >= 100;
                if !end && n.satisfaction.abs_diff(shown.satisfaction) <= HOLD {
                    n.satisfaction = shown.satisfaction;
                }
            }
        }
        self.held.clear();
        self.held.extend_from_slice(now);
    }

    /// Where it is drawn in a `target`-pixel surface laid out as `grid`,
    /// or seen through `camera`'s view of it.
    pub(crate) fn layout(
        &self,
        world: &World,
        target: (u32, u32),
        grid: &Layout,
        camera: Option<&ViewRect>,
        palette: Option<&PaletteLayout>,
        min_scale: u32,
    ) -> Option<CardLayout> {
        let image = self.image.as_ref()?;
        let anchor = anchor(world, self.what?)?;
        match camera {
            None => CardLayout::beside(target, grid, palette, min_scale, anchor, image.size),
            Some(view) => {
                CardLayout::beside_view(target, grid, view, palette, min_scale, anchor, image.size)
            }
        }
    }
}

/// The text of the live page: what changes it must rebuild the image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Live {
    title: String,
    subtitle: String,
    fields: Vec<(&'static str, String)>,
    /// A creature's Name row.
    name: Option<NameRow>,
    /// A creature's or plant's need meters, in display order (none for a
    /// species without supported needs).
    needs: Vec<NeedBar>,
    /// Whether the needs take one column, not two.
    narrow: bool,
}

/// A creature's Name row: what is drawn changes with any of these.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct NameRow {
    /// Its name, if it has one.
    name: Option<String>,
    /// Whether the host allows naming (so an unnamed creature shows the
    /// prompt, and the row can be clicked).
    can_name: bool,
    /// The text being typed, while editing.
    editing: Option<String>,
}

/// A rectangle in a target's pixels, such as the name row's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenRect {
    /// Top-left.
    pub origin: (u32, u32),
    /// Width and height.
    pub size: (u32, u32),
}

impl ScreenRect {
    /// Whether target pixel `(px, py)` is inside it.
    #[must_use]
    pub fn contains(&self, px: f64, py: f64) -> bool {
        let (x, y) = (px - f64::from(self.origin.0), py - f64::from(self.origin.1));
        x >= 0.0 && y >= 0.0 && x < f64::from(self.size.0) && y < f64::from(self.size.1)
    }
}

/// What the Name row says where naming is allowed and there is no name.
const NAME_PROMPT: &str = "N or tap to name";
/// What it says under the text being typed.
const NAME_HINT: &str = "Enter saves, Esc cancels";

enum LiveReadings {
    None,
    Creature(Vec<NeedReading>),
    Plant([NeedReading; 3]),
}

fn live_with_readings(world: &World, what: Inspected) -> Option<(Live, LiveReadings)> {
    match what {
        Inspected::Creature(id) => creature_live(world, id),
        Inspected::Plant(id) => plant_live(world, id),
        Inspected::Device(id) => Some((device_live(world, id)?, LiveReadings::None)),
        Inspected::Cell { x, y } => Some((cell_live(world, x, y)?, LiveReadings::None)),
    }
}

/// The live page of `what`, or `None` if it is gone (died, netted out,
/// eaten away) or its species is unknown.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn live(world: &World, what: Inspected) -> Option<Live> {
    let (mut page, readings) = live_with_readings(world, what)?;
    if let LiveReadings::Plant(r) = readings {
        page.needs = r
            .iter()
            .take(needs::MAX_NEEDS)
            .map(|&r| NeedBar::of(r))
            .collect();
    }
    Some(page)
}

/// What a material does, in a few words.
fn class_name(class: MovementClass) -> &'static str {
    match class {
        MovementClass::Solid => "solid",
        MovementClass::Rigid => "rigid body",
        MovementClass::Powder => "powder",
        MovementClass::Liquid => "liquid",
        MovementClass::Buoyant => "rises",
        MovementClass::Floating => "floats, then sinks",
    }
}

fn cell_live(world: &World, x: usize, y: usize) -> Option<Live> {
    let material = world.materials().get(world.cell(x, y)?.material)?;
    Some(Live {
        title: display_name(&material.name),
        subtitle: String::new(),
        fields: vec![
            ("Kind:", class_name(material.class).to_owned()),
            ("Cell:", format!("{x}, {y}")),
        ],
        name: None,
        needs: Vec::new(),
        narrow: false,
    })
}

fn device_live(world: &World, id: DeviceId) -> Option<Live> {
    let s = world.device_status(id)?;
    let art = world.device_kinds().get(s.kind);
    let place = match art.mount {
        Mount::SideGlass if s.facing_left => "right glass",
        Mount::SideGlass => "left glass",
        Mount::Bed => "bed",
        Mount::Rim => "rim",
        Mount::Floor | Mount::FloorPanel => "floor",
    };
    let mut fields = vec![("On:", place.to_owned())];
    if s.is_filter {
        let guard = if s.fry_safe {
            "fry-safe"
        } else if s.intake_guard {
            "on"
        } else {
            "off"
        };
        fields.extend([
            ("Flow:", format!("{:.1} cells/s", s.flow)),
            ("Load:", percent(s.media_load)),
            ("Maturity:", percent(s.maturity)),
            ("Guard:", guard.to_owned()),
        ]);
    } else if art.mount == Mount::FloorPanel {
        let state = if s.connected {
            "connected"
        } else {
            "not connected"
        };
        fields.push(("Filter:", state.to_owned()));
    } else {
        fields.push(("Type:", "air diffuser".to_owned()));
    }
    Some(Live {
        title: format!("{} #{}", art.label, id.0),
        subtitle: String::new(),
        fields,
        name: None,
        needs: Vec::new(),
        narrow: false,
    })
}

fn percent(v: f32) -> String {
    format!("{:.0}%", (v * 100.0).clamp(0.0, 100.0))
}

/// An age in whole life days (SPEC §2.4), worded the same on every card.
fn age_text(days: u32) -> String {
    match days {
        0 => "under a day".to_owned(),
        1 => "1 day".to_owned(),
        d => format!("{d} days"),
    }
}

fn creature_live(world: &World, id: CreatureId) -> Option<(Live, LiveReadings)> {
    let card = world.creature_card(id)?;
    let species = world.creature(id)?.species();
    let kind = format!("{} #{}", display_name(&card.species), id.0);
    let sex = if card.female { "female" } else { "male" };
    let mut fields = vec![
        ("Stage:", format!("{} {sex}", card.stage.name())),
        ("Age:", age_text(card.age_days)),
        ("Health:", percent(card.health)),
        ("Energy:", percent(card.energy)),
    ];
    if let Some(morph) = card.morph {
        fields.push(("Colour:", morph.replace('_', " ")));
    }
    if let Some(days) = card.carrying_days {
        let s = if days == 1 { "" } else { "s" };
        fields.push(("Carrying:", format!("{days} day{s} to go")));
    }
    match world.unmet_need(species) {
        Some(UnmetNeed::Depth { needs, .. }) => {
            fields.push(("Lacks:", format!("{needs} cells of water")));
        }
        Some(UnmetNeed::Air) => fields.push(("Lacks:", "air at the surface".to_owned())),
        None => {}
    }
    let needs = card
        .needs
        .iter()
        .take(crate::needs::MAX_NEEDS)
        .map(|&r| NeedBar::of(r))
        .collect();
    Some((
        Live {
            title: kind,
            subtitle: String::new(),
            fields,
            name: Some(NameRow {
                name: card.name,
                ..NameRow::default()
            }),
            needs,
            narrow: false,
        },
        LiveReadings::Creature(card.needs),
    ))
}

fn plant_live(world: &World, id: PlantId) -> Option<(Live, LiveReadings)> {
    let p = world.plant(id)?;
    let sp = world.plant_species().get(p.species)?;
    let [roots, stem, leaves] = p.parts;
    let readings = world.plant_needs(id)?;
    Some((
        Live {
            title: format!("{} #{}", display_name(&sp.name), id.0),
            subtitle: String::new(),
            fields: vec![
                ("Age:", age_text(p.age_days())),
                ("Health:", percent(p.health)),
                (
                    "Size:",
                    format!(
                        "{} cells (roots {roots}, stem {stem}, leaves {leaves})",
                        p.size()
                    ),
                ),
                ("Growth:", p.limit.describe()),
            ],
            name: None,
            needs: Vec::new(),
            narrow: false,
        },
        LiveReadings::Plant(readings),
    ))
}

/// Draws the live page, numbered if there is a second, and where its Name
/// row is (`[x, y, width, height]` in card pixels) if it has one the host
/// can click.
pub(crate) fn build_live(live: &Live, numbered: bool) -> (CardImage, Option<[u32; 4]>) {
    let mut page = Page::new(numbered.then(|| InspectPage::Live.tag()));
    page.heading(&live.title, &live.subtitle);
    let row = live.name.as_ref().and_then(|row| name_row(&mut page, row));
    for (label, text) in &live.fields {
        page.field(label, text);
    }
    needs::draw(&mut page, &live.needs, live.narrow);
    (page.finish(), row)
}

/// Draws a creature's Name row, if it shows one: the name; for an
/// unnamed creature where naming is allowed, the prompt; while editing, the
/// text typed with a caret and the hint. Returns its rect (`None` if naming
/// is not allowed, so there is nothing to click).
fn name_row(page: &mut Page, row: &NameRow) -> Option<[u32; 4]> {
    let top = match (&row.editing, &row.name) {
        (Some(text), _) => {
            let top = page.field_in("Name:", &format!("{text}_"), card::TITLE);
            page.paragraph(NAME_HINT, card::LATIN);
            top
        }
        (None, Some(name)) => page.field_in("Name:", name, card::BODY),
        (None, None) if row.can_name => page.field_in("Name:", NAME_PROMPT, card::LATIN),
        (None, None) => return None,
    };
    let bottom = page.bottom().min(CARD_MAX.1);
    row.can_name
        .then_some([1, top, CARD_W - 2, bottom.saturating_sub(top)])
}

/// Draws `page` of the card for `what`, given its live page, with the Name
/// row's rect (see [`build_live`]); `None` if its species is unknown.
pub(crate) fn build(
    world: &World,
    what: Inspected,
    page: InspectPage,
    live: &Live,
) -> Option<(CardImage, Option<[u32; 4]>)> {
    match page {
        InspectPage::Live => Some(build_live(live, what.has_species_page())),
        InspectPage::Species => {
            let species = match what {
                Inspected::Creature(id) => {
                    let c = world.creature(id)?;
                    Card::Creature(c.species(), Some(world.creature_morph(c)))
                }
                Inspected::Plant(id) => Card::Plant(world.plant(id)?.species),
                Inspected::Device(_) | Inspected::Cell { .. } => return None,
            };
            card::build_tagged(
                species,
                world.materials(),
                world.species(),
                world.plant_species(),
                Some(page.tag()),
            )
            .map(|image| (image, None))
        }
    }
}

/// The cells `what` covers, `(x0, y0, x1, y1)` (fractional for a
/// creature, which moves smoothly), or `None` if it is gone.
pub(crate) fn anchor(world: &World, what: Inspected) -> Option<[f32; 4]> {
    match what {
        Inspected::Creature(id) => {
            let c = world.creature(id)?;
            let (cx, cy) = c.centre();
            let (w, h) = world.creature_size(c);
            #[allow(clippy::cast_precision_loss)] // a few cells
            let (hw, hh) = (w.max(1) as f32 / 2.0, h.max(1) as f32 / 2.0);
            Some([cx - hw, cy - hh, cx + hw, cy + hh])
        }
        Inspected::Plant(id) => {
            let b = world.plant(id)?.bounds;
            #[allow(clippy::cast_precision_loss)] // grid coordinates
            let f = |v: usize| v as f32;
            Some([
                f(b.x0),
                f(b.y0),
                f(b.x1.max(b.x0 + 1)),
                f(b.y1.max(b.y0 + 1)),
            ])
        }
        Inspected::Device(id) => {
            let d = world.device(id)?;
            let art = world.device_kinds().get(d.kind);
            #[allow(clippy::cast_precision_loss)] // grid coordinates
            let f = |v: usize| v as f32;
            Some([f(d.x), f(d.y), f(d.x + art.size.0), f(d.y + d.span(art))])
        }
        Inspected::Cell { x, y } => {
            world.cell(x, y)?;
            #[allow(clippy::cast_precision_loss)] // grid coordinates
            let (x, y) = (x as f32, y as f32);
            Some([x, y, x + 1.0, y + 1.0])
        }
    }
}

impl CardLayout {
    /// Places an inspect card of `image` pixels beside `anchor` (cells, as
    /// `anchor` gives them) in `target`: to its right, or to its left if
    /// that has more room, vertically centred on it, then clamped inside
    /// the tank (the grid's area within `target`). Its scale is the one every
    /// species card is drawn at ([`card_scale`], from `palette` and
    /// `min_scale` as the species card gets them), so both pages and the
    /// species card match; it drops only if `image` doesn't fit `target` at
    /// that scale. `None` if it doesn't fit even at 1.
    #[must_use]
    pub fn beside(
        target: (u32, u32),
        grid: &Layout,
        palette: Option<&PaletteLayout>,
        min_scale: u32,
        anchor: [f32; 4],
        image: (u32, u32),
    ) -> Option<Self> {
        let cell = i64::from(grid.scale);
        let tank = [
            grid.offset.0,
            grid.offset.1,
            grid.offset.0 + i64::from(grid.grid.0) * cell,
            grid.offset.1 + i64::from(grid.grid.1) * cell,
        ];
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)] // pixels
        let px = |cells: f32, offset: i64| offset + (cells * grid.scale as f32).round() as i64;
        let edges = [
            px(anchor[0], grid.offset.0),
            px(anchor[1], grid.offset.1),
            px(anchor[2], grid.offset.0),
            px(anchor[3], grid.offset.1),
        ];
        Self::place(target, grid, palette, min_scale, tank, edges, image)
    }

    /// [`CardLayout::beside`] for a camera: `anchor` and the tank are the
    /// cells and grid seen through `view` of `target`, while the card's
    /// scale still comes from `grid`, the integer layout the tank has
    /// without a camera, so it is as crisp and as big as ever.
    #[must_use]
    pub fn beside_view(
        target: (u32, u32),
        grid: &Layout,
        view: &ViewRect,
        palette: Option<&PaletteLayout>,
        min_scale: u32,
        anchor: [f32; 4],
        image: (u32, u32),
    ) -> Option<Self> {
        #[allow(clippy::cast_possible_truncation)] // pixels
        let round = |px: [f64; 4]| px.map(|v| v.round().clamp(-1e9, 1e9) as i64);
        #[allow(clippy::cast_precision_loss)] // grid cells
        let grid_cells = [0.0, 0.0, grid.grid.0 as f32, grid.grid.1 as f32];
        let tank = round(view.rect_to_screen(target, grid_cells));
        let edges = round(view.rect_to_screen(target, anchor));
        Self::place(target, grid, palette, min_scale, tank, edges, image)
    }

    /// Places the card beside the anchor whose `[left, top, right, bottom]`
    /// target pixels are `edges`, inside the part of the `tank` rectangle
    /// (same form) that is on the target.
    fn place(
        target: (u32, u32),
        grid: &Layout,
        palette: Option<&PaletteLayout>,
        min_scale: u32,
        tank: [i64; 4],
        edges: [i64; 4],
        image: (u32, u32),
    ) -> Option<Self> {
        if image.0 == 0 || image.1 == 0 || target.0 == 0 || target.1 == 0 {
            return None;
        }
        let (target_w, target_h) = (i64::from(target.0), i64::from(target.1));
        // The tank's area in the target: left, top, right, bottom.
        let tank = [
            tank[0].max(0),
            tank[1].max(0),
            tank[2].min(target_w),
            tank[3].min(target_h),
        ];
        let [left_edge, top_edge, right_edge, bottom_edge] = edges;
        let want = card_scale(target, grid, palette, min_scale);
        (1..=want).rev().find_map(|scale| {
            let width = i64::from(image.0) * i64::from(scale);
            let height = i64::from(image.1) * i64::from(scale);
            if width > target_w || height > target_h {
                return None;
            }
            // Clamp to the tank where it fits, else to the target.
            let (min_x, max_x) = if width <= tank[2] - tank[0] {
                (tank[0], tank[2])
            } else {
                (0, target_w)
            };
            let (min_y, max_y) = if height <= tank[3] - tank[1] {
                (tank[1], tank[3])
            } else {
                (0, target_h)
            };
            let gap = i64::from(GAP * scale);
            let right = right_edge + gap;
            let on_right = right + width <= max_x || max_x - right_edge >= left_edge - min_x;
            let left = if on_right {
                right
            } else {
                left_edge - gap - width
            };
            let left = left.clamp(min_x, max_x - width);
            let top =
                (i64::midpoint(top_edge, bottom_edge) - height / 2).clamp(min_y, max_y - height);
            Some(Self {
                origin: (u32::try_from(left).ok()?, u32::try_from(top).ok()?),
                scale,
                image,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font;
    use desktank_sim::{MaterialTable, WorldConfig};

    fn world() -> World {
        World::new(
            WorldConfig {
                width: 160,
                height: 90,
                seed: 3,
                water_level: 0.9,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap()
    }

    fn grid() -> Layout {
        Layout::fit((320, 180), (1280, 720))
    }

    #[test]
    fn pages_turn_both_ways_and_are_tagged() {
        assert_eq!(InspectPage::default(), InspectPage::Live);
        assert_eq!(InspectPage::Live.turned(), InspectPage::Species);
        assert_eq!(InspectPage::Species.turned(), InspectPage::Live);
        assert_ne!(InspectPage::Live.tag(), InspectPage::Species.tag());
    }

    #[test]
    fn a_creature_live_page_shows_its_state_and_its_species_page_its_card() {
        let mut w = world();
        let guppy = w.species().id("guppy").unwrap();
        let id = w.add_creature(guppy, 80, 40).unwrap();
        let what = Inspected::Creature(id);
        let live = live(&w, what).unwrap();
        assert_eq!(live.title, format!("Guppy #{}", id.0));
        let labels: Vec<_> = live.fields.iter().map(|f| f.0).collect();
        assert!(labels.starts_with(&["Stage:", "Age:", "Health:", "Energy:"]));
        for text in [&live.title, &live.subtitle]
            .into_iter()
            .chain(live.fields.iter().flat_map(|f| [&f.1]))
        {
            assert!(
                text.chars().all(|c| crate::font::glyph(c).is_some()),
                "{text}"
            );
        }
        let (one, _) = build(&w, what, InspectPage::Live, &live).unwrap();
        let (two, _) = build(&w, what, InspectPage::Species, &live).unwrap();
        assert!(!one.clipped && !two.clipped);
        assert_eq!(one.size.0, card::CARD_W);
        assert_ne!(one, two);
        // Page 2 is the palette's species card with its page number.
        let plain = card::build(
            Card::Creature(guppy, None),
            w.materials(),
            w.species(),
            w.plant_species(),
        )
        .unwrap();
        assert_eq!(two.size.0, plain.size.0);
        assert_ne!(two.pixels, plain.pixels, "the tag is drawn");
        // A name is the Name row's, not the title.
        w.set_creature_name(id, "Bubbles");
        let named = super::live(&w, what).unwrap();
        assert_eq!(named.title, live.title);
        assert_eq!(
            named.name.as_ref().and_then(|row| row.name.as_deref()),
            Some("Bubbles")
        );
        assert_ne!(named, live);
    }

    /// A creature's live page for a card with these name settings.
    fn creature_page(
        w: &World,
        id: CreatureId,
        can_name: bool,
        editing: Option<&str>,
    ) -> (CardImage, Option<[u32; 4]>) {
        let what = Inspected::Creature(id);
        let mut live = live(w, what).unwrap();
        let row = live.name.as_mut().unwrap();
        row.can_name = can_name;
        row.editing = editing.map(str::to_owned);
        build(w, what, InspectPage::Live, &live).unwrap()
    }

    #[test]
    fn a_creature_card_has_a_name_row_that_prompts_shows_and_edits() {
        let mut w = world();
        let guppy = w.species().id("guppy").unwrap();
        let id = w.add_creature(guppy, 80, 40).unwrap();

        // A viewer sees no row for an unnamed creature, the owner a prompt.
        let (viewer, none) = creature_page(&w, id, false, None);
        let (owner, row) = creature_page(&w, id, true, None);
        assert_eq!(none, None);
        assert_ne!(viewer.pixels, owner.pixels);
        let [x, y, rw, rh] = row.unwrap();
        // The row is inside the card, under the heading, as wide as it.
        assert!(x >= 1 && x + rw < card::CARD_W && y > 0 && y + rh <= owner.size.1);
        assert!((font::LINE..2 * font::LINE + 8).contains(&rh));

        // Editing: the text typed with a caret, and the hint under it, so
        // the row is taller and the card too; each letter changes the image.
        let (empty, empty_row) = creature_page(&w, id, true, Some(""));
        let (typed, typed_row) = creature_page(&w, id, true, Some("Bub"));
        assert_ne!(empty.pixels, typed.pixels);
        assert_ne!(owner.pixels, empty.pixels);
        assert!(empty.size.1 > owner.size.1);
        assert!(empty_row.unwrap()[3] > rh);
        assert_eq!(
            typed_row.unwrap()[..2],
            [x, y],
            "it starts where the prompt did"
        );
        assert!(!typed.clipped);
        // The caret is drawn: the same text without it is a different image.
        let mut plain = live(&w, Inspected::Creature(id)).unwrap();
        plain.name.as_mut().unwrap().name = Some("Bub".to_owned());
        plain.name.as_mut().unwrap().can_name = true;
        let (named, _) = build_live(&plain, true);
        assert_ne!(typed.pixels, named.pixels);

        // A name is shown (a viewer sees it too, with no row to click).
        w.set_creature_name(id, "Bubbles");
        let (viewer, none) = creature_page(&w, id, false, None);
        let (owner, row) = creature_page(&w, id, true, None);
        assert_eq!(none, None);
        assert!(row.is_some());
        assert_eq!(
            viewer.size, owner.size,
            "the prompt and a name are one line"
        );

        // The longest name, and the longest typed, fit the card's width on
        // one line each (so truncating or wrapping is never needed).
        let longest = "W".repeat(MAX_NAME_CHARS);
        assert!(font::width(&format!("Name: {longest}_")) <= CARD_W - 8);
        let (long, _) = creature_page(&w, id, true, Some(&longest));
        assert_eq!(long.size, creature_page(&w, id, true, Some("")).0.size);
        assert!(!long.clipped);
        for text in [NAME_PROMPT, NAME_HINT] {
            assert!(text.chars().all(|c| font::glyph(c).is_some()), "{text}");
            assert!(font::width(&format!("Name: {text}")) <= CARD_W - 8);
        }
    }

    #[test]
    fn the_name_row_follows_the_card_and_is_hit_testable() {
        let g = grid();
        let mut w = world();
        let guppy = w.species().id("guppy").unwrap();
        let id = w.add_creature(guppy, 80, 40).unwrap();
        let mut card = InspectCard::default();
        card.set(&w, Some(Inspected::Creature(id)));
        let target = (1280, 720);
        // Not allowed: nothing to click. Allowed: a rect inside the card.
        assert_eq!(
            card.layout(&w, target, &g, None, None, 0)
                .and_then(|at| card.name_rect(&at)),
            None
        );
        assert!(card.set_can_name(&w, true));
        assert!(!card.set_can_name(&w, true));
        let at = card.layout(&w, target, &g, None, None, 0).unwrap();
        let rect = card
            .layout(&w, target, &g, None, None, 0)
            .and_then(|at| card.name_rect(&at))
            .unwrap();
        let [x, y, rw, rh] = card.name_row.unwrap();
        assert_eq!(
            rect.origin,
            (at.origin.0 + x * at.scale, at.origin.1 + y * at.scale)
        );
        assert_eq!(rect.size, (rw * at.scale, rh * at.scale));
        // The page-two card has no row.
        assert!(card.set_page(&w, InspectPage::Species));
        assert_eq!(
            card.layout(&w, target, &g, None, None, 0)
                .and_then(|at| card.name_rect(&at)),
            None
        );
        // Editing returns to page one and edits are cut and cleaned.
        assert!(card.set_name_edit(&w, Some("A\tb")));
        assert_eq!(card.page, InspectPage::Live);
        assert_eq!(card.name_edit(), Some("Ab"));
        assert!(!card.set_name_edit(&w, Some("Ab")));
        assert!(
            card.layout(&w, target, &g, None, None, 0)
                .and_then(|at| card.name_rect(&at))
                .is_some()
        );
        // Stopping naming stops the edit; a plant or a cell is not named.
        assert!(card.set_can_name(&w, false));
        assert_eq!(card.name_edit(), None);
        assert!(card.set_can_name(&w, true));
        assert!(card.set(&w, Some(Inspected::Cell { x: 5, y: 60 })));
        assert!(!card.set_name_edit(&w, Some("x")));
        assert_eq!(
            card.layout(&w, target, &g, None, None, 0)
                .and_then(|at| card.name_rect(&at)),
            None
        );
    }

    #[test]
    fn ages_read_the_same_on_every_card() {
        assert_eq!(age_text(0), "under a day");
        assert_eq!(age_text(1), "1 day");
        assert_eq!(age_text(2), "2 days");
        assert_eq!(age_text(400), "400 days");
    }

    #[test]
    fn a_plant_ages_in_life_days() {
        let mut w = world();
        // A cutting rooted in aquasoil thrives, so it is still there a day on.
        let aquasoil = w.materials().id("aquasoil").unwrap();
        w.fill_rect(
            desktank_sim::Rect {
                x0: 0,
                y0: 80,
                x1: 160,
                y1: 90,
            },
            aquasoil,
            desktank_sim::Brush::Replace,
        )
        .unwrap();
        let stem = w.materials().id("stem").unwrap();
        w.set_cell(40, 79, stem).unwrap();
        w.tick();
        let id = w.plants().next().unwrap().id;
        let age = |w: &World| live(w, Inspected::Plant(id)).unwrap().fields[0].clone();
        // Under a day for the first ten minutes of world time (a day is
        // `LIFE_DAY_TICKS` ticks).
        for _ in 0..desktank_sim::LIFE_DAY_TICKS - 10 {
            w.tick();
        }
        assert_eq!(age(&w), ("Age:", "under a day".to_owned()));
        for _ in 0..20 {
            w.tick();
        }
        assert_eq!(w.plant(id).map(|p| p.age_days()), Some(1));
        assert_eq!(age(&w), ("Age:", "1 day".to_owned()));
    }

    #[test]
    fn a_plant_live_page_shows_its_growth() {
        let mut w = world();
        // Leaf cells painted in make a plant of the default species.
        let leaf = w.materials().id("leaf").unwrap();
        w.fill_rect(
            desktank_sim::Rect {
                x0: 39,
                y0: 40,
                x1: 42,
                y1: 41,
            },
            leaf,
            desktank_sim::Brush::Replace,
        )
        .unwrap();
        let id = w.plants().next().unwrap().id;
        let what = Inspected::Plant(id);
        let live = live(&w, what).unwrap();
        assert_eq!(live.title, format!("Hygrophila #{}", id.0));
        assert_eq!(live.fields[0], ("Age:", "under a day".to_owned()));
        assert!(build(&w, what, InspectPage::Species, &live).is_some());
        let a = anchor(&w, what).unwrap();
        assert!(a[0] <= 40.0 && a[2] > 40.0, "{a:?}");
    }

    #[test]
    fn cells_and_devices_have_a_one_page_live_card() {
        let mut w = world();
        let cell = Inspected::Cell { x: 10, y: 60 };
        let page_text = super::live(&w, cell).unwrap();
        assert_eq!(page_text.title, "Water");
        assert_eq!(
            page_text.fields,
            [
                ("Kind:", "liquid".to_owned()),
                ("Cell:", "10, 60".to_owned())
            ]
        );
        assert!(!cell.has_species_page());
        assert!(build(&w, cell, InspectPage::Species, &page_text).is_none());
        let (page, row) = build(&w, cell, InspectPage::Live, &page_text).unwrap();
        assert_ne!(page, build_live(&page_text, true).0, "no page number");
        assert_eq!(row, None, "only a creature has a name row");
        assert_eq!(anchor(&w, cell), Some([10.0, 60.0, 11.0, 61.0]));
        assert!(live(&w, Inspected::Cell { x: 999, y: 0 }).is_none());

        let filter = w
            .add_device(desktank_sim::DeviceKind::InternalFilter, 4, 40, false)
            .unwrap()
            .unwrap();
        let bubbler = w
            .add_device(desktank_sim::DeviceKind::Bubbler, 80, 40, false)
            .unwrap()
            .unwrap();
        let f = live(&w, Inspected::Device(filter)).unwrap();
        assert_eq!(f.title, format!("Internal filter #{}", filter.0));
        assert_eq!(f.name, None);
        let labels: Vec<_> = f.fields.iter().map(|f| f.0).collect();
        assert_eq!(labels, ["On:", "Flow:", "Load:", "Maturity:", "Guard:"]);
        assert_eq!(f.fields[0].1, "left glass");
        let b = live(&w, Inspected::Device(bubbler)).unwrap();
        assert_eq!(
            b.fields.last().unwrap(),
            &("Type:", "air diffuser".to_owned())
        );
        let a = anchor(&w, Inspected::Device(filter)).unwrap();
        assert!(a[2] > a[0] + 4.0 && a[3] > a[1] + 4.0, "{a:?}");
        for text in [&f, &b]
            .iter()
            .flat_map(|l| l.fields.iter().map(|f| &f.1).chain([&l.title]))
        {
            assert!(
                text.chars().all(|c| crate::font::glyph(c).is_some()),
                "{text}"
            );
        }
    }

    #[test]
    fn a_one_page_card_does_not_turn() {
        let w = world();
        let mut card = InspectCard::default();
        assert!(card.set(&w, Some(Inspected::Cell { x: 5, y: 60 })));
        assert!(card.image.is_some());
        assert!(!card.set_page(&w, InspectPage::Species));
        assert_eq!(card.page, InspectPage::Live);
        assert!(card.image.is_some());
    }

    #[test]
    fn gone_things_have_no_card() {
        let w = world();
        assert!(live(&w, Inspected::Creature(CreatureId(99))).is_none());
        assert!(live(&w, Inspected::Plant(PlantId(99))).is_none());
        assert!(anchor(&w, Inspected::Creature(CreatureId(99))).is_none());
    }

    #[test]
    fn the_card_sits_beside_the_anchor_and_stays_in_the_tank() {
        let g = grid();
        let image = (144, 80);
        // Room on the right: just right of it, centred on it, at the species
        // card's scale (the cell scale 4 where it fits).
        let c = CardLayout::beside((1280, 720), &g, None, 0, [100.0, 80.0, 104.0, 84.0], image)
            .unwrap();
        assert_eq!(c.scale, 4);
        assert_eq!(c.origin.0, 104 * 4 + GAP * 4);
        assert_eq!(c.origin.1, 82 * 4 - 160);
        // Near the right edge: on its left.
        let c = CardLayout::beside((1280, 720), &g, None, 0, [300.0, 80.0, 304.0, 84.0], image)
            .unwrap();
        assert_eq!(c.origin.0 + 576 + GAP * 4, 300 * 4);
        // Near the top and bottom: clamped inside.
        let c =
            CardLayout::beside((1280, 720), &g, None, 0, [10.0, 0.0, 12.0, 2.0], image).unwrap();
        assert_eq!(c.origin.1, 0);
        let c = CardLayout::beside((1280, 720), &g, None, 0, [10.0, 178.0, 12.0, 180.0], image)
            .unwrap();
        assert_eq!(c.origin.1 + 320, 720);
        // Off the grid entirely: still inside.
        let c = CardLayout::beside(
            (1280, 720),
            &g,
            None,
            0,
            [-50.0, -50.0, -40.0, -40.0],
            image,
        )
        .unwrap();
        assert_eq!(c.origin, (0, 0));
    }

    #[test]
    fn the_card_sits_beside_the_anchor_through_a_camera_at_the_plain_scale() {
        let g = grid(); // 320×180 at scale 4 in 1280×720
        let image = (144, 80);
        let target = (1280, 720);
        let plain =
            CardLayout::beside(target, &g, None, 0, [100.0, 80.0, 104.0, 84.0], image).unwrap();
        // The full-tank view is the plain layout, to the pixel.
        let fit = ViewRect::fit(&g, target);
        let same =
            CardLayout::beside_view(target, &g, &fit, None, 0, [100.0, 80.0, 104.0, 84.0], image);
        assert_eq!(same, Some(plain));

        // Zoomed 2.5× on the anchor: it sits mid-screen at 20 pixels a cell.
        let view = ViewRect::cover(&g, target, (102.0, 82.0), 2.5);
        let zoomed = CardLayout::beside_view(
            target,
            &g,
            &view,
            None,
            0,
            [100.0, 80.0, 104.0, 84.0],
            image,
        )
        .unwrap();
        assert_eq!(zoomed.scale, plain.scale);
        let (right, _) = view.grid_to_screen(target, (104.0, 82.0));
        let gap = f64::from(GAP * zoomed.scale);
        assert!(
            (f64::from(zoomed.origin.0) - (right + gap)).abs() < 1.0,
            "{zoomed:?}"
        );
        let (_, mid) = view.grid_to_screen(target, (104.0, 82.0));
        let card_mid = f64::from(zoomed.origin.1) + f64::from(zoomed.size().1) / 2.0;
        assert!((card_mid - mid).abs() < 1.0);

        // An anchor out of view clamps the card inside the target.
        let far = CardLayout::beside_view(
            target,
            &g,
            &view,
            None,
            0,
            [300.0, 170.0, 304.0, 174.0],
            image,
        )
        .unwrap();
        let (w, h) = far.size();
        assert!(
            far.origin.0 + w <= 1280 && far.origin.1 + h <= 720,
            "{far:?}"
        );
    }

    #[test]
    fn the_card_stays_inside_a_letterboxed_tank() {
        // A 320×180 grid at scale 2 in 1000×600: offset (180, 120).
        let g = Layout::fit((320, 180), (1000, 600));
        assert_eq!((g.scale, g.offset), (2, (180, 120)));
        let c =
            CardLayout::beside((1000, 600), &g, None, 0, [0.0, 0.0, 1.0, 1.0], (144, 80)).unwrap();
        assert_eq!(c.scale, 2);
        assert!(c.origin.0 >= 180 && c.origin.1 >= 120, "{c:?}");
        let (w, h) = c.size();
        assert!(c.origin.0 + w <= 820 && c.origin.1 + h <= 480);
    }

    #[test]
    fn a_small_target_shrinks_the_card_or_hides_it() {
        let g = Layout::fit((320, 180), (512, 288));
        let c = CardLayout::beside(
            (512, 288),
            &g,
            None,
            0,
            [10.0, 10.0, 11.0, 11.0],
            (144, 100),
        )
        .unwrap();
        assert_eq!(c.scale, 1);
        let g = Layout::fit((320, 180), (100, 60));
        assert_eq!(
            CardLayout::beside((100, 60), &g, None, 0, [1.0, 1.0, 2.0, 2.0], (144, 100)),
            None
        );
        assert_eq!(
            CardLayout::beside((1280, 720), &grid(), None, 0, [0.0; 4], (0, 0)),
            None
        );
    }

    /// A school of three neon tetras (the species with needs) in a tank:
    /// the first, `a`, is the one inspected.
    fn school() -> (World, [CreatureId; 3]) {
        let mut w = world();
        let tetra = w.species().id("neon_tetra").unwrap();
        let ids = [(80, 50), (84, 50), (88, 50)].map(|(x, y)| w.add_creature(tetra, x, y).unwrap());
        (w, ids)
    }

    /// The fill and trend of each of `card`'s bars.
    fn bars(card: &InspectCard) -> Vec<(u8, i8)> {
        card.live
            .as_ref()
            .unwrap()
            .needs
            .iter()
            .map(|b| (b.fill, b.trend))
            .collect()
    }

    /// Ticks `w` `n` times, refreshing `card` each tick as a tank does.
    fn run(w: &mut World, card: &mut InspectCard, n: u32) {
        for _ in 0..n {
            w.tick();
            card.refresh(w, false);
        }
    }

    #[test]
    fn a_creature_with_needs_shows_one_labelled_bar_per_need_and_others_none() {
        let (w, [a, ..]) = school();
        let tetra = live(&w, Inspected::Creature(a)).unwrap();
        let labels: Vec<_> = tetra.needs.iter().map(|b| b.label).collect();
        assert_eq!(labels, ["Hunger", "Social"]);
        // The bars are the simulation's own per-creature readings.
        let readings = w.creature_needs(w.creature(a).unwrap());
        assert_eq!(readings.len(), 2);
        for (bar, reading) in tetra.needs.iter().zip(&readings) {
            assert_eq!(*bar, NeedBar::of(*reading));
        }
        let (with, _) = build_live(&tetra, true);
        let mut plain = tetra.clone();
        plain.needs.clear();
        let (without, _) = build_live(&plain, true);
        assert!(with.size.1 > without.size.1 + font::LINE);
        assert!(!with.clipped);
        // A species without supported needs, a plant, a cell and a device
        // have no Needs section.
        let mut w = w;
        let guppy = w.species().id("guppy").unwrap();
        let g = w.add_creature(guppy, 80, 40).unwrap();
        assert_eq!(live(&w, Inspected::Creature(g)).unwrap().needs, []);
        assert_eq!(
            live(&w, Inspected::Cell { x: 10, y: 60 }).unwrap().needs,
            []
        );
        // The Needs heading is the section's only mark when it is empty:
        // no bars, no heading.
        let (guppy_page, _) = build_live(&live(&w, Inspected::Creature(g)).unwrap(), true);
        assert!(guppy_page.size.1 < with.size.1);
    }

    #[test]
    fn eating_raises_the_feeding_bar_and_a_fish_back_with_its_school_restores_social() {
        let (mut w, [a, ..]) = school();
        let mut card = InspectCard::default();
        card.set(&w, Some(Inspected::Creature(a)));
        // A hungry tetra among companions: empty feeding bar, full social
        // (it takes a moment to look around), both long steady.
        let at = |card: &InspectCard, i: usize| bars(card)[i];
        run(&mut w, &mut card, 250);
        assert_eq!(bars(&card), [(0, 0), (needs::LEVELS, 0)]);
        let food = w.materials().id("flake_food").unwrap();
        let (x, y) = w.creature(a).unwrap().centre();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        w.set_cell(x as usize, y as usize, food).unwrap();
        run(&mut w, &mut card, 3);
        let (fill, trend) = at(&card, 0);
        assert!(fill > 0, "eating raised the bar");
        assert!(trend > 0, "and it is improving: {trend}");
        assert_eq!(at(&card, 1), (50, 0), "the other need did not move");
        assert_eq!(
            card.live.as_ref().unwrap().needs[0].level,
            desktank_sim::NeedLevel::Critical,
            "a first bite is still critically unmet"
        );
        // Five seconds on it is steady again, only slowly falling.
        run(&mut w, &mut card, 110);
        assert_eq!(at(&card, 0).1, 0);
        assert!(at(&card, 0).0 > 0);

        // Away from its companions the social bar empties and falls ...
        assert!(w.move_creature(a, 10, 80));
        run(&mut w, &mut card, 40);
        assert_eq!(at(&card, 1), (0, -3));
        // ... and as it swims back to its school the social bar fills and
        // rises again.
        let back = (0..600).find_map(|_| {
            run(&mut w, &mut card, 1);
            Some(at(&card, 1)).filter(|&(fill, trend)| fill > 0 && trend > 0)
        });
        assert!(back.is_some(), "it rejoins its school");
    }

    #[test]
    fn switching_fish_shows_the_new_individuals_values_at_once() {
        let (mut w, [a, b, third]) = school();
        let mut card = InspectCard::default();
        card.set(&w, Some(Inspected::Creature(a)));
        run(&mut w, &mut card, 200);
        // Feed only `b`, so the three differ.
        let food = w.materials().id("flake_food").unwrap();
        let (bx, by) = w.creature(b).unwrap().centre();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        w.set_cell(bx as usize, by as usize, food).unwrap();
        w.tick();
        w.tick();
        card.refresh(&w, false);
        let a_bars = bars(&card);
        // Inspecting `b` shows b's meal straight away, with no trend yet (a
        // trend is read from a creature's own history), never a's bars.
        assert!(card.set(&w, Some(Inspected::Creature(b))));
        let b_bars = bars(&card);
        let b_fed = w.creature_needs(w.creature(b).unwrap())[0].satisfaction;
        assert!(b_fed > 0);
        assert_eq!(b_bars[0], (b_fed.div_ceil(2), 0));
        assert_ne!(b_bars, a_bars);
        // The image matches what building it afresh gives.
        let rebuilt = build_live(card.live.as_ref().unwrap(), true).0;
        assert_eq!(card.image.as_ref(), Some(&rebuilt));
        // Going back to `a`, and to `c`, shows their readings, not b's.
        for id in [a, third] {
            assert!(card.set(&w, Some(Inspected::Creature(id))));
            let fed = w.creature_needs(w.creature(id).unwrap())[0].satisfaction;
            assert_eq!(bars(&card)[0], (fed.div_ceil(2), 0), "{id:?}");
        }
    }

    #[test]
    fn the_card_is_rebuilt_only_when_a_meter_moves_and_not_at_all_when_closed() {
        let (mut w, [a, ..]) = school();
        let mut card = InspectCard::default();
        // Closed: nothing is remembered, built or asked for.
        run(&mut w, &mut card, 50);
        assert!(card.image.is_none() && !card.dirty && card.trends.is_empty());
        assert!(!card.refresh(&w, false));

        card.set(&w, Some(Inspected::Creature(a)));
        card.dirty = false;
        let (mut rebuilds, mut previous) = (0, card.image.clone());
        for _ in 0..600 {
            w.tick();
            if card.refresh(&w, false) {
                rebuilds += 1;
                assert!(card.dirty);
                assert_ne!(card.image, previous, "rebuilt only for a change");
            } else {
                assert!(!card.dirty || card.image != previous);
                assert_eq!(card.image, previous, "no change, no rebuild");
            }
            card.dirty = false;
            previous.clone_from(&card.image);
        }
        // Thirty seconds of a settled fish rebuild well under once a second
        // (energy, hunger and the school each step now and then).
        assert!(rebuilds < 40, "{rebuilds} rebuilds in 600 ticks");

        // Closing forgets the history and does no more work.
        assert!(card.set(&w, None));
        assert!(card.image.is_none() && card.trends.is_empty());
        card.dirty = false;
        run(&mut w, &mut card, 100);
        assert!(card.image.is_none() && !card.dirty && card.trends.is_empty());
        // A creature that is gone closes the card.
        card.set(&w, Some(Inspected::Creature(a)));
        assert!(w.remove_creature(a));
        assert!(card.refresh(&w, false));
        assert!(card.what.is_none() && card.image.is_none());
    }

    #[test]
    fn a_narrow_card_stacks_the_needs_in_one_column() {
        let (w, [a, ..]) = school();
        let mut card = InspectCard::default();
        card.set(&w, Some(Inspected::Creature(a)));
        let wide = card.image.clone().unwrap();
        // Setting the same layout again is free; changing it rebuilds once.
        assert!(!card.set_narrow(&w, false));
        card.dirty = false;
        assert!(card.set_narrow(&w, true));
        assert!(card.dirty);
        let narrow = card.image.clone().unwrap();
        assert!(narrow.size.1 > wide.size.1, "one column is taller");
        assert!(!narrow.clipped && !wide.clipped);
        assert_eq!(narrow.size.0, wide.size.0, "the card is as wide");
        // It survives the species page and back, and a new creature.
        assert!(card.set_page(&w, InspectPage::Species));
        assert!(card.set_page(&w, InspectPage::Live));
        assert_eq!(card.image.as_ref(), Some(&narrow));
        assert!(card.set_narrow(&w, false));
        assert_eq!(card.image.as_ref(), Some(&wide));
        // With nothing inspected it is just remembered.
        assert!(card.set(&w, None));
        assert!(!card.set_narrow(&w, true));
        assert!(card.image.is_none());
        card.set(&w, Some(Inspected::Creature(a)));
        assert_eq!(card.image.as_ref(), Some(&narrow));
    }

    #[test]
    fn naming_and_the_species_page_still_work_on_a_card_with_needs() {
        let (w, [a, ..]) = school();
        let g = grid();
        let target = (1280, 720);
        let mut card = InspectCard::default();
        card.set(&w, Some(Inspected::Creature(a)));
        assert!(card.set_can_name(&w, true));
        let at = card.layout(&w, target, &g, None, None, 0).unwrap();
        let rect = card.name_rect(&at).expect("the Name row can be clicked");
        // The row is above the Needs section and inside the card.
        let [_, top, _, height] = card.name_row.unwrap();
        let needs_top =
            card.image.as_ref().unwrap().size.1 - 4 - (font::LINE + 1 + font::LINE + 7) - 3;
        assert!(top + height <= needs_top, "{top} {height} {needs_top}");
        assert!(rect.contains(f64::from(rect.origin.0 + 1), f64::from(rect.origin.1 + 1)));
        // Page 1/2 to 2/2 and back.
        let live_page = card.image.clone();
        assert!(card.set_page(&w, InspectPage::Species));
        assert_eq!(card.page.tag(), "2/2");
        assert_ne!(card.image, live_page);
        assert!(
            card.layout(&w, target, &g, None, None, 0)
                .and_then(|at| card.name_rect(&at))
                .is_none()
        );
        assert!(card.set_page(&w, InspectPage::Live));
        assert_eq!(card.image, live_page);
        assert_eq!(card.page.tag(), "1/2");
        // Typing a name keeps the bars and the card un-clipped.
        assert!(card.set_name_edit(&w, Some("Neo")));
        let editing = card.image.as_ref().unwrap();
        assert!(!editing.clipped);
        assert_eq!(bars(&card).len(), 2);
    }

    #[test]
    fn the_card_with_needs_fits_small_and_large_targets() {
        let (w, [a, ..]) = school();
        let mut card = InspectCard::default();
        card.set(&w, Some(Inspected::Creature(a)));
        for narrow in [false, true] {
            card.set_narrow(&w, narrow);
            let size = card.image.as_ref().unwrap().size;
            assert!(size.1 <= card::CARD_MAX.1, "{narrow}: {size:?}");
            // A phone-sized target still fits it at scale 1, a desktop one
            // draws it larger.
            for (target, scale_at_least) in [((360, 640), 1), ((1280, 720), 2)] {
                let g = Layout::fit((320, 180), target);
                let at = card
                    .layout(&w, target, &g, None, None, 0)
                    .unwrap_or_else(|| panic!("{narrow} {target:?}"));
                assert!(at.scale >= scale_at_least, "{target:?}: {at:?}");
                let (cw, ch) = at.size();
                assert!(at.origin.0 + cw <= target.0 && at.origin.1 + ch <= target.1);
            }
        }
        // Even a 320×180 canvas (a TV safe area, an embed) fits it once.
        let g = Layout::fit((160, 90), (320, 180));
        card.set_narrow(&w, false);
        assert!(card.layout(&w, (320, 180), &g, None, None, 0).is_some());
    }

    // ---- Plants: light, CO₂ and nutrient meters. ----

    /// Four ludwigia 40 columns apart in a tank with a bed, grown for a
    /// minute and left at their own conditions: `[open, shaded, ...]`, left
    /// to right. Nothing is dosed, so CO₂ holds each back.
    fn garden() -> (World, [PlantId; 4]) {
        let mut w = world();
        let soil = w.materials().id("aquasoil").unwrap();
        w.fill_rect(
            desktank_sim::Rect {
                x0: 0,
                y0: 84,
                x1: 160,
                y1: 90,
            },
            soil,
            desktank_sim::Brush::Replace,
        )
        .unwrap();
        let ludwigia = w.plant_species().id("ludwigia").unwrap();
        for x in [20, 60, 100, 140] {
            w.paint_plant(x, 60, 0, ludwigia, desktank_sim::Propagule::Cuttings)
                .unwrap();
        }
        for _ in 0..1200 {
            w.tick();
        }
        let mut plants: Vec<_> = w.plants().collect();
        plants.sort_by_key(|p| p.base.0);
        let ids: Vec<_> = plants.iter().map(|p| p.id).collect();
        (w, ids.try_into().expect("four plants rooted"))
    }

    /// Adds `amount` of `kind` to every field cell around plant `id`, as
    /// many times as it takes to be seen.
    fn pour(w: &mut World, id: PlantId, kind: desktank_sim::FieldKind, amount: f32) {
        let p = w.plant(id).unwrap();
        for y in p.bounds.y0.saturating_sub(6)..p.bounds.y1 + 6 {
            for x in p.bounds.x0.saturating_sub(10)..p.bounds.x1 + 10 {
                w.add_to_field(kind, x, y, amount);
            }
        }
    }

    /// A plant's reading of `need`.
    fn reading(w: &World, id: PlantId, need: desktank_sim::Need) -> u8 {
        w.plant_needs(id)
            .unwrap()
            .iter()
            .find(|r| r.need == need)
            .unwrap()
            .satisfaction
    }

    /// Doses CO₂ around `id` until its CO₂ reading reaches `to`.
    fn dose_co2(w: &mut World, id: PlantId, to: u8) {
        dose(
            w,
            id,
            desktank_sim::FieldKind::Co2,
            desktank_sim::Need::Co2,
            to,
        );
    }

    /// Pours `kind` around `id` a little at a time until its `need` reading
    /// reaches `to`.
    fn dose(
        w: &mut World,
        id: PlantId,
        kind: desktank_sim::FieldKind,
        need: desktank_sim::Need,
        to: u8,
    ) {
        for _ in 0..200 {
            if reading(w, id, need) >= to {
                return;
            }
            pour(w, id, kind, 0.002);
        }
        panic!("{need:?} never reached {to}");
    }

    /// A rock lid glued over columns `x0..x1` at the surface.
    fn shade(w: &mut World, x0: usize, x1: usize) {
        let rock = w.materials().id("rock").unwrap();
        w.fill_rect(
            desktank_sim::Rect {
                x0,
                y0: 8,
                x1,
                y1: 13,
            },
            rock,
            desktank_sim::Brush::Replace,
        )
        .unwrap();
        let body = w.body_at(x0, 8).unwrap();
        assert!(w.set_anchored(body, true));
    }

    /// The four conditions of the issue, one per plant of a garden:
    /// thriving, light-limited, CO₂-limited and nutrient-limited. With
    /// `settle` ticks the plants first update under them (every five
    /// seconds), so their Growth lines name what limits them now.
    fn conditions(settle: u32) -> (World, [PlantId; 4]) {
        let (mut w, [thriving, shaded, co2, starved]) = garden();
        shade(&mut w, 44, 80);
        for round in 0..2 {
            if round == 1 {
                for _ in 0..settle {
                    w.tick();
                }
            }
            dose_co2(&mut w, thriving, 95);
            dose_co2(&mut w, shaded, 95);
            // The CO₂-limited one is left as it is.
            dose_co2(&mut w, starved, 95);
            pour(&mut w, starved, desktank_sim::FieldKind::Nutrient, -50.0);
        }
        for _ in 0..5 {
            w.tick();
        }
        (w, [thriving, shaded, co2, starved])
    }

    fn bar_fills(card: &InspectCard) -> Vec<u8> {
        card.live
            .as_ref()
            .unwrap()
            .needs
            .iter()
            .map(|b| b.fill)
            .collect()
    }

    #[test]
    fn a_plant_live_page_has_light_co2_and_nutrient_bars_beside_its_growth() {
        let (w, [a, ..]) = garden();
        let live = live(&w, Inspected::Plant(a)).unwrap();
        let labels: Vec<_> = live.needs.iter().map(|b| b.label).collect();
        assert_eq!(labels, ["Light", "CO₂", "Nutrients"]);
        // The bars are the simulation's own per-plant readings.
        let readings = w.plant_needs(a).unwrap();
        for (bar, reading) in live.needs.iter().zip(readings) {
            assert_eq!(*bar, NeedBar::of(reading));
        }
        // The Growth line is still there, and the card fits in both
        // layouts with the Needs section in.
        assert!(live.fields.iter().any(|(label, _)| *label == "Growth:"));
        let mut plain = live.clone();
        plain.needs.clear();
        let without = build_live(&plain, true).0;
        for narrow in [false, true] {
            let mut live = live.clone();
            live.narrow = narrow;
            let (image, row) = build_live(&live, true);
            assert!(!image.clipped, "narrow {narrow}");
            assert!(
                image.size.1 <= card::CARD_MAX.1,
                "{narrow}: {:?}",
                image.size
            );
            assert!(image.size.1 > without.size.1 + font::LINE);
            assert_eq!(row, None, "plants have no name row");
        }
        // Two rows of meters in two columns, three in one.
        let rows = |narrow| {
            let mut live = live.clone();
            live.narrow = narrow;
            build_live(&live, true).0.size.1
        };
        assert!(rows(true) > rows(false));
    }

    #[test]
    fn thriving_and_limited_plants_read_differently_in_each_factor() {
        use desktank_sim::Need;
        let (w, [thriving, shaded, co2, starved]) = conditions(0);
        let fills = |id| {
            live(&w, Inspected::Plant(id))
                .unwrap()
                .needs
                .iter()
                .map(|b| (b.label, b.fill, b.level))
                .collect::<Vec<_>>()
        };
        let (ok, dim, flat, hungry) = (fills(thriving), fills(shaded), fills(co2), fills(starved));
        // Thriving: every bar comfortable.
        assert!(
            ok.iter().all(|b| b.2 >= desktank_sim::NeedLevel::Middling),
            "{ok:?}"
        );
        // Each limited plant is short in exactly its own factor.
        assert!(dim[0].1 + 10 < ok[0].1, "light: {dim:?} vs {ok:?}");
        assert!(flat[1].1 + 10 < ok[1].1, "CO₂: {flat:?} vs {ok:?}");
        assert!(
            hungry[2].1 + 10 < ok[2].1,
            "nutrients: {hungry:?} vs {ok:?}"
        );
        assert!(dim[1].1 > dim[0].1 && hungry[1].1 > hungry[2].1);
        assert!(reading(&w, co2, Need::Co2) < 75);
        // Each read from the card is the simulation's reading.
        for id in [thriving, shaded, co2, starved] {
            let readings = w.plant_needs(id).unwrap();
            let bars = live(&w, Inspected::Plant(id)).unwrap().needs;
            assert_eq!(
                bars,
                readings.iter().map(|&r| NeedBar::of(r)).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn changing_tank_conditions_moves_a_plants_bars_and_trends() {
        use desktank_sim::{FieldKind, Need};
        let (mut w, [a, ..]) = garden();
        let mut card = InspectCard::default();
        card.set(&w, Some(Inspected::Plant(a)));
        run(&mut w, &mut card, 12);
        // Steady conditions, no trend.
        let steady = bars(&card);
        assert!(steady.iter().all(|&(_, trend)| trend == 0), "{steady:?}");
        let before = bar_fills(&card);
        // Strip the nutrients: the nutrient bar falls, its trend points
        // down, and the light and CO₂ bars stay put.
        pour(&mut w, a, FieldKind::Nutrient, -50.0);
        run(&mut w, &mut card, 6);
        let after = bars(&card);
        assert!(after[2].0 < before[2], "{before:?} -> {after:?}");
        assert!(after[2].1 < 0, "falling: {after:?}");
        assert_eq!(after[0].1, 0);
        assert_eq!(after[1].1, 0);
        assert_eq!(reading(&w, a, Need::Light).div_ceil(2), after[0].0);
        // Feed them back: it rises again, and the trend turns.
        dose(&mut w, a, FieldKind::Nutrient, Need::Nutrients, 90);
        run(&mut w, &mut card, 6);
        let fed = bars(&card);
        assert!(fed[2].0 > after[2].0, "{after:?} -> {fed:?}");
        // Dim the light: the light bar falls with a falling trend.
        let p = w.plant(a).unwrap();
        shade(&mut w, p.bounds.x0.saturating_sub(10), p.bounds.x1 + 10);
        run(&mut w, &mut card, 8);
        let dim = bars(&card);
        assert!(dim[0].0 + 4 < fed[0].0, "{fed:?} -> {dim:?}");
        assert!(dim[0].1 < 0, "falling: {dim:?}");
        // A changed bar is a changed image, built from the live page.
        let rebuilt = build_live(card.live.as_ref().unwrap(), true).0;
        assert_eq!(card.image.as_ref(), Some(&rebuilt));
    }

    #[test]
    fn a_plants_shimmering_reading_is_held_steady_but_real_moves_and_the_ends_show() {
        use desktank_sim::Need;
        let at = |l, c, n| {
            [
                NeedReading {
                    need: Need::Light,
                    satisfaction: l,
                },
                NeedReading {
                    need: Need::Co2,
                    satisfaction: c,
                },
                NeedReading {
                    need: Need::Nutrients,
                    satisfaction: n,
                },
            ]
        };
        let shown = |mut r: [NeedReading; 3], card: &mut InspectCard| {
            card.hold(&mut r);
            r.map(|r| r.satisfaction)
        };
        let mut card = InspectCard::default();
        // The first reading is shown as it is.
        assert_eq!(shown(at(60, 50, 76), &mut card), [60, 50, 76]);
        // A point or two either way is the field shimmering: held.
        assert_eq!(shown(at(61, 48, 77), &mut card), [60, 50, 76]);
        assert_eq!(shown(at(58, 52, 75), &mut card), [60, 50, 76]);
        // Three points is a move, and is held from there.
        assert_eq!(shown(at(63, 50, 73), &mut card), [63, 50, 73]);
        assert_eq!(shown(at(64, 50, 72), &mut card), [63, 50, 73]);
        // Full and empty show at once, and stay while the reading is within
        // reach of them.
        assert_eq!(shown(at(100, 0, 74), &mut card), [100, 0, 73]);
        assert_eq!(shown(at(99, 1, 74), &mut card), [100, 0, 73]);
        assert_eq!(shown(at(97, 3, 74), &mut card), [97, 3, 73]);
        // A new subject starts from nothing.
        card.held.clear();
        assert_eq!(shown(at(1, 2, 3), &mut card), [1, 2, 3]);
    }

    #[test]
    fn switching_between_plants_shows_each_individuals_values_at_once() {
        let (w, [thriving, shaded, co2, starved]) = conditions(0);
        let mut card = InspectCard::default();
        card.set(&w, Some(Inspected::Plant(thriving)));
        let t_bars = bars(&card);
        let mut seen = vec![t_bars.clone()];
        for id in [shaded, co2, starved, thriving] {
            assert!(card.set(&w, Some(Inspected::Plant(id))));
            let now = bars(&card);
            // Straight away this plant's own readings, no trend carried
            // from the last one.
            let own: Vec<_> = w
                .plant_needs(id)
                .unwrap()
                .iter()
                .map(|r| (r.satisfaction.div_ceil(2), 0))
                .collect();
            assert_eq!(now, own, "{id:?}");
            let rebuilt = build_live(card.live.as_ref().unwrap(), true).0;
            assert_eq!(card.image.as_ref(), Some(&rebuilt));
            seen.push(now);
        }
        assert_eq!(seen[0], seen[4], "back to the first, the same bars");
        assert!(
            seen[..4]
                .iter()
                .enumerate()
                .all(|(i, a)| { seen[..4].iter().skip(i + 1).all(|b| a != b) }),
            "four plants, four different sets of bars: {seen:?}"
        );
    }

    #[test]
    fn the_plant_card_is_rebuilt_only_when_a_meter_moves_and_not_at_all_when_closed() {
        let (mut w, [a, ..]) = garden();
        let mut card = InspectCard::default();
        run(&mut w, &mut card, 50);
        assert!(card.image.is_none() && !card.dirty && card.trends.is_empty());
        assert!(!card.refresh(&w, false));

        card.set(&w, Some(Inspected::Plant(a)));
        card.dirty = false;
        let (mut rebuilds, mut previous) = (0, card.image.clone());
        for _ in 0..600 {
            w.tick();
            if card.refresh(&w, false) {
                rebuilds += 1;
                assert!(card.dirty);
                assert_ne!(card.image, previous, "rebuilt only for a change");
            } else {
                assert_eq!(card.image, previous, "no change, no rebuild");
                assert!(!card.dirty);
            }
            card.dirty = false;
            previous.clone_from(&card.image);
        }
        // A settled plant rarely moves a meter: well under once a second.
        assert!(rebuilds < 30, "{rebuilds} rebuilds in 600 ticks");
        // Closing forgets the history and does no more work.
        assert!(card.set(&w, None));
        assert!(card.image.is_none() && card.trends.is_empty());
        card.dirty = false;
        run(&mut w, &mut card, 100);
        assert!(card.image.is_none() && !card.dirty && card.trends.is_empty());
    }

    #[test]
    fn a_narrow_plant_card_stacks_the_bars_and_the_species_page_still_turns() {
        let (w, [a, ..]) = garden();
        let mut card = InspectCard::default();
        card.set(&w, Some(Inspected::Plant(a)));
        let wide = card.image.clone().unwrap();
        card.dirty = false;
        assert!(card.set_narrow(&w, true));
        assert!(card.dirty);
        let narrow = card.image.clone().unwrap();
        assert!(narrow.size.1 > wide.size.1, "one column is taller");
        assert!(!narrow.clipped && !wide.clipped);
        assert!(narrow.size.1 <= card::CARD_MAX.1);
        assert_eq!(bars(&card).len(), 3);
        // 1/2 and 2/2 both ways, the bars back as they were.
        assert!(card.set_page(&w, InspectPage::Species));
        assert_eq!(card.page.tag(), "2/2");
        assert_ne!(card.image.as_ref(), Some(&narrow));
        assert!(card.set_page(&w, InspectPage::Live));
        assert_eq!(card.page.tag(), "1/2");
        assert_eq!(card.image.as_ref(), Some(&narrow));
        // Turning to the species card does not drop the history.
        assert!(card.set_narrow(&w, false));
        assert_eq!(card.image.as_ref(), Some(&wide));
        // The card fits a phone and a desktop target at scale 1 and up.
        for narrow in [false, true] {
            card.set_narrow(&w, narrow);
            let g = Layout::fit((320, 180), (360, 640));
            assert!(card.layout(&w, (360, 640), &g, None, 0).is_some());
        }
        // A plant that is gone closes the card.
        let mut w = w;
        let p = w.plant(a).unwrap();
        w.fill_rect(
            p.bounds,
            w.materials().id("water").unwrap(),
            desktank_sim::Brush::Replace,
        )
        .unwrap();
        w.tick();
        assert!(card.refresh(&w, false));
        assert!(card.what.is_none() && card.image.is_none());
    }

    /// Writes the plant examples of the issue as PNGs to
    /// `target/shots/plant-inspect-need-bars/` when `INSPECT_SHOTS` is set:
    /// the four conditions side by side, then one card in two columns and in
    /// one.
    #[test]
    fn writes_plant_card_shots_on_request() {
        if std::env::var_os("INSPECT_SHOTS").is_none() {
            return;
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/shots/plant-inspect-need-bars");
        std::fs::create_dir_all(&dir).unwrap();
        let (mut w, ids) = conditions(110);
        let write = |name: &str, images: &[CardImage], scale: u32| {
            let gap = 6;
            let total_w: u32 = images.iter().map(|i| i.size.0).sum::<u32>()
                + gap * u32::try_from(images.len() - 1).unwrap();
            let height = images.iter().map(|i| i.size.1).max().unwrap();
            let mut sheet = vec![0_u8; (total_w * height * 4) as usize];
            let mut x0 = 0;
            for image in images {
                for y in 0..image.size.1 {
                    for x in 0..image.size.0 {
                        let i = ((y * image.size.0 + x) * 4) as usize;
                        let o = ((y * total_w + x0 + x) * 4) as usize;
                        sheet[o..o + 4].copy_from_slice(&image.pixels[i..i + 4]);
                    }
                }
                x0 += image.size.0 + gap;
            }
            std::fs::write(
                dir.join(name),
                crate::testing::png(
                    &crate::needs::tests::enlarge(&sheet, (total_w, height), scale),
                    (total_w * scale, height * scale),
                ),
            )
            .unwrap();
        };
        let card_of = |w: &World, id: PlantId, narrow: bool| {
            let mut card = InspectCard::default();
            card.set_narrow(w, narrow);
            card.set(w, Some(Inspected::Plant(id)));
            card.image.unwrap()
        };
        // Thriving, light-, CO₂- and nutrient-limited, two columns.
        let cards: Vec<_> = ids.iter().map(|&id| card_of(&w, id, false)).collect();
        write("thriving-and-limited-two-column.png", &cards, 4);
        let cards: Vec<_> = ids.iter().map(|&id| card_of(&w, id, true)).collect();
        write("thriving-and-limited-narrow.png", &cards, 4);
        // Desktop two-column and narrow single-column, with trends: the
        // CO₂-limited plant just dosed, its nutrients stripped meanwhile.
        let mut card = InspectCard::default();
        card.set(&w, Some(Inspected::Plant(ids[2])));
        run(&mut w, &mut card, 10);
        dose_co2(&mut w, ids[2], 95);
        pour(&mut w, ids[2], desktank_sim::FieldKind::Nutrient, -50.0);
        run(&mut w, &mut card, 6);
        let wide = card.image.clone().unwrap();
        card.set_narrow(&w, true);
        let narrow = card.image.clone().unwrap();
        write("trends-desktop-and-narrow.png", &[wide, narrow], 5);
    }

    /// Writes a tetra's live card, just after a first bite, in two columns
    /// and in one, as PNGs to `target/shots/inspect-need-bars/` when
    /// `INSPECT_SHOTS` is set.
    #[test]
    fn writes_live_card_shots_on_request() {
        if std::env::var_os("INSPECT_SHOTS").is_none() {
            return;
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/shots/inspect-need-bars");
        std::fs::create_dir_all(&dir).unwrap();
        let (mut w, [a, ..]) = school();
        w.set_creature_name(a, "Neo");
        let mut card = InspectCard::default();
        card.set(&w, Some(Inspected::Creature(a)));
        run(&mut w, &mut card, 250);
        let food = w.materials().id("flake_food").unwrap();
        let (x, y) = w.creature(a).unwrap().centre();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        w.set_cell(x as usize, y as usize, food).unwrap();
        run(&mut w, &mut card, 3);
        for (name, narrow) in [("live-two-column", false), ("live-narrow", true)] {
            card.set_narrow(&w, narrow);
            let image = card.image.as_ref().unwrap();
            let scale = 5;
            std::fs::write(
                dir.join(format!("{name}.png")),
                crate::testing::png(
                    &crate::needs::tests::enlarge(&image.pixels, image.size, scale),
                    (image.size.0 * scale, image.size.1 * scale),
                ),
            )
            .unwrap();
        }
    }
}
