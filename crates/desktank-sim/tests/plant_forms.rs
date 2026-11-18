//! SPEC T6c (part 1) acceptance tests (§2.7 *Growth forms*): rosettes and
//! carpets. A rosette grows a crown on the substrate with blades above and
//! roots below; runners creep through the top of the substrate and root
//! child plants (parent id set, generation + 1); a carpet stays low and
//! spreads sideways; the table matches the grid throughout; saves keep
//! runners; a root feeder grows on a root tab.
#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]

use std::collections::BTreeMap;

use desktank_sim::{
    Brush, MaterialId, MaterialTable, PlantInfo, PlantSpeciesId, Propagule, Rect, World,
    WorldConfig, save,
};

/// Ticks in a simulated minute.
const MINUTE: u32 = 1200;
/// Tank size and bed depth (the bed's top row is `HEIGHT - BED`).
const WIDTH: usize = 160;
const HEIGHT: usize = 64;
const BED: usize = 10;
const SURFACE: usize = HEIGHT - BED;

fn table() -> MaterialTable {
    MaterialTable::builtin().unwrap()
}

/// A tank with a flat aquasoil bed.
fn bedded(seed: u64) -> World {
    let mut w = World::new(
        WorldConfig {
            width: WIDTH,
            height: HEIGHT,
            seed,
            water_level: 0.95,
        },
        table(),
    )
    .unwrap();
    let soil = id(&w, "aquasoil");
    w.fill_rect(rect(0, SURFACE, WIDTH, HEIGHT), soil, Brush::Replace)
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

/// Plants `name` at column `x` as its default propagule (runners pour a
/// plantlet).
fn plant(w: &mut World, name: &str, x: i64) {
    let s = species(w, name);
    let p = w.plant_species().get(s).unwrap().default_propagule();
    w.paint_plant(x, 20, 0, s, p).unwrap();
}

fn run(w: &mut World, ticks: u32) {
    for _ in 0..ticks {
        w.tick();
    }
}

/// The plant table matches the grid: every part cell belongs to a plant in
/// the table, and each plant's counts (runners with roots) and bounds are
/// exactly its cells'.
fn check_table(w: &World) {
    let parts = ["root", "stem", "leaf", "runner"].map(|n| id(w, n));
    let mut grid: BTreeMap<u16, ([u32; 3], Rect)> = BTreeMap::new();
    for y in 0..w.height() {
        for x in 0..w.width() {
            let c = w.cell(x, y).unwrap();
            let Some(k) = parts.iter().position(|&m| m == c.material) else {
                continue;
            };
            let e = grid.entry(c.state).or_insert(([0; 3], Rect::EMPTY));
            e.0[k % 3] += 1;
            e.1 = e.1.union(rect(x, y, x + 1, y + 1));
        }
    }
    let table: BTreeMap<u16, ([u32; 3], Rect)> =
        w.plants().map(|p| (p.id.0, (p.parts, p.bounds))).collect();
    assert_eq!(grid, table, "tick {}", w.tick_count());
}

/// Cells of `material` belonging to plant `id`.
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

fn of_species(w: &World, s: PlantSpeciesId) -> Vec<PlantInfo> {
    w.plants().filter(|p| p.species == s).collect()
}

#[test]
fn a_rosette_grows_a_crown_on_the_substrate_with_blades_above_and_roots_below() {
    for name in ["amazon_sword", "vallisneria", "cryptocoryne"] {
        let mut w = bedded(3);
        plant(&mut w, name, 80);
        for _ in 0..10 {
            run(&mut w, MINUTE);
            check_table(&w);
        }
        let s = species(&w, name);
        let first = of_species(&w, s)[0];
        let (leaf, stem, root) = (id(&w, "leaf"), id(&w, "stem"), id(&w, "root"));
        // The crown sits on the bed, rightwards from where it rooted.
        let crown = cells_of(&w, first.id.0, stem);
        assert!(!crown.is_empty(), "{name}");
        assert!(crown.iter().all(|&(_, y)| y == SURFACE - 1), "{name}");
        assert!(crown.iter().all(|&(x, _)| x >= first.base.0), "{name}");
        // Blades rise above it, several of them, and the tallest a good way.
        // Cryptocoryne is slow.
        let least = if name == "cryptocoryne" { 8 } else { 20 };
        let blades = cells_of(&w, first.id.0, leaf);
        assert!(
            blades.len() >= least,
            "{name}: {} blade cells",
            blades.len()
        );
        assert!(blades.iter().all(|&(_, y)| y < SURFACE), "{name}");
        let top = blades.iter().map(|&(_, y)| y).min().unwrap();
        assert!(SURFACE - top >= least / 2, "{name}: {} tall", SURFACE - top);
        let starts = blades
            .iter()
            .filter(|&&(x, y)| w.cell(x, y).unwrap().variant & 63 == 1)
            .count();
        assert!(starts >= 3, "{name}: {starts} blades");
        // Roots below, in the bed.
        let roots = cells_of(&w, first.id.0, root);
        assert!(roots.len() >= least / 4, "{name}");
        assert!(roots.iter().all(|&(_, y)| y >= SURFACE), "{name}");
    }
}

#[test]
fn runners_creep_through_the_top_of_the_bed_and_root_child_plants() {
    let mut w = bedded(5);
    plant(&mut w, "vallisneria", 80);
    for _ in 0..30 {
        run(&mut w, MINUTE);
        check_table(&w);
    }
    let s = species(&w, "vallisneria");
    let plants = of_species(&w, s);
    let first = plants[0];
    assert_eq!((first.parent, first.generation), (None, 0));
    let children: Vec<_> = plants
        .iter()
        .filter(|p| p.parent == Some(first.id))
        .collect();
    assert!(children.len() >= 2, "{plants:?}");
    for c in &children {
        assert_eq!(c.generation, 1);
        // Rooted on the bed (its crown) at least a runner's spacing out.
        assert_eq!(c.base.1, SURFACE - 1);
        assert!(c.base.0.abs_diff(first.base.0) >= 9, "{:?}", c.base);
    }
    // One child to each side.
    assert!(children.iter().any(|c| c.base.0 < first.base.0));
    assert!(children.iter().any(|c| c.base.0 > first.base.0));
    // The runners lie in the top two rows of the bed.
    let runner = id(&w, "runner");
    let runs = cells_of(&w, first.id.0, runner);
    assert!(runs.len() >= 8, "{} runner cells", runs.len());
    assert!(
        runs.iter()
            .all(|&(_, y)| (SURFACE..SURFACE + 2).contains(&y))
    );
    // Children send out runners in turn: a grandchild appears later.
    for _ in 0..20 {
        run(&mut w, MINUTE);
    }
    check_table(&w);
    assert!(
        of_species(&w, s).iter().any(|p| p.generation >= 2),
        "no grandchild"
    );
}

#[test]
fn a_carpet_stays_low_and_spreads_sideways_by_runners() {
    for name in ["dwarf_hairgrass", "monte_carlo"] {
        let mut w = bedded(2);
        plant(&mut w, name, 80);
        for _ in 0..15 {
            run(&mut w, MINUTE);
            check_table(&w);
        }
        let s = species(&w, name);
        let tufts = of_species(&w, s);
        assert!(tufts.len() >= 5, "{name}: {} tufts", tufts.len());
        let leaf = id(&w, "leaf");
        let mut x0 = usize::MAX;
        let mut x1 = 0;
        for t in &tufts {
            // Each tuft is a small plant with its crown on the bed, its
            // blades within three cells of it.
            assert_eq!(t.base.1, SURFACE - 1, "{name}");
            assert!(t.size() <= 30, "{name}: {}", t.size());
            // Blades within three cells of the crown.
            for (x, y) in cells_of(&w, t.id.0, leaf) {
                assert!(y + 4 >= SURFACE && y < SURFACE, "{name}: leaf at {y}");
                (x0, x1) = (x0.min(x), x1.max(x));
            }
            if t.generation > 0 {
                assert!(t.parent.is_some(), "{name}");
            }
        }
        assert!(x1 - x0 >= 20, "{name}: {x0}..={x1}");
        assert!(x0 < 78 && x1 > 82, "{name}: spreads both ways");
    }
}

#[test]
fn a_root_feeder_grows_more_over_a_buried_root_tab() {
    let grow = |tab: bool, seed: u64| {
        let mut w = bedded(seed);
        if tab {
            let t = id(&w, "root_tab");
            w.paint(82, (SURFACE + 4) as i64, 1, t, Brush::Replace)
                .unwrap();
        }
        plant(&mut w, "amazon_sword", 80);
        run(&mut w, 10 * MINUTE);
        let s = species(&w, "amazon_sword");
        of_species(&w, s)[0].size()
    };
    for seed in [7, 8] {
        let (bare, fed) = (grow(false, seed), grow(true, seed));
        assert!(fed * 10 >= bare * 14, "seed {seed}: {bare} bare, {fed} fed");
    }
}

#[test]
fn cutting_a_runner_keeps_the_child_and_the_table_matches() {
    let mut w = bedded(5);
    plant(&mut w, "vallisneria", 80);
    run(&mut w, 20 * MINUTE);
    let s = species(&w, "vallisneria");
    let first = of_species(&w, s)[0];
    let child = *of_species(&w, s)
        .iter()
        .find(|p| p.parent == Some(first.id))
        .unwrap();
    // Scoop the runner between them, one cell in from the child.
    let x = if child.base.0 > first.base.0 {
        child.base.0 - 2
    } else {
        child.base.0 + 2
    };
    let water = id(&w, "water");
    w.set_cell(x, SURFACE, water).unwrap();
    run(&mut w, 2);
    check_table(&w);
    let after = w.plant(child.id).unwrap();
    assert_eq!(after.parent, Some(first.id));
    assert!(after.size() >= child.size());
    assert!(w.plant(first.id).is_some());
}

#[test]
fn rosettes_and_carpets_are_deterministic_and_survive_a_save() {
    let grow = || {
        let mut w = bedded(11);
        plant(&mut w, "vallisneria", 30);
        plant(&mut w, "amazon_sword", 80);
        plant(&mut w, "dwarf_hairgrass", 120);
        plant(&mut w, "monte_carlo", 145);
        run(&mut w, 8 * MINUTE);
        w
    };
    let (mut a, b) = (grow(), grow());
    assert_eq!(a.state_hash(), b.state_hash());
    assert!(a.material_counts()[usize::from(id(&a, "runner").0)] > 0);
    let mut loaded = save::decode(&save::encode(&a), table()).unwrap();
    assert_eq!(loaded.state_hash(), a.state_hash());
    for _ in 0..4 * MINUTE {
        a.tick();
        loaded.tick();
    }
    assert_eq!(loaded.state_hash(), a.state_hash());
    check_table(&loaded);
}

#[test]
fn runners_pour_a_plantlet_that_roots_like_a_cutting() {
    let mut w = bedded(1);
    let s = species(&w, "vallisneria");
    let sp = w.plant_species().get(s).unwrap();
    assert_eq!(sp.propagules().collect::<Vec<_>>(), [Propagule::Runners]);
    assert_eq!(sp.default_propagule(), Propagule::Runners);
    assert!(w.paint_plant(80, 20, 0, s, Propagule::Cuttings).is_err());
    w.paint_plant(80, 20, 0, s, Propagule::Runners).unwrap();
    assert_eq!(w.material_counts()[usize::from(id(&w, "cutting").0)], 1);
    run(&mut w, MINUTE);
    assert_eq!(of_species(&w, s).len(), 1);
}
