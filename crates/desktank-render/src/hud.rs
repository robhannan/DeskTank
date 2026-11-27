//! The interact overlay (SPEC §2.5): a small pixel-art palette in the top-left
//! corner and the brush outline under the pointer.
//!
//! The palette is up to four rows of slots: the top tabs (each showing its
//! category's first item), the open tab's sub-tabs (only when it has more
//! than one sub-category, each showing its first item), the open
//! sub-category's items and, under a selected multi-morph creature, its
//! colour morphs. It is a tiny RGBA image built on the CPU when the
//! selection or the open tabs change and drawn by the tank shader at an
//! integer scale, so it adds one texture and no pipeline (WebGL2-safe, SPEC
//! §3.5). Pixels outside the rows are transparent. Hosts hit-test clicks
//! with [`PaletteLayout`], which places it exactly as it is drawn.

use desktank_sim::tools::{PaletteItem, Tool};
use desktank_sim::{DeviceKinds, MaterialTable, PlantSpeciesTable, SpeciesTable};

use crate::Layout;

/// Palette pixels per slot side, including its 1-pixel frame.
const SLOT: u32 = 10;
/// Palette pixels between slots and around the rows.
const GAP: u32 = 1;
/// Palette pixels per row: its leading gap and its slots.
const ROW: u32 = GAP + SLOT;
/// Most rows: tabs, sub-tabs, items and morphs.
const MAX_ROWS: u32 = 4;
/// Largest palette image: the HUD texture's size (23 slots per row, four
/// rows).
pub const MAX_SIZE: (u32, u32) = (256, MAX_ROWS * ROW + GAP);

/// Colours, sRGB.
pub(crate) const BACKGROUND: [u8; 3] = [0x0b, 0x12, 0x1c];
pub(crate) const FRAME: [u8; 3] = [0x2a, 0x36, 0x48];
pub(crate) const SELECTED: [u8; 3] = [0xf4, 0xe9, 0xc8];
pub(crate) const OPEN: [u8; 3] = [0x6f, 0xb3, 0xe0];
/// The open tab's panel: the sub-tab and item rows sit on it, and the open
/// tab opens into it (no bottom edge), so the rows read as that tab's.
pub(crate) const PANEL: [u8; 3] = [0x2b, 0x48, 0x66];
/// The open sub-tab's frame: unlike the tabs' blue, so the two rows of
/// categories don't read as one.
const SUB_OPEN: [u8; 3] = [0x7f, 0xd6, 0xb0];
pub(crate) const ICON_BG: [u8; 3] = [0x16, 0x22, 0x34];
pub(crate) const ICON_FG: [u8; 3] = [0xd8, 0xe4, 0xf0];

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

/// A plant slot: a sprout, `#` in its species' colour, `s` its stem, `o` a
/// seed.
const SPROUT: [&str; 8] = [
    "...##...", "..#s##..", ".##s.#..", "...s....", ".#.s.##.", "..#s#...", "...s....", ".oo.oo..",
];
pub(crate) const SEED: [u8; 3] = [0xc8, 0xa8, 0x5a];

/// A selected multi-morph creature's morph row, as the host shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaletteMorphs {
    /// The creature species whose morph icons the row shows.
    pub species: desktank_sim::SpeciesId,
    /// Number of morphs, one slot each.
    pub count: usize,
    /// Whether a leading slot stocks the morphs in turn (automatic).
    pub cycle: bool,
    /// The picked morph, or `None` for automatic stocking.
    pub selected: Option<u8>,
}

impl PaletteMorphs {
    /// Slots in the row: the morphs, plus the automatic slot if it cycles.
    #[must_use]
    pub fn slots(&self) -> usize {
        self.count + usize::from(self.cycle)
    }

    /// The row's selected slot.
    fn framed(&self) -> Option<usize> {
        match (self.cycle, self.selected) {
            (true, None) => Some(0),
            (true, Some(m)) => Some(usize::from(m) + 1),
            (false, m) => m.map(usize::from),
        }
    }

    /// What slot `slot` shows: `None` for the automatic slot, else its
    /// morph.
    fn morph_at(&self, slot: usize) -> Option<u8> {
        let morph = slot.checked_sub(usize::from(self.cycle))?;
        Some(u8::try_from(morph).unwrap_or(u8::MAX))
    }
}

