//! Live food (SPEC §2.4 *Live food*): a pinch of live bloodworms from the
//! Food palette group sinks to the bed and wriggles along it; every stocked
//! species that eats frozen bloodworms hunts the live ones down and eats
//! them, in easy mode too, while species that don't leave them alone; an
//! uneaten worm dies into a cell of frozen bloodworm, which rots; live food
//! never breeds, keeps to its own limit beside the population cap, isn't
//! stocked or spawned, and is deterministic across a save mid-chase.
#![allow(clippy::cast_possible_wrap, clippy::cast_precision_loss)]

use desktank_sim::tools::{Button, Outcome, PaletteGroup, PaletteItem, Tool, Tools};
use desktank_sim::{
    Brush, CreatureId, LIFE_DAY_TICKS, MAX_LIVE_FOOD, MaterialTable, PINCH, Rect, SpeciesId,
    TICKS_PER_SECOND, World, WorldConfig, WorldError, cheats, scene,
};

fn world(width: usize, height: usize, seed: u64) -> World {
    let mut w = World::new(
        WorldConfig {
            width,
            height,
            seed,
            water_level: 0.95,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    let sand = w.materials().id("sand").unwrap();
    w.fill_rect(
        Rect {
            x0: 0,
            y0: height - 6,
            x1: width,
            y1: height,
        },
        sand,
        Brush::Replace,
    )
    .unwrap();
    w
}

fn species(w: &World, name: &str) -> SpeciesId {
    w.species().id(name).unwrap()
}

fn worms(w: &World) -> Vec<CreatureId> {
    let worm = species(w, "live_bloodworm");
    w.creatures()
        .iter()
        .filter(|c| c.species() == worm)
        .map(desktank_sim::Creature::id)
        .collect()
}

fn count(w: &World, name: &str) -> usize {
    let m = w.materials().id(name).unwrap();
    (0..w.height())
        .flat_map(|y| (0..w.width()).map(move |x| (x, y)))
        .filter(|&(x, y)| w.cell(x, y).unwrap().material == m)
        .count()
}

/// Selects the live bloodworm slot in the Food group and clicks at `cell`.
fn pinch(w: &mut World, cell: (i64, i64)) -> Outcome {
    let worm = species(w, "live_bloodworm");
    let item = PaletteItem::Tool(Tool::AddCreature(worm));
    let mut t = Tools::new(w.materials().id("sand").unwrap());
    t.select(item);
    let out = t.press(w, Button::Primary, cell);
    t.release(w, Button::Primary);
    out
}

#[test]
fn live_food_is_offered_in_the_food_group_only() {
    let w = world(160, 60, 1);
    let worm = PaletteItem::Tool(Tool::AddCreature(species(&w, "live_bloodworm")));
    for (group, items) in Tools::palette_groups(&w) {
        assert_eq!(
            items.contains(&worm),
            group == PaletteGroup::Food,
            "{group:?}"
        );
    }
}

#[test]
fn a_pinch_sinks_to_the_bed_and_wriggles_along_it() {
    let mut w = world(160, 60, 3);
    let Outcome::Added(_) = pinch(&mut w, (80, 12)) else {
        panic!("no pinch added");
    };
    let ids = worms(&w);
    assert_eq!(ids.len(), PINCH);
    let start: Vec<_> = ids
        .iter()
        .map(|&id| w.creature(id).unwrap().centre())
        .collect();
    // They sink 40 rows to the bed within 30 s.
    for _ in 0..30 * TICKS_PER_SECOND {
        w.tick();
    }
    let bed = w.height() - 6;
    for &id in &ids {
        let c = w.creature(id).unwrap();
        assert_eq!(w.creature_body(c).y1, bed, "worm {id:?} on the bed");
    }
    // Then they crawl along it over a minute, staying on it.
    let mut moved = vec![0.0_f32; ids.len()];
    for _ in 0..60 * TICKS_PER_SECOND {
        w.tick();
        for (k, &id) in ids.iter().enumerate() {
            let c = w.creature(id).unwrap();
            moved[k] = moved[k].max((c.centre().0 - start[k].0).abs());
            assert!(w.creature_body(c).y1 >= bed - 1, "worm {id:?} left the bed");
        }
    }
    for (k, m) in moved.iter().enumerate() {
        assert!(*m >= 3.0, "worm {k} crawled only {m} cells");
    }
}

/// The stocked species whose diet holds frozen bloodworms.
fn bloodworm_eaters(w: &World) -> Vec<SpeciesId> {
    let frozen = w.materials().id("bloodworm").unwrap();
    w.species()
        .iter()
        .filter(|s| s.stocked() && s.eats(frozen))
        .map(|s| s.id)
        .collect()
}

#[test]
fn every_bloodworm_eater_hunts_down_live_ones_in_easy_mode_too() {
    let probe = world(200, 60, 0);
    let eaters = bloodworm_eaters(&probe);
    for name in [
        "neon_tetra",
        "angelfish",
        "honey_gourami",
        "kuhli_loach",
        "harlequin_rasbora",
        "amano_shrimp",
        "african_dwarf_frog",
        "axolotl",
        "pea_puffer",
    ] {
        assert!(eaters.contains(&species(&probe, name)), "{name}");
    }
    for &s in &eaters {
        for easy in [false, true] {
            let mut w = world(200, 60, 7);
            w.set_easy_mode(easy);
            let name = w.species().get(s).unwrap().name.clone();
            // The eater at one end mid-water, a pinch 60 cells along.
            let fish = w.add_creature(s, 50, 28).unwrap();
            pinch(&mut w, (110, 40));
            let mut first = None;
            for t in 0..120 * TICKS_PER_SECOND {
                w.tick();
                if first.is_none() && worms(&w).len() < PINCH {
                    first = Some(t);
                }
                if worms(&w).is_empty() {
                    break;
                }
            }
            let first = first.unwrap_or_else(|| panic!("{name} (easy {easy}) ate none"));
            let secs = first as f32 / TICKS_PER_SECOND as f32;
            eprintln!("{name} (easy {easy}): first worm at {secs:.1} s");
            assert!(secs <= 60.0, "{name} (easy {easy}): first at {secs} s");
            assert!(w.creature(fish).unwrap().eaten() >= 1);
        }
    }
}

#[test]
fn species_that_dont_eat_bloodworms_leave_live_ones_alone() {
    let mut w = world(200, 60, 2);
    let frozen = w.materials().id("bloodworm").unwrap();
    let others: Vec<_> = w
        .species()
        .iter()
        .filter(|s| s.stocked() && !s.eats(frozen) && s.length_cm <= 8.0)
        .map(|s| s.id)
        .collect();
    assert!(others.contains(&species(&w, "cherry_shrimp")));
    for (k, &s) in others.iter().enumerate() {
        w.add_creature(s, 20 + 20 * k as i64, 40).unwrap();
    }
    pinch(&mut w, (100, 40));
    for _ in 0..120 * TICKS_PER_SECOND {
        w.tick();
    }
    assert_eq!(worms(&w).len(), PINCH);
}

#[test]
fn an_uneaten_worm_dies_into_frozen_bloodworm_which_rots() {
    let mut w = world(120, 40, 4);
    pinch(&mut w, (60, 20));
    assert_eq!(count(&w, "bloodworm"), 0);
    // A worm lives about an hour of world time (6 life days, ±10%).
    let limit = u64::from(7 * LIFE_DAY_TICKS);
    let mut most = 0;
    let mut t = 0;
    while !worms(&w).is_empty() {
        assert!(t < limit, "a worm outlived its lifespan");
        w.tick();
        t += 1;
        if !t.is_multiple_of(u64::from(TICKS_PER_SECOND)) {
            continue;
        }
        // Each death leaves one cell; none rots within the spread of
        // deaths (rot takes 5 minutes, deaths spread over 12).
        let dead = PINCH - worms(&w).len();
        let cells = count(&w, "bloodworm") + count(&w, "detritus");
        assert!(cells <= dead, "{cells} cells from {dead} deaths");
        most = most.max(count(&w, "bloodworm"));
    }
    assert!(most >= 1, "the dead worms left frozen bloodworm");
    assert_eq!(w.corpses().len(), 0, "live food leaves no corpse");
    // Ten minutes on, the frozen bloodworm has rotted away.
    for _ in 0..600 * TICKS_PER_SECOND {
        w.tick();
    }
    assert_eq!(count(&w, "bloodworm"), 0);
    assert_eq!(w.creatures().len(), 0);
}

#[test]
fn live_food_keeps_its_own_limit_and_never_breeds() {
    let mut w = world(240, 60, 5);
    // A full tank of stock (cap 2) still takes live food, and live food
    // never keeps stock out.
    w.set_creature_cap(2);
    let tetra = species(&w, "neon_tetra");
    w.add_creature(tetra, 30, 20).unwrap();
    for k in 0..20 {
        pinch(&mut w, (40 + 9 * k, 40));
    }
    assert_eq!(worms(&w).len(), MAX_LIVE_FOOD);
    assert_eq!(w.live_food_count(), MAX_LIVE_FOOD);
    assert_eq!(
        pinch(&mut w, (120, 40)),
        Outcome::AddRefused {
            species: species(&w, "live_bloodworm"),
            reason: desktank_sim::tools::AddRefused::PopulationCap {
                cap: MAX_LIVE_FOOD,
                live_food: true,
            },
        }
    );
    assert_eq!(
        w.add_creature(species(&w, "live_bloodworm"), 120, 40),
        Err(WorldError::PopulationCap(MAX_LIVE_FOOD))
    );
    w.add_creature(tetra, 60, 20).unwrap();
    assert_eq!(w.stock_count(), 2);
    assert_eq!(
        w.add_creature(tetra, 90, 20),
        Err(WorldError::PopulationCap(2))
    );
    // The cap may come down to the stock, not counting live food.
    assert_eq!(w.set_creature_cap(1), 2);
    // Worms alone never add to their number.
    let mut w = world(160, 40, 6);
    pinch(&mut w, (40, 20));
    pinch(&mut w, (120, 20));
    for _ in 0..20 * 60 * TICKS_PER_SECOND {
        w.tick();
        assert!(w.creatures().len() <= 2 * PINCH);
    }
}

#[test]
fn live_food_is_not_stock() {
    let mut w = world(200, 60, 8);
    scene::stock(&mut w, 40, 8).unwrap();
    assert_eq!(worms(&w), []);
    assert!(cheats::run(&mut w, "spawn live_bloodworm").is_err());
    assert_eq!(worms(&w), []);
    // Live food in the tank doesn't stop `spawn` or `scene::stock` short
    // of the cap: only stock counts against it.
    let mut w = world(240, 60, 9);
    w.set_creature_cap(10);
    for k in 0..4 {
        pinch(&mut w, (40 + 40 * k, 40));
    }
    assert_eq!(worms(&w).len(), 4 * PINCH);
    cheats::run(&mut w, "spawn neon_tetra 6").unwrap();
    assert_eq!(scene::stock(&mut w, 10, 9).unwrap(), 4);
    assert_eq!(w.stock_count(), 10);
}

#[test]
fn a_chased_worm_squirms_away_and_a_save_mid_chase_replays_exactly() {
    let run = |seed| {
        let mut w = world(200, 60, seed);
        let fish = w.add_creature(species(&w, "kuhli_loach"), 40, 48).unwrap();
        pinch(&mut w, (120, 40));
        let mut feared = false;
        for _ in 0..60 * TICKS_PER_SECOND {
            w.tick();
            feared |= w.creatures().iter().any(|c| c.fear() > 0);
            if w.creature(fish).unwrap().quarry().is_some() && feared {
                break;
            }
        }
        (w, feared)
    };
    let ((mut a, feared), (b, _)) = (run(9), run(9));
    assert!(feared, "a chased worm took fright");
    assert_eq!(a.state_hash(), b.state_hash());
    let mut loaded = desktank_sim::save::decode(
        &desktank_sim::save::encode(&a),
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    assert_eq!(loaded.state_hash(), a.state_hash());
    for _ in 0..60 * TICKS_PER_SECOND {
        a.tick();
        loaded.tick();
    }
    assert_eq!(loaded.state_hash(), a.state_hash());
}
