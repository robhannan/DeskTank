# Progress

## Status

M0 in progress: F1 workspace scaffold and CI done. M1: T1 (grid, chunks, movement) T2 (renderer, `desktank window`, `desktank bench`) and T3 (`desktank capture`, render tests) done; the saver crates are stubs. WB0 and WB1: the web version runs the real renderer (WebGPU or WebGL2) on GitHub Pages; the web is a product target (D15, SPEC §3.5).

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
- References blessed on Metal (M3 Max); a 5% green cut in the fragment shader fails both scenes (28% and 36% of pixels over ΔE 2, max ΔE 7.9 and 9.8), so the tolerance catches small palette shifts.
- `desktank capture` (debug build) renders 60 frames of 640×360 in ~3.7 s.

## Next increments

- `desktank capture` leaves stale `frame_NNNN.png` files from an earlier, longer run in `--out`; consider a warning or `--clean` (PR #8 review).
- `Tank::capture` allocates a target and readback buffer per frame; reuse them for long frame sequences, and give the web its own async readback (it blocks on `poll`) (PR #8 review).
- `desktank capture --time` is validated but ignored until day and night (T-later) exist; the render tests should then add the 4 times of day.

- Web demo: run `wasm-opt` in CI (binaryen isn't on the runner; Ubuntu's apt package is old), saving about 60 KiB.
- Web demo: keyboard navigation for the palette (arrow keys within the radio group) (PR #5 review).
- Work packages live in `docs/SPEC.md` §9; next up is P1 (packaging and identity spike), T4 and WB2.
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
