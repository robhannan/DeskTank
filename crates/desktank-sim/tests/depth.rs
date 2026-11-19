//! 2.5D depth, part 1 (D24): creatures pass plant cells in other depth
//! bands, so food lodged inside a dense plant is reached and fish don't
//! stall against plants.
#![allow(clippy::cast_precision_loss, clippy::many_single_char_names)]

use desktank_sim::save;
use desktank_sim::species::Zone;
use desktank_sim::{
    Brush, Creature, DEPTH_BANDS, MaterialId, MaterialTable, Propagule, Rect, SpeciesId,
    TICKS_PER_SECOND, World, WorldConfig,
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

fn run(w: &mut World, ticks: u32) {
    for _ in 0..ticks {
        w.tick();
    }
}

/// A tank with a dense stand of `plant` grown for ten minutes on aquasoil
/// (from cuttings, or the species' default).
fn thicket(seed: u64, plant: &str) -> World {
    let mut w = world(120, 80, seed);
    let soil = id(&w, "aquasoil");
    w.fill_rect(
        Rect {
            x0: 0,
            y0: 72,
            x1: 120,
            y1: 80,
        },
        soil,
        Brush::Replace,
    )
    .unwrap();
    let kind = w.plant_species().id(plant).unwrap();
    for x in (44..=76).step_by(4) {
        let s = w.plant_species().get(kind).unwrap();
        let from = if s.grows_from(Propagule::Cuttings) {
            Propagule::Cuttings
        } else {
            s.default_propagule()
        };
        w.paint_plant(x, 71, 0, kind, from).unwrap();
    }
    run(&mut w, 10 * 60 * TICKS_PER_SECOND);
    w
}

/// The food cells with plant cells on both sides of their row.
fn lodged(w: &World, food: MaterialId) -> Vec<(usize, usize)> {
    let plant = |x: usize, y: usize| w.plant_at(x, y).is_some();
    let mut out = Vec::new();
    for y in 0..w.height() {
        for x in 1..w.width() - 1 {
            if w.cell(x, y).unwrap().material == food
                && (0..x).rev().take(4).any(|l| plant(l, y))
                && (x + 1..w.width()).take(4).any(|r| plant(r, y))
            {
                out.push((x, y));
            }
        }
    }
    out
}

/// Seconds until up to eight food cells lodged in a ten-minute stand of
/// `plant` are eaten by four hungry tetras, with the rest of the food
/// cleared.
fn lodged_food_eaten(seed: u64, plant: &str, limit: u32) -> Result<f64, String> {
    let mut w = thicket(seed, plant);
    let (worm, water) = (id(&w, "bloodworm"), id(&w, "water"));
    for x in (46..=74).step_by(7) {
        w.paint(x, 20, 1, worm, Brush::Pour).unwrap();
    }
    run(&mut w, 30 * TICKS_PER_SECOND);
    // The deepest eight, so four tetras (four bites each fill one) stay
    // hungry until the last.
    let mut stuck = lodged(&w, worm);
    stuck.sort_by_key(|&(x, y)| (core::cmp::Reverse(y), x));
    stuck.truncate(8);
    if stuck.is_empty() {
        return Err(format!("no food lodged in the {plant}"));
    }
    for y in 0..w.height() {
        for x in 0..w.width() {
            if w.cell(x, y).unwrap().material == worm && !stuck.contains(&(x, y)) {
                w.set_cell(x, y, water).unwrap();
            }
        }
    }
    let tetra = species(&w, "neon_tetra");
    for (x, y) in [(10, 30), (110, 30), (10, 55), (110, 55)] {
        w.add_creature(tetra, x, y).unwrap();
    }
    for t in 0..limit {
        if !stuck
            .iter()
            .any(|&(x, y)| w.cell(x, y).unwrap().material == worm)
        {
            return Ok(f64::from(t) / f64::from(TICKS_PER_SECOND));
        }
        w.tick();
    }
    let left: Vec<_> = stuck
        .into_iter()
        .filter(|&(x, y)| w.cell(x, y).unwrap().material == worm)
        .collect();
    Err(format!("{plant}: food left at {left:?}"))
}

#[test]
fn food_lodged_in_a_dense_plant_is_eaten() {
    for (seed, plant) in [
        (1, "hygrophila"),
        (2, "rotala"),
        (3, "hygrophila"),
        (4, "rotala"),
    ] {
        match lodged_food_eaten(seed, plant, 20 * TICKS_PER_SECOND) {
            Ok(s) => eprintln!("seed {seed}: food lodged in {plant} eaten after {s:.1} s"),
            Err(e) => panic!("seed {seed}: {e}"),
        }
    }
}

/// The longest each swimmer sat still (its centre unmoved), in ticks, over
/// `minutes` in a planted tank fed every half minute.
fn longest_stalls(seed: u64, minutes: u32) -> Vec<(String, u32)> {
    let mut w = world(160, 96, seed);
    let soil = id(&w, "aquasoil");
    w.fill_rect(
        Rect {
            x0: 0,
            y0: 86,
            x1: 160,
            y1: 96,
        },
        soil,
        Brush::Replace,
    )
    .unwrap();
    for (x, plant) in [
        (20, "hygrophila"),
        (26, "rotala"),
        (40, "vallisneria"),
        (60, "ludwigia"),
        (66, "hygrophila"),
        (90, "cabomba"),
        (110, "amazon_sword"),
        (130, "rotala"),
        (136, "bacopa"),
    ] {
        let kind = w.plant_species().id(plant).unwrap();
        let from = w.plant_species().get(kind).unwrap().default_propagule();
        w.paint_plant(x, 85, 0, kind, from).unwrap();
    }
    run(&mut w, 10 * 60 * TICKS_PER_SECOND);
    let swimmers: Vec<_> = w
        .species()
        .iter()
        .filter(|s| matches!(s.zone, Zone::Mid | Zone::Surface))
        .map(|s| s.id)
        .collect();
    for (k, &s) in swimmers.iter().enumerate() {
        for j in 0..3_i64 {
            let x = 15 + j * 50 + i64::try_from(k).unwrap() * 4;
            w.add_creature(s, x, 50).unwrap();
        }
    }
    let worm = id(&w, "bloodworm");
    let n = w.creatures().len();
    let (mut still, mut longest) = (vec![0_u32; n], vec![0_u32; n]);
    let mut last: Vec<_> = w.creatures().iter().map(Creature::centre).collect();
    for t in 0..minutes * 60 * TICKS_PER_SECOND {
        if t % (30 * TICKS_PER_SECOND) == 0 {
            let x = i64::from(t / 600 % 7) * 20 + 20;
            w.paint(x, 10, 2, worm, Brush::Pour).unwrap();
        }
        w.tick();
        for (i, c) in w.creatures().iter().enumerate() {
            if c.centre() == last[i] {
                still[i] += 1;
                longest[i] = longest[i].max(still[i]);
            } else {
                still[i] = 0;
                last[i] = c.centre();
            }
        }
    }
    assert_eq!(w.creatures().len(), n);
    w.creatures()
        .iter()
        .zip(longest)
        .map(|(c, l)| (w.species().get(c.species()).unwrap().name.clone(), l))
        .collect()
}

#[test]
fn swimmers_never_stall_against_plants() {
    // Before depth bands a schooler pressed against a plant towards its
    // school sat still for up to 42 s (848 ticks) here; now a swimmer
    // holds still at most while it shifts band past a plant.
    for seed in [1, 2] {
        for (name, ticks) in longest_stalls(seed, 10) {
            eprintln!("seed {seed}: {name} longest still {ticks} ticks");
            assert!(
                ticks <= 5 * TICKS_PER_SECOND,
                "seed {seed}: {name} sat still for {ticks} ticks"
            );
        }
    }
}

#[test]
fn plant_cells_have_bands_by_part() {
    let stems = thicket(5, "hygrophila");
    let (stem, leaf) = (id(&stems, "stem"), id(&stems, "leaf"));
    let mut spread = std::collections::BTreeSet::new();
    for p in stems.plants() {
        let b = p.bounds;
        let base = stems.plant_band(p.base.0, p.base.1).unwrap();
        spread.insert(base);
        for y in b.y0..b.y1 {
            for x in b.x0..b.x1 {
                if stems.plant_at(x, y) != Some(p.id) {
                    continue;
                }
                let band = stems.plant_band(x, y).unwrap();
                assert!(band < DEPTH_BANDS);
                let m = stems.cell(x, y).unwrap().material;
                if m == stem {
                    assert_eq!(band, base, "a stem keeps its plant's band");
                } else if m == leaf {
                    assert!(band.abs_diff(base) <= 1, "a leaf is a band off at most");
                }
            }
        }
    }
    assert!(
        spread.len() >= 3,
        "plants spread over the bands: {spread:?}"
    );
    // A rosette's blades fan over up to two bands each way, by column.
    let rosettes = thicket(6, "vallisneria");
    let leaf = id(&rosettes, "leaf");
    let mut fan = std::collections::BTreeSet::new();
    for p in rosettes.plants() {
        let base = rosettes.plant_band(p.base.0, p.base.1).unwrap();
        for y in p.bounds.y0..p.bounds.y1 {
            for x in p.bounds.x0..p.bounds.x1 {
                if rosettes.plant_at(x, y) == Some(p.id)
                    && rosettes.cell(x, y).unwrap().material == leaf
                {
                    let band = rosettes.plant_band(x, y).unwrap();
                    assert!(band.abs_diff(base) <= 2);
                    fan.insert(i16::from(band) - i16::from(base));
                }
            }
        }
    }
    assert!(fan.len() >= 3, "blades fan out: {fan:?}");
    // Nothing else has a band; bands are the same in a replay.
    assert_eq!(stems.plant_band(0, 0), None);
    assert_eq!(stems.plant_band(0, 79), None);
    let again = thicket(5, "hygrophila");
    for y in 0..80 {
        for x in 0..120 {
            assert_eq!(stems.plant_band(x, y), again.plant_band(x, y));
        }
    }
}

#[test]
fn a_tank_saved_mid_shift_carries_on_exactly() {
    let mut a = thicket(7, "rotala");
    let tetra = species(&a, "neon_tetra");
    for (x, y) in [(10, 30), (110, 30), (60, 20)] {
        a.add_creature(tetra, x, y).unwrap();
    }
    // Run until a creature is between two bands.
    let mut t = 0;
    while a.creatures().iter().all(|c| c.depth().fract() == 0.0) {
        a.tick();
        t += 1;
        assert!(t < 2000, "nobody changed band");
    }
    let mut b = save::decode(&save::encode(&a), MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(a.state_hash(), b.state_hash());
    let mut bands = std::collections::BTreeSet::new();
    for _ in 0..2000 {
        a.tick();
        b.tick();
        bands.extend(a.creatures().iter().map(Creature::band));
    }
    assert_eq!(a.state_hash(), b.state_hash());
    assert!(bands.len() >= 3, "they swim at several depths: {bands:?}");
}
