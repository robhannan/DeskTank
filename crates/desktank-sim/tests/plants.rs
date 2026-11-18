//! SPEC T6 acceptance tests: plant growth and wilting, moss, algae with
//! overfeeding and a water change, and the water-quality score.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use desktank_sim::{Brush, FieldKind, MaterialId, MaterialTable, Rect, World, WorldConfig};

/// Plant cells a fed and lit cutting grows in 10 simulated minutes, at
/// least. Measured (PR for T6) on seeds 0–5 in [`planted`]: 185–235 cells
/// (stem 42–48, leaves 100–132, roots 43–61); N leaves a margin below the
/// smallest.
const N: usize = 150;
/// Ticks in a simulated minute.
const MINUTE: u32 = 1200;

fn world(width: usize, height: usize, seed: u64) -> World {
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

fn id(w: &World, name: &str) -> MaterialId {
    w.materials().id(name).unwrap()
}

fn count(w: &World, names: &[&str]) -> usize {
    let counts = w.material_counts();
    names.iter().map(|n| counts[usize::from(id(w, n).0)]).sum()
}

fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Rect {
    Rect { x0, y0, x1, y1 }
}

fn run(w: &mut World, ticks: u32) {
    for _ in 0..ticks {
        w.tick();
    }
}

const PLANT: &[&str] = &["stem", "leaf", "root"];

/// A tank with a substrate bed `bed` rows deep and one cutting dropped at
/// `x`.
fn planted(seed: u64, substrate: &str, bed: usize, x: i64) -> World {
    let mut w = world(160, 120, seed);
    let s = id(&w, substrate);
    w.fill_rect(rect(0, 120 - bed, 160, 120), s, Brush::Replace)
        .unwrap();
    let cutting = id(&w, "cutting");
    w.paint(x, 20, 0, cutting, Brush::Pour).unwrap();
    w
}

#[test]
fn a_fed_and_lit_plant_grows_at_least_n_cells_in_10_minutes() {
    for seed in 0..3 {
        let mut w = planted(seed, "aquasoil", 12, 80);
        run(&mut w, 10 * MINUTE);
        let cells = count(&w, PLANT);
        assert!(cells >= N, "seed {seed}: grew {cells} cells, want >= {N}");
        for part in PLANT {
            assert!(count(&w, &[part]) > 0, "seed {seed}: no {part}");
        }
        // The stem stands on its roots and stops at its full height.
        assert!(count(&w, &["stem"]) <= 48);
    }
}

#[test]
fn unfed_or_unlit_cuttings_dont_grow() {
    // Sand holds no nutrients.
    let mut w = planted(1, "sand", 12, 80);
    run(&mut w, 5 * MINUTE);
    assert_eq!(count(&w, PLANT), 0);
    assert_eq!(count(&w, &["cutting"]), 1);
    // Fed but in the dark under a glued lid.
    let mut w = planted(1, "aquasoil", 12, 80);
    lid(&mut w);
    run(&mut w, 5 * MINUTE);
    assert_eq!(count(&w, PLANT), 0);
}

/// Glues a slab of rock across the top two field rows of water, which
/// blocks all but 1% of the light.
fn lid(w: &mut World) {
    let rock = id(w, "rock");
    w.fill_rect(rect(0, 8, w.width(), 24), rock, Brush::Replace)
        .unwrap();
    let body = w.body_at(0, 8).unwrap();
    assert!(w.set_anchored(body, true));
}

#[test]
fn plants_wilt_without_light() {
    let mut w = planted(2, "aquasoil", 12, 80);
    run(&mut w, 10 * MINUTE);
    let grown = count(&w, &["stem", "leaf"]);
    assert!(grown >= 100);
    lid(&mut w);
    run(&mut w, 2 * MINUTE);
    let dead = count(&w, &["dead_plant", "detritus"]);
    assert!(dead > 0, "wilted cells become dead plant matter");
    run(&mut w, 8 * MINUTE);
    let left = count(&w, &["stem", "leaf"]);
    assert!(
        left * 10 <= grown,
        "{left} of {grown} cells left in the dark"
    );
}

#[test]
fn plants_wilt_without_nutrients() {
    let mut w = planted(3, "aquasoil", 12, 80);
    run(&mut w, 10 * MINUTE);
    let grown = count(&w, PLANT);
    // Starve it: take the aquasoil away and drain the field every tick.
    let water = id(&w, "water");
    w.fill_rect(rect(0, 108, 160, 120), water, Brush::Replace)
        .unwrap();
    let before_count = count(&w, PLANT);
    for _ in 0..10 * MINUTE {
        for fy in 0..w.field(FieldKind::Nutrient).height() {
            for fx in 0..w.field(FieldKind::Nutrient).width() {
                w.add_to_field(FieldKind::Nutrient, fx * 8, fy * 8, -1.0);
            }
        }
        w.tick();
    }
    let left = count(&w, PLANT);
    assert!(before_count > 0 && grown >= N);
    assert!(
        left * 10 <= before_count,
        "{left} of {before_count} cells left unfed"
    );
}

#[test]
fn co2_speeds_growth() {
    let grow = |co2: bool| {
        let mut w = planted(4, "aquasoil", 12, 80);
        for _ in 0..4 * MINUTE {
            if co2 {
                for fy in 0..w.field(FieldKind::Co2).height() {
                    for fx in 0..w.field(FieldKind::Co2).width() {
                        let v = w.field(FieldKind::Co2).get(fx, fy);
                        w.add_to_field(FieldKind::Co2, fx * 8, fy * 8, 1.0 - v);
                    }
                }
            }
            w.tick();
        }
        count(&w, &["stem", "leaf"])
    };
    let (with, without) = (grow(true), grow(false));
    assert!(
        with as f64 >= 1.4 * without as f64,
        "with CO₂ {with}, without {without}"
    );
}

#[test]
fn moss_grows_over_rock_and_unattached_moss_falls_off() {
    let mut w = world(160, 120, 5);
    let (rock, moss) = (id(&w, "rock"), id(&w, "moss"));
    w.fill_rect(rect(60, 70, 100, 90), rock, Brush::Replace)
        .unwrap();
    let body = w.body_at(60, 70).unwrap();
    assert!(w.set_anchored(body, true));
    // A blob of moss over the rock's top left corner: some of it touches
    // the rock, the rest hangs in the water.
    w.paint(60, 70, 6, moss, Brush::Pour).unwrap();
    let poured = count(&w, &["moss"]);
    run(&mut w, 3 * MINUTE);
    let attached = |w: &World| {
        let mut n = 0;
        for y in 1..w.height() - 1 {
            for x in 1..w.width() - 1 {
                if w.cell(x, y).unwrap().material == moss {
                    let touches = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dy)| {
                        let c = w.cell(x.wrapping_add_signed(dx), y.wrapping_add_signed(dy));
                        c.is_some_and(|c| c.material == rock)
                    });
                    assert!(touches, "moss at ({x}, {y}) isn't on the rock");
                    n += 1;
                }
            }
        }
        n
    };
    let kept = attached(&w);
    assert!(kept > 0 && kept < poured, "kept {kept} of {poured}");
    assert!(count(&w, &["dead_plant", "detritus"]) > 0);
    // It creeps along the rock: water holds a little nutrient here.
    for fy in 0..w.field(FieldKind::Nutrient).height() {
        for fx in 0..w.field(FieldKind::Nutrient).width() {
            w.add_to_field(FieldKind::Nutrient, fx * 8, fy * 8, 0.2);
        }
    }
    run(&mut w, 10 * MINUTE);
    let grown = attached(&w);
    assert!(grown >= kept + 20, "moss grew from {kept} to {grown}");
}

