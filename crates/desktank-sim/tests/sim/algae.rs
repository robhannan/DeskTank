//! Surface film growth, light, plant competition and persistence.

use desktank_sim::field::FIELD_SCALE;
use desktank_sim::{
    Brush, DeviceKind, FieldKind, MaterialTable, Propagule, Rect, TICKS_PER_SECOND, World,
    WorldConfig, save,
};

fn tank(seed: u64) -> World {
    let mut w = World::new(
        WorldConfig {
            width: 64,
            height: 48,
            seed,
            water_level: 1.0,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    fill(&mut w, "sand", (0, 42, 64, 48));
    w
}

fn fill(w: &mut World, name: &str, (x0, y0, x1, y1): (usize, usize, usize, usize)) {
    let material = w.materials().id(name).unwrap();
    w.fill_rect(Rect { x0, y0, x1, y1 }, material, Brush::Replace)
        .unwrap();
}

fn enrich(w: &mut World, amount: f32) {
    for y in (0..w.height()).step_by(FIELD_SCALE) {
        for x in (0..w.width()).step_by(FIELD_SCALE) {
            w.add_to_field(FieldKind::Nutrient, x, y, amount);
        }
    }
}

fn run(w: &mut World, seconds: u32) {
    for _ in 0..seconds * TICKS_PER_SECOND {
        w.tick();
    }
}

/// Tops the CO₂ up in every block, as an injection would.
fn inject_co2(w: &mut World, amount: f32) {
    for y in (0..w.height()).step_by(FIELD_SCALE) {
        for x in (0..w.width()).step_by(FIELD_SCALE) {
            w.add_to_field(FieldKind::Co2, x, y, amount);
        }
    }
}

/// Runs `seconds`, topping the CO₂ up to about `co2` every ten seconds.
fn run_with_co2(w: &mut World, seconds: u32, co2: f32) {
    for _ in 0..seconds / 10 {
        inject_co2(w, co2 - mean(w, FieldKind::Co2));
        run(w, 10);
    }
}

#[allow(clippy::cast_precision_loss)] // A few hundred blocks.
fn mean(w: &World, kind: FieldKind) -> f32 {
    let values = w.field(kind).values();
    values.iter().sum::<f32>() / values.len() as f32
}

/// A tall tank (light falls from 1.0 at the top to about 0.45 at the floor)
/// on a sand bed with a rock and a driftwood log standing on it.
fn deep_tank(seed: u64) -> World {
    let mut w = World::new(
        WorldConfig {
            width: 64,
            height: 160,
            seed,
            water_level: 1.0,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    fill(&mut w, "sand", (0, 154, 64, 160));
    for (name, x) in [("rock", 12), ("driftwood", 40)] {
        fill(&mut w, name, (x, 100, x + 10, 154));
        w.set_anchored(w.body_at(x, 100).unwrap(), true);
    }
    w
}

/// The coat cells of `variety` among `rows` of the coat, and of all coats.
fn coated(w: &World, variety: &str, rows: std::ops::Range<usize>) -> (usize, usize) {
    let id = w.algae_variety(variety).unwrap();
    let width = w.width();
    let coat = &w.coat()[rows.start * width..rows.end * width];
    (
        coat.iter().filter(|&&v| v != 0 && v >> 4 == id).count(),
        coat.iter().filter(|&&v| v != 0).count(),
    )
}

/// Glass blocks (front, back and side planes) holding `variety` in field
/// rows `rows`, and holding anything.
fn on_glass(w: &World, variety: &str, rows: std::ops::Range<usize>) -> (usize, usize) {
    let id = w.algae_variety(variety).unwrap();
    let fw = w.width().div_ceil(FIELD_SCALE);
    let mut blocks: Vec<u8> = Vec::new();
    for fy in rows {
        blocks.extend_from_slice(&w.front_glass_algae()[fy * fw..(fy + 1) * fw]);
        blocks.extend_from_slice(&w.back_glass_algae()[fy * fw..(fy + 1) * fw]);
        blocks.extend_from_slice(&w.side_glass_algae()[fy]);
    }
    (
        blocks.iter().filter(|&&v| v != 0 && v >> 4 == id).count(),
        blocks.iter().filter(|&&v| v != 0).count(),
    )
}

#[test]
fn green_film_grows_on_substrate_rigid_faces_and_all_glass_where_nothing_special_fits() {
    let mut w = tank(37);
    for (name, x) in [("rock", 12), ("driftwood", 36)] {
        fill(&mut w, name, (x, 26, x + 10, 42));
        w.set_anchored(w.body_at(x, 26).unwrap(), true);
    }
    let width = w.width();
    enrich(&mut w, 0.6);
    run_with_co2(&mut w, 120, 1.0);
    assert!(w.coat().iter().all(|&v| v == 0));
    assert!(w.front_glass_algae().iter().all(|&v| v == 0));
    enrich(&mut w, 0.2);
    // CO₂ rules out green spot and dust algae, and the water is too lean for
    // diatoms: only the generic film is left.
    run_with_co2(&mut w, 300, 1.0);
    for x in [12, 36] {
        assert!(
            (29..40).any(|y| w.coat()[y * width + x] != 0),
            "vertical face at x={x}"
        );
    }
    assert!(w.coat()[42 * width..43 * width].iter().any(|&v| v != 0));
    assert_eq!(w.front_glass_algae().iter().copied().max(), Some(0x15));
    assert_eq!(w.back_glass_algae().iter().copied().max(), Some(0x1f));
    for side in 0..2 {
        assert!(w.side_glass_algae().iter().any(|row| row[side] > 0));
    }
    let film = w.algae_variety("green_film").unwrap();
    for (i, &coat) in w.coat().iter().enumerate().filter(|&(_, &v)| v != 0) {
        assert_eq!(coat >> 4, film);
        let (x, y) = (i % width, i / width);
        let water = w.materials().id("water").unwrap();
        assert!(
            [(0, -1), (-1, 0), (1, 0), (0, 1)]
                .into_iter()
                .any(|(dx, dy)| {
                    w.cell(x.wrapping_add_signed(dx), y.wrapping_add_signed(dy))
                        .is_some_and(|c| c.material == water)
                })
        );
    }
    assert_eq!(w.awake_chunks(), 0, "algae never wakes chunks");
    w.water_change();
    run(&mut w, 600);
    assert_eq!(w.ecosystem().algae_cells, 0);
    assert!(
        w.front_glass_algae()
            .iter()
            .chain(w.back_glass_algae())
            .all(|&v| v == 0)
    );
    assert!(w.side_glass_algae().iter().flatten().all(|&v| v == 0));
}

#[test]
fn diatoms_take_a_young_dim_tank_and_recede_as_the_filter_matures() {
    let mut w = deep_tank(5);
    enrich(&mut w, 1.3);
    // Block rows 14.. have light of 0.65 or less at the front.
    run(&mut w, 600);
    let (diatoms, all) = coated(&w, "diatoms", 100..160);
    assert!(diatoms > 40 && diatoms == all, "{diatoms} of {all} cells");
    let sand = 154 * w.width();
    let id = w.algae_variety("diatoms").unwrap();
    assert!(
        w.coat()[sand..sand + w.width()]
            .iter()
            .any(|&v| v >> 4 == id)
    );
    let (glass, glass_all) = on_glass(&w, "diatoms", 14..20);
    assert!(
        glass > 20 && glass == glass_all,
        "{glass} of {glass_all} blocks"
    );
    // Brighter water up the tank is not their light: it has spots instead.
    assert_eq!(on_glass(&w, "diatoms", 0..8).0, 0);
    // A mature filter ends them. The film that takes their place starts from
    // bare cells, so the diatoms go before it grows.
    let filter = w
        .add_device(DeviceKind::InternalFilter, 4, 80, false)
        .unwrap()
        .unwrap();
    assert!(w.set_device_maturity(filter, 1.0));
    let before = w.coated_cells_of(id);
    run(&mut w, 120);
    let early = w.coated_cells_of(id);
    assert!(early < before, "diatoms {before} -> {early}");
    run(&mut w, 900);
    assert_eq!(w.coated_cells_of(id), 0);
    assert_eq!(on_glass(&w, "diatoms", 0..20).0, 0);
    let film = w.algae_variety("green_film").unwrap();
    assert!(
        w.coated_cells_of(film) > 20,
        "green film takes the surfaces"
    );
}

#[test]
fn green_spot_forms_on_glass_and_stone_in_strong_light_and_low_co2_only() {
    let mut w = deep_tank(6);
    fill(&mut w, "rock", (28, 24, 38, 44));
    w.set_anchored(w.body_at(28, 24).unwrap(), true);
    enrich(&mut w, 1.3);
    // The brightest 8 block rows: light of 0.7 or more.
    run(&mut w, 1500);
    let (spots, all) = on_glass(&w, "green_spot", 0..3);
    assert!(spots > 10 && spots == all, "{spots} of {all} blocks");
    assert!(on_glass(&w, "green_spot", 3..8).0 > 20, "and further down");
    let (dim, _) = on_glass(&w, "green_spot", 14..20);
    assert_eq!(dim, 0, "no spots in dim light");
    assert_eq!(
        coated(&w, "green_spot", 100..154).0,
        0,
        "none on the dim stone"
    );
    assert!(
        coated(&w, "green_spot", 24..44).0 > 5,
        "spots on the lit stone"
    );
    let sand = 154 * w.width();
    assert!(
        w.coat()[sand..].iter().all(|&v| v >> 4 != 3),
        "never on the bed"
    );
    // The same water with CO₂ grows the plain film instead.
    let mut gassed = deep_tank(6);
    enrich(&mut gassed, 1.3);
    run_with_co2(&mut gassed, 1500, 1.0);
    assert_eq!(on_glass(&gassed, "green_spot", 0..20).0, 0);
    assert!(on_glass(&gassed, "green_film", 0..8).0 > 20);
}

#[test]
fn green_dust_dusts_glass_and_stone_but_never_substrate_in_lean_low_co2_water() {
    let mut w = deep_tank(7);
    enrich(&mut w, 0.8);
    run(&mut w, 900);
    let dust = w.algae_variety("green_dust").unwrap();
    // Light of 0.3 to 0.68; the blocks the stone shades have the plain film.
    let (on_pane, all) = on_glass(&w, "green_dust", 12..20);
    assert!(
        on_pane > 20 && on_pane * 10 >= all * 8,
        "{on_pane} of {all} blocks"
    );
    assert_eq!(
        on_glass(&w, "diatoms", 0..20).0,
        0,
        "water too lean for them"
    );
    let (on_stone, _) = coated(&w, "green_dust", 100..154);
    assert!(on_stone > 10, "{on_stone} stone cells");
    assert!(
        w.coat()[154 * w.width()..].iter().all(|&v| v >> 4 != dust),
        "never on the bed"
    );
    // Richer water has no dust: diatoms take the dim part and spots the
    // bright part.
    let mut rich = deep_tank(7);
    enrich(&mut rich, 1.3);
    run(&mut rich, 900);
    assert_eq!(on_glass(&rich, "green_dust", 0..20).0, 0);
    assert_eq!(coated(&rich, "green_dust", 0..160).0, 0);
}

#[test]
fn a_coated_cell_keeps_its_variety_until_it_has_died_back() {
    let mut w = deep_tank(8);
    let (dust, diatoms) = (
        w.algae_variety("green_dust").unwrap(),
        w.algae_variety("diatoms").unwrap(),
    );
    let at = (12, 130);
    let i = at.1 * w.width() + at.0;
    assert!(w.set_algae_variety(at.0, at.1, dust, 12));
    // Rich water suits diatoms, not dust: the dust recedes but is not
    // overwritten while any is left.
    enrich(&mut w, 1.3);
    let mut seen = Vec::new();
    for _ in 0..40 {
        run(&mut w, 10);
        seen.push(w.coat()[i]);
    }
    let first_bare = seen.iter().position(|&v| v == 0).expect("it dies back");
    assert!(first_bare > 5, "recedes over time, not at once");
    assert!(seen[..first_bare].iter().all(|&v| v >> 4 == dust));
    assert!(
        seen[..first_bare]
            .windows(2)
            .all(|p| p[1] & 15 <= p[0] & 15)
    );
    // Diatoms may then settle where it was.
    assert!(
        seen[first_bare..]
            .iter()
            .all(|&v| v == 0 || v >> 4 == diatoms)
    );
}

#[test]
fn varieties_are_deterministic_and_round_trip_with_their_glass() {
    let mut original = deep_tank(21);
    enrich(&mut original, 1.1);
    let mut twin = original.clone();
    run(&mut original, 700);
    run(&mut twin, 700);
    assert_eq!(original.state_hash(), twin.state_hash());
    let kinds: std::collections::BTreeSet<u8> = original
        .coat()
        .iter()
        .chain(original.front_glass_algae())
        .chain(original.back_glass_algae())
        .filter(|&&v| v != 0)
        .map(|&v| v >> 4)
        .collect();
    assert!(kinds.len() >= 2, "a mix of varieties: {kinds:?}");
    let mut restored =
        save::decode(&save::encode(&original), MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(restored.coat(), original.coat());
    assert_eq!(restored.front_glass_algae(), original.front_glass_algae());
    assert_eq!(restored.back_glass_algae(), original.back_glass_algae());
    assert_eq!(restored.side_glass_algae(), original.side_glass_algae());
    for id in 1..=4 {
        assert_eq!(restored.coated_cells_of(id), original.coated_cells_of(id));
    }
    original.catch_up(300);
    restored.catch_up(300);
    assert_eq!(original.state_hash(), restored.state_hash());
}

#[test]
fn enough_healthy_plants_exclude_film_and_clear_existing_film() {
    let mut planted = tank(9);
    fill(&mut planted, "aquasoil", (0, 42, 64, 48));
    let rotala = planted.plant_species().id("rotala").unwrap();
    for x in [5, 13, 21, 29, 37, 45] {
        planted
            .paint_plant(x, 41, 0, rotala, Propagule::Cuttings)
            .unwrap();
    }
    enrich(&mut planted, 0.4);
    run(&mut planted, 600);
    assert!(planted.plant_cells() > 80);
    let x = 62;
    assert!(planted.set_algae_coat(x, 42, 15));
    enrich(&mut planted, 0.6);
    let mut bare = tank(9);
    enrich(&mut bare, 1.0);
    run(&mut planted, 300);
    run(&mut bare, 300);
    assert!(planted.ecosystem().mean_nutrient > 0.6, "water still rich");
    assert!(bare.ecosystem().algae_cells > 30);
    assert_eq!(planted.ecosystem().algae_cells, 0);
    assert!(planted.front_glass_algae().iter().all(|&v| v == 0));
}

#[test]
fn film_is_deterministic_and_round_trips_during_growth_and_catch_up() {
    let mut original = tank(23);
    enrich(&mut original, 1.0);
    let mut twin = original.clone();
    run(&mut original, 26);
    run(&mut twin, 26);
    assert_eq!(original.state_hash(), twin.state_hash());
    assert!(original.coat().iter().any(|&v| v != 0));
    let mut restored =
        save::decode(&save::encode(&original), MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(restored.coat(), original.coat());
    assert_eq!(restored.front_glass_algae(), original.front_glass_algae());
    assert_eq!(restored.back_glass_algae(), original.back_glass_algae());
    assert_eq!(restored.side_glass_algae(), original.side_glass_algae());
    for _ in 0..100 {
        original.tick();
        restored.tick();
        assert_eq!(original.state_hash(), restored.state_hash());
    }
    original.catch_up(300);
    restored.catch_up(300);
    assert_eq!(original.state_hash(), restored.state_hash());
    assert!(original.ecosystem().algae_coverage > 0.9);
}

#[test]
fn coat_is_cleared_on_overwrite_and_reduces_light_below_it() {
    let mut w = tank(12);
    fill(&mut w, "rock", (16, 16, 24, 17));
    w.set_anchored(w.body_at(16, 16).unwrap(), true);
    run(&mut w, 1);
    let before = w.field(FieldKind::Light).at_cell(16, 25);
    for x in 16..24 {
        assert!(w.set_algae_coat(x, 16, 15));
    }
    run(&mut w, 1);
    assert!(w.field(FieldKind::Light).at_cell(16, 25) < before);
    fill(&mut w, "water", (16, 16, 24, 17));
    assert_eq!(w.ecosystem().algae_cells, 0);
    let retired = w.materials().id("algae").unwrap();
    assert!(w.set_cell(16, 16, retired).is_err());
}

fn film_total(w: &World) -> usize {
    let sum = |layer: &[u8]| layer.iter().map(|&v| usize::from(v & 15)).sum::<usize>();
    sum(w.coat())
        + sum(w.front_glass_algae())
        + sum(w.back_glass_algae())
        + w.side_glass_algae()
            .iter()
            .flatten()
            .map(|&v| usize::from(v & 15))
            .sum::<usize>()
}

#[test]
fn catch_up_grows_film_at_the_live_rate_on_a_large_tank() {
    // 160 × 60 field blocks: more than one catch-up batch.
    let mut live = World::new(
        WorldConfig {
            width: 1280,
            height: 480,
            seed: 5,
            water_level: 1.0,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    fill(&mut live, "sand", (0, 440, 1280, 480));
    enrich(&mut live, 1.0);
    let mut caught_up = live.clone();
    // Long enough for every group of blocks to be visited several times.
    let seconds = 900;
    run(&mut live, seconds);
    caught_up.catch_up(seconds);
    let (live, caught_up) = (film_total(&live), film_total(&caught_up));
    assert!(live > 10_000, "live film {live}");
    assert!(
        caught_up.abs_diff(live) * 4 < live,
        "catch-up {caught_up} against live {live}"
    );
}

#[test]
fn grazer_diets_name_real_varieties_and_follow_the_guide() {
    let w = tank(1);
    for s in w.species().iter().filter(|s| !s.grazing.is_empty()) {
        assert!(s.grazes_algae, "{}", s.name);
        for (name, _) in &s.grazing {
            assert!(w.algae_variety(name).is_some(), "{} grazes {name}", s.name);
        }
    }
    let percent = |species: &str, variety: &str| {
        let s = w.species().iter().find(|s| s.name == species).unwrap();
        s.grazing
            .iter()
            .find(|(n, _)| n == variety)
            .map(|&(_, p)| p)
    };
    // Otocinclus eat diatoms best of all; nothing grazes the spots well, and
    // only a nerite takes some.
    assert_eq!(percent("otocinclus", "diatoms"), Some(100));
    assert!(percent("otocinclus", "green_film") < percent("otocinclus", "diatoms"));
    assert_eq!(percent("otocinclus", "green_spot"), Some(0));
    assert_eq!(percent("amano_shrimp", "green_spot"), Some(0));
    assert!((1..50).contains(&percent("nerite", "green_spot").unwrap()));
    let best_on_spots = w
        .species()
        .iter()
        .filter_map(|s| s.grazing.iter().find(|(n, _)| n == "green_spot"))
        .map(|&(_, p)| p)
        .max();
    assert_eq!(best_on_spots, percent("nerite", "green_spot"));
}
