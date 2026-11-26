//! Creatures (SPEC §2.4) steer in fixed point, collide with the cell grid
//! and eat within reach of their bodies. Species data selects schooling, shoaling,
//! substrate walking, contour grazing and trips to the air interface.
//!
//! - **Fixed point.** Positions and velocities are integers in 1/256 cell
//!   (velocities per tick), so every platform computes the same motion and
//!   the state hash covers them exactly. Speeds stay under a cell per tick,
//!   so a body never skips past a cell it would collide with.
//! - **Bodies.** A creature covers the cells its sprite overlaps (a box
//!   rounded outwards when it sits between cells). It may only overlap
//!   *swimmable* cells: water (dense liquids), bubbles and passable loose
//!   particles such as food and detritus (material `passable`). Rock,
//!   substrate, bodies and air block it; a plant cell blocks it only
//!   within [`BAND_REACH`] of its depth band (below), and a floater's
//!   hanging roots, soft strands, block only surface crawlers (which climb
//!   them as they would any plant). If the world moves a
//!   solid into a creature (a pour, a growing plant) it shifts to a free
//!   band where it is, or else hops to the nearest free spot.
//! - **Depth** (2.5D, D21). The tank is [`DEPTH_BANDS`] bands deep, 0 at
//!   the front glass. Each plant cell has a band ([`World::plant_band`]);
//!   rigid bodies span a range, and substrate, glass and air span every
//!   band. So do food and other loose particles, which a creature eats
//!   from any band. A creature has a depth in 1/256 band (hashed and
//!   saved) that eases towards a target band: a new random one in its
//!   species' `bands` with each wander target, or, when a banded obstacle blocks its
//!   way, the nearest band it can shift to in place where the way is clear
//!   (front first for even ids, back first for odd), holding still while
//!   it shifts, so it can slip past the obstacle; it
//!   turns away if that takes over [`DODGE_TICKS`]. A schooler that can't
//!   reach its school at all strays on a wander target of its own for a
//!   while, out of whatever pocket holds it. Crawlers keep the band they
//!   start in (unless buried, when any creature may shift band to get
//!   free), so they climb only the plants in it. A corpse sinks in
//!   the band it died in.
//! - **Walkers** keep to the substrate, stepping up one-row ledges; a
//!   species with `hop_every` (shrimp) kicks off it now and then and sinks
//!   back a few cells on. Bottom dwellers that can swim but would rather
//!   not (`swim_every`) take a short swim now and then, and burrowers
//!   (`burrow_every`) dig into loose substrate: see [`bottom`].
//! - **Attachment.** A surface crawler turns in quarter turns so its foot
//!   (the sprite's bottom row) rests on the face it holds: the floor
//!   upright, a wall on its side, a ceiling upside down. Its body box turns
//!   with it, so a 5×3 snail on a wall covers 3×5 cells. The turn is
//!   creature state (hashed); swimmers and walkers stay upright.
//! - **Randomness** comes from a PCG stream per creature per tick, derived
//!   from `(seed, tick, creature id)`, so it never depends on update order.
//!   Each creature's own **quirks** (speed, mood, restlessness, favourite
//!   depth, breathing and hopping rhythm, boldness) are in [`quirks`].
//! - **Cost.** Creatures are updated every tick, after the cells, and wake
//!   a chunk only when they eat (SPEC §2.1). Food is found through a
//!   registry of food cells kept like the living lists, not by scanning.
//! - **Life cycle** (T9): age, stages, growth with feeding, deaths and
//!   corpses are in [`lifecycle`]; sexes, breeding, eggs, colour genetics
//!   and predation in [`breeding`].
//! - **Habits** of a few species (waste, eating corpses, a home with a
//!   carpet patch) are in [`habits`].
//! - **Names** and the inspect card are in [`naming`].
//! - **Schooling** (loose, irregular, splitting and rejoining) is in
//!   [`school`]: a school follows its lead, or a sub-lead while it is
//!   split, and a fright ripples through it from fish to fish. Shoalers
//!   wander near mates of their own stage with no lead, and a fright
//!   doesn't spread through a shoal (fry that passed it on outran every
//!   hunter).
//! - **Hunting, fleeing to cover, startling a school and territory** are in
//!   [`pursuit`].
//! - **Curiosity** draws some fish to the host cursor and nearby pours.
//! - **Live food** (bloodworms) are creatures of a `live_food` species,
//!   kept to [`MAX_LIVE_FOOD`] beside the population cap, eaten whole by
//!   species whose diet holds the food they count as (see [`live`]).

mod bottom;
mod breathing;
mod breeding;
mod catch_up;
mod habits;
mod lifecycle;
mod live;
mod naming;
pub mod night;
mod panes;
pub mod pursuit;
mod quirks;
use quirks::salt;
mod school;
mod steering;

use core::mem;

use rand_core::Rng as _;
use rand_pcg::Pcg32;

use super::plants::PlantDepth;
use super::{TICKS_PER_SECOND, World, WorldError};
use crate::ecology::AlgaeConfig;
use crate::grid::Rect;
use crate::hash::Fnv64;
use crate::material::{MaterialId, MaterialTable, MovementClass};
use crate::plants::Part;
use crate::rng::chunk_stream;
use crate::save::{SaveError, v6, v9};
use crate::species::{RestSpot, Species, SpeciesId, SpeciesTable, Temperament, Zone};
pub use breeding::{
    BREED_CHECK_SECONDS, CONCEIVE_ODDS, Clutch, MAX_CLUTCHES, MUTATION_PER_MILLE, PREY_ENERGY,
    REVERSION_PER_MILLE, SETTLE_TICKS, STRIKE_ODDS,
};
use habits::{home_span, near_home};
pub use lifecycle::{
    BITE_ENERGY, CORPSE_SINK, Corpse, DeathCause, ENERGY_RESERVE, LIFE_DAY_SECONDS, LIFE_DAY_TICKS,
    LifeStage, MAX_CORPSES, POOR_QUALITY, ROT_AFTER_SECONDS, ROT_EVERY_SECONDS, STRESS_LIMIT,
};
pub use live::{LIVE_FOOD_BITES, MAX_LIVE_FOOD, PINCH};
pub use naming::{CreatureCard, MAX_NAME_CHARS, clean_name};
pub use panes::Pane;

/// The default population cap per display (SPEC §2.4), which protects the
/// budget; [`World::set_creature_cap`] changes it.
pub const DEFAULT_CREATURE_CAP: usize = 60;
/// Highest population cap a tank may set (with [`MAX_CORPSES`], within the
/// renderer's sprites with [`MAX_LIVE_FOOD`]).
pub const MAX_CREATURE_CAP: usize = 128;

/// One cell in fixed point.
const ONE: i32 = 256;
/// Satiety a full creature has, in ticks (2 minutes).
#[allow(clippy::cast_possible_truncation)] // 2400
const FULL: u16 = 120 * TICKS_PER_SECOND as u16;
/// Satiety one food cell gives, in ticks (30 seconds). A creature is hungry
/// while another bite fits.
#[allow(clippy::cast_possible_truncation)] // 600
const BITE: u16 = 30 * TICKS_PER_SECOND as u16;
/// A hungry creature looks for closer food every this many ticks.
const SEARCH_EVERY: u64 = 10;
/// Slower surface-leaf searches after giving up on food near the waterline.
const FLOAT_RETRY_EVERY: u64 = 5 * TICKS_PER_SECOND as u64;
/// How far a creature notices food, in cells.
const FOOD_SENSE: i64 = 160;
/// How far from where it is asked for a creature is added, in cells.
const PLACE_RADIUS: i32 = 8;
/// How far a buried creature can hop to clear solid cells. Allow a full
/// maximum sprite side (32 cells) so wide fish beside the glass can clear
/// a column of falling sand. The search runs only while it is buried.
const UNSTUCK_RADIUS: i32 = 32;
/// Ticks a netted creature stays held after the last hold.
const HOLD_TICKS: u64 = 2;
/// Creature random streams: above every chunk id, below the life stream.
const CREATURE_STREAM: u64 = 1 << 48;
/// Distance from the target within which a creature slows down.
const ARRIVE: i64 = 4 * ONE as i64;
/// Distance from a wander target that counts as arrived.
const REACHED: i64 = 2 * ONE as i64;
/// Wander targets last this many ticks at most (a random span from here).
const RETARGET_TICKS: (u32, u32) = (40, 160);
/// Liquids at least this dense are water to a creature (not air).
const WATER_DENSITY: f32 = 0.5;
/// Depth bands, front (0) to back (D21).
pub const DEPTH_BANDS: u8 = 8;
/// A creature collides with a plant cell whose band is at most this far
/// from its own. With 1, a creature and a leaf are each about a band
/// thick: they touch when they share a band or are next to one another,
/// so a plant blocks three of the eight bands at each cell and a fish
/// shifts two bands to slip past a leaf. (With 0, plants hardly block at
/// all; with 2, a stand of stem plants is a wall in every band.)
pub const BAND_REACH: u8 = 1;
/// Food within this many cells (each way) of the cell a creature last
/// gave up on is skipped too, so a fish blocked from a patch of food (a
/// leaf mat behind rock) doesn't flip between neighbouring cells of it.
const IGNORE_REACH: i64 = 2;
/// Depth change per tick, in 1/256 band: a band in 4 ticks (0.2 s).
const DEPTH_SPEED: i32 = ONE / 4;
/// Ticks a creature holds still shifting band past a plant before it gives
/// up and turns away (1 s: time to cross five bands).
#[allow(clippy::cast_possible_truncation)] // 40
const DODGE_TICKS: u16 = TICKS_PER_SECOND as u16;

/// What [`World::try_move`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Move {
    /// It moved (or didn't want to).
    Moved,
    /// It holds still, shifting band to get past a plant.
    Dodging,
    /// It couldn't move.
    Blocked,
}

/// Sideways speed, in 1/256 cell per tick, past which a creature turns to
/// face the way it swims.
const TURN_SPEED: i32 = 12;
/// Ticks a hopping walker kicks upwards for, at its top speed; it then
/// drifts back down at its cruising speed.
const HOP_TICKS: u64 = 8;

/// Where a crawler's foot points for each quarter turn clockwise: down,
/// left, up, right (one clockwise turn maps `(x, y)` to `(-y, x)`, as y
/// grows downwards).
const FEET: [(i32, i32); 4] = [(0, 1), (-1, 0), (0, -1), (1, 0)];

/// A creature handle, unique within a world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CreatureId(pub u32);

/// Why a species doesn't suit the tank as it is (SPEC §2.1: the inspect
/// card and the log warn).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnmetNeed {
    /// The water is shallower than the species needs, in cells.
    Depth {
        /// Depth it needs.
        needs: usize,
        /// Depth the tank has.
        has: usize,
    },
    /// It breathes air but can't reach any: the tank is full to the top,
    /// or floating plants cover the surface with no gap it fits through.
    Air,
}

/// One creature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Creature {
    id: CreatureId,
    species: SpeciesId,
    /// Body centre, 1/256 cells.
    x: i32,
    y: i32,
    /// Depth, 1/256 band from the front (0) to the back.
    z: i32,
    /// Velocity, 1/256 cells per tick.
    vx: i32,
    vy: i32,
    /// Wander target (body centre), 1/256 cells.
    tx: i32,
    ty: i32,
    /// Depth it eases towards, 1/256 band (a whole band).
    tz: i32,
    /// Ticks it has held still shifting band past a plant.
    dodging: u16,
    /// Ticks a schooling creature wanders on its own after its way to the
    /// school was blocked.
    stray: u16,
    /// Ticks until the next wander target.
    retarget: u32,
    /// Ticks of food in the belly.
    satiety: u16,
    /// The food cell it is heading for.
    food: Option<u32>,
    /// A food cell it couldn't reach, skipped by the search.
    ignore: Option<u32>,
    /// Mirrored in its own frame (sprites are drawn facing right).
    facing_left: bool,
    /// Quarter turns clockwise (0..4): which way its foot points, see
    /// [`FEET`]. Only surface crawlers turn.
    turn: u8,
    /// Held in the net up to this tick (0: free).
    held_until: u64,
    /// Next tick when an air-breather must surface.
    next_breath: u64,
    /// Food or grazing cells eaten so far.
    eaten: u32,
    /// Ticks lived (T9).
    age: u32,
    /// Ticks of growth towards maturity: a tick a tick while well fed,
    /// slower or not at all while hungry; at most the species' maturity.
    growth: u32,
    /// Stored energy in ticks, at most [`ENERGY_RESERVE`]; it starves at 0.
    energy: u32,
    /// Stress from poor water, see [`STRESS_LIMIT`].
    stress: u32,
    /// The substrate cell `(x, y)` a home owning species picked as its
    /// home and planted its carpet patch on (see [`habits`]), or the centre
    /// of a territorial species' territory (see [`pursuit`]).
    home: Option<(u32, u32)>,
    /// Its user-given name, cleaned (see [`naming`]); `None` until named.
    name: Option<Box<str>>,
    /// Its sex (T9 part 2).
    female: bool,
    /// Its two colour-morph alleles, indices into its species' morphs.
    alleles: [u8; 2],
    /// Ticks left carrying young (a livebearer; 0: not carrying).
    carrying: u32,
    /// The father's alleles while carrying.
    sire: [u8; 2],
    /// Ticks until it may breed again.
    rest: u32,
    /// The creature it is hunting or driving out of its territory (see
    /// [`pursuit`]).
    quarry: Option<CreatureId>,
    /// Ticks it has chased its quarry; without one, ticks it rests before
    /// the next chase or tail flip.
    pursuit: u16,
    /// Ticks of fright left: it flees to cover or away while above 0.
    fear: u16,
    /// Asleep for the night (see [`night`]).
    asleep: bool,
    /// Ticks it stays up after being woken at night (see [`night`]).
    woken: u16,
    /// The pane it crawls, if any (see [`panes`]).
    pane: Pane,
}

/// What a creature added by a host, a script or a test is like (see
/// [`World::add_creature_with_traits`]); `None` takes the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Traits {
    /// Age in ticks, grown as a well-fed creature would be by then (`None`:
    /// an adult just mature).
    pub age: Option<u32>,
    /// Its sex (`None`: whichever its species has fewer of in the tank, a
    /// male on a tie).
    pub female: Option<bool>,
    /// Its two morph alleles (`None`: two of the morph the palette adds).
    pub alleles: Option<[u8; 2]>,
}

impl Creature {
    /// Its handle.
    #[must_use]
    pub fn id(&self) -> CreatureId {
        self.id
    }

    /// Its species.
    #[must_use]
    pub fn species(&self) -> SpeciesId {
        self.species
    }

    /// Body centre in cells (fractional).
    #[must_use]
    pub fn centre(&self) -> (f32, f32) {
        // Positions are at most 8192 cells: exact in f32.
        #[allow(clippy::cast_precision_loss)]
        let f = |v: i32| v as f32 / ONE as f32;
        (f(self.x), f(self.y))
    }

    /// Depth in bands from the front glass (fractional, 0 to
    /// [`DEPTH_BANDS`] − 1) while it shifts between bands.
    #[must_use]
    pub fn depth(&self) -> f32 {
        // At most 7 × 256: exact in f32.
        #[allow(clippy::cast_precision_loss)]
        let z = self.z as f32 / ONE as f32;
        z
    }

    /// The depth band it collides in: its depth, rounded.
    #[must_use]
    pub fn band(&self) -> u8 {
        band_of(self.z)
    }

    /// The cell its body centre is in.
    #[must_use]
    pub fn cell(&self) -> (i64, i64) {
        (i64::from(self.x >> 8), i64::from(self.y >> 8))
    }

    /// Whether its sprite is mirrored, before [`Creature::turn`] applies
    /// (sprites are drawn facing right). Upright, this means it faces left.
    #[must_use]
    pub fn facing_left(&self) -> bool {
        self.facing_left
    }

    /// Quarter turns clockwise its sprite and body are rotated by, after
    /// mirroring: 0 upright (foot down), 1 foot left, 2 upside down, 3 foot
    /// right. Only surface crawlers turn.
    #[must_use]
    pub fn turn(&self) -> u8 {
        self.turn
    }

    /// Food cells eaten so far.
    #[must_use]
    pub fn eaten(&self) -> u32 {
        self.eaten
    }

    /// Whether it is hungry (another bite would fit).
    #[must_use]
    pub fn hungry(&self) -> bool {
        self.satiety <= FULL - BITE
    }

    /// Ticks it has lived.
    #[must_use]
    pub fn age(&self) -> u32 {
        self.age
    }

    /// Ticks of growth towards maturity (see [`World::life_stage`]).
    #[must_use]
    pub fn growth(&self) -> u32 {
        self.growth
    }

    /// Stored energy in ticks (at most [`ENERGY_RESERVE`]).
    #[must_use]
    pub fn energy(&self) -> u32 {
        self.energy
    }

    /// Stress from poor water so far (it dies at [`STRESS_LIMIT`]).
    #[must_use]
    pub fn stress(&self) -> u32 {
        self.stress
    }

    /// The substrate cell `(x, y)` it picked as its home, if it keeps one
    /// (a species with a `home_carpet`) and has picked it; for a
    /// territorial species, the cell at the centre of its territory.
    #[must_use]
    pub fn home(&self) -> Option<(usize, usize)> {
        self.home.map(|(x, y)| (x as usize, y as usize))
    }

