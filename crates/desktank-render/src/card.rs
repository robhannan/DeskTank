//! The species card: a small pixel-art info panel for a creature or plant
//! species (SPEC §2.5), drawn in the canvas on every host.
//!
//! Like the palette, the card is a tiny RGBA image built on the CPU when it
//! changes ([`build`]) and drawn by the tank shader at an integer scale
//! from a second region of the HUD texture, below the palette's. It is
//! opaque where drawn and sits on top of everything, so the shader treats
//! its pixels as the palette's. Hosts hit-test clicks with [`CardLayout`].
//!
//! Everything shown comes from the species data: its name, its Latin name,
//! its sprite (male and female side by side if they are drawn differently;
//! a sprout in the plant's colours for a plant) and its `describe_*` lines
//! and description, set in the [`crate::font`] pixel font.

use desktank_sim::tools::{PaletteItem, Tool};
use desktank_sim::{
    MaterialTable, PlantSpecies, PlantSpeciesId, PlantSpeciesTable, Species, SpeciesId,
    SpeciesTable,
};

use crate::Layout;
use crate::font;
use crate::hud::{
    self, BACKGROUND, FRAME, ICON_FG, OPEN, PaletteLayout, SELECTED, SPROUT_SIDE, sprout_ink,
};

/// A species to show on the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Card {
    /// A creature species (live food and cheat-only species included).
    Creature(SpeciesId),
    /// A plant species.
    Plant(PlantSpeciesId),
}

impl Card {
    /// The card for a selected palette item: a creature's or a plant's
    /// species; `None` for materials, devices and other tools.
    #[must_use]
    pub fn of(item: PaletteItem) -> Option<Self> {
        match item {
            PaletteItem::Tool(Tool::AddCreature(s)) => Some(Self::Creature(s)),
            PaletteItem::Tool(Tool::Plant(s)) => Some(Self::Plant(s)),
            _ => None,
        }
    }
}

/// Card width in card pixels.
pub const CARD_W: u32 = 144;
/// Largest card image: its fixed width and the tallest any species needs.
pub const CARD_MAX: (u32, u32) = (CARD_W, 176);

/// Size of the HUD texture: the palette's region (at the top) and the
/// card's region below it.
pub const HUD_TEXTURE: (u32, u32) = (
    if hud::MAX_SIZE.0 > CARD_MAX.0 {
        hud::MAX_SIZE.0
    } else {
        CARD_MAX.0
    },
    hud::MAX_SIZE.1 + CARD_MAX.1,
);
/// The row of the HUD texture where the card's region starts (its left edge
/// is column 0).
pub const TEXTURE_Y: u32 = hud::MAX_SIZE.1;

/// Pixels between the card's edge and its text.
const PAD: u32 = 4;
/// Extra pixels between paragraphs.
const PARA_GAP: u32 = 3;
/// Pixels between the male and female sprites.
const SPRITE_GAP: u32 = 8;
/// Tallest a sprite may be drawn, in card pixels: it is doubled only if
/// that stays within this.
const SPRITE_MAX_H: u32 = 48;
/// Integer scale of the plant's sprout.
const SPROUT_SCALE: u32 = 3;

/// Text colours (the frame and background are the palette's).
const TITLE: [u8; 3] = SELECTED;
const LABEL: [u8; 3] = OPEN;
const BODY: [u8; 3] = ICON_FG;
const LATIN: [u8; 3] = [0x8f, 0xa6, 0xc0];

/// The card as an RGBA image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardImage {
    /// Tightly packed RGBA rows, opaque.
    pub pixels: Vec<u8>,
    /// Width and height in card pixels (at most [`CARD_MAX`]).
    pub size: (u32, u32),
    /// Whether the content was cut off at [`CARD_MAX`]'s height.
    pub clipped: bool,
}

/// Draws the card for `card`, or `None` if the species isn't in its table.
#[must_use]
pub fn build(
    card: Card,
    materials: &MaterialTable,
    species: &SpeciesTable,
    plants: &PlantSpeciesTable,
) -> Option<CardImage> {
    let mut page = Page::new();
    match card {
        Card::Creature(id) => creature(&mut page, species.get(id)?, materials),
        Card::Plant(id) => plant(&mut page, plants.get(id)?, materials),
    }
    Some(page.finish())
}

