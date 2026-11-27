//! Under-gravel filter (UGF) integration tests (D27, DV6, #162): floor mount
//! under existing substrate (every displaced cell kept, the bed resting on
//! the plate), a telescoping uplift tube whose spout sits just under the
//! water line, left/right flip, refusal when blocked or too shallow,
//! substrate-preserving removal, down-flow through the gravel and the spray
//! bar's jet, rooted plants declining over the plate while floaters, ball
//! plants and rooted plants beside it stay healthy, filtration at the top of
//! the bed, clogging and rinsing, fry safety, zero idle cost and save/load
//! determinism; plus the modular under-gravel panels (snapping flush,
//! chaining, connecting to the main filter, disconnecting).

use std::collections::BTreeMap;

use desktank_sim::{
    Brush, CLOG_FLOW_FLOOR, Device, DeviceId, DeviceKind, MaterialId, MaterialTable, Mount,
    Placeable, Propagule, Rect, RinseKind, Traits, World, WorldConfig, save,
};

const KIND: DeviceKind = DeviceKind::UnderGravelFilter;
const PANEL: DeviceKind = DeviceKind::UnderGravelPanel;
/// Ticks in a simulated minute.
const MINUTE: u32 = 1200;

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

fn run(w: &mut World, ticks: u32) {
    for _ in 0..ticks {
        w.tick();
    }
}

fn count(w: &World, name: &str) -> usize {
    w.material_counts()[usize::from(id(w, name).0)]
}

/// A 120x80 tank at 90% water (surface row 8; the plate's box is rows 52..80)
/// with a `rows`-deep bed of `substrate` on the floor.
fn bedded(seed: u64, substrate: &str, rows: usize) -> World {
    let mut w = world(120, 80, seed, 0.9);
    if rows > 0 {
        let s = id(&w, substrate);
        w.fill_rect(rect(0, 80 - rows, 120, 80), s, Brush::Replace)
            .unwrap();
    }
    w
}

/// Lets the tank settle: ticks until no chunk is awake or 1500 ticks pass
/// (a filter's bubbles keep the water above it awake, so with one placed
/// this runs the whole 1500).
fn settle(w: &mut World) {
    for _ in 0..1500 {
        w.tick();
        if w.awake_chunks() == 0 {
            return;
        }
    }
}

/// Adds the filter centred on `x` and returns its id and a copy.
fn add(w: &mut World, x: i64, flip: bool) -> (DeviceId, Device) {
    let dev = w
        .add_device(KIND, x, 40, flip)
        .unwrap()
        .expect("the under-gravel filter fits");
    (dev, *w.device(dev).unwrap())
}

/// The topmost row of `substrate` in column `x` (the bed's surface).
fn top_of_bed(w: &World, x: usize, substrate: &str) -> usize {
    let m = id(w, substrate);
    (0..w.height())
        .find(|&y| w.cell(x, y).unwrap().material == m)
        .unwrap()
}

#[test]
fn floor_mount_lays_flush_on_the_floor_flips_and_refuses_when_blocked_or_shallow() {
    let mut w = bedded(1, "gravel", 0);
    let art = w.device_kinds().get(KIND).clone();
    assert_eq!(art.mount, Mount::Floor);
    assert_eq!(art.size, (28, 28));

    // Whatever the pointer height, the tube starts just under the water line
    // (row 8 here), the plate lies flush with the floor, centred on x and
    // clamped into the tank.
    let (ghost, ok) = w.device_placement(KIND, 60, 10, false);
    assert!(ok);
    assert_eq!((ghost.x, ghost.y), (60 - 14, w.surface_row() + 1));
    assert_eq!(ghost.y + ghost.height, 80);
    assert!(!ghost.facing_left);
    assert_eq!(w.device_placement(KIND, -30, 70, false).0.x, 0);
    assert_eq!(w.device_placement(KIND, 500, 70, false).0.x, 120 - 28);
    let (flipped, ok) = w.device_placement(KIND, 60, 10, true);
    assert!(ok && flipped.facing_left);

    // Placing writes the whole footprint as device cells.
    let cells = ghost.cells(&art).count();
    assert!(cells > art.cells, "the tube stretched: {cells}");
    assert_eq!(
        w.place(Placeable::UnderGravelFilter, 60, 10).unwrap(),
        cells
    );
    let device = id(&w, "device");
    assert_eq!(count(&w, "device"), cells);
    let (dev, d) = (w.devices().next().unwrap().id, *w.devices().next().unwrap());
    assert_eq!(w.cell(60, 79).unwrap().material, device);
    assert_eq!(w.device_at(60, 79), Some(dev));
    assert!(w.remove_device(dev));
    assert_eq!(count(&w, "device"), 0);

    // Flipping mirrors the corner uplift tube and spray bar left/right.
    let (_, left) = add(&mut w, 60, false);
    assert!(
        left.covers(&art, left.x + 2, left.y + 10),
        "tube at the back left"
    );
    assert!(!left.covers(&art, left.x + 25, left.y + 10));
    assert_eq!(left.nozzle(&art), Some((left.x + 6, left.y + 2)));
    assert!(left.nozzle_dir(&art)[0] > 0.0);
    assert_eq!(left.x, d.x);
    w.remove_device(left.id);
    let (_, right) = add(&mut w, 60, true);
    assert!(right.facing_left);
    assert!(
        right.covers(&art, right.x + 25, right.y + 10),
        "tube at the back right"
    );
    assert!(!right.covers(&art, right.x + 2, right.y + 10));
    assert_eq!(right.nozzle(&art), Some((right.x + 21, right.y + 2)));
    assert!(right.nozzle_dir(&art)[0] < 0.0);
    w.remove_device(right.id);

    // Rock in the footprint (here on the plate) refuses the placement whole;
    // rock beside it, in the open part of the box, does not.
    let rock = id(&w, "rock");
    w.fill_rect(rect(100, 40, 110, 50), rock, Brush::Replace)
        .unwrap();
    assert!(w.device_placement(KIND, 60, 40, false).1);
    w.fill_rect(rect(60, 76, 64, 78), rock, Brush::Replace)
        .unwrap();
    let (_, fits) = w.device_placement(KIND, 60, 40, false);
    assert!(!fits, "rock on the plate refuses the placement");
    assert_eq!(w.place(Placeable::UnderGravelFilter, 60, 40).unwrap(), 0);
    assert_eq!(w.devices().count(), 0);

    // A second filter can't overlap the first: dropped on it, it snaps flush
    // to its nearer side instead, or is refused if there's no room.
    let mut w = bedded(2, "gravel", 0);
    let (_, first) = add(&mut w, 40, false);
    assert_eq!(first.x, 26);
    let (_, second) = add(&mut w, 50, false);
    assert_eq!(second.x, 54, "snapped flush against the first's right edge");
    assert!(w.add_device(KIND, 100, 40, false).unwrap().is_some());
    let mut narrow = world(60, 80, 2, 0.9);
    add(&mut narrow, 14, false);
    add(&mut narrow, 42, false);
    assert_eq!(narrow.add_device(KIND, 30, 40, false).unwrap(), None);
    assert_eq!(narrow.devices().count(), 2, "a refusal places nothing");

    // Too shallow: the water line is below the top of the box's spray bar.
    let mut shallow = world(100, 60, 3, 0.3); // surface row 42 > 60 - 28
    let (_, ok) = shallow.device_placement(KIND, 50, 50, false);
    assert!(!ok);
    assert_eq!(
        shallow.place(Placeable::UnderGravelFilter, 50, 50).unwrap(),
        0
    );

    // A tank too small for the plate refuses rather than clamping out of it.
    let mut tiny = world(20, 20, 4, 0.9);
    assert_eq!(tiny.place(Placeable::UnderGravelFilter, 10, 10).unwrap(), 0);
}

