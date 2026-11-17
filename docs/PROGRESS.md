# Progress

## Status

Repository created; no code yet.

## Done

- Repository, licences (MIT OR Apache-2.0), README.
- Developer agent brief (`.claude/agents/developer.md`) and `CLAUDE.md`.
- Plan and spec (`docs/SPEC.md` v0.4): pixel-art falling-sand aquarium + pet sim; owner decisions logged in SPEC §11.
- Research: The Powder Toy inspected with REA/Hopper (`docs/research/powder-toy.md`) — ideas only, GPL, no code copied.

## Learnings

- Lock screens are restricted: macOS allows no custom lock screen and Windows only a picture, slideshow or Spotlight (no live rendering), so "lock screen" mode may mean the screensaver shown while locked.
- Reported, unverified: Windows 11 24H2 broke several wallpaper apps (Microsoft compatibility hold, [The Register](https://www.theregister.com/2025/04/14/windows_11_24h2_hold/)); test wallpaper mode on 23H2/24H2/25H2.
- Reported, unverified: macOS `legacyScreenSaver` (Sonoma–Tahoe) never deallocates `ScreenSaverView`s ([cocoa-dev](https://lists.apple.com/archives/cocoa-dev/2024/Sep/msg00001.html)); see SPEC §3.4 for mitigations.
- GNOME's mutter has at most experimental wlr-layer-shell support, so Wayland wallpaper/overlay target wlroots compositors and KDE first.
- winit 0.31 is still in beta (Nov 2026); use 0.30.
- Falling-sand engines such as The Powder Toy appear (inferred, unmeasured) to process every live particle each frame; DeskTank must use sleeping chunks + dirty rects and a sim tick below the render rate to meet its budgets.

## Next increments

- Work packages live in `docs/SPEC.md` §9; next up is F1 (workspace scaffold + CI).
- Capture mode for screenshots/recordings so PRs can show visuals, including a frame-sequence recorder for GIFs (SPEC T3).
- Decide on Cargo.lock policy (commit it: binary), an MSRV, and `#![forbid(unsafe_code)]` in the core crate; record them in CLAUDE.md.
