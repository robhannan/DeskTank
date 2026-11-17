//! Simulation tick benches (SPEC §6.1, T1): a heavy pour must tick in
//! ≤ 5 ms and a settled scene in ≤ 0.5 ms on the reference machine.
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
    g.finish();
}

criterion_group!(benches, bench_tick);
criterion_main!(benches);
