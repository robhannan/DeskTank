# D19: Creature sprites have two texels per cell side

**Creature sprites have two texels per cell side** (2026-11-18). A creature's body (collision box) is half its sprite's size in texels, so sprite sides are even; palette icons are hand-drawn at one pixel per cell. Every target draws the grid at an **even integer scale** so each texel is a whole number of pixels; scale 1 is the fallback when 2 doesn't fit, and then shows one texel of each 2×2 block.