#[test]
fn a_buried_spray_bar_or_an_unplaceable_substrate_cell_refuses_the_placement() {
    // A bed so deep that the spray bar (just under the water line) would be
    // buried.
    let mut deep = bedded(5, "gravel", 70);
    let (_, ok) = deep.device_placement(KIND, 60, 40, false);
    assert!(!ok, "the spray bar can't be buried in the bed");
    assert_eq!(deep.place(Placeable::UnderGravelFilter, 60, 40).unwrap(), 0);
    assert_eq!(count(&deep, "gravel"), 120 * 70, "a refusal moves nothing");

    // A bed a little below the spray bar fits, and the tube column's
    // displaced gravel goes somewhere in the plate's width.
    let mut ok_tank = bedded(5, "gravel", 20);
    let before = count(&ok_tank, "gravel");
    assert!(ok_tank.add_device(KIND, 60, 40, false).unwrap().is_some());
    assert_eq!(count(&ok_tank, "gravel"), before);

    // A rock between the bed and the water line seals nothing off: it just
    // can't be part of the footprint.
    let mut w = bedded(6, "sand", 10);
    let rock = id(&w, "rock");
    // Rock in the open part of the box (above the bed, off the tube).
    w.fill_rect(rect(56, 60, 60, 64), rock, Brush::Replace)
        .unwrap();
    assert!(w.device_placement(KIND, 60, 40, false).1);
    // Rock where the tube would be.
    w.fill_rect(rect(48, 60, 50, 64), rock, Brush::Replace)
        .unwrap();
    assert!(!w.device_placement(KIND, 60, 40, false).1);
}

#[test]
fn under_a_gravel_bed_every_cell_is_kept_and_the_bed_rests_on_the_plate_then_flip_mirrors() {
    for flip in [false, true] {
        let mut w = bedded(7, "gravel", 12);
        let gravel_before = count(&w, "gravel");
        assert_eq!(gravel_before, 120 * 12);
        let art = w.device_kinds().get(KIND).clone();
        let (dev, d) = add(&mut w, 60, flip);
        assert_eq!(d.facing_left, flip);
        assert_eq!(
            count(&w, "gravel"),
            gravel_before,
            "every gravel cell is kept"
        );
        assert_eq!(count(&w, "device"), d.cells(&art).count());
        // Nothing was lost to the plate and nothing lies inside it.
        for (x, y) in d.cells(&art) {
            assert_eq!(w.device_at(x, y), Some(dev));
        }
        settle(&mut w);
        assert_eq!(count(&w, "gravel"), gravel_before);

        // The plate is the floor of the bed: gravel rests right on its top
        // row in every column the plate covers (apart from the tube's), with
        // the bed above it unbroken.
        let plate_top = d.y + d.height - 3;
        let far = if flip {
            d.x..d.x + 20
        } else {
            d.x + 8..d.x + 28
        };
        for x in far {
            assert_eq!(
                w.cell(x, plate_top - 1).unwrap().material,
                id(&w, "gravel"),
                "gravel rests on the plate at column {x}"
            );
            let top = top_of_bed(&w, x, "gravel");
            for y in top..plate_top {
                assert_eq!(
                    w.cell(x, y).unwrap().material,
                    id(&w, "gravel"),
                    "({x}, {y})"
                );
            }
            assert!(top < 80 - 12, "the bed rose over the plate: top {top}");
        }
        // Beside the plate the bed is as it was.
        for x in [2, 100] {
            if (d.x..d.x + 28).contains(&x) {
                continue;
            }
            assert_eq!(top_of_bed(&w, x, "gravel"), 80 - 12);
        }
        // The tube column's displaced gravel found a home in the plate's span.
        let uplift = if flip { d.x + 25 } else { d.x + 2 };
        assert!((d.x..d.x + 28).contains(&uplift));
        assert_eq!(w.cell(uplift, d.y + 10).unwrap().material, id(&w, "device"));
        assert_eq!(
            w.cell(uplift, plate_top - 8).unwrap().material,
            id(&w, "device")
        );

        // Over a bare floor the gravel poured afterwards rests on the plate.
        let mut bare = bedded(8, "gravel", 0);
        let (_, bd) = add(&mut bare, 60, flip);
        let gravel = id(&bare, "gravel");
        let (px0, px1) = if flip {
            (bd.x, bd.x + 20)
        } else {
            (bd.x + 8, bd.x + 28)
        };
        bare.fill_rect(rect(px0, 40, px1, 44), gravel, Brush::Replace)
            .unwrap();
        settle(&mut bare);
        assert_eq!(
            top_of_bed(&bare, bd.x + 14, "gravel"),
            bd.y + bd.height - 3 - 4
        );
    }
}

