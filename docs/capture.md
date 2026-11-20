# Capture and render tests

`desktank capture` renders a scripted scene headlessly (offscreen, no window) to PNG files:

```sh
desktank capture --seed 7 --size 640x360 --frames 60 \
  --script crates/desktank/tests/scenes/starter.toml --out shots
```

| Flag | Default | Meaning |
|---|---|---|
| `--seed N` | 1 | World seed; the same seed and script give the same pixels. |
| `--time HH:MM` | 12:00 | Local time of day the tank is lit for (day and night, `desktank_sim::daylight`): 07:30–19:30 is untinted day, with warm dawn (05:30–07:30) and dusk (19:30–21:30) and blue moonlight overnight. Lighting only: the simulation is the same at every hour. |
| `--size WxH` | `640x360` | Output size in pixels. The grid is drawn at the largest integer scale that fits, letterboxed. |
| `--frames K` | 1 | Frames to write, `frame_every` ticks apart. One frame is `frame.png`; several are `frame_0000.png`, `frame_0001.png`, … |
| `--script FILE` | none | Input script (below). Without one, an empty 160×90 tank. |
| `--field F` | none | Debug overlay: tint the tank by the `nutrient`, `co2` or `light` field, or show downstream arrows with `flow`. |
| `--out DIR` | required | Output directory, created if missing. |

A GIF from a frame sequence:

```sh
ffmpeg -framerate 10 -i shots/frame_%04d.png -vf "split[a][b];[a]palettegen[p];[b][p]paletteuse" out.gif
```

## Script format

A script is TOML; every key is optional and unknown keys are an error. The world ticks at 20 Hz (one tick is 1/20 s of world time).

```toml
ticks = 200        # ticks to run before the first frame (default 0)
frame_every = 2    # ticks between frames when capturing several (default 1)

[world]
width = 160        # grid size in cells (default 160×90)
height = 90
water_level = 0.96 # fraction of the height, from the floor (default 0.96)
starter = true     # lay out the seeded starter scene first (default false)

[[pour]]           # any number of these
tick = 0           # first tick to pour on (default 0)
repeat = 40        # pour on this many consecutive ticks (default 1)
x = 80             # disc centre, in cells from the top left
y = 10
dx = 1             # move the centre this many cells per repeat (default 0)
radius = 3         # disc radius in cells (default 2)
material = "sand"  # a material name from assets/materials
brush = "pour"     # "pour" fills only fluid cells; "replace" overwrites (default "pour")
glued = true       # glue the rigid body this pour makes (rock), so it never moves (default false)
# plant = "rotala" # instead of a material: pour this plant species' seeds or cuttings
                   # (assets/plants), as the palette's plants slot does (T6b)

[[place]]          # any number of these: one device each
tick = 0           # tick to place on (default 0)
x = 20             # footprint centre, in cells from the top left
y = 60
device = "bubbler" # "bubbler" (drops onto what is below) or "filter_outlet"

[[water_change]]   # any number of these
tick = 36000       # tick to change the water on (`World::water_change`)

[[creature]]       # any number of these (T7a)
tick = 0           # tick to add them on (default 0)
species = "neon_tetra" # a species name from assets/species
x = 40             # body centre, in cells from the top left (the nearest free spot)
y = 30
count = 6          # how many (default 1)
dx = 3             # move the centre this many cells per creature (default 0)
```

Water changes, placements, pours and creatures due on tick `t` are applied, in that order, just before that tick runs. Each `[[pour]]` entry is one stroke: the rigid cells (rock) of all its repeats form one body, held still until its last repeat and then free to sink. Scripts are parsed by `desktank_sim::Script`, which simulation tests can replay too.

## Render tests

`crates/desktank/tests/render.rs` captures each scene in `crates/desktank/tests/scenes/` at 320×180 (160×90 cells at scale 2, seed 7) and compares it with `crates/desktank/tests/reference/<scene>.png`. A scene passes when at most 0.5% of pixels differ by more than CIEDE2000 ΔE 2 (SPEC §7).

- Linux CI renders on Mesa lavapipe (software Vulkan), macOS CI on Metal and Windows CI on WARP. With `CI` set, a missing GPU adapter fails the test; locally it is skipped with a message.
- After a deliberate visual change, update the references and commit them: `DESKTANK_BLESS=1 cargo test -p desktank --test render`. Look at the new PNGs before committing.
- To add a scene, add `tests/scenes/<name>.toml`, a test calling `check("<name>")`, and bless it. `check_at("<name>", "HH:MM")` captures it lit for a time of day against `<name>_HHMM.png`; the `planted` scene (T6: cuttings, seeds and moss grown for 6 minutes; from T6b each is a plant of a species) is checked at 00:00, 06:30, 12:00 and 20:30. The `creatures` scene captures a tetra school, bottom-walking corydoras, surfacing bettas and a nerite grazing a rock after 30 s. The `climbing` scene shows nerites turned onto a rock pillar, upside down under its arm and on both panes of glass. The `invertebrates` scene shows cherry and amano shrimp grazing an algae film on an aquasoil bed and Thai micro crabs on the plants and rock. The `fish` scene shows a harlequin rasbora school, male guppies and honey gouramis coming up for flakes, and otocinclus on a rock, the glass and the bed.
