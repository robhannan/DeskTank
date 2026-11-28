//! Plant species, loaded from TOML (SPEC §2.7, D18): growth form,
//! environment preferences, growth, hardiness, propagation and colours.
//!
//! Each species is one file in `assets/plants/`. Plants are individuals of
//! these species (the world's plant table, [`crate::World::plants`]); their
//! cells use a few generic part materials shared by every species
//! ([`Part`]), so a species costs no material id.
//!
//! **Cell layout** (T6b). A plant part cell's `state` holds its plant id
//! (1–65535). Its variant byte holds the part-local position in the low
//! [`POSITION_BITS`] bits (how far the cell is from where its part began:
//! stem height, leaf length, root depth; 0 for a mat) and colour noise in
//! the top two bits ([`plant_variant`]). The low byte of a seed or cutting's
//! `state` holds the [`PlantSpeciesId`] it grows into (0 for the default
//! species, the lowest id). Cuttings store their rot age in the high byte.
//!
//! **Preferences.** For light, nutrient and CO₂ a species gives `min`,
//! `optimum` and `max`: the response is 0 outside `min..max`, 1 at the
//! optimum and linear in between ([`Range::response`]). The growth rate
//! scales with the product of the three responses. A `min` below 0 means
//! it grows with none of that factor (CO₂, which a tank without injection
//! lacks): at 0 the response is `-min / (optimum - min)`.
//!
//! Units (SPEC §2.1): times in seconds of world time, rates per minute of
//! world time, field values in field units.

use std::collections::BTreeMap;

use serde::Deserialize;
use thiserror::Error;

use crate::LIFE_DAY_SECONDS;
use crate::material::{MaterialId, MaterialTable, parse_colour};
use crate::palette::{Holds, PalettePath, PalettePathError};

/// Index of a plant species in the [`PlantSpeciesTable`]; fixed in each
/// species' TOML file so saves stay stable when species are added.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlantSpeciesId(pub u8);

/// Bits of a plant cell's variant byte holding its part-local position.
pub const POSITION_BITS: u32 = 6;
/// Largest part-local position a cell records.
pub const MAX_POSITION: u8 = (1 << POSITION_BITS) - 1;

/// A plant cell's variant byte: colour noise in the top two bits (from
/// `noise`'s top bits) and the part-local position (clamped to
/// [`MAX_POSITION`]) below.
#[must_use]
pub fn plant_variant(noise: u8, position: u8) -> u8 {
    (noise & !MAX_POSITION) | position.min(MAX_POSITION)
}

/// The part-local position in a plant cell's variant byte.
#[must_use]
pub fn plant_position(variant: u8) -> u8 {
    variant & MAX_POSITION
}

pub(crate) const fn propagule_species(state: u16) -> u8 {
    state.to_le_bytes()[0]
}

pub(crate) const fn propagule_age(state: u16) -> u8 {
    state.to_le_bytes()[1]
}

pub(crate) const fn propagule_state(species: u8, age: u8) -> u16 {
    u16::from_le_bytes([species, age])
}

/// The generic plant part materials shared by every species (SPEC §2.7).
/// A rosette's or carpet's blades and an epiphyte's leaves are leaf cells
/// (their shape comes from the species' `[look] leaf`), so blades cost no
/// material id; a floater's leaves are their own, light part (T6c part 2);
/// epiphytes creep by a `rhizome` (T6c part 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Part {
    /// Grows down and sideways through the substrate.
    Root,
    /// The upright stem; a rosette's or carpet's crown; also the cells of a
    /// mat (moss).
    Stem,
    /// Grows sideways from stem nodes; a rosette's, carpet's or epiphyte's
    /// blades grow up from the crown or rhizome.
    Leaf,
    /// Creeps through the top two layers of the substrate from a crown and
    /// roots a new plant at its tip (T6c).
    Runner,
    /// A floater's leaf (T6c part 2): lighter than water, so it holds the
    /// plant up at the surface, and nearly opaque, so it shades what is
    /// below.
    Float,
    /// A creeping rhizome that clings to rock or driftwood and puts up
    /// epiphyte leaves (T6c part 3).
    Rhizome,
}

/// Slots in a plant's part counts: roots (runners count with them),
/// stems (rhizomes count with them) and leaves (floating leaves count with
/// them).
pub const COUNTED_PARTS: usize = 3;

impl Part {
    /// Every part, in discriminant order.
    pub const ALL: [Self; 6] = [
        Self::Root,
        Self::Stem,
        Self::Leaf,
        Self::Runner,
        Self::Float,
        Self::Rhizome,
    ];

    /// The part's material name.
    #[must_use]
    pub fn material_name(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Stem => "stem",
            Self::Leaf => "leaf",
            Self::Runner => "runner",
            Self::Float => "floating_leaf",
            Self::Rhizome => "rhizome",
        }
    }

    /// The discriminant (what the shader calls the part).
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// Its slot in a plant's part counts ([`COUNTED_PARTS`]): runners are
    /// counted with roots, rhizomes with stems and floating leaves with
    /// leaves, so the counts (and the save format) keep three slots.
    #[must_use]
    pub fn count_slot(self) -> usize {
        match self {
            Self::Runner => Self::Root.index(),
            Self::Rhizome => Self::Stem.index(),
            Self::Float => Self::Leaf.index(),
            p => p.index(),
        }
    }

    /// Whether it samples light (stems, rhizomes and leaves; roots and
    /// runners are in the substrate, or hang in the water under a floater).
    #[must_use]
    pub fn sees_light(self) -> bool {
        matches!(self, Self::Stem | Self::Leaf | Self::Float | Self::Rhizome)
    }
}

/// An environment factor a plant responds to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Factor {
    /// The light field.
    Light,
    /// The nutrient field.
    Nutrient,
    /// The CO₂ field.
    Co2,
}

impl Factor {
    /// Every factor, in the order samples use.
    pub const ALL: [Self; 3] = [Self::Light, Self::Nutrient, Self::Co2];

    /// Lower-case name, e.g. for the inspect card.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Nutrient => "nutrients",
            Self::Co2 => "CO₂",
        }
    }
}

/// What is holding a plant back most: the factor with the lowest response,
/// and whether it is below or above the optimum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Limit {
    /// Every factor is at its optimum.
    #[default]
    None,
    /// The factor is below the optimum.
    TooLow(Factor),
    /// The factor is above the optimum.
    TooHigh(Factor),
}

impl Limit {
    /// Words for the inspect card, e.g. "light too low".
    #[must_use]
    pub fn describe(self) -> String {
        match self {
            Self::None => "growing well".to_owned(),
            Self::TooLow(f) => format!("{} too low", f.name()),
            Self::TooHigh(f) => format!("{} too high", f.name()),
        }
    }

    /// A short machine-readable key: `none`, or `<factor>-low|high` with
    /// factor `light`, `nutrient` or `co2`.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::TooLow(Factor::Light) => "light-low",
            Self::TooHigh(Factor::Light) => "light-high",
            Self::TooLow(Factor::Nutrient) => "nutrient-low",
            Self::TooHigh(Factor::Nutrient) => "nutrient-high",
            Self::TooLow(Factor::Co2) => "co2-low",
            Self::TooHigh(Factor::Co2) => "co2-high",
        }
    }
}

/// A preference range for one factor (`min < optimum < max`).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Range {
    /// Below this the response is 0.
    pub min: f32,
    /// The response is 1 here.
    pub optimum: f32,
    /// Above this the response is 0.
    pub max: f32,
}

impl Range {
    /// 0 outside `min..max`, 1 at the optimum, linear in between.
    #[must_use]
    pub fn response(&self, v: f32) -> f32 {
        if !(v > self.min && v < self.max) {
            0.0
        } else if v <= self.optimum {
            (v - self.min) / (self.optimum - self.min)
        } else {
            (self.max - v) / (self.max - self.optimum)
        }
    }
}

/// A species' preference ranges.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    /// Light.
    pub light: Range,
    /// Nutrient.
    pub nutrient: Range,
    /// CO₂.
    pub co2: Range,
}

impl Preferences {
    /// The range for `factor`.
    #[must_use]
    pub fn range(&self, factor: Factor) -> Range {
        match factor {
            Factor::Light => self.light,
            Factor::Nutrient => self.nutrient,
            Factor::Co2 => self.co2,
        }
    }

    /// The product of the responses to `values` (light, nutrient, CO₂, in
    /// [`Factor::ALL`] order) and what limits it most.
    #[must_use]
    pub fn response(&self, values: [f32; 3]) -> (f32, Limit) {
        let mut product = 1.0;
        let mut worst = (1.0, Limit::None);
        for (f, v) in Factor::ALL.into_iter().zip(values) {
            let range = self.range(f);
            let r = range.response(v);
            product *= r;
            if r < worst.0 {
                let limit = if v <= range.optimum {
                    Limit::TooLow(f)
                } else {
                    Limit::TooHigh(f)
                };
                worst = (r, limit);
            }
        }
        (product, worst.1)
    }
}

/// How a species grows: a small growth program in Rust, parametrised by the
/// TOML (SPEC §2.7 *Growth forms*).
#[derive(Clone, Debug, PartialEq)]
pub enum Form {
    /// An upright stem with leaves at nodes and roots in the substrate.
    Stem(StemForm),
    /// A mat that creeps over the surfaces it clings to (moss).
    Mat(MatForm),
    /// A crown at the substrate with blades fanning up from it, roots
    /// below, spreading by runners (vallisneria or sword type).
    Rosette(RosetteForm),
    /// Low tufts (a rosette whose blades stay within [`MAX_CARPET_BLADE`]
    /// cells of the substrate) joined by short runners (dwarf hairgrass
    /// type).
    Carpet(RosetteForm),
    /// Leaves floating in the top water row with roots hanging below, the
    /// whole plant one buoyant body that drifts with the current, budding
    /// new plants beside it (duckweed or frogbit type, T6c part 2).
    Floater(FloaterForm),
    /// A creeping rhizome clinging to rock or driftwood that puts up
    /// upright leaves (anubias or java fern type, T6c part 3).
    Epiphyte(EpiphyteForm),
    /// A loose, round ball of stem cells resting on whatever is below it,
    /// the whole plant one sinking body the current can nudge along
    /// (marimo type).
    Ball(BallForm),
}

impl Form {
    /// The substrates it roots in, for the forms that root.
    #[must_use]
    pub fn substrates(&self) -> Option<&[MaterialId]> {
        match self {
            Self::Stem(f) => Some(&f.substrates),
            Self::Rosette(f) | Self::Carpet(f) => Some(&f.substrates),
            Self::Mat(_) | Self::Floater(_) | Self::Epiphyte(_) | Self::Ball(_) => None,
        }
    }

    /// The rosette parameters of a rosette or carpet.
    #[must_use]
    pub fn rosette(&self) -> Option<&RosetteForm> {
        match self {
            Self::Rosette(f) | Self::Carpet(f) => Some(f),
            Self::Stem(_) | Self::Mat(_) | Self::Floater(_) | Self::Epiphyte(_) | Self::Ball(_) => {
                None
            }
        }
    }

    /// The floater parameters of a floater.
    #[must_use]
    pub fn floater(&self) -> Option<&FloaterForm> {
        match self {
            Self::Floater(f) => Some(f),
            _ => None,
        }
    }

    /// The epiphyte parameters of an epiphyte.
    #[must_use]
    pub fn epiphyte(&self) -> Option<&EpiphyteForm> {
        match self {
            Self::Epiphyte(f) => Some(f),
            _ => None,
        }
    }

    /// The ball parameters of a ball.
    #[must_use]
    pub fn ball(&self) -> Option<&BallForm> {
        match self {
            Self::Ball(f) => Some(f),
            _ => None,
        }
    }

    /// Whether the whole plant moves as one body (a raft): a floater, held
    /// up by its floating leaves, or a ball, which sinks.
    #[must_use]
    pub fn drifts(&self) -> bool {
        matches!(self, Self::Floater(_) | Self::Ball(_))
    }
}

/// Largest ball radius, in cells.
pub const MAX_BALL_RADIUS: u8 = 8;

/// Parameters of the ball form (marimo type). A ball starts as one stem
/// cell where its propagule came to rest and grows into the water around
/// it, its nearest cells first, keeping within a disc of `radius` cells
/// about the middle of its bounds; each cell's position is how far out it
/// grew (0 in the middle). Like a floater it is one body (the world's
/// rafts), but its cells are denser than water, so it sinks to rest on
/// what is below it and the current can only nudge it along.
#[derive(Clone, Debug, PartialEq)]
pub struct BallForm {
    /// Largest radius, in cells (1..=[`MAX_BALL_RADIUS`]).
    pub radius: u8,
}

/// Longest rhizome an epiphyte creeps, in cells.
pub const MAX_RHIZOME: u8 = 32;
/// Thickest mat, in layers of cells off what it clings to.
pub const MAX_MAT_LAYERS: u8 = 4;

