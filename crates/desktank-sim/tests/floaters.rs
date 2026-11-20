//! SPEC T6c (part 2) acceptance tests (§2.7 *Growth forms*, D22):
//! floaters are buoyant bodies. A floater's plantlet rises to the surface
//! and rests with its leaves in the top water row under the air; it drifts
//! with the current and stops at the glass; it shades the water below it;
//! it buds child plants (parent id set) and never past the plant cap; the
//! table matches the grid while it moves and grows; a cut piece floats on
//! as a fragment; saves keep its motion exactly.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names
)]

use std::collections::BTreeMap;

use desktank_sim::{
    BAND_REACH, Brush, CreatureId, FieldKind, MaterialId, MaterialTable, Placeable, PlantInfo,
    PlantSpeciesId, Propagule, Rect, World, WorldConfig, save,
};

/// Ticks in a simulated minute.
const MINUTE: u32 = 1200;
const WIDTH: usize = 160;
const HEIGHT: usize = 64;
const BED: usize = 8;

fn table() -> MaterialTable {
    MaterialTable::builtin().unwrap()
}

/// A tank with a flat aquasoil bed (which feeds the water) under the air
/// band.
fn tank(seed: u64) -> World {
    let mut w = World::new(
        WorldConfig {
            width: WIDTH,
            height: HEIGHT,
            seed,
            water_level: 0.9,
        },
        table(),
    )
    .unwrap();
    let soil = id(&w, "aquasoil");
    w.fill_rect(rect(0, HEIGHT - BED, WIDTH, HEIGHT), soil, Brush::Replace)
        .unwrap();
    w
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

/// Pours one budded plantlet of `name` at column `x`, `depth` rows below
/// the top water row.
fn bud(w: &mut World, name: &str, x: i64, depth: i64) {
    let s = species(w, name);
    let y = w.surface_row() as i64 + depth;
    w.paint_plant(x, y, 0, s, Propagule::Buds).unwrap();
}

fn of_species(w: &World, s: PlantSpeciesId) -> Vec<PlantInfo> {
    w.plants().filter(|p| p.species == s).collect()
}

/// The cells of plant `id` of `material`.
fn cells_of(w: &World, plant: u16, material: MaterialId) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for y in 0..w.height() {
        for x in 0..w.width() {
            let c = w.cell(x, y).unwrap();
            if c.material == material && c.state == plant {
                out.push((x, y));
            }
        }
    }
    out
}

/// The plant table matches the grid: every part cell belongs to a plant in
/// the table, and each plant's counts and bounds are exactly its cells'.
fn check_table(w: &World) {
    // Runners count with roots, floating leaves with leaves.
    let parts = ["root", "stem", "leaf", "runner", "floating_leaf"].map(|n| id(w, n));
    let slot = [0, 1, 2, 0, 2];
    let mut grid: BTreeMap<u16, ([u32; 3], Rect)> = BTreeMap::new();
    for y in 0..w.height() {
        for x in 0..w.width() {
            let c = w.cell(x, y).unwrap();
            let Some(k) = parts.iter().position(|&m| m == c.material) else {
                continue;
            };
            let e = grid.entry(c.state).or_insert(([0; 3], Rect::EMPTY));
            e.0[slot[k]] += 1;
            e.1 = e.1.union(rect(x, y, x + 1, y + 1));
        }
    }
    let table: BTreeMap<u16, ([u32; 3], Rect)> =
        w.plants().map(|p| (p.id.0, (p.parts, p.bounds))).collect();
    assert_eq!(grid, table, "tick {}", w.tick_count());
}

/// The top water row: the first row of water under the air band.
fn top_water_row(w: &World) -> usize {
    w.surface_row()
}

