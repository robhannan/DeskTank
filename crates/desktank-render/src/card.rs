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
//! a small sprite of a mature example of the plant, from
//! [`crate::plant_sprite`], for a plant) and its `describe_*` lines
//! and description, set in the [`crate::font`] pixel font.

use desktank_sim::tools::{PaletteItem, Tool};
use desktank_sim::{
    MaterialTable, PlantSpecies, PlantSpeciesId, PlantSpeciesTable, Species, SpeciesId,
    SpeciesTable,
};

use crate::Layout;
use crate::font;
use crate::hud::{self, BACKGROUND, FRAME, ICON_FG, OPEN, PaletteLayout, SELECTED};
use crate::plant_sprite;

/// A species to show on the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Card {
    /// A creature species (live food and cheat-only species included), with
    /// the colour morph to preview (`None`: the species' palette morph).
    Creature(SpeciesId, Option<u8>),
    /// A plant species.
    Plant(PlantSpeciesId),
}

impl Card {
    /// The card for a selected palette item: a creature's or a plant's
    /// species; `None` for materials, devices and other tools.
    #[must_use]
    pub fn of(item: PaletteItem) -> Option<Self> {
        match item {
            PaletteItem::Tool(Tool::AddCreature(s)) => Some(Self::Creature(s, None)),
            PaletteItem::Tool(Tool::Plant(s)) => Some(Self::Plant(s)),
            _ => None,
        }
    }
}

/// Card width in card pixels.
pub const CARD_W: u32 = 144;
/// Largest card image: its fixed width and the tallest any species needs.
pub const CARD_MAX: (u32, u32) = (CARD_W, 176);

/// Size of the HUD texture: the palette's region (at the top), the species
/// card's region below it and the inspect card's below that
/// ([`crate::inspect::TEXTURE_Y`]).
pub const HUD_TEXTURE: (u32, u32) = (
    if hud::MAX_SIZE.0 > CARD_MAX.0 {
        hud::MAX_SIZE.0
    } else {
        CARD_MAX.0
    },
    hud::MAX_SIZE.1 + 2 * CARD_MAX.1,
);
/// The row of the HUD texture where the card's region starts (its left edge
/// is column 0).
pub const TEXTURE_Y: u32 = hud::MAX_SIZE.1;

/// Pixels between the card's edge and its text.
pub(crate) const PAD: u32 = 4;
/// Extra pixels between paragraphs.
pub(crate) const PARA_GAP: u32 = 3;
/// Pixels between the male and female sprites.
const SPRITE_GAP: u32 = 8;
/// Tallest a sprite may be drawn, in card pixels: it is doubled only if
/// that stays within this.
const SPRITE_MAX_H: u32 = 48;

/// Text colours (the frame and background are the palette's).
pub(crate) const TITLE: [u8; 3] = SELECTED;
pub(crate) const LABEL: [u8; 3] = OPEN;
pub(crate) const BODY: [u8; 3] = ICON_FG;
pub(crate) const LATIN: [u8; 3] = [0x8f, 0xa6, 0xc0];

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
    build_tagged(card, materials, species, plants, None)
}

/// [`build`] with `tag` (a page number such as `2/2`) in the top-right
/// corner, the heading wrapped clear of it.
pub(crate) fn build_tagged(
    card: Card,
    materials: &MaterialTable,
    species: &SpeciesTable,
    plants: &PlantSpeciesTable,
    tag: Option<&str>,
) -> Option<CardImage> {
    let mut page = Page::new(tag);
    match card {
        Card::Creature(id, morph) => creature(&mut page, species.get(id)?, morph, materials),
        Card::Plant(id) => plant(&mut page, plants.get(id)?, materials),
    }
    Some(page.finish())
}

/// A name from its data id, as the palette and inspect card write it:
/// `home_owning_crab` is "Home owning crab".
pub(crate) fn display_name(name: &str) -> String {
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
/// its coarse first frame, in colour morph `morph` (the species' palette
/// morph if `None`).
fn creature_sprites(sp: &Species, morph: Option<u8>) -> Vec<Art<'_>> {
    let morph = morph.unwrap_or(sp.palette_morph);
    let sexes: &[(bool, &str)] = if sp.dimorphic() {
        &[(false, "male"), (true, "female")]
    } else {
        &[(false, "")]
    };
    sexes
        .iter()
        .filter_map(|&(female, label)| {
            let look = sp.looks.get(sp.look(morph, female, false))?;
            Some((look.size, look.frames.first()?.as_slice(), label))
        })
        .collect()
}

