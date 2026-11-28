//! Whole plants placed in one click (#470): a juvenile or mature plant of
//! any species, snapped to where it can stand, mirrored by the flip, shown
//! as a ghost and written as ordinary plant cells (table, rafts, saves).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]

use std::collections::BTreeMap;

use desktank_sim::plants::{Form, plant_position};
use desktank_sim::tools::{Button, Outcome, PaletteItem, Tool, Tools};
use desktank_sim::{
    Brush, MaterialId, MaterialTable, Part, PlantGhost, PlantId, PlantSpeciesId, PlantStage,
    PlantStart, Propagule, Rect, World, WorldConfig, save,
};

const WIDTH: usize = 160;
const HEIGHT: usize = 96;
/// Top row of the aquasoil bed.
const BED: usize = 88;
/// Top row of the rock block standing on the bed, and its columns (centred
/// on column 120).
const ROCK_TOP: usize = 76;
const ROCK_X: (usize, usize) = (100, 141);
/// Ticks in a simulated minute.
const MINUTE: u32 = 1200;

/// A tank with an aquasoil bed and a rock block on it.
fn tank(seed: u64) -> World {
    let mut w = World::new(
        WorldConfig {
            width: WIDTH,
            height: HEIGHT,
            seed,
            water_level: 0.95,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    let soil = id(&w, "aquasoil");
    let rock = id(&w, "rock");
    w.fill_rect(rect(0, BED, WIDTH, HEIGHT), soil, Brush::Replace)
        .unwrap();
    w.fill_rect(
        rect(ROCK_X.0, ROCK_TOP, ROCK_X.1, BED),
        rock,
        Brush::Replace,
    )
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

/// Where to click for a species: over the bed, or the rock for what clings.
fn spot(w: &World, name: &str) -> (i64, i64) {
    match name {
        "java_fern" | "anubias" | "java_moss" | "christmas_moss" => (120, 60),
        "duckweed" | "frogbit" => (60, 40),
        _ => {
            let _ = w;
            (60, 50)
        }
    }
}

fn names(w: &World) -> Vec<String> {
    w.plant_species().iter().map(|s| s.name.clone()).collect()
}

/// The plant table matches the grid: every plant part cell belongs to a
/// plant in the table, whose part counts and bounds are its cells'.
fn check_table(w: &World) {
    let part_mats = [
        (id(w, "root"), Part::Root),
        (id(w, "stem"), Part::Stem),
        (id(w, "leaf"), Part::Leaf),
        (id(w, "runner"), Part::Runner),
        (id(w, "floating_leaf"), Part::Float),
        (id(w, "rhizome"), Part::Rhizome),
    ];
    let mut grid: BTreeMap<u16, ([u32; 3], Rect)> = BTreeMap::new();
    for y in 0..w.height() {
        for x in 0..w.width() {
            let c = w.cell(x, y).unwrap();
            if let Some((_, part)) = part_mats.iter().find(|&&(m, _)| m == c.material) {
                let e = grid.entry(c.state).or_insert(([0; 3], Rect::EMPTY));
                e.0[part.count_slot()] += 1;
                e.1 = e.1.union(rect(x, y, x + 1, y + 1));
            }
        }
    }
    let table: BTreeMap<u16, ([u32; 3], Rect)> =
        w.plants().map(|p| (p.id.0, (p.parts, p.bounds))).collect();
    assert_eq!(grid, table, "tick {}", w.tick_count());
}

/// The cells of plant `plant` as `(x, y)`.
fn cells_of(w: &World, plant: PlantId) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for y in 0..w.height() {
        for x in 0..w.width() {
            if w.plant_at(x, y) == Some(plant) {
                out.push((x, y));
            }
        }
    }
    out
}

/// Whether `cells` are one piece: 4-connected, but for the diagonal steps
/// between the stem cells of a side shoot's sloping stalk.
fn connected(w: &World, cells: &[(usize, usize)]) -> bool {
    let Some(&first) = cells.first() else {
        return true;
    };
    let stem = id(w, "stem");
    let is_stem = |c: (usize, usize)| w.cell(c.0, c.1).unwrap().material == stem;
    let joined = |a: (usize, usize), b: (usize, usize)| {
        let (dx, dy) = (a.0.abs_diff(b.0), a.1.abs_diff(b.1));
        dx + dy == 1 || (dx == 1 && dy == 1 && is_stem(a) && is_stem(b))
    };
    let mut seen = vec![first];
    let mut grew = true;
    while grew {
        grew = false;
        for &c in cells {
            if !seen.contains(&c) && seen.iter().any(|&s| joined(s, c)) {
                seen.push(c);
                grew = true;
            }
        }
    }
    seen.len() == cells.len()
}

fn ghost(w: &World, name: &str, stage: PlantStage, x: i64, y: i64, flip: bool) -> PlantGhost {
    w.plant_placement(species(w, name), stage, x, y, flip)
        .unwrap()
}

/// A ghost's cells as `(x, y, part, position)`, sorted.
fn shape(g: &PlantGhost) -> Vec<(usize, usize, usize, u8)> {
    let mut v: Vec<_> = g
        .cells
        .iter()
        .map(|c| (c.x, c.y, c.part.index(), plant_position(c.variant)))
        .collect();
    v.sort_unstable();
    v
}

fn place(w: &mut World, name: &str, stage: PlantStage, x: i64, y: i64, flip: bool) -> PlantId {
    let s = species(w, name);
    w.place_plant(s, stage, x, y, flip)
        .unwrap()
        .unwrap_or_else(|| panic!("{name} {stage:?} didn't fit at ({x}, {y})"))
}

#[test]
fn mature_epiphytes_fan_out_from_their_rhizomes() {
    let w = tank(7);
    for (name, count) in [("java_fern", 5), ("anubias", 4)] {
        let (x, y) = spot(&w, name);
        for flip in [false, true] {
            let g = ghost(&w, name, PlantStage::Mature, x, y, flip);
            assert!(g.fits, "{name}");
            let mut starts = Vec::new();
            let mut left = g.base.0;
            let mut right = g.base.0;
            for c in g.cells.iter().filter(|c| c.part == Part::Leaf) {
                if plant_position(c.variant) == 1 {
                    starts.push(c.x);
                }
                left = left.min(c.x);
                right = right.max(c.x);
            }
            assert_eq!(starts.len(), count, "{name}");
            assert!(left < *starts.iter().min().unwrap(), "{name} left spread");
            assert!(right > *starts.iter().max().unwrap(), "{name} right spread");
        }
    }
}

#[test]
fn every_species_places_whole_plants_that_are_ordinary_plants() {
    for name in names(&tank(1)) {
        let mut sizes = Vec::new();
        for stage in [PlantStage::Juvenile, PlantStage::Mature] {
            for flip in [false, true] {
                let mut w = tank(3);
                let s = species(&w, &name);
                let (x, y) = spot(&w, &name);
                let want = ghost(&w, &name, stage, x, y, flip);
                assert!(want.fits, "{name} {stage:?} flip={flip} fits");
                assert_ne!(want.cells.len(), 0);
                let plant = place(&mut w, &name, stage, x, y, flip);
                let info = w.plant(plant).unwrap();
                assert_eq!(info.species, s);
                assert_eq!(w.plant_count(), 1, "{name} {stage:?}");
                check_table(&w);
                // The plant is its ghost, cell for cell, base and all.
                assert_eq!(info.size() as usize, want.cells.len(), "{name} {stage:?}");
                assert_eq!(info.base, want.base, "{name} {stage:?} base");
                assert_eq!(w.plant_at(want.base.0, want.base.1), Some(plant));
                for c in &want.cells {
                    let cell = w.cell(c.x, c.y).unwrap();
                    assert_eq!(cell.state, plant.0, "{name}: owner of ({}, {})", c.x, c.y);
                    assert_eq!(cell.variant, c.variant, "{name}: variant");
                    assert_eq!(cell.material, id(&w, c.part.material_name()), "{name}");
                }
                // Within the species' mature size, in one piece.
                let species_size = w.plant_species().get(s).unwrap().growth.size;
                assert!(
                    info.size() <= species_size,
                    "{name} {stage:?}: {}",
                    info.size()
                );
                assert!(
                    connected(&w, &cells_of(&w, plant)),
                    "{name} {stage:?} in pieces"
                );
                if !flip {
                    sizes.push(info.size());
                }
                // It lives on as a normal plant: a few seconds on, the
                // table still matches and the plant is whole and healthy.
                run(&mut w, 200);
                check_table(&w);
                let roots: Vec<_> = w.plants().filter(|p| p.parent.is_none()).collect();
                assert_eq!(roots.len(), 1, "{name} {stage:?} split or died");
                assert!(
                    roots[0].health > 0.9,
                    "{name} {stage:?}: {}",
                    roots[0].health
                );
            }
        }
        assert!(
            sizes[0] < sizes[1] || sizes[0] * 4 >= sizes[1],
            "{name}: juvenile {} vs mature {}",
            sizes[0],
            sizes[1]
        );
        assert!(
            sizes[0] <= sizes[1],
            "{name}: juvenile {} vs mature {}",
            sizes[0],
            sizes[1]
        );
    }
}

#[test]
fn a_mature_plant_is_larger_than_a_juvenile_of_the_same_species() {
    let w = tank(1);
    for name in [
        "rotala",
        "amazon_sword",
        "vallisneria",
        "tiger_lotus",
        "java_fern",
        "marimo",
        "frogbit",
    ] {
        let (x, y) = spot(&w, name);
        let j = ghost(&w, name, PlantStage::Juvenile, x, y, false);
        let m = ghost(&w, name, PlantStage::Mature, x, y, false);
        assert!(
            j.cells.len() * 2 <= m.cells.len() + 4,
            "{name}: juvenile {} vs mature {}",
            j.cells.len(),
            m.cells.len()
        );
    }
}

#[test]
fn a_placed_plant_grows_like_a_grown_one() {
    for (name, stage) in [
        ("hygrophila", PlantStage::Juvenile),
        ("vallisneria", PlantStage::Juvenile),
    ] {
        let mut w = tank(5);
        let (x, y) = spot(&w, name);
        let plant = place(&mut w, name, stage, x, y, false);
        let before = w.plant(plant).unwrap().size();
        run(&mut w, 3 * MINUTE);
        check_table(&w);
        let after = w.plant(plant).map_or(0, |p| p.size());
        assert!(after > before, "{name}: {before} -> {after}");
    }
}

#[test]
fn flipping_mirrors_the_plant_about_the_pointer() {
    let w = tank(2);
    for name in names(&w) {
        for stage in [PlantStage::Juvenile, PlantStage::Mature] {
            let (x, y) = spot(&w, &name);
            let a = ghost(&w, &name, stage, x, y, false);
            let b = ghost(&w, &name, stage, x, y, true);
            assert!(a.fits && b.fits, "{name} {stage:?}");
            // A rosette's first root stays under its first crown cell, so
            // its roots don't mirror.
            let form = &w.plant_species().get(species(&w, &name)).unwrap().form;
            let rooted_rosette = matches!(form, Form::Rosette(_) | Form::Carpet(_));
            let keep = |v: Vec<(usize, usize, usize, u8)>| -> Vec<_> {
                v.into_iter()
                    .filter(|c| !(rooted_rosette && c.2 == Part::Root.index()))
                    .collect()
            };
            let mut mirrored: Vec<_> = shape(&a)
                .into_iter()
                .map(|(cx, cy, p, pos)| ((2 * x - cx as i64) as usize, cy, p, pos))
                .collect();
            mirrored.sort_unstable();
            assert_eq!(keep(shape(&b)), keep(mirrored), "{name} {stage:?} flipped");
        }
    }
}

#[test]
fn placement_is_deterministic_and_steady() {
    let mut w = tank(9);
    let a = ghost(&w, "ludwigia", PlantStage::Mature, 60, 50, false);
    run(&mut w, 100);
    let b = ghost(&w, "ludwigia", PlantStage::Mature, 60, 50, false);
    assert_eq!(a, b, "a resting pointer's ghost doesn't change");
}

#[test]
fn rooted_plants_snap_down_to_the_substrate() {
    let w = tank(1);
    for (px, py) in [
        (60, 10),
        (60, 50),
        (60, BED as i64 - 1),
        (60, BED as i64 + 3),
        (60, 0),
    ] {
        let g = ghost(&w, "rotala", PlantStage::Mature, px, py, false);
        assert!(g.fits, "pointer at ({px}, {py})");
        assert_eq!(g.base, (60, BED - 1), "pointer at ({px}, {py})");
        // Roots in the bed.
        let root = id(&w, "aquasoil");
        for c in g.cells.iter().filter(|c| c.part == Part::Root) {
            assert_eq!(w.cell(c.x, c.y).unwrap().material, root);
        }
    }
    // Deep in the bed (past reach), or on bare rock, or on the glass floor:
    // an ideal ghost at the pointer in red.
    for (px, py) in [
        (60, HEIGHT as i64 - 1),
        (120, ROCK_TOP as i64 + 4),
        (120, 60),
    ] {
        let g = ghost(&w, "rotala", PlantStage::Mature, px, py, false);
        assert!(!g.fits, "pointer at ({px}, {py})");
        assert_ne!(g.cells.len(), 0, "still shows the plant");
    }
    // A crown is centred on the pointer.
    let g = ghost(&w, "amazon_sword", PlantStage::Mature, 60, 50, false);
    let crown: Vec<_> = g
        .cells
        .iter()
        .filter(|c| c.part == Part::Stem)
        .map(|c| c.x)
        .collect();
    assert_eq!(crown, [59, 60, 61]);
}

#[test]
fn a_plant_standing_in_the_way_blocks_a_rooted_spot() {
    let mut w = tank(1);
    place(&mut w, "rotala", PlantStage::Mature, 60, 50, false);
    let g = ghost(&w, "rotala", PlantStage::Mature, 60, 40, false);
    assert!(!g.fits, "the column is taken");
    let s = species(&w, "rotala");
    assert_eq!(
        w.place_plant(s, PlantStage::Mature, 60, 40, false).unwrap(),
        None
    );
    assert_eq!(w.plant_count(), 1);
}

#[test]
fn clinging_plants_snap_to_what_they_cling_to() {
    let w = tank(1);
    let rock = id(&w, "rock");
    for name in ["anubias", "java_fern", "java_moss", "christmas_moss"] {
        // From open water above the rock, and from inside it.
        for (px, py) in [
            (120, 30),
            (120, ROCK_TOP as i64 - 1),
            (120, ROCK_TOP as i64 + 3),
        ] {
            let g = ghost(&w, name, PlantStage::Mature, px, py, false);
            assert!(g.fits, "{name} from ({px}, {py})");
            let (bx, by) = g.base;
            let touches = [(0i64, -1i64), (0, 1), (-1, 0), (1, 0)]
                .into_iter()
                .any(|(dx, dy)| {
                    w.cell((bx as i64 + dx) as usize, (by as i64 + dy) as usize)
                        .is_some_and(|c| c.material == rock)
                });
            assert!(touches, "{name}: base ({bx}, {by}) touches the rock");
            for c in &g.cells {
                assert_eq!(
                    w.cell(c.x, c.y).unwrap().material,
                    id(&w, "water"),
                    "{name}: a cell in open water only"
                );
            }
        }
        // Over the bare bed with no rock below it: nowhere to cling.
        let g = ghost(&w, name, PlantStage::Mature, 30, 50, false);
        assert!(!g.fits, "{name} over bare soil");
        assert_ne!(g.cells.len(), 0);
    }
}

#[test]
fn floaters_snap_to_the_surface() {
    let w = tank(1);
    for name in ["duckweed", "frogbit"] {
        for py in [0, 20, 90] {
            let g = ghost(&w, name, PlantStage::Mature, 70, py, false);
            assert!(g.fits, "{name} from row {py}");
            assert_eq!(g.base, (70, w.surface_row()), "{name}");
            let leaf = g.cells.iter().filter(|c| c.part == Part::Float);
            assert!(leaf.clone().all(|c| c.y == w.surface_row()));
            assert!(leaf.count() >= 2);
        }
    }
}

#[test]
fn marimo_rests_on_the_bed_or_the_rock() {
    let w = tank(1);
    for (px, ground) in [(40, BED), (120, ROCK_TOP)] {
        let g = ghost(&w, "marimo", PlantStage::Mature, px, 30, false);
        assert!(g.fits);
        let bottom = g.cells.iter().map(|c| c.y).max().unwrap();
        assert_eq!(bottom, ground - 1, "its lowest cell rests on the ground");
        // A round ball centred in its column.
        assert_eq!(g.base.0, px as usize);
        assert!(g.cells.len() >= 40);
    }
}

#[test]
fn a_plant_fits_around_what_is_in_the_way() {
    // Rock beside a stem plant clips its leaves, not the plant.
    let mut w = tank(1);
    let rock = id(&w, "rock");
    w.fill_rect(rect(61, 60, 64, BED), rock, Brush::Replace)
        .unwrap();
    let g = ghost(&w, "ludwigia", PlantStage::Mature, 60, 50, false);
    assert!(g.fits);
    let water = id(&w, "water");
    let soil = id(&w, "aquasoil");
    for c in &g.cells {
        let m = w.cell(c.x, c.y).unwrap().material;
        assert!(
            m == water || (c.part == Part::Root && m == soil),
            "{:?} at ({}, {}) is in {}",
            c.part,
            c.x,
            c.y,
            w.materials().get(m).unwrap().name
        );
    }
    let clear = ghost(&tank(1), "ludwigia", PlantStage::Mature, 60, 50, false);
    assert!(g.cells.len() < clear.cells.len());
    place(&mut w, "ludwigia", PlantStage::Mature, 60, 50, false);
    check_table(&w);
}

#[test]
fn a_full_tank_refuses_whole_plants() {
    let mut w = tank(1);
    w.set_plant_cap(1);
    place(&mut w, "rotala", PlantStage::Juvenile, 30, 50, false);
    let g = ghost(&w, "rotala", PlantStage::Juvenile, 90, 50, false);
    assert!(!g.fits, "at the plant cap");
    assert_ne!(g.cells.len(), 0);
    let s = species(&w, "rotala");
    assert_eq!(
        w.place_plant(s, PlantStage::Juvenile, 90, 50, false)
            .unwrap(),
        None
    );
    assert_eq!(w.plant_count(), 1);
    // An unknown species is an error, and has no ghost.
    assert!(
        w.place_plant(PlantSpeciesId(200), PlantStage::Mature, 5, 5, false)
            .is_err()
    );
    assert!(
        w.plant_placement(PlantSpeciesId(200), PlantStage::Mature, 5, 5, false)
            .is_none()
    );
}

#[test]
fn placed_floaters_and_balls_are_rafts_and_stay_one_plant() {
    let mut w = tank(4);
    let weed = place(&mut w, "frogbit", PlantStage::Mature, 60, 3, false);
    let ball = place(&mut w, "marimo", PlantStage::Mature, 40, 20, false);
    // A raft but not an unrooted piece: they don't rot.
    assert!(!w.plant_unrooted(weed) && !w.plant_unrooted(ball));
    run(&mut w, 3 * MINUTE);
    check_table(&w);
    for p in w.plants().filter(|p| p.parent.is_none()) {
        assert!(p.health > 0.9, "{p:?}");
    }
    let weed_cells = cells_of(&w, weed);
    assert!(weed_cells.len() >= 30);
    let surface = w.surface_row();
    let leaf = id(&w, "floating_leaf");
    for &(x, y) in &weed_cells {
        if w.cell(x, y).unwrap().material == leaf {
            assert_eq!(y, surface, "frond at the surface");
        }
    }
    let rest = cells_of(&w, ball);
    assert!(rest.len() >= 40, "{} cells", rest.len());
    assert!(
        rest.iter().map(|c| c.1).max().unwrap() >= BED - 2,
        "settled on the bed"
    );
}

#[test]
fn saved_tanks_round_trip_with_placed_plants() {
    let mut w = tank(6);
    for name in names(&w) {
        let (x, y) = spot(&w, &name);
        let stage = if name.len() % 2 == 0 {
            PlantStage::Juvenile
        } else {
            PlantStage::Mature
        };
        // Spread over the tank so they don't share a column.
        let x = if (40..80).contains(&x) {
            x - 30 + (name.len() as i64 * 3)
        } else {
            x
        };
        let s = species(&w, &name);
        let _ = w.place_plant(s, stage, x, y, name.len() % 3 == 0).unwrap();
    }
    assert!(w.plant_count() >= 10, "{} plants", w.plant_count());
    run(&mut w, MINUTE);
    check_table(&w);
    let mut loaded = save::decode(&save::encode(&w), MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(loaded.state_hash(), w.state_hash());
    for step in 0..2 {
        run(&mut w, MINUTE);
        run(&mut loaded, MINUTE);
        assert_eq!(loaded.state_hash(), w.state_hash(), "minute {step}");
    }
    check_table(&loaded);
}

#[test]
fn a_plant_slot_offers_its_propagules_then_whole_plants() {
    let w = tank(1);
    let (hyg, rotala, moss) = (
        species(&w, "hygrophila"),
        species(&w, "rotala"),
        species(&w, "java_moss"),
    );
    let mut t = Tools::new(id(&w, "sand"));
    assert_eq!(t.plant_starts(&w).len(), 0);
    assert_eq!(t.plant_start(&w), None);
    assert!(!t.set_plant_start(&w, PlantStart::Mature));
    assert_eq!(t.next_plant_start(&w), None);
    t.select(PaletteItem::Tool(Tool::Plant(hyg)));
    assert_eq!(
        t.plant_starts(&w),
        [
            PlantStart::Propagule(Propagule::Seeds),
            PlantStart::Propagule(Propagule::Cuttings),
            PlantStart::Propagule(Propagule::Fragments),
            PlantStart::Juvenile,
            PlantStart::Mature,
        ]
    );
    assert_eq!(
        t.plant_start(&w),
        Some(PlantStart::Propagule(Propagule::Cuttings))
    );
    assert!(t.set_plant_start(&w, PlantStart::Mature));
    assert_eq!(t.plant_start(&w), Some(PlantStart::Mature));
    assert_eq!(t.plant_propagule(&w), None, "a whole plant is no propagule");
    // The pick is the species' own.
    t.select(PaletteItem::Tool(Tool::Plant(rotala)));
    assert_eq!(
        t.plant_starts(&w),
        [
            PlantStart::Propagule(Propagule::Cuttings),
            PlantStart::Juvenile,
            PlantStart::Mature,
        ]
    );
    assert_eq!(t.next_plant_start(&w), Some(PlantStart::Juvenile));
    assert_eq!(t.next_plant_start(&w), Some(PlantStart::Mature));
    assert_eq!(
        t.next_plant_start(&w),
        Some(PlantStart::Propagule(Propagule::Cuttings)),
        "wraps"
    );
    t.select(PaletteItem::Tool(Tool::Plant(moss)));
    assert!(!t.set_plant_start(&w, PlantStart::Propagule(Propagule::Seeds)));
    assert!(t.set_plant_start(&w, PlantStart::Juvenile));
    t.select(PaletteItem::Tool(Tool::Plant(hyg)));
    assert_eq!(t.plant_start(&w), Some(PlantStart::Mature), "remembered");
    // The old propagule calls skip whole plants.
    assert_eq!(t.next_plant_propagule(&w), Some(Propagule::Seeds));
    assert_eq!(
        t.plant_start(&w),
        Some(PlantStart::Propagule(Propagule::Seeds))
    );
    // A host's new tools keep them.
    let mut fresh = Tools::new(id(&w, "sand"));
    fresh.keep_plant_picks(&t);
    fresh.select(PaletteItem::Tool(Tool::Plant(moss)));
    assert_eq!(fresh.plant_start(&w), Some(PlantStart::Juvenile));
    // Names round-trip.
    for s in t.plant_starts(&w) {
        assert_eq!(PlantStart::from_name(s.name()), Some(s));
    }
    assert_eq!(
        PlantStart::from_name("juvenile"),
        Some(PlantStart::Juvenile)
    );
    assert_eq!(PlantStart::from_name("spores"), None);
}

#[test]
fn clicking_places_a_whole_plant_with_a_ghost_and_a_flip() {
    let mut w = tank(2);
    let rotala = species(&w, "rotala");
    let mut t = Tools::new(id(&w, "sand"));
    assert!(!t.can_flip(&w));
    t.select(PaletteItem::Tool(Tool::Plant(rotala)));
    assert!(!t.can_flip(&w), "a cutting doesn't flip");
    assert!(t.plant_ghost(&w).is_none());
    assert!(t.set_plant_start(&w, PlantStart::Mature));
    assert!(t.can_flip(&w), "a whole plant flips");
    assert!(t.plant_ghost(&w).is_none(), "no pointer yet");
    t.moved(&mut w, (60, 30));
    assert_eq!(t.preview(&w), None, "no brush ring");
    let shown = t.plant_ghost(&w).unwrap();
    assert!(shown.fits);
    assert_eq!(shown.stage, PlantStage::Mature);
    assert_eq!(
        shown,
        ghost(&w, "rotala", PlantStage::Mature, 60, 30, false)
    );
    // Flipping shows the mirror image.
    assert!(t.flip());
    let flipped = t.plant_ghost(&w).unwrap();
    assert_eq!(
        flipped,
        ghost(&w, "rotala", PlantStage::Mature, 60, 30, true)
    );
    assert_ne!(shape(&flipped), shape(&shown));
    assert!(!t.flip());
    // One click places it, as the ghost showed; dragging places no more.
    let Outcome::Painted(n) = t.press(&mut w, Button::Primary, (60, 30)) else {
        panic!("nothing placed");
    };
    assert_eq!(n, shown.cells.len());
    assert_eq!(t.moved(&mut w, (90, 30)), Outcome::None);
    assert_eq!(t.hold(&mut w), 0);
    t.release(&mut w, Button::Primary);
    assert_eq!(w.plant_count(), 1);
    check_table(&w);
    // Where it doesn't fit, nothing happens and the ghost is red.
    t.moved(&mut w, (120, ROCK_TOP as i64 + 4));
    assert!(!t.plant_ghost(&w).unwrap().fits);
    assert_eq!(
        t.press(&mut w, Button::Primary, (120, ROCK_TOP as i64 + 4)),
        Outcome::None
    );
    t.release(&mut w, Button::Primary);
    assert_eq!(w.plant_count(), 1);
    // A juvenile is a smaller plant; propagules and seeds are as before.
    assert!(t.set_plant_start(&w, PlantStart::Juvenile));
    t.moved(&mut w, (90, 30));
    let small = t.plant_ghost(&w).unwrap();
    assert!(small.cells.len() < shown.cells.len());
    assert!(t.set_plant_start(&w, PlantStart::Propagule(Propagule::Cuttings)));
    assert!(t.plant_ghost(&w).is_none());
    assert!(!t.can_flip(&w));
}

/// A stem plant's leaf rows, top first, each as `(row, longest leaf length)`,
/// and how many upper cells it has.
fn leaf_rows(w: &World, plant: PlantId) -> (Vec<(usize, u8)>, usize) {
    let leaf = id(w, "leaf");
    let mut longest: std::collections::BTreeMap<usize, u8> = std::collections::BTreeMap::new();
    let mut upper = 0;
    for (cx, cy) in cells_of(w, plant) {
        let c = w.cell(cx, cy).unwrap();
        if c.material != leaf {
            continue;
        }
        let pos = desktank_sim::plants::plant_position(c.variant);
        if pos >= desktank_sim::plants::LEAF_UPPER {
            upper += 1;
            // Over a row cell of the same leaf.
            let below = w.cell(cx, cy + 1).unwrap();
            assert_eq!(below.material, leaf);
            assert_eq!(
                desktank_sim::plants::plant_position(below.variant),
                pos - desktank_sim::plants::LEAF_UPPER
            );
        } else {
            let e = longest.entry(cy).or_default();
            *e = (*e).max(pos);
        }
    }
    (longest.into_iter().collect(), upper)
}

/// Stem leaves taper to the plant's own tip: whatever its age, the leaves
/// are longest low on the stem, never longer than the tier below, one cell
/// long just under a bare top, and the broad ones grow upper cells.
#[test]
fn stem_plants_taper_to_their_own_tip_and_broad_leaves_have_upper_cells() {
    for stage in [PlantStage::Juvenile, PlantStage::Mature] {
        let mut w = tank(3);
        let (x, y) = spot(&w, "hygrophila");
        let plant = place(&mut w, "hygrophila", stage, x, y, false);
        let sp = w.plant_species().get(species(&w, "hygrophila")).unwrap();
        let desktank_sim::plants::Form::Stem(f) = &sp.form else {
            panic!("hygrophila is a stem plant");
        };
        let info = w.plant(plant).unwrap();
        let (rows, upper) = leaf_rows(&w, plant);
        assert!(upper > 0, "{stage:?}: no upper cells");
        // Rows are top first: lengths never shrink going down.
        for pair in rows.windows(2) {
            assert!(pair[0].1 <= pair[1].1, "{stage:?}: {rows:?}");
        }
        assert_eq!(rows.first().unwrap().1, 1, "{stage:?}: {rows:?}");
        assert!(
            rows.first().unwrap().0 >= info.bounds.y0 + usize::from(f.internode),
            "{stage:?}: the top is bare"
        );
        if stage == PlantStage::Mature {
            assert_eq!(rows.last().unwrap().1, f.leaf_length, "{rows:?}");
        }
    }
}

/// A growing stem keeps tapering as it grows: its leaves stay within what
/// their depth below the tip allows and lengthen as the tip moves up.
#[test]
fn a_growing_stem_tapers_and_its_lower_leaves_lengthen() {
    let mut w = tank(3);
    let (x, y) = spot(&w, "hygrophila");
    let plant = place(&mut w, "hygrophila", PlantStage::Juvenile, x, y, false);
    let before = leaf_rows(&w, plant).0;
    run(&mut w, 20 * 60 * 5);
    let info = w.plant(plant).unwrap();
    let (rows, _) = leaf_rows(&w, plant);
    assert!(rows.len() > before.len(), "it grew: {before:?} -> {rows:?}");
    for pair in rows.windows(2) {
        assert!(pair[0].1 <= pair[1].1, "{rows:?}");
    }
    let longest_before = before.iter().map(|r| r.1).max().unwrap();
    let longest_after = rows.iter().map(|r| r.1).max().unwrap();
    assert!(longest_after >= longest_before, "{before:?} -> {rows:?}");
    assert!(info.size() > 0);
}

/// Side shoots lean out: a shoot leaves its node on the diagonal, steps out a column
/// every two rows (diagonal links between stem cells) up to `SHOOT_LEAN`
/// columns past the stub, then rises straight; placed whole or grown, and
/// the plant stays one piece.
#[test]
fn side_shoots_lean_out_then_rise() {
    for name in ["rotala", "ludwigia"] {
        let mut w = tank(5);
        let (x, y) = spot(&w, name);
        let plant = place(&mut w, name, PlantStage::Mature, x, y, false);
        let stem = id(&w, "stem");
        let base_x = w.plant(plant).unwrap().base.0;
        let is_stem = |w: &World, x: usize, y: usize| w.cell(x, y).unwrap().material == stem;
        let check = |w: &World, what: &str| {
            let cells = cells_of(w, plant);
            let mut diagonal = 0;
            for &(cx, cy) in &cells {
                if !is_stem(w, cx, cy) {
                    continue;
                }
                // A diagonal stem link between two cells of a side shoot.
                for dx in [-1isize, 1] {
                    let (nx, ny) = (cx.wrapping_add_signed(dx), cy + 1);
                    if nx < w.width()
                        && ny < w.height()
                        && is_stem(w, nx, ny)
                        && cx != base_x
                        && nx != base_x
                    {
                        diagonal += 1;
                    }
                }
                let reach = cx.abs_diff(base_x);
                let reach = i64::try_from(reach).unwrap();
                assert!(
                    reach <= desktank_sim::plants::SHOOT_LEAN,
                    "{name} {what}: stem {reach} columns from the main stem"
                );
            }
            assert!(diagonal > 0, "{name} {what}: no sloped shoot");
            assert!(connected(w, &cells), "{name} {what} in pieces");
        };
        check(&w, "placed");
        run(&mut w, 5 * 60 * 20);
        check(&w, "grown");
    }
}

/// How many stem cells stand in a plant's base column from its base up.
fn main_stem_height(w: &World, plant: PlantId) -> usize {
    let stem = id(w, "stem");
    let (bx, by) = w.plant(plant).unwrap().base;
    (0..=by)
        .take_while(|k| w.cell(bx, by - k).unwrap().material == stem)
        .count()
}

/// With side shoots on, no shoot ever stops its main stem growing: every main
/// stem of the branching species grows to the species' height, from a placed
/// Juvenile or a cutting, across seeds.
#[test]
fn a_side_shoot_never_stops_the_main_stem_growing() {
    for name in ["rotala", "ludwigia"] {
        for seed in 1..=8 {
            for placed in [true, false] {
                let mut w = tank(seed);
                let (x, y) = spot(&w, name);
                let s = species(&w, name);
                let plant = if placed {
                    place(&mut w, name, PlantStage::Juvenile, x, y, false)
                } else {
                    w.paint_plant(x, y + 1, 0, s, desktank_sim::Propagule::Cuttings)
                        .unwrap();
                    run(&mut w, 20);
                    w.plants().next().map_or(PlantId(0), |p| p.id)
                };
                run(&mut w, 25 * 60 * 20);
                let desktank_sim::plants::Form::Stem(f) = &w.plant_species().get(s).unwrap().form
                else {
                    panic!("{name} is a stem plant");
                };
                let plant = w.plants().next().map_or(plant, |p| p.id);
                let got = main_stem_height(&w, plant);
                let cap = w.plant_species().get(s).unwrap().growth.size;
                let size = w.plant(plant).unwrap().size();
                // Full height, or its cell budget used up (shoots and leaves
                // take cells too): never a main stem held back under a shoot.
                assert!(
                    got == usize::from(f.height) || size + 10 >= cap,
                    "{name} seed {seed} placed {placed}: main stem {got} of {} at size {size} of {cap}",
                    f.height
                );
            }
        }
    }
}
