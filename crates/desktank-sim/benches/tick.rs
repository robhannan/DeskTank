//! Simulation tick benches (SPEC §6.1, T1, T5, T6, T7a): a heavy pour must
//! tick in ≤ 5 ms and a settled scene in ≤ 0.5 ms on the reference machine,
//! also with living cells (random ticks) and fields active, with grown
//! plants, and with the reference tank's 30 creatures swimming.
//!
//! `cargo bench -p desktank-sim`

#![allow(missing_docs)] // criterion's macros generate undocumented items

use std::hint::black_box;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use desktank_sim::{Brush, MaterialTable, Rect, World, WorldConfig};

/// The default 640×360 tank with a sand column covering 20% of the grid
/// (160×288 cells) dropped into the middle, above 72 rows of water.
fn heavy_pour() -> World {
    let t = MaterialTable::builtin().expect("builtin materials");
    let sand = t.id("sand").expect("sand");
    let mut w = World::new(WorldConfig::default(), t).expect("world");
    let column = Rect {
        x0: 240,
        y0: 0,
        x1: 400,
        y1: 288,
    };
    assert_eq!(column.area() * 5, w.width() * w.height());
    w.fill_rect(column, sand, Brush::Replace).expect("pour");
    w
}

fn settled_living() -> World {
    let t = MaterialTable::builtin().expect("builtin materials");
    let (soil, tab) = (
        t.id("aquasoil").expect("aquasoil"),
        t.id("root_tab").expect("root_tab"),
    );
    let mut w = World::new(WorldConfig::default(), t).expect("world");
    let bed = Rect {
        x0: 0,
        y0: 348,
        x1: 640,
        y1: 360,
    };
    w.fill_rect(bed, soil, Brush::Replace).expect("bed");
    for x in (16..640).step_by(32) {
        w.set_cell(x, 347, tab).expect("tab");
    }
    while {
        w.tick();
        w.awake_chunks() > 0
    } {}
    assert_eq!(w.random_tick_stats().visited, w.random_tick_budget());
    w
}

/// The living tank with 16 cuttings grown for 30 minutes of world time: a
/// planted tank whose plants have (nearly) finished growing.
fn planted() -> World {
    let mut w = settled_living();
    let cutting = w.materials().id("cutting").expect("cutting");
    for x in (20..640).step_by(40) {
        w.paint(x, 40, 0, cutting, Brush::Pour).expect("cutting");
    }
    advance(w, 30 * 60 * 20)
}

fn advance(mut w: World, ticks: u32) -> World {
    for _ in 0..ticks {
        w.tick();
    }
    w
}

fn bench_tick(c: &mut Criterion) {
    let mut g = c.benchmark_group("tick");
    // Early in the pour (the whole column falling through water) and
    // later (spreading into a pile).
    for (name, ticks) in [("heavy_pour_t10", 10), ("heavy_pour_t60", 60)] {
        let start = advance(heavy_pour(), ticks);
        g.bench_function(name, |b| {
            b.iter_batched_ref(|| start.clone(), World::tick, BatchSize::LargeInput);
        });
    }
    // The same tank once every chunk is asleep.
    let mut settled = heavy_pour();
    while {
        settled.tick();
        settled.awake_chunks() > 0
    } {}
    g.bench_function("settled", |b| b.iter(|| black_box(&mut settled).tick()));
    // T7a: the settled tank with 30 creatures (SPEC §6.1's reference
    // tank), ten of each species, swimming; with no food they edit no
    // cells, so every chunk stays asleep.
    let mut fish = settled.clone();
    let species: Vec<_> = fish.species().iter().map(|s| s.id).collect();
    // Spread along the tank, skipping spots the sand pile fills.
    let mut column = 10;
    while fish.creatures().len() < 30 {
        let kind = species[fish.creatures().len() % species.len()];
        let (top, bottom) = fish.zone_rows(kind).expect("zone");
        let row = i64::try_from(usize::midpoint(top, bottom)).expect("row");
        let _ = fish.add_creature(kind, column, row);
        column = (column + 23) % 630;
    }
    let mut fish = advance(fish, 200);
    g.bench_function("settled_with_fish", |b| {
        b.iter(|| black_box(&mut fish).tick());
    });
    assert_eq!(fish.awake_chunks(), 0, "swimming wakes no chunk");
    // A rock body sinking through the water (D17), and the tank once it
    // and a dozen more rocks rest on the pile and the floor.
    let rock = settled.materials().id("rock").expect("rock");
    let mut rocks = settled.clone();
    rocks
        .paint(120, 40, 12, rock, Brush::Replace)
        .expect("rock");
    let sinking = advance(rocks.clone(), 5);
    g.bench_function("body_sinking", |b| {
        b.iter_batched_ref(|| sinking.clone(), World::tick, BatchSize::LargeInput);
    });
    for i in 0..12 {
        rocks
            .paint(30 + i * 50, 20, 6, rock, Brush::Replace)
            .expect("rock");
    }
    while {
        rocks.tick();
        rocks.awake_chunks() > 0
    } {}
    assert_eq!(rocks.bodies().count(), 13);
    g.bench_function("settled_with_bodies", |b| {
        b.iter(|| black_box(&mut rocks).tick());
    });
    // A settled tank whose substrate is alive: a 12-row aquasoil bed (far
    // more cells than the random-tick budget) with root tabs on it, so
    // random ticks run their full budget and the nutrient field keeps
    // diffusing, while every chunk sleeps.
    let mut living = settled_living();
    g.bench_function("settled_living", |b| {
        b.iter(|| black_box(&mut living).tick());
    });
    assert_eq!(living.awake_chunks(), 0, "the living tank stays asleep");
    // T6: the same tank planted and grown; plants still put out the odd
    // leaf, which wakes its chunk for a second.
    let mut planted = planted();
    g.bench_function("planted", |b| {
        b.iter(|| black_box(&mut planted).tick());
    });
    g.finish();
}

criterion_group!(benches, bench_tick);
criterion_main!(benches);
