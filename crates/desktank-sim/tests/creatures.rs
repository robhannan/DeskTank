//! SPEC T7 acceptance tests: creatures never enter solid cells, keep to
//! their zones, find and eat food, replay deterministically, and the net,
//! inspect and add tools work on them.
#![allow(clippy::cast_precision_loss, clippy::many_single_char_names)]

use desktank_sim::tools::{Button, Inspection, Outcome, PaletteGroup, PaletteItem, Tool, Tools};
use desktank_sim::{
    BAND_REACH, Brush, Creature, DEFAULT_CREATURE_CAP, MaterialId, MaterialTable, MovementClass,
    Rect, Script, SpeciesId, TICKS_PER_SECOND, Traits, World, WorldConfig, WorldError,
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

/// Whether a creature's body may overlap `m` in any band: water, bubbles
/// and passable loose particles. Everything else (rock, substrate, bodies,
/// plants, air) is solid to it, plants only near its band ([`blocks`]).
fn swimmable(w: &World, m: MaterialId) -> bool {
    let m = w.materials().get(m).unwrap();
    match m.class {
        MovementClass::Liquid => m.density >= 0.5,
        MovementClass::Buoyant => true,
        _ => m.passable,
    }
}

/// Whether the cell at `(x, y)` blocks a creature in `band`: anything not
/// swimmable, except a plant cell more than [`BAND_REACH`] bands away
/// (D21).
fn blocks(w: &World, x: usize, y: usize, band: u8) -> bool {
    let m = w.cell(x, y).unwrap().material;
    !swimmable(w, m)
        && w.plant_band(x, y)
            .is_none_or(|p| p.abs_diff(band) <= BAND_REACH)
}

/// The first cell under `c`'s body that blocks it, if any.
fn solid_under(w: &World, c: &Creature) -> Option<(usize, usize, String)> {
    let b = w.creature_body(c);
    assert!(!b.is_empty(), "creature {:?} has no body", c.id());
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            let m = w.cell(x, y).unwrap().material;
            if blocks(w, x, y, c.band()) {
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
    for (i, name) in [
        "neon_tetra",
        "corydoras",
        "betta",
        "nerite",
        "cherry_shrimp",
        "amano_shrimp",
        "thai_micro_crab",
        "honey_gourami",
        "guppy",
        "harlequin_rasbora",
        "otocinclus",
    ]
    .iter()
    .enumerate()
    {
        let s = species(&w, name);
        for j in 0..3_i64 {
            let x = 12 + 36 * j + i64::try_from(i).unwrap() * 3;
            // All males in easy mode: nothing breeds or is eaten, so the
            // same 33 swim throughout.
            let male = Traits {
                female: Some(false),
                ..Traits::default()
            };
            w.add_creature_with_traits(s, x, 50, male).unwrap();
        }
    }
    w.set_easy_mode(true);
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
    assert_eq!(w.creatures().len(), 33);
    // They swim, not sit stuck: each moved on most ticks. Crawlers take a
    // step every 7 (otocinclus), 10 (nerite) or 13 (crab) ticks.
    for (i, &n) in moved.iter().enumerate() {
        let s = w.creatures()[i].species();
        let min = if s == species(&w, "nerite") || s == species(&w, "otocinclus") {
            8_000
        } else if s == species(&w, "thai_micro_crab") {
            6_000
        } else {
            50_000
        };
        assert!(n > min, "creature {i} moved on only {n} ticks");
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
    // Wide enough that two of every species (30, several of them large)
    // aren't crowded out of their zones.
    let mut w = world(220, 120, seed);
    // Easy mode: predators hunting the small species down would leave
    // them too few ticks to count.
    w.set_easy_mode(true);
    sand_bed(&mut w, 12);
    w.paint(60, 70, 6, id(&w, "rock"), Brush::Replace).unwrap();
    // Live food is eaten even in easy mode (tests/live_food.rs).
    let ids: Vec<_> = w
        .species()
        .iter()
        .filter(|s| !s.live_food)
        .map(|s| (s.id, s.name.clone()))
        .collect();
    for (k, (s, _)) in ids.iter().enumerate() {
        // Two of each fit under the population cap, but one blinker (30×22 cells).
        let one = w.species().get(*s).unwrap().name == "blinker";
        for j in 0..if one { 1 } else { 2_i64 } {
            // Start everyone mid-tank, out of most zones.
            let x = 20 + j * 36 + i64::try_from(k).unwrap() * 2;
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
        ("honey_gourami", (120, 20), "flake_food", (75, surface - 2)),
        ("guppy", (30, 30), "flake_food", (75, surface - 2)),
        (
            "harlequin_rasbora",
            (110, 45),
            "flake_food",
            (70, surface - 2),
        ),
        ("harlequin_rasbora", (30, 45), "bloodworm", (70, 30)),
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
    [[creature]]
    species = "betta"
    x = 20
    y = 25
    [[creature]]
    species = "nerite"
    x = 40
    y = 65
    [[pour]]
    tick = 300
    x = 60
    y = 72
    radius = 2
    material = "detritus"
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
    assert_eq!(a.creatures().len(), 9);
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
    let mut w = world(360, 90, 8);
    sand_bed(&mut w, 8);
    let palette = Tools::palette(&w);
    let adders: Vec<_> = palette
        .iter()
        .filter_map(|&i| match i {
            PaletteItem::Tool(Tool::AddCreature(s)) if w.species().get(s)?.stocked() => Some(s),
            _ => None,
        })
        .collect();
    // The Creatures group holds only stocked species (no live food).
    let (_, group) = Tools::palette_groups(&w)
        .into_iter()
        .find(|(g, _)| *g == PaletteGroup::Creatures)
        .unwrap();
    assert!(group.iter().all(|&i| matches!(
        i,
        PaletteItem::Tool(Tool::AddCreature(s)) if w.species().get(s).is_some_and(desktank_sim::Species::stocked)
    )));
    // Every species but the cheat-only crabs and blinker, and live food
    // (its slot is in the Food group: tests/live_food.rs).
    let stocked = w.species().iter().filter(|s| s.stocked()).count();
    let live = w.species().iter().filter(|s| s.live_food).count();
    assert_eq!(
        (adders.len(), w.species().len()),
        (stocked, stocked + 3 + live)
    );
    let mut t = Tools::new(id(&w, "sand"));
    for (k, &s) in adders.iter().enumerate() {
        t.select(PaletteItem::Tool(Tool::AddCreature(s)));
        let x = 12 + 13 * i64::try_from(k).unwrap();
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
    while w.creatures().len() < DEFAULT_CREATURE_CAP {
        w.add_creature(adders[0], 80, 40).unwrap();
    }
    assert_eq!(
        w.add_creature(adders[0], 80, 40),
        Err(WorldError::PopulationCap(DEFAULT_CREATURE_CAP))
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

#[test]
fn dispersed_tetras_form_a_school() {
    for seed in [4, 77] {
        let mut w = world(320, 120, seed);
        let tetra = species(&w, "neon_tetra");
        let lengths = 3.0 * w.species().get(tetra).unwrap().size().0 as f32;
        for i in 0..8 {
            w.add_creature(tetra, 15 + 40 * i, 45 + (i % 3) * 14)
                .unwrap();
        }
        let mut together = 0;
        let ticks = 6_000;
        for _ in 0..ticks {
            w.tick();
            let nearest = |fish: &Creature| {
                let (x, y) = fish.centre();
                w.creatures()
                    .iter()
                    .filter(|c| c.id() != fish.id())
                    .map(|c| {
                        let (mx, my) = c.centre();
                        (mx - x).hypot(my - y)
                    })
                    .fold(f32::INFINITY, f32::min)
            };
            let mean = w.creatures().iter().map(nearest).sum::<f32>() / w.creatures().len() as f32;
            together += u32::from(mean <= lengths);
        }
        eprintln!("seed {seed}: close school on {together}/{ticks} ticks");
        assert!(together * 5 >= ticks * 4);
    }
}

#[test]
fn walker_follows_a_stepped_bed() {
    let mut w = world(112, 80, 7);
    sand_bed(&mut w, 5);
    // A stable one-row ledge on the bed.
    let rock = id(&w, "rock");
    w.fill_rect(rect(55, 74, 112, 75), rock, Brush::Replace)
        .unwrap();
    w.set_anchored(w.body_at(60, 74).unwrap(), true);
    let fish = w.add_creature(species(&w, "corydoras"), 20, 42).unwrap();
    run(&mut w, 300);
    let mut contacts = 0;
    let mut range = (112, 0);
    for _ in 0..3_000 {
        w.tick();
        let c = w.creature(fish).unwrap();
        let b = w.creature_body(c);
        contacts +=
            u32::from((b.x0..b.x1).any(|x| !swimmable(&w, w.cell(x, b.y1).unwrap().material)));
        range = (range.0.min(c.cell().0), range.1.max(c.cell().0));
    }
    assert!(contacts > 2_900, "on the substrate {contacts}/3000 ticks");
    assert!(
        range.0 < 45 && range.1 > 65,
        "did not cross the ledge: {range:?}"
    );
}

#[test]
fn air_breathers_revisit_air_and_skim_for_flakes() {
    for name in ["betta", "honey_gourami"] {
        air_visits_and_skims(name);
    }
}

fn air_visits_and_skims(name: &str) {
    let mut w = world(128, 100, 19);
    let surface = w.surface_row();
    let fish = w.add_creature(species(&w, name), 60, 50).unwrap();
    let mut visits = 0;
    let mut away = true;
    let mut skimmed = false;
    let mut ate_at_surface = false;
    let mut start_x = 0;
    for _ in 0..2_400 {
        w.tick();
        let c = w.creature(fish).unwrap();
        let b = w.creature_body(c);
        if b.y0 > surface + 2 {
            away = true;
        }
        if b.y0 == surface {
            let x = c.cell().0;
            ate_at_surface |= c.eaten() == 1;
            if away {
                visits += 1;
                away = false;
                start_x = x;
                if visits == 1 {
                    w.set_cell(b.x1 + 4, surface, id(&w, "flake_food")).unwrap();
                }
            }
            skimmed |= (x - start_x).abs() >= 4;
        }
    }
    assert!(
        visits >= 3,
        "{name}: air visits with a dive between: {visits}"
    );
    assert!(skimmed, "{name} skimmed the surface");
    assert!(ate_at_surface, "{name} ate at the surface");
}

#[test]
fn nerite_traces_the_faces_of_a_submerged_rock() {
    let mut w = world(100, 80, 7);
    w.fill_rect(rect(35, 35, 65, 50), id(&w, "rock"), Brush::Replace)
        .unwrap();
    w.set_anchored(w.body_at(44, 42).unwrap(), true);
    let snail = w.add_creature(species(&w, "nerite"), 47, 28).unwrap();
    let mut faces = [false; 4];
    for _ in 0..1_800 {
        w.tick();
        let c = w.creature(snail).unwrap();
        assert_eq!(solid_under(&w, c), None);
        let (x, y) = c.cell();
        // The 5×3 shell turns to 3×5 on the sides, foot on the rock.
        let turn = c.turn();
        faces[0] |= (35..65).contains(&x) && y == 33 && turn == 0;
        faces[1] |= (35..50).contains(&y) && x == 66 && turn == 1;
        faces[2] |= (35..65).contains(&x) && y == 51 && turn == 2;
        faces[3] |= (35..50).contains(&y) && x == 33 && turn == 3;
    }
    assert_eq!(faces, [true; 4]);
}

/// Whether the cells just past `c`'s foot (the side its turn points to)
/// hold something solid in its band, or are the tank glass.
fn foot_held(w: &World, c: &Creature) -> bool {
    let b = w.creature_body(c);
    let solid = |x: usize, y: usize| {
        w.cell(x, y).unwrap().material != id(w, "air") && blocks(w, x, y, c.band())
    };
    match c.turn() {
        0 => b.y1 == w.height() || (b.x0..b.x1).any(|x| solid(x, b.y1)),
        1 => b.x0 == 0 || (b.y0..b.y1).any(|y| solid(b.x0 - 1, y)),
        2 => b.y0 == 0 || (b.x0..b.x1).any(|x| solid(x, b.y0 - 1)),
        _ => b.x1 == w.width() || (b.y0..b.y1).any(|y| solid(b.x1, y)),
    }
}

#[test]
fn nerite_climbs_a_wall_crosses_an_overhang_and_comes_back_down() {
    // A glued Γ: a pillar on the sand with an arm reaching right over it.
    let mut w = world(120, 80, 7);
    sand_bed(&mut w, 6);
    let rock = id(&w, "rock");
    for r in [rect(40, 40, 46, 74), rect(46, 40, 80, 46)] {
        w.fill_rect(r, rock, Brush::Replace).unwrap();
        w.set_anchored(w.body_at(r.x0, r.y0).unwrap(), true);
    }
    let snail = w.add_creature(species(&w, "nerite"), 20, 72).unwrap();
    let (mut held, mut led, ticks) = (0, 0, 4_000);
    // First tick on each leg: up the pillar (foot right), upside down
    // under the arm, down the pillar's far side (foot left), upright on
    // the sand beyond it, then up the tank glass (foot right).
    let mut legs = [None; 5];
    let mut last = w.creature(snail).map(|c| (c.centre(), c.turn())).unwrap();
    for _ in 0..ticks {
        w.tick();
        let c = w.creature(snail).unwrap();
        assert_eq!(solid_under(&w, c), None, "tick {}", w.tick_count());
        assert!(w.in_zone(c), "came off at tick {}", w.tick_count());
        held += u32::from(foot_held(&w, c));
        let b = w.creature_body(c);
        let size = (b.x1 - b.x0, b.y1 - b.y0);
        assert_eq!(
            size,
            if c.turn().is_multiple_of(2) {
                (5, 3)
            } else {
                (3, 5)
            }
        );
        // The head (the sprite's right, mirrored or not, then turned)
        // leads along any move across the turned body's length.
        let ((x, y), turn) = (c.centre(), c.turn());
        if turn == last.1 && (x, y) != last.0 {
            let (fx, fy) = [(0.0, 1.0), (-1.0, 0.0), (0.0, -1.0), (1.0, 0.0)][usize::from(turn)];
            let ahead = (x - last.0.0) * fy - (y - last.0.1) * fx;
            let ahead = if c.facing_left() { -ahead } else { ahead };
            assert!(ahead >= 0.0, "tail first at tick {}", w.tick_count());
            led += u32::from(ahead > 0.0);
        }
        last = ((x, y), turn);
        let leg = match turn {
            3 if b.x1 == 40 && b.y1 < 70 => Some(0),
            2 if b.y0 == 46 && b.x0 >= 50 && b.x1 <= 76 => Some(1),
            1 if b.x0 == 46 && b.y0 > 50 && b.y1 < 70 => Some(2),
            0 if b.y1 == 74 && b.x0 > 50 && b.x1 < 110 => Some(3),
            3 if b.x1 == 120 && b.y1 < 60 => Some(4),
            _ => None,
        };
        if let Some(i) = leg {
            legs[i].get_or_insert(w.tick_count());
        }
    }
    let legs = legs.map(|t| t.expect("every leg is reached"));
    assert!(legs.is_sorted(), "legs in order: {legs:?}");
    assert!(led > 300, "moved head first {led} times");
    // Only outside corners, where it holds on diagonally, leave the foot
    // off a face.
    assert!(
        held * 100 >= ticks * 95,
        "foot held on {held} of {ticks} ticks"
    );
}

#[test]
fn grazing_removes_real_cells_feeds_and_updates_the_ecosystem() {
    let grazers: [(&str, &[&str]); 9] = [
        ("corydoras", &["algae", "detritus"]),
        ("nerite", &["algae", "detritus"]),
        ("otocinclus", &["algae", "detritus"]),
        ("bristlenose_pleco", &["algae", "detritus"]),
        ("mystery_snail", &["algae", "detritus"]),
        ("ramshorn_snail", &["algae", "detritus"]),
        ("cherry_shrimp", &["algae", "detritus"]),
        ("amano_shrimp", &["algae", "detritus"]),
        ("thai_micro_crab", &["detritus"]),
    ];
    for (name, meals) in grazers {
        for &meal in meals {
            let mut w = world(128, 64, 7);
            sand_bed(&mut w, 6);
            let food = id(&w, meal);
            w.fill_rect(rect(34, 57, 46, 58), food, Brush::Replace)
                .unwrap();
            let grazer = w.add_creature(species(&w, name), 40, 54).unwrap();
            let mut control = w.clone();
            control.remove_creature(grazer);
            run(&mut w, 40);
            run(&mut control, 40);
            let c = w.creature(grazer).unwrap();
            assert_eq!(c.eaten(), 4, "{name} grazing {meal}");
            assert!(!c.hungry());
            let (after, before) = (w.ecosystem(), control.ecosystem());
            if meal == "algae" {
                assert_eq!(after.algae_cells + 4, before.algae_cells);
                assert!(after.algae_coverage < before.algae_coverage);
            } else {
                assert_eq!(after.pollutant_cells + 4, before.pollutant_cells);
            }
        }
    }
}

#[test]
fn pointer_input_repels_fish_and_clears_on_leave() {
    let mut w = world(160, 96, 44);
    let fish = w.add_creature(species(&w, "neon_tetra"), 80, 45).unwrap();
    let mut left = w.clone();
    let mut right = w.clone();
    left.set_creature_cursor(Some((77, 45)));
    right.set_creature_cursor(Some((83, 45)));
    run(&mut left, 24);
    run(&mut right, 24);
    assert!(left.creature(fish).unwrap().cell().0 > 86);
    assert!(right.creature(fish).unwrap().cell().0 < 74);
    let mut cleared = w.clone();
    cleared.set_creature_cursor(Some((77, 45)));
    cleared.set_creature_cursor(None);
    w.set_creature_cursor(Some((i64::MAX, -1)));
    run(&mut w, 80);
    run(&mut cleared, 80);
    assert_eq!(w.state_hash(), cleared.state_hash());
}

#[test]
fn shrimp_walk_the_substrate_and_hop_off_it_now_and_then() {
    for name in ["cherry_shrimp", "amano_shrimp"] {
        let mut w = world(128, 80, 21);
        sand_bed(&mut w, 6);
        let shrimp = w.add_creature(species(&w, name), 30, 60).unwrap();
        run(&mut w, 200);
        let floor = w.height() - 6;
        let (mut on_bed, mut hops, mut highest, mut aloft) = (0, 0, floor, false);
        let mut range = (128, 0);
        for _ in 0..4_000 {
            w.tick();
            let c = w.creature(shrimp).unwrap();
            assert_eq!(solid_under(&w, c), None);
            assert!(w.in_zone(c), "{name} left its zone");
            let b = w.creature_body(c);
            if b.y1 == floor {
                on_bed += 1;
                aloft = false;
            } else if b.y1 + 2 <= floor && !aloft {
                aloft = true;
                hops += 1;
            }
            highest = highest.min(b.y1);
            range = (range.0.min(c.cell().0), range.1.max(c.cell().0));
        }
        eprintln!("{name}: {hops} hops, on the bed {on_bed}/4000, top {highest}");
        // Mostly on the bed, with a hop every 6–10 s that lifts it a few
        // cells and lands it again.
        assert!(on_bed > 2_400, "{name} on the bed {on_bed}/4000 ticks");
        assert!(hops >= 15, "{name} hopped {hops} times in 200 s");
        assert!(
            floor - highest <= 6,
            "{name} hopped {} cells",
            floor - highest
        );
        assert!(range.1 - range.0 > 40, "{name} roamed {range:?}");
    }
}

#[test]
fn shrimp_find_algae_on_the_bed_and_graze_it() {
    let limit = 10 * TICKS_PER_SECOND;
    for (name, food) in [
        ("cherry_shrimp", "algae"),
        ("amano_shrimp", "algae"),
        ("amano_shrimp", "bloodworm"),
        ("cherry_shrimp", "sinking_pellet"),
    ] {
        let t = ticks_to_eat(name, (30, 78), food, (75, 81), limit)
            .unwrap_or_else(|| panic!("{name} didn't eat {food} within 10 s"));
        eprintln!("{name} ate {food} after {:.2} s", f64::from(t) / 20.0);
    }
}

#[test]
fn thai_micro_crab_filter_feeds_from_two_cells_away() {
    for (name, eats) in [("thai_micro_crab", true), ("nerite", false)] {
        let mut w = world(96, 64, 3);
        sand_bed(&mut w, 6);
        let crawler = w.add_creature(species(&w, name), 40, 56).unwrap();
        run(&mut w, 2);
        let b = w.creature_body(w.creature(crawler).unwrap());
        // A cell of mulm on the bed one cell clear of its body.
        w.set_cell(b.x1 + 1, b.y1 - 1, id(&w, "detritus")).unwrap();
        w.tick();
        let ate = w.creature(crawler).unwrap().eaten() > 0;
        assert_eq!(ate, eats, "{name}");
    }
}

#[test]
fn thai_micro_crab_clings_to_plants_and_walks_the_bottom() {
    let mut w = world(120, 80, 5);
    let (aquasoil, cutting) = (id(&w, "aquasoil"), id(&w, "cutting"));
    w.fill_rect(rect(0, 72, 120, 80), aquasoil, Brush::Replace)
        .unwrap();
    for x in [30, 60, 90] {
        w.set_cell(x, 71, cutting).unwrap();
    }
    // Ten minutes of growth for something to climb.
    run(&mut w, 10 * 60 * TICKS_PER_SECOND);
    let parts = [id(&w, "stem"), id(&w, "leaf"), id(&w, "root")];
    let plant = |m: MaterialId| parts.contains(&m);
    assert!(w.plant_count() > 0);
    let crab = w
        .add_creature(species(&w, "thai_micro_crab"), 10, 70)
        .unwrap();
    let (mut held, mut on_plant, mut on_bed, ticks) = (0, 0, 0, 6_000);
    for _ in 0..ticks {
        w.tick();
        let c = w.creature(crab).unwrap();
        assert_eq!(solid_under(&w, c), None);
        held += u32::from(foot_held(&w, c));
        let b = w.creature_body(c);
        let near = |x: usize, y: usize| w.cell(x, y).is_some_and(|cell| plant(cell.material));
        let x0 = b.x0.saturating_sub(1);
        let y0 = b.y0.saturating_sub(1);
        let touching = (y0..=b.y1).any(|y| (x0..=b.x1).any(|x| near(x, y)));
        on_plant += u32::from(touching && b.y1 < 72);
        on_bed += u32::from(c.turn() == 0 && b.y1 == 72 && !touching);
    }
    eprintln!("crab: held {held}, on plants {on_plant}, on the bed {on_bed} of {ticks}");
    assert!(
        held * 100 >= ticks * 90,
        "foot held on {held} of {ticks} ticks"
    );
    assert!(on_plant > 300, "on plants {on_plant} ticks");
    assert!(on_bed > 100, "walking the bed {on_bed} ticks");
}

#[test]
fn a_hop_never_lifts_a_shrimp_out_of_a_shallow_zone() {
    for name in ["cherry_shrimp", "amano_shrimp"] {
        let mut w = World::new(
            WorldConfig {
                width: 160,
                height: 90,
                seed: 3,
                water_level: 0.45,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        sand_bed(&mut w, 8);
        let shrimp = w.add_creature(species(&w, name), 60, 80).unwrap();
        run(&mut w, 200);
        let mut out = 0;
        for _ in 0..6_000 {
            w.tick();
            out += u32::from(!w.in_zone(w.creature(shrimp).unwrap()));
        }
        assert_eq!(out, 0, "{name} out of its zone on {out} ticks");
    }
}

/// Mean distance from each creature of `s` to its nearest neighbour of
/// the same species.
fn mean_nearest_mate(w: &World, s: SpeciesId) -> f32 {
    let fish: Vec<_> = w.creatures().iter().filter(|c| c.species() == s).collect();
    let nearest = |f: &Creature| {
        let (x, y) = f.centre();
        fish.iter()
            .filter(|c| c.id() != f.id())
            .map(|c| {
                let (mx, my) = c.centre();
                (mx - x).hypot(my - y)
            })
            .fold(f32::INFINITY, f32::min)
    };
    fish.iter().map(|f| nearest(f)).sum::<f32>() / fish.len() as f32
}

#[test]
fn harlequins_and_neons_each_school_with_their_own_kind() {
    for seed in [4, 77] {
        // Both species dispersed and interleaved (every harlequin starts
        // between neons), and each species alone with the same ids.
        let stocked = |neons: bool, harlequins: bool| {
            let mut w = world(320, 120, seed);
            let (tetra, rasbora) = (species(&w, "neon_tetra"), species(&w, "harlequin_rasbora"));
            for i in 0..8 {
                let c = w
                    .add_creature(tetra, 15 + 40 * i, 45 + (i % 3) * 14)
                    .unwrap();
                if !neons {
                    w.remove_creature(c);
                }
            }
            for i in 0..8 {
                if harlequins {
                    w.add_creature(rasbora, 35 + 40 * i, 52 + (i % 3) * 10)
                        .unwrap();
                }
            }
            w
        };
        let (mut w, mut alone_t, mut alone_r) = (
            stocked(true, true),
            stocked(true, false),
            stocked(false, true),
        );
        let (tetra, rasbora) = (species(&w, "neon_tetra"), species(&w, "harlequin_rasbora"));
        let only = |w: &World, s| -> Vec<Creature> {
            w.creatures()
                .iter()
                .filter(|c| c.species() == s)
                .cloned()
                .collect()
        };
        let lengths = |s| 3.0 * w.species().get(s).unwrap().size().0 as f32;
        let (near_t, near_r) = (lengths(tetra), lengths(rasbora));
        let (mut tight_t, mut tight_r, ticks) = (0, 0, 6_000);
        for _ in 0..ticks {
            w.tick();
            alone_t.tick();
            alone_r.tick();
            // Neither species follows or steers by the other: each swims
            // exactly as it does alone.
            assert_eq!(only(&w, tetra), alone_t.creatures());
            assert_eq!(only(&w, rasbora), alone_r.creatures());
            tight_t += u32::from(mean_nearest_mate(&w, tetra) <= near_t);
            tight_r += u32::from(mean_nearest_mate(&w, rasbora) <= near_r);
        }
        eprintln!("seed {seed}: neons {tight_t}, harlequins {tight_r} of {ticks} ticks schooled");
        assert!(tight_r * 5 >= ticks * 4, "harlequins schooled {tight_r}");
        assert!(tight_t * 5 >= ticks * 4, "neons schooled {tight_t}");
    }
}

#[test]
fn otocinclus_crawls_to_an_algae_film_on_a_rock_and_grazes_it() {
    let mut w = world(120, 80, 9);
    sand_bed(&mut w, 6);
    let rock = id(&w, "rock");
    w.fill_rect(rect(70, 46, 84, 74), rock, Brush::Replace)
        .unwrap();
    w.set_anchored(w.body_at(76, 60).unwrap(), true);
    // A film on the rock's left face and top, about 45 cells from the
    // start.
    let algae = id(&w, "algae");
    w.fill_rect(rect(69, 50, 70, 66), algae, Brush::Replace)
        .unwrap();
    w.fill_rect(rect(72, 45, 82, 46), algae, Brush::Replace)
        .unwrap();
    let oto = w.add_creature(species(&w, "otocinclus"), 20, 72).unwrap();
    // The same tank without it, for the algae it took.
    let mut control = w.clone();
    control.remove_creature(oto);
    let limit = 30 * TICKS_PER_SECOND;
    let mut first = None;
    for t in 1..=limit {
        w.tick();
        control.tick();
        let c = w.creature(oto).unwrap();
        assert_eq!(solid_under(&w, c), None);
        if first.is_none() && c.eaten() > 0 {
            first = Some(t);
            // Its first bite is at the rock's film: its body within a cell.
            let b = w.creature_body(c);
            assert!(
                b.x1 >= 69 && b.x0 <= 83 && b.y1 >= 45,
                "first bite at {b:?}"
            );
        }
        if c.eaten() >= 4 {
            break;
        }
    }
    let t = first.expect("otocinclus grazed within 30 s");
    eprintln!("otocinclus grazed after {:.1} s", f64::from(t) / 20.0);
    let eaten = w.creature(oto).unwrap().eaten();
    assert!(eaten >= 4, "grazed {eaten} cells");
    // Every bite was a cell of the film.
    let cells = |w: &World| w.material_counts()[usize::from(algae.0)];
    assert_eq!(cells(&control) - cells(&w), eaten as usize);
}

#[test]
fn guppy_bites_a_live_frond_and_damages_its_plant() {
    let mut w = world(80, 64, 7);
    let fish = w.add_creature(species(&w, "guppy"), 40, 10).unwrap();
    let leaf = id(&w, "floating_leaf");
    w.fill_rect(rect(35, 8, 46, 9), leaf, Brush::Replace)
        .unwrap();
    let plant = w.plant_at(40, 8).unwrap();
    let health = w.plant(plant).unwrap().health;
    for _ in 0..100 {
        w.tick();
        if w.creature(fish).unwrap().eaten() > 0 {
            assert!(w.plant(plant).unwrap().health < health);
            return;
        }
    }
    panic!("guppy never ate the floating leaves");
}

/// The shape and motion of a school over a run, after it has settled.
struct SchoolShape {
    /// Ticks the school met the SPEC spacing limit (≤ 3 body lengths).
    close: u32,
    ticks: u32,
    /// Mean of the school's major over minor axis (from the covariance of
    /// its positions): 1 for a round blob.
    elongation: f32,
    /// Mean polarisation, |mean unit heading| of the moving fish: 1 when
    /// every fish heads exactly the same way on a tick.
    polarisation: f32,
    /// Mean distance, in body lengths, each fish's place in the school
    /// (its offset from the centroid) moves over 10 s.
    reshuffle: f32,
}

fn school_shape(seed: u64, name: &str, fish: i64) -> SchoolShape {
    let mut w = world(320, 120, seed);
    let s = species(&w, name);
    let length = w.species().get(s).unwrap().size().0 as f32;
    for i in 0..fish {
        w.add_creature(s, 15 + 290 * i / fish, 45 + (i % 3) * 14)
            .unwrap();
    }
    run(&mut w, 1_000);
    let centres =
        |w: &World| -> Vec<(f32, f32)> { w.creatures().iter().map(Creature::centre).collect() };
    let offsets = |p: &[(f32, f32)]| {
        let n = p.len() as f32;
        let (mx, my) = p
            .iter()
            .fold((0.0, 0.0), |(a, b), &(x, y)| (a + x / n, b + y / n));
        p.iter().map(|&(x, y)| (x - mx, y - my)).collect::<Vec<_>>()
    };
    let ticks = 6_000;
    let (mut close, mut elongation, mut polarisation, mut polarised) = (0, 0.0, 0.0, 0);
    let (mut reshuffle, mut shuffles) = (0.0, 0);
    let mut before = centres(&w);
    let mut place = offsets(&before);
    for t in 1..=ticks {
        w.tick();
        let now = centres(&w);
        close += u32::from(mean_nearest_mate(&w, s) <= 3.0 * length);
        let d = offsets(&now);
        let n = d.len() as f32;
        let (sxx, syy, sxy) = d.iter().fold((0.0, 0.0, 0.0), |(a, b, c), &(x, y)| {
            (a + x * x / n, b + y * y / n, c + x * y / n)
        });
        let (mean, root) = (
            f32::midpoint(sxx, syy),
            (((sxx - syy) / 2.0).powi(2) + sxy * sxy).sqrt(),
        );
        elongation += ((mean + root) / (mean - root).max(1e-3)).sqrt().min(20.0);
        let heads: Vec<_> = now
            .iter()
            .zip(&before)
            .map(|(a, b)| (a.0 - b.0, a.1 - b.1))
            .filter(|v| v.0.hypot(v.1) > 1e-3)
            .map(|v| {
                let m = v.0.hypot(v.1);
                (v.0 / m, v.1 / m)
            })
            .collect();
        if heads.len() >= 2 {
            let m = heads.len() as f32;
            let (hx, hy) = heads
                .iter()
                .fold((0.0, 0.0), |(a, b), h| (a + h.0 / m, b + h.1 / m));
            polarisation += hx.hypot(hy);
            polarised += 1;
        }
        if t % (10 * TICKS_PER_SECOND) == 0 {
            reshuffle += d
                .iter()
                .zip(&place)
                .map(|(a, b)| (a.0 - b.0).hypot(a.1 - b.1))
                .sum::<f32>()
                / n
                / length;
            shuffles += 1;
            place = d;
        }
        before = now;
    }
    SchoolShape {
        close,
        ticks,
        elongation: elongation / ticks as f32,
        polarisation: polarisation / polarised.max(1) as f32,
        reshuffle: reshuffle / shuffles as f32,
    }
}

/// A settled school stretches along the way it swims, its fish head
/// roughly but not exactly the same way, and they change places.
#[test]
fn schools_are_loose_irregular_and_out_of_step() {
    for (seed, name) in [
        (4, "neon_tetra"),
        (77, "neon_tetra"),
        (123, "neon_tetra"),
        (9, "harlequin_rasbora"),
        (31, "harlequin_rasbora"),
    ] {
        let s = school_shape(seed, name, 10);
        eprintln!(
            "seed {seed} {name}: close {}/{}, elongation {:.2}, polarisation {:.3}, reshuffle {:.2} lengths",
            s.close, s.ticks, s.elongation, s.polarisation, s.reshuffle
        );
        // Still a school: close (SPEC T7) and heading broadly one way.
        assert!(s.close * 10 >= s.ticks * 9, "school broke up");
        assert!(s.polarisation >= 0.4, "fish swim every which way");
        // But not a round blob in lockstep that never changes (on the
        // old steering: elongation 1.19–1.20, polarisation 0.998 and
        // places moving 0.01–0.02 body lengths in 10 s).
        assert!(s.elongation >= 1.6, "a round school");
        assert!(s.polarisation <= 0.9, "fish swim in lockstep");
        assert!(s.reshuffle >= 1.0, "fish keep their places");
    }
}

#[test]
fn palette_bettas_cycle_through_inheritable_colours() {
    let mut w = world(160, 90, 8);
    let betta = species(&w, "betta");
    let mut tools = Tools::new(id(&w, "sand"));
    tools.select(PaletteItem::Tool(Tool::AddCreature(betta)));
    for morph in [0, 1, 2, 0] {
        let Outcome::Added(id) = tools.press(&mut w, Button::Primary, (80, 40)) else {
            panic!("expected a betta");
        };
        assert_eq!(w.creature(id).unwrap().alleles(), [morph; 2]);
        tools.release(&mut w, Button::Primary);
    }
}

#[test]
fn frogs_return_to_air_between_dives() {
    for name in ["african_dwarf_frog", "common_frog"] {
        let mut w = world(128, 100, 19);
        let frog = w.add_creature(species(&w, name), 60, 50).unwrap();
        let mut dives = 0;
        let mut submerged = true;
        for _ in 0..2400 {
            w.tick();
            let top = w.creature_body(w.creature(frog).unwrap()).y0;
            if top > w.surface_row() + 2 {
                submerged = true;
            } else if submerged && top == w.surface_row() {
                dives += 1;
                submerged = false;
            }
        }
        assert!(dives >= 3, "{name} surfaced {dives} times");
    }
}
