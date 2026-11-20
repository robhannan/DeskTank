//! Save files (SPEC §2.6, T8): a versioned envelope around a compressed
//! snapshot of a [`World`].
//!
//! This module is the format only: bytes in, bytes out, with no file
//! system, so the native app (files, an ownership lock, atomic writes) and
//! the web (`IndexedDB`, WB3) share it.
//!
//! **Envelope** (little-endian, [`HEADER_LEN`] bytes, then the payload):
//!
//! | Bytes | Field |
//! |---|---|
//! | 0–7 | magic, [`MAGIC`] |
//! | 8–9 | format version ([`FORMAT_VERSION`]) |
//! | 10–11 | reserved, 0 |
//! | 12–15 | payload length in bytes |
//! | 16–19 | CRC-32 (IEEE) of the payload |
//!
//! **Payload**: an LZ4 block (its uncompressed size prepended as a
//! little-endian `u32`) holding the `postcard` encoding of the version's
//! snapshot (`save::v1::Save` for version 1, `save::v2::Save` for 2,
//! `save::v3::Save` for 3, `save::v4::Save` for 4, `save::v5::Save` for 5,
//! `save::v6::Save` for 6, `save::v7::Save` for 7, `save::v8::Save` for 8,
//! `save::v9::Save` for 9, `save::v10::Save` for 10).
//! The grid inside it is stored
//! as planes: material and state run-length encoded, colour noise as its
//! difference from the noise a fresh tank has there (mostly zero runs for
//! the LZ4 pass).
//!
//! **What is saved** is everything that changes as the tank runs, so a
//! loaded world has the same [`World::state_hash`] and continues exactly as
//! the saved one would have: the size, seed, tick and surface row; the
//! grid; the chunks' pending work and sleep timers; the rigid bodies with
//! their pose and reference shape (T4c) and the next body id (T4b); the
//! random-tick lists and cursor, and the nutrient, CO₂ and light fields
//! (T5); the plant table with its update bookkeeping (T6b); the creatures
//! (T7) with their age, growth, energy and stress, the corpses, easy mode
//! and the population cap (T9, version 2), and their sex, colour-morph
//! alleles and breeding state, the egg clutches and every species' morph
//! names (T9 part 2, version 3); each creature's depth and
//! target depth and each corpse's band (D21, version 4); and each
//! creature's home (a home owning species', version 5); and each
//! creature's name (T9 part 3, version 6); and each floater's raft, its
//! motion as a buoyant body (T6c part 2, version 7); and each plant's
//! colouration (T6d, version 8); and each creature's quarry, pursuit
//! counter and fright (hunting and territory, version 9); and the
//! sub-cell outline masks of rigid cells and bodies' shapes (version 10).
//! Random streams are derived from `(seed, tick, stream)` (SPEC
//! §2.1), so no generator state is stored. Lookups built from the material,
//! species and reaction tables are rebuilt on load; the tables come from
//! the build, and the save records the name behind every material and
//! species id so a load can check they still match. The built-in reaction
//! table and field settings are assumed ([`World::new`]).
//!
//! **Versions.** A newer build reads every older version: each version
//! keeps its snapshot types (`v1`, …) and a migration to the next, and
//! [`decode`] runs them in turn. A version newer than the build is
//! rejected. Version 2 (T9) migrates a version 1 tank's creatures to
//! well-fed adults just mature, with the default cap and easy mode off.
//! Version 3 (T9 part 2) gives a version 2 tank's creatures the morph they
//! were drawn in (the palette's) and alternates their sexes within each
//! species in id order; none is carrying, and each settles for a day
//! before breeding. A species' morphs may only be appended to: a save
//! whose morph names aren't a prefix of the build's is a mismatch.
//! Version 4 (D21) puts a version 3 tank's creatures at rest in the band a
//! creature just added would start in (a hash of the seed and its id,
//! within its species' bands), and its corpses in the front band.
//! Version 5 migrates a version 4 tank's creatures with no home, version
//! 6 a version 5 tank's with no name, version 7 a version 6 tank with
//! no rafts (no build before it had floaters), and version 8 a version 7
//! tank's plants uncoloured, and version 9 a version 8 tank's creatures
//! calm, with no quarry and no rest from a chase, and version 10 a
//! version 9 tank's rigid cells whole.

