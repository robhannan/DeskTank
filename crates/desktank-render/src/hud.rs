//! The interact overlay (SPEC §2.5): a small pixel-art palette in the top-left
//! corner and the brush outline under the pointer.
//!
//! The palette is two rows of slots: the group tabs on top (each showing its
//! group's first item) and the open group's items below. It is a tiny RGBA
//! image built on the CPU when the selection or the open group changes and
//! drawn by the tank shader at an integer scale, so it adds one texture and
//! no pipeline (WebGL2-safe, SPEC §3.5). Pixels outside the rows are
//! transparent. Hosts hit-test clicks with [`PaletteLayout`], which places it
//! exactly as it is drawn.

use desktank_sim::tools::{PaletteItem, Tool};
use desktank_sim::{MaterialTable, Placeable, PlantSpeciesId, PlantSpeciesTable, SpeciesTable};

use crate::Layout;

/// Palette pixels per slot side, including its 1-pixel frame.
const SLOT: u32 = 10;
/// Palette pixels between slots and around the rows.
const GAP: u32 = 1;
/// Palette pixels per row: its leading gap and its slots.
const ROW: u32 = GAP + SLOT;
/// Largest palette image: the HUD texture's size (23 slots per row).
pub const MAX_SIZE: (u32, u32) = (256, 24);

/// Colours, sRGB.
const BACKGROUND: [u8; 3] = [0x0b, 0x12, 0x1c];
const FRAME: [u8; 3] = [0x2a, 0x36, 0x48];
const SELECTED: [u8; 3] = [0xf4, 0xe9, 0xc8];
const OPEN: [u8; 3] = [0x6f, 0xb3, 0xe0];
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
const MAGNIFY: [&str; 8] = [
    ".#####..", "#..#..#.", "#.###.#.", "#..#..#.", ".#####..", ".....##.", "......##", ".......#",
];

/// Placeable icons: `#` is the device's colour, `o` a bubble or the jet.
const BUBBLER: [&str; 8] = [
    "...o....", ".o....o.", "....o...", "..o..o..", "........", "..####..", ".######.", "........",
];
const OUTLET: [&str; 8] = [
    "........", "........", "###.....", "####.o.o", "####o.o.", "###.....", "........", "........",
];
const BUBBLE: [u8; 3] = [0xe8, 0xf6, 0xff];

/// The plants slot: a sprout, `#` in the picked species' colour, `s` its
/// stem, `o` a seed.
const SPROUT: [&str; 8] = [
    "...##...", "..#s##..", ".##s.#..", "...s....", ".#.s.##.", "..#s#...", "...s....", ".oo.oo..",
];
const SEED: [u8; 3] = [0xc8, 0xa8, 0x5a];

/// The palette as the host shows it: the group tabs, the open group's items
/// and the selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    /// One tab per group, left to right: the item whose face it shows
    /// (the group's first).
    pub tabs: Vec<PaletteItem>,
    /// Index of the open tab.
    pub open: usize,
    /// The open group's items, left to right.
    pub items: Vec<PaletteItem>,
    /// The selection: its group's tab and its index in that group. Framed
    /// in the item row while its group is open, marked on its tab when not.
    pub selected: Option<(usize, usize)>,
    /// The plant species the plants slot pours, which colours its icon.
    pub plant: Option<PlantSpeciesId>,
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

/// The magnified lens to draw around the pointer: centre cell and
/// magnification factor, as [`desktank_sim::tools::Tools::magnifier`] gives
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Magnifier {
    /// Centre cell.
    pub cell: (i64, i64),
    /// Magnification factor (`2`, `4`, `6`, `8`).
    pub zoom: u32,
}