/// The palette as the host shows it: the top tabs, the open tab's
/// sub-tabs, the open sub-category's items and the selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    /// One tab per top category, left to right: the item whose face it
    /// shows (the category's first).
    pub tabs: Vec<PaletteItem>,
    /// Index of the open tab.
    pub open: usize,
    /// One sub-tab per sub-category of the open tab (its first item's
    /// face), or none when it has only one: then no sub-tab row is drawn.
    pub subs: Vec<PaletteItem>,
    /// Index of the open sub-category within the open tab.
    pub open_sub: usize,
    /// The open sub-category's items, left to right.
    pub items: Vec<PaletteItem>,
    /// The selection: its tab, its sub-category in that tab and its index
    /// there. Framed in the item row while its sub-category is open,
    /// marked on its sub-tab or tab when not.
    pub selected: Option<(usize, usize, usize)>,
    /// The selected creature's morph row, shown under the items.
    pub morphs: Option<PaletteMorphs>,
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
    /// A top tab (first row).
    Tab(usize),
    /// A sub-tab of the open tab (second row, when shown).
    Sub(usize),
    /// An item of the open sub-category.
    Item(usize),
    /// A slot of the morph row (last row, when shown): the automatic slot
    /// first if the species cycles.
    Morph(usize),
}

/// Where the palette is drawn in a target, for hit tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaletteLayout {
    /// Top-left in target pixels.
    pub origin: (u32, u32),
    /// Target pixels per palette pixel.
    pub scale: u32,
    /// Number of top tabs (first row).
    pub tabs: usize,
    /// Number of sub-tabs (second row; 0: no sub-tab row).
    pub subs: usize,
    /// Number of items in the open sub-category.
    pub items: usize,
    /// Number of slots in the morph row (0: no morph row).
    pub morphs: usize,
}

impl PaletteLayout {
    /// Places a palette of `tabs` tabs, `subs` sub-tabs (0 for none),
    /// `items` items and `morphs` morph slots (0 for none) in the top-left
    /// of `target`, at the grid's cell scale (at least 2) or smaller if it
    /// wouldn't fit.
    #[must_use]
    pub fn fit(
        target: (u32, u32),
        grid: &Layout,
        tabs: usize,
        subs: usize,
        items: usize,
        morphs: usize,
    ) -> Self {
        let (w, _) = image_size(tabs, subs, items, morphs);
        let mut scale = grid.scale.max(2);
        while scale > 1 && (w + 4) * scale > target.0 {
            scale -= 1;
        }
        Self {
            origin: (2 * scale, 2 * scale),
            scale,
            tabs,
            subs,
            items,
            morphs,
        }
    }

