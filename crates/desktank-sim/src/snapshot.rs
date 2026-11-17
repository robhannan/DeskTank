//! Debug snapshots of the grid, until the renderer exists: a binary PPM
//! image in the materials' placeholder colours, and an ASCII dump.

use std::io::{self, Write};

use crate::world::World;

/// Writes the grid as a binary PPM (P6) image, each cell drawn as a
/// `scale`×`scale` block (a scale of 0 is treated as 1).
///
/// # Errors
///
/// Returns any error from `out`.
pub fn write_ppm(world: &World, scale: usize, out: &mut impl Write) -> io::Result<()> {
    let scale = scale.max(1);
    let mut lut = [[255, 0, 255]; 256];
    for m in world.materials().iter() {
        lut[usize::from(m.id.0)] = m.colour;
    }
    let (w, h) = (world.width(), world.height());
    write!(out, "P6\n{} {}\n255\n", w * scale, h * scale)?;
    let mut row = Vec::with_capacity(w * scale * 3);
    for y in 0..h {
        row.clear();
        for c in &world.cells()[y * w..(y + 1) * w] {
            let rgb = lut[usize::from(c.material.0)];
            for _ in 0..scale {
                row.extend_from_slice(&rgb);
            }
        }
        for _ in 0..scale {
            out.write_all(&row)?;
        }
    }
    Ok(())
}

/// The grid as text, one line per row, using the first character of each
/// material's name (air is a space, water `~`).
#[must_use]
pub fn ascii(world: &World) -> String {
    let mut lut = ['?'; 256];
    for m in world.materials().iter() {
        lut[usize::from(m.id.0)] = match m.name.as_str() {
            "air" => ' ',
            "water" => '~',
            "air_bubble" => 'o',
            name => name.chars().next().unwrap_or('?'),
        };
    }
    let w = world.width();
    let mut s = String::with_capacity((w + 1) * world.height());
    for row in world.cells().chunks(w) {
        s.extend(row.iter().map(|c| lut[usize::from(c.material.0)]));
        s.push('\n');
    }
    s
}
