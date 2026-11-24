//! Under-gravel filter (UGF) integration tests (D27, DV6, #162): floor mount
//! under existing substrate (every displaced cell kept, the bed resting on
//! the plate), left/right flip, refusal when blocked or too shallow,
//! substrate-preserving removal, down-flow through the gravel and the spray
//! bar's jet, rooted plants declining over the plate while floaters, ball
//! plants and rooted plants beside it stay healthy, filtration at the top of
//! the bed, clogging and rinsing, fry safety, zero idle cost and save/load
//! determinism.

use std::collections::BTreeMap;

use desktank_sim::{
    Brush, CLOG_FLOW_FLOOR, Device, DeviceId, DeviceKind, MaterialId, MaterialTable, Mount,
    Placeable, Propagule, Rect, RinseKind, Traits, World, WorldConfig, save,
};

const KIND: DeviceKind = DeviceKind::UnderGravelFilter;
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

    // Whatever the pointer height, the plate lies flush with the floor,
    // centred on x and clamped into the tank.
    let (ghost, ok) = w.device_placement(KIND, 60, 10, false);
    assert!(ok);
    assert_eq!((ghost.x, ghost.y), (60 - 14, 80 - 28));
    assert!(!ghost.facing_left);
    assert_eq!(w.device_placement(KIND, -30, 70, false).0.x, 0);
    assert_eq!(w.device_placement(KIND, 500, 70, false).0.x, 120 - 28);
    let (flipped, ok) = w.device_placement(KIND, 60, 10, true);
    assert!(ok && flipped.facing_left);

    // Placing writes the whole footprint as device cells.
    assert_eq!(
        w.place(Placeable::UnderGravelFilter, 60, 10).unwrap(),
        art.cells
    );
    let device = id(&w, "device");
    assert_eq!(count(&w, "device"), art.cells);
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

    // A second filter can't overlap the first.
    let mut w = bedded(2, "gravel", 0);
    add(&mut w, 40, false);
    assert_eq!(w.add_device(KIND, 50, 40, false).unwrap(), None);
    assert!(w.add_device(KIND, 90, 40, false).unwrap().is_some());

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
    // A bed so deep that the spray bar would be buried.
    let mut deep = bedded(5, "gravel", 27);
    let (_, ok) = deep.device_placement(KIND, 60, 40, false);
    assert!(!ok, "the spray bar can't be buried in the bed");
    assert_eq!(deep.place(Placeable::UnderGravelFilter, 60, 40).unwrap(), 0);
    assert_eq!(count(&deep, "gravel"), 120 * 27, "a refusal moves nothing");

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
        assert_eq!(count(&w, "device"), art.cells);
        // Nothing was lost to the plate and nothing lies inside it.
        for (x, y) in d.cells(&art) {
            assert_eq!(w.device_at(x, y), Some(dev));
        }
        settle(&mut w);
        assert_eq!(count(&w, "gravel"), gravel_before);

        // The plate is the floor of the bed: gravel rests right on its top
        // row in every column the plate covers (apart from the tube's), with
        // the bed above it unbroken.
        let plate_top = d.y + 25;
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

        // Over a bare floor the gravel poured afterwards rests on the plate.
        let mut bare = bedded(8, "gravel", 0);
        let (_, bd) = add(&mut bare, 60, flip);
        let gravel = id(&bare, "gravel");
        bare.fill_rect(rect(bd.x + 8, 40, bd.x + 28, 44), gravel, Brush::Replace)
            .unwrap();
        settle(&mut bare);
        assert_eq!(top_of_bed(&bare, bd.x + 14, "gravel"), bd.y + 25 - 4);
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
            thin.cell(x, td.y + 24).unwrap().material,
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
        let [_, inside] = w.current_at(x, d.y + 22);
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
    assert!(top < d.y + 25, "intakes lie under a bed");
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
