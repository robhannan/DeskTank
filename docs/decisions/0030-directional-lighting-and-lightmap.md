# D30 · Directional lighting and 2D lightmap

DeskTank's lighting moves from a single tank-wide colour multiplier (`view.daylight`)
to an optional **2D coloured lightmap** with soft directional shadows
([#502](https://github.com/robhannan/DeskTank/issues/502)). The same lightmap
is designed to accept future placeable **lamp devices** (D27) without changing
the tank's fragment shaders.

**Why a lightmap, not inline raymarching.** `desktank-render` targets WebGL2
downlevel limits (no compute shaders or storage buffers; 16 sampled textures
per stage, 12 currently used) and must fit inside Chromium 68 SwiftShader's
4096-register cap (D26) and Windows DX12 FXC's shader limits. `fs_ground` has
only ~160 statements of headroom, so per-pixel shadow marching inside the grid
or sprite shaders is ruled out. Instead, lighting is precomputed into one
quarter-resolution `RGBA8Unorm` texture (`160×90` on a `640×360` grid, ~58 KiB)
bound at `@binding(13)` and read with a single bilinear lookup in `grid_post`
and `fs_sprite` (before blinker night-glow, leaving the HUD untouched).

**Channels and lamp readiness.** Rather than storing a one-channel sun shadow
mask, the lightmap stores accumulated light colour in `rgb` and direct-light
visibility in `a` (usable for future caustics or local shading):

- **Phase 1 (directional daylight):** `rgb = ambient + directional * visibility`,
  computed by a CPU sheared top-to-bottom opacity sweep using each material's
  existing `opacity_of`, water absorption and algae coat density. Rebuilding
  `160×90` texels on CPU takes ~20–40 µs and runs only when cells change or
  the light direction steps, adding no GPU render pass (important on SwiftShader
  and Android TV's Mali-G31) and zero work on settled frames.
- **Phase 2 (future placed lamps):** Each active lamp device accumulates its
  shadowed cone (`lamp.colour * cone_falloff * visibility`) into the same
  `RGBA8Unorm` lightmap. Whether a tank has zero lamps or several, the fragment
  shaders still perform the same single bilinear lookup. At night, a warm lamp
  naturally illuminates its cone while unlit regions stay dim blue moonlight.

**Render-only vs. simulation-coupled.** Global directional lighting and its
optional solar sweep stay **render-only**, matching `daylight::tint`:
`FieldKind::Light` in `desktank-sim` remains vertical, so toggling directional
lighting or sweeping the sun across the day never wakes chunks, changes plant
or algae ecology, or alters save hashes. When solar sweep is enabled, the light
angle steps in coarse quantized buckets derived from `DaylightSchedule` so a
settled tank does not redraw continuously. Future placed lamp devices live in
`desktank-sim` as deterministic world state and will feed both `FieldKind::Light`
(at 1/8 resolution, sharing the same 2D opacity-sweep math) and the renderer's
lightmap (at 1/4 resolution), so plant growth and rendered lamp cones match.

**Terrain bevels and sprites.** Existing neighbor-aware edge shading
(`stone_bevel`, sand, gravel and driftwood) shifts from hardcoded top-left
multipliers to weights driven by `view.light_dir`. Creature sprites have
hand-shaded pixel-art highlights baked into their frames (D19) and mirror
horizontally, so they *receive* the lightmap rather than being relit with
normal maps. Soft dynamic creature drop shadows (stamped into the lightmap or
drawn in an instanced pass respecting D21 depth bands) are deferred to a
follow-up increment.

**Overlay and OLED modes.** In transparent click-through overlay mode, shadows
in water respect `water_opacity()` so dark silhouettes are not painted over
desktop windows. In OLED mode, multiplicative lighting preserves `#000000`
water and air.
