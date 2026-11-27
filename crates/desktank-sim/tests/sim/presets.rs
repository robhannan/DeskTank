//! New Tank presets (#473): the preset list, centimetre sizes, the Empty and
//! Artificial layouts, determinism and the initial settings.

use desktank_sim::species::CELLS_PER_CM;
use desktank_sim::{
    DEFAULT_CREATURE_CAP, DeviceKind, MAX_CREATURE_CAP, MAX_TIME_RATIO, NewTankConfig, TankPreset,
    World, WorldConfig, dimensions_cm, save,
};

fn config(preset: TankPreset, width: usize, height: usize, seed: u64) -> NewTankConfig {
    NewTankConfig {
        world: WorldConfig {
            width,
            height,
            seed,
            ..WorldConfig::default()
        },
        preset,
        ..NewTankConfig::default()
    }
}

fn count(w: &World, name: &str) -> usize {
    let id = w.materials().id(name).expect("material exists");
    w.material_counts()[usize::from(id.0)]
}

fn rock_bodies(w: &World) -> Vec<desktank_sim::BodyInfo> {
    let rock = w.materials().id("rock").expect("rock exists");
    w.bodies().filter(|b| b.material == rock).collect()
}

#[test]
fn presets_have_stable_ids_names_and_descriptions() {
    assert_eq!(
        TankPreset::ALL,
        &[TankPreset::Empty, TankPreset::Artificial]
    );
    assert_eq!(TankPreset::default(), TankPreset::Empty);
    for (preset, id, name) in [
        (TankPreset::Empty, "empty", "Empty"),
        (TankPreset::Artificial, "artificial", "Artificial"),
    ] {
        assert_eq!(preset.id(), id);
        assert_eq!(preset.name(), name);
        assert!(preset.description().len() > 10);
        assert_eq!(TankPreset::from_id(id), Some(preset));
        assert_eq!(TankPreset::from_id(&id.to_uppercase()), Some(preset));
    }
    for preset in TankPreset::ALL {
        assert_eq!(TankPreset::from_id(preset.id()), Some(*preset));
    }
    assert_eq!(TankPreset::from_id("reef"), None);
    assert_eq!(TankPreset::from_id(""), None);
}

#[test]
fn dimensions_follow_cells_per_cm() {
    let (w, h) = dimensions_cm(640, 360);
    assert!((w - 640.0 / CELLS_PER_CM).abs() < f32::EPSILON);
    assert!((h - 360.0 / CELLS_PER_CM).abs() < f32::EPSILON);
    assert_eq!(dimensions_cm(64, 48), (32.0, 24.0));
    assert_eq!(
        config(TankPreset::Empty, 320, 180, 0).dimensions_cm(),
        (160.0, 90.0)
    );
}

#[test]
fn default_config_matches_the_world_defaults() {
    let c = NewTankConfig::default();
    assert_eq!(c.preset, TankPreset::Empty);
    assert!(c.include_devices);
    assert!(!c.easy_mode);
    assert_eq!(usize::from(c.population_cap), DEFAULT_CREATURE_CAP);
    assert!((c.time_ratio - 1.0).abs() < f32::EPSILON);
    let w = World::from_new_tank(&c).unwrap();
    assert_eq!((w.width(), w.height()), (640, 360));
    assert_eq!(w.creature_cap(), DEFAULT_CREATURE_CAP);
}

#[test]
fn empty_tank_is_water_and_air_only() {
    for include_devices in [true, false] {
        let mut c = config(TankPreset::Empty, 160, 90, 3);
        c.include_devices = include_devices;
        let w = c.build().unwrap();
        assert_eq!(w.devices().count(), 0);
        assert_eq!(w.bodies().count(), 0);
        assert_eq!(w.creatures().len(), 0);
        let (water, air) = (count(&w, "water"), count(&w, "air"));
        assert_eq!(water + air, 160 * 90, "only water and air");
        assert!(water > 0 && air > 0);
    }
}

#[test]
fn artificial_tank_has_gravel_rocks_and_devices() {
    let mut c = config(TankPreset::Artificial, 320, 180, 11);
    let w = c.build().unwrap();
    assert!(count(&w, "clown_vomit_gravel") > 0);
    let rocks = rock_bodies(&w);
    assert!((2..=4).contains(&rocks.len()), "{} rocks", rocks.len());
    let surface = w.surface_row();
    for rock in &rocks {
        assert!(rock.anchored, "rocks are glued");
        assert!(rock.bounds.y0 > surface, "rock stays under the water line");
    }
    let kinds: Vec<_> = w.devices().map(|d| d.kind).collect();
    assert!(kinds.contains(&DeviceKind::InternalFilter));
    assert!(kinds.contains(&DeviceKind::Bubbler));
    assert!(w.devices().all(|d| w.device_status(d.id).is_some()));
    assert_eq!(w.creatures().len(), 0);

    c.include_devices = false;
    let bare = c.build().unwrap();
    assert_eq!(bare.devices().count(), 0);
    assert!(count(&bare, "clown_vomit_gravel") > 0);
    assert!(!rock_bodies(&bare).is_empty(), "rocks stay without devices");
}

