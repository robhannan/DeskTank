//! DV3 acceptance tests (`docs/spec/devices.md` §2.8 *Filtration*, D27, #159):
//! - Intakes capture suspended particles into media load and touch only intake cells.
//! - Media load clogs the filter and flow falls with it, never below ~30% at full clog.
//! - Long screensaver-style unrinsed runs never drop flow below the floor.
//! - Rinse clears media load; rinsing hard or replacing media sets maturity back.
//! - Media matures over days; a mature filter breaks waste down faster and raises water quality.
//! - Intakes suck in fry and small shrimp unless the intake guard is on (or easy mode is on).

use desktank_sim::{
    CLOG_FLOW_FLOOR, DeviceKind, LIFE_DAY_TICKS, MaterialTable, RinseKind, TICKS_PER_SECOND, World,
    WorldConfig,
};

fn world(width: usize, height: usize, seed: u64) -> World {
    World::new(
        WorldConfig {
            width,
            height,
            seed,
            water_level: 1.0,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap()
}

/// Returns the first open water cell immediately adjacent to device `dev`'s intake.
fn intake_water_cell(w: &World, dev: desktank_sim::DeviceId) -> (usize, usize) {
    let d = w.device(dev).unwrap();
    let art = w.device_kinds().get(d.kind);
    let (ix, iy) = d.intakes(art).next().unwrap();
    let water = w.materials().id("water").unwrap();
    for (dx, dy) in [
        (1isize, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (1, -1),
        (-1, 1),
        (-1, -1),
    ] {
        let x = ix.checked_add_signed(dx).unwrap();
        let y = iy.checked_add_signed(dy).unwrap();
        if w.cell(x, y).is_some_and(|c| c.material == water) {
            return (x, y);
        }
    }
    panic!("no open water cell next to intake ({ix}, {iy})");
}

#[test]
fn intakes_capture_suspended_particles_and_touch_only_intake_cells() {
    let mut small = world(64, 64, 1);
    let mut large = world(256, 128, 1);
    let dev_s = small
        .add_device(DeviceKind::InternalFilter, 4, 30, false)
        .unwrap()
        .unwrap();
    let dev_l = large
        .add_device(DeviceKind::InternalFilter, 4, 30, false)
        .unwrap()
        .unwrap();

    let waste = small.materials().id("waste").unwrap();
    let detritus = small.materials().id("detritus").unwrap();
    let water = small.materials().id("water").unwrap();
    let (wx, wy) = intake_water_cell(&small, dev_s);

    // Place a particle directly next to the intake and another resting on the far floor.
    small.set_cell(wx, wy, waste).unwrap();
    small.set_cell(55, 63, detritus).unwrap();

    let small_before = small.intake_cell_checks();
    let large_before = large.intake_cell_checks();
    small.tick();
    large.tick();

    // The adjacent waste cell is captured into water and bumps media_load.
    assert_eq!(small.cell(wx, wy).unwrap().material, water);
    let status = small.device_status(dev_s).unwrap();
    assert!(
        status.media_load > 0.0,
        "capturing waste must increase media_load"
    );
    // The distant detritus cell is untouched by the intake.
    assert_eq!(small.cell(55, 63).unwrap().material, detritus);

    // Intake work touches ONLY the 8 neighbours of each intake cell,
    // regardless of tank dimensions (64x64 vs 256x128).
    let intakes_count = small
        .device_kinds()
        .get(DeviceKind::InternalFilter)
        .intakes
        .len() as u64;
    assert_eq!(small.intake_cell_checks() - small_before, 8 * intakes_count);
    assert_eq!(large.intake_cell_checks() - large_before, 8 * intakes_count);
    let _ = dev_l;
}

#[test]
fn media_load_clogs_flow_down_to_floor_and_never_below() {
    let mut w = world(128, 64, 2);
    let dev = w
        .add_device(DeviceKind::InternalFilter, 4, 30, false)
        .unwrap()
        .unwrap();
    w.tick();
    let clean_flow = w.device_status(dev).unwrap().flow;
    assert!(clean_flow > 1.0, "clean filter should drive strong flow");

    let waste = w.materials().id("waste").unwrap();
    let (wx, wy) = intake_water_cell(&w, dev);

    // Feed enough particles to half-clog and then fully clog the filter.
    for _ in 0..100 {
        w.set_cell(wx, wy, waste).unwrap();
        w.tick();
    }
    let mid = w.device_status(dev).unwrap();
    assert!(mid.media_load >= 0.45 && mid.media_load <= 0.55);
    assert!(
        mid.flow < clean_flow && mid.flow > clean_flow * CLOG_FLOW_FLOOR,
        "half-clogged flow {} should sit between floor and clean {}",
        mid.flow,
        clean_flow
    );

    for _ in 0..150 {
        w.set_cell(wx, wy, waste).unwrap();
        w.tick();
    }
    let full = w.device_status(dev).unwrap();
    assert!((full.media_load - 1.0).abs() < 1e-6);
    let ratio = full.flow / clean_flow;
    assert!(
        (CLOG_FLOW_FLOOR - 0.05..=CLOG_FLOW_FLOOR + 0.05).contains(&ratio),
        "full-clog flow ratio {ratio:.3} should be near {CLOG_FLOW_FLOOR}"
    );
    assert!(
        ratio >= CLOG_FLOW_FLOOR - 0.02,
        "flow ratio {ratio:.3} must never fall below the ~30% floor"
    );
}

#[test]
fn unrinsed_screensaver_run_never_drops_flow_below_floor() {
    let mut w = world(128, 64, 3);
    let dev = w
        .add_device(DeviceKind::InternalFilter, 4, 30, false)
        .unwrap()
        .unwrap();
    w.tick();
    let clean_flow = w.device_status(dev).unwrap().flow;
    let floor_flow = clean_flow * (CLOG_FLOW_FLOOR - 0.02);

    let waste = w.materials().id("waste").unwrap();
    let (wx, wy) = intake_water_cell(&w, dev);

    // Long screensaver-style run: 4,000 ticks (~3.3 world minutes) with
    // continuous heavy particle load and zero rinses.
    for t in 0..4000 {
        if t % 5 == 0 {
            w.set_cell(wx, wy, waste).unwrap();
        }
        w.tick();
        if t % 200 == 0 {
            let s = w.device_status(dev).unwrap();
            assert!(
                s.media_load <= 1.0,
                "media_load must stay clamped at 1.0 (tick {t})"
            );
            assert!(
                s.flow >= floor_flow,
                "unrinsed flow {} fell below floor {} at tick {t}",
                s.flow,
                floor_flow
            );
        }
    }
    let end = w.device_status(dev).unwrap();
    assert!((end.media_load - 1.0).abs() < 1e-6);
    assert!(end.flow >= floor_flow);
}

#[test]
fn rinse_clears_load_and_hard_rinse_or_replace_sets_maturity_back() {
    let mut w = world(128, 64, 4);
    let dev = w
        .add_device(DeviceKind::InternalFilter, 4, 30, false)
        .unwrap()
        .unwrap();
    w.tick();
    let clean_flow = w.device_status(dev).unwrap().flow;

    // Clog and mature the filter.
    assert!(w.set_device_media_load(dev, 1.0));
    assert!(w.set_device_maturity(dev, 0.8));
    let clogged = w.device_status(dev).unwrap();
    assert!(clogged.flow < clean_flow * 0.5);

    // Gentle rinse clears load, restores flow, and preserves maturity.
    assert!(w.rinse_device(dev, RinseKind::Gentle));
    let gentle = w.device_status(dev).unwrap();
    assert_eq!(gentle.media_load, 0.0);
    assert!((gentle.maturity - 0.8).abs() < 1e-6);
    assert!(
        (gentle.flow - clean_flow).abs() < 1e-3,
        "gentle rinse must restore full flow"
    );

    // Hard rinse clears load and sets maturity back sharply.
    assert!(w.set_device_media_load(dev, 0.6));
    assert!(w.rinse_device(dev, RinseKind::Hard));
    let hard = w.device_status(dev).unwrap();
    assert_eq!(hard.media_load, 0.0);
    assert!(
        hard.maturity < 0.3 && hard.maturity > 0.0,
        "hard rinse should set maturity back sharply, got {}",
        hard.maturity
    );
    assert!((hard.flow - clean_flow).abs() < 1e-3);

    // Replacing media clears load and resets maturity to zero.
    assert!(w.set_device_media_load(dev, 0.5));
    assert!(w.rinse_device(dev, RinseKind::Replace));
    let replaced = w.device_status(dev).unwrap();
    assert_eq!(replaced.media_load, 0.0);
    assert_eq!(replaced.maturity, 0.0);
    assert!((replaced.flow - clean_flow).abs() < 1e-3);
}

#[test]
fn media_matures_over_days_speeds_waste_breakdown_and_raises_water_quality() {
    // 1. Maturity progresses over simulated days and caps at 1.0.
    let mut aging = world(64, 64, 5);
    let dev = aging
        .add_device(DeviceKind::InternalFilter, 4, 30, false)
        .unwrap()
        .unwrap();
    assert_eq!(aging.device_status(dev).unwrap().maturity, 0.0);
    // Run half a life-day (6,000 ticks = 5 world minutes).
    for _ in 0..(LIFE_DAY_TICKS / 2) {
        aging.tick();
    }
    let m_half_day = aging.device_status(dev).unwrap().maturity;
    assert!(
        m_half_day > 0.05 && m_half_day < 0.25,
        "maturity after 0.5 life-days should be partial, got {m_half_day}"
    );
    // Run another half life-day: maturity increases monotonically.
    for _ in 0..(LIFE_DAY_TICKS / 2) {
        aging.tick();
    }
    let m_one_day = aging.device_status(dev).unwrap().maturity;
    assert!(
        m_one_day > m_half_day,
        "maturity must keep rising ({m_one_day} <= {m_half_day})"
    );

    // 2. Compare two tanks with identical pollutants far from the intake:
    //    one with a fresh filter (maturity 0), one with a mature filter (maturity 1).
    let setup = |maturity: f32| {
        let mut w = world(128, 64, 6);
        let d = w
            .add_device(DeviceKind::InternalFilter, 4, 30, false)
            .unwrap()
            .unwrap();
        assert!(w.set_device_maturity(d, maturity));
        let waste = w.materials().id("waste").unwrap();
        let rock = w.materials().id("rock").unwrap();
        // Pocket of rock on the far right holding waste so current can't carry it to the intake.
        for x in 100..124 {
            w.set_cell(x, 60, rock).unwrap();
        }
        for y in 52..60 {
            w.set_cell(100, y, rock).unwrap();
            w.set_cell(123, y, rock).unwrap();
        }
        for y in 54..60 {
            for x in 102..122 {
                w.set_cell(x, y, waste).unwrap();
            }
        }
        w
    };

    let mut fresh = setup(0.0);
    let mut mature = setup(1.0);

    // Water-quality score is immediately higher with a mature filter (when
    // pollutants pull baseline below 1.0).
    let q_fresh = fresh.ecosystem().quality;
    let q_mature = mature.ecosystem().quality;
    assert!(
        q_mature > q_fresh,
        "mature filter quality ({q_mature}) must exceed fresh filter ({q_fresh})"
    );
    assert!(
        (mature.water_quality() - q_mature).abs() < 1e-4,
        "water_quality() and ecosystem().quality must agree"
    );

    // After 30 world seconds (600 ticks), waste (60 s base decay) breaks down
    // much faster with the mature filter (3x speed => ~20 s).
    let waste = fresh.materials().id("waste").unwrap();
    for _ in 0..(30 * TICKS_PER_SECOND) {
        fresh.tick();
        mature.tick();
    }
    let count_waste = |w: &World| w.cells().iter().filter(|c| c.material == waste).count();
    let rem_fresh = count_waste(&fresh);
    let rem_mature = count_waste(&mature);
    assert!(
        rem_mature < rem_fresh,
        "mature filter should break waste down faster (mature left {rem_mature}, fresh left {rem_fresh})"
    );
    assert_eq!(
        rem_mature, 0,
        "with 3x breakdown speed, 60s waste should be gone by 30s"
    );
}

#[test]
fn unguarded_intake_sucks_in_fry_and_small_shrimp_but_guard_protects_them() {
    let run_case = |species_name: &str, age: Option<u32>, guard: bool, easy: bool| -> usize {
        let mut w = world(128, 64, 7);
        w.set_easy_mode(easy);
        let dev = w
            .add_device(DeviceKind::InternalFilter, 4, 30, false)
            .unwrap()
            .unwrap();
        assert!(w.set_device_intake_guard(dev, guard));
        let (wx, wy) = intake_water_cell(&w, dev);
        let sp = w.species().id(species_name).unwrap();
        let x = i64::try_from(wx).unwrap();
        let y = i64::try_from(wy).unwrap();
        match age {
            Some(a) => {
                w.add_creature_aged(sp, x, y, a).unwrap();
            }
            None => {
                w.add_creature(sp, x, y).unwrap();
            }
        }
        for _ in 0..40 {
            w.tick();
        }
        w.creatures().len()
    };

    // Fry (age 0 guppy): sucked in with guard off, safe with guard on, safe in easy mode.
    assert_eq!(
        run_case("guppy", Some(0), false, false),
        0,
        "unguarded intake must suck in fry"
    );
    assert_eq!(
        run_case("guppy", Some(0), true, false),
        1,
        "intake guard must protect fry"
    );
    assert_eq!(
        run_case("guppy", Some(0), false, true),
        1,
        "easy mode must protect fry even without guard"
    );

    // Small shrimp (cherry shrimp): sucked in with guard off, safe with guard on.
    assert_eq!(
        run_case("cherry_shrimp", None, false, false),
        0,
        "unguarded intake must suck in cherry shrimp"
    );
    assert_eq!(
        run_case("cherry_shrimp", None, true, false),
        1,
        "intake guard must protect cherry shrimp"
    );

    // Adult fish (guppy) and adult amano shrimp (4.0 cm) are too large to be sucked in.
    assert_eq!(
        run_case("guppy", None, false, false),
        1,
        "adult guppy must not be sucked in"
    );
    assert_eq!(
        run_case("amano_shrimp", None, false, false),
        1,
        "adult amano shrimp must not be sucked in"
    );
}
