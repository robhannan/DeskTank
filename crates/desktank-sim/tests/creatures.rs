//! SPEC T7a acceptance tests: creatures never enter solid cells, keep to
//! their zones, find and eat food, replay deterministically, and the net,
//! inspect and add tools work on them.
#![allow(clippy::cast_precision_loss, clippy::many_single_char_names)]

use desktank_sim::tools::{Button, Inspection, Outcome, PaletteItem, Tool, Tools};
use desktank_sim::{
    Brush, Creature, MAX_CREATURES, MaterialId, MaterialTable, MovementClass, Rect, Script,
    SpeciesId, TICKS_PER_SECOND, World, WorldConfig, WorldError,
};

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

fn species(w: &World, name: &str) -> SpeciesId {
    w.species().id(name).unwrap()
}

fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Rect {
    Rect { x0, y0, x1, y1 }
}

fn run(w: &mut World, ticks: u32) {
    for _ in 0..ticks {
        w.tick();
    }
}

/// A sand bed `bed` rows deep along the floor.
fn sand_bed(w: &mut World, bed: usize) {
    let (width, height) = (w.width(), w.height());
    let sand = id(w, "sand");
    w.fill_rect(rect(0, height - bed, width, height), sand, Brush::Replace)
        .unwrap();
}

/// Whether a creature's body may overlap `m`: water, bubbles and passable
/// loose particles. Everything else (rock, substrate, bodies, plants,
/// air) is solid to it.
fn swimmable(w: &World, m: MaterialId) -> bool {
    let m = w.materials().get(m).unwrap();
    match m.class {
        MovementClass::Liquid => m.density >= 0.5,
        MovementClass::Buoyant => true,
        _ => m.passable,
    }
}

/// The first solid cell under `c`'s body, if any.
fn solid_under(w: &World, c: &Creature) -> Option<(usize, usize, String)> {
    let b = w.creature_body(c);
    assert!(!b.is_empty(), "creature {:?} has no body", c.id());
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            let m = w.cell(x, y).unwrap().material;
            if !swimmable(w, m) {
                let name = w.materials().get(m).unwrap().name.clone();
                return Some((x, y, name));
            }
        }
    }
    None
}

/// A busy tank: sand and gravel, an aquasoil patch with cuttings, a glued
/// rock ledge, a free rock resting on the sand, driftwood and creatures of
/// every species.
fn busy_tank(seed: u64) -> World {
    let mut w = world(128, 96, seed);
    sand_bed(&mut w, 10);
    let (gravel, aquasoil, rock) = (id(&w, "gravel"), id(&w, "aquasoil"), id(&w, "rock"));
    w.fill_rect(rect(96, 82, 128, 96), gravel, Brush::Replace)
        .unwrap();
    w.fill_rect(rect(8, 80, 48, 90), aquasoil, Brush::Replace)
        .unwrap();
    // A glued ledge in mid-water and a free rock that drops onto the sand.
    w.paint(70, 45, 7, rock, Brush::Replace).unwrap();
    let ledge = w.body_at(70, 45).unwrap();
    w.set_anchored(ledge, true);
    w.paint(100, 60, 5, rock, Brush::Replace).unwrap();
    w.paint(30, 30, 2, id(&w, "driftwood"), Brush::Replace)
        .unwrap();
    let cutting = id(&w, "cutting");
    for x in [14, 26, 40] {
        w.set_cell(x, 20, cutting).unwrap();
    }
    for (i, name) in ["neon_tetra", "corydoras", "betta"].iter().enumerate() {
        let s = species(&w, name);
        for j in 0..3_i64 {
            let x = 16 + 36 * j + i64::try_from(i).unwrap() * 6;
            w.add_creature(s, x, 50).unwrap();
        }
    }
    w
}

