//! The cheat-only crabs (SPEC §2.5): `why not?` adds a medical crab and
//! `home owner!` a home owning crab, within the population cap. Both are
//! peaceful bottom walkers that never breed, eat corpses and everything
//! edible that isn't alive, and leave far more waste than any stocked
//! species; the home owner picks a home on the substrate, plants a small
//! carpet patch there, keeps near it, and keeps its home across a save.
#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]

use desktank_sim::cheats::{self, CheatError, HOME_OWNING_CRAB, MEDICAL_CRAB};
use desktank_sim::plants::Form;
use desktank_sim::save;
use desktank_sim::{
    Brush, CreatureId, MaterialId, MaterialTable, Rect, SpeciesId, TICKS_PER_SECOND, World,
    WorldConfig, WorldError,
};

/// Tank size and the top row of its aquasoil bed.
const WIDTH: usize = 160;
const HEIGHT: usize = 96;
const BED: usize = 86;
/// Plantlets a home owner pours (`habits::PATCH`).
const PATCH: usize = 3;
/// How far from home a home owner wanders (`habits::HOME_RANGE`).
const HOME_RANGE: usize = 12;

fn world(seed: u64) -> World {
    let mut w = World::new(
        WorldConfig {
            width: WIDTH,
            height: HEIGHT,
            seed,
            water_level: 0.95,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    let soil = id(&w, "aquasoil");
    w.fill_rect(rect(0, BED, WIDTH, HEIGHT), soil, Brush::Replace)
        .unwrap();
    w.set_easy_mode(true);
    w
}

fn id(w: &World, name: &str) -> MaterialId {
    w.materials().id(name).unwrap()
}

fn species(w: &World, name: &str) -> SpeciesId {
    w.species().id(name).unwrap()
}

fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Rect {
    Rect { x0, y0, x1, y1 }
}

fn run(w: &mut World, seconds: u32) {
    for _ in 0..seconds * TICKS_PER_SECOND {
        w.tick();
    }
}

fn count(w: &World, name: &str) -> usize {
    w.material_counts()[usize::from(id(w, name).0)]
}

fn only(w: &World) -> CreatureId {
    assert_eq!(w.creatures().len(), 1);
    w.creatures()[0].id()
}

#[test]
fn each_cheat_adds_its_crab_ignoring_case_and_spacing() {
    let mut w = world(1);
    let out = cheats::run(&mut w, "why not?").unwrap();
    assert_eq!(out.message, format!("spawned 1 {MEDICAL_CRAB}"));
    assert!(out.world_changed);
    assert_eq!(w.creatures()[0].species(), species(&w, MEDICAL_CRAB));
    cheats::run(&mut w, "  HOME   Owner!  ").unwrap();
    assert_eq!(w.creatures()[1].species(), species(&w, HOME_OWNING_CRAB));
    cheats::run(&mut w, "Why Not?").unwrap();
    assert_eq!(w.creatures().len(), 3);
    // The phrase is the whole line, with its punctuation.
    for line in ["why", "why not", "why not? 2", "home owner", "home"] {
        assert!(cheats::run(&mut w, line).is_err(), "{line}");
    }
    assert_eq!(w.creatures().len(), 3);
    // They aren't in `spawn`, and `help` lists both phrases.
    for name in [MEDICAL_CRAB, HOME_OWNING_CRAB] {
        let Err(CheatError::Usage(m)) = cheats::run(&mut w, &format!("spawn {name}")) else {
            panic!("spawn {name} ran");
        };
        let (_, offered) = m.split_once("one of").unwrap();
        assert!(!offered.contains(name), "{m}");
    }
    let help = cheats::run(&mut w, "help").unwrap().message;
    assert!(
        help.contains("why not?") && help.contains("home owner!"),
        "{help}"
    );
}

#[test]
fn the_cheats_respect_the_population_cap() {
    let mut w = world(2);
    w.set_creature_cap(2);
    cheats::run(&mut w, "why not?").unwrap();
    cheats::run(&mut w, "home owner!").unwrap();
    for line in ["why not?", "home owner!"] {
        assert_eq!(
            cheats::run(&mut w, line),
            Err(CheatError::World(WorldError::PopulationCap(2)))
        );
    }
    assert_eq!(w.creatures().len(), 2);
}

#[test]
fn crabs_never_breed() {
    let mut w = world(3);
    for _ in 0..3 {
        cheats::run(&mut w, "why not?").unwrap();
        cheats::run(&mut w, "home owner!").unwrap();
    }
    cheats::run(&mut w, "feed").unwrap();
    let ids: Vec<_> = w
        .creatures()
        .iter()
        .map(desktank_sim::Creature::id)
        .collect();
    run(&mut w, 600);
    let now: Vec<_> = w
        .creatures()
        .iter()
        .map(desktank_sim::Creature::id)
        .collect();
    assert_eq!(now, ids, "no new creatures in 10 minutes");
    assert!(
        w.species()
            .iter()
            .filter(|s| s.cheat_only)
            .all(|s| s.breeding.is_none())
    );
}

#[test]
fn a_crab_leaves_far_more_waste_than_a_stocked_species() {
    // The waste in the tank after `seconds` with one creature of `name`.
    let waste_after = |name: &str, seconds: u32| {
        let mut w = world(4);
        let s = species(&w, name);
        w.add_creature(s, 80, 80).unwrap();
        run(&mut w, seconds);
        // Waste rots into detritus, which the crab eats: count droppings
        // as waste and detritus made, plus what it ate.
        count(&w, "waste") + count(&w, "detritus")
    };
    assert_eq!(waste_after("amano_shrimp", 60), 0);
    assert_eq!(waste_after("corydoras", 60), 0);
    let crab = waste_after(MEDICAL_CRAB, 60);
    assert!((10..=16).contains(&crab), "{crab} droppings in a minute");
    // The bioload is higher than every other species'.
    let w = world(4);
    let crab = w
        .species()
        .get(species(&w, MEDICAL_CRAB))
        .unwrap()
        .bioload();
    for s in w.species().iter().filter(|s| !s.cheat_only) {
        assert!(s.bioload() < crab, "{}", s.name);
    }
}

#[test]
fn a_crab_bites_corpses_but_not_plants() {
    let mut w = world(5);
    // A tetra dies of old age just beside the crab, on the bed.
    let crab = species(&w, MEDICAL_CRAB);
    let c = w.add_creature(crab, 80, 82).unwrap();
    let tetra = species(&w, "neon_tetra");
    let t = w.add_creature(tetra, 85, 84).unwrap();
    let span = w.creature_lifespan(w.creature(t).unwrap());
    w.set_creature_age(t, span);
    w.tick();
    assert_eq!(w.corpses().len(), 1);
    // Well before a corpse starts to rot (60 s), the crab has bitten it.
    let mut bitten = false;
    for _ in 0..30 * TICKS_PER_SECOND {
        w.tick();
        if w.corpses().first().is_none_or(|k| k.decay() > 0.0) {
            bitten = true;
            break;
        }
    }
    assert!(bitten, "the crab never bit the corpse");
    assert!(w.creature(c).unwrap().eaten() > 0);
    // It never eats live plant parts or propagules.
    for name in ["leaf", "stem", "root", "runner", "seed", "cutting"] {
        assert!(
            !w.species().get(crab).unwrap().eats(id(&w, name)),
            "eats {name}"
        );
    }
}

#[test]
fn the_home_owner_plants_a_bounded_patch_at_its_home_and_stays_near() {
    for seed in [6, 9, 10] {
        home_owner(seed);
    }
}

fn home_owner(seed: u64) {
    let mut w = world(seed);
    cheats::run(&mut w, "home owner!").unwrap();
    let crab = only(&w);
    // It lands on the bed and claims a home within half a minute.
    run(&mut w, 30);
    let (hx, hy) = w.creature(crab).unwrap().home().expect("a home");
    assert_eq!(hy, BED, "the home is the top of the bed");
    let carpet = w.plant_species().id("dwarf_hairgrass").unwrap();
    assert!(matches!(
        w.plant_species().get(carpet).unwrap().form,
        Form::Carpet(_)
    ));
    // A handful of plantlets, once.
    let poured = count(&w, "cutting") + w.plant_count();
    assert!((1..=PATCH).contains(&poured), "{poured} plantlets");
    // The patch roots near the home, and the crab stays close by.
    let mut farthest = 0;
    for _ in 0..180 {
        run(&mut w, 1);
        let x = usize::try_from(w.creature(crab).unwrap().cell().0).unwrap();
        farthest = farthest.max(x.abs_diff(hx));
        assert_eq!(w.creature(crab).unwrap().home(), Some((hx, hy)));
    }
    assert!(farthest <= HOME_RANGE + 4, "{farthest} cells from home");
    let tufts: Vec<_> = w.plants().filter(|p| p.species == carpet).collect();
    assert!(!tufts.is_empty(), "the patch rooted");
    // It never plants a second patch: the first tufts are within the
    // patch, and later ones are their runners' children.
    for t in &tufts {
        if t.generation == 0 {
            assert!(t.base.0.abs_diff(hx) <= 4, "a tuft at {:?}", t.base);
        }
    }
    assert!(tufts.iter().filter(|t| t.generation == 0).count() <= PATCH);
}

#[test]
fn the_medical_crab_keeps_no_home() {
    let mut w = world(7);
    cheats::run(&mut w, "why not?").unwrap();
    run(&mut w, 10);
    assert_eq!(w.creatures()[0].home(), None);
    assert_eq!(w.plant_count() + count(&w, "cutting"), 0);
}

#[test]
fn the_home_survives_a_save_and_load() {
    let mut w = world(8);
    cheats::run(&mut w, "home owner!").unwrap();
    cheats::run(&mut w, "why not?").unwrap();
    run(&mut w, 30);
    let home = w.creatures()[0].home();
    assert!(home.is_some());
    let bytes = save::encode(&w);
    assert_eq!(save::check(&bytes), Ok(save::FORMAT_VERSION));
    let mut loaded = save::decode(&bytes, MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(loaded.state_hash(), w.state_hash());
    assert_eq!(loaded.creatures()[0].home(), home);
    assert_eq!(loaded.creatures()[1].home(), None);
    for _ in 0..30 * TICKS_PER_SECOND {
        w.tick();
        loaded.tick();
    }
    assert_eq!(loaded.state_hash(), w.state_hash());
    assert_eq!(loaded.creatures()[0].home(), home);
}