    /// Size of the image's bounding box in target pixels; parts of it
    /// right of the shorter rows are transparent.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        let (w, h) = image_size(self.tabs, self.subs, self.items, self.morphs);
        (w * self.scale, h * self.scale)
    }

    /// The palette pixel under target pixel `(px, py)`, if inside the
    /// bounding box.
    fn pixel(&self, px: f64, py: f64) -> Option<(u32, u32)> {
        let (width, height) = image_size(self.tabs, self.subs, self.items, self.morphs);
        let scale = f64::from(self.scale);
        let (x, y) = (
            (px - f64::from(self.origin.0)) / scale,
            (py - f64::from(self.origin.1)) / scale,
        );
        let inside = x >= 0.0 && y >= 0.0 && x < f64::from(width) && y < f64::from(height);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // inside the image
        inside.then_some((x as u32, y as u32))
    }

    /// Whether target pixel `(px, py)` is over the drawn palette (any row,
    /// not the transparent part beside a shorter one).
    #[must_use]
    pub fn contains(&self, px: f64, py: f64) -> bool {
        self.pixel(px, py).is_some_and(|(x, y)| {
            let row = (y / ROW).min(self.rows() - 1);
            x < row_width(self.row(row).1)
        })
    }

    /// Rows drawn.
    fn rows(&self) -> u32 {
        rows(self.subs, self.morphs)
    }

    /// What row `row` holds and how many slots: the tabs, then the
    /// sub-tabs if any, the items and the morphs if any.
    fn row(&self, row: u32) -> (RowKind, usize) {
        let mut kinds = [
            (RowKind::Tabs, self.tabs),
            (RowKind::Subs, self.subs),
            (RowKind::Items, self.items),
            (RowKind::Morphs, self.morphs),
        ]
        .into_iter()
        .enumerate()
        .filter(|&(i, (_, n))| i == 0 || i == 2 || n > 0)
        .map(|(_, k)| k);
        kinds
            .nth(row as usize)
            .unwrap_or((RowKind::Items, self.items))
    }

    /// The tab, sub-tab, item or morph under target pixel `(px, py)`, if any.
    #[must_use]
    pub fn hit(&self, px: f64, py: f64) -> Option<PaletteHit> {
        let (x, y) = self.pixel(px, py)?;
        let row = y / ROW;
        let slot = usize::try_from(x.checked_sub(GAP)? / ROW).ok()?;
        let within = (x - GAP) % ROW < SLOT && y % ROW >= GAP && row < self.rows();
        let (kind, slots) = self.row(row);
        if !within || slot >= slots {
            return None;
        }
        Some(match kind {
            RowKind::Tabs => PaletteHit::Tab(slot),
            RowKind::Subs => PaletteHit::Sub(slot),
            RowKind::Items => PaletteHit::Item(slot),
            RowKind::Morphs => PaletteHit::Morph(slot),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowKind {
    Tabs,
    Subs,
    Items,
    Morphs,
}

/// Rows drawn: tabs and items, plus a row for sub-tabs and one for morphs
/// when there are any.
fn rows(subs: usize, morphs: usize) -> u32 {
    2 + u32::from(subs > 0) + u32::from(morphs > 0)
}

/// Width in palette pixels of a row of `slots` slots, capped at
/// [`MAX_SIZE`].
fn row_width(slots: usize) -> u32 {
    let n = u32::try_from(slots).unwrap_or(u32::MAX / ROW);
    (GAP + n.saturating_mul(ROW)).min(MAX_SIZE.0)
}

/// Palette image size in palette pixels: two rows, plus one with sub-tabs
/// (`subs` > 0) and one with morphs (`morphs` > 0), as wide as the longest.
#[must_use]
pub fn image_size(tabs: usize, subs: usize, items: usize, morphs: usize) -> (u32, u32) {
    (
        row_width(tabs.max(subs).max(items).max(morphs)),
        (rows(subs, morphs) * ROW + GAP).min(MAX_SIZE.1),
    )
}

/// One row of slots as [`build`] draws it.
struct Row<'a> {
    face: Face<'a>,
    /// The slot framed as open (tabs) or selected (items).
    framed: Option<usize>,
    /// The closed slot holding the selection, marked.
    holds: Option<usize>,
    /// The frame colour of the framed slot.
    colour: [u8; 3],
    /// The row's background.
    background: [u8; 3],
}

/// What a row's slots show.
#[derive(Clone, Copy)]
enum Face<'a> {
    /// A palette item each.
    Items(&'a [PaletteItem]),
    /// A creature's morph icons (and the automatic slot).
    Morphs(PaletteMorphs),
}

impl Face<'_> {
    fn len(&self) -> usize {
        match self {
            Face::Items(items) => items.len(),
            Face::Morphs(m) => m.slots(),
        }
    }
}

/// The rows [`build`] draws, top to bottom.
fn palette_rows(palette: &Palette) -> Vec<Row<'_>> {
    let sel = palette.selected;
    let in_open_tab = sel.filter(|&(t, _, _)| t == palette.open);
    let in_open_sub = in_open_tab.filter(|&(_, s, _)| s == palette.open_sub);
    let mut rows = vec![Row {
        face: Face::Items(&palette.tabs),
        framed: Some(palette.open),
        holds: sel.map(|(t, _, _)| t),
        colour: OPEN,
        background: BACKGROUND,
    }];
    if !palette.subs.is_empty() {
        rows.push(Row {
            face: Face::Items(&palette.subs),
            framed: Some(palette.open_sub),
            holds: in_open_tab.map(|(_, s, _)| s),
            colour: SUB_OPEN,
            background: PANEL,
        });
    }
    rows.push(Row {
        face: Face::Items(&palette.items),
        framed: in_open_sub.map(|(_, _, i)| i),
        holds: None,
        colour: SELECTED,
        background: PANEL,
    });
    if let Some(m) = palette.morphs.filter(|m| m.slots() > 0) {
        rows.push(Row {
            face: Face::Morphs(m),
            framed: m.framed(),
            holds: None,
            colour: SELECTED,
            background: PANEL,
        });
    }
    rows
}

