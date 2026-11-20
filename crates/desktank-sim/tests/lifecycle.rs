//! SPEC T9 (part 1) acceptance tests: a test for each cause of death so far
//! (old age, starvation, poor water quality), easy mode allowing only
//! old-age deaths, corpses that sink and turn into detritus, growth that
//! depends on feeding, the population cap, and the new state surviving a
//! save and load.
#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]

use desktank_sim::save;
use desktank_sim::{
    BITE_ENERGY, Brush, CreatureId, DEFAULT_CREATURE_CAP, DeathCause, ENERGY_RESERVE, FieldKind,
    LIFE_DAY_TICKS, LifeStage, MAX_CREATURE_CAP, MaterialId, MaterialTable, POOR_QUALITY,
    ROT_AFTER_SECONDS, ROT_EVERY_SECONDS, Rect, STRESS_LIMIT, SpeciesId, TICKS_PER_SECOND, World,
    WorldConfig, WorldError,
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
    let sand = id(&w, "sand");
    w.fill_rect(rect(0, 88, 128, 96), sand, Brush::Replace)
        .unwrap();
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

fn seconds(s: u32) -> u32 {
    s * TICKS_PER_SECOND
}

/// Ticks until `id` is gone, at most `limit`.
fn ticks_to_death(w: &mut World, id: CreatureId, limit: u32) -> Option<u32> {
    for t in 1..=limit {
        w.tick();
        if w.creature(id).is_none() {
            return Some(t);
        }
    }
    None
}

/// A tetra in mid water.
fn tetra(w: &mut World) -> CreatureId {
    let s = species(w, "neon_tetra");
    w.add_creature(s, 64, 50).unwrap()
}

/// Makes the water foul: a layer of waste and detritus on the bed (over
/// 20 per 1000 water cells) and nutrients well past the bad level.
fn foul(w: &mut World) {
    let (waste, detritus) = (id(w, "waste"), id(w, "detritus"));
    w.fill_rect(rect(0, 84, 128, 86), waste, Brush::Replace)
        .unwrap();
    w.fill_rect(rect(0, 86, 128, 88), detritus, Brush::Replace)
        .unwrap();
    for y in (0..96).step_by(8) {
        for x in (0..128).step_by(8) {
            w.add_to_field(FieldKind::Nutrient, x, y, 3.0);
        }
    }
}

#[test]
fn old_age_kills_at_the_lifespan() {
    let mut w = world(1);
    let s = species(&w, "neon_tetra");
    let lifespan_days = w.species().get(s).unwrap().lifespan_days;
    // Each creature's own lifespan is within ±10% of the species' 5 years,
    // and they differ.
    let ids: Vec<_> = (0..8)
        .map(|i| w.add_creature(s, 10 + 14 * i, 50).unwrap())
        .collect();
    let spans: Vec<_> = ids
        .iter()
        .map(|&a| w.creature_lifespan(w.creature(a).unwrap()))
        .collect();
    let span = f64::from(lifespan_days);
    for &own in &spans {
        let days = f64::from(own) / f64::from(LIFE_DAY_TICKS);
        assert!((0.9 * span..=1.1 * span).contains(&days), "{days}");
    }
    assert!(spans.iter().any(|&s| s != spans[0]));
    let old = ids[3];
    let own = spans[3];
    for &a in &ids {
        if a != old {
            w.remove_creature(a);
        }
    }
    w.set_creature_age(old, own * 85 / 100);
    assert_eq!(w.life_stage(w.creature(old).unwrap()), LifeStage::Elder);
    w.set_creature_age(old, own * 75 / 100);
    assert_eq!(w.life_stage(w.creature(old).unwrap()), LifeStage::Adult);
    w.set_creature_age(old, own - 40);
    let died = ticks_to_death(&mut w, old, 100).expect("dies of old age");
    assert_eq!(died, 40);
    assert_eq!(w.corpses().len(), 1);
    assert_eq!(w.corpses()[0].cause(), DeathCause::OldAge);
    assert_eq!(w.corpses()[0].species(), s);
}

#[test]
fn a_starving_creature_dies_and_a_fed_one_lives() {
    let mut w = world(2);
    let hungry = tetra(&mut w);
    assert!(w.set_creature_energy(hungry, 100));
    assert_eq!(ticks_to_death(&mut w, hungry, 200), Some(100));
    assert_eq!(w.corpses()[0].cause(), DeathCause::Starvation);

    // With food near, the same creature eats and lives on.
    let mut w = world(2);
    let fed = tetra(&mut w);
    w.set_creature_energy(fed, 100);
    let flake = id(&w, "flake_food");
    w.paint(64, 46, 1, flake, Brush::Pour).unwrap();
    assert_eq!(ticks_to_death(&mut w, fed, 400), None);
    let c = w.creature(fed).unwrap();
    assert!(c.eaten() > 0);
    assert!(c.energy() > BITE_ENERGY / 2, "{}", c.energy());
}

#[test]
fn foul_water_kills_slowly_and_clean_water_doesnt() {
    let mut w = world(3);
    foul(&mut w);
    let q = w.water_quality();
    assert!(q < 0.05, "{q}");
    assert!((q - w.ecosystem().quality).abs() < 1e-4);
    let a = tetra(&mut w);
    // Gently: still alive after four minutes in the foulest water.
    assert_eq!(ticks_to_death(&mut w, a, seconds(240)), None);
    assert!(w.creature(a).unwrap().stress() > STRESS_LIMIT / 2);
    assert!(w.creature(a).unwrap().health() < 0.5);
    let died = ticks_to_death(&mut w, a, seconds(15 * 60)).expect("dies of poor water");
    assert!(died > 0);
    assert_eq!(w.corpses()[0].cause(), DeathCause::PoorWater);

    // The same tank, clean: nothing dies, and no stress builds up.
    let mut w = world(3);
    assert!(w.water_quality() >= POOR_QUALITY);
    let a = tetra(&mut w);
    assert_eq!(ticks_to_death(&mut w, a, seconds(20 * 60)), None);
    assert_eq!(w.creature(a).unwrap().stress(), 0);
}

#[test]
fn easy_mode_allows_only_old_age() {
    let mut w = world(4);
    w.set_easy_mode(true);
    foul(&mut w);
    let s = species(&w, "neon_tetra");
    let starving = w.add_creature(s, 30, 50).unwrap();
    w.set_creature_energy(starving, 10);
    let fouled = w.add_creature(s, 64, 50).unwrap();
    let old = w.add_creature(s, 100, 50).unwrap();
    let own = w.creature_lifespan(w.creature(old).unwrap());
    w.set_creature_age(old, own - 5);
    for _ in 0..seconds(20 * 60) {
        w.tick();
    }
    assert!(w.creature(starving).is_some(), "no starvation in easy mode");
    assert_eq!(w.creature(starving).unwrap().energy(), 0);
    assert!(
        w.creature(fouled).is_some(),
        "no foul-water deaths in easy mode"
    );
    assert!(w.creature(old).is_none(), "old age still kills");
    assert!(w.corpses().iter().all(|c| c.cause() == DeathCause::OldAge));

    // Switched off, the same tank kills the starving one at once.
    w.set_easy_mode(false);
    w.tick();
    assert!(w.creature(starving).is_none());
}

#[test]
fn a_corpse_sinks_and_rots_into_detritus() {
    let mut w = world(5);
    let s = species(&w, "neon_tetra");
    let detritus = id(&w, "detritus");
    let a = w.add_creature(s, 64, 30).unwrap();
    let own = w.creature_lifespan(w.creature(a).unwrap());
    w.set_creature_age(a, own - 1);
    let (w0, h0) = w.creature_size(w.creature(a).unwrap());
    w.tick();
    assert!(w.creature(a).is_none());
    let before = w.material_counts()[usize::from(detritus.0)];
    let start = w.corpses()[0].centre().1;
    assert_eq!(w.corpses()[0].size(), (w0, h0));
    // It sinks a cell a second onto the bed and rests there.
    for _ in 0..seconds(59) {
        w.tick();
    }
    let rest = w.corpse_body(&w.corpses()[0]);
    assert_eq!(rest.y1, 88, "resting on the bed");
    assert!(w.corpses()[0].centre().1 > start + 30.0);
    assert_eq!(w.corpses()[0].decay(), 0.0);
    // Then it rots: a detritus cell every couple of seconds, fading.
    let total = (w0 * h0).div_ceil(3);
    let rot = seconds(ROT_AFTER_SECONDS + ROT_EVERY_SECONDS * total as u32);
    for _ in seconds(59)..rot {
        w.tick();
        if let Some(c) = w.corpses().first() {
            assert!(c.decay() < 1.0);
        }
    }
    assert!(w.corpses().is_empty(), "gone once rotted");
    let after = w.material_counts()[usize::from(detritus.0)];
    assert_eq!(after - before, total);
    // The detritus disintegrates like any other.
    for _ in 0..seconds(240) {
        w.tick();
    }
    assert_eq!(w.material_counts()[usize::from(detritus.0)], before);
}

#[test]
fn fry_grow_with_feeding() {
    let mut w = world(6);
    let s = species(&w, "guppy");
    let maturity = w.species().get(s).unwrap().maturity_days * LIFE_DAY_TICKS;
    let fed = w.add_creature_aged(s, 30, 50, 0).unwrap();
    let half = w.add_creature_aged(s, 64, 50, 0).unwrap();
    let hungry = w.add_creature_aged(s, 100, 50, 0).unwrap();
    w.set_creature_energy(half, ENERGY_RESERVE / 4);
    w.set_creature_energy(hungry, ENERGY_RESERVE / 10);
    let adult = w.add_creature(s, 64, 30).unwrap();
    let full = w.creature_size(w.creature(adult).unwrap());
    let fry = w.creature_size(w.creature(fed).unwrap());
    assert_eq!(w.life_stage(w.creature(fed).unwrap()), LifeStage::Fry);
    assert!(fry.0 < full.0 && fry.1 <= full.1, "{fry:?} < {full:?}");
    // It would hunt the fry down.
    w.remove_creature(adult);
    for _ in 0..1000 {
        w.tick();
    }
    let growth = |w: &World, c| w.creature(c).unwrap().growth();
    assert_eq!(growth(&w, fed), 1000);
    assert_eq!(growth(&w, half), 500);
    assert_eq!(growth(&w, hungry), 0);

    // A well-fed fry is a juvenile a quarter of the way to maturity and a
    // full-sized adult at it.
    let mut w = world(6);
    let c = w.add_creature_aged(s, 64, 50, maturity / 4 - 1).unwrap();
    assert_eq!(w.life_stage(w.creature(c).unwrap()), LifeStage::Fry);
    w.tick();
    assert_eq!(w.life_stage(w.creature(c).unwrap()), LifeStage::Juvenile);
    let mid = w.creature_size(w.creature(c).unwrap());
    assert!(mid.0 > fry.0 && mid.0 < full.0, "{mid:?}");
    let c = w.add_creature_aged(s, 64, 30, maturity - 1).unwrap();
    assert_eq!(w.life_stage(w.creature(c).unwrap()), LifeStage::Juvenile);
    w.tick();
    let grown = w.creature(c).unwrap();
    assert_eq!(w.life_stage(grown), LifeStage::Adult);
    assert_eq!(w.creature_size(grown), full);
    // The body box rounds out by a cell when it sits between cells.
    let b = w.creature_body(grown);
    assert!((full.0..=full.0 + 1).contains(&(b.x1 - b.x0)), "{b:?}");
}

#[test]
fn the_population_never_exceeds_the_cap() {
    let mut w = world(7);
    assert_eq!(w.creature_cap(), DEFAULT_CREATURE_CAP);
    let s = species(&w, "cherry_shrimp");
    assert_eq!(w.set_creature_cap(5), 5);
    let mut ids = Vec::new();
    for i in 0..10 {
        match w.add_creature(s, 10 + 10 * i, 80) {
            Ok(id) => ids.push(id),
            Err(e) => assert_eq!(e, WorldError::PopulationCap(5)),
        }
        assert!(w.creatures().len() <= w.creature_cap());
    }
    assert_eq!(ids.len(), 5);
    // The cap can't go below the population, nor past the hard limit.
    assert_eq!(w.set_creature_cap(2), 5);
    assert_eq!(w.set_creature_cap(0), 5);
    assert_eq!(w.set_creature_cap(10_000), MAX_CREATURE_CAP);
    assert_eq!(w.set_creature_cap(6), 6);
    w.add_creature(s, 70, 80).unwrap();
    assert!(w.add_creature(s, 80, 80).is_err());
    // Deaths free room; with a full tank deaths and additions never push
    // it over.
    w.set_creature_energy(ids[0], 3);
    for t in 0..200 {
        w.tick();
        let _ = w.add_creature(s, 20 + (t % 90), 80);
        assert!(w.creatures().len() <= w.creature_cap());
    }
    assert_eq!(w.creatures().len(), 6);
    // A save keeps the cap.
    let back = save::decode(&save::encode(&w), MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(back.creature_cap(), 6);
}

/// A tank with every kind of life state: a fry, a hungry creature, a
/// stressed one, corpses at different stages of rot, easy mode off and a
/// lowered cap.
fn lively(seed: u64) -> World {
    let mut w = world(seed);
    foul(&mut w);
    w.set_creature_cap(20);
    let s = species(&w, "neon_tetra");
    let g = species(&w, "guppy");
    w.add_creature_aged(g, 30, 40, 100).unwrap();
    let hungry = w.add_creature(s, 50, 40).unwrap();
    w.set_creature_energy(hungry, 500);
    w.add_creature(s, 70, 40).unwrap();
    let old = w.add_creature(g, 90, 40).unwrap();
    let own = w.creature_lifespan(w.creature(old).unwrap());
    w.set_creature_age(old, own - 10);
    for _ in 0..seconds(70) {
        w.tick();
    }
    w
}

#[test]
fn life_state_survives_a_save() {
    let mut w = lively(8);
    assert_eq!(w.corpses().len(), 2, "the old one and the starved one");
    assert!(w.corpses().iter().any(|c| c.decay() > 0.0));
    assert!(w.creatures().iter().any(|c| c.stress() > 0));
    assert!(
        w.creatures()
            .iter()
            .any(|c| w.life_stage(c) == LifeStage::Fry)
    );
    let bytes = save::encode(&w);
    assert_eq!(save::check(&bytes), Ok(save::FORMAT_VERSION));
    let mut back = save::decode(&bytes, MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(back.state_hash(), w.state_hash());
    assert_eq!(save::encode(&back), bytes);
    assert_eq!(back.creature_cap(), 20);
    assert_eq!(back.corpses(), w.corpses());
    // It carries on exactly, deaths and rot included.
    for _ in 0..seconds(120) {
        w.tick();
        back.tick();
        assert_eq!(back.state_hash(), w.state_hash());
    }
    assert!(w.corpses().len() != 2 || w.creatures().len() < 3);
    let mut easy = lively(8);
    easy.set_easy_mode(true);
    let back = save::decode(&save::encode(&easy), MaterialTable::builtin().unwrap()).unwrap();
    assert!(back.easy_mode());
}

#[test]
fn the_life_cycle_is_deterministic() {
    let (mut a, mut b) = (lively(9), lively(9));
    for _ in 0..seconds(60) {
        a.tick();
        b.tick();
    }
    assert_eq!(a.state_hash(), b.state_hash());
    assert_ne!(a.state_hash(), lively(10).state_hash());
}

#[test]
fn the_quality_sample_matches_a_full_scan() {
    let mut w = world(11);
    let q = |w: &mut World| (w.water_quality(), w.ecosystem().quality);
    let (sampled, scanned) = q(&mut w);
    assert!((sampled - scanned).abs() < 1e-4);
    foul(&mut w);
    let (sampled, scanned) = q(&mut w);
    assert!((sampled - scanned).abs() < 1e-4, "{sampled} {scanned}");
    for _ in 0..seconds(100) {
        w.tick();
    }
    // Waste has turned to detritus and some detritus to nutrients.
    let (sampled, scanned) = q(&mut w);
    assert!((sampled - scanned).abs() < 1e-4, "{sampled} {scanned}");
    w.water_change();
    let (sampled, scanned) = q(&mut w);
    assert!((sampled - scanned).abs() < 1e-4, "{sampled} {scanned}");
    assert!(sampled > 0.3);
}

#[test]
fn growing_and_ageing_ask_for_a_redraw_even_when_held() {
    let mut w = world(12);
    let s = species(&w, "guppy");
    let maturity = w.species().get(s).unwrap().maturity_days * LIFE_DAY_TICKS;
    // Held still in the net, a fry one tick from maturity grows to full
    // size: the renderer must hear of it.
    let fry = w.add_creature_aged(s, 64, 50, maturity - 1).unwrap();
    w.hold_creature(fry);
    let before = w.creatures_version();
    w.tick();
    assert_eq!(w.life_stage(w.creature(fry).unwrap()), LifeStage::Adult);
    assert_ne!(w.creatures_version(), before);
    // And an adult turning elder while held.
    let span = w.creature_lifespan(w.creature(fry).unwrap());
    w.set_creature_age(fry, span * 4 / 5 - 1);
    w.hold_creature(fry);
    let before = w.creatures_version();
    w.tick();
    assert_eq!(w.life_stage(w.creature(fry).unwrap()), LifeStage::Elder);
    assert_ne!(w.creatures_version(), before);
}
