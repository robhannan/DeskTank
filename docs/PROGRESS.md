# Progress

## Status

M0 in progress: F1 workspace scaffold and CI done; all crates are stubs.

## Done

- Repository, licences (MIT OR Apache-2.0), README.
- Developer agent brief (`.claude/agents/developer.md`) and `CLAUDE.md`.
- Plan and spec (`docs/SPEC.md` v0.4): pixel-art falling-sand aquarium + pet sim; owner decisions logged in SPEC §11.
- F1 workspace scaffold + CI: six crate stubs (SPEC §4.1), `clap` CLI skeleton for every §4.3 subcommand, toolchain pinned at 1.100.0, workspace lints, size-tuned release profile, `deny.toml`, Dependabot, CI (fmt, clippy, test, doc on macOS/Windows/Ubuntu; `cargo deny` on Ubuntu).
- Decided: `Cargo.lock` committed; MSRV = pinned toolchain; `forbid(unsafe_code)` in sim and render (recorded in CLAUDE.md).
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
- Falling-sand engines such as The Powder Toy appear (inferred, unmeasured) to process every live particle each frame; DeskTank must use sleeping chunks + dirty rects and a sim tick below the render rate to meet its budgets.

## Next increments

- Work packages live in `docs/SPEC.md` §9; next up is P1 (packaging and identity spike), then M1.
- Capture mode for screenshots/recordings so PRs can show visuals, including a frame-sequence recorder for GIFs (SPEC T3).
