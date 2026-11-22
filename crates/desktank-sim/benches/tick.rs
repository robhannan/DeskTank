//! Simulation tick benches (SPEC §6.1, T1, T5, T6, T6b, T7a): a heavy pour
//! must tick in ≤ 5 ms and a settled scene in ≤ 0.5 ms on the reference
//! machine, also with living cells (random ticks) and fields active, with
//! grown plants (and 500 of them, of every species), with floaters covering
//! the surface (at rest, and drifting in a current), and with the reference
//! tank's 30 creatures swimming (in open water, and among the 500 plants).
//!
//! `cargo bench -p desktank-sim`

#![allow(missing_docs)] // criterion's macros generate undocumented items

use std::hint::black_box;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use desktank_sim::{Brush, MaterialTable, Propagule, Rect, World, WorldConfig};

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

/// T6b, T6c: the living tank with 500 plants of every form: hygrophila in
/// three of every four columns along most of the bed and amazon sword,
/// vallisneria and cryptocoryne spreading by runners along the rest; four
/// glued rock shelves with aquasoil a third of the way down, two with
/// rotala (which needs strong light) and two carpeting with dwarf
/// hairgrass and Monte Carlo; and java moss on four glued rocks, grown for
/// 10 minutes and left until every chunk sleeps (or 10 more minutes pass).
fn five_hundred_plants() -> World {
    let mut w = settled_living();
    let t = w.materials().clone();
    let (rock, soil) = (t.id("rock").expect("rock"), t.id("aquasoil").expect("soil"));
    let ids = |n| w.plant_species().id(n).expect("species");
    let (hyg, rotala, moss) = (ids("hygrophila"), ids("rotala"), ids("java_moss"));
    let (xmas, anubias, java_fern) = (ids("christmas_moss"), ids("anubias"), ids("java_fern"));
    let carpets = [ids("dwarf_hairgrass"), ids("monte_carlo")];
    let rosettes = [ids("amazon_sword"), ids("vallisneria"), ids("cryptocoryne")];
    let glue = |w: &mut World, x: usize, y: usize| {
        let body = w.body_at(x, y).expect("body");
        w.set_anchored(body, true);
    };
    for k in 0..4 {
        let x0 = 40 + k * 160;
        let shelf = Rect {
            x0,
            y0: 120,
            x1: x0 + 80,
            y1: 124,
        };
        w.fill_rect(shelf, rock, Brush::Replace).expect("shelf");
        glue(&mut w, x0, 120);
        let bed = Rect {
            y0: 117,
            y1: 120,
            ..shelf
        };
        w.fill_rect(bed, soil, Brush::Replace).expect("bed");
        if k % 2 == 0 {
            for x in (x0 + 2..x0 + 78).step_by(4) {
                w.paint_plant(
                    i64::try_from(x).expect("x"),
                    110,
                    0,
                    rotala,
                    Propagule::Cuttings,
                )
                .expect("rotala");
            }
        } else {
            for x in (x0 + 4..x0 + 78).step_by(8) {
                w.paint_plant(
                    i64::try_from(x).expect("x"),
                    110,
                    0,
                    carpets[k / 2],
                    Propagule::Runners,
                )
                .expect("carpet");
            }
        }
        let x = 140 + 160 * i64::try_from(k).expect("k");
        w.paint(x, 220, 8, rock, Brush::Replace).expect("rock");
        glue(&mut w, usize::try_from(x).expect("x"), 220);
        let on_rock = match k {
            0 => moss,
            1 => xmas,
            2 => anubias,
            _ => java_fern,
        };
        for dx in [-6, -3, 0, 3, 6] {
            w.paint_plant(x + dx, 205, 0, on_rock, Propagule::Fragments)
                .expect("on_rock");
        }
    }
    for x in (0..584).filter(|x| x % 4 != 3) {
        w.paint_plant(x, 346, 0, hyg, Propagule::Cuttings)
            .expect("cutting");
    }
    for (k, x) in (592..640).step_by(9).enumerate() {
        w.paint_plant(x, 346, 0, rosettes[k % 3], Propagule::Runners)
            .expect("rosette");
    }
    let mut w = advance(w, 10 * 60 * 20);
    for _ in 0..10 * 60 * 20 {
        if w.awake_chunks() == 0 {
            break;
        }
        w.tick();
    }
    for s in [hyg, rotala, moss, xmas, anubias, java_fern]
        .into_iter()
        .chain(carpets)
        .chain(rosettes)
    {
        assert!(w.plants().any(|p| p.species == s), "{s:?} grew");
    }
    assert!(
        w.plants().any(|p| p.parent.is_some()),
        "runners rooted plants"
    );
    assert!(w.plant_count() >= 500, "{} plants", w.plant_count());
    w
}

