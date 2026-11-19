//! SPEC T8 acceptance tests (§2.6): a save round-trips to the same state
//! hash, a loaded tank carries on exactly as the saved one would have
//! (every part of the state that feeds later ticks is saved, including the
//! rigid bodies the hash leaves out), and the version 1 and 2 fixtures
//! load (version 1 migrated to 2, T9).
//!
//! The version 1 fixture is frozen: this build no longer writes version 1.
//! `DESKTANK_BLESS=1 cargo test -p desktank-sim --test save` rewrites the
//! version 2 fixture; only do that while version 2 is unreleased or its
//! writer was wrong, since the point of a fixture is that old saves keep
//! loading.

#![allow(clippy::cast_possible_wrap)]

use std::path::PathBuf;

use desktank_sim::save::{self, FORMAT_VERSION, SaveError};
use desktank_sim::{
    Brush, DEFAULT_CREATURE_CAP, ENERGY_RESERVE, LIFE_DAY_TICKS, LifeStage, MaterialId,
    MaterialTable, Placeable, Rect, World, WorldConfig,
};

fn table() -> MaterialTable {
    MaterialTable::builtin().unwrap()
}

fn id(w: &World, name: &str) -> MaterialId {
    w.materials().id(name).unwrap()
}

fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Rect {
    Rect { x0, y0, x1, y1 }
}

/// A busy tank: an aquasoil and sand bed, a glued ledge and a free rock
/// above it, plants of every species, a bubbler and a filter outlet, food,
/// fertiliser and every creature species, run for `ticks`.
fn busy(seed: u64, ticks: u32) -> World {
    let mut w = World::new(
        WorldConfig {
            width: 200,
            height: 120,
            seed,
            water_level: 0.92,
        },
        table(),
    )
    .unwrap();
    let (soil, sand, rock, wood) = (
        id(&w, "aquasoil"),
        id(&w, "sand"),
        id(&w, "rock"),
        id(&w, "driftwood"),
    );
    w.fill_rect(rect(0, 104, 200, 120), soil, Brush::Replace)
        .unwrap();
    w.fill_rect(rect(120, 98, 200, 104), sand, Brush::Replace)
        .unwrap();
    w.fill_rect(rect(20, 60, 60, 64), rock, Brush::Replace)
        .unwrap();
    let ledge = w.body_at(30, 61).unwrap();
    w.set_anchored(ledge, true);
    let species: Vec<_> = w
        .plant_species()
        .iter()
        .map(|s| (s.id, s.default_propagule()))
        .collect();
    for (k, (s, propagule)) in species.into_iter().enumerate() {
        w.paint_plant(80 + 30 * k as i64, 100, 2, s, propagule)
            .unwrap();
    }
    w.place(Placeable::Bubbler, 10, 102).unwrap();
    w.place(Placeable::FilterOutlet, 190, 40).unwrap();
    for name in [
        "flake_food",
        "sinking_pellet",
        "liquid_fertiliser",
        "root_tab",
    ] {
        let m = id(&w, name);
        w.paint(100, 20, 2, m, Brush::Pour).unwrap();
    }
    let species: Vec<_> = w.species().iter().map(|s| s.id).collect();
    for (k, s) in species.into_iter().enumerate() {
        let (top, bottom) = w.zone_rows(s).unwrap();
        let y = i64::try_from(usize::midpoint(top, bottom)).unwrap();
        // Room for a creature is not guaranteed; a full spot is fine.
        let _ = w.add_creature(s, 15 + 17 * k as i64, y);
    }
    for t in 0..ticks {
        w.tick();
        // Keep things moving: a rock and driftwood dropped in mid-run.
        if t == ticks / 2 {
            w.paint(150, 30, 4, rock, Brush::Pour).unwrap();
            w.paint(50, 90, 3, wood, Brush::Pour).unwrap();
        }
    }
    w
}

fn reload(w: &World) -> World {
    save::decode(&save::encode(w), table()).unwrap()
}

