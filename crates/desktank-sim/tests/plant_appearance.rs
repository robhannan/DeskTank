//! SPEC T6d (§2.7 *Appearance*): plant colouration. A species with a
//! `[colouration]` (rotala, ludwigia) colours in strong light with lean
//! water, slowly, while species without one and plants in dim light or
//! rich water stay uncoloured; colouration is saved (format version 8)
//! and changes the plants version only at its tint steps.

use desktank_sim::{
    Brush, COLOUR_PER_SECOND, MaterialTable, PlantInfo, Propagule, Rect, TICKS_PER_SECOND, World,
    WorldConfig, save,
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
