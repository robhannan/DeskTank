# Progress

## Status

Repository created; no code yet.

## Done

- Repository, licences (MIT OR Apache-2.0), README.
- Developer agent brief (`.claude/agents/developer.md`) and `CLAUDE.md`.

## Learnings

- Lock screens are restricted: macOS allows no custom lock screen and Windows only a picture, slideshow or Spotlight (no live rendering), so "lock screen" mode may mean the screensaver shown while locked.

## Next increments

- Scaffold the Cargo workspace (2024 edition, pinned `rust-toolchain.toml`, workspace lints, rustfmt config) with an OS-agnostic core crate.
- CI on macOS, Windows and Linux: fmt, clippy, test, `cargo deny`; Dependabot.
- Overlay mode: transparent, click-through, always-on-top window.
- Wallpaper mode, screensaver packaging (`.saver` / `.scr` / XScreenSaver), lock-screen mode.
- Capture mode for screenshots/recordings so PRs can show visuals, including a frame-sequence recorder for GIFs.
- Set numeric idle budgets (CPU %, RSS) and add event-loop guidance (wait/redraw-on-demand, release profile) to the Developer brief once the render stack is chosen.
- Add Linux and Windows CPU/memory measurement commands to the Developer brief.
- Decide on Cargo.lock policy (commit it: binary), an MSRV, and `#![forbid(unsafe_code)]` in the core crate; record them in CLAUDE.md.
