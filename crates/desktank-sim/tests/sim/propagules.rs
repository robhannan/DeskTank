//! Discrete plant propagules (#469, #289): cuttings, fragments, runners
//! and buds are placed as small pieces of a species' real part cells, one
//! per click, not poured as powder. An unrooted piece of a plant that roots
//! or clings drifts and sinks until it roots or clings, or rots; one cut
//! off a plant by an edit does the same. Seeds stay powder, saves with
//! drifting pieces round-trip, and legacy powder cuttings still root.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names
)]

use std::collections::BTreeMap;

use desktank_sim::tools::{Button, Outcome, PaletteItem, Tool, Tools};
use desktank_sim::{
    Brush, MaterialId, MaterialTable, Part, PlantId, PlantInfo, PlantSpeciesId, Propagule, Rect,
    World, WorldConfig, WorldError, save,
};

/// Ticks in a simulated minute.
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

/// A 160-wide tank `height` rows tall with an aquasoil bed `bed` rows deep.
fn bedded(height: usize, bed: usize, seed: u64) -> World {
    let mut w = world(160, height, seed);
    let soil = id(&w, "aquasoil");
    w.fill_rect(rect(0, height - bed, 160, height), soil, Brush::Replace)
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

fn count(w: &World, name: &str) -> usize {
    w.material_counts()[usize::from(id(w, name).0)]
}

fn run(w: &mut World, ticks: u32) {
    for _ in 0..ticks {
        w.tick();
    }
}

/// The plant table matches the grid: every plant part cell of any part
/// belongs to a plant in the table, whose part counts are its cells'.
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

/// The cells of plant `plant`, as `(x, y, material)`.
fn cells_of(w: &World, plant: PlantId) -> Vec<(usize, usize, MaterialId)> {
    let mut out = Vec::new();
    for y in 0..w.height() {
        for x in 0..w.width() {
            if w.plant_at(x, y) == Some(plant) {
                out.push((x, y, w.cell(x, y).unwrap().material));
            }
        }
    }
    out
}

fn place(w: &mut World, x: i64, y: i64, name: &str, p: Propagule) -> PlantId {
    let s = species(w, name);
    w.place_propagule(x, y, s, p)
        .unwrap()
        .unwrap_or_else(|| panic!("{name} {p:?} didn't fit"))
}

#[test]
fn every_species_places_a_recognisable_piece_of_its_own_cells() {
    let probe = world(160, 64, 1);
    let species_list: Vec<_> = probe
        .plant_species()
        .iter()
        .map(|s| (s.id, s.name.clone(), s.propagules().collect::<Vec<_>>()))
        .collect();
    for (sid, name, propagules) in species_list {
        for p in propagules {
            if p == Propagule::Seeds {
                continue;
            }
            let mut w = bedded(64, 8, 7);
            let plant = w
                .place_propagule(80, 30, sid, p)
                .unwrap_or_else(|e| panic!("{name} {p:?}: {e}"))
                .unwrap_or_else(|| panic!("{name} {p:?} didn't fit"));
            let info = w.plant(plant).unwrap();
            assert_eq!(info.species, sid);
            assert_eq!(w.plant_count(), 1, "{name} {p:?}");
            // Real part cells, not powder; the table matches the grid.
            assert_eq!(count(&w, "cutting"), 0, "{name} {p:?} left powder");
            check_table(&w);
            let cells = cells_of(&w, plant);
            assert_eq!(cells.len(), info.size() as usize, "{name} {p:?}");
            let buds = p == Propagule::Buds;
            assert!(
                cells.len() >= if buds { 1 } else { 2 } && cells.len() <= 12,
                "{name} {p:?}: {} cells",
                cells.len()
            );
            // 4-connected, so cutting it never splits it.
            let first = cells[0];
            let mut seen = vec![first];
            let mut grew = true;
            while grew {
                grew = false;
                for &c in &cells {
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
            assert_eq!(seen.len(), cells.len(), "{name} {p:?} is in pieces");
        }
    }
}

#[test]
fn stem_cuttings_and_fragments_have_distinct_starter_shapes() {
    let shape = |p| {
        let mut w = bedded(64, 8, 3);
        let plant = place(&mut w, 80, 30, "hygrophila", p);
        let mut cells: Vec<_> = cells_of(&w, plant)
            .into_iter()
            .map(|(x, y, m)| (x as i64 - 80, y as i64 - 30, m))
            .collect();
        cells.sort_by_key(|c| (c.1, c.0, c.2.0));
        (cells, w.plant(plant).unwrap().parts)
    };
    let (cutting, cp) = shape(Propagule::Cuttings);
    let (fragment, fp) = shape(Propagule::Fragments);
    assert_ne!(cutting, fragment);
    // A cutting is taller and leafier: three stem cells and two pairs of
    // leaves against two and one.
    assert_eq!((cp[1], fp[1]), (3, 2), "stem cells");
    assert!(cp[2] > fp[2] && fp[2] >= 2, "leaves {} vs {}", cp[2], fp[2]);
    assert_eq!(cp[0] + fp[0], 0, "no roots until it roots");
}

#[test]
fn a_placed_cutting_sinks_roots_and_grows_like_any_plant() {
    let mut w = bedded(64, 8, 5);
    let hyg = species(&w, "hygrophila");
    let plant = place(&mut w, 80, 20, "hygrophila", Propagule::Cuttings);
    assert!(w.plant_unrooted(plant));
    let top = cells_of(&w, plant)[0].1;
    run(&mut w, 20);
    assert!(
        cells_of(&w, plant).iter().all(|c| c.1 >= top),
        "it sinks, it doesn't rise"
    );
    // The aquasoil starts at row 56: it falls about 36 rows, gently.
    for _ in 0..3 * MINUTE {
        w.tick();
        if !w.plant_unrooted(plant) {
            break;
        }
    }
    check_table(&w);
    assert!(!w.plant_unrooted(plant), "it never rooted");
    let info = w.plant(plant).unwrap();
    assert_eq!(info.species, hyg);
    assert!(info.parts[0] >= 1, "a root went into the bed");
    let (bx, by) = info.base;
    assert_eq!(w.cell(bx, by).unwrap().material, id(&w, "stem"));
    assert_eq!(w.cell(bx, by + 1).unwrap().material, id(&w, "root"));
    assert!(by > 40, "it settled on the bed, not mid-water: {by}");
    // Rooted, it grows on.
    let size = info.size();
    run(&mut w, 10 * MINUTE);
    check_table(&w);
    let info = w.plant(plant).unwrap();
    assert!(info.size() > size + 10, "{size} -> {}", info.size());
    assert_eq!(count(&w, "cutting"), 0);
}

#[test]
fn a_piece_drifts_with_the_current() {
    // Two identical tanks, one with a filter jetting along the water: its
    // piece is carried further downstream as it sinks.
    let make = |filter: bool| {
        let mut w = bedded(64, 8, 9);
        if filter {
            let row = w.surface_row() as i64 + 15;
            assert!(
                w.add_device(desktank_sim::DeviceKind::InternalFilter, 2, row, false)
                    .unwrap()
                    .is_some()
            );
        }
        let plant = place(&mut w, 30, 20, "rotala", Propagule::Cuttings);
        run(&mut w, MINUTE / 2);
        (w, plant)
    };
    let ((still, a), (stirred, b)) = (make(false), make(true));
    check_table(&still);
    check_table(&stirred);
    let (x_still, x_stirred) = (
        still.plant(a).unwrap().bounds.x0,
        stirred.plant(b).unwrap().bounds.x0,
    );
    assert!(
        x_still.abs_diff(30) <= 2,
        "no current: it sinks straight down, not {x_still}"
    );
    assert!(
        x_stirred >= x_still + 10,
        "the current carried it: {x_still} vs {x_stirred}"
    );
    assert!(stirred.plant_unrooted(b) || stirred.plant(b).is_some());
}

#[test]
fn an_unrooted_piece_with_nowhere_to_root_rots_in_seven_and_a_half_minutes() {
    // A rock floor: nothing a stem roots in.
    let mut w = world(96, 48, 4);
    let rock = id(&w, "rock");
    w.fill_rect(rect(0, 40, 96, 48), rock, Brush::Replace)
        .unwrap();
    let body = w.body_at(10, 40).unwrap();
    assert!(w.set_anchored(body, true));
    let plant = place(&mut w, 48, 20, "rotala", Propagule::Cuttings);
    run(&mut w, 3 * MINUTE);
    assert!(w.plant_unrooted(plant), "no substrate to root in");
    let early = w.plant(plant).unwrap().health;
    assert!(early < 0.7, "health falls while it is loose: {early}");
    check_table(&w);
    run(&mut w, 5 * MINUTE);
    assert_eq!(w.plant_count(), 0, "it rotted");
    assert!(count(&w, "dead_plant") > 0);
    assert_eq!(count(&w, "stem") + count(&w, "leaf"), 0);
    check_table(&w);
}

#[test]
fn epiphyte_and_moss_pieces_sink_onto_rock_and_cling() {
    for (name, grows) in [("anubias", "rhizome"), ("java_moss", "stem")] {
        let mut w = bedded(80, 8, 6);
        let rock = id(&w, "rock");
        w.fill_rect(rect(60, 60, 100, 72), rock, Brush::Replace)
            .unwrap();
        let body = w.body_at(60, 60).unwrap();
        assert!(w.set_anchored(body, true));
        let plant = place(&mut w, 80, 20, name, Propagule::Fragments);
        assert!(w.plant_unrooted(plant), "{name}");
        run(&mut w, 4 * MINUTE);
        check_table(&w);
        assert!(!w.plant_unrooted(plant), "{name} never clung");
        let cells = cells_of(&w, plant);
        let grows = id(&w, grows);
        let touches_rock = |x: usize, y: usize| {
            [(0isize, 1isize), (0, -1), (1, 0), (-1, 0)]
                .into_iter()
                .any(|(dx, dy)| {
                    x.checked_add_signed(dx)
                        .zip(y.checked_add_signed(dy))
                        .and_then(|(nx, ny)| w.cell(nx, ny))
                        .is_some_and(|c| c.material == rock)
                })
        };
        assert!(
            cells
                .iter()
                .any(|&(x, y, m)| m == grows && touches_rock(x, y)),
            "{name} rests on the rock"
        );
        // Clinging, it stays and grows.
        run(&mut w, 8 * MINUTE);
        check_table(&w);
        let info = w.plant(plant).expect("it lives on");
        assert!(info.size() > cells.len() as u32, "{name} grew");
    }
}

#[test]
fn a_piece_cut_off_by_the_scoop_is_loose_sinks_and_roots_or_rots() {
    let mut w = bedded(64, 8, 12);
    let hyg = species(&w, "hygrophila");
    w.paint_plant(80, 30, 0, hyg, Propagule::Cuttings).unwrap();
    run(&mut w, 12 * MINUTE);
    let whole = w.plants().next().unwrap();
    assert!(!w.plant_unrooted(whole.id));
    assert!(whole.size() > 20, "{}", whole.size());
    // Scoop a gap through the middle of the stem: the top comes away.
    let mut tools = Tools::new(id(&w, "sand"));
    tools.select(PaletteItem::Tool(Tool::Scoop));
    tools.resize(-i32::try_from(tools.radius()).unwrap() + 1);
    let (bx, by) = whole.base;
    tools.press(&mut w, Button::Primary, (bx as i64, by as i64 - 8));
    tools.release(&mut w, Button::Primary);
    run(&mut w, 2);
    check_table(&w);
    // What is left rooted stays rooted; the cut-off top is a loose piece.
    assert!(!w.plant_unrooted(whole.id));
    let loose: Vec<PlantInfo> = w.plants().filter(|p| w.plant_unrooted(p.id)).collect();
    assert!(!loose.is_empty(), "the cut-off top is a loose piece");
    let top = loose[0];
    assert_eq!(top.parent, Some(whole.id));
    let y0 = top.bounds.y1;
    run(&mut w, 200);
    assert!(w.plant(top.id).unwrap().bounds.y1 > y0, "it sinks");
    check_table(&w);
    // It comes to rest on its mother's stump, where it can't root: it is
    // one or the other within the rot time, never loose for ever.
    run(&mut w, 9 * MINUTE);
    check_table(&w);
    assert!(
        w.plant(top.id).is_none_or(|p| !w.plant_unrooted(p.id)),
        "{:?}",
        w.plant(top.id)
    );
    assert!(w.plants().all(|p| !w.plant_unrooted(p.id)));
}

#[test]
fn a_bud_and_a_floater_fragment_float_to_the_surface() {
    let mut w = bedded(64, 8, 2);
    let bud = place(&mut w, 60, 40, "duckweed", Propagule::Buds);
    let frag = place(&mut w, 100, 40, "frogbit", Propagule::Fragments);
    // Floaters are rafts already, but are not "unrooted pieces".
    assert!(!w.plant_unrooted(bud) && !w.plant_unrooted(frag));
    assert_eq!(w.plant(bud).unwrap().size(), 1);
    assert_eq!(w.plant(frag).unwrap().parts, [1, 0, 2]);
    run(&mut w, MINUTE);
    check_table(&w);
    let surface = w.surface_row();
    assert!(w.plant(bud).unwrap().bounds.y0 <= surface + 1);
    assert!(w.plant(frag).unwrap().bounds.y0 <= surface + 1);
}

#[test]
fn a_marimo_fragment_is_a_small_ball_that_rests_on_the_bed() {
    let mut w = bedded(64, 8, 2);
    let ball = place(&mut w, 80, 20, "marimo", Propagule::Fragments);
    assert!(!w.plant_unrooted(ball));
    assert_eq!(w.plant(ball).unwrap().size(), 5);
    run(&mut w, 3 * MINUTE);
    check_table(&w);
    assert!(w.plant(ball).unwrap().bounds.y1 >= 54, "it sank to the bed");
    assert_eq!(count(&w, "cutting"), 0);
}

#[test]
fn placing_refuses_what_it_should_and_never_exceeds_the_cap() {
    let mut w = bedded(64, 8, 1);
    let (hyg, rotala, moss) = (
        species(&w, "hygrophila"),
        species(&w, "rotala"),
        species(&w, "java_moss"),
    );
    // Seeds are poured, and rotala doesn't grow from fragments.
    assert!(matches!(
        w.place_propagule(80, 20, hyg, Propagule::Seeds),
        Err(WorldError::PouredPropagule(Propagule::Seeds))
    ));
    assert!(matches!(
        w.place_propagule(80, 20, rotala, Propagule::Fragments),
        Err(WorldError::PlantPropagule(..))
    ));
    assert!(matches!(
        w.place_propagule(80, 20, PlantSpeciesId(200), Propagule::Cuttings),
        Err(WorldError::UnknownPlantSpecies(_))
    ));
    // Nothing placed in air, in the substrate or off the grid.
    for (x, y) in [(80, 1), (80, 60), (-5, 20), (80, 100), (0, 20), (159, 20)] {
        assert_eq!(
            w.place_propagule(x, y, hyg, Propagule::Cuttings).unwrap(),
            None,
            "({x}, {y})"
        );
    }
    assert_eq!(w.plant_count(), 0);
    // A piece doesn't overlap another plant.
    assert!(
        w.place_propagule(80, 20, moss, Propagule::Fragments)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        w.place_propagule(80, 20, moss, Propagule::Fragments)
            .unwrap(),
        None
    );
    // And a full tank takes no more.
    w.set_plant_cap(1);
    assert_eq!(
        w.place_propagule(30, 20, hyg, Propagule::Cuttings).unwrap(),
        None
    );
    assert_eq!(w.plant_count(), 1);
}

#[test]
fn a_click_places_one_piece_and_dragging_places_no_more() {
    let mut w = bedded(64, 8, 1);
    let hyg = species(&w, "hygrophila");
    let mut tools = Tools::new(id(&w, "sand"));
    tools.select(PaletteItem::Tool(Tool::Plant(hyg)));
    assert_eq!(tools.plant_propagule(&w), Some(Propagule::Cuttings));
    // Cuttings place a piece, so no brush ring.
    tools.moved(&mut w, (60, 20));
    assert_eq!(tools.preview(&w), None);
    let Outcome::Painted(cells) = tools.press(&mut w, Button::Primary, (80, 20)) else {
        panic!("nothing placed");
    };
    assert!(cells >= 5);
    assert_eq!(tools.moved(&mut w, (90, 20)), Outcome::None);
    assert_eq!(tools.hold(&mut w), 0);
    tools.release(&mut w, Button::Primary);
    assert_eq!(w.plant_count(), 1);
    assert_eq!(count(&w, "cutting"), 0);
    // Where it doesn't fit, nothing happens.
    assert_eq!(tools.press(&mut w, Button::Primary, (80, 2)), Outcome::None);
    tools.release(&mut w, Button::Primary);
    assert_eq!(w.plant_count(), 1);
    // Seeds still pour with the brush, which shows its ring.
    assert!(tools.set_plant_propagule(&w, Propagule::Seeds));
    tools.moved(&mut w, (60, 20));
    assert!(tools.preview(&w).is_some());
    assert!(matches!(
        tools.press(&mut w, Button::Primary, (120, 20)),
        Outcome::Painted(n) if n > 1
    ));
    tools.release(&mut w, Button::Primary);
    assert!(count(&w, "seed") > 1);
    assert_eq!(w.plant_count(), 1);
}

#[test]
fn saves_with_drifting_pieces_round_trip_and_replay_exactly() {
    let mut w = bedded(64, 8, 21);
    place(&mut w, 40, 10, "hygrophila", Propagule::Cuttings);
    place(&mut w, 70, 12, "rotala", Propagule::Cuttings);
    place(&mut w, 100, 8, "amazon_sword", Propagule::Runners);
    place(&mut w, 120, 14, "java_moss", Propagule::Fragments);
    // A legacy powder cutting too.
    let hyg = species(&w, "hygrophila");
    w.paint_plant(20, 20, 0, hyg, Propagule::Cuttings).unwrap();
    run(&mut w, 15);
    let loose = w.plants().filter(|p| w.plant_unrooted(p.id)).count();
    assert!(loose >= 3, "{loose} loose pieces");
    let mut loaded = save::decode(&save::encode(&w), MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(loaded.state_hash(), w.state_hash());
    assert_eq!(
        loaded
            .plants()
            .filter(|p| loaded.plant_unrooted(p.id))
            .count(),
        loose
    );
    for step in 0..6 {
        run(&mut w, MINUTE);
        run(&mut loaded, MINUTE);
        assert_eq!(loaded.state_hash(), w.state_hash(), "minute {step}");
    }
    check_table(&loaded);
    // The powder cutting rooted too.
    assert_eq!(count(&loaded, "cutting"), 0);
}

#[test]
fn legacy_powder_cuttings_still_root_and_rot() {
    let mut w = bedded(64, 8, 17);
    let hyg = species(&w, "hygrophila");
    w.paint_plant(80, 40, 1, hyg, Propagule::Cuttings).unwrap();
    assert!(count(&w, "cutting") > 1);
    // A cutting that doesn't root turns to dead matter within one in-tank
    // day (ten minutes); the last of them can take more than eight.
    run(&mut w, 10 * MINUTE);
    assert_eq!(count(&w, "cutting"), 0);
    assert!(w.plant_count() >= 1);
    check_table(&w);
    // And with nothing to root in they rot.
    let mut bare = world(64, 48, 17);
    let rock = id(&bare, "rock");
    bare.fill_rect(rect(0, 40, 64, 48), rock, Brush::Replace)
        .unwrap();
    bare.paint_plant(32, 20, 1, hyg, Propagule::Cuttings)
        .unwrap();
    run(&mut bare, 10 * MINUTE);
    assert_eq!(count(&bare, "cutting"), 0);
    assert_eq!(bare.plant_count(), 0);
}

#[test]
fn placing_is_deterministic() {
    let play = || {
        let mut w = bedded(64, 8, 33);
        place(&mut w, 50, 10, "ludwigia", Propagule::Fragments);
        place(&mut w, 90, 10, "cryptocoryne", Propagule::Runners);
        place(&mut w, 120, 10, "monte_carlo", Propagule::Fragments);
        run(&mut w, 6 * MINUTE);
        w.state_hash()
    };
    assert_eq!(play(), play());
}

#[test]
fn rosette_and_carpet_plantlets_root_their_crown_in_the_bed() {
    for (name, p) in [
        ("amazon_sword", Propagule::Runners),
        ("vallisneria", Propagule::Runners),
        ("monte_carlo", Propagule::Fragments),
        ("dwarf_hairgrass", Propagule::Runners),
    ] {
        let mut w = bedded(64, 8, 14);
        let plant = place(&mut w, 80, 30, name, p);
        assert!(w.plant_unrooted(plant), "{name}");
        run(&mut w, 3 * MINUTE);
        check_table(&w);
        assert!(!w.plant_unrooted(plant), "{name} never rooted");
        let info = w.plant(plant).unwrap();
        let (bx, by) = info.base;
        let c = w.cell(bx, by).unwrap();
        assert_eq!(c.material, id(&w, "stem"), "{name}: its crown");
        assert_eq!(
            w.cell(bx, by + 1).unwrap().material,
            id(&w, "root"),
            "{name}: its first root"
        );
        assert_eq!(by, 55, "{name} sits on top of the bed");
    }
}

#[test]
fn uprooting_a_stem_plant_turns_the_remaining_shoot_into_a_drifting_piece() {
    let mut w = bedded(64, 8, 21);
    let plant = place(&mut w, 80, 50, "rotala", Propagule::Cuttings);
    run(&mut w, 4 * MINUTE);
    assert!(!w.plant_unrooted(plant), "should be rooted first");
    let before_y1 = w.plant(plant).unwrap().bounds.y1;
    // Scoop the substrate and roots beneath the stem, leaving the upper shoot
    // unanchored in open water.
    let water = id(&w, "water");
    w.fill_rect(rect(70, 54, 90, 64), water, Brush::Replace)
        .unwrap();
    w.tick();
    assert!(
        w.plant_unrooted(plant),
        "keeper piece without roots should become an unrooted raft"
    );
    let after_scoop_y1 = w.plant(plant).unwrap().bounds.y1;
    run(&mut w, 30 * 20);
    let sunk_y1 = w.plant(plant).unwrap().bounds.y1;
    assert!(
        sunk_y1 > after_scoop_y1 && sunk_y1 >= before_y1,
        "uprooted keeper piece should sink rather than hang frozen (after_scoop={after_scoop_y1}, sunk={sunk_y1})"
    );
    check_table(&w);
}
