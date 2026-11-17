# Progress

## Status

M0 in progress: F1 workspace scaffold and CI done. M1: T1 (grid, chunks, movement) done in `desktank-sim`; the other crates are stubs. WB0: the sim core runs in the browser on GitHub Pages; the web is now a product target (D15, SPEC §3.5).

## Done

- Repository, licences (MIT OR Apache-2.0), README.
- Developer agent brief (`.claude/agents/developer.md`) and `CLAUDE.md`.
- Plan and spec (`docs/SPEC.md` v0.4): pixel-art falling-sand aquarium + pet sim; owner decisions logged in SPEC §11.
- F1 workspace scaffold + CI: six crate stubs (SPEC §4.1), `clap` CLI skeleton for every §4.3 subcommand, toolchain pinned at 1.100.0, workspace lints, size-tuned release profile, `deny.toml`, Dependabot, CI (fmt, clippy, test, doc on macOS/Windows/Ubuntu; `cargo deny` on Ubuntu).
- Decided: `Cargo.lock` committed; MSRV = pinned toolchain; `forbid(unsafe_code)` in sim and render (recorded in CLAUDE.md).
- T1 grid, chunks and movement (`desktank-sim`): 4-byte cells, 64×64 chunks with sleep/wake and dirty rectangles, five movement classes, materials from embedded TOML (`assets/materials/`), fixed 1/20 s tick, per-chunk PCG streams, FNV-1a state hash, `set_cell`/`paint`/`fill_rect` edit API, PPM/ASCII debug snapshots (`cargo run --release -p desktank-sim --example snapshot`), criterion benches (`cargo bench -p desktank-sim`). Mechanisms the spec left open are recorded in SPEC §2.1 (*T1* notes).
- WB0 browser demo (`desktank-web` + `web/`): the sim core in WebAssembly on GitHub Pages (https://robhannan.github.io/DeskTank/). Pour/erase with a brush, play/pause/step/reset, seed, stats; 20 Hz fixed tick with rAF drawing that uploads a frame only when the world changed. Built by `scripts/build-web.mjs` (zx, `pnpm build:web`); CI builds it on every PR, `pages.yml` deploys from `main`.
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

## Next increments

- Web demo: run `wasm-opt` in CI (binaryen isn't on the runner; Ubuntu's apt package is old), saving about 60 KiB.
- Web demo: stop the rAF loop while paused and idle (it does no work then, but still wakes 60×/s) (PR #5 review).
- Web demo: keyboard navigation for the palette (arrow keys within the radio group) (PR #5 review).
- Work packages live in `docs/SPEC.md` §9; next up is P1 (packaging and identity spike), then T2.
- Surface levelling: a heavy pour can leave the water surface a few cells higher over the pour (terraces of one cell per 32 columns). A slow random-tick leveller (SPEC §2.1 random ticks) could flatten it fully.
- Displaced-liquid chains cross chunks within one tick; design the optional parallel pass (SPEC §4.2) around them before enabling it.
- Cap total cells (not just each side): 8192×8192 would allocate about 512 MB (PR #4 review).
- A differential sleep test (force-wake every chunk vs. asleep, tick once, compare) as a regression guard for missed wakes (PR #4 review).
- CI could run `cargo bench -p desktank-sim` and record the numbers (SPEC §7), without gating.
- Capture mode for screenshots/recordings so PRs can show visuals, including a frame-sequence recorder for GIFs (SPEC T3).
- Add `.gitattributes` (`* text=auto eol=lf`) to guard Windows `core.autocrlf` checkouts (PR #3 review).
- Dependabot: add `groups` / `open-pull-requests-limit` once real dependencies land (PR #3 review).
- Release symbolication: consider `strip = "debuginfo"` plus split debug info (dSYM/PDB) kept out of the release archive, so crash reports can be symbolicated (PR #3 review).