#[test]
fn no_creature_enters_a_solid_cell_in_100k_ticks() {
    let mut w = busy_tank(11);
    let sand = id(&w, "sand");
    let mut moved = vec![0_u32; w.creatures().len()];
    let mut last: Vec<_> = w.creatures().iter().map(Creature::centre).collect();
    for t in 0..100_000_u32 {
        // Now and then something lands on them: sand poured from above.
        if t % 9_000 == 4_000 {
            let x = i64::from(t % 100) + 14;
            w.paint(x, 30, 3, sand, Brush::Pour).unwrap();
        }
        w.tick();
        for (i, c) in w.creatures().iter().enumerate() {
            if let Some((x, y, m)) = solid_under(&w, c) {
                panic!(
                    "tick {}: creature {:?} overlaps {m} at ({x}, {y})",
                    w.tick_count(),
                    c.id()
                );
            }
            if c.centre() != last[i] {
                moved[i] += 1;
                last[i] = c.centre();
            }
        }
    }
    assert_eq!(w.creatures().len(), 9);
    // They swim, not sit stuck: each moved on most ticks.
    for (i, &n) in moved.iter().enumerate() {
        assert!(n > 50_000, "creature {i} moved on only {n} ticks");
    }
    // The plants grew around them.
    let stem = id(&w, "stem");
    assert!(w.material_counts()[usize::from(stem.0)] > 20);
}

#[test]
fn a_creature_buried_by_a_pour_hops_out() {
    let mut w = world(96, 64, 2);
    sand_bed(&mut w, 8);
    let s = species(&w, "neon_tetra");
    let fish = w.add_creature(s, 48, 40).unwrap();
    w.paint(48, 40, 3, id(&w, "sand"), Brush::Replace).unwrap();
    w.tick();
    let c = w.creature(fish).unwrap();
    assert_eq!(solid_under(&w, c), None);
}

/// Fraction of creature-ticks each species spends in its zone over
/// `minutes`, with no food in the tank.
fn zone_fractions(seed: u64, minutes: u32) -> Vec<(String, f64)> {
    let mut w = world(160, 120, seed);
    sand_bed(&mut w, 12);
    w.paint(60, 70, 6, id(&w, "rock"), Brush::Replace).unwrap();
    let ids: Vec<_> = w.species().iter().map(|s| (s.id, s.name.clone())).collect();
    for (k, (s, _)) in ids.iter().enumerate() {
        for j in 0..4_i64 {
            // Start everyone mid-tank, out of most zones.
            let x = 20 + j * 36 + i64::try_from(k).unwrap() * 4;
            w.add_creature(*s, x, 60).unwrap();
        }
    }
    let mut inside = vec![0_u64; 256];
    let mut total = vec![0_u64; 256];
    for _ in 0..minutes * 60 * TICKS_PER_SECOND {
        w.tick();
        for c in w.creatures() {
            let k = usize::from(c.species().0);
            total[k] += 1;
            inside[k] += u64::from(w.in_zone(c));
        }
    }
    ids.into_iter()
        .map(|(s, name)| {
            let k = usize::from(s.0);
            (name, inside[k] as f64 / total[k] as f64)
        })
        .collect()
}

#[test]
fn each_species_spends_most_of_its_time_in_its_zone() {
    for seed in [1, 2] {
        for (name, f) in zone_fractions(seed, 10) {
            eprintln!("seed {seed}: {name} in its zone {:.1}%", f * 100.0);
            assert!(
                f >= 0.7,
                "seed {seed}: {name} in its zone {:.0}%",
                f * 100.0
            );
        }
    }
}

#[test]
fn zones_follow_the_species_data() {
    let w = world(160, 120, 1);
    let s = w.surface_row();
    let water = 120 - s;
    let (top, bottom) = w.zone_rows(species(&w, "betta")).unwrap();
    assert_eq!(top, s);
    assert!(bottom <= s + water / 3 + 1);
    let (top, bottom) = w.zone_rows(species(&w, "corydoras")).unwrap();
    assert!(top > s + water / 2);
    assert_eq!(bottom, 120);
}