    /// Health from 0 to 1 for the inspect card: the lower of how full its
    /// energy reserve is (counting a quarter full as healthy) and how far
    /// it is from dying of poor water.
    #[must_use]
    pub fn health(&self) -> f32 {
        #[allow(clippy::cast_precision_loss)] // ≤ 576 000
        let fed = (self.energy as f32 * 4.0 / ENERGY_RESERVE as f32).min(1.0);
        #[allow(clippy::cast_precision_loss)]
        let water = 1.0 - (self.stress as f32 / STRESS_LIMIT as f32).min(1.0);
        fed.min(water)
    }

    /// Whether it is female (T9 part 2).
    #[must_use]
    pub fn female(&self) -> bool {
        self.female
    }

    /// Its two colour-morph alleles (indices into its species'
    /// [`crate::Species::morphs`]); [`World::creature_morph`] is the morph
    /// it shows.
    #[must_use]
    pub fn alleles(&self) -> [u8; 2] {
        self.alleles
    }

    /// Ticks left until a carrying female gives birth (0: not carrying).
    #[must_use]
    pub fn carrying(&self) -> u32 {
        self.carrying
    }

    /// Ticks until it may breed again.
    #[must_use]
    pub fn rest(&self) -> u32 {
        self.rest
    }

    /// The creature it is hunting or driving out of its territory, if any.
    #[must_use]
    pub fn quarry(&self) -> Option<CreatureId> {
        self.quarry
    }

    /// Ticks of fright left (0: calm).
    #[must_use]
    pub fn fear(&self) -> u32 {
        u32::from(self.fear)
    }

    /// Mirrors it so its head leads along `(dx, dy)` in its turned frame,
    /// where the sprite's right points along the foot turned anticlockwise.
    /// A move across that axis leaves it as it was.
    fn face_along(&mut self, dx: i32, dy: i32) {
        let (fx, fy) = FEET[usize::from(self.turn)];
        let ahead = dx * fy - dy * fx;
        if ahead != 0 {
            self.facing_left = ahead < 0;
        }
    }
}

/// A species' movement numbers in fixed point.
#[derive(Clone, Copy, Debug)]
#[allow(clippy::struct_excessive_bools)] // independent species traits
struct Kind {
    /// Body size in cells: the base sprite in the cache, the individual's
    /// size after `grown_kind` selects and scales it.
    w: i32,
    h: i32,
    female_size: (i32, i32),
    /// Cruising and top speed, 1/256 cells per tick.
    cruise: i32,
    top: i32,
    /// Most velocity change per tick.
    accel: i32,
    /// Depth band as fractions of the water column.
    depth: [f32; 2],
    /// Front and back depth bands it keeps to (D21), inclusive.
    bands: (u8, u8),
    schooling: bool,
    shoaling: bool,
    shy: bool,
    curious: bool,
    skittish: bool,
    zone: Zone,
    breathes_air: bool,
    /// Stress per second once out of air (0: it doesn't breathe air).
    air_stress: u32,
    /// Cells past the body it eats from.
    reach: usize,
    /// Ticks between a walker's hops (0: it never hops).
    hop_every: u64,
    /// Ticks of growth to maturity, and the species' lifespan in ticks.
    maturity: u32,
    lifespan: u32,
    /// Adult body length by sex, and the longest prey it eats (0: none), in mm.
    length_mm: [u32; 2],
    prey_mm: u32,
    /// How it breeds, if it does.
    breeding: Option<breeding::Breed>,
    /// Its morph count, the wildtype's index and the palette's.
    morphs: u8,
    wild: u8,
    palette_morph: u8,
    edible: [bool; 256],
    /// Ticks between droppings of waste (0: it leaves none).
    waste_every: u64,
    /// It bites corpses within reach too.
    eats_corpses: bool,
    /// It picks a home and plants a carpet there.
    homebody: bool,
    /// Its territory's radius in cells (0: it keeps none).
    territory: i32,
    /// Ticks between a bottom dweller's chances to swim (0: it can't), and
    /// to burrow (0: it doesn't); see [`bottom`].
    swim_every: u64,
    burrow_every: u64,
    /// It is live food (not stock: see [`live`]).
    live: bool,
    /// The material a live food dies into.
    food_as: Option<MaterialId>,
    /// Bits by species id: the live foods it eats (those whose material
    /// is in its diet).
    live_prey: [u64; 4],
    /// It eats some live food.
    eats_live: bool,
    /// Bottom-dwelling shrimp climb surfaces and graze sponge filters (DV4).
    is_bottom_shrimp: bool,
    grazes_algae: bool,
    /// By algae variety id: how well it grazes the variety, in percent of a
    /// full bite (0 ignores it).
    graze: [u8; 16],
    /// Where it rests at night (see [`night`]).
    rests: RestSpot,
    /// Its body on the edges, and on a pane (head right) if it has pane
    /// art (see [`panes`]); a grown kind scales both, and `w`/`h` are
    /// the pane's while it is on one.
    side: (i32, i32),
    pane: Option<(i32, i32)>,
}

impl Kind {
    /// Whether it grazes some algae variety at all.
    fn grazes(&self) -> bool {
        self.graze.iter().any(|&percent| percent != 0)
    }

    /// Adult body size for a sex. Call it on the species' base kind only:
    /// a grown kind (`grown_kind`) scales `w`/`h` but not `female_size`.
    fn size_of(&self, female: bool) -> (i32, i32) {
        if female {
            self.female_size
        } else {
            (self.w, self.h)
        }
    }
}

/// The creatures in a world and the per-species and per-material lookups
/// their update uses.
#[derive(Clone, Debug)]
pub(super) struct Creatures {
    species: SpeciesTable,
    list: Vec<Creature>,
    /// Cell-space input from a host with avoidance enabled.
    cursor: Option<(i32, i32)>,
    /// Local time of day from the host, minutes after midnight (see
    /// [`night`]).
    minute: u16,
    /// Reused start-of-tick snapshots by species, each in creature id order.
    school: Vec<Vec<Schoolmate>>,
    /// Whether a schooler is frightened enough this tick to startle its
    /// mates (see [`pursuit`]); calm tanks skip the look.
    scared: bool,
    /// Reused start-of-tick snapshot for hunting and territory, in id
    /// order (empty while no creature hunts or keeps a territory).
    near: Vec<pursuit::Near>,
    /// Reused start-of-tick list of open water at the surface (empty
    /// while no air-breather is searching for air).
    gaps: Vec<breathing::Gap>,
    next_id: u32,
    kinds: Vec<Option<Kind>>,
    /// Per material: a creature's body may overlap it.
    swim: Vec<bool>,
    /// Per material: loose substrate a burrower digs into.
    loose: Vec<bool>,
    water: MaterialId,
    air: MaterialId,
    detritus: Option<MaterialId>,
    waste: Option<MaterialId>,
    /// Bumped whenever a creature or corpse is added, removed, moves or
    /// turns, or its drawn size or look changes.
    version: u64,
    /// The population cap.
    cap: usize,
    /// Easy mode: only old age kills.
    easy: bool,
    /// Dead creatures sinking and rotting, oldest first.
    corpses: Vec<Corpse>,
    /// Egg clutches waiting to hatch, oldest first.
    clutches: Vec<Clutch>,
    /// The next clutch id to try (never 0).
    next_clutch: u16,
    egg: Option<MaterialId>,
}

#[derive(Clone, Copy, Debug)]
struct Schoolmate {
    id: CreatureId,
    x: i32,
    y: i32,
    vx: i32,
    vy: i32,
    /// Ticks of fright it had at the start of the tick.
    fear: u16,
    /// The one its group follows: its species' lead, or a sub-lead while
    /// the school is split (see [`school`]).
    head: CreatureId,
    /// Grown to maturity: a shoaler keeps with mates of its own stage.
    adult: bool,
}

impl Creatures {
    #[allow(clippy::too_many_lines)] // one field per piece of tank state
    pub(super) fn new(
        species: SpeciesTable,
        materials: &MaterialTable,
        algae: Option<&AlgaeConfig>,
        water: MaterialId,
        air: MaterialId,
    ) -> Self {
        let swim = per_material(materials, |m| match m.class {
            MovementClass::Liquid => m.density >= WATER_DENSITY,
            MovementClass::Buoyant => true,
            _ => m.passable,
        });
        let loose = per_material(materials, |m| bottom::is_loose(m.class, m.palette));
        let mut kinds = vec![None; 256];
        let live: Vec<_> = species
            .iter()
            .filter_map(|l| l.live_food.then_some((l.id, l.food_as?)))
            .collect();
        for s in species.iter() {
            let mut live_prey = [0_u64; 4];
            for &(id, food) in &live {
                if s.diet.contains(&food) {
                    live_prey[usize::from(id.0 >> 6)] |= 1 << (id.0 & 63);
                }
            }
            // Sprite sides are at most 32 cells.
            let size = |female| {
                let (w, h) = s.size_of(female);
                (i32::try_from(w).unwrap_or(1), i32::try_from(h).unwrap_or(1))
            };
            let (w, h) = size(false);
            let mut edible = [false; 256];
            for m in &s.diet {
                edible[usize::from(m.0)] = true;
            }
            let (cruise, top) = (speed_per_tick(s.speed), speed_per_tick(s.max_speed));
            kinds[usize::from(s.id.0)] = Some(Kind {
                w,
                h,
                female_size: size(true),
                cruise,
                top,
                accel: (top / 6).max(2),
                depth: s.depth,
                bands: (s.bands[0], s.bands[1]),
                schooling: s.temperament == Temperament::Schooling,
                shoaling: s.temperament == Temperament::Shoaling,
                shy: s.shy,
                curious: s.curious,
                skittish: s.skittish,
                zone: s.zone,
                breathes_air: s.breathes_air,
                air_stress: breathing::air_stress(s.air_tolerance_seconds),
                reach: usize::from(s.reach),
                // Validated to 1..=600 s.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                hop_every: s.hop_every.map_or(0, |every| {
                    (f64::from(every) * f64::from(TICKS_PER_SECOND)).round() as u64
                }),
                // Validated to at most 7300 days: far inside u32.
                maturity: s.maturity_days.saturating_mul(LIFE_DAY_TICKS),
                lifespan: s.lifespan_days.saturating_mul(LIFE_DAY_TICKS),
                // Validated lengths fit the sprite: at most 16 cm.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                length_mm: [false, true]
                    .map(|female| (s.length_cm_of(female) * 10.0).round() as u32),
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                prey_mm: s.prey_cm.map_or(0, |p| (p * 10.0).round() as u32),
                breeding: s.breeding.as_ref().map(breeding::Breed::of),
                // At most MAX_MORPHS.
                morphs: u8::try_from(s.morphs.len()).unwrap_or(1),
                wild: s.wildtype,
                palette_morph: s.palette_morph,
                edible,
                // Validated to 1..=600 s.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                waste_every: s.waste_every.map_or(0, |every| {
                    (f64::from(every) * f64::from(TICKS_PER_SECOND)).round() as u64
                }),
                eats_corpses: s.eats_corpses,
                homebody: s.home_carpet.is_some(),
                // Validated to 2..=50 cm.
                #[allow(clippy::cast_possible_truncation)]
                territory: s
                    .territory_cm
                    .map_or(0, |t| (t * crate::species::CELLS_PER_CM).round() as i32),
                swim_every: every_ticks(s.swim_every),
                burrow_every: every_ticks(s.burrow_every),
                live: s.live_food,
                food_as: s.food_as,
                live_prey,
                eats_live: live_prey != [0; 4],
                grazes_algae: s.grazes_algae,
                graze: grazing(s, algae),
                is_bottom_shrimp: s.zone == Zone::Bottom && s.name.ends_with("_shrimp"),
                rests: s.rests,
                side: (w, h),
                #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                pane: s.pane_size().map(|(w, h)| (w as i32, h as i32)),
            });
        }
        let school = vec![
            Vec::new();
            species
                .iter()
                .map(|s| usize::from(s.id.0) + 1)
                .max()
                .unwrap_or(0)
        ];
        Self {
            species,
            list: Vec::new(),
            cursor: None,
            minute: crate::daylight::NOON,
            school,
            scared: false,
            gaps: Vec::new(),
            near: Vec::new(),
            next_id: 1,
            kinds,
            swim,
            loose,
            water,
            air,
            detritus: materials.id("detritus"),
            waste: materials.id("waste"),
            version: 0,
            cap: DEFAULT_CREATURE_CAP,
            easy: false,
            corpses: Vec::new(),
            clutches: Vec::new(),
            next_clutch: 1,
            egg: materials.id("egg"),
        }
    }

    /// The built-in species, and the materials any of them eats.
    pub(super) fn builtin(
        materials: &MaterialTable,
        algae: Option<&AlgaeConfig>,
        water: MaterialId,
        air: MaterialId,
    ) -> Result<(Self, Vec<MaterialId>), WorldError> {
        let species =
            SpeciesTable::builtin_for(materials).map_err(|e| WorldError::Species(e.to_string()))?;
        if let Some(cfg) = algae {
            species
                .check_grazing(|name| cfg.by_name(name).is_some())
                .map_err(|e| WorldError::Species(e.to_string()))?;
        }
        let foods = Self::foods(&species);
        Ok((Self::new(species, materials, algae, water, air), foods))
    }

    /// The materials any species eats.
    pub(super) fn foods(species: &SpeciesTable) -> Vec<MaterialId> {
        let mut foods: Vec<MaterialId> = species.iter().flat_map(|s| s.diet.clone()).collect();
        foods.sort_unstable();
        foods.dedup();
        foods
    }
}

/// Density levels scraped off the film by one full bite.
const FILM_BITE: u8 = 4;

/// How well species `s` grazes each algae variety, in percent of a full
/// bite: its own `grazing` entry, else what the variety's resistance
/// leaves. A species that doesn't graze algae ignores all of it.
fn grazing(s: &Species, algae: Option<&AlgaeConfig>) -> [u8; 16] {
    let mut graze = [0; 16];
    if let Some(cfg) = algae.filter(|_| s.grazes_algae) {
        for v in &cfg.varieties {
            graze[usize::from(v.id)] = s
                .grazing
                .iter()
                .find(|(name, _)| *name == v.name)
                .map_or(100 - v.resistance, |&(_, percent)| percent);
        }
    }
    graze
}

/// Density levels one bite of `percent` of a full bite scrapes off: at least
/// one.
fn bite_levels(percent: u8) -> u8 {
    u8::try_from((u16::from(FILM_BITE) * u16::from(percent) + 50) / 100)
        .unwrap_or(FILM_BITE)
        .max(1)
}

fn nibble_film(coat: u8, percent: u8) -> u8 {
    use super::algae::DENSITY_MASK;
    let left = (coat & DENSITY_MASK).saturating_sub(bite_levels(percent));
    if left == 0 {
        0
    } else {
        (coat & !DENSITY_MASK) | left
    }
}

impl World {
    /// Sets the pointer cell fish react to. A missing or out-of-tank pointer
    /// disables the reaction. Hosts supply this input only when enabled.
    pub fn set_creature_cursor(&mut self, cell: Option<(i64, i64)>) {
        self.creatures.cursor = cell
            .filter(|&(x, y)| {
                (0..to_i64(self.width)).contains(&x) && (0..to_i64(self.height)).contains(&y)
            })
            .map(|(x, y)| (centre_of(x), centre_of(y)));
    }

    /// The species table.
    #[must_use]
    pub fn species(&self) -> &SpeciesTable {
        &self.creatures.species
    }

    /// Every creature, in id order.
    #[must_use]
    pub fn creatures(&self) -> &[Creature] {
        &self.creatures.list
    }

    /// The creature with this id, if it is in the tank.
    #[must_use]
    pub fn creature(&self, id: CreatureId) -> Option<&Creature> {
        self.creatures
            .list
            .binary_search_by_key(&id, |c| c.id)
            .ok()
            .map(|i| &self.creatures.list[i])
    }

    /// Bumped when a creature or corpse changes in a way the renderer draws.
    #[must_use]
    pub fn creatures_version(&self) -> u64 {
        self.creatures.version
    }

    /// The cells `creature`'s body covers.
    #[must_use]
    pub fn creature_body(&self, creature: &Creature) -> Rect {
        self.grown_kind(creature)
            .and_then(|k| {
                body(
                    &k,
                    creature.turn,
                    creature.x,
                    creature.y,
                    self.width,
                    self.height,
                )
            })
            .unwrap_or(Rect::EMPTY)
    }