#[test]
fn a_plantlet_rises_and_rests_with_its_leaves_in_the_top_water_row() {
    for name in ["duckweed", "frogbit"] {
        let mut w = tank(1);
        let s = species(&w, name);
        let leaf = id(&w, "floating_leaf");
        let air = id(&w, "air");
        // Poured near the bed: it opens in open water and floats up.
        let depth = (HEIGHT - BED - 4 - w.surface_row()) as i64;
        bud(&mut w, name, 80, depth);
        run(&mut w, MINUTE / 2);
        let plants = of_species(&w, s);
        assert_eq!(plants.len(), 1, "{name}: one plant");
        let p = plants[0];
        let surface = top_water_row(&w);
        // Then it stays there, its leaves under the air, as it grows roots
        // and fronds.
        for _ in 0..3 * MINUTE / 20 {
            run(&mut w, 20);
            check_table(&w);
            let leaves = cells_of(&w, p.id.0, leaf);
            assert!(!leaves.is_empty(), "{name}: has leaves");
            for &(x, y) in &leaves {
                assert_eq!(
                    y,
                    surface,
                    "{name}: leaf at ({x}, {y}), tick {}",
                    w.tick_count()
                );
                assert_eq!(w.cell(x, y - 1).unwrap().material, air, "{name}: air above");
            }
        }
        let grown = w.plant(p.id).unwrap();
        assert!(grown.parts[0] >= 1, "{name}: hanging roots grew: {grown:?}");
        assert!(grown.parts[2] >= 2, "{name}: fronds grew: {grown:?}");
    }
}

#[test]
fn floaters_drift_with_the_current_and_stop_at_the_glass() {
    let make = |outlet: bool| {
        let mut w = tank(2);
        if outlet {
            // A jet along the surface, rightwards from the left wall.
            let row = w.surface_row() as i64 + 2;
            w.place(Placeable::FilterOutlet, 2, row).unwrap();
        }
        bud(&mut w, "duckweed", 30, 3);
        run(&mut w, MINUTE / 4);
        w
    };
    let (mut with, mut without) = (make(true), make(false));
    let s = species(&with, "duckweed");
    let start = (
        of_species(&with, s)[0].base.0,
        of_species(&without, s)[0].base.0,
    );
    run(&mut with, MINUTE / 4);
    run(&mut without, MINUTE / 4);
    let moved = of_species(&with, s)[0].base.0 as i64 - start.0 as i64;
    let still = of_species(&without, s)[0].base.0 as i64 - start.1 as i64;
    assert!(moved >= 20, "drifted {moved} cells in 15 s of current");
    assert_eq!(still, 0, "no current: it stays put");
    check_table(&with);
    // Downstream it fetches up against the glass and rests there.
    run(&mut with, 2 * MINUTE);
    let p = of_species(&with, s)[0];
    assert!(p.bounds.x1 >= WIDTH - 1, "at the far glass: {p:?}");
    let leaf = id(&with, "floating_leaf");
    for (_, y) in cells_of(&with, p.id.0, leaf) {
        assert_eq!(y, with.surface_row(), "still in the top water row");
    }
    check_table(&with);
}

#[test]
fn a_floating_mat_measurably_shades_the_water_below() {
    let mut w = tank(3);
    // A row of frogbit across the middle third of the surface.
    for x in (56..104).step_by(6) {
        bud(&mut w, "frogbit", x, 2);
    }
    run(&mut w, 4 * MINUTE);
    check_table(&w);
    let leaf = id(&w, "floating_leaf");
    let mut covered = 0;
    for x in 56..104 {
        covered += usize::from(w.cell(x, w.surface_row()).unwrap().material == leaf);
    }
    assert!(
        covered >= 24,
        "the frogbit covers half the strip: {covered}"
    );
    let light = w.field(FieldKind::Light);
    let y = HEIGHT - BED - 4;
    let shaded = light.at_cell(80, y);
    let open = light.at_cell(16, y);
    assert!(
        shaded < 0.75 * open,
        "light under the floaters {shaded}, in the open {open}"
    );
}