/// A sand-bottomed tank with no plants, fed far too much: a pinch of
/// pellets every 15 s for the first 10 minutes. Each pellet rots into
/// detritus after 10 minutes, which breaks down into nutrients 3 minutes
/// later. Returns the tank and a stepper that feeds and ticks once.
fn overfed(seed: u64) -> (World, impl FnMut(&mut World)) {
    let mut w = world(320, 180, seed);
    let sand = id(&w, "sand");
    w.fill_rect(rect(0, 168, 320, 180), sand, Brush::Replace)
        .unwrap();
    let pellet = id(&w, "sinking_pellet");
    let mut t = 0u32;
    let step = move |w: &mut World| {
        if t.is_multiple_of(300) && t / 300 < 40 {
            let k = i64::from(t / 300);
            w.paint(20 + (k * 97) % 280, 20, 3, pellet, Brush::Pour)
                .unwrap();
        }
        t += 1;
        w.tick();
    };
    (w, step)
}

#[test]
fn overfeeding_grows_algae_and_a_water_change_clears_it() {
    let (mut w, mut step) = overfed(0);
    let mut worst = 1.0_f32;
    for _ in 0..30 * MINUTE {
        step(&mut w);
        if w.tick_count().is_multiple_of(u64::from(MINUTE)) {
            worst = worst.min(w.ecosystem().quality);
        }
    }
    let before = w.ecosystem();
    assert!(
        before.algae_coverage > 0.05,
        "algae covers {:.1}% after 30 minutes",
        before.algae_coverage * 100.0
    );
    assert!(before.mean_nutrient > 0.6, "{before:?}");
    // Rotting food fouls the water.
    assert!(worst < 0.9, "quality never fell below {worst}");
    w.water_change();
    let after_change = w.ecosystem();
    assert!(after_change.quality > before.quality);
    assert!(after_change.mean_nutrient < 0.5 * before.mean_nutrient + 1e-3);
    for _ in 0..30 * MINUTE {
        step(&mut w);
    }
    let after = w.ecosystem();
    assert!(
        after.algae_coverage < 0.01,
        "algae still covers {:.1}% 30 minutes after the water change",
        after.algae_coverage * 100.0
    );
}

