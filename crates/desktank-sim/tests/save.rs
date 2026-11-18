//! SPEC T8 acceptance tests (§2.6): a save round-trips to the same state
//! hash, a loaded tank carries on exactly as the saved one would have
//! (every part of the state that feeds later ticks is saved, including the
//! rigid bodies the hash leaves out), and a version 1 fixture loads.
//!
//! `DESKTANK_BLESS=1 cargo test -p desktank-sim --test save` rewrites the
//! fixture; only do that when the version 1 writer itself was wrong, since
//! the point of the fixture is that old saves keep loading.

#![allow(clippy::cast_possible_wrap)]

use std::path::PathBuf;

use desktank_sim::save::{self, FORMAT_VERSION, SaveError};
use desktank_sim::{Brush, MaterialId, MaterialTable, Placeable, Rect, World, WorldConfig};

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

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v1.dtank")
}

#[test]
fn a_version_1_fixture_loads() {
    let path = fixture();
    if std::env::var_os("DESKTANK_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, save::encode(&busy(1, 1200))).unwrap();
    }
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(save::check(&bytes), Ok(1));
    let mut w = save::decode(&bytes, table()).unwrap();
    // What the fixture was written with (seed 1, 1200 ticks of `busy` as
    // it was then, with the first seven species; `busy` now adds every
    // species, so re-blessing writes a different, larger tank).
    assert_eq!((w.width(), w.height(), w.tick_count()), (200, 120, 1200));
    assert_eq!(w.plant_count(), 11);
    assert_eq!(w.creatures().len(), 7);
    assert!(w.bodies().any(|b| b.anchored));
    let soil = id(&w, "aquasoil");
    assert!(w.material_counts()[usize::from(soil.0)] > 3000);
    // It runs on.
    for _ in 0..200 {
        w.tick();
    }
    assert_eq!(w.tick_count(), 1400);
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