    /// The topmost (newest) creature whose body covers cell `(x, y)`.
    #[must_use]
    pub fn creature_at(&self, x: i64, y: i64) -> Option<CreatureId> {
        let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y)) else {
            return None;
        };
        self.creatures
            .list
            .iter()
            .rev()
            .find(|c| self.creature_body(c).contains(x, y))
            .map(|c| c.id)
    }

    /// Adds an adult of `species`, just mature and well fed, with its body
    /// centred on cell `(x, y)`, or on the nearest free spot within a few
    /// cells. It takes whichever sex its species has fewer of in the tank
    /// (a male on a tie), shows the morph the palette adds, and settles in
    /// for [`SETTLE_TICKS`] before it breeds.
    ///
    /// # Errors
    ///
    /// Returns an error if the species is unknown, the tank is at its
    /// [population cap](World::creature_cap), or there is no room there.
    pub fn add_creature(
        &mut self,
        species: SpeciesId,
        x: i64,
        y: i64,
    ) -> Result<CreatureId, WorldError> {
        self.add_creature_with_traits(species, x, y, Traits::default())
    }

    pub(crate) fn add_palette_creature(
        &mut self,
        species: SpeciesId,
        x: i64,
        y: i64,
        morph: Option<u8>,
    ) -> Result<CreatureId, WorldError> {
        if self.is_live(species) {
            return self.add_pinch(species, x, y);
        }
        let morph = morph.or_else(|| {
            self.species()
                .get(species)
                .filter(|s| s.cycle_palette)
                .map(|s| {
                    let count = self
                        .creatures()
                        .iter()
                        .filter(|c| c.species == species)
                        .count();
                    u8::try_from(count % s.morphs.len()).unwrap_or(s.palette_morph)
                })
        });
        let traits = Traits {
            alleles: morph.map(|m| [m; 2]),
            ..Traits::default()
        };
        self.add_creature_with_traits(species, x, y, traits)
    }

    /// [`World::add_creature`] with the age, sex or morph alleles given.
    ///
    /// # Errors
    ///
    /// As [`World::add_creature`], or if an allele isn't one of the
    /// species' morphs.
    pub fn add_creature_with_traits(
        &mut self,
        species: SpeciesId,
        x: i64,
        y: i64,
        traits: Traits,
    ) -> Result<CreatureId, WorldError> {
        let k = *self
            .kind(species)
            .ok_or(WorldError::UnknownSpecies(species))?;
        let alleles = traits.alleles.unwrap_or([k.palette_morph; 2]);
        if let Some(&bad) = alleles.iter().find(|&&a| a >= k.morphs) {
            return Err(WorldError::UnknownMorph(species, bad));
        }
        let female = traits.female.unwrap_or_else(|| self.default_sex(species));
        let age = traits.age.unwrap_or(k.maturity);
        self.spawn(species, x, y, age, female, alleles, SETTLE_TICKS)
    }

    /// Adds a creature `age` ticks old, grown as a well-fed one would be,
    /// resting `rest` ticks before it breeds.
    #[allow(clippy::too_many_arguments)]
    fn spawn(
        &mut self,
        species: SpeciesId,
        x: i64,
        y: i64,
        age: u32,
        female: bool,
        alleles: [u8; 2],
        rest: u32,
    ) -> Result<CreatureId, WorldError> {
        let base = *self
            .kind(species)
            .ok_or(WorldError::UnknownSpecies(species))?;
        if base.live {
            if self.live_food_count() >= MAX_LIVE_FOOD {
                return Err(WorldError::PopulationCap(MAX_LIVE_FOOD));
            }
        } else if self.stock_count() >= self.creatures.cap {
            return Err(WorldError::PopulationCap(self.creatures.cap));
        }
        let growth = age.min(base.maturity);
        let mut probe = Creature {
            id: CreatureId(self.creatures.next_id),
            species,
            x: 0,
            y: 0,
            z: 0,
            vx: 0,
            vy: 0,
            tx: 0,
            ty: 0,
            tz: 0,
            dodging: 0,
            stray: 0,
            retarget: 0,
            satiety: 0,
            food: None,
            ignore: None,
            facing_left: self.creatures.next_id % 2 == 1,
            turn: 0,
            held_until: 0,
            next_breath: self.tick + u64::from(self.creatures.next_id % 20),
            eaten: 0,
            age,
            growth,
            energy: ENERGY_RESERVE,
            stress: 0,
            female,
            alleles,
            carrying: 0,
            sire: [0; 2],
            rest,
            home: None,
            name: None,
            quarry: None,
            pursuit: 0,
            fear: 0,
            asleep: false,
            woken: 0,
            pane: Pane::Off,
        };
        let k = self.grown_kind(&probe).unwrap_or(base);
        let (fx, fy) = (centre_of(x), centre_of(y));
        let band = default_band(self.seed, probe.id.0, k.bands);
        let (fx, fy, band) = if self.fits(&k, 0, fx, fy, band) {
            (fx, fy, band)
        } else if let Some(b) = self.free_band(&k, 0, fx, fy, band) {
            (fx, fy, b)
        } else {
            let (x, y) = self
                .nearest_fit(&k, 0, fx, fy, band, PLACE_RADIUS)
                .ok_or(WorldError::NoRoom)?;
            (x, y, band)
        };
        let id = probe.id;
        self.creatures.next_id += 1;
        (probe.x, probe.y, probe.tx, probe.ty) = (fx, fy, fx, fy);
        (probe.z, probe.tz) = (z_of(band), z_of(band));
        self.creatures.list.push(probe);
        self.creatures.version += 1;
        Ok(id)
    }

    /// Takes a creature out of the tank. Returns whether it was there.
    pub fn remove_creature(&mut self, id: CreatureId) -> bool {
        let before = self.creatures.list.len();
        self.creatures.list.retain(|c| c.id != id);
        let removed = self.creatures.list.len() != before;
        if removed {
            self.creatures.version += 1;
        }
        removed
    }

    /// Holds a creature still (in the net) until two ticks from now; call
    /// again each tick to keep holding it. Returns whether it exists.
    pub fn hold_creature(&mut self, id: CreatureId) -> bool {
        let until = self.tick + HOLD_TICKS;
        self.creature_mut(id)
            .map(|c| {
                c.held_until = until;
                // The net lifts it off a pane, upright (see `panes`).
                if c.pane != Pane::Off {
                    (c.pane, c.turn) = (Pane::Off, 0);
                }
                // Netting wakes a sleeper (see `night`).
                if c.asleep {
                    c.wake();
                    c.woken = night::WOKEN_TICKS;
                }
            })
            .is_some()
    }

    /// Lets a held creature go: it swims off from where it is.
    pub fn release_creature(&mut self, id: CreatureId) {
        if let Some(c) = self.creature_mut(id) {
            c.held_until = 0;
            c.retarget = 0;
            c.food = None;
        }
    }

    /// Moves a creature's body centre to cell `(x, y)` if it fits there
    /// (the net carrying it). Returns whether it moved.
    pub fn move_creature(&mut self, id: CreatureId, x: i64, y: i64) -> bool {
        let Some(c) = self.creature(id) else {
            return false;
        };
        let Some(k) = self.grown_kind(c) else {
            return false;
        };
        let (fx, fy) = (centre_of(x), centre_of(y));
        if (fx, fy) == (c.x, c.y) || !self.fits(&k, c.turn, fx, fy, c.band()) {
            return false;
        }
        let tick = self.tick;
        if let Some(c) = self.creature_mut(id) {
            c.face_along(fx - c.x, fy - c.y);
            (c.x, c.y, c.vx, c.vy) = (fx, fy, 0, 0);
            c.held_until = c.held_until.max(tick + HOLD_TICKS);
        }
        self.creatures.version += 1;
        true
    }

    /// The rows (half-open, from the top) of `species`' depth zone in this
    /// tank: its `depth` band of the water column.
    #[must_use]
    pub fn zone_rows(&self, species: SpeciesId) -> Option<(usize, usize)> {
        let k = self.kind(species)?;
        Some(zone(k, self.surface_row, self.height))
    }

    /// Whether the creature is in its depth band and, for a surface grazer,
    /// touching a submerged support.
    #[must_use]
    pub fn in_zone(&self, creature: &Creature) -> bool {
        if let Some(k) = self.grown_kind(creature)
            && k.zone == Zone::Surfaces
            && creature.pane == Pane::Off
            && !self.on_surface(&k, creature.turn, creature.x, creature.y, creature.band())
        {
            return false;
        }
        self.zone_rows(creature.species)
            .is_some_and(|(top, bottom)| {
                let row = usize::try_from(creature.y >> 8).unwrap_or(usize::MAX);
                (top..bottom).contains(&row)
            })
    }

    /// What `species` needs that this tank lacks, if anything (SPEC §2.1).
    #[must_use]
    pub fn unmet_need(&self, species: SpeciesId) -> Option<UnmetNeed> {
        let s = self.creatures.species.get(species)?;
        let has = self.height - self.surface_row;
        if has < s.min_depth {
            return Some(UnmetNeed::Depth {
                needs: s.min_depth,
                has,
            });
        }
        if !s.breathes_air {
            return None;
        }
        let mut gaps = Vec::new();
        self.surface_gaps(&mut gaps);
        // This describes the whole species, including the wider sex.
        let wide = s.size_of(false).0.max(s.size_of(true).0);
        (!gaps.iter().any(|g| g.width() >= wide)).then_some(UnmetNeed::Air)
    }

    /// The creatures, corpses and life settings as saved (T8, T9). The
    /// host's cursor isn't saved.
    pub(super) fn save_creatures(&self) -> v9::Creatures {
        let cr = &self.creatures;
        v9::Creatures {
            next_id: cr.next_id,
            cap: u32::try_from(cr.cap).unwrap_or(u32::MAX),
            easy: cr.easy,
            list: cr
                .list
                .iter()
                .map(|c| v9::Creature {
                    base: v6::Creature {
                        id: c.id.0,
                        species: c.species.0,
                        x: c.x,
                        y: c.y,
                        z: c.z,
                        tz: c.tz,
                        dodging: c.dodging,
                        stray: c.stray,
                        vx: c.vx,
                        vy: c.vy,
                        tx: c.tx,
                        ty: c.ty,
                        retarget: c.retarget,
                        satiety: c.satiety,
                        food: c.food,
                        ignore: c.ignore,
                        facing_left: c.facing_left,
                        turn: c.turn,
                        held_until: c.held_until,
                        next_breath: c.next_breath,
                        eaten: c.eaten,
                        age: c.age,
                        growth: c.growth,
                        energy: c.energy,
                        stress: c.stress,
                        female: c.female,
                        alleles: c.alleles,
                        carrying: c.carrying,
                        sire: c.sire,
                        rest: c.rest,
                        home: c.home,
                        name: c.name.as_deref().unwrap_or_default().to_owned(),
                    },
                    quarry: c.quarry.map(|q| q.0),
                    pursuit: c.pursuit,
                    fear: c.fear,
                })
                .collect(),
            corpses: cr
                .corpses
                .iter()
                .map(|c| v6::Corpse {
                    species: c.species.0,
                    cause: c.cause.code(),
                    x: c.x,
                    y: c.y,
                    w: c.w,
                    h: c.h,
                    facing_left: c.facing_left,
                    band: c.band,
                    died: c.died,
                    released: c.released,
                    total: c.total,
                    morph: c.morph,
                    female: c.female,
                })
                .collect(),
            next_clutch: cr.next_clutch,
            clutches: cr
                .clutches
                .iter()
                .map(|c| v6::Clutch {
                    id: c.id,
                    species: c.species.0,
                    mother: c.mother,
                    sire: c.sire,
                    hatch: c.hatch,
                    eggs: c.eggs.clone(),
                })
                .collect(),
        }
    }

    /// Restores the creatures, corpses and life settings from a save.
    #[allow(clippy::too_many_lines)] // one check per property
    pub(super) fn load_creatures(&mut self, saved: v9::Creatures) -> Result<(), SaveError> {
        let (w, h) = (
            to_i64(self.width) * i64::from(ONE),
            to_i64(self.height) * i64::from(ONE),
        );
        let inside =
            |x: i32, y: i32| (0..=w).contains(&i64::from(x)) && (0..=h).contains(&i64::from(y));
        let cap = saved.cap as usize;
        let live = saved
            .list
            .iter()
            .filter(|c| self.kind(SpeciesId(c.base.species)).is_some_and(|k| k.live))
            .count();
        if !(1..=MAX_CREATURE_CAP).contains(&cap)
            || saved.list.len() - live > cap
            || live > MAX_LIVE_FOOD
        {
            return Err(SaveError::Invalid(format!(
                "{} creatures ({live} live food) with a cap of {cap}",
                saved.list.len()
            )));
        }
        let ids: Vec<u32> = saved.list.iter().map(|c| c.base.id).collect();
        let mut list = Vec::with_capacity(saved.list.len());
        for v9::Creature {
            base: c,
            quarry,
            pursuit,
            fear,
        } in saved.list
        {
            let invalid = |what: &str| SaveError::Invalid(format!("creature {}: {what}", c.id));
            let species = SpeciesId(c.species);
            let Some(k) = self.kind(species) else {
                return Err(invalid("unknown species"));
            };
            let slow = |v: i32| v.abs() <= 16 * ONE;
            if !(inside(c.x, c.y) && inside(c.tx, c.ty) && slow(c.vx) && slow(c.vy) && c.turn < 4) {
                return Err(invalid("position, speed or turn out of range"));
            }
            if c.growth > k.maturity || c.energy > ENERGY_RESERVE {
                return Err(invalid("growth or energy out of range"));
            }
            // Older saves can hold a stray for a species that no longer schools.
            let stray = if k.schooling { c.stray } else { 0 };
            // A depth outside its species' bands came from older species
            // data (shy fish moved to the back, say): bring it into range.
            let (front, back) = (z_of(k.bands.0), z_of(k.bands.1));
            let deep = |z: i32| (0..=z_of(DEPTH_BANDS - 1)).contains(&z);
            if !(deep(c.z)
                && deep(c.tz)
                && c.tz % ONE == 0
                && c.dodging <= DODGE_TICKS
                && u32::from(stray) < RETARGET_TICKS.1)
            {
                return Err(invalid("depth out of range"));
            }
            let carry_limit = k
                .breeding
                .filter(|b| !b.eggs && c.female)
                .map_or(0, |b| b.days);
            if c.alleles.iter().chain(&c.sire).any(|&a| a >= k.morphs) || c.carrying > carry_limit {
                return Err(invalid("morph or pregnancy out of range"));
            }
            for i in [c.food, c.ignore].into_iter().flatten() {
                self.cell_index(i, "a creature's food")?;
            }
            // Names as `set_creature_name` keeps them.
            if !self.home_fits(k, c.home) || clean_name(&c.name) != c.name {
                return Err(invalid("home out of place, or a name not as kept"));
            }
            if !pursuit::fits(c.id, quarry, pursuit, fear, &ids) {
                return Err(invalid("quarry, pursuit or fright out of range"));
            }
            list.push(Creature {
                id: CreatureId(c.id),
                species,
                x: c.x,
                y: c.y,
                z: c.z.clamp(front, back),
                vx: c.vx,
                vy: c.vy,
                tx: c.tx,
                ty: c.ty,
                tz: c.tz.clamp(front, back),
                dodging: c.dodging,
                stray,
                retarget: c.retarget,
                satiety: c.satiety,
                food: c.food,
                ignore: c.ignore,
                facing_left: c.facing_left,
                turn: c.turn,
                held_until: c.held_until,
                next_breath: c.next_breath,
                eaten: c.eaten,
                age: c.age,
                growth: c.growth,
                energy: c.energy,
                stress: c.stress,
                female: c.female,
                alleles: c.alleles,
                carrying: c.carrying,
                sire: c.sire,
                rest: c.rest,
                home: c.home,
                name: (!c.name.is_empty()).then(|| c.name.into_boxed_str()),
                quarry: quarry.map(CreatureId),
                pursuit,
                fear,
                asleep: false,
                woken: 0,
                pane: Pane::Off,
            });
        }
        let corpses = self.load_corpses(saved.corpses)?;
        let clutches = self.load_clutches(saved.clutches)?;
        self.creatures.list = list;
        self.creatures.clutches = clutches;
        self.creatures.next_clutch = saved.next_clutch.max(1);
        self.creatures.corpses = corpses;
        self.creatures.cap = cap;
        self.creatures.easy = saved.easy;
        self.creatures.next_id = saved.next_id.max(1);
        self.creatures.version += 1;
        Ok(())
    }

    /// The corpses from a save, as `corpse_of` makes them.
    fn load_corpses(&self, saved: Vec<v6::Corpse>) -> Result<Vec<Corpse>, SaveError> {
        let (w, h) = (
            to_i64(self.width) * i64::from(ONE),
            to_i64(self.height) * i64::from(ONE),
        );
        let inside =
            |x: i32, y: i32| (0..=w).contains(&i64::from(x)) && (0..=h).contains(&i64::from(y));
        if saved.len() > MAX_CORPSES {
            return Err(SaveError::Invalid(format!("{} corpses", saved.len())));
        }
        let mut corpses = Vec::with_capacity(saved.len());
        for c in saved {
            let invalid = |what: &str| SaveError::Invalid(format!("corpse: {what}"));
            let species = SpeciesId(c.species);
            let Some(k) = self.kind(species) else {
                return Err(invalid("unknown species"));
            };
            let cause = DeathCause::from_code(c.cause).ok_or_else(|| invalid("unknown cause"))?;
            // Keep the size at death, including females saved when they
            // still shared the male's dimensions.
            let (full_w, full_h) = k.size_of(c.female);
            let fits = |side: u8, full: i32| (1..=full).contains(&i32::from(side));
            let size_fits = (fits(c.w, full_w) && fits(c.h, full_h))
                || (c.female && fits(c.w, k.w) && fits(c.h, k.h));
            let total = (u16::from(c.w) * u16::from(c.h)).div_ceil(3).max(1);
            if !inside(c.x, c.y)
                || !size_fits
                || c.total != total
                || c.released >= c.total
                || c.band >= DEPTH_BANDS
                || c.morph >= k.morphs
            {
                return Err(invalid("position, size or decay out of range"));
            }
            corpses.push(Corpse {
                species,
                cause,
                x: c.x,
                y: c.y,
                w: c.w,
                h: c.h,
                facing_left: c.facing_left,
                band: c.band,
                died: c.died,
                released: c.released,
                total: c.total,
                morph: c.morph,
                female: c.female,
            });
        }
        Ok(corpses)
    }

    pub(super) fn hash_creatures(&self, h: &mut Fnv64) {
        h.write(&(self.creatures.list.len() as u64).to_le_bytes());
        for c in &self.creatures.list {
            h.write(&c.id.0.to_le_bytes());
            h.write(&[c.species.0, u8::from(c.facing_left), c.turn]);
            for v in [c.x, c.y, c.z, c.vx, c.vy, c.tx, c.ty, c.tz] {
                h.write(&v.to_le_bytes());
            }
            h.write(&c.retarget.to_le_bytes());
            h.write(&c.satiety.to_le_bytes());
            h.write(&c.dodging.to_le_bytes());
            h.write(&c.stray.to_le_bytes());
            for v in [c.food, c.ignore] {
                h.write(&v.map_or(u64::MAX, u64::from).to_le_bytes());
            }
            h.write(&c.held_until.to_le_bytes());
            h.write(&c.next_breath.to_le_bytes());
            h.write(&c.eaten.to_le_bytes());
            for v in [c.age, c.growth, c.energy, c.stress, c.carrying, c.rest] {
                h.write(&v.to_le_bytes());
            }
            h.write(&[
                u8::from(c.female),
                c.alleles[0],
                c.alleles[1],
                c.sire[0],
                c.sire[1],
            ]);
            // Only pane crawlers hash their pane, so other tanks hash as
            // before.
            if c.pane != Pane::Off {
                h.write(&[b'p', c.pane.code()]);
            }
            // Only home owners have one, so other tanks hash as before.
            if let Some((x, y)) = c.home {
                h.write(&x.to_le_bytes());
                h.write(&y.to_le_bytes());
            }
            // Only named creatures hash their name, as with homes.
            if let Some(name) = &c.name {
                h.write(&(name.len() as u64).to_le_bytes());
                h.write(name.as_bytes());
            }
            // Only hunters, territory keepers and frightened creatures hash
            // their pursuit, so calm tanks hash as before version 9.
            if c.quarry.is_some() || c.pursuit > 0 || c.fear > 0 {
                h.write(&[0x70]);
                h.write(&c.quarry.map_or(u64::MAX, |q| u64::from(q.0)).to_le_bytes());
                h.write(&c.pursuit.to_le_bytes());
                h.write(&c.fear.to_le_bytes());
            }
            // Only sleeping or woken creatures hash their rest, so a tank by
            // day hashes as before version 15.
            if c.asleep || c.woken > 0 {
                h.write(&[0x73, u8::from(c.asleep)]);
                h.write(&c.woken.to_le_bytes());
            }
        }
        self.hash_corpses(h);
        self.hash_breeding(h);
    }

    /// Runs every creature for one tick, after the cells and life: each
    /// lives a tick (and may die, leaving a corpse), then moves; then the
    /// corpses sink and rot.
    pub(super) fn update_creatures(&mut self) {
        if self.creatures.list.is_empty()
            && self.creatures.corpses.is_empty()
            && self.creatures.clutches.is_empty()
        {
            return;
        }
        // Water quality matters once a second, and not in easy mode.
        let quality = (!self.creatures.easy
            && !self.creatures.list.is_empty()
            && self.tick.is_multiple_of(u64::from(TICKS_PER_SECOND)))
        .then(|| self.water_quality());
        let mut list = mem::take(&mut self.creatures.list);
        let mut changed = false;
        let mut dead = Vec::new();
        let mut breeding = self.breed_tick(&list);
        let mut died_live = Vec::new();
        list.retain_mut(|c| {
            if let Some(cause) = self.live(c, quality, &mut changed) {
                match self.kind(c.species) {
                    // Live food dies into a cell of its food, no corpse.
                    Some(k) if k.live => died_live.push((self.creature_body(c), k.food_as)),
                    _ => dead.push(self.corpse_of(c, cause)),
                }
                return false;
            }
            self.breed_over(c, &mut breeding, 1, &mut changed);
            true
        });
        if !dead.is_empty() {
            self.bury(dead);
            changed = true;
        }
        if !died_live.is_empty() {
            self.drop_live_food(&died_live);
            changed = true;
        }
        // Live food is eaten once a second, in easy mode too: it is food.
        if self.tick.is_multiple_of(u64::from(TICKS_PER_SECOND)) {
            changed |= self.eat_live_food(&mut list);
        }
        // Predators hunt once a second, with the water-quality sample.
        if quality.is_some() {
            changed |= self.predation(&mut list);
        }
        self.snapshot_near(&list);
        self.refresh_air_gaps(&list);
        for mates in &mut self.creatures.school {
            mates.clear();
        }
        let mut scared = false;
        for c in &list {
            // A sleeper doesn't join the school or shoal (see `night`); only
            // schoolers startle each other.
            let Some(k) = self.kind(c.species) else {
                continue;
            };
            if (k.schooling || k.shoaling) && c.held_until < self.tick && !c.asleep {
                let adult = c.growth >= k.maturity;
                scared |= k.schooling && c.fear > pursuit::STARTLE_HOP;
                self.creatures.school[usize::from(c.species.0)].push(Schoolmate {
                    id: c.id,
                    x: c.x,
                    y: c.y,
                    vx: c.vx,
                    vy: c.vy,
                    fear: c.fear,
                    head: c.id,
                    adult,
                });
            }
        }
        self.creatures.scared = scared;
        for (mates, kind) in self.creatures.school.iter_mut().zip(&self.creatures.kinds) {
            if !mates.is_empty() && kind.as_ref().is_some_and(|k| k.schooling) {
                school::divide(mates, self.seed, self.tick);
            }
        }
        for c in &mut list {
            let z = c.z;
            // A shift in depth alone changes how it is drawn (D21).
            changed |= self.update_creature(c) || c.z != z;
        }
        changed |= self.intake_suction(&mut list);
        self.creatures.list = list;
        changed |= self.finish_breeding(breeding);
        if !self.creatures.corpses.is_empty() {
            changed |= self.update_corpses(self.creatures.detritus);
        }
        if changed {
            self.creatures.version += 1;
        }
    }

    /// Unguarded filter intakes suck in fry and small shrimp that reach
    /// adjacent to an intake cell (D27, DV3). Skipped in easy mode (D4) and
    /// for fry-safe filters or filters with their intake guard on.
    fn intake_suction(&mut self, list: &mut Vec<Creature>) -> bool {
        /// Shrimp up to 2.6 cm long (all cherry shrimp and young amano shrimp)
        /// fit through an unguarded intake slot.
        const SMALL_SHRIMP_MAX_MM: u32 = 26;
        if self.creatures.easy || self.devices.is_empty() || list.is_empty() {
            return false;
        }
        let mut intakes: Vec<(crate::DeviceId, usize, usize, Option<u8>)> = Vec::new();
        for d in self.devices.values() {
            let art = self.devices.kinds().get(d.kind);
            if art.fry_safe || d.intake_guard {
                continue;
            }
            let back_row = d.back_band_row(art);
            for (ix, iy) in d.intakes(art) {
                let band = back_row.and_then(|r| (iy >= r).then_some(DEPTH_BANDS - 1));
                intakes.push((d.id, ix, iy, band));
            }
        }
        if intakes.is_empty() {
            return false;
        }
        let mut sucked: Vec<crate::DeviceId> = Vec::new();
        let before = list.len();
        list.retain(|c| {
            if c.held_until >= self.tick {
                return true;
            }
            let is_fry = self.life_stage(c) == LifeStage::Fry;
            let is_small_shrimp = !is_fry
                && self
                    .creatures
                    .species
                    .get(c.species)
                    .is_some_and(|s| s.name.ends_with("_shrimp"))
                && self.length_mm(c) <= SMALL_SHRIMP_MAX_MM;
            if !is_fry && !is_small_shrimp {
                return true;
            }
            let body = self.creature_body(c);
            if body.is_empty() {
                return true;
            }
            let c_band = c.band();
            if let Some(&(dev_id, _, _, _)) = intakes.iter().find(|&&(_, ix, iy, band)| {
                band.is_none_or(|b| b.abs_diff(c_band) <= BAND_REACH)
                    && ix + 1 >= body.x0
                    && ix <= body.x1
                    && iy + 1 >= body.y0
                    && iy <= body.y1
            }) {
                sucked.push(dev_id);
                false
            } else {
                true
            }
        });
        if sucked.is_empty() {
            return false;
        }
        let cap = self.life.eco.filtration.capacity;
        let load_delta = if cap > 0.0 { 2.0 / cap } else { 0.0 };
        let mut flow_changed = false;
        for dev_id in sucked {
            flow_changed |= self.add_device_load(dev_id, load_delta);
        }
        if flow_changed {
            self.recompute_current();
        }
        list.len() != before
    }

    /// One tick of one creature. Returns whether it moved or turned.
    fn update_creature(&mut self, c: &mut Creature) -> bool {
        let Some(k) = self.grown_kind(c) else {
            return false;
        };
        let k = self.quirks(c).own_kind(k);
        let before = (c.x, c.y, c.facing_left, c.turn);
        if c.held_until >= self.tick {
            return false;
        }
        // A burrower in the bed digs, holds or digs out, and does nothing
        // else (see `bottom`).
        if k.burrow_every > 0 && self.burrow(&k, c, c.hungry()) {
            c.satiety = c.satiety.saturating_sub(1);
            return (c.x, c.y) != (before.0, before.1);
        }
        if self.unstick(&k, c) {
            return (c.x, c.y) != (before.0, before.1);
        }
        // Asleep it only rests (see `night`).
        if self.sleeps(&k, c) {
            return self.rest_step(&k, c);
        }
        let mut rng = chunk_stream(self.seed, self.tick, CREATURE_STREAM + u64::from(c.id.0));
        c.satiety = c.satiety.saturating_sub(1);
        let hungry = c.hungry();
        self.habits(&k, c, &mut rng);
        self.shift_depth(&k, c);
        let swim = self.swim(&k, c).filter(|_| c.food.is_none() && c.fear == 0);
        if k.zone == Zone::Surfaces {
            if c.pane != Pane::Off {
                self.pane_step(&k, c, &mut rng);
                if hungry && self.anything_to_eat(&k) {
                    self.eat(&k, c);
                }
                return (c.x, c.y, c.facing_left, c.turn) != before;
            }
            if self.seek_pane(&k, c) {
                return true;
            }
            if let Some(swim) = swim
                && self.swim_step(&k, c, swim)
            {
                return (c.x, c.y, c.facing_left, c.turn) != before;
            }
            self.crawl(&k, c);
            if hungry && self.anything_to_eat(&k) {
                self.eat(&k, c);
            }
            return (c.x, c.y, c.facing_left, c.turn) != before;
        }
        self.pursue(&k, c);
        self.startle(&k, c);
        self.choose_food(&k, c, hungry);
        let sponge = self.sponge_graze_target(&k, c, hungry);
        if sponge.is_none() && self.forage_climb(&k, c, hungry) {
            return (c.x, c.y, c.facing_left, c.turn) != before;
        }
        let (tx, ty, speed) = self.steering_target(&k, c, sponge, &mut rng);
        // Hovering at the cursor (curious): it holds station, even
        // against the current, but still eats what is under its mouth.
        if speed == 0 {
            (c.vx, c.vy, c.dodging) = (0, 0, 0);
            if hungry && self.anything_to_eat(&k) && self.eat(&k, c) {
                c.food = None;
            }
            return (c.x, c.y, c.facing_left, c.turn) != before;
        }
        // A walker's swim takes over from wandering, not from food, sponge
        // grazing, or fright.
        if let Some(swim) = swim.filter(|_| c.food.is_none() && sponge.is_none() && c.fear == 0) {
            self.swim_step(&k, c, swim);
            if hungry && self.anything_to_eat(&k) && self.eat(&k, c) {
                c.food = None;
            }
            return (c.x, c.y, c.facing_left, c.turn) != before;
        }
        let flipping = pursuit::flipping(&k, c);
        let moved = self.step_toward(&k, c, (tx, ty, speed), sponge.is_none(), flipping);
        if hungry && self.anything_to_eat(&k) && self.eat(&k, c) {
            c.food = None;
        } else if moved != Move::Moved {
            (c.vx, c.vy) = (0, 0);
            if moved == Move::Blocked && !flipping {
                if let Some(f) = c.food.take() {
                    c.ignore = Some(f);
                }
                self.pursuit_blocked(&k, c);
                self.pick_target(&k, c, &mut rng);
                // A schooler that can't reach its school strays for a while
                // on its new target, out of whatever pocket holds it.
                if k.schooling {
                    c.stray = u16::try_from(c.retarget).unwrap_or(0);
                }
            }
        }
        if !flipping {
            update_facing(c, sponge);
        }
        (c.x, c.y, c.facing_left, c.turn) != before
    }

    /// Steer, apply the current and step, giving up a blocked depth dodge
    /// once it has had time to clear the obstacle.
    fn step_toward(
        &self,
        k: &Kind,
        c: &mut Creature,
        target: (i32, i32, i32),
        walking: bool,
        flipping: bool,
    ) -> Move {
        let (tx, ty, speed) = target;
        if flipping {
            flip_kick(c, tx, ty, speed);
        } else {
            steer(k, c, tx, ty, speed, speed > k.cruise);
        }
        let walking = walking && !flipping;
        // Walkers keep a downward bias even on long, nearly horizontal
        // routes. A one-row step lets them follow a sloping sand bed.
        // Hoppers (shrimp) kick upwards now and then and sink back, except
        // while climbing/perching on a sponge filter to graze its foam (DV4).
        let mut bottom_walking = false;
        if k.zone == Zone::Bottom && walking {
            let hopping = self.hopping(k, c);
            bottom_walking = !hopping;
            c.vy = if hopping {
                -k.top
            } else if k.live {
                // Live food sinks faster than it crawls.
                live::SINK
            } else {
                k.cruise
            };
        }
        self.apply_current(c, bottom_walking);

        // Move, sliding along whatever blocks one axis; a plant in the way
        // sends it to another band.
        let mut moved = self.try_move(k, c);
        if moved == Move::Dodging {
            c.dodging += 1;
            // A dodge that doesn't clear the plant in time is given up.
            if c.dodging > DODGE_TICKS {
                moved = Move::Blocked;
                c.tz = z_of(c.band());
            }
        }
        if moved != Move::Dodging {
            c.dodging = 0;
        }
        moved
    }

    /// The world moved something solid into `c`: it shifts to a free band
    /// where it is, or hops out. Returns whether it was stuck (which takes
    /// its tick).
    fn unstick(&self, k: &Kind, c: &mut Creature) -> bool {
        if self.fits(k, c.turn, c.x, c.y, c.band()) {
            return false;
        }
        if let Some(b) = self.free_band(k, c.turn, c.x, c.y, c.band()) {
            (c.z, c.tz) = (z_of(b), z_of(b));
        } else if let Some((x, y)) = self.nearest_fit(k, c.turn, c.x, c.y, c.band(), UNSTUCK_RADIUS)
        {
            (c.x, c.y, c.vx, c.vy) = (x, y, 0, 0);
            c.retarget = 0;
        }
        true
    }

    /// Whether a hopping walker is in the kick of a hop this tick. Each
    /// creature's hops are offset by its id, so a group doesn't hop as one.
    /// A kick ends early (or never starts) where the rest of it would lift
    /// the walker above its zone, so a hop never leaves its depth band.
    fn hopping(&self, k: &Kind, c: &Creature) -> bool {
        if k.hop_every == 0 {
            return false;
        }
        // Its own rhythm, 80–125% of its species'.
        let every = self
            .quirks(c)
            .scale(salt::HOP, 80, 125, k.hop_every)
            .max(HOP_TICKS + 1);
        let phase = (self.tick + u64::from(c.id.0) * 7919) % every;
        if phase >= HOP_TICKS {
            return false;
        }
        let (top, _) = zone(k, self.surface_row, self.height);
        // The most the rest of this kick can rise, in 1/256 cells (at most
        // HOP_TICKS = 8 ticks at top speed).
        #[allow(clippy::cast_possible_truncation)]
        let rise = (HOP_TICKS - phase) as i32 * k.top;
        let body_top = c.y - k.h * ONE / 2;
        i64::from(body_top - rise) >= to_i64(top) * i64::from(ONE)
    }

    fn edible_cell(&self, k: &Kind, index: usize) -> bool {
        self.edible_material_cell(k, index)
            || (self.algae.coat[index] != 0
                && k.graze[usize::from(self.algae.coat[index] >> 4)] != 0
                && self.algae_water(index).next().is_some())
    }

    fn edible_material_cell(&self, k: &Kind, index: usize) -> bool {
        let cell = self.cells[index];
        k.edible[usize::from(cell.material.0)]
            && (self.plants.part(cell.material).is_none() || self.palatable_plant(cell.state))
    }

    fn is_floating_leaf(&self, i: u32) -> bool {
        self.plants.part(self.cells[i as usize].material) == Some(Part::Float)
    }

    /// Keeps `c` heading for food while hungry: a new search when its food
    /// has gone, and every [`SEARCH_EVERY`] ticks for something closer.
    fn choose_food(&self, k: &Kind, c: &mut Creature, hungry: bool) {
        if !hungry
            || (self.life.food_cells() == 0
                && !self.algae.any_coat_where(|v| k.graze[usize::from(v)] != 0))
        {
            c.food = None;
            return;
        }
        let lost = c.food.is_some_and(|i| !self.edible_cell(k, i as usize));
        if lost || (self.tick + u64::from(c.id.0)).is_multiple_of(SEARCH_EVERY) {
            c.food = self.nearest_food(k, c);
        }
    }

    /// Nudges a creature's velocity with the 2D water current at its centre.
    /// Does nothing when the tank has no current. Bottom walkers feel a
    /// gentler horizontal push so they keep to the bed.
    pub(super) fn apply_current(&self, c: &mut Creature, bottom_walking: bool) {
        let (Some(cx), Some(cy)) = (
            usize::try_from(c.x / ONE).ok(),
            usize::try_from(c.y / ONE).ok(),
        ) else {
            return;
        };
        let [vx, vy] = self.current_at_cell(cx, cy);
        if vx == 0.0 && vy == 0.0 {
            return;
        }
        #[allow(clippy::cast_precision_loss)] // small exact constants
        let per_second = ONE as f32 / TICKS_PER_SECOND as f32;
        let factor = if bottom_walking { 0.15 } else { 0.35 };
        #[allow(clippy::cast_possible_truncation)] // bounded by current speed (≤ 20 cells/s)
        let dx = (vx * per_second * factor).round() as i32;
        #[allow(clippy::cast_possible_truncation)]
        let dy = if bottom_walking {
            0
        } else {
            (vy * per_second * factor).round() as i32
        };
        let limit = 16 * ONE;
        c.vx = (c.vx + dx).clamp(-limit, limit);
        c.vy = (c.vy + dy).clamp(-limit, limit);
    }

    /// Moves `c` by its velocity, or along one axis if the other is
    /// blocked. Where a move is blocked only by plants and clear in a band
    /// it can shift to, it holds still and heads for that band instead (see
    /// the module docs).
    fn try_move(&self, k: &Kind, c: &mut Creature) -> Move {
        if (c.vx, c.vy) == (0, 0) {
            return Move::Moved;
        }
        let band = c.band();
        if k.zone == Zone::Bottom
            && c.vx != 0
            && !self.fits(k, c.turn, c.x, c.y + ONE, band)
            && !self.fits(k, c.turn, c.x + c.vx, c.y, band)
            && self.fits(k, c.turn, c.x, c.y - ONE, band)
            && self.fits(k, c.turn, c.x + c.vx, c.y - ONE, band)
        {
            c.x += c.vx;
            c.y -= ONE;
            return Move::Moved;
        }
        for (dx, dy) in [(c.vx, c.vy), (c.vx, 0), (0, c.vy)] {
            if (dx, dy) == (0, 0) {
                continue;
            }
            if self.fits(k, c.turn, c.x + dx, c.y + dy, band) {
                c.x += dx;
                c.y += dy;
                if dx == 0 {
                    c.vx = 0;
                }
                if dy == 0 {
                    c.vy = 0;
                }
                return Move::Moved;
            }
            // Blocked in this band: if the move is clear in a band it can
            // reach, shift there rather than slide, keeping to a shift
            // under way.
            if c.dodging > 0 && c.z != c.tz && self.only_banded_block(k, c.turn, c.x + dx, c.y + dy)
            {
                return Move::Dodging;
            }
            if let Some(b) = self.depth_route(k, c, c.x + dx, c.y + dy) {
                c.tz = z_of(b);
                return Move::Dodging;
            }
        }
        Move::Blocked
    }

    /// Eases `c`'s depth a step towards its target band. It enters a new
    /// band only where its body fits there; if not, it stays and gives up
    /// on that target.
    fn shift_depth(&self, k: &Kind, c: &mut Creature) {
        if c.z == c.tz {
            return;
        }
        let z = c.z + (c.tz - c.z).clamp(-DEPTH_SPEED, DEPTH_SPEED);
        if band_of(z) == c.band() || self.fits(k, c.turn, c.x, c.y, band_of(z)) {
            c.z = z;
        } else {
            c.tz = z_of(c.band());
            c.z = c.tz;
        }
    }

    /// The nearest band in `c`'s species range, other than its own, where
    /// its body fits at `(x, y)` and that it can shift to from where it is
    /// (it fits in every band on the way): front first for an even id,
    /// back first for an odd one.
    #[allow(clippy::many_single_char_names)] // cell maths
    fn depth_route(&self, k: &Kind, c: &Creature, x: i32, y: i32) -> Option<u8> {
        if !self.only_banded_block(k, c.turn, x, y) {
            return None;
        }
        let here = i32::from(c.band());
        let (lo, hi) = (i32::from(k.bands.0), i32::from(k.bands.1));
        let dirs = if c.id.0.is_multiple_of(2) {
            [-1, 1]
        } else {
            [1, -1]
        };
        let mut open = [true; 2];
        for d in 1..i32::from(DEPTH_BANDS) {
            for (side, dir) in dirs.into_iter().enumerate() {
                let b = here + dir * d;
                if !open[side] {
                    continue;
                }
                let Ok(b) = u8::try_from(b) else {
                    open[side] = false;
                    continue;
                };
                if !(lo..=hi).contains(&i32::from(b)) || !self.fits(k, c.turn, c.x, c.y, b) {
                    open[side] = false;
                } else if self.fits(k, c.turn, x, y, b) {
                    return Some(b);
                }
            }
            if open == [false; 2] {
                break;
            }
        }
        None
    }

    /// Whether a body turned `turn` at `(x, y)` lies inside the grid and
    /// overlaps depth-banded obstacles and nothing else it can't swim through.
    fn only_banded_block(&self, k: &Kind, turn: u8, x: i32, y: i32) -> bool {
        let (wide, tall) = extent(k, turn);
        let Some(to) = body_box(wide, tall, x, y, self.width, self.height) else {
            return false;
        };
        let mut banded = false;
        for row in to.y0..to.y1 {
            for i in row * self.width + to.x0..row * self.width + to.x1 {
                let m = self.cells[i].material;
                if !self.creatures.swim[usize::from(m.0)] {
                    if self.plants.part(m).is_none()
                        && self.device_cell_band(i).is_none()
                        && self
                            .rigid_bands(i)
                            .is_none_or(|bands| bands == (0, DEPTH_BANDS - 1))
                    {
                        return false;
                    }
                    banded = true;
                }
            }
        }
        banded
    }

    /// The nearest other band in the species' range where a body turned
    /// `turn` fits at `(x, y)`, front first.
    fn free_band(&self, k: &Kind, turn: u8, x: i32, y: i32, band: u8) -> Option<u8> {
        let (lo, hi) = k.bands;
        (1..DEPTH_BANDS)
            .flat_map(|d| [band.checked_sub(d), band.checked_add(d)])
            .flatten()
            .filter(|b| (lo..=hi).contains(b))
            .find(|&b| self.fits(k, turn, x, y, b))
    }

    /// Eats one food cell within the species' reach of the body, if there
    /// is one.
    fn eat(&mut self, k: &Kind, c: &mut Creature) -> bool {
        let Some(reach) = self.feeding_reach(k, c) else {
            return false;
        };
        for y in reach.y0..reach.y1 {
            for x in reach.x0..reach.x1 {
                if self.edible_material_cell(k, y * self.width + x) {
                    self.eat_cell(c, x, y);
                    return true;
                }
            }
        }
        (k.grazes_algae && self.graze_algae(k, reach, c))
            || (k.eats_corpses && self.bite_corpse(reach, c))
            || (k.is_bottom_shrimp && self.graze_sponge(reach, c))
    }

    fn feeding_reach(&self, k: &Kind, c: &Creature) -> Option<Rect> {
        let b = body(k, c.turn, c.x, c.y, self.width, self.height)?;
        Some(Rect {
            x0: b.x0.saturating_sub(k.reach),
            y0: b.y0.saturating_sub(k.reach),
            x1: (b.x1 + k.reach).min(self.width),
            y1: (b.y1 + k.reach).min(self.height),
        })
    }

    fn eat_cell(&mut self, c: &mut Creature, x: usize, y: usize) {
        let to = if y < self.surface_row {
            self.creatures.air
        } else {
            self.creatures.water
        };
        let cell = self.cells[y * self.width + x];
        if self.plants.part(cell.material).is_some() {
            self.graze_plant(cell.state);
        }
        self.write_cell(x, y, to, &mut None);
        Self::feed_bite(c, 100);
    }

    fn graze_algae(&mut self, k: &Kind, reach: Rect, c: &mut Creature) -> bool {
        if self.algae.coated != 0 {
            for y in reach.y0..reach.y1 {
                for x in reach.x0..reach.x1 {
                    let i = y * self.width + x;
                    let coat = self.algae.coat[i];
                    let percent = k.graze[usize::from(coat >> 4)];
                    if coat != 0
                        && percent != 0
                        && self
                            .algae_water(i)
                            .any(|j| reach.contains(j % self.width, j / self.width))
                    {
                        self.write_coat(i, nibble_film(coat, percent));
                        Self::feed_bite(c, percent);
                        return true;
                    }
                }
            }
        }
        self.graze_glass(k, reach, c)
    }

    /// A bite of `percent` of a full one: the satiety and energy it gives.
    fn feed_bite(c: &mut Creature, percent: u8) {
        let part = |full: u32| (full * u32::from(percent) / 100).max(1);
        c.satiety = c
            .satiety
            .saturating_add(u16::try_from(part(u32::from(BITE))).unwrap_or(BITE))
            .min(FULL);
        c.energy = c
            .energy
            .saturating_add(part(BITE_ENERGY))
            .min(ENERGY_RESERVE);
        c.eaten += 1;
    }

    fn graze_glass(&mut self, k: &Kind, reach: Rect, c: &mut Creature) -> bool {
        use crate::field::FIELD_SCALE;

        if self.algae.glass == 0 || reach.is_empty() {
            return false;
        }
        let band = c.band();
        let fw = self.width.div_ceil(FIELD_SCALE);
        for fy in reach.y0 / FIELD_SCALE..=(reach.y1 - 1) / FIELD_SCALE {
            let algae = &mut self.algae;
            let glass = if band == 0 || band == DEPTH_BANDS - 1 {
                let pane = if band == 0 {
                    &mut algae.front
                } else {
                    &mut algae.back
                };
                pane[fy * fw + reach.x0 / FIELD_SCALE..=fy * fw + (reach.x1 - 1) / FIELD_SCALE]
                    .iter_mut()
                    .find(|film| **film != 0 && k.graze[usize::from(**film >> 4)] != 0)
            } else {
                None
            };
            let glass = glass.or_else(|| {
                let sides = &mut algae.sides[fy];
                let grazeable = |film: u8| film != 0 && k.graze[usize::from(film >> 4)] != 0;
                let edge = if reach.x0 == 0 && grazeable(sides[0]) {
                    0
                } else if reach.x1 == self.width && grazeable(sides[1]) {
                    1
                } else {
                    return None;
                };
                Some(&mut sides[edge])
            });
            if let Some(film) = glass {
                let old = *film;
                let percent = k.graze[usize::from(old >> 4)];
                *film = nibble_film(old, percent);
                let new = *film;
                self.algae.glass_changed((old, new));
                Self::feed_bite(c, percent);
                return true;
            }
        }
        false
    }

    /// Shrimp biofilm grazing on an adjacent sponge filter's foam body (DV4):
    /// picks invisible biofilm/mulm off the foam at a steady cadence without
    /// altering device cells, restoring satiety and energy and gently trimming
    /// mechanical media load.
    fn graze_sponge(&mut self, reach: Rect, c: &mut Creature) -> bool {
        /// Ticks between biofilm nibbles on a sponge filter (1 world second).
        const SPONGE_GRAZE_INTERVAL: u64 = TICKS_PER_SECOND as u64;
        /// Tiny mechanical load reduction per shrimp nibble (0.2% of capacity).
        const SPONGE_GRAZE_CLEAN: f32 = 0.002;
        if !self.has_grazeable_device()
            || !(self.tick + u64::from(c.id.0)).is_multiple_of(SPONGE_GRAZE_INTERVAL)
        {
            return false;
        }
        let mut hit = None;
        'scan: for y in reach.y0..reach.y1 {
            for x in reach.x0..reach.x1 {
                if let Some(dev_id) = self.device_at(x, y)
                    && let Some(d) = self.device(dev_id)
                    && let Some((gy0, gy1)) = d.graze_band(self.devices.kinds().get(d.kind))
                    && (gy0..=gy1).contains(&y)
                {
                    hit = Some(dev_id);
                    break 'scan;
                }
            }
        }
        let Some(dev_id) = hit else {
            return false;
        };
        c.satiety = c.satiety.saturating_add(BITE / 2).min(FULL);
        c.energy = c.energy.saturating_add(BITE_ENERGY / 2).min(ENERGY_RESERVE);
        c.eaten += 1;
        if self.add_device_load(dev_id, -SPONGE_GRAZE_CLEAN) {
            self.recompute_current();
        }
        true
    }

    /// The nearest food cell `c` eats within [`FOOD_SENSE`], skipping those
    /// within [`IGNORE_REACH`] of the one it gave up on.
    #[allow(clippy::many_single_char_names)] // cell maths
    fn nearest_food(&self, k: &Kind, c: &Creature) -> Option<u32> {
        let (cx, cy) = (i64::from(c.x >> 8), i64::from(c.y >> 8));
        let sense2 = FOOD_SENSE * FOOD_SENSE;
        let mut best: Option<(i64, u32)> = None;
        // A fish blocked at the surface can otherwise flip between distant
        // cells of the same mat. Give it a stretch of wandering or following
        // loose food between leaf retries. Use the remembered row since a
        // floating leaf may have moved off the ignored cell.
        let skip_float = c.food.is_none()
            && c.ignore
                .is_some_and(|i| i as usize / self.width <= self.surface_row + 1)
            && !(self.tick + u64::from(c.id.0)).is_multiple_of(FLOAT_RETRY_EVERY);
        let ignored = c.ignore.map(|i| {
            (
                to_i64(i as usize % self.width),
                to_i64(i as usize / self.width),
            )
        });
        for (ci, list) in self.life.food_lists().iter().enumerate() {
            let film = k.grazes_algae.then(|| &self.algae.by_chunk[ci]);
            if list.is_empty() && film.is_none_or(std::collections::BTreeSet::is_empty) {
                continue;
            }
            // Skip whole chunks out of range (or no closer than the best).
            let b = self.chunks[ci].bounds;
            let gap = |v: i64, lo: usize, hi: usize| {
                let (lo, hi) = (to_i64(lo), to_i64(hi) - 1);
                (lo - v).max(v - hi).max(0)
            };
            let (gx, gy) = (gap(cx, b.x0, b.x1), gap(cy, b.y0, b.y1));
            let limit = best.map_or(sense2, |(d, _)| d);
            if gx * gx + gy * gy > limit {
                continue;
            }
            // The coat set is row-major: visit only its cells inside the
            // sensing square, a range per row, not every coated cell.
            let rows = (cy - FOOD_SENSE).max(to_i64(b.y0))..(cy + FOOD_SENSE + 1).min(to_i64(b.y1));
            let x0 = (cx - FOOD_SENSE).max(to_i64(b.x0));
            let x1 = (cx + FOOD_SENSE + 1).min(to_i64(b.x1)).max(x0);
            let w = to_i64(self.width);
            let coat = film.into_iter().flat_map(|set| {
                rows.clone().flat_map(move |y| {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    // Row-major cell indices fit u32 (MAX_SIDE squared).
                    set.range((y * w + x0) as u32..(y * w + x1) as u32)
                })
            });
            for &i in list.iter().chain(coat) {
                if !self.edible_cell(k, i as usize) {
                    continue;
                }
                let (fx, fy) = (
                    to_i64(i as usize % self.width),
                    to_i64(i as usize / self.width),
                );
                if ignored.is_some_and(|(ix, iy)| {
                    (fx - ix).abs() <= IGNORE_REACH && (fy - iy).abs() <= IGNORE_REACH
                }) {
                    continue;
                }
                if skip_float && self.is_floating_leaf(i) {
                    continue;
                }
                if !near_home(k, c, fx) {
                    continue;
                }
                let d = (fx - cx).pow(2) + (fy - cy).pow(2);
                if d <= sense2 && best.is_none_or(|(bd, bi)| d < bd || (d == bd && i < bi)) {
                    best = Some((d, i));
                }
            }
        }
        best.map(|(_, i)| i)
    }

    /// A new wander target, giving a shy creature up to three draws for cover.
    fn pick_target(&self, k: &Kind, c: &mut Creature, rng: &mut Pcg32) {
        let previous_band = c.tz;
        self.draw_target(k, c, rng);
        if k.shy {
            let (w, h) = extent(k, c.turn);
            for _ in 0..2 {
                if self.in_cover(w, h, c.tx, c.ty, band_of(c.tz)) {
                    break;
                }
                c.tz = previous_band;
                self.draw_target(k, c, rng);
            }
        }
    }

    /// A random wander target inside the zone, and how long to keep it.
    #[allow(clippy::many_single_char_names)] // cell maths
    fn draw_target(&self, k: &Kind, c: &mut Creature, rng: &mut Pcg32) {
        let (top, bottom) = zone(k, self.surface_row, self.height);
        let w = to_i64(self.width);
        let span = |lo: i64, hi: i64, rng: &mut Pcg32| {
            if hi <= lo {
                return i64::midpoint(lo, hi);
            }
            let n = u64::try_from(hi - lo).unwrap_or(1);
            lo + i64::try_from(u64::from(rng.next_u32()) % n).unwrap_or(0)
        };
        // Body centres that keep the whole body inside the zone and walls.
        let (hw, hh) = (i64::from(k.w / 2 + 1), i64::from(k.h / 2));
        // A home owner wanders only near its home.
        let (x0, x1) =
            home_span(k, c).map_or((hw, w - hw), |(lo, hi)| (lo.max(hw), hi.min(w - hw)));
        let x = span(x0, x1, rng);
        // A territorial fish keeps to rows within its territory, too.
        let (y0, y1) = (to_i64(top) + hh, to_i64(bottom) - hh);
        let (y0, y1) = match c.home {
            Some((_, hy)) if k.territory > 0 => {
                let r = i64::from(k.territory);
                let (lo, hi) = (y0.max(i64::from(hy) - r), y1.min(i64::from(hy) + r + 1));
                if lo < hi { (lo, hi) } else { (y0, y1) }
            }
            _ => (y0, y1),
        };
        let zone_rows = (y0, y1);
        // Every other swimmer's target is near its favourite depth: a
        // fifth of the rows either side of its own spot in the range.
        let q = self.quirks(c);
        let (y0, y1) = if k.zone != Zone::Bottom && rng.next_u32().is_multiple_of(2) {
            let favourite =
                y0 + (y1 - y0) * i64::try_from(q.percent(salt::DEPTH, 0, 100)).unwrap_or(50) / 100;
            let reach = (y1 - y0) / 5;
            ((favourite - reach).max(y0), (favourite + reach + 1).min(y1))
        } else {
            (y0, y1)
        };
        let y = span(y0, y1, rng);
        (c.tx, c.ty) = (centre_of(x), centre_of(y));
        c.retarget = self.wander_span(c, rng);
        // Every other target or so, a new band too.
        let (front, back) = k.bands;
        let pick = rng.next_u32();
        if pick.is_multiple_of(2) {
            let n = u32::from(back - front) + 1;
            c.tz = z_of(front + u8::try_from((pick >> 1) % n).unwrap_or(0));
        }
        if k.shoaling
            && let Some((tx, ty)) = self.shoal_target(k, c, rng)
        {
            c.tx = clamp_target(tx, x0, x1);
            if k.zone != Zone::Bottom {
                c.ty = clamp_target(ty, zone_rows.0, zone_rows.1);
            }
        }
    }

    /// A restless one keeps a target for less time than a placid one.
    fn wander_span(&self, c: &Creature, rng: &mut Pcg32) -> u32 {
        let (lo, hi) = RETARGET_TICKS;
        let linger = u32::try_from(self.quirks(c).percent(salt::RESTLESS, 50, 100)).unwrap_or(100);
        lo + rng.next_u32() % (hi - lo) * linger / 100
    }

    /// A target near a random shoalmate, or the nearest one if straying.
    fn shoal_target(&self, k: &Kind, c: &Creature, rng: &mut Pcg32) -> Option<(i32, i32)> {
        // Eight directions in fixed point, or two along the bed.
        const DIAGONAL: i32 = 181;
        const DIRECTIONS: [(i32, i32); 8] = [
            (ONE, 0),
            (-ONE, 0),
            (0, ONE),
            (0, -ONE),
            (DIAGONAL, DIAGONAL),
            (-DIAGONAL, DIAGONAL),
            (DIAGONAL, -DIAGONAL),
            (-DIAGONAL, -DIAGONAL),
        ];
        let mates = &self.creatures.school[usize::from(c.species.0)];
        let bottom = k.zone == Zone::Bottom;
        let distance_sq = |m: &Schoolmate| {
            let dx = i64::from(m.x) - i64::from(c.x);
            let dy = if bottom {
                0
            } else {
                i64::from(m.y) - i64::from(c.y)
            };
            dx * dx + dy * dy
        };
        // Young shoal with young and adults with adults, as real shoals
        // sort by size: guppy fry don't swim up to the adults that eat them.
        let adult = self.kind(c.species).is_some_and(|g| c.growth >= g.maturity);
        let others = || mates.iter().filter(|m| m.id != c.id && m.adult == adult);
        let nearest = others().min_by_key(|m| distance_sq(m))?;
        // In the shorter sex's full-grown length, so a shoal of big females
        // (a guppy's are half as long again) keeps as close as the males'.
        let length = self
            .kind(c.species)
            .map_or(k.w, |g| g.size_of(false).0.min(g.size_of(true).0))
            * ONE;
        let mate = if distance_sq(nearest) > i64::from(8 * length).pow(2) {
            nearest
        } else {
            if rng.next_u32().is_multiple_of(4) {
                return None;
            }
            let count = u32::try_from(others().count()).ok()?;
            others().nth(usize::try_from(rng.next_u32() % count).ok()?)?
        };
        let reach = length * (150 + i32::try_from(rng.next_u32() % 251).ok()?) / 100;
        let n = if bottom { 2 } else { 8 };
        let (dx, dy) = DIRECTIONS[usize::try_from(rng.next_u32() % n).ok()?];
        Some((mate.x + dx * reach / ONE, mate.y + dy * reach / ONE))
    }

    /// The nearest spot within `radius` cells where the body fits in
    /// `band`, searching rings outwards in a fixed order.
    fn nearest_fit(
        &self,
        k: &Kind,
        turn: u8,
        x: i32,
        y: i32,
        band: u8,
        radius: i32,
    ) -> Option<(i32, i32)> {
        for r in 1..=radius {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs() != r && dy.abs() != r {
                        continue;
                    }
                    let (nx, ny) = (x + dx * ONE, y + dy * ONE);
                    if self.fits(k, turn, nx, ny, band) {
                        return Some((nx, ny));
                    }
                }
            }
        }
        None
    }

    /// Whether a body turned `turn` and centred at `(x, y)` (fixed point)
    /// lies inside the grid on cells it may overlap in `band`.
    /// A surface crawler holds floaters' hanging roots as it does any
    /// plant; everything else passes through them (see the module docs).
    fn fits(&self, k: &Kind, turn: u8, x: i32, y: i32, band: u8) -> bool {
        let (wide, tall) = extent(k, turn);
        let soft = k.zone != Zone::Surfaces;
        self.fits_box(wide, tall, x, y, Some(band), soft)
    }

    /// Whether a `w × h`-cell box centred at `(x, y)` (fixed point) lies
    /// inside the grid on cells it may overlap: swimmable ones, plant
    /// cells further than [`BAND_REACH`] from `band`, rigid cells outside
    /// their occupied bands and, if `soft` and in a band, a floater's
    /// hanging roots. With no band, every obstacle counts.
    fn fits_box(&self, wide: i32, tall: i32, x: i32, y: i32, band: Option<u8>, soft: bool) -> bool {
        let Some(b) = body_box(wide, tall, x, y, self.width, self.height) else {
            return false;
        };
        let mut memo = None;
        (b.y0..b.y1).all(|row| {
            (row * self.width + b.x0..row * self.width + b.x1)
                .all(|i| !self.blocks_with(i, band, soft, &mut memo))
        })
    }

    /// Whether the cell at index `i` blocks a body in `band` (`None`: in
    /// every band). A floater's roots count as solid here: this is what a
    /// crawler holds on to.
    pub(super) fn blocks(&self, i: usize, band: Option<u8>) -> bool {
        self.blocks_with(i, band, false, &mut None)
    }

    /// [`World::blocks`], with a plant memo for a run of cells; `soft`
    /// lets a body in a band through a floater's hanging roots.
    #[inline]
    fn blocks_with(
        &self,
        i: usize,
        band: Option<u8>,
        soft: bool,
        memo: &mut Option<PlantDepth>,
    ) -> bool {
        let m = self.cells[i].material;
        if self.creatures.swim[usize::from(m.0)] {
            return false;
        }
        match (band, self.plants.part(m)) {
            (Some(band), Some(part)) => {
                let Some(p) = self.plant_band_with(i, memo) else {
                    return true;
                };
                if soft && part == Part::Root && memo.as_ref().is_some_and(PlantDepth::floats) {
                    return false;
                }
                p.abs_diff(band) <= BAND_REACH
            }
            (Some(band), None) if self.is_rigid(i) => {
                let (front, back) = self.body_bands_at(i);
                (front..=back).contains(&band)
            }
            (Some(band), None) => self
                .device_cell_band(i)
                .is_none_or(|dev_band| dev_band.abs_diff(band) <= BAND_REACH),
            _ => true,
        }
    }

    fn kind(&self, species: SpeciesId) -> Option<&Kind> {
        self.creatures.kinds[usize::from(species.0)].as_ref()
    }

    fn creature_mut(&mut self, id: CreatureId) -> Option<&mut Creature> {
        let i = self
            .creatures
            .list
            .binary_search_by_key(&id, |c| c.id)
            .ok()?;
        Some(&mut self.creatures.list[i])
    }
}