/// Parameters of the epiphyte form (T6c part 3). A creeping rhizome
/// ([`Part::Rhizome`]) clings to rock or driftwood, putting up leaves
/// ([`Part::Leaf`] with a blade shape) every `spacing` rhizome cells; when
/// what holds the rhizome is gone, the rhizome falls off and any leaf piece
/// left without a rhizome dies.
#[derive(Clone, Debug, PartialEq)]
pub struct EpiphyteForm {
    /// Materials the rhizome clings to.
    pub on: Vec<MaterialId>,
    /// Longest rhizome, in cells (1..=[`MAX_RHIZOME`]).
    pub rhizome_length: u8,
    /// Rhizome cells between leaves (≥ 1).
    pub spacing: u8,
    /// Longest leaf, in cells (1..=[`MAX_POSITION`]).
    pub leaf_length: u8,
    /// Cells of leaf between two outward steps away from the base (0:
    /// upright).
    pub lean: u8,
}

impl EpiphyteForm {
    /// Near the base leaves stand upright. Farther out, take an extra
    /// sideways step between the usual ones so the leaves fan apart.
    pub(crate) fn blade_steps_out(&self, position: u8, offset: u64) -> bool {
        if self.lean == 0 || offset == 0 || position < 2 {
            return false;
        }
        let cadence = self.lean.max(3);
        let phase = (position - 2) % cadence;
        phase == 0 || (offset >= 2 * u64::from(self.spacing) && phase == cadence / 2)
    }
}

/// Most floating leaves one floater spreads to.
pub const MAX_FRONDS: u8 = 8;
/// Longest root a floater hangs, in cells.
pub const MAX_HANGING_ROOT: u8 = 32;

/// Parameters of the floater form (T6c part 2). Floating leaves (the
/// [`Part::Float`] material) spread sideways along the top water row, under
/// the air, from where the plant started, up to `fronds` cells; `roots`
/// strands hang from them, each up to `root_length` cells. Once it has all
/// its fronds (or reached its mature size) it buds: a new plant (parent id
/// set, generation + 1) starts as one floating leaf in the water beside an
/// end frond. The whole plant moves as one buoyant body (the world's
/// floater rafts), so it rests at the surface and drifts with the current.
#[derive(Clone, Debug, PartialEq)]
pub struct FloaterForm {
    /// Floating leaf cells, side by side (1..=[`MAX_FRONDS`]).
    pub fronds: u8,
    /// Root strands hanging from the fronds, a cell apart (1..=`fronds`
    /// / 2, rounded up).
    pub roots: u8,
    /// Longest root, in cells (1..=[`MAX_HANGING_ROOT`]).
    pub root_length: u8,
}

/// Longest runner, in cells (its positions must fit [`MAX_POSITION`]).
pub const MAX_RUNNER: u8 = MAX_POSITION - 1;
/// Widest runner spacing: a runner needs some cells past it to find a place
/// to root.
pub const MAX_SPACING: u8 = 48;
/// Deepest a rosette's runners creep below the bed's top layer, in cells.
pub const MAX_RUNNER_DEPTH: u8 = 8;

/// Widest leaf pad a rosette's blade spreads to each side, in cells.
pub const MAX_PAD: u8 = 4;

/// Longest blade a carpet grows, in cells: carpets stay low.
pub const MAX_CARPET_BLADE: u8 = 3;
/// Most crown cells a rosette spreads along the substrate.
pub const MAX_CROWN: u8 = 3;

/// Parameters of the rosette and carpet forms (T6c). A crown of stem cells
/// sits on the substrate, growing rightwards from where it rooted to one to
/// [`MAX_CROWN`] cells wide; blades (leaf cells) start above each crown cell
/// and beside its ends and grow up, those off the crown's centre stepping a
/// cell outwards at their second cell and every `lean` cells after (more
/// often the further out they are, so outer blades arch away); roots grow
/// down from the first crown cell. With `runners` in its propagation, a runner leaves the
/// root under the crown on each side, creeps through the top substrate
/// layer and, `spacing` cells out, roots a child plant (parent id set,
/// generation + 1) whose crown sits on the substrate above.
///
/// With a `pad` (tiger lotus type) the blades are petioles of differing
/// lengths: each stops at `blade_length` or between about half and all of
/// it, by plant and the column its tip has reached. Where a petiole stops (at its length, or under the
/// air or something else) it spreads a broad leaf of leaf cells sideways
/// along its tip's row, up to `pad` cells each way, at positions
/// `blade_length + 1` on; at the surface (under the air) the pad is of
/// floating leaves, which shade the water below. Under water each pad cell
/// short of its edge (and the petiole's tip) also puts a cell above it, at
/// [`RosetteForm::pad_top`], so a submerged leaf is two rows deep, its
/// upper row narrower.
#[derive(Clone, Debug, PartialEq)]
pub struct RosetteForm {
    /// Crown cells along the substrate (1 for a carpet).
    pub crown: u8,
    /// Most blades (at most `crown + 2`).
    pub blades: u8,
    /// Longest blade, in cells.
    pub blade_length: u8,
    /// Cells of blade between two outward steps near the centre (0:
    /// upright).
    pub lean: u8,
    /// Deepest root, in cells from the crown.
    pub root_depth: u8,
    /// Runner cells from the crown to a new plant (a runner gives up at
    /// twice this, or at [`MAX_RUNNER`] cells).
    pub spacing: u8,
    /// How deep its runners creep, in substrate cells below the bed's top
    /// layer (0: in the top two layers; 1..=[`MAX_RUNNER_DEPTH`]: they dive
    /// to it, and climb back up to root the next plant).
    pub runner_depth: u8,
    /// Substrates it roots in and runners creep through.
    pub substrates: Vec<MaterialId>,
    /// Cells a petiole's leaf pad spreads to each side (0: plain blades;
    /// 1..=[`MAX_PAD`]).
    pub pad: u8,
}

impl RosetteForm {
    /// The longest a runner grows before it gives up, in cells.
    #[must_use]
    pub fn runner_limit(&self) -> u8 {
        self.spacing.saturating_mul(2).min(MAX_RUNNER)
    }

    /// Whether a leaf cell at `position` is part of a leaf pad rather than
    /// a blade.
    #[must_use]
    pub fn is_pad(&self, position: u8) -> bool {
        self.pad > 0 && position > self.blade_length
    }

    /// The position of a submerged pad's upper row cells (one past its
    /// widest).
    #[must_use]
    pub fn pad_top(&self) -> u8 {
        self.blade_length + self.pad + 1
    }
}

/// Parameters of the stem form.
#[derive(Clone, Debug, PartialEq)]
pub struct StemForm {
    /// Tallest stem, in cells.
    pub height: u8,
    /// Stem cells from one node (pair of leaves) to the next (≥ 2, so
    /// leaves on one side sit at least a row apart).
    pub internode: u8,
    /// Longest leaf, in cells.
    pub leaf_length: u8,
    /// Deepest root, in cells from where it started.
    pub root_depth: u8,
    /// Share of stem nodes that put out a side shoot (0.0..=1.0, T6c part
    /// 3).
    pub branching: f32,
    /// Substrates a seed or cutting roots in and roots grow through.
    pub substrates: Vec<MaterialId>,
}

/// What a broad stem leaf's upper cells add to their position along the
/// leaf (see [`StemForm::leaf_upper`]): a position at or above it is the
/// cell stacked on the leaf's row, at `position - LEAF_UPPER`.
pub const LEAF_UPPER: u8 = 32;

/// Columns out from the main stem a side shoot leans before it rises
/// straight: it leaves its node on the diagonal and steps out a column every
/// two rows. The sprite's four pixels (a texel each, half a cell) are two
/// cells, and its shoot starts a column out. Wider leans wove neighbouring
/// stands into walls that stalled swimmers.
pub const SHOOT_LEAN: i64 = 3;

impl StemForm {
    /// How many cells long the leaves at a node `below_tip` cells under the
    /// tip of its stem (or side shoot) may be, as the species sprite's are:
    /// none on the bare top `internode` cells (the stem ends in a bud), then
    /// one cell, lengthening in whole cells towards `leaf_length` at half the
    /// stem's `height` below the tip and staying there. Measured from the
    /// stem's own tip, so a young stem is tapered too and its lower leaves
    /// lengthen as it grows past them, and monotonic: a tier is never
    /// longer than the one below it on the same stem.
    #[must_use]
    pub fn leaf_taper(&self, below_tip: u8) -> u8 {
        if below_tip < self.internode {
            return 0;
        }
        let length = i64::from(self.leaf_length);
        let half = i64::from((self.height / 2).max(1));
        let d = i64::from(below_tip).min(half);
        u8::try_from(((2 * length * d + half) / (2 * half)).clamp(1, length.max(1))).unwrap_or(1)
    }

    /// Whether the leaves at the node `pos` cells up the stem are broad
    /// ones that grow an upper cell (see [`StemForm::leaf_upper`]): every
    /// other node, so the tiers alternate broad and narrow and a swimmer
    /// can pass between them (a stack at every node is a wall).
    #[must_use]
    pub fn broad_node(&self, pos: u8) -> bool {
        (pos / self.internode.max(1)) % 2 == 1
    }

    /// The positions along a leaf of `length` cells, as `first..=last`, that
    /// carry a second cell above them (at [`LEAF_UPPER`] + their position)
    /// when the leaves are `shape`: the broad and feathered shapes (lanceolate,
    /// round, feather) fill the internode row above a node's leaf, as a blade
    /// two cells thick at its middle. A leaf of one or two cells has it on
    /// its outer cell, and on its inner one too (a fuller round leaf) when
    /// it is `full` length, as the lowest leaves of a stem are. Needle leaves
    /// stay a row thick.
    #[must_use]
    pub fn leaf_upper(shape: LeafShape, length: u8, full: bool) -> Option<(u8, u8)> {
        if !matches!(
            shape,
            LeafShape::Lanceolate | LeafShape::Round | LeafShape::Feather
        ) || length >= LEAF_UPPER
        {
            return None;
        }
        Some(if length <= 2 {
            (if full { 1 } else { length.max(1) }, length.max(1))
        } else {
            (2, length - 1)
        })
    }
}

/// Parameters of the mat form.
#[derive(Clone, Debug, PartialEq)]
pub struct MatForm {
    /// Materials it clings to; layer-0 cells must touch one, and outer
    /// layers (up to `layers` cells thick) must touch a nearer layer, or
    /// they fall off as dead plant matter.
    pub on: Vec<MaterialId>,
    /// Layers of cells the mat builds up off what it clings to
    /// (1..=[`MAX_MAT_LAYERS`]; 1 for a flat mat like java moss, 3 for
    /// layered christmas moss).
    pub layers: u8,
}

/// Growth speed and size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Growth {
    /// New cells per minute at full response.
    pub rate: f32,
    /// Mature size in cells: it stops growing there.
    pub size: u32,
    /// Nutrient each new cell takes from the field where it grows.
    pub cost: f32,
    /// Lifespan in seconds of world time, if it ages: past it, health
    /// declines. A species gives `lifespan_days` on the life clock (SPEC
    /// §2.4, [`LIFE_DAY_SECONDS`] per day, as creatures do), converted here.
    pub lifespan: Option<f32>,
    /// It feeds through its roots: only its roots and runners sample the
    /// nutrient field, so it follows the substrate (root tabs) rather than
    /// the water (sword type). Only for forms that root.
    pub root_feeder: bool,
}

/// How a species copes with conditions outside its ranges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hardiness {
    /// Seconds outside its ranges before health starts to fall.
    pub tolerance: f32,
    /// Health lost per second after that (health runs from 0 to 1).
    pub decline: f32,
    /// Health regained per second while inside its ranges.
    pub recovery: f32,
}

/// How a species spreads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // a set of flags: the TOML's list
pub struct Propagation {
    /// Grows from seeds.
    pub seeds: bool,
    /// Grows from cuttings.
    pub cuttings: bool,
    /// A cut-off piece holding a stem lives on as a new plant.
    pub fragments: bool,
    /// Sends out runners that root new plants (rosettes and carpets).
    pub runners: bool,
    /// Buds new plants beside it (floaters).
    pub buds: bool,
}

/// Colour ramps per part, dark to light (sRGB). The renderer shades each
/// plant cell from its part's ramp by position (T6d); the palette swatch
/// and the web picker use the middle leaf shade.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ramps {
    /// Root shades.
    pub root: Vec<[u8; 3]>,
    /// Stem shades.
    pub stem: Vec<[u8; 3]>,
    /// Leaf shades.
    pub leaf: Vec<[u8; 3]>,
}

/// Most shades in one ramp.
pub const MAX_RAMP: usize = 8;

/// How far the mean light must rise above a species' colouration `light`
/// for the full colour (T6d).
pub const COLOURATION_LIGHT_SPAN: f32 = 0.25;

/// A species' colouration in strong light with low nutrients (rotala type,
/// SPEC §2.7 *Appearance*, T6d; `[colouration]` in its TOML): its leaves
/// and, less, its stems shift towards `colour`, the new growth most.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colouration {
    /// The shade it turns (sRGB); each ramp shade keeps its lightness.
    pub colour: [u8; 3],
    /// Mean light above which it starts to colour; it colours fully at
    /// [`COLOURATION_LIGHT_SPAN`] above.
    pub light: f32,
    /// Mean nutrient below which it starts to colour; it colours fully at
    /// half of it.
    pub nutrient: f32,
}

