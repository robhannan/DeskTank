//! Repeated feeding of a neon tetra school (#434, part of #432).
//!
//! Each seeded school runs continuously, so every fish carries its feeding
//! history forward, and one flake drops on alternating sides. The pilot
//! keeps what the earlier choices got right (every flake found within a few
//! seconds, a school that holds together and doesn't ignore food) and adds
//! what a single flake can show: a fish that has just eaten hangs back
//! with the school, hungrier schoolmates go, and once everyone is hungry
//! again nobody is held back. `examples/tetra_feeding.rs` measures the same
//! things against older builds; see `docs/notes/tetra-feeding-pilot.md`.
#![allow(clippy::cast_precision_loss)]

use desktank_sim::{
    Brush, Creature, CreatureId, MaterialTable, TICKS_PER_SECOND, World, WorldConfig, save,
};

const FISH: usize = 6;
const DROP_Y: i64 = 2;
const WARMUP: u32 = 12 * TICKS_PER_SECOND;
const EARLY: u32 = 2 * TICKS_PER_SECOND;
const BITE_WINDOW: u32 = 10 * TICKS_PER_SECOND;

fn school(seed: u64) -> (World, Vec<CreatureId>) {
    let mut world = World::new(
        WorldConfig {
            width: 160,
            height: 90,
            seed,
            water_level: 0.96,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap();
    let tetra = world.species().id("neon_tetra").unwrap();
    let ids = (0..6_i64)
        .map(|i| {
            world
                .add_creature(tetra, 64 + 6 * i, 39 + 5 * (i % 3))
                .unwrap()
        })
        .collect();
    // Begin the repeated feeding trials with a hungry school.
    world.set_time_ratio(desktank_sim::MAX_TIME_RATIO);
    for _ in 0..18 {
        world.tick();
    }
    world.set_time_ratio(1.0);
    for _ in 18..WARMUP {
        world.tick();
    }
    (world, ids)
}

fn drop_x(index: usize) -> i64 {
    if index.is_multiple_of(2) { 54 } else { 106 }
}

fn distance(fish: &Creature, x: i64) -> f32 {
    let (fx, fy) = fish.centre();
    (fx - x as f32).hypot(fy - DROP_Y as f32)
}

/// What one flake did to the school.
#[derive(Clone, Copy, Default)]
struct Flake {
    /// Fish at least ten cells nearer the drop two seconds later.
    approached: [bool; FISH],
    /// Fish that bit at any point before the next flake.
    fed: [bool; FISH],
    /// The first fish to bite and when, within ten seconds.
    first: Option<(usize, u32)>,
}

/// Drop one flake at `x` and run `seconds`, clearing anything left.
fn feed(world: &mut World, ids: &[CreatureId], x: i64, seconds: u32) -> Flake {
    let flake = world.materials().id("flake_food").unwrap();
    let water = world.materials().id("water").unwrap();
    world.paint(x, DROP_Y, 0, flake, Brush::Pour).unwrap();
    let before: Vec<(f32, u32)> = ids
        .iter()
        .map(|&id| {
            let fish = world.creature(id).unwrap();
            (distance(fish, x), fish.eaten())
        })
        .collect();
    let mut out = Flake::default();
    for t in 1..=seconds * TICKS_PER_SECOND {
        world.tick();
        for (i, &id) in ids.iter().enumerate() {
            let fish = world.creature(id).unwrap();
            if t <= EARLY && before[i].0 - distance(fish, x) >= 10.0 {
                out.approached[i] = true;
            }
            if fish.eaten() > before[i].1 {
                out.fed[i] = true;
                if t <= BITE_WINDOW && out.first.is_none() {
                    out.first = Some((i, t));
                }
            }
        }
    }
    let width = world.width();
    let leftovers: Vec<usize> = world
        .cells()
        .iter()
        .enumerate()
        .filter(|(_, cell)| cell.material == flake)
        .map(|(i, _)| i)
        .collect();
    for i in leftovers {
        world.set_cell(i % width, i / width, water).unwrap();
    }
    out
}

/// Every flake of `flakes` presentations for each of `seeds` schools.
fn runs(seeds: u64, flakes: usize, seconds: u32) -> Vec<Vec<Flake>> {
    (1..=seeds)
        .map(|seed| {
            let (mut world, ids) = school(seed);
            (0..flakes)
                .map(|n| feed(&mut world, &ids, drop_x(n), seconds))
                .collect()
        })
        .collect()
}

/// Of the fish that did and did not bite the previous flake, the share of
/// each that approached this one.
fn approach_after(runs: &[Vec<Flake>]) -> (f64, f64) {
    let (mut after, mut after_n, mut other, mut other_n) = (0, 0, 0, 0);
    for run in runs {
        for pair in run.windows(2) {
            for i in 0..FISH {
                let moved = usize::from(pair[1].approached[i]);
                if pair[0].fed[i] {
                    (after, after_n) = (after + moved, after_n + 1);
                } else {
                    (other, other_n) = (other + moved, other_n + 1);
                }
            }
        }
    }
    (
        after as f64 / f64::from(after_n),
        other as f64 / f64::from(other_n),
    )
}

#[test]
fn every_flake_is_found_quickly_and_the_school_does_not_ignore_it() {
    let runs = runs(8, 8, 15);
    let flakes: Vec<_> = runs.iter().flatten().collect();
    let eaten = flakes.iter().filter(|f| f.first.is_some()).count();
    let seconds: f64 = flakes
        .iter()
        .filter_map(|f| f.first)
        .map(|(_, t)| f64::from(t) / f64::from(TICKS_PER_SECOND))
        .sum();
    let ignored = flakes
        .iter()
        .filter(|f| f.approached.iter().all(|&a| !a))
        .count();
    assert!(
        eaten * 100 >= flakes.len() * 95,
        "{eaten} of {}",
        flakes.len()
    );
    assert!(seconds / eaten as f64 <= 6.0, "{seconds} s over {eaten}");
    assert!(ignored * 100 <= flakes.len() * 12, "{ignored} ignored");
}

#[test]
fn a_fish_that_just_ate_hangs_back_while_hungrier_schoolmates_go() {
    let runs = runs(12, 12, 15);
    let (after_meal, otherwise) = approach_after(&runs);
    assert!(
        after_meal + 0.12 < otherwise,
        "{after_meal:.2} after a meal, {otherwise:.2} otherwise"
    );
    // The fish that bit first last time rarely does so again: the chance
    // of that for six fish at random is one in six.
    let (mut repeats, mut pairs) = (0, 0);
    for run in &runs {
        for pair in run.windows(2) {
            if let (Some((a, _)), Some((b, _))) = (pair[0].first, pair[1].first) {
                pairs += 1;
                repeats += usize::from(a == b);
            }
        }
    }
    assert!(repeats * 100 <= pairs * 20, "{repeats} repeats in {pairs}");
    // Hanging back is a lean, not a rule: some still go, and they eat.
    assert!(after_meal > 0.1, "{after_meal:.2}");
    let firsts: Vec<_> = runs
        .iter()
        .map(|run| run.iter().filter_map(|f| f.first.map(|(i, _)| i)))
        .collect();
    for first in firsts {
        let mut seen = [false; FISH];
        first.for_each(|i| seen[i] = true);
        assert!(seen.iter().filter(|&&s| s).count() >= 3, "{seen:?}");
    }
}

#[test]
fn after_a_long_wait_everyone_is_hungry_again_and_goes() {
    let (quick, _) = approach_after(&runs(12, 8, 15));
    let long = runs(12, 6, 90);
    let (after_meal, otherwise) = approach_after(&long);
    // A bite lasts about half a minute: after 90 s nobody is held back.
    assert!(
        after_meal + 0.1 >= otherwise,
        "{after_meal:.2} vs {otherwise:.2}"
    );
    assert!(
        after_meal > quick + 0.2,
        "{after_meal:.2} after 90 s, {quick:.2} after 15 s"
    );
    let flakes = long.iter().flatten();
    let approaches: usize = flakes
        .clone()
        .map(|f| f.approached.iter().filter(|&&a| a).count())
        .sum();
    let mean = approaches as f64 / flakes.count() as f64;
    assert!(mean >= 3.0, "{mean:.2} fish approach on average");
}

#[test]
fn with_no_food_the_school_stays_together_and_swims_without_jitter() {
    let (mut world, ids) = school(3);
    let mut facing: Vec<bool> = ids
        .iter()
        .map(|&id| world.creature(id).unwrap().facing_left())
        .collect();
    let (mut flips, mut gaps, mut gap_sum) = (0_u32, 0_u32, 0.0_f32);
    for _ in 0..120 * TICKS_PER_SECOND {
        world.tick();
        for (i, &id) in ids.iter().enumerate() {
            let fish = world.creature(id).unwrap();
            flips += u32::from(fish.facing_left() != facing[i]);
            facing[i] = fish.facing_left();
            let (x, y) = fish.centre();
            gap_sum += ids
                .iter()
                .filter(|&&m| m != id)
                .map(|&m| {
                    let (mx, my) = world.creature(m).unwrap().centre();
                    (x - mx).hypot(y - my)
                })
                .fold(f32::MAX, f32::min);
            gaps += 1;
        }
    }
    let per_fish_minute = flips as f32 / (FISH as f32 * 2.0);
    assert!(
        per_fish_minute <= 18.0,
        "{per_fish_minute} turns per fish a minute"
    );
    let gap = gap_sum / gaps as f32;
    assert!(gap <= 10.0, "nearest mates {gap:.1} cells apart");
}

#[test]
fn repeated_feeding_replays_and_resumes_from_a_save() {
    let play = |save_at: Option<usize>| {
        let (mut world, ids) = school(5);
        for n in 0..4 {
            feed(&mut world, &ids, drop_x(n), 15);
            if save_at == Some(n) {
                let bytes = save::encode(&world);
                world = save::decode(&bytes, MaterialTable::builtin().unwrap()).unwrap();
            }
        }
        world.state_hash()
    };
    let straight = play(None);
    assert_eq!(play(None), straight);
    assert_eq!(play(Some(1)), straight);
}
