//! SPEC T5 acceptance tests: each reaction-table rule, fertiliser diffusion,
//! detritus timing, the filter current, the bubbler, the random-tick budget,
//! and sleep and determinism with living cells.
// Statistics over small counts and times.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]

use desktank_sim::{
    Brush, Ecology, FieldKind, MaterialId, MaterialTable, Placeable, Rect, World, WorldConfig,
};

const REACTIONS: &str = include_str!("../../../assets/reactions.toml");
const FIELDS: &str = include_str!("../../../assets/fields.toml");

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

fn count(w: &World, m: MaterialId) -> usize {
    w.material_counts()[usize::from(m.0)]
}

fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Rect {
    Rect { x0, y0, x1, y1 }
}

fn run(w: &mut World, ticks: u32) {
    for _ in 0..ticks {
        w.tick();
    }
}

/// The entries of one array table in `assets/reactions.toml`.
fn entries(kind: &str) -> Vec<toml::Table> {
    let doc: toml::Table = toml::from_str(REACTIONS).unwrap();
    doc.get(kind)
        .and_then(|v| v.as_array())
        .map_or_else(Vec::new, |a| {
            a.iter().map(|v| v.as_table().unwrap().clone()).collect()
        })
}

fn float(t: &toml::Table, key: &str) -> f64 {
    let v = &t[key];
    v.as_float()
        .or_else(|| v.as_integer().map(|i| i as f64))
        .unwrap()
}

/// Scatters `n` single cells of `m` one apart over a band of open water,
/// so each one touches water on every side.
fn scatter(w: &mut World, m: MaterialId, n: usize, y0: usize) {
    let per_row = (w.width() - 8) / 4;
    for k in 0..n {
        w.set_cell(4 + (k % per_row) * 4, y0 + (k / per_row) * 4, m)
            .unwrap();
    }
}

/// Places an internal filter on the side glass near `(x, y)`.
fn place_filter(w: &mut World, x: i64, y: i64) {
    assert!(
        w.add_device(desktank_sim::DeviceKind::InternalFilter, x, y, false)
            .unwrap()
            .is_some(),
        "internal filter should fit at ({x}, {y})"
    );
}

#[test]
fn liquid_fertiliser_dissolves_at_its_rate_and_releases_nutrients() {
    let rule = entries("reaction")
        .into_iter()
        .find(|r| r["a"].as_str() == Some("liquid_fertiliser"))
        .unwrap();
    let (p, amount) = (
        float(&rule, "p"),
        float(rule["release"].as_table().unwrap(), "amount"),
    );
    // More cells than the random-tick budget, so they are sampled.
    let n = 960;
    let mut w = world(256, 256, 3, 1.0);
    let fert = id(&w, "liquid_fertiliser");
    scatter(&mut w, fert, n, 8);
    let secs = 2;
    run(&mut w, secs * 20);
    let left = count(&w, fert) as f64 / n as f64;
    let want = (1.0 - p).powi(i32::try_from(secs).unwrap());
    assert!((left - want).abs() < 0.06, "left {left}, want {want}");
    // Every dissolved cell released its nutrients (the field conserves).
    let dissolved = (n - count(&w, fert)) as f64;
    let total = w.field(FieldKind::Nutrient).total();
    assert!((total - dissolved * amount).abs() < 1e-2 * dissolved * amount);
}

#[test]
fn co2_bubbles_dissolve_only_below_saturation() {
    let rule = entries("reaction")
        .into_iter()
        .find(|r| r["a"].as_str() == Some("co2_bubble"))
        .unwrap();
    let p = float(&rule, "p");
    let saturation = float(rule["when"].as_table().unwrap(), "below") as f32;
    let fresh = || {
        let mut w = world(256, 320, 5, 1.0);
        let co2 = id(&w, "co2_bubble");
        scatter(&mut w, co2, 960, 220);
        w
    };
    let mut w = fresh();
    let (co2, water) = (id(&w, "co2_bubble"), id(&w, "water"));
    run(&mut w, 40);
    let left = count(&w, co2) as f64 / 960.0;
    let want = (1.0 - p).powi(2);
    assert!((left - want).abs() < 0.06, "left {left}, want {want}");
    assert!(w.field(FieldKind::Co2).total() > 0.0);

    // Saturated water: nothing dissolves (no bubble turns into water).
    let mut w = fresh();
    for y in (0..320).step_by(8) {
        for x in (0..256).step_by(8) {
            w.add_to_field(FieldKind::Co2, x, y, saturation + 1.0);
        }
    }
    let before = count(&w, water);
    run(&mut w, 40);
    assert_eq!(count(&w, water), before);
    assert_eq!(count(&w, co2), 960, "bubbles still rising");
}