impl Colouration {
    /// How strongly (0–1) a plant colours at mean `light` and `nutrient`:
    /// the product of how far light is above its threshold and nutrient
    /// below its.
    #[must_use]
    pub fn target(&self, light: f32, nutrient: f32) -> f32 {
        let lit = ((light - self.light) / COLOURATION_LIGHT_SPAN).clamp(0.0, 1.0);
        let lean = ((self.nutrient - nutrient) / (self.nutrient * 0.5)).clamp(0.0, 1.0);
        lit * lean
    }
}

/// How a species' leaves are drawn at two texels per cell side (T6d,
/// `[look] leaf` in its TOML). The discriminant is what the shader reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum LeafShape {
    /// A broad blade on a short petiole, rising to a pointed tip
    /// (hygrophila type).
    #[default]
    Lanceolate = 0,
    /// A fine one-texel blade rising from base to tip (rotala type).
    Needle = 1,
    /// A broad blade with a blunt, rounded tip (ludwigia or bacopa type).
    Round = 2,
    /// A midrib with a fringe of fine teeth above it (cabomba type).
    Feather = 3,
    /// Blade shapes, for rosettes and carpets (T6c), drawn up from the
    /// crown: a long, one-texel strap that twists, crossing sides every
    /// few cells (vallisneria type).
    Ribbon = 4,
    /// A broad blade on a narrow stalk with a pale midrib and a pointed tip
    /// (sword type).
    Sword = 5,
    /// A fine, one-texel blade (hairgrass type).
    Grass = 6,
    /// A tiny round leaf on a short stalk, the tip cell a full pad (Monte
    /// Carlo type).
    Pad = 7,
    /// Floating leaf shapes, for floaters (T6c part 2), drawn lying on the
    /// water: small flat ovals side by side, lit on top (duckweed type).
    Lens = 8,
    /// A round, glossy pad with a pale vein and a dark, spongy underside
    /// (frogbit type).
    Disc = 9,
    /// A blade shape for rosettes with leaf pads: a thin petiole up to a
    /// broad, mottled leaf spread sideways from its tip, lying flat on the
    /// water where it reaches the surface (tiger lotus type).
    Lotus = 10,
}

impl LeafShape {
    /// Every shape, in discriminant order.
    pub const ALL: [Self; 11] = [
        Self::Lanceolate,
        Self::Needle,
        Self::Round,
        Self::Feather,
        Self::Ribbon,
        Self::Sword,
        Self::Grass,
        Self::Pad,
        Self::Lens,
        Self::Disc,
        Self::Lotus,
    ];

    /// Whether it is a blade shape (rosettes and carpets), drawn upright,
    /// rather than a leaf growing sideways from a stem.
    #[must_use]
    pub fn is_blade(self) -> bool {
        matches!(
            self,
            Self::Ribbon | Self::Sword | Self::Grass | Self::Pad | Self::Lotus
        )
    }

    /// Whether it is a floating leaf shape (floaters).
    #[must_use]
    pub fn is_floating(self) -> bool {
        matches!(self, Self::Lens | Self::Disc)
    }
}

/// How a species' stem cells are drawn (`[look] stem`). A mat's cells are
/// stems, so mats pick a mat shape here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum StemShape {
    /// A one-texel stalk that widens at nodes and the growing tip.
    #[default]
    Stalk = 0,
    /// A full-width stem, shaded on one side (bacopa type).
    Thick = 1,
    /// A soft mat whose exposed edges break into a ragged fringe (java
    /// moss type).
    Fuzz = 2,
    /// A mat of layered, downswept fronds (christmas moss type).
    Frond = 3,
    /// A ball's dense, velvety pile, lit on top and shaded below (marimo
    /// type).
    Velvet = 4,
}

impl StemShape {
    /// Every shape, in discriminant order.
    pub const ALL: [Self; 5] = [
        Self::Stalk,
        Self::Thick,
        Self::Fuzz,
        Self::Frond,
        Self::Velvet,
    ];
}

/// How a species' root cells are drawn (`[look] root`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum RootShape {
    /// A thin, wandering one-texel strand.
    #[default]
    Strand = 0,
    /// A full-width root (rhizome type).
    Thick = 1,
}

impl RootShape {
    /// Every shape, in discriminant order.
    pub const ALL: [Self; 2] = [Self::Strand, Self::Thick];
}

/// How a stem plant's leaves are arranged round its stem (`[look] arrangement`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Arrangement {
    /// Opposite pairs, each turned a quarter from the pair below (decussate):
    /// side pairs at alternate nodes, front and back pairs between.
    #[default]
    Opposite = 0,
    /// A whorl: every node's leaves fan out in a full disc round the stem
    /// (cabomba).
    Whorl = 1,
}

/// A species' drawn shapes per part (SPEC §2.7 *Appearance*, T6d). Every
/// key is optional in the TOML: leaves default to lanceolate, roots to
/// strands, and stems to stalks (fuzz for a mat).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Look {
    /// Leaf shape.
    pub leaf: LeafShape,
    /// Stem (or mat) shape.
    pub stem: StemShape,
    /// Root shape.
    pub root: RootShape,
    /// How leaves are arranged round a stem plant's stem.
    pub arrangement: Arrangement,
}
/// Fastest growth rate, in cells per minute at full response (the plant
/// update grows at most 8 cells per plant every 5 s).
pub const MAX_RATE: f32 = 90.0;

/// One plant species, validated.
#[derive(Clone, Debug, PartialEq)]
pub struct PlantSpecies {
    /// Stable ID.
    pub id: PlantSpeciesId,
    /// Unique name, e.g. `"hygrophila"`.
    pub name: String,
    /// Latin binomial (with cultivar or variety if it has one), e.g.
    /// `"Hygrophila polysperma"`, for the species cards.
    pub scientific_name: String,
    /// One line for the inspect card.
    pub description: String,
    /// The palette sub-category it is offered in (`palette = "plants/stem"`),
    /// or `None` if the palette doesn't offer it.
    pub palette: Option<PalettePath>,
    /// Growth form and its parameters.
    pub form: Form,
    /// Environment preferences.
    pub prefer: Preferences,
    /// Growth speed and size.
    pub growth: Growth,
    /// Hardiness.
    pub hardiness: Hardiness,
    /// How it spreads.
    pub propagation: Propagation,
    /// Whether creatures whose diet includes this part may eat it.
    pub palatable: bool,
    /// Colour ramps.
    pub colours: Ramps,
    /// Drawn shapes.
    pub look: Look,
    /// Colouration in strong light with low nutrients, if it colours.
    pub colouration: Option<Colouration>,
}

impl PlantSpecies {
    /// Its main colour: the middle leaf shade (stem for a mat or ball).
    #[must_use]
    pub fn colour(&self) -> [u8; 3] {
        let ramp = match self.form {
            Form::Mat(_) | Form::Ball(_) => &self.colours.stem,
            Form::Stem(_)
            | Form::Rosette(_)
            | Form::Carpet(_)
            | Form::Floater(_)
            | Form::Epiphyte(_) => &self.colours.leaf,
        };
        ramp.get(ramp.len() / 2)
            .copied()
            .unwrap_or([0x40, 0x90, 0x40])
    }

    /// Whether it grows from `propagule` (lists it under `propagation`).
    #[must_use]
    pub fn grows_from(&self, propagule: Propagule) -> bool {
        let p = self.propagation;
        match propagule {
            Propagule::Seeds => p.seeds,
            Propagule::Cuttings => p.cuttings,
            Propagule::Fragments => p.fragments,
            Propagule::Runners => p.runners,
            Propagule::Buds => p.buds,
        }
    }

    /// The ways it grows, in [`Propagule::ALL`] order (never empty).
    pub fn propagules(&self) -> impl Iterator<Item = Propagule> + '_ {
        Propagule::ALL.into_iter().filter(|&p| self.grows_from(p))
    }

    /// What kind of plant it is, in one lower-case word for the species
    /// cards: `"stem"`, `"mat"`, `"rosette"`, `"carpet"`, `"floater"`,
    /// `"epiphyte"` or `"ball"`.
    #[must_use]
    pub fn describe_form(&self) -> &'static str {
        match self.form {
            Form::Stem(_) => "stem",
            Form::Mat(_) => "mat",
            Form::Rosette(_) => "rosette",
            Form::Carpet(_) => "carpet",
            Form::Floater(_) => "floater",
            Form::Epiphyte(_) => "epiphyte",
            Form::Ball(_) => "ball",
        }
    }

    /// What it needs, as comma-separated phrases for the species cards:
    /// its light band, any CO₂ need, root feeding and placement for forms
    /// that do not root in substrate.
    /// Derived from its preferences, growth and form; material names come
    /// from `materials`.
    #[must_use]
    pub fn describe_requirements(&self, materials: &MaterialTable) -> String {
        let mut parts: Vec<String> = Vec::new();
        let light = self.prefer.light.optimum;
        parts.push(
            if light < 0.45 {
                "low light"
            } else if light < 0.70 {
                "moderate light"
            } else {
                "strong light"
            }
            .to_owned(),
        );
        let co2_min = self.prefer.co2.min;
        if co2_min >= 0.0 {
            parts.push("needs CO₂".to_owned());
        } else if co2_min > -0.8 {
            parts.push("benefits from CO₂".to_owned());
        }
        if self.growth.root_feeder {
            parts.push("root feeder".to_owned());
        }
        let surfaces = match &self.form {
            Form::Stem(_) | Form::Rosette(_) | Form::Carpet(_) => None,
            Form::Mat(f) => Some(f.on.as_slice()),
            Form::Epiphyte(f) => Some(f.on.as_slice()),
            Form::Floater(_) => {
                parts.push("floats at surface".to_owned());
                None
            }
            Form::Ball(_) => {
                parts.push("rests on bottom".to_owned());
                None
            }
        };
        if let Some(ids) = surfaces
            && let names = material_names(ids, materials)
            && !names.is_empty()
        {
            parts.push(format!("clings to {names}"));
        }
        parts.join(", ")
    }

    /// The compatible rooting substrates, comma-separated, for the species card.
    #[must_use]
    pub fn describe_rooting_substrates(&self, materials: &MaterialTable) -> Option<String> {
        let names = material_names(self.form.substrates()?, materials);
        (!names.is_empty()).then_some(names)
    }

    /// The ways it grows, comma-separated, for the species cards (the
    /// names of [`PlantSpecies::propagules`]).
    #[must_use]
    pub fn describe_propagation(&self) -> String {
        self.propagules()
            .map(Propagule::name)
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The propagule the palette pours until another is picked: cuttings
    /// if it grows from them (they root soonest), else its first way.
    #[must_use]
    pub fn default_propagule(&self) -> Propagule {
        if self.propagation.cuttings {
            Propagule::Cuttings
        } else {
            self.propagules().next().unwrap_or(Propagule::Cuttings)
        }
    }
}

/// Display names of materials, in species data order.
fn material_names(ids: &[MaterialId], materials: &MaterialTable) -> String {
    ids.iter()
        .filter_map(|&id| materials.get(id))
        .map(|m| m.name.replace('_', " "))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A way to start a plant from the palette: what a plant slot pours
/// (the names are those of a species' `propagation` list).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Propagule {
    /// Seeds: the `seed` material, slow to germinate.
    Seeds,
    /// Stem cuttings: the `cutting` material, quick to root.
    Cuttings,
    /// Fragments: a cut-off piece of the plant. Poured, a fragment is the
    /// `cutting` material (a loose piece that roots or clings like one);
    /// it has no material of its own.
    Fragments,
    /// Runners: the species spreads by runners as it grows (T6c). Poured,
    /// it is a plantlet taken off a runner: the `cutting` material, which
    /// roots like one.
    Runners,
    /// Buds: the species buds new plants as it grows (floaters, T6c part
    /// 2). Poured, it is a budded-off plantlet: the `cutting` material,
    /// which in open water turns into a floating leaf that rises to the
    /// surface.
    Buds,
}

impl Propagule {
    /// Every propagule, in the order hosts list them.
    pub const ALL: [Self; 5] = [
        Self::Seeds,
        Self::Cuttings,
        Self::Fragments,
        Self::Runners,
        Self::Buds,
    ];

    /// Its name, as a species' `propagation` list and scripts spell it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Seeds => "seeds",
            Self::Cuttings => "cuttings",
            Self::Fragments => "fragments",
            Self::Runners => "runners",
            Self::Buds => "buds",
        }
    }

    /// The propagule with this name, if any.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == name)
    }

    /// The material poured for it.
    #[must_use]
    pub fn material_name(self) -> &'static str {
        match self {
            Self::Seeds => "seed",
            Self::Cuttings | Self::Fragments | Self::Runners | Self::Buds => "cutting",
        }
    }
}

