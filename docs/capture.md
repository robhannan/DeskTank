# Capture and render tests

`desktank capture` renders a scripted scene headlessly (offscreen, no window) to PNG files:

```sh
desktank capture --seed 7 --size 640x360 --frames 60 \
  --script crates/desktank/tests/scenes/starter.toml --out shots
```

| Flag | Default | Meaning |
|---|---|---|
| `--seed N` | 1 | World seed; the same seed and script give the same pixels. |
| `--time HH:MM` | none | Time of day. Accepted and validated, but has no effect until day and night exist. |
| `--size WxH` | `640x360` | Output size in pixels. The grid is drawn at the largest integer scale that fits, letterboxed. |
| `--frames K` | 1 | Frames to write, `frame_every` ticks apart. One frame is `frame.png`; several are `frame_0000.png`, `frame_0001.png`, … |
| `--script FILE` | none | Input script (below). Without one, an empty 160×90 tank. |
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
```

Pours due on tick `t` are applied just before that tick runs. Each `[[pour]]` entry is one stroke: the rigid cells (rock) of all its repeats form one body, held still until its last repeat and then free to sink. Scripts are parsed by `desktank_sim::Script`, which simulation tests can replay too.

## Render tests

`crates/desktank/tests/render.rs` captures each scene in `crates/desktank/tests/scenes/` at 320×180 (160×90 cells at scale 2, seed 7) and compares it with `crates/desktank/tests/reference/<scene>.png`. A scene passes when at most 0.5% of pixels differ by more than CIEDE2000 ΔE 2 (SPEC §7).

- Linux CI renders on Mesa lavapipe (software Vulkan), macOS CI on Metal and Windows CI on WARP. With `CI` set, a missing GPU adapter fails the test; locally it is skipped with a message.
- After a deliberate visual change, update the references and commit them: `DESKTANK_BLESS=1 cargo test -p desktank --test render`. Look at the new PNGs before committing.
- To add a scene, add `tests/scenes/<name>.toml`, a test calling `check("<name>")`, and bless it.
