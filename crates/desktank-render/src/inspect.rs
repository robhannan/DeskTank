//! The in-tank inspect card (SPEC §2.5): a two-page card pinned beside the
//! creature or plant last inspected, following it as it moves.
//!
//! Page 1 is what inspect found right now (its name, stage, age, health and
//! so on, from [`World::creature_card`] or [`World::plant`]); page 2 is its
//! species card ([`crate::card`]). Each page carries its number in the
//! top-right corner. The card is built on the CPU like the species card and
//! drawn from a third region of the HUD texture, so it needs nothing a host
//! provides beyond the tank: a screensaver or wallpaper can show it as well
//! as an interactive window.
//!
//! The live page is rebuilt only when its text changes (energy moves a
//! percent, a day passes), never per frame; while nothing is inspected
//! nothing is built, uploaded or drawn.

use desktank_sim::tools::Inspection;
use desktank_sim::{CreatureId, PlantId, UnmetNeed, World};

use crate::card::{self, CARD_MAX, Card, CardImage, CardLayout, Page, display_name};
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
}

impl Inspected {
    /// What an inspection pins the card to: the creature or plant found;
    /// `None` for a cell or a device.
    #[must_use]
    pub fn of(found: Inspection) -> Option<Self> {
        match found {
            Inspection::Creature { id, .. } => Some(Self::Creature(id)),
            Inspection::Plant { id, .. } => Some(Self::Plant(id)),
            Inspection::Cell { .. } | Inspection::Device { .. } => None,
        }
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
        self.refresh(world, true);
        true
    }

