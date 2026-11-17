# Progress

## Status

Repository created; no code yet.

## Done

- Repository, licences (MIT OR Apache-2.0), README.
- Developer agent brief (`.claude/agents/developer.md`) and `CLAUDE.md`.
- Plan and spec draft (`docs/SPEC.md`), with open questions for the owner.

## Learnings

- Lock screens are restricted: macOS allows no custom lock screen and Windows only a picture, slideshow or Spotlight (no live rendering), so "lock screen" mode may mean the screensaver shown while locked.
- Windows 11 24H2 broke several wallpaper apps (WorkerW hierarchy change, compatibility hold); test wallpaper mode on 23H2/24H2/25H2.
- macOS `legacyScreenSaver` (Sonoma–Tahoe) leaks: it never deallocates `ScreenSaverView`s. Free GPU resources on stop and exit on `com.apple.screensaver.willstop`.
- GNOME's mutter has at most experimental wlr-layer-shell support, so Wayland wallpaper/overlay target wlroots compositors and KDE first.
- winit 0.31 is still in beta (Nov 2026); use 0.30.

## Next increments

- Work packages live in `docs/SPEC.md` §9; next up is F1 (workspace scaffold + CI).
- Capture mode for screenshots/recordings so PRs can show visuals, including a frame-sequence recorder for GIFs.
- Set numeric idle budgets (CPU %, RSS) and add event-loop guidance (wait/redraw-on-demand, release profile) to the Developer brief once the render stack is chosen.
- Add Linux and Windows CPU/memory measurement commands to the Developer brief.
- Decide on Cargo.lock policy (commit it: binary), an MSRV, and `#![forbid(unsafe_code)]` in the core crate; record them in CLAUDE.md.