/// The multiset of `(variant, state)` over every cell of `material`.
fn cell_census(w: &World, material: MaterialId) -> BTreeMap<(u8, u16), usize> {
    let mut census = BTreeMap::new();
    for y in 0..w.height() {
        for x in 0..w.width() {
            let c = w.cell(x, y).unwrap();
            if c.material == material {
                *census.entry((c.variant, c.state)).or_default() += 1;
            }
        }
    }
    census
}

#[test]
fn displaced_aquasoil_keeps_its_cells_exactly_and_a_thin_bed_rests_on_the_plate() {
    let mut w = bedded(9, "aquasoil", 12);
    let soil = id(&w, "aquasoil");
    let before = cell_census(&w, soil);
    assert!(before.len() > 1, "poured cells vary");
    add(&mut w, 60, false);
    assert_eq!(
        cell_census(&w, soil),
        before,
        "each displaced cell keeps its material, variant and state"
    );
    settle(&mut w);
    assert_eq!(count(&w, "aquasoil"), 120 * 12);

    // A bed thinner than the plate: the plate still goes in and the substrate
    // rests on top of it.
    let mut thin = bedded(10, "sand", 2);
    let (_, td) = add(&mut thin, 60, false);
    assert_eq!(count(&thin, "sand"), 120 * 2);
    settle(&mut thin);
    assert_eq!(count(&thin, "sand"), 120 * 2);
    for x in td.x + 8..td.x + 28 {
        assert_eq!(
            thin.cell(x, td.y + td.height - 4).unwrap().material,
            id(&thin, "sand"),
            "sand rests on the plate at {x}"
        );
    }
}

#[test]
fn removal_leaves_every_substrate_cell_which_settles_back_to_the_floor() {
    let mut w = bedded(11, "gravel", 12);
    let gravel_before = count(&w, "gravel");
    let (dev, d) = add(&mut w, 60, false);
    settle(&mut w);
    let art = w.device_kinds().get(KIND).clone();
    assert!(w.remove_device(dev));
    assert_eq!(count(&w, "device"), 0);
    assert_eq!(count(&w, "gravel"), gravel_before, "no substrate is lost");
    settle(&mut w);
    assert_eq!(count(&w, "gravel"), gravel_before);
    // Every column rests on the floor again with no hole under the bed.
    for x in 0..120 {
        let top = top_of_bed(&w, x, "gravel");
        for y in top..80 {
            assert_eq!(
                w.cell(x, y).unwrap().material,
                id(&w, "gravel"),
                "hole at ({x}, {y}) under the bed after removal"
            );
        }
    }
    // The plate's old cells are water or gravel, never air or device.
    for (x, y) in d.cells(&art) {
        let m = w.cell(x, y).unwrap().material;
        assert!(m == id(&w, "water") || m == id(&w, "gravel"));
    }

    // Scooping any cell of a placed one removes the whole device and likewise
    // leaves the substrate.
    let (dev, d) = add(&mut w, 60, false);
    let (sx, sy) = d.cells(&art).next().unwrap();
    w.set_cell(sx, sy, id(&w, "water")).unwrap();
    assert!(w.device(dev).is_none());
    assert_eq!(count(&w, "device"), 0);
    assert_eq!(count(&w, "gravel"), gravel_before);
}

#[test]
fn water_is_drawn_down_through_the_gravel_and_the_spray_bar_drives_a_jet() {
    for flip in [false, true] {
        let mut w = bedded(12, "gravel", 12);
        let art = w.device_kinds().get(KIND).clone();
        let (dev, d) = add(&mut w, 60, flip);
        run(&mut w, 10);

        // Over the plate: open water above the bed and the gravel in it both
        // flow downwards, into the plate.
        let x = if flip { d.x + 8 } else { d.x + 16 };
        let top = top_of_bed(&w, x, "gravel");
        let [_, above] = w.current_at(x, top - 3);
        assert!(
            above > 0.0,
            "open water above the gravel is drawn down, got {above}"
        );
        let [_, inside] = w.current_at(x, d.y + d.height - 6);
        assert!(inside > 0.0, "the gravel bed seeps downwards, got {inside}");

        // The spray bar jets away from the tube along the surface-facing top.
        let (nx, ny) = d.nozzle(&art).unwrap();
        let dir = d.nozzle_dir(&art)[0];
        let reach = if flip { nx - 10 } else { nx + 10 };
        let [vx, _] = w.current_at(reach, ny);
        assert!(
            vx * dir > 1.0,
            "the spray bar jets sideways, got vx={vx} for dir {dir}"
        );
        assert!(w.device_status(dev).unwrap().flow > 0.0);
    }

    // Beside the plate, away from any flow, the gravel is still.
    let mut w = bedded(13, "gravel", 12);
    add(&mut w, 30, false);
    run(&mut w, 10);
    assert_eq!(w.current_at(110, 76), [0.0, 0.0]);
}

#[test]
fn removing_the_filter_ends_its_current() {
    let mut w = bedded(14, "gravel", 12);
    let (dev, d) = add(&mut w, 60, false);
    run(&mut w, 5);
    assert!(w.current_at(d.x + 16, 60)[1] != 0.0);
    w.remove_device(dev);
    run(&mut w, 5);
    assert_eq!(w.current_at(d.x + 16, 60), [0.0, 0.0]);
    assert_eq!(w.current_at(d.x + 16, d.y + 22), [0.0, 0.0]);
}

/// A tall tank with a fed aquasoil bed, the filter under the left of it and
/// the right left free, with the same plants on each side. Returns the world
/// and the filter's device.
fn planted_tank(seed: u64, with_filter: bool) -> (World, Option<Device>) {
    let mut w = world(200, 100, seed, 0.95);
    let soil = id(&w, "aquasoil");
    w.fill_rect(rect(0, 88, 200, 100), soil, Brush::Replace)
        .unwrap();
    let d = with_filter.then(|| {
        let id = w.add_device(KIND, 40, 60, false).unwrap().expect("fits");
        *w.device(id).unwrap()
    });
    (w, d)
}

fn plant(w: &mut World, name: &str, x: i64) {
    let s = w.plant_species().id(name).unwrap();
    let p = w.plant_species().get(s).unwrap().default_propagule();
    w.paint_plant(x, 20, 0, s, p).unwrap();
}