#[test]
fn reactions_can_convert_both_cells() {
    let t = MaterialTable::builtin().unwrap();
    let eco = Ecology::from_toml(
        "[[reaction]]\na = \"sand\"\nb = \"rock\"\ninto = \"gravel\"\nb_into = \"water\"\np = 1.0",
        FIELDS,
        &t,
        true,
    )
    .unwrap();
    let config = WorldConfig {
        width: 64,
        height: 64,
        seed: 1,
        water_level: 1.0,
    };
    let mut w = World::with_ecology(config, t, eco).unwrap();
    let (sand, rock, gravel, water) = (
        id(&w, "sand"),
        id(&w, "rock"),
        id(&w, "gravel"),
        id(&w, "water"),
    );
    w.set_cell(10, 63, rock).unwrap();
    w.set_cell(11, 63, sand).unwrap();
    w.set_cell(40, 63, sand).unwrap();
    run(&mut w, 2);
    assert_eq!(w.cell(11, 63).unwrap().material, gravel);
    assert_eq!(w.cell(10, 63).unwrap().material, water);
    // No partner, no reaction.
    assert_eq!(w.cell(40, 63).unwrap().material, sand);
}

#[test]
fn every_decay_happens_at_its_configured_time() {
    for rule in entries("decay") {
        let name = rule["material"].as_str().unwrap();
        let after = float(&rule, "after");
        let mut w = world(64, 32, 2, 1.0);
        let (m, into) = (id(&w, name), id(&w, rule["into"].as_str().unwrap()));
        // A row on the floor, so the cells rest and nothing else changes.
        w.fill_rect(rect(8, 31, 40, 32), m, Brush::Replace).unwrap();
        let ticks = (after * 20.0).round() as u64;
        let mut gone_at = None;
        while w.tick_count() < ticks * 12 / 10 {
            w.tick();
            if count(&w, m) == 0 {
                gone_at = Some(w.tick_count());
                break;
            }
        }
        let gone_at = gone_at.unwrap_or_else(|| panic!("{name} never decayed"));
        assert!(
            (gone_at as f64 / ticks as f64 - 1.0).abs() <= 0.1,
            "{name}: gone at {gone_at} ticks, configured {ticks}"
        );
        assert!(count(&w, into) >= 32, "{name} didn't become {into:?}");
        if let Some(r) = rule.get("release").and_then(|r| r.as_table()) {
            let total = w.field(FieldKind::Nutrient).total();
            assert!(
                (total - 32.0 * float(r, "amount")).abs() < 0.01,
                "{name}: released {total}"
            );
        }
    }
}

#[test]
fn detritus_disintegrates_within_10_percent_when_sampled() {
    // Far more detritus than the random-tick budget, so each cell is
    // visited only every few ticks.
    let mut w = world(256, 64, 9, 1.0);
    let detritus = id(&w, "detritus");
    w.fill_rect(rect(0, 52, 256, 64), detritus, Brush::Replace)
        .unwrap();
    let n = count(&w, detritus);
    assert!(n > 10 * w.random_tick_budget());
    let after = entries("decay")
        .into_iter()
        .find(|r| r["material"].as_str() == Some("detritus"))
        .map(|r| float(&r, "after"))
        .unwrap();
    let ticks = (after * 20.0).round() as u64;
    let mut first = None;
    while count(&w, detritus) > 0 {
        assert!(
            w.tick_count() < ticks * 2,
            "detritus outlived twice its time"
        );
        w.tick();
        if first.is_none() && count(&w, detritus) < n {
            first = Some(w.tick_count());
        }
        assert!(w.random_tick_stats().visited <= w.random_tick_budget());
    }
    let (first, last) = (first.unwrap() as f64, w.tick_count() as f64);
    let t = ticks as f64;
    assert!(
        first >= 0.9 * t && last <= 1.1 * t,
        "decayed {first}..{last} ticks, configured {t}"
    );
}