/// T6c part 2: the living tank with duckweed and frogbit plantlets every
/// 40 cells along the surface, grown for 20 minutes, until they cover much
/// of it, and left until every chunk sleeps (or 10 more minutes pass):
/// every floater at rest.
fn floating() -> World {
    let mut w = settled_living();
    let ids = |n| w.plant_species().id(n).expect("species");
    let (duckweed, frogbit) = (ids("duckweed"), ids("frogbit"));
    let row = i64::try_from(w.surface_row()).expect("row") + 4;
    for (k, x) in (10..640).step_by(40).enumerate() {
        let s = if k % 2 == 0 { duckweed } else { frogbit };
        w.paint_plant(x, row, 0, s, Propagule::Buds)
            .expect("plantlet");
    }
    let mut w = advance(w, 20 * 60 * 20);
    for _ in 0..10 * 60 * 20 {
        if w.awake_chunks() == 0 {
            break;
        }
        w.tick();
    }
    w
}

fn advance(mut w: World, ticks: u32) -> World {
    for _ in 0..ticks {
        w.tick();
    }
    w
}

/// A second consecutive grazing tick: ten walkers bite an algae/detritus
/// film while twenty other fish swim. The previous bites have woken the bed.
fn grazing() -> World {
    let t = MaterialTable::builtin().expect("materials");
    let mut w = World::new(WorldConfig::default(), t).expect("world");
    let sand = w.materials().id("sand").expect("sand");
    w.fill_rect(
        Rect {
            x0: 0,
            y0: 348,
            x1: 640,
            y1: 360,
        },
        sand,
        Brush::Replace,
    )
    .expect("bed");
    w = advance(w, 100);
    let [algae, mulm] = ["algae", "detritus"].map(|n| w.materials().id(n).expect("film"));
    for x in 0..640 {
        w.set_cell(x, 347, if x % 2 == 0 { algae } else { mulm })
            .expect("film");
    }
    for (name, row) in [("neon_tetra", 180), ("betta", 90), ("corydoras", 344)] {
        let species = w.species().id(name).expect("fish");
        for x in (20..640).step_by(64) {
            w.add_creature(species, x, row).expect("stock");
        }
    }
    w.tick();
    let bites = |w: &World| {
        w.creatures()
            .iter()
            .map(desktank_sim::Creature::eaten)
            .sum::<u32>()
    };
    assert_eq!(bites(&w), 10);
    let mut check = w.clone();
    check.tick();
    assert_eq!(bites(&check), 20);
    w
}

/// Adds 30 creatures of `names` in turn, mid-zone, spread along the tank
/// and skipping spots where there is no room.
fn add_thirty(w: &mut World, names: &[&str]) {
    let species: Vec<_> = names
        .iter()
        .map(|n| w.species().id(n).expect("species"))
        .collect();
    let mut column = 10;
    while w.creatures().len() < 30 {
        let kind = species[w.creatures().len() % species.len()];
        let (top, bottom) = w.zone_rows(kind).expect("zone");
        let row = i64::try_from(usize::midpoint(top, bottom)).expect("row");
        let _ = w.add_creature(kind, column, row);
        column = (column + 23) % 630;
    }
}