fn health_of(w: &World, name: &str, x_range: core::ops::Range<usize>) -> Vec<f32> {
    let s = w.plant_species().id(name).unwrap();
    w.plants()
        .filter(|p| p.species == s && x_range.contains(&p.base.0))
        .map(|p| p.health)
        .collect()
}

#[test]
fn rooted_plants_over_the_plate_decline_while_those_beside_it_and_floaters_stay_healthy() {
    let (mut w, d) = planted_tank(15, true);
    let d = d.unwrap();
    let over = d.x + 8..d.x + 28;
    let beside = 100..200;
    let rooted = ["rotala", "vallisneria", "dwarf_hairgrass"];
    let dx = i64::try_from(d.x).unwrap();
    for (k, name) in rooted.into_iter().enumerate() {
        let off = i64::try_from(k).unwrap() * 4;
        plant(&mut w, name, dx + 10 + off);
        plant(&mut w, name, 140 + off);
    }
    // Non-rooted plants over the plate: a floater at the top of the water, a
    // marimo ball that sinks to the bed (and rolls along it in the current)
    // and an anubias epiphyte on an anchored rock above it.
    plant(&mut w, "duckweed", dx + 19);
    plant(&mut w, "marimo", dx + 27);
    let rock = id(&w, "rock");
    w.fill_rect(rect(d.x + 20, 72, d.x + 26, 76), rock, Brush::Replace)
        .unwrap();
    let body = w.body_at(d.x + 20, 72).unwrap();
    assert!(w.set_anchored(body, true));
    let anubias = w.plant_species().id("anubias").unwrap();
    w.paint_plant(dx + 23, 68, 1, anubias, Propagule::Fragments)
        .unwrap();
    run(&mut w, MINUTE);
    let floaters = health_of(&w, "duckweed", 0..200);
    assert!(
        !floaters.is_empty() && floaters.iter().all(|&h| h >= 0.99),
        "{floaters:?}"
    );
    let rocks = health_of(&w, "anubias", over.clone());
    assert!(
        !rocks.is_empty() && rocks.iter().all(|&h| h >= 0.99),
        "an epiphyte over the plate is unharmed: {rocks:?}"
    );
    let ball = health_of(&w, "marimo", 0..200);
    assert!(
        !ball.is_empty() && ball.iter().all(|&h| h >= 0.99),
        "{ball:?}"
    );

    for name in rooted {
        // Beside the plate they thrive; over it, with just as good light,
        // nutrients and CO2, they are already declining.
        let beside_health = health_of(&w, name, beside.clone());
        assert!(
            !beside_health.is_empty(),
            "{name} beside the plate is alive"
        );
        assert!(
            beside_health.iter().all(|&h| h >= 0.99),
            "{name} beside the plate stays healthy: {beside_health:?}"
        );
        let over_health = health_of(&w, name, over.clone());
        assert!(
            !over_health.is_empty(),
            "{name} over the plate is still alive"
        );
        assert!(
            over_health.iter().all(|&h| h < 0.85),
            "{name} over the plate declines: {over_health:?}"
        );
    }
    run(&mut w, 5 * MINUTE);
    for name in rooted {
        assert!(
            health_of(&w, name, over.clone()).is_empty(),
            "{name} over the plate eventually dies"
        );
        let beside_health = health_of(&w, name, beside.clone());
        assert!(!beside_health.is_empty() && beside_health.iter().all(|&h| h >= 0.99));
    }
    // Floaters and epiphytes are not rooted: unaffected however long.
    for name in ["duckweed", "anubias"] {
        let health = health_of(&w, name, 0..200);
        assert!(
            !health.is_empty() && health.iter().all(|&h| h >= 0.99),
            "{name} is unaffected: {health:?}"
        );
    }
}

#[test]
fn a_removed_filter_stops_hurting_rooted_plants() {
    let (mut w, d) = planted_tank(16, true);
    let d = d.unwrap();
    plant(&mut w, "vallisneria", i64::try_from(d.x).unwrap() + 16);
    run(&mut w, 3 * MINUTE / 4);
    let hurt = health_of(&w, "vallisneria", d.x..d.x + 28);
    assert!(!hurt.is_empty(), "the plant is alive");
    assert!(
        hurt.iter().all(|&h| h < 0.95),
        "the plant is stressed over the plate: {hurt:?}"
    );
    assert!(w.remove_device(d.id));
    run(&mut w, 6 * MINUTE);
    let recovered = health_of(&w, "vallisneria", 0..200);
    assert!(!recovered.is_empty(), "the plant survived the removal");
    assert!(
        recovered.iter().all(|&h| h >= 0.99),
        "the plant recovers once the filter is gone: {recovered:?}"
    );
}

#[test]
fn the_filter_captures_particles_at_the_top_of_the_bed_clogs_and_rinses() {
    let mut w = bedded(17, "gravel", 12);
    let art = w.device_kinds().get(KIND).clone();
    let (dev, d) = add(&mut w, 60, false);
    settle(&mut w);
    assert!(art.is_filter());
    let (ix, _) = d.intakes(&art).nth(2).unwrap();
    let top = top_of_bed(&w, ix, "gravel");
    assert!(top < d.y + d.height - 3, "intakes lie under a bed");
    assert_eq!(w.device(dev).unwrap().media_load, 0.0);
    // Detritus resting on the gravel above an intake is drawn in.
    let detritus = id(&w, "detritus");
    w.set_cell(ix, top - 1, detritus).unwrap();
    w.tick();
    assert!(
        w.device(dev).unwrap().media_load > 0.0,
        "an intake captures detritus lying on the bed above it"
    );
    assert_eq!(w.cell(ix, top - 1).unwrap().material, id(&w, "water"));

    // Clogged, its flow falls towards the floor; a rinse restores it.
    let clean = w.device_status(dev).unwrap().flow;
    assert!(clean > 0.0);
    assert!(w.set_device_media_load(dev, 1.0));
    let clogged = w.device_status(dev).unwrap().flow;
    assert!(clogged < clean, "clogged {clogged} < clean {clean}");
    assert!(
        clogged > clean * CLOG_FLOW_FLOOR * 0.5,
        "a clogged filter never stops: {clogged} vs {clean}"
    );
    assert!(w.rinse_device(dev, RinseKind::Gentle));
    assert_eq!(w.device(dev).unwrap().media_load, 0.0);
    let rinsed = w.device_status(dev).unwrap().flow;
    assert!((rinsed - clean).abs() < 1e-4, "{rinsed} vs {clean}");

    // It matures like any filter.
    assert!(w.set_device_maturity(dev, 0.0));
    run(&mut w, 40);
    assert!(w.device(dev).unwrap().maturity > 0.0);
    assert!(w.filter_bio_capacity() > 0.0);
}