#[test]
fn liquid_fertiliser_spreads_at_the_configured_diffusion_rate() {
    let doc: toml::Table = toml::from_str(FIELDS).unwrap();
    let d = float(doc["nutrient"].as_table().unwrap(), "diffusion");
    let mut w = world(512, 512, 4, 1.0);
    let fert = id(&w, "liquid_fertiliser");
    w.paint(256, 120, 4, fert, Brush::Replace).unwrap();
    let probe = |w: &World| w.field(FieldKind::Nutrient).at_cell(256, 200);
    let far = |w: &World| w.field(FieldKind::Nutrient).at_cell(60, 60);
    // It dissolves as it sinks, raising the field around it.
    let mut t = 0;
    while count(&w, fert) > 0 {
        w.tick();
        t += 1;
        assert!(t < 20 * 120, "fertiliser never dissolved");
    }
    assert!(probe(&w) > 0.0);
    assert!(far(&w) < 1e-3, "little has reached the far corner yet");
    let variance = |w: &World| {
        let f = w.field(FieldKind::Nutrient);
        let total = f.total();
        let (mut mx, mut my, mut m2) = (0.0, 0.0, 0.0);
        for y in 0..f.height() {
            for x in 0..f.width() {
                let v = f64::from(f.get(x, y)) / total;
                let (cx, cy) = (x as f64 * 8.0, y as f64 * 8.0);
                mx += v * cx;
                my += v * cy;
                m2 += v * (cx * cx + cy * cy);
            }
        }
        m2 - mx * mx - my * my
    };
    let v0 = variance(&w);
    let secs = 30.0;
    run(&mut w, 600);
    let measured = (variance(&w) - v0) / (4.0 * secs);
    assert!(
        (measured / d - 1.0).abs() < 0.1,
        "measured D {measured} cells²/s, configured {d}"
    );
}

#[test]
fn root_tab_feeds_the_field_and_aquasoil_buffers_it() {
    let emit = entries("emit")
        .into_iter()
        .find(|r| r["material"].as_str() == Some("root_tab"))
        .unwrap();
    let rate = float(&emit, "rate");
    let mut w = world(128, 64, 6, 1.0);
    let tab = id(&w, "root_tab");
    for x in [20, 60, 100] {
        w.set_cell(x, 63, tab).unwrap();
    }
    run(&mut w, 200);
    let total = w.field(FieldKind::Nutrient).total();
    let want = 3.0 * rate * 10.0;
    assert!(
        (total / want - 1.0).abs() < 0.05,
        "released {total}, want {want}"
    );
    assert!(
        w.field(FieldKind::Nutrient).at_cell(60, 63) > w.field(FieldKind::Nutrient).at_cell(60, 0)
    );

    let buffer = entries("buffer")
        .into_iter()
        .find(|r| r["material"].as_str() == Some("aquasoil"))
        .unwrap();
    let level = float(&buffer, "level") as f32;
    // Bare water: the soil tops the field up towards its level...
    let mut w = world(64, 64, 6, 1.0);
    let soil = id(&w, "aquasoil");
    w.fill_rect(rect(0, 56, 64, 64), soil, Brush::Replace)
        .unwrap();
    run(&mut w, 20 * 120);
    let over = w.field(FieldKind::Nutrient).at_cell(30, 60);
    assert!(
        over > 0.5 * level && over <= level + 1e-3,
        "over the soil: {over}"
    );
    // ...and soaks up a surplus.
    for y in (0..64).step_by(8) {
        for x in (0..64).step_by(8) {
            w.add_to_field(FieldKind::Nutrient, x, y, 2.0);
        }
    }
    let before = w.field(FieldKind::Nutrient).total();
    run(&mut w, 20 * 60);
    assert!(w.field(FieldKind::Nutrient).total() < before - 1.0);
}

