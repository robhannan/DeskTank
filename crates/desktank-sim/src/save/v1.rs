//! Format version 1 snapshot types. Frozen: once version 1 has shipped,
//! these types never change; a new version adds its own module and a
//! migration from this one.
//!
//! Field order is the wire order (`postcard` writes fields in declaration
//! order with no names), so fields are never reordered or removed.

use serde::{Deserialize, Serialize};

/// A whole tank.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Save {
    pub width: u32,
    pub height: u32,
    pub seed: u64,
    pub tick: u64,
    pub last_change_tick: u64,
    pub surface_row: u32,
    /// The table entries the save was written with, to check on load.
    pub materials: Vec<Named>,
    pub plant_species: Vec<Named>,
    pub species: Vec<Named>,
    pub grid: Grid,
    pub chunks: Vec<Chunk>,
    pub next_body: u16,
    pub bodies: Vec<Body>,
    pub life: Life,
    pub plants: Plants,
    pub creatures: Creatures,
}

/// An id and the name it stood for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Named {
    pub id: u8,
    pub name: String,
}

/// The cell grid as planes, row-major.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Grid {
    /// Material ids, run-length encoded.
    pub materials: Vec<(u32, u8)>,
    /// Each cell's variant XOR the noise a fresh tank has there.
    pub variants: Vec<u8>,
    /// Cell states, run-length encoded.
    pub states: Vec<(u32, u16)>,
}

/// A half-open cell rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Rect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

/// A chunk's work for the next tick and its sleep timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Chunk {
    pub work_next: Rect,
    pub changed_next: Rect,
    pub idle_ticks: u32,
    pub awake: bool,
}

/// A rigid body (T4b, T4c).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Body {
    pub id: u16,
    pub material: u8,
    pub cells: Vec<u32>,
    pub bounds: Rect,
    pub anchored: bool,
    /// 0 none, 1 sort, 2 split.
    pub pending: u8,
    pub awake: bool,
    pub held_until: u64,
    pub shape: Shape,
    pub anchor: (i64, i64),
    pub drawn: i64,
    pub pos: (i64, i64),
    pub vel: (i64, i64),
    pub angle: i64,
    pub spin: i64,
    pub still: u16,
}

/// A body's reference shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Shape {
    pub w: u32,
    pub h: u32,
    pub grid: Vec<u16>,
    pub n: i64,
    pub com: (i64, i64),
    pub inertia: i64,
    pub radius: i64,
}

/// Random ticks and fields (T5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Life {
    pub budget: u32,
    pub cursor: u64,
    /// Per chunk, in scheduler order.
    pub living: Vec<Vec<u32>>,
    pub devices: Vec<Vec<u32>>,
    pub food: Vec<Vec<u32>>,
    pub devices_dirty: bool,
    pub nutrient: Field,
    pub co2: Field,
    pub light: Field,
}

/// A field's values and whether it is still settling.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Field {
    pub values: Vec<f32>,
    pub active: bool,
}

/// The plant table (T6b).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Plants {
    pub next_id: u16,
    pub cursor: u16,
    pub any_pending: bool,
    pub cap: u32,
    pub table: Vec<Plant>,
}

/// One plant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Plant {
    pub id: u16,
    pub species: u8,
    pub born: u64,
    pub health: f32,
    pub energy: f32,
    pub parts: [u32; 3],
    pub base: u32,
    pub bounds: Rect,
    pub parent: u16,
    pub generation: u16,
    pub stress: f32,
    pub limit: Limit,
    pub last_update: u64,
    pub sums: [f32; 3],
    pub counts: [u32; 3],
    pub pending: bool,
    pub idle: u8,
}

/// What held a plant back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Limit {
    None,
    TooLow(Factor),
    TooHigh(Factor),
}

/// A growth factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Factor {
    Light,
    Nutrient,
    Co2,
}

/// The creatures (T7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Creatures {
    pub next_id: u32,
    pub list: Vec<Creature>,
}

/// One creature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Creature {
    pub id: u32,
    pub species: u8,
    pub x: i32,
    pub y: i32,
    pub vx: i32,
    pub vy: i32,
    pub tx: i32,
    pub ty: i32,
    pub retarget: u32,
    pub satiety: u16,
    pub food: Option<u32>,
    pub ignore: Option<u32>,
    pub facing_left: bool,
    pub turn: u8,
    pub held_until: u64,
    pub next_breath: u64,
    pub eaten: u32,
}
