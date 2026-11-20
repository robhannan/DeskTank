//! Creature quirks: two of a kind added together don't act as one. Each
//! has its own speed, favourite depth, breathing and hopping rhythm (see
//! `world::creatures::quirks`).
#![allow(clippy::cast_precision_loss)]

use desktank_sim::{Brush, CreatureId, MaterialTable, Rect, World, WorldConfig};

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

/// `n` of species `name` in a row along the middle of a fresh tank.
fn stocked(seed: u64, name: &str, n: i64) -> (World, Vec<CreatureId>) {
    let mut w = world(240, 120, seed);
    let s = w.species().id(name).unwrap();
    let ids = (0..n)
        .map(|i| w.add_creature(s, 30 + 180 * i / n, 50).unwrap())
        .collect();
    (w, ids)
}

/// `(min, max)` of `values`.
fn range(values: &[f32]) -> (f32, f32) {
    values
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        })
}

#[test]
fn roamers_swim_at_their_own_speeds_and_depths() {
    for seed in [3, 41] {
        let (mut w, ids) = stocked(seed, "guppy", 6);
        let ticks = 12_000;
        let mut travelled = vec![0.0; ids.len()];
        let mut depth = vec![0.0; ids.len()];
        let mut last: Vec<_> = ids
            .iter()
            .map(|&i| w.creature(i).unwrap().centre())
            .collect();
        for _ in 0..ticks {
            w.tick();
            for (j, &i) in ids.iter().enumerate() {
                let (x, y) = w.creature(i).unwrap().centre();
                travelled[j] += (x - last[j].0).hypot(y - last[j].1);
                depth[j] += y / ticks as f32;
                last[j] = (x, y);
            }
        }
        let (slow, fast) = range(&travelled);
        let (high, low) = range(&depth);
        eprintln!("seed {seed}: travelled {slow:.0}–{fast:.0} cells, mean rows {high:.1}–{low:.1}");
        // Each its own pace (85–115% of the species' speed) and its own
        // favourite depth.
        assert!(fast >= slow * 1.12, "one speed: {travelled:?}");
        assert!(low - high >= 6.0, "one depth: {depth:?}");
    }
}

#[test]
fn air_breathers_surface_on_beats_of_their_own() {
    let (mut w, ids) = stocked(5, "honey_gourami", 4);
    let surface = w.surface_row();
    let mut visits: Vec<Vec<u64>> = vec![Vec::new(); ids.len()];
    let mut away = vec![true; ids.len()];
    for t in 0..4_800 {
        w.tick();
        for (j, &i) in ids.iter().enumerate() {
            let c = w.creature(i).unwrap();
            let b = w.creature_body(c);
            if b.y0 > surface + 2 {
                away[j] = true;
            } else if b.y0 == surface && away[j] {
                away[j] = false;
                visits[j].push(t);
            }
        }
    }
    eprintln!("visits {visits:?}");
    // They don't arrive together: the first visits spread over seconds.
    let firsts: Vec<f32> = visits.iter().map(|v| v[0] as f32).collect();
    let (first, last) = range(&firsts);
    assert!(last - first >= 60.0, "first breaths together: {firsts:?}");
    // And each keeps its own beat, so their mean gaps differ.
    let gaps: Vec<f32> = visits
        .iter()
        .map(|v| (v[v.len() - 1] - v[0]) as f32 / (v.len() - 1) as f32)
        .collect();
    let (short, long) = range(&gaps);
    assert!(long >= short * 1.1, "one beat: {gaps:?}");
}

#[test]
fn shrimp_hop_in_rhythms_of_their_own() {
    let (mut w, ids) = stocked(8, "cherry_shrimp", 4);
    let (width, height) = (w.width(), w.height());
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
    let mut hops: Vec<Vec<u64>> = vec![Vec::new(); ids.len()];
    let mut last: Vec<f32> = vec![0.0; ids.len()];
    let mut rising = vec![false; ids.len()];
    for t in 0..12_000 {
        w.tick();
        for (j, &i) in ids.iter().enumerate() {
            let y = w.creature(i).unwrap().centre().1;
            let up = y < last[j] - 0.1;
            if up && !rising[j] && t > 400 {
                hops[j].push(t);
            }
            rising[j] = up;
            last[j] = y;
        }
    }
    let gaps: Vec<f32> = hops
        .iter()
        .map(|h| (h[h.len() - 1] - h[0]) as f32 / (h.len() - 1).max(1) as f32)
        .collect();
    eprintln!(
        "hops {:?} gaps {gaps:?}",
        hops.iter().map(Vec::len).collect::<Vec<_>>()
    );
    let (short, long) = range(&gaps);
    assert!(long >= short * 1.1, "one rhythm: {gaps:?}");
}