/// Centroid column of `m`.
fn centroid_x(w: &World, m: MaterialId) -> f64 {
    let (mut sum, mut n) = (0.0, 0.0);
    for (i, c) in w.cells().iter().enumerate() {
        if c.material == m {
            sum += (i % w.width()) as f64;
            n += 1.0;
        }
    }
    sum / n
}

#[test]
fn rising_bubbles_follow_the_signed_filter_current() {
    for material in ["air_bubble", "co2_bubble"] {
        for (outlet, sign) in [(Some(5), 1.0), (Some(122), -1.0), (None, 0.0)] {
            // Isolate transport: dissolving CO₂ would also shift the centroid.
            let table = MaterialTable::builtin().unwrap();
            let ecology = Ecology::from_toml("", FIELDS, &table, false).unwrap();
            let config = WorldConfig {
                width: 128,
                height: 80,
                seed: 17,
                water_level: 1.0,
            };
            let mut w = World::with_ecology(config, table, ecology).unwrap();
            if let Some(x) = outlet {
                place_filter(&mut w, x, 40);
            }
            let bubble = id(&w, material);
            for x in (44..84).step_by(3) {
                w.set_cell(x, 47, bubble).unwrap();
            }
            let start = centroid_x(&w, bubble);
            run(&mut w, 10);
            let drift = centroid_x(&w, bubble) - start;
            if outlet.is_some() {
                assert!(drift * sign > 1.0, "{material} drifted {drift}");
            } else {
                assert!(drift.abs() < 0.01);
            }
        }
    }
}

#[test]
fn the_current_carries_floating_and_liquid_cells_downstream() {
    // A liquid that neither reacts nor sinks: water-dense dye.
    let dye = "id = 70\nname = \"dye\"\nclass = \"liquid\"\ndensity = 1.0\ncolour = \"#ff00ff\"";
    let mut sources: Vec<&str> = vec![dye];
    for src in [
        include_str!("../../../assets/materials/water.toml"),
        include_str!("../../../assets/materials/air.toml"),
        include_str!("../../../assets/materials/flake_food.toml"),
        include_str!("../../../assets/materials/device.toml"),
    ] {
        sources.push(src);
    }
    let table = MaterialTable::from_toml_sources(&sources).unwrap();
    for right in [true, false] {
        let make = |outlet: bool| {
            let config = WorldConfig {
                width: 320,
                height: 64,
                seed: 8,
                water_level: 0.9,
            };
            let eco = Ecology::from_toml("", FIELDS, &table, false).unwrap();
            let mut w = World::with_ecology(config, table.clone(), eco).unwrap();
            let x = if right { 6 } else { 313 };
            if outlet {
                place_filter(&mut w, x, 20);
            }
            let (flake, dye) = (id(&w, "flake_food"), id(&w, "dye"));
            let mid = if right { 60 } else { 260 };
            w.paint(mid, 1, 2, flake, Brush::Pour).unwrap();
            w.paint(mid, 14, 3, dye, Brush::Replace).unwrap();
            w
        };
        let (mut with, mut without) = (make(true), make(false));
        let (flake, dye) = (id(&with, "flake_food"), id(&with, "dye"));
        let sign: f64 = if right { 1.0 } else { -1.0 };
        let start = (centroid_x(&with, flake), centroid_x(&with, dye));
        // Less than the flakes' float time, so they stay at the surface.
        run(&mut with, 100);
        run(&mut without, 100);
        assert!(f64::from(with.current_at(if right { 40 } else { 280 }, 14)[0]) * sign > 1.0);
        let flake_moved = (centroid_x(&with, flake) - start.0) * sign;
        let dye_moved = (centroid_x(&with, dye) - start.1) * sign;
        let flake_drift = (centroid_x(&without, flake) - start.0).abs();
        let dye_drift = (centroid_x(&without, dye) - start.1).abs();
        assert!(
            flake_moved > 5.0 + flake_drift,
            "flakes moved {flake_moved}, still water {flake_drift}"
        );
        assert!(
            dye_moved > 5.0 + dye_drift,
            "dye moved {dye_moved}, still water {dye_drift}"
        );
        assert_eq!(
            count(&with, dye),
            count(&without, dye),
            "the current conserves liquid"
        );
    }
}

