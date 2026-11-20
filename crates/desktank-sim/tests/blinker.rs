//! The cheat-only blinker (SPEC §2.5): `radiation leak` adds one 15 cm
//! three-eyed mutant fish, within the population cap. It swims mid-water
//! like a goldfish, never breeds, eats corpses and nearly everything
//! edible, swallows the smallest creatures when hungry, and leaves the most
//! waste of any species. A tank too shallow for its 22-cell body turns the
//! cheat down with the reason.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use desktank_sim::cheats::{self, BLINKER, CheatError};
use desktank_sim::{
    Brush, MaterialId, MaterialTable, Rect, SpeciesId, TICKS_PER_SECOND, World, WorldConfig,
    WorldError,
};

fn tank(width: usize, height: usize, seed: u64) -> World {
    World::new(
        WorldConfig {
            width,
            height,
            seed,
            water_level: 0.95,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap()
}

/// A 160×96 tank with a sand bed.
fn world(seed: u64) -> World {
    let mut w = tank(160, 96, seed);
    let sand = id(&w, "sand");
    w.fill_rect(
        Rect {
            x0: 0,
            y0: 88,
            x1: 160,
            y1: 96,
        },
        sand,
        Brush::Replace,
    )
    .unwrap();
    w
}

fn id(w: &World, name: &str) -> MaterialId {
    w.materials().id(name).unwrap()
}

fn species(w: &World, name: &str) -> SpeciesId {
    w.species().id(name).unwrap()
}

fn run(w: &mut World, seconds: u32) {
    for _ in 0..seconds * TICKS_PER_SECOND {
        w.tick();
    }
}

fn count(w: &World, name: &str) -> usize {
    w.material_counts()[usize::from(id(w, name).0)]
}

#[test]
fn the_cheat_adds_a_blinker_ignoring_case_and_spacing() {
    let mut w = world(1);
    let out = cheats::run(&mut w, "radiation leak").unwrap();
    assert_eq!(out.message, format!("spawned 1 {BLINKER}"));
    assert!(out.world_changed);
    let blinker = species(&w, BLINKER);
    assert_eq!(w.creatures()[0].species(), blinker);
    cheats::run(&mut w, "  RADIATION   Leak ").unwrap();
    assert_eq!(w.creatures().len(), 2);
    // The phrase is the whole line.
    for line in ["radiation", "leak", "radiation leak!", "radiation leak 2"] {
        assert!(cheats::run(&mut w, line).is_err(), "{line}");
    }
    assert_eq!(w.creatures().len(), 2);
    // Not in `spawn`, and `help` lists the phrase.
    let Err(CheatError::Usage(m)) = cheats::run(&mut w, "spawn blinker") else {
        panic!("spawn blinker ran");
    };
    let (_, offered) = m.split_once("one of").unwrap();
    assert!(!offered.contains(BLINKER), "{m}");
    let help = cheats::run(&mut w, "help").unwrap().message;
    assert!(help.contains("radiation leak"), "{help}");
}

#[test]
fn the_cheat_respects_the_population_cap() {
    let mut w = world(2);
    w.set_creature_cap(1);
    cheats::run(&mut w, "radiation leak").unwrap();
    assert_eq!(
        cheats::run(&mut w, "radiation leak"),
        Err(CheatError::World(WorldError::PopulationCap(1)))
    );
    assert_eq!(w.creatures().len(), 1);
}

/// The smallest tanks: too shallow or too narrow for its 30×22-cell body,
/// the cheat changes nothing and says why; a tank just big enough takes it.
#[test]
fn a_tank_too_small_for_it_turns_the_cheat_down() {
    let mut shallow = tank(120, 20, 3);
    let hash = shallow.state_hash();
    let Err(CheatError::Usage(m)) = cheats::run(&mut shallow, "radiation leak") else {
        panic!("a blinker fit 19 rows of water");
    };
    assert!(m.contains("needs 48 rows of water"), "{m}");
    assert_eq!(shallow.state_hash(), hash);
    assert_eq!(shallow.creatures().len(), 0);
    // Narrower than its body: no room, deep as the water is.
    let mut narrow = tank(24, 80, 3);
    assert_eq!(
        cheats::run(&mut narrow, "radiation leak"),
        Err(CheatError::World(WorldError::NoRoom))
    );
    // A tank just big enough holds it, and it swims there for a minute.
    let mut snug = tank(36, 28, 3);
    cheats::run(&mut snug, "radiation leak").unwrap();
    run(&mut snug, 60);
    assert_eq!(snug.creatures().len(), 1);
}

#[test]
fn it_swims_its_band_and_never_breeds() {
    for seed in [4, 5, 6] {
        let mut w = world(seed);
        for _ in 0..2 {
            cheats::run(&mut w, "radiation leak").unwrap();
        }
        let start: Vec<_> = w
            .creatures()
            .iter()
            .map(desktank_sim::Creature::centre)
            .collect();
        let mut in_zone = 0;
        let ticks = 180 * TICKS_PER_SECOND;
        for _ in 0..ticks {
            w.tick();
            in_zone += w.creatures().iter().filter(|c| w.in_zone(c)).count();
        }
        assert_eq!(w.creatures().len(), 2, "seed {seed}: no young, no deaths");
        // Mostly within its band, and it has moved.
        let share = in_zone as f32 / (2 * ticks) as f32;
        assert!(share > 0.8, "seed {seed}: in its band {share}");
        for (c, s) in w.creatures().iter().zip(start) {
            let (x, y) = c.centre();
            assert!((x - s.0).abs() + (y - s.1).abs() > 1.0, "seed {seed} stuck");
        }
    }
}

#[test]
fn it_leaves_the_most_waste() {
    let mut w = world(7);
    w.add_creature(species(&w, BLINKER), 80, 50).unwrap();
    run(&mut w, 60);
    // Waste rots into detritus, which it eats: count what is left plus
    // what it ate.
    let droppings = count(&w, "waste") + count(&w, "detritus");
    let eaten = w.creatures()[0].eaten() as usize;
    assert!(
        (15..=21).contains(&(droppings + eaten)),
        "{droppings} droppings and {eaten} bites in a minute"
    );
}

#[test]
fn a_hungry_blinker_swallows_a_micro_crab_but_not_an_adult_tetra() {
    let mut w = tank(40, 30, 8);
    let blinker = w.add_creature(species(&w, BLINKER), 20, 14).unwrap();
    let tetra = w.add_creature(species(&w, "neon_tetra"), 4, 14).unwrap();
    let crab = w
        .add_creature(species(&w, "thai_micro_crab"), 20, 27)
        .unwrap();
    let energy = desktank_sim::ENERGY_RESERVE / 2;
    w.set_creature_energy(blinker, energy);
    let eaten = (1..=180 * TICKS_PER_SECOND).find(|_| {
        w.tick();
        w.set_creature_energy(blinker, energy.min(w.creature(blinker).unwrap().energy()));
        w.creature(crab).is_none()
    });
    assert!(eaten.is_some(), "the blinker never ate the micro crab");
    assert!(w.creature(tetra).is_some(), "an adult tetra is too big");
    assert!(w.creature(blinker).unwrap().eaten() >= 1);
}
