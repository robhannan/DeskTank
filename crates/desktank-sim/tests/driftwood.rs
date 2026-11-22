//! Driftwood waterlogging, wicking, buoyancy loss, air drying, gluing,
//! rotation, splitting, sleep discipline and save determinism (Issue #102).

use desktank_sim::save::{self, FORMAT_VERSION};
use desktank_sim::{
    Brush, DRIFTWOOD_SOAK_EVERY, GLUED_VARIANT_BIT, MAX_DRIFTWOOD_WETNESS, MaterialId,
    MaterialTable, Rect, TICKS_PER_SECOND, World, WorldConfig, driftwood_wetness,
};

fn table() -> MaterialTable {
    MaterialTable::builtin().unwrap()
}

fn world(width: usize, height: usize, seed: u64, water_level: f32) -> World {
    World::new(
        WorldConfig {
            width,
            height,
            seed,
            water_level,
        },
        table(),
    )
    .unwrap()
}

fn id(w: &World, name: &str) -> MaterialId {
    w.materials().id(name).unwrap()
}

fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Rect {
    Rect { x0, y0, x1, y1 }
}

fn wood_cells(w: &World, wood: MaterialId) -> Vec<(usize, usize, u8)> {
    w.cells()
        .iter()
        .enumerate()
        .filter(|(_, c)| c.material == wood)
        .map(|(i, c)| (i % w.width(), i / w.width(), c.variant))
        .collect()
}

#[allow(clippy::cast_precision_loss)]
fn mean_wetness(w: &World, wood: MaterialId) -> f64 {
    let cells = wood_cells(w, wood);
    assert_ne!(cells.len(), 0);
    let sum: u64 = cells
        .iter()
        .map(|&(_, _, v)| u64::from(driftwood_wetness(v)))
        .sum();
    sum as f64 / cells.len() as f64
}

fn settle(w: &mut World, limit: usize) {
    for _ in 0..limit {
        w.tick();
        if w.awake_chunks() == 0 {
            return;
        }
    }
}

/// A typical floating piece of driftwood rises to the waterline, absorbs
/// water from its wet edges inward (soaking beating air drying), crosses
/// neutral buoyancy within 15–30 minutes of world time, sinks gently onto
/// the sand bed without burying itself, reaches saturation and sleeps with
/// zero dirty rectangles.
#[test]
fn floating_driftwood_waterlogs_and_sinks_within_medium_timescale() {
    let mut w = world(96, 96, 17, 0.80);
    let wood = id(&w, "driftwood");
    let sand = id(&w, "sand");
    w.fill_rect(rect(0, 84, 96, 96), sand, Brush::Replace)
        .unwrap();
    let sand_before = w.material_counts()[usize::from(sand.0)];
    w.paint(48, 55, 5, wood, Brush::Pour).unwrap();
    let body = w.bodies().find(|b| b.material == wood).unwrap().id;
    let wood_count = w.material_counts()[usize::from(wood.0)];

    // Starts completely dry at 0.7 density.
    assert!((mean_wetness(&w, wood) - 0.0).abs() < f64::EPSILON);
    assert!((w.body_density(body).unwrap() - 0.7).abs() < 0.01);

    // Rises to the surface and rests straddling the waterline.
    settle(&mut w, 400);
    assert_eq!(w.awake_chunks(), 0);
    let b = w.body(body).unwrap();
    let surface = w.surface_row();
    assert!(
        b.bounds.y0 < surface && b.bounds.y1 > surface,
        "floating bounds {:?}, surface {surface}",
        b.bounds
    );

    // After a few soak steps, edge cells touching water are wetter than the
    // dry interior/top cells, and soaking does not wake any chunk while the
    // body hasn't moved.
    let mut saw_gradient = false;
    let mut sink_tick = None;
    let max_ticks = 35 * 60 * u64::from(TICKS_PER_SECOND);
    for _ in 0..max_ticks {
        w.tick();
        if !saw_gradient && w.tick_count() >= DRIFTWOOD_SOAK_EVERY * 10 {
            let cells = wood_cells(&w, wood);
            let min_w = cells
                .iter()
                .map(|&(_, _, v)| driftwood_wetness(v))
                .min()
                .unwrap();
            let max_w = cells
                .iter()
                .map(|&(_, _, v)| driftwood_wetness(v))
                .max()
                .unwrap();
            if max_w > min_w {
                saw_gradient = true;
            }
        }
        let b = w.body(body).unwrap();
        if sink_tick.is_none() && b.bounds.y0 >= surface {
            sink_tick = Some(w.tick_count());
        }
        if sink_tick.is_some() && b.bounds.y1 >= 84 && w.awake_chunks() == 0 {
            break;
        }
    }
    assert!(saw_gradient, "per-cell wicking should form a gradient");
    let sink_tick = sink_tick.expect("floating driftwood should sink below the surface");
    let sink_minutes =
        f64::from(u32::try_from(sink_tick).unwrap()) / (60.0 * f64::from(TICKS_PER_SECOND));
    assert!(
        (15.0..=30.0).contains(&sink_minutes),
        "expected sinking in 15–30 min, sank at {sink_minutes:.1} min (tick {sink_tick})"
    );

    // Rests on top of the sand bed without burying itself or losing cells.
    let b = w.body(body).unwrap();
    assert!(
        b.bounds.y1 >= 84 && b.bounds.y1 <= 86,
        "wood should settle gently on sand (y=84), bounds {:?}",
        b.bounds
    );
    assert_eq!(w.material_counts()[usize::from(wood.0)], wood_count);
    assert_eq!(w.material_counts()[usize::from(sand.0)], sand_before);

    // Run until fully saturated, then verify it settles quietly with zero
    // dirty rects and zero awake chunks across multiple soak intervals.
    for _ in 0..(DRIFTWOOD_SOAK_EVERY * 80) {
        w.tick();
    }
    assert!(
        w.body_density(body).unwrap() >= 1.15,
        "saturated density {}",
        w.body_density(body).unwrap()
    );
    assert_eq!(w.awake_chunks(), 0);
    for _ in 0..(DRIFTWOOD_SOAK_EVERY * 3) {
        w.tick();
        assert_eq!(w.awake_chunks(), 0);
        assert_eq!(w.dirty_rects().count(), 0);
    }
}

