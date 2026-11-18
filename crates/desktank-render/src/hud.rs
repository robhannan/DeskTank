//! The interact overlay (SPEC §2.5): a small pixel-art palette in the top-left
//! corner and the brush outline under the pointer.
//!
//! The palette is a tiny RGBA image built on the CPU when the selection
//! changes and drawn by the tank shader at an integer scale, so it adds one
//! texture and no pipeline (WebGL2-safe, SPEC §3.5). Hosts hit-test clicks
//! with [`PaletteLayout`], which places it exactly as it is drawn.

use desktank_sim::tools::{PaletteItem, Tool};
use desktank_sim::{MaterialTable, Placeable, SpeciesTable};

use crate::Layout;

/// Palette pixels per slot side, including its 1-pixel frame.
const SLOT: u32 = 10;
/// Palette pixels between slots and around the strip.
const GAP: u32 = 1;
/// Largest palette image: the HUD texture's size (34 slots).
pub const MAX_SIZE: (u32, u32) = (384, 16);

/// Colours, sRGB.
const BACKGROUND: [u8; 3] = [0x0b, 0x12, 0x1c];
const FRAME: [u8; 3] = [0x2a, 0x36, 0x48];
const SELECTED: [u8; 3] = [0xf4, 0xe9, 0xc8];
const ICON_BG: [u8; 3] = [0x16, 0x22, 0x34];
const ICON_FG: [u8; 3] = [0xd8, 0xe4, 0xf0];

/// 8×8 tool icons, `#` = ink.
const SCOOP: [&str; 8] = [
    "........", ".######.", ".#....#.", ".#....#.", "..####..", "...##...", "...##...", "...##...",
];
const NET: [&str; 8] = [
    "..####..", ".#.#.##.", "#.#.#.#.", ".#.#.##.", "..####..", ".....#..", "......#.", ".......#",
];
const GLUE: [&str; 8] = [
    "...##...", "..####..", "..#..#..", ".######.", ".#....#.", ".#.##.#.", ".#....#.", ".######.",
];
const INSPECT: [&str; 8] = [
    ".####...", "#....#..", "#.#..#..", "#....#..", ".####...", "....##..", ".....##.", "......##",
];

/// Placeable icons: `#` is the device's colour, `o` a bubble or the jet.
const BUBBLER: [&str; 8] = [
    "...o....", ".o....o.", "....o...", "..o..o..", "........", "..####..", ".######.", "........",
];
const OUTLET: [&str; 8] = [
    "........", "........", "###.....", "####.o.o", "####o.o.", "###.....", "........", "........",
];
const BUBBLE: [u8; 3] = [0xe8, 0xf6, 0xff];

/// The palette as the host shows it: the items and which one is selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    /// Slots, left to right.
    pub items: Vec<PaletteItem>,
    /// Index of the selected slot.
    pub selected: Option<usize>,
}

/// The brush outline to draw: centre cell and radius, as
/// [`desktank_sim::tools::Tools::preview`] gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrushPreview {
    /// Centre cell.
    pub cell: (i64, i64),
    /// Radius in cells.
    pub radius: u32,
}

/// Where the palette is drawn in a target, for hit tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaletteLayout {
    /// Top-left in target pixels.
    pub origin: (u32, u32),
    /// Target pixels per palette pixel.
    pub scale: u32,
    /// Number of slots.
    pub slots: usize,
}

impl PaletteLayout {
    /// Places `slots` slots in the top-left of `target`, at the grid's cell
    /// scale (at least 2) or smaller if the strip wouldn't fit.
    #[must_use]
    pub fn fit(target: (u32, u32), grid: &Layout, slots: usize) -> Self {
        let (w, _) = image_size(slots);
        let mut scale = grid.scale.max(2);
        while scale > 1 && (w + 4) * scale > target.0 {
            scale -= 1;
        }
        Self {
            origin: (2 * scale, 2 * scale),
            scale,
            slots,
        }
    }