/// How grown a whole plant placed in one click is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlantStage {
    /// A young plant: about a third of its species' mature size.
    Juvenile,
    /// An (almost) fully grown plant.
    Mature,
}

impl PlantStage {
    /// Its name, as the palette spells it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Juvenile => "juvenile",
            Self::Mature => "mature",
        }
    }
}

/// What a plant slot places per click: one of the species' own propagules
/// (seeds pour as powder, anything else is one small placed piece), or a
/// whole [`PlantStage::Juvenile`] or [`PlantStage::Mature`] plant of any
/// species ([`crate::World::place_plant`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlantStart {
    /// A propagule the species grows from.
    Propagule(Propagule),
    /// A whole young plant.
    Juvenile,
    /// A whole grown plant.
    Mature,
}

impl PlantStart {
    /// Its name: a propagule's ([`Propagule::name`]), or `"juvenile"` or
    /// `"mature"`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Propagule(p) => p.name(),
            Self::Juvenile => PlantStage::Juvenile.name(),
            Self::Mature => PlantStage::Mature.name(),
        }
    }

    /// The start with this name, if any.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "juvenile" => Some(Self::Juvenile),
            "mature" => Some(Self::Mature),
            _ => Propagule::from_name(name).map(Self::Propagule),
        }
    }

    /// The stage of a whole plant, or `None` for a propagule.
    #[must_use]
    pub fn stage(self) -> Option<PlantStage> {
        match self {
            Self::Propagule(_) => None,
            Self::Juvenile => Some(PlantStage::Juvenile),
            Self::Mature => Some(PlantStage::Mature),
        }
    }
}

