//! SPEC T4b and T4c acceptance tests: rigid bodies (D17) move as one piece,
//! accelerate, feel buoyancy and drag, topple, embed in sand, rest on
//! powders, the floor and glued bodies, conserve mass, split when cut, stay
//! put when glued, and let a settled scene sleep.

use desktank_sim::tools::{Button, Outcome, PaletteItem, Tool, Tools};
use desktank_sim::{
    BodyId, Brush, GLUED_VARIANT_BIT, MaterialId, MaterialTable, Rect, World, WorldConfig,
};

fn world(width: usize, height: usize, seed: u64, water_level: f32) -> World {
    World::new(
        WorldConfig {
            width,
            height,
            seed,
            water_level,
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

/// The cells of `material`, row-major.
fn positions(w: &World, m: MaterialId) -> Vec<(usize, usize)> {
    w.cells()
        .iter()
        .enumerate()
        .filter(|(_, c)| c.material == m)
        .map(|(i, _)| (i % w.width(), i / w.width()))
        .collect()
}

/// `cells` moved down by `dy` rows.
fn shifted(cells: &[(usize, usize)], dy: usize) -> Vec<(usize, usize)> {
    cells.iter().map(|&(x, y)| (x, y + dy)).collect()
}

/// Ticks until every chunk sleeps; panics after `limit` ticks.
fn settle(w: &mut World, limit: u32) {
    for _ in 0..limit {
        w.tick();
        if w.awake_chunks() == 0 {
            return;
        }
    }
    panic!("not settled after {limit} ticks");
}

/// A sand bed `depth` rows deep on the floor.
fn sand_bed(w: &mut World, depth: usize) {
    let (width, height, sand) = (w.width(), w.height(), id(w, "sand"));
    w.fill_rect(rect(0, height - depth, width, height), sand, Brush::Replace)
        .unwrap();
}

/// Drops a rock disc into water and checks every tick that it moves as one
/// piece: the same shape, shifted straight down, never more than four rows per
/// tick. Returns how far it sank.
fn drop_rock(w: &mut World, x: i64, y: i64, radius: u32) -> usize {
    let rock = id(w, "rock");
    w.paint(x, y, radius, rock, Brush::Replace).unwrap();
    let shape = positions(w, rock);
    let mut dy = 0;
    for _ in 0..400 {
        w.tick();
        let now = positions(w, rock);
        let step = now[0].1 - shape[0].1 - dy;
        assert!(step <= 4, "moved {step} rows in a tick");
        dy += step;
        assert_eq!(now, shifted(&shape, dy), "lost its shape");
        if w.awake_chunks() == 0 {
            break;
        }
    }
    assert_eq!(w.bodies().count(), 1);
    dy
}

#[test]
fn rock_sinks_as_one_piece_and_rests_on_sand() {
    let mut w = world(96, 96, 1, 0.9);
    sand_bed(&mut w, 10);
    let sand_before = positions(&w, id(&w, "sand"));
    let dy = drop_rock(&mut w, 48, 30, 5);
    let rock = positions(&w, id(&w, "rock"));
    let bottom = rock.iter().map(|p| p.1).max().unwrap();
    assert!(bottom >= 86);
    assert_eq!(dy, bottom - 35);
    assert_eq!(positions(&w, id(&w, "sand")).len(), sand_before.len());
    let mut gentle = world(96, 96, 1, 0.9);
    sand_bed(&mut gentle, 10);
    assert!(85 + drop_rock(&mut gentle, 48, 80, 5) < bottom);
}

#[test]
fn rock_sinks_through_air_and_water_to_the_floor() {
    let mut w = world(64, 80, 2, 0.5);
    drop_rock(&mut w, 32, 6, 3);
    let rock = positions(&w, id(&w, "rock"));
    assert_eq!(rock.iter().map(|p| p.1).max(), Some(79));
}

#[test]
fn mass_is_conserved_while_bodies_sink() {
    let mut w = world(128, 96, 3, 0.7);
    let (rock, bubble) = (id(&w, "rock"), id(&w, "air_bubble"));
    sand_bed(&mut w, 6);
    // Rocks above and below the surface, and bubbles under one of them.
    w.paint(30, 10, 6, rock, Brush::Replace).unwrap();
    w.paint(90, 50, 8, rock, Brush::Replace).unwrap();
    w.fill_rect(rect(80, 60, 100, 64), bubble, Brush::Replace)
        .unwrap();
    // Bubbles pop into air at the surface: count them together.
    let air = id(&w, "air");
    let counts = |w: &World| {
        let mut c = w.material_counts();
        c[usize::from(air.0)] += c[usize::from(bubble.0)];
        c[usize::from(bubble.0)] = 0;
        c
    };
    let before = counts(&w);
    let start = w.cells().to_vec();
    for _ in 0..300 {
        w.tick();
        assert_eq!(counts(&w), before);
    }
    let moved = start
        .iter()
        .zip(w.cells())
        .filter(|(a, b)| a.material != b.material)
        .count();
    assert!(moved > 100, "only {moved} cells changed");
}

#[test]
fn powder_and_liquid_rest_on_a_body() {
    let mut w = world(96, 64, 4, 0.0);
    let (rock, sand, water) = (id(&w, "rock"), id(&w, "sand"), id(&w, "water"));
    // A slab on the floor, sand and water poured onto it.
    w.fill_rect(rect(20, 54, 76, 64), rock, Brush::Replace)
        .unwrap();
    w.paint(48, 10, 2, sand, Brush::Pour).unwrap();
    settle(&mut w, 2000);
    assert!(positions(&w, sand).iter().all(|p| p.1 < 54));
    w.paint(30, 10, 1, water, Brush::Pour).unwrap();
    settle(&mut w, 2000);
    assert_eq!(
        positions(&w, rock),
        (54..64)
            .flat_map(|y| (20..76).map(move |x| (x, y)))
            .collect::<Vec<_>>()
    );
}

#[test]
fn glued_body_stays_put_while_its_support_is_scooped() {
    let mut w = world(96, 96, 5, 0.9);
    sand_bed(&mut w, 30);
    let rock = id(&w, "rock");
    // Half buried in the sand.
    w.paint(48, 66, 6, rock, Brush::Replace).unwrap();
    w.tick();
    let body = w.body_at(48, 66).unwrap();
    let mut tools = Tools::new(rock);
    tools.select(PaletteItem::Tool(Tool::Glue));
    assert_eq!(
        tools.press(&mut w, Button::Primary, (48, 66)),
        Outcome::Glued(1)
    );
    tools.release(&mut w, Button::Primary);
    assert!(w.body(body).unwrap().anchored);
    assert!(
        positions(&w, rock)
            .iter()
            .all(|&(x, y)| w.cell(x, y).unwrap().variant & GLUED_VARIANT_BIT != 0)
    );
    let shape = positions(&w, rock);
    // Scoop all the sand and water under and around it.
    tools.select(PaletteItem::Tool(Tool::Scoop));
    for y in (76..96).step_by(4) {
        tools.press(&mut w, Button::Primary, (20, y));
        tools.moved(&mut w, (76, y));
        tools.release(&mut w, Button::Primary);
    }
    assert_eq!(w.cell(48, 74).unwrap().material, id(&w, "water"));
    for _ in 0..200 {
        w.tick();
    }
    assert_eq!(positions(&w, rock), shape);
    // Ungluing (a click toggles) lets it drop.
    tools.select(PaletteItem::Tool(Tool::Glue));
    tools.press(&mut w, Button::Primary, (48, 66));
    tools.release(&mut w, Button::Primary);
    assert!(!w.body(body).unwrap().anchored);
    for _ in 0..10 {
        w.tick();
    }
    let now = positions(&w, rock);
    assert!(now[0].1 > shape[0].1);
    assert_eq!(now, shifted(&shape, now[0].1 - shape[0].1));
}

#[test]
fn a_rock_cut_in_half_becomes_two_bodies_that_move_independently() {
    let mut w = world(96, 64, 6, 1.0);
    let (rock, water) = (id(&w, "rock"), id(&w, "water"));
    // A glued rock pillar (its own body) holding up the left end of a slab.
    w.fill_rect(rect(20, 30, 40, 64), rock, Brush::Replace)
        .unwrap();
    w.set_anchored(BodyId(1), true);
    w.fill_rect(rect(20, 24, 60, 30), rock, Brush::Replace)
        .unwrap();
    let slab = w.body_at(40, 24).unwrap();
    assert_eq!(w.body(slab).unwrap().bounds, rect(20, 24, 60, 30));
    // Cut through the middle; the split applies at the next tick.
    w.fill_rect(rect(38, 24, 40, 30), water, Brush::Replace)
        .unwrap();
    w.tick();
    let bodies: Vec<_> = w.bodies().collect();
    assert_eq!(bodies.len(), 3);
    assert_eq!(
        bodies[1].id, slab,
        "the piece with the first cell keeps the id"
    );
    assert_eq!(bodies[1].bounds, rect(20, 24, 38, 30));
    assert_eq!(bodies[2].bounds.x0, 40);
    assert!(!bodies[2].anchored);
    settle(&mut w, 400);
    // The supported half stays; the other falls to the floor.
    let after: Vec<_> = w.bodies().collect();
    assert_eq!(after[1].bounds, rect(20, 24, 38, 30));
    assert_eq!(after[2].bounds, rect(40, 58, 60, 64));
    assert_eq!(after[2].cells, 20 * 6);
}

#[test]
fn a_stroke_makes_one_body_held_until_released() {
    let mut w = world(128, 96, 7, 0.9);
    let rock = id(&w, "rock");
    let mut tools = Tools::new(rock);
    tools.press(&mut w, Button::Primary, (30, 40));
    for x in (32..=90).step_by(2) {
        tools.moved(&mut w, (x, 40));
        w.tick();
        tools.hold(&mut w);
    }
    // Held while drawing: still where it was poured, one body.
    assert_eq!(w.bodies().count(), 1);
    let shape = positions(&w, rock);
    assert_eq!(shape.iter().map(|p| p.1).min(), Some(37));
    for _ in 0..10 {
        w.tick();
        tools.hold(&mut w);
    }
    assert_eq!(positions(&w, rock), shape);
    tools.release(&mut w, Button::Primary);
    for _ in 0..12 {
        w.tick();
    }
    // Released: it drops as one piece (after the two-tick hold).
    let now = positions(&w, rock);
    assert!(now[0].1 > shape[0].1);
    assert_eq!(now, shifted(&shape, now[0].1 - shape[0].1));
    assert_eq!(w.bodies().count(), 1);
}

#[test]
fn glue_drag_glues_every_body_it_crosses() {
    let mut w = world(128, 96, 8, 1.0);
    let rock = id(&w, "rock");
    for x in [20, 50, 80, 110] {
        w.paint(x, 90, 3, rock, Brush::Replace).unwrap();
    }
    w.tick();
    assert_eq!(w.bodies().count(), 4);
    let mut tools = Tools::new(rock);
    tools.select(PaletteItem::Tool(Tool::Glue));
    tools.press(&mut w, Button::Primary, (20, 92));
    // The first body sets the way (glue); the drag glues the next two.
    assert_eq!(tools.moved(&mut w, (80, 92)), Outcome::Glued(2));
    tools.release(&mut w, Button::Primary);
    let glued: Vec<_> = w.bodies().map(|b| b.anchored).collect();
    assert_eq!(glued, [true, true, true, false]);
    // Sand isn't a body: glue does nothing there.
    w.paint(60, 10, 2, id(&w, "sand"), Brush::Pour).unwrap();
    assert_eq!(
        tools.press(&mut w, Button::Primary, (60, 10)),
        Outcome::Glued(0)
    );
}

#[test]
fn a_body_lighter_than_water_rises_and_floats() {
    let wood = r##"id = 13
name = "wood"
class = "rigid"
density = 0.6
colour = "#7a5a3a""##;
    let water = include_str!("../../../assets/materials/water.toml");
    let air = include_str!("../../../assets/materials/air.toml");
    let table = MaterialTable::from_toml_sources(&[water, air, wood]).unwrap();
    let config = WorldConfig {
        width: 64,
        height: 64,
        seed: 9,
        water_level: 0.5,
    };
    let mut w = World::new(config, table).unwrap();
    let wood = id(&w, "wood");
    w.fill_rect(rect(20, 50, 40, 54), wood, Brush::Replace)
        .unwrap();
    settle(&mut w, 400);
    // It rose until its top met the air (lighter than it) at the surface.
    let ps = positions(&w, wood);
    assert_eq!(ps.len(), 80);
    assert!(ps.first().unwrap().1 < w.surface_row() && ps.last().unwrap().1 >= w.surface_row());
}

#[test]
fn settled_scene_with_bodies_sleeps() {
    let mut w = world(192, 128, 10, 0.9);
    sand_bed(&mut w, 12);
    let rock = id(&w, "rock");
    for (i, x) in [30, 70, 110, 150].into_iter().enumerate() {
        w.paint(
            x,
            20 + 10 * i64::try_from(i).unwrap(),
            6,
            rock,
            Brush::Replace,
        )
        .unwrap();
    }
    // A rock dropped onto another.
    w.paint(70, 2, 4, rock, Brush::Replace).unwrap();
    settle(&mut w, 1000);
    assert_eq!(w.bodies().count(), 5);
    let bodies: Vec<_> = w.bodies().collect();
    let cells = w.cells().to_vec();
    for _ in 0..100 {
        w.tick();
        assert_eq!(w.awake_chunks(), 0);
    }
    assert_eq!(w.cells(), &cells[..]);
    assert_eq!(w.bodies().collect::<Vec<_>>(), bodies);
}

#[test]
fn set_cell_on_a_body_keeps_it_whole() {
    let mut w = world(64, 64, 12, 1.0);
    let rock = id(&w, "rock");
    w.fill_rect(rect(10, 58, 30, 64), rock, Brush::Replace)
        .unwrap();
    w.set_cell(20, 60, rock).unwrap();
    w.tick();
    assert_eq!(w.bodies().count(), 1);
}

#[test]
fn painting_with_every_body_id_taken_stays_cheap() {
    // 65 535 one-cell bodies fill the id space; more rock is then static.
    let mut w = world(256, 260, 13, 0.0);
    let rock = id(&w, "rock");
    for i in 0..usize::from(u16::MAX) {
        w.set_cell(i % 256, i / 256, rock).unwrap();
    }
    assert_eq!(w.bodies().count(), usize::from(u16::MAX));
    let start = std::time::Instant::now();
    w.fill_rect(rect(0, 259, 256, 260), rock, Brush::Replace)
        .unwrap();
    assert!(start.elapsed().as_secs() < 5, "id exhaustion is slow");
    assert_eq!(w.cell(5, 259).unwrap().state, 0, "static, no body");
}

/// A scene that pours, glues, cuts and drops bodies; returns the hash.
fn body_run(seed: u64) -> (u64, Vec<(BodyId, Rect, bool)>) {
    let mut w = world(128, 96, seed, 0.85);
    sand_bed(&mut w, 8);
    let (rock, sand) = (id(&w, "rock"), id(&w, "sand"));
    let mut tools = Tools::new(rock);
    for t in 0..1500_i64 {
        match t {
            0 => {
                tools.press(&mut w, Button::Primary, (20, 20));
            }
            1..=30 => {
                tools.moved(&mut w, (20 + t, 20 + t / 3));
            }
            31 => {
                tools.release(&mut w, Button::Primary);
            }
            40 => {
                w.paint(90, 30, 7, rock, Brush::Replace).unwrap();
                tools.select(PaletteItem::Tool(Tool::Glue));
                tools.press(&mut w, Button::Primary, (90, 30));
                tools.release(&mut w, Button::Primary);
            }
            60..=200 if t % 7 == 0 => {
                w.paint(90 + (t % 13) - 6, 2, 2, sand, Brush::Pour).unwrap();
            }
            300 => {
                w.fill_rect(rect(88, 20, 92, 40), id(&w, "water"), Brush::Replace)
                    .unwrap();
            }
            // The split applies at the tick after the cut.
            301 => {
                tools.press(&mut w, Button::Primary, (98, 30));
                tools.release(&mut w, Button::Primary);
            }
            _ => {}
        }
        w.tick();
        tools.hold(&mut w);
    }
    let bodies = w.bodies().map(|b| (b.id, b.bounds, b.anchored)).collect();
    (w.state_hash(), bodies)
}

#[test]
fn bodies_are_deterministic() {
    let a = body_run(11);
    assert_eq!(a, body_run(11));
    // The scene did what it says: two halves of the glued rock, one freed.
    assert_eq!(a.1.len(), 3);
    assert_eq!(a.1.iter().filter(|b| b.2).count(), 1);
}

#[test]
fn rock_topples_off_its_point() {
    let mut w = world(64, 64, 22, 0.0);
    let rock = id(&w, "rock");
    w.fill_rect(rect(25, 45, 38, 64), rock, Brush::Replace)
        .unwrap();
    let air = id(&w, "air");
    w.fill_rect(rect(25, 59, 31, 64), air, Brush::Replace)
        .unwrap();
    w.fill_rect(rect(32, 59, 38, 64), air, Brush::Replace)
        .unwrap();
    let before = positions(&w, rock);
    settle(&mut w, 1000);
    let after = positions(&w, rock);
    assert_eq!(after.len(), before.len());
    assert_ne!(after, before);
    assert!(after.iter().filter(|p| p.1 == 63).count() > 1);
}

/// The topmost row of `material`'s cells after each of `ticks` ticks.
fn top_rows(w: &mut World, m: MaterialId, ticks: usize) -> Vec<usize> {
    (0..ticks)
        .map(|_| {
            w.tick();
            positions(w, m).first().unwrap().1
        })
        .collect()
}

/// Rows moved per tick, averaged over the last `n` of `rows`.
fn mean_speed(rows: &[usize], n: usize) -> f64 {
    let tail = &rows[rows.len() - n..];
    let moved = tail[0].abs_diff(tail[n - 1]);
    f64::from(u32::try_from(moved).unwrap()) / f64::from(u32::try_from(n - 1).unwrap())
}

#[test]
fn a_rock_accelerates_in_air_and_reaches_a_terminal_speed_in_water() {
    let mut speeds = Vec::new();
    for level in [0.0, 1.0] {
        let mut w = world(64, 256, 3, level);
        let rock = id(&w, "rock");
        w.fill_rect(rect(28, 4, 36, 10), rock, Brush::Replace)
            .unwrap();
        let rows = top_rows(&mut w, rock, 70);
        // It starts slowly and speeds up.
        assert!(rows[10] - rows[0] < rows[30] - rows[20], "{rows:?}");
        speeds.push((mean_speed(&rows[..50], 10), mean_speed(&rows, 10)));
    }
    let (air, water) = (speeds[0], speeds[1]);
    // In air it hits the speed cap; in water drag holds it near a cell per
    // tick, the same over the last 20 ticks.
    assert!(air.1 > 3.0, "air {air:?}");
    assert!((0.5..1.5).contains(&water.1), "water {water:?}");
    assert!((water.0 - water.1).abs() < 0.25, "water {water:?}");
}

#[test]
fn driftwood_rises_slower_than_rock_sinks() {
    let mut w = world(64, 256, 3, 1.0);
    let rock = id(&w, "rock");
    w.fill_rect(rect(28, 4, 36, 10), rock, Brush::Replace)
        .unwrap();
    let sinking = mean_speed(&top_rows(&mut w, rock, 80), 30);
    let mut w = world(64, 256, 3, 1.0);
    let wood = id(&w, "driftwood");
    w.fill_rect(rect(28, 240, 36, 246), wood, Brush::Replace)
        .unwrap();
    let rising = mean_speed(&top_rows(&mut w, wood, 80), 30);
    assert!(rising > 0.1, "wood {rising}");
    assert!(rising * 2.0 < sinking, "wood {rising}, rock {sinking}");
}

#[test]
fn a_flat_slab_lands_level_and_sleeps_without_jitter() {
    let mut w = world(96, 64, 5, 1.0);
    let rock = id(&w, "rock");
    w.fill_rect(rect(30, 40, 60, 44), rock, Brush::Replace)
        .unwrap();
    settle(&mut w, 300);
    let body = w.bodies().next().unwrap();
    assert_eq!(body.bounds, rect(30, 60, 60, 64));
    let rest = positions(&w, rock);
    for _ in 0..200 {
        w.tick();
    }
    assert_eq!(w.awake_chunks(), 0);
    assert_eq!(positions(&w, rock), rest);
}

#[test]
fn a_rock_overhanging_a_ledge_tips_down_a_staircase() {
    let mut w = world(96, 64, 4, 1.0);
    let rock = id(&w, "rock");
    // Glued steps, 4 wide and 3 rows apart, falling to the right.
    for s in 0..10 {
        w.fill_rect(
            rect(10 + s * 4, 30 + s * 3, 14 + s * 4, 64),
            rock,
            Brush::Replace,
        )
        .unwrap();
    }
    let steps: Vec<_> = w.bodies().map(|b| b.id).collect();
    for s in &steps {
        w.set_anchored(*s, true);
    }
    let stairs: Vec<_> = steps.iter().map(|&s| w.body(s).unwrap().bounds).collect();
    // A block on the top step, its centre past the step's edge.
    w.fill_rect(rect(11, 24, 17, 30), rock, Brush::Replace)
        .unwrap();
    let block = w.body_at(12, 26).unwrap();
    settle(&mut w, 2000);
    let end = w.body(block).unwrap();
    assert_eq!(end.cells, 36);
    assert!(end.bounds.x0 > 11 && end.bounds.y1 > 30, "{:?}", end.bounds);
    // The glued steps never moved.
    let now: Vec<_> = steps.iter().map(|&s| w.body(s).unwrap().bounds).collect();
    assert_eq!(now, stairs);
}

#[test]
fn a_rock_rests_on_a_glued_pillar() {
    let mut w = world(64, 64, 8, 1.0);
    let rock = id(&w, "rock");
    w.fill_rect(rect(26, 40, 38, 64), rock, Brush::Replace)
        .unwrap();
    let pillar = w.bodies().next().unwrap().id;
    w.set_anchored(pillar, true);
    w.fill_rect(rect(27, 20, 37, 26), rock, Brush::Replace)
        .unwrap();
    let top = w.body_at(30, 22).unwrap();
    settle(&mut w, 500);
    assert_eq!(w.body(pillar).unwrap().bounds, rect(26, 40, 38, 64));
    assert_eq!(w.body(top).unwrap().bounds, rect(27, 34, 37, 40));
}
