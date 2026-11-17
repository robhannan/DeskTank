//! Starting scenes shared by `desktank window` and `desktank bench`.

use desktank_sim::{Brush, MaterialId, Rect, World};

/// A built-in material's ID. The built-in table always has these.
pub fn material(world: &World, name: &str) -> anyhow::Result<MaterialId> {
    world
        .materials()
        .id(name)
        .ok_or_else(|| anyhow::anyhow!("built-in material {name:?} is missing"))
}

/// A sand bed with a gravel patch and a few rocks, laid out from the seed.
///
/// # Errors
///
/// Returns an error if a built-in material is missing.
pub fn starter(world: &mut World, seed: u64) -> anyhow::Result<()> {
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

fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Rect {
    Rect { x0, y0, x1, y1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktank_sim::{MaterialTable, WorldConfig};

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
}
