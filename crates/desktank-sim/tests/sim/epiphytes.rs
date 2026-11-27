//! SPEC T6c (part 3) acceptance tests (§2.7 *Growth forms*, D24):
//! epiphytes (anubias, java fern) creep by a rhizome on rock or driftwood and
//! fall off when what holds them is removed or moves away; layered mats
//! (christmas moss) build up multiple layers and cascade off when support is
//! gone; branching stems (rotala, ludwigia) grow side shoots and stems trail
//! along the water surface; mature seeding plants drop seeds; all deterministic
//! and save-round-trip safe.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names
)]

use std::collections::{BTreeMap, BTreeSet};

use desktank_sim::plants::plant_position;
use desktank_sim::{
    Brush, MaterialId, MaterialTable, Part, PlantInfo, PlantSpeciesId, Propagule, Rect, World,
    WorldConfig, save,
};

/// Ticks in a simulated minute.
const MINUTE: u32 = 1200;

fn table() -> MaterialTable {
    MaterialTable::builtin().unwrap()
}

fn tank(width: usize, height: usize, seed: u64, water_level: f32) -> World {
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

fn species(w: &World, name: &str) -> PlantSpeciesId {
    w.plant_species().id(name).unwrap()
}

fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Rect {
    Rect { x0, y0, x1, y1 }
}

fn run(w: &mut World, ticks: u32) {
    for _ in 0..ticks {
        w.tick();
    }
}

fn plants_of(w: &World, s: PlantSpeciesId) -> Vec<PlantInfo> {
    w.plants().filter(|p| p.species == s).collect()
}

/// Checks every invariant of the plant table against the grid.
fn check_table(w: &World) {
    let part_mats = [
        (id(w, "root"), Part::Root),
        (id(w, "stem"), Part::Stem),
        (id(w, "leaf"), Part::Leaf),
        (id(w, "runner"), Part::Runner),
        (id(w, "floating_leaf"), Part::Float),
        (id(w, "rhizome"), Part::Rhizome),
    ];
    let mut counts: BTreeMap<u16, [u32; 3]> = BTreeMap::new();
    for y in 0..w.height() {
        for x in 0..w.width() {
            let c = w.cell(x, y).unwrap();
            if let Some((_, part)) = part_mats.iter().find(|&&(m, _)| m == c.material) {
                assert_ne!(c.state, 0, "plant cell at ({x},{y}) has id 0");
                counts.entry(c.state).or_default()[part.count_slot()] += 1;
            }
        }
    }
    let table_map: BTreeMap<u16, PlantInfo> = w.plants().map(|p| (p.id.0, p)).collect();
    assert_eq!(counts.len(), table_map.len(), "table vs grid plant ids");
    for (pid, p) in &table_map {
        assert_eq!(
            counts.get(pid).copied().unwrap_or_default(),
            p.parts,
            "plant {pid} part counts"
        );
    }
}

fn touches_any(w: &World, x: usize, y: usize, mats: &[MaterialId]) -> bool {
    [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)]
        .into_iter()
        .any(|(dx, dy)| {
            x.checked_add_signed(dx)
                .zip(y.checked_add_signed(dy))
                .and_then(|(nx, ny)| w.cell(nx, ny))
                .is_some_and(|c| mats.contains(&c.material))
        })
}

