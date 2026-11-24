//! Natural schooling (SPEC §2.4 *Schooling*): a school splits into groups
//! that travel apart and rejoins, a fright ripples through it from the fish
//! that saw the threat, and its fish never aim outside the side walls.
#![allow(clippy::cast_precision_loss, clippy::many_single_char_names)]

use desktank_sim::pursuit::FLEE_SENSE;
use desktank_sim::{Creature, MaterialTable, SpeciesId, TICKS_PER_SECOND, World, WorldConfig};

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

fn run(w: &mut World, ticks: u32) {
    for _ in 0..ticks {
        w.tick();
    }
}

fn species(w: &World, name: &str) -> SpeciesId {
    w.species().id(name).unwrap()
}

/// The sizes of the groups `points` fall into when any two within `link`
/// of each other are joined (single linkage), largest first.
fn clusters(points: &[(f32, f32)], link: f32) -> Vec<usize> {
    let mut group: Vec<usize> = (0..points.len()).collect();
    for i in 0..points.len() {
        for j in 0..i {
            let d = (points[i].0 - points[j].0).hypot(points[i].1 - points[j].1);
            if d <= link {
                let (from, to) = (group[i].max(group[j]), group[i].min(group[j]));
                for g in &mut group {
                    if *g == from {
                        *g = to;
                    }
                }
            }
        }
    }
    let mut sizes: Vec<usize> = (0..points.len())
        .map(|g| group.iter().filter(|&&x| x == g).count())
        .filter(|&n| n > 0)
        .collect();
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    sizes
}

/// What a school is doing on a tick: in `groups` of at least two fish
/// held apart by more than `link`, or in one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    One,
    Split,
    Straggling,
}

fn shape_of(w: &World, link: f32) -> Shape {
    let pts: Vec<_> = w.creatures().iter().map(Creature::centre).collect();
    let c = clusters(&pts, link);
    match (c.len(), c.get(1)) {
        (1, _) => Shape::One,
        (_, Some(&n)) if n >= 2 => Shape::Split,
        _ => Shape::Straggling,
    }
}

/// A school of ten neon tetras in a wide tank over twenty minutes divides
/// into groups that travel apart (two or more groups of at least two fish,
/// more than four body lengths from each other) a good part of the time,
/// and comes back together after each: it is one group most of the time.
#[test]
fn a_school_splits_into_groups_that_travel_apart_and_rejoins() {
    let ticks = 20 * 60 * TICKS_PER_SECOND;
    for seed in [4, 77, 123] {
        let mut w = world(640, 120, seed);
        let s = species(&w, "neon_tetra");
        let length = w.species().get(s).unwrap().size().0 as f32;
        for i in 0..10 {
            w.add_creature(s, 300 + 6 * i, 45 + (i % 3) * 8).unwrap();
        }
        run(&mut w, 1_000);
        let (mut split, mut one) = (0, 0);
        // Splits that end with the school one group again.
        let (mut splits, mut rejoins) = (0, 0);
        let mut apart = false;
        for _ in 0..ticks {
            w.tick();
            let now = shape_of(&w, 4.0 * length);
            split += u32::from(now == Shape::Split);
            one += u32::from(now == Shape::One);
            if now == Shape::Split && !apart {
                (apart, splits) = (true, splits + 1);
            }
            if now == Shape::One && apart {
                (apart, rejoins) = (false, rejoins + 1);
            }
        }
        eprintln!(
            "seed {seed}: split {split}, one group {one} of {ticks} ticks, {splits} splits, {rejoins} rejoins"
        );
        assert!(split * 20 >= ticks, "split on {split} ticks");
        assert!(split * 2 <= ticks, "split on {split} ticks");
        assert!(one * 5 >= ticks * 3, "one group on {one} ticks");
        assert!(splits >= 5, "split {splits} times");
        assert!(rejoins * 5 >= splits * 4, "rejoined {rejoins} of {splits}");
    }
}
/// Ten neons in a loose row with a hungry flowerhorn 20 cells from the
/// nearest: the tick each took fright (if it did) and how far it was from
/// the predator then, in cells.
fn fright_onsets(seed: u64, predator: bool) -> Vec<Option<(u32, f32)>> {
    let mut w = world(320, 120, seed);
    let s = species(&w, "neon_tetra");
    let fish: Vec<_> = (0..10)
        .map(|i| w.add_creature(s, 120 + 6 * i, 58 + (i % 3) * 4).unwrap())
        .collect();
    let hunter = predator.then(|| w.add_creature(species(&w, "flowerhorn"), 100, 60).unwrap());
    let mut onset = vec![None; fish.len()];
    for t in 0..200 {
        w.tick();
        let there = hunter.map_or((0.0, 0.0), |h| w.creature(h).unwrap().centre());
        for (o, &id) in onset.iter_mut().zip(&fish) {
            if let Some(c) = w.creature(id).filter(|c| o.is_none() && c.fear() > 0) {
                let (x, y) = c.centre();
                *o = Some((t, (x - there.0).hypot(y - there.1)));
            }
        }
    }
    onset
}

