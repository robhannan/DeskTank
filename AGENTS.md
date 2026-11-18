# DeskTank

Lightweight virtual aquarium desktop toy in Rust: wallpaper, screensaver,
lock screen and click-through overlay on macOS, Windows and Linux. Read
`docs/PROGRESS.md` first (status, learnings, next increments) and
`docs/SPEC.md` for the plan and spec (display modes, architecture,
resource budgets, work packages with acceptance criteria).

- Build/test: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`.
- Keep the simulation/rendering core OS-agnostic; platform code lives behind `cfg(target_os = …)` boundaries.
- Minimal resource use is a core requirement: no uncapped redraws; pause when hidden.
- For features and fixes, invoke the named `desktank-developer` agent, which follows the shared workflow (`.agents/skills/desktank-developer/SKILL.md`): one PR per increment, never commit to `main`.
- Toolchain: pinned exactly in `rust-toolchain.toml`; that version is the MSRV (`rust-version` in the root `Cargo.toml`). Bump both together.
- `Cargo.lock` is committed (binary project); CI builds with `--locked`.
- `desktank-sim` and `desktank-render` use `#![forbid(unsafe_code)]`. The workspace denies `unsafe_code`; only `desktank-platform` and `desktank-saver-mac` allow it, at FFI boundaries with `// SAFETY:` comments.
- Lints live in the root `Cargo.toml` (`clippy::pedantic` warn, `-D warnings` in CI); every crate sets `[lints] workspace = true`.
- Web (`crates/desktank-web`, `web/`): `pnpm install && pnpm build:web` builds `target/site/` (needs the wasm-bindgen CLI at the `Cargo.lock` version) and fails over the 3 MB gzip budget; `pnpm test:web` runs the Playwright tests in `tests/web/` (CI runs them), `pnpm bench:web` measures frame intervals. The web is a product target (SPEC §3.5, D15): `desktank-sim` and `desktank-render` must build for `wasm32-unknown-unknown` (CI checks this from T2) and `desktank-render` must stay within wgpu's WebGL2 limits.
- Work in small increments; update `docs/PROGRESS.md` in the same PR.