/// A creature: its sprites, adult size, diet, behaviour and needs, and
/// description.
fn creature(page: &mut Page, sp: &Species, morph: Option<u8>, materials: &MaterialTable) {
    page.heading(&display_name(&sp.name), &sp.scientific_name);
    page.sprites(&creature_sprites(sp, morph));
    page.field("Size:", &sp.describe_size());
    page.field("Diet:", &sp.describe_diet(materials));
    page.field("Behaviour:", &sp.describe_behaviour());
    page.field("Needs:", &sp.describe_requirements());
    page.paragraph(&sp.description, BODY);
}

/// A plant: a sprite of a mature example, what kind it is and what it needs.
fn plant(page: &mut Page, sp: &PlantSpecies, materials: &MaterialTable) {
    page.heading(&display_name(&sp.name), &sp.scientific_name);
    let (size, pixels) = plant_sprite::build(sp);
    page.plant_sprite(size, &pixels);
    page.field("Form:", sp.describe_form());
    page.field("Needs:", &sp.describe_requirements(materials));
    page.paragraph(&sp.description, BODY);
}

/// A card being drawn top to bottom into a [`CARD_MAX`]-sized image that
/// [`Page::finish`] crops.
pub(crate) struct Page {
    pixels: Vec<u8>,
    /// Top of the next thing to draw.
    y: u32,
    /// Width the heading keeps clear of a corner tag, in pixels.
    tag_width: u32,
}

impl Page {
    /// An empty page, with `tag` in the label colour in its top-right
    /// corner if given.
    pub(crate) fn new(tag: Option<&str>) -> Self {
        let mut pixels = Vec::with_capacity((CARD_MAX.0 * CARD_MAX.1 * 4) as usize);
        for _ in 0..CARD_MAX.0 * CARD_MAX.1 {
            pixels.extend_from_slice(&[BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]);
        }
        let mut page = Self {
            pixels,
            y: PAD,
            tag_width: 0,
        };
        if let Some(tag) = tag {
            let w = font::width(tag);
            page.text(CARD_W - PAD - w, tag, LABEL);
            page.tag_width = w + font::ADVANCE;
        }
        page
    }

    /// Width of the text area.
    pub(crate) const fn inner() -> u32 {
        CARD_W - 2 * PAD
    }

    fn text(&mut self, x: u32, text: &str, colour: [u8; 3]) {
        font::draw(&mut self.pixels, CARD_MAX, (x, self.y), text, colour);
    }

    /// Draws `text` with its top-left at `(x, y)`.
    pub(crate) fn text_at(&mut self, x: u32, y: u32, text: &str, colour: [u8; 3]) {
        font::draw(&mut self.pixels, CARD_MAX, (x, y), text, colour);
    }

    /// Moves the next thing down by `dy` pixels.
    pub(crate) fn skip(&mut self, dy: u32) {
        self.y += dy;
    }