/// A fright ripples through a school: the fish a hunter has come for take
/// fright first, and the rest catch it from their mates, later the further
/// they are, though the hunter is nowhere near them.
#[test]
fn a_fright_ripples_through_the_school() {
    let sense = FLEE_SENSE as f32;
    // The school's calm without a hunter.
    assert!(fright_onsets(1, false).iter().all(Option::is_none));
    for seed in [1, 2, 5, 6] {
        let mut seen: Vec<_> = fright_onsets(seed, true).into_iter().flatten().collect();
        seen.sort_by(|a, b| a.1.total_cmp(&b.1));
        let first = seen.iter().map(|o| o.0).min().unwrap();
        let (near, far) = seen.split_at(seen.len() / 2);
        let mean =
            |fish: &[(u32, f32)]| fish.iter().map(|o| o.0 as f32).sum::<f32>() / fish.len() as f32;
        eprintln!(
            "seed {seed}: {} frightened, first at {first}, nearer half {:.1}, further {:.1}, furthest {:.0} cells",
            seen.len(),
            mean(near),
            mean(far),
            seen.last().unwrap().1
        );
        // Most of the school took fright, some of it caught it from mates
        // well beyond the range a hunter frightens from...
        assert!(seen.len() >= 8, "{} fish took fright", seen.len());
        let caught: Vec<_> = seen.iter().filter(|o| o.1 > 2.0 * sense).collect();
        assert!(caught.len() >= 2, "{} fish caught it", caught.len());
        // ...later than the first, and later the further from the hunter.
        assert!(caught.iter().all(|o| o.0 > first), "{caught:?}");
        assert!(mean(far) > mean(near) + 3.0, "{seen:?}");
        // And the ripple takes two seconds at most to cross the school.
        assert!(
            seen.iter().all(|o| o.0 <= first + 2 * TICKS_PER_SECOND),
            "{seen:?}"
        );
    }
}

/// Followers never aim outside the side walls: in a tank barely wider
/// than a few body lengths a school never presses against the glass (a
/// slot beyond a wall once pulled fish onto it, thousands of fish-ticks).
#[test]
fn a_school_never_presses_against_the_side_walls() {
    for seed in [1, 2, 3] {
        let width = 64;
        let mut w = world(width, 100, seed);
        let s = species(&w, "neon_tetra");
        // A body centre is clear of the glass at half its length and a cell.
        let hw = (w.species().get(s).unwrap().size().0 / 2 + 1) as f32;
        for i in 0..8 {
            w.add_creature(s, 8 + 6 * i, 45 + (i % 3) * 6).unwrap();
        }
        let mut pressed = 0;
        for _ in 0..12_000 {
            w.tick();
            pressed += w
                .creatures()
                .iter()
                .filter(|c| c.centre().0 < hw || c.centre().0 > width as f32 - hw)
                .count();
        }
        eprintln!("seed {seed}: {pressed} fish-ticks against a wall");
        assert_eq!(pressed, 0, "seed {seed}");
    }
}