#[test]
fn round_trip_keeps_the_state_hash() {
    let w = busy(3, 2400);
    assert!(w.plant_count() >= 3 && !w.creatures().is_empty());
    assert!(w.bodies().count() >= 3);
    let bytes = save::encode(&w);
    assert_eq!(save::check(&bytes), Ok(FORMAT_VERSION));
    let loaded = save::decode(&bytes, table()).unwrap();
    assert_eq!(loaded.state_hash(), w.state_hash());
    assert_eq!(loaded.tick_count(), w.tick_count());
    assert_eq!(loaded.awake_chunks(), w.awake_chunks());
    // Everything saved comes back: saving again gives the same bytes.
    assert_eq!(save::encode(&loaded), bytes);
}

#[test]
fn a_loaded_tank_carries_on_exactly() {
    // Saved mid-run, with a rock falling through the water and another
    // tipping off the glued ledge's edge.
    let mut a = busy(11, 1300);
    let rock = id(&a, "rock");
    a.paint(150, 12, 4, rock, Brush::Pour).unwrap();
    a.fill_rect(rect(52, 50, 70, 54), rock, Brush::Pour)
        .unwrap();
    for _ in 0..25 {
        a.tick();
    }
    let mut b = reload(&a);
    for t in 0..2400 {
        a.tick();
        b.tick();
        if t % 100 == 0 {
            assert_eq!(a.state_hash(), b.state_hash(), "tick {t}");
        }
    }
    assert_eq!(a.state_hash(), b.state_hash());
    // The hash leaves out bodies; the saves cover them.
    assert_eq!(save::encode(&a), save::encode(&b));
}

#[test]
fn saving_at_any_tick_carries_on_exactly() {
    // Many save points through a pour, so mid-tick bookkeeping (chunk work,
    // held bodies, the random-tick rotation, field steps every 4th tick)
    // is caught at every phase.
    let mut a = busy(5, 0);
    let sand = id(&a, "sand");
    // The same input for the original and every fork: a pour over the
    // first 60 ticks.
    let step = |w: &mut World| {
        let t = w.tick_count();
        if t < 60 {
            w.paint(40 + t.cast_signed(), 10, 2, sand, Brush::Pour)
                .unwrap();
        }
        w.tick();
    };
    let mut forks = Vec::new();
    for t in 0..240_u32 {
        step(&mut a);
        if t % 23 == 0 {
            forks.push(reload(&a));
        }
    }
    for b in &mut forks {
        while b.tick_count() < a.tick_count() {
            step(b);
        }
        assert_eq!(b.state_hash(), a.state_hash(), "fork");
        assert_eq!(save::encode(b), save::encode(&a));
    }
}

#[test]
fn a_fresh_tank_compresses_well() {
    let w = World::new(WorldConfig::default(), table()).unwrap();
    let bytes = save::encode(&w);
    // 640 × 360 cells (900 KiB in memory): runs and untouched colour noise
    // compress to almost nothing.
    assert!(bytes.len() < 20_000, "{} bytes", bytes.len());
}

fn fixture(version: u16) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/v{version}.dtank"))
}