#[test]
fn the_filter_is_fry_safe_and_needs_no_intake_guard() {
    let mut w = bedded(18, "gravel", 12);
    let art = w.device_kinds().get(KIND).clone();
    let (dev, d) = add(&mut w, 60, false);
    settle(&mut w);
    assert!(art.fry_safe);
    assert!(w.device_status(dev).unwrap().fry_safe);
    assert_eq!(w.toggle_device_intake_guard(dev), Some(false));
    assert!(w.set_device_intake_guard(dev, true));
    assert!(!w.device(dev).unwrap().intake_guard);
    // A fry swimming right over an intake isn't sucked in.
    let guppy = w.species().id("guppy").unwrap();
    let fry = Traits {
        age: Some(0),
        ..Traits::default()
    };
    let (ix, _) = d.intakes(&art).nth(1).unwrap();
    let top = i64::try_from(top_of_bed(&w, ix, "gravel")).unwrap();
    let fish = w
        .add_creature_with_traits(guppy, i64::try_from(ix).unwrap(), top - 2, fry)
        .expect("fry spawns");
    run(&mut w, 40);
    assert!(
        w.creature(fish).is_some(),
        "the under-gravel filter doesn't suck in fry"
    );
}

#[test]
fn a_settled_filter_rebuilds_no_current_and_scans_nothing() {
    // The water line is off an 8-cell field-block boundary, so a bubble
    // reaching the surface doesn't flip a block between open and air (which
    // rebuilds the current, as for any air-driven device).
    let mut w = world(120, 80, 19, 0.88);
    assert_ne!(w.surface_row() % 8, 0);
    let gravel = id(&w, "gravel");
    w.fill_rect(rect(0, 68, 120, 80), gravel, Brush::Replace)
        .unwrap();
    add(&mut w, 60, false);
    settle(&mut w);
    let rebuilds = w.device_rebuilds();
    let scans = w.current_block_scans();
    let checks = w.intake_cell_checks();
    let intakes = w
        .device_kinds()
        .get(DeviceKind::UnderGravelFilter)
        .intakes
        .len() as u64;
    run(&mut w, 400);
    assert_eq!(
        w.device_rebuilds(),
        rebuilds,
        "no current rebuilds while idle"
    );
    assert_eq!(
        w.current_block_scans(),
        scans,
        "the settled bed scans no current blocks"
    );
    assert_eq!(
        w.intake_cell_checks() - checks,
        400 * 8 * intakes,
        "settled floor intakes cache their bed top and stay O(1) per tick"
    );
}

#[test]
fn under_gravel_filter_save_load_round_trip_replays_identically() {
    for flip in [false, true] {
        let mut w = bedded(20, "gravel", 12);
        let (dev, _) = add(&mut w, 55, flip);
        assert!(w.set_device_media_load(dev, 0.35));
        assert!(w.set_device_maturity(dev, 0.7));
        let tetra = w.species().id("neon_tetra").unwrap();
        w.add_creature(tetra, 20, 30).unwrap();
        run(&mut w, 40);

        let bytes = save::encode(&w);
        let mut loaded = save::decode(&bytes, MaterialTable::builtin().unwrap()).unwrap();
        assert_eq!(w.state_hash(), loaded.state_hash());
        let ld = *loaded.device(dev).unwrap();
        assert_eq!(ld.kind, KIND);
        assert_eq!(ld.facing_left, flip);
        assert!((ld.media_load - 0.35).abs() < 1e-6);
        assert_eq!(
            loaded.current(),
            w.current(),
            "the current is rebuilt identically"
        );

        for _ in 0..160 {
            w.tick();
            loaded.tick();
        }
        assert_eq!(
            w.state_hash(),
            loaded.state_hash(),
            "a loaded under-gravel tank replays bit-for-bit"
        );
    }
}

/// A 160x90 tank at 90% water (surface row 9) with a `rows`-deep gravel bed.
fn wide(seed: u64, rows: usize) -> World {
    let mut w = world(160, 90, seed, 0.9);
    let gravel = id(&w, "gravel");
    w.fill_rect(rect(0, 90 - rows, 160, 90), gravel, Brush::Replace)
        .unwrap();
    w
}

/// Adds a panel at pointer column `x` (it snaps to a neighbour) and returns it.
fn add_panel(w: &mut World, x: i64) -> Device {
    let dev = w
        .add_device(PANEL, x, 40, false)
        .unwrap()
        .expect("the under-gravel panel fits");
    *w.device(dev).unwrap()
}