/// Even a thin 2-row floating slab (half in air, half in water) reliably
/// waterlogs and sinks instead of stalling at the waterline.
#[test]
fn flat_floating_driftwood_at_waterline_still_sinks() {
    let mut w = world(64, 64, 5, 0.75);
    let wood = id(&w, "driftwood");
    w.fill_rect(rect(24, 30, 36, 32), wood, Brush::Replace)
        .unwrap();
    let body = w.bodies().find(|b| b.material == wood).unwrap().id;
    settle(&mut w, 300);
    let surface = w.surface_row();
    assert!(w.body(body).unwrap().bounds.y0 <= surface);

    for _ in 0..(30 * 60 * u64::from(TICKS_PER_SECOND)) {
        w.tick();
    }
    let b = w.body(body).unwrap();
    assert_eq!(
        b.bounds.y1,
        w.height(),
        "thin floating slab should sink to floor, bounds {:?}",
        b.bounds
    );
}

/// A dry driftwood body in air touching a waterlogged driftwood body wicks
/// moisture across the contact and darkens, even without touching water.
#[test]
fn wicking_spreads_from_waterlogged_driftwood_to_touching_dry_driftwood() {
    let mut w = world(64, 64, 9, 0.5);
    let wood = id(&w, "driftwood");
    let surface = w.surface_row();

    // Body A: glued underwater up to the waterline.
    w.fill_rect(rect(28, surface, 34, surface + 6), wood, Brush::Replace)
        .unwrap();
    let body_a = w.body_at(30, surface + 1).unwrap();
    w.set_anchored(body_a, true);

    // Body B: separate glued body sitting in air right on top of Body A.
    w.fill_rect(rect(28, surface - 4, 34, surface), wood, Brush::Replace)
        .unwrap();
    let body_b = w.body_at(30, surface - 2).unwrap();
    assert_ne!(body_a, body_b);
    w.set_anchored(body_b, true);

    for _ in 0..(DRIFTWOOD_SOAK_EVERY * 40) {
        w.tick();
    }

    // Body B's bottom row (touching Body A, not water) wicked moisture!
    let bottom_b_wet = driftwood_wetness(w.cell(30, surface - 1).unwrap().variant);
    let top_a_wet = driftwood_wetness(w.cell(30, surface).unwrap().variant);
    assert!(top_a_wet > 30, "body A soaked: {top_a_wet}");
    assert!(
        bottom_b_wet > 10,
        "body B wicked from body A: {bottom_b_wet}"
    );
    assert!(
        bottom_b_wet < top_a_wet,
        "gradient across contact: B={bottom_b_wet}, A={top_a_wet}"
    );
}