/// Where the circular magnifying glass is drawn in a target, in target pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MagnifierLayout {
    /// Centre of the circular glass lens in target pixels.
    pub centre: (u32, u32),
    /// Inner glass radius in target pixels.
    pub radius: u32,
    /// Pixel-art block size for the bezel and handle, in target pixels.
    pub step: u32,
    /// Tight bounding box of the circular glass lens (for scissoring the
    /// magnified grid and sprites).
    pub lens_origin: (u32, u32),
    /// Width and height of the glass lens bounding box.
    pub lens_size: (u32, u32),
    /// Bounding box of the full magnifying glass (bezel + handle, clamped to
    /// the target) for scissoring `fs_lens_frame`.
    pub frame_origin: (u32, u32),
    /// Width and height of the full magnifying glass bounding box.
    pub frame_size: (u32, u32),
    /// Target pixels per cell inside the lens (`grid.scale * zoom`).
    pub scale: u32,
    /// Top-left of the grid in target pixels for the magnified view.
    pub offset: (i64, i64),
}

/// Smallest and largest lens diameter in target pixels (before clamping to
/// the target itself).
const MIN_LENS_SIDE: u32 = 48;
const MAX_LENS_SIDE: u32 = 440;
/// Outer bezel width in art pixels (4 px rim + 2 px drop shadow).
const BEZEL_ART_PX: u32 = 6;
/// Handle length bounds in art pixels (matches `fs_lens_frame` in `tank.wgsl`).
const MIN_HANDLE_ART_PX: u32 = 14;
const MAX_HANDLE_ART_PX: u32 = 44;

impl MagnifierLayout {
    /// Places a circular magnifying glass for `m` over `grid` inside `target`
    /// pixels. Returns `None` if `target` is empty or `m.zoom <= 1`.
    #[must_use]
    pub fn fit(target: (u32, u32), grid: &Layout, m: Magnifier) -> Option<Self> {
        if target.0 == 0 || target.1 == 0 || m.zoom <= 1 {
            return None;
        }
        let min_dim = target.0.min(target.1);
        let step = if min_dim >= 900 {
            3
        } else if min_dim >= 300 {
            2
        } else {
            1
        };
        let pad = BEZEL_ART_PX * step;
        let max_diameter = min_dim.saturating_sub(2 * pad);
        if max_diameter < 4 {
            return None;
        }
        let raw = (min_dim * 2 / 5)
            .clamp(MIN_LENS_SIDE, MAX_LENS_SIDE)
            .min(max_diameter);
        let side = raw & !1;
        let radius = side / 2;
        if radius == 0 {
            return None;
        }
        let margin = i64::from(radius + pad);
        let gs = i64::from(grid.scale);
        let target_pt = (
            grid.offset.0 + m.cell.0 * gs + gs / 2,
            grid.offset.1 + m.cell.1 * gs + gs / 2,
        );
        let cx = target_pt.0.clamp(margin, i64::from(target.0) - margin);
        let cy = target_pt.1.clamp(margin, i64::from(target.1) - margin);
        let scale = grid.scale.saturating_mul(m.zoom);
        let ms = i64::from(scale);
        let offset = (cx - m.cell.0 * ms - ms / 2, cy - m.cell.1 * ms - ms / 2);

        let r_art = radius / step;
        let handle_art = (r_art * 72 / 100).clamp(MIN_HANDLE_ART_PX, MAX_HANDLE_ART_PX);
        // Handle extends down-right at 45° from the rim; leave room for its
        // half-width and outline.
        let reach = i64::from(radius + pad + (handle_art + 8) * step);
        let fx0 = (cx - margin).clamp(0, i64::from(target.0));
        let fy0 = (cy - margin).clamp(0, i64::from(target.1));
        let fx1 = (cx + reach).clamp(0, i64::from(target.0));
        let fy1 = (cy + reach).clamp(0, i64::from(target.1));

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0..=target
        Some(Self {
            centre: (cx as u32, cy as u32),
            radius,
            step,
            lens_origin: (
                (cx - i64::from(radius)) as u32,
                (cy - i64::from(radius)) as u32,
            ),
            lens_size: (side, side),
            frame_origin: (fx0 as u32, fy0 as u32),
            frame_size: ((fx1 - fx0) as u32, (fy1 - fy0) as u32),
            scale,
            offset,
        })
    }
}