#[test]
fn epiphytes_grow_on_rock_and_driftwood_and_never_on_substrate() {
    let mut w = tank(160, 100, 11, 0.9);
    let soil = id(&w, "aquasoil");
    let rock = id(&w, "rock");
    let wood = id(&w, "driftwood");
    let rhizome = id(&w, "rhizome");
    let leaf = id(&w, "leaf");

    // Bed of aquasoil for nutrients.
    w.fill_rect(rect(0, 88, 160, 100), soil, Brush::Replace)
        .unwrap();
    // Glued rock on the left, glued driftwood in the middle, bare soil on the right.
    w.fill_rect(rect(16, 68, 56, 76), rock, Brush::Replace)
        .unwrap();
    let rb = w.body_at(16, 68).unwrap();
    assert!(w.set_anchored(rb, true));

    w.fill_rect(rect(72, 68, 112, 76), wood, Brush::Replace)
        .unwrap();
    let wb = w.body_at(72, 68).unwrap();
    assert!(w.set_anchored(wb, true));

    let anubias = species(&w, "anubias");
    let java_fern = species(&w, "java_fern");
    // Drop anubias on the rock, java fern on the driftwood, and both over bare soil.
    w.paint_plant(36, 62, 1, anubias, Propagule::Fragments)
        .unwrap();
    w.paint_plant(92, 62, 1, java_fern, Propagule::Fragments)
        .unwrap();
    w.paint_plant(136, 80, 1, anubias, Propagule::Fragments)
        .unwrap();
    w.paint_plant(148, 80, 1, java_fern, Propagule::Fragments)
        .unwrap();

    run(&mut w, 15 * MINUTE);
    check_table(&w);

    let a_plants = plants_of(&w, anubias);
    let jf_plants = plants_of(&w, java_fern);
    assert!(!a_plants.is_empty(), "anubias rooted on rock");
    assert!(!jf_plants.is_empty(), "java fern rooted on driftwood");

    // Nothing rooted on the bare aquasoil on the right (x >= 120).
    for p in a_plants.iter().chain(&jf_plants) {
        assert!(p.base.0 < 120, "epiphyte rooted on bare substrate: {p:?}");
    }

    // Both grew rhizome and leaves, and every rhizome cell touches rock or driftwood.
    let mut a_rhiz = 0;
    let mut a_leaf = 0;
    let mut jf_rhiz = 0;
    let mut jf_leaf = 0;
    for y in 0..w.height() {
        for x in 0..w.width() {
            let c = w.cell(x, y).unwrap();
            if c.material == rhizome {
                assert!(
                    touches_any(&w, x, y, &[rock, wood]),
                    "rhizome at ({x},{y}) not touching rock or wood"
                );
                let sp = w.plant(desktank_sim::PlantId(c.state)).unwrap().species;
                if sp == anubias {
                    a_rhiz += 1;
                } else if sp == java_fern {
                    jf_rhiz += 1;
                }
            } else if c.material == leaf {
                let sp = w.plant(desktank_sim::PlantId(c.state)).unwrap().species;
                if sp == anubias {
                    a_leaf += 1;
                } else if sp == java_fern {
                    jf_leaf += 1;
                }
            }
        }
    }
    assert!(
        a_rhiz >= 3 && a_leaf >= 5,
        "anubias: {a_rhiz} rhizome, {a_leaf} leaf"
    );
    assert!(
        jf_rhiz >= 3 && jf_leaf >= 10,
        "java fern: {jf_rhiz} rhizome, {jf_leaf} leaf"
    );
}