/// Ticks until `fish` (of `name`, added at `start`) has eaten one cell of
/// `food` poured at `at`, or `None` within `limit` ticks.
fn ticks_to_eat(
    name: &str,
    start: (i64, i64),
    food: &str,
    at: (i64, i64),
    limit: u32,
) -> Option<u32> {
    let mut w = world(160, 90, 4);
    sand_bed(&mut w, 8);
    let fish = w.add_creature(species(&w, name), start.0, start.1).unwrap();
    // Let it settle into swimming first.
    run(&mut w, 40);
    let f = id(&w, food);
    w.paint(at.0, at.1, 1, f, Brush::Pour).unwrap();
    for t in 1..=limit {
        w.tick();
        if w.creature(fish).unwrap().eaten() > 0 {
            return Some(t);
        }
    }
    None
}

#[test]
fn food_is_found_and_eaten_within_10_seconds() {
    let limit = 10 * TICKS_PER_SECOND;
    let w = world(160, 90, 4);
    let surface = i64::try_from(w.surface_row()).unwrap();
    // Each food, eaten by a species that eats it, dropped 40–50 cells away.
    let cases = [
        ("neon_tetra", (30, 40), "flake_food", (75, surface - 2)),
        ("betta", (120, 10), "flake_food", (75, surface - 2)),
        ("neon_tetra", (110, 45), "bloodworm", (70, 30)),
        ("corydoras", (30, 76), "sinking_pellet", (75, 40)),
        ("corydoras", (120, 76), "algae_wafer", (80, 40)),
    ];
    for (name, start, food, at) in cases {
        let t = ticks_to_eat(name, start, food, at, limit);
        let t = t.unwrap_or_else(|| panic!("{name} didn't eat {food} within 10 s"));
        eprintln!("{name} ate {food} after {:.2} s", f64::from(t) / 20.0);
    }
}

#[test]
fn creatures_eat_until_full_and_only_their_diet() {
    let mut w = world(96, 64, 3);
    sand_bed(&mut w, 6);
    let tetra = w.add_creature(species(&w, "neon_tetra"), 48, 40).unwrap();
    // Wafers sink past the tetra, which doesn't eat them.
    let wafer = id(&w, "algae_wafer");
    w.paint(48, 30, 2, wafer, Brush::Pour).unwrap();
    let wafers = w.material_counts()[usize::from(wafer.0)];
    run(&mut w, 400);
    assert_eq!(w.creature(tetra).unwrap().eaten(), 0);
    assert_eq!(w.material_counts()[usize::from(wafer.0)], wafers);
    // A pile of bloodworms: it eats four bites, then is full.
    w.paint(48, 40, 3, id(&w, "bloodworm"), Brush::Pour)
        .unwrap();
    run(&mut w, 400);
    let c = w.creature(tetra).unwrap();
    assert_eq!(c.eaten(), 4);
    assert!(!c.hungry());
    // 30 s later another bite fits.
    run(&mut w, 30 * TICKS_PER_SECOND + 200);
    assert_eq!(w.creature(tetra).unwrap().eaten(), 5);
}

#[test]
fn swimming_creatures_leave_a_settled_tank_asleep() {
    let mut w = world(128, 96, 5);
    sand_bed(&mut w, 10);
    w.paint(70, 80, 6, id(&w, "rock"), Brush::Replace).unwrap();
    for name in ["neon_tetra", "corydoras", "betta"] {
        let s = species(&w, name);
        for x in [20, 60, 100] {
            w.add_creature(s, x, 50).unwrap();
        }
    }
    run(&mut w, 400);
    // No food: nothing edits a cell, so every chunk sleeps while they swim.
    let before = w.creatures_version();
    for _ in 0..200 {
        w.tick();
        assert_eq!(w.awake_chunks(), 0);
    }
    assert!(w.creatures_version() >= before + 190, "they keep moving");
}

const SCRIPT: &str = r#"
    ticks = 600
    [world]
    width = 128
    height = 80
    [[pour]]
    repeat = 32
    x = 0
    y = 76
    dx = 4
    radius = 4
    material = "sand"
    brush = "replace"
    [[creature]]
    species = "neon_tetra"
    x = 20
    y = 40
    count = 5
    dx = 8
    [[creature]]
    tick = 100
    species = "corydoras"
    x = 60
    y = 60
    count = 2
    [[pour]]
    tick = 200
    x = 64
    y = 10
    radius = 3
    material = "sinking_pellet"