/// Accelerates `c` towards `(tx, ty)` at up to `speed`, slowing on
/// arrival. While wandering vertical speed is held to half, as fish mostly
/// swim level; `darting` (above cruising speed: after food, hunting or
/// fleeing) lifts that.
fn update_facing(c: &mut Creature, sponge: Option<(i32, i32, i32)>) {
    if c.vx > TURN_SPEED {
        c.facing_left = false;
    } else if c.vx < -TURN_SPEED {
        c.facing_left = true;
    } else if let Some((gx, gy, mid_x)) = sponge
        && dist(gx - c.x, gy - c.y) <= i64::from(2 * ONE)
    {
        // Perched against the sponge: face inward towards the foam.
        c.facing_left = mid_x < c.x;
    }
}

fn steer(k: &Kind, c: &mut Creature, tx: i32, ty: i32, speed: i32, darting: bool) {
    let (dx, dy) = (i64::from(tx - c.x), i64::from(ty - c.y));
    let d = dist(tx - c.x, ty - c.y);
    let speed = i64::from(speed);
    let want = if d < ARRIVE {
        speed * d / ARRIVE
    } else {
        speed
    };
    let (wx, wy) = if d == 0 {
        (0, 0)
    } else {
        (dx * want / d, dy * want / d)
    };
    let a = i64::from(k.accel);
    let top = i64::from(k.top);
    let vx = i64::from(c.vx) + (wx - i64::from(c.vx)).clamp(-a, a);
    let vy = i64::from(c.vy) + (wy - i64::from(c.vy)).clamp(-a, a);
    // Within ±top (< one cell per tick), so the casts are exact.
    c.vx = i32::try_from(vx.clamp(-top, top)).unwrap_or(0);
    let rise = if darting { top } else { top / 2 };
    c.vy = i32::try_from(vy.clamp(-rise, rise)).unwrap_or(0);
}