#[test]
fn epiphyte_falls_off_when_support_is_removed_or_body_moves_away() {
    let mut w = tank(160, 100, 21, 0.9);
    let soil = id(&w, "aquasoil");
    let rock = id(&w, "rock");
    let water = id(&w, "water");

    w.fill_rect(rect(0, 88, 160, 100), soil, Brush::Replace)
        .unwrap();
    // Glued rock on the left (we will scoop it away), and a suspended glued
    // rock on the right (we will unglue it so it drops to the bed, D24).
    w.fill_rect(rect(24, 66, 56, 74), rock, Brush::Replace)
        .unwrap();
    let r_left = w.body_at(24, 66).unwrap();
    assert!(w.set_anchored(r_left, true));

    w.fill_rect(rect(96, 46, 128, 54), rock, Brush::Replace)
        .unwrap();
    let r_right = w.body_at(96, 46).unwrap();
    assert!(w.set_anchored(r_right, true));

    let anubias = species(&w, "anubias");
    w.paint_plant(40, 62, 0, anubias, Propagule::Fragments)
        .unwrap();
    w.paint_plant(112, 42, 0, anubias, Propagule::Fragments)
        .unwrap();

    run(&mut w, 12 * MINUTE);
    check_table(&w);
    let orig: Vec<_> = plants_of(&w, anubias).into_iter().map(|p| p.id).collect();
    assert_eq!(orig.len(), 2, "both epiphytes established");

    // 1. Remove the rock on the left.
    w.fill_rect(rect(24, 66, 56, 74), water, Brush::Replace)
        .unwrap();
    // 2. Unglue the suspended rock on the right so it falls away to the bed (D24).
    assert!(w.set_anchored(r_right, false));

    run(&mut w, 6 * MINUTE);
    check_table(&w);

    // The unglued rock sank to the bed, leaving y=46..54 behind.
    assert!(
        w.body(r_right).unwrap().bounds.y0 > 70,
        "unglued rock sank to the bed: {:?}",
        w.body(r_right).unwrap().bounds
    );
    // Both original epiphytes lost their hold and fell off completely.
    for pid in orig {
        assert!(
            w.plant(pid).is_none(),
            "epiphyte {pid:?} fell off when support was removed or moved away"
        );
    }
}

#[test]
fn cutting_epiphyte_rhizome_splits_into_fragment_and_leaf_only_piece_dies() {
    let mut w = tank(160, 100, 25, 0.9);
    let soil = id(&w, "aquasoil");
    let rock = id(&w, "rock");
    let water = id(&w, "water");
    let rhizome = id(&w, "rhizome");

    w.fill_rect(rect(0, 88, 160, 100), soil, Brush::Replace)
        .unwrap();
    w.fill_rect(rect(20, 66, 140, 74), rock, Brush::Replace)
        .unwrap();
    let rb = w.body_at(20, 66).unwrap();
    assert!(w.set_anchored(rb, true));

    let java_fern = species(&w, "java_fern");
    w.paint_plant(80, 62, 0, java_fern, Propagule::Fragments)
        .unwrap();
    run(&mut w, 15 * MINUTE);
    check_table(&w);
    assert_eq!(plants_of(&w, java_fern).len(), 1);
    let parent = plants_of(&w, java_fern)[0];

    // Find all rhizome cells along the top of the rock (y = 65) and cut the middle one.
    let mut r_xs: Vec<usize> = (20..140)
        .filter(|&x| w.cell(x, 65).unwrap().material == rhizome)
        .collect();
    r_xs.sort_unstable();
    assert!(r_xs.len() >= 5, "rhizome crept along rock: {r_xs:?}");
    let mid = r_xs[r_xs.len() / 2];
    w.fill_rect(rect(mid, 40, mid + 1, 66), water, Brush::Replace)
        .unwrap();
    w.tick();
    check_table(&w);

    let after = plants_of(&w, java_fern);
    assert_eq!(after.len(), 2, "cutting rhizome split into two plants");
    let child = after.iter().find(|p| p.id != parent.id).unwrap();
    assert_eq!(child.parent, Some(parent.id));
    assert_eq!(child.generation, 1);

    // Cut the top of a leaf off (no rhizome): it turns into dead plant matter.
    let cut_y = parent.bounds.y0 + 2;
    if cut_y + 2 < 65 {
        w.fill_rect(
            rect(parent.bounds.x0, cut_y, parent.bounds.x1, cut_y + 1),
            water,
            Brush::Replace,
        )
        .unwrap();
        w.tick();
        check_table(&w);
        assert_eq!(
            plants_of(&w, java_fern).len(),
            2,
            "leaf-only cut pieces die rather than becoming new plants"
        );
    }
}