    /// Size in target pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        let (w, h) = image_size(self.slots);
        (w * self.scale, h * self.scale)
    }

    /// Whether target pixel `(px, py)` is over the palette.
    #[must_use]
    pub fn contains(&self, px: f64, py: f64) -> bool {
        let (w, h) = self.size();
        let (x, y) = (px - f64::from(self.origin.0), py - f64::from(self.origin.1));
        x >= 0.0 && y >= 0.0 && x < f64::from(w) && y < f64::from(h)
    }

    /// The slot under target pixel `(px, py)`, if any.
    #[must_use]
    pub fn hit(&self, px: f64, py: f64) -> Option<usize> {
        if !self.contains(px, py) {
            return None;
        }
        let s = f64::from(self.scale);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // inside the strip
        let (x, y) = (
            ((px - f64::from(self.origin.0)) / s) as u32,
            ((py - f64::from(self.origin.1)) / s) as u32,
        );
        let slot = usize::try_from(x.checked_sub(GAP)? / (SLOT + GAP)).ok()?;
        let within = (x - GAP) % (SLOT + GAP) < SLOT && (GAP..GAP + SLOT).contains(&y);
        (within && slot < self.slots).then_some(slot)
    }
}

/// Palette image size in palette pixels.
#[must_use]
pub fn image_size(slots: usize) -> (u32, u32) {
    let n = u32::try_from(slots).unwrap_or(u32::MAX / (SLOT + GAP));
    (
        (GAP + n * (SLOT + GAP)).min(MAX_SIZE.0),
        (SLOT + 2 * GAP).min(MAX_SIZE.1),
    )
}