/// A tail flip's kick (see [`pursuit`]): straight at `speed` (above top
/// speed) towards the flip target, with no easing in, slowing only within
/// a tick's travel of it.
fn flip_kick(c: &mut Creature, tx: i32, ty: i32, speed: i32) {
    let (dx, dy) = (i64::from(tx - c.x), i64::from(ty - c.y));
    let d = dist(tx - c.x, ty - c.y);
    let speed = i64::from(speed);
    let want = speed.min(d);
    let (wx, wy) = if d == 0 {
        (0, 0)
    } else {
        (dx * want / d, dy * want / d)
    };
    c.vx = i32::try_from(wx).unwrap_or(0);
    c.vy = i32::try_from(wy).unwrap_or(0);
}

/// A body's width and height in cells once turned `turn` quarter turns.
fn extent(k: &Kind, turn: u8) -> (i32, i32) {
    if turn % 2 == 1 {
        (k.h, k.w)
    } else {
        (k.w, k.h)
    }
}

/// The cells a body turned `turn` and centred at `(x, y)` (fixed point)
/// covers, rounded outwards; `None` if any part is outside the grid.
/// A per-material-id lookup of `f`.
fn per_material(
    materials: &MaterialTable,
    f: impl Fn(&crate::material::Material) -> bool,
) -> Vec<bool> {
    let mut out = vec![false; 256];
    for m in materials.iter() {
        out[usize::from(m.id.0)] = f(m);
    }
    out
}