/// Waterlogged driftwood exposed to air dries back out, lightens toward 0,
/// regains buoyancy (< 1.0), and floats again when water returns.
#[test]
fn exposed_to_air_dries_out_and_regains_buoyancy() {
    let mut w = world(64, 64, 11, 0.85);
    let wood = id(&w, "driftwood");
    let water = id(&w, "water");
    let air = id(&w, "air");

    w.fill_rect(rect(28, 40, 36, 46), wood, Brush::Replace)
        .unwrap();
    let body = w.bodies().find(|b| b.material == wood).unwrap().id;

    // Soak until it sinks and rests on the floor.
    for _ in 0..(DRIFTWOOD_SOAK_EVERY * 135) {
        w.tick();
    }
    assert_eq!(w.body(body).unwrap().bounds.y1, w.height());
    assert!(w.body_density(body).unwrap() > 1.15);

    // Drain the tank (replace all water with air).
    for y in 0..w.height() {
        for x in 0..w.width() {
            if w.cell(x, y).unwrap().material == water {
                w.set_cell(x, y, air).unwrap();
            }
        }
    }

    // Let it dry completely in air.
    for _ in 0..(DRIFTWOOD_SOAK_EVERY * 270) {
        w.tick();
    }
    assert!(
        (mean_wetness(&w, wood) - 0.0).abs() < f64::EPSILON,
        "should dry completely to 0, got {}",
        mean_wetness(&w, wood)
    );
    assert!((w.body_density(body).unwrap() - 0.7).abs() < 0.01);

    // Dry piece in air sleeps with zero dirty rects.
    settle(&mut w, 40);
    for _ in 0..(DRIFTWOOD_SOAK_EVERY * 3) {
        w.tick();
        assert_eq!(w.awake_chunks(), 0);
        assert_eq!(w.dirty_rects().count(), 0);
    }

    // Refill water: the dried piece wakes and floats back up to the surface!
    let surface = w.surface_row();
    for y in surface..w.height() {
        for x in 0..w.width() {
            if w.cell(x, y).unwrap().material == air {
                w.set_cell(x, y, water).unwrap();
            }
        }
    }
    settle(&mut w, 400);
    let b = w.body(body).unwrap();
    assert!(
        b.bounds.y0 < surface + 2,
        "dried driftwood should float back up to surface {surface}, bounds {:?}",
        b.bounds
    );
}

/// Glued driftwood stays anchored in place, keeps its glue bit, darkens in
/// water, forms a stable gradient when half-in-water, and settles with zero
/// dirty rectangles.
#[test]
fn glued_driftwood_stays_anchored_darkens_and_settles() {
    let mut w = world(64, 64, 19, 0.5);
    let wood = id(&w, "driftwood");
    let surface = w.surface_row();

    // A vertical glued branch straddling the waterline (half in air, half in water).
    let branch_rect = rect(30, surface - 12, 33, surface + 12);
    w.fill_rect(branch_rect, wood, Brush::Replace).unwrap();
    let body = w.body_at(31, surface).unwrap();
    w.set_anchored(body, true);

    for _ in 0..(DRIFTWOOD_SOAK_EVERY * 160) {
        w.tick();
    }

    // Never moved, every cell still has GLUED_VARIANT_BIT set.
    assert_eq!(w.body(body).unwrap().bounds, branch_rect);
    for (_, _, v) in wood_cells(&w, wood) {
        assert_ne!(v & GLUED_VARIANT_BIT, 0, "glued bit must be preserved");
    }

    // Submerged edge is fully waterlogged (127), interior is 126, top in air is much drier.
    let wet_edge = driftwood_wetness(w.cell(30, surface + 10).unwrap().variant);
    let wet_core = driftwood_wetness(w.cell(31, surface + 10).unwrap().variant);
    let dry_top = driftwood_wetness(w.cell(30, surface - 12).unwrap().variant);
    assert_eq!(wet_edge, MAX_DRIFTWOOD_WETNESS);
    assert_eq!(wet_core, MAX_DRIFTWOOD_WETNESS - 1);
    assert!(
        dry_top < wet_edge,
        "top in air ({dry_top}) should be drier than submerged ({wet_edge})"
    );

    // Reaches a static fixed point: zero dirty rects, zero awake chunks.
    settle(&mut w, 40);
    for _ in 0..(DRIFTWOOD_SOAK_EVERY * 3) {
        w.tick();
        assert_eq!(w.awake_chunks(), 0);
        assert_eq!(w.dirty_rects().count(), 0);
    }
}

