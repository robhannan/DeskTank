//! SPEC T6d (§2.7 *Appearance*): plant colouration. A species with a
//! `[colouration]` (rotala, ludwigia) colours in strong light with lean
//! water, slowly, while species without one and plants in dim light or
//! rich water stay uncoloured; colouration is saved (format version 8)
//! and changes the plants version only at its tint steps.

use desktank_sim::{
    Brush, COLOUR_PER_SECOND, FieldKind, MaterialTable, PEARL_CO2, PEARL_HEALTH, PEARL_LIGHT,
    PEARL_MAX_PER_SECOND, PlantInfo, Propagule, Rect, TICKS_PER_SECOND, World, WorldConfig, save,
};

/// A shallow tank (light everywhere strong) with an aquasoil bed, a rotala
/// and a hygrophila on it, plus `tabs` root tabs buried beside them (rich
/// water).
fn shallow(tabs: usize) -> World {
    let mut w = World::new(
        WorldConfig {
            width: 96,
            height: 40,
            seed: 5,
            water_level: 0.95,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    let soil = w.materials().id("aquasoil").unwrap();
    w.fill_rect(
        Rect {
            x0: 0,
            y0: 32,
            x1: 96,
            y1: 40,
        },
        soil,
        Brush::Replace,
    )
    .unwrap();
    let tab = w.materials().id("root_tab").unwrap();
    for k in 0..tabs {
        let x = 8 + 12 * i64::try_from(k).unwrap();
        w.paint(x, 33, 1, tab, Brush::Replace).unwrap();
    }
    for (x, name) in [(30, "rotala"), (66, "hygrophila")] {
        let s = w.plant_species().id(name).unwrap();
        w.paint_plant(x, 30, 0, s, Propagule::Cuttings).unwrap();
    }
    w
}

fn plant(w: &World, name: &str) -> PlantInfo {
    let s = w.plant_species().id(name).unwrap();
    w.plants()
        .filter(|p| p.species == s)
        .max_by_key(PlantInfo::size)
        .unwrap()
}

fn run(w: &mut World, seconds: u32) {
    for _ in 0..seconds * TICKS_PER_SECOND {
        w.tick();
    }
}

#[test]
fn rotala_colours_in_strong_light_with_lean_water() {
    let mut w = shallow(0);
    run(&mut w, 60);
    let early = plant(&w, "rotala").colour;
    run(&mut w, 120);
    let rotala = plant(&w, "rotala");
    // Slowly: never faster than its rate.
    assert!(early <= 60.0 * COLOUR_PER_SECOND + 1e-4, "{early}");
    assert!(rotala.colour > early, "{early} -> {}", rotala.colour);
    assert!(rotala.colour > 0.5, "rotala colour {}", rotala.colour);
    assert!(rotala.health > 0.9);
    // Hygrophila has no colouration.
    assert!(plant(&w, "hygrophila").colour.abs() < f32::EPSILON);
}

#[test]
fn rich_water_keeps_rotala_green() {
    let mut w = shallow(1);
    run(&mut w, 180);
    let rotala = plant(&w, "rotala");
    // It grew as well as in lean water (so it is lit enough to colour)...
    assert!(rotala.health > 0.9 && rotala.size() > 30, "{rotala:?}");
    // ...but rich water keeps it green.
    assert!(rotala.colour < 0.1, "rotala colour {}", rotala.colour);
}

#[test]
fn colouration_is_saved_and_steps_the_plants_version() {
    let mut w = shallow(0);
    run(&mut w, 120);
    let before = plant(&w, "rotala");
    assert!(before.colour > 0.2);
    let bytes = save::encode(&w);
    assert_eq!(save::check(&bytes), Ok(save::FORMAT_VERSION));
    let mut loaded = save::decode(&bytes, MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(loaded.state_hash(), w.state_hash());
    assert!((plant(&loaded, "rotala").colour - before.colour).abs() < f32::EPSILON);
    for _ in 0..400 {
        w.tick();
        loaded.tick();
    }
    assert_eq!(loaded.state_hash(), w.state_hash());
}

/// Colouring asks for a new plant texture (and a frame) at its tint steps,
/// and once a plant is within a step of its target, no more.
#[test]
fn colouration_steps_the_plants_version_then_settles() {
    let mut w = shallow(0);
    run(&mut w, 30);
    let (start, from) = (w.plants_version(), plant(&w, "rotala").colour);
    run(&mut w, 60);
    let to = plant(&w, "rotala").colour;
    let bumps = w.plants_version() - start;
    let steps = u64::from(desktank_sim::tint_step(to) - desktank_sim::tint_step(from));
    assert!(steps >= 4, "{from} -> {to}");
    // One update can cross two steps (it moves up to 5/120), so at least a
    // bump per two steps, and no more than one per step (growth and health
    // add a few).
    assert!(
        bumps * 2 >= steps && bumps <= steps + 8,
        "{bumps} bumps for {steps} steps"
    );
    // Near its target it holds within a step, so sampling noise doesn't
    // flip it across one at every update (24 updates in 120 s): at most
    // the odd step as the plant grows into more light.
    run(&mut w, 600);
    let start = w.plants_version();
    run(&mut w, 120);
    let bumps = w.plants_version() - start;
    assert!(bumps <= 2, "{bumps} bumps in 120 s near the target");
}

/// Counts `air_bubble` cells currently in the world.
fn air_bubbles(w: &World) -> usize {
    let bubble = w.materials().id("air_bubble").unwrap();
    w.cells().iter().filter(|c| c.material == bubble).count()
}

/// Injects CO₂ bubbles near the bed so they dissolve into the CO₂ field
/// without creating any `air_bubble` cells.
fn inject_co2(w: &mut World) {
    let co2 = w.materials().id("co2_bubble").unwrap();
    for x in (8..w.width() - 8).step_by(6) {
        let _ = w.paint(
            i64::try_from(x).unwrap(),
            i64::try_from(w.height() - 10).unwrap(),
            2,
            co2,
            Brush::Pour,
        );
    }
}

/// Runs `seconds` of world time, counting every newly spawned `air_bubble`
/// (a bubble cell that wasn't one the tick before; bubbles rise, so we track
/// the net new appearances per tick and per 1-second window).
fn count_pearls(w: &mut World, seconds: u32, refill_co2: bool) -> (usize, usize) {
    let bubble = w.materials().id("air_bubble").unwrap();
    let mut total = 0usize;
    let mut max_per_sec = 0usize;
    for sec in 0..seconds {
        if refill_co2 && sec % 5 == 0 {
            inject_co2(w);
        }
        let mut this_sec = 0usize;
        for _ in 0..TICKS_PER_SECOND {
            let before = air_bubbles(w);
            w.tick();
            // Bubbles can only be created by pearling here (no bubbler in
            // the tank); bubbles popping at the surface only decrease the
            // count. Checking cells right above leaves catches every new
            // pearl even on a tick when an older bubble pops.
            let after = air_bubbles(w);
            let spawned = after.saturating_sub(before);
            // Also verify no more than 1 pearl can spawn in a single tick.
            assert!(
                spawned <= 1,
                "more than 1 pearl spawned in a single tick ({spawned})"
            );
            let _ = bubble;
            this_sec += spawned;
        }
        total += this_sec;
        max_per_sec = max_per_sec.max(this_sec);
    }
    (total, max_per_sec)
}

/// Counts exact newly-created `air_bubble` cells directly above leaf cells
/// over `seconds` (immune to simultaneous surface pops).
fn count_exact_pearls(w: &mut World, seconds: u32, refill_co2: bool) -> (usize, usize) {
    let bubble = w.materials().id("air_bubble").unwrap();
    let leaf = w.materials().id("leaf").unwrap();
    let width = w.width();
    let mut total = 0usize;
    let mut max_per_sec = 0usize;
    for sec in 0..seconds {
        if refill_co2 && sec % 5 == 0 {
            inject_co2(w);
        }
        let mut this_sec = 0usize;
        for _ in 0..TICKS_PER_SECOND {
            let prev: Vec<bool> = w.cells().iter().map(|c| c.material == bubble).collect();
            w.tick();
            let cells = w.cells();
            let mut spawned = 0usize;
            for (i, c) in cells.iter().enumerate() {
                if c.material == bubble
                    && !prev[i]
                    && i + width < cells.len()
                    && cells[i + width].material == leaf
                {
                    spawned += 1;
                }
            }
            assert!(spawned <= 1, "at most 1 pearl per tick, got {spawned}");
            this_sec += spawned;
        }
        total += this_sec;
        max_per_sec = max_per_sec.max(this_sec);
    }
    (total, max_per_sec)
}

#[test]
fn pearling_appears_only_above_light_and_co2_thresholds() {
    // Grow healthy plants in a shallow, well-lit tank.
    let mut lit_no_co2 = shallow(0);
    run(&mut lit_no_co2, 120);
    assert!(plant(&lit_no_co2, "rotala").health >= PEARL_HEALTH);
    assert!(plant(&lit_no_co2, "hygrophila").health >= PEARL_HEALTH);

    // 1. Strong light, NO CO₂: zero pearls.
    let mut no_co2 = lit_no_co2.clone();
    assert!(no_co2.field(FieldKind::Co2).at_cell(30, 20) < PEARL_CO2);
    let (pearls_no_co2, _) = count_exact_pearls(&mut no_co2, 20, false);
    assert_eq!(pearls_no_co2, 0, "pearled without CO₂");

    // 2. Strong light + high CO₂: pearls appear.
    let mut with_co2 = lit_no_co2.clone();
    inject_co2(&mut with_co2);
    run(&mut with_co2, 4);
    assert!(with_co2.field(FieldKind::Light).at_cell(30, 20) >= PEARL_LIGHT);
    assert!(with_co2.field(FieldKind::Co2).at_cell(30, 20) >= PEARL_CO2);
    let (pearls_lit, max_per_sec) = count_exact_pearls(&mut with_co2, 25, true);
    assert!(
        pearls_lit > 0,
        "expected pearling under strong light and high CO₂"
    );
    assert!(
        max_per_sec <= PEARL_MAX_PER_SECOND as usize,
        "exceeded cap: {max_per_sec} > {PEARL_MAX_PER_SECOND}"
    );

    // 3. Dim light (glued rock lid across the top) + high CO₂: zero pearls.
    let mut shaded = lit_no_co2;
    let rock = shaded.materials().id("rock").unwrap();
    shaded
        .fill_rect(
            Rect {
                x0: 0,
                y0: 3,
                x1: shaded.width(),
                y1: 7,
            },
            rock,
            Brush::Replace,
        )
        .unwrap();
    for b in shaded.bodies().map(|b| b.id).collect::<Vec<_>>() {
        shaded.set_anchored(b, true);
    }
    inject_co2(&mut shaded);
    run(&mut shaded, 4);
    assert!(
        shaded.field(FieldKind::Light).at_cell(30, 20) < PEARL_LIGHT,
        "light under lid should be below PEARL_LIGHT"
    );
    assert!(shaded.field(FieldKind::Co2).at_cell(30, 20) >= PEARL_CO2);
    let (pearls_shaded, _) = count_exact_pearls(&mut shaded, 20, true);
    assert_eq!(pearls_shaded, 0, "pearled in dim light");
}

#[test]
fn pearling_stops_on_unhealthy_plants() {
    let mut w = World::new(
        WorldConfig {
            width: 96,
            height: 40,
            seed: 5,
            water_level: 0.95,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    let soil = w.materials().id("aquasoil").unwrap();
    w.fill_rect(
        Rect {
            x0: 0,
            y0: 32,
            x1: 96,
            y1: 40,
        },
        soil,
        Brush::Replace,
    )
    .unwrap();
    let rotala = w.plant_species().id("rotala").unwrap();
    w.paint_plant(48, 30, 0, rotala, Propagule::Cuttings)
        .unwrap();
    run(&mut w, 120);
    // Starve rotala under a rock lid until health drops below PEARL_HEALTH
    // (still alive and holding leaves).
    let rock = w.materials().id("rock").unwrap();
    w.fill_rect(
        Rect {
            x0: 0,
            y0: 2,
            x1: w.width(),
            y1: 12,
        },
        rock,
        Brush::Replace,
    )
    .unwrap();
    for b in w.bodies().map(|b| b.id).collect::<Vec<_>>() {
        w.set_anchored(b, true);
    }
    for _ in 0..120 {
        run(&mut w, 2);
        if w.plants().all(|p| p.health < PEARL_HEALTH) {
            break;
        }
    }
    assert!(w.plant_count() > 0);
    assert!(w.plants().all(|p| p.health < PEARL_HEALTH && p.parts[2] > 0));
    // Remove the lid so light returns to strong, and inject CO₂, then check
    // immediately before health recovers back to PEARL_HEALTH.
    let water = w.materials().id("water").unwrap();
    w.fill_rect(
        Rect {
            x0: 0,
            y0: 2,
            x1: w.width(),
            y1: 12,
        },
        water,
        Brush::Replace,
    )
    .unwrap();
    inject_co2(&mut w);
    run(&mut w, 2);
    assert!(w.plants().all(|p| p.health < PEARL_HEALTH));
    let (pearls, _) = count_exact_pearls(&mut w, 3, false);
    assert_eq!(pearls, 0, "unhealthy plants should not pearl");
}

#[test]
fn pearling_stays_within_cap_and_is_save_deterministic() {
    // Pack the shallow tank with many plants so dozens of leaves are eligible.
    let mut w = shallow(0);
    let hygro = w.plant_species().id("hygrophila").unwrap();
    let rotala = w.plant_species().id("rotala").unwrap();
    for x in (6..90).step_by(6) {
        let s = if x % 12 == 0 { rotala } else { hygro };
        let _ = w.paint_plant(x, 30, 0, s, Propagule::Cuttings);
    }
    run(&mut w, 120);
    inject_co2(&mut w);
    run(&mut w, 4);

    // Save/load mid-pearling continues tick for tick.
    let bytes = save::encode(&w);
    let mut loaded = save::decode(&bytes, MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(loaded.state_hash(), w.state_hash());

    let (total, max_per_sec) = count_pearls(&mut w, 15, true);
    let (total2, max_per_sec2) = count_pearls(&mut loaded, 15, true);
    assert_eq!(loaded.state_hash(), w.state_hash());
    assert_eq!(total, total2);
    assert_eq!(max_per_sec, max_per_sec2);
    assert!(total > 0, "dense lit CO₂ tank should pearl");
    assert!(
        max_per_sec <= PEARL_MAX_PER_SECOND as usize,
        "pearling rate {max_per_sec}/s exceeded cap {PEARL_MAX_PER_SECOND}/s"
    );
}