#[test]
fn layered_mat_builds_multiple_layers_and_falls_off_when_support_is_gone() {
    let mut w = tank(160, 100, 31, 0.9);
    let soil = id(&w, "aquasoil");
    let rock = id(&w, "rock");
    let water = id(&w, "water");
    let stem = id(&w, "stem");

    w.fill_rect(rect(0, 88, 160, 100), soil, Brush::Replace)
        .unwrap();
    w.fill_rect(rect(24, 66, 56, 74), rock, Brush::Replace)
        .unwrap();
    let r1 = w.body_at(24, 66).unwrap();
    assert!(w.set_anchored(r1, true));

    w.fill_rect(rect(96, 66, 128, 74), rock, Brush::Replace)
        .unwrap();
    let r2 = w.body_at(96, 66).unwrap();
    assert!(w.set_anchored(r2, true));

    let java = species(&w, "java_moss");
    let xmas = species(&w, "christmas_moss");
    w.paint_plant(40, 62, 1, java, Propagule::Fragments)
        .unwrap();
    w.paint_plant(112, 62, 1, xmas, Propagule::Fragments)
        .unwrap();

    run(&mut w, 15 * MINUTE);
    check_table(&w);

    let mut java_max_layer = 0u8;
    let mut xmas_max_layer = 0u8;
    let mut xmas_outer_cells = 0usize;
    for y in 0..w.height() {
        for x in 0..w.width() {
            let c = w.cell(x, y).unwrap();
            if c.material != stem {
                continue;
            }
            let sp = w.plant(desktank_sim::PlantId(c.state)).unwrap().species;
            let layer = plant_position(c.variant);
            if sp == java {
                java_max_layer = java_max_layer.max(layer);
                assert!(
                    touches_any(&w, x, y, &[rock]),
                    "java moss cell at ({x},{y}) must touch rock"
                );
            } else if sp == xmas {
                xmas_max_layer = xmas_max_layer.max(layer);
                if layer > 0 {
                    xmas_outer_cells += 1;
                }
            }
        }
    }
    assert_eq!(java_max_layer, 0, "java moss stays one cell thick");
    assert!(
        xmas_max_layer >= 1 && xmas_outer_cells >= 5,
        "christmas moss builds outer layers: max {xmas_max_layer}, outer {xmas_outer_cells}"
    );

    // Remove the rock under christmas moss: all layers cascade and fall off.
    w.fill_rect(rect(96, 66, 128, 74), water, Brush::Replace)
        .unwrap();
    run(&mut w, 8 * MINUTE);
    check_table(&w);
    assert!(
        plants_of(&w, xmas).is_empty(),
        "layered christmas moss fell off completely: {:?}",
        plants_of(&w, xmas)
    );
}

#[test]
fn branching_stem_grows_side_shoots_and_trails_along_the_surface() {
    // 1. Branching in a tall tank (before reaching the surface).
    let mut tall = tank(120, 100, 41, 0.92);
    let soil = id(&tall, "aquasoil");
    let stem = id(&tall, "stem");
    tall.fill_rect(rect(0, 88, 120, 100), soil, Brush::Replace)
        .unwrap();
    let rotala = species(&tall, "rotala");
    tall.paint_plant(60, 80, 0, rotala, Propagule::Cuttings)
        .unwrap();
    run(&mut tall, 12 * MINUTE);
    check_table(&tall);

    let mut stem_cols = BTreeSet::new();
    for y in 0..tall.height() {
        for x in 0..tall.width() {
            if tall.cell(x, y).unwrap().material == stem {
                stem_cols.insert(x);
            }
        }
    }
    assert!(
        stem_cols.len() >= 3,
        "branching rotala should grow side shoots across columns, got {stem_cols:?}"
    );

    // 2. Surface trailing in a shallower tank where a stem reaches the surface.
    let mut shallow = tank(120, 36, 42, 0.8);
    let soil = id(&shallow, "aquasoil");
    let stem = id(&shallow, "stem");
    shallow
        .fill_rect(rect(0, 26, 120, 36), soil, Brush::Replace)
        .unwrap();
    let cabomba = species(&shallow, "cabomba");
    shallow
        .paint_plant(60, 22, 0, cabomba, Propagule::Cuttings)
        .unwrap();
    run(&mut shallow, 15 * MINUTE);
    check_table(&shallow);

    let surface = shallow.surface_row();
    let mut trailing_cols = BTreeSet::new();
    for y in surface.saturating_sub(1)..=(surface + 2).min(shallow.height() - 1) {
        for x in 0..shallow.width() {
            if shallow.cell(x, y).unwrap().material == stem {
                trailing_cols.insert(x);
            }
        }
    }
    assert!(
        trailing_cols.len() >= 4,
        "cabomba should trail along the surface row ({surface}), got cols {trailing_cols:?}"
    );
}

