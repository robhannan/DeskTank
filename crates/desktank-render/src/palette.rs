//! The palette: a 256×256 RGBA table indexed by (material, variant) that the
//! fragment shader reads with `textureLoad`. Plant part cells are drawn
//! from their species instead (`plants.rs`, T6d), so their columns only
//! hold the material colour.

use desktank_sim::{
    GLUED_VARIANT_BIT, MAX_DRIFTWOOD_WETNESS, Material, MaterialTable, MovementClass,
    driftwood_wetness,
};

/// Table side: one column per material ID, one row per variant byte.
pub const SIDE: usize = 256;

/// How the fragment shader textures a non-plant material at scale ≥ 2,
/// stored in the palette texture's alpha byte (`tank.wgsl` `STYLE_*`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroundStyle {
    /// Generic cell (food, devices, propagules, eggs): fine sub-cell grain.
    Default = 0,
    /// Water: sub-cell depth gradient, subtle shimmer and surface meniscus.
    Water = 1,
    /// Air: clean backdrop with subtle sub-cell grain and gentle sky lift.
    Air = 2,
    /// Liquid fertiliser: depth-tinted liquid with wispy plume variation.
    Fertiliser = 3,
    /// Sand: fine multi-grain quartz texture with top-edge water lighting.
    Sand = 4,
    /// Gravel: rounded pebbles with per-stone variation and dark crevices.
    Gravel = 5,
    /// Aquasoil: dark rounded matte soil pellets with top-edge lighting.
    Aquasoil = 6,
    /// Rock: faceted stone strata, fine grain and edge bevels (keeps glue tint).
    Rock = 7,
    /// Driftwood: flowing wood grain fibres and bark edges (keeps glue tint).
    Driftwood = 8,
    /// Air and CO₂ bubbles: rounded translucent droplet with specular glint.
    Bubble = 9,
    /// Detritus, dead plant matter and waste: fine organic mulm specks.
    Detritus = 11,
    /// Prefab device footprint cell (D27): drawn as air above the water line
    /// and as water at or below it; the device sprite renders on top.
    Device = 12,
}

