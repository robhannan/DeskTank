//! Rigid prefab artwork. Each opaque pixel is a cell of one rigid body.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::MaterialId;
use crate::material::parse_colour;

/// Largest ornament footprint, also the renderer's ghost instance budget.
pub const MAX_CELLS: usize = 1024;

/// An ornament's rest-pose bitmap and optional DV7 air outlets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrnamentArt {
    size: (usize, usize),
    pixels: Vec<Option<u8>>,
    colours: BTreeMap<u8, [u8; 3]>,
    pub(crate) bubbler_outlets: Vec<[i16; 2]>,
}

impl OrnamentArt {
    /// Stored frame dimensions in tank cells.
    #[must_use]
    pub fn size(&self) -> (usize, usize) {
        self.size
    }

    /// Opaque cells in top-to-bottom order, with their packed colour variants.
    pub fn pixels(&self) -> impl Iterator<Item = (usize, usize, u8)> + '_ {
        self.pixels
            .iter()
            .enumerate()
            .filter_map(|(i, &v)| Some((i % self.size.0, i / self.size.0, v?)))
    }

    /// The RGB colour stored in the variant's upper seven bits.
    #[must_use]
    pub fn colour(&self, variant: u8) -> Option<[u8; 3]> {
        self.colours.get(&(variant >> 1)).copied()
    }

    /// An icon sample from the same artwork, scaled and centred in an 8×8 slot.
    #[must_use]
    pub fn icon_pixel(&self, x: usize, y: usize) -> Option<[u8; 3]> {
        let side = self.size.0.max(self.size.1);
        let sample =
            |p: usize, len: usize| ((p * side + side / 2) / 8).checked_sub((side - len) / 2);
        let (x, y) = (sample(x, self.size.0)?, sample(y, self.size.1)?);
        if x >= self.size.0 || y >= self.size.1 {
            return None;
        }
        self.colour(self.pixels[y * self.size.0 + x]?)
    }
}

/// World origin and validity of an ornament's placement preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrnamentGhost {
    /// Rigid material whose artwork the preview shows.
    pub material: MaterialId,
    /// Top-left corner of the bitmap.
    pub origin: (usize, usize),
    /// Whether a click here can place the whole ornament.
    pub fits: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OrnamentDef {
    colours: BTreeMap<char, String>,
    sprite: Vec<String>,
    #[serde(default)]
    bubbler_outlets: Vec<[i16; 2]>,
}

impl OrnamentDef {
    pub(crate) fn validate(&self) -> Option<OrnamentArt> {
        let (w, h) = (self.sprite.first()?.len(), self.sprite.len());
        if w == 0 || w.checked_mul(h)? > MAX_CELLS {
            return None;
        }
        let mut colours = BTreeMap::new();
        for (&key, rgb) in &self.colours {
            if !key.is_ascii_graphic() || key == '.' {
                return None;
            }
            colours.insert(u8::try_from(key).ok()?, parse_colour(rgb)?);
        }
        let mut pixels = Vec::with_capacity(w * h);
        for row in &self.sprite {
            if row.len() != w || !row.is_ascii() {
                return None;
            }
            for c in row.bytes() {
                // Store the ASCII key itself so adding colours preserves saved variants.
                if c == b'.' {
                    pixels.push(None);
                } else if colours.contains_key(&c) {
                    pixels.push(Some(c << 1));
                } else {
                    return None;
                }
            }
        }
        // Tight frames keep the saved rigid reference origin aligned with the art.
        let solid = |x, y| pixels.get(y * w + x).is_some_and(Option::is_some);
        if !(0..w).any(|x| solid(x, 0))
            || !(0..w).any(|x| solid(x, h - 1))
            || !(0..h).any(|y| solid(0, y))
            || !(0..h).any(|y| solid(w - 1, y))
        {
            return None;
        }
        for (i, &[x, y]) in self.bubbler_outlets.iter().enumerate() {
            let x = usize::try_from(x).ok()?;
            let lip = usize::try_from(i32::from(y) + 1).ok()?;
            if x >= w
                || lip >= h
                || !solid(x, lip)
                || (lip > 0 && solid(x, lip - 1))
                || self.bubbler_outlets[..i].contains(&self.bubbler_outlets[i])
            {
                return None;
            }
        }
        Some(OrnamentArt {
            size: (w, h),
            pixels,
            colours,
            bubbler_outlets: self.bubbler_outlets.clone(),
        })
    }
}
