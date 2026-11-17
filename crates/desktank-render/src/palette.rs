//! The palette: a 256×256 RGBA table indexed by (material, variant) that the
//! fragment shader reads with `textureLoad`.

use desktank_sim::{MaterialTable, MovementClass};

/// Table side: one column per material ID, one row per variant byte.
pub const SIDE: usize = 256;

/// Brightness of each shade relative to the material colour, in 1/256. A
/// cell picks one from its variant byte: mostly the base, sometimes a darker
/// or lighter speck.
const SHADES: [u32; 4] = [256, 240, 274, 225];

/// Colour of unknown material IDs: loud magenta, so a gap is obvious.
const UNKNOWN: [u8; 3] = [0xff, 0x00, 0xff];

/// Liquids at least this dense darken with depth (water, not air).
const DEPTH_TINT_DENSITY: f32 = 0.5;

/// Which shade a variant byte picks.
#[must_use]
pub fn shade(variant: u8) -> usize {
    match variant {
        0..=119 => 0,
        120..=191 => 1,
        192..=239 => 2,
        _ => 3,
    }
}

/// Builds the RGBA table, row-major with rows = variants. Alpha is 255 for
/// materials that darken with depth and 0 otherwise (the shader's flag;
/// output is opaque).
#[must_use]
pub fn build(materials: &MaterialTable) -> Vec<u8> {
    let mut columns = [[UNKNOWN; 4]; SIDE];
    let mut tinted = [false; SIDE];
    for m in materials.iter() {
        let i = usize::from(m.id.0);
        columns[i] = SHADES.map(|k| m.colour.map(|c| scale(c, k)));
        tinted[i] = m.class == MovementClass::Liquid && m.density >= DEPTH_TINT_DENSITY;
    }
    let mut out = vec![0; SIDE * SIDE * 4];
    for (variant, row) in out.as_chunks_mut::<{ SIDE * 4 }>().0.iter_mut().enumerate() {
        #[allow(clippy::cast_possible_truncation)] // variant < 256
        let s = shade(variant as u8);
        for (material, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let [r, g, b] = columns[material][s];
            px.copy_from_slice(&[r, g, b, if tinted[material] { 255 } else { 0 }]);
        }
    }
    out
}

fn scale(c: u8, k: u32) -> u8 {
    u8::try_from(u32::from(c) * k / 256).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texel(table: &[u8], material: u8, variant: u8) -> [u8; 4] {
        let i = (usize::from(variant) * SIDE + usize::from(material)) * 4;
        table[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn base_shade_is_the_material_colour() {
        let mats = MaterialTable::builtin().unwrap();
        let table = build(&mats);
        let sand = mats.get(mats.id("sand").unwrap()).unwrap();
        let [r, g, b, a] = texel(&table, sand.id.0, 0);
        assert_eq!([r, g, b], sand.colour);
        assert_eq!(a, 0);
    }

    #[test]
    fn variants_pick_lighter_and_darker_specks() {
        let mats = MaterialTable::builtin().unwrap();
        let table = build(&mats);
        let id = mats.id("gravel").unwrap().0;
        let base = texel(&table, id, 0)[0];
        assert!(texel(&table, id, 150)[0] < base);
        assert!(texel(&table, id, 200)[0] > base);
        assert!(texel(&table, id, 250)[0] < base);
    }

    #[test]
    fn only_water_is_depth_tinted() {
        let mats = MaterialTable::builtin().unwrap();
        let table = build(&mats);
        for m in mats.iter() {
            let tinted = texel(&table, m.id.0, 0)[3] == 255;
            assert_eq!(tinted, m.name == "water", "{}", m.name);
        }
    }

    #[test]
    fn unknown_ids_are_magenta() {
        let mats = MaterialTable::builtin().unwrap();
        let table = build(&mats);
        assert_eq!(texel(&table, 255, 0), [0xff, 0, 0xff, 0]);
    }
}
