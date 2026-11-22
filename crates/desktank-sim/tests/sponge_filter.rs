//! DV4 acceptance tests (`docs/spec/devices.md` §2.8, D27, #160):
//! - Bed mount: drops straight down onto substrate, hardscape or floor; lifts
//!   onto the bed when clicked on/in it; whole-or-nothing placement; symmetric
//!   (ignores wall flip); ghost preview matches placement.
//! - Whole-device removal: scooping or overwriting any footprint cell clears
//!   the whole sponge filter cleanly.
//! - Air-driven uplift: spawns bubbles from the top of the uplift tube
//!   (conserving water level, scaling with media clog) and drives gentle upward
//!   flow in the 2D current field.
//! - Filtration, clogging, maturity and Rinse work like any prefab filter.
//! - Fry-safety: intrinsically safe for fry and small shrimp without a guard.
//! - Shrimp grazing: cherry and amano shrimp seek out and graze the sponge
//!   filter deterministically without erasing device cells.
//! - Save/load round-trip preserves the sponge filter and replays identically.

use desktank_sim::{
    Brush, CLOG_FLOW_FLOOR, DeviceKind, MaterialTable, Mount, Placeable, Rect, RinseKind,
    TICKS_PER_SECOND, World, WorldConfig, save,
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

#[test]
fn bed_mount_drops_onto_floor_substrate_and_hardscape_and_refuses_when_blocked() {
    let mut w = world(120, 80, 1, 0.9);
    let art = w.device_kinds().get(DeviceKind::SpongeFilter).clone();
    assert_eq!(art.mount, Mount::Bed);
    let (sw, sh) = art.size;

    // 1. Dropped in open water: lands on the tank floor, centred on x, ignoring flip.
    let (ghost_unflipped, ok1) = w.device_placement(DeviceKind::SpongeFilter, 30, 20, false);
    let (ghost_flipped, ok2) = w.device_placement(DeviceKind::SpongeFilter, 30, 20, true);
    assert!(ok1 && ok2);
    assert_eq!(ghost_unflipped, ghost_flipped, "bed mount ignores flip");
    assert!(!ghost_unflipped.facing_left);
    assert_eq!(ghost_unflipped.x, 30 - sw / 2);
    assert_eq!(ghost_unflipped.y, 80 - sh);

    let id1 = w
        .add_device(DeviceKind::SpongeFilter, 30, 20, true)
        .unwrap()
        .unwrap();
    let d1 = *w.device(id1).unwrap();
    assert_eq!((d1.x, d1.y, d1.facing_left), (30 - sw / 2, 80 - sh, false));

    // 2. Over a sand bed: drops onto the sand surface, and clicking inside the
    //    sand bed lifts cleanly onto the bed surface too.
    let sand = w.materials().id("sand").unwrap();
    w.fill_rect(
        Rect {
            x0: 55,
            y0: 66,
            x1: 95,
            y1: 80,
        },
        sand,
        Brush::Replace,
    )
    .unwrap();
    let (from_above, ok_above) = w.device_placement(DeviceKind::SpongeFilter, 75, 25, false);
    let (from_inside, ok_inside) = w.device_placement(DeviceKind::SpongeFilter, 75, 72, false);
    assert!(ok_above && ok_inside);
    assert_eq!(from_above.y, 66 - sh, "rests cleanly on top of the sand");
    assert_eq!(
        from_inside.y,
        66 - sh,
        "clicking inside the bed lifts onto it"
    );
    let id2 = w
        .add_device(DeviceKind::SpongeFilter, 75, 72, false)
        .unwrap()
        .unwrap();
    assert_eq!(w.device(id2).unwrap().y, 66 - sh);

    // 3. Whole-or-nothing refusal when overlapping an existing device or rock column.
    assert!(
        w.add_device(DeviceKind::SpongeFilter, 75, 60, false)
            .unwrap()
            .is_none(),
        "overlapping sponge filter must be refused"
    );
    let rock = w.materials().id("rock").unwrap();
    w.fill_rect(
        Rect {
            x0: 96,
            y0: 0,
            x1: 120,
            y1: 80,
        },
        rock,
        Brush::Replace,
    )
    .unwrap();
    let (_, fits_in_rock) = w.device_placement(DeviceKind::SpongeFilter, 108, 40, false);
    assert!(!fits_in_rock);
    assert!(
        w.add_device(DeviceKind::SpongeFilter, 108, 40, false)
            .unwrap()
            .is_none()
    );

    // 4. Too-shallow water refuses cleanly (must sit wholly under the water line).
    let shallow = world(64, 24, 1, 1.0);
    assert!(
        !shallow
            .device_placement(DeviceKind::SpongeFilter, 32, 12, false)
            .1
    );

    // 5. Clicking shallow open water above a rock shelf refuses rather than
    //    jumping downward through the shelf into water beneath it.
    let mut shelf = world(64, 80, 1, 0.625); // surface_row = 30
    let rock = shelf.materials().id("rock").unwrap();
    shelf
        .fill_rect(
            Rect {
                x0: 10,
                y0: 40,
                x1: 54,
                y1: 49,
            },
            rock,
            Brush::Replace,
        )
        .unwrap();
    assert!(
        !shelf
            .device_placement(DeviceKind::SpongeFilter, 32, 35, false)
            .1,
        "must not jump through a rock shelf to reach water below"
    );
    // Clicking in open water beneath the shelf still drops onto the floor.
    let (under, ok_under) = shelf.device_placement(DeviceKind::SpongeFilter, 32, 55, false);
    assert!(ok_under && under.y == 80 - sh);
}

#[test]
fn overwriting_or_scooping_any_cell_removes_the_whole_sponge_filter() {
    let mut w = world(80, 64, 2, 1.0);
    let water = w.materials().id("water").unwrap();
    let device_mat = w.materials().id("device").unwrap();
    let changed = w.place(Placeable::SpongeFilter, 40, 20).unwrap();
    let art = w.device_kinds().get(DeviceKind::SpongeFilter).clone();
    assert_eq!(changed, art.cells);
    assert_eq!(w.devices().count(), 1);
    let d = *w.devices().next().unwrap();

    // Overwrite just the top uplift tube tip with water: whole device disappears.
    let (nx, ny) = d.nozzle(&art);
    assert_eq!(w.device_at(nx, ny), Some(d.id));
    w.set_cell(nx, ny, water).unwrap();
    assert_eq!(w.devices().count(), 0);
    assert_eq!(
        w.cells()
            .iter()
            .filter(|c| c.material == device_mat)
            .count(),
        0,
        "no leftover device cells after removal"
    );
}

#[test]
fn uplift_spawns_bubbles_drives_gentle_upward_flow_and_scales_with_clog() {
    let mut w_sponge = world(128, 72, 3, 0.9);
    let sponge = w_sponge
        .add_device(DeviceKind::SpongeFilter, 64, 30, false)
        .unwrap()
        .unwrap();
    let mut w_internal = world(128, 72, 3, 0.9);
    let internal = w_internal
        .add_device(DeviceKind::InternalFilter, 4, 36, false)
        .unwrap()
        .unwrap();

    w_sponge.tick();
    w_internal.tick();

    // Gentle upward uplift flow: positive magnitude, weaker than internal filter,
    // and directed upward (-y) right above the uplift tube mouth.
    let sponge_status = w_sponge.device_status(sponge).unwrap();
    let internal_status = w_internal.device_status(internal).unwrap();
    assert!(
        sponge_status.flow > 0.5 && sponge_status.flow < internal_status.flow,
        "sponge flow ({}) should be positive and gentler than internal filter ({})",
        sponge_status.flow,
        internal_status.flow
    );
    let art = w_sponge
        .device_kinds()
        .get(DeviceKind::SpongeFilter)
        .clone();
    let d = *w_sponge.device(sponge).unwrap();
    let (nx, ny) = d.nozzle(&art);
    let [vx, vy] = w_sponge.current_at(nx, ny.saturating_sub(4));
    assert!(
        vy < -0.5 && vy.abs() > vx.abs(),
        "uplift current above tube should point upward, got [{vx:.2}, {vy:.2}]"
    );

    // Bubbles rise out of the uplift tube mouth, and clogging reduces both
    // uplift flow and bubble rate without stopping them (stays >= ~30% floor).
    let bubble = w_sponge.materials().id("air_bubble").unwrap();
    let water = w_sponge.materials().id("water").unwrap();
    let initial_water = w_sponge
        .cells()
        .iter()
        .filter(|c| c.material == water)
        .count();
    let count_bubbles_over = |w: &mut World, ticks: u32| -> usize {
        let mut seen = 0;
        for _ in 0..ticks {
            w.tick();
            // Count freshly spawned bubbles right above the 3 tube-mouth cells.
            for (bx, by) in d.bubble_outlets(&art) {
                if w.cell(bx, by - 1).is_some_and(|c| c.material == bubble) {
                    seen += 1;
                }
            }
        }
        seen
    };
    let clean_bubbles = count_bubbles_over(&mut w_sponge, 400);
    assert!(
        clean_bubbles >= 15,
        "clean sponge filter should emit a steady stream of bubbles, saw {clean_bubbles}"
    );
    let after_water = w_sponge
        .cells()
        .iter()
        .filter(|c| c.material == water)
        .count();
    let active_bubbles = w_sponge
        .cells()
        .iter()
        .filter(|c| c.material == bubble)
        .count();
    assert!(
        initial_water.abs_diff(after_water + active_bubbles) <= 4,
        "water + active bubbles should stay conserved ({initial_water} vs {after_water} + {active_bubbles})"
    );

    // Fully clog the sponge filter: flow drops to ~30% floor and bubble rate drops too.
    assert!(w_sponge.set_device_media_load(sponge, 1.0));
    let clogged_flow = w_sponge.device_status(sponge).unwrap().flow;
    let ratio = clogged_flow / sponge_status.flow;
    assert!(
        (CLOG_FLOW_FLOOR - 0.05..=CLOG_FLOW_FLOOR + 0.05).contains(&ratio),
        "clogged uplift flow ratio {ratio:.3} should be near {CLOG_FLOW_FLOOR}"
    );
    let clogged_bubbles = count_bubbles_over(&mut w_sponge, 400);
    assert!(
        clogged_bubbles > 0 && clogged_bubbles < clean_bubbles,
        "clogged sponge should still bubble gently ({clogged_bubbles}) but less than clean ({clean_bubbles})"
    );

    // Rinse restores full uplift flow.
    assert!(w_sponge.rinse_device(sponge, RinseKind::Gentle));
    let rinsed_flow = w_sponge.device_status(sponge).unwrap().flow;
    assert!(
        (rinsed_flow - sponge_status.flow).abs() < 1e-3,
        "rinse must restore uplift flow ({rinsed_flow} vs {})",
        sponge_status.flow
    );
}

#[test]
fn sponge_filter_captures_particles_at_intakes_and_is_always_fry_and_shrimp_safe() {
    let mut w = world(128, 72, 4, 1.0);
    w.set_easy_mode(false);
    let dev = w
        .add_device(DeviceKind::SpongeFilter, 64, 30, false)
        .unwrap()
        .unwrap();
    let status = w.device_status(dev).unwrap();
    assert!(status.fry_safe);
    assert!(!status.intake_guard);
    // Toggling intake guard on an intrinsically fry-safe filter stays off.
    assert_eq!(w.toggle_device_intake_guard(dev), Some(false));
    assert!(w.set_device_intake_guard(dev, true));
    assert!(!w.device_status(dev).unwrap().intake_guard);

    let art = w.device_kinds().get(DeviceKind::SpongeFilter).clone();
    let d = *w.device(dev).unwrap();
    let (ix, iy) = d.intakes(&art).next().unwrap();
    let water = w.materials().id("water").unwrap();
    let waste = w.materials().id("waste").unwrap();

    // Open water cell directly beside the left intake captures suspended waste.
    let (wx, wy) = (ix - 1, iy);
    assert_eq!(w.cell(wx, wy).unwrap().material, water);
    w.set_cell(wx, wy, waste).unwrap();
    w.tick();
    assert_eq!(w.cell(wx, wy).unwrap().material, water);
    assert!(w.device_status(dev).unwrap().media_load > 0.0);

    // Place fry (age 0 guppy) and small shrimp (cherry shrimp) right beside
    // both left and right sponge intakes with guard off and easy mode off:
    // none are ever sucked in.
    let guppy = w.species().id("guppy").unwrap();
    let cherry = w.species().id("cherry_shrimp").unwrap();
    let right_intake = d.intakes(&art).last().unwrap();
    w.add_creature_aged(
        guppy,
        i64::try_from(ix - 2).unwrap(),
        i64::try_from(iy).unwrap(),
        0,
    )
    .unwrap();
    w.add_creature(
        cherry,
        i64::try_from(right_intake.0 + 3).unwrap(),
        i64::try_from(right_intake.1).unwrap(),
    )
    .unwrap();
    assert_eq!(w.creatures().len(), 2);
    for _ in 0..80 {
        w.tick();
    }
    assert_eq!(
        w.creatures().len(),
        2,
        "sponge filter must never suck in fry or small shrimp"
    );
}

#[test]
#[allow(clippy::cast_precision_loss)] // small grid coordinates
fn shrimp_seek_out_and_graze_the_sponge_filter_without_erasing_cells() {
    let mut w = world(128, 72, 5, 0.95);
    let sand = w.materials().id("sand").unwrap();
    w.fill_rect(
        Rect {
            x0: 0,
            y0: 66,
            x1: 128,
            y1: 72,
        },
        sand,
        Brush::Replace,
    )
    .unwrap();
    let dev = w
        .add_device(DeviceKind::SpongeFilter, 64, 30, false)
        .unwrap()
        .unwrap();
    assert!(w.set_device_media_load(dev, 0.25));
    let device_mat = w.materials().id("device").unwrap();
    let initial_cells = w
        .cells()
        .iter()
        .filter(|c| c.material == device_mat)
        .count();

    // Stock hungry cherry and amano shrimp on either side of the sponge with
    // zero loose food in the tank.
    let cherry = w.species().id("cherry_shrimp").unwrap();
    let amano = w.species().id("amano_shrimp").unwrap();
    let s1 = w.add_creature(cherry, 30, 62).unwrap();
    let s2 = w.add_creature(amano, 98, 62).unwrap();
    assert_eq!(w.creature(s1).unwrap().eaten(), 0);
    assert_eq!(w.creature(s2).unwrap().eaten(), 0);

    // Run 30 world seconds (600 ticks): both shrimp travel to the sponge and graze it.
    let mut approached = [false; 2];
    let d = *w.device(dev).unwrap();
    let art = w.device_kinds().get(DeviceKind::SpongeFilter).clone();
    let mid_x = (d.x + art.size.0 / 2) as f32;
    for _ in 0..(30 * TICKS_PER_SECOND) {
        w.tick();
        for (i, id) in [s1, s2].into_iter().enumerate() {
            let (cx, _) = w.creature(id).unwrap().centre();
            if (cx - mid_x).abs() <= (art.size.0 as f32) {
                approached[i] = true;
            }
        }
    }
    assert!(approached[0], "cherry shrimp should approach the sponge");
    assert!(approached[1], "amano shrimp should approach the sponge");
    let e1 = w.creature(s1).unwrap().eaten();
    let e2 = w.creature(s2).unwrap().eaten();
    assert!(
        e1 > 0 && e2 > 0,
        "both shrimp must graze biofilm off the sponge (cherry={e1}, amano={e2})"
    );
    assert_eq!(
        w.cells()
            .iter()
            .filter(|c| c.material == device_mat)
            .count(),
        initial_cells,
        "grazing must never erase sponge filter cells"
    );
    assert!(
        w.device_status(dev).unwrap().media_load < 0.25,
        "shrimp grazing should gently trim sponge media load"
    );
}

#[test]
fn sponge_filter_save_load_round_trip_replays_identically() {
    let mut w = world(128, 72, 6, 0.9);
    let sand = w.materials().id("sand").unwrap();
    w.fill_rect(
        Rect {
            x0: 0,
            y0: 66,
            x1: 128,
            y1: 72,
        },
        sand,
        Brush::Replace,
    )
    .unwrap();
    let dev = w
        .add_device(DeviceKind::SpongeFilter, 64, 30, false)
        .unwrap()
        .unwrap();
    assert!(w.set_device_media_load(dev, 0.35));
    assert!(w.set_device_maturity(dev, 0.6));
    let cherry = w.species().id("cherry_shrimp").unwrap();
    w.add_creature(cherry, 42, 62).unwrap();

    for _ in 0..120 {
        w.tick();
    }
    let bytes = save::encode(&w);
    let mut loaded = save::decode(&bytes, MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(w.state_hash(), loaded.state_hash());
    assert_eq!(w.device_status(dev), loaded.device_status(dev));

    for _ in 0..120 {
        w.tick();
        loaded.tick();
    }
    assert_eq!(
        w.state_hash(),
        loaded.state_hash(),
        "loaded sponge filter world must replay bit-for-bit identically"
    );
}
