//! Creatures (SPEC §2.4) steer in fixed point, collide with the cell grid
//! and eat within reach of their bodies. Species data selects schooling,
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
//!   within [`BAND_REACH`] of its depth band (below). If the world moves a
//!   solid into a creature (a pour, a growing plant) it shifts to a free
//!   band where it is, or else hops to the nearest free spot.
//! - **Depth** (2.5D, D21). The tank is [`DEPTH_BANDS`] bands deep, 0 at
//!   the front glass. Each plant cell has a band ([`World::plant_band`]);
//!   everything else (rock, substrate, glass, bodies, air) spans every
//!   band, and so do food and other loose particles, which a creature eats
//!   from any band. A creature has a depth in 1/256 band (hashed and
//!   saved) that eases towards a target band: a new random one in its
//!   species' `bands` with each wander target, or, when a plant blocks its
//!   way, the nearest band it can shift to in place where the way is clear
//!   (front first for even ids, back first for odd), holding still while
//!   it shifts, so it slips past the plant rather than turning away; it
//!   turns away if that takes over [`DODGE_TICKS`]. A schooler that can't
//!   reach its school at all strays on a wander target of its own for a
//!   while, out of whatever pocket holds it. Crawlers keep the band they
//!   start in (unless buried, when any creature may shift band to get
//!   free), so they climb only the plants in it. A corpse sinks in
//!   the band it died in.
//! - **Walkers** keep to the substrate, stepping up one-row ledges; a
//!   species with `hop_every` (shrimp) kicks off it now and then and sinks
//!   back a few cells on.
//! - **Attachment.** A surface crawler turns in quarter turns so its foot
//!   (the sprite's bottom row) rests on the face it holds: the floor
//!   upright, a wall on its side, a ceiling upside down. Its body box turns
//!   with it, so a 5×3 snail on a wall covers 3×5 cells. The turn is
//!   creature state (hashed); swimmers and walkers stay upright.
//! - **Randomness** comes from a PCG stream per creature per tick, derived
//!   from `(seed, tick, creature id)`, so it never depends on update order.
//! - **Cost.** Creatures are updated every tick, after the cells, and wake
//!   a chunk only when they eat (SPEC §2.1). Food is found through a
//!   registry of food cells kept like the living lists, not by scanning.
//! - **Life cycle** (T9): age, stages, growth with feeding, deaths and
//!   corpses are in [`lifecycle`]; sexes, breeding, eggs, colour genetics
//!   and predation in [`breeding`].
//! - **Habits** of a few species (waste, eating corpses, a home with a
//!   carpet patch) are in [`habits`].

mod breeding;
mod habits;
mod lifecycle;
mod steering;

use core::mem;

use rand_core::Rng as _;
use rand_pcg::Pcg32;

use super::plants::PlantDepth;
use super::{TICKS_PER_SECOND, World, WorldError};
use crate::grid::Rect;
use crate::hash::Fnv64;
use crate::material::{MaterialId, MaterialTable, MovementClass};
use crate::rng::chunk_stream;
use crate::save::{SaveError, v5};
use crate::species::{SpeciesId, SpeciesTable, Temperament, Zone};
pub use breeding::{
    BREED_CHECK_SECONDS, CONCEIVE_ODDS, Clutch, MAX_CLUTCHES, MUTATION_PER_MILLE, PREY_ENERGY,
    REVERSION_PER_MILLE, SETTLE_TICKS, STRIKE_ODDS,
};
use habits::{home_span, near_home};
pub use lifecycle::{
    BITE_ENERGY, CORPSE_SINK, Corpse, DeathCause, ENERGY_RESERVE, LIFE_DAY_SECONDS, LIFE_DAY_TICKS,
    LifeStage, MAX_CORPSES, POOR_QUALITY, ROT_AFTER_SECONDS, ROT_EVERY_SECONDS, STRESS_LIMIT,
};