#[test]
fn an_intake_draws_loose_particles_towards_it() {
    for right in [true, false] {
        let make = |filter: bool| {
            let mut w = world(160, 80, 19, 1.0);
            let x = if right { 4 } else { 155 };
            if filter {
                // Filter low on the wall (y = 50..80, intake at y = 74).
                place_filter(&mut w, x, 65);
            }
            // Drop loose detritus in front of the lower intake slots.
            let detritus = id(&w, "detritus");
            let px = if right { 26 } else { 133 };
            w.paint(px, 68, 2, detritus, Brush::Replace).unwrap();
            w
        };
        let (mut with, mut without) = (make(true), make(false));
        let detritus = id(&with, "detritus");
        let start = centroid_x(&with, detritus);
        run(&mut with, 200);
        run(&mut without, 200);
        // Towards the filter's glass: leftwards when filter is on the left,
        // rightwards when filter is on the right.
        let towards = if right { -1.0 } else { 1.0 };
        let pulled = (centroid_x(&with, detritus) - start) * towards;
        let still = (centroid_x(&without, detritus) - start).abs();
        assert!(
            pulled > 4.0 + still,
            "intake should draw detritus towards it: pulled {pulled}, still {still}"
        );
    }
}

#[test]
fn swimmers_follow_the_2d_current_direction() {
    for right in [true, false] {
        let make = |filter: bool| {
            let mut w = world(240, 80, 31, 0.95);
            let x = if right { 4 } else { 235 };
            if filter {
                place_filter(&mut w, x, 36);
            }
            let tetra = w.species().id("neon_tetra").unwrap();
            let fx = if right { 40 } else { 200 };
            w.add_creature(tetra, fx, 30).unwrap();
            w
        };
        let (mut with, mut without) = (make(true), make(false));
        run(&mut with, 40);
        run(&mut without, 40);
        let sign = if right { 1.0 } else { -1.0 };
        let dx =
            f64::from(with.creatures()[0].centre().0 - without.creatures()[0].centre().0) * sign;
        assert!(
            dx > 2.0,
            "swimmer in jet should drift downstream relative to still water: dx={dx}"
        );
    }
}

#[test]
fn settled_tank_with_bed_and_filter_scans_zero_current_blocks() {
    let mut w = world(160, 80, 41, 0.9);
    let sand = id(&w, "sand");
    w.fill_rect(
        desktank_sim::Rect {
            x0: 0,
            y0: 68,
            x1: 160,
            y1: 80,
        },
        sand,
        Brush::Replace,
    )
    .unwrap();
    place_filter(&mut w, 4, 40);
    run(&mut w, 200);
    assert_eq!(w.awake_chunks(), 0, "tank settled");
    let scans = w.current_block_scans();
    run(&mut w, 200);
    assert_eq!(
        w.current_block_scans(),
        scans,
        "settled tank with sand bed and filter performs zero current block scans"
    );
}

#[test]
fn the_bubbler_releases_bubbles_and_keeps_the_water_level() {
    let mut w = world(128, 96, 11, 0.9);
    let (water, bubble) = (id(&w, "water"), id(&w, "air_bubble"));
    w.place(Placeable::Bubbler, 64, 40).unwrap();
    let water_before = count(&w, water);
    let mut seen = 0;
    for _ in 0..20 * 30 {
        w.tick();
        seen = seen.max(count(&w, bubble));
        assert_eq!(count(&w, water), water_before);
    }
    assert!(seen >= 5, "at most {seen} bubbles at once");
}

