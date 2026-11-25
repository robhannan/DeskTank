//! Surface film growth, light, plant competition and persistence.

use desktank_sim::field::FIELD_SCALE;
use desktank_sim::{
    Brush, FieldKind, MaterialTable, Propagule, Rect, TICKS_PER_SECOND, World, WorldConfig, save,
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

#[test]
fn natural_film_grows_on_substrate_rigid_faces_and_all_glass() {
    let mut w = tank(37);
    for (name, x) in [("rock", 12), ("driftwood", 36)] {
        fill(&mut w, name, (x, 26, x + 10, 42));
        w.set_anchored(w.body_at(x, 26).unwrap(), true);
    }
    let width = w.width();
    enrich(&mut w, 0.6);
    run(&mut w, 120);
    assert!(w.coat().iter().all(|&v| v == 0));
    assert!(w.front_glass_algae().iter().all(|&v| v == 0));
    enrich(&mut w, 0.4);
    run(&mut w, 300);
    for x in [12, 36] {
        assert!(
            (29..40).any(|y| w.coat()[y * width + x] != 0),
            "vertical face at x={x}"
        );
    }
    assert!(w.coat()[42 * width..43 * width].iter().any(|&v| v != 0));
    assert_eq!(w.front_glass_algae().iter().copied().max(), Some(5));
    assert_eq!(w.back_glass_algae().iter().copied().max(), Some(15));
    for side in 0..2 {
        assert!(w.side_glass_algae().iter().any(|row| row[side] > 0));
    }
    for (i, &coat) in w.coat().iter().enumerate().filter(|&(_, &v)| v != 0) {
        assert_eq!(coat >> 4, 1);
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