#[test]
fn the_uplift_tube_telescopes_so_the_spout_sits_just_under_the_water_line() {
    let art = world(160, 90, 30, 0.9).device_kinds().get(KIND).clone();
    for (level, flip) in [(0.9, false), (0.9, true), (0.6, false), (0.45, true)] {
        let mut w = world(160, 90, 30, level);
        let gravel = id(&w, "gravel");
        w.fill_rect(rect(0, 78, 160, 90), gravel, Brush::Replace)
            .unwrap();
        let surface = w.surface_row();
        let (dev, d) = {
            let id = w.add_device(KIND, 60, 40, flip).unwrap().expect("fits");
            (id, *w.device(id).unwrap())
        };
        // The box starts right under the water line and reaches the floor.
        assert_eq!(d.y, surface + 1, "level {level}");
        assert_eq!(d.y + d.height, 90);
        assert_eq!(d.span(&art), 90 - surface - 1);
        // The spout and its bubble outlets sit near the top of the tank...
        let (nx, ny) = d.nozzle(&art).unwrap();
        assert_eq!(ny, surface + 1 + 2);
        for (_, by) in d.bubble_outlets(&art) {
            assert_eq!(by, surface + 1 + 1);
        }
        // ...and the intakes on the plate on the floor.
        for (_, iy) in d.intakes(&art) {
            assert_eq!(iy, 90 - 3);
        }
        // The clear tube is solid from the spout to the collar in its column.
        let tube_x = if flip { d.x + 25 } else { d.x + 2 };
        for y in d.y + 5..d.y + d.height - 11 {
            assert_eq!(
                w.device_at(tube_x, y),
                Some(dev),
                "tube cell ({tube_x}, {y})"
            );
        }
        assert_eq!(w.device_at(tube_x, 89), Some(dev));
        let cells: Vec<_> = d.cells(&art).collect();
        assert_eq!(count(&w, "device"), cells.len());
        assert!(cells.iter().all(|&(x, y)| w.device_at(x, y) == Some(dev)));
        // The jet leaves the spout near the surface, away from the tube.
        run(&mut w, 10);
        let dir = d.nozzle_dir(&art)[0];
        let reach = if flip { nx - 10 } else { nx + 10 };
        assert!(w.current_at(reach, ny)[0] * dir > 1.0, "level {level}");
        // Its bubbles come out of the spout at the top of the tank (and pop
        // at the surface a moment later).
        let bubble = id(&w, "air_bubble");
        let mut seen = false;
        for _ in 0..300 {
            w.tick();
            seen |= (surface..surface + 12)
                .flat_map(|y| (d.x..d.x + 28).map(move |x| (x, y)))
                .any(|(x, y)| w.cell(x, y).unwrap().material == bubble);
        }
        assert!(seen, "bubbles rise from the spout at level {level}");
        // Save/load keeps the telescoped height (rebuilt from the anchor).
        let bytes = save::encode(&w);
        let loaded = save::decode(&bytes, MaterialTable::builtin().unwrap()).unwrap();
        assert_eq!(loaded.device(dev), w.device(dev));
        assert_eq!(w.state_hash(), loaded.state_hash());
    }
}

#[test]
fn a_panel_snaps_flush_beside_the_filter_and_chains_under_existing_gravel() {
    let mut w = wide(31, 12);
    let gravel_before = count(&w, "gravel");
    let panel_art = w.device_kinds().get(PANEL).clone();
    assert_eq!(panel_art.size, (24, 3));
    let (ugf_id, ugf) = add(&mut w, 50, false);
    assert_eq!((ugf.x, ugf.x + 28), (36, 64));
    assert_eq!(count(&w, "gravel"), gravel_before);

    // Beside, on, or a few cells from the filter, it snaps to the nearer
    // flush edge; further off it stays where the pointer puts it.
    let place = |w: &World, x: i64| w.device_placement(PANEL, x, 10, false);
    let (ghost, ok) = place(&w, 80);
    assert!(ok && ghost.x == 64, "gap of 4 snaps: {}", ghost.x);
    assert_eq!(
        place(&w, 60).0.x,
        64,
        "overlapping the right half snaps right"
    );
    assert_eq!(
        place(&w, 30).0.x,
        12,
        "overlapping the left half snaps left"
    );
    assert_eq!(place(&w, 90).0.x, 78, "10 cells off stays put");
    assert_eq!(place(&w, 120).0.x, 108);
    assert!(!place(&w, 120).0.facing_left && !panel_art.mount.flippable());
    // Whatever the pointer height it lies flush with the floor, under the
    // water line like any floor plate.
    assert_eq!(ghost.y, 87);
    assert_eq!(
        w.device_placement(PANEL, 80, 2, true).0.x,
        64,
        "flip is ignored"
    );

    // Placing writes the plate's 72 cells; the gravel bed keeps every cell.
    assert_eq!(w.place(Placeable::UnderGravelPanel, 80, 10).unwrap(), 72);
    let first = *w.devices().find(|d| d.kind == PANEL).unwrap();
    assert_eq!(first.x, 64);
    assert_eq!(count(&w, "gravel"), gravel_before, "no substrate is lost");
    // A second one chains on beside the first (not overlapping it).
    let second = add_panel(&mut w, 95);
    assert_eq!(second.x, 88, "flush against the first's right edge");
    // And a third on the filter's other side.
    let third = add_panel(&mut w, 20);
    assert_eq!(third.x, 12, "flush against the filter's left edge");
    // Every device cell is the right device and nothing is lost.
    assert_eq!(
        count(&w, "device"),
        ugf.cells(w.device_kinds().get(KIND)).count() + 3 * 72
    );
    assert_eq!(count(&w, "gravel"), gravel_before);
    settle(&mut w);
    assert_eq!(count(&w, "gravel"), gravel_before);
    // The bed rests on the panels' plates like the filter's.
    for p in [first, second, third] {
        for x in p.x..p.x + 24 {
            assert_eq!(
                w.cell(x, p.y - 1).unwrap().material,
                id(&w, "gravel"),
                "gravel rests on the panel at column {x}"
            );
        }
    }

    // A panel on its own pointer can't overlap a filter it has no room beside.
    let mut crowded = world(60, 80, 32, 0.9);
    add(&mut crowded, 14, false);
    assert!(crowded.add_device(PANEL, 40, 40, false).unwrap().is_some());
    assert_eq!(
        crowded.add_device(PANEL, 40, 40, false).unwrap(),
        None,
        "no room left for another"
    );

    // Removing any device leaves every substrate cell, which settles back.
    for id in [first.id, second.id, third.id, ugf_id] {
        assert!(w.remove_device(id));
        assert_eq!(count(&w, "gravel"), gravel_before, "removal keeps the bed");
    }
    assert_eq!(count(&w, "device"), 0);
    settle(&mut w);
    assert_eq!(count(&w, "gravel"), gravel_before);
    for x in 0..160 {
        let top = top_of_bed(&w, x, "gravel");
        for y in top..90 {
            assert_eq!(
                w.cell(x, y).unwrap().material,
                id(&w, "gravel"),
                "({x}, {y})"
            );
        }
    }
}

