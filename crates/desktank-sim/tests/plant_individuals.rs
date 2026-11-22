//! SPEC T6b acceptance tests (D18, §2.7): plants are individuals of
//! data-defined species, with their own ids, sizes and health, a table that
//! always matches the grid, splitting by scoop, health and death, the
//! population cap, the inspect card and a health tint that changes without
//! waking a chunk.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names
)]

use std::collections::{BTreeMap, BTreeSet};

use desktank_sim::tools::{Button, Inspection, Outcome, PaletteItem, Tool, Tools};
use desktank_sim::{
    Brush, Factor, FieldKind, Limit, MaterialId, MaterialTable, PlantId, PlantSpeciesId, Propagule,
    Rect, World, WorldConfig,
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

/// A tank `height` rows tall with an aquasoil bed `bed` rows deep.
fn bedded(height: usize, bed: usize, seed: u64) -> World {
    let mut w = world(160, height, seed);
    let soil = id(&w, "aquasoil");
    w.fill_rect(rect(0, height - bed, 160, height), soil, Brush::Replace)
        .unwrap();
    w
}

/// Ticks `ticks` times, checking the plant table against the grid after
/// every tick.
fn run_checked(w: &mut World, ticks: u32) {
    for _ in 0..ticks {
        w.tick();
        check_table(w);
    }
}

fn run(w: &mut World, ticks: u32) {
    for _ in 0..ticks {
        w.tick();
    }
}

/// The plant table matches the grid: every plant part cell belongs to a
/// plant in the table, and each plant's part counts and bounds are exactly
/// its cells'.
fn check_table(w: &World) {
    let parts = ["root", "stem", "leaf"].map(|n| id(w, n));
    let mut grid: BTreeMap<u16, ([u32; 3], Rect)> = BTreeMap::new();
    for y in 0..w.height() {
        for x in 0..w.width() {
            let c = w.cell(x, y).unwrap();
            let Some(k) = parts.iter().position(|&m| m == c.material) else {
                continue;
            };
            let e = grid.entry(c.state).or_insert(([0; 3], Rect::EMPTY));
            e.0[k] += 1;
            e.1 = e.1.union(rect(x, y, x + 1, y + 1));
        }
    }
    let table: BTreeMap<u16, ([u32; 3], Rect)> =
        w.plants().map(|p| (p.id.0, (p.parts, p.bounds))).collect();
    assert_eq!(grid, table, "tick {}", w.tick_count());
}

/// The cell indices of plant `id`.
fn cells_of(w: &World, plant: PlantId) -> BTreeSet<usize> {
    (0..w.width() * w.height())
        .filter(|&i| w.plant_at(i % w.width(), i / w.width()) == Some(plant))
        .collect()
}

/// Glues a rock slab over columns `x0..x1`, rows 8 to `8 + rows`.
fn lid(w: &mut World, x0: usize, x1: usize, rows: usize) {
    let rock = id(w, "rock");
    w.fill_rect(rect(x0, 8, x1, 8 + rows), rock, Brush::Replace)
        .unwrap();
    let body = w.body_at(x0, 8).unwrap();
    assert!(w.set_anchored(body, true));
}

fn plant_of(w: &World, s: PlantSpeciesId) -> Vec<desktank_sim::PlantInfo> {
    w.plants().filter(|p| p.species == s).collect()
}

#[test]
fn two_touching_plants_of_one_species_keep_separate_ids_sizes_and_health() {
    let mut w = bedded(64, 8, 1);
    let hyg = species(&w, "hygrophila");
    // Stems 7 columns apart: their 3-cell leaves meet between them. The
    // left one is in field column 9 (x 72–79), the right in column 10.
    w.paint_plant(76, 30, 0, hyg, Propagule::Cuttings).unwrap();
    w.paint_plant(83, 30, 0, hyg, Propagule::Cuttings).unwrap();
    run_checked(&mut w, 10 * MINUTE);
    let plants: Vec<_> = w.plants().collect();
    assert_eq!(plants.len(), 2);
    let (a, b) = (plants[0], plants[1]);
    assert!(a.base.0 < b.base.0 && a.species == hyg && b.species == hyg);
    let (cells_a, cells_b) = (cells_of(&w, a.id), cells_of(&w, b.id));
    assert_eq!(cells_a.len(), a.size() as usize);
    assert_eq!(cells_b.len(), b.size() as usize);
    let touching = cells_a.iter().any(|&i| cells_b.contains(&(i + 1)));
    assert!(touching, "the plants grew into each other");
    // Shade only the left plant: its health falls, the right one's doesn't.
    lid(&mut w, 64, 80, 16);
    run_checked(&mut w, MINUTE);
    let (a, b) = (w.plant(a.id).unwrap(), w.plant(b.id).unwrap());
    assert!(
        a.health < 0.9 && (b.health - 1.0).abs() < 1e-6,
        "{a:?} vs {b:?}"
    );
    assert_eq!(a.limit, Limit::TooLow(Factor::Light));
    // Scooping through the left one never changes the right one.
    let before = (cells_of(&w, b.id), w.plant(b.id).unwrap());
    let mut tools = Tools::new(id(&w, "sand"));
    tools.select(PaletteItem::Tool(Tool::Scoop));
    tools.resize(-2);
    let (ax, ay) = (a.base.0 as i64, a.base.1 as i64);
    tools.press(&mut w, Button::Primary, (ax - 2, ay - 12));
    tools.moved(&mut w, (ax - 2, ay - 30));
    tools.release(&mut w, Button::Primary);
    w.tick();
    check_table(&w);
    let after = (cells_of(&w, b.id), w.plant(b.id).unwrap());
    assert_eq!(before.0, after.0);
    assert_eq!(before.1.parts, after.1.parts);
    assert_eq!(before.1.bounds, after.1.bounds);
    assert!(w.plant(a.id).unwrap().size() < a.size());
}

/// One rotala and one hygrophila grown for 10 minutes in a shallow tank,
/// under a lid `rows` thick (0: none). Returns the two sizes and the
/// rotala's limit.
fn compete(rows: usize, seed: u64) -> (u32, u32, Limit) {
    let mut w = bedded(48, 6, seed);
    if rows > 0 {
        lid(&mut w, 0, 160, rows);
    }
    let (rotala, hyg) = (species(&w, "rotala"), species(&w, "hygrophila"));
    w.paint_plant(40, 30, 0, rotala, Propagule::Cuttings)
        .unwrap();
    w.paint_plant(120, 30, 0, hyg, Propagule::Cuttings).unwrap();
    run(&mut w, 10 * MINUTE);
    check_table(&w);
    let size = |s| {
        plant_of(&w, s)
            .iter()
            .map(desktank_sim::PlantInfo::size)
            .sum::<u32>()
    };
    let limit = plant_of(&w, rotala)
        .first()
        .map_or(Limit::None, |p| p.limit);
    (size(rotala), size(hyg), limit)
}

#[test]
fn rotala_outgrows_hygrophila_in_strong_light_and_loses_in_weak_light() {
    // Measured (seeds 0–1): open tank, light 0.73–0.91 over the plants,
    // rotala 147 cells, hygrophila 94–102; under a 5-row rock lid, light
    // 0.39–0.42, rotala 33, hygrophila 159–161.
    for seed in 0..2 {
        let (rotala, hyg, _) = compete(0, seed);
        assert!(
            rotala * 10 >= hyg * 13,
            "strong light: rotala {rotala}, hygrophila {hyg}"
        );
        let (rotala, hyg, limit) = compete(5, seed);
        assert!(
            hyg >= 3 * rotala,
            "weak light: rotala {rotala}, hygrophila {hyg}"
        );
        assert_eq!(limit, Limit::TooLow(Factor::Light));
    }
}

#[test]
fn scooping_through_a_stem_makes_a_fragment_or_dead_plant_matter_by_species() {
    let mut w = bedded(64, 8, 2);
    let (hyg, rotala) = (species(&w, "hygrophila"), species(&w, "rotala"));
    w.paint_plant(40, 30, 0, hyg, Propagule::Cuttings).unwrap();
    w.paint_plant(120, 30, 0, rotala, Propagule::Cuttings)
        .unwrap();
    run_checked(&mut w, 10 * MINUTE);
    let (h, r) = (plant_of(&w, hyg)[0], plant_of(&w, rotala)[0]);
    let dead = count(&w, "dead_plant") + count(&w, "detritus");
    // A two-cell-wide cut across each stem, 15 cells above its base: the
    // stem and the leaves at those rows go.
    let water = id(&w, "water");
    for p in [h, r] {
        let (x, y) = p.base;
        w.fill_rect(rect(x - 3, y - 16, x + 4, y - 14), water, Brush::Replace)
            .unwrap();
    }
    w.tick();
    check_table(&w);
    // Hygrophila propagates by fragments: the top lives on as a new plant.
    let hs = plant_of(&w, hyg);
    assert_eq!(hs.len(), 2, "{hs:?}");
    let (base, top) = (hs[0], hs[1]);
    assert_eq!(
        (base.id, base.base),
        (h.id, h.base),
        "the rooted piece keeps the id"
    );
    assert_eq!(top.parent, Some(h.id));
    assert_eq!(top.generation, 1);
    assert!(top.bounds.y1 <= h.base.1 - 15 && top.parts[1] > 0);
    // Rotala doesn't: its top becomes dead plant matter.
    let rs = plant_of(&w, rotala);
    assert_eq!(rs.len(), 1);
    assert_eq!((rs[0].id, rs[0].base), (r.id, r.base));
    assert!(rs[0].bounds.y0 > r.base.1 - 15);
    assert!(count(&w, "dead_plant") + count(&w, "detritus") > dead + 20);
    // The table keeps matching while the pieces regrow and the dead matter
    // sinks and rots.
    run_checked(&mut w, 2 * MINUTE);
}

/// A hygrophila grown in T6's tank until it has finished growing and every
/// chunk sleeps.
fn grown_and_asleep(seed: u64) -> World {
    let mut w = bedded(120, 12, seed);
    // Keep a single plant so spontaneous seeding doesn't sprout a second one
    // while testing single-plant starvation and oldest-leaf drop.
    w.set_plant_cap(1);
    let hyg = species(&w, "hygrophila");
    w.paint_plant(80, 20, 0, hyg, Propagule::Cuttings).unwrap();
    run(&mut w, 30 * MINUTE);
    let mut asleep = 0;
    for _ in 0..20 * MINUTE {
        w.tick();
        asleep = if w.awake_chunks() == 0 { asleep + 1 } else { 0 };
        if asleep > 200 {
            break;
        }
    }
    assert!(asleep > 200, "the grown tank never fell asleep");
    check_table(&w);
    w
}

/// Drains the nutrient field everywhere (as T6's starvation test does).
fn drain(w: &mut World) {
    let (fw, fh) = {
        let f = w.field(FieldKind::Nutrient);
        (f.width(), f.height())
    };
    for fy in 0..fh {
        for fx in 0..fw {
            w.add_to_field(FieldKind::Nutrient, fx * 8, fy * 8, -1.0);
        }
    }
}

#[test]
fn a_starving_plants_tint_changes_without_waking_a_chunk_or_rewriting_cells() {
    let mut w = grown_and_asleep(3);
    let plant = w.plants().next().unwrap();
    assert!((plant.health - 1.0).abs() < 1e-6);
    // What the renderer draws (material and variant) and the plant ids;
    // aquasoil keeps its buffer level in its state, quietly.
    let drawn = |w: &World| -> Vec<(u8, u8, Option<PlantId>)> {
        (0..w.width() * w.height())
            .map(|i| {
                let (x, y) = (i % w.width(), i / w.width());
                let c = w.cell(x, y).unwrap();
                (c.material.0, c.variant, w.plant_at(x, y))
            })
            .collect()
    };
    let cells = drawn(&w);
    let version = w.plants_version();
    // 70 s of starvation: health falls after its 20 s tolerance, but stays
    // above the 0.5 at which leaves drop.
    for _ in 0..70 * 20 {
        drain(&mut w);
        w.tick();
        assert_eq!(w.awake_chunks(), 0, "tick {}", w.tick_count());
        assert_eq!(w.dirty_rects().count(), 0);
    }
    assert!(drawn(&w) == cells, "no cell was rewritten");
    let starving = w.plant(plant.id).unwrap();
    assert!(
        starving.health < 0.9 && starving.health > 0.5,
        "{starving:?}"
    );
    assert_eq!(starving.limit, Limit::TooLow(Factor::Nutrient));
    assert!(w.plants_version() > version, "the tint texture must update");
}

#[test]
fn low_health_drops_the_oldest_leaves_first_then_the_plant_dies() {
    let mut w = grown_and_asleep(4);
    let plant = w.plants().next().unwrap();
    let leaf = id(&w, "leaf");
    let leaves = |w: &World| -> Vec<(usize, usize)> {
        let mut v = Vec::new();
        for y in 0..w.height() {
            for x in 0..w.width() {
                if w.cell(x, y).unwrap().material == leaf {
                    v.push((x, y));
                }
            }
        }
        v
    };
    let before = leaves(&w);
    lid(&mut w, 0, 160, 16);
    // The first leaves to go are the lowest.
    let mut first = None;
    for _ in 0..5 * MINUTE {
        w.tick();
        check_table(&w);
        let now = leaves(&w);
        if now.len() < before.len() {
            first = Some(now);
            break;
        }
    }
    let now = first.expect("no leaf dropped in 5 minutes of darkness");
    let lowest_kept = now.iter().map(|&(_, y)| y).max().unwrap();
    for gone in before.iter().filter(|c| !now.contains(c)) {
        assert!(
            gone.1 >= lowest_kept,
            "dropped {gone:?} above {lowest_kept}"
        );
    }
    assert!(
        w.plant(plant.id).is_some(),
        "it lives on while it drops leaves"
    );
    // Then the whole plant becomes dead plant matter.
    run_checked(&mut w, 5 * MINUTE);
    assert_eq!(w.plant_count(), 0);
    assert_eq!(w.plant_cells(), 0);
    assert!(count(&w, "dead_plant") + count(&w, "detritus") > 0);
}

#[test]
fn the_population_cap_holds() {
    let mut w = bedded(64, 8, 5);
    w.set_plant_cap(3);
    let hyg = species(&w, "hygrophila");
    for x in (20..140).step_by(12) {
        w.paint_plant(x, 30, 0, hyg, Propagule::Cuttings).unwrap();
    }
    run_checked(&mut w, 2 * MINUTE);
    assert_eq!(w.plant_count(), 3);
    // The rest wait as cuttings.
    assert_eq!(count(&w, "cutting"), 7);
    // An edit making a fourth plant writes dead plant matter instead.
    let stem = id(&w, "stem");
    w.fill_rect(rect(150, 10, 151, 14), stem, Brush::Replace)
        .unwrap();
    w.tick();
    check_table(&w);
    assert_eq!(w.plant_count(), 3);
}

#[test]
fn inspect_shows_species_age_health_size_and_what_limits_growth() {
    let mut w = bedded(64, 8, 6);
    let rotala = species(&w, "rotala");
    w.paint_plant(80, 30, 0, rotala, Propagule::Cuttings)
        .unwrap();
    run(&mut w, 4 * MINUTE);
    let p = plant_of(&w, rotala)[0];
    let mut tools = Tools::new(id(&w, "sand"));
    tools.select(PaletteItem::Tool(Tool::Inspect));
    let (x, y) = p.base;
    let out = tools.press(&mut w, Button::Primary, (x as i64, y as i64 - 1));
    let Outcome::Inspected(Inspection::Plant { id: found, .. }) = out else {
        panic!("inspect found {out:?}");
    };
    assert_eq!(found, p.id);
    let info = w.plant(found).unwrap();
    let s = w.plant_species().get(info.species).unwrap();
    assert_eq!(s.name, "rotala");
    assert!(info.age > 200.0 && info.age < 241.0, "{}", info.age);
    assert!((info.health - 1.0).abs() < 1e-6);
    assert!(info.size() > 20);
    // No CO₂ injection: CO₂ holds it back most (half its best response),
    // until the light is lower than that.
    assert_eq!(info.limit, Limit::TooLow(Factor::Co2));
    assert_eq!(info.limit.describe(), "CO₂ too low");
}

#[test]
fn each_plant_species_has_a_slot_that_pours_it() {
    let mut w = bedded(64, 8, 7);
    let items = Tools::palette(&w);
    for s in w.plant_species().iter() {
        assert!(
            items.contains(&PaletteItem::Tool(Tool::Plant(s.id))),
            "{}",
            s.name
        );
    }
    let mut tools = Tools::new(id(&w, "sand"));
    let moss = species(&w, "java_moss");
    tools.select(PaletteItem::Tool(Tool::Plant(moss)));
    assert_eq!(tools.selected_plant(), Some(moss));
    tools.resize(-3);
    assert!(matches!(
        tools.press(&mut w, Button::Primary, (80, 20)),
        Outcome::Painted(1)
    ));
    tools.release(&mut w, Button::Primary);
    let c = w.cell(80, 20).unwrap();
    assert_eq!(
        (c.material, c.state),
        (id(&w, "cutting"), u16::from(moss.0))
    );
}

#[test]
fn a_plant_slot_pours_the_propagule_picked_within_its_species() {
    let w = bedded(64, 8, 9);
    let (hyg, moss, rotala) = (
        species(&w, "hygrophila"),
        species(&w, "java_moss"),
        species(&w, "rotala"),
    );
    let mut tools = Tools::new(id(&w, "sand"));
    let slot = |s| PaletteItem::Tool(Tool::Plant(s));
    // Each species starts on its default: cuttings if it lists them.
    tools.select(slot(hyg));
    assert_eq!(tools.plant_propagule(&w), Some(Propagule::Cuttings));
    tools.select(slot(moss));
    assert_eq!(tools.plant_propagule(&w), Some(Propagule::Fragments));
    // Only the methods the species lists are taken.
    assert!(!tools.set_plant_propagule(&w, Propagule::Seeds));
    assert_eq!(tools.plant_propagule(&w), Some(Propagule::Fragments));
    tools.select(slot(hyg));
    assert!(tools.set_plant_propagule(&w, Propagule::Seeds));
    assert_eq!(tools.plant_propagule(&w), Some(Propagule::Seeds));
    // Picks are per species: rotala keeps its own default.
    tools.select(slot(rotala));
    assert_eq!(tools.plant_propagule(&w), Some(Propagule::Cuttings));
    // A host's new tools keep the picks as they are.
    let mut fresh = Tools::new(id(&w, "sand"));
    fresh.keep_plant_picks(&tools);
    fresh.select(slot(hyg));
    assert_eq!(fresh.plant_propagule(&w), Some(Propagule::Seeds));
    tools.select(slot(hyg));
    assert_eq!(tools.plant_propagule(&w), Some(Propagule::Seeds));
    // Cycling goes through hygrophila's three methods and wraps.
    let cycle: Vec<_> = (0..3)
        .map(|_| tools.next_plant_propagule(&w).unwrap())
        .collect();
    assert_eq!(
        cycle,
        [Propagule::Cuttings, Propagule::Fragments, Propagule::Seeds]
    );
    tools.select(slot(rotala));
    assert_eq!(tools.next_plant_propagule(&w), Some(Propagule::Cuttings));
    // The world refuses a propagule the species doesn't grow from.
    let mut w = w;
    assert!(w.paint_plant(80, 20, 0, rotala, Propagule::Seeds).is_err());
}

#[test]
fn hygrophila_seeds_pour_from_the_plants_slot_and_germinate() {
    let mut w = bedded(64, 8, 10);
    let hyg = species(&w, "hygrophila");
    let mut tools = Tools::new(id(&w, "sand"));
    tools.select(PaletteItem::Tool(Tool::Plant(hyg)));
    assert!(tools.set_plant_propagule(&w, Propagule::Seeds));
    tools.resize(-3);
    assert!(matches!(
        tools.press(&mut w, Button::Primary, (80, 20)),
        Outcome::Painted(1)
    ));
    tools.release(&mut w, Button::Primary);
    let c = w.cell(80, 20).unwrap();
    assert_eq!((c.material, c.state), (id(&w, "seed"), u16::from(hyg.0)));
    run(&mut w, 5 * MINUTE);
    assert_eq!(count(&w, "seed"), 0, "the seed germinated");
    assert_eq!(plant_of(&w, hyg).len(), 1);
}

#[test]
fn water_quality_counts_plant_cells_from_the_table() {
    let mut w = bedded(64, 8, 8);
    let hyg = species(&w, "hygrophila");
    w.paint_plant(50, 30, 0, hyg, Propagule::Cuttings).unwrap();
    w.paint_plant(110, 30, 0, hyg, Propagule::Cuttings).unwrap();
    run(&mut w, 5 * MINUTE);
    let cells = ["root", "stem", "leaf"]
        .iter()
        .map(|n| count(&w, n))
        .sum::<usize>();
    assert!(cells > 50, "{cells}");
    assert_eq!(w.plant_cells(), cells);
    assert_eq!(w.ecosystem().plant_cells, cells);
}

#[test]
fn a_mixed_planting_is_deterministic() {
    let grow = |seed| {
        let mut w = bedded(64, 8, seed);
        let rock = id(&w, "rock");
        w.fill_rect(rect(100, 40, 130, 50), rock, Brush::Replace)
            .unwrap();
        let body = w.body_at(100, 40).unwrap();
        w.set_anchored(body, true);
        let (hyg, rotala, moss) = (
            species(&w, "hygrophila"),
            species(&w, "rotala"),
            species(&w, "java_moss"),
        );
        w.paint_plant(20, 30, 1, hyg, Propagule::Cuttings).unwrap();
        w.paint_plant(60, 30, 0, rotala, Propagule::Cuttings)
            .unwrap();
        w.paint_plant(115, 30, 2, moss, Propagule::Fragments)
            .unwrap();
        run_checked(&mut w, 8 * MINUTE);
        assert!(w.plants().any(|p| p.species == moss));
        w
    };
    let (a, b) = (grow(9), grow(9));
    assert_eq!(a.state_hash(), b.state_hash());
    assert_ne!(a.state_hash(), grow(10).state_hash());
}

/// One plant of each named species grown alone for 10 minutes in its own
/// shallow tank under a lid `rows` thick (0: none). Returns each one's
/// size and limit.
fn grow_alone(names: &[&str], rows: usize, seed: u64) -> Vec<(u32, Limit)> {
    names
        .iter()
        .map(|name| {
            let mut w = bedded(48, 6, seed);
            if rows > 0 {
                lid(&mut w, 0, 160, rows);
            }
            let s = species(&w, name);
            w.paint_plant(80, 30, 0, s, Propagule::Cuttings).unwrap();
            run(&mut w, 10 * MINUTE);
            check_table(&w);
            let plants = plant_of(&w, s);
            let size = plants.iter().map(desktank_sim::PlantInfo::size).sum();
            (size, plants.first().map_or(Limit::None, |p| p.limit))
        })
        .collect()
}

#[test]
fn ludwigia_cabomba_and_bacopa_grow_by_their_care() {
    // Measured (seeds 0–1, cells after 10 minutes): open tank, ludwigia
    // 135–136, cabomba 218–225, bacopa 104; under a 5-row rock lid
    // (light about 0.4), ludwigia 110, cabomba 66–67, bacopa 99–100.
    let names = ["ludwigia", "cabomba", "bacopa"];
    for seed in 0..2 {
        let open = grow_alone(&names, 0, seed);
        let [(lud, _), (cab, _), (bac, _)] = open[..] else {
            unreachable!()
        };
        // In the open tank's light the fast cabomba leads and the slow
        // bacopa trails (all three are held back by CO₂ there, not light).
        assert!(cab > lud && lud > bac && bac > 50, "open: {open:?}");
        let shaded = grow_alone(&names, 5, seed);
        let [(s_lud, _), (s_cab, cab_limit), (s_bac, _)] = shaded[..] else {
            unreachable!()
        };
        // Cabomba needs fairly strong light: shaded, it is the smallest,
        // under half its open size and held back by light; undemanding
        // bacopa hardly notices.
        assert_eq!(cab_limit, Limit::TooLow(Factor::Light), "{shaded:?}");
        assert!(
            s_cab * 2 < cab && s_cab < s_bac && s_cab < s_lud,
            "{shaded:?}"
        );
        assert!(s_bac * 10 >= bac * 8, "bacopa open {bac}, shaded {s_bac}");
    }
}

#[test]
fn christmas_moss_grows_over_rock_slower_than_java_moss() {
    // Measured (seed 5): java moss grows 24 cells in 10 minutes, christmas
    // moss 16.
    let mut sizes = Vec::new();
    for name in ["java_moss", "christmas_moss"] {
        let mut w = world(160, 120, 5);
        let rock = id(&w, "rock");
        w.fill_rect(rect(60, 70, 100, 90), rock, Brush::Replace)
            .unwrap();
        let body = w.body_at(60, 70).unwrap();
        assert!(w.set_anchored(body, true));
        let moss = species(&w, name);
        w.paint_plant(80, 60, 2, moss, Propagule::Fragments)
            .unwrap();
        run(&mut w, MINUTE);
        let start: u32 = plant_of(&w, moss)
            .iter()
            .map(desktank_sim::PlantInfo::size)
            .sum();
        run(&mut w, 10 * MINUTE);
        check_table(&w);
        let end: u32 = plant_of(&w, moss)
            .iter()
            .map(desktank_sim::PlantInfo::size)
            .sum();
        assert!(start > 0 && end > start, "{name}: {start} -> {end}");
        sizes.push(end - start);
    }
    assert!(
        sizes[1] < sizes[0],
        "java moss {}, christmas moss {}",
        sizes[0],
        sizes[1]
    );
}