#[test]
fn a_water_change_siphons_debris_and_dilutes_the_fields() {
    let mut w = world(160, 120, 6);
    for (name, x) in [
        ("waste", 20),
        ("detritus", 50),
        ("dead_plant", 80),
        ("sinking_pellet", 110),
    ] {
        let m = id(&w, name);
        w.paint(x, 100, 3, m, Brush::Pour).unwrap();
    }
    let rock = id(&w, "rock");
    w.paint(140, 100, 3, rock, Brush::Replace).unwrap();
    w.add_to_field(FieldKind::Nutrient, 40, 40, 2.0);
    w.add_to_field(FieldKind::Co2, 40, 40, 1.0);
    let debris = count(&w, &["waste", "detritus", "dead_plant", "sinking_pellet"]);
    let rocks = count(&w, &["rock"]);
    let polluted = w.ecosystem();
    assert!(polluted.pollutant_cells > 0);
    assert_eq!(w.water_change(), debris);
    assert_eq!(
        count(&w, &["waste", "detritus", "dead_plant", "sinking_pellet"]),
        0
    );
    assert_eq!(count(&w, &["rock"]), rocks);
    assert!((w.field(FieldKind::Nutrient).total() - 1.0).abs() < 1e-5);
    assert!((w.field(FieldKind::Co2).total() - 0.5).abs() < 1e-5);
    let clean = w.ecosystem();
    assert_eq!(clean.pollutant_cells, 0);
    assert!(
        clean.quality > polluted.quality,
        "{clean:?} vs {polluted:?}"
    );
}

#[test]
fn plants_raise_water_quality() {
    // Nutrients just over the comfortable level cost some quality; a
    // planted tank earns it back.
    let quality = |plants: bool| {
        let mut w = planted(1, "sand", 12, 80);
        if plants {
            let (stem, leaf) = (id(&w, "stem"), id(&w, "leaf"));
            w.fill_rect(rect(70, 40, 71, 108), stem, Brush::Replace)
                .unwrap();
            w.fill_rect(rect(71, 40, 74, 108), leaf, Brush::Replace)
                .unwrap();
        }
        for fy in 0..w.field(FieldKind::Nutrient).height() {
            for fx in 0..w.field(FieldKind::Nutrient).width() {
                w.add_to_field(FieldKind::Nutrient, fx * 8, fy * 8, 0.8);
            }
        }
        w.ecosystem()
    };
    let (bare, planted) = (quality(false), quality(true));
    assert!(bare.quality < 1.0 && planted.plant_cells > 200);
    assert!(planted.quality > bare.quality, "{planted:?} vs {bare:?}");
}

#[test]
fn a_planted_tank_is_deterministic_and_falls_asleep_once_grown() {
    let grow = |seed| {
        let mut w = planted(seed, "aquasoil", 12, 50);
        let seed_id = id(&w, "seed");
        w.paint(110, 20, 1, seed_id, Brush::Pour).unwrap();
        run(&mut w, 30 * MINUTE);
        w
    };
    let (a, b) = (grow(7), grow(7));
    assert_eq!(a.state_hash(), b.state_hash());
    assert_ne!(a.state_hash(), grow(8).state_hash());
    // Grown plants only wake chunks when they change; a grown tank sleeps.
    let mut w = a;
    assert!(count(&w, PLANT) > 2 * N);
    let mut asleep = 0;
    for _ in 0..10 * MINUTE {
        w.tick();
        asleep = if w.awake_chunks() == 0 { asleep + 1 } else { 0 };
    }
    assert!(asleep > 0, "still awake after 40 minutes");
}