/// A validated swimming speed in 1/256 cells per tick.
fn speed_per_tick(cells_per_s: f32) -> i32 {
    // Validated to (0, 16] cells per second: under a cell per tick.
    #[allow(clippy::cast_possible_truncation)]
    let speed =
        (f64::from(cells_per_s) * f64::from(ONE) / f64::from(TICKS_PER_SECOND)).round() as i32;
    speed.max(1)
}

/// A species' seconds between chances (validated to at most 600 s) in
/// ticks, 0 for none.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn every_ticks(every: Option<f32>) -> u64 {
    every.map_or(0, |e| {
        (f64::from(e) * f64::from(TICKS_PER_SECOND)).round() as u64
    })
}

fn body(k: &Kind, turn: u8, x: i32, y: i32, width: usize, height: usize) -> Option<Rect> {
    let (wide, tall) = extent(k, turn);
    body_box(wide, tall, x, y, width, height)
}

/// The cells a `wide × tall`-cell box centred at `(x, y)` (fixed point)
/// covers, rounded outwards; `None` if any part is outside the grid.
fn body_box(wide: i32, tall: i32, x: i32, y: i32, width: usize, height: usize) -> Option<Rect> {
    let (left, top) = (x - wide * ONE / 2, y - tall * ONE / 2);
    let x0 = usize::try_from(left.div_euclid(ONE)).ok()?;
    let y0 = usize::try_from(top.div_euclid(ONE)).ok()?;
    let x1 = usize::try_from((left + wide * ONE + ONE - 1).div_euclid(ONE)).ok()?;
    let y1 = usize::try_from((top + tall * ONE + ONE - 1).div_euclid(ONE)).ok()?;
    (x1 <= width && y1 <= height).then_some(Rect { x0, y0, x1, y1 })
}

/// The zone's rows, `[top, bottom)`, for the water from `surface_row` to
/// the floor at `height`.
fn zone(k: &Kind, surface_row: usize, height: usize) -> (usize, usize) {
    let water = height - surface_row;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )] // fractions in 0..=1 of at most 8192 rows
    let at = |f: f32| (f64::from(f) * water as f64).round() as usize;
    let (top, bottom) = (surface_row + at(k.depth[0]), surface_row + at(k.depth[1]));
    (top, bottom.max(top + 1).min(height))
}

/// The band a depth (1/256 band) rounds to.
fn band_of(z: i32) -> u8 {
    u8::try_from(((z + ONE / 2) / ONE).clamp(0, i32::from(DEPTH_BANDS) - 1)).unwrap_or(0)
}

/// The depth (1/256 band) of the middle of `band`.
fn z_of(band: u8) -> i32 {
    i32::from(band.min(DEPTH_BANDS - 1)) * ONE
}

/// The band creature `id` starts in (and a save from before depth gives
/// it): a hash of the world seed and its id, within `bands`.
pub(crate) fn default_band(seed: u64, id: u32, bands: (u8, u8)) -> u8 {
    let (front, back) = (bands.0.min(bands.1), bands.0.max(bands.1));
    let n = u64::from(back - front) + 1;
    let h = crate::rng::splitmix64(seed ^ crate::rng::splitmix64(u64::from(id) ^ 0x0064_6570_7468));
    front + u8::try_from(h % n).unwrap_or(0)
}

/// Clamp to a wander span, which excludes its upper cell. A body too
/// large for the span aims at the midpoint.
fn clamp_target(value: i32, lo: i64, hi: i64) -> i32 {
    if lo < hi {
        value.clamp(centre_of(lo), centre_of(hi - 1))
    } else {
        centre_of(i64::midpoint(lo, hi))
    }
}

/// The fixed-point centre of cell `v`, clamped well inside `i32`.
fn centre_of(v: i64) -> i32 {
    let v = v.clamp(-(1 << 20), 1 << 20);
    i32::try_from(v).unwrap_or(0) * ONE + ONE / 2
}

/// Euclidean length of a fixed-point vector (integer square root).
fn dist(dx: i32, dy: i32) -> i64 {
    let (dx, dy) = (i64::from(dx), i64::from(dy));
    let sq = u64::try_from(dx * dx + dy * dy).unwrap_or(0);
    i64::try_from(sq.isqrt()).unwrap_or(i64::MAX)
}

