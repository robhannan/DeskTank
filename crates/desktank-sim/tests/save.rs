//! SPEC T8 acceptance tests (§2.6): a save round-trips to the same state
//! hash, a loaded tank carries on exactly as the saved one would have
//! (every part of the state that feeds later ticks is saved, including the
//! rigid bodies the hash leaves out), and the version 1 to 8 fixtures
//! load (version 1 migrated to 2, T9; version 2 to 3, T9 part 2; version
//! 3 to 4, D21; version 4 to 5, the crabs' homes; version 5 to 6, names;
//! version 6 to 7, floaters' rafts; version 7 to 8, plant colouration).
//!
//! The version 1 to 7 fixtures are frozen: this build no longer writes
//! them. `DESKTANK_BLESS=1 cargo test -p desktank-sim --test save` rewrites
//! the version 8 fixture; only do that while version 8 is unreleased or its
//! writer was wrong, since the point of a fixture is that old saves keep
//! loading.

#![allow(clippy::cast_possible_wrap)]

use std::path::PathBuf;

use desktank_sim::save::{self, FORMAT_VERSION, SaveError};
use desktank_sim::{
    Brush, DEFAULT_CREATURE_CAP, ENERGY_RESERVE, LIFE_DAY_TICKS, LifeStage, MaterialId,
    MaterialTable, Placeable, Rect, SETTLE_TICKS, Traits, World, WorldConfig,
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
    // Every species but the blinker, added after the pinned fixtures were
    // written (it would move their hashes; `tests/blinker.rs` covers its
    // save round trip).
    let species: Vec<_> = w
        .species()
        .iter()
        .filter(|s| s.name != "blinker")
        .map(|s| s.id)
        .collect();
    let n = species.len() as i64;
    for (k, s) in species.into_iter().enumerate() {
        let (top, bottom) = w.zone_rows(s).unwrap();
        let y = i64::try_from(usize::midpoint(top, bottom)).unwrap();
        // Room for a creature is not guaranteed; a full spot is fine.
        let _ = w.add_creature(s, 15 + 175 * k as i64 / n, y);
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
    // It runs on, and saves as the current version that loads to the same
    // state.
    for _ in 0..200 {
        w.tick();
    }
    assert_eq!(w.tick_count(), 1400);
    let again = save::encode(&w);
    assert_eq!(save::check(&again), Ok(FORMAT_VERSION));
    assert_eq!(
        save::decode(&again, table()).unwrap().state_hash(),
        w.state_hash()
    );
}

/// The version 2 fixture holds `busy` (seed 3, 600 ticks, as it was then)
/// with life state: a fry, a hungry creature, a corpse, easy mode on and a
/// cap of 40, run 100 ticks more.
#[test]
fn a_version_2_fixture_loads() {
    let bytes = std::fs::read(fixture(2)).unwrap();
    assert_eq!(save::check(&bytes), Ok(2));
    let mut w = save::decode(&bytes, table()).unwrap();
    // Version 2 had no homes.
    assert!(w.creatures().iter().all(|c| c.home().is_none()));
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
    // Version 3 (T9 part 2): each creature shows its palette morph with two
    // alleles of it, the sexes alternate within a species in id order, and
    // each settles for a day before breeding; nothing is carrying and
    // there are no clutches.
    let mut seen = std::collections::BTreeMap::new();
    for c in w.creatures() {
        let s = w.species().get(c.species()).unwrap();
        assert_eq!(c.alleles(), [s.palette_morph; 2]);
        assert_eq!(w.creature_morph(c), s.palette_morph);
        let n: &mut u32 = seen.entry(c.species()).or_default();
        assert_eq!(c.female(), *n % 2 == 1, "{:?}", c.id());
        *n += 1;
        assert_eq!((c.carrying(), c.rest()), (0, SETTLE_TICKS));
    }
    assert_eq!(w.clutches().len(), 0);
    assert!(!w.corpses()[0].female());
    // Version 4 (D21) puts each creature at rest in its starting band: a
    // hash of the seed and its id, so they spread over the bands.
    assert_at_rest_in_bands(&w);
    // Decoding is pure, so the decoded state is the same on every platform:
    // pin it, and the life state behind it.
    assert_eq!(w.state_hash(), 0xf6de_4336_106b_62f3);
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

/// What the version 3 and 4 fixtures hold: `busy` with breeding state (T9 part
/// 2): a blue cherry shrimp pair and a red one, a female guppy carrying a
/// red male's young, a corydoras clutch and a corpse of a blue shrimp.
fn bred() -> World {
    let mut w = busy(5, 600);
    let get = |w: &World, name: &str| w.species().id(name).unwrap();
    let (shrimp, guppy, cory) = (
        get(&w, "cherry_shrimp"),
        get(&w, "guppy"),
        get(&w, "corydoras"),
    );
    let morph = |w: &World, s, name: &str| w.species().get(s).unwrap().morph(name).unwrap();
    let (blue, red) = (morph(&w, shrimp, "blue"), morph(&w, guppy, "red"));
    let traits = |female: bool, morph: u8| Traits {
        female: Some(female),
        alleles: Some([morph, morph]),
        ..Traits::default()
    };
    for (x, female) in [(30, false), (40, true)] {
        w.add_creature_with_traits(shrimp, x, 100, traits(female, blue))
            .unwrap();
    }
    w.add_creature_with_traits(guppy, 90, 40, traits(false, red))
        .unwrap();
    let mum = w
        .add_creature_with_traits(guppy, 100, 40, traits(true, 0))
        .unwrap();
    for (x, female) in [(150, false), (160, true)] {
        w.add_creature_with_traits(cory, x, 95, traits(female, 0))
            .unwrap();
    }
    w.skip_breeding_time(SETTLE_TICKS);
    // Until the guppy carries and the corydoras has laid (seeded: a few
    // minutes of world time).
    for _ in 0..40_000 {
        w.tick();
        if w.creature(mum).unwrap().carrying() > 0 && !w.clutches().is_empty() {
            break;
        }
    }
    let dying = w
        .add_creature_with_traits(shrimp, 60, 100, traits(false, blue))
        .unwrap();
    let span = w.creature_lifespan(w.creature(dying).unwrap());
    w.set_creature_age(dying, span);
    for _ in 0..20 {
        w.tick();
    }
    w
}

#[test]
fn a_version_3_fixture_loads() {
    let bytes = std::fs::read(fixture(3)).unwrap();
    assert_eq!(save::check(&bytes), Ok(3));
    let mut w = save::decode(&bytes, table()).unwrap();
    let guppy = w.species().id("guppy").unwrap();
    let carrying = w
        .creatures()
        .iter()
        .find(|c| c.species() == guppy && c.carrying() > 0)
        .unwrap();
    assert!(carrying.female());
    assert_eq!(carrying.alleles(), [0, 0]);
    let clutch = &w.clutches()[0];
    assert_eq!(clutch.species(), w.species().id("corydoras").unwrap());
    assert_ne!(clutch.eggs().len(), 0);
    let egg = id(&w, "egg");
    for &e in clutch.eggs() {
        assert_eq!(w.cells()[e as usize].material, egg);
        assert_eq!(w.cells()[e as usize].state, clutch.id());
    }
    let shrimp = w.species().id("cherry_shrimp").unwrap();
    let blue = w.species().get(shrimp).unwrap().morph("blue").unwrap();
    assert!(w.corpses().iter().any(|c| c.morph() == blue));
    assert_at_rest_in_bands(&w);
    assert!(w.corpses().iter().all(|c| c.band() == 0));
    // Decoding is pure, so the decoded state is the same on every platform:
    // pin it.
    let hash = w.state_hash();
    assert_eq!(hash, 0xa27c_ed7f_1831_9970);
    // It carries on, and round-trips mid-pregnancy tick for tick.
    let again = save::decode(&save::encode(&w), table()).unwrap();
    assert_eq!(again.state_hash(), hash);
    let mut again = again;
    for _ in 0..200 {
        w.tick();
        again.tick();
    }
    assert_eq!(w.state_hash(), again.state_hash());
}

#[test]
fn a_version_4_fixture_loads() {
    let bytes = std::fs::read(fixture(4)).unwrap();
    assert_eq!(save::check(&bytes), Ok(4));
    let mut w = save::decode(&bytes, table()).unwrap();
    assert!(!w.clutches().is_empty() && !w.corpses().is_empty());
    // Version 5 migrates it with no homes.
    assert!(w.creatures().iter().all(|c| c.home().is_none()));
    // Decoding is pure, so the decoded state is the same on every platform:
    // pin it. A creature without a home hashes as before version 5, so this
    // is the hash version 4 pinned.
    let hash = w.state_hash();
    assert_eq!(hash, 0x07c8_5bc4_801d_be6e);
    let again = save::decode(&save::encode(&w), table()).unwrap();
    assert_eq!(again.state_hash(), hash);
    let mut again = again;
    for _ in 0..200 {
        w.tick();
        again.tick();
    }
    assert_eq!(w.state_hash(), again.state_hash());
}

/// A creature's id, depth and home.
type Saved = (u32, f32, Option<(usize, usize)>);

/// What the version 5 fixture holds: [`bred`] (with every species, the
/// cheat-only crabs too) and a home owning crab on the sand that has had
/// time to pick its home.
fn homely() -> World {
    let mut w = bred();
    let owner = w.species().id("home_owning_crab").unwrap();
    w.add_creature(owner, 170, 94).unwrap();
    for _ in 0..600 {
        w.tick();
    }
    w
}

#[test]
fn a_version_5_fixture_loads() {
    let bytes = std::fs::read(fixture(5)).unwrap();
    assert_eq!(save::check(&bytes), Ok(5));
    let mut w = save::decode(&bytes, table()).unwrap();
    assert_ne!(w.clutches().len(), 0);
    // Every home owning crab picked its home, and only they have one.
    let owner = w.species().id("home_owning_crab").unwrap();
    for c in w.creatures() {
        assert_eq!(c.home().is_some(), c.species() == owner, "{:?}", c.id());
    }
    assert!(w.creatures().iter().any(|c| c.species() == owner));
    // Version 6 migrates it with no names.
    assert!(w.creatures().iter().all(|c| c.name().is_none()));
    // Homes and depth are saved: decoded, it matches the tank that wrote it.
    let fresh = homely();
    let saved = |w: &World| -> Vec<Saved> {
        w.creatures()
            .iter()
            .map(|c| (c.id().0, c.depth(), c.home()))
            .collect()
    };
    assert_eq!(saved(&w), saved(&fresh));
    assert_eq!(w.state_hash(), fresh.state_hash());
    // Pinned, so every platform decodes the same state. An unnamed
    // creature hashes as before version 6, so this is the hash version 5
    // pinned.
    assert_eq!(w.state_hash(), 0x38d9_9bb5_afdb_8907);
    let again = save::decode(&save::encode(&w), table()).unwrap();
    let mut again = again;
    for _ in 0..200 {
        w.tick();
        again.tick();
    }
    assert_eq!(w.state_hash(), again.state_hash());
}

/// What the version 6 fixture holds: [`homely`] with three creatures
/// named (one with a name outside ASCII) and one named and then unnamed.
fn named() -> World {
    let mut w = homely();
    let ids: Vec<_> = w
        .creatures()
        .iter()
        .map(desktank_sim::Creature::id)
        .collect();
    for (id, name) in ids.iter().zip(["Bubbles", "Señor Fin 🐟", "Dot", "Gone"]) {
        assert!(w.set_creature_name(*id, name).is_some());
    }
    assert_eq!(w.set_creature_name(ids[3], "  "), Some(""));
    w
}

#[test]
fn a_version_6_fixture_loads() {
    let bytes = std::fs::read(fixture(6)).unwrap();
    assert_eq!(save::check(&bytes), Ok(6));
    let mut w = save::decode(&bytes, table()).unwrap();
    // Names are saved: decoded, they match the tank that wrote it.
    let fresh = named();
    let names = |w: &World| -> Vec<(u32, Option<String>)> {
        w.creatures()
            .iter()
            .map(|c| (c.id().0, c.name().map(str::to_owned)))
            .collect()
    };
    assert_eq!(names(&w), names(&fresh));
    let given: Vec<_> = w.creatures().iter().filter_map(|c| c.name()).collect();
    assert_eq!(given, ["Bubbles", "Señor Fin 🐟", "Dot"]);
    assert_eq!(w.state_hash(), fresh.state_hash());
    // Pinned, so every platform decodes the same state.
    assert_eq!(w.state_hash(), 0x029c_0413_f990_0954);
    let again = save::decode(&save::encode(&w), table()).unwrap();
    let mut again = again;
    for _ in 0..200 {
        w.tick();
        again.tick();
    }
    assert_eq!(w.state_hash(), again.state_hash());
    assert_eq!(names(&w), names(&again));
}

/// A tank of floaters (T6c part 2) drifting in a filter's current, with
/// duckweed budding and frogbit hanging roots, run for 40 s.
fn floating() -> World {
    let mut w = World::new(
        WorldConfig {
            width: 160,
            height: 64,
            seed: 7,
            water_level: 0.9,
        },
        table(),
    )
    .unwrap();
    let soil = id(&w, "aquasoil");
    w.fill_rect(rect(0, 56, 160, 64), soil, Brush::Replace)
        .unwrap();
    let row = w.surface_row() as i64;
    w.place(Placeable::FilterOutlet, 2, row + 2).unwrap();
    for (k, name) in ["duckweed", "frogbit", "duckweed", "frogbit"]
        .into_iter()
        .enumerate()
    {
        let s = w.plant_species().id(name).unwrap();
        w.paint_plant(
            20 + 30 * k as i64,
            row + 4,
            0,
            s,
            desktank_sim::Propagule::Buds,
        )
        .unwrap();
    }
    for _ in 0..800 {
        w.tick();
    }
    w
}

#[test]
fn a_version_7_fixture_loads() {
    let bytes = std::fs::read(fixture(7)).unwrap();
    assert_eq!(save::check(&bytes), Ok(7));
    let mut w = save::decode(&bytes, table()).unwrap();
    let mut fresh = floating();
    assert_eq!(w.state_hash(), fresh.state_hash());
    // Pinned, so every platform decodes the same state.
    assert_eq!(w.state_hash(), 0x7dae_ff6d_93e9_fadf);
    let floaters = |w: &World| {
        w.plants()
            .filter(|p| {
                let s = w.plant_species().get(p.species).unwrap();
                s.form.floater().is_some()
            })
            .count()
    };
    assert!(floaters(&w) >= 4, "{} floaters", floaters(&w));
    // The rafts are saved: it drifts on exactly as the tank that wrote it.
    for _ in 0..400 {
        w.tick();
        fresh.tick();
    }
    assert_eq!(w.state_hash(), fresh.state_hash());
    // Version 8 migrates it uncoloured.
    assert!(w.plants().all(|p| p.colour == 0.0));
}

/// A shallow, brightly lit aquasoil tank (T6d) with rotala, ludwigia and
/// hygrophila grown for 3 minutes: the rotala and ludwigia colour, the
/// hygrophila doesn't.
fn coloured() -> World {
    let mut w = World::new(
        WorldConfig {
            width: 96,
            height: 40,
            seed: 11,
            water_level: 0.95,
        },
        table(),
    )
    .unwrap();
    let soil = id(&w, "aquasoil");
    w.fill_rect(rect(0, 32, 96, 40), soil, Brush::Replace)
        .unwrap();
    for (x, name) in [(20, "rotala"), (48, "ludwigia"), (76, "hygrophila")] {
        let s = w.plant_species().id(name).unwrap();
        w.paint_plant(x, 30, 0, s, desktank_sim::Propagule::Cuttings)
            .unwrap();
    }
    for _ in 0..3600 {
        w.tick();
    }
    w
}

#[test]
fn a_version_8_fixture_loads() {
    let path = fixture(8);
    if std::env::var_os("DESKTANK_BLESS").is_some() {
        std::fs::write(&path, save::encode(&coloured())).unwrap();
    }
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(save::check(&bytes), Ok(8));
    let mut w = save::decode(&bytes, table()).unwrap();
    let mut fresh = coloured();
    assert_eq!(w.state_hash(), fresh.state_hash());
    let colours =
        |w: &World| -> Vec<(u16, f32)> { w.plants().map(|p| (p.id.0, p.colour)).collect() };
    assert_eq!(colours(&w), colours(&fresh));
    assert!(
        w.plants().filter(|p| p.colour > 0.2).count() >= 2,
        "{:?}",
        colours(&w)
    );
    for _ in 0..400 {
        w.tick();
        fresh.tick();
    }
    assert_eq!(w.state_hash(), fresh.state_hash());
}

/// A migrated tank's creatures rest whole bands deep, spread over them.
fn assert_at_rest_in_bands(w: &World) {
    let bands: std::collections::BTreeSet<u8> = w
        .creatures()
        .iter()
        .map(desktank_sim::Creature::band)
        .collect();
    for c in w.creatures() {
        assert_eq!(c.depth(), f32::from(c.band()));
    }
    assert!(bands.len() >= 4, "bands {bands:?}");
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
