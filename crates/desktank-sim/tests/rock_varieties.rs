//! Rock varieties (#457): `dragon_stone` and `seiryu_stone` are rigid
//! hardscape that behaves like `rock` everywhere: bodies, moss, algae and
//! saves.

use desktank_sim::{
    Brush, FULL_MASK, FieldKind, MaterialId, MaterialTable, Propagule, Rect, World, WorldConfig,
    disc_mask, quarter_bit, save,
};

/// The stones, with the plain rock first.
const STONES: [&str; 3] = ["rock", "dragon_stone", "seiryu_stone"];
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

fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Rect {
    Rect { x0, y0, x1, y1 }
}

fn run(w: &mut World, ticks: u32) {
    for _ in 0..ticks {
        w.tick();
    }
}

fn positions(w: &World, material: MaterialId) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for y in 0..w.height() {
        for x in 0..w.width() {
            if w.cell(x, y).is_some_and(|c| c.material == material) {
                out.push((x, y));
            }
        }
    }
    out
}

#[test]
fn the_new_stones_are_rigid_and_ordered_by_density() {
    let w = world(32, 32, 0);
    let table = w.materials();
    let get = |name: &str| table.get(id(&w, name)).unwrap();
    for name in STONES {
        assert_eq!(get(name).class, get("rock").class, "{name}");
    }
    assert!(get("dragon_stone").density < get("rock").density);
    assert!(get("seiryu_stone").density > get("rock").density);
    // Appended above the old maximum, so old saves are unchanged.
    assert!(get("dragon_stone").id.0 > 80 && get("seiryu_stone").id.0 > 80);
}

#[test]
fn each_stone_sinks_as_one_piece_and_rests_on_sand() {
    // Rock sets the benchmark: every kind is one body that lands on the bed.
    for name in STONES {
        let mut w = world(96, 96, 1);
        let (sand, stone) = (id(&w, "sand"), id(&w, name));
        w.fill_rect(rect(0, 86, 96, 96), sand, Brush::Replace)
            .unwrap();
        w.paint(48, 30, 5, stone, Brush::Replace).unwrap();
        let shape = positions(&w, stone);
        for _ in 0..600 {
            w.tick();
            if w.awake_chunks() == 0 {
                break;
            }
        }
        let after = positions(&w, stone);
        assert_eq!(after.len(), shape.len(), "{name} lost cells");
        assert_eq!(w.bodies().count(), 1, "{name}");
        assert!(after.iter().map(|p| p.1).max().unwrap() >= 80, "{name}");
        // Glued, it stays put.
        let body = w.body_at(after[0].0, after[0].1).unwrap();
        assert!(w.set_anchored(body, true));
        run(&mut w, 40);
        assert_eq!(positions(&w, stone), after, "{name} moved when glued");
    }
}

#[test]
fn moss_spreads_onto_each_stone() {
    for name in STONES {
        let mut w = world(160, 120, 5);
        let stone = id(&w, name);
        w.fill_rect(rect(60, 70, 100, 90), stone, Brush::Replace)
            .unwrap();
        let body = w.body_at(60, 70).unwrap();
        assert!(w.set_anchored(body, true));
        let moss = w.plant_species().id("java_moss").unwrap();
        w.paint_plant(80, 60, 2, moss, Propagule::Fragments)
            .unwrap();
        run(&mut w, 3 * MINUTE);
        let on_stone = |w: &World| {
            let mut n = 0;
            for y in 1..w.height() - 1 {
                for x in 1..w.width() - 1 {
                    if w.plant_at(x, y).is_none() {
                        continue;
                    }
                    let touches = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dy)| {
                        w.cell(x.wrapping_add_signed(dx), y.wrapping_add_signed(dy))
                            .is_some_and(|c| c.material == stone)
                    });
                    assert!(touches, "{name}: moss at ({x}, {y}) isn't on the stone");
                    n += 1;
                }
            }
            n
        };
        let kept = on_stone(&w);
        assert!(kept > 0, "no moss rooted on {name}");
        for fy in 0..w.field(FieldKind::Nutrient).height() {
            for fx in 0..w.field(FieldKind::Nutrient).width() {
                w.add_to_field(FieldKind::Nutrient, fx * 8, fy * 8, 0.2);
            }
        }
        run(&mut w, 10 * MINUTE);
        let grown = on_stone(&w);
        assert!(
            grown >= kept + 20,
            "{name}: moss grew from {kept} to {grown}"
        );
    }
}