    /// One pixel of colour `c` (cut off outside the page).
    pub(crate) fn put(&mut self, x: u32, y: u32, c: [u8; 3]) {
        if x < CARD_MAX.0 && y < CARD_MAX.1 {
            let i = ((y * CARD_MAX.0 + x) * 4) as usize;
            self.pixels[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }

    /// The name in the title colour and the Latin name (or another
    /// subtitle; none if empty) under it, both clear of the corner tag.
    pub(crate) fn heading(&mut self, name: &str, latin: &str) {
        let width = Self::inner() - self.tag_width;
        for line in font::wrap(name, width) {
            self.text(PAD, &line, TITLE);
            self.y += font::LINE;
        }
        for line in font::wrap(latin, width) {
            self.text(PAD, &line, LATIN);
            self.y += font::LINE;
        }
        self.y += PARA_GAP;
    }

    /// A one-line section heading in the label colour.
    pub(crate) fn section(&mut self, text: &str) {
        self.text(PAD, text, LABEL);
        self.y += font::LINE + 1;
    }

    /// Wrapped text in `colour`.
    pub(crate) fn paragraph(&mut self, text: &str, colour: [u8; 3]) {
        for line in font::wrap(text, Self::inner()) {
            self.text(PAD, &line, colour);
            self.y += font::LINE;
        }
        self.y += PARA_GAP;
    }

    /// `label` in the label colour, then `text`, wrapped as one paragraph.
    pub(crate) fn field(&mut self, label: &str, text: &str) {
        self.field_in(label, text, BODY);
    }

    /// [`field`](Self::field) with `text` in `colour`. Returns the top of
    /// its rows in card pixels, its gap above included; [`bottom`](Self::bottom)
    /// after it is the bottom, so a click just off its text still lands.
    pub(crate) fn field_in(&mut self, label: &str, text: &str, colour: [u8; 3]) -> u32 {
        let top = self.y.saturating_sub(PARA_GAP);
        let full = format!("{label} {text}");
        let label_len = label.chars().count();
        for (i, line) in font::wrap(&full, Self::inner()).iter().enumerate() {
            // The label is the first word, so it is whole on the first line.
            if i == 0 {
                let rest: String = line.chars().skip(label_len).collect();
                let rest_x = PAD + font::ADVANCE * u32::try_from(label_len).unwrap_or(0);
                self.text(PAD, label, LABEL);
                self.text(rest_x, &rest, colour);
            } else {
                self.text(PAD, line, colour);
            }
            self.y += font::LINE;
        }
        self.y += PARA_GAP;
        top
    }

    /// Top of the next thing to draw, in card pixels.
    pub(crate) fn bottom(&self) -> u32 {
        self.y
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
            self.blit(x, top, size, frame, scale);
            if labelled {
                let at = self.y + tallest * scale + 2;
                let label_x = x + (w * scale).saturating_sub(font::width(label)) / 2;
                font::draw(&mut self.pixels, CARD_MAX, (label_x, at), label, LABEL);
            }
            x += w * scale + SPRITE_GAP;
        }
        self.y += tallest * scale + 2 + if labelled { font::LINE } else { 0 } + PARA_GAP;
    }

    /// Draws `frame` (`size` texels, sRGB RGBA, alpha 0 or 255) with its
    /// top-left at `(x, top)`, each texel `scale` card pixels square.
    fn blit(&mut self, x: u32, top: u32, size: (usize, usize), frame: &[[u8; 4]], scale: u32) {
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
    }

    /// A plant's sprite (from [`plant_sprite::build`], within
    /// [`plant_sprite::MAX_WIDTH`] by [`plant_sprite::MAX_HEIGHT`]), centred
    /// at one card pixel per texel.
    fn plant_sprite(&mut self, size: (usize, usize), frame: &[[u8; 4]]) {
        debug_assert!(
            size.0 <= plant_sprite::MAX_WIDTH && size.1 <= plant_sprite::MAX_HEIGHT,
            "a {size:?} plant sprite"
        );
        let w = u32::try_from(size.0).unwrap_or(u32::MAX);
        let h = u32::try_from(size.1).unwrap_or(u32::MAX);
        let x = PAD + Self::inner().saturating_sub(w) / 2;
        self.blit(x, self.y, size, frame, 1);
        self.y += h + PARA_GAP;
    }

    /// Crops to the content and draws the frame.
    pub(crate) fn finish(self) -> CardImage {
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

/// Which edge of the canvas a host's out-of-canvas palette is on, so the
/// species card can sit on the side nearest it. It matters only with no
/// in-canvas palette: with one, the card sits directly below it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CardAnchor {
    /// The palette is above the canvas, or there is none: the card is in
    /// the top-left.
    #[default]
    Top,
    /// The palette is below the canvas (the web's DOM palette): the card is
    /// in the bottom-left.
    Bottom,
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
    /// Places a card image of `image` pixels in `target`, at the scale
    /// [`card_scale`] gives for it (`min_scale` is the host's minimum, see
    /// there). With a palette it sits just below it, left-aligned with it
    /// (two card pixels under it); with none, it sits in the top-left or,
    /// for [`CardAnchor::Bottom`], the bottom-left, two card pixels in from
    /// the edges.
    ///
    /// `image` only has to fit at that scale; `None` if it doesn't fit even
    /// at 1.
    #[must_use]
    pub fn fit(
        target: (u32, u32),
        grid: &Layout,
        palette: Option<&PaletteLayout>,
        anchor: CardAnchor,
        min_scale: u32,
        image: (u32, u32),
    ) -> Option<Self> {
        if image.0 == 0 || image.1 == 0 {
            return None;
        }
        let scale = card_scale(target, grid, palette, min_scale);
        let origin = card_origin(target, palette, anchor, scale, image.1)?;
        fits_at(target, origin, scale, image).then_some(Self {
            origin,
            scale,
            image,
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

/// The card's top-left in `target` at `scale` for a card `height` pixels
/// tall: below the palette if there is one, else in the top-left or
/// bottom-left. `None` if it would start off the target.
fn card_origin(
    target: (u32, u32),
    palette: Option<&PaletteLayout>,
    anchor: CardAnchor,
    scale: u32,
    height: u32,
) -> Option<(u32, u32)> {
    let margin = i64::from(2 * scale);
    let (x, y) = match (palette, anchor) {
        (Some(p), _) => (
            i64::from(p.origin.0),
            i64::from(p.origin.1) + i64::from(p.size().1) + margin,
        ),
        (None, CardAnchor::Top) => (margin, margin),
        (None, CardAnchor::Bottom) => (
            margin,
            i64::from(target.1) - margin - i64::from(height) * i64::from(scale),
        ),
    };
    Some((u32::try_from(x).ok()?, u32::try_from(y).ok()?))
}

/// Whether a card image of `size` card pixels at `origin` and `scale` is
/// inside `target`.
fn fits_at(target: (u32, u32), origin: (u32, u32), scale: u32, size: (u32, u32)) -> bool {
    i64::from(origin.0) + i64::from(size.0) * i64::from(scale) <= i64::from(target.0)
        && i64::from(origin.1) + i64::from(size.1) * i64::from(scale) <= i64::from(target.1)
}

/// The one scale every card is drawn at in `target` (target pixels per card
/// pixel): the largest up to `want` at which the biggest card
/// ([`CARD_MAX`]) fits inside `target` and, above scale 1, covers at most
/// half its width, so a small canvas (a phone) keeps most of the tank clear.
/// `want` is the palette's scale, or the grid's cell scale without one
/// (never under 2), or `min_scale` if that is more: the host's floor for a
/// readable card, a high-density display (the web passes its device-pixel
/// ratio) where 2 target pixels are too few to read. It is 1 where nothing
/// larger fits; the species card and the inspect card both take it, so every
/// card and page matches.
#[must_use]
pub fn card_scale(
    target: (u32, u32),
    grid: &Layout,
    palette: Option<&PaletteLayout>,
    min_scale: u32,
) -> u32 {
    let want = palette
        .map_or(grid.scale, |p| p.scale)
        .max(2)
        .max(min_scale);
    (2..=want)
        .rev()
        .find(|&scale| {
            let half = 2 * u64::from(CARD_MAX.0) * u64::from(scale) <= u64::from(target.0);
            half && card_origin(target, palette, CardAnchor::Top, scale, CARD_MAX.1)
                .is_some_and(|origin| fits_at(target, origin, scale, CARD_MAX))
        })
        .unwrap_or(1)
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
        let creatures = s
            .iter()
            .map(|sp| (sp.name.clone(), Card::Creature(sp.id, None)));
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
            let art = creature_sprites(sp, None);
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
        let tetra = creature_sprites(species("neon_tetra"), None);
        assert_eq!(tetra.len(), 1);
        assert_eq!(tetra[0].2, "");
        // The guppy's sprites are doubled: its male's first opaque texel's
        // colour fills at least four card pixels per such texel.
        let guppy = species("guppy");
        let card = build(Card::Creature(guppy.id, None), &m, &s, &p).unwrap();
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
    fn a_creature_card_previews_the_picked_morph() {
        let (m, s, p) = tables();
        for name in ["cherry_shrimp", "betta"] {
            let sp = s.get(s.id(name).unwrap()).unwrap();
            let count = u8::try_from(sp.morphs.len()).unwrap();
            assert!(count > 1, "{name}");
            let card = |morph| build(Card::Creature(sp.id, morph), &m, &s, &p).unwrap();
            // No pick previews the palette morph.
            assert_eq!(card(None), card(Some(sp.palette_morph)), "{name}");
            for morph in 0..count {
                let art = creature_sprites(sp, Some(morph));
                for (female, (size, frame, _)) in [false, true].into_iter().zip(&art) {
                    let look = &sp.looks[sp.look(morph, female, false)];
                    assert_eq!((*size, *frame), (look.size, look.frames[0].as_slice()));
                }
            }
            // Different morphs draw different pixels.
            let images: Vec<_> = (0..count).map(|k| card(Some(k)).pixels).collect();
            let distinct = images
                .iter()
                .enumerate()
                .all(|(i, a)| images[i + 1..].iter().all(|b| a != b));
            assert!(distinct, "{name}: every morph's card differs");
        }
    }

    /// The pixels of `card` below the heading that are in `sprite`, drawn
    /// at 1×: its opaque texels whose colour is not a text colour.
    fn sprite_pixels_in(card: &CardImage, sprite: &plant_sprite::Sprite) -> usize {
        let text = [TITLE, LABEL, BODY, LATIN, BACKGROUND, FRAME];
        let colours: Vec<[u8; 3]> = sprite
            .1
            .iter()
            .filter(|p| p[3] == 255)
            .map(|p| [p[0], p[1], p[2]])
            .filter(|c| !text.contains(c))
            .collect();
        card.pixels
            .chunks(4)
            .filter(|p| colours.contains(&[p[0], p[1], p[2]]))
            .count()
    }

    #[test]
    fn a_plant_card_shows_a_sprite_of_the_mature_plant() {
        let (m, s, p) = tables();
        assert_eq!(p.len(), 18);
        let mut cards = Vec::new();
        for sp in p.iter() {
            let card = build(Card::Plant(sp.id), &m, &s, &p).unwrap();
            let sprite = plant_sprite::build(sp);
            let opaque = sprite.1.iter().filter(|px| px[3] == 255).count();
            // Every texel of the sprite is on the card, at 1× (text may add
            // pixels of the same colour, never take them away).
            assert!(
                sprite_pixels_in(&card, &sprite) >= opaque,
                "{}: the sprite is not on its card",
                sp.name
            );
            // It sits within the card's width, centred under the heading.
            let (w, _) = sprite.0;
            assert!(w <= Page::inner() as usize, "{}", sp.name);
            cards.push((sp.name.clone(), card.pixels));
        }
        // No two plants share a card.
        for (i, (a, pa)) in cards.iter().enumerate() {
            for (b, pb) in &cards[i + 1..] {
                assert_ne!(pa, pb, "{a} and {b}");
            }
        }
    }

    #[test]
    fn every_plant_card_fits_with_its_sprite_tagged_or_not() {
        let (m, s, p) = tables();
        for sp in p.iter() {
            let (size, _) = plant_sprite::build(sp);
            assert!(
                size.0 <= plant_sprite::MAX_WIDTH && size.1 <= plant_sprite::MAX_HEIGHT,
                "{}: {size:?}",
                sp.name
            );
            for tag in [None, Some("2/2")] {
                let image = build_tagged(Card::Plant(sp.id), &m, &s, &p, tag).unwrap();
                assert!(!image.clipped, "{} {tag:?} is cut off", sp.name);
                assert!(image.size.1 <= CARD_MAX.1, "{} {tag:?}", sp.name);
                assert_eq!(
                    image.pixels.len(),
                    (image.size.0 * image.size.1 * 4) as usize
                );
            }
        }
    }

    #[test]
    fn unknown_species_have_no_card() {
        let (m, s, p) = tables();
        assert!(build(Card::Creature(SpeciesId(250), None), &m, &s, &p).is_none());
        assert!(build(Card::Plant(PlantSpeciesId(250)), &m, &s, &p).is_none());
    }

    #[test]
    fn the_hud_texture_holds_both_regions() {
        assert!(HUD_TEXTURE.0 >= hud::MAX_SIZE.0 && HUD_TEXTURE.0 >= CARD_MAX.0);
        assert_eq!(TEXTURE_Y, hud::MAX_SIZE.1);
        assert_eq!(crate::inspect::TEXTURE_Y, TEXTURE_Y + CARD_MAX.1);
        assert_eq!(HUD_TEXTURE.1, crate::inspect::TEXTURE_Y + CARD_MAX.1);
        // Within WebGL2's minimum 2048 texture side.
        assert!(HUD_TEXTURE.0 <= 2048 && HUD_TEXTURE.1 <= 2048);
    }

    fn grid() -> Layout {
        Layout::fit((320, 180), (1280, 720))
    }

    use CardAnchor::{Bottom, Top};

    #[test]
    fn the_card_sits_below_the_palette_at_the_palettes_scale_when_it_fits() {
        // A tall canvas, so the biggest card fits at the palette's scale 4.
        let target = (1280, 1600);
        let g = Layout::fit((320, 400), target);
        let p = PaletteLayout::fit(target, &g, 9, 0, 4, 0);
        assert_eq!(p.scale, 4);
        for anchor in [Top, Bottom] {
            let c = CardLayout::fit(target, &g, Some(&p), anchor, 0, (144, 80)).unwrap();
            assert_eq!(c.scale, p.scale);
            assert_eq!(c.origin.0, p.origin.0);
            // The palette's bottom and a 2-pixel gap at scale 4.
            assert_eq!(c.origin.1, p.origin.1 + p.size().1 + 8);
            assert_eq!(c.size(), (144 * 4, 80 * 4));
            assert!(c.contains(f64::from(c.origin.0), f64::from(c.origin.1)));
            assert!(c.contains(f64::from(c.origin.0 + 575), f64::from(c.origin.1 + 319)));
            assert!(!c.contains(f64::from(c.origin.0 + 576), f64::from(c.origin.1)));
            assert!(!c.contains(f64::from(c.origin.0) - 0.5, f64::from(c.origin.1)));
        }
    }

    #[test]
    fn with_a_palette_the_card_scale_does_not_depend_on_the_card() {
        let g = grid();
        let p = PaletteLayout::fit((1280, 720), &g, 9, 0, 4, 0);
        let scales: Vec<u32> = [(144, 40), (144, 80), (144, 176)]
            .into_iter()
            .map(|image| {
                CardLayout::fit((1280, 720), &g, Some(&p), Top, 0, image)
                    .unwrap()
                    .scale
            })
            .collect();
        assert!(scales.windows(2).all(|w| w[0] == w[1]), "{scales:?}");
        assert!(scales[0] <= p.scale);
    }

    #[test]
    fn without_a_palette_the_card_takes_the_top_left_below_cell_scale() {
        // The biggest card (144×176) at the grid's 4 is 576 of 1280 columns
        // and, with the 8-pixel margin, 712 of 720 rows: it fits.
        let c = CardLayout::fit((1280, 720), &grid(), None, Top, 0, (144, 80)).unwrap();
        assert_eq!((c.scale, c.origin), (4, (8, 8)));
        // A coarse grid still gets at least scale 2 where it fits.
        let g = Layout::fit((320, 180), (320, 180));
        assert_eq!(g.scale, 1);
        let c = CardLayout::fit((320, 180), &g, None, Top, 0, (144, 100)).unwrap();
        assert_eq!(c.scale, 1, "2 doesn't fit 176 rows plus margins in 180");
        let c = CardLayout::fit(
            (1280, 1440),
            &Layout::fit((320, 360), (1280, 1440)),
            None,
            Top,
            0,
            (144, 80),
        )
        .unwrap();
        assert_eq!((c.scale, c.origin), (4, (8, 8)));
    }

    #[test]
    fn above_scale_1_the_biggest_card_covers_at_most_half_the_width() {
        // 144 columns at 4 is 576 of 1280: under half. It may be taller than
        // half the target (712 of 720 rows) as long as it fits.
        let c = CardLayout::fit((1280, 720), &grid(), None, Top, 0, (144, 100)).unwrap();
        assert_eq!(c.scale, 4);
        // Too tall at 4 (720 rows don't fit in 700), so 3.
        let c = CardLayout::fit((1280, 700), &grid(), None, Top, 0, (144, 100)).unwrap();
        assert_eq!(c.scale, 3);
        // A phone-sized canvas: scale 2 (288 of 512 columns) is too wide.
        let g = Layout::fit((128, 72), (512, 288));
        let c = CardLayout::fit((512, 288), &g, None, Top, 0, (144, 100)).unwrap();
        assert_eq!(c.scale, 1, "only scale 1 may cover more than half");
    }

    #[test]
    fn every_card_gets_the_same_scale_on_a_phone() {
        let t = tables();
        let guppy = t.1.id("guppy").unwrap();
        let goldfish = t.1.id("fancy_goldfish").unwrap();
        let cards = every_card(&t);
        let size = |card: Card| cards.iter().find(|c| c.1 == card).unwrap().2.size;
        assert_ne!(
            size(Card::Creature(guppy, None)).1,
            size(Card::Creature(goldfish, None)).1,
            "the guppy and the fancy goldfish differ in height"
        );
        // Canvases about the size of a phone's in device pixels (grid scale
        // 2), from landscape down to a small window, bottom and top anchors.
        for target in [
            (1200, 800),
            (850, 450),
            (1000, 600),
            (700, 380),
            (600, 330),
            (480, 270),
        ] {
            let grid = Layout::fit((target.0 / 2, target.1 / 2), target);
            assert_eq!(grid.scale, 2, "{target:?}");
            for anchor in [Top, Bottom] {
                let scales: Vec<(String, Option<u32>)> = cards
                    .iter()
                    .map(|(name, _, image)| {
                        let c = CardLayout::fit(target, &grid, None, anchor, 0, image.size);
                        (name.clone(), c.map(|c| c.scale))
                    })
                    .collect();
                let first = scales[0].1;
                assert!(first.is_some(), "{target:?}: nothing fits");
                assert!(
                    scales.iter().all(|(_, s)| *s == first),
                    "{target:?} {anchor:?}: {scales:?}"
                );
            }
        }
        // And at the sizes where the guppy used to drop a scale, the two
        // match at 2: readable, 288 of 850 columns, 352 + 4 of 450 rows.
        let target = (850, 450);
        let grid = Layout::fit((425, 225), target);
        let at = |card: Card| CardLayout::fit(target, &grid, None, Top, 0, size(card)).unwrap();
        assert_eq!(at(Card::Creature(guppy, None)).scale, 2);
        assert_eq!(at(Card::Creature(goldfish, None)).scale, 2);
    }

    #[test]
    fn a_host_minimum_scale_raises_the_cap_where_the_card_fits() {
        // A DPR-3 phone in landscape: about 1092×614 device pixels, grid
        // scale 2 (the web's 2× zoom), so scale 2 is only 3 CSS pixels a row.
        let target = (1092, 614);
        let grid = Layout::fit((target.0 / 2, target.1 / 2), target);
        assert_eq!(grid.scale, 2);
        assert_eq!(card_scale(target, &grid, None, 0), 2);
        // Min scale 3 gets scale 3: 432 of 1092 columns, 530 + 12 of 614 rows.
        assert_eq!(card_scale(target, &grid, None, 3), 3);
        for anchor in [Top, Bottom] {
            let c = CardLayout::fit(target, &grid, None, anchor, 3, (144, 80)).unwrap();
            assert_eq!(c.scale, 3);
            let (w, h) = c.size();
            assert!(c.origin.0 + w <= target.0 && c.origin.1 + h <= target.1);
        }
        // A minimum that doesn't fit is not forced: 4 is 576 of 1092
        // columns, over half.
        assert_eq!(card_scale(target, &grid, None, 4), 3);
        // A portrait phone (390×844 at 3×): the minimum 3 is taken.
        let portrait = (1170, 2532);
        let g = Layout::fit((portrait.0 / 2, portrait.1 / 2), portrait);
        assert_eq!(card_scale(portrait, &g, None, 3), 3);
        // Where nothing but 1 fits, the minimum changes nothing.
        let tiny = (200, 120);
        let g = Layout::fit((100, 60), tiny);
        assert_eq!(card_scale(tiny, &g, None, 3), 1);
        // A host that sets none, or a desktop with a big grid scale, is as
        // before.
        let desk = (1280, 720);
        assert_eq!(card_scale(desk, &Layout::fit((320, 180), desk), None, 0), 4);
        assert_eq!(card_scale(desk, &Layout::fit((320, 180), desk), None, 2), 4);
    }

    #[test]
    fn every_card_shares_the_scale_with_a_host_minimum_too() {
        let t = tables();
        let cards = every_card(&t);
        for (target, min) in [((1092, 614), 3), ((1170, 2532), 3), ((1280, 720), 2)] {
            let grid = Layout::fit((target.0 / 2, target.1 / 2), target);
            let want = card_scale(target, &grid, None, min);
            for anchor in [Top, Bottom] {
                for (name, _, image) in &cards {
                    let c = CardLayout::fit(target, &grid, None, anchor, min, image.size)
                        .unwrap_or_else(|| panic!("{name} fits {target:?}"));
                    assert_eq!(c.scale, want, "{name} {target:?} {anchor:?}");
                }
            }
        }
    }

    #[test]
    fn a_bottom_anchored_card_sits_in_the_bottom_left() {
        let target = (1280, 720);
        let g = grid();
        let top = CardLayout::fit(target, &g, None, Top, 0, (144, 80)).unwrap();
        let c = CardLayout::fit(target, &g, None, Bottom, 0, (144, 80)).unwrap();
        assert_eq!(c.scale, top.scale);
        let (width, height) = (f64::from(c.size().0), f64::from(c.size().1));
        // Two card pixels in from the left and bottom edges, at its scale.
        assert_eq!(c.origin.0, 2 * c.scale);
        assert_eq!(c.origin.1 + c.size().1 + 2 * c.scale, target.1);
        assert!(c.origin.1 > top.origin.1);
        // Hit tests follow the drawing.
        let (left, upper) = (f64::from(c.origin.0), f64::from(c.origin.1));
        assert!(c.contains(left, upper));
        assert!(c.contains(left + width - 1.0, upper + height - 1.0));
        assert!(!c.contains(left + width, upper));
        assert!(!c.contains(left, upper - 1.0));
        assert!(!c.contains(left, upper + height));
        let top_left = (f64::from(top.origin.0), f64::from(top.origin.1));
        assert!(!c.contains(top_left.0, top_left.1));
        assert!(top.contains(top_left.0, top_left.1));
        // A shorter card is lower, not at the same top, so it stays on the
        // edge.
        let short = CardLayout::fit(target, &g, None, Bottom, 0, (144, 40)).unwrap();
        assert_eq!(short.scale, c.scale);
        assert_eq!(short.origin.1 + short.size().1 + 2 * short.scale, target.1);
    }

    #[test]
    fn a_palette_in_the_canvas_overrides_the_anchor() {
        let g = grid();
        let p = PaletteLayout::fit((1280, 720), &g, 9, 0, 4, 0);
        let top = CardLayout::fit((1280, 720), &g, Some(&p), Top, 0, (144, 80));
        let bottom = CardLayout::fit((1280, 720), &g, Some(&p), Bottom, 0, (144, 80));
        assert!(top.is_some());
        assert_eq!(top, bottom);
    }

    #[test]
    fn the_card_shrinks_to_fit_and_is_hidden_when_it_cannot() {
        let grid = Layout::fit((320, 180), (1280, 720));
        let palette = PaletteLayout::fit((1280, 720), &grid, 9, 0, 4, 0);
        // Too tall at scale 4 below the palette: shrinks, stays inside.
        let card = CardLayout::fit((1280, 400), &grid, Some(&palette), Top, 0, (144, 100)).unwrap();
        assert!(card.scale < 4);
        let (width, height) = card.size();
        assert!(card.origin.0 + width <= 1280 && card.origin.1 + height <= 400);
        // Bottom-anchored, it stays inside too.
        let card = CardLayout::fit((1280, 400), &grid, None, Bottom, 0, (144, 100)).unwrap();
        let (width, height) = card.size();
        assert!(card.origin.0 + width <= 1280 && card.origin.1 + height <= 400);
        // Too small at scale 1: nothing.
        let grid = Layout::fit((320, 180), (160, 90));
        let palette = PaletteLayout::fit((160, 90), &grid, 9, 0, 4, 0);
        assert_eq!(
            CardLayout::fit((160, 90), &grid, Some(&palette), Top, 0, (144, 100)),
            None
        );
        assert_eq!(
            CardLayout::fit((160, 90), &grid, None, Bottom, 0, (144, 100)),
            None
        );
        assert_eq!(
            CardLayout::fit((0, 0), &grid, None, Top, 0, (144, 100)),
            None
        );
        assert_eq!(
            CardLayout::fit((1280, 720), &grid, None, Top, 0, (0, 0)),
            None
        );
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
                crate::testing::png(&big, (w * scale, h * scale)),
            )
            .unwrap();
        }
    }
}
