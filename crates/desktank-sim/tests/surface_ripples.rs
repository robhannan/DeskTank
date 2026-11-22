//! Tests for current-driven water-surface ripples and agitation (#165).

use desktank_sim::{Brush, DeviceKind, MaterialTable, Rect, RinseKind, World, WorldConfig};

fn world(width: usize, height: usize, seed: u64, water_level: f32) -> World {
    World::new(
        WorldConfig {
            width,
            height,
            seed,
            water_level,
        },
        MaterialTable::builtin().expect("built-in materials"),
    )
    .expect("valid world")
}

#[test]
fn high_internal_filter_agitates_surface_with_zero_idle_chunk_wakes() {
    // Internal filter mounted high on the left glass drives a strong
    // horizontal jet along the surface and up the far wall.
    let mut w = world(160, 96, 11, 0.85); // surface_row = 14
    let surface = w.surface_row();
    assert!(!w.has_surface_current_ripples());
    assert!(!w.has_surface_ripples());
    assert!(w.surface_agitation().iter().all(|&a| a == 0.0));

    let dev = w
        .add_device(
            DeviceKind::InternalFilter,
            0,
            i64::try_from(surface).unwrap(),
            false,
        )
        .unwrap()
        .expect("internal filter fits near surface");
    for _ in 0..8 {
        w.tick();
    }

    assert!(
        w.has_surface_current_ripples(),
        "high internal filter should trigger surface-current ripples"
    );
    assert!(w.has_surface_ripples());
    let peak = w
        .surface_agitation()
        .iter()
        .copied()
        .fold(0.0_f32, f32::max);
    assert!(
        peak > 0.1,
        "expected strong surface agitation near high jet, got peak={peak}"
    );
    assert!(
        w.surface_agitation_at(32) > 0.0,
        "surface directly ahead of high nozzle should be agitated"
    );

    // Clogging softens surface agitation; rinsing restores it.
    let clean_sum: f32 = w.surface_agitation().iter().sum();
    assert!(w.set_device_media_load(dev, 1.0));
    let clogged_sum: f32 = w.surface_agitation().iter().sum();
    assert!(
        clogged_sum < clean_sum,
        "clogged filter should reduce surface agitation ({clogged_sum} vs {clean_sum})"
    );
    assert!(w.rinse_device(dev, RinseKind::Gentle));
    let rinsed_sum: f32 = w.surface_agitation().iter().sum();
    assert!(
        (rinsed_sum - clean_sum).abs() < 1e-5,
        "rinsed agitation {rinsed_sum} should match clean {clean_sum}"
    );

    // Let the tank settle and verify zero per-tick chunk wake / rebuild cost.
    for _ in 0..120 {
        w.tick();
    }
    assert_eq!(w.awake_chunks(), 0, "settled tank wakes no chunks");
    let (rebuilds, changed, scans, hash) = (
        w.device_rebuilds(),
        w.last_change_tick(),
        w.current_block_scans(),
        w.state_hash(),
    );
    for _ in 0..120 {
        w.tick();
    }
    assert_eq!(w.awake_chunks(), 0);
    assert_eq!(w.device_rebuilds(), rebuilds);
    assert_eq!(w.last_change_tick(), changed);
    assert_eq!(w.current_block_scans(), scans);
    assert_ne!(w.state_hash(), hash); // tick count advances deterministically

    // Removing the filter calms the surface completely.
    assert!(w.remove_device(dev));
    for _ in 0..8 {
        w.tick();
    }
    assert!(!w.has_surface_current_ripples());
    assert!(!w.has_surface_ripples());
    assert!(w.surface_agitation().iter().all(|&a| a == 0.0));
}