#[test]
fn a_panel_with_no_filter_beside_it_does_nothing() {
    let mut w = wide(33, 12);
    let (ugf_id, ugf) = add(&mut w, 40, false); // 26..54
    let panel = add_panel(&mut w, 130); // far away: 118..142
    assert!(panel.x > ugf.x + 28 + 4);
    run(&mut w, 40);
    let status = w.device_status(panel.id).unwrap();
    assert!(!status.connected, "no filter is connected to it");
    assert!(!status.is_filter && status.fry_safe && status.flow == 0.0);
    assert!(w.device_status(ugf_id).unwrap().connected);

    // The filter's own down-flow is there, the panel's is not.
    let top = top_of_bed(&w, ugf.x + 16, "gravel");
    assert!(w.current_at(ugf.x + 16, top - 3)[1] > 0.0);
    for x in [panel.x + 3, panel.x + 12, panel.x + 21] {
        let top = top_of_bed(&w, x, "gravel");
        assert_eq!(w.current_at(x, top - 3), [0.0, 0.0], "no down-flow at {x}");
        assert_eq!(w.current_at(x, 85), [0.0, 0.0]);
    }

    // It captures nothing: detritus on the bed above its intakes stays put,
    // and its intakes aren't even checked.
    let detritus = id(&w, "detritus");
    let art = w.device_kinds().get(PANEL).clone();
    let (ix, _) = panel.intakes(&art).nth(2).unwrap();
    let top = top_of_bed(&w, ix, "gravel");
    w.set_cell(ix, top - 1, detritus).unwrap();
    let before = w.intake_cell_checks();
    w.tick();
    assert_eq!(w.device(ugf_id).unwrap().media_load, 0.0);
    assert_eq!(w.device(panel.id).unwrap().media_load, 0.0);
    let ugf_intakes = w.device_kinds().get(KIND).intakes.len() as u64;
    assert_eq!(w.intake_cell_checks() - before, 8 * ugf_intakes);
}

#[test]
fn a_connected_panel_extends_the_downflow_filtration_and_plant_decline_to_its_footprint() {
    let mut w = wide(34, 12);
    let (ugf_id, ugf) = add(&mut w, 40, false); // 26..54
    let direct = add_panel(&mut w, 70); // snaps to 54..78
    assert_eq!(direct.x, 54);
    let chained = add_panel(&mut w, 90); // snaps to 78..102
    assert_eq!(chained.x, 78);
    let far = add_panel(&mut w, 130); // off on its own: not connected
    assert!(far.x > 106);
    settle(&mut w);
    run(&mut w, 10);
    let art = w.device_kinds().get(PANEL).clone();
    for p in [direct, chained] {
        assert!(w.device_status(p.id).unwrap().connected, "panel at {}", p.x);
        // Down-flow through the gravel extends over the panel...
        let x = p.x + 12;
        let top = top_of_bed(&w, x, "gravel");
        let [_, above] = w.current_at(x, top - 3);
        assert!(
            above > 0.0,
            "open water over the panel is drawn down: {above}"
        );
        let [_, inside] = w.current_at(x, 85);
        assert!(
            inside > 0.0,
            "the gravel over the panel seeps down: {inside}"
        );
        // ...and debris resting on the bed above it is captured into the
        // main filter's media.
        let detritus = id(&w, "detritus");
        let (ix, _) = p.intakes(&art).nth(1).unwrap();
        let top = top_of_bed(&w, ix, "gravel");
        let load = w.device(ugf_id).unwrap().media_load;
        w.set_cell(ix, top - 1, detritus).unwrap();
        w.tick();
        assert_eq!(w.cell(ix, top - 1).unwrap().material, id(&w, "water"));
        assert!(
            w.device(ugf_id).unwrap().media_load > load,
            "the main filter's media took the panel's catch"
        );
        assert_eq!(w.device(p.id).unwrap().media_load, 0.0);
    }
    assert!(!w.device_status(far.id).unwrap().connected);
    assert_eq!(w.current_at(far.x + 12, 85), [0.0, 0.0]);
    assert_eq!(w.device(ugf_id).unwrap().x, ugf.x);
    // Panels are plain plates: no media to rinse, no maturity, no bio capacity.
    assert!(!w.rinse_device(direct.id, RinseKind::Gentle));
    assert!(w.set_device_maturity(direct.id, 1.0));
    assert!(w.filter_bio_capacity() < 0.01);
    assert_eq!(w.toggle_device_intake_guard(direct.id), Some(false));

    // The filter draws less per intake the more panels share its flow, and
    // clogging it slows the panels' down-flow too.
    let x = direct.x + 12;
    let top = top_of_bed(&w, x, "gravel");
    let clean = w.current_at(x, top - 3)[1];
    assert!(w.set_device_media_load(ugf_id, 1.0));
    run(&mut w, 5);
    let clogged = w.current_at(x, top - 3)[1];
    assert!(clogged < clean, "clogged {clogged} < clean {clean}");
}

#[test]
fn between_two_filters_a_panel_belongs_to_the_one_with_the_lower_id() {
    let mut w = wide(35, 12);
    let (a, da) = add(&mut w, 14, false); // 0..28
    let (b, db) = add(&mut w, 66, false); // 52..80
    assert_eq!((da.x, db.x), (0, 52));
    let mid = add_panel(&mut w, 40); // 28..52 touches both
    assert_eq!(mid.x, 28);
    let art = w.device_kinds().get(PANEL).clone();
    settle(&mut w);
    let (ix, _) = mid.intakes(&art).nth(2).unwrap();
    let top = top_of_bed(&w, ix, "gravel");
    w.set_cell(ix, top - 1, id(&w, "detritus")).unwrap();
    w.tick();
    assert!(
        w.device(a).unwrap().media_load > 0.0,
        "the lower id takes it"
    );
    assert_eq!(w.device(b).unwrap().media_load, 0.0);
}

