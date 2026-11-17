# Progress

## Status

Repository created; no code yet.

## Done

- Repository, licences (MIT OR Apache-2.0), README.
- Developer agent brief (`.claude/agents/developer.md`) and `CLAUDE.md`.

## Learnings

- Lock screens are restricted: macOS allows no custom lock screen and Windows only a static image, so "lock screen" mode may mean the screensaver shown while locked.

## Next increments

- Scaffold the Cargo workspace (2024 edition, pinned `rust-toolchain.toml`, workspace lints, rustfmt config) with an OS-agnostic core crate.
- CI on macOS, Windows and Linux: fmt, clippy, test, `cargo deny`; Dependabot.
- Overlay mode: transparent, click-through, always-on-top window.
- Wallpaper mode, screensaver packaging (`.saver` / `.scr` / XScreenSaver), lock-screen mode.
- Capture mode for screenshots/recordings so PRs can show visuals.
