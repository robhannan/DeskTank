//! Issue #45 acceptance tests: marimo and tiger lotus. A marimo fragment
//! sinks to the bed, roots as a ball and grows round, within its radius;
//! a current nudges it along the bottom, where it stays; it is one body
//! that saves keep exactly. A tiger lotus puts up petioles of differing
//! lengths, each spreading a broad leaf pad where it stops, those at the
//! surface lying on the water as floating leaves that shade the tank; the
//! table matches the grid throughout and growth is deterministic. It
//! spreads by deep runners that root new bulbs far off.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names
)]

use std::collections::{BTreeMap, BTreeSet};

use desktank_sim::{
    Brush, FieldKind, MaterialId, MaterialTable, PlantInfo, PlantSpeciesId, Rect, World,
    WorldConfig, save,
};

/// Ticks in a simulated minute.
const MINUTE: u32 = 1200;
const WIDTH: usize = 160;
const HEIGHT: usize = 90;
/// The bed's top row is `HEIGHT - BED`.
const BED: usize = 8;
const FLOOR: usize = HEIGHT - BED;

fn table() -> MaterialTable {
    MaterialTable::builtin().unwrap()
}

/// A tank with a flat aquasoil bed, filled to `water_level`.
fn tank(seed: u64, water_level: f32) -> World {
    let mut w = World::new(
        WorldConfig {
            width: WIDTH,
            height: HEIGHT,
            seed,
            water_level,
        },
        table(),
    )
    .unwrap();
    let soil = id(&w, "aquasoil");
    w.fill_rect(rect(0, FLOOR, WIDTH, HEIGHT), soil, Brush::Replace)
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

/// Pours one propagule of `name` (its default) at `(x, y)`.
fn pour(w: &mut World, name: &str, x: i64, y: i64) {
    let s = species(w, name);
    let p = w.plant_species().get(s).unwrap().default_propagule();
    w.paint_plant(x, y, 0, s, p).unwrap();
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
fn a_marimo_sinks_to_the_bed_and_grows_round() {
    for seed in 0..3 {
        let mut w = tank(seed, 0.9);
        let s = species(&w, "marimo");
        let radius = w
            .plant_species()
            .get(s)
            .unwrap()
            .form
            .ball()
            .unwrap()
            .radius;
        let span = 2 * usize::from(radius) + 1;
        let stem = id(&w, "stem");
        // Dropped in mid-water, the fragment sinks and roots on the bed.
        pour(&mut w, "marimo", 80, 40);
        run(&mut w, MINUTE);
        let balls = of_species(&w, s);
        assert_eq!(balls.len(), 1, "seed {seed}: {balls:?}");
        let id0 = balls[0].id;
        for _ in 0..30 {
            run(&mut w, MINUTE);
            check_table(&w);
            let p = w.plant(id0).unwrap();
            // A ball: stem cells only, resting on the bed, within its disc.
            assert_eq!((p.parts[0], p.parts[2]), (0, 0), "{p:?}");
            assert_eq!(p.bounds.y1, FLOOR, "seed {seed}: resting on the bed: {p:?}");
            assert!(p.bounds.x1 - p.bounds.x0 <= span && p.bounds.y1 - p.bounds.y0 <= span);
        }
        let p = w.plant(id0).unwrap();
        let cells = cells_of(&w, id0.0, stem);
        let (wide, tall) = (p.bounds.x1 - p.bounds.x0, p.bounds.y1 - p.bounds.y0);
        // Grown to most of its size, about as wide as it is tall, and
        // solid: each row a single run, its middle row the widest.
        assert!(p.size() >= 30, "seed {seed}: {p:?}");
        assert!(wide.abs_diff(tall) <= 2, "seed {seed}: {wide}x{tall}");
        let mut rows: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        for &(x, y) in &cells {
            rows.entry(y).or_default().insert(x);
        }
        for xs in rows.values() {
            let (a, b) = (xs.first().unwrap(), xs.last().unwrap());
            assert_eq!(b - a + 1, xs.len(), "seed {seed}: gap in a row: {cells:?}");
        }
        let widths: Vec<usize> = rows.values().map(BTreeSet::len).collect();
        let widest = *widths.iter().max().unwrap();
        assert!(widths[0] < widest, "seed {seed}: a rounded top: {widths:?}");
    }
}

#[test]
fn a_current_nudges_a_marimo_along_the_bottom() {
    let make = |outlet: bool| {
        let mut w = tank(2, 0.9);
        pour(&mut w, "marimo", 60, 60);
        run(&mut w, 3 * MINUTE);
        if outlet {
            // An internal filter high on the right wall: its leftward jet
            // deflects down the left wall and circulates back rightwards along
            // the bed, nudging the marimo right.
            place_filter(&mut w, WIDTH as i64 - 2, 24);
        }
        w
    };
    let (mut with, mut without) = (make(true), make(false));
    let s = species(&with, "marimo");
    let start = of_species(&with, s)[0];
    assert!(start.size() >= 3, "{start:?}");
    assert_eq!(start, of_species(&without, s)[0]);
    run(&mut with, MINUTE / 2);
    run(&mut without, MINUTE / 2);
    let p = of_species(&with, s)[0];
    let moved = p.base.0 as i64 - start.base.0 as i64;
    let still = of_species(&without, s)[0].base.0 as i64 - start.base.0 as i64;
    // The jet's lower edge reaches the bed at a couple of cells a second;
    // the ball rolls at a quarter of that at most.
    assert!(
        (4..=70).contains(&moved),
        "nudged {moved} cells in 30 s of current"
    );
    assert_eq!(still, 0, "no current: it stays put");
    // It stays on the bottom: the current pushes it along, never up.
    assert_eq!(p.bounds.y1, FLOOR, "{p:?}");
    check_table(&with);
}

#[test]
fn a_marimo_rolls_down_a_slope_and_settles_at_the_bottom() {
    for run_per_step in 1..=3 {
        let mut w = tank(2, 0.95);
        let s = species(&w, "marimo");
        let rock = id(&w, "rock");
        let start = 64;
        let top = FLOOR - 12;
        let end = start + (FLOOR - top) * run_per_step;
        w.fill_rect(rect(0, top, end, FLOOR), rock, Brush::Replace)
            .unwrap();
        pour(&mut w, "marimo", start as i64, top as i64 - 1);
        run(&mut w, 6 * MINUTE);
        let before = of_species(&w, s)[0];
        assert!(before.bounds.x1 - before.bounds.x0 >= 3, "{before:?}");
        assert_eq!(before.roll, 0);
        let water = id(&w, "water");
        // Lower the bed from the centre of the ball to its right.
        for x in start + 1..end {
            let y = top + (x - start - 1) / run_per_step + 1;
            w.fill_rect(rect(x, top, x + 1, y), water, Brush::Replace)
                .unwrap();
        }
        let mut rolled = false;
        for _ in 0..10 {
            let version = w.plants_version();
            run(&mut w, MINUTE);
            if w.plant(before.id).unwrap().roll != 0 && !rolled {
                assert!(w.plants_version() > version);
                rolled = true;
            }
        }
        let p = of_species(&w, s)[0];
        assert!(rolled, "1:{run_per_step}: {p:?}");
        assert_eq!(p.bounds.y1, FLOOR, "1:{run_per_step}: {p:?}");
        assert!(p.bounds.x1 >= end, "1:{run_per_step}: {p:?}");
        run(&mut w, MINUTE);
        let later = w.plant(p.id).unwrap();
        assert_eq!((later.base, later.roll), (p.base, p.roll));
        check_table(&w);
    }

    for shelf_height in [0, 8] {
        let mut flat = tank(2, 0.95);
        let bed = FLOOR - shelf_height;
        if shelf_height > 0 {
            let rock = id(&flat, "rock");
            flat.fill_rect(rect(0, bed, 80, FLOOR), rock, Brush::Replace)
                .unwrap();
        }
        pour(&mut flat, "marimo", 64, bed as i64 - 1);
        run(&mut flat, 3 * MINUTE);
        let p = of_species(&flat, species(&flat, "marimo"))[0];
        assert_eq!((p.base.0, p.bounds.y1, p.roll), (64, bed, 0));
        let version = flat.plants_version();
        run(&mut flat, MINUTE);
        assert_eq!(flat.plants_version(), version);
    }
}

#[test]
fn a_poured_disc_of_marimo_fragments_becomes_one_ball() {
    for seed in [0, 1, 2, 4] {
        let mut w = tank(seed, 0.9);
        let s = species(&w, "marimo");
        let fragment = w.plant_species().get(s).unwrap().default_propagule();
        w.paint_plant(80, 60, 3, s, fragment).unwrap();
        run(&mut w, 3 * MINUTE);
        let balls = of_species(&w, s);
        assert!((1..=2).contains(&balls.len()), "seed {seed}: {balls:?}");
        assert!(balls.iter().map(PlantInfo::size).sum::<u32>() >= 29);
        check_table(&w);
    }
}

#[test]
fn a_saved_rolling_marimo_continues_exactly() {
    let mut w = tank(6, 0.9);
    place_filter(&mut w, 2, FLOOR as i64 - 15);
    for x in [25, 60, 90] {
        pour(&mut w, "marimo", x, 40);
    }
    run(&mut w, MINUTE);
    let s = species(&w, "marimo");
    let balls = of_species(&w, s);
    assert!(balls.iter().any(|p| p.roll != 0), "{balls:?}");
    let bytes = save::encode(&w);
    let mut loaded = save::decode(&bytes, table()).unwrap();
    assert_eq!(of_species(&loaded, s), balls);
    assert_eq!(save::check(&bytes), Ok(14));
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
fn a_broken_marimo_piece_rounds_into_a_ball_of_its_own() {
    let mut w = tank(5, 0.9);
    let s = species(&w, "marimo");
    pour(&mut w, "marimo", 80, 60);
    run(&mut w, 20 * MINUTE);
    let p = of_species(&w, s)[0];
    assert!(p.size() >= 12, "{p:?}");
    // Cut it in two down a column through its middle.
    let water = id(&w, "water");
    let x = usize::midpoint(p.bounds.x0, p.bounds.x1);
    for y in p.bounds.y0..p.bounds.y1 {
        w.set_cell(x, y, water).unwrap();
    }
    run(&mut w, 20);
    check_table(&w);
    let pieces = of_species(&w, s);
    assert_eq!(pieces.len(), 2, "{pieces:?}");
    let piece = pieces.iter().find(|q| q.parent == Some(p.id)).unwrap();
    assert_eq!(piece.generation, p.generation + 1);
    let size = piece.size();
    run(&mut w, 5 * MINUTE);
    check_table(&w);
    let later = w.plant(piece.id).unwrap();
    assert!(later.size() > size, "the piece grows: {size} -> {later:?}");
    assert_eq!(later.bounds.y1, FLOOR, "and rests on the bed: {later:?}");
}

/// A tiger lotus in water shallow enough for its longest petioles to
/// reach the surface, grown for `minutes`.
fn lotus(seed: u64, minutes: u32) -> World {
    // About 40 cells of water over the bed, less than its longest petiole
    // reaches.
    let mut w = tank(seed, 0.55);
    pour(&mut w, "tiger_lotus", 80, 40);
    run(&mut w, minutes * MINUTE);
    w
}

#[test]
fn a_tiger_lotus_spreads_leaf_pads_and_floats_some_on_the_surface() {
    let mut w = lotus(1, 0);
    let s = species(&w, "tiger_lotus");
    let f = w
        .plant_species()
        .get(s)
        .unwrap()
        .form
        .rosette()
        .unwrap()
        .clone();
    let (leaf, float) = (id(&w, "leaf"), id(&w, "floating_leaf"));
    for _ in 0..40 {
        run(&mut w, MINUTE);
        check_table(&w);
    }
    let plants = of_species(&w, s);
    let first = plants.iter().find(|p| p.parent.is_none()).unwrap();
    let cells: Vec<_> = cells_of(&w, first.id.0, leaf)
        .into_iter()
        .chain(cells_of(&w, first.id.0, float))
        .map(|(x, y)| (x, y, w.cell(x, y).unwrap().variant & 63))
        .collect();
    let (pads, petioles): (Vec<&(usize, usize, u8)>, Vec<_>) =
        cells.iter().partition(|c| f.is_pad(c.2));
    assert!(!petioles.is_empty() && !pads.is_empty(), "{first:?}");
    // Petioles stop at differing heights: their tips (where pads start)
    // sit on more than one row.
    let tips: BTreeSet<usize> = pads
        .iter()
        .filter(|c| c.2 == f.blade_length + 1)
        .map(|c| c.1)
        .collect();
    assert!(tips.len() >= 2, "pads at rows {tips:?}");
    // A pad is broad: some row of pad cells runs at least five wide.
    let mut rows: BTreeMap<usize, usize> = BTreeMap::new();
    for c in &pads {
        *rows.entry(c.1).or_default() += 1;
    }
    assert!(rows.values().any(|&n| n >= 5), "pad rows {rows:?}");
    // Submerged pads have a narrower upper row.
    assert!(pads.iter().any(|c| c.2 == f.pad_top()), "no upper rows");
    // The longest reach the surface and lie on it as floating leaves,
    // which shade the bed below them.
    let surface = w.surface_row();
    let floating = cells_of(&w, first.id.0, float);
    assert!(floating.len() >= 3, "floating pads {floating:?}");
    assert!(floating.iter().all(|&(_, y)| y == surface), "{floating:?}");
    let light = w.field(FieldKind::Light);
    let under = floating[floating.len() / 2].0;
    // (Light is full just under the surface; look a little deeper.)
    let shaded = light.at_cell(under, surface + 16);
    // Open water: a column well clear of every leaf (its runners root
    // bulbs across the tank).
    let leaves: Vec<usize> = (0..WIDTH)
        .filter(|&x| (0..FLOOR).any(|y| [leaf, float].contains(&w.cell(x, y).unwrap().material)))
        .collect();
    let clear = (0..WIDTH)
        .find(|&x| leaves.iter().all(|&l| l.abs_diff(x) > 8))
        .expect("open water somewhere");
    let open = light.at_cell(clear, surface + 16);
    assert!(
        shaded < 0.95 * open,
        "light under a pad {shaded}, in the open {open}"
    );
}

#[test]
fn a_tiger_lotus_roots_new_bulbs_far_off_through_deep_runners() {
    let mut w = lotus(1, 0);
    let s = species(&w, "tiger_lotus");
    let f = w
        .plant_species()
        .get(s)
        .unwrap()
        .form
        .rosette()
        .unwrap()
        .clone();
    let runner = id(&w, "runner");
    for _ in 0..40 {
        run(&mut w, MINUTE);
        check_table(&w);
    }
    let plants = of_species(&w, s);
    let first = plants.iter().find(|p| p.parent.is_none()).unwrap();
    // A child bulb on each side, far further out than a carpet's (3
    // cells) or a sword's (14), and each alone: a runner stops once it has
    // rooted its plant.
    let fx = first.base.0 as i64;
    let mut sides = BTreeSet::new();
    for c in plants.iter().filter(|p| p.parent == Some(first.id)) {
        let dx = c.base.0 as i64 - fx;
        assert!(dx.abs() >= 30, "a child {dx} cells out: {plants:?}");
        sides.insert(dx.signum());
    }
    assert_eq!(sides.len(), 2, "{plants:?}");
    let bases: Vec<i64> = plants.iter().map(|p| p.base.0 as i64).collect();
    for (k, a) in bases.iter().enumerate() {
        for b in &bases[k + 1..] {
            assert!((a - b).abs() >= 20, "bulbs crowd at {bases:?}");
        }
    }
    // Its runners creep underground: most of their cells at runner_depth
    // (or a cell short of it) below the bed's top row, none deeper.
    let cells = cells_of(&w, first.id.0, runner);
    let depth = usize::from(f.runner_depth);
    let deep = cells
        .iter()
        .filter(|&&(_, y)| y + 1 >= FLOOR + depth)
        .count();
    assert!(deep * 2 > cells.len(), "{deep} of {} deep", cells.len());
    assert!(cells.iter().all(|&(_, y)| y <= FLOOR + depth), "{cells:?}");
}

#[test]
fn a_saved_tiger_lotus_with_floating_pads_continues_exactly() {
    let mut w = lotus(1, 40);
    let s = species(&w, "tiger_lotus");
    let float = id(&w, "floating_leaf");
    let first = of_species(&w, s)
        .into_iter()
        .find(|p| p.parent.is_none())
        .unwrap();
    assert!(
        !cells_of(&w, first.id.0, float).is_empty(),
        "no floating pads yet: {first:?}"
    );
    let bytes = save::encode(&w);
    let mut loaded = save::decode(&bytes, table()).unwrap();
    assert_eq!(loaded.state_hash(), w.state_hash());
    for _ in 0..10 {
        run(&mut w, MINUTE / 2);
        run(&mut loaded, MINUTE / 2);
        assert_eq!(
            loaded.state_hash(),
            w.state_hash(),
            "tick {}",
            w.tick_count()
        );
    }
    check_table(&loaded);
    assert_eq!(save::encode(&loaded), save::encode(&w));
}

#[test]
fn tiger_lotus_growth_is_deterministic() {
    let a = lotus(3, 12);
    let b = lotus(3, 12);
    assert_eq!(a.state_hash(), b.state_hash());
    assert_ne!(a.state_hash(), lotus(4, 12).state_hash());
    check_table(&a);
}