#[test]
fn rooted_plants_over_a_connected_panel_decline_and_over_a_disconnected_one_stay_healthy() {
    let mut w = world(240, 100, 36, 0.95);
    let soil = id(&w, "aquasoil");
    w.fill_rect(rect(0, 88, 240, 100), soil, Brush::Replace)
        .unwrap();
    let filter = w.add_device(KIND, 40, 60, false).unwrap().expect("fits"); // 26..54
    let linked = add_panel(&mut w, 70); // 54..78
    let apart = add_panel(&mut w, 170); // 158..182, not touching anything
    assert_eq!(linked.x, 54);
    assert!(apart.x > 100);
    let off = |p: &Device| i64::try_from(p.x).unwrap() + 12;
    let names = ["rotala", "vallisneria", "dwarf_hairgrass"];
    for (k, name) in names.into_iter().enumerate() {
        let shift = i64::try_from(k).unwrap() * 3;
        plant(&mut w, name, off(&linked) + shift - 3);
        plant(&mut w, name, off(&apart) + shift - 3);
        plant(&mut w, name, 210 + shift);
    }
    run(&mut w, MINUTE);
    for name in names {
        let over = health_of(&w, name, linked.x..linked.x + 24);
        assert!(!over.is_empty(), "{name} over the connected panel is alive");
        assert!(
            over.iter().all(|&h| h < 0.85),
            "{name} over the connected panel declines: {over:?}"
        );
        for (label, range) in [
            ("disconnected panel", apart.x..apart.x + 24),
            ("open bed", 200..240),
        ] {
            let healthy = health_of(&w, name, range);
            assert!(!healthy.is_empty(), "{name} over the {label} is alive");
            assert!(
                healthy.iter().all(|&h| h >= 0.99),
                "{name} over the {label} stays healthy: {healthy:?}"
            );
        }
    }
    assert!(w.device_status(filter).unwrap().connected);
}

#[test]
fn removing_the_filter_or_a_bridging_panel_disconnects_what_it_linked() {
    // A filter with two panels chained beside it; remove the bridge.
    let mut w = world(240, 100, 37, 0.95);
    let soil = id(&w, "aquasoil");
    w.fill_rect(rect(0, 88, 240, 100), soil, Brush::Replace)
        .unwrap();
    let filter = w.add_device(KIND, 40, 60, false).unwrap().expect("fits");
    let bridge = add_panel(&mut w, 70); // 54..78
    let tail = add_panel(&mut w, 90); // 78..102
    assert_eq!((bridge.x, tail.x), (54, 78));
    plant(&mut w, "vallisneria", i64::try_from(tail.x).unwrap() + 12);
    run(&mut w, MINUTE * 3 / 4);
    assert!(
        w.device_status(tail.id).unwrap().connected,
        "chained through the bridge"
    );
    let stressed = health_of(&w, "vallisneria", tail.x..tail.x + 24);
    assert!(
        !stressed.is_empty() && stressed.iter().all(|&h| h < 0.95),
        "the plant over the chained panel is stressed: {stressed:?}"
    );
    assert!(
        w.current_at(tail.x + 12, 92)[1] > 0.0,
        "down-flow over the tail"
    );

    // (A plant's roots take a soil cell or two, so count just before.)
    let soil_before = count(&w, "aquasoil");
    assert!(w.remove_device(bridge.id));
    assert_eq!(count(&w, "aquasoil"), soil_before, "removal keeps the bed");
    assert!(!w.device_status(tail.id).unwrap().connected);
    assert!(w.device_status(filter).unwrap().connected);
    run(&mut w, 6 * MINUTE);
    assert_eq!(
        w.current_at(tail.x + 12, 92),
        [0.0, 0.0],
        "down-flow stopped"
    );
    let recovered = health_of(&w, "vallisneria", 0..240);
    assert!(!recovered.is_empty(), "the plant survived");
    assert!(
        recovered.iter().all(|&h| h >= 0.99),
        "the plant recovers once its panel is cut off: {recovered:?}"
    );

    // Removing the filter itself disconnects every panel.
    let (mut w, filter) = {
        let mut w = wide(38, 12);
        let (f, _) = add(&mut w, 40, false);
        (w, f)
    };
    let p = add_panel(&mut w, 70);
    assert!(w.device_status(p.id).unwrap().connected);
    run(&mut w, 10);
    let top = top_of_bed(&w, p.x + 12, "gravel");
    assert!(w.current_at(p.x + 12, top - 3)[1] > 0.0);
    let gravel = count(&w, "gravel");
    assert!(w.remove_device(filter));
    assert!(!w.device_status(p.id).unwrap().connected);
    run(&mut w, 10);
    assert_eq!(w.current_at(p.x + 12, top - 3), [0.0, 0.0]);
    assert_eq!(count(&w, "gravel"), gravel, "substrate stays intact");
}

#[test]
fn settled_connected_panels_stay_o1_and_disconnected_ones_cost_nothing() {
    // The water line is off an 8-cell field-block boundary (see the settled
    // filter test above).
    let mut w = world(160, 90, 39, 0.88);
    assert_ne!(w.surface_row() % 8, 0);
    let gravel = id(&w, "gravel");
    w.fill_rect(rect(0, 78, 160, 90), gravel, Brush::Replace)
        .unwrap();
    add(&mut w, 40, false);
    add_panel(&mut w, 70);
    add_panel(&mut w, 90);
    let far = add_panel(&mut w, 150);
    assert!(!w.device_status(far.id).unwrap().connected);
    settle(&mut w);
    let rebuilds = w.device_rebuilds();
    let scans = w.current_block_scans();
    let checks = w.intake_cell_checks();
    let per_tick = (w.device_kinds().get(KIND).intakes.len()
        + 2 * w.device_kinds().get(PANEL).intakes.len()) as u64;
    run(&mut w, 400);
    assert_eq!(
        w.device_rebuilds(),
        rebuilds,
        "no current rebuilds while idle"
    );
    assert_eq!(
        w.current_block_scans(),
        scans,
        "the settled bed scans nothing"
    );
    assert_eq!(
        w.intake_cell_checks() - checks,
        400 * 8 * per_tick,
        "connected panels' floor intakes are cached and O(1) per tick; \
         a disconnected panel's aren't checked at all"
    );
}

#[test]
fn panels_save_and_load_and_replay_identically() {
    let mut w = wide(40, 12);
    let (f, _) = add(&mut w, 40, true);
    let p1 = add_panel(&mut w, 18);
    let p2 = add_panel(&mut w, 120);
    assert!(w.set_device_media_load(f, 0.4));
    run(&mut w, 40);
    let bytes = save::encode(&w);
    let mut loaded = save::decode(&bytes, MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(w.state_hash(), loaded.state_hash());
    for id in [f, p1.id, p2.id] {
        assert_eq!(loaded.device(id), w.device(id));
        assert_eq!(loaded.device_status(id), w.device_status(id));
    }
    assert_eq!(
        loaded.current(),
        w.current(),
        "the current is rebuilt identically"
    );
    for _ in 0..160 {
        w.tick();
        loaded.tick();
    }
    assert_eq!(w.state_hash(), loaded.state_hash());
}
