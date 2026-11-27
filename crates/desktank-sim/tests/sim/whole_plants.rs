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

/// Whether `cells` are one 4-connected piece.
fn connected(cells: &[(usize, usize)]) -> bool {
    let Some(&first) = cells.first() else {
        return true;
    };
    let mut seen = vec![first];
    let mut grew = true;
    while grew {
        grew = false;
        for &c in cells {
            if !seen.contains(&c)
                && seen
                    .iter()
                    .any(|s| s.0.abs_diff(c.0) + s.1.abs_diff(c.1) == 1)
            {
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
                    connected(&cells_of(&w, plant)),
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

/// Stem leaves taper: a mature hygrophila's leaves are longest low on the
/// stem and shortest at the top, as `StemForm::leaf_taper` says, and its
/// broad leaves grow upper cells at every other node, which don't join the
/// leaf above them.
#[test]
fn a_mature_stem_plant_tapers_and_broad_leaves_have_upper_cells() {
    let mut w = tank(3);
    let (x, y) = spot(&w, "hygrophila");
    let plant = place(&mut w, "hygrophila", PlantStage::Mature, x, y, false);
    let cells = cells_of(&w, plant);
    let sp = w.plant_species().get(species(&w, "hygrophila")).unwrap();
    let desktank_sim::plants::Form::Stem(f) = &sp.form else {
        panic!("hygrophila is a stem plant");
    };
    let leaf = id(&w, "leaf");
    let stem = id(&w, "stem");
    let at = |x: usize, y: usize| w.cell(x, y).unwrap();
    // The longest leaf (row cells only) at each node row.
    let mut longest: std::collections::BTreeMap<usize, u8> = std::collections::BTreeMap::new();
    let mut upper = 0;
    for &(cx, cy) in &cells {
        let c = at(cx, cy);
        if c.material != leaf {
            continue;
        }
        let pos = desktank_sim::plants::plant_position(c.variant);
        if pos >= desktank_sim::plants::LEAF_UPPER {
            upper += 1;
            // Over a row cell of the same leaf.
            let below = at(cx, cy + 1);
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
    assert!(upper > 0, "no upper cells");
    let rows: Vec<(usize, u8)> = longest.into_iter().collect();
    // Rows count up from the top: the lowest leaves are the longest.
    let (top, bottom) = (rows.first().unwrap().1, rows.last().unwrap().1);
    assert_eq!(bottom, f.leaf_length);
    assert!(top < bottom, "top leaves {top}, bottom {bottom}");
    // Each node's length is what the taper gives at its stem height.
    for &(cx, cy) in &cells {
        let c = at(cx, cy);
        if c.material != stem
            || (desktank_sim::plants::plant_position(c.variant)) % f.internode != 0
        {
            continue;
        }
        let pos = desktank_sim::plants::plant_position(c.variant);
        if let Some(&(_, len)) = rows.iter().find(|r| r.0 == cy) {
            assert!(
                len <= desktank_sim::plants::StemForm::leaf_taper(f.leaf_length, f.height, pos),
                "node {pos}: {len}"
            );
        }
    }
}
