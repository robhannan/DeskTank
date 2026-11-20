//! Hunting, fleeing to cover and territory (SPEC §2.4 *Hunting and
//! territory*): a hungry predator hunts down prey that wandering would
//! never bring it to; prey hiding in plants survives more often; nothing
//! hunts in easy mode; a territorial betta drives an intruder out of its
//! territory and goes home; peaceful fish carry on as before; and all of it
//! is deterministic and survives a save mid-chase.
#![allow(clippy::cast_possible_wrap, clippy::cast_precision_loss)]

use desktank_sim::pursuit::{DRIVE_TICKS, SENSE_EVERY};
use desktank_sim::{
    Brush, CreatureId, MaterialTable, Rect, SpeciesId, TICKS_PER_SECOND, World, WorldConfig,
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

/// Cells between two creatures' bodies (0 when they touch or overlap).
fn gap(w: &World, a: CreatureId, b: CreatureId) -> usize {
    let (a, b) = (
        w.creature_body(w.creature(a).unwrap()),
        w.creature_body(w.creature(b).unwrap()),
    );
    let apart = |lo0: usize, hi0: usize, lo1: usize, hi1: usize| {
        lo1.saturating_sub(hi0).max(lo0.saturating_sub(hi1))
    };
    apart(a.x0, a.x1, b.x0, b.x1).max(apart(a.y0, a.y1, b.y0, b.y1))
}

/// A betta near the surface and a cherry shrimp on the bed 30 cells below
/// and 12 along: their zones never overlap, so wandering never brings them
/// together.
fn betta_over_shrimp(seed: u64) -> (World, CreatureId, CreatureId) {
    let mut w = world(160, 48, seed);
    let betta = w.add_creature(species(&w, "betta"), 60, 6).unwrap();
    let shrimp = w
        .add_creature(species(&w, "cherry_shrimp"), 72, 40)
        .unwrap();
    (w, betta, shrimp)
}

#[test]
fn a_hungry_predator_hunts_down_prey_that_wandering_never_meets() {
    let minutes = 2 * 60 * TICKS_PER_SECOND;
    let mut eaten = 0;
    for seed in 1..=8 {
        // In easy mode nothing hunts: the betta wanders its zone and never
        // comes within reach of the shrimp.
        let (mut calm, betta, shrimp) = betta_over_shrimp(seed);
        calm.set_easy_mode(true);
        let mut closest = usize::MAX;
        for _ in 0..minutes {
            calm.tick();
            closest = closest.min(gap(&calm, betta, shrimp));
            assert!(calm.creature(betta).unwrap().quarry().is_none());
            assert_eq!(calm.creature(shrimp).unwrap().fear(), 0);
        }
        assert!(closest > 4, "seed {seed}: wandered within {closest} cells");
        assert!(calm.creature(shrimp).is_some());

        // Hungry (as every new arrival is), it hunts the shrimp down.
        let (mut w, betta, shrimp) = betta_over_shrimp(seed);
        let mut chased = false;
        let mut frightened = false;
        for _ in 0..minutes {
            w.tick();
            let Some(prey) = w.creature(shrimp) else {
                eaten += 1;
                break;
            };
            frightened |= prey.fear() > 0;
            chased |= w.creature(betta).unwrap().quarry() == Some(shrimp);
        }
        assert!(chased, "seed {seed}: the betta never gave chase");
        assert!(frightened, "seed {seed}: the shrimp never fled");
        assert!(w.corpses().is_empty(), "eaten whole");
    }
    eprintln!("eaten in {eaten} of 8");
    assert!(eaten >= 6, "eaten in {eaten} of 8 tanks within two minutes");
}

/// Fry left with a hungry adult guppy for three minutes, in an open tank
/// or one with a stand of plants beside them: how many survive.
fn fry_left(seed: u64, planted: bool) -> usize {
    let mut w = world(120, 64, seed);
    if planted {
        let soil = w.materials().id("aquasoil").unwrap();
        w.fill_rect(
            Rect {
                x0: 0,
                y0: 58,
                x1: 120,
                y1: 64,
            },
            soil,
            Brush::Replace,
        )
        .unwrap();
        for x in (30..=90).step_by(6) {
            let kind = w.plant_species().id("hygrophila").unwrap();
            let from = w.plant_species().get(kind).unwrap().default_propagule();
            w.paint_plant(x, 57, 0, kind, from).unwrap();
        }
        for _ in 0..6 * 60 * TICKS_PER_SECOND {
            w.tick();
        }
    }
    let guppy = species(&w, "guppy");
    let fry: Vec<_> = (0..6)
        .map(|i| w.add_creature_aged(guppy, 36 + i * 9, 40, 0).unwrap())
        .collect();
    w.add_creature(guppy, 60, 20).unwrap();
    for _ in 0..3 * 60 * TICKS_PER_SECOND {
        w.tick();
    }
    fry.iter().filter(|&&f| w.creature(f).is_some()).count()
}

#[test]
fn fry_hiding_in_plants_survive_more_often() {
    let (mut open, mut planted) = (0, 0);
    for seed in 1..=6 {
        let (o, p) = (fry_left(seed, false), fry_left(seed, true));
        eprintln!("seed {seed}: {o} of 6 fry left in the open, {p} among plants");
        open += o;
        planted += p;
    }
    assert!(open < 36, "the guppy ate no fry in the open");
    assert!(
        planted >= open + 6,
        "{planted} fry left among plants, {open} in the open"
    );
}

/// A territorial betta settled at (40, 8) with a honey gourami (too big to
/// eat, a surface swimmer) put down 12 cells from it.
fn betta_and_intruder(seed: u64) -> (World, CreatureId, CreatureId) {
    let mut w = world(200, 48, seed);
    let betta = w.add_creature(species(&w, "betta"), 40, 8).unwrap();
    for _ in 0..SENSE_EVERY {
        w.tick();
    }
    let gourami = w.add_creature(species(&w, "honey_gourami"), 52, 8).unwrap();
    (w, betta, gourami)
}

#[test]
fn a_territorial_betta_drives_an_intruder_out_and_goes_home() {
    for seed in 1..=4 {
        let (mut w, betta, gourami) = betta_and_intruder(seed);
        let home = w.creature(betta).unwrap().home().expect("claimed a home");
        let radius = 30.0; // territory_cm 15 at 2 cells per cm
        let from_home = |w: &World, id| {
            let (x, y) = w.creature(id).unwrap().centre();
            ((x - home.0 as f32).powi(2) + (y - home.1 as f32).powi(2)).sqrt()
        };
        let mut chased_at = None;
        let mut fled = false;
        let mut furthest = 0.0_f32;
        for t in 0..20 * TICKS_PER_SECOND {
            w.tick();
            if w.creature(betta).unwrap().quarry() == Some(gourami) {
                chased_at.get_or_insert(t);
            }
            fled |= w.creature(gourami).unwrap().fear() > 0;
            furthest = furthest.max(from_home(&w, gourami));
        }
        let chased_at = chased_at.unwrap_or_else(|| panic!("seed {seed}: no chase"));
        assert!(chased_at < TICKS_PER_SECOND, "seed {seed}: chased late");
        assert!(fled, "seed {seed}: the gourami never fled");
        assert!(
            furthest > radius,
            "seed {seed}: gourami kept within {furthest}"
        );
        // The betta keeps to its territory: it ends the chase and swims
        // back, and is home within a few seconds of any drive.
        assert_eq!(w.creature(betta).unwrap().home(), Some(home));
        let mut back = false;
        for _ in 0..u32::from(DRIVE_TICKS) + 10 * TICKS_PER_SECOND {
            w.tick();
            back |= w.creature(betta).unwrap().quarry().is_none() && from_home(&w, betta) < radius;
        }
        assert!(back, "seed {seed}: the betta never went home");
    }
}

#[test]
fn peaceful_fish_neither_hunt_nor_flee_nor_keep_territory() {
    let mut w = world(200, 64, 3);
    for (name, x) in [
        ("neon_tetra", 30),
        ("neon_tetra", 40),
        ("corydoras", 60),
        ("harlequin_rasbora", 80),
        ("harlequin_rasbora", 90),
        ("amano_shrimp", 120),
    ] {
        let s = species(&w, name);
        let row = i64::try_from(w.zone_rows(s).unwrap().0).unwrap() + 4;
        w.add_creature(s, x, row).unwrap();
    }
    for _ in 0..3 * 60 * TICKS_PER_SECOND {
        w.tick();
        for c in w.creatures() {
            assert!(c.quarry().is_none() && c.fear() == 0 && c.home().is_none());
        }
    }
    assert_eq!(w.creatures().len(), 6);
}

#[test]
fn hunting_and_territory_are_deterministic_and_survive_a_save_mid_chase() {
    let run = |seed| {
        let (mut w, betta, _) = betta_and_intruder(seed);
        // A predator hunting too: a hungry guppy with fry in sight.
        let guppy = species(&w, "guppy");
        w.add_creature(guppy, 150, 20).unwrap();
        for x in [130, 170] {
            w.add_creature_aged(guppy, x, 24, 0).unwrap();
        }
        for _ in 0..60 * TICKS_PER_SECOND {
            if w.creature(betta).unwrap().quarry().is_some() {
                break;
            }
            w.tick();
        }
        w
    };
    let (mut a, b) = (run(5), run(5));
    assert_eq!(a.state_hash(), b.state_hash());
    assert!(a.creatures().iter().any(|c| c.quarry().is_some()));
    let mut loaded = desktank_sim::save::decode(
        &desktank_sim::save::encode(&a),
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    assert_eq!(loaded.state_hash(), a.state_hash());
    for _ in 0..30 * TICKS_PER_SECOND {
        a.tick();
        loaded.tick();
    }
    assert_eq!(loaded.state_hash(), a.state_hash());
}