#[test]
fn random_ticks_stay_within_their_budget() {
    let mut w = world(320, 128, 12, 0.9);
    let soil = id(&w, "aquasoil");
    w.fill_rect(rect(0, 120, 320, 128), soil, Brush::Replace)
        .unwrap();
    for budget in [256, 100, 4000] {
        w.set_random_tick_budget(budget);
        for _ in 0..20 {
            w.tick();
            let s = w.random_tick_stats();
            assert_eq!(s.living, 320 * 8);
            assert_eq!(s.visited, budget.min(s.living));
        }
    }
}

#[test]
fn living_cells_dont_keep_a_settled_tank_awake() {
    let mut w = world(192, 128, 13, 0.9);
    let (soil, tab) = (id(&w, "aquasoil"), id(&w, "root_tab"));
    w.fill_rect(rect(0, 118, 192, 128), soil, Brush::Replace)
        .unwrap();
    w.set_cell(50, 117, tab).unwrap();
    for _ in 0..200 {
        w.tick();
        if w.awake_chunks() == 0 {
            break;
        }
    }
    assert_eq!(w.awake_chunks(), 0);
    let before = w.field(FieldKind::Nutrient).total();
    for _ in 0..400 {
        w.tick();
        assert_eq!(w.awake_chunks(), 0, "a random tick woke a chunk");
        assert!(w.random_tick_stats().visited > 0);
    }
    assert!(
        w.field(FieldKind::Nutrient).total() > before,
        "the soil and tab still feed the field"
    );
}

#[test]
fn light_falls_with_depth_and_under_cover() {
    let mut w = world(128, 128, 14, 0.9);
    let rock = id(&w, "rock");
    w.fill_rect(rect(0, 40, 48, 48), rock, Brush::Replace)
        .unwrap();
    run(&mut w, 4);
    let light = w.field(FieldKind::Light);
    assert!(light.at_cell(100, 120) < light.at_cell(100, 20));
    assert!(
        light.at_cell(20, 100) < 0.5 * light.at_cell(100, 100),
        "the rock shades below it"
    );
}

#[test]
fn living_tanks_are_deterministic() {
    let scripted = |seed| {
        let mut w = world(192, 128, seed, 0.9);
        let names = [
            "liquid_fertiliser",
            "co2_bubble",
            "waste",
            "detritus",
            "aquasoil",
            "root_tab",
        ];
        let ids: Vec<_> = names.iter().map(|n| id(&w, n)).collect();
        w.place(Placeable::Bubbler, 100, 60).unwrap();
        w.place(Placeable::InternalFilter, 4, 70).unwrap();
        for t in 0..3000u64 {
            if t % 50 == 0 && t < 2000 {
                let k = usize::try_from(t / 50).unwrap();
                let x = i64::try_from((k * 41) % 192).unwrap();
                w.paint(x, 20, 2, ids[k % ids.len()], Brush::Pour).unwrap();
            }
            w.tick();
        }
        (
            w.state_hash(),
            w.field(FieldKind::Nutrient).values().to_vec(),
        )
    };
    let a = scripted(21);
    assert_eq!(a, scripted(21));
    assert_ne!(a.0, scripted(22).0);
}

#[test]
fn driftwood_floats_up_and_rests_at_the_surface() {
    let mut w = world(96, 96, 15, 0.9);
    let wood = id(&w, "driftwood");
    w.paint(48, 85, 3, wood, Brush::Pour).unwrap();
    let n = count(&w, wood);
    assert_eq!(w.bodies().count(), 1, "one stroke is one body");
    for _ in 0..400 {
        w.tick();
        if w.awake_chunks() == 0 {
            break;
        }
    }
    assert_eq!(w.awake_chunks(), 0, "the wood came to rest");
    assert_eq!(count(&w, wood), n, "the body kept every cell");
    let rows: Vec<usize> = w
        .cells()
        .iter()
        .enumerate()
        .filter(|(_, c)| c.material == wood)
        .map(|(i, _)| i / w.width())
        .collect();
    let (top, bottom) = (*rows.iter().min().unwrap(), *rows.iter().max().unwrap());
    let surface = w.surface_row();
    // It rises through the water and stops under the air.
    assert!(
        top < surface && surface <= bottom,
        "wood rows {top}..={bottom}, surface {surface}"
    );
    assert!(bottom > top);
}
