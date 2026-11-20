# DeskTank

Lightweight virtual aquarium desktop toy in Rust: wallpaper, screensaver,
lock screen and click-through overlay on macOS, Windows and Linux. It also
runs in the browser (the web) and on Android, led by the Android TV
screensaver (SPEC §3.6, D23). Read
`docs/PROGRESS.md` first (status, learnings, next increments) and
`docs/SPEC.md` for the plan and spec (display modes, architecture,
resource budgets, work packages with acceptance criteria).

- Build/test: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`.
- Keep the simulation/rendering core OS-agnostic; platform code lives behind `cfg(target_os = …)` boundaries.
- Minimal resource use is a core requirement: no uncapped redraws; pause when hidden.
- For features and fixes, invoke the named `desktank-developer` agent, which follows the shared workflow (`.agents/skills/desktank-developer/SKILL.md`): one PR per increment, never commit to `main`.
- Toolchain: pinned exactly in `rust-toolchain.toml`; that version is the MSRV (`rust-version` in the root `Cargo.toml`). Bump both together.
- `Cargo.lock` is committed (binary project); CI builds with `--locked`.
- `desktank-sim` and `desktank-render` use `#![forbid(unsafe_code)]`. The workspace denies `unsafe_code`; only `desktank-platform`, `desktank-saver-mac` and `desktank-android` allow it, at FFI (JNI/NDK) boundaries with `// SAFETY:` comments.
- Lints live in the root `Cargo.toml` (`clippy::pedantic` warn, `-D warnings` in CI); every crate sets `[lints] workspace = true`.
- Web (`crates/desktank-web`, `web/`): `pnpm install && pnpm build:web` builds `target/site/` (needs the wasm-bindgen CLI at the `Cargo.lock` version) and fails over the 3 MB gzip budget; `pnpm test:web` runs the Playwright tests in `tests/web/` (CI runs them), `pnpm bench:web` measures frame intervals, `pnpm typecheck` runs `tsc` over the page, scripts and tests (CI runs it; after one `pnpm build:web`, which writes wasm-bindgen's declarations). The web is a product target (SPEC §3.5, D15): `desktank-sim` and `desktank-render` must build for `wasm32-unknown-unknown` (CI checks this from T2) and `desktank-render` must stay within wgpu's WebGL2 limits.
- Non-Rust code is TypeScript (`.ts`) wherever possible; JS only where a tool genuinely cannot take TS. The one other exception is the Android shim in `android/`, which is Kotlin (Rust can't subclass `DreamService` and friends; keep it thin, with no tank state). Strict `tsconfig.json`, erasable syntax only: Node runs `scripts/*.ts` by type stripping (relative imports keep their `.ts` extension), Playwright loads the tests directly, and `scripts/build-web.ts` emits `web/*.ts` to readable `.js` with `tsc` (`web/tsconfig.json`).
- Android (`crates/desktank-android`, `android/`): `cd android && ./gradlew assembleDebug` builds `libdesktank_android.so` with `cargo-ndk` and the debug APK (needs JDK 17+, the Android SDK with the NDK pinned in `android/gradle/libs.versions.toml`, `cargo install --locked cargo-ndk` and the `rustup` targets `aarch64-linux-android armv7-linux-androideabi x86_64-linux-android`; `-Pdesktank.abis=arm64-v8a` builds one ABI). The JNI glue compiles only for Android: run `cargo clippy -p desktank-android --target aarch64-linux-android` (CI does).
- Work in small increments; update `docs/PROGRESS.md` in the same PR.
