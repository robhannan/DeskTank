//! Integration tests for the prefab bubbler (DV7, `docs/spec/devices.md` §2.8, D27):
//! bed-mount placement and whole-device removal, dense diffuser-top bubble
//! emission with water-level conservation, zero water-current/filtration work,
//! current drift interaction, and v12/v13 legacy `bubbler` cell migration.

use desktank_sim::save;
use desktank_sim::{
    Brush, DeviceKind, MaterialId, MaterialTable, Mount, Placeable, Rect, RinseKind, World,
    WorldConfig,
};

fn table() -> MaterialTable {
    MaterialTable::builtin().expect("builtin materials load")
}

fn world(width: usize, height: usize, seed: u64, water_level: f32) -> World {
    World::new(
        WorldConfig {
            width,
            height,
            seed,
            water_level,
        },
        table(),
    )
    .expect("world builds")
}

fn id(w: &World, name: &str) -> MaterialId {
    w.materials().id(name).expect("material exists")
}

fn count(w: &World, m: MaterialId) -> usize {
    w.material_counts()[usize::from(m.0)]
}

#[test]
fn bed_mount_drops_onto_floor_substrate_and_hardscape_and_refuses_when_blocked() {
    let mut w = world(140, 80, 1, 0.9);
    let art = w.device_kinds().get(DeviceKind::Bubbler).clone();
    assert_eq!(art.mount, Mount::Bed);
    assert!(!art.mount.flippable());

    // Open water: drops straight to the tank floor, ignoring flip.
    let (ghost_a, ok_a) = w.device_placement(DeviceKind::Bubbler, 30, 20, false);
    let (ghost_b, ok_b) = w.device_placement(DeviceKind::Bubbler, 30, 20, true);
    assert!(ok_a && ok_b);
    assert_eq!(ghost_a, ghost_b, "bed mount is symmetric and ignores flip");
    assert_eq!(ghost_a.y, 80 - art.size.1);
    assert!(!ghost_a.facing_left);

    let floor_id = w
        .add_device(DeviceKind::Bubbler, 30, 20, true)
        .unwrap()
        .unwrap();
    let floor_dev = *w.device(floor_id).unwrap();
    assert_eq!(floor_dev.y, 80 - art.size.1);

    // Substrate bed: drops onto the sand surface, lifting up if clicked inside it.
    let sand = id(&w, "sand");
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
    let (from_above, ok_above) = w.device_placement(DeviceKind::Bubbler, 75, 25, false);
    let (from_inside, ok_inside) = w.device_placement(DeviceKind::Bubbler, 75, 68, false);
    assert!(ok_above && ok_inside);
    assert_eq!(from_above.y, 66 - art.size.1);
    assert_eq!(from_inside.y, 66 - art.size.1);
    let bed_id = w
        .add_device(DeviceKind::Bubbler, 75, 68, false)
        .unwrap()
        .unwrap();
    assert_eq!(w.device(bed_id).unwrap().y, 66 - art.size.1);

    // Blocked by rock: refused cleanly.
    let rock = id(&w, "rock");
    w.fill_rect(
        Rect {
            x0: 100,
            y0: 0,
            x1: 140,
            y1: 80,
        },
        rock,
        Brush::Replace,
    )
    .unwrap();
    let (_, fits_in_rock) = w.device_placement(DeviceKind::Bubbler, 118, 40, false);
    assert!(!fits_in_rock);
    assert_eq!(
        w.add_device(DeviceKind::Bubbler, 118, 40, false).unwrap(),
        None
    );
}

#[test]
fn overwriting_or_scooping_any_cell_removes_the_whole_bubbler() {
    let mut w = world(100, 70, 2, 0.9);
    let art = w.device_kinds().get(DeviceKind::Bubbler).clone();
    let device = id(&w, "device");
    let water = id(&w, "water");

    let changed = w.place(Placeable::Bubbler, 50, 25).unwrap();
    assert_eq!(changed, art.cells);
    assert_eq!(count(&w, device), art.cells);
    let d = *w.devices().next().unwrap();
    assert_eq!(d.kind, DeviceKind::Bubbler);
    assert!(d.cells(&art).all(|(x, y)| w.device_at(x, y) == Some(d.id)));

    // Scooping a single footprint cell removes the whole device cleanly.
    let (cx, cy) = d.cells(&art).nth(art.cells / 2).unwrap();
    w.set_cell(cx, cy, water).unwrap();
    assert_eq!(w.devices().count(), 0);
    assert_eq!(count(&w, device), 0);
    assert!(
        d.cells(&art)
            .all(|(x, y)| w.cell(x, y).unwrap().material == water)
    );
}

