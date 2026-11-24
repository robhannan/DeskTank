//! Starting scenes shared by the hosts: `desktank window`, `desktank bench`,
//! `desktank capture` and the Android Dream.

use crate::species::Zone;
use crate::{
    Brush, FieldKind, MaterialId, MaterialTable, Propagule, Rect, SETTLE_TICKS, TICKS_PER_SECOND,
    World, WorldConfig, WorldError,
};

/// A built-in material's ID. The built-in table always has these.
///
/// # Errors
///
/// Returns [`WorldError::MissingMaterial`] if the world's table lacks it.
pub fn material(world: &World, name: &'static str) -> Result<MaterialId, WorldError> {
    world
        .materials()
        .id(name)
        .ok_or(WorldError::MissingMaterial(name))
}

/// A sand bed with a gravel patch and a few rocks, laid out from the seed.
///
/// # Errors
///
/// Returns an error if a built-in material is missing or the world rejects
/// a fill.
pub fn starter(world: &mut World, seed: u64) -> Result<(), WorldError> {
    let (w, h) = (world.width(), world.height());
    let [sand, gravel, rock] = ["sand", "gravel", "rock"].map(|n| material(world, n));
    let (sand, gravel, rock) = (sand?, gravel?, rock?);
    // A gently rolling bed: 6–10% of the height, varying slowly across.
    let base = h / 16;
    let swell = (h / 40).max(1);
    let phase = seed % 97;
    for x in 0..w {
        let t = (x as u64 + phase) % 160;
        let bump = usize::try_from(t.min(160 - t)).unwrap_or(0) * swell / 80;
        let top = h.saturating_sub(base + bump);
        world.fill_rect(rect(x, top, x + 1, h), sand, Brush::Replace)?;
    }
    // Gravel at one end, a few rocks on the bed.
    let gw = w / 5;
    world.fill_rect(rect(w - gw, h - base / 2, w, h), gravel, Brush::Replace)?;
    for (i, frac) in [0.2, 0.47, 0.71].into_iter().enumerate() {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let cx = (w as f64 * frac) as i64;
        let r = u32::try_from(h / 30 + i * h / 90).unwrap_or(1);
        let cy = i64::try_from(h - base).unwrap_or(0);
        world.paint(cx, cy, r, rock, Brush::Replace)?;
    }
    Ok(())
}

