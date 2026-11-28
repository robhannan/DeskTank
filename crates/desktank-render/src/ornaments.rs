//! Fine ornament art and drawn body poses share one nearest-texel texture.
//! Rows 0–255 hold one inverse pose per body id. Row 256 indexes rest-pose
//! images by material id. The images follow, packed in linear texel order.

use std::collections::BTreeMap;

use desktank_sim::World;

pub const WIDTH: usize = 256;
const IMAGE_ROW: usize = WIDTH + 1;
pub const TEXEL_BYTES: usize = 16;

pub fn bake(world: &World, max_height: usize) -> Vec<[f32; 4]> {
    let mut texels = vec![[0.0; 4]; WIDTH * IMAGE_ROW];
    for material in world.materials().iter() {
        let Some(art) = &material.ornament else {
            continue;
        };
        let (width, height) = art.size();
        let (width, height) = (width * 4, height * 4);
        let start = texels.len();
        if start.saturating_add(width * height) > WIDTH * max_height {
            continue;
        }
        let is_castle = matches!(material.name.as_str(), "stone_castle" | "pastel_castle");
        let is_volcano = material.name == "volcano_bubbler";
        #[allow(clippy::cast_precision_loss)] // Texture addresses fit in 24 bits.
        let info = [start as f32, width as f32, height as f32, 1.0];
        texels[WIDTH * WIDTH + usize::from(material.id.0)] = info;
        texels.resize(start + width * height, [0.0; 4]);
        let pixels: BTreeMap<_, _> = art.pixels().map(|(x, y, v)| ((x, y), v)).collect();
        for (&(x, y), &variant) in &pixels {
            let Some(colour) = art.colour(variant) else {
                continue;
            };
            for qy in 0..4 {
                for qx in 0..4 {
                    let q = (x * 4 + qx, y * 4 + qy);
                    let (dx, dy) = (qx.min(3 - qx), qy.min(3 - qy));
                    let side = if qx < 2 { x.wrapping_sub(1) } else { x + 1 };
                    let row = if qy < 2 { y.wrapping_sub(1) } else { y + 1 };
                    let open_x = !pixels.contains_key(&(side, y));
                    let open_y = !pixels.contains_key(&(x, row));
                    // Step the convex corners down to the same quarter-cell grid as
                    // creatures and hardscape, while keeping the authored openings.
                    if dx + dy < 2 && open_x && open_y {
                        continue;
                    }
                    let edge = (dx == 0 && open_x) || (dy == 0 && open_y);
                    let tone = tone(q, variant >> 1, is_castle, is_volcano, edge);
                    let [r, g, b] = colour.map(|c| (f32::from(c) / 255.0 * tone).min(1.0));
                    texels[start + q.1 * width + q.0] = [r, g, b, 1.0];
                }
            }
        }
    }
    texels.resize(texels.len().next_multiple_of(WIDTH), [0.0; 4]);
    texels
}

/// Roof courses, staggered masonry and arch trim for the castles. The
/// volcano gets narrow highlights in its hot flows and broken basalt facets.
fn tone((x, y): (usize, usize), key: u8, castle: bool, volcano: bool, edge: bool) -> f32 {
    let grain = (x * 13 + y * 23 + x * y * 7) % 19;
    let variation = match grain {
        0..=2 => 0.12,
        3..=5 => -0.10,
        _ => 0.0,
    };
    let shade = match (castle, volcano, key) {
        (true, _, b'b' | b'r' | b't') => {
            if y % 4 == 3 {
                0.72
            } else if y % 4 == 0 {
                1.23
            } else {
                1.0
            }
        }
        (true, _, b'k' | b'd' | b'm' | b's' | b'h') => {
            let joint = (x + y / 5 % 2 * 4) % 8;
            if edge {
                // A bevel follows the open arch as well as the outer walls.
                1.26
            } else if y % 5 == 4 || joint == 0 {
                0.68
            } else if y % 5 == 0 || joint == 1 {
                1.20
            } else {
                1.0
            }
        }
        (_, true, b'r' | b's' | b'h' | b't') => match (x + y / 3 + (y / 9) % 2) % 6 {
            0 => 0.75,
            1 => 1.35,
            2 => 1.16,
            _ => 0.98,
        },
        (_, true, _) => match (x / 3 + y / 2 + x / 9) % 5 {
            0 => 0.74,
            1 => 1.23,
            _ => 1.0,
        },
        _ if edge => 0.87,
        _ => 1.0,
    };
    shade + variation
}

pub fn has_image(texels: &[[f32; 4]], material: usize) -> bool {
    texels[WIDTH * WIDTH + material][3] > 0.0
}

pub fn bytes(texels: &[[f32; 4]]) -> Vec<u8> {
    texels
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fine_art_trims_edges_and_shades_each_builtin_ornament() {
        let world = World::new(
            desktank_sim::WorldConfig::default(),
            desktank_sim::MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let texels = bake(&world, 2048);
        for name in ["stone_castle", "pastel_castle", "volcano_bubbler"] {
            let id = world.materials().id(name).unwrap();
            let art = world
                .materials()
                .get(id)
                .unwrap()
                .ornament
                .as_ref()
                .unwrap();
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let [start, width, ..] = texels[WIDTH * WIDTH + usize::from(id.0)].map(|v| v as usize);
            let mut trimmed = false;
            let mut shaded = false;
            for (x, y, _) in art.pixels() {
                let cell: Vec<_> = (0..4)
                    .flat_map(|dy| {
                        let row = start + (y * 4 + dy) * width + x * 4;
                        &texels[row..row + 4]
                    })
                    .collect();
                assert!(cell.iter().any(|t| t[3] > 0.0), "{name} ({x}, {y})");
                trimmed |= cell.iter().any(|t| t[3] == 0.0);
                shaded |= cell[0][3] > 0.0
                    && cell
                        .iter()
                        .any(|t| t[3] > 0.0 && (t[0] - cell[0][0]).abs() > 0.01);
            }
            assert!(trimmed && shaded, "{name}");
        }
    }
}