/// Rotating a waterlogged driftwood body (tipping off a ledge) and splitting
/// one with a scoop both preserve per-cell wetness.
#[test]
fn rotation_and_splitting_preserve_per_cell_wetness() {
    let mut w = world(96, 64, 23, 1.0);
    let wood = id(&w, "driftwood");
    let rock = id(&w, "rock");
    let water = id(&w, "water");

    // Glued rock ledge so a waterlogged driftwood bar overhanging it will tip.
    w.fill_rect(rect(20, 40, 34, 44), rock, Brush::Replace)
        .unwrap();
    let ledge = w.body_at(25, 42).unwrap();
    w.set_anchored(ledge, true);

    // Glue the driftwood bar on top of the ledge while soaking it.
    w.fill_rect(rect(28, 36, 52, 40), wood, Brush::Replace)
        .unwrap();
    let bar = w.body_at(35, 38).unwrap();
    w.set_anchored(bar, true);

    for _ in 0..(DRIFTWOOD_SOAK_EVERY * 130) {
        w.tick();
    }
    let wet_before = mean_wetness(&w, wood);
    assert!(wet_before > 115.0, "bar soaked to {wet_before}");

    // Unglue: the waterlogged bar sinks and tips clockwise off the ledge!
    w.set_anchored(bar, false);
    let mut rotated = false;
    for _ in 0..300 {
        w.tick();
        if w.body(bar).unwrap().angle != 0 {
            rotated = true;
        }
    }
    assert!(rotated, "overhanging waterlogged bar should tip and rotate");
    let wet_after_turn = mean_wetness(&w, wood);
    assert!(
        wet_after_turn >= wet_before - 5.0,
        "rotation must preserve wetness: before={wet_before}, after={wet_after_turn}"
    );

    // Now split a second waterlogged driftwood bar by scooping its middle.
    let mut w2 = world(64, 64, 29, 1.0);
    w2.fill_rect(rect(16, 30, 48, 34), wood, Brush::Replace)
        .unwrap();
    let b2 = w2.body_at(20, 32).unwrap();
    w2.set_anchored(b2, true);
    for _ in 0..(DRIFTWOOD_SOAK_EVERY * 60) {
        w2.tick();
    }
    let pre_split = mean_wetness(&w2, wood);
    assert!(pre_split > 50.0);
    w2.fill_rect(rect(30, 28, 34, 36), water, Brush::Replace)
        .unwrap();
    w2.tick();
    let pieces: Vec<_> = w2.bodies().filter(|b| b.material == wood).collect();
    assert_eq!(pieces.len(), 2, "scooping middle splits driftwood in two");
    let post_split = mean_wetness(&w2, wood);
    assert!(
        (post_split - pre_split).abs() < 2.0,
        "splitting preserves wetness: before={pre_split}, after={post_split}"
    );
}

/// Saving and loading mid-soak preserves exact per-cell wetness, density,
/// state hash, and deterministic evolution.
#[test]
fn save_v11_round_trip_and_determinism() {
    let mut w = world(64, 64, 31, 0.75);
    let wood = id(&w, "driftwood");
    w.paint(24, 36, 4, wood, Brush::Pour).unwrap();
    w.paint(44, 24, 3, wood, Brush::Pour).unwrap();
    let glued = w.body_at(44, 24).unwrap();
    w.set_anchored(glued, true);

    for _ in 0..(DRIFTWOOD_SOAK_EVERY * 35) {
        w.tick();
    }
    assert!(mean_wetness(&w, wood) > 10.0);

    let bytes = save::encode(&w);
    assert_eq!(save::check(&bytes), Ok(FORMAT_VERSION));
    let mut loaded = save::decode(&bytes, table()).unwrap();
    assert_eq!(loaded.state_hash(), w.state_hash());
    assert_eq!(wood_cells(&loaded, wood), wood_cells(&w, wood));

    for _ in 0..(DRIFTWOOD_SOAK_EVERY * 5) {
        w.tick();
        loaded.tick();
    }
    assert_eq!(loaded.state_hash(), w.state_hash());
}