/// Draws the palette as RGBA rows (alpha 255 = drawn, 0 = transparent).
/// Slots that don't fit [`MAX_SIZE`] are left out.
#[must_use]
pub fn build(
    palette: &Palette,
    materials: &MaterialTable,
    species: &SpeciesTable,
    plants: &PlantSpeciesTable,
    devices: &DeviceKinds,
) -> (Vec<u8>, (u32, u32)) {
    let (w, h) = image_size(
        palette.tabs.len(),
        palette.subs.len(),
        palette.items.len(),
        palette.morphs.map_or(0, |m| m.slots()),
    );
    let mut img = vec![0u8; (w * h * 4) as usize];
    let mut put = |x: u32, y: u32, c: [u8; 3]| {
        if x < w && y < h {
            let i = ((y * w + x) * 4) as usize;
            img[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    };
    let rows = palette_rows(palette);
    let last = rows.len() - 1;
    for (
        row,
        Row {
            face,
            framed,
            holds,
            colour: frame_colour,
            background,
        },
    ) in rows.into_iter().enumerate()
    {
        let y0 = u32::try_from(row).unwrap_or(0) * ROW;
        // The row's background, including the gap under the last row.
        let bottom = if row == last { h - y0 } else { ROW };
        for y in 0..bottom {
            for x in 0..row_width(face.len()) {
                put(x, y0 + y, background);
            }
        }
        for i in 0..face.len() {
            let Ok(n) = u32::try_from(i) else { break };
            let x0 = GAP + n * ROW;
            if x0 + SLOT > w {
                break;
            }
            let top = y0 + GAP;
            let frame = if framed == Some(i) {
                frame_colour
            } else {
                FRAME
            };
            for d in 0..SLOT {
                for (x, y) in [(d, 0), (d, SLOT - 1), (0, d), (SLOT - 1, d)] {
                    put(x0 + x, top + y, frame);
                }
            }
            for y in 0..SLOT - 2 {
                for x in 0..SLOT - 2 {
                    let px = match face {
                        Face::Items(items) => {
                            slot_pixel(items[i], materials, species, plants, devices, x, y)
                        }
                        Face::Morphs(m) => morph_pixel(&m, i, species, x, y),
                    };
                    put(x0 + 1 + x, top + 1 + y, px);
                }
            }
            // The open tab opens into the panel below it, where the panel
            // reaches under it.
            let under = palette.subs.len().max(palette.items.len());
            if row == 0 && framed == Some(i) && x0 + SLOT <= row_width(under) {
                for x in 1..SLOT - 1 {
                    put(x0 + x, top + SLOT - 1, PANEL);
                }
            }
            // A closed tab or sub-tab holding the selection: a mark on its
            // bottom edge.
            if framed != Some(i) && holds == Some(i) {
                for x in 3..SLOT - 3 {
                    put(x0 + x, top + SLOT - 1, SELECTED);
                }
            }
        }
    }
    (img, (w, h))
}

/// The automatic-stocking slot's icon: two chasing arrows.
const CYCLE: [&str; 8] = [
    ".####.#.", "#....###", "#.....#.", "#.......", ".......#", ".#.....#", "###....#", ".#.####.",
];

/// Pixel `(x, y)` of morph-row slot `slot`'s 8×8 face.
fn morph_pixel(m: &PaletteMorphs, slot: usize, species: &SpeciesTable, x: u32, y: u32) -> [u8; 3] {
    if let Some(morph) = m.morph_at(slot) {
        crate::sprites::morph_icon(species, m.species, morph, x, y).unwrap_or(ICON_BG)
    } else {
        let ink = CYCLE[y as usize].as_bytes().get(x as usize) == Some(&b'#');
        if ink { ICON_FG } else { ICON_BG }
    }
}

/// Pixel `(x, y)` of the sprout art, in `colours` (leaf, stem); `None`
/// where it is bare.
fn sprout_ink(colours: Option<([u8; 3], [u8; 3])>, x: u32, y: u32) -> Option<[u8; 3]> {
    let (leaf, stem) = colours.unwrap_or((ICON_FG, ICON_FG));
    match SPROUT.get(y as usize)?.as_bytes().get(x as usize) {
        Some(b'#') => Some(leaf),
        Some(b's') => Some(stem),
        Some(b'o') => Some(SEED),
        _ => None,
    }
}

/// Pixel `(x, y)` of a plant slot's face, in `colours` (leaf, stem).
fn sprout(colours: Option<([u8; 3], [u8; 3])>, x: u32, y: u32) -> [u8; 3] {
    sprout_ink(colours, x, y).unwrap_or(ICON_BG)
}

/// Pixel `(x, y)` of a slot's 8×8 face.
fn slot_pixel(
    item: PaletteItem,
    materials: &MaterialTable,
    species: &SpeciesTable,
    plants: &PlantSpeciesTable,
    devices: &DeviceKinds,
    x: u32,
    y: u32,
) -> [u8; 3] {
    match item {
        // A sprout in the species' leaf and stem colours.
        PaletteItem::Tool(Tool::Plant(s)) => {
            let colours = plants
                .get(s)
                .map(|s| (s.colour(), s.colours.stem[s.colours.stem.len() / 2]));
            sprout(colours, x, y)
        }
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
            // A prefab device's hand-drawn icon, centred (D27).
            if let Some(kind) = p.device() {
                let icon = &devices.get(kind).sprite.icon;
                let (ox, oy) = (
                    8_usize.saturating_sub(icon.width) / 2,
                    8_usize.saturating_sub(icon.height) / 2,
                );
                let (ix, iy) = (x as usize, y as usize);
                return (ix >= ox && iy >= oy && ix - ox < icon.width && iy - oy < icon.height)
                    .then(|| icon.pixels[(iy - oy) * icon.width + ix - ox])
                    .filter(|p| p[3] > 0)
                    .map_or(ICON_BG, |p| [p[0], p[1], p[2]]);
            }
            ICON_BG
        }
        PaletteItem::Tool(t) => {
            let icon = match t {
                Tool::Scoop
                | Tool::Pour
                | Tool::Place(_)
                | Tool::AddCreature(_)
                | Tool::Plant(_) => SCOOP,
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
    use crate::sprites;

    fn grid() -> Layout {
        Layout::fit((320, 180), (1280, 720))
    }

    #[test]
    fn palette_sits_top_left_at_cell_scale() {
        let p = PaletteLayout::fit((1280, 720), &grid(), 9, 0, 4, 0);
        assert_eq!(p.scale, 4);
        assert_eq!(p.origin, (8, 8));
        assert_eq!(p.size(), ((1 + 9 * 11) * 4, 23 * 4));
        // A sub-tab row adds a row.
        let p = PaletteLayout::fit((1280, 720), &grid(), 7, 3, 4, 0);
        assert_eq!(p.size(), ((1 + 7 * 11) * 4, 34 * 4));
        assert_eq!(image_size(7, 3, 4, 0).1, 34);
        // A morph row adds one too; with both the palette is MAX_SIZE tall.
        assert_eq!(image_size(7, 0, 4, 5).1, 34);
        assert_eq!(image_size(7, 3, 4, 5), (row_width(7), MAX_SIZE.1));
        assert_eq!(MAX_SIZE, (256, 45));
    }

    #[test]
    fn hit_maps_pixels_to_tabs_and_items() {
        let p = PaletteLayout::fit((1280, 720), &grid(), 9, 0, 4, 0);
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
    fn hit_maps_the_sub_tab_row() {
        let p = PaletteLayout::fit((1280, 720), &grid(), 7, 3, 5, 0);
        let row = |r: f64| 8.0 + 4.0 * (11.0 * r + 5.0);
        assert_eq!(p.hit(20.0, row(0.0)), Some(PaletteHit::Tab(0)));
        assert_eq!(p.hit(20.0, row(1.0)), Some(PaletteHit::Sub(0)));
        assert_eq!(p.hit(8.0 + 4.0 * 30.0, row(1.0)), Some(PaletteHit::Sub(2)));
        assert_eq!(p.hit(8.0 + 4.0 * 40.0, row(1.0)), None, "past the sub-tabs");
        assert_eq!(p.hit(8.0 + 4.0 * 40.0, row(2.0)), Some(PaletteHit::Item(3)));
        assert_eq!(p.hit(20.0, 8.0 + 4.0 * 33.5), None, "bottom gap");
        assert!(!p.contains(8.0 + 4.0 * 40.0, row(1.0)));
        assert!(p.contains(8.0 + 4.0 * 40.0, row(2.0)));
        assert!(p.contains(20.0, 8.0 + 4.0 * 33.5), "under the item row");
    }

    #[test]
    fn shrinks_to_fit_a_narrow_target() {
        let p = PaletteLayout::fit((200, 100), &Layout::fit((50, 25), (200, 100)), 9, 0, 3, 0);
        assert_eq!(p.scale, 1);
    }

    #[test]
    fn every_default_row_fits() {
        let w = desktank_sim::World::new(
            desktank_sim::WorldConfig::default(),
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let tree = desktank_sim::tools::Tools::palette_tree(&w);
        let subs = tree.iter().map(|t| t.subs.len()).max().unwrap();
        // Plants' five sub-tabs are the widest sub-tab row. Fish has four.
        assert_eq!(subs, 5);
        assert_eq!(tree.len(), 9, "nine top tabs");
        let widest = tree
            .iter()
            .flat_map(|t| &t.subs)
            .map(|s| s.items.len())
            .max()
            .unwrap();
        let n = tree.len().max(subs).max(widest);
        assert_eq!(image_size(tree.len(), subs, widest, 0).0, row_width(n));
        assert!(GAP + u32::try_from(n).unwrap() * ROW <= MAX_SIZE.0);
        // All nine tabs fit on one row, every one clickable, from the
        // window's minimum size (160 × 90) up.
        for target in [(160, 90), (320, 180), (1280, 720), (3840, 2160)] {
            let grid = Layout::fit((320, 180), target);
            let p = PaletteLayout::fit(target, &grid, tree.len(), subs, widest, 0);
            assert!(p.origin.0 + p.size().0 <= target.0, "{target:?}");
            let centre =
                |slot: u32| f64::from(p.origin.0 + p.scale * (GAP + slot * ROW + SLOT / 2));
            let y = f64::from(p.origin.1 + p.scale * (GAP + SLOT / 2));
            for (slot, n) in (0..).zip(0..tree.len()) {
                assert_eq!(
                    p.hit(centre(slot), y),
                    Some(PaletteHit::Tab(n)),
                    "{target:?}"
                );
            }
        }
    }

    #[test]
    fn image_frames_the_open_tabs_and_the_selection() {
        let kinds = DeviceKinds::builtin().unwrap();
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
            subs: Vec::new(),
            open_sub: 0,
            items: vec![PaletteItem::Material(sand), PaletteItem::Tool(Tool::Net)],
            selected: Some((1, 0, 1)),
            morphs: None,
        };
        let (img, (w, h)) = build(&palette, &mats, &species, &plants, &kinds);
        assert_eq!((w, h), (34, 23));
        let rgb = |img: &[u8], x, y| {
            let p = px(img, w, x, y);
            [p[0], p[1], p[2]]
        };
        assert_eq!(rgb(&img, 1, 1), FRAME, "closed tab");
        assert_eq!(rgb(&img, 12, 1), OPEN, "open tab");
        assert_eq!(
            rgb(&img, 13, 10),
            PANEL,
            "the open tab opens into the panel"
        );
        assert_eq!(rgb(&img, 12, 10), OPEN, "its corner stays");
        assert_eq!(rgb(&img, 1, 10), FRAME, "a closed tab keeps its edge");
        assert_eq!(rgb(&img, 0, 0), BACKGROUND, "the tab row");
        assert_eq!(rgb(&img, 0, 11), PANEL, "the item row is on the panel");
        assert_eq!(rgb(&img, 1, 12), FRAME);
        assert_eq!(rgb(&img, 12, 12), SELECTED, "selected item");
        let sand_colour = mats.get(sand).unwrap().colour;
        assert_eq!(rgb(&img, 3, 14), sand_colour);
        assert_eq!(rgb(&img, 2, 13), sand_colour.map(|v| v.saturating_sub(24)));
        // Beside the two-item row the image is transparent.
        assert_eq!(px(&img, w, 30, 15)[3], 0);
        assert_eq!(px(&img, w, 30, 5)[3], 255);

        // Another tab open: the selection marks its tab instead.
        palette.open = 2;
        let (img, _) = build(&palette, &mats, &species, &plants, &kinds);
        assert_eq!(rgb(&img, 16, 10), SELECTED, "mark on the selection's tab");
        assert_eq!(rgb(&img, 12, 12), FRAME, "nothing selected here");

        // With sub-tabs: three rows, the open sub-tab framed, and the
        // selection's closed sub-tab marked.
        palette.open = 1;
        palette.subs = vec![PaletteItem::Material(sand), PaletteItem::Material(water)];
        palette.open_sub = 1;
        let (img, (w3, h3)) = build(&palette, &mats, &species, &plants, &kinds);
        assert_eq!((w3, h3), (34, 34));
        assert_eq!(rgb(&img, 12, 1), OPEN, "open tab");
        assert_eq!(rgb(&img, 12, 12), SUB_OPEN, "open sub-tab, unlike a tab");
        assert_eq!(rgb(&img, 0, 11), PANEL, "the sub-tab row is on the panel");
        assert_eq!(
            rgb(&img, 5, 21),
            SELECTED,
            "mark on the selection's sub-tab"
        );
        assert_eq!(rgb(&img, 12, 23), FRAME, "selection is in another sub-tab");
        palette.open_sub = 0;
        let (img, _) = build(&palette, &mats, &species, &plants, &kinds);
        assert_eq!(rgb(&img, 12, 23), SELECTED, "selected item");

        // A plant slot shows its own species' colour.
        palette.subs.clear();
        let leaf = |palette: &Palette| {
            let (img, (w, _)) = build(palette, &mats, &species, &plants, &kinds);
            let p = px(&img, w, GAP + 1 + 3, ROW + GAP + 1);
            [p[0], p[1], p[2]]
        };
        for name in ["hygrophila", "java_moss"] {
            let s = plants.id(name).unwrap();
            palette.items = vec![PaletteItem::Tool(Tool::Plant(s))];
            assert_eq!(leaf(&palette), plants.get(s).unwrap().colour(), "{name}");
        }
    }

    #[test]
    fn hit_maps_the_morph_row_below_the_items() {
        // Tabs, items and morphs (no sub-tabs): the morph row is the third.
        let p = PaletteLayout::fit((1280, 720), &grid(), 7, 0, 5, 6);
        assert_eq!(p.size(), ((1 + 7 * 11) * 4, 34 * 4));
        let row = |r: f64| 8.0 + 4.0 * (11.0 * r + 5.0);
        let col = |c: f64| 8.0 + 4.0 * (11.0 * c + 5.0);
        assert_eq!(p.hit(col(0.0), row(1.0)), Some(PaletteHit::Item(0)));
        assert_eq!(p.hit(col(0.0), row(2.0)), Some(PaletteHit::Morph(0)));
        assert_eq!(p.hit(col(5.0), row(2.0)), Some(PaletteHit::Morph(5)));
        assert_eq!(p.hit(col(6.0), row(2.0)), None, "past the morphs");
        assert_eq!(p.hit(col(5.0), row(1.0)), None, "past the items");
        assert_eq!(p.hit(20.0, 8.0 + 4.0 * 33.5), None, "bottom gap");
        assert!(p.contains(col(5.0), row(2.0)));
        assert!(!p.contains(col(6.5), row(2.0)));
        assert!(p.contains(col(6.0), row(0.0)), "the tab row is wider");
        assert!(p.contains(20.0, 8.0 + 4.0 * 33.5), "under the morph row");

        // With sub-tabs too it is the fourth row, as tall as MAX_SIZE.
        let p = PaletteLayout::fit((1280, 720), &grid(), 7, 3, 5, 6);
        assert_eq!(p.size(), ((1 + 7 * 11) * 4, MAX_SIZE.1 * 4));
        assert_eq!(p.hit(col(0.0), row(1.0)), Some(PaletteHit::Sub(0)));
        assert_eq!(p.hit(col(4.0), row(2.0)), Some(PaletteHit::Item(4)));
        assert_eq!(p.hit(col(5.0), row(3.0)), Some(PaletteHit::Morph(5)));
        assert_eq!(p.hit(col(5.0), row(2.0)), None);
        assert_eq!(p.hit(20.0, 8.0 + 4.0 * 44.5), None, "bottom gap");
        assert!(p.contains(20.0, 8.0 + 4.0 * 44.5));
        assert!(!p.contains(col(6.5), row(3.0)));
    }

    #[test]
    fn the_biggest_palette_fits_the_hud_texture_in_webgl2() {
        assert_eq!(MAX_SIZE.1, 4 * ROW + GAP);
        assert!(crate::card::HUD_TEXTURE.1 < 2048);
        assert!(crate::card::HUD_TEXTURE.0 >= MAX_SIZE.0);
    }

    #[test]
    fn image_draws_the_morph_row_with_and_without_the_automatic_slot() {
        let kinds = DeviceKinds::builtin().unwrap();
        let mats = MaterialTable::builtin().unwrap();
        let sand = mats.id("sand").unwrap();
        let species = desktank_sim::SpeciesTable::builtin_for(&mats).unwrap();
        let plants = PlantSpeciesTable::builtin_for(&mats).unwrap();
        let cherry = species.id("cherry_shrimp").unwrap();
        let count = species.get(cherry).unwrap().morphs.len();
        assert!(count > 2);
        let mut palette = Palette {
            tabs: vec![PaletteItem::Material(sand), PaletteItem::Material(sand)],
            open: 0,
            subs: Vec::new(),
            open_sub: 0,
            items: vec![PaletteItem::Tool(Tool::AddCreature(cherry))],
            selected: Some((0, 0, 0)),
            morphs: Some(PaletteMorphs {
                species: cherry,
                count,
                cycle: true,
                selected: None,
            }),
        };
        let rgb = |img: &[u8], w: u32, x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [img[i], img[i + 1], img[i + 2]]
        };
        let alpha = |img: &[u8], w: u32, x: u32, y: u32| img[((y * w + x) * 4 + 3) as usize];
        // The slot at `k` in the morph row (the third row) has its frame at
        // x = 1 + 11k and its icon inside it, from y = 2 * 11 + 1.
        let (y0, face_y0) = (2 * ROW + GAP, 2 * ROW + GAP + 1);
        let slot_x = |k: u32| GAP + k * ROW;
        let face = |img: &[u8], w: u32, k: u32| -> Vec<[u8; 3]> {
            (0..SLOT - 2)
                .flat_map(|y| (0..SLOT - 2).map(move |x| (x, y)))
                .map(|(x, y)| rgb(img, w, slot_x(k) + 1 + x, face_y0 + y))
                .collect()
        };

        // Cycling: slot 0 is the automatic icon (framed while no morph is
        // picked), slot k the morph k - 1.
        let (img, (w, h)) = build(&palette, &mats, &species, &plants, &kinds);
        let slots = u32::try_from(count).unwrap() + 1;
        assert_eq!((w, h), (row_width(count + 1), 3 * ROW + GAP));
        assert_eq!(
            rgb(&img, w, 0, 2 * ROW),
            PANEL,
            "the morph row is on the panel"
        );
        assert_eq!(rgb(&img, w, slot_x(0), y0), SELECTED, "automatic is framed");
        assert_eq!(rgb(&img, w, slot_x(1), y0), FRAME);
        let cycle_face = face(&img, w, 0);
        let ink = |f: &[[u8; 3]]| f.iter().filter(|&&p| p == ICON_FG).count();
        assert!(ink(&cycle_face) > 8, "a drawn icon");
        assert!(cycle_face.iter().all(|&p| p == ICON_FG || p == ICON_BG));
        for k in 1..slots {
            let want: Vec<_> = (0..SLOT - 2)
                .flat_map(|y| (0..SLOT - 2).map(move |x| (x, y)))
                .map(|(x, y)| {
                    sprites::morph_icon(&species, cherry, u8::try_from(k - 1).unwrap(), x, y)
                        .unwrap_or(ICON_BG)
                })
                .collect();
            assert_eq!(face(&img, w, k), want, "morph {}", k - 1);
        }
        assert_ne!(face(&img, w, 1), face(&img, w, 2), "morphs differ");
        assert_eq!(alpha(&img, w, w - 1, h - 1), 255, "the bottom gap is drawn");

        // Picking morph 1 frames the second picture slot, not the automatic.
        palette.morphs = palette.morphs.map(|m| PaletteMorphs {
            selected: Some(1),
            ..m
        });
        let (img, (w, _)) = build(&palette, &mats, &species, &plants, &kinds);
        assert_eq!(rgb(&img, w, slot_x(0), y0), FRAME);
        assert_eq!(rgb(&img, w, slot_x(1), y0), FRAME);
        assert_eq!(rgb(&img, w, slot_x(2), y0), SELECTED);

        // Not cycling: slot k is morph k, and the default morph is framed.
        palette.morphs = Some(PaletteMorphs {
            species: cherry,
            count,
            cycle: false,
            selected: Some(0),
        });
        let (img, (w, h)) = build(&palette, &mats, &species, &plants, &kinds);
        assert_eq!(
            (w, h),
            (row_width(palette.tabs.len().max(count)), 3 * ROW + GAP)
        );
        assert_eq!(rgb(&img, w, slot_x(0), y0), SELECTED);
        assert_eq!(rgb(&img, w, slot_x(1), y0), FRAME);
        for k in 0..u32::try_from(count).unwrap() {
            let want: Vec<_> = (0..SLOT - 2)
                .flat_map(|y| (0..SLOT - 2).map(move |x| (x, y)))
                .map(|(x, y)| {
                    sprites::morph_icon(&species, cherry, u8::try_from(k).unwrap(), x, y)
                        .unwrap_or(ICON_BG)
                })
                .collect();
            assert_eq!(face(&img, w, k), want, "morph {k}");
        }

        // No morph row: back to the plain palette.
        palette.morphs = None;
        let (_, size) = build(&palette, &mats, &species, &plants, &kinds);
        assert_eq!(size, (row_width(2), 2 * ROW + GAP));
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