/// A name from its data id, as the palette and inspect card write it:
/// `home_owning_crab` is "Home owning crab".
fn display_name(name: &str) -> String {
    let name = name.replace('_', " ");
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// A sprite to draw: its coarse size in texels, its first frame and the
/// label under it (empty for none).
type Art<'a> = ((usize, usize), &'a [[u8; 4]], &'static str);

/// The adult sprites of a creature: the male and the female, labelled, if
/// it is drawn differently by sex, else the one look, unlabelled. Each is
/// its coarse first frame.
fn creature_sprites(sp: &Species) -> Vec<Art<'_>> {
    let sexes: &[(bool, &str)] = if sp.dimorphic() {
        &[(false, "male"), (true, "female")]
    } else {
        &[(false, "")]
    };
    sexes
        .iter()
        .filter_map(|&(female, label)| {
            let look = sp.looks.get(sp.look(0, female, false))?;
            Some((look.size, look.frames.first()?.as_slice(), label))
        })
        .collect()
}

/// A creature: its sprites, adult size, diet, behaviour and needs, and
/// description.
fn creature(page: &mut Page, sp: &Species, materials: &MaterialTable) {
    page.heading(&display_name(&sp.name), &sp.scientific_name);
    page.sprites(&creature_sprites(sp));
    page.field("Size:", &sp.describe_size());
    page.field("Diet:", &sp.describe_diet(materials));
    page.field("Behaviour:", &sp.describe_behaviour());
    page.field("Needs:", &sp.describe_requirements());
    page.paragraph(&sp.description, BODY);
}

/// A plant: its sprout in its colours, what kind it is and what it needs.
fn plant(page: &mut Page, sp: &PlantSpecies, materials: &MaterialTable) {
    page.heading(&display_name(&sp.name), &sp.scientific_name);
    let stem = sp.colours.stem.get(sp.colours.stem.len() / 2).copied();
    let colours = Some((sp.colour(), stem.unwrap_or_else(|| sp.colour())));
    page.sprout(colours);
    page.field("Form:", sp.describe_form());
    page.field("Needs:", &sp.describe_requirements(materials));
    page.paragraph(&sp.description, BODY);
}

/// The card being drawn top to bottom into a [`CARD_MAX`]-sized image that
/// [`Page::finish`] crops.
struct Page {
    pixels: Vec<u8>,
    /// Top of the next thing to draw.
    y: u32,
}

impl Page {
    fn new() -> Self {
        let mut pixels = Vec::with_capacity((CARD_MAX.0 * CARD_MAX.1 * 4) as usize);
        for _ in 0..CARD_MAX.0 * CARD_MAX.1 {
            pixels.extend_from_slice(&[BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]);
        }
        Self { pixels, y: PAD }
    }

    /// Width of the text area.
    const fn inner() -> u32 {
        CARD_W - 2 * PAD
    }

    fn text(&mut self, x: u32, text: &str, colour: [u8; 3]) {
        font::draw(&mut self.pixels, CARD_MAX, (x, self.y), text, colour);
    }

