use desktank_sim::tools::{Button, Outcome, PaletteItem, Tool, Tools};
use desktank_sim::{Brush, DEPTH_BANDS, MaterialTable, Placeable, World, WorldConfig, save};

fn tank() -> World {
    World::new(
        WorldConfig {
            width: 128,
            height: 100,
            water_level: 1.0,
            seed: 7,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap()
}

const ORNAMENTS: [Placeable; 3] = [
    Placeable::VolcanoBubbler,
    Placeable::StoneCastle,
    Placeable::PastelCastle,
];

#[test]
fn preview_and_placement_match_and_a_single_cell_scoop_removes_everything() {
    for ornament in ORNAMENTS {
        preview_and_scoop(ornament);
    }
}

fn preview_and_scoop(ornament: Placeable) {
    let mut world = tank();
    let material = world.materials().id(ornament.name()).unwrap();
    let rock = world.materials().id("rock").unwrap();
    let mut tools = Tools::new(rock);
    tools.select(PaletteItem::Tool(Tool::Place(ornament)));
    tools.moved(&mut world, (50, 40));
    let ghost = tools.ornament_ghost(&world).unwrap();
    assert!(ghost.fits);
    let art = world
        .materials()
        .get(material)
        .unwrap()
        .ornament
        .clone()
        .unwrap();
    let pixels: Vec<_> = art
        .pixels()
        .map(|(x, y, v)| (ghost.origin.0 + x, ghost.origin.1 + y, v))
        .collect();
    let (x, y, _) = pixels[0];
    world.set_cell(x, y, rock).unwrap();
    let blocked = world.refresh_ornament_ghost(ghost);
    assert!(!blocked.fits);
    let before = world.state_hash();
    assert_eq!(world.place(ornament, 50, 40).unwrap(), 0);
    assert_eq!(world.state_hash(), before);
    let water = world.materials().id("water").unwrap();
    world.set_cell(x, y, water).unwrap();
    assert_eq!(world.place(ornament, 50, 40).unwrap(), pixels.len());
    let body = world.body_at(x, y).unwrap();
    let (front, back) = world.rigid_bands(y * world.width() + x).unwrap();
    assert!(front < back && back - front + 1 < DEPTH_BANDS);
    for &(x, y, variant) in &pixels {
        assert_eq!(world.body_at(x, y), Some(body));
        assert_eq!(world.cell(x, y).unwrap().variant, variant);
    }
    let (x, y) = (i64::try_from(x).unwrap(), i64::try_from(y).unwrap());
    assert_eq!(
        tools.press(&mut world, Button::Secondary, (x, y)),
        Outcome::Painted(pixels.len())
    );
    assert!(world.body(body).is_none());
    assert_eq!(world.material_counts()[usize::from(material.0)], 0);
}

#[test]
fn ornaments_topple_off_a_rock_onto_sand_and_replay_after_load() {
    for ornament in ORNAMENTS {
        topple_and_reload(ornament);
    }
}

fn topple_and_reload(ornament: Placeable) {
    let mut world = tank();
    let rock = world.materials().id("rock").unwrap();
    let sand = world.materials().id("sand").unwrap();
    for y in 88..world.height() {
        for x in 0..world.width() {
            world.set_cell(x, y, sand).unwrap();
        }
    }
    let (_, pillar) = world
        .paint_into(50, 64, 3, rock, Brush::Pour, None)
        .unwrap();
    world.set_anchored(pillar.unwrap(), true);
    let count = world.place(ornament, 61, 39).unwrap();
    assert!(count > 0);
    let material = world.materials().id(ornament.name()).unwrap();
    let id = world.bodies().find(|b| b.material == material).unwrap().id;
    let mut height_at_turn = None;
    for _ in 0..180 {
        world.tick();
        let body = world.body(id).unwrap();
        if body.angle != 0 {
            height_at_turn = Some(body.bounds.y1);
            break;
        }
    }
    assert!(
        height_at_turn.is_some(),
        "an off-centre rock tips the {ornament:?}"
    );
    let mut restored =
        save::decode(&save::encode(&world), MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(world.ornament_poses().count(), 1);
    assert!(restored.ornament_poses().eq(world.ornament_poses()));
    for _ in 0..260 {
        world.tick();
        restored.tick();
    }
    let settled = world.body(id).unwrap();
    assert_eq!(settled.cells, count);
    assert!(settled.bounds.y1 >= 88, "settled at the sand: {settled:?}");
    let air_bubble = world.materials().id("air_bubble").unwrap();
    assert_eq!(
        world.material_counts()[usize::from(air_bubble.0)] > 0,
        ornament == Placeable::VolcanoBubbler,
        "only the volcano emits bubbles"
    );
    assert_eq!(restored.state_hash(), world.state_hash());
}

#[test]
fn castle_variants_share_the_traced_art_with_different_colours() {
    let materials = MaterialTable::builtin().unwrap();
    let art = |ornament: Placeable| {
        let material = materials.id(ornament.name()).unwrap();
        materials.get(material).unwrap().ornament.as_ref().unwrap()
    };
    let stone = art(Placeable::StoneCastle);
    let pastel = art(Placeable::PastelCastle);
    assert_eq!(stone.size(), pastel.size());
    assert!(stone.pixels().eq(pastel.pixels()));
    assert!(stone.pixels().all(|(_, _, variant)| {
        stone.colour(variant).unwrap() != pastel.colour(variant).unwrap()
    }));

    for (gate_x, gate_y) in [(17, 22), (30, 24)] {
        assert!(!stone.pixels().any(|(x, y, _)| (x, y) == (gate_x, gate_y)));
    }
}