/// Adds `count` creatures (stopping at the tank's population cap), cycling through the
/// swimming and bottom-walking species (fish and shrimp; not the surface
/// crawlers or the cheat-only crabs), each placed at a seeded spot in the middle of its zone (or the
/// nearest free water). Returns how many were added.
///
/// # Errors
///
/// Returns an error if the world rejects a creature for a reason other
/// than having no room at its spot.
pub fn stock(world: &mut World, count: usize, seed: u64) -> Result<usize, WorldError> {
    let species: Vec<_> = world
        .species()
        .iter()
        .filter(|s| s.zone != Zone::Surfaces && s.stocked())
        .map(|s| s.id)
        .collect();
    if species.is_empty() {
        return Ok(0);
    }
    let width = u64::try_from(world.width()).unwrap_or(1).max(1);
    let mut state = seed ^ 0x9e37_79b9_7f4a_7c15;
    let mut added = 0;
    for i in 0..count {
        if world.stock_count() >= world.creature_cap() {
            break;
        }
        let s = species[i % species.len()];
        let Some((top, bottom)) = world.zone_rows(s) else {
            continue;
        };
        // A small LCG: spread across the tank, the same for a seed.
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let x = i64::try_from((state >> 33) % width).unwrap_or(0);
        let y = i64::try_from(usize::midpoint(top, bottom)).unwrap_or(0);
        match world.add_creature(s, x, y) {
            Ok(_) => added += 1,
            Err(WorldError::NoRoom) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(added)
}

fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Rect {
    Rect { x0, y0, x1, y1 }
}

/// Small reference tank for a day of unobserved simulation. Young rotala
/// cuttings grow in nutrient-rich water. Flakes supplement the algae that
/// feeds the breeding cherry shrimp, corydoras and nerites.
///
/// # Errors
///
/// Returns an error if the world lacks the scene's materials or species,
/// or cannot place its contents.
pub fn catch_up_reference(seed: u64, materials: MaterialTable) -> Result<World, WorldError> {
    const WIDTH: usize = 160;
    const HEIGHT: usize = 90;
    const BED: usize = 83;
    let mut world = World::new(
        WorldConfig {
            width: WIDTH,
            height: HEIGHT,
            seed,
            ..WorldConfig::default()
        },
        materials,
    )?;
    let sand = material(&world, "sand")?;
    let flake = material(&world, "flake_food")?;
    world.fill_rect(rect(0, BED, WIDTH, HEIGHT), sand, Brush::Replace)?;
    let field_scale = crate::field::FIELD_SCALE;
    for y in (0..HEIGHT).step_by(field_scale) {
        for x in (0..WIDTH).step_by(field_scale) {
            world.add_to_field(FieldKind::Nutrient, x, y, 0.92);
        }
    }
    let rotala = world
        .plant_species()
        .id("rotala")
        .ok_or_else(|| WorldError::PlantSpecies("missing rotala".into()))?;
    for x in [24, 80, 132] {
        world.paint_plant(x, 82, 0, rotala, Propagule::Cuttings)?;
    }
    for x in (2..WIDTH).step_by(2) {
        world.set_cell(x, BED - 2, flake)?;
    }
    for _ in 0..600 {
        world.tick();
    }
    world.set_creature_cap(96);
    let species = |name| {
        world
            .species()
            .id(name)
            .ok_or_else(|| WorldError::Species(format!("missing {name}")))
    };
    let shrimp = species("cherry_shrimp")?;
    let cory = species("corydoras")?;
    let nerite = species("nerite")?;
    for x in [16, 52, 96, 143] {
        world.add_creature(shrimp, x, 76)?;
    }
    world.add_creature(cory, 16, 72)?;
    world.add_creature(nerite, 16, 77)?;
    let elder = world.add_creature(nerite, 52, 77)?;
    if let Some(creature) = world.creature(elder) {
        // Leave one natural age death in the day, even with abundant food.
        let remaining = 18 * 3600 * TICKS_PER_SECOND;
        let age = world.creature_lifespan(creature).saturating_sub(remaining);
        world.set_creature_age(elder, age);
    }
    world.skip_breeding_time(SETTLE_TICKS);
    Ok(world)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn starter_scene_settles() {
        let config = WorldConfig {
            width: 320,
            height: 180,
            seed: 7,
            ..WorldConfig::default()
        };
        let mut world = World::new(config, MaterialTable::builtin().unwrap()).unwrap();
        starter(&mut world, 7).unwrap();
        let sand = material(&world, "sand").unwrap();
        assert!(world.material_counts()[usize::from(sand.0)] > 320 * 180 / 20);
        for _ in 0..2000 {
            world.tick();
            if world.awake_chunks() == 0 {
                return;
            }
        }
        panic!("starter scene still awake after 100 s");
    }

    #[test]
    fn stock_adds_every_swimmer_and_walker_up_to_the_cap() {
        let config = WorldConfig {
            width: 320,
            height: 180,
            seed: 7,
            ..WorldConfig::default()
        };
        let mut world = World::new(config, MaterialTable::builtin().unwrap()).unwrap();
        starter(&mut world, 7).unwrap();
        assert_eq!(stock(&mut world, 30, 7).unwrap(), 30);
        let stocked = world
            .species()
            .iter()
            .filter(|s| s.zone != Zone::Surfaces && s.stocked())
            .count();
        // Round robin: every swimmer and walker gets 30 / n, the first
        // 30 % n one more; crawlers none.
        for (i, s) in world
            .species()
            .iter()
            .filter(|s| s.zone != Zone::Surfaces && s.stocked())
            .enumerate()
        {
            let n = world
                .creatures()
                .iter()
                .filter(|c| c.species() == s.id)
                .count();
            assert_eq!(n, 30 / stocked + usize::from(i < 30 % stocked));
        }
        assert!(
            world
                .creatures()
                .iter()
                .all(|c| world.species().get(c.species()).unwrap().zone != Zone::Surfaces)
        );
        assert_eq!(
            stock(&mut world, 1000, 7).unwrap(),
            crate::DEFAULT_CREATURE_CAP - 30
        );
    }
}