/// What a click on the palette hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteHit {
    /// A group tab (top row).
    Tab(usize),
    /// An item of the open group (bottom row).
    Item(usize),
}

/// Where the palette is drawn in a target, for hit tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaletteLayout {
    /// Top-left in target pixels.
    pub origin: (u32, u32),
    /// Target pixels per palette pixel.
    pub scale: u32,
    /// Number of group tabs (top row).
    pub tabs: usize,
    /// Number of items in the open group (bottom row).
    pub items: usize,
}

impl PaletteLayout {
    /// Places a palette of `tabs` tabs over `items` items in the top-left
    /// of `target`, at the grid's cell scale (at least 2) or smaller if it
    /// wouldn't fit.
    #[must_use]
    pub fn fit(target: (u32, u32), grid: &Layout, tabs: usize, items: usize) -> Self {
        let (w, _) = image_size(tabs, items);
        let mut scale = grid.scale.max(2);
        while scale > 1 && (w + 4) * scale > target.0 {
            scale -= 1;
        }
        Self {
            origin: (2 * scale, 2 * scale),
            scale,
            tabs,
            items,
        }
    }

    /// Size of the image's bounding box in target pixels; parts of it
    /// right of the shorter row are transparent.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        let (w, h) = image_size(self.tabs, self.items);
        (w * self.scale, h * self.scale)
    }

    /// The palette pixel under target pixel `(px, py)`, if inside the
    /// bounding box.
    fn pixel(&self, px: f64, py: f64) -> Option<(u32, u32)> {
        let (width, height) = image_size(self.tabs, self.items);
        let scale = f64::from(self.scale);
        let (x, y) = (
            (px - f64::from(self.origin.0)) / scale,
            (py - f64::from(self.origin.1)) / scale,
        );
        let inside = x >= 0.0 && y >= 0.0 && x < f64::from(width) && y < f64::from(height);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // inside the image
        inside.then_some((x as u32, y as u32))
    }

    /// Whether target pixel `(px, py)` is over the drawn palette (either
    /// row, not the transparent part beside the shorter one).
    #[must_use]
    pub fn contains(&self, px: f64, py: f64) -> bool {
        self.pixel(px, py)
            .is_some_and(|(x, y)| x < row_width(self.row_slots(y / ROW)))
    }

    fn row_slots(&self, row: u32) -> usize {
        if row == 0 { self.tabs } else { self.items }
    }

    /// The tab or item under target pixel `(px, py)`, if any.
    #[must_use]
    pub fn hit(&self, px: f64, py: f64) -> Option<PaletteHit> {
        let (x, y) = self.pixel(px, py)?;
        let row = y / ROW;
        let slot = usize::try_from(x.checked_sub(GAP)? / ROW).ok()?;
        let within = (x - GAP) % ROW < SLOT && y % ROW >= GAP && row < 2;
        if !within || slot >= self.row_slots(row) {
            return None;
        }
        Some(if row == 0 {
            PaletteHit::Tab(slot)
        } else {
            PaletteHit::Item(slot)
        })
    }
}

/// Width in palette pixels of a row of `slots` slots, capped at
/// [`MAX_SIZE`].
fn row_width(slots: usize) -> u32 {
    let n = u32::try_from(slots).unwrap_or(u32::MAX / ROW);
    (GAP + n.saturating_mul(ROW)).min(MAX_SIZE.0)
}

/// Palette image size in palette pixels: two rows, as wide as the longer.
#[must_use]
pub fn image_size(tabs: usize, items: usize) -> (u32, u32) {
    (row_width(tabs.max(items)), (2 * ROW + GAP).min(MAX_SIZE.1))
}

