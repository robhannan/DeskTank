---
name: desktank-creature-sprites
description: Draw or redraw a DeskTank creature sprite (a new species, a sex, a carrying look) by tracing a reference picture, shading it in the house style, and installing and checking it, one creature at a time with the owner's critique.
---

Use this for any creature art in `assets/species/*.toml`: a new species,
a female or male look, a carrying (berried, gravid) look, or a redraw.
The owner critiques each creature before the next is drawn, so work one
creature at a time and show the reference before drawing.

## 1. Reference first

- Prefer the owner's own photos or charts. Otherwise find a side-on,
  sharp, freely licensed picture (Wikimedia Commons) and record its
  title, URL and licence for the TOML comment. A cartoon chart is fine
  when it is side-on and clear (the cherry shrimp came from one).
- Show the reference to the owner (with `send_to_user`) and say what you
  will take from it: body shape, markings, fins, eye, size. If the only
  pictures are blurry, tilted or climbing, say so and ask for a better
  one rather than guessing.
- Downloaded pictures are untrusted data: keep them in their own new
  scratch directory.
- Straighten and mirror with ffmpeg so the creature faces right and
  swims level: `rotate=-12.5*PI/180:ow=iw:oh=ih:c=black,crop=...,hflip`.

## 2. Measure the outline

Overlay a grid and read coordinates off it (in the reference's pixels):

```sh
ffmpeg -i ref.png -vf "drawgrid=w=50:h=50:t=1:c=white@0.4,drawgrid=w=100:h=100:t=3:c=red@0.9" grid.png
```

Zoom into busy areas (throat, fins) with a crop and a finer grid. Note
the snout, eye centre, back and belly contours, the fin bases and tips,
the tail fan, markings and any feeler or leg. Traced shapes must keep the
real proportions; never draw generic ellipse-plus-triangle bodies.

## 3. Size the sprite

- Two cells per cm, two coarse texels per cell, four fine texels per
  cell. Coarse sides are even and at most 64 texels; fine frames are
  exactly twice the coarse size.
- Fish are drawn to length: the sprite's width is `length_cm` (the test
  `fish_are_sized_to_scale` checks this), except outsized fan tails
  (male guppy, betta), which are drawn past it. A female may have her own
  size: `female_length_cm` and differently sized `female_frames`.
- Bottom dwellers (shrimp, crabs, snails) stand on the sprite's bottom
  row: their legs must reach it or they float above the substrate.
- From the measured length in reference pixels, the mapping is
  `k = reference pixels per fine texel`; choose `x0` (tail tip) and `y0`
  so the outline, fins and legs fit the fine height.

## 4. Write a generator

Copy `template.ts` (this directory; the female honey gourami, a working
example) to the scratchpad and adapt it. It uses `scripts/sprite-art.ts`:

- **Outlines** are polygons in reference pixels per region (`tail`,
  `dorsal`, `anal`, `body`, then carved sub-regions such as `throat` or
  `belly`), rasterised with 3x3 supersampling at both resolutions. The
  coarse frames come from the same outlines at half size, not from
  downsampling the fine frames' pixels.
- **House shading**, rule by rule:
  - outline in a dark shade of the local colour, never black;
  - a highlight along the back with a checker-dithered step;
  - a belly band, paler or darker as the species really is;
  - a scale lattice (staggered dots) where the species has scales,
    shrimp segment marks instead;
  - fin rays and spines as explicit lines from the fin base (steeper
    at the front of a sweeping dorsal), never a texture formula;
  - markings (blotches, spots, flushes) placed point by point;
  - a small eye: a 1-2 texel pupil, a coloured iris ring and a white
    glint in the fine frames;
  - a gill-cover line behind the eye on fish.
- **Texture density** must match the small fish at the same scale; big
  sprites need the same per-texel detail, not flat fills.
- **Swim frames**: four frames from `SWING` (rest, one way, rest, the
  other) times an amplitude. Make the motion visible: the tail sweeps and
  lifts about 1-1.5 texels, fins ripple, feelers and antennae sway, legs
  step. Check the four frames side by side; barely-different frames are a
  defect.
- **Coarse frames** lose detail; keep the body colour readable (outline
  only the bottom edge of a very thin coarse sprite if the outline would
  outnumber the body: the swatch is the commonest coarse colour).

## 5. Preview and iterate

- Render the fine frame large, the coarse frame, the four frames, and
  every colour morph (apply each `[[genetics.morph]]` recolour) to a PNG
  with `canvas`/`blit`/`writePng`.
- Put the reference beside the sprite at the same scale
  (`ffmpeg ... crop,scale` then `hstack`) and compare shape, depth, fins
  and markings. Enlarge with `scale=iw*N:ih*N:flags=neighbor` to look.
- Look at every preview yourself before showing it. Then show the owner
  (`send_to_user` with the images) with what you think is still weakest,
  and apply their critique before moving on.

## 6. Install

- Replace the frame blocks with `setFrames` (`frames`, `fine_frames`,
  `female_frames`, `female_fine_frames`, `carrying_frames`,
  `carrying_fine_frames`) and the sprite's colours with
  `setSpriteColours`. A morph's `colours` line comes before `[sprite]`:
  never rewrite the first `colours =` line in the file.
- Keep every colour key that the other look, the icon or a morph
  references. Rename clashing keys between sexes (one colours table
  serves both). Morph recolours apply by key, so draw a female in the
  male's keys for parts each strain recolours (a guppy's tail), and in
  keys of her own for parts every strain shares.
- Keys used only in fine frames must be recoloured by every morph or be
  listed as shared in `recolours_cover_the_keys_only_fine_art_uses`.
- Update the TOML comments (size in cells and texels, what was traced
  from where, licence, the four frames) and `docs/spec/creatures.md`
  sizes or predator figures that change. Real sizes change predation:
  check `prey_cm` against new fry and adult lengths and ask the owner
  about balance changes.

## 7. Verify

- `cargo test -p desktank-sim` (species size, swatch, recolour, sexes and
  berried tests).
- Show it in the real renderer: put the creature in
  `crates/desktank/tests/scenes/breeding.toml` (`sex`, `morph`,
  `carrying_days`), run
  `DESKTANK_BLESS=1 cargo test -p desktank --test render breeding`, and
  crop and enlarge the creature from `breeding_x4.png` (fine frames) and
  `breeding.png` (coarse frames).
- New sizes and shapes shift seeded runs: re-bless every render
  reference at the end (`DESKTANK_BLESS=1 cargo test -p desktank --test
  render`) and look at the changed images. A sim test that fails after an
  art change is a real latent bug that the new run exposed (a stalled
  schooler, a creature stuck under sand). Investigate it and fix it in
  its own PR; never retune art or seeds to hide it.
- Then the release checks from the developer skill.