#[test]
fn artificial_bed_is_gentle_and_leaves_water_above() {
    for (width, height) in [(64, 48), (320, 180), (640, 360)] {
        let w = config(TankPreset::Artificial, width, height, 5)
            .build()
            .unwrap();
        let gravel = w.materials().id("clown_vomit_gravel").unwrap();
        let water_rows = height - w.surface_row();
        let bed: usize = (0..width)
            .map(|x| {
                (0..height)
                    .filter(|&y| w.cell(x, y).is_some_and(|c| c.material == gravel))
                    .count()
            })
            .max()
            .unwrap();
        assert!(bed > 0, "{width}x{height} has a bed");
        assert!(bed <= water_rows / 3, "{width}x{height}: bed {bed} rows");
        assert!(bed <= height * 12 / 100 + 1);
    }
}

#[test]
fn tanks_of_every_size_build() {
    for preset in TankPreset::ALL {
        for (width, height) in [(64, 48), (320, 180), (640, 360), (1, 1), (8, 6), (30, 20)] {
            for include_devices in [true, false] {
                for seed in 0..4 {
                    let mut c = config(*preset, width, height, seed);
                    c.include_devices = include_devices;
                    let w = c.build().unwrap_or_else(|e| {
                        panic!("{} {width}x{height} seed {seed}: {e}", preset.id())
                    });
                    assert_eq!((w.width(), w.height()), (width, height));
                }
            }
        }
    }
}

#[test]
fn small_artificial_tank_still_gets_rocks() {
    let w = config(TankPreset::Artificial, 64, 48, 1).build().unwrap();
    assert!(count(&w, "clown_vomit_gravel") > 0);
    assert!(rock_bodies(&w).len() >= 2);
}

#[test]
fn builds_are_deterministic_and_seeds_vary_the_layout() {
    let hash = |seed| {
        config(TankPreset::Artificial, 320, 180, seed)
            .build()
            .unwrap()
            .state_hash()
    };
    assert_eq!(hash(4), hash(4));
    let hashes: Vec<_> = (0..6).map(hash).collect();
    for (i, a) in hashes.iter().enumerate() {
        for b in &hashes[i + 1..] {
            assert_ne!(a, b, "different seeds make different tanks");
        }
    }
    // A run after the build is deterministic too.
    let run = || {
        let mut w = config(TankPreset::Artificial, 160, 90, 9).build().unwrap();
        for _ in 0..100 {
            w.tick();
        }
        w.state_hash()
    };
    assert_eq!(run(), run());
}

#[test]
fn initial_settings_apply_and_survive_a_save() {
    let mut c = config(TankPreset::Artificial, 160, 90, 2);
    c.world.water_level = 0.8;
    c.easy_mode = true;
    c.population_cap = 24;
    c.time_ratio = 4.0;
    let w = c.build().unwrap();
    assert_eq!(w.surface_row(), w.height() - (w.height() * 4 / 5));
    assert!(w.easy_mode());
    assert_eq!(w.creature_cap(), 24);
    assert!((w.time_ratio() - 4.0).abs() < f32::EPSILON);

    let bytes = save::encode(&w);
    let back = save::decode(&bytes, desktank_sim::MaterialTable::builtin().unwrap()).unwrap();
    assert_eq!(back.surface_row(), w.surface_row());
    assert!(back.easy_mode());
    assert_eq!(back.creature_cap(), 24);
    assert!((back.time_ratio() - 4.0).abs() < f32::EPSILON);
    assert_eq!(back.devices().count(), w.devices().count());
    assert_eq!(back.bodies().count(), w.bodies().count());
    assert_eq!(back.state_hash(), w.state_hash());
}

#[test]
fn settings_are_clamped_not_rejected() {
    let mut c = config(TankPreset::Empty, 64, 48, 0);
    c.population_cap = u16::MAX;
    c.time_ratio = MAX_TIME_RATIO * 10.0;
    let w = c.build().unwrap();
    assert_eq!(w.creature_cap(), MAX_CREATURE_CAP);
    assert!((w.time_ratio() - MAX_TIME_RATIO).abs() < f32::EPSILON);
    c.population_cap = 0;
    c.time_ratio = f32::NAN;
    let w = c.build().unwrap();
    assert_eq!(w.creature_cap(), 1);
    assert!((w.time_ratio() - 1.0).abs() < f32::EPSILON);
    c.world.water_level = 2.0;
    assert!(c.build().is_err());
}

#[test]
fn artificial_layout_holds_still_when_ticked() {
    for (width, height) in [(64, 48), (320, 180)] {
        for seed in 0..6 {
            let mut w = config(TankPreset::Artificial, width, height, seed)
                .build()
                .unwrap();
            let rocks: Vec<_> = rock_bodies(&w)
                .iter()
                .map(|b| (b.bounds, b.cells))
                .collect();
            let devices = w.devices().count();
            assert!(!rocks.is_empty(), "{width}x{height} seed {seed}: no rocks");
            for _ in 0..200 {
                w.tick();
            }
            let after: Vec<_> = rock_bodies(&w)
                .iter()
                .map(|b| (b.bounds, b.cells))
                .collect();
            assert_eq!(rocks, after, "{width}x{height} seed {seed}: rocks moved");
            assert_eq!(w.devices().count(), devices);
        }
    }
}