#[test]
fn algae_coats_each_stone_like_rock() {
    for name in STONES {
        let mut w = world(64, 64, 2);
        let stone = id(&w, name);
        w.fill_rect(rect(10, 40, 50, 60), stone, Brush::Replace)
            .unwrap();
        assert!(w.set_algae_coat(30, 40, 8), "{name}: top face takes film");
        assert!(w.coated_cells() > 0, "{name}");
    }
}

#[test]
fn a_save_with_the_new_stones_round_trips() {
    let mut w = world(96, 96, 3);
    let sand = id(&w, "sand");
    w.fill_rect(rect(0, 86, 96, 96), sand, Brush::Replace)
        .unwrap();
    for (name, x) in [("rock", 20), ("dragon_stone", 48), ("seiryu_stone", 76)] {
        w.paint(x, 30, 5, id(&w, name), Brush::Replace).unwrap();
    }
    run(&mut w, 300);
    let bytes = save::encode(&w);
    let loaded = save::decode(&bytes, MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(loaded.state_hash(), w.state_hash());
    assert_eq!(save::encode(&loaded), bytes);
    for name in STONES {
        let stone = id(&loaded, name);
        assert_eq!(positions(&loaded, stone), positions(&w, stone), "{name}");
        assert!(!positions(&loaded, stone).is_empty(), "{name}");
    }
}

/// The weathered stones, with their shapes' names.
const WEATHERED: [&str; 2] = ["dragon_stone", "seiryu_stone"];

/// A drag of dabs: a rising stroke of a stone, as a tool drag would make.
fn drag(w: &mut World, name: &str, radius: u32) {
    let stone = id(w, name);
    let mut body = None;
    for i in 0..24 {
        let (_, b) = w
            .paint_into(30 + i, 70 - i / 3, radius, stone, Brush::Replace, body)
            .unwrap();
        body = b;
    }
}

/// Every cell's mask in the grid, row by row.
fn masks(w: &World) -> Vec<u16> {
    let mut out = Vec::new();
    for y in 0..w.height() {
        for x in 0..w.width() {
            out.push(w.cell_mask(x, y));
        }
    }
    out
}

/// The number of 4-connected pieces of `stone`'s drawn quarters.
fn quarter_pieces(w: &World, stone: MaterialId) -> usize {
    let side = (4 * w.width(), 4 * w.height());
    let drawn = |x: usize, y: usize| {
        let (cx, cy) = (x / 4, y / 4);
        w.cell(cx, cy).is_some_and(|c| c.material == stone)
            && w.cell_mask(cx, cy) & quarter_bit(x % 4, y % 4) != 0
    };
    let mut seen = vec![false; side.0 * side.1];
    let mut pieces = 0;
    for y in 0..side.1 {
        for x in 0..side.0 {
            if seen[y * side.0 + x] || !drawn(x, y) {
                continue;
            }
            pieces += 1;
            seen[y * side.0 + x] = true;
            let mut stack = vec![(x, y)];
            while let Some((x, y)) = stack.pop() {
                let around = [
                    (x.wrapping_sub(1), y),
                    (x + 1, y),
                    (x, y.wrapping_sub(1)),
                    (x, y + 1),
                ];
                for (nx, ny) in around {
                    if nx < side.0 && ny < side.1 && !seen[ny * side.0 + nx] && drawn(nx, ny) {
                        seen[ny * side.0 + nx] = true;
                        stack.push((nx, ny));
                    }
                }
            }
        }
    }
    pieces
}

/// The quarters drawn for `stone` over the whole grid.
fn drawn_quarters(w: &World, stone: MaterialId) -> u32 {
    positions(w, stone)
        .iter()
        .map(|&(x, y)| w.cell_mask(x, y).count_ones())
        .sum()
}

#[test]
fn weathered_strokes_are_not_the_plain_brush_circle() {
    for name in WEATHERED {
        let mut plain = world(120, 100, 4);
        let mut stone = world(120, 100, 4);
        drag(&mut plain, "rock", 8);
        drag(&mut stone, name, 8);
        let (rock, kind) = (id(&plain, "rock"), id(&stone, name));
        // The same cells are drawn at the same places: the weathering trims
        // quarters and drops fringe cells, it doesn't move the stone.
        let cells = positions(&stone, kind);
        assert!(positions(&plain, rock).len() >= cells.len(), "{name}");
        let trimmed = cells
            .iter()
            .filter(|&&(x, y)| stone.cell_mask(x, y) != plain.cell_mask(x, y))
            .count();
        assert!(trimmed > cells.len() / 12, "{name}: only {trimmed} trimmed");
        // About the size painted: trimmed, not shrunk away.
        let (plain_q, stone_q) = (drawn_quarters(&plain, rock), drawn_quarters(&stone, kind));
        assert!(stone_q < plain_q, "{name}: {stone_q} of {plain_q}");
        assert!(
            stone_q * 100 > plain_q * 55,
            "{name}: {stone_q} of {plain_q}"
        );
        // No rigid cell is left invisible.
        for &(x, y) in &cells {
            assert_ne!(stone.cell_mask(x, y), 0, "{name} ({x}, {y})");
        }
    }
}

#[test]
fn a_weathered_stroke_is_one_piece() {
    for name in WEATHERED {
        for seed in 0..6 {
            for radius in [4, 6, 9] {
                let mut w = world(120, 100, seed);
                drag(&mut w, name, radius);
                assert_eq!(w.bodies().count(), 1, "{name} seed {seed} r {radius}");
                assert_eq!(
                    quarter_pieces(&w, id(&w, name)),
                    1,
                    "{name} seed {seed} r {radius}"
                );
            }
        }
    }
}

#[test]
fn weathering_is_deterministic_and_seeded() {
    for name in WEATHERED {
        let (mut a, mut b, mut c) = (world(120, 100, 4), world(120, 100, 4), world(120, 100, 5));
        for w in [&mut a, &mut b, &mut c] {
            drag(w, name, 8);
        }
        assert_eq!(masks(&a), masks(&b), "{name}: same seed, same stone");
        assert_eq!(a.state_hash(), b.state_hash(), "{name}");
        assert_ne!(masks(&a), masks(&c), "{name}: another seed, another stone");
        // Drawing over the same strokes again changes nothing: the edge is
        // a function of position, not of the dab that reached it.
        let before = a.state_hash();
        drag(&mut a, name, 8);
        assert_eq!(a.state_hash(), before, "{name}: redrawn stroke changed");
    }
}

#[test]
fn plain_rock_and_driftwood_keep_the_brush_outline() {
    for name in ["rock", "driftwood"] {
        for radius in [1_u32, 4, 9] {
            let mut w = world(80, 80, 6);
            let stone = id(&w, name);
            w.paint(40, 40, radius, stone, Brush::Replace).unwrap();
            let r = i64::from(radius);
            let mut partial = 0;
            for y in 0..80_i64 {
                for x in 0..80_i64 {
                    let (dx, dy) = (x - 40, y - 40);
                    if dx * dx + dy * dy > r * r + r {
                        continue;
                    }
                    let got = w.cell_mask(usize::try_from(x).unwrap(), usize::try_from(y).unwrap());
                    assert_eq!(got, disc_mask(dx, dy, r), "{name} r {radius} ({dx}, {dy})");
                    partial += usize::from(got != FULL_MASK);
                }
            }
            assert!(partial > 0, "{name} r {radius}");
        }
    }
}

#[test]
fn tiny_dabs_of_a_weathered_stone_stay_round() {
    for name in WEATHERED {
        for radius in [0_u32, 1, 2] {
            let mut w = world(60, 60, 6);
            w.paint(30, 30, radius, id(&w, name), Brush::Replace)
                .unwrap();
            let r = i64::from(radius);
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx * dx + dy * dy <= r * r + r {
                        let at = |d: i64| usize::try_from(30 + d).unwrap();
                        assert_eq!(w.cell_mask(at(dx), at(dy)), disc_mask(dx, dy, r), "{name}");
                    }
                }
            }
        }
    }
}

#[test]
fn a_weathered_shape_survives_a_save_and_a_fall() {
    let mut w = world(120, 100, 9);
    let sand = id(&w, "sand");
    w.fill_rect(rect(0, 90, 120, 100), sand, Brush::Replace)
        .unwrap();
    for name in WEATHERED {
        drag(&mut w, name, 6);
        run(&mut w, 400);
    }
    let bytes = save::encode(&w);
    let loaded = save::decode(&bytes, MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(masks(&loaded), masks(&w));
    assert_eq!(loaded.state_hash(), w.state_hash());
    assert_eq!(save::encode(&loaded), bytes);
    for name in WEATHERED {
        let stone = id(&w, name);
        assert!(drawn_quarters(&w, stone) > 0, "{name}");
        // It still rests as one trimmed piece after loading.
        assert_eq!(quarter_pieces(&loaded, stone), quarter_pieces(&w, stone));
    }
}