/// Ticks `w`. T9: unfed creatures starve after 8 hours of world time
/// (576 000 ticks), sooner than criterion's run ends, so this tops their
/// energy up once per 100 000 ticks (a negligible share of the measured
/// time) to keep measuring living, ageing fish with the water sampled once
/// a second.
fn tick_fed(w: &mut World, since_fed: &mut u32) {
    *since_fed += 1;
    if *since_fed == 100_000 {
        *since_fed = 0;
        let ids: Vec<_> = w
            .creatures()
            .iter()
            .map(desktank_sim::Creature::id)
            .collect();
        for id in ids {
            w.set_creature_energy(id, desktank_sim::ENERGY_RESERVE);
        }
    }
    black_box(w).tick();
}

/// T6c part 2: floaters covering the surface, at rest and drifting.
fn bench_floaters(g: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>) {
    // T6c part 2: floaters covering the surface, at rest: each sleeping
    // raft costs a check of its chunks per tick.
    let mut floating = floating();
    println!(
        "settled_floaters: {} plants, {} cells, {} awake chunks",
        floating.plant_count(),
        floating.plant_cells(),
        floating.awake_chunks()
    );
    g.bench_function("settled_floaters", |b| {
        b.iter(|| black_box(&mut floating).tick());
    });
    // The same floaters a few seconds after an internal filter near the
    // surface starts a current: the rafts in its jet drift, each stepped
    // every tick. Each sample ticks a fresh copy, so they never all fetch
    // up against the far glass.
    let mut drifting = floating.clone();
    let row = i64::try_from(drifting.surface_row()).expect("row") + 15;
    drifting
        .add_device(desktank_sim::DeviceKind::InternalFilter, 1, row, false)
        .expect("filter")
        .expect("fits");
    let drifting = advance(drifting, 5 * 20);
    g.bench_function("drifting_floaters", |b| {
        b.iter_batched_ref(|| drifting.clone(), World::tick, BatchSize::LargeInput);
    });
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
    add_thirty(&mut fish, &["neon_tetra", "corydoras", "betta"]);
    // T9 part 2: they are pairs, so cap the tank at 30: the females check
    // to breed every minute but never conceive, so the population stays
    // put for as long as criterion runs.
    fish.set_creature_cap(30);
    let mut fish = advance(fish, 200);
    let mut since_fed = 0_u32;
    g.bench_function("settled_with_fish", |b| {
        b.iter(|| tick_fed(&mut fish, &mut since_fed));
    });
    assert_eq!(fish.creatures().len(), 30, "every fish lived");
    assert_eq!(fish.awake_chunks(), 0, "swimming wakes no chunk");
    let grazing = grazing();
    g.bench_function("grazing", |b| {
        b.iter_batched_ref(|| grazing.clone(), World::tick, BatchSize::LargeInput);
    });
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
    // T6b: 500 grown plants of every species; the plant update visits up
    // to 5 of them per tick.
    let mut plants = five_hundred_plants();
    println!(
        "settled_500_plants: {} plants, {} cells, {} awake chunks",
        plants.plant_count(),
        plants.plant_cells(),
        plants.awake_chunks()
    );
    g.bench_function("settled_500_plants", |b| {
        b.iter(|| black_box(&mut plants).tick());
    });
    bench_floaters(&mut g);
    // D21: the 500 plants with 30 swimmers of every swimming species among
    // them, so collision checks meet plant cells (and their depth bands)
    // on most ticks.
    let mut among = plants.clone();
    add_thirty(
        &mut among,
        &[
            "neon_tetra",
            "harlequin_rasbora",
            "guppy",
            "betta",
            "honey_gourami",
        ],
    );
    let mut since_fed = 0_u32;
    g.bench_function("plants_with_fish", |b| {
        b.iter(|| tick_fed(&mut among, &mut since_fed));
    });
    g.finish();
}

criterion_group!(benches, bench_tick);
criterion_main!(benches);