use thiserror::Error;

use crate::material::MaterialTable;
use crate::species::{SpeciesId, SpeciesTable};
use crate::world::{DEPTH_BANDS, LIFE_DAY_TICKS, World, WorldError, default_band};

pub(crate) mod v1;
pub(crate) mod v10;
pub(crate) mod v2;
pub(crate) mod v3;
pub(crate) mod v4;
pub(crate) mod v5;
pub(crate) mod v6;
pub(crate) mod v7;
pub(crate) mod v8;
pub(crate) mod v9;

/// The first eight bytes of every save.
pub const MAGIC: [u8; 8] = *b"DeskTank";
/// The format version [`encode`] writes.
pub const FORMAT_VERSION: u16 = 10;
/// Envelope header size in bytes.
pub const HEADER_LEN: usize = 20;
/// Largest uncompressed payload a save may declare (256 MiB): bounds the
/// allocation a damaged or hostile file can ask for.
const MAX_PAYLOAD: usize = 256 << 20;

/// Why a save couldn't be read.
#[derive(Debug, Error, PartialEq)]
pub enum SaveError {
    /// The bytes don't start with [`MAGIC`].
    #[error("not a DeskTank save")]
    NotASave,
    /// The file is shorter than its header says.
    #[error("the save is truncated ({have} of {want} bytes)")]
    Truncated {
        /// Bytes present.
        have: usize,
        /// Bytes the header declares.
        want: usize,
    },
    /// The file is longer than its header says.
    #[error("the save has {0} unexpected bytes after its payload")]
    TrailingBytes(usize),
    /// Written by a newer DeskTank (or an unknown version).
    #[error("save format version {0} is not supported (this build reads 1..={FORMAT_VERSION})")]
    UnsupportedVersion(u16),
    /// The payload doesn't match its checksum.
    #[error("the save's checksum doesn't match: it is damaged")]
    Checksum,
    /// The payload couldn't be decompressed or decoded.
    #[error("the save's payload is damaged: {0}")]
    Payload(String),
    /// A material or species id now names something else (or nothing).
    #[error("the save doesn't match this build: {0}")]
    Mismatch(String),
    /// The snapshot is inconsistent (sizes, indices out of range).
    #[error("the save is invalid: {0}")]
    Invalid(String),
    /// The world couldn't be created.
    #[error(transparent)]
    World(#[from] WorldError),
}

/// Encodes `world` as a save file of the current [`FORMAT_VERSION`].
#[must_use]
pub fn encode(world: &World) -> Vec<u8> {
    let snapshot = world.to_v10();
    // Serialising plain data to a Vec can't fail.
    let raw = postcard::to_allocvec(&snapshot).unwrap_or_default();
    let payload = lz4_flex::block::compress_prepend_size(&raw);
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    // Payloads are far below 4 GiB (MAX_SIDE² cells).
    #[allow(clippy::cast_possible_truncation)]
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&crc32fast::hash(&payload).to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

/// Reads a save file into a world built on `materials` (the table the save
/// was written with, or a later one that keeps its ids).
///
/// # Errors
///
/// Returns an error if the bytes aren't an intact save of a supported
/// version, or the snapshot doesn't fit the tables or itself.
pub fn decode(bytes: &[u8], materials: MaterialTable) -> Result<World, SaveError> {
    let (version, payload) = open(bytes)?;
    let raw = decompress(payload)?;
    let payload = |e: postcard::Error| SaveError::Payload(e.to_string());
    let from_v8 = |s: v8::Save| World::from_v9(v9::Save::from_v8(s), materials.clone());
    let from_v7 = |s: v7::Save| from_v8(v8::Save::from_v7(s));
    let from_v6 = |s: v6::Save| from_v7(v7::Save::from_v6(s));
    if version == 10 {
        let snapshot: v10::Save = postcard::from_bytes(&raw).map_err(payload)?;
        return World::from_v10(snapshot, materials);
    }
    if version == 9 {
        let snapshot: v9::Save = postcard::from_bytes(&raw).map_err(payload)?;
        return World::from_v10(v10::Save::from_v9(snapshot), materials);
    }
    if version == 8 {
        let snapshot: v8::Save = postcard::from_bytes(&raw).map_err(payload)?;
        return from_v8(snapshot);
    }
    if version == 7 {
        let snapshot: v7::Save = postcard::from_bytes(&raw).map_err(payload)?;
        return from_v7(snapshot);
    }
    if version == 6 {
        let snapshot: v6::Save = postcard::from_bytes(&raw).map_err(payload)?;
        return from_v6(snapshot);
    }
    if version == 5 {
        let snapshot: v5::Save = postcard::from_bytes(&raw).map_err(payload)?;
        return from_v6(v6::Save::from_v5(snapshot));
    }
    if version == 4 {
        let snapshot: v4::Save = postcard::from_bytes(&raw).map_err(payload)?;
        return from_v6(v6::Save::from_v5(v5::Save::from_v4(snapshot)));
    }
    let species =
        || SpeciesTable::builtin_for(&materials).map_err(|e| WorldError::Species(e.to_string()));
    let snapshot = match version {
        1 | 2 => {
            let species = species()?;
            let v2 = if version == 1 {
                let v1: v1::Save = postcard::from_bytes(&raw).map_err(payload)?;
                v2::Save::from_v1(v1, |id| {
                    species
                        .get(SpeciesId(id))
                        .map_or(0, |s| s.maturity_days.saturating_mul(LIFE_DAY_TICKS))
                })
            } else {
                postcard::from_bytes(&raw).map_err(payload)?
            };
            v3::Save::from_v2(v2, |id| {
                species.get(SpeciesId(id)).map(|s| v3::Before {
                    palette_morph: s.palette_morph,
                    morphs: s.morphs.clone(),
                })
            })
        }
        3 => postcard::from_bytes(&raw).map_err(payload)?,
        v => return Err(SaveError::UnsupportedVersion(v)),
    };
    let species = species()?;
    let seed = snapshot.seed;
    let snapshot = v4::Save::from_v3(snapshot, |id, s| {
        let bands = species
            .get(SpeciesId(s))
            .map_or((0, DEPTH_BANDS - 1), |s| (s.bands[0], s.bands[1]));
        default_band(seed, id, bands)
    });
    from_v6(v6::Save::from_v5(v5::Save::from_v4(snapshot)))
}

/// Checks the envelope (magic, version, length, checksum) without decoding
/// the snapshot. Returns the format version.
///
/// # Errors
///
/// As [`decode`], for the envelope.
pub fn check(bytes: &[u8]) -> Result<u16, SaveError> {
    open(bytes).map(|(version, _)| version)
}

/// Validates the envelope; returns the version and the payload.
fn open(bytes: &[u8]) -> Result<(u16, &[u8]), SaveError> {
    if bytes.len() < MAGIC.len() || bytes[..MAGIC.len()] != MAGIC {
        return Err(SaveError::NotASave);
    }
    if bytes.len() < HEADER_LEN {
        return Err(SaveError::Truncated {
            have: bytes.len(),
            want: HEADER_LEN,
        });
    }
    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let u32_at =
        |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let version = u16_at(8);
    if version == 0 || version > FORMAT_VERSION || u16_at(10) != 0 {
        return Err(SaveError::UnsupportedVersion(version));
    }
    let len = u32_at(12) as usize;
    let want = HEADER_LEN + len;
    match bytes.len() {
        have if have < want => return Err(SaveError::Truncated { have, want }),
        have if have > want => return Err(SaveError::TrailingBytes(have - want)),
        _ => {}
    }
    let payload = &bytes[HEADER_LEN..];
    if crc32fast::hash(payload) != u32_at(16) {
        return Err(SaveError::Checksum);
    }
    Ok((version, payload))
}

/// Decompresses an LZ4 block with its size prepended, refusing sizes above
/// [`MAX_PAYLOAD`] or beyond what LZ4 can expand the input to.
fn decompress(payload: &[u8]) -> Result<Vec<u8>, SaveError> {
    let Some(size) = payload.get(..4) else {
        return Err(SaveError::Payload("no size".to_owned()));
    };
    let size = u32::from_le_bytes([size[0], size[1], size[2], size[3]]) as usize;
    // LZ4 expands at most about 255:1.
    if size > MAX_PAYLOAD || size > payload.len().saturating_mul(255) {
        return Err(SaveError::Payload(format!("implausible size {size}")));
    }
    lz4_flex::block::decompress_size_prepended(payload)
        .map_err(|e| SaveError::Payload(e.to_string()))
}

/// Run-length encodes `values`: (run length, value) pairs.
pub(crate) fn runs<T: Copy + PartialEq>(values: impl IntoIterator<Item = T>) -> Vec<(u32, T)> {
    let mut out: Vec<(u32, T)> = Vec::new();
    for v in values {
        match out.last_mut() {
            Some((n, last)) if *last == v && *n < u32::MAX => *n += 1,
            _ => out.push((1, v)),
        }
    }
    out
}

/// Expands runs into exactly `len` values.
///
/// # Errors
///
/// Returns [`SaveError::Invalid`] if the runs don't add up to `len`.
pub(crate) fn unrun<T: Copy>(
    runs: &[(u32, T)],
    len: usize,
    what: &str,
) -> Result<Vec<T>, SaveError> {
    let total: u64 = runs.iter().map(|&(n, _)| u64::from(n)).sum();
    if total != len as u64 {
        return Err(SaveError::Invalid(format!(
            "{what} covers {total} cells, the grid has {len}"
        )));
    }
    let mut out = Vec::with_capacity(len);
    for &(n, v) in runs {
        out.extend(core::iter::repeat_n(v, n as usize));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_round_trip() {
        let v = [0u8, 0, 0, 5, 5, 1, 0, 0];
        let r = runs(v);
        assert_eq!(r, vec![(3, 0), (2, 5), (1, 1), (2, 0)]);
        assert_eq!(unrun(&r, v.len(), "x").unwrap(), v);
        assert!(unrun(&r, 9, "x").is_err());
        assert_eq!(runs(core::iter::empty::<u8>()), Vec::new());
    }

    #[test]
    fn rejects_bad_plant_colours() {
        let mut world = World::new(
            crate::WorldConfig {
                width: 64,
                height: 32,
                ..crate::WorldConfig::default()
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let soil = world.materials().id("aquasoil").unwrap();
        let bed = crate::Rect {
            x0: 0,
            y0: 26,
            x1: 64,
            y1: 32,
        };
        world.fill_rect(bed, soil, crate::Brush::Replace).unwrap();
        let rotala = world.plant_species().id("rotala").unwrap();
        world
            .paint_plant(32, 24, 0, rotala, crate::Propagule::Cuttings)
            .unwrap();
        for _ in 0..200 {
            world.tick();
        }
        let id = world.plants().next().unwrap().id.0;
        let mut good = world.to_v9();
        let colour = |plant, colour| v9::PlantColour { plant, colour };
        good.colours = vec![colour(id, 0.5)];
        let load = |s: v9::Save| World::from_v9(s, MaterialTable::builtin().unwrap());
        let loaded = load(good.clone()).unwrap();
        assert!((loaded.plants().next().unwrap().colour - 0.5).abs() < f32::EPSILON);
        for bad in [
            vec![colour(id, 1.5)],
            vec![colour(id, f32::NAN)],
            vec![colour(id + 100, 0.5)],
            vec![colour(id, 0.5), colour(id, 0.25)],
        ] {
            let mut s = good.clone();
            s.colours = bad;
            assert!(matches!(load(s), Err(SaveError::Invalid(_))));
        }
    }

    #[test]
    fn old_food_lists_gain_settled_fronds() {
        let mut world = World::new(
            crate::WorldConfig::default(),
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let frond = world.materials().id("floating_leaf").unwrap();
        world
            .paint(
                20,
                i64::try_from(world.surface_row()).unwrap(),
                0,
                frond,
                crate::Brush::Replace,
            )
            .unwrap();
        for _ in 0..20 {
            world.tick();
        }
        let mut saved = world.to_v9();
        assert_eq!(saved.life.food.iter().map(Vec::len).sum::<usize>(), 1);
        saved.life.food.iter_mut().for_each(Vec::clear);
        let restored = World::from_v9(saved, MaterialTable::builtin().unwrap()).unwrap();
        assert_eq!(restored.to_v9().life.food, world.to_v9().life.food);
    }

    #[test]
    fn rejects_snapshots_that_dont_fit() {
        let mut world = World::new(
            crate::WorldConfig {
                width: 128,
                height: 48,
                ..crate::WorldConfig::default()
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let rock = world.materials().id("rock").unwrap();
        world.paint(20, 10, 3, rock, crate::Brush::Pour).unwrap();
        world.tick();
        let good = world.to_v9();
        let load = |s: v9::Save| World::from_v9(s, MaterialTable::builtin().unwrap());
        assert!(load(good.clone()).is_ok());

        let mut renamed = good.clone();
        renamed.materials[0].name = "unobtainium".to_owned();
        assert!(matches!(load(renamed), Err(SaveError::Mismatch(_))));
        let mut gone = good.clone();
        gone.species.push(v1::Named {
            id: 250,
            name: "kraken".to_owned(),
        });
        assert!(matches!(load(gone), Err(SaveError::Mismatch(_))));

        let mut short = good.clone();
        short.grid.variants.pop();
        assert!(matches!(load(short), Err(SaveError::Invalid(_))));
        let mut runs = good.clone();
        runs.grid.materials[0].0 += 1;
        assert!(matches!(load(runs), Err(SaveError::Invalid(_))));
        let mut body = good.clone();
        body.bodies[0].cells.push(128 * 48);
        assert!(matches!(load(body), Err(SaveError::Invalid(_))));
        let mut chunks = good.clone();
        chunks.chunks.pop();
        assert!(matches!(load(chunks), Err(SaveError::Invalid(_))));
        let mut listed = good.clone();
        // Cell (100, 0) lies in the second chunk.
        listed.life.living[0].push(100);
        assert!(matches!(load(listed), Err(SaveError::Invalid(_))));
        let mut field = good.clone();
        field.life.nutrient.values[0] = f32::NAN;
        assert!(matches!(load(field), Err(SaveError::Invalid(_))));
        // Corpses are no bigger than the adult and release a third of
        // their cells, as a death makes them.
        let mut dead = good.clone();
        let tetra = world.species().id("neon_tetra").unwrap();
        dead.creatures.corpses.push(v6::Corpse {
            species: tetra.0,
            cause: 0,
            x: 20 * 256,
            y: 20 * 256,
            w: 7,
            h: 3,
            facing_left: false,
            band: 3,
            died: 0,
            released: 0,
            total: 7,
            morph: 0,
            female: false,
        });
        assert!(load(dead.clone()).is_ok());
        for bad in [
            |c: &mut v6::Corpse| c.w = 255,
            |c: &mut v6::Corpse| c.h = 0,
            |c: &mut v6::Corpse| c.total = 65_535,
            |c: &mut v6::Corpse| c.released = 7,
            |c: &mut v6::Corpse| c.morph = 1,
            |c: &mut v6::Corpse| c.band = DEPTH_BANDS,
        ] {
            let mut s = dead.clone();
            bad(&mut s.creatures.corpses[0]);
            assert!(matches!(load(s), Err(SaveError::Invalid(_))));
        }
        let mut capped = good.clone();
        capped.creatures.cap = 0;
        assert!(matches!(load(capped), Err(SaveError::Invalid(_))));
        let mut big = good;
        big.width = 9000;
        assert!(matches!(load(big), Err(SaveError::World(_))));
    }

    /// Crafted version 3 saves with breeding state no tank makes: a
    /// carrying male, a pregnancy longer than the species', alleles outside
    /// its morphs, and clutches that are malformed or of a species that lays
    /// no eggs.
    #[test]
    fn rejects_bad_breeding_state() {
        let mut world = World::new(
            crate::WorldConfig {
                width: 128,
                height: 48,
                ..crate::WorldConfig::default()
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let guppy = world.species().id("guppy").unwrap();
        let tetra = world.species().id("neon_tetra").unwrap();
        let nerite = world.species().id("nerite").unwrap();
        for (x, female) in [(40, false), (60, true)] {
            let traits = crate::Traits {
                female: Some(female),
                ..crate::Traits::default()
            };
            world
                .add_creature_with_traits(guppy, x, 20, traits)
                .unwrap();
        }
        let mut good = world.to_v9();
        let gestation =
            world.species().get(guppy).unwrap().breeding.unwrap().days * crate::LIFE_DAY_TICKS;
        let female = good
            .creatures
            .list
            .iter()
            .position(|c| c.base.female)
            .unwrap();
        let male = 1 - female;
        good.creatures.list[female].base.carrying = gestation;
        good.creatures.clutches.push(v3::Clutch {
            id: 1,
            species: tetra.0,
            mother: [0, 0],
            sire: [0, 0],
            hatch: 100,
            eggs: vec![130, 140],
        });
        good.creatures.next_clutch = 2;
        let load = |s: v9::Save| World::from_v9(s, MaterialTable::builtin().unwrap());
        assert!(load(good.clone()).is_ok());
        // The next clutch id is only where the search starts: 0 becomes 1.
        let mut zero = good.clone();
        zero.creatures.next_clutch = 0;
        assert!(load(zero).is_ok());

        let creature_bad: [fn(&mut v6::Creature); 3] = [
            |c| c.carrying += 1,
            |c| c.alleles = [0, 9],
            |c| c.sire = [9, 0],
        ];
        for (n, bad) in creature_bad.iter().enumerate() {
            let mut s = good.clone();
            bad(&mut s.creatures.list[female].base);
            assert!(matches!(load(s), Err(SaveError::Invalid(_))), "female {n}");
        }
        let mut carrying_male = good.clone();
        carrying_male.creatures.list[male].base.carrying = 1;
        assert!(matches!(load(carrying_male), Err(SaveError::Invalid(_))));

        let clutch_bad: [fn(&mut v3::Clutch); 7] = [
            |c| c.id = 0,
            |c| c.species = 250,
            |c| c.mother = [0, 9],
            |c| c.sire = [9, 9],
            |c| c.eggs = vec![140, 130],
            |c| c.eggs = vec![130, 130],
            |c| c.eggs = vec![128 * 48],
        ];
        for (n, bad) in clutch_bad.iter().enumerate() {
            let mut s = good.clone();
            bad(&mut s.creatures.clutches[0]);
            assert!(matches!(load(s), Err(SaveError::Invalid(_))), "clutch {n}");
        }
        let mut snail = good.clone();
        snail.creatures.clutches[0].species = nerite.0;
        assert!(matches!(load(snail), Err(SaveError::Invalid(_))));
        let mut live = good.clone();
        live.creatures.clutches[0].species = guppy.0;
        assert!(matches!(load(live), Err(SaveError::Invalid(_))));
        let mut twice = good.clone();
        let again = twice.creatures.clutches[0].clone();
        twice.creatures.clutches.push(again);
        assert!(matches!(load(twice), Err(SaveError::Invalid(_))));
        let mut many = good;
        let one = many.creatures.clutches[0].clone();
        many.creatures.clutches = (1..=65)
            .map(|id| v3::Clutch { id, ..one.clone() })
            .collect();
        assert!(matches!(load(many), Err(SaveError::Invalid(_))));
    }

    #[test]
    fn rejects_homes_outside_the_grid_or_for_species_without_one() {
        let mut world = World::new(
            crate::WorldConfig {
                width: 64,
                height: 48,
                ..crate::WorldConfig::default()
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        for name in ["neon_tetra", "home_owning_crab"] {
            let s = world.species().id(name).unwrap();
            world.add_creature(s, 32, 30).unwrap();
        }
        let good = world.to_v9();
        let load = |s: v9::Save| World::from_v9(s, MaterialTable::builtin().unwrap());
        let with = |i: usize, home| {
            let mut s = good.clone();
            s.creatures.list[i].base.home = Some(home);
            load(s)
        };
        let owner = with(1, (10, 40)).unwrap();
        assert_eq!(owner.creatures()[1].home(), Some((10, 40)));
        assert!(matches!(with(1, (64, 40)), Err(SaveError::Invalid(_))));
        assert!(matches!(with(1, (10, 48)), Err(SaveError::Invalid(_))));
        assert!(matches!(with(0, (10, 40)), Err(SaveError::Invalid(_))));
    }

    #[test]
    fn rejects_pursuit_state_a_tank_wouldnt_have() {
        use crate::world::pursuit::{DRIVE_TICKS, FEAR_TICKS, HUNT_TICKS, PURSUIT_REST};
        let mut world = World::new(
            crate::WorldConfig {
                width: 64,
                height: 48,
                ..crate::WorldConfig::default()
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        for name in ["betta", "neon_tetra"] {
            let s = world.species().id(name).unwrap();
            world.add_creature(s, 32, 30).unwrap();
        }
        let good = world.to_v9();
        let load = |s: v9::Save| World::from_v9(s, MaterialTable::builtin().unwrap());
        let with = |quarry: Option<u32>, pursuit: u16, fear: u16| {
            let mut s = good.clone();
            let c = &mut s.creatures.list[0];
            (c.quarry, c.pursuit, c.fear) = (quarry, pursuit, fear);
            load(s)
        };
        let hunting = with(Some(2), HUNT_TICKS.max(DRIVE_TICKS), FEAR_TICKS).unwrap();
        let betta = &hunting.creatures()[0];
        assert_eq!(betta.quarry(), Some(crate::CreatureId(2)));
        assert_eq!(betta.fear(), u32::from(FEAR_TICKS));
        assert!(with(None, PURSUIT_REST, 0).is_ok());
        let bad = [
            (Some(1), 0, 0),
            (Some(9), 0, 0),
            (Some(2), HUNT_TICKS.max(DRIVE_TICKS) + 1, 0),
            (None, PURSUIT_REST + 1, 0),
            (None, 0, FEAR_TICKS + 1),
        ];
        for (quarry, pursuit, fear) in bad {
            assert!(
                matches!(with(quarry, pursuit, fear), Err(SaveError::Invalid(_))),
                "{quarry:?} {pursuit} {fear}"
            );
        }
    }

    #[test]
    fn keeps_names_and_rejects_ones_a_host_wouldnt_keep() {
        let mut world = World::new(
            crate::WorldConfig {
                width: 64,
                height: 48,
                ..crate::WorldConfig::default()
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let tetra = world.species().id("neon_tetra").unwrap();
        let id = world.add_creature(tetra, 32, 30).unwrap();
        world.set_creature_name(id, "Bubbles");
        let good = world.to_v9();
        assert_eq!(good.creatures.list[0].base.name, "Bubbles");
        let load = |s: v9::Save| World::from_v9(s, MaterialTable::builtin().unwrap());
        let with = |name: &str| {
            let mut s = good.clone();
            s.creatures.list[0].base.name = name.to_owned();
            load(s)
        };
        assert_eq!(
            with("Bubbles").unwrap().creatures()[0].name(),
            Some("Bubbles")
        );
        assert_eq!(with("").unwrap().creatures()[0].name(), None);
        let long = "x".repeat(crate::MAX_NAME_CHARS + 1);
        for bad in [" Bubbles", "Bub  bles", "Bub\u{7}bles", long.as_str()] {
            assert!(matches!(with(bad), Err(SaveError::Invalid(_))), "{bad:?}");
        }
    }

    /// A raft must belong to a floater plant, once, with a pose in range;
    /// a floater the save has no raft for gets one at rest.
    #[test]
    fn rejects_rafts_that_dont_fit_and_fills_in_missing_ones() {
        let mut world = World::new(
            crate::WorldConfig {
                width: 64,
                height: 48,
                ..crate::WorldConfig::default()
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let duckweed = world.plant_species().id("duckweed").unwrap();
        world
            .paint_plant(20, 20, 0, duckweed, crate::Propagule::Buds)
            .unwrap();
        while world.plant_count() == 0 {
            world.tick();
        }
        // A stem cell drawn in starts a plant of the default species.
        let stem = world.materials().id("stem").unwrap();
        world.set_cell(40, 40, stem).unwrap();
        world.tick();
        assert_eq!(world.plant_count(), 2);
        let good = world.to_v9();
        assert_eq!(good.rafts.len(), 1, "only the floater has a raft");
        let load = |s: v9::Save| World::from_v9(s, MaterialTable::builtin().unwrap());
        assert_eq!(load(good.clone()).unwrap().state_hash(), world.state_hash());
        let floater = good.rafts[0].plant;
        let other = good
            .plants
            .table
            .iter()
            .find(|p| p.id != floater)
            .unwrap()
            .id;
        let bad: [fn(&mut v9::Raft, u16); 4] = [
            |r, other| r.plant = other,
            |r, _| r.plant = 999,
            |r, _| r.off = (0, 5 * 256),
            |r, _| r.vel = (-5 * 256, 0),
        ];
        for (n, bad) in bad.iter().enumerate() {
            let mut s = good.clone();
            bad(&mut s.rafts[0], other);
            assert!(matches!(load(s), Err(SaveError::Invalid(_))), "raft {n}");
        }
        let mut twice = good.clone();
        twice.rafts.push(twice.rafts[0]);
        assert!(matches!(load(twice), Err(SaveError::Invalid(_))));
        let mut missing = good;
        missing.rafts.clear();
        let filled = load(missing).unwrap();
        assert_eq!(filled.to_v9().rafts.len(), 1);
    }

    #[test]
    fn rejects_bad_envelopes() {
        let world = World::new(
            crate::WorldConfig {
                width: 16,
                height: 16,
                ..crate::WorldConfig::default()
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let good = encode(&world);
        assert_eq!(check(&good), Ok(FORMAT_VERSION));
        let t = || MaterialTable::builtin().unwrap();

        assert_eq!(decode(b"PNG", t()).err(), Some(SaveError::NotASave));
        assert!(matches!(
            decode(&good[..good.len() - 1], t()),
            Err(SaveError::Truncated { .. })
        ));
        let mut long = good.clone();
        long.push(0);
        assert_eq!(decode(&long, t()).err(), Some(SaveError::TrailingBytes(1)));
        let mut newer = good.clone();
        newer[8..10].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        assert_eq!(
            decode(&newer, t()).err(),
            Some(SaveError::UnsupportedVersion(FORMAT_VERSION + 1))
        );
        let mut flipped = good.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 0x40;
        assert_eq!(decode(&flipped, t()).err(), Some(SaveError::Checksum));
        // A payload with a matching checksum that claims a huge size.
        let mut bomb = good[..HEADER_LEN].to_vec();
        let payload = [0xff, 0xff, 0xff, 0x7f, 0, 0];
        bomb[12..16].copy_from_slice(&6u32.to_le_bytes());
        bomb[16..20].copy_from_slice(&crc32fast::hash(&payload).to_le_bytes());
        bomb.extend_from_slice(&payload);
        assert!(matches!(decode(&bomb, t()), Err(SaveError::Payload(_))));
    }
}