/// Draws the palette as RGBA rows (alpha 255 = drawn, 0 = transparent).
/// Slots that don't fit [`MAX_SIZE`] are left out.
#[must_use]
pub fn build(
    palette: &Palette,
    materials: &MaterialTable,
    species: &SpeciesTable,
) -> (Vec<u8>, (u32, u32)) {
    let (w, h) = image_size(palette.items.len());
    let mut img = vec![0u8; (w * h * 4) as usize];
    let mut put = |x: u32, y: u32, c: [u8; 3]| {
        if x < w && y < h {
            let i = ((y * w + x) * 4) as usize;
            img[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    };
    for y in 0..h {
        for x in 0..w {
            put(x, y, BACKGROUND);
        }
    }
    for (i, item) in palette.items.iter().enumerate() {
        let Ok(i) = u32::try_from(i) else { break };
        let x0 = GAP + i * (SLOT + GAP);
        if x0 + SLOT > w {
            break;
        }
        let frame = if palette.selected == Some(i as usize) {
            SELECTED
        } else {
            FRAME
        };
        for d in 0..SLOT {
            for (x, y) in [(d, 0), (d, SLOT - 1), (0, d), (SLOT - 1, d)] {
                put(x0 + x, GAP + y, frame);
            }
        }
        for y in 0..SLOT - 2 {
            for x in 0..SLOT - 2 {
                put(
                    x0 + 1 + x,
                    GAP + 1 + y,
                    slot_pixel(*item, materials, species, x, y),
                );
            }
        }
    }
    (img, (w, h))
}

/// Pixel `(x, y)` of a slot's 8×8 face.
fn slot_pixel(
    item: PaletteItem,
    materials: &MaterialTable,
    species: &SpeciesTable,
    x: u32,
    y: u32,
) -> [u8; 3] {
    match item {
        // The species' sprite, centred.
        PaletteItem::Tool(Tool::AddCreature(s)) => {
            crate::sprites::icon(species, s, x, y).unwrap_or(ICON_BG)
        }
        PaletteItem::Material(id) => {
            let c = materials.get(id).map_or([0xff, 0, 0xff], |m| m.colour);
            // A few darker and lighter specks, like the grid's variants.
            match (x * 5 + y * 3) % 11 {
                0 => c.map(|v| v.saturating_sub(24)),
                5 => c.map(|v| v.saturating_add(18)),
                _ => c,
            }
        }
        PaletteItem::Tool(Tool::Place(p)) => {
            let icon = match p {
                Placeable::Bubbler => BUBBLER,
                Placeable::FilterOutlet => OUTLET,
            };
            let device = materials
                .id(p.material_name())
                .and_then(|id| materials.get(id))
                .map_or(ICON_FG, |m| m.colour);
            match icon[y as usize].as_bytes().get(x as usize) {
                Some(b'#') => device.map(|v| v.saturating_add(40)),
                Some(b'o') => BUBBLE,
                _ => ICON_BG,
            }
        }
        PaletteItem::Tool(t) => {
            let icon = match t {
                Tool::Scoop | Tool::Pour | Tool::Place(_) | Tool::AddCreature(_) => SCOOP,
                Tool::Net => NET,
                Tool::Inspect => INSPECT,
                Tool::Glue => GLUE,
            };
            let ink = icon[y as usize].as_bytes().get(x as usize) == Some(&b'#');
            if ink { ICON_FG } else { ICON_BG }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Layout {
        Layout::fit((320, 180), (1280, 720))
    }

    #[test]
    fn palette_sits_top_left_at_cell_scale() {
        let p = PaletteLayout::fit((1280, 720), &grid(), 9);
        assert_eq!(p.scale, 4);
        assert_eq!(p.origin, (8, 8));
        assert_eq!(p.size(), ((1 + 9 * 11) * 4, 12 * 4));
    }

    #[test]
    fn hit_maps_pixels_to_slots() {
        let p = PaletteLayout::fit((1280, 720), &grid(), 9);
        // Slot 0 spans palette x 1..11, i.e. target 12..52 (origin 8, scale 4).
        assert_eq!(p.hit(12.0, 12.0), Some(0));
        assert_eq!(p.hit(51.9, 50.0), Some(0));
        assert_eq!(p.hit(53.0, 20.0), None); // gap
        assert_eq!(p.hit(56.0, 20.0), Some(1));
        assert_eq!(p.hit(8.0 + 4.0 * 95.0, 20.0), Some(8));
        assert_eq!(p.hit(9.0, 20.0), None); // leading gap
        assert_eq!(p.hit(500.0, 20.0), None);
        assert!(p.contains(9.0, 9.0));
        assert!(!p.contains(4.0, 20.0));
    }

    #[test]
    fn shrinks_to_fit_a_narrow_target() {
        let p = PaletteLayout::fit((200, 100), &Layout::fit((50, 25), (200, 100)), 9);
        assert_eq!(p.scale, 1);
    }

    #[test]
    fn the_whole_default_palette_fits() {
        let w = desktank_sim::World::new(
            desktank_sim::WorldConfig::default(),
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let n = desktank_sim::tools::Tools::palette(&w).len();
        assert_eq!(
            image_size(n).0,
            GAP + u32::try_from(n).unwrap() * (SLOT + GAP)
        );
    }

    #[test]
    fn image_frames_the_selection() {
        let mats = MaterialTable::builtin().unwrap();
        let sand = mats.id("sand").unwrap();
        let palette = Palette {
            items: vec![PaletteItem::Material(sand), PaletteItem::Tool(Tool::Net)],
            selected: Some(1),
        };
        let species = desktank_sim::SpeciesTable::builtin_for(&mats).unwrap();
        let (img, (w, h)) = build(&palette, &mats, &species);
        assert_eq!((w, h), (23, 12));
        let px = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [img[i], img[i + 1], img[i + 2]]
        };
        assert_eq!(px(1, 1), FRAME);
        assert_eq!(px(12, 1), SELECTED);
        let sand_colour = mats.get(sand).unwrap().colour;
        assert_eq!(px(3, 3), sand_colour);
        assert!(img.chunks(4).all(|p| p[3] == 255));
    }
}