impl GroundStyle {
    /// Every style with its WGSL constant name (checked by `renderer.rs`).
    #[cfg(test)]
    pub const ALL: [(&'static str, Self); 12] = [
        ("STYLE_DEFAULT", Self::Default),
        ("STYLE_WATER", Self::Water),
        ("STYLE_AIR", Self::Air),
        ("STYLE_FERTILISER", Self::Fertiliser),
        ("STYLE_SAND", Self::Sand),
        ("STYLE_GRAVEL", Self::Gravel),
        ("STYLE_AQUASOIL", Self::Aquasoil),
        ("STYLE_ROCK", Self::Rock),
        ("STYLE_DRIFTWOOD", Self::Driftwood),
        ("STYLE_BUBBLE", Self::Bubble),
        ("STYLE_DETRITUS", Self::Detritus),
        ("STYLE_DEVICE", Self::Device),
    ];

    /// Picks the sub-cell style for `m`.
    #[must_use]
    pub fn for_material(m: &Material) -> Self {
        match m.name.as_str() {
            "water" => Self::Water,
            // A device's footprint cells are drawn as air above the water line
            // and as water below it; its sprite goes on top (D27, DV5).
            "device" => Self::Device,
            "air" => Self::Air,
            "liquid_fertiliser" => Self::Fertiliser,
            "sand" => Self::Sand,
            "gravel" | "clown_vomit_gravel" => Self::Gravel,
            "aquasoil" => Self::Aquasoil,
            "rock" => Self::Rock,
            "driftwood" => Self::Driftwood,
            "air_bubble" | "co2_bubble" => Self::Bubble,
            "detritus" | "dead_plant" | "waste" => Self::Detritus,
            _ if m.class == MovementClass::Liquid && m.density >= DEPTH_TINT_DENSITY => Self::Water,
            _ => Self::Default,
        }
    }

    /// Whether this style darkens with depth below the water surface.
    #[cfg(test)]
    #[must_use]
    pub const fn is_depth_tinted(self) -> bool {
        matches!(self, Self::Water | Self::Fertiliser)
    }
}

/// Brightness of each shade relative to the material colour, in 1/256. A
/// cell picks one from its variant byte: mostly the base, sometimes a darker
/// or lighter speck.
const SHADES: [u32; 4] = [256, 240, 274, 225];

const NEON_GRAVEL_COLOURS: [[u8; 3]; 6] = [
    [0xff, 0x2a, 0x9d],
    [0x00, 0x88, 0xff],
    [0x52, 0xf2, 0x18],
    [0xff, 0xde, 0x00],
    [0x9b, 0x26, 0xff],
    [0xff, 0x66, 0x00],
];

/// Colour of unknown material IDs: loud magenta, so a gap is obvious.
const UNKNOWN: [u8; 3] = [0xff, 0x00, 0xff];

/// Glued rigid cells (odd variants, D17) mix this much of [`GLUE_TINT`]
/// into their shade, in 1/256: a subtle amber cast.
const GLUE_MIX: u32 = 44;
/// The colour glued cells lean towards, sRGB.
const GLUE_TINT: [u8; 3] = [0xd9, 0xa4, 0x41];
/// Channel scale (in 1/256) for fully waterlogged driftwood: a darker,
/// richer wet bogwood tone (~0.56 R, 0.50 G, 0.44 B).
const DRIFTWOOD_WET_SCALE: [u32; 3] = [143, 128, 113];

/// Liquids at least this dense darken with depth (water, not air).
const DEPTH_TINT_DENSITY: f32 = 0.5;

/// Which shade a variant byte picks. Every range starts on an even byte,
/// so a rigid cell's glued bit ([`GLUED_VARIANT_BIT`]) never changes it.
#[must_use]
pub fn shade(variant: u8) -> usize {
    match variant {
        0..=119 => 0,
        120..=191 => 1,
        192..=239 => 2,
        _ => 3,
    }
}

/// Builds the RGBA table, row-major with rows = variants. Alpha holds the
/// material's [`GroundStyle`] discriminant (the shader's flag; output is
/// opaque). Rigid materials shade odd variants (glued bodies) with an amber
/// tint.
#[must_use]
pub fn build(materials: &MaterialTable) -> Vec<u8> {
    let mut columns = [[UNKNOWN; 4]; SIDE];
    let mut styles = [GroundStyle::Default; SIDE];
    let mut rigid = [false; SIDE];
    for m in materials.iter() {
        let i = usize::from(m.id.0);
        columns[i] = SHADES.map(|k| m.colour.map(|c| scale(c, k)));
        styles[i] = GroundStyle::for_material(m);
        rigid[i] = m.class == MovementClass::Rigid;
    }
    // A device's footprint cells are drawn as the water behind it; its
    // sprite goes on top (D27).
    if let (Some(device), Some(water)) = (materials.id("device"), materials.id("water")) {
        let (d, w) = (usize::from(device.0), usize::from(water.0));
        columns[d] = columns[w];
    }
    let mut out = vec![0; SIDE * SIDE * 4];
    let neon_gravel = materials.id("clown_vomit_gravel");
    for (variant, row) in out.as_chunks_mut::<{ SIDE * 4 }>().0.iter_mut().enumerate() {
        #[allow(clippy::cast_possible_truncation)] // variant < 256
        let s = shade(variant as u8);
        #[allow(clippy::cast_possible_truncation)] // variant < 256
        let glued = variant as u8 & GLUED_VARIANT_BIT != 0;
        #[allow(clippy::cast_possible_truncation)] // variant < 256
        let wetness = driftwood_wetness(variant as u8);
        for (material, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let mut rgb = if styles[material] == GroundStyle::Driftwood {
                waterlog_rgb(columns[material][0], wetness)
            } else {
                columns[material][s]
            };
            if glued && rigid[material] {
                rgb = glue(rgb);
            }
            let [r, g, b] = rgb;
            // The high bit marks rigid materials for sprite occlusion.
            let flag = styles[material] as u8 | (u8::from(rigid[material]) << 7);
            px.copy_from_slice(&[r, g, b, flag]);
        }
        if let Some(id) = neon_gravel {
            // A powder cell carries its variant with it as it falls and slides.
            let rgb = NEON_GRAVEL_COLOURS[variant % NEON_GRAVEL_COLOURS.len()];
            let offset = usize::from(id.0) * 4;
            row[offset..offset + 3].copy_from_slice(&rgb);
        }
    }
    out
}

fn glue(rgb: [u8; 3]) -> [u8; 3] {
    let mut out = rgb;
    for (o, (c, t)) in out.iter_mut().zip(rgb.into_iter().zip(GLUE_TINT)) {
        let v = (u32::from(c) * (256 - GLUE_MIX) + u32::from(t) * GLUE_MIX) / 256;
        *o = u8::try_from(v).unwrap_or(u8::MAX);
    }
    out
}

fn scale(c: u8, k: u32) -> u8 {
    u8::try_from(u32::from(c) * k / 256).unwrap_or(u8::MAX)
}

fn waterlog_rgb(dry: [u8; 3], wetness: u8) -> [u8; 3] {
    let w = u32::from(wetness);
    let max_w = u32::from(MAX_DRIFTWOOD_WETNESS);
    let mut out = dry;
    for (o, (c, wet_k)) in out.iter_mut().zip(dry.into_iter().zip(DRIFTWOOD_WET_SCALE)) {
        // Interpolate channel scale from 256 (dry) to wet_k (fully waterlogged).
        let k = 256 - ((256 - wet_k) * w + max_w / 2) / max_w;
        *o = scale(c, k);
    }
    out
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
        assert_eq!(a, GroundStyle::Sand as u8);
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
    fn clown_vomit_gravel_variants_are_bright_saturated_pebbles() {
        let mats = MaterialTable::builtin().unwrap();
        let table = build(&mats);
        let id = mats.id("clown_vomit_gravel").unwrap().0;
        let mut colours = std::collections::BTreeSet::new();
        for v in 0..=u8::MAX {
            let [r, g, b, a] = texel(&table, id, v);
            let rgb = [r, g, b];
            assert_eq!(a, GroundStyle::Gravel as u8);
            let (min, max) = (*rgb.iter().min().unwrap(), *rgb.iter().max().unwrap());
            assert!(max >= 210 && max - min >= 180, "{rgb:?}");
            if v < 6 {
                colours.insert(rgb);
            }
        }
        assert_eq!(colours.len(), 6);
    }

    #[test]
    fn glued_rigid_cells_get_a_tint_and_keep_their_shade() {
        let mats = MaterialTable::builtin().unwrap();
        let table = build(&mats);
        let rock = mats.id("rock").unwrap().0;
        let sand = mats.id("sand").unwrap().0;
        for v in (0..=254u8).step_by(2) {
            assert_eq!(shade(v), shade(v | GLUED_VARIANT_BIT));
            let (free, glued) = (texel(&table, rock, v), texel(&table, rock, v + 1));
            assert_ne!(free, glued);
            // Subtle: within 40 levels per channel, and warmer.
            assert!(free.iter().zip(glued).all(|(a, b)| a.abs_diff(b) <= 40));
            assert!(glued[0] > free[0] && glued[2] <= free[2]);
            // Other classes ignore the bit.
            assert_eq!(texel(&table, sand, v), texel(&table, sand, v + 1));
        }
    }

    #[test]
    fn only_dense_liquids_are_depth_tinted() {
        let mats = MaterialTable::builtin().unwrap();
        let table = build(&mats);
        for m in mats.iter() {
            let style = GroundStyle::for_material(m);
            assert_eq!(texel(&table, m.id.0, 0)[3] & 127, style as u8, "{}", m.name);
            assert_eq!(
                texel(&table, m.id.0, 0)[3] & 128 != 0,
                m.class == MovementClass::Rigid
            );
            let dense = matches!(m.name.as_str(), "water" | "liquid_fertiliser");
            assert_eq!(style.is_depth_tinted(), dense, "{}", m.name);
        }
    }

    #[test]
    fn ground_styles_match_expected_materials() {
        let mats = MaterialTable::builtin().unwrap();
        let table = build(&mats);
        let expected = [
            ("water", GroundStyle::Water),
            ("air", GroundStyle::Air),
            ("device", GroundStyle::Device),
            ("liquid_fertiliser", GroundStyle::Fertiliser),
            ("sand", GroundStyle::Sand),
            ("gravel", GroundStyle::Gravel),
            ("clown_vomit_gravel", GroundStyle::Gravel),
            ("aquasoil", GroundStyle::Aquasoil),
            ("rock", GroundStyle::Rock),
            ("driftwood", GroundStyle::Driftwood),
            ("air_bubble", GroundStyle::Bubble),
            ("co2_bubble", GroundStyle::Bubble),
            ("detritus", GroundStyle::Detritus),
            ("dead_plant", GroundStyle::Detritus),
            ("waste", GroundStyle::Detritus),
            ("flake_food", GroundStyle::Default),
            ("bubbler", GroundStyle::Default),
        ];
        for (name, style) in expected {
            let id = mats.id(name).unwrap().0;
            assert_eq!(texel(&table, id, 0)[3] & 127, style as u8, "{name}");
            assert_eq!(texel(&table, id, 1)[3] & 127, style as u8, "{name} (glued)");
        }
    }

    #[test]
    fn driftwood_darkens_with_wetness_and_keeps_glue_tint() {
        let mats = MaterialTable::builtin().unwrap();
        let table = build(&mats);
        let wood = mats.get(mats.id("driftwood").unwrap()).unwrap();
        let dry = texel(&table, wood.id.0, 0);
        assert_eq!(&dry[..3], &wood.colour);
        let wet = texel(
            &table,
            wood.id.0,
            desktank_sim::driftwood_variant(MAX_DRIFTWOOD_WETNESS, false),
        );
        // Fully waterlogged driftwood is noticeably darker across all channels.
        for c in 0..3 {
            assert!(
                wet[c] < dry[c],
                "channel {c}: wet {} vs dry {}",
                wet[c],
                dry[c]
            );
        }
        assert!(u32::from(wet[0]) * 2 < u32::from(dry[0]) * 3);
        // Monotonic darkening for both free and glued driftwood, with glued warmer.
        let mut prev_free = dry;
        let mut prev_glued = texel(&table, wood.id.0, desktank_sim::driftwood_variant(0, true));
        for w in 1..=MAX_DRIFTWOOD_WETNESS {
            let free = texel(&table, wood.id.0, desktank_sim::driftwood_variant(w, false));
            let glued = texel(&table, wood.id.0, desktank_sim::driftwood_variant(w, true));
            for c in 0..3 {
                assert!(free[c] <= prev_free[c], "free ch {c} at w={w}");
                assert!(glued[c] <= prev_glued[c], "glued ch {c} at w={w}");
            }
            assert!(glued[0] > free[0], "glued stays warmer at w={w}");
            prev_free = free;
            prev_glued = glued;
        }
        let glued_dry = texel(&table, wood.id.0, desktank_sim::driftwood_variant(0, true));
        let glued_wet = texel(
            &table,
            wood.id.0,
            desktank_sim::driftwood_variant(MAX_DRIFTWOOD_WETNESS, true),
        );
        assert!(
            glued_wet[0] < glued_dry[0]
                && glued_wet[1] < glued_dry[1]
                && glued_wet[2] < glued_dry[2]
        );
    }

    #[test]
    fn unknown_ids_are_magenta() {
        let mats = MaterialTable::builtin().unwrap();
        let table = build(&mats);
        assert_eq!(texel(&table, 255, 0), [0xff, 0, 0xff, 0]);
    }
}