/// The default population cap per display (SPEC §2.4), which protects the
/// budget; [`World::set_creature_cap`] changes it.
pub const DEFAULT_CREATURE_CAP: usize = 60;
/// Highest population cap a tank may set (with [`MAX_CORPSES`], within the
/// renderer's 256 sprites).
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
/// How far a creature notices food, in cells.
const FOOD_SENSE: i64 = 160;
/// How far from where it is asked for a creature is added, in cells.
const PLACE_RADIUS: i32 = 8;
/// How far a creature hops to get out of a solid, in cells: twice
/// [`PLACE_RADIUS`], so a big walker caught by sand raining onto a mound
/// still finds room (the search runs only while it is buried).
const UNSTUCK_RADIUS: i32 = 16;
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
    /// It breathes air but the tank is full to the top.
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
    /// home and planted its carpet patch on (see [`habits`]).
    home: Option<(u32, u32)>,
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
    /// (a species with a `home_carpet`) and has picked it.
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
    /// Body size in cells.
    w: i32,
    h: i32,
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
    zone: Zone,
    breathes_air: bool,
    /// Cells past the body it eats from.
    reach: usize,
    /// Ticks between a walker's hops (0: it never hops).
    hop_every: u64,
    /// Ticks of growth to maturity, and the species' lifespan in ticks.
    maturity: u32,
    lifespan: u32,
    /// Adult body length, and the longest prey it eats (0: none), in mm.
    length_mm: u32,
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
}

/// The creatures in a world and the per-species and per-material lookups
/// their update uses.
#[derive(Clone, Debug)]
pub(super) struct Creatures {
    species: SpeciesTable,
    list: Vec<Creature>,
    /// Cell-space input from a host with avoidance enabled.
    cursor: Option<(i32, i32)>,
    /// Reused start-of-tick snapshots for schooling, in creature id order.
    school: Vec<Schoolmate>,
    next_id: u32,
    kinds: Vec<Option<Kind>>,
    /// Per material: a creature's body may overlap it.
    swim: Vec<bool>,
    water: MaterialId,
    air: MaterialId,
    detritus: Option<MaterialId>,
    waste: Option<MaterialId>,
    /// Bumped whenever a creature or corpse is added, removed, moves or
    /// turns, or a corpse fades.
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
    species: SpeciesId,
    x: i32,
    y: i32,
    vx: i32,
    vy: i32,
}

impl Creatures {
    pub(super) fn new(
        species: SpeciesTable,
        materials: &MaterialTable,
        water: MaterialId,
        air: MaterialId,
    ) -> Self {
        let mut swim = vec![false; 256];
        for m in materials.iter() {
            swim[usize::from(m.id.0)] = match m.class {
                MovementClass::Liquid => m.density >= WATER_DENSITY,
                MovementClass::Buoyant => true,
                _ => m.passable,
            };
        }
        let mut kinds = vec![None; 256];
        for s in species.iter() {
            let (w, h) = s.size();
            let per_tick = |cells_per_s: f32| {
                // Validated to (0, 16] cells per second: under a cell per tick.
                #[allow(clippy::cast_possible_truncation)]
                let v = (f64::from(cells_per_s) * f64::from(ONE) / f64::from(TICKS_PER_SECOND))
                    .round() as i32;
                v.max(1)
            };
            let mut edible = [false; 256];
            for m in &s.diet {
                edible[usize::from(m.0)] = true;
            }
            let (cruise, top) = (per_tick(s.speed), per_tick(s.max_speed));
            kinds[usize::from(s.id.0)] = Some(Kind {
                // Sprite sides are at most 16.
                w: i32::try_from(w).unwrap_or(1),
                h: i32::try_from(h).unwrap_or(1),
                cruise,
                top,
                accel: (top / 6).max(2),
                depth: s.depth,
                bands: (s.bands[0], s.bands[1]),
                schooling: s.temperament == Temperament::Schooling,
                zone: s.zone,
                breathes_air: s.breathes_air,
                reach: usize::from(s.reach),
                // Validated to 1..=600 s.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                hop_every: s.hop_every.map_or(0, |every| {
                    (f64::from(every) * f64::from(TICKS_PER_SECOND)).round() as u64
                }),
                // Validated to at most 7300 days: far inside u32.
                maturity: s.maturity_days.saturating_mul(LIFE_DAY_TICKS),
                lifespan: s.lifespan_days.saturating_mul(LIFE_DAY_TICKS),
                // Validated lengths fit the sprite: at most 8 cm.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                length_mm: (s.length_cm * 10.0).round() as u32,
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
            });
        }
        Self {
            species,
            list: Vec::new(),
            cursor: None,
            school: Vec::new(),
            next_id: 1,
            kinds,
            swim,
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
        water: MaterialId,
        air: MaterialId,
    ) -> Result<(Self, Vec<MaterialId>), WorldError> {
        let species =
            SpeciesTable::builtin_for(materials).map_err(|e| WorldError::Species(e.to_string()))?;
        let foods = Self::foods(&species);
        Ok((Self::new(species, materials, water, air), foods))
    }

    /// The materials any species eats.
    pub(super) fn foods(species: &SpeciesTable) -> Vec<MaterialId> {
        let mut foods: Vec<MaterialId> = species.iter().flat_map(|s| s.diet.clone()).collect();
        foods.sort_unstable();
        foods.dedup();
        foods
    }
}