    fn put(&mut self, x: u32, y: u32, c: [u8; 3]) {
        if x < CARD_MAX.0 && y < CARD_MAX.1 {
            let i = ((y * CARD_MAX.0 + x) * 4) as usize;
            self.pixels[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }

    /// The name in the title colour and the Latin name under it.
    fn heading(&mut self, name: &str, latin: &str) {
        for line in font::wrap(name, Self::inner()) {
            self.text(PAD, &line, TITLE);
            self.y += font::LINE;
        }
        for line in font::wrap(latin, Self::inner()) {
            self.text(PAD, &line, LATIN);
            self.y += font::LINE;
        }
        self.y += PARA_GAP;
    }

    /// Wrapped text in `colour`.
    fn paragraph(&mut self, text: &str, colour: [u8; 3]) {
        for line in font::wrap(text, Self::inner()) {
            self.text(PAD, &line, colour);
            self.y += font::LINE;
        }
        self.y += PARA_GAP;
    }

    /// `label` in the label colour, then `text`, wrapped as one paragraph.
    fn field(&mut self, label: &str, text: &str) {
        let full = format!("{label} {text}");
        let label_len = label.chars().count();
        for (i, line) in font::wrap(&full, Self::inner()).iter().enumerate() {
            // The label is the first word, so it is whole on the first line.
            if i == 0 {
                let rest: String = line.chars().skip(label_len).collect();
                let rest_x = PAD + font::ADVANCE * u32::try_from(label_len).unwrap_or(0);
                self.text(PAD, label, LABEL);
                self.text(rest_x, &rest, BODY);
            } else {
                self.text(PAD, line, BODY);
            }
            self.y += font::LINE;
        }
        self.y += PARA_GAP;
    }

    /// The sprites (coarse frame texels, sRGB RGBA, alpha 0 or 255) side by
    /// side, centred and doubled if they stay within [`SPRITE_MAX_H`] and
    /// the width, each with its label under it (none if all labels are
    /// empty).
    fn sprites(&mut self, sprites: &[Art<'_>]) {
        let dims = |(w, h): (usize, usize)| {
            (
                u32::try_from(w).unwrap_or(u32::MAX),
                u32::try_from(h).unwrap_or(u32::MAX),
            )
        };
        let gaps = SPRITE_GAP * u32::try_from(sprites.len().saturating_sub(1)).unwrap_or(0);
        let total_w: u32 = sprites.iter().map(|s| dims(s.0).0).sum();
        let tallest = sprites.iter().map(|s| dims(s.0).1).max().unwrap_or(0);
        let scale = if (total_w * 2 + gaps) <= Self::inner() && tallest * 2 <= SPRITE_MAX_H {
            2
        } else {
            1
        };
        let labelled = sprites.iter().any(|s| !s.2.is_empty());
        let mut x = PAD + Self::inner().saturating_sub(total_w * scale + gaps) / 2;
        for &(size, frame, label) in sprites {
            let (w, h) = dims(size);
            let top = self.y + (tallest - h) * scale / 2;
            for (i, px) in frame.iter().enumerate() {
                if px[3] > 0 {
                    let (tx, ty) = (
                        u32::try_from(i % size.0.max(1)).unwrap_or(0),
                        u32::try_from(i / size.0.max(1)).unwrap_or(0),
                    );
                    for (dx, dy) in (0..scale).flat_map(|dx| (0..scale).map(move |dy| (dx, dy))) {
                        self.put(
                            x + tx * scale + dx,
                            top + ty * scale + dy,
                            [px[0], px[1], px[2]],
                        );
                    }
                }
            }
            if labelled {
                let at = self.y + tallest * scale + 2;
                let label_x = x + (w * scale).saturating_sub(font::width(label)) / 2;
                font::draw(&mut self.pixels, CARD_MAX, (label_x, at), label, LABEL);
            }
            x += w * scale + SPRITE_GAP;
        }
        self.y += tallest * scale + 2 + if labelled { font::LINE } else { 0 } + PARA_GAP;
    }

    /// The palette's sprout, enlarged, centred.
    fn sprout(&mut self, colours: Option<([u8; 3], [u8; 3])>) {
        let side = SPROUT_SIDE * SPROUT_SCALE;
        let x0 = PAD + Self::inner().saturating_sub(side) / 2;
        for (x, y) in (0..SPROUT_SIDE).flat_map(|x| (0..SPROUT_SIDE).map(move |y| (x, y))) {
            if let Some(c) = sprout_ink(colours, x, y) {
                for (dx, dy) in
                    (0..SPROUT_SCALE).flat_map(|a| (0..SPROUT_SCALE).map(move |b| (a, b)))
                {
                    self.put(
                        x0 + x * SPROUT_SCALE + dx,
                        self.y + y * SPROUT_SCALE + dy,
                        c,
                    );
                }
            }
        }
        self.y += side + PARA_GAP;
    }

    /// Crops to the content and draws the frame.
    fn finish(self) -> CardImage {
        let wanted = self.y + PAD - PARA_GAP;
        let height = wanted.min(CARD_MAX.1);
        let row = (CARD_MAX.0 * 4) as usize;
        let mut pixels = self.pixels;
        pixels.truncate(row * height as usize);
        let mut image = CardImage {
            pixels,
            size: (CARD_W, height),
            clipped: wanted > CARD_MAX.1,
        };
        let (w, h) = image.size;
        for d in 0..w.max(h) {
            for (x, y) in [(d, 0), (d, h - 1), (0, d), (w - 1, d)] {
                if x < w && y < h {
                    let i = ((y * w + x) * 4) as usize;
                    image.pixels[i..i + 4].copy_from_slice(&[FRAME[0], FRAME[1], FRAME[2], 255]);
                }
            }
        }
        image
    }
}

/// Where the card is drawn in a target, for hit tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CardLayout {
    /// Top-left in target pixels.
    pub origin: (u32, u32),
    /// Target pixels per card pixel.
    pub scale: u32,
    /// The card image's size in card pixels.
    pub image: (u32, u32),
}

impl CardLayout {
    /// Places a card image of `image` pixels in `target`. It takes the
    /// palette's scale and sits just below it, left-aligned with it (two
    /// palette pixels under it, as it is from the top), or, with no
    /// palette, in the top-left at the grid's cell scale (at least 2). The
    /// scale drops until the card fits inside `target` and, above scale 1,
    /// covers at most half its width and half its height, so a small
    /// canvas (a phone) keeps most of the tank clear; `None` if it doesn't
    /// fit even at 1.
    #[must_use]
    pub fn fit(
        target: (u32, u32),
        grid: &Layout,
        palette: Option<&PaletteLayout>,
        image: (u32, u32),
    ) -> Option<Self> {
        if image.0 == 0 || image.1 == 0 {
            return None;
        }
        let want = palette.map_or_else(|| grid.scale.max(2), |p| p.scale);
        (1..=want).rev().find_map(|scale| {
            let origin = palette.map_or((2 * scale, 2 * scale), |p| {
                (p.origin.0, p.origin.1 + p.size().1 + 2 * scale)
            });
            let (w, h) = (
                u64::from(image.0) * u64::from(scale),
                u64::from(image.1) * u64::from(scale),
            );
            let fits = u64::from(origin.0) + w <= u64::from(target.0)
                && u64::from(origin.1) + h <= u64::from(target.1);
            let small =
                scale == 1 || (2 * w <= u64::from(target.0) && 2 * h <= u64::from(target.1));
            (fits && small).then_some(Self {
                origin,
                scale,
                image,
            })
        })
    }

    /// Size in target pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.image.0 * self.scale, self.image.1 * self.scale)
    }

    /// Whether target pixel `(px, py)` is over the card.
    #[must_use]
    pub fn contains(&self, px: f64, py: f64) -> bool {
        let (w, h) = self.size();
        let (x, y) = (px - f64::from(self.origin.0), py - f64::from(self.origin.1));
        x >= 0.0 && y >= 0.0 && x < f64::from(w) && y < f64::from(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables() -> (MaterialTable, SpeciesTable, PlantSpeciesTable) {
        let m = MaterialTable::builtin().unwrap();
        let s = SpeciesTable::builtin_for(&m).unwrap();
        let p = PlantSpeciesTable::builtin_for(&m).unwrap();
        (m, s, p)
    }

    fn every_card(
        (m, s, p): &(MaterialTable, SpeciesTable, PlantSpeciesTable),
    ) -> Vec<(String, Card, CardImage)> {
        let creatures = s.iter().map(|sp| (sp.name.clone(), Card::Creature(sp.id)));
        let plants = p.iter().map(|sp| (sp.name.clone(), Card::Plant(sp.id)));
        creatures
            .chain(plants)
            .map(|(name, card)| (name, card, build(card, m, s, p).unwrap()))
            .collect()
    }

    /// Every string a species card shows, for font coverage.
    fn every_string((m, s, p): &(MaterialTable, SpeciesTable, PlantSpeciesTable)) -> Vec<String> {
        let creatures = s.iter().flat_map(|sp| {
            [
                display_name(&sp.name),
                sp.scientific_name.clone(),
                sp.description.clone(),
                sp.describe_diet(m),
                sp.describe_behaviour(),
                sp.describe_requirements(),
                sp.describe_size(),
            ]
        });
        let plants = p.iter().flat_map(|sp| {
            [
                display_name(&sp.name),
                sp.scientific_name.clone(),
                sp.description.clone(),
                sp.describe_form().to_owned(),
                sp.describe_requirements(m),
            ]
        });
        creatures
            .chain(plants)
            .chain(
                [
                    "Size:",
                    "Diet:",
                    "Behaviour:",
                    "Needs:",
                    "Form:",
                    "male",
                    "female",
                ]
                .map(str::to_owned),
            )
            .collect()
    }

    #[test]
    fn every_shipped_string_has_glyphs() {
        for text in every_string(&tables()) {
            for c in text.chars() {
                assert!(font::glyph(c).is_some(), "no glyph for {c:?} in {text:?}");
            }
        }
    }

    #[test]
    fn every_species_fits_the_card() {
        let t = tables();
        let cards = every_card(&t);
        assert_eq!(cards.len(), t.1.len() + t.2.len());
        for (name, _, image) in &cards {
            assert!(!image.clipped, "{name} is cut off at {CARD_MAX:?}");
            assert_eq!(image.size.0, CARD_W, "{name}");
            assert!(image.size.1 <= CARD_MAX.1 && image.size.1 > 20, "{name}");
            assert_eq!(
                image.pixels.len(),
                (image.size.0 * image.size.1 * 4) as usize,
                "{name}"
            );
            assert!(image.pixels.chunks(4).all(|p| p[3] == 255), "{name}");
            // Some text: more than the background and the frame.
            let ink = image
                .pixels
                .chunks(4)
                .filter(|p| p[..3] != BACKGROUND && p[..3] != FRAME)
                .count();
            assert!(ink > 200, "{name} drew only {ink} pixels");
        }
    }

    #[test]
    fn text_rows_are_not_empty_and_fit_the_width() {
        let t = tables();
        for text in every_string(&t) {
            assert_ne!(text.trim(), "");
            for line in font::wrap(&text, Page::inner()) {
                assert!(font::width(&line) <= Page::inner(), "{line:?}");
            }
        }
    }

    #[test]
    fn display_names_come_from_the_data_name() {
        assert_eq!(display_name("guppy"), "Guppy");
        assert_eq!(display_name("african_dwarf_frog"), "African dwarf frog");
        assert_eq!(display_name(""), "");
    }

    #[test]
    fn dimorphic_species_are_those_drawn_differently_by_sex() {
        let (_, s, _) = tables();
        let two: Vec<_> = s.iter().filter(|sp| sp.dimorphic()).collect();
        let names: Vec<_> = two.iter().map(|sp| sp.name.as_str()).collect();
        assert!(
            names.contains(&"guppy") && names.contains(&"betta"),
            "{names:?}"
        );
        for sp in &two {
            assert!(sp.looks[sp.look(0, true, false)].female, "{}", sp.name);
        }
        assert!(s.iter().any(|sp| !sp.dimorphic()));
    }

    /// Counts the pixels in `image` equal to `colour`.
    fn count(image: &CardImage, colour: [u8; 3]) -> usize {
        image.pixels.chunks(4).filter(|p| p[..3] == colour).count()
    }

    #[test]
    fn a_dimorphic_card_draws_both_sexes_and_a_plain_one_draws_one() {
        let (m, s, p) = tables();
        let species = |name: &str| s.get(s.id(name).unwrap()).unwrap();
        for name in ["guppy", "betta"] {
            let sp = species(name);
            let art = creature_sprites(sp);
            let labels: Vec<_> = art.iter().map(|a| a.2).collect();
            assert_eq!(labels, ["male", "female"], "{name}");
            for (female, (size, frame, _)) in [false, true].into_iter().zip(&art) {
                let look = &sp.looks[sp.look(0, female, false)];
                assert!(look.female == female && *size == look.size, "{name}");
                assert_eq!(*frame, look.frames[0].as_slice(), "{name}");
            }
            // The two are different pictures.
            assert_ne!(art[0].1, art[1].1, "{name}");
        }
        let tetra = creature_sprites(species("neon_tetra"));
        assert_eq!(tetra.len(), 1);
        assert_eq!(tetra[0].2, "");
        // The guppy's sprites are doubled: its male's first opaque texel's
        // colour fills at least four card pixels per such texel.
        let guppy = species("guppy");
        let card = build(Card::Creature(guppy.id), &m, &s, &p).unwrap();
        let male = &guppy.looks[guppy.look(0, false, false)].frames[0];
        let own = |c: [u8; 3]| ![TITLE, LABEL, BODY, LATIN, BACKGROUND, FRAME].contains(&c);
        let colour = male
            .iter()
            .map(|p| [p[0], p[1], p[2]])
            .find(|&c| own(c))
            .unwrap();
        let texels = male
            .iter()
            .filter(|p| p[3] > 0 && [p[0], p[1], p[2]] == colour)
            .count();
        assert!(count(&card, colour) >= texels * 4, "{texels} texels");
    }

    #[test]
    fn a_plant_card_shows_the_sprout_in_its_colours() {
        let (m, s, p) = tables();
        let moss = p.get(p.id("java_moss").unwrap()).unwrap();
        let card = build(Card::Plant(moss.id), &m, &s, &p).unwrap();
        // The leaf colour fills 3×3 blocks of the sprout.
        assert!(count(&card, moss.colour()) >= 9 * 6);
    }

    #[test]
    fn unknown_species_have_no_card() {
        let (m, s, p) = tables();
        assert!(build(Card::Creature(SpeciesId(250)), &m, &s, &p).is_none());
        assert!(build(Card::Plant(PlantSpeciesId(250)), &m, &s, &p).is_none());
    }

    #[test]
    fn the_hud_texture_holds_both_regions() {
        assert!(HUD_TEXTURE.0 >= hud::MAX_SIZE.0 && HUD_TEXTURE.0 >= CARD_MAX.0);
        assert_eq!(TEXTURE_Y, hud::MAX_SIZE.1);
        assert_eq!(HUD_TEXTURE.1, TEXTURE_Y + CARD_MAX.1);
        // Within WebGL2's minimum 2048 texture side.
        assert!(HUD_TEXTURE.0 <= 2048 && HUD_TEXTURE.1 <= 2048);
    }

    fn grid() -> Layout {
        Layout::fit((320, 180), (1280, 720))
    }

    #[test]
    fn the_card_sits_below_the_palette_at_its_scale() {
        let g = grid();
        let p = PaletteLayout::fit((1280, 720), &g, 9, 0, 4);
        let c = CardLayout::fit((1280, 720), &g, Some(&p), (144, 80)).unwrap();
        assert_eq!(c.scale, p.scale);
        assert_eq!(c.origin.0, p.origin.0);
        // The palette's bottom (8 + 23 * 4) and a 2-pixel gap at scale 4.
        assert_eq!(c.origin.1, p.origin.1 + p.size().1 + 8);
        assert_eq!(c.size(), (144 * 4, 80 * 4));
        assert!(c.contains(f64::from(c.origin.0), f64::from(c.origin.1)));
        assert!(c.contains(f64::from(c.origin.0 + 575), f64::from(c.origin.1 + 319)));
        assert!(!c.contains(f64::from(c.origin.0 + 576), f64::from(c.origin.1)));
        assert!(!c.contains(f64::from(c.origin.0) - 0.5, f64::from(c.origin.1)));
    }

    #[test]
    fn without_a_palette_the_card_takes_the_top_left_at_cell_scale() {
        let c = CardLayout::fit((1280, 720), &grid(), None, (144, 80)).unwrap();
        assert_eq!((c.scale, c.origin), (4, (8, 8)));
        // A coarse grid still gets at least scale 2.
        let g = Layout::fit((320, 180), (320, 180));
        assert_eq!(g.scale, 1);
        let c = CardLayout::fit((320, 180), &g, None, (144, 100)).unwrap();
        assert_eq!(c.scale, 1, "2 doesn't fit 100 rows plus margins in 180");
        let c = CardLayout::fit(
            (640, 360),
            &Layout::fit((320, 180), (640, 360)),
            None,
            (144, 80),
        )
        .unwrap();
        assert_eq!((c.scale, c.origin), (2, (4, 4)));
    }

    #[test]
    fn above_scale_1_the_card_covers_at_most_half_the_target() {
        // 144×100 at the grid's 4 would be 400 of 720 rows: scale 3.
        let c = CardLayout::fit((1280, 720), &grid(), None, (144, 100)).unwrap();
        assert_eq!(c.scale, 3);
        // A phone-sized canvas: scale 2 (288 of 512 columns) is too wide.
        let g = Layout::fit((128, 72), (512, 288));
        let c = CardLayout::fit((512, 288), &g, None, (144, 100)).unwrap();
        assert_eq!(c.scale, 1, "only scale 1 may cover more than half");
    }

    #[test]
    fn the_card_shrinks_to_fit_and_is_hidden_when_it_cannot() {
        let grid = Layout::fit((320, 180), (1280, 720));
        let palette = PaletteLayout::fit((1280, 720), &grid, 9, 0, 4);
        // Too tall at scale 4 below the palette: shrinks, stays inside.
        let card = CardLayout::fit((1280, 400), &grid, Some(&palette), (144, 100)).unwrap();
        assert!(card.scale < 4);
        let (width, height) = card.size();
        assert!(card.origin.0 + width <= 1280 && card.origin.1 + height <= 400);
        // Too small at scale 1: nothing.
        let grid = Layout::fit((320, 180), (160, 90));
        let palette = PaletteLayout::fit((160, 90), &grid, 9, 0, 4);
        assert_eq!(
            CardLayout::fit((160, 90), &grid, Some(&palette), (144, 100)),
            None
        );
        assert_eq!(CardLayout::fit((0, 0), &grid, None, (144, 100)), None);
        assert_eq!(CardLayout::fit((1280, 720), &grid, None, (0, 0)), None);
    }

    /// Writes cards as PNGs to `target/shots/card-*.png` when `CARD_SHOTS`
    /// is set, enlarged four times, to look at.
    #[test]
    fn writes_card_shots_on_request() {
        if std::env::var_os("CARD_SHOTS").is_none() {
            return;
        }
        let t = tables();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).unwrap();
        for (name, _, image) in every_card(&t) {
            let scale = 4;
            let (w, h) = image.size;
            let mut big = Vec::new();
            for y in 0..h * scale {
                for x in 0..w * scale {
                    let i = (((y / scale) * w + x / scale) * 4) as usize;
                    big.extend_from_slice(&image.pixels[i..i + 4]);
                }
            }
            std::fs::write(
                dir.join(format!("card-{name}.png")),
                png(&big, (w * scale, h * scale)),
            )
            .unwrap();
        }
    }

    /// A minimal PNG of tightly packed RGBA: stored (uncompressed) blocks.
    fn png(rgba: &[u8], (width, height): (u32, u32)) -> Vec<u8> {
        fn crc(bytes: &[u8]) -> u32 {
            let mut c = !0_u32;
            for &b in bytes {
                c ^= u32::from(b);
                for _ in 0..8 {
                    c = if c & 1 == 1 {
                        0xedb8_8320 ^ (c >> 1)
                    } else {
                        c >> 1
                    };
                }
            }
            !c
        }
        fn chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
            out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
            let start = out.len();
            out.extend_from_slice(&kind);
            out.extend_from_slice(data);
            let sum = crc(&out[start..]);
            out.extend_from_slice(&sum.to_be_bytes());
        }
        let mut raw = Vec::new();
        for row in rgba.chunks((width * 4) as usize) {
            raw.push(0);
            raw.extend_from_slice(row);
        }
        let mut zlib = vec![0x78, 0x01];
        let mut blocks = raw.chunks(65535).peekable();
        while let Some(block) = blocks.next() {
            zlib.push(u8::from(blocks.peek().is_none()));
            let len = u16::try_from(block.len()).unwrap();
            zlib.extend_from_slice(&len.to_le_bytes());
            zlib.extend_from_slice(&(!len).to_le_bytes());
            zlib.extend_from_slice(block);
        }
        let (mut low, mut high) = (1_u32, 0_u32);
        for &x in &raw {
            low = (low + u32::from(x)) % 65521;
            high = (high + low) % 65521;
        }
        zlib.extend_from_slice(&((high << 16) | low).to_be_bytes());
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut header = Vec::new();
        header.extend_from_slice(&width.to_be_bytes());
        header.extend_from_slice(&height.to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(&mut out, *b"IHDR", &header);
        chunk(&mut out, *b"IDAT", &zlib);
        chunk(&mut out, *b"IEND", &[]);
        out
    }
}