"#;

#[test]
fn same_seed_and_script_give_the_same_state_hash() {
    let s = Script::parse(SCRIPT).unwrap();
    let make = |seed| {
        let mut w = World::new(s.world_config(seed), MaterialTable::builtin().unwrap()).unwrap();
        s.run(&mut w, s.ticks).unwrap();
        w
    };
    let (a, b, c) = (make(9), make(9), make(10));
    assert_eq!(a.creatures().len(), 7);
    assert_eq!(a.state_hash(), b.state_hash());
    assert_eq!(a.creatures(), b.creatures());
    assert_ne!(a.state_hash(), c.state_hash());
    assert!(a.creatures().iter().map(Creature::eaten).sum::<u32>() > 0);
    // The hash covers the creatures: moving one changes it.
    let mut d = make(9);
    let first = d.creatures()[0].id();
    let (x, y) = d.creatures()[0].cell();
    assert!(d.move_creature(first, x + 1, y) || d.move_creature(first, x - 1, y));
    assert_ne!(d.state_hash(), a.state_hash());
}

#[test]
fn the_net_catches_carries_and_lifts_out_a_creature() {
    let mut w = world(96, 64, 6);
    sand_bed(&mut w, 6);
    let fish = w.add_creature(species(&w, "neon_tetra"), 40, 40).unwrap();
    let (x, y) = w.creature(fish).unwrap().cell();
    let mut t = Tools::new(id(&w, "sand"));
    t.select(PaletteItem::Tool(Tool::Net));
    assert_eq!(
        t.press(&mut w, Button::Primary, (x, y)),
        Outcome::Caught(fish)
    );
    // Held: it doesn't swim off while the net holds it.
    for _ in 0..30 {
        t.hold(&mut w);
        w.tick();
    }
    assert_eq!(w.creature(fish).unwrap().cell(), (x, y));
    // Carried with the pointer, keeping the grab offset.
    t.moved(&mut w, (x + 20, y - 5));
    assert_eq!(w.creature(fish).unwrap().cell(), (x + 20, y - 5));
    // Into rock it can't go: it stays where it last fitted.
    w.fill_rect(rect(70, 20, 90, 40), id(&w, "rock"), Brush::Replace)
        .unwrap();
    t.moved(&mut w, (80, 30));
    assert_eq!(w.creature(fish).unwrap().cell(), (x + 20, y - 5));
    // Let go in the water: it swims again.
    assert_eq!(t.release(&mut w, Button::Primary), Outcome::Dropped(fish));
    run(&mut w, 40);
    assert_ne!(w.creature(fish).unwrap().cell(), (x + 20, y - 5));
    // Caught again and lifted above the water: out of the tank.
    let (x, y) = w.creature(fish).unwrap().cell();
    t.press(&mut w, Button::Primary, (x, y));
    t.moved(&mut w, (x, 0));
    assert_eq!(t.release(&mut w, Button::Primary), Outcome::Removed(fish));
    assert!(w.creature(fish).is_none());
    // A net swung through empty water catches nothing and changes nothing.
    let before = w.state_hash();
    assert_eq!(t.press(&mut w, Button::Primary, (40, 40)), Outcome::None);
    assert_eq!(t.release(&mut w, Button::Primary), Outcome::None);
    assert_eq!(w.state_hash(), before);
}