impl World {
    /// Sets the pointer cell fish avoid. A missing or out-of-tank pointer
    /// disables avoidance. Hosts supply this input only when enabled.
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

    /// Bumped whenever a creature is added, removed, moves or turns, so a
    /// renderer redraws only then.
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
        if self.creatures.list.len() >= self.creatures.cap {
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
            .map(|c| c.held_until = until)
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
        (s.breathes_air && self.surface_row == 0).then_some(UnmetNeed::Air)
    }

    /// The creatures, corpses and life settings as saved (T8, T9). The
    /// host's cursor isn't saved.
    pub(super) fn save_creatures(&self) -> v5::Creatures {
        let cr = &self.creatures;
        v5::Creatures {
            next_id: cr.next_id,
            cap: u32::try_from(cr.cap).unwrap_or(u32::MAX),
            easy: cr.easy,
            list: cr
                .list
                .iter()
                .map(|c| v5::Creature {
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
                })
                .collect(),
            corpses: cr
                .corpses
                .iter()
                .map(|c| v5::Corpse {
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
                .map(|c| v5::Clutch {
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
    pub(super) fn load_creatures(&mut self, saved: v5::Creatures) -> Result<(), SaveError> {
        let (w, h) = (
            to_i64(self.width) * i64::from(ONE),
            to_i64(self.height) * i64::from(ONE),
        );
        let inside =
            |x: i32, y: i32| (0..=w).contains(&i64::from(x)) && (0..=h).contains(&i64::from(y));
        let cap = saved.cap as usize;
        if !(1..=MAX_CREATURE_CAP).contains(&cap) || saved.list.len() > cap {
            return Err(SaveError::Invalid(format!(
                "{} creatures with a cap of {cap}",
                saved.list.len()
            )));
        }
        let mut list = Vec::with_capacity(saved.list.len());
        for c in saved.list {
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
            // Within its species' bands (as every move keeps it).
            let deep = |z: i32| (z_of(k.bands.0)..=z_of(k.bands.1)).contains(&z);
            if !(deep(c.z)
                && deep(c.tz)
                && (k.schooling || c.stray == 0)
                && c.tz % ONE == 0
                && c.dodging <= DODGE_TICKS
                && u32::from(c.stray) < RETARGET_TICKS.1)
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
            if !self.home_fits(k, c.home) {
                return Err(invalid(
                    "home outside the grid or for a species without one",
                ));
            }
            list.push(Creature {
                id: CreatureId(c.id),
                species,
                x: c.x,
                y: c.y,
                z: c.z,
                vx: c.vx,
                vy: c.vy,
                tx: c.tx,
                ty: c.ty,
                tz: c.tz,
                dodging: c.dodging,
                stray: c.stray,
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
    fn load_corpses(&self, saved: Vec<v5::Corpse>) -> Result<Vec<Corpse>, SaveError> {
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
            if self.kind(species).is_none() {
                return Err(invalid("unknown species"));
            }
            let cause = DeathCause::from_code(c.cause).ok_or_else(|| invalid("unknown cause"))?;
            // As `corpse_of` makes them: at most the adult body, and a
            // third of its cells to release.
            let (full_w, full_h, morphs) = self
                .kind(species)
                .map_or((0, 0, 0), |k| (k.w, k.h, k.morphs));
            let fits = |side: u8, full: i32| (1..=full).contains(&i32::from(side));
            let total = (u16::from(c.w) * u16::from(c.h)).div_ceil(3).max(1);
            if !inside(c.x, c.y)
                || !fits(c.w, full_w)
                || !fits(c.h, full_h)
                || c.total != total
                || c.released >= c.total
                || c.band >= DEPTH_BANDS
                || c.morph >= morphs
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
            // Only home owners have one, so other tanks hash as before.
            if let Some((x, y)) = c.home {
                h.write(&x.to_le_bytes());
                h.write(&y.to_le_bytes());
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
        list.retain_mut(|c| {
            if let Some(cause) = self.live(c, quality, &mut changed) {
                dead.push(self.corpse_of(c, cause));
                return false;
            }
            self.breed(c, &mut breeding);
            true
        });
        if !dead.is_empty() {
            self.bury(dead);
            changed = true;
        }
        // Predators hunt once a second, with the water-quality sample.
        if quality.is_some() {
            changed |= self.predation(&mut list);
        }
        self.creatures.school.clear();
        for c in &list {
            if self.kind(c.species).is_some_and(|k| k.schooling) && c.held_until < self.tick {
                self.creatures.school.push(Schoolmate {
                    id: c.id,
                    species: c.species,
                    x: c.x,
                    y: c.y,
                    vx: c.vx,
                    vy: c.vy,
                });
            }
        }
        for c in &mut list {
            changed |= self.update_creature(c);
        }
        self.creatures.list = list;
        changed |= self.finish_breeding(breeding);
        if !self.creatures.corpses.is_empty() {
            changed |= self.update_corpses(self.creatures.detritus);
        }
        if changed {
            self.creatures.version += 1;
        }
    }

    /// One tick of one creature. Returns whether it moved or turned.
    fn update_creature(&mut self, c: &mut Creature) -> bool {
        let Some(k) = self.grown_kind(c) else {
            return false;
        };
        let before = (c.x, c.y, c.facing_left, c.turn);
        if c.held_until >= self.tick {
            return false;
        }
        // The world moved something solid into it: shift to a free band
        // where it is, or hop out.
        if !self.fits(&k, c.turn, c.x, c.y, c.band()) {
            if let Some(b) = self.free_band(&k, c.turn, c.x, c.y, c.band()) {
                (c.z, c.tz) = (z_of(b), z_of(b));
            } else if let Some((x, y)) =
                self.nearest_fit(&k, c.turn, c.x, c.y, c.band(), UNSTUCK_RADIUS)
            {
                (c.x, c.y, c.vx, c.vy) = (x, y, 0, 0);
                c.retarget = 0;
            }
            return (c.x, c.y) != (before.0, before.1);
        }
        let mut rng = chunk_stream(self.seed, self.tick, CREATURE_STREAM + u64::from(c.id.0));
        c.satiety = c.satiety.saturating_sub(1);
        let hungry = c.hungry();
        self.habits(&k, c, &mut rng);
        self.shift_depth(&k, c);
        if k.zone == Zone::Surfaces {
            self.crawl(&k, c);
            if hungry && self.anything_to_eat(&k) {
                self.eat(&k, c);
            }
            return (c.x, c.y, c.facing_left, c.turn) != before;
        }
        self.choose_food(&k, c, hungry);

        let (tx, ty, speed) = self.steering_target(&k, c, &mut rng);
        steer(&k, c, tx, ty, speed, speed == k.top);
        // Walkers keep a downward bias even on long, nearly horizontal
        // routes. A one-row step lets them follow a sloping sand bed.
        // Hoppers (shrimp) kick upwards now and then and sink back.
        if k.zone == Zone::Bottom {
            c.vy = if self.hopping(&k, c) {
                -k.top
            } else {
                k.cruise
            };
        }

        // Move, sliding along whatever blocks one axis; a plant in the way
        // sends it to another band.
        let mut moved = self.try_move(&k, c);
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
        if hungry && self.anything_to_eat(&k) && self.eat(&k, c) {
            c.food = None;
        } else if moved != Move::Moved {
            (c.vx, c.vy) = (0, 0);
            if moved == Move::Blocked {
                if let Some(f) = c.food.take() {
                    c.ignore = Some(f);
                }
                self.pick_target(&k, c, &mut rng);
                // A schooler that can't reach its school strays for a while
                // on its new target, out of whatever pocket holds it.
                if k.schooling {
                    c.stray = u16::try_from(c.retarget).unwrap_or(0);
                }
            }
        }
        if c.vx > TURN_SPEED {
            c.facing_left = false;
        } else if c.vx < -TURN_SPEED {
            c.facing_left = true;
        }
        (c.x, c.y, c.facing_left, c.turn) != before
    }

    /// Whether a hopping walker is in the kick of a hop this tick. Each
    /// creature's hops are offset by its id, so a group doesn't hop as one.
    /// A kick ends early (or never starts) where the rest of it would lift
    /// the walker above its zone, so a hop never leaves its depth band.
    fn hopping(&self, k: &Kind, c: &Creature) -> bool {
        if k.hop_every == 0 {
            return false;
        }
        let phase = (self.tick + u64::from(c.id.0) * 7919) % k.hop_every;
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

    /// Keeps `c` heading for food while hungry: a new search when its food
    /// has gone, and every [`SEARCH_EVERY`] ticks for something closer.
    fn choose_food(&self, k: &Kind, c: &mut Creature, hungry: bool) {
        if !hungry || self.life.food_cells() == 0 {
            c.food = None;
            return;
        }
        let lost = c
            .food
            .is_some_and(|i| !k.edible[usize::from(self.cells[i as usize].material.0)]);
        if lost || (self.tick + u64::from(c.id.0)).is_multiple_of(SEARCH_EVERY) {
            c.food = self.nearest_food(k, c);
        }
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
            if c.dodging > 0 && c.z != c.tz && self.only_plants_block(k, c.turn, c.x + dx, c.y + dy)
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
        // Only plants have bands: anything else in the way blocks them all.
        if !self.only_plants_block(k, c.turn, x, y) {
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
    /// overlaps plant cells but nothing else it can't swim through.
    fn only_plants_block(&self, k: &Kind, turn: u8, x: i32, y: i32) -> bool {
        let (wide, tall) = extent(k, turn);
        let Some(to) = body_box(wide, tall, x, y, self.width, self.height) else {
            return false;
        };
        let mut plant = false;
        for row in to.y0..to.y1 {
            for i in row * self.width + to.x0..row * self.width + to.x1 {
                let m = self.cells[i].material;
                if !self.creatures.swim[usize::from(m.0)] {
                    if self.plants.part(m).is_none() {
                        return false;
                    }
                    plant = true;
                }
            }
        }
        plant
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
        let Some(b) = body(k, c.turn, c.x, c.y, self.width, self.height) else {
            return false;
        };
        let reach = Rect {
            x0: b.x0.saturating_sub(k.reach),
            y0: b.y0.saturating_sub(k.reach),
            x1: (b.x1 + k.reach).min(self.width),
            y1: (b.y1 + k.reach).min(self.height),
        };
        for y in reach.y0..reach.y1 {
            for x in reach.x0..reach.x1 {
                let m = self.cells[y * self.width + x].material;
                if k.edible[usize::from(m.0)] {
                    let to = if y < self.surface_row {
                        self.creatures.air
                    } else {
                        self.creatures.water
                    };
                    self.write_cell(x, y, to, &mut None);
                    c.satiety = c.satiety.saturating_add(BITE).min(FULL);
                    c.energy = c.energy.saturating_add(BITE_ENERGY).min(ENERGY_RESERVE);
                    c.eaten += 1;
                    return true;
                }
            }
        }
        k.eats_corpses && self.bite_corpse(reach, c)
    }

    /// The nearest food cell `c` eats within [`FOOD_SENSE`], skipping the
    /// one it gave up on.
    #[allow(clippy::many_single_char_names)] // cell maths
    fn nearest_food(&self, k: &Kind, c: &Creature) -> Option<u32> {
        let (cx, cy) = (i64::from(c.x >> 8), i64::from(c.y >> 8));
        let sense2 = FOOD_SENSE * FOOD_SENSE;
        let mut best: Option<(i64, u32)> = None;
        for (ci, list) in self.life.food_lists().iter().enumerate() {
            if list.is_empty() {
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
            for &i in list {
                if Some(i) == c.ignore {
                    continue;
                }
                let m = self.cells[i as usize].material;
                if !k.edible[usize::from(m.0)] {
                    continue;
                }
                let (fx, fy) = (
                    to_i64(i as usize % self.width),
                    to_i64(i as usize / self.width),
                );
                if !near_home(c, fx) {
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

    /// A new random wander target inside the zone, and how long to keep it.
    #[allow(clippy::many_single_char_names)] // cell maths
    fn pick_target(&self, k: &Kind, c: &mut Creature, rng: &mut Pcg32) {
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
        let (x0, x1) = home_span(c).map_or((hw, w - hw), |(lo, hi)| (lo.max(hw), hi.min(w - hw)));
        let x = span(x0, x1, rng);
        let y = span(to_i64(top) + hh, to_i64(bottom) - hh, rng);
        (c.tx, c.ty) = (centre_of(x), centre_of(y));
        let (lo, hi) = RETARGET_TICKS;
        c.retarget = lo + rng.next_u32() % (hi - lo);
        // Every other target or so, a new band too.
        let (front, back) = k.bands;
        let pick = rng.next_u32();
        if pick.is_multiple_of(2) {
            let n = u32::from(back - front) + 1;
            c.tz = z_of(front + u8::try_from((pick >> 1) % n).unwrap_or(0));
        }
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
    fn fits(&self, k: &Kind, turn: u8, x: i32, y: i32, band: u8) -> bool {
        let (wide, tall) = extent(k, turn);
        self.fits_box(wide, tall, x, y, Some(band))
    }

    /// Whether a `w × h`-cell box centred at `(x, y)` (fixed point) lies
    /// inside the grid on cells it may overlap: swimmable ones, and plant
    /// cells further than [`BAND_REACH`] from `band` (`None`: none).
    fn fits_box(&self, wide: i32, tall: i32, x: i32, y: i32, band: Option<u8>) -> bool {
        let Some(b) = body_box(wide, tall, x, y, self.width, self.height) else {
            return false;
        };
        let mut memo = None;
        (b.y0..b.y1).all(|row| {
            (row * self.width + b.x0..row * self.width + b.x1)
                .all(|i| !self.blocks_with(i, band, &mut memo))
        })
    }

    /// Whether the cell at index `i` blocks a body in `band` (`None`: in
    /// every band).
    pub(super) fn blocks(&self, i: usize, band: Option<u8>) -> bool {
        self.blocks_with(i, band, &mut None)
    }

    /// [`World::blocks`], with a plant memo for a run of cells.
    #[inline]
    fn blocks_with(&self, i: usize, band: Option<u8>, memo: &mut Option<PlantDepth>) -> bool {
        let m = self.cells[i].material;
        if self.creatures.swim[usize::from(m.0)] {
            return false;
        }
        match (band, self.plants.part(m)) {
            (Some(band), Some(_)) => self
                .plant_band_with(i, memo)
                .is_none_or(|p| p.abs_diff(band) <= BAND_REACH),
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
/// swim level; `darting` (after food) lifts that.
fn steer(k: &Kind, c: &mut Creature, tx: i32, ty: i32, speed: i32, darting: bool) {
    let (dx, dy) = (i64::from(tx - c.x), i64::from(ty - c.y));
    let d = dist(tx - c.x, ty - c.y);
    let want = if d < ARRIVE {
        i64::from(speed) * d / ARRIVE
    } else {
        i64::from(speed)
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
        let start = w.creature(snail).unwrap().band();
        let mut seen = [false; DEPTH_BANDS as usize];
        for _ in 0..4000 {
            w.tick();
            for &id in &ids {
                let c = w.creature(id).unwrap();
                assert!((2..=3).contains(&c.band()), "band {}", c.band());
                assert!((2.0..=3.0).contains(&c.depth()), "depth {}", c.depth());
                seen[usize::from(c.band())] = true;
            }
            // A crawler keeps its band.
            assert_eq!(w.creature(snail).unwrap().band(), start);
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
}
