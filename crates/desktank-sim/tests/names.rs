//! SPEC T9 (part 3) acceptance tests: a named creature keeps its name
//! across save and load; names are cleaned, optional and don't change how
//! a tank runs; young are born unnamed; and the inspect card shows the
//! life-cycle state.

use desktank_sim::save;
use desktank_sim::{
    Brush, CreatureId, LIFE_DAY_TICKS, LifeStage, MAX_NAME_CHARS, MaterialTable, Rect,
    SETTLE_TICKS, SpeciesId, Traits, World, WorldConfig,
};

fn world(seed: u64) -> World {
    let mut w = World::new(
        WorldConfig {
            width: 128,
            height: 96,
            seed,
            water_level: 0.95,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    let sand = w.materials().id("sand").unwrap();
    let floor = Rect {
        x0: 0,
        y0: 88,
        x1: 128,
        y1: 96,
    };
    w.fill_rect(floor, sand, Brush::Replace).unwrap();
    w
}

fn species(w: &World, name: &str) -> SpeciesId {
    w.species().id(name).unwrap()
}

fn adult(female: bool) -> Traits {
    Traits {
        female: Some(female),
        ..Traits::default()
    }
}

/// A tank with a few tetras and a guppy pair.
fn stocked(seed: u64) -> (World, Vec<CreatureId>) {
    let mut w = world(seed);
    let tetra = species(&w, "neon_tetra");
    let guppy = species(&w, "guppy");
    let mut ids: Vec<_> = (0..3)
        .map(|i| w.add_creature(tetra, 20 + 20 * i, 40).unwrap())
        .collect();
    for (x, female) in [(80, false), (100, true)] {
        ids.push(
            w.add_creature_with_traits(guppy, x, 50, adult(female))
                .unwrap(),
        );
    }
    (w, ids)
}

#[test]
fn a_named_creature_keeps_its_name_across_save_and_load() {
    let (mut w, ids) = stocked(1);
    assert_eq!(w.set_creature_name(ids[0], "Bubbles"), Some("Bubbles"));
    assert_eq!(w.set_creature_name(ids[3], "Señor Fin"), Some("Señor Fin"));
    for _ in 0..100 {
        w.tick();
    }
    let mut loaded = save::decode(&save::encode(&w), MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(loaded.creature(ids[0]).unwrap().name(), Some("Bubbles"));
    assert_eq!(loaded.creature(ids[3]).unwrap().name(), Some("Señor Fin"));
    assert_eq!(loaded.creature(ids[1]).unwrap().name(), None);
    assert_eq!(loaded.state_hash(), w.state_hash());
    // It carries on tick for tick, names and all.
    for _ in 0..200 {
        w.tick();
        loaded.tick();
    }
    assert_eq!(loaded.state_hash(), w.state_hash());
    assert_eq!(loaded.creature(ids[0]).unwrap().name(), Some("Bubbles"));
    // A rename and a cleared name are saved too.
    loaded.set_creature_name(ids[0], "Bubbles II");
    loaded.set_creature_name(ids[3], "");
    let again = save::decode(&save::encode(&loaded), MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(again.creature(ids[0]).unwrap().name(), Some("Bubbles II"));
    assert_eq!(again.creature(ids[3]).unwrap().name(), None);
}

#[test]
fn names_are_cleaned_and_only_name_creatures_that_exist() {
    let (mut w, ids) = stocked(2);
    assert_eq!(w.set_creature_name(ids[0], "  Mr \t Fin\n"), Some("Mr Fin"));
    let long = "y".repeat(MAX_NAME_CHARS * 2);
    let kept = w.set_creature_name(ids[0], &long).unwrap().to_owned();
    assert_eq!(kept.chars().count(), MAX_NAME_CHARS);
    assert_eq!(w.creature(ids[0]).unwrap().name(), Some(kept.as_str()));
    // Blank removes it.
    assert_eq!(w.set_creature_name(ids[0], " \t "), Some(""));
    assert_eq!(w.creature(ids[0]).unwrap().name(), None);
    assert_eq!(w.set_creature_name(CreatureId(999), "Nobody"), None);
}

#[test]
fn names_dont_change_how_a_tank_runs() {
    let (mut named, ids) = stocked(3);
    let (mut plain, _) = stocked(3);
    for &id in &ids {
        named.set_creature_name(id, "Pet");
    }
    for _ in 0..600 {
        named.tick();
        plain.tick();
    }
    let places = |w: &World| -> Vec<_> {
        w.creatures()
            .iter()
            .map(|c| (c.id(), c.centre(), c.age(), c.energy()))
            .collect()
    };
    assert_eq!(places(&named), places(&plain));
    // Unnamed, it hashes as a tank that never had names.
    for &id in &ids {
        named.set_creature_name(id, "");
    }
    assert_eq!(named.state_hash(), plain.state_hash());
}

#[test]
fn the_card_shows_the_life_cycle_and_young_are_born_unnamed() {
    let (mut w, ids) = stocked(4);
    let (male, female) = (ids[3], ids[4]);
    w.set_creature_name(female, "Mum");
    let card = w.creature_card(female).unwrap();
    assert_eq!(card.name.as_deref(), Some("Mum"));
    assert_eq!(card.species, "guppy");
    assert_eq!(card.stage, LifeStage::Adult);
    let maturity = w.species().get(species(&w, "guppy")).unwrap().maturity_days;
    assert_eq!(card.age_days, maturity);
    assert!(card.female && card.carrying_days.is_none());
    assert!((0.0..=1.0).contains(&card.health) && (0.0..=1.0).contains(&card.energy));
    // Guppies come in strains; the palette adds the wild one.
    assert_eq!(card.morph.as_deref(), Some("wild"));
    assert_eq!(
        card.summary(),
        format!(
            "Mum · guppy #{} · adult female · {maturity} days old · health {:.0}% · \
             energy {:.0}% · wild",
            female.0,
            card.health * 100.0,
            card.energy * 100.0
        )
    );
    // Neon tetras have one morph, so the card leaves it out.
    let tetra = w.creature_card(ids[0]).unwrap();
    assert_eq!((tetra.name, tetra.morph), (None, None));
    assert!(!w.creature_card(male).unwrap().female);
    assert!(w.creature_card(CreatureId(999)).is_none());

    // She conceives, and the card counts down her days to go.
    w.skip_breeding_time(SETTLE_TICKS);
    let carrying = (1..=30_000)
        .find(|_| {
            w.tick();
            w.creature(female).unwrap().carrying() > 0
        })
        .is_some();
    assert!(carrying, "she conceives");
    let card = w.creature_card(female).unwrap();
    assert_eq!(card.carrying_days, Some(28));
    assert!(card.summary().ends_with("carrying, 28 days to go"));
    // Her young are born unnamed; she keeps her name.
    let before = w.creatures().len();
    w.skip_breeding_time(28 * LIFE_DAY_TICKS);
    w.tick();
    assert!(w.creatures().len() > before, "she gives birth");
    let young: Vec<_> = w
        .creatures()
        .iter()
        .filter(|c| w.life_stage(c) == LifeStage::Fry)
        .collect();
    assert_ne!(young.len(), 0);
    assert!(young.iter().all(|c| c.name().is_none()));
    assert_eq!(w.creature(female).unwrap().name(), Some("Mum"));
}
