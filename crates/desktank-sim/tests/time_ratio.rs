//! The life-time multiplier (#74): `World::time_ratio` scales how fast
//! creatures age, grow, burn energy, breed and tire of foul water, how fast
//! plants grow and filter media mature, in live ticks and in catch-up, and
//! leaves a tank at ratio 1 exactly as it was.

#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use desktank_sim::{
    Brush, DEFAULT_TIME_RATIO, DeviceKind, ENERGY_RESERVE, FieldKind, MAX_TIME_RATIO,
    MIN_TIME_RATIO, MaterialTable, Propagule, Rect, SpeciesId, TICKS_PER_SECOND, Traits, World,
    WorldConfig, save,
};

fn tank(seed: u64) -> World {
    World::new(
        WorldConfig {
            width: 96,
            height: 64,
            seed,
            water_level: 0.95,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap()
}

fn species(w: &World, name: &str) -> SpeciesId {
    w.species().id(name).unwrap()
}

fn run(w: &mut World, ticks: u32) {
    for _ in 0..ticks {
        w.tick();
    }
}

/// A tank with a fry, an adult pair and a plant, as a busy one.
fn busy(seed: u64) -> World {
    let mut w = tank(seed);
    let guppy = species(&w, "guppy");
    w.add_creature_aged(guppy, 20, 30, 0).unwrap();
    w.add_creature(guppy, 40, 30).unwrap();
    w.add_creature(guppy, 60, 30).unwrap();
    let marimo = w.plant_species().id("marimo").unwrap();
    w.paint_plant(30, 62, 0, marimo, Propagule::Fragments)
        .unwrap();
    w
}

#[test]
fn ratio_one_is_the_default_and_leaves_the_tank_as_it_was() {
    let mut plain = busy(5);
    let mut set = busy(5);
    assert!((plain.time_ratio() - DEFAULT_TIME_RATIO).abs() < f32::EPSILON);
    assert!((set.set_time_ratio(1.0) - 1.0).abs() < f32::EPSILON);
    assert_eq!(plain.state_hash(), set.state_hash());
    for _ in 0..600 {
        plain.tick();
        set.tick();
        assert_eq!(set.last_life_ticks(), 1);
    }
    assert_eq!(plain.state_hash(), set.state_hash());
    // Catch-up at ratio 1 agrees too.
    plain.catch_up(3600);
    set.catch_up(3600);
    assert_eq!(plain.state_hash(), set.state_hash());
    // A tank off real time hashes differently from one on it.
    let mut fast = busy(5);
    fast.set_time_ratio(2.0);
    assert_ne!(fast.state_hash(), busy(5).state_hash());
}

#[test]
fn the_ratio_is_clamped_and_bad_values_become_real_time() {
    let mut w = tank(1);
    assert!((w.set_time_ratio(0.0) - MIN_TIME_RATIO).abs() < f32::EPSILON);
    assert!((w.set_time_ratio(-5.0) - MIN_TIME_RATIO).abs() < f32::EPSILON);
    assert!((w.set_time_ratio(1e9) - MAX_TIME_RATIO).abs() < f32::EPSILON);
    assert!((w.set_time_ratio(f32::INFINITY) - 1.0).abs() < f32::EPSILON);
    assert!((w.set_time_ratio(f32::NAN) - 1.0).abs() < f32::EPSILON);
    assert!((w.set_time_ratio(2.5) - 2.5).abs() < f32::EPSILON);
    assert!((w.time_ratio() - 2.5).abs() < f32::EPSILON);
}

#[test]
fn life_ticks_follow_the_ratio_with_the_fraction_carried() {
    for (ratio, per_ten) in [(2.0, 20), (0.5, 5), (1.5, 15), (0.1, 1), (100.0, 1000)] {
        let mut w = tank(2);
        w.set_time_ratio(ratio);
        let mut total = 0;
        for _ in 0..10 {
            w.tick();
            total += w.last_life_ticks();
        }
        assert_eq!(total, per_ten, "ratio {ratio}");
    }
    // At 0.5 life ticks come every other tick.
    let mut w = tank(2);
    w.set_time_ratio(0.5);
    let pattern: Vec<u32> = (0..6)
        .map(|_| {
            w.tick();
            w.last_life_ticks()
        })
        .collect();
    assert_eq!(pattern, [0, 1, 0, 1, 0, 1]);
}

/// A fed fry (full energy) in a tank at `ratio`.
fn fry(ratio: f32) -> (World, desktank_sim::CreatureId) {
    let mut w = tank(3);
    w.set_time_ratio(ratio);
    let guppy = species(&w, "guppy");
    let id = w.add_creature_aged(guppy, 40, 30, 0).unwrap();
    w.set_creature_energy(id, ENERGY_RESERVE);
    (w, id)
}

#[test]
fn live_ticks_scale_ageing_energy_burn_and_growth() {
    for (ratio, expect) in [(1.0, 200), (2.0, 400), (0.5, 100), (4.0, 800)] {
        let (mut w, id) = fry(ratio);
        run(&mut w, 200);
        let c = w.creature(id).unwrap();
        assert_eq!(c.age(), expect, "age at {ratio}");
        assert_eq!(c.energy(), ENERGY_RESERVE - expect, "energy at {ratio}");
        // Well fed, a fry grows a tick per tick of life.
        assert_eq!(c.growth(), expect, "growth at {ratio}");
    }
}

#[test]
fn catch_up_scales_ageing_energy_burn_and_growth() {
    let seconds = 137;
    for ratio in [1.0_f32, 2.0, 0.5, 10.0] {
        let (mut w, id) = fry(ratio);
        let expect = (f64::from(seconds * TICKS_PER_SECOND) * f64::from(ratio)) as u32;
        w.catch_up(seconds);
        let c = w.creature(id).unwrap();
        assert_eq!(c.age(), expect, "age at {ratio}");
        assert_eq!(c.energy(), ENERGY_RESERVE - expect, "energy at {ratio}");
        assert_eq!(c.growth(), expect, "growth at {ratio}");
        // The same span run tick by tick lands in the same place.
        let (mut exact, eid) = fry(ratio);
        run(&mut exact, seconds * TICKS_PER_SECOND);
        let e = exact.creature(eid).unwrap();
        assert_eq!(
            (c.age(), c.energy(), c.growth()),
            (e.age(), e.energy(), e.growth())
        );
    }
}

#[test]
fn live_and_catch_up_pregnancy_counts_down_at_the_ratio() {
    let guppy_female = |w: &mut World| {
        let g = species(w, "guppy");
        let id = w
            .add_creature_with_traits(
                g,
                40,
                30,
                Traits {
                    female: Some(true),
                    ..Traits::default()
                },
            )
            .unwrap();
        w.set_creature_energy(id, ENERGY_RESERVE);
        assert!(w.start_carrying(id, 10_000));
        id
    };
    for (ratio, left) in [(1.0, 9_000), (2.0, 8_000), (0.5, 9_500)] {
        let mut w = tank(4);
        w.set_time_ratio(ratio);
        let id = guppy_female(&mut w);
        run(&mut w, 1000);
        assert_eq!(w.creature(id).unwrap().carrying(), left, "live at {ratio}");
        let mut c = tank(4);
        c.set_time_ratio(ratio);
        let id = guppy_female(&mut c);
        c.catch_up(60);
        let want = 10_000 - (1200.0 * ratio) as u32;
        assert_eq!(
            c.creature(id).unwrap().carrying(),
            want,
            "catch up at {ratio}"
        );
    }
}

fn plant_cells_after(ratio: f32, catch_up: bool) -> usize {
    let mut w = tank(6);
    w.set_time_ratio(ratio);
    let sand = w.materials().id("sand").unwrap();
    w.fill_rect(
        Rect {
            x0: 0,
            y0: 60,
            x1: 96,
            y1: 64,
        },
        sand,
        Brush::Replace,
    )
    .unwrap();
    let marimo = w.plant_species().id("marimo").unwrap();
    w.paint_plant(40, 59, 0, marimo, Propagule::Fragments)
        .unwrap();
    if catch_up {
        w.catch_up(600);
    } else {
        run(&mut w, 1800 * TICKS_PER_SECOND);
    }
    w.plant_cells()
}

#[test]
fn plants_grow_faster_at_a_higher_ratio() {
    // Live: growth energy accrues with the ratio.
    let (slow, real, fast) = (
        plant_cells_after(0.1, false),
        plant_cells_after(1.0, false),
        plant_cells_after(10.0, false),
    );
    assert!(slow < real && real < fast, "{slow} < {real} < {fast}");
    // Catch-up grows in coarse steps, so a faster ratio may only reach the
    // same cap: it is never slower.
    let (slow, real, fast) = (
        plant_cells_after(0.1, true),
        plant_cells_after(1.0, true),
        plant_cells_after(10.0, true),
    );
    assert!(slow < real && real <= fast, "{slow} < {real} <= {fast}");
}

#[test]
fn filter_media_matures_at_the_ratio() {
    let maturity = |ratio: f32| {
        let mut w = tank(8);
        w.set_time_ratio(ratio);
        let dev = w
            .add_device(DeviceKind::InternalFilter, 4, 30, false)
            .unwrap()
            .unwrap();
        run(&mut w, 600 * TICKS_PER_SECOND);
        w.device_status(dev).unwrap().maturity
    };
    let (real, double, half) = (maturity(1.0), maturity(2.0), maturity(0.5));
    assert!(real > 0.0);
    assert!(
        (double - 2.0 * real).abs() < real * 0.05,
        "{double} vs {real}"
    );
    assert!((half - 0.5 * real).abs() < real * 0.05, "{half} vs {real}");
}

#[test]
fn foul_water_stresses_faster_at_a_higher_ratio() {
    let stress = |ratio: f32| {
        let mut w = tank(9);
        w.set_time_ratio(ratio);
        let (waste, detritus) = (
            w.materials().id("waste").unwrap(),
            w.materials().id("detritus").unwrap(),
        );
        let rows = |y0, y1| Rect {
            x0: 0,
            y0,
            x1: 96,
            y1,
        };
        w.fill_rect(rows(56, 58), waste, Brush::Replace).unwrap();
        w.fill_rect(rows(58, 60), detritus, Brush::Replace).unwrap();
        for y in (0..64).step_by(8) {
            for x in (0..96).step_by(8) {
                w.add_to_field(FieldKind::Nutrient, x, y, 3.0);
            }
        }
        let g = species(&w, "guppy");
        let id = w.add_creature(g, 40, 30).unwrap();
        run(&mut w, 10 * TICKS_PER_SECOND);
        w.creature(id).unwrap().stress()
    };
    let (slow, real, fast) = (stress(0.5), stress(1.0), stress(2.0));
    assert!(slow > 0, "foul water stresses");
    assert!(slow < real && real < fast, "{slow} < {real} < {fast}");
}

#[test]
fn the_ratio_and_its_fraction_save_and_replay() {
    let mut w = busy(11);
    w.set_time_ratio(1.7);
    run(&mut w, 333);
    let bytes = save::encode(&w);
    assert_eq!(save::check(&bytes), Ok(save::FORMAT_VERSION));
    let mut loaded = save::decode(&bytes, MaterialTable::builtin().unwrap()).unwrap();
    assert!((loaded.time_ratio() - 1.7).abs() < f32::EPSILON);
    assert_eq!(loaded.state_hash(), w.state_hash());
    for _ in 0..400 {
        w.tick();
        loaded.tick();
        assert_eq!(w.last_life_ticks(), loaded.last_life_ticks());
    }
    assert_eq!(loaded.state_hash(), w.state_hash());
    w.catch_up(600);
    loaded.catch_up(600);
    assert_eq!(loaded.state_hash(), w.state_hash());
}
