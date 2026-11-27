//! Rock varieties (#457): `dragon_stone` and `seiryu_stone` are rigid
//! hardscape that behaves like `rock` everywhere: bodies, moss, algae and
//! saves.

use desktank_sim::{
    Brush, FieldKind, MaterialId, MaterialTable, Propagule, Rect, World, WorldConfig, save,
};

/// The stones, with the plain rock first.
const STONES: [&str; 3] = ["rock", "dragon_stone", "seiryu_stone"];
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

fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Rect {
    Rect { x0, y0, x1, y1 }
}

fn run(w: &mut World, ticks: u32) {
    for _ in 0..ticks {
        w.tick();
    }
}

fn positions(w: &World, material: MaterialId) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for y in 0..w.height() {
        for x in 0..w.width() {
            if w.cell(x, y).is_some_and(|c| c.material == material) {
                out.push((x, y));
            }
        }
    }
    out
}

#[test]
fn the_new_stones_are_rigid_and_ordered_by_density() {
    let w = world(32, 32, 0);
    let table = w.materials();
    let get = |name: &str| table.get(id(&w, name)).unwrap();
    for name in STONES {
        assert_eq!(get(name).class, get("rock").class, "{name}");
    }
    assert!(get("dragon_stone").density < get("rock").density);
    assert!(get("seiryu_stone").density > get("rock").density);
    // Appended above the old maximum, so old saves are unchanged.
    assert!(get("dragon_stone").id.0 > 80 && get("seiryu_stone").id.0 > 80);
}

#[test]
fn each_stone_sinks_as_one_piece_and_rests_on_sand() {
    // Rock sets the benchmark: every kind is one body that lands on the bed.
    for name in STONES {
        let mut w = world(96, 96, 1);
        let (sand, stone) = (id(&w, "sand"), id(&w, name));
        w.fill_rect(rect(0, 86, 96, 96), sand, Brush::Replace)
            .unwrap();
        w.paint(48, 30, 5, stone, Brush::Replace).unwrap();
        let shape = positions(&w, stone);
        for _ in 0..600 {
            w.tick();
            if w.awake_chunks() == 0 {
                break;
            }
        }
        let after = positions(&w, stone);
        assert_eq!(after.len(), shape.len(), "{name} lost cells");
        assert_eq!(w.bodies().count(), 1, "{name}");
        assert!(after.iter().map(|p| p.1).max().unwrap() >= 80, "{name}");
        // Glued, it stays put.
        let body = w.body_at(after[0].0, after[0].1).unwrap();
        assert!(w.set_anchored(body, true));
        run(&mut w, 40);
        assert_eq!(positions(&w, stone), after, "{name} moved when glued");
    }
}

#[test]
fn moss_spreads_onto_each_stone() {
    for name in STONES {
        let mut w = world(160, 120, 5);
        let stone = id(&w, name);
        w.fill_rect(rect(60, 70, 100, 90), stone, Brush::Replace)
            .unwrap();
        let body = w.body_at(60, 70).unwrap();
        assert!(w.set_anchored(body, true));
        let moss = w.plant_species().id("java_moss").unwrap();
        w.paint_plant(80, 60, 2, moss, Propagule::Fragments)
            .unwrap();
        run(&mut w, 3 * MINUTE);
        let on_stone = |w: &World| {
            let mut n = 0;
            for y in 1..w.height() - 1 {
                for x in 1..w.width() - 1 {
                    if w.plant_at(x, y).is_none() {
                        continue;
                    }
                    let touches = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dy)| {
                        w.cell(x.wrapping_add_signed(dx), y.wrapping_add_signed(dy))
                            .is_some_and(|c| c.material == stone)
                    });
                    assert!(touches, "{name}: moss at ({x}, {y}) isn't on the stone");
                    n += 1;
                }
            }
            n
        };
        let kept = on_stone(&w);
        assert!(kept > 0, "no moss rooted on {name}");
        for fy in 0..w.field(FieldKind::Nutrient).height() {
            for fx in 0..w.field(FieldKind::Nutrient).width() {
                w.add_to_field(FieldKind::Nutrient, fx * 8, fy * 8, 0.2);
            }
        }
        run(&mut w, 10 * MINUTE);
        let grown = on_stone(&w);
        assert!(
            grown >= kept + 20,
            "{name}: moss grew from {kept} to {grown}"
        );
    }
}

#[test]
fn algae_coats_each_stone_like_rock() {
    for name in STONES {
        let mut w = world(64, 64, 2);
        let stone = id(&w, name);
        w.fill_rect(rect(10, 40, 50, 60), stone, Brush::Replace)
            .unwrap();
        assert!(w.set_algae_coat(30, 40, 8), "{name}: top face takes film");
        assert!(w.coated_cells() > 0, "{name}");
    }
}

#[test]
fn a_save_with_the_new_stones_round_trips() {
    let mut w = world(96, 96, 3);
    let sand = id(&w, "sand");
    w.fill_rect(rect(0, 86, 96, 96), sand, Brush::Replace)
        .unwrap();
    for (name, x) in [("rock", 20), ("dragon_stone", 48), ("seiryu_stone", 76)] {
        w.paint(x, 30, 5, id(&w, name), Brush::Replace).unwrap();
    }
    run(&mut w, 300);
    let bytes = save::encode(&w);
    let loaded = save::decode(&bytes, MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(loaded.state_hash(), w.state_hash());
    assert_eq!(save::encode(&loaded), bytes);
    for name in STONES {
        let stone = id(&loaded, name);
        assert_eq!(positions(&loaded, stone), positions(&w, stone), "{name}");
        assert!(!positions(&loaded, stone).is_empty(), "{name}");
    }
}