fn to_i64(v: usize) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Brush, WorldConfig};

    #[test]
    fn only_bottom_shrimp_can_climb_and_graze_sponges() {
        let world = World::new(WorldConfig::default(), MaterialTable::builtin().unwrap()).unwrap();
        for (name, grazes) in [
            ("cherry_shrimp", true),
            ("amano_shrimp", true),
            ("live_brine_shrimp", false),
        ] {
            let id = world.species().id(name).unwrap();
            assert_eq!(world.kind(id).unwrap().is_bottom_shrimp, grazes, "{name}");
        }
    }

    fn shy_world(shy: bool) -> World {
        let mut world = World::new(
            WorldConfig {
                width: 120,
                height: 64,
                seed: 1,
                water_level: 0.95,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let gourami = include_str!("../../../../assets/species/honey_gourami.toml");
        let gourami = gourami.replace("shy = true", &format!("shy = {shy}"));
        let goldfish = include_str!("../../../../assets/species/common_goldfish.toml");
        let species =
            SpeciesTable::from_toml_sources(&[&gourami, goldfish], &world.materials).unwrap();
        world.creatures = Creatures::new(
            species,
            &world.materials,
            world.life.eco.algae.as_deref(),
            world.creatures.water,
            world.creatures.air,
        );
        world.set_easy_mode(true);
        world
    }

    #[test]
    fn shy_gourami_spends_more_time_in_plant_cover() {
        let time_in_cover = |shy| {
            let mut world = shy_world(shy);
            let soil = world.materials().id("aquasoil").unwrap();
            world
                .fill_rect(
                    crate::Rect {
                        x0: 0,
                        y0: 58,
                        x1: 120,
                        y1: 64,
                    },
                    soil,
                    Brush::Replace,
                )
                .unwrap();
            let kind = world.plant_species().id("hygrophila").unwrap();
            let from = world.plant_species().get(kind).unwrap().default_propagule();
            for x in (30..=90).step_by(6) {
                world.paint_plant(x, 57, 0, kind, from).unwrap();
            }
            for _ in 0..10 * 60 * TICKS_PER_SECOND {
                world.tick();
            }
            let gourami = world.species().id("honey_gourami").unwrap();
            let id = world.add_creature(gourami, 60, 24).unwrap();
            let mut covered = 0;
            for tick in 0..3 * 60 * TICKS_PER_SECOND {
                world.tick();
                if tick.is_multiple_of(TICKS_PER_SECOND)
                    && world.creature_in_cover(world.creature(id).unwrap())
                {
                    covered += 1;
                }
            }
            covered
        };
        let (shy, peaceful) = (time_in_cover(true), time_in_cover(false));
        assert!(
            shy >= 6 && shy >= 2 * peaceful,
            "seconds in cover: shy {shy}, peaceful {peaceful}"
        );
    }

    #[test]
    fn a_shy_awake_gourami_startles_at_an_approaching_larger_swimmer() {
        for (shy, asleep, vx, expected) in [
            (true, false, -ONE, true),
            (true, false, ONE, false),
            (true, false, 0, false),
            (true, true, -ONE, false),
            (false, false, -ONE, false),
        ] {
            let mut world = shy_world(shy);
            // Let the gourami's prey setting activate sensing in the non-shy case.
            world.set_easy_mode(shy);
            let gourami = world.species().id("honey_gourami").unwrap();
            let goldfish = world.species().id("common_goldfish").unwrap();
            let id = world.add_creature(gourami, 40, 25).unwrap();
            let other = world.add_creature(goldfish, 56, 25).unwrap();
            world.tick = pursuit::SENSE_EVERY * 2 - u64::from(id.0);
            if asleep {
                world.set_time_of_day(0);
                let fish = world.creature_mut(id).unwrap();
                fish.asleep = true;
                fish.next_breath = u64::MAX;
            }
            let threat = world.creature_mut(other).unwrap();
            threat.vx = vx;
            threat.vy = 0;
            let list = world.creatures.list.clone();
            world.snapshot_near(&list);
            assert_eq!(world.creatures.near.len(), 2);
            let mut fish = world.creature(id).unwrap().clone();
            world.update_creature(&mut fish);
            assert_eq!(
                fish.fear > 0,
                expected,
                "shy {shy}, asleep {asleep}, vx {vx}"
            );
            if expected {
                assert!(fish.tx < fish.x, "the startled gourami should run away");
            }
            assert_eq!(fish.asleep, asleep);
        }
    }

    #[test]
    fn a_cherry_shrimp_flips_away_from_an_approaching_goldfish_and_settles() {
        for (species, vx, flips) in [
            ("cherry_shrimp", -ONE, true),
            ("cherry_shrimp", ONE, false),
            ("amano_shrimp", -ONE, false),
        ] {
            let mut world = World::new(
                WorldConfig {
                    width: 120,
                    height: 64,
                    seed: 1,
                    water_level: 0.95,
                },
                MaterialTable::builtin().unwrap(),
            )
            .unwrap();
            world.set_easy_mode(true);
            let sand = world.materials().id("sand").unwrap();
            world
                .fill_rect(
                    Rect {
                        x0: 0,
                        y0: 60,
                        x1: 120,
                        y1: 64,
                    },
                    sand,
                    Brush::Replace,
                )
                .unwrap();
            let species = world.species().id(species).unwrap();
            let shrimp_id = world.add_creature(species, 40, 59).unwrap();
            let goldfish = world.species().id("common_goldfish").unwrap();
            let fish_id = world.add_creature(goldfish, 50, 54).unwrap();
            let start_tick = pursuit::SENSE_EVERY * 2 - u64::from(shrimp_id.0);
            world.tick = start_tick;
            world.creature_mut(fish_id).unwrap().vx = vx;
            let mut list = world.creatures.list.clone();
            world.snapshot_near(&list);

            let mut shrimp = world.creature(shrimp_id).unwrap().clone();
            shrimp.facing_left = false;
            let (bed_x, bed_y) = (shrimp.x, shrimp.y);
            world.update_creature(&mut shrimp);
            assert_eq!(shrimp.fear, if flips { pursuit::FLIP_TICKS } else { 0 });
            if !flips {
                continue;
            }
            assert!(!shrimp.facing_left);
            for _ in 1..pursuit::FLIP_TICKS {
                world.tick += 1;
                world.update_creature(&mut shrimp);
                assert!(!shrimp.facing_left, "the shrimp should keep facing right");
            }
            assert!(shrimp.x < bed_x - 2 * ONE && shrimp.y <= bed_y - 2 * ONE);
            // Keep a fish approaching from within range even as the shrimp moves away.
            let fish = list.iter().position(|c| c.id == fish_id).unwrap();
            for elapsed in 0..=pursuit::FLIP_REST {
                if elapsed == pursuit::FLIP_REST {
                    assert!(
                        shrimp.y >= bed_y,
                        "the shrimp should settle back onto the bed"
                    );
                    assert_eq!(shrimp.pursuit, 1);
                }
                list[fish].x = shrimp.x + 8 * ONE;
                world.snapshot_near(&list);
                world.tick += 1;
                world.update_creature(&mut shrimp);
                if elapsed < pursuit::FLIP_REST {
                    assert_eq!(shrimp.fear, 0);
                }
            }
            assert_eq!(shrimp.fear, pursuit::FLIP_TICKS, "recovery should expire");
        }
    }

    #[test]
    fn skittish_shrimp_alone_activate_near_sensing() {
        let mut world =
            World::new(WorldConfig::default(), MaterialTable::builtin().unwrap()).unwrap();
        world.set_easy_mode(false);
        let amano = world.species().id("amano_shrimp").unwrap();
        let id = world.add_creature(amano, 40, 30).unwrap();
        world.tick();
        assert!(world.creatures.near.is_empty());
        assert_eq!(world.creatures.near.capacity(), 0);
        world.remove_creature(id);
        let cherry = world.species().id("cherry_shrimp").unwrap();
        world.add_creature(cherry, 40, 30).unwrap();
        world.add_creature(cherry, 50, 30).unwrap();
        world.tick();
        assert_eq!(world.creatures.near.len(), 2);
    }

    #[test]
    fn shy_gourami_startles_at_nearby_non_food_pours_by_day() {
        for (material, x, expected) in [
            ("sand", 48, true),
            ("rock", 48, true),
            ("flake_food", 48, false),
            ("sand", 85, false),
        ] {
            let mut world = shy_world(true);
            let gourami = world.species().id("honey_gourami").unwrap();
            let id = world.add_creature(gourami, 40, 25).unwrap();
            // Past tick 0, where `held_until` 0 still counts as held.
            world.tick += 1;
            let material = world.materials().id(material).unwrap();
            assert!(world.paint(x, 25, 0, material, Brush::Replace).unwrap() > 0);
            let fish = world.creature(id).unwrap();
            assert_eq!(fish.fear > 0, expected);
            if expected {
                assert!(fish.tx < fish.x, "the startled gourami should run away");
            }
        }
    }

    #[test]
    fn a_netted_shy_gourami_isnt_frightened_by_a_pour() {
        let mut world = shy_world(true);
        let gourami = world.species().id("honey_gourami").unwrap();
        let id = world.add_creature(gourami, 40, 25).unwrap();
        world.tick += 1;
        assert!(world.hold_creature(id));
        let sand = world.materials().id("sand").unwrap();
        assert!(world.paint(48, 25, 0, sand, Brush::Replace).unwrap() > 0);
        assert_eq!(world.creature(id).unwrap().fear, 0);
    }

    #[test]
    fn shy_gourami_avoids_the_cursor_from_four_body_lengths_away() {
        for shy in [false, true] {
            let mut world = shy_world(shy);
            let gourami = world.species().id("honey_gourami").unwrap();
            let id = world.add_creature(gourami, 40, 25).unwrap();
            let mut fish = world.creature(id).unwrap().clone();
            let kind = world.grown_kind(&fish).unwrap();
            world.set_creature_cursor(Some((40 + i64::from(4 * kind.w), 25)));
            (fish.tx, fish.ty, fish.retarget) = (centre_of(60), fish.y, 10);
            let mut rng = chunk_stream(world.seed, world.tick, CREATURE_STREAM);
            let (target, _, speed) = world.steering_target(&kind, &mut fish, None, &mut rng);
            assert_eq!(target < fish.x && speed == kind.top, shy);
        }
    }

    fn puffer_world() -> (World, CreatureId) {
        let mut world = World::new(
            WorldConfig {
                width: 120,
                height: 64,
                seed: 1,
                water_level: 0.95,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        world.set_easy_mode(true);
        let puffer = world.species().id("pea_puffer").unwrap();
        let id = world.add_creature(puffer, 40, 25).unwrap();
        (world, id)
    }

    #[test]
    fn a_curious_puffer_approaches_the_cursor_instead_of_fleeing() {
        let (mut world, id) = puffer_world();
        let mut fish = world.creature(id).unwrap().clone();
        let mut kind = world.grown_kind(&fish).unwrap();
        assert!(kind.curious);
        let length = i64::from(kind.w);
        let mut rng = chunk_stream(world.seed, world.tick, CREATURE_STREAM);
        for separation in [5, 2] {
            let px = 40 + separation * length;
            world.set_creature_cursor(Some((px, 25)));
            (fish.tx, fish.ty, fish.retarget) = (centre_of(20), fish.y, 10);
            let (tx, ty, speed) = world.steering_target(&kind, &mut fish, None, &mut rng);
            let stop = i32::try_from(length).unwrap() * ONE * 3 / 2;
            assert_eq!((tx, ty, speed), (centre_of(px) - stop, fish.y, kind.cruise));
        }
        kind.curious = false;
        let (tx, _, speed) = world.steering_target(&kind, &mut fish, None, &mut rng);
        assert!(tx < fish.x);
        assert_eq!(speed, kind.top);
    }

    #[test]
    fn a_curious_puffer_hovers_then_loses_interest() {
        let (mut world, id) = puffer_world();
        let kind = world.grown_kind(world.creature(id).unwrap()).unwrap();
        let px = 40 + i64::from(3 * kind.w);
        let cursor_x = centre_of(px);
        let cursor_y = centre_of(25);
        let length = i64::from(kind.w * ONE);
        world.set_creature_cursor(Some((px, 25)));
        let distance = |world: &World| {
            let fish = world.creature(id).unwrap();
            dist(fish.x - cursor_x, fish.y - cursor_y)
        };
        // Start where this fish's interest begins.
        world.tick = (1..100 * u64::from(TICKS_PER_SECOND))
            .find(|&t| {
                steering::interested(world.seed, id, t)
                    && !steering::interested(world.seed, id, t - 1)
            })
            .unwrap();
        for _ in 0..16 * TICKS_PER_SECOND {
            world.tick();
            assert!(distance(&world) > length);
        }
        assert!(distance(&world) <= 2 * length);
        let fish = world.creature(id).unwrap();
        assert!(!fish.facing_left);
        assert_eq!((fish.vx, fish.vy), (0, 0));
        for _ in 0..14 * TICKS_PER_SECOND {
            world.tick();
        }
        assert!(distance(&world) > 2 * length);
    }

    #[test]
    fn a_reloaded_curious_fish_watches_the_cursor_as_before() {
        let (mut world, id) = puffer_world();
        let kind = world.grown_kind(world.creature(id).unwrap()).unwrap();
        let cursor = (40 + i64::from(3 * kind.w), 25);
        world.set_creature_cursor(Some(cursor));
        for _ in 0..10 * TICKS_PER_SECOND {
            world.tick();
        }
        let mut again = crate::save::decode(
            &crate::save::encode(&world),
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        // The host supplies its cursor again after a load.
        again.set_creature_cursor(Some(cursor));
        for _ in 0..60 * TICKS_PER_SECOND {
            world.tick();
            again.tick();
            assert_eq!(again.state_hash(), world.state_hash());
        }
    }

    #[test]
    fn a_curious_puffer_investigates_nearby_non_food_pours() {
        for (species, material, separation, expected) in [
            ("pea_puffer", "sand", 20, true),
            ("pea_puffer", "sand", 24, true),
            ("pea_puffer", "sand", 25, false),
            ("pea_puffer", "bloodworm", 20, false),
            ("common_goldfish", "sand", 20, false),
        ] {
            let (mut world, puffer) = puffer_world();
            let id = if species == "pea_puffer" {
                puffer
            } else {
                world.remove_creature(puffer);
                let species = world.species().id(species).unwrap();
                world.add_creature(species, 40, 25).unwrap()
            };
            // Past tick 0, where `held_until` 0 still counts as held.
            world.tick += 1;
            let material = world.materials().id(material).unwrap();
            let x = 40 + separation;
            assert!(world.paint(x, 25, 0, material, Brush::Replace).unwrap() > 0);
            let fish = world.creature(id).unwrap();
            assert_eq!(fish.ty, centre_of(25));
            if expected {
                assert_eq!(fish.tx, centre_of(x));
                assert!((RETARGET_TICKS.0..RETARGET_TICKS.1).contains(&fish.retarget));
            } else {
                assert_eq!((fish.tx, fish.retarget), (fish.x, 0));
            }
        }
    }

    #[test]
    fn peaceful_tank_needs_no_near_snapshot() {
        let mut world = shy_world(false);
        let goldfish = world.species().id("common_goldfish").unwrap();
        world.add_creature(goldfish, 40, 30).unwrap();
        world.set_easy_mode(false);
        world.tick();
        assert!(world.creatures.near.is_empty());
        assert_eq!(world.creatures.near.capacity(), 0);
    }

    #[test]
    fn old_saves_with_shy_species_in_front_bands_still_load() {
        let mut world =
            World::new(WorldConfig::default(), MaterialTable::builtin().unwrap()).unwrap();
        for (name, y) in [("honey_gourami", 30), ("kuhli_loach", 50)] {
            let species = world.species().id(name).unwrap();
            world.add_creature(species, 40, y).unwrap();
        }
        let mut saved = world.save_creatures();
        for fish in &mut saved.list {
            fish.base.z = z_of(0);
            fish.base.tz = z_of(1);
        }
        world.load_creatures(saved).unwrap();
        for fish in world.creatures() {
            let front = world.kind(fish.species).unwrap().bands.0;
            assert_eq!((fish.band(), band_of(fish.tz)), (front, front));
        }
    }

    #[test]
    fn a_save_only_keeps_stray_ticks_for_schoolers() {
        let mut world =
            World::new(WorldConfig::default(), MaterialTable::builtin().unwrap()).unwrap();
        let goldfish = world.species().id("common_goldfish").unwrap();
        let tetra = world.species().id("neon_tetra").unwrap();
        let goldfish = world.add_creature(goldfish, 40, 30).unwrap();
        let tetra = world.add_creature(tetra, 60, 30).unwrap();
        let mut saved = world.save_creatures();
        for c in &mut saved.list {
            c.base.stray = 20;
        }
        world.load_creatures(saved).unwrap();
        assert_eq!(world.creature(goldfish).unwrap().stray, 0);
        assert_eq!(world.creature(tetra).unwrap().stray, 20);
    }

    #[test]
    fn dispersed_shoalers_draw_closer_than_peaceful_fish() {
        let mean_gap = |seed, temperament| {
            let mut world = World::new(
                WorldConfig {
                    width: 360,
                    height: 100,
                    seed,
                    water_level: 0.95,
                },
                MaterialTable::builtin().unwrap(),
            )
            .unwrap();
            let source = include_str!("../../../../assets/species/guppy.toml");
            let source = source.replace("temperament = \"shoaling\"", temperament);
            let species = SpeciesTable::from_toml_sources(&[&source], &world.materials).unwrap();
            let guppy = species.id("guppy").unwrap();
            world.creatures = Creatures::new(
                species,
                &world.materials,
                world.life.eco.algae.as_deref(),
                world.creatures.water,
                world.creatures.air,
            );
            for index in 0..8 {
                world.add_creature(guppy, 20 + 45 * index, 35).unwrap();
            }
            let mut sum = 0;
            for tick in 0..(3 * 60 * TICKS_PER_SECOND) {
                world.tick();
                if tick >= 2 * 60 * TICKS_PER_SECOND && tick.is_multiple_of(TICKS_PER_SECOND) {
                    let fish = world.creatures();
                    assert_eq!(fish.len(), 8);
                    sum += fish
                        .iter()
                        .map(|c| {
                            fish.iter()
                                .filter(|m| m.id != c.id)
                                .map(|m| dist(m.x - c.x, m.y - c.y))
                                .min()
                                .unwrap()
                        })
                        .sum::<i64>();
                }
            }
            sum / (60 * 8)
        };

        // Each seed draws closer, and over all of them by a fifth or more.
        let (mut shoals, mut roams) = (0, 0);
        for seed in [4, 5, 9, 13, 21, 77] {
            let peaceful = mean_gap(seed, "temperament = \"peaceful\"");
            let shoaling = mean_gap(seed, "temperament = \"shoaling\"");
            assert!(
                shoaling < peaceful,
                "seed {seed}, shoaling {shoaling}, peaceful {peaceful}"
            );
            (shoals, roams) = (shoals + shoaling, roams + peaceful);
        }
        assert!(
            shoals * 5 < roams * 4,
            "shoaling {shoals}, peaceful {roams}"
        );
    }

    #[test]
    fn glass_grazing_reaches_the_nearby_wall_or_pane() {
        use crate::world::algae::GlassPane;

        let mut w = World::new(
            WorldConfig {
                width: 32,
                height: 32,
                water_level: 1.0,
                ..WorldConfig::default()
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let film = w.algae_variety("green_film").unwrap();
        let block = 5; // Field column 1, row 1.
        for (pane, fx, fy) in [
            (GlassPane::Front, 1, 1),
            (GlassPane::Back, 1, 1),
            (GlassPane::Left, 0, 1),
            (GlassPane::Right, 3, 1),
        ] {
            assert!(w.set_algae_glass(pane, fx, fy, film, 5));
        }
        let nerite = w.species().id("nerite").unwrap();
        let id = w.add_creature(nerite, 3, 16).unwrap();
        let mut c = w.creature(id).unwrap().clone();
        let k = *w.kind(nerite).unwrap();
        let centre = Rect {
            x0: 12,
            y0: 12,
            x1: 14,
            y1: 14,
        };
        c.z = z_of(3);
        assert!(!w.graze_algae(&k, centre, &mut c));
        for (band, front) in [(0, true), (DEPTH_BANDS - 1, false)] {
            let before = if front {
                w.front_glass_algae()[block]
            } else {
                w.back_glass_algae()[block]
            };
            assert!(before & 15 >= FILM_BITE);
            c.z = z_of(band);
            assert!(w.graze_algae(&k, centre, &mut c));
            let after = if front {
                w.front_glass_algae()[block]
            } else {
                w.back_glass_algae()[block]
            };
            assert_eq!((before & 15) - (after & 15), FILM_BITE);
            assert_eq!(after >> 4, film, "the variety stays while any is left");
        }
        c.z = z_of(3);
        for (i, (x0, x1)) in [(0, 2), (30, 32)].into_iter().enumerate() {
            let before = w.side_glass_algae()[1][i];
            assert!(before & 15 >= FILM_BITE);
            assert!(w.graze_algae(&k, Rect { x0, x1, ..centre }, &mut c));
            assert_eq!((before & 15) - (w.side_glass_algae()[1][i] & 15), FILM_BITE);
        }
        assert_eq!(c.eaten(), 4);
    }

    #[test]
    fn only_glass_film_of_a_variety_it_grazes_is_food() {
        use crate::world::algae::GlassPane;
        let mut w = World::new(
            WorldConfig {
                width: 32,
                height: 32,
                water_level: 1.0,
                ..WorldConfig::default()
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let oto = *w.kind(w.species().id("otocinclus").unwrap()).unwrap();
        let spot = w.algae_variety("green_spot").unwrap();
        let diatoms = w.algae_variety("diatoms").unwrap();
        assert!(!w.has_film_to_graze(&oto));
        // Green spot on the front pane: otocinclus won't touch it.
        assert!(w.set_algae_glass(GlassPane::Front, 1, 1, spot, 3));
        assert!(!w.has_film_to_graze(&oto));
        assert!(!w.anything_to_eat(&oto));
        // Diatoms on the back wall are food.
        assert!(w.set_algae_glass(GlassPane::Back, 2, 1, diatoms, 3));
        assert!(w.has_film_to_graze(&oto));
        // And stop being food once cleared.
        assert!(w.set_algae_glass(GlassPane::Back, 2, 1, diatoms, 0));
        assert!(!w.has_film_to_graze(&oto));
    }

    #[test]
    fn grazers_take_each_algae_variety_as_well_as_their_diet_says() {
        let mut w = World::new(
            WorldConfig {
                width: 32,
                height: 32,
                water_level: 1.0,
                ..WorldConfig::default()
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let rock = w.materials().id("rock").unwrap();
        let ledge = Rect {
            x0: 8,
            y0: 20,
            x1: 16,
            y1: 24,
        };
        w.fill_rect(ledge, rock, crate::Brush::Replace).unwrap();
        let reach = Rect {
            x0: 9,
            y0: 19,
            x1: 12,
            y1: 22,
        };
        let varieties = ["diatoms", "green_film", "green_dust", "green_spot"];
        // The density each grazer takes in one bite of each variety (0: it
        // won't graze it), and whether the bite is a full one.
        let table: [(&str, [u8; 4]); 5] = [
            ("otocinclus", [4, 3, 0, 0]),
            ("amano_shrimp", [4, 3, 0, 0]),
            ("nerite", [4, 4, 1, 1]),
            ("bristlenose_pleco", [4, 4, 1, 0]),
            ("home_owning_crab", [4, 4, 1, 0]),
        ];
        for (name, takes) in table {
            let species = w.species().id(name).unwrap();
            let k = *w.kind(species).unwrap();
            let creature = w.add_creature(species, 24, 27).unwrap();
            for (variety, take) in varieties.into_iter().zip(takes) {
                let id = w.algae_variety(variety).unwrap();
                assert!(w.set_algae_variety(10, 20, id, 12), "{variety}");
                let mut c = w.creature(creature).unwrap().clone();
                c.satiety = 0;
                let eaten = w.graze_algae(&k, reach, &mut c);
                let left = w.coat()[20 * 32 + 10];
                assert_eq!(eaten, take != 0, "{name} on {variety}");
                assert_eq!(left & 15, 12 - take, "{name} on {variety}");
                if take != 0 {
                    assert_eq!(left >> 4, id);
                    // A small bite feeds less than a full one.
                    let full = k.graze[usize::from(id)] == 100;
                    assert_eq!(c.satiety == BITE, full, "{name} on {variety}");
                }
                w.set_algae_variety(10, 20, id, 0);
            }
            w.remove_creature(creature);
        }
    }

    fn dimorphic_tank(female_art: &str) -> World {
        let source = r##"
            id = 0
            name = "test_fish"
            scientific_name = "Piscis fictus"
            description = "A small fish for creature tests."
            zone = "mid"
            depth = [0.2, 0.6]
            min_depth = 2
            temperament = "peaceful"
            length_cm = 0.5
            lifespan_days = 100
            maturity_days = 10
            speed = 2.0
            max_speed = 4.0
            diet = []
            [sprite]
            colours = { a = "#102030" }
            frames = [["aaaa", "aaaa"]]
            fine_frames = [["aaaaaaaa", "aaaaaaaa", "aaaaaaaa", "aaaaaaaa"]]
            icon = ["a"]
        "##;
        let mut world = World::new(
            WorldConfig {
                width: 40,
                height: 32,
                ..WorldConfig::default()
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let art = if female_art.is_empty() {
            source.to_owned()
        } else {
            format!(
                "{}\n{female_art}",
                source.replace(
                    "length_cm = 0.5",
                    "length_cm = 0.5\nfemale_length_cm = 1.25"
                )
            )
        };
        let species = SpeciesTable::from_toml_sources(&[&art], &world.materials).unwrap();
        world.creatures = Creatures::new(
            species,
            &world.materials,
            world.life.eco.algae.as_deref(),
            world.creatures.water,
            world.creatures.air,
        );
        world
    }

    const BIGGER_FEMALE_ART: &str = r#"
        female_frames = [["aaaaaa", "aaaaaa", "aaaaaa", "aaaaaa"]]
        female_fine_frames = [["aaaaaaaaaaaa", "aaaaaaaaaaaa", "aaaaaaaaaaaa", "aaaaaaaaaaaa",
                               "aaaaaaaaaaaa", "aaaaaaaaaaaa", "aaaaaaaaaaaa", "aaaaaaaaaaaa"]]
    "#;

    #[test]
    fn individuals_use_their_sexes_size_and_length() {
        let mut world = dimorphic_tank(BIGGER_FEMALE_ART);
        let species = SpeciesId(0);
        let male = world.add_creature(species, 10, 15).unwrap();
        let female = world.add_creature(species, 20, 15).unwrap();
        let male_creature = world.creature(male).unwrap();
        let female_creature = world.creature(female).unwrap();
        assert_eq!(world.creature_size(male_creature), (2, 1));
        assert_eq!(world.creature_size(female_creature), (3, 2));
        assert_eq!(
            (
                world.length_mm(male_creature),
                world.length_mm(female_creature)
            ),
            (5, 13)
        );
        assert_eq!(world.creature_at(20, 16), Some(female));
        assert!(world.move_creature(male, 10, 31));
        assert!(!world.move_creature(female, 20, 31));
        let corpse = world.corpse_of(world.creature(female).unwrap(), DeathCause::OldAge);
        assert!(corpse.female());
        assert_eq!(corpse.size(), (3, 2));
        world.bury([corpse]);
        world.load_creatures(world.save_creatures()).unwrap();
        assert_eq!(world.corpses()[0].size(), (3, 2));
    }

    #[test]
    fn female_outgrows_saved_position_and_moves_clear_on_tick() {
        for rock in [false, true] {
            let prepare = |art| {
                let mut world = dimorphic_tank(art);
                if rock {
                    let rock = world.materials().id("rock").unwrap();
                    world.set_cell(10, 17, rock).unwrap();
                    let body = world.body_at(10, 17).unwrap();
                    world.set_anchored(body, true);
                }
                world
            };
            let pos = if rock { (10, 16) } else { (10, 31) };
            let mut old = prepare("");
            let female = old
                .add_creature_with_traits(
                    SpeciesId(0),
                    pos.0,
                    pos.1,
                    Traits {
                        female: Some(true),
                        ..Traits::default()
                    },
                )
                .unwrap();
            let before = old.creature(female).unwrap();
            let before = (before.centre(), before.band());
            let mut current = prepare(BIGGER_FEMALE_ART);
            current.load_creatures(old.save_creatures()).unwrap();
            let loaded = current.creature(female).unwrap();
            let k = current.grown_kind(loaded).unwrap();
            assert_eq!((loaded.centre(), loaded.band()), before);
            assert!(!current.fits(&k, loaded.turn, loaded.x, loaded.y, loaded.band()));
            current.tick();
            let after = current.creature(female).unwrap();
            assert!(current.fits(&k, after.turn, after.x, after.y, after.band()));
            assert_ne!((after.centre(), after.band()), before);
        }
    }

    fn planted(seed: u64) -> World {
        let mut w = World::new(
            WorldConfig {
                width: 120,
                height: 80,
                seed,
                water_level: 0.95,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let soil = w.materials().id("aquasoil").unwrap();
        w.fill_rect(
            Rect {
                x0: 0,
                y0: 72,
                x1: 120,
                y1: 80,
            },
            soil,
            Brush::Replace,
        )
        .unwrap();
        let cutting = w.materials().id("cutting").unwrap();
        for x in (20..100).step_by(10) {
            w.set_cell(x, 71, cutting).unwrap();
        }
        for _ in 0..6000 {
            w.tick();
        }
        w
    }

    #[test]
    fn creatures_keep_to_their_species_bands() {
        let mut w = planted(3);
        let tetra = w.species().id("neon_tetra").unwrap();
        w.creatures.kinds[usize::from(tetra.0)]
            .as_mut()
            .unwrap()
            .bands = (2, 3);
        let ids: Vec<_> = [(15, 40), (60, 30), (100, 50)]
            .into_iter()
            .map(|(x, y)| w.add_creature(tetra, x, y).unwrap())
            .collect();
        let nerite = w.species().id("nerite").unwrap();
        let snail = w.add_creature(nerite, 60, 70).unwrap();
        let mut band = w.creature(snail).unwrap().band();
        let mut seen = [false; DEPTH_BANDS as usize];
        for _ in 0..4000 {
            let before = w.creature(snail).unwrap().clone();
            w.tick();
            for &id in &ids {
                let c = w.creature(id).unwrap();
                assert!((2..=3).contains(&c.band()), "band {}", c.band());
                assert!((2.0..=3.0).contains(&c.depth()), "depth {}", c.depth());
                seen[usize::from(c.band())] = true;
            }
            // A crawler keeps its band, unless a growing plant buries it or
            // it heads for a pane (#371).
            let now = w.creature(snail).unwrap().band();
            let to_pane = before.tz == z_of(0) || before.tz == z_of(DEPTH_BANDS - 1);
            if now != band && !to_pane {
                let k = w.grown_kind(&before).unwrap();
                assert!(
                    !w.fits(&k, before.turn, before.x, before.y, band),
                    "left band {band} for {now} unburied"
                );
                band = now;
            }
        }
        assert!(seen[2] && seen[3], "they wander between their bands");
    }

    /// Deepest depth, 1/256 band.
    const MAX_Z: i32 = (DEPTH_BANDS as i32 - 1) * ONE;

    #[test]
    fn bands_round_and_default_within_range() {
        assert_eq!(band_of(0), 0);
        assert_eq!(band_of(ONE / 2 - 1), 0);
        assert_eq!(band_of(ONE / 2), 1);
        assert_eq!(band_of(MAX_Z), DEPTH_BANDS - 1);
        assert_eq!(z_of(DEPTH_BANDS), MAX_Z);
        let mut seen = [false; DEPTH_BANDS as usize];
        for id in 1..200 {
            let b = default_band(7, id, (0, DEPTH_BANDS - 1));
            seen[usize::from(b)] = true;
            assert_eq!(b, default_band(7, id, (0, DEPTH_BANDS - 1)));
            assert!((2..=4).contains(&default_band(7, id, (2, 4))));
        }
        assert_eq!(seen, [true; DEPTH_BANDS as usize]);
    }

    #[test]
    fn food_next_to_a_cell_given_up_on_is_skipped_too() {
        let mut w = World::new(
            WorldConfig {
                width: 80,
                height: 40,
                seed: 1,
                water_level: 0.95,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let flake = w.materials().id("flake_food").unwrap();
        // A clump of flakes near the fish, one further off.
        for (x, y) in [(30, 20), (31, 20), (32, 20), (33, 20), (50, 20)] {
            w.set_cell(x, y, flake).unwrap();
        }
        let guppy = w.species().id("guppy").unwrap();
        let id = w.add_creature(guppy, 25, 20).unwrap();
        // Registers the food (it may drift a cell).
        w.tick();
        let mut c = w.creature(id).unwrap().clone();
        let k = *w.kind(guppy).unwrap();
        let xy = |i: u32| (i as usize % w.width, i as usize / w.width);
        let near = w.nearest_food(&k, &c).unwrap();
        assert!(xy(near).0 < 40, "a near flake first: {:?}", xy(near));
        // Blocked from one, it doesn't flip to the next in the clump.
        c.ignore = Some(near);
        let next = w.nearest_food(&k, &c).unwrap();
        assert!(xy(next).0 > 40, "the far flake next: {:?}", xy(next));
    }

    #[test]
    fn blocked_surface_food_does_not_crowd_out_loose_food_before_retry() {
        let mut w = World::new(
            WorldConfig {
                width: 80,
                height: 40,
                seed: 1,
                water_level: 0.8,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let leaf = w.materials().id("floating_leaf").unwrap();
        let flake = w.materials().id("flake_food").unwrap();
        let surf = w.surface_row;
        w.set_cell(30, surf, leaf).unwrap();
        w.set_cell(50, surf + 12, flake).unwrap();
        w.tick(); // Register the food cells.
        let guppy = w.species().id("guppy").unwrap();
        let id = w.add_creature(guppy, 30, to_i64(surf + 8)).unwrap();
        let k = *w.kind(guppy).unwrap();
        let mut c = w.creature(id).unwrap().clone();
        let nearest = w.nearest_food(&k, &c).unwrap();
        assert!(w.is_floating_leaf(nearest));

        // The previous target is gone and the next leaf is far enough from
        // it to pass the ordinary local ignore filter.
        c.ignore = Some(u32::try_from((surf - 1) * w.width + 24).unwrap());
        w.tick = SEARCH_EVERY - u64::from(id.0);
        w.choose_food(&k, &mut c, true);
        let loose = c.food.unwrap();
        assert_eq!(w.cells[loose as usize].material, flake);

        c.food = None;
        w.tick = FLOAT_RETRY_EVERY - u64::from(id.0);
        w.choose_food(&k, &mut c, true);
        assert_eq!(c.food, Some(nearest));
    }

    #[test]
    fn swimming_to_a_surface_leaf_targets_a_body_position_in_biting_reach() {
        let mut w = World::new(
            WorldConfig {
                width: 80,
                height: 40,
                seed: 1,
                water_level: 0.8,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let leaf = w.materials().id("floating_leaf").unwrap();
        let surf = w.surface_row;
        w.set_cell(30, surf - 1, leaf).unwrap();
        let guppy = w.species().id("guppy").unwrap();
        let id = w.add_creature(guppy, 30, to_i64(surf + 8)).unwrap();
        let k = *w.kind(guppy).unwrap();
        let mut c = w.creature(id).unwrap().clone();
        c.food = Some(u32::try_from((surf - 1) * w.width + 30).unwrap());
        let mut rng = chunk_stream(w.seed, w.tick, CREATURE_STREAM + u64::from(id.0));
        let (tx, ty, _) = w.steering_target(&k, &mut c, None, &mut rng);
        assert!(w.fits(&k, c.turn, tx, ty, c.band()));
        (c.x, c.y) = (tx, ty);
        assert!(w.eat(&k, &mut c));
        assert_eq!(c.eaten(), 1);
    }

    #[test]
    fn only_crawlers_hold_a_floaters_roots() {
        let mut w = World::new(
            WorldConfig {
                width: 60,
                height: 40,
                seed: 2,
                water_level: 0.9,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let duckweed = w.plant_species().id("duckweed").unwrap();
        let top = i64::try_from(w.surface_row()).unwrap();
        w.paint_plant(30, top + 2, 0, duckweed, crate::Propagule::Buds)
            .unwrap();
        for _ in 0..3000 {
            w.tick();
        }
        let root = w.materials().id("root").unwrap();
        let i = (0..w.cells.len())
            .find(|&i| w.cells[i].material == root)
            .expect("the duckweed hangs a root");
        let band = w.plant_band_at(i).unwrap();
        let (x, y) = (to_i64(i % w.width), to_i64(i / w.width));
        // One-cell bodies, so only the root cell is tested.
        let body = |name: &str| {
            let mut kind = *w.kind(w.species().id(name).unwrap()).unwrap();
            (kind.w, kind.h) = (1, 1);
            kind
        };
        let (guppy, snail) = (body("guppy"), body("ramshorn_snail"));
        assert!(w.fits(&guppy, 0, centre_of(x), centre_of(y), band));
        assert!(!w.fits(&snail, 0, centre_of(x), centre_of(y), band));
        assert!(w.blocks(i, Some(band)), "a crawler holds it");
    }
}
