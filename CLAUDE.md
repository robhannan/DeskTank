# DeskTank

Lightweight virtual aquarium desktop toy in Rust: wallpaper, screensaver,
lock screen and click-through overlay on macOS, Windows and Linux. Read
`docs/PROGRESS.md` first (status, learnings, next increments).

- Build/test: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`.
- Keep the simulation/rendering core OS-agnostic; platform code lives behind `cfg(target_os = …)` boundaries.
- Minimal resource use is a core requirement: no uncapped redraws; pause when hidden.
- Features are built by the Developer agent (`.claude/agents/developer.md`): one PR per increment, never commit to `main`.
- Work in small increments; update `docs/PROGRESS.md` in the same PR.