#[test]
fn a_version_1_fixture_loads() {
    let bytes = std::fs::read(fixture(1)).unwrap();
    assert_eq!(save::check(&bytes), Ok(1));
    let mut w = save::decode(&bytes, table()).unwrap();
    // What the fixture was written with (seed 1, 1200 ticks of `busy` as
    // it was then, with the first seven species; `busy` now adds every
    // species, so re-blessing writes a different, larger tank).
    assert_eq!((w.width(), w.height(), w.tick_count()), (200, 120, 1200));
    assert_eq!(w.plant_count(), 11);
    assert_eq!(w.creatures().len(), 7);
    // Version 1 creatures arrive as well-fed adults just mature, with the
    // default cap and easy mode off (T9's migration).
    for c in w.creatures() {
        assert_eq!(w.life_stage(c), LifeStage::Adult);
        let s = w.species().get(c.species()).unwrap();
        assert_eq!(c.age(), s.maturity_days * LIFE_DAY_TICKS);
        assert_eq!(c.energy(), ENERGY_RESERVE);
    }
    assert_eq!(w.creature_cap(), DEFAULT_CREATURE_CAP);
    assert!(!w.easy_mode() && w.corpses().is_empty());
    assert!(w.bodies().any(|b| b.anchored));
    let soil = id(&w, "aquasoil");
    assert!(w.material_counts()[usize::from(soil.0)] > 3000);
    // It runs on, and saves as version 2 that loads to the same state.
    for _ in 0..200 {
        w.tick();
    }
    assert_eq!(w.tick_count(), 1400);
    let again = save::encode(&w);
    assert_eq!(save::check(&again), Ok(2));
    assert_eq!(
        save::decode(&again, table()).unwrap().state_hash(),
        w.state_hash()
    );
}

/// What the version 2 fixture holds: `busy` with life state (a fry, a
/// hungry creature, a corpse, easy mode on and a cap of 40).
fn lively() -> World {
    let mut w = busy(3, 600);
    w.set_easy_mode(true);
    w.set_creature_cap(40);
    let tetra = w.species().id("neon_tetra").unwrap();
    w.add_creature_aged(tetra, 100, 50, 300).unwrap();
    let hungry = w.add_creature(tetra, 60, 50).unwrap();
    w.set_creature_energy(hungry, 5000);
    let old = w.add_creature(tetra, 140, 50).unwrap();
    let span = w.creature_lifespan(w.creature(old).unwrap());
    w.set_creature_age(old, span - 20);
    for _ in 0..100 {
        w.tick();
    }
    w
}

#[test]
fn a_version_2_fixture_loads() {
    let path = fixture(2);
    if std::env::var_os("DESKTANK_BLESS").is_some() {
        std::fs::write(&path, save::encode(&lively())).unwrap();
    }
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(save::check(&bytes), Ok(2));
    let mut w = save::decode(&bytes, table()).unwrap();
    assert_eq!(w.tick_count(), 700);
    assert!(w.easy_mode());
    assert_eq!(w.creature_cap(), 40);
    assert_eq!(w.corpses().len(), 1);
    assert!(
        w.creatures()
            .iter()
            .any(|c| w.life_stage(c) == LifeStage::Fry)
    );
    assert!(w.creatures().iter().any(|c| c.energy() < 5000));
    // Decoding is pure, so the decoded state is the same on every platform:
    // pin it, and the life state behind it.
    assert_eq!(w.state_hash(), 0xf7c4_4953_00e1_0fff);
    let life = |id: u32| {
        let c = w.creatures().iter().find(|c| c.id().0 == id).unwrap();
        (c.age(), c.growth(), c.energy(), c.stress())
    };
    assert_eq!(life(1), (2_160_700, 2_160_000, 575_643, 0));
    assert_eq!(life(12), (400, 400, 575_900, 0), "the fry");
    assert_eq!(life(13), (2_160_100, 2_160_000, 4900, 0), "the hungry one");
    let corpse = &w.corpses()[0];
    assert_eq!((corpse.died(), corpse.decay()), (620, 0.0));
    assert_eq!(corpse.cause(), desktank_sim::DeathCause::OldAge);
    for _ in 0..200 {
        w.tick();
    }
    assert_eq!(w.tick_count(), 900);
}

#[test]
fn rejects_damage() {
    let bytes = save::encode(&busy(2, 100));
    for i in [0, 9, 13, 30, bytes.len() / 2, bytes.len() - 1] {
        let mut bad = bytes.clone();
        bad[i] ^= 0x10;
        assert!(save::decode(&bad, table()).is_err(), "byte {i}");
    }
    assert!(matches!(
        save::decode(&bytes[..bytes.len() / 2], table()),
        Err(SaveError::Truncated { .. })
    ));
    assert_eq!(save::decode(&[], table()).err(), Some(SaveError::NotASave));
}