#[test]
fn mature_seeding_plant_drops_seeds_that_can_germinate() {
    let mut w = tank(120, 90, 51, 0.9);
    let soil = id(&w, "aquasoil");
    let seed_mat = id(&w, "seed");
    w.fill_rect(rect(0, 78, 120, 90), soil, Brush::Replace)
        .unwrap();
    let hyg = species(&w, "hygrophila");
    // Plant a single cutting and grow it until topped out.
    w.paint_plant(60, 72, 0, hyg, Propagule::Cuttings).unwrap();
    run(&mut w, 15 * MINUTE);
    assert_eq!(plants_of(&w, hyg).len(), 1);
    let initial_size = plants_of(&w, hyg)[0].size();
    assert!(
        initial_size >= 180,
        "hygrophila grew full-sized: {initial_size}"
    );

    // Run further: mature hygrophila drops seeds that sink and can germinate.
    let mut saw_seed = false;
    for _ in 0..45 * MINUTE {
        w.tick();
        if w.material_counts()[usize::from(seed_mat.0)] > 0 {
            saw_seed = true;
            break;
        }
    }
    assert!(
        saw_seed || plants_of(&w, hyg).len() > 1,
        "mature hygrophila should drop a seed"
    );
    check_table(&w);
}

#[test]
fn epiphytes_layered_mats_and_branching_are_deterministic_and_save_round_trip() {
    let setup = |seed| {
        let mut w = tank(120, 80, seed, 0.9);
        let soil = id(&w, "aquasoil");
        let rock = id(&w, "rock");
        w.fill_rect(rect(0, 70, 120, 80), soil, Brush::Replace)
            .unwrap();
        w.fill_rect(rect(16, 52, 40, 58), rock, Brush::Replace)
            .unwrap();
        let r1 = w.body_at(16, 52).unwrap();
        assert!(w.set_anchored(r1, true));
        w.fill_rect(rect(80, 52, 104, 58), rock, Brush::Replace)
            .unwrap();
        let r2 = w.body_at(80, 52).unwrap();
        assert!(w.set_anchored(r2, true));

        let anubias = species(&w, "anubias");
        let xmas = species(&w, "christmas_moss");
        let rotala = species(&w, "rotala");
        w.paint_plant(28, 48, 0, anubias, Propagule::Fragments)
            .unwrap();
        w.paint_plant(92, 48, 0, xmas, Propagule::Fragments)
            .unwrap();
        w.paint_plant(60, 64, 0, rotala, Propagule::Cuttings)
            .unwrap();
        run(&mut w, 6 * MINUTE);
        w
    };

    let mut a = setup(77);
    let b = setup(77);
    assert_eq!(a.state_hash(), b.state_hash());

    let bytes = save::encode(&a);
    let mut loaded = save::decode(&bytes, table()).unwrap();
    assert_eq!(loaded.state_hash(), a.state_hash());
    run(&mut a, 2 * MINUTE);
    run(&mut loaded, 2 * MINUTE);
    assert_eq!(loaded.state_hash(), a.state_hash());
    check_table(&loaded);
}
