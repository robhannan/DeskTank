//! Coarse time advances preserve the world clock and core ecology rules.

use desktank_sim::{
    BITE_ENERGY, Brush, CatchUp, ENERGY_RESERVE, FieldKind, MAX_CATCH_UP_SECONDS, MaterialTable,
    Propagule, Rect, SETTLE_TICKS, TICKS_PER_SECOND, World, WorldConfig, save, scene,
};

fn tank() -> World {
    World::new(
        WorldConfig {
            width: 80,
            height: 48,
            seed: 7,
            ..WorldConfig::default()
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap()
}

#[test]
fn zero_seconds_preserves_pending_edits() {
    let mut w = tank();
    let detritus = scene::material(&w, "detritus").unwrap();
    w.set_cell(3, 25, detritus).unwrap();
    let before = save::encode(&w);
    assert_eq!(w.catch_up(0), CatchUp::default());
    assert_eq!(save::encode(&w), before);
}

#[test]
fn clamps_a_day_and_is_deterministic() {
    let mut w = scene::catch_up_reference(7, MaterialTable::builtin().unwrap()).unwrap();
    let mut exact = w.clone();
    let start = w.tick_count();
    let report = w.catch_up(u32::MAX);
    assert_eq!(report, exact.catch_up(MAX_CATCH_UP_SECONDS));
    assert_eq!(report.seconds, MAX_CATCH_UP_SECONDS);
    assert_eq!(
        w.tick_count() - start,
        24 * 3600 * u64::from(TICKS_PER_SECOND)
    );
    assert_eq!(w.state_hash(), exact.state_hash());
}

#[test]
fn ages_and_grows_a_fry_for_the_elapsed_time() {
    let mut w = tank();
    let guppy = w.species().id("guppy").unwrap();
    let fry = w.add_creature_aged(guppy, 25, 25, 0).unwrap();
    let seconds = 137;
    for energy in [
        ENERGY_RESERVE,
        ENERGY_RESERVE / 2 + 857,
        ENERGY_RESERVE / 8 + 859,
    ] {
        let mut coarse = w.clone();
        coarse.set_creature_energy(fry, energy);
        let mut exact = coarse.clone();
        for _ in 0..seconds * TICKS_PER_SECOND {
            exact.tick();
        }
        coarse.catch_up(seconds);
        let c = coarse.creature(fry).unwrap();
        let expected = exact.creature(fry).unwrap();
        assert_eq!(c.age(), seconds * TICKS_PER_SECOND);
        assert_eq!(c.growth(), expected.growth());
        assert_eq!(c.energy(), expected.energy());
    }
}

#[test]
fn unfed_creatures_starve_and_rot() {
    let mut w = tank();
    let guppy = w.species().id("guppy").unwrap();
    let id = w.add_creature(guppy, 25, 25).unwrap();
    w.set_creature_energy(id, 3 * TICKS_PER_SECOND);
    let report = w.catch_up(300);
    assert_eq!(report.starvation_deaths, 1);
    assert_eq!(w.creatures(), []);
    assert_eq!(w.corpses(), []);
    let detritus = scene::material(&w, "detritus").unwrap();
    assert!(w.material_counts()[usize::from(detritus.0)] > 0);
}

#[test]
fn easy_mode_only_allows_old_age_deaths() {
    let mut w = tank();
    let guppy = w.species().id("guppy").unwrap();
    let hungry = w.add_creature(guppy, 25, 25).unwrap();
    let old = w.add_creature(guppy, 50, 25).unwrap();
    let stressed = w.add_creature(guppy, 40, 35).unwrap();
    let lifespan = w.creature_lifespan(w.creature(old).unwrap());
    w.set_creature_age(old, lifespan - 1);
    w.set_creature_energy(hungry, 0);
    for y in (0..48).step_by(8) {
        for x in (0..80).step_by(8) {
            w.add_to_field(FieldKind::Nutrient, x, y, 3.0);
        }
    }
    let mut regular = w.clone();
    let report = regular.catch_up(3600);
    assert_eq!(report.poor_water_deaths, 1);
    assert!(regular.creature(stressed).is_none());
    w.set_easy_mode(true);
    let report = w.catch_up(3600);
    assert_eq!(report.old_age_deaths, 1);
    assert_eq!(report.starvation_deaths, 0);
    assert_eq!(report.poor_water_deaths, 0);
    assert!(w.creature(hungry).is_some());
    assert!(w.creature(stressed).is_some());
}

#[test]
fn a_hungry_creature_eats_available_food() {
    let mut w = tank();
    let guppy = w.species().id("guppy").unwrap();
    let id = w.add_creature(guppy, 25, 25).unwrap();
    w.set_creature_energy(id, 3 * TICKS_PER_SECOND);
    let food = scene::material(&w, "flake_food").unwrap();
    w.set_cell(45, 30, food).unwrap();
    let report = w.catch_up(60);
    let c = w.creature(id).unwrap();
    assert_eq!(report.food_eaten, 1);
    assert_eq!(c.eaten(), 1);
    assert_eq!(c.energy(), BITE_ENERGY);
    assert_eq!(w.material_counts()[usize::from(food.0)], 0);
}

#[test]
fn a_settled_pair_breeds() {
    let mut w = tank();
    let guppy = w.species().id("guppy").unwrap();
    w.add_creature(guppy, 25, 25).unwrap();
    w.add_creature(guppy, 50, 25).unwrap();
    w.skip_breeding_time(SETTLE_TICKS);
    let report = w.catch_up(6 * 3600);
    assert!((3..=8).contains(&report.births));
    assert_eq!(w.creatures().len(), 2 + report.births);
}

#[test]
fn eggs_laid_during_catch_up_hatch_when_due() {
    let mut w = tank();
    let sand = scene::material(&w, "sand").unwrap();
    w.fill_rect(
        Rect {
            x0: 0,
            y0: 42,
            x1: 80,
            y1: 48,
        },
        sand,
        Brush::Replace,
    )
    .unwrap();
    let cory = w.species().id("corydoras").unwrap();
    w.add_creature(cory, 25, 35).unwrap();
    w.add_creature(cory, 50, 35).unwrap();
    w.skip_breeding_time(SETTLE_TICKS);
    assert_eq!(w.catch_up(30 * 60).births, 0);
    assert_eq!(w.clutches().len(), 1);
    let report = w.catch_up(40 * 60);
    assert_eq!(w.clutches(), []);
    assert!((4..=10).contains(&report.births));
    assert_eq!(w.creatures().len(), 2 + report.births);
}

#[test]
fn detritus_decays_and_a_plant_grows() {
    let mut w = tank();
    let detritus = scene::material(&w, "detritus").unwrap();
    let sand = scene::material(&w, "sand").unwrap();
    w.fill_rect(
        Rect {
            x0: 0,
            y0: 42,
            x1: 80,
            y1: 48,
        },
        sand,
        Brush::Replace,
    )
    .unwrap();
    let marimo = w.plant_species().id("marimo").unwrap();
    w.paint_plant(24, 41, 0, marimo, Propagule::Fragments)
        .unwrap();
    w.set_cell(40, 25, detritus).unwrap();
    assert!(w.field(FieldKind::Nutrient).total() < f64::EPSILON);
    w.catch_up(3600);
    assert_eq!(w.material_counts()[usize::from(detritus.0)], 0);
    assert!((0.3..=0.500_01).contains(&w.field(FieldKind::Nutrient).total()));
    assert!(w.plant_cells() > 1);
    assert!(w.dirty_rects().any(|r| r.contains(40, 25)));
    let mut loaded = save::decode(&save::encode(&w), MaterialTable::builtin().unwrap()).unwrap();
    w.tick();
    loaded.tick();
    assert!(w.dirty_rects().any(|r| r.contains(40, 25)));
    assert_eq!(w.state_hash(), loaded.state_hash());
}