#[test]
fn a_netted_creature_is_let_go_when_the_net_stops_holding_it() {
    let mut w = world(96, 64, 6);
    let fish = w.add_creature(species(&w, "betta"), 40, 20).unwrap();
    let (x, y) = w.creature(fish).unwrap().cell();
    let mut t = Tools::new(id(&w, "sand"));
    t.select(PaletteItem::Tool(Tool::Net));
    t.press(&mut w, Button::Primary, (x, y));
    // The host switches tools mid-drag: the hold lapses.
    t.select(PaletteItem::Tool(Tool::Inspect));
    run(&mut w, 60);
    assert_ne!(w.creature(fish).unwrap().cell(), (x, y));
    // Leaving the tank drops it in the water.
    t.select(PaletteItem::Tool(Tool::Net));
    let (x, y) = w.creature(fish).unwrap().cell();
    assert_eq!(
        t.press(&mut w, Button::Primary, (x, y)),
        Outcome::Caught(fish)
    );
    t.leave(&mut w);
    assert!(w.creature(fish).is_some());
}

#[test]
fn inspect_reports_the_creature_and_the_cell() {
    let mut w = world(96, 64, 6);
    let tetra = species(&w, "neon_tetra");
    let fish = w.add_creature(tetra, 40, 40).unwrap();
    let (x, y) = w.creature(fish).unwrap().cell();
    let water = id(&w, "water");
    let before = w.state_hash();
    let mut t = Tools::new(id(&w, "sand"));
    t.select(PaletteItem::Tool(Tool::Inspect));
    let (ux, uy) = (usize::try_from(x).unwrap(), usize::try_from(y).unwrap());
    assert_eq!(
        t.press(&mut w, Button::Primary, (x, y)),
        Outcome::Inspected(Inspection::Creature {
            id: fish,
            species: tetra,
            x: ux,
            y: uy,
            material: water,
        })
    );
    assert_eq!(
        t.moved(&mut w, (x + 20, y)),
        Outcome::Inspected(Inspection::Cell {
            x: ux + 20,
            y: uy,
            material: water,
        })
    );
    assert_eq!(w.state_hash(), before);
}

#[test]
fn the_palette_adds_creatures_of_each_species() {
    let mut w = world(160, 90, 8);
    sand_bed(&mut w, 8);
    let palette = Tools::palette(&w);
    let adders: Vec<_> = palette
        .iter()
        .filter_map(|&i| match i {
            PaletteItem::Tool(Tool::AddCreature(s)) => Some(s),
            _ => None,
        })
        .collect();
    assert_eq!(adders.len(), w.species().len());
    let mut t = Tools::new(id(&w, "sand"));
    for (k, &s) in adders.iter().enumerate() {
        t.select(PaletteItem::Tool(Tool::AddCreature(s)));
        let x = 30 + 40 * i64::try_from(k).unwrap();
        let Outcome::Added(c) = t.press(&mut w, Button::Primary, (x, 40)) else {
            panic!("no creature added");
        };
        // A drag adds nothing more.
        assert_eq!(t.moved(&mut w, (x + 10, 40)), Outcome::None);
        t.release(&mut w, Button::Primary);
        assert_eq!(w.creature(c).unwrap().species(), s);
    }
    // Deep in the sand there's no room.
    assert_eq!(t.press(&mut w, Button::Primary, (80, 89)), Outcome::None);
    t.release(&mut w, Button::Primary);
    assert_eq!(w.creatures().len(), adders.len());
    // The population cap holds.
    while w.creatures().len() < MAX_CREATURES {
        w.add_creature(adders[0], 80, 40).unwrap();
    }
    assert_eq!(
        w.add_creature(adders[0], 80, 40),
        Err(WorldError::PopulationCap(MAX_CREATURES))
    );
}

#[test]
fn shallow_water_and_a_full_tank_are_flagged() {
    let shallow = World::new(
        WorldConfig {
            width: 64,
            height: 64,
            seed: 1,
            water_level: 0.2,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    let betta = species(&shallow, "betta");
    assert!(matches!(
        shallow.unmet_need(species(&shallow, "neon_tetra")),
        Some(desktank_sim::UnmetNeed::Depth { needs: 24, has: 13 })
    ));
    let full = World::new(
        WorldConfig {
            width: 64,
            height: 64,
            seed: 1,
            water_level: 1.0,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    assert_eq!(full.unmet_need(betta), Some(desktank_sim::UnmetNeed::Air));
    assert_eq!(full.unmet_need(species(&full, "corydoras")), None);
}
