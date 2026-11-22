//! Hang-on-back (HOB) filter integration tests (D27, DV5, #161):
//! rim mount partly above the water line, left/right flip, whole-device
//! removal restoring air above and water below the water line, back-band
//! intake tube (fish pass in front of the tube and dodge past it, while
//! the solid body blocks every depth band), downward waterfall circulation
//! and surface-ripple reporting, depth-band-aware intake suction & guard,
//! zero idle cost, and save/load determinism.

use desktank_sim::{
    Brush, CLOG_FLOW_FLOOR, DEPTH_BANDS, DeviceKind, MaterialTable, Mount, Placeable, Rect,
    RinseKind, Traits, World, WorldConfig, save,
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
fn rim_mount_hangs_over_top_edge_flips_restores_air_and_water_and_refuses_when_blocked_or_dry() {
    let mut w = world(140, 90, 1, 0.88); // surface_row = 11
    let art = w.device_kinds().get(DeviceKind::HobFilter).clone();
    let surface = w.surface_row();
    assert_eq!(art.mount, Mount::Rim);
    assert!(surface > 0 && surface < art.size.1);

    // Clicking anywhere vertically hangs the HOB on the top rim (y = 0),
    // horizontally centred on x, partly above the water line.
    let (ghost_a, ok_a) = w.device_placement(DeviceKind::HobFilter, 40, 60, false);
    assert!(ok_a);
    assert_eq!(ghost_a.y, 0);
    assert_eq!(ghost_a.x, 40 - art.size.0 / 2);
    assert!(!ghost_a.facing_left);

    let (ghost_b, ok_b) = w.device_placement(DeviceKind::HobFilter, 95, 10, true);
    assert!(ok_b);
    assert_eq!(ghost_b.y, 0);
    assert!(ghost_b.facing_left);
    assert_ne!(
        ghost_a
            .spillway_world(&art)
            .map(|(x0, _, _)| x0 - ghost_a.x),
        ghost_b
            .spillway_world(&art)
            .map(|(x0, _, _)| x0 - ghost_b.x),
        "flipping mirrors the spillway and intake tube horizontally"
    );

    let id = w
        .add_device(DeviceKind::HobFilter, 40, 50, false)
        .unwrap()
        .expect("HOB fits on the rim");
    let d = *w.device(id).unwrap();
    let device_mat = w.materials().id("device").unwrap();
    let air = w.materials().id("air").unwrap();
    let water = w.materials().id("water").unwrap();

    // Footprint covers both air rows (above surface_row) and water rows (below).
    let above_count = d.cells(&art).filter(|&(_, cy)| cy < surface).count();
    let below_count = d.cells(&art).filter(|&(_, cy)| cy >= surface).count();
    assert!(above_count > 0, "HOB hood must extend above the water line");
    assert!(below_count > 0, "HOB tube must extend into the water");

    // Overlapping placement is refused.
    assert_eq!(
        w.add_device(DeviceKind::HobFilter, 44, 0, false).unwrap(),
        None,
        "second HOB cannot overlap the first"
    );

    // Blocked by hardscape near the rim is refused.
    let rock = w.materials().id("rock").unwrap();
    w.fill_rect(
        Rect {
            x0: 100,
            y0: 20,
            x1: 112,
            y1: 28,
        },
        rock,
        Brush::Replace,
    )
    .unwrap();
    let (_, fits_on_rock) = w.device_placement(DeviceKind::HobFilter, 100, 0, false);
    assert!(!fits_on_rock);
    assert_eq!(
        w.add_device(DeviceKind::HobFilter, 100, 0, false).unwrap(),
        None
    );

    // Overwriting a single cell removes the whole HOB, restoring air above
    // surface_row and water at/below surface_row.
    let (sample_x, sample_y) = d.cells(&art).next().unwrap();
    let cells: Vec<_> = d.cells(&art).collect();
    w.set_cell(sample_x, sample_y, water).unwrap();
    assert!(w.device(id).is_none());
    assert_eq!(
        w.cells()
            .iter()
            .filter(|c| c.material == device_mat)
            .count(),
        0,
        "no stray device cells remain after whole removal"
    );
    for (cx, cy) in cells {
        if (cx, cy) == (sample_x, sample_y) {
            continue;
        }
        let expected = if cy < surface { air } else { water };
        assert_eq!(
            w.cell(cx, cy).unwrap().material,
            expected,
            "removed HOB cell ({cx}, {cy}) should become {expected:?}"
        );
    }

    // In a nearly drained tank where the intake strainer would hang in air,
    // placement is refused.
    let mut dry = world(100, 80, 2, 0.45); // surface_row = 44 > strainer row 29
    let (_, dry_ok) = dry.device_placement(DeviceKind::HobFilter, 50, 0, false);
    assert!(!dry_ok, "HOB refuses placement when intake hangs in air");
    assert_eq!(dry.place(Placeable::HobFilter, 50, 0).unwrap(), 0);
}

#[test]
fn fish_pass_in_front_of_intake_tube_but_cannot_pass_through_body() {
    // High water level (0.95 -> surface_row = 5) so the lower HOB body rows
    // (rows 5..10) sit submerged in water and can be tested against swimming fish.
    let mut w = world(120, 96, 3, 0.95);
    let art = w.device_kinds().get(DeviceKind::HobFilter).clone();
    let back_row = art.back_band_from.expect("HOB has back-band intake tube");
    let dev = w
        .add_device(DeviceKind::HobFilter, 60, 0, false)
        .unwrap()
        .expect("HOB fits");
    let d = *w.device(dev).unwrap();
    let tube_x = i64::try_from(d.x + 17).unwrap();
    let tube_y = i64::try_from(d.y + back_row + 8).unwrap();
    assert_eq!(
        w.device_at(
            usize::try_from(tube_x).unwrap(),
            usize::try_from(tube_y).unwrap()
        ),
        Some(dev),
        "probe point is inside the HOB intake tube"
    );

    // Find a submerged cell inside the solid HOB body (rows < back_row, >= surface_row).
    let body_y = i64::try_from(w.surface_row() + 2).unwrap();
    let body_x = i64::try_from(d.x + 6).unwrap();
    assert!(usize::try_from(body_y).unwrap() < d.y + back_row);
    assert_eq!(
        w.device_at(
            usize::try_from(body_x).unwrap(),
            usize::try_from(body_y).unwrap()
        ),
        Some(dev),
        "probe point is inside the submerged HOB body"
    );

    let tetra = w.species().id("neon_tetra").unwrap();
    // Spawn a fish and move it right across the intake tube in a front band.
    // Try several spawns until we have one in front bands (0..=4).
    let mut front_fish = None;
    for k in 0..24 {
        let id = w.add_creature(tetra, 20 + k * 2, 40).expect("tetra spawns");
        if w.creature(id).unwrap().band() <= 4 {
            front_fish = Some(id);
            break;
        }
    }
    let fish = front_fish.expect("found a tetra in a front depth band");
    assert!(
        w.move_creature(fish, tube_x, tube_y),
        "fish in front depth band {} must pass directly in front of the HOB intake tube",
        w.creature(fish).unwrap().band()
    );
    assert_eq!(w.creature(fish).unwrap().cell(), (tube_x, tube_y));

    // No fish in ANY depth band (0..DEPTH_BANDS) may overlap the solid HOB body.
    for x_off in [2usize, 6, 10, 16] {
        let bx = i64::try_from(d.x + x_off).unwrap();
        assert!(
            !w.move_creature(fish, bx, body_y),
            "fish must never pass through the solid HOB body at ({bx}, {body_y})"
        );
    }

    // A fish in the back depth band (DEPTH_BANDS - 1) is blocked by the back-band
    // intake tube at the tube coordinates, while a front-band fish fits.
    let mut back_fish = None;
    for k in 0..48 {
        let id = w
            .add_creature(tetra, 15 + (k % 20) * 3, 55 + (k / 20) * 4)
            .expect("tetra spawns");
        if w.creature(id).unwrap().band() == DEPTH_BANDS - 1 {
            back_fish = Some(id);
            break;
        }
    }
    let back = back_fish.expect("found a tetra in the back depth band");
    assert!(
        !w.move_creature(back, tube_x, tube_y),
        "back-band fish cannot overlap the back-band intake tube"
    );
}

#[test]
#[allow(clippy::cast_precision_loss)] // small grid coordinates in test
fn waterfall_drives_downward_circulation_above_surface_ripples_and_costs_zero_when_idle() {
    let mut w = world(140, 96, 5, 0.86); // surface_row = 13 > spillway y = 9
    let art = w.device_kinds().get(DeviceKind::HobFilter).clone();
    let surface = w.surface_row();
    assert!(surface > art.nozzle.1, "spillway sits in air above water");

    let dev = w
        .add_device(DeviceKind::HobFilter, 70, 0, false)
        .unwrap()
        .expect("HOB fits");
    for _ in 0..10 {
        w.tick();
    }

    let d = *w.device(dev).unwrap();
    let (sx0, sx1, sy) = d.spillway_world(&art).expect("HOB has spillway");
    let mid_x = usize::midpoint(sx0, sx1);

    // Waterfall injects strong downward current into the water column directly
    // beneath the spillway even though the spillway lip itself is in air.
    let [_, vy] = w.current_at(mid_x, surface + 6);
    assert!(
        vy > 1.0,
        "expected downward waterfall current under spillway, got vy={vy}"
    );
    let status = w.device_status(dev).unwrap();
    assert!(
        status.flow > 1.0,
        "expected positive HOB flow, got {}",
        status.flow
    );

    // Surface ripple zone tracks the spillway and scales down with clog.
    let ripples = w.surface_ripples();
    assert_eq!(ripples.len(), 1);
    assert!((ripples[0][0] - sx0 as f32).abs() < 1e-4);
    assert!((ripples[0][1] - sx1 as f32).abs() < 1e-4);
    assert!((ripples[0][2] - sy as f32).abs() < 1e-4);
    assert!((ripples[0][3] - 1.0).abs() < 1e-4);

    assert!(w.set_device_media_load(dev, 1.0));
    let clogged_ripples = w.surface_ripples();
    assert!(
        (clogged_ripples[0][3] - CLOG_FLOW_FLOOR).abs() < 0.02,
        "clogged ripple strength {} should approach floor {CLOG_FLOW_FLOOR}",
        clogged_ripples[0][3]
    );
    assert!(w.rinse_device(dev, RinseKind::Gentle));
    assert!((w.surface_ripples()[0][3] - 1.0).abs() < 1e-4);

    // Let tank settle completely, then verify zero idle simulation cost.
    for _ in 0..200 {
        w.tick();
    }
    let (rebuilds, changed, scans, hash) = (
        w.device_rebuilds(),
        w.last_change_tick(),
        w.current_block_scans(),
        w.state_hash(),
    );
    for _ in 0..200 {
        w.tick();
    }
    assert_eq!(
        w.device_rebuilds(),
        rebuilds,
        "idle HOB rebuilds no current"
    );
    assert_eq!(w.last_change_tick(), changed, "idle HOB mutates no cells");
    assert_eq!(
        w.current_block_scans(),
        scans,
        "settled HOB tank scans zero current blocks"
    );
    assert_eq!(w.awake_chunks(), 0, "idle HOB keeps all chunks asleep");
    assert_ne!(w.state_hash(), hash, "tick still advances");
}

#[test]
fn hob_captures_particles_and_intake_guard_protects_back_band_fry() {
    let mut w = world(120, 80, 9, 0.90);
    let art = w.device_kinds().get(DeviceKind::HobFilter).clone();
    let dev = w
        .add_device(DeviceKind::HobFilter, 60, 0, false)
        .unwrap()
        .expect("HOB fits");
    let d = *w.device(dev).unwrap();
    let (ix, iy) = d.intakes(&art).next().unwrap();

    // Place detritus right next to the left-flank strainer intake.
    let detritus = w.materials().id("detritus").unwrap();
    w.set_cell(ix - 1, iy, detritus).unwrap();
    w.tick();
    assert!(
        w.device(dev).unwrap().media_load > 0.0,
        "HOB intake should capture adjacent detritus into media load"
    );

    // Spawn fry near the strainer: a front-band fry passing in front of the
    // back-band strainer is NOT sucked in, whereas a back-band fry IS sucked
    // in when unguarded and survives when the intake guard is on.
    let guppy = w.species().id("guppy").unwrap();
    let fry_traits = Traits {
        age: Some(0),
        ..Traits::default()
    };
    let mut front_fry = None;
    let mut back_fry = None;
    for k in 0..64 {
        let id = w
            .add_creature_with_traits(guppy, 16 + (k % 16) * 2, 40 + (k / 16) * 4, fry_traits)
            .expect("fry spawns");
        let band = w.creature(id).unwrap().band();
        if band <= 3 && front_fry.is_none() {
            front_fry = Some(id);
        } else if band == DEPTH_BANDS - 1 && back_fry.is_none() {
            back_fry = Some(id);
        }
        if front_fry.is_some() && back_fry.is_some() {
            break;
        }
    }
    let front_fry = front_fry.expect("spawned a front-band fry");
    let back_fry = back_fry.expect("spawned a back-band fry");
    let near_x = i64::try_from(ix - 2).unwrap();
    let near_y = i64::try_from(iy).unwrap();

    // Move front fry right across the strainer's x/y: stays safe in front band.
    assert!(w.move_creature(front_fry, i64::try_from(ix).unwrap(), near_y));
    w.release_creature(front_fry);
    // Move back fry right next to the unguarded strainer: sucked in on tick.
    assert!(w.move_creature(back_fry, near_x, near_y));
    w.release_creature(back_fry);
    w.tick();
    assert!(
        w.creature(front_fry).is_some(),
        "front-band fry passing in front of the back-band strainer must not be sucked in"
    );
    assert!(
        w.creature(back_fry).is_none(),
        "unguarded back-band strainer must suck in adjacent back-band fry"
    );

    // With intake guard fitted, a back-band fry adjacent to the strainer survives.
    assert!(w.set_device_intake_guard(dev, true));
    let mut guarded_fry = None;
    for k in 0..64 {
        let id = w
            .add_creature_with_traits(guppy, 80 + (k % 12) * 2, 40 + (k / 12) * 4, fry_traits)
            .expect("fry spawns");
        if w.creature(id).unwrap().band() == DEPTH_BANDS - 1 {
            guarded_fry = Some(id);
            break;
        }
    }
    let guarded_fry = guarded_fry.expect("spawned a guarded back-band fry");
    assert!(w.move_creature(guarded_fry, near_x, near_y));
    w.release_creature(guarded_fry);
    w.tick();
    assert!(
        w.creature(guarded_fry).is_some(),
        "intake guard must protect back-band fry at the HOB strainer"
    );
}

#[test]
fn hob_filter_save_load_round_trip_replays_identically() {
    let mut w = world(120, 80, 11, 0.88);
    let dev = w
        .add_device(DeviceKind::HobFilter, 55, 0, true)
        .unwrap()
        .expect("HOB fits");
    assert!(w.set_device_media_load(dev, 0.35));
    assert!(w.set_device_maturity(dev, 0.7));
    assert!(w.set_device_intake_guard(dev, true));
    let tetra = w.species().id("neon_tetra").unwrap();
    w.add_creature(tetra, 40, 35).unwrap();
    for _ in 0..40 {
        w.tick();
    }

    let bytes = save::encode(&w);
    let mut loaded = save::decode(&bytes, MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(w.state_hash(), loaded.state_hash());
    let ld = *loaded.device(dev).unwrap();
    assert_eq!(ld.kind, DeviceKind::HobFilter);
    assert!(ld.facing_left);
    assert!(ld.intake_guard);
    assert!((ld.media_load - 0.35).abs() < 1e-6);
    assert!((ld.maturity - w.device(dev).unwrap().maturity).abs() < 1e-6);

    for _ in 0..160 {
        w.tick();
        loaded.tick();
    }
    assert_eq!(
        w.state_hash(),
        loaded.state_hash(),
        "loaded HOB tank must replay bit-for-bit identically"
    );
}
