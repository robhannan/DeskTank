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
//! The live page is rebuilt only when its text changes (energy moves a
//! percent, a day passes, a letter is typed), never per frame; while nothing
//! is inspected nothing is built, uploaded or drawn.

use desktank_sim::tools::Inspection;
use desktank_sim::{
    CreatureId, DeviceId, MAX_NAME_CHARS, Mount, MovementClass, PlantId, UnmetNeed, World,
};

use crate::card::{
    self, CARD_MAX, CARD_W, Card, CardImage, CardLayout, Page, card_scale, display_name,
};
use crate::hud::PaletteLayout;
use crate::layout::Layout;

/// The row of the HUD texture where the inspect card's region starts (below
/// the species card's; its left edge is column 0).
pub const TEXTURE_Y: u32 = card::TEXTURE_Y + CARD_MAX.1;

/// Target pixels between the inspected thing and the card, per card pixel.
const GAP: u32 = 3;

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
        self.refresh(world, true);
        true
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
        let Some(mut live) = live(world, what) else {
            *self = Self {
                can_name: self.can_name,
                ..Self::default()
            };
            return true;
        };
        if let Some(row) = &mut live.name {
            row.can_name = self.can_name;
            row.editing.clone_from(&self.edit);
        }
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

    /// Where it is drawn in a `target`-pixel surface laid out as `grid`.
    pub(crate) fn layout(
        &self,
        world: &World,
        target: (u32, u32),
        grid: &Layout,
        palette: Option<&PaletteLayout>,
        min_scale: u32,
    ) -> Option<CardLayout> {
        let image = self.image.as_ref()?;
        CardLayout::beside(
            target,
            grid,
            palette,
            min_scale,
            anchor(world, self.what?)?,
            image.size,
        )
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

/// The live page of `what`, or `None` if it is gone (died, netted out,
/// eaten away) or its species is unknown.
pub(crate) fn live(world: &World, what: Inspected) -> Option<Live> {
    match what {
        Inspected::Creature(id) => creature_live(world, id),
        Inspected::Plant(id) => plant_live(world, id),
        Inspected::Device(id) => device_live(world, id),
        Inspected::Cell { x, y } => cell_live(world, x, y),
    }
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

fn creature_live(world: &World, id: CreatureId) -> Option<Live> {
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
            fields.push(("Needs:", format!("{needs} cells of water")));
        }
        Some(UnmetNeed::Air) => fields.push(("Needs:", "air at the surface".to_owned())),
        None => {}
    }
    Some(Live {
        title: kind,
        subtitle: String::new(),
        fields,
        name: Some(NameRow {
            name: card.name,
            ..NameRow::default()
        }),
    })
}

fn plant_live(world: &World, id: PlantId) -> Option<Live> {
    let p = world.plant(id)?;
    let sp = world.plant_species().get(p.species)?;
    let [roots, stem, leaves] = p.parts;
    Some(Live {
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
    })
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
        if image.0 == 0 || image.1 == 0 || target.0 == 0 || target.1 == 0 {
            return None;
        }
        let cell = i64::from(grid.scale);
        let (target_w, target_h) = (i64::from(target.0), i64::from(target.1));
        // The tank's area in the target: left, top, right, bottom.
        let tank = [
            grid.offset.0.max(0),
            grid.offset.1.max(0),
            (grid.offset.0 + i64::from(grid.grid.0) * cell).min(target_w),
            (grid.offset.1 + i64::from(grid.grid.1) * cell).min(target_h),
        ];
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)] // pixels
        let px = |cells: f32, offset: i64| offset + (cells * grid.scale as f32).round() as i64;
        let (left_edge, top_edge) = (px(anchor[0], grid.offset.0), px(anchor[1], grid.offset.1));
        let (right_edge, bottom_edge) =
            (px(anchor[2], grid.offset.0), px(anchor[3], grid.offset.1));
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
            card.layout(&w, target, &g, None, 0)
                .and_then(|at| card.name_rect(&at)),
            None
        );
        assert!(card.set_can_name(&w, true));
        assert!(!card.set_can_name(&w, true));
        let at = card.layout(&w, target, &g, None, 0).unwrap();
        let rect = card
            .layout(&w, target, &g, None, 0)
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
            card.layout(&w, target, &g, None, 0)
                .and_then(|at| card.name_rect(&at)),
            None
        );
        // Editing returns to page one and edits are cut and cleaned.
        assert!(card.set_name_edit(&w, Some("A\tb")));
        assert_eq!(card.page, InspectPage::Live);
        assert_eq!(card.name_edit(), Some("Ab"));
        assert!(!card.set_name_edit(&w, Some("Ab")));
        assert!(
            card.layout(&w, target, &g, None, 0)
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
            card.layout(&w, target, &g, None, 0)
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
}
