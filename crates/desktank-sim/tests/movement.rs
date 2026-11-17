//! SPEC T1 acceptance tests: determinism, mass conservation, chunk sleep,
//! bubbles, angle of repose, floating and liquid spread.
//!
//! Grids are small so the suite stays fast in debug CI.

use desktank_sim::{
    Brush, MaterialId, MaterialTable, Rect, SLEEP_TICKS, TICKS_PER_SECOND, World, WorldConfig,
};

fn world(width: usize, height: usize, seed: u64, water_level: f32) -> World {
    World::new(
        WorldConfig {
            width,
            height,
            seed,
            water_level,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap()
}

fn id(w: &World, name: &str) -> MaterialId {
    w.materials().id(name).unwrap()
}

fn positions(w: &World, m: MaterialId) -> Vec<(usize, usize)> {
    w.cells()
        .iter()
        .enumerate()
        .filter(|(_, c)| c.material == m)
        .map(|(i, _)| (i % w.width(), i / w.width()))
        .collect()
}

/// Ticks until every chunk sleeps; panics after `limit` ticks.
fn settle(w: &mut World, limit: u32) {
    for _ in 0..limit {
        w.tick();
        if w.awake_chunks() == 0 {
            return;
        }
    }
    panic!("not settled after {limit} ticks");
}

/// A deterministic input script: pours of every test material.
fn scripted_run(seed: u64, ticks: u64) -> u64 {
    let mut w = world(128, 128, seed, 0.9);
    let names = [
        "sand",
        "gravel",
        "air_bubble",
        "flake_food",
        "rock",
        "water",
    ];
    let ids: Vec<_> = names.iter().map(|n| id(&w, n)).collect();
    for tick in 0..ticks {
        if tick % 97 == 0 && tick < 8000 {
            let k = usize::try_from(tick / 97).unwrap();
            let material = ids[k % ids.len()];
            let x = i64::try_from((k * 37) % 128).unwrap();
            let y = if material == ids[2] { 120 } else { 4 };
            w.paint(x, y, 3, material, Brush::Pour).unwrap();
        }
        w.tick();
    }
    w.state_hash()
}

#[test]
fn same_seed_and_script_give_the_same_hash_after_10k_ticks() {
    let a = scripted_run(42, 10_000);
    let b = scripted_run(42, 10_000);
    assert_eq!(a, b);
    // The hash covers the random choices: another seed diverges.
    assert_ne!(a, scripted_run(43, 10_000));
}

#[test]
fn non_reacting_mass_is_conserved() {
    let mut w = world(160, 128, 7, 0.8);
    let (sand, gravel, rock, water) = (
        id(&w, "sand"),
        id(&w, "gravel"),
        id(&w, "rock"),
        id(&w, "water"),
    );
    w.fill_rect(
        Rect {
            x0: 60,
            y0: 100,
            x1: 100,
            y1: 110,
        },
        rock,
        Brush::Replace,
    )
    .unwrap();
    w.paint(40, 10, 8, sand, Brush::Pour).unwrap();
    w.paint(80, 30, 6, gravel, Brush::Pour).unwrap();
    w.paint(120, 2, 2, water, Brush::Pour).unwrap();
    let before = w.material_counts();
    for _ in 0..1500 {
        w.tick();
        assert_eq!(w.material_counts(), before);
    }
}

#[test]
fn settled_scene_sleeps_within_one_second_of_the_last_change() {
    let mut w = world(192, 128, 3, 0.9);
    let (sand, gravel, water) = (id(&w, "sand"), id(&w, "gravel"), id(&w, "water"));
    for t in 0..300 {
        if t < 200 {
            w.paint(50, 2, 1, sand, Brush::Pour).unwrap();
            w.paint(140, 2, 0, gravel, Brush::Pour).unwrap();
            w.paint(100, 2, 0, water, Brush::Pour).unwrap();
        }
        w.tick();
    }
    let mut slept = false;
    for _ in 0..5000 {
        w.tick();
        let idle = w.tick_count() - w.last_change_tick();
        if idle >= u64::from(SLEEP_TICKS) {
            assert_eq!(w.awake_chunks(), 0, "awake {idle} ticks after last change");
            slept = true;
            break;
        }
        assert!(
            w.awake_chunks() > 0,
            "asleep only {idle} ticks after a change"
        );
    }
    assert!(slept, "the scene never settled");
    // Asleep means no work: further ticks change nothing.
    let hash = w.state_hash();
    let cells = w.cells().to_vec();
    w.tick();
    assert_eq!(w.cells(), &cells[..]);
    assert_ne!(hash, w.state_hash(), "the hash includes the tick");
    assert_eq!(w.dirty_rects().count(), 0);
}

fn release_bubbles(water_level: f32) {
    let mut w = world(96, 128, 11, water_level);
    let (bubble, air) = (id(&w, "air_bubble"), id(&w, "air"));
    let surface = w.surface_row();
    let air_before = positions(&w, air).len();
    let xs: Vec<i64> = (0..12).map(|i| 4 + i * 8).collect();
    for &x in &xs {
        w.paint(x, 127, 0, bubble, Brush::Pour).unwrap();
    }
    let mut highest = usize::MAX;
    for _ in 0..400 {
        w.tick();
        let ps = positions(&w, bubble);
        if let Some(top) = ps.iter().map(|p| p.1).min() {
            highest = highest.min(top);
        }
        if ps.is_empty() {
            break;
        }
    }
    assert!(
        positions(&w, bubble).is_empty(),
        "bubbles left in the water"
    );
    // Every bubble rose to the configured surface (the top edge when full)
    // and popped there, never above it.
    assert_eq!(highest, surface);
    assert_eq!(positions(&w, air).len(), air_before + xs.len());
}

#[test]
fn bubbles_released_at_the_bottom_reach_the_water_level() {
    release_bubbles(0.5);
    release_bubbles(0.8);
    release_bubbles(1.0);
}

/// Least-squares slope of `ys` against `xs`.
#[allow(clippy::cast_precision_loss)]
fn slope(points: &[(usize, usize)]) -> f64 {
    let n = points.len() as f64;
    let mx = points.iter().map(|p| p.0 as f64).sum::<f64>() / n;
    let my = points.iter().map(|p| p.1 as f64).sum::<f64>() / n;
    let sxy: f64 = points
        .iter()
        .map(|p| (p.0 as f64 - mx) * (p.1 as f64 - my))
        .sum();
    let sxx: f64 = points.iter().map(|p| (p.0 as f64 - mx).powi(2)).sum();
    sxy / sxx
}

#[test]
fn sand_pile_slope_is_35_degrees() {
    let (width, height) = (240, 96);
    let mut w = world(width, height, 5, 0.95);
    let sand = id(&w, "sand");
    let centre = 120;
    for _ in 0..1100 {
        w.fill_rect(
            Rect {
                x0: centre - 1,
                y0: 0,
                x1: centre + 2,
                y1: 1,
            },
            sand,
            Brush::Pour,
        )
        .unwrap();
        w.tick();
    }
    settle(&mut w, 2000);
    // Pile height above the floor in each column.
    let heights: Vec<usize> = (0..width)
        .map(|x| {
            (0..height)
                .find(|&y| w.cell(x, y).unwrap().material == sand)
                .map_or(0, |y| height - y)
        })
        .collect();
    let peak = *heights.iter().max().unwrap();
    assert!(peak > 30, "pile too small to measure: {peak}");
    // Fit each flank between 20% and 80% of the peak height.
    let (lo, hi) = (peak / 5, peak * 4 / 5);
    for (name, flank) in [("left", 0..centre), ("right", centre..width)] {
        let points: Vec<_> = flank
            .map(|x| (x, heights[x]))
            .filter(|&(_, h)| h >= lo && h <= hi)
            .collect();
        let angle = slope(&points).abs().atan().to_degrees();
        println!("{name} flank: {angle:.1}° over {} columns", points.len());
        assert!(
            (30.0..=40.0).contains(&angle),
            "{name} flank slope {angle:.1}°"
        );
    }
}

#[test]
fn flake_floats_for_its_float_time_then_sinks() {
    let mut w = world(64, 64, 9, 0.5);
    let flake = id(&w, "flake_food");
    let float_time = w.materials().get(flake).unwrap().float_time;
    let surface = w.surface_row();
    w.set_cell(32, 4, flake).unwrap();
    let (mut arrived, mut left) = (None, None);
    for _ in 0..1000 {
        w.tick();
        let ps = positions(&w, flake);
        assert_eq!(ps.len(), 1);
        let (_, y) = ps[0];
        assert!(arrived.is_some() || y < surface, "fell into the water");
        if arrived.is_none() && y == surface - 1 {
            arrived = Some(w.tick_count());
        }
        if arrived.is_some() && left.is_none() && y >= surface {
            left = Some(w.tick_count());
        }
        if y == 63 {
            break;
        }
    }
    let (arrived, left) = (arrived.unwrap(), left.expect("never sank"));
    let tick_secs = 1.0 / f64::from(TICKS_PER_SECOND);
    #[allow(clippy::cast_precision_loss)]
    let floated = (left - arrived) as f64 * tick_secs;
    assert!(
        (floated - f64::from(float_time)).abs() <= 2.0 * tick_secs,
        "floated {floated} s, float_time {float_time} s"
    );
    assert_eq!(positions(&w, flake), vec![(32, 63)], "sank to the floor");
}

#[test]
fn liquid_spreads_flat_up_to_its_dispersion() {
    // No water: a dry tank with a column of water dropped into it.
    let mut w = world(64, 32, 1, 0.0);
    let water = id(&w, "water");
    let dispersion = usize::from(w.materials().get(water).unwrap().dispersion);
    w.fill_rect(
        Rect {
            x0: 30,
            y0: 0,
            x1: 31,
            y1: 20,
        },
        water,
        Brush::Replace,
    )
    .unwrap();
    // Watch one tick's sideways spread at the floor.
    let mut prev = positions(&w, water);
    for _ in 0..200 {
        w.tick();
        let now = positions(&w, water);
        let xs = |ps: &[(usize, usize)]| {
            let min = ps.iter().map(|p| p.0).min().unwrap();
            let max = ps.iter().map(|p| p.0).max().unwrap();
            (min, max)
        };
        let ((a0, a1), (b0, b1)) = (xs(&prev), xs(&now));
        assert!(a0.saturating_sub(b0) <= dispersion && b1.saturating_sub(a1) <= dispersion);
        prev = now;
    }
    settle(&mut w, 1000);
    // Settled: a surface steps by at most one cell per `dispersion + 1`
    // columns, so 20 cells end at most two rows deep, all on the floor.
    let ps = positions(&w, water);
    assert_eq!(ps.len(), 20);
    assert!(ps.iter().all(|p| p.1 >= 30), "{ps:?}");
}