    /// Shows `page`; returns whether a frame is needed.
    pub(crate) fn set_page(&mut self, world: &World, page: InspectPage) -> bool {
        if self.page == page {
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
        let Some(live) = live(world, what) else {
            *self = Self::default();
            return true;
        };
        let changed = self.live.as_ref() != Some(&live);
        let rebuild = force || (changed && self.page == InspectPage::Live);
        if rebuild {
            self.image = build(world, what, self.page, &live);
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
    ) -> Option<CardLayout> {
        let image = self.image.as_ref()?;
        CardLayout::beside(target, grid, anchor(world, self.what?)?, image.size)
    }
}

/// The text of the live page: what changes it must rebuild the image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Live {
    title: String,
    subtitle: String,
    fields: Vec<(&'static str, String)>,
}

/// The live page of `what`, or `None` if it is gone (died, netted out,
/// eaten away) or its species is unknown.
pub(crate) fn live(world: &World, what: Inspected) -> Option<Live> {
    match what {
        Inspected::Creature(id) => creature_live(world, id),
        Inspected::Plant(id) => plant_live(world, id),
    }
}

fn percent(v: f32) -> String {
    format!("{:.0}%", (v * 100.0).clamp(0.0, 100.0))
}

fn creature_live(world: &World, id: CreatureId) -> Option<Live> {
    let card = world.creature_card(id)?;
    let species = world.creature(id)?.species();
    let kind = format!("{} #{}", display_name(&card.species), id.0);
    let (title, subtitle) = match card.name {
        Some(name) => (name, kind),
        None => (kind, String::new()),
    };
    let sex = if card.female { "female" } else { "male" };
    let mut fields = vec![
        ("Stage:", format!("{} {sex}", card.stage.name())),
        (
            "Age:",
            match card.age_days {
                0 => "under a day".to_owned(),
                1 => "1 day".to_owned(),
                d => format!("{d} days"),
            },
        ),
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
        title,
        subtitle,
        fields,
    })
}

fn plant_live(world: &World, id: PlantId) -> Option<Live> {
    let p = world.plant(id)?;
    let sp = world.plant_species().get(p.species)?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // ages ≥ 0
    let minutes = (p.age / 60.0) as u64;
    let age = match minutes {
        0 => "under a minute".to_owned(),
        m if m < 60 => format!("{m} min"),
        m => format!("{} h {} min", m / 60, m % 60),
    };
    let [roots, stem, leaves] = p.parts;
    Some(Live {
        title: format!("{} #{}", display_name(&sp.name), id.0),
        subtitle: String::new(),
        fields: vec![
            ("Age:", age),
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
    })
}

/// Draws the live page.
pub(crate) fn build_live(live: &Live) -> CardImage {
    let mut page = Page::new(Some(InspectPage::Live.tag()));
    page.heading(&live.title, &live.subtitle);
    for (label, text) in &live.fields {
        page.field(label, text);
    }
    page.finish()
}

/// Draws `page` of the card for `what`, given its live page; `None` if its
/// species is unknown.
pub(crate) fn build(
    world: &World,
    what: Inspected,
    page: InspectPage,
    live: &Live,
) -> Option<CardImage> {
    match page {
        InspectPage::Live => Some(build_live(live)),
        InspectPage::Species => {
            let species = match what {
                Inspected::Creature(id) => Card::Creature(world.creature(id)?.species()),
                Inspected::Plant(id) => Card::Plant(world.plant(id)?.species),
            };
            card::build_tagged(
                species,
                world.materials(),
                world.species(),
                world.plant_species(),
                Some(page.tag()),
            )
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
    }
}

impl CardLayout {
    /// Places an inspect card of `image` pixels beside `anchor` (cells, as
    /// [`anchor`] gives them) in `target`: to its right, or to its left if
    /// that has more room, vertically centred on it, then clamped inside
    /// the tank (the grid's area within `target`). Its scale is half the
    /// grid's cell scale (at least 2), dropped until it covers at most half
    /// the target each way and fits; `None` if it doesn't fit even at 1.
    #[must_use]
    pub fn beside(
        target: (u32, u32),
        grid: &Layout,
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
        let want = grid.scale.div_ceil(2).max(2);
        (1..=want).rev().find_map(|scale| {
            let width = i64::from(image.0) * i64::from(scale);
            let height = i64::from(image.1) * i64::from(scale);
            let small = scale == 1 || (2 * width <= target_w && 2 * height <= target_h);
            if !small || width > target_w || height > target_h {
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
        let one = build(&w, what, InspectPage::Live, &live).unwrap();
        let two = build(&w, what, InspectPage::Species, &live).unwrap();
        assert!(!one.clipped && !two.clipped);
        assert_eq!(one.size.0, card::CARD_W);
        assert_ne!(one, two);
        // Page 2 is the palette's species card with its page number.
        let plain = card::build(
            Card::Creature(guppy),
            w.materials(),
            w.species(),
            w.plant_species(),
        )
        .unwrap();
        assert_eq!(two.size.0, plain.size.0);
        assert_ne!(two.pixels, plain.pixels, "the tag is drawn");
        // A name becomes the title, the species the subtitle.
        w.set_creature_name(id, "Bubbles");
        let named = super::live(&w, what).unwrap();
        assert_eq!(
            (named.title.as_str(), named.subtitle.as_str()),
            ("Bubbles", live.title.as_str())
        );
        assert_ne!(named, live);
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
        assert_eq!(live.fields[0], ("Age:", "under a minute".to_owned()));
        assert!(build(&w, what, InspectPage::Species, &live).is_some());
        let a = anchor(&w, what).unwrap();
        assert!(a[0] <= 40.0 && a[2] > 40.0, "{a:?}");
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
        // Room on the right: just right of it, centred on it, at half the
        // cell scale.
        let c = CardLayout::beside((1280, 720), &g, [100.0, 80.0, 104.0, 84.0], image).unwrap();
        assert_eq!(c.scale, 2);
        assert_eq!(c.origin.0, 104 * 4 + GAP * 2);
        assert_eq!(c.origin.1, 82 * 4 - 80);
        // Near the right edge: on its left.
        let c = CardLayout::beside((1280, 720), &g, [300.0, 80.0, 304.0, 84.0], image).unwrap();
        assert_eq!(c.origin.0 + 288 + GAP * 2, 300 * 4);
        // Near the top and bottom: clamped inside.
        let c = CardLayout::beside((1280, 720), &g, [10.0, 0.0, 12.0, 2.0], image).unwrap();
        assert_eq!(c.origin.1, 0);
        let c = CardLayout::beside((1280, 720), &g, [10.0, 178.0, 12.0, 180.0], image).unwrap();
        assert_eq!(c.origin.1 + 160, 720);
        // Off the grid entirely: still inside.
        let c = CardLayout::beside((1280, 720), &g, [-50.0, -50.0, -40.0, -40.0], image).unwrap();
        assert_eq!(c.origin, (0, 0));
    }

    #[test]
    fn the_card_stays_inside_a_letterboxed_tank() {
        // A 320×180 grid at scale 2 in 1000×600: offset (180, 120).
        let g = Layout::fit((320, 180), (1000, 600));
        assert_eq!((g.scale, g.offset), (2, (180, 120)));
        let c = CardLayout::beside((1000, 600), &g, [0.0, 0.0, 1.0, 1.0], (144, 80)).unwrap();
        assert_eq!(c.scale, 2);
        assert!(c.origin.0 >= 180 && c.origin.1 >= 120, "{c:?}");
        let (w, h) = c.size();
        assert!(c.origin.0 + w <= 820 && c.origin.1 + h <= 480);
    }

    #[test]
    fn a_small_target_shrinks_the_card_or_hides_it() {
        let g = Layout::fit((320, 180), (512, 288));
        let c = CardLayout::beside((512, 288), &g, [10.0, 10.0, 11.0, 11.0], (144, 100)).unwrap();
        assert_eq!(c.scale, 1);
        let g = Layout::fit((320, 180), (100, 60));
        assert_eq!(
            CardLayout::beside((100, 60), &g, [1.0, 1.0, 2.0, 2.0], (144, 100)),
            None
        );
        assert_eq!(
            CardLayout::beside((1280, 720), &grid(), [0.0; 4], (0, 0)),
            None
        );
    }
}