#[test]
fn rock_deflected_jet_agitates_surface() {
    // Internal filter mounted below the surface shooting into a rock baffle
    // that deflects the jet upward onto the water surface.
    let mut deflected = world(112, 80, 19, 0.85); // surface_row = 12
    let rock = deflected.materials().id("rock").unwrap();
    deflected
        .fill_rect(
            Rect {
                x0: 48,
                y0: 36,
                x1: 64,
                y1: 80,
            },
            rock,
            Brush::Replace,
        )
        .unwrap();
    for _ in 0..10 {
        deflected.tick();
    }
    deflected
        .add_device(DeviceKind::InternalFilter, 0, 28, false)
        .unwrap()
        .expect("internal filter fits");
    for _ in 0..8 {
        deflected.tick();
    }
    assert!(
        deflected.has_surface_current_ripples(),
        "jet deflected upward by rock wall should agitate the water surface"
    );
    assert!(
        deflected.surface_agitation_at(40) > 0.0 || deflected.surface_agitation_at(48) > 0.0,
        "surface above the rock deflection zone should be agitated"
    );
}

#[test]
fn weak_surface_current_and_brim_full_tank_trigger_no_ripples_and_wake_no_chunks() {
    // Deep internal filter in a tall tank keeps surface current below threshold.
    let mut deep = world(192, 160, 23, 0.90); // surface_row = 16
    deep.add_device(DeviceKind::InternalFilter, 0, 128, false)
        .unwrap()
        .expect("deep internal filter fits");
    for _ in 0..120 {
        deep.tick();
    }
    let thresh = deep.ecology().current.surface_ripple_speed;
    let surface = deep.surface_row();
    let max_surface_speed = (0..deep.width())
        .step_by(8)
        .map(|x| {
            let [vx, vy] = deep.current_at(x, surface);
            (vx * vx + vy * vy).sqrt()
        })
        .fold(0.0_f32, f32::max);
    assert!(
        max_surface_speed <= thresh,
        "expected weak surface speed <= {thresh}, got {max_surface_speed}"
    );
    assert!(
        !deep.has_surface_current_ripples(),
        "weak surface current must not activate ripples"
    );
    assert!(!deep.has_surface_ripples());
    assert!(deep.surface_agitation().iter().all(|&a| a == 0.0));
    assert_eq!(
        deep.awake_chunks(),
        0,
        "settled weak-current tank wakes no chunks"
    );

    let (rebuilds, changed, scans) = (
        deep.device_rebuilds(),
        deep.last_change_tick(),
        deep.current_block_scans(),
    );
    for _ in 0..120 {
        deep.tick();
    }
    assert_eq!(deep.awake_chunks(), 0);
    assert_eq!(deep.device_rebuilds(), rebuilds);
    assert_eq!(deep.last_change_tick(), changed);
    assert_eq!(deep.current_block_scans(), scans);

    // Brim-full tank (water_level = 1.0 -> surface_row = 0) has no air-water
    // surface and must never report surface ripples.
    let mut full = world(160, 96, 29, 1.0);
    assert_eq!(full.surface_row(), 0);
    full.add_device(DeviceKind::InternalFilter, 0, 0, false)
        .unwrap()
        .expect("internal filter fits in brim-full tank");
    for _ in 0..8 {
        full.tick();
    }
    assert!(!full.has_surface_current_ripples());
    assert!(!full.has_surface_ripples());
    assert!(full.surface_agitation().iter().all(|&a| a == 0.0));
}

#[test]
fn hob_and_surface_current_agitation_coexist_cleanly_and_replay_deterministically() {
    let setup = || {
        let mut w = world(144, 80, 41, 0.84);
        w.add_device(DeviceKind::HobFilter, 40, 0, false)
            .unwrap()
            .expect("HOB fits");
        for _ in 0..16 {
            w.tick();
        }
        w
    };
    let a = setup();
    let b = setup();
    assert_eq!(a.surface_agitation(), b.surface_agitation());
    assert_eq!(a.surface_ripples(), b.surface_ripples());
    assert!(a.has_surface_ripples());
    assert_eq!(
        a.surface_agitation_at(a.width() - 8),
        0.0,
        "far surface away from HOB should stay calm"
    );
}