/// Error loading or validating plant species.
#[derive(Debug, Error)]
pub enum PlantSpeciesError {
    /// A TOML source didn't parse or didn't match the schema.
    #[error("plant species source {index}: {source}")]
    Parse {
        /// Index of the source in the list passed in.
        index: usize,
        /// The parse error.
        #[source]
        source: toml::de::Error,
    },
    /// Two species share an ID (ids start at 1).
    #[error("duplicate or zero plant species id {0}")]
    BadId(u8),
    /// Two species share a name.
    #[error("duplicate plant species name {0:?}")]
    DuplicateName(String),
    /// A property is out of range.
    #[error("plant species {name:?}: {reason}")]
    Invalid {
        /// Species name.
        name: String,
        /// What is wrong.
        reason: &'static str,
    },
    /// `palette` names no sub-category that holds plants.
    #[error("plant species {name:?}: {source}")]
    Palette {
        /// Species name.
        name: String,
        /// What is wrong with the path.
        #[source]
        source: PalettePathError,
    },
    /// A material it names doesn't exist.
    #[error("plant species {name:?}: unknown material {material:?}")]
    UnknownMaterial {
        /// Species name.
        name: String,
        /// The missing material.
        material: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpeciesDef {
    id: u8,
    name: String,
    scientific_name: String,
    description: String,
    #[serde(default)]
    palette: Option<String>,
    form: FormName,
    stem: Option<StemDef>,
    mat: Option<MatDef>,
    rosette: Option<RosetteDef>,
    carpet: Option<RosetteDef>,
    floater: Option<FloaterDef>,
    epiphyte: Option<EpiphyteDef>,
    ball: Option<BallDef>,
    prefer: Preferences,
    growth: GrowthDef,
    hardiness: HardinessDef,
    propagation: Vec<Propagule>,
    #[serde(default)]
    palatable: bool,
    colours: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    look: LookDef,
    colouration: Option<ColourationDef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ColourationDef {
    colour: String,
    light: f32,
    nutrient: f32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LookDef {
    leaf: Option<LeafShape>,
    stem: Option<StemShape>,
    root: Option<RootShape>,
    arrangement: Option<Arrangement>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FormName {
    Stem,
    Mat,
    Rosette,
    Carpet,
    Floater,
    Epiphyte,
    Ball,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BallDef {
    radius: u8,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EpiphyteDef {
    on: Vec<String>,
    rhizome_length: u8,
    spacing: u8,
    leaf_length: u8,
    #[serde(default)]
    lean: u8,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FloaterDef {
    fronds: u8,
    roots: u8,
    root_length: u8,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StemDef {
    height: u8,
    internode: u8,
    leaf_length: u8,
    root_depth: u8,
    #[serde(default)]
    branching: f32,
    substrates: Vec<String>,
}

fn default_one() -> u8 {
    1
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MatDef {
    on: Vec<String>,
    #[serde(default = "default_one")]
    layers: u8,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RosetteDef {
    /// Absent for a carpet (always 1).
    crown: Option<u8>,
    blades: u8,
    blade_length: u8,
    #[serde(default)]
    lean: u8,
    root_depth: u8,
    spacing: u8,
    #[serde(default)]
    runner_depth: u8,
    substrates: Vec<String>,
    #[serde(default)]
    pad: u8,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrowthDef {
    rate: f32,
    size: u32,
    cost: f32,
    /// Life days (SPEC §2.4), as a creature's `lifespan_days`.
    lifespan_days: Option<f32>,
    #[serde(default)]
    root_feeder: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HardinessDef {
    tolerance: f32,
    decline: f32,
    recovery: f32,
}

/// The plant species embedded in the binary (T6b: T6's plant and moss, and
/// a high-light stem plant; then ludwigia, cabomba, bacopa and christmas
/// moss; T6c's rosettes and carpets: amazon sword, vallisneria, dwarf
/// hairgrass, Monte Carlo and cryptocoryne; T6c's floaters: duckweed and
/// Amazon frogbit; T6c part 3's epiphytes: anubias and java fern; then
/// issue #45's marimo ball and tiger lotus).
const BUILTIN: &[&str] = &[
    include_str!("../../../assets/plants/hygrophila.toml"),
    include_str!("../../../assets/plants/java_moss.toml"),
    include_str!("../../../assets/plants/rotala.toml"),
    include_str!("../../../assets/plants/ludwigia.toml"),
    include_str!("../../../assets/plants/cabomba.toml"),
    include_str!("../../../assets/plants/bacopa.toml"),
    include_str!("../../../assets/plants/christmas_moss.toml"),
    include_str!("../../../assets/plants/amazon_sword.toml"),
    include_str!("../../../assets/plants/vallisneria.toml"),
    include_str!("../../../assets/plants/dwarf_hairgrass.toml"),
    include_str!("../../../assets/plants/monte_carlo.toml"),
    include_str!("../../../assets/plants/cryptocoryne.toml"),
    include_str!("../../../assets/plants/duckweed.toml"),
    include_str!("../../../assets/plants/frogbit.toml"),
    include_str!("../../../assets/plants/anubias.toml"),
    include_str!("../../../assets/plants/java_fern.toml"),
    include_str!("../../../assets/plants/marimo.toml"),
    include_str!("../../../assets/plants/tiger_lotus.toml"),
];

/// All plant species, ordered by ID.
#[derive(Clone, Debug, PartialEq)]
pub struct PlantSpeciesTable {
    species: Vec<PlantSpecies>,
}

impl PlantSpeciesTable {
    /// The species embedded from `assets/plants/*.toml`, with materials
    /// looked up in `materials`. Materials the table lacks (a test's own
    /// set) are left out of substrates and surfaces.
    ///
    /// # Errors
    ///
    /// Returns an error if an embedded file is invalid (a build defect,
    /// covered by tests).
    pub fn builtin_for(materials: &MaterialTable) -> Result<Self, PlantSpeciesError> {
        Self::load(BUILTIN, materials, true)
    }

    /// Parses one TOML document per species and validates the set.
    ///
    /// # Errors
    ///
    /// Returns an error if a source doesn't parse, a property is invalid,
    /// IDs or names clash, or a material name is unknown.
    pub fn from_toml_sources(
        sources: &[&str],
        materials: &MaterialTable,
    ) -> Result<Self, PlantSpeciesError> {
        Self::load(sources, materials, false)
    }

    fn load(
        sources: &[&str],
        materials: &MaterialTable,
        skip_unknown: bool,
    ) -> Result<Self, PlantSpeciesError> {
        let mut defs = Vec::with_capacity(sources.len());
        for (index, src) in sources.iter().enumerate() {
            let def: SpeciesDef =
                toml::from_str(src).map_err(|source| PlantSpeciesError::Parse { index, source })?;
            defs.push(def);
        }
        defs.sort_by_key(|d| d.id);
        for (i, d) in defs.iter().enumerate() {
            if d.id == 0 || defs[..i].iter().any(|o| o.id == d.id) {
                return Err(PlantSpeciesError::BadId(d.id));
            }
            if defs[..i].iter().any(|o| o.name == d.name) {
                return Err(PlantSpeciesError::DuplicateName(d.name.clone()));
            }
        }
        let species = defs
            .iter()
            .map(|d| validate(d, materials, skip_unknown))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { species })
    }

    /// The species with this ID, if any.
    #[must_use]
    pub fn get(&self, id: PlantSpeciesId) -> Option<&PlantSpecies> {
        self.species
            .binary_search_by_key(&id, |s| s.id)
            .ok()
            .map(|i| &self.species[i])
    }

    /// The ID of the species with this name, if any.
    #[must_use]
    pub fn id(&self, name: &str) -> Option<PlantSpeciesId> {
        self.species.iter().find(|s| s.name == name).map(|s| s.id)
    }

    /// The species a seed or cutting with `state` grows into: the low-byte
    /// id if it exists, else the default (the lowest id).
    #[must_use]
    pub fn for_state(&self, state: u16) -> Option<&PlantSpecies> {
        self.get(PlantSpeciesId(propagule_species(state)))
            .or_else(|| self.species.first())
    }

    /// The default species (the lowest id), for plant cells made by edits
    /// and for seeds and cuttings poured as plain materials.
    #[must_use]
    pub fn default_species(&self) -> Option<&PlantSpecies> {
        self.species.first()
    }

    /// All species, ordered by ID.
    pub fn iter(&self) -> impl Iterator<Item = &PlantSpecies> {
        self.species.iter()
    }

    /// Number of species.
    #[must_use]
    pub fn len(&self) -> usize {
        self.species.len()
    }

    /// Whether there are no species.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.species.is_empty()
    }
}

/// The error for a species without exactly its form's table.
const ONE_TABLE: &str = "needs exactly the [stem], [mat], [rosette], [carpet], [floater], [epiphyte] or [ball] table its form names";

#[allow(clippy::too_many_lines)] // one check per property
fn validate(
    d: &SpeciesDef,
    materials: &MaterialTable,
    skip_unknown: bool,
) -> Result<PlantSpecies, PlantSpeciesError> {
    let invalid = |reason| PlantSpeciesError::Invalid {
        name: d.name.clone(),
        reason,
    };
    let lookup = |names: &[String]| -> Result<Vec<MaterialId>, PlantSpeciesError> {
        let mut out = Vec::with_capacity(names.len());
        for n in names {
            match materials.id(n) {
                Some(id) => out.push(id),
                None if skip_unknown => {}
                None => {
                    return Err(PlantSpeciesError::UnknownMaterial {
                        name: d.name.clone(),
                        material: n.clone(),
                    });
                }
            }
        }
        Ok(out)
    };
    if d.scientific_name.trim().is_empty() {
        return Err(invalid("scientific_name must not be empty"));
    }
    if d.description.trim().is_empty() {
        return Err(invalid("description must not be empty"));
    }
    let tables = [
        d.stem.is_some(),
        d.mat.is_some(),
        d.rosette.is_some(),
        d.carpet.is_some(),
        d.floater.is_some(),
        d.epiphyte.is_some(),
        d.ball.is_some(),
    ];
    if tables.iter().filter(|&&t| t).count() != 1 {
        return Err(invalid(ONE_TABLE));
    }
    let form = match (d.form, &d.stem, &d.mat) {
        (FormName::Ball, ..) => {
            let Some(b) = &d.ball else {
                return Err(invalid(ONE_TABLE));
            };
            if !(1..=MAX_BALL_RADIUS).contains(&b.radius) {
                return Err(invalid("ball radius must be 1..=8"));
            }
            Form::Ball(BallForm { radius: b.radius })
        }
        (FormName::Epiphyte, ..) => {
            let Some(e) = &d.epiphyte else {
                return Err(invalid(ONE_TABLE));
            };
            let max = MAX_POSITION;
            if !(1..=MAX_RHIZOME).contains(&e.rhizome_length)
                || !(1..=MAX_RHIZOME).contains(&e.spacing)
                || !(1..=max).contains(&e.leaf_length)
                || e.lean > max
            {
                return Err(invalid(
                    "epiphyte rhizome_length and spacing must be 1..=32, leaf_length 1..=63 and lean 0..=63",
                ));
            }
            Form::Epiphyte(EpiphyteForm {
                on: lookup(&e.on)?,
                rhizome_length: e.rhizome_length,
                spacing: e.spacing,
                leaf_length: e.leaf_length,
                lean: e.lean,
            })
        }
        (FormName::Floater, ..) => {
            let Some(f) = &d.floater else {
                return Err(invalid(ONE_TABLE));
            };
            if !(1..=MAX_FRONDS).contains(&f.fronds)
                || !(1..=f.fronds.div_ceil(2)).contains(&f.roots)
                || !(1..=MAX_HANGING_ROOT).contains(&f.root_length)
            {
                return Err(invalid(
                    "floater fronds must be 1..=8, roots 1..=(fronds + 1) / 2 and root_length 1..=32",
                ));
            }
            Form::Floater(FloaterForm {
                fronds: f.fronds,
                roots: f.roots,
                root_length: f.root_length,
            })
        }
        (FormName::Rosette | FormName::Carpet, ..) => {
            let carpet = d.form == FormName::Carpet;
            let Some(r) = (if carpet { &d.carpet } else { &d.rosette }) else {
                return Err(invalid(ONE_TABLE));
            };
            let max = MAX_POSITION;
            let crown = match (carpet, r.crown) {
                (true, None) => 1,
                (false, Some(c)) if (1..=MAX_CROWN).contains(&c) => c,
                (true, Some(_)) => return Err(invalid("a carpet has no crown key (it is 1)")),
                (false, _) => return Err(invalid("rosette crown must be 1..=3")),
            };
            let longest = if carpet { MAX_CARPET_BLADE } else { max };
            if !(1..=crown + 2).contains(&r.blades) {
                return Err(invalid("blades must be 1..=crown + 2"));
            }
            if !(1..=longest).contains(&r.blade_length) {
                return Err(invalid(
                    "blade_length must be 1..=63 (1..=3 for a carpet, which stays low)",
                ));
            }
            if r.lean > max || !(1..=max).contains(&r.root_depth) {
                return Err(invalid("lean must be 0..=63 and root_depth 1..=63"));
            }
            if !(2..=MAX_SPACING).contains(&r.spacing) {
                return Err(invalid("runner spacing must be 2..=48"));
            }
            if r.runner_depth > MAX_RUNNER_DEPTH || (carpet && r.runner_depth > 0) {
                return Err(invalid("runner_depth must be 0..=8, and 0 for a carpet"));
            }
            if r.pad > 0 && (carpet || r.pad > MAX_PAD || r.blade_length + r.pad >= max) {
                return Err(invalid(
                    "pad must be 1..=4, on a rosette, with blade_length + pad < 63",
                ));
            }
            let f = RosetteForm {
                crown,
                blades: r.blades,
                blade_length: r.blade_length,
                lean: r.lean,
                root_depth: r.root_depth,
                spacing: r.spacing,
                runner_depth: r.runner_depth,
                substrates: lookup(&r.substrates)?,
                pad: r.pad,
            };
            if carpet {
                Form::Carpet(f)
            } else {
                Form::Rosette(f)
            }
        }
        (FormName::Stem, Some(s), None) => {
            let max = MAX_POSITION;
            if !(1..=max).contains(&s.height)
                || !(2..=max).contains(&s.internode)
                || !(1..LEAF_UPPER).contains(&s.leaf_length)
                || !(1..=max).contains(&s.root_depth)
            {
                return Err(invalid(
                    "stem height and root_depth must be 1..=63, leaf_length 1..=31 (the upper cells' positions start at 32) and internode 2..=63",
                ));
            }
            if !(s.branching.is_finite() && (0.0..=1.0).contains(&s.branching)) {
                return Err(invalid("stem branching must be within 0..=1"));
            }
            Form::Stem(StemForm {
                height: s.height,
                internode: s.internode,
                leaf_length: s.leaf_length,
                root_depth: s.root_depth,
                branching: s.branching,
                substrates: lookup(&s.substrates)?,
            })
        }
        (FormName::Mat, None, Some(m)) => {
            if !(1..=MAX_MAT_LAYERS).contains(&m.layers) {
                return Err(invalid("mat layers must be 1..=4"));
            }
            Form::Mat(MatForm {
                on: lookup(&m.on)?,
                layers: m.layers,
            })
        }
        _ => return Err(invalid(ONE_TABLE)),
    };
    for f in Factor::ALL {
        let r = d.prefer.range(f);
        let finite = r.min.is_finite() && r.optimum.is_finite() && r.max.is_finite();
        if !(finite && r.min < r.optimum && r.optimum < r.max) {
            return Err(invalid("each preference needs min < optimum < max"));
        }
    }
    let g = &d.growth;
    if !(g.rate.is_finite() && g.rate > 0.0 && g.rate <= MAX_RATE) {
        return Err(invalid(
            "growth rate must be within (0, 90] cells per minute",
        ));
    }
    if !(1..=100_000).contains(&g.size) {
        return Err(invalid("growth size must be 1..=100000 cells"));
    }
    if !(g.cost.is_finite() && (0.0..=1.0).contains(&g.cost)) {
        return Err(invalid("growth cost must be within 0..=1"));
    }
    if g.lifespan_days.is_some_and(|l| !(l.is_finite() && l > 0.0)) {
        return Err(invalid("lifespan_days must be > 0 life days"));
    }
    let h = &d.hardiness;
    let ok = |v: f32| v.is_finite() && v >= 0.0;
    if !(ok(h.tolerance) && ok(h.decline) && h.decline > 0.0 && ok(h.recovery)) {
        return Err(invalid(
            "hardiness tolerance and recovery must be >= 0 and decline > 0",
        ));
    }
    let mut propagation = Propagation::default();
    for p in &d.propagation {
        match p {
            Propagule::Seeds => propagation.seeds = true,
            Propagule::Cuttings => propagation.cuttings = true,
            Propagule::Fragments => propagation.fragments = true,
            Propagule::Runners => propagation.runners = true,
            Propagule::Buds => propagation.buds = true,
        }
    }
    if propagation == Propagation::default() {
        return Err(invalid("propagation needs at least one way to spread"));
    }
    if propagation.runners && form.rosette().is_none() {
        return Err(invalid("only rosettes and carpets spread by runners"));
    }
    if propagation.buds != form.floater().is_some() {
        return Err(invalid("floaters, and only floaters, spread by buds"));
    }
    if g.root_feeder && form.substrates().is_none() {
        return Err(invalid("only a plant that roots can be a root feeder"));
    }
    let mut ramps = [Vec::new(), Vec::new(), Vec::new()];
    for (key, shades) in &d.colours {
        let slot = match key.as_str() {
            "root" => 0,
            "stem" => 1,
            "leaf" => 2,
            _ => return Err(invalid("colours are given for root, stem and leaf")),
        };
        if !(1..=MAX_RAMP).contains(&shades.len()) {
            return Err(invalid("a colour ramp has 1..=8 shades"));
        }
        for s in shades {
            ramps[slot]
                .push(parse_colour(s).ok_or_else(|| invalid("colours must be \"#rrggbb\""))?);
        }
    }
    if ramps.iter().any(Vec::is_empty) {
        return Err(invalid("colours need a root, stem and leaf ramp"));
    }
    let [root, stem, leaf] = ramps;
    let (default_leaf, default_stem) = match form {
        Form::Stem(_) => (LeafShape::Lanceolate, StemShape::Stalk),
        Form::Mat(_) => (LeafShape::Lanceolate, StemShape::Fuzz),
        Form::Rosette(_) => (LeafShape::Ribbon, StemShape::Thick),
        Form::Carpet(_) => (LeafShape::Grass, StemShape::Thick),
        Form::Floater(_) => (LeafShape::Lens, StemShape::Stalk),
        Form::Epiphyte(_) => (LeafShape::Sword, StemShape::Thick),
        Form::Ball(_) => (LeafShape::Lanceolate, StemShape::Velvet),
    };
    let look = Look {
        leaf: d.look.leaf.unwrap_or(default_leaf),
        stem: d.look.stem.unwrap_or(default_stem),
        root: d.look.root.unwrap_or_default(),
        arrangement: d.look.arrangement.unwrap_or_default(),
    };
    let mat_shape = matches!(look.stem, StemShape::Fuzz | StemShape::Frond);
    if mat_shape != matches!(form, Form::Mat(_))
        || (look.stem == StemShape::Velvet) != matches!(form, Form::Ball(_))
    {
        return Err(invalid(
            "look stem is stalk or thick for a stem, rosette, carpet or epiphyte, fuzz or frond for a mat, velvet for a ball",
        ));
    }
    let pads = form.rosette().is_some_and(|f| f.pad > 0);
    if (look.leaf == LeafShape::Lotus) != pads {
        return Err(invalid(
            "look leaf is lotus exactly for a rosette with leaf pads",
        ));
    }
    if look.leaf.is_blade() != (form.rosette().is_some() || form.epiphyte().is_some()) {
        return Err(invalid(
            "look leaf is a blade shape (ribbon, sword, grass, pad, lotus) exactly for rosettes, carpets and epiphytes",
        ));
    }
    if look.leaf.is_floating() != form.floater().is_some() {
        return Err(invalid(
            "look leaf is a floating shape (lens, disc) exactly for floaters",
        ));
    }
    let colouration = match &d.colouration {
        None => None,
        Some(c) => {
            let colour = parse_colour(&c.colour)
                .ok_or_else(|| invalid("colouration colour must be \"#rrggbb\""))?;
            let ok = |v: f32, max: f32| v.is_finite() && v > 0.0 && v <= max;
            if !ok(c.light, 4.0) || !ok(c.nutrient, 8.0) {
                return Err(invalid(
                    "colouration light must be within (0, 4] and nutrient within (0, 8]",
                ));
            }
            Some(Colouration {
                colour,
                light: c.light,
                nutrient: c.nutrient,
            })
        }
    };
    Ok(PlantSpecies {
        id: PlantSpeciesId(d.id),
        name: d.name.clone(),
        scientific_name: d.scientific_name.clone(),
        description: d.description.clone(),
        palette: d
            .palette
            .as_deref()
            .map(|p| PalettePath::parse(p, Holds::Plants))
            .transpose()
            .map_err(|source| PlantSpeciesError::Palette {
                name: d.name.clone(),
                source,
            })?,
        form,
        prefer: d.prefer,
        growth: Growth {
            rate: g.rate,
            size: g.size,
            cost: g.cost,
            // Life days to world seconds.
            #[allow(clippy::cast_precision_loss)] // 600 is exact
            lifespan: g.lifespan_days.map(|d| d * LIFE_DAY_SECONDS as f32),
            root_feeder: g.root_feeder,
        },
        hardiness: Hardiness {
            tolerance: h.tolerance,
            decline: h.decline,
            recovery: h.recovery,
        },
        propagation,
        palatable: d.palatable,
        colours: Ramps { root, stem, leaf },
        look,
        colouration,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r##"
        id = 9
        name = "test_weed"
        scientific_name = "Testus weedus"
        description = "A test plant."
        form = "stem"
        propagation = ["cuttings"]
        [stem]
        height = 20
        internode = 2
        leaf_length = 3
        root_depth = 6
        substrates = ["sand"]
        [prefer]
        light = { min = 0.1, optimum = 0.5, max = 1.0 }
        nutrient = { min = 0.0, optimum = 0.4, max = 2.0 }
        co2 = { min = -1.0, optimum = 1.0, max = 3.0 }
        [growth]
        rate = 10.0
        size = 100
        cost = 0.01
        [hardiness]
        tolerance = 10.0
        decline = 0.01
        recovery = 0.02
        [colours]
        root = ["#d9cfa8"]
        stem = ["#2f5a20", "#3f7a2e"]
        leaf = ["#4c8a30", "#6dbb45", "#8fd660"]
    "##;

    fn materials() -> MaterialTable {
        MaterialTable::builtin().unwrap()
    }

    #[test]
    fn builtin_species_have_card_text() {
        let m = materials();
        let t = PlantSpeciesTable::builtin_for(&m).unwrap();
        for s in t.iter() {
            let n = &s.name;
            assert!(!s.scientific_name.trim().is_empty(), "{n} scientific_name");
            assert!(!s.description.trim().is_empty(), "{n} description");
            assert!(!s.describe_form().is_empty(), "{n} form");
            assert!(!s.describe_requirements(&m).is_empty(), "{n} requirements");
            assert!(!s.describe_propagation().is_empty(), "{n} propagation");
        }
    }

    #[test]
    fn blank_scientific_name_or_description_is_rejected() {
        for (from, to) in [
            (
                "scientific_name = \"Testus weedus\"",
                "scientific_name = \"\"",
            ),
            (
                "scientific_name = \"Testus weedus\"",
                "scientific_name = \"  \"",
            ),
            ("description = \"A test plant.\"", "description = \"\""),
            ("description = \"A test plant.\"", "description = \" \\t \""),
        ] {
            let src = OK.replace(from, to);
            assert_ne!(src, OK, "{from:?} not in the template");
            let r = PlantSpeciesTable::from_toml_sources(&[&src], &materials());
            assert!(matches!(r, Err(PlantSpeciesError::Invalid { .. })), "{r:?}");
        }
        for line in [
            "scientific_name = \"Testus weedus\"",
            "description = \"A test plant.\"",
        ] {
            let src = OK.replace(line, "");
            assert_ne!(src, OK, "{line:?} not in the template");
            let r = PlantSpeciesTable::from_toml_sources(&[&src], &materials());
            assert!(matches!(r, Err(PlantSpeciesError::Parse { .. })), "{r:?}");
        }
    }

    #[test]
    fn describes_species_for_cards() {
        let m = materials();
        let t = PlantSpeciesTable::builtin_for(&m).unwrap();
        let get = |n: &str| t.get(t.id(n).unwrap()).unwrap();
        let soil = Some("sand, gravel, aquasoil, clown vomit gravel");

        let hygro = get("hygrophila");
        assert_eq!(hygro.scientific_name, "Hygrophila polysperma");
        assert_eq!(hygro.describe_form(), "stem");
        assert_eq!(hygro.describe_requirements(&m), "low light");
        assert_eq!(hygro.describe_rooting_substrates(&m).as_deref(), soil);
        assert_eq!(hygro.describe_propagation(), "seeds, cuttings, fragments");

        let sword = get("amazon_sword");
        assert_eq!(sword.describe_form(), "rosette");
        assert_eq!(sword.describe_requirements(&m), "low light, root feeder");
        assert_eq!(sword.describe_rooting_substrates(&m).as_deref(), soil);
        assert_eq!(sword.describe_propagation(), "runners");

        let carpet = get("monte_carlo");
        assert_eq!(carpet.describe_form(), "carpet");
        assert_eq!(
            carpet.describe_requirements(&m),
            "strong light, benefits from CO₂"
        );
        assert_eq!(carpet.describe_rooting_substrates(&m).as_deref(), soil);
        assert_eq!(carpet.describe_propagation(), "fragments, runners");

        let anubias = get("anubias");
        assert_eq!(anubias.describe_form(), "epiphyte");
        assert_eq!(
            anubias.describe_requirements(&m),
            "low light, clings to rock, dragon stone, seiryu stone, driftwood"
        );
        assert_eq!(anubias.describe_propagation(), "fragments");

        let moss = get("java_moss");
        assert_eq!(moss.describe_form(), "mat");
        assert_eq!(
            moss.describe_requirements(&m),
            "low light, clings to rock, dragon stone, seiryu stone, driftwood"
        );

        let duckweed = get("duckweed");
        assert_eq!(duckweed.describe_form(), "floater");
        assert_eq!(
            duckweed.describe_requirements(&m),
            "strong light, floats at surface"
        );
        assert_eq!(duckweed.describe_propagation(), "fragments, buds");

        let marimo = get("marimo");
        assert_eq!(marimo.describe_form(), "ball");
        assert_eq!(
            marimo.describe_requirements(&m),
            "low light, rests on bottom"
        );
        for plant in [anubias, moss, duckweed, marimo] {
            assert_eq!(
                plant.describe_rooting_substrates(&m),
                None,
                "{}",
                plant.name
            );
        }
    }

    #[test]
    fn requirement_bands_follow_the_ranges() {
        let m = materials();
        let t = PlantSpeciesTable::builtin_for(&m).unwrap();
        let mut s = t.get(t.id("hygrophila").unwrap()).unwrap().clone();
        for (optimum, band) in [
            (0.44, "low light"),
            (0.45, "moderate light"),
            (0.69, "moderate light"),
            (0.70, "strong light"),
        ] {
            s.prefer.light.optimum = optimum;
            assert!(s.describe_requirements(&m).starts_with(band), "{optimum}");
        }
        s.prefer.co2.min = 0.0;
        assert!(s.describe_requirements(&m).contains("needs CO₂"));
        s.prefer.co2.min = -0.5;
        assert!(s.describe_requirements(&m).contains("benefits from CO₂"));
        s.prefer.co2.min = -0.8;
        assert!(!s.describe_requirements(&m).contains("CO₂"));
    }

    #[test]
    fn builtin_species_load() {
        let m = materials();
        let t = PlantSpeciesTable::builtin_for(&m).unwrap();
        assert_eq!(t.len(), 18);
        let first = t.default_species().unwrap();
        assert_eq!(first.name, "hygrophila", "T6's plant is the default");
        assert!(matches!(first.form, Form::Stem(_)));
        let moss = t.get(t.id("java_moss").unwrap()).unwrap();
        assert!(matches!(&moss.form, Form::Mat(m) if m.on.len() == 4));
        assert_eq!(t.for_state(0).unwrap().id, first.id);
        assert_eq!(t.for_state(u16::from(moss.id.0)).unwrap().id, moss.id);
        assert_eq!(
            t.for_state(propagule_state(moss.id.0, 123)).unwrap().id,
            moss.id
        );
        assert_eq!(t.for_state(propagule_state(255, 42)).unwrap().id, first.id);
        // The two stem plants want different light, so each wins somewhere.
        let rotala = t.get(t.id("rotala").unwrap()).unwrap();
        assert!(rotala.prefer.light.optimum > first.prefer.light.optimum);
        assert!(rotala.prefer.light.min > first.prefer.light.min);
    }

    #[test]
    fn species_list_their_propagules_and_default_to_cuttings() {
        let m = materials();
        let t = PlantSpeciesTable::builtin_for(&m).unwrap();
        let get = |n| t.get(t.id(n).unwrap()).unwrap();
        let ways = |n| get(n).propagules().collect::<Vec<_>>();
        assert_eq!(
            ways("hygrophila"),
            [Propagule::Seeds, Propagule::Cuttings, Propagule::Fragments]
        );
        assert_eq!(ways("rotala"), [Propagule::Cuttings]);
        assert_eq!(ways("java_moss"), [Propagule::Fragments]);
        assert_eq!(get("hygrophila").default_propagule(), Propagule::Cuttings);
        assert_eq!(get("java_moss").default_propagule(), Propagule::Fragments);
        // Every propagule pours a material the built-in table has.
        for p in Propagule::ALL {
            assert_eq!(Propagule::from_name(p.name()), Some(p));
            assert!(m.id(p.material_name()).is_some(), "{p:?}");
        }
        assert_eq!(Propagule::from_name("spores"), None);
    }

    #[test]
    fn responses_are_linear_with_a_peak_at_the_optimum() {
        let r = Range {
            min: 0.0,
            optimum: 1.0,
            max: 3.0,
        };
        assert!((r.response(0.5) - 0.5).abs() < 1e-6);
        assert!((r.response(1.0) - 1.0).abs() < 1e-6);
        assert!((r.response(2.0) - 0.5).abs() < 1e-6);
        assert!(r.response(0.0).abs() < 1e-6 && r.response(3.5).abs() < 1e-6);
        assert!(r.response(f32::NAN).abs() < 1e-6);
        let t = PlantSpeciesTable::from_toml_sources(&[OK], &materials()).unwrap();
        let s = t.get(PlantSpeciesId(9)).unwrap();
        // Light 0.2 is a quarter of the way to the optimum; no CO₂ is half
        // the CO₂ response; nutrient at its optimum.
        let (p, limit) = s.prefer.response([0.2, 0.4, 0.0]);
        assert!((p - 0.125).abs() < 1e-6, "{p}");
        assert_eq!(limit, Limit::TooLow(Factor::Light));
        let (p, limit) = s.prefer.response([0.5, 2.5, 1.0]);
        assert!(p.abs() < 1e-6);
        assert_eq!(limit, Limit::TooHigh(Factor::Nutrient));
        assert_eq!(limit.describe(), "nutrients too high");
        assert_eq!(s.prefer.response([0.5, 0.4, 1.0]).1, Limit::None);
    }

    #[test]
    fn rejects_bad_definitions() {
        let m = materials();
        let bad = |from: &str, to: &str| {
            let src = OK.replace(from, to);
            assert_ne!(src, OK, "{from:?} not in the template");
            PlantSpeciesTable::from_toml_sources(&[&src], &m)
        };
        let invalid = |r: Result<PlantSpeciesTable, PlantSpeciesError>| {
            assert!(matches!(r, Err(PlantSpeciesError::Invalid { .. })), "{r:?}");
        };
        invalid(bad("optimum = 0.5", "optimum = 0.05"));
        invalid(bad("internode = 2", "internode = 1"));
        invalid(bad("height = 20", "height = 64"));
        // Stem leaves must stay below the upper cells' positions.
        invalid(bad("leaf_length = 3", "leaf_length = 32"));
        invalid(bad("leaf_length = 3", "leaf_length = 0"));
        invalid(bad("rate = 10.0", "rate = 0.0"));
        invalid(bad("decline = 0.01", "decline = 0.0"));
        invalid(bad("[\"cuttings\"]", "[]"));
        invalid(bad("form = \"stem\"", "form = \"mat\""));
        invalid(bad("root = [\"#d9cfa8\"]", "root = []"));
        invalid(bad("root = [\"#d9cfa8\"]", "trunk = [\"#d9cfa8\"]"));
        assert!(matches!(
            bad("[\"sand\"]", "[\"lava\"]"),
            Err(PlantSpeciesError::UnknownMaterial { .. })
        ));
        assert!(matches!(
            bad("[\"cuttings\"]", "[\"spores\"]"),
            Err(PlantSpeciesError::Parse { .. })
        ));
        assert!(matches!(
            bad("id = 9", "id = 0"),
            Err(PlantSpeciesError::BadId(0))
        ));
        assert!(matches!(
            PlantSpeciesTable::from_toml_sources(&[OK, OK], &m),
            Err(PlantSpeciesError::BadId(9))
        ));
    }

    #[test]
    fn lifespan_is_given_in_life_days() {
        let m = materials();
        let with = |days: &str| {
            let src = OK.replace(
                "cost = 0.01",
                &format!("cost = 0.01\nlifespan_days = {days}"),
            );
            PlantSpeciesTable::from_toml_sources(&[&src], &m)
        };
        // Optional, and none by default.
        let plain = PlantSpeciesTable::from_toml_sources(&[OK], &m).unwrap();
        assert_eq!(plain.get(PlantSpeciesId(9)).unwrap().growth.lifespan, None);
        // A life day is `LIFE_DAY_SECONDS` of world time; fractions work.
        for (days, seconds) in [("1", 600.0), ("30", 18_000.0), ("0.5", 300.0)] {
            let t = with(days).unwrap();
            let span = t.get(PlantSpeciesId(9)).unwrap().growth.lifespan.unwrap();
            assert!((span - seconds).abs() < 1e-3, "{days}: {span}");
        }
        for bad in ["0", "-1", "nan", "inf"] {
            assert!(
                matches!(with(bad), Err(PlantSpeciesError::Invalid { .. })),
                "{bad}"
            );
        }
        // The old key in seconds is gone.
        let old = OK.replace("cost = 0.01", "cost = 0.01\nlifespan = 600.0");
        assert!(matches!(
            PlantSpeciesTable::from_toml_sources(&[&old], &m),
            Err(PlantSpeciesError::Parse { .. })
        ));
    }

    #[test]
    fn colouration_is_read_and_checked() {
        let m = materials();
        let with = |c: &str| {
            let src = format!("{OK}\n[colouration]\n{c}\n");
            PlantSpeciesTable::from_toml_sources(&[&src], &m)
        };
        let t = with("colour = \"#e0566a\"\nlight = 0.6\nnutrient = 0.8").unwrap();
        let s = t.get(PlantSpeciesId(9)).unwrap();
        let c = s.colouration.unwrap();
        assert_eq!(c.colour, [0xe0, 0x56, 0x6a]);
        // None in dim light or rich water; full in strong light with water
        // at half the nutrient threshold or leaner.
        assert!(c.target(0.5, 0.0).abs() < f32::EPSILON);
        assert!(c.target(1.0, 0.9).abs() < f32::EPSILON);
        assert!((c.target(0.6 + COLOURATION_LIGHT_SPAN, 0.4) - 1.0).abs() < 1e-6);
        assert!((c.target(2.0, 0.0) - 1.0).abs() < 1e-6);
        let half = c.target(0.6 + COLOURATION_LIGHT_SPAN / 2.0, 0.2);
        assert!((half - 0.5).abs() < 1e-5, "{half}");
        // Optional: the template has none.
        let plain = PlantSpeciesTable::from_toml_sources(&[OK], &m).unwrap();
        assert!(plain.get(PlantSpeciesId(9)).unwrap().colouration.is_none());
        for bad in [
            "colour = \"red\"\nlight = 0.6\nnutrient = 0.8",
            "colour = \"#e0566a\"\nlight = 0.0\nnutrient = 0.8",
            "colour = \"#e0566a\"\nlight = 0.6\nnutrient = -1.0",
        ] {
            assert!(
                matches!(with(bad), Err(PlantSpeciesError::Invalid { .. })),
                "{bad}"
            );
        }
        assert!(matches!(
            with("colour = \"#e0566a\"\nlight = 0.6"),
            Err(PlantSpeciesError::Parse { .. })
        ));
        // Rotala and ludwigia colour; hygrophila doesn't.
        let t = PlantSpeciesTable::builtin_for(&m).unwrap();
        let colours = |n: &str| t.get(t.id(n).unwrap()).unwrap().colouration.is_some();
        assert!(colours("rotala") && colours("ludwigia") && !colours("hygrophila"));
    }

    #[test]
    fn looks_are_read_with_defaults_by_form() {
        let m = materials();
        let t = PlantSpeciesTable::builtin_for(&m).unwrap();
        let look = |n: &str| t.get(t.id(n).unwrap()).unwrap().look;
        assert_eq!(look("hygrophila").leaf, LeafShape::Lanceolate);
        assert_eq!(look("rotala").leaf, LeafShape::Needle);
        assert_eq!(look("java_moss").stem, StemShape::Fuzz);
        // Absent keys take the defaults.
        let s = PlantSpeciesTable::from_toml_sources(&[OK], &m).unwrap();
        assert_eq!(s.get(PlantSpeciesId(9)).unwrap().look, Look::default());
        let custom =
            format!("{OK}\n[look]\nleaf = \"feather\"\nstem = \"thick\"\nroot = \"thick\"\n");
        let s = PlantSpeciesTable::from_toml_sources(&[&custom], &m).unwrap();
        let look = s.get(PlantSpeciesId(9)).unwrap().look;
        assert_eq!(
            (look.leaf, look.stem, look.root),
            (LeafShape::Feather, StemShape::Thick, RootShape::Thick)
        );
        // Leaves are arranged in opposite pairs unless a species asks for a
        // whorl (a data option no shipped species uses yet).
        for sp in t.iter() {
            assert_eq!(sp.look.arrangement, Arrangement::Opposite, "{}", sp.name);
        }
        let whorled = format!("{OK}\n[look]\narrangement = \"whorl\"\n");
        let s = PlantSpeciesTable::from_toml_sources(&[&whorled], &m).unwrap();
        assert_eq!(
            s.get(PlantSpeciesId(9)).unwrap().look.arrangement,
            Arrangement::Whorl
        );
        let odd = format!("{OK}\n[look]\narrangement = \"spiral\"\n");
        assert!(matches!(
            PlantSpeciesTable::from_toml_sources(&[&odd], &m),
            Err(PlantSpeciesError::Parse { .. })
        ));
        // A mat shape on a stem plant is invalid.
        let mat_on_stem = format!("{OK}\n[look]\nstem = \"fuzz\"\n");
        assert!(matches!(
            PlantSpeciesTable::from_toml_sources(&[&mat_on_stem], &m),
            Err(PlantSpeciesError::Invalid { .. })
        ));
        // Unknown shapes and keys are errors.
        for bad in ["leaf = \"spiky\"", "flower = \"round\""] {
            let src = format!("{OK}\n[look]\n{bad}\n");
            assert!(matches!(
                PlantSpeciesTable::from_toml_sources(&[&src], &m),
                Err(PlantSpeciesError::Parse { .. })
            ));
        }
    }

    #[test]
    fn ludwigia_cabomba_bacopa_and_christmas_moss_load_with_their_care_and_look() {
        let m = materials();
        let t = PlantSpeciesTable::builtin_for(&m).unwrap();
        let get = |n: &str| t.get(t.id(n).unwrap()).unwrap();
        let stem = |n: &str| match &get(n).form {
            Form::Stem(f) => f.clone(),
            _ => panic!("{n} is a stem plant"),
        };
        let (hyg, rotala, java) = (get("hygrophila"), get("rotala"), get("java_moss"));
        let (ludwigia, cabomba, bacopa, xmas) = (
            get("ludwigia"),
            get("cabomba"),
            get("bacopa"),
            get("christmas_moss"),
        );
        // Ludwigia: round leaves on a stalk, reddening up the stem; light
        // between hygrophila's and rotala's.
        assert_eq!(
            (ludwigia.look.leaf, ludwigia.look.stem),
            (LeafShape::Round, StemShape::Stalk)
        );
        let red = |c: [u8; 3]| c[0] > c[1] && c[0] > c[2];
        assert!(red(*ludwigia.colours.leaf.last().unwrap()));
        assert!(!red(ludwigia.colours.leaf[0]));
        assert!(ludwigia.prefer.light.optimum > hyg.prefer.light.optimum);
        assert!(ludwigia.prefer.light.optimum < rotala.prefer.light.optimum);
        // Cabomba: feathery fans, the longest leaves, fast, wanting fairly
        // strong light and quick to suffer outside it.
        assert_eq!(cabomba.look.leaf, LeafShape::Feather);
        assert!(stem("cabomba").leaf_length > stem("hygrophila").leaf_length);
        assert!(cabomba.prefer.light.min > ludwigia.prefer.light.min);
        assert!(cabomba.growth.rate > rotala.growth.rate);
        assert!(cabomba.hardiness.tolerance < hyg.hardiness.tolerance);
        // Bacopa: round leaves on a thick stem, short and fleshy; the
        // slowest stem plant and the hardiest.
        assert_eq!(
            (bacopa.look.leaf, bacopa.look.stem),
            (LeafShape::Round, StemShape::Thick)
        );
        assert!(stem("bacopa").leaf_length < stem("ludwigia").leaf_length);
        for other in [hyg, rotala, ludwigia, cabomba] {
            assert!(bacopa.growth.rate < other.growth.rate, "{}", other.name);
            assert!(bacopa.hardiness.tolerance > other.hardiness.tolerance);
        }
        // Christmas moss: a frond mat, brighter than java moss and slower.
        assert!(matches!(&xmas.form, Form::Mat(f) if f.on.len() == 4));
        assert_eq!(
            (java.look.stem, xmas.look.stem),
            (StemShape::Fuzz, StemShape::Frond)
        );
        let bright = |c: [u8; 3]| u32::from(c[0]) + u32::from(c[1]) + u32::from(c[2]);
        assert!(bright(xmas.colour()) > bright(java.colour()));
        assert!(xmas.growth.rate < java.growth.rate);
        assert_eq!(
            xmas.propagules().collect::<Vec<_>>(),
            [Propagule::Fragments]
        );
        // Each species has its own swatch colour in the picker.
        let all: Vec<_> = t.iter().collect();
        for (i, a) in all.iter().enumerate() {
            for b in &all[..i] {
                assert_ne!(a.colour(), b.colour(), "{} {}", a.name, b.name);
            }
        }
    }

    #[test]
    fn rosettes_and_carpets_load_with_their_care_and_look() {
        let m = materials();
        let t = PlantSpeciesTable::builtin_for(&m).unwrap();
        let get = |n: &str| t.get(t.id(n).unwrap()).unwrap();
        let rosette = |n: &str| get(n).form.rosette().unwrap().clone();
        let (sword, vals, crypt) = (get("amazon_sword"), get("vallisneria"), get("cryptocoryne"));
        let (grass, monte) = (get("dwarf_hairgrass"), get("monte_carlo"));
        for s in [sword, vals, crypt] {
            assert!(matches!(s.form, Form::Rosette(_)), "{}", s.name);
            assert!(s.propagation.runners, "{}", s.name);
        }
        for s in [grass, monte] {
            assert!(matches!(s.form, Form::Carpet(_)), "{}", s.name);
            assert!(s.grows_from(Propagule::Runners) && s.grows_from(Propagule::Fragments));
            let f = rosette(&s.name);
            assert!(f.crown == 1 && f.blade_length <= MAX_CARPET_BLADE);
            assert!(f.spacing < rosette("vallisneria").spacing);
        }
        // Looks: broad swords, twisting ribbons, fine grass, round pads.
        let looks = [sword, vals, grass, monte, crypt].map(|s| s.look.leaf);
        assert_eq!(
            looks,
            [
                LeafShape::Sword,
                LeafShape::Ribbon,
                LeafShape::Grass,
                LeafShape::Pad,
                LeafShape::Sword
            ]
        );
        assert!(looks.iter().all(|l| l.is_blade()));
        assert!(!LeafShape::Round.is_blade());
        // Vallisneria has the longest blades and the shortest runners of
        // the rosettes, and grows fastest; the sword is the widest and a
        // root feeder that wants rich soil; cryptocoryne the smallest and
        // slowest, wanting the least light.
        assert!(rosette("vallisneria").blade_length > rosette("amazon_sword").blade_length);
        assert!(rosette("vallisneria").spacing < rosette("amazon_sword").spacing);
        assert!(vals.growth.rate > sword.growth.rate && sword.growth.rate > crypt.growth.rate);
        assert!(rosette("amazon_sword").crown > rosette("vallisneria").crown);
        assert!(sword.growth.root_feeder && crypt.growth.root_feeder && !vals.growth.root_feeder);
        assert!(sword.prefer.nutrient.optimum > vals.prefer.nutrient.optimum);
        assert!(rosette("cryptocoryne").blade_length < rosette("amazon_sword").blade_length);
        for other in [sword, vals, grass, monte] {
            assert!(crypt.prefer.light.optimum < other.prefer.light.optimum);
            assert!(crypt.hardiness.tolerance > other.hardiness.tolerance);
        }
        // Monte Carlo wants the most light and CO₂ of the carpets.
        assert!(monte.prefer.light.min > grass.prefer.light.min);
        assert!(monte.prefer.co2.min > grass.prefer.co2.min);
    }

    #[test]
    fn rosette_and_carpet_definitions_are_checked() {
        let m = materials();
        let rosette = OK
            .replace("form = \"stem\"", "form = \"rosette\"")
            .replace("[\"cuttings\"]", "[\"runners\"]")
            .replace(
                "[stem]\n        height = 20\n        internode = 2\n        leaf_length = 3\n        root_depth = 6",
                "[rosette]\n        crown = 1\n        blades = 3\n        blade_length = 20\n        lean = 5\n        root_depth = 6\n        spacing = 6",
            );
        assert_ne!(rosette, OK);
        let t = PlantSpeciesTable::from_toml_sources(&[&rosette], &m).unwrap();
        let s = t.get(PlantSpeciesId(9)).unwrap();
        assert_eq!(
            s.look.leaf,
            LeafShape::Ribbon,
            "rosettes default to ribbons"
        );
        assert_eq!(s.look.stem, StemShape::Thick);
        assert_eq!(s.default_propagule(), Propagule::Runners);
        let carpet = rosette
            .replace("form = \"rosette\"", "form = \"carpet\"")
            .replace("[rosette]", "[carpet]")
            .replace("crown = 1\n", "")
            .replace("blade_length = 20", "blade_length = 3");
        let t = PlantSpeciesTable::from_toml_sources(&[&carpet], &m).unwrap();
        assert_eq!(
            t.get(PlantSpeciesId(9)).unwrap().look.leaf,
            LeafShape::Grass
        );
        let invalid = |src: String| {
            let r = PlantSpeciesTable::from_toml_sources(&[&src], &m);
            assert!(matches!(r, Err(PlantSpeciesError::Invalid { .. })), "{r:?}");
        };
        // Too many blades for the crown, a carpet too tall or with a crown,
        // runners too short, a stem leaf on a rosette, a blade on a stem
        // plant, runners or root feeding where they don't apply.
        invalid(rosette.replace("blades = 3", "blades = 4"));
        invalid(rosette.replace("crown = 1", "crown = 4"));
        invalid(carpet.replace("blade_length = 3", "blade_length = 4"));
        invalid(carpet.replace("blades = 3", "crown = 1\n        blades = 3"));
        invalid(rosette.replace("spacing = 6", "spacing = 1"));
        invalid(format!("{rosette}\n[look]\nleaf = \"round\"\n"));
        invalid(format!("{OK}\n[look]\nleaf = \"grass\"\n"));
        invalid(OK.replace("[\"cuttings\"]", "[\"cuttings\", \"runners\"]"));
        let mat = OK
            .replace("form = \"stem\"", "form = \"mat\"")
            .replace(
                "[stem]\n        height = 20\n        internode = 2\n        leaf_length = 3\n        root_depth = 6\n        substrates = [\"sand\"]",
                "[mat]\n        on = [\"rock\"]",
            )
            .replace("[look]", "");
        let mat = format!("{mat}\n[look]\nstem = \"fuzz\"\n");
        assert!(PlantSpeciesTable::from_toml_sources(&[&mat], &m).is_ok());
        invalid(mat.replace("cost = 0.01", "cost = 0.01\n        root_feeder = true"));
        invalid(format!(
            "{rosette}\n[stem]\nheight = 2\ninternode = 2\nleaf_length = 1\nroot_depth = 1\nsubstrates = []\n"
        ));
    }

    #[test]
    fn floaters_load_and_their_definitions_are_checked() {
        let m = materials();
        let t = PlantSpeciesTable::builtin_for(&m).unwrap();
        for (name, leaf) in [("duckweed", LeafShape::Lens), ("frogbit", LeafShape::Disc)] {
            let s = t.get(t.id(name).unwrap()).unwrap();
            let f = s.form.floater().unwrap();
            assert!(f.roots <= f.fronds.div_ceil(2), "{name}: strands fit");
            assert!(s.growth.size >= u32::from(f.fronds), "{name}: buds at size");
            assert_eq!(s.look.leaf, leaf);
            assert_eq!(
                s.propagules().collect::<Vec<_>>(),
                [Propagule::Fragments, Propagule::Buds]
            );
            assert!(s.form.substrates().is_none(), "{name} doesn't root");
        }
        let floater = OK
            .replace("form = \"stem\"", "form = \"floater\"")
            .replace("[\"cuttings\"]", "[\"buds\"]")
            .replace(
                "[stem]\n        height = 20\n        internode = 2\n        leaf_length = 3\n        root_depth = 6\n        substrates = [\"sand\"]",
                "[floater]\n        fronds = 3\n        roots = 2\n        root_length = 4",
            );
        assert_ne!(floater, OK);
        let t = PlantSpeciesTable::from_toml_sources(&[&floater], &m).unwrap();
        let s = t.get(PlantSpeciesId(9)).unwrap();
        assert_eq!(s.look.leaf, LeafShape::Lens, "floaters default to lenses");
        assert_eq!(s.default_propagule(), Propagule::Buds);
        let invalid = |src: String| {
            let r = PlantSpeciesTable::from_toml_sources(&[&src], &m);
            assert!(matches!(r, Err(PlantSpeciesError::Invalid { .. })), "{r:?}");
        };
        // Strands a cell apart, frond and root limits, buds only (and
        // always) for floaters, floating shapes only for floaters.
        invalid(floater.replace("roots = 2", "roots = 3"));
        invalid(floater.replace("fronds = 3", "fronds = 9"));
        invalid(floater.replace("root_length = 4", "root_length = 33"));
        invalid(floater.replace("[\"buds\"]", "[\"cuttings\"]"));
        invalid(OK.replace("[\"cuttings\"]", "[\"cuttings\", \"buds\"]"));
        invalid(format!("{floater}\n[look]\nleaf = \"round\"\n"));
        invalid(format!("{OK}\n[look]\nleaf = \"disc\"\n"));
    }

    #[test]
    fn epiphytes_layered_mats_and_branching_are_checked() {
        let m = materials();
        let t = PlantSpeciesTable::builtin_for(&m).unwrap();
        for (name, leaf) in [("anubias", LeafShape::Pad), ("java_fern", LeafShape::Sword)] {
            let s = t.get(t.id(name).unwrap()).unwrap();
            let e = s.form.epiphyte().unwrap();
            assert!(!e.on.is_empty(), "{name} clings to rock/wood");
            assert!(e.rhizome_length > 0 && e.leaf_length > 0);
            assert_eq!(s.look.leaf, leaf);
            assert_eq!(s.propagules().collect::<Vec<_>>(), [Propagule::Fragments]);
            assert!(s.form.substrates().is_none(), "{name} doesn't root");
        }
        let java_moss = t.get(t.id("java_moss").unwrap()).unwrap();
        let xmas_moss = t.get(t.id("christmas_moss").unwrap()).unwrap();
        assert!(matches!(&java_moss.form, Form::Mat(f) if f.layers == 1));
        assert!(matches!(&xmas_moss.form, Form::Mat(f) if f.layers == 3));
        let rotala = t.get(t.id("rotala").unwrap()).unwrap();
        assert!(matches!(&rotala.form, Form::Stem(f) if f.branching > 0.0));

        let epi = OK
            .replace("form = \"stem\"", "form = \"epiphyte\"")
            .replace("[\"cuttings\"]", "[\"fragments\"]")
            .replace(
                "[stem]\n        height = 20\n        internode = 2\n        leaf_length = 3\n        root_depth = 6\n        substrates = [\"sand\"]",
                "[epiphyte]\n        on = [\"rock\"]\n        rhizome_length = 10\n        spacing = 2\n        leaf_length = 6\n        lean = 4",
            );
        let t = PlantSpeciesTable::from_toml_sources(&[&epi], &m).unwrap();
        let s = t.get(PlantSpeciesId(9)).unwrap();
        assert_eq!(s.look.leaf, LeafShape::Sword, "epiphytes default to sword");
        assert_eq!(s.look.stem, StemShape::Thick);

        let invalid = |src: String| {
            let r = PlantSpeciesTable::from_toml_sources(&[&src], &m);
            assert!(matches!(r, Err(PlantSpeciesError::Invalid { .. })), "{r:?}");
        };
        invalid(epi.replace("rhizome_length = 10", "rhizome_length = 33"));
        invalid(epi.replace("spacing = 2", "spacing = 0"));
        invalid(epi.replace("leaf_length = 6", "leaf_length = 64"));
        invalid(format!("{epi}\n[look]\nleaf = \"round\"\n"));
        invalid(format!("{epi}\n[look]\nstem = \"fuzz\"\n"));
        invalid(OK.replace("height = 20", "height = 20\n        branching = 1.5"));
        let mat = OK
            .replace("form = \"stem\"", "form = \"mat\"")
            .replace(
                "[stem]\n        height = 20\n        internode = 2\n        leaf_length = 3\n        root_depth = 6\n        substrates = [\"sand\"]",
                "[mat]\n        on = [\"rock\"]\n        layers = 5",
            );
        invalid(mat);
    }

    #[test]
    #[allow(clippy::many_single_char_names)]
    fn marimo_and_tiger_lotus_load_with_their_care_and_look() {
        let m = materials();
        let t = PlantSpeciesTable::builtin_for(&m).unwrap();
        let get = |n: &str| t.get(t.id(n).unwrap()).unwrap();
        let (marimo, lotus) = (get("marimo"), get("tiger_lotus"));
        // Marimo: a velvet ball that spreads only by pieces breaking off,
        // happy in the least light, the slowest grower of all, and hardy.
        assert_eq!(marimo.form, Form::Ball(BallForm { radius: 4 }));
        assert!(marimo.form.drifts() && marimo.form.substrates().is_none());
        assert_eq!(marimo.look.stem, StemShape::Velvet);
        assert_eq!(
            marimo.propagules().collect::<Vec<_>>(),
            [Propagule::Fragments]
        );
        assert_eq!(marimo.default_propagule(), Propagule::Fragments);
        for other in t.iter().filter(|s| s.id != marimo.id) {
            assert!(marimo.growth.rate < other.growth.rate, "{}", other.name);
            assert!(
                marimo.prefer.light.min <= other.prefer.light.min,
                "{}",
                other.name
            );
        }
        assert!(marimo.hardiness.tolerance >= get("cryptocoryne").hardiness.tolerance);
        // A ball's mature size fits within its disc.
        assert!(marimo.growth.size <= 4 * 4 * 3 + 4 * 4);
        // Tiger lotus: a rosette whose petioles carry leaf pads, wanting
        // more light than the low-light rosettes, fast, a root feeder that
        // spreads by runners and reddens in strong light.
        let f = lotus.form.rosette().unwrap();
        assert!(matches!(lotus.form, Form::Rosette(_)));
        assert_eq!((f.pad, lotus.look.leaf), (4, LeafShape::Lotus));
        assert!(f.is_pad(f.blade_length + 1) && !f.is_pad(f.blade_length));
        assert!(f.pad_top() <= MAX_POSITION);
        assert!(f.blade_length > get("amazon_sword").form.rosette().unwrap().blade_length);
        assert!(lotus.prefer.light.optimum > get("amazon_sword").prefer.light.optimum);
        assert!(lotus.prefer.light.optimum > get("cryptocoryne").prefer.light.optimum);
        assert!(lotus.growth.rate > get("cryptocoryne").growth.rate);
        assert!(lotus.growth.root_feeder && lotus.colouration.is_some());
        assert_eq!(lotus.default_propagule(), Propagule::Runners);
        // Its runners creep deep in the bed and root new bulbs far further
        // out than any other runner's.
        assert!(f.runner_depth >= 3, "{f:?}");
        for other in t.iter().filter(|o| o.name != "tiger_lotus") {
            if let Some(o) = other.form.rosette() {
                assert!(f.spacing >= 2 * o.spacing, "{}", other.name);
                assert_eq!(o.runner_depth, 0, "{}", other.name);
            }
        }
        assert!(f.spacing >= 10 * get("monte_carlo").form.rosette().unwrap().spacing);
        assert!(f.runner_limit() > f.spacing && f.runner_limit() <= MAX_RUNNER);
        // Its leaf ramp runs from maroon (the blotches) to green.
        let red = |c: [u8; 3]| c[0] > c[1] && c[0] > c[2];
        assert!(red(lotus.colours.leaf[0]));
        assert!(!red(*lotus.colours.leaf.last().unwrap()));
    }

    #[test]
    #[allow(clippy::many_single_char_names)]
    fn balls_and_leaf_pads_are_checked() {
        let m = materials();
        let ball = OK
            .replace("form = \"stem\"", "form = \"ball\"")
            .replace("[\"cuttings\"]", "[\"fragments\"]")
            .replace(
                "[stem]\n        height = 20\n        internode = 2\n        leaf_length = 3\n        root_depth = 6\n        substrates = [\"sand\"]",
                "[ball]\n        radius = 3",
            );
        assert_ne!(ball, OK);
        let t = PlantSpeciesTable::from_toml_sources(&[&ball], &m).unwrap();
        let s = t.get(PlantSpeciesId(9)).unwrap();
        assert_eq!(s.look.stem, StemShape::Velvet, "balls default to velvet");
        assert_eq!(s.colour(), s.colours.stem[s.colours.stem.len() / 2]);
        let rosette = OK
            .replace("form = \"stem\"", "form = \"rosette\"")
            .replace("[\"cuttings\"]", "[\"runners\"]")
            .replace(
                "[stem]\n        height = 20\n        internode = 2\n        leaf_length = 3\n        root_depth = 6",
                "[rosette]\n        crown = 1\n        blades = 3\n        blade_length = 20\n        lean = 5\n        root_depth = 6\n        spacing = 6\n        pad = 2",
            );
        let lotus = format!("{rosette}\n[look]\nleaf = \"lotus\"\n");
        let t = PlantSpeciesTable::from_toml_sources(&[&lotus], &m).unwrap();
        let f = t
            .get(PlantSpeciesId(9))
            .unwrap()
            .form
            .rosette()
            .unwrap()
            .clone();
        assert_eq!((f.pad, f.pad_top()), (2, 23));
        let invalid = |src: String| {
            let r = PlantSpeciesTable::from_toml_sources(&[&src], &m);
            assert!(matches!(r, Err(PlantSpeciesError::Invalid { .. })), "{r:?}");
        };
        // A ball too big or too small, a ball with runners or roots to feed
        // by, velvet on anything but a ball and a mat shape on a ball.
        invalid(ball.replace("radius = 3", "radius = 0"));
        invalid(ball.replace("radius = 3", "radius = 9"));
        invalid(ball.replace("[\"fragments\"]", "[\"runners\"]"));
        invalid(ball.replace("cost = 0.01", "cost = 0.01\n        root_feeder = true"));
        invalid(format!("{OK}\n[look]\nstem = \"velvet\"\n"));
        invalid(format!("{ball}\n[look]\nstem = \"fuzz\"\n"));
        // Pads too wide or too long for the position bits, lotus leaves
        // without pads and pads without lotus leaves.
        invalid(lotus.replace("pad = 2", "pad = 5"));
        invalid(lotus.replace("blade_length = 20", "blade_length = 61"));
        invalid(format!(
            "{}\n[look]\nleaf = \"lotus\"\n",
            rosette.replace("\n        pad = 2", "")
        ));
        invalid(rosette.clone());
        // Deep runners: at most 8 down, and never on a carpet; spacing up to
        // 48, the runner then giving up at the position bits' limit.
        let deep = rosette.replace("spacing = 6", "spacing = 48\n        runner_depth = 8");
        let t = PlantSpeciesTable::from_toml_sources(&[&deep.replace("\n        pad = 2", "")], &m)
            .unwrap();
        let f = t
            .get(PlantSpeciesId(9))
            .unwrap()
            .form
            .rosette()
            .unwrap()
            .clone();
        assert_eq!(
            (f.spacing, f.runner_depth, f.runner_limit()),
            (48, 8, MAX_RUNNER)
        );
        invalid(deep.replace("runner_depth = 8", "runner_depth = 9"));
        invalid(deep.replace("spacing = 48", "spacing = 49"));
        let carpet = deep
            .replace("form = \"rosette\"", "form = \"carpet\"")
            .replace("[rosette]\n        crown = 1", "[carpet]")
            .replace("blade_length = 20", "blade_length = 2")
            .replace("\n        pad = 2", "");
        invalid(carpet.clone());
        PlantSpeciesTable::from_toml_sources(
            &[&carpet.replace("\n        runner_depth = 8", "")],
            &m,
        )
        .unwrap();
        assert!(matches!(
            PlantSpeciesTable::from_toml_sources(&[&ball.replace("[ball]", "[balls]")], &m),
            Err(PlantSpeciesError::Parse { .. })
        ));
    }

    #[test]
    fn variant_bytes_keep_noise_and_position_apart() {
        let v = plant_variant(0b1011_0110, 5);
        assert_eq!(plant_position(v), 5);
        assert_eq!(v >> POSITION_BITS, 0b10);
        assert_eq!(plant_position(plant_variant(0xff, 200)), MAX_POSITION);
    }
}
