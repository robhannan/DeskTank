# Progress

## Status

M0 in progress: F1 workspace scaffold and CI done. M1: T1 (grid, chunks, movement), T2 (renderer, `desktank window`, `desktank bench`), T3 (`desktank capture`, render tests), T4 (window-mode interaction) T4b (rigid bodies and glue, D17) and T5 (materials, reactions and fields) done; the saver crates are stubs. WB0 and WB1: the web version runs the real renderer (WebGPU or WebGL2) on GitHub Pages; the web is a product target (D15, SPEC §3.5).

## Done

- Repository, licences (MIT OR Apache-2.0), README.
- Developer agent brief (`.claude/agents/developer.md`) and `CLAUDE.md`.
- Plan and spec (`docs/SPEC.md` v0.4): pixel-art falling-sand aquarium + pet sim; owner decisions logged in SPEC §11.
- F1 workspace scaffold + CI: six crate stubs (SPEC §4.1), `clap` CLI skeleton for every §4.3 subcommand, toolchain pinned at 1.100.0, workspace lints, size-tuned release profile, `deny.toml`, Dependabot, CI (fmt, clippy, test, doc on macOS/Windows/Ubuntu; `cargo deny` on Ubuntu).
- Decided: `Cargo.lock` committed; MSRV = pinned toolchain; `forbid(unsafe_code)` in sim and render (recorded in CLAUDE.md).
- T1 grid, chunks and movement (`desktank-sim`): 4-byte cells, 64×64 chunks with sleep/wake and dirty rectangles, five movement classes, materials from embedded TOML (`assets/materials/`), fixed 1/20 s tick, per-chunk PCG streams, FNV-1a state hash, `set_cell`/`paint`/`fill_rect` edit API, PPM/ASCII debug snapshots (`cargo run --release -p desktank-sim --example snapshot`), criterion benches (`cargo bench -p desktank-sim`). Mechanisms the spec left open are recorded in SPEC §2.1 (*T1* notes).
- WB0 browser demo (`desktank-web` + `web/`): the sim core in WebAssembly on GitHub Pages (https://robhannan.github.io/DeskTank/). Pour/erase with a brush, play/pause/step/reset, seed, stats; 20 Hz fixed tick with rAF drawing that uploads a frame only when the world changed. Built by `scripts/build-web.mjs` (zx, `pnpm build:web`); CI builds it on every PR, `pages.yml` deploys from `main`.
- T2 renderer and window (`desktank-render`, `desktank`): loop-agnostic `Tank` (`tick(now) -> FrameRequest`, `draw`, `render_to`) with a fixed 20 Hz tick, a 30 fps cap and frames only when the world changed; dirty rectangles (merged per chunk across ticks) uploaded to an `Rg8Unorm` texture; a palette shader (256×256 material×variant table, integer scale, letterbox, water depth tint) within WebGL2 limits (device opened with `downlevel_webgl2_defaults`; CI runs clippy for wasm32). `desktank window` (winit 0.30) sleeps with `WaitUntil`, pauses on occlusion/minimise with no timers, and has a debug brush (left pour, right erase, keys 1–5). `desktank bench [--seconds N]` runs a seeded pour then settle offscreen at 2560×1440 and prints CPU, tick time, awake chunks, GPU time (timestamp queries) and RSS. Process CPU/RSS readers in `desktank-platform`. An offscreen GPU smoke test reads pixels back (skips without an adapter).
- T3 capture and render tests: `desktank capture` renders a seeded TOML input script (`desktank_sim::Script`: timed pours, run N ticks, frames every M ticks) offscreen to PNG (`Tank::capture` reads pixels back; `Tank::step` ticks outside real-time pacing). Render tests capture two scenes at 320×180 and compare them with references in `crates/desktank/tests/reference/` (≤ 0.5% of pixels over CIEDE2000 ΔE 2); `DESKTANK_BLESS=1` updates them. Linux CI runs them on lavapipe; GPU tests fail instead of skipping when `CI` is set. Docs: `docs/capture.md`.
- WB1 web host on the real renderer: `desktank-web` drives `desktank-render`'s `Tank` through wgpu (`WebTank` for the world, pacing and edits, tested natively; `Graphics` for the device and canvas surface). WebGPU first, WebGL2 if there is no adapter or the device fails, `?backend=webgpu|webgl2` to force one; WebGL context loss and WebGPU device loss drop the graphics and reopen them, keeping the world. 20 Hz tick, frames only when something changed, capped at 30 fps (15 on battery via the Battery Status API), paused while the page is hidden, rAF stopped while paused and idle. Grid fixed at load with the largest integer scale giving ≥ 320 cells across (2048 max side), letterboxed on resize. `scripts/build-web.mjs` reports the gzip transfer size and fails above 3 MB; Playwright tests (`tests/web/`, `pnpm test:web`, in CI) cover pouring on WebGL2 and the default backend, context-loss recovery and a WebGL2 render of the `layers` scene against the native reference (ΔE2000 ported to JS); `pnpm bench:web` measures frame intervals.
- T4 window-mode interaction: platform-free tools in `desktank_sim::tools` (pour, scoop, net, inspect; brush radius 0–24 on the wheel; Bresenham stroke interpolation so fast drags leave no gaps; holding still re-pours once per tick), pixel→cell and palette hit tests in `desktank-render` (`Layout`, `PaletteLayout`), and a pixel-art palette strip plus brush outline drawn by the tank shader (one extra 256×16 texture, uniforms only; redraws only when the selection or outline changes). `desktank window`: left drag uses the tool, right drag scoops, click the palette or keys 1–9; inspect shows the material in the title bar. `desktank window --demo-pour N` scripts a drag and prints CPU. Net and inspect wait for creatures (T7).
- T4b rigid bodies and glue (D17): a `rigid` movement class (rock); each pour or stroke of rock is one body (cells carry the body id in `state`, a `BTreeMap` body table in `World`) that sinks a row per tick through lighter fluids, or rises through heavier ones, as one piece, resting on powders, other bodies and the floor. Scooped bodies split into 4-connected pieces at the next tick. A body being poured into is held still until the stroke ends. A glue tool (palette slot 10, key 0 in `desktank window`) toggles bodies static; glued cells carry a variant bit the palette shades amber. Input scripts gained `glued = true`, and `Script::player()` keeps one body per `[[pour]]`. The web demo pours one body per drag. Tests in `crates/desktank-sim/tests/bodies.rs`; the `layers` reference was re-blessed (its rock ledge is now glued, so it shows the tint instead of sinking).
- T5 materials, reactions and fields: 12 new materials (aquasoil, rigid driftwood that floats, sinking pellets, bloodworms, algae wafers, root tabs, liquid fertiliser, CO₂ bubbles, waste, detritus, and the bubbler and filter-outlet device cells); the reaction table and slow processes in `assets/reactions.toml` (`desktank_sim::ecology`: `[[reaction]]` with field gates and releases, `[[decay]]`, `[[emit]]`, `[[buffer]]`); nutrient, CO₂ and light fields at 1/8 resolution stepped every 4th tick (`desktank_sim::field`, constants in `assets/fields.toml`); the random-tick scheduler (per-chunk living lists refreshed from changed rects, a rotating window of 256 visits per tick); placeables (`Placeable`, `World::place`, a palette slot each, `[[place]]` in scripts): a bubbler that keeps the water level and a filter outlet whose jet carries floating cells and non-water liquids. Debug field tint: `F` in `desktank window`, `desktank capture --field nutrient|co2|light`. The window palette keeps keys 1–9, 0 (glue) and appends the new materials and devices; the web demo lists them too. Mechanisms in SPEC §2.1 (*T5* notes, Fields, Devices) and §2.2. Tests: `crates/desktank-sim/tests/ecology.rs` (each rule, diffusion rate, detritus timing, current, bubbler, budget, sleep, light, determinism, driftwood) plus unit tests.
- Research: The Powder Toy inspected with REA/Hopper (`docs/research/powder-toy.md`) — ideas only, GPL, no code copied.

## Learnings

- Lock screens are restricted: macOS allows no custom lock screen and Windows only a picture, slideshow or Spotlight (no live rendering), so "lock screen" mode may mean the screensaver shown while locked.
- Reported, unverified: Windows 11 24H2 broke several wallpaper apps (Microsoft compatibility hold, [The Register](https://www.theregister.com/2025/04/14/windows_11_24h2_hold/)); test wallpaper mode on 23H2/24H2/25H2.
- Reported, unverified: macOS `legacyScreenSaver` (Sonoma–Tahoe) never deallocates `ScreenSaverView`s ([cocoa-dev](https://lists.apple.com/archives/cocoa-dev/2024/Sep/msg00001.html)); see SPEC §3.4 for mitigations.
- GNOME's mutter has at most experimental wlr-layer-shell support, so Wayland wallpaper/overlay target wlroots compositors and KDE first.
- Cargo 1.100 warns on unused dependencies (`cargo::unused_dependencies`). Stub crates keep their SPEC §4.1 dependency edges with `use <crate> as _;`; drop those lines once the crate is really used.
- Dependabot labels 0.x bumps such as winit 0.30 → 0.31 as "minor", so the winit ignore covers semver-minor as well as major.
- Release profile uses `panic = "abort"` (smaller binaries, and a panic across the `.saver`'s `extern "C"` boundary aborts anyway). Revisit if a host ever needs `catch_unwind` to survive a panic.
- Stub release binaries on macOS: `desktank` 0.5 MB, `desktank-saver` 0.3 MB.
- winit 0.31 is still in beta (Nov 2026); use 0.30.
- T1 bench on the dev Mac (Apple Silicon, release): heavy pour 1.43 ms/tick early, 1.56 ms later (budget 5 ms); settled 4 µs (budget 0.5 ms). The pour settles after ~3100 ticks (155 s of world time).
- If two classes can each initiate the same exchange (water falling into a bubble, the bubble rising into water), the pair moves twice per tick and drifts sideways. Only liquids are displaceable; buoyant cells move themselves.
- A liquid that wanders sideways with no drop in reach never settles (chunks stay awake forever). Liquids only move towards a reachable drop, which leaves terraces of one cell per `LEVEL_REACH` columns.
- Diagonal-only powder slides give 45° piles; a per-row run pattern derived from `repose` gives any angle from 15° to 45° exactly.
- Falling-sand engines such as The Powder Toy appear (inferred, unmeasured) to process every live particle each frame; DeskTank must use sleeping chunks + dirty rects and a sim tick below the render rate to meet its budgets.

- wasm-bindgen's generated `unsafe` doesn't trip the workspace `unsafe_code = "deny"` lint (it comes from an external proc macro), so `desktank-web` needs no allow; CI runs clippy for `wasm32` to catch a change.
- The release profile's `strip = true` removes the `target_features` section wasm-bindgen reads, so the web build overrides it with `strip = "debuginfo"`. Demo wasm: 280 KiB, 218 KiB after `wasm-opt -Oz`.
- In the demo (Apple Silicon, Chrome, release wasm) a 320×180 pour ticks in about 1.5 ms; settled ticks about 0.1 ms.

- T2 numbers on the dev Mac (M3 Max, release; the SPEC reference is an M1 Air): `desktank bench` settled CPU 0.19% of a core, settled tick 0.027 ms, pour CPU 5.4%, pour tick 0.65 ms mean, GPU 0.03 ms mean / 0.16 ms p95 at 2560×1440, RSS 21 MB (offscreen). `desktank window` settled: 0.25% CPU over 60 s; hidden 60 s: 0.00 s CPU, 1 wake-up per 20 s. Window memory: footprint 53 MB (Activity Monitor), but `ps` RSS 90 MB because it counts ~40 MB of clean shared framework pages; the swapchain's IOSurfaces are 29 MB of the footprint.
- wgpu 30: `SurfaceConfiguration` gained `color_space` (use `Auto`), `get_current_texture` returns a `CurrentSurfaceTexture` enum, and `InstanceDescriptor` takes the winit `OwnedDisplayHandle` (needed for GLES on Wayland). Timestamp queries work on Apple Silicon Metal.
- winit 0.30 reports `Occluded` on macOS and X11 but not on Windows or Wayland; Windows minimise shows up as a zero size, and `Window::is_minimized` is polled in `about_to_wait` as a fallback. A window fully covered on Windows, or hidden on Wayland, isn't detected yet.
- Settled ticks cost ~25 µs because the loop still wakes at 20 Hz to tick world time; with zero awake chunks the tick could be skipped or batched once creatures exist.

- WB1 numbers (M3 Max, 120 Hz display, DPR 2, 1884×1060 canvas, 376×212 cells, 60 s of pouring): Chrome 157 (WebGPU) rAF interval p50 8.3 ms / p95 9.1 ms / max 49.6 ms, drawn frames p50 50.0 / p95 50.7 ms; Playwright WebKit 27.2 (WebGPU) rAF p50 17.0 / p95 18.0 / max 58 ms, drawn p95 51 ms. Playwright's Firefox wouldn't launch in the agent sandbox, so Firefox and real Safari are unmeasured.
- Transfer size: 1.07 MB gzip with `wasm-opt` locally, 1.17 MB in CI without it (wasm 2.7 MB / 3.8 MB raw: wgpu-core, naga's GLSL backend for WebGL2, and the sim).
- The renderer used to shade (water depth dim) in linear space on sRGB targets and in encoded space on `Rgba8Unorm` captures, so a window or WebGL2 canvas didn't match `desktank capture` (49% of pixels over ΔE 2). It now shades in encoded space everywhere and encodes for sRGB targets in the shader; the water in the native window is a little lighter at depth than before.
- A canvas keeps the first context type it hands out, so the WebGPU attempt opens the adapter and device before creating the surface, leaving the canvas free for the WebGL2 fallback. wgpu's default uncaptured-error handler panics (aborting the wasm module); the web host logs instead.
- Headless Chromium under DPR emulation reports CSS pixels in `devicePixelContentBoxSize`; the page sizes the canvas from the content box × `devicePixelRatio` instead.
- Headless Chromium on the Linux runner draws WebGL2 on SwiftShader only with `--enable-unsafe-swiftshader`; it matches the Metal reference to max ΔE 0.78.
- T4 numbers (M3 Max, release, 1280×720 pt window at 2560×1440 px): `desktank window --demo-pour 10` used 8.1% of a core over a 10 s continuous pour at the default radius 3 (budget 15%), and 16.5% at the maximum radius 24 (a 49-cell-wide sweep moving far more sand); 0.85% over the next 20 s while the sand settled. RSS 93 MB (as T2).
- Synthetic mouse events (`CGEventPost`) need Accessibility permission for the posting process, which agent shells don't have, so real-input checks in `desktank window` are manual; `--demo-pour` drives the same code paths instead.
- References blessed on Metal (M3 Max); a 5% green cut in the fragment shader fails both scenes (28% and 36% of pixels over ΔE 2, max ΔE 7.9 and 9.8), so the tolerance catches small palette shifts.
- `desktank capture` (debug build) renders 60 frames of 640×360 in ~3.7 s.
- T4b bench (M3 Max): heavy pour 1.43 / 1.55 ms and settled 3.4 µs per tick (unchanged from T1); a radius-12 rock sinking through water 37 µs per tick; a settled tank with 13 resting rocks 3.5 µs.
- A rigid body's glued flag lives in the low bit of the variant byte so the renderer needs no third texture channel: every palette shade range starts on an even byte, so the bit never changes a cell's shade.
- T5 bench (M3 Max, criterion): heavy pour 1.48 / 1.62 ms per tick (was 1.43 / 1.55 ms on main: the living-list rescan of changed rects), settled 4.2 µs, settled with bodies 4.2 µs, and a settled tank with fields and random ticks active (a 640×12 aquasoil bed and 20 root tabs: 256 visits per tick, every chunk asleep) 6.1 µs, against the 0.5 ms budget. `desktank window --demo-pour 3` 3.8% of a core.
- Random ticks sampled with replacement made decay times spread like a geometric sum; a rotating window over the living lists (each cell visited every ⌈living/budget⌉ ticks) keeps every detritus cell within ±10% of its time even at 12× the budget.
- A current that lets water swap with water is a symmetric random walk for anything carried in it: dye didn't drift at all. Water is the medium and only fills in behind carried cells, which also means a jet through clear water costs no wakes.
- Two seeds of the fertiliser dissolve test differed by 3σ from the mean with 240 cells; acceptance tests on rates use ≥ 960 cells.
- A bubbler that only turns water into bubbles drains the tank (a bubble pops into air); returning the first air cell above each release to water keeps the level exact, at the cost of a small mound over the bubbler.
- A sand pillar can't hold up anything for long: it slumps to its angle of repose. Tests that need a support use a glued rock.

## Next increments

- T4b: splits apply at the next tick, so gluing in the same frame as a cut toggles the whole pre-cut body. Harmless with real input (a tick passes between strokes); a script or WB2 sequence should tick between them.
- T4b: bodies move one row per tick whatever their density; a density-derived rate (driftwood rising slowly) is still open (T5 made driftwood rigid with density 0.7, so it rises a row per tick).
- T5: fields are not saved yet (T8 must store nutrient and CO₂, which the state hash includes) and their f32 maths is only bit-identical across IEEE platforms without fused multiply-add.
- T5: nutrients diffuse through substrate and rock as through water; blocking or slowing diffusion in solids is an option for T6 roots.
- T5: the current is horizontal only and doesn't carry bubbles or sinking powders; outlets can't be aimed.
- T5: the web demo has no field overlay toggle (the renderer supports it); add one with WB2.
- T5: the bubbler's surface mound and the T1 surface terraces would both flatten with a random-tick surface leveller.
- T5: reaction keep-probabilities use `powf` at load time, which could differ in the last ULP between native and wasm; a deterministic routine or checked-in table would make trajectories identical across targets (PR #12 review).
- T5: tests for the random-tick rotation (each living cell visited once per ⌈living/budget⌉ ticks), the web page's own `TOOLS` list against the sim's materials, and wasm determinism (PR #12 review).
- T5: a refresh moves a chunk's touched living entries to the end of its list, so a rotation can visit some cells twice and skip others; ages stay within ±10% in tests, but keeping list order stable would make visits exact (PR #12 review).
- T5: the field rest threshold leaves a tiny CO₂ residual that never drains, and `Field::step` copies the whole grid even when only a patch moves (PR #12 review).
- T5: the web palette now has 19 swatches and wraps below the canvas on small screens; WB2's palette should be compact or scroll sideways.
- T4b: bodies don't move sideways, slide off slopes or rotate; a body resting on a single grain stays put. A tipping or sliding rule is a later option.
- T4b: the web demo has no glue tool yet; WB2 moves the web onto `desktank_sim::tools`, which brings it.
- T4b: save files (T8) can rebuild the body table from the grid (id in `state`, glued in the variant bit); the next id counter should be saved or recomputed.

- `desktank capture` leaves stale `frame_NNNN.png` files from an earlier, longer run in `--out`; consider a warning or `--clean` (PR #8 review).
- `Tank::capture` allocates a target and readback buffer per frame; reuse them for long frame sequences, and give the web its own async readback (it blocks on `poll`) (PR #8 review).
- `desktank capture --time` is validated but ignored until day and night (T-later) exist; the render tests should then add the 4 times of day.

- Web demo: run `wasm-opt` in CI (binaryen isn't on the runner; Ubuntu's apt package is old), saving about 60 KiB.
- Web demo: keyboard navigation for the palette (arrow keys within the radio group) (PR #5 review).
- Work packages live in `docs/SPEC.md` §9; next up is P1 (packaging and identity spike), WB2 (web interaction on `desktank_sim::tools`) and T5.
- T4: a drawn inspect card (pixel font) instead of the title bar, and the net carrying creatures, with T7.
- T4: scoop turns everything (rock, gravel) into water below the line, while pour never overwrites solids; decide whether scoop should spare rock before T7 / WB2 (PR #9 review).
- T4: a drag ends when it crosses the palette or leaves the window; keeping the stroke and resuming on re-entry is an option (PR #9 review).
- T4: at brush radius 24 a 10 s sweep uses 16.5% of a core, just over the 15% budget; profile whether the sim (much more falling sand) or the stamping dominates, or cap the radius lower.
- T4: palette hover highlight and tooltips; the palette is a single strip of at most 23 slots (256-px texture).
- Web: while running and settled, the rAF loop still wakes every vsync to call `frame` (cheap, but 60–120 wake-ups/s); it could sleep until `wake_at` with a timer. Measure page CPU in Chrome's task manager (not measured in WB1).
- Web: measure frame intervals in Firefox and real Safari (WB1 could only drive Chrome and Playwright WebKit).
- Web: reset reopens the GPU graphics for the new tank; a `WebTank` could hand its graphics over instead.
- Web: after WebGPU device loss, recovery reopens WebGPU only; falling back to WebGL2 then needs a fresh canvas element, since a canvas keeps its first context type (PR #10 review).
- Web: a WebGL2 context that is lost and never restored leaves the tank frozen with no message; show one after a timeout (PR #10 review).
- Web: while no graphics are attached (failed open) a running tank still wakes every vsync, and a persistent draw `Validation` error logs on every frame; back off both (PR #10 review).
- Web tests: replace the fixed `waitForTimeout` sleeps in `tests/web/smoke.spec.mjs` with polled conditions if they ever flake (PR #10 review).
- Pause detection gaps (T2): Windows fully-covered windows (DWM cloaking / present status) and Wayland (frame callbacks); winit 0.30 has no `Occluded` there.
- Resizing the window rescales the fixed grid with letterboxing; resizing the world itself (or choosing the grid from the display, SPEC §2.1) comes with the display modes.
- `desktank bench` could add a run without GPU timing, so per-frame CPU excludes the timestamp readback (PR #7 review).
- Decide whether the §6.1 memory budget means RSS or physical footprint on macOS (the window is 90 MB RSS, 53 MB footprint).
- Surface levelling: a heavy pour can leave the water surface a few cells higher over the pour (terraces of one cell per 32 columns). A slow random-tick leveller (SPEC §2.1 random ticks) could flatten it fully.
- Displaced-liquid chains cross chunks within one tick; design the optional parallel pass (SPEC §4.2) around them before enabling it.
- Cap total cells (not just each side): 8192×8192 would allocate about 512 MB (PR #4 review).
- A differential sleep test (force-wake every chunk vs. asleep, tick once, compare) as a regression guard for missed wakes (PR #4 review).
- CI could run `cargo bench -p desktank-sim` and record the numbers (SPEC §7), without gating.
- Add `.gitattributes` (`* text=auto eol=lf`) to guard Windows `core.autocrlf` checkouts (PR #3 review).
- Dependabot: add `groups` / `open-pull-requests-limit` once real dependencies land (PR #3 review).
- Release symbolication: consider `strip = "debuginfo"` plus split debug info (dSYM/PDB) kept out of the release archive, so crash reports can be symbolicated (PR #3 review).