#[test]
fn floaters_bud_child_plants_within_the_cap() {
    let mut w = tank(4);
    let s = species(&w, "duckweed");
    bud(&mut w, "duckweed", 40, 2);
    bud(&mut w, "duckweed", 120, 2);
    run(&mut w, 2 * MINUTE);
    let plants = of_species(&w, s);
    // Every bud names its parent, a generation on.
    let by_id: BTreeMap<_, _> = plants.iter().map(|p| (p.id, *p)).collect();
    let children: Vec<_> = plants.iter().filter(|p| p.parent.is_some()).collect();
    assert!(!children.is_empty(), "buds made child plants: {plants:?}");
    for c in &children {
        if let Some(parent) = by_id.get(&c.parent.unwrap()) {
            assert_eq!(c.generation, parent.generation + 1);
        }
    }
    check_table(&w);
    // At the cap no bud opens.
    let cap = w.plant_count() + 2;
    w.set_plant_cap(cap);
    for _ in 0..4 * MINUTE / 100 {
        run(&mut w, 100);
        assert!(
            w.plant_count() <= cap,
            "{} plants over the cap {cap}",
            w.plant_count()
        );
    }
    assert_eq!(w.plant_count(), cap, "it buds up to the cap");
    check_table(&w);
}

#[test]
fn a_cut_floater_piece_floats_on_as_a_fragment() {
    let mut w = tank(5);
    let s = species(&w, "frogbit");
    bud(&mut w, "frogbit", 80, 2);
    run(&mut w, 5 * MINUTE);
    let p = of_species(&w, s)[0];
    assert!(p.parts[2] >= 3, "it has fronds to cut between: {p:?}");
    // Cut through the leaf row between its first and second fronds.
    let leaf = id(&w, "floating_leaf");
    let water = id(&w, "water");
    let leaves = cells_of(&w, p.id.0, leaf);
    let (x, y) = leaves[1];
    for dy in 0..HEIGHT - BED - y {
        w.set_cell(x, y + dy, water).unwrap();
    }
    run(&mut w, 20);
    check_table(&w);
    let pieces = of_species(&w, s);
    assert!(pieces.len() >= 2, "the cut made a fragment: {pieces:?}");
    let fragment = pieces.iter().find(|q| q.parent == Some(p.id)).unwrap();
    run(&mut w, MINUTE / 2);
    for (_, y) in cells_of(&w, fragment.id.0, leaf) {
        assert_eq!(y, w.surface_row(), "the fragment floats");
    }
    check_table(&w);
}

#[test]
fn a_floater_stripped_of_its_leaves_dies() {
    let mut w = tank(5);
    let s = species(&w, "frogbit");
    bud(&mut w, "frogbit", 80, 2);
    run(&mut w, 5 * MINUTE);
    let p = of_species(&w, s)[0];
    let root = id(&w, "root");
    assert!(
        !cells_of(&w, p.id.0, root).is_empty(),
        "it hangs roots: {p:?}"
    );
    // Take every floating leaf: the hanging roots alone can't float or
    // grow, so the plant dies rather than living on as roots.
    let (leaf, water) = (id(&w, "floating_leaf"), id(&w, "water"));
    for (x, y) in cells_of(&w, p.id.0, leaf) {
        w.set_cell(x, y, water).unwrap();
    }
    run(&mut w, 20);
    assert!(
        w.plants().all(|q| q.id != p.id),
        "the plant is gone: {:?}",
        of_species(&w, s)
    );
    assert!(cells_of(&w, p.id.0, root).is_empty(), "its roots died");
    check_table(&w);
}

#[test]
fn a_saved_drifting_floater_continues_exactly() {
    let mut w = tank(6);
    let row = w.surface_row() as i64 + 2;
    w.place(Placeable::FilterOutlet, 2, row).unwrap();
    for x in [20, 40, 60] {
        bud(&mut w, "duckweed", x, 3);
        bud(&mut w, "frogbit", x + 8, 8);
    }
    run(&mut w, MINUTE / 3);
    let bytes = save::encode(&w);
    let mut loaded = save::decode(&bytes, table()).unwrap();
    assert_eq!(loaded.state_hash(), w.state_hash());
    for _ in 0..30 {
        run(&mut w, 20);
        run(&mut loaded, 20);
        assert_eq!(
            loaded.state_hash(),
            w.state_hash(),
            "tick {}",
            w.tick_count()
        );
    }
    assert_eq!(save::encode(&loaded), save::encode(&w));
}