/// Draws the palette as RGBA rows (alpha 255 = drawn, 0 = transparent).
/// Slots that don't fit [`MAX_SIZE`] are left out.
#[must_use]
pub fn build(
    palette: &Palette,
    materials: &MaterialTable,
    species: &SpeciesTable,
    plants: &PlantSpeciesTable,
) -> (Vec<u8>, (u32, u32)) {
    let plant = palette
        .plant
        .and_then(|id| plants.get(id))
        .or_else(|| plants.default_species())
        .map(|s| (s.colour(), s.colours.stem[s.colours.stem.len() / 2]));
    let (w, h) = image_size(palette.tabs.len(), palette.items.len());
    let mut img = vec![0u8; (w * h * 4) as usize];
    let mut put = |x: u32, y: u32, c: [u8; 3]| {
        if x < w && y < h {
            let i = ((y * w + x) * 4) as usize;
            img[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    };
    let (sel_tab, sel_item) = palette.selected.unzip();
    let rows = [(&palette.tabs, 0), (&palette.items, 1)];
    for (items, row) in rows {
        let y0 = row * ROW;
        // The row's background, including the gap under the item row.
        let bottom = if row == 0 { ROW } else { h - y0 };
        for y in 0..bottom {
            for x in 0..row_width(items.len()) {
                put(x, y0 + y, BACKGROUND);
            }
        }
        for (i, item) in items.iter().enumerate() {
            let Ok(n) = u32::try_from(i) else { break };
            let x0 = GAP + n * ROW;
            if x0 + SLOT > w {
                break;
            }
            let top = y0 + GAP;
            let frame = match row {
                0 if i == palette.open => OPEN,
                1 if sel_tab == Some(palette.open) && sel_item == Some(i) => SELECTED,
                _ => FRAME,
            };
            for d in 0..SLOT {
                for (x, y) in [(d, 0), (d, SLOT - 1), (0, d), (SLOT - 1, d)] {
                    put(x0 + x, top + y, frame);
                }
            }
            for y in 0..SLOT - 2 {
                for x in 0..SLOT - 2 {
                    let px = match item {
                        PaletteItem::Tool(Tool::Plant) => sprout(plant, x, y),
                        _ => slot_pixel(*item, materials, species, x, y),
                    };
                    put(x0 + 1 + x, top + 1 + y, px);
                }
            }
            // A closed tab holding the selection: a mark on its bottom edge.
            if row == 0 && i != palette.open && sel_tab == Some(i) {
                for x in 3..SLOT - 3 {
                    put(x0 + x, top + SLOT - 1, SELECTED);
                }
            }
        }
    }
    (img, (w, h))
}

/// Pixel `(x, y)` of the plants slot's face, in `colours` (leaf, stem).
fn sprout(colours: Option<([u8; 3], [u8; 3])>, x: u32, y: u32) -> [u8; 3] {
    let (leaf, stem) = colours.unwrap_or((ICON_FG, ICON_FG));
    match SPROUT[y as usize].as_bytes().get(x as usize) {
        Some(b'#') => leaf,
        Some(b's') => stem,
        Some(b'o') => SEED,
        _ => ICON_BG,
    }
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
                Tool::Scoop | Tool::Pour | Tool::Place(_) | Tool::AddCreature(_) | Tool::Plant => {
                    SCOOP
                }
                Tool::Net => NET,
                Tool::Inspect => INSPECT,
                Tool::Glue => GLUE,
                Tool::Magnify => MAGNIFY,
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
        let p = PaletteLayout::fit((1280, 720), &grid(), 9, 4);
        assert_eq!(p.scale, 4);
        assert_eq!(p.origin, (8, 8));
        assert_eq!(p.size(), ((1 + 9 * 11) * 4, 23 * 4));
    }

    #[test]
    fn hit_maps_pixels_to_tabs_and_items() {
        let p = PaletteLayout::fit((1280, 720), &grid(), 9, 4);
        // Tab 0 spans palette x 1..11, y 1..11, i.e. target 12..52 (origin
        // 8, scale 4); item 0 is the row below, palette y 12..22.
        assert_eq!(p.hit(12.0, 12.0), Some(PaletteHit::Tab(0)));
        assert_eq!(p.hit(51.9, 51.9), Some(PaletteHit::Tab(0)));
        assert_eq!(p.hit(53.0, 20.0), None); // gap
        assert_eq!(p.hit(56.0, 20.0), Some(PaletteHit::Tab(1)));
        assert_eq!(p.hit(8.0 + 4.0 * 95.0, 20.0), Some(PaletteHit::Tab(8)));
        assert_eq!(p.hit(9.0, 20.0), None); // leading gap
        assert_eq!(p.hit(20.0, 54.0), None); // gap between the rows
        assert_eq!(p.hit(20.0, 8.0 + 4.0 * 12.0), Some(PaletteHit::Item(0)));
        assert_eq!(p.hit(8.0 + 4.0 * 40.0, 60.0), Some(PaletteHit::Item(3)));
        assert_eq!(p.hit(8.0 + 4.0 * 50.0, 60.0), None); // past the items
        assert_eq!(p.hit(20.0, 8.0 + 4.0 * 22.5), None); // bottom gap
        assert_eq!(p.hit(500.0, 20.0), None);
        assert!(p.contains(9.0, 9.0));
        assert!(!p.contains(4.0, 20.0));
        // Beside the shorter item row the tank shows through.
        assert!(p.contains(8.0 + 4.0 * 50.0, 20.0));
        assert!(!p.contains(8.0 + 4.0 * 50.0, 60.0));
    }

    #[test]
    fn shrinks_to_fit_a_narrow_target() {
        let p = PaletteLayout::fit((200, 100), &Layout::fit((50, 25), (200, 100)), 9, 3);
        assert_eq!(p.scale, 1);
    }

    #[test]
    fn every_default_group_fits() {
        let w = desktank_sim::World::new(
            desktank_sim::WorldConfig::default(),
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let groups = desktank_sim::tools::Tools::palette_groups(&w);
        let widest = groups.iter().map(|(_, i)| i.len()).max().unwrap();
        let n = groups.len().max(widest);
        assert_eq!(image_size(groups.len(), widest).0, row_width(n));
        assert!(GAP + u32::try_from(n).unwrap() * ROW <= MAX_SIZE.0);
    }

    #[test]
    fn image_frames_the_open_tab_and_the_selection() {
        let mats = MaterialTable::builtin().unwrap();
        let sand = mats.id("sand").unwrap();
        let water = mats.id("water").unwrap();
        let px = |img: &[u8], w: u32, x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [img[i], img[i + 1], img[i + 2], img[i + 3]]
        };
        let species = desktank_sim::SpeciesTable::builtin_for(&mats).unwrap();
        let plants = PlantSpeciesTable::builtin_for(&mats).unwrap();
        let mut palette = Palette {
            tabs: vec![
                PaletteItem::Tool(Tool::Scoop),
                PaletteItem::Material(sand),
                PaletteItem::Material(water),
            ],
            open: 1,
            items: vec![PaletteItem::Material(sand), PaletteItem::Tool(Tool::Net)],
            selected: Some((1, 1)),
            plant: None,
        };
        let (img, (w, h)) = build(&palette, &mats, &species, &plants);
        assert_eq!((w, h), (34, 23));
        let rgb = |x, y| {
            let p = px(&img, w, x, y);
            [p[0], p[1], p[2]]
        };
        assert_eq!(rgb(1, 1), FRAME, "closed tab");
        assert_eq!(rgb(12, 1), OPEN, "open tab");
        assert_eq!(rgb(1, 12), FRAME);
        assert_eq!(rgb(12, 12), SELECTED, "selected item");
        let sand_colour = mats.get(sand).unwrap().colour;
        assert_eq!(rgb(3, 14), sand_colour);
        assert_eq!(rgb(2, 13), sand_colour.map(|v| v.saturating_sub(24)));
        // Beside the two-item row the image is transparent.
        assert_eq!(px(&img, w, 30, 15)[3], 0);
        assert_eq!(px(&img, w, 30, 5)[3], 255);

        // Another group open: the selection marks its tab instead.
        palette.open = 2;
        let (img, _) = build(&palette, &mats, &species, &plants);
        assert_eq!(
            px(&img, w, 16, 10)[..3],
            SELECTED,
            "mark on the selection's tab"
        );
        assert_eq!(
            px(&img, w, 12, 12)[..3],
            FRAME,
            "nothing selected in this group"
        );

        // The plants slot shows the picked species: the default, then moss.
        palette.items = vec![PaletteItem::Tool(Tool::Plant)];
        let leaf = |palette: &Palette| {
            let (img, (w, _)) = build(palette, &mats, &species, &plants);
            let p = px(&img, w, GAP + 1 + 3, ROW + GAP + 1);
            [p[0], p[1], p[2]]
        };
        assert_eq!(leaf(&palette), plants.default_species().unwrap().colour());
        palette.plant = plants.id("java_moss");
        let moss = plants.get(palette.plant.unwrap()).unwrap();
        assert_eq!(leaf(&palette), moss.colour());
    }

    #[test]
    fn magnifier_layout_centres_cell_and_clamps_to_target() {
        let g = grid(); // 320×180 at scale 4 in 1280×720
        assert_eq!(
            MagnifierLayout::fit(
                (1280, 720),
                &g,
                Magnifier {
                    cell: (160, 90),
                    zoom: 1
                }
            ),
            None
        );
        assert_eq!(
            MagnifierLayout::fit(
                (0, 720),
                &g,
                Magnifier {
                    cell: (160, 90),
                    zoom: 2
                }
            ),
            None
        );

        // Centred in the middle of the tank.
        let m = MagnifierLayout::fit(
            (1280, 720),
            &g,
            Magnifier {
                cell: (160, 90),
                zoom: 2,
            },
        )
        .unwrap();
        assert_eq!(m.lens_size, (288, 288));
        assert_eq!(m.radius, 144);
        assert_eq!(m.scale, 8);
        assert!(m.lens_origin.0 + m.lens_size.0 <= 1280);
        assert!(m.lens_origin.1 + m.lens_size.1 <= 720);
        assert!(m.frame_origin.0 + m.frame_size.0 <= 1280);
        assert!(m.frame_origin.1 + m.frame_size.1 <= 720);
        // The lens centre maps to the exact centre of cell (160, 90).
        let cx = i64::from(m.centre.0);
        let cy = i64::from(m.centre.1);
        assert_eq!((cx - m.offset.0) / i64::from(m.scale), 160);
        assert_eq!((cy - m.offset.1) / i64::from(m.scale), 90);
        // Even scale means exact half-cell alignment.
        assert_eq!(
            (cx - m.offset.0) % i64::from(m.scale),
            i64::from(m.scale / 2)
        );

        // Near the top-left corner: rim stays inside the target while still
        // centring cell (1, 1) at the lens centre.
        let corner = MagnifierLayout::fit(
            (1280, 720),
            &g,
            Magnifier {
                cell: (1, 1),
                zoom: 4,
            },
        )
        .unwrap();
        assert_eq!(corner.frame_origin, (0, 0));
        assert_eq!(corner.scale, 16);
        let ccx = i64::from(corner.centre.0);
        let ccy = i64::from(corner.centre.1);
        assert_eq!((ccx - corner.offset.0) / i64::from(corner.scale), 1);
        assert_eq!((ccy - corner.offset.1) / i64::from(corner.scale), 1);

        // Near the bottom-right corner: lens and frame stay inside target bounds.
        let br = MagnifierLayout::fit(
            (1280, 720),
            &g,
            Magnifier {
                cell: (319, 179),
                zoom: 8,
            },
        )
        .unwrap();
        assert!(br.lens_origin.0 + br.lens_size.0 <= 1280);
        assert!(br.lens_origin.1 + br.lens_size.1 <= 720);
        assert!(br.frame_origin.0 + br.frame_size.0 <= 1280);
        assert!(br.frame_origin.1 + br.frame_size.1 <= 720);
        assert_eq!(br.scale, 32);
    }
}
