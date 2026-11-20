//! Sub-cell outlines of rigid cells (`world::hardscape`): a round brush
//! records a round 4× outline, erasing clears and carves it, it moves with a
//! falling body, and saves keep it.

use std::collections::BTreeMap;

use desktank_sim::tools::{Button, PaletteItem, Tool, Tools};
use desktank_sim::{
    Brush, FULL_MASK, MaterialId, MaterialTable, World, WorldConfig, disc_mask, save,
};

fn world(width: usize, height: usize) -> World {
    World::new(
        WorldConfig {
            width,
            height,
            seed: 7,
            water_level: 1.0,
        },
        MaterialTable::builtin().unwrap(),
    )
    .unwrap()
}

fn id(w: &World, name: &str) -> MaterialId {
    w.materials().id(name).unwrap()
}

/// Every rigid cell's mask, by position.
fn masks(w: &World) -> BTreeMap<(usize, usize), u16> {
    (0..w.height())
        .flat_map(|y| (0..w.width()).map(move |x| (x, y)))
        .filter_map(|(x, y)| w.rigid_mask(y * w.width() + x).map(|m| ((x, y), m)))
        .collect()
}

#[test]
fn a_round_pour_records_a_round_outline() {
    let mut w = world(60, 60);
    let rock = id(&w, "rock");
    w.paint(30, 30, 10, rock, Brush::Replace).unwrap();
    let m = masks(&w);
    // The cells are the ones a brush always took.
    assert_eq!(
        m.len(),
        (-10_i64..=10)
            .flat_map(|y| (-10_i64..=10).map(move |x| (x, y)))
            .filter(|(x, y)| x * x + y * y <= 110)
            .count()
    );
    for (&(x, y), &mask) in &m {
        #[allow(clippy::cast_possible_wrap)]
        let (dx, dy) = (x as i64 - 30, y as i64 - 30);
        assert_eq!(mask, disc_mask(dx, dy, 10), "({x}, {y})");
        if dx * dx + dy * dy <= 64 {
            assert_eq!(mask, FULL_MASK, "interior ({x}, {y})");
        }
        // Mirror images match their mirrored masks.
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let mirror = m[&((30 - dx) as usize, y)];
        let flipped = (0..16).fold(0u16, |acc, b| {
            let (qx, qy) = (b % 4, b / 4);
            if mask & (1 << b) != 0 {
                acc | 1 << (qy * 4 + 3 - qx)
            } else {
                acc
            }
        });
        assert_eq!(mirror, flipped, "mirror of ({x}, {y})");
    }
    assert!(m.values().any(|&k| k != FULL_MASK));
    // A cell set on its own is whole.
    w.set_cell(2, 2, rock).unwrap();
    assert_eq!(w.cell_mask(2, 2), FULL_MASK);
    // Non-rigid cells report no outline.
    assert_eq!(w.rigid_mask(0), None);
}

#[test]
fn a_stroke_fills_in_its_own_edges() {
    let mut w = world(60, 40);
    let rock = id(&w, "rock");
    // Two overlapping dabs: the second covers the first's trimmed edge.
    let (_, body) = w.paint_into(20, 20, 4, rock, Brush::Replace, None).unwrap();
    let edge = w.cell_mask(24, 20);
    assert_ne!(edge, FULL_MASK);
    w.paint_into(22, 20, 4, rock, Brush::Replace, body).unwrap();
    assert_eq!(w.cell_mask(24, 20), FULL_MASK);
}

#[test]
fn erasing_clears_and_carves_outlines() {
    let mut w = world(80, 60);
    let rock = id(&w, "rock");
    let water = id(&w, "water");
    w.paint(40, 30, 12, rock, Brush::Replace).unwrap();
    let body = w.body_at(40, 30).unwrap();
    w.set_anchored(body, true);
    // Overwriting a cell makes it whole again if it is rigid once more.
    w.set_cell(40, 18, water).unwrap();
    w.set_cell(40, 18, rock).unwrap();
    assert_eq!(w.cell_mask(40, 18), FULL_MASK);
    // Scooping a hole in the middle carves a round hollow into the cells
    // around it, and the erased cells keep no outline.
    let mut tools = Tools::new(rock);
    tools.select(PaletteItem::Tool(Tool::Scoop));
    tools.press(&mut w, Button::Primary, (40, 30));
    tools.release(&mut w, Button::Primary);
    let r = i64::from(tools.radius());
    assert_eq!(w.cell(40, 30).unwrap().material, water);
    assert_eq!(w.rigid_mask(30 * 80 + 40), None);
    let rim = usize::try_from(40 + r + 1).unwrap();
    assert_eq!(w.cell(rim, 30).unwrap().material, rock);
    let carved = w.cell_mask(rim, 30);
    assert_ne!(
        carved, FULL_MASK,
        "the rim cell right of the hole is carved"
    );
    // Its left (hole-facing) quarters go, its right ones stay.
    assert_eq!(carved & 0x1111, 0, "{carved:#06x}");
    assert_eq!(carved & 0x8888, 0x8888, "{carved:#06x}");
}

#[test]
fn outlines_fall_with_their_body() {
    let mut w = world(60, 80);
    let rock = id(&w, "rock");
    w.paint(30, 20, 6, rock, Brush::Replace).unwrap();
    let before = masks(&w);
    let top = before.keys().map(|k| k.1).min().unwrap();
    // Let it drop to the floor (it lands upright: a disc doesn't tip).
    for _ in 0..400 {
        w.tick();
    }
    let after = masks(&w);
    let landed = after.keys().map(|k| k.1).min().unwrap();
    assert!(landed > top + 20, "fell from {top} to {landed}");
    assert_eq!(after.len(), before.len());
    let dy = landed - top;
    let dx_set: Vec<i64> = after
        .keys()
        .zip(before.keys())
        .map(|(a, b)| {
            #[allow(clippy::cast_possible_wrap)]
            let d = a.0 as i64 - b.0 as i64;
            d
        })
        .collect();
    let dx = dx_set[0];
    for (((ax, ay), am), ((bx, by), bm)) in after.iter().zip(before.iter()) {
        #[allow(clippy::cast_possible_wrap)]
        let shifted = (*ax as i64 - dx, *ay as i64 - dy as i64);
        #[allow(clippy::cast_possible_wrap)]
        let orig = (*bx as i64, *by as i64);
        assert_eq!(shifted, orig);
        assert_eq!(am, bm, "mask at ({ax}, {ay})");
    }
}

#[test]
fn saves_keep_outlines() {
    let mut w = world(60, 60);
    let rock = id(&w, "rock");
    let wood = id(&w, "driftwood");
    w.paint(20, 30, 7, rock, Brush::Replace).unwrap();
    w.paint(45, 40, 5, wood, Brush::Replace).unwrap();
    for _ in 0..30 {
        w.tick();
    }
    let bytes = save::encode(&w);
    let back = save::decode(&bytes, MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(masks(&back), masks(&w));
    assert_eq!(back.state_hash(), w.state_hash());
    // And they keep running the same.
    let (mut a, mut b) = (w, back);
    for _ in 0..200 {
        a.tick();
        b.tick();
    }
    assert_eq!(a.state_hash(), b.state_hash());
    assert_eq!(masks(&a), masks(&b));
}