#[test]
fn bubbler_emits_dense_stream_conserves_water_and_has_no_filter_or_jet_work() {
    let mut w = world(128, 96, 7, 0.9);
    let (water, bubble, waste) = (id(&w, "water"), id(&w, "air_bubble"), id(&w, "waste"));
    let art = w.device_kinds().get(DeviceKind::Bubbler).clone();
    let dev_id = w
        .add_device(DeviceKind::Bubbler, 64, 40, false)
        .unwrap()
        .unwrap();

    // Status reports a non-filter air diffuser with zero jet flow.
    let status = w.device_status(dev_id).unwrap();
    assert_eq!(status.kind, DeviceKind::Bubbler);
    assert!(!status.is_filter);
    assert!(status.fry_safe);
    assert_eq!(status.flow, 0.0);
    assert!(!w.rinse_device(dev_id, RinseKind::Gentle));
    assert_eq!(w.filter_bio_capacity(), 0.0);

    // Drop waste near the bubbler: it must not capture particles or mature.
    let d = *w.device(dev_id).unwrap();
    w.set_cell(d.x.saturating_sub(1), d.y + 3, waste).unwrap();
    let water_before = count(&w, water);

    let mut max_bubbles = 0;
    let mut seen_cols = std::collections::BTreeSet::new();
    for _ in 0..20 * 30 {
        w.tick();
        max_bubbles = max_bubbles.max(count(&w, bubble));
        assert_eq!(count(&w, water), water_before, "water level conserved");
        for (bx, by) in d.bubble_outlets(&art) {
            if w.cell(bx, by - 1).unwrap().material == bubble {
                seen_cols.insert(bx);
            }
        }
    }
    assert!(max_bubbles >= 6, "dense bubble stream: {max_bubbles}");
    assert!(
        seen_cols.len() >= 5,
        "bubbles emit across the diffuser face, saw columns {seen_cols:?}"
    );
    assert_eq!(
        w.intake_cell_checks(),
        0,
        "bubbler performs zero intake checks"
    );
    assert_eq!(w.device(dev_id).unwrap().media_load, 0.0);
    assert_eq!(w.device(dev_id).unwrap().maturity, 0.0);
    assert_eq!(w.filter_bio_capacity(), 0.0);
    assert_eq!(
        w.current_at(d.x + art.size.0 / 2, d.y.saturating_sub(10)),
        [0.0, 0.0],
        "bubbler drives no directed current jet"
    );
}

#[test]
fn bubbler_save_load_round_trip_replays_identically() {
    let mut w = world(128, 96, 19, 0.9);
    w.place(Placeable::Bubbler, 64, 40).unwrap();
    for _ in 0..80 {
        w.tick();
    }
    let bytes = save::encode(&w);
    let mut loaded = save::decode(&bytes, table()).unwrap();
    assert_eq!(loaded.state_hash(), w.state_hash());
    assert_eq!(loaded.devices().count(), 1);
    assert_eq!(loaded.devices().next().unwrap().kind, DeviceKind::Bubbler);
    for _ in 0..120 {
        w.tick();
        loaded.tick();
    }
    assert_eq!(loaded.state_hash(), w.state_hash());
}

#[test]
fn legacy_bubbler_cells_migrate_on_load_and_boxed_in_groups_stay_legacy() {
    let mut w = world(160, 96, 11, 0.9);
    let (bubbler, rock, bubble) = (id(&w, "bubbler"), id(&w, "rock"), id(&w, "air_bubble"));

    // Paint an open legacy 5x2 bubbler group on the floor.
    for dx in 0..5 {
        w.set_cell(40 + dx, 95, bubbler).unwrap();
    }
    for dx in 1..4 {
        w.set_cell(40 + dx, 94, bubbler).unwrap();
    }
    // Paint a second legacy bubbler group boxed into a tight rock pocket
    // (smaller than the 14x6 prefab footprint) with a narrow chimney above it.
    w.fill_rect(
        Rect {
            x0: 120,
            y0: 76,
            x1: 140,
            y1: 96,
        },
        rock,
        Brush::Replace,
    )
    .unwrap();
    let water = id(&w, "water");
    w.fill_rect(
        Rect {
            x0: 128,
            y0: 76,
            x1: 132,
            y1: 94,
        },
        water,
        Brush::Replace,
    )
    .unwrap();
    for dx in 0..3 {
        w.set_cell(129 + dx, 93, bubbler).unwrap();
    }
    assert_eq!(count(&w, bubbler), 11);

    // Save and reload: open group becomes a prefab bubbler; boxed-in stays legacy.
    let bytes = save::encode(&w);
    let mut loaded = save::decode(&bytes, table()).unwrap();
    let devices: Vec<_> = loaded.devices().copied().collect();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].kind, DeviceKind::Bubbler);
    assert_eq!(
        count(&loaded, bubbler),
        3,
        "boxed-in legacy group stays legacy"
    );

    // Round-tripping again keeps the exact state hash (boxed-in group does not
    // mutate or wake chunks on subsequent loads).
    let re_saved = save::encode(&loaded);
    let again = save::decode(&re_saved, table()).unwrap();
    assert_eq!(again.state_hash(), loaded.state_hash());

    // Both the migrated prefab and the boxed-in legacy cells still emit bubbles.
    let mut prefab_bubbles = 0;
    let mut legacy_bubbles = 0;
    for _ in 0..20 * 25 {
        loaded.tick();
        for x in 30..60 {
            if loaded.cell(x, 85).unwrap().material == bubble {
                prefab_bubbles += 1;
            }
        }
        for x in 128..132 {
            if loaded.cell(x, 85).unwrap().material == bubble {
                legacy_bubbles += 1;
            }
        }
    }
    assert!(prefab_bubbles > 0, "migrated prefab bubbler emits bubbles");
    assert!(
        legacy_bubbles > 0,
        "boxed-in legacy bubbler still emits bubbles"
    );
}