#[test]
fn floater_growth_is_deterministic() {
    let grow = || {
        let mut w = tank(7);
        bud(&mut w, "duckweed", 50, 20);
        bud(&mut w, "frogbit", 110, 30);
        run(&mut w, 2 * MINUTE);
        w.state_hash()
    };
    assert_eq!(grow(), grow());
}

#[test]
fn guppies_leave_frogbit_intact() {
    let mut w = tank(7);
    bud(&mut w, "frogbit", 40, 5);
    let fish = w
        .add_creature(w.species().id("guppy").unwrap(), 40, 12)
        .unwrap();
    run(&mut w, 200);
    assert_eq!(w.creature(fish).unwrap().eaten(), 0);
    assert!(w.plants().any(|p| p.species == species(&w, "frogbit")));
}

/// Plant cells inside `fish`'s body in reach of its band, as (material,
/// plant species).
fn plant_cells_in_reach(w: &World, fish: CreatureId) -> Vec<(MaterialId, PlantSpeciesId)> {
    let c = w.creature(fish).unwrap();
    let b = w.creature_body(c);
    let mut out = Vec::new();
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            let Some(band) = w.plant_band(x, y) else {
                continue;
            };
            if band.abs_diff(c.band()) <= BAND_REACH {
                let p = w.plant(w.plant_at(x, y).unwrap()).unwrap();
                out.push((w.cell(x, y).unwrap().material, p.species));
            }
        }
    }
    out
}

#[test]
fn guppies_swim_through_hanging_roots_but_not_stem_plants() {
    let mut w = tank(8);
    for x in (4..WIDTH as i64 - 4).step_by(6) {
        bud(&mut w, "duckweed", x, 2);
    }
    // A stand of stem plants (they still block within reach of their
    // bands).
    let rotala = species(&w, "rotala");
    for x in (60..100).step_by(4) {
        w.paint_plant(x, (HEIGHT - BED - 1) as i64, 0, rotala, Propagule::Cuttings)
            .unwrap();
    }
    run(&mut w, 5 * MINUTE);
    // A mat across the surface with roots hanging under it.
    let (leaf, root) = (id(&w, "floating_leaf"), id(&w, "root"));
    let row = w.surface_row();
    let count = |m: MaterialId, y: usize| {
        (0..WIDTH)
            .filter(|&x| w.cell(x, y).unwrap().material == m)
            .count()
    };
    let (leaves, roots) = (count(leaf, row), count(root, row + 1));
    assert!(
        leaves > WIDTH / 2 && roots > WIDTH / 6,
        "a rooted mat: {leaves} leaves, {roots} roots"
    );
    let duckweed = species(&w, "duckweed");
    let guppy = w.species().id("guppy").unwrap();
    let fish: Vec<_> = (0..4)
        .map(|i| w.add_creature(guppy, 20 + 40 * i, 30).unwrap())
        .collect();
    let mut through_roots = 0;
    for _ in 0..MINUTE {
        w.tick();
        for &f in &fish {
            let cells = plant_cells_in_reach(&w, f);
            for &(m, s) in &cells {
                assert!(
                    m == root && s == duckweed,
                    "a guppy overlaps a plant cell other than a floater's root \
                     in reach of its band, tick {}",
                    w.tick_count()
                );
            }
            through_roots += usize::from(!cells.is_empty());
        }
    }
    assert!(through_roots > 0, "the guppies swam among the roots");
    let eaten: u32 = fish
        .iter()
        .filter_map(|&f| w.creature(f))
        .map(desktank_sim::Creature::eaten)
        .sum();
    assert!(eaten >= 4, "the guppies grazed the mat: {eaten} bites");
}
